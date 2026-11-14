//! Stem separation: a song in, four stems out (vocals, drums, bass and everything else).
//!
//! This is a fast signal-processing split, **not a learned model**: there are no weights, no
//! download and no training data behind it, so nothing about it carries non-commercial terms. It
//! has bleed between stems, the kind a model trades away for its size and its time. The pieces:
//!
//! * [`Stft`]: the short-time Fourier transform of a stereo signal (a periodic Hann window, 2048
//!   points, a hop of 512) and its overlap-add inverse.
//! * [`separate`]: the split. A harmonic/percussive separation (median filtering of the
//!   magnitude across time and across frequency, soft masks) gives the drums; the harmonic rest
//!   below a crossover of about 150 Hz is the bass; the harmonic rest above it that sits in the
//!   centre of the stereo image, weighted to the voice's range, is the vocals; the residual is
//!   the other stem, so the four add up to the mix. Chunks of about twenty seconds with a
//!   crossfade between them keep the memory bounded by the chunk and not the song.
//!
//! Everything here is pure and deterministic: no clock, no random numbers, a fixed order of every
//! sum, the same bits out for the same input whatever the number of threads. Nothing here
//! touches the network or the disk.
//!
//! A learned model can be plugged in later behind the app's `StemBackend` trait; none is, because
//! no model is known whose weights and training data both allow commercial use.

mod separate;
mod stft;

pub use separate::{CHUNK_SECONDS, StemSink, separate, separate_in_chunks};
pub use stft::{Spectrogram, Stft};

use std::sync::atomic::{AtomicBool, Ordering};

/// The sample rate the separation runs at (audio is resampled to it and back).
pub const RATE: u32 = 44_100;

/// One of the four stems.
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
    /// The four stems in order.
    pub const ALL: [Stem; 4] = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];

    /// The plain English name.
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

/// Why a separation did not finish.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StemError {
    /// The user cancelled.
    #[error("cancelled")]
    Cancelled,
    /// There is no separator here.
    #[error("{0}")]
    Unavailable(String),
    /// No stem was asked for.
    #[error("no stem was asked for")]
    NothingWanted,
    /// The audio is not usable (wrong rate).
    #[error("the audio is not usable: {0}")]
    Audio(String),
    /// The receiver of the stems failed (for example a file could not be written).
    #[error("the stems could not be stored: {0}")]
    Sink(String),
}

/// The cancel flag the long loops look at.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Control<'a> {
    cancel: &'a AtomicBool,
}

impl<'a> Control<'a> {
    /// A control that looks at `cancel`.
    pub(crate) fn new(cancel: &'a AtomicBool) -> Self {
        Self { cancel }
    }

    /// Whether the flag is set.
    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// `Err(Cancelled)` once the flag is set.
    pub(crate) fn check(&self) -> Result<(), StemError> {
        if self.cancelled() {
            Err(StemError::Cancelled)
        } else {
            Ok(())
        }
    }
}
