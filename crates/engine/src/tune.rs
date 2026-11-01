//! The master tune the engine hands the devices.
//!
//! The project's tune ([`crate::EngineHandle::set_master_tune`]) arrives through the control
//! queue, so it changes at a batch boundary like every other edit and never from a live atomic:
//! a bounce reads exactly the values a playback of the same edits did. [`Glide`] turns a change
//! into a short exponential move, one step per batch (a fixed count of steps for a given rate,
//! whatever the host block size), so notes that are held do not step in pitch. At rest it is
//! exact: a tune that never moved from 0 stays 0.0 and the devices add nothing.

use atomik_core::BATCH;

/// How long a glide takes to cover most of the way, in seconds.
const GLIDE_SECONDS: f64 = 0.02;
/// A glide ends when it is this close to its target (cents).
const GLIDE_DONE: f32 = 0.005;

/// The master tune in cents as the batches see it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Glide {
    now: f32,
    target: f32,
    /// The share of the remaining distance one batch covers.
    coef: f32,
}

impl Glide {
    /// At 0 cents for an engine at `sample_rate`.
    pub(crate) fn new(sample_rate: f64) -> Self {
        Self {
            now: 0.0,
            target: 0.0,
            coef: (1.0 - (-(BATCH as f64) / (GLIDE_SECONDS * sample_rate)).exp()) as f32,
        }
    }

    /// Moves to `cents`: at once, or from the next batch on over [`GLIDE_SECONDS`].
    pub(crate) fn set(&mut self, cents: f32, glide: bool) {
        self.target = cents;
        if !glide {
            self.now = cents;
        }
    }

    /// The tune of the batch about to run; one step of any glide in progress.
    pub(crate) fn next(&mut self) -> f32 {
        if self.now.to_bits() != self.target.to_bits() {
            self.now += (self.target - self.now) * self.coef;
            if (self.target - self.now).abs() < GLIDE_DONE {
                self.now = self.target;
            }
        }
        self.now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tune_that_never_moved_is_exactly_zero() {
        let mut g = Glide::new(48_000.0);
        for _ in 0..1000 {
            assert_eq!(g.next().to_bits(), 0.0f32.to_bits());
        }
    }

    #[test]
    fn an_immediate_change_is_in_force_at_once() {
        let mut g = Glide::new(48_000.0);
        g.set(-37.5, false);
        assert_eq!(g.next(), -37.5);
        assert_eq!(g.next(), -37.5);
    }

    #[test]
    fn a_glide_moves_monotonically_and_lands_exactly() {
        let mut g = Glide::new(48_000.0);
        g.set(50.0, true);
        let mut last = 0.0f32;
        let mut steps = 0;
        while g.next().to_bits() != 50.0f32.to_bits() {
            let v = g.now;
            assert!(v > last && v < 50.0, "{v}");
            last = v;
            steps += 1;
            assert!(steps < 5000, "never lands");
        }
        // About 20 ms per time constant: it is done well within a second.
        let seconds = f64::from(steps) * BATCH as f64 / 48_000.0;
        assert!((0.05..0.5).contains(&seconds), "{seconds} s");
        assert_eq!(g.next(), 50.0);
    }

    #[test]
    fn the_same_changes_give_the_same_values() {
        let run = || {
            let mut g = Glide::new(44_100.0);
            let mut out = Vec::new();
            for batch in 0..400 {
                if batch == 10 {
                    g.set(-100.0, true);
                }
                if batch == 200 {
                    g.set(25.0, true);
                }
                out.push(g.next().to_bits());
            }
            out
        };
        assert_eq!(run(), run());
    }
}
