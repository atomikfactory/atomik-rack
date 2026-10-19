//! Signal analysis helpers for assertions.

use rustfft::{FftPlanner, num_complex::Complex};

/// Root mean square of `x` (0 for empty input).
pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>() / x.len() as f64).sqrt() as f32
}

/// Largest absolute sample value.
pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// Linear amplitude to dBFS (−∞ for 0).
pub fn db(amplitude: f32) -> f32 {
    20.0 * amplitude.log10()
}

/// Frequency (Hz) of the strongest FFT bin, refined by parabolic interpolation, using a Hann window.
pub fn dominant_frequency(x: &[f32], sample_rate: f64) -> f64 {
    let n = x.len();
    assert!(n >= 16, "need at least 16 samples");
    let mut buf: Vec<Complex<f64>> = x
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos();
            Complex::new(f64::from(*s) * w, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let mags: Vec<f64> = buf[..n / 2].iter().map(|c| c.norm()).collect();
    let k = (1..mags.len() - 1)
        .max_by(|a, b| mags[*a].total_cmp(&mags[*b]))
        .unwrap_or(1);
    let (a, b, c) = (mags[k - 1].ln(), mags[k].ln(), mags[k + 1].ln());
    let delta = 0.5 * (a - c) / (a - 2.0 * b + c);
    let delta = if delta.is_finite() { delta } else { 0.0 };
    (k as f64 + delta) * sample_rate / n as f64
}

/// Magnitude spectrum (linear) of `x` with a Hann window; bin `k` is `k * sr / x.len()` Hz.
pub fn magnitude_spectrum(x: &[f32]) -> Vec<f64> {
    let n = x.len();
    let mut buf: Vec<Complex<f64>> = x
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let w =
                0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n.max(2) - 1) as f64).cos();
            Complex::new(f64::from(*s) * w, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    buf[..n / 2].iter().map(|c| c.norm()).collect()
}

/// Largest absolute difference between consecutive samples (click detector).
pub fn max_step(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dominant_frequency_finds_sine() {
        let sr = 48_000.0;
        let x: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f64::consts::PI * 440.0 * i as f64 / sr).sin() as f32)
            .collect();
        assert!((dominant_frequency(&x, sr) - 440.0).abs() < 0.5);
        assert!((rms(&x) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3);
        assert!((peak(&x) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn max_step_detects_jump() {
        assert!((max_step(&[0.0, 0.1, 0.9]) - 0.8).abs() < 1e-6);
        assert!((db(0.5) + 6.0206).abs() < 1e-3);
    }
}
