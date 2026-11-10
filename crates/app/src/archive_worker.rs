//! The project archive worker: packs a project and its audio into one `.atomikz` file, or unpacks
//! one, on its own thread (see [`crate::AppCore::start_archive_export`]). A share
//! ([`ShareJob`], see [`crate::AppCore::start_share`]) is a pack that first renders a preview
//! and the stems of the song, on the same thread, with one progress counter and one cancel.
//!
//! The format and its safety checks belong to the `atomik-audio-file` crate; this thread only runs
//! them, shares the progress and the cancel flag with the UI thread and reports through
//! [`Msg::Archived`] followed by the usual completion message, even when the thread panics.

use crate::audio_import::Msg;
use crate::export_worker::{self, Work};
use atomik_audio_file::{
    ArchiveError, ArchiveExtras, ArchiveLimits, ArchiveReport, Extracted, archive_size,
    archive_work_with, extract_archive, is_stem_file, write_archive, write_archive_with,
};
use atomik_render::plugin_pass::RenderControl;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;

/// The share of an archive's progress that one render is worth, in the unit of the counter.
pub(crate) const RENDER_UNITS: u64 = 1_000_000;

/// What a worker does.
pub(crate) enum Job {
    /// Pack: write `dest` from the project text and the audio files.
    Pack {
        /// The archive to write.
        dest: PathBuf,
        /// The project's name inside the archive.
        name: String,
        /// The project text, stored byte for byte.
        project: Vec<u8>,
        /// The audio files, `(asset hash, path)`.
        audio: Vec<(String, PathBuf)>,
    },
    /// Share: render the preview and the stems, then pack everything.
    Share(Box<ShareJob>),
    /// Unpack: extract `archive` into the new folder `dest`.
    Unpack {
        /// The archive to read.
        archive: PathBuf,
        /// The folder to make.
        dest: PathBuf,
    },
}

/// A share: the archive of a project with a listening copy, the stems and the credits.
pub(crate) struct ShareJob {
    /// The archive to write.
    pub dest: PathBuf,
    /// The project's name inside the archive.
    pub name: String,
    /// The project text, stored byte for byte.
    pub project: Vec<u8>,
    /// The audio files, `(asset hash, path)`.
    pub audio: Vec<(String, PathBuf)>,
    /// The text of `credits.txt`.
    pub credits: String,
    /// The folder the renders go to; made here and removed when the job ends, whatever the end.
    pub temp: PathBuf,
    /// The preview to render and the file it writes (an `.mp3` or `.wav` inside `temp`).
    pub preview: Option<(Work, PathBuf)>,
    /// The stems to render (into a folder inside `temp`).
    pub stems: Option<Work>,
}

/// What a finished worker made.
#[derive(Debug)]
pub(crate) enum Done {
    /// An archive was written.
    Packed {
        /// What it holds.
        report: ArchiveReport,
    },
    /// An archive was unpacked.
    Unpacked(Extracted),
}

/// Starts the worker. `progress` counts bytes up to `total`, which the worker sets when it
/// knows it (an unpack reads the directory first); `cancel` stops it and removes what it made.
pub(crate) fn spawn(
    tx: Sender<Msg>,
    job_id: u64,
    job: Job,
    (cancel, progress, total): (Arc<AtomicBool>, Arc<AtomicU64>, Arc<AtomicU64>),
) -> std::io::Result<()> {
    #[cfg(test)]
    let hold = hold::take();
    std::thread::Builder::new()
        .name("atomik-archive".into())
        .spawn(move || {
            #[cfg(test)]
            hold::wait(hold);
            let mut guard = Guard {
                tx,
                job: job_id,
                sent: false,
            };
            let limits = ArchiveLimits::default();
            let result = match job {
                Job::Pack {
                    dest,
                    name,
                    project,
                    audio,
                } => write_archive(&dest, &name, &project, &audio, &cancel, &progress)
                    .map(|report| Done::Packed { report }),
                Job::Share(job) => run_share(*job, &cancel, &progress, &total)
                    .map(|report| Done::Packed { report }),
                Job::Unpack { archive, dest } => archive_size(&archive, &limits)
                    .and_then(|bytes| {
                        total.store(bytes, std::sync::atomic::Ordering::Relaxed);
                        extract_archive(&archive, &dest, &limits, &cancel, &progress)
                    })
                    .map(Done::Unpacked),
            };
            guard.send(result);
        })
        .map(|_| ())
}

/// Removes the folder of the renders when dropped.
struct TempFolder(PathBuf);

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs one render of a share: `base` units of the counter are done before it. `None`: the
/// share was cancelled.
fn render_step(
    work: Work,
    base: u64,
    what: &str,
    (cancel, progress): (&AtomicBool, &AtomicU64),
) -> Result<export_worker::Done, ArchiveError> {
    let report = |p: f32| {
        let at = f64::from(p.clamp(0.0, 1.0)) * RENDER_UNITS as f64;
        progress.store(base + at as u64, Ordering::Relaxed);
    };
    let done = export_worker::run(
        work,
        RenderControl {
            cancel: Some(cancel),
            progress: Some(&report),
        },
    )
    .map_err(|e| ArchiveError::Failed(format!("the {what} could not be made: {e}")))?;
    done.ok_or(ArchiveError::Cancelled)
}

/// The share: the preview, the stems, then the archive. Any end (a cancel, a failure) removes the
/// renders and leaves no archive.
fn run_share(
    job: ShareJob,
    cancel: &AtomicBool,
    progress: &AtomicU64,
    total: &AtomicU64,
) -> Result<ArchiveReport, ArchiveError> {
    let ShareJob {
        dest,
        name,
        project,
        audio,
        credits,
        temp,
        preview,
        stems,
    } = job;
    std::fs::create_dir_all(&temp).map_err(|source| ArchiveError::Io {
        path: temp.clone(),
        source,
    })?;
    let _temp = TempFolder(temp);
    let mut extras = ArchiveExtras {
        credits: Some(credits),
        ..ArchiveExtras::default()
    };
    let mut base = 0u64;
    if let Some((work, path)) = preview {
        match render_step(work, base, "preview", (cancel, progress))? {
            export_worker::Done::File(_) => extras.preview = Some(path),
            _ => return Err(ArchiveError::Failed("the preview is not a file".to_owned())),
        }
        base += RENDER_UNITS;
        progress.store(base, Ordering::Relaxed);
    }
    if let Some(work) = stems {
        match render_step(work, base, "stems", (cancel, progress))? {
            export_worker::Done::Stems(report) => {
                extras.stems = report
                    .files
                    .iter()
                    .map(|(path, _)| path.clone())
                    .filter(|path| {
                        path.file_name()
                            .is_some_and(|n| is_stem_file(&n.to_string_lossy()))
                    })
                    .collect();
            }
            _ => return Err(ArchiveError::Failed("the stems are not files".to_owned())),
        }
        base += RENDER_UNITS;
        progress.store(base, Ordering::Relaxed);
    }
    let paths: Vec<PathBuf> = audio.iter().map(|(_, p)| p.clone()).collect();
    total.store(
        base + archive_work_with(project.len() as u64, &paths, &extras),
        Ordering::Relaxed,
    );
    write_archive_with(&dest, &name, (&project, &audio), &extras, cancel, progress)
}

/// Sends the result, and the completion of the worker, even if the thread panics.
struct Guard {
    tx: Sender<Msg>,
    job: u64,
    sent: bool,
}

impl Guard {
    fn send(&mut self, result: Result<Done, ArchiveError>) {
        self.sent = true;
        let _ = self.tx.send(Msg::Archived {
            job: self.job,
            result: Box::new(result),
        });
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.sent {
            self.send(Err(ArchiveError::Corrupt(
                "the archive worker stopped unexpectedly".into(),
            )));
        }
        let _ = self.tx.send(Msg::LoadDone);
    }
}

/// A test seam: the archive job the calling thread starts next waits for a release before it
/// works, so a test can press CANCEL while the worker is certainly still at its start.
#[cfg(test)]
pub(crate) mod hold {
    use std::cell::RefCell;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::Duration;

    thread_local! {
        static NEXT: RefCell<Option<Receiver<()>>> = const { RefCell::new(None) };
    }

    /// Holds the next archive job started on this thread; send (or drop the sender) to let it run.
    pub(crate) fn next_job() -> Sender<()> {
        let (tx, rx) = channel();
        NEXT.with(|n| *n.borrow_mut() = Some(rx));
        tx
    }

    pub(super) fn take() -> Option<Receiver<()>> {
        NEXT.with(|n| n.borrow_mut().take())
    }

    pub(super) fn wait(hold: Option<Receiver<()>>) {
        if let Some(rx) = hold {
            let _ = rx.recv_timeout(Duration::from_secs(60));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(dir: &std::path::Path, name: &str) -> ShareJob {
        ShareJob {
            dest: dir.join("song.atomikz"),
            name: name.to_owned(),
            project: b"{}".to_vec(),
            audio: Vec::new(),
            credits: "Title: x\n".to_owned(),
            temp: dir.join("work"),
            preview: None,
            stems: None,
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("atomik-share-worker-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_renders_folder_is_removed_when_the_share_ends_well_or_badly() {
        let dir = scratch("temp");
        let flags = (AtomicBool::new(false), AtomicU64::new(0), AtomicU64::new(0));
        let report = run_share(job(&dir, "song"), &flags.0, &flags.1, &flags.2).unwrap();
        assert_eq!(report.extra_files, 1, "the credits");
        assert!(dir.join("song.atomikz").is_file());
        assert!(!dir.join("work").exists(), "removed after a good share");
        std::fs::remove_file(dir.join("song.atomikz")).unwrap();
        // A name the archive refuses: no archive, no leftover folder.
        let bad = run_share(job(&dir, "a/b"), &flags.0, &flags.1, &flags.2);
        assert!(matches!(bad, Err(ArchiveError::Unsafe(_))));
        assert!(!dir.join("song.atomikz").exists() && !dir.join("work").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
