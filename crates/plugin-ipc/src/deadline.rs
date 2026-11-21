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

    /// A deadline `after` from now, or `until` when that is later, but never more than `cap`
    /// from now (a wrong or far-off `until` cannot make a wait long). One clock read, like
    /// [`Deadline::after`]. `until` already passed or `None`: the same as `after`.
    pub fn after_or_until(after: Duration, until: Option<Instant>, cap: Duration) -> Self {
        let now = now();
        let Some(base) = now.checked_add(after) else {
            return Self::Never;
        };
        let at = match (until, now.checked_add(cap.max(after))) {
            (Some(until), Some(limit)) => base.max(until.min(limit)),
            _ => base,
        };
        Self::At(at)
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
    fn a_later_until_extends_the_wait_but_never_past_the_cap() {
        let base = Duration::from_millis(1);
        let far = now() + Duration::from_secs(3600);
        let Deadline::At(capped) =
            Deadline::after_or_until(base, Some(far), Duration::from_secs(10))
        else {
            panic!("a finite deadline");
        };
        assert!(capped < now() + Duration::from_secs(11));
        assert!(capped > now() + Duration::from_secs(5));
        let until = now() + Duration::from_secs(60);
        let Deadline::At(at) =
            Deadline::after_or_until(base, Some(until), Duration::from_secs(3600))
        else {
            panic!("a finite deadline");
        };
        assert_eq!(at, until);
    }

    #[test]
    fn an_until_that_has_passed_or_is_missing_is_the_plain_period() {
        let base = Duration::from_secs(30);
        for until in [None, Some(now() - Duration::from_millis(1))] {
            let Deadline::At(at) = Deadline::after_or_until(base, until, Duration::from_secs(60))
            else {
                panic!("a finite deadline");
            };
            assert!(at > now() + Duration::from_secs(20));
            assert!(at < now() + Duration::from_secs(31));
        }
    }

    #[test]
    fn a_deadline_too_far_to_represent_is_never() {
        assert_eq!(Deadline::after(Duration::MAX), Deadline::Never);
    }
}
