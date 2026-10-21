//! The live session (deliverables 3 and 4): engine, audio stream and MIDI input, behind the
//! [`EngineSink`] the [`crate::AppCore`] drives.
//!
//! It follows `atomik-play`'s lifecycle (engine with workers, live sender taken before
//! anything is queued, frame clock, stream, MIDI with one host buffer of latency, garbage
//! collected on the UI thread, engine dropped on the UI thread after the stream stops) but
//! restarts as often as the app needs: [`EngineSink::reset`] stops everything and starts a
//! fresh engine and stream, and the core then re-inserts the document's devices.
//!
//! When audio is off (disabled, or the device failed to open) the sink calls are accepted
//! and ignored: the next restart rebuilds the engine from the document anyway.

use crate::audio_record::record_offset_frames;
use crate::midi::{MidiConnection, MidiLink};
use crate::record::{RecordTap, RecordedNote};
use crate::settings::AudioSettings;
use crate::sink::EngineSink;
use crate::status::AudioStatus;
use crate::target::{NoteTarget, Target};
use atomik_audio_io::{HostInfo, IoError, RunningStream, StreamInfo, StreamSpec, Telemetry};
use atomik_core::{NoteKind, ParamId, Tempo};
use atomik_device_sdk::{Device, PlayerDataRef, SampleMapRef};
use atomik_engine::{
    DeviceInstanceId, Endpoint, Engine, EngineConfig, EngineError, EngineHandle, EngineOptions,
    FrameClock, MeterReading, Meters, RecordTap as EngineRecordTap, RoutingPlan, SongInput,
    SongSnapshot, Topology, TransportCommand, TransportState, WorkerConfig,
    new_engine_with_options,
};
use atomik_midi_io::MidiError;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// MIDI latency used until the host reports its buffer size, in frames.
pub const FALLBACK_LATENCY_FRAMES: u64 = 1024;

/// A running audio stream that drives the engine.
pub trait LiveStream {
    /// Telemetry since the previous call (loads reset on read; counters are totals).
    fn telemetry(&self) -> Telemetry;
    /// What was opened.
    fn info(&self) -> &StreamInfo;
    /// Stops the stream and hands the engine back (dropped on the caller's thread).
    fn stop(self) -> Engine;
}

/// An open MIDI input.
pub trait MidiPort {
    /// Full port name.
    fn port_name(&self) -> &str;
    /// Notes dropped because the live queue was full.
    fn dropped(&self) -> u64;
    /// Closes the port and hands the link (with the engine's live sender) back.
    fn close(self) -> MidiLink;
    /// Sends a note now on the link's live queue (audition; [`MidiLink::send_now`]).
    fn send_now(&mut self, target: Target, kind: NoteKind) -> Result<(), EngineError>;
}

/// The outside world: audio devices and MIDI ports.
pub trait AudioBackend {
    /// Stream type.
    type Stream: LiveStream;
    /// MIDI input type.
    type Midi: MidiPort;
    /// Lists audio hosts and devices.
    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError>;
    /// Lists MIDI input ports.
    fn list_midi(&self) -> Result<Vec<String>, MidiError>;
    /// Opens and starts a stream driving `engine` (dropped here on failure). With input
    /// channels, `input_device` names the capture device (`None` = the device named like the
    /// output; never the host default input).
    fn start(
        &self,
        spec: &StreamSpec,
        input_device: Option<&str>,
        engine: Engine,
        clock: Arc<FrameClock>,
    ) -> Result<Self::Stream, IoError>;
    /// Opens a MIDI input; on failure the link is handed back.
    fn open_midi(&self, name: &str, link: MidiLink) -> Result<Self::Midi, (MidiError, MidiLink)>;
}

/// The real devices (`atomik-audio-io`, midir).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemBackend;

impl LiveStream for RunningStream {
    fn telemetry(&self) -> Telemetry {
        RunningStream::telemetry(self)
    }
    fn info(&self) -> &StreamInfo {
        RunningStream::info(self)
    }
    fn stop(self) -> Engine {
        RunningStream::stop(self)
    }
}

impl MidiPort for MidiConnection {
    fn port_name(&self) -> &str {
        MidiConnection::port_name(self)
    }
    fn dropped(&self) -> u64 {
        MidiConnection::dropped(self)
    }
    fn close(self) -> MidiLink {
        MidiConnection::close(self)
    }
    fn send_now(&mut self, target: Target, kind: NoteKind) -> Result<(), EngineError> {
        MidiConnection::send_now(self, target, kind)
    }
}

impl AudioBackend for SystemBackend {
    type Stream = RunningStream;
    type Midi = MidiConnection;

    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        atomik_audio_io::list()
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        atomik_midi_io::list_inputs()
    }

    fn start(
        &self,
        spec: &StreamSpec,
        input_device: Option<&str>,
        engine: Engine,
        clock: Arc<FrameClock>,
    ) -> Result<RunningStream, IoError> {
        atomik_audio_io::start_with_input(spec, input_device, engine, clock)
    }

    fn open_midi(
        &self,
        name: &str,
        link: MidiLink,
    ) -> Result<MidiConnection, (MidiError, MidiLink)> {
        MidiConnection::open(name, link)
    }
}

/// No audio or MIDI at all: lists nothing and fails to start (headless tests, smoke runs).
#[derive(Debug, Clone, Copy, Default)]
pub struct NullBackend;

impl AudioBackend for NullBackend {
    type Stream = RunningStream;
    type Midi = MidiConnection;

    fn list_audio(&self) -> Result<Vec<HostInfo>, IoError> {
        Ok(Vec::new())
    }

    fn list_midi(&self) -> Result<Vec<String>, MidiError> {
        Ok(Vec::new())
    }

    fn start(
        &self,
        _spec: &StreamSpec,
        _input_device: Option<&str>,
        _engine: Engine,
        _clock: Arc<FrameClock>,
    ) -> Result<RunningStream, IoError> {
        Err(IoError::HostUnavailable("none (null backend)".into()))
    }

    fn open_midi(
        &self,
        name: &str,
        link: MidiLink,
    ) -> Result<MidiConnection, (MidiError, MidiLink)> {
        Err((MidiError::PortNotFound(name.to_string()), link))
    }
}

/// What a settings change requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsChange {
    /// Nothing changed.
    None,
    /// Only the MIDI input changed; it was reopened, the engine kept running.
    MidiReopened,
    /// Audio settings changed: restart the engine from the document
    /// ([`crate::AppCore::rebuild_engine`]).
    RestartAudio,
}

struct Running<B: AudioBackend> {
    handle: EngineHandle,
    /// The engine's lock-free meter bank.
    meters: Arc<Meters>,
    /// The host device's `(input, output)` channel counts.
    host_channels: (u16, u16),
    stream: B::Stream,
    midi: Option<B::Midi>,
    /// The live link while no MIDI port is open.
    idle_link: Option<MidiLink>,
}

/// Engine, stream and MIDI input for the app.
pub struct LiveSession<B: AudioBackend> {
    backend: B,
    settings: AudioSettings,
    enabled: bool,
    running: Option<Running<B>>,
    target: Arc<NoteTarget>,
    latency: Arc<AtomicU64>,
    tap: Arc<RecordTap>,
    status: AudioStatus,
}

impl<B: AudioBackend> std::fmt::Debug for LiveSession<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSession")
            .field("settings", &self.settings)
            .field("enabled", &self.enabled)
            .field("running", &self.running.is_some())
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl<B: AudioBackend> LiveSession<B> {
    /// A session that opens audio on the first [`EngineSink::reset`] (when `enabled`).
    pub fn new(backend: B, settings: AudioSettings, enabled: bool) -> Self {
        let status = AudioStatus {
            host: settings.host.label().to_string(),
            ..AudioStatus::default()
        };
        Self {
            backend,
            settings,
            enabled,
            running: None,
            target: Arc::new(NoteTarget::new()),
            latency: Arc::new(AtomicU64::new(FALLBACK_LATENCY_FRAMES)),
            tap: Arc::new(RecordTap::new()),
            status,
        }
    }

    /// The backend.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// The active settings.
    pub fn settings(&self) -> &AudioSettings {
        &self.settings
    }

    /// Whether a stream is running.
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Whether audio is enabled (the next reset opens a stream).
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Enables or disables audio; takes effect at the next reset.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// The last status (see [`Self::poll`]).
    pub fn status(&self) -> &AudioStatus {
        &self.status
    }

    /// True if the running engine holds device `id` (in queue order).
    pub fn contains_device(&self, id: DeviceInstanceId) -> bool {
        self.running
            .as_ref()
            .is_some_and(|r| r.handle.contains_device(id))
    }

    /// The MIDI latency in frames currently applied to live notes.
    pub fn latency_frames(&self) -> u64 {
        self.latency.load(Ordering::Relaxed)
    }

    /// Applies new settings. A MIDI-only change reopens the MIDI input at once; any other
    /// change asks for an audio restart.
    pub fn set_settings(&mut self, settings: AudioSettings) -> SettingsChange {
        let old = std::mem::replace(&mut self.settings, settings);
        let audio_changed = old.host != self.settings.host
            || old.output_device != self.settings.output_device
            || old.input_device != self.settings.input_device
            || old.buffer_frames != self.settings.buffer_frames
            || old.workers != self.settings.workers
            || old.worker_linger_spins != self.settings.worker_linger_spins;
        if audio_changed {
            self.status.host = self.settings.host.label().to_string();
            SettingsChange::RestartAudio
        } else if old.midi_input != self.settings.midi_input {
            self.reopen_midi();
            SettingsChange::MidiReopened
        } else {
            SettingsChange::None
        }
    }

    /// Reads telemetry into the status (call a few times per second): DSP load, xruns, the
    /// actual buffer size (which also becomes the MIDI latency).
    pub fn poll(&mut self) -> &AudioStatus {
        if let Some(r) = &self.running {
            let t = r.stream.telemetry();
            self.status.dsp_load = t.dsp_load_avg;
            self.status.dsp_peak = t.dsp_load_peak;
            self.status.xruns = t.xruns;
            self.status.input_latency_frames = t.input_latency_frames;
            self.status.output_latency_frames = t.output_latency_frames;
            self.status.stream_underruns = r.handle.stream_underruns();
            if t.buffer_frames > 0 {
                self.status.buffer_frames = Some(t.buffer_frames);
                self.latency
                    .store(u64::from(t.buffer_frames), Ordering::Relaxed);
            }
        }
        &self.status
    }

    /// Stops MIDI and the stream and drops the engine on this thread.
    pub fn shutdown(&mut self) {
        if let Some(r) = self.running.take() {
            let Running {
                mut handle,
                meters: _,
                host_channels: _,
                stream,
                midi,
                idle_link,
            } = r;
            // MIDI first (it feeds the engine), then the stream, which hands the engine back
            // so it is dropped here and not on the audio thread.
            drop(midi.map(MidiPort::close));
            drop(idle_link);
            let engine = stream.stop();
            drop(engine);
            handle.collect_garbage();
        }
        self.status = AudioStatus {
            host: self.settings.host.label().to_string(),
            error: self.status.error.take(),
            ..AudioStatus::default()
        };
    }

    /// Closes the MIDI port (if any) and opens the one in the settings, keeping the engine.
    fn reopen_midi(&mut self) {
        let Some(r) = &mut self.running else {
            return;
        };
        let link = match (r.midi.take(), r.idle_link.take()) {
            (Some(midi), _) => midi.close(),
            (None, Some(link)) => link,
            // The live sender is gone (cannot happen: it is always in one of the two).
            (None, None) => return,
        };
        self.status.midi_port = None;
        self.status.midi_error = None;
        match &self.settings.midi_input {
            None => r.idle_link = Some(link),
            Some(name) => match self.backend.open_midi(name, link) {
                Ok(midi) => {
                    self.status.midi_port = Some(midi.port_name().to_string());
                    r.midi = Some(midi);
                }
                Err((e, link)) => {
                    self.status.midi_error = Some(e.to_string());
                    r.idle_link = Some(link);
                }
            },
        }
    }
}

impl<B: AudioBackend> EngineSink for LiveSession<B> {
    fn reset(&mut self, config: &EngineConfig) {
        self.status.error = None;
        self.shutdown();
        if !self.enabled {
            return;
        }
        let options = EngineOptions {
            workers: WorkerConfig {
                threads: self.settings.workers,
                // Workers render on the audio deadline too: same RT priority as the callback.
                on_thread_start: Some(atomik_audio_io::promote_worker_thread),
            },
            worker_linger_spins: self.settings.worker_linger_spins,
        };
        // Document rates are whole Hz (the model accepts the standard rates only).
        let rate = config.sample_rate.hz().round() as u32;
        let input_device = self.settings.input_device.clone();
        // One attempt: a fresh engine (a failed start drops it) and the stream around it.
        let attempt = |backend: &B, inputs: u16| {
            let (engine, mut handle) = new_engine_with_options(*config, options);
            // Taken before anything is queued; `Some` exactly once per engine.
            let sender = handle.live_notes();
            let clock = Arc::new(FrameClock::new());
            let spec = self
                .settings
                .stream_spec(rate, config.output_channels, inputs);
            backend
                .start(&spec, input_device.as_deref(), engine, Arc::clone(&clock))
                .map(|stream| (stream, handle, sender, clock))
        };
        let wanted = config.input_channels;
        let mut input_error = None;
        let started = match attempt(&self.backend, wanted) {
            Ok(started) => Ok(started),
            // The input never fails a start: retry output-only. The engine keeps its input
            // sockets (they read silence), and the next reset tries the input again.
            Err(e) if wanted > 0 => {
                input_error = Some(e.to_string());
                attempt(&self.backend, 0)
            }
            Err(e) => Err(e),
        };
        let (stream, handle, sender, clock) = match started {
            Ok(started) => started,
            Err(e) => {
                self.status.error = Some(e.to_string());
                return;
            }
        };
        let info = stream.info();
        let accepted = info.requested_buffer_frames;
        self.latency.store(
            accepted.map_or(FALLBACK_LATENCY_FRAMES, u64::from),
            Ordering::Relaxed,
        );
        self.status = AudioStatus {
            host: self.settings.host.label().to_string(),
            device: info.output_device.clone(),
            sample_rate: info.sample_rate,
            buffer_frames: accepted,
            running: true,
            input_channels_wanted: wanted,
            input_channels_open: info.input_channels,
            input_device: info.input_device.clone(),
            input_error,
            ..AudioStatus::default()
        };
        let idle_link = sender.map(|sender| MidiLink {
            sender,
            clock,
            target: Arc::clone(&self.target),
            sample_rate: config.sample_rate.hz(),
            latency: Arc::clone(&self.latency),
            tap: Arc::clone(&self.tap),
        });
        self.running = Some(Running {
            meters: handle.meters(),
            host_channels: (info.input_channels, info.output_channels),
            handle,
            stream,
            midi: None,
            idle_link,
        });
        self.reopen_midi();
    }

    fn insert_device(
        &mut self,
        id: DeviceInstanceId,
        device: Box<dyn Device>,
        params: &[(ParamId, f32)],
    ) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => r.handle.insert_device(id, device, params),
            None => Ok(()),
        }
    }

    fn remove_device(&mut self, id: DeviceInstanceId) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => r.handle.remove_device(id),
            None => Ok(()),
        }
    }

    fn submit_routing(&mut self, plan: RoutingPlan) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => r.handle.submit_routing(plan),
            None => Ok(()),
        }
    }

    fn set_param(
        &mut self,
        id: DeviceInstanceId,
        param: ParamId,
        plain: f32,
    ) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => r.handle.set_param(id, param, plain),
            None => Ok(()),
        }
    }

    fn set_device_samples(
        &mut self,
        id: DeviceInstanceId,
        map: Option<SampleMapRef>,
    ) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => r.handle.set_device_samples(id, map),
            None => Ok(()),
        }
    }

    fn set_device_player_data(
        &mut self,
        id: DeviceInstanceId,
        data: Option<PlayerDataRef>,
    ) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => r.handle.set_device_player_data(id, data),
            None => Ok(()),
        }
    }

    fn set_tempo(&mut self, tempo: Tempo) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => r.handle.set_tempo(tempo),
            None => Ok(()),
        }
    }

    fn collect_garbage(&mut self) -> usize {
        self.running
            .as_mut()
            .map_or(0, |r| r.handle.collect_garbage())
    }

    fn note_target(&self) -> Option<Arc<NoteTarget>> {
        Some(Arc::clone(&self.target))
    }

    fn submit_song(&mut self, _input: &SongInput, song: SongSnapshot) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => r.handle.submit_song(song),
            None => Ok(()),
        }
    }

    fn transport(&mut self, command: TransportCommand) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => r.handle.transport(command),
            None => Ok(()),
        }
    }

    fn transport_state(&self) -> TransportState {
        self.running
            .as_ref()
            .map_or_else(TransportState::default, |r| r.handle.transport_state())
    }

    fn live_note(&mut self, target: Target, kind: NoteKind) -> Result<(), EngineError> {
        let Some(r) = &mut self.running else {
            return Ok(());
        };
        match (&mut r.midi, &mut r.idle_link) {
            (Some(midi), _) => midi.send_now(target, kind),
            (None, Some(link)) => link.send_now(target, kind),
            (None, None) => Ok(()),
        }
    }

    fn set_recording(&mut self, devices: &[DeviceInstanceId]) {
        self.tap.set_armed(devices);
    }

    fn drain_recorded(&mut self, out: &mut Vec<RecordedNote>) {
        self.tap.drain(out);
    }

    fn latency_epoch(&self) -> u64 {
        self.running
            .as_ref()
            .map_or(0, |r| r.handle.latency_epoch())
    }

    fn fill_latencies(&self, topology: &mut Topology) {
        if let Some(r) = &self.running {
            r.handle.fill_latencies(topology);
        }
    }

    fn read_meter(&self, endpoint: Endpoint) -> Option<MeterReading> {
        self.running.as_ref()?.meters.read(endpoint)
    }

    fn read_device_meter_values(
        &self,
        id: DeviceInstanceId,
        out: &mut [f32],
    ) -> Option<(usize, u64)> {
        self.running.as_ref()?.meters.device_values(id, out)
    }

    fn start_audio_record(
        &mut self,
        channels: &[usize],
        capacity_frames: usize,
    ) -> Option<EngineRecordTap> {
        self.running
            .as_mut()?
            .handle
            .record_tap(channels, capacity_frames)
            .ok()
    }

    fn stop_audio_record(&mut self) {
        if let Some(r) = &mut self.running {
            // A full control queue leaves the tap running; the next one replaces it.
            let _ = r.handle.stop_record_tap();
        }
    }

    fn record_offset_frames(&self) -> i64 {
        record_offset_frames(
            self.status.input_latency_frames,
            self.status.output_latency_frames,
            self.settings.record_offset_frames,
        )
    }

    fn stream_underruns(&self) -> u64 {
        self.running
            .as_ref()
            .map_or(0, |r| r.handle.stream_underruns())
    }

    fn stream_primed(&self) -> bool {
        self.running
            .as_ref()
            .is_none_or(|r| r.handle.stream_primed())
    }

    fn stream_threshold_bytes(&self) -> Option<u64> {
        self.settings.stream_threshold_bytes
    }

    fn host_channels(&self) -> Option<(u16, u16)> {
        self.running.as_ref().map(|r| r.host_channels)
    }

    fn has_engine(&self) -> bool {
        self.running.is_some()
    }

    fn preview_start(
        &mut self,
        stream: atomik_engine::PreviewStream,
        gain: f32,
    ) -> Result<(), EngineError> {
        // Without an engine the stream is dropped: its feed sees it abandoned.
        match &mut self.running {
            Some(r) => r.handle.preview_start(stream, gain),
            None => Ok(()),
        }
    }

    fn preview_stop(&mut self) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => r.handle.preview_stop(),
            None => Ok(()),
        }
    }

    fn preview_active(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|r| r.handle.preview_active())
    }

    fn engine_error(&self) -> Option<String> {
        self.status.error.clone()
    }
}

impl<B: AudioBackend> Drop for LiveSession<B> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
