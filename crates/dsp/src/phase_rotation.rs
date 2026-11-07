//! Phase rotation: a first-order all-pass that turns the phase of a signal by a set angle at a
//! set frequency without changing its level at any frequency.
//!
//! The section `y[n] = a * x[n] + x[n-1] - a * y[n-1]` has a gain of exactly 1 at every
//! frequency and a phase that lags from 0 at DC towards 180 degrees at the top of the band,
//! reaching 90 degrees at its corner. [`PhaseRotator::set`] places the corner so that the lag is
//! `angle` at `freq_hz`: below that frequency the signal is turned by less, above it by more (up
//! to 180 degrees), so the rotation is felt most where the energy of a bass or a kick is. An
//! angle of 0 is the identity (the input bit for bit) and an angle of 180 is a polarity
//! inversion (`-x` exactly).
//!
//! Turning the phase reshapes the waveform in time: a signal whose positive and negative swings
//! are uneven (a filtered or naturally asymmetric source) can be turned until the two are close,
//! which lowers its peak. [`best_rotation`] searches the angle that does that for a given
//! signal and reports the headroom gained. It is pure and deterministic (IEEE `+ - * /`, `tan`
//! and `atan` only) and runs off the real-time thread.

use std::f64::consts::PI;

/// Largest rotation angle, degrees (a polarity inversion).
pub const MAX_ANGLE_DEG: f32 = 180.0;
/// An angle at or below this is the identity.
const IDENTITY_BELOW_DEG: f32 = 0.005;
/// An angle at or above this is a polarity inversion.
const INVERT_ABOVE_DEG: f32 = 179.5;
/// The gain in headroom (dB) below which [`best_rotation`] leaves the signal alone.
pub const MIN_GAIN_DB: f32 = 0.1;

/// The all-pass section turning the phase by an angle at a frequency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhaseRotator {
    mode: Mode,
    a: f64,
    x1: f64,
    y1: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Identity,
    Section,
    Invert,
}

impl Default for PhaseRotator {
    fn default() -> Self {
        Self::new()
    }
}

impl PhaseRotator {
    /// A rotator at rest (the identity).
    pub const fn new() -> Self {
        Self {
            mode: Mode::Identity,
            a: 0.0,
            x1: 0.0,
            y1: 0.0,
        }
    }

    /// Turns the phase by `angle_deg` (0 to 180, clamped) at `freq_hz` for `sample_rate`. The
    /// state is kept, so a change while the signal runs does not click more than the change of
    /// the phase itself.
    pub fn set(&mut self, angle_deg: f32, freq_hz: f32, sample_rate: f32) {
        let angle = if angle_deg.is_nan() {
            0.0
        } else {
            angle_deg.clamp(0.0, MAX_ANGLE_DEG)
        };
        self.mode = if angle <= IDENTITY_BELOW_DEG {
            Mode::Identity
        } else if angle >= INVERT_ABOVE_DEG {
            Mode::Invert
        } else {
            Mode::Section
        };
        if self.mode == Mode::Section {
            self.a = coefficient(angle, freq_hz, sample_rate);
        }
    }

    /// Whether the rotator passes its input unchanged.
    pub fn is_identity(&self) -> bool {
        self.mode == Mode::Identity
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    /// Filters one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        match self.mode {
            Mode::Identity => x,
            Mode::Invert => -x,
            Mode::Section => {
                let xi = f64::from(x);
                let mut y = self.a * xi + self.x1 - self.a * self.y1;
                if y.abs() < 1e-30 {
                    y = 0.0;
                }
                self.x1 = xi;
                self.y1 = y;
                y as f32
            }
        }
    }
}

/// The coefficient `a` of the section whose lag is `angle_deg` (strictly between 0 and 180) at
/// `freq_hz`.
fn coefficient(angle_deg: f32, freq_hz: f32, sample_rate: f32) -> f64 {
    let sr = f64::from(sample_rate.max(1.0));
    let f = f64::from(freq_hz).clamp(1.0, 0.45 * sr);
    let at = (PI * f / sr).tan();
    let t = at / (f64::from(angle_deg) * PI / 360.0).tan();
    (t - 1.0) / (t + 1.0)
}

/// The lag in degrees (0 to 180) that a rotator set to `angle_deg` at `freq_hz` gives at
/// `at_hz`, for `sample_rate`.
pub fn lag_at(angle_deg: f32, freq_hz: f32, sample_rate: f32, at_hz: f32) -> f32 {
    let angle = angle_deg.clamp(0.0, MAX_ANGLE_DEG);
    if angle <= IDENTITY_BELOW_DEG {
        return 0.0;
    }
    if angle >= INVERT_ABOVE_DEG {
        return MAX_ANGLE_DEG;
    }
    let sr = f64::from(sample_rate.max(1.0));
    let a = coefficient(angle, freq_hz, sr as f32);
    let w = 2.0 * PI * f64::from(at_hz).clamp(0.0, 0.5 * sr) / sr;
    // H(e^jw) = (a + e^-jw) / (1 + a e^-jw)
    let (s, c) = w.sin_cos();
    let (nr, ni) = (a + c, -s);
    let (dr, di) = (1.0 + a * c, -a * s);
    let phase = ni.atan2(nr) - di.atan2(dr);
    let lag = (-phase).rem_euclid(2.0 * PI);
    (lag * 180.0 / PI) as f32
}

/// The largest positive and the largest negative swing of `x`, both as magnitudes.
pub fn peaks(x: &[f32]) -> (f32, f32) {
    x.iter().fold((0.0f32, 0.0f32), |(p, n), &s| {
        if !s.is_finite() {
            (p, n)
        } else {
            (p.max(s), n.max(-s))
        }
    })
}

/// What [`best_rotation`] found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RotationFix {
    /// The angle to rotate by, degrees (0: nothing to gain).
    pub angle_deg: f32,
    /// Positive and negative peak before, as magnitudes.
    pub before: (f32, f32),
    /// Positive and negative peak after the rotation.
    pub after: (f32, f32),
    /// The headroom the rotation gives, dB (the drop of the larger peak; 0 for no rotation).
    pub headroom_db: f32,
}

fn rotated_peaks(x: &[f32], angle: f32, freq_hz: f32, sample_rate: f32) -> (f32, f32) {
    let mut r = PhaseRotator::new();
    r.set(angle, freq_hz, sample_rate);
    let mut pos = 0.0f32;
    let mut neg = 0.0f32;
    for &s in x {
        let s = if s.is_finite() { s } else { 0.0 };
        let y = r.process(s);
        pos = pos.max(y);
        neg = neg.max(-y);
    }
    (pos, neg)
}

/// The rotation angle (0 to 180 degrees, whole degrees) at `freq_hz` that lowers the larger
/// peak of `x` most, with the peaks before and after and the headroom gained. A signal whose
/// swings are already even, or a gain under [`MIN_GAIN_DB`], gives angle 0. Ties go to the
/// smaller angle.
pub fn best_rotation(x: &[f32], sample_rate: f32, freq_hz: f32) -> RotationFix {
    let before = peaks(x);
    let top = |p: (f32, f32)| p.0.max(p.1);
    let none = RotationFix {
        angle_deg: 0.0,
        before,
        after: before,
        headroom_db: 0.0,
    };
    if top(before) <= 0.0 {
        return none;
    }
    let mut best = (0.0f32, top(before));
    let consider = |angle: f32, best: &mut (f32, f32)| {
        let p = top(rotated_peaks(x, angle, freq_hz, sample_rate));
        if p < best.1 - 1e-9 {
            *best = (angle, p);
        }
    };
    let mut angle = 5.0;
    while angle < 180.0 {
        consider(angle, &mut best);
        angle += 5.0;
    }
    let centre = best.0;
    if centre > 0.0 {
        let lo = (centre - 4.0).max(1.0) as i32;
        let hi = (centre + 4.0).min(179.0) as i32;
        for a in lo..=hi {
            consider(a as f32, &mut best);
        }
    }
    let gain = 20.0 * (top(before) / best.1.max(1e-12)).log10();
    if gain < MIN_GAIN_DB {
        return none;
    }
    RotationFix {
        angle_deg: best.0,
        before,
        after: rotated_peaks(x, best.0, freq_hz, sample_rate),
        headroom_db: gain,
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(hz: f32, phase: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / SR + phase).sin())
            .collect()
    }

    fn run(r: &mut PhaseRotator, x: &[f32]) -> Vec<f32> {
        x.iter().map(|&s| r.process(s)).collect()
    }

    #[test]
    fn angle_zero_is_the_input_bit_for_bit_and_180_inverts() {
        let x = sine(60.0, 0.3, 4_800);
        let mut r = PhaseRotator::new();
        r.set(0.0, 100.0, SR);
        assert!(r.is_identity());
        assert_eq!(run(&mut r, &x), x);
        r.set(180.0, 100.0, SR);
        let y = run(&mut r, &x);
        assert!(x.iter().zip(&y).all(|(a, b)| *b == -*a));
    }

    #[test]
    fn the_lag_at_the_set_frequency_is_the_angle() {
        for angle in [20.0, 45.0, 90.0, 135.0, 170.0] {
            for f in [40.0, 100.0, 400.0] {
                let lag = lag_at(angle, f, SR, f);
                assert!((lag - angle).abs() < 0.05, "{angle} at {f}: {lag}");
            }
        }
        // Less below the set frequency, more above it.
        assert!(lag_at(90.0, 100.0, SR, 25.0) < 90.0);
        assert!(lag_at(90.0, 100.0, SR, 400.0) > 90.0);
        assert!(lag_at(90.0, 100.0, SR, 20_000.0) <= 180.0);
    }

    #[test]
    fn the_level_is_the_same_at_every_frequency() {
        for hz in [30.0, 80.0, 250.0, 2_000.0] {
            let x = sine(hz, 0.0, 48_000);
            let mut r = PhaseRotator::new();
            r.set(90.0, 100.0, SR);
            let y = run(&mut r, &x);
            let rms =
                |v: &[f32]| (v[24_000..].iter().map(|s| s * s).sum::<f32>() / 24_000.0).sqrt();
            assert!((rms(&y) / rms(&x) - 1.0).abs() < 0.002, "{hz}");
        }
    }

    #[test]
    fn a_measured_phase_matches_the_formula() {
        // A 100 Hz sine turned by 90 degrees at 100 Hz comes out as a cosine: the lag shifts
        // the zero crossing by a quarter period (120 samples).
        let n = 48_000;
        let x = sine(100.0, 0.0, n);
        let mut r = PhaseRotator::new();
        r.set(90.0, 100.0, SR);
        let y = run(&mut r, &x);
        let expect = sine(100.0, -std::f32::consts::FRAC_PI_2, n);
        let err = y[30_000..]
            .iter()
            .zip(&expect[30_000..])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(err < 0.01, "{err}");
    }

    #[test]
    fn the_best_rotation_of_an_uneven_wave_gains_headroom() {
        // A sine with a second harmonic is lopsided: its positive peak is far above its
        // negative one at some phases.
        let n = 48_000;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = 2.0 * std::f32::consts::PI * 60.0 * i as f32 / SR;
                0.6 * t.sin() + 0.4 * (2.0 * t + 1.0).sin()
            })
            .collect();
        let fix = best_rotation(&x, SR, 60.0);
        assert!(fix.angle_deg > 0.0, "{fix:?}");
        assert!(fix.headroom_db > 0.3, "{fix:?}");
        let (p, q) = fix.after;
        assert!((p.max(q) / fix.before.0.max(fix.before.1)) < 1.0);
    }

    #[test]
    fn a_symmetric_wave_and_silence_are_left_alone() {
        let x = sine(60.0, 0.0, 48_000);
        assert_eq!(best_rotation(&x, SR, 60.0).angle_deg, 0.0);
        assert_eq!(best_rotation(&[0.0; 100], SR, 60.0).headroom_db, 0.0);
        assert_eq!(best_rotation(&[], SR, 60.0).angle_deg, 0.0);
    }

    #[test]
    fn non_finite_input_does_not_poison_the_search() {
        let mut x = sine(60.0, 0.0, 4_800);
        x[10] = f32::NAN;
        x[20] = f32::INFINITY;
        let fix = best_rotation(&x, SR, 60.0);
        assert!(fix.before.0.is_finite() && fix.after.0.is_finite());
    }
}
