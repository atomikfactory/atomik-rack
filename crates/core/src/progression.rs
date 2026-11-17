//! Chord progressions: chords, roman numerals and symbols, voicing with voice leading, a
//! seeded mood-based generator and ranked next-chord suggestions. Pure functions on pitch
//! classes and MIDI keys; nothing here knows about clips, the UI or the engine.
//!
//! * **Chords.** A [`Chord`] is a root pitch class, a [`ChordKind`] and an optional slash bass.
//!   [`roman`] names it against a [`Key`] (`I`, `ii`, `V7`, `bVII`, `V7/vi`, `subV7`; numerals are
//!   relative to the major scale of the tonic in both modes, so natural minor reads `i ii° bIII iv v
//!   bVI bVII`); [`symbol`] gives the chord symbol (`Am7`, `G/B`).
//! * **Voicing.** [`voice`] turns a chord into at most eight keys around a register: close, open
//!   (drop-2) or wide (open with the root doubled below), optionally extended to a seventh or
//!   ninth in the key, and with voice leading: the inversion and octave nearest to the previous
//!   voicing. Fixed arrays only, so the audio thread can call it.
//! * **Generator.** [`generate`] walks a weighted transition model per mood (see `moods`):
//!   the weight of a move is the harmonic function pair (tonic, predominant, dominant, colour)
//!   times the root motion times the mood's idioms times the affinity of the chord, with the
//!   colour chords of a mood (borrowed, secondary dominants, tritone substitutes, chromatic
//!   mediants, sus colours) scaled by the spice. The first chord favours the tonic, the last one
//!   the mood's cadence and a lead back into the first chord, a locked chord is fixed and its
//!   neighbours are chosen to lead into and out of it, and a chord never repeats immediately. A
//!   mood's signature progressions are used for about half of the unlocked results. All
//!   arithmetic is integer, the random numbers come from a small splitmix generator seeded by the
//!   caller, so the same inputs give the same progression on every platform.
//! * **Suggestions.** [`suggest_next`] ranks what the same model offers after a chord.

mod moods;
#[cfg(test)]
mod tests;

use crate::harmony::SongChord;
use crate::theory::{ChordKind, MAX_KEY, PITCH_NAMES, Scale};
use moods::{Col, Node, Profile, Template, profile};

/// The most chords a progression holds.
pub const MAX_SLOTS: usize = 8;

/// The chord lengths a slot can take, in beats (the `slot.N.beats` choices).
pub const BEATS: [u8; 8] = [1, 2, 3, 4, 6, 8, 12, 16];

/// The index in [`BEATS`] of the length nearest to `beats`.
pub fn beats_index(beats: u8) -> usize {
    let mut best = 0;
    for (i, b) in BEATS.iter().enumerate() {
        if b.abs_diff(beats) < BEATS[best].abs_diff(beats) {
            best = i;
        }
    }
    best
}

/// The most keys a voicing holds.
pub const MAX_VOICE_NOTES: usize = 8;

/// A chord: root pitch class, quality and optional slash bass pitch class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    /// Root pitch class, `0..12` (C = 0).
    pub root: u8,
    /// The quality.
    pub kind: ChordKind,
    /// The bass pitch class when it is not the root.
    pub bass: Option<u8>,
}

impl Chord {
    /// A chord on `root` (taken modulo 12) without a slash bass.
    pub fn new(root: u8, kind: ChordKind) -> Self {
        Self {
            root: root % 12,
            kind,
            bass: None,
        }
    }

    /// The chord over `bass` (taken modulo 12; the root itself is no slash).
    pub fn over(self, bass: u8) -> Self {
        let bass = bass % 12;
        Self {
            bass: (bass != self.root).then_some(bass),
            ..self
        }
    }

    /// The chord moved up by `semitones` (root and bass).
    pub fn transposed(self, semitones: u8) -> Self {
        Self {
            root: (self.root + semitones % 12) % 12,
            kind: self.kind,
            bass: self.bass.map(|b| (b + semitones % 12) % 12),
        }
    }

    /// The same chord as the song's chord lane holds it.
    pub fn to_song_chord(self) -> SongChord {
        SongChord {
            root: self.root,
            kind: self.kind,
            bass: self.bass,
        }
    }
}

impl From<SongChord> for Chord {
    fn from(c: SongChord) -> Self {
        Self {
            root: c.root % 12,
            kind: c.kind,
            bass: c.bass.map(|b| b % 12),
        }
    }
}

/// Major or minor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyMode {
    /// Major.
    Major,
    /// Natural minor (the dominant V is still available).
    Minor,
}

/// A key: tonic pitch class and mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    /// Tonic pitch class, `0..12`.
    pub root: u8,
    /// The mode.
    pub mode: KeyMode,
}

impl Key {
    /// The key on `root` (taken modulo 12).
    pub fn new(root: u8, mode: KeyMode) -> Self {
        Self {
            root: root % 12,
            mode,
        }
    }

    /// The tonic triad of the key.
    pub fn tonic(self) -> Chord {
        let kind = match self.mode {
            KeyMode::Major => ChordKind::Major,
            KeyMode::Minor => ChordKind::Minor,
        };
        Chord::new(self.root, kind)
    }

    /// Semitones from the tonic up to `root`.
    fn degree(self, root: u8) -> u8 {
        (root % 12 + 12 - self.root) % 12
    }

    /// The chord of `kind` on the degree `d` (semitones above the tonic).
    pub fn chord_at(self, d: u8, kind: ChordKind) -> Chord {
        Chord::new(self.root + d % 12, kind)
    }

    /// Whether the key is written with flats.
    pub fn flats(self) -> bool {
        match self.mode {
            KeyMode::Major => matches!(self.root, 1 | 3 | 5 | 6 | 8 | 10),
            KeyMode::Minor => matches!(self.root, 0 | 2 | 3 | 5 | 7 | 10),
        }
    }
}

/// How far to transpose a progression written in `device` to sit in the song key `song`: the
/// semitones from the device tonic up to the song's tonic, where a song key of the other mode
/// counts as its relative major or minor so the chords still fit. `None` when the scale has no
/// clear mode (chromatic, whole tone).
pub fn song_key_shift(device: Key, song: Scale) -> Option<u8> {
    let intervals = song.kind.intervals();
    let minor_third = intervals.contains(&3);
    let major_third = intervals.contains(&4);
    let song_minor = match (minor_third, major_third) {
        (true, false) => true,
        (false, true) => false,
        _ => return None,
    };
    let tonic = match (song_minor, device.mode == KeyMode::Minor) {
        (a, b) if a == b => song.root % 12,
        // Song major, device minor: the relative minor.
        (false, true) => (song.root % 12 + 9) % 12,
        // Song minor, device major: the relative major.
        _ => (song.root % 12 + 3) % 12,
    };
    Some((tonic + 12 - device.root % 12) % 12)
}

/// The mood of a generated progression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mood {
    /// Radio loops.
    Pop,
    /// Bright and forward.
    Uplifting,
    /// Wistful, descending.
    Melancholy,
    /// Minor-side, falling.
    Dark,
    /// Floating sevenths and sus colours.
    Dreamy,
    /// ii-V-I and turnarounds.
    Jazz,
    /// Lush ninths and thirteenths.
    NeoSoul,
    /// Mellow diatonic sevenths.
    LoFi,
    /// Slow, wide, mediant moves.
    Cinematic,
    /// Big open triads.
    Anthem,
    /// Dominant sevenths on I, IV and V.
    Blues,
    /// Modal vamps without a leading tone.
    Modal,
}

/// The labels of [`Mood::ALL`], in order (the `gen.mood` choices).
pub const MOOD_LABELS: [&str; 12] = [
    "Pop",
    "Uplifting",
    "Melancholy",
    "Dark",
    "Dreamy",
    "Jazz",
    "Neo Soul",
    "Lo-fi",
    "Cinematic",
    "Anthem",
    "Blues",
    "Modal",
];

impl Mood {
    /// Every mood, in menu order.
    pub const ALL: [Mood; 12] = [
        Mood::Pop,
        Mood::Uplifting,
        Mood::Melancholy,
        Mood::Dark,
        Mood::Dreamy,
        Mood::Jazz,
        Mood::NeoSoul,
        Mood::LoFi,
        Mood::Cinematic,
        Mood::Anthem,
        Mood::Blues,
        Mood::Modal,
    ];

    /// The position in [`Mood::ALL`].
    pub fn index(self) -> usize {
        Mood::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }

    /// The mood at `i` (`None` past the end).
    pub fn from_index(i: usize) -> Option<Mood> {
        Mood::ALL.get(i).copied()
    }

    /// The name shown in menus.
    pub fn label(self) -> &'static str {
        MOOD_LABELS[self.index()]
    }
}

/// How strongly the model recommends a suggested chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Strength {
    /// A functional move of the mood.
    Strong,
    /// A colour move of the mood: borrowed, secondary dominant, substitute, mediant, sus.
    Colour,
    /// Outside the mood's vocabulary.
    Surprise,
}

/// How the notes of a voicing are spread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Spread {
    /// The chord as stacked.
    Close,
    /// Drop-2: the second note from the top moves down an octave.
    Open,
    /// Open, with the root doubled an octave down.
    Wide,
}

impl Spread {
    /// The spread of a `voice.spread` choice (past the end: wide).
    pub fn from_index(i: usize) -> Spread {
        match i {
            0 => Spread::Close,
            1 => Spread::Open,
            _ => Spread::Wide,
        }
    }
}

/// How far a chord is extended before voicing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Extend {
    /// As written.
    AsWritten,
    /// Triads become sevenths in the key.
    Sevenths,
    /// Triads and sevenths become ninths in the key.
    Ninths,
}

impl Extend {
    /// The extension of a `voice.extend` choice (past the end: ninths).
    pub fn from_index(i: usize) -> Extend {
        match i {
            0 => Extend::AsWritten,
            1 => Extend::Sevenths,
            _ => Extend::Ninths,
        }
    }
}

/// The options of [`voice`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceOpts {
    /// The key the chords are read in (for [`Extend`]).
    pub key: Key,
    /// Pick the inversion and octave nearest to the previous voicing.
    pub lead: bool,
    /// The register: 0 low (C2), 1 mid-low (C3), 2 mid (C4), 3 high (C5).
    pub register: u8,
    /// The spread.
    pub spread: Spread,
    /// The extension.
    pub extend: Extend,
}

/// The keys of one voiced chord, ascending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Voicing {
    /// The keys; only the first `len` count.
    pub notes: [u8; MAX_VOICE_NOTES],
    /// How many keys sound.
    pub len: u8,
}

impl Voicing {
    /// No keys.
    pub const EMPTY: Voicing = Voicing {
        notes: [0; MAX_VOICE_NOTES],
        len: 0,
    };

    /// The keys, ascending.
    pub fn as_slice(&self) -> &[u8] {
        &self.notes[..usize::from(self.len)]
    }

    /// The lowest key.
    pub fn lowest(&self) -> Option<u8> {
        self.as_slice().first().copied()
    }

    /// The highest key.
    pub fn highest(&self) -> Option<u8> {
        self.as_slice().last().copied()
    }
}

/// A generated progression: chords and the suggested length of every slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progression {
    /// The chords; slots past the requested count hold locks or the tonic.
    pub chords: [Chord; MAX_SLOTS],
    /// The suggested lengths in beats (always one of [`BEATS`]).
    pub beats: [u8; MAX_SLOTS],
}

// ------------------------------------------------------------------------------------------
// Names

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Major,
    Minor,
    Dim,
}

fn family(kind: ChordKind) -> Family {
    use ChordKind as K;
    match kind {
        K::Minor | K::MinorSixth | K::MinorSeventh | K::MinorNinth => Family::Minor,
        K::Dim | K::HalfDim | K::DimSeventh => Family::Dim,
        _ => Family::Major,
    }
}

/// A chord that can be a dominant: a major triad or a dominant seventh family chord.
fn dominant_type(kind: ChordKind) -> bool {
    matches!(
        kind,
        ChordKind::Major
            | ChordKind::Seventh
            | ChordKind::Ninth
            | ChordKind::Eleventh
            | ChordKind::Thirteenth
    )
}

/// A dominant chord with a seventh.
fn dominant_seventh(kind: ChordKind) -> bool {
    dominant_type(kind) && kind != ChordKind::Major
}

/// The family of the diatonic triad on degree `d` of the mode (the minor key's dominant V is
/// handled by the callers).
fn diatonic(mode: KeyMode, d: u8) -> Option<Family> {
    match (mode, d) {
        (KeyMode::Major, 0 | 5 | 7) => Some(Family::Major),
        (KeyMode::Major, 2 | 4 | 9) => Some(Family::Minor),
        (KeyMode::Major, 11) => Some(Family::Dim),
        (KeyMode::Minor, 0 | 5) => Some(Family::Minor),
        (KeyMode::Minor, 3 | 8 | 10) => Some(Family::Major),
        (KeyMode::Minor, 7) => Some(Family::Minor),
        (KeyMode::Minor, 2 | 11) => Some(Family::Dim),
        _ => None,
    }
}

/// Whether a secondary dominant can resolve to degree `d`.
fn is_target(mode: KeyMode, d: u8) -> bool {
    d != 0 && d != 11 && diatonic(mode, d).is_some()
}

const ROMAN: [(&str, &str); 12] = [
    ("", "I"),
    ("b", "II"),
    ("", "II"),
    ("b", "III"),
    ("", "III"),
    ("", "IV"),
    ("b", "V"),
    ("", "V"),
    ("b", "VI"),
    ("", "VI"),
    ("b", "VII"),
    ("", "VII"),
];

fn numeral(d: u8, fam: Family) -> String {
    let (acc, letters) = ROMAN[usize::from(d % 12)];
    let (acc, letters) = match (d, fam) {
        (6, Family::Dim) => ("#", "IV"),
        _ => (acc, letters),
    };
    match fam {
        Family::Major => format!("{acc}{letters}"),
        _ => format!("{acc}{}", letters.to_lowercase()),
    }
}

fn roman_suffix(kind: ChordKind) -> &'static str {
    use ChordKind as K;
    match kind {
        K::Major | K::Minor => "",
        K::Dim => "\u{b0}",
        K::Aug => "+",
        K::Sus2 => "sus2",
        K::Sus4 => "sus4",
        K::Sixth | K::MinorSixth => "6",
        K::Seventh | K::MinorSeventh => "7",
        K::MajorSeventh => "maj7",
        K::HalfDim => "\u{f8}7",
        K::DimSeventh => "\u{b0}7",
        K::Ninth | K::MinorNinth => "9",
        K::MajorNinth => "maj9",
        K::Add9 => "add9",
        K::Eleventh => "11",
        K::Thirteenth => "13",
        K::Power => "5",
        K::Octave => "8",
    }
}

/// The roman numeral of `chord` in `key`: `I`, `ii`, `V7`, `bVII`, `vii°`, `V7/vi`, `subV7`.
/// The numeral is relative to the major scale of the tonic; the slash bass is not shown.
pub fn roman(key: Key, chord: Chord) -> String {
    let d = key.degree(chord.root);
    let fam = family(chord.kind);
    let suffix = roman_suffix(chord.kind);
    // The minor key's IV (dorian) is a plain IV, not the dominant of bVII.
    let in_key = d == 7
        || (d == 0 && fam != Family::Dim)
        || (d == 5 && key.mode == KeyMode::Minor && fam == Family::Major)
        || diatonic(key.mode, d) == Some(fam);
    if dominant_type(chord.kind) && !in_key {
        if d == 1 && dominant_seventh(chord.kind) {
            return format!("subV{suffix}");
        }
        let t = (d + 5) % 12;
        if is_target(key.mode, t) {
            let tfam = diatonic(key.mode, t).unwrap_or(Family::Major);
            return format!("V{suffix}/{}", numeral(t, tfam));
        }
    }
    format!("{}{suffix}", numeral(d, fam))
}

fn name(pc: u8, flats: bool) -> &'static str {
    const FLAT: [&str; 12] = [
        "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
    ];
    if flats {
        FLAT[usize::from(pc % 12)]
    } else {
        PITCH_NAMES[usize::from(pc % 12)]
    }
}

fn symbol_suffix(kind: ChordKind) -> &'static str {
    use ChordKind as K;
    match kind {
        K::Major => "",
        K::Minor => "m",
        K::Dim => "dim",
        K::Aug => "aug",
        K::Sus2 => "sus2",
        K::Sus4 => "sus4",
        K::Sixth => "6",
        K::MinorSixth => "m6",
        K::Seventh => "7",
        K::MajorSeventh => "maj7",
        K::MinorSeventh => "m7",
        K::HalfDim => "m7b5",
        K::DimSeventh => "dim7",
        K::Ninth => "9",
        K::MajorNinth => "maj9",
        K::MinorNinth => "m9",
        K::Add9 => "add9",
        K::Eleventh => "11",
        K::Thirteenth => "13",
        K::Power => "5",
        K::Octave => "(8)",
    }
}

fn symbol_with(chord: Chord, flats: bool) -> String {
    let mut s = format!("{}{}", name(chord.root, flats), symbol_suffix(chord.kind));
    if let Some(b) = chord.bass {
        s.push('/');
        s.push_str(name(b, flats));
    }
    s
}

/// The chord symbol with sharps: `Am7`, `G/B`, `F#dim`.
pub fn symbol(chord: Chord) -> String {
    symbol_with(chord, false)
}

/// The chord symbol spelled for `key` (flats in a flat key): `Bbmaj7` in F.
pub fn symbol_in(key: Key, chord: Chord) -> String {
    symbol_with(chord, key.flats())
}

// ------------------------------------------------------------------------------------------
// Voicing

/// The chord as the extension setting writes it, read in `key`: a major triad becomes a major
/// seventh (a dominant seventh on V or when it is a secondary dominant), a minor triad a minor
/// seventh, a diminished triad a half-diminished seventh; with [`Extend::Ninths`] the sevenths
/// become ninths and a major triad an add9. Sus, sixth, power and chords that already have a ninth
/// or more stay as written.
pub fn extend_chord(key: Key, chord: Chord, extend: Extend) -> Chord {
    use ChordKind as K;
    if extend == Extend::AsWritten {
        return chord;
    }
    let d = key.degree(chord.root);
    let secondary = d != 0
        && diatonic(key.mode, d) != Some(Family::Major)
        && is_target(key.mode, (d + 5) % 12)
        && chord.kind == K::Major;
    let dominant = d == 7 || secondary;
    let ninths = extend == Extend::Ninths;
    let kind = match (chord.kind, ninths) {
        (K::Major, false) if dominant => K::Seventh,
        (K::Major, false) => K::MajorSeventh,
        (K::Major, true) if dominant => K::Ninth,
        (K::Major, true) => K::Add9,
        (K::Minor, false) => K::MinorSeventh,
        (K::Minor, true) => K::MinorNinth,
        (K::Dim, _) => K::HalfDim,
        (K::Add9, false) if dominant => K::Ninth,
        (K::Add9, false) => K::MajorNinth,
        (K::Seventh, true) => K::Ninth,
        (K::MajorSeventh, true) => K::MajorNinth,
        (K::MinorSeventh, true) => K::MinorNinth,
        (other, _) => other,
    };
    Chord { kind, ..chord }
}

const CENTRES: [i32; 4] = [36, 48, 60, 72];

/// The chord's stack as offsets from the root, ascending, with the spread applied.
fn shape(kind: ChordKind, spread: Spread) -> ([i32; MAX_VOICE_NOTES], usize) {
    let mut out = [0i32; MAX_VOICE_NOTES];
    let mut n = 0;
    for i in kind.intervals() {
        if n < MAX_VOICE_NOTES {
            out[n] = i32::from(*i);
            n += 1;
        }
    }
    out[..n].sort_unstable();
    if spread != Spread::Close && n >= 3 {
        if n == 3 {
            // Root, fifth, third an octave up.
            let (a, b, c) = (out[0], out[1], out[2]);
            out[0] = a;
            out[1] = c;
            out[2] = b + 12;
        } else {
            out[n - 2] -= 12;
        }
        out[..n].sort_unstable();
    }
    if spread == Spread::Wide && n < MAX_VOICE_NOTES {
        out.copy_within(0..n, 1);
        out[0] = -12;
        n += 1;
    }
    (out, n)
}

/// The sum of the distances of every key of `a` to the nearest of `b` and the other way round.
fn movement(a: &[i32], b: &[i32]) -> i32 {
    let near = |x: i32, set: &[i32]| set.iter().map(|y| (x - y).abs()).min().unwrap_or(0);
    a.iter().map(|x| near(*x, b)).sum::<i32>() + b.iter().map(|y| near(*y, a)).sum::<i32>()
}

/// Voices `chord` (after `opts.extend`) around the register: the chord tones only, ascending,
/// at most eight keys, every key in `0..=127`. Without `opts.lead` or without a previous
/// voicing the root position nearest to the register centre is taken; with both, the inversion
/// and octave whose keys move least from `prev` (ties: nearer the centre, lower inversion).
/// The mean of a voicing stays within a tritone of the register centre where the range allows.
pub fn voice(chord: Chord, prev: Option<&Voicing>, opts: VoiceOpts) -> Voicing {
    let chord = extend_chord(opts.key, chord, opts.extend);
    let centre = CENTRES[usize::from(opts.register.min(3))];
    let (base, n) = shape(chord.kind, opts.spread);
    let prev_keys: Option<[i32; MAX_VOICE_NOTES]> = prev.filter(|_| opts.lead).map(|p| {
        let mut k = [0i32; MAX_VOICE_NOTES];
        for (dst, src) in k.iter_mut().zip(p.as_slice()) {
            *dst = i32::from(*src);
        }
        k
    });
    let prev_len = prev.map_or(0, |p| usize::from(p.len));
    let rotations = if n >= 3 && prev_keys.is_some() { n } else { 1 };

    // Candidate key: (inside the window, cost, distance from the centre, rotation, root octave).
    type Rank = (bool, i32, i32, usize, i32);
    let mut best: Option<(Rank, [i32; MAX_VOICE_NOTES])> = None;
    for r in 0..rotations {
        let mut shape = base;
        for _ in 0..r {
            let low = shape[0];
            shape.copy_within(1..n, 0);
            shape[n - 1] = low + 12;
            shape[..n].sort_unstable();
        }
        for oct in 0..11 {
            let root = i32::from(chord.root) + 12 * oct;
            let mut keys = [0i32; MAX_VOICE_NOTES];
            let mut ok = true;
            let mut sum = 0;
            for i in 0..n {
                keys[i] = root + shape[i];
                ok &= (0..=i32::from(MAX_KEY)).contains(&keys[i]);
                sum += keys[i];
            }
            if !ok {
                continue;
            }
            let dev = (sum - centre * n as i32).abs();
            let inside = dev <= 7 * n as i32;
            let cost = match &prev_keys {
                Some(p) => movement(&keys[..n], &p[..prev_len]),
                None => 0,
            };
            // Outside the window loses against anything inside it.
            let rank = (!inside, cost, dev, r, oct);
            if best.as_ref().is_none_or(|(b, _)| rank < *b) {
                best = Some((rank, keys));
            }
        }
    }
    let mut out = Voicing::EMPTY;
    if let Some((_, keys)) = best {
        for (dst, k) in out.notes.iter_mut().zip(&keys[..n]) {
            *dst = *k as u8;
        }
        out.len = n as u8;
    }
    out
}

// ------------------------------------------------------------------------------------------
// Generator

/// splitmix64: a small deterministic generator, integer arithmetic only.
struct Prng(u64);

impl Prng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n` (0 for an empty range).
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { (self.next() >> 11) % n }
    }

    /// Whether a roll of 100 lands under `pct`.
    fn chance(&mut self, pct: u8) -> bool {
        self.below(100) < u64::from(pct)
    }
}

/// Harmonic function of a degree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Func {
    Tonic,
    Pre,
    Dom,
    Colour,
}

impl Func {
    fn index(self) -> usize {
        match self {
            Func::Tonic => 0,
            Func::Pre => 1,
            Func::Dom => 2,
            Func::Colour => 3,
        }
    }
}

const T: Func = Func::Tonic;
const P: Func = Func::Pre;
const D: Func = Func::Dom;
const C: Func = Func::Colour;

const FUNC_MAJOR: [Func; 12] = [T, C, P, C, T, P, C, D, P, T, D, D];
const FUNC_MINOR: [Func; 12] = [T, P, P, T, C, P, C, D, P, C, D, D];

/// Base weight of a move between functions (tonic, predominant, dominant, colour).
const FUNC_MOVE: [[u64; 4]; 4] = [[4, 9, 6, 4], [5, 3, 10, 2], [12, 3, 2, 2], [7, 6, 6, 3]];

/// Preference by root motion in semitones up: a fourth up (fifth down) and a third down lead.
const ROOT_MOVE: [u64; 12] = [20, 60, 100, 90, 70, 140, 40, 110, 100, 120, 90, 70];

/// A chord as the transition model sees it.
#[derive(Debug, Clone, Copy)]
struct Pt {
    d: u8,
    f: Func,
    /// The degree it wants to resolve to (secondary dominants, substitutes).
    res: Option<u8>,
}

fn func_of(mode: KeyMode, d: u8) -> Func {
    match mode {
        KeyMode::Major => FUNC_MAJOR[usize::from(d % 12)],
        KeyMode::Minor => FUNC_MINOR[usize::from(d % 12)],
    }
}

fn node_pt(mode: KeyMode, n: &Node) -> Pt {
    let res = match n.c {
        Col::Secondary(t) | Col::Tritone(t) => Some(t),
        _ => None,
    };
    Pt {
        d: n.d,
        f: if res.is_some() {
            Func::Dom
        } else {
            func_of(mode, n.d)
        },
        res,
    }
}

/// The model's view of any chord (a locked or previous chord that is not in a vocabulary).
fn classify(key: Key, chord: Chord) -> Pt {
    let d = key.degree(chord.root);
    let mut res = None;
    if dominant_type(chord.kind)
        && d != 0
        && d != 7
        && diatonic(key.mode, d) != Some(family(chord.kind))
    {
        let t = (d + 5) % 12;
        if is_target(key.mode, t) {
            res = Some(t);
        } else if dominant_seventh(chord.kind) && is_target(key.mode, (d + 11) % 12) {
            res = Some((d + 11) % 12);
        }
    }
    Pt {
        d,
        f: if res.is_some() {
            Func::Dom
        } else {
            func_of(key.mode, d)
        },
        res,
    }
}

/// The weight of the move `a -> b` in the mood's profile (always at least 1).
fn trans(prof: &Profile, a: Pt, b: Pt) -> u64 {
    let mut v = FUNC_MOVE[a.f.index()][b.f.index()] * ROOT_MOVE[usize::from((b.d + 12 - a.d) % 12)];
    if let Some((_, _, pct)) = prof.idioms.iter().find(|(f, t, _)| *f == a.d && *t == b.d) {
        v = v * u64::from(*pct) / 100;
    }
    if let Some(t) = a.res {
        v = if b.d == t { v * 8 } else { v / 4 };
    }
    v.max(1)
}

/// The spice as permille.
fn permille(spice: f32) -> u64 {
    (spice.clamp(0.0, 1.0) * 1000.0).round() as u64
}

/// How much of a chord's weight survives the spice (permille).
fn gate(c: Col, s: u64) -> u64 {
    match c {
        Col::Core => 1000,
        Col::Borrowed | Col::Secondary(_) | Col::Colour => s,
        Col::Tritone(_) | Col::Mediant => s * s / 1000,
    }
}

fn pct_of(list: &[(u8, u16)], d: u8, default: u64) -> u64 {
    list.iter()
        .find(|(x, _)| *x == d)
        .map_or(default, |(_, p)| u64::from(*p))
}

/// Which end of the progression a slot is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Start,
    Mid,
    Half,
    Final,
}

struct Gen<'a> {
    key: Key,
    prof: &'static Profile,
    s: u64,
    n: usize,
    rng: Prng,
    locks: &'a [Option<Chord>; MAX_SLOTS],
    chords: [Chord; MAX_SLOTS],
    /// The first chord of the next pass (the chord a final or half slot leads into).
    phrase: bool,
}

impl Gen<'_> {
    /// Chooses the chord of slot `i` from what is before it.
    fn pick(&mut self, i: usize) -> Chord {
        let key = self.key;
        let mode = key.mode;
        let place = if i == 0 {
            Place::Start
        } else if i + 1 == self.n {
            Place::Final
        } else if self.n == MAX_SLOTS && i == 3 {
            Place::Half
        } else {
            Place::Mid
        };
        let prev = (i > 0).then(|| self.chords[i - 1]);
        let prev_pt = prev.map(|c| classify(key, c));
        // The chord this one has to lead into.
        let target = if i + 1 < self.n {
            self.locks[i + 1].or(if place == Place::Half && self.phrase {
                Some(self.chords[0])
            } else {
                None
            })
        } else if self.n > 1 {
            Some(self.chords[0])
        } else {
            None
        };
        let target_pt = target.map(|c| classify(key, c));
        let mut weights = [0u64; 32];
        let nodes: &[Node] = self.prof.nodes;
        let mut total = 0u64;
        for (j, node) in nodes.iter().enumerate().take(weights.len()) {
            let g = gate(node.c, self.s);
            if g == 0 {
                continue;
            }
            let chord = key.chord_at(node.d, node.k);
            if prev == Some(chord) || target == Some(chord) {
                continue;
            }
            // Two chords on one degree only follow each other where the mood asks for it (a sus
            // resolving, a seventh blooming into a ninth).
            if prev_pt.is_some_and(|p| p.d == node.d)
                && !self
                    .prof
                    .idioms
                    .iter()
                    .any(|(f, t, _)| *f == node.d && *t == node.d)
            {
                continue;
            }
            let pt = node_pt(mode, node);
            let mut w = u64::from(node.w) * g / 10;
            w = match prev_pt {
                Some(p) => w * trans(self.prof, p, pt),
                None => {
                    w * 100 * pct_of(self.prof.start, node.d, if node.d == 0 { 500 } else { 30 })
                        / 100
                }
            };
            match place {
                Place::Final => w = w * pct_of(self.prof.end, node.d, 60) / 100,
                Place::Half => w = w * pct_of(self.prof.half, node.d, 70) / 100,
                _ => {}
            }
            if let Some(t) = target_pt {
                w = w * (100 + trans(self.prof, pt, t)) / 100;
            }
            let seen = self.chords[..i].iter().filter(|c| **c == chord).count();
            for _ in 0..seen {
                w = w * u64::from(self.prof.diversity) / 100;
            }
            weights[j] = w;
            total += w;
        }
        if total == 0 {
            return key.tonic();
        }
        let mut r = self.rng.below(total);
        for (j, w) in weights.iter().enumerate().take(nodes.len()) {
            if r < *w {
                return key.chord_at(nodes[j].d, nodes[j].k);
            }
            r -= *w;
        }
        key.tonic()
    }

    /// Fills slots `from..to` with the chosen chords, locks first.
    fn fill(&mut self, from: usize, to: usize) {
        for i in from..to {
            self.chords[i] = match self.locks[i] {
                Some(c) => c,
                None => self.pick(i),
            };
        }
    }

    /// A signature progression of exactly `len` steps, if one fits and the roll takes it.
    fn template(&mut self, len: usize) -> Option<([Chord; MAX_SLOTS], [u8; MAX_SLOTS])> {
        let prof = self.prof;
        let mut weights = [0u64; 12];
        let mut total = 0u64;
        for (t, w) in prof.templates.iter().zip(weights.iter_mut()) {
            if t.steps.len() != len || self.s < u64::from(t.min_spice) {
                continue;
            }
            *w = u64::from(t.w) * if t.min_spice == 0 { 1000 } else { self.s };
            total += *w;
        }
        if total == 0 || !self.rng.chance(prof.tmpl) {
            return None;
        }
        let mut r = self.rng.below(total);
        let mut chosen: Option<&Template> = None;
        for (t, w) in prof.templates.iter().zip(weights) {
            if r < w {
                chosen = Some(t);
                break;
            }
            r -= w;
        }
        let t = chosen?;
        let mut chords = self.chords;
        let mut beats = [4u8; MAX_SLOTS];
        for (i, (d, k, b)) in t.steps.iter().enumerate() {
            chords[i] = self.key.chord_at(*d, *k);
            beats[i] = *b;
        }
        Some((chords, beats))
    }
}

/// Lays out the chord lengths of a generated progression.
fn lengths(
    prof: &Profile,
    key: Key,
    n: usize,
    chords: &[Chord; MAX_SLOTS],
    rng: &mut Prng,
) -> [u8; MAX_SLOTS] {
    let mut beats = [4u8; MAX_SLOTS];
    let len = prof.len;
    let mut split = [false; MAX_SLOTS];
    if len.split_pair {
        let mut i = 0;
        while i + 1 < n {
            let a = func_of(key.mode, key.degree(chords[i].root));
            let b = classify(key, chords[i + 1]).f;
            if a == Func::Pre && b == Func::Dom {
                beats[i] = 2;
                beats[i + 1] = 2;
                split[i] = true;
                split[i + 1] = true;
                i += 2;
            } else {
                i += 1;
            }
        }
    }
    if len.long_any > 0 && n <= 4 {
        for i in 0..n {
            if !split[i] && rng.chance(len.long_any) {
                beats[i] = 8;
            }
        }
    }
    if len.long_last > 0 && n > 1 && !split[n - 1] && beats[n - 1] == 4 && rng.chance(len.long_last)
    {
        beats[n - 1] = 8;
    }
    beats
}

/// Generates a progression of `slots` chords (1..=8) in `key`.
///
/// `spice` (0..=1) scales the colour chords of the mood: at 0 only its core chords sound. `seed`
/// picks one result; the same arguments always give the same progression. A `Some` entry of
/// `locks` fixes that slot (at any index, also past `slots`), and its neighbours are chosen to
/// lead into and out of it. The suggested lengths cover all slots; the caller applies them to the
/// slots it writes.
pub fn generate(
    key: Key,
    mood: Mood,
    spice: f32,
    seed: u32,
    slots: usize,
    locks: &[Option<Chord>; MAX_SLOTS],
) -> Progression {
    let n = slots.clamp(1, MAX_SLOTS);
    let prof = profile(mood.index(), key.mode == KeyMode::Minor);
    let mix = u64::from(seed)
        ^ ((mood.index() as u64) << 32)
        ^ (u64::from(key.mode == KeyMode::Minor) << 40)
        ^ ((n as u64) << 44);
    let mut rng = Prng::new(mix);
    // Warm the generator so nearby seeds diverge.
    rng.next();
    let mut g = Gen {
        key,
        prof,
        s: permille(spice),
        n,
        rng,
        locks,
        chords: [key.tonic(); MAX_SLOTS],
        phrase: false,
    };
    // Lengths a signature progression brings: the first `fixed` slots.
    let mut sig: Option<([u8; MAX_SLOTS], usize)> = None;
    let any_lock = locks[..n].iter().any(Option::is_some);
    if !any_lock {
        if let Some((chords, b)) = g.template(n) {
            g.chords = chords;
            sig = Some((b, n));
        } else if n == MAX_SLOTS && g.rng.chance(prof.repeat) {
            // A phrase of four, played twice, with a new ending.
            g.phrase = true;
            let mut b = [4u8; MAX_SLOTS];
            if let Some((chords, tb)) = g.template(4) {
                g.chords = chords;
                b[..4].copy_from_slice(&tb[..4]);
            } else {
                g.fill(0, 4);
                let first = lengths(prof, key, 4, &g.chords, &mut g.rng);
                b[..4].copy_from_slice(&first[..4]);
            }
            for i in 4..7 {
                g.chords[i] = g.chords[i - 4];
                b[i] = b[i - 4];
            }
            g.chords[7] = g.pick(7);
            sig = Some((b, 7));
        } else {
            g.fill(0, n);
        }
    } else {
        g.fill(0, n);
    }
    // Slots past the count keep their locks.
    for (i, lock) in locks.iter().enumerate().skip(n) {
        if let Some(c) = lock {
            g.chords[i] = *c;
        }
    }
    let mut out_beats = lengths(prof, key, n, &g.chords, &mut g.rng);
    if let Some((b, fixed)) = sig {
        out_beats[..fixed].copy_from_slice(&b[..fixed]);
        // A half bar left alone (its partner was copied away or replaced) becomes a whole bar.
        let mut i = 0;
        while i < n {
            if out_beats[i] != 2 {
                i += 1;
            } else if i + 1 < n && out_beats[i + 1] == 2 {
                i += 2;
            } else {
                out_beats[i] = 4;
                i += 1;
            }
        }
    }
    for b in &mut out_beats {
        if !BEATS.contains(b) {
            *b = BEATS[beats_index(*b)];
        }
    }
    Progression {
        chords: g.chords,
        beats: out_beats,
    }
}

/// The chords a mood offers in a key at `spice`, without repeats: its core chords always, the
/// colour chords once the spice reaches them (at 0 only the core).
pub fn vocabulary(key: Key, mood: Mood, spice: f32) -> Vec<Chord> {
    let prof = profile(mood.index(), key.mode == KeyMode::Minor);
    let s = permille(spice);
    let mut out: Vec<Chord> = Vec::new();
    for n in prof.nodes {
        if gate(n.c, s) > 0 {
            let c = key.chord_at(n.d, n.k);
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
    out
}

/// What the model offers after `prev`, best first: every chord of the mood's vocabulary
/// ([`Strength::Strong`] for its core, [`Strength::Colour`] for its colour chords, which the spice
/// lifts) and a handful of chromatic chords outside it ([`Strength::Surprise`]). The chord that
/// is `prev` itself is left out.
pub fn suggest_next(key: Key, mood: Mood, prev: Chord, spice: f32) -> Vec<(Chord, Strength)> {
    let prof = profile(mood.index(), key.mode == KeyMode::Minor);
    let s = permille(spice);
    let from = classify(key, prev);
    let mut scored: Vec<(u64, u8, Chord, Strength)> = Vec::new();
    for n in prof.nodes {
        let chord = key.chord_at(n.d, n.k);
        if chord == prev || scored.iter().any(|(_, _, c, _)| *c == chord) {
            continue;
        }
        // The colour chords always show, the spice moves them up.
        let g = match n.c {
            Col::Core => 1000,
            _ => 250 + 750 * gate(n.c, s) / 1000,
        };
        let w = trans(prof, from, node_pt(key.mode, n)) * u64::from(n.w) * g / 10;
        let strength = if n.c == Col::Core {
            Strength::Strong
        } else {
            Strength::Colour
        };
        scored.push((w, n.d, chord, strength));
    }
    let surprises: &[(u8, ChordKind)] = match key.mode {
        KeyMode::Major => &[
            (1, ChordKind::Major),
            (3, ChordKind::Major),
            (6, ChordKind::Dim),
            (8, ChordKind::Major),
            (10, ChordKind::Major),
            (4, ChordKind::Major),
            (9, ChordKind::Major),
            (2, ChordKind::Major),
            (5, ChordKind::Minor),
        ],
        KeyMode::Minor => &[
            (1, ChordKind::Major),
            (4, ChordKind::Major),
            (6, ChordKind::Dim),
            (9, ChordKind::Dim),
            (2, ChordKind::Major),
            (5, ChordKind::Major),
            (7, ChordKind::Major),
            (11, ChordKind::Dim),
        ],
    };
    for (d, kind) in surprises {
        let chord = key.chord_at(*d, *kind);
        let family_taken = scored
            .iter()
            .any(|(_, sd, c, _)| *sd == *d && family(c.kind) == family(*kind));
        if chord == prev || family_taken {
            continue;
        }
        let to = classify(key, chord);
        scored.push((
            trans(prof, from, to) * 3 / 10,
            *d,
            chord,
            Strength::Surprise,
        ));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, c, st)| (c, st)).collect()
}
