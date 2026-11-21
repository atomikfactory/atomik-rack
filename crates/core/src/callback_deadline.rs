//! When the live audio callback that is running must be done with the plugins.
//!
//! The audio callback publishes, at its start, the moment by which everything that waits on
//! another process (a sandboxed plugin) has to have answered for the callback to finish in time;
//! a wait on a plugin may use that slack. One writer (the callback), any number of readers (the
//! callback thread and the render workers). Publishing is one relaxed atomic store, reading one
//! relaxed load; neither allocates, locks or blocks.
//!
//! The value is a time, so it only ever bounds how long a LIVE wait lasts. Offline rendering
//! never reads it: a bounce does not depend on wall time.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// The published end of the running callback's plugin budget, as nanoseconds since an epoch.
/// `0` means that nothing is published (no stream runs, or none ever ran).
#[derive(Debug)]
pub struct CallbackDeadline {
    epoch: Instant,
    nanos: AtomicU64,
}

impl CallbackDeadline {
    /// A cell with nothing published, counting from `epoch`.
    pub fn new(epoch: Instant) -> Self {
        Self {
            epoch,
            nanos: AtomicU64::new(0),
        }
    }

    /// The cell of this process: the audio callback publishes to it and every sandboxed plugin
    /// reads it, so the two need not be wired together (there is one live callback per process).
    pub fn shared() -> &'static Arc<CallbackDeadline> {
        static SHARED: OnceLock<Arc<CallbackDeadline>> = OnceLock::new();
        SHARED.get_or_init(|| Arc::new(CallbackDeadline::new(epoch_now())))
    }

    /// Publishes that the running callback must be done by `at`. Wait-free.
    pub fn publish(&self, at: Instant) {
        let since = at.saturating_duration_since(self.epoch);
        // Never 0: that value means "nothing published".
        let nanos = u64::try_from(since.as_nanos()).unwrap_or(u64::MAX).max(1);
        self.nanos.store(nanos, Ordering::Relaxed);
    }

    /// Takes the published value back (no stream runs any more).
    pub fn clear(&self) {
        self.nanos.store(0, Ordering::Relaxed);
    }

    /// The published deadline, `None` while nothing is published. A deadline that has already
    /// passed (a stale one, from an earlier callback) is returned as it is: the reader compares.
    pub fn read(&self) -> Option<Instant> {
        match self.nanos.load(Ordering::Relaxed) {
            0 => None,
            n => self.epoch.checked_add(Duration::from_nanos(n)),
        }
    }
}

/// The epoch of the shared cell. The one clock read of this module; it never reaches audio.
// Justification: a live-only wait bound; the epoch only anchors the published times (HI-2).
#[allow(clippy::disallowed_methods)]
fn epoch_now() -> Instant {
    Instant::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_published_at_first_and_after_a_clear() {
        let d = CallbackDeadline::new(epoch_now());
        assert_eq!(d.read(), None);
        d.publish(epoch_now() + Duration::from_millis(5));
        assert!(d.read().is_some());
        d.clear();
        assert_eq!(d.read(), None);
    }

    #[test]
    fn a_published_time_reads_back_exactly() {
        let epoch = epoch_now();
        let d = CallbackDeadline::new(epoch);
        let at = epoch + Duration::from_micros(7_250);
        d.publish(at);
        assert_eq!(d.read(), Some(at));
        // A time at or before the epoch is still a published value.
        d.publish(epoch);
        assert_eq!(d.read(), Some(epoch + Duration::from_nanos(1)));
    }

    #[test]
    fn the_shared_cell_is_one_cell() {
        assert!(Arc::ptr_eq(
            CallbackDeadline::shared(),
            CallbackDeadline::shared()
        ));
    }
}
