//! An idle frame of the shell allocates nothing: with no input, no panel open and nothing
//! animating, the per-frame passes only compare what they already hold. Each of them used to
//! build a list, a string or a map entry before it found there was nothing to do (the MIDI
//! mapping rows for a panel that is not open, the plugin device list, the pinned plugin params,
//! the list of open menus, the inputs listening offers).

use atomik_app::AppCore;
use atomik_app::midi::{MidiConnection, MidiLink};
use atomik_app::session::{AudioBackend, LiveStream};
use atomik_app::session::{LiveSession, NullBackend};
use atomik_app::settings::AudioSettings;
use atomik_app::shell::{Shell, ShellOptions};
use atomik_app::windows::WindowKind;
use atomik_audio_io::{HostInfo, IoError, StreamInfo, StreamSpec, Telemetry};
use atomik_engine::{Engine, FrameClock};
use atomik_midi_io::MidiError;
use atomik_model::{
    Command, DeviceKey, MapOptions, MidiMapCommand, MidiMapping, MidiSource, MidiTarget, SourceKind,
};
use atomik_ui::UiEvent;
use atomik_ui::kurbo::Size;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

atomik_testkit::install_alloc_guard!();

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    s
}

/// Frames at the idle rate (ten a second): a warm-up that fills the caches, then the measured
/// stretch, which is long enough to contain every once-a-second pass.
fn idle_allocations(s: &mut Shell<NullBackend>) -> u64 {
    for i in 0..30 {
        s.frame_at(f64::from(i) * 0.1);
    }
    let (_, n) = atomik_testkit::no_alloc::allocations_of(|| {
        for i in 30..60 {
            s.frame_at(f64::from(i) * 0.1);
        }
    });
    n
}

#[test]
fn an_idle_frame_allocates_nothing() {
    assert_eq!(idle_allocations(&mut shell()), 0);
}

#[test]
fn an_idle_frame_with_a_patch_open_allocates_nothing() {
    let mut s = shell();
    s.open_file(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/patches/slice-demo.v2.json"
    )));
    assert!(!s.core().doc().devices.is_empty(), "the patch opened");
    assert_eq!(idle_allocations(&mut s), 0);
}

#[test]
fn an_idle_frame_with_midi_mappings_and_no_panel_allocates_nothing() {
    let mut s = shell();
    s.open_file(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/patches/slice-demo.v2.json"
    )));
    let synth = s.core().doc().devices[0].key.clone();
    for cc in 1..=8u8 {
        let mapping = MidiMapping {
            id: 0,
            source: MidiSource {
                port: None,
                channel: None,
                kind: SourceKind::Cc(cc),
                surface: None,
            },
            target: MidiTarget::Param {
                device: synth.clone(),
                param: "filter.cutoff".into(),
            },
            options: MapOptions::default(),
        };
        s.core_mut()
            .apply_command(Command::MidiMap(MidiMapCommand::Add(mapping)))
            .unwrap();
    }
    assert_eq!(s.core().doc().midi_maps.len(), 8);
    assert!(!s.midi_maps_open(), "the panel is closed");
    assert_eq!(idle_allocations(&mut s), 0);
}

/// A shell of the windowed app's kind (it analyses only what is shown) with every analyser and
/// the two devices that carry the observed flag in its rack.
fn shell_with_analysers() -> Shell<NullBackend> {
    let mut s = shell();
    s.core_mut().set_analyse_only_shown(true);
    for (i, type_id) in [
        "atomik.spectrum",
        "atomik.phase-meter",
        "atomik.loudness-meter",
        "atomik.tuner",
        "atomik.mastering",
    ]
    .into_iter()
    .enumerate()
    {
        s.core_mut()
            .apply_command(Command::AddDevice {
                index: usize::MAX,
                type_id: type_id.into(),
                key: Some(DeviceKey(format!("a{i}"))),
            })
            .unwrap();
    }
    s
}

#[test]
fn an_idle_frame_with_analysers_on_screen_allocates_nothing() {
    let mut s = shell_with_analysers();
    assert_eq!(s.core().analyser_devices().len(), 4);
    assert_eq!(idle_allocations(&mut s), 0);
}

#[test]
fn an_idle_frame_with_analysers_in_a_minimized_window_allocates_nothing() {
    let mut s = shell_with_analysers();
    s.set_window_minimized(WindowKind::Main, true);
    assert_eq!(idle_allocations(&mut s), 0);
    assert!(
        !s.core().view().window_shown(),
        "the rack is told it is hidden"
    );
}

/// The fx-demo project, opened in `s` (the project file, so its song and console load too).
fn open_fx_demo<B: atomik_app::session::AudioBackend>(s: &mut Shell<B>) {
    s.open_file(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/projects/fx-demo/fx-demo.atomik"
    )));
    assert!(s.core().doc().devices.len() > 3, "the project opened");
}

/// An idle frame of the fx-demo allocates nothing: the console's meter feed hands its readings
/// to the console and rack views by reference, in a buffer it keeps from frame to frame.
#[test]
fn an_idle_frame_with_the_fx_demo_open_allocates_nothing() {
    let mut s = shell();
    open_fx_demo(&mut s);
    assert_eq!(idle_allocations(&mut s), 0);
}

/// A backend with no device: the engine the session starts is kept where the test can render it
/// block by block, so the transport moves exactly as far as the test lets it.
#[derive(Clone, Default)]
struct Bench {
    engine: Rc<RefCell<Option<Engine>>>,
    blocks: Rc<Cell<u64>>,
}

struct BenchStream {
    info: StreamInfo,
    bench: Bench,
}

impl LiveStream for BenchStream {
    fn telemetry(&self) -> Telemetry {
        Telemetry {
            callbacks: self.bench.blocks.get(),
            buffer_frames: 256,
            ..Telemetry::default()
        }
    }
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn stop(self) -> Result<Engine, IoError> {
        self.bench
            .engine
            .borrow_mut()
            .take()
            .ok_or_else(|| IoError::HostUnavailable("the engine is gone".into()))
    }
}

impl AudioBackend for Bench {
    type Stream = BenchStream;
    type Midi = MidiConnection;

    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        Ok(Vec::new())
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        Ok(Vec::new())
    }

    fn start(
        &self,
        spec: &StreamSpec,
        _input_device: Option<&str>,
        engine: Engine,
        _clock: Arc<FrameClock>,
    ) -> Result<BenchStream, IoError> {
        *self.engine.borrow_mut() = Some(engine);
        Ok(BenchStream {
            info: StreamInfo {
                host: spec.host,
                output_device: "Bench".into(),
                input_device: None,
                sample_rate: spec.sample_rate,
                output_channels: spec.output_channels,
                input_channels: 0,
                sample_format: "f32",
                requested_buffer_frames: Some(spec.buffer_frames),
            },
            bench: self.clone(),
        })
    }

    fn open_midi(&self, name: &str, _link: MidiLink) -> Result<MidiConnection, MidiError> {
        Err(MidiError::PortNotFound(name.to_string()))
    }
}

impl Bench {
    /// Renders `frames` frames of audio, the way the audio callback would.
    fn render(&self, frames: usize) {
        let mut engine = self.engine.borrow_mut();
        let engine = engine.as_mut().expect("the engine runs");
        let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
        engine.render(&[], &mut [l.as_mut_slice(), r.as_mut_slice()]);
        self.blocks.set(self.blocks.get() + 1);
    }
}

/// Frames a window shows each second while the transport plays.
const FPS: f64 = 60.0;

/// Heap operations of `frames` playing frames of the fx-demo (one frame of audio each is
/// rendered between them, outside the count), after a warm-up, and how far the playhead went.
fn playhead_allocations(frames: u32) -> (u64, i64) {
    let bench = Bench::default();
    let session = LiveSession::new(bench.clone(), AudioSettings::default(), true);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    open_fx_demo(&mut s);
    assert!(s.core().sink().is_running(), "the engine runs");
    let per_frame = (48_000.0 / FPS) as usize;
    let mut at = 0.0;
    let mut step = |s: &mut Shell<Bench>| {
        bench.render(per_frame);
        at += 1.0 / FPS;
        atomik_testkit::no_alloc::allocations_of(|| s.frame_at(at)).1
    };
    for _ in 0..30 {
        step(&mut s);
    }
    s.core_mut().handle_action(atomik_ui::UiAction::Transport(
        atomik_ui::TransportAction::Play,
    ));
    for _ in 0..120 {
        step(&mut s);
    }
    let start = s.core().position();
    let total: u64 = (0..frames).map(|_| step(&mut s)).sum();
    (total, s.core().position() - start)
}

/// What sixty playing frames of the fx-demo allocate: the meter feeds and the playhead's tempo
/// lookup allocate nothing. What remains happens in five of the sixty frames, once per window
/// of the device CPU readout (about five a second), which rebuilds each track's chain summary
/// (`AppCore::feed_cpu_loads`, about 140 heap operations a pass). The bound keeps it from
/// growing; it drops to 0 when that pass reuses its buffers.
const FX_DEMO_PLAYHEAD_BOUND: u64 = 800;

#[test]
fn a_frame_where_only_the_playhead_moves_allocates_no_more_than_its_feeds() {
    let (total, moved) = playhead_allocations(60);
    assert!(moved > 0, "the playhead moved: {moved} ticks");
    eprintln!("playhead: {total} heap operations over 60 frames");
    assert!(
        total <= FX_DEMO_PLAYHEAD_BOUND,
        "{total} heap operations over 60 frames"
    );
}
