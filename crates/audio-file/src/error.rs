//! Error type of the crate.

use std::io;

/// Everything that can go wrong in this crate. No function here panics on bad input.
#[derive(Debug, thiserror::Error)]
pub enum AudioFileError {
    /// A file could not be read, written, renamed or listed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// The container, codec or sample format is not supported.
    #[error("unsupported audio: {0}")]
    Unsupported(String),
    /// The data is malformed or truncated and could not be decoded.
    #[error("decode error: {0}")]
    Decode(String),
    /// The file decoded to zero frames.
    #[error("audio is empty")]
    Empty,
    /// Arguments or stored data break an invariant (unequal channels, bad hash, bad peak file).
    #[error("invalid data: {0}")]
    InvalidData(String),
    /// The bundle has no audio file for this content hash.
    #[error("audio asset missing: {hash}")]
    AssetMissing {
        /// Content hash of the missing asset.
        hash: String,
    },
}

/// Result alias used across the crate.
pub type Result<T, E = AudioFileError> = std::result::Result<T, E>;
