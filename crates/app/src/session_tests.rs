//! `LiveSession` tests with a fake backend (no audio or MIDI hardware).

use super::*;
use crate::midi::PortInput;
use crate::settings::{HostChoice, MidiInputRole, MidiInputSetting};
use atomik_audio_io::HostKind;
use atomik_core::SampleRate;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

#[derive(Default)]
struct Fake {
    fail_start: RefCell<Option<IoError>>,
    starts: RefCell<Vec<StreamSpec>>,
    stops: Rc<Cell<usize>>,
    ports: Vec<String>,
    /// Ports that are unplugged right now (listed in `ports`, not offered or openable).
    hidden: RefCell<Vec<String>>,
    /// The open inputs, to send messages through.
    inputs: FakeInputs,
    opened: RefCell<Vec<String>>,
    closes: Rc<Cell<usize>>,
    telemetry: Rc<Cell<Telemetry>>,
    accepted_buffer: Option<u32>,
    targets: RefCell<Vec<Arc<NoteTarget>>>,
    /// The input device name of every start.
    input_devices: RefCell<Vec<Option<String>>>,
    /// Fail every start that asks for input channels.
    fail_input: Cell<bool>,
    /// The host opens at most this many input channels.
    host_inputs: Cell<Option<u16>>,
    /// Every start naming this output device fails (`"*"`: every start fails).
    broken_device: RefCell<Option<String>>,
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

struct FakeStream {
    info: StreamInfo,
    engine: Engine,
    telemetry: Rc<Cell<Telemetry>>,
    stops: Rc<Cell<usize>>,
}

impl LiveStream for FakeStream {
    fn telemetry(&self) -> Telemetry {
        self.telemetry.get()
    }
    fn info(&self) -> &StreamInfo {
        &self.info
    }
    fn stop(self) -> Result<Engine, IoError> {
        self.stops.set(self.stops.get() + 1);
        Ok(self.engine)
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
    /// The names a plugged-in port list shows: every port but the unplugged ones.
    fn visible(&self) -> Vec<String> {
        let hidden = self.hidden.borrow();
        self.ports
            .iter()
            .filter(|p| !hidden.contains(p))
            .cloned()
            .collect()
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
        Ok(vec![])
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        Ok(self.visible())
    }

    fn start(
        &self,
        spec: &StreamSpec,
        input_device: Option<&str>,
        engine: Engine,
        _clock: Arc<FrameClock>,
    ) -> Result<FakeStream, IoError> {
        if let Some(e) = self.fail_start.borrow_mut().take() {
            return Err(e);
        }
        let broken = self.broken_device.borrow();
        if let Some(name) = broken.as_deref()
            && (name == "*" || spec.device.as_deref() == Some(name))
        {
            let device = spec.device.as_deref().unwrap_or("the default device");
            return Err(IoError::UnsupportedConfig(format!(
                "{device} cannot be opened"
            )));
        }
        drop(broken);
        if spec.input_channels > 0 && self.fail_input.get() {
            self.starts.borrow_mut().push(spec.clone());
            self.input_devices
                .borrow_mut()
                .push(input_device.map(str::to_owned));
            return Err(IoError::DeviceNotFound("no input device".into()));
        }
        let opened = self
            .host_inputs
            .get()
            .map_or(spec.input_channels, |n| n.min(spec.input_channels));
        self.starts.borrow_mut().push(spec.clone());
        self.input_devices
            .borrow_mut()
            .push(input_device.map(str::to_owned));
        Ok(FakeStream {
            info: StreamInfo {
                host: spec.host,
                output_device: spec.device.clone().unwrap_or_else(|| "Speakers".into()),
                input_device: (opened > 0)
                    .then(|| input_device.unwrap_or("Default input").to_owned()),
                sample_rate: spec.sample_rate,
                output_channels: spec.output_channels,
                input_channels: opened,
                sample_format: "f32",
                requested_buffer_frames: self.accepted_buffer,
            },
            engine,
            telemetry: Rc::clone(&self.telemetry),
            stops: Rc::clone(&self.stops),
        })
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
        let i = crate::midi::match_port(&self.visible(), name)?;
        let port = self.visible()[i].clone();
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
        plugins: atomik_plugin_host::PluginSettings::default(),
        surfaces: Vec::new(),
        midi_out_port: None,
        sync: crate::settings::SyncSettings::default(),
        device_loss: crate::settings::DeviceLossSettings::default(),
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
        *fake.starts.borrow(),
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
    assert!(fake.starts.borrow().is_empty());
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
    *fake.fail_start.borrow_mut() = Some(IoError::DeviceNotFound("Virtual Cable Input".into()));
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
    assert_eq!(fake.stops.get(), 1);
    assert_eq!(fake.starts.borrow().len(), 2);
    assert!(
        !s.contains_device(DeviceInstanceId(1)),
        "a reset starts an empty engine"
    );
}

#[test]
fn the_midi_input_opens_with_the_shared_target() {
    let fake = Fake {
        ports: vec!["Example Keyboard".into()],
        ..Fake::default()
    };
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
        })
        .collect()
}

fn two_ports() -> Fake {
    Fake {
        ports: vec!["Example Keys".into(), "Example Pads".into()],
        ..Fake::default()
    }
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
    assert_eq!(fake.starts.borrow().len(), 1, "no audio restart");
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
    *fake.hidden.borrow_mut() = vec!["Example Pads".into()];
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
    fake.hidden.borrow_mut().clear();
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
    let fake = Fake {
        ports: vec!["Example Keyboard".into()],
        ..Fake::default()
    };
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
    assert_eq!(fake.starts.borrow().len(), 1, "no audio restart");
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
        fake.starts.borrow().len(),
        1,
        "the restart is the caller's call"
    );
    s.reset(&config(48_000.0));
    assert_eq!(fake.starts.borrow()[1].buffer_frames, 512);
}

#[test]
fn polling_reads_telemetry_and_sets_the_midi_latency() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert_eq!(s.latency_frames(), FALLBACK_LATENCY_FRAMES);
    assert_eq!(s.status().buffer_frames, None);
    fake.telemetry.set(Telemetry {
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
fn an_accepted_buffer_size_is_the_initial_latency() {
    let fake = Fake {
        accepted_buffer: Some(128),
        ..Fake::default()
    };
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert_eq!(s.latency_frames(), 128);
    assert_eq!(s.status().buffer_frames, Some(128));
}

#[test]
fn shutdown_closes_midi_and_stops_the_stream() {
    let fake = Fake {
        ports: vec!["Example Keyboard".into()],
        ..Fake::default()
    };
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
    assert_eq!(fake.stops.get(), 1);
    assert!(!s.status().running);
    drop(s);
    assert_eq!(
        fake.stops.get(),
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
    assert_eq!(fake.stops.get(), 1);
}

#[test]
fn song_transport_audition_and_recording_calls_are_accepted_live_and_idle() {
    use atomik_engine::{SongInput, SongTempo, TransportCommand, compile_song};
    let input = SongInput {
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
    assert_eq!(fake.starts.borrow()[0].input_channels, 2);
    assert_eq!(
        *fake.input_devices.borrow(),
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
    assert_eq!(*fake.input_devices.borrow(), vec![None]);
}

#[test]
fn no_input_channels_means_an_output_only_stream_as_before() {
    let fake = Fake::default();
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config(48_000.0));
    assert_eq!(fake.starts.borrow()[0].input_channels, 0);
    let st = s.status();
    assert_eq!((st.input_channels_wanted, st.input_channels_open), (0, 0));
    assert_eq!(st.input_device, None);
}

#[test]
fn when_the_input_cannot_be_opened_the_session_falls_back_to_output_only() {
    let fake = Fake::default();
    fake.fail_input.set(true);
    let mut s = LiveSession::new(&fake, settings(), true);
    s.reset(&config_in(48_000.0, 2));
    assert!(s.is_running(), "never a failed start because of the input");
    assert_eq!(
        fake.starts.borrow().len(),
        2,
        "first with, then without input"
    );
    assert_eq!(fake.starts.borrow()[0].input_channels, 2);
    assert_eq!(fake.starts.borrow()[1].input_channels, 0);
    let st = s.status();
    assert_eq!(st.error, None);
    assert!(st.running);
    let msg = st.input_error.clone().expect("a clear message");
    assert!(msg.contains("no input device"), "{msg}");
    assert_eq!((st.input_channels_wanted, st.input_channels_open), (2, 0));
    let text = crate::status::status_text(st);
    assert!(text.contains("input off"), "{text}");
    // A later reset tries the input again.
    fake.fail_input.set(false);
    s.reset(&config_in(48_000.0, 2));
    assert_eq!(s.status().input_error, None);
    assert_eq!(s.status().input_channels_open, 2);
}

#[test]
fn fewer_host_input_channels_than_the_document_are_reported() {
    let fake = Fake::default();
    fake.host_inputs.set(Some(1));
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
    fake.host_inputs.set(Some(1));
    let mut s = LiveSession::new(&fake, settings(), true);
    assert_eq!(s.host_channels(), None, "no stream yet");
    s.reset(&config_in(48_000.0, 2));
    let outputs = fake.starts.borrow()[0].output_channels;
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
    fake.telemetry.set(Telemetry {
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
    fake.telemetry.set(quiet());
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
    fake.telemetry.set(Telemetry {
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
    fake.telemetry.set(quiet());
    s.poll_at(t0);
    assert!(
        s.poll_at(t0 + Duration::from_secs(1))
            .stream_state
            .is_dead()
    );
    s.reset(&config(48_000.0));
    assert_eq!(fake.starts.borrow().len(), 2, "a new stream was opened");
    // The new stream's callbacks count from zero again and arrive.
    fake.telemetry.set(Telemetry {
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
    fake.telemetry.set(Telemetry {
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
    fake.telemetry.set(Telemetry {
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
    *fake.broken_device.borrow_mut() = Some("Unplugged".into());
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
    *fake.broken_device.borrow_mut() = Some("Virtual Cable Input".into());
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
    *fake.broken_device.borrow_mut() = Some("*".into());
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
