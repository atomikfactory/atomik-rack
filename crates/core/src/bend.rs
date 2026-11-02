//! The bent automation segment: a power curve whose exponent comes from a small integer.
//!
//! A segment from value `p` to value `q` with bend `n` runs `p + (q - p) * shape(x, n)` for the
//! position `x` in `[0, 1]` along it. `shape` is `x^k` with `k = 2^(n / 32)`, so `n = 0` is the
//! straight line (`k = 1`), a positive bend eases in (`k` up to 4 at `n = 64`: slow start, fast
//! end) and a negative bend eases out (`k` down to 1/4).
//!
//! The evaluation is the same on every platform: no libm call, no table, only IEEE additions,
//! multiplications and divisions (which round the same everywhere) and bit manipulation, so a
//! bounce is bit-identical across block sizes, worker counts and machines. It never allocates
//! or locks, which makes it fit for the audio thread. It agrees with `x.powf(k)` to about 1e-15.

/// The largest bend magnitude: the exponent is `2^(n / 32)` for `n` in `-BEND_MAX..=BEND_MAX`.
pub const BEND_MAX: i8 = 64;

/// How many steps of bend double the exponent.
const STEPS_PER_OCTAVE: f64 = 32.0;

const LN2: f64 = std::f64::consts::LN_2;

/// `bend` limited to `-BEND_MAX..=BEND_MAX`.
pub fn clamp_bend(bend: i8) -> i8 {
    bend.clamp(-BEND_MAX, BEND_MAX)
}

/// The exponent of bend `bend` (`2^(bend / 32)`, from the clamped bend).
pub fn bend_exponent(bend: i8) -> f64 {
    exp2(f64::from(clamp_bend(bend)) / STEPS_PER_OCTAVE)
}

/// The shape of a bend: `x^k` for `x` in `[0, 1]` (outside it the end values hold), `k` from
/// [`bend_exponent`]. Exact 0 at 0 and 1 at 1; a zero bend returns `x` unchanged.
pub fn bend_shape(x: f64, bend: i8) -> f64 {
    if x.is_nan() || x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    if bend == 0 {
        return x;
    }
    exp2(bend_exponent(bend) * log2(x))
}

/// `log2(x)` for a positive finite `x`: the exponent from the bits plus an `atanh` series of
/// the mantissa.
fn log2(x: f64) -> f64 {
    let bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7ff) as i64;
    let mut m_bits = bits & 0x000f_ffff_ffff_ffff;
    if e == 0 {
        // Subnormal: scale into the normal range first.
        let scaled = x * f64::from_bits(0x4330_0000_0000_0000); // 2^52
        let b = scaled.to_bits();
        e = ((b >> 52) & 0x7ff) as i64 - 52;
        m_bits = b & 0x000f_ffff_ffff_ffff;
    }
    let mut m = f64::from_bits(m_bits | 0x3ff0_0000_0000_0000); // [1, 2)
    let mut e = (e - 1023) as f64;
    if m > std::f64::consts::SQRT_2 {
        m *= 0.5;
        e += 1.0;
    }
    // ln(m) = 2 atanh(s), s = (m - 1) / (m + 1), |s| <= 0.1716.
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let mut sum = 0.0;
    let mut k = 21;
    while k >= 1 {
        sum = sum * s2 + 1.0 / f64::from(k);
        k -= 2;
    }
    e + 2.0 * s * sum / LN2
}

/// `2^y` for `y` in about `-1000..=1000`: the integer part is built in the exponent bits, the
/// fraction is a Taylor series of `exp`.
fn exp2(y: f64) -> f64 {
    if y < -1000.0 {
        return 0.0;
    }
    let n = (y + 0.5).floor();
    let f = (y - n) * LN2; // |f| <= ln2 / 2
    let mut sum = 1.0;
    let mut k = 17;
    while k >= 1 {
        sum = 1.0 + sum * f / f64::from(k);
        k -= 1;
    }
    let scale = f64::from_bits(((n as i64 + 1023) as u64) << 52);
    sum * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_matches_the_power_function() {
        for bend in -BEND_MAX..=BEND_MAX {
            let k = (f64::from(bend) / 32.0).exp2();
            for i in 0..=400 {
                let x = f64::from(i) / 400.0;
                let got = bend_shape(x, bend);
                assert!(
                    (got - x.powf(k)).abs() < 1e-12,
                    "bend {bend} at {x}: {got} vs {}",
                    x.powf(k)
                );
            }
        }
    }

    #[test]
    fn the_ends_are_exact_and_zero_is_linear() {
        for bend in -BEND_MAX..=BEND_MAX {
            assert_eq!(bend_shape(0.0, bend), 0.0);
            assert_eq!(bend_shape(1.0, bend), 1.0);
        }
        assert_eq!(bend_shape(0.3, 0), 0.3);
        assert_eq!(bend_shape(-1.0, 10), 0.0);
        assert_eq!(bend_shape(2.0, 10), 1.0);
        assert_eq!(bend_shape(f64::NAN, 10), 0.0);
    }

    #[test]
    fn a_positive_bend_eases_in_and_a_negative_one_eases_out() {
        assert!(bend_shape(0.5, 32) < 0.5);
        assert!(bend_shape(0.5, -32) > 0.5);
        assert!((bend_shape(0.5, 32) - 0.25).abs() < 1e-12); // k = 2
        assert!((bend_exponent(64) - 4.0).abs() < 1e-12);
        assert!((bend_exponent(-64) - 0.25).abs() < 1e-12);
        assert_eq!(bend_exponent(100), bend_exponent(64));
    }

    #[test]
    fn it_is_monotonic_and_tiny_positions_work() {
        let mut last = 0.0;
        for i in 0..=1000 {
            let v = bend_shape(f64::from(i) / 1000.0, 40);
            assert!(v >= last);
            last = v;
        }
        assert!(bend_shape(1e-300, 64) >= 0.0);
        assert!(bend_shape(5e-324, -64) > 0.0);
    }
}
