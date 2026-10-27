//! The input conditioner (at the base rate) and the preamp (at the oversampled rate).
//!
//! # Input stage
//!
//! A second-order high-pass (`tight`, 20..500 Hz) removes the low end before it can choke the
//! gain stages (the "tight" of a palm-muted chug on a down-tuned guitar), and a peaking boost
//! (`boost`, 0..18 dB at 780 Hz, Q 0.7) pushes the mids into the first stage, like a booster pedal
//! in front of the amp.
//!
//! # Preamp
//!
//! Three cascaded gain stages with a [`ToneStack`] between the second and the third. A stage is
//!
//! ```text
//! x -> x gain + bias -> asymmetric soft clip -> - clip(bias) -> coupling high-pass -> low-pass
//! ```
//!
//! * the **clip** is algebraic: the positive half is `u / sqrt(1 + u^2)`, the negative half is
//!   `u / sqrt(1 + (1.35 u)^2)`. Both have unity slope at 0, the negative half saturates at -0.74
//!   instead of 1, so a driven stage makes even harmonics like a triode does, and a stage never
//!   outputs more than 1 whatever the input;
//! * the fixed **bias** (0.08, 0.05, 0.03) moves the operating point a little further into the
//!   asymmetry; the curve's value at the bias is subtracted so a bias is not a step, and the
//!   coupling high-pass (one pole, 20 / 90 / 140 Hz) blocks the DC the asymmetry still makes and
//!   keeps the low end of a driven stage tight;
//! * the **low-pass** (second order, 7.5 / 5.8 / 5 kHz, Q 0.55) after each stage is the inter-stage
//!   filtering that keeps the next stage from being hit with the top of the previous stage's
//!   edges, which also keeps the aliasing of the following clip low;
//! * the **gain** of the three stages follows one knob ([`stage_gains_db`]): the first stage takes
//!   the most, the last only joins in at high settings. A gain glides over 10 ms.
//!
//! After the third stage a "fizz" low-pass (two second-order sections at 7.5 kHz, Q 0.54) takes
//! the harsh top off before the signal is brought back to the base rate.

use super::{Adaa, BlockBiquad, ToneStack, db_to_gain, glide_samples};
use crate::{BiquadCoeffs, BiquadShape, OnePoleSmoother};

/// Makeup of the last stage that pays for the tone stack's insertion loss, in dB.
const STACK_MAKEUP_DB: f32 = 6.0;
/// The negative half of the clip is this much steeper.
const NEG_K: f64 = 1.35;
const BIAS: [f64; 3] = [0.08, 0.05, 0.03];
/// Coupling high-pass corners in Hz.
const COUPLING_HZ: [f64; 3] = [20.0, 90.0, 140.0];
/// Inter-stage low-pass corners in Hz.
const STAGE_LP_HZ: [f32; 3] = [7_500.0, 5_800.0, 5_000.0];
/// Corner of the fizz filter in Hz.
const FIZZ_HZ: f32 = 7_500.0;
/// Time in ms in which a gain follows its knob.
const GAIN_MS: f32 = 10.0;

/// Gains of the three stages in dB for the gain knob `0..=1` (clamped).
pub fn stage_gains_db(gain: f32) -> [f32; 3] {
    let k = if gain.is_finite() {
        gain.clamp(0.0, 1.0)
    } else {
        0.0
    };
    [2.0 + 28.0 * k, 26.0 * k, STACK_MAKEUP_DB + 14.0 * k * k]
}

/// The asymmetric soft clip of one stage.
#[inline]
fn clip(u: f64) -> f64 {
    if u >= 0.0 {
        u / (1.0 + u * u).sqrt()
    } else {
        let v = NEG_K * u;
        u / (1.0 + v * v).sqrt()
    }
}

/// The antiderivative of [`clip`], continuous at 0 (`F(0) = 1`).
#[inline]
fn clip_anti(u: f64) -> f64 {
    if u >= 0.0 {
        (1.0 + u * u).sqrt()
    } else {
        let v = NEG_K * u;
        (1.0 + v * v).sqrt() / (NEG_K * NEG_K) + 1.0 - 1.0 / (NEG_K * NEG_K)
    }
}

/// The tight high-pass and the mid boost in front of the preamp (base rate, mono).
#[derive(Debug, Clone)]
pub struct InputStage {
    sample_rate: f32,
    hp: BlockBiquad,
    boost: BlockBiquad,
    tight_hz: f32,
    boost_db: f32,
}

impl InputStage {
    /// An input stage for `sample_rate`: high-pass at 20 Hz, no boost.
    pub fn new(sample_rate: f32) -> Self {
        let mut s = Self {
            sample_rate,
            hp: BlockBiquad::new(),
            boost: BlockBiquad::new(),
            tight_hz: 20.0,
            boost_db: 0.0,
        };
        s.apply(true);
        s
    }

    /// High-pass corner in Hz (clamped to 20..=500).
    pub fn set_tight_hz(&mut self, hz: f32) {
        let hz = if hz.is_finite() {
            hz.clamp(20.0, 500.0)
        } else {
            20.0
        };
        if hz.to_bits() != self.tight_hz.to_bits() {
            self.tight_hz = hz;
            self.apply(false);
        }
    }

    /// Mid boost in dB (clamped to 0..=18).
    pub fn set_boost_db(&mut self, db: f32) {
        let db = if db.is_finite() {
            db.clamp(0.0, 18.0)
        } else {
            0.0
        };
        if db.to_bits() != self.boost_db.to_bits() {
            self.boost_db = db;
            self.apply(false);
        }
    }

    fn apply(&mut self, now: bool) {
        let sr = f64::from(self.sample_rate);
        let hp = BiquadCoeffs::design(
            BiquadShape::Highpass,
            f64::from(self.tight_hz),
            0.7,
            0.0,
            sr,
        );
        let boost = if self.boost_db == 0.0 {
            BiquadCoeffs::IDENTITY
        } else {
            BiquadCoeffs::design(
                BiquadShape::Peaking,
                780.0,
                0.7,
                f64::from(self.boost_db),
                sr,
            )
        };
        let n = glide_samples(GLIDE_MS_INPUT, self.sample_rate);
        if now {
            self.hp.set_now(hp);
            self.boost.set_now(boost);
        } else {
            self.hp.glide_to(hp, n);
            self.boost.glide_to(boost, n);
        }
    }

    /// Clears the filter state.
    pub fn reset(&mut self) {
        self.apply(true);
        self.hp.reset();
        self.boost.reset();
    }

    /// Filters one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.boost.process(self.hp.process(x))
    }
}

/// Glide of the input filters in ms.
const GLIDE_MS_INPUT: f32 = 10.0;

#[derive(Debug, Clone)]
struct Stage {
    gain: OnePoleSmoother,
    bias: f64,
    offset: f64,
    adaa: Adaa,
    hp_r: f64,
    hp_x: f64,
    hp_y: f64,
    lp: BlockBiquad,
}

impl Stage {
    fn new(i: usize, sample_rate: f32, gain: f32) -> Self {
        let bias = BIAS[i];
        let r = 1.0 - std::f64::consts::TAU * COUPLING_HZ[i] / f64::from(sample_rate);
        let mut lp = BlockBiquad::new();
        lp.set_now(BiquadCoeffs::design(
            BiquadShape::Lowpass,
            f64::from(STAGE_LP_HZ[i]),
            0.55,
            0.0,
            f64::from(sample_rate),
        ));
        Self {
            gain: OnePoleSmoother::new(gain, GAIN_MS, sample_rate),
            bias,
            offset: clip(bias),
            adaa: Adaa::new(bias, clip_anti),
            hp_r: r,
            hp_x: 0.0,
            hp_y: 0.0,
            lp,
        }
    }

    fn reset(&mut self, gain: f32) {
        self.gain.set_immediate(gain);
        self.hp_x = 0.0;
        self.hp_y = 0.0;
        self.adaa.reset(self.bias, clip_anti);
        self.lp.reset();
    }

    #[inline]
    fn process(&mut self, x: f32, target_gain: f32) -> f32 {
        let g = self.gain.next_value(target_gain);
        let u = f64::from(x) * f64::from(g) + self.bias;
        let xd = self.adaa.process(u, clip, clip_anti) - self.offset;
        let yd = xd - self.hp_x + self.hp_r * self.hp_y;
        self.hp_x = xd;
        self.hp_y = if yd.abs() < 1e-30 { 0.0 } else { yd };
        self.lp.process(self.hp_y as f32)
    }
}

/// The three-stage preamp with the tone stack (see the module docs). Mono.
#[derive(Debug, Clone)]
pub struct Preamp {
    stages: [Stage; 3],
    tone: ToneStack,
    fizz: [BlockBiquad; 2],
    /// Target gains, linear.
    gains: [f32; 3],
}

impl Preamp {
    /// A preamp fed at `sample_rate` (the oversampled rate) with the gain knob at 0 and the tone
    /// knobs at the middle.
    pub fn new(sample_rate: f32) -> Self {
        let gains = stage_gains_db(0.0).map(db_to_gain);
        let mut fizz = [BlockBiquad::new(), BlockBiquad::new()];
        for f in &mut fizz {
            f.set_now(BiquadCoeffs::design(
                BiquadShape::Lowpass,
                f64::from(FIZZ_HZ),
                0.54,
                0.0,
                f64::from(sample_rate),
            ));
        }
        Self {
            stages: [
                Stage::new(0, sample_rate, gains[0]),
                Stage::new(1, sample_rate, gains[1]),
                Stage::new(2, sample_rate, gains[2]),
            ],
            tone: ToneStack::new(sample_rate),
            fizz,
            gains,
        }
    }

    /// The gain knob `0..=1`.
    pub fn set_gain(&mut self, gain: f32) {
        self.gains = stage_gains_db(gain).map(db_to_gain);
    }

    /// The tone stack knobs `0..=1`.
    pub fn set_tone(&mut self, bass: f32, mid: f32, treble: f32) {
        self.tone.set(bass, mid, treble);
    }

    /// The tone stack (for its response).
    pub fn tone(&self) -> &ToneStack {
        &self.tone
    }

    /// Clears the state; the gains jump to their targets.
    pub fn reset(&mut self) {
        for (s, g) in self.stages.iter_mut().zip(self.gains) {
            s.reset(g);
        }
        self.tone.reset();
        for f in &mut self.fizz {
            f.reset();
        }
    }

    /// Processes one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let a = self.stages[0].process(x, self.gains[0]);
        let b = self.stages[1].process(a, self.gains[1]);
        let t = self.tone.process(b);
        let c = self.stages[2].process(t, self.gains[2]);
        let f = self.fizz[0].process(c);
        self.fizz[1].process(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_antiderivative_is_continuous_and_differentiates_to_the_clip() {
        let h = 1e-6;
        for i in -400..=400 {
            let u = f64::from(i) * 0.05 + 0.013;
            let d = (clip_anti(u + h) - clip_anti(u - h)) / (2.0 * h);
            assert!((d - clip(u)).abs() < 1e-6, "{u}: {d} {}", clip(u));
        }
        assert!((clip_anti(1e-12) - clip_anti(-1e-12)).abs() < 1e-9);
        assert_eq!(clip(0.0), 0.0);
        // Unity slope at 0; the negative half saturates lower.
        assert!((clip(1e-6) / 1e-6 - 1.0).abs() < 1e-6);
        assert!((clip(-1e-6) / -1e-6 - 1.0).abs() < 1e-6);
        assert!(clip(1e6) > 0.999 && clip(-1e6) < -0.73 && clip(-1e6) > -0.75);
    }

    #[test]
    fn antiderivative_anti_aliasing_follows_the_curve_for_a_slow_signal() {
        let mut a = Adaa::new(0.0, clip_anti);
        let mut worst = 0.0f64;
        let mut prev = 0.0;
        for i in 0..2_000 {
            let u = 3.0 * (f64::from(i) * 0.002).sin();
            let y = a.process(u, clip, clip_anti);
            // The mean over a step of 0.006 is the value at the middle of the step, to a hair.
            worst = worst.max((y - clip(0.5 * (u + prev))).abs());
            prev = u;
        }
        assert!(worst < 2e-3, "{worst}");
    }
}
