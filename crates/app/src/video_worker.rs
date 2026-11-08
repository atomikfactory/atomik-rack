//! The video worker: looks for FFmpeg, or looks for it and prepares a film, or takes the sound
//! out of a film, on its own thread (see [`crate::AppCore::check_ffmpeg`],
//! [`crate::AppCore::start_prepare_video`] and [`crate::AppCore::import_video_audio`]).
//!
//! Nothing about FFmpeg runs on the UI thread or the audio thread: the search starts
//! `ffmpeg -version`, the preparation runs `ffprobe` and `ffmpeg`, the audio job runs `ffprobe`,
//! then `ffmpeg` to a WAV file in a temporary folder and hands that file to the song's own
//! import (decode, resample to the song's rate, store in the bundle, peaks), all through the
//! [`atomik_video::VideoTool`] the core was given. The thread shares its progress and its cancel
//! flag with the core and reports through [`Msg::Video`], even when it panics.

use crate::audio_import::{Loaded, Msg, display_name, import_file};
use atomik_audio_file::Bundle;
use atomik_video::{
    AudioStream, Control, FfmpegStatus, Pick, Prepared, ProbeInfo, ProxyCache, ProxySpec, Search,
    VideoError, VideoTool, discover, extract_audio, lead_us, pick_stream, prepare, probe_file,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::mpsc::Sender;

/// What a worker does.
pub(crate) enum Job {
    /// Look for FFmpeg.
    Check {
        /// The folder set in Settings (`None`: the system path).
        folder: Option<PathBuf>,
    },
    /// Look for FFmpeg, then probe `source` and make or find its proxy.
    Prepare {
        /// The folder set in Settings.
        folder: Option<PathBuf>,
        /// The film.
        source: PathBuf,
        /// The proxy cache folder.
        cache: PathBuf,
        /// Report what the probe found as soon as it is known ([`Msg::VideoProbed`]), before
        /// the proxy is built (Import Video and Relink Video put the film in the song then).
        announce: bool,
    },
    /// Look for FFmpeg, probe `source`, choose an audio track, write it to a WAV file and import
    /// that file into `bundle` at `rate`.
    Audio {
        /// The folder set in Settings.
        folder: Option<PathBuf>,
        /// The film.
        source: PathBuf,
        /// The stream number of the track asked for (`None`: the only one, else the user is
        /// asked).
        stream: Option<u32>,
        /// The song's bundle (a clone: the asset is stored in the folder it points at).
        bundle: Box<Bundle>,
        /// The song's sample rate.
        rate: u32,
        /// The streaming threshold of the song's imports.
        threshold: u64,
    },
}

/// What an audio job came to.
pub(crate) enum AudioOutcome {
    /// The film has several tracks and none was asked for: the user chooses (the tracks, in file
    /// order).
    Choose(Vec<AudioStream>),
    /// The track is in the bundle.
    Imported(Box<AudioImported>),
}

/// The track an audio job stored.
pub(crate) struct AudioImported {
    /// The asset, loaded for the engine and the views.
    pub loaded: Loaded,
    /// The track it came from.
    pub stream: AudioStream,
    /// How long after the film's first frame the track's first sample sits, in microseconds.
    pub lead_us: i64,
}

/// What a finished worker found and made.
pub(crate) struct Done {
    /// FFmpeg as the search found it.
    pub status: FfmpegStatus,
    /// The film of a prepare job (`None` for a check).
    pub prepared: Option<Result<Box<Prepared>, VideoError>>,
    /// What an audio job found (`None` for the other jobs).
    pub audio: Option<Result<AudioOutcome, VideoError>>,
}

/// What the core shares with the worker: the cancel flag, the frames done and the frames
/// expected.
pub(crate) type Shared = (Arc<AtomicBool>, Arc<AtomicU64>, Arc<AtomicU64>);

/// Starts the worker. `progress` counts frames up to `total`.
pub(crate) fn spawn(
    tx: Sender<Msg>,
    job_id: u64,
    tool: Arc<dyn VideoTool>,
    job: Job,
    (cancel, progress, total): Shared,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("atomik-video".into())
        .spawn(move || {
            let mut guard = Guard {
                tx,
                job: job_id,
                kind: match job {
                    Job::Check { .. } => Kind::Check,
                    Job::Prepare { .. } => Kind::Prepare,
                    Job::Audio { .. } => Kind::Audio,
                },
                sent: false,
            };
            let announce = |probe: &ProbeInfo| {
                let _ = guard.tx.send(Msg::VideoProbed {
                    job: job_id,
                    probe: Box::new(probe.clone()),
                });
            };
            let done = run(&*tool, job, &cancel, &progress, &total, &announce);
            guard.send(done);
        })
        .map(|_| ())
}

fn search(tool: &dyn VideoTool, folder: Option<&std::path::Path>) -> FfmpegStatus {
    let path_var = std::env::var_os("PATH");
    discover(
        &Search::of_process(folder, path_var.as_deref()),
        tool,
        &|p| p.is_file(),
    )
}

fn run(
    tool: &dyn VideoTool,
    job: Job,
    cancel: &AtomicBool,
    done: &AtomicU64,
    total: &AtomicU64,
    announce: &dyn Fn(&ProbeInfo),
) -> Done {
    match job {
        Job::Check { folder } => Done {
            status: search(tool, folder.as_deref()),
            prepared: None,
            audio: None,
        },
        Job::Audio {
            folder,
            source,
            stream,
            bundle,
            rate,
            threshold,
        } => {
            let status = search(tool, folder.as_deref());
            let audio = match status.tools() {
                None => Err(VideoError::NotUsable(status.message())),
                Some(ffmpeg) => audio_of(
                    tool,
                    ffmpeg,
                    &source,
                    stream,
                    (*bundle, rate, threshold),
                    &Control {
                        cancel,
                        done,
                        total,
                    },
                ),
            };
            Done {
                status,
                prepared: None,
                audio: Some(audio),
            }
        }
        Job::Prepare {
            folder,
            source,
            cache,
            announce: tell,
        } => {
            let status = search(tool, folder.as_deref());
            let prepared = match status.tools() {
                None => Err(VideoError::NotUsable(status.message())),
                Some(ffmpeg) => {
                    probed(tell, announce, tool, ffmpeg, &source, cancel).and_then(|()| {
                        prepare(
                            tool,
                            ffmpeg,
                            &source,
                            &ProxyCache::new(cache),
                            &ProxySpec::default(),
                            &Control {
                                cancel,
                                done,
                                total,
                            },
                        )
                        .map(Box::new)
                    })
                }
            };
            Done {
                status,
                prepared: Some(prepared),
                audio: None,
            }
        }
    }
}

/// Removes the temporary folder of an audio job (and the WAV file in it) when it is dropped.
struct TempFolder(PathBuf);

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The folder an audio job writes its WAV file to: one per process and job.
fn temp_folder() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "atomik-video-audio-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// The audio job (see [`Job::Audio`]): probe, choose the track, extract it, import the file.
fn audio_of(
    tool: &dyn VideoTool,
    ffmpeg: &atomik_video::Ffmpeg,
    source: &Path,
    wanted: Option<u32>,
    (mut bundle, rate, threshold): (Bundle, u32, u64),
    ctl: &Control<'_>,
) -> Result<AudioOutcome, VideoError> {
    let probe = probe_file(tool, ffmpeg, source, ctl.cancel)?;
    let stream = match pick_stream(&probe, wanted)? {
        Pick::Choose(tracks) => return Ok(AudioOutcome::Choose(tracks.to_vec())),
        Pick::One(track) => track.clone(),
    };
    let folder = TempFolder(temp_folder());
    std::fs::create_dir_all(&folder.0)?;
    let name = source.file_stem().map_or_else(
        || display_name(source),
        |s| s.to_string_lossy().into_owned(),
    );
    let wav = folder.0.join(format!("{name} audio.wav"));
    extract_audio(tool, ffmpeg, source, (&probe, &stream), &wav, ctl)?;
    // Stopped after the file was written: nothing is stored.
    if ctl.cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(VideoError::Cancelled);
    }
    let loaded = import_file(&mut bundle, &wav, rate, threshold).map_err(VideoError::Import)?;
    Ok(AudioOutcome::Imported(Box::new(AudioImported {
        lead_us: lead_us(&probe, &stream),
        loaded,
        stream,
    })))
}

/// The probe the import and the relink need first: its answer goes out at once, so the film is in
/// the song while the proxy builds. Nothing happens for a plain preparation.
fn probed(
    tell: bool,
    announce: &dyn Fn(&ProbeInfo),
    tool: &dyn VideoTool,
    ffmpeg: &atomik_video::Ffmpeg,
    source: &std::path::Path,
    cancel: &AtomicBool,
) -> Result<(), VideoError> {
    if tell {
        announce(&probe_file(tool, ffmpeg, source, cancel)?);
    }
    Ok(())
}

/// What the job of a [`Guard`] is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Check,
    Prepare,
    Audio,
}

/// Sends the result, and the completion of the worker, even if the thread panics.
struct Guard {
    tx: Sender<Msg>,
    job: u64,
    kind: Kind,
    sent: bool,
}

impl Guard {
    fn send(&mut self, done: Done) {
        self.sent = true;
        let _ = self.tx.send(Msg::Video {
            job: self.job,
            done: Box::new(done),
        });
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.sent {
            let why = "the video worker stopped unexpectedly";
            let failure = || VideoError::Proxy(why.to_owned());
            self.send(Done {
                status: FfmpegStatus::Unchecked,
                prepared: (self.kind == Kind::Prepare).then(|| Err(failure())),
                audio: (self.kind == Kind::Audio).then(|| Err(failure())),
            });
        }
        let _ = self.tx.send(Msg::LoadDone);
    }
}
