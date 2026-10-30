//! Stream telemetry: xrun detection and classification, DSP load, published through
//! atomics, plus the xrun event channel (audio thread → control thread).

use crate::{Telemetry, XRUN_EVENT_CAPACITY, XrunEvent, XrunKind};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The only wall-clock read of this crate: timestamps the start and end of each audio callback.
#[allow(clippy::disallowed_methods)] // Justification:  telemetry live-event timing
// only. The instant feeds xrun/DSP-load counters and the FrameClock used to place live MIDI notes;
// it never influences rendered samples, so audio output stays deterministic for given inputs
//. `Instant::now` reads a monotonic counter and does not allocate, lock or block.
pub(crate) fn now() -> Instant {
    Instant::now()
}

/// Counts late callbacks: a callback whose distance to the previous one exceeds 1.5× the
/// period (`frames / sample_rate`) of both the previous and the current buffer. Pure: fed with
/// instants, never reads a clock itself.
///
/// [`XrunDetector::classify`] also judges the render time and attributes each xrun to one
/// [`XrunKind`] (see its docs for the counting rule).
#[derive(Debug, Clone, Copy, Default)]
pub struct XrunDetector {
    previous: Option<(Instant, u32, u32)>,
    late: u64,
    overruns: u64,
    /// Render time of the previous callback, when it overran its period (`classify` only).
    previous_overrun: Option<Duration>,
    /// Gap between the two latest callbacks, in frames at the latest callback's rate.
    last_gap_frames: u32,
}

impl XrunDetector {
    /// A detector that has seen no callback yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a callback starting at `at` that renders `frames` at `sample_rate`; returns
    /// whether it was late (an xrun).
    pub fn on_callback(&mut self, at: Instant, frames: u32, sample_rate: u32) -> bool {
        let gap = self.record(at, frames, sample_rate);
        let late = gap.is_some_and(|(gap, prev_frames, prev_rate)| {
            Self::late_gap(gap, prev_frames, prev_rate, frames, sample_rate)
        });
        if late {
            self.late += 1;
        }
        self.previous_overrun = None;
        late
    }

    /// Records a callback that started at `at`, rendered `frames` at `sample_rate` and whose
    /// render took `render`; returns the xrun it represents, if any. Pure: no clock read, no
    /// allocation.
    ///
    /// **Counting rule (no double counting).** Each callback is at most one xrun:
    ///
    /// - `render > frames / sample_rate` → [`XrunKind::RenderOverrun`], whether or not the
    ///   callback also arrived late (the event's `gap_frames` still shows the gap);
    /// - otherwise a late gap ([`is_late`]) → [`XrunKind::LateCallback`], except right after an
    ///   overrun: the host cannot call back before the overrunning render returned, so then
    ///   lateness is measured from the *end* of that render (`gap − previous render`). A late
    ///   callback that the overrun explains is therefore counted once, as the overrun; a stall
    ///   longer than the overrun explains is still a late callback.
    pub fn classify(
        &mut self,
        at: Instant,
        frames: u32,
        sample_rate: u32,
        render: Duration,
    ) -> Option<XrunKind> {
        let gap = self.record(at, frames, sample_rate);
        let late = gap.is_some_and(|(gap, prev_frames, prev_rate)| {
            let gap = match self.previous_overrun {
                Some(previous_render) => gap.saturating_sub(previous_render),
                None => gap,
            };
            Self::late_gap(gap, prev_frames, prev_rate, frames, sample_rate)
        });
        let overrun = is_overrun(render, frames, sample_rate);
        self.previous_overrun = overrun.then_some(render);
        if overrun {
            self.overruns += 1;
            Some(XrunKind::RenderOverrun)
        } else if late {
            self.late += 1;
            Some(XrunKind::LateCallback)
        } else {
            None
        }
    }

    /// Late callbacks seen so far.
    pub fn late_callbacks(&self) -> u64 {
        self.late
    }

    /// Render overruns seen so far by [`XrunDetector::classify`].
    pub fn render_overruns(&self) -> u64 {
        self.overruns
    }

    /// Gap between the start of the latest callback and the previous one, in frames at the
    /// latest callback's sample rate (0 after the first callback).
    pub fn last_gap_frames(&self) -> u32 {
        self.last_gap_frames
    }

    /// Stores this callback; returns the gap to the previous one with the previous buffer.
    fn record(
        &mut self,
        at: Instant,
        frames: u32,
        sample_rate: u32,
    ) -> Option<(Duration, u32, u32)> {
        let gap = self.previous.map(|(prev_at, prev_frames, prev_rate)| {
            (
                at.saturating_duration_since(prev_at),
                prev_frames,
                prev_rate,
            )
        });
        self.last_gap_frames = gap.map_or(0, |(gap, _, _)| duration_frames(gap, sample_rate));
        self.previous = Some((at, frames, sample_rate));
        gap
    }

    /// Hosts may deliver variable buffers (WASAPI shared mode: a long gap followed by a
    /// double-size buffer). Late only if the gap exceeds 1.5x both periods.
    fn late_gap(gap: Duration, prev_frames: u32, prev_rate: u32, frames: u32, rate: u32) -> bool {
        is_late(gap, prev_frames, prev_rate) && is_late(gap, frames, rate)
    }
}

/// `d` in frames at `sample_rate`, rounded down and saturated to `u32`.
fn duration_frames(d: Duration, sample_rate: u32) -> u32 {
    let frames = d.as_nanos() * u128::from(sample_rate) / 1_000_000_000;
    u32::try_from(frames).unwrap_or(u32::MAX)
}

/// The frames at `sample_rate` between two stream instants (`later - earlier`, saturating to 0
/// when the host reports them the wrong way round).
pub(crate) fn frames_between(
    later: cpal::StreamInstant,
    earlier: cpal::StreamInstant,
    sample_rate: u32,
) -> u32 {
    duration_frames(later.saturating_duration_since(earlier), sample_rate)
}

/// A smoothed latency in frames, written by one audio callback and read by the control
/// thread. Stored as `frames + 1` so that 0 means "not measured yet".
#[derive(Debug, Default)]
pub(crate) struct LatencyMeter(AtomicU32);

impl LatencyMeter {
    /// Folds in one measurement: the first is taken as is, later ones move the value by 1/8
    /// of the difference (a one-pole smoother). Wait-free; single writer.
    pub(crate) fn record(&self, frames: u32) {
        let old = self.0.load(Ordering::Relaxed);
        let new = if old == 0 {
            i64::from(frames)
        } else {
            let old = i64::from(old - 1);
            old + (i64::from(frames) - old) / 8
        };
        let stored = u32::try_from(new.max(0)).unwrap_or(u32::MAX - 1);
        self.0.store(stored.saturating_add(1), Ordering::Relaxed);
    }

    /// The smoothed value; 0 before the first measurement.
    pub(crate) fn get(&self) -> u32 {
        self.0.load(Ordering::Relaxed).saturating_sub(1)
    }
}

/// Whether a callback `gap` after the previous one, whose buffer held `frames` at
/// `sample_rate`, is late: `gap > 1.5 × frames / sample_rate`. Empty buffers are never judged.
pub fn is_late(gap: Duration, frames: u32, sample_rate: u32) -> bool {
    if frames == 0 || sample_rate == 0 {
        return false;
    }
    // gap > 1.5·frames/sr  ⇔  2·gap_ns·sr > 3·frames·1e9 (exact integer arithmetic).
    2 * gap.as_nanos() * u128::from(sample_rate) > 3 * u128::from(frames) * 1_000_000_000
}

/// Whether a render that took `render` for `frames` at `sample_rate` overran its budget:
/// `render > frames / sample_rate` (a DSP load above 100 %). Empty buffers are never judged.
pub fn is_overrun(render: Duration, frames: u32, sample_rate: u32) -> bool {
    if frames == 0 || sample_rate == 0 {
        return false;
    }
    // render > frames/sr  ⇔  render_ns·sr > frames·1e9 (exact integer arithmetic).
    render.as_nanos() * u128::from(sample_rate) > u128::from(frames) * 1_000_000_000
}

/// Fixed-point scale of the load accumulators (parts per million).
const PPM: f64 = 1_000_000.0;

/// DSP load meter: render time divided by the buffer period, accumulated lock-free by the
/// audio thread and read (and reset) by the control thread.
#[derive(Debug, Default)]
pub struct LoadMeter {
    sum_ppm: AtomicU64,
    count: AtomicU64,
    peak_ppm: AtomicU64,
}

impl LoadMeter {
    /// An empty meter.
    pub fn new() -> Self {
        Self::default()
    }

    /// The load of one callback: `render / (frames / sample_rate)`; 0 for an empty buffer.
    pub fn load(render: Duration, frames: u32, sample_rate: u32) -> f32 {
        if frames == 0 || sample_rate == 0 {
            return 0.0;
        }
        (render.as_secs_f64() * f64::from(sample_rate) / f64::from(frames)) as f32
    }

    /// Records one callback (audio thread; wait-free, never allocates).
    pub fn record(&self, render: Duration, frames: u32, sample_rate: u32) {
        if frames == 0 || sample_rate == 0 {
            return;
        }
        let ppm = (f64::from(Self::load(render, frames, sample_rate)) * PPM).round() as u64;
        self.sum_ppm.fetch_add(ppm, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.peak_ppm.fetch_max(ppm, Ordering::Relaxed);
    }

    /// Average and peak load since the previous call, then resets both.
    pub fn take(&self) -> (f32, f32) {
        let sum = self.sum_ppm.swap(0, Ordering::Relaxed);
        let count = self.count.swap(0, Ordering::Relaxed);
        let peak = self.peak_ppm.swap(0, Ordering::Relaxed);
        let avg = if count == 0 {
            0.0
        } else {
            sum as f64 / count as f64 / PPM
        };
        (avg as f32, (peak as f64 / PPM) as f32)
    }
}

/// A new xrun event channel of [`XRUN_EVENT_CAPACITY`] events (allocates; call it off the
/// audio thread). The producer goes to one callback, the consumer to the control side.
pub(crate) fn event_channel() -> (Producer<XrunEvent>, Consumer<XrunEvent>) {
    RingBuffer::new(XRUN_EVENT_CAPACITY)
}

/// Telemetry shared between the audio callback, the stream error callback and the control
/// thread.
#[derive(Debug, Default)]
pub(crate) struct TelemetryState {
    callbacks: AtomicU64,
    late: AtomicU64,
    overruns: AtomicU64,
    errors: AtomicU64,
    max_spins: AtomicU64,
    events_dropped: AtomicU64,
    buffer_frames: AtomicU32,
    /// Sticky engine fault: 0 none, `u64::MAX` the callback itself panicked, else the faulted
    /// device's instance id + 1.
    fault: AtomicU64,
    load: LoadMeter,
    /// Input capture-to-callback delay, output callback-to-playback delay, and the frames
    /// waiting in the input ring after each render (all smoothed, in frames).
    in_capture: LatencyMeter,
    in_backlog: LatencyMeter,
    out_playback: LatencyMeter,
}

impl TelemetryState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Publishes the engine's fault state (one relaxed store; the callback calls it each time).
    pub(crate) fn set_fault(&self, device: Option<u32>) {
        let value = device.map_or(0, |id| u64::from(id) + 1);
        // A callback-level panic is never overwritten by a device fault.
        if self.fault.load(Ordering::Relaxed) != u64::MAX {
            self.fault.store(value, Ordering::Relaxed);
        }
    }

    /// Records that the render callback itself panicked (sticky).
    pub(crate) fn set_callback_panicked(&self) {
        self.fault.store(u64::MAX, Ordering::Relaxed);
    }

    /// Records one audio callback (wait-free, never allocates); returns its xrun, if any.
    /// `barrier_spins` is the engine's level-barrier wait during this callback.
    pub(crate) fn on_callback(
        &self,
        detector: &mut XrunDetector,
        at: Instant,
        frames: u32,
        sample_rate: u32,
        render: Duration,
        barrier_spins: u64,
    ) -> Option<XrunKind> {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        self.buffer_frames.store(frames, Ordering::Relaxed);
        self.max_spins.fetch_max(barrier_spins, Ordering::Relaxed);
        let kind = detector.classify(at, frames, sample_rate, render);
        match kind {
            Some(XrunKind::LateCallback) => self.late.fetch_add(1, Ordering::Relaxed),
            Some(XrunKind::RenderOverrun) => self.overruns.fetch_add(1, Ordering::Relaxed),
            Some(XrunKind::StreamError) | None => 0,
        };
        self.load.record(render, frames, sample_rate);
        kind
    }

    /// Records the timestamps of one input callback: `callback - capture` (wait-free).
    pub(crate) fn on_input_timing(
        &self,
        callback: cpal::StreamInstant,
        capture: cpal::StreamInstant,
        sample_rate: u32,
    ) {
        self.in_capture
            .record(frames_between(callback, capture, sample_rate));
    }

    /// Records the timestamps of one output callback: `playback - callback` (wait-free).
    pub(crate) fn on_output_timing(
        &self,
        callback: cpal::StreamInstant,
        playback: cpal::StreamInstant,
        sample_rate: u32,
    ) {
        self.out_playback
            .record(frames_between(playback, callback, sample_rate));
    }

    /// Records the captured frames still waiting in the input ring after a render (the delay
    /// the ring adds on top of the device's own).
    pub(crate) fn on_input_backlog(&self, frames: u32) {
        self.in_backlog.record(frames);
    }

    /// Records one stream error callback (wait-free).
    pub(crate) fn on_stream_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Pushes `event` without blocking or allocating; when the channel is full the event is
    /// dropped and counted. Returns whether it was queued.
    pub(crate) fn push_event(&self, tx: &mut Producer<XrunEvent>, event: XrunEvent) -> bool {
        let queued = tx.push(event).is_ok();
        if !queued {
            self.events_dropped.fetch_add(1, Ordering::Relaxed);
        }
        queued
    }

    pub(crate) fn snapshot(&self) -> Telemetry {
        let (dsp_load_avg, dsp_load_peak) = self.load.take();
        let late_callbacks = self.late.load(Ordering::Relaxed);
        let render_overruns = self.overruns.load(Ordering::Relaxed);
        let stream_errors = self.errors.load(Ordering::Relaxed);
        let fault = self.fault.load(Ordering::Relaxed);
        Telemetry {
            callbacks: self.callbacks.load(Ordering::Relaxed),
            xruns: late_callbacks + render_overruns + stream_errors,
            buffer_frames: self.buffer_frames.load(Ordering::Relaxed),
            dsp_load_avg,
            dsp_load_peak,
            late_callbacks,
            render_overruns,
            stream_errors,
            max_barrier_spins: self.max_spins.swap(0, Ordering::Relaxed),
            events_dropped: self.events_dropped.load(Ordering::Relaxed),
            input_latency_frames: self.in_capture.get().saturating_add(self.in_backlog.get()),
            output_latency_frames: self.out_playback.get(),
            faulted: fault != 0,
            faulted_device: match fault {
                0 | u64::MAX => None,
                n => u32::try_from(n - 1).ok(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_fault_is_sticky_and_a_callback_panic_wins() {
        let state = TelemetryState::new();
        assert!(!state.snapshot().faulted);
        state.set_fault(Some(7));
        let t = state.snapshot();
        assert!(t.faulted);
        assert_eq!(t.faulted_device, Some(7));
        state.set_callback_panicked();
        state.set_fault(Some(7));
        let t = state.snapshot();
        assert!(t.faulted);
        assert_eq!(t.faulted_device, None);
    }

    #[allow(clippy::disallowed_methods)] // Justification: test-only origin of a simulated timeline.
    fn origin() -> Instant {
        Instant::now()
    }

    fn period(frames: u32, sr: u32) -> Duration {
        Duration::from_secs_f64(f64::from(frames) / f64::from(sr))
    }

    #[test]
    fn xrun_detector_regular_timeline_counts_nothing() {
        let mut d = XrunDetector::new();
        let t0 = origin();
        let p = period(256, 48_000);
        for i in 0..100 {
            assert!(!d.on_callback(t0 + p * i, 256, 48_000));
        }
        assert_eq!(d.late_callbacks(), 0);
    }

    #[test]
    fn xrun_detector_one_late_callback_counts_exactly_one() {
        //
        let mut d = XrunDetector::new();
        let t0 = origin();
        let p = period(256, 48_000);
        let mut t = t0;
        for i in 0..20 {
            t += if i == 10 { p * 2 } else { p };
            d.on_callback(t, 256, 48_000);
        }
        assert_eq!(d.late_callbacks(), 1);
    }

    #[test]
    fn xrun_detector_jitter_below_one_and_a_half_periods_is_not_an_xrun() {
        let mut d = XrunDetector::new();
        let t0 = origin();
        let p = period(480, 48_000); // 10 ms
        d.on_callback(t0, 480, 48_000);
        assert!(!d.on_callback(t0 + Duration::from_millis(14), 480, 48_000));
        assert!(!d.on_callback(t0 + Duration::from_millis(15), 480, 48_000)); // early is fine
        assert!(d.on_callback(
            t0 + Duration::from_millis(15) + p * 3 / 2 + Duration::from_micros(1),
            480,
            48_000
        ));
        assert_eq!(d.late_callbacks(), 1);
    }

    #[test]
    fn xrun_detector_judges_the_gap_against_the_larger_of_both_buffers() {
        // A host delivering variable buffers (WASAPI: 441, then 882 frames after a 20 ms gap)
        // is not late: the long buffer covers the long gap.
        let mut d = XrunDetector::new();
        let t0 = origin();
        d.on_callback(t0, 441, 44_100);
        assert!(!d.on_callback(t0 + Duration::from_millis(20), 882, 44_100));
        // A short buffer after a long gap is late.
        let t1 = t0 + Duration::from_millis(20);
        assert!(d.on_callback(t1 + Duration::from_millis(40), 441, 44_100));
        // Two short buffers with a gap of 2 periods: late.
        let t2 = t1 + Duration::from_millis(40);
        d.on_callback(t2 + period(64, 48_000), 64, 48_000);
        assert!(d.on_callback(t2 + period(64, 48_000) * 3, 64, 48_000));
        assert_eq!(d.late_callbacks(), 2);
    }

    #[test]
    fn xrun_detector_first_callback_and_empty_buffers_are_never_late() {
        let mut d = XrunDetector::new();
        let t0 = origin();
        assert!(!d.on_callback(t0, 256, 48_000));
        d.on_callback(t0 + Duration::from_millis(1), 0, 48_000);
        assert!(!d.on_callback(t0 + Duration::from_secs(1), 256, 48_000));
        assert_eq!(d.late_callbacks(), 0);
    }

    #[test]
    fn load_meter_load_is_render_time_over_period() {
        let l = LoadMeter::load(Duration::from_millis(5), 480, 48_000);
        assert!((l - 0.5).abs() < 1e-6, "{l}");
        assert!(LoadMeter::load(Duration::from_millis(5), 0, 48_000).abs() < f32::EPSILON);
    }

    #[test]
    fn load_meter_take_returns_average_and_peak_then_resets() {
        let m = LoadMeter::new();
        m.record(Duration::from_millis(1), 480, 48_000); // 0.1
        m.record(Duration::from_millis(3), 480, 48_000); // 0.3
        m.record(Duration::from_millis(2), 480, 48_000); // 0.2
        let (avg, peak) = m.take();
        assert!((avg - 0.2).abs() < 1e-4, "{avg}");
        assert!((peak - 0.3).abs() < 1e-4, "{peak}");
        let (avg, peak) = m.take();
        assert!(avg.abs() < f32::EPSILON && peak.abs() < f32::EPSILON);
        m.record(Duration::from_millis(15), 480, 48_000); // overload 1.5
        let (avg, peak) = m.take();
        assert!((avg - 1.5).abs() < 1e-4 && (peak - 1.5).abs() < 1e-4);
    }

    #[test]
    fn telemetry_state_snapshot_counts_callbacks_errors_and_late_callbacks() {
        // through the published atomics.
        let state = TelemetryState::new();
        let mut d = XrunDetector::new();
        let t0 = origin();
        let p = period(128, 48_000);
        let mut t = t0;
        for i in 0..10 {
            t += if i == 5 { p * 3 } else { p };
            state.on_callback(&mut d, t, 128, 48_000, p / 4, 0);
        }
        state.on_stream_error();
        let s = state.snapshot();
        assert_eq!(s.callbacks, 10);
        assert_eq!(s.xruns, 2); // one late callback + one stream error
        assert_eq!(s.buffer_frames, 128);
        assert!((s.dsp_load_avg - 0.25).abs() < 1e-3, "{}", s.dsp_load_avg);
        assert!((s.dsp_load_peak - 0.25).abs() < 1e-3);
        let s = state.snapshot();
        assert_eq!((s.callbacks, s.xruns), (10, 2)); // counters are totals
        assert!(s.dsp_load_avg.abs() < f32::EPSILON); // load resets on read
    }

    #[test]
    fn telemetry_state_callback_is_allocation_free() {
        let state = TelemetryState::new();
        let mut d = XrunDetector::new();
        let t0 = origin();
        atomik_testkit::no_alloc::assert_no_alloc(|| {
            state.on_callback(&mut d, t0, 128, 48_000, Duration::from_micros(100), 7);
            state.on_stream_error();
        });
    }
    // ---- latency ----------------------------------------------------------------------

    #[test]
    fn instants_convert_to_frames_at_the_stream_rate() {
        let a = cpal::StreamInstant::from_nanos(1_000_000_000);
        let b = cpal::StreamInstant::from_nanos(1_010_000_000); // 10 ms later
        assert_eq!(frames_between(b, a, 48_000), 480);
        assert_eq!(frames_between(b, a, 44_100), 441);
        // A timestamp that goes backwards (a driver quirk) is no latency, never a wrap.
        assert_eq!(frames_between(a, b, 48_000), 0);
    }

    #[test]
    fn the_latency_meter_starts_at_the_first_value_then_smooths() {
        let m = LatencyMeter::default();
        assert_eq!(m.get(), 0);
        m.record(480);
        assert_eq!(m.get(), 480, "the first measurement is taken as is");
        m.record(480 + 80);
        assert_eq!(m.get(), 490, "one eighth of the step");
        for _ in 0..200 {
            m.record(1_000);
        }
        assert!(m.get().abs_diff(1_000) <= 8, "{}", m.get());
        m.record(0);
        assert!(m.get() < 1_000, "a zero is a measurement, not 'unset'");
    }

    #[test]
    fn latency_recording_is_allocation_free_and_reaches_the_snapshot() {
        let state = TelemetryState::new();
        let cap = cpal::StreamInstant::from_nanos(5_000_000_000);
        let cb = cpal::StreamInstant::from_nanos(5_004_000_000);
        let pb = cpal::StreamInstant::from_nanos(5_016_000_000);
        atomik_testkit::no_alloc::assert_no_alloc(|| {
            state.on_input_timing(cb, cap, 48_000);
            state.on_output_timing(cb, pb, 48_000);
            state.on_input_backlog(96);
        });
        let s = state.snapshot();
        assert_eq!(s.output_latency_frames, 576, "12 ms");
        assert_eq!(
            s.input_latency_frames,
            192 + 96,
            "4 ms capture to callback, plus the ring backlog"
        );
        let fresh = TelemetryState::new().snapshot();
        assert_eq!(
            (fresh.input_latency_frames, fresh.output_latency_frames),
            (0, 0)
        );
    }

    // ---- xrun classification (diagnostics) -------------------------------------------------

    #[test]
    fn is_overrun_means_render_longer_than_the_buffer_period() {
        let p = Duration::from_nanos(2_666_667); // 128 frames at 48 kHz = 2 666 666.7 ns
        assert!(!is_overrun(Duration::from_nanos(2_666_666), 128, 48_000));
        assert!(is_overrun(p, 128, 48_000));
        assert!(is_overrun(p * 2, 128, 48_000));
        assert!(
            !is_overrun(Duration::from_secs(1), 0, 48_000),
            "empty buffers are never judged"
        );
        assert!(!is_overrun(Duration::from_secs(1), 128, 0));
    }

    #[test]
    fn classify_regular_timeline_reports_nothing() {
        let mut d = XrunDetector::new();
        let t0 = origin();
        let p = period(128, 48_000);
        for i in 0..1000 {
            assert_eq!(d.classify(t0 + p * i, 128, 48_000, p / 10), None);
        }
        assert_eq!((d.late_callbacks(), d.render_overruns()), (0, 0));
    }

    #[test]
    fn classify_reports_a_render_overrun_with_its_gap() {
        let mut d = XrunDetector::new();
        let t0 = origin();
        let p = period(128, 48_000);
        d.classify(t0, 128, 48_000, p / 10);
        let kind = d.classify(t0 + p, 128, 48_000, p * 112 / 100);
        assert_eq!(kind, Some(XrunKind::RenderOverrun));
        assert_eq!(d.last_gap_frames(), 128);
        assert_eq!((d.late_callbacks(), d.render_overruns()), (0, 1));
    }

    #[test]
    fn classify_reports_a_late_callback_with_its_gap() {
        let mut d = XrunDetector::new();
        let t0 = origin();
        let p = period(128, 48_000);
        d.classify(t0, 128, 48_000, p / 10);
        assert_eq!(
            d.classify(t0 + p * 3, 128, 48_000, p / 10),
            Some(XrunKind::LateCallback)
        );
        assert_eq!(d.last_gap_frames(), 384);
        assert_eq!((d.late_callbacks(), d.render_overruns()), (1, 0));
    }

    #[test]
    fn a_late_callback_caused_by_the_previous_overrun_is_counted_once() {
        // Callback 1 renders for 2.2 periods; the host can only call back after it returned
        // and skips to the next period boundary (gap = 3 periods). That late callback is the
        // overrun's consequence: lateness is measured from the end of the overrunning render
        // (3 - 2.2 = 0.8 period, not late), so the pair counts as ONE xrun (render_overrun).
        let mut d = XrunDetector::new();
        let t0 = origin();
        let p = period(128, 48_000);
        d.classify(t0, 128, 48_000, p / 10);
        assert_eq!(
            d.classify(t0 + p, 128, 48_000, p * 22 / 10),
            Some(XrunKind::RenderOverrun)
        );
        assert_eq!(d.classify(t0 + p * 4, 128, 48_000, p / 10), None);
        assert_eq!(d.classify(t0 + p * 5, 128, 48_000, p / 10), None);
        assert_eq!((d.late_callbacks(), d.render_overruns()), (0, 1));
    }

    #[test]
    fn a_stall_beyond_what_the_overrun_explains_is_also_late() {
        let mut d = XrunDetector::new();
        let t0 = origin();
        let p = period(128, 48_000);
        d.classify(t0, 128, 48_000, p / 10);
        d.classify(t0 + p, 128, 48_000, p * 12 / 10); // overrun by 0.2 period
        // 10 periods later: 10 - 1.2 = 8.8 periods after the render ended, the driver stalled.
        assert_eq!(
            d.classify(t0 + p * 11, 128, 48_000, p / 10),
            Some(XrunKind::LateCallback)
        );
        assert_eq!((d.late_callbacks(), d.render_overruns()), (1, 1));
    }

    #[test]
    fn a_late_callback_that_also_overruns_counts_once_as_an_overrun() {
        let mut d = XrunDetector::new();
        let t0 = origin();
        let p = period(128, 48_000);
        d.classify(t0, 128, 48_000, p / 10);
        assert_eq!(
            d.classify(t0 + p * 3, 128, 48_000, p * 2),
            Some(XrunKind::RenderOverrun)
        );
        assert_eq!(
            d.last_gap_frames(),
            384,
            "the event still shows the late gap"
        );
        assert_eq!((d.late_callbacks(), d.render_overruns()), (0, 1));
    }

    #[test]
    fn telemetry_state_splits_xruns_by_kind_and_sums_them() {
        let state = TelemetryState::new();
        let mut d = XrunDetector::new();
        let t0 = origin();
        let p = period(128, 48_000);
        let renders = [1, 1, 30, 1, 1, 1, 1, 1]; // tenths of a period; index 2 overruns
        let mut t = t0;
        let mut kinds = Vec::new();
        for (i, r) in renders.into_iter().enumerate() {
            t += match i {
                3 => p * 3, // the host waits for the overrunning render (its consequence)
                5 => p * 4, // a driver stall
                _ => p,
            };
            kinds.push(state.on_callback(&mut d, t, 128, 48_000, p * r / 10, i as u64 * 10));
        }
        state.on_stream_error();
        let s = state.snapshot();
        assert_eq!(kinds[2], Some(XrunKind::RenderOverrun));
        assert_eq!(
            kinds[3], None,
            "the overrun's own late follow-up is not counted again"
        );
        assert_eq!(kinds[5], Some(XrunKind::LateCallback));
        assert_eq!(
            (s.late_callbacks, s.render_overruns, s.stream_errors),
            (1, 1, 1)
        );
        assert_eq!(s.xruns, 3, "xruns = late + overruns + errors");
        assert_eq!(s.max_barrier_spins, 70);
        let s = state.snapshot();
        assert_eq!(s.xruns, 3, "counters are totals");
        assert_eq!(s.max_barrier_spins, 0, "the spin max resets on read");
    }

    fn event(kind: XrunKind) -> XrunEvent {
        XrunEvent {
            at: Duration::from_millis(1500),
            kind,
            load: 1.119,
            gap_frames: 128,
            barrier_spins: 42,
            frames: 128,
        }
    }

    #[test]
    fn events_flow_through_the_channel_and_overflow_is_counted() {
        let state = TelemetryState::new();
        let (mut tx, mut rx) = event_channel();
        for _ in 0..XRUN_EVENT_CAPACITY {
            assert!(state.push_event(&mut tx, event(XrunKind::RenderOverrun)));
        }
        assert!(!state.push_event(&mut tx, event(XrunKind::LateCallback)));
        assert!(!state.push_event(&mut tx, event(XrunKind::LateCallback)));
        assert_eq!(state.snapshot().events_dropped, 2);
        assert_eq!(rx.pop().unwrap(), event(XrunKind::RenderOverrun));
        let mut n = 1;
        while rx.pop().is_ok() {
            n += 1;
        }
        assert_eq!(
            n, XRUN_EVENT_CAPACITY,
            "the oldest events are kept, new ones dropped"
        );
        assert!(state.push_event(&mut tx, event(XrunKind::StreamError)));
    }

    #[test]
    fn event_push_path_is_allocation_free_even_when_full() {
        let state = TelemetryState::new();
        let (mut tx, _rx) = event_channel();
        let mut d = XrunDetector::new();
        let t0 = origin();
        let p = period(128, 48_000);
        let total = XRUN_EVENT_CAPACITY as u32 + 10;
        atomik_testkit::no_alloc::assert_no_alloc(|| {
            for i in 0..total {
                let at = t0 + p * i * 3; // every render overruns and the host waits for it
                let render = p * 2;
                let kind = state.on_callback(&mut d, at, 128, 48_000, render, 5);
                let e = XrunEvent {
                    at: at.saturating_duration_since(t0),
                    kind: kind.unwrap_or(XrunKind::LateCallback),
                    load: LoadMeter::load(render, 128, 48_000),
                    gap_frames: d.last_gap_frames(),
                    barrier_spins: 5,
                    frames: 128,
                };
                state.push_event(&mut tx, e);
            }
            state.on_stream_error();
        });
        let s = state.snapshot();
        assert_eq!(s.render_overruns, u64::from(total));
        assert_eq!(s.events_dropped, 10);
    }

    #[test]
    fn xrun_kind_names_match_the_player_output() {
        assert_eq!(XrunKind::LateCallback.to_string(), "late_callback");
        assert_eq!(XrunKind::RenderOverrun.to_string(), "render_overrun");
        assert_eq!(XrunKind::StreamError.to_string(), "stream_error");
        assert_eq!(XrunKind::RenderOverrun.name(), "render_overrun");
    }
}
