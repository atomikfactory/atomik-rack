//! Low-frequency oscillator with sine, triangle, saw, square and sample & hold shapes.
//!
//! Output is bipolar −1..=1. Shapes are naive (no band-limiting): LFO rates are far below
//! Nyquist. Sample & hold draws a new uniform value from a seeded [`Rng`] at every cycle start.

use crate::Rng;

/// LFO waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfoShape {
    /// Sine, 0 at phase 0, rising.
    Sine,
    /// Triangle, 0 at phase 0, rising, +1 at 1/4, −1 at 3/4.
    Triangle,
    /// Rising saw, −1 at phase 0.
    Saw,
    /// Square, +1 for the first half cycle.
    Square,
    /// A new random value each cycle, held.
    SampleHold,
}

impl LfoShape {
    /// Maps a choice index (0 sine, 1 triangle, 2 saw, 3 square, 4 S&H; out of range clamps).
    #[inline]
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Sine,
            1 => Self::Triangle,
            2 => Self::Saw,
            3 => Self::Square,
            _ => Self::SampleHold,
        }
    }
}

/// Free-running LFO. Rate changes are phase-continuous.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lfo {
    phase: f64,
    inc: f64,
    shape: LfoShape,
    held: f32,
    seed: u64,
    rng: Rng,
}

impl Lfo {
    /// A sine LFO at 0 Hz whose sample & hold stream is derived from `seed`.
    pub fn new(seed: u64) -> Self {
        let mut l = Self {
            phase: 0.0,
            inc: 0.0,
            shape: LfoShape::Sine,
            held: 0.0,
            seed,
            rng: Rng::new(seed),
        };
        l.reset();
        l
    }

    /// Sets the rate in Hz (clamped to `0..=0.5·sample_rate`).
    #[inline]
    pub fn set_rate(&mut self, hz: f32, sample_rate: f32) {
        let inc = f64::from(hz) / f64::from(sample_rate);
        self.inc = if inc.is_finite() {
            inc.clamp(0.0, 0.5)
        } else {
            0.0
        };
    }

    /// Selects the shape (phase is kept).
    #[inline]
    pub fn set_shape(&mut self, shape: LfoShape) {
        self.shape = shape;
    }

    /// Restarts at phase 0 and re-seeds the sample & hold stream (deterministic).
    #[inline]
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.rng = Rng::new(self.seed);
        self.held = self.rng.next_bipolar();
    }

    /// Produces one sample in −1..=1 and advances the phase.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let p = self.phase;
        let y = match self.shape {
            LfoShape::Sine => (std::f64::consts::TAU * p).sin() as f32,
            LfoShape::Triangle => (1.0 - 4.0 * ((p + 0.25).fract() - 0.5).abs()) as f32,
            LfoShape::Saw => (2.0 * p - 1.0) as f32,
            LfoShape::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoShape::SampleHold => self.held,
        };
        self.phase += self.inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            self.held = self.rng.next_bipolar();
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::max_step;

    const SR: f32 = 48_000.0;

    fn render(shape: LfoShape, hz: f32, seed: u64, n: usize) -> Vec<f32> {
        let mut l = Lfo::new(seed);
        l.set_shape(shape);
        l.set_rate(hz, SR);
        (0..n).map(|_| l.next_sample()).collect()
    }

    fn rising_zero_crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    #[test]
    fn lfo_rates_and_shapes() {
        let n = SR as usize; // 1 s
        for shape in [
            LfoShape::Sine,
            LfoShape::Triangle,
            LfoShape::Saw,
            LfoShape::Square,
            LfoShape::SampleHold,
        ] {
            let x = render(shape, 5.0, 1, n);
            assert!(
                x.iter().all(|v| (-1.0..=1.0).contains(v)),
                "{shape:?} range"
            );
        }
        // Rate: 5 Hz sine has 5 cycles per second.
        let sine = render(LfoShape::Sine, 5.0, 1, n);
        assert!(rising_zero_crossings(&sine).abs_diff(5) <= 1);
        assert!((sine[2_400] - 1.0).abs() < 1e-3, "sine peak at 1/4 cycle");
        // Shapes at known phases (5 Hz: one cycle = 9600 samples).
        let tri = render(LfoShape::Triangle, 5.0, 1, n);
        assert!(tri[0].abs() < 1e-3 && (tri[2_400] - 1.0).abs() < 1e-3);
        assert!((tri[7_200] + 1.0).abs() < 1e-3 && tri[4_800].abs() < 1e-3);
        let saw = render(LfoShape::Saw, 5.0, 1, n);
        assert!((saw[0] + 1.0).abs() < 1e-3 && saw[4_800].abs() < 1e-3);
        assert!((saw[9_599] - 1.0).abs() < 1e-3);
        let sq = render(LfoShape::Square, 5.0, 1, n);
        assert!(sq.iter().all(|v| (v.abs() - 1.0).abs() < 1e-6));
        assert!((sq[100] - 1.0).abs() < 1e-6 && (sq[5_000] + 1.0).abs() < 1e-6);
        // Rate 2.5 Hz halves the number of cycles.
        assert!(rising_zero_crossings(&render(LfoShape::Saw, 2.5, 1, n)).abs_diff(2) <= 1);
    }

    #[test]
    fn lfo_sample_and_hold_is_seeded_and_held_per_cycle() {
        let n = SR as usize;
        let a = render(LfoShape::SampleHold, 10.0, 7, n);
        let b = render(LfoShape::SampleHold, 10.0, 7, n);
        let c = render(LfoShape::SampleHold, 10.0, 8, n);
        assert_eq!(a, b);
        assert_ne!(a, c);
        let changes = a
            .windows(2)
            .filter(|w| w[0].to_bits() != w[1].to_bits())
            .count();
        assert!(changes.abs_diff(9) <= 1, "{changes} changes in 10 cycles");
        // Held within a cycle.
        assert!(a[1..4_799].iter().all(|v| v.to_bits() == a[0].to_bits()));
    }

    #[test]
    fn lfo_reset_restarts_deterministically() {
        let mut l = Lfo::new(3);
        l.set_shape(LfoShape::SampleHold);
        l.set_rate(30.0, SR);
        let first: Vec<f32> = (0..10_000).map(|_| l.next_sample()).collect();
        l.reset();
        let again: Vec<f32> = (0..10_000).map(|_| l.next_sample()).collect();
        assert_eq!(first, again);
    }

    #[test]
    fn lfo_rate_change_is_phase_continuous() {
        let mut l = Lfo::new(0);
        l.set_rate(2.0, SR);
        let mut x: Vec<f32> = (0..10_000).map(|_| l.next_sample()).collect();
        l.set_rate(20.0, SR);
        x.extend((0..10_000).map(|_| l.next_sample()));
        assert!(max_step(&x) <= std::f32::consts::TAU * 20.0 / SR * 1.01);
    }
}
