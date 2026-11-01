//! Step tools: shift, randomise, alter and invert the pad rows of a drum machine clip as seen in
//! the step editor's grid.
//!
//! A tool takes the notes of a clip and its [`StepGrid`] and returns the new notes; the editor
//! sends them as one `SetClipContent`, so a tool is one undo step. Only **on-grid** notes are
//! touched: a note whose key is a pad key and whose tick lies in `[0, steps * s)`. Everything
//! else (other keys, notes past the grid) comes back unchanged, like every other grid edit.
//!
//! A cell is on when a note of the pad's key starts inside its step (the unswung tick in
//! `[k * s, (k + 1) * s)`); a cell a tool turns on holds one note at the step start, one step
//! long, at the velocity the caller gives. Every function is pure: the random tools take a seed
//! and draw each decision from [`hash64`] of `(seed, pad, step)`, so the same notes and the same
//! seed always give the same result, and another seed is another take.

use crate::groove::hash64;
use crate::song::{Note, StepGrid};
use crate::zones::{PAD_COUNT, pad_key, pad_of_key};

/// Smallest velocity of a note a tool adds.
const MIN_ADDED: f32 = 1.0 / 127.0;

/// One step edit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StepTool {
    /// Moves every on-grid note this many steps later (negative: earlier), wrapping around the
    /// end of the grid. A note keeps its offset inside its step and its length.
    Shift(i32),
    /// Replaces the hits of the chosen pads: each cell turns on with probability `density`
    /// (`0..=1`) at a velocity between 70 % and 100 % of the given one.
    Randomise {
        /// The chance of a cell being on.
        density: f32,
        /// Another seed is another take.
        seed: u32,
    },
    /// Flips each cell of the chosen pads with probability `chance` (`0..=1`): a hit goes, a gap
    /// becomes a hit.
    Alter {
        /// The chance of a cell flipping.
        chance: f32,
        /// Another seed is another take.
        seed: u32,
    },
    /// Turns every hit of the chosen pads into a gap and every gap into a hit.
    Invert,
}

impl StepTool {
    /// Checks every range; `Err` says what is wrong.
    pub fn validate(&self) -> Result<(), String> {
        let unit = |what: &str, v: f32| {
            if (0.0..=1.0).contains(&v) {
                Ok(())
            } else {
                Err(format!("{what} {v} is outside 0..=1"))
            }
        };
        match *self {
            StepTool::Shift(_) | StepTool::Invert => Ok(()),
            StepTool::Randomise { density, .. } => unit("density", density),
            StepTool::Alter { chance, .. } => unit("chance", chance),
        }
    }
}

fn on_grid(n: &Note, steps: i64, s: i64) -> bool {
    pad_of_key(n.key).is_some() && n.tick >= 0 && n.tick < steps * s
}

fn pad_bit(pad: u8) -> u16 {
    1 << (pad.clamp(1, PAD_COUNT) - 1)
}

/// The pads (as a bit set, bit 0 = pad 1) that have at least one on-grid note.
pub fn pads_in_use(notes: &[Note], grid: StepGrid) -> u16 {
    let (steps, s) = (i64::from(grid.steps), grid.resolution.ticks());
    notes
        .iter()
        .filter(|n| on_grid(n, steps, s))
        .filter_map(|n| pad_of_key(n.key))
        .fold(0, |bits, pad| bits | pad_bit(pad))
}

/// A uniform number in `0..1` for `(seed, pad, step)`, from the top 53 bits of the hash.
fn unit(seed: u32, pad: u8, step: i64, salt: u64) -> f64 {
    let h = hash64(u64::from(seed) ^ salt, u64::from(pad), step as u64);
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Applies `tool` to the on-grid notes of the pads in `pads` (a bit set, bit 0 = pad 1; the shift
/// ignores it and moves every pad). `velocity` (`0..=1`) is the velocity of added notes. The
/// result is sorted by tick then key.
pub fn apply_step_tool(
    notes: &[Note],
    grid: StepGrid,
    tool: StepTool,
    pads: u16,
    velocity: f32,
) -> Vec<Note> {
    let (steps, s) = (i64::from(grid.steps.max(1)), grid.resolution.ticks());
    let level = velocity.clamp(MIN_ADDED, 1.0);
    let mut out: Vec<Note> = Vec::with_capacity(notes.len());
    // Notes the tool does not touch.
    let touched = |n: &Note| -> bool {
        on_grid(n, steps, s)
            && match tool {
                StepTool::Shift(_) => true,
                _ => pad_of_key(n.key).is_some_and(|p| pads & pad_bit(p) != 0),
            }
    };
    match tool {
        StepTool::Shift(by) => {
            let by = i64::from(by).rem_euclid(steps);
            for n in notes {
                if touched(n) {
                    let step = n.tick / s;
                    let within = n.tick - step * s;
                    let to = (step + by).rem_euclid(steps);
                    out.push(Note {
                        tick: to * s + within,
                        ..*n
                    });
                } else {
                    out.push(*n);
                }
            }
        }
        StepTool::Randomise { density, seed } => {
            out.extend(notes.iter().filter(|n| !touched(n)).copied());
            for pad in 1..=PAD_COUNT {
                if pads & pad_bit(pad) == 0 {
                    continue;
                }
                for step in 0..steps {
                    if unit(seed, pad, step, 0) < f64::from(density) {
                        let v = level * (0.7 + 0.3 * unit(seed, pad, step, 0x5555) as f32);
                        out.push(Note::new(step * s, s, pad_key(pad), v.max(MIN_ADDED)));
                    }
                }
            }
        }
        StepTool::Alter { chance, seed } => {
            for pad in 1..=PAD_COUNT {
                if pads & pad_bit(pad) == 0 {
                    continue;
                }
                for step in 0..steps {
                    if unit(seed, pad, step, 0) >= f64::from(chance) {
                        continue;
                    }
                    if cell_on(notes, pad, step, s) {
                        // The cell goes: its notes are dropped below.
                    } else {
                        out.push(Note::new(step * s, s, pad_key(pad), level));
                    }
                }
            }
            // The notes of every cell that stays, and every untouched note.
            for n in notes {
                let flipped = touched(n)
                    && pad_of_key(n.key).is_some_and(|p| {
                        let step = n.tick / s;
                        unit(seed, p, step, 0) < f64::from(chance)
                    });
                if !flipped {
                    out.push(*n);
                }
            }
        }
        StepTool::Invert => {
            for n in notes {
                if !touched(n) {
                    out.push(*n);
                }
            }
            for pad in 1..=PAD_COUNT {
                if pads & pad_bit(pad) == 0 {
                    continue;
                }
                for step in 0..steps {
                    if !cell_on(notes, pad, step, s) {
                        out.push(Note::new(step * s, s, pad_key(pad), level));
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.tick.cmp(&b.tick).then(a.key.cmp(&b.key)));
    out
}

fn cell_on(notes: &[Note], pad: u8, step: i64, s: i64) -> bool {
    let key = pad_key(pad);
    notes
        .iter()
        .any(|n| n.key == key && n.tick >= step * s && n.tick < (step + 1) * s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::StepResolution;

    const S: i64 = 3_840;

    fn grid(steps: u16) -> StepGrid {
        StepGrid {
            resolution: StepResolution::Sixteenth,
            steps,
            swing: 0,
        }
    }

    fn hit(pad: u8, step: i64) -> Note {
        Note::new(step * S, S, pad_key(pad), 0.8)
    }

    fn cells(notes: &[Note]) -> Vec<(u8, i64)> {
        notes
            .iter()
            .map(|n| (pad_of_key(n.key).unwrap_or(0), n.tick / S))
            .collect()
    }

    const ALL: u16 = 0xFFFF;

    #[test]
    fn shift_wraps_around_the_grid_and_keeps_offsets_and_lengths() {
        let mut late = hit(1, 15);
        late.tick += 100;
        late.length = 1_000;
        let notes = [hit(1, 0), late, hit(3, 7)];
        let right = apply_step_tool(&notes, grid(16), StepTool::Shift(1), ALL, 0.8);
        assert_eq!(cells(&right), [(1, 0), (1, 1), (3, 8)]);
        let wrapped = right.iter().find(|n| n.tick == 100).unwrap();
        assert_eq!((wrapped.length, wrapped.key), (1_000, pad_key(1)));
        let left = apply_step_tool(&notes, grid(16), StepTool::Shift(-1), ALL, 0.8);
        assert_eq!(cells(&left), [(3, 6), (1, 14), (1, 15)]);
        // A full turn and a turn by the grid length are the identity.
        for by in [0, 16, -16, 32] {
            let same = apply_step_tool(&notes, grid(16), StepTool::Shift(by), ALL, 0.8);
            assert_eq!(same, sorted(&notes), "shift by {by}");
        }
        // Shifting forth and back restores the notes.
        let there = apply_step_tool(&notes, grid(16), StepTool::Shift(5), ALL, 0.8);
        let back = apply_step_tool(&there, grid(16), StepTool::Shift(-5), ALL, 0.8);
        assert_eq!(back, sorted(&notes));
    }

    fn sorted(notes: &[Note]) -> Vec<Note> {
        let mut v = notes.to_vec();
        v.sort_by(|a, b| a.tick.cmp(&b.tick).then(a.key.cmp(&b.key)));
        v
    }

    #[test]
    fn notes_off_the_grid_are_never_touched() {
        let off_key = Note::new(0, S, 20, 0.5);
        let past = Note::new(40 * S, S, pad_key(2), 0.5);
        let notes = [hit(1, 3), off_key, past];
        for tool in [
            StepTool::Shift(3),
            StepTool::Randomise {
                density: 1.0,
                seed: 1,
            },
            StepTool::Alter {
                chance: 1.0,
                seed: 1,
            },
            StepTool::Invert,
        ] {
            let out = apply_step_tool(&notes, grid(16), tool, ALL, 0.8);
            assert!(out.contains(&off_key), "{tool:?}");
            assert!(out.contains(&past), "{tool:?}");
        }
    }

    #[test]
    fn invert_swaps_hits_and_gaps_on_the_chosen_pads_only() {
        let notes = [hit(1, 0), hit(1, 4), hit(2, 1)];
        let out = apply_step_tool(&notes, grid(16), StepTool::Invert, 0b01, 0.6);
        let pad1: Vec<i64> = out
            .iter()
            .filter(|n| n.key == pad_key(1))
            .map(|n| n.tick / S)
            .collect();
        assert_eq!(pad1.len(), 14);
        assert!(!pad1.contains(&0) && !pad1.contains(&4) && pad1.contains(&1));
        assert!(out.contains(&hit(2, 1)), "pad 2 is not chosen");
        assert!(
            out.iter()
                .any(|n| n.key == pad_key(1) && (n.velocity - 0.6).abs() < 1e-6)
        );
        // Twice is the identity for the cells (velocities of added notes aside).
        let twice = apply_step_tool(&out, grid(16), StepTool::Invert, 0b01, 0.8);
        let mut a: Vec<(u8, i64)> = cells(&twice);
        let mut b = cells(&sorted(&notes));
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
    }

    #[test]
    fn randomise_is_a_pure_function_of_the_seed() {
        let notes = [hit(1, 0)];
        let tool = |seed| StepTool::Randomise { density: 0.5, seed };
        let a = apply_step_tool(&notes, grid(32), tool(7), 0b101, 0.9);
        assert_eq!(a, apply_step_tool(&notes, grid(32), tool(7), 0b101, 0.9));
        assert_ne!(a, apply_step_tool(&notes, grid(32), tool(8), 0b101, 0.9));
        assert!(a.iter().all(|n| n.key == pad_key(1) || n.key == pad_key(3)));
        let n1 = a.iter().filter(|n| n.key == pad_key(1)).count();
        assert!((6..=26).contains(&n1), "about half of 32 cells: {n1}");
        assert!(a.iter().all(|n| (0.6..=0.9).contains(&n.velocity)));
        // One note per cell, inside the grid.
        let mut seen = std::collections::BTreeSet::new();
        for n in &a {
            assert!(n.tick >= 0 && n.tick < 32 * S && seen.insert((n.key, n.tick)));
        }
        let none = apply_step_tool(&notes, grid(16), tool(1), 0b1, 0.9);
        assert_eq!(
            apply_step_tool(
                &notes,
                grid(16),
                StepTool::Randomise {
                    density: 0.0,
                    seed: 1
                },
                0b1,
                0.9
            ),
            Vec::new(),
            "density 0 clears the chosen pads"
        );
        assert!(!none.is_empty());
        let all = apply_step_tool(
            &notes,
            grid(16),
            StepTool::Randomise {
                density: 1.0,
                seed: 1,
            },
            0b1,
            0.9,
        );
        assert_eq!(all.len(), 16);
    }

    #[test]
    fn alter_flips_about_the_chance_and_nothing_at_zero() {
        let notes: Vec<Note> = (0..16).step_by(2).map(|i| hit(1, i)).collect();
        let none = apply_step_tool(
            &notes,
            grid(16),
            StepTool::Alter {
                chance: 0.0,
                seed: 3,
            },
            0b1,
            0.8,
        );
        assert_eq!(none, sorted(&notes));
        let all = apply_step_tool(
            &notes,
            grid(16),
            StepTool::Alter {
                chance: 1.0,
                seed: 3,
            },
            0b1,
            0.8,
        );
        let inv = apply_step_tool(&notes, grid(16), StepTool::Invert, 0b1, 0.8);
        assert_eq!(cells(&all), cells(&inv), "a chance of 1 is an invert");
        let some = |seed| {
            apply_step_tool(
                &notes,
                grid(16),
                StepTool::Alter { chance: 0.25, seed },
                0b1,
                0.8,
            )
        };
        assert_eq!(some(5), some(5));
        assert_ne!(some(5), some(6));
        let changed = cells(&some(5))
            .iter()
            .filter(|c| !cells(&notes).contains(c))
            .count()
            + cells(&notes)
                .iter()
                .filter(|c| !cells(&some(5)).contains(c))
                .count();
        assert!((1..=10).contains(&changed), "{changed}");
    }

    #[test]
    fn pads_in_use_and_validation() {
        let notes = [hit(1, 0), hit(16, 3), Note::new(0, S, 20, 0.5), hit(2, 99)];
        assert_eq!(pads_in_use(&notes, grid(16)), 0b1000_0000_0000_0001);
        assert!(
            StepTool::Randomise {
                density: 1.5,
                seed: 0
            }
            .validate()
            .is_err()
        );
        assert!(
            StepTool::Alter {
                chance: -0.1,
                seed: 0
            }
            .validate()
            .is_err()
        );
        assert!(StepTool::Shift(-3).validate().is_ok());
    }
}
