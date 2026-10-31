//! Audio to notes: the notes found in the audio of a clip, as a note clip that lines up with it.
//!
//! The detectors (`atomik_dsp::notes`) work on frames of the audio they were given; this turns
//! their result into ticks of the song. Nothing here reads samples, and no new command is
//! needed: the clip is added with `AddClip` (to a note track that exists) or in a new track with
//! `WorkflowCommand::ImportMidi` (one undo step either way), so the format does not change.
//!
//! **Time.** A note found `f` frames into the audio the app analysed sits where the clip plays
//! that frame ([`crate::tick_at_source`]): through the song's tempo map for a clip without warp
//! (a tempo ramp or a tempo change under the clip moves the notes with the audio), through the
//! warp time map (tempo-following stretch and markers) for a warped one. A reversed clip is
//! analysed in the order it plays, so its frames count from the clip's start like any other. The
//! note clip starts and ends with the audio clip, with no content offset.
//!
//! **Beats timing** ([`NoteOptions::snap`]) rounds the start of every note to a grid, keeping its
//! length, so a loose performance lands on the beat grid; the notes of one key never overlap
//! afterwards.

use crate::song::{AudioClipContent, Clip, ClipContent, ClipId, Note, Song};
use crate::warp::tick_at_source;

/// One note found in the audio, in frames of the analysed audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoundNote {
    /// First frame, counted from the first frame the clip plays of its asset (from the clip's
    /// start for a reversed clip, whose audio is analysed as it plays).
    pub start: u64,
    /// Length in frames.
    pub length: u64,
    /// MIDI key as heard (before [`NoteOptions::transpose`]).
    pub key: u8,
    /// Velocity in `(0, 1]`.
    pub velocity: f32,
}

/// How [`notes_clip`] places the notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NoteOptions {
    /// Round the start of each note to a multiple of this many ticks from the clip start (0 for
    /// no rounding).
    pub snap: i64,
    /// Semitones added to every key (the transposition of a warped clip), the result kept in
    /// `0..=127`.
    pub transpose: i32,
}

/// The note clip for `found`, the notes of the audio clip `audio` that starts at tick `start` and
/// lasts `length` ticks. `Err` says why there is none (nothing found inside the clip).
pub fn notes_clip(
    song: &Song,
    sample_rate: u32,
    (start, length): (i64, i64),
    audio: &AudioClipContent,
    found: &[FoundNote],
    options: NoteOptions,
) -> Result<Clip, String> {
    let at = |frame: u64| {
        let src = audio.source_offset as f64 + frame as f64;
        tick_at_source(song, sample_rate, start, audio, src).round() as i64 - start
    };
    let grid = options.snap.max(0);
    let mut notes: Vec<Note> = Vec::new();
    for f in found {
        let on = at(f.start).max(0);
        let len = (at(f.start.saturating_add(f.length)) - on).max(1);
        let on = if grid > 1 {
            ((on + grid / 2) / grid) * grid
        } else {
            on
        };
        if on >= length {
            continue;
        }
        let off = (on + len).min(length);
        let key = (i32::from(f.key) + options.transpose).clamp(0, 127) as u8;
        let velocity = if f.velocity.is_finite() {
            f.velocity.clamp(0.01, 1.0)
        } else {
            0.5
        };
        notes.push(Note::new(on, off - on, key, velocity));
    }
    notes.sort_by_key(|n| (n.tick, n.key));
    // A note of a key ends where the next of that key starts (snapping can make them touch).
    for i in 0..notes.len() {
        let (tick, key) = (notes[i].tick, notes[i].key);
        if let Some(next) = notes[i + 1..]
            .iter()
            .find(|n| n.key == key && n.tick > tick)
            .map(|n| n.tick)
            && tick + notes[i].length > next
        {
            notes[i].length = next - tick;
        }
    }
    // Two notes of one key that snapped to the same tick are one note.
    notes.dedup_by(|b, a| a.key == b.key && a.tick == b.tick);
    if notes.is_empty() {
        return Err("no notes found in the clip".to_owned());
    }
    Ok(Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        content: ClipContent::Notes(notes.into()),
    })
}
