//! The rule for restarting the audio by itself after a contained engine fault (a device that
//! panicked while rendering, or a callback panic that halted the stream).
//!
//! The engine reports a fault with a flag that the shell reads when it polls the audio status;
//! this module only decides *when* the shell may rebuild the engine in answer:
//!
//! * only while the transport is stopped (a fault during playback waits for the stop);
//! * at most one restart a minute (the clock is the `Instant` the shell polls with, so a test
//!   moves time by passing a later one);
//! * at most [`MAX_RESTARTS`] restarts for the whole session (a plain count: the simplest rule
//!   that cannot loop). After that a fault gets the manual message again.
//!
//! The restart itself, and the bypass of the faulted device before it, are the shell's and the
//! core's ([`crate::shell::Shell`], `AppCore::bypass_faulted_device`).

use std::time::{Duration, Instant};

/// The least time between two automatic restarts.
pub const MIN_GAP: Duration = Duration::from_secs(60);

/// How many automatic restarts one session gets.
pub const MAX_RESTARTS: u32 = 3;

/// What a fault that has just been seen gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrival {
    /// The audio restarts by itself as soon as the transport is stopped and the gap has passed.
    Automatic,
    /// The limit is used up: the user restarts the audio.
    Manual,
}

/// The restart budget of a session and the restart a fault is waiting for.
#[derive(Debug, Default)]
pub struct FaultRestart {
    /// Restarts made so far.
    restarts: u32,
    /// When the last one was made.
    last: Option<Instant>,
    /// A fault is waiting for its restart.
    waiting: bool,
}

impl FaultRestart {
    /// A fault was seen: queues its restart, or says the user has to do it.
    pub fn arrive(&mut self) -> Arrival {
        if self.restarts >= MAX_RESTARTS {
            self.waiting = false;
            return Arrival::Manual;
        }
        self.waiting = true;
        Arrival::Automatic
    }

    /// True when a restart may run at `at` (stopped transport, a minute since the last).
    pub fn allowed(&self, at: Instant, stopped: bool) -> bool {
        stopped
            && self
                .last
                .is_none_or(|last| at.saturating_duration_since(last) >= MIN_GAP)
    }

    /// True once, when a queued restart may run at `at`: the restart is counted.
    pub fn take_due(&mut self, at: Instant, stopped: bool) -> bool {
        if !self.waiting || !self.allowed(at, stopped) {
            return false;
        }
        self.waiting = false;
        self.restarts += 1;
        self.last = Some(at);
        true
    }

    /// The fault is gone (the engine was rebuilt some other way): nothing is waiting.
    pub fn clear(&mut self) {
        self.waiting = false;
    }

    /// Whether a restart is queued.
    #[cfg(test)]
    pub fn is_waiting(&self) -> bool {
        self.waiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> Instant {
        // One fixed origin: the rule only compares instants.
        static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *ORIGIN.get_or_init(crate::session::clock_now) + Duration::from_secs(secs)
    }

    #[test]
    fn a_fault_while_stopped_restarts_once() {
        let mut r = FaultRestart::default();
        assert_eq!(r.arrive(), Arrival::Automatic);
        assert!(r.take_due(at(0), true));
        assert!(!r.take_due(at(1), true), "one fault, one restart");
    }

    #[test]
    fn playing_waits_for_the_stop() {
        let mut r = FaultRestart::default();
        r.arrive();
        assert!(!r.take_due(at(5), false));
        assert!(r.is_waiting());
        assert!(r.take_due(at(9), true));
    }

    #[test]
    fn a_second_fault_inside_a_minute_waits_for_the_minute() {
        let mut r = FaultRestart::default();
        r.arrive();
        assert!(r.take_due(at(0), true));
        assert_eq!(r.arrive(), Arrival::Automatic);
        assert!(!r.take_due(at(59), true));
        assert!(r.take_due(at(60), true));
    }

    #[test]
    fn the_fourth_fault_is_left_to_the_user() {
        let mut r = FaultRestart::default();
        for n in 0..MAX_RESTARTS {
            assert_eq!(r.arrive(), Arrival::Automatic);
            assert!(r.take_due(at(u64::from(n) * 60), true));
        }
        assert_eq!(r.arrive(), Arrival::Manual);
        assert!(!r.is_waiting());
        assert!(!r.take_due(at(10_000), true));
    }

    #[test]
    fn a_cleared_fault_queues_nothing() {
        let mut r = FaultRestart::default();
        r.arrive();
        r.clear();
        assert!(!r.take_due(at(0), true));
    }
}
