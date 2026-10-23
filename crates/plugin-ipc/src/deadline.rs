//! Deadlines for the spin wait, and the one clock read of this crate.

use std::time::{Duration, Instant};

/// When a wait gives up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deadline {
    /// Never: the wait ends when the result is there or the child is dead.
    Never,
    /// At this time.
    At(Instant),
}

impl Deadline {
    /// A deadline `after` from now (`Never` when that is too far to represent).
    pub fn after(after: Duration) -> Self {
        match now().checked_add(after) {
            Some(at) => Self::At(at),
            None => Self::Never,
        }
    }

    /// True once the deadline has come.
    pub fn passed(&self) -> bool {
        match self {
            Self::Never => false,
            Self::At(at) => now() >= *at,
        }
    }
}

/// The current time. The only clock read of the crate: the spin wait and the doorbell timeout
/// need to know when to stop, and the result never reaches audio or state, only the decision
/// to give up waiting.
// Justification: wall-clock time bounds a wait for another process; it feeds no signal path (HI-2).
#[allow(clippy::disallowed_methods)]
pub(crate) fn now() -> Instant {
    Instant::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_does_not_pass() {
        assert!(!Deadline::Never.passed());
    }

    #[test]
    fn a_zero_deadline_has_already_passed() {
        assert!(Deadline::after(Duration::ZERO).passed());
    }

    #[test]
    fn a_far_deadline_has_not_passed() {
        assert!(!Deadline::after(Duration::from_secs(3600)).passed());
    }

    #[test]
    fn a_deadline_too_far_to_represent_is_never() {
        assert_eq!(Deadline::after(Duration::MAX), Deadline::Never);
    }
}
