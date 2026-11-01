//! The bulk WAV writer and reader are byte-for-byte what the sample-at-a-time versions were,
//! and the streaming reader reads ahead.
#![allow(missing_docs)]

mod common;

use atomik_audio_file::{AudioData, WavStreamReader, read_wav_f32, write_wav_f32};
use atomik_testkit::no_alloc::{assert_no_alloc, peak_heap_of};

atomik_testkit::install_alloc_guard!();

fn signal(channels: usize, frames: usize) -> AudioData {
    let chans = (0..channels)
        .map(|c| {
            (0..frames)
                .map(|i| match i % 5 {
                    0 => -0.0,
                    1 => f32::MIN_POSITIVE,
                    _ => ((i * 13 + c * 7) as f32).cos() * 2.0,
                })
                .collect()
        })
        .collect();
    AudioData::new(44_100, chans).unwrap()
}

/// The file `hound` writes for `data`, sample at a time: the reference the bulk writer matches.
fn hound_bytes(data: &AudioData) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: data.channels.len() as u16,
        sample_rate: data.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
    for i in 0..data.frames() {
        for ch in &data.channels {
            w.write_sample(ch[i]).unwrap();
        }
    }
    w.finalize().unwrap();
    cursor.into_inner()
}

#[test]
fn written_bytes_equal_the_sample_at_a_time_writer() {
    for (ch, frames) in [
        (1, 0),
        (1, 1),
        (2, 8191),
        (2, 8192),
        (2, 8193),
        (3, 20_000),
        (8, 9),
    ] {
        let data = signal(ch, frames);
        let path = common::tmp_dir(&format!("bytes-{ch}-{frames}")).join("a.wav");
        write_wav_f32(&path, &data).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            hound_bytes(&data),
            "{ch} channels, {frames} frames"
        );
    }
}

#[test]
fn bulk_read_returns_every_sample_bit_for_bit() {
    for (ch, frames) in [(1, 1), (2, 8191), (2, 8192), (2, 8193), (5, 30_000)] {
        let data = signal(ch, frames);
        let path = common::tmp_dir(&format!("read-{ch}-{frames}")).join("a.wav");
        write_wav_f32(&path, &data).unwrap();
        let back = read_wav_f32(&path).unwrap();
        for (a, b) in data
            .channels
            .iter()
            .flatten()
            .zip(back.channels.iter().flatten())
        {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        assert_eq!((back.channel_count(), back.frames()), (ch, frames));
    }
}

#[test]
fn a_data_chunk_ending_inside_a_frame_is_still_an_error() {
    let data = signal(2, 10);
    let path = common::tmp_dir("partial-frame").join("a.wav");
    write_wav_f32(&path, &data).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    // Cut one sample off the end and say so in both size fields.
    bytes.truncate(bytes.len() - 4);
    let len = bytes.len() as u32;
    bytes[4..8].copy_from_slice(&(len - 8).to_le_bytes());
    let at = bytes.len() - 19 * 4 - 4;
    bytes[at..at + 4].copy_from_slice(&(19u32 * 4).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let err = read_wav_f32(&path).unwrap_err().to_string();
    assert!(!err.is_empty(), "{err}");
}

#[test]
fn sequential_playback_reads_ahead() {
    let frames = 4096 * 3 + 100;
    let data = signal(2, frames);
    let path = common::tmp_dir("read-ahead").join("a.wav");
    write_wav_f32(&path, &data).unwrap();
    let mut r = WavStreamReader::open(&path).unwrap();
    let mut out = vec![vec![0.0f32; 256]; 2];
    let mut at = 0u64;
    while at < frames as u64 {
        let n = r.read_frames(at, &mut out).unwrap();
        for (c, got) in out.iter().enumerate() {
            let want = &data.channels[c][at as usize..at as usize + n];
            assert!(
                got[..n]
                    .iter()
                    .zip(want)
                    .all(|(a, b)| a.to_bits() == b.to_bits())
            );
        }
        at += n as u64;
    }
    // 4096-frame blocks: one system call per block, not per 256-frame chunk.
    assert_eq!(r.read_calls(), 4);
    // A jump back reads once more, and reading it does not allocate.
    assert_no_alloc(|| r.read_frames(10, &mut out).unwrap());
    assert_eq!(r.read_calls(), 5);
}

#[test]
fn validate_does_not_copy_the_buffer() {
    let data = signal(2, 100_000);
    let (ok, peak) = peak_heap_of(|| data.validate().is_ok());
    assert!(ok);
    assert_eq!(peak, 0);
    let mut bad = signal(2, 10);
    bad.channels[1].pop();
    assert!(bad.validate().is_err());
}

/// Hand check: `cargo test -p atomik-audio-file --release --test wav_io -- --ignored --nocapture`
/// prints the time of the bulk writer and reader against the sample-at-a-time loops.
#[test]
#[ignore = "timing"]
fn bulk_io_is_several_times_faster_than_sample_at_a_time() {
    use std::time::Instant;
    let data = signal(2, 44_100 * 60);
    let dir = common::tmp_dir("throughput");
    let (a, b) = (dir.join("a.wav"), dir.join("b.wav"));
    let t = Instant::now();
    write_wav_f32(&a, &data).unwrap();
    let bulk_write = t.elapsed();
    let t = Instant::now();
    std::fs::write(&b, hound_bytes(&data)).unwrap();
    let slow_write = t.elapsed();
    let t = Instant::now();
    let back = read_wav_f32(&a).unwrap();
    let bulk_read = t.elapsed();
    let t = Instant::now();
    let mut r = hound::WavReader::open(&b).unwrap();
    let mut slow = vec![Vec::new(); 2];
    for (i, s) in r.samples::<f32>().enumerate() {
        slow[i % 2].push(s.unwrap());
    }
    let slow_read = t.elapsed();
    assert_eq!(back.channels, slow);
    println!(
        "write: bulk {bulk_write:?} vs {slow_write:?}; read: bulk {bulk_read:?} vs {slow_read:?}"
    );
}
