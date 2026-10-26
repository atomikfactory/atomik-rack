//! Writing a pattern: the drum generator's role templates, thinned or thickened by density and
//! jittered by a seed. A pure function of its arguments, so the same seed writes the same
//! pattern on every machine (the app's Generate and Randomise commands rely on it).

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

fn unit(seed: u64, step: usize, lane: usize, slot: u64) -> f64 {
    unit_f64(hash64(seed, step as u64, (slot << 8) | lane as u64))
}

/// A pattern like `base` (its length, resolution, swing and lanes) with new steps: each role's
/// template of `style` played at the right positions of a 4/4 bar, then `density` (-1..=1)
/// thins the hits (below zero, weak positions first) or adds some on empty steps (above zero,
/// strong positions first), and `seed` draws every chance and jitters the velocities. Only the
/// first lane of a role gets the template, so two snares do not double. Every probability is 100.
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
            let (_, offset) = meter.locate(grid.start(step as i64) as i64);
            let pos = meter.position(offset);
            let w = role_weight(role, pos, offset as f64 / meter.bar_ticks as f64);
            // The template slot: a sixteenth of the bar, when the step lies on one.
            let slot = (offset % 3_840 == 0).then_some((offset / 3_840) as usize);
            let templated = slot.is_some_and(|s| template.get(s) == Some(&b'x'));
            let hit = if templated {
                !(density < 0.0 && unit(seed, step, lane, SLOT_REMOVE) < -density * (1.0 - w))
            } else {
                density > 0.0 && unit(seed, step, lane, SLOT_ADD) < density * w * 0.6
            };
            if !hit {
                continue;
            }
            let mut v = role_velocity(role) * if templated { 1.0 } else { 0.7 };
            match pos {
                Position::Down => v += 8.0,
                Position::Sixteenth | Position::Other => v -= 14.0,
                _ => {}
            }
            v += (unit(seed, step, lane, SLOT_JITTER) - 0.5) * 12.0;
            out.steps[lane][step].velocity = v.round().clamp(1.0, 127.0) as u8;
        }
    }
    out
}
