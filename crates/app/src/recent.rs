//! The list of recently opened songs, kept in `recent.json` next to `settings.json`.
//!
//! The newest song is first; the list holds [`MAX_RECENT`] entries and no song twice (two
//! spellings of one path count as one, see [`PathKey`]). Reading is lenient: a missing or
//! unusable file is an empty list, never an error, because the list is a convenience.

use atomik_library::PathKey;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The most songs the list remembers.
pub const MAX_RECENT: usize = 8;

/// The file name next to `settings.json`.
pub const RECENT_FILE: &str = "recent.json";

/// The recent songs, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecentFiles {
    paths: Vec<PathBuf>,
}

impl RecentFiles {
    /// The songs, newest first.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Puts `path` first (moving it up when it was in the list) and drops what no longer fits.
    /// Returns whether the list changed.
    pub fn push(&mut self, path: &Path) -> bool {
        if self.paths.first().is_some_and(|p| p == path) {
            return false;
        }
        let key = PathKey::new(path);
        self.paths.retain(|p| PathKey::new(p) != key);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(MAX_RECENT);
        true
    }

    /// Removes `path` (by [`PathKey`]); whether it was in the list.
    pub fn remove(&mut self, path: &Path) -> bool {
        let key = PathKey::new(path);
        let before = self.paths.len();
        self.paths.retain(|p| PathKey::new(p) != key);
        self.paths.len() != before
    }

    /// Empties the list; whether it held anything.
    pub fn clear(&mut self) -> bool {
        let had = !self.paths.is_empty();
        self.paths.clear();
        had
    }

    /// Removes the songs `exists` says are gone and returns them.
    pub fn prune_missing(&mut self, exists: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
        let (kept, gone): (Vec<PathBuf>, Vec<PathBuf>) =
            self.paths.drain(..).partition(|p| exists(p));
        self.paths = kept;
        gone
    }

    /// Parses `recent.json` leniently: anything that is not a list of paths is ignored, the
    /// list is de-duplicated and cut to [`MAX_RECENT`].
    pub fn from_json(text: &str) -> Self {
        let mut list = Self::default();
        let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(text) else {
            return list;
        };
        let Some(Value::Array(items)) = obj.get("recent") else {
            return list;
        };
        // The file is newest first: pushing from the oldest end keeps that order.
        for item in items.iter().rev() {
            if let Some(p) = item.as_str().filter(|p| !p.is_empty()) {
                list.push(Path::new(p));
            }
        }
        list
    }

    /// The file's text.
    pub fn to_json(&self) -> String {
        let items = self
            .paths
            .iter()
            .map(|p| Value::String(p.to_string_lossy().into_owned()))
            .collect();
        let mut map = Map::new();
        map.insert("recent".into(), Value::Array(items));
        // Plain data with string keys: serialization cannot fail.
        let mut text = serde_json::to_string_pretty(&Value::Object(map)).expect("recent serialize");
        text.push('\n');
        text
    }

    /// Reads `path`; an empty list when it is missing or unusable.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|t| Self::from_json(&t))
            .unwrap_or_default()
    }

    /// Writes the list to `path` atomically.
    ///
    /// # Errors
    /// The I/O error, with the path.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        crate::keymap_store::write_atomic(path, &self.to_json())
    }
}

/// The text a recent song has in the menu: its file name without the extension.
pub fn menu_label(path: &Path) -> String {
    path.file_stem().or_else(|| path.file_name()).map_or_else(
        || path.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The recent list file next to a settings file.
pub fn recent_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(RECENT_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn the_newest_song_is_first_and_a_reopened_one_moves_up() {
        let mut r = RecentFiles::default();
        assert!(r.push(&p("/songs/a.atomik")));
        assert!(r.push(&p("/songs/b.atomik")));
        assert!(r.push(&p("/songs/c.atomik")));
        assert!(r.push(&p("/songs/a.atomik")));
        assert_eq!(
            r.paths(),
            [
                p("/songs/a.atomik"),
                p("/songs/c.atomik"),
                p("/songs/b.atomik")
            ]
        );
        assert!(
            !r.push(&p("/songs/a.atomik")),
            "the first entry again changes nothing"
        );
    }

    #[test]
    fn two_spellings_of_one_path_are_one_song() {
        let mut r = RecentFiles::default();
        r.push(&p("/songs/a.atomik"));
        r.push(&p("/songs/./x/../a.atomik"));
        assert_eq!(r.paths().len(), 1);
        assert_eq!(r.paths()[0], p("/songs/./x/../a.atomik"));
    }

    #[test]
    fn the_list_holds_eight_songs() {
        let mut r = RecentFiles::default();
        for i in 0..12 {
            r.push(&p(&format!("/songs/{i}.atomik")));
        }
        assert_eq!(r.paths().len(), MAX_RECENT);
        assert_eq!(r.paths()[0], p("/songs/11.atomik"));
        assert_eq!(r.paths()[MAX_RECENT - 1], p("/songs/4.atomik"));
    }

    #[test]
    fn missing_songs_are_pruned_and_reported() {
        let mut r = RecentFiles::default();
        for n in ["a", "b", "c"] {
            r.push(&p(&format!("/songs/{n}.atomik")));
        }
        let gone = r.prune_missing(|path| path != Path::new("/songs/b.atomik"));
        assert_eq!(gone, [p("/songs/b.atomik")]);
        assert_eq!(r.paths().len(), 2);
        assert!(r.remove(&p("/songs/a.atomik")));
        assert!(!r.remove(&p("/songs/a.atomik")));
    }

    #[test]
    fn clear_empties_the_list() {
        let mut r = RecentFiles::default();
        assert!(!r.clear());
        r.push(&p("/songs/a.atomik"));
        assert!(r.clear());
        assert!(r.is_empty());
    }

    #[test]
    fn the_file_round_trips_in_order() {
        let mut r = RecentFiles::default();
        for n in ["a", "b", "c"] {
            r.push(&p(&format!("/songs/{n}.atomik")));
        }
        let back = RecentFiles::from_json(&r.to_json());
        assert_eq!(back, r);
    }

    #[test]
    fn a_damaged_file_is_an_empty_list() {
        for text in [
            "",
            "not json",
            "[]",
            "{\"recent\": 3}",
            "{\"recent\": [1, null]}",
        ] {
            assert!(RecentFiles::from_json(text).is_empty(), "{text}");
        }
        let dup = RecentFiles::from_json("{\"recent\": [\"/a.atomik\", \"/a.atomik\", \"\"]}");
        assert_eq!(dup.paths(), [p("/a.atomik")]);
    }

    #[test]
    fn save_and_load_use_the_file() {
        let dir = std::env::temp_dir().join(format!("atomik-recent-{}", std::process::id()));
        let file = dir.join(RECENT_FILE);
        assert!(RecentFiles::load(&file).is_empty(), "missing is empty");
        let mut r = RecentFiles::default();
        r.push(&p("/songs/a.atomik"));
        r.save(&file).unwrap();
        assert_eq!(RecentFiles::load(&file), r);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_menu_label_is_the_file_name_without_extension() {
        assert_eq!(menu_label(&p("/songs/My Song.atomik")), "My Song");
        assert_eq!(
            recent_path(Path::new("/cfg/settings.json")),
            p("/cfg/recent.json")
        );
    }
}
