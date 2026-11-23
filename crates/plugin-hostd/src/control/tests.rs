//! The control with a fake plugin and a fake window system: the notifications of a tick and the
//! editor requests, without a process.

use super::*;
use crate::editor::fakes::{FakeInstance, FakeWindows, Log, log};
use atomik_plugin_host::{ParamPhase, ScannedPlugin};
use atomik_plugin_ipc::{
    ChildPort, Doorbell, DoorbellName, SharedBlock, WireParent, WirePhase, block_file_name,
    default_block_dir, into_backend_error, read_frame,
};
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A writer the test can read back.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Sink {
    /// How many bytes are waiting to be read.
    fn pending(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    /// The frames written since the last call.
    fn frames(&self) -> Vec<FromChild> {
        let data = std::mem::take(&mut *self.0.lock().unwrap());
        let mut cursor = io::Cursor::new(data);
        let mut out = Vec::new();
        while let Ok((msg, _)) = read_frame::<FromChild>(&mut cursor) {
            out.push(msg);
        }
        out
    }
}

/// A writer that always fails: the parent's pipe is gone.
struct Broken;

impl Write for Broken {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
}

struct FakeBackend {
    log: Log,
    on_poll: Vec<PluginMessage>,
    fail_open: bool,
    /// The host handle the last instance was given: the test posts through it.
    host: Arc<Mutex<Option<HostHandle>>>,
    /// The gate the next instance's preset walk waits for (see `FakeInstance::preset_gate`).
    gate: Arc<Mutex<Option<Gate>>>,
    /// The size of the state and of a param text the next instance gives.
    sizes: Arc<(AtomicUsize, AtomicUsize)>,
}

/// A gate a walk of preset folders waits for: open when the flag is true.
type Gate = Arc<(Mutex<bool>, std::sync::Condvar)>;

impl PluginBackend for FakeBackend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Clap
    }
    fn scan_file(
        &self,
        _path: &Path,
    ) -> Result<Vec<atomik_plugin_host::ScannedPlugin>, BackendError> {
        let _: Option<ScannedPlugin> = None;
        Err(BackendError::Unsupported("fake".to_string()))
    }
    fn instantiate(
        &self,
        _plugin: &PluginRef,
        host: HostHandle,
    ) -> Result<Box<dyn PluginInstance>, BackendError> {
        let mut i = FakeInstance::new(&self.log);
        *self.host.lock().unwrap() = Some(host.clone());
        i.host = Some(host);
        i.on_poll = self.on_poll.clone();
        i.fail_open = self.fail_open;
        i.preset_gate = self.gate.lock().unwrap().clone();
        i.state_len = self.sizes.0.load(Ordering::Relaxed);
        i.text_len = self.sizes.1.load(Ordering::Relaxed);
        Ok(Box::new(i))
    }
}

static NEXT: AtomicU64 = AtomicU64::new(900_000);

fn audio() -> AudioThread {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let path = default_block_dir().join(block_file_name(pid, n));
    let block = SharedBlock::create(&path).unwrap();
    let bell = Doorbell::create(&DoorbellName::for_instance(pid, n), &block).unwrap();
    AudioThread::spawn(ChildPort::new(block, bell)).unwrap()
}

fn args() -> Args {
    Args {
        format: Format::Null,
        plugin_file: "-".into(),
        plugin_id: "fake".to_string(),
        block: "-".into(),
        doorbell: "-".to_string(),
        parent_pid: 0,
    }
}

struct Rig {
    control: Control,
    sink: Sink,
    windows: FakeWindows,
    log: Log,
    host: Arc<Mutex<Option<HostHandle>>>,
    gate: Arc<Mutex<Option<Gate>>>,
    sizes: Arc<(AtomicUsize, AtomicUsize)>,
}

fn rig_with(
    on_poll: Vec<PluginMessage>,
    fail_open: bool,
    out: Option<Box<dyn Write + Send>>,
) -> Rig {
    let log = log();
    let sink = Sink::default();
    let host = Arc::new(Mutex::new(None));
    let gate = Arc::new(Mutex::new(None));
    let sizes = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
    let backend = FakeBackend {
        log: Rc::clone(&log),
        on_poll,
        fail_open,
        host: Arc::clone(&host),
        gate: Arc::clone(&gate),
        sizes: Arc::clone(&sizes),
    };
    let out = out.unwrap_or_else(|| Box::new(sink.clone()));
    let mut control = Control::new(&args(), Box::new(backend), audio(), out);
    let mut windows = FakeWindows::new(&log);
    control.on_event(
        Event::Frame(Box::new((
            ToChild::Hello {
                protocol: PROTOCOL_VERSION,
            },
            None,
        ))),
        &mut windows,
    );
    Rig {
        control,
        sink,
        windows,
        log,
        host,
        gate,
        sizes,
    }
}

fn rig() -> Rig {
    rig_with(Vec::new(), false, None)
}

impl Rig {
    fn send(&mut self, msg: ToChild) -> Option<u8> {
        self.control
            .on_event(Event::Frame(Box::new((msg, None))), &mut self.windows)
    }

    fn instantiated(mut self) -> Self {
        self.send(ToChild::Instantiate { id: 1 });
        assert!(matches!(
            self.sink.frames().as_slice(),
            [FromChild::Instantiated { .. }]
        ));
        self
    }

    fn open(&mut self, id: u64) {
        self.send(ToChild::OpenGui {
            id,
            owner: None,
            scale: 1.0,
            embed: false,
        });
        self.sink.frames();
        self.log.borrow_mut().clear();
    }

    fn lines(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

#[test]
fn a_tick_polls_the_plugin_and_forwards_what_it_posted_flags_first() {
    let mut r = rig_with(
        vec![
            PluginMessage::StateDirty,
            PluginMessage::ParamEdit {
                id: 3,
                value: 0.5,
                phase: ParamPhase::Change,
            },
            PluginMessage::RequestCallback,
            PluginMessage::LatencyChanged,
        ],
        false,
        None,
    )
    .instantiated();
    assert!(
        r.sink.frames().is_empty(),
        "nothing is forwarded before a tick"
    );
    assert_eq!(r.control.on_event(Event::Tick, &mut r.windows), None);
    assert_eq!(
        r.sink.frames(),
        vec![
            // The flags first, in their own order; the messages that carry data after them.
            FromChild::StateDirty,
            FromChild::LatencyChanged,
            FromChild::ParamEdit {
                param: 3,
                value: 0.5,
                phase: WirePhase::Change
            },
        ]
    );
    r.control.on_event(Event::Tick, &mut r.windows);
    assert!(r.sink.frames().is_empty(), "each message is forwarded once");
}

#[test]
fn opening_the_editor_answers_its_size_and_a_second_open_raises_the_same_window() {
    let mut r = rig().instantiated();
    r.send(ToChild::OpenGui {
        id: 2,
        owner: Some(WireParent::Win32(5)),
        scale: 1.0,
        embed: false,
    });
    assert_eq!(
        r.sink.frames(),
        vec![FromChild::GuiOpened {
            id: 2,
            w: 400,
            h: 300
        }]
    );
    assert!(
        r.lines()[0].starts_with("create Fake Some(Win32(5))"),
        "{:?}",
        r.lines()
    );
    r.log.borrow_mut().clear();
    r.send(ToChild::OpenGui {
        id: 3,
        owner: None,
        scale: 1.0,
        embed: false,
    });
    assert_eq!(
        r.sink.frames(),
        vec![FromChild::GuiOpened {
            id: 3,
            w: 400,
            h: 300
        }]
    );
    assert_eq!(r.lines(), vec!["show".to_string()]);
}

#[test]
fn the_editor_is_shown_only_after_the_answer_to_open_gui_is_written() {
    let mut r = rig().instantiated();
    let (sink, log) = (r.sink.clone(), Rc::clone(&r.log));
    r.windows.on_show = Some(Box::new(move || {
        log.borrow_mut()
            .push(format!("answered before show: {}", sink.pending() > 0));
    }));
    for id in [2, 3] {
        r.send(ToChild::OpenGui {
            id,
            owner: Some(WireParent::Win32(5)),
            scale: 1.0,
            embed: false,
        });
        let lines = r.lines();
        assert_eq!(
            &lines[lines.len() - 2..],
            ["show", "answered before show: true"],
            "open {id} (the second one raises): {lines:?}"
        );
        r.sink.frames();
        r.log.borrow_mut().clear();
    }
}

#[test]
fn an_embedded_editor_is_made_a_child_and_tells_the_app_its_size_after_the_answer() {
    let mut r = rig().instantiated();
    r.send(ToChild::OpenGui {
        id: 2,
        owner: Some(WireParent::X11(0x00A1_1CE0)),
        scale: 1.0,
        embed: true,
    });
    assert!(
        r.lines()[0].ends_with(" embedded"),
        "the window is made as a child: {:?}",
        r.lines()
    );
    let frames = r.sink.frames();
    assert_eq!(
        frames[..2],
        [
            FromChild::GuiOpened {
                id: 2,
                w: 400,
                h: 300
            },
            FromChild::GuiResize { w: 400, h: 300 }
        ],
        "the answer first, then the size the app's frame takes: {frames:?}"
    );
}

#[test]
fn the_apps_frame_resizing_an_embedded_editor_is_answered_with_the_size_it_took() {
    let mut r = rig().instantiated();
    r.send(ToChild::OpenGui {
        id: 2,
        owner: Some(WireParent::Win32(5)),
        scale: 1.0,
        embed: true,
    });
    r.sink.frames();
    r.log.borrow_mut().clear();
    r.send(ToChild::ResizeGui {
        id: 3,
        w: 900,
        h: 700,
    });
    assert_eq!(
        r.sink.frames(),
        vec![FromChild::GuiSized {
            id: 3,
            w: 900,
            h: 700
        }]
    );
    assert_eq!(r.lines(), ["resize_gui 900x700", "resize 900x700"]);
}

#[test]
fn a_frame_resize_of_an_editor_in_its_own_window_or_none_is_an_error() {
    let mut r = rig().instantiated();
    r.send(ToChild::ResizeGui { id: 2, w: 1, h: 1 });
    assert!(matches!(
        r.sink.frames().as_slice(),
        [FromChild::Err { id: 2, .. }]
    ));
    r.send(ToChild::OpenGui {
        id: 3,
        owner: None,
        scale: 1.0,
        embed: false,
    });
    r.sink.frames();
    r.log.borrow_mut().clear();
    r.send(ToChild::ResizeGui { id: 4, w: 1, h: 1 });
    assert!(matches!(
        r.sink.frames().as_slice(),
        [FromChild::Err { id: 4, .. }]
    ));
    assert!(r.lines().is_empty(), "the window is not touched");
}

#[test]
fn a_failed_open_shows_nothing() {
    let mut r = rig_with(Vec::new(), true, None).instantiated();
    r.send(ToChild::OpenGui {
        id: 2,
        owner: None,
        scale: 1.0,
        embed: false,
    });
    assert!(!r.lines().contains(&"show".to_string()));
}

#[test]
fn an_editor_that_cannot_open_is_an_error_and_the_child_goes_on() {
    let mut r = rig_with(Vec::new(), true, None).instantiated();
    let end = r.send(ToChild::OpenGui {
        id: 2,
        owner: None,
        scale: 1.0,
        embed: false,
    });
    assert_eq!(end, None);
    assert!(matches!(
        r.sink.frames().as_slice(),
        [FromChild::Err {
            id: 2,
            kind: WireErrorKind::Failed,
            ..
        }]
    ));
    assert!(r.lines().contains(&"destroy".to_string()));
}

#[test]
fn an_open_before_instantiate_is_an_error() {
    let mut r = rig();
    r.send(ToChild::OpenGui {
        id: 2,
        owner: None,
        scale: 1.0,
        embed: false,
    });
    assert!(matches!(
        r.sink.frames().as_slice(),
        [FromChild::Err { id: 2, .. }]
    ));
    assert!(r.lines().is_empty());
}

#[test]
fn close_gui_closes_quietly_and_a_user_close_tells_the_parent() {
    let mut r = rig().instantiated();
    r.open(2);
    r.send(ToChild::CloseGui { id: 3 });
    assert_eq!(
        r.sink.frames(),
        vec![FromChild::Ok { id: 3 }],
        "no GuiClosed"
    );
    assert_eq!(
        r.lines(),
        vec!["close_gui".to_string(), "destroy".to_string()]
    );

    r.open(4);
    let end = r
        .control
        .on_window(WindowInput::CloseRequested, &mut r.windows);
    assert_eq!(end, None);
    assert_eq!(r.sink.frames(), vec![FromChild::GuiClosed]);
    assert_eq!(
        r.lines(),
        vec!["close_gui".to_string(), "destroy".to_string()]
    );
    r.control
        .on_window(WindowInput::CloseRequested, &mut r.windows);
    assert!(
        r.sink.frames().is_empty(),
        "a window that is gone cannot close again"
    );
}

#[test]
fn a_user_resize_reaches_the_plugin() {
    let mut r = rig().instantiated();
    r.open(2);
    r.control
        .on_window(WindowInput::Resized(700, 500), &mut r.windows);
    assert_eq!(r.lines(), vec!["resize_gui 700x500".to_string()]);
}

#[test]
fn the_windows_frame_is_told_to_the_parent_only_while_the_editor_is_open() {
    let mut r = rig().instantiated();
    let frame = WindowInput::Frame {
        x: -8,
        y: 40,
        w: 716,
        h: 539,
    };
    r.control.on_window(frame, &mut r.windows);
    assert_eq!(r.sink.frames(), vec![], "no editor, no frame");
    r.open(2);
    let before = r.sink.frames().len();
    r.control.on_window(frame, &mut r.windows);
    assert_eq!(
        r.sink.frames()[before..].to_vec(),
        vec![FromChild::GuiFrame {
            x: -8,
            y: 40,
            w: 716,
            h: 539
        }]
    );
}

#[test]
fn the_plugins_own_resize_hide_and_close_follow_the_window_and_are_forwarded() {
    let mut r = rig_with(
        vec![
            PluginMessage::GuiResize { w: 900, h: 700 },
            PluginMessage::GuiRequestHide,
            PluginMessage::GuiClosed,
        ],
        false,
        None,
    )
    .instantiated();
    r.open(2);
    r.control.on_event(Event::Tick, &mut r.windows);
    assert_eq!(
        r.sink.frames(),
        vec![
            FromChild::GuiResize { w: 900, h: 700 },
            FromChild::GuiRequestHide,
            FromChild::GuiClosed
        ]
    );
    assert_eq!(
        r.lines(),
        vec![
            "resize 900x700".to_string(),
            "hide".to_string(),
            "close_gui".to_string(),
            "destroy".to_string()
        ]
    );
}

#[test]
fn the_end_of_stdin_closes_an_open_editor_and_ends_the_child() {
    let mut r = rig().instantiated();
    r.open(2);
    assert_eq!(r.control.on_event(Event::Eof, &mut r.windows), Some(0));
    assert_eq!(
        r.lines(),
        vec!["close_gui".to_string(), "destroy".to_string()]
    );
}

#[test]
fn a_broken_pipe_to_the_parent_ends_the_child() {
    let mut r = rig_with(Vec::new(), false, Some(Box::new(Broken)));
    assert_eq!(r.send(ToChild::Instantiate { id: 1 }), Some(0));
}

#[test]
fn what_the_plugin_posted_goes_out_before_the_reply_of_the_next_call() {
    let mut r = rig().instantiated();
    let host = r.host.lock().unwrap().clone().unwrap();
    host.messenger().post(PluginMessage::LatencyChanged);
    host.messenger().post(PluginMessage::StateDirty);
    r.send(ToChild::LatencyFrames { id: 5 });
    assert_eq!(
        r.sink.frames(),
        vec![
            FromChild::StateDirty,
            FromChild::LatencyChanged,
            FromChild::Latency { id: 5, frames: 0 }
        ],
        "notifications first (the flags in their fixed order), then the answer"
    );
}

#[test]
fn flush_polls_the_plugin_sends_its_notifications_and_then_answers_ok() {
    let mut r = rig_with(vec![PluginMessage::RequestRestart], false, None).instantiated();
    let host = r.host.lock().unwrap().clone().unwrap();
    host.messenger().post(PluginMessage::StateDirty);
    r.send(ToChild::Flush { id: 9 });
    assert_eq!(
        r.sink.frames(),
        vec![
            FromChild::StateDirty,
            FromChild::RequestRestart,
            FromChild::Ok { id: 9 }
        ]
    );
    r.send(ToChild::Flush { id: 10 });
    assert_eq!(r.sink.frames(), vec![FromChild::Ok { id: 10 }]);
}

#[test]
fn flush_before_instantiate_is_an_error() {
    let mut r = rig();
    r.send(ToChild::Flush { id: 1 });
    assert!(matches!(
        r.sink.frames().as_slice(),
        [FromChild::Err { id: 1, .. }]
    ));
}

#[test]
fn the_preset_walk_runs_on_a_thread_of_its_own_while_the_main_thread_goes_on() {
    let r = rig();
    let gate: Gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    *r.gate.lock().unwrap() = Some(Arc::clone(&gate));
    let mut r = r.instantiated();
    // The walk is held: it cannot finish until the test opens the gate.
    assert_eq!(r.send(ToChild::ListPresets { id: 5 }), None);
    assert!(
        r.sink.frames().is_empty(),
        "no answer is written by the main thread, which did not walk"
    );
    // The main thread answers the next request while the walk is still going.
    r.send(ToChild::LatencyFrames { id: 6 });
    assert_eq!(
        r.sink.frames(),
        vec![FromChild::Latency { id: 6, frames: 0 }]
    );
    {
        let (open, cv) = &*gate;
        *open.lock().unwrap() = true;
        cv.notify_all();
    }
    // The walk's thread writes its own answer; the bound is generous (a slow runner).
    let mut got = Vec::new();
    for _ in 0..3000 {
        got.extend(r.sink.frames());
        if !got.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(got.len(), 1, "{got:?}");
    match &got[0] {
        FromChild::Presets { id, presets } => {
            assert_eq!(*id, 5);
            assert_eq!(presets.len(), 1);
            assert_eq!(presets[0].id, "walked", "the list came from the walk");
        }
        other => panic!("not the list: {other:?}"),
    }
}

#[test]
fn a_plugin_whose_presets_need_it_is_asked_on_the_main_thread_as_before() {
    let mut r = rig().instantiated();
    assert_eq!(r.send(ToChild::ListPresets { id: 5 }), None);
    match r.sink.frames().as_slice() {
        [FromChild::Presets { id: 5, presets }] => assert_eq!(presets[0].id, "inline"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_tick_turns_slow_when_nothing_happens_and_fast_again_for_a_call_or_an_editor() {
    let mut r = rig().instantiated();
    assert!(r.control.wants_fast_tick(), "a new control starts fast");
    r.control.on_event(Event::Tick, &mut r.windows);
    assert!(
        !r.control.wants_fast_tick(),
        "a tick that forwarded nothing, with no editor, votes slow"
    );
    r.control.on_event(Event::Tick, &mut r.windows);
    assert!(!r.control.wants_fast_tick());
    r.send(ToChild::Flush { id: 9 });
    r.sink.frames();
    assert!(
        r.control.wants_fast_tick(),
        "a call from the parent votes fast"
    );
    r.control.on_event(Event::Tick, &mut r.windows);
    assert!(!r.control.wants_fast_tick());
    r.open(2);
    r.control.on_event(Event::Tick, &mut r.windows);
    assert!(
        r.control.wants_fast_tick(),
        "an open editor keeps the tick fast"
    );
    r.send(ToChild::CloseGui { id: 3 });
    r.sink.frames();
    r.control.on_event(Event::Tick, &mut r.windows);
    assert!(!r.control.wants_fast_tick(), "closed again: slow");
}

#[test]
fn a_tick_that_forwarded_something_stays_fast() {
    let mut r = rig_with(vec![PluginMessage::StateDirty], false, None).instantiated();
    r.control.on_event(Event::Tick, &mut r.windows);
    assert_eq!(r.sink.frames(), vec![FromChild::StateDirty]);
    assert!(r.control.wants_fast_tick(), "traffic keeps the rate up");
    r.control.on_event(Event::Tick, &mut r.windows);
    assert!(
        !r.control.wants_fast_tick(),
        "and a quiet tick after it lets it go"
    );
}

#[test]
fn a_state_over_the_frame_limit_is_an_error_naming_the_size_and_the_child_goes_on() {
    let r = rig();
    r.sizes.0.store(MAX_FRAME_BYTES + 1, Ordering::Relaxed);
    let mut r = r.instantiated();
    for id in [5, 6] {
        let end = r.send(ToChild::CaptureState { id });
        assert_eq!(end, None, "the child does not end for a large state");
        match r.sink.frames().as_slice() {
            [
                FromChild::Err {
                    id: got,
                    kind,
                    text,
                },
            ] => {
                assert_eq!(*got, id);
                assert_eq!(*kind, WireErrorKind::StateTooLarge);
                assert_eq!(
                    into_backend_error(*kind, text.clone()),
                    BackendError::StateTooLarge {
                        bytes: MAX_FRAME_BYTES as u64 + 1,
                        limit: MAX_FRAME_BYTES as u64,
                    }
                );
            }
            other => panic!("{other:?}"),
        }
    }
    // Still answering.
    assert_eq!(r.send(ToChild::LatencyFrames { id: 7 }), None);
    assert!(matches!(
        r.sink.frames().as_slice(),
        [FromChild::Latency { id: 7, .. }]
    ));
}

#[test]
fn a_reply_the_parent_would_refuse_is_an_error_and_the_child_goes_on() {
    let r = rig();
    // A param text longer than the JSON of a reply may be.
    r.sizes.1.store(MAX_REPLY_JSON_BYTES + 1, Ordering::Relaxed);
    let mut r = r.instantiated();
    let end = r.send(ToChild::ParamText {
        id: 8,
        param: 0,
        plain: 0.0,
    });
    assert_eq!(end, None);
    assert!(matches!(
        r.sink.frames().as_slice(),
        [FromChild::Err {
            id: 8,
            kind: WireErrorKind::Failed,
            ..
        }]
    ));
    assert_eq!(r.send(ToChild::LatencyFrames { id: 9 }), None);
    assert!(matches!(
        r.sink.frames().as_slice(),
        [FromChild::Latency { id: 9, .. }]
    ));
}
