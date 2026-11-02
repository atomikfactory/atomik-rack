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
use crate::midi_control::ControlHub;
use crate::record::{RecordTap, RecordedNote};
use crate::settings::{AudioSettings, LastGood, MidiInputRole};
use crate::sink::EngineSink;
use crate::status::{AudioStatus, SyncStatus};
use crate::target::{NoteTarget, Target};
use atomik_audio_io::{
    HostInfo, IoError, Liveness, RunningStream, StreamInfo, StreamSpec, StreamState, Telemetry,
};
use atomik_core::{NoteKind, ParamId, Tempo};
use atomik_device_sdk::{Device, PlayerDataRef, SampleMapRef};
use atomik_engine::{
    DeviceInstanceId, Endpoint, Engine, EngineConfig, EngineError, EngineHandle, EngineOptions,
    ExternalClock, FrameClock, LiveNoteSender, MeterReading, Meters, RecordTap as EngineRecordTap,
    RoutingPlan, SongInput, SongSnapshot, SyncConfig, TapChannel, TapControls, Topology,
    TransportCommand, TransportState, WorkerConfig, new_engine_with_options,
};
use atomik_midi_io::MidiError;
use atomik_midi_io::out::{Opener, OutStats, OutputThread, SyncPort, Timeline};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The wall clock the liveness check runs on.
#[allow(clippy::disallowed_methods)] // Justification: stream liveness is UI-thread bookkeeping
// (how long the audio callbacks have been silent); it never reaches the engine or the samples.
pub(crate) fn clock_now() -> Instant {
    Instant::now()
}

/// What opening a stream gives: the stream, the engine's control handle and live-note sender, and
/// the frame clock the notes are placed on.
type Opened<B> = (
    <B as AudioBackend>::Stream,
    EngineHandle,
    Option<LiveNoteSender>,
    Arc<FrameClock>,
);

/// Counts a control push that found its queue full ([`EngineError::QueueFull`]).
fn counted<T>(stalls: &mut u64, result: Result<T, EngineError>) -> Result<T, EngineError> {
    if matches!(result, Err(EngineError::QueueFull)) {
        *stalls = stalls.saturating_add(1);
    }
    result
}

/// MIDI latency used until the host reports its buffer size, in frames.
pub const FALLBACK_LATENCY_FRAMES: u64 = 1024;

/// A running audio stream that drives the engine.
pub trait LiveStream {
    /// Telemetry since the previous call (loads reset on read; counters are totals).
    fn telemetry(&self) -> Telemetry;
    /// What was opened.
    fn info(&self) -> &StreamInfo;
    /// Stops the stream and hands the engine back (dropped on the caller's thread). An error
    /// means the backend never released the engine; the stream is gone either way.
    fn stop(self) -> Result<Engine, IoError>;
}

/// An open MIDI input.
pub trait MidiPort {
    /// Full port name.
    fn port_name(&self) -> &str;
    /// Notes dropped because the live queue was full.
    fn dropped(&self) -> u64;
    /// Closes the port.
    fn close(self);
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
    /// Lists MIDI output ports (a control surface's motors, LEDs and displays); none by default.
    fn list_midi_out(&self) -> Result<Vec<String>, MidiError> {
        Ok(Vec::new())
    }
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
    /// Opens a MIDI input that feeds the engine through `link` (shared with the other open
    /// ports).
    fn open_midi(&self, name: &str, link: MidiLink) -> Result<Self::Midi, MidiError>;
    /// How the MIDI output thread opens the ports of the MIDI Out devices. The default opens
    /// nothing (every port is reported as not found), so a backend that has no MIDI never
    /// touches a real port.
    fn midi_out_opener(&self) -> Opener {
        Arc::new(|name| Err(MidiError::PortNotFound(name.to_string())))
    }
    /// Opens the MIDI input that carries the clock to follow and feeds `cell` from it. The
    /// value keeps the port open: dropping it closes the port. The default opens nothing.
    fn open_clock_in(
        &self,
        name: &str,
        _cell: Arc<ExternalClock>,
    ) -> Result<Box<dyn std::any::Any + Send>, MidiError> {
        Err(MidiError::PortNotFound(name.to_string()))
    }
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
    fn stop(self) -> Result<Engine, IoError> {
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
    fn close(self) {
        MidiConnection::close(self);
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

    fn list_midi_out(&self) -> Result<Vec<String>, MidiError> {
        atomik_midi_io::ports::list_outputs()
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

    fn open_midi(&self, name: &str, link: MidiLink) -> Result<MidiConnection, MidiError> {
        MidiConnection::open(name, link)
    }

    fn midi_out_opener(&self) -> Opener {
        atomik_midi_io::out::system_opener()
    }

    fn open_clock_in(
        &self,
        name: &str,
        cell: Arc<ExternalClock>,
    ) -> Result<Box<dyn std::any::Any + Send>, MidiError> {
        let input = atomik_midi_io::clock::open_clock_in(name, cell)?;
        Ok(Box::new(input))
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

    fn open_midi(&self, name: &str, _link: MidiLink) -> Result<MidiConnection, MidiError> {
        Err(MidiError::PortNotFound(name.to_string()))
    }
}

/// What a settings change requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsChange {
    /// Nothing changed.
    None,
    /// Only the MIDI inputs changed; the ports were opened or closed, the engine kept running.
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
    /// The open MIDI input ports, in settings order.
    midi: Vec<OpenInput<B::Midi>>,
    /// The live link every port shares (also the audition and Piano Keys path); `None` when the
    /// engine gave no live sender.
    link: Option<MidiLink>,
    /// The stream's frame clock (the output thread times messages on it).
    clock: Arc<FrameClock>,
    sample_rate: f64,
    /// The output latency in frames, as the stream reports it.
    out_latency: Arc<AtomicU64>,
    /// The thread that sends the MIDI Out devices' messages and the clock: started with the first
    /// such device, or with the clock.
    midi_out: Option<OutputThread>,
    /// Whether the thread has the clock's route (it then polls every millisecond).
    sync_route: bool,
    /// The open MIDI clock input, while the transport follows it.
    clock_in: Option<ClockIn>,
}

/// The open clock input and the setting it was opened for.
struct ClockIn {
    /// The name in the settings.
    wanted: String,
    /// Keeps the port open.
    _port: Box<dyn std::any::Any + Send>,
}

/// How long the clock's route stays after clock out is switched off, so the Stop it sends gets
/// out.
const SYNC_RELEASE_AFTER: Duration = Duration::from_millis(500);

/// An open MIDI port and the setting it was opened for.
struct OpenInput<M> {
    /// The name in the settings (the port's full name is the port's).
    wanted: String,
    /// What the port was opened as.
    role: MidiInputRole,
    port: M,
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
    /// The note taps the take installed (Direct Record), by device.
    note_taps: Vec<(DeviceInstanceId, Arc<atomik_device_sdk::MidiTap>)>,
    hub: Arc<ControlHub>,
    /// The overflow counter of the audio recording tap that runs (or ran last).
    audio_tap_overflow: Option<Arc<AtomicU64>>,
    /// Frames the earlier taps of this stream dropped.
    tap_overflow_base: u64,
    status: AudioStatus,
    /// Watches the callback counter of the running stream.
    liveness: Option<Liveness>,
    /// Control pushes (parameters, routing, transport, ...) that found the engine's queue full.
    control_stalled: u64,
    /// A message for the user that the next [`Self::take_notice`] hands out.
    notice: Option<String>,
    /// The port each MIDI Out device chose (by engine device id); a device that has none uses
    /// the settings' `midi_out_port`.
    out_ports: BTreeMap<u32, Option<String>>,
    /// Why the clock input did not open, if it did not.
    clock_error: Option<String>,
    /// The clock count at the last poll, and whether it moved since.
    clock_seen: u64,
    clock_receiving: bool,
    /// When the clock's route leaves the output thread (after clock out was switched off).
    sync_release_at: Option<Instant>,
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
            note_taps: Vec::new(),
            hub: Arc::new(ControlHub::new()),
            audio_tap_overflow: None,
            tap_overflow_base: 0,
            status,
            liveness: None,
            control_stalled: 0,
            notice: None,
            out_ports: BTreeMap::new(),
            clock_error: None,
            clock_seen: 0,
            clock_receiving: false,
            sync_release_at: None,
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

    /// Applies new settings. A MIDI-only change opens and closes the MIDI ports at once; any other
    /// change asks for an audio restart.
    pub fn set_settings(&mut self, mut settings: AudioSettings) -> SettingsChange {
        // The last working configuration is the session's to track, not the dialog's.
        settings.device_loss.last_good = self.settings.device_loss.last_good.clone();
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
        } else if old.midi_inputs != self.settings.midi_inputs {
            self.sync_midi();
            self.refresh_midi_out();
            if old.sync != self.settings.sync {
                self.refresh_sync();
            }
            SettingsChange::MidiReopened
        } else if old.midi_out_port != self.settings.midi_out_port {
            self.refresh_midi_out();
            SettingsChange::None
        } else {
            if old.sync != self.settings.sync {
                self.refresh_sync();
            }
            SettingsChange::None
        }
    }

    /// Reads telemetry into the status (call a few times per second): DSP load, xruns, the
    /// actual buffer size (which also becomes the MIDI latency).
    pub fn poll(&mut self) -> &AudioStatus {
        self.poll_at(clock_now())
    }

    /// [`Self::poll`] at a given time: the stream is stalled when no callback arrived for
    /// `max(4 periods, 250 ms)` before `at` (see [`Liveness`]).
    pub fn poll_at(&mut self, at: Instant) -> &AudioStatus {
        if let Some(r) = &self.running {
            let t = r.stream.telemetry();
            self.status.stream_errors = t.stream_errors;
            self.status.overload.input_overruns = t.input_overruns;
            if let Some(live) = &mut self.liveness {
                let state = live.observe(at, &t, r.stream.info().sample_rate);
                self.status.stream_state = state;
                self.status.stream_silent_ms = match state {
                    StreamState::Stalled => {
                        u64::try_from(live.silent_for(at).as_millis()).unwrap_or(u64::MAX)
                    }
                    _ => 0,
                };
            }
            r.out_latency
                .store(u64::from(t.output_latency_frames), Ordering::Relaxed);
            self.status.dsp_load = t.dsp_load_avg;
            self.status.dsp_peak = t.dsp_load_peak;
            self.status.xruns = t.xruns;
            self.status.input_latency_frames = t.input_latency_frames;
            self.status.output_latency_frames = t.output_latency_frames;
            self.status.faulted = t.faulted || r.handle.fault().is_some();
            self.status.faulted_device = r.handle.fault().map(|id| id.0);
            self.status.stream_underruns = r.handle.stream_underruns();
            self.status.overload.dropped_events = r.handle.dropped_events();
            self.status.overload.tap_overflow_frames = self.tap_overflow_base
                + self
                    .audio_tap_overflow
                    .as_ref()
                    .map_or(0, |c| c.load(Ordering::Relaxed));
            if t.buffer_frames > 0 {
                self.status.buffer_frames = Some(t.buffer_frames);
                self.latency
                    .store(u64::from(t.buffer_frames), Ordering::Relaxed);
            }
        }
        self.poll_sync(at);
        // Pushes that found the engine's queue full, and control messages (MIDI to
        // parameters, surfaces) the inbox had no room for.
        self.status.overload.control_stalled =
            self.control_stalled.saturating_add(self.hub.dropped());
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
                link,
                midi_out,
                sync_route: _,
                clock_in,
                clock: _,
                sample_rate: _,
                out_latency: _,
            } = r;
            // The clock input first, so nothing more reaches the engine from it.
            drop(clock_in);
            // The MIDI Out thread: dropping it releases every note it started.
            drop(midi_out);
            // MIDI first (it feeds the engine), then the stream, which hands the engine back
            // so it is dropped here and not on the audio thread.
            for open in midi {
                open.port.close();
            }
            drop(link);
            let stop_error = match stream.stop() {
                Ok(engine) => {
                    drop(engine);
                    None
                }
                Err(e) => Some(e.to_string()),
            };
            handle.collect_garbage();
            if stop_error.is_some() {
                self.status.error = stop_error;
            }
        }
        self.audio_tap_overflow = None;
        self.tap_overflow_base = 0;
        self.liveness = None;
        self.sync_release_at = None;
        self.clock_error = None;
        self.clock_seen = 0;
        self.clock_receiving = false;
        self.status = AudioStatus {
            host: self.settings.host.label().to_string(),
            error: self.status.error.take(),
            ..AudioStatus::default()
        };
    }

    /// Opens an engine and a stream for `settings`: one attempt with the document's input
    /// channels, then output-only when the input fails (the second value is why). A failed start
    /// drops its engine.
    fn open_stream(
        &self,
        settings: &AudioSettings,
        config: &EngineConfig,
    ) -> Result<(Opened<B>, Option<String>), IoError> {
        // Never more workers than cores - 1: the audio thread keeps a core of its own.
        let requested = if settings.workers_auto {
            atomik_audio_io::auto_workers_here()
        } else {
            settings.workers
        };
        let threads = atomik_audio_io::cap_workers_here(requested);
        if settings.workers_auto {
            log::info!("render workers: {threads} (automatic)");
        } else if threads != settings.workers {
            log::info!(
                "render workers: {} requested, {threads} started (one core stays free)",
                settings.workers
            );
        }
        let options = EngineOptions {
            workers: WorkerConfig {
                threads,
                // Workers render on the audio deadline too: same RT priority as the callback.
                on_thread_start: Some(atomik_audio_io::promote_worker_thread),
            },
            worker_linger_spins: settings.worker_linger_spins,
        };
        // Document rates are whole Hz (the model accepts the standard rates only).
        let rate = config.sample_rate.hz().round() as u32;
        let input_device = settings.input_device.as_deref();
        // One attempt: a fresh engine and the stream around it.
        let attempt = |inputs: u16| {
            let (engine, mut handle) = new_engine_with_options(*config, options);
            // Taken before anything is queued; `Some` exactly once per engine.
            let sender = handle.live_notes();
            let clock = Arc::new(FrameClock::new());
            let spec = settings.stream_spec(rate, config.output_channels, inputs);
            self.backend
                .start(&spec, input_device, engine, Arc::clone(&clock))
                .map(|stream| (stream, handle, sender, clock))
        };
        let wanted = config.input_channels;
        match attempt(wanted) {
            Ok(started) => Ok((started, None)),
            // The input never fails a start: retry output-only. The engine keeps its input
            // sockets (they read silence), and the next reset tries the input again.
            Err(e) if wanted > 0 => attempt(0).map(|started| (started, Some(e.to_string()))),
            Err(e) => Err(e),
        }
    }

    /// After `failure` opening the current settings: opens the last configuration that worked
    /// instead, when there is one and it differs. On success the session's settings become that
    /// configuration and [`Self::take_notice`] says so; on failure nothing changes.
    fn fall_back_to_last_good(
        &mut self,
        config: &EngineConfig,
        failure: &IoError,
    ) -> Option<(Opened<B>, Option<String>)> {
        let good = self.settings.device_loss.last_good.clone()?;
        if good == LastGood::of(&self.settings) {
            return None;
        }
        let mut fallback = self.settings.clone();
        fallback.use_devices(&good);
        let opened = self.open_stream(&fallback, config).ok()?;
        let named = |d: &Option<String>| d.clone().unwrap_or_else(|| "the host default".into());
        self.notice = Some(format!(
            "Audio could not open {} ({failure}); using the last working configuration, {}",
            named(&self.settings.output_device),
            named(&good.output_device)
        ));
        self.settings = fallback;
        Some(opened)
    }

    /// The message about a fallback to the last working configuration, once.
    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }

    /// Brings the open MIDI ports in line with the settings, keeping the engine: a port that is
    /// no longer enabled (or changed what it is for) is closed, an enabled one that is not open
    /// is opened, and the ports that stay as they are are not touched (a held note keeps its
    /// port). A port that fails to open is reported in the status and retried by the next call
    /// (a controller that was plugged in since).
    fn sync_midi(&mut self) {
        let Some(r) = &mut self.running else {
            return;
        };
        let wanted: Vec<(String, MidiInputRole)> = self
            .settings
            .enabled_midi_inputs()
            .map(|i| (i.name.clone(), i.role))
            .collect();
        let mut keep = Vec::with_capacity(r.midi.len());
        for open in std::mem::take(&mut r.midi) {
            if wanted
                .iter()
                .any(|(name, role)| *name == open.wanted && *role == open.role)
            {
                keep.push(open);
            } else {
                open.port.close();
            }
        }
        let mut errors = Vec::new();
        let listed = self.backend.list_midi().ok();
        if let Some(link) = &r.link {
            for (name, role) in &wanted {
                if keep.iter().any(|o| o.wanted == *name) {
                    continue;
                }
                // Two settings that name one port (a full name and a substring of it) open it
                // once.
                let full = listed
                    .as_ref()
                    .and_then(|l| crate::midi::match_port(l, name).ok().map(|i| &l[i]));
                if full.is_some_and(|f| keep.iter().any(|o| o.port.port_name() == f)) {
                    continue;
                }
                let link = link.with_plays_notes(*role == MidiInputRole::Keyboard);
                match self.backend.open_midi(name, link) {
                    Ok(port) => keep.push(OpenInput {
                        wanted: name.clone(),
                        role: *role,
                        port,
                    }),
                    Err(e) => errors.push(e.to_string()),
                }
            }
        }
        // Settings order, so the status lists the ports the way the dialog does.
        keep.sort_by_key(|o| wanted.iter().position(|(name, _)| *name == o.wanted));
        r.midi = keep;
        self.status.midi_port = (!r.midi.is_empty()).then(|| {
            r.midi
                .iter()
                .map(|o| o.port.port_name())
                .collect::<Vec<_>>()
                .join(", ")
        });
        self.status.midi_error = (!errors.is_empty()).then(|| errors.join("; "));
    }

    /// Hands the taps of the MIDI Out devices in the engine, and where each one sends, to the
    /// output thread, starting the thread with the first device (or with the clock). A device
    /// without a port of its own sends to the settings' `midi_out_port`. The thread also gets
    /// where the engine's clock goes while clock out is on.
    fn refresh_midi_out(&mut self) {
        let Some(r) = &mut self.running else {
            return;
        };
        let sync = self.settings.sync.sends();
        let taps: Vec<(u32, _)> = r
            .meters
            .midi_taps()
            .into_iter()
            .map(|(id, tap)| (id.0, tap))
            .collect();
        if r.midi_out.is_none() {
            if taps.is_empty() && !sync {
                return;
            }
            let timeline = Timeline::new(
                Arc::clone(&r.clock),
                r.sample_rate,
                Arc::clone(&r.out_latency),
            );
            match OutputThread::spawn(self.backend.midi_out_opener(), timeline) {
                Ok(thread) => r.midi_out = Some(thread),
                Err(e) => {
                    log::warn!("the MIDI output thread did not start: {e}");
                    return;
                }
            }
        }
        let ports: BTreeMap<u32, Option<String>> = taps
            .iter()
            .map(|(id, _)| {
                let own = self.out_ports.get(id).cloned().flatten();
                (*id, own.or_else(|| self.settings.midi_out_port.clone()))
            })
            .collect();
        let Some(thread) = &r.midi_out else {
            return;
        };
        thread.set_routes(taps, ports);
        if sync {
            let ports = self
                .settings
                .sync
                .out_ports
                .iter()
                .map(|p| SyncPort {
                    name: p.name.clone(),
                    offset_ms: p.offset_ms,
                })
                .collect();
            thread.set_sync(Some(r.handle.sync_out().tap()), ports);
            r.sync_route = true;
            self.sync_release_at = None;
        } else if r.sync_route && self.sync_release_at.is_none() {
            // The engine says Stop to the receivers once; the route stays until it is out.
            self.sync_release_at = Some(clock_now() + SYNC_RELEASE_AFTER);
        }
    }

    /// Applies the sync settings: what the engine sends as clock, where it goes, and whether the
    /// transport follows a clock input (opening or closing the port).
    fn refresh_sync(&mut self) {
        let Some(r) = &mut self.running else {
            return;
        };
        let s = &self.settings.sync;
        r.handle.sync_out().configure(SyncConfig {
            enabled: s.sends(),
            while_stopped: s.send_while_stopped,
            song_position: s.send_position,
            restart_on_locate: s.restart_on_locate,
        });
        let want = s.follows().then(|| s.clock_in_port.clone()).flatten();
        let cell = r.handle.external_clock();
        match want {
            Some(name) if r.clock_in.as_ref().is_some_and(|c| c.wanted == name) => {}
            Some(name) => {
                r.clock_in = None;
                match self.backend.open_clock_in(&name, Arc::clone(&cell)) {
                    Ok(port) => {
                        r.clock_in = Some(ClockIn {
                            wanted: name,
                            _port: port,
                        });
                        self.clock_error = None;
                        cell.set_following(true);
                    }
                    Err(e) => {
                        log::warn!("the MIDI clock input did not open: {e}");
                        self.clock_error = Some(e.to_string());
                        cell.set_following(false);
                    }
                }
            }
            None => {
                r.clock_in = None;
                self.clock_error = None;
                cell.set_following(false);
            }
        }
        self.refresh_midi_out();
    }

    /// Housekeeping of the sync state at a poll: the clock route leaves the output thread once
    /// the Stop of a switched-off clock is out, and the status shows whether clocks arrive.
    fn poll_sync(&mut self, at: Instant) {
        if self.sync_release_at.is_some_and(|t| at >= t)
            && !self.settings.sync.sends()
            && let Some(r) = &mut self.running
        {
            if let Some(thread) = &r.midi_out {
                thread.set_sync(None, Vec::new());
            }
            r.sync_route = false;
            self.sync_release_at = None;
        }
        let Some(r) = &self.running else {
            self.status.sync = SyncStatus::default();
            return;
        };
        let cell = r.handle.external_clock();
        let state = cell.read();
        self.clock_receiving = state.clocks != self.clock_seen;
        self.clock_seen = state.clocks;
        self.status.sync = self.sync_status();
    }

    /// Opens the enabled MIDI ports that are not open: a controller plugged in after start-up
    /// (or that a failed open left out) comes up without restarting the audio. A clock input that
    /// failed to open is tried again too.
    pub fn rescan_midi(&mut self) {
        self.sync_midi();
        if self.clock_error.is_some() {
            self.refresh_sync();
        }
    }

    /// Whether every enabled MIDI port is open (nothing for [`Self::rescan_midi`] to do): the
    /// last sync left no port it could not open.
    pub fn midi_complete(&self) -> bool {
        self.running.is_none() || self.status.midi_error.is_none()
    }

    /// What MIDI clock is doing now (see [`EngineSink::sync_status`]).
    pub fn sync_status(&self) -> SyncStatus {
        let Some(r) = &self.running else {
            return SyncStatus::default();
        };
        let cell = r.handle.external_clock();
        let following = r.clock_in.is_some() && cell.following();
        let state = cell.read();
        SyncStatus {
            clock_out: r.handle.sync_out().config().enabled,
            clock_in_port: r.clock_in.as_ref().map(|c| c.wanted.clone()),
            clock_in_error: self.clock_error.clone(),
            following,
            external_tempo: state.tempo.filter(|_| following),
            external_running: following && state.running,
            receiving: self.clock_receiving,
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
        let wanted = config.input_channels;
        let (started, input_error) = match self.open_stream(&self.settings, config) {
            Ok(opened) => opened,
            Err(e) => match self.fall_back_to_last_good(config, &e) {
                Some(opened) => opened,
                None => {
                    self.status.error = Some(e.to_string());
                    return;
                }
            },
        };
        let (stream, handle, sender, clock) = started;
        // This configuration opened a stream: it is the one to fall back to. (An input that
        // failed to open stays out of it: the output is what worked.)
        let mut good = LastGood::of(&self.settings);
        if input_error.is_some() {
            good.input_device = self
                .settings
                .device_loss
                .last_good
                .as_ref()
                .and_then(|g| g.input_device.clone());
        }
        self.settings.device_loss.last_good = Some(good);
        self.liveness = Some(Liveness::new(clock_now()));
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
            stream_state: StreamState::Starting,
            ..AudioStatus::default()
        };
        let out_clock = Arc::clone(&clock);
        let link = sender.map(|sender| {
            MidiLink::new(
                sender,
                clock,
                Arc::clone(&self.target),
                config.sample_rate.hz(),
                Arc::clone(&self.latency),
                Arc::clone(&self.tap),
                Arc::clone(&self.hub),
            )
        });
        self.running = Some(Running {
            meters: handle.meters(),
            host_channels: (info.input_channels, info.output_channels),
            handle,
            stream,
            midi: Vec::new(),
            link,
            clock: out_clock,
            sample_rate: config.sample_rate.hz(),
            out_latency: Arc::new(AtomicU64::new(0)),
            midi_out: None,
            sync_route: false,
            clock_in: None,
        });
        self.sync_midi();
        self.refresh_sync();
    }

    fn insert_device(
        &mut self,
        id: DeviceInstanceId,
        device: Box<dyn Device>,
        params: &[(ParamId, f32)],
    ) -> Result<(), EngineError> {
        let result = match &mut self.running {
            Some(r) => counted(
                &mut self.control_stalled,
                r.handle.insert_device(id, device, params),
            ),
            None => Ok(()),
        };
        // A MIDI Out device brings its tap with it.
        self.refresh_midi_out();
        result
    }

    fn remove_device(&mut self, id: DeviceInstanceId) -> Result<(), EngineError> {
        let result = match &mut self.running {
            Some(r) => counted(&mut self.control_stalled, r.handle.remove_device(id)),
            None => Ok(()),
        };
        // The device's notes are released and its tap forgotten.
        self.refresh_midi_out();
        result
    }

    fn set_midi_out_ports(&mut self, ports: &[(DeviceInstanceId, Option<String>)]) {
        self.out_ports = ports.iter().map(|(id, p)| (id.0, p.clone())).collect();
        self.refresh_midi_out();
    }

    fn sync_status(&self) -> SyncStatus {
        LiveSession::sync_status(self)
    }

    fn midi_out_stats(&self) -> OutStats {
        self.running
            .as_ref()
            .and_then(|r| r.midi_out.as_ref())
            .map(OutputThread::stats)
            .unwrap_or_default()
    }

    fn submit_routing(&mut self, plan: RoutingPlan) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => counted(&mut self.control_stalled, r.handle.submit_routing(plan)),
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
            Some(r) => counted(
                &mut self.control_stalled,
                r.handle.set_param(id, param, plain),
            ),
            None => Ok(()),
        }
    }

    fn set_device_samples(
        &mut self,
        id: DeviceInstanceId,
        map: Option<SampleMapRef>,
    ) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => counted(
                &mut self.control_stalled,
                r.handle.set_device_samples(id, map),
            ),
            None => Ok(()),
        }
    }

    fn set_device_player_data(
        &mut self,
        id: DeviceInstanceId,
        data: Option<PlayerDataRef>,
    ) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => counted(
                &mut self.control_stalled,
                r.handle.set_device_player_data(id, data),
            ),
            None => Ok(()),
        }
    }

    fn set_tempo(&mut self, tempo: Tempo) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => counted(&mut self.control_stalled, r.handle.set_tempo(tempo)),
            None => Ok(()),
        }
    }

    fn set_master_tune(&mut self, cents: f32, glide: bool) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => counted(
                &mut self.control_stalled,
                r.handle.set_master_tune(cents, glide),
            ),
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

    fn control_hub(&self) -> Option<Arc<ControlHub>> {
        Some(Arc::clone(&self.hub))
    }

    fn submit_song(&mut self, _input: &SongInput, song: SongSnapshot) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => counted(&mut self.control_stalled, r.handle.submit_song(song)),
            None => Ok(()),
        }
    }

    fn transport(&mut self, command: TransportCommand) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => counted(&mut self.control_stalled, r.handle.transport(command)),
            None => Ok(()),
        }
    }

    fn transport_state(&self) -> TransportState {
        self.running
            .as_ref()
            .map_or_else(TransportState::default, |r| r.handle.transport_state())
    }

    fn live_note(&mut self, target: Target, kind: NoteKind) -> Result<(), EngineError> {
        match self.running.as_ref().and_then(|r| r.link.as_ref()) {
            Some(link) => link.send_now(target, kind),
            None => Ok(()),
        }
    }

    fn play_note(&mut self, target: Target, kind: NoteKind) -> Result<(), EngineError> {
        match self.running.as_ref().and_then(|r| r.link.as_ref()) {
            Some(link) => link.play_now(target, kind),
            None => Ok(()),
        }
    }

    fn set_recording(&mut self, devices: &[DeviceInstanceId]) {
        self.tap.set_armed(devices);
    }

    fn set_note_taps(&mut self, devices: &[DeviceInstanceId]) {
        let Some(r) = &mut self.running else {
            self.note_taps.clear();
            return;
        };
        self.note_taps.retain(|(id, _)| {
            let keep = devices.contains(id);
            if !keep {
                // A full control queue leaves the tap on; it is replaced by the next take.
                let _ = r.handle.untap_notes(*id);
            }
            keep
        });
        for id in devices {
            if self.note_taps.iter().all(|(t, _)| t != id)
                && let Ok(tap) = r.handle.tap_notes(*id)
            {
                self.note_taps.push((*id, tap));
            }
        }
    }

    fn drain_recorded(&mut self, out: &mut Vec<RecordedNote>) {
        self.tap.drain(out);
        for (device, tap) in &self.note_taps {
            while let Some(e) = tap.pop() {
                if let Some(kind) = crate::record::note_of_midi(e.bytes()) {
                    out.push(RecordedNote {
                        device: *device,
                        kind,
                        frame: e.frame,
                    });
                }
            }
        }
    }

    fn hold_automation(&mut self, lanes: &[(DeviceInstanceId, ParamId)]) {
        if let Some(r) = &self.running {
            let _ = r.handle.hold_automation(lanes);
        }
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

    fn set_cpu_timing(&mut self, on: bool) {
        if on {
            atomik_engine::timing::install_clock(atomik_audio_io::monotonic_ns);
        }
        atomik_engine::timing::set_enabled(on);
    }

    fn read_device_cpu(&self, id: DeviceInstanceId) -> Option<atomik_engine::timing::CpuSample> {
        self.running.as_ref()?.meters.device_cpu(id)
    }

    fn read_device_scope(
        &self,
        id: DeviceInstanceId,
        left: &mut [f32],
        right: &mut [f32],
    ) -> Option<(usize, u64)> {
        let tap = self.running.as_ref()?.meters.scope_tap(id)?;
        Some(tap.read_latest(left, right))
    }

    fn start_audio_record(
        &mut self,
        channels: &[TapChannel],
        capacity_frames: usize,
        controls: Arc<TapControls>,
    ) -> Option<EngineRecordTap> {
        let tap = self
            .running
            .as_mut()?
            .handle
            .record_tap_gained(channels, capacity_frames, controls)
            .ok()?;
        // A new tap replaces the last: what the last one dropped stays counted.
        if let Some(last) = self.audio_tap_overflow.replace(tap.overflow_counter()) {
            self.tap_overflow_base += last.load(Ordering::Relaxed);
        }
        Some(tap)
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
            Some(r) => counted(
                &mut self.control_stalled,
                r.handle.preview_start(stream, gain),
            ),
            None => Ok(()),
        }
    }

    fn preview_stop(&mut self) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) => counted(&mut self.control_stalled, r.handle.preview_stop()),
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
