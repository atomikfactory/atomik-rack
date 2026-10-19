//! Disk streaming of big audio assets.
//!
//! Assets whose decoded size (32-bit float, all channels) is above a threshold are not loaded
//! into RAM: the engine plays them through [`SongAsset::Stream`], and its streamer thread reads
//! them with a [`WavStreamSource`] over the bundle's WAV file. The threshold is
//! [`STREAM_THRESHOLD_BYTES`], overridable by the hidden `stream_threshold_bytes` setting
//! ([`crate::settings::AudioSettings`]) for tests and the stress run. Bounces never stream
//! (the render crate loads everything, so they stay bit-identical).

use atomik_audio_file::{AudioFileError, Bundle, WavStreamReader};
use atomik_engine::{SongAsset, StreamSource};
use std::sync::{Arc, Mutex, PoisonError};

/// Decoded size above which an asset streams from disk: 64 MiB of f32 samples.
pub const STREAM_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;

/// Bytes `frames` frames of `channels` channels take decoded as f32.
pub fn decoded_bytes(frames: u64, channels: u16) -> u64 {
    frames.saturating_mul(u64::from(channels)).saturating_mul(4)
}

/// Whether an asset of `frames` frames and `channels` channels streams under `threshold` bytes
/// (strictly above it).
pub fn should_stream(frames: u64, channels: u16, threshold: u64) -> bool {
    decoded_bytes(frames, channels) > threshold
}

/// A seekable source of planar frames, behind a mutex (the engine's [`StreamSource::read`]
/// takes `&self`). Implemented by the WAV reader; tests plug in slow or failing fakes.
pub trait FrameReader: Send {
    /// Reads frames from `start_frame` into `out` (one buffer per channel, the wanted length
    /// is the buffers' common length); returns the frames read (0 at or past the end).
    ///
    /// # Errors
    /// A message when the file cannot be read.
    fn read_frames(&mut self, start_frame: u64, out: &mut [Vec<f32>]) -> Result<usize, String>;
}

impl FrameReader for WavStreamReader {
    fn read_frames(&mut self, start_frame: u64, out: &mut [Vec<f32>]) -> Result<usize, String> {
        WavStreamReader::read_frames(self, start_frame, out).map_err(|e| e.to_string())
    }
}

/// The engine's [`StreamSource`] over a [`FrameReader`]. A read error plays as the end of the
/// file (silence) and is counted; it never panics (the streamer thread must survive a bad
/// disk).
pub struct ReaderSource<R: FrameReader> {
    reader: Mutex<R>,
    errors: std::sync::atomic::AtomicU64,
}

/// The source of a bundle's WAV files.
pub type WavStreamSource = ReaderSource<WavStreamReader>;

impl<R: FrameReader> ReaderSource<R> {
    /// Wraps `reader`.
    pub fn new(reader: R) -> Self {
        Self {
            reader: Mutex::new(reader),
            errors: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Reads that failed so far.
    pub fn errors(&self) -> u64 {
        self.errors.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl<R: FrameReader> StreamSource for ReaderSource<R> {
    fn read(&self, start_frame: u64, out: &mut [Vec<f32>]) -> usize {
        let mut reader = self.reader.lock().unwrap_or_else(PoisonError::into_inner);
        match reader.read_frames(start_frame, out) {
            Ok(n) => n,
            Err(_) => {
                self.errors
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                0
            }
        }
    }
}

/// A streamed engine asset for the asset `hash` of `bundle` (the WAV file must exist and be 32-bit
/// float, as every file in the bundle is).
///
/// # Errors
/// [`AudioFileError`] when the file is missing or unreadable.
pub fn open_stream_asset(bundle: &Bundle, hash: &str) -> Result<SongAsset, AudioFileError> {
    let reader = bundle.stream_reader(hash)?;
    let (frames, channels) = (reader.frames(), reader.channel_count());
    let channels = u16::try_from(channels)
        .map_err(|_| AudioFileError::InvalidData("too many channels".into()))?;
    Ok(SongAsset::stream(
        frames,
        channels,
        Arc::new(WavStreamSource::new(reader)),
    ))
}

#[cfg(test)]
#[path = "stream_source_tests.rs"]
mod tests;
