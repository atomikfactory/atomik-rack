//! Tap tempo: the average interval of the last few taps, as a tempo.
//!
//! The clock is injected (`now` in seconds, any monotonic source), so the logic is pure. Taps
//! further apart than [`TIMEOUT`] start a new sequence; a sequence keeps the last [`MAX_TAPS`]
//! taps, and the first tap alone gives no tempo.

use atomik_model::{TEMPO_MAX, TEMPO_MIN};

/// Seconds without a tap after which the next tap starts a new sequence.
pub const TIMEOUT: f64 = 2.0;
/// The most taps one sequence keeps (the average runs over the last of them).
pub const MAX_TAPS: usize = 16;

/// The taps of the sequence in progress.
#[derive(Debug, Clone, Default)]
pub struct TapTempo {
    taps: Vec<f64>,
}

impl TapTempo {
    /// A tap at `now` seconds. Returns the tempo (BPM, clamped to the tempo range, rounded to
    /// hundredths) once there are two taps or more.
    pub fn tap(&mut self, now: f64) -> Option<f64> {
        if self
            .taps
            .last()
            .is_some_and(|last| now - last > TIMEOUT || now < *last)
        {
            self.taps.clear();
        }
        if self.taps.len() == MAX_TAPS {
            self.taps.remove(0);
        }
        self.taps.push(now);
        let (first, last) = (self.taps.first()?, self.taps.last()?);
        let intervals = self.taps.len().checked_sub(1).filter(|n| *n > 0)?;
        let mean = (last - first) / intervals as f64;
        if mean <= 0.0 {
            return None;
        }
        let bpm = (60.0 / mean).clamp(TEMPO_MIN, TEMPO_MAX);
        Some((bpm * 100.0).round() / 100.0)
    }

    /// The taps held (for display and tests).
    pub fn len(&self) -> usize {
        self.taps.len()
    }

    /// No tap is held.
    pub fn is_empty(&self) -> bool {
        self.taps.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_taps_at_half_a_second_make_120_bpm() {
        let mut t = TapTempo::default();
        assert_eq!(t.tap(10.0), None);
        let mut last = None;
        for i in 1..4 {
            last = t.tap(10.0 + 0.5 * f64::from(i));
        }
        let bpm = last.unwrap();
        assert!((bpm - 120.0).abs() < 0.5, "{bpm}");
    }

    #[test]
    fn a_pause_of_two_seconds_starts_over() {
        let mut t = TapTempo::default();
        t.tap(0.0);
        assert!(t.tap(0.5).is_some());
        assert_eq!(t.tap(3.0), None, "a new sequence has one tap");
        assert_eq!(t.len(), 1);
        let bpm = t.tap(3.6).unwrap();
        assert!((bpm - 100.0).abs() < 0.5);
    }

    #[test]
    fn the_average_runs_over_the_last_sixteen_taps() {
        let mut t = TapTempo::default();
        // Slow taps first, then fast ones: only the last sixteen count.
        let mut now = 0.0;
        for _ in 0..4 {
            t.tap(now);
            now += 1.0;
        }
        let mut bpm = 0.0;
        for _ in 0..20 {
            now += 0.5;
            bpm = t.tap(now).unwrap();
        }
        assert_eq!(t.len(), MAX_TAPS);
        assert!((bpm - 120.0).abs() < 0.5, "{bpm}");
    }

    #[test]
    fn the_tempo_stays_in_range() {
        let mut t = TapTempo::default();
        t.tap(0.0);
        let fast = t.tap(0.001).unwrap();
        assert!(fast <= TEMPO_MAX);
        let mut t = TapTempo::default();
        t.tap(0.0);
        let slow = t.tap(1.9).unwrap();
        assert!(slow >= TEMPO_MIN);
    }
}
