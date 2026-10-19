//! Decoding of every supported format from the fixture set.
#![allow(missing_docs)]

mod common;

use atomik_audio_file::{AudioFileError, decode_bytes, decode_file};
use common::*;

/// Lossless fixtures decode bit-exactly to `i16 / 2^15`, for both channel layouts.
#[test]
fn lossless_formats_are_bit_exact() {
    for stereo in [true, false] {
        let prefix = if stereo { "stereo" } else { "mono" };
        let expect = source_f32(stereo);
        for file in [
            "wav16.wav",
            "wav24.wav",
            "wavf32.wav",
            "aiff16.aif",
            "flac.flac",
        ] {
            let name = format!("{prefix}_{file}");
            let d = decode_file(&fixture(&name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(d.sample_rate, RATE, "{name}");
            assert_eq!(d.channel_count(), expect.len(), "{name}");
            assert_eq!(d.frames(), FRAMES, "{name}");
            for (c, (got, want)) in d.channels.iter().zip(&expect).enumerate() {
                for (i, (g, w)) in got.iter().zip(want).enumerate() {
                    assert_eq!(g.to_bits(), w.to_bits(), "{name} ch{c} frame {i}");
                }
            }
        }
    }
}

#[test]
fn decode_bytes_matches_decode_file_and_ignores_wrong_hint() {
    let name = "stereo_flac.flac";
    let bytes = std::fs::read(fixture(name)).unwrap();
    let a = decode_file(&fixture(name)).unwrap();
    assert_eq!(a, decode_bytes(&bytes, Some("flac")).unwrap());
    assert_eq!(a, decode_bytes(&bytes, None).unwrap());
    assert_eq!(
        a,
        decode_bytes(&bytes, Some("mp3")).unwrap(),
        "probe falls back to content"
    );
}

/// Lossy fixtures: right channel count, frequency and level correct. The encoders add priming
/// padding, so the decoded length is `FRAMES` plus at most two codec frames of padding; the
/// level and frequency are measured on the middle half where padding cannot matter.
#[test]
fn lossy_formats_keep_frequency_level_and_length() {
    for stereo in [true, false] {
        let prefix = if stereo { "stereo" } else { "mono" };
        let src = source_f32(stereo);
        for file in ["mp3.mp3", "ogg.ogg"] {
            let name = format!("{prefix}_{file}");
            let d = decode_file(&fixture(&name)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(d.sample_rate, RATE, "{name}");
            assert_eq!(d.channel_count(), src.len(), "{name}");
            let n = d.frames();
            assert!(
                (FRAMES.saturating_sub(64)..=FRAMES + 2 * 1152).contains(&n),
                "{name}: {n} frames, expected {FRAMES} + codec padding"
            );
            let freqs = [440.0, 660.0];
            for c in 0..d.channel_count() {
                let mid = &d.channels[c][n / 4..3 * n / 4];
                let mid_src = &src[c][FRAMES / 4..3 * FRAMES / 4];
                let f = estimate_freq(mid, RATE);
                assert!(
                    (f - freqs[c]).abs() < freqs[c] * 0.01,
                    "{name} ch{c}: {f} Hz"
                );
                let diff = db(rms(mid) / rms(mid_src));
                assert!(diff.abs() < 1.0, "{name} ch{c}: level off by {diff} dB");
            }
        }
    }
}

fn wav_bytes(channels: u16, bits: u16, float: bool, frames: usize) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: 22_050,
        bits_per_sample: bits,
        sample_format: if float {
            hound::SampleFormat::Float
        } else {
            hound::SampleFormat::Int
        },
    };
    let mut cur = std::io::Cursor::new(Vec::new());
    let mut w = hound::WavWriter::new(&mut cur, spec).unwrap();
    for i in 0..frames {
        for c in 0..channels {
            let v = (i as i64 * 7 + i64::from(c) * 3) % 200 - 100; // small ramp, -100..99
            match (float, bits) {
                (true, _) => w.write_sample(v as f32 / 128.0).unwrap(),
                (false, 8) => w.write_sample(v as i8).unwrap(),
                (false, 16) => w.write_sample((v * 300) as i16).unwrap(),
                (false, 24) => w.write_sample((v * 70_000) as i32).unwrap(),
                (false, _) => w.write_sample((v * 15_000_000) as i32).unwrap(),
            }
        }
    }
    w.finalize().unwrap();
    cur.into_inner()
}

#[test]
fn integer_pcm_converts_exactly_by_bit_depth() {
    for bits in [8u16, 16, 24, 32] {
        let d = decode_bytes(&wav_bytes(2, bits, false, 300), Some("wav")).unwrap();
        assert_eq!(
            (d.sample_rate, d.channel_count(), d.frames()),
            (22_050, 2, 300)
        );
        let scale = 2f64.powi(i32::from(bits) - 1);
        for i in 0..300usize {
            for c in 0..2usize {
                let v = (i as i64 * 7 + c as i64 * 3) % 200 - 100;
                let int = match bits {
                    8 => v,
                    16 => v * 300,
                    24 => v * 70_000,
                    _ => v * 15_000_000,
                };
                assert_eq!(
                    d.channels[c][i],
                    (int as f64 / scale) as f32,
                    "{bits}-bit frame {i}"
                );
            }
        }
    }
}

#[test]
fn float32_wav_is_exact() {
    let d = decode_bytes(&wav_bytes(1, 32, true, 200), None).unwrap();
    for (i, s) in d.channels[0].iter().enumerate() {
        let v = (i as i64 * 7) % 200 - 100;
        assert_eq!(s.to_bits(), (v as f32 / 128.0).to_bits());
    }
}

#[test]
fn more_than_two_channels_are_kept() {
    let d = decode_bytes(&wav_bytes(4, 16, false, 50), Some("wav")).unwrap();
    assert_eq!((d.channel_count(), d.frames()), (4, 50));
}

#[test]
fn bad_inputs_are_errors() {
    assert!(matches!(
        decode_file(&fixture("does-not-exist.wav")),
        Err(AudioFileError::Io(_))
    ));
    assert!(decode_bytes(b"", None).is_err());
    assert!(decode_bytes(b"this is not audio at all, just text", Some("wav")).is_err());
    let empty = wav_bytes(2, 16, false, 0);
    assert!(matches!(
        decode_bytes(&empty, Some("wav")),
        Err(AudioFileError::Empty)
    ));
    // A WAV cut in the middle of its data chunk must be reported, not silently shortened.
    let full = wav_bytes(2, 16, false, 1000);
    assert!(decode_bytes(&full[..full.len() / 2], Some("wav")).is_err());
    let flac = std::fs::read(fixture("stereo_flac.flac")).unwrap();
    assert!(decode_bytes(&flac[..flac.len() / 2], Some("flac")).is_err());
}

#[test]
fn decoding_is_deterministic() {
    for name in ["stereo_mp3.mp3", "stereo_ogg.ogg", "stereo_flac.flac"] {
        let a = decode_file(&fixture(name)).unwrap();
        let b = decode_file(&fixture(name)).unwrap();
        assert_eq!(a, b, "{name}");
    }
}

/// Symphonia 0.6.1 overflows a shift on corrupt WAV headers with more than 32 channels; the
/// crate rejects them before they reach it.
#[test]
fn absurd_channel_counts_are_rejected_cleanly() {
    let ok = decode_bytes(&wav_bytes(8, 16, false, 4), Some("wav")).unwrap();
    assert_eq!(ok.channel_count(), 8);
    let too_many = wav_bytes(33, 16, false, 4);
    assert!(matches!(
        decode_bytes(&too_many, Some("wav")),
        Err(AudioFileError::Unsupported(_))
    ));
    let mut zero = wav_bytes(1, 16, false, 4);
    zero[22] = 0; // fmt chunk: channel count
    assert!(matches!(
        decode_bytes(&zero, Some("wav")),
        Err(AudioFileError::Decode(_))
    ));
}
