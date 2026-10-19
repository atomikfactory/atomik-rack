//! Player errors. `Display` is the message printed after `error: `; patch problems start with
//! their JSON path, as in `atomik-render`.

use crate::target::TargetError;
use atomik_audio_io::IoError;
use atomik_engine::EngineError;
use atomik_midi_io::MidiError;
use atomik_render::build::BuildError;
use atomik_render::patch::PatchError;

/// Any failure of the player (exit code 2).
#[derive(Debug, thiserror::Error)]
pub enum PlayError {
    /// Invalid command-line arguments (beyond what clap checks).
    #[error("{0}")]
    Args(String),
    /// The patch file could not be read.
    #[error("{}: cannot read the patch: {source}", path.display())]
    Read {
        /// The patch file.
        path: std::path::PathBuf,
        /// The I/O error.
        source: std::io::Error,
    },
    /// The `--log` file could not be created or written.
    #[error("{}: cannot write the log: {source}", path.display())]
    Log {
        /// The log file.
        path: std::path::PathBuf,
        /// The I/O error.
        source: std::io::Error,
    },
    /// The patch is malformed or invalid (`<json path>: <message>`).
    #[error(transparent)]
    Patch(#[from] PatchError),
    /// The patch cannot be built (`<json path>: <message>`).
    #[error(transparent)]
    Build(#[from] BuildError),
    /// `--to` does not name a notes input.
    #[error(transparent)]
    Target(#[from] TargetError),
    /// The audio stream could not be opened.
    #[error(transparent)]
    Audio(#[from] IoError),
    /// The MIDI input could not be opened.
    #[error(transparent)]
    Midi(#[from] MidiError),
    /// The engine did not accept the plan.
    #[error("cannot submit the plan to the engine: {0}")]
    Engine(#[from] EngineError),
}
