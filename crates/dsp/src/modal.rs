//! Modal resonator bank: up to twelve two-pole resonators with a per-mode frequency, decay and
//! gain, excited by the input.
//!
//! Each mode is `y = b0 x + 2 r cos(theta) y1 - r^2 y2` with `r = 10^(-3 / (rt60 fs))`, so the
//! amplitude of its ringing falls by 60 dB in `rt60` seconds. `b0 = 2 gain (1 - r) sin(theta)`
//! gives a peak gain of `gain` at the resonance. Even modes go to the left output and odd modes to
//! the right, and the sum of the first `active` modes is divided by `sqrt(active)` so adding modes
//! does not raise the level much. Frequencies are clamped to `20 Hz..=0.45 fs`, decays to
//! `10 ms..=60 s`. Coefficients come from the deterministic `exp2`/cosine (no libm); the state
//! is flushed against denormals, so a ringing tail ends at exact zero.

use crate::det_trig::{cos_cycles, sin_cycles};
use crate::{exp2_det_f64, flush_denormal};

/// Largest number of modes.
pub const MAX_MODES: usize = 12;
/// `3 log2(10)`.
const THREE_LOG2_10: f64 = 9.965_784_284_662_087;

#[derive(Debug, Clone, Copy, Default)]
struct Mode {
    b0: f32,
    a1: f32,
    a2: f32,
    y1: f32,
    y2: f32,
}

/// A bank of up to twelve resonators.
#[derive(Debug, Clone)]
pub struct Modal {
    sample_rate: f32,
    modes: [Mode; MAX_MODES],
    active: usize,
}

impl Modal {
    /// A silent bank (every mode at gain 0) with no active modes. Call [`Modal::set_mode`] and
    /// [`Modal::set_active`].
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            modes: [Mode::default(); MAX_MODES],
            active: 0,
        }
    }

    /// Configures mode `i` (ignored when `i >= 12`). Non-finite arguments are ignored.
    pub fn set_mode(&mut self, i: usize, freq_hz: f32, rt60_s: f32, gain: f32) {
        if i >= MAX_MODES || !freq_hz.is_finite() || !rt60_s.is_finite() || !gain.is_finite() {
            return;
        }
        let sr = f64::from(self.sample_rate);
        let f = f64::from(freq_hz).clamp(20.0, 0.45 * sr);
        let t = f64::from(rt60_s).clamp(0.01, 60.0);
        let r = exp2_det_f64(-THREE_LOG2_10 / (t * sr));
        let cycles = f / sr;
        let m = &mut self.modes[i];
        m.a1 = (2.0 * r * cos_cycles(cycles)) as f32;
        m.a2 = (-r * r) as f32;
        m.b0 = (2.0 * f64::from(gain) * (1.0 - r) * sin_cycles(cycles)) as f32;
    }

    /// Number of active modes `0..=12` (the others are silenced and cleared).
    pub fn set_active(&mut self, n: usize) {
        let n = n.min(MAX_MODES);
        for m in &mut self.modes[n..] {
            m.y1 = 0.0;
            m.y2 = 0.0;
        }
        self.active = n;
    }

    /// The number of active modes.
    pub fn active(&self) -> usize {
        self.active
    }

    /// Clears the ringing of every mode.
    pub fn reset(&mut self) {
        for m in &mut self.modes {
            m.y1 = 0.0;
            m.y2 = 0.0;
        }
    }

    /// Excites the bank with `x` and returns `(left, right)`: even modes left, odd modes right.
    #[inline]
    pub fn process(&mut self, x: f32) -> (f32, f32) {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for (i, m) in self.modes[..self.active].iter_mut().enumerate() {
            let y = flush_denormal(m.b0 * x + m.a1 * m.y1 + m.a2 * m.y2);
            m.y2 = m.y1;
            m.y1 = y;
            if i % 2 == 0 {
                l += y;
            } else {
                r += y;
            }
        }
        let k = if self.active > 0 {
            1.0 / (self.active as f32).sqrt()
        } else {
            0.0
        };
        (l * k, r * k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{db, dominant_hz, noise};

    const SR: f32 = 48_000.0;

    fn impulse_response(m: &mut Modal, n: usize) -> (Vec<f32>, Vec<f32>) {
        let mut l = Vec::with_capacity(n);
        let mut r = Vec::with_capacity(n);
        for i in 0..n {
            let (a, b) = m.process(if i == 0 { 1.0 } else { 0.0 });
            l.push(a);
            r.push(b);
        }
        (l, r)
    }

    fn peak(x: &[f32], from: usize, len: usize) -> f32 {
        x[from..from + len].iter().fold(0.0, |a, v| a.max(v.abs()))
    }

    #[test]
    fn a_mode_rings_at_its_frequency_with_the_requested_decay() {
        for (f, rt) in [(440.0f32, 1.0f32), (1500.0, 2.0), (90.0, 0.5)] {
            let mut m = Modal::new(SR);
            m.set_mode(0, f, rt, 1.0);
            m.set_active(1);
            let (l, _) = impulse_response(&mut m, 32_768);
            let got = dominant_hz(&l[..16_384], f64::from(SR));
            assert!((got - f64::from(f)).abs() < 1.0, "{f}: {got}");
            // Peak envelope in two windows 0.4 s apart: 60 dB per rt60 seconds.
            let win = (SR * 0.02) as usize;
            let a = peak(&l, (SR * 0.05) as usize, win);
            let b = peak(&l, (SR * 0.45) as usize, win);
            let measured_rt = 60.0 * 0.4 / db(f64::from(a / b)) as f32;
            assert!(
                (measured_rt / rt - 1.0).abs() < 0.08,
                "{f} Hz rt {rt}: {measured_rt}"
            );
        }
    }

    #[test]
    fn peak_gain_at_resonance_is_the_mode_gain() {
        let mut m = Modal::new(SR);
        m.set_mode(0, 1000.0, 0.8, 0.5);
        m.set_active(1);
        let mut peak = 0.0f32;
        for i in 0..48_000 {
            let x = (std::f64::consts::TAU * 1000.0 * f64::from(i) / f64::from(SR)).sin() as f32;
            peak = peak.max(m.process(x).0.abs());
        }
        assert!((peak - 0.5).abs() < 0.03, "{peak}");
    }

    #[test]
    fn even_modes_go_left_odd_modes_go_right_and_active_count_limits() {
        let mut m = Modal::new(SR);
        for i in 0..4 {
            m.set_mode(i, 300.0 + 100.0 * i as f32, 0.5, 1.0);
        }
        m.set_active(2);
        let (l, r) = impulse_response(&mut m, 4_000);
        assert!(l.iter().any(|v| v.abs() > 1e-4) && r.iter().any(|v| v.abs() > 1e-4));
        // Only mode 1 (right) rings when mode 0 has zero gain.
        m.reset();
        m.set_mode(0, 300.0, 0.5, 0.0);
        let (l, r) = impulse_response(&mut m, 4_000);
        assert!(l.iter().all(|v| *v == 0.0) && r.iter().any(|v| v.abs() > 1e-4));
        // Deactivated modes are silent and cleared.
        m.set_active(0);
        assert_eq!(m.process(1.0), (0.0, 0.0));
        m.set_active(99);
        assert_eq!(m.active(), MAX_MODES);
        // The sum is divided by the square root of the active count.
        let mut one = Modal::new(SR);
        one.set_mode(0, 500.0, 0.5, 1.0);
        one.set_active(1);
        let mut four = Modal::new(SR);
        for i in [0, 2, 4, 6] {
            four.set_mode(i, 500.0, 0.5, 1.0);
        }
        four.set_active(7);
        let (a, _) = impulse_response(&mut one, 100);
        let (b, _) = impulse_response(&mut four, 100);
        // Four identical left modes among seven active: 4 / sqrt(7) times one mode.
        assert!((b[10] / a[10] - 4.0 / 7f32.sqrt()).abs() < 1e-4);
    }

    #[test]
    fn long_tails_end_at_exact_zero_and_parameters_are_clamped() {
        let mut m = Modal::new(SR);
        m.set_mode(0, 1.0, 0.0001, 1.0); // clamped to 20 Hz, 10 ms
        m.set_mode(1, 1.0e9, 1.0e9, 1.0); // clamped to 0.45 fs, 60 s
        m.set_mode(2, f32::NAN, 1.0, 1.0);
        m.set_mode(99, 100.0, 1.0, 1.0);
        m.set_active(2);
        let (l, r) = impulse_response(&mut m, 48_000 * 2);
        assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 3.0));
        let mut m = Modal::new(SR);
        m.set_mode(0, 500.0, 0.01, 1.0);
        m.set_active(1);
        let (l, _) = impulse_response(&mut m, 48_000);
        assert_eq!(*l.last().unwrap(), 0.0);
    }

    proptest::proptest! {
        #[test]
        fn finite_and_bounded_for_any_parameters(
            f in 0.0f32..30_000.0, rt in 0.0f32..100.0, g in 0.0f32..2.0,
            active in 0usize..14, seed in 0u64..300
        ) {
            let mut m = Modal::new(SR);
            for i in 0..MAX_MODES {
                m.set_mode(i, f * (1.0 + i as f32 * 0.37), rt, g);
            }
            m.set_active(active);
            let x = noise(seed, 4096, 1.0);
            for v in x {
                let (a, b) = m.process(v);
                proptest::prop_assert!(a.is_finite() && b.is_finite() && a.abs() < 60.0 && b.abs() < 60.0);
            }
        }
    }
}
