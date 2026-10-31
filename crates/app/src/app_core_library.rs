//! Save to Library: the selected devices become a patch or a container item, a selected note
//! clip a clip item, written into the library folder (see [`atomik_library::LibrarySection`]).
//!
//! * The item text comes from the model (`save_device_item`, `save_clip_file`): the same
//!   selection always gives the same bytes. Saving is not an edit: the document and the undo
//!   history stay as they are.
//! * The file is written with the atomic text save under a free name
//!   ([`atomik_library::unique_name`]); the library and section folders are made on the first
//!   save (the browser never makes them). The name is the device's (the first one's, plus
//!   `+N` for the N others) or `<track> bar <bar>` for a clip, with the characters a file name
//!   cannot hold replaced by `_`.
//! * Audio the devices use is copied into the item's sibling bundle `<stem>/Audio/<hash>.wav`
//!   (the layout `atomik_render::bundle::existing_bundle_root` reads) by the library worker.
//!   The item is written first; the message that it is saved waits for the copy, which says
//!   so when audio is missing from the project.
//! * Afterwards the panel (when it shows) lists the section and selects the new row;
//!   otherwise the status line says `Saved 'X' to the library`. Errors go to the status line.
//!
//! # Load
//!
//! [`AppCore::load_library_item`] puts an item into the document where [`LoadTarget`] says,
//! as one undo step, and never edits the item file.
//!
//! * A **device row** (`atomik-device:<type>`) is added as the rack's browser adds it: the
//!   creation chain for an instrument or an effect at the top level, a plain add otherwise.
//! * A **patch** or **container** is pasted (`WorkflowCommand::PasteDevices`, or
//!   `PasteDevicesAt` for a rack slot): fresh keys, the cables between the devices kept. A
//!   `.atomikcontainer` that holds anything but one container is refused.
//! * Its **audio**: the pool entries are registered (no undo step), then a worker copies the
//!   `<hash>.wav` files the session bundle lacks from the item's sibling bundle into it and
//!   loads them for the engine, like an opened project's audio. A file the item does not have
//!   stays reported missing and can be relinked; the devices load anyway.
//! * A **clip** is pasted on a note lane: the selected note track and lane at the snapped
//!   playhead, or the lane of a track target.
//! * A **project** is not loaded here: the shell asks about unsaved changes and opens it
//!   ([`AppCore::take_open_request`]).
//!
//! # Rename
//!
//! `BrowseAction::RenameItem` renames an item file inside the library folder (never the
//! folder itself, never anything outside it, `..` included), keeping its extension. The new
//! name must be a valid file name stem (the characters [`stem_of`] replaces are refused, not
//! replaced) and free: a file, or a folder such as an item's audio bundle, of that name
//! blocks it, except for the item itself (a change of case). The sibling bundle folder
//! `<stem>/` is renamed with the file; if that fails the file goes back. The favourite flag and
//! tags move to the new path in `library.json`, and the panel lists the place again and selects
//! the row. A refusal goes to the status line.

use super::*;
use crate::browser_workers::{CopyAssets, CopyReport};
use atomik_audio_file::save_text_atomic;
use atomik_device_sdk::DeviceKind;
use atomik_library::{
    CLIP_EXT, CONTAINER_EXT, FileKind, LibrarySection, PATCH_EXT, classify, device_type_of,
    unique_name,
};
use atomik_model::{
    ClipEntry, ClipFile, ClipSnapshot, ContainerCommand, DeviceSnapshot, ItemKind, LaneKind,
    RackSlot, WorkflowCommand, load_clip_file, load_device_item, save_clip_file, save_device_item,
};
use atomik_ui::Modifiers;
use std::collections::BTreeMap;

/// Where a library item loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadTarget {
    /// Where a double click or Enter puts it: devices after the selected one in the rack view's
    /// scope (else at the end), a clip on the selected note track and lane at the snapped
    /// playhead.
    Default,
    /// A rack slot (a drop on the rack): `container` is the path key of the container the rack
    /// view showed, `None` at the top level (a column and an index, the column after the last
    /// makes a new one); inside a container only `slot.index` counts.
    Rack {
        /// The container the drop was in.
        container: Option<DeviceKey>,
        /// Where it goes.
        slot: RackSlot,
    },
    /// A device of the rack (a drop on its patch strip): a patch item of one device of the same
    /// type replaces the device's sound in place, as one undo step. Any other item is refused
    /// with a message.
    Device(DeviceKey),
    /// A note lane of a track at a song tick (a drop on the arrangement).
    Track {
        /// The track's instrument device.
        device: DeviceKey,
        /// The note lane.
        lane: usize,
        /// Where the clip starts.
        tick: i64,
    },
}

/// The most characters of a file name stem.
const MAX_STEM: usize = 80;

/// `name` as a file name stem: characters a file name cannot hold become `_`, the ends lose
/// spaces and dots, and an empty result is `fallback`.
fn stem_of(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(MAX_STEM)
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == ' ' || c == '.');
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

impl<S: EngineSink> AppCore<S> {
    /// Saves the selected rack devices to the library: a single container goes to
    /// **Containers**, anything else to **Patches**. The document and the undo history do not
    /// change. A refusal or a write error goes to the status line.
    pub fn save_selection_to_library(&mut self) {
        if let Err(message) = self.save_devices() {
            self.report(message);
        }
    }

    /// Saves the selected note clip to the library's **Clips** section, named after its track
    /// and bar, with the song's tempo and meter at the clip's start. The document does not
    /// change. A refusal or a write error goes to the status line.
    pub fn save_clip_to_library(&mut self) {
        if let Err(message) = self.save_clip() {
            self.report(message);
        }
    }

    /// Loads the library item `path` (a patch, a container, a clip, a device row or a project)
    /// as [`LoadTarget`] says. A refusal or a read error goes to the status line and changes
    /// nothing. See the module docs.
    pub fn load_library_item(&mut self, path: &Path, target: LoadTarget) {
        if let Err(message) = self.load_item(path, target)
            && !message.is_empty()
        {
            self.report(message);
        }
    }

    /// Asks the shell to open the project, patch or container `path` as a project (after the
    /// unsaved-changes question). Other paths are ignored.
    pub(super) fn open_library_project(&mut self, path: &Path) {
        if matches!(
            classify(path, false),
            Some(FileKind::Project | FileKind::Patch | FileKind::Container)
        ) {
            self.request_open(path.to_path_buf());
        }
    }

    fn load_item(&mut self, path: &Path, target: LoadTarget) -> Result<(), String> {
        if let LoadTarget::Device(device) = &target {
            return match classify(path, false) {
                Some(FileKind::Patch) => self.apply_patch_file(path, device),
                _ => Err(format!("'{}' is not a device patch", file_name(path))),
            };
        }
        if let Some(type_id) = device_type_of(path) {
            return self.load_device_row(type_id, target);
        }
        match classify(path, false) {
            Some(kind @ (FileKind::Patch | FileKind::Container)) => {
                self.load_devices(path, kind, target)
            }
            Some(FileKind::Clip) => self.load_clip(path, target),
            Some(FileKind::Project) => {
                self.request_open(path.to_path_buf());
                Ok(())
            }
            _ => Err(format!("'{}' is not a library item", file_name(path))),
        }
    }

    fn load_device_row(&mut self, type_id: &str, target: LoadTarget) -> Result<(), String> {
        let desc = self
            .registry
            .descriptor(type_id)
            .ok_or_else(|| format!("Unknown device type '{type_id}'"))?;
        let none = Modifiers::NONE;
        let top = self.view.scope().is_empty();
        let action = match target {
            LoadTarget::Rack {
                container: Some(container),
                slot,
            } => add_inner(&container, slot.index, desc),
            LoadTarget::Rack {
                container: None,
                slot,
            } if top => self.view.add_device_slot_action_for(desc, slot, none),
            LoadTarget::Rack {
                container: None,
                slot,
            } => UiAction::Command(Command::AddDeviceAt {
                slot,
                type_id: type_id.to_string(),
                key: None,
            }),
            _ => match self.view.scope_path() {
                Some(container) => {
                    add_inner(&container, self.rack_add_index(Some(&container)), desc)
                }
                None => self.view.add_device_action_for(desc, self.doc(), none),
            },
        };
        self.handle_action(action);
        Ok(())
    }

    fn load_devices(
        &mut self,
        path: &Path,
        kind: FileKind,
        target: LoadTarget,
    ) -> Result<(), String> {
        let name = file_name(path);
        let text = atomik_model::read_document_text(path)
            .map_err(|e| format!("'{name}' cannot be read: {e}"))?;
        let snapshot = load_device_item(&text, &self.registry)
            .map_err(|e| format!("'{name}' cannot be loaded: {e}"))?;
        if kind == FileKind::Container && ItemKind::of(&snapshot) != ItemKind::Container {
            return Err(format!("'{name}' is not a container"));
        }
        // The plugins of the item that are not installed get stand-ins built from its layouts.
        let mut holder = empty_doc();
        holder.devices.clone_from(&snapshot.devices);
        register_stand_ins(&holder, &mut self.registry)
            .map_err(|e| format!("'{name}' cannot be loaded: {e}"))?;
        let assets = snapshot.assets.clone();
        match target {
            LoadTarget::Rack {
                container: None,
                slot,
            } => self.apply_workflow(WorkflowCommand::PasteDevicesAt {
                snapshot,
                container: None,
                slot,
                route: self.view.auto_route(),
            }),
            LoadTarget::Rack {
                container: Some(container),
                slot,
            } => self.apply_workflow(WorkflowCommand::PasteDevices {
                snapshot,
                container: Some(container),
                at: slot.index,
                route: false,
            }),
            _ => self.paste_devices(snapshot, Pane::Rack),
        }?;
        self.adopt_assets(&path.with_extension(""), &assets);
        Ok(())
    }

    fn load_clip(&mut self, path: &Path, target: LoadTarget) -> Result<(), String> {
        let (track, lane, at_tick) = match target {
            LoadTarget::Track { device, lane, tick } => (device, lane, tick),
            _ => {
                let song = &self.doc().song;
                let (track, lane) = self
                    .arrangement
                    .selected_lane()
                    .filter(|(t, l)| song.track(t).is_some_and(|t| t.is_note_lane(*l)))
                    .ok_or_else(|| "Select an instrument track to load a clip".to_string())?;
                (track.clone(), lane, self.snapped_playhead())
            }
        };
        let name = file_name(path);
        let text = atomik_model::read_document_text(path)
            .map_err(|e| format!("'{name}' cannot be read: {e}"))?;
        let file = load_clip_file(&text).map_err(|e| format!("'{name}' cannot be loaded: {e}"))?;
        let span = file.clip.length;
        let clips = ClipSnapshot {
            entries: vec![ClipEntry {
                device: track.clone(),
                lane: LaneKind::Notes,
                note_lane: lane,
                clip: file.clip,
            }],
            origin: 0,
            span,
            assets: BTreeMap::new(),
        };
        self.apply_workflow(WorkflowCommand::PasteClips {
            track: Some(track),
            lane: Some(lane),
            at_tick,
            clips,
        })
    }

    /// Renames the library item `path` to the stem `to` (see the module docs). A refusal or a
    /// failure goes to the status line.
    pub(super) fn rename_library_item(&mut self, path: &Path, to: &str) {
        match self.rename_item(path, to) {
            Ok(new) => {
                self.library_moved(path, &new);
                self.show_renamed(new.clone());
                self.report(format!(
                    "Renamed '{}' to '{}'",
                    stem_name(path),
                    stem_name(&new)
                ));
            }
            Err(message) => self.report(message),
        }
    }

    fn rename_item(&self, path: &Path, to: &str) -> Result<PathBuf, String> {
        let root = self.library_folder()?;
        let inside = path != root
            && path.starts_with(&root)
            && !path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir));
        if !inside {
            return Err("Only items inside the library folder can be renamed".into());
        }
        let name = file_name(path);
        if !classify(path, false).is_some_and(|k| k.is_item() && k != FileKind::Device) {
            return Err(format!("'{name}' is not a library item"));
        }
        if !path.is_file() {
            return Err(format!("'{name}' is gone"));
        }
        let to = to.trim();
        if to.is_empty() {
            return Err("The name is empty".into());
        }
        if stem_of(to, "") != to {
            return Err(format!(
                "'{to}' contains characters a file name cannot hold"
            ));
        }
        let dir = path.parent().unwrap_or(&root);
        let mut new_name = to.to_string();
        if let Some(ext) = path.extension() {
            new_name.push('.');
            new_name.push_str(&ext.to_string_lossy());
        }
        let new = dir.join(new_name);
        // The item itself (and its audio folder) may differ in case only; anything else of
        // that name, a file or a folder, is taken.
        let bundle = path.with_extension("");
        let new_bundle = new.with_extension("");
        let file_taken = new.exists() && !same_name(&new, path);
        let folder_taken = new_bundle.exists() && !same_name(&new_bundle, &bundle);
        if file_taken || folder_taken {
            return Err(format!("'{to}' already exists in this folder"));
        }
        std::fs::rename(path, &new).map_err(|e| format!("'{name}' cannot be renamed: {e}"))?;
        if bundle.is_dir()
            && let Err(e) = std::fs::rename(&bundle, &new_bundle)
        {
            let _ = std::fs::rename(&new, path);
            return Err(format!(
                "The audio folder of '{name}' cannot be renamed: {e}"
            ));
        }
        Ok(new)
    }

    fn library_folder(&self) -> Result<PathBuf, String> {
        self.library_root().ok_or_else(|| {
            "There is no library folder: choose one in Settings > Workflow".to_string()
        })
    }

    fn save_devices(&mut self) -> Result<(), String> {
        let root = self.library_folder()?;
        let keys = self.view.selection();
        if keys.is_empty() {
            return Err("Nothing is selected to save to the library".into());
        }
        let doc = self.document.doc();
        let snapshot = DeviceSnapshot::capture(doc, &keys, false).map_err(|e| {
            if keys.iter().all(|k| {
                doc.device(k)
                    .is_some_and(|d| atomik_model::console::is_console_type(&d.type_id))
            }) {
                "The Master Section and the console devices cannot be saved to the library".into()
            } else {
                e.to_string()
            }
        })?;
        let kind = ItemKind::of(&snapshot);
        let (section, ext, fallback) = match kind {
            ItemKind::Container => (LibrarySection::Containers, CONTAINER_EXT, "Container"),
            _ => (LibrarySection::Patches, PATCH_EXT, "Patch"),
        };
        let first = snapshot.devices.first().map_or("", |d| d.name.as_str());
        let name = match snapshot.devices.len() {
            0 | 1 => stem_of(first, fallback),
            n => stem_of(&format!("{first}+{}", n - 1), fallback),
        };
        let text = save_device_item(&snapshot, doc);
        let hashes: Vec<String> = snapshot.assets.keys().cloned().collect();
        let (path, stem) = write_item(&root, section, &name, ext, &text)?;
        if hashes.is_empty() {
            self.library_saved(&stem, section, path, None);
            return Ok(());
        }
        let job = CopyAssets {
            name: stem.clone(),
            section,
            to_root: path.with_file_name(&stem),
            path,
            from: self.bundle_clone(),
            hashes,
        };
        self.copy_item_assets(job);
        Ok(())
    }

    fn save_clip(&mut self) -> Result<(), String> {
        let root = self.library_folder()?;
        let ids = self.arrangement.selected_clips();
        let [id] = ids.as_slice() else {
            return Err("Select exactly one note clip to save to the library".into());
        };
        let doc = self.document.doc();
        let (track, _, clip) = doc
            .song
            .clip(*id)
            .ok_or_else(|| "The selected clip is gone".to_string())?;
        if !clip.content.is_notes() {
            return Err("Only a note clip can be saved to the library".into());
        }
        let meters = doc.song.meter_table();
        let bar = meters.map().bar_at(clip.start);
        let meter = meters.map().meter_at(clip.start);
        let tempo_bpm = doc.song.tempo_table().map().bpm(clip.start as f64);
        let mut saved = clip.clone();
        saved.id = atomik_model::ClipId(0);
        saved.start = 0;
        let text = save_clip_file(&ClipFile {
            tempo_bpm,
            meter,
            clip: saved,
        })
        .map_err(|e| e.to_string())?;
        let track_name = doc
            .device(&track.device)
            .map_or_else(|| track.device.0.as_str(), |d| d.name.as_str());
        let name = stem_of(&format!("{track_name} bar {bar}"), "Clip");
        let (path, stem) = write_item(&root, LibrarySection::Clips, &name, CLIP_EXT, &text)?;
        self.library_saved(&stem, LibrarySection::Clips, path, None);
        Ok(())
    }

    /// An item is saved (its audio, if any, copied): show it, or say so.
    pub(super) fn library_saved(
        &mut self,
        name: &str,
        section: LibrarySection,
        path: PathBuf,
        warning: Option<String>,
    ) {
        let shown = self.show_saved(section, path);
        match warning {
            Some(warning) => self.report(warning),
            None if !shown => self.report(format!("Saved '{name}' to the library")),
            None => {}
        }
    }

    /// The copy of an item's audio finished.
    pub(super) fn library_audio_copied(
        &mut self,
        name: &str,
        section: LibrarySection,
        path: PathBuf,
        report: &CopyReport,
    ) {
        let warning = if let Some(error) = &report.error {
            Some(format!(
                "Saved '{name}', but its audio was not copied: {error}"
            ))
        } else if report.missing > 0 {
            Some(format!(
                "Saved '{name}', but {} audio file(s) are missing from the project and were not copied",
                report.missing
            ))
        } else {
            None
        };
        self.library_saved(name, section, path, warning);
    }
}

/// Whether two paths name the same file in the same folder, ignoring case (the way a
/// case-insensitive file system sees them; on others only a case-only rename passes here).
fn same_name(a: &Path, b: &Path) -> bool {
    a.parent() == b.parent()
        && a.file_name().zip(b.file_name()).is_some_and(|(x, y)| {
            x.to_string_lossy().to_lowercase() == y.to_string_lossy().to_lowercase()
        })
}

/// The stem of `path` for a message.
fn stem_name(path: &Path) -> String {
    path.file_stem().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The file name of `path` for a message.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The action that adds the device `desc` inside `container` at `index`.
fn add_inner(container: &DeviceKey, index: usize, desc: &DeviceDescriptor) -> UiAction {
    UiAction::Command(Command::Container(ContainerCommand::AddInner {
        container: container.clone(),
        index,
        type_id: desc.type_id.to_string(),
        key: None,
        auto_split: desc.kind == DeviceKind::Instrument,
    }))
}

/// Writes `text` as `<root>/<Section>/<stem>.<ext>` under the first free name (also free of a
/// folder of that stem, where the item's audio goes), making the folders on the way. Returns
/// the path and the stem actually used.
fn write_item(
    root: &Path,
    section: LibrarySection,
    stem: &str,
    ext: &str,
    text: &str,
) -> Result<(PathBuf, String), String> {
    let folder = section
        .folder_name()
        .ok_or_else(|| "This section holds no files".to_string())?;
    let dir = root.join(folder);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let taken = |p: &Path| p.exists() || p.with_extension("").exists();
    let path = unique_name(&dir, stem, ext, &taken);
    save_text_atomic(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    let used = path
        .file_stem()
        .map_or_else(|| stem.to_string(), |s| s.to_string_lossy().into_owned());
    Ok((path, used))
}
