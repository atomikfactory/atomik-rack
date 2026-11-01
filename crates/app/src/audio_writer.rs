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

use crate::audio_import::{Msg, RecDoneGuard};
use crate::audio_record::{PlannedTrack, TakeRange, split_laps};
use crate::stream_source::{open_stream_asset, should_stream};
use atomik_audio_file::{AssetInfo, AudioData, Bundle, PeakPyramid, WavStreamReader};
use atomik_engine::{RecordTap, SongAsset, SongPosition};
use atomik_model::DeviceKey;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::Arc;
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
    /// Assets above this decoded size stream from disk instead of staying in RAM.
    pub stream_threshold: u64,
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

/// Starts the writer thread for `tap`; the result arrives as [`Msg::Recorded`]. Returns the
/// control channel.
///
/// # Errors
/// The thread could not be spawned.
pub(crate) fn spawn(
    tap: RecordTap,
    config: WriterConfig,
    tx: Sender<Msg>,
) -> std::io::Result<(Sender<Ctl>, std::thread::JoinHandle<()>)> {
    let (ctl_tx, ctl_rx) = channel();
    let generation = config.generation;
    let handle = std::thread::Builder::new()
        .name("atomik-record".into())
        .spawn(move || {
            let mut guard = RecDoneGuard::new(tx, generation);
            match run(tap, &config, &ctl_rx) {
                Some(result) => guard.send(result),
                None => guard.disarm(),
            }
        })?;
    Ok((ctl_tx, handle))
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

/// Appends `frames` frames of `drained` (tap channel buffers, from frame `from`) to every file.
fn write_frames(
    writers: &mut [Wav],
    tracks: &[WriterTrack],
    drained: &[Vec<f32>],
    from: usize,
    frames: usize,
) -> Result<(), String> {
    for (w, t) in writers.iter_mut().zip(tracks) {
        for i in from..from + frames {
            for &ch in &t.planned.tap_index {
                let s = drained
                    .get(ch)
                    .and_then(|c| c.get(i))
                    .copied()
                    .unwrap_or(0.0);
                w.write_sample(s).map_err(io)?;
            }
        }
    }
    Ok(())
}

fn write_silence(writers: &mut [Wav], tracks: &[WriterTrack], frames: u64) -> Result<(), String> {
    for (w, t) in writers.iter_mut().zip(tracks) {
        for _ in 0..frames {
            for _ in &t.planned.tap_index {
                w.write_sample(0.0f32).map_err(io)?;
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
                }
                if unflushed >= FLUSH_SECONDS * rate / 2 {
                    for w in &mut writers {
                        w.flush().map_err(io)?;
                    }
                    unflushed = 0;
                }
                Ok::<(), String>(())
            })();
            if let Err(e) = result {
                return Some(Err(format!("writing the recording failed: {e}")));
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
    let result = store_laps(config, first.unwrap_or(0), frames, &finish);
    remove_partials(config);
    Some(result.map(|laps| TakeResult {
        take: config.take,
        laps,
        overflow_frames,
        gap_frames,
        frames,
    }))
}

/// Cuts every file into laps and stores them. A track that records a console strip is placed with
/// its own compensation (the latency of the strip's path), the others with the take's.
fn store_laps(
    config: &WriterConfig,
    first: u64,
    frames: u64,
    finish: &FinishInfo,
) -> Result<Vec<RecordedLap>, String> {
    let mut bundle = config.bundle.clone();
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
            let mut planar = vec![vec![0.0f32; len]; channels];
            let n = reader.read_frames(seg.first, &mut planar).map_err(io)?;
            if n < len {
                return Err(format!(
                    "{} is shorter than expected ({n} of {len} frames)",
                    track.path.display()
                ));
            }
            let data = AudioData::new(config.sample_rate, planar).map_err(io)?;
            let name = format!("{} take {}", track.name, seg.lap + 1);
            let info = bundle.add_recording(&data, &name).map_err(io)?;
            let peaks = Arc::new(bundle.load_peaks(&info.hash).map_err(io)?);
            let asset = if should_stream(info.frames, info.channels, config.stream_threshold) {
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
