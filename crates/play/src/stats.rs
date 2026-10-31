//! Statistics: the once-per-second `--stats` line, the run totals and the exit-code policy.

use atomik_audio_io::{CallbackStats, Telemetry, XrunEvent};

/// Exit code of a clean run.
pub const EXIT_OK: i32 = 0;
/// Exit code of a timed run (`--duration`) that had at least one xrun (fail).
pub const EXIT_XRUNS: i32 = 1;
/// Exit code of any error (arguments, patch, devices).
pub const EXIT_FAILURE: i32 = 2;

/// One `--stats` line, printed once per second:
/// `t=12s cb=4500 buf=128 xruns=0 dsp=avg 3.1% peak 7.9% midi_dropped=0 events_dropped=0`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StatsLine {
    /// Seconds since the stream started.
    pub seconds: u64,
    /// Callbacks executed so far.
    pub callbacks: u64,
    /// Buffer size of the latest callback, in frames.
    pub buffer_frames: u32,
    /// Xruns so far.
    pub xruns: u64,
    /// Average DSP load over the last second (1.0 = 100 %).
    pub dsp_avg: f32,
    /// Peak DSP load over the last second (1.0 = 100 %).
    pub dsp_peak: f32,
    /// MIDI notes dropped because the live queue was full.
    pub midi_dropped: u64,
    /// Note events the engine dropped (queue overflow or undeliverable).
    pub events_dropped: u64,
}

impl std::fmt::Display for StatsLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "t={}s cb={} buf={} xruns={} dsp=avg {:.1}% peak {:.1}% midi_dropped={} events_dropped={}",
            self.seconds,
            self.callbacks,
            self.buffer_frames,
            self.xruns,
            percent(self.dsp_avg),
            percent(self.dsp_peak),
            self.midi_dropped,
            self.events_dropped
        )
    }
}

/// Totals over a whole run, fed with one [`Telemetry`] snapshot per second.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RunTotals {
    /// Snapshots recorded (seconds elapsed).
    pub seconds: u64,
    /// Total callbacks.
    pub callbacks: u64,
    /// Total xruns.
    pub xruns: u64,
    /// Latest buffer size in frames.
    pub buffer_frames: u32,
    /// Highest DSP load seen in any snapshot.
    pub dsp_peak: f32,
    /// Latest snapshot's average DSP load.
    pub dsp_avg: f32,
    /// Latest snapshot's peak DSP load.
    pub last_peak: f32,
    /// MIDI notes dropped (latest count).
    pub midi_dropped: u64,
    /// Engine note events dropped (latest count).
    pub events_dropped: u64,
    /// Xruns classified as late callbacks (driver or OS), total.
    pub late_callbacks: u64,
    /// Xruns classified as render overruns (render longer than the period), total.
    pub render_overruns: u64,
    /// Stream errors, total.
    pub stream_errors: u64,
    /// Highest engine level-barrier wait of any callback in the run, in spins.
    pub max_barrier_spins: u64,
    /// Latest callback time and barrier-wait histograms (totals since the stream started).
    pub callback: CallbackStats,
}

impl RunTotals {
    /// Records one snapshot. `elapsed_seconds` is the stream's age; counters in `t` are totals,
    /// loads cover the time since the previous snapshot.
    pub fn record(&mut self, t: Telemetry, elapsed_seconds: u64, midi: u64, events: u64) {
        self.seconds = elapsed_seconds;
        self.callbacks = t.callbacks;
        self.xruns = t.xruns;
        if t.buffer_frames > 0 {
            self.buffer_frames = t.buffer_frames;
        }
        self.dsp_avg = t.dsp_load_avg;
        self.last_peak = t.dsp_load_peak;
        self.dsp_peak = self.dsp_peak.max(t.dsp_load_peak);
        self.midi_dropped = midi;
        self.events_dropped = events;
        self.late_callbacks = t.late_callbacks;
        self.render_overruns = t.render_overruns;
        self.stream_errors = t.stream_errors;
        self.max_barrier_spins = self.max_barrier_spins.max(t.max_barrier_spins);
        self.callback = t.callback;
    }

    /// The `--stats` line for the latest snapshot.
    pub fn line(&self) -> StatsLine {
        StatsLine {
            seconds: self.seconds,
            callbacks: self.callbacks,
            buffer_frames: self.buffer_frames,
            xruns: self.xruns,
            dsp_avg: self.dsp_avg,
            dsp_peak: self.last_peak,
            midi_dropped: self.midi_dropped,
            events_dropped: self.events_dropped,
        }
    }

    /// The final summary printed when the run stops. The xrun split (`late`, `overruns`,
    /// `errors`, whose sum is `xruns`), `max_barrier_spins` and the callback-time percentiles
    /// (`cb_p50_us` .. `cb_max_us`, `spins_p99`) come last, before `result=`.
    pub fn summary(&self, duration: Option<u64>) -> String {
        let result = match duration {
            None => "stopped",
            Some(_) if self.xruns > 0 => "FAIL",
            Some(_) => "PASS",
        };
        format!(
            "summary: t={}s callbacks={} buf={} xruns={} peak_dsp={:.1}% midi_dropped={} events_dropped={} late={} overruns={} errors={} max_barrier_spins={} {} result={result}",
            self.seconds,
            self.callbacks,
            self.buffer_frames,
            self.xruns,
            percent(self.dsp_peak),
            self.midi_dropped,
            self.events_dropped,
            self.late_callbacks,
            self.render_overruns,
            self.stream_errors,
            self.max_barrier_spins,
            self.callback.summary_fields()
        )
    }
}

/// One xrun event line (printed with `--stats` as soon as the event is drained):
/// `xrun t=123.457s kind=render_overrun load=111.9% gap=128 frames=128 barrier_spins=48213`.
///
/// `t` is the time since the stream started, `load` the callback's DSP load, `gap` the frames
/// since the previous callback started, `frames` the callback's buffer size and
/// `barrier_spins` the engine's busy-wait for its workers during that callback.
pub fn event_line(e: &XrunEvent) -> String {
    format!(
        "xrun t={:.3}s kind={} load={:.1}% gap={} frames={} barrier_spins={}",
        e.at.as_secs_f64(),
        e.kind,
        percent(e.load),
        e.gap_frames,
        e.frames,
        e.barrier_spins
    )
}

/// Exit code policy: [`EXIT_XRUNS`] when a timed run (`--duration`) had any xrun (the
/// pass/fail signal); otherwise [`EXIT_OK`]. Runs stopped with Ctrl-C only (no duration) always
/// succeed.
pub fn exit_code(xruns: u64, duration: Option<u64>) -> i32 {
    if xruns > 0 && duration.is_some() {
        EXIT_XRUNS
    } else {
        EXIT_OK
    }
}

/// A load fraction as a percentage (1.0 → 100.0).
fn percent(load: f32) -> f64 {
    f64::from(load) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_audio_io::XrunKind;
    use std::time::Duration;

    #[test]
    fn stats_line_has_the_documented_format() {
        let line = StatsLine {
            seconds: 12,
            callbacks: 4500,
            buffer_frames: 128,
            xruns: 0,
            dsp_avg: 0.031,
            dsp_peak: 0.079,
            midi_dropped: 0,
            events_dropped: 0,
        };
        assert_eq!(
            line.to_string(),
            "t=12s cb=4500 buf=128 xruns=0 dsp=avg 3.1% peak 7.9% midi_dropped=0 events_dropped=0"
        );
    }

    #[test]
    fn stats_line_rounds_loads_to_one_decimal_and_shows_counts() {
        let line = StatsLine {
            seconds: 1,
            callbacks: 375,
            buffer_frames: 480,
            xruns: 3,
            dsp_avg: 0.12345,
            dsp_peak: 1.5,
            midi_dropped: 2,
            events_dropped: 7,
        };
        assert_eq!(
            line.to_string(),
            "t=1s cb=375 buf=480 xruns=3 dsp=avg 12.3% peak 150.0% midi_dropped=2 events_dropped=7"
        );
    }

    fn tel(callbacks: u64, xruns: u64, avg: f32, peak: f32) -> Telemetry {
        Telemetry {
            callbacks,
            xruns,
            buffer_frames: 128,
            dsp_load_avg: avg,
            dsp_load_peak: peak,
            ..Telemetry::default()
        }
    }

    #[test]
    fn totals_keep_the_highest_peak_and_latest_counters() {
        let mut t = RunTotals::default();
        t.record(tel(375, 0, 0.02, 0.05), 1, 0, 0);
        t.record(tel(750, 1, 0.03, 0.30), 2, 1, 2);
        t.record(tel(1125, 1, 0.01, 0.04), 3, 1, 2);
        assert_eq!(t.seconds, 3);
        assert_eq!((t.callbacks, t.xruns, t.buffer_frames), (1125, 1, 128));
        assert!((t.dsp_peak - 0.30).abs() < 1e-6);
        assert!((t.dsp_avg - 0.01).abs() < 1e-6);
        let line = t.line();
        assert_eq!(line.seconds, 3);
        assert!(
            (line.dsp_peak - 0.04).abs() < 1e-6,
            "the line shows this second's peak"
        );
        assert_eq!((line.midi_dropped, line.events_dropped), (1, 2));
    }

    #[test]
    fn summary_reports_totals_and_verdict() {
        let mut t = RunTotals::default();
        t.record(tel(1875, 0, 0.02, 0.079), 5, 0, 0);
        let s = t.summary(Some(5));
        assert_eq!(
            s,
            "summary: t=5s callbacks=1875 buf=128 xruns=0 peak_dsp=7.9% midi_dropped=0 \
             events_dropped=0 late=0 overruns=0 errors=0 max_barrier_spins=0 \
             cb_p50_us=0.0 cb_p99_us=0.0 cb_p999_us=0.0 cb_max_us=0.0 spins_p99=0 result=PASS"
        );
        t.record(tel(2250, 2, 0.02, 0.05), 6, 0, 0);
        let failed = t.summary(Some(6));
        assert!(failed.contains(" xruns=2 peak_dsp=7.9% "), "{failed}");
        assert!(failed.ends_with(" result=FAIL"), "{failed}");
        assert!(
            t.summary(None).ends_with("result=stopped"),
            "{}",
            t.summary(None)
        );
    }

    #[test]
    fn exit_code_policy() {
        assert_eq!(exit_code(0, None), EXIT_OK);
        assert_eq!(exit_code(0, Some(600)), EXIT_OK);
        assert_eq!(exit_code(1, Some(600)), EXIT_XRUNS);
        assert_eq!(
            exit_code(7, None),
            EXIT_OK,
            "Ctrl-C runs have no pass/fail verdict"
        );
        assert_eq!((EXIT_OK, EXIT_XRUNS, EXIT_FAILURE), (0, 1, 2));
    }

    #[test]
    fn summary_appends_the_xrun_split_before_the_verdict() {
        let mut t = RunTotals::default();
        t.record(
            Telemetry {
                late_callbacks: 3,
                render_overruns: 37,
                stream_errors: 1,
                max_barrier_spins: 900,
                ..tel(225_000, 41, 0.05, 1.119)
            },
            599,
            0,
            0,
        );
        t.record(
            Telemetry {
                late_callbacks: 3,
                render_overruns: 37,
                stream_errors: 1,
                max_barrier_spins: 12, // per-snapshot max; the run keeps the highest
                ..tel(225_768, 41, 0.05, 0.08)
            },
            600,
            0,
            0,
        );
        assert_eq!(
            (t.late_callbacks, t.render_overruns, t.stream_errors),
            (3, 37, 1)
        );
        assert_eq!(t.max_barrier_spins, 900);
        assert_eq!(
            t.summary(Some(600)),
            "summary: t=600s callbacks=225768 buf=128 xruns=41 peak_dsp=111.9% midi_dropped=0 \
             events_dropped=0 late=3 overruns=37 errors=1 max_barrier_spins=900 \
             cb_p50_us=0.0 cb_p99_us=0.0 cb_p999_us=0.0 cb_max_us=0.0 spins_p99=0 result=FAIL"
        );
    }

    #[test]
    fn summary_prints_the_callback_percentiles_of_the_latest_snapshot() {
        let mut callback = CallbackStats::default();
        callback.time[atomik_audio_io::time_bucket(200_000)] = 99;
        callback.time[atomik_audio_io::time_bucket(900_000)] = 1;
        callback.spins[atomik_audio_io::spin_bucket(5)] = 100;
        callback.max_ns = 900_000;
        callback.max_spins = 5;
        let mut t = RunTotals::default();
        t.record(
            Telemetry {
                callback,
                ..tel(100, 0, 0.1, 0.2)
            },
            1,
            0,
            0,
        );
        let s = t.summary(Some(1));
        assert!(
            s.contains(" cb_p50_us=2") && s.contains(" cb_max_us=900.0 spins_p99=5 result=PASS"),
            "{s}"
        );
    }

    #[test]
    fn xrun_event_line_has_the_documented_format() {
        let e = XrunEvent {
            at: Duration::from_micros(123_456_789),
            kind: XrunKind::RenderOverrun,
            load: 1.119,
            gap_frames: 128,
            barrier_spins: 48_213,
            frames: 128,
        };
        assert_eq!(
            event_line(&e),
            "xrun t=123.457s kind=render_overrun load=111.9% gap=128 frames=128 \
             barrier_spins=48213"
        );
        let e = XrunEvent {
            at: Duration::from_millis(2_500),
            kind: XrunKind::StreamError,
            load: 0.0,
            gap_frames: 0,
            barrier_spins: 0,
            frames: 0,
        };
        assert_eq!(
            event_line(&e),
            "xrun t=2.500s kind=stream_error load=0.0% gap=0 frames=0 barrier_spins=0"
        );
    }
}
