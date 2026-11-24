//! Disk streaming of big audio assets.
//!
//! Assets whose decoded size (32-bit float, all channels) is above a threshold are not loaded
//! into RAM: the engine plays them through [`SongAsset::Stream`], and its streamer thread reads
//! them with a [`WavStreamSource`] over the bundle's WAV file. The threshold is
//! [`STREAM_THRESHOLD_BYTES`], overridable by the hidden `stream_threshold_bytes` setting
//! ([`crate::settings::AudioSettings`]) for tests and the stress run. Bounces never stream
//! (the render crate loads everything, so they stay bit-identical).
//!
//! # The session budget
//!
//! The threshold looks at one asset; a project of forty mid-sized stems would keep all of them
//! in RAM. A [`StreamRule`] adds a budget for the sum: the bytes of audio a session keeps
//! resident ([`crate::memory_limits::audio_budget_for`]: a quarter of the machine's memory, at
//! least 256 MiB, no budget when the memory is not known). When the resident audio would
//! exceed it, the largest assets stream instead, whatever their own size, except
//!
//! * an asset of at most [`STREAM_FLOOR_BYTES`] (one-shots and short loops stay resident: a
//!   streamed lane costs a ring and disk reads, and saves too little), and
//! * an asset a sampler zone uses (zones always play from RAM; a streamed one would be read
//!   whole into a second copy).
//!
//! [`plan`] decides for a whole list at once (a project opening: largest first, ties broken by
//! key, so two opens of the same project agree) and [`StreamRule::admit`] for one more asset
//! (an import, a recording, a paste: it streams when keeping it would exceed the budget). An
//! asset handed to the engine is never converted: a deletion gives memory back, but what
//! streams keeps streaming until the project is opened again. Without a budget both are
//! exactly the threshold rule of [`should_stream`].

use atomik_audio_file::{AudioFileError, Bundle, WavStreamReader};
use atomik_engine::{SongAsset, StreamSource};
use std::collections::BTreeSet;
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

/// Decoded size at or below which an asset never streams because of the budget: 4 MiB.
pub const STREAM_FLOOR_BYTES: u64 = 4 * 1024 * 1024;

/// How the loading workers decide between RAM and the disk: the per-asset threshold, the floor
/// and the session budget, with what the session already holds. `Copy`: a worker takes its own
/// and counts what it loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamRule {
    /// Decoded bytes above which an asset streams, budget or not.
    pub threshold: u64,
    /// Decoded bytes up to which the budget never makes an asset stream.
    pub floor: u64,
    /// The most resident audio a session keeps (`None`: no budget, only the threshold).
    pub budget: Option<u64>,
    /// Decoded bytes the session holds in RAM already.
    pub resident: u64,
}

impl From<u64> for StreamRule {
    /// The rule of a bare threshold: no budget.
    fn from(threshold: u64) -> Self {
        Self::new(threshold, None, 0)
    }
}

impl StreamRule {
    /// A rule with the standard floor.
    pub fn new(threshold: u64, budget: Option<u64>, resident: u64) -> Self {
        Self {
            threshold,
            floor: STREAM_FLOOR_BYTES,
            budget,
            resident,
        }
    }

    /// Whether the next asset of `frames` frames and `channels` channels streams. One that does
    /// not is counted as resident. `may_stream` false (a sampler's audio) exempts the asset
    /// from the budget; the threshold applies to it as it always did.
    pub fn admit(&mut self, frames: u64, channels: u16, may_stream: bool) -> bool {
        let bytes = decoded_bytes(frames, channels);
        if bytes > self.threshold {
            return true;
        }
        let over = self
            .budget
            .is_some_and(|b| self.resident.saturating_add(bytes) > b);
        if may_stream && over && bytes > self.floor {
            return true;
        }
        self.resident = self.resident.saturating_add(bytes);
        false
    }
}

/// One asset of a list [`plan`] decides for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate<K> {
    /// Identifies the asset (breaks ties between equal sizes).
    pub key: K,
    /// Its decoded size in bytes.
    pub bytes: u64,
    /// False for audio that must stay in RAM (a sampler's).
    pub may_stream: bool,
}

/// The keys of the assets that stream, for a whole list at once. Those above the threshold
/// stream first. When the rest, with the `rule.resident` bytes already held, exceeds the budget,
/// the largest of the assets that may stream and are above the floor follow (size descending,
/// then key ascending) until the sum fits, or until none is left to try. Pure: the same list
/// and rule give the same set.
pub fn plan<K: Ord + Clone>(items: &[Candidate<K>], rule: StreamRule) -> BTreeSet<K> {
    let mut streams = BTreeSet::new();
    let mut resident = rule.resident;
    let mut movable = Vec::new();
    for item in items {
        if item.bytes > rule.threshold {
            streams.insert(item.key.clone());
        } else {
            resident = resident.saturating_add(item.bytes);
            if item.may_stream && item.bytes > rule.floor {
                movable.push(item);
            }
        }
    }
    let Some(budget) = rule.budget else {
        return streams;
    };
    movable.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.key.cmp(&b.key)));
    for item in movable {
        if resident <= budget {
            break;
        }
        resident -= item.bytes;
        streams.insert(item.key.clone());
    }
    streams
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
