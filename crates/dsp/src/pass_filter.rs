//! 18 dB/octave (third-order Butterworth) highpass and lowpass filters for the console strip.
//!
//! Design: a cascade of a first-order bilinear section and a second-order RBJ section with
//! Q = 1, both tuned to the corner frequency. With the same bilinear pre-warping in both, the
//! cascade is exactly the bilinear transform of the analog third-order Butterworth
//! `1/((s+1)(s²+s+1))`: maximally flat, −3.01 dB at the corner, 18 dB/octave asymptotic slope.
//! Frequency changes are smoothed like the EQ (see [`crate::SmoothedBiquad`]).

use crate::biquad::{BiquadShape, SmoothedBiquad};

/// Which side of the corner passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassKind {
    /// Removes lows.
    Highpass,
    /// Removes highs.
    Lowpass,
}

/// Slope of [`PassFilter`] in dB per octave.
pub const PASS_FILTER_SLOPE_DB_PER_OCT: f32 = 18.0;

/// A stereo third-order Butterworth HP or LP with a smoothed corner frequency.
#[derive(Debug, Clone, PartialEq)]
pub struct PassFilter {
    first: SmoothedBiquad,
    second: SmoothedBiquad,
}

/// Q of the second-order section of a third-order Butterworth.
const SECOND_ORDER_Q: f32 = 1.0;

impl PassFilter {
    /// A settled filter at `freq` Hz.
    pub fn new(kind: PassKind, freq: f32, sample_rate: f32) -> Self {
        let (s1, s2) = match kind {
            PassKind::Highpass => (BiquadShape::FirstOrderHighpass, BiquadShape::Highpass),
            PassKind::Lowpass => (BiquadShape::FirstOrderLowpass, BiquadShape::Lowpass),
        };
        Self {
            first: SmoothedBiquad::new(s1, freq, SECOND_ORDER_Q, 0.0, sample_rate),
            second: SmoothedBiquad::new(s2, freq, SECOND_ORDER_Q, 0.0, sample_rate),
        }
    }

    /// Changes the sample rate and snaps to the target.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.first.set_sample_rate(sample_rate);
        self.second.set_sample_rate(sample_rate);
    }

    /// Sets the target corner frequency (smoothed).
    pub fn set_freq(&mut self, freq: f32) {
        self.first.set_freq(freq);
        self.second.set_freq(freq);
    }

    /// Snaps to the target and clears the state.
    pub fn reset(&mut self) {
        self.first.reset();
        self.second.reset();
    }

    /// Snaps to the target frequency, keeping the state.
    pub fn snap(&mut self) {
        self.first.snap();
        self.second.snap();
    }

    /// Magnitude response in dB of the current coefficients.
    pub fn response_db(&self, freq: f32) -> f32 {
        self.first.response_db(freq) + self.second.response_db(freq)
    }

    /// Filters one stereo sample.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let (l, r) = self.first.process(l, r);
        self.second.process(l, r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::rms;

    const SR: f32 = 48_000.0;

    #[test]
    fn pass_filter_corner_is_minus_3db() {
        for kind in [PassKind::Highpass, PassKind::Lowpass] {
            let f = PassFilter::new(kind, 1_000.0, SR);
            assert!((f.response_db(1_000.0) + 3.0103).abs() < 1e-3, "{kind:?}");
        }
    }

    #[test]
    fn pass_filter_slope_is_18db_per_octave() {
        let hp = PassFilter::new(PassKind::Highpass, 1_000.0, SR);
        let s = hp.response_db(125.0) - hp.response_db(62.5);
        assert!((s - 18.0).abs() < 0.2, "HP slope {s}");
        assert!(hp.response_db(10_000.0).abs() < 0.01, "HP passband flat");
        let lp = PassFilter::new(PassKind::Lowpass, 500.0, SR);
        let s = lp.response_db(2_000.0) - lp.response_db(4_000.0);
        assert!((17.5..=19.5).contains(&s), "LP slope {s}");
        assert!(lp.response_db(20.0).abs() < 0.01, "LP passband flat");
    }

    #[test]
    fn pass_filter_measured_slope_matches_design() {
        let mut f = PassFilter::new(PassKind::Highpass, 400.0, SR);
        let gain = |f: &mut PassFilter, hz: f32| {
            f.reset();
            let y: Vec<f32> = (0..48_000)
                .map(|i| {
                    let x = (std::f32::consts::TAU * hz * i as f32 / SR).sin();
                    f.process(x, x).0
                })
                .collect();
            20.0 * (rms(&y[24_000..]) / std::f32::consts::FRAC_1_SQRT_2).log10()
        };
        let a = gain(&mut f, 50.0);
        let b = gain(&mut f, 100.0);
        assert!((b - a - 18.0).abs() < 0.3, "measured slope {}", b - a);
        assert!((a - f.response_db(50.0)).abs() < 0.1);
    }

    #[test]
    fn pass_filter_freq_change_is_smoothed() {
        let mut f = PassFilter::new(PassKind::Lowpass, 20_000.0, SR);
        f.set_freq(200.0);
        for _ in 0..20 {
            f.process(0.0, 0.0);
        }
        assert!(f.response_db(2_000.0) > -3.0, "still moving");
        for _ in 0..9_600 {
            f.process(0.0, 0.0);
        }
        assert!(f.response_db(2_000.0) < -50.0, "arrived");
    }
}
