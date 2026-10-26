//! Music theory shared by the note editors and the note players: scales, scale degrees and
//! chords.
//!
//! Everything here is pure arithmetic on MIDI keys (`0..=127`, middle C = 60) and pitch classes
//! (`0..12`, C = 0); it knows nothing of clips, the UI or the engine. A [`Scale`] is a root and a
//! [`ScaleKind`]; a [`ChordKind`] is a stack of semitone offsets.
//!
//! Conventions:
//! * **Nearest in-scale key** ([`Scale::nearest`]): the closest key in the scale, ties go
//!   **down** (the key below wins), and the result stays inside `0..=127`.
//! * **Degree stepping** ([`Scale::step`]): moving `n` degrees moves to the `n`-th in-scale key
//!   above (or below). From a key that is not in the scale the first step lands on the nearest
//!   in-scale key in the direction of travel. A step that leaves `0..=127` is `None`.
//! * **Inversions** ([`chord_into`], [`diatonic_chord_into`]): the lowest note moves up an octave once
//!   per inversion; chords of fewer than three notes have no inversions, and the inversion is
//!   limited to `notes - 1`. Notes above 127 are dropped.

/// Highest MIDI key.
pub const MAX_KEY: u8 = 127;

/// The names of the twelve pitch classes (sharps), C first.
pub const PITCH_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// The name of a pitch class (`pc` is taken modulo 12): `C`, `C#`, ...
pub fn pitch_name(pc: u8) -> &'static str {
    PITCH_NAMES[usize::from(pc % 12)]
}

/// A kind of scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScaleKind {
    /// All twelve notes: no scale at all.
    Chromatic,
    /// Major (Ionian).
    Major,
    /// Natural minor (Aeolian).
    NaturalMinor,
    /// Harmonic minor.
    HarmonicMinor,
    /// Melodic minor (ascending form).
    MelodicMinor,
    /// Dorian.
    Dorian,
    /// Phrygian.
    Phrygian,
    /// Lydian.
    Lydian,
    /// Mixolydian.
    Mixolydian,
    /// Locrian.
    Locrian,
    /// Major pentatonic.
    MajorPentatonic,
    /// Minor pentatonic.
    MinorPentatonic,
    /// Blues (minor pentatonic with the flat fifth).
    Blues,
    /// Whole tone.
    WholeTone,
    /// Phrygian dominant (the fifth mode of harmonic minor).
    PhrygianDominant,
    /// Hungarian minor.
    HungarianMinor,
}

impl ScaleKind {
    /// Every kind, [`ScaleKind::Chromatic`] first, in menu order.
    pub const ALL: [ScaleKind; 16] = [
        ScaleKind::Chromatic,
        ScaleKind::Major,
        ScaleKind::NaturalMinor,
        ScaleKind::HarmonicMinor,
        ScaleKind::MelodicMinor,
        ScaleKind::Dorian,
        ScaleKind::Phrygian,
        ScaleKind::Lydian,
        ScaleKind::Mixolydian,
        ScaleKind::Locrian,
        ScaleKind::MajorPentatonic,
        ScaleKind::MinorPentatonic,
        ScaleKind::Blues,
        ScaleKind::WholeTone,
        ScaleKind::PhrygianDominant,
        ScaleKind::HungarianMinor,
    ];

    /// The semitone offsets from the root, ascending, starting with 0.
    pub fn intervals(self) -> &'static [u8] {
        match self {
            ScaleKind::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            ScaleKind::Major => &[0, 2, 4, 5, 7, 9, 11],
            ScaleKind::NaturalMinor => &[0, 2, 3, 5, 7, 8, 10],
            ScaleKind::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            ScaleKind::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            ScaleKind::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            ScaleKind::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            ScaleKind::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            ScaleKind::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            ScaleKind::Locrian => &[0, 1, 3, 5, 6, 8, 10],
            ScaleKind::MajorPentatonic => &[0, 2, 4, 7, 9],
            ScaleKind::MinorPentatonic => &[0, 3, 5, 7, 10],
            ScaleKind::Blues => &[0, 3, 5, 6, 7, 10],
            ScaleKind::WholeTone => &[0, 2, 4, 6, 8, 10],
            ScaleKind::PhrygianDominant => &[0, 1, 4, 5, 7, 8, 10],
            ScaleKind::HungarianMinor => &[0, 2, 3, 6, 7, 8, 11],
        }
    }

    /// The stable name used in files and command arguments (`natural_minor`).
    pub fn id(self) -> &'static str {
        match self {
            ScaleKind::Chromatic => "chromatic",
            ScaleKind::Major => "major",
            ScaleKind::NaturalMinor => "natural_minor",
            ScaleKind::HarmonicMinor => "harmonic_minor",
            ScaleKind::MelodicMinor => "melodic_minor",
            ScaleKind::Dorian => "dorian",
            ScaleKind::Phrygian => "phrygian",
            ScaleKind::Lydian => "lydian",
            ScaleKind::Mixolydian => "mixolydian",
            ScaleKind::Locrian => "locrian",
            ScaleKind::MajorPentatonic => "major_pentatonic",
            ScaleKind::MinorPentatonic => "minor_pentatonic",
            ScaleKind::Blues => "blues",
            ScaleKind::WholeTone => "whole_tone",
            ScaleKind::PhrygianDominant => "phrygian_dominant",
            ScaleKind::HungarianMinor => "hungarian_minor",
        }
    }

    /// The name shown in menus.
    pub fn label(self) -> &'static str {
        match self {
            ScaleKind::Chromatic => "Chromatic",
            ScaleKind::Major => "Major",
            ScaleKind::NaturalMinor => "Natural Minor",
            ScaleKind::HarmonicMinor => "Harmonic Minor",
            ScaleKind::MelodicMinor => "Melodic Minor",
            ScaleKind::Dorian => "Dorian",
            ScaleKind::Phrygian => "Phrygian",
            ScaleKind::Lydian => "Lydian",
            ScaleKind::Mixolydian => "Mixolydian",
            ScaleKind::Locrian => "Locrian",
            ScaleKind::MajorPentatonic => "Major Pentatonic",
            ScaleKind::MinorPentatonic => "Minor Pentatonic",
            ScaleKind::Blues => "Blues",
            ScaleKind::WholeTone => "Whole Tone",
            ScaleKind::PhrygianDominant => "Phrygian Dominant",
            ScaleKind::HungarianMinor => "Hungarian Minor",
        }
    }

    /// The kind named `id` (see [`Self::id`]).
    pub fn from_id(id: &str) -> Option<ScaleKind> {
        ScaleKind::ALL.into_iter().find(|k| k.id() == id)
    }

    /// The pitch classes of the scale on C as a 12-bit mask (bit `i` = semitone `i`).
    fn mask(self) -> u16 {
        self.intervals().iter().fold(0, |m, i| m | (1 << i))
    }
}

/// A scale: a root pitch class and a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Scale {
    /// The root pitch class, `0..12` (C = 0).
    pub root: u8,
    /// The kind.
    pub kind: ScaleKind,
}

impl Scale {
    /// A scale on `root` (taken modulo 12).
    pub fn new(root: u8, kind: ScaleKind) -> Self {
        Self {
            root: root % 12,
            kind,
        }
    }

    /// Whether the scale has all twelve notes, so it never restricts anything.
    pub fn is_chromatic(&self) -> bool {
        self.kind == ScaleKind::Chromatic
    }

    /// The pitch classes of the scale as a 12-bit mask (bit `pc`).
    pub fn pitch_mask(&self) -> u16 {
        let m = u32::from(self.kind.mask());
        let r = u32::from(self.root);
        (((m << r) | (m >> (12 - r))) & 0x0fff) as u16
    }

    /// Whether `key` is a note of the scale.
    pub fn contains(&self, key: u8) -> bool {
        self.pitch_mask() & (1 << (key % 12)) != 0
    }

    /// Whether `key` is the root (any octave).
    pub fn is_root(&self, key: u8) -> bool {
        key % 12 == self.root
    }

    /// The degree of `key` (0 = root) when it is in the scale.
    pub fn degree(&self, key: u8) -> Option<usize> {
        let rel = (key % 12 + 12 - self.root) % 12;
        self.kind.intervals().iter().position(|i| *i == rel)
    }

    /// The in-scale key nearest to `key` (`key` itself when it is in the scale), ties down,
    /// within `0..=127`.
    pub fn nearest(&self, key: u8) -> u8 {
        for d in 0..12u8 {
            if let Some(k) = key.checked_sub(d)
                && self.contains(k)
            {
                return k;
            }
            let up = key.saturating_add(d);
            if up <= MAX_KEY && self.contains(up) {
                return up;
            }
        }
        key
    }

    /// The next in-scale key above `key` (any key, in or out of the scale), if it exists.
    fn next_above(&self, key: i32) -> Option<i32> {
        (key + 1..key + 13).find(|k| self.contains_i32(*k))
    }

    /// The next in-scale key below `key`.
    fn next_below(&self, key: i32) -> Option<i32> {
        (key - 12..key).rev().find(|k| self.contains_i32(*k))
    }

    fn contains_i32(&self, key: i32) -> bool {
        self.pitch_mask() & (1 << key.rem_euclid(12)) != 0
    }

    /// Moves `degrees` scale steps up (positive) or down (negative) from `key`; `None` when the
    /// result leaves `0..=127`. See the module documentation for keys outside the scale.
    pub fn step(&self, key: u8, degrees: i32) -> Option<u8> {
        let mut k = i32::from(key);
        for _ in 0..degrees.unsigned_abs() {
            k = if degrees > 0 {
                self.next_above(k)?
            } else {
                self.next_below(k)?
            };
            if !(0..=i32::from(MAX_KEY)).contains(&k) {
                return None;
            }
        }
        Some(k as u8)
    }

    /// The number of degrees between `from` and the in-scale key `to`: `step(from, n) == to`.
    /// Positive upwards: the in-scale keys in `(from, to]`; negative downwards: minus the
    /// in-scale keys in `[to, from)`.
    pub fn degrees_between(&self, from: u8, to: u8) -> i32 {
        if to >= from {
            ((from + 1..=to).filter(|k| self.contains(*k)).count()) as i32
        } else {
            -((to..from).filter(|k| self.contains(*k)).count() as i32)
        }
    }

    /// The in-scale keys, ascending, in `lo..=hi` (clamped to the MIDI range). Allocates: for
    /// the editors, not for the audio thread.
    pub fn keys_in(&self, lo: u8, hi: u8) -> Vec<u8> {
        (lo..=hi.min(MAX_KEY))
            .filter(|k| self.contains(*k))
            .collect()
    }
}

/// A kind of chord: a stack of semitone offsets above the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChordKind {
    /// Major triad.
    Major,
    /// Minor triad.
    Minor,
    /// Diminished triad.
    Dim,
    /// Augmented triad.
    Aug,
    /// Suspended second.
    Sus2,
    /// Suspended fourth.
    Sus4,
    /// Major sixth (`6`).
    Sixth,
    /// Minor sixth (`m6`).
    MinorSixth,
    /// Dominant seventh (`7`).
    Seventh,
    /// Major seventh.
    MajorSeventh,
    /// Minor seventh.
    MinorSeventh,
    /// Half-diminished seventh (`m7b5`).
    HalfDim,
    /// Diminished seventh.
    DimSeventh,
    /// Dominant ninth (`9`).
    Ninth,
    /// Major ninth.
    MajorNinth,
    /// Minor ninth.
    MinorNinth,
    /// Major triad with an added ninth.
    Add9,
    /// Dominant eleventh (`11`).
    Eleventh,
    /// Dominant thirteenth (`13`).
    Thirteenth,
    /// Power chord (root and fifth).
    Power,
    /// Root and its octave.
    Octave,
}

impl ChordKind {
    /// Every chord, in menu order.
    pub const ALL: [ChordKind; 21] = [
        ChordKind::Major,
        ChordKind::Minor,
        ChordKind::Dim,
        ChordKind::Aug,
        ChordKind::Sus2,
        ChordKind::Sus4,
        ChordKind::Sixth,
        ChordKind::MinorSixth,
        ChordKind::Seventh,
        ChordKind::MajorSeventh,
        ChordKind::MinorSeventh,
        ChordKind::HalfDim,
        ChordKind::DimSeventh,
        ChordKind::Ninth,
        ChordKind::MajorNinth,
        ChordKind::MinorNinth,
        ChordKind::Add9,
        ChordKind::Eleventh,
        ChordKind::Thirteenth,
        ChordKind::Power,
        ChordKind::Octave,
    ];

    /// The semitone offsets from the root, ascending, starting with 0.
    pub fn intervals(self) -> &'static [u8] {
        match self {
            ChordKind::Major => &[0, 4, 7],
            ChordKind::Minor => &[0, 3, 7],
            ChordKind::Dim => &[0, 3, 6],
            ChordKind::Aug => &[0, 4, 8],
            ChordKind::Sus2 => &[0, 2, 7],
            ChordKind::Sus4 => &[0, 5, 7],
            ChordKind::Sixth => &[0, 4, 7, 9],
            ChordKind::MinorSixth => &[0, 3, 7, 9],
            ChordKind::Seventh => &[0, 4, 7, 10],
            ChordKind::MajorSeventh => &[0, 4, 7, 11],
            ChordKind::MinorSeventh => &[0, 3, 7, 10],
            ChordKind::HalfDim => &[0, 3, 6, 10],
            ChordKind::DimSeventh => &[0, 3, 6, 9],
            ChordKind::Ninth => &[0, 4, 7, 10, 14],
            ChordKind::MajorNinth => &[0, 4, 7, 11, 14],
            ChordKind::MinorNinth => &[0, 3, 7, 10, 14],
            ChordKind::Add9 => &[0, 4, 7, 14],
            ChordKind::Eleventh => &[0, 4, 7, 10, 14, 17],
            ChordKind::Thirteenth => &[0, 4, 7, 10, 14, 17, 21],
            ChordKind::Power => &[0, 7],
            ChordKind::Octave => &[0, 12],
        }
    }

    /// The stable name used in command arguments (`maj7`).
    pub fn id(self) -> &'static str {
        match self {
            ChordKind::Major => "major",
            ChordKind::Minor => "minor",
            ChordKind::Dim => "dim",
            ChordKind::Aug => "aug",
            ChordKind::Sus2 => "sus2",
            ChordKind::Sus4 => "sus4",
            ChordKind::Sixth => "6",
            ChordKind::MinorSixth => "m6",
            ChordKind::Seventh => "7",
            ChordKind::MajorSeventh => "maj7",
            ChordKind::MinorSeventh => "m7",
            ChordKind::HalfDim => "m7b5",
            ChordKind::DimSeventh => "dim7",
            ChordKind::Ninth => "9",
            ChordKind::MajorNinth => "maj9",
            ChordKind::MinorNinth => "m9",
            ChordKind::Add9 => "add9",
            ChordKind::Eleventh => "11",
            ChordKind::Thirteenth => "13",
            ChordKind::Power => "power",
            ChordKind::Octave => "octave",
        }
    }

    /// The name shown in menus.
    pub fn label(self) -> &'static str {
        match self {
            ChordKind::Major => "Major",
            ChordKind::Minor => "Minor",
            ChordKind::Dim => "Diminished",
            ChordKind::Aug => "Augmented",
            ChordKind::Sus2 => "Sus2",
            ChordKind::Sus4 => "Sus4",
            ChordKind::Sixth => "6",
            ChordKind::MinorSixth => "m6",
            ChordKind::Seventh => "7",
            ChordKind::MajorSeventh => "maj7",
            ChordKind::MinorSeventh => "m7",
            ChordKind::HalfDim => "m7b5",
            ChordKind::DimSeventh => "dim7",
            ChordKind::Ninth => "9",
            ChordKind::MajorNinth => "maj9",
            ChordKind::MinorNinth => "m9",
            ChordKind::Add9 => "add9",
            ChordKind::Eleventh => "11",
            ChordKind::Thirteenth => "13",
            ChordKind::Power => "Power (5)",
            ChordKind::Octave => "Octave",
        }
    }

    /// A short label for a toolbar field (`Dim`, `maj7`).
    pub fn short(self) -> &'static str {
        match self {
            ChordKind::Major => "Maj",
            ChordKind::Minor => "Min",
            ChordKind::Dim => "Dim",
            ChordKind::Aug => "Aug",
            ChordKind::Power => "5",
            ChordKind::Octave => "Oct",
            other => other.id(),
        }
    }

    /// The chord named `id` (see [`Self::id`]).
    pub fn from_id(id: &str) -> Option<ChordKind> {
        ChordKind::ALL.into_iter().find(|k| k.id() == id)
    }
}

/// How many chord tones a diatonic chord stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiatonicSize {
    /// Three notes: scale degrees 1, 3, 5.
    Triad,
    /// Four notes: 1, 3, 5, 7.
    Seventh,
    /// Five notes: 1, 3, 5, 7, 9.
    Ninth,
}

impl DiatonicSize {
    /// Every size.
    pub const ALL: [DiatonicSize; 3] = [
        DiatonicSize::Triad,
        DiatonicSize::Seventh,
        DiatonicSize::Ninth,
    ];

    /// The number of notes.
    pub fn notes(self) -> usize {
        match self {
            DiatonicSize::Triad => 3,
            DiatonicSize::Seventh => 4,
            DiatonicSize::Ninth => 5,
        }
    }

    /// The stable name used in command arguments.
    pub fn id(self) -> &'static str {
        match self {
            DiatonicSize::Triad => "triad",
            DiatonicSize::Seventh => "seventh",
            DiatonicSize::Ninth => "ninth",
        }
    }

    /// The name shown in menus.
    pub fn label(self) -> &'static str {
        match self {
            DiatonicSize::Triad => "Triad",
            DiatonicSize::Seventh => "7th",
            DiatonicSize::Ninth => "9th",
        }
    }

    /// The size named `id`.
    pub fn from_id(id: &str) -> Option<DiatonicSize> {
        DiatonicSize::ALL.into_iter().find(|s| s.id() == id)
    }
}

/// The most keys a chord helper writes.
pub const MAX_CHORD_KEYS: usize = 8;

/// Turns offsets from `root` into keys, inverted `inversion` times, dropping keys outside the
/// MIDI range; ascending and deduplicated. Writes into `out`, returns the count. Never allocates.
fn stack_into(
    root: u8,
    offsets: impl Iterator<Item = i32>,
    inversion: u8,
    out: &mut [u8; MAX_CHORD_KEYS],
) -> usize {
    let mut keys = [0i32; MAX_CHORD_KEYS];
    let mut n = 0;
    for o in offsets {
        let k = i32::from(root) + o;
        if (0..=i32::from(MAX_KEY)).contains(&k) && n < MAX_CHORD_KEYS {
            keys[n] = k;
            n += 1;
        }
    }
    keys[..n].sort_unstable();
    if n >= 3 {
        for _ in 0..usize::from(inversion).min(n - 1) {
            let low = keys[0];
            keys.copy_within(1..n, 0);
            keys[n - 1] = low + 12;
        }
    }
    keys[..n].sort_unstable();
    let mut count = 0;
    for k in &keys[..n] {
        if *k <= i32::from(MAX_KEY) && (count == 0 || out[count - 1] != *k as u8) {
            out[count] = *k as u8;
            count += 1;
        }
    }
    count
}

/// The keys of `kind` on `root` (clamped to 127), ascending, in the given inversion (0 = root
/// position), written to `out`; returns how many. The lowest note moves up an octave once per
/// inversion, limited to `notes - 1`; notes above 127 are dropped. Allocation-free.
pub fn chord_into(
    root: u8,
    kind: ChordKind,
    inversion: u8,
    out: &mut [u8; MAX_CHORD_KEYS],
) -> usize {
    stack_into(
        root.min(MAX_KEY),
        kind.intervals().iter().map(|i| i32::from(*i)),
        inversion,
        out,
    )
}

/// The diatonic chord on `key` in `scale` (`size.notes()` thirds stacked through the scale),
/// written to `out`; returns how many. The key is first moved to the nearest in-scale key (ties
/// down). Same rules as the editors' chord helper, allocation-free.
pub fn diatonic_chord_into(
    scale: Scale,
    key: u8,
    size: DiatonicSize,
    inversion: u8,
    out: &mut [u8; MAX_CHORD_KEYS],
) -> usize {
    let root = scale.nearest(key.min(MAX_KEY));
    let mut keys = [0u8; MAX_CHORD_KEYS];
    let mut n = 0;
    for i in 0..size.notes().min(MAX_CHORD_KEYS) {
        match scale.step(root, 2 * i as i32) {
            Some(k) => {
                keys[n] = k;
                n += 1;
            }
            None => break,
        }
    }
    let base = if n > 0 { keys[0] } else { root };
    stack_into(
        base,
        keys[..n].iter().map(|k| i32::from(*k) - i32::from(keys[0])),
        inversion,
        out,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scale_has_the_documented_intervals() {
        let expect: [(ScaleKind, &[u8]); 16] = [
            (
                ScaleKind::Chromatic,
                &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            ),
            (ScaleKind::Major, &[0, 2, 4, 5, 7, 9, 11]),
            (ScaleKind::NaturalMinor, &[0, 2, 3, 5, 7, 8, 10]),
            (ScaleKind::HarmonicMinor, &[0, 2, 3, 5, 7, 8, 11]),
            (ScaleKind::MelodicMinor, &[0, 2, 3, 5, 7, 9, 11]),
            (ScaleKind::Dorian, &[0, 2, 3, 5, 7, 9, 10]),
            (ScaleKind::Phrygian, &[0, 1, 3, 5, 7, 8, 10]),
            (ScaleKind::Lydian, &[0, 2, 4, 6, 7, 9, 11]),
            (ScaleKind::Mixolydian, &[0, 2, 4, 5, 7, 9, 10]),
            (ScaleKind::Locrian, &[0, 1, 3, 5, 6, 8, 10]),
            (ScaleKind::MajorPentatonic, &[0, 2, 4, 7, 9]),
            (ScaleKind::MinorPentatonic, &[0, 3, 5, 7, 10]),
            (ScaleKind::Blues, &[0, 3, 5, 6, 7, 10]),
            (ScaleKind::WholeTone, &[0, 2, 4, 6, 8, 10]),
            (ScaleKind::PhrygianDominant, &[0, 1, 4, 5, 7, 8, 10]),
            (ScaleKind::HungarianMinor, &[0, 2, 3, 6, 7, 8, 11]),
        ];
        assert_eq!(expect.len(), ScaleKind::ALL.len());
        for (kind, intervals) in expect {
            assert_eq!(kind.intervals(), intervals, "{kind:?}");
            assert!(intervals.windows(2).all(|w| w[0] < w[1]), "{kind:?}");
        }
    }

    #[test]
    fn modes_are_rotations_of_the_major_scale() {
        let major = ScaleKind::Major.intervals();
        let rotate = |from: usize| -> Vec<u8> {
            let base = major[from];
            (0..7)
                .map(|i| (major[(from + i) % 7] + 12 - base) % 12)
                .collect::<Vec<_>>()
        };
        let mode = |kind: ScaleKind, from: usize| {
            let mut r = rotate(from);
            r.sort_unstable();
            assert_eq!(kind.intervals(), r.as_slice(), "{kind:?}");
        };
        mode(ScaleKind::Dorian, 1);
        mode(ScaleKind::Phrygian, 2);
        mode(ScaleKind::Lydian, 3);
        mode(ScaleKind::Mixolydian, 4);
        mode(ScaleKind::NaturalMinor, 5);
        mode(ScaleKind::Locrian, 6);
    }

    #[test]
    fn ids_and_labels_are_unique_and_round_trip() {
        for (i, a) in ScaleKind::ALL.iter().enumerate() {
            assert_eq!(ScaleKind::from_id(a.id()), Some(*a));
            for b in &ScaleKind::ALL[i + 1..] {
                assert_ne!(a.id(), b.id());
                assert_ne!(a.label(), b.label());
            }
        }
        assert_eq!(ScaleKind::from_id("nope"), None);
        for (i, a) in ChordKind::ALL.iter().enumerate() {
            assert_eq!(ChordKind::from_id(a.id()), Some(*a));
            for b in &ChordKind::ALL[i + 1..] {
                assert_ne!(a.id(), b.id());
                assert_ne!(a.label(), b.label());
            }
            assert!(!a.short().is_empty());
        }
        for s in DiatonicSize::ALL {
            assert_eq!(DiatonicSize::from_id(s.id()), Some(s));
        }
    }

    #[test]
    fn a_scale_on_a_root_contains_the_right_pitch_classes_in_every_octave() {
        let d_minor = Scale::new(2, ScaleKind::NaturalMinor);
        let pcs: Vec<u8> = (0..12).filter(|k| d_minor.contains(*k)).collect();
        assert_eq!(pcs, [0, 2, 4, 5, 7, 9, 10]);
        for k in 0..=127u8 {
            assert_eq!(d_minor.contains(k), pcs.contains(&(k % 12)), "key {k}");
        }
        assert!(d_minor.is_root(62) && d_minor.is_root(2) && !d_minor.is_root(64));
        assert_eq!(d_minor.degree(62), Some(0));
        assert_eq!(d_minor.degree(70), Some(5), "B flat is the sixth degree");
        assert_eq!(d_minor.degree(60), Some(6));
        assert_eq!(d_minor.degree(61), None);
        // Wrapping of the mask: B major has F#, C#, D#, E, G#, A#.
        let b_major = Scale::new(11, ScaleKind::Major);
        assert_eq!(b_major.pitch_mask(), 0b1101_0101_1010);
        assert!(b_major.contains(66) && b_major.contains(59) && !b_major.contains(60));
        assert_eq!(Scale::new(14, ScaleKind::Major).root, 2);
    }

    #[test]
    fn chromatic_contains_everything_and_nearest_is_the_key() {
        let c = Scale::new(5, ScaleKind::Chromatic);
        assert!(c.is_chromatic());
        for k in 0..=127u8 {
            assert!(c.contains(k));
            assert_eq!(c.nearest(k), k);
        }
    }

    #[test]
    fn nearest_prefers_the_key_itself_then_the_closest_and_ties_go_down() {
        let c_major = Scale::new(0, ScaleKind::Major);
        assert_eq!(c_major.nearest(60), 60);
        // C#: equally far from C and D, so the lower one wins.
        assert_eq!(c_major.nearest(61), 60);
        // D#: D and E are both 1 away: down wins.
        assert_eq!(c_major.nearest(63), 62);
        // F#: F below, G above, both 1 away: down.
        assert_eq!(c_major.nearest(66), 65);
        // A#: A below, B above: down.
        assert_eq!(c_major.nearest(70), 69);
        // A gap of three in the minor pentatonic (E to G): F is nearer E? F is 1 above E, 2 below G.
        let a_pent = Scale::new(9, ScaleKind::MinorPentatonic); // A C D E G
        assert_eq!(a_pent.nearest(65), 64);
        assert_eq!(a_pent.nearest(66), 67);
        // C major pentatonic: C D E G A; F is 1 above E and 2 below G.
        let c_pent = Scale::new(0, ScaleKind::MajorPentatonic);
        assert_eq!(c_pent.nearest(65), 64);
        assert_eq!(c_pent.nearest(71), 72, "B is 2 above A and 1 below C");
    }

    #[test]
    fn nearest_stays_inside_the_midi_range_at_both_ends() {
        let f_sharp = Scale::new(6, ScaleKind::Major);
        for k in 0..=127u8 {
            let n = f_sharp.nearest(k);
            assert!(n <= 127 && f_sharp.contains(n), "key {k} -> {n}");
            let d = i32::from(n) - i32::from(k);
            assert!(d.abs() <= 1, "major gaps are at most 2: {k} -> {n}");
        }
        // Key 0 is C; in F# major the nearest is C# (up one) and key 127 (G) goes down to F#.
        assert_eq!(f_sharp.nearest(0), 1);
        assert_eq!(f_sharp.nearest(127), 126);
        let whole = Scale::new(1, ScaleKind::WholeTone);
        assert_eq!(whole.nearest(0), 1);
        assert_eq!(
            whole.nearest(127),
            127,
            "G is in the whole tone scale on C#"
        );
    }

    #[test]
    fn stepping_degrees_walks_the_scale_across_octaves() {
        let c_major = Scale::new(0, ScaleKind::Major);
        let up: Vec<u8> = (0..=8).map(|n| c_major.step(60, n).unwrap()).collect();
        assert_eq!(up, [60, 62, 64, 65, 67, 69, 71, 72, 74]);
        let down: Vec<u8> = (0..=8).map(|n| c_major.step(60, -n).unwrap()).collect();
        assert_eq!(down, [60, 59, 57, 55, 53, 52, 50, 48, 47]);
        assert_eq!(c_major.step(60, 7), Some(72));
        assert_eq!(c_major.step(60, -7), Some(48));
        assert_eq!(c_major.step(60, 14), Some(84));
        // Pentatonic: five steps are one octave.
        let pent = Scale::new(9, ScaleKind::MinorPentatonic);
        assert_eq!(pent.step(57, 5), Some(69));
        assert_eq!(pent.step(57, -5), Some(45));
        assert_eq!(pent.step(57, 1), Some(60));
        assert_eq!(pent.step(57, -1), Some(55));
        // Zero steps keep the key, even one outside the scale.
        assert_eq!(c_major.step(61, 0), Some(61));
    }

    #[test]
    fn stepping_from_a_key_outside_the_scale_lands_on_the_next_scale_key_first() {
        let c_major = Scale::new(0, ScaleKind::Major);
        assert_eq!(c_major.step(61, 1), Some(62));
        assert_eq!(c_major.step(61, -1), Some(60));
        assert_eq!(c_major.step(61, 2), Some(64));
        assert_eq!(c_major.step(61, -2), Some(59));
        assert_eq!(c_major.step(66, 1), Some(67));
        assert_eq!(c_major.step(66, -1), Some(65));
    }

    #[test]
    fn stepping_at_the_midi_bounds_is_none_beyond_and_exact_at_the_edge() {
        let c_major = Scale::new(0, ScaleKind::Major);
        // 127 is G, 126 is F#, 125 F, 124 E.
        assert_eq!(c_major.step(124, 1), Some(125));
        assert_eq!(c_major.step(125, 1), Some(127));
        assert_eq!(c_major.step(127, 1), None);
        assert_eq!(c_major.step(127, -1), Some(125));
        assert_eq!(c_major.step(125, 2), None);
        assert_eq!(c_major.step(0, -1), None);
        assert_eq!(c_major.step(0, 1), Some(2));
        assert_eq!(c_major.step(1, -1), Some(0));
        assert_eq!(c_major.step(2, -2), None);
        assert_eq!(c_major.step(0, 5000), None);
        assert_eq!(c_major.step(127, i32::MIN), None);
        let chroma = Scale::new(0, ScaleKind::Chromatic);
        assert_eq!(chroma.step(127, 1), None);
        assert_eq!(chroma.step(0, -1), None);
        assert_eq!(chroma.step(60, 3), Some(63));
    }

    #[test]
    fn degrees_between_inverts_step_for_in_scale_targets() {
        let scales = [
            Scale::new(0, ScaleKind::Major),
            Scale::new(9, ScaleKind::MinorPentatonic),
            Scale::new(4, ScaleKind::HungarianMinor),
        ];
        for s in scales {
            for from in (0..=127u8).step_by(3) {
                for to in 0..=127u8 {
                    if !s.contains(to) {
                        continue;
                    }
                    let n = s.degrees_between(from, to);
                    if from == to {
                        assert_eq!(n, 0);
                    }
                    assert_eq!(s.step(from, n), Some(to), "{s:?} {from} -> {to} = {n}");
                }
            }
        }
    }

    #[test]
    fn keys_in_lists_the_scale_keys_of_a_range() {
        let c_major = Scale::new(0, ScaleKind::Major);
        assert_eq!(c_major.keys_in(60, 72), [60, 62, 64, 65, 67, 69, 71, 72]);
        assert_eq!(c_major.keys_in(120, 200).last(), Some(&127));
        assert_eq!(c_major.keys_in(0, 127).len(), 75);
    }

    #[test]
    fn pitch_names_are_sharps_from_c() {
        assert_eq!(pitch_name(0), "C");
        assert_eq!(pitch_name(1), "C#");
        assert_eq!(pitch_name(11), "B");
        assert_eq!(pitch_name(13), "C#");
    }

    #[test]
    fn chord_into_writes_the_documented_triad_and_inversions() {
        let mut out = [0u8; MAX_CHORD_KEYS];
        let n = chord_into(60, ChordKind::Major, 0, &mut out);
        assert_eq!(&out[..n], [60, 64, 67]);
        let n = chord_into(60, ChordKind::Major, 1, &mut out);
        assert_eq!(&out[..n], [64, 67, 72]);
        let n = chord_into(60, ChordKind::Major, 9, &mut out);
        assert_eq!(&out[..n], [67, 72, 76]);
    }

    #[test]
    fn diatonic_chord_into_gives_the_classic_triads_of_c_major() {
        let c = Scale::new(0, ScaleKind::Major);
        let mut out = [0u8; MAX_CHORD_KEYS];
        let n = diatonic_chord_into(c, 62, DiatonicSize::Triad, 0, &mut out);
        assert_eq!(&out[..n], [62, 65, 69]);
        // An out-of-scale key moves to the nearest scale key (ties down) first.
        let n = diatonic_chord_into(c, 61, DiatonicSize::Seventh, 1, &mut out);
        assert_eq!(&out[..n], [60 + 4, 67, 71, 72]);
    }
}
