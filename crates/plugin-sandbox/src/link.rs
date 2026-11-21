//! The control link to one child: a writer thread, a reader thread and a stderr thread around a
//! [`Transport`], and the one-slot mailbox that turns the child's replies into answers for the
//! main thread's calls.
//!
//! Only the main thread makes calls and only one is in flight at a time, so a single slot is
//! enough; replies are matched by request id and anything else is ignored. A call that matters
//! and is not answered in time kills the child, unless the child shows a sign of life (its audio
//! thread still bumps the counter of the block, see [`ALIVE_WINDOW`]): a live plugin whose main
//! thread is busy is never stopped for being slow. Its call fails, the link is then stuck on
//! that request (every call fails at once without being sent, see [`Link::stuck`]) and
//! recovers by itself when the reader thread sees the late answer. A cosmetic call (see
//! [`Link::soft_call`]) only drops its request. One request at a time can instead be a
//! background request (see
//! [`Link::begin_background`]): it has a slot of its own, so the state of a plugin whose main
//! thread is busy can be asked for while the UI goes on, and nothing times it out. Once the
//! child is killed the link is lost, and every call fails at once without any I/O. The reader
//! thread is the one that learns that the child ended: it reaps the exit status, reads the
//! stderr tail, marks the audio block dead (so the engine side sees
//! [`atomik_plugin_host::ProcessStatus::Lost`]) and posts [`PluginMessage::ProcessExited`].

use crate::clock::now;
use crate::transport::{ChildProcess, Transport};
use atomik_plugin_host::{
    BackendError, HUNG, HostMessenger, LOST, LogLevel, NOT_RESPONDING, ParamPhase, PluginMessage,
    STALL_THRESHOLD, StallOutcome,
};
use atomik_plugin_ipc::{
    DeathHandle, FromChild, MAX_FRAME_BYTES, MAX_GUI_DIMENSION, MAX_LOG_BYTES,
    MAX_REPLY_JSON_BYTES, PANIC_LOG_PREFIX, PROTOCOL_VERSION, ToChild, WireLayout,
    into_backend_error, read_frame_with, write_frame,
};
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// How much of the child's stderr is kept for the crash reason.
pub(crate) const STDERR_TAIL_BYTES: usize = 2048;

/// How long a child gets to end after its stdout closed before it is killed.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// How long the reader waits for the stderr thread to see the end of the stream.
const STDERR_GRACE: Duration = Duration::from_millis(300);

/// How long a dropped link waits for the child to end after `Shutdown`.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

/// How many cosmetic calls in a row may go unanswered before the instance counts as slow (the
/// app then asks less often and says so).
const MISSES_TO_SLOW: u32 = 3;

/// How many cosmetic calls in a row may go unanswered before the child is stopped: a plugin
/// that is merely busy answers sooner or later, one that never does is wedged.
const MISSES_TO_KILL: u32 = 12;

/// How recently the child's audio thread must have shown a sign of life for the child to count
/// as alive when a call times out: the counter in the block header has to change within this
/// window (the child's audio loop comes round every few milliseconds).
pub(crate) const ALIVE_WINDOW: Duration = Duration::from_millis(500);

/// The most parameter edits held back for the main thread at once.
const MAX_HELD_EDITS: usize = 4096;

/// The timeouts of the main-thread calls.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timeouts {
    /// Loading: the Hello, `Instantiate` and opening the editor. A plugin may check its licence
    /// online or show its own sign-in dialogs then (and a firewall may ask the user first), so
    /// this is minutes, not seconds; the waits are polled, so the app stays responsive.
    pub load: Duration,
    /// Activate, capture and restore state: a call that is not answered in time kills the
    /// child, unless it shows a sign of life (then the instance is stuck, see [`Link::stuck`]).
    pub create: Duration,
    /// Every other call that matters (deactivate, flush): killing as well.
    pub call: Duration,
    /// Cosmetic calls (param text, pins, layout, closing the editor): an answer that is late is
    /// dropped and the child is left alone; see [`Link::soft_call`].
    pub text: Duration,
    /// Offline audio batches.
    pub watchdog: Duration,
    /// Live audio: how long a child may answer no batch at all before it counts as hung.
    pub hung: Duration,
    /// How long a slow instance is left alone between two cosmetic calls.
    pub slow_retry: Duration,
    /// A bounce: every batch waits for the child whatever mode the plugin is told.
    pub bounce: bool,
}

/// Counts the children that are alive; the reader thread holds one guard per child.
#[derive(Debug)]
pub(crate) struct LiveGuard(Arc<AtomicUsize>);

impl LiveGuard {
    pub fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(counter))
    }
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the waiting call expects next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Nothing,
    Hello,
    Id(u64),
}

/// A child's reply: the message and the blob that followed it.
#[derive(Debug)]
pub(crate) struct Reply {
    pub msg: FromChild,
    pub blob: Option<Vec<u8>>,
}

#[derive(Debug)]
struct Mailbox {
    expect: Expect,
    reply: Option<Reply>,
    /// The id of the background request that is out, if any (see [`Link::begin_background`]).
    bg_id: Option<u64>,
    /// The answer to it, until [`Link::poll_background`] takes it.
    bg_reply: Option<Reply>,
    /// The request a timed-out call left unanswered while the child was alive: the link is
    /// stuck on it until its answer comes (see [`Shared::enter_stuck`]).
    stuck: Option<Stuck>,
    /// The child's stdout closed: nothing more will come.
    ended: bool,
    /// Set once the child is reaped: why it ended.
    reason: Option<String>,
}

/// A request that was not answered in time by a child that is alive.
#[derive(Debug, Clone)]
struct Stuck {
    id: u64,
    /// The name of the call.
    what: String,
}

enum Waited {
    Reply(Reply),
    Ended,
    /// The time ran out. `alive`: the child's audio thread showed a sign of life in the last
    /// part of the wait (see [`ALIVE_WINDOW`]).
    TimedOut {
        alive: bool,
    },
}

/// Takes the answer of the background request that is out, ending it.
fn take_background(mb: &mut Mailbox) -> Option<Reply> {
    let reply = mb.bg_reply.take()?;
    mb.bg_id = None;
    Some(reply)
}

/// What the main thread and the helper threads share.
pub(crate) struct Shared {
    mailbox: Mutex<Mailbox>,
    cv: Condvar,
    /// The child is gone or condemned: no more I/O.
    lost: AtomicBool,
    /// The instance is being dropped on purpose: no `ProcessExited`.
    closing: AtomicBool,
    kill_reason: Mutex<Option<String>>,
    child: Mutex<Box<dyn ChildProcess>>,
    death: DeathHandle,
    stderr_tail: Mutex<Vec<u8>>,
    stderr_done: AtomicBool,
    pid: AtomicU32,
    messenger: HostMessenger,
    /// The audio side saw the child answer no batch for too long (shared with the processor,
    /// which only sets it).
    hung: Arc<AtomicBool>,
    /// A layout change whose call had given up before the child answered it.
    late_layout: Mutex<Option<WireLayout>>,
    /// The newest value of each param the plugin edited since the main thread last looked: a
    /// burst of edits is handed on as one per param, in order with the gesture's begin and end.
    edits: Mutex<BTreeMap<u32, f64>>,
    /// Notifications refused because they were out of bounds.
    rejected: AtomicU64,
}

impl Shared {
    fn expect(&self, what: Expect) {
        let mut mb = lock(&self.mailbox);
        mb.expect = what;
        mb.reply = None;
    }

    fn wait(&self, timeout: Duration) -> Waited {
        self.wait_for(timeout, |mb| {
            let reply = mb.reply.take()?;
            mb.expect = Expect::Nothing;
            Some(reply)
        })
    }

    /// Waits until `take` finds a reply in the mailbox, the child ends or `timeout` passes.
    fn wait_for(
        &self,
        timeout: Duration,
        mut take: impl FnMut(&mut Mailbox) -> Option<Reply>,
    ) -> Waited {
        let start = now();
        // The counter is read twice, at the start and at the end of the last `window` of the
        // wait: the child counts as alive when it moved in between. No time is added.
        let window = ALIVE_WINDOW.min(timeout / 2);
        let sample_at = timeout - window;
        let mut before = None;
        let mut mb = lock(&self.mailbox);
        loop {
            if let Some(reply) = take(&mut mb) {
                return Waited::Reply(reply);
            }
            if mb.ended {
                return Waited::Ended;
            }
            let elapsed = now().saturating_duration_since(start);
            if before.is_none() && elapsed >= sample_at {
                before = Some(self.death.pulse());
            }
            let left = timeout.saturating_sub(elapsed);
            if left.is_zero() {
                let alive = before.is_some_and(|b| self.death.pulse() != b);
                return Waited::TimedOut { alive };
            }
            let step = if before.is_none() {
                sample_at.saturating_sub(elapsed).min(left)
            } else {
                left
            };
            mb = self
                .cv
                .wait_timeout(mb, step)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The child's sign of life: changes while its audio thread runs.
    fn pulse(&self) -> u64 {
        self.death.pulse()
    }

    /// Leaves the request `id`, which a timed-out call gave up on, as the one the link is stuck
    /// on: until its answer arrives every call fails at once. When the answer is already there
    /// (it came after the wait ended) it is returned instead and the link is not stuck. With
    /// `background` the request is the background one, whose slot is emptied.
    fn enter_stuck(&self, id: u64, what: &str, background: bool) -> Option<Reply> {
        let mut mb = lock(&self.mailbox);
        let reply = if background {
            let reply = mb.bg_reply.take().filter(|_| mb.bg_id == Some(id));
            mb.bg_id = None;
            mb.bg_reply = None;
            reply
        } else if mb.expect == Expect::Id(id) {
            mb.expect = Expect::Nothing;
            mb.reply.take()
        } else {
            None
        };
        if reply.is_none() {
            mb.stuck = Some(Stuck {
                id,
                what: what.to_string(),
            });
        }
        reply
    }

    /// Ends the stuck state when `id` is the request it waits for (true: the reply is of no
    /// use to anyone else).
    fn recover(&self, id: u64) -> bool {
        let mut mb = lock(&self.mailbox);
        if mb.stuck.as_ref().is_some_and(|s| s.id == id) {
            mb.stuck = None;
            true
        } else {
            false
        }
    }

    /// The name of the call the link is stuck on: the plugin's process is alive but its main
    /// thread has not answered it. `None` when it is not stuck or the process is gone.
    pub fn stuck(&self) -> Option<String> {
        if self.is_lost() {
            return None;
        }
        lock(&self.mailbox).stuck.as_ref().map(|s| s.what.clone())
    }

    /// What the wait would return now, without waiting.
    fn try_take(&self) -> Option<Waited> {
        let mut mb = lock(&self.mailbox);
        if let Some(reply) = mb.reply.take() {
            mb.expect = Expect::Nothing;
            return Some(Waited::Reply(reply));
        }
        mb.ended.then_some(Waited::Ended)
    }

    /// Waits until the child is reaped and returns why it ended.
    pub fn wait_reason(&self, timeout: Duration) -> Option<String> {
        let start = now();
        let mut mb = lock(&self.mailbox);
        loop {
            if let Some(reason) = &mb.reason {
                return Some(reason.clone());
            }
            let left = timeout.saturating_sub(now().saturating_duration_since(start));
            if left.is_zero() {
                return None;
            }
            mb = self
                .cv
                .wait_timeout(mb, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Hands `reply` to the call that waits for it; false when nobody does (it gave up).
    fn deliver(&self, reply: Reply, wanted: Expect) -> bool {
        let mut mb = lock(&self.mailbox);
        if mb.expect == wanted {
            mb.reply = Some(reply);
            self.cv.notify_all();
            true
        } else {
            false
        }
    }

    /// Hands `reply` to the background request when `id` is the one that is out (true; the reply
    /// is taken), else leaves it where it is.
    fn deliver_background(&self, reply: &mut Option<Reply>, id: u64) -> bool {
        let mut mb = lock(&self.mailbox);
        if mb.bg_id != Some(id) {
            return false;
        }
        mb.bg_reply = reply.take();
        self.cv.notify_all();
        true
    }

    fn expect_background(&self, id: u64) {
        let mut mb = lock(&self.mailbox);
        mb.bg_id = Some(id);
        mb.bg_reply = None;
    }

    fn clear_background(&self) {
        let mut mb = lock(&self.mailbox);
        mb.bg_id = None;
        mb.bg_reply = None;
    }

    fn background_out(&self) -> bool {
        lock(&self.mailbox).bg_id.is_some()
    }

    /// The id of the background request that is out.
    fn background_id(&self) -> Option<u64> {
        lock(&self.mailbox).bg_id
    }

    /// What the background request has now, without waiting: its answer, or that the child
    /// ended (which also ends the request).
    fn try_take_background(&self) -> Option<Waited> {
        let mut mb = lock(&self.mailbox);
        if let Some(reply) = take_background(&mut mb) {
            return Some(Waited::Reply(reply));
        }
        if mb.ended {
            mb.bg_id = None;
            return Some(Waited::Ended);
        }
        None
    }

    /// The flag the audio side sets when the child answers no batch.
    pub fn hung_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.hung)
    }

    /// A layout change that arrived after its call had given up (taken once).
    pub fn take_late_layout(&self) -> Option<WireLayout> {
        lock(&self.late_layout).take()
    }

    /// Notifications refused so far for being out of bounds.
    #[cfg(test)]
    pub fn rejected(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }

    /// Hands the held param edits on to the main thread's message ring.
    pub fn flush_edits(&self) {
        let mut edits = lock(&self.edits);
        self.post_edits(&mut edits);
    }

    fn post_edits(&self, edits: &mut BTreeMap<u32, f64>) {
        for (id, value) in std::mem::take(edits) {
            self.messenger.post(PluginMessage::ParamEdit {
                id,
                value,
                phase: ParamPhase::Change,
            });
        }
    }

    fn param_edit(&self, param: u32, value: f64, phase: ParamPhase) {
        let mut edits = lock(&self.edits);
        if phase == ParamPhase::Change {
            if edits.len() >= MAX_HELD_EDITS && !edits.contains_key(&param) {
                self.post_edits(&mut edits);
            }
            edits.insert(param, value);
            return;
        }
        // A gesture's begin and end keep their place around its last change.
        if let Some(held) = edits.remove(&param) {
            self.messenger.post(PluginMessage::ParamEdit {
                id: param,
                value: held,
                phase: ParamPhase::Change,
            });
        }
        self.messenger.post(PluginMessage::ParamEdit {
            id: param,
            value,
            phase,
        });
    }

    /// Posts a log line of the app's own about this child.
    fn warn(&self, text: String) {
        self.messenger.post(PluginMessage::Log {
            level: LogLevel::Warning,
            text,
        });
    }

    /// Waits like [`Shared::wait`] and, when the wait took at least [`STALL_THRESHOLD`], tells
    /// the app which call held the main thread up and how it ended. Only the main thread's
    /// blocking calls go through here.
    fn wait_noted(&self, what: &str, timeout: Duration) -> Waited {
        self.noted(what, |shared| shared.wait(timeout))
    }

    /// [`Shared::wait_noted`] for the answer to the background request that is out.
    fn wait_background_noted(&self, what: &str, timeout: Duration) -> Waited {
        self.noted(what, |shared| shared.wait_for(timeout, take_background))
    }

    fn noted(&self, what: &str, wait: impl FnOnce(&Self) -> Waited) -> Waited {
        let start = now();
        let waited = wait(self);
        let elapsed = now().saturating_duration_since(start);
        if elapsed >= STALL_THRESHOLD {
            let outcome = match waited {
                Waited::Reply(_) => StallOutcome::Answered,
                Waited::Ended => StallOutcome::ChildGone,
                Waited::TimedOut { .. } => StallOutcome::TimedOut,
            };
            self.messenger.post(PluginMessage::Stall {
                call: what.to_string(),
                ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                outcome,
            });
        }
        waited
    }

    /// Condemns the child: remembers `reason` (the first one wins), makes every later call
    /// fail at once, ends every audio wait and kills the process. Does not wait for it.
    pub fn kill(&self, reason: String) {
        {
            let mut r = lock(&self.kill_reason);
            if r.is_none() {
                *r = Some(reason);
            }
        }
        self.lost.store(true, Ordering::SeqCst);
        self.death.mark_dead();
        lock(&self.child).kill();
    }

    /// True when the child is gone, condemned, or the audio side declared it dead.
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst) || self.death.is_dead()
    }

    /// The audio side may only mark the block dead (it must not lock): the main thread
    /// finishes the job here by killing the child, once.
    pub fn finish_audio_death(&self) {
        if self.death.is_dead() && !self.lost.load(Ordering::SeqCst) {
            if self.hung.load(Ordering::Acquire) {
                self.kill(format!("{HUNG} and was stopped"));
            } else {
                self.kill(
                    "the plugin process stopped answering audio requests and was stopped"
                        .to_string(),
                );
            }
        }
    }

    /// The child's process id (0 before its Hello).
    pub fn pid(&self) -> u32 {
        self.pid.load(Ordering::Relaxed)
    }

    fn mark_ended(&self) {
        self.lost.store(true, Ordering::SeqCst);
        self.death.mark_dead();
        let mut mb = lock(&self.mailbox);
        mb.ended = true;
        self.cv.notify_all();
    }

    fn reap(&self) -> String {
        let start = now();
        let mut killed = false;
        loop {
            if let Some(status) = lock(&self.child).try_wait() {
                return status;
            }
            if now().saturating_duration_since(start) >= EXIT_GRACE {
                if killed {
                    return "the plugin process did not end".to_string();
                }
                // Its stdout closed but it lives on: it is of no use to anyone.
                lock(&self.child).kill();
                killed = true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn finish(&self, reason: String) {
        let mut mb = lock(&self.mailbox);
        mb.reason = Some(reason);
        self.cv.notify_all();
    }

    fn compose_reason(&self, status: String) -> String {
        // Why the app stopped the child (when it did) and how the process ended, both: the
        // exit code tells a kill from a crash that came first.
        let head = match lock(&self.kill_reason).take() {
            Some(kill) => format!("{kill} ({status})"),
            None => status,
        };
        // A process shared with other instances says so: all of them were lost with it.
        let head = match lock(&self.child).note() {
            Some(note) => format!("{head}; {note}"),
            None => head,
        };
        let tail = {
            let tail = lock(&self.stderr_tail);
            String::from_utf8_lossy(&tail).trim().to_string()
        };
        if tail.is_empty() {
            head
        } else {
            format!("{head}: {tail}")
        }
    }

    fn route(&self, msg: FromChild, blob: Option<Vec<u8>>) {
        let post = |m: PluginMessage| {
            self.messenger.post(m);
        };
        let id = match &msg {
            FromChild::Hello { pid, .. } => {
                self.pid.store(*pid, Ordering::Relaxed);
                self.deliver(Reply { msg, blob }, Expect::Hello);
                return;
            }
            FromChild::Ok { id }
            | FromChild::Err { id, .. }
            | FromChild::Instantiated { id, .. }
            | FromChild::Activated { id, .. }
            | FromChild::State { id }
            | FromChild::Text { id, .. }
            | FromChild::Latency { id, .. }
            | FromChild::Pins { id, .. }
            | FromChild::Presets { id, .. }
            | FromChild::ParamValues { id, .. }
            | FromChild::LayoutChanged { id, .. }
            | FromChild::GuiOpened { id, .. }
            | FromChild::GuiSized { id, .. } => *id,
            FromChild::ParamEdit {
                param,
                value,
                phase,
            } => return self.param_edit(*param, *value, ParamPhase::from(*phase)),
            FromChild::StateDirty => return post(PluginMessage::StateDirty),
            FromChild::LatencyChanged => return post(PluginMessage::LatencyChanged),
            FromChild::RescanParams => return post(PluginMessage::RescanParams),
            FromChild::RequestRestart => return post(PluginMessage::RequestRestart),
            FromChild::GuiResize { w, h } => {
                if *w > MAX_GUI_DIMENSION || *h > MAX_GUI_DIMENSION {
                    self.rejected.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                return post(PluginMessage::GuiResize { w: *w, h: *h });
            }
            FromChild::GuiFrame { x, y, w, h } => {
                if *w > MAX_GUI_DIMENSION || *h > MAX_GUI_DIMENSION {
                    self.rejected.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                return post(PluginMessage::GuiFrame {
                    x: *x,
                    y: *y,
                    w: *w,
                    h: *h,
                });
            }
            FromChild::GuiClosed => return post(PluginMessage::GuiClosed),
            FromChild::GuiRequestShow => return post(PluginMessage::GuiRequestShow),
            FromChild::GuiRequestHide => return post(PluginMessage::GuiRequestHide),
            FromChild::Log { level, text } => {
                if text.len() > MAX_LOG_BYTES {
                    self.rejected.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                let mut text = text.clone();
                if text.starts_with(PANIC_LOG_PREFIX) {
                    // The child's own words about the panic are the most useful thing next to it.
                    text = with_stderr_tail(text, &lock(&self.stderr_tail));
                }
                return post(PluginMessage::Log {
                    level: LogLevel::from(*level),
                    text,
                });
            }
        };
        let late_layout = match &msg {
            FromChild::LayoutChanged {
                layout: Some(layout),
                ..
            } => Some(layout.clone()),
            _ => None,
        };
        if self.recover(id) {
            // The answer a stuck link waited for: the call that asked gave up long ago.
            return;
        }
        let mut reply = Some(Reply { msg, blob });
        if self.deliver_background(&mut reply, id) {
            return;
        }
        let Some(reply) = reply else {
            return;
        };
        if !self.deliver(reply, Expect::Id(id))
            && let Some(layout) = late_layout
        {
            // The call that asked has given up, and the child will not say it again.
            *lock(&self.late_layout) = Some(layout);
        }
    }
}

fn run_reader(shared: &Shared, mut reader: Box<dyn Read + Send>, live: LiveGuard) {
    let bad = loop {
        // Replies are small and only the state has a blob: both limits are checked before
        // anything is allocated, so a hostile child cannot make the app grow.
        let read = read_frame_with::<FromChild>(&mut reader, MAX_REPLY_JSON_BYTES, |m| {
            if matches!(m, FromChild::State { .. }) {
                MAX_FRAME_BYTES
            } else {
                0
            }
        });
        match read {
            Ok((msg, blob)) => shared.route(msg, blob),
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => break None,
            Err(e) => break Some(e),
        }
    };
    shared.mark_ended();
    if let Some(e) = bad {
        shared.kill(format!(
            "the plugin process sent a message the app cannot read: {e}"
        ));
    }
    let status = shared.reap();
    let start = now();
    while !shared.stderr_done.load(Ordering::Acquire)
        && now().saturating_duration_since(start) < STDERR_GRACE
    {
        std::thread::sleep(Duration::from_millis(2));
    }
    let reason = shared.compose_reason(status);
    // Posted before the reason is published, so that a caller that learns the reason and then
    // drops the instance cannot race this check.
    if !shared.closing.load(Ordering::SeqCst) {
        shared.messenger.post(PluginMessage::ProcessExited {
            reason: reason.clone(),
        });
    }
    drop(live);
    shared.finish(reason);
}

fn run_writer(rx: &Receiver<Vec<u8>>, mut writer: Box<dyn Write + Send>) {
    while let Ok(bytes) = rx.recv() {
        if writer
            .write_all(&bytes)
            .and_then(|()| writer.flush())
            .is_err()
        {
            break;
        }
    }
}

fn run_stderr(shared: &Shared, mut stderr: Box<dyn Read + Send>) {
    let mut buf = [0u8; 1024];
    loop {
        match stderr.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut tail = lock(&shared.stderr_tail);
                tail.extend_from_slice(&buf[..n]);
                let over = tail.len().saturating_sub(STDERR_TAIL_BYTES);
                tail.drain(..over);
            }
        }
    }
    shared.stderr_done.store(true, Ordering::Release);
}

/// How many of the child's last stderr lines a panic line carries.
const PANIC_STDERR_LINES: usize = 3;

/// `text` followed by the last lines of the child's stderr (none when it wrote nothing).
pub(crate) fn with_stderr_tail(text: String, tail: &[u8]) -> String {
    let tail = String::from_utf8_lossy(tail);
    let lines: Vec<&str> = tail
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let from = lines.len().saturating_sub(PANIC_STDERR_LINES);
    if lines.len() == from {
        return text;
    }
    format!("{text} [child stderr: {}]", lines[from..].join(" | "))
}

fn lost() -> BackendError {
    BackendError::Failed(LOST.to_string())
}

/// The link of one remote instance.
pub(crate) struct Link {
    shared: Arc<Shared>,
    tx: Option<Sender<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    next_id: u64,
    timeouts: Timeouts,
    /// Cosmetic calls in a row that were not answered in time.
    misses: u32,
    /// When the last cosmetic call went unanswered.
    last_miss: Option<std::time::Instant>,
    /// When the child was asked to end (set once; the drop waits from then on).
    shutdown_at: Option<std::time::Instant>,
    /// A request sent by [`Link::begin_call`] whose answer [`Link::finish_call`] has not taken.
    pending: bool,
    /// A request sent by [`Link::begin_held_call`] whose answer may take minutes: until
    /// [`Link::poll_reply`] has it, every other call fails at once without being sent, so it
    /// neither abandons that request nor counts as a miss of a busy child.
    held: Option<&'static str>,
    /// What the main thread saw of the child's sign of life between polls (see
    /// [`Link::alive_by_polling`]).
    vitals: Option<Vitals>,
}

/// The child's sign of life as the polled waits see it.
#[derive(Debug, Clone, Copy)]
struct Vitals {
    /// The counter at the last sample.
    pulse: u64,
    /// When it was sampled.
    sampled: std::time::Instant,
    /// When it was last seen to change.
    changed: Option<std::time::Instant>,
}

/// The error of a call made on a link that is stuck on `call`.
fn not_responding(call: &str, what: &str) -> BackendError {
    BackendError::Failed(format!(
        "{NOT_RESPONDING}: it has not answered {call} yet; {what} was not sent"
    ))
}

impl Link {
    /// Starts the helper threads over `transport` and sends the Hello; the greeting is awaited
    /// by [`Link::hello`] or polled by [`Link::poll_reply`].
    ///
    /// # Errors
    /// The child did not greet in time, spoke another protocol, or ended first (the error
    /// then carries the exit description and the stderr tail).
    pub fn launch(
        transport: Transport,
        death: DeathHandle,
        timeouts: Timeouts,
        messenger: HostMessenger,
        live: LiveGuard,
    ) -> Result<Self, BackendError> {
        let Transport {
            writer,
            reader,
            stderr,
            child,
        } = transport;
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox {
                expect: Expect::Hello,
                reply: None,
                bg_id: None,
                bg_reply: None,
                stuck: None,
                ended: false,
                reason: None,
            }),
            cv: Condvar::new(),
            lost: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            kill_reason: Mutex::new(None),
            child: Mutex::new(child),
            death,
            stderr_tail: Mutex::new(Vec::new()),
            stderr_done: AtomicBool::new(stderr.is_none()),
            pid: AtomicU32::new(0),
            messenger,
            hung: Arc::new(AtomicBool::new(false)),
            late_layout: Mutex::new(None),
            edits: Mutex::new(BTreeMap::new()),
            rejected: AtomicU64::new(0),
        });
        let (tx, rx) = channel();
        let reader_handle = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("sandbox-reader".to_string())
                .spawn(move || run_reader(&shared, reader, live))
        };
        let writer_handle = std::thread::Builder::new()
            .name("sandbox-writer".to_string())
            .spawn(move || run_writer(&rx, writer));
        if let Some(stderr) = stderr {
            let shared = Arc::clone(&shared);
            // Not joined: a grandchild holding the pipe open must not stall a drop.
            let _ = std::thread::Builder::new()
                .name("sandbox-stderr".to_string())
                .spawn(move || run_stderr(&shared, stderr));
        }
        let mut link = Self {
            shared,
            tx: Some(tx),
            reader: reader_handle.ok(),
            writer: writer_handle.ok(),
            next_id: 0,
            timeouts,
            misses: 0,
            last_miss: None,
            shutdown_at: None,
            pending: false,
            held: None,
            vitals: None,
        };
        if link.reader.is_none() || link.writer.is_none() {
            link.shared
                .kill("the app could not start its helper threads".to_string());
            return Err(BackendError::Failed(
                "cannot start the plugin's helper threads".to_string(),
            ));
        }
        link.send_hello()?;
        Ok(link)
    }

    /// Starts the helper threads over `transport` and performs the Hello exchange.
    ///
    /// # Errors
    /// The child did not greet in time, spoke another protocol, or ended first (the error
    /// then carries the exit description and the stderr tail).
    pub fn start(
        transport: Transport,
        death: DeathHandle,
        timeouts: Timeouts,
        messenger: HostMessenger,
        live: LiveGuard,
    ) -> Result<Self, BackendError> {
        let mut link = Self::launch(transport, death, timeouts, messenger, live)?;
        let reply = link
            .await_reply("Hello", timeouts.load)
            .map_err(|e| link.explain(e))?;
        link.check_hello(reply)?;
        Ok(link)
    }

    fn send_hello(&mut self) -> Result<(), BackendError> {
        self.send(
            &ToChild::Hello {
                protocol: PROTOCOL_VERSION,
            },
            None,
        )
    }

    /// Checks the child's greeting: the protocol must be the app's.
    pub fn check_hello(&mut self, reply: Reply) -> Result<(), BackendError> {
        match reply.msg {
            FromChild::Hello { protocol, .. } if protocol == PROTOCOL_VERSION => Ok(()),
            FromChild::Hello { protocol, .. } => {
                let text = format!(
                    "the plugin process speaks protocol {protocol}, the app speaks {PROTOCOL_VERSION}"
                );
                self.shared.kill(text.clone());
                Err(BackendError::Failed(text))
            }
            other => Err(self.unexpected("Hello", &other)),
        }
    }

    /// The answer to the request sent last, without waiting: `None` while it has not come and
    /// `since + timeout` has not passed. Like [`Link::call`], an answer that is too late kills
    /// the child.
    pub fn poll_reply(
        &mut self,
        what: &str,
        since: std::time::Instant,
        timeout: Duration,
    ) -> Option<Result<Reply, BackendError>> {
        let waited = match self.shared.try_take() {
            Some(waited) => waited,
            None if now().saturating_duration_since(since) >= timeout => Waited::TimedOut {
                alive: self.alive_by_polling(),
            },
            None => {
                self.sense();
                return None;
            }
        };
        self.pending = false;
        self.held = None;
        let id = self.next_id;
        Some(self.settle(waited, what, timeout, id, false))
    }

    /// Takes a sample of the child's sign of life (the polled waits call it on every poll).
    fn sense(&mut self) {
        let (pulse, t) = (self.shared.pulse(), now());
        let v = self.vitals.get_or_insert(Vitals {
            pulse,
            sampled: t,
            changed: None,
        });
        if pulse != v.pulse {
            // A change seen after a long gap is dated at the sample before it: it says nothing
            // about the last moments.
            v.changed = Some(if t.saturating_duration_since(v.sampled) <= ALIVE_WINDOW {
                t
            } else {
                v.sampled
            });
            v.pulse = pulse;
        }
        v.sampled = t;
    }

    /// True when the sign of life changed within [`ALIVE_WINDOW`] of the polls so far.
    fn alive_by_polling(&mut self) -> bool {
        self.sense();
        let t = now();
        self.vitals
            .and_then(|v| v.changed)
            .is_some_and(|c| t.saturating_duration_since(c) <= ALIVE_WINDOW)
    }

    /// The error of a call made while a held request is out (see [`Link::begin_held_call`]).
    fn busy(&self, what: &str) -> Option<BackendError> {
        if let Some(held) = self.held {
            return Some(BackendError::Failed(format!(
                "the plugin is busy with {held}; {what} was not sent"
            )));
        }
        self.stuck().map(|call| not_responding(&call, what))
    }

    /// The name of the call the plugin has not answered although its process is alive (its main
    /// thread is busy): until the answer comes, every call fails at once without being sent, and
    /// the answer ends the state by itself. `None` when it is not stuck or the child is gone.
    pub fn stuck(&self) -> Option<String> {
        self.shared.stuck()
    }

    /// The name of the held request that is out, if any.
    pub fn held(&self) -> Option<&'static str> {
        self.held
    }

    /// Like [`Link::begin_call`] for a request whose answer may take minutes (an editor that
    /// shows licence or sign-in dialogs before it opens): [`Link::poll_reply`] takes the
    /// answer, and every other call fails at once until it has.
    pub fn begin_held_call(
        &mut self,
        what: &'static str,
        make: impl FnOnce(u64) -> ToChild,
    ) -> Result<(), BackendError> {
        self.begin_call(make)?;
        // Taken by `poll_reply` only: a `finish_call` must not mistake it for its own.
        self.pending = false;
        self.held = Some(what);
        self.vitals = None;
        self.sense();
        Ok(())
    }

    /// For the errors of a call made while the child may be starting or ending: a lost link
    /// is explained by how the child ended.
    pub fn explain(&self, e: BackendError) -> BackendError {
        match &e {
            BackendError::Failed(t) if t == LOST => {
                let reason = self
                    .shared
                    .wait_reason(Duration::from_secs(3))
                    .unwrap_or_else(|| LOST.to_string());
                BackendError::Failed(reason)
            }
            _ => e,
        }
    }

    /// The state shared with the helper threads.
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// The timeouts this link was made with.
    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    /// True once the child is gone or condemned.
    pub fn is_lost(&self) -> bool {
        self.shared.is_lost()
    }

    fn send(&mut self, msg: &ToChild, blob: Option<&[u8]>) -> Result<(), BackendError> {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, msg, blob)
            .map_err(|e| BackendError::Failed(format!("cannot encode a request: {e}")))?;
        let sent = self.tx.as_ref().is_some_and(|tx| tx.send(bytes).is_ok());
        if sent {
            Ok(())
        } else {
            self.shared
                .kill("the plugin process closed its input".to_string());
            Err(lost())
        }
    }

    fn await_reply(&mut self, what: &str, timeout: Duration) -> Result<Reply, BackendError> {
        let waited = self.shared.wait_noted(what, timeout);
        let id = self.next_id;
        self.settle(waited, what, timeout, id, false)
    }

    /// Whether a call that timed out leaves the child alone: it showed a sign of life, the link
    /// is not a bounce's, and the call is one that has an id to wait for. Without it the child
    /// is stopped.
    fn spares(&mut self, alive: bool, id: u64) -> bool {
        // A short wait can fall between two beats of a healthy child on a loaded machine: the
        // samples taken at earlier calls count too (the beat must have moved within
        // `ALIVE_WINDOW` of them).
        let alive = alive || self.alive_by_polling();
        alive && id != 0 && !self.timeouts.bounce
    }

    /// Turns what a wait for the request `id` ended with into the reply or the error. A request
    /// that went unanswered while the child is alive does not stop it: the link is stuck on it
    /// (`background`: the request was the background one).
    fn settle(
        &mut self,
        waited: Waited,
        what: &str,
        timeout: Duration,
        id: u64,
        background: bool,
    ) -> Result<Reply, BackendError> {
        match waited {
            Waited::Reply(reply) => match reply.msg {
                FromChild::Err { kind, text, .. } => Err(into_backend_error(kind, text)),
                _ => Ok(reply),
            },
            Waited::Ended => Err(lost()),
            Waited::TimedOut { alive } => {
                let ms = timeout.as_millis();
                if self.spares(alive, id) {
                    // The answer may have come between the end of the wait and now.
                    if let Some(reply) = self.shared.enter_stuck(id, what, background) {
                        return self.settle(Waited::Reply(reply), what, timeout, id, background);
                    }
                    return Err(BackendError::Failed(format!(
                        "{NOT_RESPONDING}: it did not answer {what} within {ms} ms"
                    )));
                }
                if background {
                    self.shared.clear_background();
                }
                self.shared.kill(format!(
                    "the plugin process did not answer {what} within {ms} ms and was stopped"
                ));
                Err(BackendError::Failed(format!(
                    "the plugin did not answer {what} within {ms} ms"
                )))
            }
        }
    }

    /// Sends the request `make(id)` and returns without waiting: [`Link::finish_call`] takes the
    /// answer. A pipelined burst (one request to every child, then the answers) costs the
    /// slowest child, not the sum. Any other call on this link before the finish abandons the
    /// request (its answer is dropped).
    pub fn begin_call(&mut self, make: impl FnOnce(u64) -> ToChild) -> Result<(), BackendError> {
        if let Some(e) = self.busy("a request") {
            return Err(e);
        }
        self.pending = false;
        if self.shared.is_lost() {
            return Err(lost());
        }
        self.next_id += 1;
        let id = self.next_id;
        self.shared.expect(Expect::Id(id));
        self.send(&make(id), None)?;
        self.pending = true;
        Ok(())
    }

    /// The answer to the request [`Link::begin_call`] sent, waiting up to `timeout` for it like
    /// [`Link::call`]; `None` when no request is pending.
    pub fn finish_call(
        &mut self,
        what: &str,
        timeout: Duration,
    ) -> Option<Result<Reply, BackendError>> {
        if let Some(e) = self.busy(what) {
            return Some(Err(e));
        }
        if !std::mem::take(&mut self.pending) {
            return None;
        }
        Some(self.await_reply(what, timeout))
    }

    /// Sends the request `make(id)` as the background request: its answer goes to a slot of its
    /// own, so it can be out while other calls go on, and [`Link::poll_background`] takes it
    /// without waiting. Nothing times it out: a child whose main thread is busy answers when
    /// it is free. Only one is out at a time; asking while one is out changes nothing.
    pub fn begin_background(
        &mut self,
        make: impl FnOnce(u64) -> ToChild,
    ) -> Result<(), BackendError> {
        if self.shared.is_lost() {
            return Err(lost());
        }
        if let Some(call) = self.stuck() {
            return Err(not_responding(&call, "a request"));
        }
        if self.shared.background_out() {
            return Ok(());
        }
        self.next_id += 1;
        let id = self.next_id;
        self.shared.expect_background(id);
        let sent = self.send(&make(id), None);
        if sent.is_err() {
            self.shared.clear_background();
        }
        sent
    }

    /// True while the background request has not been answered or collected.
    pub fn background_out(&self) -> bool {
        self.shared.background_out()
    }

    /// The answer to the background request, without waiting; `None` while it has not come. A
    /// child that ended answers with a lost link.
    pub fn poll_background(&mut self) -> Option<Result<Reply, BackendError>> {
        Some(match self.shared.try_take_background()? {
            Waited::Reply(reply) => match reply.msg {
                FromChild::Err { kind, text, .. } => Err(into_backend_error(kind, text)),
                _ => Ok(reply),
            },
            Waited::Ended | Waited::TimedOut { .. } => Err(lost()),
        })
    }

    /// Waits up to `timeout` for the background request that is out, like [`Link::call`]: a
    /// call that needs the state now (a save) takes the request that is out instead of
    /// sending a second one. `None` when none is out.
    pub fn finish_background(
        &mut self,
        what: &str,
        timeout: Duration,
    ) -> Option<Result<Reply, BackendError>> {
        if !self.shared.background_out() {
            return None;
        }
        let id = self.shared.background_id()?;
        let waited = self.shared.wait_background_noted(what, timeout);
        if matches!(waited, Waited::Ended) {
            self.shared.clear_background();
        }
        Some(self.settle(waited, what, timeout, id, true))
    }

    /// Sends the request `make(id)` (with `blob`) and waits `timeout` for its answer. A lost
    /// link fails at once without sending anything; a call that times out kills the child.
    pub fn call(
        &mut self,
        what: &str,
        timeout: Duration,
        make: impl FnOnce(u64) -> ToChild,
        blob: Option<&[u8]>,
    ) -> Result<Reply, BackendError> {
        if let Some(e) = self.busy(what) {
            return Err(e);
        }
        self.pending = false;
        if self.shared.is_lost() {
            return Err(lost());
        }
        self.next_id += 1;
        let id = self.next_id;
        self.shared.expect(Expect::Id(id));
        self.send(&make(id), blob)?;
        self.await_reply(what, timeout)
    }

    /// Like [`Link::call`] for a request that only polishes the interface (param text, pins,
    /// the layout, closing the editor): when it is not answered in `timeout` the request is
    /// dropped and the child is left alone, because a plugin that is busy on its main thread
    /// (loading a sample, rebuilding its editor) answers none of them and is not at fault.
    /// After a few misses in a row the instance counts as slow and is asked at most once a
    /// second; after many the child is stopped, since it is wedged. An answer that arrives later
    /// is ignored.
    pub fn soft_call(
        &mut self,
        what: &str,
        timeout: Duration,
        make: impl FnOnce(u64) -> ToChild,
    ) -> Result<Reply, BackendError> {
        if let Some(e) = self.busy(what) {
            return Err(e);
        }
        self.pending = false;
        if self.shared.is_lost() {
            return Err(lost());
        }
        if self.is_slow()
            && self
                .last_miss
                .is_some_and(|t| now().saturating_duration_since(t) < self.timeouts.slow_retry)
        {
            return Err(BackendError::Failed(format!(
                "the plugin is slow to answer {what}"
            )));
        }
        self.next_id += 1;
        let id = self.next_id;
        self.sense();
        self.shared.expect(Expect::Id(id));
        self.send(&make(id), None)?;
        match self.shared.wait_noted(what, timeout) {
            Waited::Reply(reply) => {
                self.misses = 0;
                match reply.msg {
                    FromChild::Err { kind, text, .. } => Err(into_backend_error(kind, text)),
                    _ => Ok(reply),
                }
            }
            Waited::Ended => Err(lost()),
            Waited::TimedOut { alive } => {
                self.misses += 1;
                self.last_miss = Some(now());
                let ms = timeout.as_millis();
                if self.misses == MISSES_TO_SLOW {
                    self.shared.warn(format!(
                        "the plugin is slow to answer {what}; the app asks less often until it does"
                    ));
                }
                if self.misses >= MISSES_TO_KILL {
                    if self.spares(alive, id) {
                        // Busy, not wedged: the process shows a sign of life.
                        if let Some(reply) = self.shared.enter_stuck(id, what, false) {
                            self.misses = 0;
                            return match reply.msg {
                                FromChild::Err { kind, text, .. } => {
                                    Err(into_backend_error(kind, text))
                                }
                                _ => Ok(reply),
                            };
                        }
                        let n = self.misses;
                        return Err(BackendError::Failed(format!(
                            "{NOT_RESPONDING}: it did not answer {what} {n} times in a row"
                        )));
                    }
                    let n = self.misses;
                    self.shared.kill(format!(
                        "the plugin process did not answer {what} {n} times in a row and was stopped"
                    ));
                }
                Err(BackendError::Failed(format!(
                    "the plugin did not answer {what} within {ms} ms"
                )))
            }
        }
    }

    /// True while the child misses cosmetic calls (see [`Link::soft_call`]).
    pub fn is_slow(&self) -> bool {
        self.misses >= MISSES_TO_SLOW
    }

    /// Asks the child to end and returns at once; the drop then waits for it, together with the
    /// children that were asked at the same time.
    pub fn begin_shutdown(&mut self) {
        if self.shutdown_at.is_some() {
            return;
        }
        self.shared.closing.store(true, Ordering::SeqCst);
        if !self.shared.is_lost() {
            let _ = self.send(&ToChild::Shutdown, None);
        }
        // Closing the channel ends the writer, which closes the child's stdin.
        self.tx = None;
        self.shutdown_at = Some(now());
    }

    /// The error for a reply of the wrong kind: the child breaks the protocol.
    pub fn unexpected(&self, what: &str, got: &FromChild) -> BackendError {
        let text = format!("the plugin process answered {what} with {got:?}");
        self.shared.kill(text.clone());
        BackendError::Failed(text)
    }
}

impl Drop for Link {
    /// Asks the child to end (unless that was done already), gives it what is left of a
    /// second, then kills it; leaves no thread behind that can end.
    fn drop(&mut self) {
        self.begin_shutdown();
        let waited = self
            .shutdown_at
            .map_or(Duration::ZERO, |t| now().saturating_duration_since(t));
        if self
            .shared
            .wait_reason(SHUTDOWN_GRACE.saturating_sub(waited))
            .is_none()
        {
            self.shared
                .kill("the plugin process did not end when asked and was stopped".to_string());
            let _ = self.shared.wait_reason(EXIT_GRACE + STDERR_GRACE * 2);
        }
        // Only a finished thread is joined: a stuck one (a grandchild holding the pipe) must
        // not stall the caller.
        let finished = lock(&self.shared.mailbox).reason.is_some();
        if finished {
            if let Some(h) = self.reader.take() {
                let _ = h.join();
            }
            if let Some(h) = self.writer.take() {
                let _ = h.join();
            }
        }
    }
}

#[cfg(test)]
mod stderr_tail_tests {
    use super::with_stderr_tail;

    #[test]
    fn a_panic_line_carries_the_last_three_stderr_lines_and_nothing_when_there_are_none() {
        assert_eq!(with_stderr_tail("x".to_string(), b""), "x");
        assert_eq!(
            with_stderr_tail(
                "x".to_string(),
                b" 

"
            ),
            "x"
        );
        assert_eq!(
            with_stderr_tail(
                "x".to_string(),
                b"a
b

c
d
"
            ),
            "x [child stderr: b | c | d]"
        );
    }
}
