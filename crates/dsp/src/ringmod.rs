//! Stereo ring modulator: a sine carrier times the input, with an amount that crossfades between
//! the dry and the ring-modulated signal.
//!
//! `y = x (1 - amount + amount c)`, `c = sin(2 pi phase)`: amount 0 is the exact dry signal
//! (skipped when the smoothed amount is exactly 0) and amount 1 is the plain product. It runs at
//! the base rate on purpose (no oversampling; the sidebands may alias). The carrier phase is an
//! `f64` accumulator, frequency changes are phase-continuous and glide through a critically damped
//! smoother (5 ms), as does the amount, so sweeping them does not click. The right channel's
//! carrier can be offset in phase (`stereo_phase`, cycles). The carrier is the deterministic
//! private `det_trig` sine (no libm).

use crate::det_trig::sin_cycles;
use crate::smoother2::Smoother2;

/// Smoothing time (90 % rise) of frequency and amount.
const SMOOTH_MS: f32 = 5.0;

/// Stereo ring modulator.
#[derive(Debug, Clone)]
pub struct RingMod {
    sample_rate: f32,
    phase: f64,
    freq: Smoother2,
    amount: Smoother2,
    stereo_phase: f64,
}

impl RingMod {
    /// A ring modulator with carrier 100 Hz and amount 0.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            phase: 0.0,
            freq: Smoother2::new(100.0, SMOOTH_MS, sample_rate),
            amount: Smoother2::new(0.0, SMOOTH_MS, sample_rate),
            stereo_phase: 0.0,
        }
    }

    /// Carrier frequency in Hz (clamped to `0..=0.45 fs`).
    pub fn set_freq_hz(&mut self, hz: f32) {
        if !hz.is_nan() {
            self.freq.set_target(hz.clamp(0.0, 0.45 * self.sample_rate));
        }
    }

    /// Amount `0..=1`.
    pub fn set_amount(&mut self, amount: f32) {
        if !amount.is_nan() {
            self.amount.set_target(amount.clamp(0.0, 1.0));
        }
    }

    /// Phase offset of the right carrier in cycles.
    pub fn set_stereo_phase(&mut self, cycles: f32) {
        if cycles.is_finite() {
            self.stereo_phase = f64::from(cycles);
        }
    }

    /// Jumps frequency and amount to their targets (no glide).
    pub fn snap(&mut self) {
        self.freq.set_immediate(self.freq.target());
        self.amount.set_immediate(self.amount.target());
    }

    /// Restarts the carrier at phase 0 and snaps the smoothers.
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.snap();
    }

    /// Processes one stereo sample.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let f = self.freq.next_value();
        let a = self.amount.next_value();
        let c_l = sin_cycles(self.phase) as f32;
        let c_r = sin_cycles(self.phase + self.stereo_phase) as f32;
        self.phase += f64::from(f) / f64::from(self.sample_rate);
        self.phase -= self.phase.floor();
        if a == 0.0 {
            return (l, r);
        }
        (l * (1.0 - a + a * c_l), r * (1.0 - a + a * c_r))
    }

    /// Processes a stereo block in place (identical to per-sample calls).
    pub fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let (x, y) = self.process(*a, *b);
            *a = x;
            *b = y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{assert_chunk_invariant_stereo, db, noise, sine, tone_amplitude};

    const SR: f32 = 48_000.0;

    fn run(amount: f32, carrier: f32, input_hz: f64, n: usize) -> Vec<f32> {
        let mut m = RingMod::new(SR);
        m.set_freq_hz(carrier);
        m.set_amount(amount);
        m.snap();
        let mut l = sine(input_hz, f64::from(SR), n, 0.5);
        let mut r = l.clone();
        m.process_block(&mut l, &mut r);
        l
    }

    #[test]
    fn amount_zero_is_bit_exact_dry() {
        let mut m = RingMod::new(SR);
        m.set_freq_hz(440.0);
        let x = noise(1, 5_000, 1.0);
        for v in &x {
            let (a, b) = m.process(*v, -*v);
            assert_eq!(a.to_bits(), v.to_bits());
            assert_eq!(b.to_bits(), (-*v).to_bits());
        }
    }

    #[test]
    fn full_amount_makes_sum_and_difference_frequencies_only() {
        // 1 kHz input, 300 Hz carrier: 700 and 1300 Hz, nothing at 1000 or 300.
        let y = run(1.0, 300.0, 1000.0, 48_000);
        let a700 = tone_amplitude(&y, 700.0, f64::from(SR));
        let a1300 = tone_amplitude(&y, 1300.0, f64::from(SR));
        // 0.5 (input) * 0.5 (product of two sines) = 0.25 each.
        assert!(
            (a700 - 0.25).abs() < 1e-3 && (a1300 - 0.25).abs() < 1e-3,
            "{a700} {a1300}"
        );
        assert!(db(tone_amplitude(&y, 1000.0, f64::from(SR)) / a700) < -80.0);
        assert!(db(tone_amplitude(&y, 300.0, f64::from(SR)) / a700) < -80.0);
    }

    #[test]
    fn partial_amount_keeps_the_dry_part() {
        let y = run(0.4, 300.0, 1000.0, 48_000);
        let dry = tone_amplitude(&y, 1000.0, f64::from(SR));
        let side = tone_amplitude(&y, 700.0, f64::from(SR));
        assert!((dry - 0.5 * 0.6).abs() < 1e-3, "{dry}");
        assert!((side - 0.5 * 0.4 * 0.5).abs() < 1e-3, "{side}");
    }

    #[test]
    fn carrier_frequency_is_exact_and_changes_are_continuous() {
        // Modulating DC (a constant input) shows the carrier itself.
        let mut m = RingMod::new(SR);
        m.set_freq_hz(480.0);
        m.set_amount(1.0);
        m.snap();
        let mut l = vec![1.0f32; 48_000];
        let mut r = l.clone();
        m.process_block(&mut l, &mut r);
        assert!((tone_amplitude(&l, 480.0, f64::from(SR)) - 1.0).abs() < 1e-4);
        // Sweeping the frequency never jumps the carrier by more than a small step.
        m.set_freq_hz(4000.0);
        let mut l = vec![1.0f32; 4_800];
        let mut r = l.clone();
        m.process_block(&mut l, &mut r);
        let max_step = l.windows(2).fold(0.0f32, |a, w| a.max((w[1] - w[0]).abs()));
        assert!(max_step < 0.6, "{max_step}");
    }

    #[test]
    fn stereo_phase_offsets_the_right_carrier() {
        let mut m = RingMod::new(SR);
        m.set_freq_hz(100.0);
        m.set_amount(1.0);
        m.set_stereo_phase(0.25);
        m.snap();
        let mut l = vec![1.0f32; 480];
        let mut r = l.clone();
        m.process_block(&mut l, &mut r);
        // The right carrier is a cosine where the left is a sine.
        assert!(l[0].abs() < 1e-6 && (r[0] - 1.0).abs() < 1e-6);
        m.set_stereo_phase(f32::NAN);
        m.set_freq_hz(f32::NAN);
        m.set_amount(f32::NAN);
        m.reset();
        assert!(m.process(0.5, 0.5).0.is_finite());
    }

    #[test]
    fn process_is_chunk_invariant() {
        let a = noise(4, 3_000, 0.7);
        let b = noise(5, 3_000, 0.7);
        assert_chunk_invariant_stereo(
            || {
                let mut m = RingMod::new(SR);
                m.set_freq_hz(333.0);
                m.set_amount(0.8);
                m.set_stereo_phase(0.1);
                m
            },
            |m, x, y| m.process_block(x, y),
            &a,
            &b,
        );
    }

    proptest::proptest! {
        #[test]
        fn finite_and_bounded_for_any_parameters(
            f in 0.0f32..30_000.0, a in 0.0f32..1.0, sp in -2.0f32..2.0, seed in 0u64..300
        ) {
            let mut m = RingMod::new(SR);
            m.set_freq_hz(f);
            m.set_amount(a);
            m.set_stereo_phase(sp);
            let mut l = noise(seed, 1024, 1.0);
            let mut r = noise(seed + 9, 1024, 1.0);
            m.process_block(&mut l, &mut r);
            proptest::prop_assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() <= 1.0 + 1e-5));
        }
    }
}
