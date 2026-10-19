//! Atomik Rack live player: plays a patch on an audio device, with live
//! MIDI notes routed to a notes input, and reports xruns and DSP load.
//!
//! The library holds all logic so it can be tested without devices; the `atomik-play` binary
//! only installs the Ctrl-C handler and calls [`cli::main_with`] with the [`SystemBackend`].
//!
//! - [`args`]: clap arguments and validation (rate, ASIO availability).
//! - [`target`]: `--rate` override, ignored patch notes, `--to` resolution.
//! - [`backend`]: the device boundary ([`Backend`]); [`SystemBackend`] uses
//!   `atomik-audio-io` and `atomik-midi-io`.
//! - [`session`]: the lifecycle (build → engine → stream → MIDI → stats loop → stop).
//! - [`stats`]: the `--stats` line, run totals and the exit-code policy.
//! - [`listing`]: `--list` output.
//!
//! Lifecycle (`session::play`): the patch is built with `atomik_render::build` (notes dropped,
//! one probe note on the target to resolve its engine ids); the engine is created with
//! `--workers` threads and its live note sender taken before the plan is submitted; the stream
//! is started with a shared [`atomik_engine::FrameClock`]; MIDI opens with a latency of one
//! actual host buffer; then once per second telemetry is read (and printed with `--stats`) and
//! retired plans are collected. On Ctrl-C or after `--duration`, the MIDI input is closed, the
//! stream is stopped and the returned engine is dropped on the main thread.

pub mod args;
pub mod backend;
pub mod cli;
pub mod error;
pub mod listing;
pub mod session;
pub mod stats;
pub mod target;

pub use args::{Args, HostArg};
pub use backend::{Backend, LiveStream, MidiPort, SystemBackend};
pub use error::PlayError;
pub use stats::{EXIT_FAILURE, EXIT_OK, EXIT_XRUNS, RunTotals, StatsLine, event_line, exit_code};
pub use target::{Target, TargetError, resolve_target};
