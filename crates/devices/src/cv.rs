//! Shared helpers for the CV inputs added to the instruments (`<target>_cv_in`).
//!
//! Convention (see `docs/EXTENDING.md`, "Amount and unit convention"): the CV sample is clamped
//! to -1..+1 (NaN counts as 0), multiplied by the bipolar amount param of its target and added
//! to the target in the target's own unit: semitones for pitch, decibels for level, pan units
//! (-1..+1) for pan, normalised units for resonance, octaves for cutoff. The helpers are only
//! evaluated for a connected socket, so an unconnected input leaves the output bit-identical.

use atomik_core::db_to_gain;

/// The CV sample clamped to -1..=1; NaN gives 0.
#[inline]
pub(crate) fn clean(cv: f32) -> f32 {
    if cv.is_nan() {
        0.0
    } else {
        cv.clamp(-1.0, 1.0)
    }
}

/// Linear gain of a level CV: `10^(amount_db * cv / 20)`.
#[inline]
pub(crate) fn db_gain(amount_db: f32, cv: f32) -> f32 {
    db_to_gain(amount_db * clean(cv))
}
