//! Waveshaper stage: five drive curves with analytic derivatives and a level-dependent type.
//!
//! The stateless curves act on `u = g x` (`g` the drive as a linear factor):
//!
//! * [`ShaperKind::SoftClip`]: `u - u^3/3` up to `|u| = 1`, then flat at `2/3`. Odd, smooth knee.
//! * [`ShaperKind::Asymmetric`]: the same on the positive side; the negative side is steeper
//!   (`f(1.6 u)/1.6`, a lower limit), so even harmonics appear (tape-like). Slope 1 at the origin.
//! * [`ShaperKind::Hard`]: `clamp(u, -1, 1)`.
//! * [`ShaperKind::HardAsymmetric`]: `clamp(u, -0.6, 1)`.
//! * [`ShaperKind::Fold`]: a triangle fold with period 4 (`f(u) = u` up to `|u| = 1`, then it
//!   folds back), bounded by 1.
//! * [`ShaperKind::Dynamic`]: no curve. The gain falls as a peak level estimate of the driven
//!   input rises: `y = u / (1 + env)`, `env = max(|u|, env * release)`, so `|y| < 1` always
//!   (never clips). Its steady-state curve for a constant level is `u / (1 + |u|)`.
//!
//! The curved types lose level as the drive rises: the output is scaled by `1/sqrt(max(g, 1))`,
//! only a partial make-up, so a loud signal gets quieter and a quiet one louder. Changing the
//! kind crossfades old and new over [`FADE_SAMPLES`]. Everything is `+ - * / sqrt` plus the
//! deterministic [`crate::exp2_det`] (no libm in the per-sample path). Run it at 2x through
//! [`crate::oversample::Oversampler`] for clean-sounding presets (see the aliasing test).
//!
//! # Antiderivative anti-aliasing (ADAA)
//!
//! [`Shaper::set_adaa`] switches the four stateless curves (soft clip, asymmetric, hard, hard
//! asymmetric) to **first-order antiderivative anti-aliasing**: instead of the point value
//! `f(u)` the output is the mean of the curve over the step from the previous driven sample,
//! `(F(u) - F(u1)) / (u - u1)` with `F` the antiderivative of `f` ([`curve_integral`]); for
//! a step smaller than [`ADAA_EPS`] the value at the midpoint is used. The result is the exact
//! output of the curve for a piecewise-linear input, which removes much of the folded
//! aliasing of the sharp knees at the price of half a sample of delay (the same at every
//! drive). The fold and dynamic types have no antiderivative here and ignore the flag. The
//! default is off, so existing users of the shaper keep their bits. The differences run in
//! `f64`.

use crate::exp2_det;

/// Crossfade length when the kind changes.
pub const FADE_SAMPLES: u32 = 128;

/// Positive limit of the soft knee.
const SOFT_LIMIT: f32 = 2.0 / 3.0;
/// Steepness of the negative side of the asymmetric soft curve.
const ASYM_K: f32 = 1.6;
/// Negative limit of the asymmetric hard clip.
const HARD_ASYM_LOW: f32 = -0.6;
/// `log2(10)/20`: dB to log2 of amplitude.
const DB_TO_LOG2: f32 = 0.166_096_4;
/// Lowest drive in dB.
const DRIVE_MIN_DB: f32 = -24.0;
/// Highest drive in dB.
const DRIVE_MAX_DB: f32 = 60.0;

/// The drive type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaperKind {
    /// Odd soft saturation.
    SoftClip,
    /// Soft saturation with a steeper negative side (even harmonics).
    Asymmetric,
    /// Hard clip at plus and minus one.
    Hard,
    /// Hard clip with a lower negative limit.
    HardAsymmetric,
    /// Triangle wave folder.
    Fold,
    /// Level-dependent gain, never clips.
    Dynamic,
}

#[inline]
fn soft(u: f32) -> f32 {
    if u >= 1.0 {
        SOFT_LIMIT
    } else if u <= -1.0 {
        -SOFT_LIMIT
    } else {
        u - u * u * u * (1.0 / 3.0)
    }
}

#[inline]
fn soft_d(u: f32) -> f32 {
    if u.abs() >= 1.0 { 0.0 } else { 1.0 - u * u }
}

/// The stateless curve of `kind` at `u` (for [`ShaperKind::Dynamic`], the steady-state curve
/// `u / (1 + |u|)`). No output scaling.
#[inline]
pub fn curve(kind: ShaperKind, u: f32) -> f32 {
    match kind {
        ShaperKind::SoftClip => soft(u),
        ShaperKind::Asymmetric => {
            if u >= 0.0 {
                soft(u)
            } else {
                soft(ASYM_K * u) / ASYM_K
            }
        }
        ShaperKind::Hard => u.clamp(-1.0, 1.0),
        ShaperKind::HardAsymmetric => u.clamp(HARD_ASYM_LOW, 1.0),
        ShaperKind::Fold => {
            let t = (u - 1.0).rem_euclid(4.0);
            (t - 2.0).abs() - 1.0
        }
        ShaperKind::Dynamic => u / (1.0 + u.abs()),
    }
}

/// The analytic derivative of [`curve`] (zero on the flat parts; the sign of the slope on a fold).
#[inline]
pub fn curve_derivative(kind: ShaperKind, u: f32) -> f32 {
    match kind {
        ShaperKind::SoftClip => soft_d(u),
        ShaperKind::Asymmetric => {
            if u >= 0.0 {
                soft_d(u)
            } else {
                soft_d(ASYM_K * u)
            }
        }
        ShaperKind::Hard => {
            if u.abs() < 1.0 {
                1.0
            } else {
                0.0
            }
        }
        ShaperKind::HardAsymmetric => {
            if u > HARD_ASYM_LOW && u < 1.0 {
                1.0
            } else {
                0.0
            }
        }
        ShaperKind::Fold => {
            let t = (u - 1.0).rem_euclid(4.0);
            if t < 2.0 { -1.0 } else { 1.0 }
        }
        ShaperKind::Dynamic => {
            let d = 1.0 + u.abs();
            1.0 / (d * d)
        }
    }
}

/// Below this step in `u` the ADAA value falls back to the curve at the midpoint.
pub const ADAA_EPS: f64 = 1.0e-3;

/// `F` of the soft knee (even; `F(0) = 0`).
#[inline]
fn soft_integral(u: f64) -> f64 {
    let a = u.abs();
    if a < 1.0 {
        let a2 = a * a;
        a2 * 0.5 - a2 * a2 * (1.0 / 12.0)
    } else {
        5.0 / 12.0 + (2.0 / 3.0) * (a - 1.0)
    }
}

/// The antiderivative `F(u)` (the integral of `curve(kind, t)` from 0 to `u`) of the stateless
/// curves (soft clip, asymmetric, hard, hard asymmetric), exactly as [`curve`] defines them;
/// `None` for the fold and the dynamic type.
pub fn curve_integral(kind: ShaperKind, u: f64) -> Option<f64> {
    Some(match kind {
        ShaperKind::SoftClip => soft_integral(u),
        ShaperKind::Asymmetric => {
            if u >= 0.0 {
                soft_integral(u)
            } else {
                let k = f64::from(ASYM_K);
                soft_integral(k * u) / (k * k)
            }
        }
        ShaperKind::Hard => {
            if u.abs() <= 1.0 {
                0.5 * u * u
            } else {
                u.abs() - 0.5
            }
        }
        ShaperKind::HardAsymmetric => {
            let lo = f64::from(HARD_ASYM_LOW);
            if u > 1.0 {
                0.5 + (u - 1.0)
            } else if u < lo {
                0.5 * lo * lo + lo * (u - lo)
            } else {
                0.5 * u * u
            }
        }
        ShaperKind::Fold | ShaperKind::Dynamic => return None,
    })
}

/// The first-order ADAA value of the stateless curve `kind` for the step `u1 -> u`
/// (`None` for the kinds without an antiderivative).
#[inline]
fn adaa_curve(kind: ShaperKind, u: f32, u1: f32) -> Option<f32> {
    let (a, b) = (f64::from(u), f64::from(u1));
    let d = a - b;
    if d.abs() < ADAA_EPS {
        curve_integral(kind, a)?;
        return Some(curve(kind, 0.5 * (u + u1)));
    }
    Some(((curve_integral(kind, a)? - curve_integral(kind, b)?) / d) as f32)
}

/// A shaper with drive, kind switching and (for the dynamic type) a level estimate.
#[derive(Debug, Clone)]
pub struct Shaper {
    kind: ShaperKind,
    prev: ShaperKind,
    fade: u32,
    gain: f32,
    out_scale: f32,
    env: f32,
    release: f32,
    adaa: bool,
    u1: f32,
}

impl Shaper {
    /// A soft-clip shaper at 0 dB drive; `sample_rate` sets the dynamic type's release (50 ms).
    pub fn new(sample_rate: f32) -> Self {
        let release = 1.0 - 1.0 / (0.050 * sample_rate).max(1.0);
        Self {
            kind: ShaperKind::SoftClip,
            prev: ShaperKind::SoftClip,
            fade: 0,
            gain: 1.0,
            out_scale: 1.0,
            env: 0.0,
            release,
            adaa: false,
            u1: 0.0,
        }
    }

    /// Switches first-order antiderivative anti-aliasing on or off (default off; see the
    /// module docs). Takes effect from the next sample.
    pub fn set_adaa(&mut self, on: bool) {
        self.adaa = on;
    }

    /// Whether ADAA is on.
    pub fn adaa(&self) -> bool {
        self.adaa
    }

    /// Sets the drive in dB (clamped to -24..=60; NaN ignored).
    pub fn set_drive_db(&mut self, db: f32) {
        if db.is_nan() {
            return;
        }
        let db = db.clamp(DRIVE_MIN_DB, DRIVE_MAX_DB);
        self.gain = exp2_det(db * DB_TO_LOG2);
        self.out_scale = 1.0 / self.gain.max(1.0).sqrt();
    }

    /// The linear drive factor.
    pub fn drive_gain(&self) -> f32 {
        self.gain
    }

    /// Selects the kind; a change crossfades from the previous kind over [`FADE_SAMPLES`].
    pub fn set_kind(&mut self, kind: ShaperKind) {
        if kind != self.kind {
            self.prev = self.kind;
            self.kind = kind;
            self.fade = FADE_SAMPLES;
        }
    }

    /// The current kind.
    pub fn kind(&self) -> ShaperKind {
        self.kind
    }

    /// Clears the level estimate and any running crossfade.
    pub fn reset(&mut self) {
        self.env = 0.0;
        self.fade = 0;
        self.prev = self.kind;
        self.u1 = 0.0;
    }

    #[inline]
    fn run(&self, kind: ShaperKind, u: f32) -> f32 {
        match kind {
            ShaperKind::Dynamic => u / (1.0 + self.env),
            ShaperKind::Fold => curve(kind, u),
            _ => {
                let y = if self.adaa {
                    adaa_curve(kind, u, self.u1).unwrap_or_else(|| curve(kind, u))
                } else {
                    curve(kind, u)
                };
                y * self.out_scale
            }
        }
    }

    /// Shapes one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let u = x * self.gain;
        self.env = crate::flush_denormal(u.abs().max(self.env * self.release));
        let new = self.run(self.kind, u);
        let out = if self.fade == 0 {
            new
        } else {
            let w = 1.0 - self.fade as f32 / FADE_SAMPLES as f32;
            self.fade -= 1;
            let old = self.run(self.prev, u);
            old + (new - old) * w
        };
        self.u1 = if u.is_finite() { u } else { 0.0 };
        out
    }

    /// Shapes a block in place (identical to calling [`Shaper::process`] per sample).
    pub fn process_block(&mut self, buf: &mut [f32]) {
        for s in buf {
            *s = self.process(*s);
        }
    }
}

// ------------------------------------------------------------------ character curves

/// Slope parameter of [`CharacterKind::Modulate`]: the curve peaks at 1 (at `u = 1/(a sqrt 2)`).
const MODULATE_A: f64 = 0.384_9;
/// Pre-gain of [`CharacterKind::Fuzz`] and its negative ceiling.
const FUZZ_GAIN: f64 = 4.0;
const FUZZ_NEG: f64 = 0.6;
/// Slope and the lower ceiling of [`CharacterKind::Scream`].
const SCREAM_GAIN: f64 = 3.0;
const SCREAM_LOW: f64 = -0.5;

/// The additional distortion curves. Each is a memoryless curve with a unit output ceiling and a
/// closed-form antiderivative ([`character_integral`]), so [`character_adaa`] removes much of its
/// aliasing. They evaluate in `f64` with only `+ - * /` and `sqrt` (deterministic).
///
/// * [`CharacterKind::Tape`]: the cubic soft knee `1.5 u - 0.5 u^3` up to `|u| = 1`, flat beyond:
///   a rounded, odd saturation.
/// * [`CharacterKind::Fuzz`]: an asymmetric algebraic clip with a pre-gain of 4; the positive
///   side saturates at `+1`, the negative side at `-0.6`: dense even and odd harmonics.
/// * [`CharacterKind::Modulate`]: `u / (1 + a^2 u^2)^(3/2)`: the gain collapses as the level
///   rises, so a loud signal folds back towards zero after its peak at 1 (self-modulation).
/// * [`CharacterKind::Warp`]: the smooth plateau `c (15 - 10 c^2 + 3 c^4) / 8` on
///   `c = clamp(u, -1, 1)`: a flat-topped, nearly square limit with strong odd harmonics.
/// * [`CharacterKind::Scream`]: `clamp(3 u, -0.5, 1)`, an asymmetric hard clip with gain, the
///   harshest of the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterKind {
    /// Cubic soft knee.
    Tape,
    /// Asymmetric high-gain clip.
    Fuzz,
    /// Self-modulating gain collapse.
    Modulate,
    /// Smooth plateau limit.
    Warp,
    /// Asymmetric hard clip with gain.
    Scream,
}

impl CharacterKind {
    /// All five kinds, in a stable order.
    pub const ALL: [Self; 5] = [
        Self::Tape,
        Self::Fuzz,
        Self::Modulate,
        Self::Warp,
        Self::Scream,
    ];
}

/// The value of the character curve `kind` at `u` (non-finite `u` gives 0).
pub fn character_curve(kind: CharacterKind, u: f64) -> f64 {
    if !u.is_finite() {
        return 0.0;
    }
    match kind {
        CharacterKind::Tape => {
            let c = u.clamp(-1.0, 1.0);
            c * (1.5 - 0.5 * c * c)
        }
        CharacterKind::Fuzz => {
            if u >= 0.0 {
                let x = FUZZ_GAIN * u;
                x / (1.0 + x * x).sqrt()
            } else {
                let x = FUZZ_GAIN / FUZZ_NEG * u;
                FUZZ_NEG * x / (1.0 + x * x).sqrt()
            }
        }
        CharacterKind::Modulate => {
            let s = 1.0 + MODULATE_A * MODULATE_A * u * u;
            u / (s * s.sqrt())
        }
        CharacterKind::Warp => {
            let c = u.clamp(-1.0, 1.0);
            let c2 = c * c;
            c * (15.0 - 10.0 * c2 + 3.0 * c2 * c2) * 0.125
        }
        CharacterKind::Scream => (SCREAM_GAIN * u).clamp(SCREAM_LOW, 1.0),
    }
}

/// The antiderivative of [`character_curve`] with `F(0) = 0`.
pub fn character_integral(kind: CharacterKind, u: f64) -> f64 {
    if !u.is_finite() {
        return 0.0;
    }
    match kind {
        CharacterKind::Tape => {
            let a = u.abs();
            if a <= 1.0 {
                let a2 = a * a;
                0.75 * a2 - 0.125 * a2 * a2
            } else {
                0.625 + (a - 1.0)
            }
        }
        CharacterKind::Fuzz => {
            if u >= 0.0 {
                ((1.0 + FUZZ_GAIN * FUZZ_GAIN * u * u).sqrt() - 1.0) / FUZZ_GAIN
            } else {
                let c = FUZZ_GAIN / FUZZ_NEG;
                FUZZ_NEG * ((1.0 + c * c * u * u).sqrt() - 1.0) / c
            }
        }
        CharacterKind::Modulate => {
            let a2 = MODULATE_A * MODULATE_A;
            (1.0 - 1.0 / (1.0 + a2 * u * u).sqrt()) / a2
        }
        CharacterKind::Warp => {
            let a = u.abs();
            if a <= 1.0 {
                let a2 = a * a;
                (7.5 * a2 - 2.5 * a2 * a2 + 0.5 * a2 * a2 * a2) * 0.125
            } else {
                0.687_5 + (a - 1.0)
            }
        }
        CharacterKind::Scream => {
            let hi = 1.0 / SCREAM_GAIN;
            let lo = SCREAM_LOW / SCREAM_GAIN;
            if u > hi {
                0.5 * SCREAM_GAIN * hi * hi + (u - hi)
            } else if u < lo {
                0.5 * SCREAM_GAIN * lo * lo + SCREAM_LOW * (u - lo)
            } else {
                0.5 * SCREAM_GAIN * u * u
            }
        }
    }
}

/// First-order antiderivative anti-aliased output of the character curve for the step
/// `u1 -> u` (the mean of the curve over the step, as for [`Shaper::set_adaa`]); under
/// [`ADAA_EPS`] it falls back to the curve at the midpoint.
#[inline]
pub fn character_adaa(kind: CharacterKind, u: f64, u1: f64) -> f64 {
    let d = u - u1;
    if d.abs() < ADAA_EPS || !d.is_finite() {
        return character_curve(kind, 0.5 * (u + u1));
    }
    (character_integral(kind, u) - character_integral(kind, u1)) / d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oversample::{OversampleFactor, Oversampler};
    use crate::test_util::{
        assert_chunk_invariant_mono, db, noise, rms, sine, spectrum, tone_amplitude,
    };

    const SR: f32 = 48_000.0;
    const KINDS: [ShaperKind; 6] = [
        ShaperKind::SoftClip,
        ShaperKind::Asymmetric,
        ShaperKind::Hard,
        ShaperKind::HardAsymmetric,
        ShaperKind::Fold,
        ShaperKind::Dynamic,
    ];

    #[test]
    fn curve_derivative_matches_a_numeric_derivative() {
        let h = 1e-3f32;
        for kind in KINDS {
            let mut u = -5.0f32;
            while u < 5.0 {
                // Skip the kinks, where the one-sided slopes differ.
                let odd = ((u + 1.0) / 2.0).round() * 2.0 - 1.0;
                let near_kink = (u - odd).abs() < 0.01
                    || [-0.6, 0.0, -1.0 / ASYM_K]
                        .iter()
                        .any(|k| (u - k).abs() < 0.01);
                if !near_kink {
                    let num = (curve(kind, u + h) - curve(kind, u - h)) / (2.0 * h);
                    let an = curve_derivative(kind, u);
                    assert!((num - an).abs() < 2e-2, "{kind:?} u={u}: {num} vs {an}");
                }
                u += 0.0371;
            }
        }
    }

    #[test]
    fn soft_clip_is_odd_monotone_and_bounded_by_two_thirds() {
        let mut prev = f32::MIN;
        let mut u = -6.0f32;
        while u <= 6.0 {
            let y = curve(ShaperKind::SoftClip, u);
            assert!(y >= prev && y.abs() <= SOFT_LIMIT + 1e-6);
            assert!((y + curve(ShaperKind::SoftClip, -u)).abs() < 1e-6);
            prev = y;
            u += 0.01;
        }
        assert!((curve(ShaperKind::SoftClip, 0.01) - 0.01).abs() < 1e-6);
        assert_eq!(
            curve(ShaperKind::SoftClip, 3.0).to_bits(),
            SOFT_LIMIT.to_bits()
        );
    }

    #[test]
    fn fold_and_hard_clip_are_bounded_and_known_at_points() {
        assert_eq!(curve(ShaperKind::Fold, 0.5), 0.5);
        assert!((curve(ShaperKind::Fold, 1.5) - 0.5).abs() < 1e-6);
        assert!(curve(ShaperKind::Fold, 2.0).abs() < 1e-6);
        assert!((curve(ShaperKind::Fold, 3.0) + 1.0).abs() < 1e-6);
        assert!((curve(ShaperKind::Fold, -1.5) + 0.5).abs() < 1e-6);
        let mut u = -40.0f32;
        while u < 40.0 {
            assert!(curve(ShaperKind::Fold, u).abs() <= 1.0 + 1e-6);
            assert!(curve(ShaperKind::Hard, u).abs() <= 1.0);
            assert!((HARD_ASYM_LOW..=1.0).contains(&curve(ShaperKind::HardAsymmetric, u)));
            u += 0.013;
        }
    }

    #[test]
    fn soft_clip_loses_level_with_rising_drive() {
        let x = sine(1000.0, f64::from(SR), 9600, 1.0);
        let mut last = f32::MAX;
        for drive in [0.0f32, 6.0, 12.0, 24.0, 36.0, 48.0] {
            let mut s = Shaper::new(SR);
            s.set_drive_db(drive);
            let mut y = x.clone();
            s.process_block(&mut y);
            let r = rms(&y);
            assert!(r < last, "drive {drive}: {r} not below {last}");
            last = r;
        }
    }

    #[test]
    fn dynamic_type_never_clips_and_matches_its_steady_state_curve() {
        let mut s = Shaper::new(SR);
        s.set_kind(ShaperKind::Dynamic);
        s.reset();
        s.set_drive_db(60.0);
        let big = noise(3, 20_000, 100.0);
        assert!(big.iter().all(|v| s.process(*v).abs() < 1.0));
        // Constant input: the level estimate equals |u|, so y = u / (1 + |u|).
        let mut s = Shaper::new(SR);
        s.set_kind(ShaperKind::Dynamic);
        s.reset();
        let y = (0..2_000).map(|_| s.process(0.5)).last().unwrap();
        assert!((y - 0.5 / 1.5).abs() < 1e-5, "{y}");
        // A sine stays nearly linear: it is a gain, not a curve (no harmonics beyond the
        // slow gain movement).
        let mut s = Shaper::new(SR);
        s.set_kind(ShaperKind::Dynamic);
        s.reset();
        let x = sine(1000.0, f64::from(SR), 9600, 0.3);
        let y: Vec<f32> = x.iter().map(|v| s.process(*v)).collect();
        let h3 = tone_amplitude(&y[4800..], 3000.0, f64::from(SR));
        let h1 = tone_amplitude(&y[4800..], 1000.0, f64::from(SR));
        assert!(h3 / h1 < 0.01, "H3 {}", h3 / h1);
    }

    fn harmonic_db(kind: ShaperKind, n: u32) -> f64 {
        let mut s = Shaper::new(SR);
        s.set_kind(kind);
        s.reset();
        s.set_drive_db(12.0);
        let mut y = sine(1000.0, f64::from(SR), 48_000, 0.4);
        s.process_block(&mut y);
        let y = &y[960..960 + 45_600]; // 950 whole cycles
        let a = tone_amplitude(y, 1000.0 * f64::from(n), f64::from(SR));
        db(a / tone_amplitude(y, 1000.0, f64::from(SR)))
    }

    #[test]
    fn asymmetric_adds_even_harmonics_and_symmetric_does_not() {
        assert!(harmonic_db(ShaperKind::SoftClip, 2) < -90.0);
        assert!(harmonic_db(ShaperKind::Asymmetric, 2) > -40.0);
        assert!(harmonic_db(ShaperKind::SoftClip, 3) > -40.0);
        assert!(harmonic_db(ShaperKind::HardAsymmetric, 2) > -40.0);
    }

    fn alias_energy(y: &[f32], f0_bin: usize, n: usize) -> f64 {
        let m = spectrum(&y[..n]);
        let mut e = 0.0;
        for (k, v) in m.iter().enumerate().skip(2) {
            // Skip bins near a true harmonic below Nyquist; everything else is an alias.
            let mut is_harmonic = false;
            let mut h = f0_bin;
            while h < n / 2 {
                if k.abs_diff(h) <= 4 {
                    is_harmonic = true;
                    break;
                }
                h += f0_bin;
            }
            if !is_harmonic {
                e += v * v;
            }
        }
        e
    }

    #[test]
    fn oversampling_lowers_aliasing_of_a_soft_drive() {
        let n = 8192usize;
        let bin = 853usize; // about 5 kHz at 48 kHz
        let f0 = bin as f64 * f64::from(SR) / n as f64;
        let x = sine(f0, f64::from(SR), n + 2048, 0.7);
        // Direct at the base rate.
        let mut s = Shaper::new(SR);
        s.set_kind(ShaperKind::SoftClip);
        s.reset();
        s.set_drive_db(18.0);
        let mut direct = x.clone();
        s.process_block(&mut direct);
        // 2x oversampled: the shaper runs at twice the rate.
        let mut os = Oversampler::new(64);
        os.set_factor(OversampleFactor::X2);
        let mut s2 = Shaper::new(2.0 * SR);
        s2.set_kind(ShaperKind::SoftClip);
        s2.reset();
        s2.set_drive_db(18.0);
        let mut over = vec![0.0f32; x.len()];
        for (xi, yi) in x.chunks(64).zip(over.chunks_mut(64)) {
            let up = os.upsample(xi);
            s2.process_block(up);
            os.downsample(yi);
        }
        let a_direct = alias_energy(&direct[1024..], bin, n);
        let a_over = alias_energy(&over[1024..], bin, n);
        assert!(
            a_over < a_direct * 0.1,
            "aliases {a_direct} direct vs {a_over} oversampled"
        );
    }

    const ADAA_KINDS: [ShaperKind; 4] = [
        ShaperKind::SoftClip,
        ShaperKind::Asymmetric,
        ShaperKind::Hard,
        ShaperKind::HardAsymmetric,
    ];

    #[test]
    fn the_antiderivatives_integrate_the_curves() {
        for kind in ADAA_KINDS {
            assert_eq!(curve_integral(kind, 0.0), Some(0.0), "{kind:?}");
            let h = 1.0e-5f64;
            let mut u = -4.0f64;
            while u < 4.0 {
                let f = |x: f64| curve_integral(kind, x).unwrap();
                let num = (f(u + h) - f(u - h)) / (2.0 * h);
                let want = f64::from(curve(kind, u as f32));
                // Skip the kinks of the curve, where the one-sided slopes differ.
                let near_kink = [-1.0, -0.6, -1.0 / 1.6, 0.0, 1.0]
                    .iter()
                    .any(|k| (u - k).abs() < 2e-4);
                if !near_kink {
                    assert!((num - want).abs() < 2e-3, "{kind:?} u={u}: {num} vs {want}");
                }
                u += 0.00937;
            }
        }
        assert!(curve_integral(ShaperKind::Fold, 0.5).is_none());
        assert!(curve_integral(ShaperKind::Dynamic, 0.5).is_none());
    }

    #[test]
    fn adaa_is_off_by_default_and_the_flag_is_reported() {
        let mut s = Shaper::new(SR);
        assert!(!s.adaa());
        s.set_drive_db(12.0);
        let mut t = s.clone();
        t.set_adaa(false);
        let x = noise(2, 500, 1.0);
        let a: Vec<f32> = x.iter().map(|v| s.process(*v)).collect();
        let b: Vec<f32> = x.iter().map(|v| t.process(*v)).collect();
        assert_eq!(a, b);
        s.set_adaa(true);
        assert!(s.adaa());
    }

    #[test]
    fn adaa_is_the_curve_half_a_sample_earlier_for_a_slow_signal() {
        let (f0, drive) = (1_000.0f64, 6.0f32);
        for kind in ADAA_KINDS {
            let mut s = Shaper::new(SR);
            s.set_kind(kind);
            s.reset();
            s.set_drive_db(drive);
            s.set_adaa(true);
            let g = s.drive_gain();
            let scale = 1.0 / g.max(1.0).sqrt();
            for n in 0..2_000usize {
                let t = |k: f64| (std::f64::consts::TAU * f0 * k / f64::from(SR)).sin() as f32;
                let y = s.process(0.5 * t(n as f64));
                if n > 2 {
                    let want = curve(kind, g * 0.5 * t(n as f64 - 0.5)) * scale;
                    assert!((y - want).abs() < 3e-3, "{kind:?} n={n}: {y} vs {want}");
                }
            }
        }
    }

    #[test]
    fn adaa_lowers_the_aliasing_of_every_curve_it_covers() {
        let n = 8192usize;
        let bin = 853usize; // about 5 kHz at 48 kHz
        let f0 = bin as f64 * f64::from(SR) / n as f64;
        let x = sine(f0, f64::from(SR), n + 2048, 0.7);
        for (kind, drive) in [
            (ShaperKind::SoftClip, 18.0f32),
            (ShaperKind::Asymmetric, 18.0),
            (ShaperKind::Hard, 24.0),
            (ShaperKind::HardAsymmetric, 24.0),
        ] {
            let run = |adaa: bool| {
                let mut s = Shaper::new(SR);
                s.set_kind(kind);
                s.reset();
                s.set_drive_db(drive);
                s.set_adaa(adaa);
                let mut y = x.clone();
                s.process_block(&mut y);
                alias_energy(&y[1024..], bin, n)
            };
            let (plain, adaa) = (run(false), run(true));
            assert!(
                adaa < plain * 0.35,
                "{kind:?}: aliases {plain} plain vs {adaa} with ADAA"
            );
        }
    }

    #[test]
    fn adaa_survives_hostile_input_and_ignores_fold_and_dynamic() {
        for kind in KINDS {
            let mut s = Shaper::new(SR);
            s.set_kind(kind);
            s.reset();
            s.set_drive_db(60.0);
            s.set_adaa(true);
            for i in 0..400usize {
                let v = match i % 7 {
                    0 => f32::NAN,
                    1 => f32::INFINITY,
                    2 => -1.0e30,
                    3 => 1.0e-30,
                    _ => ((i * 37 % 19) as f32 - 9.0) * 0.4,
                };
                let o = s.process(v);
                assert!(o.is_finite() || !v.is_finite(), "{kind:?} {v}");
            }
            for _ in 0..4 {
                s.process(0.1);
            }
            assert!(s.process(0.1).is_finite(), "{kind:?} recovers");
        }
        // The fold type produces the same bits with and without the flag.
        let x = noise(3, 300, 1.0);
        let run = |adaa: bool| {
            let mut s = Shaper::new(SR);
            s.set_kind(ShaperKind::Fold);
            s.reset();
            s.set_drive_db(12.0);
            s.set_adaa(adaa);
            x.iter().map(|v| s.process(*v)).collect::<Vec<_>>()
        };
        assert_eq!(run(false), run(true));
    }

    #[test]
    fn kind_switch_crossfades_to_the_new_curve() {
        let mut s = Shaper::new(SR);
        s.set_drive_db(12.0);
        s.set_kind(ShaperKind::Hard);
        let first = s.process(0.3);
        let g = s.drive_gain();
        let hard = (0.3 * g).clamp(-1.0, 1.0) / g.sqrt();
        let soft_y = curve(ShaperKind::SoftClip, 0.3 * g) / g.sqrt();
        assert!((first - soft_y).abs() < 0.02, "starts at the old curve");
        for _ in 0..FADE_SAMPLES {
            s.process(0.3);
        }
        assert!(
            (s.process(0.3) - hard).abs() < 1e-6,
            "ends at the new curve"
        );
        assert_eq!(s.kind(), ShaperKind::Hard);
        s.set_kind(ShaperKind::Hard);
    }

    #[test]
    fn drive_is_clamped_and_nan_ignored() {
        let mut s = Shaper::new(SR);
        s.set_drive_db(1000.0);
        let g = s.drive_gain();
        s.set_drive_db(f32::NAN);
        assert_eq!(s.drive_gain().to_bits(), g.to_bits());
        s.set_drive_db(-1000.0);
        assert!(s.drive_gain() > 0.05);
        assert!(s.process(1.0).is_finite());
    }

    #[test]
    fn process_block_is_chunk_invariant() {
        let x = noise(9, 2_000, 1.0);
        for kind in KINDS {
            assert_chunk_invariant_mono(
                || {
                    let mut s = Shaper::new(SR);
                    s.set_drive_db(20.0);
                    s.set_kind(kind);
                    s
                },
                |s, b| s.process_block(b),
                &x,
            );
        }
    }

    proptest::proptest! {
        #[test]
        fn output_is_finite_and_bounded_for_any_parameters(
            drive in -30.0f32..70.0, kind in 0usize..6, seed in 0u64..1000
        ) {
            let mut s = Shaper::new(SR);
            s.set_drive_db(drive);
            s.set_kind(KINDS[kind]);
            let mut x = noise(seed, 512, 8.0);
            s.process_block(&mut x);
            proptest::prop_assert!(x.iter().all(|v| v.is_finite()));
            proptest::prop_assert!(x.iter().all(|v| v.abs() <= 16.0));
        }
    }

    #[test]
    fn character_integrals_differentiate_to_their_curves() {
        let h = 1e-6f64;
        for kind in CharacterKind::ALL {
            assert_eq!(character_integral(kind, 0.0), 0.0, "{kind:?}");
            let mut u = -6.0f64;
            while u < 6.0 {
                let d =
                    (character_integral(kind, u + h) - character_integral(kind, u - h)) / (2.0 * h);
                let want = character_curve(kind, u);
                assert!((d - want).abs() < 1e-4, "{kind:?} u={u}: {d} vs {want}");
                u += 0.0371;
            }
        }
    }

    #[test]
    fn character_curves_are_bounded_by_one_and_hold_their_shapes() {
        for kind in CharacterKind::ALL {
            let mut u = -50.0f64;
            while u < 50.0 {
                assert!(character_curve(kind, u).abs() <= 1.0 + 1e-9, "{kind:?} {u}");
                u += 0.013;
            }
            assert_eq!(character_curve(kind, f64::NAN), 0.0);
        }
        // Odd curves are odd; fuzz and scream are not.
        for kind in [
            CharacterKind::Tape,
            CharacterKind::Modulate,
            CharacterKind::Warp,
        ] {
            assert_eq!(character_curve(kind, 0.37), -character_curve(kind, -0.37));
        }
        assert!(character_curve(CharacterKind::Fuzz, -9.0) > -0.61);
        assert!(character_curve(CharacterKind::Fuzz, 9.0) > 0.99);
        assert_eq!(character_curve(CharacterKind::Scream, -9.0), -0.5);
        assert_eq!(character_curve(CharacterKind::Scream, 9.0), 1.0);
        // Modulate peaks near 1 and falls back after its peak.
        let peak = character_curve(CharacterKind::Modulate, 1.0 / (MODULATE_A * 2f64.sqrt()));
        assert!((peak - 1.0).abs() < 1e-3);
        assert!(character_curve(CharacterKind::Modulate, 30.0) < 0.1);
    }

    #[test]
    fn adaa_lowers_the_aliasing_of_every_character_curve() {
        let n = 8192usize;
        let bin = 853usize;
        let f0 = bin as f64 * f64::from(SR) / n as f64;
        let x = sine(f0, f64::from(SR), n + 2048, 0.7);
        for (kind, drive) in [
            (CharacterKind::Tape, 12.0f64),
            (CharacterKind::Fuzz, 12.0),
            (CharacterKind::Modulate, 6.0),
            (CharacterKind::Warp, 12.0),
            (CharacterKind::Scream, 12.0),
        ] {
            let g = 10f64.powf(drive / 20.0);
            let run = |adaa: bool| {
                let mut u1 = 0.0f64;
                let y: Vec<f32> = x
                    .iter()
                    .map(|v| {
                        let u = f64::from(*v) * g;
                        let o = if adaa {
                            character_adaa(kind, u, u1)
                        } else {
                            character_curve(kind, u)
                        };
                        u1 = u;
                        o as f32
                    })
                    .collect();
                alias_energy(&y[1024..], bin, n)
            };
            let (plain, adaa) = (run(false), run(true));
            assert!(
                adaa < plain * 0.5,
                "{kind:?}: aliases {plain} plain vs {adaa} with ADAA"
            );
        }
    }

    #[test]
    fn character_adaa_is_finite_for_hostile_steps() {
        for kind in CharacterKind::ALL {
            for (u, u1) in [
                (1e30, -1e30),
                (f64::NAN, 0.0),
                (0.0, f64::INFINITY),
                (0.5, 0.5),
            ] {
                assert!(character_adaa(kind, u, u1).is_finite(), "{kind:?} {u} {u1}");
            }
        }
    }
}
