//! Shared helpers for the integration tests.
#![allow(dead_code, missing_docs)]

use std::path::PathBuf;

pub const RATE: u32 = 44_100;
/// Fixture length in frames (0.1 s at 44.1 kHz).
pub const FRAMES: usize = 4410;

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn tmp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("atomik-audio-file-tests")
        .join(format!("{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The source PCM of every fixture: planar i16 samples read from the committed 16-bit WAV.
pub fn source_pcm(stereo: bool) -> Vec<Vec<i16>> {
    let name = if stereo {
        "stereo_wav16.wav"
    } else {
        "mono_wav16.wav"
    };
    let mut r = hound::WavReader::open(fixture(name)).unwrap();
    let n = usize::from(r.spec().channels);
    let mut out = vec![Vec::new(); n];
    for (i, s) in r.samples::<i16>().enumerate() {
        out[i % n].push(s.unwrap());
    }
    out
}

pub fn source_f32(stereo: bool) -> Vec<Vec<f32>> {
    source_pcm(stereo)
        .into_iter()
        .map(|c| c.into_iter().map(|s| f32::from(s) / 32768.0).collect())
        .collect()
}

pub fn sine(rate: u32, freq: f64, amp: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            (amp * (2.0 * std::f64::consts::PI * freq * n as f64 / f64::from(rate)).sin()) as f32
        })
        .collect()
}

pub fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

pub fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// Frequency estimate from positive-going zero crossings with linear interpolation.
pub fn estimate_freq(x: &[f32], rate: u32) -> f64 {
    let mut crossings = Vec::new();
    for i in 1..x.len() {
        if x[i - 1] < 0.0 && x[i] >= 0.0 {
            let frac = f64::from(-x[i - 1]) / f64::from(x[i] - x[i - 1]);
            crossings.push((i - 1) as f64 + frac);
        }
    }
    assert!(
        crossings.len() >= 3,
        "too few zero crossings: {}",
        crossings.len()
    );
    let cycles = (crossings.len() - 1) as f64;
    cycles * f64::from(rate) / (crossings[crossings.len() - 1] - crossings[0])
}
