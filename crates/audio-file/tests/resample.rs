//! Sample-rate conversion: frequency, level, alignment, length and determinism.
#![allow(missing_docs)]

mod common;

use atomik_audio_file::{AudioData, AudioFileError, resample};
use common::*;

fn sine_data(rate: u32, freq: f64, frames: usize, channels: usize) -> AudioData {
    let chans = (0..channels)
        .map(|c| sine(rate, freq * (c + 1) as f64, 0.5, frames))
        .collect();
    AudioData::new(rate, chans).unwrap()
}

fn expected_len(frames: usize, src: u32, dst: u32) -> usize {
    ((frames as f64) * f64::from(dst) / f64::from(src)).round() as usize
}

#[test]
fn equal_rates_are_an_identity_copy() {
    let d = sine_data(44_100, 1000.0, 3000, 2);
    assert_eq!(resample(&d, 44_100).unwrap(), d);
}

#[test]
fn sine_keeps_frequency_level_and_alignment() {
    for (src, dst) in [
        (44_100, 48_000),
        (48_000, 44_100),
        (96_000, 44_100),
        (22_050, 44_100),
        (44_100, 88_200),
    ] {
        let frames = src as usize; // one second
        let d = sine_data(src, 1000.0, frames, 2);
        let out = resample(&d, dst).unwrap();
        assert_eq!(out.sample_rate, dst);
        assert_eq!(out.channel_count(), 2);
        assert_eq!(out.frames(), expected_len(frames, src, dst), "{src}->{dst}");
        for c in 0..2 {
            let f = 1000.0 * (c + 1) as f64;
            let n = out.frames();
            let mid = &out.channels[c][n / 8..7 * n / 8];
            let est = estimate_freq(mid, dst);
            assert!((est - f).abs() < f * 1e-3, "{src}->{dst} ch{c}: {est} Hz");
            let ideal = sine(dst, f, 0.5, n);
            let gain = db(rms(mid) / rms(&ideal[n / 8..7 * n / 8]));
            assert!(
                gain.abs() < 0.1,
                "{src}->{dst} ch{c}: level off by {gain} dB"
            );
            // No offset: the output matches the analytic sine at the new rate sample by sample.
            let max_err = (n / 8..7 * n / 8)
                .map(|i| (out.channels[c][i] - ideal[i]).abs())
                .fold(0.0f32, f32::max);
            assert!(
                max_err < 2e-3,
                "{src}->{dst} ch{c}: max error {max_err} (offset or ripple)"
            );
        }
    }
}

#[test]
fn impulse_lands_at_the_scaled_position() {
    let mut x = vec![0.0f32; 4410];
    x[1000] = 1.0;
    let d = AudioData::new(44_100, vec![x]).unwrap();
    let out = resample(&d, 48_000).unwrap();
    let peak = out.channels[0]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0;
    let expect = 1000.0 * 48_000.0 / 44_100.0; // 1088.44
    assert!(
        (peak as f64 - expect).abs() <= 1.0,
        "peak at {peak}, expected {expect}"
    );
}

#[test]
fn length_is_rounded_for_awkward_sizes() {
    for frames in [1usize, 2, 7, 100, 1023, 1025, 4410, 12_345] {
        for (src, dst) in [(44_100, 48_000), (48_000, 44_100), (8000, 96_000)] {
            let d = sine_data(src, 300.0, frames, 1);
            let out = resample(&d, dst).unwrap();
            let want = ((frames as u128 * u128::from(dst) * 2 + u128::from(src))
                / (2 * u128::from(src))) as usize;
            assert_eq!(out.frames(), want, "{frames} frames {src}->{dst}");
        }
    }
}

#[test]
fn deterministic_and_channel_independent() {
    let d = sine_data(44_100, 440.0, 20_000, 3);
    let a = resample(&d, 48_000).unwrap();
    let b = resample(&d, 48_000).unwrap();
    assert_eq!(a, b);
    // A channel resampled alone equals the same channel inside a multichannel buffer.
    let solo = AudioData::new(44_100, vec![d.channels[1].clone()]).unwrap();
    assert_eq!(resample(&solo, 48_000).unwrap().channels[0], a.channels[1]);
}

#[test]
fn empty_audio_and_bad_target() {
    let empty = AudioData::new(44_100, vec![vec![]]).unwrap();
    let out = resample(&empty, 48_000).unwrap();
    assert_eq!(
        (out.sample_rate, out.frames(), out.channel_count()),
        (48_000, 0, 1)
    );
    let d = sine_data(44_100, 440.0, 100, 1);
    assert!(matches!(
        resample(&d, 0),
        Err(AudioFileError::InvalidData(_))
    ));
}

#[test]
fn fixture_resamples_through_the_whole_pipeline() {
    let d = atomik_audio_file::decode_file(&fixture("stereo_flac.flac")).unwrap();
    let out = resample(&d, 48_000).unwrap();
    assert_eq!(out.frames(), 4800);
    assert!(out.channels.iter().flatten().all(|v| v.is_finite()));
}

/// Max deviation from the analytic sine at the new rate over the middle of the clip.
fn alignment_error(src: u32, dst: u32) -> f32 {
    let d = sine_data(src, 1000.0, src as usize / 2, 1);
    let out = resample(&d, dst).unwrap();
    let ideal = sine(dst, 1000.0, 0.5, out.frames());
    let n = out.frames();
    (n / 8..7 * n / 8)
        .map(|i| (out.channels[0][i] - ideal[i]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn alignment_is_exact_for_common_and_odd_rate_pairs() {
    for (src, dst) in [
        (44_100, 32_000),
        (48_000, 32_000),
        (32_000, 48_000),
        (44_100, 192_000),
        (11_025, 44_100),
        (44_100, 47_999),
    ] {
        let e = alignment_error(src, dst);
        assert!(
            e < 2e-3,
            "{src}->{dst}: max error {e} (an offset would give > 0.03)"
        );
    }
}

/// Rate pairs whose exact alignment would need more than 65 536 padding frames fall back to a
/// rounded delay: at most half an output frame off (0.5 * 2 pi * 1000 / 44101 * 0.5 = 0.036).
#[test]
fn pathological_rate_pair_is_within_half_a_sample() {
    let e = alignment_error(96_000, 44_101);
    assert!(e < 0.04, "max error {e}");
}
