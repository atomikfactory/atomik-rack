//! The user library database: favourites, tags and bookmarked places.

use crate::{FileRow, PathKey, Tag};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The JSON format version written by [`LibraryDb::to_json`].
pub const LIBRARY_VERSION: u32 = 1;

/// The favourite flag and tags of one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mark {
    /// Favourite flag.
    pub favorite: bool,
    /// Tags, ordered by [`Tag`]'s order (case-insensitive).
    pub tags: Vec<Tag>,
}

/// Why a library file could not be read.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    /// Not valid JSON or not the expected shape.
    #[error("the library file is damaged: {0}")]
    Corrupt(String),
    /// Written by a newer version of the app.
    #[error("the library file has version {0}, newer than this app supports")]
    NewerVersion(u32),
}

/// Favourites and tags keyed by [`PathKey`], plus the bookmarked folders shown under Places.
///
/// Entries that end up with no tag and no favourite flag are removed, so the database only
/// holds marked files. Every mutating method returns whether something changed (the app saves
/// only then).
///
/// JSON (version 1):
/// `{"version":1,"places":["D:/Samples"],"entries":{"d:/samples/kick.wav":{"favorite":true,"tags":["808"]}}}`
/// (`favorite` omitted when false, `tags` omitted when empty).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryDb {
    entries: BTreeMap<PathKey, Mark>,
    places: Vec<PathBuf>,
}

impl Mark {
    fn is_empty(&self) -> bool {
        !self.favorite && self.tags.is_empty()
    }

    /// Inserts `tag` in order; false when an equal tag is already present.
    fn insert_tag(&mut self, tag: &Tag) -> bool {
        match self.tags.binary_search(tag) {
            Ok(_) => false,
            Err(at) => {
                self.tags.insert(at, tag.clone());
                true
            }
        }
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

    /// Copies this database's favourite flag and tags (display spelling) onto `row`.
    pub fn decorate(&self, row: &mut FileRow) {
        row.tags.clear();
        match self.entries.get(&PathKey::new(&row.path)) {
            Some(mark) => {
                row.favorite = mark.favorite;
                row.tags
                    .extend(mark.tags.iter().map(|t| t.as_str().to_string()));
            }
            None => row.favorite = false,
        }
    }

    /// Serialises to pretty JSON (deterministic: keys in order).
    pub fn to_json(&self) -> String {
        let out = JsonOut {
            version: LIBRARY_VERSION,
            places: self
                .places
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
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
                        },
                    )
                })
                .collect(),
        };
        serde_json::to_string_pretty(&out).unwrap_or_default()
    }

    /// Parses JSON written by [`Self::to_json`]. Invalid tags inside are skipped, not fatal.
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
        for (key, entry) in raw.entries {
            let mut mark = Mark {
                favorite: entry.favorite,
                tags: Vec::new(),
            };
            for name in &entry.tags {
                if let Ok(tag) = Tag::new(name) {
                    mark.insert_tag(&tag);
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

#[derive(Serialize)]
struct JsonOut<'a> {
    version: u32,
    places: Vec<String>,
    entries: BTreeMap<&'a PathKey, EntryOut<'a>>,
}

#[derive(Serialize)]
struct EntryOut<'a> {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    favorite: bool,
    #[serde(skip_serializing_if = "<[Tag]>::is_empty")]
    tags: &'a [Tag],
}

#[derive(Deserialize)]
struct JsonIn {
    #[serde(default)]
    places: Vec<String>,
    #[serde(default)]
    entries: BTreeMap<PathKey, EntryIn>,
}

#[derive(Deserialize)]
struct EntryIn {
    #[serde(default)]
    favorite: bool,
    #[serde(default)]
    tags: Vec<String>,
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
            favorite: false,
            tags: Vec::new(),
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
        assert_eq!(sample_db().to_json(), expected);
    }

    #[test]
    fn empty_db_json() {
        assert_eq!(
            LibraryDb::new().to_json(),
            "{\n  \"version\": 1,\n  \"places\": [],\n  \"entries\": {}\n}"
        );
    }

    #[test]
    fn json_round_trip_is_lossless() {
        let db = sample_db();
        let back = LibraryDb::from_json(&db.to_json()).unwrap();
        assert_eq!(back, db);
        assert_eq!(back.to_json(), db.to_json());
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
    fn from_json_newer_version() {
        let err = LibraryDb::from_json(r#"{"version":2,"entries":"future shape"}"#).unwrap_err();
        assert!(matches!(err, LibraryError::NewerVersion(2)), "{err:?}");
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
        assert_eq!(db.to_json().matches("/b.wav").count(), 0);
        assert_eq!(db.to_json().matches("/c.wav").count(), 0);
    }

    #[test]
    fn from_json_dedupes_places_and_defaults_missing_fields() {
        let db = LibraryDb::from_json(r#"{"version":1,"places":["/a","/a/","/b"]}"#).unwrap();
        assert_eq!(db.places(), [p("/a"), p("/b")]);
    }
}
