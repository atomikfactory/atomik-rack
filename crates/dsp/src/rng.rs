//! Deterministic pseudo-random generator (xorshift64*), seeded explicitly.

/// splitmix64 finaliser step (integer only, no libm).
#[inline]
const fn splitmix(z: u64) -> u64 {
    let mut z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Stateless hash of `(seed, a, b)`: `splitmix(splitmix(splitmix(seed) ^ a) ^ b)`.
///
/// Musical decisions (step probability, arpeggiator picks) are a pure function of the device
/// seed, a step number and a slot id, so a live run, a bounce, a locate and a loop pass of the
/// same step agree. Integer arithmetic only: identical on every platform.
#[inline]
pub const fn hash64(seed: u64, a: u64, b: u64) -> u64 {
    splitmix(splitmix(splitmix(seed) ^ a) ^ b)
}

/// Maps a hash to a uniform value in `[0, 1)` (the top 53 bits, exact in `f64`).
#[inline]
pub fn unit_f64(hash: u64) -> f64 {
    (hash >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
}

/// Maps a hash to an index in `0..n` (0 when `n` is 0), without modulo bias beyond 2^-32
/// (multiply-shift on the top 32 bits).
#[inline]
pub fn below(hash: u64, n: u32) -> u32 {
    (((hash >> 32) * u64::from(n)) >> 32) as u32
}

/// xorshift64* pseudo-random generator. Never seeded from time or the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Creates a generator from `seed`. Any seed (including 0) is valid.
    pub fn new(seed: u64) -> Self {
        // SplitMix64 scrambles the seed so nearby seeds give unrelated streams and the
        // xorshift state is never zero.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self {
            state: if z == 0 { 0x2545_F491_4F6C_DD1D } else { z },
        }
    }

    /// Next raw 64-bit value.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform value in `[0, 1)`.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        // Top 24 bits give every representable multiple of 2^-24 in [0, 1).
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }

    /// Uniform value in `[-1, 1)`.
    #[inline]
    pub fn next_bipolar(&mut self) -> f32 {
        2.0 * self.next_f32() - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED_000: u64 = 0x2382_75BC_38FC_BE91;

    #[test]
    fn hash64_is_a_pure_function_with_pinned_values() {
        assert_eq!(hash64(1, 2, 3), hash64(1, 2, 3));
        // Pinned: the stream must never change (live equals bounce across versions of a project).
        assert_eq!(hash64(0, 0, 0), PINNED_000);
        for (a, b) in [
            (hash64(0, 0, 0), hash64(1, 0, 0)),
            (hash64(0, 0, 0), hash64(0, 1, 0)),
            (hash64(0, 0, 0), hash64(0, 0, 1)),
            (hash64(0, 1, 0), hash64(0, 0, 1)),
        ] {
            assert_ne!(a, b);
        }
    }

    #[test]
    fn hash64_matches_the_splitmix64_reference() {
        // splitmix64 of the folded key, computed independently here.
        fn mix(mut z: u64) -> u64 {
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        for (seed, a, b) in [
            (0u64, 0u64, 0u64),
            (42, 7, 1),
            (u64::MAX, u64::MAX, u64::MAX),
        ] {
            assert_eq!(hash64(seed, a, b), mix(mix(mix(seed) ^ a) ^ b));
        }
    }

    #[test]
    fn unit_f64_is_in_range_and_below_is_bounded_and_uniform() {
        let mut sum = 0.0;
        let n = 20_000u64;
        let mut buckets = [0u32; 5];
        for k in 0..n {
            let h = hash64(9, k, 0);
            let u = unit_f64(h);
            assert!((0.0..1.0).contains(&u));
            sum += u;
            let i = below(h, 5);
            assert!(i < 5);
            buckets[i as usize] += 1;
        }
        assert!((sum / n as f64 - 0.5).abs() < 0.01);
        for c in buckets {
            assert!((c as f64 / n as f64 - 0.2).abs() < 0.02, "{buckets:?}");
        }
        assert_eq!(unit_f64(0), 0.0);
        assert!(unit_f64(u64::MAX) < 1.0);
        assert_eq!(below(u64::MAX, 0), 0);
        assert_eq!(below(u64::MAX, 1), 0);
    }

    #[test]
    fn rng_seeded_is_reproducible() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let mut c = Rng::new(43);
        let xs: Vec<u64> = (0..100).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..100).map(|_| b.next_u64()).collect();
        let zs: Vec<u64> = (0..100).map(|_| c.next_u64()).collect();
        assert_eq!(xs, ys);
        assert_ne!(xs, zs);
    }

    #[test]
    fn rng_zero_seed_is_not_stuck() {
        let mut r = Rng::new(0);
        let first = r.next_u64();
        assert!((0..10).any(|_| r.next_u64() != first));
    }

    #[test]
    fn rng_floats_are_in_range_and_roughly_uniform() {
        let mut r = Rng::new(7);
        let n = 100_000;
        let mut sum = 0.0f64;
        for _ in 0..n {
            let u = r.next_f32();
            assert!((0.0..1.0).contains(&u));
            let b = r.next_bipolar();
            assert!((-1.0..1.0).contains(&b));
            sum += f64::from(u);
        }
        let mean = sum / f64::from(n);
        assert!((mean - 0.5).abs() < 0.01, "mean {mean}");
    }
}
