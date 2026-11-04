use super::*;
use crate::wav::{read_wav, write_wav_with};
use std::io::Cursor;

fn flac(channels: &[Vec<f32>], rate: u32, format: SampleFormat, dither: DitherMode) -> Vec<u8> {
    let mut out = Vec::new();
    write_flac(&mut out, channels, rate, format, (dither, 7)).expect("encodes");
    out
}

/// A tiny deterministic noise source in -1..1.
fn noise(len: usize, seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 40) as f32 / (1u64 << 23) as f32) - 1.0
        })
        .collect()
}

fn sine(len: usize, hz: f32, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin())
        .collect()
}

/// Decodes `bytes` with the project's reader and checks the audio against the WAV of the same
/// audio, dither and seed: the very same codes, bit for bit.
fn assert_round_trip(channels: &[Vec<f32>], format: SampleFormat, dither: DitherMode) -> usize {
    let bytes = flac(channels, 48_000, format, dither);
    let decoded = atomik_audio_file::decode_bytes(&bytes, Some("flac")).expect("decodes");
    let mut wav = Cursor::new(Vec::new());
    write_wav_with(&mut wav, channels, 48_000, format, (dither, 7)).unwrap();
    let wav = read_wav(Cursor::new(wav.into_inner())).unwrap();
    assert_eq!(decoded.sample_rate, 48_000);
    assert_eq!(decoded.channels.len(), channels.len());
    for (c, (got, want)) in decoded.channels.iter().zip(&wav.channels).enumerate() {
        assert_eq!(got.len(), want.len(), "channel {c} length");
        let bad = got
            .iter()
            .zip(want)
            .position(|(a, b)| a.to_bits() != b.to_bits());
        assert_eq!(bad, None, "channel {c} differs");
    }
    bytes.len()
}

#[test]
fn the_digest_is_md5() {
    let digest = |text: &[u8]| {
        let mut md5 = Md5::new();
        md5.update(text);
        md5.finish()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    assert_eq!(digest(b""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(digest(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        digest(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"),
        "57edf4a22be3c955ac49da2e2107b67a"
    );
}

#[test]
fn the_constants_are_the_sines() {
    for (i, k) in Md5::K.iter().enumerate() {
        let want = ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32;
        assert_eq!(*k, want, "round {i}");
    }
}

#[test]
fn the_checksums_match_known_values() {
    // The CRC-8 and CRC-16 check values of the "123456789" string (CRC-8/SMBUS, CRC-16/UMTS).
    assert_eq!(crc8(b"123456789"), 0xF4);
    assert_eq!(crc16(b"123456789"), 0xFEE8);
}

#[test]
fn frame_numbers_use_the_utf8_form() {
    let encoded = |n: u64| {
        let mut out = Vec::new();
        put_frame_number(&mut out, n);
        out
    };
    assert_eq!(encoded(0), [0]);
    assert_eq!(encoded(127), [127]);
    assert_eq!(encoded(128), [0xC2, 0x80]);
    assert_eq!(encoded(0x7FF), [0xDF, 0xBF]);
    assert_eq!(encoded(0x800), [0xE0, 0xA0, 0x80]);
    assert_eq!(encoded(0x1_0000), [0xF0, 0x90, 0x80, 0x80]);
}

#[test]
fn every_kind_of_signal_round_trips_bit_exactly() {
    let len = 3 * BLOCK + 123;
    let signals: Vec<(&str, Vec<f32>)> = vec![
        ("silence", vec![0.0; len]),
        ("dc", vec![0.25; len]),
        ("sine", sine(len, 440.0, 0.8)),
        ("loud noise", noise(len, 1)),
        (
            "quiet noise",
            noise(len, 2).iter().map(|x| x * 1e-3).collect(),
        ),
        ("clipped", sine(len, 100.0, 4.0)),
        (
            "square",
            (0..len)
                .map(|i| if (i / 50) % 2 == 0 { 1.0 } else { -1.0 })
                .collect(),
        ),
        (
            "impulses",
            (0..len)
                .map(|i| if i % 997 == 0 { 0.9 } else { 0.0 })
                .collect(),
        ),
    ];
    for (_name, left) in &signals {
        for format in [SampleFormat::Pcm16, SampleFormat::Pcm24] {
            for dither in [DitherMode::None, DitherMode::Tpdf, DitherMode::Shaped] {
                let right: Vec<f32> = left.iter().rev().copied().collect();
                assert_round_trip(&[left.clone(), right], format, dither);
            }
        }
    }
}

#[test]
fn block_edges_and_tiny_streams_round_trip() {
    for len in [
        1,
        2,
        3,
        4,
        5,
        15,
        16,
        17,
        BLOCK - 1,
        BLOCK,
        BLOCK + 1,
        2 * BLOCK,
        2 * BLOCK + 1,
    ] {
        let mono = sine(len, 1_000.0, 0.5);
        assert_round_trip(
            std::slice::from_ref(&mono),
            SampleFormat::Pcm16,
            DitherMode::Tpdf,
        );
        assert_round_trip(
            &[mono.clone(), noise(len, 3)],
            SampleFormat::Pcm24,
            DitherMode::None,
        );
    }
}

#[test]
fn stereo_decorrelation_and_other_channel_counts_round_trip() {
    let len = 2 * BLOCK + 7;
    let s = sine(len, 220.0, 0.7);
    let n = noise(len, 4);
    // The same signal in both channels (mid/side wins), one channel only (left or right wins),
    // opposite polarity, and unrelated channels.
    assert_round_trip(
        &[s.clone(), s.clone()],
        SampleFormat::Pcm24,
        DitherMode::None,
    );
    assert_round_trip(
        &[s.clone(), vec![0.0; len]],
        SampleFormat::Pcm16,
        DitherMode::Tpdf,
    );
    assert_round_trip(
        &[vec![0.0; len], s.clone()],
        SampleFormat::Pcm16,
        DitherMode::Tpdf,
    );
    let inverted: Vec<f32> = s.iter().map(|x| -x).collect();
    assert_round_trip(
        &[s.clone(), inverted],
        SampleFormat::Pcm24,
        DitherMode::None,
    );
    assert_round_trip(
        &[s.clone(), n.clone()],
        SampleFormat::Pcm24,
        DitherMode::None,
    );
    // A 24-bit side channel needs 25 bits: full-scale opposite samples.
    let loud: Vec<f32> = (0..len)
        .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
        .collect();
    let anti: Vec<f32> = loud.iter().map(|x| -x).collect();
    assert_round_trip(&[loud, anti], SampleFormat::Pcm24, DitherMode::None);
    for count in [1, 3, 4, 6, 8] {
        let channels: Vec<Vec<f32>> = (0..count)
            .map(|c| noise(BLOCK + 50, 10 + c as u64))
            .collect();
        assert_round_trip(&channels, SampleFormat::Pcm16, DitherMode::Tpdf);
    }
}

#[test]
fn the_stream_info_and_the_frames_describe_the_file() {
    let len = 2 * BLOCK + 100;
    let bytes = flac(
        &[sine(len, 300.0, 0.5), sine(len, 301.0, 0.5)],
        44_100,
        SampleFormat::Pcm24,
        DitherMode::None,
    );
    assert_eq!(&bytes[..4], b"fLaC");
    // The last (only) metadata block is the stream info, 34 bytes long.
    assert_eq!(bytes[4], 0x80);
    assert_eq!(&bytes[5..8], &[0, 0, 34]);
    let info = &bytes[8..42];
    assert_eq!(u16::from_be_bytes([info[0], info[1]]), 4096);
    assert_eq!(u16::from_be_bytes([info[2], info[3]]), 4096);
    let min_frame = u32::from_be_bytes([0, info[4], info[5], info[6]]) as usize;
    let max_frame = u32::from_be_bytes([0, info[7], info[8], info[9]]) as usize;
    assert!(min_frame > 0 && min_frame <= max_frame);
    let packed = u64::from_be_bytes([
        info[10], info[11], info[12], info[13], info[14], info[15], info[16], info[17],
    ]);
    assert_eq!(packed >> 44, 44_100);
    assert_eq!((packed >> 41) & 7, 1);
    assert_eq!((packed >> 36) & 31, 23);
    assert_eq!(packed & ((1 << 36) - 1), len as u64);
    // The digest is the MD5 of the little-endian 24-bit interleaved samples.
    let decoded = atomik_audio_file::decode_bytes(&bytes, Some("flac")).unwrap();
    let mut md5 = Md5::new();
    for n in 0..len {
        for c in &decoded.channels {
            let code = (f64::from(c[n]) * 8_388_608.0).round() as i32;
            md5.update(&code.to_le_bytes()[..3]);
        }
    }
    assert_eq!(&info[18..], &md5.finish());
    // Every frame starts with the sync code and its CRC-16 holds.
    let mut at = 42;
    let mut frames = 0;
    while at < bytes.len() {
        assert_eq!(&bytes[at..at + 2], &[0xFF, 0xF8], "frame {frames}");
        let next = (at + min_frame - 2..=(at + max_frame - 2).min(bytes.len() - 2))
            .find(|&end| {
                crc16(&bytes[at..end]).to_be_bytes() == bytes[end..end + 2]
                    && (end + 2 == bytes.len() || bytes[end + 2..end + 4] == [0xFF, 0xF8])
            })
            .expect("a frame ends where its CRC says");
        at = next + 2;
        frames += 1;
    }
    assert_eq!(frames, 3);
}

#[test]
fn the_same_audio_gives_the_same_bytes() {
    let left = noise(3 * BLOCK, 5);
    let right = sine(3 * BLOCK, 90.0, 0.6);
    let a = flac(
        &[left.clone(), right.clone()],
        48_000,
        SampleFormat::Pcm16,
        DitherMode::Tpdf,
    );
    let b = flac(
        &[left, right],
        48_000,
        SampleFormat::Pcm16,
        DitherMode::Tpdf,
    );
    assert_eq!(a, b);
}

#[test]
fn tonal_music_is_smaller_than_the_wav_and_silence_is_tiny() {
    let len = 20 * BLOCK;
    let wav_bytes = len * 2 * 3;
    let tone = sine(len, 196.0, 0.5);
    let size = assert_round_trip(&[tone.clone(), tone], SampleFormat::Pcm24, DitherMode::None);
    assert!(size * 2 < wav_bytes, "{size} bytes against {wav_bytes}");
    let silent = assert_round_trip(
        &[vec![0.0; len], vec![0.0; len]],
        SampleFormat::Pcm24,
        DitherMode::None,
    );
    assert!(silent < 1_200, "{silent} bytes of silence");
    // Full-scale noise cannot shrink, and costs only the frame overhead.
    let loud = assert_round_trip(
        &[noise(len, 8), noise(len, 9)],
        SampleFormat::Pcm16,
        DitherMode::None,
    );
    assert!(loud < len * 2 * 2 + len / 8, "{loud} bytes of noise");
}

#[test]
fn bad_input_is_refused() {
    let mut out = Vec::new();
    let none: Vec<Vec<f32>> = Vec::new();
    let both = (DitherMode::None, 0);
    assert!(matches!(
        write_flac(&mut out, &none, 48_000, SampleFormat::Pcm16, both),
        Err(WavError::NoChannels)
    ));
    assert!(matches!(
        write_flac(
            &mut out,
            &[vec![0.0; 3], vec![0.0; 2]],
            48_000,
            SampleFormat::Pcm16,
            both
        ),
        Err(WavError::UnequalChannels)
    ));
    assert!(matches!(
        write_flac(&mut out, &[vec![0.0; 3]], 48_000, SampleFormat::F32, both),
        Err(WavError::Unsupported(_))
    ));
    let nine = vec![vec![0.0; 3]; 9];
    assert!(matches!(
        write_flac(&mut out, &nine, 48_000, SampleFormat::Pcm16, both),
        Err(WavError::Unsupported(_))
    ));
    assert!(matches!(
        write_flac(&mut out, &[vec![0.0; 3]], 0, SampleFormat::Pcm16, both),
        Err(WavError::Unsupported(_))
    ));
}

#[test]
fn an_empty_stream_is_a_valid_header() {
    let bytes = flac(
        &[Vec::new(), Vec::new()],
        48_000,
        SampleFormat::Pcm16,
        DitherMode::None,
    );
    assert_eq!(bytes.len(), 42);
}
