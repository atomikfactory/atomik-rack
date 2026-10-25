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

use super::*;
use crate::browser_workers::{CopyAssets, CopyReport};
use atomik_audio_file::save_text_atomic;
use atomik_library::{CLIP_EXT, CONTAINER_EXT, LibrarySection, PATCH_EXT, unique_name};
use atomik_model::{ClipFile, DeviceSnapshot, ItemKind, save_clip_file, save_device_item};

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
