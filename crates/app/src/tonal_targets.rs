//! The saved targets of the tonal balance meter: the targets the user learned from reference
//! tracks and kept, by name, in `tonal-targets.json` next to the library file (the place of
//! `spectrum-reference.json`), so every tonal balance device of every project can offer them.
//!
//! ```text
//! { "version": 1, "next_id": 3, "targets": [
//!     { "id": 1, "name": "Club master", "bands": [ ...31 dB... ],
//!       "tol": [ ...31 dB... ], "region_tol": [ 4 dB... ] }, ... ] }
//! ```
//!
//! A target has an id that is never used again (`next_id` only grows), because a project keeps
//! the id of its target in the device's `hidden.target` param (`10 + id`): a deleted target does
//! not come back as another one. A missing file means no targets. A damaged one is ignored (with
//! a log line) and replaced by the next save; a file from a newer version is left alone and
//! never overwritten.

use atomik_dsp::spectrum::BAND_COUNT;
use atomik_dsp::tonal::{REGIONS, Target};
use std::path::{Path, PathBuf};

/// The file name of the saved targets.
pub const TARGETS_FILE: &str = "tonal-targets.json";
/// The format version this build writes and reads.
const VERSION: u64 = 1;
/// The most targets kept.
pub const MAX_TARGETS: usize = 100;
/// The largest id a target can have: the device's `hidden.target` is `10 + id` up to 1000.
pub const MAX_ID: u32 = 990;
/// The longest name, in characters.
pub const MAX_NAME_CHARS: usize = 40;

/// The file next to a library file.
pub fn targets_path(library_file: &Path) -> PathBuf {
    library_file.with_file_name(TARGETS_FILE)
}

/// A saved target.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    /// Its id, never reused.
    pub id: u32,
    /// Its name.
    pub name: String,
    /// The curve and the ranges.
    pub target: Target,
}

/// All the saved targets.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Store {
    /// The id the next saved target gets.
    pub next_id: u32,
    /// The targets, in the order they were saved.
    pub targets: Vec<Saved>,
}

/// A name as it is kept: trimmed, at most [`MAX_NAME_CHARS`] characters; `None` when blank.
pub fn clean_name(name: &str) -> Option<String> {
    let name: String = name.trim().chars().take(MAX_NAME_CHARS).collect();
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

impl Store {
    /// The target with `id`.
    pub fn get(&self, id: u32) -> Option<&Saved> {
        self.targets.iter().find(|t| t.id == id)
    }

    /// Whether a target other than `except` has this name (case does not matter).
    pub fn has_name(&self, name: &str, except: Option<u32>) -> bool {
        self.targets
            .iter()
            .any(|t| Some(t.id) != except && t.name.eq_ignore_ascii_case(name))
    }

    /// `name`, or `name 2`, `name 3`... when it is taken.
    pub fn unique_name(&self, name: &str, except: Option<u32>) -> String {
        if !self.has_name(name, except) {
            return name.to_string();
        }
        let base: String = name
            .chars()
            .take(MAX_NAME_CHARS.saturating_sub(4))
            .collect();
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|candidate| !self.has_name(candidate, except))
            .unwrap_or_else(|| name.to_string())
    }

    /// Adds a target under `name` (made unique) and returns its id; `None` when the store is
    /// full or the ids ran out.
    pub fn add(&mut self, name: &str, target: Target) -> Option<u32> {
        let name = clean_name(name)?;
        let id = self.next_id.max(1);
        if self.targets.len() >= MAX_TARGETS || id > MAX_ID {
            return None;
        }
        let name = self.unique_name(&name, None);
        self.targets.push(Saved {
            id,
            name,
            target: target.sanitised(),
        });
        self.next_id = id + 1;
        Some(id)
    }

    /// Renames target `id`; false when there is none or the name is blank or taken by another.
    pub fn rename(&mut self, id: u32, name: &str) -> bool {
        let Some(name) = clean_name(name) else {
            return false;
        };
        if self.has_name(&name, Some(id)) {
            return false;
        }
        match self.targets.iter_mut().find(|t| t.id == id) {
            Some(t) => {
                t.name = name;
                true
            }
            None => false,
        }
    }

    /// Removes target `id`; false when there is none. Its id is not given out again.
    pub fn remove(&mut self, id: u32) -> bool {
        let before = self.targets.len();
        self.targets.retain(|t| t.id != id);
        self.targets.len() != before
    }
}

/// Why a file was not used.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The text is not a store this build understands.
    #[error("{0}")]
    Invalid(String),
    /// Written by a newer version.
    #[error("written by a newer version ({0})")]
    Newer(u64),
}

fn numbers(list: &serde_json::Value, key: &str, n: usize) -> Result<Vec<f32>, StoreError> {
    let array = list
        .get(key)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| StoreError::Invalid(format!("no `{key}` list")))?;
    if array.len() != n {
        return Err(StoreError::Invalid(format!(
            "{} values in `{key}` instead of {n}",
            array.len()
        )));
    }
    array
        .iter()
        .map(|v| {
            v.as_f64()
                .filter(|x| x.is_finite())
                .map(|x| x as f32)
                .ok_or_else(|| StoreError::Invalid(format!("a value of `{key}` is not a number")))
        })
        .collect()
}

fn list(values: &[f32]) -> String {
    let v: Vec<String> = values.iter().map(|v| format!("{v:.2}")).collect();
    format!("[{}]", v.join(", "))
}

/// The store in its file form.
pub fn encode(store: &Store) -> String {
    let mut out = format!(
        "{{\n  \"version\": {VERSION},\n  \"next_id\": {},\n  \"targets\": [",
        store.next_id
    );
    for (i, t) in store.targets.iter().enumerate() {
        let name = serde_json::Value::String(t.name.clone());
        out.push_str(&format!(
            "{}\n    {{ \"id\": {}, \"name\": {name}, \"bands\": {}, \"tol\": {}, \"region_tol\": {} }}",
            if i == 0 { "" } else { "," },
            t.id,
            list(&t.target.bands),
            list(&t.target.tol),
            list(&t.target.region_tol),
        ));
    }
    out.push_str("\n  ]\n}\n");
    out
}

/// Reads the file form.
///
/// # Errors
/// [`StoreError::Invalid`] for anything but a version-1 store, [`StoreError::Newer`] for a later
/// version.
pub fn parse(text: &str) -> Result<Store, StoreError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| StoreError::Invalid(e.to_string()))?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| StoreError::Invalid("no version".to_string()))?;
    if version > VERSION {
        return Err(StoreError::Newer(version));
    }
    let array = value
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| StoreError::Invalid("no target list".to_string()))?;
    let mut store = Store::default();
    for item in array {
        let id = item
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .filter(|id| (1..=u64::from(MAX_ID)).contains(id))
            .ok_or_else(|| StoreError::Invalid("a target has no valid id".to_string()))?
            as u32;
        let name = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .and_then(clean_name)
            .ok_or_else(|| StoreError::Invalid("a target has no name".to_string()))?;
        if store.get(id).is_some() {
            return Err(StoreError::Invalid(format!("the id {id} is used twice")));
        }
        let bands: [f32; BAND_COUNT] = numbers(item, "bands", BAND_COUNT)?
            .try_into()
            .map_err(|_| StoreError::Invalid("bands".to_string()))?;
        let tol: [f32; BAND_COUNT] = numbers(item, "tol", BAND_COUNT)?
            .try_into()
            .map_err(|_| StoreError::Invalid("tol".to_string()))?;
        let region_tol: [f32; REGIONS] = numbers(item, "region_tol", REGIONS)?
            .try_into()
            .map_err(|_| StoreError::Invalid("region_tol".to_string()))?;
        store.targets.push(Saved {
            id,
            name,
            target: Target {
                bands,
                tol,
                region_tol,
            }
            .sanitised(),
        });
    }
    let highest = store.targets.iter().map(|t| t.id).max().unwrap_or(0);
    let next = value
        .get("next_id")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
        .min(u64::from(MAX_ID) + 1) as u32;
    store.next_id = next.max(highest + 1);
    Ok(store)
}

/// What [`load`] found.
#[derive(Debug, PartialEq)]
pub enum Loaded {
    /// A store.
    Store(Store),
    /// No file.
    Missing,
    /// A file that cannot be used: the store is empty and, for a newer version, the file must
    /// not be overwritten.
    Unusable {
        /// Why.
        why: String,
        /// A newer version wrote it, or it cannot be read.
        newer: bool,
    },
}

/// Reads the store at `path`.
pub fn load(path: &Path) -> Loaded {
    match std::fs::read_to_string(path) {
        Ok(text) => match parse(&text) {
            Ok(store) => Loaded::Store(store),
            Err(e @ StoreError::Newer(_)) => Loaded::Unusable {
                why: e.to_string(),
                newer: true,
            },
            Err(e) => Loaded::Unusable {
                why: e.to_string(),
                newer: false,
            },
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Missing,
        Err(e) => Loaded::Unusable {
            why: e.to_string(),
            newer: true,
        },
    }
}

/// Writes the store atomically.
///
/// # Errors
/// The I/O error.
pub fn save(path: &Path, store: &Store) -> std::io::Result<()> {
    crate::recovery::atomic_write(path, encode(store).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_dsp::tonal::builtin;

    fn target(n: usize) -> Target {
        builtin(n)
    }

    fn store() -> Store {
        let mut s = Store::default();
        s.add("Club master", target(0)).unwrap();
        s.add("Radio \"edit\"", target(1)).unwrap();
        s
    }

    #[test]
    fn a_store_round_trips_through_its_file_form() {
        let s = store();
        let back = parse(&encode(&s)).unwrap();
        assert_eq!(back.targets.len(), 2);
        assert_eq!(back.next_id, 3);
        assert_eq!(back.targets[1].name, "Radio \"edit\"");
        for (a, b) in s.targets.iter().zip(&back.targets) {
            assert_eq!((a.id, &a.name), (b.id, &b.name));
            for i in 0..BAND_COUNT {
                assert!((a.target.bands[i] - b.target.bands[i]).abs() < 0.006);
                assert!((a.target.tol[i] - b.target.tol[i]).abs() < 0.006);
            }
            assert_eq!(a.target.region_tol, b.target.region_tol);
        }
    }

    #[test]
    fn ids_are_never_reused_and_names_stay_unique() {
        let mut s = store();
        assert!(s.remove(2));
        assert!(!s.remove(2));
        let id = s.add("Third", target(2)).unwrap();
        assert_eq!(id, 3, "the freed id 2 is not given out again");
        // The counter survives a save and load.
        let back = parse(&encode(&s)).unwrap();
        assert_eq!(back.next_id, 4);
        assert_eq!(s.unique_name("club MASTER", None), "club MASTER 2");
        let id = s.add("Club master", target(3)).unwrap();
        assert_eq!(s.get(id).unwrap().name, "Club master 2");
        assert!(s.has_name("CLUB master", None));
        assert!(
            !s.has_name("CLUB master", Some(1)),
            "a target may keep its own name"
        );
    }

    #[test]
    fn rename_refuses_blank_taken_and_unknown() {
        let mut s = store();
        assert!(s.rename(1, "  Warm up  "));
        assert_eq!(s.get(1).unwrap().name, "Warm up");
        assert!(!s.rename(1, "   "));
        assert!(!s.rename(1, "radio \"EDIT\""), "taken by another");
        assert!(s.rename(1, "WARM UP"), "its own name in another case");
        assert!(!s.rename(9, "Nobody"));
        assert_eq!(
            clean_name(&"x".repeat(80)).unwrap().chars().count(),
            MAX_NAME_CHARS
        );
        assert_eq!(clean_name("  "), None);
    }

    #[test]
    fn the_store_is_bounded_in_size_and_in_ids() {
        let mut s = Store::default();
        for i in 0..MAX_TARGETS {
            assert!(s.add(&format!("t{i}"), target(0)).is_some());
        }
        assert!(s.add("one more", target(0)).is_none());
        let mut s = Store {
            next_id: MAX_ID,
            ..Store::default()
        };
        assert_eq!(s.add("last", target(0)), Some(MAX_ID));
        assert!(s.add("none left", target(0)).is_none());
        assert!(s.add("", target(0)).is_none());
    }

    #[test]
    fn only_a_version_one_store_with_whole_targets_is_read() {
        for bad in [
            "",
            "[]",
            "{}",
            "{\"version\":1}",
            "{\"version\":1,\"targets\":[{}]}",
            "{\"version\":1,\"targets\":[{\"id\":0,\"name\":\"x\"}]}",
            "{\"version\":1,\"targets\":[{\"id\":1,\"name\":\"x\",\"bands\":[1],\"tol\":[],\"region_tol\":[]}]}",
        ] {
            assert!(matches!(parse(bad), Err(StoreError::Invalid(_))), "{bad}");
        }
        assert!(matches!(
            parse("{\"version\":2,\"targets\":[]}"),
            Err(StoreError::Newer(2))
        ));
        // A duplicate id.
        let mut s = store();
        s.targets[1].id = 1;
        assert!(matches!(parse(&encode(&s)), Err(StoreError::Invalid(_))));
        // An empty list is fine and the counter starts at 1.
        let e = parse("{\"version\":1,\"targets\":[]}").unwrap();
        assert_eq!((e.targets.len(), e.next_id), (0, 1));
    }

    #[test]
    fn numbers_from_a_file_are_bounded() {
        let mut s = store();
        s.targets[0].target.bands[0] = 900.0;
        s.targets[0].target.tol[0] = -5.0;
        let back = parse(&encode(&s)).unwrap();
        assert!(back.targets[0].target.bands[0] <= 30.0);
        assert!(back.targets[0].target.tol[0] >= 1.0);
    }

    #[test]
    fn the_file_lives_next_to_the_library() {
        let p = targets_path(Path::new("cfg/library.json"));
        assert_eq!(p, Path::new("cfg").join(TARGETS_FILE));
    }

    #[test]
    fn loading_tells_missing_from_damaged_from_newer() {
        let dir = std::env::temp_dir().join(format!("atomik-tonal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(TARGETS_FILE);
        let _ = std::fs::remove_file(&file);
        assert_eq!(load(&file), Loaded::Missing);
        save(&file, &store()).unwrap();
        assert!(matches!(load(&file), Loaded::Store(s) if s.targets.len() == 2));
        assert!(!dir.join("tonal-targets.json.tmp").exists());
        std::fs::write(&file, "not json").unwrap();
        assert!(matches!(load(&file), Loaded::Unusable { newer: false, .. }));
        std::fs::write(&file, "{\"version\":9,\"targets\":[]}").unwrap();
        assert!(matches!(load(&file), Loaded::Unusable { newer: true, .. }));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
