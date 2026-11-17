//! Tablature data a note clip carries: the instrument and its tuning, where each note is played
//! on the neck and how, chord diagrams and lyrics.
//!
//! The notes of a tab clip live in the clip's note list, which stays the only source of what is
//! heard. [`TabData`] annotates them: one [`TabNoteRef`] per note that has a position or
//! techniques, found by the note's start tick and key like the per-note expression is (a clip
//! never holds two notes at one start and key). A note without an entry is fretted
//! automatically when it is shown. The techniques of a note are turned into sound by
//! [`crate::tab_render`], which writes the note's expression curves, velocity and length, so a
//! reader that ignores the sidecar still plays the techniques through pitch bends.
//!
//! The data rides in the clip's [`crate::MidiData`], so a split, a join, a loop conversion and a
//! clip copy carry it with the notes, and it is saved only while a clip has some (an older reader
//! drops it and plays the notes without it). Like the expression, an entry follows its note by
//! start tick and key: a note moved or retuned in the piano roll loses its entry (the saved file
//! leaves out entries that no note matches) and is fretted automatically again.

use atomik_tab::instrument::{MAX_CAPO, MAX_FRETS};
use atomik_tab::score::{
    Bend, HarmonicKind, SlideKind, StrumDirection, TechFlags, Techniques, VibratoKind,
    limits as score_limits,
};
use atomik_tab::voicing::Voicing;
use atomik_tab::{Capo, Tuning};

/// The most notes with a tab entry one clip keeps.
pub const MAX_TAB_NOTES: usize = 65_536;
/// The most chord diagrams one clip keeps.
pub const MAX_TAB_DIAGRAMS: usize = 1024;
/// The most lyric syllables one clip keeps.
pub const MAX_TAB_LYRICS: usize = 16_384;
/// The longest lyric syllable or diagram name, in characters.
pub const MAX_TAB_TEXT: usize = 64;
/// The longest instrument id, in characters.
pub const MAX_INSTRUMENT_ID: usize = 64;
/// The longest string label of a tuning, in characters.
pub const MAX_STRING_LABEL: usize = 8;

/// The velocity and length a note had before its techniques changed them (see
/// [`crate::tab_render`]): kept so that rendering again, or clearing the techniques, starts from
/// the plain note and the effect never compounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlainNote {
    /// The velocity in `(0, 1]` before ghost and accent were applied.
    pub velocity: f32,
    /// The length in ticks before staccato, let ring, dead and slides were applied.
    pub length: i64,
}

/// Where one note of the clip is played and how.
#[derive(Debug, Clone, PartialEq)]
pub struct TabNoteRef {
    /// Start tick of the note in the clip's content ticks.
    pub tick: i64,
    /// Key of the note, `0..=127`: the pitch the fretted position sounds.
    pub key: u8,
    /// The string, in tab order (0 = top line).
    pub string: u8,
    /// The fret, relative to the capo (0 = open).
    pub fret: u8,
    /// How the note is played.
    pub tech: Techniques,
    /// The plain velocity and length, set while the techniques changed them.
    pub plain: Option<PlainNote>,
}

impl TabNoteRef {
    /// A plain note at a position.
    pub fn new(tick: i64, key: u8, string: u8, fret: u8) -> Self {
        Self {
            tick,
            key,
            string,
            fret,
            tech: Techniques::default(),
            plain: None,
        }
    }
}

/// A chord diagram pinned to a tick of the clip.
#[derive(Debug, Clone, PartialEq)]
pub struct TabDiagram {
    /// Content tick the diagram applies from.
    pub tick: i64,
    /// The chord name shown with it.
    pub name: String,
    /// The fingering, one fret per string of the clip's tuning.
    pub voicing: Voicing,
}

/// One lyric syllable at a tick of the clip.
#[derive(Debug, Clone, PartialEq)]
pub struct TabLyric {
    /// Content tick.
    pub tick: i64,
    /// The syllable.
    pub text: String,
}

/// The tab sidecar of a note clip (module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct TabData {
    /// The instrument's id (`guitar6`, ...). Any text up to [`MAX_INSTRUMENT_ID`] characters: an
    /// id this build's catalogue does not know still keeps its tuning.
    pub instrument_id: String,
    /// The strings (custom tunings are allowed).
    pub tuning: Tuning,
    /// The capo.
    pub capo: Capo,
    /// The notes that have an entry, sorted by `(tick, key)`.
    pub notes: Vec<TabNoteRef>,
    /// Chord diagrams, sorted by tick.
    pub diagrams: Vec<TabDiagram>,
    /// Lyric syllables, sorted by tick (empty: the clip has none).
    pub lyrics: Vec<TabLyric>,
}

impl TabData {
    /// Tab data for a catalogue instrument in its default tuning, without notes; `None` for an id
    /// the catalogue lacks.
    pub fn for_instrument(instrument_id: &str) -> Option<TabData> {
        let instrument = atomik_tab::instrument::find_instrument(instrument_id)?;
        Some(TabData::with_tuning(
            instrument_id,
            instrument.default_tuning().tuning.clone(),
            Capo::NONE,
        ))
    }

    /// Tab data for `tuning` and `capo`, without notes.
    pub fn with_tuning(instrument_id: &str, tuning: Tuning, capo: Capo) -> TabData {
        TabData {
            instrument_id: instrument_id.to_owned(),
            tuning,
            capo,
            notes: Vec::new(),
            diagrams: Vec::new(),
            lyrics: Vec::new(),
        }
    }

    /// The entry of the note at `tick` on `key`, if it has one.
    pub fn note_of(&self, tick: i64, key: u8) -> Option<&TabNoteRef> {
        self.notes
            .binary_search_by_key(&(tick, key), |n| (n.tick, n.key))
            .ok()
            .map(|i| &self.notes[i])
    }

    /// The entry of the note at `tick` on `string`, if it has one (a string plays one note at a
    /// time, so there is at most one).
    pub fn note_on_string(&self, tick: i64, string: u8) -> Option<&TabNoteRef> {
        self.notes
            .iter()
            .find(|n| n.tick == tick && n.string == string)
    }

    /// The data with `entry` set for its note (replacing the one it had). Refused (`None`)
    /// beyond [`MAX_TAB_NOTES`].
    pub fn with_note(&self, entry: TabNoteRef) -> Option<TabData> {
        let mut out = self.clone();
        match out
            .notes
            .binary_search_by_key(&(entry.tick, entry.key), |n| (n.tick, n.key))
        {
            Ok(i) => out.notes[i] = entry,
            Err(_) if out.notes.len() >= MAX_TAB_NOTES => return None,
            Err(i) => out.notes.insert(i, entry),
        }
        Some(out.canonical())
    }

    /// The data without the entry of the note at `tick` on `key`; `None` when it had none.
    pub fn without_note(&self, tick: i64, key: u8) -> Option<TabData> {
        let i = self
            .notes
            .binary_search_by_key(&(tick, key), |n| (n.tick, n.key))
            .ok()?;
        let mut out = self.clone();
        out.notes.remove(i);
        Some(out)
    }

    /// The data in canonical form: entries sorted by `(tick, key)` (a later entry of one note
    /// wins), techniques bounded, ticks inside `0..=MAX_TICK` and every list cut at its limit;
    /// diagrams and lyrics sorted by tick (equal ticks keep their order).
    pub fn canonical(mut self) -> Self {
        for n in &mut self.notes {
            n.tick = n.tick.clamp(0, crate::song::MAX_TICK);
            fit_techniques(&mut n.tech);
            if let Some(p) = &mut n.plain {
                p.velocity = if p.velocity.is_finite() {
                    p.velocity.clamp(f32::MIN_POSITIVE, 1.0)
                } else {
                    1.0
                };
                p.length = p.length.clamp(1, crate::song::MAX_TICK);
            }
        }
        self.notes.sort_by_key(|n| (n.tick, n.key));
        let mut notes: Vec<TabNoteRef> = Vec::with_capacity(self.notes.len());
        for n in self.notes {
            match notes.last_mut() {
                Some(last) if (last.tick, last.key) == (n.tick, n.key) => *last = n,
                _ => notes.push(n),
            }
        }
        notes.truncate(MAX_TAB_NOTES);
        self.notes = notes;
        for d in &mut self.diagrams {
            d.tick = d.tick.clamp(0, crate::song::MAX_TICK);
        }
        self.diagrams.sort_by_key(|d| d.tick);
        self.diagrams.truncate(MAX_TAB_DIAGRAMS);
        for l in &mut self.lyrics {
            l.tick = l.tick.clamp(0, crate::song::MAX_TICK);
        }
        self.lyrics.sort_by_key(|l| l.tick);
        self.lyrics.truncate(MAX_TAB_LYRICS);
        self
    }

    /// Checks the data: counts and sizes within the limits, ticks in `0..=max_tick`, a valid
    /// tuning, every position on a string of the tuning, techniques inside their bounds, entries
    /// sorted by `(tick, key)` without duplicates. `Err` names the first fault.
    pub fn check(&self, max_tick: i64) -> Result<(), String> {
        if self.instrument_id.chars().count() > MAX_INSTRUMENT_ID
            || self.instrument_id.chars().any(char::is_control)
        {
            return Err(format!(
                "tab: the instrument id is at most {MAX_INSTRUMENT_ID} characters without \
                 control characters"
            ));
        }
        self.tuning
            .validate()
            .map_err(|e| format!("tab tuning: {e}"))?;
        if self.tuning.strings.iter().any(|s| {
            s.label.chars().count() > MAX_STRING_LABEL || s.label.chars().any(char::is_control)
        }) {
            return Err(format!(
                "tab tuning: a string label is at most {MAX_STRING_LABEL} characters"
            ));
        }
        if self.capo.fret > MAX_CAPO {
            return Err(format!("tab: the capo is at most fret {MAX_CAPO}"));
        }
        if self.notes.len() > MAX_TAB_NOTES {
            return Err(format!(
                "a clip keeps tab data for at most {MAX_TAB_NOTES} notes"
            ));
        }
        let strings = self.tuning.len();
        for (i, n) in self.notes.iter().enumerate() {
            if !(0..=max_tick).contains(&n.tick) {
                return Err(format!(
                    "tab note {i}: tick {} is outside 0..={max_tick}",
                    n.tick
                ));
            }
            if n.key > 127 {
                return Err(format!("tab note {i}: key beyond 127"));
            }
            if usize::from(n.string) >= strings {
                return Err(format!(
                    "tab note {i}: string {} but the tuning has {strings}",
                    n.string
                ));
            }
            if n.fret > self.tuning.frets {
                return Err(format!(
                    "tab note {i}: fret {} beyond the {} frets of the neck",
                    n.fret, self.tuning.frets
                ));
            }
            techniques_ok(&n.tech).map_err(|m| format!("tab note {i}: {m}"))?;
            if let Some(p) = n.plain
                && (!(p.velocity > 0.0 && p.velocity <= 1.0)
                    || !(1..=crate::song::MAX_TICK).contains(&p.length))
            {
                return Err(format!(
                    "tab note {i}: the plain velocity or length is out of range"
                ));
            }
            if i > 0 {
                let before = &self.notes[i - 1];
                if (before.tick, before.key) >= (n.tick, n.key) {
                    return Err(format!(
                        "tab notes must be sorted by note without duplicates (entry {i})"
                    ));
                }
            }
        }
        if self.diagrams.len() > MAX_TAB_DIAGRAMS {
            return Err(format!(
                "a clip keeps at most {MAX_TAB_DIAGRAMS} chord diagrams"
            ));
        }
        for (i, d) in self.diagrams.iter().enumerate() {
            if !(0..=max_tick).contains(&d.tick) {
                return Err(format!(
                    "tab diagram {i}: tick {} is outside 0..={max_tick}",
                    d.tick
                ));
            }
            if text_bad(&d.name) {
                return Err(format!(
                    "tab diagram {i}: the name is at most {MAX_TAB_TEXT} characters"
                ));
            }
            voicing_ok(&d.voicing, strings).map_err(|m| format!("tab diagram {i}: {m}"))?;
            if i > 0 && self.diagrams[i - 1].tick > d.tick {
                return Err(format!("tab diagrams must be sorted by tick (diagram {i})"));
            }
        }
        if self.lyrics.len() > MAX_TAB_LYRICS {
            return Err(format!(
                "a clip keeps at most {MAX_TAB_LYRICS} lyric syllables"
            ));
        }
        for (i, l) in self.lyrics.iter().enumerate() {
            if !(0..=max_tick).contains(&l.tick) {
                return Err(format!(
                    "tab lyric {i}: tick {} is outside 0..={max_tick}",
                    l.tick
                ));
            }
            if text_bad(&l.text) {
                return Err(format!(
                    "tab lyric {i}: a syllable is at most {MAX_TAB_TEXT} characters"
                ));
            }
            if i > 0 && self.lyrics[i - 1].tick > l.tick {
                return Err(format!("tab lyrics must be sorted by tick (lyric {i})"));
            }
        }
        Ok(())
    }

    /// The part inside the content window `[from, to)`, relative to `from` and moved by `shift`.
    pub fn window(&self, from: i64, to: i64, shift: i64) -> TabData {
        let inside = |t: i64| (from..to).contains(&t);
        let mut out = TabData {
            instrument_id: self.instrument_id.clone(),
            tuning: self.tuning.clone(),
            capo: self.capo,
            notes: self
                .notes
                .iter()
                .filter(|n| inside(n.tick))
                .cloned()
                .collect(),
            diagrams: self
                .diagrams
                .iter()
                .filter(|d| inside(d.tick))
                .cloned()
                .collect(),
            lyrics: self
                .lyrics
                .iter()
                .filter(|l| inside(l.tick))
                .cloned()
                .collect(),
        };
        out.shift_by(shift - from);
        out
    }

    /// Every tick mapped through `f` (the order stays, so a map that is not monotonic is the
    /// caller's problem; `canonical` repairs ties).
    pub fn map_ticks(&self, f: impl Fn(i64) -> i64) -> TabData {
        let mut out = self.clone();
        for n in &mut out.notes {
            n.tick = f(n.tick);
        }
        for d in &mut out.diagrams {
            d.tick = f(d.tick);
        }
        for l in &mut out.lyrics {
            l.tick = f(l.tick);
        }
        out.canonical()
    }

    /// Appends `other` (already in this clip's ticks). The instrument, tuning and capo of `self`
    /// stay: the positions of `other` are kept only when it is played the same way, otherwise its
    /// notes lose their entries (they are fretted again when shown) and its diagrams are dropped.
    pub fn append(&mut self, other: TabData) {
        let mut merged = self.clone();
        if merged.tuning == other.tuning && merged.capo == other.capo {
            merged.notes.extend(other.notes);
            merged.diagrams.extend(other.diagrams);
        }
        merged.lyrics.extend(other.lyrics);
        *self = merged.canonical();
    }

    fn shift_by(&mut self, delta: i64) {
        for n in &mut self.notes {
            n.tick += delta;
        }
        for d in &mut self.diagrams {
            d.tick += delta;
        }
        for l in &mut self.lyrics {
            l.tick += delta;
        }
    }
}

fn text_bad(text: &str) -> bool {
    text.chars().count() > MAX_TAB_TEXT || text.chars().any(char::is_control)
}

fn voicing_ok(v: &Voicing, strings: usize) -> Result<(), String> {
    if v.frets.len() != strings || v.fingers.len() != strings {
        return Err(format!("the voicing needs {strings} strings"));
    }
    if v.frets.iter().flatten().any(|f| *f > MAX_FRETS) || v.base_fret > MAX_FRETS {
        return Err("a fret of the voicing is beyond the neck".to_owned());
    }
    if v.fingers.iter().flatten().any(|f| *f > 4) {
        return Err("a finger is 1 to 4".to_owned());
    }
    if v.barres.len() > 1 {
        return Err("a voicing has at most one barre".to_owned());
    }
    for b in &v.barres {
        if b.from > b.to || b.to >= strings || b.fret > MAX_FRETS {
            return Err("the barre is outside the strings".to_owned());
        }
    }
    Ok(())
}

/// Puts a note's techniques inside their bounds: curves, divisions, the harmonic and grace frets.
fn fit_techniques(t: &mut Techniques) {
    t.sanitize();
    if let Some(h) = &mut t.harmonic {
        h.fret = h
            .fret
            .filter(|f| f.is_finite())
            .map(|f| f.clamp(0.0, f32::from(MAX_FRETS)));
    }
    if let Some(g) = &mut t.grace {
        g.fret = g.fret.min(MAX_FRETS);
    }
}

/// Checks that techniques are already inside their bounds (see [`fit_techniques`]).
fn techniques_ok(t: &Techniques) -> Result<(), String> {
    let mut fitted = t.clone();
    fit_techniques(&mut fitted);
    if &fitted != t {
        return Err("a technique is outside its bounds".to_owned());
    }
    let curve_ok = |b: &Bend| {
        b.points.len() <= score_limits::MAX_CURVE_POINTS
            && b.points.windows(2).all(|w| w[0].pos <= w[1].pos)
    };
    for b in t.bend.iter().chain(t.whammy.iter()) {
        if !curve_ok(b) {
            return Err("a bend curve is not sorted or has too many points".to_owned());
        }
    }
    Ok(())
}

/// The stable names of the technique values the project file stores.
pub mod ids {
    use super::*;

    /// Every flag and its file name, in saved order.
    pub const FLAGS: [(TechFlags, &str); 14] = [
        (TechFlags::HAMMER_ON, "hammer_on"),
        (TechFlags::PULL_OFF, "pull_off"),
        (TechFlags::PALM_MUTE, "palm_mute"),
        (TechFlags::LET_RING, "let_ring"),
        (TechFlags::DEAD, "dead"),
        (TechFlags::GHOST, "ghost"),
        (TechFlags::ACCENT, "accent"),
        (TechFlags::HEAVY_ACCENT, "heavy_accent"),
        (TechFlags::TAP, "tap"),
        (TechFlags::SLAP, "slap"),
        (TechFlags::POP, "pop"),
        (TechFlags::STACCATO, "staccato"),
        (TechFlags::FADE_IN, "fade_in"),
        (TechFlags::THUMB, "thumb"),
    ];

    const SLIDES: [SlideKind; 6] = [
        SlideKind::Legato,
        SlideKind::Shift,
        SlideKind::InFromBelow,
        SlideKind::InFromAbove,
        SlideKind::OutDown,
        SlideKind::OutUp,
    ];

    /// The file name of a slide.
    pub fn slide_id(s: SlideKind) -> &'static str {
        match s {
            SlideKind::Legato => "legato",
            SlideKind::Shift => "shift",
            SlideKind::InFromBelow => "in_below",
            SlideKind::InFromAbove => "in_above",
            SlideKind::OutDown => "out_down",
            SlideKind::OutUp => "out_up",
        }
    }

    /// The slide a file name stands for.
    pub fn slide_from_id(id: &str) -> Option<SlideKind> {
        SLIDES.into_iter().find(|s| slide_id(*s) == id)
    }

    /// The file name of a vibrato.
    pub fn vibrato_id(v: VibratoKind) -> &'static str {
        match v {
            VibratoKind::Normal => "normal",
            VibratoKind::Wide => "wide",
        }
    }

    /// The vibrato a file name stands for.
    pub fn vibrato_from_id(id: &str) -> Option<VibratoKind> {
        [VibratoKind::Normal, VibratoKind::Wide]
            .into_iter()
            .find(|v| vibrato_id(*v) == id)
    }

    /// The file name of a harmonic.
    pub fn harmonic_id(h: HarmonicKind) -> &'static str {
        match h {
            HarmonicKind::Natural => "natural",
            HarmonicKind::Artificial => "artificial",
            HarmonicKind::Pinch => "pinch",
            HarmonicKind::Tapped => "tapped",
            HarmonicKind::Semi => "semi",
        }
    }

    /// The harmonic a file name stands for.
    pub fn harmonic_from_id(id: &str) -> Option<HarmonicKind> {
        [
            HarmonicKind::Natural,
            HarmonicKind::Artificial,
            HarmonicKind::Pinch,
            HarmonicKind::Tapped,
            HarmonicKind::Semi,
        ]
        .into_iter()
        .find(|h| harmonic_id(*h) == id)
    }

    /// The file name of a strum direction.
    pub fn strum_id(s: StrumDirection) -> &'static str {
        match s {
            StrumDirection::Down => "down",
            StrumDirection::Up => "up",
        }
    }

    /// The strum direction a file name stands for.
    pub fn strum_from_id(id: &str) -> Option<StrumDirection> {
        [StrumDirection::Down, StrumDirection::Up]
            .into_iter()
            .find(|s| strum_id(*s) == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::PPQ;
    use atomik_tab::score::Harmonic;

    fn guitar() -> TabData {
        TabData::for_instrument("guitar6").expect("guitar6 is in the catalogue")
    }

    fn entry(tick: i64, key: u8) -> TabNoteRef {
        TabNoteRef::new(tick, key, 0, 0)
    }

    const MAX: i64 = crate::song::MAX_TICK;

    #[test]
    fn a_catalogue_instrument_gives_its_default_tuning() {
        let d = guitar();
        assert_eq!(d.tuning.len(), 6);
        assert!(d.notes.is_empty());
        assert!(d.check(MAX).is_ok());
        assert!(TabData::for_instrument("no_such_thing").is_none());
    }

    #[test]
    fn canonical_sorts_and_the_last_entry_of_a_note_wins() {
        let mut d = guitar();
        d.notes = vec![
            entry(PPQ, 60),
            TabNoteRef::new(0, 64, 1, 5),
            TabNoteRef::new(PPQ, 60, 2, 9),
        ];
        let d = d.canonical();
        assert_eq!(d.notes.len(), 2);
        assert_eq!(d.notes[0].tick, 0);
        assert_eq!(d.notes[1].string, 2, "the later entry replaced the earlier");
        assert!(d.check(MAX).is_ok());
    }

    #[test]
    fn the_check_refuses_what_cannot_be_played() {
        let mut d = guitar();
        d.notes = vec![TabNoteRef::new(0, 60, 6, 0)];
        assert!(d.check(MAX).unwrap_err().contains("string 6"));
        d.notes = vec![TabNoteRef::new(0, 60, 0, 99)];
        assert!(d.check(MAX).unwrap_err().contains("fret 99"));
        d.notes = vec![entry(0, 60), entry(0, 60)];
        assert!(d.check(MAX).unwrap_err().contains("sorted"));
        d.notes = vec![TabNoteRef::new(-1, 60, 0, 0)];
        assert!(d.check(MAX).is_err());
        let mut d = guitar();
        d.capo = Capo {
            fret: 99,
            strings: None,
        };
        assert!(d.check(MAX).unwrap_err().contains("capo"));
        let mut d = guitar();
        d.tuning.strings.clear();
        assert!(d.check(MAX).unwrap_err().contains("tuning"));
        let mut d = guitar();
        let mut n = entry(0, 60);
        n.tech.bend = Some(Bend::from_pairs(&[(0.5, 100), (0.1, 0)]));
        d.notes = vec![n];
        assert!(d.check(MAX).unwrap_err().contains("bounds"));
        let mut n = entry(0, 60);
        n.tech.harmonic = Some(Harmonic {
            kind: HarmonicKind::Natural,
            fret: Some(f32::NAN),
        });
        d.notes = vec![n];
        assert!(d.check(MAX).is_err(), "a NaN harmonic fret is refused");
    }

    #[test]
    fn canonical_brings_techniques_inside_their_bounds() {
        let mut d = guitar();
        let mut n = entry(0, 60);
        n.tech.bend = Some(Bend::from_pairs(&[(2.0, 99_999), (-1.0, -99_999)]));
        n.tech.tremolo_division = 7;
        n.tech.harmonic = Some(Harmonic {
            kind: HarmonicKind::Artificial,
            fret: Some(f32::INFINITY),
        });
        d.notes = vec![n];
        let d = d.canonical();
        assert_eq!(d.check(MAX), Ok(()));
        assert_eq!(d.notes[0].tech.tremolo_division, 0);
    }

    #[test]
    fn a_window_keeps_what_starts_inside_and_moves_it() {
        let mut d = guitar();
        d.notes = vec![entry(0, 60), entry(PPQ, 62), entry(2 * PPQ, 64)];
        d.lyrics = vec![TabLyric {
            tick: PPQ,
            text: "la".into(),
        }];
        let w = d.window(PPQ, 2 * PPQ, 10);
        assert_eq!(w.notes.len(), 1);
        assert_eq!(w.notes[0].tick, 10);
        assert_eq!(w.lyrics[0].tick, 10);
        assert_eq!(w.tuning, d.tuning);
    }

    #[test]
    fn map_ticks_moves_everything_and_repairs_ties() {
        let mut d = guitar();
        d.notes = vec![entry(0, 60), entry(PPQ, 60)];
        let m = d.map_ticks(|_| 5);
        assert_eq!(m.notes.len(), 1, "two entries of one note collapse");
    }

    #[test]
    fn appending_keeps_positions_only_for_the_same_neck() {
        let mut a = guitar();
        a.notes = vec![entry(0, 60)];
        let mut b = guitar();
        b.notes = vec![entry(PPQ, 62)];
        let mut same = a.clone();
        same.append(b.clone());
        assert_eq!(same.notes.len(), 2);
        let mut other = a.clone();
        b.capo = Capo::at(2);
        other.append(b);
        assert_eq!(
            other.notes.len(),
            1,
            "another capo: the entries are left out"
        );
    }

    #[test]
    fn entries_are_set_and_removed_by_note() {
        let d = guitar();
        let d = d.with_note(TabNoteRef::new(0, 60, 1, 1)).unwrap();
        let d = d.with_note(TabNoteRef::new(0, 60, 2, 5)).unwrap();
        assert_eq!(d.notes.len(), 1);
        assert_eq!(d.note_of(0, 60).unwrap().string, 2);
        assert!(d.note_on_string(0, 2).is_some());
        assert!(d.note_on_string(0, 1).is_none());
        assert!(d.without_note(0, 61).is_none());
        assert!(d.without_note(0, 60).unwrap().notes.is_empty());
    }

    #[test]
    fn ids_round_trip() {
        let all = ids::FLAGS.iter().fold(0, |m, (f, _)| m | f.0);
        assert_eq!(all, TechFlags::ALL.0, "every flag has a name");
        for (flag, _) in ids::FLAGS {
            assert_eq!(flag.0.count_ones(), 1);
        }
        for s in [SlideKind::Legato, SlideKind::OutUp, SlideKind::InFromAbove] {
            assert_eq!(ids::slide_from_id(ids::slide_id(s)), Some(s));
        }
        assert_eq!(ids::slide_from_id("sideways"), None);
        assert_eq!(
            ids::harmonic_from_id(ids::harmonic_id(HarmonicKind::Pinch)),
            Some(HarmonicKind::Pinch)
        );
        assert_eq!(
            ids::strum_from_id(ids::strum_id(StrumDirection::Up)),
            Some(StrumDirection::Up)
        );
        assert_eq!(
            ids::vibrato_from_id(ids::vibrato_id(VibratoKind::Wide)),
            Some(VibratoKind::Wide)
        );
    }
}
