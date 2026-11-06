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
use atomik_audio_file::{
    Bundle, CollectPlan, CollectProgress, CollectReport, RetireReport, plan_collect_into,
    retire_old_project, save_text_atomic,
};
use atomik_model::RackDoc;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread::JoinHandle;

/// The bundle folder Collect Files fills for the project file `file`: the folder the file sits
/// in when it is named like the file, else the folder `<name>/` beside it (made if needed), so a
/// project that shares an `Audio/` folder with others gets audio of its own.
fn collect_root(file: &Path) -> PathBuf {
    let folder = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match file.file_stem() {
        Some(stem) if folder.file_name() == Some(stem) => folder.to_path_buf(),
        _ => super::sibling_bundle_root(file),
    }
}

/// The folders near the project file `file` that can hold its audio: its bundle folder, its own
/// folder and the folder above it, each only when it has an `Audio/` directory.
fn audio_folders(file: &Path) -> Vec<PathBuf> {
    let folder = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut found = Vec::new();
    let candidates = [
        super::existing_bundle_root(file),
        Some(super::sibling_bundle_root(file)),
        Some(folder.to_path_buf()),
        folder.parent().map(Path::to_path_buf),
    ];
    for root in candidates.into_iter().flatten() {
        if root.join("Audio").is_dir() && !found.contains(&root) {
            found.push(root);
        }
    }
    found
}

/// How the destination bundle is made.
enum BundlePlan {
    /// The project has no audio (and no bundle to tidy).
    None,
    /// The bundle the session holds is the destination; only its orphans go.
    Existing(Bundle),
    /// Save As and Collect Files: the audio the project uses is copied from the bundle folder
    /// `from` (and the job's other sources) into the bundle at `to`.
    Copy { to: PathBuf },
    /// The first save of a project whose audio is already in `to`'s place: made empty.
    Create(PathBuf),
}

/// The old place of a project that is being moved (File > Move Project).
#[derive(Clone)]
pub(super) struct Relocate {
    /// The old project file.
    old_file: PathBuf,
    /// The old bundle folder, when the project had audio.
    old_root: Option<PathBuf>,
}

/// What a collection would do, for the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectPreview {
    /// The folder the audio would go into.
    pub folder: PathBuf,
    /// What would be copied, what is there, what is missing.
    pub plan: CollectPlan,
    /// The names of the files to copy (their original file names), in the order of the plan.
    pub copy_names: Vec<String>,
    /// The names of the files no folder holds.
    pub missing_names: Vec<String>,
}

/// What the worker needs: everything is owned, nothing reaches back into the session.
pub(super) struct Job {
    file: PathBuf,
    snapshot: RackDoc,
    keep: BTreeSet<String>,
    plan: BundlePlan,
    /// Bundle folders the audio the destination lacks is looked for in, in order.
    sources: Vec<PathBuf>,
    /// Collect Files: look for the audio the destination lacks even when it is the bundle the
    /// session holds.
    collect: bool,
    /// Move Project: the old place to remove once the new one is written and verified.
    relocate: Option<Relocate>,
    /// Cancels the copying from the UI thread and counts the files.
    progress: Arc<CollectProgress>,
}

/// What a finished save hands back.
pub(super) struct Saved {
    bundle: Option<Bundle>,
    report: CollectReport,
    /// What Move Project removed of the old place.
    moved: Option<RetireReport>,
    /// The copying was cancelled: nothing was written.
    cancelled: bool,
}

impl Job {
    /// Writes the project and tidies the bundle. Returns the bundle the project now lives in.
    pub(super) fn run(self) -> Result<Saved, AppError> {
        let text = atomik_model::save_project_checked(&self.snapshot)?;
        let mut report = CollectReport::default();
        let mut dest = match &self.plan {
            BundlePlan::None => None,
            BundlePlan::Existing(bundle) => Some(bundle.clone()),
            BundlePlan::Copy { to } => Some(Bundle::create(to).map_err(|e| io_error(to, e))?),
            BundlePlan::Create(root) => Some(Bundle::create(root).map_err(|e| io_error(root, e))?),
        };
        if let Some(bundle) = &dest
            && (self.collect || matches!(self.plan, BundlePlan::Copy { .. }))
        {
            report = bundle
                .collect_from_with(&self.sources, &self.keep, &self.progress)
                .map_err(|e| io_error(bundle.root(), e))?;
            if report.cancelled {
                // Nothing is written: the project and its old place stay as they were.
                return Ok(Saved {
                    bundle: None,
                    report,
                    moved: None,
                    cancelled: true,
                });
            }
        }
        save_text_atomic(&self.file, &text).map_err(|e| io_error(&self.file, e))?;
        // Move Project: the old place goes only when the new one is proved to hold everything.
        let moved = match &self.relocate {
            Some(old) => Some(
                retire_old_project(
                    &old.old_file,
                    old.old_root.as_deref(),
                    &self.file,
                    &text,
                    dest.as_ref().map(Bundle::root),
                    &self.keep,
                )
                .map_err(AppError::Audio)?,
            ),
            None => None,
        };
        // Only a bundle folder that belongs to this project alone is tidied: where other project
        // files share the `Audio/` folder, what this one does not use may be theirs.
        if let Some(bundle) = &mut dest
            && !bundle.shared_with_other_projects(&self.file)
        {
            bundle
                .remove_orphans(&self.keep)
                .map_err(|e| io_error(bundle.root(), e))?;
        }
        Ok(Saved {
            bundle: dest,
            report,
            moved,
            cancelled: false,
        })
    }
}

/// A save a worker is running.
pub(super) struct Flight {
    file: PathBuf,
    snapshot: RackDoc,
    note_recent: bool,
    collect: bool,
    /// The old project file of a Move Project.
    relocate: Option<PathBuf>,
    progress: Arc<CollectProgress>,
    rx: Receiver<Result<Saved, AppError>>,
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
        self.save_now_with(file, root, false, None)
    }

    fn save_now_with(
        &mut self,
        file: &Path,
        root: Option<PathBuf>,
        collect: bool,
        relocate: Option<Relocate>,
    ) -> Result<(), AppError> {
        self.settle_save();
        let mut job = self.prepare_save(file, root, collect);
        job.relocate = relocate.clone();
        let snapshot = job.snapshot.clone();
        let saved = job.run()?;
        self.finish_save(file, snapshot, saved.bundle);
        self.after_collect(collect, &saved.report);
        if let (Some(old), Some(moved)) = (relocate, &saved.moved) {
            self.after_move(&old.old_file, moved);
        }
        Ok(())
    }

    /// File > Collect Files: the audio the project uses is copied into the project's own
    /// bundle folder (see the module docs of `app_core_audio.rs`), the project is saved, and the
    /// toast says what was copied and what is still missing.
    ///
    /// # Errors
    /// [`AppError::NoPath`] without a project file; what a save fails with.
    pub fn collect_files(&mut self) -> Result<(), AppError> {
        let file = self
            .file
            .clone()
            .filter(|p| super::super::is_project_path(p))
            .ok_or(AppError::NoPath)?;
        if self.document.referenced_assets().is_empty() {
            self.reject("The project uses no audio files".to_owned());
            return Ok(());
        }
        let root = collect_root(&file);
        if self.audio.background_save {
            self.save_later(&file, Some(root), false, true, None)
        } else {
            self.save_now_with(&file, Some(root), true, None)
        }
    }

    /// The bundle folders the audio a save lacks is looked for in: the bundle the session holds,
    /// then the folders a moved or shared project keeps its audio in.
    fn collect_sources(&self) -> Vec<PathBuf> {
        let mut sources: Vec<PathBuf> = Vec::new();
        if let Some(b) = &self.audio.bundle {
            sources.push(b.bundle.root().to_path_buf());
        }
        if let Some(old) = &self.file {
            for root in audio_folders(old) {
                if !sources.contains(&root) {
                    sources.push(root);
                }
            }
        }
        sources
    }

    /// What File > Collect Files would do now, without doing it: the files it would copy (with
    /// their size), the ones already in the project folder and the ones no folder has.
    ///
    /// # Errors
    /// [`AppError::NoPath`] without a project file.
    pub fn collect_preview(&self) -> Result<CollectPreview, AppError> {
        let file = self
            .file
            .clone()
            .filter(|p| super::super::is_project_path(p))
            .ok_or(AppError::NoPath)?;
        let folder = collect_root(&file);
        let wanted = self.document.referenced_assets();
        let plan = plan_collect_into(&folder, &self.collect_sources(), &wanted);
        let copy_names = plan
            .to_copy
            .iter()
            .map(|(hash, _)| self.asset_name(hash))
            .collect();
        let missing_names = plan.missing.iter().map(|h| self.asset_name(h)).collect();
        Ok(CollectPreview {
            folder,
            plan,
            copy_names,
            missing_names,
        })
    }

    /// The text of the preview of Collect Files.
    pub fn collect_preview_message(&self) -> Result<String, AppError> {
        let preview = self.collect_preview()?;
        if self.document.referenced_assets().is_empty() {
            return Ok("The project uses no audio files".to_owned());
        }
        let folder = preview.folder.file_name().map_or_else(
            || "the project folder".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let plan = &preview.plan;
        let count = |n: usize| format!("{n} audio file{}", if n == 1 { "" } else { "s" });
        let mut text = if plan.to_copy.is_empty() {
            format!("Collect Files would copy nothing: all of the audio is in {folder} already")
        } else {
            let shown = preview
                .copy_names
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let more = plan.to_copy.len().saturating_sub(3);
            let tail = if more > 0 {
                format!(": {shown} and {more} more")
            } else {
                format!(": {shown}")
            };
            format!(
                "Collect Files would copy {} ({}) into {folder}{tail}",
                count(plan.to_copy.len()),
                super::super::archive::size_text(plan.bytes()),
            )
        };
        if !plan.to_copy.is_empty() && !plan.present.is_empty() {
            text.push_str(&format!("; {} already there", plan.present.len()));
        }
        if !plan.missing.is_empty() {
            let shown = preview
                .missing_names
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let more = plan.missing.len().saturating_sub(3);
            text.push_str(&format!("; {} not found: {shown}", plan.missing.len()));
            if more > 0 {
                text.push_str(&format!(" and {more} more"));
            }
        }
        Ok(text)
    }

    /// File > Move Project: the project goes to a new folder `<folder>/<name>/` (with the audio it
    /// uses) as Save As does it, and the old project is removed, but only after the copy has
    /// been checked against the original byte for byte, and never audio that another project in
    /// the old folder may use (see `atomik_audio_file::retire_old_project`). Edits and the
    /// project file name switch to the new place when the worker reports in.
    ///
    /// # Errors
    /// [`AppError::NoPath`] without a project file; a refusal (the destination already holds a
    /// project of that name) is a message and `Ok`; what a save fails with.
    pub fn move_project(&mut self, folder: &Path) -> Result<(), AppError> {
        let old = self
            .file
            .clone()
            .filter(|p| super::super::is_project_path(p))
            .ok_or(AppError::NoPath)?;
        let Some(stem) = old.file_stem() else {
            return Err(AppError::NoPath);
        };
        let picked = folder.join(format!(
            "{}.{}",
            stem.to_string_lossy(),
            super::super::PROJECT_EXTENSION
        ));
        let (file, root) = self.save_as_target(&picked);
        let taken = file.exists() || root.as_ref().is_some_and(|r| r.exists() && *r != folder);
        if taken {
            self.reject(format!(
                "{} already exists in that folder: nothing was moved",
                stem.to_string_lossy()
            ));
            return Ok(());
        }
        let relocate = Relocate {
            old_file: old,
            old_root: self
                .audio
                .bundle
                .as_ref()
                .filter(|b| !b.scratch)
                .map(|b| b.bundle.root().to_path_buf()),
        };
        if self.audio.background_save {
            self.save_later(&file, root, true, false, Some(relocate))
        } else {
            self.save_now_with(&file, root, false, Some(relocate))?;
            self.note_recent();
            Ok(())
        }
    }

    /// Asks the save running on its worker to stop copying audio (Collect Files, Move Project, a
    /// Save As that copies audio). Nothing is written then and the project stays as it was;
    /// the audio already copied stays too. False when no save runs.
    pub fn cancel_collect(&mut self) -> bool {
        match &self.audio.saving {
            Some(flight) => {
                flight.progress.cancel();
                true
            }
            None => false,
        }
    }

    /// A save runs that copies audio: the Cancel item is enabled.
    pub fn collect_busy(&self) -> bool {
        self.audio
            .saving
            .as_ref()
            .is_some_and(|f| f.collect || f.relocate.is_some())
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
        self.save_later(&file, Some(root), false, false, None)
    }

    /// [`AppCore::save_as`] with the writing on a worker (see the module docs).
    ///
    /// # Errors
    /// What a blocking save fails with.
    pub fn save_as_in_background(&mut self, path: &Path) -> Result<(), AppError> {
        let (file, root) = self.save_as_target(path);
        self.save_later(&file, root, true, false, None)
    }

    fn save_later(
        &mut self,
        file: &Path,
        root: Option<PathBuf>,
        note_recent: bool,
        collect: bool,
        relocate: Option<Relocate>,
    ) -> Result<(), AppError> {
        self.settle_save();
        if self.audio_work_running() {
            self.save_now_with(file, root, collect, relocate)?;
            if note_recent {
                self.note_recent();
            }
            return Ok(());
        }
        let mut job = self.prepare_save(file, root, collect);
        job.relocate = relocate;
        self.launch(job, note_recent, collect)
    }

    /// Runs `job` on its worker; the result is applied by [`AppCore::poll_save`].
    fn launch(&mut self, job: Job, note_recent: bool, collect: bool) -> Result<(), AppError> {
        let file = job.file.clone();
        let relocate = job.relocate.as_ref().map(|r| r.old_file.clone());
        let progress = job.progress.clone();
        let snapshot = job.snapshot.clone();
        let (tx, rx) = channel();
        let worker = std::thread::Builder::new()
            .name("atomik-save".to_owned())
            .spawn(move || {
                let _ = tx.send(job.run());
            })
            .map_err(|e| AppError::Io {
                path: file.clone(),
                source: e,
            })?;
        self.audio.saving = Some(Flight {
            file,
            snapshot,
            note_recent,
            collect,
            relocate,
            progress,
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
    fn prepare_save(&mut self, file: &Path, root: Option<PathBuf>, collect: bool) -> Job {
        // The plugins' own state goes into the document first.
        self.plugins_capture_all();
        let keep = self.document.referenced_assets();
        let mut snapshot = self.doc().clone();
        snapshot.prune_unreferenced_assets();
        let plan = match (&root, keep.is_empty(), &self.audio.bundle) {
            (Some(root), false, Some(b)) if b.bundle.root() == root.as_path() => {
                BundlePlan::Existing(b.bundle.clone())
            }
            (Some(root), false, Some(_)) => BundlePlan::Copy { to: root.clone() },
            (Some(root), false, None) => BundlePlan::Create(root.clone()),
            // The last audio was removed: its files still go.
            (Some(root), true, Some(b)) if !b.scratch && b.bundle.root() == root.as_path() => {
                BundlePlan::Existing(b.bundle.clone())
            }
            _ => BundlePlan::None,
        };
        let sources = self.collect_sources();
        Job {
            file: file.to_path_buf(),
            snapshot,
            keep,
            plan,
            sources,
            collect,
            relocate: None,
            progress: Arc::new(CollectProgress::new()),
        }
    }

    /// A save that collected: loads the audio it brought back and says what it did.
    fn after_collect(&mut self, collect: bool, report: &CollectReport) {
        let back: Vec<String> = self
            .audio
            .missing
            .iter()
            .filter(|m| report.copied.contains(&m.hash))
            .map(|m| m.hash.clone())
            .collect();
        if !back.is_empty()
            && let Some(bundle) = self.audio.bundle.as_ref().map(|b| b.bundle.clone())
        {
            let threshold = self.stream_threshold();
            let started = crate::audio_import::spawn_load(
                self.audio.tx.clone(),
                self.audio.generation,
                bundle,
                threshold,
                back,
            );
            match started {
                Ok(()) => self.audio.pending += 1,
                Err(e) => self.reject(format!("audio: cannot start the loader: {e}")),
            }
        }
        if collect {
            let message = self.collect_message(report);
            self.reject(message);
        }
    }

    /// A save that moved the project: forgets the old file in the recent list and says what was
    /// removed of the old place.
    fn after_move(&mut self, old_file: &Path, moved: &RetireReport) {
        let folder = self
            .audio
            .bundle
            .as_ref()
            .map(|b| b.bundle.root().to_path_buf())
            .or_else(|| {
                self.file
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
            })
            .map_or_else(
                || "the new folder".to_owned(),
                |f| f.to_string_lossy().into_owned(),
            );
        let name = old_file
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let message = if let Some(why) = moved.refused {
            format!("Saved a copy in {folder}; the old project {name} was left: {why}")
        } else {
            self.forget_recent(old_file);
            if moved.shared {
                format!(
                    "Moved to {folder}. The old project file {name} was removed; its audio folder stays because another project is in it"
                )
            } else if moved.folder_removed {
                format!("Moved to {folder}. The old project folder was removed")
            } else {
                format!(
                    "Moved to {folder}. The old project {name} and its audio were removed; other files in the old folder stay"
                )
            }
        };
        self.reject(message);
    }

    /// The toast of a collection that was cancelled.
    fn cancel_message(&self, flight: &Flight, report: &CollectReport) -> String {
        let (done, total) = flight.progress.files();
        let what = if flight.relocate.is_some() {
            "Move Project"
        } else {
            "Collect Files"
        };
        let copied = report.copied.len();
        let kept = if copied == 0 {
            String::new()
        } else {
            format!(
                "; the {copied} file{} copied stay{}",
                if copied == 1 { "" } else { "s" },
                if copied == 1 { "s" } else { "" }
            )
        };
        format!(
            "{what} cancelled after {done} of {total} audio files{kept}; nothing was removed and the project was not changed"
        )
    }

    /// The toast of Collect Files.
    fn collect_message(&self, report: &CollectReport) -> String {
        let folder = self
            .audio
            .bundle
            .as_ref()
            .and_then(|b| {
                b.bundle
                    .root()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "the project folder".to_owned());
        let lost: Vec<String> = report.missing.iter().map(|h| self.asset_name(h)).collect();
        let shown = lost.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
        let more = lost.len().saturating_sub(5);
        let tail = if lost.is_empty() {
            String::new()
        } else if more > 0 {
            format!("; {} not found: {shown} and {more} more", lost.len())
        } else {
            format!("; {} not found: {shown}", lost.len())
        };
        if !report.copied.is_empty() {
            let n = report.copied.len();
            format!(
                "Collected {n} audio file{} ({}) into {folder}{tail}",
                if n == 1 { "" } else { "s" },
                super::super::archive::size_text(report.bytes),
            )
        } else if report.present.is_empty() {
            format!("Nothing was collected into {folder}{tail}")
        } else {
            let n = report.present.len();
            format!(
                "All {n} audio file{} already in {folder}{tail}",
                if n == 1 { " is" } else { "s are" },
            )
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
            Ok(Ok(saved)) if saved.cancelled => {
                let message = self.cancel_message(&flight, &saved.report);
                self.reject(message);
            }
            Ok(Ok(saved)) => {
                self.finish_save(&flight.file, flight.snapshot, saved.bundle);
                self.after_collect(flight.collect, &saved.report);
                if flight.note_recent {
                    self.note_recent();
                }
                if let (Some(old), Some(moved)) = (&flight.relocate, &saved.moved) {
                    self.after_move(old, moved);
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
        let flight = self.audio.saving.as_ref()?;
        let (done, total) = flight.progress.files();
        Some(
            if (flight.collect || flight.relocate.is_some()) && total > 0 {
                let what = if flight.relocate.is_some() {
                    "Moving"
                } else {
                    "Collecting"
                };
                format!("{what} audio {done}/{total}...")
            } else {
                "Saving...".to_owned()
            },
        )
    }
}

#[cfg(test)]
#[path = "app_core_save_tests.rs"]
mod tests;
