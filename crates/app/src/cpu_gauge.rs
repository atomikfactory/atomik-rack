//! The pure parts of the CPU readout: the smoothed figure of a device with its peak hold, and
//! the watch that decides when to suggest freezing the heaviest track.
//!
//! Nothing here reads a clock or the engine: the caller passes the audio time each reading
//! covers (or, for the watch, a time in seconds), so a test feeds fake series.

use atomik_model::DeviceKey;
use std::collections::BTreeSet;

/// Time constant of the smoothed figure, in nanoseconds of audio (about 1 s).
pub const AVERAGE_NS: u64 = 1_000_000_000;
/// How long the peak figure holds the highest reading, in nanoseconds of audio.
pub const PEAK_HOLD_NS: u64 = 2_000_000_000;

/// A device's CPU figure: its load averaged over about a second, and the peak of the readings
/// held for two seconds before it falls back to the average.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Gauge {
    avg: Option<f32>,
    peak: f32,
    peak_age_ns: u64,
}

impl Gauge {
    /// Takes the reading of one window: `reading` is the share of the audio time (percent) the
    /// device used during `window_ns` nanoseconds of audio. The first reading is the figure; the
    /// next ones move it by `1 - exp(-window / 1 s)` of the way.
    pub fn push(&mut self, reading: f32, window_ns: u64) {
        let reading = reading.max(0.0);
        let alpha = 1.0 - (-(window_ns as f64) / AVERAGE_NS as f64).exp() as f32;
        let avg = match self.avg {
            Some(old) => old + (reading - old) * alpha,
            None => reading,
        };
        self.avg = Some(avg);
        if reading >= self.peak {
            self.peak = reading;
            self.peak_age_ns = 0;
        } else {
            self.peak_age_ns = self.peak_age_ns.saturating_add(window_ns);
            if self.peak_age_ns > PEAK_HOLD_NS {
                // The hold is over: the peak falls towards what the device uses now.
                let floor = reading.max(avg);
                self.peak = (self.peak + (floor - self.peak) * alpha).max(floor);
            }
        }
    }

    /// The smoothed load in percent of the audio time; 0 before the first reading.
    pub fn average(&self) -> f32 {
        self.avg.unwrap_or(0.0)
    }

    /// The held peak in percent of the audio time.
    pub fn peak(&self) -> f32 {
        self.peak
    }
}

/// The engine load (percent) above which the hint starts counting.
pub const BUSY_PERCENT: f32 = 70.0;
/// How long the engine load must stay above [`BUSY_PERCENT`] while playing, in seconds.
pub const BUSY_SECONDS: f64 = 5.0;

/// A suggestion to freeze a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Hint {
    /// The track (its instrument's key).
    pub track: DeviceKey,
    /// What the track's chain uses, in percent of the audio time.
    pub percent: f32,
}

/// Decides when the freeze hint shows: the engine load stayed above [`BUSY_PERCENT`] for
/// [`BUSY_SECONDS`] while playing. Each track is suggested at most once per watch (the app
/// starts a new one for every song).
#[derive(Debug, Clone, Default)]
pub struct HintWatch {
    since: Option<f64>,
    told: BTreeSet<DeviceKey>,
}

impl HintWatch {
    /// One look at the engine: `now` in seconds on any monotonic clock, whether the song is
    /// `playing`, the `engine_load` in percent and the track that `heaviest` names with its
    /// load (`None` when no track can be frozen). Returns the hint when it is time.
    pub fn step(
        &mut self,
        now: f64,
        playing: bool,
        engine_load: f32,
        heaviest: Option<(DeviceKey, f32)>,
    ) -> Option<Hint> {
        if !playing || engine_load <= BUSY_PERCENT {
            self.since = None;
            return None;
        }
        let since = *self.since.get_or_insert(now);
        if now - since < BUSY_SECONDS {
            return None;
        }
        let (track, percent) = heaviest?;
        if !self.told.insert(track.clone()) {
            return None;
        }
        // A second hint needs another stretch of high load.
        self.since = None;
        Some(Hint { track, percent })
    }

    /// Forgets the tracks already suggested and the stretch of load (a new song).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Whether `track` was suggested already.
    pub fn told(&self, track: &DeviceKey) -> bool {
        self.told.contains(track)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: u64 = 200_000_000;

    fn key(k: &str) -> DeviceKey {
        DeviceKey(k.to_string())
    }

    #[test]
    fn the_first_reading_is_the_figure_and_the_next_ones_average_over_about_a_second() {
        let mut g = Gauge::default();
        assert_eq!(g.average(), 0.0);
        g.push(10.0, WINDOW);
        assert_eq!(g.average(), 10.0);
        // A load that jumps to 50 % needs about a second to be mostly there.
        for _ in 0..5 {
            g.push(50.0, WINDOW);
        }
        let after_a_second = g.average();
        assert!(
            (30.0..=10.0 + 40.0 * 0.7).contains(&after_a_second),
            "{after_a_second}"
        );
        for _ in 0..25 {
            g.push(50.0, WINDOW);
        }
        assert!((g.average() - 50.0).abs() < 0.1, "{}", g.average());
        // The window length counts: one long window moves further than one short one.
        let (mut short, mut long) = (Gauge::default(), Gauge::default());
        short.push(0.0, WINDOW);
        long.push(0.0, WINDOW);
        short.push(100.0, WINDOW);
        long.push(100.0, 4 * WINDOW);
        assert!(long.average() > short.average());
    }

    #[test]
    fn the_peak_holds_for_two_seconds_then_falls_back_to_the_average() {
        let mut g = Gauge::default();
        g.push(5.0, WINDOW);
        g.push(80.0, WINDOW);
        assert_eq!(g.peak(), 80.0);
        // Nine quiet windows (1.8 s): still held.
        for _ in 0..9 {
            g.push(5.0, WINDOW);
        }
        assert_eq!(g.peak(), 80.0);
        // Past two seconds it starts to fall, and ends near the load.
        for _ in 0..4 {
            g.push(5.0, WINDOW);
        }
        assert!(g.peak() < 80.0);
        let falling = g.peak();
        for _ in 0..60 {
            g.push(5.0, WINDOW);
        }
        assert!(g.peak() < falling);
        assert!((g.peak() - g.average()).abs() < 0.5, "{}", g.peak());
        // A new high takes the hold at once.
        g.push(40.0, WINDOW);
        assert_eq!(g.peak(), 40.0);
    }

    #[test]
    fn a_negative_reading_counts_as_zero() {
        let mut g = Gauge::default();
        g.push(-3.0, WINDOW);
        assert_eq!(g.average(), 0.0);
        assert_eq!(g.peak(), 0.0);
    }

    fn series(w: &mut HintWatch, loads: &[f32], playing: bool) -> Vec<Option<Hint>> {
        let mut out = Vec::new();
        for (i, load) in loads.iter().enumerate() {
            let heaviest = Some((key("bass"), 41.0));
            out.push(w.step(i as f64, playing, *load, heaviest));
        }
        out
    }

    #[test]
    fn the_hint_comes_after_five_seconds_above_seventy_percent() {
        let mut w = HintWatch::default();
        let got = series(&mut w, &[80.0; 8], true);
        assert!(got[..5].iter().all(Option::is_none), "{got:?}");
        assert_eq!(
            got[5],
            Some(Hint {
                track: key("bass"),
                percent: 41.0
            })
        );
        assert!(got[6..].iter().all(Option::is_none));
    }

    #[test]
    fn a_dip_a_pause_or_exactly_seventy_start_the_count_again() {
        let mut w = HintWatch::default();
        let mut loads = vec![90.0; 4];
        loads.push(70.0);
        loads.extend([90.0; 4]);
        assert!(series(&mut w, &loads, true).iter().all(Option::is_none));
        let mut w = HintWatch::default();
        assert!(
            series(&mut w, &[95.0; 9], false)
                .iter()
                .all(Option::is_none)
        );
    }

    #[test]
    fn a_track_is_suggested_once_per_watch_and_a_new_song_starts_a_new_one() {
        let mut w = HintWatch::default();
        let first: Vec<_> = series(&mut w, &[90.0; 20], true)
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(first.len(), 1, "once for the track");
        assert!(w.told(&key("bass")));
        // Another track can still be suggested.
        let mut hints = Vec::new();
        for i in 20..30 {
            hints.extend(w.step(i as f64, true, 90.0, Some((key("lead"), 30.0))));
        }
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].track, key("lead"));
        w.reset();
        let again: Vec<_> = series(&mut w, &[90.0; 8], true)
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(again.len(), 1);
    }

    #[test]
    fn no_freezable_track_means_no_hint() {
        let mut w = HintWatch::default();
        for i in 0..12 {
            assert_eq!(w.step(f64::from(i), true, 99.0, None), None);
        }
    }
}
