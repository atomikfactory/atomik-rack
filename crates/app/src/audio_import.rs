//! Audio import helpers: which files are audio, how long a clip for an
//! asset is, and the background workers that decode, store and load assets off the UI thread.
//!
//! The worker side is deliberately message-passing only: a worker owns a clone of the project
//! [`Bundle`], does the slow work (decode, resample, write the asset and its peaks, read them
//! back into RAM) and sends `Msg`s over one channel; [`crate::AppCore`] drains the channel
//! on the UI thread (every frame, and in tests through `wait_background`) and applies the
//! results to the document. A `DoneGuard` sends the completion message even if a worker
//! panics, so a counter of running workers can never get stuck.

use crate::audio_writer::TakeResult;
use crate::stream_source::{open_stream_asset, should_stream};
use atomik_audio_file::{AssetInfo, Bundle, FileMeta, PeakPyramid};
use atomik_device_sdk::SampleBuffer;
use atomik_engine::SongAsset;
use atomik_model::Song;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;

/// File extensions the import dialog offers (decoded by `atomik-audio-file`).
pub const AUDIO_EXTENSIONS: [&str; 6] = ["wav", "aif", "aiff", "flac", "mp3", "ogg"];

/// True if `path` has one of the [`AUDIO_EXTENSIONS`] (any case).
pub fn is_audio_path(path: &Path) -> bool {
    path.extension().is_some_and(|e| {
        AUDIO_EXTENSIONS
            .iter()
            .any(|known| e.eq_ignore_ascii_case(known))
    })
}

/// The length in ticks of a clip that plays `frames` frames starting at `start_tick`: the
/// smallest whole number of ticks whose span, through the song's tempo map
/// ([`Song::frame_at`]), is at least `frames` (audio is not stretched). At least 1, at
/// most [`atomik_model::song::MAX_TICK`].
pub fn ticks_for_frames(song: &Song, start_tick: i64, frames: u64, sample_rate: u32) -> i64 {
    atomik_model::ticks_for_frames(song, start_tick, frames, sample_rate)
}

/// A decoded asset ready for the engine and the view.
pub(crate) struct Loaded {
    /// Asset id.
    pub hash: String,
    /// What the model stores (set by imports; `None` for assets loaded on open).
    pub info: Option<AssetInfo>,
    /// How the engine plays it: all samples in RAM, or streamed from the bundle's file above
    /// the streaming threshold ([`crate::stream_source`]).
    pub asset: SongAsset,
    /// The waveform peaks.
    pub peaks: Arc<PeakPyramid>,
    /// What the source file says about its tempo (set by imports; empty otherwise).
    pub meta: FileMeta,
}

/// What workers tell the UI thread.
pub(crate) enum Msg {
    /// One file of an import batch finished (or failed).
    Imported {
        generation: u64,
        batch: u64,
        path: PathBuf,
        result: Result<Loaded, String>,
    },
    /// An import worker has finished all its files.
    BatchDone { generation: u64 },
    /// One asset of an open project was loaded (or failed).
    Loaded {
        generation: u64,
        hash: String,
        result: Result<Loaded, String>,
    },
    /// A load worker has finished.
    LoadDone,
    /// An asset was loaded fully into RAM for a sampler, or failed.
    Sampled {
        generation: u64,
        hash: String,
        result: Result<Arc<SampleBuffer>, String>,
    },
    /// Transient detection of a loop slicer's loop finished: the markers or the
    /// failure, with the job that asked.
    Sliced {
        generation: u64,
        job: SliceJob,
        result: Result<Vec<u64>, String>,
    },
    /// A folder scan for missing audio finished: the files found, see
    /// [`crate::audio_relink`].
    Scanned {
        generation: u64,
        root: PathBuf,
        found: Vec<(String, PathBuf)>,
        truncated: bool,
    },
    /// A warp buffer was rendered (`None`: the job was cancelled): see [`crate::AppCore`]'s
    /// warp cache.
    Warped {
        generation: u64,
        key: u64,
        result: Option<SongAsset>,
    },
    /// A freeze or bounce render finished, stored in the bundle (`Ok(None)`: it was cancelled), or
    /// failed: see [`crate::freeze_worker`].
    Frozen {
        generation: u64,
        job: u64,
        result: Box<Result<Option<crate::freeze_worker::Done>, String>>,
    },
    /// An audio export finished and wrote its file (`Ok(None)`: it was cancelled, nothing was
    /// written), or failed: see [`crate::export_worker`].
    Exported {
        generation: u64,
        job: u64,
        result: Box<Result<Option<atomik_render::export::ExportReport>, String>>,
    },
    /// The notes of an audio clip were extracted (or it failed): see the Audio to Notes
    /// section of [`crate::AppCore`].
    Noted {
        generation: u64,
        job: u64,
        result: Result<NotesFound, String>,
    },
    /// An audio take was written and stored (or failed): see [`crate::audio_writer`].
    Recorded {
        generation: u64,
        result: Result<TakeResult, String>,
    },
}

/// What a notes worker found in the audio of a clip.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NotesFound {
    /// The notes, in frames from the start of the analysed audio.
    pub notes: Vec<atomik_model::FoundNote>,
    /// True when the clip is longer than the analysis reads (only its start was analysed).
    pub truncated: bool,
}

/// What a transient-detection worker was asked for: carried through the
/// worker and back, so the UI thread knows what to apply the markers to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SliceJob {
    /// The loop slicer.
    pub device: atomik_model::DeviceKey,
    /// The loop asset.
    pub asset: String,
    /// Frames of the loop asset.
    pub frames: u64,
    /// The session sample rate.
    pub rate: u32,
    /// Sensitivity in percent (0..=100).
    pub sensitivity: u8,
    /// `Some(beats)` for a fresh loop (set `loop.beats`, replace the zones); `None` for a
    /// re-slice of the current loop (keep the per-slice fields).
    pub load_beats: Option<u32>,
}

/// Where a detection worker gets the loop's samples from.
pub(crate) enum LoopSource {
    /// The RAM buffer the sampler cache holds.
    Buffer(Arc<SampleBuffer>),
    /// A streamed asset: the worker reads the whole file itself.
    File(Bundle, String),
}

/// Starts a worker that detects the transients of `source` and sends [`Msg::Sliced`].
pub(crate) fn spawn_onsets(
    tx: Sender<Msg>,
    generation: u64,
    job: SliceJob,
    source: LoopSource,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-onsets".into())
        .spawn(move || {
            let _done = DoneGuard(tx.clone(), Some(Msg::LoadDone));
            let result = (|| {
                let buffer = match source {
                    LoopSource::Buffer(b) => b,
                    LoopSource::File(bundle, hash) => load_sample_buffer(&bundle, &hash)?,
                };
                let channels: Vec<&[f32]> = buffer.data.iter().map(Vec::as_slice).collect();
                Ok(atomik_dsp::onset::detect_onsets(
                    &channels,
                    job.rate,
                    f32::from(job.sensitivity.min(100)) / 100.0,
                ))
            })();
            let _ = tx.send(Msg::Sliced {
                generation,
                job,
                result,
            });
        })
        .map(|_| ())
}

/// Starts a worker that renders the warp buffer of `job` from `asset` and sends
/// [`Msg::Warped`]; `cancel` stops it early (the result is then `None`).
pub(crate) fn spawn_warp(
    tx: Sender<Msg>,
    generation: u64,
    job: atomik_model::WarpJob,
    asset: SongAsset,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-warp".into())
        .spawn(move || {
            let _done = DoneGuard(tx.clone(), Some(Msg::LoadDone));
            let stop = || cancel.load(std::sync::atomic::Ordering::Relaxed);
            let result = atomik_render::warp::render_warp(&job, &asset, &stop);
            let _ = tx.send(Msg::Warped {
                generation,
                key: job.key(),
                result,
            });
        })
        .map(|_| ())
}

/// Starts a worker that runs `work` (the analysis of one clip) and sends [`Msg::Noted`]; a panic
/// sends a failure instead, so the job can never stay running.
pub(crate) fn spawn_notes(
    tx: Sender<Msg>,
    generation: u64,
    job: u64,
    work: Box<dyn FnOnce() -> Result<NotesFound, String> + Send>,
) -> std::io::Result<()> {
    struct Sent {
        tx: Sender<Msg>,
        generation: u64,
        job: u64,
        sent: bool,
    }
    impl Sent {
        fn send(&mut self, result: Result<NotesFound, String>) {
            self.sent = true;
            let _ = self.tx.send(Msg::Noted {
                generation: self.generation,
                job: self.job,
                result,
            });
        }
    }
    impl Drop for Sent {
        fn drop(&mut self) {
            if !self.sent {
                self.send(Err("the analysis stopped unexpectedly".to_owned()));
            }
        }
    }
    std::thread::Builder::new()
        .name("atomik-notes".into())
        .spawn(move || {
            let _done = DoneGuard(tx.clone(), Some(Msg::LoadDone));
            let mut sent = Sent {
                tx,
                generation,
                job,
                sent: false,
            };
            let result = work();
            sent.send(result);
        })
        .map(|_| ())
}

/// Sends the failure of a recording writer thread that ended without a result (a panic), and
/// is disarmed by a result or a cancel.
pub(crate) struct RecDoneGuard {
    tx: Sender<Msg>,
    generation: u64,
    armed: bool,
}

impl RecDoneGuard {
    pub(crate) fn new(tx: Sender<Msg>, generation: u64) -> Self {
        Self {
            tx,
            generation,
            armed: true,
        }
    }

    /// Sends the take's result.
    pub(crate) fn send(&mut self, result: Result<TakeResult, String>) {
        self.armed = false;
        let _ = self.tx.send(Msg::Recorded {
            generation: self.generation,
            result,
        });
    }

    /// The take was cancelled: nothing to report.
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RecDoneGuard {
    fn drop(&mut self) {
        if self.armed {
            self.send(Err("the recording writer stopped unexpectedly".to_owned()));
        }
    }
}

/// Sends a message when dropped (the completion of a worker, panic or not).
struct DoneGuard(Sender<Msg>, Option<Msg>);

impl Drop for DoneGuard {
    fn drop(&mut self) {
        if let Some(msg) = self.1.take() {
            let _ = self.0.send(msg);
        }
    }
}

/// The file name of `path` for messages.
pub(crate) fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Prepares an asset of `bundle` for the engine, with its peaks: fully loaded into memory, or
/// (decoded size above `threshold` bytes) opened for streaming from its file.
pub(crate) fn load_asset(bundle: &Bundle, hash: &str, threshold: u64) -> Result<Loaded, String> {
    let reader = bundle.stream_reader(hash).map_err(|e| e.to_string())?;
    let streamed = u16::try_from(reader.channel_count())
        .is_ok_and(|ch| should_stream(reader.frames(), ch, threshold));
    drop(reader);
    let asset = if streamed {
        open_stream_asset(bundle, hash).map_err(|e| e.to_string())?
    } else {
        let data = bundle.load(hash).map_err(|e| e.to_string())?;
        SongAsset::memory(Arc::from(data.channels))
    };
    let peaks = bundle.load_peaks(hash).map_err(|e| e.to_string())?;
    Ok(Loaded {
        hash: hash.to_owned(),
        info: None,
        asset,
        peaks: Arc::new(peaks),
        meta: FileMeta::default(),
    })
}

/// Decodes `path`, converts it to `rate`, stores it in `bundle` and loads it back.
pub(crate) fn import_file(
    bundle: &mut Bundle,
    path: &Path,
    rate: u32,
    threshold: u64,
) -> Result<Loaded, String> {
    let info = bundle.import(path, rate).map_err(|e| e.to_string())?;
    let mut loaded = load_asset(bundle, &info.hash, threshold)?;
    loaded.meta = atomik_audio_file::read_file_meta(path);
    loaded.info = Some(info);
    Ok(loaded)
}

/// Starts a worker that imports `paths` one after another into `bundle`.
pub(crate) fn spawn_import(
    tx: Sender<Msg>,
    generation: u64,
    batch: u64,
    mut bundle: Bundle,
    rate: u32,
    threshold: u64,
    paths: Vec<PathBuf>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-import".into())
        .spawn(move || {
            let _done = DoneGuard(tx.clone(), Some(Msg::BatchDone { generation }));
            for path in paths {
                let result = import_file(&mut bundle, &path, rate, threshold);
                let _ = tx.send(Msg::Imported {
                    generation,
                    batch,
                    path,
                    result,
                });
            }
        })
        .map(|_| ())
}

/// Starts a worker that scans `root` for the files of `wanted` and sends [`Msg::Scanned`].
pub(crate) fn spawn_scan(
    tx: Sender<Msg>,
    generation: u64,
    root: PathBuf,
    wanted: Vec<crate::audio_relink::Wanted>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-relink-scan".into())
        .spawn(move || {
            let _done = DoneGuard(tx.clone(), Some(Msg::LoadDone));
            let result = crate::audio_relink::scan_folder(&root, &wanted);
            let _ = tx.send(Msg::Scanned {
                generation,
                root,
                found: result.found,
                truncated: result.truncated,
            });
        })
        .map(|_| ())
}

/// Starts a worker that loads the assets `hashes` of `bundle`.
pub(crate) fn spawn_load(
    tx: Sender<Msg>,
    generation: u64,
    bundle: Bundle,
    threshold: u64,
    hashes: Vec<String>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-load".into())
        .spawn(move || {
            let _done = DoneGuard(tx.clone(), Some(Msg::LoadDone));
            for hash in hashes {
                let result = load_asset(&bundle, &hash, threshold);
                let _ = tx.send(Msg::Loaded {
                    generation,
                    hash,
                    result,
                });
            }
        })
        .map(|_| ())
}

/// Brings the asset `hash` of a library item's bundle `from` into `to` (a file copy, unless
/// `to` has it already) and loads it.
fn adopt_asset(from: &Path, to: &Bundle, hash: &str, threshold: u64) -> Result<Loaded, String> {
    if !to.exists(hash) {
        let source = Bundle::open(from)
            .map_err(|_| "the item has no audio folder".to_string())?
            .asset_path(hash);
        if !source.is_file() {
            return Err("the item does not hold this audio".to_string());
        }
        std::fs::create_dir_all(to.audio_dir()).map_err(|e| e.to_string())?;
        crate::browser_workers::copy_atomic(&source, &to.asset_path(hash))
            .map_err(|e| e.to_string())?;
    }
    load_asset(to, hash, threshold)
}

/// Starts a worker that adopts the assets `hashes` of the item bundle `from` into `to`: it
/// copies the files `to` lacks, then loads them (a failure comes back as a failed load of that
/// asset, which the core lists as missing).
pub(crate) fn spawn_adopt(
    tx: Sender<Msg>,
    generation: u64,
    from: PathBuf,
    to: Bundle,
    threshold: u64,
    hashes: Vec<String>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-adopt".into())
        .spawn(move || {
            let _done = DoneGuard(tx.clone(), Some(Msg::LoadDone));
            for hash in hashes {
                let result = adopt_asset(&from, &to, &hash, threshold);
                let _ = tx.send(Msg::Loaded {
                    generation,
                    hash,
                    result,
                });
            }
        })
        .map(|_| ())
}

/// The whole asset `hash` of `bundle` as a sampler buffer, always in RAM whatever its size.
pub(crate) fn load_sample_buffer(bundle: &Bundle, hash: &str) -> Result<Arc<SampleBuffer>, String> {
    let data = bundle.load(hash).map_err(|e| e.to_string())?;
    SampleBuffer::new(Arc::from(data.channels))
        .map(Arc::new)
        .map_err(|e| e.to_string())
}

/// Starts a worker that loads the assets `hashes` of `bundle` fully into RAM for samplers (a
/// streamed audio-track asset has no memory copy: sampler zones always play from RAM).
pub(crate) fn spawn_sample_load(
    tx: Sender<Msg>,
    generation: u64,
    bundle: Bundle,
    hashes: Vec<String>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-sample-load".into())
        .spawn(move || {
            let _done = DoneGuard(tx.clone(), Some(Msg::LoadDone));
            for hash in hashes {
                let result = load_sample_buffer(&bundle, &hash);
                let _ = tx.send(Msg::Sampled {
                    generation,
                    hash,
                    result,
                });
            }
        })
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::PPQ;
    use atomik_model::song::MAX_TICK;
    use atomik_model::{TempoEvent, empty_song};

    fn song_at(bpm: f64) -> Song {
        empty_song(bpm)
    }

    #[test]
    fn audio_extensions_are_recognised_in_any_case() {
        for name in [
            "a.wav", "a.WAV", "a.aif", "a.aiff", "a.flac", "a.Mp3", "a.ogg",
        ] {
            assert!(is_audio_path(Path::new(name)), "{name}");
        }
        for name in ["a.txt", "a", "a.atomik", "wav", ".wav.bak", "a.m4a"] {
            assert!(!is_audio_path(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn a_clip_is_as_long_as_its_asset_rounded_up_to_whole_ticks() {
        let song = song_at(120.0);
        // 120 BPM at 48 kHz: one tick is 1.5625 frames, so 100 000 frames are 64 000 ticks.
        assert_eq!(ticks_for_frames(&song, 0, 100_000, 48_000), 64_000);
        assert_eq!(ticks_for_frames(&song, 0, 100_001, 48_000), 64_001);
        assert_eq!(ticks_for_frames(&song, 0, 99_999, 48_000), 64_000);
        // One beat is 24 000 frames.
        assert_eq!(ticks_for_frames(&song, 0, 24_000, 48_000), PPQ);
        assert_eq!(ticks_for_frames(&song, 0, 1, 48_000), 1);
        assert_eq!(ticks_for_frames(&song, 0, 0, 48_000), 1, "never empty");
    }

    #[test]
    fn the_length_is_the_smallest_that_covers_the_asset() {
        let song = song_at(97.0);
        for frames in [1u64, 7, 480, 12_345, 48_000, 777_777] {
            for start in [0i64, 1_000, 3 * PPQ + 17] {
                let len = ticks_for_frames(&song, start, frames, 48_000);
                let span = |l: i64| song.frame_at(start + l, 48_000) - song.frame_at(start, 48_000);
                assert!(span(len) >= frames, "{frames} at {start}: {len}");
                assert!(
                    len == 1 || span(len - 1) < frames,
                    "{frames} at {start}: {len}"
                );
            }
        }
    }

    #[test]
    fn the_length_follows_the_tempo_map_across_a_tempo_change() {
        let mut song = song_at(120.0);
        song.tempo = vec![TempoEvent::step(0, 120.0), TempoEvent::step(4 * PPQ, 60.0)];
        // From beat 3: one beat at 120 BPM (24 000 frames), then 72 000 frames at 60 BPM
        // (1.5 beats), so 2 s of audio is 2.5 beats.
        assert_eq!(
            ticks_for_frames(&song, 3 * PPQ, 96_000, 48_000),
            5 * PPQ / 2
        );
        // Entirely inside the slow part: 2 s is 2 beats.
        assert_eq!(ticks_for_frames(&song, 5 * PPQ, 96_000, 48_000), 2 * PPQ);
    }

    #[test]
    fn a_huge_asset_is_capped_at_the_largest_tick() {
        let song = song_at(999.0);
        assert_eq!(ticks_for_frames(&song, 0, u64::MAX / 2, 48_000), MAX_TICK);
    }
}
