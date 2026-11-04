//! The K-weighting filter of ITU-R BS.1770-4: its coefficients for a sample rate and its
//! magnitude response, shared by the loudness meters and the spectrum analyser's K-weighted view.
//!
//! Two cascaded biquads, designed for the actual sample rate with the bilinear method used to
//! derive the standard's 48 kHz coefficients (so at 48 kHz they reproduce the published
//! numbers):
//!
//! * **Pre-filter** (head-related high shelf): `f0 = 1681.9744509555319 Hz`,
//!   `G = 3.999843853973347 dB`, `Q = 0.7071752369554196`. With `K = tan(pi f0 / sr)`,
//!   `Vh = 10^(G/20)`, `Vb = Vh^0.4996667741545416`, `a0 = 1 + K/Q + K^2`:
//!   `b = [Vh + Vb K/Q + K^2, 2 (K^2 - Vh), Vh - Vb K/Q + K^2] / a0`,
//!   `a = [1, 2 (K^2 - 1) / a0, (1 - K/Q + K^2) / a0]`.
//! * **RLB high-pass**: `f0 = 38.13547087613982 Hz`, `Q = 0.5003270373253953`,
//!   `b = [1, -2, 1]` (not normalised, as in the standard), `a = [1, 2 (K^2 - 1) / a0,
//!   (1 - K/Q + K^2) / a0]`.
//!
//! [`k_weighting_db`] is the response of the two sections in dB, with the deterministic sine of
//! the stage blocks, so a table built from it is the same on every platform.

use crate::BiquadCoeffs;
use crate::det_trig::cos_cycles;

/// K-weighting coefficients `(pre-filter, RLB high-pass)` for `sr`.
pub fn k_weighting(sr: f64) -> (BiquadCoeffs, BiquadCoeffs) {
    let pi = std::f64::consts::PI;
    let (f0, g, q) = (
        1_681.974_450_955_532,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (pi * f0 / sr).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = BiquadCoeffs {
        b0: (vh + vb * k / q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    };
    let (f0, q) = (38.135_470_876_139_82, 0.500_327_037_325_395_3);
    let k = (pi * f0 / sr).tan();
    let a0 = 1.0 + k / q + k * k;
    let rlb = BiquadCoeffs {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    };
    (shelf, rlb)
}

/// The squared magnitude of one section at `cycles` (frequency over sample rate).
fn power_gain(c: &BiquadCoeffs, cycles: f64) -> f64 {
    let (c1, c2) = (cos_cycles(cycles), cos_cycles(2.0 * cycles));
    // sin(x) = cos(x - 1/4 cycle).
    let (s1, s2) = (cos_cycles(cycles - 0.25), cos_cycles(2.0 * cycles - 0.25));
    let num_re = c.b0 + c.b1 * c1 + c.b2 * c2;
    let num_im = c.b1 * s1 + c.b2 * s2;
    let den_re = 1.0 + c.a1 * c1 + c.a2 * c2;
    let den_im = c.a1 * s1 + c.a2 * s2;
    (num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im).max(1e-300)
}

/// The response of the K-weighting at `hz` for the sample rate `sr`, as a power factor (the
/// squared magnitude of both sections; 1.0 is 0 dB). About 1.58 (+2 dB) at 4 kHz and 0.01 at
/// 20 Hz.
pub fn k_weighting_power(hz: f64, sr: f64) -> f64 {
    if !(hz.is_finite() && sr > 0.0) {
        return 1.0;
    }
    let (shelf, rlb) = k_weighting(sr);
    let cycles = hz / sr;
    power_gain(&shelf, cycles) * power_gain(&rlb, cycles)
}

/// [`k_weighting_power`] in dB.
pub fn k_weighting_db(hz: f64, sr: f64) -> f64 {
    10.0 * k_weighting_power(hz, sr).max(1e-30).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_response_has_the_published_shape() {
        // BS.1770: close to 0 dB at 1 kHz, about +4 dB shelf at the top, roll-off below 100 Hz.
        assert!(k_weighting_db(1000.0, 48_000.0).abs() < 1.0);
        let top = k_weighting_db(10_000.0, 48_000.0);
        assert!((3.5..4.3).contains(&top), "{top}");
        assert!(k_weighting_db(20.0, 48_000.0) < -8.0);
        assert!(k_weighting_db(40.0, 48_000.0) < k_weighting_db(400.0, 48_000.0));
    }

    #[test]
    fn the_response_matches_a_direct_evaluation() {
        let (shelf, rlb) = k_weighting(44_100.0);
        for hz in [30.0, 100.0, 1500.0, 8000.0] {
            let w = std::f64::consts::TAU * hz / 44_100.0;
            let mag = |c: &BiquadCoeffs| {
                let n = (
                    c.b0 + c.b1 * w.cos() + c.b2 * (2.0 * w).cos(),
                    c.b1 * w.sin() + c.b2 * (2.0 * w).sin(),
                );
                let d = (
                    1.0 + c.a1 * w.cos() + c.a2 * (2.0 * w).cos(),
                    c.a1 * w.sin() + c.a2 * (2.0 * w).sin(),
                );
                (n.0 * n.0 + n.1 * n.1) / (d.0 * d.0 + d.1 * d.1)
            };
            let direct = 10.0 * (mag(&shelf) * mag(&rlb)).log10();
            assert!((k_weighting_db(hz, 44_100.0) - direct).abs() < 1e-6, "{hz}");
        }
    }
}
