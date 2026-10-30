//! cpal streams driving the engine.
//!
//! cpal has no duplex stream: with input channels, a separate input stream on the matching
//! capture device pushes converted samples into a wait-free ring that the output (render)
//! callback drains. Everything the callbacks touch is allocated here, before the streams start.

use crate::adapter::CallbackAdapter;
use crate::convert::{self, IntSample};
use crate::handoff::{ReturnOnDrop, build_with_fallback};
use crate::ring::{RingReader, RingWriter, sample_ring};
use crate::rt::{self, Promotion};
use crate::select::{
    ConfigCandidate, SampleKind, buffer_request, choose_config, default_input_index, host_kind,
    host_name, match_device,
};
use crate::telemetry::{self, LoadMeter, TelemetryState, XrunDetector, event_channel};
use crate::{DeviceInfo, HostInfo, HostKind, IoError, StreamSpec, Telemetry, XrunEvent, XrunKind};
use atomik_engine::{Engine, FrameClock};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Consumer, Producer};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Sample rates reported by [`list`].
const STANDARD_RATES: [u32; 5] = [44_100, 48_000, 88_200, 96_000, 192_000];

/// Smallest chunk the callback scratch is preallocated for; larger host buffers are rendered in
/// chunks of this many frames (or of the requested buffer size, if larger).
const MIN_CHUNK_FRAMES: usize = 1024;

/// Captured input kept in the ring beyond the current buffer, in buffers, before the oldest
/// samples are discarded (bounds the latency added by input/output clock drift).
const INPUT_SLACK_BUFFERS: usize = 2;

/// Input ring capacity, in chunks of `max_frames`.
const INPUT_RING_CHUNKS: usize = 8;

/// How long the backend may take to build a stream, and to release the engine on stop.
const BACKEND_TIMEOUT: Duration = Duration::from_secs(5);

/// What [`start`] actually opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    /// Host API (as requested; `Default` stays `Default`).
    pub host: HostKind,
    /// Output device name.
    pub output_device: String,
    /// Input device name, when input channels were requested.
    pub input_device: Option<String>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Output channels of the host stream (host channels beyond the engine's are silent).
    pub output_channels: u16,
    /// Input channels of the host stream (0 for output-only).
    pub input_channels: u16,
    /// Output sample format: `"f32"`, `"i32"` or `"i16"`.
    pub sample_format: &'static str,
    /// The fixed buffer size the host accepted, or `None` when it rejected the request and its
    /// default period is used. The size of each callback is reported by
    /// [`Telemetry::buffer_frames`].
    pub requested_buffer_frames: Option<u32>,
}

/// A running stream. Dropping it stops the stream; prefer [`RunningStream::stop`] to get the
/// engine back so it is dropped on the caller's (non-RT) thread.
pub struct RunningStream {
    // Field order matters for plain drops: the output stream (owning the engine) goes first.
    output: cpal::Stream,
    input: Option<cpal::Stream>,
    payload_rx: Receiver<OutputPayload>,
    telemetry: Arc<TelemetryState>,
    promotion: Arc<Promotion>,
    info: StreamInfo,
    /// Consumers of the xrun event channels (render callback, error callbacks). Locked by the
    /// control thread only; the callbacks own the producers and never touch this mutex.
    events: Mutex<Vec<Consumer<XrunEvent>>>,
}

impl std::fmt::Debug for RunningStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningStream")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl RunningStream {
    /// Telemetry since the previous call (averages and peaks reset on read; counters are totals).
    ///
    /// The first call after a failed real-time promotion of the audio thread also prints a
    /// one-time warning to stderr (from this, the control thread; the callback never prints).
    pub fn telemetry(&self) -> Telemetry {
        if self.promotion.take_warning() {
            eprintln!(
                "atomik-audio-io: warning: could not raise the audio thread to real-time priority"
            );
        }
        self.telemetry.snapshot()
    }

    /// Every [`XrunEvent`] queued since the previous call, oldest first (control thread; may
    /// allocate). Events the full channel could not hold are counted in
    /// [`Telemetry::events_dropped`].
    pub fn drain_events(&self) -> Vec<XrunEvent> {
        let mut consumers = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        let mut events = Vec::new();
        for rx in consumers.iter_mut() {
            while let Ok(event) = rx.pop() {
                events.push(event);
            }
        }
        events.sort_by_key(|e| e.at);
        events
    }

    /// Stops the stream and returns the engine.
    ///
    /// If the audio backend does not release the stream callback within 5 s (a driver that
    /// hangs on stop) this returns an error instead of aborting: the stream has already been
    /// dropped, so the engine stays with the backend's callback (it is leaked, never freed on
    /// the wrong thread) and the caller should build a new engine.
    pub fn stop(self) -> Result<Engine, IoError> {
        let Self {
            output,
            input,
            payload_rx,
            ..
        } = self;
        // Dropping the output stream drops its callback, which sends the payload back here.
        drop(output);
        drop(input);
        recover(&payload_rx, BACKEND_TIMEOUT).map(|payload| payload.engine)
    }

    /// What was opened.
    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    /// `None` before the first callback, then whether the one-time real-time promotion of the
    /// audio thread succeeded.
    pub fn realtime_priority(&self) -> Option<bool> {
        self.promotion.result()
    }
}

/// Waits up to `timeout` for the callback's payload to come back after its stream was dropped.
fn recover<P>(rx: &Receiver<P>, timeout: Duration) -> Result<P, IoError> {
    rx.recv_timeout(timeout).map_err(|_| {
        IoError::Backend(
            "the audio backend did not release the engine after the stream was dropped".to_string(),
        )
    })
}

/// Lists every available host and its devices.
pub fn list() -> Result<Vec<HostInfo>, IoError> {
    let mut hosts = Vec::new();
    for id in cpal::available_hosts() {
        let Some(kind) = host_kind(id.name()) else {
            continue; // a cpal host without a HostKind (e.g. PulseAudio)
        };
        let Ok(host) = cpal::host_from_id(id) else {
            continue;
        };
        let default_output = host.default_output_device();
        let devices = host
            .devices()
            .map_err(backend)?
            .map(|d| device_info(&d, default_output.as_ref()))
            .collect();
        hosts.push(HostInfo { kind, devices });
    }
    Ok(hosts)
}

fn device_info(device: &cpal::Device, default_output: Option<&cpal::Device>) -> DeviceInfo {
    let outputs: Vec<cpal::SupportedStreamConfigRange> = device
        .supported_output_configs()
        .map(Iterator::collect)
        .unwrap_or_default();
    let inputs: Vec<cpal::SupportedStreamConfigRange> = device
        .supported_input_configs()
        .map(Iterator::collect)
        .unwrap_or_default();
    let all = || outputs.iter().chain(&inputs);
    DeviceInfo {
        name: device.to_string(),
        max_inputs: inputs.iter().map(|c| c.channels()).max().unwrap_or(0),
        max_outputs: outputs.iter().map(|c| c.channels()).max().unwrap_or(0),
        sample_rates: STANDARD_RATES
            .into_iter()
            .filter(|r| all().any(|c| c.contains_rate(*r)))
            .collect(),
        buffer_range: all().find_map(|c| buffer_range(c.buffer_size())),
        is_default_output: default_output.is_some_and(|d| d == device),
    }
}

/// Opens and starts a stream driving `engine`. The callback publishes `clock` (engine host
/// frame + instant) at the start of every callback, then renders. With input channels the
/// capture device is the one named like the output device if the host has it as an input, else
/// no input (never the host default input) (see [`start_with_input`] to name it).
pub fn start(
    spec: &StreamSpec,
    engine: Engine,
    clock: Arc<FrameClock>,
) -> Result<RunningStream, IoError> {
    start_with_input(spec, None, engine, clock)
}

/// Like [`start`], with an explicit capture device for `spec.input_channels > 0` (exact name or
/// unique case-insensitive substring; `None` = the default described at [`start`]). It is
/// ignored when no input channels are requested.
pub fn start_with_input(
    spec: &StreamSpec,
    input_device: Option<&str>,
    engine: Engine,
    clock: Arc<FrameClock>,
) -> Result<RunningStream, IoError> {
    if spec.output_channels == 0 {
        return Err(IoError::UnsupportedConfig(
            "at least one output channel is required".into(),
        ));
    }
    let host = select_host(spec.host)?;
    let out_device = find_device(&host, spec.device.as_deref(), Direction::Output)?;
    let out_choice = choose(&out_device, Direction::Output, spec)?;
    let in_side = if spec.input_channels > 0 {
        let device = find_input_device(&host, input_device, &out_device.to_string())?;
        let choice = choose(&device, Direction::Input, spec)?;
        Some((device, choice))
    } else {
        None
    };
    let requested = buffer_request(spec.buffer_frames, out_choice.buffer);
    let max_frames = usize::try_from(requested)
        .unwrap_or(MIN_CHUNK_FRAMES)
        .max(MIN_CHUNK_FRAMES);
    let in_channels = in_side.as_ref().map_or(0, |(_, c)| c.channels);
    let (ring_writer, ring_reader) = match in_channels {
        0 => (None, None),
        ch => {
            let (w, r) = sample_ring(max_frames * usize::from(ch) * INPUT_RING_CHUNKS);
            (Some(w), Some(r))
        }
    };

    let telemetry = Arc::new(TelemetryState::new());
    let promotion = Arc::new(Promotion::new());
    // Event times count from here (before the first callback can run).
    let origin = telemetry::now();
    // One event channel per callback attempt; the consumer of the attempt that succeeded wins.
    let mut out_events = None;
    let mut out_errors = None;
    let mut in_errors = None;

    // Output (render) stream. Everything except the engine is rebuilt per attempt.
    let payload = OutputPayload {
        engine,
        input: ring_reader,
    };
    let (output, accepted, payload_rx) =
        build_with_fallback(payload, requested, |size, payload| {
            let (events, rx) = event_channel();
            out_events = Some(rx);
            let (on_error, errors) = error_callback(&telemetry, origin);
            out_errors = Some(errors);
            let mut callback = OutputCallback {
                events,
                origin,
                payload,
                adapter: CallbackAdapter::new(in_channels, out_choice.channels, max_frames),
                clock: Arc::clone(&clock),
                telemetry: Arc::clone(&telemetry),
                promotion: Arc::clone(&promotion),
                detector: XrunDetector::new(),
                sample_rate: spec.sample_rate,
                format: out_choice.format,
                scratch_out: vec![0.0; max_frames * usize::from(out_choice.channels)],
                scratch_in: vec![0.0; max_frames * usize::from(in_channels)],
                halted: false,
            };
            out_device.build_output_stream_raw(
                stream_config(out_choice, spec.sample_rate, size),
                cpal_format(out_choice.format),
                move |data: &mut cpal::Data, info: &cpal::OutputCallbackInfo| {
                    callback.run(data, info);
                },
                on_error,
                Some(BACKEND_TIMEOUT),
            )
        })
        .map_err(IoError::UnsupportedConfig)?;

    // Optional capture stream feeding the ring.
    let input = match (in_side, ring_writer) {
        (Some((device, choice)), Some(writer)) => {
            let chunk = max_frames * usize::from(choice.channels);
            let (stream, _, _) = build_with_fallback(writer, requested, |size, writer| {
                let (on_error, errors) = error_callback(&telemetry, origin);
                in_errors = Some(errors);
                let mut callback = InputCallback {
                    writer,
                    scratch: vec![0.0; chunk],
                    format: choice.format,
                    telemetry: Arc::clone(&telemetry),
                    sample_rate: spec.sample_rate,
                };
                device.build_input_stream_raw(
                    stream_config(choice, spec.sample_rate, size),
                    cpal_format(choice.format),
                    move |data: &cpal::Data, info: &cpal::InputCallbackInfo| {
                        callback.run(data, info);
                    },
                    on_error,
                    Some(BACKEND_TIMEOUT),
                )
            })
            .map_err(IoError::UnsupportedConfig)?;
            Some((stream, device.to_string()))
        }
        _ => None,
    };

    if let Some((stream, _)) = &input {
        stream.play().map_err(backend)?;
    }
    output.play().map_err(backend)?;

    let info = StreamInfo {
        host: spec.host,
        output_device: out_device.to_string(),
        input_device: input.as_ref().map(|(_, name)| name.clone()),
        sample_rate: spec.sample_rate,
        output_channels: out_choice.channels,
        input_channels: in_channels,
        sample_format: format_name(out_choice.format),
        requested_buffer_frames: accepted,
    };
    Ok(RunningStream {
        output,
        input: input.map(|(stream, _)| stream),
        payload_rx,
        telemetry,
        promotion,
        info,
        events: Mutex::new(
            [out_events, out_errors, in_errors]
                .into_iter()
                .flatten()
                .collect(),
        ),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Input,
    Output,
}

fn backend(e: impl std::fmt::Display) -> IoError {
    IoError::Backend(e.to_string())
}

/// A stream error callback and the consumer of its own event channel. Every stream error
/// (xrun, device change, …) counts and is reported as an [`XrunKind::StreamError`] event; the
/// callback only bumps atomics and pushes to a wait-free queue.
fn error_callback(
    telemetry: &Arc<TelemetryState>,
    origin: Instant,
) -> (
    impl FnMut(cpal::Error) + Send + 'static,
    Consumer<XrunEvent>,
) {
    let telemetry = Arc::clone(telemetry);
    let (mut tx, rx) = event_channel();
    let callback = move |_| {
        telemetry.on_stream_error();
        let event = XrunEvent {
            at: telemetry::now().saturating_duration_since(origin),
            kind: XrunKind::StreamError,
            load: 0.0,
            gap_frames: 0,
            barrier_spins: 0,
            frames: 0,
        };
        telemetry.push_event(&mut tx, event);
    };
    (callback, rx)
}

fn stream_config(
    choice: ConfigCandidate,
    sample_rate: u32,
    size: Option<u32>,
) -> cpal::StreamConfig {
    cpal::StreamConfig {
        channels: choice.channels,
        sample_rate,
        buffer_size: size.map_or(cpal::BufferSize::Default, cpal::BufferSize::Fixed),
    }
}

fn select_host(kind: HostKind) -> Result<cpal::Host, IoError> {
    let Some(name) = host_name(kind) else {
        return Ok(cpal::default_host());
    };
    let id = cpal::available_hosts()
        .into_iter()
        .find(|id| id.name().eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            let hint = if kind == HostKind::Asio && !cfg!(feature = "asio") {
                " (build atomik-audio-io with the `asio` feature)"
            } else {
                ""
            };
            IoError::HostUnavailable(format!("{name}{hint}"))
        })?;
    cpal::host_from_id(id).map_err(|e| IoError::HostUnavailable(format!("{name}: {e}")))
}

/// The capture device for a duplex stream: `requested` by name, else the device named like the
/// output. Never the host default input: that may be an unrelated microphone (a webcam), so
/// without a match the stream fails over to output-only and the user picks an input.
fn find_input_device(
    host: &cpal::Host,
    requested: Option<&str>,
    output_name: &str,
) -> Result<cpal::Device, IoError> {
    if requested.is_some() {
        return find_device(host, requested, Direction::Input);
    }
    let devices: Vec<cpal::Device> = host.input_devices().map_err(backend)?.collect();
    let names: Vec<String> = devices.iter().map(ToString::to_string).collect();
    match default_input_index(&names, output_name) {
        Some(i) => devices
            .into_iter()
            .nth(i)
            .ok_or_else(|| IoError::DeviceNotFound(output_name.to_string())),
        None => Err(IoError::DeviceNotFound(format!(
            "no input device is named like the output {output_name:?}; choose an input device              in Audio Settings"
        ))),
    }
}

fn find_device(
    host: &cpal::Host,
    query: Option<&str>,
    direction: Direction,
) -> Result<cpal::Device, IoError> {
    let Some(query) = query else {
        let default = match direction {
            Direction::Output => host.default_output_device(),
            Direction::Input => host.default_input_device(),
        };
        return default.ok_or_else(|| {
            IoError::DeviceNotFound(format!("the host has no default {direction:?} device"))
        });
    };
    let devices: Vec<cpal::Device> = match direction {
        Direction::Output => host.output_devices().map_err(backend)?.collect(),
        Direction::Input => host.input_devices().map_err(backend)?.collect(),
    };
    let names: Vec<String> = devices.iter().map(ToString::to_string).collect();
    let index = match_device(&names, query)?;
    devices
        .into_iter()
        .nth(index)
        .ok_or_else(|| IoError::DeviceNotFound(query.to_string()))
}

fn sample_kind(format: cpal::SampleFormat) -> Option<SampleKind> {
    match format {
        cpal::SampleFormat::F32 => Some(SampleKind::F32),
        cpal::SampleFormat::I32 => Some(SampleKind::I32),
        cpal::SampleFormat::I16 => Some(SampleKind::I16),
        _ => None,
    }
}

fn format_name(kind: SampleKind) -> &'static str {
    match kind {
        SampleKind::F32 => "f32",
        SampleKind::I32 => "i32",
        SampleKind::I16 => "i16",
    }
}

fn cpal_format(kind: SampleKind) -> cpal::SampleFormat {
    match kind {
        SampleKind::F32 => cpal::SampleFormat::F32,
        SampleKind::I32 => cpal::SampleFormat::I32,
        SampleKind::I16 => cpal::SampleFormat::I16,
    }
}

fn buffer_range(size: &cpal::SupportedBufferSize) -> Option<(u32, u32)> {
    match size {
        cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
        cpal::SupportedBufferSize::Unknown => None,
    }
}

fn choose(
    device: &cpal::Device,
    direction: Direction,
    spec: &StreamSpec,
) -> Result<ConfigCandidate, IoError> {
    let ranges: Vec<cpal::SupportedStreamConfigRange> = match direction {
        Direction::Output => device
            .supported_output_configs()
            .map_err(backend)?
            .collect(),
        Direction::Input => device.supported_input_configs().map_err(backend)?.collect(),
    };
    let candidates: Vec<ConfigCandidate> = ranges
        .iter()
        .filter_map(|r| {
            Some(ConfigCandidate {
                channels: r.channels(),
                min_rate: r.min_sample_rate(),
                max_rate: r.max_sample_rate(),
                format: sample_kind(r.sample_format())?,
                buffer: buffer_range(r.buffer_size()),
            })
        })
        .collect();
    let channels = match direction {
        Direction::Output => spec.output_channels,
        Direction::Input => spec.input_channels,
    };
    choose_config(&candidates, channels, spec.sample_rate).ok_or_else(|| {
        IoError::UnsupportedConfig(format!(
            "{device}: no f32/i32/i16 {direction:?} configuration at {} Hz (supported: {ranges:?})",
            spec.sample_rate
        ))
    })
}

/// What the output callback owns and gives back when dropped.
struct OutputPayload {
    engine: Engine,
    input: Option<RingReader>,
}

/// State of the output (render) callback. Everything is preallocated in [`start`].
struct OutputCallback {
    /// Producer of the xrun event channel (this callback is its only user).
    events: Producer<XrunEvent>,
    /// Stream start, the origin of [`XrunEvent::at`].
    origin: Instant,
    payload: ReturnOnDrop<OutputPayload>,
    adapter: CallbackAdapter,
    clock: Arc<FrameClock>,
    telemetry: Arc<TelemetryState>,
    promotion: Arc<Promotion>,
    detector: XrunDetector,
    sample_rate: u32,
    format: SampleKind,
    scratch_out: Vec<f32>,
    scratch_in: Vec<f32>,
    /// The callback caught a panic from the engine itself: it renders silence from then on.
    /// (A panicking device never gets here: the engine contains it.)
    halted: bool,
}

impl OutputCallback {
    fn run(&mut self, data: &mut cpal::Data, info: &cpal::OutputCallbackInfo) {
        // The one documented HI-1 exception: a single priority promotion on the first callback,
        // before anything is rendered. Later callbacks only load an atomic here.
        self.promotion.promote_once(rt::promote_current_thread);
        let start = telemetry::now();
        let Some(payload) = self.payload.get_mut().filter(|_| !self.halted) else {
            data.bytes_mut().fill(0);
            return;
        };
        self.clock.publish(payload.engine.host_frames(), start);
        let out_ch = usize::from(self.adapter.outputs()).max(1);
        let frames = data.len() / out_ch;
        let timestamp = info.timestamp();
        self.telemetry
            .on_output_timing(timestamp.callback, timestamp.playback, self.sample_rate);
        if let Some(reader) = payload.input.as_mut() {
            let in_ch = usize::from(self.adapter.inputs());
            reader.trim(in_ch * frames * (1 + INPUT_SLACK_BUFFERS));
        }
        let mut render = Render {
            engine: &mut payload.engine,
            input: payload.input.as_mut(),
            adapter: &mut self.adapter,
            scratch_in: &mut self.scratch_in,
        };
        // Last line of defence: a panic must never unwind into the host's audio thread. (The
        // engine already contains device panics; this catches anything else, and `catch_unwind`
        // costs nothing while nothing panics.)
        let rendered = catch_unwind(AssertUnwindSafe(|| match self.format {
            SampleKind::F32 => match data.as_slice_mut::<f32>() {
                Some(out) => render.f32(out),
                None => data.bytes_mut().fill(0),
            },
            SampleKind::I32 => match data.as_slice_mut::<i32>() {
                Some(out) => render.int(&mut self.scratch_out, out),
                None => data.bytes_mut().fill(0),
            },
            SampleKind::I16 => match data.as_slice_mut::<i16>() {
                Some(out) => render.int(&mut self.scratch_out, out),
                None => data.bytes_mut().fill(0),
            },
        }));
        if rendered.is_err() {
            self.halted = true;
            self.telemetry.set_callback_panicked();
            data.bytes_mut().fill(0);
            return;
        }
        self.telemetry
            .set_fault(payload.engine.fault().map(|id| id.0));
        if let Some(reader) = payload.input.as_ref() {
            let in_ch = usize::from(self.adapter.inputs()).max(1);
            let backlog = u32::try_from(reader.available() / in_ch).unwrap_or(u32::MAX);
            self.telemetry.on_input_backlog(backlog);
        }
        let elapsed = telemetry::now().saturating_duration_since(start);
        // Max over this callback's render calls (one for buffers up to the chunk size, where it
        // equals `Engine::last_render_barrier_spins`).
        let barrier_spins = payload.engine.take_max_barrier_spins();
        let frames = u32::try_from(frames).unwrap_or(u32::MAX);
        let kind = self.telemetry.on_callback(
            &mut self.detector,
            start,
            frames,
            self.sample_rate,
            elapsed,
            barrier_spins,
        );
        if let Some(kind) = kind {
            let event = XrunEvent {
                at: start.saturating_duration_since(self.origin),
                kind,
                load: LoadMeter::load(elapsed, frames, self.sample_rate),
                gap_frames: self.detector.last_gap_frames(),
                barrier_spins,
                frames,
            };
            self.telemetry.push_event(&mut self.events, event);
        }
    }
}

/// Borrowed render state for one callback.
struct Render<'a> {
    engine: &'a mut Engine,
    input: Option<&'a mut RingReader>,
    adapter: &'a mut CallbackAdapter,
    scratch_in: &'a mut [f32],
}

impl Render<'_> {
    /// Renders interleaved f32 `out` in chunks of `max_frames`, pulling captured input from the
    /// ring (silence without an input stream or on underrun).
    fn f32(&mut self, out: &mut [f32]) {
        let out_ch = usize::from(self.adapter.outputs()).max(1);
        let in_ch = usize::from(self.adapter.inputs());
        for dst in out.chunks_mut(self.adapter.max_frames() * out_ch) {
            let frames = dst.len() / out_ch;
            let src = &mut self.scratch_in[..frames * in_ch];
            match self.input.as_mut() {
                Some(reader) => {
                    reader.read(src);
                }
                None => src.fill(0.0),
            }
            self.adapter.process(self.engine, src, dst);
        }
    }

    /// Renders through the f32 scratch, then converts chunk by chunk.
    fn int<T: IntSample>(&mut self, scratch_out: &mut [f32], out: &mut [T]) {
        let out_ch = usize::from(self.adapter.outputs()).max(1);
        let chunk = self.adapter.max_frames() * out_ch;
        for dst in out.chunks_mut(chunk) {
            let scratch = &mut scratch_out[..dst.len()];
            self.f32(scratch);
            convert::from_f32(scratch, dst);
        }
    }
}

/// State of the input (capture) callback: converts to f32 and pushes into the ring.
struct InputCallback {
    writer: ReturnOnDrop<RingWriter>,
    scratch: Vec<f32>,
    format: SampleKind,
    telemetry: Arc<TelemetryState>,
    sample_rate: u32,
}

impl InputCallback {
    fn run(&mut self, data: &cpal::Data, info: &cpal::InputCallbackInfo) {
        let timestamp = info.timestamp();
        self.telemetry
            .on_input_timing(timestamp.callback, timestamp.capture, self.sample_rate);
        let Some(writer) = self.writer.get_mut() else {
            return;
        };
        match self.format {
            SampleKind::F32 => {
                if let Some(src) = data.as_slice::<f32>() {
                    writer.write(src);
                }
            }
            SampleKind::I32 => {
                if let Some(src) = data.as_slice::<i32>() {
                    push_int(writer, &mut self.scratch, src);
                }
            }
            SampleKind::I16 => {
                if let Some(src) = data.as_slice::<i16>() {
                    push_int(writer, &mut self.scratch, src);
                }
            }
        }
    }
}

fn push_int<T: IntSample>(writer: &mut RingWriter, scratch: &mut [f32], src: &[T]) {
    for chunk in src.chunks(scratch.len().max(1)) {
        let scratch = &mut scratch[..chunk.len()];
        convert::to_f32(chunk, scratch);
        writer.write(scratch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_backend_that_never_releases_the_engine_is_an_error_not_an_abort() {
        // The fake backend keeps the callback (here: just the sender) alive forever.
        let (tx, rx) = std::sync::mpsc::sync_channel::<u32>(1);
        let result = recover(&rx, Duration::from_millis(50));
        assert!(matches!(result, Err(IoError::Backend(_))));
        drop(tx);
    }

    #[test]
    fn a_backend_that_releases_the_callback_hands_the_payload_back() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<u32>(1);
        tx.send(7).unwrap();
        assert_eq!(recover(&rx, Duration::from_millis(50)).unwrap(), 7);
    }
}
