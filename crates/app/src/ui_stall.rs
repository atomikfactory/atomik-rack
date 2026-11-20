//! What blocks the UI thread: the counters of the stall lines in the log and the rate limit of
//! the frame line.
//!
//! Two things are recorded. A blocking call on a plugin's process that took at least
//! [`atomik_plugin_host::STALL_THRESHOLD`] is written as `ui stall <ms> ms: <call> [<device>]
//! (<outcome>)`, and a whole UI frame (the platform's per-tick work, or one window's redraw) that
//! took at least [`FRAME_STALL`] as `ui stall <ms> ms: frame`. Both are warnings in the log file
//! and the plugin ones also go to the in-app plugin log. The counts and the longest of each, since
//! the app started, are in Help > Export Diagnostics ([`snapshot`]).
//!
//! Nothing here runs on the audio thread. The counters are process-wide relaxed atomics: the UI
//! thread writes them and the diagnostics report reads them.

use atomik_plugin_host::StallOutcome;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long a whole UI frame may take before it is logged as a stall (`ui stall 180 ms: frame`).
pub const FRAME_STALL: Duration = Duration::from_millis(100);

/// The most often a frame stall line is written; the stalls in between are counted and named in
/// the next line.
pub const FRAME_LINE_PERIOD: Duration = Duration::from_secs(1);

static PLUGIN_STALLS: AtomicU64 = AtomicU64::new(0);
static PLUGIN_LONGEST_MS: AtomicU64 = AtomicU64::new(0);
static SLOW_FRAMES: AtomicU64 = AtomicU64::new(0);
static FRAME_LONGEST_MS: AtomicU64 = AtomicU64::new(0);

/// The stall counters since the app started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StallStats {
    /// Plugin calls that held the UI thread up.
    pub plugin_stalls: u64,
    /// The longest of them, in milliseconds.
    pub plugin_longest_ms: u64,
    /// UI frames that were slow.
    pub slow_frames: u64,
    /// The longest of them, in milliseconds.
    pub frame_longest_ms: u64,
}

/// The counters now.
#[must_use]
pub fn snapshot() -> StallStats {
    StallStats {
        plugin_stalls: PLUGIN_STALLS.load(Ordering::Relaxed),
        plugin_longest_ms: PLUGIN_LONGEST_MS.load(Ordering::Relaxed),
        slow_frames: SLOW_FRAMES.load(Ordering::Relaxed),
        frame_longest_ms: FRAME_LONGEST_MS.load(Ordering::Relaxed),
    }
}

/// Counts a plugin call that held the UI thread up for `ms`.
pub fn count_plugin_stall(ms: u64) {
    PLUGIN_STALLS.fetch_add(1, Ordering::Relaxed);
    PLUGIN_LONGEST_MS.fetch_max(ms, Ordering::Relaxed);
}

/// The log line of a plugin call that held the UI thread up.
#[must_use]
pub fn plugin_stall_line(call: &str, ms: u64, device: &str, outcome: StallOutcome) -> String {
    format!("ui stall {ms} ms: {call} [{device}] ({})", outcome.label())
}

/// Decides which slow frames are written to the log.
#[derive(Debug, Default)]
pub struct FrameWatch {
    last_line: Option<Instant>,
    suppressed: u64,
}

impl FrameWatch {
    /// A frame (or redraw) took `took`, ending at `now`. Counts it when it is slow and returns
    /// the line to write, if one is due: at most one per [`FRAME_LINE_PERIOD`], naming how many
    /// slow frames the ones before it hid.
    pub fn note(&mut self, took: Duration, now: Instant) -> Option<String> {
        if took < FRAME_STALL {
            return None;
        }
        let ms = u64::try_from(took.as_millis()).unwrap_or(u64::MAX);
        SLOW_FRAMES.fetch_add(1, Ordering::Relaxed);
        FRAME_LONGEST_MS.fetch_max(ms, Ordering::Relaxed);
        if self
            .last_line
            .is_some_and(|t| now.saturating_duration_since(t) < FRAME_LINE_PERIOD)
        {
            self.suppressed += 1;
            return None;
        }
        self.last_line = Some(now);
        let hidden = std::mem::take(&mut self.suppressed);
        Some(if hidden == 0 {
            format!("ui stall {ms} ms: frame")
        } else {
            format!("ui stall {ms} ms: frame ({hidden} more since the last line)")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::disallowed_methods, reason = "a test clock")]
    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn the_plugin_line_has_the_shape_of_the_log() {
        assert_eq!(
            plugin_stall_line("CaptureState", 10_012, "m1", StallOutcome::TimedOut),
            "ui stall 10012 ms: CaptureState [m1] (timed out)"
        );
        assert_eq!(
            plugin_stall_line("ReadParams", 61, "m1", StallOutcome::ChildGone),
            "ui stall 61 ms: ReadParams [m1] (child gone)"
        );
    }

    #[test]
    fn a_fast_frame_is_not_a_stall_and_slow_ones_are_rate_limited_with_a_count() {
        let mut w = FrameWatch::default();
        let t = t0();
        let before = snapshot();
        assert_eq!(w.note(Duration::from_millis(99), t), None);
        assert_eq!(
            w.note(Duration::from_millis(180), t).as_deref(),
            Some("ui stall 180 ms: frame")
        );
        assert_eq!(
            w.note(Duration::from_millis(150), t + Duration::from_millis(200)),
            None
        );
        assert_eq!(
            w.note(Duration::from_millis(300), t + Duration::from_millis(900)),
            None
        );
        assert_eq!(
            w.note(Duration::from_millis(120), t + Duration::from_millis(1_100))
                .as_deref(),
            Some("ui stall 120 ms: frame (2 more since the last line)")
        );
        let after = snapshot();
        assert!(after.slow_frames >= before.slow_frames + 4);
        assert!(after.frame_longest_ms >= 300);
    }

    #[test]
    fn plugin_stalls_are_counted_with_the_longest() {
        let before = snapshot();
        count_plugin_stall(70);
        count_plugin_stall(10_012);
        let after = snapshot();
        assert!(after.plugin_stalls >= before.plugin_stalls + 2);
        assert!(after.plugin_longest_ms >= 10_012);
    }
}
