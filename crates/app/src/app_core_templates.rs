//! Templates, the default song, song information and the scratch folder in the app core.
//!
//! # Templates
//!
//! The templates folder is `Templates` next to the library folder (`<library>/../Templates`, so
//! `Documents/Atomik Rack/Templates` by default). A template is a project: a `.atomik` file in the
//! folder, or `<name>/<name>.atomik` with its `Audio` folder, which is the layout Save As makes.
//! [`AppCore::templates`] lists them by name (at most [`MAX_TEMPLATES`], sorted without regard to
//! case); the shell refreshes the list a few times a second while the app runs, so a project
//! dropped into the folder shows in File > New from Template without a restart.
//!
//! * [`AppCore::open_template`] opens a project as an **untitled** song: no file name, so Save asks
//!   for one and the template is never written. Its audio is copied into the session scratch
//!   bundle by the same worker that adopts a library item's audio, so it plays at once and goes
//!   into the project folder on the first save. A plugin the machine lacks keeps its stand-in, as
//!   in any opened project.
//! * [`AppCore::save_as_template`] writes a copy of the song into the folder under its name (a
//!   free name: `Name 2` when `Name` is taken) with the audio it uses. The song, its file name and
//!   its unsaved state do not change.
//! * [`AppCore::save_default_song`] is the same copy under the name `Default Song`, which replaces
//!   the previous one; File > New opens the song named by the `default_song` workflow preference
//!   instead of the starter project ([`AppCore::open_template`] too).
//!
//! # Song information
//!
//! [`AppCore::set_song_info`] sets the title, author, notes, address and e-mail as one undo step;
//! the title shows in the window title ([`AppCore::title`]).
//!
//! # Scratch folder
//!
//! [`AppCore::set_scratch_folder`] picks where the session scratch bundle (the audio of a song
//! that has no folder yet) is made, from the next import on. Save already writes only the audio
//! the song uses and removes the rest, so the folder holds nothing the project needs.

use super::*;
use atomik_audio_file::{Bundle, save_text_atomic};
use atomik_library::unique_name;
use atomik_model::{AssetMeta, SongInfo};
use atomik_render::bundle::{existing_bundle_root, resolve_project_path, save_as_layout};
use std::collections::BTreeSet;

/// The folder of the templates, next to the library folder.
pub const TEMPLATES_FOLDER: &str = "Templates";

/// The name the default song is saved under.
pub const DEFAULT_SONG_NAME: &str = "Default Song";

/// The most templates the File menu lists.
pub const MAX_TEMPLATES: usize = atomik_ui::keymap::TEMPLATE_ARGS.len();

/// Seconds between two looks at the templates folder.
const REFRESH_SECONDS: f64 = 1.0;

/// One template of the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateEntry {
    /// The name shown in the menu: the file name without its extension.
    pub name: String,
    /// The project file.
    pub path: PathBuf,
}

/// What the core remembers about the templates folder.
pub(super) struct TemplatesState {
    entries: Vec<TemplateEntry>,
    /// The time of the last look at the folder.
    checked: f64,
}

impl Default for TemplatesState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            checked: f64::NEG_INFINITY,
        }
    }
}

/// The templates in `dir`: the `.atomik` files in it and the `<name>/<name>.atomik` projects of
/// its folders, by name without regard to case, one per name, at most [`MAX_TEMPLATES`]. A
/// missing or unreadable folder has none; names that start with a dot are skipped.
pub fn list_templates(dir: &Path) -> Vec<TemplateEntry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<TemplateEntry> = Vec::new();
    for entry in read.filter_map(Result::ok) {
        let path = entry.path();
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if path.is_file() {
            if is_project_path(&path)
                && let Some(stem) = path.file_stem()
            {
                found.push(TemplateEntry {
                    name: stem.to_string_lossy().into_owned(),
                    path,
                });
            }
        } else if path.is_dir() {
            let project = path.join(format!("{name}.{PROJECT_EXTENSION}"));
            if project.is_file() {
                found.push(TemplateEntry {
                    name,
                    path: project,
                });
            }
        }
    }
    found.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
    found.dedup_by(|b, a| a.name == b.name);
    found.truncate(MAX_TEMPLATES);
    found
}

/// The name a template of `name` goes by: the characters a file name cannot hold become `_`.
pub(super) fn template_stem(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .take(80)
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == ' ' || c == '.');
    if trimmed.is_empty() {
        "Untitled".to_owned()
    } else {
        trimmed.to_owned()
    }
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ templates

    /// The templates folder: `Templates` next to the library folder. `None` while the library
    /// folder is not known or has no parent.
    pub fn templates_dir(&self) -> Option<PathBuf> {
        let library = self.library_root()?;
        let parent = library.parent()?;
        Some(parent.join(TEMPLATES_FOLDER))
    }

    /// The templates as of the last look at the folder.
    pub fn templates(&self) -> &[TemplateEntry] {
        &self.templates.entries
    }

    /// Looks at the templates folder again when a second has passed since the last look (`now` is
    /// any monotonic clock in seconds). True when the list changed.
    pub fn refresh_templates(&mut self, now: f64) -> bool {
        if now - self.templates.checked < REFRESH_SECONDS {
            return false;
        }
        self.templates.checked = now;
        self.reload_templates()
    }

    /// Looks at the templates folder now. True when the list changed.
    pub fn reload_templates(&mut self) -> bool {
        let entries = self
            .templates_dir()
            .map_or_else(Vec::new, |dir| list_templates(&dir));
        let changed = entries != self.templates.entries;
        self.templates.entries = entries;
        changed
    }

    /// The templates folder for File > Show Template Folder: made when it does not exist yet, so
    /// there is a folder to show. `None` (and a message) when it cannot be made.
    pub fn template_folder_for_show(&mut self) -> Option<PathBuf> {
        let Some(dir) = self.templates_dir() else {
            self.reject(
                "The library folder is not known, so there is no template folder".to_owned(),
            );
            return None;
        };
        match std::fs::create_dir_all(&dir) {
            Ok(()) => Some(dir),
            Err(e) => {
                self.reject(format!(
                    "Cannot make the template folder {}: {e}",
                    dir.display()
                ));
                None
            }
        }
    }

    /// Opens the project `path` as an untitled song (see the module docs): the file is never the
    /// song's file, so it cannot be overwritten, and its audio is copied into the session bundle.
    /// On error nothing changes.
    ///
    /// # Errors
    /// The file cannot be read or is not a valid project.
    pub fn open_template(&mut self, path: &Path) -> Result<(), AppError> {
        let file = resolve_project_path(path);
        let text = atomik_model::read_document_text(&file).map_err(|source| AppError::Io {
            path: file.clone(),
            source,
        })?;
        self.audio.adopt_from = existing_bundle_root(&file);
        let opened = self.open_str(&text, None);
        // `audio_opened` takes it; an open that failed before that must not leave it behind.
        self.audio.adopt_from = None;
        opened
    }

    /// A copy of the song in the templates folder under `name` (made unique), with the audio it
    /// uses. Returns the project file. The song itself is untouched.
    ///
    /// # Errors
    /// The folder or the files cannot be written.
    pub fn save_as_template(&mut self, name: &str) -> Result<PathBuf, AppError> {
        let dir = self.templates_dir().ok_or_else(|| AppError::Io {
            path: PathBuf::from(TEMPLATES_FOLDER),
            source: std::io::Error::other("the library folder is not known"),
        })?;
        let stem = template_stem(name);
        // A name is taken by a file of it and by the folder of a project with audio.
        let free = |p: &Path| p.exists() || p.with_extension("").exists();
        let target = unique_name(&dir, &stem, PROJECT_EXTENSION, &free);
        let file = self.write_copy(&dir, &target)?;
        self.reload_templates();
        Ok(file)
    }

    /// The name File > Save as Template proposes: the song's title, else its file name, else
    /// `Untitled`.
    pub fn template_name(&self) -> String {
        if !self.doc().info.title.trim().is_empty() {
            return self.doc().info.title.trim().to_owned();
        }
        self.file.as_ref().and_then(|p| p.file_stem()).map_or_else(
            || "Untitled".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        )
    }

    /// Saves a copy of the song as the default song: `Default Song` in the templates folder,
    /// replacing the one that was there. Returns its project file.
    ///
    /// # Errors
    /// The folder or the files cannot be written.
    pub fn save_default_song(&mut self) -> Result<PathBuf, AppError> {
        let dir = self.templates_dir().ok_or_else(|| AppError::Io {
            path: PathBuf::from(TEMPLATES_FOLDER),
            source: std::io::Error::other("the library folder is not known"),
        })?;
        // Write beside the old one first and swap, so a failure keeps the old default song.
        let fresh = dir.join(format!("{DEFAULT_SONG_NAME}.new.{PROJECT_EXTENSION}"));
        let written = self.write_copy(&dir, &fresh)?;
        let _ = written;
        let target = dir.join(format!("{DEFAULT_SONG_NAME}.{PROJECT_EXTENSION}"));
        let target_folder = dir.join(DEFAULT_SONG_NAME);
        let fresh_folder = dir.join(format!("{DEFAULT_SONG_NAME}.new"));
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_dir_all(&target_folder);
        let result = if fresh_folder.is_dir() {
            // With audio: `Default Song.new/Default Song.new.atomik` becomes the folder project.
            std::fs::rename(&fresh_folder, &target_folder)
                .and_then(|()| {
                    std::fs::rename(
                        target_folder.join(format!("{DEFAULT_SONG_NAME}.new.{PROJECT_EXTENSION}")),
                        target_folder.join(format!("{DEFAULT_SONG_NAME}.{PROJECT_EXTENSION}")),
                    )
                })
                .map(|()| target_folder.join(format!("{DEFAULT_SONG_NAME}.{PROJECT_EXTENSION}")))
        } else {
            std::fs::rename(&fresh, &target).map(|()| target.clone())
        };
        self.reload_templates();
        result.map_err(|source| AppError::Io {
            path: target,
            source,
        })
    }

    /// Writes the song as a project at `target` (an unused file name in `dir`): the text of the
    /// saved form, and when it uses audio the bundle folder `<dir>/<stem>/` with that audio only.
    /// The core's file, saved state and bundle do not change.
    fn write_copy(&mut self, dir: &Path, target: &Path) -> Result<PathBuf, AppError> {
        let io = |path: &Path, source| AppError::Io {
            path: path.to_path_buf(),
            source,
        };
        std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
        self.plugins_capture_all();
        let keep: BTreeSet<String> = self.document.referenced_assets();
        let mut saved = self.doc().clone();
        saved.prune_unreferenced_assets();
        let text = atomik_model::save_project_checked(&saved)?;
        if keep.is_empty() {
            save_text_atomic(target, &text).map_err(|e| audio_error(target, e))?;
            return Ok(target.to_path_buf());
        }
        let (file, root) = save_as_layout(target);
        let mut bundle = match self.audio.bundle_ref() {
            Some(b) => Bundle::copy_bundle(b.root(), &root),
            None => Bundle::create(&root),
        }
        .map_err(|e| audio_error(&root, e))?;
        // Only the audio the song uses goes along.
        bundle
            .remove_orphans(&keep)
            .map_err(|e| audio_error(&root, e))?;
        save_text_atomic(&file, &text).map_err(|e| audio_error(&file, e))?;
        Ok(file)
    }

    // ------------------------------------------------------------------ song information

    /// The song information.
    pub fn song_info(&self) -> &SongInfo {
        &self.doc().info
    }

    /// Makes `info` the song information (one undo step). Nothing happens when it is the same.
    ///
    /// # Errors
    /// A text is over its limit or holds a control character (see [`SongInfo::check`]); the
    /// message is also shown as the toast.
    pub fn set_song_info(&mut self, info: SongInfo) -> Result<(), ModelError> {
        if &info == self.song_info() {
            return Ok(());
        }
        let result = self.apply_command(Command::SetSongInfo(info));
        if let Err(e) = &result {
            self.reject(e.to_string());
        }
        result
    }

    // ------------------------------------------------------------------ scratch folder

    /// Sets where the session scratch bundle is made (`None`: the OS temporary folder), from the
    /// next import that needs one on. The folder is made when it is used.
    pub fn set_scratch_folder(&mut self, folder: Option<PathBuf>) {
        self.audio.scratch_base = folder.filter(|f| !f.as_os_str().is_empty());
    }

    /// The scratch folder in use, `None` for the OS temporary folder.
    pub fn scratch_folder(&self) -> Option<&Path> {
        self.audio.scratch_base.as_deref()
    }

    // ------------------------------------------------------------------ project folder

    /// Takes the missing audio files from the project folder `folder` (File > Relink, PROJECT
    /// FOLDER): the folder that holds the project's `Audio` folder, or that folder itself. The
    /// files are matched by their hash, copied into the session bundle and loaded by a worker; the
    /// ones the folder does not have stay missing. A message tells which case it was.
    pub fn relink_project_folder(&mut self, folder: &Path) {
        if self.audio.missing.is_empty() {
            return self.reject("No missing audio files to relink".to_owned());
        }
        // The project's folder, the Audio folder itself, or the folder beside the project file
        // that holds `<name>/Audio`.
        let beside = self
            .file
            .as_ref()
            .and_then(|f| f.file_stem())
            .map(|stem| folder.join(stem));
        let root = if folder.join("Audio").is_dir() {
            Some(folder.to_path_buf())
        } else if folder.file_name().is_some_and(|n| n == "Audio") && folder.is_dir() {
            folder.parent().map(Path::to_path_buf)
        } else {
            beside.filter(|b| b.join("Audio").is_dir())
        };
        let Some(root) = root else {
            return self.reject(format!(
                "{} has no Audio folder: choose the folder of the project",
                crate::audio_import::display_name(folder)
            ));
        };
        let Ok(bundle) = Bundle::open(&root) else {
            return self.reject(format!(
                "{} cannot be read as a project folder",
                crate::audio_import::display_name(folder)
            ));
        };
        let found: BTreeMap<String, AssetMeta> = self
            .audio
            .missing
            .iter()
            .filter(|m| bundle.exists(&m.hash))
            .filter_map(|m| Some((m.hash.clone(), self.doc().assets.get(&m.hash)?.clone())))
            .collect();
        let left = self.audio.missing.len() - found.len();
        if found.is_empty() {
            return self.reject(format!(
                "None of the missing audio files is in {}",
                crate::audio_import::display_name(&root)
            ));
        }
        let count = found.len();
        self.adopt_assets(&root, &found);
        let plural = if count == 1 { "" } else { "s" };
        self.reject(format!(
            "Relinking {count} audio file{plural} from {}; {left} still missing",
            crate::audio_import::display_name(&root)
        ));
    }
}

/// An audio-file error as an [`AppError`] naming `path`.
fn audio_error(path: &Path, e: atomik_audio_file::AudioFileError) -> AppError {
    match e {
        atomik_audio_file::AudioFileError::Io(source) => AppError::Io {
            path: path.to_path_buf(),
            source,
        },
        other => AppError::Audio(other),
    }
}

#[cfg(test)]
#[path = "app_core_templates_tests.rs"]
mod tests;
