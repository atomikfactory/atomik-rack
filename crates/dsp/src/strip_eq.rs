//! The magnitude response of a console strip's filters and EQ, for drawing: the same sections
//! the strip runs (third-order Butterworth high- and low-pass, a high shelf or bell, two bells
//! and a low shelf or bell), summed in dB. Pure maths: a window that draws the curve and a test
//! that measures the strip with a sweep use the same function.

use crate::biquad::{BiquadParams, BiquadShape, magnitude_db};
use crate::pass_filter::{PassKind, pass_magnitude_db};

/// Q of an outer band that is a bell (the strip's fixed value).
pub const OUTER_BELL_Q: f64 = 0.7;
/// Q of an outer band that is a shelf (the steepest shelf without overshoot).
pub const SHELF_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// How much the gain widens the Q of a bell in E mode: the Q is multiplied by
/// `1 + |gain| / E_MODE_GAIN_DB`, so it doubles at that many dB of boost or cut.
pub const E_MODE_GAIN_DB: f64 = 18.0;

/// The Q a bell runs at in E mode: `q` made narrower as the gain grows (proportional Q), so a
/// small move stays broad and a large one gets focused. Flat (0 dB) it is `q` itself.
#[must_use]
pub fn e_mode_q(q: f64, gain_db: f64) -> f64 {
    q * (1.0 + gain_db.abs() / E_MODE_GAIN_DB)
}

/// One EQ band as the strip stores it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripBand {
    /// Gain in dB.
    pub gain_db: f64,
    /// Centre or corner frequency in Hz.
    pub freq: f64,
    /// Q of a middle band (ignored by the outer bands, whose Q is fixed).
    pub q: f64,
    /// An outer band is a bell instead of a shelf (ignored by the middle bands).
    pub bell: bool,
}

/// The filters and the four EQ bands of a strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripCurve {
    /// The sample rate the strip runs at.
    pub sample_rate: f64,
    /// The EQ section is on (off, the bands are bypassed).
    pub eq_on: bool,
    /// E mode: every bell's Q follows its gain ([`e_mode_q`]); off, the Q is the stored one.
    pub e_mode: bool,
    /// The high-pass corner in Hz when it is on.
    pub hpf: Option<f64>,
    /// The low-pass corner in Hz when it is on.
    pub lpf: Option<f64>,
    /// High frequency band (shelf or bell).
    pub hf: StripBand,
    /// High-mid bell.
    pub hmf: StripBand,
    /// Low-mid bell.
    pub lmf: StripBand,
    /// Low frequency band (shelf or bell).
    pub lf: StripBand,
}

impl StripCurve {
    /// The bands in the order HF, HMF, LMF, LF with their shapes and Q.
    fn sections(&self) -> [(BiquadShape, BiquadParams); 4] {
        let make = |b: &StripBand, shape: BiquadShape, q: f64| {
            let q = if self.e_mode && shape == BiquadShape::Peaking {
                e_mode_q(q, b.gain_db)
            } else {
                q
            };
            (
                shape,
                BiquadParams {
                    freq: b.freq,
                    q,
                    gain_db: b.gain_db,
                    sample_rate: self.sample_rate,
                },
            )
        };
        let outer = |b: &StripBand, shelf: BiquadShape| {
            if b.bell {
                make(b, BiquadShape::Peaking, OUTER_BELL_Q)
            } else {
                make(b, shelf, SHELF_Q)
            }
        };
        [
            outer(&self.hf, BiquadShape::HighShelf),
            make(&self.hmf, BiquadShape::Peaking, self.hmf.q),
            make(&self.lmf, BiquadShape::Peaking, self.lmf.q),
            outer(&self.lf, BiquadShape::LowShelf),
        ]
    }

    /// The response of the four EQ bands alone at `hz`, in dB (0 while the EQ is off).
    pub fn eq_db(&self, hz: f64) -> f64 {
        if !self.eq_on {
            return 0.0;
        }
        self.sections()
            .iter()
            .map(|(shape, p)| magnitude_db(*shape, hz, p))
            .sum()
    }

    /// The response of the two filters alone at `hz`, in dB.
    pub fn filters_db(&self, hz: f64) -> f64 {
        let hp = self.hpf.map_or(0.0, |f| {
            pass_magnitude_db(PassKind::Highpass, f, hz, self.sample_rate)
        });
        let lp = self.lpf.map_or(0.0, |f| {
            pass_magnitude_db(PassKind::Lowpass, f, hz, self.sample_rate)
        });
        hp + lp
    }

    /// The response of the whole chain, filters and EQ, at `hz`, in dB.
    pub fn response_db(&self, hz: f64) -> f64 {
        self.filters_db(hz) + self.eq_db(hz)
    }

    /// The response of one band alone at `hz`, in dB (band 0..4 in the order HF, HMF, LMF, LF).
    pub fn band_db(&self, band: usize, hz: f64) -> f64 {
        self.sections()
            .get(band)
            .map_or(0.0, |(shape, p)| magnitude_db(*shape, hz, p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn flat() -> StripCurve {
        let band = |freq| StripBand {
            gain_db: 0.0,
            freq,
            q: 1.0,
            bell: false,
        };
        StripCurve {
            sample_rate: SR,
            eq_on: true,
            e_mode: false,
            hpf: None,
            lpf: None,
            hf: band(8_000.0),
            hmf: band(2_500.0),
            lmf: band(500.0),
            lf: band(100.0),
        }
    }

    #[test]
    fn a_flat_strip_is_flat() {
        let c = flat();
        for hz in [20.0, 100.0, 1_000.0, 10_000.0, 20_000.0] {
            assert!(c.response_db(hz).abs() < 1e-9, "{hz}");
        }
    }

    #[test]
    fn a_bell_peaks_at_its_gain_and_the_eq_switch_removes_it() {
        let mut c = flat();
        c.hmf.gain_db = 9.0;
        assert!((c.response_db(2_500.0) - 9.0).abs() < 1e-6);
        c.eq_on = false;
        assert_eq!(c.response_db(2_500.0), 0.0);
    }

    #[test]
    fn e_mode_narrows_a_bell_with_its_gain_and_leaves_the_peak_and_the_shelves_alone() {
        let mut c = flat();
        c.hmf.gain_db = 12.0;
        let wide = c.band_db(1, 4_000.0);
        c.e_mode = true;
        // The peak is the gain either way; off the centre the response is lower (narrower).
        assert!((c.band_db(1, 2_500.0) - 12.0).abs() < 1e-6);
        assert!(c.band_db(1, 4_000.0) < wide - 0.5);
        // Flat, the Q is the stored one.
        c.hmf.gain_db = 0.0;
        assert!(c.band_db(1, 4_000.0).abs() < 1e-9);
        assert_eq!(e_mode_q(1.5, 0.0), 1.5);
        assert!((e_mode_q(1.0, -18.0) - 2.0).abs() < 1e-12);
        // A shelf is not a bell: E mode does not change it.
        let mut shelf = flat();
        shelf.hf.gain_db = 9.0;
        let before = shelf.band_db(0, 5_000.0);
        shelf.e_mode = true;
        assert_eq!(shelf.band_db(0, 5_000.0), before);
    }

    #[test]
    fn the_filters_are_minus_three_db_at_their_corners() {
        let mut c = flat();
        c.hpf = Some(80.0);
        c.lpf = Some(12_000.0);
        assert!((c.filters_db(80.0) + 3.0103).abs() < 0.01);
        assert!((c.filters_db(12_000.0) + 3.0103).abs() < 0.05);
        assert!(c.filters_db(1_000.0).abs() < 0.01);
    }

    #[test]
    fn a_shelf_gives_half_its_gain_at_the_corner_and_a_bell_option_changes_the_shape() {
        let mut c = flat();
        c.lf.gain_db = 12.0;
        assert!((c.band_db(3, 100.0) - 6.0).abs() < 0.05);
        let shelf = c.band_db(3, 20.0);
        c.lf.bell = true;
        assert!(
            c.band_db(3, 20.0) < shelf - 3.0,
            "a bell does not hold its gain below"
        );
        assert!((c.band_db(3, 100.0) - 12.0).abs() < 1e-6);
    }
}
