//! The project archive worker: packs a project and its audio into one `.atomikz` file, or unpacks
//! one, on its own thread (see [`crate::AppCore::start_archive_export`]).
//!
//! The format and its safety checks belong to the `atomik-audio-file` crate; this thread only runs
//! them, shares the progress and the cancel flag with the UI thread and reports through
//! [`Msg::Archived`] followed by the usual completion message, even when the thread panics.

use crate::audio_import::Msg;
use atomik_audio_file::{
    ArchiveError, ArchiveLimits, ArchiveReport, Extracted, archive_size, extract_archive,
    write_archive,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::mpsc::Sender;

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
    /// Unpack: extract `archive` into the new folder `dest`.
    Unpack {
        /// The archive to read.
        archive: PathBuf,
        /// The folder to make.
        dest: PathBuf,
    },
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
    std::thread::Builder::new()
        .name("atomik-archive".into())
        .spawn(move || {
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
