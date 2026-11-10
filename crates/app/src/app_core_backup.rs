//! Backup copies of the saved project, and File > Restore from Backup.
//!
//! With a **backup folder** set (Settings > Backup) every successful save also keeps a copy of
//! the project in that folder, for example on another drive: `<folder>/<name>/<name> <date and
//! time>.atomik`, and with **Include audio** the audio files it uses in `<folder>/<name>/Audio/`
//! (see `atomik_audio_file::backup` for the layout, the de-duplication and the pruning to the
//! newest **Keep** copies).
//!
//! The copy runs on its own worker ([`BACKUP_THREAD`]) after the save is confirmed, so it never
//! delays a save and never fails one. A failure (the folder is gone, the disk is full) is said in
//! a toast once per session, `Backup failed: <reason>`, and every later failure only goes to the
//! log. A save that ends while a copy is still running queues the next copy behind it, so two
//! workers never write into the same folder; only the newest queued copy is kept. Autosave
//! snapshots are recovery files and never make a backup.
//!
//! [`AppCore::start_backup_list`] lists the copies of the open project on a worker as well (the
//! folder may be a drive that is slow to answer), and [`AppCore::take_backup_list`] hands the list
//! over once it is ready. [`AppCore::restore_backup`] opens one copy as a new unsaved song, like a
//! template: the copy is never the song's file, so it cannot be overwritten, and its audio comes
//! from the copy's own `Audio/` folder.

use super::*;
use atomik_audio_file::{
    BackupEntry, BackupPlan, Bundle, backup_dir, backup_project, list_backups,
};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread::JoinHandle;

/// The name of the thread a backup runs on.
pub const BACKUP_THREAD: &str = "atomik-backup";

/// The settings of the backup copies (Settings > Backup).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BackupSettings {
    /// The folder the copies go into (`None`: no backups).
    pub folder: Option<PathBuf>,
    /// How many copies of a project stay.
    pub keep: u32,
    /// Whether the audio the project uses is copied too.
    pub audio: bool,
}

impl Default for BackupSettings {
    fn default() -> Self {
        Self {
            folder: None,
            keep: crate::keymap_store::DEFAULT_BACKUP_KEEP,
            audio: true,
        }
    }
}

/// One copy to make: everything is owned, nothing reaches back into the session.
struct Request {
    file: PathBuf,
    bundle: Option<Bundle>,
    settings: BackupSettings,
    now: u64,
}

/// A backup a worker is running.
struct Running {
    rx: Receiver<Result<(), String>>,
    worker: Option<JoinHandle<()>>,
}

/// The state of the backups of one song.
#[derive(Default)]
pub(super) struct BackupState {
    settings: BackupSettings,
    running: Option<Running>,
    /// The newest copy that waits for the running one.
    queued: Option<Request>,
    /// The toast of a failure was shown.
    failed_shown: bool,
    /// The list of copies a worker is making.
    listing: Option<Receiver<Result<Vec<BackupEntry>, String>>>,
}

/// One copy in the restore list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupRow {
    /// The copy's file.
    pub path: PathBuf,
    /// When it was made, `2026-11-10 09:31:12` (UTC).
    pub when: String,
    /// Its size for a person: `12.4 KB`.
    pub size: String,
}

/// `2026-11-10 093112` as `2026-11-10 09:31:12`.
fn readable_stamp(stamp: &str) -> String {
    match (
        stamp.get(..11),
        stamp.get(11..13),
        stamp.get(13..15),
        stamp.get(15..17),
    ) {
        (Some(day), Some(h), Some(m), Some(s)) => format!("{day}{h}:{m}:{s}"),
        _ => stamp.to_owned(),
    }
}

/// What a worker does for `request`: the copy, with a stamp no other copy has.
fn run(request: Request) -> Result<(), String> {
    let folder = request
        .settings
        .folder
        .as_deref()
        .ok_or_else(|| "no backup folder".to_owned())?;
    let name = request
        .file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or_else(|| "the project has no name".to_owned())?;
    let text = atomik_model::read_document_text(&request.file)
        .map_err(|e| format!("cannot read the saved project: {e}"))?;
    // Two saves in one second keep two copies: the stamp moves on until it is free.
    let dir = backup_dir(folder, &name);
    let mut secs = request.now;
    let stamp = loop {
        let stamp = crate::logging::backup_stamp(secs);
        if !dir.join(format!("{name} {stamp}.atomik")).exists() {
            break stamp;
        }
        secs += 1;
    };
    let plan = BackupPlan {
        folder,
        name: &name,
        stamp: &stamp,
        keep: request.settings.keep as usize,
        include_audio: request.settings.audio,
    };
    let report = backup_project(&plan, &request.file, &text, request.bundle.as_ref())
        .map_err(|e| format!("{}: {e}", folder.display()))?;
    log::info!(
        "backup: {} ({} audio copied, {} already there, {} missing; {} old copies and {} audio files removed)",
        report.copy.display(),
        report.audio_copied,
        report.audio_present,
        report.audio_missing,
        report.pruned,
        report.audio_removed,
    );
    if let Some(why) = report.unreadable {
        log::warn!("backup: no audio was removed because a file cannot be read: {why}");
    }
    Ok(())
}

impl<S: EngineSink> AppCore<S> {
    /// Sets the backup folder, how many copies stay and whether the audio comes along (Settings >
    /// Backup). A count is kept between 1 and 50.
    pub fn set_backup(&mut self, folder: Option<PathBuf>, keep: u32, audio: bool) {
        self.backup.settings = BackupSettings {
            folder: folder.filter(|f| !f.as_os_str().is_empty()),
            keep: keep.clamp(1, crate::keymap_store::MAX_BACKUP_KEEP),
            audio,
        };
    }

    /// The backup folder, when one is set.
    pub fn backup_folder(&self) -> Option<&Path> {
        self.backup.settings.folder.as_deref()
    }

    /// How many copies of a project stay.
    pub fn backup_keep(&self) -> u32 {
        self.backup.settings.keep
    }

    /// Whether a copy brings the audio.
    pub fn backup_audio(&self) -> bool {
        self.backup.settings.audio
    }

    /// True while a backup runs or waits for its turn.
    pub fn backup_running(&self) -> bool {
        self.backup.running.is_some() || self.backup.queued.is_some()
    }

    /// A save of `file` just ended: copies it into the backup folder on a worker (module docs).
    pub(super) fn backup_after_save(&mut self, file: &Path) {
        let Some(folder) = self.backup.settings.folder.clone() else {
            return;
        };
        // The factory folder is read-only and never saved into; a patch is not a project.
        if !super::is_project_path(file) || self.in_factory(file) {
            return;
        }
        let request = Request {
            file: file.to_path_buf(),
            bundle: self.bundle_clone(),
            settings: BackupSettings {
                folder: Some(folder),
                ..self.backup.settings.clone()
            },
            now: crate::logging::unix_seconds(),
        };
        if self.backup.running.is_some() {
            self.backup.queued = Some(request);
        } else {
            self.launch_backup(request);
        }
    }

    fn launch_backup(&mut self, request: Request) {
        let (tx, rx) = channel();
        let started = std::thread::Builder::new()
            .name(BACKUP_THREAD.to_owned())
            .spawn(move || {
                let _ = tx.send(run(request));
            });
        match started {
            Ok(worker) => {
                self.backup.running = Some(Running {
                    rx,
                    worker: Some(worker),
                });
            }
            Err(e) => self.backup_failed(&format!("cannot start the backup: {e}")),
        }
    }

    /// A backup failed: the toast shows the first time, the log every time.
    fn backup_failed(&mut self, reason: &str) {
        log::warn!("backup failed: {reason}");
        if !self.backup.failed_shown {
            self.backup.failed_shown = true;
            self.reject(format!("Backup failed: {reason}"));
        }
    }

    /// Takes the result of the backup that finished (with `wait`, blocks for it) and starts the
    /// queued one. True when one ended.
    fn collect_backup(&mut self, wait: bool) -> bool {
        let Some(running) = &self.backup.running else {
            return false;
        };
        let result = if wait {
            running.rx.recv().map_err(|_| ())
        } else {
            match running.rx.try_recv() {
                Ok(result) => Ok(result),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => Err(()),
            }
        };
        if let Some(mut running) = self.backup.running.take()
            && let Some(worker) = running.worker.take()
        {
            let _ = worker.join();
        }
        match result {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => self.backup_failed(&reason),
            Err(()) => self.backup_failed("the backup stopped unexpectedly"),
        }
        if let Some(next) = self.backup.queued.take() {
            self.launch_backup(next);
        }
        true
    }

    /// Applies a backup that finished meanwhile (called every frame).
    pub(super) fn poll_backup(&mut self) -> bool {
        self.collect_backup(false)
    }

    /// Waits for the running and queued backups (tests, scripts).
    pub fn settle_backup(&mut self) {
        while self.backup.running.is_some() {
            self.collect_backup(true);
        }
    }

    // ------------------------------------------------------------------ the restore list

    /// File > Restore from Backup: starts listing the copies of the open project on a worker;
    /// [`AppCore::take_backup_list`] has the answer. Says why when there is nothing to list.
    ///
    /// # Errors
    /// A message for the toast: no backup folder is set, or the song has no project file yet.
    pub fn start_backup_list(&mut self) -> Result<(), String> {
        let Some(folder) = self.backup.settings.folder.clone() else {
            return Err("No backup folder is set: choose one in Settings > Backup".to_owned());
        };
        let Some(file) = self.file.clone().filter(|p| super::is_project_path(p)) else {
            return Err("Save the song first: backups are kept for saved projects".to_owned());
        };
        let name = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name(BACKUP_THREAD.to_owned())
            .spawn(move || {
                let _ = tx.send(
                    list_backups(&folder, &name).map_err(|e| format!("{}: {e}", folder.display())),
                );
            })
            .map_err(|e| format!("cannot look for backups: {e}"))?;
        self.backup.listing = Some(rx);
        Ok(())
    }

    /// The list [`AppCore::start_backup_list`] started, once it is ready (`None` while the
    /// worker still looks).
    pub fn take_backup_list(&mut self) -> Option<Result<Vec<BackupRow>, String>> {
        let rx = self.backup.listing.as_ref()?;
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("the search stopped unexpectedly".to_owned()),
        };
        self.backup.listing = None;
        Some(result.map(|entries| {
            entries
                .into_iter()
                .map(|e| BackupRow {
                    when: readable_stamp(&e.stamp),
                    size: super::archive::size_text(e.size),
                    path: e.path,
                })
                .collect()
        }))
    }

    /// True while the list of copies is being made.
    pub fn backup_listing(&self) -> bool {
        self.backup.listing.is_some()
    }

    /// Opens the copy `path` as a new unsaved song (module docs). The caller has asked about
    /// unsaved changes. Only a copy of the open project in the backup folder is opened; on error
    /// nothing changes.
    ///
    /// # Errors
    /// The file is not a copy of this project's, cannot be read, or is not a valid project.
    pub fn restore_backup(&mut self, path: &Path) -> Result<(), AppError> {
        let not_a_copy = || {
            AppError::Model(atomik_model::ModelError::BadScope(
                "that is not a backup copy of this project".to_owned(),
            ))
        };
        let folder = self.backup.settings.folder.clone().ok_or_else(not_a_copy)?;
        let name = self
            .file
            .as_deref()
            .filter(|p| super::is_project_path(p))
            .and_then(Path::file_stem)
            .map(|s| s.to_string_lossy().into_owned())
            .ok_or_else(not_a_copy)?;
        let listed =
            list_backups(&folder, &name).is_ok_and(|all| all.iter().any(|e| e.path == path));
        if !listed {
            return Err(not_a_copy());
        }
        self.settle_save();
        self.open_template(path)?;
        self.reject("Backup copy opened as a new song; Save asks where to keep it".to_owned());
        Ok(())
    }
}

#[cfg(test)]
#[path = "app_core_backup_tests.rs"]
mod tests;
