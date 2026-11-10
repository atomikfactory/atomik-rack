//! The themes the program offers: the three built-in ones and the user's own, one JSON file each
//! in the `themes/` folder next to `settings.json`.
//!
//! A [`ThemeLibrary`] reads that folder ([`ThemeLibrary::load`]) and does what the theme manager
//! asks: duplicate, rename, delete, import, export and save a theme. The built-in themes are
//! read-only (a duplicate of one is the way to change it). A file that is not a theme (not JSON,
//! an unknown version, a wrong colour) is skipped, with one log line, and the names of the files
//! skipped are kept for one message to the user.
//!
//! A theme is named by an id: `dark`, `light` or `synthwave` for the built-in ones, `user:<file
//! stem>` for a user file. The id is what `workflow.json` keeps (`theme_id`), so renaming a theme
//! never loses the choice.

use atomik_ui::Theme;
use atomik_ui::theme_doc::{
    self, BUILTIN_IDS, Base, MAX_THEME_BYTES, ThemeDoc, ThemeError, builtin_doc, builtin_label,
};
use std::path::{Path, PathBuf};

/// The folder of the user's themes, next to the settings file.
pub const FOLDER: &str = "themes";

/// The prefix of the id of a user theme.
pub const USER_PREFIX: &str = "user:";

/// The themes folder next to a settings file.
pub fn folder_of(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(FOLDER)
}

/// Why a theme operation did not happen (the text is shown as it is).
#[derive(Debug, thiserror::Error)]
pub enum ThemeStoreError {
    /// There is no settings folder to keep themes in (a run without settings).
    #[error("there is no settings folder to keep themes in")]
    NoFolder,
    /// A built-in theme cannot be changed or deleted.
    #[error("a built-in theme cannot be changed: duplicate it first")]
    BuiltIn,
    /// No such theme.
    #[error("there is no such theme")]
    Unknown,
    /// The name is empty.
    #[error("a theme needs a name")]
    NoName,
    /// Another theme has the name.
    #[error("a theme named {0} exists already")]
    Taken(String),
    /// The file is not a theme.
    #[error("{0}")]
    Invalid(#[from] ThemeError),
    /// A file could not be read or written.
    #[error("{0}")]
    Io(String),
}

/// One theme of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeEntry {
    /// The id (see the module documentation).
    pub id: String,
    /// The theme's file: its name, what it falls back to and what it sets.
    pub doc: ThemeDoc,
    /// The file of a user theme.
    pub file: Option<PathBuf>,
    /// A built-in theme: read-only.
    pub builtin: bool,
}

impl ThemeEntry {
    /// The name shown in the list (the built-in themes' names are translated where drawn).
    pub fn name(&self) -> &str {
        if self.builtin {
            builtin_label(&self.id)
        } else {
            &self.doc.name
        }
    }

    /// The colours the theme paints with.
    pub fn theme(&self) -> Theme {
        if self.builtin {
            theme_doc::builtin_theme(&self.id).unwrap_or_default()
        } else {
            self.doc.resolve()
        }
    }
}

/// A name as a file name: lower-case letters and digits, one dash between words.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "theme".into() } else { out }
}

/// The built-in themes and the user's, with the folder the user's live in.
#[derive(Debug, Clone, Default)]
pub struct ThemeLibrary {
    dir: Option<PathBuf>,
    entries: Vec<ThemeEntry>,
    skipped: Vec<String>,
}

impl ThemeLibrary {
    /// The built-in themes only (a run with no settings folder).
    pub fn builtin_only() -> Self {
        let mut lib = Self::default();
        lib.rebuild(Vec::new());
        lib
    }

    /// Reads the folder (a missing folder is an empty one). Each file that is not a theme is
    /// left out, logged once and kept in [`ThemeLibrary::skipped`].
    pub fn load(dir: Option<PathBuf>) -> Self {
        let mut lib = Self {
            dir,
            ..Self::default()
        };
        lib.reload();
        lib
    }

    /// Reads the folder again.
    pub fn reload(&mut self) {
        let mut users = Vec::new();
        self.skipped.clear();
        if let Some(dir) = &self.dir
            && let Ok(read) = std::fs::read_dir(dir)
        {
            let mut files: Vec<PathBuf> = read
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
                })
                .collect();
            files.sort();
            for file in files {
                let shown = file
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match read_theme(&file) {
                    Ok(doc) => {
                        let stem = file
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        users.push(ThemeEntry {
                            id: format!("{USER_PREFIX}{stem}"),
                            doc,
                            file: Some(file),
                            builtin: false,
                        });
                    }
                    Err(e) => {
                        log::warn!("theme file {shown} skipped: {e}");
                        self.skipped.push(shown);
                    }
                }
            }
        }
        self.rebuild(users);
    }

    fn rebuild(&mut self, mut users: Vec<ThemeEntry>) {
        users.sort_by_key(|u| u.doc.name.to_lowercase());
        self.entries = BUILTIN_IDS
            .iter()
            .filter_map(|id| {
                builtin_doc(id).map(|doc| ThemeEntry {
                    id: (*id).to_string(),
                    doc,
                    file: None,
                    builtin: true,
                })
            })
            .collect();
        self.entries.extend(users);
    }

    /// The file names skipped by the last [`ThemeLibrary::load`] or [`ThemeLibrary::reload`].
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    /// The themes folder, when there is one.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Every theme: the built-in ones, then the user's by name.
    pub fn entries(&self) -> &[ThemeEntry] {
        &self.entries
    }

    /// The theme of an id.
    pub fn get(&self, id: &str) -> Option<&ThemeEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// The colours of the theme of an id.
    pub fn theme_of(&self, id: &str) -> Option<Theme> {
        self.get(id).map(ThemeEntry::theme)
    }

    /// The id after `id` in list order (wrapping); the first when `id` is not in the list.
    pub fn next_id(&self, id: &str) -> String {
        let at = self.entries.iter().position(|e| e.id == id);
        let next = at.map_or(0, |i| (i + 1) % self.entries.len());
        self.entries[next].id.clone()
    }

    fn name_taken(&self, name: &str, except: Option<&str>) -> bool {
        self.entries
            .iter()
            .filter(|e| Some(e.id.as_str()) != except)
            .any(|e| e.name().eq_ignore_ascii_case(name) || e.doc.name.eq_ignore_ascii_case(name))
    }

    /// `name`, or `name 2`, `name 3`... when the name is taken.
    fn free_name(&self, name: &str) -> String {
        if !self.name_taken(name, None) {
            return name.to_string();
        }
        (2..)
            .map(|n| format!("{name} {n}"))
            .find(|candidate| !self.name_taken(candidate, None))
            .unwrap_or_else(|| name.to_string())
    }

    fn free_stem(&self, name: &str) -> Result<String, ThemeStoreError> {
        let dir = self.dir.as_ref().ok_or(ThemeStoreError::NoFolder)?;
        let base = slug(name);
        (1..)
            .map(|n| {
                if n == 1 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                }
            })
            .find(|stem| {
                !dir.join(format!("{stem}.json")).exists()
                    && !BUILTIN_IDS.contains(&stem.as_str())
                    && self.get(&format!("{USER_PREFIX}{stem}")).is_none()
            })
            .ok_or(ThemeStoreError::NoFolder)
    }

    fn write_new(&mut self, doc: ThemeDoc) -> Result<String, ThemeStoreError> {
        let stem = self.free_stem(&doc.name)?;
        let dir = self.dir.clone().ok_or(ThemeStoreError::NoFolder)?;
        let file = dir.join(format!("{stem}.json"));
        write_file(&file, &doc.to_json())?;
        let id = format!("{USER_PREFIX}{stem}");
        self.reload();
        Ok(id)
    }

    /// Makes a user theme as a copy of the theme `id` (a built-in one is copied whole), named
    /// "<name> copy"; the id of the copy.
    ///
    /// # Errors
    /// No such theme, no folder, or the file could not be written.
    pub fn duplicate(&mut self, id: &str) -> Result<String, ThemeStoreError> {
        let entry = self.get(id).ok_or(ThemeStoreError::Unknown)?;
        let mut doc = entry.doc.clone();
        doc.name = theme_doc::clean_name(&format!("{} copy", entry.name()));
        doc.name = self.free_name(&doc.name);
        self.write_new(doc)
    }

    /// Makes a user theme of `doc` (named as it is, or with a number when that is taken).
    ///
    /// # Errors
    /// No folder, or the file could not be written.
    pub fn create(&mut self, mut doc: ThemeDoc) -> Result<String, ThemeStoreError> {
        doc.name = self.free_name(&doc.name);
        self.write_new(doc)
    }

    /// Renames a user theme.
    ///
    /// # Errors
    /// A built-in or unknown theme, an empty or taken name, or the file could not be written.
    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), ThemeStoreError> {
        let entry = self.get(id).ok_or(ThemeStoreError::Unknown)?;
        if entry.builtin {
            return Err(ThemeStoreError::BuiltIn);
        }
        let name = theme_doc::clean_name(name);
        if name.is_empty() {
            return Err(ThemeStoreError::NoName);
        }
        if self.name_taken(&name, Some(id)) {
            return Err(ThemeStoreError::Taken(name));
        }
        let mut doc = entry.doc.clone();
        doc.name = name;
        self.save(id, &doc)
    }

    /// Writes the file of a user theme (the editor's Save).
    ///
    /// # Errors
    /// A built-in or unknown theme, or the file could not be written.
    pub fn save(&mut self, id: &str, doc: &ThemeDoc) -> Result<(), ThemeStoreError> {
        let entry = self.get(id).ok_or(ThemeStoreError::Unknown)?;
        let file = match (&entry.file, entry.builtin) {
            (_, true) => return Err(ThemeStoreError::BuiltIn),
            (Some(f), false) => f.clone(),
            (None, false) => return Err(ThemeStoreError::Unknown),
        };
        write_file(&file, &doc.to_json())?;
        self.reload();
        Ok(())
    }

    /// Deletes the file of a user theme.
    ///
    /// # Errors
    /// A built-in or unknown theme, or the file could not be removed.
    pub fn delete(&mut self, id: &str) -> Result<(), ThemeStoreError> {
        let entry = self.get(id).ok_or(ThemeStoreError::Unknown)?;
        if entry.builtin {
            return Err(ThemeStoreError::BuiltIn);
        }
        let file = entry.file.clone().ok_or(ThemeStoreError::Unknown)?;
        std::fs::remove_file(&file).map_err(|e| ThemeStoreError::Io(e.to_string()))?;
        self.reload();
        Ok(())
    }

    /// Checks a `.json` file and copies it into the themes folder as a new user theme (renamed
    /// with a number when its name is taken); the id of the new theme.
    ///
    /// # Errors
    /// The file is not a theme, there is no folder, or the copy could not be written.
    pub fn import(&mut self, path: &Path) -> Result<String, ThemeStoreError> {
        let doc = read_theme(path)?;
        self.create(doc)
    }

    /// Writes the theme `id` as a theme file to `path` (a built-in theme as the whole file).
    ///
    /// # Errors
    /// No such theme, or the file could not be written.
    pub fn export(&self, id: &str, path: &Path) -> Result<(), ThemeStoreError> {
        let entry = self.get(id).ok_or(ThemeStoreError::Unknown)?;
        let mut doc = entry.doc.clone();
        doc.name = entry.name().to_string();
        write_file(path, &doc.to_json())
    }
}

fn write_file(path: &Path, text: &str) -> Result<(), ThemeStoreError> {
    crate::recovery::atomic_write(path, text.as_bytes())
        .map_err(|e| ThemeStoreError::Io(format!("{}: {e}", path.display())))
}

/// Reads and checks a theme file (never more than [`MAX_THEME_BYTES`] of it).
fn read_theme(path: &Path) -> Result<ThemeDoc, ThemeStoreError> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| ThemeStoreError::Io(e.to_string()))?;
    let mut text = String::new();
    file.take(MAX_THEME_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|e| ThemeStoreError::Io(e.to_string()))?;
    Ok(ThemeDoc::parse(&text)?)
}

/// What the Themes tab and the View menu show for the choice of a theme: the id of the theme in
/// use for a [`crate::keymap_store::WorkflowPrefs`].
pub fn active_id(theme_id: &str, choice: atomik_ui::ThemeChoice, system_dark: bool) -> String {
    if !theme_id.is_empty() {
        return theme_id.to_string();
    }
    match choice {
        atomik_ui::ThemeChoice::Dark => "dark".into(),
        atomik_ui::ThemeChoice::Light => "light".into(),
        atomik_ui::ThemeChoice::System => if system_dark { "dark" } else { "light" }.into(),
    }
}

/// The base a new theme of the editor starts from, for a theme that is light or dark.
pub fn base_of(theme: &Theme) -> Base {
    if theme.dark { Base::Dark } else { Base::Light }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Self {
            let d =
                std::env::temp_dir().join(format!("atomik-themes-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Dir(d)
        }
        fn themes(&self) -> PathBuf {
            self.0.join(FOLDER)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn lib(d: &Dir) -> ThemeLibrary {
        ThemeLibrary::load(Some(d.themes()))
    }

    #[test]
    fn the_builtin_themes_come_first_and_cannot_be_changed() {
        let d = Dir::new("builtin");
        let mut l = lib(&d);
        let ids: Vec<_> = l.entries().iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["dark", "light", "synthwave"]);
        assert!(l.entries().iter().all(|e| e.builtin));
        assert!(matches!(l.delete("dark"), Err(ThemeStoreError::BuiltIn)));
        assert!(matches!(
            l.rename("synthwave", "Mine"),
            Err(ThemeStoreError::BuiltIn)
        ));
        let doc = l.get("light").unwrap().doc.clone();
        assert!(matches!(
            l.save("light", &doc),
            Err(ThemeStoreError::BuiltIn)
        ));
        assert_eq!(l.theme_of("dark"), Some(Theme::dark()));
        assert_eq!(l.theme_of("synthwave"), Some(Theme::synthwave()));
        assert!(!d.themes().exists(), "nothing was written");
    }

    /// Duplicate makes a file, a user theme with the same colours that can be edited, renamed
    /// and deleted; the built-in one is untouched.
    #[test]
    fn duplicate_rename_save_and_delete_work_on_user_themes_only() {
        let d = Dir::new("user");
        let mut l = lib(&d);
        let id = l.duplicate("synthwave").unwrap();
        assert_eq!(id, "user:synthwave-copy");
        assert!(d.themes().join("synthwave-copy.json").is_file());
        assert_eq!(l.get(&id).unwrap().name(), "Synthwave copy");
        assert!(!l.get(&id).unwrap().builtin);
        assert_eq!(l.theme_of(&id), Some(Theme::synthwave()));
        // A second copy gets its own name and file.
        let second = l.duplicate("synthwave").unwrap();
        assert_eq!(second, "user:synthwave-copy-2");
        assert_eq!(l.get(&second).unwrap().name(), "Synthwave copy 2");

        l.rename(&id, "Night drive").unwrap();
        assert_eq!(l.get(&id).unwrap().name(), "Night drive");
        // A name is free for its own theme (only the case changes).
        l.rename(&id, "night DRIVE").unwrap();
        assert!(matches!(
            l.rename(&second, "night drive"),
            Err(ThemeStoreError::Taken(_))
        ));
        assert!(matches!(
            l.rename(&second, "Dark"),
            Err(ThemeStoreError::Taken(_))
        ));
        assert!(matches!(
            l.rename(&second, "  "),
            Err(ThemeStoreError::NoName)
        ));

        let mut doc = l.get(&id).unwrap().doc.clone();
        doc.set_colour("text", [1, 2, 3, 255]);
        l.save(&id, &doc).unwrap();
        let again = lib(&d);
        assert_eq!(again.theme_of(&id).unwrap().text, doc.resolve().text);
        assert_eq!(Theme::synthwave(), l.theme_of("synthwave").unwrap());

        l.delete(&id).unwrap();
        assert!(l.get(&id).is_none());
        assert!(!d.themes().join("synthwave-copy.json").exists());
        assert!(l.get("synthwave").is_some());
    }

    /// A corrupt file, a file of a version this program does not know and a file with a bad
    /// colour are skipped and named; the others load.
    #[test]
    fn a_file_that_is_not_a_theme_is_skipped_and_named() {
        let d = Dir::new("skip");
        let dir = d.themes();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.json"), "{ nope").unwrap();
        std::fs::write(
            dir.join("future.json"),
            r#"{"version": 9, "name": "Future"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("badcolour.json"),
            r##"{"version": 1, "name": "Bad", "colors": {"text": "#12"}}"##,
        )
        .unwrap();
        std::fs::write(
            dir.join("fine.json"),
            r##"{"version": 1, "name": "Fine", "based_on": "light", "colors": {"text": "#101010"}}"##,
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "not a theme file at all").unwrap();
        let l = lib(&d);
        assert_eq!(
            l.skipped(),
            ["badcolour.json", "broken.json", "future.json"]
        );
        let user: Vec<_> = l.entries().iter().filter(|e| !e.builtin).collect();
        assert_eq!(user.len(), 1);
        assert_eq!(user[0].name(), "Fine");
        // It falls back to the light theme for what it does not set.
        let t = l.theme_of("user:fine").unwrap();
        assert_eq!(t.background, Theme::light().background);
        assert_ne!(t.text, Theme::light().text);
        // Nothing was deleted.
        assert!(dir.join("broken.json").exists());
    }

    #[test]
    fn import_validates_and_copies_and_export_round_trips() {
        let d = Dir::new("import");
        let mut l = lib(&d);
        let outside = d.0.join("outside");
        std::fs::create_dir_all(&outside).unwrap();

        // Export a built-in theme, import it back: a user theme with the same colours.
        let file = outside.join("neon.json");
        l.export("synthwave", &file).unwrap();
        let id = l.import(&file).unwrap();
        assert_eq!(
            l.get(&id).unwrap().name(),
            "Synthwave 2",
            "the name was taken"
        );
        assert_eq!(l.theme_of(&id), Some(Theme::synthwave()));
        assert!(d.themes().join("synthwave-2.json").is_file());

        // A user theme exports as it is.
        let mut doc = l.get(&id).unwrap().doc.clone();
        doc.set_colour("lane", [9, 8, 7, 255]);
        l.save(&id, &doc).unwrap();
        let out2 = outside.join("copy.json");
        l.export(&id, &out2).unwrap();
        assert_eq!(
            ThemeDoc::parse(&std::fs::read_to_string(&out2).unwrap()).unwrap(),
            doc
        );

        // Files that are not themes are refused and nothing is copied.
        let before = std::fs::read_dir(d.themes()).unwrap().count();
        std::fs::write(outside.join("bad.json"), "[1, 2]").unwrap();
        assert!(matches!(
            l.import(&outside.join("bad.json")),
            Err(ThemeStoreError::Invalid(_))
        ));
        std::fs::write(outside.join("v2.json"), r#"{"version": 2, "name": "x"}"#).unwrap();
        assert!(matches!(
            l.import(&outside.join("v2.json")),
            Err(ThemeStoreError::Invalid(ThemeError::Version(2)))
        ));
        assert!(matches!(
            l.import(&outside.join("missing.json")),
            Err(ThemeStoreError::Io(_))
        ));
        assert_eq!(std::fs::read_dir(d.themes()).unwrap().count(), before);
    }

    #[test]
    fn without_a_folder_nothing_can_be_made() {
        let mut l = ThemeLibrary::builtin_only();
        assert!(matches!(
            l.duplicate("dark"),
            Err(ThemeStoreError::NoFolder)
        ));
        assert_eq!(l.entries().len(), 3);
        assert_eq!(l.next_id("dark"), "light");
        assert_eq!(l.next_id("synthwave"), "dark");
        assert_eq!(l.next_id("gone"), "dark");
    }

    #[test]
    fn a_name_makes_a_safe_file_name() {
        assert_eq!(slug("Night Drive!"), "night-drive");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("\u{e9}\u{e9}"), "theme");
        let d = Dir::new("slug");
        let mut l = lib(&d);
        let id = l.create(ThemeDoc::new("../evil", Base::Dark)).unwrap();
        assert_eq!(id, "user:evil");
        assert!(d.themes().join("evil.json").is_file());
        // A user file named like a built-in one cannot shadow it.
        let id = l.create(ThemeDoc::new("Dark ", Base::Dark)).unwrap();
        assert_eq!(l.get("dark").unwrap().name(), "Dark");
        assert_eq!(l.get(&id).unwrap().name(), "Dark 2");
        assert_eq!(id, "user:dark-2");
    }

    #[test]
    fn the_active_id_follows_the_id_then_the_choice() {
        use atomik_ui::ThemeChoice;
        assert_eq!(active_id("user:x", ThemeChoice::Light, false), "user:x");
        assert_eq!(active_id("", ThemeChoice::Light, true), "light");
        assert_eq!(active_id("", ThemeChoice::System, true), "dark");
        assert_eq!(active_id("", ThemeChoice::System, false), "light");
        assert_eq!(base_of(&Theme::synthwave()), Base::Dark);
        assert_eq!(base_of(&Theme::light()), Base::Light);
    }
}
