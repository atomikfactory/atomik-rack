//! The wire of the plugin sandbox: what the app process and an `atomik-plugin-host` child
//! share. No plugin format and no process spawning live here.
//!
//! * The shared audio block (one per plugin instance): a fixed `#[repr(C)]` layout mapped by
//!   both processes, with one request area and one response area, exchanged by sequence
//!   numbers. The engine side ([`ParentPort`]) never allocates, locks or makes a blocking call.
//! * The [`Doorbell`]: a cross-process wake-up whose `ring` never blocks.
//! * The control channel: length-prefixed JSON frames with an optional binary payload, and the
//!   messages in both directions.
//!
//! The contract is `not-for-git/plan/sandbox/03-interfaces.md`, section B.

pub mod block;
pub mod child;
pub mod codec;
pub mod deadline;
pub mod doorbell;
pub mod frame;
pub mod layout;
pub mod lifetime;
pub mod message;
pub mod parent;
pub mod stdio;

pub use block::{SharedBlock, block_file_name, default_block_dir, sweep_stale_blocks};
pub use child::{ChildPort, Request};
pub use codec::AudioOp;
pub use deadline::Deadline;
pub use doorbell::{Doorbell, DoorbellName};
pub use frame::{read_frame, read_frame_with, write_frame};
pub use layout::{
    BLOCK_SIZE, MAGIC, MAX_CHANNELS, MAX_EVENTS_IN, MAX_EVENTS_OUT, MAX_FRAME_BYTES,
    MAX_GUI_DIMENSION, MAX_LOG_BYTES, MAX_REPLY_JSON_BYTES, PROTOCOL_VERSION,
};
pub use message::{
    FromChild, MAX_PRESET_TEXT, MAX_PRESETS, ToChild, WireChannels, WireErrorKind, WireKind,
    WireLayout, WireLevel, WireParam, WireParent, WirePhase, WirePreset, into_backend_error,
};
pub use parent::{DeathHandle, ParentPort, Poll, ProcessorInfo, Response, Ticket};
pub use stdio::take_frame_stdout;
