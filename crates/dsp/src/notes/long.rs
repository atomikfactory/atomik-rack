//! Audio to notes over a take of any length: the extraction run a stretch at a time.
//!
//! The detectors hold a whole take in memory and look at all of it at once, which is fine for a
//! phrase and not for an hour of audio. [`extract_in_segments`] cuts the take into stretches of
//! [`SEGMENT_SECONDS`], reads each with [`CONTEXT_SECONDS`] of the audio around it (so that a note
//! at the edge of a stretch is heard whole), runs the detector on that, and keeps the notes that
//! **start** inside the stretch. Every note belongs to exactly one stretch. After each stretch the
//! caller is told how far the analysis has got and may stop it.
//!
//! A take of at most one stretch and its context is analysed in one go, exactly as the detector
//! alone would. Beyond that the stretches are separate analyses, so a quiet passage is judged
//! against the loudest sound of its own stretch and a note longer than the context that crosses
//! the edge of a stretch is cut there. Velocities are the exception: they are worked out again at
//! the end against the loudest note of the whole take.

use super::{
    DrumParams, ExtractedNote, PitchParams, extract_drum_notes, extract_pitched_notes, velocity_for,
};
use crate::harmony::{PolyParams, extract_poly_notes};

/// Length of one stretch of the analysis, in seconds.
pub const SEGMENT_SECONDS: u64 = 60;
/// Audio read on each side of a stretch, in seconds.
pub const CONTEXT_SECONDS: u64 = 3;

/// What to look for.
#[derive(Debug, Clone, Copy)]
pub enum Analysis<'a> {
    /// One voice or instrument.
    Pitched(&'a PitchParams),
    /// A drum loop.
    Drums(&'a DrumParams),
    /// Chords and several voices of a piano or a guitar.
    Poly(&'a PolyParams),
}

impl Analysis<'_> {
    fn run(self, mono: &[f32], rate: u32) -> Vec<ExtractedNote> {
        match self {
            Analysis::Pitched(p) => extract_pitched_notes(&[mono], rate, p),
            Analysis::Drums(p) => extract_drum_notes(&[mono], rate, p),
            Analysis::Poly(p) => extract_poly_notes(&[mono], rate, p),
        }
    }
}

/// The notes of a take of `total` frames at `rate`, in order.
///
/// `read(from, n)` gives the mono audio of frames `[from, from + n)` (fewer if the take ends
/// earlier). `progress(done)` is called with the number of frames finished, first with 0 and then
/// after each stretch; it returns `false` to stop, and the result is then `None`.
pub fn extract_in_segments(
    total: u64,
    rate: u32,
    what: Analysis<'_>,
    read: &mut dyn FnMut(u64, u64) -> Vec<f32>,
    progress: &mut dyn FnMut(u64) -> bool,
) -> Option<Vec<ExtractedNote>> {
    if !progress(0) {
        return None;
    }
    let rate64 = u64::from(rate);
    let (segment, context) = (SEGMENT_SECONDS * rate64, CONTEXT_SECONDS * rate64);
    if total <= segment + 2 * context {
        let mono = read(0, total);
        let notes = what.run(&mono, rate);
        return progress(total).then_some(notes);
    }
    let mut found: Vec<ExtractedNote> = Vec::new();
    let mut from = 0u64;
    while from < total {
        let to = (from + segment).min(total);
        let lead = from.saturating_sub(context);
        let tail = (to + context).min(total);
        let mono = read(lead, tail - lead);
        for mut note in what.run(&mono, rate) {
            let start = note.start + lead;
            // Notes that start before this stretch belong to the one before it; a note that
            // starts after it belongs to the next.
            if start < from || start >= to {
                continue;
            }
            note.start = start;
            for point in &mut note.bend {
                point.frame += lead;
            }
            found.push(note);
        }
        from = to;
        if !progress(to) {
            return None;
        }
    }
    let reference = found.iter().fold(0.0f32, |m, n| m.max(n.level));
    for note in &mut found {
        note.velocity = velocity_for(note.level, reference);
    }
    Some(found)
}
