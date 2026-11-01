//! The user library database: favourites, tags, authors, categories, named favourite lists
//! and bookmarked places.

use crate::{FileRow, PathKey, Tag};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The JSON format version written by [`LibraryDb::to_json`]. Version 1 files (favourites,
/// tags and places only) still load and are written as version 2 by the next save.
pub const LIBRARY_VERSION: u32 = 2;

/// The longest author, in characters.
pub const MAX_AUTHOR_CHARS: usize = 64;

/// The marks of one file: favourite flag, tags, author and categories.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mark {
    /// Favourite flag.
    pub favorite: bool,
    /// Tags, ordered by [`Tag`]'s order (case-insensitive).
    pub tags: Vec<Tag>,
    /// Who made the file (empty when not given).
    pub author: String,
    /// Categories the user gave the file, ordered like tags.
    pub categories: Vec<Tag>,
}

/// Why a library file could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    /// Not valid JSON or not the expected shape.
    #[error("the library file is damaged: {0}")]
    Corrupt(String),
    /// Written by a newer version of the app.
    #[error("the library file has version {0}, newer than this app supports")]
    NewerVersion(u32),
    /// The database could not be turned into text. Nothing may be written then: an empty
    /// replacement would wipe the user's library.
    #[error("the library could not be encoded: {0}")]
    Encode(String),
}

/// A named list of favourite files (a favourites folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedList {
    name: Tag,
    paths: Vec<PathKey>,
}

impl NamedList {
    /// The list's name.
    pub fn name(&self) -> &Tag {
        &self.name
    }

    /// The files in the list, in the order they were added.
    pub fn paths(&self) -> &[PathKey] {
        &self.paths
    }
}

/// Favourites, tags, authors and categories keyed by [`PathKey`], named favourite lists, plus
/// the bookmarked folders shown under Places.
///
/// Entries that end up with nothing to remember are removed, so the database only holds marked
/// files. Every mutating method returns whether something changed (the app saves only then).
///
/// JSON (version 2):
/// `{"version":2,"places":["D:/Samples"],"lists":[{"name":"Drums","paths":["d:/samples/kick.wav"]}],
/// "entries":{"d:/samples/kick.wav":{"favorite":true,"tags":["808"],"author":"Ada",
/// "categories":["Percussion"]}}}` (`favorite`, `tags`, `author` and `categories` omitted when
/// empty, `lists` omitted when there are none). Version 1 is the same without `lists`,
/// `author` and `categories`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryDb {
    entries: BTreeMap<PathKey, Mark>,
    places: Vec<PathBuf>,
    lists: Vec<NamedList>,
}

impl Mark {
    fn is_empty(&self) -> bool {
        !self.favorite
            && self.tags.is_empty()
            && self.author.is_empty()
            && self.categories.is_empty()
    }

    /// Inserts `tag` in order; false when an equal tag is already present.
    fn insert_tag(&mut self, tag: &Tag) -> bool {
        insert_sorted(&mut self.tags, tag)
    }
}

impl LibraryDb {
    /// An empty database.
    pub fn new() -> Self {
        Self::default()
    }

    /// The mark of `path` (default when unmarked).
    pub fn mark(&self, path: &Path) -> Mark {
        self.entries
            .get(&PathKey::new(path))
            .cloned()
            .unwrap_or_default()
    }

    /// Whether `path` is a favourite.
    pub fn is_favorite(&self, path: &Path) -> bool {
        self.entries
            .get(&PathKey::new(path))
            .is_some_and(|m| m.favorite)
    }

    /// Sets the favourite flag.
    pub fn set_favorite(&mut self, path: &Path, favorite: bool) -> bool {
        let key = PathKey::new(path);
        if is_favorite_key(&self.entries, &key) == favorite {
            return false;
        }
        if favorite {
            self.entries.entry(key).or_default().favorite = true;
        } else {
            self.update(&key, |m| m.favorite = false);
        }
        true
    }

    /// Toggles favourite on all `paths` together: if any of them is not a favourite, all
    /// become favourites; otherwise all are cleared. Returns whether something changed.
    pub fn toggle_favorite(&mut self, paths: &[PathBuf]) -> bool {
        if paths.is_empty() {
            return false;
        }
        let make_favorite = paths.iter().any(|p| !self.is_favorite(p));
        let mut changed = false;
        for path in paths {
            changed |= self.set_favorite(path, make_favorite);
        }
        changed
    }

    /// Adds `tag` to `path` (no-op if it already has it, in any case spelling).
    pub fn add_tag(&mut self, path: &Path, tag: &Tag) -> bool {
        self.entries
            .entry(PathKey::new(path))
            .or_default()
            .insert_tag(tag)
    }

    /// Removes `tag` from `path`.
    pub fn remove_tag(&mut self, path: &Path, tag: &Tag) -> bool {
        let key = PathKey::new(path);
        let Some(mark) = self.entries.get_mut(&key) else {
            return false;
        };
        let Ok(at) = mark.tags.binary_search(tag) else {
            return false;
        };
        mark.tags.remove(at);
        self.prune(&key);
        true
    }

    /// Moves the favourite flag and tags of `from` to `to` (a renamed file), merging with what
    /// `to` already has. Returns whether `from` had any mark.
    pub fn move_path(&mut self, from: &Path, to: &Path) -> bool {
        let (from, to) = (PathKey::new(from), PathKey::new(to));
        if from == to {
            return false;
        }
        let in_lists = self.move_in_lists(&from, &to);
        let Some(mark) = self.entries.remove(&from) else {
            return in_lists;
        };
        let target = self.entries.entry(to).or_default();
        target.favorite |= mark.favorite;
        for tag in &mark.tags {
            target.insert_tag(tag);
        }
        for category in &mark.categories {
            insert_sorted(&mut target.categories, category);
        }
        if target.author.is_empty() {
            target.author = mark.author;
        }
        true
    }

    /// Every tag in use with the number of files carrying it, ordered by tag.
    pub fn all_tags(&self) -> Vec<(Tag, usize)> {
        let mut counts: BTreeMap<Tag, usize> = BTreeMap::new();
        for mark in self.entries.values() {
            for tag in &mark.tags {
                // The map keeps the first spelling it saw (entries are visited in key order).
                *counts.entry(tag.clone()).or_insert(0) += 1;
            }
        }
        counts.into_iter().collect()
    }

    /// The keys of every favourite, in key order.
    pub fn favorites(&self) -> Vec<PathKey> {
        self.entries
            .iter()
            .filter(|(_, m)| m.favorite)
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// The keys of every file tagged `tag`, in key order.
    pub fn tagged(&self, tag: &Tag) -> Vec<PathKey> {
        self.entries
            .iter()
            .filter(|(_, m)| m.tags.binary_search(tag).is_ok())
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Renames `from` to `to` on every file. When a file already has `to`, the two merge.
    pub fn rename_tag(&mut self, from: &Tag, to: &Tag) -> bool {
        let mut changed = false;
        for mark in self.entries.values_mut() {
            let Ok(at) = mark.tags.binary_search(from) else {
                continue;
            };
            if mark.tags[at].as_str() == to.as_str() {
                continue;
            }
            mark.tags.remove(at);
            mark.insert_tag(to);
            changed = true;
        }
        changed
    }

    /// Removes `tag` from every file.
    pub fn delete_tag(&mut self, tag: &Tag) -> bool {
        let mut changed = false;
        for mark in self.entries.values_mut() {
            if let Ok(at) = mark.tags.binary_search(tag) {
                mark.tags.remove(at);
                changed = true;
            }
        }
        self.entries.retain(|_, m| !m.is_empty());
        changed
    }

    /// The bookmarked folders, in the order they were added.
    pub fn places(&self) -> &[PathBuf] {
        &self.places
    }

    /// Bookmarks a folder (no duplicate by [`PathKey`]).
    pub fn add_place(&mut self, path: &Path) -> bool {
        let key = PathKey::new(path);
        if self.places.iter().any(|p| PathKey::new(p) == key) {
            return false;
        }
        self.places.push(path.to_path_buf());
        true
    }

    /// Removes a bookmark (matched by [`PathKey`]).
    pub fn remove_place(&mut self, path: &Path) -> bool {
        let key = PathKey::new(path);
        let before = self.places.len();
        self.places.retain(|p| PathKey::new(p) != key);
        self.places.len() != before
    }

    /// The author of `path` (empty when none was given).
    pub fn author(&self, path: &Path) -> String {
        self.entries
            .get(&PathKey::new(path))
            .map(|m| m.author.clone())
            .unwrap_or_default()
    }

    /// Sets the author of `path`: whitespace runs collapse, control characters go and the
    /// text is cut to [`MAX_AUTHOR_CHARS`] characters; an empty result clears it.
    pub fn set_author(&mut self, path: &Path, author: &str) -> bool {
        let author = clean_author(author);
        let key = PathKey::new(path);
        if self
            .entries
            .get(&key)
            .map_or(author.is_empty(), |m| m.author == author)
        {
            return false;
        }
        self.entries.entry(key.clone()).or_default().author = author;
        self.prune(&key);
        true
    }

    /// Replaces the categories of `path` (duplicates in any case spelling collapse).
    pub fn set_categories(&mut self, path: &Path, categories: &[Tag]) -> bool {
        let mut wanted: Vec<Tag> = Vec::new();
        for category in categories {
            insert_sorted(&mut wanted, category);
        }
        let key = PathKey::new(path);
        if self
            .entries
            .get(&key)
            .map_or(wanted.is_empty(), |m| m.categories == wanted)
        {
            return false;
        }
        self.entries.entry(key.clone()).or_default().categories = wanted;
        self.prune(&key);
        true
    }

    /// Every category in use with the number of files carrying it, ordered by category.
    pub fn all_categories(&self) -> Vec<(Tag, usize)> {
        let mut counts: BTreeMap<Tag, usize> = BTreeMap::new();
        for mark in self.entries.values() {
            for category in &mark.categories {
                *counts.entry(category.clone()).or_insert(0) += 1;
            }
        }
        counts.into_iter().collect()
    }

    /// The named favourite lists, in the order they were made.
    pub fn lists(&self) -> &[NamedList] {
        &self.lists
    }

    /// The list called `name` (compared ignoring case).
    pub fn list(&self, name: &Tag) -> Option<&NamedList> {
        self.lists.iter().find(|l| l.name == *name)
    }

    /// Makes an empty list; false when one with that name exists.
    pub fn create_list(&mut self, name: &Tag) -> bool {
        if self.list(name).is_some() {
            return false;
        }
        self.lists.push(NamedList {
            name: name.clone(),
            paths: Vec::new(),
        });
        true
    }

    /// Renames a list; false when `from` does not exist or another list already has the name
    /// `to` (a change of letter case alone is allowed).
    pub fn rename_list(&mut self, from: &Tag, to: &Tag) -> bool {
        if from != to && self.list(to).is_some() {
            return false;
        }
        let Some(list) = self.lists.iter_mut().find(|l| l.name == *from) else {
            return false;
        };
        if list.name.as_str() == to.as_str() {
            return false;
        }
        list.name = to.clone();
        true
    }

    /// Deletes a list (the files stay).
    pub fn delete_list(&mut self, name: &Tag) -> bool {
        let before = self.lists.len();
        self.lists.retain(|l| l.name != *name);
        self.lists.len() != before
    }

    /// Puts `paths` into the list `name`, making the list when it does not exist; a file is
    /// in a list once. Returns whether something changed.
    pub fn add_to_list(&mut self, name: &Tag, paths: &[PathBuf]) -> bool {
        let mut changed = self.create_list(name);
        let Some(list) = self.lists.iter_mut().find(|l| l.name == *name) else {
            return changed;
        };
        for path in paths {
            let key = PathKey::new(path);
            if !list.paths.contains(&key) {
                list.paths.push(key);
                changed = true;
            }
        }
        changed
    }

    /// Takes `paths` out of the list `name`.
    pub fn remove_from_list(&mut self, name: &Tag, paths: &[PathBuf]) -> bool {
        let Some(list) = self.lists.iter_mut().find(|l| l.name == *name) else {
            return false;
        };
        let before = list.paths.len();
        let drop: Vec<PathKey> = paths.iter().map(|p| PathKey::new(p)).collect();
        list.paths.retain(|k| !drop.contains(k));
        list.paths.len() != before
    }

    /// Whether `path` is in the list `name`.
    pub fn in_list(&self, name: &Tag, path: &Path) -> bool {
        let key = PathKey::new(path);
        self.list(name).is_some_and(|l| l.paths.contains(&key))
    }

    /// Follows a renamed file in every list; whether any list held it.
    fn move_in_lists(&mut self, from: &PathKey, to: &PathKey) -> bool {
        let mut moved = false;
        for list in &mut self.lists {
            let Some(at) = list.paths.iter().position(|k| k == from) else {
                continue;
            };
            moved = true;
            if list.paths.contains(to) {
                list.paths.remove(at);
            } else {
                list.paths[at] = to.clone();
            }
        }
        moved
    }

    /// Copies this database's favourite flag, tags, author and categories (display spelling)
    /// onto `row`.
    pub fn decorate(&self, row: &mut FileRow) {
        row.tags.clear();
        row.categories.clear();
        row.author.clear();
        match self.entries.get(&PathKey::new(&row.path)) {
            Some(mark) => {
                row.favorite = mark.favorite;
                row.tags
                    .extend(mark.tags.iter().map(|t| t.as_str().to_string()));
                row.categories
                    .extend(mark.categories.iter().map(|t| t.as_str().to_string()));
                row.author.clone_from(&mark.author);
            }
            None => row.favorite = false,
        }
    }

    /// Serialises to pretty JSON (deterministic: keys in order).
    ///
    /// # Errors
    ///
    /// [`LibraryError::Encode`] when the text cannot be produced. The caller must then keep the
    /// file it has: writing an empty text would erase the library.
    pub fn to_json(&self) -> Result<String, LibraryError> {
        let out = JsonOut {
            version: LIBRARY_VERSION,
            places: self
                .places
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            lists: self
                .lists
                .iter()
                .map(|l| ListOut {
                    name: &l.name,
                    paths: &l.paths,
                })
                .collect(),
            entries: self
                .entries
                .iter()
                .map(|(k, m)| {
                    (
                        k,
                        EntryOut {
                            favorite: m.favorite,
                            tags: &m.tags,
                            author: &m.author,
                            categories: &m.categories,
                        },
                    )
                })
                .collect(),
        };
        serde_json::to_string_pretty(&out).map_err(|e| LibraryError::Encode(e.to_string()))
    }

    /// Parses JSON written by [`Self::to_json`] (version 1 or 2). Invalid tags, categories and
    /// list names inside are skipped, not fatal.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Corrupt`] for bad JSON, a wrong shape or a missing/zero `version`;
    /// [`LibraryError::NewerVersion`] when `version` is above [`LIBRARY_VERSION`].
    pub fn from_json(text: &str) -> Result<Self, LibraryError> {
        let corrupt = |e: serde_json::Error| LibraryError::Corrupt(e.to_string());
        let value: serde_json::Value = serde_json::from_str(text).map_err(corrupt)?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| LibraryError::Corrupt("missing or invalid \"version\"".into()))?;
        if version == 0 {
            return Err(LibraryError::Corrupt("version 0 does not exist".into()));
        }
        if version > u64::from(LIBRARY_VERSION) {
            return Err(LibraryError::NewerVersion(
                u32::try_from(version).unwrap_or(u32::MAX),
            ));
        }
        let raw: JsonIn = serde_json::from_value(value).map_err(corrupt)?;

        let mut db = Self::new();
        for place in raw.places {
            db.add_place(Path::new(&place));
        }
        for list in raw.lists {
            let Ok(name) = Tag::new(&list.name) else {
                continue;
            };
            let paths: Vec<PathBuf> = list.paths.iter().map(path_of).collect();
            db.add_to_list(&name, &paths);
        }
        for (key, entry) in raw.entries {
            let mut mark = Mark {
                favorite: entry.favorite,
                author: clean_author(&entry.author),
                ..Mark::default()
            };
            for name in &entry.tags {
                if let Ok(tag) = Tag::new(name) {
                    mark.insert_tag(&tag);
                }
            }
            for name in &entry.categories {
                if let Ok(tag) = Tag::new(name) {
                    insert_sorted(&mut mark.categories, &tag);
                }
            }
            if !mark.is_empty() {
                db.entries.insert(key, mark);
            }
        }
        Ok(db)
    }

    /// Applies `f` to the mark under `key`, then drops the entry if it became empty.
    fn update(&mut self, key: &PathKey, f: impl FnOnce(&mut Mark)) {
        if let Some(mark) = self.entries.get_mut(key) {
            f(mark);
        }
        self.prune(key);
    }

    fn prune(&mut self, key: &PathKey) {
        if self.entries.get(key).is_some_and(Mark::is_empty) {
            self.entries.remove(key);
        }
    }
}

fn is_favorite_key(entries: &BTreeMap<PathKey, Mark>, key: &PathKey) -> bool {
    entries.get(key).is_some_and(|m| m.favorite)
}

fn path_of(key: &PathKey) -> PathBuf {
    PathBuf::from(key.as_str())
}

/// Inserts `tag` into the ordered `list`; false when an equal tag is already there.
fn insert_sorted(list: &mut Vec<Tag>, tag: &Tag) -> bool {
    match list.binary_search(tag) {
        Ok(_) => false,
        Err(at) => {
            list.insert(at, tag.clone());
            true
        }
    }
}

/// Trims an author: whitespace runs collapse to one space, control characters go, and the
/// text is cut at [`MAX_AUTHOR_CHARS`] characters.
fn clean_author(text: &str) -> String {
    let mut out = String::new();
    for word in text.split_whitespace() {
        let word: String = word.chars().filter(|c| !c.is_control()).collect();
        if word.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&word);
    }
    out.chars()
        .take(MAX_AUTHOR_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[derive(Serialize)]
struct JsonOut<'a> {
    version: u32,
    places: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    lists: Vec<ListOut<'a>>,
    entries: BTreeMap<&'a PathKey, EntryOut<'a>>,
}

#[derive(Serialize)]
struct ListOut<'a> {
    name: &'a Tag,
    paths: &'a [PathKey],
}

#[derive(Serialize)]
struct EntryOut<'a> {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    favorite: bool,
    #[serde(skip_serializing_if = "<[Tag]>::is_empty")]
    tags: &'a [Tag],
    #[serde(skip_serializing_if = "str::is_empty")]
    author: &'a str,
    #[serde(skip_serializing_if = "<[Tag]>::is_empty")]
    categories: &'a [Tag],
}

#[derive(Deserialize)]
struct JsonIn {
    #[serde(default)]
    places: Vec<String>,
    #[serde(default)]
    lists: Vec<ListIn>,
    #[serde(default)]
    entries: BTreeMap<PathKey, EntryIn>,
}

#[derive(Deserialize)]
struct ListIn {
    name: String,
    #[serde(default)]
    paths: Vec<PathKey>,
}

#[derive(Deserialize)]
struct EntryIn {
    #[serde(default)]
    favorite: bool,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    author: String,
    #[serde(default)]
    categories: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioFormat, FileKind};

    trait TagNames {
        fn tags_of_names(&self, path: &Path) -> Vec<String>;
    }

    impl TagNames for LibraryDb {
        fn tags_of_names(&self, path: &Path) -> Vec<String> {
            self.mark(path)
                .tags
                .iter()
                .map(|t| t.as_str().to_string())
                .collect()
        }
    }

    fn t(name: &str) -> Tag {
        Tag::new(name).unwrap()
    }

    fn p(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    fn row(path: &str) -> FileRow {
        FileRow {
            path: p(path),
            name: path.rsplit('/').next().unwrap().to_string(),
            kind: FileKind::Audio(AudioFormat::Wav),
            size: 0,
            info: None,
            item: None,
            favorite: false,
            tags: Vec::new(),
            author: String::new(),
            categories: Vec::new(),
        }
    }

    #[test]
    fn unmarked_files_have_default_mark() {
        let db = LibraryDb::new();
        assert_eq!(db.mark(&p("/a.wav")), Mark::default());
        assert!(!db.is_favorite(&p("/a.wav")));
    }

    #[test]
    fn set_favorite_reports_changes() {
        let mut db = LibraryDb::new();
        assert!(!db.set_favorite(&p("/a.wav"), false));
        assert!(db.set_favorite(&p("/a.wav"), true));
        assert!(!db.set_favorite(&p("/a.wav"), true));
        assert!(db.is_favorite(&p("/a.wav")));
        assert!(db.set_favorite(&p("/a.wav"), false));
        assert_eq!(db, LibraryDb::new(), "empty entry is removed");
    }

    #[test]
    fn paths_are_keyed_by_path_key() {
        let mut db = LibraryDb::new();
        db.set_favorite(&p("/a/./b/../c.wav"), true);
        assert!(db.is_favorite(&p("/a/c.wav")));
    }

    #[test]
    fn toggle_favorite_group_semantics() {
        let mut db = LibraryDb::new();
        let all = [p("/a.wav"), p("/b.wav"), p("/c.wav")];
        db.set_favorite(&all[0], true);
        assert!(
            db.toggle_favorite(&all),
            "mixed selection: all become favourite"
        );
        assert!(all.iter().all(|x| db.is_favorite(x)));
        assert!(db.toggle_favorite(&all), "all favourite: all cleared");
        assert!(all.iter().all(|x| !db.is_favorite(x)));
        assert_eq!(db, LibraryDb::new());
        assert!(!db.toggle_favorite(&[]));
    }

    #[test]
    fn toggle_keeps_tags_when_clearing() {
        let mut db = LibraryDb::new();
        db.add_tag(&p("/a.wav"), &t("x"));
        db.toggle_favorite(&[p("/a.wav")]);
        db.toggle_favorite(&[p("/a.wav")]);
        assert_eq!(db.tags_of_names(&p("/a.wav")), ["x"]);
    }

    #[test]
    fn add_tag_dedupes_ignoring_case_and_keeps_order() {
        let mut db = LibraryDb::new();
        let f = p("/a.wav");
        assert!(db.add_tag(&f, &t("snare")));
        assert!(db.add_tag(&f, &t("808")));
        assert!(db.add_tag(&f, &t("Kick")));
        assert!(!db.add_tag(&f, &t("KICK")));
        assert_eq!(db.tags_of_names(&f), ["808", "Kick", "snare"]);
    }

    #[test]
    fn remove_tag_prunes_empty_entries() {
        let mut db = LibraryDb::new();
        let f = p("/a.wav");
        db.add_tag(&f, &t("x"));
        assert!(!db.remove_tag(&f, &t("y")));
        assert!(!db.remove_tag(&p("/none.wav"), &t("x")));
        assert!(db.remove_tag(&f, &t("X")));
        assert_eq!(db, LibraryDb::new());
    }

    #[test]
    fn remove_tag_keeps_favourite_entry() {
        let mut db = LibraryDb::new();
        let f = p("/a.wav");
        db.set_favorite(&f, true);
        db.add_tag(&f, &t("x"));
        db.remove_tag(&f, &t("x"));
        assert!(db.is_favorite(&f));
    }

    #[test]
    fn all_tags_counts_files_in_tag_order() {
        let mut db = LibraryDb::new();
        db.add_tag(&p("/a.wav"), &t("Kick"));
        db.add_tag(&p("/b.wav"), &t("kick"));
        db.add_tag(&p("/b.wav"), &t("808"));
        let tags = db.all_tags();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0], (t("808"), 1));
        assert_eq!(tags[1], (t("kick"), 2));
        assert_eq!(tags[1].0.as_str(), "Kick", "first spelling by key order");
    }

    #[test]
    fn favorites_and_tagged_in_key_order() {
        let mut db = LibraryDb::new();
        db.set_favorite(&p("/b.wav"), true);
        db.set_favorite(&p("/a.wav"), true);
        db.add_tag(&p("/c.wav"), &t("x"));
        db.add_tag(&p("/a.wav"), &t("X"));
        let keys = |v: Vec<PathKey>| v.iter().map(|k| k.as_str().to_string()).collect::<Vec<_>>();
        assert_eq!(keys(db.favorites()), ["/a.wav", "/b.wav"]);
        assert_eq!(keys(db.tagged(&t("x"))), ["/a.wav", "/c.wav"]);
        assert!(db.tagged(&t("none")).is_empty());
    }

    #[test]
    fn rename_tag_moves_and_merges() {
        let mut db = LibraryDb::new();
        db.add_tag(&p("/a.wav"), &t("old"));
        db.add_tag(&p("/b.wav"), &t("old"));
        db.add_tag(&p("/b.wav"), &t("New"));
        db.add_tag(&p("/c.wav"), &t("zzz"));
        assert!(db.rename_tag(&t("OLD"), &t("new")));
        assert_eq!(db.tags_of_names(&p("/a.wav")), ["new"]);
        assert_eq!(
            db.tags_of_names(&p("/b.wav")),
            ["New"],
            "merged, no duplicate"
        );
        assert_eq!(db.tags_of_names(&p("/c.wav")), ["zzz"]);
        assert!(
            !db.rename_tag(&t("old"), &t("other")),
            "nothing carries it any more"
        );
    }

    #[test]
    fn rename_tag_changes_spelling_and_keeps_order() {
        let mut db = LibraryDb::new();
        db.add_tag(&p("/a.wav"), &t("b"));
        db.add_tag(&p("/a.wav"), &t("c"));
        assert!(db.rename_tag(&t("b"), &t("d")));
        assert_eq!(db.tags_of_names(&p("/a.wav")), ["c", "d"]);
        assert!(db.rename_tag(&t("c"), &t("C")), "respelling is a change");
        assert!(!db.rename_tag(&t("C"), &t("C")));
        assert_eq!(db.tags_of_names(&p("/a.wav")), ["C", "d"]);
    }

    #[test]
    fn delete_tag_removes_everywhere_and_prunes() {
        let mut db = LibraryDb::new();
        db.add_tag(&p("/a.wav"), &t("x"));
        db.add_tag(&p("/b.wav"), &t("x"));
        db.add_tag(&p("/b.wav"), &t("y"));
        assert!(db.delete_tag(&t("X")));
        assert!(!db.delete_tag(&t("x")));
        assert_eq!(db.tags_of_names(&p("/b.wav")), ["y"]);
        assert_eq!(db.mark(&p("/a.wav")), Mark::default());
        assert_eq!(db.all_tags().len(), 1);
    }

    #[test]
    fn places_keep_order_and_dedupe_by_key() {
        let mut db = LibraryDb::new();
        assert!(db.add_place(&p("/z")));
        assert!(db.add_place(&p("/a")));
        assert!(!db.add_place(&p("/z/")));
        assert!(!db.add_place(&p("/a/./")));
        assert_eq!(db.places(), [p("/z"), p("/a")]);
        assert!(!db.remove_place(&p("/nope")));
        assert!(db.remove_place(&p("/z/.")));
        assert_eq!(db.places(), [p("/a")]);
    }

    #[test]
    fn decorate_copies_marks_onto_rows() {
        let mut db = LibraryDb::new();
        db.set_favorite(&p("/a.wav"), true);
        db.add_tag(&p("/a.wav"), &t("Kick"));
        db.add_tag(&p("/a.wav"), &t("808"));
        let mut r = row("/a.wav");
        db.decorate(&mut r);
        assert!(r.favorite);
        assert_eq!(r.tags, ["808", "Kick"]);

        let mut other = row("/b.wav");
        other.favorite = true;
        other.tags = vec!["stale".into()];
        db.decorate(&mut other);
        assert!(!other.favorite);
        assert!(other.tags.is_empty());
    }

    fn sample_db() -> LibraryDb {
        let mut db = LibraryDb::new();
        db.add_place(&p("D:/Samples"));
        db.set_favorite(&p("/b.wav"), true);
        db.add_tag(&p("/b.wav"), &t("808"));
        db.add_tag(&p("/a.wav"), &t("Kick"));
        db.add_tag(&p("/a.wav"), &t("hat"));
        db.set_favorite(&p("/c.wav"), true);
        db
    }

    #[test]
    fn json_golden_text() {
        let expected = r#"{
  "version": 2,
  "places": [
    "D:/Samples"
  ],
  "entries": {
    "/a.wav": {
      "tags": [
        "hat",
        "Kick"
      ]
    },
    "/b.wav": {
      "favorite": true,
      "tags": [
        "808"
      ]
    },
    "/c.wav": {
      "favorite": true
    }
  }
}"#;
        assert_eq!(sample_db().to_json().unwrap(), expected);
    }

    #[test]
    fn empty_db_json() {
        assert_eq!(
            LibraryDb::new().to_json().unwrap(),
            "{\n  \"version\": 2,\n  \"places\": [],\n  \"entries\": {}\n}"
        );
    }

    #[test]
    fn json_round_trip_is_lossless() {
        let db = sample_db();
        let back = LibraryDb::from_json(&db.to_json().unwrap()).unwrap();
        assert_eq!(back, db);
        assert_eq!(back.to_json().unwrap(), db.to_json().unwrap());
    }

    #[test]
    fn from_json_missing_version_is_corrupt() {
        let err = LibraryDb::from_json(r#"{"places":[],"entries":{}}"#).unwrap_err();
        assert!(matches!(err, LibraryError::Corrupt(_)), "{err:?}");
    }

    #[test]
    fn from_json_garbage_is_corrupt() {
        for text in [
            "",
            "nope",
            "[]",
            "{\"version\":\"x\"}",
            "{\"version\":1,\"entries\":3}",
        ] {
            let err = LibraryDb::from_json(text).unwrap_err();
            assert!(matches!(err, LibraryError::Corrupt(_)), "{text}: {err:?}");
        }
    }

    #[test]
    fn move_path_carries_favourite_and_tags_to_the_new_path() {
        let mut db = LibraryDb::new();
        db.add_tag(&p("/lib/Lead.atomikpatch"), &t("bass"));
        db.set_favorite(&p("/lib/Lead.atomikpatch"), true);
        assert!(db.move_path(&p("/lib/Lead.atomikpatch"), &p("/lib/Pad.atomikpatch")));
        assert_eq!(db.mark(&p("/lib/Lead.atomikpatch")), Mark::default());
        assert!(db.is_favorite(&p("/lib/Pad.atomikpatch")));
        assert_eq!(db.tags_of_names(&p("/lib/Pad.atomikpatch")), ["bass"]);
        assert_eq!(db.tagged(&t("bass")).len(), 1);
    }

    #[test]
    fn move_path_merges_with_a_mark_already_on_the_target_and_ignores_unmarked_files() {
        let mut db = LibraryDb::new();
        db.add_tag(&p("/a"), &t("x"));
        db.add_tag(&p("/b"), &t("y"));
        db.set_favorite(&p("/b"), true);
        assert!(db.move_path(&p("/a"), &p("/b")));
        assert_eq!(db.tags_of_names(&p("/b")), ["x", "y"]);
        assert!(db.is_favorite(&p("/b")));
        assert!(!db.move_path(&p("/none"), &p("/c")), "unmarked: no change");
    }

    #[test]
    fn from_json_newer_version() {
        let err = LibraryDb::from_json(r#"{"version":3,"entries":"future shape"}"#).unwrap_err();
        assert!(matches!(err, LibraryError::NewerVersion(3)), "{err:?}");
    }

    #[test]
    fn from_json_skips_invalid_tags_and_empty_entries() {
        let text = r##"{"version":1,"entries":{
            "/a.wav":{"tags":["ok","bad#tag","","Ok","x"]},
            "/b.wav":{"tags":["bad,tag"]},
            "/c.wav":{"favorite":false}
        }}"##;
        let db = LibraryDb::from_json(text).unwrap();
        assert_eq!(db.tags_of_names(&p("/a.wav")), ["ok", "x"]);
        assert_eq!(db.mark(&p("/b.wav")), Mark::default());
        assert_eq!(db.to_json().unwrap().matches("/b.wav").count(), 0);
        assert_eq!(db.to_json().unwrap().matches("/c.wav").count(), 0);
    }

    #[test]
    fn from_json_dedupes_places_and_defaults_missing_fields() {
        let db = LibraryDb::from_json(r#"{"version":1,"places":["/a","/a/","/b"]}"#).unwrap();
        assert_eq!(db.places(), [p("/a"), p("/b")]);
    }

    /// A version 1 file as an earlier release wrote it (the golden of that release).
    const V1_FILE: &str = r#"{
  "version": 1,
  "places": [
    "D:/Samples"
  ],
  "entries": {
    "/a.wav": {
      "tags": [
        "hat",
        "Kick"
      ]
    },
    "/b.wav": {
      "favorite": true,
      "tags": [
        "808"
      ]
    },
    "/c.wav": {
      "favorite": true
    }
  }
}"#;

    #[test]
    fn a_version_1_file_migrates_without_losing_anything() {
        let db = LibraryDb::from_json(V1_FILE).unwrap();
        assert_eq!(db, sample_db(), "same favourites, tags and places");
        assert!(db.lists().is_empty());
        assert_eq!(db.mark(&p("/b.wav")).author, "");
        // Saved again it is a version 2 file with the same content and reads back equal.
        let saved = db.to_json().unwrap();
        assert!(saved.contains("\"version\": 2"));
        assert!(!saved.contains("lists"), "no empty lists key: {saved}");
        assert_eq!(LibraryDb::from_json(&saved).unwrap(), db);
        // The v1 text differs from the v2 text only by the version number.
        assert_eq!(saved.replace("\"version\": 2", "\"version\": 1"), V1_FILE);
    }

    #[test]
    fn author_is_cleaned_and_clearing_prunes() {
        let mut db = LibraryDb::new();
        let f = p("/a.wav");
        assert!(!db.set_author(&f, "   "), "nothing to clear");
        assert!(db.set_author(&f, "  Ada \t Quinn\u{7} "));
        assert_eq!(db.author(&f), "Ada Quinn");
        assert!(!db.set_author(&f, "Ada Quinn"));
        assert!(db.set_author(&f, &"x".repeat(200)));
        assert_eq!(db.author(&f).chars().count(), MAX_AUTHOR_CHARS);
        assert!(db.set_author(&f, ""));
        assert_eq!(db, LibraryDb::new(), "an empty mark is removed");
    }

    #[test]
    fn categories_are_replaced_as_a_set() {
        let mut db = LibraryDb::new();
        let f = p("/a.wav");
        assert!(!db.set_categories(&f, &[]));
        assert!(db.set_categories(&f, &[t("Snare"), t("Drums"), t("snare")]));
        let names: Vec<String> = db
            .mark(&f)
            .categories
            .iter()
            .map(|c| c.as_str().to_string())
            .collect();
        assert_eq!(names, ["Drums", "Snare"]);
        assert!(!db.set_categories(&f, &[t("SNARE"), t("drums")]));
        assert_eq!(db.all_categories().len(), 2);
        assert!(db.set_categories(&f, &[]));
        assert_eq!(db, LibraryDb::new());
    }

    #[test]
    fn author_and_categories_round_trip_and_decorate_rows() {
        let mut db = sample_db();
        db.set_author(&p("/a.wav"), "Ada");
        db.set_categories(&p("/a.wav"), &[t("Percussion")]);
        db.set_categories(&p("/new.wav"), &[t("Pads")]);
        let back = LibraryDb::from_json(&db.to_json().unwrap()).unwrap();
        assert_eq!(back, db);
        let mut r = row("/a.wav");
        back.decorate(&mut r);
        assert_eq!(r.author, "Ada");
        assert_eq!(r.categories, ["Percussion"]);
        let mut other = row("/zzz.wav");
        other.author = "stale".into();
        other.categories = vec!["stale".into()];
        back.decorate(&mut other);
        assert!(other.author.is_empty() && other.categories.is_empty());
    }

    #[test]
    fn lists_are_named_ordered_and_hold_a_file_once() {
        let mut db = LibraryDb::new();
        assert!(db.add_to_list(&t("Drums"), &[p("/b.wav"), p("/a.wav"), p("/b.wav")]));
        assert!(
            !db.add_to_list(&t("DRUMS"), &[p("/a.wav")]),
            "same list, same file"
        );
        assert!(db.create_list(&t("Pads")));
        assert!(!db.create_list(&t("pads")));
        let names: Vec<&str> = db.lists().iter().map(|l| l.name().as_str()).collect();
        assert_eq!(names, ["Drums", "Pads"]);
        let drums = db.list(&t("drums")).unwrap();
        let keys: Vec<&str> = drums.paths().iter().map(PathKey::as_str).collect();
        assert_eq!(keys, ["/b.wav", "/a.wav"], "order of adding");
        assert!(db.in_list(&t("Drums"), &p("/a/../a.wav")));
        assert!(db.remove_from_list(&t("Drums"), &[p("/b.wav")]));
        assert!(!db.remove_from_list(&t("Drums"), &[p("/b.wav")]));
        assert!(!db.remove_from_list(&t("none"), &[p("/b.wav")]));
    }

    #[test]
    fn lists_rename_delete_and_follow_a_moved_file() {
        let mut db = LibraryDb::new();
        db.add_to_list(&t("Drums"), &[p("/a.wav")]);
        db.add_to_list(&t("Pads"), &[p("/a.wav"), p("/b.wav")]);
        assert!(!db.rename_list(&t("Drums"), &t("pads")), "taken");
        assert!(!db.rename_list(&t("none"), &t("x")));
        assert!(db.rename_list(&t("Drums"), &t("Kits")));
        assert!(db.rename_list(&t("Kits"), &t("kits")), "letter case only");
        assert!(!db.rename_list(&t("Kits"), &t("kits")));
        assert_eq!(db.lists()[0].name().as_str(), "kits");
        // A renamed file keeps its places in the lists, merging with a file already there.
        assert!(db.move_path(&p("/a.wav"), &p("/c.wav")));
        assert!(db.in_list(&t("kits"), &p("/c.wav")) && !db.in_list(&t("kits"), &p("/a.wav")));
        assert!(db.move_path(&p("/c.wav"), &p("/b.wav")));
        assert_eq!(db.list(&t("Pads")).unwrap().paths().len(), 1);
        assert!(db.delete_list(&t("PADS")));
        assert!(!db.delete_list(&t("PADS")));
        assert_eq!(db.lists().len(), 1);
    }

    #[test]
    fn lists_round_trip_and_bad_list_names_are_skipped() {
        let mut db = sample_db();
        db.add_to_list(&t("Drums"), &[p("/a.wav"), p("/b.wav")]);
        db.create_list(&t("Empty"));
        let back = LibraryDb::from_json(&db.to_json().unwrap()).unwrap();
        assert_eq!(back, db);
        let text = r##"{"version":2,"lists":[
            {"name":"ok","paths":["/a"]},{"name":"bad#name","paths":["/z"]},
            {"name":"OK","paths":["/b"]},{"name":""}
        ]}"##;
        let db = LibraryDb::from_json(text).unwrap();
        assert_eq!(db.lists().len(), 1, "duplicates merge, bad names go");
        assert_eq!(db.lists()[0].paths().len(), 2);
    }

    #[test]
    fn move_path_carries_author_and_categories() {
        let mut db = LibraryDb::new();
        db.set_author(&p("/a"), "Ada");
        db.set_categories(&p("/a"), &[t("x")]);
        db.set_author(&p("/b"), "Bob");
        db.set_categories(&p("/b"), &[t("y")]);
        assert!(db.move_path(&p("/a"), &p("/b")));
        let m = db.mark(&p("/b"));
        assert_eq!(m.author, "Bob", "the target keeps its own author");
        assert_eq!(m.categories.len(), 2);
        assert!(db.move_path(&p("/b"), &p("/c")));
        assert_eq!(db.author(&p("/c")), "Bob");
    }

    #[test]
    fn a_future_version_is_refused_not_read() {
        // The app keeps such a file untouched (it never saves over a newer version).
        let text = r#"{"version":3,"places":["/a"],"lists":"new shape"}"#;
        assert!(matches!(
            LibraryDb::from_json(text),
            Err(LibraryError::NewerVersion(3))
        ));
    }
}
