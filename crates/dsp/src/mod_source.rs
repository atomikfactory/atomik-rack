//! Building blocks of the Mod Source device: the LFO wave shapes and a deterministic ADSR.
//!
//! Everything here is a pure function of its arguments (phase, morph, random values) and uses
//! only `+ - * /` and [`sin_cycles`](crate::sin_cycles) / [`exp2_det_f64`] / [`log2_det`], so the
//! output is bit-identical on every platform (no libm call).
//!
//! # Shapes and morph
//!
//! A shape is evaluated at a phase `p` in `0..1` (cycles) with a `morph` in `-1..=1`; `0` is the
//! neutral setting of every shape. All shapes return `-1..=1`.
//!
//! | Shape | `p = 0` | Morph |
//! |---|---|---|
//! | sine | 0, rising | **skew**: the positive half-cycle takes `s = 0.5 + 0.48 morph` of the cycle |
//! | triangle | 0, rising | the same skew `s` applied to a triangle |
//! | saw up | -1, rising to +1 | **curve**: `q = p (1-k) / (1 - k p)`, `k = 0.95 morph`, wave `2q - 1`; positive morph is slow-start (convex), negative is fast-start |
//! | saw down | +1, falling to -1 | the same curve, wave `1 - 2q` |
//! | pulse | +1 | **pulse width** `w = 0.5 + 0.45 morph`: +1 for `p < w`, -1 after |
//! | sample & hold | the cycle's value | **glide**: the first `max(morph, 0)` of each cycle slides linearly from the previous value to the new one |
//! | smooth random | the cycle's value | **curve** of the glide between this cycle's value and the next: `morph < 0` blends smoothstep towards linear, `morph > 0` blends towards smootherstep |
//!
//! The skew maps the phase through a two-piece linear warp `u(p)` (`u = p / (2 s)` for `p < s`,
//! `0.5 + (p - s) / (2 (1 - s))` after) and evaluates the plain sine or triangle at `u`, so a
//! skewed wave still crosses zero at `p = 0` and `p = s`.

use crate::{AdsrStage, exp2_det_f64, log2_det, sin_cycles};

/// The LFO shapes of the Mod Source, in the order of the device's `shape` choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModShape {
    /// Sine.
    Sine,
    /// Triangle.
    Triangle,
    /// Rising saw.
    SawUp,
    /// Falling saw.
    SawDown,
    /// Pulse with adjustable width.
    Pulse,
    /// Sample and hold (stepped random).
    SampleHold,
    /// Smooth random (interpolated random values).
    SmoothRandom,
}

impl ModShape {
    /// The shape of a choice index (out-of-range values give the nearest shape).
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Sine,
            1 => Self::Triangle,
            2 => Self::SawUp,
            3 => Self::SawDown,
            4 => Self::Pulse,
            5 => Self::SampleHold,
            _ => Self::SmoothRandom,
        }
    }

    /// Whether the shape needs the per-cycle random values.
    pub fn is_random(self) -> bool {
        matches!(self, Self::SampleHold | Self::SmoothRandom)
    }
}

/// The two-piece warp putting the positive half-cycle at fraction `s` of the cycle.
#[inline]
fn skew(p: f64, s: f64) -> f64 {
    if p < s {
        0.5 * p / s
    } else {
        0.5 + 0.5 * (p - s) / (1.0 - s)
    }
}

/// Triangle in cycles: 0 at 0, +1 at 0.25, 0 at 0.5, -1 at 0.75.
#[inline]
fn tri(u: f64) -> f64 {
    if u < 0.25 {
        4.0 * u
    } else if u < 0.75 {
        2.0 - 4.0 * u
    } else {
        4.0 * u - 4.0
    }
}

/// The saw curve `p (1-k) / (1 - k p)` (0 at 0, 1 at 1, monotone for `|k| < 1`).
#[inline]
fn bend(p: f64, k: f64) -> f64 {
    p * (1.0 - k) / (1.0 - k * p)
}

/// Value of `shape` at phase `p` (cycles, any finite value; only the fraction counts) with
/// `morph` (`-1..=1`, clamped). `rand` holds the random values of the previous, current and
/// next cycle (each `-1..=1`), used by the two random shapes only.
pub fn mod_shape(shape: ModShape, p: f64, morph: f32, rand: [f32; 3]) -> f32 {
    let p = if p.is_finite() { p - p.floor() } else { 0.0 };
    let m = f64::from(if morph.is_finite() {
        morph.clamp(-1.0, 1.0)
    } else {
        0.0
    });
    let v = match shape {
        ModShape::Sine => sin_cycles(skew(p, (0.5 + 0.48 * m).clamp(0.02, 0.98))),
        ModShape::Triangle => tri(skew(p, (0.5 + 0.48 * m).clamp(0.02, 0.98))),
        ModShape::SawUp => 2.0 * bend(p, 0.95 * m) - 1.0,
        ModShape::SawDown => 1.0 - 2.0 * bend(p, 0.95 * m),
        ModShape::Pulse => {
            if p < 0.5 + 0.45 * m {
                1.0
            } else {
                -1.0
            }
        }
        ModShape::SampleHold => {
            let g = m.max(0.0);
            let (prev, cur) = (f64::from(rand[0]), f64::from(rand[1]));
            if p < g {
                let t = p / g;
                prev * (1.0 - t) + cur * t
            } else {
                cur
            }
        }
        ModShape::SmoothRandom => {
            let (cur, next) = (f64::from(rand[1]), f64::from(rand[2]));
            let ss = p * p * (3.0 - 2.0 * p);
            let t = if m < 0.0 {
                ss * (1.0 + m) - m * p
            } else {
                let sss = p * p * p * (p * (6.0 * p - 15.0) + 10.0);
                ss * (1.0 - m) + sss * m
            };
            cur * (1.0 - t) + next * t
        }
    };
    (v as f32).clamp(-1.0, 1.0)
}

const ATTACK_OVERSHOOT: f64 = 0.3;
const FALL_UNDERSHOOT: f64 = 0.001;
const SUSTAIN_FOLLOW_MS: f32 = 5.0;

/// A deterministic ADSR envelope (`0..=1`) with the curves of [`crate::Adsr`] but computed with
/// [`exp2_det_f64`] / [`log2_det`] only, so it is bit-identical on every platform.
///
/// * Attack: one-pole rise towards 1.3, cut at 1, so it reaches 1 in the attack time from 0.
/// * Decay: one-pole fall towards `sustain - 0.001`, reaching `sustain` in the decay time from 1.
/// * Sustain: follows the sustain level with a 5 ms one-pole.
/// * Release: one-pole fall towards -0.001 at a fixed rate (0 in the release time from 1; shorter
///   from lower levels), then idle.
///
/// Every transition starts from the current level, so a retrigger is continuous.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModEnv {
    stage: AdsrStage,
    level: f32,
    sample_rate: f64,
    attack_ms: f32,
    decay_ms: f32,
    sustain: f32,
    release_ms: f32,
    attack_coef: f32,
    decay_coef: f32,
    release_coef: f32,
    follow_coef: f32,
}

impl ModEnv {
    /// An idle envelope: A 10 ms, D 200 ms, S 0.7, R 300 ms.
    pub fn new(sample_rate: f32) -> Self {
        let mut e = Self {
            stage: AdsrStage::Idle,
            level: 0.0,
            sample_rate: f64::from(sample_rate),
            attack_ms: 10.0,
            decay_ms: 200.0,
            sustain: 0.7,
            release_ms: 300.0,
            attack_coef: 0.0,
            decay_coef: 0.0,
            release_coef: 0.0,
            follow_coef: 0.0,
        };
        e.update();
        e
    }

    fn samples(&self, ms: f32) -> f64 {
        (f64::from(ms) * 0.001 * self.sample_rate).max(1.0)
    }

    /// `ratio^(1/n)` for `0 < ratio < 1`.
    fn root(ratio: f64, n: f64) -> f32 {
        exp2_det_f64(f64::from(log2_det(ratio as f32)) / n) as f32
    }

    fn update(&mut self) {
        let a = self.samples(self.attack_ms);
        self.attack_coef = Self::root(ATTACK_OVERSHOOT / (1.0 + ATTACK_OVERSHOOT), a);
        let d = self.samples(self.decay_ms);
        let s = f64::from(self.sustain);
        self.decay_coef = Self::root(FALL_UNDERSHOOT / (1.0 - s + FALL_UNDERSHOOT), d);
        let r = self.samples(self.release_ms);
        self.release_coef = Self::root(FALL_UNDERSHOOT / (1.0 + FALL_UNDERSHOOT), r);
        let f = self.samples(SUSTAIN_FOLLOW_MS);
        self.follow_coef = exp2_det_f64(-std::f64::consts::LOG2_E / f) as f32;
    }

    /// Sets the sample rate (times stay in ms).
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = f64::from(sample_rate);
        self.update();
    }

    /// Sets attack, decay (ms), sustain (0..=1) and release (ms) in one coefficient update.
    pub fn set_adsr(&mut self, attack_ms: f32, decay_ms: f32, sustain: f32, release_ms: f32) {
        let ok = |v: f32, d: f32| if v.is_finite() { v } else { d };
        self.attack_ms = ok(attack_ms, 10.0).max(0.0);
        self.decay_ms = ok(decay_ms, 200.0).max(0.0);
        self.sustain = ok(sustain, 0.7).clamp(0.0, 1.0);
        self.release_ms = ok(release_ms, 300.0).max(0.0);
        self.update();
    }

    /// Starts (or retriggers) the attack from the current level.
    pub fn gate_on(&mut self) {
        self.stage = AdsrStage::Attack;
    }

    /// Starts the release from the current level (no-op when idle).
    pub fn gate_off(&mut self) {
        if self.stage != AdsrStage::Idle {
            self.stage = AdsrStage::Release;
        }
    }

    /// Back to idle at level 0.
    pub fn reset(&mut self) {
        self.stage = AdsrStage::Idle;
        self.level = 0.0;
    }

    /// Current stage (never `Fade`).
    pub fn stage(&self) -> AdsrStage {
        self.stage
    }

    /// Current level.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Advances one sample and returns the level.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        match self.stage {
            AdsrStage::Idle | AdsrStage::Fade => {}
            AdsrStage::Attack => {
                let target = (1.0 + ATTACK_OVERSHOOT) as f32;
                self.level = target + (self.level - target) * self.attack_coef;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = AdsrStage::Decay;
                }
            }
            AdsrStage::Decay => {
                let target = self.sustain - FALL_UNDERSHOOT as f32;
                self.level = target + (self.level - target) * self.decay_coef;
                if self.level <= self.sustain {
                    self.stage = AdsrStage::Sustain;
                }
            }
            AdsrStage::Sustain => {
                self.level = self.sustain + (self.level - self.sustain) * self.follow_coef;
                if (self.level - self.sustain).abs() < 1e-7 {
                    self.level = self.sustain;
                }
            }
            AdsrStage::Release => {
                let target = -FALL_UNDERSHOOT as f32;
                self.level = target + (self.level - target) * self.release_coef;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = AdsrStage::Idle;
                }
            }
        }
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const Z: [f32; 3] = [0.0; 3];

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn sine_and_triangle_hit_known_phases() {
        for (p, want) in [(0.0, 0.0), (0.25, 1.0), (0.5, 0.0), (0.75, -1.0)] {
            assert!(
                close(mod_shape(ModShape::Sine, p, 0.0, Z), want),
                "sine {p}"
            );
            assert!(
                close(mod_shape(ModShape::Triangle, p, 0.0, Z), want),
                "tri {p}"
            );
        }
        assert!(close(mod_shape(ModShape::Triangle, 0.125, 0.0, Z), 0.5));
        assert!(close(mod_shape(ModShape::Triangle, 0.875, 0.0, Z), -0.5));
    }

    #[test]
    fn skew_moves_the_positive_half() {
        // morph 1: s = 0.98, so the zero crossing is near the end of the cycle.
        let v = mod_shape(ModShape::Sine, 0.9, 1.0, Z);
        assert!(v > 0.0, "{v}");
        let w = mod_shape(ModShape::Sine, 0.1, -1.0, Z);
        assert!(w < 0.0, "{w}");
        // The peak is at s / 2.
        assert!(close(mod_shape(ModShape::Sine, 0.49, 1.0, Z), 1.0));
    }

    #[test]
    fn saws_run_between_the_extremes_and_bend_with_morph() {
        assert!(close(mod_shape(ModShape::SawUp, 0.0, 0.0, Z), -1.0));
        assert!(close(mod_shape(ModShape::SawUp, 0.5, 0.0, Z), 0.0));
        assert!(mod_shape(ModShape::SawUp, 0.999_999, 0.0, Z) > 0.999);
        assert!(close(mod_shape(ModShape::SawDown, 0.0, 0.0, Z), 1.0));
        assert!(close(mod_shape(ModShape::SawDown, 0.5, 0.0, Z), 0.0));
        // Positive morph is slow-start (below the straight line), negative fast-start.
        assert!(mod_shape(ModShape::SawUp, 0.5, 1.0, Z) < -0.5);
        assert!(mod_shape(ModShape::SawUp, 0.5, -1.0, Z) > 0.25);
        // Down is the mirror of up.
        for p in [0.1, 0.3, 0.7] {
            let up = mod_shape(ModShape::SawUp, p, 0.4, Z);
            let down = mod_shape(ModShape::SawDown, p, 0.4, Z);
            assert!(close(up, -down));
        }
    }

    #[test]
    fn pulse_width_follows_morph() {
        let high = |m: f32| {
            (0..1000)
                .filter(|i| mod_shape(ModShape::Pulse, f64::from(*i) / 1000.0, m, Z) > 0.0)
                .count()
        };
        assert_eq!(high(0.0), 500);
        assert_eq!(high(1.0), 950);
        assert_eq!(high(-1.0), 50);
    }

    #[test]
    fn sample_hold_and_smooth_random_use_the_cycle_values() {
        let r = [-0.5, 0.25, 0.75];
        assert!(close(mod_shape(ModShape::SampleHold, 0.0, 0.0, r), 0.25));
        assert!(close(mod_shape(ModShape::SampleHold, 0.9, 0.0, r), 0.25));
        // Glide: starts at the previous value and arrives at g.
        assert!(close(mod_shape(ModShape::SampleHold, 0.0, 0.5, r), -0.5));
        assert!(close(mod_shape(ModShape::SampleHold, 0.25, 0.5, r), -0.125));
        assert!(close(mod_shape(ModShape::SampleHold, 0.6, 0.5, r), 0.25));
        assert!(close(mod_shape(ModShape::SmoothRandom, 0.0, 0.0, r), 0.25));
        assert!(close(mod_shape(ModShape::SmoothRandom, 0.5, 0.0, r), 0.5));
        assert!(close(
            mod_shape(ModShape::SmoothRandom, 0.999_999, 0.0, r),
            0.75
        ));
        // Morph -1 is linear: a quarter of the way.
        assert!(close(
            mod_shape(ModShape::SmoothRandom, 0.25, -1.0, r),
            0.375
        ));
    }

    #[test]
    fn shapes_stay_within_one_for_any_input() {
        let r = [-1.0, 1.0, -1.0];
        for s in 0..7usize {
            for mi in -4..=4 {
                for i in 0..=400 {
                    let v = mod_shape(
                        ModShape::from_index(s),
                        f64::from(i) / 400.0 + s as f64,
                        mi as f32 / 4.0,
                        r,
                    );
                    assert!((-1.0..=1.0).contains(&v), "{s} {mi} {i} {v}");
                }
            }
        }
        assert_eq!(
            mod_shape(ModShape::Sine, f64::NAN, f32::NAN, Z).to_bits(),
            0
        );
    }

    #[test]
    fn mod_env_reaches_one_in_the_attack_time_and_releases_to_idle() {
        let mut e = ModEnv::new(48_000.0);
        e.set_adsr(10.0, 20.0, 0.5, 30.0);
        e.gate_on();
        let mut n = 0;
        while e.stage() == AdsrStage::Attack {
            e.next_sample();
            n += 1;
            assert!(n < 2000);
        }
        assert!((n - 480_i32).abs() <= 2, "{n}");
        assert!(close(e.level(), 1.0));
        for _ in 0..4800 {
            e.next_sample();
        }
        assert_eq!(e.stage(), AdsrStage::Sustain);
        assert!((e.level() - 0.5).abs() < 1e-4);
        e.gate_off();
        for _ in 0..4800 {
            e.next_sample();
        }
        assert_eq!(e.stage(), AdsrStage::Idle);
        assert_eq!(e.level().to_bits(), 0);
    }
}
