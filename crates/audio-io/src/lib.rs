//! Atomik Rack real-time audio I/O.
//!
//! Public signatures are a stable contract: changes need review.
//!
//! # Overview
//!
//! - [`list`] enumerates the hosts Atomik Rack knows (WASAPI, ASIO, CoreAudio, JACK,
//!   ALSA) and their devices.
//! - [`start`] opens an output stream (plus a capture stream when input channels are
//!   requested; cpal has no duplex stream, so captured samples reach the render callback through
//!   a wait-free ring) and drives [`atomik_engine::Engine::render`] from the callback. Devices
//!   are matched by exact name or unique case-insensitive substring. f32 is preferred; devices
//!   that only offer i32/i16 are converted through preallocated scratch. A fixed buffer size is
//!   requested first (clamped to the reported range); if the host rejects it, the host default
//!   period is used ([`StreamInfo::requested_buffer_frames`] is then `None`).
//! - Each callback: one-time real-time promotion on the first callback (the documented HI-1
//!   exception), then `clock.publish(engine.host_frames(), now)`, then the render through
//!   [`CallbackAdapter`], then telemetry. Nothing in the callback allocates, frees,
//!   locks, blocks or prints.
//! - Telemetry: [`XrunDetector`] counts callbacks arriving later than 1.5× the previous
//!   buffer period; stream error callbacks also count as xruns. [`LoadMeter`] accumulates
//!   render time / buffer period; average and peak reset on read
//!   ([`RunningStream::telemetry`]).
//! - Xrun diagnostics: each xrun is classified ([`XrunKind`]: late callback, render overrun,
//!   stream error) and counted separately in [`Telemetry`] (`xruns` is their sum, at most one
//!   per callback). The callback pushes an [`XrunEvent`] (time, kind, load, gap, the engine's
//!   level-barrier spins) into a wait-free channel; the control thread reads them with
//!   [`RunningStream::drain_events`]. A full channel drops and counts events, never blocks.
//! - Liveness: [`Liveness`] compares the callback counter against the clock on the control
//!   thread and reports [`StreamState::Stalled`] when no callback arrives for
//!   `max(4 periods, 250 ms)`; a stream error that says the device is gone sets
//!   [`Telemetry::device_lost`] (the error callback only stores an atomic). Neither needs the
//!   callback to do more than it does.
//! - [`RunningStream::stop`] returns the engine: the callback owns it and sends it back through
//!   a one-slot channel when the stream drops the callback, so the engine is dropped on the
//!   caller's thread.
//!
//! Wall-clock reads exist only in the telemetry module (`telemetry::now`, with a justified
//! local clippy allow); they time callbacks and live events and never influence audio.
//!
//! # The `asio` feature (Windows)
//!
//! ASIO is opt-in because it needs the Steinberg ASIO SDK at build time:
//!
//! 1. Download the ASIO SDK from Steinberg's developer site and unpack it, e.g. to
//!    `<path-to-asio-sdk>`.
//! 2. Set `CPAL_ASIO_DIR` to that directory (cpal's `asio-sys` build script reads it).
//! 3. Install LLVM/Clang (needed by `bindgen` to generate the SDK bindings) and make sure
//!    `LIBCLANG_PATH` points at its `bin` directory if it is not found automatically.
//! 4. Build with `cargo build -p atomik-audio-io --features asio` (or enable the feature from
//!    the depending crate), then select [`HostKind::Asio`].
//!
//! Without the feature, [`HostKind::Asio`] yields [`IoError::HostUnavailable`].
//!
//! Licence: the ASIO SDK (2.3.4, 2025) is dual-licensed by Steinberg under its proprietary
//! ASIO licence *or* GPL-3.0, so GPL-3.0 builds of Atomik Rack may include it (risk
//! resolved). The SDK itself is never committed to this repository. ASIO is a trademark of
//! Steinberg Media Technologies GmbH; if the name or logo is used in branding, follow
//! Steinberg's trademark guidelines.
//!
//! Verified on Windows with ASIO SDK 2.3.4 and LLVM (winget `LLVM.LLVM`):
//! `CPAL_ASIO_DIR=<sdk>\ASIOSDK`, `LIBCLANG_PATH=C:\Program Files\LLVM\bin`,
//! `cargo build --release -p atomik-play --features asio`.

mod adapter;

mod convert;
mod handoff;
mod histogram;
mod liveness;
mod ring;
mod rt;
mod select;
mod stream;
mod telemetry;

pub use adapter::CallbackAdapter;
pub use histogram::{
    CallbackStats, SPIN_BUCKETS, TIME_BUCKETS, spin_bucket, spin_bucket_max, time_bucket,
    time_bucket_max_ns,
};
pub use liveness::{Liveness, STALL_MIN, STALL_PERIODS, START_GRACE, StreamState, stall_after};
pub use stream::{RunningStream, StreamInfo, list, start, start_with_input};
pub use telemetry::{LoadMeter, XrunDetector, is_late, is_overrun};

static WORKER_PROMOTION_FAILURES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Raises the calling thread to real-time priority (same policy as the audio callback).
/// Meant for `atomik_engine::WorkerConfig::on_thread_start`, which runs once per worker before
/// it renders anything (non-RT context). Failures are counted, never fatal.
pub fn promote_worker_thread() {
    if !rt::promote_current_thread() {
        WORKER_PROMOTION_FAILURES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod cap_tests {
    use super::cap_workers;

    #[test]
    fn workers_leave_a_core_for_the_audio_thread() {
        assert_eq!(cap_workers(4, 8), 4);
        assert_eq!(cap_workers(16, 4), 3);
        assert_eq!(cap_workers(2, 2), 1);
        assert_eq!(cap_workers(2, 1), 0);
        assert_eq!(cap_workers(2, 0), 0);
        assert_eq!(cap_workers(0, 8), 0);
        assert_eq!(cap_workers(3, usize::MAX), 3);
    }
}

/// Number of [`promote_worker_thread`] calls that failed to raise the priority.
pub fn worker_promotion_failures() -> u64 {
    WORKER_PROMOTION_FAILURES.load(std::sync::atomic::Ordering::Relaxed)
}

/// How many render workers to start for `requested` on a machine with `cores` hardware threads:
/// at most `cores - 1`, so the audio thread always has a core of its own and a worker never
/// competes with it. One core (or an unknown count of zero) means no workers.
pub fn cap_workers(requested: u16, cores: usize) -> u16 {
    let room = cores.saturating_sub(1).min(usize::from(u16::MAX)) as u16;
    requested.min(room)
}

/// [`cap_workers`] for this machine (`available_parallelism`; one core when unknown).
pub fn cap_workers_here(requested: u16) -> u16 {
    cap_workers(
        requested,
        std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
    )
}

/// Audio host API selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// The platform default host.
    Default,
    /// Windows WASAPI (shared mode).
    Wasapi,
    /// Windows ASIO (only with the `asio` feature).
    Asio,
    /// macOS CoreAudio.
    CoreAudio,
    /// JACK (Linux).
    Jack,
    /// ALSA (Linux).
    Alsa,
}

/// What to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSpec {
    /// Host API.
    pub host: HostKind,
    /// Device name (exact or unique case-insensitive substring); `None` = host default output.
    pub device: Option<String>,
    /// Sample rate in Hz (must match the engine's `EngineConfig`).
    pub sample_rate: u32,
    /// Requested buffer size in frames (best effort; the actual size is reported in telemetry).
    pub buffer_frames: u32,
    /// Host input channels routed to the engine's interface `in_k` sockets.
    pub input_channels: u16,
    /// Host output channels fed from the engine's interface `out_k` sockets.
    pub output_channels: u16,
}

/// One audio device as listed by [`list`].
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    /// Device name.
    pub name: String,
    /// Maximum input channels.
    pub max_inputs: u16,
    /// Maximum output channels.
    pub max_outputs: u16,
    /// Supported sample rates among 44.1/48/88.2/96/192 kHz.
    pub sample_rates: Vec<u32>,
    /// Supported buffer-size range in frames, if the host reports one.
    pub buffer_range: Option<(u32, u32)>,
    /// Whether this is the host's default output device.
    pub is_default_output: bool,
}

/// One host API and its devices.
#[derive(Debug, Clone, PartialEq)]
pub struct HostInfo {
    /// Host kind.
    pub kind: HostKind,
    /// Devices.
    pub devices: Vec<DeviceInfo>,
}

/// Snapshot of stream telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Telemetry {
    /// Callbacks executed.
    pub callbacks: u64,
    /// Detected xruns: `late_callbacks + render_overruns + stream_errors`. Each callback adds
    /// at most one; a late callback that the previous render overrun explains is not counted
    /// again (see [`XrunDetector::classify`]).
    pub xruns: u64,
    /// Actual buffer size of the most recent callback, in frames.
    pub buffer_frames: u32,
    /// Average DSP load (render time / buffer period) since the last snapshot, 0..∞.
    pub dsp_load_avg: f32,
    /// Peak DSP load since the last snapshot.
    pub dsp_load_peak: f32,
    /// Xruns classified [`XrunKind::LateCallback`] (total): the host called back late (driver
    /// or OS).
    pub late_callbacks: u64,
    /// Xruns classified [`XrunKind::RenderOverrun`] (total): the render took longer than the
    /// buffer period.
    pub render_overruns: u64,
    /// Stream error callbacks (total), [`XrunKind::StreamError`].
    pub stream_errors: u64,
    /// Highest per-callback level-barrier wait of the engine since the last snapshot, in spins
    /// (`atomik_engine::Engine::take_max_barrier_spins`); resets on read.
    pub max_barrier_spins: u64,
    /// [`XrunEvent`]s dropped (total) because the event channel was full.
    pub events_dropped: u64,
    /// Smoothed input latency in frames: the delay from the ADC to the output callback
    /// that reads the frame: the input callback's `callback - capture` plus the frames the
    /// input ring holds back. 0 without input or before the first callbacks.
    pub input_latency_frames: u32,
    /// Smoothed output latency in frames: the output callback's `playback - callback`
    /// (the engine's own 64-frame FIFO is not included). 0 before the first callback.
    pub output_latency_frames: u32,
    /// True once the engine has faulted: a device panicked while rendering (it is silent from
    /// then on and everything else keeps sounding), or the render callback itself caught a
    /// panic (the callback is then silent). Sticky until the stream is rebuilt.
    pub faulted: bool,
    /// The engine's device instance id of the first device that panicked, when that is what
    /// faulted; `None` for a fault outside any device (or no fault).
    pub faulted_device: Option<u32>,
    /// Captured input frames (per channel) lost because the input and output clocks drift
    /// apart: the writer found the input ring full, or the render callback discarded the oldest
    /// captured frames to keep the latency bounded (the very first discard, of whatever was
    /// captured before the output started, is not counted). Each is a click in monitored input
    /// and recorded takes. Total; 0 without input.
    pub input_overruns: u64,
    /// True once a stream error callback reported the device gone (unplugged, or the stream
    /// became invalid). Sticky until the stream is rebuilt. A stream whose callbacks merely stop
    /// is detected by [`Liveness`] instead.
    pub device_lost: bool,
    /// Callback time and level-barrier wait histograms, totals since the stream started (they do
    /// not reset on read). Percentiles: [`CallbackStats::time_percentile_ns`].
    pub callback: CallbackStats,
}

/// What kind of xrun an [`XrunEvent`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum XrunKind {
    /// The callback arrived more than 1.5 periods after the previous one, and the previous
    /// render does not explain it: the driver or the OS delivered it late.
    LateCallback,
    /// The render took longer than the buffer period.
    RenderOverrun,
    /// The backend reported a stream error.
    StreamError,
}

impl XrunKind {
    /// The snake-case name used in player output: `late_callback`, `render_overrun`,
    /// `stream_error`.
    pub fn name(self) -> &'static str {
        match self {
            Self::LateCallback => "late_callback",
            Self::RenderOverrun => "render_overrun",
            Self::StreamError => "stream_error",
        }
    }
}

impl std::fmt::Display for XrunKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// One xrun, sent from the audio callback (or the stream error callback) to the control
/// thread through a lock-free channel of [`XRUN_EVENT_CAPACITY`] events; read them with
/// [`RunningStream::drain_events`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct XrunEvent {
    /// When the callback started (or the error was reported), since the stream started.
    pub at: std::time::Duration,
    /// What happened.
    pub kind: XrunKind,
    /// DSP load of that callback (render time / buffer period; 1.0 = 100 %). 0 for stream
    /// errors.
    pub load: f32,
    /// Distance to the previous callback's start, in frames at the stream rate (0 for the
    /// first callback and for stream errors).
    pub gap_frames: u32,
    /// Highest engine level-barrier wait among the callback's render calls, in spins (0 for
    /// stream errors and single-threaded engines).
    pub barrier_spins: u64,
    /// Buffer size of that callback, in frames (0 for stream errors).
    pub frames: u32,
}

/// Capacity of the xrun event channel. When it is full, new events are dropped and counted in
/// [`Telemetry::events_dropped`].
pub const XRUN_EVENT_CAPACITY: usize = 256;

/// Audio I/O errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IoError {
    /// The requested host is not available (e.g. ASIO without the feature).
    #[error("audio host {0} is not available")]
    HostUnavailable(String),
    /// No device matched the requested name, or the match was ambiguous.
    #[error("audio device not found or ambiguous: {0}")]
    DeviceNotFound(String),
    /// The device rejected the configuration.
    #[error("unsupported stream configuration: {0}")]
    UnsupportedConfig(String),
    /// The backend failed.
    #[error("audio backend error: {0}")]
    Backend(String),
}

#[cfg(test)]
atomik_testkit::install_alloc_guard!();
