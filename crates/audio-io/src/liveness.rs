//! Stream liveness: tells a stream that keeps calling back from one that has gone quiet.
//!
//! The audio callback already counts itself ([`crate::Telemetry::callbacks`]). A device that is
//! unplugged mid-stream, or a driver that stops delivering buffers, leaves that counter standing
//! still while the engine looks like it is running. [`Liveness`] is fed the counter and the time
//! by the control thread, a few times a second, and says which of the cases applies. It is pure
//! (no clock of its own, no allocation) so a fake stream drives it in tests.

use crate::Telemetry;
use std::time::{Duration, Instant};

/// How many buffer periods without a callback make a stream stalled.
pub const STALL_PERIODS: u32 = 4;

/// The shortest silence reported as a stall, whatever the buffer size: scheduling hiccups of a
/// busy machine are shorter than this.
pub const STALL_MIN: Duration = Duration::from_millis(250);

/// How long a stream that has not called back yet gets to start (drivers can take a while to
/// open) before it is reported as stalled.
pub const START_GRACE: Duration = Duration::from_secs(2);

/// What a stream is doing, as seen from the control thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamState {
    /// No stream is open, or it has not been observed yet.
    #[default]
    Off,
    /// The stream is open and no callback has arrived yet.
    Starting,
    /// Callbacks arrive.
    Running,
    /// No callback arrived for more than [`stall_after`]. The state clears by itself when the
    /// callbacks come back.
    Stalled,
    /// The backend reported the device gone (unplugged, or the stream became invalid). Final
    /// until the stream is rebuilt.
    Lost,
}

impl StreamState {
    /// True for [`StreamState::Stalled`] and [`StreamState::Lost`]: the engine is not
    /// producing audio although it should.
    pub fn is_dead(self) -> bool {
        matches!(self, Self::Stalled | Self::Lost)
    }
}

/// The silence after which a stream of `buffer_frames` at `sample_rate` is stalled:
/// `max(4 periods, 250 ms)`. An unknown buffer size (0) gets the minimum.
pub fn stall_after(buffer_frames: u32, sample_rate: u32) -> Duration {
    let period = if sample_rate == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(f64::from(buffer_frames) / f64::from(sample_rate))
    };
    (period * STALL_PERIODS).max(STALL_MIN)
}

/// Watches the callback counter of one stream.
#[derive(Debug, Clone, Copy)]
pub struct Liveness {
    opened: Instant,
    callbacks: u64,
    /// When the counter last moved (or the stream opened).
    changed: Instant,
}

impl Liveness {
    /// Starts watching a stream opened at `now`.
    pub fn new(now: Instant) -> Self {
        Self {
            opened: now,
            callbacks: 0,
            changed: now,
        }
    }

    /// The state at `now` given the stream's latest telemetry and sample rate (the buffer size
    /// is the telemetry's).
    pub fn observe(
        &mut self,
        now: Instant,
        telemetry: &Telemetry,
        sample_rate: u32,
    ) -> StreamState {
        if telemetry.callbacks != self.callbacks {
            self.callbacks = telemetry.callbacks;
            self.changed = now;
        }
        if telemetry.device_lost {
            return StreamState::Lost;
        }
        let silent = now.saturating_duration_since(self.changed);
        if self.callbacks == 0 {
            let waited = now.saturating_duration_since(self.opened);
            return if waited > START_GRACE {
                StreamState::Stalled
            } else {
                StreamState::Starting
            };
        }
        if silent > stall_after(telemetry.buffer_frames, sample_rate) {
            StreamState::Stalled
        } else {
            StreamState::Running
        }
    }

    /// How long the callbacks have been silent, as far as the observations show.
    pub fn silent_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::disallowed_methods)] // Justification: test-only origin of a simulated timeline.
    fn origin() -> Instant {
        Instant::now()
    }

    fn tel(callbacks: u64) -> Telemetry {
        Telemetry {
            callbacks,
            buffer_frames: 480,
            ..Telemetry::default()
        }
    }

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn the_threshold_is_four_periods_but_at_least_250_ms() {
        assert_eq!(stall_after(480, 48_000), STALL_MIN);
        assert_eq!(stall_after(0, 48_000), STALL_MIN);
        assert_eq!(stall_after(480, 0), STALL_MIN);
        // 2048 frames at 44.1 kHz is 46.4 ms, four of them 185.8 ms: still the minimum.
        assert_eq!(stall_after(2048, 44_100), STALL_MIN);
        // 8192 frames at 44.1 kHz: 4 periods is 743 ms.
        let long = stall_after(8192, 44_100);
        assert!(long > MS(740) && long < MS(745), "{long:?}");
    }

    #[test]
    fn a_stream_that_keeps_calling_back_is_running() {
        let t0 = origin();
        let mut live = Liveness::new(t0);
        assert_eq!(live.observe(t0, &tel(0), 48_000), StreamState::Starting);
        for i in 1..=20u64 {
            let now = t0 + MS(100 * i);
            assert_eq!(
                live.observe(now, &tel(i * 10), 48_000),
                StreamState::Running
            );
        }
    }

    #[test]
    fn stalled_stream_is_reported() {
        let t0 = origin();
        let mut live = Liveness::new(t0);
        assert_eq!(
            live.observe(t0 + MS(10), &tel(5), 48_000),
            StreamState::Running
        );
        // The counter stands still: running until the threshold, then stalled.
        assert_eq!(
            live.observe(t0 + MS(200), &tel(5), 48_000),
            StreamState::Running
        );
        assert_eq!(
            live.observe(t0 + MS(259), &tel(5), 48_000),
            StreamState::Running
        );
        assert_eq!(
            live.observe(t0 + MS(261), &tel(5), 48_000),
            StreamState::Stalled
        );
        assert!(live.silent_for(t0 + MS(300)) > MS(250));
        // The callbacks come back: running again.
        assert_eq!(
            live.observe(t0 + MS(400), &tel(6), 48_000),
            StreamState::Running
        );
    }

    #[test]
    fn a_stream_that_never_starts_is_stalled_after_the_grace() {
        let t0 = origin();
        let mut live = Liveness::new(t0);
        assert_eq!(
            live.observe(t0 + MS(1900), &tel(0), 48_000),
            StreamState::Starting
        );
        assert_eq!(
            live.observe(t0 + MS(2100), &tel(0), 48_000),
            StreamState::Stalled
        );
    }

    #[test]
    fn a_lost_device_is_reported_at_once() {
        let t0 = origin();
        let mut live = Liveness::new(t0);
        let lost = Telemetry {
            device_lost: true,
            ..tel(50)
        };
        assert_eq!(live.observe(t0 + MS(10), &lost, 48_000), StreamState::Lost);
        assert!(StreamState::Lost.is_dead() && StreamState::Stalled.is_dead());
        assert!(!StreamState::Running.is_dead() && !StreamState::Off.is_dead());
    }
}
