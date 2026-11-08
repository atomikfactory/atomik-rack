//! Video in the app core: finding the user's FFmpeg and preparing a film.
//!
//! Video is decoded by the FFmpeg the user installed (the `atomik-video` crate holds the search,
//! the probe and the proxy). The core runs two jobs on a worker thread
//! ([`crate::video_worker`]), one at a time:
//!
//! * **Check FFmpeg** ([`AppCore::check_ffmpeg`]) looks in the folder set in Settings (else on
//!   the system path), runs `ffmpeg -version` and keeps the answer ([`AppCore::video_status`]):
//!   found with its version, missing, too old or broken. A machine without FFmpeg is not an
//!   error anywhere: the status says what to do and the video features are off.
//! * **Prepare Video** ([`AppCore::start_prepare_video`]) probes a film and builds (or finds in
//!   the per-user cache) its low-resolution proxy, the file the viewer and the thumbnails read.
//!   The result is kept in [`AppCore::prepared_videos`] for the session. The song is not
//!   touched: this item changes no project data.
//! * **Cancel** ([`AppCore::cancel_video`]) stops the worker, which kills ffmpeg and removes the
//!   half-written proxy; closing the app does the same.
//!
//! Progress is the frames written over the frames expected ([`AppCore::video_progress`], shown
//! in the status line). The toast says how a job ended.

use super::*;
use crate::audio_import::display_name;
use crate::video_worker::{self, Done, Job};
use atomik_video::{FfmpegStatus, NoVideoTools, Prepared, VideoError, VideoTool};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The video job that runs.
struct Running {
    job: u64,
    /// The film's file name for a prepare job; `None` for a check.
    film: Option<String>,
    /// A check nobody asked for (the Settings tab opened): it says nothing in a toast.
    quiet: bool,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

/// The video state of the core: it outlives the open project (a job goes on when the song is
/// replaced).
pub(super) struct VideoState {
    tool: Arc<dyn VideoTool>,
    folder: Option<PathBuf>,
    cache: Option<PathBuf>,
    status: FfmpegStatus,
    running: Option<Running>,
    next_job: u64,
    prepared: Vec<Prepared>,
}

impl Default for VideoState {
    /// No tools (a headless run never starts a program); the platform gives the real ones.
    fn default() -> Self {
        Self {
            tool: Arc::new(NoVideoTools),
            folder: None,
            cache: None,
            status: FfmpegStatus::Unchecked,
            running: None,
            next_job: 0,
            prepared: Vec::new(),
        }
    }
}

impl VideoState {
    /// Stops the running job (the core goes away).
    pub(super) fn stop(&self) {
        if let Some(r) = &self.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// The folder proxies go to when the platform names none.
fn fallback_cache() -> PathBuf {
    std::env::temp_dir().join("atomik-rack-video-cache")
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ setup

    /// Sets the programs video runs (the platform passes [`atomik_video::SystemTool`]; the core
    /// starts without any).
    pub fn set_video_tool(&mut self, tool: Arc<dyn VideoTool>) {
        self.video.tool = tool;
    }

    /// Sets the folder FFmpeg is looked for in (`None` or empty: the system path). A change
    /// forgets what was found, so the next check or preparation looks again.
    pub fn set_video_folder(&mut self, folder: Option<PathBuf>) {
        let folder = folder.filter(|f| !f.as_os_str().is_empty());
        if folder != self.video.folder {
            self.video.folder = folder;
            self.video.status = FfmpegStatus::Unchecked;
        }
    }

    /// Sets the per-user folder the proxies are cached in.
    pub fn set_video_cache(&mut self, dir: PathBuf) {
        self.video.cache = Some(dir);
    }

    // ------------------------------------------------------------------ queries

    /// What the last check found.
    pub fn video_status(&self) -> &FfmpegStatus {
        &self.video.status
    }

    /// The folder FFmpeg is looked for in, `None` for the system path.
    pub fn video_folder(&self) -> Option<&Path> {
        self.video.folder.as_deref()
    }

    /// True while FFmpeg is being looked for or a film is being prepared.
    pub fn video_busy(&self) -> bool {
        self.video.running.is_some()
    }

    /// The films prepared in this session, oldest first.
    pub fn prepared_videos(&self) -> &[Prepared] {
        &self.video.prepared
    }

    /// The preparation of `film`, when this session made or found one.
    pub fn prepared_video(&self, film: &Path) -> Option<&Prepared> {
        self.video.prepared.iter().rev().find(|p| p.source == film)
    }

    /// The fraction done, `0.0..=1.0`, and a line saying what runs; `None` for a check (it is
    /// over before a bar would show) and when nothing runs.
    pub fn video_progress(&self) -> Option<(f32, String)> {
        let r = self.video.running.as_ref()?;
        let film = r.film.as_ref()?;
        let total = r.total.load(Ordering::Relaxed);
        let done = r.progress.load(Ordering::Relaxed);
        if total == 0 {
            return Some((0.0, format!("Preparing video {film}")));
        }
        let fraction = (done as f64 / total as f64).clamp(0.0, 1.0) as f32;
        Some((
            fraction,
            format!("Preparing video {film}: {:.0}%", fraction * 100.0),
        ))
    }

    // ------------------------------------------------------------------ commands

    /// Starts looking for FFmpeg (File > Video > Check FFmpeg); the toast says what was found.
    ///
    /// # Errors
    /// A video job runs already or the worker cannot start; the message is also the toast.
    pub fn check_ffmpeg(&mut self) -> Result<(), String> {
        if self.video.running.is_some() {
            return self.video_refused("A video job is already running".to_owned());
        }
        let job = Job::Check {
            folder: self.video.folder.clone(),
        };
        self.spawn_video(job, None, false)
    }

    /// Looks for FFmpeg without a toast when it has not been looked for since the folder was
    /// set (the Video tab of Settings shows the answer). Does nothing when it is known or a
    /// job runs.
    pub fn refresh_ffmpeg_status(&mut self) {
        if self.video.status != FfmpegStatus::Unchecked || self.video.running.is_some() {
            return;
        }
        let job = Job::Check {
            folder: self.video.folder.clone(),
        };
        let _ = self.spawn_video(job, None, true);
    }

    /// Starts preparing `film`: FFmpeg is looked for again, the film probed and its proxy made
    /// or found in the cache (see the module docs).
    ///
    /// # Errors
    /// A video job runs already, `film` is not a file, or the worker cannot start; the message
    /// is also the toast. A machine without FFmpeg is reported by the worker.
    pub fn start_prepare_video(&mut self, film: PathBuf) -> Result<(), String> {
        if self.video.running.is_some() {
            return self.video_refused("A video job is already running".to_owned());
        }
        if !film.is_file() {
            return self.video_refused(format!("{} is not a file", film.display()));
        }
        let name = display_name(&film);
        let job = Job::Prepare {
            folder: self.video.folder.clone(),
            source: film,
            cache: self.video.cache.clone().unwrap_or_else(fallback_cache),
        };
        self.spawn_video(job, Some(name), false)
    }

    /// Stops the running video job; it leaves no half-written proxy.
    pub fn cancel_video(&mut self) {
        match &self.video.running {
            Some(r) => r.cancel.store(true, Ordering::Relaxed),
            None => self.reject("No video job is running".to_owned()),
        }
    }

    fn video_refused(&mut self, why: String) -> Result<(), String> {
        self.reject(why.clone());
        Err(why)
    }

    fn spawn_video(&mut self, job: Job, film: Option<String>, quiet: bool) -> Result<(), String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(0));
        let id = self.video.next_job;
        self.video.next_job += 1;
        let started = video_worker::spawn(
            self.audio.tx.clone(),
            id,
            Arc::clone(&self.video.tool),
            job,
            (
                Arc::clone(&cancel),
                Arc::clone(&progress),
                Arc::clone(&total),
            ),
        );
        match started {
            Ok(()) => {
                self.audio.pending += 1;
                let message = match &film {
                    Some(name) => format!("Preparing video {name}..."),
                    None => "Looking for FFmpeg...".to_owned(),
                };
                self.video.running = Some(Running {
                    job: id,
                    film,
                    quiet,
                    cancel,
                    progress,
                    total,
                });
                if !quiet {
                    self.reject(message);
                }
                Ok(())
            }
            Err(e) => self.video_refused(format!("Cannot start the video worker: {e}")),
        }
    }

    /// A video job finished (or failed, or was stopped): keeps what it found and says so.
    pub(super) fn apply_video(&mut self, job: u64, done: Done) {
        let Some(running) = self.video.running.take_if(|r| r.job == job) else {
            return;
        };
        if done.status != FfmpegStatus::Unchecked {
            self.video.status = done.status;
        }
        let Some(result) = done.prepared else {
            if !running.quiet {
                let message = self.video.status.message();
                self.reject(message);
            }
            return;
        };
        let film = running.film.unwrap_or_default();
        match result {
            Ok(prepared) => {
                let p = *prepared;
                let (width, height) = (p.info.width, p.info.height);
                let how = if p.reused { "from the cache" } else { "built" };
                let message = format!(
                    "Prepared {film}: {:.1} s at {:.3} fps, proxy {width}x{height} ({how})",
                    p.probe.duration_s,
                    p.probe.video.frame_rate.as_f64(),
                );
                self.video.prepared.retain(|q| q.source != p.source);
                self.video.prepared.push(p);
                self.reject(message);
            }
            Err(VideoError::Cancelled) => self.reject(format!("Preparing {film} cancelled")),
            Err(VideoError::NotUsable(message)) => self.reject(message),
            Err(e) => self.reject(format!("Preparing {film} failed: {e}")),
        }
    }
}
