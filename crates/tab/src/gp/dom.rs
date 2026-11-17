//! The neutral document every Guitar Pro reader builds before conversion.
//!
//! Note values are already ticks (in sub-tick units, see [`K`]), strings are in tab order (index 0 =
//! top line), and the effects of a beat are already folded into the techniques of its notes. What
//! the converter still has to do is the timeline: repeats, ties, hammer-ons and the tempo map.

use crate::score::{Bend, KeySignature, PPQ, Techniques};

/// Sub-tick units per tick. Durations are kept as exact multiples of `1 / K` of a tick, so that a
/// bar of quintuplets, septuplets or elevenlets adds up to the bar without drift. `K` is divisible by
/// every tuplet size Guitar Pro offers (3 to 13).
pub(crate) const K: i64 = 180_180;

/// The length of a whole note in sub-tick units.
pub(crate) const WHOLE: i64 = 4 * PPQ * K;

/// The most voices a bar has (Guitar Pro 5 writes two, the XML formats four).
pub(crate) const MAX_VOICES: usize = 4;

/// The text fields of a score.
#[derive(Debug, Default, Clone)]
pub(crate) struct Info {
    pub title: String,
    pub subtitle: String,
    pub artist: String,
    pub album: String,
    pub words: String,
    pub music: String,
    pub tab_by: String,
    pub copyright: String,
    pub notices: String,
}

/// One note.
#[derive(Debug, Clone)]
pub(crate) struct Note {
    /// The string, tab order. For a percussion track just the lane the format used.
    pub string: u8,
    /// The fret (the MIDI key for percussion).
    pub fret: u8,
    /// MIDI velocity.
    pub velocity: u8,
    /// Continues the previous note on the string.
    pub tie: bool,
    /// The note is the origin of a hammer-on or pull-off (the link goes to the next note on the string).
    pub hopo_origin: bool,
    /// The note is the destination of a hammer-on or pull-off.
    pub hopo_dest: bool,
    /// Everything else about how it is played (without hammer and pull flags).
    pub tech: Techniques,
}

/// A chord diagram as a format stores it.
#[derive(Debug, Clone)]
pub(crate) struct Diagram {
    pub name: String,
    /// The fret of each string in tab order (`None` = not played).
    pub frets: Vec<Option<u8>>,
}

/// One beat: a chord (or a single note) or a rest.
#[derive(Debug, Clone, Default)]
pub(crate) struct Beat {
    /// Where it starts in the bar, in sub-tick units (set by [`place`]).
    pub start: i64,
    /// How long it lasts, in sub-tick units.
    pub units: i64,
    /// The notes (none for a rest).
    pub notes: Vec<Note>,
    /// A tempo change that takes effect on this beat.
    pub tempo: Option<f64>,
    /// A chord diagram shown over this beat.
    pub diagram: Option<Diagram>,
    /// The lyric syllable sung on this beat.
    pub lyric: Option<String>,
}

/// Sets the start of each beat from the lengths before it.
pub(crate) fn place(beats: &mut [Beat]) {
    let mut pos = 0i64;
    for b in beats {
        b.start = pos;
        pos = pos.saturating_add(b.units);
    }
}

/// One bar of one track: up to [`MAX_VOICES`] voices of beats.
#[derive(Debug, Clone, Default)]
pub(crate) struct Bar {
    pub voices: Vec<Vec<Beat>>,
}

/// One track.
#[derive(Debug, Clone)]
pub(crate) struct Track {
    pub name: String,
    /// Open pitches in tab order.
    pub opens: Vec<u8>,
    pub frets: u8,
    pub capo: u8,
    pub program: u8,
    pub percussion: bool,
    pub banjo: bool,
    pub twelve_string: bool,
    /// One bar per master bar.
    pub bars: Vec<Bar>,
}

/// The facts of one bar that hold for every track.
#[derive(Debug, Clone)]
pub(crate) struct MasterBar {
    pub num: u8,
    pub den: u8,
    pub repeat_open: bool,
    /// How many times the section that ends here is played in all (2 = once repeated).
    pub repeat_close: Option<u8>,
    /// The passes (bit 0 = the first) this bar belongs to as an alternative ending; 0 = all.
    pub alt: u8,
    pub marker: Option<String>,
    pub key: Option<KeySignature>,
    /// Tempo changes `(position in the bar 0..1, bpm)`.
    pub tempo_auto: Vec<(f64, f64)>,
}

impl MasterBar {
    pub(crate) fn new(num: u8, den: u8) -> Self {
        Self {
            num,
            den,
            repeat_open: false,
            repeat_close: None,
            alt: 0,
            marker: None,
            key: None,
            tempo_auto: Vec::new(),
        }
    }
}

/// Lyrics: the track they belong to and the lines with the bar each starts at.
#[derive(Debug, Clone, Default)]
pub(crate) struct Lyrics {
    pub track: usize,
    pub lines: Vec<(usize, String)>,
}

/// A whole score.
#[derive(Debug, Default)]
pub(crate) struct Doc {
    pub info: Info,
    pub tempo: f64,
    pub master_bars: Vec<MasterBar>,
    pub tracks: Vec<Track>,
    pub lyrics: Option<Lyrics>,
}

/// A whole-tone or a semitone bend value in the units the formats use (50 per semitone) to cents.
pub(crate) fn bend_units_to_cents(v: i64) -> i32 {
    i32::try_from((v * 2).clamp(-4800, 4800)).unwrap_or(0)
}

/// Makes a bend from `(position 0..1, units)` points.
pub(crate) fn bend_from_units(points: &[(f32, i64)]) -> Bend {
    let mut b = Bend::from_pairs(&[]);
    for (pos, v) in points {
        b.points.push(crate::score::BendPoint {
            pos: *pos,
            cents: bend_units_to_cents(*v),
        });
    }
    b.sanitize();
    b
}

/// Splits the text of a lyric line into syllables: words are separated by spaces, syllables by a
/// hyphen (`Hel-lo world` gives `Hel`, `lo`, `world`).
pub(crate) fn syllables(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        for part in word.split('-') {
            if !part.is_empty() {
                out.push(part.to_owned());
            }
        }
    }
    out
}

/// Hands the lyric lines to the beats with notes of the lyric track, from the bar each line starts
/// at.
pub(crate) fn assign_lyrics(doc: &mut Doc) {
    let Some(ly) = doc.lyrics.take() else {
        return;
    };
    let Some(track) = doc.tracks.get_mut(ly.track) else {
        return;
    };
    for (start, text) in &ly.lines {
        let mut words = syllables(text).into_iter();
        'bars: for bar in track.bars.iter_mut().skip(*start) {
            let Some(voice) = bar.voices.first_mut() else {
                continue;
            };
            for beat in voice.iter_mut().filter(|b| !b.notes.is_empty()) {
                match words.next() {
                    Some(w) => beat.lyric = Some(w),
                    None => break 'bars,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k_divides_every_tuplet() {
        for n in [3, 5, 6, 7, 9, 10, 11, 12, 13] {
            assert_eq!(K % n, 0, "{n}");
        }
        assert_eq!(WHOLE % 64, 0);
    }

    #[test]
    fn syllables_split_on_spaces_and_hyphens() {
        assert_eq!(
            syllables("Hel-lo  big world"),
            ["Hel", "lo", "big", "world"]
        );
        assert!(syllables("  - ").is_empty());
    }

    #[test]
    fn bend_units_are_fifty_per_semitone() {
        assert_eq!(bend_units_to_cents(100), 200);
        assert_eq!(bend_units_to_cents(25), 50);
        assert_eq!(bend_units_to_cents(i64::MAX / 4), 4800);
    }
}
