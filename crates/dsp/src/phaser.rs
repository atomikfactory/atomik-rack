//! Stereo phaser: DC blocker, 2/4/6 first-order all-pass poles swept by a sine LFO, feedback
//! from the last pole, output summed with the dry signal.
//!
//! Per channel the wet path is `x -> DC blocker -> (+ feedback) -> all-pass poles`; the output is
//! `x + mix/2 (wet - x)`, so `mix = 0` is the exact dry signal and `mix = 1` the equal sum of dry
//! and phase-shifted signal (notches where the pole chain turns the phase by an odd multiple of
//! 180 degrees). The pole frequency is `center * 2^(depth * lfo)` (the sweep is in octaves);
//! the coefficient `a = (t - 1)/(t + 1)`, `t = tan(pi f / fs)`, is refreshed every
//! [`CONTROL_INTERVAL`] samples from a deterministic sine/cosine (no libm). The right channel's
//! LFO is scaled by `1 - 2 stereo`, from in phase (`stereo = 0`) to inverted (`stereo = 1`).
//! Feedback is clamped to `+-0.95`, which keeps the loop stable (an all-pass chain has unit
//! gain, so the loop gain never exceeds the feedback).

use crate::det_trig::{SineLfo, cos_cycles, sin_cycles};
use crate::{exp2_det, flush_denormal};

/// Samples between coefficient refreshes.
pub const CONTROL_INTERVAL: u32 = 8;
/// Largest number of all-pass poles.
pub const MAX_STAGES: usize = 6;
/// DC blocker pole.
const DC_R: f32 = 0.995;
/// Largest feedback magnitude.
const MAX_FEEDBACK: f32 = 0.95;

#[derive(Debug, Clone, Copy, Default)]
struct Channel {
    dc_x: f32,
    dc_y: f32,
    ap_x: [f32; MAX_STAGES],
    ap_y: [f32; MAX_STAGES],
    last: f32,
    a: f32,
}

impl Channel {
    #[inline]
    fn process(&mut self, x: f32, stages: usize, feedback: f32) -> f32 {
        let dc = x - self.dc_x + DC_R * self.dc_y;
        self.dc_x = x;
        self.dc_y = flush_denormal(dc);
        let mut v = dc + feedback * self.last;
        for s in 0..stages {
            let y = self.a * v + self.ap_x[s] - self.a * self.ap_y[s];
            self.ap_x[s] = v;
            self.ap_y[s] = flush_denormal(y);
            v = y;
        }
        self.last = flush_denormal(v);
        v
    }
}

/// Stereo phaser.
#[derive(Debug, Clone)]
pub struct Phaser {
    sample_rate: f32,
    stages: usize,
    center_hz: f32,
    depth_oct: f32,
    feedback: f32,
    mix: f32,
    stereo: f32,
    lfo: SineLfo,
    counter: u32,
    ch: [Channel; 2],
}

impl Phaser {
    /// A phaser with 4 poles, centre 1 kHz, depth 1 octave, rate 0.5 Hz, no feedback, mix 1.
    pub fn new(sample_rate: f32) -> Self {
        let mut p = Self {
            sample_rate,
            stages: 4,
            center_hz: 1000.0,
            depth_oct: 1.0,
            feedback: 0.0,
            mix: 1.0,
            stereo: 0.0,
            lfo: SineLfo::new(0.0),
            counter: 0,
            ch: [Channel::default(); 2],
        };
        p.lfo.set_rate(0.5, sample_rate);
        p.refresh(0.0);
        p
    }

    /// Number of all-pass poles: 2, 4 or 6 (other values round to the nearest even count).
    pub fn set_stages(&mut self, n: usize) {
        self.stages = (n.clamp(2, MAX_STAGES) / 2) * 2;
    }

    /// LFO rate in Hz.
    pub fn set_rate_hz(&mut self, hz: f32) {
        self.lfo.set_rate(hz.clamp(0.0, 20.0), self.sample_rate);
    }

    /// Centre frequency of the sweep in Hz (clamped to 50..=8000).
    pub fn set_center_hz(&mut self, hz: f32) {
        if !hz.is_nan() {
            self.center_hz = hz.clamp(50.0, 8000.0);
        }
    }

    /// Sweep depth in octaves each side of the centre (clamped to 0..=3).
    pub fn set_depth_oct(&mut self, oct: f32) {
        if !oct.is_nan() {
            self.depth_oct = oct.clamp(0.0, 3.0);
        }
    }

    /// Feedback (clamped to +-0.95).
    pub fn set_feedback(&mut self, fb: f32) {
        if !fb.is_nan() {
            self.feedback = fb.clamp(-MAX_FEEDBACK, MAX_FEEDBACK);
        }
    }

    /// Dry/phased balance `0..=1` (0 is the exact dry signal).
    pub fn set_mix(&mut self, mix: f32) {
        if !mix.is_nan() {
            self.mix = mix.clamp(0.0, 1.0);
        }
    }

    /// Right-channel LFO scale from in phase (0) to inverted (1).
    pub fn set_stereo(&mut self, stereo: f32) {
        if !stereo.is_nan() {
            self.stereo = stereo.clamp(0.0, 1.0);
        }
    }

    /// Restarts the LFO at `phase` cycles.
    pub fn set_phase(&mut self, phase: f32) {
        self.lfo.set_phase(f64::from(phase));
    }

    /// Clears all state and restarts the control counter (the LFO phase is kept).
    pub fn reset(&mut self) {
        self.ch = [Channel::default(); 2];
        self.counter = 0;
    }

    fn pole(&self, lfo: f32) -> f32 {
        let f = self.center_hz * exp2_det(self.depth_oct * lfo);
        let f = f.clamp(20.0, 0.45 * self.sample_rate);
        let x = f64::from(f) / f64::from(self.sample_rate) * 0.5; // pi f / fs in half turns
        let t = sin_cycles(x) / cos_cycles(x);
        // H(z) = (a + z^-1) / (1 + a z^-1) turns the phase by 90 degrees at `f` for a = (t - 1) / (t + 1)
        // (the opposite sign puts the break at fs/2 - f).
        ((t - 1.0) / (t + 1.0)) as f32
    }

    fn refresh(&mut self, lfo: f32) {
        self.ch[0].a = self.pole(lfo);
        self.ch[1].a = self.pole(lfo * (1.0 - 2.0 * self.stereo));
    }

    /// Processes one stereo sample.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let lfo = self.lfo.next_sample();
        if self.counter == 0 {
            self.refresh(lfo);
        }
        self.counter += 1;
        if self.counter == CONTROL_INTERVAL {
            self.counter = 0;
        }
        let wl = self.ch[0].process(l, self.stages, self.feedback);
        let wr = self.ch[1].process(r, self.stages, self.feedback);
        let h = 0.5 * self.mix;
        (l + h * (wl - l), r + h * (wr - r))
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

    /// Analytic gain in dB of the static phaser (depth 0, no feedback) at `f` Hz.
    fn analytic_db(p: &Phaser, f: f64) -> f64 {
        let a = f64::from(p.pole(0.0));
        let w = std::f64::consts::TAU * f / f64::from(SR);
        let z = (w.cos(), -w.sin()); // e^{-jw}
        let cmul = |u: (f64, f64), v: (f64, f64)| (u.0 * v.0 - u.1 * v.1, u.0 * v.1 + u.1 * v.0);
        let cdiv = |u: (f64, f64), v: (f64, f64)| {
            let d = v.0 * v.0 + v.1 * v.1;
            ((u.0 * v.0 + u.1 * v.1) / d, (u.1 * v.0 - u.0 * v.1) / d)
        };
        let ap = cdiv((a + z.0, z.1), (1.0 + a * z.0, a * z.1));
        let mut h = (1.0, 0.0);
        for _ in 0..p.stages {
            h = cmul(h, ap);
        }
        let dc = cdiv(
            (1.0 - z.0, -z.1),
            (1.0 - f64::from(DC_R) * z.0, -f64::from(DC_R) * z.1),
        );
        let wet = cmul(h, dc);
        let s = (0.5 * (1.0 + wet.0), 0.5 * wet.1);
        db((s.0 * s.0 + s.1 * s.1).sqrt())
    }

    fn measured_db(p: &mut Phaser, f: f64) -> f64 {
        let n = 48_000;
        let x = sine(f, f64::from(SR), n, 0.5);
        let mut l = x.clone();
        let mut r = x;
        p.reset();
        p.process_block(&mut l, &mut r);
        db(tone_amplitude(&l[24_000..], f, f64::from(SR)) / 0.5)
    }

    #[test]
    fn static_response_matches_the_analytic_allpass_sum() {
        for stages in [2usize, 4, 6] {
            let mut p = Phaser::new(SR);
            p.set_stages(stages);
            p.set_depth_oct(0.0);
            p.set_center_hz(900.0);
            for f in [150.0f64, 400.0, 700.0, 1300.0, 2500.0, 6000.0] {
                let got = measured_db(&mut p, f);
                let want = analytic_db(&p, f);
                assert!(
                    (got - want).abs() < 0.15,
                    "{stages} stages at {f}: {got} vs {want}"
                );
            }
        }
    }

    #[test]
    fn a_two_pole_phaser_notches_at_its_centre_frequency() {
        // Each first-order all-pass turns the phase by 90 degrees at its break frequency, so two
        // of them turn it by 180 degrees at the centre and the dry plus phased sum has a notch
        // there (the DC blocker's small phase lead keeps it from being exact).
        for centre in [300.0f32, 1_000.0, 2_500.0, 5_000.0] {
            let mut p = Phaser::new(SR);
            p.set_stages(2);
            p.set_depth_oct(0.0);
            p.set_center_hz(centre);
            let at = measured_db(&mut p, f64::from(centre));
            let away = measured_db(&mut p, f64::from(centre) * 0.4);
            assert!(at < -18.0, "centre {centre}: {at} dB");
            assert!(away > -3.0, "centre {centre}: {away} dB at 0.4 of it");
        }
    }

    #[test]
    fn mix_zero_is_the_exact_dry_signal_and_stages_round_to_even() {
        let mut p = Phaser::new(SR);
        p.set_mix(0.0);
        p.set_feedback(0.9);
        let x = noise(1, 2_000, 1.0);
        for v in &x {
            let (a, b) = p.process(*v, *v);
            assert_eq!(a.to_bits(), v.to_bits());
            assert_eq!(b.to_bits(), v.to_bits());
        }
        p.set_stages(5);
        assert_eq!(p.stages, 4);
        p.set_stages(99);
        assert_eq!(p.stages, 6);
        p.set_stages(0);
        assert_eq!(p.stages, 2);
    }

    #[test]
    fn the_sweep_runs_at_the_lfo_rate() {
        let mut p = Phaser::new(SR);
        p.set_stages(4);
        p.set_depth_oct(1.0);
        p.set_center_hz(1000.0);
        p.set_rate_hz(2.0);
        let n = 480_000;
        let mut l = sine(1200.0, f64::from(SR), n, 0.5);
        let mut r = l.clone();
        p.process_block(&mut l, &mut r);
        // Envelope in 10 ms windows; a swept notch crosses the tone, so the envelope repeats once
        // per LFO cycle (0.5 s at 2 Hz): its autocorrelation peaks at a lag of one cycle and is
        // lower a quarter of a cycle away.
        let env: Vec<f64> = l
            .chunks(480)
            .skip(20)
            .map(|c| f64::from(c.iter().map(|v| v * v).sum::<f32>() / 480.0))
            .collect();
        let mean = env.iter().sum::<f64>() / env.len() as f64;
        let e: Vec<f64> = env.iter().map(|v| v - mean).collect();
        let ac = |lag: usize| {
            let num: f64 = (lag..e.len()).map(|i| e[i] * e[i - lag]).sum();
            num / e.iter().map(|v| v * v).sum::<f64>()
        };
        assert!(ac(50) > 0.9, "one cycle: {}", ac(50));
        assert!(
            ac(12) < ac(50) - 0.2,
            "a quarter cycle: {} against {}",
            ac(12),
            ac(50)
        );
        // The sweep is real: the level of the tone varies by several dB over a cycle.
        let (hi, lo) = (
            env.iter().fold(0.0f64, |m, v| m.max(*v)),
            env.iter().fold(f64::MAX, |m, v| m.min(*v)),
        );
        assert!(
            10.0 * (hi / lo).log10() > 6.0,
            "{}",
            10.0 * (hi / lo).log10()
        );
    }

    #[test]
    fn stereo_control_moves_the_right_channel_lfo() {
        let x = noise(3, 20_000, 0.5);
        let run = |stereo: f32| {
            let mut p = Phaser::new(SR);
            p.set_stereo(stereo);
            p.set_phase(0.25);
            let mut l = x.clone();
            let mut r = x.clone();
            p.process_block(&mut l, &mut r);
            (l, r)
        };
        let (l0, r0) = run(0.0);
        assert!(l0.iter().zip(&r0).all(|(a, b)| a.to_bits() == b.to_bits()));
        let (l1, r1) = run(1.0);
        assert!(l1.iter().zip(&r1).any(|(a, b)| (a - b).abs() > 0.01));
    }

    #[test]
    fn feedback_is_stable_bounded_and_the_tail_decays_to_zero() {
        let mut p = Phaser::new(SR);
        p.set_feedback(5.0);
        p.set_depth_oct(0.5);
        p.set_rate_hz(3.0);
        let x = noise(2, 48_000, 1.0);
        let peak = x
            .iter()
            .map(|v| p.process(*v, *v).0.abs())
            .fold(0.0f32, f32::max);
        assert!(peak.is_finite() && peak < 25.0, "{peak}");
        let mut last = 1.0;
        for _ in 0..(SR as usize * 20) {
            last = p.process(0.0, 0.0).0;
        }
        assert_eq!(last, 0.0, "denormal flush reaches exact zero");
    }

    #[test]
    fn setters_clamp_and_ignore_nan() {
        let mut p = Phaser::new(SR);
        p.set_center_hz(f32::NAN);
        p.set_depth_oct(f32::NAN);
        p.set_feedback(f32::NAN);
        p.set_mix(f32::NAN);
        p.set_stereo(f32::NAN);
        p.set_rate_hz(f32::NAN);
        p.set_center_hz(1.0e9);
        p.set_depth_oct(100.0);
        let (a, b) = p.process(0.3, -0.3);
        assert!(a.is_finite() && b.is_finite());
    }

    #[test]
    fn process_is_chunk_invariant() {
        let a = noise(4, 3_000, 0.7);
        let b = noise(5, 3_000, 0.7);
        assert_chunk_invariant_stereo(
            || {
                let mut p = Phaser::new(SR);
                p.set_rate_hz(4.0);
                p.set_feedback(0.6);
                p.set_stereo(0.7);
                p
            },
            |p, x, y| p.process_block(x, y),
            &a,
            &b,
        );
    }

    proptest::proptest! {
        #[test]
        fn finite_and_bounded_for_any_parameters(
            stages in 0usize..9, rate in 0.0f32..20.0, center in 50.0f32..8000.0,
            depth in 0.0f32..3.0, fb in -1.0f32..1.0, mix in 0.0f32..1.0,
            stereo in 0.0f32..1.0, seed in 0u64..300
        ) {
            let mut p = Phaser::new(SR);
            p.set_stages(stages);
            p.set_rate_hz(rate);
            p.set_center_hz(center);
            p.set_depth_oct(depth);
            p.set_feedback(fb);
            p.set_mix(mix);
            p.set_stereo(stereo);
            let mut l = noise(seed, 1024, 1.0);
            let mut r = noise(seed + 9, 1024, 1.0);
            p.process_block(&mut l, &mut r);
            proptest::prop_assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 100.0));
        }
    }
}
