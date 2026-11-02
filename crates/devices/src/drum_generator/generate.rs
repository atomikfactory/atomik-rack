//! Writing a pattern: the drum generator's role templates, varied and jittered by a seed and
//! thinned or thickened by density. A pure function of its arguments, so the same seed writes the
//! same pattern on every machine (the app's Generate and Randomise commands rely on it).

use atomik_device_sdk::{DrumPattern, DrumRole, DrumStep, MAX_STEPS};
use atomik_dsp::rng::{hash64, unit_f64};

use super::algo::{Meter, Position, role_weight};
use crate::note_clock::Grid;

/// A starting rhythm for the generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrumStyle {
    /// Kick on every beat, clap on the backbeats, hats on the eighths.
    #[default]
    FourOnTheFloor,
    /// A syncopated kick against a backbeat snare.
    BrokenBeat,
    /// The snare once a bar, on the third beat.
    HalfTime,
    /// A plain kick and snare pattern with open hats on the off beats.
    Straight,
}

impl DrumStyle {
    /// Every style, in the order of the device's `style` choice.
    pub const ALL: [Self; 4] = [
        Self::FourOnTheFloor,
        Self::BrokenBeat,
        Self::HalfTime,
        Self::Straight,
    ];

    /// The style at `index` in [`DrumStyle::ALL`], the first for an index beyond it.
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// The index in [`DrumStyle::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }

    /// One bar of sixteenths for a role; `x` is a hit, anything else is empty.
    fn template(self, role: DrumRole) -> &'static str {
        match (self, role) {
            (Self::FourOnTheFloor, DrumRole::Kick) => "x...x...x...x...",
            (Self::FourOnTheFloor, DrumRole::Clap) => "....x.......x...",
            (Self::FourOnTheFloor, DrumRole::ClosedHat) => "x.x.x.x.x.x.x.x.",
            (Self::FourOnTheFloor, DrumRole::OpenHat) => "..x...x...x...x.",
            (Self::BrokenBeat, DrumRole::Kick) => "x..x..x...x..x..",
            (Self::BrokenBeat, DrumRole::Snare) => "....x.......x...",
            (Self::BrokenBeat, DrumRole::ClosedHat) => "x.x.x.x.x.x.x.x.",
            (Self::HalfTime, DrumRole::Kick) => "x.......x.x.....",
            (Self::HalfTime, DrumRole::Snare) => "........x.......",
            (Self::HalfTime, DrumRole::ClosedHat) => "x.x.x.x.x.x.x.x.",
            (Self::Straight, DrumRole::Kick) => "x.......x.x.....",
            (Self::Straight, DrumRole::Snare) => "....x.......x...",
            (Self::Straight, DrumRole::ClosedHat) => "x.x.x.x.x.x.x.x.",
            (Self::Straight, DrumRole::OpenHat) => "......x.......x.",
            _ => "",
        }
    }
}

/// The usual velocity of a role.
fn role_velocity(role: DrumRole) -> f64 {
    match role {
        DrumRole::Kick => 112.0,
        DrumRole::Snare => 104.0,
        DrumRole::Clap => 100.0,
        DrumRole::ClosedHat => 82.0,
        DrumRole::OpenHat => 90.0,
        DrumRole::TomLow | DrumRole::TomMid | DrumRole::TomHigh => 96.0,
        DrumRole::Cymbal => 100.0,
        DrumRole::Percussion => 84.0,
    }
}

const SLOT_JITTER: u64 = 0;
const SLOT_ADD: u64 = 1;
const SLOT_REMOVE: u64 = 2;
/// Slot of the seed's overall liveliness (one draw per pattern).
const SLOT_LIVELY: u64 = 3;
/// Slot of the draw that decides whether the end of the pattern gets a fill.
const SLOT_FILL: u64 = 4;
/// Slot of the per-step variation chance (every rule of [`vary`] reads its own step and lane).
const SLOT_VARY: u64 = 5;
/// Slot of the velocity of a step the variation added.
const SLOT_VARY_VEL: u64 = 6;
/// Slot of the accent chance of a templated step.
const SLOT_ACCENT: u64 = 7;

fn unit(seed: u64, step: usize, lane: usize, slot: u64) -> f64 {
    unit_f64(hash64(seed, step as u64, (slot << 8) | lane as u64))
}

/// What the seed's variation does to one step of a role.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Vary {
    /// The template (or nothing) stands.
    Keep,
    /// A templated hit is dropped.
    Drop,
    /// An empty step gains a hit of this velocity.
    Add(f64),
    /// A templated hit is played harder by this much.
    Accent(f64),
}

/// The sixteenth slots (of a 4/4 bar) where each role may gain a hit, by style.
fn ghost_slots(style: DrumStyle, role: DrumRole) -> &'static [usize] {
    match (style, role) {
        (_, DrumRole::Kick) => match style {
            // Four on the floor keeps its four kicks; the others pick up on weak sixteenths.
            DrumStyle::FourOnTheFloor => &[],
            DrumStyle::BrokenBeat => &[2, 8, 11, 15],
            DrumStyle::HalfTime => &[3, 6, 14, 15],
            DrumStyle::Straight => &[3, 6, 7, 14, 15],
        },
        (_, DrumRole::Snare) => &[1, 3, 6, 7, 9, 11, 14, 15],
        (_, DrumRole::Clap) => &[7, 14, 15],
        (_, DrumRole::ClosedHat) => &[1, 3, 5, 7, 9, 11, 13, 15],
        (_, DrumRole::OpenHat) => &[2, 6, 10, 14],
        (_, DrumRole::Percussion) => &[3, 5, 7, 10, 11, 13, 15],
        (_, DrumRole::TomLow | DrumRole::TomMid | DrumRole::TomHigh | DrumRole::Cymbal) => &[],
    }
}

/// Where the seed varies the role at sixteenth `s` of a bar: ghost notes on the snare, extra kicks
/// on weak sixteenths, hat and percussion embellishments, an open hat that wanders, an odd
/// removal on a weak hat or kick, and (on the last bar of the pattern, when `fill` is set) a
/// short fill of snare and toms. The kick on the one and the backbeats of the template are never
/// touched. `lively` (0.5..1.5) scales every chance, so some seeds are sparse and others busy.
#[allow(clippy::too_many_arguments)]
fn vary(
    style: DrumStyle,
    role: DrumRole,
    s: usize,
    templated: bool,
    last_bar: bool,
    fill: bool,
    lively: f64,
    draw: f64,
    shade: f64,
    accent: f64,
) -> Vary {
    // The end-of-pattern fill: snare on the last sixteenths, toms stepping down.
    if last_bar && fill && s >= 12 {
        let (chance, vel) = match role {
            DrumRole::Snare => (0.55, 70.0 + 10.0 * (s - 12) as f64),
            DrumRole::TomHigh if s == 12 || s == 13 => (0.45, 84.0),
            DrumRole::TomMid if s == 13 || s == 14 => (0.45, 88.0),
            DrumRole::TomLow if s == 14 || s == 15 => (0.45, 92.0),
            DrumRole::Cymbal if s == 15 => (0.25, 96.0),
            _ => (0.0, 0.0),
        };
        if !templated && draw < chance * lively {
            return Vary::Add(vel + 10.0 * shade);
        }
    }
    if templated {
        let weak = match role {
            DrumRole::Kick => s != 0 && !s.is_multiple_of(4),
            DrumRole::ClosedHat => s % 4 == 2,
            DrumRole::OpenHat => true,
            _ => false,
        };
        if weak && draw < 0.18 * lively {
            return Vary::Drop;
        }
        if matches!(role, DrumRole::ClosedHat | DrumRole::OpenHat) && accent < 0.25 * lively {
            return Vary::Accent(10.0);
        }
        return Vary::Keep;
    }
    let chance = match role {
        DrumRole::Snare => 0.20,
        DrumRole::Clap => 0.12,
        DrumRole::Kick => 0.12,
        DrumRole::ClosedHat => 0.16,
        DrumRole::OpenHat => 0.12,
        DrumRole::Percussion => 0.22,
        _ => 0.0,
    };
    if ghost_slots(style, role).contains(&s) && draw < chance * lively {
        let vel = match role {
            DrumRole::Snare | DrumRole::Clap => 30.0 + 18.0 * shade,
            DrumRole::Kick => 70.0 + 20.0 * shade,
            DrumRole::ClosedHat => 46.0 + 20.0 * shade,
            DrumRole::OpenHat => 64.0 + 14.0 * shade,
            _ => 54.0 + 26.0 * shade,
        };
        return Vary::Add(vel);
    }
    Vary::Keep
}

/// A pattern like `base` (its length, resolution, swing and lanes) with new steps: each role's
/// template of `style` played at the right positions of a 4/4 bar, then varied by `seed` (ghost
/// notes, weak-position kicks, hat and percussion embellishments, odd removals on weak hats, and
/// sometimes a short fill at the end of the pattern; the kick on the one and the backbeats stay),
/// then `density` (-1..=1) thins the hits (below zero, weak positions first) or adds some on empty
/// steps (above zero, strong positions first). `seed` draws every chance and jitters the
/// velocities, so two seeds write two rhythms even at density 0. Only the first lane of a role
/// gets the template, so two snares do not double. Every probability is 100.
pub fn generate_pattern(
    base: &DrumPattern,
    style: DrumStyle,
    density: f64,
    seed: u64,
) -> DrumPattern {
    let mut out = *base;
    let density = if density.is_finite() {
        density.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let meter = Meter::new(4, 4);
    let grid = Grid {
        s: base.resolution.step_ticks(),
        swing: 0,
    };
    let len = usize::from(base.length.clamp(1, MAX_STEPS as u8));
    let last_bar = meter.locate(grid.start(len as i64 - 1) as i64).0;
    let lively = 0.5 + unit(seed, 0, 0, SLOT_LIVELY);
    let fill = unit(seed, 0, 0, SLOT_FILL) < 0.55;
    let mut seen = [false; DrumRole::ALL.len()];
    for lane in 0..base.lanes.len() {
        let role = base.lanes[lane].role;
        let first = !std::mem::replace(&mut seen[role.index()], true);
        let template = if first {
            style.template(role).as_bytes()
        } else {
            &[]
        };
        out.steps[lane] = [DrumStep::default(); MAX_STEPS];
        for step in 0..MAX_STEPS {
            if step >= len {
                continue;
            }
            let (bar, offset) = meter.locate(grid.start(step as i64) as i64);
            let pos = meter.position(offset);
            let w = role_weight(role, pos, offset as f64 / meter.bar_ticks as f64);
            // The template slot: a sixteenth of the bar, when the step lies on one.
            let slot = (offset % 3_840 == 0).then_some((offset / 3_840) as usize);
            let templated = slot.is_some_and(|s| template.get(s) == Some(&b'x'));
            let varied = match slot.filter(|s| first && *s < 16) {
                Some(s) => vary(
                    style,
                    role,
                    s,
                    templated,
                    bar == last_bar,
                    fill,
                    lively,
                    unit(seed, step, lane, SLOT_VARY),
                    unit(seed, step, lane, SLOT_VARY_VEL),
                    unit(seed, step, lane, SLOT_ACCENT),
                ),
                None => Vary::Keep,
            };
            let added = match varied {
                Vary::Add(v) => Some(v),
                _ => None,
            };
            let present = match varied {
                Vary::Drop => false,
                Vary::Add(_) => true,
                _ => templated,
            };
            let hit = if present {
                !(density < 0.0 && unit(seed, step, lane, SLOT_REMOVE) < -density * (1.0 - w))
            } else {
                density > 0.0 && unit(seed, step, lane, SLOT_ADD) < density * w * 0.6
            };
            if !hit {
                continue;
            }
            let mut v = match added {
                Some(v) => v,
                None => {
                    let mut v = role_velocity(role) * if templated { 1.0 } else { 0.7 };
                    match pos {
                        Position::Down => v += 8.0,
                        Position::Sixteenth | Position::Other => v -= 14.0,
                        _ => {}
                    }
                    if let Vary::Accent(a) = varied {
                        v += a;
                    }
                    v
                }
            };
            v += (unit(seed, step, lane, SLOT_JITTER) - 0.5) * 12.0;
            out.steps[lane][step].velocity = v.round().clamp(1.0, 127.0) as u8;
        }
    }
    out
}
