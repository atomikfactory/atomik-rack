//! Flat-sum 24 dB/oct two-band crossover (Linkwitz-Riley, fourth order).
//!
//! Each band is two identical Butterworth sections in cascade (Q = 1/sqrt 2). The low and high
//! bands are both 6 dB down at the corner and in phase, so their sum is an all-pass: the
//! magnitude of low plus high is exactly flat. Coefficients come from the RBJ design of [`crate::BiquadCoeffs`] and
//! are recomputed only when the frequency changes; the filter state is `f64` direct form I, so a
//! frequency change while running is click-free for slow sweeps (the device smooths it).

use crate::{Biquad, BiquadCoeffs, BiquadShape};

const BUTTERWORTH_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// Lowest corner in Hz.
pub const MIN_FREQ_HZ: f32 = 20.0;

/// A mono two-band crossover (use two for a stereo signal, with the same frequency).
#[derive(Debug, Clone)]
pub struct Crossover {
    sample_rate: f32,
    freq: f32,
    lp: BiquadCoeffs,
    hp: BiquadCoeffs,
    lp_state: [Biquad; 2],
    hp_state: [Biquad; 2],
}

impl Crossover {
    /// A crossover at 1 kHz.
    pub fn new(sample_rate: f32) -> Self {
        let mut c = Self {
            sample_rate,
            freq: 0.0,
            lp: BiquadCoeffs::IDENTITY,
            hp: BiquadCoeffs::IDENTITY,
            lp_state: [Biquad::default(); 2],
            hp_state: [Biquad::default(); 2],
        };
        c.set_freq(1000.0);
        c
    }

    /// Sets the corner frequency in Hz (clamped to `20 Hz..=0.45 fs`; NaN ignored).
    pub fn set_freq(&mut self, hz: f32) {
        if hz.is_nan() {
            return;
        }
        let f = hz.clamp(MIN_FREQ_HZ, 0.45 * self.sample_rate);
        if f.to_bits() == self.freq.to_bits() {
            return;
        }
        self.freq = f;
        let sr = f64::from(self.sample_rate);
        self.lp = BiquadCoeffs::design(BiquadShape::Lowpass, f64::from(f), BUTTERWORTH_Q, 0.0, sr);
        self.hp = BiquadCoeffs::design(BiquadShape::Highpass, f64::from(f), BUTTERWORTH_Q, 0.0, sr);
    }

    /// The corner frequency.
    pub fn freq(&self) -> f32 {
        self.freq
    }

    /// Clears the filter state.
    pub fn reset(&mut self) {
        for b in self.lp_state.iter_mut().chain(self.hp_state.iter_mut()) {
            b.reset();
        }
    }

    /// Splits one sample into `(low, high)`.
    #[inline]
    pub fn process(&mut self, x: f32) -> (f32, f32) {
        let x = f64::from(x);
        let l0 = self.lp_state[0].process(&self.lp, x);
        let low = self.lp_state[1].process(&self.lp, l0);
        let h0 = self.hp_state[0].process(&self.hp, x);
        let high = self.hp_state[1].process(&self.hp, h0);
        (low as f32, high as f32)
    }

    /// Splits a block into `low` and `high` (all three slices must have the same length).
    pub fn process_block(&mut self, input: &[f32], low: &mut [f32], high: &mut [f32]) {
        for ((x, lo), hi) in input.iter().zip(low.iter_mut()).zip(high.iter_mut()) {
            let (a, b) = self.process(*x);
            *lo = a;
            *hi = b;
        }
    }

    /// Analytic magnitude in dB of `(low, high)` at `freq` Hz.
    pub fn response_db(&self, freq: f32) -> (f32, f32) {
        let sr = f64::from(self.sample_rate);
        let f = f64::from(freq);
        (
            (2.0 * self.lp.response_db(f, sr)) as f32,
            (2.0 * self.hp.response_db(f, sr)) as f32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{assert_chunk_invariant_mono, db, noise, sine, tone_amplitude};

    const SR: f32 = 48_000.0;

    /// Measured gains (dB) of low, high and their sum for a sine at a bin-centred frequency.
    fn measure(c: &mut Crossover, freq: f64) -> (f64, f64, f64) {
        let n = 48_000usize;
        let x = sine(freq, f64::from(SR), n, 0.5);
        let mut lo = vec![0.0f32; n];
        let mut hi = vec![0.0f32; n];
        c.reset();
        c.process_block(&x, &mut lo, &mut hi);
        let sum: Vec<f32> = lo.iter().zip(&hi).map(|(a, b)| a + b).collect();
        let w = 24_000..n;
        let g = |y: &[f32]| db(tone_amplitude(&y[w.clone()], freq, f64::from(SR)) / 0.5);
        (g(&lo), g(&hi), g(&sum))
    }

    #[test]
    fn bands_sum_flat_within_a_tenth_of_a_db() {
        let mut c = Crossover::new(SR);
        for corner in [120.0f32, 1000.0, 5000.0] {
            c.set_freq(corner);
            for k in 0..14 {
                let f = f64::from(corner) * 2f64.powf(-3.0 + 6.0 * f64::from(k) / 13.0);
                if f > 20_000.0 || f < 30.0 {
                    continue;
                }
                let f = (f / 1.0).round(); // whole Hz: whole cycles in one second
                let (_, _, sum) = measure(&mut c, f);
                assert!(sum.abs() < 0.1, "corner {corner} at {f} Hz: sum {sum} dB");
            }
        }
    }

    #[test]
    fn bands_are_six_db_down_at_the_corner_with_a_24_db_per_octave_slope() {
        let mut c = Crossover::new(SR);
        c.set_freq(1000.0);
        let (lo, hi, _) = measure(&mut c, 1000.0);
        assert!(
            (lo + 6.02).abs() < 0.1 && (hi + 6.02).abs() < 0.1,
            "{lo} {hi}"
        );
        // Low band one and two octaves above the corner: 24 dB per octave within 1 dB.
        let (l4, _, _) = measure(&mut c, 2000.0);
        let (l8, _, _) = measure(&mut c, 4000.0);
        assert!(((l4 - l8) - 24.0).abs() < 1.0, "{l4} {l8}");
        // High band one and two octaves below.
        let (_, h250, _) = measure(&mut c, 500.0);
        let (_, h125, _) = measure(&mut c, 250.0);
        assert!(((h250 - h125) - 24.0).abs() < 1.0, "{h250} {h125}");
        // The analytic response agrees with the measurement.
        let (al, ah) = c.response_db(2000.0);
        assert!((f64::from(al) - l4).abs() < 0.05);
        let (_, ah2) = c.response_db(500.0);
        assert!((f64::from(ah2) - h250).abs() < 0.05);
        assert!(ah.is_finite());
    }

    #[test]
    fn frequency_is_clamped_and_nan_ignored() {
        let mut c = Crossover::new(SR);
        c.set_freq(1.0);
        assert_eq!(c.freq(), MIN_FREQ_HZ);
        c.set_freq(1.0e9);
        assert_eq!(c.freq(), 0.45 * SR);
        c.set_freq(f32::NAN);
        assert_eq!(c.freq(), 0.45 * SR);
    }

    #[test]
    fn process_is_chunk_invariant() {
        let x = noise(2, 3_000, 0.8);
        assert_chunk_invariant_mono(
            || Crossover::new(SR),
            |c, b| {
                let mut lo = b.to_vec();
                let mut hi = b.to_vec();
                c.process_block(b, &mut lo, &mut hi);
                for ((o, a), h) in b.iter_mut().zip(&lo).zip(&hi) {
                    *o = a - h;
                }
            },
            &x,
        );
    }

    proptest::proptest! {
        #[test]
        fn output_is_finite_and_bounded_when_the_frequency_sweeps(
            f1 in 20.0f32..20_000.0, f2 in 20.0f32..20_000.0, seed in 0u64..500
        ) {
            let mut c = Crossover::new(SR);
            let x = noise(seed, 2048, 1.0);
            let mut peak = 0.0f32;
            for (i, v) in x.iter().enumerate() {
                if i % 64 == 0 {
                    c.set_freq(if (i / 64) % 2 == 0 { f1 } else { f2 });
                }
                let (a, b) = c.process(*v);
                peak = peak.max(a.abs()).max(b.abs());
            }
            proptest::prop_assert!(peak.is_finite() && peak < 1000.0);
        }
    }
}
