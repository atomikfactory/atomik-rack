//! Fretting: choosing a string and fret for each pitch.
//!
//! The search is cost based. Every note has a handful of candidate positions (each string that
//! can sound the pitch). A chord picks one string per note, and its cost is made of the hand span
//! (hard limit 5 frets, 4 at low positions; beyond it a large penalty, never a refusal), a small
//! price for every fret (so low positions win), a larger one above the twelfth fret, a bonus for
//! open strings (weighted by [`FretOptions::prefer_open`]) and a penalty for strings skipped
//! inside a strum. A sequence is solved with a Viterbi pass over the chords in time order: the
//! transition cost is the movement of the hand position (the mean fretted fret), so a melody
//! stays in one position until moving is cheaper than reaching.
//!
//! * A note whose pitch no string can sound, or that finds no free string left in its chord, is
//!   reported as unplayable (`None` in the result and an entry in
//!   [`SequenceResult::unplayable`]); notes are never dropped silently.
//! * A **fixed** placement (a position the user or the file chose) is always kept as given, even
//!   when it does not sound the note's pitch; free notes only avoid its string.
//! * The search is bounded: at most [`FretOptions::beam`] candidate chords are kept per event and
//!   a node budget stops the enumeration of very large chords (12 strings) after the greedy
//!   solutions are found.

use crate::instrument::{Capo, Tuning};

/// A string and a fret (relative to the capo, 0 = open).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Placement {
    /// The string, in tab order (0 = top line).
    pub string: usize,
    /// The fret; 0 is the open string.
    pub fret: u8,
}

/// Tuning knobs of the search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FretOptions {
    /// How much open strings are preferred (0 = indifferent, 1 = the default, 2 = strongly).
    pub prefer_open: f32,
    /// The hard span limit in frets above the fourth fret.
    pub max_span: u8,
    /// The hard span limit at the low positions (the frets are wider there).
    pub low_span: u8,
    /// Candidate chords kept per event.
    pub beam: usize,
}

impl Default for FretOptions {
    fn default() -> Self {
        Self {
            prefer_open: 1.0,
            max_span: 5,
            low_span: 4,
            beam: 48,
        }
    }
}

/// A note of an event to fret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteSlot {
    /// The MIDI key to sound.
    pub key: u8,
    /// A position that must be kept as it is.
    pub fixed: Option<Placement>,
}

impl NoteSlot {
    /// A free note.
    pub fn new(key: u8) -> Self {
        Self { key, fixed: None }
    }
}

/// The notes that sound together (one chord, or one single note).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FretEvent {
    /// The notes.
    pub notes: Vec<NoteSlot>,
}

/// The solution of a sequence.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SequenceResult {
    /// For each event, a placement per note (`None` = unplayable).
    pub placements: Vec<Vec<Option<Placement>>>,
    /// `(event, note)` of every note that could not be placed.
    pub unplayable: Vec<(usize, usize)>,
}

/// Every position that sounds `key` on `tuning` with `capo`, lowest fret first.
pub fn candidates(tuning: &Tuning, capo: Capo, key: u8) -> Vec<Placement> {
    let mut out: Vec<Placement> = (0..tuning.len())
        .filter_map(|string| {
            tuning
                .fret_for_pitch(string, key, capo)
                .map(|fret| Placement { string, fret })
        })
        .collect();
    out.sort_by_key(|p| (p.fret, p.string));
    out
}

/// Frets one chord (or one note): a placement per key, in the order of `keys`.
///
/// `prev` is the previous hand position (a mean fret) when there is one.
pub fn fret_notes(
    tuning: &Tuning,
    capo: Capo,
    keys: &[u8],
    prev: Option<f32>,
    opts: &FretOptions,
) -> Vec<Option<Placement>> {
    let event = FretEvent {
        notes: keys.iter().map(|k| NoteSlot::new(*k)).collect(),
    };
    let mut r = fret_sequence(tuning, capo, &[event], prev, opts);
    r.placements.pop().unwrap_or_default()
}

/// Frets a whole sequence of events with the hand position carried through it.
pub fn fret_sequence(
    tuning: &Tuning,
    capo: Capo,
    events: &[FretEvent],
    start: Option<f32>,
    opts: &FretOptions,
) -> SequenceResult {
    let mut cands: Vec<Vec<Cand>> = Vec::with_capacity(events.len());
    for ev in events {
        cands.push(event_candidates(tuning, capo, ev, opts));
    }
    // Viterbi over the events: `dp[j]` is the best (cost, carried hand, back pointer) that ends
    // in candidate `j`.
    let mut dp: Vec<Vec<(f32, Option<f32>, usize)>> = Vec::with_capacity(events.len());
    for (t, list) in cands.iter().enumerate() {
        let mut row = Vec::with_capacity(list.len());
        for c in list {
            if t == 0 {
                row.push((c.cost + transition(start, c.hand), c.hand.or(start), 0));
                continue;
            }
            let mut best = (f32::INFINITY, None, 0usize);
            for (i, (pc, carried, _)) in dp[t - 1].iter().enumerate() {
                let total = pc + c.cost + transition(*carried, c.hand);
                if total < best.0 {
                    best = (total, c.hand.or(*carried), i);
                }
            }
            row.push(best);
        }
        dp.push(row);
    }
    let mut chosen = vec![0usize; events.len()];
    if let Some(last) = dp.last() {
        let mut best = 0usize;
        for (j, r) in last.iter().enumerate() {
            if r.0 < last[best].0 {
                best = j;
            }
        }
        let mut j = best;
        for t in (0..events.len()).rev() {
            chosen[t] = j;
            j = dp[t][j].2;
        }
    }
    let mut result = SequenceResult::default();
    for (t, ev) in events.iter().enumerate() {
        let placements = cands[t]
            .get(chosen[t])
            .map(|c| c.assign.clone())
            .unwrap_or_else(|| vec![None; ev.notes.len()]);
        for (n, p) in placements.iter().enumerate() {
            if p.is_none() {
                result.unplayable.push((t, n));
            }
        }
        result.placements.push(placements);
    }
    result
}

/// The mean fretted fret of a placement set, `None` when only open strings are used.
pub fn hand_position(placements: &[Option<Placement>]) -> Option<f32> {
    let fretted: Vec<f32> = placements
        .iter()
        .flatten()
        .filter(|p| p.fret > 0)
        .map(|p| f32::from(p.fret))
        .collect();
    (!fretted.is_empty()).then(|| fretted.iter().sum::<f32>() / fretted.len() as f32)
}

struct Cand {
    assign: Vec<Option<Placement>>,
    cost: f32,
    hand: Option<f32>,
}

fn transition(from: Option<f32>, to: Option<f32>) -> f32 {
    match (from, to) {
        (Some(a), Some(b)) => {
            let d = (a - b).abs();
            d + (d - 4.0).max(0.0) * 2.0
        }
        _ => 0.0,
    }
}

const SKIP_COST: f32 = 1000.0;
const NODE_BUDGET: usize = 40_000;

fn event_candidates(tuning: &Tuning, capo: Capo, ev: &FretEvent, opts: &FretOptions) -> Vec<Cand> {
    let n = ev.notes.len();
    if n == 0 {
        return vec![Cand {
            assign: Vec::new(),
            cost: 0.0,
            hand: None,
        }];
    }
    let mut assign: Vec<Option<Placement>> = vec![None; n];
    let mut used: u16 = 0;
    let mut order: Vec<usize> = Vec::with_capacity(n);
    // Fixed notes first: their strings are taken before the free notes choose.
    for (i, s) in ev.notes.iter().enumerate() {
        if let Some(p) = s.fixed {
            assign[i] = Some(p);
            if p.string < 16 {
                used |= 1 << p.string;
            }
        }
    }
    let mut options: Vec<Vec<Placement>> = vec![Vec::new(); n];
    for (i, s) in ev.notes.iter().enumerate() {
        if s.fixed.is_none() {
            options[i] = candidates(tuning, capo, s.key);
            order.push(i);
        }
    }
    // The notes with the fewest options choose first.
    order.sort_by_key(|i| (options[*i].len(), *i));
    let mut out = Vec::new();
    let mut nodes = 0usize;
    dfs(
        tuning,
        opts,
        &order,
        0,
        &options,
        &mut assign,
        used,
        &mut out,
        &mut nodes,
    );
    if out.is_empty() {
        // Nothing but skips (no note can be placed).
        out.push(finish(tuning, opts, &assign));
    }
    out.sort_by(|a, b| a.cost.total_cmp(&b.cost));
    out.dedup_by(|a, b| a.assign == b.assign);
    out.truncate(opts.beam.max(1));
    out
}

#[allow(clippy::too_many_arguments)]
fn dfs(
    tuning: &Tuning,
    opts: &FretOptions,
    order: &[usize],
    depth: usize,
    options: &[Vec<Placement>],
    assign: &mut Vec<Option<Placement>>,
    used: u16,
    out: &mut Vec<Cand>,
    nodes: &mut usize,
) {
    if depth == order.len() {
        out.push(finish(tuning, opts, assign));
        return;
    }
    *nodes += 1;
    if *nodes > NODE_BUDGET && !out.is_empty() {
        return;
    }
    let note = order[depth];
    let mut any = false;
    for p in &options[note] {
        if p.string < 16 && used & (1 << p.string) != 0 {
            continue;
        }
        any = true;
        assign[note] = Some(*p);
        let next_used = if p.string < 16 {
            used | (1 << p.string)
        } else {
            used
        };
        dfs(
            tuning,
            opts,
            order,
            depth + 1,
            options,
            assign,
            next_used,
            out,
            nodes,
        );
    }
    if !any {
        assign[note] = None;
        dfs(
            tuning,
            opts,
            order,
            depth + 1,
            options,
            assign,
            used,
            out,
            nodes,
        );
    }
    assign[note] = None;
}

fn finish(tuning: &Tuning, opts: &FretOptions, assign: &[Option<Placement>]) -> Cand {
    let placed: Vec<Placement> = assign.iter().flatten().copied().collect();
    let skipped = assign.len() - placed.len();
    let mut cost = skipped as f32 * SKIP_COST;
    let mut min_f = u8::MAX;
    let mut max_f = 0u8;
    for p in &placed {
        if p.fret == 0 {
            cost -= 1.5 * opts.prefer_open;
            continue;
        }
        min_f = min_f.min(p.fret);
        max_f = max_f.max(p.fret);
        cost += f32::from(p.fret) * 0.15;
        if p.fret > 12 {
            cost += f32::from(p.fret - 12) * 0.6;
        }
    }
    if min_f != u8::MAX {
        let span = max_f - min_f;
        cost += f32::from(span) * 3.0;
        let limit = if min_f <= 4 {
            opts.low_span
        } else {
            opts.max_span
        };
        if span > limit {
            cost += f32::from(span - limit) * 20.0;
        }
    }
    if placed.len() >= 3 {
        let lo = placed.iter().map(|p| p.string).min().unwrap_or(0);
        let hi = placed.iter().map(|p| p.string).max().unwrap_or(0);
        let gaps = (lo..=hi)
            .filter(|s| !placed.iter().any(|p| p.string == *s))
            .count();
        cost += gaps as f32 * 2.5;
    }
    let _ = tuning;
    Cand {
        assign: assign.to_vec(),
        cost,
        hand: hand_position(assign),
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
    fn open_strings_win_for_single_notes() {
        let t = guitar();
        let o = FretOptions::default();
        assert_eq!(
            fret_notes(&t, Capo::NONE, &[64], None, &o),
            vec![Some(Placement { string: 0, fret: 0 })]
        );
        assert_eq!(
            fret_notes(&t, Capo::NONE, &[40], None, &o),
            vec![Some(Placement { string: 5, fret: 0 })]
        );
    }

    #[test]
    fn open_e_chord_takes_the_open_shape() {
        let t = guitar();
        let keys = [40, 47, 52, 56, 59, 64];
        let p = fret_notes(&t, Capo::NONE, &keys, None, &FretOptions::default());
        let frets: Vec<u8> = {
            let mut v = vec![255u8; 6];
            for x in p.iter().flatten() {
                v[x.string] = x.fret;
            }
            v
        };
        // Tab order: top line first.
        assert_eq!(frets, vec![0, 0, 1, 2, 2, 0]);
    }

    #[test]
    fn unplayable_notes_are_reported_not_dropped() {
        let t = guitar();
        let ev = [
            FretEvent {
                notes: vec![NoteSlot::new(64)],
            },
            FretEvent {
                notes: vec![NoteSlot::new(20), NoteSlot::new(60)],
            },
            FretEvent {
                notes: vec![NoteSlot::new(120)],
            },
        ];
        let r = fret_sequence(&t, Capo::NONE, &ev, None, &FretOptions::default());
        assert_eq!(r.placements.len(), 3);
        assert_eq!(r.placements[1].len(), 2);
        assert_eq!(r.placements[1][0], None);
        assert!(r.placements[1][1].is_some());
        assert_eq!(r.unplayable, vec![(1, 0), (2, 0)]);
    }

    #[test]
    fn more_notes_than_strings_reports_the_extra() {
        let t = Tuning::from_opens(&[64, 59], 12);
        let r = fret_notes(&t, Capo::NONE, &[64, 59, 66], None, &FretOptions::default());
        assert_eq!(r.iter().filter(|p| p.is_none()).count(), 1);
    }

    #[test]
    fn fixed_placements_are_kept() {
        let t = guitar();
        let ev = [FretEvent {
            notes: vec![
                NoteSlot {
                    key: 64,
                    fixed: Some(Placement { string: 1, fret: 5 }),
                },
                NoteSlot::new(59),
            ],
        }];
        let r = fret_sequence(&t, Capo::NONE, &ev, None, &FretOptions::default());
        assert_eq!(r.placements[0][0], Some(Placement { string: 1, fret: 5 }));
        // The free note cannot use the fixed note's string: B3 moves to the G string.
        let p = r.placements[0][1].unwrap();
        assert_ne!(p.string, 1);
        assert_eq!(t.pitch(p.string, p.fret, Capo::NONE), Some(59));
    }

    #[test]
    fn a_melody_stays_in_position() {
        let t = guitar();
        // A made-up run in the open position.
        let keys = [57, 60, 62, 64, 62, 60, 57, 55, 57];
        let ev: Vec<FretEvent> = keys
            .iter()
            .map(|k| FretEvent {
                notes: vec![NoteSlot::new(*k)],
            })
            .collect();
        let r = fret_sequence(&t, Capo::NONE, &ev, None, &FretOptions::default());
        assert!(r.unplayable.is_empty());
        let frets: Vec<u8> = r.placements.iter().map(|p| p[0].unwrap().fret).collect();
        let fretted: Vec<u8> = frets.iter().copied().filter(|f| *f > 0).collect();
        let span = fretted.iter().max().unwrap() - fretted.iter().min().unwrap();
        assert!(span <= 5, "{frets:?}");
        for (k, p) in keys.iter().zip(&r.placements) {
            let p = p[0].unwrap();
            assert_eq!(t.pitch(p.string, p.fret, Capo::NONE), Some(*k));
        }
    }

    #[test]
    fn capo_shifts_the_frets() {
        let t = guitar();
        let o = FretOptions::default();
        // E4 with a capo at 2 is one fret below the capo fret of the top string: not playable
        // on the top string (it would be fret -2); the B string plays it at fret 5.
        let p = fret_notes(&t, Capo::at(2), &[64], None, &o)[0].unwrap();
        assert_eq!(t.pitch(p.string, p.fret, Capo::at(2)), Some(64));
        assert!(p.string >= 1);
        // F#4 is the open top string with the capo.
        let q = fret_notes(&t, Capo::at(2), &[66], None, &o)[0].unwrap();
        assert_eq!(q, Placement { string: 0, fret: 0 });
    }

    #[test]
    fn banjo_drone_is_open_or_high() {
        let t = find_tuning("banjo5", "open_g").unwrap().tuning.clone();
        let o = FretOptions::default();
        // G4 sounds open on the drone or at the third fret of the first string.
        let p = fret_notes(&t, Capo::NONE, &[67], None, &o)[0].unwrap();
        assert_eq!(t.pitch(p.string, p.fret, Capo::NONE), Some(67));
        // A4 (69) cannot be sounded by the drone (frets 1-4 do not exist) but the neck has it.
        let a = fret_notes(&t, Capo::NONE, &[69], None, &o)[0].unwrap();
        assert!(a.string != 4 || a.fret >= 5);
    }

    #[test]
    fn empty_events_and_empty_input() {
        let t = guitar();
        let r = fret_sequence(&t, Capo::NONE, &[], None, &FretOptions::default());
        assert!(r.placements.is_empty());
        let r = fret_sequence(
            &t,
            Capo::NONE,
            &[FretEvent::default()],
            None,
            &FretOptions::default(),
        );
        assert_eq!(r.placements, vec![Vec::<Option<Placement>>::new()]);
    }

    #[test]
    fn twelve_string_chord_finishes() {
        let t = Tuning::from_opens(&[70, 68, 66, 64, 62, 60, 58, 56, 54, 52, 50, 48], 24);
        let keys: Vec<u8> = (48..72).step_by(2).collect();
        let r = fret_notes(&t, Capo::NONE, &keys, None, &FretOptions::default());
        assert_eq!(r.len(), keys.len());
    }
}
