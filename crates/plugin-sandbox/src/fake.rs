//! A fake child for the unit tests: a thread that speaks the control protocol over an anonymous
//! pipe pair and another that serves a [`ChildPort`], so the parent's logic (RPC routing,
//! timeouts, notifications, Lost, Late and offline) is tested without starting a process.
//!
//! The fake is scripted per request: [`Action::Reply`] answers, [`Action::Silent`] never
//! answers (a hung plugin with no sign of life, until the parent kills it), [`Action::Blocked`]
//! does not answer either but its audio thread goes on (a plugin whose main thread is busy: the
//! parent must not stop it), [`Action::Exit`] ends the "process".

use crate::instance::RemoteInstance;
use crate::link::{LiveGuard, Timeouts};
use crate::transport::{ChildProcess, Transport};
use atomik_core::BATCH;
use atomik_plugin_host::{
    HostMessenger, InstanceId, ManagerEnd, PluginEvent, ProcessStatus, SleepRule, TailLength,
    plugin_channel,
};
use atomik_plugin_ipc::{
    AudioOp, Before, ChildPort, Doorbell, DoorbellName, FromChild, PROTOCOL_VERSION, ParentPort,
    ProcessorInfo, SharedBlock, ToChild, WireChannels, WireKind, WireLayout, WireParam, WirePreset,
    block_file_name, default_block_dir, read_frame, write_frame,
};
use std::io::{PipeReader, PipeWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

/// The pid the fake child reports.
pub(crate) const FAKE_PID: u32 = 4242;

/// What the fake does with a request.
pub(crate) enum Action {
    /// Send these frames (message, blob).
    Reply(Vec<(FromChild, Option<Vec<u8>>)>),
    /// Never answer: the child hangs until it is killed, and its audio thread stops too (no
    /// sign of life in the block).
    Silent,
    /// Answer with these frames once the test calls [`Control::release`]: the plugin's main
    /// thread is busy, but its audio thread runs and shows its sign of life.
    Blocked(Vec<(FromChild, Option<Vec<u8>>)>),
    /// Answer with these frames after this long: a plugin that is busy, not hung. The fake
    /// serves no other request meanwhile.
    Delayed(Duration, Vec<(FromChild, Option<Vec<u8>>)>),
    /// The process ends with this exit description.
    Exit(String),
}

type Script = Box<dyn FnMut(&ToChild, Option<&[u8]>) -> Option<Action> + Send>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How the fake's audio thread answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioMode {
    /// Answers every request at once: `main_out = main_in * 0.5`.
    Echo,
    /// Takes no request (a hung audio thread); resumes when set back to `Echo`.
    Stall,
}

/// The state shared between a test, the fake's threads and the parent's view of its process.
pub(crate) struct Control {
    killed: AtomicBool,
    exit: Mutex<Option<String>>,
    /// Signalled when the process ends (see [`Control::exit_silently`]) or the fake is dropped.
    exited: Condvar,
    /// How many waits for the end of the process are running (see [`Control::watching`]).
    watching: AtomicUsize,
    out: Mutex<Option<PipeWriter>>,
    err: Mutex<Option<PipeWriter>>,
    requests: Mutex<Vec<ToChild>>,
    restored: Mutex<Vec<Vec<u8>>>,
    stop: AtomicBool,
    audio_mode: AtomicU8,
    audio_delay_us: AtomicU64,
    audio_status: AtomicU8,
    audio_ops: Mutex<Vec<AudioOp>>,
    queued: Mutex<Vec<FromChild>>,
    audio_events: Mutex<Vec<Vec<PluginEvent>>>,
    info: Mutex<Option<ProcessorInfo>>,
    wake: AtomicBool,
    /// The whole process hangs (see [`Action::Silent`]): the audio thread neither serves nor
    /// shows a sign of life.
    dead: AtomicBool,
    /// A [`Action::Blocked`] request may be answered now.
    released: AtomicBool,
    /// Extra microseconds the audio thread idles per pass: a slow beat (a loaded machine).
    audio_idle_us: AtomicU64,
    /// A start that rides on a request is held until [`Control::release_start`] (a plugin that is
    /// slow to start, for as long as the test wants).
    start_held: AtomicBool,
    /// What a start answers (false: the plugin refuses it).
    start_refuses: AtomicBool,
    /// The work that rode on the requests the audio thread has answered, in order.
    audio_before: Mutex<Vec<Before>>,
}

impl Control {
    fn new(out: PipeWriter, err: PipeWriter) -> Self {
        Self {
            killed: AtomicBool::new(false),
            exit: Mutex::new(None),
            exited: Condvar::new(),
            watching: AtomicUsize::new(0),
            out: Mutex::new(Some(out)),
            err: Mutex::new(Some(err)),
            requests: Mutex::new(Vec::new()),
            restored: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
            audio_mode: AtomicU8::new(0),
            audio_delay_us: AtomicU64::new(0),
            audio_status: AtomicU8::new(0),
            audio_ops: Mutex::new(Vec::new()),
            queued: Mutex::new(Vec::new()),
            audio_events: Mutex::new(Vec::new()),
            info: Mutex::new(None),
            wake: AtomicBool::new(false),
            dead: AtomicBool::new(false),
            released: AtomicBool::new(false),
            audio_idle_us: AtomicU64::new(0),
            start_held: AtomicBool::new(false),
            start_refuses: AtomicBool::new(false),
            audio_before: Mutex::new(Vec::new()),
        }
    }

    /// Makes the audio thread idle `idle` on every pass, so its sign of life beats that rarely.
    pub fn set_audio_idle(&self, idle: Duration) {
        self.audio_idle_us
            .store(idle.as_micros() as u64, Ordering::SeqCst);
    }

    /// The next start that rides on a request is not performed until [`Control::release_start`].
    pub fn hold_start(&self) {
        self.start_held.store(true, Ordering::SeqCst);
    }

    /// Lets the held start (or the next one) be performed.
    pub fn release_start(&self) {
        self.start_held.store(false, Ordering::SeqCst);
    }

    /// Makes a start answer false (the plugin refuses to start) or true again.
    pub fn refuse_start(&self, refuses: bool) {
        self.start_refuses.store(refuses, Ordering::SeqCst);
    }

    /// The work that rode on each request the audio thread answered, in order.
    pub fn audio_before(&self) -> Vec<Before> {
        lock(&self.audio_before).clone()
    }

    /// Makes the next [`Action::Blocked`] request wait for [`Control::release`] again.
    pub fn hold(&self) {
        self.released.store(false, Ordering::SeqCst);
    }

    /// Lets the [`Action::Blocked`] request that is waiting (or the next one) be answered.
    pub fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
    }

    /// Sends a frame to the parent (a notification, or an unsolicited reply).
    pub fn send(&self, msg: &FromChild, blob: Option<&[u8]>) {
        if let Some(w) = lock(&self.out).as_mut() {
            let _ = write_frame(w, msg, blob);
        }
    }

    /// Writes to the fake process's stderr.
    pub fn stderr(&self, text: &str) {
        if let Some(w) = lock(&self.err).as_mut() {
            let _ = w.write_all(text.as_bytes());
            let _ = w.flush();
        }
    }

    /// The process ends with `description`: its pipes close.
    pub fn exit(&self, description: &str) {
        self.exit_silently(description);
        *lock(&self.err) = None;
        *lock(&self.out) = None;
    }

    /// The process ends with `description` but its pipes stay open, as when it has started a
    /// helper process that inherited them: only the process itself tells.
    pub fn exit_silently(&self, description: &str) {
        lock(&self.exit).get_or_insert_with(|| description.to_string());
        self.exited.notify_all();
    }

    /// How many waits for the end of the process are running: the watcher of the link, which
    /// ends with the process.
    pub fn watching(&self) -> usize {
        self.watching.load(Ordering::SeqCst)
    }

    /// Blocks until the process ended (true) or the fake was dropped (false).
    fn wait_exit(&self) -> bool {
        self.watching.fetch_add(1, Ordering::SeqCst);
        let ended = self.wait_exit_inner();
        self.watching.fetch_sub(1, Ordering::SeqCst);
        ended
    }

    fn wait_exit_inner(&self) -> bool {
        let mut exit = lock(&self.exit);
        while exit.is_none() {
            if self.stop.load(Ordering::SeqCst) {
                return false;
            }
            exit = self
                .exited
                .wait(exit)
                .unwrap_or_else(PoisonError::into_inner);
        }
        true
    }

    /// True once the parent killed the fake process.
    pub fn was_killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }

    /// Every request the fake received, in order.
    pub fn requests(&self) -> Vec<ToChild> {
        lock(&self.requests).clone()
    }

    /// Blobs of the `RestoreState` requests.
    pub fn restored(&self) -> Vec<Vec<u8>> {
        lock(&self.restored).clone()
    }

    /// The ops the fake's audio thread has answered.
    pub fn audio_ops(&self) -> Vec<AudioOp> {
        lock(&self.audio_ops).clone()
    }

    /// The events of every request the audio thread has answered, in order.
    pub fn audio_events(&self) -> Vec<Vec<PluginEvent>> {
        lock(&self.audio_events).clone()
    }

    /// Queues a notification that is sent just before the answer to the next `Flush`.
    pub fn queue_notification(&self, msg: FromChild) {
        lock(&self.queued).push(msg);
    }

    /// Sets how the audio thread answers.
    pub fn set_audio(&self, mode: AudioMode) {
        self.audio_mode.store(mode as u8, Ordering::SeqCst);
    }

    /// Makes the audio thread take `delay` before it answers each request.
    pub fn set_audio_delay(&self, delay: Duration) {
        self.audio_delay_us
            .store(delay.as_micros() as u64, Ordering::SeqCst);
    }

    /// Sets the status the audio thread answers with.
    pub fn set_audio_status(&self, status: ProcessStatus) {
        self.audio_status.store(
            match status {
                ProcessStatus::Continue => 0,
                ProcessStatus::ContinueIfNotQuiet => 1,
                ProcessStatus::Tail => 2,
                ProcessStatus::Sleep => 3,
                ProcessStatus::Error => 4,
                ProcessStatus::Late => 5,
                ProcessStatus::Lost => 6,
                ProcessStatus::Refused => 7,
                ProcessStatus::StartFailed => 8,
            },
            Ordering::SeqCst,
        );
    }

    /// The facts the audio thread writes into the block at its next turn.
    pub fn set_info(&self, rule: SleepRule, tail: TailLength) {
        *lock(&self.info) = Some(ProcessorInfo {
            sleep_rule: rule,
            tail,
        });
    }

    /// The plugin asks to be processed.
    pub fn request_wake(&self) {
        self.wake.store(true, Ordering::SeqCst);
    }
}

struct FakeProcess(Arc<Control>);

impl ChildProcess for FakeProcess {
    fn kill(&mut self) {
        self.0.killed.store(true, Ordering::SeqCst);
        self.0.exit("the fake process was killed");
    }

    fn try_wait(&mut self) -> Option<String> {
        lock(&self.0.exit).clone()
    }

    fn exit_waiter(&mut self) -> Option<crate::transport::ExitWaiter> {
        let ctl = Arc::clone(&self.0);
        Some(Box::new(move || ctl.wait_exit()))
    }
}

/// The wire form of the layout every fake plugin has.
pub(crate) fn fake_layout() -> WireLayout {
    WireLayout {
        name: "Fake Gain".to_string(),
        vendor: "Atomik".to_string(),
        version: "1".to_string(),
        kind: WireKind::Effect,
        main_in: Some(WireChannels::Stereo),
        main_out: Some(WireChannels::Stereo),
        aux_out: 0,
        sidechain: false,
        notes_in: false,
        notes_out: false,
        params: vec![WireParam {
            id: 1,
            name: "Gain".to_string(),
            module: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            stepped: false,
            steps: 0,
        }],
    }
}

fn reply(msg: FromChild) -> Action {
    Action::Reply(vec![(msg, None)])
}

fn default_script(ctl: &Control, req: &ToChild, blob: Option<&[u8]>) -> Action {
    match req {
        ToChild::Instantiate { id } => reply(FromChild::Instantiated {
            id: *id,
            layout: fake_layout(),
            has_gui: true,
            latency: 7,
        }),
        ToChild::Activate { id, .. } => reply(FromChild::Activated {
            id: *id,
            latency: 9,
        }),
        ToChild::Deactivate { id } | ToChild::CloseGui { id } => reply(FromChild::Ok { id: *id }),
        ToChild::CaptureState { id } => Action::Reply(vec![(
            FromChild::State { id: *id },
            Some(b"fake state".to_vec()),
        )]),
        ToChild::RestoreState { id } => {
            lock(&ctl.restored).push(blob.unwrap_or_default().to_vec());
            reply(FromChild::Ok { id: *id })
        }
        ToChild::ParamText { id, plain, .. } => reply(FromChild::Text {
            id: *id,
            text: Some(format!("{plain:.2} units")),
        }),
        ToChild::LatencyFrames { id } => reply(FromChild::Latency { id: *id, frames: 9 }),
        ToChild::DefaultPins { id } => reply(FromChild::Pins {
            id: *id,
            pins: vec![1, 2],
        }),
        ToChild::ReadParams { id } => reply(FromChild::ParamValues {
            id: *id,
            values: vec![(1, 0.5), (2, 2.0)],
        }),
        ToChild::ListPresets { id } => reply(FromChild::Presets {
            id: *id,
            presets: vec![
                WirePreset {
                    id: "a".into(),
                    name: "Init".into(),
                    group: "Factory".into(),
                },
                WirePreset {
                    id: "b".into(),
                    name: "Warm".into(),
                    group: String::new(),
                },
            ],
        }),
        ToChild::LoadPreset { id, preset } => {
            lock(&ctl.restored).push(format!("preset:{preset}").into_bytes());
            reply(FromChild::Ok { id: *id })
        }
        ToChild::TakeLayoutChanged { id } => reply(FromChild::LayoutChanged {
            id: *id,
            layout: None,
        }),
        ToChild::OpenGui { id, .. } => reply(FromChild::GuiOpened {
            id: *id,
            w: 640,
            h: 480,
        }),
        ToChild::ResizeGui { id, w, h } => reply(FromChild::GuiSized {
            id: *id,
            w: *w,
            h: *h,
        }),
        ToChild::Shutdown => {
            Action::Exit("the plugin process exited with code 0 (0x0)".to_string())
        }
        ToChild::Flush { id } => {
            let mut frames: Vec<_> = lock(&ctl.queued).drain(..).map(|m| (m, None)).collect();
            frames.push((FromChild::Ok { id: *id }, None));
            Action::Reply(frames)
        }
        ToChild::Hello { .. } => Action::Reply(Vec::new()),
    }
}

fn run_control(ctl: &Control, mut stdin: PipeReader, mut script: Script, protocol: u32) {
    ctl.send(
        &FromChild::Hello {
            protocol,
            pid: FAKE_PID,
        },
        None,
    );
    while let Ok((req, blob)) = read_frame::<ToChild>(&mut stdin) {
        lock(&ctl.requests).push(req.clone());
        if matches!(req, ToChild::Hello { .. }) {
            continue;
        }
        let action = script(&req, blob.as_deref())
            .unwrap_or_else(|| default_script(ctl, &req, blob.as_deref()));
        match action {
            Action::Reply(frames) => {
                for (msg, blob) in frames {
                    ctl.send(&msg, blob.as_deref());
                }
            }
            Action::Silent => {
                ctl.dead.store(true, Ordering::SeqCst);
                while !ctl.killed.load(Ordering::SeqCst) && !ctl.stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                return;
            }
            Action::Blocked(frames) => {
                while !ctl.released.load(Ordering::SeqCst) {
                    if ctl.killed.load(Ordering::SeqCst) || ctl.stop.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                for (msg, blob) in frames {
                    ctl.send(&msg, blob.as_deref());
                }
            }
            Action::Delayed(delay, frames) => {
                let until = crate::clock::now() + delay;
                while crate::clock::now() < until {
                    if ctl.killed.load(Ordering::SeqCst) || ctl.stop.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                for (msg, blob) in frames {
                    ctl.send(&msg, blob.as_deref());
                }
            }
            Action::Exit(description) => {
                ctl.exit(&description);
                return;
            }
        }
    }
    // The parent closed our stdin without a Shutdown.
    ctl.exit("the plugin process exited with code 0 (0x0)");
}

fn status_of(code: u8) -> ProcessStatus {
    match code {
        1 => ProcessStatus::ContinueIfNotQuiet,
        2 => ProcessStatus::Tail,
        3 => ProcessStatus::Sleep,
        4 => ProcessStatus::Error,
        5 => ProcessStatus::Late,
        6 => ProcessStatus::Lost,
        7 => ProcessStatus::Refused,
        8 => ProcessStatus::StartFailed,
        _ => ProcessStatus::Continue,
    }
}

fn run_audio(ctl: &Control, mut port: ChildPort) {
    while !ctl.stop.load(Ordering::SeqCst) {
        if let Some(info) = lock(&ctl.info).take() {
            port.set_info(info);
        }
        if ctl.wake.swap(false, Ordering::SeqCst) {
            port.request_wake();
        }
        if ctl.audio_mode.load(Ordering::SeqCst) == AudioMode::Stall as u8
            || ctl.dead.load(Ordering::SeqCst)
        {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        let idle = ctl.audio_idle_us.load(Ordering::SeqCst);
        if idle > 0 {
            std::thread::sleep(Duration::from_micros(idle));
        }
        let Some(req) = port.wait_request(Duration::from_millis(2)) else {
            continue;
        };
        if ctl.audio_mode.load(Ordering::SeqCst) == AudioMode::Stall as u8 {
            // Stalled while waiting: the request stays unanswered and comes back later.
            continue;
        }
        let op = req.op();
        let before = req.before();
        if before.has(Before::START) {
            // A plugin that is slow to start: nothing is answered until the test lets it.
            while ctl.start_held.load(Ordering::SeqCst)
                && !ctl.stop.load(Ordering::SeqCst)
                && !ctl.dead.load(Ordering::SeqCst)
            {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        // The work that rides on the request is performed (and recorded) before the op.
        for (work, rides) in [
            (AudioOp::Stop, before.has(Before::STOP)),
            (AudioOp::Start, before.has(Before::START)),
            (AudioOp::Reset, before.has(Before::RESET)),
        ] {
            if rides {
                lock(&ctl.audio_ops).push(work);
            }
        }
        let refused = before.has(Before::START) && ctl.start_refuses.load(Ordering::SeqCst);
        let delay = ctl.audio_delay_us.load(Ordering::SeqCst);
        if delay > 0 {
            std::thread::sleep(Duration::from_micros(delay));
        }
        let status = status_of(ctl.audio_status.load(Ordering::SeqCst));
        req.complete(|io| {
            lock(&ctl.audio_events).push(io.events_in.to_vec());
            if refused {
                return (ProcessStatus::StartFailed, false);
            }
            for (ch, out) in io.main_out.iter_mut().enumerate() {
                if let Some(src) = io.main_in.get(ch) {
                    for i in 0..BATCH {
                        out[i] = src[i] * 0.5;
                    }
                }
            }
            (status, true)
        });
        // Recorded once the answer is published, so a test that sees the op can rely on it.
        if !refused {
            lock(&ctl.audio_ops).push(op);
        }
        lock(&ctl.audio_before).push(before);
    }
}

thread_local! {
    /// The block file of the last ports made on this thread.
    pub(crate) static LAST_BLOCK: std::cell::RefCell<std::path::PathBuf> = const { std::cell::RefCell::new(std::path::PathBuf::new()) };
}

static NEXT: AtomicU64 = AtomicU64::new(9_000_000);

/// A parent port and the child port of the same block, both in this process.
pub(crate) fn make_ports() -> (ParentPort, ChildPort) {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let path = default_block_dir().join(block_file_name(pid, n));
    LAST_BLOCK.with(|p| p.borrow_mut().clone_from(&path));
    let name = DoorbellName::for_instance(pid, n);
    let parent_block = SharedBlock::create(&path).expect("block");
    let child_block = SharedBlock::open(&path).expect("open block");
    let parent_bell = Doorbell::create(&name, &parent_block).expect("doorbell");
    let child_bell = Doorbell::open(&name, &child_block).expect("open doorbell");
    (
        ParentPort::new(parent_block, parent_bell),
        ChildPort::new(child_block, child_bell),
    )
}

/// Ends the fake's threads when the test is done with it.
pub(crate) struct Fake {
    pub ctl: Arc<Control>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.ctl.stop.store(true, Ordering::SeqCst);
        // Under the lock the waiter checks under, so it cannot miss the wake-up.
        let _held = lock(&self.ctl.exit);
        self.ctl.exited.notify_all();
    }
}

/// How a fake is built.
pub(crate) struct FakeOptions {
    /// Overrides the default answers: `None` falls back to them.
    pub script: Option<Script>,
    /// The protocol the fake's Hello names.
    pub protocol: u32,
}

impl Default for FakeOptions {
    fn default() -> Self {
        Self {
            script: None,
            protocol: PROTOCOL_VERSION,
        }
    }
}

impl FakeOptions {
    /// Answers with `script` where it returns `Some`, and as the default otherwise.
    pub fn script(
        mut self,
        script: impl FnMut(&ToChild, Option<&[u8]>) -> Option<Action> + Send + 'static,
    ) -> Self {
        self.script = Some(Box::new(script));
        self
    }
}

/// Timeouts short enough for tests, long enough for a busy CI machine.
pub(crate) fn test_timeouts() -> Timeouts {
    Timeouts {
        load: Duration::from_secs(5),
        create: Duration::from_secs(5),
        call: Duration::from_millis(600),
        text: Duration::from_millis(200),
        watchdog: Duration::from_millis(400),
        hung: Duration::ZERO,
        slow_retry: Duration::from_millis(20),
        slow_window: Duration::from_secs(5),
        bounce: false,
    }
}

/// A started fake and everything a test needs from it.
pub(crate) struct Started {
    pub instance: Result<RemoteInstance, atomik_plugin_host::BackendError>,
    pub fake: Fake,
    pub manager: ManagerEnd,
    pub live: Arc<AtomicUsize>,
}

/// Builds the transport of a fake child, plus its port. The parent port is returned for the
/// instance; the child port goes to the fake's audio thread.
pub(crate) fn fake_transport(
    options: FakeOptions,
    ports: (ParentPort, ChildPort),
) -> (Transport, ParentPort, Fake) {
    let (parent_port, child_port) = ports;
    let (to_child_r, to_child_w) = std::io::pipe().expect("pipe");
    let (from_child_r, from_child_w) = std::io::pipe().expect("pipe");
    let (err_r, err_w) = std::io::pipe().expect("pipe");
    let ctl = Arc::new(Control::new(from_child_w, err_w));
    let FakeOptions { script, protocol } = options;
    let script: Script = script.unwrap_or_else(|| Box::new(|_, _| None));
    {
        let ctl = Arc::clone(&ctl);
        std::thread::spawn(move || run_control(&ctl, to_child_r, script, protocol));
    }
    {
        let ctl = Arc::clone(&ctl);
        std::thread::spawn(move || run_audio(&ctl, child_port));
    }
    let transport = Transport {
        writer: Box::new(to_child_w),
        reader: Box::new(from_child_r),
        stderr: Some(Box::new(err_r)),
        child: Box::new(FakeProcess(Arc::clone(&ctl))),
    };
    (transport, parent_port, Fake { ctl })
}

/// The transport of a fake process that only exists: it takes everything written to it and says
/// nothing, and its pipes stay open until the test lets its process end (see
/// [`Control::exit_silently`]). For the tests of a group child, which must not be disturbed by a
/// fake that reads what the group writes as a plugin's requests.
pub(crate) fn idle_transport() -> (Transport, Fake) {
    let (from_child_r, from_child_w) = std::io::pipe().expect("pipe");
    let (_err_r, err_w) = std::io::pipe().expect("pipe");
    let ctl = Arc::new(Control::new(from_child_w, err_w));
    let transport = Transport {
        writer: Box::new(std::io::sink()),
        reader: Box::new(from_child_r),
        stderr: None,
        child: Box::new(FakeProcess(Arc::clone(&ctl))),
    };
    (transport, Fake { ctl })
}

/// Starts an instance over a fake child with the test timeouts.
pub(crate) fn start(options: FakeOptions) -> Started {
    start_with(options, test_timeouts())
}

/// Starts an instance over a fake child with these timeouts.
pub(crate) fn start_with(options: FakeOptions, timeouts: Timeouts) -> Started {
    start_with_budget(options, timeouts, atomik_plugin_host::SoftBudget::default())
}

/// Starts an instance over a fake child with these timeouts, drawing on `budget` (shared with
/// other instances that were given the same one).
pub(crate) fn start_with_budget(
    options: FakeOptions,
    timeouts: Timeouts,
    budget: atomik_plugin_host::SoftBudget,
) -> Started {
    let (_device, manager) = plugin_channel(InstanceId(1));
    let messenger: HostMessenger = manager.messenger();
    let live = Arc::new(AtomicUsize::new(0));
    let (transport, port, fake) = fake_transport(options, make_ports());
    let guard = LiveGuard::new(&live);
    let instance = RemoteInstance::start(transport, port, timeouts, messenger, guard, budget);
    Started {
        instance,
        fake,
        manager,
        live,
    }
}
