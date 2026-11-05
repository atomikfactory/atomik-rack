//! Quantize, swing, groove templates and humanize: pure functions over note lists.
//!
//! # Model
//!
//! **Quantize** moves a note's start towards the nearest *target* position by a strength
//! (0 = stay, 1 = land on it). The targets are the grid positions `i * grid` of the song (ticks
//! from the start of the song, so bars stay in line whatever the clip's own offset is), each
//! moved by the **swing** (an odd position is delayed) and by the **groove** (the slot of the
//! position adds its timing offset). A groove is a [`Groove`]: a named list of per-slot
//! [`GrooveSlot`]s, each a timing offset in fractions of one step and a velocity scale; slot
//! `i mod len` belongs to grid position `i`. A groove has a step ([`StepResolution`]) and a grid
//! that differs from it would not mean anything, so a quantize that names a groove runs on the
//! groove's step.
//!
//! **Swing** is on the drum-machine scale: the percentage of a pair of steps taken by the first step. 50 is
//! straight, 66 is triplet feel and 75 is the most the model accepts. An odd position is delayed
//! by `(swing - 50) / 50` of a step. (A clip's live [`crate::song::StepGrid`] swing is a different
//! thing on a different scale: it delays notes at playback and never edits them. This one edits
//! the notes.)
//!
//! **Humanize** adds a bounded random timing and velocity offset to each selected note. The
//! offset is a pure function of the seed, the clip id, the note's tick and key *before* the
//! edit (the same [`hash64`] splitmix as the sequencers use, see `atomik_dsp::rng`), so the same
//! command on the same song gives the same notes, a bounce is reproducible and an undo followed
//! by a redo cannot change anything. Another seed gives another take of the same amounts.
//!
//! **Swing parity.** [`quantize_notes_in`] takes the song's bar lines: an odd step is the odd step
//! of its bar, so after a meter change, or a bar that holds an odd number of steps, the swing
//! starts straight again at the bar line. (Without bar lines the steps count from the start of
//! the song.) A 4/4 song is not affected.
//!
//! **Lengths.** A quantize can also humanize the note lengths (a bounded random change, one tick
//! at the least) and make the quantized notes legato (each reaches the start of the next).
//!
//! # Collisions
//!
//! A clip holds at most one note per `(tick, key)`. Two notes of one key that quantize onto the
//! same tick would collide, so the model keeps both and moves the later one forward by single
//! ticks to the next free tick (never drops a note). Unselected notes keep their places. (Stacking
//! two notes on one position and pitch is not possible here, so the nudge stays.)
//!
//! # Live groove
//!
//! A track can also *name* a groove and a strength ([`crate::song::TrackGroove`]) instead of
//! having it applied to its notes. The groove is then applied when the song is flattened for the
//! engine and the offline render ([`crate::song::flatten_track_notes`], [`groove_notes`]): each
//! note moves by the offset of the slot of the straight grid position nearest to its start, and
//! its velocity is scaled by the slot's velocity scale, both by the strength. The stored notes
//! never change, so editing a groove, switching it or removing it re-times every track that uses
//! it, and a track whose strength is 0 plays exactly as it is stored. The grid is the song's, as
//! in a quantize.
//!
//! # Not in here
//!
//! The commands that apply this to clips and keep the grooves in the song are in
//! [`crate::groove_cmd`]. Audio clips are not touched.
//!
//! # Grid per note
//!
//! With [`QuantizeParams::grid_by_length`] each note picks its own grid ([`grid_for_length`]):
//! the note value nearest to the note's length, from a 1/32 note up to the grid of the
//! parameters, which is the coarsest a note may snap to. A long note then lands on the big grid
//! and a short one on a fine grid in one quantize, instead of every note on one value. A groove
//! sets one grid (its step), so the option does nothing while a groove is named.
//!
//! # Controller lanes
//!
//! Controller lane points of a note clip follow their notes ([`QuantizeParams::controllers`],
//! [`crate::controller_edit::follow_note_moves`]): a point between a note's old start (included)
//! and its new start (excluded) moves by the same distance as the note.

use crate::song::{MAX_TICK, Note, StepResolution};
use atomik_core::{MeterMap, PPQ};
use std::collections::{BTreeMap, BTreeSet};

/// Most grooves a song may hold (the built-in ones are not counted).
pub const MAX_GROOVES: usize = 64;
/// Longest groove name, in characters.
pub const MAX_GROOVE_NAME: usize = 32;
/// Most slots a groove may have (two bars of 16ths).
pub const MAX_GROOVE_SLOTS: usize = 32;
/// Largest timing offset of a slot, in steps (either way).
pub const GROOVE_OFFSET_MAX: f32 = 0.5;
/// Largest velocity scale of a slot.
pub const GROOVE_VELOCITY_MAX: f32 = 2.0;
/// Straight swing, in percent.
pub const SWING_MIN: u8 = 50;
/// Largest swing, in percent.
pub const SWING_MAX: u8 = 75;
/// Longest grid a quantize accepts: four 4/4 bars.
pub const QUANTIZE_GRID_MAX: i64 = 16 * PPQ;
/// Largest strength of a track's live groove, in percent.
pub const GROOVE_STRENGTH_MAX: u8 = 100;
/// What 1.0 of humanize timing means in ticks: a 1/16 note, either way.
pub const HUMANIZE_TIME_TICKS: i64 = PPQ / 4;
/// What 1.0 of humanize velocity means: plus or minus half the velocity range.
pub const HUMANIZE_VELOCITY_RANGE: f32 = 0.5;
/// The finest grid a note takes when each note picks its own ([`grid_for_length`]): a 1/32 note.
pub const PER_NOTE_GRID_MIN: i64 = PPQ / 8;
/// The lowest velocity quantize leaves a note with (a MIDI velocity of 1).
pub(crate) const MIN_VELOCITY: f32 = 1.0 / 127.0;

/// One slot of a [`Groove`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrooveSlot {
    /// Timing offset in steps, `-0.5..=0.5` (positive plays later).
    pub offset: f32,
    /// Velocity scale, `0..=2` (1 keeps the velocity).
    pub velocity: f32,
}

impl GrooveSlot {
    /// A slot that changes nothing.
    pub const STRAIGHT: GrooveSlot = GrooveSlot {
        offset: 0.0,
        velocity: 1.0,
    };
}

/// A groove template: per-slot timing and velocity, repeating along the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Groove {
    /// Trimmed, non-empty, at most [`MAX_GROOVE_NAME`] characters, unique among the song's
    /// grooves and different from every built-in name.
    pub name: String,
    /// The length of one slot.
    pub step: StepResolution,
    /// `1..=`[`MAX_GROOVE_SLOTS`] slots; grid position `i` uses slot `i mod len`.
    pub slots: Vec<GrooveSlot>,
}

impl Groove {
    /// Checks every range; `Err` says what is wrong.
    pub fn validate(&self) -> Result<(), String> {
        let n = self.name.chars().count();
        if self.name.trim() != self.name || n == 0 || n > MAX_GROOVE_NAME {
            return Err(format!(
                "groove name {:?} must be trimmed, non-empty and at most {MAX_GROOVE_NAME} characters",
                self.name
            ));
        }
        if self.slots.is_empty() || self.slots.len() > MAX_GROOVE_SLOTS {
            return Err(format!(
                "groove {:?} needs 1..={MAX_GROOVE_SLOTS} slots, found {}",
                self.name,
                self.slots.len()
            ));
        }
        for (i, s) in self.slots.iter().enumerate() {
            if !(-GROOVE_OFFSET_MAX..=GROOVE_OFFSET_MAX).contains(&s.offset) {
                return Err(format!(
                    "groove {:?} slot {i}: offset {} is outside -{GROOVE_OFFSET_MAX}..={GROOVE_OFFSET_MAX}",
                    self.name, s.offset
                ));
            }
            if !(0.0..=GROOVE_VELOCITY_MAX).contains(&s.velocity) {
                return Err(format!(
                    "groove {:?} slot {i}: velocity scale {} is outside 0..={GROOVE_VELOCITY_MAX}",
                    self.name, s.velocity
                ));
            }
        }
        Ok(())
    }
}

/// The built-in grooves, in menu order. Their names cannot be used for a song's grooves.
pub fn builtin_grooves() -> Vec<Groove> {
    let swing = |pct: u8| Groove {
        name: format!("Swing {pct}%"),
        step: StepResolution::Sixteenth,
        slots: vec![
            GrooveSlot::STRAIGHT,
            GrooveSlot {
                offset: swing_delay(pct) as f32,
                velocity: 1.0,
            },
        ],
    };
    vec![
        Groove {
            name: "Straight".into(),
            step: StepResolution::Sixteenth,
            slots: vec![GrooveSlot::STRAIGHT; 2],
        },
        swing(55),
        swing(60),
        swing(66),
        Groove {
            name: "Shuffle 8th 66%".into(),
            step: StepResolution::Eighth,
            slots: vec![
                GrooveSlot::STRAIGHT,
                GrooveSlot {
                    offset: swing_delay(66) as f32,
                    velocity: 1.0,
                },
            ],
        },
        Groove {
            name: "Accent 16ths".into(),
            step: StepResolution::Sixteenth,
            slots: [1.0, 0.7, 0.85, 0.7]
                .into_iter()
                .map(|velocity| GrooveSlot {
                    offset: 0.0,
                    velocity,
                })
                .collect(),
        },
    ]
}

/// The built-in groove called `name`.
pub fn builtin_groove(name: &str) -> Option<Groove> {
    builtin_grooves().into_iter().find(|g| g.name == name)
}

/// The delay of an odd step, in steps, for a swing in percent: `(pct - 50) / 50`.
pub fn swing_delay(pct: u8) -> f64 {
    f64::from(pct.clamp(SWING_MIN, SWING_MAX) - SWING_MIN) / 50.0
}

/// What a quantize does.
#[derive(Debug, Clone, PartialEq)]
pub struct QuantizeParams {
    /// The grid in ticks, `1..=`[`QUANTIZE_GRID_MAX`]. Not used when `groove` names one (the
    /// groove's step is the grid).
    pub grid: i64,
    /// How far note starts move towards their target, `0..=1`.
    pub strength: f32,
    /// Also move note ends towards their targets by `strength` (else a note keeps its length).
    pub snap_ends: bool,
    /// Swing in percent, [`SWING_MIN`]`..=`[`SWING_MAX`]; 50 is straight.
    pub swing: u8,
    /// The groove to apply by name: a built-in one or one of the song's.
    pub groove: Option<String>,
    /// How much of the groove's velocity scale is applied, `0..=1`.
    pub groove_velocity: f32,
    /// Random timing, `0..=1` of [`HUMANIZE_TIME_TICKS`], either way.
    pub humanize_time: f32,
    /// Random velocity, `0..=1` of [`HUMANIZE_VELOCITY_RANGE`], either way.
    pub humanize_velocity: f32,
    /// Random note length, `0..=1` of [`HUMANIZE_TIME_TICKS`], either way (a note stays at least
    /// one tick long).
    pub humanize_length: f32,
    /// After the notes moved, stretch each one to reach the start of the next selected note.
    pub legato: bool,
    /// Another seed is another take of the same humanize amounts.
    pub seed: u32,
    /// Each note snaps to a grid of its own, the note value nearest to its length (see
    /// [`grid_for_length`]), with `grid` as the coarsest. Not used when `groove` names one.
    pub grid_by_length: bool,
    /// Controller lane points of the note clips move with their notes (see
    /// [`crate::controller_edit::follow_note_moves`]).
    pub controllers: bool,
}

impl Default for QuantizeParams {
    /// Quantize to 1/16 notes, fully, with nothing else.
    fn default() -> Self {
        Self {
            grid: PPQ / 4,
            strength: 1.0,
            snap_ends: false,
            swing: SWING_MIN,
            groove: None,
            groove_velocity: 1.0,
            humanize_time: 0.0,
            humanize_velocity: 0.0,
            humanize_length: 0.0,
            legato: false,
            seed: 0,
            grid_by_length: false,
            controllers: true,
        }
    }
}

impl QuantizeParams {
    /// Checks every range; `Err` says what is wrong. (The groove name is looked up later.)
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=QUANTIZE_GRID_MAX).contains(&self.grid) {
            return Err(format!(
                "quantize grid {} is outside 1..={QUANTIZE_GRID_MAX}",
                self.grid
            ));
        }
        for (what, v) in [
            ("strength", self.strength),
            ("groove velocity", self.groove_velocity),
            ("humanize time", self.humanize_time),
            ("humanize velocity", self.humanize_velocity),
            ("humanize length", self.humanize_length),
        ] {
            if !(0.0..=1.0).contains(&v) {
                return Err(format!("quantize {what} {v} is outside 0..=1"));
            }
        }
        if !(SWING_MIN..=SWING_MAX).contains(&self.swing) {
            return Err(format!(
                "quantize swing {} is outside {SWING_MIN}..={SWING_MAX}",
                self.swing
            ));
        }
        Ok(())
    }

    /// Whether this changes nothing at all (strength 0 and no groove velocity or humanize).
    pub fn is_identity(&self, groove: Option<&Groove>) -> bool {
        let velocity = groove.is_some() && self.groove_velocity > 0.0;
        self.strength == 0.0
            && !velocity
            && self.humanize_time == 0.0
            && self.humanize_velocity == 0.0
            && self.humanize_length == 0.0
            && !self.legato
    }
}

/// The grid a note of `length` ticks takes when each note picks its own: the nearest of `limit`
/// and the straight note values from [`PER_NOTE_GRID_MIN`] up to it, nearest meaning the
/// smallest ratio between the two lengths (so a note of 100 ticks is nearer to 128 than to 64);
/// two equally near take the finer. A note shorter than the finest value takes the finest.
pub fn grid_for_length(length: i64, limit: i64) -> i64 {
    let limit = limit.clamp(1, QUANTIZE_GRID_MAX);
    let length = i128::from(length.max(1));
    let mut candidates = Vec::new();
    let mut v = PER_NOTE_GRID_MIN;
    while v < limit {
        candidates.push(v);
        v *= 2;
    }
    candidates.push(limit);
    // The ratio of the longer to the shorter as a fraction: compared by cross multiplying.
    let ratio = |c: i64| {
        let c = i128::from(c);
        (length.max(c), length.min(c))
    };
    let mut best = candidates[0];
    for &c in &candidates[1..] {
        let (an, ad) = ratio(c);
        let (bn, bd) = ratio(best);
        if an * bd < bn * ad {
            best = c;
        }
    }
    best
}

/// splitmix64 finaliser step. The same integer-only function as `atomik_dsp::rng`'s (the model
/// does not depend on the DSP crate); a test pins its values.
const fn splitmix(z: u64) -> u64 {
    let mut z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Stateless hash of `(seed, a, b)`, identical to `atomik_dsp::rng::hash64`.
pub const fn hash64(seed: u64, a: u64, b: u64) -> u64 {
    splitmix(splitmix(splitmix(seed) ^ a) ^ b)
}

/// A hash as a value in `[-1, 1)`.
pub(crate) fn signed_unit(hash: u64) -> f64 {
    ((hash >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)) * 2.0 - 1.0
}

/// Applies a live groove to flattened notes `(song tick, length, key, velocity)`: the notes keep
/// their lengths and move by the offset of the slot nearest to their start, scaled by `strength`
/// (percent, [`GROOVE_STRENGTH_MAX`] at most), and a start never goes below tick 0. The velocity is
/// scaled by the slot's velocity scale the same way. The result is sorted by `(tick, key)` like
/// the input (stably, so equal ticks and keys keep their order). Integer rounding of one `f64`
/// product per note, so the same song gives the same notes on every host.
pub fn groove_notes(
    notes: Vec<(i64, i64, u8, f32)>,
    groove: &Groove,
    strength: u8,
) -> Vec<(i64, i64, u8, f32)> {
    if strength == 0 || notes.is_empty() {
        return notes;
    }
    let amount = f64::from(strength.min(GROOVE_STRENGTH_MAX)) / 100.0;
    let step = groove.step.ticks().max(1);
    let targets = Targets {
        grid: step,
        delay: 0.0,
        groove: Some(groove),
        meter: None,
    };
    let mut out: Vec<(i64, i64, u8, f32)> = notes
        .into_iter()
        .map(|(tick, length, key, velocity)| {
            let k = (tick + step / 2).div_euclid(step);
            let slot = targets.slot(k);
            let moved = (f64::from(slot.offset) * step as f64 * amount).round() as i64;
            let scale = 1.0 + (f64::from(slot.velocity) - 1.0) * amount;
            let velocity = (f64::from(velocity) * scale) as f32;
            (
                (tick + moved).clamp(0, MAX_TICK),
                length,
                key,
                velocity.clamp(MIN_VELOCITY, 1.0),
            )
        })
        .collect();
    out.sort_by_key(|&(t, _, k, _)| (t, k));
    out
}

/// The result of [`quantize_notes`].
#[derive(Debug, Clone, PartialEq)]
pub struct Quantized {
    /// The notes, sorted by `(tick, key)` without duplicates.
    pub notes: Vec<Note>,
    /// Where each input note went: `notes[new_index[i]]` is input note `i`.
    pub new_index: Vec<usize>,
}

impl Quantized {
    /// The `(old start, new start)` of every input note whose start changed (`old` is the list
    /// that was quantized), in input order.
    pub fn moves(&self, old: &[Note]) -> Vec<(i64, i64)> {
        old.iter()
            .zip(&self.new_index)
            .filter_map(|(o, &at)| {
                let new = self.notes[at].tick;
                (new != o.tick).then_some((o.tick, new))
            })
            .collect()
    }
}

/// The target positions: the grid with its swing and groove.
pub(crate) struct Targets<'a> {
    pub(crate) grid: i64,
    pub(crate) delay: f64,
    pub(crate) groove: Option<&'a Groove>,
    /// The song's bar lines. With them a step is odd (and swung) when it is the odd step of its
    /// own bar, so a bar of an odd number of steps starts straight again; without them steps
    /// count from the start of the song.
    pub(crate) meter: Option<MeterMap<'a>>,
}

impl Targets<'_> {
    /// The groove slot of grid position `i`.
    fn slot(&self, i: i64) -> GrooveSlot {
        self.groove.map_or(GrooveSlot::STRAIGHT, |g| {
            g.slots[i.rem_euclid(g.slots.len() as i64) as usize]
        })
    }

    /// Whether grid position `i` is an odd step (the swing delays it).
    fn is_odd(&self, i: i64) -> bool {
        let Some(meter) = self.meter else {
            return i.rem_euclid(2) == 1;
        };
        let at = i.saturating_mul(self.grid).max(0);
        let from_bar = at - meter.bar_start(meter.bar_at(at));
        ((from_bar + self.grid / 2) / self.grid) % 2 == 1
    }

    /// Where grid position `i` is, in ticks.
    pub(crate) fn position(&self, i: i64) -> f64 {
        let grid = self.grid as f64;
        let swing = if self.is_odd(i) {
            self.delay * grid
        } else {
            0.0
        };
        i as f64 * grid + swing + f64::from(self.slot(i).offset) * grid
    }

    /// The position index nearest to tick `t` (the earlier one when two are as near).
    pub(crate) fn nearest(&self, t: i64) -> i64 {
        let k = (t + self.grid / 2).div_euclid(self.grid);
        let mut best = k - 1;
        let mut best_d = (self.position(best) - t as f64).abs();
        for i in [k, k + 1] {
            let d = (self.position(i) - t as f64).abs();
            if d < best_d {
                (best, best_d) = (i, d);
            }
        }
        best
    }
}

/// Moves `from` towards `to` by `strength`, rounded to a tick.
fn towards(from: i64, to: f64, strength: f32) -> i64 {
    (from as f64 + (to - from as f64) * f64::from(strength)).round() as i64
}

/// Quantizes, swings, grooves and humanizes the notes of a clip.
///
/// `origin` is the song tick of the clip's content tick 0 (`clip.start - clip.offset`), so the
/// grid is the song's, not the clip's. Only the notes of `selected` (all of them when `None`) are
/// touched. `clip_seed` (the clip's id) and `params.seed` make the humanize offsets. `groove` is
/// the groove `params.groove` names, already looked up; the caller passes `None` for none.
pub fn quantize_notes(
    notes: &[Note],
    selected: Option<&BTreeSet<usize>>,
    origin: i64,
    clip_seed: u64,
    params: &QuantizeParams,
    groove: Option<&Groove>,
) -> Quantized {
    quantize_notes_in(notes, selected, origin, clip_seed, params, groove, None)
}

/// [`quantize_notes`] on the song's bar lines: with `meter` the swing counts steps from the start
/// of each bar (see [`Targets`]), so a meter change or a bar of an odd number of steps does not
/// turn the swing around. A 4/4 song gives the same notes as [`quantize_notes`].
pub fn quantize_notes_in(
    notes: &[Note],
    selected: Option<&BTreeSet<usize>>,
    origin: i64,
    clip_seed: u64,
    params: &QuantizeParams,
    groove: Option<&Groove>,
    meter: Option<MeterMap<'_>>,
) -> Quantized {
    let grid = groove.map_or(params.grid, |g| g.step.ticks()).max(1);
    let delay = swing_delay(params.swing);
    let targets_of = |grid: i64| Targets {
        grid,
        delay,
        groove,
        meter,
    };
    // The grid of a note: one for the whole selection, or (without a groove) one per note.
    let per_note = params.grid_by_length && groove.is_none();
    let grid_of = |n: &Note| {
        if per_note {
            grid_for_length(n.length, grid)
        } else {
            grid
        }
    };
    let by_grid: BTreeMap<i64, Targets<'_>> = notes
        .iter()
        .enumerate()
        .filter(|(i, _)| selected.is_none_or(|s| s.contains(i)))
        .map(|(_, n)| grid_of(n))
        .chain([grid])
        .map(|g| (g, targets_of(g)))
        .collect();
    let seed = u64::from(params.seed);
    let moved: Vec<Note> = notes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            if !selected.is_none_or(|s| s.contains(&i)) {
                return *n;
            }
            let grid = grid_of(n);
            let targets = &by_grid[&grid];
            let t = origin + n.tick;
            let k = targets.nearest(t);
            let mut start = towards(t, targets.position(k), params.strength);
            let mut length = n.length;
            if params.snap_ends {
                let end = t + n.length;
                let e = towards(end, targets.position(targets.nearest(end)), params.strength);
                length = if e > start { e - start } else { grid };
            }
            let mut velocity = n.velocity;
            if groove.is_some() {
                let scale = targets.slot(k).velocity;
                velocity *= 1.0 + (scale - 1.0) * params.groove_velocity;
            }
            // Humanize: the offsets come from where the note was, not where it goes.
            let at = (n.tick as u64) ^ (u64::from(n.key) << 56);
            if params.humanize_time > 0.0 {
                let u = signed_unit(hash64(seed, clip_seed, at));
                let ticks = u * f64::from(params.humanize_time) * HUMANIZE_TIME_TICKS as f64;
                start += ticks.round() as i64;
            }
            if params.humanize_velocity > 0.0 {
                let u = signed_unit(hash64(seed ^ 0xA5A5_A5A5, clip_seed, at));
                velocity +=
                    (u * f64::from(params.humanize_velocity * HUMANIZE_VELOCITY_RANGE)) as f32;
            }
            if params.humanize_length > 0.0 {
                let u = signed_unit(hash64(seed ^ 0x5A5A_5A5A, clip_seed, at));
                let ticks = u * f64::from(params.humanize_length) * HUMANIZE_TIME_TICKS as f64;
                length += ticks.round() as i64;
            }
            Note {
                tick: (start - origin).clamp(0, MAX_TICK),
                length: length.clamp(1, MAX_TICK),
                velocity: velocity.clamp(MIN_VELOCITY, 1.0),
                ..*n
            }
        })
        .collect();
    let mut out = resolve(notes, selected, moved);
    if params.legato {
        legato(&mut out, selected);
    }
    out
}

/// Stretches (or shortens) each selected note to reach the start of the next later selected
/// note; the last one keeps its length. Starts and keys do not change, so the notes stay sorted.
fn legato(out: &mut Quantized, selected: Option<&BTreeSet<usize>>) {
    let is_selected = |i: usize| selected.is_none_or(|s| s.contains(&i));
    let at: Vec<usize> = (0..out.new_index.len())
        .filter(|&i| is_selected(i))
        .map(|i| out.new_index[i])
        .collect();
    let starts: BTreeSet<i64> = at.iter().map(|&p| out.notes[p].tick).collect();
    for p in at {
        let n = &mut out.notes[p];
        if let Some(&next) = starts.range(n.tick + 1..).next() {
            n.length = (next - n.tick).clamp(1, MAX_TICK);
        }
    }
}

/// Claims the first free `(tick, key)` at or after `tick`, or, when everything up to the last tick
/// is taken, the nearest free one before it: a note never leaves `0..=MAX_TICK`.
///
/// `resume` remembers, per `(key, tick)` asked for, where the last search ended: the ticks from
/// there back to the one asked for are all taken (the set only grows), so a pile of notes that
/// land on one tick costs one step each instead of a walk over the whole pile.
fn free_tick(
    taken: &mut BTreeSet<(i64, u8)>,
    resume: &mut BTreeMap<(u8, i64), i64>,
    tick: i64,
    key: u8,
) -> i64 {
    let from = resume.get(&(key, tick)).copied().unwrap_or(tick);
    let at = (from..=MAX_TICK)
        .find(|t| !taken.contains(&(*t, key)))
        .or_else(|| (0..tick).rev().find(|t| !taken.contains(&(*t, key))))
        .unwrap_or(tick);
    taken.insert((at, key));
    if at >= tick {
        resume.insert((key, tick), at + 1);
    }
    at
}

/// Sorts the moved notes and gives collisions of `(tick, key)` the next free tick: the notes that
/// did not move claim their places first, then the moved ones in their input order.
fn resolve(old: &[Note], selected: Option<&BTreeSet<usize>>, moved: Vec<Note>) -> Quantized {
    let is_moved = |i: usize| selected.is_none_or(|s| s.contains(&i));
    let mut taken: BTreeSet<(i64, u8)> = BTreeSet::new();
    let mut resume: BTreeMap<(u8, i64), i64> = BTreeMap::new();
    let mut placed: Vec<(Note, usize)> = Vec::with_capacity(moved.len());
    for pass in [false, true] {
        for (i, mut n) in moved.iter().copied().enumerate() {
            if is_moved(i) != pass {
                continue;
            }
            n.tick = free_tick(&mut taken, &mut resume, n.tick, n.key);
            placed.push((n, i));
        }
    }
    debug_assert_eq!(placed.len(), old.len());
    placed.sort_by_key(|(n, i)| (n.tick, n.key, *i));
    let mut new_index = vec![0; placed.len()];
    for (pos, (_, i)) in placed.iter().enumerate() {
        new_index[*i] = pos;
    }
    Quantized {
        notes: placed.into_iter().map(|(n, _)| n).collect(),
        new_index,
    }
}

/// Measures the groove of a clip's notes: for each of `slots` positions of the `resolution`
/// grid, the average timing offset of the notes nearest to it (in steps) and their average
/// velocity relative to the average of all notes. A slot no note is near stays straight.
/// `origin` is the song tick of content tick 0. Muted notes are ignored. `Err` when there is no
/// note to measure or `slots` is out of range. Values are rounded to 1/1000.
pub fn extract_groove(
    name: &str,
    notes: &[Note],
    origin: i64,
    resolution: StepResolution,
    slots: usize,
) -> Result<Groove, String> {
    if slots == 0 || slots > MAX_GROOVE_SLOTS {
        return Err(format!(
            "a groove has 1..={MAX_GROOVE_SLOTS} slots, not {slots}"
        ));
    }
    let step = resolution.ticks();
    let mut off = vec![(0.0f64, 0u32); slots];
    let mut vel = vec![0.0f64; slots];
    let (mut vel_all, mut count) = (0.0f64, 0u32);
    for n in notes.iter().filter(|n| !n.muted) {
        let t = origin + n.tick;
        let k = (t + step / 2).div_euclid(step);
        let slot = k.rem_euclid(slots as i64) as usize;
        off[slot].0 += (t - k * step) as f64 / step as f64;
        off[slot].1 += 1;
        vel[slot] += f64::from(n.velocity);
        vel_all += f64::from(n.velocity);
        count += 1;
    }
    if count == 0 {
        return Err("the clip has no unmuted notes to measure".into());
    }
    let mean_all = vel_all / f64::from(count);
    let round = |x: f64| (x * 1000.0).round() / 1000.0;
    let slots = off
        .iter()
        .zip(&vel)
        .map(|(&(sum, n), &v)| {
            if n == 0 {
                return GrooveSlot::STRAIGHT;
            }
            let n = f64::from(n);
            GrooveSlot {
                offset: round(sum / n).clamp(-0.5, 0.5) as f32,
                velocity: round(v / n / mean_all).clamp(0.0, 2.0) as f32,
            }
        })
        .collect();
    let g = Groove {
        name: name.to_string(),
        step: resolution,
        slots,
    };
    g.validate()?;
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = PPQ / 4;

    fn note(tick: i64, key: u8) -> Note {
        Note::new(tick, S, key, 0.8)
    }

    fn params() -> QuantizeParams {
        QuantizeParams::default()
    }

    fn q(notes: &[Note], p: &QuantizeParams, groove: Option<&Groove>) -> Vec<Note> {
        quantize_notes(notes, None, 0, 1, p, groove).notes
    }

    #[test]
    fn hash64_matches_the_dsp_crate_pinned_value() {
        assert_eq!(hash64(0, 0, 0), 0x2382_75BC_38FC_BE91);
        assert_ne!(hash64(1, 0, 0), hash64(0, 1, 0));
    }

    #[test]
    fn a_pile_of_notes_on_one_tick_takes_the_following_ticks_in_input_order() {
        // 3 000 notes of one key all snap to tick 0: each takes the next free tick, the way a
        // walk from tick 0 for every note would find them.
        let notes: Vec<Note> = (0..3000).map(|_| note(10, 60)).collect();
        let out = q(&notes, &params(), None);
        let ticks: Vec<i64> = out.iter().map(|n| n.tick).collect();
        assert_eq!(ticks, (0..3000).collect::<Vec<i64>>());
        // A note of another key on the same tick is not pushed by the pile.
        let mut mixed = notes.clone();
        mixed.push(note(10, 61));
        let out = quantize_notes(&mixed, None, 0, 1, &params(), None);
        assert_eq!(out.notes.iter().filter(|n| n.key == 61).count(), 1);
        assert_eq!(
            out.notes.iter().find(|n| n.key == 61).map(|n| n.tick),
            Some(0)
        );
    }

    #[test]
    fn quantize_snaps_to_the_nearest_grid_position() {
        let notes = [note(S + 300, 60), note(2 * S - 500, 62), note(0, 64)];
        let out = q(&notes, &params(), None);
        assert_eq!(
            out.iter().map(|n| n.tick).collect::<Vec<_>>(),
            [0, S, 2 * S]
        );
        assert!(out.iter().all(|n| n.length == S), "lengths are kept");
    }

    #[test]
    fn quantize_strength_moves_partway_and_zero_is_a_no_op() {
        let notes = [note(S + 400, 60)];
        let half = QuantizeParams {
            strength: 0.5,
            ..params()
        };
        assert_eq!(q(&notes, &half, None)[0].tick, S + 200);
        let zero = QuantizeParams {
            strength: 0.0,
            ..params()
        };
        assert_eq!(q(&notes, &zero, None), notes);
        assert!(zero.is_identity(None));
        assert!(!half.is_identity(None));
    }

    #[test]
    fn quantize_is_idempotent_at_full_strength() {
        let notes = [note(S + 311, 60), note(3 * S - 99, 61), note(5, 62)];
        let p = QuantizeParams {
            swing: 62,
            ..params()
        };
        let once = q(&notes, &p, None);
        assert_eq!(q(&once, &p, None), once);
    }

    #[test]
    fn swing_delays_the_odd_positions_on_the_mpc_scale() {
        let notes = [note(0, 60), note(S, 60), note(2 * S, 60), note(3 * S, 60)];
        for (pct, delay) in [(50u8, 0), (66, 1229), (75, S / 2)] {
            let p = QuantizeParams {
                swing: pct,
                ..params()
            };
            let out = q(&notes, &p, None);
            assert_eq!(out[0].tick, 0);
            assert_eq!(out[1].tick, S + delay, "swing {pct}");
            assert_eq!(out[2].tick, 2 * S);
            assert_eq!(out[3].tick, 3 * S + delay);
        }
    }

    #[test]
    fn a_note_goes_to_the_nearest_swung_position_not_the_nearest_grid_line() {
        // At 75% the off-beat sits half a step late: a note at 0.6 step is nearer to the on-beat
        // before it than to the off-beat, although plain rounding would pick step 1.
        let p = QuantizeParams {
            swing: 75,
            ..params()
        };
        let out = q(&[note(S * 6 / 10, 60), note(S, 61)], &p, None);
        assert_eq!(out[0].tick, 0);
        assert_eq!(out[1].tick, S + S / 2);
    }

    #[test]
    fn the_grid_follows_the_song_not_the_clip_start() {
        // A clip starting a sixteenth in: its content tick 0 is song tick S, on the grid.
        let notes = [note(40, 60)];
        let out = quantize_notes(&notes, None, S, 1, &params(), None).notes;
        assert_eq!(out[0].tick, 0);
        // A clip that starts 100 ticks late: the grid line one step in is 100 ticks back in the
        // clip's own ticks.
        let out = quantize_notes(&[note(S - 60, 60)], None, 100, 1, &params(), None).notes;
        assert_eq!(out[0].tick, S - 100);
    }

    #[test]
    fn a_groove_offsets_slots_and_scales_velocity() {
        let g = Groove {
            name: "t".into(),
            step: StepResolution::Sixteenth,
            slots: vec![
                GrooveSlot {
                    offset: 0.0,
                    velocity: 1.0,
                },
                GrooveSlot {
                    offset: 0.25,
                    velocity: 0.5,
                },
            ],
        };
        let notes = [note(3, 60), note(S - 7, 61), note(2 * S + 9, 62)];
        let out = q(&notes, &params(), Some(&g));
        assert_eq!(
            out.iter().map(|n| n.tick).collect::<Vec<_>>(),
            [0, S + S / 4, 2 * S]
        );
        assert!((out[0].velocity - 0.8).abs() < 1e-6);
        assert!((out[1].velocity - 0.4).abs() < 1e-6);
        let half = QuantizeParams {
            groove_velocity: 0.5,
            ..params()
        };
        assert!((q(&notes, &half, Some(&g))[1].velocity - 0.6).abs() < 1e-6);
    }

    #[test]
    fn a_groove_sets_the_grid() {
        let g = builtin_groove("Shuffle 8th 66%").unwrap();
        let p = QuantizeParams {
            grid: S,
            ..params()
        };
        let eighth = PPQ / 2;
        let out = q(&[note(eighth - 100, 60)], &p, Some(&g));
        assert_eq!(out[0].tick, eighth + 2458);
    }

    #[test]
    fn quantize_only_touches_the_selected_notes() {
        let notes = [note(100, 60), note(S + 100, 62)];
        let sel: BTreeSet<usize> = [1].into();
        let out = quantize_notes(&notes, Some(&sel), 0, 1, &params(), None);
        assert_eq!(out.notes[0].tick, 100);
        assert_eq!(out.notes[1].tick, S);
        assert_eq!(out.new_index, [0, 1]);
    }

    #[test]
    fn snapping_ends_moves_the_end_to_its_target() {
        let p = QuantizeParams {
            snap_ends: true,
            ..params()
        };
        let n = Note::new(S + 100, S + 300, 60, 0.8);
        let out = q(&[n], &p, None);
        assert_eq!((out[0].tick, out[0].length), (S, S));
        let tiny = Note::new(S + 100, 50, 60, 0.8);
        assert_eq!(q(&[tiny], &p, None)[0].length, S, "never empty");
    }

    #[test]
    fn colliding_notes_keep_both_and_the_later_moves_on() {
        let notes = [note(S - 30, 60), note(S + 30, 60), note(S, 61)];
        let out = quantize_notes(&notes, None, 0, 1, &params(), None);
        let ticks: Vec<_> = out.notes.iter().map(|n| (n.tick, n.key)).collect();
        assert_eq!(ticks, [(S, 60), (S, 61), (S + 1, 60)]);
        assert_eq!(out.new_index, [0, 2, 1]);
    }

    #[test]
    fn humanize_is_bounded_and_deterministic() {
        let notes: Vec<Note> = (0..64).map(|i| note(i * S, 60)).collect();
        let p = QuantizeParams {
            strength: 0.0,
            humanize_time: 0.5,
            humanize_velocity: 0.5,
            seed: 7,
            ..params()
        };
        let a = quantize_notes(&notes, None, 0, 9, &p, None).notes;
        let b = quantize_notes(&notes, None, 0, 9, &p, None).notes;
        assert_eq!(a, b, "the same input gives the same notes");
        let max_dt = HUMANIZE_TIME_TICKS / 2;
        let mut moved = 0;
        for (x, y) in notes.iter().zip(&a) {
            assert!((x.tick - y.tick).abs() <= max_dt.max(0));
            assert!((x.velocity - y.velocity).abs() <= 0.25 + 1e-6);
            assert!(y.tick >= 0 && y.velocity > 0.0 && y.velocity <= 1.0);
            moved += usize::from(x.tick != y.tick);
        }
        assert!(moved > 40, "most notes move: {moved}");
        let other = QuantizeParams {
            seed: 8,
            ..p.clone()
        };
        assert_ne!(quantize_notes(&notes, None, 0, 9, &other, None).notes, a);
        assert_ne!(quantize_notes(&notes, None, 0, 10, &p, None).notes, a);
    }

    #[test]
    fn humanize_does_not_depend_on_the_other_notes() {
        let notes = [note(S, 60), note(2 * S, 62), note(3 * S, 64)];
        let p = QuantizeParams {
            strength: 0.0,
            humanize_time: 1.0,
            ..params()
        };
        let all = quantize_notes(&notes, None, 0, 3, &p, None).notes;
        let only: BTreeSet<usize> = [1].into();
        let one = quantize_notes(&notes, Some(&only), 0, 3, &p, None).notes;
        assert_eq!(one[1], all[1]);
        assert_eq!(one[0], notes[0]);
    }

    #[test]
    fn humanize_never_leaves_the_tick_range() {
        let p = QuantizeParams {
            strength: 0.0,
            humanize_time: 1.0,
            ..params()
        };
        for k in 0..50u8 {
            let out = q(&[note(0, k), note(MAX_TICK, k)], &p, None);
            assert!(out.iter().all(|n| (0..=MAX_TICK).contains(&n.tick)));
        }
    }

    #[test]
    fn extract_groove_measures_offsets_and_velocity_per_slot() {
        let notes: Vec<Note> = (0..16)
            .map(|i| {
                let late = if i % 2 == 1 { 600 } else { -100 };
                let v = if i % 2 == 1 { 0.4 } else { 0.8 };
                Note::new((i * S + late).max(0), S, 60, v)
            })
            .collect();
        let g = extract_groove("Mine", &notes, 0, StepResolution::Sixteenth, 2).unwrap();
        assert_eq!(g.slots.len(), 2);
        // Slot 0 holds the on-beats: note 0 is clamped to tick 0, so the mean is 7 notes of -100
        // and one of 0.
        assert!((g.slots[0].offset - (-87.5 / S as f32)).abs() < 2e-3);
        assert!((g.slots[1].offset - 600.0 / S as f32).abs() < 2e-3);
        assert!((g.slots[0].velocity - 0.8 / 0.6).abs() < 2e-3);
        assert!((g.slots[1].velocity - 0.4 / 0.6).abs() < 2e-3);
        // Quantizing with the extracted groove gives the clip's own feel to straight notes.
        let straight: Vec<Note> = (0..4).map(|i| note(i * S, 60)).collect();
        let out = q(&straight, &params(), Some(&g));
        assert!(out[1].tick > S && out[0].tick <= 0);
    }

    #[test]
    fn extract_groove_refuses_nothing_to_measure_and_bad_slot_counts() {
        let muted = Note {
            muted: true,
            ..note(0, 60)
        };
        assert!(extract_groove("x", &[muted], 0, StepResolution::Sixteenth, 2).is_err());
        assert!(extract_groove("x", &[note(0, 60)], 0, StepResolution::Sixteenth, 0).is_err());
        assert!(extract_groove("x", &[note(0, 60)], 0, StepResolution::Sixteenth, 33).is_err());
    }

    #[test]
    fn builtin_grooves_are_valid_and_have_unique_names() {
        let all = builtin_grooves();
        let mut names: Vec<_> = all.iter().map(|g| g.name.clone()).collect();
        for g in &all {
            g.validate().unwrap();
        }
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all.len());
        let s66 = builtin_groove("Swing 66%").unwrap();
        assert!((s66.slots[1].offset - 0.32).abs() < 1e-6);
    }

    #[test]
    fn swing_follows_the_bar_lines_of_the_meter() {
        use atomik_core::{MeterEvent, MeterTable};
        // 3/16 bars hold three steps: counting from the song start, the first step of bar 2 would be
        // an odd one. With the bar lines it is the first step of its bar and plays straight.
        let table = MeterTable::new(&[MeterEvent::new(1, 3, 16)]).unwrap();
        let notes: Vec<Note> = (0..6).map(|i| note(i * S, 60)).collect();
        let p = QuantizeParams {
            swing: 66,
            ..params()
        };
        let delay = 1229;
        let by_song = quantize_notes(&notes, None, 0, 1, &p, None).notes;
        let by_bar = quantize_notes_in(&notes, None, 0, 1, &p, None, Some(table.map())).notes;
        let late = |out: &[Note]| -> Vec<bool> {
            out.iter()
                .enumerate()
                .map(|(i, n)| n.tick == i as i64 * S + delay)
                .collect()
        };
        assert_eq!(late(&by_song), [false, true, false, true, false, true]);
        assert_eq!(late(&by_bar), [false, true, false, false, true, false]);
        // 4/4 bars hold an even number of steps: both agree.
        let four = MeterTable::constant(4, 4);
        let by_four = quantize_notes_in(&notes, None, 0, 1, &p, None, Some(four.map())).notes;
        assert_eq!(by_four, by_song);
    }

    #[test]
    fn swing_restarts_after_a_meter_change() {
        use atomik_core::{MeterEvent, MeterTable};
        // One 7/16 bar (seven steps), then 4/4: the second bar starts on an odd song step but
        // its own first step is straight.
        let table =
            MeterTable::new(&[MeterEvent::new(1, 7, 16), MeterEvent::new(2, 4, 4)]).unwrap();
        let p = QuantizeParams {
            swing: 66,
            ..params()
        };
        let notes = [note(7 * S, 60), note(8 * S, 60)];
        let out = quantize_notes_in(&notes, None, 0, 1, &p, None, Some(table.map())).notes;
        assert_eq!(out[0].tick, 7 * S, "first step of the 4/4 bar is straight");
        assert_eq!(out[1].tick, 8 * S + 1229, "its second step is swung");
    }

    #[test]
    fn humanize_length_changes_lengths_within_bounds_and_is_reproducible() {
        let notes: Vec<Note> = (0..8).map(|i| note(i * S, 60 + i as u8)).collect();
        let p = QuantizeParams {
            humanize_length: 1.0,
            seed: 3,
            ..params()
        };
        let a = q(&notes, &p, None);
        assert_eq!(a, q(&notes, &p, None));
        assert!(a.iter().any(|n| n.length != S));
        assert!(
            a.iter()
                .all(|n| (n.length - S).abs() <= HUMANIZE_TIME_TICKS && n.length >= 1)
        );
        assert!(
            a.iter().zip(&notes).all(|(x, y)| x.tick == y.tick),
            "starts stay"
        );
        let other = QuantizeParams {
            seed: 4,
            ..p.clone()
        };
        assert_ne!(q(&notes, &other, None), a);
        assert!(!p.is_identity(None));
        assert!(
            QuantizeParams {
                humanize_length: 1.5,
                ..params()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn legato_reaches_the_next_selected_start_and_keeps_the_last_length() {
        let notes = [note(10, 60), note(S + 10, 62), note(3 * S + 10, 64)];
        let p = QuantizeParams {
            legato: true,
            ..params()
        };
        let out = q(&notes, &p, None);
        assert_eq!(
            out.iter().map(|n| (n.tick, n.length)).collect::<Vec<_>>(),
            [(0, S), (S, 2 * S), (3 * S, S)]
        );
        // Only the selected notes take part.
        let sel: BTreeSet<usize> = [0, 2].into();
        let out = quantize_notes(&notes, Some(&sel), 0, 1, &p, None).notes;
        assert_eq!((out[0].tick, out[0].length), (0, 3 * S));
        assert_eq!(
            (out[1].tick, out[1].length),
            (S + 10, S),
            "unselected note untouched"
        );
        assert!(!p.is_identity(None));
    }

    #[test]
    fn validation_names_the_bad_field() {
        let mut g = builtin_groove("Straight").unwrap();
        g.slots[0].offset = 0.6;
        assert!(g.validate().unwrap_err().contains("offset"));
        g.slots[0].offset = 0.0;
        g.slots[1].velocity = 2.5;
        assert!(g.validate().unwrap_err().contains("velocity"));
        g.name = " x".into();
        assert!(g.validate().unwrap_err().contains("name"));
        let p = QuantizeParams {
            swing: 80,
            ..params()
        };
        assert!(p.validate().unwrap_err().contains("swing"));
        let p = QuantizeParams {
            strength: f32::NAN,
            ..params()
        };
        assert!(p.validate().is_err());
        let p = QuantizeParams {
            grid: 0,
            ..params()
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn a_note_takes_the_note_value_nearest_to_its_length_up_to_the_limit() {
        let limit = PPQ; // 1/4
        for (length, want) in [
            (1, PER_NOTE_GRID_MIN),
            (PPQ / 8, PER_NOTE_GRID_MIN),
            (PPQ / 4, PPQ / 4),
            (PPQ / 2, PPQ / 2),
            (PPQ, PPQ),
            (3 * PPQ, PPQ),
            // A length is nearer in ratio to the value it is closer to on a log scale: the
            // middle between a 1/16 and a 1/8 is about 0.354 of a quarter.
            (PPQ * 36 / 100, PPQ / 2),
            (PPQ * 35 / 100, PPQ / 4),
        ] {
            assert_eq!(grid_for_length(length, limit), want, "length {length}");
        }
        // The limit is the coarsest, even when it is not a straight value.
        assert_eq!(grid_for_length(2 * PPQ, PPQ / 3), PPQ / 3);
        // A limit finer than the finest value is the only choice.
        assert_eq!(grid_for_length(PPQ, PPQ / 12), PPQ / 12);
        // Nonsense lengths and limits stay inside the accepted grids.
        assert_eq!(grid_for_length(-5, 0), 1);
        assert_eq!(grid_for_length(i64::MAX, i64::MAX), QUANTIZE_GRID_MAX);
    }

    #[test]
    fn grid_by_length_snaps_each_note_to_its_own_grid() {
        // A quarter note and a 1/16 note, both a little late; the limit is a 1/4 grid.
        let long = Note::new(PPQ + 400, PPQ, 60, 0.8);
        let short = Note::new(2 * PPQ + 4 * S + 400, S, 62, 0.8);
        let notes = [long, short];
        let by_length = QuantizeParams {
            grid: PPQ,
            grid_by_length: true,
            ..params()
        };
        let out = q(&notes, &by_length, None);
        // The long note goes to the quarter line, the short one to the nearest 1/16 line.
        assert_eq!(out[0].tick, PPQ);
        assert_eq!(out[1].tick, 2 * PPQ + 4 * S + (400 + S / 2) / S * S);
        // One grid for both: the short note goes to a quarter line as well.
        let fixed = QuantizeParams {
            grid_by_length: false,
            ..by_length.clone()
        };
        let out = q(&notes, &fixed, None);
        assert_eq!((out[0].tick, out[1].tick), (PPQ, 3 * PPQ));
    }

    #[test]
    fn grid_by_length_leaves_unselected_notes_and_ignores_a_groove() {
        let notes = [
            Note::new(S + 100, S, 60, 0.8),
            Note::new(PPQ + 100, PPQ, 62, 0.8),
        ];
        let by_length = QuantizeParams {
            grid: PPQ,
            grid_by_length: true,
            ..params()
        };
        let only_second: BTreeSet<usize> = [1].into();
        let out = quantize_notes(&notes, Some(&only_second), 0, 1, &by_length, None).notes;
        assert_eq!(out[0], notes[0], "an unselected note stays");
        assert_eq!(out[1].tick, PPQ);
        // A groove sets the grid: the option changes nothing.
        let groove = builtin_groove("Straight").unwrap();
        let with = q(&notes, &by_length, Some(&groove));
        let without = q(
            &notes,
            &QuantizeParams {
                grid_by_length: false,
                ..by_length
            },
            Some(&groove),
        );
        assert_eq!(with, without);
    }

    #[test]
    fn snap_ends_with_a_grid_per_note_uses_the_grid_of_that_note() {
        let short = Note::new(S + 50, S - 70, 60, 0.8);
        let p = QuantizeParams {
            grid: PPQ,
            grid_by_length: true,
            snap_ends: true,
            ..params()
        };
        let out = q(&[short], &p, None);
        assert_eq!((out[0].tick, out[0].length), (S, S));
    }

    #[test]
    fn moves_lists_the_notes_whose_start_changed() {
        let notes = [note(S + 300, 60), note(2 * S, 62), note(3 * S - 400, 64)];
        let out = quantize_notes(&notes, None, 0, 1, &params(), None);
        assert_eq!(out.moves(&notes), [(S + 300, S), (3 * S - 400, 3 * S)]);
    }
}
