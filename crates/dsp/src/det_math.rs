//! Deterministic scalar helpers for feedback networks: `2^x`, a denormal flush and the 8-point
//! Hadamard butterfly.
//!
//! Everything uses only IEEE `+ - *` in a fixed order (plus integer bit construction), so every
//! platform rounds identically: no libm call.

/// `2^x` for `x` clamped to `-126..=126`, from a degree-9 Taylor polynomial of `2^frac`
/// (relative error below 2e-9) and an exact power of two built from the exponent bits.
#[inline]
pub fn exp2_det(x: f32) -> f32 {
    if x.is_nan() {
        return 1.0;
    }
    exp2_det_f64(f64::from(x)) as f32
}

/// [`exp2_det`] in `f64` (same polynomial, so the relative error is below 2e-9); NaN gives 1.
/// Use it for filter coefficients that sit very close to 1 (long time constants).
#[inline]
pub fn exp2_det_f64(x: f64) -> f64 {
    if x.is_nan() {
        return 1.0;
    }
    let x = x.clamp(-126.0, 126.0);
    let n = x.floor();
    let f = x - n;
    // Horner in a fixed order; coefficients are ln(2)^k / k!.
    const C: [f64; 10] = [
        1.0,
        std::f64::consts::LN_2,
        0.240_226_506_959_100_7,
        0.055_504_108_664_821_58,
        0.009_618_129_107_628_477,
        0.001_333_355_814_642_844_3,
        0.000_154_035_303_933_816_1,
        0.000_015_252_733_804_059_84,
        0.000_001_321_548_679_014_03,
        0.000_000_101_780_860_092_4,
    ];
    let mut p = C[9];
    let mut k = 9;
    while k > 0 {
        k -= 1;
        p = p * f + C[k];
    }
    let scale = f64::from_bits(((n as i64 + 1023) as u64) << 52);
    p * scale
}

/// `log2(x)` for positive finite `x`, deterministic like [`exp2_det`]: the exponent comes from the
/// float bits and the mantissa (folded into `[1/sqrt2, sqrt2)`) from the atanh series
/// `ln m = 2(s + s^3/3 + s^5/5 + ...)`, `s = (m-1)/(m+1)`, with only IEEE `+ - * /`. The absolute
/// error is below 1e-12 in `f64` and the result is rounded to `f32`. Inputs that are zero,
/// negative, NaN or below the smallest normal `f32` return -126 (a floor, never `-inf`);
/// `+inf` returns 128.
#[inline]
pub fn log2_det(x: f32) -> f32 {
    if x.is_nan() || x < f32::MIN_POSITIVE {
        return -126.0;
    }
    if x.is_infinite() {
        return 128.0;
    }
    let bits = x.to_bits();
    let mut e = ((bits >> 23) & 0xff) as i32 - 127;
    let mut m = f64::from(f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000));
    if m > std::f64::consts::SQRT_2 {
        m *= 0.5;
        e += 1;
    }
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    // Horner over 1, 1/3, 1/5, ..., 1/17.
    let mut p = 1.0 / 17.0;
    let mut k = 15.0;
    while k > 0.0 {
        p = p * s2 + 1.0 / k;
        k -= 2.0;
    }
    let ln_m = 2.0 * s * p;
    (f64::from(e) + ln_m * std::f64::consts::LOG2_E) as f32
}

/// Flushes magnitudes below 1e-20 to exactly 0 (keeps feedback paths out of the denormal range).
#[inline]
pub fn flush_denormal(x: f32) -> f32 {
    if x.abs() < 1e-20 { 0.0 } else { x }
}

/// In-place orthonormal 8-point Hadamard transform (Sylvester order, scaled by `1/sqrt(8)`).
/// It is symmetric and its own inverse; 24 additions and 8 multiplications.
#[inline]
pub fn hadamard8(v: &mut [f32; 8]) {
    let mut len = 1;
    while len < 8 {
        let mut i = 0;
        while i < 8 {
            for j in i..i + len {
                let a = v[j];
                let b = v[j + len];
                v[j] = a + b;
                v[j + len] = a - b;
            }
            i += len * 2;
        }
        len *= 2;
    }
    for x in v.iter_mut() {
        *x *= 0.353_553_4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp2_det_matches_powf() {
        let mut x = -60.0f32;
        while x < 60.0 {
            let want = f64::from(x).exp2();
            let got = f64::from(exp2_det(x));
            assert!(((got - want) / want).abs() < 1e-6, "x={x}: {got} vs {want}");
            x += 0.137;
        }
        assert_eq!(exp2_det(0.0), 1.0);
        assert_eq!(exp2_det(3.0), 8.0);
        assert!(exp2_det(f32::NAN).is_finite());
        assert!(exp2_det(f32::NEG_INFINITY) >= 0.0 && exp2_det(f32::INFINITY).is_finite());
    }

    #[test]
    fn log2_det_matches_log2() {
        let mut x = 1e-30f32;
        while x < 1e30 {
            let want = f64::from(x).log2();
            let got = f64::from(log2_det(x));
            assert!(
                (got - want).abs() < 2e-6 * want.abs().max(1.0),
                "x={x}: {got} vs {want}"
            );
            x *= 1.37;
        }
        assert_eq!(log2_det(1.0), 0.0);
        assert_eq!(log2_det(8.0), 3.0);
        assert_eq!(log2_det(0.0), -126.0);
        assert_eq!(log2_det(-1.0), -126.0);
        assert_eq!(log2_det(f32::NAN), -126.0);
        assert_eq!(log2_det(f32::INFINITY), 128.0);
        assert!(log2_det(1e-40).is_finite());
    }

    #[test]
    fn flush_denormal_zeroes_tiny_values_only() {
        assert_eq!(flush_denormal(1e-25).to_bits(), 0.0f32.to_bits());
        assert_eq!(flush_denormal(-1e-25).to_bits(), 0.0f32.to_bits());
        assert_eq!(flush_denormal(1e-10), 1e-10);
        assert_eq!(flush_denormal(-0.5), -0.5);
    }

    #[test]
    fn hadamard8_preserves_energy_and_is_involutive() {
        let x = [0.3, -1.2, 0.7, 2.0, -0.4, 0.05, 1.1, -0.9];
        let mut y = x;
        hadamard8(&mut y);
        let e0: f32 = x.iter().map(|v| v * v).sum();
        let e1: f32 = y.iter().map(|v| v * v).sum();
        assert!((e0 - e1).abs() < 1e-5);
        hadamard8(&mut y);
        for (a, b) in x.iter().zip(&y) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}
