//! Technique to sound: what a note's playing techniques do to the note a clip holds.
//!
//! The clip's note list is what is heard, so the techniques of [`crate::tab_data`] are rendered
//! into it. [`render_note`] is a pure function from a note's plain velocity and length, its
//! techniques and the next note on its string to the note as it sounds:
//!
//! | technique | rendered as |
//! |---|---|
//! | bend, pre-bend, release, whammy | pitch curve (cents / 100 semitones, positions along the note) |
//! | vibrato | pitch sine after a short onset ([`VIBRATO_NORMAL`], [`VIBRATO_WIDE`] semitones) |
//! | slide | pitch glide that reaches the next note's pitch exactly when it starts; the note is cut or stretched to end there. In/out slides glide [`SLIDE_EDGE_SEMITONES`] at the edge |
//! | harmonic | pitch offset to the sounding partial on the pitch lane, and pressure code [`PRESSURE_HARMONIC`] at the note start |
//! | palm mute | Slide (timbre) lane, the damping amount [`TIMBRE_PALM_MUTE`] |
//! | dead | pressure code [`PRESSURE_DEAD`] at the note start (and a very short length) |
//! | ghost, accent, heavy accent | velocity x [`GHOST_VELOCITY`], [`ACCENT_VELOCITY`], [`HEAVY_ACCENT_VELOCITY`], capped at 1 |
//! | staccato | length halved (not below [`MIN_LENGTH`]) |
//! | dead | length at most [`DEAD_LENGTH`] |
//! | let ring | length to the next note on the string (or [`LET_RING_TAIL`]), at most [`MAX_LET_RING`] |
//!
//! Hammer-on, pull-off, tap, slap, pop, thumb, fade in, tremolo picking, grace notes and strum directions
//! are drawn in the tab but not rendered: they have no counterpart in a note list.
//!
//! [`render_notes`] applies this to a clip's notes and expression after an edit. A note whose
//! techniques changed it keeps its plain velocity and length in its [`crate::tab_data::PlainNote`],
//! so rendering again (or clearing the techniques) never compounds the effect. A note with
//! curve-producing techniques owns its expression entry; a note without keeps what the user drew
//! unless its techniques only just stopped producing curves.

use std::collections::BTreeSet;

use atomik_core::PPQ;
use atomik_tab::score::{Harmonic, HarmonicKind, SlideKind, TechFlags, Techniques, VibratoKind};

use crate::midi_data::MidiData;
use crate::note_expression::{
    ExpressionPoint, MAX_EXPRESSION_NOTES, MAX_EXPRESSION_POINTS, NoteExpression, PITCH_LIMIT,
    value_at,
};
use crate::song::{MAX_TICK, Note};
use crate::tab_data::{PlainNote, TabData, TabNoteRef};

/// The velocity factor of a ghost note.
pub const GHOST_VELOCITY: f32 = 0.6;
/// The velocity factor of an accent.
pub const ACCENT_VELOCITY: f32 = 1.2;
/// The velocity factor of a heavy accent.
pub const HEAVY_ACCENT_VELOCITY: f32 = 1.4;
/// The lowest velocity a rendered note has (one MIDI step).
pub const MIN_VELOCITY: f32 = 1.0 / 127.0;
/// The longest a dead (muted) note sounds, in ticks: a 64th note.
pub const DEAD_LENGTH: i64 = PPQ / 16;
/// The shortest a staccato note is cut to, in ticks.
pub const MIN_LENGTH: i64 = PPQ / 32;
/// The most a let-ring note is stretched to, in ticks (two bars of 4/4).
pub const MAX_LET_RING: i64 = 8 * PPQ;
/// How long a let-ring note rings when no note follows on its string, in ticks.
pub const LET_RING_TAIL: i64 = 2 * PPQ;
/// The timbre value of a palm-muted note (the amount of damping, `0..=1`).
pub const TIMBRE_PALM_MUTE: f32 = 0.7;
/// The pressure code of a harmonic note (the plucked-string device reads `0.25..0.75` as harmonic;
/// it must equal its `EXPR_HARMONIC`).
pub const PRESSURE_HARMONIC: f32 = 0.5;
/// The pressure code of a dead note (above `0.75`; it must equal the device's `EXPR_DEAD`).
pub const PRESSURE_DEAD: f32 = 1.0;
/// The depth of a normal vibrato, in semitones either way.
pub const VIBRATO_NORMAL: f32 = 0.3;
/// The depth of a wide vibrato, in semitones either way.
pub const VIBRATO_WIDE: f32 = 0.7;
/// Vibrato cycles per quarter note.
pub const VIBRATO_CYCLES_PER_BEAT: f64 = 2.0;
/// The distance an in-slide starts from and an out-slide ends at, in semitones.
pub const SLIDE_EDGE_SEMITONES: f32 = 3.0;

/// The sample points of a vibrato cycle.
const VIBRATO_POINTS_PER_CYCLE: i64 = 8;
/// The most points a vibrato adds to one curve.
const VIBRATO_MAX_POINTS: i64 = 300;

/// The next note on a note's string: where it starts and the key it sounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextNote {
    /// Start tick in the clip's content ticks.
    pub tick: i64,
    /// Key of the note.
    pub key: u8,
}

/// A note as its techniques make it sound.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The velocity in `(0, 1]`.
    pub velocity: f32,
    /// The length in ticks, at least 1.
    pub length: i64,
    /// The curves the techniques produce (empty when they produce none).
    pub expression: NoteExpression,
}

/// True when the techniques produce expression curves (pitch, timbre or pressure).
pub fn produces_curves(t: &Techniques) -> bool {
    t.bend.as_ref().is_some_and(|b| !b.points.is_empty())
        || t.whammy.as_ref().is_some_and(|b| !b.points.is_empty())
        || t.slide.is_some()
        || t.vibrato.is_some()
        || t.harmonic.is_some()
        || t.has(TechFlags::PALM_MUTE)
        || t.has(TechFlags::DEAD)
}

/// True when the techniques change the velocity or the length of the note.
pub fn changes_note(t: &Techniques) -> bool {
    t.has(TechFlags::GHOST)
        || t.has(TechFlags::ACCENT)
        || t.has(TechFlags::HEAVY_ACCENT)
        || t.has(TechFlags::STACCATO)
        || t.has(TechFlags::LET_RING)
        || t.has(TechFlags::DEAD)
        || matches!(t.slide, Some(SlideKind::Legato | SlideKind::Shift))
}

/// The nodes of the open string a natural harmonic sounds at: `(fret, semitones above the open
/// string)`.
const HARMONIC_NODES: [(f32, f32); 10] = [
    (12.0, 12.0),
    (7.0, 19.0),
    (19.0, 19.0),
    (5.0, 24.0),
    (24.0, 24.0),
    (4.0, 28.0),
    (9.0, 28.0),
    (16.0, 28.0),
    (3.0, 31.0),
    (28.0, 28.0 + 12.0),
];

/// The semitones above the open string that a harmonic node `distance` frets from the nut of its
/// string sounds, `None` when no node is within half a fret.
fn node_semitones(distance: f32) -> Option<f32> {
    HARMONIC_NODES
        .iter()
        .filter(|(f, _)| (f - distance).abs() <= 0.6)
        .min_by(|a, b| (a.0 - distance).abs().total_cmp(&(b.0 - distance).abs()))
        .map(|(_, s)| *s)
}

/// The pitch a harmonic adds to the fretted note, in semitones: a natural harmonic at fret `f`
/// sounds the partial of that node instead of the fretted pitch; an artificial, pinch or tapped
/// one sounds the node as far above the fretted note as the touched fret is (an octave when no
/// touched fret is known).
pub fn harmonic_offset(h: &Harmonic, fret: u8) -> f32 {
    let fretted = f32::from(fret);
    match h.kind {
        HarmonicKind::Natural => node_semitones(fretted).map_or(0.0, |s| s - fretted),
        HarmonicKind::Semi => 0.0,
        HarmonicKind::Artificial | HarmonicKind::Pinch | HarmonicKind::Tapped => h
            .fret
            .filter(|t| t.is_finite())
            .and_then(|t| node_semitones(t - fretted))
            .unwrap_or(12.0),
    }
}

/// A piecewise-linear curve from `(tick, value)` pairs in ascending tick order.
fn curve(points: impl IntoIterator<Item = (i64, f32)>) -> Vec<ExpressionPoint> {
    let mut out: Vec<ExpressionPoint> = Vec::new();
    for (tick, value) in points {
        let p = ExpressionPoint::new(tick.max(0), value);
        match out.last_mut() {
            Some(last) if last.tick >= p.tick => *last = p,
            _ => out.push(p),
        }
    }
    out
}

/// The glide of a slide as pitch points, for a note `length` ticks long at `key`; `next` is the
/// next note on the string.
fn slide_points(kind: SlideKind, key: u8, length: i64, next: Option<NextNote>) -> Vec<(i64, f32)> {
    let edge = (length / 3).clamp(1, PPQ / 4);
    match kind {
        SlideKind::Legato | SlideKind::Shift => {
            let Some(next) = next else {
                return Vec::new();
            };
            let target = f32::from(next.key) - f32::from(key);
            let glide = if kind == SlideKind::Legato {
                (length * 6 / 10).clamp(1, PPQ / 2)
            } else {
                (length * 4 / 10).clamp(1, PPQ / 4)
            };
            vec![(0, 0.0), (length - glide, 0.0), (length, target)]
        }
        SlideKind::InFromBelow => vec![(0, -SLIDE_EDGE_SEMITONES), (edge, 0.0)],
        SlideKind::InFromAbove => vec![(0, SLIDE_EDGE_SEMITONES), (edge, 0.0)],
        SlideKind::OutDown => vec![(length / 2, 0.0), (length, -SLIDE_EDGE_SEMITONES)],
        SlideKind::OutUp => vec![(length / 2, 0.0), (length, SLIDE_EDGE_SEMITONES)],
    }
}

/// The sine of a vibrato over a note `length` ticks long, as points.
fn vibrato_points(kind: VibratoKind, length: i64) -> Vec<(i64, f32)> {
    let depth = match kind {
        VibratoKind::Normal => VIBRATO_NORMAL,
        VibratoKind::Wide => VIBRATO_WIDE,
    };
    let onset = (length / 4).min(PPQ / 2);
    let cycle = (PPQ as f64 / VIBRATO_CYCLES_PER_BEAT) as i64;
    let mut step = (cycle / VIBRATO_POINTS_PER_CYCLE).max(1);
    let span = (length - onset).max(0);
    if span / step > VIBRATO_MAX_POINTS {
        step = span / VIBRATO_MAX_POINTS + 1;
    }
    let mut out = vec![(0, 0.0)];
    let mut t = onset;
    while t < length {
        let into = (t - onset) as f64;
        let phase = into / cycle as f64 * std::f64::consts::TAU;
        // The depth grows over the first cycle so the vibrato does not start with a jump.
        let grow = (into / cycle as f64).min(1.0);
        out.push((t, (phase.sin() * f64::from(depth) * grow) as f32));
        t += step;
    }
    out.push((length, 0.0));
    out
}

/// The sum of piecewise-linear curves, sampled at every tick any of them has (and at 0 and at
/// `length`), with the points that lie on a straight line between their neighbours left out.
/// Empty when the sum is zero everywhere.
fn sum_curves(parts: &[Vec<ExpressionPoint>], length: i64) -> Vec<ExpressionPoint> {
    let mut ticks: BTreeSet<i64> = BTreeSet::from([0, length.max(0)]);
    for part in parts {
        ticks.extend(part.iter().map(|p| p.tick.clamp(0, length.max(0))));
    }
    let at = |t: i64| -> f32 {
        parts
            .iter()
            .map(|p| value_at(p, t).unwrap_or(0.0))
            .sum::<f32>()
            .clamp(-PITCH_LIMIT, PITCH_LIMIT)
    };
    let points: Vec<ExpressionPoint> = ticks
        .into_iter()
        .map(|t| {
            let v = at(t);
            ExpressionPoint::new(t, if v.is_finite() { v } else { 0.0 })
        })
        .collect();
    if points.iter().all(|p| p.value.abs() < 1e-4) {
        return Vec::new();
    }
    let mut thin: Vec<ExpressionPoint> = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        let collinear = match (thin.last(), points.get(i + 1)) {
            (Some(a), Some(b)) => {
                let span = (b.tick - a.tick) as f64;
                let t = (p.tick - a.tick) as f64 / span;
                let on_line = f64::from(a.value) + (f64::from(b.value) - f64::from(a.value)) * t;
                (f64::from(p.value) - on_line).abs() < 1e-4
            }
            _ => false,
        };
        if !collinear {
            thin.push(*p);
        }
    }
    thin.truncate(MAX_EXPRESSION_POINTS);
    thin
}

/// Where a note is: its start tick and key in the clip and the fret it is played at (the fret
/// decides where a natural harmonic sits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spot {
    /// Start tick in the clip's content ticks.
    pub tick: i64,
    /// Key of the note.
    pub key: u8,
    /// The fret, relative to the capo.
    pub fret: u8,
}

/// Renders one note: `plain` is its velocity and length without techniques, `next` is the next
/// note on the same string. Techniques out of bounds are brought inside them first, so a hostile
/// value cannot produce a NaN.
pub fn render_note(
    spot: Spot,
    plain: PlainNote,
    tech: &Techniques,
    next: Option<NextNote>,
) -> Rendered {
    let Spot { tick, key, fret } = spot;
    let mut tech = tech.clone();
    tech.sanitize();
    let base_velocity = if plain.velocity.is_finite() {
        plain.velocity.clamp(MIN_VELOCITY, 1.0)
    } else {
        1.0
    };
    let next = next.filter(|n| n.tick > tick);

    // Velocity.
    let mut velocity = base_velocity;
    if tech.has(TechFlags::GHOST) {
        velocity *= GHOST_VELOCITY;
    }
    if tech.has(TechFlags::HEAVY_ACCENT) {
        velocity *= HEAVY_ACCENT_VELOCITY;
    } else if tech.has(TechFlags::ACCENT) {
        velocity *= ACCENT_VELOCITY;
    }
    let velocity = velocity.clamp(MIN_VELOCITY, 1.0);

    // Length.
    let mut length = plain.length.clamp(1, MAX_TICK);
    if tech.has(TechFlags::STACCATO) {
        length = (length / 2).max(MIN_LENGTH.min(length));
    } else if tech.has(TechFlags::LET_RING) && !tech.has(TechFlags::DEAD) {
        let ring = next.map_or(LET_RING_TAIL, |n| n.tick - tick);
        length = length.max(ring).min(MAX_LET_RING.max(length));
    }
    if let (Some(SlideKind::Legato | SlideKind::Shift), Some(n)) = (tech.slide, next) {
        length = n.tick - tick;
    }
    if tech.has(TechFlags::DEAD) {
        length = length.min(DEAD_LENGTH);
    }
    let length = length.clamp(1, MAX_TICK);

    // Curves.
    let mut pitch_parts: Vec<Vec<ExpressionPoint>> = Vec::new();
    for bend in tech.bend.iter().chain(tech.whammy.iter()) {
        if bend.points.is_empty() {
            continue;
        }
        pitch_parts.push(curve(bend.points.iter().map(|p| {
            let at = (f64::from(p.pos) * length as f64).round() as i64;
            (at.clamp(0, length), p.cents as f32 / 100.0)
        })));
    }
    if let Some(h) = &tech.harmonic {
        let offset = harmonic_offset(h, fret);
        if offset != 0.0 {
            pitch_parts.push(curve([(0, offset)]));
        }
    }
    if let Some(kind) = tech.slide {
        let pts = slide_points(kind, key, length, next);
        if !pts.is_empty() {
            pitch_parts.push(curve(pts));
        }
    }
    if let Some(kind) = tech.vibrato {
        pitch_parts.push(curve(vibrato_points(kind, length)));
    }
    let pitch = sum_curves(&pitch_parts, length);

    // The Slide (timbre) lane carries the palm-mute amount; the pressure lane is the technique
    // code the plucked-string device reads at the note start (dead above 0.75, harmonic 0.25..0.75).
    let timbre = if tech.has(TechFlags::PALM_MUTE) {
        vec![ExpressionPoint::new(0, TIMBRE_PALM_MUTE)]
    } else {
        Vec::new()
    };
    let pressure = if tech.has(TechFlags::DEAD) {
        vec![ExpressionPoint::new(0, PRESSURE_DEAD)]
    } else if tech.harmonic.is_some() {
        vec![ExpressionPoint::new(0, PRESSURE_HARMONIC)]
    } else {
        Vec::new()
    };
    let expression = NoteExpression {
        tick,
        key,
        pitch,
        pressure,
        timbre,
    }
    .canonical();
    Rendered {
        velocity,
        length,
        expression,
    }
}

/// The notes of each string by start, to find the next and the previous note on a string.
struct StringIndex {
    per_string: Vec<Vec<(i64, u8)>>,
}

impl StringIndex {
    fn new(tab: &TabData) -> Self {
        let mut per_string: Vec<Vec<(i64, u8)>> = vec![Vec::new(); tab.tuning.len()];
        for n in &tab.notes {
            if let Some(list) = per_string.get_mut(usize::from(n.string)) {
                list.push((n.tick, n.key));
            }
        }
        Self { per_string }
    }

    /// The first note on `string` that starts after `tick`.
    fn next(&self, string: u8, tick: i64) -> Option<NextNote> {
        let list = self.per_string.get(usize::from(string))?;
        let i = list.partition_point(|(t, _)| *t <= tick);
        list.get(i).map(|&(tick, key)| NextNote { tick, key })
    }

    /// The last note on `string` that starts before `tick`.
    fn prev(&self, string: u8, tick: i64) -> Option<(i64, u8)> {
        let list = self.per_string.get(usize::from(string))?;
        let i = list.partition_point(|(t, _)| *t < tick);
        i.checked_sub(1).and_then(|i| list.get(i)).copied()
    }
}

fn spot_of(r: &TabNoteRef) -> Spot {
    Spot {
        tick: r.tick,
        key: r.key,
        fret: r.fret,
    }
}

/// The plain velocity and length of `note`, whose entry `old` (with `next` the next note on its
/// string) rendered it: the plain values the entry kept when the note still is what rendering
/// them gave, else what the note has now (the user edited it since).
pub fn current_plain(note: &Note, old: &TabNoteRef, next: Option<NextNote>) -> PlainNote {
    let now = PlainNote {
        velocity: note.velocity,
        length: note.length,
    };
    match old.plain {
        Some(p) => {
            let r = render_note(spot_of(old), p, &old.tech, next);
            if (r.velocity - note.velocity).abs() < 1e-4 && r.length == note.length {
                p
            } else {
                now
            }
        }
        None => now,
    }
}

fn remove_expression(expression: &mut Vec<NoteExpression>, tick: i64, key: u8) {
    if let Ok(i) = expression.binary_search_by_key(&(tick, key), |e| (e.tick, e.key)) {
        expression.remove(i);
    }
}

fn set_expression(expression: &mut Vec<NoteExpression>, entry: NoteExpression) {
    if entry.is_empty() {
        remove_expression(expression, entry.tick, entry.key);
        return;
    }
    match expression.binary_search_by_key(&(entry.tick, entry.key), |e| (e.tick, e.key)) {
        Ok(i) => expression[i] = entry,
        Err(_) if expression.len() >= MAX_EXPRESSION_NOTES => {}
        Err(i) => expression.insert(i, entry),
    }
}

/// Gives the notes in `selection` back their plain velocity and length and removes the curves
/// their techniques rendered, and forgets the plain values in their entries. Used before an edit
/// that moves a note to another key (the entry follows the note by tick and key).
pub fn unrender(notes: &mut [Note], midi: &mut MidiData, selection: &[(i64, u8)]) {
    let MidiData {
        tab, expression, ..
    } = midi;
    let Some(tab) = tab.as_mut() else {
        return;
    };
    let index = StringIndex::new(tab);
    for &(tick, key) in selection {
        let Ok(ti) = tab
            .notes
            .binary_search_by_key(&(tick, key), |n| (n.tick, n.key))
        else {
            continue;
        };
        let old = tab.notes[ti].clone();
        if let Ok(i) = notes.binary_search_by_key(&(tick, key), |n| (n.tick, n.key)) {
            let plain = current_plain(&notes[i], &old, index.next(old.string, old.tick));
            notes[i].velocity = plain.velocity;
            notes[i].length = plain.length;
        }
        if produces_curves(&old.tech) {
            remove_expression(expression, tick, key);
        }
        tab.notes[ti].plain = None;
    }
}

/// Renders the techniques of the notes in `touched` (and of the note before each on its string,
/// whose length may depend on it) into the clip: velocity and length into `notes` (sorted by
/// `(tick, key)`), curves into the clip's expression, the plain values into the entries.
///
/// `before` is the tab data as it was before the edit that changed it: the plain values of a note
/// are recovered from its entry there, and curves that techniques stopped producing are removed.
/// With `None` every note starts from the velocity and length it has now.
pub fn render_notes(
    notes: &mut [Note],
    midi: &mut MidiData,
    touched: &[(i64, u8)],
    before: Option<&TabData>,
) {
    let MidiData {
        tab, expression, ..
    } = midi;
    let Some(tab) = tab.as_mut() else {
        return;
    };
    let new_index = StringIndex::new(tab);
    let old_index = before.map(StringIndex::new);
    let mut set: BTreeSet<(i64, u8)> = touched.iter().copied().collect();
    for &(tick, key) in touched {
        if let Some(r) = tab.note_of(tick, key)
            && let Some(p) = new_index.prev(r.string, r.tick)
        {
            set.insert(p);
        }
        if let (Some(b), Some(ix)) = (before, &old_index)
            && let Some(r) = b.note_of(tick, key)
            && let Some(p) = ix.prev(r.string, r.tick)
        {
            set.insert(p);
        }
    }
    for (tick, key) in set {
        let ni = notes
            .binary_search_by_key(&(tick, key), |n| (n.tick, n.key))
            .ok();
        let old = before.and_then(|b| b.note_of(tick, key));
        let ti = tab
            .notes
            .binary_search_by_key(&(tick, key), |n| (n.tick, n.key))
            .ok();
        let Some(ti) = ti else {
            // The entry is gone: the note plays plain again and its rendered curves go.
            if let Some(old) = old {
                if let (Some(i), Some(p)) = (ni, old.plain) {
                    notes[i].velocity = p.velocity;
                    notes[i].length = p.length;
                }
                if produces_curves(&old.tech) {
                    remove_expression(expression, tick, key);
                }
            }
            continue;
        };
        let Some(i) = ni else {
            continue;
        };
        let entry = tab.notes[ti].clone();
        let base = match old {
            Some(o) => {
                let next = old_index.as_ref().and_then(|ix| ix.next(o.string, o.tick));
                current_plain(&notes[i], o, next)
            }
            None => PlainNote {
                velocity: notes[i].velocity,
                length: notes[i].length,
            },
        };
        let r = render_note(
            spot_of(&entry),
            base,
            &entry.tech,
            new_index.next(entry.string, entry.tick),
        );
        notes[i].velocity = r.velocity;
        notes[i].length = r.length;
        tab.notes[ti].plain = (r.velocity.to_bits() != base.velocity.to_bits()
            || r.length != base.length)
            .then_some(base);
        if produces_curves(&entry.tech) {
            set_expression(expression, r.expression);
        } else if old.is_some_and(|o| produces_curves(&o.tech)) {
            remove_expression(expression, tick, key);
        }
    }
}

/// [`render_notes`] for every entry of the clip, each starting from the velocity and length its
/// note has now unless the entry's plain values still explain them (a clip that was rendered
/// before keeps its plain values). Used after an import.
pub fn render_all(notes: &mut [Note], midi: &mut MidiData) {
    let Some(tab) = midi.tab.clone() else {
        return;
    };
    let touched: Vec<(i64, u8)> = tab.notes.iter().map(|n| (n.tick, n.key)).collect();
    render_notes(notes, midi, &touched, Some(&tab));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note_expression::value_at;
    use atomik_tab::score::{Bend, SlideKind};

    fn plain(length: i64) -> PlainNote {
        PlainNote {
            velocity: 0.8,
            length,
        }
    }

    fn spot(fret: u8) -> Spot {
        Spot {
            tick: 0,
            key: 60,
            fret,
        }
    }

    fn pitch_at(r: &Rendered, t: i64) -> f32 {
        value_at(&r.expression.pitch, t).unwrap_or(0.0)
    }

    #[test]
    fn a_bend_reaches_its_cents_at_its_position_and_holds() {
        let tech = Techniques {
            bend: Some(Bend::up(200)),
            ..Techniques::default()
        };
        let len = 4 * PPQ;
        let r = render_note(spot(5), plain(len), &tech, None);
        assert!(pitch_at(&r, 0).abs() < 1e-6);
        assert!((pitch_at(&r, (0.3 * len as f64) as i64) - 2.0).abs() < 0.01);
        assert!((pitch_at(&r, len) - 2.0).abs() < 1e-6);
        assert!((pitch_at(&r, len / 2) - 2.0).abs() < 1e-6, "held");
        // Halfway up the rise is a semitone.
        assert!((pitch_at(&r, (0.15 * len as f64) as i64) - 1.0).abs() < 0.02);
    }

    #[test]
    fn a_release_returns_to_pitch_and_a_pre_bend_starts_bent() {
        let len = 4 * PPQ;
        let rel = Techniques {
            bend: Some(Bend::bend_release(100)),
            ..Techniques::default()
        };
        let r = render_note(spot(5), plain(len), &rel, None);
        assert!((pitch_at(&r, len / 2) - 1.0).abs() < 0.01);
        assert!(pitch_at(&r, len).abs() < 1e-6);
        let pre = Techniques {
            bend: Some(Bend::pre_bend_release(200)),
            ..Techniques::default()
        };
        let r = render_note(spot(5), plain(len), &pre, None);
        assert!((pitch_at(&r, 0) - 2.0).abs() < 1e-6, "bent at the attack");
        assert!(pitch_at(&r, len).abs() < 1e-6);
    }

    #[test]
    fn a_shift_slide_hits_the_next_pitch_exactly_when_it_starts() {
        let tech = Techniques {
            slide: Some(SlideKind::Shift),
            ..Techniques::default()
        };
        let next = NextNote { tick: PPQ, key: 64 };
        // The note was shorter than the gap: it stretches to the next note.
        let r = render_note(spot(5), plain(PPQ / 2), &tech, Some(next));
        assert_eq!(r.length, PPQ);
        assert!(pitch_at(&r, 0).abs() < 1e-6);
        assert!((pitch_at(&r, r.length) - 4.0).abs() < 1e-6);
        assert!(
            pitch_at(&r, r.length - PPQ / 4 - 1).abs() < 0.05,
            "glide is late"
        );
        // Longer than the gap: it is cut at the next note.
        let r = render_note(spot(5), plain(4 * PPQ), &tech, Some(next));
        assert_eq!(r.length, PPQ);
        // A slide down.
        let down = NextNote { tick: PPQ, key: 55 };
        let r = render_note(spot(5), plain(PPQ), &tech, Some(down));
        assert!((pitch_at(&r, r.length) + 5.0).abs() < 1e-6);
        // No next note: nothing to glide to.
        let r = render_note(spot(5), plain(PPQ), &tech, None);
        assert!(r.expression.pitch.is_empty());
    }

    #[test]
    fn edge_slides_glide_three_semitones() {
        let len = 2 * PPQ;
        let mk = |kind| Techniques {
            slide: Some(kind),
            ..Techniques::default()
        };
        let r = render_note(spot(5), plain(len), &mk(SlideKind::InFromBelow), None);
        assert!((pitch_at(&r, 0) + SLIDE_EDGE_SEMITONES).abs() < 1e-6);
        assert!(pitch_at(&r, len).abs() < 1e-6);
        let r = render_note(spot(5), plain(len), &mk(SlideKind::OutDown), None);
        assert!(pitch_at(&r, len / 2).abs() < 1e-6);
        assert!((pitch_at(&r, len) + SLIDE_EDGE_SEMITONES).abs() < 1e-6);
        let r = render_note(spot(5), plain(len), &mk(SlideKind::OutUp), None);
        assert!((pitch_at(&r, len) - SLIDE_EDGE_SEMITONES).abs() < 1e-6);
    }

    #[test]
    fn harmonics_offset_the_pitch_and_set_the_pressure_code() {
        let nat = |fret: u8, kind| {
            let tech = Techniques {
                harmonic: Some(Harmonic { kind, fret: None }),
                ..Techniques::default()
            };
            render_note(spot(fret), plain(PPQ), &tech, None)
        };
        let r = nat(12, HarmonicKind::Natural);
        assert!(
            r.expression.pitch.is_empty(),
            "the 12th fret node is the fretted pitch"
        );
        assert_eq!(r.expression.pressure[0].value, PRESSURE_HARMONIC);
        assert!((pitch_at(&nat(7, HarmonicKind::Natural), 0) - 12.0).abs() < 1e-6);
        assert!((pitch_at(&nat(5, HarmonicKind::Natural), 0) - 19.0).abs() < 1e-6);
        assert!((pitch_at(&nat(9, HarmonicKind::Natural), 0) - 19.0).abs() < 1e-6);
        // An artificial harmonic touched 12 frets above the fretted note is an octave up.
        let tech = Techniques {
            harmonic: Some(Harmonic {
                kind: HarmonicKind::Artificial,
                fret: Some(17.0),
            }),
            ..Techniques::default()
        };
        let r = render_note(spot(5), plain(PPQ), &tech, None);
        assert!((pitch_at(&r, 0) - 12.0).abs() < 1e-6);
        // A pinch harmonic without a touched fret is an octave.
        assert!((pitch_at(&nat(5, HarmonicKind::Pinch), 0) - 12.0).abs() < 1e-6);
    }

    #[test]
    fn vibrato_stays_inside_its_depth_and_starts_and_ends_on_pitch() {
        let len = 4 * PPQ;
        for (kind, depth) in [
            (VibratoKind::Normal, VIBRATO_NORMAL),
            (VibratoKind::Wide, VIBRATO_WIDE),
        ] {
            let tech = Techniques {
                vibrato: Some(kind),
                ..Techniques::default()
            };
            let r = render_note(spot(5), plain(len), &tech, None);
            let pts = &r.expression.pitch;
            assert!(pts.len() > 8 && pts.len() <= MAX_EXPRESSION_POINTS);
            assert!(
                pts.iter()
                    .all(|p| p.value.abs() <= depth + 1e-4 && p.value.is_finite())
            );
            assert!(
                pts.iter().any(|p| p.value > depth * 0.9),
                "it reaches its depth"
            );
            assert!(pitch_at(&r, 0).abs() < 1e-6);
            assert!(pitch_at(&r, len).abs() < 1e-6);
        }
        // A very long note keeps the point count bounded.
        let tech = Techniques {
            vibrato: Some(VibratoKind::Normal),
            ..Techniques::default()
        };
        let r = render_note(spot(5), plain(MAX_TICK / 4), &tech, None);
        assert!(r.expression.pitch.len() <= MAX_EXPRESSION_POINTS);
    }

    #[test]
    fn bend_and_vibrato_add_up() {
        let len = 4 * PPQ;
        let tech = Techniques {
            bend: Some(Bend::pre_bend(100)),
            vibrato: Some(VibratoKind::Normal),
            ..Techniques::default()
        };
        let r = render_note(spot(5), plain(len), &tech, None);
        assert!(
            r.expression
                .pitch
                .iter()
                .all(|p| p.value > 0.6 && p.value < 1.4)
        );
    }

    #[test]
    fn velocity_is_ghost_accent_and_capped() {
        let v = |flags: TechFlags, base: f32| {
            let tech = Techniques {
                flags,
                ..Techniques::default()
            };
            let p = PlainNote {
                velocity: base,
                length: PPQ,
            };
            render_note(spot(0), p, &tech, None).velocity
        };
        assert!((v(TechFlags::GHOST, 0.5) - 0.3).abs() < 1e-6);
        assert!((v(TechFlags::ACCENT, 0.5) - 0.6).abs() < 1e-6);
        assert!((v(TechFlags::HEAVY_ACCENT, 0.5) - 0.7).abs() < 1e-6);
        assert_eq!(v(TechFlags::ACCENT, 0.95), 1.0, "capped");
        assert!((v(TechFlags::GHOST | TechFlags::ACCENT, 0.5) - 0.36).abs() < 1e-6);
        assert_eq!(v(TechFlags::NONE, 0.5), 0.5);
        assert!(v(TechFlags::GHOST, 0.001) >= MIN_VELOCITY, "never silent");
    }

    #[test]
    fn lengths_follow_staccato_dead_and_let_ring() {
        let len = |flags: TechFlags, base: i64, next: Option<NextNote>| {
            let tech = Techniques {
                flags,
                ..Techniques::default()
            };
            render_note(spot(0), plain(base), &tech, next).length
        };
        assert_eq!(len(TechFlags::STACCATO, PPQ, None), PPQ / 2);
        assert_eq!(
            len(TechFlags::STACCATO, 4, None),
            4,
            "a tiny note is not cut below itself"
        );
        assert_eq!(len(TechFlags::DEAD, PPQ, None), DEAD_LENGTH);
        assert_eq!(len(TechFlags::DEAD, 10, None), 10, "never longer");
        let next = Some(NextNote {
            tick: 3 * PPQ,
            key: 62,
        });
        assert_eq!(len(TechFlags::LET_RING, PPQ, next), 3 * PPQ);
        assert_eq!(len(TechFlags::LET_RING, PPQ, None), LET_RING_TAIL);
        let far = Some(NextNote {
            tick: 100 * PPQ,
            key: 62,
        });
        assert_eq!(len(TechFlags::LET_RING, PPQ, far), MAX_LET_RING);
        assert_eq!(
            len(TechFlags::LET_RING, 9 * PPQ, far),
            9 * PPQ,
            "a long note keeps its length"
        );
        // A next note at or before the start is not a target.
        let behind = Some(NextNote { tick: 0, key: 62 });
        assert_eq!(len(TechFlags::LET_RING, PPQ, behind), LET_RING_TAIL);
    }

    #[test]
    fn palm_mute_is_the_slide_lane_and_dead_is_the_pressure_code() {
        let pm = Techniques::default().with(TechFlags::PALM_MUTE);
        let r = render_note(spot(0), plain(PPQ), &pm, None);
        assert_eq!(
            r.expression.timbre,
            vec![ExpressionPoint::new(0, TIMBRE_PALM_MUTE)]
        );
        assert!(r.expression.pressure.is_empty());
        let dead = Techniques::default().with(TechFlags::DEAD);
        let r = render_note(spot(0), plain(PPQ), &dead, None);
        assert_eq!(
            r.expression.pressure,
            vec![ExpressionPoint::new(0, PRESSURE_DEAD)]
        );
        assert!(r.expression.timbre.is_empty());
    }

    #[test]
    fn hostile_techniques_never_make_a_nan() {
        let tech = Techniques {
            bend: Some(Bend::from_pairs(&[
                (f32::NAN, i32::MAX),
                (f32::INFINITY, i32::MIN),
                (-3.0, 7),
            ])),
            whammy: Some(Bend::from_pairs(&[(0.5, 99_999)])),
            vibrato: Some(VibratoKind::Wide),
            harmonic: Some(Harmonic {
                kind: HarmonicKind::Artificial,
                fret: Some(f32::NAN),
            }),
            slide: Some(SlideKind::Legato),
            ..Techniques::default()
        }
        .with(TechFlags::ALL);
        for (len, vel) in [(1, 0.5), (PPQ, f32::NAN), (MAX_TICK, 1.0), (0, -1.0)] {
            let p = PlainNote {
                velocity: vel,
                length: len,
            };
            let next = Some(NextNote { tick: 1, key: 127 });
            let r = render_note(spot(255), p, &tech, next);
            assert!(r.velocity.is_finite() && r.velocity > 0.0 && r.velocity <= 1.0);
            assert!((1..=MAX_TICK).contains(&r.length));
            assert_eq!(r.expression.check(MAX_TICK), Ok(()), "{:?}", r.expression);
        }
    }

    // ---- render_notes -------------------------------------------------------------------

    fn clip() -> (Vec<Note>, MidiData) {
        let mut tab = TabData::for_instrument("guitar6").unwrap();
        tab.notes = vec![
            TabNoteRef::new(0, 64, 0, 0),
            TabNoteRef::new(PPQ, 64, 0, 0),
            TabNoteRef::new(2 * PPQ, 67, 1, 8),
        ];
        let notes = vec![
            Note::new(0, PPQ / 2, 64, 0.8),
            Note::new(PPQ, PPQ / 2, 64, 0.8),
            Note::new(2 * PPQ, PPQ / 2, 67, 0.8),
        ];
        (
            notes,
            MidiData {
                tab: Some(tab),
                ..MidiData::default()
            },
        )
    }

    fn set_tech(midi: &mut MidiData, tick: i64, key: u8, f: impl Fn(&mut Techniques)) {
        let tab = midi.tab.as_mut().unwrap();
        let r = tab
            .notes
            .iter_mut()
            .find(|n| n.tick == tick && n.key == key)
            .unwrap();
        f(&mut r.tech);
    }

    #[test]
    fn rendering_twice_does_not_compound_and_clearing_restores_the_note() {
        let (mut notes, mut midi) = clip();
        let before = midi.tab.clone();
        set_tech(&mut midi, 0, 64, |t| t.flags.insert(TechFlags::GHOST));
        render_notes(&mut notes, &mut midi, &[(0, 64)], before.as_ref());
        assert!((notes[0].velocity - 0.48).abs() < 1e-6);
        assert_eq!(
            midi.tab.as_ref().unwrap().notes[0].plain.unwrap().velocity,
            0.8
        );
        // Again, with the state as it is now.
        let before = midi.tab.clone();
        render_notes(&mut notes, &mut midi, &[(0, 64)], before.as_ref());
        assert!((notes[0].velocity - 0.48).abs() < 1e-6, "not 0.288");
        // Clear.
        let before = midi.tab.clone();
        set_tech(&mut midi, 0, 64, |t| *t = Techniques::default());
        render_notes(&mut notes, &mut midi, &[(0, 64)], before.as_ref());
        assert!((notes[0].velocity - 0.8).abs() < 1e-6);
        assert!(midi.tab.as_ref().unwrap().notes[0].plain.is_none());
    }

    #[test]
    fn a_velocity_edited_by_hand_becomes_the_new_plain_value() {
        let (mut notes, mut midi) = clip();
        let before = midi.tab.clone();
        set_tech(&mut midi, 0, 64, |t| t.flags.insert(TechFlags::GHOST));
        render_notes(&mut notes, &mut midi, &[(0, 64)], before.as_ref());
        notes[0].velocity = 0.5; // the user drags the velocity in the piano roll
        let before = midi.tab.clone();
        set_tech(&mut midi, 0, 64, |t| t.flags.insert(TechFlags::ACCENT));
        render_notes(&mut notes, &mut midi, &[(0, 64)], before.as_ref());
        assert!((notes[0].velocity - 0.5 * 0.6 * 1.2).abs() < 1e-5);
    }

    #[test]
    fn curves_follow_the_techniques_and_leave_user_curves_alone() {
        let (mut notes, mut midi) = clip();
        // The user drew pressure on the third note; it has no techniques.
        let mut drawn = NoteExpression::new(2 * PPQ, 67);
        drawn.pressure = vec![ExpressionPoint::new(0, 0.3)];
        midi.expression.push(drawn.clone());
        let before = midi.tab.clone();
        set_tech(&mut midi, 0, 64, |t| t.bend = Some(Bend::up(200)));
        render_notes(&mut notes, &mut midi, &[(0, 64)], before.as_ref());
        assert_eq!(midi.expression.len(), 2);
        assert!(midi.expression[0].pitch.len() >= 2);
        assert_eq!(midi.expression[1], drawn);
        // Removing the bend removes its curve; the other note's entry stays.
        let before = midi.tab.clone();
        set_tech(&mut midi, 0, 64, |t| t.bend = None);
        render_notes(&mut notes, &mut midi, &[(0, 64)], before.as_ref());
        assert_eq!(midi.expression, vec![drawn]);
    }

    #[test]
    fn a_let_ring_note_follows_the_next_note_on_its_string_and_the_one_before_updates() {
        let (mut notes, mut midi) = clip();
        let before = midi.tab.clone();
        set_tech(&mut midi, 0, 64, |t| t.flags.insert(TechFlags::LET_RING));
        render_notes(&mut notes, &mut midi, &[(0, 64)], before.as_ref());
        assert_eq!(notes[0].length, PPQ, "to the next note on the string");
        // Remove the next note's entry and note: the ringing note re-renders as touched via
        // its predecessor relation.
        let before = midi.tab.clone();
        midi.tab.as_mut().unwrap().notes.remove(1);
        notes.remove(1);
        render_notes(&mut notes, &mut midi, &[(PPQ, 64)], before.as_ref());
        assert_eq!(notes[0].length, LET_RING_TAIL);
    }

    #[test]
    fn render_all_is_idempotent_and_unrender_restores() {
        let (mut notes, mut midi) = clip();
        set_tech(&mut midi, 0, 64, |t| {
            t.flags.insert(TechFlags::GHOST);
            t.flags.insert(TechFlags::PALM_MUTE);
        });
        set_tech(&mut midi, 2 * PPQ, 67, |t| {
            t.vibrato = Some(VibratoKind::Normal)
        });
        render_all(&mut notes, &mut midi);
        let once = (notes.clone(), midi.clone());
        render_all(&mut notes, &mut midi);
        assert_eq!((notes.clone(), midi.clone()), once);
        unrender(&mut notes, &mut midi, &[(0, 64), (2 * PPQ, 67)]);
        assert!((notes[0].velocity - 0.8).abs() < 1e-6);
        assert!(midi.expression.is_empty());
    }
}
