//! Constant-power pan law.

/// Constant-power pan, −3 dB at centre: θ = (pan + 1)·π/4, returns `(cos θ, sin θ)`.
/// `pan` is clamped to −1..=1 (−1 = hard left).
#[inline]
pub fn pan_constant_power(pan: f32) -> (f32, f32) {
    let theta = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (theta.cos(), theta.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_constant_power_center_is_minus_3db() {
        let (l, r) = pan_constant_power(0.0);
        assert!((l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((r - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((20.0 * l.log10() + 3.0103).abs() < 1e-3);
    }

    #[test]
    fn pan_constant_power_extremes_and_power_sum() {
        let (l, r) = pan_constant_power(-1.0);
        assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
        let (l, r) = pan_constant_power(1.0);
        assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
        for i in -10..=10 {
            let (l, r) = pan_constant_power(i as f32 / 10.0);
            assert!((l * l + r * r - 1.0).abs() < 1e-5);
        }
        let (l, _) = pan_constant_power(-5.0);
        assert!((l - 1.0).abs() < 1e-6, "clamped");
    }
}

/// Mid/side stereo width: `M = (l + r)/2`, `S = (l − r)/2 · width`, returns `(M + S, M − S)`.
/// `width = 0` is mono (both sides = M), `1` leaves the signal unchanged; values above 1
/// widen. `width` is clamped to `0..=2`.
#[inline]
pub fn stereo_width(l: f32, r: f32, width: f32) -> (f32, f32) {
    let w = if width.is_nan() {
        1.0
    } else {
        width.clamp(0.0, 2.0)
    };
    let m = 0.5 * (l + r);
    let s = 0.5 * (l - r) * w;
    (m + s, m - s)
}

#[cfg(test)]
mod width_tests {
    use super::*;

    #[test]
    fn stereo_width_mono_unity_and_wide() {
        let (l, r) = stereo_width(1.0, 0.0, 0.0);
        assert!((l - 0.5).abs() < 1e-7 && (r - 0.5).abs() < 1e-7);
        let (l, r) = stereo_width(0.3, -0.7, 1.0);
        assert!((l - 0.3).abs() < 1e-7 && (r + 0.7).abs() < 1e-7);
        let (l, r) = stereo_width(1.0, 0.0, 2.0);
        assert!((l - 1.5).abs() < 1e-7 && (r + 0.5).abs() < 1e-7);
        let (l, r) = stereo_width(1.0, 0.0, 9.0);
        assert!((l - 1.5).abs() < 1e-7 && (r + 0.5).abs() < 1e-7, "clamped");
        // A mono signal is unaffected by width.
        let (l, r) = stereo_width(0.4, 0.4, 0.0);
        assert!((l - 0.4).abs() < 1e-7 && (r - 0.4).abs() < 1e-7);
    }
}
