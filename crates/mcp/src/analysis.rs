//! Audio analysis for the QA tools: levels, silence, a windowed loudness fingerprint, a coarse
//! spectrum, and the comparison of two renders. Pure functions over de-interleaved channels.

use rustfft::{FftPlanner, num_complex::Complex};
use serde_json::{Value, json};

/// dB floor: silence reports this instead of minus infinity (JSON has no infinity).
pub const DB_FLOOR: f64 = -240.0;
/// Below this level (dBFS) a window counts as silent.
pub const SILENCE_DB: f64 = -80.0;
/// A silent stretch shorter than this (seconds) is not reported.
pub const MIN_SILENCE_S: f64 = 0.05;
/// At most this many loudness windows are reported (the window grows to fit).
pub const MAX_WINDOWS: usize = 400;

/// Linear amplitude to dBFS, floored at [`DB_FLOOR`].
pub fn db(x: f64) -> f64 {
    if x <= 0.0 {
        DB_FLOOR
    } else {
        (20.0 * x.log10()).max(DB_FLOOR)
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// Frames of the shortest channel (all channels of a render have the same length).
pub fn frames(channels: &[Vec<f32>]) -> usize {
    channels.iter().map(Vec::len).min().unwrap_or(0)
}

/// The levels of one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelStats {
    /// Largest absolute sample.
    pub peak: f64,
    /// Root mean square.
    pub rms: f64,
    /// Mean sample (DC offset).
    pub dc: f64,
    /// Samples at or above full scale.
    pub clipped: usize,
    /// NaN and infinite samples.
    pub non_finite: usize,
}

/// Measures one channel.
pub fn channel_stats(x: &[f32]) -> ChannelStats {
    let (mut peak, mut sum, mut sq, mut clipped, mut bad) = (0.0f64, 0.0f64, 0.0f64, 0, 0);
    for &s in x {
        if !s.is_finite() {
            bad += 1;
            continue;
        }
        let v = f64::from(s);
        peak = peak.max(v.abs());
        sum += v;
        sq += v * v;
        if v.abs() >= 1.0 {
            clipped += 1;
        }
    }
    let n = (x.len() - bad).max(1) as f64;
    ChannelStats {
        peak,
        rms: (sq / n).sqrt(),
        dc: sum / n,
        clipped,
        non_finite: bad,
    }
}

/// The mono mix (mean of the channels; non-finite samples count as 0).
pub fn mono(channels: &[Vec<f32>]) -> Vec<f64> {
    let n = frames(channels);
    let c = channels.len().max(1) as f64;
    (0..n)
        .map(|i| {
            channels
                .iter()
                .map(|ch| {
                    let v = f64::from(ch[i]);
                    if v.is_finite() { v } else { 0.0 }
                })
                .sum::<f64>()
                / c
        })
        .collect()
}

/// RMS in dBFS of consecutive windows of `window` frames of `x` (the last may be shorter).
pub fn window_rms_db(x: &[f64], window: usize) -> Vec<f64> {
    let window = window.max(1);
    x.chunks(window)
        .map(|w| db((w.iter().map(|v| v * v).sum::<f64>() / w.len() as f64).sqrt()))
        .collect()
}

/// The window length in frames: `window_s` seconds, grown so at most [`MAX_WINDOWS`] result.
pub fn window_frames(frames: usize, sample_rate: u32, window_s: f64) -> usize {
    let wanted = (window_s.max(0.001) * f64::from(sample_rate)).round() as usize;
    wanted.max(frames.div_ceil(MAX_WINDOWS)).max(1)
}

/// The stretches (seconds) that stay below [`SILENCE_DB`] for at least [`MIN_SILENCE_S`], found
/// on 10 ms windows.
pub fn silences(x: &[f64], sample_rate: u32) -> Vec<(f64, f64)> {
    let step = (f64::from(sample_rate) * 0.01).round().max(1.0) as usize;
    let levels = window_rms_db(x, step);
    let secs = |i: usize| (i * step) as f64 / f64::from(sample_rate);
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, l) in levels
        .iter()
        .enumerate()
        .chain(std::iter::once((levels.len(), &0.0)))
    {
        let quiet = i < levels.len() && *l < SILENCE_DB;
        match (quiet, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                let (a, b) = (
                    secs(s),
                    secs(i).min(x.len() as f64 / f64::from(sample_rate)),
                );
                if b - a >= MIN_SILENCE_S {
                    out.push((a, b));
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// The band edges (Hz) of the coarse spectrum and their names.
pub const BANDS: [(&str, f64, f64); 8] = [
    ("sub", 20.0, 60.0),
    ("bass", 60.0, 250.0),
    ("low-mid", 250.0, 500.0),
    ("mid", 500.0, 2000.0),
    ("upper-mid", 2000.0, 4000.0),
    ("presence", 4000.0, 6000.0),
    ("brilliance", 6000.0, 12_000.0),
    ("air", 12_000.0, 20_000.0),
];

/// The average power spectrum of `x` (Hann frames of `frame` samples, 50 % overlap): the power
/// of each bin `0..=frame/2`.
pub fn power_spectrum(x: &[f64], frame: usize) -> Vec<f64> {
    let fft = FftPlanner::<f64>::new().plan_fft_forward(frame);
    let hann: Vec<f64> = (0..frame)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / frame as f64).cos())
        .collect();
    let mut acc = vec![0.0; frame / 2 + 1];
    let mut count = 0usize;
    let hop = frame / 2;
    let mut pos = 0;
    while pos + frame <= x.len() {
        let mut buf: Vec<Complex<f64>> = x[pos..pos + frame]
            .iter()
            .zip(&hann)
            .map(|(s, w)| Complex::new(s * w, 0.0))
            .collect();
        fft.process(&mut buf);
        for (a, c) in acc.iter_mut().zip(&buf) {
            *a += c.norm_sqr();
        }
        count += 1;
        pos += hop;
    }
    if count > 0 {
        for a in &mut acc {
            *a /= count as f64;
        }
    }
    acc
}

/// The frame size of the spectrum for `frames` samples: 4096, or the largest power of two that
/// fits a shorter signal (at least 256).
pub fn spectrum_frame(frames: usize) -> usize {
    let mut f = 4096;
    while f > 256 && f > frames {
        f /= 2;
    }
    f
}

/// The coarse spectrum as JSON: the share of the power in each band (dB relative to the total
/// power, so the bands of a full-range signal sum to about 0 dB) and the strongest bin.
pub fn spectrum_json(x: &[f64], sample_rate: u32) -> Value {
    let frame = spectrum_frame(x.len());
    let spec = power_spectrum(x, frame);
    let total: f64 = spec.iter().skip(1).sum();
    let bin_hz = f64::from(sample_rate) / frame as f64;
    let bands: Vec<Value> = BANDS
        .iter()
        .map(|(name, lo, hi)| {
            let power: f64 = spec
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    let f = *i as f64 * bin_hz;
                    f >= *lo && f < *hi
                })
                .map(|(_, p)| p)
                .sum();
            let share = if total > 0.0 { power / total } else { 0.0 };
            json!({
                "band": name,
                "from_hz": lo,
                "to_hz": hi,
                "share_db": round2(db(share.sqrt())),
            })
        })
        .collect();
    let dominant = spec
        .iter()
        .enumerate()
        .skip(1)
        .max_by(|a, b| a.1.total_cmp(b.1))
        .filter(|(_, p)| **p > 0.0)
        .map(|(i, _)| round2(i as f64 * bin_hz));
    json!({ "frame": frame, "bands": bands, "dominant_hz": dominant })
}

/// The full report of a render.
pub fn report(channels: &[Vec<f32>], sample_rate: u32, window_s: f64) -> Value {
    let n = frames(channels);
    let stats: Vec<ChannelStats> = channels.iter().map(|c| channel_stats(c)).collect();
    let mix = mono(channels);
    let window = window_frames(n, sample_rate, window_s);
    let levels = window_rms_db(&mix, window);
    let peak = stats.iter().fold(0.0f64, |m, s| m.max(s.peak));
    let rms_all = if stats.is_empty() {
        0.0
    } else {
        (stats.iter().map(|s| s.rms * s.rms).sum::<f64>() / stats.len() as f64).sqrt()
    };
    let silent = peak == 0.0;
    let per_channel: Vec<Value> = stats
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "channel": i,
                "peak": round4(s.peak),
                "peak_dbfs": round2(db(s.peak)),
                "rms_dbfs": round2(db(s.rms)),
                "dc_offset": round4(s.dc),
                "clipped_samples": s.clipped,
                "non_finite_samples": s.non_finite,
            })
        })
        .collect();
    let silence: Vec<Value> = silences(&mix, sample_rate)
        .into_iter()
        .map(|(a, b)| json!({"from_s": round4(a), "to_s": round4(b)}))
        .collect();
    json!({
        "sample_rate": sample_rate,
        "channels": channels.len(),
        "frames": n,
        "duration_s": round4(n as f64 / f64::from(sample_rate.max(1))),
        "peak_dbfs": round2(db(peak)),
        "rms_dbfs": round2(db(rms_all)),
        "silent": silent,
        "clipped_samples": stats.iter().map(|s| s.clipped).sum::<usize>(),
        "non_finite_samples": stats.iter().map(|s| s.non_finite).sum::<usize>(),
        "per_channel": per_channel,
        "silences": silence,
        "window_s": round4(window as f64 / f64::from(sample_rate.max(1))),
        "window_rms_dbfs": levels.iter().map(|l| round2(*l)).collect::<Vec<_>>(),
        "spectrum": spectrum_json(&mix, sample_rate),
    })
}

/// The result of comparing two renders.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    /// Same sample rate, channel count and length.
    pub same_shape: bool,
    /// Every sample equal (and the same shape).
    pub bit_identical: bool,
    /// Largest absolute sample difference over the common frames.
    pub max_abs_diff: f64,
    /// First frame that differs, if any.
    pub first_diff_frame: Option<usize>,
    /// Largest per-window difference of the loudness (dB) between the two.
    pub max_window_diff_db: f64,
    /// The window where it happens, in seconds.
    pub worst_window_s: f64,
    /// The window length used (seconds).
    pub window_s: f64,
}

/// Loudness differences below this level in both renders count as no difference.
const COMPARE_QUIET_DB: f64 = -90.0;

/// Compares `a` with `b`.
pub fn compare(
    a: &[Vec<f32>],
    a_rate: u32,
    b: &[Vec<f32>],
    b_rate: u32,
    window_s: f64,
) -> Comparison {
    let (na, nb) = (frames(a), frames(b));
    let common = na.min(nb);
    let same_shape = a_rate == b_rate && a.len() == b.len() && na == nb;
    let mut max_abs = 0.0f64;
    let mut first = None;
    for (ca, cb) in a.iter().zip(b) {
        for i in 0..common {
            let d = (f64::from(ca[i]) - f64::from(cb[i])).abs();
            // A NaN on one side is a difference whatever the other holds.
            let d = if d.is_nan() { f64::INFINITY } else { d };
            if d > 0.0 {
                first = Some(first.map_or(i, |f: usize| f.min(i)));
            }
            max_abs = max_abs.max(d);
        }
    }
    let window = window_frames(common, a_rate, window_s);
    let (la, lb) = (
        window_rms_db(&mono(a)[..common], window),
        window_rms_db(&mono(b)[..common], window),
    );
    let (mut worst, mut at) = (0.0f64, 0usize);
    for (i, (x, y)) in la.iter().zip(&lb).enumerate() {
        let d = if *x < COMPARE_QUIET_DB && *y < COMPARE_QUIET_DB {
            0.0
        } else {
            (x - y).abs()
        };
        if d > worst {
            worst = d;
            at = i;
        }
    }
    Comparison {
        same_shape,
        bit_identical: same_shape && first.is_none() && max_abs == 0.0,
        max_abs_diff: max_abs,
        first_diff_frame: first,
        max_window_diff_db: worst,
        worst_window_s: (at * window) as f64 / f64::from(a_rate.max(1)),
        window_s: window as f64 / f64::from(a_rate.max(1)),
    }
}

impl Comparison {
    /// The comparison as JSON, with the verdict for `tolerance_db`: the same shape and no window
    /// louder or quieter than the tolerance.
    pub fn to_json(&self, tolerance_db: f64) -> Value {
        let matches = self.same_shape && self.max_window_diff_db <= tolerance_db;
        json!({
            "matches": matches,
            "tolerance_db": tolerance_db,
            "bit_identical": self.bit_identical,
            "same_shape": self.same_shape,
            "max_abs_diff": self.max_abs_diff,
            "first_diff_frame": self.first_diff_frame,
            "max_window_diff_db": round4(self.max_window_diff_db),
            "worst_window_s": round4(self.worst_window_s),
            "window_s": round4(self.window_s),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn sine(freq: f64, amp: f64, seconds: f64) -> Vec<f32> {
        (0..(seconds * f64::from(SR)) as usize)
            .map(|i| {
                (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(SR)).sin()) as f32
            })
            .collect()
    }

    #[test]
    fn a_half_scale_sine_measures_minus_six_dbfs_peak_and_its_frequency() {
        let ch = vec![sine(1000.0, 0.5, 1.0)];
        let r = report(&ch, SR, 0.5);
        assert!((r["peak_dbfs"].as_f64().unwrap() + 6.02).abs() < 0.05);
        // RMS of a sine is the peak minus 3.01 dB.
        assert!((r["rms_dbfs"].as_f64().unwrap() + 9.03).abs() < 0.05);
        let dominant = r["spectrum"]["dominant_hz"].as_f64().unwrap();
        assert!((dominant - 1000.0).abs() < 12.0, "dominant {dominant}");
        assert_eq!(r["silent"], false);
        assert_eq!(r["clipped_samples"], 0);
    }

    #[test]
    fn silence_is_reported_with_its_stretches() {
        let mut x = sine(440.0, 0.5, 0.5);
        x.extend(vec![0.0; (SR / 2) as usize]);
        x.extend(sine(440.0, 0.5, 0.5));
        let r = report(&[x], SR, 0.25);
        let s = r["silences"].as_array().unwrap();
        assert_eq!(s.len(), 1, "{s:?}");
        assert!((s[0]["from_s"].as_f64().unwrap() - 0.5).abs() < 0.02);
        assert!((s[0]["to_s"].as_f64().unwrap() - 1.0).abs() < 0.02);
        let all_zero = report(&[vec![0.0; 4800]], SR, 0.1);
        assert_eq!(all_zero["silent"], true);
        assert_eq!(all_zero["peak_dbfs"], DB_FLOOR);
    }

    #[test]
    fn clipping_dc_and_non_finite_samples_are_counted() {
        let x = vec![0.0, 1.0, -1.5, f32::NAN, 0.25];
        let s = channel_stats(&x);
        assert_eq!(s.clipped, 2);
        assert_eq!(s.non_finite, 1);
        assert!((s.dc - (-0.0625)).abs() < 1e-9);
    }

    #[test]
    fn the_window_grows_so_a_long_render_reports_at_most_the_cap() {
        let w = window_frames(SR as usize * 600, SR, 0.5);
        assert!(SR as usize * 600 / w <= MAX_WINDOWS);
        assert_eq!(window_frames(1000, SR, 0.5), (0.5 * f64::from(SR)) as usize);
    }

    #[test]
    fn identical_renders_are_bit_identical_and_match() {
        let a = vec![sine(220.0, 0.4, 0.5), sine(330.0, 0.4, 0.5)];
        let c = compare(&a, SR, &a.clone(), SR, 0.1);
        assert!(c.bit_identical && c.same_shape);
        assert_eq!(c.to_json(0.5)["matches"], true);
    }

    #[test]
    fn a_louder_render_fails_the_tolerance_and_names_the_window() {
        let a = vec![sine(220.0, 0.25, 1.0)];
        let mut loud = sine(220.0, 0.25, 1.0);
        for s in &mut loud[SR as usize / 2..] {
            *s *= 2.0; // +6 dB in the second half
        }
        let c = compare(&a, SR, &[loud], SR, 0.25);
        assert!(!c.bit_identical);
        assert!((c.max_window_diff_db - 6.02).abs() < 0.1, "{c:?}");
        assert!(c.worst_window_s >= 0.5 - 1e-9);
        assert!(c.first_diff_frame.unwrap() >= SR as usize / 2);
        assert_eq!(c.to_json(0.5)["matches"], false);
        assert_eq!(c.to_json(7.0)["matches"], true);
    }

    #[test]
    fn a_different_length_is_not_the_same_shape() {
        let a = vec![sine(220.0, 0.4, 0.5)];
        let b = vec![sine(220.0, 0.4, 0.6)];
        let c = compare(&a, SR, &b, SR, 0.1);
        assert!(!c.same_shape);
        assert_eq!(c.to_json(100.0)["matches"], false);
    }

    #[test]
    fn a_nan_on_one_side_is_a_difference() {
        let a = vec![vec![0.1, 0.2, 0.3]];
        let b = vec![vec![0.1, f32::NAN, 0.3]];
        let c = compare(&a, SR, &b, SR, 0.1);
        assert!(!c.bit_identical);
        assert_eq!(c.first_diff_frame, Some(1));
    }
}
