//! The list of recently opened songs, kept in `recent.json` next to `settings.json`.
//!
//! The newest song is first; the list holds [`MAX_RECENT`] entries and no song twice (two
//! spellings of one path count as one, see [`PathKey`]). Up to [`MAX_PINNED`] songs can be
//! pinned: a pinned song stays at the top of the list, in the order it was pinned, however many
//! songs are opened after it, and Clear Menu does not forget it. Reading is lenient: a missing
//! or unusable file is an empty list, never an error, because the list is a convenience.

use atomik_library::PathKey;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The most songs the list remembers.
pub const MAX_RECENT: usize = 8;

/// The most songs that can be pinned (the rest of the list stays for the newest songs).
pub const MAX_PINNED: usize = 4;

/// The file name next to `settings.json`.
pub const RECENT_FILE: &str = "recent.json";

/// The recent songs, newest first, the pinned ones before them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecentFiles {
    /// The pinned songs first, then the others, newest first.
    paths: Vec<PathBuf>,
    /// How many of `paths` (from the start) are pinned.
    pinned: usize,
}

impl RecentFiles {
    /// The songs: the pinned ones, then the others newest first.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// How many songs at the start of [`Self::paths`] are pinned.
    pub fn pinned_count(&self) -> usize {
        self.pinned
    }

    /// Whether `path` is a pinned song.
    pub fn is_pinned(&self, path: &Path) -> bool {
        let key = PathKey::new(path);
        self.paths[..self.pinned]
            .iter()
            .any(|p| PathKey::new(p) == key)
    }

    /// A list of `paths` (taken as they are, cut to [`MAX_RECENT`]) whose first `pinned` songs
    /// are pinned (at most [`MAX_PINNED`]).
    pub fn from_parts(paths: &[PathBuf], pinned: usize) -> Self {
        let mut paths = paths.to_vec();
        paths.truncate(MAX_RECENT);
        let pinned = pinned.min(MAX_PINNED).min(paths.len());
        Self { paths, pinned }
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Where `path` is in the list (by [`PathKey`]).
    fn position(&self, path: &Path) -> Option<usize> {
        let key = PathKey::new(path);
        self.paths.iter().position(|p| PathKey::new(p) == key)
    }

    /// Puts `path` first among the songs that are not pinned (moving it up when it was in the
    /// list) and drops what no longer fits. A pinned song stays where it is. Returns whether the
    /// list changed.
    pub fn push(&mut self, path: &Path) -> bool {
        match self.position(path) {
            Some(at) if at < self.pinned => {
                let changed = self.paths[at] != path;
                self.paths[at] = path.to_path_buf();
                return changed;
            }
            Some(at) if at == self.pinned && self.paths[at] == path => return false,
            Some(at) => {
                self.paths.remove(at);
            }
            None => {}
        }
        self.paths.insert(self.pinned, path.to_path_buf());
        self.paths.truncate(MAX_RECENT);
        true
    }

    /// Pins `path`, which must be in the list: it moves to the end of the pinned songs. False
    /// when it is not listed, is pinned already, or [`MAX_PINNED`] songs are pinned.
    pub fn pin(&mut self, path: &Path) -> bool {
        let Some(at) = self.position(path) else {
            return false;
        };
        if at < self.pinned || self.pinned >= MAX_PINNED {
            return false;
        }
        let song = self.paths.remove(at);
        self.paths.insert(self.pinned, song);
        self.pinned += 1;
        true
    }

    /// Unpins `path`: it becomes the newest song that is not pinned. False when it was not
    /// pinned.
    pub fn unpin(&mut self, path: &Path) -> bool {
        let Some(at) = self.position(path).filter(|at| *at < self.pinned) else {
            return false;
        };
        let song = self.paths.remove(at);
        self.pinned -= 1;
        self.paths.insert(self.pinned, song);
        true
    }

    /// Removes `path` (by [`PathKey`]), pinned or not; whether it was in the list.
    pub fn remove(&mut self, path: &Path) -> bool {
        let Some(at) = self.position(path) else {
            return false;
        };
        self.paths.remove(at);
        if at < self.pinned {
            self.pinned -= 1;
        }
        true
    }

    /// Forgets the songs that are not pinned; whether it forgot any.
    pub fn clear(&mut self) -> bool {
        let had = self.paths.len() > self.pinned;
        self.paths.truncate(self.pinned);
        had
    }

    /// Removes the songs `exists` says are gone (pinned ones too) and returns them.
    pub fn prune_missing(&mut self, exists: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
        let pinned = self.pinned;
        let mut gone = Vec::new();
        let mut kept = Vec::new();
        let mut kept_pinned = 0;
        for (i, p) in self.paths.drain(..).enumerate() {
            if exists(&p) {
                kept_pinned += usize::from(i < pinned);
                kept.push(p);
            } else {
                gone.push(p);
            }
        }
        self.paths = kept;
        self.pinned = kept_pinned;
        gone
    }

    /// Parses `recent.json` leniently: anything that is not a list of paths is ignored, the
    /// list is de-duplicated and cut to [`MAX_RECENT`]. The optional `pinned` list names the
    /// pinned songs (at most [`MAX_PINNED`]); older files have none.
    pub fn from_json(text: &str) -> Self {
        let mut list = Self::default();
        let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(text) else {
            return list;
        };
        let strings = |name: &str| -> Vec<PathBuf> {
            match obj.get(name) {
                Some(Value::Array(items)) => items
                    .iter()
                    .filter_map(|i| i.as_str().filter(|p| !p.is_empty()))
                    .map(PathBuf::from)
                    .collect(),
                _ => Vec::new(),
            }
        };
        let mut keys: Vec<PathKey> = Vec::new();
        let mut take = |list: &mut Vec<PathBuf>, path: PathBuf, room: usize| {
            let key = PathKey::new(&path);
            if list.len() < room && !keys.contains(&key) {
                keys.push(key);
                list.push(path);
                true
            } else {
                false
            }
        };
        let mut paths = Vec::new();
        let mut pinned = 0;
        for path in strings("pinned") {
            pinned += usize::from(take(&mut paths, path, MAX_PINNED));
        }
        // The file is newest first.
        for path in strings("recent") {
            take(&mut paths, path, MAX_RECENT);
        }
        list.paths = paths;
        list.pinned = pinned;
        list
    }

    /// The file's text. `recent` lists every song, so a build that does not know pins still
    /// reads the list; `pinned` names the pinned ones.
    pub fn to_json(&self) -> String {
        let items = |paths: &[PathBuf]| {
            Value::Array(
                paths
                    .iter()
                    .map(|p| Value::String(p.to_string_lossy().into_owned()))
                    .collect(),
            )
        };
        let mut map = Map::new();
        map.insert("recent".into(), items(&self.paths));
        if self.pinned > 0 {
            map.insert("pinned".into(), items(&self.paths[..self.pinned]));
        }
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

    #[test]
    fn a_pinned_song_stays_on_top_while_others_come_and_go() {
        let mut r = RecentFiles::default();
        for n in ["a", "b", "c"] {
            r.push(&p(&format!("/songs/{n}.atomik")));
        }
        assert!(r.pin(&p("/songs/a.atomik")));
        assert!(!r.pin(&p("/songs/a.atomik")), "pinned already");
        assert!(!r.pin(&p("/songs/zzz.atomik")), "not listed");
        assert_eq!(r.pinned_count(), 1);
        assert_eq!(r.paths()[0], p("/songs/a.atomik"));
        assert!(r.push(&p("/songs/d.atomik")));
        assert_eq!(
            r.paths(),
            [
                p("/songs/a.atomik"),
                p("/songs/d.atomik"),
                p("/songs/c.atomik"),
                p("/songs/b.atomik")
            ]
        );
        // Opening the pinned song again changes nothing.
        assert!(!r.push(&p("/songs/a.atomik")));
        assert_eq!(r.paths()[0], p("/songs/a.atomik"));
        // Twelve more songs push the old ones out, never the pinned one.
        for i in 0..12 {
            r.push(&p(&format!("/songs/n{i}.atomik")));
        }
        assert_eq!(r.paths().len(), MAX_RECENT);
        assert_eq!(r.paths()[0], p("/songs/a.atomik"));
        assert_eq!(r.paths()[1], p("/songs/n11.atomik"));
    }

    #[test]
    fn pins_keep_the_order_they_were_made_in_and_are_capped() {
        let mut r = RecentFiles::default();
        for n in 0..6 {
            r.push(&p(&format!("/songs/{n}.atomik")));
        }
        for n in [1, 3, 0, 2] {
            assert!(r.pin(&p(&format!("/songs/{n}.atomik"))));
        }
        assert_eq!(r.pinned_count(), MAX_PINNED);
        assert!(!r.pin(&p("/songs/4.atomik")), "the pin limit");
        assert_eq!(
            &r.paths()[..MAX_PINNED],
            [
                p("/songs/1.atomik"),
                p("/songs/3.atomik"),
                p("/songs/0.atomik"),
                p("/songs/2.atomik")
            ]
        );
        assert!(r.unpin(&p("/songs/3.atomik")));
        assert!(!r.unpin(&p("/songs/3.atomik")));
        assert_eq!(r.pinned_count(), 3);
        assert_eq!(
            r.paths()[3],
            p("/songs/3.atomik"),
            "an unpinned song is the newest of the rest"
        );
        assert!(r.is_pinned(&p("/songs/1.atomik")) && !r.is_pinned(&p("/songs/3.atomik")));
    }

    #[test]
    fn clear_menu_and_a_missing_file_treat_pins_apart() {
        let mut r = RecentFiles::default();
        for n in ["a", "b", "c"] {
            r.push(&p(&format!("/songs/{n}.atomik")));
        }
        r.pin(&p("/songs/b.atomik"));
        assert!(r.clear());
        assert_eq!(r.paths(), [p("/songs/b.atomik")], "a pinned song stays");
        assert!(!r.clear());
        let gone = r.prune_missing(|_| false);
        assert_eq!(gone, [p("/songs/b.atomik")]);
        assert_eq!(r.pinned_count(), 0, "a pinned song that is gone leaves");
        r.push(&p("/songs/x.atomik"));
        r.pin(&p("/songs/x.atomik"));
        assert!(r.remove(&p("/songs/x.atomik")));
        assert_eq!(r.pinned_count(), 0);
    }

    #[test]
    fn the_pins_round_trip_and_an_older_file_has_none() {
        let mut r = RecentFiles::default();
        for n in ["a", "b", "c"] {
            r.push(&p(&format!("/songs/{n}.atomik")));
        }
        r.pin(&p("/songs/a.atomik"));
        let back = RecentFiles::from_json(&r.to_json());
        assert_eq!(back, r);
        let old = RecentFiles::from_json("{\"recent\": [\"/a.atomik\", \"/b.atomik\"]}");
        assert_eq!(old.pinned_count(), 0);
        // A damaged pinned list is ignored, a duplicate counts once, a pin the list lacks is added.
        let odd = RecentFiles::from_json(
            "{\"recent\": [\"/a.atomik\", \"/b.atomik\"], \"pinned\": [7, \"/b.atomik\", \"/b.atomik\", \"/c.atomik\"]}",
        );
        assert_eq!(odd.pinned_count(), 2);
        assert_eq!(
            odd.paths(),
            [p("/b.atomik"), p("/c.atomik"), p("/a.atomik")]
        );
        let none = RecentFiles::from_json("{\"recent\": [\"/a.atomik\"], \"pinned\": 5}");
        assert_eq!(none.pinned_count(), 0);
        assert_eq!(none.paths(), [p("/a.atomik")]);
    }

    #[test]
    fn from_parts_cuts_the_list_and_the_pins() {
        let paths: Vec<PathBuf> = (0..10).map(|n| p(&format!("/songs/{n}.atomik"))).collect();
        let r = RecentFiles::from_parts(&paths, 9);
        assert_eq!(r.paths().len(), MAX_RECENT);
        assert_eq!(r.pinned_count(), MAX_PINNED);
        assert_eq!(RecentFiles::from_parts(&paths[..2], 3).pinned_count(), 2);
    }
}
