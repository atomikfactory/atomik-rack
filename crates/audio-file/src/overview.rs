//! A small picture of a file's loudness over time, for the browser's info pane: one level per
//! bucket, read by streaming the file so that memory stays constant whatever its length.

use crate::error::Result;
use crate::stream_decode::StreamDecoder;
use std::path::Path;

/// Frames per block of the first pass (blocks are folded into the buckets afterwards).
const BLOCK_FRAMES: usize = 4_096;
/// Most blocks kept (about 13 hours at 48 kHz); a longer file is read to there.
const MAX_BLOCKS: usize = 12_000;

/// The loudest sample level (0 to 1, all channels together) in each of `buckets` equal parts
/// of the file's first `max_frames` frames. A file that ends early gives fewer than `buckets`
/// parts, never an empty part; a silent or empty file gives zeros or an empty list. The
/// stream is read like a preview is: a damaged tail ends it.
///
/// # Errors
/// The file cannot be opened or decoded from the start.
pub fn overview_levels(path: &Path, buckets: usize, max_frames: u64) -> Result<Vec<f32>> {
    let mut decoder = StreamDecoder::open(path)?;
    let mut blocks: Vec<f32> = Vec::new();
    let mut level = 0.0f32;
    let mut in_block = 0usize;
    let mut frames = 0u64;
    'read: while let Some(chunk) = decoder.next_chunk(BLOCK_FRAMES)? {
        let len = chunk.first().map_or(0, Vec::len);
        for i in 0..len {
            if frames >= max_frames {
                break 'read;
            }
            for channel in &chunk {
                level = level.max(channel[i].abs());
            }
            in_block += 1;
            frames += 1;
            if in_block == BLOCK_FRAMES {
                blocks.push(finite(level));
                level = 0.0;
                in_block = 0;
                if blocks.len() == MAX_BLOCKS {
                    break 'read;
                }
            }
        }
    }
    if in_block > 0 {
        blocks.push(finite(level));
    }
    Ok(fold(&blocks, buckets))
}

fn finite(level: f32) -> f32 {
    if level.is_finite() {
        level.min(1.0)
    } else {
        0.0
    }
}

/// `blocks` folded into at most `buckets` levels, each the maximum of its share of the blocks.
fn fold(blocks: &[f32], buckets: usize) -> Vec<f32> {
    if blocks.is_empty() || buckets == 0 {
        return Vec::new();
    }
    let parts = buckets.min(blocks.len());
    (0..parts)
        .map(|k| {
            let from = k * blocks.len() / parts;
            let to = ((k + 1) * blocks.len() / parts).max(from + 1);
            blocks[from..to].iter().copied().fold(0.0, f32::max)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioData, write_wav_f32};

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("atomik-overview-{name}-{}.wav", std::process::id()))
    }

    #[test]
    fn folding_keeps_the_loudest_block_of_each_part_and_never_makes_an_empty_part() {
        assert_eq!(fold(&[0.1, 0.5, 0.2, 0.9], 2), vec![0.5, 0.9]);
        assert_eq!(fold(&[0.1, 0.5, 0.2], 8), vec![0.1, 0.5, 0.2]);
        assert_eq!(fold(&[0.3; 10], 4).len(), 4);
        assert!(fold(&[], 4).is_empty() && fold(&[0.5], 0).is_empty());
    }

    #[test]
    fn a_file_with_a_loud_second_half_shows_it_in_the_levels() {
        let frames = BLOCK_FRAMES * 40;
        let mut left = vec![0.1f32; frames];
        let mut right = vec![0.0f32; frames];
        for s in left.iter_mut().skip(frames / 2) {
            *s = 0.8;
        }
        right[10] = -0.4;
        let path = scratch("loud");
        let data = AudioData::new(48_000, vec![left, right]).unwrap();
        write_wav_f32(&path, &data).unwrap();
        let levels = overview_levels(&path, 20, u64::MAX).unwrap();
        assert_eq!(levels.len(), 20);
        assert!(
            levels[..10]
                .iter()
                .all(|l| (*l - 0.1).abs() < 1e-6 || (*l - 0.4).abs() < 1e-6)
        );
        assert!(
            levels[10..].iter().all(|l| (*l - 0.8).abs() < 1e-6),
            "{levels:?}"
        );
        // The frame limit cuts the read: only the quiet half is seen.
        let cut = overview_levels(&path, 20, (frames / 2) as u64).unwrap();
        assert!(cut.iter().all(|l| *l < 0.5), "{cut:?}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_file_that_is_not_audio_is_an_error() {
        let path = scratch("bad");
        std::fs::write(&path, b"not audio at all").unwrap();
        assert!(overview_levels(&path, 10, 1000).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
