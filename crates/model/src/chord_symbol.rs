//! Chord symbols: reading `Am7`, `F#m7b5` or `C/G` into a root, a quality and a bass note, and
//! writing them back.
//!
//! The qualities are the chord kinds of the chord stamp ([`ChordKind`]). The reader takes the
//! spellings people type: a root letter `A`..`G` (either case) with one `#`, `b`, `\u{266f}` or
//! `\u{266d}`, then a quality (`m`, `min`, `-`, `maj7`, `M7`, `dim`, `\u{b0}`, `sus`, `add9`, `5`
//! and the like, in the table of [`parse_quality`]), then an optional `/bass` note. Spaces and
//! round brackets around the quality are ignored. A bass equal to the root is dropped.
//!
//! [`format_chord`] writes the canonical symbol (sharps, `maj7`, `m7b5`); reading it back gives the
//! same chord.

use crate::theory::{ChordKind, pitch_name};

/// A chord as a symbol means it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChordSymbol {
    /// The root pitch class, `0..12` (C = 0).
    pub root: u8,
    /// The chord quality.
    pub quality: ChordKind,
    /// The bass note when it is not the root (`C/G`).
    pub bass: Option<u8>,
}

/// Why a chord symbol could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChordSymbolError {
    /// Nothing was typed.
    #[error("type a chord such as Am7 or C/G")]
    Empty,
    /// The text does not start with a note name.
    #[error("{0:?} does not start with a note name (A to G)")]
    BadRoot(String),
    /// The quality after the root is not one the app knows.
    #[error("unknown chord quality {0:?}")]
    UnknownQuality(String),
    /// The note after the slash is not a note name.
    #[error("{0:?} is not a bass note (A to G)")]
    BadBass(String),
}

/// The spellings of every quality, in the order [`ChordKind::ALL`] lists them. The first spelling
/// is the one [`format_chord`] writes.
const SPELLINGS: [(ChordKind, &[&str]); 21] = [
    (ChordKind::Major, &["", "maj", "major", "M"]),
    (ChordKind::Minor, &["m", "min", "minor", "-", "mi"]),
    (ChordKind::Dim, &["dim", "\u{b0}", "o", "mb5"]),
    (ChordKind::Aug, &["aug", "+", "#5"]),
    (ChordKind::Sus2, &["sus2"]),
    (ChordKind::Sus4, &["sus4", "sus"]),
    (ChordKind::Sixth, &["6", "maj6", "M6"]),
    (ChordKind::MinorSixth, &["m6", "min6", "-6"]),
    (ChordKind::Seventh, &["7", "dom7"]),
    (
        ChordKind::MajorSeventh,
        &["maj7", "M7", "major7", "ma7", "\u{394}", "\u{394}7"],
    ),
    (
        ChordKind::MinorSeventh,
        &["m7", "min7", "-7", "minor7", "mi7"],
    ),
    (
        ChordKind::HalfDim,
        &["m7b5", "min7b5", "-7b5", "\u{f8}", "\u{f8}7", "m7-5"],
    ),
    (ChordKind::DimSeventh, &["dim7", "\u{b0}7", "o7"]),
    (ChordKind::Ninth, &["9", "dom9"]),
    (ChordKind::MajorNinth, &["maj9", "M9", "major9", "\u{394}9"]),
    (ChordKind::MinorNinth, &["m9", "min9", "-9", "minor9"]),
    (ChordKind::Add9, &["add9", "add2"]),
    (ChordKind::Eleventh, &["11", "dom11"]),
    (ChordKind::Thirteenth, &["13", "dom13"]),
    (ChordKind::Power, &["5"]),
    (ChordKind::Octave, &["oct", "8"]),
];

/// The quality a spelling names: an exact match first (so `M7` is major and `m7` minor), then the
/// same spelling in lower case (`MAJ7`, `DIM`). Empty text is the major triad.
pub fn parse_quality(text: &str) -> Option<ChordKind> {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '(' && *c != ')')
        .collect();
    let exact = SPELLINGS
        .iter()
        .find(|(_, names)| names.contains(&clean.as_str()))
        .map(|(kind, _)| *kind);
    if exact.is_some() {
        return exact;
    }
    let lower = clean.to_lowercase();
    SPELLINGS
        .iter()
        .find(|(_, names)| names.contains(&lower.as_str()))
        .map(|(kind, _)| *kind)
}

/// The suffix [`format_chord`] writes for a quality: `""`, `m`, `m7b5`, `maj7`.
pub fn quality_suffix(quality: ChordKind) -> &'static str {
    SPELLINGS
        .iter()
        .find(|(kind, _)| *kind == quality)
        .map_or("", |(_, names)| names[0])
}

/// Reads a note name (a letter and at most one accidental) from the start of `text`: the pitch
/// class and what is left.
fn note_name(text: &str) -> Option<(u8, &str)> {
    let mut chars = text.chars();
    let base: i32 = match chars.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (shift, rest) = match rest.chars().next() {
        Some('#' | '\u{266f}') => (1, &rest[rest.chars().next()?.len_utf8()..]),
        Some('b' | '\u{266d}') => (-1, &rest[rest.chars().next()?.len_utf8()..]),
        _ => (0, rest),
    };
    Some(((base + shift).rem_euclid(12) as u8, rest))
}

/// Reads a chord symbol such as `Am7`, `F#m7b5`, `Bb`, `C/G` or `Dsus4/A`.
pub fn parse_chord(text: &str) -> Result<ChordSymbol, ChordSymbolError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ChordSymbolError::Empty);
    }
    let (head, bass_text) = match text.rsplit_once('/') {
        Some((h, b)) => (h, Some(b.trim())),
        None => (text, None),
    };
    let (root, quality_text) =
        note_name(head).ok_or_else(|| ChordSymbolError::BadRoot(head.to_owned()))?;
    let quality = parse_quality(quality_text)
        .ok_or_else(|| ChordSymbolError::UnknownQuality(quality_text.trim().to_owned()))?;
    let bass = match bass_text {
        None => None,
        Some(b) => match note_name(b) {
            Some((pc, "")) => Some(pc),
            _ => return Err(ChordSymbolError::BadBass(b.to_owned())),
        },
    };
    Ok(ChordSymbol {
        root,
        quality,
        bass: bass.filter(|b| *b != root),
    })
}

/// The canonical symbol of a chord: `C`, `Am7`, `F#m7b5`, `C/G`.
pub fn format_chord(root: u8, quality: ChordKind, bass: Option<u8>) -> String {
    let mut s = format!("{}{}", pitch_name(root), quality_suffix(quality));
    if let Some(b) = bass.filter(|b| b % 12 != root % 12) {
        s.push('/');
        s.push_str(pitch_name(b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str) -> (u8, ChordKind, Option<u8>) {
        let c = parse_chord(text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
        (c.root, c.quality, c.bass)
    }

    #[test]
    fn plain_triads() {
        assert_eq!(ok("C"), (0, ChordKind::Major, None));
        assert_eq!(ok("Am"), (9, ChordKind::Minor, None));
        assert_eq!(ok("Cmaj"), (0, ChordKind::Major, None));
        assert_eq!(ok("Cmin"), (0, ChordKind::Minor, None));
        assert_eq!(ok("C-"), (0, ChordKind::Minor, None));
        assert_eq!(ok("Cdim"), (0, ChordKind::Dim, None));
        assert_eq!(ok("C\u{b0}"), (0, ChordKind::Dim, None));
        assert_eq!(ok("Caug"), (0, ChordKind::Aug, None));
        assert_eq!(ok("C+"), (0, ChordKind::Aug, None));
        assert_eq!(ok("Csus2"), (0, ChordKind::Sus2, None));
        assert_eq!(ok("Csus4"), (0, ChordKind::Sus4, None));
        assert_eq!(ok("Csus"), (0, ChordKind::Sus4, None));
        assert_eq!(ok("C5"), (0, ChordKind::Power, None));
    }

    #[test]
    fn sevenths_sixths_and_extensions() {
        assert_eq!(ok("Am7"), (9, ChordKind::MinorSeventh, None));
        assert_eq!(ok("G7"), (7, ChordKind::Seventh, None));
        assert_eq!(ok("Cmaj7"), (0, ChordKind::MajorSeventh, None));
        assert_eq!(ok("CM7"), (0, ChordKind::MajorSeventh, None));
        assert_eq!(ok("C\u{394}7"), (0, ChordKind::MajorSeventh, None));
        assert_eq!(ok("F#m7b5"), (6, ChordKind::HalfDim, None));
        assert_eq!(ok("B\u{f8}"), (11, ChordKind::HalfDim, None));
        assert_eq!(ok("Cdim7"), (0, ChordKind::DimSeventh, None));
        assert_eq!(ok("C6"), (0, ChordKind::Sixth, None));
        assert_eq!(ok("Am6"), (9, ChordKind::MinorSixth, None));
        assert_eq!(ok("D9"), (2, ChordKind::Ninth, None));
        assert_eq!(ok("Dmaj9"), (2, ChordKind::MajorNinth, None));
        assert_eq!(ok("Dm9"), (2, ChordKind::MinorNinth, None));
        assert_eq!(ok("Cadd9"), (0, ChordKind::Add9, None));
        assert_eq!(ok("C11"), (0, ChordKind::Eleventh, None));
        assert_eq!(ok("C13"), (0, ChordKind::Thirteenth, None));
    }

    #[test]
    fn accidentals_in_both_spellings() {
        assert_eq!(ok("Bb"), (10, ChordKind::Major, None));
        assert_eq!(ok("Bbm7"), (10, ChordKind::MinorSeventh, None));
        assert_eq!(ok("F#"), (6, ChordKind::Major, None));
        assert_eq!(ok("F\u{266f}m"), (6, ChordKind::Minor, None));
        assert_eq!(ok("E\u{266d}"), (3, ChordKind::Major, None));
        assert_eq!(ok("C#dim"), (1, ChordKind::Dim, None));
        assert_eq!(ok("Cb"), (11, ChordKind::Major, None));
        assert_eq!(ok("E#m"), (5, ChordKind::Minor, None));
    }

    #[test]
    fn slash_chords_name_the_bass() {
        assert_eq!(ok("C/G"), (0, ChordKind::Major, Some(7)));
        assert_eq!(ok("Am7/G"), (9, ChordKind::MinorSeventh, Some(7)));
        assert_eq!(ok("D/F#"), (2, ChordKind::Major, Some(6)));
        assert_eq!(ok("Dsus4/A"), (2, ChordKind::Sus4, Some(9)));
        assert_eq!(ok("C / Bb"), (0, ChordKind::Major, Some(10)));
        assert_eq!(
            ok("C/C"),
            (0, ChordKind::Major, None),
            "a root bass is dropped"
        );
    }

    #[test]
    fn case_space_and_brackets_are_forgiven() {
        assert_eq!(ok("  am7 "), (9, ChordKind::MinorSeventh, None));
        assert_eq!(ok("CMAJ7"), (0, ChordKind::MajorSeventh, None));
        assert_eq!(ok("c"), (0, ChordKind::Major, None));
        assert_eq!(ok("bb"), (10, ChordKind::Major, None));
        assert_eq!(ok("C(add9)"), (0, ChordKind::Add9, None));
        assert_eq!(ok("C maj 7"), (0, ChordKind::MajorSeventh, None));
        assert_eq!(ok("CDIM"), (0, ChordKind::Dim, None));
    }

    #[test]
    fn capital_m_is_major_and_small_m_is_minor() {
        assert_eq!(ok("CM"), (0, ChordKind::Major, None));
        assert_eq!(ok("Cm"), (0, ChordKind::Minor, None));
        assert_eq!(ok("CM7"), (0, ChordKind::MajorSeventh, None));
        assert_eq!(ok("Cm7"), (0, ChordKind::MinorSeventh, None));
    }

    #[test]
    fn errors_say_what_is_wrong() {
        assert_eq!(parse_chord(""), Err(ChordSymbolError::Empty));
        assert_eq!(parse_chord("   "), Err(ChordSymbolError::Empty));
        assert_eq!(
            parse_chord("H7"),
            Err(ChordSymbolError::BadRoot("H7".into()))
        );
        assert_eq!(parse_chord("7"), Err(ChordSymbolError::BadRoot("7".into())));
        assert_eq!(
            parse_chord("/G"),
            Err(ChordSymbolError::BadRoot(String::new()))
        );
        assert_eq!(
            parse_chord("Cxyz"),
            Err(ChordSymbolError::UnknownQuality("xyz".into()))
        );
        assert_eq!(
            parse_chord("C7#9"),
            Err(ChordSymbolError::UnknownQuality("7#9".into()))
        );
        assert_eq!(
            parse_chord("C/H"),
            Err(ChordSymbolError::BadBass("H".into()))
        );
        assert_eq!(
            parse_chord("C/"),
            Err(ChordSymbolError::BadBass(String::new()))
        );
        assert_eq!(
            parse_chord("C/G7"),
            Err(ChordSymbolError::BadBass("G7".into()))
        );
        assert_eq!(
            parse_chord("Cbb"),
            Err(ChordSymbolError::UnknownQuality("b".into())),
            "a double flat is not a spelling"
        );
        assert!(
            ChordSymbolError::Empty.to_string().contains("Am7"),
            "the message shows an example"
        );
    }

    #[test]
    fn every_quality_round_trips_through_its_symbol() {
        for quality in ChordKind::ALL {
            for root in 0..12 {
                for bass in [None, Some((root + 7) % 12)] {
                    let text = format_chord(root, quality, bass);
                    let back = parse_chord(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
                    assert_eq!(
                        (back.root, back.quality, back.bass),
                        (root, quality, bass),
                        "{text}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_canonical_symbols() {
        assert_eq!(format_chord(0, ChordKind::Major, None), "C");
        assert_eq!(format_chord(9, ChordKind::MinorSeventh, None), "Am7");
        assert_eq!(format_chord(6, ChordKind::HalfDim, None), "F#m7b5");
        assert_eq!(format_chord(0, ChordKind::Major, Some(7)), "C/G");
        assert_eq!(format_chord(0, ChordKind::Major, Some(0)), "C");
        assert_eq!(format_chord(10, ChordKind::MajorSeventh, None), "A#maj7");
    }
}
