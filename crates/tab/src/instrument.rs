//! Fretted instruments as data: strings, tunings, capos and the built-in catalogue.
//!
//! # String order
//!
//! Strings are listed in **tab order**: index 0 is the top line of a tab staff, which is the
//! highest-sounding course of the standard layout, and the last index is the bottom line. Every
//! converter in this crate maps to that order. Never assume the strings are sorted by pitch:
//! re-entrant tunings (the ukulele's high G, the banjo's short drone string, the charango) break
//! the rule on purpose, and [`Tuning::is_re_entrant`] says so.
//!
//! # Frets and capos
//!
//! A fret number in tab is **relative to the capo**: fret 0 is the open string as the capo leaves
//! it, fret 1 is the first fret above the capo. [`Tuning::pitch`] and [`Tuning::fret_for_pitch`]
//! take the capo into account. A string with a `start_fret` above 0 (the banjo's short fifth
//! string) has no frets below that position: it can sound open or at frets `>= start_fret`
//! counted from the nut; with a capo of `c` frets the first playable relative fret is
//! `start_fret - c` (at least 1). A global capo raises every string by `c` semitones, the short
//! string included (the player retunes it or uses a spike).

use std::sync::OnceLock;

/// The highest string count a tuning may have.
pub const MAX_STRINGS: usize = 12;
/// The highest fret count a tuning may have.
pub const MAX_FRETS: u8 = 36;
/// The highest capo position.
pub const MAX_CAPO: u8 = 24;

/// One string (or one course of two strings) of an instrument.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StringDef {
    /// The open pitch as a MIDI key (middle C = 60).
    pub open: u8,
    /// The first fret the string can be pressed at, counted from the nut: 0 for an ordinary
    /// string, 5 for the banjo's short drone string. Open (fret 0) always plays.
    pub start_fret: u8,
    /// Strings in the course: 1 (single) or 2 (a doubled course of a mandolin or 12-string).
    pub course: u8,
    /// Whether the second string of the course sounds an octave above (12-string guitar, the
    /// lower courses of an octave mandolin) instead of in unison.
    pub octave_course: bool,
    /// A short label for the string (`E`, `g`), shown in tab and diagrams.
    pub label: String,
}

impl StringDef {
    /// An ordinary single string on `open` (the label is its note name).
    pub fn new(open: u8) -> Self {
        Self {
            open,
            start_fret: 0,
            course: 1,
            octave_course: false,
            label: pitch_class_name(open).to_owned(),
        }
    }

    /// The same string with a first playable fret (a short string).
    pub fn with_start_fret(mut self, fret: u8) -> Self {
        self.start_fret = fret;
        self
    }

    /// The same string as a doubled course, in unison or with an octave pair.
    pub fn doubled(mut self, octave: bool) -> Self {
        self.course = 2;
        self.octave_course = octave;
        self
    }

    /// The same string with another label.
    pub fn labelled(mut self, label: &str) -> Self {
        self.label = label.to_owned();
        self
    }
}

/// Why a custom tuning was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TuningError {
    /// No strings, or more than [`MAX_STRINGS`].
    #[error("a tuning needs 1 to {MAX_STRINGS} strings, not {0}")]
    StringCount(usize),
    /// The fret count is 0 or above [`MAX_FRETS`].
    #[error("a neck needs 1 to {MAX_FRETS} frets, not {0}")]
    FretCount(u8),
    /// A string is outside the MIDI range once its top fret is added.
    #[error("string {0} reaches outside the MIDI range")]
    PitchRange(usize),
    /// A string starts at a fret the neck does not have.
    #[error("string {0} starts at a fret beyond the neck")]
    StartFret(usize),
    /// A course that is not 1 or 2 strings.
    #[error("string {0} has {1} strings in its course (1 or 2 are possible)")]
    Course(usize, u8),
}

/// A capo: the fret it clamps, and which strings it covers.
///
/// `strings` is a bit mask over tab-order string indices (bit 0 = the top line); `None` covers
/// every string. A partial capo leaves the other strings as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Capo {
    /// The capo fret; 0 means no capo.
    pub fret: u8,
    /// The covered strings (bit `i` = string `i`), or `None` for all of them.
    pub strings: Option<u16>,
}

impl Capo {
    /// No capo.
    pub const NONE: Capo = Capo {
        fret: 0,
        strings: None,
    };

    /// A capo across the whole neck at `fret`.
    pub fn at(fret: u8) -> Self {
        Self {
            fret: fret.min(MAX_CAPO),
            strings: None,
        }
    }

    /// A capo at `fret` over the strings of `mask` only.
    pub fn partial(fret: u8, mask: u16) -> Self {
        Self {
            fret: fret.min(MAX_CAPO),
            strings: Some(mask),
        }
    }

    /// How many frets the capo raises string `string` (0 when it is not covered).
    pub fn raise(&self, string: usize) -> u8 {
        match self.strings {
            None => self.fret,
            Some(mask) if string < 16 && mask & (1 << string) != 0 => self.fret,
            Some(_) => 0,
        }
    }
}

/// The strings of an instrument and the fret count of its neck.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tuning {
    /// The strings in tab order (index 0 = top line).
    pub strings: Vec<StringDef>,
    /// The number of frets of the neck, counted from the nut.
    pub frets: u8,
}

impl Tuning {
    /// A tuning from open pitches in tab order, with ordinary strings.
    pub fn from_opens(opens: &[u8], frets: u8) -> Self {
        Self {
            strings: opens.iter().map(|o| StringDef::new(*o)).collect(),
            frets,
        }
    }

    /// The number of strings.
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// Whether there are no strings.
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    /// Checks a user-defined tuning: 1 to 12 strings, sane frets, pitches inside MIDI.
    pub fn validate(&self) -> Result<(), TuningError> {
        let n = self.strings.len();
        if n == 0 || n > MAX_STRINGS {
            return Err(TuningError::StringCount(n));
        }
        if self.frets == 0 || self.frets > MAX_FRETS {
            return Err(TuningError::FretCount(self.frets));
        }
        for (i, s) in self.strings.iter().enumerate() {
            if s.course == 0 || s.course > 2 {
                return Err(TuningError::Course(i, s.course));
            }
            if s.start_fret > self.frets {
                return Err(TuningError::StartFret(i));
            }
            let top =
                u16::from(s.open) + u16::from(self.frets) + if s.octave_course { 12 } else { 0 };
            if s.open > 127 || top > 127 {
                return Err(TuningError::PitchRange(i));
            }
        }
        Ok(())
    }

    /// The open pitch of `string` with `capo` applied.
    pub fn open_pitch(&self, string: usize, capo: Capo) -> Option<u8> {
        let s = self.strings.get(string)?;
        let p = u16::from(s.open) + u16::from(capo.raise(string));
        u8::try_from(p).ok().filter(|p| *p <= 127)
    }

    /// The sounding pitch of `string` at the relative `fret` (0 = open) with `capo` applied.
    pub fn pitch(&self, string: usize, fret: u8, capo: Capo) -> Option<u8> {
        let p = u16::from(self.open_pitch(string, capo)?) + u16::from(fret);
        u8::try_from(p).ok().filter(|p| *p <= 127)
    }

    /// The lowest relative fret above the open string that `string` can be pressed at with
    /// `capo` (1 for an ordinary string; higher for a short string).
    pub fn min_fret(&self, string: usize, capo: Capo) -> u8 {
        let start = self.strings.get(string).map_or(0, |s| s.start_fret);
        if start == 0 {
            1
        } else {
            start.saturating_sub(capo.raise(string)).max(1)
        }
    }

    /// The highest relative fret of `string` with `capo` applied.
    pub fn max_fret(&self, string: usize, capo: Capo) -> u8 {
        self.frets.saturating_sub(capo.raise(string))
    }

    /// The relative fret that makes `string` sound `pitch` (0 = open), if there is one.
    pub fn fret_for_pitch(&self, string: usize, pitch: u8, capo: Capo) -> Option<u8> {
        let open = self.open_pitch(string, capo)?;
        let fret = pitch.checked_sub(open)?;
        if fret == 0 {
            return Some(0);
        }
        (fret >= self.min_fret(string, capo) && fret <= self.max_fret(string, capo)).then_some(fret)
    }

    /// Whether the strings are not sorted from high to low: a re-entrant tuning (a ukulele with
    /// a high fourth string, a banjo with its drone, a charango).
    pub fn is_re_entrant(&self) -> bool {
        self.strings.windows(2).any(|w| w[1].open > w[0].open)
    }

    /// The lowest and highest pitch the neck can sound with `capo` (`None` without strings).
    pub fn range(&self, capo: Capo) -> Option<(u8, u8)> {
        let mut lo = u8::MAX;
        let mut hi = 0u8;
        for i in 0..self.strings.len() {
            if let Some(p) = self.open_pitch(i, capo) {
                lo = lo.min(p);
                let top = u16::from(p) + u16::from(self.max_fret(i, capo));
                hi = hi.max(u8::try_from(top.min(127)).unwrap_or(127));
            }
        }
        (lo <= hi).then_some((lo, hi))
    }

    /// The open pitches in tab order.
    pub fn opens(&self) -> Vec<u8> {
        self.strings.iter().map(|s| s.open).collect()
    }

    /// A tuning for note names read from tab labels (pitch classes in tab order): the catalogue
    /// tuning with the same pitch classes when there is one, else an invented neck that stacks
    /// each string below the one above it, starting near the top string of a guitar.
    pub fn from_pitch_classes(pcs: &[u8]) -> Option<Tuning> {
        if pcs.is_empty() || pcs.len() > MAX_STRINGS {
            return None;
        }
        let want: Vec<u8> = pcs.iter().map(|p| p % 12).collect();
        for inst in catalogue() {
            for nt in &inst.tunings {
                let have: Vec<u8> = nt.tuning.strings.iter().map(|s| s.open % 12).collect();
                if have == want {
                    return Some(nt.tuning.clone());
                }
            }
        }
        let mut top = 64i32 - 5 * (6 - pcs.len() as i32).max(0);
        let mut opens = Vec::with_capacity(want.len());
        for (i, pc) in want.iter().enumerate() {
            let mut p = top - (top - i32::from(*pc)).rem_euclid(12);
            if i > 0 && p >= top {
                p -= 12;
            }
            p = p.clamp(12, 100);
            opens.push(p as u8);
            top = p;
        }
        Some(Tuning::from_opens(&opens, 24))
    }
}

/// What kind of instrument a catalogue entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// Guitars (six, seven, eight and twelve strings).
    Guitar,
    /// Bass guitars.
    Bass,
    /// The ukulele family.
    Ukulele,
    /// Banjos.
    Banjo,
    /// Mandolin family.
    Mandolin,
    /// Slide instruments (resonator guitar, lap steel).
    Slide,
    /// Other plucked folk instruments.
    Folk,
    /// Lutes.
    Lute,
}

impl Family {
    /// The name shown in the instrument list.
    pub fn label(self) -> &'static str {
        match self {
            Family::Guitar => "Guitar",
            Family::Bass => "Bass",
            Family::Ukulele => "Ukulele",
            Family::Banjo => "Banjo",
            Family::Mandolin => "Mandolin family",
            Family::Slide => "Slide",
            Family::Folk => "Folk",
            Family::Lute => "Lute",
        }
    }
}

/// A named tuning of an instrument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedTuning {
    /// A stable identifier, unique inside the instrument (`standard`, `drop_d`).
    pub id: &'static str,
    /// The name shown to the user.
    pub name: &'static str,
    /// The strings and neck.
    pub tuning: Tuning,
}

/// A catalogue entry: an instrument with its tuning presets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instrument {
    /// A stable identifier (`guitar6`, `banjo5`), stored in files.
    pub id: &'static str,
    /// The name shown to the user.
    pub name: &'static str,
    /// The family.
    pub family: Family,
    /// The tuning presets, the default first.
    pub tunings: Vec<NamedTuning>,
    /// The default fret count.
    pub default_frets: u8,
    /// Octaves the instrument is written above its sounding pitch in a notated staff (1 for the
    /// guitar and bass, 0 otherwise).
    pub clef_octave: i8,
}

impl Instrument {
    /// The default tuning (the first preset).
    pub fn default_tuning(&self) -> &NamedTuning {
        &self.tunings[0]
    }

    /// The preset called `id`.
    pub fn tuning(&self, id: &str) -> Option<&NamedTuning> {
        self.tunings.iter().find(|t| t.id == id)
    }
}

/// Every built-in instrument, in menu order.
pub fn catalogue() -> &'static [Instrument] {
    static CAT: OnceLock<Vec<Instrument>> = OnceLock::new();
    CAT.get_or_init(build_catalogue)
}

/// The catalogue instrument a track's name suggests (`guitar6` when it suggests none): the one the
/// editors fret a clip without tab data for.
pub fn instrument_for_name(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["bass"]) {
        "bass4"
    } else if has(&["banjo"]) {
        "banjo5"
    } else if has(&["ukulele", "uke"]) {
        "ukulele_soprano"
    } else if has(&["mandolin"]) {
        "mandolin"
    } else if has(&["bouzouki"]) {
        "bouzouki_irish"
    } else if has(&["12-string", "12 string", "twelve"]) {
        "guitar12"
    } else {
        "guitar6"
    }
}

/// The instrument called `id`.
pub fn find_instrument(id: &str) -> Option<&'static Instrument> {
    catalogue().iter().find(|i| i.id == id)
}

/// The preset `tuning_id` of the instrument `instrument_id`.
pub fn find_tuning(instrument_id: &str, tuning_id: &str) -> Option<&'static NamedTuning> {
    find_instrument(instrument_id)?.tuning(tuning_id)
}

/// The note names of the twelve pitch classes without the octave, sharps.
pub fn pitch_class_name(key: u8) -> &'static str {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    NAMES[usize::from(key % 12)]
}

/// The MIDI key of a note name with an octave (`E2`, `F#3`, `Bb1`; middle C = `C4` = 60).
pub fn parse_pitch(text: &str) -> Option<u8> {
    let t = text.trim();
    let mut chars = t.chars();
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
    let (shift, rest) = if let Some(r) = rest.strip_prefix(['#', '\u{266f}']) {
        (1, r)
    } else if let Some(r) = rest.strip_prefix(['b', '\u{266d}']) {
        (-1, r)
    } else {
        (0, rest)
    };
    let octave: i32 = rest.parse().ok()?;
    let key = (octave + 1) * 12 + base + shift;
    u8::try_from(key).ok().filter(|k| *k <= 127)
}

/// A tuning from note names in tab order (`"E4 B3 G3 D3 A2 E2"`).
fn notes(names: &str, frets: u8) -> Tuning {
    let strings = names
        .split_whitespace()
        .map(|n| StringDef::new(parse_pitch(n).unwrap_or(60)))
        .collect();
    Tuning { strings, frets }
}

/// Marks courses: `course_all` doubles every string, in unison.
fn doubled(mut t: Tuning, octave_from: usize) -> Tuning {
    let n = t.strings.len();
    for (i, s) in t.strings.iter_mut().enumerate() {
        s.course = 2;
        s.octave_course = i >= octave_from && octave_from < n;
    }
    t
}

fn nt(id: &'static str, name: &'static str, tuning: Tuning) -> NamedTuning {
    NamedTuning { id, name, tuning }
}

fn guitar(id: &'static str, name: &'static str, tunings: Vec<NamedTuning>, clef: i8) -> Instrument {
    Instrument {
        id,
        name,
        family: Family::Guitar,
        default_frets: tunings[0].tuning.frets,
        tunings,
        clef_octave: clef,
    }
}

fn simple(
    id: &'static str,
    name: &'static str,
    family: Family,
    tunings: Vec<NamedTuning>,
) -> Instrument {
    Instrument {
        id,
        name,
        family,
        default_frets: tunings[0].tuning.frets,
        tunings,
        clef_octave: 0,
    }
}

fn banjo5(notes5: &str) -> Tuning {
    // Four long strings in tab order, then the short drone string at the bottom line, starting
    // at the fifth fret.
    let mut t = notes(notes5, 22);
    if let Some(last) = t.strings.last_mut() {
        last.start_fret = 5;
        last.label = last.label.to_lowercase();
    }
    t
}

#[allow(clippy::vec_init_then_push)]
fn build_catalogue() -> Vec<Instrument> {
    let g = |s: &str| notes(s, 24);
    let mut out = Vec::new();

    out.push(guitar(
        "guitar6",
        "Guitar (6 strings)",
        vec![
            nt("standard", "Standard (EADGBE)", g("E4 B3 G3 D3 A2 E2")),
            nt("drop_d", "Drop D", g("E4 B3 G3 D3 A2 D2")),
            nt("dadgad", "DADGAD", g("D4 A3 G3 D3 A2 D2")),
            nt("open_g", "Open G", g("D4 B3 G3 D3 G2 D2")),
            nt("open_d", "Open D", g("D4 A3 F#3 D3 A2 D2")),
            nt("open_e", "Open E", g("E4 B3 G#3 E3 B2 E2")),
            nt("half_down", "Half step down", g("Eb4 Bb3 Gb3 Db3 Ab2 Eb2")),
            nt("full_down", "Full step down", g("D4 A3 F3 C3 G2 D2")),
            nt("drop_c", "Drop C", g("D4 A3 F3 C3 G2 C2")),
        ],
        1,
    ));
    out.push(guitar(
        "guitar7",
        "Guitar (7 strings)",
        vec![nt(
            "standard",
            "Standard (BEADGBE)",
            g("E4 B3 G3 D3 A2 E2 B1"),
        )],
        1,
    ));
    out.push(guitar(
        "guitar8",
        "Guitar (8 strings)",
        vec![nt(
            "standard",
            "Standard (F#BEADGBE)",
            g("E4 B3 G3 D3 A2 E2 B1 F#1"),
        )],
        1,
    ));
    out.push(guitar(
        "guitar12",
        "12-string guitar",
        vec![nt(
            "standard",
            "Standard (EADGBE, octave pairs)",
            doubled(g("E4 B3 G3 D3 A2 E2"), 2),
        )],
        1,
    ));

    let b = |s: &str| notes(s, 24);
    out.push(simple(
        "bass4",
        "Bass (4 strings)",
        Family::Bass,
        vec![
            nt("standard", "Standard (EADG)", b("G2 D2 A1 E1")),
            nt("drop_d", "Drop D", b("G2 D2 A1 D1")),
            nt("half_down", "Half step down", b("Gb2 Db2 Ab1 Eb1")),
        ],
    ));
    out.push(simple(
        "bass5",
        "Bass (5 strings)",
        Family::Bass,
        vec![
            nt("standard", "Standard (BEADG)", b("G2 D2 A1 E1 B0")),
            nt("high_c", "High C (EADGC)", b("C3 G2 D2 A1 E1")),
        ],
    ));
    out.push(simple(
        "bass6",
        "Bass (6 strings)",
        Family::Bass,
        vec![nt("standard", "Standard (BEADGC)", b("C3 G2 D2 A1 E1 B0"))],
    ));
    for inst in out.iter_mut().filter(|i| i.family == Family::Bass) {
        inst.clef_octave = 1;
    }

    // Ukuleles: the same four pitches, different necks. The re-entrant tuning puts the
    // highest string (the fourth, G4) at the bottom line.
    for (id, name, frets) in [
        ("ukulele_soprano", "Ukulele (soprano)", 15),
        ("ukulele_concert", "Ukulele (concert)", 18),
        ("ukulele_tenor", "Ukulele (tenor)", 19),
    ] {
        out.push(simple(
            id,
            name,
            Family::Ukulele,
            vec![
                nt(
                    "reentrant",
                    "Re-entrant (gCEA)",
                    notes("A4 E4 C4 G4", frets),
                ),
                nt("low_g", "Low G (GCEA)", notes("A4 E4 C4 G3", frets)),
            ],
        ));
    }
    out.push(simple(
        "ukulele_baritone",
        "Ukulele (baritone)",
        Family::Ukulele,
        vec![nt("standard", "Standard (DGBE)", notes("E4 B3 G3 D3", 19))],
    ));

    out.push(simple(
        "banjo5",
        "Banjo (5 strings)",
        Family::Banjo,
        vec![
            nt("open_g", "Open G (gDGBD)", banjo5("D4 B3 G3 D3 G4")),
            nt("double_c", "Double C (gCGCD)", banjo5("D4 C4 G3 C3 G4")),
            nt("open_d", "Open D (f#DF#AD)", banjo5("D4 A3 F#3 D3 F#4")),
            nt("standard_c", "Standard C (gCGBD)", banjo5("D4 B3 G3 C3 G4")),
            nt("modal_g", "Modal G (gDGCD)", banjo5("D4 C4 G3 D3 G4")),
        ],
    ));
    out.push(simple(
        "banjo4_tenor",
        "Banjo (4 strings, tenor)",
        Family::Banjo,
        vec![
            nt("standard", "Standard (CGDA)", notes("A4 D4 G3 C3", 19)),
            nt("irish", "Irish (GDAE)", notes("E4 A3 D3 G2", 19)),
        ],
    ));
    out.push(simple(
        "banjo4_plectrum",
        "Banjo (4 strings, plectrum)",
        Family::Banjo,
        vec![nt("standard", "Standard (CGBD)", notes("D4 B3 G3 C3", 22))],
    ));

    out.push(simple(
        "mandolin",
        "Mandolin",
        Family::Mandolin,
        vec![nt(
            "standard",
            "Standard (GDAE)",
            doubled(notes("E5 A4 D4 G3", 20), 99),
        )],
    ));
    out.push(simple(
        "mandola",
        "Mandola",
        Family::Mandolin,
        vec![nt(
            "standard",
            "Standard (CGDA)",
            doubled(notes("A4 D4 G3 C3", 20), 99),
        )],
    ));
    out.push(simple(
        "octave_mandolin",
        "Octave mandolin",
        Family::Mandolin,
        vec![nt(
            "standard",
            "Standard (GDAE, octave pairs below)",
            doubled(notes("E4 A3 D3 G2", 20), 2),
        )],
    ));
    out.push(simple(
        "mandocello",
        "Mandocello",
        Family::Mandolin,
        vec![nt(
            "standard",
            "Standard (CGDA)",
            doubled(notes("A3 D3 G2 C2", 20), 99),
        )],
    ));
    out.push(simple(
        "bouzouki_irish",
        "Bouzouki (Irish)",
        Family::Folk,
        vec![nt(
            "standard",
            "Standard (GDAD)",
            doubled(notes("D4 A3 D3 G2", 24), 2),
        )],
    ));
    out.push(simple(
        "balalaika_prima",
        "Balalaika (prima)",
        Family::Folk,
        vec![nt("standard", "Standard (EEA)", notes("A4 E4 E4", 15))],
    ));
    out.push(simple(
        "dobro",
        "Resonator guitar / lap steel",
        Family::Slide,
        vec![
            nt(
                "open_g",
                "Open G (high bass: GBDGBD)",
                notes("D4 B3 G3 D3 B2 G2", 18),
            ),
            nt(
                "open_d",
                "Open D (DAD F#AD)",
                notes("A4 F#4 D4 A3 F#3 D3", 18),
            ),
        ],
    ));
    out.push(simple(
        "cavaquinho",
        "Cavaquinho",
        Family::Folk,
        vec![nt("standard", "Standard (DGBD)", notes("D5 B4 G4 D4", 17))],
    ));
    out.push(simple(
        "charango",
        "Charango",
        Family::Folk,
        vec![nt("standard", "Standard (GCEAE, re-entrant)", {
            let mut t = doubled(notes("E5 A4 E5 C5 G4", 15), 99);
            t.strings[2].octave_course = true;
            t
        })],
    ));
    out.push(Instrument {
        id: "lute",
        name: "Lute (renaissance)",
        family: Family::Lute,
        default_frets: 9,
        tunings: vec![nt("g_tuning", "G tuning", {
            let mut t = doubled(notes("G4 D4 A3 F3 C3 G2", 9), 3);
            t.strings[0].course = 1;
            t
        })],
        clef_octave: 0,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_names_parse() {
        assert_eq!(parse_pitch("C4"), Some(60));
        assert_eq!(parse_pitch("E2"), Some(40));
        assert_eq!(parse_pitch("F#3"), Some(54));
        assert_eq!(parse_pitch("Bb1"), Some(34));
        assert_eq!(parse_pitch("B0"), Some(23));
        assert_eq!(parse_pitch("H2"), None);
        assert_eq!(parse_pitch("E"), None);
        assert_eq!(parse_pitch("C99"), None);
    }

    #[test]
    fn standard_guitar_in_tab_order() {
        let t = &find_tuning("guitar6", "standard").unwrap().tuning;
        assert_eq!(t.opens(), vec![64, 59, 55, 50, 45, 40]);
        assert!(!t.is_re_entrant());
        assert_eq!(t.pitch(0, 3, Capo::NONE), Some(67));
        assert_eq!(t.pitch(0, 3, Capo::at(2)), Some(69));
    }

    #[test]
    fn banjo_drone_is_a_short_string() {
        let t = &find_tuning("banjo5", "open_g").unwrap().tuning;
        assert_eq!(t.opens(), vec![62, 59, 55, 50, 67]);
        assert!(t.is_re_entrant());
        assert_eq!(t.strings[4].start_fret, 5);
        assert_eq!(t.min_fret(4, Capo::NONE), 5);
        assert_eq!(t.min_fret(0, Capo::NONE), 1);
        // Frets 1 to 4 of the drone do not exist; open and 5 and above do.
        assert_eq!(t.fret_for_pitch(4, 67, Capo::NONE), Some(0));
        assert_eq!(t.fret_for_pitch(4, 69, Capo::NONE), None);
        assert_eq!(t.fret_for_pitch(4, 72, Capo::NONE), Some(5));
        // A capo at the seventh fret leaves relative fret 1 as the first playable one.
        assert_eq!(t.min_fret(4, Capo::at(7)), 1);
        assert_eq!(t.min_fret(4, Capo::at(2)), 3);
    }

    #[test]
    fn partial_capo_only_raises_covered_strings() {
        let t = &find_tuning("guitar6", "standard").unwrap().tuning;
        let c = Capo::partial(2, 0b000111);
        assert_eq!(t.open_pitch(0, c), Some(66));
        assert_eq!(t.open_pitch(2, c), Some(57));
        assert_eq!(t.open_pitch(3, c), Some(50));
    }

    #[test]
    fn courses_are_marked() {
        let m = &find_tuning("mandolin", "standard").unwrap().tuning;
        assert!(m.strings.iter().all(|s| s.course == 2 && !s.octave_course));
        let g12 = &find_tuning("guitar12", "standard").unwrap().tuning;
        assert_eq!(g12.len(), 6);
        assert!(g12.strings[2].octave_course);
        assert!(!g12.strings[0].octave_course);
    }

    #[test]
    fn catalogue_is_complete_and_valid() {
        for id in [
            "guitar6",
            "guitar7",
            "guitar8",
            "guitar12",
            "bass4",
            "bass5",
            "bass6",
            "ukulele_soprano",
            "ukulele_concert",
            "ukulele_tenor",
            "ukulele_baritone",
            "banjo5",
            "banjo4_tenor",
            "banjo4_plectrum",
            "mandolin",
            "mandola",
            "octave_mandolin",
            "mandocello",
            "bouzouki_irish",
            "balalaika_prima",
            "dobro",
            "cavaquinho",
            "charango",
            "lute",
        ] {
            assert!(find_instrument(id).is_some(), "missing {id}");
        }
        let mut ids: Vec<_> = catalogue().iter().map(|i| i.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), catalogue().len(), "instrument ids are unique");
        for inst in catalogue() {
            assert!(!inst.tunings.is_empty());
            for t in &inst.tunings {
                assert_eq!(t.tuning.validate(), Ok(()), "{}/{}", inst.id, t.id);
            }
            let mut tids: Vec<_> = inst.tunings.iter().map(|t| t.id).collect();
            tids.sort_unstable();
            tids.dedup();
            assert_eq!(tids.len(), inst.tunings.len(), "{} tuning ids", inst.id);
        }
        let guitar_names: Vec<_> = find_instrument("guitar6")
            .unwrap()
            .tunings
            .iter()
            .map(|t| t.id)
            .collect();
        for want in [
            "standard",
            "drop_d",
            "dadgad",
            "open_g",
            "open_d",
            "open_e",
            "half_down",
            "full_down",
            "drop_c",
        ] {
            assert!(guitar_names.contains(&want), "{want}");
        }
    }

    #[test]
    fn ukulele_is_re_entrant_and_low_g_is_not() {
        let r = &find_tuning("ukulele_soprano", "reentrant").unwrap().tuning;
        let l = &find_tuning("ukulele_soprano", "low_g").unwrap().tuning;
        assert!(r.is_re_entrant());
        assert!(!l.is_re_entrant());
        assert_eq!(r.opens(), vec![69, 64, 60, 67]);
    }

    #[test]
    fn custom_tuning_validation() {
        assert_eq!(
            Tuning::from_opens(&[], 12).validate(),
            Err(TuningError::StringCount(0))
        );
        assert_eq!(
            Tuning::from_opens(&[40; 13], 12).validate(),
            Err(TuningError::StringCount(13))
        );
        assert_eq!(
            Tuning::from_opens(&[40], 0).validate(),
            Err(TuningError::FretCount(0))
        );
        assert_eq!(
            Tuning::from_opens(&[120], 24).validate(),
            Err(TuningError::PitchRange(0))
        );
        assert_eq!(Tuning::from_opens(&[40; 12], 24).validate(), Ok(()));
        assert_eq!(Tuning::from_opens(&[40], 24).validate(), Ok(()));
        let mut t = Tuning::from_opens(&[40, 45], 12);
        t.strings[1].start_fret = 13;
        assert_eq!(t.validate(), Err(TuningError::StartFret(1)));
        t.strings[1].start_fret = 0;
        t.strings[1].course = 3;
        assert_eq!(t.validate(), Err(TuningError::Course(1, 3)));
    }

    #[test]
    fn tuning_from_labels() {
        let std = Tuning::from_pitch_classes(&[4, 11, 7, 2, 9, 4]).unwrap();
        assert_eq!(std.opens(), vec![64, 59, 55, 50, 45, 40]);
        let odd = Tuning::from_pitch_classes(&[2, 9, 5]).unwrap();
        assert_eq!(odd.len(), 3);
        assert!(odd.opens().windows(2).all(|w| w[0] > w[1]));
        assert!(Tuning::from_pitch_classes(&[]).is_none());
    }

    #[test]
    fn range_follows_the_capo() {
        let t = &find_tuning("guitar6", "standard").unwrap().tuning;
        assert_eq!(t.range(Capo::NONE), Some((40, 88)));
        assert_eq!(t.range(Capo::at(2)), Some((42, 88)));
    }
}
