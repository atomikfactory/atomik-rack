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
//! [`RtStretcher`] is the real-time companion (see the [`realtime`] module): a streaming,
//! allocation-free WSOLA that keeps the pitch and plays the transposition while the engine
//! previews a warped clip whose finished buffer is still being rendered. It sounds close to the
//! Beats and Tonal buffers (same pitch and timing, a little lower quality).
//!
//! **Formants.** The Tonal mode can keep the spectral envelope of the source where it was while
//! the pitch moves ([`StretchParams::keep_formants`]), and move the envelope on its own
//! ([`StretchParams::formant_ratio`]): a transposed chord or a whole clip then keeps its
//! timbre. It works on any material, where the [`psola`] module below needs a single voice.
//!
//! The [`psola`] module shifts the notes of monophonic audio one by one while keeping the
//! formants (TD-PSOLA): the pitch edits of the audio clip editor's Vocal mode, applied to the
//! asset before the time map.
//!
//! Everything else here is offline (it allocates), deterministic (the same input gives the same
//! bits on every platform: only IEEE `+ - * /`, `sqrt`, and the deterministic trigonometry of
//! this crate are used) and cancellable: the `cancel` callback is polled about every
//! thousandth of a second of audio, and a `true` makes the call return `None`.

mod beats;
pub(crate) mod fft;
mod formant;
mod map;
pub mod psola;
pub mod realtime;
mod repitch;
mod tonal;

#[cfg(test)]
mod formant_tests;
#[cfg(test)]
mod psola_tests;
#[cfg(test)]
mod realtime_tests;
#[cfg(test)]
mod tests;

pub use map::TimeMap;
pub use realtime::{RT_MAX_SAMPLE_RATE, RtStretcher};

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
    /// Tonal only: keep the spectral envelope (the formants) where it was while the pitch moves,
    /// see the `formant` module. Off leaves the envelope to move with the pitch.
    pub keep_formants: bool,
    /// Tonal with `keep_formants`: the factor the envelope itself is moved by (1 for none),
    /// clamped to [`FORMANT_RATIO_MIN`]`..=`[`FORMANT_RATIO_MAX`].
    pub formant_ratio: f64,
    /// Tonal only: both channels of a stereo source take the same phase rotation, so the image
    /// of the stretched clip is kept.
    pub link_stereo: bool,
}

impl StretchParams {
    /// Plain parameters: no formant work, channels phased on their own.
    pub fn new(mode: StretchMode, pitch_ratio: f64, sample_rate: u32) -> Self {
        Self {
            mode,
            pitch_ratio,
            sample_rate,
            keep_formants: false,
            formant_ratio: 1.0,
            link_stereo: false,
        }
    }
}

/// Smallest formant ratio (an octave down).
pub const FORMANT_RATIO_MIN: f64 = 0.5;
/// Largest formant ratio (an octave up).
pub const FORMANT_RATIO_MAX: f64 = 2.0;

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
    // A sample that is not a number would poison every frame it falls in: it is silence.
    let dirty = source.iter().any(|c| c.iter().any(|v| !v.is_finite()));
    let owned: Vec<Vec<f32>> = if dirty {
        source
            .iter()
            .map(|c| {
                c.iter()
                    .map(|v| if v.is_finite() { *v } else { 0.0 })
                    .collect()
            })
            .collect()
    } else {
        Vec::new()
    };
    let source: Vec<&[f32]> = if dirty {
        owned.iter().map(Vec::as_slice).collect()
    } else {
        source
    };
    if params.mode == StretchMode::Repitch {
        return repitch(&source, map, out_frames, cancel);
    }
    let ratio = if params.pitch_ratio.is_finite() {
        params.pitch_ratio.clamp(PITCH_RATIO_MIN, PITCH_RATIO_MAX)
    } else {
        1.0
    };
    // The resample moves the envelope by `ratio`: the engine puts it at `ratio / formant_ratio`
    // times its place first so it ends up where it started (or at the formant shift).
    let formant = if params.formant_ratio.is_finite() {
        params
            .formant_ratio
            .clamp(FORMANT_RATIO_MIN, FORMANT_RATIO_MAX)
    } else {
        1.0
    };
    let options = tonal::TonalOptions {
        env_scale: if params.keep_formants {
            ratio / formant
        } else {
            1.0
        },
        link_stereo: params.link_stereo,
    };
    let engine = |map: &TimeMap, frames: usize| match params.mode {
        StretchMode::Beats => beats::beats(&source, map, frames, params.sample_rate, cancel),
        _ => tonal::tonal(&source, map, frames, params.sample_rate, options, cancel),
    };
    if (ratio - 1.0).abs() < 1.0e-9 {
        return engine(map, out_frames);
    }
    // A signal `ratio` times as long, read `ratio` times as fast.
    let long = (out_frames as f64 * ratio).ceil() as usize + 4;
    let stretched = engine(&map.scaled_out(ratio), long)?;
    Some(resample_by(&stretched, ratio, out_frames))
}
