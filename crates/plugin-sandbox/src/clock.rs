//! The one wall-clock read of this crate.

use std::time::Instant;

/// The current time. Used to bound waits on another process (RPC timeouts, the exit grace
/// periods); the result never reaches audio or state, only the decision to stop waiting.
// Justification: wall-clock time bounds a wait for a child process; it feeds no signal path (HI-2).
#[allow(clippy::disallowed_methods)]
pub(crate) fn now() -> Instant {
    Instant::now()
}
