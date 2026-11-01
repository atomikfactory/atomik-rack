//! Four-pole (24 dB/oct) lowpass ladder filter with an anti-aliased drive stage.
//!
//! * **Core.** Four trapezoidal (TPT) one-pole lowpass stages in series with the output fed back
//!   to the input through the gain `k`. The feedback loop has no unit delay: the four stage
//!   equations are solved together (`y4 = G^4 u + S` with `S` the states' contribution), so the
//!   resonance peak sits exactly at the cutoff for any cutoff, and `u = (x - k S) / (1 + k G^4)`
//!   follows in closed form. The state is `f64`.
//! * **Resonance.** `r` in `0..=1` maps to `k = 3.9 r`: the loop gain reaches `3.9` where the
//!   linear ladder would oscillate at `4`, so the poles stay inside the unit circle for every
//!   cutoff and the filter rings strongly at `r = 1` but never self-oscillates or blows up.
//! * **Drive.** A cubic soft clipper `x - x^3/3` (saturating at `2/3`) in front of the loop, with
//!   first-order antiderivative anti-aliasing (ADAA): the output of each sample is the mean of
//!   the clipper over the input's path since the last sample, taken from the closed-form
//!   antiderivative, which removes most of the aliasing the hard corners of a clipper make.
//!   A drive of 0 bypasses the stage exactly.
//! * **Determinism.** `g = tan(pi fc / sr)` comes from [`Svf::cutoff_to_g_det`] and everything
//!   else is IEEE `+ - * /`, so every platform produces the same bits.

use crate::Svf;

/// Loop gain at full resonance (the linear ladder oscillates at 4).
pub const MAX_FEEDBACK: f64 = 3.9;
/// Input gain at full drive (1 at no drive).
const MAX_DRIVE_GAIN: f64 = 16.0;
/// Inputs closer than this are clipped at their mean instead of through the antiderivative
/// (which divides by the difference).
const ADAA_EPSILON: f64 = 1.0e-6;

/// The cubic soft clipper, saturating at `+-2/3` beyond `+-1`.
#[inline]
fn clip(x: f64) -> f64 {
    if x >= 1.0 {
        2.0 / 3.0
    } else if x <= -1.0 {
        -2.0 / 3.0
    } else {
        x - x * x * x * (1.0 / 3.0)
    }
}

/// The antiderivative of [`clip`]: `x^2/2 - x^4/12` inside `+-1`, then linear (continuous at 1).
#[inline]
fn clip_integral(x: f64) -> f64 {
    let a = x.abs();
    if a <= 1.0 {
        let x2 = x * x;
        x2 * 0.5 - x2 * x2 * (1.0 / 12.0)
    } else {
        a * (2.0 / 3.0) - 0.25
    }
}

/// The drive stage on its own: the anti-aliased clipper (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drive {
    gain: f64,
    previous: f64,
}

impl Default for Drive {
    fn default() -> Self {
        Self::new()
    }
}

impl Drive {
    /// A drive stage at drive 0 (bypassed).
    pub fn new() -> Self {
        Self {
            gain: 1.0,
            previous: 0.0,
        }
    }

    /// Sets the drive `0..=1` (clamped; NaN gives 0); 0 is a bypass.
    pub fn set(&mut self, drive: f32) {
        let d = if drive.is_nan() {
            0.0
        } else {
            f64::from(drive.clamp(0.0, 1.0))
        };
        self.gain = 1.0 + (MAX_DRIVE_GAIN - 1.0) * d;
    }

    /// Whether the stage changes the signal.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.gain > 1.0
    }

    /// Clears the antiderivative memory.
    pub fn reset(&mut self) {
        self.previous = 0.0;
    }

    /// Clips one sample. The output is scaled by 1.5 so the clip level is 1.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        if !self.is_active() {
            return x;
        }
        let a = f64::from(x) * self.gain;
        let d = a - self.previous;
        let y = if d.abs() < ADAA_EPSILON {
            clip(0.5 * (a + self.previous))
        } else {
            (clip_integral(a) - clip_integral(self.previous)) / d
        };
        self.previous = a;
        (y * 1.5) as f32
    }
}

/// TPT ladder lowpass (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ladder {
    /// The one-pole coefficient `G = g / (1 + g)`.
    big_g: f64,
    k: f64,
    state: [f64; 4],
    drive: Drive,
}

impl Default for Ladder {
    fn default() -> Self {
        Self::new()
    }
}

impl Ladder {
    /// A ladder at 1 kHz / 48 kHz, no resonance, no drive.
    pub fn new() -> Self {
        let mut l = Self {
            big_g: 0.0,
            k: 0.0,
            state: [0.0; 4],
            drive: Drive::new(),
        };
        l.set_params(1_000.0, 0.0, 48_000.0);
        l
    }

    /// The integrator gain `g = tan(pi fc / sr)` for a cutoff (deterministic).
    #[inline]
    pub fn cutoff_to_g(cutoff_hz: f32, sample_rate: f32) -> f32 {
        Svf::cutoff_to_g_det(cutoff_hz, sample_rate)
    }

    /// The loop gain for a resonance in `0..=1` (clamped; NaN gives 0).
    #[inline]
    pub fn resonance_to_k(resonance: f32) -> f32 {
        let r = if resonance.is_nan() {
            0.0
        } else {
            resonance.clamp(0.0, 1.0)
        };
        (MAX_FEEDBACK * f64::from(r)) as f32
    }

    /// Sets cutoff (Hz), resonance (`0..=1`) and the sample rate.
    pub fn set_params(&mut self, cutoff_hz: f32, resonance: f32, sample_rate: f32) {
        self.set_coefficients(
            Self::cutoff_to_g(cutoff_hz, sample_rate),
            Self::resonance_to_k(resonance),
        );
    }

    /// Sets the raw coefficients: the integrator gain `g >= 0` and the loop gain `k` (clamped
    /// to `0..=MAX_FEEDBACK`).
    #[inline]
    pub fn set_coefficients(&mut self, g: f32, k: f32) {
        let g = f64::from(g.max(0.0));
        self.big_g = g / (1.0 + g);
        self.k = if k.is_nan() {
            0.0
        } else {
            f64::from(k).clamp(0.0, MAX_FEEDBACK)
        };
    }

    /// Sets the drive `0..=1` (see [`Drive::set`]).
    pub fn set_drive(&mut self, drive: f32) {
        self.drive.set(drive);
    }

    /// Clears the filter state and the drive memory.
    pub fn reset(&mut self) {
        self.state = [0.0; 4];
        self.drive.reset();
    }

    /// Filters one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = f64::from(self.drive.process(x));
        let g = self.big_g;
        let h = 1.0 - g;
        let [s0, s1, s2, s3] = self.state;
        // The states' share of the output: y4 = G^4 u + share.
        let share = h * (g * g * g * s0 + g * g * s1 + g * s2 + s3);
        let g4 = g * g * g * g;
        let u = (x - self.k * share) / (1.0 + self.k * g4);
        let y1 = g * u + h * s0;
        let y2 = g * y1 + h * s1;
        let y3 = g * y2 + h * s2;
        let y4 = g * y3 + h * s3;
        self.state = [
            flush(2.0 * y1 - s0),
            flush(2.0 * y2 - s1),
            flush(2.0 * y3 - s2),
            flush(2.0 * y4 - s3),
        ];
        y4 as f32
    }
}

#[inline]
fn flush(v: f64) -> f64 {
    if v.abs() < 1.0e-20 { 0.0 } else { v }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;
    use crate::test_util::{rms, spectrum};
    use proptest::prelude::*;

    const SR: f32 = 48_000.0;

    fn gain_db(l: &mut Ladder, hz: f32) -> f32 {
        l.reset();
        let n = 48_000;
        let y: Vec<f32> = (0..n)
            .map(|i| l.process((std::f32::consts::TAU * hz * i as f32 / SR).sin()))
            .collect();
        20.0 * (rms(&y[n / 2..]) / std::f32::consts::FRAC_1_SQRT_2).log10()
    }

    #[test]
    fn the_ladder_rolls_off_at_24_db_per_octave() {
        let mut l = Ladder::new();
        l.set_params(1_000.0, 0.0, SR);
        let pass = gain_db(&mut l, 100.0);
        let g4 = gain_db(&mut l, 4_000.0);
        let g8 = gain_db(&mut l, 8_000.0);
        assert!(pass.abs() < 0.3, "passband {pass}");
        assert!((g4 - g8 - 24.0).abs() < 3.0, "octave slope {g4} -> {g8}");
        assert!(g4 < -40.0, "4 kHz {g4}");
        // Four poles of -3 dB each at the cutoff.
        assert!(gain_db(&mut l, 1_000.0) < -9.0);
    }

    #[test]
    fn resonance_peaks_near_the_cutoff() {
        let mut l = Ladder::new();
        l.set_params(2_000.0, 0.9, SR);
        // The strongest tone over a sweep.
        let best = (5..60)
            .map(|i| {
                let hz = 200.0 * i as f32;
                (hz, gain_db(&mut l, hz))
            })
            .fold((0.0f32, f32::MIN), |m, x| if x.1 > m.1 { x } else { m });
        assert!(best.1 > 6.0, "peak {best:?}");
        assert!((best.0 - 2_000.0).abs() < 800.0, "peak at {best:?}");
        l.set_params(2_000.0, 0.0, SR);
        assert!(gain_db(&mut l, best.0) < best.1 - 6.0);
    }

    #[test]
    fn the_deterministic_tan_agrees_with_libm() {
        for fc in [20.0f32, 440.0, 4_000.0, 12_000.0, 21_000.0] {
            let want = (std::f32::consts::PI * fc / SR).tan();
            let got = Ladder::cutoff_to_g(fc, SR);
            assert!((got - want).abs() <= want * 1e-5, "{fc}: {got} vs {want}");
        }
        assert!((Svf::resonance_to_k_det(0.4) - 1.0 / Svf::resonance_to_q(0.4)).abs() < 1e-5);
        assert!(Ladder::cutoff_to_g(f32::NAN, SR).is_finite());
    }

    #[test]
    fn drive_zero_is_a_bypass_and_drive_saturates() {
        let mut d = Drive::new();
        for x in [-3.0f32, -0.2, 0.0, 0.7, 5.0] {
            assert_eq!(d.process(x).to_bits(), x.to_bits());
        }
        d.set(1.0);
        let big = (0..64).map(|_| d.process(4.0)).last().unwrap();
        assert!((big - 1.0).abs() < 1e-4, "{big}");
        let neg = (0..64).map(|_| d.process(-4.0)).last().unwrap();
        assert!((neg + 1.0).abs() < 1e-4, "{neg}");
    }

    /// Worst alias, in dB below the fundamental, of a driven sine (partials of `f0` that fold
    /// back below 20 kHz).
    fn alias_db(clipped: &[f32], f0: f64) -> f64 {
        let m = spectrum(clipped);
        let n = clipped.len() as f64;
        let bin = |hz: f64| (hz * n / f64::from(SR)).round() as usize;
        let fund = m[bin(f0) - 3..=bin(f0) + 3]
            .iter()
            .fold(0.0f64, |a, b| a.max(*b));
        let mut masked = m.clone();
        let mut k = 1.0;
        while k * f0 < f64::from(SR) / 2.0 {
            let c = bin(k * f0);
            for v in masked
                .iter_mut()
                .take((c + 7).min(m.len()))
                .skip(c.saturating_sub(6))
            {
                *v = 0.0;
            }
            k += 1.0;
        }
        for v in masked.iter_mut().take(6) {
            *v = 0.0;
        }
        let worst = masked[..bin(20_000.0)]
            .iter()
            .fold(0.0f64, |a, b| a.max(*b))
            .max(1e-300);
        20.0 * (worst / fund).log10()
    }

    #[test]
    fn the_drive_stage_aliases_less_than_a_plain_clipper() {
        let n = 1 << 15;
        let f0 = 3_137.0f64;
        let x: Vec<f32> = (0..n)
            .map(|i| (std::f64::consts::TAU * f0 * i as f64 / f64::from(SR)).sin() as f32)
            .collect();
        let mut d = Drive::new();
        d.set(1.0);
        let adaa: Vec<f32> = x.iter().map(|s| d.process(*s)).collect();
        let plain: Vec<f32> = x
            .iter()
            .map(|s| (clip(f64::from(*s) * MAX_DRIVE_GAIN) * 1.5) as f32)
            .collect();
        let (a, p) = (alias_db(&adaa, f0), alias_db(&plain, f0));
        assert!(a < p - 4.0, "ADAA {a} dB vs plain {p} dB");
    }

    #[test]
    fn full_drive_through_the_ladder_stays_finite_and_bounded() {
        let mut l = Ladder::new();
        l.set_params(3_000.0, 1.0, SR);
        l.set_drive(1.0);
        let mut rng = Rng::new(7);
        let peak = (0..96_000)
            .map(|_| l.process(rng.next_bipolar() * 4.0).abs())
            .fold(0.0f32, f32::max);
        assert!(peak.is_finite() && peak < 80.0, "{peak}");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn the_ladder_is_stable_at_full_resonance_for_every_cutoff(
            ratio in 0.0f32..=1.0,
            sr in prop::sample::select(vec![44_100.0f32, 48_000.0, 96_000.0]),
            seed in any::<u64>(),
            drive in 0.0f32..=1.0,
            modulate in any::<bool>(),
        ) {
            let lo = 20.0f32;
            let hi = 0.45 * sr;
            let cutoff = lo * (hi / lo).powf(ratio);
            let mut l = Ladder::new();
            l.set_params(cutoff, 1.0, sr);
            l.set_drive(drive);
            let mut rng = Rng::new(seed);
            let n = (sr * 0.25) as usize;
            let mut peak = 0.0f32;
            for i in 0..n {
                if modulate && i % 32 == 0 {
                    let r = rng.next_f32();
                    l.set_params(lo * (hi / lo).powf(r), 1.0, sr);
                }
                let sq = if (i as f32 * cutoff / sr).fract() < 0.5 { 1.0 } else { -1.0 };
                let y = l.process(0.5 * rng.next_bipolar() + 0.5 * sq);
                prop_assert!(y.is_finite());
                peak = peak.max(y.abs());
            }
            // The resonant gain of the 4-pole core at k = 3.9 is about 40.
            prop_assert!(peak < 200.0, "peak {}", peak);
            // Free decay: the energy does not grow.
            let tail: Vec<f32> = (0..(sr * 0.2) as usize).map(|_| l.process(0.0)).collect();
            let q = tail.len() / 4;
            prop_assert!(rms(&tail[3 * q..]) <= rms(&tail[..q]) + 1e-6);
        }
    }

    #[test]
    fn the_ladder_is_deterministic_and_resets() {
        let run = || {
            let mut l = Ladder::new();
            l.set_params(1_500.0, 0.8, SR);
            l.set_drive(0.5);
            let mut rng = Rng::new(3);
            (0..4_096)
                .map(|_| l.process(rng.next_bipolar()).to_bits())
                .collect::<Vec<u32>>()
        };
        assert_eq!(run(), run());
        let mut l = Ladder::new();
        l.set_params(500.0, 0.9, SR);
        l.process(1.0);
        l.reset();
        assert_eq!(l.process(0.0), 0.0);
    }

    #[test]
    fn silence_flushes_to_exact_zero() {
        let mut l = Ladder::new();
        l.set_params(200.0, 0.95, SR);
        l.process(1.0);
        for _ in 0..(SR as usize * 40) {
            l.process(0.0);
        }
        assert_eq!(l.process(0.0).to_bits(), 0);
    }
}
