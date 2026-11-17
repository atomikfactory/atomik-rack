//! Key-aware transposition of chords, and capo shapes.
//!
//! The spelling of a transposed note comes from the target key when there is one: a note that is
//! in the key's scale gets the scale's own spelling (`Eb` in E-flat major, `E#` in F-sharp major),
//! any other note follows the key signature's lean (flats for a flat key, sharps otherwise). With
//! no key a [`Spelling::Sharps`] or [`Spelling::Flats`] preference decides. `E#`, `B#`, `Cb` and
//! `Fb` therefore only appear in a key that needs them, never for a stray pitch class.

use crate::chord::{ChordSymbol, Note};

/// A key: a spelled tonic and a mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    /// The tonic.
    pub tonic: Note,
    /// Minor (natural minor spelling) instead of major.
    pub minor: bool,
}

/// How to spell a transposed note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spelling {
    /// Sharps for every black key.
    Sharps,
    /// Flats for every black key.
    Flats,
    /// The spelling of this key.
    Key(Key),
}

const MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR: [u8; 7] = [0, 2, 3, 5, 7, 8, 10];

impl Key {
    /// A key.
    pub fn new(tonic: Note, minor: bool) -> Self {
        Self { tonic, minor }
    }

    /// Reads `G`, `F#m`, `Bb`, `Am`, `C minor`, `Eb major`.
    pub fn parse(text: &str) -> Option<Key> {
        let t = text.trim();
        let (tonic, rest) = Note::parse_prefix(t)?;
        let rest = rest.trim().to_lowercase();
        let minor = match rest.as_str() {
            "" | "maj" | "major" | "dur" => false,
            "m" | "min" | "minor" | "mol" => true,
            _ => return None,
        };
        Some(Key { tonic, minor })
    }

    /// The name: `G`, `F#m`.
    pub fn name(&self) -> String {
        format!("{}{}", self.tonic.name(), if self.minor { "m" } else { "" })
    }

    /// The seven notes of the key's scale, spelled from the tonic's letter. `None` when the
    /// spelling would need more than two accidentals on a note (`Fb` major and the like).
    pub fn scale(&self) -> Option<[Note; 7]> {
        let steps = if self.minor { MINOR } else { MAJOR };
        let tonic_pc = i32::from(self.tonic.pitch_class());
        let mut out = [Note::new(0, 0); 7];
        for (i, st) in steps.iter().enumerate() {
            let letter = (self.tonic.letter + i as u8) % 7;
            let natural = i32::from([0u8, 2, 4, 5, 7, 9, 11][usize::from(letter)]);
            let target = (tonic_pc + i32::from(*st)).rem_euclid(12);
            let mut acc = (target - natural).rem_euclid(12);
            if acc > 6 {
                acc -= 12;
            }
            if acc.abs() > 1 {
                return None;
            }
            out[i] = Note::new(letter, acc as i8);
        }
        Some(out)
    }

    /// Whether the key signature leans to flats (more flats than sharps in its scale).
    pub fn prefers_flats(&self) -> bool {
        match self.scale() {
            Some(s) => s.iter().map(|n| i32::from(n.accidental)).sum::<i32>() < 0,
            None => self.tonic.accidental < 0,
        }
    }

    /// The signed number of accidentals of the key signature (sharps positive).
    pub fn fifths(&self) -> i32 {
        self.scale()
            .map_or(0, |s| s.iter().map(|n| i32::from(n.accidental)).sum())
    }
}

/// The spelled tonic for a pitch class when no spelling is given, by the usual key choice
/// (fewest accidentals; F# and B major, Db and Ab major, Eb and Bb minor, G# and C# minor).
fn tonic_for(pc: u8, minor: bool) -> Note {
    const MAJ: [(u8, i8); 12] = [
        (0, 0),
        (1, -1),
        (1, 0),
        (2, -1),
        (2, 0),
        (3, 0),
        (3, 1),
        (4, 0),
        (5, -1),
        (5, 0),
        (6, -1),
        (6, 0),
    ];
    const MIN: [(u8, i8); 12] = [
        (0, 0),
        (0, 1),
        (1, 0),
        (2, -1),
        (2, 0),
        (3, 0),
        (3, 1),
        (4, 0),
        (4, 1),
        (5, 0),
        (6, -1),
        (6, 0),
    ];
    let (l, a) = if minor { MIN } else { MAJ }[usize::from(pc % 12)];
    Note::new(l, a)
}

/// The spelling of `pc` under `spelling`.
pub fn spell(pc: u8, spelling: Spelling) -> Note {
    match spelling {
        Spelling::Sharps => Note::from_pitch_class(pc, false),
        Spelling::Flats => Note::from_pitch_class(pc, true),
        Spelling::Key(k) => {
            if let Some(scale) = k.scale()
                && let Some(n) = scale.iter().find(|n| n.pitch_class() == pc % 12)
            {
                return *n;
            }
            Note::from_pitch_class(pc, k.prefers_flats())
        }
    }
}

/// `note` moved by `semitones`, spelled as `spelling` says.
pub fn transpose_note(note: Note, semitones: i32, spelling: Spelling) -> Note {
    let pc = (i32::from(note.pitch_class()) + semitones).rem_euclid(12) as u8;
    spell(pc, spelling)
}

/// The key `semitones` away, with the usual spelling of the new tonic (the same lean as the
/// old key when both spellings are equally common).
pub fn transpose_key(key: Key, semitones: i32) -> Key {
    let pc = (i32::from(key.tonic.pitch_class()) + semitones).rem_euclid(12) as u8;
    let mut tonic = tonic_for(pc, key.minor);
    // A key spelled with flats keeps flats on the black keys when sharps and flats tie
    // (F# major against Gb major, D# minor against Eb minor).
    if key.tonic.accidental < 0 && tonic.accidental > 0 {
        let flat = Note::from_pitch_class(pc, true);
        if Key::new(flat, key.minor).scale().is_some() {
            tonic = flat;
        }
    }
    Key {
        tonic,
        minor: key.minor,
    }
}

/// `chord` moved by `semitones`: the root and the slash bass are re-spelled, everything else is
/// kept. `raw` is cleared.
pub fn transpose_chord(chord: &ChordSymbol, semitones: i32, spelling: Spelling) -> ChordSymbol {
    let mut out = chord.clone();
    out.root = transpose_note(chord.root, semitones, spelling);
    out.bass = chord
        .bass
        .map(|b| transpose_note(b, semitones, spelling))
        .filter(|b| b.pitch_class() != out.root.pitch_class());
    out.raw.clear();
    out
}

/// The chord to play with a capo at fret `capo`: the shapes are `capo` semitones lower than
/// the sounding chord. The spelling follows the shape key (the song key moved down by the capo)
/// when there is a key.
pub fn capo_shape(chord: &ChordSymbol, capo: u8, key: Option<Key>) -> ChordSymbol {
    let sp = match key {
        Some(k) => Spelling::Key(transpose_key(k, -i32::from(capo))),
        None => Spelling::Flats,
    };
    let sp = if key.is_none() && uses_sharps(chord) {
        Spelling::Sharps
    } else {
        sp
    };
    transpose_chord(chord, -i32::from(capo), sp)
}

fn uses_sharps(chord: &ChordSymbol) -> bool {
    chord.root.accidental > 0 || chord.bass.is_some_and(|b| b.accidental > 0)
}

/// The shape key of a song key with a capo.
pub fn capo_key(key: Key, capo: u8) -> Key {
    transpose_key(key, -i32::from(capo))
}

/// Every chord of a list in capo shapes, and the capo (0 to `max_capo`) that makes the shapes
/// easiest: the one with the most chords from the friendly open set (C, A, G, E, D, Am, Em, Dm
/// and their seventh forms). Ties go to the lower capo. Returns `(capo, shapes)`.
pub fn suggest_capo(
    chords: &[ChordSymbol],
    max_capo: u8,
    key: Option<Key>,
) -> (u8, Vec<ChordSymbol>) {
    let mut best = (0u8, i32::MIN);
    for capo in 0..=max_capo {
        let score: i32 = chords
            .iter()
            .map(|c| {
                let s = capo_shape(c, capo, key);
                friendly(&s)
            })
            .sum();
        if score > best.1 {
            best = (capo, score);
        }
    }
    let shapes = chords.iter().map(|c| capo_shape(c, best.0, key)).collect();
    (best.0, shapes)
}

/// How easy a shape is on a guitar-like neck: open chords score high, barre chords low.
fn friendly(c: &ChordSymbol) -> i32 {
    use crate::chord::Quality;
    let open = match (c.root.pitch_class(), c.quality) {
        (0 | 7 | 9 | 4 | 2, Quality::Major) => 3,
        (9 | 4 | 2, Quality::Minor) => 3,
        _ => 0,
    };
    let accidental_penalty = i32::from(c.root.accidental != 0);
    open - accidental_penalty
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(t: &str) -> ChordSymbol {
        ChordSymbol::parse(t).unwrap()
    }
    fn k(t: &str) -> Key {
        Key::parse(t).unwrap()
    }

    #[test]
    fn keys_parse_and_name() {
        assert_eq!(k("G").name(), "G");
        assert_eq!(k("F#m").name(), "F#m");
        assert_eq!(k("Bb major").name(), "Bb");
        assert_eq!(k("C minor").name(), "Cm");
        assert!(Key::parse("Hm").is_none());
        assert!(Key::parse("C lydian").is_none());
    }

    #[test]
    fn key_scales_are_spelled() {
        let names = |t: &str| {
            k(t).scale()
                .unwrap()
                .iter()
                .map(|n| n.name())
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(names("C"), "C D E F G A B");
        assert_eq!(names("Eb"), "Eb F G Ab Bb C D");
        assert_eq!(names("F#"), "F# G# A# B C# D# E#");
        assert_eq!(names("Am"), "A B C D E F G");
        assert_eq!(names("Bbm"), "Bb C Db Eb F Gb Ab");
        assert!(
            Key::new(Note::new(3, -1), false).scale().is_none(),
            "Fb major needs double flats"
        );
    }

    #[test]
    fn chords_follow_the_target_key() {
        // C major up to Eb major: C, F, G become Eb, Ab, Bb.
        let to = Spelling::Key(k("Eb"));
        let names: Vec<String> = ["C", "F", "G7", "Am"]
            .iter()
            .map(|t| transpose_chord(&c(t), 3, to).name())
            .collect();
        assert_eq!(names, vec!["Eb", "Ab", "Bb7", "Cm"]);
        // The same up by a tritone-ish step in sharps: C to F# gives F# and the key's E#.
        let fs = Spelling::Key(k("F#"));
        assert_eq!(transpose_chord(&c("C"), 6, fs).name(), "F#");
        assert_eq!(transpose_chord(&c("B"), 6, fs).name(), "E#");
        assert_eq!(transpose_chord(&c("G"), 6, fs).name(), "C#");
    }

    #[test]
    fn stray_pitch_classes_avoid_exotic_names() {
        // No key: sharps or flats, never E#, B#, Cb or Fb.
        for semis in 0..12 {
            for t in ["C", "D", "E", "F", "G", "A", "B", "C#", "Eb", "F#", "Bb"] {
                for sp in [Spelling::Sharps, Spelling::Flats] {
                    let n = transpose_chord(&c(t), semis, sp).name();
                    for bad in ["E#", "B#", "Cb", "Fb"] {
                        assert!(!n.starts_with(bad), "{t}+{semis} -> {n}");
                    }
                }
            }
        }
        // A non-diatonic chord in a flat key is spelled with flats.
        let to = Spelling::Key(k("F"));
        assert_eq!(transpose_chord(&c("A"), 1, to).name(), "Bb");
        assert_eq!(transpose_chord(&c("C#"), 0, to).name(), "Db");
        // And with sharps in a sharp key.
        assert_eq!(
            transpose_chord(&c("Bb"), 0, Spelling::Key(k("G"))).name(),
            "A#"
        );
    }

    #[test]
    fn slash_bass_moves_with_the_chord() {
        let t = transpose_chord(&c("C/E"), 2, Spelling::Key(k("D")));
        assert_eq!(t.name(), "D/F#");
        let t = transpose_chord(&c("Am7/G"), 3, Spelling::Key(k("Cm")));
        assert_eq!(t.name(), "Cm7/Bb");
    }

    #[test]
    fn transpose_round_trip_is_stable() {
        for t in ["C", "Am7", "F#m7b5", "Bb/D", "G7#9", "Dsus4"] {
            let ch = c(t);
            let up = transpose_chord(&ch, 5, Spelling::Sharps);
            let back = transpose_chord(&up, -5, Spelling::Sharps);
            assert_eq!(back.root.pitch_class(), ch.root.pitch_class(), "{t}");
            assert_eq!(back.quality, ch.quality);
            assert_eq!(back.extension, ch.extension);
        }
        // Twelve semitones is the identity on pitch classes.
        let ch = c("Eb7");
        assert_eq!(
            transpose_chord(&ch, 12, Spelling::Flats).root.pitch_class(),
            ch.root.pitch_class()
        );
    }

    #[test]
    fn transposed_keys() {
        assert_eq!(transpose_key(k("C"), 3).name(), "Eb");
        assert_eq!(transpose_key(k("G"), -2).name(), "F");
        assert_eq!(transpose_key(k("Am"), 2).name(), "Bm");
        assert_eq!(transpose_key(k("Em"), -1).name(), "Ebm");
        assert_eq!(transpose_key(k("C"), 6).name(), "F#");
        assert_eq!(transpose_key(k("F"), 5).name(), "Bb");
    }

    #[test]
    fn capo_shapes() {
        // A song in B-flat with a capo on the third fret is played with G shapes.
        let key = k("Bb");
        assert_eq!(capo_key(key, 3).name(), "G");
        let shapes: Vec<String> = ["Bb", "Eb", "F", "Gm"]
            .iter()
            .map(|t| capo_shape(&c(t), 3, Some(key)).name())
            .collect();
        assert_eq!(shapes, vec!["G", "C", "D", "Em"]);
        // Capo 0 changes nothing.
        assert_eq!(capo_shape(&c("F#m7"), 0, None).name(), "F#m7");
        // No key: sharp chords stay sharp.
        assert_eq!(capo_shape(&c("F#m"), 2, None).name(), "Em");
        assert_eq!(capo_shape(&c("G#"), 1, None).name(), "G");
    }

    #[test]
    fn capo_suggestion_prefers_open_shapes() {
        let chords: Vec<ChordSymbol> = ["Bb", "Eb", "F", "Gm"].iter().map(|t| c(t)).collect();
        let (capo, shapes) = suggest_capo(&chords, 7, Some(k("Bb")));
        assert_eq!(capo, 3);
        assert_eq!(shapes[0].name(), "G");
        let (zero, _) = suggest_capo(&[c("G"), c("C"), c("D")], 7, None);
        assert_eq!(zero, 0);
    }
}
