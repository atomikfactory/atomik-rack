//! The short-time Fourier transform the network was trained with: a periodic window, frames
//! centred on `hop * i` with the signal reflected at both ends, and the overlap-add inverse that
//! divides by the summed squared window.

use std::f64::consts::PI;

/// A radix-2 complex FFT with precomputed twiddles.
#[derive(Debug, Clone)]
pub(crate) struct Fft {
    n: usize,
    rev: Vec<u32>,
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Fft {
    pub(crate) fn new(n: usize) -> Self {
        debug_assert!(n.is_power_of_two() && n >= 2);
        let bits = n.trailing_zeros();
        let rev = (0..n as u32)
            .map(|i| i.reverse_bits() >> (32 - bits))
            .collect();
        let cos = (0..n / 2)
            .map(|k| (2.0 * PI * k as f64 / n as f64).cos() as f32)
            .collect();
        let sin = (0..n / 2)
            .map(|k| (2.0 * PI * k as f64 / n as f64).sin() as f32)
            .collect();
        Self { n, rev, cos, sin }
    }

    /// In-place forward transform `X[k] = sum x[n] e^{-2 pi i k n / N}`.
    pub(crate) fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = n / len;
            let mut i = 0;
            while i < n {
                for j in 0..half {
                    let (wr, wi) = (self.cos[j * step], -self.sin[j * step]);
                    let (a, b) = (i + j, i + j + half);
                    let tr = wr * re[b] - wi * im[b];
                    let ti = wr * im[b] + wi * re[b];
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
                i += len;
            }
            len *= 2;
        }
    }

    /// In-place inverse transform including the `1/N` scale (the swap trick).
    pub(crate) fn inverse(&self, re: &mut [f32], im: &mut [f32]) {
        self.forward(im, re);
        let s = 1.0 / self.n as f32;
        for v in re.iter_mut().chain(im.iter_mut()) {
            *v *= s;
        }
    }
}

/// The analysis and synthesis of a stereo signal.
#[derive(Debug, Clone)]
pub struct Stft {
    n_fft: usize,
    hop: usize,
    window: Vec<f32>,
    fft: Fft,
}

/// A stereo spectrogram: for each frame, the left bins then the right bins, as separate real and
/// imaginary arrays of `frames * 2 * bins`.
#[derive(Debug, Clone, Default)]
pub struct Spectrogram {
    /// How many frames.
    pub frames: usize,
    /// The real parts.
    pub re: Vec<f32>,
    /// The imaginary parts.
    pub im: Vec<f32>,
}

impl Stft {
    /// A transform of `n_fft` points (a power of two) with the given window (`n_fft` values) and
    /// a hop of a quarter window.
    pub fn new(n_fft: usize, window: Vec<f32>) -> Self {
        debug_assert_eq!(window.len(), n_fft);
        Self {
            n_fft,
            hop: n_fft / 4,
            window,
            fft: Fft::new(n_fft),
        }
    }

    /// The periodic Hann window of `n` points.
    pub fn hann(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()) as f32)
            .collect()
    }

    /// The bins of one frame (`n_fft / 2 + 1`).
    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// The hop between frames.
    pub fn hop(&self) -> usize {
        self.hop
    }

    /// The frames a signal of `len` samples gives.
    pub fn frames_for(&self, len: usize) -> usize {
        len / self.hop + 1
    }

    /// The reflected sample of a padded signal.
    fn at(x: &[f32], i: isize) -> f32 {
        let n = x.len() as isize;
        if n == 0 {
            return 0.0;
        }
        if n == 1 {
            return x[0];
        }
        let period = 2 * (n - 1);
        let mut j = i.rem_euclid(period);
        if j >= n {
            j = period - j;
        }
        x[j as usize]
    }

    /// The spectrogram of a stereo signal.
    pub fn analyse(&self, left: &[f32], right: &[f32]) -> Spectrogram {
        let len = left.len().min(right.len());
        let (left, right) = (&left[..len], &right[..len]);
        let frames = self.frames_for(len);
        let bins = self.bins();
        let n = self.n_fft;
        let mut s = Spectrogram {
            frames,
            re: vec![0.0; frames * 2 * bins],
            im: vec![0.0; frames * 2 * bins],
        };
        let mut re = vec![0f32; n];
        let mut im = vec![0f32; n];
        for f in 0..frames {
            let start = (f * self.hop) as isize - (n / 2) as isize;
            for i in 0..n {
                let w = self.window[i];
                re[i] = Self::at(left, start + i as isize) * w;
                im[i] = Self::at(right, start + i as isize) * w;
            }
            self.fft.forward(&mut re, &mut im);
            // Two real signals in one complex transform: split them by symmetry.
            let base = f * 2 * bins;
            for k in 0..bins {
                let m = (n - k) % n;
                let (zr, zi, cr, ci) = (re[k], im[k], re[m], -im[m]);
                // L = (Z + conj Z[-k]) / 2, R = (Z - conj Z[-k]) / 2i
                s.re[base + k] = 0.5 * (zr + cr);
                s.im[base + k] = 0.5 * (zi + ci);
                s.re[base + bins + k] = 0.5 * (zi - ci);
                s.im[base + bins + k] = -0.5 * (zr - cr);
            }
        }
        s
    }

    /// Adds the signal of one frame (given as its left and right half spectra) into `out_l` and
    /// `out_r` at the frame's position, and the squared window into `norm`. `first` is the
    /// signal index of the frame's first sample (negative before the signal starts).
    fn add_frame(
        &self,
        left: (&[f32], &[f32]),
        right: (&[f32], &[f32]),
        first: isize,
        out: &mut [Vec<f32>; 2],
        scratch: (&mut [f32], &mut [f32]),
    ) {
        let n = self.n_fft;
        let bins = self.bins();
        let (re, im) = scratch;
        // Z = L + iR where both are Hermitian: build the full spectra.
        for k in 0..bins {
            let (lr, li, rr, ri) = (left.0[k], left.1[k], right.0[k], right.1[k]);
            re[k] = lr - ri;
            im[k] = li + rr;
            if k != 0 && k != n / 2 {
                let m = n - k;
                re[m] = lr + ri;
                im[m] = -li + rr;
            }
        }
        self.fft.inverse(re, im);
        for i in 0..n {
            let pos = first + i as isize;
            if pos >= 0 && (pos as usize) < out[0].len() {
                let w = self.window[i];
                out[0][pos as usize] += re[i] * w;
                out[1][pos as usize] += im[i] * w;
            }
        }
    }

    /// The signal of a spectrogram, `len` samples per channel. The result is divided by the
    /// summed squared window, so analysis then synthesis returns the signal.
    pub fn synthesise(&self, s: &Spectrogram, len: usize) -> [Vec<f32>; 2] {
        let n = self.n_fft;
        let bins = self.bins();
        let mut out = [vec![0f32; len], vec![0f32; len]];
        let mut norm = vec![0f32; len];
        let mut re = vec![0f32; n];
        let mut im = vec![0f32; n];
        for f in 0..s.frames {
            let first = (f * self.hop) as isize - (n / 2) as isize;
            let base = f * 2 * bins;
            self.add_frame(
                (&s.re[base..base + bins], &s.im[base..base + bins]),
                (
                    &s.re[base + bins..base + 2 * bins],
                    &s.im[base + bins..base + 2 * bins],
                ),
                first,
                &mut out,
                (&mut re, &mut im),
            );
            for i in 0..n {
                let pos = first + i as isize;
                if pos >= 0 && (pos as usize) < len {
                    norm[pos as usize] += self.window[i] * self.window[i];
                }
            }
        }
        for channel in &mut out {
            for (v, w) in channel.iter_mut().zip(&norm) {
                if *w > 1e-8 {
                    *v /= *w;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(len: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..len)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect()
    }

    #[test]
    fn analysis_then_synthesis_returns_the_signal() {
        let stft = Stft::new(256, Stft::hann(256));
        for len in [1000usize, 1024, 4097] {
            let (l, r) = (signal(len, 1), signal(len, 2));
            let spec = stft.analyse(&l, &r);
            assert_eq!(spec.frames, len / 64 + 1);
            let [ol, or] = stft.synthesise(&spec, len);
            for i in 0..len {
                assert!((ol[i] - l[i]).abs() < 1e-4, "left {i} of {len}");
                assert!((or[i] - r[i]).abs() < 1e-4, "right {i} of {len}");
            }
        }
    }

    #[test]
    fn a_sine_peaks_in_its_bin_in_the_right_channel_only() {
        let stft = Stft::new(512, Stft::hann(512));
        let len = 4096;
        let r: Vec<f32> = (0..len)
            .map(|i| (2.0 * PI * 20.0 * i as f64 / 512.0).sin() as f32)
            .collect();
        let l = vec![0f32; len];
        let spec = stft.analyse(&l, &r);
        let bins = stft.bins();
        let f = 10;
        let mag = |c: usize, k: usize| {
            let i = f * 2 * bins + c * bins + k;
            spec.re[i].hypot(spec.im[i])
        };
        assert!(mag(1, 20) > 50.0);
        assert!(mag(0, 20) < 1e-3);
        assert!(mag(1, 40) < 1.0);
    }
}
