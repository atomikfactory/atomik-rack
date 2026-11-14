//! The chord of one short sound (Sound Match): several notes sounding together in a sample.
//!
//! [`detect_sound_chord`] runs the multi-pitch analysis of [`super::analyse`] over the whole
//! sound, with the settings of the chord analysis. A key counts as a note of the sound when it is
//! found in at least [`PRESENT_SHARE`] of the loud frames (the frames within [`LOUD_SHARE`] of the
//! loudest one) and, summed over those frames, is at least [`NOTE_SHARE`] as strong as the
//! strongest key. Fewer than two different pitch classes is not a chord: the sound is one note
//! (or an octave), and the caller keeps the single-note path. The chord's root, quality and bass
//! come from the same templates as the chord detection of a recording, applied to the chroma of
//! the whole sound as one unit.

use super::chords::{BeatChroma, ChordOptions, recognise_chords};
use super::{AnalysisParams, MAX_VOICES, analyse};
use atomik_core::theory::ChordKind;

/// A frame is loud when its level is at least this share of the loudest frame's.
pub const LOUD_SHARE: f32 = 0.5;
/// A note must be found in at least this share of the loud frames.
pub const PRESENT_SHARE: f32 = 0.6;
/// A note's summed strength must be at least this share of the strongest note's.
pub const NOTE_SHARE: f32 = 0.35;
/// The fewest loud frames a verdict is given on.
pub const MIN_FRAMES: usize = 3;

/// The notes and the chord of a sound.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundChord {
    /// The notes, MIDI keys, lowest first (two to [`MAX_VOICES`]).
    pub keys: Vec<u8>,
    /// Root pitch class, `0..12`.
    pub root: u8,
    /// Quality.
    pub quality: ChordKind,
    /// Bass pitch class when it is not the root.
    pub bass: Option<u8>,
    /// How well the chroma fits the chord, `0..=1`.
    pub confidence: f32,
}

/// The chord of the sound in `channels`, or `None` when it is one note, unpitched, silent or too
/// short to tell.
pub fn detect_sound_chord(channels: &[&[f32]], rate: u32) -> Option<SoundChord> {
    let a = analyse(channels, rate, &AnalysisParams::chords(rate));
    let top = a.frames.iter().fold(0.0f32, |m, f| m.max(f.level));
    if top <= 0.0 {
        return None;
    }
    let loud: Vec<&super::Frame> = a
        .frames
        .iter()
        .filter(|f| f.level >= LOUD_SHARE * top && !f.voices.is_empty())
        .collect();
    if loud.len() < MIN_FRAMES {
        return None;
    }
    let mut weight = [0.0f32; 128];
    let mut present = [0usize; 128];
    for f in &loud {
        for v in &f.voices {
            weight[usize::from(v.key & 0x7F)] += v.strength;
            present[usize::from(v.key & 0x7F)] += 1;
        }
    }
    let strongest = weight.iter().fold(0.0f32, |m, w| m.max(*w));
    let need = (PRESENT_SHARE * loud.len() as f32).ceil() as usize;
    let mut keys: Vec<u8> = (0..128u8)
        .filter(|k| {
            let i = usize::from(*k);
            present[i] >= need && weight[i] >= NOTE_SHARE * strongest
        })
        .collect();
    // Keep the strongest when there are more than a frame can hold.
    if keys.len() > MAX_VOICES {
        keys.sort_by(|x, y| weight[usize::from(*y)].total_cmp(&weight[usize::from(*x)]));
        keys.truncate(MAX_VOICES);
        keys.sort_unstable();
    }
    let mut classes = [false; 12];
    for k in &keys {
        classes[usize::from(k % 12)] = true;
    }
    if classes.iter().filter(|c| **c).count() < 2 {
        return None;
    }
    let mut unit = BeatChroma::default();
    for f in &loud {
        unit.add(f);
    }
    let chord = recognise_chords(&[unit], &ChordOptions::default())
        .into_iter()
        .next()?;
    Some(SoundChord {
        keys,
        root: chord.root,
        quality: chord.quality,
        bass: chord.bass,
        confidence: chord.confidence,
    })
}
