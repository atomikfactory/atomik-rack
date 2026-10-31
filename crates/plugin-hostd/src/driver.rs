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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// How often the plugin is polled and its messages are forwarded.
pub const TICK: Duration = Duration::from_millis(16);

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

/// Posts a tick every [`TICK`], never more than one that has not been handled.
pub fn spawn_ticker<P: Post>(post: P, pending: Arc<AtomicBool>) {
    let started = std::thread::Builder::new()
        .name("host-ticker".to_string())
        .spawn(move || {
            loop {
                std::thread::sleep(TICK);
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
