//! Mono bit crusher: a floor quantiser followed by a sample-and-hold with a fractional rate.
//!
//! * **Quantiser:** `y = clamp(floor(x q) / q, -1, 1)` with `q = 2^(bits - 1)`: for `x` in `[-1, 1)` exactly
//!   `2^bits` output levels. No dither. Bypassed (bit exact) at `bits >= 24`, the fine end.
//! * **Sample-and-hold:** a free-running ramp advances by `rate / fs` per sample and the held value
//!   is refreshed from the quantised input each time it wraps (the first sample is taken
//!   immediately), so the hold rate is exact on average and the hold length alternates between
//!   `floor` and `ceil` of `fs / rate`. Bypassed (bit exact) at `rate >= fs`.
//!
//! There is no anti-alias filter on purpose: both stages alias (the images are the sound). Use
//! one instance per channel for independent crush. Uses `+ - * /`, `floor` and
//! [`crate::exp2_det`]: bit-identical on every platform.

use crate::exp2_det;

/// Bit depth at and above which the quantiser is bypassed.
pub const BYPASS_BITS: f32 = 24.0;

/// Mono bit crusher.
#[derive(Debug, Clone)]
pub struct Bitcrush {
    sample_rate: f64,
    bits: f32,
    steps: f32,
    inc: f64,
    phase: f64,
    held: f32,
    hold_bypass: bool,
}

impl Bitcrush {
    /// A crusher with both stages bypassed.
    pub fn new(sample_rate: f32) -> Self {
        let mut b = Self {
            sample_rate: f64::from(sample_rate),
            bits: BYPASS_BITS,
            steps: 1.0,
            inc: 1.0,
            phase: 0.0,
            held: 0.0,
            hold_bypass: true,
        };
        b.set_bits(BYPASS_BITS);
        b.set_rate_hz(sample_rate);
        b
    }

    /// Bit depth (clamped to `1..=24`; fractional depths are allowed; NaN ignored).
    pub fn set_bits(&mut self, bits: f32) {
        if bits.is_nan() {
            return;
        }
        self.bits = bits.clamp(1.0, BYPASS_BITS);
        self.steps = exp2_det(self.bits - 1.0);
    }

    /// Hold rate in Hz (clamped to `1..=fs`; `fs` bypasses the hold; NaN ignored).
    pub fn set_rate_hz(&mut self, hz: f32) {
        if hz.is_nan() {
            return;
        }
        let f = f64::from(hz).clamp(1.0, self.sample_rate);
        self.hold_bypass = f >= self.sample_rate;
        self.inc = f / self.sample_rate;
    }

    /// True when the quantiser is bypassed.
    pub fn quantiser_bypassed(&self) -> bool {
        self.bits >= BYPASS_BITS
    }

    /// True when the sample-and-hold is bypassed.
    pub fn hold_bypassed(&self) -> bool {
        self.hold_bypass
    }

    /// Restarts the hold ramp so the next sample is taken, and clears the held value.
    pub fn reset(&mut self) {
        self.phase = 1.0 - self.inc;
        self.held = 0.0;
    }

    /// Crushes one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let q = if self.quantiser_bypassed() {
            x
        } else {
            ((x * self.steps).floor() / self.steps).clamp(-1.0, 1.0)
        };
        if self.hold_bypass {
            return q;
        }
        self.phase += self.inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            self.held = q;
        }
        self.held
    }

    /// Crushes a block in place (identical to per-sample calls).
    pub fn process_block(&mut self, buf: &mut [f32]) {
        for s in buf {
            *s = self.process(*s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{assert_chunk_invariant_mono, noise};

    const SR: f32 = 48_000.0;

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| -1.0 + 2.0 * i as f32 / n as f32).collect()
    }

    fn distinct(x: &[f32]) -> usize {
        let mut v: Vec<u32> = x.iter().map(|s| s.to_bits()).collect();
        v.sort_unstable();
        v.dedup();
        v.len()
    }

    #[test]
    fn bit_depth_is_exact() {
        for bits in [1.0f32, 2.0, 3.0, 4.0, 8.0, 12.0] {
            let mut c = Bitcrush::new(SR);
            c.set_bits(bits);
            let mut x = ramp(1 << 17);
            c.process_block(&mut x);
            assert_eq!(distinct(&x), 1usize << bits as u32, "{bits} bits");
            let q = (bits - 1.0).exp2();
            assert!(
                x.iter()
                    .all(|v| (v * q).fract() == 0.0 && *v >= -1.0 && *v < 1.0)
            );
        }
    }

    #[test]
    fn quantiser_is_a_floor_quantiser() {
        let mut c = Bitcrush::new(SR);
        c.set_bits(3.0); // steps of 1/4
        assert_eq!(c.process(0.3), 0.25);
        assert_eq!(c.process(-0.01), -0.25);
        assert_eq!(c.process(0.0), 0.0);
        assert_eq!(c.process(0.999), 0.75);
    }

    #[test]
    fn fine_end_is_bit_exact_bypass() {
        let mut c = Bitcrush::new(SR);
        assert!(c.quantiser_bypassed() && c.hold_bypassed());
        let x = noise(1, 5_000, 1.0);
        let mut y = x.clone();
        c.process_block(&mut y);
        assert!(x.iter().zip(&y).all(|(a, b)| a.to_bits() == b.to_bits()));
        c.set_bits(23.9);
        assert!(!c.quantiser_bypassed());
        c.set_bits(100.0);
        assert!(c.quantiser_bypassed());
        c.set_rate_hz(SR - 1.0);
        assert!(!c.hold_bypassed());
        c.set_rate_hz(1.0e9);
        assert!(c.hold_bypassed());
    }

    #[test]
    fn hold_rate_is_exact() {
        // 6 kHz at 48 kHz: every 8th sample is taken, the first immediately.
        let mut c = Bitcrush::new(SR);
        c.set_rate_hz(6000.0);
        c.reset();
        let mut x: Vec<f32> = (0..800).map(|i| i as f32).collect();
        c.process_block(&mut x);
        for (i, v) in x.iter().enumerate() {
            assert_eq!(*v, (i / 8 * 8) as f32, "sample {i}");
        }
        // 7 kHz: 7000 holds in one second, run lengths 6 or 7.
        let mut c = Bitcrush::new(SR);
        c.set_rate_hz(7000.0);
        c.reset();
        let mut x: Vec<f32> = (0..48_000).map(|i| i as f32).collect();
        c.process_block(&mut x);
        let updates = 1 + x
            .windows(2)
            .filter(|w| w[1].to_bits() != w[0].to_bits())
            .count();
        assert!(updates.abs_diff(7000) <= 1, "{updates}");
        let mut runs = Vec::new();
        let mut len = 1;
        for w in x.windows(2) {
            if w[1].to_bits() == w[0].to_bits() {
                len += 1;
            } else {
                runs.push(len);
                len = 1;
            }
        }
        assert!(runs.iter().all(|r| *r == 6 || *r == 7), "{:?}", &runs[..10]);
    }

    #[test]
    fn both_stages_combine_and_nan_is_ignored() {
        let mut c = Bitcrush::new(SR);
        c.set_bits(f32::NAN);
        c.set_rate_hz(f32::NAN);
        assert!(c.quantiser_bypassed() && c.hold_bypassed());
        c.set_bits(2.0);
        c.set_rate_hz(4800.0);
        c.reset();
        let mut x = ramp(4_800);
        c.process_block(&mut x);
        assert!(distinct(&x) <= 4);
        let runs = x
            .windows(2)
            .filter(|w| w[0].to_bits() != w[1].to_bits())
            .count();
        assert!(runs <= 4, "{runs}");
    }

    #[test]
    fn process_is_chunk_invariant() {
        let x = noise(2, 3_000, 0.9);
        assert_chunk_invariant_mono(
            || {
                let mut c = Bitcrush::new(SR);
                c.set_bits(5.5);
                c.set_rate_hz(3333.0);
                c.reset();
                c
            },
            |c, b| c.process_block(b),
            &x,
        );
    }

    proptest::proptest! {
        #[test]
        fn finite_and_bounded_for_any_parameters(
            bits in 0.0f32..30.0, rate in 0.0f32..60_000.0, seed in 0u64..300
        ) {
            let mut c = Bitcrush::new(SR);
            c.set_bits(bits);
            c.set_rate_hz(rate);
            let mut x = noise(seed, 1024, 1.0);
            c.process_block(&mut x);
            proptest::prop_assert!(x.iter().all(|v| v.is_finite() && *v >= -1.0 && *v <= 1.0));
        }
    }
}
