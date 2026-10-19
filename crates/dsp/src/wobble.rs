//! Wow and flutter: a stereo delay modulated by seeded smooth random LFOs.
//!
//! Each channel reads a [`crate::ModDelay`] at `center + depth * m` samples, where
//! `m = 0.6 wow + 0.25 flutter + 0.15 drift` lies in `-1..=1`: `wow` is a slow sine (default
//! 0.6 Hz), `flutter` a faster one (default 6 Hz) and `drift` a [`crate::noise::Wander`] smooth
//! random control. Phases and rate jitter (0.85 to 1.15 times) are drawn from the seed per channel,
//! so the two channels wobble independently, and everything depends on the seed and the sample
//! index only. The output is always the delayed signal: with `depth = 0` it is the input delayed by
//! exactly [`Wobble::center_delay_samples`] (the maximum depth plus two samples), so the latency
//! never changes with the depth. The delay is the source of the pitch wobble: its rate of change is
//! the pitch deviation.

use crate::ModDelay;
use crate::det_trig::SineLfo;
use crate::noise::Wander;
use crate::rng::{Rng, hash64};

/// One channel's modulation sources.
#[derive(Debug, Clone)]
struct Source {
    wow: SineLfo,
    flutter: SineLfo,
    drift: Wander,
    wow_jitter: f32,
    flutter_jitter: f32,
}

impl Source {
    fn new(seed: u64, sample_rate: f32) -> Self {
        let mut rng = Rng::new(seed);
        let wow = SineLfo::new(f64::from(rng.next_f32()));
        let flutter = SineLfo::new(f64::from(rng.next_f32()));
        Self {
            wow,
            flutter,
            drift: Wander::new(rng.next_u64(), sample_rate, 1.0),
            wow_jitter: 0.85 + 0.3 * rng.next_f32(),
            flutter_jitter: 0.85 + 0.3 * rng.next_f32(),
        }
    }

    #[inline]
    fn next_sample(&mut self) -> f32 {
        0.6 * self.wow.next_sample()
            + 0.25 * self.flutter.next_sample()
            + 0.15 * (2.0 * self.drift.next_sample() - 1.0)
    }
}

/// Stereo wow and flutter.
#[derive(Debug, Clone)]
pub struct Wobble {
    sample_rate: f32,
    seed: u64,
    lines: [ModDelay; 2],
    sources: [Source; 2],
    center: f32,
    max_depth: f32,
    depth: f32,
    wow_hz: f32,
    flutter_hz: f32,
}

impl Wobble {
    /// A wobble whose depth can reach `max_depth_ms` (non-RT allocation), seeded by `seed`.
    /// Defaults: depth 0, wow 0.6 Hz, flutter 6 Hz.
    pub fn new(sample_rate: f32, seed: u64, max_depth_ms: f32) -> Self {
        let max_depth =
            (f64::from(max_depth_ms.max(0.0)) * 0.001 * f64::from(sample_rate)).ceil() as f32;
        let center = max_depth + 2.0;
        let cap = (center + max_depth) as usize + 4;
        let mut w = Self {
            sample_rate,
            seed,
            lines: [ModDelay::new(cap), ModDelay::new(cap)],
            sources: Self::sources(seed, sample_rate),
            center,
            max_depth,
            depth: 0.0,
            wow_hz: 0.6,
            flutter_hz: 6.0,
        };
        w.set_wow_hz(0.6);
        w.set_flutter_hz(6.0);
        w
    }

    fn sources(seed: u64, sample_rate: f32) -> [Source; 2] {
        [
            Source::new(hash64(seed, 0x57, 0), sample_rate),
            Source::new(hash64(seed, 0x57, 1), sample_rate),
        ]
    }

    /// The constant delay of the centre in samples (the latency at zero depth).
    pub fn center_delay_samples(&self) -> f32 {
        self.center
    }

    /// Depth in ms (peak deviation of the delay; clamped to the constructor's maximum).
    pub fn set_depth_ms(&mut self, ms: f32) {
        if !ms.is_nan() {
            self.depth = ((f64::from(ms.max(0.0)) * 0.001 * f64::from(self.sample_rate)) as f32)
                .min(self.max_depth);
        }
    }

    /// Wow rate in Hz (clamped to `0.05..=5`); each channel jitters it by its own factor.
    pub fn set_wow_hz(&mut self, hz: f32) {
        if hz.is_nan() {
            return;
        }
        let hz = hz.clamp(0.05, 5.0);
        self.wow_hz = hz;
        for s in &mut self.sources {
            s.wow.set_rate(hz * s.wow_jitter, self.sample_rate);
            s.drift.set_rate_hz(hz * 1.7);
        }
    }

    /// Flutter rate in Hz (clamped to `1..=30`); each channel jitters it by its own factor.
    pub fn set_flutter_hz(&mut self, hz: f32) {
        if hz.is_nan() {
            return;
        }
        let hz = hz.clamp(1.0, 30.0);
        self.flutter_hz = hz;
        for s in &mut self.sources {
            s.flutter.set_rate(hz * s.flutter_jitter, self.sample_rate);
        }
    }

    /// Clears the delay lines (the modulation phases keep running).
    pub fn reset(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
    }

    /// Restores the whole start state: clears the delay lines and restarts every modulation
    /// source (phases, jitter and the random drift) from the seed, keeping the depth and rates,
    /// so a render after `restart` equals the first render.
    pub fn restart(&mut self) {
        self.reset();
        self.sources = Self::sources(self.seed, self.sample_rate);
        let (wow, flutter) = (self.wow_hz, self.flutter_hz);
        self.set_wow_hz(wow);
        self.set_flutter_hz(flutter);
    }

    /// Processes one stereo sample.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let dl = self.center + self.depth * self.sources[0].next_sample();
        let dr = self.center + self.depth * self.sources[1].next_sample();
        let yl = self.lines[0].read(dl);
        let yr = self.lines[1].read(dr);
        self.lines[0].write(l);
        self.lines[1].write(r);
        (yl, yr)
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
    use crate::test_util::{assert_chunk_invariant_stereo, noise, spectrum};

    const SR: f32 = 48_000.0;

    /// The delay seen by each channel for a slow ramp input.
    fn delay_track(w: &mut Wobble, n: usize) -> (Vec<f64>, Vec<f64>) {
        let slope = 1.0e-5f32;
        let mut l: Vec<f32> = (0..n).map(|i| i as f32 * slope).collect();
        let mut r = l.clone();
        w.process_block(&mut l, &mut r);
        let d = |y: &[f32]| {
            (0..n)
                .map(|i| i as f64 - f64::from(y[i]) / f64::from(slope))
                .collect()
        };
        (d(&l), d(&r))
    }

    #[test]
    fn zero_depth_is_an_exact_fixed_delay() {
        let mut w = Wobble::new(SR, 1, 2.0);
        let c = w.center_delay_samples() as usize;
        assert_eq!(c, 98); // ceil(2 ms) = 96 samples, plus 2
        let mut l = vec![0.0f32; 400];
        l[0] = 1.0;
        let mut r = l.clone();
        w.process_block(&mut l, &mut r);
        assert_eq!(l[c], 1.0);
        assert_eq!(r[c], 1.0);
        assert!(l.iter().enumerate().all(|(i, v)| i == c || *v == 0.0));
    }

    #[test]
    fn delay_deviation_is_bounded_by_depth_and_uses_most_of_it() {
        let mut w = Wobble::new(SR, 2, 3.0);
        w.set_depth_ms(1.0); // 48 samples
        let n = 480_000;
        let (dl, dr) = delay_track(&mut w, n);
        let c = f64::from(w.center_delay_samples());
        for d in [&dl, &dr] {
            let dev = d[2_000..].iter().fold(0.0f64, |a, v| a.max((v - c).abs()));
            assert!(dev <= 48.05, "{dev}");
            assert!(dev > 24.0, "uses the depth: {dev}");
        }
    }

    #[test]
    fn modulation_is_slow_and_channels_are_independent() {
        let mut w = Wobble::new(SR, 3, 3.0);
        w.set_depth_ms(2.0);
        let n = 1 << 18;
        let (dl, dr) = delay_track(&mut w, n);
        let c = f64::from(w.center_delay_samples());
        let ml: Vec<f32> = dl[1_000..1_000 + 65_536]
            .iter()
            .map(|v| (v - c) as f32)
            .collect();
        let sp = spectrum(&ml);
        let bin = f64::from(SR) / 65_536.0;
        let low: f64 = sp
            .iter()
            .enumerate()
            .filter(|(k, _)| *k as f64 * bin < 20.0)
            .map(|(_, v)| v * v)
            .sum();
        let all: f64 = sp.iter().map(|v| v * v).sum();
        assert!(low / all > 0.99, "{}", low / all);
        let (a, b): (Vec<f64>, Vec<f64>) = (
            dl[1_000..].iter().map(|v| v - c).collect(),
            dr[1_000..].iter().map(|v| v - c).collect(),
        );
        let dot: f64 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        let na: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
        let nb: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!(
            (dot / (na * nb)).abs() < 0.6,
            "channels correlate: {}",
            dot / (na * nb)
        );
    }

    #[test]
    fn seed_decides_everything_and_setters_clamp() {
        let run = |seed: u64| {
            let mut w = Wobble::new(SR, seed, 2.0);
            w.set_depth_ms(1.5);
            let mut l = noise(1, 5_000, 0.5);
            let mut r = l.clone();
            w.process_block(&mut l, &mut r);
            (l, r)
        };
        assert_eq!(run(4), run(4));
        assert_ne!(run(4), run(5));
        let mut w = Wobble::new(SR, 1, 2.0);
        w.set_depth_ms(f32::NAN);
        w.set_wow_hz(f32::NAN);
        w.set_flutter_hz(f32::NAN);
        w.set_depth_ms(1.0e6);
        w.set_wow_hz(1000.0);
        w.set_flutter_hz(0.0);
        let (a, b) = w.process(0.3, 0.3);
        assert!(a.is_finite() && b.is_finite());
        w.reset();
        assert_eq!(w.process(0.0, 0.0), (0.0, 0.0));
    }

    #[test]
    fn restart_restores_the_start_state_including_the_modulation_phases() {
        let mut w = Wobble::new(SR, 21, 2.0);
        w.set_depth_ms(1.5);
        w.set_wow_hz(1.3);
        w.set_flutter_hz(9.0);
        let x = noise(6, 6_000, 0.5);
        let run = |w: &mut Wobble| {
            let (mut l, mut r) = (x.clone(), x.clone());
            w.process_block(&mut l, &mut r);
            (l, r)
        };
        let first = run(&mut w);
        let second = run(&mut w);
        assert_ne!(first, second, "the phases keep running without a restart");
        w.restart();
        assert_eq!(run(&mut w), first, "restart equals the first render");
        // `reset` alone keeps the phases running (the older behaviour stays).
        w.reset();
        assert_ne!(run(&mut w), first);
    }

    #[test]
    fn process_is_chunk_invariant() {
        let a = noise(4, 3_000, 0.7);
        let b = noise(5, 3_000, 0.7);
        assert_chunk_invariant_stereo(
            || {
                let mut w = Wobble::new(SR, 9, 2.0);
                w.set_depth_ms(1.0);
                w
            },
            |w, x, y| w.process_block(x, y),
            &a,
            &b,
        );
    }

    proptest::proptest! {
        #[test]
        fn finite_and_bounded_for_any_parameters(
            depth in 0.0f32..10.0, wow in 0.0f32..10.0, flutter in 0.0f32..40.0, seed in 0u64..300
        ) {
            let mut w = Wobble::new(SR, seed, 3.0);
            w.set_depth_ms(depth);
            w.set_wow_hz(wow);
            w.set_flutter_hz(flutter);
            let mut l = noise(seed, 1024, 1.0);
            let mut r = noise(seed + 9, 1024, 1.0);
            w.process_block(&mut l, &mut r);
            proptest::prop_assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() <= 2.0));
        }
    }
}
