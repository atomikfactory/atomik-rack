//! Stereo-linked peak-linked upward leveller.
//!
//! A loudness leveller, not a downward compressor. The stereo peak `p = max(|l|, |r|)` is
//! followed by an instant-attack peak detector (decay [`PEAK_DECAY_MS`]) and drives one gain
//! for both channels, so the stereo image is kept. The target gain of the law is
//!
//! ```text
//! g(p) = (C / p) ^ ratio          (limited to max_boost)
//! ```
//!
//! with `C` the ceiling, so a steady level `p` leaves at `C^ratio * p^(1 - ratio)`: ratio 0 is a
//! clean pass-through (bit exact, the block is skipped), ratio 1 holds everything at the ceiling,
//! and in between the quiet material is raised (the boost is largest for vanishing levels and
//! grows with the ratio, capped by `max_boost`) while loud material is held near the ceiling.
//! The gain then follows its target as a one-pole with separate times: the attack time applies
//! while the gain falls (loud material arrives), the release time while it rises. Both are time
//! constants (63 % of the way). Only `+ - * /`, [`crate::log2_det`] and [`crate::exp2_det`] are
//! used, so the result is bit-identical on every platform.

use crate::{exp2_det, exp2_det_f64, flush_denormal, log2_det};

/// Decay time constant of the peak detector in milliseconds.
pub const PEAK_DECAY_MS: f32 = 5.0;
/// Smallest peak the law looks at (about -120 dBFS); below it the gain is at its cap.
const PEAK_FLOOR: f32 = 1.0e-6;

/// Stereo peak-linked upward leveller.
#[derive(Debug, Clone)]
pub struct Leveller {
    sample_rate: f32,
    ratio: f32,
    ceiling: f32,
    ceiling_log2: f32,
    max_boost: f32,
    attack: f32,
    release: f32,
    peak_decay: f32,
    peak: f32,
    gain: f32,
}

fn pole(time_ms: f32, sample_rate: f32) -> f32 {
    let n = f64::from(time_ms.max(0.0)) * 0.001 * f64::from(sample_rate);
    if n > 1e-3 {
        (1.0 - exp2_det_f64(-std::f64::consts::LOG2_E / n)) as f32
    } else {
        1.0
    }
}

impl Leveller {
    /// A pass-through leveller (ratio 0) with a ceiling of -6 dB, attack 10 ms, release 150 ms
    /// and a boost cap of 30 dB.
    pub fn new(sample_rate: f32) -> Self {
        let mut l = Self {
            sample_rate,
            ratio: 0.0,
            ceiling: 0.0,
            ceiling_log2: 0.0,
            max_boost: 1.0,
            attack: 0.0,
            release: 0.0,
            peak_decay: pole(PEAK_DECAY_MS, sample_rate),
            peak: 0.0,
            gain: 1.0,
        };
        l.set_ceiling_db(-6.0);
        l.set_max_boost_db(30.0);
        l.set_attack_ms(10.0);
        l.set_release_ms(150.0);
        l
    }

    /// Ratio control `0..=1` (clamped; NaN ignored): 0 is a clean pass-through.
    pub fn set_ratio(&mut self, ratio: f32) {
        if !ratio.is_nan() {
            self.ratio = ratio.clamp(0.0, 1.0);
        }
    }

    /// Ceiling in dB (clamped to -60..=0).
    pub fn set_ceiling_db(&mut self, db: f32) {
        if db.is_nan() {
            return;
        }
        self.ceiling_log2 = db.clamp(-60.0, 0.0) * 0.166_096_4;
        self.ceiling = exp2_det(self.ceiling_log2);
    }

    /// Cap of the boost in dB (clamped to 0..=60).
    pub fn set_max_boost_db(&mut self, db: f32) {
        if !db.is_nan() {
            self.max_boost = exp2_det(db.clamp(0.0, 60.0) * 0.166_096_4);
        }
    }

    /// Attack time constant in ms (applies while the gain falls).
    pub fn set_attack_ms(&mut self, ms: f32) {
        if !ms.is_nan() {
            self.attack = pole(ms.clamp(0.0, 5_000.0), self.sample_rate);
        }
    }

    /// Release time constant in ms (applies while the gain rises).
    pub fn set_release_ms(&mut self, ms: f32) {
        if !ms.is_nan() {
            self.release = pole(ms.clamp(0.0, 10_000.0), self.sample_rate);
        }
    }

    /// Clears the detector and the gain.
    pub fn reset(&mut self) {
        self.peak = 0.0;
        self.gain = 1.0;
    }

    /// The gain applied to the last sample (linear).
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Processes one stereo sample.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.ratio == 0.0 {
            self.gain = 1.0;
            return (l, r);
        }
        let p = l.abs().max(r.abs());
        self.peak = flush_denormal(p.max(self.peak * (1.0 - self.peak_decay)));
        let lp = log2_det(self.peak.max(PEAK_FLOOR));
        let target = exp2_det(self.ratio * (self.ceiling_log2 - lp)).min(self.max_boost);
        let c = if target < self.gain {
            self.attack
        } else {
            self.release
        };
        self.gain += c * (target - self.gain);
        (l * self.gain, r * self.gain)
    }

    /// Processes a stereo block in place (identical to calling [`Leveller::process`] per sample).
    pub fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let (x, y) = self.process(*a, *b);
            *a = x;
            *b = y;
        }
    }

    /// The static output level of the law for a steady peak `p` (linear): `min(g, boost) * p`.
    pub fn static_out(&self, p: f32) -> f32 {
        if self.ratio == 0.0 {
            return p;
        }
        let lp = log2_det(p.max(PEAK_FLOOR));
        exp2_det(self.ratio * (self.ceiling_log2 - lp)).min(self.max_boost) * p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{assert_chunk_invariant_stereo, db, noise, sine};

    const SR: f32 = 48_000.0;

    fn dc(l: &mut Leveller, level: f32, n: usize) -> f32 {
        let mut y = 0.0;
        for _ in 0..n {
            y = l.process(level, level).0;
        }
        y
    }

    #[test]
    fn ratio_zero_is_a_bit_exact_pass_through() {
        let mut l = Leveller::new(SR);
        let x = noise(1, 4_000, 1.0);
        let y = noise(2, 4_000, 1.0);
        for (a, b) in x.iter().zip(&y) {
            let (p, q) = l.process(*a, *b);
            assert_eq!(p.to_bits(), a.to_bits());
            assert_eq!(q.to_bits(), b.to_bits());
        }
        assert_eq!(l.static_out(0.3), 0.3);
    }

    #[test]
    fn steady_levels_follow_the_law_quiet_raised_loud_held() {
        let mut l = Leveller::new(SR);
        l.set_max_boost_db(60.0);
        l.set_ceiling_db(-6.0);
        let c = 10f32.powf(-6.0 / 20.0);
        for ratio in [0.25f32, 0.5, 0.8, 1.0] {
            l.set_ratio(ratio);
            for level in [0.003f32, 0.03, 0.2, 0.9] {
                l.reset();
                let y = dc(&mut l, level, 48_000);
                let want = c.powf(ratio) * level.powf(1.0 - ratio);
                assert!(
                    (db(f64::from(y)) - db(f64::from(want))).abs() < 0.05,
                    "ratio {ratio} level {level}: {y} vs {want}"
                );
                assert!((db(f64::from(l.static_out(level))) - db(f64::from(want))).abs() < 0.05);
            }
        }
        // Quiet material is raised, loud material is held near the ceiling.
        l.set_ratio(0.9);
        l.reset();
        assert!(dc(&mut l, 0.01, 48_000) > 0.01 * 10.0);
        l.reset();
        let loud = dc(&mut l, 1.0, 48_000);
        assert!((loud - c).abs() / c < 0.1, "{loud}");
    }

    #[test]
    fn boost_is_capped_at_max_boost() {
        let mut l = Leveller::new(SR);
        l.set_ratio(1.0);
        l.set_max_boost_db(20.0);
        let y = dc(&mut l, 1e-5, 48_000);
        assert!((db(f64::from(y / 1e-5)) - 20.0).abs() < 0.05);
        let y = dc(&mut l, 0.0, 48_000);
        assert_eq!(y, 0.0);
        assert!(l.gain().is_finite());
    }

    fn time_to_fraction(l: &mut Leveller, from: f32, to: f32, frac: f32) -> f32 {
        l.reset();
        dc(l, from, 96_000);
        let g0 = l.gain();
        let g1 = {
            let mut probe = l.clone();
            dc(&mut probe, to, 192_000);
            probe.gain()
        };
        let thr = g0 + (g1 - g0) * frac;
        let mut n = 0;
        loop {
            n += 1;
            l.process(to, to);
            let g = l.gain();
            if (g1 > g0 && g >= thr) || (g1 < g0 && g <= thr) || n > 500_000 {
                return n as f32 / SR * 1000.0;
            }
        }
    }

    #[test]
    fn attack_and_release_times_are_within_ten_percent() {
        let frac = 1.0 - (-1.0f32).exp(); // 63.2 %
        for att in [5.0f32, 20.0, 80.0] {
            let mut l = Leveller::new(SR);
            l.set_ratio(0.7);
            l.set_attack_ms(att);
            let t = time_to_fraction(&mut l, 0.05, 0.8, frac);
            assert!((t - att).abs() / att < 0.1, "attack {att}: {t}");
        }
        for rel in [100.0f32, 300.0, 1000.0] {
            let mut l = Leveller::new(SR);
            l.set_ratio(0.7);
            l.set_release_ms(rel);
            let t = time_to_fraction(&mut l, 0.8, 0.05, frac);
            assert!((t - rel).abs() / rel < 0.1, "release {rel}: {t}");
        }
    }

    #[test]
    fn stereo_link_keeps_the_image() {
        let mut l = Leveller::new(SR);
        l.set_ratio(0.8);
        let x = sine(300.0, f64::from(SR), 9_600, 0.8);
        for v in &x {
            let (a, b) = l.process(*v, *v * 0.25);
            assert!((b - a * 0.25).abs() <= 1e-6 * a.abs().max(1e-3), "{a} {b}");
        }
    }

    #[test]
    fn block_processing_is_chunk_invariant() {
        let a = noise(4, 3_000, 0.7);
        let b = noise(5, 3_000, 0.3);
        assert_chunk_invariant_stereo(
            || {
                let mut l = Leveller::new(SR);
                l.set_ratio(0.6);
                l
            },
            |l, x, y| l.process_block(x, y),
            &a,
            &b,
        );
    }

    #[test]
    fn setters_clamp_and_ignore_nan() {
        let mut l = Leveller::new(SR);
        l.set_ratio(f32::NAN);
        assert_eq!(l.static_out(0.5), 0.5);
        l.set_ratio(5.0);
        l.set_ceiling_db(f32::NAN);
        l.set_max_boost_db(f32::NAN);
        l.set_attack_ms(f32::NAN);
        l.set_release_ms(f32::NAN);
        l.set_ceiling_db(100.0);
        l.set_attack_ms(0.0);
        l.set_release_ms(0.0);
        let (a, b) = l.process(0.5, 0.5);
        assert!(a.is_finite() && b.is_finite());
    }

    proptest::proptest! {
        #[test]
        fn output_is_finite_and_bounded_for_any_parameters(
            ratio in 0.0f32..1.0, ceil in -60.0f32..0.0, boost in 0.0f32..60.0,
            att in 0.0f32..500.0, rel in 0.0f32..2000.0, seed in 0u64..500
        ) {
            let mut l = Leveller::new(SR);
            l.set_ratio(ratio);
            l.set_ceiling_db(ceil);
            l.set_max_boost_db(boost);
            l.set_attack_ms(att);
            l.set_release_ms(rel);
            let mut a = noise(seed, 1024, 4.0);
            let mut b = noise(seed + 1, 1024, 0.001);
            l.process_block(&mut a, &mut b);
            let cap = 4.0 * 1000.0 * 1.001;
            proptest::prop_assert!(a.iter().chain(&b).all(|v| v.is_finite() && v.abs() <= cap));
        }
    }
}
