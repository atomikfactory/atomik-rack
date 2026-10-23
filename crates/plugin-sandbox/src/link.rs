//! The control link to one child: a writer thread, a reader thread and a stderr thread around a
//! [`Transport`], and the one-slot mailbox that turns the child's replies into answers for the
//! main thread's calls.
//!
//! Only the main thread makes calls and only one is in flight at a time, so a single slot is
//! enough; replies are matched by request id and anything else is ignored. A call that is not
//! answered in time kills the child: from then on the link is lost, and every call fails at
//! once without any I/O. The reader thread is the one that learns that the child ended: it
//! reaps the exit status, reads the stderr tail, marks the audio block dead (so the engine
//! side sees [`atomik_plugin_host::ProcessStatus::Lost`]) and posts
//! [`PluginMessage::ProcessExited`].

use crate::clock::now;
use crate::transport::{ChildProcess, Transport};
use atomik_plugin_host::{BackendError, HostMessenger, LOST, LogLevel, ParamPhase, PluginMessage};
use atomik_plugin_ipc::{
    DeathHandle, FromChild, PROTOCOL_VERSION, ToChild, into_backend_error, read_frame, write_frame,
};
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
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

/// The timeouts of the main-thread calls.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timeouts {
    /// Hello, instantiate, activate, capture and restore state.
    pub create: Duration,
    /// Every other call.
    pub call: Duration,
    /// Param text.
    pub text: Duration,
    /// Offline audio batches.
    pub watchdog: Duration,
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

    fn deliver(&self, reply: Reply, wanted: Expect) {
        let mut mb = lock(&self.mailbox);
        if mb.expect == wanted {
            mb.reply = Some(reply);
            self.cv.notify_all();
        }
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
            self.kill(
                "the plugin process stopped answering audio requests and was stopped".to_string(),
            );
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
            | FromChild::LayoutChanged { id, .. }
            | FromChild::GuiOpened { id, .. } => *id,
            FromChild::ParamEdit {
                param,
                value,
                phase,
            } => {
                return post(PluginMessage::ParamEdit {
                    id: *param,
                    value: *value,
                    phase: ParamPhase::from(*phase),
                });
            }
            FromChild::StateDirty => return post(PluginMessage::StateDirty),
            FromChild::LatencyChanged => return post(PluginMessage::LatencyChanged),
            FromChild::RescanParams => return post(PluginMessage::RescanParams),
            FromChild::RequestRestart => return post(PluginMessage::RequestRestart),
            FromChild::GuiResize { w, h } => {
                return post(PluginMessage::GuiResize { w: *w, h: *h });
            }
            FromChild::GuiClosed => return post(PluginMessage::GuiClosed),
            FromChild::GuiRequestShow => return post(PluginMessage::GuiRequestShow),
            FromChild::GuiRequestHide => return post(PluginMessage::GuiRequestHide),
            FromChild::Log { level, text } => {
                return post(PluginMessage::Log {
                    level: LogLevel::from(*level),
                    text: text.clone(),
                });
            }
        };
        self.deliver(Reply { msg, blob }, Expect::Id(id));
    }
}

fn run_reader(shared: &Shared, mut reader: Box<dyn Read + Send>, live: LiveGuard) {
    let bad = loop {
        match read_frame::<FromChild>(&mut reader) {
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
}

impl Link {
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
        };
        if link.reader.is_none() || link.writer.is_none() {
            link.shared
                .kill("the app could not start its helper threads".to_string());
            return Err(BackendError::Failed(
                "cannot start the plugin's helper threads".to_string(),
            ));
        }
        link.hello()?;
        Ok(link)
    }

    fn hello(&mut self) -> Result<(), BackendError> {
        self.send(
            &ToChild::Hello {
                protocol: PROTOCOL_VERSION,
            },
            None,
        )?;
        let reply = self
            .await_reply("Hello", self.timeouts.create)
            .map_err(|e| self.explain(e))?;
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
        match self.shared.wait(timeout) {
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

    /// Sends the request `make(id)` (with `blob`) and waits `timeout` for its answer. A lost
    /// link fails at once without sending anything; a call that times out kills the child.
    pub fn call(
        &mut self,
        what: &str,
        timeout: Duration,
        make: impl FnOnce(u64) -> ToChild,
        blob: Option<&[u8]>,
    ) -> Result<Reply, BackendError> {
        if self.shared.is_lost() {
            return Err(lost());
        }
        self.next_id += 1;
        let id = self.next_id;
        self.shared.expect(Expect::Id(id));
        self.send(&make(id), blob)?;
        self.await_reply(what, timeout)
    }

    /// The error for a reply of the wrong kind: the child breaks the protocol.
    pub fn unexpected(&self, what: &str, got: &FromChild) -> BackendError {
        let text = format!("the plugin process answered {what} with {got:?}");
        self.shared.kill(text.clone());
        BackendError::Failed(text)
    }
}

impl Drop for Link {
    /// Asks the child to end, gives it a second, then kills it; leaves no thread behind that
    /// can end.
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::SeqCst);
        if !self.shared.is_lost() {
            let _ = self.send(&ToChild::Shutdown, None);
        }
        // Closing the channel ends the writer, which closes the child's stdin.
        self.tx = None;
        if self.shared.wait_reason(SHUTDOWN_GRACE).is_none() {
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
