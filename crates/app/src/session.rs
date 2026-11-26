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
//!
//! # Opening and closing a stream
//!
//! A driver can take seconds to open or to release a stream, so the blocking calls
//! ([`AudioBackend::start`] and [`LiveStream::stop`]) run on a worker thread, one job at a time
//! per session. The session is in one of these states:
//!
//! - **off**: no stream and nothing in flight; the sink calls are ignored.
//! - **starting**: [`EngineSink::reset`] returned and a job is opening the stream (or waits for
//!   the job in flight). There is no engine to talk to yet ([`EngineSink::has_engine`] is
//!   false, as when audio is off), so the control calls are kept in order and sent when the
//!   stream is up; the newest value of a parameter and the newest routing and song replace
//!   the older ones. A new reset while starting replaces the request (last wins): the stream
//!   that arrives for the old one is stopped, never used.
//! - **running**: the stream is open and drives the engine.
//!
//! Streams stopped by a reset (or by [`LiveSession::shutdown`]) ride the same job, before the
//! start, so a device is never opened while the session itself still holds it; a job that
//! opens waits as well for the stops of the other sessions (see `STOPS`). The engine a
//! stopped stream hands back is dropped on the UI thread, when the job's answer is taken.
//! [`StartMode::Blocking`] (the default: headless runs and tests) waits for the job inside
//! `reset`, which is the behaviour of a session without a worker; the windowed app asks for
//! [`StartMode::Background`].

use crate::audio_record::record_offset_frames;
use crate::midi::{MidiConnection, MidiLink};
use crate::midi_control::ControlHub;
use crate::record::{RecordTap, RecordedNote};
use crate::settings::{AudioSettings, HostChoice, LastGood, MidiInputRole, MpeSetting};
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
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The wall clock the liveness check runs on.
#[allow(clippy::disallowed_methods)] // Justification: stream liveness is UI-thread bookkeeping
// (how long the audio callbacks have been silent); it never reaches the engine or the samples.
pub(crate) fn clock_now() -> Instant {
    Instant::now()
}

/// What opening a stream gives: the stream, the engine's control handle and live-note sender, and
/// the frame clock the notes are placed on.
type Opened<S> = (S, EngineHandle, Option<LiveNoteSender>, Arc<FrameClock>);

/// What [`AudioBackend::start`] does, detached from the backend so a worker thread can run it.
pub type Starter<S> =
    Box<dyn FnMut(&StreamSpec, Option<&str>, Engine, Arc<FrameClock>) -> Result<S, IoError> + Send>;

/// How long a quit waits for the worker (a stream that is opening, a stream that is stopping)
/// before it goes on without it: two of the audio backend's own five-second limits.
pub const QUIT_WAIT: Duration = Duration::from_secs(10);

/// How long an opening waits for the streams other sessions are stopping, so two songs that
/// hand the audio over never fight for the device.
const STOP_WAIT: Duration = Duration::from_secs(10);

/// How many streams of any session are being stopped now, and the signal that one finished.
static STOPS: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// One stream being stopped, counted in `STOPS` until it is dropped.
struct StopTicket;

impl StopTicket {
    fn take() -> Self {
        *STOPS.0.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Self
    }
}

impl Drop for StopTicket {
    fn drop(&mut self) {
        let mut stopping = STOPS.0.lock().unwrap_or_else(PoisonError::into_inner);
        *stopping = stopping.saturating_sub(1);
        STOPS.1.notify_all();
    }
}

/// Blocks (on the worker) until no session is stopping a stream, for at most [`STOP_WAIT`].
fn wait_for_stops() {
    let stopping = STOPS.0.lock().unwrap_or_else(PoisonError::into_inner);
    drop(
        STOPS
            .1
            .wait_timeout_while(stopping, STOP_WAIT, |n| *n > 0)
            .unwrap_or_else(PoisonError::into_inner),
    );
}

/// Whether [`EngineSink::reset`] waits for the stream to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartMode {
    /// `reset` returns when the stream is open (or failed): the session of headless runs and of
    /// the tests, where the next line reads the outcome.
    #[default]
    Blocking,
    /// `reset` returns at once; the stream opens on a worker and [`LiveSession::poll`] takes
    /// it over: the window stays alive while a driver is slow.
    Background,
}

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
pub trait LiveStream: Send + 'static {
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
    /// The call that opens and starts a stream driving an engine (dropped by the call on
    /// failure), as a value the worker thread owns while the session goes on without waiting
    /// for it. With input channels, the input device names the capture device (`None` = the
    /// device named like the output; never the host default input).
    fn starter(&self) -> Starter<Self::Stream>;
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

    fn starter(&self) -> Starter<RunningStream> {
        Box::new(atomik_audio_io::start_with_input)
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

    fn starter(&self) -> Starter<RunningStream> {
        Box::new(|_spec, _input_device, _engine, _clock| {
            Err(IoError::HostUnavailable("none (null backend)".into()))
        })
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

/// How often the device list is read while the session runs on a fallback, to see whether the
/// device the user chose is back.
const PREFERRED_POLL: Duration = Duration::from_secs(3);

/// The devices the user chose while a fallback runs in their place.
#[derive(Debug, Clone)]
struct Preferred {
    devices: LastGood,
    /// When the device list was last read for it.
    polled: Option<Instant>,
    /// The device was missing from the list at some read: only a device that went away can
    /// return (one that is listed but does not open is not tried again).
    gone: bool,
}

/// An open MIDI port and the setting it was opened for.
struct OpenInput<M> {
    /// The name in the settings (the port's full name is the port's).
    wanted: String,
    /// What the port was opened as.
    role: MidiInputRole,
    /// The MPE mode it was opened with.
    mpe: MpeSetting,
    port: M,
}

/// Whether a listed device is the one the settings name (exact or case-insensitive substring,
/// like the stream's device lookup).
fn device_named(listed: &str, wanted: &str) -> bool {
    listed == wanted || listed.to_lowercase().contains(&wanted.to_lowercase())
}

/// The render worker count an engine is asked for: `auto` (what the machine gets) while the
/// settings say automatic, made conservative at Low power; the count the user fixed otherwise,
/// whatever the power level.
fn requested_workers(settings: &AudioSettings, low_power: bool, auto: u16) -> u16 {
    if !settings.workers_auto {
        return settings.workers;
    }
    let level = if low_power {
        crate::performance::Level::Low
    } else {
        crate::performance::Level::Full
    };
    crate::performance::workers_for(level, auto)
}

/// What the worker is asked to open: the settings and the engine's shape, as they were when the
/// reset asked.
struct StartRequest {
    settings: AudioSettings,
    config: EngineConfig,
    low_power: bool,
    /// The reset is the one try at the chosen devices (see [`LiveSession::take_switch_back`]).
    switching_back: bool,
}

/// Which configuration a stream opened with.
enum Used {
    /// The one asked for.
    Chosen,
    /// The last working configuration, after `failure` opening the one asked for.
    Fallback { good: LastGood, failure: IoError },
}

/// An open stream, with the engine's side of it, as the worker hands it over.
struct Started<S> {
    stream: S,
    handle: EngineHandle,
    sender: Option<LiveNoteSender>,
    clock: Arc<FrameClock>,
    /// Why the input did not open (the stream runs output-only), if it did not.
    input_error: Option<String>,
    used: Used,
}

/// What a worker job gives back.
struct Done<S> {
    /// What each stopped stream gave back, in the order they were asked to stop.
    stopped: Vec<Result<Engine, IoError>>,
    /// The outcome of the open, when the job had one.
    started: Option<Result<Started<S>, IoError>>,
}

/// The job in flight: the streams it stops, then (maybe) the stream it opens.
struct Job<S> {
    rx: Receiver<Done<S>>,
    /// The handles of the engines whose streams the job stops, in the same order.
    retiring: Vec<EngineHandle>,
    /// What it opens; `None` for a job that only stops.
    request: Option<StartRequest>,
    /// The reset it belongs to: an answer for an older one is not used.
    generation: u64,
}

/// A stream that waits for the next job to stop it, with the handle of its engine.
type Retired<S> = (S, EngineHandle);

/// A control call made while there is no engine to take it yet.
enum Op {
    Insert {
        id: DeviceInstanceId,
        device: Box<dyn Device>,
        params: Vec<(ParamId, f32)>,
    },
    Remove(DeviceInstanceId),
    Routing(RoutingPlan),
    Samples(DeviceInstanceId, Option<SampleMapRef>),
    PlayerData(DeviceInstanceId, Option<PlayerDataRef>),
    Tempo(Tempo),
    MasterTune(f32, bool),
    Song(SongSnapshot),
    Transport(TransportCommand),
    PreviewStart(atomik_engine::PreviewStream, f32),
    PreviewStop,
    ReferenceStart(
        atomik_engine::PreviewStream,
        atomik_engine::ReferenceSettings,
    ),
    ReferenceSet(atomik_engine::ReferenceSettings),
    Lens(atomik_engine::LensSettings),
}

impl Op {
    /// Sends the call to the engine.
    fn apply(self, handle: &mut EngineHandle) -> Result<(), EngineError> {
        match self {
            Self::Insert { id, device, params } => handle.insert_device(id, device, &params),
            Self::Remove(id) => handle.remove_device(id),
            Self::Routing(plan) => handle.submit_routing(plan),
            Self::Samples(id, map) => handle.set_device_samples(id, map),
            Self::PlayerData(id, data) => handle.set_device_player_data(id, data),
            Self::Tempo(tempo) => handle.set_tempo(tempo),
            Self::MasterTune(cents, glide) => handle.set_master_tune(cents, glide),
            Self::Song(song) => handle.submit_song(song),
            Self::Transport(command) => handle.transport(command),
            Self::PreviewStart(stream, gain) => handle.preview_start(stream, gain),
            Self::PreviewStop => handle.preview_stop(),
            Self::ReferenceStart(stream, settings) => handle.reference_start(stream, settings),
            Self::ReferenceSet(settings) => handle.reference_set(settings),
            Self::Lens(settings) => handle.lens_set(settings),
        }
    }
}

/// What the worker thread does: stop the streams, then open the one asked for.
struct Work<S> {
    starter: Starter<S>,
    streams: Vec<S>,
    /// One per stream: dropped when that stream is stopped.
    tickets: Vec<StopTicket>,
    /// What to open, if anything: the settings, the engine's shape and the power level.
    start: Option<(AudioSettings, EngineConfig, bool)>,
}

impl<S: LiveStream> Work<S> {
    fn run(self) -> Done<S> {
        let Self {
            mut starter,
            streams,
            tickets,
            start,
        } = self;
        // The streams of this session go first: a device is never opened while it is held.
        let mut stopped = Vec::with_capacity(streams.len());
        for (stream, ticket) in streams.into_iter().zip(tickets) {
            stopped.push(stream.stop());
            drop(ticket);
        }
        let started = start.map(|(settings, config, low_power)| {
            // The streams other sessions are stopping may hold the same device.
            wait_for_stops();
            open(&mut starter, &settings, &config, low_power)
        });
        Done { stopped, started }
    }
}

/// Opens the configuration asked for, or the last one that worked when it does not open (and
/// differs). The error is the first one.
fn open<S>(
    starter: &mut Starter<S>,
    settings: &AudioSettings,
    config: &EngineConfig,
    low_power: bool,
) -> Result<Started<S>, IoError> {
    match open_stream(starter, settings, config, low_power) {
        Ok(opened) => Ok(Started::new(opened, Used::Chosen)),
        Err(failure) => {
            fall_back_to_last_good(starter, settings, config, low_power, failure.clone())
                .ok_or(failure)
        }
    }
}

impl<S> Started<S> {
    fn new(
        ((stream, handle, sender, clock), input_error): (Opened<S>, Option<String>),
        used: Used,
    ) -> Self {
        Self {
            stream,
            handle,
            sender,
            clock,
            input_error,
            used,
        }
    }
}

/// Opens an engine and a stream for `settings`: one attempt with the document's input channels,
/// then output-only when the input fails (the second value is why). A failed start drops its
/// engine.
fn open_stream<S>(
    starter: &mut Starter<S>,
    settings: &AudioSettings,
    config: &EngineConfig,
    low_power: bool,
) -> Result<(Opened<S>, Option<String>), IoError> {
    // Never more workers than cores - 1: the audio thread keeps a core of its own.
    let requested = requested_workers(settings, low_power, atomik_audio_io::auto_workers_here());
    let threads = atomik_audio_io::cap_workers_here(requested);
    if settings.workers_auto {
        let (physical, logical) = atomik_audio_io::core_counts_here();
        log::info!(
            "render workers: {threads} (automatic{}; {physical} physical cores, {logical} logical)",
            if low_power { ", low power" } else { "" }
        );
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
        // A live stream: the workers are used only while the audio thread needs help.
        realtime: true,
    };
    // Document rates are whole Hz (the model accepts the standard rates only).
    let rate = config.sample_rate.hz().round() as u32;
    let input_device = settings.input_device.as_deref();
    // One attempt: a fresh engine and the stream around it.
    let mut attempt = |inputs: u16| {
        let (engine, mut handle) = new_engine_with_options(*config, options);
        // Taken before anything is queued; `Some` exactly once per engine.
        let sender = handle.live_notes();
        let clock = Arc::new(FrameClock::new());
        let spec = settings.stream_spec(rate, config.output_channels, inputs);
        starter(&spec, input_device, engine, Arc::clone(&clock))
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

/// After `failure` opening `settings`: opens the last configuration that worked instead, when
/// there is one and it differs.
fn fall_back_to_last_good<S>(
    starter: &mut Starter<S>,
    settings: &AudioSettings,
    config: &EngineConfig,
    low_power: bool,
    failure: IoError,
) -> Option<Started<S>> {
    let good = settings.device_loss.last_good.clone()?;
    if good == LastGood::of(settings) {
        return None;
    }
    let mut fallback = settings.clone();
    fallback.use_devices(&good);
    let opened = open_stream(starter, &fallback, config, low_power).ok()?;
    Some(Started::new(opened, Used::Fallback { good, failure }))
}

/// How many control messages a replay leaves free in the engine's queue.
const REPLAY_RESERVE: usize = 2;

/// Engine, stream and MIDI input for the app.
pub struct LiveSession<B: AudioBackend> {
    backend: B,
    settings: AudioSettings,
    enabled: bool,
    running: Option<Running<B>>,
    mode: StartMode,
    /// How long [`Self::shutdown`] waits for the worker.
    quit_wait: Duration,
    /// The worker's job in flight, if any.
    job: Option<Job<B::Stream>>,
    /// The request the next job opens (the newest one: it replaced any older).
    next: Option<StartRequest>,
    /// The streams the next job stops first.
    retired: Vec<Retired<B::Stream>>,
    /// Counts the resets, so an answer for an older one is told from the current.
    generation: u64,
    /// The control calls made while the stream opens (or while the replay of them goes on), in
    /// order.
    queued: VecDeque<Op>,
    target: Arc<NoteTarget>,
    latency: Arc<AtomicU64>,
    tap: Arc<RecordTap>,
    /// The note taps the take installed (Direct Record), by device.
    note_taps: Vec<(DeviceInstanceId, Arc<atomik_device_sdk::MidiTap>)>,
    /// The note players that run a preview clock in the engine ([`EngineSink::set_previews`]).
    previewing: Vec<DeviceInstanceId>,
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
    /// Batches the engine ran with a parked retirement (the last polled value).
    stalled_batches: u64,
    /// The input trim count the debug line was last written for.
    input_trims_logged: u64,
    /// A tap stop the full control queue refused; retried on the next poll.
    stop_tap_pending: bool,
    /// The newest value of each parameter the full control queue refused, sent again by the
    /// next poll (a drag whose last value found the queue full still ends on it). Lives on the
    /// UI thread: nothing of it is on the audio path.
    held_params: BTreeMap<(DeviceInstanceId, ParamId), f32>,
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
    /// The devices the user chose, while a fallback (the last working configuration, or the
    /// host defaults after a loss) runs instead. Cleared when the user picks devices.
    preferred: Option<Preferred>,
    /// The chosen devices are back and the settings name them again: the app restarts the audio
    /// when it is safe ([`Self::take_switch_back`]).
    switch_back: bool,
    /// The restart that follows is the one try at the chosen devices: a failure of it does not
    /// make them the preferred ones again.
    switching_back: bool,
    /// Low power: an automatic render worker count is the conservative one (see
    /// [`crate::performance::workers_for`]). Read when an engine starts, so a change applies
    /// from the next start; a count the user fixed is never touched.
    low_power: bool,
    /// Whether the song of this session has the Input Focus: the enabled MIDI inputs are open
    /// only while it does (the settings keep the list either way).
    midi_held: bool,
}

impl<B: AudioBackend> std::fmt::Debug for LiveSession<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSession")
            .field("settings", &self.settings)
            .field("enabled", &self.enabled)
            .field("running", &self.running.is_some())
            .field("starting", &self.starting())
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
            mode: StartMode::Blocking,
            quit_wait: QUIT_WAIT,
            job: None,
            next: None,
            retired: Vec::new(),
            generation: 0,
            queued: VecDeque::new(),
            target: Arc::new(NoteTarget::new()),
            latency: Arc::new(AtomicU64::new(FALLBACK_LATENCY_FRAMES)),
            tap: Arc::new(RecordTap::new()),
            note_taps: Vec::new(),
            previewing: Vec::new(),
            hub: Arc::new(ControlHub::new()),
            audio_tap_overflow: None,
            tap_overflow_base: 0,
            status,
            liveness: None,
            control_stalled: 0,
            stalled_batches: 0,
            input_trims_logged: 0,
            stop_tap_pending: false,
            held_params: BTreeMap::new(),
            notice: None,
            out_ports: BTreeMap::new(),
            clock_error: None,
            clock_seen: 0,
            clock_receiving: false,
            sync_release_at: None,
            preferred: None,
            switch_back: false,
            switching_back: false,
            low_power: false,
            midi_held: true,
        }
    }

    /// Whether an automatic render worker count is the conservative one (Low power).
    pub fn low_power(&self) -> bool {
        self.low_power
    }

    /// Sets whether an automatic render worker count is the conservative one (Low power). It
    /// applies to the next engine this session starts; nothing restarts.
    pub fn set_low_power(&mut self, on: bool) {
        self.low_power = on;
    }

    /// Whether the enabled MIDI inputs are open (the song has the Input Focus).
    pub fn midi_held(&self) -> bool {
        self.midi_held
    }

    /// Holds the enabled MIDI inputs open (`true`, the default) or lets them go: a running
    /// stream opens or closes the ports at once and keeps the engine; one that is not running
    /// opens them when it starts. The list in the settings is not touched.
    pub fn set_midi_held(&mut self, held: bool) {
        if self.midi_held != held {
            self.midi_held = held;
            self.sync_midi();
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

    /// The loudest sample sent to the device and the loudest taken from it since the last call,
    /// as `(input, output)`; `None` while no engine runs. The output includes the preview voice
    /// (the audio setup's test tone), which the master meters do not.
    pub fn io_peaks(&self) -> Option<(f32, f32)> {
        let peaks = self.running.as_ref()?.handle.io_peaks();
        Some((peaks.take_input(), peaks.take_output()))
    }

    /// The note-ons the MIDI inputs have sent since the start and the key of the last one.
    pub fn note_activity(&self) -> (u64, Option<u8>) {
        self.hub.note_activity()
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
            // Devices that are picked now are the user's choice: nothing to go back to.
            self.preferred = None;
            self.switch_back = false;
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
        self.poll_job();
        self.flush_queued();
        if let Some(r) = &self.running {
            let t = r.stream.telemetry();
            self.status.stream_errors = t.stream_errors;
            self.status.overload.input_overruns = t.input_overruns;
            self.status.input_trim = crate::status::InputTrim {
                trims: t.input_trims,
                backlog_max_frames: t.input_backlog_max_frames,
                callback_min: t.callback_frames_min,
                callback_max: t.callback_frames_max,
                allowed_backlog_frames: t.input_backlog_allowed_frames,
                extra_backlog_frames: t.input_backlog_extra_frames,
            };
            self.status.workers = r.handle.worker_stats();
            if t.input_trims > self.input_trims_logged {
                self.input_trims_logged = t.input_trims;
                log::debug!("{}", crate::status::input_trim_line(&self.status));
            }
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
            self.stalled_batches = r.handle.stalled_batches();
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
        if self.stop_tap_pending {
            if let Some(r) = &mut self.running {
                self.stop_tap_pending = r.handle.stop_record_tap().is_err();
            } else {
                self.stop_tap_pending = false;
            }
        }
        self.resend_held_params();
        self.poll_sync(at);
        self.watch_preferred(at);
        // Pushes that found the engine's queue full, and control messages (MIDI to
        // parameters, surfaces) the inbox had no room for.
        self.status.overload.control_stalled = self
            .control_stalled
            .saturating_add(self.hub.dropped())
            .saturating_add(self.stalled_batches);
        &self.status
    }

    /// Sends again the parameter values the full control queue refused, newest per parameter,
    /// until the queue is full once more. The retry is not a new stall: the push that failed
    /// was counted when it failed.
    fn resend_held_params(&mut self) {
        // While the engine is still opening, the values wait for it (after the calls that
        // wait too: a device must exist before its parameters are set).
        if self.starting() || !self.queued.is_empty() {
            return;
        }
        let Some(r) = &mut self.running else {
            self.held_params.clear();
            return;
        };
        while let Some((&(id, param), &plain)) = self.held_params.first_key_value() {
            if r.handle.set_param(id, param, plain).is_err() {
                break;
            }
            self.held_params.remove(&(id, param));
        }
    }

    /// Whether the stream opens or is about to: a reset asked for it and no answer was taken yet.
    pub fn starting(&self) -> bool {
        self.next.is_some() || self.job.as_ref().is_some_and(|j| j.request.is_some())
    }

    /// Whether `reset` waits for the stream to open (see [`StartMode`]).
    pub fn start_mode(&self) -> StartMode {
        self.mode
    }

    /// Sets whether `reset` waits for the stream to open.
    pub fn set_start_mode(&mut self, mode: StartMode) {
        self.mode = mode;
    }

    /// Sets how long [`Self::shutdown`] waits for the worker (the default is [`QUIT_WAIT`]).
    pub fn set_quit_wait(&mut self, wait: Duration) {
        self.quit_wait = wait;
    }

    /// Stops MIDI and the stream and drops the engine on this thread. A stream that is still
    /// opening is stopped when it arrives. Waits for the worker, but for at most the quit wait:
    /// a driver that does not answer by then is left behind (its stream is never freed on the
    /// wrong thread).
    pub fn shutdown(&mut self) {
        self.generation += 1;
        self.next = None;
        self.teardown();
        self.settle(Some(clock_now() + self.quit_wait));
        self.status.starting = false;
    }

    /// Everything of [`Self::shutdown`] that does not wait: the calls kept for an engine are
    /// dropped, MIDI and the clock input close, and the stream (with the handle of its engine)
    /// waits for the next job to stop it.
    fn teardown(&mut self) {
        // The values and calls were meant for the engine that is going away.
        self.held_params.clear();
        self.queued.clear();
        // The next engine starts with no preview clock on any device.
        self.previewing.clear();
        if let Some(r) = self.running.take() {
            let Running {
                handle,
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
            self.retired.push((stream, handle));
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

    /// Starts the next job when none is in flight and there is something to stop or to open.
    fn pump(&mut self) {
        if self.job.is_some() || (self.retired.is_empty() && self.next.is_none()) {
            return;
        }
        let retired = std::mem::take(&mut self.retired);
        let request = self.next.take();
        let (streams, retiring): (Vec<_>, Vec<_>) = retired.into_iter().unzip();
        let tickets: Vec<StopTicket> = streams.iter().map(|_| StopTicket::take()).collect();
        let work = Work {
            starter: self.backend.starter(),
            streams,
            tickets,
            start: request
                .as_ref()
                .map(|r| (r.settings.clone(), r.config, r.low_power)),
        };
        let (tx, rx) = mpsc::channel();
        // A thread that cannot be made leaves the work here, to run on this thread: slow, as
        // before the worker existed, but never lost.
        let slot = Arc::new(Mutex::new(Some(work)));
        let theirs = Arc::clone(&slot);
        let sender = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("audio-open".into())
            .spawn(move || {
                let work = theirs.lock().unwrap_or_else(PoisonError::into_inner).take();
                if let Some(work) = work {
                    let _ = sender.send(work.run());
                }
            });
        if let Err(e) = spawned {
            log::warn!("the audio worker thread did not start ({e}): opening on this thread");
            let work = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(work) = work {
                let _ = tx.send(work.run());
            }
        }
        self.job = Some(Job {
            rx,
            retiring,
            request,
            generation: self.generation,
        });
    }

    /// Takes the worker's answer when it is there, and starts the next job when one waits.
    fn poll_job(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        match job.rx.try_recv() {
            Ok(done) => self.finish(Some(done)),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.finish(None),
        }
    }

    /// Waits for the worker until nothing is in flight or waits, or until `deadline` (`None`:
    /// as long as it takes: the backend bounds its own calls). A job still out at the deadline
    /// is left behind with the streams that waited for it.
    fn settle(&mut self, deadline: Option<Instant>) {
        loop {
            self.pump();
            let Some(job) = &self.job else {
                return;
            };
            let answer = match deadline {
                None => job.rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                Some(at) => job
                    .rx
                    .recv_timeout(at.saturating_duration_since(clock_now())),
            };
            match answer {
                Ok(done) => self.finish(Some(done)),
                Err(RecvTimeoutError::Disconnected) => self.finish(None),
                Err(RecvTimeoutError::Timeout) => {
                    log::warn!("the audio worker did not answer in time: leaving it behind");
                    self.job = None;
                    // Freeing these would wait on the driver that did not answer.
                    for stream in std::mem::take(&mut self.retired) {
                        std::mem::forget(stream);
                    }
                    return;
                }
            }
        }
    }

    /// Takes over the answer of the job in flight (`None`: the worker died), then starts the
    /// next job.
    fn finish(&mut self, done: Option<Done<B::Stream>>) {
        let Some(job) = self.job.take() else {
            return;
        };
        let (stopped, started) = match done {
            Some(done) => (done.stopped, done.started),
            None => (
                Vec::new(),
                job.request.as_ref().map(|_| {
                    Err(IoError::Backend(
                        "the audio worker stopped before the stream opened".into(),
                    ))
                }),
            ),
        };
        // The engines the stopped streams handed back go here, on the UI thread, then what they
        // retired.
        let mut stop_error = None;
        let mut handles = job.retiring.into_iter();
        for result in stopped {
            match result {
                Ok(engine) => drop(engine),
                Err(e) => stop_error = Some(e.to_string()),
            }
            if let Some(mut handle) = handles.next() {
                handle.collect_garbage();
            }
        }
        let current = job.generation == self.generation;
        match (job.request, started) {
            // A job that only stopped: a failed stop is shown while audio stays off.
            (None, _) => {
                if current && self.next.is_none() && stop_error.is_some() {
                    self.status.error = stop_error;
                }
            }
            (Some(_), None) => {}
            // An answer for a reset that was replaced (or by a shutdown): the stream is stopped,
            // never used.
            (Some(_), Some(Ok(started))) if !current => {
                let Started { stream, handle, .. } = started;
                self.retired.push((stream, handle));
            }
            (Some(_), Some(Err(_))) if !current => {}
            (Some(request), Some(Ok(started))) => self.attach(request, started),
            (Some(_), Some(Err(e))) => {
                self.status.error = Some(e.to_string());
            }
        }
        self.status.starting = self.starting();
        self.pump();
    }

    /// Makes the stream the session's: remembers the configuration that opened, builds the
    /// status and the MIDI link, sends the calls that waited and opens the MIDI ports.
    fn attach(&mut self, request: StartRequest, started: Started<B::Stream>) {
        let Started {
            stream,
            handle,
            sender,
            clock,
            input_error,
            used,
        } = started;
        if let Used::Fallback { good, failure } = used {
            // The device that did not open is the one to go back to when it can.
            if self.settings.output_device != good.output_device {
                self.prefer_unless(LastGood::of(&self.settings), request.switching_back);
            }
            let named = |d: &Option<String>| d.clone().unwrap_or_else(|| "the host default".into());
            self.notice = Some(format!(
                "Audio could not open {} ({failure}); using the last working configuration, {}",
                named(&self.settings.output_device),
                named(&good.output_device)
            ));
            self.settings.use_devices(&good);
        }
        let config = request.config;
        let wanted = config.input_channels;
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
        self.input_trims_logged = 0;
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
            buffer_requested: accepted,
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
        crate::startup::mark(crate::startup::AUDIO_RUNNING);
        self.flush_queued();
        self.sync_midi();
        self.refresh_sync();
    }

    /// Sends the calls that waited for an engine, in order, while the engine's queue has room;
    /// the rest goes on the next poll (and every call made meanwhile waits behind it). A call
    /// the engine refuses is told to the user once.
    fn flush_queued(&mut self) {
        let Some(r) = &mut self.running else {
            return;
        };
        while r.handle.control_room() >= REPLAY_RESERVE
            && let Some(op) = self.queued.pop_front()
        {
            if let Err(e) = counted(&mut self.control_stalled, op.apply(&mut r.handle)) {
                log::warn!("a control call made while the audio opened was refused: {e}");
                self.notice
                    .get_or_insert_with(|| format!("Audio engine: {e}"));
            }
        }
    }

    /// Keeps `op` for the engine that is about to exist (dropping it when none is: audio is
    /// off). The newest routing and the newest song replace the older ones.
    fn defer(&mut self, op: Op) -> Result<(), EngineError> {
        if !self.starting() && self.running.is_none() {
            return Ok(());
        }
        match &op {
            Op::Routing(_) => self.queued.retain(|o| !matches!(o, Op::Routing(_))),
            Op::Song(_) => self.queued.retain(|o| !matches!(o, Op::Song(_))),
            _ => {}
        }
        self.queued.push_back(op);
        self.flush_queued();
        Ok(())
    }

    /// The running engine, once the calls that waited have all been sent (before that, a call
    /// goes behind them).
    fn live(&mut self) -> Option<&mut Running<B>> {
        self.running.as_mut().filter(|_| self.queued.is_empty())
    }

    /// Switches to the host's default devices after the chosen ones were lost (the chosen devices
    /// are kept in memory to go back to when they return: see [`Self::take_switch_back`]).
    /// Returns what [`Self::set_settings`] returns for the change.
    pub fn fall_back_to_defaults(&mut self) -> SettingsChange {
        let chosen = LastGood::of(&self.settings);
        let mut settings = self.settings.clone();
        settings.output_device = None;
        settings.input_device = None;
        let change = self.set_settings(settings);
        if chosen.output_device.is_some() {
            self.prefer(chosen);
        }
        change
    }

    /// Remembers `devices` as the ones to go back to.
    fn prefer(&mut self, devices: LastGood) {
        self.prefer_unless(devices, self.switching_back);
    }

    /// [`Self::prefer`], except when the restart is the one try at the chosen devices.
    fn prefer_unless(&mut self, devices: LastGood, switching_back: bool) {
        // A restart that goes back to them is the one try: it must not make them wanted again.
        if !switching_back {
            self.preferred = Some(Preferred {
                devices,
                polled: None,
                gone: false,
            });
        }
    }

    /// While a fallback runs, reads the device list every few seconds; when the chosen output
    /// device is listed again the settings name it again, a notice says so and
    /// [`Self::take_switch_back`] asks for the restart. One try: the chosen devices are not
    /// wanted again after it, whatever it does.
    fn watch_preferred(&mut self, at: Instant) {
        if self.running.is_none() {
            return;
        }
        let Some(wanted) = &mut self.preferred else {
            return;
        };
        if wanted
            .polled
            .is_some_and(|last| at.saturating_duration_since(last) < PREFERRED_POLL)
        {
            return;
        }
        wanted.polled = Some(at);
        let devices = wanted.devices.clone();
        let Some(name) = devices.output_device.clone() else {
            self.preferred = None;
            return;
        };
        if LastGood::of(&self.settings) == devices {
            self.preferred = None;
            return;
        }
        let Ok(hosts) = self.backend.list_audio() else {
            return;
        };
        let back = hosts
            .iter()
            .filter(|h| devices.host == HostChoice::Default || h.kind == devices.host.kind())
            .flat_map(|h| &h.devices)
            .any(|d| d.max_outputs > 0 && device_named(&d.name, &name));
        if let Some(wanted) = &mut self.preferred {
            if !back {
                wanted.gone = true;
            }
            if !(back && wanted.gone) {
                return;
            }
        }
        log::info!("audio device {name} is back: switching to it");
        self.settings.use_devices(&devices);
        self.status.host = self.settings.host.label().to_string();
        self.preferred = None;
        self.switch_back = true;
        self.notice = Some(format!("Audio device {name} is back; switching to it."));
    }

    /// True once when the chosen devices are back and the settings name them again: the caller
    /// restarts the audio ([`EngineSink::reset`]) at a moment that suits it (not while playing or
    /// recording), and the restart is the one try.
    pub fn take_switch_back(&mut self) -> bool {
        let due = std::mem::take(&mut self.switch_back);
        if due {
            self.switching_back = true;
        }
        due
    }

    /// Whether a switch back to the chosen devices is waiting for its restart.
    pub fn switch_back_pending(&self) -> bool {
        self.switch_back
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
        let wanted: Vec<(String, MidiInputRole, MpeSetting)> = self
            .settings
            .enabled_midi_inputs()
            .filter(|_| self.midi_held)
            .map(|i| (i.name.clone(), i.role, i.mpe))
            .collect();
        let mut keep = Vec::with_capacity(r.midi.len());
        for open in std::mem::take(&mut r.midi) {
            if wanted.iter().any(|(name, role, mpe)| {
                *name == open.wanted && *role == open.role && *mpe == open.mpe
            }) {
                keep.push(open);
            } else {
                open.port.close();
            }
        }
        let mut errors = Vec::new();
        let listed = self.backend.list_midi().ok();
        if let Some(link) = &r.link {
            for (name, role, mpe) in &wanted {
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
                let link = link
                    .with_plays_notes(*role == MidiInputRole::Keyboard)
                    .with_mpe(mpe.zones());
                match self.backend.open_midi(name, link) {
                    Ok(port) => keep.push(OpenInput {
                        wanted: name.clone(),
                        role: *role,
                        mpe: *mpe,
                        port,
                    }),
                    Err(e) => errors.push(e.to_string()),
                }
            }
        }
        // Settings order, so the status lists the ports the way the dialog does.
        keep.sort_by_key(|o| wanted.iter().position(|(name, _, _)| *name == o.wanted));
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
        // A fault belongs to the engine that is being replaced.
        self.status.faulted = false;
        self.status.faulted_device = None;
        // Last wins: an open that has not started yet is replaced, one in flight is stopped when
        // it arrives.
        self.generation += 1;
        self.teardown();
        self.next = self.enabled.then(|| StartRequest {
            settings: self.settings.clone(),
            config: *config,
            low_power: self.low_power,
            switching_back: self.switching_back,
        });
        if self.enabled {
            // The one try at the chosen devices is made, whichever way it goes.
            self.switching_back = false;
        }
        self.status.starting = self.next.is_some();
        self.pump();
        if self.mode == StartMode::Blocking {
            self.settle(None);
        }
    }

    fn insert_device(
        &mut self,
        id: DeviceInstanceId,
        device: Box<dyn Device>,
        params: &[(ParamId, f32)],
    ) -> Result<(), EngineError> {
        let result = match &mut self.running {
            Some(r) if self.queued.is_empty() => counted(
                &mut self.control_stalled,
                r.handle.insert_device(id, device, params),
            ),
            _ => self.defer(Op::Insert {
                id,
                device,
                params: params.to_vec(),
            }),
        };
        // A MIDI Out device brings its tap with it.
        self.refresh_midi_out();
        result
    }

    fn remove_device(&mut self, id: DeviceInstanceId) -> Result<(), EngineError> {
        let result = match &mut self.running {
            Some(r) if self.queued.is_empty() => {
                counted(&mut self.control_stalled, r.handle.remove_device(id))
            }
            _ => self.defer(Op::Remove(id)),
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
            Some(r) if self.queued.is_empty() => {
                counted(&mut self.control_stalled, r.handle.submit_routing(plan))
            }
            _ => self.defer(Op::Routing(plan)),
        }
    }

    fn set_param(
        &mut self,
        id: DeviceInstanceId,
        param: ParamId,
        plain: f32,
    ) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) if self.queued.is_empty() => {
                let result = counted(
                    &mut self.control_stalled,
                    r.handle.set_param(id, param, plain),
                );
                match result {
                    // A refused value waits for the next poll (the newest one per parameter);
                    // one that went through makes an older waiting value of its own stale.
                    Err(EngineError::QueueFull) => {
                        self.held_params.insert((id, param), plain);
                    }
                    Ok(()) => {
                        self.held_params.remove(&(id, param));
                    }
                    Err(_) => {}
                }
                result
            }
            // The engine is opening (or still being sent what waited for it): the newest value
            // of the parameter goes after that, as a refused one does.
            _ => {
                if self.starting() || self.running.is_some() {
                    self.held_params.insert((id, param), plain);
                }
                Ok(())
            }
        }
    }

    fn set_device_samples(
        &mut self,
        id: DeviceInstanceId,
        map: Option<SampleMapRef>,
    ) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) if self.queued.is_empty() => counted(
                &mut self.control_stalled,
                r.handle.set_device_samples(id, map),
            ),
            _ => self.defer(Op::Samples(id, map)),
        }
    }

    fn set_device_player_data(
        &mut self,
        id: DeviceInstanceId,
        data: Option<PlayerDataRef>,
    ) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) if self.queued.is_empty() => counted(
                &mut self.control_stalled,
                r.handle.set_device_player_data(id, data),
            ),
            _ => self.defer(Op::PlayerData(id, data)),
        }
    }

    fn set_tempo(&mut self, tempo: Tempo) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) if self.queued.is_empty() => {
                counted(&mut self.control_stalled, r.handle.set_tempo(tempo))
            }
            _ => self.defer(Op::Tempo(tempo)),
        }
    }

    fn set_master_tune(&mut self, cents: f32, glide: bool) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) if self.queued.is_empty() => counted(
                &mut self.control_stalled,
                r.handle.set_master_tune(cents, glide),
            ),
            _ => self.defer(Op::MasterTune(cents, glide)),
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
            Some(r) if self.queued.is_empty() => {
                counted(&mut self.control_stalled, r.handle.submit_song(song))
            }
            _ => self.defer(Op::Song(song)),
        }
    }

    fn transport(&mut self, command: TransportCommand) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) if self.queued.is_empty() => {
                counted(&mut self.control_stalled, r.handle.transport(command))
            }
            _ => self.defer(Op::Transport(command)),
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
        let Some(r) = self.running.as_mut().filter(|_| self.queued.is_empty()) else {
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

    fn set_previews(&mut self, devices: &[DeviceInstanceId]) {
        let Some(r) = self.running.as_mut().filter(|_| self.queued.is_empty()) else {
            self.previewing.clear();
            return;
        };
        // The ones that go first, then the ones that start (a full control queue leaves a clock
        // as it was: the next call brings the sets in line again).
        self.previewing.retain(|id| {
            let keep = devices.contains(id);
            if !keep && r.handle.preview_player(*id, false).is_err() {
                return true;
            }
            keep
        });
        for id in devices {
            if !self.previewing.contains(id) && r.handle.preview_player(*id, true).is_ok() {
                self.previewing.push(*id);
            }
        }
    }

    fn launch_states(&self, out: &mut Vec<atomik_engine::LaunchState>) {
        match &self.running {
            Some(r) => r.handle.launch_states(out),
            None => out.clear(),
        }
    }

    fn next_launch_event(&mut self) -> Option<atomik_engine::LaunchEvent> {
        self.running.as_mut()?.handle.next_launch_event()
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

    fn set_device_scope_live(&mut self, id: DeviceInstanceId, live: bool) -> bool {
        let Some(tap) = self.running.as_ref().and_then(|r| r.meters.scope_tap(id)) else {
            return false;
        };
        tap.set_live(live);
        true
    }

    fn set_device_observed(&mut self, id: DeviceInstanceId, observed: bool) -> bool {
        let Some(tap) = self.running.as_ref().and_then(|r| r.meters.scope_tap(id)) else {
            return false;
        };
        tap.set_observed(observed);
        true
    }

    fn set_device_scope_point(&mut self, id: DeviceInstanceId, pre: bool) -> bool {
        let Some(tap) = self.running.as_ref().and_then(|r| r.meters.scope_tap(id)) else {
            return false;
        };
        tap.set_pre(pre);
        true
    }

    fn reserve_device_scope(&mut self, id: DeviceInstanceId) -> bool {
        let Some(tap) = self.running.as_ref().and_then(|r| r.meters.scope_tap(id)) else {
            return false;
        };
        tap.reserve();
        true
    }

    fn reserve_device_scope_large(&mut self, id: DeviceInstanceId) -> bool {
        let Some(tap) = self.running.as_ref().and_then(|r| r.meters.scope_tap(id)) else {
            return false;
        };
        tap.reserve_large();
        true
    }

    fn start_audio_record(
        &mut self,
        channels: &[TapChannel],
        capacity_frames: usize,
        controls: Arc<TapControls>,
    ) -> Option<EngineRecordTap> {
        let tap = self
            .live()?
            .handle
            .record_tap_gained(channels, capacity_frames, controls)
            .ok()?;
        // A new tap replaces the last (and a stop still waiting to be retried).
        self.stop_tap_pending = false;
        // A new tap replaces the last: what the last one dropped stays counted.
        if let Some(last) = self.audio_tap_overflow.replace(tap.overflow_counter()) {
            self.tap_overflow_base += last.load(Ordering::Relaxed);
        }
        Some(tap)
    }

    fn stop_audio_record(&mut self) {
        if let Some(r) = &mut self.running {
            // A full control queue refuses the stop: poll retries it on the next frame, so
            // the tap never keeps recording behind a finished take.
            self.stop_tap_pending = r.handle.stop_record_tap().is_err();
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

    fn stream_budget_bytes(&self) -> Option<u64> {
        self.settings.stream_budget_bytes
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
            Some(r) if self.queued.is_empty() => counted(
                &mut self.control_stalled,
                r.handle.preview_start(stream, gain),
            ),
            _ => self.defer(Op::PreviewStart(stream, gain)),
        }
    }

    fn preview_stop(&mut self) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) if self.queued.is_empty() => {
                counted(&mut self.control_stalled, r.handle.preview_stop())
            }
            _ => self.defer(Op::PreviewStop),
        }
    }

    fn preview_active(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|r| r.handle.preview_active())
    }

    fn reference_start(
        &mut self,
        stream: atomik_engine::PreviewStream,
        settings: atomik_engine::ReferenceSettings,
    ) -> Result<(), EngineError> {
        // Without an engine the stream is dropped: its feed sees it abandoned.
        match &mut self.running {
            Some(r) if self.queued.is_empty() => counted(
                &mut self.control_stalled,
                r.handle.reference_start(stream, settings),
            ),
            _ => self.defer(Op::ReferenceStart(stream, settings)),
        }
    }

    fn reference_set(
        &mut self,
        settings: atomik_engine::ReferenceSettings,
    ) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) if self.queued.is_empty() => {
                counted(&mut self.control_stalled, r.handle.reference_set(settings))
            }
            _ => self.defer(Op::ReferenceSet(settings)),
        }
    }

    fn reference_level(&self) -> Option<f32> {
        self.running
            .as_ref()
            .and_then(|r| r.handle.reference_shared().short_term_lufs())
    }

    fn lens_set(&mut self, settings: atomik_engine::LensSettings) -> Result<(), EngineError> {
        match &mut self.running {
            Some(r) if self.queued.is_empty() => {
                counted(&mut self.control_stalled, r.handle.lens_set(settings))
            }
            _ => self.defer(Op::Lens(settings)),
        }
    }

    fn engine_error(&self) -> Option<String> {
        self.status.error.clone()
    }

    fn audio_starting(&self) -> bool {
        self.starting()
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

#[cfg(test)]
#[path = "session_start_tests.rs"]
mod start_tests;
