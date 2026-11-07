//! Energy features of a rendered mix: how loud, how bright and how wide a stretch of it is.
//!
//! It runs on the UI side's workers from rendered audio, never on the audio thread, so nothing here
//! has to be real-time safe. It reads the band levels of [`crate::masking::analyse`] (the in-house
//! radix-2 transform, deterministic sine, exponential and logarithm) and plain sums of squares, so
//! the same audio gives the same numbers on every platform to within the rounding of one `f32`
//! sum.
//!
//! [`analyse`] turns a stereo mix into one [`Block`] per [`HOP`] frames. [`Blocks::features`] then
//! reads a range of frames (a section of the song) as [`Features`]:
//!
//! * **Loudness.** The power of every band of the block summed, then the mean over the blocks whose
//!   centre lies in the range, in dB. A full-scale sine reads 0 dB; the bands run from 40 Hz to
//!   about 18 kHz, so a rumble below that does not count.
//! * **Brightness.** The power-weighted mean of the band positions (a centroid on the log
//!   frequency scale, so it is not dragged up by the width of the top bands), in Hz.
//! * **Width.** The share of the side signal, `sqrt(S / (M + S))` with `M` and `S` the mean squares
//!   of `(L + R) / 2` and `(L - R) / 2`: 0 for a mono mix, about 0.71 for two unrelated channels
//!   of equal level, 1 for channels in opposite phase.
//!
//! A range with no block (shorter than half a block and between two centres) reads the nearest
//! block, so a very short section still has a value; an empty mix reads [`SILENT_DB`] and 0.

use crate::det_math::exp2_det_f64;
use crate::masking::{
    BANDS, BandMap, FIRST_HZ, HOP, SILENT_DB, analyse as band_map, db_to_power, power_to_db,
};

/// The measurements of one block of [`HOP`] frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Block {
    /// The summed power of the bands (linear; a full-scale sine in one band is 1).
    pub power: f64,
    /// The sum over the bands of `power * band`, the numerator of the centroid.
    pub moment: f64,
    /// The mean square of the mid signal `(L + R) / 2`.
    pub mid: f64,
    /// The mean square of the side signal `(L - R) / 2`.
    pub side: f64,
}

/// What a range of the mix measures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Features {
    /// The loudness in dB ([`SILENT_DB`] for silence).
    pub loudness_db: f32,
    /// The brightness in Hz (the centroid of the band power; 0 for silence).
    pub brightness_hz: f32,
    /// The width, 0 (mono) to 1 (opposite phase).
    pub width: f32,
}

impl Features {
    /// What an empty range reads.
    pub const SILENT: Self = Self {
        loudness_db: SILENT_DB,
        brightness_hz: 0.0,
        width: 0.0,
    };
}

/// The measurements of a whole mix, a [`Block`] per [`HOP`] frames.
#[derive(Debug, Clone, PartialEq)]
pub struct Blocks {
    /// Sample rate of the analysed audio.
    pub rate: u32,
    /// One entry per block.
    pub rows: Vec<Block>,
}

/// The centre frame of block `i`.
fn centre(i: usize) -> u64 {
    (i * HOP + HOP / 2) as u64
}

/// The blocks of a band map: the power and its moment over the bands.
fn power_rows(map: &BandMap) -> Vec<(f64, f64)> {
    map.levels
        .iter()
        .map(|row| {
            let mut power = 0.0;
            let mut moment = 0.0;
            for (band, level) in row.iter().enumerate().take(BANDS) {
                let p = db_to_power(*level);
                power += p;
                moment += p * band as f64;
            }
            (power, moment)
        })
        .collect()
}

/// The mean squares of mid and side over the frames of block `i`.
fn mid_side(left: &[f32], right: &[f32], i: usize) -> (f64, f64) {
    let from = (i * HOP).min(left.len());
    let to = ((i + 1) * HOP).min(left.len());
    let len = to - from;
    if len == 0 {
        return (0.0, 0.0);
    }
    let (mut mid, mut side) = (0.0f64, 0.0f64);
    for k in from..to {
        let l = f64::from(finite(left[k]));
        let r = if right.is_empty() {
            l
        } else {
            f64::from(finite(right[k]))
        };
        let m = (l + r) * 0.5;
        let s = (l - r) * 0.5;
        mid += m * m;
        side += s * s;
    }
    (mid / len as f64, side / len as f64)
}

fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// The blocks of a stereo mix (see the module docs). `right` may be empty for a mono mix; the two
/// are read up to the shorter one when both are given.
pub fn analyse(left: &[f32], right: &[f32], rate: u32) -> Blocks {
    let len = if right.is_empty() {
        left.len()
    } else {
        left.len().min(right.len())
    };
    let (left, right) = (
        &left[..len],
        if right.is_empty() {
            right
        } else {
            &right[..len]
        },
    );
    let map = band_map(left, right, rate);
    let rows = power_rows(&map)
        .into_iter()
        .enumerate()
        .map(|(i, (power, moment))| {
            let (mid, side) = mid_side(left, right, i);
            Block {
                power,
                moment,
                mid,
                side,
            }
        })
        .collect();
    Blocks { rate, rows }
}

impl Blocks {
    /// The blocks whose centre frame lies in `[from, to)`; the block nearest the range when none
    /// does and there are blocks at all.
    fn in_range(&self, from: u64, to: u64) -> &[Block] {
        if self.rows.is_empty() || to <= from {
            return &[];
        }
        let first = (0..self.rows.len()).find(|&i| centre(i) >= from);
        let Some(first) = first else {
            return &self.rows[self.rows.len() - 1..];
        };
        let count = self.rows[first..]
            .iter()
            .enumerate()
            .take_while(|(k, _)| centre(first + k) < to)
            .count();
        if count == 0 {
            // Nearest block: the one before `from` or the first after, whichever is closer.
            let nearest = if first > 0 && from - centre(first - 1) <= centre(first) - from {
                first - 1
            } else {
                first
            };
            return &self.rows[nearest..=nearest];
        }
        &self.rows[first..first + count]
    }

    /// What the frames `[from, to)` of the mix measure (see the module docs).
    pub fn features(&self, from: u64, to: u64) -> Features {
        let rows = self.in_range(from, to);
        if rows.is_empty() {
            return Features::SILENT;
        }
        let n = rows.len() as f64;
        let power: f64 = rows.iter().map(|b| b.power).sum();
        let moment: f64 = rows.iter().map(|b| b.moment).sum();
        let mid: f64 = rows.iter().map(|b| b.mid).sum();
        let side: f64 = rows.iter().map(|b| b.side).sum();
        let loudness_db = power_to_db(power / n);
        let brightness_hz = if power > 0.0 {
            (FIRST_HZ * exp2_det_f64(moment / power / 3.0)) as f32
        } else {
            0.0
        };
        let width = if mid + side > 0.0 {
            (side / (mid + side)).sqrt() as f32
        } else {
            0.0
        };
        Features {
            loudness_db,
            brightness_hz,
            width,
        }
    }
}

#[cfg(test)]
mod tests;
