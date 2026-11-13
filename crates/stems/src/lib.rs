//! Stem separation: a song in, four stems out (vocals, drums, bass and everything else).
//!
//! The model is Open-Unmix (the `umxhq` weights): one small recurrent network per stem, run in
//! pure Rust on the CPU. The pieces:
//!
//! * [`files`]: the four weight files, their size and SHA-256 pinned in the source, and the
//!   store they are downloaded into (the shared `atomik-download` crate does the download).
//! * [`torch`]: a reader of the published checkpoint format that never runs anything.
//! * [`Models`]: the four networks, loaded from a verified folder.
//! * [`separate`]: the offline separation: chunks of about twenty seconds with a crossfade
//!   between them, a worker-friendly progress callback and a cancel flag, the same bits out for
//!   the same input on one machine, and memory that depends on the chunk and not on the song.
//!
//! Everything here is pure and deterministic: no clock, no random numbers, a fixed order of
//! every sum. Nothing here touches the network.

mod gemm;
mod net;
mod separate;
mod stft;
pub mod torch;

pub use net::Net;
pub use separate::{Models, StemSink, separate};
pub use stft::{Spectrogram, Stft};

use std::sync::atomic::{AtomicBool, Ordering};

/// The sample rate the models run at (audio is resampled to it and back).
pub const RATE: u32 = 44_100;

/// One of the four stems, in the order of the model files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stem {
    /// The singing and speech.
    Vocals,
    /// Drums and percussion.
    Drums,
    /// The bass.
    Bass,
    /// Everything else.
    Other,
}

impl Stem {
    /// The four stems in the order of the model files.
    pub const ALL: [Stem; 4] = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];

    /// The plain English name (also the model's name for it).
    pub fn label(self) -> &'static str {
        match self {
            Stem::Vocals => "Vocals",
            Stem::Drums => "Drums",
            Stem::Bass => "Bass",
            Stem::Other => "Other",
        }
    }

    /// The position in [`Stem::ALL`].
    pub fn index(self) -> usize {
        match self {
            Stem::Vocals => 0,
            Stem::Drums => 1,
            Stem::Bass => 2,
            Stem::Other => 3,
        }
    }
}

/// Why a separation or a model load did not finish.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StemError {
    /// The user cancelled.
    #[error("cancelled")]
    Cancelled,
    /// A weight file cannot be read or does not fit the network.
    #[error("the stem models cannot be used: {0}")]
    Weights(String),
    /// No stem was asked for.
    #[error("no stem was asked for")]
    NothingWanted,
    /// The audio is not usable (wrong rate for the models).
    #[error("the audio is not usable: {0}")]
    Audio(String),
    /// The receiver of the stems failed (for example a file could not be written).
    #[error("the stems could not be stored: {0}")]
    Sink(String),
}

/// The cancel flag the long loops look at.
#[derive(Debug, Clone, Copy)]
pub struct Control<'a> {
    cancel: &'a AtomicBool,
}

impl<'a> Control<'a> {
    /// A control that looks at `cancel`.
    pub fn new(cancel: &'a AtomicBool) -> Self {
        Self { cancel }
    }

    /// `Err(Cancelled)` once the flag is set.
    pub fn check(&self) -> Result<(), StemError> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(StemError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// The weight files of the models and where they come from.
pub mod files {
    use atomik_download::{ModelFile, ModelStore};
    use std::path::Path;

    /// The vocals network.
    pub const VOCALS: ModelFile = ModelFile {
        name: "vocals-b62c91ce.pth",
        url: "https://zenodo.org/records/3370489/files/vocals-b62c91ce.pth?download=1",
        size: 35_637_796,
        sha256: "b62c91cedbc7a066f1778ead5b5cecb377aa3a46a31af1cce7c5c8769339d083",
    };
    /// The drums network.
    pub const DRUMS: ModelFile = ModelFile {
        name: "drums-9619578f.pth",
        url: "https://zenodo.org/records/3370489/files/drums-9619578f.pth?download=1",
        size: 35_637_796,
        sha256: "9619578f885c54737cb0234f9f9a4a679ee4f31438fd77fd1dbe02bb16c2da0a",
    };
    /// The bass network.
    pub const BASS: ModelFile = ModelFile {
        name: "bass-8d85a5bd.pth",
        url: "https://zenodo.org/records/3370489/files/bass-8d85a5bd.pth?download=1",
        size: 35_637_796,
        sha256: "8d85a5bd3f996a8867fca0e8442e077e5a3f5ec747a6112742452a8f347b39c8",
    };
    /// The network of everything else.
    pub const OTHER: ModelFile = ModelFile {
        name: "other-b52fbbf7.pth",
        url: "https://zenodo.org/records/3370489/files/other-b52fbbf7.pth?download=1",
        size: 35_637_796,
        sha256: "b52fbbf76479e752bd72e02304c602ac7802aa5bfbfb9cd12054b2695d5093ab",
    };

    /// The four files in the order of [`Stem::ALL`](crate::Stem::ALL).
    pub const MODEL_FILES: &[ModelFile] = &[VOCALS, DRUMS, BASS, OTHER];

    /// The weights are published with the Open-Unmix project under the MIT licence (the licence
    /// field of the Zenodo record of the `umxhq` weights).
    pub const MODEL_LICENCE: &str = "MIT";

    /// The store of the stem models in the app's data folder: `<data>/models/stems/`.
    pub fn stems_store(data_dir: &Path) -> ModelStore {
        ModelStore::in_data_dir(data_dir, "stems", MODEL_FILES)
    }
}
