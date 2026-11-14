//! The spectral envelope of a frame and the gain that moves it: the formant part of the tonal
//! engine.
//!
//! A frame's envelope is found by **cepstral liftering**: the log magnitude spectrum is turned
//! into a cepstrum (a transform of the log spectrum), only the low quefrencies are kept (what
//! varies slowly along the frequency axis, the formants; the fine comb of the harmonics lives at
//! the pitch period and is dropped by the lifter) and the result is turned back into a smooth
//! log spectrum. Pitch shifting by stretch and resample moves that envelope by the pitch ratio
//! `r`; a formant shift of `s` wants it at `s` instead. So before the resample, bin `j` is
//! multiplied by `E(j * r / s) / E(j)`, which makes the envelope after the resample
//! `E(f / s)`: the pitch has moved, the formants have not (or moved by `s`).
//!
//! Only IEEE `+ - * /` and the deterministic `log2`/`exp2` of this crate are used.

use super::fft::Fft;
use crate::{exp2_det_f64, log2_det};

/// Largest gain either way, as a power of two (a factor of 16, 24 dB).
const MAX_GAIN_LOG2: f64 = 4.0;

/// The lifter: how much of the cepstrum is kept, and the transform that gets there.
pub(crate) struct Envelope {
    n: usize,
    half: usize,
    /// Quefrencies (in samples) up to which the cepstrum is kept in full; it tapers to 0 at
    /// twice that.
    keep: usize,
}

impl Envelope {
    /// The lifter of a frame of `n` samples at `sample_rate`. It keeps what changes over about
    /// 400 Hz of frequency or more, so a voice above that pitch (and any chord) keeps a smooth
    /// envelope between its harmonics.
    pub(crate) fn new(n: usize, sample_rate: u32) -> Self {
        let keep = (sample_rate as usize / 800).clamp(4, n / 8);
        Self {
            n,
            half: n / 2,
            keep,
        }
    }

    /// The smoothed log2 magnitude of `mag` (bins `0..=n/2`) into `out`. `zr` and `zi` are
    /// scratch of `n` samples each.
    pub(crate) fn log_envelope(
        &self,
        fft: &Fft,
        mag: &[f64],
        zr: &mut [f64],
        zi: &mut [f64],
        out: &mut [f64],
    ) {
        let (n, half) = (self.n, self.half);
        zi.fill(0.0);
        for j in 0..=half {
            let m = (mag[j] as f32).max(1.0e-9);
            let l = f64::from(log2_det(m));
            zr[j] = l;
            if j > 0 && j < half {
                zr[n - j] = l;
            }
        }
        fft.run(zr, zi, false);
        let taper = (2 * self.keep) as f64;
        for q in 0..n {
            let d = q.min(n - q);
            let w = if d <= self.keep {
                1.0
            } else if d >= 2 * self.keep {
                0.0
            } else {
                // Linear taper: cheap and free of ripple beyond the lifter.
                (taper - d as f64) / self.keep as f64
            };
            zr[q] *= w;
            zi[q] *= w;
        }
        fft.run(zr, zi, false);
        let s = 1.0 / n as f64;
        for j in 0..=half {
            out[j] = zr[j] * s;
        }
    }
}

/// The per-bin gain that moves an envelope: `2^(L(j * scale) - L(j))` for the log2 envelope `l`
/// (bins `0..=n/2`), read with linear interpolation and held at the last bin beyond it, limited
/// to a factor of 16 either way. `scale` 1 gives gain 1.
pub(crate) fn gains(l: &[f64], scale: f64, out: &mut [f64]) {
    let last = l.len() - 1;
    for (j, g) in out.iter_mut().enumerate().take(l.len()) {
        let at = j as f64 * scale;
        let v = if at >= last as f64 {
            l[last]
        } else {
            let i = at as usize;
            let f = at - i as f64;
            l[i] + (l[i + 1] - l[i]) * f
        };
        *g = exp2_det_f64((v - l[j]).clamp(-MAX_GAIN_LOG2, MAX_GAIN_LOG2));
    }
}
