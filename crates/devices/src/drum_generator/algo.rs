//! The drum generator's rules as pure functions of the global step: which lane hits at step `k`,
//! and how hard. Nothing here keeps state, so a locate, a loop wrap, a bounce at any block size
//! and a continuous run agree (HI-2). Only f64 `+ - * /`, `floor` and `round` are used.
//!
//! The order of the rules for step `k` of a lane:
//!
//! 1. **Written.** The pattern's step (`k` modulo the length) when it has a velocity and passes
//!    its probability. In a *varied* bar a hit may move one step later (never onto an occupied
//!    step, never the kick).
//! 2. **Density.** Above zero an empty step gains a hit with chance `density * w`, below zero a
//!    hit is removed with chance `|density| * (1 - w)`; `w` is the role's weight at that bar
//!    position ([`role_weight`]).
//! 3. **Ghosts.** In a varied bar an empty snare or hat step may gain a soft hit.
//! 4. **Fill.** In the last `fill.amount` part of the last bar of each `fill.every` group the
//!    snare, clap and toms gain hits of rising velocity, hats and cymbals are thinned, and the
//!    kick hits on the bar's last beat.
//! 5. **Accent and scale.** A hit on a strong position of its role is boosted by `accent`, then
//!    every velocity is scaled by `velocity.scale`.
//!
//! Randomness is `hash64(seed_mix, step, slot)` with one slot per rule and lane.

use atomik_device_sdk::{DrumPattern, DrumRole};
use atomik_dsp::rng::{hash64, unit_f64};

use crate::note_clock::Grid;

/// Ticks per quarter note.
const QUARTER: i64 = 15_360;

// Hash slots: the rule in the high byte, the lane in the low one.
const SLOT_PROB: u64 = 0x100;
const SLOT_MOVE: u64 = 0x200;
const SLOT_GHOST: u64 = 0x300;
const SLOT_GHOST_VEL: u64 = 0x400;
const SLOT_ADD: u64 = 0x500;
const SLOT_REMOVE: u64 = 0x600;
const SLOT_FILL: u64 = 0x700;
/// Slot of the map draw (the lane in the low byte).
const SLOT_MAP: u64 = 0x900;
/// Slot of the per-bar variation draw (lane 0).
const SLOT_VARIED: u64 = 0x800;

/// Chance that a hit of a varied bar moves one step later.
const MOVE_CHANCE: f64 = 0.3;
/// Chance of a ghost note on an empty snare or hat step of a varied bar.
const GHOST_CHANCE: f64 = 0.2;

/// Where a step lies in its bar, coarse to fine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Position {
    /// The first beat of the bar.
    Down,
    /// A later even-numbered beat (beat 3 of 4/4).
    Strong,
    /// An odd-numbered beat (beats 2 and 4 of 4/4).
    Back,
    /// Halfway between two beats.
    Eighth,
    /// A quarter of a beat.
    Sixteenth,
    /// Anywhere else (triplets, swung steps).
    Other,
}

/// The bar of a time signature in ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Meter {
    pub bar_ticks: i64,
    pub beat_ticks: i64,
}

impl Meter {
    /// The bar of `num/den`; anything unusable is 4/4.
    pub fn new(num: u8, den: u8) -> Self {
        let (num, den) = if num == 0 || den == 0 {
            (4, 4)
        } else {
            (num, den)
        };
        let beat = (4 * QUARTER / i64::from(den)).max(4);
        Self {
            bar_ticks: beat * i64::from(num),
            beat_ticks: beat,
        }
    }

    /// The bar a tick lies in (counted from tick 0) and the tick's offset into it.
    pub fn locate(&self, tick: i64) -> (i64, i64) {
        (
            tick.div_euclid(self.bar_ticks),
            tick.rem_euclid(self.bar_ticks),
        )
    }

    /// The position class of an offset into the bar.
    pub fn position(&self, offset: i64) -> Position {
        let beat = offset / self.beat_ticks;
        let rem = offset % self.beat_ticks;
        if rem == 0 {
            if beat == 0 {
                Position::Down
            } else if beat % 2 == 1 {
                Position::Back
            } else {
                Position::Strong
            }
        } else if rem % (self.beat_ticks / 2).max(1) == 0 {
            Position::Eighth
        } else if rem % (self.beat_ticks / 4).max(1) == 0 {
            Position::Sixteenth
        } else {
            Position::Other
        }
    }
}

/// The weight (0..=1) of a role at a bar position; `late` is how far into the bar the step lies
/// (0..1). An original table: the kick favours the beat starts, snare and clap the backbeats, the
/// hats every eighth and sixteenth, the toms the end of the bar.
pub(crate) fn role_weight(role: DrumRole, pos: Position, late: f64) -> f64 {
    // Columns: Down, Strong, Back, Eighth, Sixteenth, Other.
    let row: [f64; 6] = match role {
        DrumRole::Kick => [1.0, 0.8, 0.15, 0.3, 0.1, 0.05],
        DrumRole::Snare => [0.05, 0.1, 1.0, 0.1, 0.12, 0.05],
        DrumRole::Clap => [0.05, 0.1, 0.9, 0.08, 0.08, 0.04],
        DrumRole::ClosedHat => [0.8, 0.8, 0.8, 1.0, 0.5, 0.25],
        DrumRole::OpenHat => [0.1, 0.1, 0.1, 0.7, 0.1, 0.05],
        DrumRole::TomLow | DrumRole::TomMid | DrumRole::TomHigh => [0.3, 0.4, 0.4, 0.6, 0.5, 0.3],
        DrumRole::Cymbal => [0.9, 0.1, 0.05, 0.05, 0.02, 0.01],
        DrumRole::Percussion => [0.3, 0.3, 0.4, 0.6, 0.5, 0.3],
    };
    let w = row[pos as usize];
    match role {
        DrumRole::TomLow | DrumRole::TomMid | DrumRole::TomHigh => w * (0.25 + 0.75 * late),
        _ => w,
    }
}

/// The params the rules read.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tuning {
    /// -1..=1.
    pub density: f64,
    /// 0..=1.
    pub variation: f64,
    /// 0 = off, else bars per group.
    pub fill_every: u32,
    /// 0..=1 of the last bar.
    pub fill_amount: f64,
    /// 0..=1.
    pub accent: f64,
    /// 0..=2.
    pub vel_scale: f64,
    pub seed_mix: u64,
}

fn unit(seed: u64, k: i64, slot: u64) -> f64 {
    unit_f64(hash64(seed, k as u64, slot))
}

/// Everything a rule needs to know about one step.
struct Spot {
    bar: i64,
    offset: i64,
    pos: Position,
    late: f64,
}

struct Rules<'a> {
    p: &'a DrumPattern,
    grid: Grid,
    t: &'a Tuning,
    m: &'a Meter,
}

impl Rules<'_> {
    fn spot(&self, k: i64) -> Spot {
        let (bar, offset) = self.m.locate(self.grid.start(k) as i64);
        Spot {
            bar,
            offset,
            pos: self.m.position(offset),
            late: offset as f64 / self.m.bar_ticks as f64,
        }
    }

    fn len(&self) -> i64 {
        i64::from(self.p.length.clamp(1, 64))
    }

    fn written(&self, lane: usize, k: i64) -> u8 {
        self.p.steps[lane][k.rem_euclid(self.len()) as usize].velocity
    }

    fn prob_pass(&self, lane: usize, k: i64) -> bool {
        let prob = self.p.steps[lane][k.rem_euclid(self.len()) as usize].prob;
        prob >= 100 || unit(self.t.seed_mix, k, SLOT_PROB + lane as u64) * 100.0 < f64::from(prob)
    }

    fn varied(&self, bar: i64) -> bool {
        self.t.variation > 0.0 && unit(self.t.seed_mix, bar, SLOT_VARIED) < self.t.variation
    }

    /// Whether the hit written at step `j` of `lane` moves to step `j + 1`.
    fn moves(&self, lane: usize, j: i64) -> bool {
        self.written(lane, j) > 0
            && self.p.lanes[lane].role != DrumRole::Kick
            && self.written(lane, j + 1) == 0
            && self.varied(self.spot(j).bar)
            && unit(self.t.seed_mix, j, SLOT_MOVE + lane as u64) < MOVE_CHANCE
    }

    /// Step 1: the hit that is written, moved and probability-tested.
    fn base(&self, lane: usize, k: i64) -> u8 {
        let src = if self.written(lane, k) > 0 && !self.moves(lane, k) {
            k
        } else if self.moves(lane, k - 1) {
            k - 1
        } else {
            return 0;
        };
        if self.prob_pass(lane, src) {
            self.written(lane, src)
        } else {
            0
        }
    }
}

/// The map: four corner patterns blended by an XY position. Corner A (pattern 1) sits at the
/// origin, B (pattern 2) at x = 1, C (pattern 3) at y = 1 and D (pattern 4) at both; a missing
/// corner is empty.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MapView<'a> {
    pub corners: [Option<&'a DrumPattern>; 4],
    /// 0..=1 (already mirrored).
    pub x: f64,
    /// 0..=1.
    pub y: f64,
    /// 0..=1: scales every hit chance (0 is silent).
    pub density: f64,
}

/// The base velocity (0 = no hit) of a lane under the map: each corner contributes the hit its
/// own pattern would play with weight `w_c` (the bilinear weight of the corner); the hit plays
/// when a hash of the seed, the global step and the lane is below the summed weight of the
/// corners that hit, times `density`; its velocity is the weighted mean of those corners'.
/// At a corner the weight is 0 or 1, so the hits are that pattern's own.
fn map_base(map: &MapView<'_>, lane: usize, k: i64, grid: Grid, t: &Tuning, m: &Meter) -> u8 {
    let (x, y) = (map.x.clamp(0.0, 1.0), map.y.clamp(0.0, 1.0));
    let weights = [(1.0 - x) * (1.0 - y), x * (1.0 - y), (1.0 - x) * y, x * y];
    let mut blend = 0.0;
    let mut vel_sum = 0.0;
    for (corner, w) in map.corners.iter().zip(weights) {
        let Some(p) = corner else { continue };
        if w <= 0.0 {
            continue;
        }
        let v = Rules { p, grid, t, m }.base(lane, k);
        if v > 0 {
            blend += w;
            vel_sum += w * f64::from(v);
        }
    }
    if blend <= 0.0 || unit(t.seed_mix, k, SLOT_MAP + lane as u64) >= blend * map.density {
        return 0;
    }
    (vel_sum / blend).round().clamp(1.0, 127.0) as u8
}

/// The velocity (0 = no hit) of `lane` at global step `k` of `p`, whose grid is `grid`.
pub(crate) fn hit_velocity(
    p: &DrumPattern,
    lane: usize,
    k: i64,
    grid: Grid,
    t: &Tuning,
    m: &Meter,
) -> u8 {
    let base = Rules { p, grid, t, m }.base(lane, k);
    hit_velocity_from(p, lane, k, (grid, t, m), base)
}

/// [`hit_velocity`] under the map for a lane the map does not lock.
pub(crate) fn mapped_hit_velocity(
    p: &DrumPattern,
    map: &MapView<'_>,
    lane: usize,
    k: i64,
    (grid, t, m): (Grid, &Tuning, &Meter),
) -> u8 {
    let base = map_base(map, lane, k, grid, t, m);
    hit_velocity_from(p, lane, k, (grid, t, m), base)
}

/// The rules after the written hit: `base` is what step 1 gave.
fn hit_velocity_from(
    p: &DrumPattern,
    lane: usize,
    k: i64,
    (grid, t, m): (Grid, &Tuning, &Meter),
    base: u8,
) -> u8 {
    let r = Rules { p, grid, t, m };
    let role = p.lanes[lane].role;
    let s = r.spot(k);
    let w = role_weight(role, s.pos, s.late);
    let lane_slot = lane as u64;
    let mut vel = f64::from(base);

    // Density.
    if vel == 0.0 && t.density > 0.0 {
        if unit(t.seed_mix, k, SLOT_ADD + lane_slot) < t.density * w {
            vel = (50.0 + 50.0 * w).round();
        }
    } else if vel > 0.0
        && t.density < 0.0
        && unit(t.seed_mix, k, SLOT_REMOVE + lane_slot) < -t.density * (1.0 - w)
    {
        vel = 0.0;
    }

    // Ghost notes.
    if vel == 0.0
        && matches!(role, DrumRole::Snare | DrumRole::ClosedHat)
        && r.varied(s.bar)
        && unit(t.seed_mix, k, SLOT_GHOST + lane_slot) < GHOST_CHANCE
    {
        let soft = 0.3 + 0.2 * unit(t.seed_mix, k, SLOT_GHOST_VEL + lane_slot);
        vel = (100.0 * soft).round();
    }

    // Fill.
    if t.fill_every > 0
        && t.fill_amount > 0.0
        && s.bar.rem_euclid(i64::from(t.fill_every)) == i64::from(t.fill_every) - 1
    {
        let bar = m.bar_ticks as f64;
        let from = bar * (1.0 - t.fill_amount.min(1.0));
        let at = s.offset as f64;
        if at >= from {
            let progress = if bar > from {
                (at - from) / (bar - from)
            } else {
                1.0
            };
            let ramp = (45.0 + 70.0 * progress).round();
            let chance = unit(t.seed_mix, k, SLOT_FILL + lane_slot);
            match role {
                DrumRole::Snare => vel = vel.max(ramp),
                DrumRole::Clap if chance < 0.35 => vel = vel.max(ramp),
                DrumRole::TomLow | DrumRole::TomMid | DrumRole::TomHigh if chance < 0.5 => {
                    vel = vel.max((ramp * 0.9).round());
                }
                DrumRole::ClosedHat | DrumRole::OpenHat | DrumRole::Cymbal if chance < 0.5 => {
                    vel = 0.0
                }
                DrumRole::Kick if s.offset == m.bar_ticks - m.beat_ticks => vel = vel.max(110.0),
                _ => {}
            }
        }
    }

    // Accent and scale.
    if vel > 0.0 {
        if t.accent > 0.0 && w >= 0.8 {
            vel *= 1.0 + 0.5 * t.accent;
        }
        vel = (vel * t.vel_scale).round().clamp(1.0, 127.0);
    }
    vel as u8
}
