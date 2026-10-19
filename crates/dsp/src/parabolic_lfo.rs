//! Sine-like LFO built from a parabola: no libm in the per-sample path, bit-identical everywhere.
//!
//! With `u = 2*phase - 1` the output is `4*u*(1 - |u|)`, a smooth bipolar wave in -1..=1 (zero at
//! the cycle start, -1 at a quarter cycle, +1 at three quarters) whose slope is continuous across
//! the wrap. Start phase and rate are drawn from a seeded [`crate::Rng`] by
//! [`ParabolicLfo::seeded`].

use crate::Rng;

/// Free-running parabolic LFO.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParabolicLfo {
    phase: f32,
    inc: f32,
}

impl ParabolicLfo {
    /// An LFO at `phase` (0..1) with `rate_hz` at `sample_rate`.
    pub fn new(phase: f32, rate_hz: f32, sample_rate: f32) -> Self {
        Self {
            phase: phase - phase.floor(),
            inc: (rate_hz / sample_rate).clamp(0.0, 0.5),
        }
    }

    /// Start phase uniform in 0..1 and rate uniform in `min_hz..max_hz`, both from `rng`.
    pub fn seeded(rng: &mut Rng, min_hz: f32, max_hz: f32, sample_rate: f32) -> Self {
        let phase = rng.next_f32();
        let rate = min_hz + (max_hz - min_hz) * rng.next_f32();
        Self::new(phase, rate, sample_rate)
    }

    /// Next sample in -1..=1.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let u = 2.0 * self.phase - 1.0;
        let y = 4.0 * u * (1.0 - u.abs());
        self.phase += self.inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parabolic_lfo_range_shape_and_rate() {
        let mut l = ParabolicLfo::new(0.0, 10.0, 48_000.0);
        let x: Vec<f32> = (0..48_000).map(|_| l.next_sample()).collect();
        assert!(x.iter().all(|v| (-1.0..=1.0).contains(v)));
        // u = -1 at phase 0: output 0; quarter cycle (u = -0.5) is -1; three quarters is +1.
        assert!(x[0].abs() < 1e-6);
        assert!((x[1_200] + 1.0).abs() < 1e-3 && (x[3_600] - 1.0).abs() < 1e-3);
        let crossings = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!(crossings.abs_diff(10) <= 1);
        let max_step = x.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(max_step < 4.0 * 2.0 * 10.0 / 48_000.0 * 1.05, "{max_step}");
    }

    #[test]
    fn parabolic_lfo_seeding_is_deterministic() {
        let mk = |seed| {
            let mut r = Rng::new(seed);
            let mut l = ParabolicLfo::seeded(&mut r, 0.2, 1.2, 48_000.0);
            (0..1_000).map(|_| l.next_sample()).collect::<Vec<_>>()
        };
        assert_eq!(mk(5), mk(5));
        assert_ne!(mk(5), mk(6));
    }
}
