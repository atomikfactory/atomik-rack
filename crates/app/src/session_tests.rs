//! `LiveSession` tests with a fake backend (no audio or MIDI hardware): the shared scriptable
//! audio side (`atomik_testkit::backend::FakeAudio`) and, here, the MIDI side.

use super::*;
use crate::midi::PortInput;
use crate::settings::{HostChoice, MidiInputRole, MidiInputSetting};
use crate::test_backend::{AcceptedBuffer, FakeAudio, FakeStream};
use atomik_audio_io::HostKind;
use atomik_core::SampleRate;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

#[derive(Default)]
struct Fake {
    /// The audio devices and streams: scripted and counted through this.
    audio: FakeAudio,
    opened: RefCell<Vec<String>>,
    closes: Rc<Cell<usize>>,
    /// The open inputs, to send messages through.
    inputs: FakeInputs,
    targets: RefCell<Vec<Arc<NoteTarget>>>,
    /// What the MIDI output thread sent, as `(port, bytes)`.
    out_wire: OutWire,
    /// The clock inputs opened, and the cell the last one feeds.
    clock_opened: RefCell<Vec<String>>,
    clock_cell: RefCell<Option<Arc<atomik_engine::ExternalClock>>>,
    /// How many opened clock inputs were closed (dropped).
    clock_closes: Arc<std::sync::atomic::AtomicUsize>,
    /// Opening a clock input fails while this is set.
    clock_missing: Cell<bool>,
}

/// What a fake MIDI output was sent: `(port, bytes)` in order.
type OutWire = Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

/// A MIDI output port that logs what it is sent.
struct FakeOut {
    name: String,
    wire: OutWire,
}

impl atomik_midi_io::out::MidiOutPort for FakeOut {
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        self.wire
            .lock()
            .unwrap()
            .push((self.name.clone(), bytes.to_vec()));
        Ok(())
    }
}

/// A fake open clock input: counts its close.
struct ClockGuard(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for ClockGuard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// The inputs a fake port has open, by port name.
type FakeInputs = Rc<RefCell<Vec<(String, Rc<RefCell<PortInput>>)>>>;

struct FakeMidi {
    name: String,
    inputs: FakeInputs,
    closes: Rc<Cell<usize>>,
}

impl MidiPort for FakeMidi {
    fn port_name(&self) -> &str {
        &self.name
    }
    fn dropped(&self) -> u64 {
        0
    }
    fn close(self) {
        self.closes.set(self.closes.get() + 1);
        self.inputs
            .borrow_mut()
            .retain(|(name, _)| *name != self.name);
    }
}

impl Fake {
    /// A backend with these MIDI input ports plugged in.
    fn with_ports(ports: &[&str]) -> Self {
        let fake = Self::default();
        fake.audio.set_midi_ports(ports);
        fake
    }

    /// A backend whose host accepts a fixed buffer of `frames`.
    fn accepting(frames: u32) -> Self {
        let fake = Self::default();
        fake.audio.accept_buffer(AcceptedBuffer::Fixed(frames));
        fake
    }

    /// Sends `bytes` into the open port `port`, as its MIDI thread would.
    fn send(&self, port: &str, bytes: &[u8]) {
        let input = self
            .inputs
            .borrow()
            .iter()
            .find(|(name, _)| name == port)
            .map(|(_, input)| Rc::clone(input))
            .unwrap_or_else(|| panic!("port {port} is not open"));
        input.borrow_mut().on_message(bytes);
    }

    /// The names of the open ports.
    fn open_ports(&self) -> Vec<String> {
        self.inputs
            .borrow()
            .iter()
            .map(|(n, _)| n.clone())
            .collect()
    }
}

impl AudioBackend for &Fake {
    type Stream = FakeStream;
    type Midi = FakeMidi;

    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        self.audio.list_audio()
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        Ok(self.audio.list_midi())
    }

    fn start(
        &self,
        spec: &StreamSpec,
        input_device: Option<&str>,
        engine: Engine,
        _clock: Arc<FrameClock>,
    ) -> Result<FakeStream, IoError> {
        self.audio.start(spec, input_device, engine)
    }

    fn midi_out_opener(&self) -> atomik_midi_io::out::Opener {
        let wire = Arc::clone(&self.out_wire);
        Arc::new(move |name| {
            Ok(Box::new(FakeOut {
                name: name.to_owned(),
                wire: Arc::clone(&wire),
            }) as Box<dyn atomik_midi_io::out::MidiOutPort>)
        })
    }

    fn open_clock_in(
        &self,
        name: &str,
        cell: Arc<atomik_engine::ExternalClock>,
    ) -> Result<Box<dyn std::any::Any + Send>, MidiError> {
        if self.clock_missing.get() {
            return Err(MidiError::PortNotFound(name.to_string()));
        }
        self.clock_opened.borrow_mut().push(name.to_owned());
        *self.clock_cell.borrow_mut() = Some(cell);
        Ok(Box::new(ClockGuard(Arc::clone(&self.clock_closes))))
    }

    fn open_midi(&self, name: &str, link: MidiLink) -> Result<FakeMidi, MidiError> {
        let visible = self.audio.visible_midi();
        let i = crate::midi::match_port(&visible, name)?;
        let port = visible[i].clone();
        self.opened.borrow_mut().push(port.clone());
        self.targets.borrow_mut().push(Arc::clone(&link.target));
        self.inputs.borrow_mut().push((
            port.clone(),
            Rc::new(RefCell::new(PortInput::new(&port, link))),
        ));
        Ok(FakeMidi {
            name: port,
            inputs: Rc::clone(&self.inputs),
            closes: Rc::clone(&self.closes),
        })
    }
}

fn settings() -> AudioSettings {
    AudioSettings {
        host: HostChoice::Wasapi,
        output_device: Some("Virtual Cable Input".into()),
        buffer_frames: 256,
        midi_inputs: Vec::new(),
        workers: 0,
        workers_auto: false,
        worker_linger_spins: 0,
        metronome_gain: 0.5,
        input_device: None,
        record_offset_frames: 0,
        pre_count_bars: 0,
        stream_threshold_bytes: None,
        stream_budget_bytes: None,
        plugins: atomik_plugin_host::PluginSettings::default(),
        surfaces: Vec::new(),
        midi_out_port: None,
        sync: crate::settings::SyncSettings::default(),
        device_loss: crate::settings::DeviceLossSettings::default(),
        master_tune_cents: 0.0,
    }
}

fn config(rate: f64) -> EngineConfig {
    config_in(rate, 0)
}

fn config_in(rate: f64, inputs: u16) -> EngineConfig {
    EngineConfig {
        sample_rate: SampleRate::new(rate).unwrap(),
        tempo: Tempo::new(120.0).unwrap(),
        input_channels: inputs,
        output_channels: 2,
        seed: 0,
    }
}

fn synth() -> Box<dyn Device> {
    crate::app_core::default_registry()
        .create("atomik.poly-synth")
        .unwrap()
}

#[test]
fn reset_opens_a_stream_with_the_settings_at_the_document_rate() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    assert!(!s.is_running(), "nothing opens before the first reset");
    s.reset(&config(44_100.0));
    assert!(s.is_running());
    assert_eq!(
        fake.audio.starts(),
        vec![StreamSpec {
            host: HostKind::Wasapi,
            device: Some("Virtual Cable Input".into()),
            sample_rate: 44_100,
            buffer_frames: 256,
            input_channels: 0,
            output_channels: 2,
        }]
    );
    let st = s.status();
    assert!(st.running);
    assert_eq!(st.host, "WASAPI");
    assert_eq!(st.device, "Virtual Cable Input");
    assert_eq!(st.sample_rate, 44_100);
    assert_eq!(st.error, None);
}

#[test]
fn engine_calls_reach_the_engine_while_running() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    s.insert_device(DeviceInstanceId(1), synth(), &[]).unwrap();
    assert!(s.contains_device(DeviceInstanceId(1)));
    s.remove_device(DeviceInstanceId(1)).unwrap();
    assert!(!s.contains_device(DeviceInstanceId(1)));
}

#[test]
fn a_disabled_session_opens_nothing_and_accepts_engine_calls() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), false);
    s.reset(&config(48_000.0));
    assert!(!s.is_running());
    assert!(fake.audio.starts().is_empty());
    assert!(s.insert_device(DeviceInstanceId(1), synth(), &[]).is_ok());
    assert!(s.set_tempo(Tempo::new(99.0).unwrap()).is_ok());
    assert_eq!(s.collect_garbage(), 0);
    assert!(!s.status().running);
    assert_eq!(s.status().host, "WASAPI");
    s.set_enabled(true);
    s.reset(&config(48_000.0));
    assert!(s.is_running());
}

#[test]
fn a_failed_start_reports_the_error_and_ignores_engine_calls() {
    let fake = Fake::default();
    fake.audio
        .fail_next_start(IoError::DeviceNotFound("Virtual Cable Input".into()));
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert!(!s.is_running());
    let err = s.status().error.clone().expect("error shown");
    assert!(err.contains("Virtual Cable Input"), "{err}");
    assert!(s.insert_device(DeviceInstanceId(1), synth(), &[]).is_ok());
    // The next reset tries again.
    s.reset(&config(48_000.0));
    assert!(s.is_running());
    assert_eq!(s.status().error, None);
}

#[test]
fn a_reset_stops_the_previous_stream_before_starting_the_next() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    s.insert_device(DeviceInstanceId(1), synth(), &[]).unwrap();
    s.reset(&config(48_000.0));
    assert_eq!(fake.audio.stops(), 1);
    assert_eq!(fake.audio.starts().len(), 2);
    assert!(
        !s.contains_device(DeviceInstanceId(1)),
        "a reset starts an empty engine"
    );
}

#[test]
fn the_midi_input_opens_with_the_shared_target() {
    let fake = Fake::with_ports(&["Example Keyboard"]);
    let mut s = LiveSession::new(
        &fake,
        AudioSettings {
            midi_inputs: vec![MidiInputSetting::enabled("Keyboard")],
            ..settings()
        },
        true,
    );
    s.reset(&config(48_000.0));
    assert_eq!(*fake.opened.borrow(), vec!["Example Keyboard".to_string()]);
    assert_eq!(s.status().midi_port.as_deref(), Some("Example Keyboard"));
    let shared = s.note_target().unwrap();
    assert!(Arc::ptr_eq(&fake.targets.borrow()[0], &shared));
}

fn inputs(entries: &[(&str, bool, MidiInputRole)]) -> Vec<MidiInputSetting> {
    entries
        .iter()
        .map(|(name, enabled, role)| MidiInputSetting {
            name: (*name).into(),
            enabled: *enabled,
            role: *role,
            ..Default::default()
        })
        .collect()
}

fn two_ports() -> Fake {
    Fake::with_ports(&["Example Keys", "Example Pads"])
}

fn keys_and_pads() -> AudioSettings {
    AudioSettings {
        midi_inputs: inputs(&[
            ("Example Keys", true, MidiInputRole::Keyboard),
            ("Example Pads", true, MidiInputRole::Keyboard),
        ]),
        ..settings()
    }
}

/// Aim the shared target at device 7 and record it, so what the ports play can be read back.
fn record_device_7(s: &mut LiveSession<&Fake>) {
    let target = s.note_target().unwrap();
    target.set(Some((DeviceInstanceId(7), atomik_device_sdk::SocketId(0))));
    s.set_recording(&[DeviceInstanceId(7)]);
}

#[test]
fn every_enabled_midi_input_is_open_at_once_and_plays_the_shared_target() {
    let fake = two_ports();
    let mut s = LiveSession::new(&fake, keys_and_pads(), true);
    s.reset(&config(48_000.0));
    assert_eq!(fake.open_ports(), ["Example Keys", "Example Pads"]);
    assert_eq!(
        s.status().midi_port.as_deref(),
        Some("Example Keys, Example Pads")
    );
    assert_eq!(s.status().midi_error, None);
    assert!(s.midi_complete());
    record_device_7(&mut s);
    fake.send("Example Keys", &[0x90, 60, 100]);
    fake.send("Example Pads", &[0x99, 36, 90]);
    // The target moves; each note-off still goes where its note-on went.
    s.note_target()
        .unwrap()
        .set(Some((DeviceInstanceId(8), atomik_device_sdk::SocketId(0))));
    fake.send("Example Pads", &[0x89, 36, 0]);
    fake.send("Example Keys", &[0x80, 60, 0]);
    let mut out = Vec::new();
    s.drain_recorded(&mut out);
    let keys: Vec<(u8, bool)> = out
        .iter()
        .map(|n| match n.kind {
            NoteKind::On { key, .. } => (key, true),
            NoteKind::Off { key } => (key, false),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(keys, [(60, true), (36, true), (36, false), (60, false)]);
    assert!(
        out.iter().all(|n| n.device == DeviceInstanceId(7)),
        "{out:?}"
    );
    assert!(
        out.windows(2).all(|w| w[0].frame <= w[1].frame),
        "one ordered queue: {out:?}"
    );
}

#[test]
fn switching_a_port_off_closes_only_that_port() {
    let fake = two_ports();
    let mut s = LiveSession::new(&fake, keys_and_pads(), true);
    s.reset(&config(48_000.0));
    let opened = fake.opened.borrow().len();
    let change = s.set_settings(AudioSettings {
        midi_inputs: inputs(&[
            ("Example Keys", true, MidiInputRole::Keyboard),
            ("Example Pads", false, MidiInputRole::Keyboard),
        ]),
        ..settings()
    });
    assert_eq!(change, SettingsChange::MidiReopened);
    assert_eq!(fake.audio.starts().len(), 1, "no audio restart");
    assert_eq!(fake.closes.get(), 1);
    assert_eq!(fake.open_ports(), ["Example Keys"]);
    assert_eq!(
        fake.opened.borrow().len(),
        opened,
        "the keyboard stayed open"
    );
    assert_eq!(s.status().midi_port.as_deref(), Some("Example Keys"));
    // The closed port is no longer there to play.
    assert!(s.midi_complete());
    // Everything off closes the last one.
    assert_eq!(s.set_settings(settings()), SettingsChange::MidiReopened);
    assert!(fake.open_ports().is_empty());
    assert_eq!(s.status().midi_port, None);
}

#[test]
fn a_port_plugged_in_later_opens_on_a_rescan() {
    let fake = two_ports();
    fake.audio.hide_midi_port("Example Pads");
    let mut s = LiveSession::new(&fake, keys_and_pads(), true);
    s.reset(&config(48_000.0));
    assert_eq!(fake.open_ports(), ["Example Keys"]);
    assert!(
        s.status().midi_error.is_some(),
        "the missing pads are named"
    );
    assert!(!s.midi_complete());
    // Still unplugged: nothing changes, and the keyboard is not reopened.
    s.rescan_midi();
    assert_eq!(fake.open_ports(), ["Example Keys"]);
    assert_eq!(fake.opened.borrow().len(), 1);
    // Plugged in: the rescan opens it and clears the error.
    fake.audio.show_midi_port("Example Pads");
    s.rescan_midi();
    assert_eq!(fake.open_ports(), ["Example Keys", "Example Pads"]);
    assert_eq!(fake.opened.borrow().len(), 2);
    assert_eq!(s.status().midi_error, None);
    assert!(s.midi_complete());
}

#[test]
fn a_mapping_only_port_feeds_the_mappings_and_plays_nothing() {
    let fake = two_ports();
    let mut s = LiveSession::new(
        &fake,
        AudioSettings {
            midi_inputs: inputs(&[
                ("Example Keys", true, MidiInputRole::Keyboard),
                ("Example Pads", true, MidiInputRole::Mapping),
            ]),
            ..settings()
        },
        true,
    );
    s.reset(&config(48_000.0));
    record_device_7(&mut s);
    fake.send("Example Pads", &[0x99, 36, 90]);
    fake.send("Example Keys", &[0x90, 60, 100]);
    let mut out = Vec::new();
    s.drain_recorded(&mut out);
    assert_eq!(out.len(), 1, "only the keyboard plays: {out:?}");
    let mut seen = Vec::new();
    s.hub.drain(&mut seen);
    assert_eq!(seen.len(), 2, "the mappings hear both ports");
    // Changing the role reopens just that port.
    let opened = fake.opened.borrow().len();
    s.set_settings(AudioSettings {
        midi_inputs: inputs(&[
            ("Example Keys", true, MidiInputRole::Keyboard),
            ("Example Pads", true, MidiInputRole::Keyboard),
        ]),
        ..settings()
    });
    assert_eq!(fake.opened.borrow().len(), opened + 1);
    fake.send("Example Pads", &[0x99, 37, 90]);
    s.drain_recorded(&mut out);
    assert_eq!(out.len(), 2, "now the pads play too: {out:?}");
}

#[test]
fn two_settings_naming_one_port_open_it_once() {
    let fake = two_ports();
    let mut s = LiveSession::new(
        &fake,
        AudioSettings {
            midi_inputs: inputs(&[
                ("Example Keys", true, MidiInputRole::Keyboard),
                ("Keys", true, MidiInputRole::Keyboard),
            ]),
            ..settings()
        },
        true,
    );
    s.reset(&config(48_000.0));
    assert_eq!(fake.open_ports(), ["Example Keys"]);
}
#[test]
fn a_missing_port_is_reported_and_a_fix_reopens_midi_without_a_restart() {
    let fake = Fake::with_ports(&["Example Keyboard"]);
    let mut s = LiveSession::new(
        &fake,
        AudioSettings {
            midi_inputs: vec![MidiInputSetting::enabled("Keystation")],
            ..settings()
        },
        true,
    );
    s.reset(&config(48_000.0));
    assert!(s.status().midi_error.is_some());
    assert_eq!(s.status().midi_port, None);
    let change = s.set_settings(AudioSettings {
        midi_inputs: vec![MidiInputSetting::enabled("Example")],
        ..settings()
    });
    assert_eq!(change, SettingsChange::MidiReopened);
    assert_eq!(fake.audio.starts().len(), 1, "no audio restart");
    assert_eq!(s.status().midi_port.as_deref(), Some("Example Keyboard"));
    assert_eq!(s.status().midi_error, None);
    // Turning MIDI off closes the port.
    assert_eq!(s.set_settings(settings()), SettingsChange::MidiReopened);
    assert_eq!(fake.closes.get(), 1);
    assert_eq!(s.status().midi_port, None);
    assert_eq!(s.status().midi_error, None);
}

#[test]
fn audio_changes_ask_for_a_restart_and_apply_at_the_next_reset() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert_eq!(s.set_settings(settings()), SettingsChange::None);
    for changed in [
        AudioSettings {
            buffer_frames: 512,
            ..settings()
        },
        AudioSettings {
            output_device: None,
            ..settings()
        },
        AudioSettings {
            host: HostChoice::Asio,
            ..settings()
        },
        AudioSettings {
            workers: 3,
            ..settings()
        },
        AudioSettings {
            worker_linger_spins: 7,
            ..settings()
        },
        AudioSettings {
            input_device: Some("Line In".into()),
            ..settings()
        },
    ] {
        let mut s2 = LiveSession::new(&fake, settings(), false);
        assert_eq!(s2.set_settings(changed), SettingsChange::RestartAudio);
    }
    assert_eq!(
        s.set_settings(AudioSettings {
            buffer_frames: 512,
            ..settings()
        }),
        SettingsChange::RestartAudio
    );
    assert_eq!(
        fake.audio.starts().len(),
        1,
        "the restart is the caller's call"
    );
    s.reset(&config(48_000.0));
    assert_eq!(fake.audio.starts()[1].buffer_frames, 512);
}

#[test]
fn polling_reads_telemetry_and_sets_the_midi_latency() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert_eq!(s.latency_frames(), FALLBACK_LATENCY_FRAMES);
    assert_eq!(s.status().buffer_frames, None);
    fake.audio.set_telemetry(Telemetry {
        callbacks: 10,
        xruns: 3,
        buffer_frames: 480,
        dsp_load_avg: 0.1,
        dsp_load_peak: 0.2,
        ..Telemetry::default()
    });
    let st = s.poll().clone();
    assert_eq!(st.buffer_frames, Some(480));
    assert_eq!(st.xruns, 3);
    assert!((st.dsp_load - 0.1).abs() < 1e-6);
    assert!((st.dsp_peak - 0.2).abs() < 1e-6);
    assert_eq!(s.latency_frames(), 480);
}

#[test]
fn polling_publishes_the_input_trims_and_the_requested_buffer() {
    let fake = Fake::accepting(480);
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert_eq!(s.status().buffer_requested, Some(480));
    assert_eq!(s.status().input_trim, crate::status::InputTrim::default());
    fake.audio.set_telemetry(Telemetry {
        input_trims: 3,
        input_backlog_max_frames: 1440,
        callback_frames_min: 96,
        callback_frames_max: 480,
        input_backlog_allowed_frames: 1536,
        input_backlog_extra_frames: 512,
        ..Telemetry::default()
    });
    let st = s.poll().clone();
    assert_eq!(
        st.input_trim,
        crate::status::InputTrim {
            trims: 3,
            backlog_max_frames: 1440,
            callback_min: 96,
            callback_max: 480,
            allowed_backlog_frames: 1536,
            extra_backlog_frames: 512,
        }
    );
    assert_eq!(
        crate::status::input_trim_line(&st),
        "input trimmed: 3 trims, backlog max 1440 frames, allowed 1536 frames, callback 96..480 frames, buffer 480"
    );
}

#[test]
fn an_accepted_buffer_size_is_the_initial_latency() {
    let fake = Fake::accepting(128);
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert_eq!(s.latency_frames(), 128);
    assert_eq!(s.status().buffer_frames, Some(128));
}

#[test]
fn shutdown_closes_midi_and_stops_the_stream() {
    let fake = Fake::with_ports(&["Example Keyboard"]);
    let mut s = LiveSession::new(
        &fake,
        AudioSettings {
            midi_inputs: vec![MidiInputSetting::enabled("Keyboard")],
            ..settings()
        },
        true,
    );
    s.reset(&config(48_000.0));
    s.shutdown();
    assert!(!s.is_running());
    assert_eq!(fake.closes.get(), 1);
    assert_eq!(fake.audio.stops(), 1);
    assert!(!s.status().running);
    drop(s);
    assert_eq!(
        fake.audio.stops(),
        1,
        "drop after shutdown stops nothing twice"
    );
}

#[test]
fn dropping_a_running_session_stops_it() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    drop(s);
    assert_eq!(fake.audio.stops(), 1);
}

#[test]
fn song_transport_audition_and_recording_calls_are_accepted_live_and_idle() {
    use atomik_engine::{SongInput, SongTempo, TransportCommand, compile_song};
    let input = SongInput {
        harmony: Default::default(),
        tracks: vec![],
        tempo: vec![SongTempo {
            tick: atomik_core::Ticks(0),
            tempo: Tempo::new(120.0).unwrap(),
            ramp: false,
        }],
        meter: vec![atomik_core::MeterEvent::new(1, 4, 4)],
        audio: Vec::new(),
        assets: Vec::new(),
    };
    let fake = Fake::default();
    for enabled in [true, false] {
        let mut s = LiveSession::new(&fake, settings(), enabled);
        s.reset(&config(48_000.0));
        assert_eq!(s.is_running(), enabled);
        let song = compile_song(&input).unwrap();
        assert!(s.submit_song(&input, song).is_ok());
        assert!(s.transport(TransportCommand::Play).is_ok());
        // Nothing rendered yet: the default state.
        assert_eq!(s.transport_state(), TransportState::default());
        let target = (DeviceInstanceId(1), atomik_device_sdk::SocketId(0));
        let on = NoteKind::On {
            key: 60,
            velocity: 0.8,
        };
        assert!(s.live_note(target, on).is_ok());
        s.set_recording(&[DeviceInstanceId(1)]);
        let mut out = Vec::new();
        s.drain_recorded(&mut out);
        assert!(out.is_empty(), "audition is not recorded");
        s.set_recording(&[]);
    }
}

// ---- duplex input -------------------------------------------------

#[test]
fn the_documents_input_channels_are_requested_and_the_input_device_is_passed() {
    let fake = Fake::default();
    let named = AudioSettings {
        input_device: Some("Line In".into()),
        ..settings()
    };
    let mut s = LiveSession::new(&fake, named, true);
    s.reset(&config_in(48_000.0, 2));
    assert_eq!(fake.audio.starts()[0].input_channels, 2);
    assert_eq!(
        fake.audio.input_devices(),
        vec![Some("Line In".to_string())]
    );
    let st = s.status();
    assert_eq!((st.input_channels_wanted, st.input_channels_open), (2, 2));
    assert_eq!(st.input_device.as_deref(), Some("Line In"));
    assert_eq!(st.input_error, None);
    // Without a named device the backend picks (None).
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config_in(48_000.0, 1));
    assert_eq!(fake.audio.input_devices(), vec![None]);
}

#[test]
fn no_input_channels_means_an_output_only_stream_as_before() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert_eq!(fake.audio.starts()[0].input_channels, 0);
    let st = s.status();
    assert_eq!((st.input_channels_wanted, st.input_channels_open), (0, 0));
    assert_eq!(st.input_device, None);
}

#[test]
fn when_the_input_cannot_be_opened_the_session_falls_back_to_output_only() {
    let fake = Fake::default();
    fake.audio.fail_input(true);
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config_in(48_000.0, 2));
    assert!(s.is_running(), "never a failed start because of the input");
    assert_eq!(
        fake.audio.starts().len(),
        2,
        "first with, then without input"
    );
    assert_eq!(fake.audio.starts()[0].input_channels, 2);
    assert_eq!(fake.audio.starts()[1].input_channels, 0);
    let st = s.status();
    assert_eq!(st.error, None);
    assert!(st.running);
    let msg = st.input_error.clone().expect("a clear message");
    assert!(msg.contains("no input device"), "{msg}");
    assert_eq!((st.input_channels_wanted, st.input_channels_open), (2, 0));
    let text = crate::status::status_text(st);
    assert!(text.contains("input off"), "{text}");
    // A later reset tries the input again.
    fake.audio.fail_input(false);
    s.reset(&config_in(48_000.0, 2));
    assert_eq!(s.status().input_error, None);
    assert_eq!(s.status().input_channels_open, 2);
}

#[test]
fn fewer_host_input_channels_than_the_document_are_reported() {
    let fake = Fake::default();
    fake.audio.limit_host_inputs(Some(1));
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config_in(48_000.0, 2));
    let st = s.status();
    assert_eq!((st.input_channels_wanted, st.input_channels_open), (2, 1));
    assert_eq!(st.input_error, None);
    let text = crate::status::status_text(st);
    assert!(text.contains("in 1 of 2 ch"), "{text}");
}

#[test]
fn the_session_reports_the_host_channel_counts_of_the_running_stream() {
    // The Audio I/O lamps need (inputs, outputs) of the open stream.
    let fake = Fake::default();
    fake.audio.limit_host_inputs(Some(1));
    let mut s = LiveSession::new(&fake, settings(), true);
    assert_eq!(s.host_channels(), None, "no stream yet");
    s.reset(&config_in(48_000.0, 2));
    let outputs = fake.audio.starts()[0].output_channels;
    assert_ne!(outputs, 1, "the two counts differ, so a swap shows");
    assert_eq!(s.host_channels(), Some((1, outputs)));
}

#[test]
fn polling_reads_the_latencies_and_the_record_offset_adds_the_user_offset() {
    let fake = Fake::default();
    let mut s = LiveSession::new(
        &fake,
        AudioSettings {
            record_offset_frames: 100,
            ..settings()
        },
        true,
    );
    s.reset(&config_in(48_000.0, 2));
    fake.audio.set_telemetry(Telemetry {
        callbacks: 10,
        buffer_frames: 128,
        input_latency_frames: 200,
        output_latency_frames: 500,
        ..Telemetry::default()
    });
    let st = s.poll().clone();
    assert_eq!(
        (st.input_latency_frames, st.output_latency_frames),
        (200, 500)
    );
    assert_eq!(s.record_offset_frames(), 200 + 500 + 64 + 100);
    // Before any measurement only the engine FIFO and the user offset count.
    let mut idle = LiveSession::new(&fake, settings(), false);
    idle.reset(&config(48_000.0));
    assert_eq!(idle.record_offset_frames(), 64);
}

#[test]
fn the_record_tap_needs_a_running_engine() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), false);
    s.reset(&config_in(48_000.0, 2));
    let controls = Arc::new(TapControls::new(1));
    let both = [TapChannel::host(0, 0), TapChannel::host(1, 0)];
    assert!(
        s.start_audio_record(&both, 4_096, controls.clone())
            .is_none()
    );
    s.set_enabled(true);
    s.reset(&config_in(48_000.0, 2));
    let tap = s
        .start_audio_record(&both[1..], 4_096, controls)
        .expect("tap");
    assert_eq!(tap.channels(), 1);
    s.stop_audio_record();
    assert_eq!(s.stream_underruns(), 0);
    assert!(s.stream_primed());
}

#[test]
fn the_streaming_threshold_comes_from_the_settings_override() {
    let fake = Fake::default();
    let s = LiveSession::new(&fake, settings(), false);
    assert_eq!(s.stream_threshold_bytes(), None);
    let s = LiveSession::new(
        &fake,
        AudioSettings {
            stream_threshold_bytes: Some(1_000),
            ..settings()
        },
        false,
    );
    assert_eq!(s.stream_threshold_bytes(), Some(1_000));
}

fn quiet() -> Telemetry {
    Telemetry {
        callbacks: 10,
        buffer_frames: 480,
        ..Telemetry::default()
    }
}

#[test]
fn stalled_stream_is_reported() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert_eq!(s.status().stream_state, StreamState::Starting);
    let t0 = clock_now();
    fake.audio.set_telemetry(quiet());
    assert_eq!(s.poll_at(t0).stream_state, StreamState::Running);
    // The callbacks stop: a stall once max(4 periods, 250 ms) went by without one.
    let st = s.poll_at(t0 + Duration::from_millis(200)).clone();
    assert_eq!(st.stream_state, StreamState::Running);
    let st = s.poll_at(t0 + Duration::from_millis(400)).clone();
    assert_eq!(st.stream_state, StreamState::Stalled);
    assert!(st.stream_silent_ms >= 400, "{}", st.stream_silent_ms);
    assert!(
        crate::status::stream_alert(&st)
            .unwrap()
            .contains("AUDIO STOPPED")
    );
    // They come back by themselves: no longer stalled.
    fake.audio.set_telemetry(Telemetry {
        callbacks: 11,
        ..quiet()
    });
    assert_eq!(
        s.poll_at(t0 + Duration::from_millis(450)).stream_state,
        StreamState::Running
    );
}

#[test]
fn a_reset_restores_the_callbacks_of_a_stalled_stream() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    let t0 = clock_now();
    fake.audio.set_telemetry(quiet());
    s.poll_at(t0);
    assert!(
        s.poll_at(t0 + Duration::from_secs(1))
            .stream_state
            .is_dead()
    );
    s.reset(&config(48_000.0));
    assert_eq!(fake.audio.starts().len(), 2, "a new stream was opened");
    // The new stream's callbacks count from zero again and arrive.
    fake.audio.set_telemetry(Telemetry {
        callbacks: 3,
        ..quiet()
    });
    let t1 = clock_now();
    assert_eq!(s.poll_at(t1).stream_state, StreamState::Running);
    assert_eq!(s.status().stream_silent_ms, 0);
}

#[test]
fn a_lost_device_is_reported_at_once_and_stream_errors_are_shown() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    fake.audio.set_telemetry(Telemetry {
        stream_errors: 2,
        device_lost: true,
        ..quiet()
    });
    let st = s.poll_at(clock_now()).clone();
    assert_eq!(st.stream_state, StreamState::Lost);
    assert_eq!(st.stream_errors, 2);
    let text = crate::status::status_text(&st);
    assert!(
        text.contains("stream errors 2 \u{b7} AUDIO DEVICE LOST"),
        "{text}"
    );
}

#[test]
fn input_overruns_and_stalled_control_pushes_reach_the_overload_counters() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    fake.audio.set_telemetry(Telemetry {
        input_overruns: 7,
        ..quiet()
    });
    assert_eq!(s.poll().overload.input_overruns, 7);
    assert_eq!(s.poll().overload.control_stalled, 0);
    // Nothing renders in the test, so the control queue fills up and then refuses.
    let id = DeviceInstanceId(1);
    let mut refused = 0;
    for _ in 0..100_000 {
        if s.set_param(id, ParamId(0), 0.5) == Err(EngineError::QueueFull) {
            refused += 1;
            if refused == 3 {
                break;
            }
        }
    }
    assert_eq!(refused, 3, "the queue fills up");
    assert_eq!(s.poll().overload.control_stalled, 3);
    assert!(
        crate::status::status_text(s.status()).contains("control stalled 3"),
        "{}",
        crate::status::status_text(s.status())
    );
}

/// Renders one block of silence through the session's engine (the fake stream owns it).
fn render_block(fake: &Fake) {
    let (mut left, mut right) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    fake.audio
        .with_engine(|engine| engine.render(&[], &mut [&mut left, &mut right]))
        .expect("a running engine");
}

#[test]
fn a_value_the_full_queue_refused_is_sent_again_by_the_next_poll() {
    use atomik_testkit::fakes::{ParamLog, ParamLogEntry};
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    let id = DeviceInstanceId(1);
    let (device, log) = ParamLog::boxed(100_000);
    s.insert_device(id, device, &[]).unwrap();
    render_block(&fake);
    // Fill the queue with one knob's drag, then one more value of it and a value of another
    // parameter: both are refused.
    let mut pushed = 0u32;
    while s.set_param(id, ParamLog::A, 0.1) == Ok(()) {
        pushed += 1;
        assert!(pushed < 1_000_000, "the queue fills up");
    }
    assert_eq!(
        s.set_param(id, ParamLog::A, 0.7),
        Err(EngineError::QueueFull)
    );
    assert_eq!(
        s.set_param(id, ParamLog::A, 0.9),
        Err(EngineError::QueueFull)
    );
    assert_eq!(
        s.set_param(id, ParamLog::B, -3.0),
        Err(EngineError::QueueFull)
    );
    // The engine takes the queue empty; the next poll sends what was refused: the newest value
    // of each parameter, not the one in between.
    render_block(&fake);
    s.poll();
    render_block(&fake);
    let sets: Vec<_> = log
        .entries()
        .into_iter()
        .filter_map(|e| match e {
            ParamLogEntry::Set(p, v) => Some((p, v)),
            ParamLogEntry::Process(_) => None,
        })
        .collect();
    assert_eq!(
        sets.last(),
        Some(&(ParamLog::B, -3.0)),
        "{:?}",
        &sets[sets.len() - 3..]
    );
    let last_a = sets.iter().rev().find(|(p, _)| *p == ParamLog::A).unwrap();
    assert_eq!(last_a.1.to_bits(), 0.9f32.to_bits());
    assert!(
        !sets
            .iter()
            .any(|&(p, v)| p == ParamLog::A && v.to_bits() == 0.7f32.to_bits()),
        "a value that a newer one replaced is not sent"
    );
    // Nothing is left waiting: another poll sends nothing more.
    let before = log.entries().len();
    s.poll();
    render_block(&fake);
    let after = log.entries();
    assert!(
        after[before..]
            .iter()
            .all(|e| matches!(e, ParamLogEntry::Process(_))),
        "nothing is sent twice"
    );
}

#[test]
fn a_configuration_that_does_not_open_falls_back_to_the_last_working_one() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert_eq!(
        s.settings()
            .device_loss
            .last_good
            .as_ref()
            .unwrap()
            .output_device
            .as_deref(),
        Some("Virtual Cable Input")
    );
    let change = s.set_settings(AudioSettings {
        output_device: Some("Unplugged".into()),
        ..settings()
    });
    assert_eq!(change, SettingsChange::RestartAudio);
    fake.audio.break_device("Unplugged");
    s.reset(&config(48_000.0));
    assert!(s.is_running(), "the last working configuration opened");
    assert_eq!(
        s.settings().output_device.as_deref(),
        Some("Virtual Cable Input")
    );
    assert_eq!(s.status().device, "Virtual Cable Input");
    let notice = s.take_notice().expect("the fallback is announced");
    assert!(
        notice.contains("Unplugged") && notice.contains("Virtual Cable Input"),
        "{notice}"
    );
    assert_eq!(s.take_notice(), None, "once");
}

#[test]
fn without_a_working_configuration_to_fall_back_to_audio_stays_off_with_the_error() {
    let fake = Fake::default();
    fake.audio.break_device("Virtual Cable Input");
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert!(!s.is_running());
    assert!(
        s.status()
            .error
            .as_deref()
            .unwrap()
            .contains("Virtual Cable Input")
    );
    assert_eq!(s.take_notice(), None);
    assert_eq!(s.settings().device_loss.last_good, None);
}

#[test]
fn a_failing_fallback_keeps_the_settings_and_the_first_error() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    s.set_settings(AudioSettings {
        output_device: Some("Unplugged".into()),
        ..settings()
    });
    // Both fail (say the document's rate is not supported by either).
    fake.audio.break_device("*");
    s.reset(&config(48_000.0));
    assert!(!s.is_running());
    assert_eq!(s.settings().output_device.as_deref(), Some("Unplugged"));
    assert!(s.status().error.as_deref().unwrap().contains("Unplugged"));
    assert_eq!(s.take_notice(), None);
}

#[test]
fn the_dialogs_copy_never_replaces_the_remembered_configuration() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    let remembered = s.settings().device_loss.last_good.clone();
    assert!(remembered.is_some());
    s.set_settings(settings());
    assert_eq!(s.settings().device_loss.last_good, remembered);
}

// ---- MIDI Out devices -------------------------------------------------------------------

/// A MIDI Out device in the session's engine, and the handle to feed its tap as its audio
/// thread would.
fn midi_out_device() -> (Box<dyn Device>, Arc<atomik_device_sdk::MidiTap>) {
    let device = atomik_devices::midi_out::MidiOut::new();
    let tap = device.midi_tap().unwrap();
    (Box::new(device), tap)
}

fn tap_push(tap: &atomik_device_sdk::MidiTap, frame: u64, bytes: &[u8]) {
    assert!(tap.push(atomik_device_sdk::MidiEvent::new(frame, bytes).unwrap()));
}

/// Waits (up to two seconds) until `fake` has sent at least `n` messages.
fn wait_for_out(fake: &Fake, n: usize) -> Vec<(String, Vec<u8>)> {
    for _ in 0..400 {
        if fake.out_wire.lock().unwrap().len() >= n {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    fake.out_wire.lock().unwrap().clone()
}

#[test]
fn no_output_thread_runs_without_a_midi_out_device() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    s.insert_device(DeviceInstanceId(1), synth(), &[]).unwrap();
    s.set_midi_out_ports(&[]);
    assert!(s.running.as_ref().unwrap().midi_out.is_none());
}

#[test]
fn a_midi_out_devices_messages_go_to_its_port_and_the_fallback_port_serves_the_rest() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    let (a, tap_a) = midi_out_device();
    let (b, tap_b) = midi_out_device();
    s.insert_device(DeviceInstanceId(1), a, &[]).unwrap();
    s.insert_device(DeviceInstanceId(2), b, &[]).unwrap();
    assert!(s.running.as_ref().unwrap().midi_out.is_some());
    // The first device chose a port; the second has none and the settings name none either.
    s.set_midi_out_ports(&[
        (DeviceInstanceId(1), Some("Synth A".into())),
        (DeviceInstanceId(2), None),
    ]);
    tap_push(&tap_a, 0, &[0x90, 60, 100]);
    tap_push(&tap_b, 0, &[0x90, 61, 100]);
    let wire = wait_for_out(&fake, 1);
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(wire, vec![("Synth A".to_string(), vec![0x90, 60, 100])]);
    assert_eq!(
        fake.out_wire.lock().unwrap().len(),
        1,
        "no port, no message"
    );
    // The settings' fallback port takes the second device from now on, and a change of it moves
    // the device: its sounding note is released where it was.
    let mut next = settings();
    next.midi_out_port = Some("Fallback".into());
    assert_eq!(s.set_settings(next), SettingsChange::None);
    tap_push(&tap_b, 0, &[0x90, 62, 100]);
    let wire = wait_for_out(&fake, 2);
    assert_eq!(wire[1], ("Fallback".to_string(), vec![0x90, 62, 100]));
    let mut next = settings();
    next.midi_out_port = Some("Other".into());
    s.set_settings(next);
    tap_push(&tap_b, 0, &[0x90, 63, 100]);
    let wire = wait_for_out(&fake, 5);
    let on_fallback: Vec<_> = wire.iter().filter(|(p, _)| p == "Fallback").collect();
    assert_eq!(
        on_fallback.last().unwrap().1,
        vec![0xB0, 123, 0],
        "released on the port it left: {wire:?}"
    );
    assert_eq!(
        wire.last().unwrap(),
        &("Other".to_string(), vec![0x90, 63, 100])
    );
}

#[test]
fn removing_the_device_or_stopping_the_session_releases_what_it_started() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    let (a, tap_a) = midi_out_device();
    s.insert_device(DeviceInstanceId(1), a, &[]).unwrap();
    s.set_midi_out_ports(&[(DeviceInstanceId(1), Some("Synth".into()))]);
    tap_push(&tap_a, 0, &[0x92, 60, 100]);
    wait_for_out(&fake, 1);
    s.remove_device(DeviceInstanceId(1)).unwrap();
    let wire = wait_for_out(&fake, 3);
    assert_eq!(
        wire[1..],
        [
            ("Synth".to_string(), vec![0x82, 60, 0]),
            ("Synth".to_string(), vec![0xB2, 123, 0]),
        ],
        "a note off and All Notes Off on its channel"
    );
    // A second device, then the whole session goes away.
    let (b, tap_b) = midi_out_device();
    s.insert_device(DeviceInstanceId(3), b, &[]).unwrap();
    s.set_midi_out_ports(&[(DeviceInstanceId(3), Some("Synth".into()))]);
    tap_push(&tap_b, 0, &[0x90, 70, 100]);
    wait_for_out(&fake, 4);
    s.shutdown();
    let wire = fake.out_wire.lock().unwrap().clone();
    assert_eq!(
        wire[4..],
        [
            ("Synth".to_string(), vec![0x80, 70, 0]),
            ("Synth".to_string(), vec![0xB0, 123, 0]),
        ]
    );
}

// ---- MIDI clock -------------------------------------------------------------------------

fn sync_settings(f: impl FnOnce(&mut crate::settings::SyncSettings)) -> AudioSettings {
    let mut s = settings();
    f(&mut s.sync);
    s
}

fn clock_out_to(port: &str, offset_ms: i32) -> AudioSettings {
    sync_settings(|y| {
        y.clock_out = true;
        y.out_ports = vec![crate::settings::ClockOutPort {
            name: port.into(),
            offset_ms,
        }];
    })
}

#[test]
fn clock_out_runs_the_output_thread_without_a_midi_out_device_and_sends_the_engines_clock() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, clock_out_to("Drum Machine", 0), true);
    s.reset(&config(48_000.0));
    let r = s.running.as_ref().unwrap();
    assert!(r.midi_out.is_some(), "the thread runs for the clock alone");
    assert!(r.handle.sync_out().config().enabled);
    // The engine writes its clock into the tap; the thread sends it to the port.
    let tap = r.handle.sync_out().tap();
    tap_push(&tap, 0, &[0xFA]);
    tap_push(&tap, 0, &[0xF8]);
    let wire = wait_for_out(&fake, 2);
    assert_eq!(
        wire,
        vec![
            ("Drum Machine".to_string(), vec![0xFA]),
            ("Drum Machine".to_string(), vec![0xF8])
        ]
    );
    s.poll();
    assert!(s.status().sync.clock_out);
    assert!(crate::status::status_text(s.status()).contains("clock out"));
}

#[test]
fn clock_out_needs_a_port_and_follows_the_settings() {
    let fake = Fake::default();
    // On but with no port: nothing is generated and no thread runs.
    let mut s = LiveSession::new(&fake, sync_settings(|y| y.clock_out = true), true);
    s.reset(&config(48_000.0));
    let r = s.running.as_ref().unwrap();
    assert!(r.midi_out.is_none());
    assert!(!r.handle.sync_out().config().enabled);
    // A port makes it run; the options reach the engine.
    let mut next = clock_out_to("Synth", 3);
    next.sync.send_while_stopped = true;
    next.sync.send_position = false;
    next.sync.restart_on_locate = false;
    s.set_settings(next);
    let cfg = s.running.as_ref().unwrap().handle.sync_out().config();
    assert_eq!(
        (
            cfg.enabled,
            cfg.while_stopped,
            cfg.song_position,
            cfg.restart_on_locate
        ),
        (true, true, false, false)
    );
    assert!(s.running.as_ref().unwrap().midi_out.is_some());
}

#[test]
fn switching_clock_out_off_keeps_the_route_until_the_stop_is_out() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, clock_out_to("Synth", 0), true);
    s.reset(&config(48_000.0));
    assert!(s.running.as_ref().unwrap().sync_route);
    s.set_settings(settings());
    assert!(
        !s.running
            .as_ref()
            .unwrap()
            .handle
            .sync_out()
            .config()
            .enabled
    );
    // The route stays a moment for the Stop the engine sends, then leaves.
    assert!(s.running.as_ref().unwrap().sync_route);
    s.poll_at(clock_now());
    assert!(s.running.as_ref().unwrap().sync_route);
    s.poll_at(clock_now() + Duration::from_secs(1));
    assert!(!s.running.as_ref().unwrap().sync_route);
    // Switched on again before it left: the route stays.
    let mut s = LiveSession::new(&fake, clock_out_to("Synth", 0), true);
    s.reset(&config(48_000.0));
    s.set_settings(settings());
    s.set_settings(clock_out_to("Synth", 0));
    s.poll_at(clock_now() + Duration::from_secs(1));
    assert!(s.running.as_ref().unwrap().sync_route);
}

fn follow(port: &str) -> AudioSettings {
    sync_settings(|y| {
        y.follow_clock = true;
        y.clock_in_port = Some(port.into());
    })
}

#[test]
fn following_opens_the_clock_port_once_and_closing_it_ends_the_following() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, follow("Sequencer"), true);
    s.reset(&config(48_000.0));
    assert_eq!(*fake.clock_opened.borrow(), vec!["Sequencer".to_string()]);
    let cell = fake.clock_cell.borrow().clone().unwrap();
    assert!(cell.following());
    assert!(s.sync_status().following);
    // The same settings again do not reopen it.
    s.set_settings(follow("Sequencer"));
    assert_eq!(fake.clock_opened.borrow().len(), 1);
    // The tempo and the transport of the clock show in the status.
    cell.publish(&atomik_engine::ExternalState {
        tempo: Some(97.5),
        running: true,
        position: 0,
        locates: 1,
        clocks: 10,
        phase: Some(0),
    });
    s.poll();
    assert!(s.status().sync.receiving);
    let text = crate::status::status_text(s.status());
    assert!(
        text.contains("following clock 97.5 BPM (running)"),
        "{text}"
    );
    // No new clocks since the last poll: it says so.
    s.poll();
    assert!(!s.status().sync.receiving);
    assert!(crate::status::status_text(s.status()).contains("no clock, last 97.5 BPM"));
    // Off: the port closes and the engine takes its own transport back.
    s.set_settings(settings());
    assert_eq!(fake.clock_closes.load(Ordering::Relaxed), 1);
    assert!(!cell.following());
    assert!(!s.sync_status().following);
    // Another port replaces the first.
    s.set_settings(follow("Sequencer"));
    s.set_settings(follow("Other"));
    assert_eq!(fake.clock_closes.load(Ordering::Relaxed), 2);
    assert_eq!(fake.clock_opened.borrow().last().unwrap(), "Other");
}

#[test]
fn a_clock_port_that_will_not_open_is_reported_and_tried_again_on_rescan() {
    let fake = Fake::default();
    fake.clock_missing.set(true);
    let mut s = LiveSession::new(&fake, follow("Sequencer"), true);
    s.reset(&config(48_000.0));
    let status = s.sync_status();
    assert!(!status.following);
    assert!(
        status
            .clock_in_error
            .as_deref()
            .unwrap()
            .contains("Sequencer")
    );
    s.poll();
    assert!(crate::status::status_text(s.status()).contains("clock in error"));
    // The port comes back.
    fake.clock_missing.set(false);
    s.rescan_midi();
    assert!(s.sync_status().following);
    assert_eq!(s.sync_status().clock_in_error, None);
}

#[test]
fn a_restarted_engine_follows_again() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, follow("Sequencer"), true);
    s.reset(&config(48_000.0));
    s.reset(&config(44_100.0));
    assert_eq!(fake.clock_opened.borrow().len(), 2);
    assert!(fake.clock_cell.borrow().as_ref().unwrap().following());
    assert_eq!(fake.clock_closes.load(Ordering::Relaxed), 1);
}

/// A session that opened "Virtual Cable Input" and then was asked for "Headset", which is
/// unplugged: it fell back, and wants to go back to the headset.
fn fallen_back(fake: &Fake) -> LiveSession<&Fake> {
    let mut s = LiveSession::new(fake, settings(), true);
    s.reset(&config(48_000.0));
    s.set_settings(AudioSettings {
        output_device: Some("Headset".into()),
        ..settings()
    });
    fake.audio.break_device("Headset");
    s.reset(&config(48_000.0));
    assert!(s.is_running());
    assert_eq!(s.status().device, "Virtual Cable Input");
    assert!(s.take_notice().is_some());
    s
}

#[test]
fn the_chosen_device_is_taken_back_once_it_is_listed_again() {
    let fake = Fake::default();
    fake.audio.plug("Virtual Cable Input");
    let mut s = fallen_back(&fake);
    let t0 = clock_now();

    // Unplugged: nothing to do, however often it is looked at.
    s.poll_at(t0);
    s.poll_at(t0 + Duration::from_secs(4));
    assert!(!s.switch_back_pending());
    assert_eq!(
        s.settings().output_device.as_deref(),
        Some("Virtual Cable Input")
    );

    // Plugged in: the list is read every few seconds, not every poll.
    fake.audio.plug("USB Headset");
    fake.audio.start_works();
    s.poll_at(t0 + Duration::from_secs(5));
    assert!(!s.switch_back_pending(), "read again only after the period");
    s.poll_at(t0 + Duration::from_secs(8));
    assert!(s.switch_back_pending());
    assert_eq!(s.settings().output_device.as_deref(), Some("Headset"));
    let notice = s.take_notice().expect("the return is told");
    assert!(
        notice.contains("Headset") && notice.contains("back"),
        "{notice}"
    );

    assert!(s.take_switch_back());
    assert!(!s.take_switch_back(), "once");
    s.reset(&config(48_000.0));
    assert!(s.is_running());
    assert_eq!(s.status().device, "Headset");
    assert_eq!(s.take_notice(), None);
    // Nothing more to wait for.
    s.poll_at(t0 + Duration::from_secs(20));
    assert!(!s.switch_back_pending());
}

#[test]
fn a_return_that_does_not_open_is_tried_once() {
    let fake = Fake::default();
    let mut s = fallen_back(&fake);
    let t0 = clock_now();
    s.poll_at(t0);
    // It shows in the list but still cannot be opened.
    fake.audio.plug("Headset");
    s.poll_at(t0 + Duration::from_secs(4));
    assert!(s.take_switch_back());
    s.reset(&config(48_000.0));
    assert!(s.is_running(), "the fallback opened again");
    assert_eq!(s.status().device, "Virtual Cable Input");
    let notice = s.take_notice().expect("the failure is told");
    assert!(notice.contains("Headset"), "{notice}");
    for k in 1..6 {
        s.poll_at(t0 + Duration::from_secs(4 + 4 * k));
        assert!(!s.switch_back_pending(), "no endless retries");
    }
}

#[test]
fn a_device_that_never_left_the_list_is_not_switched_back_to() {
    let fake = Fake::default();
    fake.audio.plug("Headset");
    let mut s = fallen_back(&fake);
    let t0 = clock_now();
    for k in 0..4 {
        s.poll_at(t0 + Duration::from_secs(4 * k));
    }
    assert!(!s.switch_back_pending());
}

#[test]
fn picking_devices_in_the_dialog_forgets_the_one_to_go_back_to() {
    let fake = Fake::default();
    let mut s = fallen_back(&fake);
    let t0 = clock_now();
    s.poll_at(t0);
    s.set_settings(AudioSettings {
        output_device: Some("Speakers".into()),
        ..settings()
    });
    fake.audio.plug("Headset");
    s.poll_at(t0 + Duration::from_secs(10));
    assert!(!s.switch_back_pending());
    assert_eq!(s.settings().output_device.as_deref(), Some("Speakers"));
}

#[test]
fn the_default_devices_after_a_loss_go_back_to_the_chosen_ones_when_they_return() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert_eq!(s.fall_back_to_defaults(), SettingsChange::RestartAudio);
    s.reset(&config(48_000.0));
    assert_eq!(s.settings().output_device, None);
    let t0 = clock_now();
    s.poll_at(t0);
    fake.audio.plug("Virtual Cable Input");
    s.poll_at(t0 + Duration::from_secs(4));
    assert!(s.take_switch_back());
    s.reset(&config(48_000.0));
    assert_eq!(s.status().device, "Virtual Cable Input");
    assert_eq!(
        s.settings().output_device.as_deref(),
        Some("Virtual Cable Input")
    );
}

#[test]
fn defaults_chosen_by_the_user_have_nothing_to_go_back_to() {
    let fake = Fake::default();
    let mut s = LiveSession::new(
        &fake,
        AudioSettings {
            output_device: None,
            ..settings()
        },
        true,
    );
    s.reset(&config(48_000.0));
    s.fall_back_to_defaults();
    s.poll_at(clock_now());
    s.poll_at(clock_now() + Duration::from_secs(10));
    assert!(!s.switch_back_pending());
}

#[test]
fn low_power_makes_only_an_automatic_worker_count_conservative() {
    let automatic = AudioSettings {
        workers_auto: true,
        workers: 5,
        ..AudioSettings::default()
    };
    // What the machine gets is used as it is at Full and cut down at Low power.
    assert_eq!(requested_workers(&automatic, false, 7), 7);
    assert_eq!(requested_workers(&automatic, true, 7), 1);
    assert_eq!(requested_workers(&automatic, true, 0), 0);
    // A count fixed by hand is never changed by the power level.
    let fixed = AudioSettings {
        workers_auto: false,
        workers: 5,
        ..AudioSettings::default()
    };
    assert_eq!(requested_workers(&fixed, false, 7), 5);
    assert_eq!(requested_workers(&fixed, true, 7), 5);
}

#[test]
fn the_low_power_flag_is_for_the_next_engine_and_restarts_nothing() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    assert!(!s.low_power);
    s.set_low_power(true);
    assert!(s.low_power);
    assert!(fake.audio.starts().is_empty(), "no stream was started");
}

// ---- Scenarios the scripted fake backend makes possible (simulated time, no sleeping) ----

/// A session running at 48 kHz on the settings' device, 256 frames a buffer (a period of
/// 5.33 ms).
fn running(fake: &Fake) -> LiveSession<&Fake> {
    let mut s = LiveSession::new(fake, settings(), true);
    s.reset(&config(48_000.0));
    assert!(s.is_running());
    s
}

#[test]
fn a_rate_change_under_a_running_stream_loses_the_device_until_it_is_opened_at_the_new_rate() {
    let fake = Fake::default();
    let mut s = running(&fake);
    let t0 = clock_now();
    fake.audio.run_callbacks(4);
    assert_eq!(s.poll_at(t0).stream_state, StreamState::Running);

    // The user switches the device's format in the system settings.
    fake.audio.set_rate(44_100);
    let st = s.poll_at(t0 + Duration::from_millis(20)).clone();
    assert_eq!(st.stream_state, StreamState::Lost);
    assert_eq!(st.stream_errors, 1);

    // The document still wants 48 kHz: the device refuses it and the error says so.
    s.reset(&config(48_000.0));
    assert!(!s.is_running());
    let err = s.status().error.clone().expect("the refusal is shown");
    assert!(err.contains("48000"), "{err}");

    // A document at the device's rate opens again.
    s.reset(&config(44_100.0));
    assert!(s.is_running());
    assert_eq!(s.status().sample_rate, 44_100);
    assert_eq!(s.status().error, None);
    assert_eq!(fake.audio.start_calls().len(), 3);
    assert_eq!(fake.audio.stops(), 1, "only the first stream was ever open");
}

#[test]
fn a_buffer_size_change_under_a_running_stream_moves_the_midi_latency_without_a_restart() {
    let fake = Fake::default();
    let mut s = running(&fake);
    fake.audio.run_callbacks(1);
    assert_eq!(s.poll().buffer_frames, Some(256));
    assert_eq!(s.latency_frames(), 256);

    // The driver changes its period under the stream.
    fake.audio.set_buffer(64);
    fake.audio.run_callbacks(1);
    assert_eq!(s.poll().buffer_frames, Some(64));
    assert_eq!(s.latency_frames(), 64, "live notes follow the actual size");
    assert_eq!(fake.audio.start_calls().len(), 1, "no restart");
    assert!(s.is_running());
}

#[test]
fn an_unplugged_device_is_lost_during_playback_and_opens_again_once_it_is_plugged_back() {
    let fake = Fake::default();
    fake.audio.plug("Virtual Cable Input");
    let mut s = running(&fake);
    let t0 = clock_now();
    fake.audio.run_callbacks(3);
    assert_eq!(s.poll_at(t0).stream_state, StreamState::Running);

    fake.audio.unplug();
    fake.audio.run_callbacks(2);
    let st = s.poll_at(t0 + Duration::from_secs(1)).clone();
    assert_eq!(st.stream_state, StreamState::Lost);
    assert_eq!(st.stream_errors, 1);
    assert!(
        fake.audio.plugged().is_empty(),
        "the host no longer lists it"
    );

    // Reopening while it is gone fails and names the device.
    s.reset(&config(48_000.0));
    assert!(!s.is_running());
    let err = s.status().error.clone().expect("the loss is shown");
    assert!(err.contains("Virtual Cable Input"), "{err}");

    fake.audio.replug();
    assert_eq!(
        fake.audio.plugged(),
        vec!["Virtual Cable Input".to_string()]
    );
    s.reset(&config(48_000.0));
    assert!(s.is_running());
    fake.audio.run_callbacks(1);
    let st = s.poll_at(t0 + Duration::from_secs(2)).clone();
    assert_eq!(st.stream_state, StreamState::Running);
    assert_eq!(st.error, None);
}

#[test]
fn a_stall_longer_than_the_threshold_is_reported_and_a_shorter_one_is_not() {
    let fake = Fake::default();
    let mut s = running(&fake);
    let t0 = clock_now();
    fake.audio.run_callbacks(10);
    assert_eq!(s.poll_at(t0).stream_state, StreamState::Running);

    // The driver stops delivering buffers for 600 ms: more than max(4 periods, 250 ms).
    fake.audio.stall();
    fake.audio.advance(Duration::from_millis(600));
    let st = s.poll_at(t0 + Duration::from_millis(600)).clone();
    assert_eq!(st.stream_state, StreamState::Stalled);
    assert_eq!(st.stream_silent_ms, 600);

    // The callbacks come back, and so does the state.
    fake.audio.resume();
    fake.audio.run_callbacks(1);
    let at = t0 + Duration::from_millis(700);
    assert_eq!(s.poll_at(at).stream_state, StreamState::Running);

    // A hiccup of 100 ms is shorter than the threshold: not a stall.
    fake.audio.stall_for(Duration::from_millis(100));
    fake.audio.advance(Duration::from_millis(100));
    assert_eq!(
        s.poll_at(at + Duration::from_millis(100)).stream_state,
        StreamState::Running
    );
}

#[test]
fn callbacks_that_cost_more_than_their_period_count_as_xruns() {
    let fake = Fake::default();
    let mut s = running(&fake);

    // 1 ms of a 5.33 ms period: a light load and no xrun.
    fake.audio.callback_cost_ns(1_000_000);
    fake.audio.run_callbacks(20);
    let st = s.poll().clone();
    assert_eq!(st.xruns, 0);
    assert!((st.dsp_load - 0.1875).abs() < 0.01, "{}", st.dsp_load);

    // 8 ms does not fit: every callback is an overrun.
    fake.audio.callback_cost_ns(8_000_000);
    fake.audio.run_callbacks(5);
    let st = s.poll().clone();
    assert_eq!(st.xruns, 5);
    assert!(st.dsp_load > 1.0 && st.dsp_peak > 1.0, "{st:?}");

    // Back to light: the total stays, the load comes down.
    fake.audio.callback_cost_ns(1_000_000);
    fake.audio.run_callbacks(3);
    let st = s.poll().clone();
    assert_eq!(st.xruns, 5);
    assert!(st.dsp_load < 0.5, "{}", st.dsp_load);
}
