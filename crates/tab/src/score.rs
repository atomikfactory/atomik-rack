//! The neutral tab document that every file reader and writer converts to and from.
//!
//! A [`TabScore`] holds the song-level facts (titles, tempo, meter and key maps, section
//! markers) and a list of [`TabTrack`]s. A track is one fretted instrument: its tuning, its capo
//! and its notes as `(tick, string, fret)` with the playing [`Techniques`]. Ticks are at the
//! Atomik Rack resolution ([`atomik_core::PPQ`] = 15 360 per quarter note), repeats are expanded by
//! the readers (the original bar structure is kept in [`TabScore::bars`] when a format has it).
//!
//! # Conventions
//!
//! * Strings are in tab order (index 0 = top line, see [`crate::instrument`]) and frets are
//!   relative to the capo.
//! * `hammer-on` and `pull-off` mark the note that is **played by** the hammer or the pull (the
//!   destination), because that is the note whose attack they change. A reader whose format stores
//!   the flag on the note it starts from moves it to the next note.
//! * `slide` is stored on the note it **starts from** (legato and shift slides run to the next
//!   note; the in and out slides are about this note alone).
//! * A tied note (`tie`) continues the previous note on the same string: it is not struck again.
//! * Bend and whammy curves are lists of `(position 0..1 along the note, cents)` points.

use crate::chord::ChordSymbol;
use crate::fretting::Placement;
use crate::instrument::{Capo, Tuning};
use crate::transpose::Key;
use crate::voicing::Voicing;

/// Ticks per quarter note (the Atomik Rack resolution).
pub const PPQ: i64 = atomik_core::PPQ;

/// Size limits shared by every reader, so that hostile input cannot exhaust memory.
pub mod limits {
    /// Tracks in a score.
    pub const MAX_TRACKS: usize = 128;
    /// Notes in one track.
    pub const MAX_NOTES_PER_TRACK: usize = 400_000;
    /// Notes in a whole score.
    pub const MAX_NOTES: usize = 1_000_000;
    /// Bars in a score.
    pub const MAX_BARS: usize = 20_000;
    /// Characters in one text field (titles, lyrics syllables, names).
    pub const MAX_TEXT: usize = 4_096;
    /// Points on one bend curve.
    pub const MAX_CURVE_POINTS: usize = 64;
    /// Tempo, meter, key and marker changes.
    pub const MAX_CHANGES: usize = 20_000;
}

/// The playing techniques that are plain on/off switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TechFlags(pub u32);

impl TechFlags {
    /// The note is played by a hammer-on.
    pub const HAMMER_ON: TechFlags = TechFlags(1 << 0);
    /// The note is played by a pull-off.
    pub const PULL_OFF: TechFlags = TechFlags(1 << 1);
    /// Palm muted.
    pub const PALM_MUTE: TechFlags = TechFlags(1 << 2);
    /// Let the note ring.
    pub const LET_RING: TechFlags = TechFlags(1 << 3);
    /// A dead (muted, percussive) note.
    pub const DEAD: TechFlags = TechFlags(1 << 4);
    /// A ghost note (quiet, in brackets).
    pub const GHOST: TechFlags = TechFlags(1 << 5);
    /// Accented.
    pub const ACCENT: TechFlags = TechFlags(1 << 6);
    /// Heavily accented (marcato).
    pub const HEAVY_ACCENT: TechFlags = TechFlags(1 << 7);
    /// Two-hand tapping.
    pub const TAP: TechFlags = TechFlags(1 << 8);
    /// Slapped (bass).
    pub const SLAP: TechFlags = TechFlags(1 << 9);
    /// Popped (bass).
    pub const POP: TechFlags = TechFlags(1 << 10);
    /// Staccato.
    pub const STACCATO: TechFlags = TechFlags(1 << 11);
    /// Faded in (volume swell).
    pub const FADE_IN: TechFlags = TechFlags(1 << 12);
    /// Played with the thumb (banjo and fingerstyle rolls).
    pub const THUMB: TechFlags = TechFlags(1 << 13);
    /// Every defined flag.
    pub const ALL: TechFlags = TechFlags((1 << 14) - 1);

    /// No flag.
    pub const NONE: TechFlags = TechFlags(0);

    /// Whether every flag of `other` is set.
    pub fn contains(self, other: TechFlags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Sets the flags of `other`.
    pub fn insert(&mut self, other: TechFlags) {
        self.0 |= other.0;
    }

    /// Clears the flags of `other`.
    pub fn remove(&mut self, other: TechFlags) {
        self.0 &= !other.0;
    }

    /// Whether no flag is set.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for TechFlags {
    type Output = TechFlags;
    fn bitor(self, rhs: TechFlags) -> TechFlags {
        TechFlags(self.0 | rhs.0)
    }
}

/// How a note slides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlideKind {
    /// Slide to the next note without picking it again.
    Legato,
    /// Slide to the next note and pick it.
    Shift,
    /// Slide up into the note from below.
    InFromBelow,
    /// Slide down into the note from above.
    InFromAbove,
    /// Slide down out of the note.
    OutDown,
    /// Slide up out of the note.
    OutUp,
}

/// Vibrato width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VibratoKind {
    /// A slight vibrato.
    Normal,
    /// A wide vibrato.
    Wide,
}

/// The kind of a harmonic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HarmonicKind {
    /// A natural harmonic at the fret.
    Natural,
    /// An artificial harmonic.
    Artificial,
    /// A pinch harmonic.
    Pinch,
    /// A tapped harmonic.
    Tapped,
    /// A semi-harmonic.
    Semi,
}

/// A harmonic with the fret that is touched, when the format says (`12.0` for the octave node).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Harmonic {
    /// The kind.
    pub kind: HarmonicKind,
    /// The touched fret for artificial and tapped harmonics.
    pub fret: Option<f32>,
}

/// The direction of a strum stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StrumDirection {
    /// A down stroke.
    Down,
    /// An up stroke.
    Up,
}

/// One point of a bend curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BendPoint {
    /// The position along the note, `0.0` (the attack) to `1.0` (the end).
    pub pos: f32,
    /// The pitch offset in cents (100 = a semitone), positive is up.
    pub cents: i32,
}

/// The shape of a bend curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BendKind {
    /// Bend up and hold.
    Bend,
    /// Bend up, then release back.
    BendRelease,
    /// Start bent, hold.
    PreBend,
    /// Start bent, release.
    PreBendRelease,
    /// Anything else.
    Custom,
}

/// A pitch curve: a bend of the string or a whammy-bar movement.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Bend {
    /// The points, ascending in position.
    pub points: Vec<BendPoint>,
}

impl Bend {
    /// A bend up by `cents`, reached a third of the way into the note and held.
    pub fn up(cents: i32) -> Self {
        Self::from_pairs(&[(0.0, 0), (0.3, cents), (1.0, cents)])
    }

    /// A bend up by `cents`, then released back to pitch.
    pub fn bend_release(cents: i32) -> Self {
        Self::from_pairs(&[(0.0, 0), (0.25, cents), (0.5, cents), (0.8, 0), (1.0, 0)])
    }

    /// A note that starts already bent by `cents` and holds it.
    pub fn pre_bend(cents: i32) -> Self {
        Self::from_pairs(&[(0.0, cents), (1.0, cents)])
    }

    /// A note that starts bent by `cents` and is released to pitch.
    pub fn pre_bend_release(cents: i32) -> Self {
        Self::from_pairs(&[(0.0, cents), (0.4, cents), (0.8, 0), (1.0, 0)])
    }

    /// A curve from `(position, cents)` pairs.
    pub fn from_pairs(p: &[(f32, i32)]) -> Self {
        Self {
            points: p
                .iter()
                .map(|(pos, cents)| BendPoint {
                    pos: *pos,
                    cents: *cents,
                })
                .collect(),
        }
    }

    /// The highest offset of the curve in cents (0 for an empty curve).
    pub fn peak(&self) -> i32 {
        self.points.iter().map(|p| p.cents).max().unwrap_or(0)
    }

    /// Classifies the curve.
    pub fn kind(&self) -> BendKind {
        let p = &self.points;
        let (Some(first), Some(last)) = (p.first(), p.last()) else {
            return BendKind::Custom;
        };
        let peak = self.peak();
        if peak <= 0 {
            return BendKind::Custom;
        }
        match (first.cents == 0, last.cents == 0) {
            (true, false) => BendKind::Bend,
            (true, true) => BendKind::BendRelease,
            (false, false) => BendKind::PreBend,
            (false, true) => BendKind::PreBendRelease,
        }
    }

    /// Puts the curve in order and within bounds: at most
    /// [`limits::MAX_CURVE_POINTS`] points, positions in `0..=1`, offsets within four octaves.
    pub fn sanitize(&mut self) {
        self.points.truncate(limits::MAX_CURVE_POINTS);
        for p in &mut self.points {
            p.pos = if p.pos.is_finite() {
                p.pos.clamp(0.0, 1.0)
            } else {
                0.0
            };
            p.cents = p.cents.clamp(-4800, 4800);
        }
        self.points.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    }

    /// The offset in cents at `pos` (linear between points, held outside them).
    pub fn at(&self, pos: f32) -> f32 {
        let p = &self.points;
        let Some(first) = p.first() else {
            return 0.0;
        };
        if pos <= first.pos {
            return first.cents as f32;
        }
        for w in p.windows(2) {
            if pos <= w[1].pos {
                let span = (w[1].pos - w[0].pos).max(f32::EPSILON);
                let t = (pos - w[0].pos) / span;
                return w[0].cents as f32 + t * (w[1].cents - w[0].cents) as f32;
            }
        }
        p[p.len() - 1].cents as f32
    }
}

/// A grace note before or on the beat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grace {
    /// The fret of the grace note (same string as the main note).
    pub fret: u8,
    /// Its length in ticks.
    pub ticks: i64,
    /// Whether it falls on the beat (stealing time from the main note) instead of before it.
    pub on_beat: bool,
}

/// Everything about how a note is played, beyond its string and fret.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Techniques {
    /// The on/off techniques.
    pub flags: TechFlags,
    /// A slide.
    pub slide: Option<SlideKind>,
    /// A vibrato.
    pub vibrato: Option<VibratoKind>,
    /// A harmonic.
    pub harmonic: Option<Harmonic>,
    /// A string bend.
    pub bend: Option<Bend>,
    /// A whammy-bar dive or lift.
    pub whammy: Option<Bend>,
    /// Tremolo picking: the note value of each stroke (8, 16, 32) or 0 for none.
    pub tremolo_division: u8,
    /// A grace note.
    pub grace: Option<Grace>,
    /// The stroke of a strummed chord this note belongs to.
    pub strum: Option<StrumDirection>,
}

impl Techniques {
    /// Whether the note is plain.
    pub fn is_empty(&self) -> bool {
        *self == Techniques::default()
    }

    /// Whether `flag` is set.
    pub fn has(&self, flag: TechFlags) -> bool {
        self.flags.contains(flag)
    }

    /// The same techniques with `flag` set.
    pub fn with(mut self, flag: TechFlags) -> Self {
        self.flags.insert(flag);
        self
    }

    /// Bounds every curve and parameter.
    pub fn sanitize(&mut self) {
        self.flags.0 &= TechFlags::ALL.0;
        if let Some(b) = &mut self.bend {
            b.sanitize();
        }
        if let Some(b) = &mut self.whammy {
            b.sanitize();
        }
        if !matches!(self.tremolo_division, 0 | 4 | 8 | 16 | 32 | 64) {
            self.tremolo_division = 0;
        }
        if let Some(g) = &mut self.grace {
            g.ticks = g.ticks.clamp(1, PPQ * 4);
        }
    }
}

/// One note of a tab track.
#[derive(Debug, Clone, PartialEq)]
pub struct TabNote {
    /// The start, in ticks.
    pub tick: i64,
    /// The length, in ticks.
    pub len: i64,
    /// The string, in tab order.
    pub string: u8,
    /// The fret, relative to the capo (0 = open).
    pub fret: u8,
    /// The velocity, 1 to 127.
    pub velocity: u8,
    /// The playing techniques.
    pub tech: Techniques,
    /// The note continues the previous one on its string instead of being struck.
    pub tie: bool,
}

impl TabNote {
    /// A plain note.
    pub fn new(tick: i64, len: i64, string: u8, fret: u8) -> Self {
        Self {
            tick,
            len,
            string,
            fret,
            velocity: 90,
            tech: Techniques::default(),
            tie: false,
        }
    }

    /// The position as a [`Placement`].
    pub fn placement(&self) -> Placement {
        Placement {
            string: usize::from(self.string),
            fret: self.fret,
        }
    }
}

/// A chord diagram pinned to a point of a track.
#[derive(Debug, Clone, PartialEq)]
pub struct DiagramRef {
    /// Where the diagram applies, in ticks.
    pub tick: i64,
    /// The chord name shown above it.
    pub name: String,
    /// The fingering.
    pub voicing: Voicing,
}

/// One instrument's part.
#[derive(Debug, Clone, PartialEq)]
pub struct TabTrack {
    /// The track name.
    pub name: String,
    /// The catalogue id of the instrument (`guitar6`), or an empty text for a custom one.
    pub instrument_id: String,
    /// The strings.
    pub tuning: Tuning,
    /// The capo.
    pub capo: Capo,
    /// The General MIDI program, 0 to 127.
    pub midi_program: u8,
    /// A drum track (its notes are percussion keys, not frets).
    pub is_percussion: bool,
    /// The notes, sorted by tick once [`TabScore::sanitize`] has run.
    pub notes: Vec<TabNote>,
    /// Chord diagrams.
    pub diagrams: Vec<DiagramRef>,
    /// Lyric syllables.
    pub lyrics: Vec<(i64, String)>,
    /// Chord symbols above the staff.
    pub chords: Vec<(i64, ChordSymbol)>,
}

impl TabTrack {
    /// An empty track on a tuning.
    pub fn new(name: &str, instrument_id: &str, tuning: Tuning) -> Self {
        Self {
            name: name.to_owned(),
            instrument_id: instrument_id.to_owned(),
            tuning,
            capo: Capo::NONE,
            midi_program: 25,
            is_percussion: false,
            notes: Vec::new(),
            diagrams: Vec::new(),
            lyrics: Vec::new(),
            chords: Vec::new(),
        }
    }

    /// The sounding MIDI key of a note (`None` when its string or fret does not exist).
    pub fn key_of(&self, n: &TabNote) -> Option<u8> {
        self.tuning.pitch(usize::from(n.string), n.fret, self.capo)
    }

    /// The tick just after the last note ends.
    pub fn end_tick(&self) -> i64 {
        self.notes.iter().map(|n| n.tick + n.len).max().unwrap_or(0)
    }
}

/// A key signature as the formats store it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeySignature {
    /// Sharps (positive) or flats (negative), `-7..=7`.
    pub fifths: i8,
    /// Minor mode.
    pub minor: bool,
}

impl KeySignature {
    /// The key with its usual spelling.
    pub fn to_key(self) -> Key {
        // The tonic of a major key at `fifths` sharps: C G D A E B F# C#, F Bb Eb Ab Db Gb Cb.
        const MAJOR_NAMES: [&str; 15] = [
            "Cb", "Gb", "Db", "Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E", "B", "F#", "C#",
        ];
        const MINOR_NAMES: [&str; 15] = [
            "Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E", "B", "F#", "C#", "G#", "D#", "A#",
        ];
        let i = usize::try_from(i32::from(self.fifths.clamp(-7, 7)) + 7).unwrap_or(7);
        let name = if self.minor {
            MINOR_NAMES[i]
        } else {
            MAJOR_NAMES[i]
        };
        let tonic = Key::parse(name).map_or(crate::chord::Note::new(0, 0), |k| k.tonic);
        Key::new(tonic, self.minor)
    }

    /// The key signature of a key (the nearest one when the key needs double accidentals).
    pub fn from_key(key: Key) -> Self {
        let f = key.fifths().clamp(-7, 7);
        Self {
            fifths: f as i8,
            minor: key.minor,
        }
    }
}

/// A bar of the original score (kept so an exporter can restore repeats).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BarInfo {
    /// The first tick of the bar in the expanded timeline.
    pub tick: i64,
    /// The length in ticks.
    pub len: i64,
    /// A repeat starts here.
    pub repeat_open: bool,
    /// A repeat ends here: the number of times the section is played.
    pub repeat_close: Option<u8>,
    /// Alternative endings (volta) this bar belongs to, as a bit mask of passes (bit 0 = first).
    pub volta: Option<u8>,
}

/// A whole tab document.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TabScore {
    /// The title.
    pub title: String,
    /// The subtitle.
    pub subtitle: String,
    /// The artist.
    pub artist: String,
    /// The album.
    pub album: String,
    /// The lyricist.
    pub words_by: String,
    /// The composer.
    pub music_by: String,
    /// The transcriber.
    pub tab_by: String,
    /// The copyright line.
    pub copyright: String,
    /// Free notes.
    pub notices: String,
    /// Tempo changes `(tick, bpm)`.
    pub tempo: Vec<(i64, f64)>,
    /// Meter changes `(first bar, numerator, denominator)`, bars counted from 0.
    pub meter: Vec<(u32, u8, u8)>,
    /// Key changes.
    pub key: Vec<(i64, KeySignature)>,
    /// Section markers `(tick, name)`.
    pub markers: Vec<(i64, String)>,
    /// The original bar structure, when the format has one.
    pub bars: Vec<BarInfo>,
    /// The tracks.
    pub tracks: Vec<TabTrack>,
    /// A chord sheet that came with the score.
    pub chord_sheet: Option<crate::sheet::ChordSheet>,
}

/// The length of a bar of `num/den` in ticks.
pub fn bar_ticks(num: u8, den: u8) -> i64 {
    let den = i64::from(den.max(1));
    4 * PPQ * i64::from(num.max(1)) / den
}

impl TabScore {
    /// An empty score.
    pub fn new() -> Self {
        Self::default()
    }

    /// The `(start tick, length)` of each bar up to the end of the music (at least one bar),
    /// following the meter changes (4/4 when there are none).
    pub fn bar_grid(&self) -> Vec<(i64, i64)> {
        let end = self
            .tracks
            .iter()
            .map(TabTrack::end_tick)
            .max()
            .unwrap_or(0);
        let mut meters = self.meter.clone();
        meters.sort_by_key(|m| m.0);
        let mut out = Vec::new();
        let mut tick = 0i64;
        let (mut num, mut den) = (4u8, 4u8);
        let mut mi = 0usize;
        let mut bar = 0u32;
        while (tick < end || out.is_empty()) && out.len() < limits::MAX_BARS {
            while mi < meters.len() && meters[mi].0 <= bar {
                num = meters[mi].1.max(1);
                den = meters[mi].2.max(1);
                mi += 1;
            }
            let len = bar_ticks(num, den);
            out.push((tick, len));
            tick += len;
            bar += 1;
        }
        out
    }

    /// The total number of notes.
    pub fn note_count(&self) -> usize {
        self.tracks.iter().map(|t| t.notes.len()).sum()
    }

    /// Puts the score in the shape the rest of the program relies on and reports what it had to
    /// change: caps the counts, sorts notes and maps, drops notes on strings that do not exist,
    /// bounds velocities, lengths and curves. Readers call it last.
    pub fn sanitize(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        if self.tracks.len() > limits::MAX_TRACKS {
            self.tracks.truncate(limits::MAX_TRACKS);
            warnings.push(format!(
                "only the first {} tracks were kept",
                limits::MAX_TRACKS
            ));
        }
        for text in [
            &mut self.title,
            &mut self.subtitle,
            &mut self.artist,
            &mut self.album,
            &mut self.words_by,
            &mut self.music_by,
            &mut self.tab_by,
            &mut self.copyright,
            &mut self.notices,
        ] {
            clip_text(text);
        }
        self.tempo.retain(|(_, b)| b.is_finite() && *b > 0.0);
        for (_, b) in &mut self.tempo {
            *b = b.clamp(10.0, 1000.0);
        }
        self.tempo.sort_by_key(|t| t.0);
        self.tempo.truncate(limits::MAX_CHANGES);
        self.meter
            .retain(|(_, n, d)| *n > 0 && *d > 0 && d.is_power_of_two());
        self.meter.sort_by_key(|m| m.0);
        self.meter.truncate(limits::MAX_CHANGES);
        self.key.sort_by_key(|k| k.0);
        self.key.truncate(limits::MAX_CHANGES);
        self.markers.sort_by_key(|m| m.0);
        self.markers.truncate(limits::MAX_CHANGES);
        for (_, m) in &mut self.markers {
            clip_text(m);
        }
        self.bars.truncate(limits::MAX_BARS);
        let mut budget = limits::MAX_NOTES;
        for (ti, t) in self.tracks.iter_mut().enumerate() {
            clip_text(&mut t.name);
            t.midi_program = t.midi_program.min(127);
            let strings = t.tuning.len();
            let before = t.notes.len();
            if !t.is_percussion {
                t.notes.retain(|n| usize::from(n.string) < strings);
            }
            let dropped = before - t.notes.len();
            if dropped > 0 {
                warnings.push(format!(
                    "track {}: {dropped} notes on strings the tuning does not have were dropped",
                    ti + 1
                ));
            }
            let cap = limits::MAX_NOTES_PER_TRACK.min(budget);
            if t.notes.len() > cap {
                t.notes.truncate(cap);
                warnings.push(format!(
                    "track {}: too many notes, the rest were dropped",
                    ti + 1
                ));
            }
            budget -= t.notes.len();
            t.notes.retain(|n| n.tick >= 0);
            for n in &mut t.notes {
                n.len = n.len.clamp(1, PPQ * 64);
                n.velocity = n.velocity.clamp(1, 127);
                n.tech.sanitize();
            }
            t.notes.sort_by_key(|n| (n.tick, n.string));
            t.lyrics.truncate(limits::MAX_CHANGES * 4);
            for (_, s) in &mut t.lyrics {
                clip_text(s);
            }
            t.lyrics.sort_by_key(|l| l.0);
            t.chords.truncate(limits::MAX_CHANGES);
            t.chords.sort_by_key(|c| c.0);
            t.diagrams.truncate(limits::MAX_CHANGES);
            t.diagrams.sort_by_key(|d| d.tick);
        }
        warnings
    }

    /// Checks the score without changing it: the problems [`sanitize`](Self::sanitize) would fix
    /// and frets beyond the neck.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (ti, t) in self.tracks.iter().enumerate() {
            if t.tuning.validate().is_err() {
                out.push(format!("track {}: invalid tuning", ti + 1));
            }
            if t.is_percussion {
                continue;
            }
            let bad = t
                .notes
                .iter()
                .filter(|n| usize::from(n.string) >= t.tuning.len() || t.key_of(n).is_none())
                .count();
            if bad > 0 {
                out.push(format!("track {}: {bad} notes outside the neck", ti + 1));
            }
        }
        out
    }
}

fn clip_text(s: &mut String) {
    if s.chars().count() > limits::MAX_TEXT {
        *s = s.chars().take(limits::MAX_TEXT).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instrument::find_tuning;

    fn guitar() -> Tuning {
        find_tuning("guitar6", "standard").unwrap().tuning.clone()
    }

    #[test]
    fn flags_work_like_a_set() {
        let mut f = TechFlags::NONE;
        assert!(f.is_empty());
        f.insert(TechFlags::PALM_MUTE | TechFlags::ACCENT);
        assert!(f.contains(TechFlags::PALM_MUTE));
        assert!(!f.contains(TechFlags::DEAD));
        f.remove(TechFlags::PALM_MUTE);
        assert!(!f.contains(TechFlags::PALM_MUTE));
        assert!(f.contains(TechFlags::ACCENT));
        let t = Techniques::default().with(TechFlags::GHOST);
        assert!(t.has(TechFlags::GHOST));
        assert!(!t.is_empty());
        assert!(Techniques::default().is_empty());
    }

    #[test]
    fn bend_shapes_are_classified() {
        assert_eq!(Bend::up(200).kind(), BendKind::Bend);
        assert_eq!(Bend::bend_release(100).kind(), BendKind::BendRelease);
        assert_eq!(Bend::pre_bend(100).kind(), BendKind::PreBend);
        assert_eq!(Bend::pre_bend_release(100).kind(), BendKind::PreBendRelease);
        assert_eq!(Bend::default().kind(), BendKind::Custom);
        assert_eq!(Bend::up(200).peak(), 200);
        let b = Bend::up(200);
        assert!((b.at(0.0)).abs() < 1e-3);
        assert!((b.at(0.3) - 200.0).abs() < 1e-3);
        assert!((b.at(0.15) - 100.0).abs() < 1e-3);
        assert!((b.at(1.0) - 200.0).abs() < 1e-3);
    }

    #[test]
    fn bends_are_bounded() {
        let mut b = Bend::from_pairs(&[(2.0, 99_999), (f32::NAN, -99_999), (-1.0, 5)]);
        b.points.extend((0..200).map(|i| BendPoint {
            pos: i as f32,
            cents: 1,
        }));
        b.sanitize();
        assert!(b.points.len() <= limits::MAX_CURVE_POINTS);
        assert!(b.points.iter().all(|p| (0.0..=1.0).contains(&p.pos)));
        assert!(b.points.iter().all(|p| p.cents.abs() <= 4800));
        assert!(b.points.windows(2).all(|w| w[0].pos <= w[1].pos));
    }

    #[test]
    fn key_signatures_round_trip() {
        for fifths in -7..=7i8 {
            for minor in [false, true] {
                let ks = KeySignature { fifths, minor };
                let key = ks.to_key();
                assert_eq!(
                    KeySignature::from_key(key),
                    ks,
                    "{fifths} {minor} {}",
                    key.name()
                );
            }
        }
        assert_eq!(
            KeySignature {
                fifths: 2,
                minor: false
            }
            .to_key()
            .name(),
            "D"
        );
        assert_eq!(
            KeySignature {
                fifths: -3,
                minor: true
            }
            .to_key()
            .name(),
            "Cm"
        );
    }

    #[test]
    fn bar_grid_follows_the_meter() {
        let mut s = TabScore::new();
        let mut t = TabTrack::new("g", "guitar6", guitar());
        t.notes.push(TabNote::new(0, PPQ, 0, 0));
        t.notes.push(TabNote::new(7 * PPQ, PPQ, 0, 0));
        s.tracks.push(t);
        s.meter = vec![(0, 4, 4), (1, 3, 4)];
        let g = s.bar_grid();
        assert_eq!(g[0], (0, 4 * PPQ));
        assert_eq!(g[1], (4 * PPQ, 3 * PPQ));
        assert_eq!(
            g.last().map(|b| b.0 + b.1).map(|e| e >= 8 * PPQ),
            Some(true)
        );
        assert_eq!(TabScore::new().bar_grid().len(), 1);
        assert_eq!(bar_ticks(6, 8), 3 * PPQ);
    }

    #[test]
    fn sanitize_sorts_clamps_and_reports() {
        let mut s = TabScore::new();
        let mut t = TabTrack::new("g", "guitar6", guitar());
        t.notes.push(TabNote::new(PPQ, 10, 2, 5));
        t.notes.push(TabNote::new(0, 0, 9, 3));
        t.notes.push(TabNote::new(-5, 10, 0, 0));
        let mut loud = TabNote::new(0, 10, 0, 0);
        loud.velocity = 200;
        t.notes.push(loud);
        s.tracks.push(t);
        s.tempo = vec![(100, 120.0), (0, f64::NAN), (50, -3.0), (0, 90.0)];
        let w = s.sanitize();
        assert_eq!(w.len(), 1, "{w:?}");
        let n = &s.tracks[0].notes;
        assert_eq!(n.len(), 2);
        assert!(n.windows(2).all(|p| p[0].tick <= p[1].tick));
        assert!(n.iter().all(|x| x.velocity <= 127 && x.len >= 1));
        assert_eq!(s.tempo.len(), 2);
        assert!(s.problems().is_empty());
    }

    #[test]
    fn note_keys_follow_tuning_and_capo() {
        let mut t = TabTrack::new("g", "guitar6", guitar());
        let n = TabNote::new(0, PPQ, 0, 3);
        assert_eq!(t.key_of(&n), Some(67));
        t.capo = Capo::at(2);
        assert_eq!(t.key_of(&n), Some(69));
        assert_eq!(t.key_of(&TabNote::new(0, 1, 9, 0)), None);
    }

    #[test]
    fn caps_track_and_text_sizes() {
        let mut s = TabScore::new();
        s.title = "x".repeat(limits::MAX_TEXT + 50);
        for _ in 0..(limits::MAX_TRACKS + 3) {
            s.tracks.push(TabTrack::new("t", "guitar6", guitar()));
        }
        let w = s.sanitize();
        assert_eq!(s.tracks.len(), limits::MAX_TRACKS);
        assert_eq!(s.title.chars().count(), limits::MAX_TEXT);
        assert!(!w.is_empty());
    }
}
