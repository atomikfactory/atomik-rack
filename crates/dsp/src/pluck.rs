//! The plucked string: an extended Karplus-Strong waveguide, its pick and its body. An original
//! design from the textbook of digital waveguides (Karplus and Strong, Jaffe and Smith); the
//! voices, the expression and the device are in `atomik-devices`.
//!
//! * **String.** [`PluckString`] is a delay line closed on itself through a loop filter. Each
//!   sample reads the line with a four-point Lagrange interpolator (so the delay can change
//!   smoothly, which is how a bend glides), runs the result through up to [`STAGES`] first-order
//!   allpass sections (the dispersion of a stiff string: the higher partials run sharp) and a
//!   one-pole low-pass (the loss that is stronger for the higher partials), scales it by the loop
//!   gain and writes it back. The loop gain sets the decay: the fundamental falls by 60 dB in
//!   `decay_s` seconds. The low-pass cutoff follows the pitch (4 to 256 times the fundamental,
//!   `brightness` 0 to 1), so a note keeps its character over the keyboard.
//! * **Tuning.** The line is shortened by the exact phase delay of the low-pass and of the
//!   allpass chain at the fundamental, computed in `f64` with a deterministic arctangent, so
//!   the pitch is within a fraction of a cent over the whole range at every sample rate, with
//!   any stiffness or brightness. [`PluckString::set_pitch`] is cheap to call every few samples:
//!   when nothing changed it returns at once, and a new pitch ramps the delay linearly over the
//!   `steps` given (a glide, no click).
//! * **Pick.** [`Pick`] makes the excitation: a burst of noise one period long through a
//!   low-pass (the hardness of the pick, a harder or louder strike is brighter) and a feed-forward
//!   comb (the pick position: the partials whose antinode is at the plucked point are cancelled).
//!   The burst is a function of the seed only.
//! * **Body.** [`body::Body`] is a stereo cascade of peaking, shelving and pass sections per
//!   instrument body, mixed in parallel with the dry signal.
//!
//! Everything is deterministic for a given seed (the tuning maths use `+ - * / sqrt` and the
//! deterministic `exp2`, and the arctangent here), allocation-free after construction, flushed
//! against denormals, and a non-finite sample clears the string instead of spreading.

use crate::DelayLine;
use crate::det_math::{exp2_det_f64, flush_denormal};
use crate::det_trig::{cos_cycles, sin_cycles};
use crate::rng::Rng;

pub mod body;
#[cfg(test)]
mod tests;

/// Allpass sections of the dispersion chain.
pub const STAGES: usize = 4;
/// The lowest fundamental a string is tuned to, Hz.
pub const MIN_HZ: f64 = 20.0;
/// The highest fundamental as a share of the sample rate.
pub const MAX_HZ_SHARE: f64 = 0.2;
/// The shortest line, in samples (the interpolator reads two samples either side).
const MIN_LINE: f64 = 2.0;
/// The most the loop gain at DC may be (a loop at 1 or more would grow).
const LOOP_GAIN_MAX: f64 = 0.999_95;
/// `3 log2(10)`: a fall by 60 dB as a power of two.
const THREE_LOG2_10: f64 = 9.965_784_284_662_087;
/// The coefficient magnitude of the dispersion allpass at stiffness 1.
const MAX_DISPERSION: f64 = 0.95;

/// `atan(x)` from `+ - * / sqrt` only: argument halving twice, then the Taylor series (error
/// below 1e-12).
fn atan_det(x: f64) -> f64 {
    if x.is_nan() {
        return 0.0;
    }
    let a = x.abs();
    if a.is_infinite() {
        return std::f64::consts::FRAC_PI_2.copysign(x);
    }
    let inverted = a > 1.0;
    let mut t = if inverted { 1.0 / a } else { a };
    t /= 1.0 + (1.0 + t * t).sqrt();
    t /= 1.0 + (1.0 + t * t).sqrt();
    let t2 = t * t;
    let mut acc = 1.0 / 15.0;
    let mut k = 13.0;
    while k >= 1.0 {
        acc = 1.0 / k - t2 * acc;
        k -= 2.0;
    }
    let mut r = 4.0 * t * acc;
    if inverted {
        r = std::f64::consts::FRAC_PI_2 - r;
    }
    r.copysign(x)
}

/// `atan2(y, x)` on [`atan_det`].
fn atan2_det(y: f64, x: f64) -> f64 {
    use std::f64::consts::{FRAC_PI_2, PI};
    if x > 0.0 {
        atan_det(y / x)
    } else if x < 0.0 {
        let r = atan_det(y / x);
        if y >= 0.0 { r + PI } else { r - PI }
    } else if y > 0.0 {
        FRAC_PI_2
    } else if y < 0.0 {
        -FRAC_PI_2
    } else {
        0.0
    }
}

/// The phase delay in samples at `cycles` (frequency over sample rate) of the one-pole low-pass
/// `(1 - p) / (1 - p z^-1)`.
fn lowpass_delay(p: f64, cycles: f64) -> f64 {
    let w = std::f64::consts::TAU * cycles;
    atan2_det(p * sin_cycles(cycles), 1.0 - p * cos_cycles(cycles)) / w
}

/// The magnitude of the same low-pass at `cycles`.
fn lowpass_gain(p: f64, cycles: f64) -> f64 {
    (1.0 - p) / (1.0 - 2.0 * p * cos_cycles(cycles) + p * p).sqrt()
}

/// The phase delay in samples at `cycles` of the allpass `(a + z^-1) / (1 + a z^-1)`.
fn allpass_delay(a: f64, cycles: f64) -> f64 {
    let w = std::f64::consts::TAU * cycles;
    let (s, c) = (sin_cycles(cycles), cos_cycles(cycles));
    let num = atan2_det(-s, a + c);
    let den = atan2_det(-a * s, 1.0 + a * c);
    -(num - den) / w
}

/// What a string sounds like (the loop settings).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StringTone {
    /// Seconds the fundamental takes to fall by 60 dB (clamped to 5 ms to 60 s).
    pub decay_s: f32,
    /// Loop low-pass, 0 (dull: the cutoff at 4 times the fundamental) to 1 (bright: 256 times).
    pub brightness: f32,
    /// Dispersion, 0 (harmonic) to 1 (a stiff, bell-like string).
    pub stiffness: f32,
}

/// One string: the delay line, the loop filter and the tuning.
#[derive(Debug, Clone)]
pub struct PluckString {
    sample_rate: f64,
    ring: Vec<f32>,
    mask: usize,
    write: usize,
    // Loop filter.
    lp: f32,
    lp_pole: f32,
    lp_in: f32,
    gain: f32,
    ap: [f32; STAGES],
    ap_coef: f32,
    stages: usize,
    // Line delay and its glide.
    delay: f32,
    target: f32,
    step: f32,
    remaining: u32,
    period: f32,
    // What the last `set_pitch` was for.
    primed: bool,
    last: [u32; 4],
}

impl PluckString {
    /// A silent string for `sample_rate` (allocates the delay line, non-RT).
    pub fn new(sample_rate: f32) -> Self {
        let sr = f64::from(sample_rate.max(8_000.0));
        let ring = ((sr / MIN_HZ).ceil() as usize + 8).next_power_of_two();
        Self {
            sample_rate: sr,
            ring: vec![0.0; ring],
            mask: ring - 1,
            write: 0,
            lp: 0.0,
            lp_pole: 0.0,
            lp_in: 1.0,
            gain: 0.0,
            ap: [0.0; STAGES],
            ap_coef: 0.0,
            stages: 0,
            delay: 100.0,
            target: 100.0,
            step: 0.0,
            remaining: 0,
            period: 100.0,
            primed: false,
            last: [0; 4],
        }
    }

    /// Silences the string; the next [`PluckString::set_pitch`] jumps to its pitch.
    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.write = 0;
        self.lp = 0.0;
        self.ap = [0.0; STAGES];
        self.primed = false;
        self.remaining = 0;
    }

    /// The period of the string in samples as last set.
    pub fn period(&self) -> f32 {
        self.period
    }

    /// Tunes the string to `hz` (clamped to `MIN_HZ ..= MAX_HZ_SHARE` of the sample rate) with the
    /// loop of `tone`. A string that was silent jumps to the pitch; a sounding one moves its delay
    /// linearly over `steps` samples. Calling it again with the same arguments does nothing.
    pub fn set_pitch(&mut self, hz: f32, tone: &StringTone, steps: u32) {
        let key = [
            hz.to_bits(),
            tone.decay_s.to_bits(),
            tone.brightness.to_bits(),
            tone.stiffness.to_bits(),
        ];
        if self.primed && key == self.last {
            return;
        }
        self.last = key;
        let sr = self.sample_rate;
        let hz = if hz.is_finite() { f64::from(hz) } else { 220.0 };
        let f0 = hz.clamp(MIN_HZ, MAX_HZ_SHARE * sr);
        let cycles = f0 / sr;
        let period = sr / f0;

        // The loop low-pass: its cutoff is a multiple of the fundamental.
        let bright = f64::from(finite_or(tone.brightness, 0.5)).clamp(0.0, 1.0);
        let cutoff = (f0 * exp2_det_f64(2.0 + 6.0 * bright)).min(0.45 * sr);
        let pole = exp2_det_f64(-std::f64::consts::TAU * cutoff / sr * std::f64::consts::LOG2_E);
        let lp_delay = lowpass_delay(pole, cycles);

        // The dispersion: shrunk, or dropped, when the line would get too short for it.
        let stiff = f64::from(finite_or(tone.stiffness, 0.0)).clamp(0.0, 1.0);
        let mut coef = -MAX_DISPERSION * stiff;
        let mut stages = if coef < -1e-3 { STAGES } else { 0 };
        let mut ap_delay = 0.0;
        while stages > 0 {
            ap_delay = allpass_delay(coef, cycles) * stages as f64;
            if period - lp_delay - ap_delay >= 4.0 {
                break;
            }
            coef *= 0.5;
            if coef > -1e-3 {
                stages = 0;
                ap_delay = 0.0;
            }
        }
        let line = (period - lp_delay - ap_delay).max(MIN_LINE);

        // The loop gain: the fundamental falls by 60 dB in `decay_s`.
        let decay = f64::from(finite_or(tone.decay_s, 1.0)).clamp(0.005, 60.0);
        let at_f0 = exp2_det_f64(-THREE_LOG2_10 / (decay * f0));
        let gain = (at_f0 / lowpass_gain(pole, cycles)).min(LOOP_GAIN_MAX);

        self.lp_pole = pole as f32;
        self.lp_in = (1.0 - pole) as f32;
        self.gain = gain as f32;
        self.ap_coef = coef as f32;
        if self.stages == 0 && stages > 0 {
            self.ap = [0.0; STAGES];
        }
        self.stages = stages;
        self.period = period as f32;
        let max_line = (self.ring.len() - 4) as f32;
        self.target = (line as f32).min(max_line);
        if !self.primed || steps == 0 {
            self.delay = self.target;
            self.remaining = 0;
            self.step = 0.0;
        } else {
            self.remaining = steps;
            self.step = (self.target - self.delay) / steps as f32;
        }
        self.primed = true;
    }

    /// The four-point Lagrange read `d` samples back (`2 <= d`).
    #[inline]
    fn read(&self, d: f32) -> f32 {
        let d = d.clamp(MIN_LINE as f32, (self.ring.len() - 4) as f32);
        let i = d as usize;
        let t = d - i as f32;
        let at = |k: usize| self.ring[self.write.wrapping_sub(k) & self.mask];
        let (xm, x0, x1, x2) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        let c_m = -t * (t - 1.0) * (t - 2.0) * (1.0 / 6.0);
        let c_0 = (t + 1.0) * (t - 1.0) * (t - 2.0) * 0.5;
        let c_1 = -(t + 1.0) * t * (t - 2.0) * 0.5;
        let c_2 = (t + 1.0) * t * (t - 1.0) * (1.0 / 6.0);
        c_m * xm + c_0 * x0 + c_1 * x1 + c_2 * x2
    }

    /// One sample: the loop with `excitation` added in. Returns the new loop sample.
    #[inline]
    pub fn tick(&mut self, excitation: f32) -> f32 {
        if self.remaining > 0 {
            self.delay += self.step;
            self.remaining -= 1;
            if self.remaining == 0 {
                self.delay = self.target;
            }
        }
        let mut y = self.read(self.delay);
        let a = self.ap_coef;
        for s in &mut self.ap[..self.stages] {
            let out = a * y + *s;
            *s = flush_denormal(y - a * out);
            y = out;
        }
        self.lp = flush_denormal(self.lp_in * y + self.lp_pole * self.lp);
        let v = self.gain * self.lp + excitation;
        if !v.is_finite() {
            self.reset();
            return 0.0;
        }
        self.ring[self.write] = flush_denormal(v);
        self.write = (self.write + 1) & self.mask;
        v
    }
}

fn finite_or(x: f32, fallback: f32) -> f32 {
    if x.is_finite() { x } else { fallback }
}

/// How a pluck is struck.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickParams {
    /// The period of the string in samples (the burst is as long).
    pub period: f32,
    /// The cutoff of the pick's low-pass, Hz.
    pub cutoff_hz: f32,
    /// Where the string is struck as a share of its length (`0` for no comb).
    pub position: f32,
    /// The level of the burst (its RMS before the comb).
    pub level: f32,
}

/// The excitation of a string: a shaped burst of noise.
#[derive(Debug, Clone)]
pub struct Pick {
    sample_rate: f32,
    rng: Rng,
    line: DelayLine,
    lp: f32,
    coef: f32,
    norm: f32,
    comb: f32,
    burst: u32,
    remaining: u32,
}

impl Pick {
    /// A pick for `sample_rate` (allocates the comb, non-RT).
    pub fn new(sample_rate: f32) -> Self {
        let max_period = (f64::from(sample_rate.max(8_000.0)) / MIN_HZ).ceil() as usize;
        Self {
            sample_rate: sample_rate.max(8_000.0),
            rng: Rng::new(0),
            line: DelayLine::new(max_period / 2 + 8),
            lp: 0.0,
            coef: 1.0,
            norm: 0.0,
            comb: 0.0,
            burst: 0,
            remaining: 0,
        }
    }

    /// Silences the pick.
    pub fn reset(&mut self) {
        self.line.clear();
        self.lp = 0.0;
        self.burst = 0;
        self.remaining = 0;
    }

    /// True while the burst is still coming.
    pub fn active(&self) -> bool {
        self.remaining > 0
    }

    /// Starts a burst from the noise stream `seed`.
    pub fn start(&mut self, p: &PickParams, seed: u64) {
        self.rng = Rng::new(seed);
        self.line.clear();
        self.lp = 0.0;
        let sr = f64::from(self.sample_rate);
        let period = f64::from(finite_or(p.period, 100.0)).clamp(4.0, sr / MIN_HZ);
        let cutoff = f64::from(finite_or(p.cutoff_hz, 4_000.0)).clamp(50.0, 0.45 * sr);
        let c = 1.0 - exp2_det_f64(-std::f64::consts::TAU * cutoff / sr * std::f64::consts::LOG2_E);
        self.coef = c as f32;
        // Unit RMS after the low-pass: uniform noise has a variance of 1/3 and the one-pole
        // low-pass keeps `c / (2 - c)` of it.
        self.norm = (3.0 * (2.0 - c) / c).sqrt() as f32 * finite_or(p.level, 0.0).max(0.0);
        let position = f64::from(finite_or(p.position, 0.0)).clamp(0.0, 0.5);
        let comb = (position * period).min((self.line.max_delay() - 1) as f64);
        self.comb = if comb > 0.0 {
            comb.max(1.0) as f32
        } else {
            0.0
        };
        self.burst = period.round() as u32;
        self.remaining = self.burst + self.comb.ceil() as u32 + 2;
    }

    /// The next sample of the burst (0 once it is over).
    #[inline]
    pub fn sample(&mut self) -> f32 {
        if self.remaining == 0 {
            return 0.0;
        }
        self.remaining -= 1;
        let e = if self.burst > 0 {
            self.burst -= 1;
            self.lp += self.coef * (self.rng.next_bipolar() - self.lp);
            self.lp * self.norm
        } else {
            0.0
        };
        let out = if self.comb > 0.0 {
            e - self.line.read(self.comb)
        } else {
            e
        };
        self.line.write(e);
        out
    }
}
