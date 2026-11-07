//! The version history of a project: older states of the project file kept next to it.
//!
//! A version is the text of a project file, stored as it was; nothing about the project format
//! changes. The versions of a project live in one folder, found by [`versions_dir`]:
//!
//! * `<name>/Versions/` when the project file sits in a bundle folder named like it, or has a
//!   sibling bundle folder `<name>/` (see `atomik_render::bundle`);
//! * `<name> Versions/` beside the project file otherwise (a project without audio, or one that
//!   shares an `Audio/` folder with other projects).
//!
//! The folder holds one file per version, named `<name> v<number>.atomikver` (the text of a
//! project file: rename it to `.atomik` to open it by hand), and an index, `versions.txt`, a
//! small tab-separated text that says what each file is: automatic or named, protected or not,
//! when it was made, its name and the audio files it uses. The extension is not `.atomik` on
//! purpose: a version must never be mistaken for another project that shares the folder.
//!
//! # What may be deleted
//!
//! Nothing outside the versions folder is ever removed, and inside it only what the index
//! lists. [`VersionStore::prune`] removes the oldest *automatic* versions that are not
//! protected, beyond the number to keep; a named version and a protected one are never touched,
//! and neither is a file the index does not list. A name in the index that is not a plain file
//! name of this folder is dropped when the index is read, so it can never reach a delete.
//! [`retire_versions`] (the end of a Move Project) removes the listed files, the index and the
//! folder, and only when [`carry_versions`] has copied them to the new place and checked the
//! copies byte for byte.

use crate::archive::is_safe_component;
use crate::bundle::{is_valid_hash, save_text_atomic};
use crate::error::{AudioFileError, Result};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The folder inside a bundle folder that holds the versions.
pub const VERSIONS_FOLDER: &str = "Versions";
/// The extension of a version file.
pub const VERSION_EXTENSION: &str = "atomikver";
/// The name of the index file.
pub const INDEX_FILE: &str = "versions.txt";
/// The longest name of a version, in characters.
pub const MAX_VERSION_NAME_CHARS: usize = 80;

const HEADER: &str = "atomik-versions 1";

/// How a version came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionKind {
    /// Kept by a save (the file it replaced) or before a restore; the oldest are pruned.
    Automatic,
    /// Kept on request (Save Version) under a name; never pruned.
    Named,
}

/// One version in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// Its number: later versions have higher numbers.
    pub id: u32,
    /// Automatic or named.
    pub kind: VersionKind,
    /// A protected version is never pruned.
    pub protected: bool,
    /// When it was made, in seconds since 1970.
    pub time: u64,
    /// Its name (empty for most automatic versions).
    pub name: String,
    /// Its file name inside the versions folder.
    pub file: String,
    /// The audio files (content hashes) the version uses.
    pub assets: Vec<String>,
}

impl Version {
    /// Pruning never removes this version.
    pub fn kept(&self) -> bool {
        self.protected || self.kind == VersionKind::Named
    }
}

/// The candidate versions folders of the project file `file`, best first.
fn candidates(file: &Path) -> Vec<PathBuf> {
    let folder = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned());
    let Some(stem) = stem else {
        return vec![folder.join(VERSIONS_FOLDER)];
    };
    let mut out = Vec::new();
    if folder
        .file_name()
        .is_some_and(|n| n.to_string_lossy() == stem)
    {
        out.push(folder.join(VERSIONS_FOLDER));
    }
    let sibling = folder.join(&stem);
    if sibling.is_dir() {
        out.push(sibling.join(VERSIONS_FOLDER));
    }
    out.push(folder.join(format!("{stem} {VERSIONS_FOLDER}")));
    out
}

/// The versions folder of the project file `file` (module docs). A folder that already holds an
/// index wins, so a project that gained its bundle folder later keeps its history.
pub fn versions_dir(file: &Path) -> PathBuf {
    let all = candidates(file);
    all.iter()
        .find(|d| d.join(INDEX_FILE).is_file())
        .or_else(|| all.first())
        .cloned()
        .unwrap_or_else(|| PathBuf::from(VERSIONS_FOLDER))
}

/// The audio files (content hashes) the project text `text` names: every quoted text in it that
/// is a content hash.
pub fn scan_assets(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for part in text.split('"') {
        if is_valid_hash(part) {
            out.insert(part.to_owned());
        }
    }
    out
}

/// A name that is safe in the index: no control characters, trimmed and limited.
fn clean_name(name: &str) -> String {
    let flat: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    flat.trim().chars().take(MAX_VERSION_NAME_CHARS).collect()
}

/// A file name the index may list: a plain name with the version extension, not the index.
fn plain_version_file(name: &str) -> bool {
    is_safe_component(name)
        && name != INDEX_FILE
        && Path::new(name)
            .extension()
            .is_some_and(|e| e == VERSION_EXTENSION)
}

fn parse_line(line: &str) -> Option<Version> {
    let mut parts = line.splitn(8, '\t');
    if parts.next()? != "v" {
        return None;
    }
    let id = parts.next()?.parse().ok()?;
    let kind = match parts.next()? {
        "a" => VersionKind::Automatic,
        "n" => VersionKind::Named,
        _ => return None,
    };
    let protected = parts.next()? == "1";
    let time = parts.next()?.parse().ok()?;
    let file = parts.next()?;
    if !plain_version_file(file) {
        return None;
    }
    let assets = parts
        .next()?
        .split(',')
        .filter(|h| is_valid_hash(h))
        .map(str::to_owned)
        .collect();
    let name = clean_name(parts.next().unwrap_or(""));
    Some(Version {
        id,
        kind,
        protected,
        time,
        name,
        file: file.to_owned(),
        assets,
    })
}

/// The versions of one project: the index of a versions folder.
#[derive(Debug, Clone)]
pub struct VersionStore {
    dir: PathBuf,
    stem: String,
    entries: Vec<Version>,
    next: u32,
}

impl VersionStore {
    /// The store of the project file `file` (the folder need not exist: the store is empty).
    ///
    /// # Errors
    /// I/O errors of reading the index. An index that is not an index (a damaged file) is moved
    /// aside as `versions.txt.bad` and the store starts empty; the version files stay.
    pub fn for_project(file: &Path) -> Result<Self> {
        let stem = file.file_stem().map_or_else(
            || "project".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        Self::open(&versions_dir(file), &stem)
    }

    /// The store of the versions folder `dir`, naming new files after `stem`.
    ///
    /// # Errors
    /// As [`VersionStore::for_project`].
    pub fn open(dir: &Path, stem: &str) -> Result<Self> {
        let mut store = Self {
            dir: dir.to_path_buf(),
            stem: stem.to_owned(),
            entries: Vec::new(),
            next: 1,
        };
        let index = dir.join(INDEX_FILE);
        let text = match fs::read_to_string(&index) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(store),
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                let _ = fs::rename(&index, dir.join(format!("{INDEX_FILE}.bad")));
                return Ok(store);
            }
            Err(e) => return Err(e.into()),
        };
        let mut lines = text.lines();
        if lines.next() != Some(HEADER) {
            let _ = fs::rename(&index, dir.join(format!("{INDEX_FILE}.bad")));
            return Ok(store);
        }
        for line in lines {
            if let Some(n) = line.strip_prefix("next\t") {
                store.next = n.trim().parse().unwrap_or(1);
            } else if let Some(version) = parse_line(line)
                && !store
                    .entries
                    .iter()
                    .any(|v| v.id == version.id || v.file == version.file)
            {
                store.entries.push(version);
            }
        }
        store.entries.sort_by_key(|v| v.id);
        let after_last = store.entries.last().map_or(1, |v| v.id.saturating_add(1));
        store.next = store.next.max(after_last);
        Ok(store)
    }

    /// The folder the versions are in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The versions, oldest first.
    pub fn entries(&self) -> &[Version] {
        &self.entries
    }

    /// The version `id`.
    pub fn get(&self, id: u32) -> Option<&Version> {
        self.entries.iter().find(|v| v.id == id)
    }

    /// The path of the file of the version `id`.
    pub fn path_of(&self, id: u32) -> Option<PathBuf> {
        self.get(id).map(|v| self.dir.join(&v.file))
    }

    /// The text of the version `id`.
    ///
    /// # Errors
    /// [`AudioFileError::InvalidData`] for a number the index does not list; I/O errors.
    pub fn read(&self, id: u32) -> Result<String> {
        let path = self
            .path_of(id)
            .ok_or_else(|| AudioFileError::InvalidData(format!("there is no version {id}")))?;
        Ok(fs::read_to_string(path)?)
    }

    /// The audio files any version uses: a save must not delete them as orphans.
    pub fn assets(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .flat_map(|v| v.assets.iter().cloned())
            .collect()
    }

    /// Whether `text` is what the newest version holds.
    pub fn latest_is(&self, text: &str) -> bool {
        self.entries
            .last()
            .and_then(|v| fs::read_to_string(self.dir.join(&v.file)).ok())
            .is_some_and(|held| held == text)
    }

    fn index_text(&self) -> String {
        let mut out = format!("{HEADER}\nnext\t{}\n", self.next);
        for v in &self.entries {
            let kind = match v.kind {
                VersionKind::Automatic => "a",
                VersionKind::Named => "n",
            };
            out.push_str(&format!(
                "v\t{}\t{kind}\t{}\t{}\t{}\t{}\t{}\n",
                v.id,
                u8::from(v.protected),
                v.time,
                v.file,
                v.assets.join(","),
                v.name
            ));
        }
        out
    }

    fn write_index(&self) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        save_text_atomic(&self.dir.join(INDEX_FILE), &self.index_text())
    }

    /// Keeps `text` as a new version made at `time` (seconds since 1970) and returns its number.
    /// `name` is cleaned (no control characters, at most [`MAX_VERSION_NAME_CHARS`]).
    ///
    /// # Errors
    /// I/O errors; a failure leaves the index as it was (a file written for it is removed).
    pub fn add(
        &mut self,
        kind: VersionKind,
        name: &str,
        text: &str,
        assets: &BTreeSet<String>,
        time: u64,
    ) -> Result<u32> {
        fs::create_dir_all(&self.dir)?;
        let id = self.next;
        let mut file = format!("{} v{id:04}.{VERSION_EXTENSION}", self.stem);
        if !plain_version_file(&file) {
            file = format!("project v{id:04}.{VERSION_EXTENSION}");
        }
        let path = self.dir.join(&file);
        save_text_atomic(&path, text)?;
        self.entries.push(Version {
            id,
            kind,
            protected: false,
            time,
            name: clean_name(name),
            file,
            assets: assets.iter().cloned().collect(),
        });
        self.next = id.saturating_add(1);
        if let Err(e) = self.write_index() {
            self.entries.pop();
            self.next = id;
            let _ = fs::remove_file(&path);
            return Err(e);
        }
        Ok(id)
    }

    /// Marks the version `id` protected or not. Returns false for a number the index does not
    /// list.
    ///
    /// # Errors
    /// I/O errors of writing the index.
    pub fn set_protected(&mut self, id: u32, protected: bool) -> Result<bool> {
        let Some(v) = self.entries.iter_mut().find(|v| v.id == id) else {
            return Ok(false);
        };
        v.protected = protected;
        self.write_index()?;
        Ok(true)
    }

    /// Removes the oldest automatic versions that are not protected, so that `keep` of them
    /// stay. Named and protected versions are never counted and never removed. Returns the
    /// numbers removed.
    ///
    /// # Errors
    /// I/O errors of removing a file or writing the index; what was removed before the error is
    /// recorded in the index.
    pub fn prune(&mut self, keep: usize) -> Result<Vec<u32>> {
        let prunable: Vec<u32> = self
            .entries
            .iter()
            .filter(|v| !v.kept())
            .map(|v| v.id)
            .collect();
        let excess = prunable.len().saturating_sub(keep);
        let mut removed = Vec::new();
        let mut failure = None;
        for id in prunable.into_iter().take(excess) {
            let Some(at) = self.entries.iter().position(|v| v.id == id) else {
                continue;
            };
            let version = &self.entries[at];
            // Only an automatic, unprotected version listed in the index is ever deleted.
            if version.kept() || !plain_version_file(&version.file) {
                continue;
            }
            match fs::remove_file(self.dir.join(&version.file)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
            self.entries.remove(at);
            removed.push(id);
        }
        if !removed.is_empty() {
            self.write_index()?;
        }
        match failure {
            Some(e) => Err(e.into()),
            None => Ok(removed),
        }
    }
}

fn same_place(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Copies the versions of the project file `old_file` to the versions folder of `new_file` (a
/// Move Project), each copy checked against its original byte for byte. The versions keep their
/// kind, protection, time and name. Returns how many were copied. Nothing is removed.
///
/// # Errors
/// [`AudioFileError::Unverified`] when a copy differs from its original; I/O errors.
pub fn carry_versions(old_file: &Path, new_file: &Path) -> Result<usize> {
    let old = VersionStore::for_project(old_file)?;
    if old.entries().is_empty() {
        return Ok(0);
    }
    let mut new = VersionStore::for_project(new_file)?;
    if same_place(old.dir(), new.dir()) {
        return Ok(0);
    }
    let mut copied = 0;
    for v in old.entries() {
        // A version that is gone has nothing to carry; one that cannot be read stops the move,
        // since `retire_versions` would otherwise remove a file that was never copied.
        let text = match fs::read_to_string(old.dir().join(&v.file)) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(AudioFileError::Unverified(format!(
                    "version {} cannot be read: {e}",
                    v.id
                )));
            }
        };
        let assets: BTreeSet<String> = v.assets.iter().cloned().collect();
        let id = new.add(v.kind, &v.name, &text, &assets, v.time)?;
        if new.read(id)? != text {
            return Err(AudioFileError::Unverified(format!(
                "version {} differs from its copy",
                v.id
            )));
        }
        if v.protected {
            new.set_protected(id, true)?;
        }
        copied += 1;
    }
    Ok(copied)
}

/// Removes the versions of the project file `old_file`: the files its index lists, the index
/// and the folder when nothing else is in it. Call it only after [`carry_versions`] succeeded
/// for the same two files. Nothing is removed when the new place shares the old versions folder.
/// Returns how many version files were removed.
///
/// # Errors
/// I/O errors.
pub fn retire_versions(old_file: &Path, new_file: &Path) -> Result<usize> {
    let old = VersionStore::for_project(old_file)?;
    if same_place(old.dir(), &versions_dir(new_file)) {
        return Ok(0);
    }
    let mut removed = 0;
    for v in old.entries() {
        match fs::remove_file(old.dir().join(&v.file)) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    match fs::remove_file(old.dir().join(INDEX_FILE)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    // Only an empty folder goes: anything else in it stays.
    let _ = fs::remove_dir(old.dir());
    Ok(removed)
}

#[cfg(test)]
mod tests;
