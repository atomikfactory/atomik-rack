//! The pure rules of typing into a tab: note values, the cursor's steps on the value grid, and
//! the buffer that turns two quick digits into one fret.
//!
//! Nothing here knows a view, a clip or a clock: the view feeds the buffer the time it was last
//! ticked with, so the rules are tested with plain numbers.

use crate::timeline::{Bars, PPQ};

/// The note values the entry offers, longest first (the divisor of a whole note).
pub const DIVISIONS: [u8; 7] = [1, 2, 4, 8, 16, 32, 64];

/// A note value: the length a typed note gets and the step the cursor moves by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteValue {
    /// The divisor of a whole note: 1 whole, 2 half, 4 quarter … 64 sixty-fourth.
    pub division: u8,
    /// Lengthened by half.
    pub dotted: bool,
    /// Three in the time of two.
    pub triplet: bool,
}

impl NoteValue {
    /// A quarter note, the entry's start value.
    pub const QUARTER: NoteValue = NoteValue {
        division: 4,
        dotted: false,
        triplet: false,
    };

    /// A plain note value (`division` is one of [`DIVISIONS`]).
    pub fn plain(division: u8) -> Self {
        NoteValue {
            division: if DIVISIONS.contains(&division) {
                division
            } else {
                4
            },
            dotted: false,
            triplet: false,
        }
    }

    /// The length in ticks.
    pub fn ticks(self) -> i64 {
        let base = 4 * PPQ / i64::from(self.division.max(1));
        if self.dotted {
            base * 3 / 2
        } else if self.triplet {
            base * 2 / 3
        } else {
            base
        }
    }

    /// The value named by a command argument (`quarter`, `thirty_second` …).
    pub fn from_name(name: &str) -> Option<Self> {
        let division = match name {
            "whole" => 1,
            "half" => 2,
            "quarter" => 4,
            "eighth" => 8,
            "sixteenth" => 16,
            "thirty_second" => 32,
            "sixty_fourth" => 64,
            _ => return None,
        };
        Some(Self::plain(division))
    }

    /// The same value with the dot toggled (a triplet and a dot exclude each other).
    pub fn toggle_dotted(self) -> Self {
        Self {
            dotted: !self.dotted,
            triplet: false,
            ..self
        }
    }

    /// The same value with the triplet toggled.
    pub fn toggle_triplet(self) -> Self {
        Self {
            triplet: !self.triplet,
            dotted: false,
            ..self
        }
    }

    /// The text of the readout: `1/4`, `1/8.` (dotted), `1/16T` (triplet); `1` for a whole note.
    pub fn label(self) -> String {
        let mut s = if self.division == 1 {
            "1".to_string()
        } else {
            format!("1/{}", self.division)
        };
        if self.dotted {
            s.push('.');
        }
        if self.triplet {
            s.push('T');
        }
        s
    }

    /// How many flags (or beams) the note's stem carries: 0 for a quarter or longer, 1 for an
    /// eighth …, 4 for a sixty-fourth.
    pub fn flags(self) -> u8 {
        match self.division {
            8 => 1,
            16 => 2,
            32 => 3,
            64 => 4,
            _ => 0,
        }
    }

    /// The note value that best names a span of `ticks`: the longest value that is not longer
    /// than the span (with a small tolerance for a note a hair short), else the shortest.
    pub fn classify(ticks: i64) -> Self {
        let slack = ticks / 32;
        let mut best: Option<(i64, NoteValue)> = None;
        for division in DIVISIONS {
            for (dotted, triplet) in [(false, false), (true, false), (false, true)] {
                let v = NoteValue {
                    division,
                    dotted,
                    triplet,
                };
                let t = v.ticks();
                if t <= ticks + slack && best.is_none_or(|(b, _)| t > b) {
                    best = Some((t, v));
                }
            }
        }
        best.map_or_else(|| NoteValue::plain(64), |(_, v)| v)
    }
}

impl Default for NoteValue {
    fn default() -> Self {
        Self::QUARTER
    }
}

/// The tick of the next (`dir > 0`) or previous (`dir < 0`) point of the grid of `step` ticks,
/// from `tick`; a tick between two points goes to the point it is moving toward. Never below 0
/// and never at or past `end` (the cursor stays inside the clip).
pub fn step_on_grid(tick: i64, step: i64, dir: i32, end: i64) -> i64 {
    let step = step.max(1);
    let last = ((end - 1).max(0) / step) * step;
    let next = if dir >= 0 {
        (tick.div_euclid(step) + 1) * step
    } else if tick.rem_euclid(step) == 0 {
        tick - step
    } else {
        tick.div_euclid(step) * step
    };
    next.clamp(0, last)
}

/// `tick` moved to the nearest point of the grid of `step` ticks.
pub fn snap_to_grid(tick: i64, step: i64) -> i64 {
    let step = step.max(1);
    ((tick + step / 2).div_euclid(step) * step).max(0)
}

/// The start of the bar after (`dir > 0`) or before (`dir < 0`) `tick`, kept inside `[0, end)`.
pub fn step_bar(tick: i64, dir: i32, bars: &Bars, end: i64) -> i64 {
    let t = if dir >= 0 {
        bars.next_bar_line(tick)
    } else {
        bars.prev_bar_line(tick)
    };
    t.clamp(0, (end - 1).max(0))
}

/// How long two digits may be apart and still be one fret, in seconds.
pub const FRET_TIMEOUT: f64 = 0.6;

/// The fret being typed: digits of one fret on one cell, joined while they come quickly.
#[derive(Debug, Clone, Default)]
pub struct FretBuffer {
    value: u16,
    cell: Option<(i64, u8)>,
    /// When the buffer expires; set by the first [`FretBuffer::tick`] after a digit, so the
    /// window starts when the frame clock next moves, not at a time the key did not carry.
    deadline: Option<f64>,
    clock: f64,
}

/// What a digit did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Typed {
    /// The fret now typed.
    pub fret: u8,
    /// Whether the digit extended the previous one (the edit joins the previous undo step).
    pub joined: bool,
}

impl FretBuffer {
    /// Adds `digit` for the fret on `cell` (a tick and a string); `max_fret` is the highest fret
    /// of the string. A digit that would make the fret too high, or that comes after the window,
    /// or on another cell, starts a new fret.
    pub fn push(&mut self, digit: u8, cell: (i64, u8), max_fret: u8) -> Typed {
        let alive = self.cell == Some(cell) && self.deadline.is_none_or(|d| self.clock <= d);
        let joined_value = u16::from(digit) + self.value * 10;
        // A leading zero never extends (0 is the open string).
        let joined = alive && self.value > 0 && joined_value <= u16::from(max_fret);
        self.value = if joined {
            joined_value
        } else {
            u16::from(digit)
        };
        self.cell = Some(cell);
        self.deadline = None;
        Typed {
            fret: u8::try_from(self.value).unwrap_or(u8::MAX),
            joined,
        }
    }

    /// The frame clock: starts the window of a fresh digit and ends the one that ran out.
    /// Returns true when the buffer just expired (the host ends the merged undo step).
    pub fn tick(&mut self, now: f64) -> bool {
        self.clock = now;
        if self.cell.is_none() {
            return false;
        }
        let deadline = *self.deadline.get_or_insert(now + FRET_TIMEOUT);
        if now > deadline {
            self.clear();
            return true;
        }
        false
    }

    /// Drops what is typed (another cell, a command, Esc).
    pub fn clear(&mut self) {
        self.value = 0;
        self.cell = None;
        self.deadline = None;
    }

    /// Whether digits are waiting for a next one.
    pub fn is_open(&self) -> bool {
        self.cell.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_values_have_their_lengths() {
        assert_eq!(NoteValue::plain(1).ticks(), 4 * PPQ);
        assert_eq!(NoteValue::QUARTER.ticks(), PPQ);
        assert_eq!(NoteValue::plain(8).ticks(), PPQ / 2);
        assert_eq!(NoteValue::plain(8).toggle_dotted().ticks(), PPQ * 3 / 4);
        assert_eq!(NoteValue::plain(8).toggle_triplet().ticks(), PPQ / 3);
        assert_eq!(NoteValue::plain(64).ticks(), PPQ / 16);
        assert_eq!(
            NoteValue::plain(5),
            NoteValue::QUARTER,
            "an odd divisor is a quarter"
        );
    }

    #[test]
    fn dotted_and_triplet_exclude_each_other() {
        let v = NoteValue::plain(8).toggle_dotted().toggle_triplet();
        assert!(v.triplet && !v.dotted);
        let v = v.toggle_dotted();
        assert!(v.dotted && !v.triplet);
        assert_eq!(v.toggle_dotted(), NoteValue::plain(8));
    }

    #[test]
    fn labels_read_like_the_notation() {
        assert_eq!(NoteValue::plain(1).label(), "1");
        assert_eq!(NoteValue::QUARTER.label(), "1/4");
        assert_eq!(NoteValue::plain(8).toggle_dotted().label(), "1/8.");
        assert_eq!(NoteValue::plain(16).toggle_triplet().label(), "1/16T");
    }

    #[test]
    fn names_map_to_values() {
        assert_eq!(NoteValue::from_name("whole"), Some(NoteValue::plain(1)));
        assert_eq!(
            NoteValue::from_name("sixty_fourth"),
            Some(NoteValue::plain(64))
        );
        assert_eq!(NoteValue::from_name("breve"), None);
    }

    #[test]
    fn classify_names_the_longest_value_that_fits() {
        for v in [
            NoteValue::plain(1),
            NoteValue::plain(2),
            NoteValue::QUARTER,
            NoteValue::plain(8),
            NoteValue::plain(16),
            NoteValue::plain(8).toggle_dotted(),
            NoteValue::plain(8).toggle_triplet(),
            NoteValue::plain(16).toggle_triplet(),
            NoteValue::QUARTER.toggle_dotted(),
        ] {
            assert_eq!(NoteValue::classify(v.ticks()), v, "{}", v.label());
        }
        // A bit short still reads as the value; a long gap reads as the longest value.
        assert_eq!(NoteValue::classify(PPQ - 100), NoteValue::QUARTER);
        assert_eq!(
            NoteValue::classify(8 * PPQ),
            NoteValue::plain(1).toggle_dotted()
        );
        assert_eq!(NoteValue::classify(1), NoteValue::plain(64));
    }

    #[test]
    fn flags_count_the_beams() {
        assert_eq!(NoteValue::QUARTER.flags(), 0);
        assert_eq!(NoteValue::plain(8).flags(), 1);
        assert_eq!(NoteValue::plain(32).flags(), 3);
    }

    #[test]
    fn the_cursor_steps_on_the_grid_and_stays_in_the_clip() {
        let q = PPQ;
        let end = 4 * PPQ;
        assert_eq!(step_on_grid(0, q, 1, end), q);
        assert_eq!(step_on_grid(q, q, -1, end), 0);
        assert_eq!(step_on_grid(0, q, -1, end), 0, "never before the start");
        assert_eq!(
            step_on_grid(3 * q, q, 1, end),
            3 * q,
            "never past the last point"
        );
        // Off the grid it goes to the point it is moving toward.
        assert_eq!(step_on_grid(q + 5, q, 1, end), 2 * q);
        assert_eq!(step_on_grid(q + 5, q, -1, end), q);
    }

    #[test]
    fn a_tick_snaps_to_the_nearest_point() {
        assert_eq!(snap_to_grid(PPQ / 2 - 1, PPQ), 0);
        assert_eq!(snap_to_grid(PPQ / 2, PPQ), PPQ);
        assert_eq!(snap_to_grid(-40, PPQ), 0);
    }

    #[test]
    fn bars_step_by_the_meter() {
        let bars = Bars::single(crate::timeline::Meter::default());
        let bar = 4 * PPQ;
        assert_eq!(step_bar(PPQ, 1, &bars, 4 * bar), bar);
        assert_eq!(step_bar(bar + PPQ, -1, &bars, 4 * bar), bar);
        assert_eq!(step_bar(bar, -1, &bars, 4 * bar), 0);
        assert_eq!(
            step_bar(3 * bar, 1, &bars, 4 * bar),
            4 * bar - 1,
            "kept inside the clip"
        );
    }

    #[test]
    fn two_quick_digits_make_one_fret() {
        let mut b = FretBuffer::default();
        let cell = (0, 2);
        assert_eq!(
            b.push(1, cell, 24),
            Typed {
                fret: 1,
                joined: false
            }
        );
        b.tick(1.0);
        assert_eq!(
            b.push(2, cell, 24),
            Typed {
                fret: 12,
                joined: true
            }
        );
    }

    #[test]
    fn a_digit_after_the_window_starts_a_new_fret() {
        let mut b = FretBuffer::default();
        let cell = (0, 2);
        b.push(1, cell, 24);
        assert!(!b.tick(1.0), "the window opens at the first tick");
        assert!(
            b.tick(1.0 + FRET_TIMEOUT + 0.01),
            "and closes after the timeout"
        );
        assert!(!b.is_open());
        assert_eq!(
            b.push(2, cell, 24),
            Typed {
                fret: 2,
                joined: false
            }
        );
    }

    #[test]
    fn a_digit_inside_the_window_joins_even_if_the_clock_is_stale() {
        // No frame ran between the digits: the window has not started, so the digit joins.
        let mut b = FretBuffer::default();
        b.push(1, (0, 0), 24);
        assert_eq!(
            b.push(0, (0, 0), 24),
            Typed {
                fret: 10,
                joined: true
            }
        );
    }

    #[test]
    fn another_cell_or_a_too_high_fret_starts_a_new_fret() {
        let mut b = FretBuffer::default();
        b.push(1, (0, 0), 24);
        assert!(!b.push(2, (0, 1), 24).joined, "another string");
        let mut b = FretBuffer::default();
        b.push(2, (0, 0), 24);
        assert_eq!(
            b.push(5, (0, 0), 24),
            Typed {
                fret: 5,
                joined: false
            },
            "25 is beyond fret 24"
        );
        let mut b = FretBuffer::default();
        b.push(2, (0, 0), 24);
        assert_eq!(
            b.push(4, (0, 0), 24),
            Typed {
                fret: 24,
                joined: true
            }
        );
    }

    #[test]
    fn zero_never_extends_and_is_never_extended() {
        let mut b = FretBuffer::default();
        b.push(0, (0, 0), 24);
        assert_eq!(
            b.push(5, (0, 0), 24),
            Typed {
                fret: 5,
                joined: false
            }
        );
    }

    #[test]
    fn clearing_forgets_the_digits() {
        let mut b = FretBuffer::default();
        b.push(1, (0, 0), 24);
        b.clear();
        assert_eq!(
            b.push(2, (0, 0), 24),
            Typed {
                fret: 2,
                joined: false
            }
        );
    }
}
