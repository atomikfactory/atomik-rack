//! Saving a project without freezing the window.
//!
//! A save has a part that must see the live session and a part that does not:
//!
//! * **On the UI thread** ([`AppCore::prepare_save`]): the plugins' states are captured (one
//!   request to every sandboxed plugin, then the answers, see [`AppCore::plugins_capture_all`])
//!   and put into the document; the document is cloned (the song, the note lists and the
//!   plugin states are shared, so the clone copies no bulk data) and pruned to what the song
//!   uses; the destination bundle is planned from the bundle the session holds.
//! * **On a worker** ([`Job::run`]): the project text is built and validated, the destination
//!   bundle is made (Save As copies the audio), the file is written atomically and the audio no
//!   one uses any more is deleted.
//!
//! [`AppCore::save_in_background`] and [`AppCore::save_as_in_background`] (File > Save and the
//! file picked for Save As) run the worker beside the UI: edits stay possible meanwhile, the
//! status line says "Saving...", and the file name, the bundle and the saved baseline switch to
//! the saved state when the worker reports in ([`AppCore::poll_background`]). A change made
//! during the save is therefore still unsaved afterwards, and undoing back to the saved state
//! is clean. A save that fails is reported and nothing switches; the file it was replacing is
//! intact (atomic write). [`AppCore::save`] and [`AppCore::save_as`] run the same steps and wait
//! (scripts, tests, "Save" in the question about unsaved changes, quitting).
//!
//! Things that would touch the project bundle meanwhile wait for the save first: an import or a
//! recording asks for the bundle ([`AppCore::settle_save`]), and so does opening another
//! project. A save that starts while audio is being loaded, recorded, converted or frozen is
//! not run in the background (it would delete files the worker is about to register).

use super::{AppCore, AppError, ProjectBundle, io_error};
use crate::sink::EngineSink;
use atomik_audio_file::{Bundle, save_text_atomic};
use atomik_model::RackDoc;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread::JoinHandle;

/// How the destination bundle is made.
enum BundlePlan {
    /// The project has no audio (and no bundle to tidy).
    None,
    /// The bundle the session holds is the destination; only its orphans go.
    Existing(Bundle),
    /// Save As: the audio of `from` is copied into a new bundle at `to`.
    Copy { from: PathBuf, to: PathBuf },
    /// The first save of a project whose audio is already in `to`'s place: made empty.
    Create(PathBuf),
}

/// What the worker needs: everything is owned, nothing reaches back into the session.
pub(super) struct Job {
    file: PathBuf,
    snapshot: RackDoc,
    keep: BTreeSet<String>,
    plan: BundlePlan,
}

impl Job {
    /// Writes the project and tidies the bundle. Returns the bundle the project now lives in.
    pub(super) fn run(self) -> Result<Option<Bundle>, AppError> {
        let text = atomik_model::save_project_checked(&self.snapshot)?;
        let mut dest = match &self.plan {
            BundlePlan::None => None,
            BundlePlan::Existing(bundle) => Some(bundle.clone()),
            BundlePlan::Copy { from, to } => {
                Some(Bundle::copy_bundle(from, to).map_err(|e| io_error(to, e))?)
            }
            BundlePlan::Create(root) => Some(Bundle::create(root).map_err(|e| io_error(root, e))?),
        };
        save_text_atomic(&self.file, &text).map_err(|e| io_error(&self.file, e))?;
        if let Some(bundle) = &mut dest {
            bundle
                .remove_orphans(&self.keep)
                .map_err(|e| io_error(bundle.root(), e))?;
        }
        Ok(dest)
    }
}

/// A save a worker is running.
pub(super) struct Flight {
    file: PathBuf,
    snapshot: RackDoc,
    note_recent: bool,
    rx: Receiver<Result<Option<Bundle>, AppError>>,
    worker: Option<JoinHandle<()>>,
}

impl<S: EngineSink> AppCore<S> {
    /// The file and bundle root File > Save writes.
    pub(super) fn save_target(&self) -> Result<(PathBuf, PathBuf), AppError> {
        let file = self
            .file
            .clone()
            .filter(|p| super::super::is_project_path(p))
            .ok_or(AppError::NoPath)?;
        let root =
            super::existing_bundle_root(&file).unwrap_or_else(|| super::sibling_bundle_root(&file));
        Ok((file, root))
    }

    /// The file and bundle root (none without audio) Save As to `path` writes.
    pub(super) fn save_as_target(&self, path: &Path) -> (PathBuf, Option<PathBuf>) {
        if self.document.referenced_assets().is_empty() {
            (path.to_path_buf(), None)
        } else {
            let (file, root) = super::save_as_layout(path);
            (file, Some(root))
        }
    }

    /// [`AppCore::save`]'s steps with the worker's part on the caller's thread.
    pub(super) fn save_now(&mut self, file: &Path, root: Option<PathBuf>) -> Result<(), AppError> {
        self.settle_save();
        let job = self.prepare_save(file, root);
        let snapshot = job.snapshot.clone();
        let dest = job.run()?;
        self.finish_save(file, snapshot, dest);
        Ok(())
    }

    /// Makes [`AppCore::save_interactive`] and [`AppCore::save_as_interactive`] write on a
    /// worker (the window); off (the default: scripts, tests, the QA server) they block until
    /// the file is written.
    pub fn set_background_save(&mut self, on: bool) {
        self.audio.background_save = on;
    }

    /// File > Save as the user asked for it: in the background when
    /// [`AppCore::set_background_save`] is on, else like [`AppCore::save`].
    ///
    /// # Errors
    /// [`AppError::NoPath`] without a project file; what a blocking save fails with.
    pub fn save_interactive(&mut self) -> Result<(), AppError> {
        if self.audio.background_save {
            self.save_in_background()
        } else {
            self.save()
        }
    }

    /// Save As to the file the user picked: in the background when
    /// [`AppCore::set_background_save`] is on, else like [`AppCore::save_as`].
    ///
    /// # Errors
    /// What a blocking save fails with.
    pub fn save_as_interactive(&mut self, path: &Path) -> Result<(), AppError> {
        if self.audio.background_save {
            self.save_as_in_background(path)
        } else {
            self.save_as(path)
        }
    }

    /// Saves to the current file like [`AppCore::save`], but the writing runs on a worker (see
    /// the module docs). Falls back to the blocking save while audio work is running.
    ///
    /// # Errors
    /// [`AppError::NoPath`] without a project file; what a blocking save fails with.
    pub fn save_in_background(&mut self) -> Result<(), AppError> {
        let (file, root) = self.save_target()?;
        self.save_later(&file, Some(root), false)
    }

    /// [`AppCore::save_as`] with the writing on a worker (see the module docs).
    ///
    /// # Errors
    /// What a blocking save fails with.
    pub fn save_as_in_background(&mut self, path: &Path) -> Result<(), AppError> {
        let (file, root) = self.save_as_target(path);
        self.save_later(&file, root, true)
    }

    fn save_later(
        &mut self,
        file: &Path,
        root: Option<PathBuf>,
        note_recent: bool,
    ) -> Result<(), AppError> {
        self.settle_save();
        if self.audio_work_running() {
            self.save_now(file, root)?;
            if note_recent {
                self.note_recent();
            }
            return Ok(());
        }
        let job = self.prepare_save(file, root);
        let snapshot = job.snapshot.clone();
        let (tx, rx) = channel();
        let worker = std::thread::Builder::new()
            .name("atomik-save".to_owned())
            .spawn(move || {
                let _ = tx.send(job.run());
            })
            .map_err(|e| AppError::Io {
                path: file.to_path_buf(),
                source: e,
            })?;
        self.audio.saving = Some(Flight {
            file: file.to_path_buf(),
            snapshot,
            note_recent,
            rx,
            worker: Some(worker),
        });
        Ok(())
    }

    /// Audio is being loaded, recorded, converted or frozen: files are about to appear in the
    /// bundle that a background save must not mistake for orphans.
    fn audio_work_running(&self) -> bool {
        self.audio.pending > 0
            || self.audio.rec_pending > 0
            || self.audio.recording.is_some()
            || self.audio.converting.is_some()
            || self.freeze_busy()
    }

    /// The UI-thread part of a save (see the module docs).
    fn prepare_save(&mut self, file: &Path, root: Option<PathBuf>) -> Job {
        // The plugins' own state goes into the document first.
        self.plugins_capture_all();
        let keep = self.document.referenced_assets();
        let mut snapshot = self.doc().clone();
        snapshot.prune_unreferenced_assets();
        let plan = match (&root, keep.is_empty(), &self.audio.bundle) {
            (Some(root), false, Some(b)) if b.bundle.root() == root.as_path() => {
                BundlePlan::Existing(b.bundle.clone())
            }
            (Some(root), false, Some(b)) => BundlePlan::Copy {
                from: b.bundle.root().to_path_buf(),
                to: root.clone(),
            },
            (Some(root), false, None) => BundlePlan::Create(root.clone()),
            // The last audio was removed: its files still go.
            (Some(root), true, Some(b)) if !b.scratch && b.bundle.root() == root.as_path() => {
                BundlePlan::Existing(b.bundle.clone())
            }
            _ => BundlePlan::None,
        };
        Job {
            file: file.to_path_buf(),
            snapshot,
            keep,
            plan,
        }
    }

    /// The project is on disk: switches to its bundle and file, makes the saved state the
    /// baseline and tidies the live state.
    fn finish_save(&mut self, file: &Path, snapshot: RackDoc, dest: Option<Bundle>) {
        let was_scratch = self.audio.bundle.as_ref().is_some_and(|b| b.scratch);
        self.audio.bundle = dest.map(|bundle| ProjectBundle {
            bundle,
            scratch: false,
        });
        if was_scratch && self.audio.pending == 0 {
            self.audio.remove_scratch();
        }
        let before: Vec<String> = self.doc().assets.keys().cloned().collect();
        self.document.prune_unreferenced_assets();
        for hash in before {
            if !self.doc().assets.contains_key(&hash) {
                self.forget_asset(&hash);
            }
        }
        self.file = Some(file.to_path_buf());
        self.set_saved(snapshot);
        // The recovered recordings are in the saved project now: their files can go.
        for path in std::mem::take(&mut self.audio.recovered_files) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Takes the result of the save the worker finished; with `wait` blocks for it. True when a
    /// save ended.
    fn collect_save(&mut self, wait: bool) -> bool {
        let Some(flight) = &self.audio.saving else {
            return false;
        };
        let result = if wait {
            flight.rx.recv().map_err(|_| ())
        } else {
            match flight.rx.try_recv() {
                Ok(result) => Ok(result),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => Err(()),
            }
        };
        let Some(mut flight) = self.audio.saving.take() else {
            return false;
        };
        if let Some(worker) = flight.worker.take() {
            let _ = worker.join();
        }
        match result {
            Ok(Ok(dest)) => {
                self.finish_save(&flight.file, flight.snapshot, dest);
                if flight.note_recent {
                    self.note_recent();
                }
            }
            Ok(Err(e)) => self.reject(e.to_string()),
            Err(()) => self.reject("saving the project failed unexpectedly".to_owned()),
        }
        true
    }

    /// Applies a save that finished meanwhile (called every frame).
    pub(super) fn poll_save(&mut self) -> bool {
        self.collect_save(false)
    }

    /// Waits for the running save, if any, and applies it. Anything that replaces the project
    /// or touches its bundle calls this first.
    pub fn settle_save(&mut self) {
        self.collect_save(true);
    }

    /// True while a save runs on its worker.
    pub fn saving(&self) -> bool {
        self.audio.saving.is_some()
    }

    /// The status-line text of a running save.
    pub fn save_progress(&self) -> Option<String> {
        self.saving().then(|| "Saving...".to_owned())
    }
}

#[cfg(test)]
#[path = "app_core_save_tests.rs"]
mod tests;
