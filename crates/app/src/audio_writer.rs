//! The audio recording writer thread: drains the engine's record tap
//! while a take runs, appends every frame to one in-progress WAV file per armed track, and on
//! finish cuts the file into laps and stores each as a bundle asset.
//!
//! # Design
//!
//! * **Never on the UI or audio thread.** `spawn` starts a thread that polls the tap (2 ms)
//!   with [`atomik_engine::RecordTap::drain`] and writes with `hound`. The audio thread only
//!   copies into the tap's ring; the UI thread only sends [`Ctl`] messages.
//! * **Crash safety.** The in-progress files live in the bundle's `Audio/` as
//!   `rec-<pid>-<take>-<n>.wav` (not a content hash, so orphan cleanup leaves them alone) and
//!   are written incrementally; the header is refreshed about every half second of audio
//!   (`WavWriter::flush`), so after a crash the file opens with everything but the last moments.
//! * **Aligned by construction.** File frame `i` is engine frame `first + i`. A gap in the tap
//!   (overflow, reported by [`atomik_engine::RecordTap::overflow_frames`]) is written as silence,
//!   so the take stays aligned and every later frame lands where it was played.
//! * **Finish.** [`Ctl::Finish`] carries the end engine frame (exclusive), the compensation and
//!   the song-position function. The thread keeps draining until it has the frames up to the end
//!   (or the tap has been quiet for 300 ms), finalises the files, then [`split_laps`] cuts each
//!   file into laps; every lap is read back, stored with `Bundle::add_recording` (hashed, written
//!   as `Audio/<hash>.wav`, peaks cached), and the result goes to the UI as one message. The
//!   in-progress files are deleted once their laps are stored. **One asset per lap and track**:
//!   an asset is exactly the audio of one pass (its sub-tick head is skipped by `source_offset`).
//! * **Cancel.** [`Ctl::Cancel`] (or a dropped sender) deletes the files and ends the thread.
//! * **No abort at the end of a long take.** A lap is read back whole to be hashed and stored,
//!   so its buffers are the one allocation that grows with the take. They are reserved with
//!   `try_reserve_exact`: when the memory is not there the take ends with a notice, the in-progress
//!   files stay where they are (the orphan scan of [`crate::recovery`] brings them back at the next
//!   start) and the app goes on. The same holds for any other failure while the laps are stored:
//!   the in-progress files are only deleted once every lap is stored. Hashing, the file write and
//!   the peak scan work in bounded blocks and need nothing proportional to the take.
//! * **A write error stops the take at once.** The thread that cannot write sets the take's
//!   failure flag (a shared [`AtomicBool`] the UI thread reads every frame, nothing on the audio
//!   thread) and ends with the reason as its result; the app then ends the recording as if the
//!   user had stopped it, and the transport keeps running.

use crate::audio_import::{Msg, RecDoneGuard};
use crate::audio_record::{PlannedTrack, TakeRange, split_laps};
use crate::stream_source::{StreamRule, open_stream_asset};
use atomik_audio_file::{AssetInfo, AudioData, Bundle, PeakPyramid, WavStreamReader};
use atomik_engine::{RecordTap, SongAsset, SongPosition};
use atomik_model::DeviceKey;
use atomik_ui::i18n;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::Duration;

/// How many idle polls (2 ms each, so about 300 ms) the tap may stay quiet after a finish before
/// the thread gives up waiting for the last frames (the engine may have stopped).
const QUIET_POLLS: u32 = 150;
/// Poll period of the drain loop.
const POLL: Duration = Duration::from_millis(2);
/// Seconds of audio between header refreshes of the in-progress files.
const FLUSH_SECONDS: u64 = 1;

/// One in-progress file.
#[derive(Debug, Clone)]
pub struct WriterTrack {
    /// What the app needs to place the laps.
    pub planned: PlannedTrack,
    /// Name for the stored assets.
    pub name: String,
    /// The in-progress WAV file.
    pub path: PathBuf,
}

/// What a writer needs to start.
pub struct WriterConfig {
    /// The tracks, in plan order.
    pub tracks: Vec<WriterTrack>,
    /// Where the laps are stored.
    pub bundle: Bundle,
    /// Session rate in Hz.
    pub sample_rate: u32,
    /// How a recorded take reaches the engine: from RAM, or streamed from disk (the threshold and
    /// the session budget, with what the session holds already).
    pub stream: StreamRule,
    /// The document generation (messages of an older one are ignored).
    pub generation: u64,
    /// Take id (for the message).
    pub take: u64,
}

/// How the take ends.
pub struct FinishInfo {
    /// First engine frame **not** to record.
    pub end_engine_frame: u64,
    /// The compensation ([`crate::audio_record::record_offset_frames`]).
    pub compensation: i64,
    /// Engine frame the transport started at (frames compensated to before it are dropped).
    pub valid_from: u64,
    /// Engine frame to song position (the latest playing state of the transport).
    pub position: Box<dyn Fn(u64) -> SongPosition + Send>,
}

/// UI to writer messages.
pub enum Ctl {
    /// End the take.
    Finish(Box<FinishInfo>),
    /// Throw the take away.
    Cancel,
}

/// One stored lap of one track.
pub struct RecordedLap {
    /// The audio track.
    pub device: DeviceKey,
    /// Loop pass (0-based, counted from the take's first kept frame).
    pub lap: u64,
    /// Song frame of the lap's first frame.
    pub song_frame: u64,
    /// The stored asset.
    pub info: AssetInfo,
    /// How the engine plays it (memory, or streamed above the threshold).
    pub asset: SongAsset,
    /// Waveform peaks.
    pub peaks: Arc<PeakPyramid>,
}

/// What a finished take hands to the UI.
pub struct TakeResult {
    /// Take id.
    pub take: u64,
    /// The laps, per track in plan order, then by lap.
    pub laps: Vec<RecordedLap>,
    /// Input frames the tap dropped because its ring was full (written as silence).
    pub overflow_frames: u64,
    /// Frames of silence written for gaps in the tap.
    pub gap_frames: u64,
    /// Frames recorded per track (including dropped head frames).
    pub frames: u64,
}

/// Failures a test asks the writer to simulate, so that the code runs without allocating
/// gigabytes or filling a disk. The app never sets any (`Faults::default()`).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Faults {
    /// A lap of more samples than this (all channels) is refused as if its buffers could not be
    /// allocated.
    pub lap_limit_samples: Option<u64>,
    /// Writing fails with a full disk once this many frames of the take are written.
    pub fail_after_frames: Option<u64>,
}

/// Starts the writer thread for `tap`; the result arrives as [`Msg::Recorded`]. Returns the
/// control channel.
///
/// # Errors
/// The thread could not be spawned.
#[cfg(test)]
pub(crate) fn spawn(
    tap: RecordTap,
    config: WriterConfig,
    tx: Sender<Msg>,
) -> std::io::Result<(Sender<Ctl>, std::thread::JoinHandle<()>)> {
    spawn_with(tap, config, tx, Faults::default())
}

/// [`spawn_watched`] without the flag (for tests).
///
/// # Errors
/// The thread could not be spawned.
#[cfg(test)]
pub(crate) fn spawn_with(
    tap: RecordTap,
    config: WriterConfig,
    tx: Sender<Msg>,
    faults: Faults,
) -> std::io::Result<(Sender<Ctl>, std::thread::JoinHandle<()>)> {
    spawn_watched(tap, config, tx, faults).map(|(ctl, handle, _)| (ctl, handle))
}

/// Starts the writer thread like [`spawn`] and also returns the take's failure flag: it is set,
/// before the result is sent, when writing the take failed and the thread has ended. `faults`
/// is for tests.
///
/// # Errors
/// The thread could not be spawned.
pub(crate) fn spawn_watched(
    tap: RecordTap,
    config: WriterConfig,
    tx: Sender<Msg>,
    faults: Faults,
) -> std::io::Result<(Sender<Ctl>, std::thread::JoinHandle<()>, Arc<AtomicBool>)> {
    let (ctl_tx, ctl_rx) = channel();
    let generation = config.generation;
    let failed = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&failed);
    let handle = std::thread::Builder::new()
        .name("atomik-record".into())
        .spawn(move || {
            let mut guard = RecDoneGuard::new(tx, generation);
            match run(tap, &config, &ctl_rx, &flag, faults) {
                Some(result) => guard.send(result),
                None => guard.disarm(),
            }
        })?;
    Ok((ctl_tx, handle, failed))
}

type Wav = hound::WavWriter<BufWriter<File>>;

fn io(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn open_wav(track: &WriterTrack, rate: u32) -> Result<Wav, String> {
    let spec = hound::WavSpec {
        channels: u16::try_from(track.planned.tap_index.len()).map_err(io)?,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    hound::WavWriter::create(&track.path, spec).map_err(io)
}

fn remove_partials(config: &WriterConfig) {
    for t in &config.tracks {
        let _ = std::fs::remove_file(&t.path);
    }
}

/// The files of the take, as the notices name them.
fn kept_files(config: &WriterConfig) -> String {
    config
        .tracks
        .iter()
        .filter_map(|t| t.path.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Why a write failed, in words: a full disk says so, anything else gives the error's text.
fn write_reason(e: &hound::Error) -> String {
    match e {
        hound::Error::IoError(io) if io.kind() == std::io::ErrorKind::StorageFull => {
            i18n::t("the disk is full").to_owned()
        }
        other => other.to_string(),
    }
}

/// The notice of a take that stopped because writing failed; the files written so far stay.
fn write_failure(config: &WriterConfig, e: &hound::Error) -> String {
    i18n::tf(
        "writing the recording failed ({reason}): the recording stopped, and what was written so far was kept as {files} in the project's Audio folder and comes back as a recovered take the next time Atomik Rack starts",
        &[("reason", &write_reason(e)), ("files", &kept_files(config))],
    )
}

/// Why the laps of a take could not be stored.
enum StoreError {
    /// The buffers of a lap could not be allocated.
    TooLong,
    /// Anything else (the file, the bundle).
    Failed(String),
}

impl From<String> for StoreError {
    fn from(e: String) -> Self {
        Self::Failed(e)
    }
}

/// The notice of a take whose laps could not be stored; the files of the take stay.
fn store_failure(config: &WriterConfig, e: StoreError) -> String {
    let files = kept_files(config);
    match e {
        StoreError::TooLong => i18n::tf(
            "the recording is too long to finish in memory: the take was kept as {files} in the project's Audio folder and comes back as a recovered take the next time Atomik Rack starts",
            &[("files", &files)],
        ),
        StoreError::Failed(error) => i18n::tf(
            "{error}: the take was kept as {files} in the project's Audio folder and comes back as a recovered take the next time Atomik Rack starts",
            &[("error", &error), ("files", &files)],
        ),
    }
}

/// The buffers to read a lap of `len` frames of `channels` channels into, zero-filled, or `None`
/// when they cannot be had: the take is as long as the user played, so the request may be larger
/// than the memory, and a failed allocation of that size would abort the whole program.
/// `limit` (a test seam) refuses more than that many samples.
fn lap_buffers(channels: usize, len: usize, limit: Option<u64>) -> Option<Vec<Vec<f32>>> {
    let samples = (channels as u64).saturating_mul(len as u64);
    if limit.is_some_and(|l| samples > l) {
        return None;
    }
    let mut planar: Vec<Vec<f32>> = Vec::new();
    planar.try_reserve_exact(channels).ok()?;
    for _ in 0..channels {
        let mut channel = Vec::new();
        channel.try_reserve_exact(len).ok()?;
        // Within the reserved capacity: nothing is allocated here.
        channel.resize(len, 0.0);
        planar.push(channel);
    }
    Some(planar)
}

/// Appends `frames` frames of `drained` (tap channel buffers, from frame `from`) to every file.
fn write_frames(
    writers: &mut [Wav],
    tracks: &[WriterTrack],
    drained: &[Vec<f32>],
    from: usize,
    frames: usize,
) -> Result<(), hound::Error> {
    for (w, t) in writers.iter_mut().zip(tracks) {
        for i in from..from + frames {
            for &ch in &t.planned.tap_index {
                let s = drained
                    .get(ch)
                    .and_then(|c| c.get(i))
                    .copied()
                    .unwrap_or(0.0);
                w.write_sample(s)?;
            }
        }
    }
    Ok(())
}

fn write_silence(
    writers: &mut [Wav],
    tracks: &[WriterTrack],
    frames: u64,
) -> Result<(), hound::Error> {
    for (w, t) in writers.iter_mut().zip(tracks) {
        for _ in 0..frames {
            for _ in &t.planned.tap_index {
                w.write_sample(0.0f32)?;
            }
        }
    }
    Ok(())
}

/// The thread body. `None` when cancelled (nothing to report).
fn run(
    mut tap: RecordTap,
    config: &WriterConfig,
    ctl: &Receiver<Ctl>,
    failed: &AtomicBool,
    faults: Faults,
) -> Option<Result<TakeResult, String>> {
    let mut writers = Vec::new();
    for t in &config.tracks {
        match open_wav(t, config.sample_rate) {
            Ok(w) => writers.push(w),
            Err(e) => {
                remove_partials(config);
                return Some(Err(format!("cannot create {}: {e}", t.path.display())));
            }
        }
    }
    let rate = u64::from(config.sample_rate);
    let mut drained: Vec<Vec<f32>> = Vec::new();
    let mut first: Option<u64> = None;
    let mut next = 0u64;
    let mut gap_frames = 0u64;
    let mut unflushed = 0u64;
    let mut written = 0u64;
    let mut finish: Option<Box<FinishInfo>> = None;
    let mut quiet_polls = 0u32;
    loop {
        loop {
            match ctl.try_recv() {
                Ok(Ctl::Finish(f)) => finish = Some(f),
                Ok(Ctl::Cancel) => {
                    drop(writers);
                    remove_partials(config);
                    return None;
                }
                // The UI dropped its sender: after a finish that is normal (the take ends on
                // its own), before one the take is abandoned.
                Err(TryRecvError::Disconnected) if finish.is_none() => {
                    drop(writers);
                    remove_partials(config);
                    return None;
                }
                Err(TryRecvError::Disconnected | TryRecvError::Empty) => break,
            }
        }
        drained.iter_mut().for_each(Vec::clear);
        let got = tap.drain(&mut drained);
        if let Some((start, n)) = got {
            quiet_polls = 0;
            let begin = *first.get_or_insert_with(|| {
                next = start;
                start
            });
            debug_assert!(start >= begin);
            let mut skip = 0usize;
            let mut gap = 0u64;
            if start > next {
                gap = start - next;
            } else {
                skip = usize::try_from(next - start).unwrap_or(usize::MAX).min(n);
            }
            let end = finish.as_ref().map_or(u64::MAX, |f| f.end_engine_frame);
            let usable_from = start + skip as u64;
            let keep = n - skip;
            let keep =
                usize::try_from((end.saturating_sub(usable_from)).min(keep as u64)).unwrap_or(keep);
            let result = (|| {
                if gap > 0 && end > next {
                    let gap = gap.min(end - next);
                    write_silence(&mut writers, &config.tracks, gap)?;
                    gap_frames += gap;
                    next += gap;
                }
                if keep > 0 {
                    write_frames(&mut writers, &config.tracks, &drained, skip, keep)?;
                    next = usable_from + keep as u64;
                    unflushed += keep as u64;
                    written += keep as u64;
                    if faults.fail_after_frames.is_some_and(|n| written >= n) {
                        return Err(hound::Error::IoError(
                            std::io::ErrorKind::StorageFull.into(),
                        ));
                    }
                }
                if unflushed >= FLUSH_SECONDS * rate / 2 {
                    for w in &mut writers {
                        w.flush()?;
                    }
                    unflushed = 0;
                }
                Ok::<(), hound::Error>(())
            })();
            if let Err(e) = result {
                // Closing the files writes their headers as far as the disk allows; what they
                // hold stays for the orphan scan. The flag is set before the thread ends.
                drop(writers);
                failed.store(true, Ordering::Release);
                return Some(Err(write_failure(config, &e)));
            }
        } else if finish.is_some() {
            quiet_polls += 1;
        }
        if let Some(f) = &finish
            && (next >= f.end_engine_frame || quiet_polls >= QUIET_POLLS)
        {
            break;
        }
        if got.is_none() {
            std::thread::sleep(POLL);
        }
    }
    let finish = finish?;
    let overflow_frames = tap.overflow_frames();
    for w in writers {
        if let Err(e) = w.finalize() {
            return Some(Err(format!("finishing the recording failed: {e}")));
        }
    }
    // Frames drained before the finish arrived may run past its end: the take stops there.
    let frames = first.map_or(0, |f| {
        next.min(finish.end_engine_frame.max(f)).saturating_sub(f)
    });
    match store_laps(config, first.unwrap_or(0), frames, &finish, faults) {
        Ok(laps) => {
            remove_partials(config);
            Some(Ok(TakeResult {
                take: config.take,
                laps,
                overflow_frames,
                gap_frames,
                frames,
            }))
        }
        // The in-progress files are the only copy of what was recorded: they stay.
        Err(e) => Some(Err(store_failure(config, e))),
    }
}

/// Cuts every file into laps and stores them. A track that records a console strip is placed with
/// its own compensation (the latency of the strip's path), the others with the take's.
fn store_laps(
    config: &WriterConfig,
    first: u64,
    frames: u64,
    finish: &FinishInfo,
    faults: Faults,
) -> Result<Vec<RecordedLap>, StoreError> {
    let mut bundle = config.bundle.clone();
    let mut rule = config.stream;
    let mut laps = Vec::new();
    for track in &config.tracks {
        let range = TakeRange {
            first_engine_frame: first,
            frames,
            compensation: track
                .planned
                .input
                .compensation()
                .unwrap_or(finish.compensation),
            valid_from: finish.valid_from,
        };
        let segments = split_laps(&range, &*finish.position);
        if segments.is_empty() {
            continue;
        }
        let mut reader = WavStreamReader::open(&track.path).map_err(io)?;
        let channels = track.planned.tap_index.len();
        for seg in &segments {
            let len = usize::try_from(seg.frames).map_err(io)?;
            let mut planar =
                lap_buffers(channels, len, faults.lap_limit_samples).ok_or(StoreError::TooLong)?;
            let n = reader.read_frames(seg.first, &mut planar).map_err(io)?;
            if n < len {
                return Err(StoreError::Failed(format!(
                    "{} is shorter than expected ({n} of {len} frames)",
                    track.path.display()
                )));
            }
            let data = AudioData::new(config.sample_rate, planar).map_err(io)?;
            let name = format!("{} take {}", track.name, seg.lap + 1);
            let info = bundle.add_recording(&data, &name).map_err(io)?;
            let peaks = Arc::new(bundle.load_peaks(&info.hash).map_err(io)?);
            let asset = if rule.admit(info.frames, info.channels, true) {
                drop(data);
                open_stream_asset(&bundle, &info.hash).map_err(io)?
            } else {
                SongAsset::memory(Arc::from(data.channels))
            };
            laps.push(RecordedLap {
                device: track.planned.device.clone(),
                lap: seg.lap,
                song_frame: seg.song_frame,
                info,
                asset,
                peaks,
            });
        }
    }
    Ok(laps)
}

#[cfg(test)]
#[path = "audio_writer_tests.rs"]
mod tests;
