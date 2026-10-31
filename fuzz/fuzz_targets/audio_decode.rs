//! Audio files: the whole-file decoder (WAV, AIFF, FLAC, MP3 and Ogg through symphonia), the WAV
//! reader and the seekable streaming reader, and the progressive decoder of previews. The first
//! byte picks the reader and the container hint, the rest is the file.
#![no_main]

use std::path::PathBuf;
use std::sync::OnceLock;

use atomik_audio_file::{StreamDecoder, WavStreamReader, decode_bytes_with, read_wav_f32_limited};
use libfuzzer_sys::fuzz_target;

/// Memory stays small however the input lies: 2^22 samples is 16 MiB of `f32`.
const MAX_SAMPLES: u64 = 1 << 22;

const HINTS: [Option<&str>; 7] = [
    None,
    Some("wav"),
    Some("flac"),
    Some("mp3"),
    Some("ogg"),
    Some("aif"),
    Some("bin"),
];

/// A scratch file for the readers that take a path (one fuzzing process, one input at a time).
fn scratch(ext: &str) -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    let dir = DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("atomik-fuzz-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch folder");
        dir
    });
    dir.join(format!("input.{ext}"))
}

fuzz_target!(|data: &[u8]| {
    let Some((&which, bytes)) = data.split_first() else {
        return;
    };
    let hint = HINTS[usize::from(which >> 4) % HINTS.len()];
    match which & 0x0F {
        0..=7 => {
            if let Ok(decoded) = decode_bytes_with(bytes, hint, MAX_SAMPLES) {
                assert!(
                    decoded
                        .audio
                        .channels
                        .iter()
                        .flatten()
                        .all(|s| s.is_finite())
                );
            }
        }
        8..=10 => {
            let path = scratch("wav");
            if std::fs::write(&path, bytes).is_err() {
                return;
            }
            let _ = read_wav_f32_limited(&path, MAX_SAMPLES);
            if let Ok(mut reader) = WavStreamReader::open(&path) {
                let mut out = vec![vec![0.0f32; 256]; reader.channel_count()];
                let frames = reader.frames();
                for start in [0, frames / 2, frames.saturating_sub(100), frames] {
                    let _ = reader.read_frames(start, &mut out);
                }
            }
        }
        _ => {
            let path = scratch(hint.unwrap_or("bin"));
            if std::fs::write(&path, bytes).is_err() {
                return;
            }
            if let Ok(mut decoder) = StreamDecoder::open(&path) {
                // A few chunks are enough: the point is the first packets of a hostile file.
                for _ in 0..8 {
                    match decoder.next_chunk(4096) {
                        Ok(Some(_)) => {}
                        Ok(None) | Err(_) => break,
                    }
                }
            }
        }
    }
});
