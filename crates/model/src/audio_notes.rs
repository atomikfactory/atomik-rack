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
//!
//! **Pitch bend.** A note found with a contour ([`FoundNote::bend`]: how far the voice is from the
//! note's key, in semitones) keeps it: the clip gets one pitch-bend lane
//! ([`crate::ControllerKind::PitchBend`]) that follows the notes. The lane assumes the default bend
//! range of [`BEND_RANGE_SEMITONES`], so a voice 0.5 semitone sharp is a bend of 0.25. Each note's
//! contour starts at the note's first tick and its last point holds (a `Step`) until the next
//! note's contour starts, so notes never glide into each other; a note without a contour puts the
//! bend back to the centre when the one before it left it away, and the last note does the same
//! after it ends. A clip with no contour has no lane.

use crate::controller::{ControllerKind, ControllerLane};
use crate::song::{
    AudioClipContent, AutomationPoint, Clip, ClipContent, ClipId, Curve, Note, Song,
};
use crate::warp::tick_at_source;

/// The pitch-bend range, in semitones each way, that the bend lane of a clip is scaled for: the
/// default of every instrument.
pub const BEND_RANGE_SEMITONES: f32 = 2.0;

/// One point of the pitch of a found note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoundBend {
    /// Frame, counted like [`FoundNote::start`].
    pub frame: u64,
    /// Semitones above (positive) or below the note's key.
    pub semitones: f32,
}

/// One note found in the audio, in frames of the analysed audio.
#[derive(Debug, Clone, PartialEq)]
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
    /// How the pitch moves inside the note, in time order (empty: it holds its key).
    pub bend: Vec<FoundBend>,
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
    // A note with the ticks of its bend, relative to the clip.
    let mut built: Vec<(Note, Vec<(i64, f32)>)> = Vec::new();
    for f in found {
        let raw = at(f.start).max(0);
        let len = (at(f.start.saturating_add(f.length)) - raw).max(1);
        let on = if grid > 1 {
            ((raw + grid / 2) / grid) * grid
        } else {
            raw
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
        // The contour moves with its note when the start snaps.
        let bend = f
            .bend
            .iter()
            .filter(|p| p.semitones.is_finite())
            .map(|p| (at(p.frame).max(0) + on - raw, p.semitones))
            .collect();
        built.push((Note::new(on, off - on, key, velocity), bend));
    }
    built.sort_by_key(|(n, _)| (n.tick, n.key));
    // A note of a key ends where the next of that key starts (snapping can make them touch).
    for i in 0..built.len() {
        let (tick, key) = (built[i].0.tick, built[i].0.key);
        if let Some(next) = built[i + 1..]
            .iter()
            .find(|(n, _)| n.key == key && n.tick > tick)
            .map(|(n, _)| n.tick)
            && tick + built[i].0.length > next
        {
            built[i].0.length = next - tick;
        }
    }
    // Two notes of one key that snapped to the same tick are one note.
    built.dedup_by(|b, a| a.0.key == b.0.key && a.0.tick == b.0.tick);
    if built.is_empty() {
        return Err("no notes found in the clip".to_owned());
    }
    let controllers = bend_lane(&built, length);
    let notes: Vec<Note> = built.into_iter().map(|(n, _)| n).collect();
    Ok(Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers,
        name: None,
        color: None,
        content: ClipContent::Notes(notes.into()),
    })
}

/// The pitch-bend lane for `notes` and their contours (module docs); none when no note bends.
fn bend_lane(notes: &[(Note, Vec<(i64, f32)>)], length: i64) -> Vec<ControllerLane> {
    let kind = ControllerKind::PitchBend;
    let mut points: Vec<AutomationPoint> = Vec::new();
    // Puts a point, in strictly increasing ticks: one at the tick of the last replaces it.
    fn put(points: &mut Vec<AutomationPoint>, length: i64, p: AutomationPoint) {
        if p.tick < 0 || p.tick >= length {
            return;
        }
        match points.last_mut() {
            Some(last) if last.tick == p.tick => *last = p,
            Some(last) if last.tick > p.tick => {}
            _ => points.push(p),
        }
    }
    let at = |tick: i64, value: f32, curve: Curve| AutomationPoint { tick, value, curve };
    let mut away = false;
    let mut any = false;
    for (note, bend) in notes {
        let end = note.tick + note.length;
        let inside: Vec<(i64, f32)> = bend
            .iter()
            .filter(|(t, _)| *t >= note.tick && *t < end)
            .copied()
            .collect();
        if inside.is_empty() {
            if away {
                put(&mut points, length, at(note.tick, 0.0, Curve::Step));
                away = false;
            }
            continue;
        }
        any = true;
        away = true;
        let value = |semitones: f32| kind.quantize(semitones / BEND_RANGE_SEMITONES);
        // The contour begins with the note, however late its first point is.
        put(
            &mut points,
            length,
            at(note.tick, value(inside[0].1), Curve::Linear),
        );
        for (t, v) in &inside {
            put(&mut points, length, at(*t, value(*v), Curve::Linear));
        }
        if let Some(last) = points.last_mut() {
            last.curve = Curve::Step;
        }
    }
    if !any {
        return Vec::new();
    }
    if away && let Some((last, _)) = notes.last() {
        put(
            &mut points,
            length,
            at(last.tick + last.length, 0.0, Curve::Step),
        );
    }
    vec![ControllerLane::new(kind, points)]
}
