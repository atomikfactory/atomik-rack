//! Device patches in the app core: a one-device library item applied to a device that is
//! already in the rack, the device's settings copied and pasted, a device put back to the
//! sound of a new one, a device saved as a patch under a name the user picked, and the step to
//! the previous or next patch of the same device type in a folder.
//!
//! * **Apply** reads the `.atomikpatch`, checks it holds one device of the target's type (a
//!   patch for another type, or for several devices, changes nothing and says so), registers
//!   its samples through the usual paste path, and sends one
//!   [`WorkflowCommand::ApplyPatch`]: one undo step, with the patch's name (the file name when
//!   the file has none) on the device. The audio of the patch is adopted like a loaded item's.
//! * **Save** writes the device as a patch item (the file name is the patch name, kept on the
//!   device too, which is one undo step). The shell asks for the name with the platform's save
//!   dialog (it confirms an overwrite) and calls [`AppCore::save_device_patch`] with the path.
//! * **Previous / next** step through the patches of the device's type in the folder of the
//!   patch it last took (else the folder last browsed for its type, else the library's Patches
//!   folder), in natural name order and wrapping around; each is applied as its own undo step.
//!   The folder's patches are probed once and kept (by the folder's modification time), not
//!   read again at every step.
//! * **Copy and paste settings** use the clipboard (`ClipboardData::Settings`), which holds one
//!   device's snapshot: pasting applies it to the primary selected device when its type is the
//!   same.
//! * **Reset** is [`WorkflowCommand::ResetDevice`] on the primary selected device.
//!
//! # Browse focus
//!
//! [`AppCore::begin_patch_browse`] aims the file browser at one device (an orange band names
//! it, only the patches for its type are listed). While it is aimed, selecting a patch in the
//! browser, stepping with previous and next, or picking a sibling from the name's pop-up
//! *auditions* it: the patch is applied to the device as a **trial** ([`Document::apply_trial`]):
//! the sound changes, the engine gets the usual param, zone and state syncs (the device's
//! node is never rebuilt, so notes that sound keep sounding), and no undo entry is made.
//!
//! * **Accept** ([`AppCore::commit_patch_browse`]): Return, a double-click on a patch, pressing
//!   another device's Browse, or any other command or an undo or redo. The whole run is one
//!   undo step, from the device as it was to the patch it has now.
//! * **Cancel** ([`AppCore::cancel_patch_browse`]): Escape. The device is put back as it was,
//!   byte for byte, and the redo history is as it was.
//!
//! See `docs/DECISIONS.md#d-device-patch-format`.

use super::*;
use crate::browser_workers::item_info;
use crate::clipboard::ClipboardData;
use atomik_audio_file::save_text_atomic;
use atomik_library::{FileKind, LibrarySection, PATCH_EXT, classify, natural_cmp};
use atomik_model::{
    DeviceSnapshot, MAX_PATCH_NAME_CHARS, WorkflowCommand, load_device_item, save_device_item,
};

/// The device the file browser applies patches to (see the module docs).
#[derive(Debug)]
struct BrowseTarget {
    device: DeviceKey,
    type_id: String,
    /// What `last` held for the device before the browse (put back by a cancel).
    before: Option<PathBuf>,
    /// The patch auditioned last (not applied again when selected again).
    current: Option<PathBuf>,
}

/// The patches of one folder: `(file, device type)` in natural name order, and the folder's
/// modification time when it was read.
#[derive(Debug)]
struct FolderPatches {
    stamp: Option<std::time::SystemTime>,
    items: Vec<(PathBuf, String)>,
}

/// Where each device last took a patch from or was saved to, the device the patch browser is
/// aimed at, the folder last browsed for each device type and the probed folders (session state;
/// the folders are also kept in `ui-state.json`).
#[derive(Debug, Default)]
pub(super) struct PatchState {
    last: BTreeMap<DeviceKey, PathBuf>,
    browse: Option<BrowseTarget>,
    folders: BTreeMap<String, PathBuf>,
    cache: BTreeMap<PathBuf, FolderPatches>,
}

/// The patch name a file name makes: the stem without control characters, trimmed and cut to
/// the longest name; `None` when nothing is left.
fn patch_name_of(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    let cleaned: String = stem
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_PATCH_NAME_CHARS)
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl<S: EngineSink> AppCore<S> {
    /// The name of the device type of `device`, for messages.
    fn type_label(&self, type_id: &str) -> String {
        self.registry
            .descriptor(type_id)
            .map_or_else(|| type_id.to_owned(), |d| d.name.to_owned())
    }

    /// The primary selected rack device, or the reason there is none.
    fn primary_device(&self) -> Result<DeviceKey, String> {
        self.view
            .primary()
            .ok_or_else(|| "Select a device first".to_string())
    }

    // ------------------------------------------------------------------ apply

    /// Applies the patch file `path` to `device` (see the module docs). The caller reports an
    /// error unless it is empty (the command was rejected and already reported).
    pub(super) fn apply_patch_file(
        &mut self,
        path: &Path,
        device: &DeviceKey,
    ) -> Result<(), String> {
        // Aimed at this device, a patch is an audition: accepted or cancelled later.
        let trial = self.browsing(device);
        self.apply_patch_with(path, device, trial)
    }

    /// [`Self::apply_patch_file`], as a trial step (no undo entry) or as an undo step.
    fn apply_patch_with(
        &mut self,
        path: &Path,
        device: &DeviceKey,
        trial: bool,
    ) -> Result<(), String> {
        let name = file_name(path);
        let text = atomik_model::read_document_text(path)
            .map_err(|e| format!("'{name}' cannot be read: {e}"))?;
        let mut snapshot = load_device_item(&text, &self.registry)
            .map_err(|e| format!("'{name}' cannot be loaded: {e}"))?;
        let Some(patch) = snapshot.single_device() else {
            return Err(format!(
                "'{name}' holds {} devices: a device patch holds exactly one",
                snapshot.devices.len()
            ));
        };
        let target = self
            .doc()
            .resolve(device)
            .ok_or_else(|| format!("The device '{device}' is gone"))?;
        if patch.type_id != target.type_id {
            return Err(format!(
                "'{name}' is a patch for {}, not for {} ({})",
                self.type_label(&patch.type_id),
                target.name,
                self.type_label(&target.type_id)
            ));
        }
        // The plugin of the patch that is not installed gets a stand-in built from its layout.
        let mut holder = empty_doc();
        holder.devices.clone_from(&snapshot.devices);
        register_stand_ins(&holder, &mut self.registry)
            .map_err(|e| format!("'{name}' cannot be loaded: {e}"))?;
        if snapshot.devices[0].patch_name.is_empty()
            && let Some(own) = patch_name_of(path)
        {
            snapshot.devices[0].patch_name = own;
        }
        let assets = snapshot.assets.clone();
        self.trial_next = trial;
        let applied = self.apply_workflow(WorkflowCommand::ApplyPatch {
            device: device.clone(),
            snapshot,
        });
        self.trial_next = false;
        applied?;
        self.adopt_assets(&path.with_extension(""), &assets);
        self.patches.last.insert(device.clone(), path.to_path_buf());
        Ok(())
    }

    // ------------------------------------------------------------------ reset, settings

    /// Puts the primary selected device back to the sound of a new device of its type.
    pub fn reset_selected_device(&mut self) {
        match self.primary_device() {
            Ok(device) => {
                let _ = self.apply_workflow(WorkflowCommand::ResetDevice { device });
            }
            Err(message) => self.report(message),
        }
    }

    /// Copies the settings (the whole sound) of the primary selected device to the clipboard.
    pub fn copy_settings(&mut self) {
        let result = self.primary_device().and_then(|device| {
            DeviceSnapshot::capture(self.doc(), std::slice::from_ref(&device), false)
                .map_err(|e| e.to_string())
        });
        match result {
            Ok(snapshot) => self.clipboard.set(ClipboardData::Settings(snapshot)),
            Err(message) => self.report(message),
        }
    }

    /// Whether the clipboard holds settings the primary selected device can take (a device of
    /// the same type).
    pub fn can_paste_settings(&self) -> bool {
        let (Some(ClipboardData::Settings(s)), Some(primary)) =
            (self.clipboard.get(), self.view.primary())
        else {
            return false;
        };
        s.single_device().is_some_and(|d| {
            self.doc()
                .resolve(&primary)
                .is_some_and(|t| t.type_id == d.type_id)
        })
    }

    /// Pastes the settings on the clipboard onto the primary selected device (one undo step).
    pub fn paste_settings(&mut self) {
        let snapshot = match self.clipboard.get() {
            Some(ClipboardData::Settings(s)) => s.clone(),
            Some(other) => {
                return self.report(format!(
                    "The clipboard holds {}: copy the settings of a device first",
                    other.kind().noun()
                ));
            }
            None => {
                return self.report("The clipboard is empty: copy the settings of a device first");
            }
        };
        let device = match self.primary_device() {
            Ok(d) => d,
            Err(message) => return self.report(message),
        };
        let (Some(from), Some(to)) = (snapshot.single_device(), self.doc().resolve(&device)) else {
            return self.report("The settings cannot be pasted here");
        };
        if from.type_id != to.type_id {
            let message = format!(
                "These are the settings of a {}: select one to paste them",
                self.type_label(&from.type_id)
            );
            return self.report(message);
        }
        let _ = self.apply_workflow(WorkflowCommand::ApplyPatch { device, snapshot });
    }

    // ------------------------------------------------------------------ save

    /// Where the save dialog for the patch of `device` starts: the library's Patches folder and
    /// a file name from the patch name (else the device name). `None` without a library folder.
    pub fn patch_save_suggestion(&self, device: &DeviceKey) -> Option<(PathBuf, String)> {
        let dir = self
            .library_root()?
            .join(LibrarySection::Patches.folder_name()?);
        let d = self.doc().resolve(device)?;
        let name = if d.patch_name.is_empty() {
            &d.name
        } else {
            &d.patch_name
        };
        Some((dir, format!("{name}.{PATCH_EXT}")))
    }

    /// Saves `device` as the patch item `path` (the platform's dialog has asked for the name
    /// and confirmed an overwrite): the file name becomes the patch name, kept on the device as
    /// one undo step. The audio of its zones is copied beside the item. A refusal or a write
    /// error goes to the status line.
    pub fn save_device_patch(&mut self, device: &DeviceKey, path: &Path) {
        if let Err(message) = self.save_patch_to(device, path) {
            self.report(message);
        }
    }

    fn save_patch_to(&mut self, device: &DeviceKey, path: &Path) -> Result<(), String> {
        let mut path = path.to_path_buf();
        if classify(&path, false) != Some(FileKind::Patch) {
            path.set_extension(PATCH_EXT);
        }
        let doc = self.document.doc();
        let mut snapshot = DeviceSnapshot::capture(doc, std::slice::from_ref(device), false)
            .map_err(|e| e.to_string())?;
        let name = patch_name_of(&path);
        if let Some(own) = &name {
            snapshot.devices[0].patch_name.clone_from(own);
        }
        let text = save_device_item(&snapshot, doc);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        save_text_atomic(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
        self.patches.last.insert(device.clone(), path.clone());
        if let Some(dir) = path.parent() {
            self.patches.cache.remove(dir);
        }
        let stem = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let hashes: Vec<String> = snapshot.assets.keys().cloned().collect();
        if hashes.is_empty() {
            self.library_saved(&stem, LibrarySection::Patches, path, None);
        } else {
            let job = crate::browser_workers::CopyAssets {
                name: stem.clone(),
                section: LibrarySection::Patches,
                to_root: path.with_extension(""),
                path,
                from: self.bundle_clone(),
                hashes,
            };
            self.copy_item_assets(job);
        }
        if let Some(own) = name {
            let _ = self.apply_workflow(WorkflowCommand::SetPatchName {
                device: device.clone(),
                name: own,
            });
        }
        Ok(())
    }

    // ------------------------------------------------------------------ previous and next

    /// The folder of the patches `device` steps through: where its patch came from or was saved
    /// to, else the folder last browsed for its type, else the library's Patches folder.
    pub fn patch_folder(&self, device: &DeviceKey) -> Option<PathBuf> {
        if let Some(dir) = self.patches.last.get(device).and_then(|p| p.parent())
            && dir.is_dir()
        {
            return Some(dir.to_path_buf());
        }
        let type_id = &self.doc().resolve(device)?.type_id;
        if let Some(dir) = self.patches.folders.get(type_id)
            && dir.is_dir()
        {
            return Some(dir.clone());
        }
        Some(
            self.library_root()?
                .join(LibrarySection::Patches.folder_name()?),
        )
    }

    /// The folder the previous and next of `device` walk: the one the file browser shows while
    /// the device is the browse target (it lists what the user sees), else [`Self::patch_folder`].
    fn stepping_folder(&self, device: &DeviceKey) -> Option<PathBuf> {
        if self.browsing(device)
            && let Some(atomik_ui::Place::Folder(dir)) = self.file_browser.location()
            && dir.is_dir()
        {
            return Some(dir.clone());
        }
        self.patch_folder(device)
    }

    /// The patch files of `folder` for devices of `type_id`, in natural name order. The folder
    /// is probed once and kept; it is probed again when its modification time changed (a patch
    /// added or removed) or `fresh` says so (a new browse, a patch saved into it).
    pub(super) fn patches_of_type(
        &mut self,
        folder: &Path,
        type_id: &str,
        fresh: bool,
    ) -> Vec<PathBuf> {
        let stamp = std::fs::metadata(folder).and_then(|m| m.modified()).ok();
        let stale = fresh
            || self
                .patches
                .cache
                .get(folder)
                .is_none_or(|c| c.stamp.is_none() || c.stamp != stamp);
        if stale {
            let mut items: Vec<(PathBuf, String)> = std::fs::read_dir(folder)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file() && classify(p, false) == Some(FileKind::Patch))
                .map(|p| {
                    let kind = item_info(&p, FileKind::Patch).map(|i| i.device_type);
                    (p, kind.unwrap_or_default())
                })
                .collect();
            items.sort_by(|a, b| natural_cmp(&file_name(&a.0), &file_name(&b.0)));
            self.patches
                .cache
                .insert(folder.to_path_buf(), FolderPatches { stamp, items });
        }
        self.patches.cache.get(folder).map_or_else(Vec::new, |c| {
            c.items
                .iter()
                .filter(|(_, t)| t == type_id)
                .map(|(p, _)| p.clone())
                .collect()
        })
    }

    /// Applies the patch `delta` places after (or, negative, before) the current one among the
    /// patches of the device's type in its patch folder, wrapping around. Each step is its own
    /// undo step, except while the device is the browse target: then it is an audition (no undo
    /// entry until accepted) and the file browser follows. A refusal goes to the status line.
    pub fn step_patch(&mut self, device: &DeviceKey, delta: i32) {
        if let Err(message) = self.step_patch_inner(device, delta)
            && !message.is_empty()
        {
            self.report(message);
        }
    }

    fn step_patch_inner(&mut self, device: &DeviceKey, delta: i32) -> Result<(), String> {
        let folder = self.stepping_folder(device).ok_or_else(|| {
            "There is no library folder: choose one in Settings > Workflow".to_string()
        })?;
        let target = self
            .doc()
            .resolve(device)
            .ok_or_else(|| format!("The device '{device}' is gone"))?;
        let (type_id, patch_name) = (target.type_id.clone(), target.patch_name.clone());
        let list = self.patches_of_type(&folder, &type_id, false);
        if list.is_empty() {
            return Err(format!(
                "There are no patches for {} in '{}'",
                self.type_label(&type_id),
                folder.display()
            ));
        }
        let known = self.patches.last.get(device);
        let current = list.iter().position(|p| known == Some(p)).or_else(|| {
            (!patch_name.is_empty())
                .then(|| {
                    list.iter()
                        .position(|p| patch_name_of(p).as_deref() == Some(&patch_name))
                })
                .flatten()
        });
        let n = list.len() as i64;
        let at = match current {
            Some(i) => (i as i64 + i64::from(delta)).rem_euclid(n),
            None if delta < 0 => n - 1,
            None => 0,
        };
        let path = list[at as usize].clone();
        self.apply_patch_file(&path, device)?;
        if self.browsing(device) {
            self.note_audition(&path);
        }
        Ok(())
    }

    /// The patches a pop-up on the patch name of `device` lists: the patches of its type in its
    /// folder, in natural name order, as `(name, file)` with the index of the current one. A long
    /// list is cut to the dozen before and after the current one.
    pub fn patch_siblings(
        &mut self,
        device: &DeviceKey,
    ) -> (Vec<(String, PathBuf)>, Option<usize>) {
        const AROUND: usize = 12;
        let (Some(folder), Some(type_id)) = (
            self.stepping_folder(device),
            self.doc().resolve(device).map(|d| d.type_id.clone()),
        ) else {
            return (Vec::new(), None);
        };
        let list = self.patches_of_type(&folder, &type_id, false);
        let known = self.patches.last.get(device);
        let current = list.iter().position(|p| known == Some(p));
        let (from, to) = match current {
            Some(c) => (c.saturating_sub(AROUND), (c + AROUND + 1).min(list.len())),
            None => (0, list.len().min(2 * AROUND + 1)),
        };
        let items = list[from..to]
            .iter()
            .map(|p| (patch_name_of(p).unwrap_or_else(|| file_name(p)), p.clone()))
            .collect();
        (items, current.map(|c| c - from))
    }

    /// Applies `path`, chosen in the pop-up of the patch name, to `device`: an audition while
    /// the device is the browse target, else one undo step.
    pub fn pick_patch(&mut self, device: &DeviceKey, path: &Path) {
        match self.apply_patch_file(path, device) {
            Ok(()) if self.browsing(device) => self.note_audition(path),
            Ok(()) => {}
            Err(message) if !message.is_empty() => self.report(message),
            Err(_) => {}
        }
    }

    // ------------------------------------------------------------------ browse

    /// Shows the patch folder of `device` in the file browser (the caller has made the panel
    /// visible).
    pub fn browse_patches(&mut self, device: &DeviceKey) {
        match self.patch_folder(device) {
            Some(folder) if folder.is_dir() => self.show_folder(folder),
            Some(folder) => self.report(format!(
                "The patch folder '{}' does not exist yet: save a patch first",
                folder.display()
            )),
            None => self.report("There is no library folder: choose one in Settings > Workflow"),
        }
    }

    /// Whether the patch browser is aimed at `device`.
    pub fn browsing(&self, device: &DeviceKey) -> bool {
        self.patches
            .browse
            .as_ref()
            .is_some_and(|b| &b.device == device)
    }

    /// The device the patch browser is aimed at.
    pub fn patch_browse_device(&self) -> Option<&DeviceKey> {
        self.patches.browse.as_ref().map(|b| &b.device)
    }

    /// Aims the patch browser at `device` (the caller has made the panel visible): the file
    /// browser lists the patches for its type in the folder it took its patch from, and
    /// selecting, stepping and picking patches auditions them (see the module docs). Another
    /// device that was the target is accepted first.
    pub fn begin_patch_browse(&mut self, device: &DeviceKey) {
        if self.patches.browse.is_some() && !self.browsing(device) {
            self.commit_patch_browse();
        }
        let Some(target) = self.doc().resolve(device) else {
            return self.report(format!("The device '{device}' is gone"));
        };
        let (type_id, label) = (target.type_id.clone(), target.name.clone());
        if self.patches.browse.is_none() {
            self.patches.browse = Some(BrowseTarget {
                device: device.clone(),
                type_id: type_id.clone(),
                before: self.patches.last.get(device).cloned(),
                current: None,
            });
        }
        self.view.set_patch_browse(Some(device.clone()));
        self.file_browser.set_patch_target(Some((label, type_id)));
        if let Some(folder) = self.patch_folder(device) {
            // A patch saved or copied in since the last browse shows up.
            self.patches.cache.remove(&folder);
        }
        self.browse_patches(device);
    }

    /// Accepts the audition: the device keeps the patch it has and the whole browse is one undo
    /// step. Does nothing when the patch browser is not aimed at a device.
    pub fn commit_patch_browse(&mut self) {
        let Some(b) = self.patches.browse.take() else {
            return;
        };
        self.document.commit_trial();
        self.end_patch_target();
        if let Some(dir) = self.patches.last.get(&b.device).and_then(|p| p.parent()) {
            self.patches.folders.insert(b.type_id, dir.to_path_buf());
        }
    }

    /// Cancels the audition: the device is put back as it was before the browse (and the redo
    /// history with it) and the patch browser is released. Does nothing when it is not aimed at
    /// a device.
    pub fn cancel_patch_browse(&mut self) {
        let Some(b) = self.patches.browse.take() else {
            return;
        };
        self.end_patch_target();
        let undone = catch_unwind(AssertUnwindSafe(|| self.document.cancel_trial()));
        match undone {
            Ok(Ok(ops)) => {
                self.sync_ops(&ops);
                self.refresh_missing();
            }
            Ok(Err(e)) => {
                log::error!("cancelling a patch browse failed: {e}");
                self.reject(format!("Cannot put the device back: {e}"));
            }
            Err(_) => {
                self.after_internal_error("a patch browse");
            }
        }
        match b.before {
            Some(path) => self.patches.last.insert(b.device, path),
            None => self.patches.last.remove(&b.device),
        };
    }

    /// The patch browser is released: the file browser lists everything again and the plate
    /// loses its colour.
    fn end_patch_target(&mut self) {
        self.view.set_patch_browse(None);
        self.file_browser.set_patch_target(None);
    }

    /// A new document replaces the one a browse was aimed at.
    pub(super) fn end_patch_browse_for_new_document(&mut self) {
        if self.patches.browse.take().is_some() {
            self.end_patch_target();
        }
    }

    /// The file browser selected the patch `path` while aimed at a device: audition it. A patch
    /// that was just auditioned is not applied again.
    pub(super) fn audition_patch(&mut self, path: &Path) {
        let Some(device) = self.patch_browse_device().cloned() else {
            return;
        };
        if self
            .patches
            .browse
            .as_ref()
            .and_then(|b| b.current.as_deref())
            == Some(path)
        {
            return;
        }
        match self.apply_patch_with(path, &device, true) {
            Ok(()) => self.note_audition(path),
            Err(message) if !message.is_empty() => self.report(message),
            Err(_) => {}
        }
    }

    /// The file browser opened the patch `path` (Return, a double-click) while aimed at a
    /// device: it goes on the device if it is not already there, and the browse is accepted.
    pub(super) fn accept_patch(&mut self, path: &Path) {
        self.audition_patch(path);
        self.commit_patch_browse();
    }

    /// Remembers the patch auditioned last and lets the file browser show it selected.
    fn note_audition(&mut self, path: &Path) {
        if let Some(b) = &mut self.patches.browse {
            b.current = Some(path.to_path_buf());
        }
        self.file_browser.select_path(path);
    }

    /// The folder last browsed for each device type (kept in `ui-state.json`).
    pub fn patch_folders(&self) -> &BTreeMap<String, PathBuf> {
        &self.patches.folders
    }

    /// Restores the folders saved by [`Self::patch_folders`].
    pub fn set_patch_folders(&mut self, folders: BTreeMap<String, PathBuf>) {
        self.patches.folders = folders;
    }

    /// The file browser showed `folder` while aimed at a device: that is the folder to go back to
    /// for the device's type.
    pub(super) fn remember_patch_folder(&mut self, folder: &Path) {
        if let Some(b) = &self.patches.browse {
            self.patches
                .folders
                .insert(b.type_id.clone(), folder.to_path_buf());
        }
    }
}
