//! The video worker: looks for FFmpeg, or looks for it and prepares a film, on its own thread
//! (see [`crate::AppCore::check_ffmpeg`] and [`crate::AppCore::start_prepare_video`]).
//!
//! Nothing about FFmpeg runs on the UI thread or the audio thread: the search starts
//! `ffmpeg -version`, the preparation runs `ffprobe` and `ffmpeg`, all through the
//! [`atomik_video::VideoTool`] the core was given. The thread shares its progress and its cancel
//! flag with the core and reports through [`Msg::Video`], even when it panics.

use crate::audio_import::Msg;
use atomik_video::{
    Control, FfmpegStatus, Prepared, ProbeInfo, ProxyCache, ProxySpec, Search, VideoError,
    VideoTool, discover, prepare, probe_file,
};
use std::path::PathBuf;
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
}

/// What a finished worker found and made.
pub(crate) struct Done {
    /// FFmpeg as the search found it.
    pub status: FfmpegStatus,
    /// The film of a prepare job (`None` for a check).
    pub prepared: Option<Result<Box<Prepared>, VideoError>>,
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
                prepares: matches!(job, Job::Prepare { .. }),
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
        },
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
            }
        }
    }
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

/// Sends the result, and the completion of the worker, even if the thread panics.
struct Guard {
    tx: Sender<Msg>,
    job: u64,
    prepares: bool,
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
            self.send(Done {
                status: FfmpegStatus::Unchecked,
                prepared: self
                    .prepares
                    .then(|| Err(VideoError::Proxy(why.to_owned()))),
            });
        }
        let _ = self.tx.send(Msg::LoadDone);
    }
}
