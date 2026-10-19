//! Stereo chorus / flanger: a modulated fractional delay with feedback and a mix.
//!
//! Each channel is a [`crate::ModDelay`] (Hermite-interpolated) read at
//! `center + depth * lfo` milliseconds, where `lfo` is a sine; the right channel's LFO is
//! inverted by default (`invert_right`), so the two sides sweep against each other. The wet signal
//! is fed back (`feedback`, clamped to `+-0.95`) into the line together with the input, and the
//! output is the crossfade `(1 - mix) x + mix wet`: `mix = 0` is the exact dry signal. A long
//! centre with a shallow depth is a chorus; a centre of a millisecond or less with feedback is a
//! flanger. The delay is clamped to `2..=max` samples (the line's own limits).

use crate::det_trig::SineLfo;
use crate::{ModDelay, flush_denormal};

/// Largest feedback magnitude.
const MAX_FEEDBACK: f32 = 0.95;

/// Stereo chorus / flanger.
#[derive(Debug, Clone)]
pub struct Chorus {
    sample_rate: f32,
    lines: [ModDelay; 2],
    lfo: SineLfo,
    center_ms: f32,
    depth_ms: f32,
    feedback: f32,
    mix: f32,
    invert_right: bool,
    max_ms: f32,
}

impl Chorus {
    /// A chorus whose lines hold up to `max_ms` milliseconds (non-RT allocation). Defaults:
    /// centre 12 ms, depth 3 ms, rate 0.6 Hz, no feedback, mix 0.5, right LFO inverted.
    pub fn new(sample_rate: f32, max_ms: f32) -> Self {
        let max_samples = (max_ms.max(1.0) * 0.001 * sample_rate).ceil() as usize + 2;
        let mut lfo = SineLfo::new(0.0);
        lfo.set_rate(0.6, sample_rate);
        Self {
            sample_rate,
            lines: [ModDelay::new(max_samples), ModDelay::new(max_samples)],
            lfo,
            center_ms: 12.0,
            depth_ms: 3.0,
            feedback: 0.0,
            mix: 0.5,
            invert_right: true,
            max_ms,
        }
    }

    /// LFO rate in Hz (clamped to 0..=20).
    pub fn set_rate_hz(&mut self, hz: f32) {
        self.lfo.set_rate(hz.clamp(0.0, 20.0), self.sample_rate);
    }

    /// Delay centre in ms (clamped to `0..=max_ms`).
    pub fn set_center_ms(&mut self, ms: f32) {
        if !ms.is_nan() {
            self.center_ms = ms.clamp(0.0, self.max_ms);
        }
    }

    /// Peak delay deviation in ms (clamped to `0..=max_ms`).
    pub fn set_depth_ms(&mut self, ms: f32) {
        if !ms.is_nan() {
            self.depth_ms = ms.clamp(0.0, self.max_ms);
        }
    }

    /// Feedback (clamped to +-0.95).
    pub fn set_feedback(&mut self, fb: f32) {
        if !fb.is_nan() {
            self.feedback = fb.clamp(-MAX_FEEDBACK, MAX_FEEDBACK);
        }
    }

    /// Dry/wet crossfade `0..=1`.
    pub fn set_mix(&mut self, mix: f32) {
        if !mix.is_nan() {
            self.mix = mix.clamp(0.0, 1.0);
        }
    }

    /// Whether the right channel's LFO is inverted.
    pub fn set_invert_right(&mut self, invert: bool) {
        self.invert_right = invert;
    }

    /// Restarts the LFO at `phase` cycles.
    pub fn set_phase(&mut self, phase: f32) {
        self.lfo.set_phase(f64::from(phase));
    }

    /// Clears the delay lines.
    pub fn reset(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
    }

    /// Processes one stereo sample.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let s = self.lfo.next_sample();
        let k = 0.001 * f64::from(self.sample_rate);
        let dl = (f64::from(self.center_ms + self.depth_ms * s) * k) as f32;
        let sr_ = if self.invert_right { -s } else { s };
        let dr = (f64::from(self.center_ms + self.depth_ms * sr_) * k) as f32;
        let wl = self.lines[0].read(dl);
        let wr = self.lines[1].read(dr);
        self.lines[0].write(flush_denormal(l + self.feedback * wl));
        self.lines[1].write(flush_denormal(r + self.feedback * wr));
        (l + self.mix * (wl - l), r + self.mix * (wr - r))
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
    use crate::test_util::{assert_chunk_invariant_stereo, noise};

    const SR: f32 = 48_000.0;

    /// Instantaneous delay in samples seen by a slow ramp (Hermite is exact on a line).
    fn delay_track(c: &mut Chorus, n: usize) -> Vec<f64> {
        let slope = 1.0e-4f32;
        let mut l: Vec<f32> = (0..n).map(|i| i as f32 * slope).collect();
        let mut r = l.clone();
        c.process_block(&mut l, &mut r);
        (0..n)
            .map(|i| i as f64 - f64::from(l[i]) / f64::from(slope))
            .collect()
    }

    #[test]
    fn static_delay_is_exact_for_an_integer_delay() {
        let mut c = Chorus::new(SR, 50.0);
        c.set_depth_ms(0.0);
        c.set_center_ms(5.0); // 240 samples
        c.set_mix(1.0);
        let mut l = vec![0.0f32; 1000];
        l[0] = 1.0;
        let mut r = l.clone();
        c.process_block(&mut l, &mut r);
        assert_eq!(l[240], 1.0);
        assert_eq!(r[240], 1.0);
        assert!(l.iter().enumerate().all(|(i, v)| i == 240 || *v == 0.0));
    }

    #[test]
    fn delay_stays_within_center_plus_minus_depth_at_the_lfo_rate() {
        let mut c = Chorus::new(SR, 50.0);
        c.set_center_ms(10.0);
        c.set_depth_ms(2.0);
        c.set_rate_hz(4.0);
        c.set_mix(1.0);
        let d = delay_track(&mut c, 48_000);
        let (lo, hi) = d[1000..]
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        assert!(
            (lo - 384.0).abs() < 0.1 && (hi - 576.0).abs() < 0.1,
            "{lo}..{hi}"
        );
        let up = d[1000..]
            .windows(2)
            .filter(|w| w[0] < 480.0 && w[1] >= 480.0)
            .count();
        assert!(up.abs_diff(4) <= 1, "{up} cycles in 1 s");
    }

    #[test]
    fn right_channel_is_inverted_by_default() {
        let mut c = Chorus::new(SR, 50.0);
        c.set_mix(1.0);
        c.set_rate_hz(2.0);
        c.set_phase(0.1);
        let n = 20_000;
        let slope = 1.0e-4f32;
        let mut l: Vec<f32> = (0..n).map(|i| i as f32 * slope).collect();
        let mut r = l.clone();
        c.process_block(&mut l, &mut r);
        let i = 5_000;
        let dl = i as f64 - f64::from(l[i]) / f64::from(slope);
        let dr = i as f64 - f64::from(r[i]) / f64::from(slope);
        assert!(((dl - 576.0) + (dr - 576.0)).abs() < 0.2, "{dl} {dr}");
        assert!((dl - dr).abs() > 1.0);
        c.set_invert_right(false);
    }

    #[test]
    fn mix_zero_is_exact_dry_and_feedback_is_stable() {
        let mut c = Chorus::new(SR, 50.0);
        c.set_mix(0.0);
        c.set_feedback(0.9);
        for v in noise(1, 1_000, 1.0) {
            assert_eq!(c.process(v, v).0.to_bits(), v.to_bits());
        }
        let mut c = Chorus::new(SR, 50.0);
        c.set_feedback(9.0);
        c.set_center_ms(0.5);
        c.set_depth_ms(0.4);
        c.set_mix(1.0);
        let peak = noise(2, 96_000, 1.0)
            .iter()
            .map(|v| c.process(*v, *v).0.abs())
            .fold(0.0f32, f32::max);
        assert!(peak < 25.0, "{peak}");
        let mut last = 1.0;
        for _ in 0..(SR as usize * 20) {
            last = c.process(0.0, 0.0).0;
        }
        assert_eq!(last, 0.0);
    }

    #[test]
    fn setters_clamp_and_ignore_nan() {
        let mut c = Chorus::new(SR, 20.0);
        c.set_center_ms(f32::NAN);
        c.set_depth_ms(f32::NAN);
        c.set_feedback(f32::NAN);
        c.set_mix(f32::NAN);
        c.set_center_ms(1.0e6);
        c.set_depth_ms(1.0e6);
        c.set_rate_hz(f32::NAN);
        let (a, b) = c.process(0.5, 0.5);
        assert!(a.is_finite() && b.is_finite());
        c.reset();
    }

    #[test]
    fn process_is_chunk_invariant() {
        let a = noise(4, 3_000, 0.7);
        let b = noise(5, 3_000, 0.7);
        assert_chunk_invariant_stereo(
            || {
                let mut c = Chorus::new(SR, 30.0);
                c.set_rate_hz(3.0);
                c.set_feedback(0.5);
                c
            },
            |c, x, y| c.process_block(x, y),
            &a,
            &b,
        );
    }

    proptest::proptest! {
        #[test]
        fn finite_and_bounded_for_any_parameters(
            rate in 0.0f32..20.0, center in 0.0f32..50.0, depth in 0.0f32..50.0,
            fb in -1.0f32..1.0, mix in 0.0f32..1.0, inv in proptest::bool::ANY, seed in 0u64..300
        ) {
            let mut c = Chorus::new(SR, 50.0);
            c.set_rate_hz(rate);
            c.set_center_ms(center);
            c.set_depth_ms(depth);
            c.set_feedback(fb);
            c.set_mix(mix);
            c.set_invert_right(inv);
            let mut l = noise(seed, 1024, 1.0);
            let mut r = noise(seed + 9, 1024, 1.0);
            c.process_block(&mut l, &mut r);
            proptest::prop_assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 100.0));
        }
    }
}
