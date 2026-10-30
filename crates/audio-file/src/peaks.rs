//! Waveform overview data: min/max peaks per bucket, a multi-resolution pyramid and its
//! compact binary form (cached in the project bundle).

use crate::data::AudioData;
use crate::error::{AudioFileError, Result};

const MAGIC: &[u8; 4] = b"ATPK";
const VERSION: u32 = 1;
const MAX_LEVELS: u32 = 16;

fn invalid(msg: &str) -> AudioFileError {
    AudioFileError::InvalidData(format!("peaks: {msg}"))
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| invalid("size overflow"))?;
        let s = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| invalid("truncated"))?;
        self.pos = end;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}

/// Frames per bucket of the pyramid levels, finest first.
pub const PEAK_LEVELS: [u32; 4] = [256, 1024, 4096, 16384];

/// Frames per bucket of the finest pyramid level: below this many frames per pixel a view reads
/// the samples themselves (see [`raw_min_max`]) when it has them.
pub const FINEST_PEAK_FRAMES: u32 = PEAK_LEVELS[0];

/// Min and max of the samples of one channel over the frames `[from, to)` (fractional frames
/// round outwards, so a range inside one frame still reads that frame): the exact answer a peak
/// bucket would give at any resolution. Frames before 0 and after the end are not read.
///
/// The cost is the number of frames in the range: callers ask for one pixel column at a time,
/// a range of at most [`FINEST_PEAK_FRAMES`] frames. NaN samples are ignored. `None` when the
/// range holds no frame (empty, outside the channel, not a number or all NaN).
pub fn raw_min_max(samples: &[f32], from: f64, to: f64) -> Option<(f32, f32)> {
    let end = to.min(samples.len() as f64);
    if to.is_nan() || from.is_nan() || end <= 0.0 || from >= end {
        return None;
    }
    let first = from.max(0.0).floor() as usize;
    let last = (end.ceil() as usize).max(first + 1).min(samples.len());
    let (lo, hi) = samples
        .get(first..last)?
        .iter()
        .filter(|v| !v.is_nan())
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(*v), hi.max(*v))
        });
    (lo <= hi).then_some((lo, hi))
}

/// Min/max per bucket for every channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Peaks {
    /// Source frames covered by one bucket.
    pub frames_per_bucket: u32,
    /// Per channel, one `(min, max)` per bucket.
    pub channels: Vec<Vec<(f32, f32)>>,
}

/// Min/max peaks of `data`, `frames_per_bucket` source frames per bucket (the last bucket may
/// cover fewer).
///
/// NaN samples are ignored; a bucket with no non-NaN sample is `(0.0, 0.0)`.
pub fn compute_peaks(data: &AudioData, frames_per_bucket: u32) -> Result<Peaks> {
    data.validate()?;
    if frames_per_bucket == 0 {
        return Err(invalid("frames per bucket is zero"));
    }
    let channels = data
        .channels
        .iter()
        .map(|ch| {
            ch.chunks(frames_per_bucket as usize)
                .map(|chunk| {
                    let (lo, hi) = chunk
                        .iter()
                        .filter(|v| !v.is_nan())
                        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                            (lo.min(*v), hi.max(*v))
                        });
                    if lo > hi { (0.0, 0.0) } else { (lo, hi) }
                })
                .collect()
        })
        .collect();
    Ok(Peaks {
        frames_per_bucket,
        channels,
    })
}

impl Peaks {
    /// Merges every `factor` buckets into one (exact for min/max).
    pub fn coarsen(&self, factor: u32) -> Result<Peaks> {
        if factor == 0 {
            return Err(invalid("coarsen factor is zero"));
        }
        let frames_per_bucket = self
            .frames_per_bucket
            .checked_mul(factor)
            .ok_or_else(|| invalid("bucket size overflows"))?;
        let channels = self
            .channels
            .iter()
            .map(|ch| {
                ch.chunks(factor as usize)
                    .map(|group| {
                        group
                            .iter()
                            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), (a, b)| {
                                (lo.min(*a), hi.max(*b))
                            })
                    })
                    .collect()
            })
            .collect();
        Ok(Peaks {
            frames_per_bucket,
            channels,
        })
    }
}

/// Several peak resolutions of one clip, finest first.
#[derive(Debug, Clone, PartialEq)]
pub struct PeakPyramid {
    sample_rate: u32,
    frames: u64,
    levels: Vec<Peaks>,
}

impl PeakPyramid {
    /// Builds the [`PEAK_LEVELS`] pyramid: the finest level from the samples, the rest by merging.
    pub fn build(data: &AudioData) -> Result<Self> {
        let mut levels = vec![compute_peaks(data, PEAK_LEVELS[0])?];
        for pair in PEAK_LEVELS.windows(2) {
            let finer = levels.last().ok_or_else(|| invalid("no finest level"))?;
            let next = finer.coarsen(pair[1] / pair[0])?;
            levels.push(next);
        }
        Ok(Self {
            sample_rate: data.sample_rate,
            frames: data.frames() as u64,
            levels,
        })
    }
    /// Sample rate of the source audio.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    /// Frames of the source audio.
    pub fn frames(&self) -> u64 {
        self.frames
    }
    /// All levels, finest first.
    pub fn levels(&self) -> &[Peaks] {
        &self.levels
    }
    /// The coarsest level whose buckets are not larger than one pixel.
    pub fn level_for(&self, frames_per_pixel: f64) -> &Peaks {
        let mut best = &self.levels[0];
        for lvl in &self.levels {
            if f64::from(lvl.frames_per_bucket) <= frames_per_pixel {
                best = lvl;
            }
        }
        best
    }

    /// Serializes to the versioned little-endian format (see [`PeakPyramid::from_bytes`]).
    ///
    /// Layout: `"ATPK"`, `u32 version = 1`, `u32 sample_rate`, `u64 source_frames`,
    /// `u32 channels`, `u32 level_count`; then per level `u32 frames_per_bucket`,
    /// `u32 buckets_per_channel` and, channel by channel and bucket by bucket,
    /// `f32 min, f32 max`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let channels = self.levels.first().map_or(0, |l| l.channels.len());
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.extend_from_slice(&self.frames.to_le_bytes());
        out.extend_from_slice(&(channels as u32).to_le_bytes());
        out.extend_from_slice(&(self.levels.len() as u32).to_le_bytes());
        for lvl in &self.levels {
            out.extend_from_slice(&lvl.frames_per_bucket.to_le_bytes());
            let buckets = lvl.channels.first().map_or(0, Vec::len);
            out.extend_from_slice(&(buckets as u32).to_le_bytes());
            for (lo, hi) in lvl.channels.iter().flatten() {
                out.extend_from_slice(&lo.to_le_bytes());
                out.extend_from_slice(&hi.to_le_bytes());
            }
        }
        out
    }

    /// Parses and fully validates the serialized form.
    ///
    /// Rejects bad magic or version, truncation, trailing bytes, a zero channel or level count,
    /// non-increasing bucket sizes and bucket counts that do not match the source length, all
    /// before allocating anything proportional to a header field.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader { bytes, pos: 0 };
        if r.take(4)? != MAGIC {
            return Err(invalid("bad magic"));
        }
        if r.u32()? != VERSION {
            return Err(invalid("unsupported version"));
        }
        let sample_rate = r.u32()?;
        let frames = r.u64()?;
        let channels = r.u32()? as usize;
        let level_count = r.u32()?;
        if sample_rate == 0 || channels == 0 || channels > usize::from(u16::MAX) {
            return Err(invalid("bad sample rate or channel count"));
        }
        if level_count == 0 || level_count > MAX_LEVELS {
            return Err(invalid("bad level count"));
        }
        let mut levels: Vec<Peaks> = Vec::new();
        for _ in 0..level_count {
            let fpb = r.u32()?;
            let buckets = r.u32()? as usize;
            if fpb == 0 || levels.last().is_some_and(|l| l.frames_per_bucket >= fpb) {
                return Err(invalid("bucket sizes must be positive and increasing"));
            }
            if (buckets as u64) != frames.div_ceil(u64::from(fpb)) {
                return Err(invalid("bucket count does not match the source length"));
            }
            let per_channel = buckets
                .checked_mul(8)
                .ok_or_else(|| invalid("size overflow"))?;
            let need = per_channel
                .checked_mul(channels)
                .ok_or_else(|| invalid("size overflow"))?;
            let data = r.take(need)?;
            let mut chans = Vec::with_capacity(channels);
            for c in 0..channels {
                let raw = &data[c * per_channel..(c + 1) * per_channel];
                chans.push(
                    raw.as_chunks::<8>()
                        .0
                        .iter()
                        .map(|b| {
                            (
                                f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                                f32::from_le_bytes([b[4], b[5], b[6], b[7]]),
                            )
                        })
                        .collect(),
                );
            }
            levels.push(Peaks {
                frames_per_bucket: fpb,
                channels: chans,
            });
        }
        if r.pos != bytes.len() {
            return Err(invalid("trailing bytes"));
        }
        Ok(Self {
            sample_rate,
            frames,
            levels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AudioFileError;

    fn ramp(frames: usize) -> AudioData {
        let a: Vec<f32> = (0..frames)
            .map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0)
            .collect();
        let b: Vec<f32> = (0..frames)
            .map(|i| (i as f32 / frames as f32) * 2.0 - 1.0)
            .collect();
        AudioData::new(48_000, vec![a, b]).unwrap()
    }

    #[test]
    fn buckets_hold_exact_min_and_max() {
        let d = AudioData::new(8000, vec![vec![0.5, -0.25, 0.1, 0.9, -0.9, 0.0, 0.3]]).unwrap();
        let p = compute_peaks(&d, 3).unwrap();
        assert_eq!(p.frames_per_bucket, 3);
        assert_eq!(p.channels[0], [(-0.25, 0.5), (-0.9, 0.9), (0.3, 0.3)]);
    }

    #[test]
    fn bucket_count_is_ceil_and_edge_sizes_work() {
        for (frames, fpb, want) in [
            (0usize, 4u32, 0usize),
            (1, 4, 1),
            (4, 4, 1),
            (5, 4, 2),
            (1000, 1, 1000),
        ] {
            let d = AudioData::new(8000, vec![vec![0.25; frames]]).unwrap();
            assert_eq!(
                compute_peaks(&d, fpb).unwrap().channels[0].len(),
                want,
                "{frames}/{fpb}"
            );
        }
        let d = ramp(10);
        assert!(matches!(
            compute_peaks(&d, 0),
            Err(AudioFileError::InvalidData(_))
        ));
    }

    #[test]
    fn nan_samples_are_ignored_and_empty_buckets_are_zero() {
        // The constructor would turn the NaNs into silence; the public fields can still hold
        // them (a peak computation is defensive about it).
        let mut d = AudioData::new(8000, vec![vec![0.0, 0.5, 0.0, 0.0]]).unwrap();
        d.channels[0][0] = f32::NAN;
        d.channels[0][2] = f32::NAN;
        d.channels[0][3] = f32::NAN;
        let p = compute_peaks(&d, 2).unwrap();
        assert_eq!(p.channels[0], [(0.5, 0.5), (0.0, 0.0)]);
    }

    #[test]
    fn coarsen_equals_direct_computation() {
        let d = ramp(10_007);
        let fine = compute_peaks(&d, 100).unwrap();
        assert_eq!(fine.coarsen(7).unwrap(), compute_peaks(&d, 700).unwrap());
        assert_eq!(fine.coarsen(1).unwrap(), fine);
        assert!(fine.coarsen(0).is_err());
    }

    #[test]
    fn pyramid_levels_match_direct_peaks() {
        for frames in [1usize, 255, 256, 257, 5000, 70_001] {
            let d = ramp(frames);
            let py = PeakPyramid::build(&d).unwrap();
            assert_eq!(py.levels().len(), PEAK_LEVELS.len());
            assert_eq!((py.sample_rate(), py.frames()), (48_000, frames as u64));
            for (lvl, fpb) in py.levels().iter().zip(PEAK_LEVELS) {
                assert_eq!(
                    *lvl,
                    compute_peaks(&d, fpb).unwrap(),
                    "{frames} frames, level {fpb}"
                );
            }
        }
    }

    #[test]
    fn level_for_picks_the_coarsest_level_not_coarser_than_the_pixel() {
        let py = PeakPyramid::build(&ramp(100_000)).unwrap();
        assert_eq!(
            py.level_for(10.0).frames_per_bucket,
            256,
            "finer than finest: use finest"
        );
        assert_eq!(py.level_for(256.0).frames_per_bucket, 256);
        assert_eq!(py.level_for(1500.0).frames_per_bucket, 1024);
        assert_eq!(py.level_for(4096.0).frames_per_bucket, 4096);
        assert_eq!(py.level_for(1e9).frames_per_bucket, 16384);
        assert_eq!(py.level_for(f64::NAN).frames_per_bucket, 256);
    }

    #[test]
    fn raw_min_max_matches_a_peak_bucket_over_the_same_frames() {
        let d = ramp(2000);
        let peaks = compute_peaks(&d, 7).unwrap();
        for ch in 0..2 {
            for (b, want) in peaks.channels[ch].iter().enumerate() {
                let from = (b * 7) as f64;
                assert_eq!(
                    raw_min_max(&d.channels[ch], from, from + 7.0),
                    Some(*want),
                    "bucket {b}"
                );
            }
        }
    }

    #[test]
    fn raw_min_max_rounds_outwards_and_clips_to_the_samples() {
        let s = [0.1, 0.9, -0.5, 0.2, 0.3];
        assert_eq!(
            raw_min_max(&s, 1.2, 1.4),
            Some((0.9, 0.9)),
            "inside a frame"
        );
        assert_eq!(raw_min_max(&s, 1.9, 2.1), Some((-0.5, 0.9)), "two frames");
        assert_eq!(raw_min_max(&s, -10.0, 1.0), Some((0.1, 0.1)), "before 0");
        assert_eq!(raw_min_max(&s, 3.5, 99.0), Some((0.2, 0.3)), "past the end");
        assert_eq!(raw_min_max(&s, 5.0, 9.0), None);
        assert_eq!(raw_min_max(&s, -4.0, -1.0), None);
        assert_eq!(raw_min_max(&s, 2.0, 2.0), None, "empty range");
        assert_eq!(raw_min_max(&s, f64::NAN, 2.0), None);
        assert_eq!(raw_min_max(&s, 0.0, f64::NAN), None);
        assert_eq!(raw_min_max(&[], 0.0, 1.0), None);
        assert_eq!(
            raw_min_max(&[f32::NAN, 0.5, f32::NAN], 0.0, 3.0),
            Some((0.5, 0.5))
        );
        assert_eq!(raw_min_max(&[f32::NAN], 0.0, 1.0), None, "all NaN");
    }

    #[test]
    fn serialization_round_trips_bit_exactly() {
        for frames in [1usize, 300, 40_000] {
            let py = PeakPyramid::build(&ramp(frames)).unwrap();
            let bytes = py.to_bytes();
            assert_eq!(&bytes[..4], b"ATPK");
            assert_eq!(
                u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
                1,
                "version"
            );
            assert_eq!(PeakPyramid::from_bytes(&bytes).unwrap(), py);
            assert_eq!(PeakPyramid::from_bytes(&bytes).unwrap().to_bytes(), bytes);
        }
    }

    #[test]
    fn serialization_has_the_documented_layout() {
        let d = AudioData::new(44_100, vec![vec![0.5, -0.5]]).unwrap();
        let bytes = PeakPyramid::build(&d).unwrap().to_bytes();
        // header 28 bytes + 4 levels x (8 + 1 bucket x 8)
        assert_eq!(bytes.len(), 28 + 4 * (8 + 8));
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 44_100);
        assert_eq!(u64::from_le_bytes(bytes[12..20].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(bytes[20..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(bytes[28..32].try_into().unwrap()), 256);
        assert_eq!(u32::from_le_bytes(bytes[32..36].try_into().unwrap()), 1);
        assert_eq!(f32::from_le_bytes(bytes[36..40].try_into().unwrap()), -0.5);
        assert_eq!(f32::from_le_bytes(bytes[40..44].try_into().unwrap()), 0.5);
    }

    #[test]
    fn corrupt_input_is_rejected_without_panicking() {
        let good = PeakPyramid::build(&ramp(3000)).unwrap().to_bytes();
        for cut in 0..good.len() {
            assert!(
                PeakPyramid::from_bytes(&good[..cut]).is_err(),
                "truncated at {cut}"
            );
        }
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(PeakPyramid::from_bytes(&trailing).is_err());
        let mut bad_magic = good.clone();
        bad_magic[0] ^= 0xff;
        assert!(PeakPyramid::from_bytes(&bad_magic).is_err());
        let mut bad_version = good.clone();
        bad_version[4] = 2;
        assert!(PeakPyramid::from_bytes(&bad_version).is_err());
        let mut huge = good.clone();
        huge[24..28].copy_from_slice(&u32::MAX.to_le_bytes()); // level count
        assert!(PeakPyramid::from_bytes(&huge).is_err());
        let mut inconsistent = good.clone();
        inconsistent[28..32].copy_from_slice(&u32::MAX.to_le_bytes()); // level 0 bucket size
        assert!(PeakPyramid::from_bytes(&inconsistent).is_err());
        let mut zero_ch = good;
        zero_ch[20..24].copy_from_slice(&0u32.to_le_bytes());
        assert!(PeakPyramid::from_bytes(&zero_ch).is_err());
    }
}
