//! Core types shared by every Atomik Rack crate: time, parameters, note events and batch buffers.
//!
//! This crate is a stable contract: public changes need review.

mod note;
mod param;
mod time;

pub use note::{BufferFull, ExprKind, NoteBuffer, NoteEvent, NoteKind};
pub use param::{ParamDescriptor, ParamId, Taper, Unit, db_to_gain, gain_to_db};
pub use time::{
    LoopWrap, SampleRate, Tempo, Ticks, TimeError, TimeInfo, beats_to_frames, ticks_to_frames,
};

/// Internal processing block size in frames. Every device always processes exactly this many frames.
pub const BATCH: usize = 64;

/// One batch of mono samples (audio or CV).
pub type Batch = [f32; BATCH];

/// Musical time resolution: ticks per quarter note.
pub const PPQ: i64 = 15_360;

/// Capacity of every per-batch note event buffer created by the engine.
pub const MAX_EVENTS_PER_BATCH: usize = 1024;

/// A batch of silence, convenient for unconnected inputs.
pub const SILENT_BATCH: Batch = [0.0; BATCH];
