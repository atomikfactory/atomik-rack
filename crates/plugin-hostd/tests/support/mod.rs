//! A tiny "test parent": starts the `atomik-plugin-host` binary, owns the shared block and the
//! doorbell like the real parent does, speaks the control channel and the audio exchange. The
//! orchestrator's end-to-end tests can reuse it as it is.
//!
//! The child is killed when the [`TestParent`] drops, so no test leaves a process behind.
#![allow(dead_code)] // Justification: each test binary uses a different part of the helpers.

use atomik_core::{BATCH, Batch};
use atomik_plugin_host::{
    EventList, PluginEvent, PluginEventKind, PluginIo, PluginTransport, ProcessStatus,
};
use atomik_plugin_ipc::{
    AudioOp, Deadline, Doorbell, DoorbellName, FromChild, PROTOCOL_VERSION, ParentPort, Poll,
    Response, SharedBlock, ToChild, WireLayout, block_file_name, default_block_dir, read_frame,
    write_frame,
};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a test waits for the child before it gives up.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// The sample rate the tests activate at.
pub const SR: f64 = 48_000.0;

static NEXT: AtomicU64 = AtomicU64::new(1);

/// The path of the child binary.
pub fn host_binary() -> &'static str {
    env!("CARGO_BIN_EXE_atomik-plugin-host")
}

/// How to start a child.
#[derive(Debug, Clone)]
pub struct Spawn {
    /// `clap`, `vst3` or `null`.
    pub format: String,
    /// The plugin file (anything for `null`).
    pub file: PathBuf,
    /// The plugin id.
    pub plugin: String,
    /// Extra environment variables.
    pub envs: Vec<(String, String)>,
    /// Writes this protocol version in the block instead of the real one.
    pub block_version: Option<u32>,
}

impl Spawn {
    /// A built-in `null` plugin.
    pub fn null(plugin: &str) -> Self {
        Self {
            format: "null".to_string(),
            file: PathBuf::from("-"),
            plugin: plugin.to_string(),
            envs: Vec::new(),
            block_version: None,
        }
    }

    /// A real plugin of `format` (`clap` or `vst3`).
    pub fn real(format: &str, file: &Path, plugin: &str) -> Self {
        Self {
            format: format.to_string(),
            file: file.to_path_buf(),
            plugin: plugin.to_string(),
            envs: Vec::new(),
            block_version: None,
        }
    }

    /// Sets an environment variable of the child.
    #[must_use]
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.envs.push((key.to_string(), value.to_string()));
        self
    }

    /// Makes the block carry another protocol version.
    #[must_use]
    pub fn with_block_version(mut self, version: u32) -> Self {
        self.block_version = Some(version);
        self
    }
}

type Frame = io::Result<(FromChild, Option<Vec<u8>>)>;

/// What `process` got back.
#[derive(Debug)]
pub struct Out {
    /// The processor's status.
    pub status: ProcessStatus,
    /// The main output (zeroed beyond the channels asked for).
    pub main_out: [Batch; 2],
    /// The events the plugin emitted.
    pub events: Vec<PluginEvent>,
}

/// The id of the request a message answers, `None` for a notification.
pub fn reply_id(m: &FromChild) -> Option<u64> {
    match m {
        FromChild::Ok { id }
        | FromChild::Err { id, .. }
        | FromChild::Instantiated { id, .. }
        | FromChild::Activated { id, .. }
        | FromChild::State { id }
        | FromChild::Text { id, .. }
        | FromChild::Latency { id, .. }
        | FromChild::Pins { id, .. }
        | FromChild::LayoutChanged { id, .. }
        | FromChild::GuiOpened { id, .. } => Some(*id),
        _ => None,
    }
}

/// The parent end of one child process.
pub struct TestParent {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: Receiver<Frame>,
    /// The audio exchange.
    pub port: ParentPort,
    path: PathBuf,
    next_id: u64,
    /// Notifications that arrived while a request waited for its answer.
    pub notes: Vec<FromChild>,
    stderr: Arc<Mutex<String>>,
}

impl TestParent {
    /// Starts the child. Does not read the child's Hello: see [`TestParent::hello`].
    pub fn spawn(spec: &Spawn) -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = default_block_dir().join(block_file_name(pid, n));
        let bell_name = DoorbellName::for_instance(pid, n);
        let block = SharedBlock::create(&path).expect("the block is created");
        let bell = Doorbell::create(&bell_name, &block).expect("the doorbell is created");
        if let Some(version) = spec.block_version {
            patch_version(&path, version);
        }
        let port = ParentPort::new(block, bell);

        let mut cmd = Command::new(host_binary());
        cmd.arg(&spec.format)
            .arg(&spec.file)
            .arg(&spec.plugin)
            .arg("--block")
            .arg(&path)
            .arg("--doorbell")
            .arg(&bell_name.0)
            .arg("--parent-pid")
            .arg(pid.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &spec.envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("the child starts");

        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().expect("piped stdout");
        let (tx, frames) = channel();
        std::thread::spawn(move || {
            loop {
                let frame = read_frame::<FromChild>(&mut stdout);
                let end = frame.is_err();
                if tx.send(frame).is_err() || end {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let mut err = child.stderr.take().expect("piped stderr");
        let sink = Arc::clone(&stderr);
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while let Ok(n) = io::Read::read(&mut err, &mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
        Self {
            child,
            stdin,
            frames,
            port,
            path,
            next_id: 1,
            notes: Vec::new(),
            stderr,
        }
    }

    /// The child's process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Everything the child wrote to stderr so far.
    pub fn stderr(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Reads the child's Hello and answers with ours.
    pub fn hello(&mut self) -> FromChild {
        let hello = self.recv(PATIENCE).expect("the child says hello").0;
        self.send(&ToChild::Hello {
            protocol: PROTOCOL_VERSION,
        });
        hello
    }

    /// Writes one frame (a broken pipe is ignored: the child may be gone).
    pub fn send(&mut self, msg: &ToChild) {
        self.send_blob(msg, None);
    }

    /// Writes one frame with a blob.
    pub fn send_blob(&mut self, msg: &ToChild, blob: Option<&[u8]>) {
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = write_frame(stdin, msg, blob);
        }
    }

    /// The next frame from the child; `None` on timeout or when the channel ended.
    pub fn recv(&mut self, timeout: Duration) -> Option<(FromChild, Option<Vec<u8>>)> {
        match self.frames.recv_timeout(timeout) {
            Ok(Ok(frame)) => Some(frame),
            Ok(Err(_)) | Err(RecvTimeoutError::Disconnected | RecvTimeoutError::Timeout) => None,
        }
    }

    /// The next frame, with `Err` telling whether the stream ended (`true`) or timed out.
    pub fn recv_or_end(&mut self, timeout: Duration) -> Result<(FromChild, Option<Vec<u8>>), bool> {
        match self.frames.recv_timeout(timeout) {
            Ok(Ok(frame)) => Ok(frame),
            Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => Err(true),
            Err(RecvTimeoutError::Timeout) => Err(false),
        }
    }

    /// A fresh request id.
    pub fn id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Sends the request made by `make(id)` (with a blob when given) and waits for its answer;
    /// notifications that arrive first are kept in [`TestParent::notes`].
    pub fn call(
        &mut self,
        make: impl FnOnce(u64) -> ToChild,
        blob: Option<&[u8]>,
    ) -> (FromChild, Option<Vec<u8>>) {
        let id = self.id();
        self.send_blob(&make(id), blob);
        self.wait_reply(id)
    }

    /// Waits for the answer to request `id`.
    pub fn wait_reply(&mut self, id: u64) -> (FromChild, Option<Vec<u8>>) {
        loop {
            let Some((msg, blob)) = self.recv(PATIENCE) else {
                panic!(
                    "no answer to request {id}; the child said on stderr: {}",
                    self.stderr()
                );
            };
            match reply_id(&msg) {
                Some(got) if got == id => return (msg, blob),
                Some(other) => panic!("an answer to request {other} came while waiting for {id}"),
                None => self.notes.push(msg),
            }
        }
    }

    /// Instantiates the plugin named on the command line.
    pub fn instantiate(&mut self) -> (WireLayout, bool, u32) {
        match self.call(|id| ToChild::Instantiate { id }, None).0 {
            FromChild::Instantiated {
                layout,
                has_gui,
                latency,
                ..
            } => (layout, has_gui, latency),
            other => panic!("instantiate answered {other:?}"),
        }
    }

    /// Activates at [`SR`], live or offline; returns the latency.
    pub fn activate(&mut self, offline: bool) -> u32 {
        match self
            .call(
                |id| ToChild::Activate {
                    id,
                    sample_rate: SR,
                    frames: BATCH as u32,
                    offline,
                },
                None,
            )
            .0
        {
            FromChild::Activated { latency, .. } => latency,
            other => panic!("activate answered {other:?}"),
        }
    }

    /// Deactivates; panics unless the child answers Ok.
    pub fn deactivate(&mut self) {
        match self.call(|id| ToChild::Deactivate { id }, None).0 {
            FromChild::Ok { .. } => {}
            other => panic!("deactivate answered {other:?}"),
        }
    }

    /// Sends one block and waits for the result; `Err` with how the wait ended when the child
    /// did not answer (`Dead` or `TimedOut`).
    pub fn try_process(
        &mut self,
        main_in: &[Batch],
        events: &[PluginEvent],
        out_channels: usize,
        wait: Duration,
    ) -> Result<Out, Poll> {
        let mut main_out = [[0.0; BATCH]; 2];
        let mut events_out = EventList::with_capacity(512);
        let status = {
            let mut io = PluginIo {
                frames: BATCH,
                main_in,
                main_in_silent: false,
                sidechain: &[],
                sidechain_silent: true,
                main_out: &mut main_out[..out_channels],
                events_in: events,
                events_out: &mut events_out,
                transport: PluginTransport::default(),
                dropped: 0,
            };
            let t = self.port.submit(AudioOp::Process, &io);
            match self.port.wait_done(t, Deadline::after(wait)) {
                Poll::Done => {}
                other => return Err(other),
            }
            self.port.take_response(t, &mut io).status
        };
        Ok(Out {
            status,
            main_out,
            events: events_out.as_slice().to_vec(),
        })
    }

    /// Submits one block without waiting.
    pub fn submit_process(&mut self, main_in: &[Batch]) -> atomik_plugin_ipc::Ticket {
        let mut main_out = [[0.0; BATCH]; 2];
        let mut events_out = EventList::with_capacity(8);
        let io = PluginIo {
            frames: BATCH,
            main_in,
            main_in_silent: false,
            sidechain: &[],
            sidechain_silent: true,
            main_out: &mut main_out,
            events_in: &[],
            events_out: &mut events_out,
            transport: PluginTransport::default(),
            dropped: 0,
        };
        self.port.submit(AudioOp::Process, &io)
    }

    /// Sends one block and waits for the result.
    pub fn process(&mut self, main_in: &[Batch], events: &[PluginEvent]) -> Out {
        self.try_process(main_in, events, 2, PATIENCE)
            .expect("the child answers the block")
    }

    /// Runs an op that carries no audio.
    pub fn op(&mut self, op: AudioOp) -> Response {
        let t = self.port.submit_op(op);
        assert_eq!(
            self.port.wait_done(t, Deadline::after(PATIENCE)),
            Poll::Done,
            "the child answers {op:?}"
        );
        let mut main_out = [[0.0; BATCH]; 0];
        let mut events_out = EventList::with_capacity(8);
        let mut io = PluginIo {
            frames: BATCH,
            main_in: &[],
            main_in_silent: true,
            sidechain: &[],
            sidechain_silent: true,
            main_out: &mut main_out,
            events_in: &[],
            events_out: &mut events_out,
            transport: PluginTransport::default(),
            dropped: 0,
        };
        self.port.take_response(t, &mut io)
    }

    /// Closes the child's stdin (what a dead parent does).
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Kills the child.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// Waits up to `timeout` for the child to end.
    pub fn wait_exit(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let limit = Deadline::after(timeout);
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            if limit.passed() {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The block file of this child.
    pub fn block_path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestParent {
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Overwrites the protocol version in a block file's header.
fn patch_version(path: &Path, version: u32) {
    use std::io::{Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("the block file opens");
    file.seek(SeekFrom::Start(4)).expect("seek");
    file.write_all(&version.to_le_bytes()).expect("patch");
}

/// A ramp batch: sample `i` is `scale * (i + 1) / 64`.
pub fn ramp(scale: f32) -> Batch {
    let mut b = [0.0; BATCH];
    for (i, s) in b.iter_mut().enumerate() {
        *s = scale * (i as f32 + 1.0) / 64.0;
    }
    b
}

/// A parameter event at frame `offset`.
pub fn param(offset: u16, id: u32, value: f64) -> PluginEvent {
    PluginEvent {
        offset,
        kind: PluginEventKind::Param { id, value },
    }
}

/// A note on event.
pub fn note_on(offset: u16, key: u8, velocity: f32, note_id: i32) -> PluginEvent {
    PluginEvent {
        offset,
        kind: PluginEventKind::NoteOn {
            key,
            velocity,
            note_id,
        },
    }
}

/// A note off event.
pub fn note_off(offset: u16, key: u8, note_id: i32) -> PluginEvent {
    PluginEvent {
        offset,
        kind: PluginEventKind::NoteOff { key, note_id },
    }
}
