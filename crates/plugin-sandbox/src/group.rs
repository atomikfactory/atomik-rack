//! A group of plugin instances in one child process, app side.
//!
//! A [`Hub`] owns one `atomik-plugin-host --group` child and the three threads around it (the one
//! writer of its stdin, the reader of its stdout and the reader of its stderr). Each instance
//! that joins gets a **virtual [`Transport`]**: a writer that sends its frames to the child
//! tagged with the instance's slot, a reader fed with the frames the child sends back on that
//! slot, and a [`ChildProcess`] that stands for the shared process. The instance's `Link` and
//! `RemoteInstance` work over it exactly as over a child of their own, so nothing above the
//! transport knows about groups.
//!
//! * **Crash domain.** The process is shared: when it ends, every member's channel closes and
//!   every member is lost. Killing it (a hang, a call that was not answered, a broken protocol)
//!   ends them all, and the reason of each names the group.
//! * **Leaving.** An instance that is dropped asks its slot to end and waits for the child to say
//!   so; the process stays up for the others. A member that does not answer is abandoned, not
//!   killed: the others are not punished for a plugin that was merely slow to close.
//! * **Limit.** A hub holds [`MAX_GROUP_INSTANCES`]; the next instance starts another hub.
//! * **Real time.** The audio path does not touch the hub: the audio block and the doorbell of
//!   an instance are its own, and the child serves each on its own audio thread.

use crate::clock::now;
use crate::link::LiveGuard;
use crate::transport::{ChildProcess, Transport};
use atomik_plugin_ipc::{
    FromGroup, GROUP_SLOT, MAX_GROUP_INSTANCES, SlotWriter, ToGroup, read_frame, read_slot_frame,
    write_frame, write_slot_frame,
};
use std::collections::BTreeMap;
use std::io::{self, Cursor, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// How long a dropped hub waits for its child to end on its own before it kills it.
const REAP_GRACE: Duration = Duration::from_secs(2);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What an instance asks to join: the plugin and the block and doorbell the app made for it.
#[derive(Debug, Clone)]
pub(crate) struct Attach {
    pub plugin_file: String,
    pub plugin_id: String,
    pub block: String,
    pub doorbell: String,
}

/// Why a member's channel closed.
#[derive(Debug, Clone)]
enum Gone {
    /// It left the group: asked to end, or its channel broke.
    Left,
    /// The child could not add it.
    Failed(String),
    /// The whole process ended.
    GroupEnded,
}

#[derive(Debug, Default)]
struct MemberState {
    gone: Mutex<Option<Gone>>,
    /// The instance's writer is gone: it asked to leave.
    leaving: AtomicBool,
}

impl MemberState {
    fn set_gone(&self, why: Gone) {
        let mut gone = lock(&self.gone);
        if gone.is_none() {
            *gone = Some(why);
        }
    }

    fn gone(&self) -> Option<Gone> {
        lock(&self.gone).clone()
    }
}

struct Member {
    stdout: Sender<Vec<u8>>,
    state: Arc<MemberState>,
    plugin: String,
}

/// One group child.
pub(crate) struct Hub {
    /// Whole slot frames for the writer thread; `None` once the hub is dropped.
    out: Mutex<Option<Sender<Vec<u8>>>>,
    members: Mutex<BTreeMap<u32, Member>>,
    /// Where the child's stderr goes: every member's copy.
    stderr_subs: Mutex<Vec<(u32, Sender<Vec<u8>>)>>,
    next_slot: AtomicU32,
    dead: AtomicBool,
    /// The plugins of the group when it ended, for the reason.
    ended_with: Mutex<Vec<String>>,
    child: Mutex<Option<Box<dyn ChildProcess>>>,
    exit: Mutex<Option<String>>,
    live: Mutex<Option<LiveGuard>>,
}

impl Hub {
    /// Starts the helper threads over the child's `transport`.
    pub fn start(transport: Transport, live: LiveGuard) -> Arc<Self> {
        let Transport {
            writer,
            reader,
            stderr,
            child,
        } = transport;
        let (out, rx) = channel::<Vec<u8>>();
        let hub = Arc::new(Self {
            out: Mutex::new(Some(out)),
            members: Mutex::new(BTreeMap::new()),
            stderr_subs: Mutex::new(Vec::new()),
            next_slot: AtomicU32::new(1),
            dead: AtomicBool::new(false),
            ended_with: Mutex::new(Vec::new()),
            child: Mutex::new(Some(child)),
            exit: Mutex::new(None),
            live: Mutex::new(Some(live)),
        });
        let _ = std::thread::Builder::new()
            .name("sandbox-group-writer".to_string())
            .spawn(move || run_writer(&rx, writer));
        let weak = Arc::downgrade(&hub);
        let _ = std::thread::Builder::new()
            .name("sandbox-group-reader".to_string())
            .spawn(move || run_reader(&weak, reader));
        if let Some(stderr) = stderr {
            let weak = Arc::downgrade(&hub);
            let _ = std::thread::Builder::new()
                .name("sandbox-group-stderr".to_string())
                .spawn(move || run_stderr(&weak, stderr));
        }
        hub
    }

    /// True while the child lives.
    pub fn alive(&self) -> bool {
        !self.dead.load(Ordering::SeqCst)
    }

    /// Adds an instance: registers its slot, tells the child to attach it and returns its
    /// transport. `None` when the child is gone or the group is full (the caller then starts
    /// another hub).
    pub fn join(self: &Arc<Self>, attach: Attach) -> Option<Transport> {
        let (stdout_tx, stdout_rx) = channel();
        let (stderr_tx, stderr_rx) = channel();
        let state = Arc::new(MemberState::default());
        let slot = {
            let mut members = lock(&self.members);
            if !self.alive() || members.len() >= MAX_GROUP_INSTANCES {
                return None;
            }
            let slot = self.next_slot.fetch_add(1, Ordering::Relaxed);
            members.insert(
                slot,
                Member {
                    stdout: stdout_tx,
                    state: Arc::clone(&state),
                    plugin: attach.plugin_id.clone(),
                },
            );
            slot
        };
        lock(&self.stderr_subs).push((slot, stderr_tx));
        let attached = self.send_group(&ToGroup::Attach {
            slot,
            plugin_file: attach.plugin_file,
            plugin_id: attach.plugin_id,
            block: attach.block,
            doorbell: attach.doorbell,
        });
        if !attached {
            self.drop_member(slot, Gone::Left);
            return None;
        }
        let out = lock(&self.out).clone()?;
        Some(Transport {
            writer: Box::new(MemberWriter {
                inner: SlotWriter::new(slot, ChannelSink(out)),
                state: Arc::clone(&state),
            }),
            reader: Box::new(ChannelReader::new(stdout_rx)),
            stderr: Some(Box::new(ChannelReader::new(stderr_rx))),
            child: Box::new(MemberChild {
                hub: Arc::clone(self),
                slot,
                state,
            }),
        })
    }

    fn send_group(&self, msg: &ToGroup) -> bool {
        let mut inner = Vec::new();
        let mut outer = Vec::new();
        if write_frame(&mut inner, msg, None).is_err()
            || write_slot_frame(&mut outer, GROUP_SLOT, &inner).is_err()
        {
            return false;
        }
        lock(&self.out)
            .as_ref()
            .is_some_and(|out| out.send(outer).is_ok())
    }

    /// Takes the instance of `slot` out of the group: its channels close, so its link ends. Its
    /// share of the child's stderr stops with it (a process that is gone keeps its last words
    /// until the stream ends, see `end`).
    fn drop_member(&self, slot: u32, why: Gone) {
        if let Some(member) = lock(&self.members).remove(&slot) {
            member.state.set_gone(why);
        }
        lock(&self.stderr_subs).retain(|(s, _)| *s != slot);
    }

    /// Ends the process now. Does not wait for it.
    fn kill(&self) {
        if let Some(child) = lock(&self.child).as_mut() {
            child.kill();
        }
    }

    /// `Some(description)` once the process has ended (and is reaped).
    fn try_wait(&self) -> Option<String> {
        let mut exit = lock(&self.exit);
        if exit.is_none() {
            *exit = lock(&self.child).as_mut().and_then(|c| c.try_wait());
        }
        exit.clone()
    }

    /// The child's stdout closed: the group is over and every member with it.
    fn end(&self) {
        let members = std::mem::take(&mut *lock(&self.members));
        *lock(&self.ended_with) = members.values().map(|m| m.plugin.clone()).collect();
        self.dead.store(true, Ordering::SeqCst);
        for member in members.into_values() {
            member.state.set_gone(Gone::GroupEnded);
            // Dropping the sender closes the member's reader.
        }
    }

    /// The sentence that names the group in the reason of a member that was lost with it.
    fn note(&self) -> String {
        let plugins = lock(&self.ended_with).clone();
        let n = plugins.len();
        let shown: Vec<&str> = plugins.iter().take(4).map(String::as_str).collect();
        let more = if n > shown.len() { ", ..." } else { "" };
        format!(
            "this process hosted a group of {n} plugin{} ({}{more}) and every one of them was stopped",
            if n == 1 { "" } else { "s" },
            shown.join(", ")
        )
    }
}

impl Drop for Hub {
    /// Closes the child's input (it ends when its last instance has left, or at once when none
    /// is left) and reaps it on a thread of its own: a drop never waits.
    fn drop(&mut self) {
        *lock(&self.out) = None;
        let child = lock(&self.child).take();
        let live = lock(&self.live).take();
        if let Some(mut child) = child {
            let _ = std::thread::Builder::new()
                .name("sandbox-group-reaper".to_string())
                .spawn(move || {
                    let start = now();
                    let mut killed = false;
                    loop {
                        if child.try_wait().is_some() {
                            break;
                        }
                        if now().saturating_duration_since(start) >= REAP_GRACE {
                            if killed {
                                break;
                            }
                            child.kill();
                            killed = true;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    drop(live);
                });
        }
    }
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

fn run_reader(hub: &std::sync::Weak<Hub>, mut reader: Box<dyn Read + Send>) {
    while let Ok((slot, inner)) = read_slot_frame(&mut reader) {
        let Some(hub) = hub.upgrade() else {
            // Nobody listens any more; the child ends when its input closed.
            continue;
        };
        if slot == GROUP_SLOT {
            match read_frame::<FromGroup>(&mut Cursor::new(inner)) {
                Ok((FromGroup::AttachFailed { slot, text }, _)) => {
                    hub.drop_member(slot, Gone::Failed(text));
                }
                Ok((FromGroup::Detached { slot }, _)) => hub.drop_member(slot, Gone::Left),
                Err(_) => {}
            }
        } else if let Some(member) = lock(&hub.members).get(&slot) {
            let _ = member.stdout.send(inner);
        }
    }
    if let Some(hub) = hub.upgrade() {
        hub.end();
    }
}

fn run_stderr(hub: &std::sync::Weak<Hub>, mut stderr: Box<dyn Read + Send>) {
    let mut buf = [0u8; 1024];
    loop {
        match stderr.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let Some(hub) = hub.upgrade() else { continue };
                lock(&hub.stderr_subs).retain(|(_, sub)| sub.send(buf[..n].to_vec()).is_ok());
            }
        }
    }
    if let Some(hub) = hub.upgrade() {
        lock(&hub.stderr_subs).clear();
    }
}

/// Sends every write as one item: a whole slot frame.
struct ChannelSink(Sender<Vec<u8>>);

impl Write for ChannelSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .send(buf.to_vec())
            .map(|()| buf.len())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the group child is gone"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The writer of one member: its frames leave tagged with its slot. Dropping it (the instance's
/// writer thread ended: the instance is being dropped) marks the member as leaving.
struct MemberWriter {
    inner: SlotWriter<ChannelSink>,
    state: Arc<MemberState>,
}

impl Write for MemberWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Drop for MemberWriter {
    fn drop(&mut self) {
        self.state.leaving.store(true, Ordering::SeqCst);
    }
}

/// A reader over the frames (or text) a channel delivers; the end of the channel is end of file.
struct ChannelReader {
    rx: Receiver<Vec<u8>>,
    current: Vec<u8>,
    at: usize,
}

impl ChannelReader {
    fn new(rx: Receiver<Vec<u8>>) -> Self {
        Self {
            rx,
            current: Vec::new(),
            at: 0,
        }
    }
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.at >= self.current.len() {
            match self.rx.recv() {
                Ok(next) => {
                    self.current = next;
                    self.at = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.current.len() - self.at);
        buf[..n].copy_from_slice(&self.current[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// The shared process, as one member's `Link` sees it.
struct MemberChild {
    hub: Arc<Hub>,
    slot: u32,
    state: Arc<MemberState>,
}

impl ChildProcess for MemberChild {
    /// A member that is still in the group stands for the whole process: stopping it stops the
    /// group. One that was asked to leave is only abandoned.
    fn kill(&mut self) {
        match self.state.gone() {
            Some(_) => {}
            None if self.state.leaving.load(Ordering::SeqCst) => {
                self.hub.drop_member(self.slot, Gone::Left);
            }
            None => self.hub.kill(),
        }
    }

    fn try_wait(&mut self) -> Option<String> {
        match self.state.gone()? {
            Gone::Left => Some("the plugin left its process group".to_string()),
            Gone::Failed(text) => Some(format!(
                "the plugin could not join its process group: {text}"
            )),
            Gone::GroupEnded => self.hub.try_wait(),
        }
    }

    fn note(&self) -> Option<String> {
        matches!(self.state.gone(), Some(Gone::GroupEnded)).then(|| self.hub.note())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn a_channel_reader_joins_its_items_and_ends_with_the_channel() {
        let (tx, rx) = channel();
        tx.send(b"ab".to_vec()).unwrap();
        tx.send(b"cde".to_vec()).unwrap();
        drop(tx);
        let mut text = Vec::new();
        ChannelReader::new(rx).read_to_end(&mut text).unwrap();
        assert_eq!(text, b"abcde");
    }

    #[test]
    fn a_channel_reader_serves_a_small_buffer_in_pieces() {
        let (tx, rx) = channel();
        tx.send(b"abcdef".to_vec()).unwrap();
        drop(tx);
        let mut r = ChannelReader::new(rx);
        let mut two = [0u8; 2];
        let mut got = Vec::new();
        loop {
            let n = r.read(&mut two).unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&two[..n]);
        }
        assert_eq!(got, b"abcdef");
    }
}
