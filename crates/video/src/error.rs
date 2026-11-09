//! The one error type of the crate.

use std::io;

/// Why a video job could not be done. Every message reads as a sentence the user can be shown.
#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    /// The program could not be started.
    #[error("cannot start {tool}: {source}")]
    Spawn {
        /// The program's name (`ffmpeg`, `ffprobe`).
        tool: String,
        /// What the system said.
        source: io::Error,
    },
    /// The program ran and failed.
    #[error("{tool} failed{code}: {stderr}")]
    Failed {
        /// The program's name.
        tool: String,
        /// ` (exit code 1)` or empty.
        code: String,
        /// The end of what it printed on its error output (bounded).
        stderr: String,
    },
    /// The job was stopped by the user or by the app closing.
    #[error("the video job was stopped")]
    Cancelled,
    /// The probe output cannot be used.
    #[error("the video information cannot be read: {0}")]
    Probe(String),
    /// The file has no picture (an audio file, or only cover art).
    #[error("the file has no video picture")]
    NoVideo,
    /// The film has no audio track to import.
    #[error("the film has no audio track")]
    NoAudio,
    /// The audio track asked for is not in the film.
    #[error("the film has no audio track {0}")]
    NoSuchAudio(u32),
    /// The sound of the film was written but the song could not take it (decoding it, storing
    /// it in the project, its waveform).
    #[error("the audio of the film cannot be added to the song: {0}")]
    Import(String),
    /// A file operation failed (the cache folder, the proxy file).
    #[error("{0}")]
    Io(#[from] io::Error),
    /// A proxy file is damaged or unfinished.
    #[error("the video proxy is not usable: {0}")]
    Proxy(String),
    /// A proxy file was written by another version of the format and has to be rebuilt.
    #[error("the video proxy has format version {found}, this version reads {wanted}")]
    ProxyVersion {
        /// The version in the file.
        found: u32,
        /// The version this build reads.
        wanted: u32,
    },
    /// No video tools can run here (a headless run, or none was given).
    #[error("video tools are not available")]
    Unavailable,
    /// A film export cannot be done as asked; the text says why and what to choose instead.
    #[error("{0}")]
    Film(String),
    /// FFmpeg is not installed or cannot be used; the text says what to do.
    #[error("{0}")]
    NotUsable(String),
}
