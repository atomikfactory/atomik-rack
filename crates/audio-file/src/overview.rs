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

/// Most blocks the peak pass keeps when the file does not state its length.
const PEAK_MAX_BLOCKS: usize = 200_000;
/// Frames per block of the peak pass when the file does not state its length.
const PEAK_UNKNOWN_BLOCK: usize = 1_024;

/// The lowest and highest sample value (all channels together) of each of `buckets` equal
/// parts of the file's first `max_frames` frames, in time order: what a waveform picture
/// `buckets` columns wide draws. A file whose length is stated is split exactly, so a short
/// file gives one bucket per requested column (or one per frame when it has fewer frames than
/// columns); a file of unknown length is read in fixed blocks that are folded afterwards. A
/// silent file gives `(0, 0)` buckets, an empty one an empty list. Memory stays bounded
/// whatever the length: the read stops at `max_frames`.
///
/// # Errors
/// The file cannot be opened or decoded from the start.
pub fn overview_peaks(path: &Path, buckets: usize, max_frames: u64) -> Result<Vec<(f32, f32)>> {
    let mut decoder = StreamDecoder::open(path)?;
    let known = decoder.info().frames.map(|f| f.min(max_frames));
    let block = match known {
        Some(frames) if buckets > 0 => (frames as usize).div_ceil(buckets).max(1),
        _ => PEAK_UNKNOWN_BLOCK,
    };
    let mut blocks: Vec<(f32, f32)> = Vec::new();
    let (mut lo, mut hi) = (0.0f32, 0.0f32);
    let mut in_block = 0usize;
    let mut frames = 0u64;
    'read: while let Some(chunk) = decoder.next_chunk(block.clamp(256, 8_192))? {
        let len = chunk.first().map_or(0, Vec::len);
        for i in 0..len {
            if frames >= max_frames {
                break 'read;
            }
            for channel in &chunk {
                let v = channel[i];
                if v.is_finite() {
                    lo = lo.min(v.max(-1.0));
                    hi = hi.max(v.min(1.0));
                }
            }
            in_block += 1;
            frames += 1;
            if in_block == block {
                blocks.push((lo, hi));
                (lo, hi) = (0.0, 0.0);
                in_block = 0;
                if blocks.len() == PEAK_MAX_BLOCKS {
                    break 'read;
                }
            }
        }
    }
    if in_block > 0 {
        blocks.push((lo, hi));
    }
    Ok(fold_peaks(&blocks, buckets))
}

/// `blocks` folded into at most `buckets` ranges, each spanning its share of the blocks.
fn fold_peaks(blocks: &[(f32, f32)], buckets: usize) -> Vec<(f32, f32)> {
    if blocks.is_empty() || buckets == 0 {
        return Vec::new();
    }
    let parts = buckets.min(blocks.len());
    (0..parts)
        .map(|k| {
            let from = k * blocks.len() / parts;
            let to = ((k + 1) * blocks.len() / parts).max(from + 1);
            blocks[from..to]
                .iter()
                .fold((0.0f32, 0.0f32), |(lo, hi), &(l, h)| (lo.min(l), hi.max(h)))
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
    fn a_short_file_gives_one_peak_bucket_per_requested_column() {
        // Half a second at 48 kHz, asked for 1000 columns: every column is real data.
        let frames = 24_000;
        let left: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let path = scratch("short-peaks");
        write_wav_f32(&path, &AudioData::new(48_000, vec![left]).unwrap()).unwrap();
        let peaks = overview_peaks(&path, 1_000, u64::MAX).unwrap();
        assert_eq!(peaks.len(), 1_000);
        assert!(
            peaks
                .iter()
                .all(|(lo, hi)| lo <= hi && *hi <= 0.5001 && *lo >= -0.5001)
        );
        assert!(peaks.iter().any(|(_, hi)| *hi > 0.45) && peaks.iter().any(|(lo, _)| *lo < -0.45));
        // Fewer frames than columns: one bucket per frame, never an empty one.
        let tiny = scratch("tiny-peaks");
        write_wav_f32(&tiny, &AudioData::new(48_000, vec![vec![0.25; 7]]).unwrap()).unwrap();
        assert_eq!(overview_peaks(&tiny, 1_000, u64::MAX).unwrap().len(), 7);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&tiny);
    }

    #[test]
    fn peaks_find_an_impulse_in_its_own_column_and_merge_the_channels() {
        let mut left = vec![0.0f32; 1_000];
        let mut right = vec![0.0f32; 1_000];
        left[250] = 0.75;
        right[750] = -0.5;
        let path = scratch("impulse-peaks");
        write_wav_f32(&path, &AudioData::new(48_000, vec![left, right]).unwrap()).unwrap();
        let peaks = overview_peaks(&path, 10, u64::MAX).unwrap();
        assert_eq!(peaks.len(), 10);
        for (k, (lo, hi)) in peaks.iter().enumerate() {
            let want_hi = if k == 2 { 0.75 } else { 0.0 };
            let want_lo = if k == 7 { -0.5 } else { 0.0 };
            assert!(
                (hi - want_hi).abs() < 1e-6 && (lo - want_lo).abs() < 1e-6,
                "{k}: {peaks:?}"
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_peak_read_stops_at_the_frame_limit() {
        let mut left = vec![0.1f32; 20_000];
        for s in left.iter_mut().skip(10_000) {
            *s = 0.9;
        }
        let path = scratch("limit-peaks");
        write_wav_f32(&path, &AudioData::new(48_000, vec![left]).unwrap()).unwrap();
        let cut = overview_peaks(&path, 50, 10_000).unwrap();
        assert_eq!(cut.len(), 50);
        assert!(cut.iter().all(|(_, hi)| *hi < 0.5), "{cut:?}");
        assert!(overview_peaks(&path, 0, 10_000).unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn folding_peaks_spans_each_part() {
        let blocks = [(-0.1, 0.2), (-0.5, 0.1), (0.0, 0.9), (-0.2, 0.0)];
        assert_eq!(fold_peaks(&blocks, 2), vec![(-0.5, 0.2), (-0.2, 0.9)]);
        assert_eq!(fold_peaks(&blocks[..1], 8).len(), 1);
        assert!(fold_peaks(&[], 4).is_empty());
    }

    #[test]
    fn a_file_that_is_not_audio_is_an_error() {
        let path = scratch("bad");
        std::fs::write(&path, b"not audio at all").unwrap();
        assert!(overview_levels(&path, 10, 1000).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
