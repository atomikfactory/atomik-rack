//! Progressive decoding: header probing and chunked decoding of every supported format.
#![allow(missing_docs)]

mod common;

use atomik_audio_file::{StreamDecoder, decode_file, probe_file};
use common::*;
use std::path::{Path, PathBuf};

/// Writes a stereo 16-bit WAV of `frames` frames (a sine on the left, a ramp on the right).
fn write_wav(path: &Path, rate: u32, frames: usize) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for n in 0..frames {
        let l = (12_000.0 * (n as f64 * 0.05).sin()) as i16;
        w.write_sample(l).unwrap();
        w.write_sample((n % 2000) as i16 - 1000).unwrap();
    }
    w.finalize().unwrap();
}

const ALL_FIXTURES: [&str; 14] = [
    "stereo_wav16.wav",
    "stereo_wav24.wav",
    "stereo_wavf32.wav",
    "stereo_aiff16.aif",
    "stereo_flac.flac",
    "stereo_mp3.mp3",
    "stereo_ogg.ogg",
    "mono_wav16.wav",
    "mono_wav24.wav",
    "mono_wavf32.wav",
    "mono_aiff16.aif",
    "mono_flac.flac",
    "mono_mp3.mp3",
    "mono_ogg.ogg",
];

fn drain(dec: &mut StreamDecoder, min: usize) -> Vec<Vec<f32>> {
    let mut out: Vec<Vec<f32>> = Vec::new();
    while let Some(chunk) = dec.next_chunk(min).unwrap() {
        if out.is_empty() {
            out = vec![Vec::new(); chunk.len()];
        }
        assert_eq!(chunk.len(), out.len());
        let n = chunk[0].len();
        assert!(n > 0, "empty chunk");
        for (dst, src) in out.iter_mut().zip(&chunk) {
            assert_eq!(src.len(), n, "ragged planes");
            dst.extend_from_slice(src);
        }
    }
    out
}

#[test]
fn probe_wav_reports_header() {
    let dir = tmp_dir("probe_wav");
    let p = dir.join("a.wav");
    write_wav(&p, 48_000, 1000);
    let info = probe_file(&p).unwrap();
    assert_eq!(info.sample_rate, 48_000);
    assert_eq!(info.channels, 2);
    assert_eq!(info.frames, Some(1000));
}

#[test]
fn probe_does_not_depend_on_length() {
    // Header says 60 s but the data is cut off after 0.1 s: only a header-only probe succeeds.
    let dir = tmp_dir("probe_trunc");
    let p = dir.join("long.wav");
    write_wav(&p, 44_100, 44_100 * 60);
    let bytes = std::fs::read(&p).unwrap();
    std::fs::write(&p, &bytes[..44 + 4 * 4410]).unwrap();
    let info = probe_file(&p).unwrap();
    assert_eq!(info.frames, Some(44_100 * 60));
}

#[test]
fn probe_all_fixtures_match_full_decode() {
    for name in ALL_FIXTURES {
        let full = decode_file(&fixture(name)).unwrap();
        let info = probe_file(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(info.sample_rate, full.sample_rate, "{name}");
        assert_eq!(usize::from(info.channels), full.channel_count(), "{name}");
        if let Some(f) = info.frames {
            // Lossy headers may count padding or not; lossless must be exact.
            if !name.ends_with("mp3") && !name.ends_with("ogg") {
                assert_eq!(f, full.frames() as u64, "{name}");
            }
        }
    }
}

#[test]
fn probe_errors() {
    let dir = tmp_dir("probe_err");
    assert!(probe_file(&dir.join("missing.wav")).is_err());
    let junk: PathBuf = dir.join("junk.wav");
    std::fs::write(&junk, b"this is not audio at all, really not").unwrap();
    assert!(probe_file(&junk).is_err());
    let empty = dir.join("empty.mp3");
    std::fs::write(&empty, b"").unwrap();
    assert!(probe_file(&empty).is_err());
}

#[test]
fn chunks_equal_full_decode() {
    for name in ALL_FIXTURES {
        let full = decode_file(&fixture(name)).unwrap();
        for min in [1usize, 100, 2048, 1 << 20] {
            let mut dec = StreamDecoder::open(&fixture(name)).unwrap();
            let got = drain(&mut dec, min);
            assert_eq!(got.len(), full.channel_count(), "{name}");
            for (c, (g, w)) in got.iter().zip(&full.channels).enumerate() {
                assert_eq!(g.len(), w.len(), "{name} min {min} ch{c} length");
                assert!(
                    g.iter().zip(w).all(|(a, b)| a.to_bits() == b.to_bits()),
                    "{name} min {min} ch{c}"
                );
            }
            assert!(
                dec.next_chunk(min).unwrap().is_none(),
                "{name}: None forever"
            );
        }
    }
}

#[test]
fn chunk_is_at_least_min_frames_and_bounded() {
    let dir = tmp_dir("chunk_size");
    let p = dir.join("a.wav");
    write_wav(&p, 44_100, 100_000);
    let mut dec = StreamDecoder::open(&p).unwrap();
    assert_eq!(dec.info().frames, Some(100_000));
    let mut total = 0;
    while let Some(c) = dec.next_chunk(2048).unwrap() {
        let n = c[0].len();
        assert!(n >= 2048.min(100_000 - total), "short chunk {n}");
        assert!(n < 2048 + 8192, "unbounded chunk {n}");
        total += n;
    }
    assert_eq!(total, 100_000);
}

#[test]
fn truncated_wav_yields_decoded_prefix() {
    let dir = tmp_dir("trunc_wav");
    let p = dir.join("a.wav");
    write_wav(&p, 44_100, 50_000);
    let bytes = std::fs::read(&p).unwrap();
    std::fs::write(&p, &bytes[..44 + 4 * 20_000 + 2]).unwrap();
    assert!(decode_file(&p).is_err(), "decode_file stays strict");
    let mut dec = StreamDecoder::open(&p).unwrap();
    let got = drain(&mut dec, 1024);
    let n = got[0].len();
    assert!((19_000..=20_001).contains(&n), "{n}");
    assert!(dec.next_chunk(1024).unwrap().is_none());
}

/// The fixture holds a single FLAC frame, so cutting it loses all audio: the stream must end
/// cleanly (no error) rather than fail like `decode_file`.
#[test]
fn truncated_flac_ends_cleanly() {
    let dir = tmp_dir("trunc_flac");
    let p = dir.join("a.flac");
    let bytes = std::fs::read(fixture("stereo_flac.flac")).unwrap();
    std::fs::write(&p, &bytes[..bytes.len() - 1000]).unwrap();
    assert!(decode_file(&p).is_err(), "decode_file stays strict");
    let mut dec = StreamDecoder::open(&p).unwrap();
    let got = drain(&mut dec, 512);
    assert!(got.iter().all(|c| c.len() < FRAMES));
    assert!(dec.next_chunk(512).unwrap().is_none());
}

#[test]
fn corrupt_mp3_tail_never_errors_or_panics() {
    let dir = tmp_dir("corrupt_mp3");
    let p = dir.join("a.mp3");
    let mut bytes = std::fs::read(fixture("stereo_mp3.mp3")).unwrap();
    let n = bytes.len();
    for b in &mut bytes[n / 2..n * 3 / 4] {
        *b = 0xA5;
    }
    std::fs::write(&p, &bytes).unwrap();
    let mut dec = StreamDecoder::open(&p).unwrap();
    while let Ok(Some(_)) = dec.next_chunk(256) {}
    assert!(dec.next_chunk(256).unwrap().is_none());
}

#[test]
fn open_rejects_bad_files() {
    let dir = tmp_dir("open_bad");
    assert!(StreamDecoder::open(&dir.join("nope.flac")).is_err());
    let junk = dir.join("junk.ogg");
    std::fs::write(&junk, vec![7u8; 5000]).unwrap();
    assert!(StreamDecoder::open(&junk).is_err());
}

/// Latency of open + first 2048-frame chunk. Run: `cargo test -p atomik-audio-file --release
/// -- --ignored stream`.
#[test]
#[ignore = "timing; run in release"]
#[allow(clippy::disallowed_methods)] // a wall-clock latency measurement needs Instant::now
fn stream_first_chunk_latency() {
    let dir = tmp_dir("latency");
    let p = dir.join("ten_min.wav");
    write_wav(&p, 44_100, 44_100 * 600);
    let targets = [p, fixture("stereo_mp3.mp3"), fixture("stereo_ogg.ogg")];
    for t in targets {
        let mut best = f64::MAX;
        for _ in 0..5 {
            let start = std::time::Instant::now();
            let mut dec = StreamDecoder::open(&t).unwrap();
            let c = dec.next_chunk(2048).unwrap().unwrap();
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            assert!(!c[0].is_empty());
            best = best.min(ms);
        }
        println!("{}: open + first chunk {best:.3} ms", t.display());
        assert!(best < 10.0, "{}: {best} ms", t.display());
    }
}
