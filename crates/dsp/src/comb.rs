//! Feedforward plus feedback comb filter with a fractional delay.
//!
//! `y[n] = x[n] + ff * x[n - D] + fb * y[n - D]`, `D` fractional (Hermite-interpolated lines,
//! so `D >= 2` samples). The response is `(1 + ff e^{-jwD}) / (1 - fb e^{-jwD})`: peaks of gain
//! `(1 + ff)/(1 - fb)` at multiples of `fs/D`. A comb with feedback is the flanger of the
//! `ion-sweep` preset (a swept `D`), a short one with feedback is a hollow resonance. The
//! feedback is clamped to `+-0.98` (stable), the feedforward to `+-1`. Mono: use one per channel.

use crate::{ModDelay, flush_denormal};

/// Largest feedback magnitude.
const MAX_FEEDBACK: f32 = 0.98;

/// Mono comb filter.
#[derive(Debug, Clone)]
pub struct Comb {
    sample_rate: f32,
    x_line: ModDelay,
    y_line: ModDelay,
    delay: f32,
    feedforward: f32,
    feedback: f32,
}

impl Comb {
    /// A comb that can delay up to `max_ms` ms (non-RT allocation); defaults: 5 ms, ff 0, fb 0.
    pub fn new(sample_rate: f32, max_ms: f32) -> Self {
        let max = (max_ms.max(0.1) * 0.001 * sample_rate).ceil() as usize + 2;
        let mut c = Self {
            sample_rate,
            x_line: ModDelay::new(max),
            y_line: ModDelay::new(max),
            delay: 2.0,
            feedforward: 0.0,
            feedback: 0.0,
        };
        c.set_delay_ms(5.0);
        c
    }

    /// Delay in ms (clamped to `2 samples..=max`).
    pub fn set_delay_ms(&mut self, ms: f32) {
        self.set_delay_samples(ms * 0.001 * self.sample_rate);
    }

    /// Delay in samples (clamped to `2..=max`; NaN ignored).
    pub fn set_delay_samples(&mut self, d: f32) {
        if !d.is_nan() {
            self.delay = d.clamp(2.0, self.x_line.max_delay() as f32);
        }
    }

    /// The delay in samples.
    pub fn delay_samples(&self) -> f32 {
        self.delay
    }

    /// Feedback (clamped to +-0.98).
    pub fn set_feedback(&mut self, fb: f32) {
        if !fb.is_nan() {
            self.feedback = fb.clamp(-MAX_FEEDBACK, MAX_FEEDBACK);
        }
    }

    /// Feedforward (clamped to +-1).
    pub fn set_feedforward(&mut self, ff: f32) {
        if !ff.is_nan() {
            self.feedforward = ff.clamp(-1.0, 1.0);
        }
    }

    /// Clears the lines.
    pub fn reset(&mut self) {
        self.x_line.clear();
        self.y_line.clear();
    }

    /// Processes one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let xd = self.x_line.read(self.delay);
        let yd = self.y_line.read(self.delay);
        let y = flush_denormal(x + self.feedforward * xd + self.feedback * yd);
        self.x_line.write(x);
        self.y_line.write(y);
        y
    }

    /// Processes a block in place (identical to per-sample calls).
    pub fn process_block(&mut self, buf: &mut [f32]) {
        for s in buf {
            *s = self.process(*s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{assert_chunk_invariant_mono, db, noise, sine, tone_amplitude};

    const SR: f32 = 48_000.0;

    fn gain_db(c: &mut Comb, f: f64) -> f64 {
        c.reset();
        let mut x = sine(f, f64::from(SR), 48_000, 0.4);
        c.process_block(&mut x);
        db(tone_amplitude(&x[24_000..], f, f64::from(SR)) / 0.4)
    }

    #[test]
    fn integer_delay_matches_the_analytic_comb_response() {
        let mut c = Comb::new(SR, 20.0);
        c.set_delay_samples(48.0);
        c.set_feedforward(0.5);
        c.set_feedback(0.6);
        // wD = 2 pi at 1 kHz: peak (1 + ff)/(1 - fb); wD = pi at 500 Hz: notch (1 - ff)/(1 + fb).
        assert!((gain_db(&mut c, 1000.0) - db(1.5 / 0.4)).abs() < 0.02);
        assert!((gain_db(&mut c, 500.0) - db(0.5 / 1.6)).abs() < 0.02);
        // An arbitrary frequency against the closed form.
        let f = 730.0f64;
        let w = std::f64::consts::TAU * f / f64::from(SR) * 48.0;
        let num = ((1.0 + 0.5 * w.cos()).powi(2) + (0.5 * w.sin()).powi(2)).sqrt();
        let den = ((1.0 - 0.6 * w.cos()).powi(2) + (0.6 * w.sin()).powi(2)).sqrt();
        assert!((gain_db(&mut c, f) - db(num / den)).abs() < 0.02);
    }

    #[test]
    fn fractional_delay_moves_the_peak_to_fs_over_d() {
        let mut c = Comb::new(SR, 20.0);
        c.set_delay_samples(48.5);
        c.set_feedback(0.7);
        let f = f64::from(SR) / 48.5; // 989.69 Hz, whole Hz not needed for amplitude ratio
        let peak = gain_db(&mut c, f.round());
        assert!((peak - db(1.0 / 0.3)).abs() < 0.6, "{peak}");
        assert!(gain_db(&mut c, (f * 1.5).round()) < peak - 10.0);
    }

    #[test]
    fn impulse_response_is_a_decaying_series() {
        let mut c = Comb::new(SR, 20.0);
        c.set_delay_samples(100.0);
        c.set_feedback(0.5);
        c.set_feedforward(0.25);
        let mut x = vec![0.0f32; 500];
        x[0] = 1.0;
        c.process_block(&mut x);
        assert_eq!(x[0], 1.0);
        assert_eq!(x[100], 0.25 + 0.5);
        assert_eq!(x[200], 0.5 * 0.75);
        assert_eq!(x[300], 0.25 * 0.75);
    }

    #[test]
    fn clamps_nan_and_tail_to_zero() {
        let mut c = Comb::new(SR, 5.0);
        c.set_delay_samples(f32::NAN);
        c.set_delay_samples(1.0e9);
        assert!(c.delay_samples() <= 244.0);
        c.set_delay_samples(0.0);
        assert_eq!(c.delay_samples(), 2.0);
        c.set_feedback(f32::NAN);
        c.set_feedforward(f32::NAN);
        c.set_feedback(5.0);
        c.set_feedforward(-5.0);
        c.set_delay_ms(1.0);
        let peak = noise(1, 48_000, 1.0)
            .iter()
            .map(|v| c.process(*v).abs())
            .fold(0.0f32, f32::max);
        assert!(peak < 200.0, "{peak}");
        let mut last = 1.0;
        for _ in 0..(SR as usize * 30) {
            last = c.process(0.0);
        }
        assert_eq!(last, 0.0);
    }

    #[test]
    fn process_is_chunk_invariant() {
        let x = noise(2, 3_000, 0.7);
        assert_chunk_invariant_mono(
            || {
                let mut c = Comb::new(SR, 20.0);
                c.set_delay_samples(33.3);
                c.set_feedback(0.7);
                c.set_feedforward(0.3);
                c
            },
            |c, b| c.process_block(b),
            &x,
        );
    }

    proptest::proptest! {
        #[test]
        fn finite_and_bounded_for_any_parameters(
            d in 0.0f32..2000.0, fb in -1.0f32..1.0, ff in -1.0f32..1.0, seed in 0u64..300
        ) {
            let mut c = Comb::new(SR, 20.0);
            c.set_delay_samples(d);
            c.set_feedback(fb);
            c.set_feedforward(ff);
            let mut x = noise(seed, 2048, 1.0);
            c.process_block(&mut x);
            proptest::prop_assert!(x.iter().all(|v| v.is_finite() && v.abs() < 400.0));
        }
    }
}
