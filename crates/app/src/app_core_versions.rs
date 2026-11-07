//! The version history of the open project: File > Save Version, File > Versions.
//!
//! A version is the text of a project file kept in the project's versions folder (see
//! `atomik_audio_file::versions` for the layout and for what may be deleted). Every save keeps
//! the file it replaces as an automatic version ([`super::audio`] runs that step on the save
//! worker); this module adds the rest:
//!
//! * [`AppCore::save_version`] keeps the song as it is now (unsaved changes included) under a
//!   name. A named version is never pruned.
//! * [`AppCore::versions`] lists them, newest first; [`AppCore::set_version_protected`] marks
//!   one so that pruning never removes it.
//! * [`AppCore::restore_version`] keeps the song as it is now as an automatic version, then
//!   opens the chosen one as the current song. The restored song is unsaved: Save writes it over
//!   the project file (and keeps the file it replaces as a version), and a second restore of the
//!   version kept first brings the old state back.
//!
//! * [`AppCore::compare_version`] summarises what differs between a version and the song as it is
//!   now, or between two versions, as text lines (see [`atomik_model::compare_docs`]).
//!
//! Nothing here saves the project or touches audio files.

use super::{AppCore, AppError};
use crate::sink::EngineSink;
use atomik_audio_file::{Version, VersionKind, VersionStore};
use atomik_model::ModelError;
use std::path::PathBuf;

/// One line of the versions list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRow {
    /// The number the commands take.
    pub id: u32,
    /// When it was made, in seconds since 1970.
    pub time: u64,
    /// Its name (empty for an automatic version that has none).
    pub name: String,
    /// A save (or a restore) kept it; the oldest of these are pruned.
    pub automatic: bool,
    /// Pruning never removes it (a named version always is).
    pub protected: bool,
}

/// What differs between two songs, for the versions panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionComparison {
    /// The older side: a version's time and name, or `The song now`.
    pub before: String,
    /// The newer side.
    pub after: String,
    /// The summary lines (see [`atomik_model::DocDiff::lines`]).
    pub lines: Vec<String>,
}

impl VersionRow {
    fn of(v: &Version) -> Self {
        Self {
            id: v.id,
            time: v.time,
            name: v.name.clone(),
            automatic: v.kind == VersionKind::Automatic,
            protected: v.protected || v.kind == VersionKind::Named,
        }
    }
}

impl<S: EngineSink> AppCore<S> {
    /// Sets how many automatic versions a save keeps (at least 1; the Settings preference).
    pub fn set_keep_versions(&mut self, n: u32) {
        self.keep_versions = n.clamp(1, crate::keymap_store::MAX_KEEP_VERSIONS);
    }

    /// How many automatic versions a save keeps.
    pub fn keep_versions(&self) -> u32 {
        self.keep_versions
    }

    /// The project file whose versions File > Versions lists: the current file when it is a
    /// project (a new song, or one opened from a patch, has none yet).
    pub fn versions_file(&self) -> Option<PathBuf> {
        self.file.clone().filter(|p| super::is_project_path(p))
    }

    /// The versions of the open project, newest first.
    ///
    /// # Errors
    /// [`AppError::NoPath`] without a project file; I/O errors of reading the index.
    pub fn versions(&self) -> Result<Vec<VersionRow>, AppError> {
        let file = self.versions_file().ok_or(AppError::NoPath)?;
        let store = VersionStore::for_project(&file)?;
        Ok(store.entries().iter().rev().map(VersionRow::of).collect())
    }

    /// The song as a project text and the audio it uses, as a save would write it.
    fn snapshot_text(&mut self) -> Result<(String, std::collections::BTreeSet<String>), AppError> {
        self.plugins_capture_all();
        let mut snapshot = self.doc().clone();
        snapshot.prune_unreferenced_assets();
        let text = atomik_model::save_project_checked(&snapshot)?;
        Ok((text, snapshot.referenced_assets()))
    }

    /// File > Save Version: keeps the song as it is now, unsaved changes included, as a version
    /// named `name` (never pruned). The project file is not written. Returns the version's
    /// number.
    ///
    /// # Errors
    /// [`AppError::NoPath`] without a project file; a document that does not validate; I/O
    /// errors.
    pub fn save_version(&mut self, name: &str) -> Result<u32, AppError> {
        let file = self.versions_file().ok_or(AppError::NoPath)?;
        self.settle_save();
        let (text, assets) = self.snapshot_text()?;
        let mut store = VersionStore::for_project(&file)?;
        let id = store.add(
            VersionKind::Named,
            name,
            &text,
            &assets,
            crate::logging::unix_seconds(),
        )?;
        let shown = store.get(id).map_or("", |v| v.name.as_str()).to_owned();
        let message = if shown.is_empty() {
            "Version kept".to_owned()
        } else {
            format!("Version kept: {shown}")
        };
        self.reject(message);
        Ok(id)
    }

    /// Protects the version `id` from pruning, or lifts that. A named version stays kept either
    /// way.
    ///
    /// # Errors
    /// [`AppError::NoPath`] without a project file; I/O errors.
    pub fn set_version_protected(&mut self, id: u32, protected: bool) -> Result<(), AppError> {
        let file = self.versions_file().ok_or(AppError::NoPath)?;
        let mut store = VersionStore::for_project(&file)?;
        store.set_protected(id, protected)?;
        Ok(())
    }

    /// A version's name, or its time when it has none, for a heading.
    fn version_label(v: &Version) -> String {
        let when = crate::logging::minute_stamp(v.time);
        if v.name.is_empty() {
            when
        } else {
            format!("{when} {}", v.name)
        }
    }

    /// File > Versions > Compare: what differs between the version `id` and `with` (another
    /// version, or the song as it is now, unsaved changes included, when `None`). Of two
    /// versions the older is the `before` side. Nothing changes: no version is kept and the song
    /// is not touched.
    ///
    /// # Errors
    /// [`AppError::NoPath`] without a project file; a version that is missing, cannot be read or
    /// does not load; the same version twice; I/O errors.
    pub fn compare_version(
        &self,
        id: u32,
        with: Option<u32>,
    ) -> Result<VersionComparison, AppError> {
        let file = self.versions_file().ok_or(AppError::NoPath)?;
        let store = VersionStore::for_project(&file)?;
        let load = |id: u32| -> Result<(String, atomik_model::RackDoc), AppError> {
            let v = store.get(id).ok_or_else(|| {
                AppError::Model(ModelError::BadScope(format!("there is no version {id}")))
            })?;
            let label = Self::version_label(v);
            let text = store.read(id)?;
            let (doc, _) = atomik_model::load_project_with_warnings(&text, &self.registry)?;
            Ok((label, doc))
        };
        let (before, after, diff) = match with {
            None => {
                let (label, doc) = load(id)?;
                let diff = atomik_model::compare_docs(&doc, self.doc(), &self.registry);
                (label, "The song now".to_owned(), diff)
            }
            Some(other) if other == id => {
                return Err(AppError::Model(ModelError::BadScope(
                    "choose two different versions to compare".to_owned(),
                )));
            }
            Some(other) => {
                let (old, new) = if id < other { (id, other) } else { (other, id) };
                let ((l_old, d_old), (l_new, d_new)) = (load(old)?, load(new)?);
                let diff = atomik_model::compare_docs(&d_old, &d_new, &self.registry);
                (l_old, l_new, diff)
            }
        };
        Ok(VersionComparison {
            before,
            after,
            lines: diff.lines(),
        })
    }

    /// File > Versions > Restore: opens the version `id` as the current song (module docs). The
    /// song as it is now is kept as an automatic version first, so a restore can be undone by
    /// restoring that one. The caller has asked about unsaved changes already. On error nothing
    /// changes (a version that cannot be read or does not load is refused before anything is
    /// kept or replaced).
    ///
    /// # Errors
    /// [`AppError::NoPath`] without a project file; a version that is missing or does not load;
    /// I/O errors.
    pub fn restore_version(&mut self, id: u32) -> Result<(), AppError> {
        let file = self.versions_file().ok_or(AppError::NoPath)?;
        self.settle_save();
        let mut store = VersionStore::for_project(&file)?;
        let text = store.read(id)?;
        // Refused before anything is kept or replaced.
        atomik_model::load_project_with_warnings(&text, &self.registry)?;
        let (now, assets) = self.snapshot_text()?;
        if !store.latest_is(&now) {
            store.add(
                VersionKind::Automatic,
                "Before restoring a version",
                &now,
                &assets,
                crate::logging::unix_seconds(),
            )?;
        }
        let baseline = self.saved.clone();
        self.open_str(&text, Some(file))?;
        // What the file holds is still the old state: the restored song is an unsaved change.
        self.set_saved(baseline);
        self.reject(
            "Version restored; Save keeps it, restoring the version before undoes it".to_owned(),
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "app_core_versions_tests.rs"]
mod tests;
