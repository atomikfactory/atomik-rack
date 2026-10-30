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
//!   patch it last took (else the library's Patches folder), in natural name order and wrapping
//!   around; each is applied as its own undo step.
//! * **Copy and paste settings** use the clipboard (`ClipboardData::Settings`), which holds one
//!   device's snapshot: pasting applies it to the primary selected device when its type is the
//!   same.
//! * **Reset** is [`WorkflowCommand::ResetDevice`] on the primary selected device.
//!
//! Browse: [`AppCore::browse_patches`] shows the patch folder of a device in the file browser;
//! it is the hook the patch browsing focus builds on.

use super::*;
use crate::browser_workers::item_info;
use crate::clipboard::ClipboardData;
use atomik_audio_file::save_text_atomic;
use atomik_library::{FileKind, LibrarySection, PATCH_EXT, classify, natural_cmp};
use atomik_model::{
    DeviceSnapshot, MAX_PATCH_NAME_CHARS, WorkflowCommand, load_device_item, save_device_item,
};

/// Where each device last took a patch from or was saved to (session state, not saved).
#[derive(Debug, Default)]
pub(super) struct PatchState {
    last: BTreeMap<DeviceKey, PathBuf>,
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
        let name = file_name(path);
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("'{name}' cannot be read: {e}"))?;
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
        self.apply_workflow(WorkflowCommand::ApplyPatch {
            device: device.clone(),
            snapshot,
        })?;
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
    /// to, else the library's Patches folder.
    pub fn patch_folder(&self, device: &DeviceKey) -> Option<PathBuf> {
        match self.patches.last.get(device).and_then(|p| p.parent()) {
            Some(dir) if dir.is_dir() => Some(dir.to_path_buf()),
            _ => Some(
                self.library_root()?
                    .join(LibrarySection::Patches.folder_name()?),
            ),
        }
    }

    /// The patch files of `folder` for devices of `type_id`, in natural name order.
    fn patches_of_type(folder: &Path, type_id: &str) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(folder) else {
            return Vec::new();
        };
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && classify(p, false) == Some(FileKind::Patch))
            .filter(|p| item_info(p, FileKind::Patch).is_some_and(|i| i.device_type == type_id))
            .collect();
        found.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
        found
    }

    /// Applies the patch `delta` places after (or, negative, before) the current one among the
    /// patches of the device's type in its patch folder, wrapping around. Each step is its own
    /// undo step. A refusal goes to the status line.
    pub fn step_patch(&mut self, device: &DeviceKey, delta: i32) {
        if let Err(message) = self.step_patch_inner(device, delta)
            && !message.is_empty()
        {
            self.report(message);
        }
    }

    fn step_patch_inner(&mut self, device: &DeviceKey, delta: i32) -> Result<(), String> {
        let folder = self.patch_folder(device).ok_or_else(|| {
            "There is no library folder: choose one in Settings > Workflow".to_string()
        })?;
        let target = self
            .doc()
            .resolve(device)
            .ok_or_else(|| format!("The device '{device}' is gone"))?;
        let (type_id, patch_name) = (target.type_id.clone(), target.patch_name.clone());
        let list = Self::patches_of_type(&folder, &type_id);
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
        self.apply_patch_file(&path, device)
    }

    // ------------------------------------------------------------------ browse

    /// Shows the patch folder of `device` in the file browser (the caller makes the panel
    /// visible first). The hook the patch browsing focus builds on.
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
}
