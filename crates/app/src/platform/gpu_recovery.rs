//! What the app does when the graphics device is lost, without a GPU: the back-off between
//! attempts to build the graphics again, the largest size a surface may have, the switch that
//! forces a loss for testing, and the small state machine that turns "the device is lost" into
//! "tear down now" and "rebuild now".
//!
//! The platform layer ([`super::gpu`], [`super::app`]) feeds it the flag the graphics driver's
//! callbacks set and the time since the app started, and does what it says. Time is a plain
//! `Duration` since the start, so everything here is deterministic and unit-tested.

use std::time::Duration;

/// The waits between attempts to rebuild the graphics after one failed: 0.5 s, 1 s, 2 s, then
/// every 5 s for as long as it takes.
const BACKOFF: [Duration; 3] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
];
/// The wait once the schedule is used up.
const BACKOFF_LAST: Duration = Duration::from_secs(5);
/// A rebuild that held this long without a new loss starts the schedule over.
const STABLE_FOR: Duration = Duration::from_secs(10);
/// The shortest period the forced-loss switch accepts (a rebuild takes a moment).
const MIN_FORCED_PERIOD: Duration = Duration::from_secs(1);

/// The environment variable that forces a graphics device loss every this many seconds, for
/// testing the recovery without a driver reset (`ATOMIK_GPU_LOSS_EVERY=8`).
pub const FORCE_LOSS_ENV: &str = "ATOMIK_GPU_LOSS_EVERY";

/// How long to wait before attempt number `attempt` (1 is the first). The first attempt runs at
/// once; each next one waits for the schedule: 0.5 s, 1 s, 2 s, 5 s, 5 s, ...
#[must_use]
pub fn wait_before(attempt: u32) -> Duration {
    match attempt {
        0 | 1 => Duration::ZERO,
        n => BACKOFF.get(n as usize - 2).copied().unwrap_or(BACKOFF_LAST),
    }
}

/// The size to configure a surface or render target with: each side at least 1 and at most
/// `max` (the device's `max_texture_dimension_2d`).
#[must_use]
pub fn clamp_size(width: u32, height: u32, max: u32) -> (u32, u32) {
    let max = max.max(1);
    (width.clamp(1, max), height.clamp(1, max))
}

/// The period of the forced-loss switch from its text: a number of seconds, at least one;
/// anything else (unset, empty, not a number, zero, negative) leaves the switch off.
#[must_use]
pub fn forced_period(text: Option<&str>) -> Option<Duration> {
    let seconds: f64 = text?.trim().parse().ok()?;
    if !seconds.is_finite() || seconds <= 0.0 || seconds > 86_400.0 {
        return None;
    }
    Some(Duration::from_secs_f64(seconds).max(MIN_FORCED_PERIOD))
}

/// What to do now about the graphics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Decision {
    /// The loss was noticed by this call: drop every surface and device-bound resource now.
    pub teardown: bool,
    /// Build the graphics again now.
    pub rebuild: bool,
}

/// The loss and recovery of the graphics: one value per app.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovery {
    /// The graphics are torn down or about to be, and wait for a rebuild.
    down: bool,
    /// Attempts since the graphics last held for [`STABLE_FOR`] (a success counts: a loss soon
    /// after a rebuild waits like a retry, so a device that keeps failing cannot spin the app).
    attempts: u32,
    /// The time since the start before which no attempt is made.
    not_before: Duration,
    /// When the last rebuild worked.
    last_ok: Option<Duration>,
    /// The recovery snapshot was asked for since the graphics last worked.
    snapshot_asked: bool,
}

impl Recovery {
    /// Whether the graphics are down (lost, and not rebuilt yet).
    #[must_use]
    pub fn is_down(&self) -> bool {
        self.down
    }

    /// How many attempts were made in the current run of failures and quick re-losses.
    #[must_use]
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Decides at `now` (time since the start) given whether the graphics report the device as
    /// lost. Call it at the start of every frame; it is cheap.
    pub fn poll(&mut self, now: Duration, lost: bool) -> Decision {
        if !self.down {
            if self.attempts > 0
                && self
                    .last_ok
                    .is_some_and(|t| now.saturating_sub(t) >= STABLE_FOR)
            {
                self.attempts = 0;
            }
            if !lost {
                return Decision::default();
            }
            self.down = true;
            self.not_before = now + wait_before(self.attempts + 1);
            return Decision {
                teardown: true,
                rebuild: self.start_attempt(now),
            };
        }
        Decision {
            teardown: false,
            rebuild: self.start_attempt(now),
        }
    }

    /// Counts an attempt when one is due at `now`.
    fn start_attempt(&mut self, now: Duration) -> bool {
        if now < self.not_before {
            return false;
        }
        self.attempts += 1;
        true
    }

    /// The attempt that [`poll`](Self::poll) asked for ended at `now`: `ok` when the graphics are
    /// back. Returns whether a recovery snapshot of the document is to be asked for (the first
    /// failure only).
    pub fn finished(&mut self, now: Duration, ok: bool) -> bool {
        if ok {
            self.down = false;
            self.last_ok = Some(now);
            self.snapshot_asked = false;
            return false;
        }
        self.not_before = now + wait_before(self.attempts + 1);
        !std::mem::replace(&mut self.snapshot_asked, true)
    }

    /// When the next attempt is due, while the graphics are down.
    #[must_use]
    pub fn next_attempt(&self) -> Option<Duration> {
        self.down.then_some(self.not_before)
    }
}

/// The forced-loss switch: the period and when it fires next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForcedLoss {
    period: Duration,
    next: Duration,
}

impl ForcedLoss {
    /// The first loss comes one period after `now`.
    #[must_use]
    pub fn new(period: Duration, now: Duration) -> Self {
        Self {
            period,
            next: now + period,
        }
    }

    /// Whether a loss is to be forced at `now`; the following one is one period later.
    pub fn due(&mut self, now: Duration) -> bool {
        if now < self.next {
            return false;
        }
        self.next = now + self.period;
        true
    }

    /// When the next loss is forced.
    #[must_use]
    pub fn next(&self) -> Duration {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn the_backoff_is_immediate_then_half_one_two_then_five_seconds() {
        let waits: Vec<_> = (0..8).map(wait_before).collect();
        assert_eq!(
            waits,
            [
                Duration::ZERO,
                Duration::ZERO,
                Duration::from_millis(500),
                s(1),
                s(2),
                s(5),
                s(5),
                s(5)
            ]
        );
    }

    #[test]
    fn a_size_is_held_inside_one_and_the_device_limit() {
        assert_eq!(clamp_size(1280, 860, 8192), (1280, 860));
        assert_eq!(clamp_size(20_000, 860, 8192), (8192, 860));
        assert_eq!(clamp_size(100, 9000, 8192), (100, 8192));
        assert_eq!(clamp_size(0, 0, 8192), (1, 1));
        assert_eq!(clamp_size(5, 5, 0), (1, 1));
    }

    #[test]
    fn the_forced_period_reads_seconds_and_refuses_the_rest() {
        assert_eq!(forced_period(Some("8")), Some(s(8)));
        assert_eq!(
            forced_period(Some(" 2.5 ")),
            Some(Duration::from_millis(2500))
        );
        assert_eq!(forced_period(Some("0.1")), Some(s(1)));
        for bad in ["", "0", "-3", "abc", "NaN", "inf", "1e9"] {
            assert_eq!(forced_period(Some(bad)), None, "{bad}");
        }
        assert_eq!(forced_period(None), None);
    }

    #[test]
    fn nothing_happens_while_the_device_is_fine() {
        let mut r = Recovery::default();
        for t in 0..5 {
            assert_eq!(r.poll(s(t), false), Decision::default());
        }
        assert!(!r.is_down());
    }

    #[test]
    fn the_first_loss_tears_down_and_rebuilds_at_once() {
        let mut r = Recovery::default();
        let d = r.poll(s(3), true);
        assert_eq!(
            d,
            Decision {
                teardown: true,
                rebuild: true
            }
        );
        assert!(r.is_down());
        assert!(!r.finished(s(3), true));
        assert!(!r.is_down());
        assert_eq!(r.next_attempt(), None);
    }

    #[test]
    fn failures_retry_on_the_schedule_and_ask_for_one_snapshot() {
        let mut r = Recovery::default();
        assert!(r.poll(s(0), true).rebuild);
        // First failure: the snapshot is asked for, the next try is half a second away.
        assert!(r.finished(s(0), false));
        assert_eq!(r.next_attempt(), Some(Duration::from_millis(500)));
        assert!(!r.poll(Duration::from_millis(499), true).rebuild);
        assert!(r.poll(Duration::from_millis(500), true).rebuild);
        // Second failure: no second snapshot; the wait is one second.
        assert!(!r.finished(Duration::from_millis(500), false));
        assert_eq!(r.next_attempt(), Some(Duration::from_millis(1500)));
        assert!(r.poll(Duration::from_millis(1500), true).rebuild);
        assert!(!r.finished(Duration::from_millis(1500), false));
        assert_eq!(r.next_attempt(), Some(Duration::from_millis(3500)));
        assert!(r.poll(Duration::from_millis(3500), true).rebuild);
        assert!(!r.finished(Duration::from_millis(3500), false));
        // Then every five seconds, as long as it takes.
        assert_eq!(r.next_attempt(), Some(Duration::from_millis(8500)));
        assert!(r.poll(Duration::from_millis(8500), true).rebuild);
        assert!(!r.finished(Duration::from_millis(8500), false));
        assert_eq!(r.next_attempt(), Some(Duration::from_millis(13_500)));
        // The loss was noticed once: only the first call tears down.
        assert!(!r.poll(Duration::from_millis(9000), true).teardown);
    }

    #[test]
    fn a_new_snapshot_is_asked_for_after_the_graphics_worked_again() {
        let mut r = Recovery::default();
        r.poll(s(0), true);
        assert!(r.finished(s(0), false));
        r.poll(s(1), true);
        assert!(!r.finished(s(1), true));
        // Lost again, long after: a fresh run, immediate rebuild, and its first failure asks again.
        let d = r.poll(s(100), true);
        assert!(d.teardown && d.rebuild);
        assert!(r.finished(s(100), false));
    }

    #[test]
    fn a_loss_soon_after_a_rebuild_waits_instead_of_spinning() {
        let mut r = Recovery::default();
        assert!(r.poll(s(10), true).rebuild);
        r.finished(s(10), true);
        // Lost again a moment later: torn down at once, rebuilt half a second after.
        let d = r.poll(Duration::from_millis(10_100), true);
        assert!(d.teardown);
        assert!(!d.rebuild);
        assert_eq!(r.next_attempt(), Some(Duration::from_millis(10_600)));
        assert!(r.poll(Duration::from_millis(10_600), true).rebuild);
        r.finished(Duration::from_millis(10_600), true);
        // The next quick loss waits a second, the one after two.
        let d = r.poll(Duration::from_millis(10_700), true);
        assert!(d.teardown && !d.rebuild);
        assert_eq!(r.next_attempt(), Some(Duration::from_millis(11_700)));
    }

    #[test]
    fn ten_quiet_seconds_start_the_schedule_over() {
        let mut r = Recovery::default();
        r.poll(s(0), true);
        r.finished(s(0), true);
        assert_eq!(r.attempts(), 1);
        assert_eq!(r.poll(s(5), false), Decision::default());
        assert_eq!(r.attempts(), 1);
        assert_eq!(r.poll(s(10), false), Decision::default());
        assert_eq!(r.attempts(), 0);
        assert!(r.poll(s(11), true).rebuild);
    }

    #[test]
    fn the_forced_loss_fires_once_per_period() {
        let mut f = ForcedLoss::new(s(8), s(2));
        assert_eq!(f.next(), s(10));
        assert!(!f.due(s(9)));
        assert!(f.due(s(10)));
        assert!(!f.due(s(17)));
        assert!(f.due(s(18)));
        // A long stall fires once, not once per missed period.
        assert!(f.due(s(100)));
        assert!(!f.due(s(101)));
    }
}
