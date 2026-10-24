//! Audio to notes: offline extraction of notes from monophonic and drum audio.
//!
//! * [`extract_pitched_notes`] ([`pitched`]) follows the pitch of a single voice or instrument
//!   (YIN) and cuts it into notes.
//! * [`extract_drum_notes`] ([`drums`]) finds the hits of a drum loop and sorts each into kick,
//!   snare, closed hat or open hat on the General MIDI keys.
//!
//! Both are pure and deterministic and run off the real-time thread (they allocate). Same
//! samples, rate and parameters give the same notes on every platform: the decision path uses
//! only IEEE `+ - * /`, `sqrt`, integer arithmetic, sorting and the deterministic `log2`, `exp2`
//! and cosine of this crate. Silence, non-finite samples and noise give no notes and never `NaN`.
//!
//! Positions and lengths are in **frames** of the input; the caller turns them into ticks.

pub mod drums;
pub mod pitched;

pub use drums::{DrumParams, extract_drum_notes};
pub use pitched::{PitchParams, extract_pitched_notes};

use crate::det_math::log2_det;

/// One note found in the audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExtractedNote {
    /// First frame of the note.
    pub start: u64,
    /// Length in frames (at least 1).
    pub length: u64,
    /// MIDI key.
    pub key: u8,
    /// Velocity in `(0, 1]`.
    pub velocity: f32,
}

/// Range of levels, in dB below the loudest note, that velocity spreads over.
const VELOCITY_RANGE_DB: f32 = 24.0;
/// Velocity of the softest notes.
const MIN_VELOCITY: f32 = 0.2;

/// The mean of the channels (non-finite samples count as 0); the shortest channel sets the length.
pub(crate) fn mono_mix(channels: &[&[f32]]) -> Vec<f32> {
    let n = channels.iter().map(|c| c.len()).min().unwrap_or(0);
    let inv = 1.0 / channels.len().max(1) as f64;
    (0..n)
        .map(|i| {
            let mut sum = 0.0f64;
            for c in channels {
                let x = c[i];
                if x.is_finite() {
                    sum += f64::from(x);
                }
            }
            (sum * inv) as f32
        })
        .collect()
}

/// The velocity of a note of level `level` when the loudest note of the take has `reference`:
/// 1 for the loudest, falling linearly with the level in dB to [`MIN_VELOCITY`] at
/// [`VELOCITY_RANGE_DB`] below it.
pub(crate) fn velocity_for(level: f32, reference: f32) -> f32 {
    if level.is_nan() || reference.is_nan() || level <= 0.0 || reference <= 0.0 {
        return MIN_VELOCITY;
    }
    let db = 6.020_6 * log2_det(level / reference);
    (1.0 + db / VELOCITY_RANGE_DB).clamp(MIN_VELOCITY, 1.0)
}

/// Sensitivity clamped to `0..=1` (NaN gives 0.5).
pub(crate) fn clamp_sensitivity(s: f32) -> f32 {
    if s.is_nan() { 0.5 } else { s.clamp(0.0, 1.0) }
}

/// Mean power of 1 ms blocks, for cheap level readings anywhere in the take.
pub(crate) struct Levels {
    /// Frames per block.
    block: usize,
    /// The sum of the squares of each block.
    sums: Vec<f64>,
    len: usize,
}

impl Levels {
    pub(crate) fn new(x: &[f32], sample_rate: u32) -> Self {
        let block = (sample_rate as usize / 1000).max(1);
        let sums = x
            .chunks(block)
            .map(|c| c.iter().map(|v| f64::from(*v) * f64::from(*v)).sum())
            .collect();
        Self {
            block,
            sums,
            len: x.len(),
        }
    }

    /// Frames per block.
    pub(crate) fn block(&self) -> usize {
        self.block
    }

    /// RMS over the 21 blocks centred on the block that holds frame `pos` (at least 20 ms, so a
    /// low note still reads steadily).
    pub(crate) fn rms_at(&self, pos: u64) -> f32 {
        if self.sums.is_empty() {
            return 0.0;
        }
        let j = ((pos as usize) / self.block).min(self.sums.len() - 1);
        let lo = j.saturating_sub(10);
        let hi = (j + 10).min(self.sums.len() - 1);
        let mut acc = 0.0f64;
        for s in &self.sums[lo..=hi] {
            acc += s;
        }
        let frames = ((hi + 1) * self.block).min(self.len) - lo * self.block;
        (acc / frames.max(1) as f64).sqrt() as f32
    }
}

#[cfg(test)]
mod tests;
