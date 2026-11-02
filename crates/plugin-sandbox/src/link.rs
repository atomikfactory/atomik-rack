//! The control link to one child: a writer thread, a reader thread and a stderr thread around a
//! [`Transport`], and the one-slot mailbox that turns the child's replies into answers for the
//! main thread's calls.
//!
//! Only the main thread makes calls and only one is in flight at a time, so a single slot is
//! enough; replies are matched by request id and anything else is ignored. A call that matters
//! and is not answered in time kills the child; a cosmetic one (see [`Link::soft_call`]) only
//! drops its request. Once the child is killed the link is lost, and every call fails at
//! once without any I/O. The reader thread is the one that learns that the child ended: it
//! reaps the exit status, reads the stderr tail, marks the audio block dead (so the engine
//! side sees [`atomik_plugin_host::ProcessStatus::Lost`]) and posts
//! [`PluginMessage::ProcessExited`].

use crate::clock::now;
use crate::transport::{ChildProcess, Transport};
use atomik_plugin_host::{
    BackendError, HUNG, HostMessenger, LOST, LogLevel, ParamPhase, PluginMessage,
};
use atomik_plugin_ipc::{
    DeathHandle, FromChild, MAX_FRAME_BYTES, MAX_GUI_DIMENSION, MAX_LOG_BYTES,
    MAX_REPLY_JSON_BYTES, PROTOCOL_VERSION, ToChild, WireLayout, into_backend_error,
    read_frame_with, write_frame,
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

/// The most parameter edits held back for the main thread at once.
const MAX_HELD_EDITS: usize = 4096;

/// The timeouts of the main-thread calls.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timeouts {
    /// Hello, instantiate, activate, capture and restore state: a call that is not answered in
    /// time kills the child.
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
    /// The child's stdout closed: nothing more will come.
    ended: bool,
    /// Set once the child is reaped: why it ended.
    reason: Option<String>,
}

enum Waited {
    Reply(Reply),
    Ended,
    TimedOut,
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
        let start = now();
        let mut mb = lock(&self.mailbox);
        loop {
            if let Some(reply) = mb.reply.take() {
                mb.expect = Expect::Nothing;
                return Waited::Reply(reply);
            }
            if mb.ended {
                return Waited::Ended;
            }
            let left = timeout.saturating_sub(now().saturating_duration_since(start));
            if left.is_zero() {
                return Waited::TimedOut;
            }
            mb = self
                .cv
                .wait_timeout(mb, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
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
        let head = lock(&self.kill_reason).take().unwrap_or(status);
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
            | FromChild::LayoutChanged { id, .. }
            | FromChild::GuiOpened { id, .. } => *id,
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
            FromChild::GuiClosed => return post(PluginMessage::GuiClosed),
            FromChild::GuiRequestShow => return post(PluginMessage::GuiRequestShow),
            FromChild::GuiRequestHide => return post(PluginMessage::GuiRequestHide),
            FromChild::Log { level, text } => {
                if text.len() > MAX_LOG_BYTES {
                    self.rejected.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                return post(PluginMessage::Log {
                    level: LogLevel::from(*level),
                    text: text.clone(),
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
        if !self.deliver(Reply { msg, blob }, Expect::Id(id))
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
            .await_reply("Hello", timeouts.create)
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
            None if now().saturating_duration_since(since) >= timeout => Waited::TimedOut,
            None => return None,
        };
        self.pending = false;
        Some(self.settle(waited, what, timeout))
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
        let waited = self.shared.wait(timeout);
        self.settle(waited, what, timeout)
    }

    /// Turns what a wait ended with into the reply or the error.
    fn settle(
        &mut self,
        waited: Waited,
        what: &str,
        timeout: Duration,
    ) -> Result<Reply, BackendError> {
        match waited {
            Waited::Reply(reply) => match reply.msg {
                FromChild::Err { kind, text, .. } => Err(into_backend_error(kind, text)),
                _ => Ok(reply),
            },
            Waited::Ended => Err(lost()),
            Waited::TimedOut => {
                let ms = timeout.as_millis();
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
        if !std::mem::take(&mut self.pending) {
            return None;
        }
        Some(self.await_reply(what, timeout))
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
        self.shared.expect(Expect::Id(id));
        self.send(&make(id), None)?;
        match self.shared.wait(timeout) {
            Waited::Reply(reply) => {
                self.misses = 0;
                match reply.msg {
                    FromChild::Err { kind, text, .. } => Err(into_backend_error(kind, text)),
                    _ => Ok(reply),
                }
            }
            Waited::Ended => Err(lost()),
            Waited::TimedOut => {
                self.misses += 1;
                self.last_miss = Some(now());
                let ms = timeout.as_millis();
                if self.misses == MISSES_TO_SLOW {
                    self.shared.warn(format!(
                        "the plugin is slow to answer {what}; the app asks less often until it does"
                    ));
                }
                if self.misses >= MISSES_TO_KILL {
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
