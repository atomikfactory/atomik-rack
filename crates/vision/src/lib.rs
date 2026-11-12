//! Hand tracking for Atomik Rack: the camera, the hand models, the gesture recogniser.
//!
//! * [`download`]: the one-time download of the model files, checked against pinned hashes.
//! * [`gesture`]: pinch, ratcheted twist and smoothing, from landmarks only.
//!
//! Nothing here depends on the application. Frames live in memory only: they are never written to
//! disk, logged or sent anywhere.

#![warn(missing_docs)]

pub mod download;
pub mod gesture;
mod hand;

pub use hand::{HandObservation, Handedness, LANDMARKS, Observation, Point3, joint};
