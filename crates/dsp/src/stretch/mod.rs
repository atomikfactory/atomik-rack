//! Time-stretch and pitch-shift of whole buffers, written in-house (no third-party code), for
//! the warped audio clips.
//!
//! [`stretch`] renders an output buffer from a source buffer through a [`TimeMap`] (which
//! source frame plays at each output frame) in one of three modes:
//!
//! * [`StretchMode::Repitch`]: the source read at the speed the map asks for with Hermite
//!   interpolation, so speed and pitch change together (the cheap default; a faster read
//!   aliases a little, it does not filter).
//! * [`StretchMode::Beats`]: transient-aware time stretch for drums and percussive loops, see
//!   the [`beats`] module: the onsets stay where the map puts them and keep their attacks, the
//!   audio between them is stretched with WSOLA.
//! * [`StretchMode::Tonal`]: a phase vocoder with identity phase locking for sustained and
//!   polyphonic material, see the [`tonal`] module.
//!
//! **Pitch shift** is a stretch plus a resample: a signal that is `r` times as long is stretched
//! first and then read `r` times as fast, so the pitch is multiplied by `r` and the length is
//! the map's. It applies to Beats and Tonal; Repitch changes pitch with speed only.
//!
//! Everything here is offline (it allocates), deterministic (the same input gives the same
//! bits on every platform: only IEEE `+ - * /`, `sqrt`, and the deterministic trigonometry of
//! this crate are used) and cancellable: the `cancel` callback is polled about every
//! thousandth of a second of audio, and a `true` makes the call return `None`.

mod beats;
pub(crate) mod fft;
mod map;
mod repitch;
mod tonal;

#[cfg(test)]
mod tests;

pub use map::TimeMap;

use repitch::{repitch, resample_by};

/// How [`stretch`] stretches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StretchMode {
    /// Speed and pitch change together.
    Repitch,
    /// Transient-preserving WSOLA, for percussive material.
    Beats,
    /// Phase vocoder with phase locking, for tonal and polyphonic material.
    Tonal,
}

/// Smallest pitch ratio (two octaves down).
pub const PITCH_RATIO_MIN: f64 = 0.25;
/// Largest pitch ratio (two octaves up).
pub const PITCH_RATIO_MAX: f64 = 4.0;

/// The frequency ratio of a transposition of `cents` (100 per semitone).
pub fn pitch_ratio_for_cents(cents: i32) -> f64 {
    crate::exp2_det_f64(f64::from(cents) / 1200.0).clamp(PITCH_RATIO_MIN, PITCH_RATIO_MAX)
}

/// What [`stretch`] does besides following the map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StretchParams {
    /// The mode.
    pub mode: StretchMode,
    /// The factor the pitch is multiplied by (1 for none; ignored by Repitch), clamped to
    /// [`PITCH_RATIO_MIN`]`..=`[`PITCH_RATIO_MAX`].
    pub pitch_ratio: f64,
    /// Sample rate of the audio.
    pub sample_rate: u32,
}

/// Renders `out_frames` frames: output frame `t` plays the source at `map.src_at(t)`. `source` is
/// one or two channels (more are ignored) of equal length; the result has as many channels as
/// were read. Source positions outside the source are silent. `None` when `cancel` returned
/// true.
pub fn stretch(
    source: &[&[f32]],
    map: &TimeMap,
    out_frames: usize,
    params: &StretchParams,
    cancel: &dyn Fn() -> bool,
) -> Option<Vec<Vec<f32>>> {
    let source: Vec<&[f32]> = source.iter().take(2).copied().collect();
    if params.mode == StretchMode::Repitch {
        return repitch(&source, map, out_frames, cancel);
    }
    let ratio = if params.pitch_ratio.is_finite() {
        params.pitch_ratio.clamp(PITCH_RATIO_MIN, PITCH_RATIO_MAX)
    } else {
        1.0
    };
    let engine = |map: &TimeMap, frames: usize| match params.mode {
        StretchMode::Beats => beats::beats(&source, map, frames, params.sample_rate, cancel),
        _ => tonal::tonal(&source, map, frames, params.sample_rate, cancel),
    };
    if (ratio - 1.0).abs() < 1.0e-9 {
        return engine(map, out_frames);
    }
    // A signal `ratio` times as long, read `ratio` times as fast.
    let long = (out_frames as f64 * ratio).ceil() as usize + 4;
    let stretched = engine(&map.scaled_out(ratio), long)?;
    Some(resample_by(&stretched, ratio, out_frames))
}
