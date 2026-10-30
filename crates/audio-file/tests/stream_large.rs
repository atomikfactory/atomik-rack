//! Assets above 4 GiB stream: the file offsets of the streaming reader are 64-bit. The file is
//! made sparse, so the test costs time rather than space, but it is still run on its own
//! (`cargo test -p atomik-audio-file --test stream_large -- --ignored`, in the scheduled job).
#![allow(missing_docs)]

mod common;

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};

use atomik_audio_file::WavStreamReader;

const RATE: u32 = 96_000;
const CHANNELS: u16 = 2;
/// Three hours of 96 kHz stereo: 8.3 GB of samples, twice what a RIFF size field can say.
const FRAMES: u64 = 3 * 3600 * RATE as u64;

/// An RF64 header: the real size of the data lives in the `ds64` chunk, the 32-bit fields
/// hold `0xFFFF_FFFF`. Returns the header (the samples follow it).
fn rf64_header(frames: u64) -> Vec<u8> {
    let data_bytes = frames * u64::from(CHANNELS) * 4;
    let mut h = Vec::new();
    h.extend_from_slice(b"RF64");
    h.extend_from_slice(&u32::MAX.to_le_bytes());
    h.extend_from_slice(b"WAVE");
    h.extend_from_slice(b"ds64");
    h.extend_from_slice(&28u32.to_le_bytes());
    h.extend_from_slice(&(data_bytes + 100).to_le_bytes()); // RIFF size, not read
    h.extend_from_slice(&data_bytes.to_le_bytes());
    h.extend_from_slice(&frames.to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes()); // no table
    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&3u16.to_le_bytes());
    h.extend_from_slice(&CHANNELS.to_le_bytes());
    h.extend_from_slice(&RATE.to_le_bytes());
    h.extend_from_slice(&(RATE * u32::from(CHANNELS) * 4).to_le_bytes());
    h.extend_from_slice(&(CHANNELS * 4).to_le_bytes());
    h.extend_from_slice(&32u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&u32::MAX.to_le_bytes());
    h
}

fn put_frame(file: &mut File, start: u64, frame: u64, l: f32, r: f32) {
    file.seek(SeekFrom::Start(start + frame * 8)).unwrap();
    file.write_all(&l.to_le_bytes()).unwrap();
    file.write_all(&r.to_le_bytes()).unwrap();
}

#[test]
#[ignore = "makes an 8 GB sparse file; run by the scheduled job"]
fn a_three_hour_stereo_file_streams_past_4_gib() {
    let path = common::tmp_dir("stream-large").join("long.wav");
    let header = rf64_header(FRAMES);
    let start = header.len() as u64;
    let mut file = File::create(&path).unwrap();
    file.write_all(&header).unwrap();
    file.set_len(start + FRAMES * 8).unwrap();
    // Frames on both sides of the 4 GiB byte mark (frame 2^29) and the very last one.
    let marks = [
        (7, 0.5, -0.5),
        ((1 << 29) - 1, 0.25, -0.25),
        (1 << 29, 0.125, -0.125),
        (FRAMES - 1, 0.75, -0.75),
    ];
    for (frame, l, r) in marks {
        put_frame(&mut file, start, frame, l, r);
    }
    drop(file);

    let mut reader = WavStreamReader::open(&path).unwrap();
    assert_eq!(reader.frames(), FRAMES);
    assert_eq!((reader.sample_rate(), reader.channel_count()), (RATE, 2));
    // The order is not monotonic on purpose: forward past 4 GiB, back, then to the end.
    for (frame, l, r) in [marks[2], marks[0], marks[1], marks[3]] {
        // Two frames, the second of which is the mark.
        let mut out = vec![vec![9.0f32; 2]; 2];
        assert_eq!(
            reader.read_frames(frame - 1, &mut out).unwrap(),
            2,
            "frame {frame}"
        );
        assert_eq!((out[0][1], out[1][1]), (l, r), "frame {frame}");
    }
    let mut out = vec![vec![9.0f32; 16]; 2];
    assert_eq!(reader.read_frames(FRAMES, &mut out).unwrap(), 0);
    assert_eq!(reader.read_frames(FRAMES - 3, &mut out).unwrap(), 3);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn an_rf64_file_and_a_streaming_size_field_read_like_a_plain_one() {
    // Small, always-run version of the above: the header forms, not the size.
    let dir = common::tmp_dir("stream-rf64");
    let frames = 1000u64;
    let samples: Vec<f32> = (0..frames * 2).map(|i| i as f32 * 0.001).collect();
    let body: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut bytes = rf64_header(frames);
    bytes.extend_from_slice(&body);
    let rf64 = dir.join("rf64.wav");
    std::fs::write(&rf64, &bytes).unwrap();
    // A plain RIFF file whose data size field was left at its maximum by a streaming writer.
    let mut plain = bytes.clone();
    plain[..4].copy_from_slice(b"RIFF");
    let plain_path = dir.join("max-size.wav");
    // Without the ds64 chunk the size field cannot be believed: the file's own length rules.
    let ds64_end = 12 + 8 + 28;
    plain.drain(12..ds64_end);
    std::fs::write(&plain_path, &plain).unwrap();

    for path in [rf64, plain_path] {
        let mut r = WavStreamReader::open(&path).unwrap();
        assert_eq!(r.frames(), frames, "{}", path.display());
        let mut out = vec![vec![0.0f32; 10]; 2];
        assert_eq!(r.read_frames(500, &mut out).unwrap(), 10);
        assert_eq!(out[0][0], samples[1000]);
        assert_eq!(out[1][9], samples[1019]);
    }
}
