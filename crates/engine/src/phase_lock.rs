//! The phase lock of the player while it follows an external MIDI clock.
//!
//! The tempo the player plays at is measured from the spacing of the clocks (the mean of the
//! last beat), so it lags a sender whose tempo wanders and the song drifts from it. The sender's
//! *position*, though, is known exactly: its last locate plus one clock's worth of ticks for
//! every clock that arrived ([`crate::ExternalState::phase`]). Once per batch the player
//! compares that position with its own and nudges itself towards it:
//!
//! * the error is the distance in ticks from the middle of the clock being counted to the
//!   player's tick (a clock marks the start of its tick, so the count runs one clock ahead of
//!   the sender's position and moves in steps: the middle of the step is half a clock back);
//!   inside a loop both are folded into the loop and the error is the shortest way round;
//! * the correction is proportional: the error is closed over [`LOCK_TICKS`] (an eighth of a
//!   beat), a first-order low-pass that looks through the staircase of the count and the jitter
//!   of the port and follows a tempo that wanders over a second or two;
//! * it is bounded: at most [`MAX_RATE`] of the ticks a batch spans, so a song never lurches
//!   (catching up half a beat takes five beats), and it is made of whole frames with the
//!   remainder carried to the next batch, so the same inputs always give the same positions;
//! * an error beyond [`RELOCATE_TICKS`] is not drift (clocks were dropped, or the sender moved
//!   without saying so): the player locates to where the sender is.
//!
//! Locates (Start, Song Position Pointer) are the hard anchors: the player jumps to the
//! position and the lock starts again from a clean slate.
//!
//! Everything here is pure: the same errors in give the same corrections out, with no clock and
//! no allocation.

use crate::sync::TICKS_PER_CLOCK;
use atomik_core::{BATCH, PPQ};

/// The time over which an error is closed, in ticks (an eighth of a beat: three clocks).
const LOCK_TICKS: f64 = (PPQ / 8) as f64;

/// The most the lock changes the pace: a share of the ticks a batch spans.
const MAX_RATE: f64 = 0.1;

/// An error larger than this (one beat) is a jump rather than a drift.
pub(crate) const RELOCATE_TICKS: f64 = PPQ as f64;

/// What the lock asks of the player for one batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Correction {
    /// Move the position by this many frames (negative: hold back), at most a few.
    Frames(i64),
    /// Locate to the sender's position.
    Relocate,
}

/// The state of the lock: the part of a correction too small for a whole frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct PhaseLock {
    carry: f64,
}

/// The tick the player should be at for a sender counted at `phase`, folded into the loop
/// `loop_ticks` (start, end) when there is one.
pub(crate) fn expected_tick(phase: i64, loop_ticks: Option<(i64, i64)>) -> f64 {
    let t = (phase as f64 - (TICKS_PER_CLOCK / 2) as f64).max(0.0);
    match loop_ticks {
        Some((ls, le)) if le > ls && t >= ls as f64 => {
            ls as f64 + (t - ls as f64).rem_euclid((le - ls) as f64)
        }
        _ => t,
    }
}

/// The error in ticks (positive: the sender is ahead) between `expected` and the player's `tick`;
/// inside a loop the shortest way round.
pub(crate) fn error_ticks(expected: f64, tick: f64, loop_ticks: Option<(i64, i64)>) -> f64 {
    let e = expected - tick;
    match loop_ticks {
        Some((ls, le)) if le > ls => {
            let len = (le - ls) as f64;
            (e + len / 2.0).rem_euclid(len) - len / 2.0
        }
        _ => e,
    }
}

impl PhaseLock {
    /// Forgets the carried remainder (after a locate).
    pub(crate) fn reset(&mut self) {
        self.carry = 0.0;
    }

    /// The correction for one batch given the `error` in ticks and the pace `ticks_per_frame`.
    pub(crate) fn step(&mut self, error: f64, ticks_per_frame: f64) -> Correction {
        if !error.is_finite() || !ticks_per_frame.is_finite() || ticks_per_frame <= 0.0 {
            return Correction::Frames(0);
        }
        if error.abs() > RELOCATE_TICKS {
            self.reset();
            return Correction::Relocate;
        }
        let span = BATCH as f64 * ticks_per_frame;
        let ticks = (error * span / LOCK_TICKS).clamp(-MAX_RATE * span, MAX_RATE * span);
        let frames = ticks / ticks_per_frame + self.carry;
        let whole = frames.trunc();
        self.carry = frames - whole;
        Correction::Frames(whole as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    #[test]
    fn the_expected_tick_is_the_middle_of_the_clock_being_counted() {
        let half = (TICKS_PER_CLOCK / 2) as f64;
        assert_eq!(expected_tick(0, None), 0.0);
        assert_eq!(
            expected_tick(TICKS_PER_CLOCK * 5, None),
            TICKS_PER_CLOCK as f64 * 5.0 - half
        );
        assert_eq!(expected_tick(-30, None), 0.0);
    }

    #[test]
    fn a_loop_folds_the_senders_position_into_it() {
        let (ls, le) = (PPQ * 4, PPQ * 8);
        let lp = Some((ls, le));
        let half = TICKS_PER_CLOCK / 2;
        assert_eq!(expected_tick(PPQ + half, lp), PPQ as f64);
        assert_eq!(expected_tick(ls + half, lp), ls as f64);
        assert_eq!(expected_tick(le + 100 + half, lp), (ls + 100) as f64);
        // Just before the end and just after the wrap are close, not a loop apart.
        let (a, b) = ((ls + 5) as f64, (le - 5) as f64);
        assert_eq!(error_ticks(a, b, lp), 10.0);
        assert_eq!(error_ticks(b, a, lp), -10.0);
        assert_eq!(error_ticks(100.0, 90.0, None), 10.0);
    }

    #[test]
    fn corrections_are_bounded_and_carry_their_remainder() {
        let mut lock = PhaseLock::default();
        let tpf = 0.64; // 120 BPM at 48 kHz
        // A beat of error is relocated only above one beat: a large one is capped by the rate.
        let Correction::Frames(big) = lock.step(PPQ as f64 * 0.9, tpf) else {
            panic!("a drift, not a jump");
        };
        let cap = (MAX_RATE * BATCH as f64).ceil() as i64;
        assert!((1..=cap).contains(&big), "{big}");
        // A small error takes whole frames only, and the pieces add up.
        let mut lock = PhaseLock::default();
        let total: i64 = (0..1000)
            .map(|_| match lock.step(100.0, tpf) {
                Correction::Frames(n) => n,
                Correction::Relocate => panic!(),
            })
            .sum();
        let frames = 1000.0 * 100.0 * BATCH as f64 / LOCK_TICKS; // ticks per batch / tpf

        assert!((total as f64 - frames).abs() <= 1.0, "{total} vs {frames}");
    }

    #[test]
    fn a_jump_is_relocated_and_nonsense_does_nothing() {
        let mut lock = PhaseLock::default();
        assert_eq!(lock.step(RELOCATE_TICKS + 1.0, 0.64), Correction::Relocate);
        assert_eq!(lock.step(-RELOCATE_TICKS - 1.0, 0.64), Correction::Relocate);
        assert_eq!(lock.step(f64::NAN, 0.64), Correction::Frames(0));
        assert_eq!(lock.step(5.0, 0.0), Correction::Frames(0));
    }

    /// The closed loop against a scripted sender, in batches and ticks, with no clock: the
    /// sender's tempo wanders 10 % either way, the measured tempo is the mean of the last beat
    /// (what the follower reports) and the count is the sender's tick in whole clocks. Returns
    /// the largest distance between the player and the sender once settled.
    fn worst_drift(locked: bool, wander: f64) -> f64 {
        let batch_secs = BATCH as f64 / SR;
        let window = 400; // batches in about a beat at 120 BPM
        let mut history: Vec<f64> = Vec::new();
        let (mut sender, mut player) = (0.0f64, 0.0f64);
        let mut lock = PhaseLock::default();
        let mut worst = 0.0f64;
        for n in 0..(SR * 24.0 / BATCH as f64) as usize {
            let t = n as f64 * batch_secs;
            let bpm = 120.0 * (1.0 + wander * (t * std::f64::consts::TAU / 3.0).sin());
            history.push(bpm);
            let from = history.len().saturating_sub(window);
            let measured = history[from..].iter().sum::<f64>() / (history.len() - from) as f64;
            let phase = ((sender / TICKS_PER_CLOCK as f64).floor() as i64 + 1) * TICKS_PER_CLOCK;
            let tpf = measured * PPQ as f64 / (60.0 * SR);
            if locked {
                let e = error_ticks(expected_tick(phase, None), player, None);
                match lock.step(e, tpf) {
                    Correction::Frames(f) => player += f as f64 * tpf,
                    Correction::Relocate => player = expected_tick(phase, None),
                }
            }
            if t > 4.0 {
                worst = worst.max((sender - player).abs());
            }
            sender += bpm * PPQ as f64 / (60.0 * SR) * BATCH as f64;
            player += tpf * BATCH as f64;
        }
        worst
    }

    #[test]
    fn a_wandering_tempo_drifts_without_the_lock_and_stays_close_with_it() {
        let free = worst_drift(false, 0.10);
        let held = worst_drift(true, 0.10);
        let clock = TICKS_PER_CLOCK as f64;
        assert!(free > clock, "free drift {free}");
        assert!(held < clock / 2.0, "locked drift {held} free {free}");
        assert!(held < free / 3.0);
    }

    #[test]
    fn a_steady_tempo_with_an_offset_start_is_pulled_in_and_stays() {
        // The same loop, but the player starts 6 ticks late at a steady tempo.
        let mut lock = PhaseLock::default();
        let tpf = 120.0 * PPQ as f64 / (60.0 * SR);
        let (mut sender, mut player) = (600.0f64, 594.0f64);
        for _ in 0..(SR * 8.0 / BATCH as f64) as usize {
            let phase = ((sender / TICKS_PER_CLOCK as f64).floor() as i64 + 1) * TICKS_PER_CLOCK;
            let e = error_ticks(expected_tick(phase, None), player, None);
            if let Correction::Frames(f) = lock.step(e, tpf) {
                player += f as f64 * tpf;
            }
            sender += tpf * BATCH as f64;
            player += tpf * BATCH as f64;
        }
        assert!(
            (sender - player).abs() < TICKS_PER_CLOCK as f64 / 2.0,
            "{}",
            sender - player
        );
    }

    #[test]
    fn the_same_inputs_give_the_same_corrections() {
        let run = || {
            let mut lock = PhaseLock::default();
            (0..500)
                .map(|k| lock.step(f64::from(k % 40) * 10.0 - 200.0, 0.62))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }
}
