//! The building blocks of the guitar amp (`atomik.guitar-amp`): an input conditioner, a three
//! stage saturating preamp with a passive-style tone stack, a power amp with sag, presence and
//! depth, and a synthesised cabinet. An original design that follows the general topology of a
//! valve guitar amplifier, not a circuit or a product.
//!
//! ```text
//! base rate:   in -> [InputStage: tight high-pass, mid boost]
//! oversampled: -> [Preamp: stage 1 -> stage 2 -> ToneStack -> stage 3 -> fizz low-pass]
//!              -> [PowerAmp: sag, soft clip, presence, depth]
//! base rate:   -> [Cabinet: speaker and mic filters, box reflection]
//! ```
//!
//! The nonlinear blocks run at a rate chosen by the caller (the device wraps them in a 4x
//! [`crate::oversample::Oversampler`] and gives them four times the sample rate), so a block does
//! not know about oversampling: it only needs the rate it is fed at. Everything is `+ - * / sqrt`
//! plus the deterministic [`crate::exp2_det`] in the per-sample path; the filter coefficients come
//! from the RBJ designs of [`crate::BiquadCoeffs`] and are only recomputed when a parameter
//! changes. Nothing allocates after construction. Gains and filter coefficients glide to a new
//! target (a few milliseconds), so a knob turn does not click.

mod cab;
mod power;
mod preamp;
mod tone;

pub use cab::{CabType, Cabinet};
pub use power::PowerAmp;
pub use preamp::{InputStage, Preamp, stage_gains_db};
pub use tone::{ToneStack, tone_gains_db};

use crate::{Biquad, BiquadCoeffs};

/// A biquad whose coefficients glide linearly to a new design over a fixed number of samples
/// (the interpolation of the five coefficients is smooth enough for a slowly moved knob and
/// costs nothing while the filter is settled).
#[derive(Debug, Clone)]
pub struct BlockBiquad {
    bq: Biquad,
    cur: BiquadCoeffs,
    target: BiquadCoeffs,
    step: [f64; 5],
    left: u32,
}

impl Default for BlockBiquad {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockBiquad {
    /// A pass-through section.
    pub fn new() -> Self {
        Self {
            bq: Biquad::default(),
            cur: BiquadCoeffs::IDENTITY,
            target: BiquadCoeffs::IDENTITY,
            step: [0.0; 5],
            left: 0,
        }
    }

    /// Jumps to `c` at once, keeping the filter state.
    pub fn set_now(&mut self, c: BiquadCoeffs) {
        self.cur = c;
        self.target = c;
        self.left = 0;
    }

    /// Glides to `c` over `samples` samples (at least 1); a design equal to the target is a no-op.
    pub fn glide_to(&mut self, c: BiquadCoeffs, samples: u32) {
        if c == self.target {
            return;
        }
        let n = samples.max(1);
        self.target = c;
        self.left = n;
        let k = f64::from(n);
        self.step = [
            (c.b0 - self.cur.b0) / k,
            (c.b1 - self.cur.b1) / k,
            (c.b2 - self.cur.b2) / k,
            (c.a1 - self.cur.a1) / k,
            (c.a2 - self.cur.a2) / k,
        ];
    }

    /// Clears the filter state and jumps to the target.
    pub fn reset(&mut self) {
        self.bq.reset();
        self.cur = self.target;
        self.left = 0;
    }

    /// True once the coefficients have reached the target.
    pub fn is_settled(&self) -> bool {
        self.left == 0
    }

    /// The target coefficients.
    pub fn target(&self) -> &BiquadCoeffs {
        &self.target
    }

    /// Filters one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        if self.left > 0 {
            self.left -= 1;
            if self.left == 0 {
                self.cur = self.target;
            } else {
                self.cur.b0 += self.step[0];
                self.cur.b1 += self.step[1];
                self.cur.b2 += self.step[2];
                self.cur.a1 += self.step[3];
                self.cur.a2 += self.step[4];
            }
        }
        self.bq.process(&self.cur, f64::from(x)) as f32
    }
}

/// Number of samples of a glide of `ms` milliseconds at `sample_rate`.
#[inline]
pub(crate) fn glide_samples(ms: f32, sample_rate: f32) -> u32 {
    ((ms * 0.001 * sample_rate).round() as u32).max(1)
}

/// `dB` to a linear factor, deterministic.
#[inline]
pub(crate) fn db_to_gain(db: f32) -> f32 {
    crate::exp2_det(db * 0.166_096_4)
}

/// First-order antiderivative anti-aliasing of a memoryless curve: instead of the point value
/// `f(u)` the output is the mean of `f` over the step from the previous sample,
/// `(F(u) - F(u1)) / (u - u1)` (`F` an antiderivative of `f`), the exact output of the curve
/// for a piecewise-linear input. It takes much of the folded aliasing out of a sharp knee, for
/// half a sample of delay. A step below `1e-6` uses the value at the midpoint. The arithmetic
/// runs in `f64`, because a driven stage's `F` is large and the difference of two of them is not.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Adaa {
    u1: f64,
    f1: f64,
}

impl Adaa {
    /// The state of a curve whose antiderivative is `anti`, at rest at `u0`.
    pub(crate) fn new(u0: f64, anti: impl Fn(f64) -> f64) -> Self {
        Self {
            u1: u0,
            f1: anti(u0),
        }
    }

    /// Back to rest at `u0`.
    pub(crate) fn reset(&mut self, u0: f64, anti: impl Fn(f64) -> f64) {
        *self = Self::new(u0, anti);
    }

    /// The curve at `u`.
    #[inline]
    pub(crate) fn process(
        &mut self,
        u: f64,
        curve: impl Fn(f64) -> f64,
        anti: impl Fn(f64) -> f64,
    ) -> f64 {
        let fu = anti(u);
        let d = u - self.u1;
        let y = if d.abs() < 1e-6 {
            curve(0.5 * (u + self.u1))
        } else {
            (fu - self.f1) / d
        };
        self.u1 = u;
        self.f1 = fu;
        y
    }
}

#[cfg(test)]
mod tests;
