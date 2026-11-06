//! The factory library: read-only items (patches, containers, clips and example projects) that
//! ship with the application, shown in the file browser as their own root next to the user
//! library.
//!
//! # Layout
//!
//! A factory folder looks like the library folder: `Patches/`, `Containers/` and `Clips/` hold
//! the item files (below any sub-folders), and `Projects/` holds the example songs, each a folder
//! `<name>/<name>.atomik` with the audio it needs in `Audio/` (the layout Save As writes). An
//! optional `factory.json` gives the items the tags, categories and author the browser shows (a
//! patch file holds no such marks; the user library keeps them in its database, which is the
//! user's to change, so the factory keeps its own, read-only):
//!
//! ```text
//! { "version": 1,
//!   "items": { "Patches/Warm Drift Pad.atomikpatch":
//!              { "author": "Atomik", "categories": ["Pad"], "tags": ["analog", "warm"] } } }
//! ```
//!
//! The paths are relative to the factory folder and use `/`. A tag or category that is not a
//! valid tag name is skipped.
//!
//! A project of the folder opens as a new unsaved song (`AppCore::open_template`), so the factory
//! copy is never the song's file and Save asks for a place.
//!
//! # Where it is found
//!
//! [`find_folder`]: the `ATOMIK_FACTORY_DIR` variable, else `factory` beside the program (the
//! zip, the tarball, the installer and the Debian package), else `../Resources/factory` (the
//! macOS bundle), else the `factory` folder of the source checkout the program was built in
//! (a `target/<profile>` folder below a `Cargo.toml`). It must live **outside** the user
//! library folder: deleting is refused for a top-level `Factory` folder inside it, and a factory
//! item is refused by every delete and rename anyway.
//!
//! The app never writes into it: Save to Library always writes to the user library.

use atomik_library::{FileRow, LibraryDb, LibrarySection, Tag};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// The environment variable that names the factory folder (it wins over every other place).
pub const FACTORY_DIR_VAR: &str = "ATOMIK_FACTORY_DIR";
/// The name of the folder beside the program, and of the folder in the source checkout.
pub const FACTORY_FOLDER: &str = "factory";
/// The marks file of a factory folder.
pub const MANIFEST: &str = "factory.json";
/// The newest manifest version this build reads.
pub const MANIFEST_VERSION: u32 = 1;
/// The most a manifest may hold, in bytes.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
/// How many folders above the program are searched for a source checkout.
const CHECKOUT_DEPTH: usize = 4;

/// Why a factory folder could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    /// The folder is not there.
    #[error("{0} is not a folder")]
    NotAFolder(PathBuf),
    /// The manifest could not be read.
    #[error("{MANIFEST}: {0}")]
    Manifest(String),
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    version: u32,
    #[serde(default)]
    items: BTreeMap<String, ManifestItem>,
}

#[derive(Debug, Default, Deserialize)]
struct ManifestItem {
    #[serde(default)]
    author: String,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// An opened factory folder: where it is, which sections it has, and the marks of its items.
#[derive(Debug, Clone)]
pub struct Factory {
    root: PathBuf,
    sections: Vec<LibrarySection>,
    marks: LibraryDb,
}

impl Factory {
    /// Opens the factory folder `root`: lists its sections and reads `factory.json` when there
    /// is one. A manifest that cannot be read is an error (the caller decides whether to go on
    /// without marks: [`Self::without_marks`]).
    ///
    /// # Errors
    /// [`FactoryError::NotAFolder`] when `root` is no folder, [`FactoryError::Manifest`] when the
    /// manifest is unreadable, too large, damaged or newer than this build.
    pub fn open(root: &Path) -> Result<Self, FactoryError> {
        let mut factory = Self::without_marks(root)?;
        let manifest = root.join(MANIFEST);
        let size = match std::fs::metadata(&manifest) {
            Ok(meta) => meta.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(factory),
            Err(e) => return Err(FactoryError::Manifest(e.to_string())),
        };
        if size > MAX_MANIFEST_BYTES {
            return Err(FactoryError::Manifest("it is too large".into()));
        }
        let text = std::fs::read_to_string(&manifest)
            .map_err(|e| FactoryError::Manifest(e.to_string()))?;
        factory.marks = read_marks(root, &text)?;
        Ok(factory)
    }

    /// The factory folder `root` with no marks (its sections only).
    ///
    /// # Errors
    /// [`FactoryError::NotAFolder`] when `root` is no folder.
    pub fn without_marks(root: &Path) -> Result<Self, FactoryError> {
        if !root.is_dir() {
            return Err(FactoryError::NotAFolder(root.to_path_buf()));
        }
        let sections = [
            LibrarySection::Patches,
            LibrarySection::Containers,
            LibrarySection::Clips,
            LibrarySection::Projects,
        ]
        .into_iter()
        .filter(|s| s.folder_name().is_some_and(|name| root.join(name).is_dir()))
        .collect();
        Ok(Self {
            root: root.to_path_buf(),
            sections,
            marks: LibraryDb::new(),
        })
    }

    /// The factory folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The sections the folder has items for, in tree order.
    pub fn sections(&self) -> &[LibrarySection] {
        &self.sections
    }

    /// The folder that holds the items of `section`, when the factory has one.
    pub fn folder(&self, section: LibrarySection) -> Option<PathBuf> {
        if !self.sections.contains(&section) {
            return None;
        }
        Some(self.root.join(section.folder_name()?))
    }

    /// Whether `path` is the factory folder or below it (a path with `..` never is).
    pub fn contains(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
            && !path.components().any(|c| matches!(c, Component::ParentDir))
    }

    /// Adds the factory's tags, categories and author of `path` to the lists of a row (what
    /// the user's own marks left stay; the factory's come after, without duplicates).
    pub fn merge(
        &self,
        path: &Path,
        tags: &mut Vec<String>,
        categories: &mut Vec<String>,
        author: &mut String,
    ) {
        let mark = self.marks.mark(path);
        let add = |into: &mut Vec<String>, from: &[Tag]| {
            for tag in from {
                if !into.iter().any(|t| t.eq_ignore_ascii_case(tag.as_str())) {
                    into.push(tag.as_str().to_owned());
                }
            }
        };
        add(tags, &mark.tags);
        add(categories, &mark.categories);
        if author.is_empty() {
            author.clone_from(&mark.author);
        }
    }

    /// [`Self::merge`] on a row.
    pub fn decorate(&self, row: &mut FileRow) {
        self.merge(
            &row.path,
            &mut row.tags,
            &mut row.categories,
            &mut row.author,
        );
    }
}

fn read_marks(root: &Path, text: &str) -> Result<LibraryDb, FactoryError> {
    let file: ManifestFile =
        serde_json::from_str(text).map_err(|e| FactoryError::Manifest(e.to_string()))?;
    if file.version > MANIFEST_VERSION {
        return Err(FactoryError::Manifest(format!(
            "version {} is newer than this build reads",
            file.version
        )));
    }
    let mut db = LibraryDb::new();
    for (relative, item) in file.items {
        let relative = Path::new(&relative);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| matches!(c, Component::ParentDir))
        {
            continue;
        }
        let path = root.join(relative);
        let tags = |names: &[String]| -> Vec<Tag> {
            names.iter().filter_map(|n| Tag::new(n).ok()).collect()
        };
        for tag in tags(&item.tags) {
            db.add_tag(&path, &tag);
        }
        db.set_categories(&path, &tags(&item.categories));
        db.set_author(&path, &item.author);
    }
    Ok(db)
}

/// Finds the factory folder (see the module docs): `exe` is the running program, `env` reads the
/// process environment, `is_dir` says whether a folder exists.
pub fn find_folder(
    exe: Option<&Path>,
    env: &dyn Fn(&str) -> Option<OsString>,
    is_dir: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(dir) = env(FACTORY_DIR_VAR).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let exe_dir = exe.and_then(Path::parent)?;
    let beside = exe_dir.join(FACTORY_FOLDER);
    if is_dir(&beside) {
        return Some(beside);
    }
    let bundle = exe_dir
        .parent()
        .map(|contents| contents.join("Resources").join(FACTORY_FOLDER));
    if let Some(bundle) = bundle.filter(|b| is_dir(b)) {
        return Some(bundle);
    }
    exe_dir
        .ancestors()
        .take(CHECKOUT_DEPTH)
        .map(|above| (above.join(FACTORY_FOLDER), above.join("Cargo.toml")))
        .find(|(factory, manifest)| is_dir(factory) && manifest.is_file())
        .map(|(factory, _)| factory)
}

/// [`find_folder`] on this process: the real environment and file system.
pub fn locate() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    find_folder(exe.as_deref(), &|k| std::env::var_os(k), &|p| p.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn the_variable_wins_then_the_folder_beside_the_program() {
        let exe = Path::new("/opt/atomik-rack/atomik");
        let there: BTreeSet<PathBuf> = [PathBuf::from("/opt/atomik-rack/factory")].into();
        let is_dir = |p: &Path| there.contains(p);
        assert_eq!(
            find_folder(Some(exe), &no_env, &is_dir),
            Some(PathBuf::from("/opt/atomik-rack/factory"))
        );
        let env = |k: &str| (k == FACTORY_DIR_VAR).then(|| OsString::from("/elsewhere"));
        assert_eq!(
            find_folder(Some(exe), &env, &is_dir),
            Some(PathBuf::from("/elsewhere"))
        );
        let empty = |k: &str| (k == FACTORY_DIR_VAR).then(OsString::new);
        assert_eq!(
            find_folder(Some(exe), &empty, &is_dir),
            Some(PathBuf::from("/opt/atomik-rack/factory"))
        );
    }

    #[test]
    fn a_macos_bundle_keeps_it_in_resources() {
        let exe = Path::new("/Applications/Atomik Rack.app/Contents/MacOS/atomik");
        let bundle = PathBuf::from("/Applications/Atomik Rack.app/Contents/Resources/factory");
        let is_dir = |p: &Path| p == bundle;
        assert_eq!(
            find_folder(Some(exe), &no_env, &is_dir),
            Some(bundle.clone())
        );
    }

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("atomik-app-factory-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(dir: &Path, relative: &str, text: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_build_in_a_checkout_finds_the_repository_folder_and_nothing_else_does() {
        let tmp = Tmp::new("checkout");
        let exe = tmp.0.join("target").join("debug").join("atomik");
        write(&tmp.0, "factory/Patches/a.atomikpatch", "x");
        let real = |p: &Path| p.is_dir();
        // No `Cargo.toml` above the build folder: a folder called factory is not a checkout's.
        assert_eq!(find_folder(Some(&exe), &no_env, &real), None);
        write(&tmp.0, "Cargo.toml", "[workspace]");
        assert_eq!(
            find_folder(Some(&exe), &no_env, &real),
            Some(tmp.0.join("factory"))
        );
        assert_eq!(find_folder(None, &no_env, &real), None);
    }

    #[test]
    fn sections_are_the_folders_that_exist() {
        let tmp = Tmp::new("sections");
        write(&tmp.0, "Patches/a.atomikpatch", "x");
        write(&tmp.0, "Clips/b.atomikclip", "x");
        write(&tmp.0, "Other/c.txt", "x");
        let f = Factory::open(&tmp.0).unwrap();
        assert_eq!(
            f.sections(),
            [LibrarySection::Patches, LibrarySection::Clips]
        );
        assert_eq!(
            f.folder(LibrarySection::Patches),
            Some(tmp.0.join("Patches"))
        );
        assert_eq!(f.folder(LibrarySection::Containers), None);
        assert_eq!(f.folder(LibrarySection::Devices), None);
        assert!(f.contains(&tmp.0.join("Patches").join("a.atomikpatch")));
        assert!(!f.contains(&tmp.0.join("Patches").join("..").join("..").join("x")));
        assert!(!f.contains(&tmp.0.parent().unwrap().join("elsewhere")));
        assert!(matches!(
            Factory::open(&tmp.0.join("missing")),
            Err(FactoryError::NotAFolder(_))
        ));
    }

    #[test]
    fn the_manifest_gives_tags_categories_and_an_author_without_duplicates() {
        let tmp = Tmp::new("marks");
        write(&tmp.0, "Patches/a.atomikpatch", "x");
        write(
            &tmp.0,
            MANIFEST,
            r##"{ "version": 1, "items": {
                "Patches/a.atomikpatch": { "author": "Atomik", "categories": ["Pad"],
                                           "tags": ["warm", "#bad", "Slow"] },
                "../escape.atomikpatch": { "tags": ["no"] } } }"##,
        );
        let f = Factory::open(&tmp.0).unwrap();
        let path = tmp.0.join("Patches").join("a.atomikpatch");
        let (mut tags, mut cats, mut author) =
            (vec!["slow".to_string()], Vec::new(), String::new());
        f.merge(&path, &mut tags, &mut cats, &mut author);
        assert_eq!(tags, ["slow", "warm"]);
        assert_eq!(cats, ["Pad"]);
        assert_eq!(author, "Atomik");
        let mut own = "Ada".to_string();
        f.merge(&path, &mut Vec::new(), &mut Vec::new(), &mut own);
        assert_eq!(own, "Ada", "a user's own author stays");
        let (mut tags, mut cats, mut author) = (Vec::new(), Vec::new(), String::new());
        f.merge(
            &tmp.0.join("Patches/other.atomikpatch"),
            &mut tags,
            &mut cats,
            &mut author,
        );
        assert!(tags.is_empty() && cats.is_empty() && author.is_empty());
    }

    #[test]
    fn a_damaged_or_newer_manifest_is_an_error_and_the_folder_still_opens_without_marks() {
        let tmp = Tmp::new("bad");
        write(&tmp.0, "Patches/a.atomikpatch", "x");
        write(&tmp.0, MANIFEST, "{ not json");
        assert!(matches!(
            Factory::open(&tmp.0),
            Err(FactoryError::Manifest(_))
        ));
        write(&tmp.0, MANIFEST, r#"{ "version": 99, "items": {} }"#);
        assert!(matches!(
            Factory::open(&tmp.0),
            Err(FactoryError::Manifest(_))
        ));
        assert_eq!(
            Factory::without_marks(&tmp.0).unwrap().sections(),
            [LibrarySection::Patches]
        );
    }
}
