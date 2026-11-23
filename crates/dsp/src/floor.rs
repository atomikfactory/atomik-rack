//! The whole part of a delay without a library call where the target has no rounding
//! instruction.
//!
//! A release build for baseline x86-64 has no SSE4.1, so `f32::floor` is a call into the C
//! runtime there, made for every read of a delay line; on `aarch64`, or with `sse4.1` enabled,
//! `floor` is one instruction and the expression is used as it is. [`split_floor`] gives the very
//! same bits as the expression for every input: it answers a delay of `1.0` up to 2^23 samples
//! with integer arithmetic whose result is shown below to be the expression's, and sends every
//! other value (anything below, a NaN, an infinity, a value that is a whole number already) to
//! the expression.

/// `(d.floor(), d.floor() as usize)` for every `d` (the pair a fractional read needs); a delay
/// in samples (`1.0` up to 2^23) is answered with integer arithmetic, anything else by the
/// expression.
#[inline]
pub(crate) fn split_floor(d: f32) -> (f32, usize) {
    #[cfg(any(target_arch = "aarch64", target_feature = "sse4.1"))]
    {
        let whole = d.floor();
        (whole, whole as usize)
    }
    #[cfg(not(any(target_arch = "aarch64", target_feature = "sse4.1")))]
    {
        split_by_bits(d)
    }
}

/// The form [`split_floor`] runs where `f32::floor` is a call (compiled everywhere, so that the
/// tests reach it on every platform).
#[inline]
#[cfg_attr(
    any(target_arch = "aarch64", target_feature = "sse4.1"),
    allow(
        dead_code,
        reason = "only the tests run it where `floor` is an instruction"
    )
)]
fn split_by_bits(d: f32) -> (f32, usize) {
    // The exponent field is 127 + e for a value in `2^e..2^(e + 1)`; `e` in `0..23` is exactly
    // `d` in `1..2^23` (a sign, a NaN, an infinity and anything smaller or larger wrap to 23 or
    // more). Of the 24 bits of the significand the top `e + 1` are the whole part, so a shift
    // drops the fraction: the integer is the floor, its `f32` is exact, and no comparison
    // between floats (whose result, for a fraction at random, a branch would guess badly) is
    // needed.
    let bits = d.to_bits();
    let e = (bits >> 23).wrapping_sub(127);
    if e < 23 {
        let whole = ((bits & 0x007F_FFFF) | 0x0080_0000) >> (23 - e);
        (whole as f32, whole as usize)
    } else {
        let whole = d.floor();
        (whole, whole as usize)
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;

    /// splitmix64: a seeded stream of raw bit patterns.
    struct Bits(u64);

    impl Bits {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Uniform in `0..1`.
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    fn split_matches(d: f32) {
        let whole = d.floor();
        let want = (whole, whole as usize);
        for got in [split_by_bits(d), split_floor(d)] {
            let alike = got.0.to_bits() == want.0.to_bits() || want.0.is_nan() && got.0.is_nan();
            assert!(alike, "whole of {d:e} ({:#x})", d.to_bits());
            assert_eq!(got.1, want.1, "index of {d:e} ({:#x})", d.to_bits());
        }
    }

    #[test]
    fn the_split_of_the_named_delays_is_the_library_pair() {
        let mut v: Vec<f32> = vec![
            0.0,
            -0.0,
            0.5,
            1.0,
            1.5,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MAX,
            f32::MIN,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
            -1.0,
            -1.5,
        ];
        // Every power of two, one and a half times it, and the neighbours of each in float steps.
        for e in -126..=127 {
            let p = 2f32.powi(e);
            for x in [p, p * 1.5, p * 1.25] {
                for b in [
                    x.to_bits().wrapping_sub(1),
                    x.to_bits(),
                    x.to_bits().wrapping_add(1),
                ] {
                    v.push(f32::from_bits(b));
                    v.push(-f32::from_bits(b));
                }
            }
        }
        for n in 0..70u32 {
            v.extend([n as f32, n as f32 + 0.5, n as f32 - 0.5]);
        }
        for d in v {
            split_matches(d);
        }
    }

    #[test]
    fn the_split_of_two_hundred_thousand_delays_is_the_library_pair() {
        let mut g = Bits(0xDE1A);
        for _ in 0..200_000 {
            split_matches(f32::from_bits(g.next() as u32));
            // The range a delay line reads: whole numbers and fractions up to a few seconds.
            split_matches(1.0 + g.unit() as f32 * 400_000.0);
            // And around 2^23 and 2^24, where every float is a whole number.
            split_matches(8_388_000.0 + g.unit() as f32 * 2_000.0);
            split_matches(16_777_000.0 + g.unit() as f32 * 2_000.0);
        }
    }

    /// Every `f32` bit pattern (about 4.3 billion, a minute or two in a release build):
    /// `cargo test -p atomik-dsp --lib --release split_of_every -- --ignored`.
    #[test]
    #[ignore = "exhaustive, minutes"]
    fn the_split_of_every_f32_is_the_library_pair() {
        for bits in 0..=u32::MAX {
            split_matches(f32::from_bits(bits));
        }
    }
}
