//! Scale and chord edits of the piano roll, as pure functions on a note list: conform to a
//! scale, move by scale degrees, the chord stamp, chordify, arpeggiate and strum. Every edit
//! takes the clip's notes and the selected indices and returns the whole new list (sorted,
//! free of notes that share a start and a key) and the indices of the notes it made, so the
//! view sends one `SetClipContent` and a gesture is one undo step. The music theory itself
//! (scales, degrees, chords) is `atomik_model::theory`.
//!
//! Conventions, also documented where they are used:
//! * **Conform** moves a note to the nearest in-scale key; a tie goes down.
//! * A **group** is the selected notes that start on the same tick; arpeggio and strum need at
//!   least two notes in a group and leave single notes alone.
//! * **Arpeggio** steps start on the group's start and fill its original length (the longest
//!   note); the last step is cut at the end. **As played** equals Up: the model keeps no entry
//!   order, so low to high is the only order a chord has.
//! * **Strum** delays the notes of a group one after the other and keeps every note's end where
//!   it was, so the delayed notes get shorter (a note is never shorter than one tick).

use crate::seq_edit::{MAX_KEY, clamp_note};
use atomik_model::Note;
use atomik_model::theory::{ChordKind, DiatonicSize, Scale, chord_keys, diatonic_chord};
use std::collections::{BTreeMap, BTreeSet};

/// The most notes one arpeggiated group turns into: a finer step is coarsened to fit.
pub const MAX_ARP_NOTES: i64 = 4096;

/// What the pencil drops and chordify builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stamp {
    /// A single note.
    #[default]
    None,
    /// A chord of a fixed shape on the key.
    Chord(ChordKind),
    /// A chord of scale tones built from the key's scale degree (needs a scale).
    Diatonic(DiatonicSize),
}

impl Stamp {
    /// Every stamp, in menu order.
    pub fn all() -> Vec<Stamp> {
        let mut v = vec![Stamp::None];
        v.extend(ChordKind::ALL.into_iter().map(Stamp::Chord));
        v.extend(DiatonicSize::ALL.into_iter().map(Stamp::Diatonic));
        v
    }

    /// The stable name used in command arguments: `none`, a chord id, `diatonic_triad`, ...
    pub fn id(self) -> &'static str {
        match self {
            Stamp::None => "none",
            Stamp::Chord(c) => c.id(),
            Stamp::Diatonic(DiatonicSize::Triad) => "diatonic_triad",
            Stamp::Diatonic(DiatonicSize::Seventh) => "diatonic_seventh",
            Stamp::Diatonic(DiatonicSize::Ninth) => "diatonic_ninth",
        }
    }

    /// The stamp named `id`.
    pub fn from_id(id: &str) -> Option<Stamp> {
        Stamp::all().into_iter().find(|s| s.id() == id)
    }

    /// The name shown in menus.
    pub fn label(self) -> &'static str {
        match self {
            Stamp::None => "None",
            Stamp::Chord(c) => c.label(),
            Stamp::Diatonic(DiatonicSize::Triad) => "Diatonic Triad",
            Stamp::Diatonic(DiatonicSize::Seventh) => "Diatonic 7th",
            Stamp::Diatonic(DiatonicSize::Ninth) => "Diatonic 9th",
        }
    }

    /// A short name for the toolbar.
    pub fn short(self) -> &'static str {
        match self {
            Stamp::None => "None",
            Stamp::Chord(c) => c.short(),
            Stamp::Diatonic(DiatonicSize::Triad) => "Dia 3",
            Stamp::Diatonic(DiatonicSize::Seventh) => "Dia 7",
            Stamp::Diatonic(DiatonicSize::Ninth) => "Dia 9",
        }
    }

    /// Whether the stamp builds a chord with this `scale`: a diatonic stamp needs a scale.
    pub fn is_active(self, scale: Option<Scale>) -> bool {
        match self {
            Stamp::None => false,
            Stamp::Chord(_) => true,
            Stamp::Diatonic(_) => scale.is_some(),
        }
    }

    /// The keys of the chord on `key`, ascending, in the given inversion; empty when the
    /// stamp is not [active](Self::is_active). A diatonic chord first moves `key` to the
    /// nearest in-scale key (ties down).
    pub fn keys(self, key: u8, scale: Option<Scale>, inversion: u8) -> Vec<u8> {
        match (self, scale) {
            (Stamp::Chord(c), _) => chord_keys(key, c, inversion),
            (Stamp::Diatonic(size), Some(s)) => diatonic_chord(s, key, size, inversion),
            _ => Vec::new(),
        }
    }
}

/// The order of an arpeggio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArpPattern {
    /// Low to high, then again from the bottom.
    #[default]
    Up,
    /// High to low.
    Down,
    /// Up, then down, without repeating the top and bottom notes.
    UpDown,
    /// Down, then up, without repeating the bottom and top notes.
    DownUp,
    /// Random keys of the chord, the same on every run (seeded by the chord's start tick).
    Random,
    /// The order the chord has in the clip: low to high (see the module documentation).
    AsPlayed,
}

impl ArpPattern {
    /// Every pattern, in menu order.
    pub const ALL: [ArpPattern; 6] = [
        ArpPattern::Up,
        ArpPattern::Down,
        ArpPattern::UpDown,
        ArpPattern::DownUp,
        ArpPattern::Random,
        ArpPattern::AsPlayed,
    ];

    /// The stable name used in command arguments.
    pub fn id(self) -> &'static str {
        match self {
            ArpPattern::Up => "up",
            ArpPattern::Down => "down",
            ArpPattern::UpDown => "up_down",
            ArpPattern::DownUp => "down_up",
            ArpPattern::Random => "random",
            ArpPattern::AsPlayed => "as_played",
        }
    }

    /// The name shown in menus.
    pub fn label(self) -> &'static str {
        match self {
            ArpPattern::Up => "Up",
            ArpPattern::Down => "Down",
            ArpPattern::UpDown => "Up-Down",
            ArpPattern::DownUp => "Down-Up",
            ArpPattern::Random => "Random",
            ArpPattern::AsPlayed => "As Played",
        }
    }

    /// The pattern named `id`.
    pub fn from_id(id: &str) -> Option<ArpPattern> {
        ArpPattern::ALL.into_iter().find(|p| p.id() == id)
    }
}

/// The settings of [`arpeggiate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArpOptions {
    /// The order.
    pub pattern: ArpPattern,
    /// Each note's length as a percentage of the step, `1..=100`.
    pub gate_percent: u8,
    /// How many octaves the arpeggio climbs, `1..=3` (the chord repeats an octave higher).
    pub octaves: u8,
}

impl Default for ArpOptions {
    fn default() -> Self {
        Self {
            pattern: ArpPattern::Up,
            gate_percent: 100,
            octaves: 1,
        }
    }
}

/// The gate choices of the menus.
pub const ARP_GATES: [u8; 4] = [25, 50, 75, 100];
/// The octave choices of the menus.
pub const ARP_OCTAVES: [u8; 3] = [1, 2, 3];

/// Which end of a chord a strum starts at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StrumDirection {
    /// The lowest note first.
    #[default]
    Up,
    /// The highest note first.
    Down,
}

impl StrumDirection {
    /// The stable name used in command arguments.
    pub fn id(self) -> &'static str {
        match self {
            StrumDirection::Up => "up",
            StrumDirection::Down => "down",
        }
    }

    /// The name shown in menus.
    pub fn label(self) -> &'static str {
        match self {
            StrumDirection::Up => "Up (Low to High)",
            StrumDirection::Down => "Down (High to Low)",
        }
    }

    /// The direction named `id`.
    pub fn from_id(id: &str) -> Option<StrumDirection> {
        [StrumDirection::Up, StrumDirection::Down]
            .into_iter()
            .find(|d| d.id() == id)
    }
}

/// The delay between two notes of a strum, as the denominator of a note value: the argument of
/// `editor.set_strum_amount` and the number of the menu label (`128` is a 1/128 note).
pub const STRUM_AMOUNTS: [u16; 4] = [128, 64, 32, 16];

/// The ticks of a `1/denominator` note.
pub fn strum_ticks(denominator: u16) -> i64 {
    atomik_core::PPQ * 4 / i64::from(denominator.max(1))
}

/// The settings of [`strum`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrumOptions {
    /// Delay per note, in ticks (> 0).
    pub ticks: i64,
    /// Which end starts.
    pub direction: StrumDirection,
}

impl Default for StrumOptions {
    fn default() -> Self {
        Self {
            ticks: strum_ticks(64),
            direction: StrumDirection::Up,
        }
    }
}

/// The notes sorted by `(tick, key)` with at most one note per start and key (the longer one
/// stays), clamped to valid values; the returned indices are those of the notes that
/// came from a `tracked` index.
pub fn unique(notes: Vec<Note>, tracked: &BTreeSet<usize>) -> (Vec<Note>, BTreeSet<usize>) {
    let mut tagged: Vec<(Note, bool)> = notes
        .into_iter()
        .enumerate()
        .map(|(i, n)| (clamp_note(n), tracked.contains(&i)))
        .collect();
    tagged.sort_by_key(|(n, _)| (n.tick, n.key, n.length));
    let mut out: Vec<(Note, bool)> = Vec::with_capacity(tagged.len());
    for (n, t) in tagged {
        match out.last_mut() {
            Some((last, lt)) if (last.tick, last.key) == (n.tick, n.key) => {
                // Sorted by length: this one is at least as long.
                *last = n;
                *lt |= t;
            }
            _ => out.push((n, t)),
        }
    }
    let sel = out
        .iter()
        .enumerate()
        .filter(|(_, (_, t))| *t)
        .map(|(i, _)| i)
        .collect();
    (out.into_iter().map(|(n, _)| n).collect(), sel)
}

/// Moves every selected note to the nearest in-scale key (a note that is in the scale stays;
/// a tie goes down).
pub fn conform(notes: &[Note], sel: &BTreeSet<usize>, scale: Scale) -> Vec<Note> {
    notes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            if sel.contains(&i) {
                Note {
                    key: scale.nearest(n.key),
                    ..*n
                }
            } else {
                *n
            }
        })
        .collect()
}

/// Moves the selected notes `degrees` scale degrees (up when positive). If any note would leave
/// the keyboard the distance is shortened until every note fits, so the group keeps its shape.
pub fn move_degrees(
    notes: &[Note],
    sel: &BTreeSet<usize>,
    degrees: i32,
    scale: Scale,
) -> Vec<Note> {
    let fits = |d: i32| {
        notes
            .iter()
            .enumerate()
            .filter(|(i, _)| sel.contains(i))
            .all(|(_, n)| scale.step(n.key, d).is_some())
    };
    let mut d = degrees;
    while d != 0 && !fits(d) {
        d -= d.signum();
    }
    notes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            if sel.contains(&i) {
                Note {
                    key: scale.step(n.key, d).unwrap_or(n.key),
                    ..*n
                }
            } else {
                *n
            }
        })
        .collect()
}

/// Replaces each selected note by the chord of `stamp` on its key: the same start, length,
/// velocity and mute. Unchanged (`None`) when the stamp is not active with `scale`.
pub fn chordify(
    notes: &[Note],
    sel: &BTreeSet<usize>,
    stamp: Stamp,
    inversion: u8,
    scale: Option<Scale>,
) -> Option<(Vec<Note>, BTreeSet<usize>)> {
    if !stamp.is_active(scale) || sel.is_empty() {
        return None;
    }
    let mut out: Vec<Note> = Vec::with_capacity(notes.len());
    let mut tracked = BTreeSet::new();
    for (i, n) in notes.iter().enumerate() {
        if !sel.contains(&i) {
            out.push(*n);
            continue;
        }
        for key in stamp.keys(n.key, scale, inversion) {
            tracked.insert(out.len());
            out.push(Note { key, ..*n });
        }
    }
    Some(unique(out, &tracked))
}

/// The chords of a selection: the selected indices of each start tick, for the ticks with at
/// least two notes.
fn groups(notes: &[Note], sel: &BTreeSet<usize>) -> BTreeMap<i64, Vec<usize>> {
    let mut by_tick: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for i in sel {
        if let Some(n) = notes.get(*i) {
            by_tick.entry(n.tick).or_default().push(*i);
        }
    }
    by_tick.retain(|_, v| v.len() >= 2);
    by_tick
}

/// The notes with the selected indices of `grouped` removed.
fn without(notes: &[Note], grouped: &BTreeMap<i64, Vec<usize>>) -> Vec<Note> {
    let gone: BTreeSet<usize> = grouped.values().flatten().copied().collect();
    notes
        .iter()
        .enumerate()
        .filter(|(i, _)| !gone.contains(i))
        .map(|(_, n)| *n)
        .collect()
}

/// A small deterministic generator (splitmix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// The indices into an ascending chord of `m` notes that an arpeggio plays, one per step.
fn pattern_order(pattern: ArpPattern, m: usize, steps: usize, seed: u64) -> Vec<usize> {
    let cycle: Vec<usize> = match pattern {
        ArpPattern::Up | ArpPattern::AsPlayed => (0..m).collect(),
        ArpPattern::Down => (0..m).rev().collect(),
        ArpPattern::UpDown => (0..m).chain((1..m.saturating_sub(1)).rev()).collect(),
        ArpPattern::DownUp => (0..m).rev().chain(1..m.saturating_sub(1)).collect(),
        ArpPattern::Random => {
            let mut rng = Rng(seed);
            let mut last = usize::MAX;
            return (0..steps)
                .map(|_| {
                    let mut i = (rng.next() % m as u64) as usize;
                    if m > 1 && i == last {
                        i = (i + 1) % m;
                    }
                    last = i;
                    i
                })
                .collect();
        }
    };
    (0..steps).map(|s| cycle[s % cycle.len()]).collect()
}

/// Turns each group of simultaneous selected notes into an arpeggio over the group's original
/// length: one note per `step` ticks, `options.gate_percent` of the step long (the last one cut
/// at the end), through the chord's keys (`options.octaves` octaves of it) in the order of
/// `options.pattern`. A note keeps the velocity and mute of the chord note it plays.
/// `None` when no group has two notes.
pub fn arpeggiate(
    notes: &[Note],
    sel: &BTreeSet<usize>,
    options: ArpOptions,
    step: i64,
) -> Option<(Vec<Note>, BTreeSet<usize>)> {
    let grouped = groups(notes, sel);
    if grouped.is_empty() {
        return None;
    }
    let mut out = without(notes, &grouped);
    let first_new = out.len();
    let octaves = i32::from(options.octaves.clamp(1, 3));
    let gate = i64::from(options.gate_percent.clamp(1, 100));
    for (tick, idxs) in &grouped {
        let mut chord: Vec<Note> = idxs.iter().map(|i| notes[*i]).collect();
        chord.sort_by_key(|n| n.key);
        let end = chord
            .iter()
            .map(|n| n.tick + n.length)
            .max()
            .unwrap_or(*tick);
        let mut tones: Vec<Note> = Vec::new();
        for o in 0..octaves {
            for n in &chord {
                let key = i32::from(n.key) + 12 * o;
                if key <= i32::from(MAX_KEY) {
                    tones.push(Note {
                        key: key as u8,
                        ..*n
                    });
                }
            }
        }
        let span = (end - tick).max(1);
        let step = step.max(1).max((span + MAX_ARP_NOTES - 1) / MAX_ARP_NOTES);
        let steps = ((span + step - 1) / step) as usize;
        let seed = (*tick as u64) ^ ((tones.len() as u64) << 48);
        let order = pattern_order(options.pattern, tones.len(), steps, seed);
        for (s, t) in order.into_iter().enumerate() {
            let start = tick + s as i64 * step;
            let length = (step * gate / 100).max(1).min(end - start);
            out.push(Note {
                tick: start,
                length,
                ..tones[t]
            });
        }
    }
    let tracked: BTreeSet<usize> = (first_new..out.len()).collect();
    Some(unique(out, &tracked))
}

/// Delays the notes of each group of simultaneous selected notes by `options.ticks` per note,
/// from the lowest (`Up`) or the highest (`Down`), keeping every note's end. A delay that would
/// leave less than a tick of the note stops one tick before its end. `None` when no group has
/// two notes.
pub fn strum(
    notes: &[Note],
    sel: &BTreeSet<usize>,
    options: StrumOptions,
) -> Option<(Vec<Note>, BTreeSet<usize>)> {
    let grouped = groups(notes, sel);
    if grouped.is_empty() {
        return None;
    }
    let ticks = options.ticks.max(1);
    let mut out = notes.to_vec();
    let mut tracked = BTreeSet::new();
    for idxs in grouped.values() {
        let mut order = idxs.clone();
        order.sort_by_key(|i| notes[*i].key);
        if options.direction == StrumDirection::Down {
            order.reverse();
        }
        for (rank, i) in order.into_iter().enumerate() {
            let n = notes[i];
            let end = n.tick + n.length;
            let start = (n.tick + rank as i64 * ticks).min(end - 1);
            out[i] = Note {
                tick: start,
                length: end - start,
                ..n
            };
            tracked.insert(i);
        }
    }
    Some(unique(out, &tracked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_model::theory::ScaleKind;

    const Q: i64 = atomik_core::PPQ;

    fn n(tick: i64, length: i64, key: u8) -> Note {
        Note::new(tick, length, key, 0.8)
    }

    fn set(i: &[usize]) -> BTreeSet<usize> {
        i.iter().copied().collect()
    }

    fn c_major() -> Scale {
        Scale::new(0, ScaleKind::Major)
    }

    fn keys(notes: &[Note]) -> Vec<u8> {
        notes.iter().map(|n| n.key).collect()
    }

    #[test]
    fn unique_sorts_and_keeps_the_longer_of_two_notes_on_the_same_start_and_key() {
        let notes = vec![n(10, 5, 60), n(0, 5, 64), n(10, 9, 60), n(10, 7, 62)];
        let (list, sel) = unique(notes, &set(&[0, 1]));
        assert_eq!(list, vec![n(0, 5, 64), n(10, 9, 60), n(10, 7, 62)]);
        assert_eq!(
            sel,
            set(&[0, 1]),
            "the tracked notes: 64 at 0 and the merged 60"
        );
    }

    #[test]
    fn conform_moves_selected_notes_to_the_nearest_scale_key_ties_down() {
        let notes = vec![
            n(0, 1, 60),
            n(0, 1, 61),
            n(0, 1, 63),
            n(0, 1, 66),
            n(0, 1, 70),
        ];
        let out = conform(&notes, &set(&[1, 2, 3, 4]), c_major());
        assert_eq!(
            keys(&out),
            [60, 60, 62, 65, 69],
            "C# -> C, D# -> D, F# -> F, A# -> A"
        );
        // Unselected notes stay, even out of the scale.
        let out = conform(&notes, &set(&[0]), c_major());
        assert_eq!(out, notes);
    }

    #[test]
    fn move_degrees_walks_each_note_along_the_scale_and_keeps_out_of_scale_notes_ordered() {
        let notes = vec![n(0, 1, 60), n(0, 1, 64), n(0, 1, 67), n(0, 1, 61)];
        let up = move_degrees(&notes, &set(&[0, 1, 2]), 1, c_major());
        assert_eq!(keys(&up), [62, 65, 69, 61]);
        let down = move_degrees(&notes, &set(&[0, 1, 2]), -2, c_major());
        assert_eq!(keys(&down), [57, 60, 64, 61]);
        let octave = move_degrees(&notes, &set(&[0, 1, 2]), 7, c_major());
        assert_eq!(keys(&octave), [72, 76, 79, 61]);
        // An out-of-scale note's first step lands on the next scale key in that direction.
        let odd = move_degrees(&notes, &set(&[3]), 1, c_major());
        assert_eq!(odd[3].key, 62);
        let odd = move_degrees(&notes, &set(&[3]), -1, c_major());
        assert_eq!(odd[3].key, 60);
    }

    #[test]
    fn move_degrees_at_the_edge_shortens_the_move_so_the_group_keeps_its_shape() {
        let notes = vec![n(0, 1, 125), n(0, 1, 127)];
        // F then G: one degree up would take G off the keyboard, so nothing moves.
        let out = move_degrees(&notes, &set(&[0, 1]), 1, c_major());
        assert_eq!(keys(&out), [125, 127]);
        let low = vec![n(0, 1, 0), n(0, 1, 4)];
        let out = move_degrees(&low, &set(&[0, 1]), -3, c_major());
        assert_eq!(keys(&out), [0, 4]);
        let mixed = vec![n(0, 1, 2), n(0, 1, 60)];
        let out = move_degrees(&mixed, &set(&[0, 1]), -5, c_major());
        assert_eq!(
            keys(&out),
            [0, 59],
            "cut to one degree: D can only go down to C"
        );
    }

    #[test]
    fn stamps_name_themselves_and_build_chords_only_when_active() {
        for s in Stamp::all() {
            assert_eq!(Stamp::from_id(s.id()), Some(s), "{}", s.id());
            assert!(!s.label().is_empty() && !s.short().is_empty());
        }
        assert_eq!(Stamp::all().len(), 1 + 21 + 3);
        assert_eq!(Stamp::from_id("nope"), None);
        let major = Stamp::Chord(ChordKind::Major);
        assert_eq!(major.keys(60, None, 0), [60, 64, 67]);
        assert_eq!(major.keys(60, None, 1), [64, 67, 72]);
        assert!(major.is_active(None));
        assert!(Stamp::None.keys(60, None, 0).is_empty());
        assert!(!Stamp::None.is_active(Some(c_major())));
        let dia = Stamp::Diatonic(DiatonicSize::Triad);
        assert!(!dia.is_active(None));
        assert!(dia.keys(62, None, 0).is_empty());
        assert!(dia.is_active(Some(c_major())));
        assert_eq!(dia.keys(62, Some(c_major()), 0), [62, 65, 69]);
        assert_eq!(
            Stamp::Diatonic(DiatonicSize::Seventh).keys(67, Some(c_major()), 0),
            [67, 71, 74, 77]
        );
    }

    #[test]
    fn chordify_turns_each_selected_note_into_the_chord_keeping_its_timing() {
        let notes = vec![
            Note {
                velocity: 0.5,
                muted: true,
                ..n(0, Q, 60)
            },
            n(Q, Q / 2, 62),
            n(2 * Q, Q, 65),
        ];
        let (out, sel) = chordify(
            &notes,
            &set(&[0, 1]),
            Stamp::Chord(ChordKind::Minor),
            0,
            None,
        )
        .unwrap();
        assert_eq!(
            out.iter()
                .map(|x| (x.tick, x.length, x.key))
                .collect::<Vec<_>>(),
            [
                (0, Q, 60),
                (0, Q, 63),
                (0, Q, 67),
                (Q, Q / 2, 62),
                (Q, Q / 2, 65),
                (Q, Q / 2, 69),
                (2 * Q, Q, 65)
            ]
        );
        assert!(out[0].muted && out[2].muted && (out[1].velocity - 0.5).abs() < 1e-6);
        assert_eq!(sel, set(&[0, 1, 2, 3, 4, 5]));
    }

    #[test]
    fn chordify_with_a_diatonic_stamp_follows_the_scale_and_the_inversion() {
        let notes = vec![n(0, Q, 60), n(Q, Q, 62), n(2 * Q, Q, 71)];
        let (out, _) = chordify(
            &notes,
            &set(&[0, 1, 2]),
            Stamp::Diatonic(DiatonicSize::Triad),
            0,
            Some(c_major()),
        )
        .unwrap();
        assert_eq!(
            keys(&out),
            [60, 64, 67, 62, 65, 69, 71, 74, 77],
            "I, ii, vii dim"
        );
        let (out, _) = chordify(
            &notes,
            &set(&[0]),
            Stamp::Diatonic(DiatonicSize::Triad),
            1,
            Some(c_major()),
        )
        .unwrap();
        assert_eq!(keys(&out)[..3], [64, 67, 72]);
    }

    #[test]
    fn chordify_does_nothing_without_an_active_stamp_or_a_selection() {
        let notes = vec![n(0, Q, 60)];
        assert!(chordify(&notes, &set(&[0]), Stamp::None, 0, None).is_none());
        assert!(
            chordify(
                &notes,
                &set(&[0]),
                Stamp::Diatonic(DiatonicSize::Triad),
                0,
                None
            )
            .is_none()
        );
        assert!(chordify(&notes, &set(&[]), Stamp::Chord(ChordKind::Major), 0, None).is_none());
    }

    #[test]
    fn chordify_merges_with_notes_that_are_already_there() {
        let notes = vec![n(0, Q, 60), n(0, 2 * Q, 64)];
        let (out, sel) =
            chordify(&notes, &set(&[0]), Stamp::Chord(ChordKind::Major), 0, None).unwrap();
        assert_eq!(out.len(), 3, "the existing E is not doubled");
        let e = out.iter().find(|x| x.key == 64).unwrap();
        assert_eq!(e.length, 2 * Q, "the longer one stays");
        assert_eq!(sel.len(), 3);
    }

    fn triad(tick: i64, length: i64) -> Vec<Note> {
        vec![
            n(tick, length, 60),
            n(tick, length, 64),
            n(tick, length, 67),
        ]
    }

    fn arp(
        notes: &[Note],
        pattern: ArpPattern,
        gate: u8,
        octaves: u8,
        step: i64,
    ) -> Vec<(i64, i64, u8)> {
        let all: BTreeSet<usize> = (0..notes.len()).collect();
        let (out, _) = arpeggiate(
            notes,
            &all,
            ArpOptions {
                pattern,
                gate_percent: gate,
                octaves,
            },
            step,
        )
        .unwrap();
        out.iter().map(|x| (x.tick, x.length, x.key)).collect()
    }

    #[test]
    fn arpeggio_up_fills_the_chord_length_with_steps_of_the_snap() {
        let s = Q / 4; // a sixteenth
        let got = arp(&triad(0, Q), ArpPattern::Up, 100, 1, s);
        assert_eq!(
            got,
            [(0, s, 60), (s, s, 64), (2 * s, s, 67), (3 * s, s, 60)]
        );
    }

    #[test]
    fn arpeggio_down_updown_downup_and_as_played() {
        let s = Q / 4;
        let len = 6 * s;
        let key = |p| {
            arp(&triad(0, len), p, 100, 1, s)
                .iter()
                .map(|x| x.2)
                .collect::<Vec<_>>()
        };
        assert_eq!(key(ArpPattern::Down), [67, 64, 60, 67, 64, 60]);
        assert_eq!(key(ArpPattern::Up), [60, 64, 67, 60, 64, 67]);
        assert_eq!(key(ArpPattern::UpDown), [60, 64, 67, 64, 60, 64]);
        assert_eq!(key(ArpPattern::DownUp), [67, 64, 60, 64, 67, 64]);
        assert_eq!(key(ArpPattern::AsPlayed), key(ArpPattern::Up));
        // Two notes: up-down is just alternation.
        let two = vec![n(0, 4 * s, 60), n(0, 4 * s, 67)];
        let k: Vec<u8> = arp(&two, ArpPattern::UpDown, 100, 1, s)
            .iter()
            .map(|x| x.2)
            .collect();
        assert_eq!(k, [60, 67, 60, 67]);
    }

    #[test]
    fn arpeggio_octaves_extend_the_chord_upwards_and_drop_keys_above_127() {
        let s = Q / 4;
        let k = |o| {
            arp(&triad(0, 9 * s), ArpPattern::Up, 100, o, s)
                .iter()
                .map(|x| x.2)
                .collect::<Vec<_>>()
        };
        assert_eq!(k(2), [60, 64, 67, 72, 76, 79, 60, 64, 67]);
        assert_eq!(k(3)[..9], [60, 64, 67, 72, 76, 79, 84, 88, 91]);
        let high = vec![n(0, 8 * s, 120), n(0, 8 * s, 127)];
        let k: Vec<u8> = arp(&high, ArpPattern::Up, 100, 3, s)
            .iter()
            .map(|x| x.2)
            .collect();
        assert_eq!(k[..2], [120, 127], "the octaves above 127 do not exist");
    }

    #[test]
    fn arpeggio_gate_shortens_the_notes_and_the_last_step_is_cut_at_the_end() {
        let s = Q / 4;
        let got = arp(&triad(0, s * 5 / 2), ArpPattern::Up, 50, 1, s);
        assert_eq!(got, [(0, s / 2, 60), (s, s / 2, 64), (2 * s, s / 2, 67)]);
        let full = arp(&triad(0, s * 5 / 2), ArpPattern::Up, 100, 1, s);
        assert_eq!(full[2], (2 * s, s / 2, 67), "cut at the chord's end");
        // A chord shorter than a step is one step.
        let short = arp(&triad(0, s / 4), ArpPattern::Up, 100, 1, s);
        assert_eq!(short, [(0, s / 4, 60)]);
    }

    #[test]
    fn arpeggio_works_with_triplet_steps_and_offsets_the_group_start() {
        let step = Q / 3;
        let got = arp(&triad(Q, Q), ArpPattern::Up, 100, 1, step);
        assert_eq!(
            got,
            [
                (Q, step, 60),
                (Q + step, step, 64),
                (Q + 2 * step, step, 67)
            ]
        );
    }

    #[test]
    fn arpeggio_random_is_the_same_every_time_and_a_pure_function_of_the_chord() {
        let s = Q / 4;
        let a = arp(&triad(0, 16 * s), ArpPattern::Random, 100, 1, s);
        let b = arp(&triad(0, 16 * s), ArpPattern::Random, 100, 1, s);
        assert_eq!(a, b);
        let keys: BTreeSet<u8> = a.iter().map(|x| x.2).collect();
        assert!(keys.is_subset(&[60u8, 64, 67].into_iter().collect()));
        assert!(keys.len() > 1, "not one note over and over: {a:?}");
        assert!(
            a.windows(2).all(|w| w[0].2 != w[1].2),
            "no immediate repeats: {a:?}"
        );
        let other = arp(&triad(Q, 16 * s), ArpPattern::Random, 100, 1, s);
        assert_eq!(a.len(), other.len());
        // Pinned: a change of the generator would change every arpeggio made from now on.
        assert_eq!(
            a.iter().map(|x| x.2).take(8).collect::<Vec<_>>(),
            [64, 67, 60, 64, 67, 60, 67, 60],
        );
    }

    #[test]
    fn arpeggio_keeps_velocities_per_key_leaves_other_notes_and_single_notes_alone() {
        let s = Q / 4;
        let notes = vec![
            Note {
                velocity: 0.3,
                ..n(0, 4 * s, 60)
            },
            Note {
                velocity: 0.9,
                ..n(0, 4 * s, 64)
            },
            n(0, 4 * s, 100), // not selected
            n(8 * s, s, 50),  // a single selected note
        ];
        let (out, sel) = arpeggiate(&notes, &set(&[0, 1, 3]), ArpOptions::default(), s).unwrap();
        let v = |k: u8, t: i64| {
            out.iter()
                .find(|x| x.key == k && x.tick == t)
                .map(|x| x.velocity)
        };
        assert_eq!(v(60, 0), Some(0.3));
        assert_eq!(v(64, s), Some(0.9));
        assert!(out.contains(&n(0, 4 * s, 100)), "unselected note untouched");
        assert!(
            out.contains(&n(8 * s, s, 50)),
            "a single note is not a chord"
        );
        assert_eq!(sel.len(), 4, "the four arpeggio notes are selected");
        assert!(arpeggiate(&notes, &set(&[3]), ArpOptions::default(), s).is_none());
        assert!(arpeggiate(&notes, &set(&[]), ArpOptions::default(), s).is_none());
    }

    #[test]
    fn arpeggio_groups_are_the_notes_on_one_start_tick() {
        let s = Q / 4;
        let mut notes = triad(0, 4 * s);
        notes.extend(vec![n(4 * s, 4 * s, 62), n(4 * s, 4 * s, 65)]);
        let all: BTreeSet<usize> = (0..notes.len()).collect();
        let (out, _) = arpeggiate(&notes, &all, ArpOptions::default(), s).unwrap();
        assert_eq!(out.len(), 8);
        assert!(out[..4].iter().all(|x| x.tick < 4 * s));
        assert_eq!(keys(&out[4..]), [62, 65, 62, 65]);
    }

    #[test]
    fn a_runaway_step_is_coarsened_so_a_group_never_exceeds_the_cap() {
        let notes = triad(0, 64 * Q);
        let all: BTreeSet<usize> = (0..3).collect();
        let (out, _) = arpeggiate(&notes, &all, ArpOptions::default(), 1).unwrap();
        assert!(out.len() as i64 <= MAX_ARP_NOTES, "{}", out.len());
        assert!(out.len() as i64 > MAX_ARP_NOTES / 2);
        assert!(out.iter().all(|x| x.length >= 1));
    }

    #[test]
    fn strum_delays_each_note_and_keeps_the_ends_where_they_were() {
        let d = strum_ticks(64);
        assert_eq!(d, 960);
        let notes = triad(Q, Q);
        let (out, sel) = strum(
            &notes,
            &set(&[0, 1, 2]),
            StrumOptions {
                ticks: d,
                direction: StrumDirection::Up,
            },
        )
        .unwrap();
        assert_eq!(
            out.iter()
                .map(|x| (x.tick, x.length, x.key))
                .collect::<Vec<_>>(),
            [(Q, Q, 60), (Q + d, Q - d, 64), (Q + 2 * d, Q - 2 * d, 67)]
        );
        assert!(out.iter().all(|x| x.tick + x.length == 2 * Q));
        assert_eq!(sel, set(&[0, 1, 2]));
    }

    #[test]
    fn strum_down_starts_on_the_top_note() {
        let d = strum_ticks(32);
        let (out, _) = strum(
            &triad(0, Q),
            &set(&[0, 1, 2]),
            StrumOptions {
                ticks: d,
                direction: StrumDirection::Down,
            },
        )
        .unwrap();
        let by_key = |k| {
            out.iter()
                .find(|x| x.key == k)
                .map(|x| (x.tick, x.length))
                .unwrap()
        };
        assert_eq!(by_key(67), (0, Q));
        assert_eq!(by_key(64), (d, Q - d));
        assert_eq!(by_key(60), (2 * d, Q - 2 * d));
    }

    #[test]
    fn strum_never_leaves_less_than_a_tick_and_ignores_singles_and_the_unselected() {
        let notes = vec![n(0, 100, 60), n(0, 100, 64), n(0, 100, 67), n(500, 100, 50)];
        let (out, _) = strum(
            &notes,
            &set(&[0, 1, 2, 3]),
            StrumOptions {
                ticks: 80,
                direction: StrumDirection::Up,
            },
        )
        .unwrap();
        let c = out.iter().find(|x| x.key == 67).unwrap();
        assert_eq!(
            (c.tick, c.length),
            (99, 1),
            "the third note would start at 160: stopped at 99"
        );
        assert!(out.contains(&n(500, 100, 50)));
        let none = strum(&notes, &set(&[3]), StrumOptions::default());
        assert!(none.is_none());
        let unsel = strum(&notes, &set(&[0]), StrumOptions::default());
        assert!(
            unsel.is_none(),
            "one selected note of the chord is not a group"
        );
    }

    #[test]
    fn strum_amounts_are_note_values_of_a_whole_note() {
        assert_eq!(STRUM_AMOUNTS.map(strum_ticks), [480, 960, 1920, 3840]);
        assert_eq!(StrumOptions::default().ticks, 960);
    }

    #[test]
    fn names_round_trip() {
        for p in ArpPattern::ALL {
            assert_eq!(ArpPattern::from_id(p.id()), Some(p));
            assert!(!p.label().is_empty());
        }
        for d in [StrumDirection::Up, StrumDirection::Down] {
            assert_eq!(StrumDirection::from_id(d.id()), Some(d));
            assert!(!d.label().is_empty());
        }
        assert_eq!(ArpPattern::from_id("x"), None);
        assert_eq!(StrumDirection::from_id("x"), None);
    }
}
