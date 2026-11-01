//! RBJ-cookbook biquads (Robert Bristow-Johnson, "Cookbook formulae for audio EQ biquad filter
//! coefficients", public domain) plus a first-order bilinear section, and a parameter-smoothed
//! stereo wrapper for console EQ bands.
//!
//! Design notes:
//! * **Coefficients and state are `f64`.** Low-frequency shelves at high sample rates put the
//!   poles very close to `z = 1`; `f64` keeps the response exact to well under 0.01 dB.
//! * **Direct form I.** The state is the last two inputs and outputs, so it stays meaningful
//!   when the coefficients change between samples (no internal-state rescaling artefacts).
//! * **Frequency clamp:** `10 Hz ..= 0.49·sr`. **Q clamp:** `0.1 ..= 40`.
//! * **Smoothing ([`SmoothedBiquad`]):** frequency (in log₂ Hz), Q (in ln Q) and gain (in dB)
//!   each follow a one-pole smoother with a [`EQ_SMOOTH_MS`] time constant. While any of them
//!   is moving, coefficients are recomputed every [`COEFF_UPDATE_INTERVAL`] samples; once all
//!   have snapped to their targets no recomputation happens. The per-update parameter step is
//!   tiny (a 16-sample slice of a 10 ms exponential), so no zipper noise is audible. A change
//!   of *shape* (bell ↔ shelf) is a discrete switch and takes effect at once.
//! * **Denormals:** output state values below 1e-30 are flushed to zero.

use crate::OnePoleSmoother;

/// Time constant of the EQ parameter smoothers.
pub const EQ_SMOOTH_MS: f32 = 10.0;
/// Samples between coefficient recomputations while parameters are moving.
pub const COEFF_UPDATE_INTERVAL: u32 = 16;

/// Lowest supported centre/corner frequency in Hz.
const MIN_FREQ: f64 = 10.0;
/// Highest supported frequency as a fraction of the sample rate.
const MAX_FREQ_RATIO: f64 = 0.49;
/// Lowest Q.
const MIN_Q: f64 = 0.1;
/// Highest Q.
const MAX_Q: f64 = 40.0;

/// The response shape of a biquad section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiquadShape {
    /// Peaking (bell) EQ: `gain_db` at `freq`, bandwidth set by `q`.
    Peaking,
    /// Low shelf: `gain_db` below `freq`, `gain_db/2` at `freq`; `q` sets the transition
    /// (Q = 1/√2 is the steepest shelf without overshoot, cookbook slope S = 1).
    LowShelf,
    /// High shelf (mirror of [`BiquadShape::LowShelf`]).
    HighShelf,
    /// Second-order lowpass, resonance `q` (gain ignored).
    Lowpass,
    /// Second-order highpass, resonance `q` (gain ignored).
    Highpass,
    /// First-order lowpass (bilinear, −3.01 dB at `freq`; `q` and gain ignored).
    FirstOrderLowpass,
    /// First-order highpass (bilinear, −3.01 dB at `freq`; `q` and gain ignored).
    FirstOrderHighpass,
}

/// Normalised biquad coefficients (`a0 = 1`):
/// `y = b0·x + b1·x₁ + b2·x₂ − a1·y₁ − a2·y₂`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadCoeffs {
    /// Feed-forward 0.
    pub b0: f64,
    /// Feed-forward 1.
    pub b1: f64,
    /// Feed-forward 2.
    pub b2: f64,
    /// Feedback 1.
    pub a1: f64,
    /// Feedback 2.
    pub a2: f64,
}

impl BiquadCoeffs {
    /// The pass-through section.
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// Designs a section per the RBJ cookbook (see the module docs for the clamps).
    pub fn design(shape: BiquadShape, freq: f64, q: f64, gain_db: f64, sample_rate: f64) -> Self {
        use std::f64::consts::PI;
        let hi = MAX_FREQ_RATIO * sample_rate;
        let f = if freq.is_nan() {
            hi
        } else {
            freq.clamp(MIN_FREQ, hi)
        };
        let q = if q.is_nan() {
            std::f64::consts::FRAC_1_SQRT_2
        } else {
            q.clamp(MIN_Q, MAX_Q)
        };
        let gain_db = if gain_db.is_finite() { gain_db } else { 0.0 };
        let w0 = 2.0 * PI * f / sample_rate;
        let (sn, cs) = w0.sin_cos();
        let alpha = sn / (2.0 * q);
        let a = 10f64.powf(gain_db / 40.0);
        // (b0, b1, b2, a0, a1, a2) before normalisation.
        let (b0, b1, b2, a0, a1, a2) = match shape {
            BiquadShape::Peaking => (
                1.0 + alpha * a,
                -2.0 * cs,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cs,
                1.0 - alpha / a,
            ),
            BiquadShape::LowShelf => {
                let k = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cs + k),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                    a * ((a + 1.0) - (a - 1.0) * cs - k),
                    (a + 1.0) + (a - 1.0) * cs + k,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                    (a + 1.0) + (a - 1.0) * cs - k,
                )
            }
            BiquadShape::HighShelf => {
                let k = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cs + k),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                    a * ((a + 1.0) + (a - 1.0) * cs - k),
                    (a + 1.0) - (a - 1.0) * cs + k,
                    2.0 * ((a - 1.0) - (a + 1.0) * cs),
                    (a + 1.0) - (a - 1.0) * cs - k,
                )
            }
            BiquadShape::Lowpass => (
                (1.0 - cs) / 2.0,
                1.0 - cs,
                (1.0 - cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            BiquadShape::Highpass => (
                (1.0 + cs) / 2.0,
                -(1.0 + cs),
                (1.0 + cs) / 2.0,
                1.0 + alpha,
                -2.0 * cs,
                1.0 - alpha,
            ),
            BiquadShape::FirstOrderLowpass => {
                let k = (w0 / 2.0).tan();
                (k, k, 0.0, 1.0 + k, k - 1.0, 0.0)
            }
            BiquadShape::FirstOrderHighpass => {
                let k = (w0 / 2.0).tan();
                (1.0, -1.0, 0.0, 1.0 + k, k - 1.0, 0.0)
            }
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Magnitude response in dB at `freq` Hz (evaluates `H(e^{jω})` exactly).
    pub fn response_db(&self, freq: f64, sample_rate: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / sample_rate;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        // e^{-jω} = c1 − j·s1; e^{-2jω} = c2 − j·s2.
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()
    }
}

/// The settings of one biquad section, for [`magnitude_db`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiquadParams {
    /// Centre or corner frequency in Hz.
    pub freq: f64,
    /// Q (the section's resonance or bandwidth).
    pub q: f64,
    /// Gain in dB (ignored by the pass shapes).
    pub gain_db: f64,
    /// The sample rate the section runs at.
    pub sample_rate: f64,
}

/// The magnitude response in dB at `hz` of a section of `shape` with `params`: the
/// response of the coefficients [`BiquadCoeffs::design`] makes, so a drawn curve is exactly what
/// the DSP does (pure, allocation-free).
pub fn magnitude_db(shape: BiquadShape, hz: f64, params: &BiquadParams) -> f64 {
    BiquadCoeffs::design(
        shape,
        params.freq,
        params.q,
        params.gain_db,
        params.sample_rate,
    )
    .response_db(hz, params.sample_rate)
}

/// Flushes values in the denormal range to zero.
#[inline]
fn flush(v: f64) -> f64 {
    if v.abs() < 1e-30 { 0.0 } else { v }
}

/// One biquad channel in direct form I (`f64` state).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Biquad {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    /// Clears the state.
    #[inline]
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Filters one sample with `c`.
    #[inline]
    pub fn process(&mut self, c: &BiquadCoeffs, x: f64) -> f64 {
        let y = c.b0 * x + c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = flush(x);
        self.y2 = self.y1;
        self.y1 = flush(y);
        y
    }
}

/// A stereo biquad with smoothed frequency, Q and gain (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct SmoothedBiquad {
    shape: BiquadShape,
    sample_rate: f32,
    /// Targets: log₂ Hz, ln Q, dB.
    target: [f32; 3],
    /// Smoothed values, same domains.
    current: [OnePoleSmoother; 3],
    coeffs: BiquadCoeffs,
    countdown: u32,
    settled: bool,
    l: Biquad,
    r: Biquad,
}

impl SmoothedBiquad {
    /// A section of `shape` with the given parameters, already settled, at `sample_rate`.
    pub fn new(shape: BiquadShape, freq: f32, q: f32, gain_db: f32, sample_rate: f32) -> Self {
        let target = [log2_hz(freq), ln_q(q), clean_db(gain_db)];
        let sm = |v: f32| OnePoleSmoother::new(v, EQ_SMOOTH_MS, sample_rate);
        let mut s = Self {
            shape,
            sample_rate,
            target,
            current: [sm(target[0]), sm(target[1]), sm(target[2])],
            coeffs: BiquadCoeffs::IDENTITY,
            countdown: 0,
            settled: true,
            l: Biquad::default(),
            r: Biquad::default(),
        };
        s.reset();
        s
    }

    /// Changes the sample rate (no allocation); snaps to the targets and clears the state.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        for c in &mut self.current {
            c.set_time(EQ_SMOOTH_MS, sample_rate);
        }
        self.reset();
    }

    /// Sets the shape (immediate).
    pub fn set_shape(&mut self, shape: BiquadShape) {
        if shape != self.shape {
            self.shape = shape;
            self.update_coeffs();
        }
    }

    fn set_target(&mut self, i: usize, v: f32) {
        if v.to_bits() != self.target[i].to_bits() {
            self.target[i] = v;
            self.settled = false;
        }
    }

    /// Sets the target frequency in Hz (smoothed).
    pub fn set_freq(&mut self, freq: f32) {
        self.set_target(0, log2_hz(freq));
    }

    /// Sets the target Q (smoothed).
    pub fn set_q(&mut self, q: f32) {
        self.set_target(1, ln_q(q));
    }

    /// Sets the target gain in dB (smoothed).
    pub fn set_gain_db(&mut self, gain_db: f32) {
        self.set_target(2, clean_db(gain_db));
    }

    /// Jumps every smoothed parameter to its target and clears the filter state.
    pub fn reset(&mut self) {
        self.snap();
        self.l.reset();
        self.r.reset();
    }

    /// Jumps every smoothed parameter to its target, keeping the filter state.
    pub fn snap(&mut self) {
        for (c, t) in self.current.iter_mut().zip(self.target) {
            c.set_immediate(t);
        }
        self.settled = true;
        self.countdown = 0;
        self.update_coeffs();
    }

    /// True when every parameter has reached its target.
    pub fn is_settled(&self) -> bool {
        self.settled
    }

    /// The coefficients currently in use.
    pub fn coeffs(&self) -> &BiquadCoeffs {
        &self.coeffs
    }

    /// Magnitude response in dB of the current coefficients.
    pub fn response_db(&self, freq: f32) -> f32 {
        self.coeffs
            .response_db(f64::from(freq), f64::from(self.sample_rate)) as f32
    }

    fn update_coeffs(&mut self) {
        let [f, q, g] = [
            self.current[0].value(),
            self.current[1].value(),
            self.current[2].value(),
        ];
        self.coeffs = BiquadCoeffs::design(
            self.shape,
            f64::from(f).exp2(),
            f64::from(q).exp(),
            f64::from(g),
            f64::from(self.sample_rate),
        );
    }

    /// Advances the parameter smoothers by one sample, recomputing the coefficients every
    /// [`COEFF_UPDATE_INTERVAL`] samples while they move (and once more when they arrive).
    #[inline]
    fn tick(&mut self) {
        if self.settled {
            return;
        }
        let mut done = true;
        for (c, t) in self.current.iter_mut().zip(self.target) {
            done &= c.next_value(t).to_bits() == t.to_bits();
        }
        if self.countdown == 0 || done {
            self.update_coeffs();
            self.countdown = COEFF_UPDATE_INTERVAL;
        }
        self.countdown -= 1;
        self.settled = done;
    }

    /// Filters one stereo sample.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        self.tick();
        let c = &self.coeffs;
        (
            self.l.process(c, f64::from(l)) as f32,
            self.r.process(c, f64::from(r)) as f32,
        )
    }
}

#[inline]
fn log2_hz(freq: f32) -> f32 {
    if freq.is_nan() {
        MIN_FREQ as f32
    } else {
        freq.clamp(MIN_FREQ as f32, 1e6).log2()
    }
}

#[inline]
fn ln_q(q: f32) -> f32 {
    if q.is_nan() {
        std::f32::consts::FRAC_1_SQRT_2.ln()
    } else {
        q.clamp(MIN_Q as f32, MAX_Q as f32).ln()
    }
}

/// Gains are kept finite and within ±60 dB.
#[inline]
fn clean_db(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(-60.0, 60.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    /// Analog prototype magnitude in dB, evaluated at the bilinear-warped frequency
    /// `Ω = tan(πf/sr)/tan(πf0/sr)`. Independent of the cookbook coefficient formulas.
    fn analog_db(shape: BiquadShape, f0: f64, q: f64, gain_db: f64, f: f64) -> f64 {
        use std::f64::consts::PI;
        let w = (PI * f / SR).tan() / (PI * f0 / SR).tan();
        let a = 10f64.powf(gain_db / 40.0);
        // |N(jw)|² / |D(jw)|² for N = n2 s² + n1 s + n0, D = d2 s² + d1 s + d0.
        let mag2 = |n2: f64, n1: f64, n0: f64, d2: f64, d1: f64, d0: f64| {
            let nr = n0 - n2 * w * w;
            let ni = n1 * w;
            let dr = d0 - d2 * w * w;
            let di = d1 * w;
            (nr * nr + ni * ni) / (dr * dr + di * di)
        };
        let m2 = match shape {
            BiquadShape::Peaking => mag2(1.0, a / q, 1.0, 1.0, 1.0 / (a * q), 1.0),
            BiquadShape::LowShelf => {
                let s = a.sqrt() / q;
                a * a * mag2(1.0, s, a, a, s, 1.0)
            }
            BiquadShape::HighShelf => {
                let s = a.sqrt() / q;
                a * a * mag2(a, s, 1.0, 1.0, s, a)
            }
            BiquadShape::Lowpass => mag2(0.0, 0.0, 1.0, 1.0, 1.0 / q, 1.0),
            BiquadShape::Highpass => mag2(1.0, 0.0, 0.0, 1.0, 1.0 / q, 1.0),
            BiquadShape::FirstOrderLowpass => mag2(0.0, 0.0, 1.0, 0.0, 1.0, 1.0),
            BiquadShape::FirstOrderHighpass => mag2(0.0, 1.0, 0.0, 0.0, 1.0, 1.0),
        };
        10.0 * m2.log10()
    }

    fn test_freqs() -> Vec<f64> {
        (0..20)
            .map(|i| 20.0 * 1000f64.powf(i as f64 / 19.0))
            .collect()
    }

    #[test]
    fn biquad_response_matches_analog_prototype_for_every_shape() {
        let shapes = [
            (BiquadShape::Peaking, 1_000.0, 1.4, 9.0),
            (BiquadShape::Peaking, 120.0, 0.5, -12.0),
            (
                BiquadShape::LowShelf,
                200.0,
                std::f64::consts::FRAC_1_SQRT_2,
                6.0,
            ),
            (
                BiquadShape::HighShelf,
                6_000.0,
                std::f64::consts::FRAC_1_SQRT_2,
                -10.0,
            ),
            (BiquadShape::Lowpass, 5_000.0, 1.0, 0.0),
            (BiquadShape::Highpass, 80.0, 0.7, 0.0),
            (BiquadShape::FirstOrderLowpass, 3_000.0, 1.0, 0.0),
            (BiquadShape::FirstOrderHighpass, 50.0, 1.0, 0.0),
        ];
        for (shape, f0, q, g) in shapes {
            let c = BiquadCoeffs::design(shape, f0, q, g, SR);
            for f in test_freqs() {
                let got = c.response_db(f, SR);
                let want = analog_db(shape, f0, q, g, f);
                assert!(
                    (got - want).abs() < 0.01,
                    "{shape:?} f0={f0} at {f:.1} Hz: {got:.4} vs {want:.4}"
                );
            }
        }
    }

    #[test]
    fn biquad_reference_points() {
        // Bell: exactly the gain at f0, 0 dB far away.
        let c = BiquadCoeffs::design(BiquadShape::Peaking, 1_000.0, 2.0, 12.0, SR);
        assert!((c.response_db(1_000.0, SR) - 12.0).abs() < 1e-6);
        assert!(c.response_db(10.0, SR).abs() < 0.01);
        // Shelves: half the gain at f0, the full gain at the far end.
        let c = BiquadCoeffs::design(
            BiquadShape::LowShelf,
            300.0,
            std::f64::consts::FRAC_1_SQRT_2,
            -8.0,
            SR,
        );
        assert!((c.response_db(300.0, SR) + 4.0).abs() < 1e-6);
        assert!((c.response_db(1.0, SR) + 8.0).abs() < 0.01);
        let c = BiquadCoeffs::design(
            BiquadShape::HighShelf,
            4_000.0,
            std::f64::consts::FRAC_1_SQRT_2,
            6.0,
            SR,
        );
        assert!((c.response_db(4_000.0, SR) - 3.0).abs() < 1e-6);
        assert!((c.response_db(23_999.0, SR) - 6.0).abs() < 0.01);
        // Butterworth LP/HP: −3.01 dB at the corner.
        for shape in [
            BiquadShape::Lowpass,
            BiquadShape::Highpass,
            BiquadShape::FirstOrderLowpass,
            BiquadShape::FirstOrderHighpass,
        ] {
            let c = BiquadCoeffs::design(shape, 2_000.0, std::f64::consts::FRAC_1_SQRT_2, 0.0, SR);
            assert!(
                (c.response_db(2_000.0, SR) + 3.0103).abs() < 1e-3,
                "{shape:?}"
            );
        }
        // Zero gain bell/shelf is the identity.
        for shape in [
            BiquadShape::Peaking,
            BiquadShape::LowShelf,
            BiquadShape::HighShelf,
        ] {
            let c = BiquadCoeffs::design(shape, 900.0, 1.0, 0.0, SR);
            assert!((c.b0 - 1.0).abs() < 1e-12 && (c.b1 - c.a1).abs() < 1e-12);
            assert!((c.b2 - c.a2).abs() < 1e-12);
        }
    }

    #[test]
    fn biquad_design_clamps_frequency_and_q() {
        let c = BiquadCoeffs::design(BiquadShape::Lowpass, 1e9, 1e9, 0.0, SR);
        let d = BiquadCoeffs::design(BiquadShape::Lowpass, 0.49 * SR, 40.0, 0.0, SR);
        assert_eq!(c, d);
        let c = BiquadCoeffs::design(BiquadShape::Highpass, -5.0, f64::NAN, 0.0, SR);
        assert!([c.b0, c.b1, c.b2, c.a1, c.a2].iter().all(|v| v.is_finite()));
    }

    /// Steady-state measured gain of `f` (stereo, both channels) for a sine at `hz`.
    fn measured_db(f: &mut SmoothedBiquad, hz: f64) -> (f64, f64) {
        let n = 48_000usize;
        let (mut sl, mut cl, mut sr, mut cr) = (0.0, 0.0, 0.0, 0.0);
        for i in 0..2 * n {
            let ph = std::f64::consts::TAU * hz * i as f64 / SR;
            let x = ph.sin() as f32;
            let (l, r) = f.process(x, 0.5 * x);
            if i >= n {
                sl += f64::from(l) * ph.sin();
                cl += f64::from(l) * ph.cos();
                sr += f64::from(r) * ph.sin();
                cr += f64::from(r) * ph.cos();
            }
        }
        let amp = |s: f64, c: f64| 2.0 * (s * s + c * c).sqrt() / n as f64;
        (
            20.0 * amp(sl, cl).log10(),
            20.0 * (amp(sr, cr) / 0.5).log10(),
        )
    }

    #[test]
    fn smoothed_biquad_measured_response_matches_design() {
        let mut f = SmoothedBiquad::new(BiquadShape::Peaking, 2_500.0, 1.2, 7.5, SR as f32);
        for hz in [100.0, 1_000.0, 2_500.0, 5_000.0, 15_000.0] {
            let want = f.response_db(hz as f32);
            let (l, r) = measured_db(&mut f, hz);
            assert!((l - f64::from(want)).abs() < 0.05, "{hz}: L {l} vs {want}");
            assert!((r - f64::from(want)).abs() < 0.05, "{hz}: R {r} vs {want}");
        }
    }

    #[test]
    fn smoothed_biquad_moves_gradually_then_settles() {
        let mut f = SmoothedBiquad::new(BiquadShape::Peaking, 1_000.0, 1.0, 0.0, SR as f32);
        assert!(f.is_settled());
        assert!(f.response_db(1_000.0).abs() < 1e-4);
        f.set_gain_db(12.0);
        assert!(!f.is_settled());
        // After one update interval the gain has moved only a little.
        for _ in 0..COEFF_UPDATE_INTERVAL + 1 {
            f.process(0.0, 0.0);
        }
        let early = f.response_db(1_000.0);
        assert!(early > 0.0 && early < 1.0, "small first step: {early}");
        // After 20 time constants it has arrived (snapped) and stops recomputing.
        for _ in 0..9_600 {
            f.process(0.0, 0.0);
        }
        assert!(f.is_settled());
        assert!((f.response_db(1_000.0) - 12.0).abs() < 1e-3);
        // snap() jumps to the target without clearing the state; reset() also clears it.
        f.set_freq(2_000.0);
        f.snap();
        assert!(f.is_settled());
        assert!((f.response_db(2_000.0) - 12.0).abs() < 1e-3);
        f.set_freq(4_000.0);
        f.reset();
        assert!(f.is_settled());
        assert!((f.response_db(4_000.0) - 12.0).abs() < 1e-3);
    }

    #[test]
    fn smoothed_biquad_sweep_has_no_zipper_steps() {
        // A sine through a bell whose gain jumps by 18 dB: the output envelope moves
        // continuously (largest sample step stays near the sine's own slope).
        let mut f = SmoothedBiquad::new(BiquadShape::Peaking, 1_000.0, 1.0, 0.0, SR as f32);
        let mut prev = 0.0f32;
        let mut max_step = 0.0f32;
        for i in 0..9_600 {
            if i == 2_400 {
                f.set_gain_db(18.0);
            }
            let x = (std::f64::consts::TAU * 100.0 * i as f64 / SR).sin() as f32 * 0.1;
            let (y, _) = f.process(x, x);
            if i > 0 {
                max_step = max_step.max((y - prev).abs());
            }
            prev = y;
        }
        // 100 Hz sine, amplitude ≤ ~0.13 after the bell: slope ≤ 2π·100·0.13/48000 ≈ 0.0017.
        assert!(max_step < 0.003, "max step {max_step}");
    }

    #[test]
    fn biquad_reset_clears_state_and_flushes_denormals() {
        let c = BiquadCoeffs::design(BiquadShape::Lowpass, 100.0, 0.7, 0.0, SR);
        let mut b = Biquad::default();
        b.process(&c, 1.0);
        b.reset();
        assert_eq!(b, Biquad::default());
        b.process(&c, 1e-35);
        for _ in 0..100 {
            b.process(&c, 0.0);
        }
        assert_eq!(b, Biquad::default(), "tiny values flush to zero");
    }
}
