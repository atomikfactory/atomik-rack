//! Requantising float audio to integer PCM, with optional dither (the last step of an audio
//! export).
//!
//! A [`Dither`] turns a float sample (full scale is `±1.0`) into the signed integer code of a
//! `bits`-bit PCM word (`-2^(bits-1) ..= 2^(bits-1) - 1`, so `+1.0` is the largest code and
//! `-1.0` the smallest). Rounding is half to even, and anything beyond full scale clips; NaN is 0.
//!
//! * [`DitherMode::None`]: plain rounding. The error is correlated with the signal, so a quiet
//!   fade turns into harmonic distortion.
//! * [`DitherMode::Tpdf`]: triangular-PDF dither of `±1` LSB (the sum of two uniform draws of
//!   `±½` LSB) is added before the rounding. The error then has a constant power, independent of
//!   the signal and uncorrelated with it: the signal's means are preserved to well below one LSB
//!   (Lipshitz, Wannamaker and Vanderkooy, "Quantization and dither: a theory", JAES 1992).
//! * [`DitherMode::Shaped`]: the same dither with a first-order error feedback, so the noise
//!   power is pushed from the low and middle frequencies, where the ear is most sensitive, to the
//!   top of the band (noise transfer function `1 - z^-1`: 0 at DC, +6 dB at Nyquist).
//!
//! Everything is seeded: the draws come from [`crate::Rng`] (one stream per channel, derived
//! from the seed), so the same input and seed give the same bits on every run and platform. The
//! words depend on the sample index only, not on how the audio is chunked.

use crate::Rng;

/// What is added before the rounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DitherMode {
    /// Nothing: plain rounding.
    None,
    /// Triangular-PDF dither, `±1` LSB.
    Tpdf,
    /// Triangular-PDF dither with first-order noise shaping.
    Shaped,
}

/// One channel's state.
#[derive(Debug, Clone)]
struct Channel {
    rng: Rng,
    /// The last quantisation error, in LSB (noise shaping).
    error: f64,
}

/// A seeded requantiser for per-channel audio. See the module docs.
#[derive(Debug, Clone)]
pub struct Dither {
    mode: DitherMode,
    channels: Vec<Channel>,
}

/// Full scale of a `bits`-bit word: `2^(bits-1)`.
fn full_scale(bits: u32) -> f64 {
    f64::from(bits - 1).exp2()
}

impl Dither {
    /// A requantiser with `channels` independent streams derived from `seed`.
    pub fn new(mode: DitherMode, seed: u64, channels: usize) -> Self {
        let channels = (0..channels.max(1))
            .map(|c| Channel {
                // Distinct streams per channel (`Rng::new` scrambles the seed).
                rng: Rng::new(seed ^ (c as u64 + 1).wrapping_mul(0xA24B_AED4_963E_E407)),
                error: 0.0,
            })
            .collect();
        Self { mode, channels }
    }

    /// The mode this requantiser was made with.
    pub fn mode(&self) -> DitherMode {
        self.mode
    }

    /// The `bits`-bit code of `x` on `channel` (8 to 32 bits; the channel wraps if it is out of
    /// range). Call it once per sample, in time order, per channel.
    pub fn quantise(&mut self, channel: usize, x: f32, bits: u32) -> i32 {
        let scale = full_scale(bits.clamp(8, 32));
        if x.is_nan() {
            return 0;
        }
        let (lo, hi) = (-scale, scale - 1.0);
        let n = self.channels.len();
        let state = &mut self.channels[channel % n];
        let wanted = f64::from(x) * scale;
        let fed = if self.mode == DitherMode::Shaped {
            wanted - state.error
        } else {
            wanted
        };
        let value = match self.mode {
            DitherMode::None => fed,
            DitherMode::Tpdf | DitherMode::Shaped => {
                // Two uniform draws of ±½ LSB: a triangular PDF of ±1 LSB.
                let noise = 0.5 * (state.rng.next_bipolar() + state.rng.next_bipolar());
                fed + f64::from(noise)
            }
        };
        let code = value.round_ties_even().clamp(lo, hi);
        if self.mode == DitherMode::Shaped {
            // The error against the fed-back sample, dither included, so the whole error is
            // shaped. A clipped sample would feed a huge error back: forget it instead.
            state.error = if value < lo || value > hi {
                0.0
            } else {
                code - fed
            };
        }
        code as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BITS: [u32; 3] = [16, 24, 32];

    #[test]
    fn plain_rounding_is_exact_on_every_code_and_clips_beyond_full_scale() {
        for bits in BITS {
            let scale = full_scale(bits);
            let mut d = Dither::new(DitherMode::None, 1, 1);
            assert_eq!(d.quantise(0, 0.0, bits), 0);
            assert_eq!(d.quantise(0, 1.0, bits), (scale - 1.0) as i32);
            assert_eq!(d.quantise(0, -1.0, bits), (-scale) as i32);
            assert_eq!(d.quantise(0, 2.5, bits), (scale - 1.0) as i32);
            assert_eq!(d.quantise(0, -7.0, bits), (-scale) as i32);
            assert_eq!(d.quantise(0, f32::NAN, bits), 0);
            // A value that is a code is that code (exact in f32 for the 16 and 24 bit words).
            if bits < 32 {
                for k in [-12_345_i32, -1, 1, 77, 32_000] {
                    assert_eq!(d.quantise(0, (f64::from(k) / scale) as f32, bits), k);
                }
            }
        }
    }

    #[test]
    fn plain_rounding_goes_half_to_even() {
        let mut d = Dither::new(DitherMode::None, 1, 1);
        // Half an LSB of 16 bits: ties go to the even code.
        let lsb = 1.0 / 32_768.0;
        assert_eq!(d.quantise(0, 0.5 * lsb, 16), 0);
        assert_eq!(d.quantise(0, 1.5 * lsb, 16), 2);
        assert_eq!(d.quantise(0, 2.5 * lsb, 16), 2);
        assert_eq!(d.quantise(0, -0.5 * lsb, 16), 0);
    }

    #[test]
    fn tpdf_noise_is_triangular_with_zero_mean() {
        // Dither a value exactly on a code: the output is that code plus the dither rounded.
        let n = 200_000;
        let mut d = Dither::new(DitherMode::Tpdf, 7, 1);
        let codes: Vec<i32> = (0..n).map(|_| d.quantise(0, 0.0, 16)).collect();
        assert!(codes.iter().all(|c| (-1..=1).contains(c)), "within one LSB");
        let mean = codes.iter().map(|&c| f64::from(c)).sum::<f64>() / f64::from(n);
        assert!(mean.abs() < 0.01, "mean {mean}");
        // Rounding a triangular ±1 draw: P(0) = 3/4, P(±1) = 1/8 each, so the power is 1/4.
        let zeros = codes.iter().filter(|&&c| c == 0).count() as f64 / f64::from(n);
        assert!((zeros - 0.75).abs() < 0.01, "P(0) = {zeros}");
        let power = codes.iter().map(|&c| f64::from(c * c)).sum::<f64>() / f64::from(n);
        assert!((power - 0.25).abs() < 0.01, "power {power}");
    }

    #[test]
    fn tpdf_keeps_a_signal_below_one_lsb_unbiased_where_rounding_loses_it() {
        // A constant at 0.3 LSB: rounding gives 0 forever, dither averages to 0.3.
        let value = (0.3 / 32_768.0) as f32;
        let n = 200_000;
        let mut plain = Dither::new(DitherMode::None, 3, 1);
        let mut dith = Dither::new(DitherMode::Tpdf, 3, 1);
        let mean = |d: &mut Dither| {
            (0..n)
                .map(|_| f64::from(d.quantise(0, value, 16)))
                .sum::<f64>()
                / f64::from(n)
        };
        assert_eq!(mean(&mut plain), 0.0);
        let m = mean(&mut dith);
        assert!((m - 0.3).abs() < 0.01, "mean {m}");
    }

    #[test]
    fn dither_is_deterministic_per_seed_and_the_channels_differ() {
        let run = |seed| {
            let mut d = Dither::new(DitherMode::Tpdf, seed, 2);
            (0..2_000)
                .map(|i| d.quantise(i % 2, 0.0, 16))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(5), run(5));
        assert_ne!(run(5), run(6));
        let mut d = Dither::new(DitherMode::Tpdf, 5, 2);
        let left: Vec<i32> = (0..500).map(|_| d.quantise(0, 0.0, 16)).collect();
        let right: Vec<i32> = (0..500).map(|_| d.quantise(1, 0.0, 16)).collect();
        assert_ne!(left, right);
    }

    #[test]
    fn shaped_dither_moves_the_noise_to_high_frequencies() {
        // The error spectrum of 1 - z^-1: the lag-1 autocorrelation of the error is negative
        // (about -0.5), where plain TPDF is white (about 0).
        let n = 100_000;
        let value = (0.37 / 32_768.0) as f32;
        let lag1 = |mode| {
            let mut d = Dither::new(mode, 11, 1);
            let e: Vec<f64> = (0..n)
                .map(|_| f64::from(d.quantise(0, value, 16)) - 0.37)
                .collect();
            let mean = e.iter().sum::<f64>() / f64::from(n);
            let var = e.iter().map(|x| (x - mean).powi(2)).sum::<f64>();
            let cov = e
                .windows(2)
                .map(|w| (w[0] - mean) * (w[1] - mean))
                .sum::<f64>();
            cov / var
        };
        assert!(lag1(DitherMode::Tpdf).abs() < 0.02);
        assert!(lag1(DitherMode::Shaped) < -0.3);
    }

    #[test]
    fn shaped_dither_clips_without_a_runaway_error() {
        let mut d = Dither::new(DitherMode::Shaped, 1, 1);
        for _ in 0..100 {
            assert_eq!(d.quantise(0, 3.0, 16), 32_767);
        }
        // Back in range right away: the code is within a few LSB of the value.
        let c = d.quantise(0, 0.5, 16);
        assert!((c - 16_384).abs() <= 3, "{c}");
    }
}
