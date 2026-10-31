//! Per-device processing time, for the rack's CPU readout.
//!
//! The engine itself never reads a clock (its output must not depend on time). A host that
//! wants the readout installs a monotonic nanosecond counter once ([`install_clock`]) and
//! switches the measurement on ([`set_enabled`]). While it is on, [`crate::plan::Node::run`]
//! reads the counter before and after each device call and adds the difference to the node's
//! own `busy_ns`; after every batch the audio thread publishes those totals, and the
//! audio time rendered so far, into [`crate::Meters`]. A reader turns two samples into a
//! load: `delta busy / delta budget`.
//!
//! The measurement only ever writes the node's counter and the atomics of the meter bank: it
//! is never fed back into the signal, allocates nothing and takes no lock (reading the clock
//! slot is an atomic load). With the clock missing or the switch off a node pays two relaxed
//! loads per call.
//!
//! The clock and the switch are process-wide, because nodes are built without a reference to
//! their engine. Two engines in one process share the switch; each still publishes its own
//! totals.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// A monotonic counter in nanoseconds. It must not allocate, lock or block.
pub type ClockFn = fn() -> u64;

static CLOCK: OnceLock<ClockFn> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Installs the process-wide counter (non-RT, once). Returns false when one is installed
/// already; the first stays.
pub fn install_clock(clock: ClockFn) -> bool {
    CLOCK.set(clock).is_ok()
}

/// Switches the per-device timing on or off. Without an installed clock it does nothing.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether the timing is on and a clock is installed.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) && CLOCK.get().is_some()
}

/// RT: the counter when the timing is on, else `None`.
#[inline]
pub(crate) fn start() -> Option<(ClockFn, u64)> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let clock = *CLOCK.get()?;
    Some((clock, clock()))
}

/// RT: nanoseconds since [`start`] read `from`.
#[inline]
pub(crate) fn elapsed(started: (ClockFn, u64)) -> u64 {
    (started.0)().saturating_sub(started.1)
}

/// Nanoseconds of audio in one batch at `rate` Hz (the budget a batch may use).
pub(crate) fn batch_budget_ns(rate: f64) -> u64 {
    (atomik_core::BATCH as f64 * 1e9 / rate.max(1.0)) as u64
}

/// One load sample: the cumulative busy time of a device, and the cumulative audio time, in
/// nanoseconds, at the same instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuSample {
    /// Time the device spent in `process` since it entered the arena.
    pub busy_ns: u64,
    /// Audio time rendered by the engine since it started.
    pub budget_ns: u64,
}

/// Percent of one callback budget that the device used between two samples, in `0.0..`
/// (above 100 means the device alone overran its share). `None` while no audio time
/// passed between them. A busy total that went down (the device was replaced) counts as 0.
pub fn load_percent(before: CpuSample, after: CpuSample) -> Option<f32> {
    let budget = after.budget_ns.checked_sub(before.budget_ns)?;
    if budget == 0 {
        return None;
    }
    let busy = after.busy_ns.saturating_sub(before.busy_ns);
    Some((busy as f64 * 100.0 / budget as f64) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_load_is_the_busy_share_of_the_audio_time() {
        let a = CpuSample::default();
        let b = CpuSample {
            busy_ns: 250,
            budget_ns: 1000,
        };
        assert_eq!(load_percent(a, b), Some(25.0));
        let c = CpuSample {
            busy_ns: 1500,
            budget_ns: 2000,
        };
        assert_eq!(load_percent(b, c), Some(125.0));
    }

    #[test]
    fn no_audio_time_means_no_load_and_a_replaced_device_reads_zero() {
        let a = CpuSample {
            busy_ns: 10,
            budget_ns: 100,
        };
        assert_eq!(load_percent(a, a), None);
        let replaced = CpuSample {
            busy_ns: 0,
            budget_ns: 200,
        };
        assert_eq!(load_percent(a, replaced), Some(0.0));
    }

    #[test]
    fn a_batch_budget_follows_the_rate() {
        assert_eq!(
            batch_budget_ns(48_000.0),
            (atomik_core::BATCH as f64 * 1e9 / 48_000.0) as u64
        );
        assert!(batch_budget_ns(0.0) > 0);
    }
}
