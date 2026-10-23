//! `LiveSession` tests with a fake backend (no audio or MIDI hardware).

use super::*;
use crate::settings::HostChoice;
use atomik_audio_io::HostKind;
use atomik_core::SampleRate;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Default)]
struct Fake {
    fail_start: RefCell<Option<IoError>>,
    starts: RefCell<Vec<StreamSpec>>,
    stops: Rc<Cell<usize>>,
    ports: Vec<String>,
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
    fn stop(self) -> Engine {
        self.stops.set(self.stops.get() + 1);
        self.engine
    }
}

struct FakeMidi {
    name: String,
    link: MidiLink,
    closes: Rc<Cell<usize>>,
}

impl MidiPort for FakeMidi {
    fn port_name(&self) -> &str {
        &self.name
    }
    fn dropped(&self) -> u64 {
        0
    }
    fn close(self) -> MidiLink {
        self.closes.set(self.closes.get() + 1);
        self.link
    }
    fn send_now(&mut self, target: Target, kind: NoteKind) -> Result<(), EngineError> {
        self.link.send_now(target, kind)
    }
}

impl AudioBackend for &Fake {
    type Stream = FakeStream;
    type Midi = FakeMidi;

    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        Ok(vec![])
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        Ok(self.ports.clone())
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

    fn open_midi(&self, name: &str, link: MidiLink) -> Result<FakeMidi, (MidiError, MidiLink)> {
        match crate::midi::match_port(&self.ports, name) {
            Ok(i) => {
                self.opened.borrow_mut().push(self.ports[i].clone());
                self.targets.borrow_mut().push(Arc::clone(&link.target));
                Ok(FakeMidi {
                    name: self.ports[i].clone(),
                    link,
                    closes: Rc::clone(&self.closes),
                })
            }
            Err(e) => Err((e, link)),
        }
    }
}

fn settings() -> AudioSettings {
    AudioSettings {
        host: HostChoice::Wasapi,
        output_device: Some("Virtual Cable Input".into()),
        buffer_frames: 256,
        midi_input: None,
        workers: 0,
        worker_linger_spins: 0,
        metronome_gain: 0.5,
        input_device: None,
        record_offset_frames: 0,
        stream_threshold_bytes: None,
        plugins: atomik_plugin_host::PluginSettings::default(),
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
            midi_input: Some("Keyboard".into()),
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

#[test]
fn a_missing_port_is_reported_and_a_fix_reopens_midi_without_a_restart() {
    let fake = Fake {
        ports: vec!["Example Keyboard".into()],
        ..Fake::default()
    };
    let mut s = LiveSession::new(
        &fake,
        AudioSettings {
            midi_input: Some("Keystation".into()),
            ..settings()
        },
        true,
    );
    s.reset(&config(48_000.0));
    assert!(s.status().midi_error.is_some());
    assert_eq!(s.status().midi_port, None);
    let change = s.set_settings(AudioSettings {
        midi_input: Some("Example".into()),
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
            midi_input: Some("Keyboard".into()),
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
    assert!(s.start_audio_record(&[0, 1], 4_096).is_none());
    s.set_enabled(true);
    s.reset(&config_in(48_000.0, 2));
    let tap = s.start_audio_record(&[1], 4_096).expect("tap");
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
