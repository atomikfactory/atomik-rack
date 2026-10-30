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
//! # Collisions
//!
//! A clip holds at most one note per `(tick, key)`. Two notes of one key that quantize onto the
//! same tick would collide, so the model keeps both and moves the later one forward by single
//! ticks to the next free tick (never drops a note). Unselected notes keep their places. (Stacking
//! two notes on one position and pitch is not possible here, so the nudge stays.)
//!
//! # Not in here
//!
//! The commands that apply this to clips and keep the grooves in the song are in
//! [`crate::groove_cmd`]. Audio clips are not touched; controller lane points stay where they are.

use crate::song::{MAX_TICK, Note, StepResolution};
use atomik_core::PPQ;
use std::collections::BTreeSet;

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
/// What 1.0 of humanize timing means in ticks: a 1/16 note, either way.
pub const HUMANIZE_TIME_TICKS: i64 = PPQ / 4;
/// What 1.0 of humanize velocity means: plus or minus half the velocity range.
pub const HUMANIZE_VELOCITY_RANGE: f32 = 0.5;
/// The lowest velocity quantize leaves a note with (a MIDI velocity of 1).
const MIN_VELOCITY: f32 = 1.0 / 127.0;

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
    /// Another seed is another take of the same humanize amounts.
    pub seed: u32,
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
            seed: 0,
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
    }
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
fn signed_unit(hash: u64) -> f64 {
    ((hash >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)) * 2.0 - 1.0
}

/// The result of [`quantize_notes`].
#[derive(Debug, Clone, PartialEq)]
pub struct Quantized {
    /// The notes, sorted by `(tick, key)` without duplicates.
    pub notes: Vec<Note>,
    /// Where each input note went: `notes[new_index[i]]` is input note `i`.
    pub new_index: Vec<usize>,
}

/// The target positions: the grid with its swing and groove.
struct Targets<'a> {
    grid: i64,
    delay: f64,
    groove: Option<&'a Groove>,
}

impl Targets<'_> {
    /// The groove slot of grid position `i`.
    fn slot(&self, i: i64) -> GrooveSlot {
        self.groove.map_or(GrooveSlot::STRAIGHT, |g| {
            g.slots[i.rem_euclid(g.slots.len() as i64) as usize]
        })
    }

    /// Where grid position `i` is, in ticks.
    fn position(&self, i: i64) -> f64 {
        let grid = self.grid as f64;
        let swing = if i.rem_euclid(2) == 1 {
            self.delay * grid
        } else {
            0.0
        };
        i as f64 * grid + swing + f64::from(self.slot(i).offset) * grid
    }

    /// The position index nearest to tick `t` (the earlier one when two are as near).
    fn nearest(&self, t: i64) -> i64 {
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
    let grid = groove.map_or(params.grid, |g| g.step.ticks()).max(1);
    let targets = Targets {
        grid,
        delay: swing_delay(params.swing),
        groove,
    };
    let seed = u64::from(params.seed);
    let moved: Vec<Note> = notes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            if !selected.is_none_or(|s| s.contains(&i)) {
                return *n;
            }
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
            Note {
                tick: (start - origin).clamp(0, MAX_TICK),
                length: length.clamp(1, MAX_TICK),
                velocity: velocity.clamp(MIN_VELOCITY, 1.0),
                ..*n
            }
        })
        .collect();
    resolve(notes, selected, moved)
}

/// Claims the first free `(tick, key)` at or after `tick`, or, when everything up to the last tick
/// is taken, the nearest free one before it: a note never leaves `0..=MAX_TICK`.
fn free_tick(taken: &mut BTreeSet<(i64, u8)>, tick: i64, key: u8) -> i64 {
    let at = (tick..=MAX_TICK)
        .chain((0..tick).rev())
        .find(|t| !taken.contains(&(*t, key)))
        .unwrap_or(tick);
    taken.insert((at, key));
    at
}

/// Sorts the moved notes and gives collisions of `(tick, key)` the next free tick: the notes that
/// did not move claim their places first, then the moved ones in their input order.
fn resolve(old: &[Note], selected: Option<&BTreeSet<usize>>, moved: Vec<Note>) -> Quantized {
    let is_moved = |i: usize| selected.is_none_or(|s| s.contains(&i));
    let mut taken: BTreeSet<(i64, u8)> = BTreeSet::new();
    let mut placed: Vec<(Note, usize)> = Vec::with_capacity(moved.len());
    for pass in [false, true] {
        for (i, mut n) in moved.iter().copied().enumerate() {
            if is_moved(i) != pass {
                continue;
            }
            n.tick = free_tick(&mut taken, n.tick, n.key);
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
}
