//! The whole player lifecycle against a fake backend (no audio or MIDI hardware): arguments →
//! build → engine → stream → MIDI → stats loop → stop → exit code.

use atomik_audio_io::{
    DeviceInfo, HostInfo, HostKind, IoError, StreamInfo, StreamSpec, Telemetry, XrunEvent, XrunKind,
};
use atomik_core::NoteKind;
use atomik_device_sdk::SocketId;
use atomik_engine::{DeviceInstanceId, Engine, FrameClock, LiveNoteSender, ScheduledNote};
use atomik_midi_io::MidiError;
use atomik_play::cli::main_with;
use atomik_play::{Backend, LiveStream, MidiPort};
use atomik_testkit::backend::{AcceptedBuffer, FakeAudio, FakeStream};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const MINIMAL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/patches/minimal.json"
);
const SLICE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/patches/slice-demo.json"
);

/// What `open_midi` was asked: port name, target device and socket, sample rate, latency frames.
type MidiCall = (String, DeviceInstanceId, SocketId, f64, u64);

/// What the fake saw, read back from the shared fake audio backend and the MIDI call.
#[derive(Debug, Default)]
struct Log {
    spec: Option<StreamSpec>,
    workers: Option<usize>,
    linger: Option<u64>,
    midi: Option<MidiCall>,
    peak_output: f32,
    stopped: bool,
    engine_frames_at_stop: u64,
}

/// The player's `Backend` over the shared fake audio backend: the figures of a stream that is
/// polled once a second (375 callbacks of 128 frames at 48 kHz), plus a MIDI input that presses
/// a key. The fields are what a test sets before the run; they are applied when the player
/// first calls the backend.
#[derive(Default)]
struct Fake {
    audio: FakeAudio,
    xruns: u64,
    /// Xrun events the stream hands out on its first drain.
    events: Vec<XrunEvent>,
    /// `StreamInfo::requested_buffer_frames`.
    accepted: Option<u32>,
    /// `Telemetry::buffer_frames` reported once the stream "runs".
    actual_buffer: u32,
    fail_start: bool,
    fail_midi: bool,
    fail_list: bool,
    midi: RefCell<Option<MidiCall>>,
}

impl Fake {
    fn new() -> Self {
        let audio = FakeAudio::new();
        audio.plug_device(DeviceInfo {
            name: "Fake Out".into(),
            max_inputs: 0,
            max_outputs: 2,
            sample_rates: vec![48_000],
            buffer_range: None,
            is_default_output: true,
        });
        Self {
            audio,
            accepted: Some(128),
            actual_buffer: 128,
            ..Self::default()
        }
    }

    /// Applies the test's settings to the shared backend.
    fn configure(&self) {
        let audio = &self.audio;
        audio.set_default_output("Fake Out");
        audio.set_midi_ports(&["Fake Keys"]);
        audio.accept_buffer(
            self.accepted
                .map_or(AcceptedBuffer::Refused, AcceptedBuffer::Fixed),
        );
        audio.set_buffer(self.actual_buffer);
        audio.callbacks_per_read(375);
        audio.render_per_read(8);
        audio.queue_events(self.events.clone());
        let xruns = self.xruns;
        audio.shape_telemetry(move |_, t| {
            t.xruns = xruns;
            t.dsp_load_avg = 0.05;
            t.dsp_load_peak = 0.10;
            t.render_overruns = xruns;
            t.max_barrier_spins = 40 * xruns;
        });
        audio.fail_list(
            self.fail_list
                .then(|| IoError::Backend("no audio here".into())),
        );
        if self.fail_start {
            audio.fail_starts(IoError::DeviceNotFound("\"Nope\"".into()));
        }
    }

    /// What the fake saw so far.
    fn log(&self) -> Log {
        let records = self.audio.start_records();
        Log {
            spec: self.audio.start_calls().last().cloned(),
            workers: records.last().map(|r| r.workers),
            linger: records.last().map(|r| r.linger),
            midi: self.midi.borrow().clone(),
            peak_output: self.audio.peak_output(),
            stopped: self.audio.stops() > 0,
            engine_frames_at_stop: self.audio.stop_frames().last().copied().unwrap_or(0),
        }
    }
}

/// The shared fake stream under the player's trait (both are foreign to this test crate).
struct PlayStream(FakeStream);

impl LiveStream for PlayStream {
    fn telemetry(&self) -> Telemetry {
        self.0.telemetry()
    }

    fn info(&self) -> &StreamInfo {
        self.0.info()
    }

    fn drain_events(&self) -> Vec<XrunEvent> {
        self.0.drain_events()
    }

    fn stop(self) -> Result<Engine, IoError> {
        self.0.stop()
    }
}

struct FakeMidi {
    _sender: LiveNoteSender,
    dropped: u64,
}

impl MidiPort for FakeMidi {
    fn port_name(&self) -> &str {
        "Fake Keys"
    }

    fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl Backend for Fake {
    type Stream = PlayStream;
    type Midi = FakeMidi;

    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        self.configure();
        self.audio.list_audio()
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        self.configure();
        Ok(self.audio.list_midi())
    }

    fn start(
        &self,
        spec: &StreamSpec,
        engine: Engine,
        _clock: Arc<FrameClock>,
    ) -> Result<PlayStream, IoError> {
        self.configure();
        self.audio.start(spec, None, engine).map(PlayStream)
    }

    fn open_midi(
        &self,
        name: &str,
        target: (DeviceInstanceId, SocketId),
        mut sender: LiveNoteSender,
        _clock: Arc<FrameClock>,
        sample_rate: f64,
        latency_frames: u64,
    ) -> Result<FakeMidi, MidiError> {
        if self.fail_midi {
            return Err(MidiError::PortNotFound(format!(
                "no input matches {name:?}"
            )));
        }
        *self.midi.borrow_mut() = Some((
            name.to_string(),
            target.0,
            target.1,
            sample_rate,
            latency_frames,
        ));
        // A key press, as a real keyboard would send it.
        sender
            .push(ScheduledNote {
                frame: 0,
                device: target.0,
                socket: target.1,
                kind: NoteKind::On {
                    key: 60,
                    velocity: 1.0,
                },
            })
            .unwrap();
        Ok(FakeMidi {
            _sender: sender,
            dropped: 0,
        })
    }
}

struct Run {
    code: i32,
    out: String,
    err: String,
    ticks: u32,
}

/// Runs the CLI against `fake`; `stop_after` ticks simulates Ctrl-C.
fn run_with(fake: &Fake, argv: &[&str], stop_after: Option<u32>) -> Run {
    let stop = AtomicBool::new(false);
    let mut ticks = 0u32;
    let mut sleep = |d: Duration| {
        assert_eq!(d, Duration::from_millis(100));
        ticks += 1;
        if stop_after.is_some_and(|n| ticks >= n) {
            stop.store(true, Ordering::SeqCst);
        }
        assert!(ticks < 100_000, "runaway loop");
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let argv = std::iter::once("atomik-play").chain(argv.iter().copied());
    let code = main_with(argv, fake, &stop, &mut sleep, &mut out, &mut err);
    Run {
        code,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
        ticks,
    }
}

fn run(fake: &Fake, argv: &[&str]) -> Run {
    run_with(fake, argv, None)
}

#[test]
fn timed_run_streams_the_patch_and_routes_midi_to_the_synth() {
    let fake = Fake::new();
    let r = run(
        &fake,
        &[MINIMAL, "--duration", "3", "--stats", "--midi", "Fake"],
    );
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(r.ticks, 30);
    let lines: Vec<&str> = r.out.lines().collect();
    assert_eq!(lines.len(), 4, "{}", r.out);
    assert_eq!(
        lines[0],
        "t=1s cb=375 buf=128 xruns=0 dsp=avg 5.0% peak 10.0% midi_dropped=0 events_dropped=0"
    );
    assert!(lines[2].starts_with("t=3s cb=1125 "), "{}", lines[2]);
    assert!(lines[3].starts_with("summary: t=3s "), "{}", lines[3]);
    assert!(lines[3].ends_with(" result=PASS"), "{}", lines[3]);
    assert!(
        r.err
            .contains("info: the patch's 1 note(s) are ignored in live mode"),
        "{}",
        r.err
    );

    let log = fake.log();
    let spec = log.spec.clone().unwrap();
    assert_eq!(
        spec,
        StreamSpec {
            host: HostKind::Default,
            device: None,
            sample_rate: 48_000,
            buffer_frames: 128,
            input_channels: 0,
            output_channels: 2,
        }
    );
    assert_eq!(log.workers, Some(0));
    let (name, device, socket, rate, latency) = log.midi.clone().unwrap();
    assert_eq!(name, "Fake");
    assert_eq!((device, socket), (DeviceInstanceId(1), SocketId(0)));
    assert!((rate - 48_000.0).abs() < 1e-9);
    assert_eq!(latency, 128);
    assert!(
        log.peak_output > 0.01,
        "the live note is heard: {}",
        log.peak_output
    );
    assert!(log.stopped && log.engine_frames_at_stop > 0);
    assert!(r.err.contains("Fake Keys -> synth:notes_in"), "{}", r.err);
}

#[test]
fn without_stats_only_the_summary_is_printed() {
    let fake = Fake::new();
    let r = run(&fake, &[MINIMAL, "--duration", "2"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out.lines().count(), 1, "{}", r.out);
    // Two per-second snapshots plus the final one taken at stop.
    assert!(
        r.out.starts_with("summary: t=2s callbacks=1125 "),
        "{}",
        r.out
    );
    assert!(fake.log().midi.is_none());
}

#[test]
fn xruns_in_a_timed_run_exit_1() {
    let fake = Fake {
        xruns: 2,
        ..Fake::new()
    };
    let r = run(&fake, &[MINIMAL, "--duration", "1"]);
    assert_eq!(r.code, 1, "{}{}", r.out, r.err);
    assert!(
        r.out.contains(" xruns=2 ") && r.out.contains("result=FAIL"),
        "{}",
        r.out
    );
    assert!(fake.log().stopped);
}

#[test]
fn ctrl_c_stops_cleanly_and_exits_0_even_with_xruns() {
    let fake = Fake {
        xruns: 5,
        ..Fake::new()
    };
    let r = run_with(&fake, &[MINIMAL, "--stats"], Some(25));
    assert_eq!(r.code, 0, "{}{}", r.out, r.err);
    assert_eq!(r.ticks, 25);
    assert_eq!(r.out.lines().filter(|l| l.starts_with("t=")).count(), 2);
    assert!(r.out.contains("summary: t=2s ") && r.out.contains("result=stopped"));
    assert!(fake.log().stopped);
}

#[test]
fn rate_override_reconfigures_the_engine_and_warns() {
    let fake = Fake::new();
    let r = run(
        &fake,
        &[MINIMAL, "--duration", "1", "--rate", "44100", "--midi", "k"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(
        r.err.contains("warning: --rate 44100 Hz differs"),
        "{}",
        r.err
    );
    let log = fake.log();
    assert_eq!(log.spec.as_ref().unwrap().sample_rate, 44_100);
    assert!((log.midi.as_ref().unwrap().3 - 44_100.0).abs() < 1e-9);
}

#[test]
fn options_reach_the_stream_spec_and_engine() {
    let fake = Fake::new();
    let r = run(
        &fake,
        &[
            SLICE,
            "--duration",
            "1",
            "--host",
            "wasapi",
            "--device",
            "Fake",
            "--buffer",
            "256",
            "--workers",
            "2",
            "--midi",
            "k",
            "--to",
            "lead:notes_in",
        ],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    let log = fake.log();
    let spec = log.spec.as_ref().unwrap();
    assert_eq!(spec.host, HostKind::Wasapi);
    assert_eq!(spec.device.as_deref(), Some("Fake"));
    assert_eq!(spec.buffer_frames, 256);
    assert_eq!(log.workers, Some(2));
    // slice-demo: bass(1), echo(2), lead(3).
    assert_eq!(log.midi.as_ref().unwrap().1, DeviceInstanceId(3));
}

#[test]
fn midi_latency_uses_the_actual_host_buffer() {
    // The host refused the fixed size and runs its default period.
    let fake = Fake {
        accepted: None,
        actual_buffer: 480,
        ..Fake::new()
    };
    let r = run(&fake, &[MINIMAL, "--duration", "1", "--midi", "k"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(fake.log().midi.as_ref().unwrap().4, 480);

    // No callback ever reports a size: fall back to the requested buffer.
    let fake = Fake {
        accepted: None,
        actual_buffer: 0,
        ..Fake::new()
    };
    let r = run(
        &fake,
        &[MINIMAL, "--duration", "1", "--midi", "k", "--buffer", "64"],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(fake.log().midi.as_ref().unwrap().4, 64);
}

fn assert_error(r: &Run, needle: &str) {
    assert_eq!(r.code, 2, "{}{}", r.out, r.err);
    // Info and warning lines may come first.
    let error = r.err.lines().find(|l| l.starts_with("error: "));
    assert!(
        error.is_some_and(|l| l.contains(needle)),
        "{} lacks {needle:?}",
        r.err
    );
}

#[test]
fn errors_exit_2_with_paths() {
    let fake = Fake::new();
    assert_error(
        &run(&fake, &["no/such/patch.json"]),
        "cannot read the patch",
    );

    let dir = std::env::temp_dir().join(format!("atomik-play-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("bad.json");
    let text = std::fs::read_to_string(MINIMAL)
        .unwrap()
        .replace("atomik.poly-synth", "atomik.kazoo");
    std::fs::write(&bad, text).unwrap();
    let bad = bad.to_str().unwrap();
    assert_error(
        &run(&fake, &[bad]),
        "error: devices[0].type: unknown device type",
    );

    assert_error(
        &run(&fake, &[MINIMAL, "--to", "synth:out_l"]),
        "synth:out_l is not a notes input",
    );
    assert_error(&run(&fake, &[MINIMAL, "--rate", "22050"]), "22050");
    assert_error(&run(&fake, &[MINIMAL, "--host", "nope"]), "nope");
    assert!(fake.log().spec.is_none(), "nothing was opened");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn asio_without_the_feature_is_a_clear_error() {
    if atomik_play::args::ASIO_COMPILED {
        return;
    }
    let fake = Fake::new();
    assert_error(&run(&fake, &[MINIMAL, "--host", "asio"]), "--features asio");
    assert!(fake.log().spec.is_none());
}

#[test]
fn device_failures_exit_2_and_release_the_stream() {
    let fake = Fake {
        fail_start: true,
        ..Fake::new()
    };
    assert_error(&run(&fake, &[MINIMAL]), "audio device not found");

    let fake = Fake {
        fail_midi: true,
        ..Fake::new()
    };
    assert_error(
        &run(&fake, &[MINIMAL, "--midi", "Nope"]),
        "no input matches",
    );
    assert!(fake.log().stopped, "the stream is stopped before exiting");
}

#[test]
fn midi_needs_a_notes_input() {
    let dir = std::env::temp_dir().join(format!("atomik-play-fx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fx.json");
    std::fs::write(
        &path,
        r#"{ "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120,
             "length_beats": 4, "output_channels": 2,
             "devices": [ { "id": "echo", "type": "atomik.stereo-delay" } ],
             "cables": [ { "from": "echo:out_l", "to": "interface:out_1" } ] }"#,
    )
    .unwrap();
    let path = path.to_str().unwrap();
    let fake = Fake::new();
    assert_error(
        &run(&fake, &[path, "--midi", "k"]),
        "no device in the patch has a notes input",
    );
    // Without MIDI the same patch plays.
    let r = run(&fake, &[path, "--duration", "1"]);
    assert_eq!(r.code, 0, "{}", r.err);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn list_prints_devices_and_midi_and_exits_0() {
    let fake = Fake::new();
    let r = run(&fake, &["--list"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("\"Fake Out\""), "{}", r.out);
    assert!(r.out.contains("midi inputs:\n  \"Fake Keys\""), "{}", r.out);
    assert!(fake.log().spec.is_none());

    let fake = Fake {
        fail_list: true,
        ..Fake::new()
    };
    let r = run(&fake, &["--list"]);
    assert_eq!(r.code, 0);
    assert!(
        r.err.contains("warning: cannot list audio devices"),
        "{}",
        r.err
    );
}

#[test]
fn help_and_version_exit_0() {
    let fake = Fake::new();
    let r = run(&fake, &["--help"]);
    assert_eq!(r.code, 0);
    assert!(r.out.contains("--to <DEVICE_ID:SOCKET>"), "{}", r.out);
    assert_eq!(run(&fake, &["--version"]).code, 0);
}

fn overrun(at_ms: u64, spins: u64) -> XrunEvent {
    XrunEvent {
        at: Duration::from_millis(at_ms),
        kind: XrunKind::RenderOverrun,
        load: 1.119,
        gap_frames: 128,
        barrier_spins: spins,
        frames: 128,
    }
}

#[test]
fn stats_print_each_xrun_event_and_the_split_summary() {
    let fake = Fake {
        xruns: 2,
        events: vec![overrun(400, 80), overrun(700, 3)],
        ..Fake::new()
    };
    let r = run(&fake, &[MINIMAL, "--duration", "2", "--stats"]);
    assert_eq!(r.code, 1, "{}{}", r.out, r.err);
    let lines: Vec<&str> = r.out.lines().collect();
    assert_eq!(
        lines[0],
        "xrun t=0.400s kind=render_overrun load=111.9% gap=128 frames=128 barrier_spins=80"
    );
    assert!(lines[1].starts_with("xrun t=0.700s "), "{}", r.out);
    assert!(lines[2].starts_with("t=1s "), "{}", r.out);
    let summary = lines.last().unwrap();
    assert!(
        summary.ends_with(
            " overruns=2 errors=0 max_barrier_spins=80 cb_p50_us=0.0 cb_p99_us=0.0 cb_p999_us=0.0 cb_max_us=0.0 spins_p99=0 result=FAIL"
        ),
        "{summary}"
    );
}

#[test]
fn without_stats_xrun_events_are_not_printed() {
    let fake = Fake {
        xruns: 1,
        events: vec![overrun(400, 80)],
        ..Fake::new()
    };
    let r = run(&fake, &[MINIMAL, "--duration", "1"]);
    assert_eq!(r.out.lines().count(), 1, "{}", r.out);
}

#[test]
fn log_file_gets_stats_events_and_summary() {
    let dir = std::env::temp_dir().join(format!("atomik-play-log-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("run.log");
    let fake = Fake {
        xruns: 1,
        events: vec![overrun(1500, 7)],
        ..Fake::new()
    };
    // Without --stats the log still records everything; stdout only has the summary.
    let r = run(
        &fake,
        &[MINIMAL, "--duration", "2", "--log", path.to_str().unwrap()],
    );
    assert_eq!(r.code, 1, "{}{}", r.out, r.err);
    assert_eq!(r.out.lines().count(), 1, "{}", r.out);
    let log = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert!(
        lines.iter().any(|l| l.starts_with("atomik-play: ")),
        "the banner is logged for context: {log}"
    );
    assert!(
        lines.contains(
            &"xrun t=1.500s kind=render_overrun load=111.9% gap=128 frames=128 barrier_spins=7"
        ),
        "{log}"
    );
    assert_eq!(
        lines.iter().filter(|l| l.starts_with("t=")).count(),
        2,
        "{log}"
    );
    assert_eq!(lines.last().copied(), r.out.lines().last(), "{log}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unwritable_log_file_is_an_error_before_anything_opens() {
    let fake = Fake::new();
    let r = run(&fake, &[MINIMAL, "--log", "no/such/dir/at/all/run.log"]);
    assert_error(&r, "cannot write the log");
    assert!(fake.log().spec.is_none(), "nothing was opened");
}

#[test]
fn worker_linger_reaches_the_engine() {
    let fake = Fake::new();
    let r = run(&fake, &[SLICE, "--duration", "1", "--workers", "2"]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(fake.log().linger, Some(500_000), "the player default");
    assert!(r.err.contains("worker linger 500000 spins"), "{}", r.err);

    let fake = Fake::new();
    let r = run(
        &fake,
        &[
            SLICE,
            "--duration",
            "1",
            "--workers",
            "2",
            "--worker-linger",
            "0",
        ],
    );
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        fake.log().linger,
        Some(atomik_engine::WORKER_SPIN_LIMIT),
        "0 restores the engine's short spin"
    );
}
