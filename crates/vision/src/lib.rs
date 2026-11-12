//! Hand tracking for Atomik Rack: the camera, the hand models, the gesture recogniser.
//!
//! * [`camera`]: a camera behind a trait, a capture thread with a one-frame handoff
//!   ([`capture`]), and a fake that replays frames.
//! * [`models`]: the hand detector and landmark models, run in pure Rust.
//! * [`download`]: the one-time download of the model files, checked against pinned hashes.
//! * [`gesture`]: pinch, ratcheted twist and smoothing, from landmarks only.
//! * [`tracker`]: all of it on one worker thread, reporting through bounded events.
//!
//! Nothing here depends on the application. Frames live in memory only: they are never written to
//! disk, logged or sent anywhere.

#![warn(missing_docs)]

pub mod camera;
pub mod capture;
mod clock;
pub mod download;
pub mod gesture;
mod hand;
pub mod models;
pub mod tracker;

pub use hand::{HandObservation, Handedness, LANDMARKS, Observation, Point3, joint};
