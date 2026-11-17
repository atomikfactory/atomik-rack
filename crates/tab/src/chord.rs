//! The chord-name grammar: parsing and formatting chord symbols with spelled notes.
//!
//! # Relation to `atomik_model::chord_symbol`
//!
//! The model keeps a small chord reader built on the 21 stamp qualities of
//! `atomik_core::theory::ChordKind` and pitch classes only. This module is the richer grammar the
//! tab features need: notes keep their **spelling** (`Gb` is not `F#`), a chord keeps its
//! extensions, added and altered degrees, omissions and `raw` text. Both agree on every
//! quality the model knows (the tests of this crate check the stamp table); they are not merged
//! yet. The planned unification is for the model to call this parser and map the result to a
//! `ChordKind` with [`ChordSymbol::stamp_intervals`], so there is one grammar; the model may then
//! depend on this crate (never the other way round, which would be a cycle).
//!
//! # Grammar
//!
//! ```text
//! chord   = root [ quality ] [ number ] { part } [ "/" bass ]
//! root    = A..G [ "#" | "b" | "♯" | "♭" ]
//! quality = "m" | "min" | "-" | "maj" | "M" | "Δ" | "dim" | "°" | "o" | "ø" | "aug" | "+"
//!         | "mMaj" | "m(maj" ...
//! number  = "5" | "6" | "69" | "6/9" | "7" | "9" | "11" | "13"
//! part    = "sus2" | "sus4" | "sus" | "add" [acc] n | "no3" | "no5" | acc n | "(" part ")"
//! acc     = "b" | "#" | "♭" | "♯" | "-" | "+"        (alterations: b5 #5 b9 #9 #11 b13)
//! ```
//!
//! The root letter must be upper case. [`ChordSymbol::name`] writes the canonical spelling and
//! reading it back gives an equal chord (equality ignores [`ChordSymbol::raw`]).

use std::fmt;

/// A spelled note: a letter and an accidental.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Note {
    /// The letter, `0..7` for C D E F G A B.
    pub letter: u8,
    /// Sharps (positive) or flats (negative), `-2..=2` in practice.
    pub accidental: i8,
}

const NATURAL_PC: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
const LETTERS: [char; 7] = ['C', 'D', 'E', 'F', 'G', 'A', 'B'];

impl Note {
    /// A note from a letter index (`0..7`, C first) and an accidental.
    pub fn new(letter: u8, accidental: i8) -> Self {
        Self {
            letter: letter % 7,
            accidental,
        }
    }

    /// The pitch class `0..12` (C = 0).
    pub fn pitch_class(self) -> u8 {
        (i32::from(NATURAL_PC[usize::from(self.letter % 7)]) + i32::from(self.accidental))
            .rem_euclid(12) as u8
    }

    /// The spelling of a pitch class with sharps or flats.
    pub fn from_pitch_class(pc: u8, flats: bool) -> Self {
        const SHARP: [(u8, i8); 12] = [
            (0, 0),
            (0, 1),
            (1, 0),
            (1, 1),
            (2, 0),
            (3, 0),
            (3, 1),
            (4, 0),
            (4, 1),
            (5, 0),
            (5, 1),
            (6, 0),
        ];
        const FLAT: [(u8, i8); 12] = [
            (0, 0),
            (1, -1),
            (1, 0),
            (2, -1),
            (2, 0),
            (3, 0),
            (4, -1),
            (4, 0),
            (5, -1),
            (5, 0),
            (6, -1),
            (6, 0),
        ];
        let (l, a) = if flats { FLAT } else { SHARP }[usize::from(pc % 12)];
        Self {
            letter: l,
            accidental: a,
        }
    }

    /// The name: `C`, `F#`, `Bb`, `C##`, `Ebb`.
    pub fn name(self) -> String {
        let mut s = String::with_capacity(3);
        s.push(LETTERS[usize::from(self.letter % 7)]);
        let n = self.accidental.unsigned_abs();
        for _ in 0..n {
            s.push(if self.accidental > 0 { '#' } else { 'b' });
        }
        s
    }

    /// Reads one note name (an upper-case letter and at most one accidental) from the start of
    /// `text` and returns it with the rest.
    pub fn parse_prefix(text: &str) -> Option<(Note, &str)> {
        let mut chars = text.chars();
        let letter = match chars.next()? {
            'C' => 0,
            'D' => 1,
            'E' => 2,
            'F' => 3,
            'G' => 4,
            'A' => 5,
            'B' => 6,
            _ => return None,
        };
        let rest = chars.as_str();
        if let Some(r) = rest.strip_prefix(['#', '\u{266f}']) {
            return Some((Note::new(letter, 1), r));
        }
        if let Some(r) = rest.strip_prefix(['b', '\u{266d}']) {
            return Some((Note::new(letter, -1), r));
        }
        Some((Note::new(letter, 0), rest))
    }

    /// Reads a whole text as one note (no rest allowed).
    pub fn parse(text: &str) -> Option<Note> {
        match Note::parse_prefix(text.trim()) {
            Some((n, "")) => Some(n),
            _ => None,
        }
    }
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// The triad (or the suspension, or the open fifth) under the extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quality {
    /// Major triad (no suffix).
    Major,
    /// Minor triad (`m`).
    Minor,
    /// Diminished triad (`dim`).
    Dim,
    /// Augmented triad (`aug`).
    Aug,
    /// Suspended second.
    Sus2,
    /// Suspended fourth.
    Sus4,
    /// Root and fifth (`5`).
    Power,
    /// Half-diminished: a minor third, a flat fifth and a minor seventh (`m7b5`, `ø`).
    HalfDim,
}

/// The stacked extension of a chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Extension {
    /// A plain triad.
    None,
    /// Added sixth (`6`).
    Six,
    /// Sixth and added ninth (`69`).
    SixNine,
    /// Seventh (dominant, minor or diminished by the quality).
    Seventh,
    /// Major seventh (`maj7`).
    MajSeventh,
    /// Ninth (`9`).
    Ninth,
    /// Major ninth (`maj9`).
    MajNinth,
    /// Eleventh (`11`).
    Eleventh,
    /// Major eleventh (`maj11`).
    MajEleventh,
    /// Thirteenth (`13`).
    Thirteenth,
    /// Major thirteenth (`maj13`).
    MajThirteenth,
}

/// A degree with an accidental: `b9`, `#11`, or (with accidental 0) an added `9`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Degree {
    /// `-1` flat, `0` natural, `1` sharp.
    pub accidental: i8,
    /// The scale degree: 2, 4, 5, 6, 9, 11 or 13.
    pub degree: u8,
}

impl Degree {
    /// Semitones above the root.
    pub fn semitones(self) -> i32 {
        let base = match self.degree {
            2 | 9 => 2,
            4 | 11 => 5,
            5 => 7,
            6 | 13 => 9,
            _ => 0,
        };
        base + i32::from(self.accidental)
    }
}

/// What a chord tone is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// The root.
    Root,
    /// The third (or the suspension that replaces it).
    Third,
    /// The fifth, natural or altered.
    Fifth,
    /// The sixth.
    Sixth,
    /// The seventh.
    Seventh,
    /// The ninth.
    Ninth,
    /// The eleventh.
    Eleventh,
    /// The thirteenth.
    Thirteenth,
    /// The bass of a slash chord when it is not otherwise a chord tone.
    Bass,
}

/// A pitch class of a chord with its role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChordTone {
    /// The pitch class `0..12`.
    pub pc: u8,
    /// Semitones above the root (`0..24`).
    pub interval: u8,
    /// The role.
    pub role: Role,
    /// Whether a voicing should not leave it out (the root, third, sixth, seventh, the top
    /// extension, every altered or added degree). The fifth and inner extensions are optional.
    pub essential: bool,
}

/// Why a chord symbol could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChordError {
    /// Nothing was typed.
    #[error("type a chord such as Am7 or C/G")]
    Empty,
    /// The text does not start with a note name.
    #[error("{0:?} does not start with a note name (A to G)")]
    BadRoot(String),
    /// Something after the root is not part of the chord grammar.
    #[error("unknown chord suffix {0:?}")]
    BadSuffix(String),
    /// The note after the slash is not a note name.
    #[error("{0:?} is not a bass note (A to G)")]
    BadBass(String),
    /// The text is too long to be a chord.
    #[error("a chord name is at most {MAX_CHORD_LEN} characters")]
    TooLong,
}

/// The longest chord name accepted.
pub const MAX_CHORD_LEN: usize = 48;

/// A chord as a symbol means it.
#[derive(Debug, Clone)]
pub struct ChordSymbol {
    /// The spelled root.
    pub root: Note,
    /// The triad.
    pub quality: Quality,
    /// The stacked extension.
    pub extension: Extension,
    /// Altered degrees (`b5 #5 b9 #9 #11 b13`) in written order.
    pub alterations: Vec<Degree>,
    /// Added degrees (`add9`, `add11`) in written order.
    pub adds: Vec<Degree>,
    /// The third is omitted (`no3`).
    pub no3: bool,
    /// The fifth is omitted (`no5`).
    pub no5: bool,
    /// The spelled bass of a slash chord (never the root).
    pub bass: Option<Note>,
    /// The text the chord was read from (empty when built in code). Ignored by equality.
    pub raw: String,
}

impl PartialEq for ChordSymbol {
    fn eq(&self, o: &Self) -> bool {
        self.root == o.root
            && self.quality == o.quality
            && self.extension == o.extension
            && self.alterations == o.alterations
            && self.adds == o.adds
            && self.no3 == o.no3
            && self.no5 == o.no5
            && self.bass == o.bass
    }
}

impl Eq for ChordSymbol {}

impl std::hash::Hash for ChordSymbol {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.root.hash(h);
        self.quality.hash(h);
        self.extension.hash(h);
        self.alterations.hash(h);
        self.adds.hash(h);
        self.no3.hash(h);
        self.no5.hash(h);
        self.bass.hash(h);
    }
}

impl fmt::Display for ChordSymbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// Reads the longest number (`69`, `13`, `11`, `9`, `7`, `6`, `5`) at the start of `s`.
fn take_number(s: &str) -> Option<(u8, &str)> {
    for (text, n) in [
        ("69", 69),
        ("13", 13),
        ("11", 11),
        ("9", 9),
        ("7", 7),
        ("6", 6),
        ("5", 5),
    ] {
        if let Some(r) = s.strip_prefix(text) {
            return Some((n, r));
        }
    }
    None
}

fn take_major_marker(s: &str) -> Option<&str> {
    for p in ["major", "Major", "maj", "Maj", "MAJ", "\u{394}", "\u{2206}"] {
        if let Some(r) = s.strip_prefix(p) {
            return Some(r);
        }
    }
    if let Some(r) = s.strip_prefix('M')
        && r.chars()
            .next()
            .is_none_or(|c| c.is_ascii_digit() || c == '(' || c == ')')
    {
        return Some(r);
    }
    None
}

impl ChordSymbol {
    /// A plain chord on `root`.
    pub fn new(root: Note, quality: Quality, extension: Extension) -> Self {
        Self {
            root,
            quality,
            extension,
            alterations: Vec::new(),
            adds: Vec::new(),
            no3: false,
            no5: false,
            bass: None,
            raw: String::new(),
        }
    }

    /// Reads a chord symbol such as `Am7`, `F#m7b5`, `Bb6/9`, `C7(#9)`, `Dsus4/A` or `Cmaj13`.
    pub fn parse(text: &str) -> Result<ChordSymbol, ChordError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ChordError::Empty);
        }
        if text.chars().count() > MAX_CHORD_LEN {
            return Err(ChordError::TooLong);
        }
        let (root, after_root) = Note::parse_prefix(text)
            .ok_or_else(|| ChordError::BadRoot(text.chars().take(8).collect()))?;
        // `6/9` is a quality, not a slash chord.
        let normalised = after_root.replace("6/9", "69");
        let (suffix, bass_text) = match normalised.rsplit_once('/') {
            Some((h, b)) => (h.to_owned(), Some(b.trim().to_owned())),
            None => (normalised, None),
        };
        let bass = match bass_text {
            None => None,
            Some(b) => match Note::parse(&b) {
                Some(n) => Some(n),
                None => return Err(ChordError::BadBass(b)),
            },
        };
        let mut c = ChordSymbol::new(root, Quality::Major, Extension::None);
        c.parse_suffix(suffix.trim())?;
        c.bass = bass.filter(|b| b.pitch_class() != root.pitch_class());
        c.raw = text.to_owned();
        Ok(c)
    }

    fn parse_suffix(&mut self, full: &str) -> Result<(), ChordError> {
        let bad = || ChordError::BadSuffix(full.to_owned());
        let mut s = full;
        let mut major_marker = false;
        // 1. The quality prefix.
        if let Some(r) = take_major_marker(s) {
            major_marker = true;
            s = r;
        } else if let Some(r) = ["min", "Min", "mi", "m", "-"]
            .iter()
            .find_map(|p| s.strip_prefix(p))
        {
            self.quality = Quality::Minor;
            s = r;
        } else if let Some(r) = ["dim", "\u{b0}", "o"]
            .iter()
            .find_map(|p| s.strip_prefix(p))
        {
            self.quality = Quality::Dim;
            s = r;
        } else if let Some(r) = ["\u{f8}", "\u{d8}"].iter().find_map(|p| s.strip_prefix(p)) {
            self.quality = Quality::HalfDim;
            s = r;
        } else if let Some(r) = ["aug", "+"].iter().find_map(|p| s.strip_prefix(p)) {
            self.quality = Quality::Aug;
            s = r;
        }
        // `mMaj7`, `m(maj7)`, `augmaj7`, `dim(maj7)`.
        if matches!(self.quality, Quality::Minor | Quality::Dim | Quality::Aug) && !major_marker {
            let inner = s.strip_prefix('(').unwrap_or(s);
            if let Some(r) = take_major_marker(inner) {
                major_marker = true;
                s = r;
            }
        }
        // 2. The number.
        let mut number = None;
        if let Some((n, r)) = take_number(s) {
            number = Some(n);
            s = r;
        }
        self.extension = match (number, self.quality, major_marker) {
            (None, Quality::Minor | Quality::Dim | Quality::Aug, true) => Extension::MajSeventh,
            (None, Quality::HalfDim, _) => Extension::Seventh,
            (None, _, _) => Extension::None,
            (Some(5), Quality::Major, false) => {
                self.quality = Quality::Power;
                Extension::None
            }
            (Some(5), _, _) => return Err(bad()),
            (Some(6), _, false) => Extension::Six,
            (Some(69), _, false) => Extension::SixNine,
            (Some(7), _, false) => Extension::Seventh,
            (Some(7), _, true) => Extension::MajSeventh,
            (Some(9), _, false) => Extension::Ninth,
            (Some(9), _, true) => Extension::MajNinth,
            (Some(11), _, false) => Extension::Eleventh,
            (Some(11), _, true) => Extension::MajEleventh,
            (Some(13), _, false) => Extension::Thirteenth,
            (Some(13), _, true) => Extension::MajThirteenth,
            _ => return Err(bad()),
        };
        // 3. Parts: suspensions, additions, omissions and alterations, in any order.
        let mut guard = 0;
        loop {
            guard += 1;
            s = s.trim_start_matches([' ', '(', ')', ',']);
            if s.is_empty() || guard > 32 {
                break;
            }
            if let Some(r) = s.strip_prefix("sus") {
                s = r;
                let (q, r) = if let Some(r) = s.strip_prefix('2') {
                    (Quality::Sus2, r)
                } else if let Some(r) = s.strip_prefix('4') {
                    (Quality::Sus4, r)
                } else {
                    (Quality::Sus4, s)
                };
                if self.quality != Quality::Major {
                    return Err(bad());
                }
                self.quality = q;
                s = r;
            } else if let Some(r) = s.strip_prefix("add") {
                let (acc, r) = take_accidental(r);
                let (deg, r) = take_degree(r).ok_or_else(bad)?;
                self.adds.push(Degree {
                    accidental: acc,
                    degree: deg,
                });
                s = r;
            } else if let Some(r) = s.strip_prefix("no3").or_else(|| s.strip_prefix("omit3")) {
                self.no3 = true;
                s = r;
            } else if let Some(r) = s.strip_prefix("no5").or_else(|| s.strip_prefix("omit5")) {
                self.no5 = true;
                s = r;
            } else {
                let (acc, r) = take_accidental(s);
                let (deg, r) = take_degree(r).ok_or_else(bad)?;
                if acc == 0 {
                    // A bare degree in brackets (`C7(9)`): an added tone.
                    if !matches!(deg, 9 | 11 | 13) {
                        return Err(bad());
                    }
                    self.adds.push(Degree {
                        accidental: 0,
                        degree: deg,
                    });
                } else {
                    if !matches!(deg, 5 | 9 | 11 | 13) {
                        return Err(bad());
                    }
                    self.alterations.push(Degree {
                        accidental: acc,
                        degree: deg,
                    });
                }
                s = r;
            }
        }
        // 4. m7b5 is the half-diminished chord.
        if self.quality == Quality::Minor
            && matches!(
                self.extension,
                Extension::Seventh | Extension::Ninth | Extension::Eleventh | Extension::Thirteenth
            )
            && let Some(i) = self
                .alterations
                .iter()
                .position(|d| d.degree == 5 && d.accidental == -1)
        {
            self.alterations.remove(i);
            self.quality = Quality::HalfDim;
        }
        Ok(())
    }

    /// The pitch class of the root.
    pub fn root_pc(&self) -> u8 {
        self.root.pitch_class()
    }

    /// The pitch class of the bass (the root unless it is a slash chord).
    pub fn bass_pc(&self) -> u8 {
        self.bass.unwrap_or(self.root).pitch_class()
    }

    /// The canonical spelling: `C`, `Am7`, `F#m7b5`, `Bb6/9` is written `Bb69`, `C7b9/E`.
    pub fn name(&self) -> String {
        let mut s = self.root.name();
        s.push_str(&self.suffix());
        if let Some(b) = self.bass {
            s.push('/');
            s.push_str(&b.name());
        }
        s
    }

    /// The suffix without the root and the bass.
    pub fn suffix(&self) -> String {
        let mut s = String::new();
        let num = match self.extension {
            Extension::None => "",
            Extension::Six => "6",
            Extension::SixNine => "69",
            Extension::Seventh => "7",
            Extension::MajSeventh => "maj7",
            Extension::Ninth => "9",
            Extension::MajNinth => "maj9",
            Extension::Eleventh => "11",
            Extension::MajEleventh => "maj11",
            Extension::Thirteenth => "13",
            Extension::MajThirteenth => "maj13",
        };
        match self.quality {
            Quality::Major => s.push_str(num),
            Quality::Minor => {
                s.push('m');
                if self.extension == Extension::MajSeventh {
                    s.push_str("Maj7");
                } else if let Some(m) = num.strip_prefix("maj") {
                    s.push_str("Maj");
                    s.push_str(m);
                } else {
                    s.push_str(num);
                }
            }
            Quality::Dim => {
                s.push_str("dim");
                s.push_str(num);
            }
            Quality::Aug => {
                s.push_str("aug");
                s.push_str(num);
            }
            Quality::Power => s.push('5'),
            Quality::Sus2 => {
                s.push_str(num);
                s.push_str("sus2");
            }
            Quality::Sus4 => {
                s.push_str(num);
                s.push_str("sus4");
            }
            Quality::HalfDim => {
                s.push('m');
                s.push_str(if num.is_empty() { "7" } else { num });
                s.push_str("b5");
            }
        }
        // Alterations right after the root would read as part of it (`Cb9`): bracket them.
        let wrap = s.is_empty() && !self.alterations.is_empty();
        if wrap {
            s.push('(');
        }
        for d in &self.alterations {
            s.push(if d.accidental < 0 { 'b' } else { '#' });
            s.push_str(&d.degree.to_string());
        }
        if wrap {
            s.push(')');
        }
        for d in &self.adds {
            s.push_str("add");
            match d.accidental.signum() {
                -1 => s.push('b'),
                1 => s.push('#'),
                _ => {}
            }
            s.push_str(&d.degree.to_string());
        }
        if self.no3 {
            s.push_str("no3");
        }
        if self.no5 {
            s.push_str("no5");
        }
        s
    }

    /// The chord tones with their roles, root first, ascending by role.
    pub fn tones(&self) -> Vec<ChordTone> {
        let mut out: Vec<ChordTone> = Vec::with_capacity(8);
        let root = i32::from(self.root_pc());
        add(&mut out, root, 0, Role::Root, true);
        if !self.no3 {
            match self.quality {
                Quality::Major | Quality::Aug => add(&mut out, root, 4, Role::Third, true),
                Quality::Minor | Quality::Dim | Quality::HalfDim => {
                    add(&mut out, root, 3, Role::Third, true)
                }
                Quality::Sus2 => add(&mut out, root, 2, Role::Third, true),
                Quality::Sus4 => add(&mut out, root, 5, Role::Third, true),
                Quality::Power => {}
            }
        }
        // The fifth: an alteration of degree 5 replaces it.
        let alt5 = self.alterations.iter().find(|d| d.degree == 5).copied();
        if let Some(d) = alt5 {
            add(
                &mut out,
                root,
                7 + i32::from(d.accidental),
                Role::Fifth,
                true,
            );
        } else if !self.no5 {
            let f = match self.quality {
                Quality::Dim | Quality::HalfDim => 6,
                Quality::Aug => 8,
                _ => 7,
            };
            add(&mut out, root, f, Role::Fifth, false);
        }
        let seventh = if self.quality == Quality::Dim { 9 } else { 10 };
        match self.extension {
            Extension::None => {}
            Extension::Six => add(&mut out, root, 9, Role::Sixth, true),
            Extension::SixNine => {
                add(&mut out, root, 9, Role::Sixth, true);
                add(&mut out, root, 14, Role::Ninth, true);
            }
            Extension::Seventh => add(&mut out, root, seventh, Role::Seventh, true),
            Extension::MajSeventh => add(&mut out, root, 11, Role::Seventh, true),
            Extension::Ninth | Extension::MajNinth => {
                let sev = if self.extension == Extension::Ninth {
                    seventh
                } else {
                    11
                };
                add(&mut out, root, sev, Role::Seventh, true);
                add(&mut out, root, 14, Role::Ninth, true);
            }
            Extension::Eleventh | Extension::MajEleventh => {
                let sev = if self.extension == Extension::Eleventh {
                    seventh
                } else {
                    11
                };
                add(&mut out, root, sev, Role::Seventh, true);
                add(&mut out, root, 14, Role::Ninth, false);
                add(&mut out, root, 17, Role::Eleventh, true);
            }
            Extension::Thirteenth | Extension::MajThirteenth => {
                let sev = if self.extension == Extension::Thirteenth {
                    seventh
                } else {
                    11
                };
                add(&mut out, root, sev, Role::Seventh, true);
                add(&mut out, root, 14, Role::Ninth, false);
                add(&mut out, root, 21, Role::Thirteenth, true);
            }
        }
        for d in self
            .adds
            .iter()
            .chain(self.alterations.iter().filter(|d| d.degree != 5))
        {
            let role = match d.degree {
                2 | 9 => Role::Ninth,
                4 | 11 => Role::Eleventh,
                _ => Role::Thirteenth,
            };
            let natural = Degree {
                accidental: 0,
                degree: d.degree,
            }
            .semitones();
            if d.accidental != 0 {
                // An altered degree replaces the natural one.
                let natural_pc = (root + natural).rem_euclid(12) as u8;
                out.retain(|t| !(t.pc == natural_pc && t.role == role));
            }
            add(&mut out, root, d.semitones(), role, true);
        }
        if let Some(b) = self.bass {
            let pc = b.pitch_class();
            if !out.iter().any(|t| t.pc == pc) {
                let interval = (i32::from(pc) - root).rem_euclid(12);
                out.push(ChordTone {
                    pc,
                    interval: interval as u8,
                    role: Role::Bass,
                    essential: true,
                });
            }
        }
        out.sort_by_key(|t| (t.role, t.interval));
        out
    }

    /// The semitone offsets of the chord above its root, ascending, without the bass note: what
    /// a stamp-style chord (`atomik_core::theory::ChordKind::intervals`) would list.
    pub fn stamp_intervals(&self) -> Vec<u8> {
        let mut v: Vec<u8> = self
            .tones()
            .iter()
            .filter(|t| t.role != Role::Bass)
            .map(|t| t.interval)
            .collect();
        v.sort_unstable();
        v
    }

    /// The pitch classes of the chord.
    pub fn pitch_classes(&self) -> Vec<u8> {
        self.tones().iter().map(|t| t.pc).collect()
    }
}

/// Adds a tone `interval` semitones above `root`; a pitch class already there only gains
/// importance.
fn add(out: &mut Vec<ChordTone>, root: i32, interval: i32, role: Role, essential: bool) {
    let pc = (root + interval).rem_euclid(12) as u8;
    if let Some(t) = out.iter_mut().find(|t| t.pc == pc) {
        t.essential |= essential;
        return;
    }
    out.push(ChordTone {
        pc,
        interval: interval.rem_euclid(24) as u8,
        role,
        essential,
    });
}

fn take_accidental(s: &str) -> (i8, &str) {
    for (p, a) in [
        ("b", -1),
        ("\u{266d}", -1),
        ("-", -1),
        ("#", 1),
        ("\u{266f}", 1),
        ("+", 1),
    ] {
        if let Some(r) = s.strip_prefix(p) {
            return (a, r);
        }
    }
    (0, s)
}

fn take_degree(s: &str) -> Option<(u8, &str)> {
    for (t, n) in [
        ("13", 13),
        ("11", 11),
        ("9", 9),
        ("6", 6),
        ("5", 5),
        ("4", 4),
        ("2", 2),
    ] {
        if let Some(r) = s.strip_prefix(t) {
            return Some((n, r));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(t: &str) -> ChordSymbol {
        ChordSymbol::parse(t).unwrap_or_else(|e| panic!("{t:?}: {e}"))
    }

    #[test]
    fn plain_triads_and_sevenths() {
        assert_eq!(p("C").name(), "C");
        assert_eq!(p("Am").quality, Quality::Minor);
        assert_eq!(p("Amin").name(), "Am");
        assert_eq!(p("A-").name(), "Am");
        assert_eq!(p("Cmaj").name(), "C");
        assert_eq!(p("CM").name(), "C");
        assert_eq!(p("Cdim").name(), "Cdim");
        assert_eq!(p("C\u{b0}").name(), "Cdim");
        assert_eq!(p("Caug").name(), "Caug");
        assert_eq!(p("C+").name(), "Caug");
        assert_eq!(p("G7").name(), "G7");
        assert_eq!(p("Gmaj7").name(), "Gmaj7");
        assert_eq!(p("GM7").name(), "Gmaj7");
        assert_eq!(p("G\u{394}").name(), "G");
        assert_eq!(p("G\u{394}7").name(), "Gmaj7");
        assert_eq!(p("Am7").name(), "Am7");
        assert_eq!(p("Am7b5").name(), "Am7b5");
        assert_eq!(p("A\u{f8}").name(), "Am7b5");
        assert_eq!(p("A\u{f8}7").name(), "Am7b5");
        assert_eq!(p("Adim7").name(), "Adim7");
        assert_eq!(p("AmMaj7").name(), "AmMaj7");
        assert_eq!(p("Am(maj7)").name(), "AmMaj7");
        assert_eq!(p("AmM7").name(), "AmMaj7");
    }

    #[test]
    fn extensions_suspensions_additions() {
        assert_eq!(p("C5").quality, Quality::Power);
        assert_eq!(p("C6").name(), "C6");
        assert_eq!(p("Cm6").name(), "Cm6");
        assert_eq!(p("C69").name(), "C69");
        assert_eq!(p("C6/9").name(), "C69");
        assert_eq!(p("C9").name(), "C9");
        assert_eq!(p("Cmaj9").name(), "Cmaj9");
        assert_eq!(p("Cm9").name(), "Cm9");
        assert_eq!(p("C11").name(), "C11");
        assert_eq!(p("C13").name(), "C13");
        assert_eq!(p("Cmaj13").name(), "Cmaj13");
        assert_eq!(p("Csus2").name(), "Csus2");
        assert_eq!(p("Csus4").name(), "Csus4");
        assert_eq!(p("Csus").name(), "Csus4");
        assert_eq!(p("C7sus4").name(), "C7sus4");
        assert_eq!(p("Cadd9").name(), "Cadd9");
        assert_eq!(p("Cmadd9").name(), "Cmadd9");
        assert_eq!(p("Cno3").name(), "Cno3");
        assert_eq!(p("C7no5").name(), "C7no5");
    }

    #[test]
    fn alterations_and_brackets() {
        assert_eq!(p("C7b9").name(), "C7b9");
        assert_eq!(p("C7#9").name(), "C7#9");
        assert_eq!(p("C7#11").name(), "C7#11");
        assert_eq!(p("C7b13").name(), "C7b13");
        assert_eq!(p("C7b5").name(), "C7b5");
        assert_eq!(p("C7#5").name(), "C7#5");
        assert_eq!(p("C7(b9)").name(), "C7b9");
        assert_eq!(p("C7(b9,#11)").name(), "C7b9#11");
        assert_eq!(p("C7-9").name(), "C7b9");
        assert_eq!(p("C+7").name(), "Caug7");
        assert_eq!(p("C7(9)").name(), "C7add9");
        assert_eq!(p("C9(#11)").name(), "C9#11");
    }

    #[test]
    fn slash_chords_and_spelling() {
        let c = p("C/G");
        assert_eq!(c.bass, Some(Note::new(4, 0)));
        assert_eq!(c.name(), "C/G");
        assert_eq!(p("D/F#").name(), "D/F#");
        assert_eq!(p("Dsus4/A").name(), "Dsus4/A");
        assert_eq!(p("C7b9/E").name(), "C7b9/E");
        assert_eq!(p("C/C").bass, None, "a bass equal to the root is dropped");
        assert_eq!(p("Bb").name(), "Bb");
        assert_eq!(p("Bb").root_pc(), 10);
        assert_eq!(p("A#").name(), "A#");
        assert_eq!(p("A#").root_pc(), 10);
        assert_ne!(p("Bb"), p("A#"), "spelling is kept");
        assert_eq!(p("F\u{266f}m").name(), "F#m");
        assert_eq!(p("B\u{266d}").name(), "Bb");
        assert!(
            ChordSymbol::parse("Ebb").is_err(),
            "a double flat is not a root"
        );
    }

    #[test]
    fn errors() {
        assert_eq!(ChordSymbol::parse(""), Err(ChordError::Empty));
        assert!(matches!(
            ChordSymbol::parse("Hm"),
            Err(ChordError::BadRoot(_))
        ));
        assert!(matches!(
            ChordSymbol::parse("am"),
            Err(ChordError::BadRoot(_))
        ));
        assert!(matches!(
            ChordSymbol::parse("Cxyz"),
            Err(ChordError::BadSuffix(_))
        ));
        assert!(matches!(
            ChordSymbol::parse("C/H"),
            Err(ChordError::BadBass(_))
        ));
        assert!(matches!(
            ChordSymbol::parse("Cm5"),
            Err(ChordError::BadSuffix(_))
        ));
        assert!(matches!(
            ChordSymbol::parse("Cmsus4"),
            Err(ChordError::BadSuffix(_))
        ));
        assert_eq!(
            ChordSymbol::parse(&"C".repeat(100)),
            Err(ChordError::TooLong)
        );
        assert!(ChordSymbol::parse("the").is_err());
        assert!(ChordSymbol::parse("Am7 hello").is_err());
    }

    #[test]
    fn tones_follow_the_symbol() {
        let pcs = |t: &str| {
            let mut v = p(t).pitch_classes();
            v.sort_unstable();
            v
        };
        assert_eq!(pcs("C"), vec![0, 4, 7]);
        assert_eq!(pcs("Am"), vec![0, 4, 9]);
        assert_eq!(pcs("G7"), vec![2, 5, 7, 11]);
        assert_eq!(pcs("Cmaj7"), vec![0, 4, 7, 11]);
        assert_eq!(pcs("Bm7b5"), vec![2, 5, 9, 11]);
        assert_eq!(pcs("Cdim7"), vec![0, 3, 6, 9]);
        assert_eq!(pcs("C7b9"), vec![0, 1, 4, 7, 10]);
        assert_eq!(pcs("C7#9"), vec![0, 3, 4, 7, 10]);
        assert_eq!(pcs("C7b5"), vec![0, 4, 6, 10]);
        assert_eq!(pcs("C7#5"), vec![0, 4, 8, 10]);
        assert_eq!(pcs("C5"), vec![0, 7]);
        assert_eq!(pcs("Csus4"), vec![0, 5, 7]);
        assert_eq!(pcs("Cno3"), vec![0, 7]);
        assert_eq!(pcs("C/E"), vec![0, 4, 7]);
        assert_eq!(pcs("C/D"), vec![0, 2, 4, 7]);
        assert_eq!(pcs("C69"), vec![0, 2, 4, 7, 9]);
        let tones = p("C7").tones();
        assert!(
            tones
                .iter()
                .find(|t| t.role == Role::Fifth)
                .is_some_and(|t| !t.essential)
        );
        assert!(
            tones
                .iter()
                .find(|t| t.role == Role::Seventh)
                .is_some_and(|t| t.essential)
        );
    }

    #[test]
    fn stamp_intervals_agree_with_the_core_table() {
        use atomik_core::theory::ChordKind;
        // The suffixes of the stamp kinds, read by this grammar, give the same notes (modulo
        // octave: the core table lists ninths at 14, this one at 14 as well).
        let table = [
            ("", ChordKind::Major),
            ("m", ChordKind::Minor),
            ("dim", ChordKind::Dim),
            ("aug", ChordKind::Aug),
            ("sus2", ChordKind::Sus2),
            ("sus4", ChordKind::Sus4),
            ("6", ChordKind::Sixth),
            ("m6", ChordKind::MinorSixth),
            ("7", ChordKind::Seventh),
            ("maj7", ChordKind::MajorSeventh),
            ("m7", ChordKind::MinorSeventh),
            ("m7b5", ChordKind::HalfDim),
            ("dim7", ChordKind::DimSeventh),
            ("9", ChordKind::Ninth),
            ("maj9", ChordKind::MajorNinth),
            ("m9", ChordKind::MinorNinth),
            ("add9", ChordKind::Add9),
            ("5", ChordKind::Power),
        ];
        for (suffix, kind) in table {
            let c = p(&format!("C{suffix}"));
            let mut want: Vec<u8> = kind.intervals().iter().map(|i| i % 12).collect();
            want.sort_unstable();
            want.dedup();
            let mut got: Vec<u8> = c.stamp_intervals().iter().map(|i| i % 12).collect();
            got.sort_unstable();
            got.dedup();
            assert_eq!(got, want, "{suffix}");
        }
    }

    #[test]
    fn round_trip_every_combination() {
        let roots = ["C", "F#", "Bb", "E", "Ab"];
        let suffixes = [
            "", "m", "dim", "aug", "5", "6", "m6", "69", "7", "maj7", "m7", "mMaj7", "m7b5",
            "dim7", "9", "maj9", "m9", "11", "m11", "13", "maj13", "sus2", "sus4", "7sus4", "add9",
            "madd9", "7b9", "7#9", "7#11", "7b13", "7b5", "7#5", "9#11", "13b9", "no3", "7no5",
            "maj7#11", "aug7", "m7add11", "7b9#9",
        ];
        for r in roots {
            for s in suffixes {
                for bass in ["", "/G", "/Bb", "/F#"] {
                    let text = format!("{r}{s}{bass}");
                    let c = p(&text);
                    let again = p(&c.name());
                    assert_eq!(c, again, "{text} -> {}", c.name());
                }
            }
        }
    }

    #[test]
    fn note_spelling_helpers() {
        assert_eq!(Note::from_pitch_class(10, true).name(), "Bb");
        assert_eq!(Note::from_pitch_class(10, false).name(), "A#");
        assert_eq!(Note::from_pitch_class(0, true).name(), "C");
        for pc in 0..12 {
            assert_eq!(Note::from_pitch_class(pc, true).pitch_class(), pc);
            assert_eq!(Note::from_pitch_class(pc, false).pitch_class(), pc);
        }
        assert_eq!(Note::new(0, -1).pitch_class(), 11, "Cb is B");
        assert_eq!(Note::new(2, 1).pitch_class(), 5, "E# is F");
    }
}
