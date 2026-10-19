//! Test-only signal analysis helpers (dsp does not depend on the test kit).

use rustfft::{FftPlanner, num_complex::Complex};

/// Hann-windowed magnitude spectrum; bin `k` is `k·sr/len` Hz.
pub fn spectrum(x: &[f32]) -> Vec<f64> {
    let n = x.len();
    let mut buf: Vec<Complex<f64>> = x
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos();
            Complex::new(f64::from(*s) * w, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    buf[..n / 2].iter().map(|c| c.norm()).collect()
}

/// Frequency of the strongest bin (parabolic interpolation on log magnitudes).
pub fn dominant_hz(x: &[f32], sr: f64) -> f64 {
    let m = spectrum(x);
    let k = (1..m.len() - 1)
        .max_by(|a, b| m[*a].total_cmp(&m[*b]))
        .unwrap_or(1);
    let (a, b, c) = (m[k - 1].ln(), m[k].ln(), m[k + 1].ln());
    let d = 0.5 * (a - c) / (a - 2.0 * b + c);
    (k as f64 + if d.is_finite() { d } else { 0.0 }) * sr / x.len() as f64
}

/// RMS of a slice.
pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt() as f32
}

/// Largest absolute sample-to-sample difference.
pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
}

/// A sine of `freq` Hz at `sr`, amplitude `amp`, `n` samples.
pub fn sine(freq: f64, sr: f64, n: usize, amp: f32) -> Vec<f32> {
    (0..n)
        .map(|i| (amp as f64 * (std::f64::consts::TAU * freq * i as f64 / sr).sin()) as f32)
        .collect()
}

/// Uniform white noise in `-amp..amp` from a seeded generator.
pub fn noise(seed: u64, n: usize, amp: f32) -> Vec<f32> {
    let mut r = crate::Rng::new(seed);
    (0..n).map(|_| r.next_bipolar() * amp).collect()
}

/// Amplitude of the component at `freq` Hz (single-bin DFT over the whole slice, rectangular
/// window; exact when the slice holds a whole number of cycles).
pub fn tone_amplitude(x: &[f32], freq: f64, sr: f64) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, s) in x.iter().enumerate() {
        let w = std::f64::consts::TAU * freq * i as f64 / sr;
        re += f64::from(*s) * w.cos();
        im += f64::from(*s) * w.sin();
    }
    2.0 * (re * re + im * im).sqrt() / x.len() as f64
}

/// Converts a linear amplitude to dB (floor at -300).
pub fn db(x: f64) -> f64 {
    20.0 * x.max(1e-15).log10()
}

/// The block sizes every block-size independence test renders with.
pub const CHUNKS: [usize; 5] = [1, 64, 333, 480, 512];

/// Renders `input` in one piece and in each of [`CHUNKS`] and asserts bit-identical results.
pub fn assert_chunk_invariant_mono<S>(
    make: impl Fn() -> S,
    process: impl Fn(&mut S, &mut [f32]),
    input: &[f32],
) {
    let mut s = make();
    let mut whole = input.to_vec();
    process(&mut s, &mut whole);
    for c in CHUNKS {
        let mut s = make();
        let mut out = input.to_vec();
        for ch in out.chunks_mut(c) {
            process(&mut s, ch);
        }
        assert!(
            out.iter()
                .zip(&whole)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "chunk {c} differs"
        );
    }
}

/// Stereo version of [`assert_chunk_invariant_mono`].
pub fn assert_chunk_invariant_stereo<S>(
    make: impl Fn() -> S,
    process: impl Fn(&mut S, &mut [f32], &mut [f32]),
    left: &[f32],
    right: &[f32],
) {
    let mut s = make();
    let (mut wl, mut wr) = (left.to_vec(), right.to_vec());
    process(&mut s, &mut wl, &mut wr);
    for c in CHUNKS {
        let mut s = make();
        let (mut ol, mut or) = (left.to_vec(), right.to_vec());
        for (a, b) in ol.chunks_mut(c).zip(or.chunks_mut(c)) {
            process(&mut s, a, b);
        }
        let same = |x: &[f32], y: &[f32]| x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits());
        assert!(same(&ol, &wl) && same(&or, &wr), "chunk {c} differs");
    }
}
