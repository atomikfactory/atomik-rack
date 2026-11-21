//! What drives the main thread: a thread that reads the parent's frames from stdin, a ticker,
//! and the loop that hands both to the [`Control`]. The loop is a winit event loop (the editor
//! window needs one) when the system has a display, a plain channel loop otherwise.

use crate::control::{Control, Event};
use crate::editor::{NewWindow, WindowSystem};
use atomik_plugin_host::NativeParent;
use atomik_plugin_ipc::lifetime::exit_after;
use atomik_plugin_ipc::{ToChild, read_frame};
use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError, mpsc};
use std::time::Duration;

/// How often the plugin is polled and its messages are forwarded while there is something going
/// on (an editor is open, the parent just called, the plugin just posted something).
pub const TICK: Duration = Duration::from_millis(16);

/// How often it is polled when nothing is going on.
pub const TICK_IDLE: Duration = Duration::from_millis(100);

/// How many controls of this process want the fast tick.
static FAST: AtomicUsize = AtomicUsize::new(0);

/// The ticker thread, to be woken when the tick turns fast again.
static TICKER: Mutex<Option<std::thread::Thread>> = Mutex::new(None);

/// One control's vote for the tick rate: fast while an editor is open or the control has just
/// worked, slow otherwise. The process ticks fast while any control votes for it (a group
/// process holds several). A new vote is fast, so the first ticks come quickly.
#[derive(Debug)]
pub struct TickVote {
    fast: bool,
}

impl TickVote {
    /// A vote for the fast tick.
    pub fn new() -> Self {
        FAST.fetch_add(1, Ordering::AcqRel);
        Self { fast: true }
    }

    /// Votes fast or slow. Turning fast when the process was ticking slowly wakes the ticker, so
    /// the next tick does not wait for the rest of the slow interval.
    pub fn set(&mut self, fast: bool) {
        if fast == self.fast {
            return;
        }
        self.fast = fast;
        if fast {
            if FAST.fetch_add(1, Ordering::AcqRel) == 0
                && let Some(ticker) = TICKER
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
            {
                ticker.unpark();
            }
        } else {
            FAST.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// The vote now.
    #[cfg(test)]
    pub fn is_fast(&self) -> bool {
        self.fast
    }
}

impl Default for TickVote {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TickVote {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// The interval the ticker waits now.
pub fn tick_interval() -> Duration {
    if FAST.load(Ordering::Acquire) > 0 {
        TICK
    } else {
        TICK_IDLE
    }
}

/// What the ticker thread does between two ticks: waits [`tick_interval`], or less when a control
/// votes fast meanwhile.
pub fn tick_pause() {
    {
        let mut ticker = TICKER.lock().unwrap_or_else(PoisonError::into_inner);
        if ticker.is_none() {
            *ticker = Some(std::thread::current());
        }
    }
    std::thread::park_timeout(tick_interval());
}

/// How long the main thread has to end the process after the control channel closed, before a
/// watchdog thread ends it anyway.
pub const EOF_GRACE: Duration = Duration::from_secs(3);

/// Where the helper threads deliver events.
pub trait Post: Clone + Send + 'static {
    /// Delivers `event`; false when the loop is gone.
    fn post(&self, event: Event) -> bool;
}

impl Post for mpsc::Sender<Event> {
    fn post(&self, event: Event) -> bool {
        self.send(event).is_ok()
    }
}

/// Reads frames from stdin until it ends, then posts `Eof`.
pub fn spawn_reader<P: Post>(post: P) {
    let started = std::thread::Builder::new()
        .name("host-stdin".to_string())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            loop {
                match read_frame::<ToChild>(&mut stdin) {
                    Ok(frame) => {
                        if !post.post(Event::Frame(Box::new(frame))) {
                            return;
                        }
                    }
                    Err(e) => {
                        if e.kind() != ErrorKind::UnexpectedEof {
                            eprintln!("atomik-plugin-host: the control channel broke: {e}");
                        }
                        post.post(Event::Eof);
                        // The main thread ends the process on `Eof`; when it is stuck inside a
                        // plugin call it cannot, and this does.
                        exit_after(EOF_GRACE);
                        return;
                    }
                }
            }
        });
    if let Err(e) = started {
        eprintln!("atomik-plugin-host: cannot start the stdin thread: {e}");
    }
}

/// Posts a tick every [`tick_interval`], never more than one that has not been handled.
pub fn spawn_ticker<P: Post>(post: P, pending: Arc<AtomicBool>) {
    let started = std::thread::Builder::new()
        .name("host-ticker".to_string())
        .spawn(move || {
            loop {
                tick_pause();
                if !pending.swap(true, Ordering::AcqRel) && !post.post(Event::Tick) {
                    return;
                }
            }
        });
    if let Err(e) = started {
        eprintln!("atomik-plugin-host: cannot start the ticker thread: {e}");
    }
}

/// The window system of a child with no display: every window request fails.
pub struct NoWindows;

impl WindowSystem for NoWindows {
    fn create(
        &mut self,
        _title: &str,
        _owner: Option<NativeParent>,
        _size: (u32, u32),
        _embed: bool,
    ) -> Result<NewWindow, String> {
        Err("this process has no display to open an editor window on".to_string())
    }
    fn resize(&mut self, _w: u32, _h: u32) {}
    fn show(&mut self) {}
    fn hide(&mut self) {}
    fn destroy(&mut self) {}
}

/// Runs the plain loop: no windows. Returns the exit code.
pub fn run_plain(mut control: Control) -> u8 {
    let (tx, rx) = mpsc::channel();
    let pending = Arc::new(AtomicBool::new(false));
    spawn_reader(tx.clone());
    spawn_ticker(tx, Arc::clone(&pending));
    while let Ok(event) = rx.recv() {
        if matches!(event, Event::Tick) {
            pending.store(false, Ordering::Release);
        }
        if let Some(code) = control.on_event(event, &mut NoWindows) {
            return code;
        }
    }
    0
}
