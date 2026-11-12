//! The one place that reads the wall clock. The recogniser and the models never do (time is a
//! number that comes with each frame); the capture and the worker read it for timestamps, frame
//! rates and the time a frame took.

use std::time::Instant;

/// The current instant. Camera timestamps and timings are real time by nature; nothing that
/// decides a gesture reads this.
#[allow(clippy::disallowed_methods)]
pub fn now() -> Instant {
    Instant::now()
}
