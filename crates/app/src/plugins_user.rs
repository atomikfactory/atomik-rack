//! What the user decides about the plugins they have: favourites, tags, disabled plugins and the
//! quarantine, kept in `plugins_user.json` in the settings folder.
//!
//! Everything is keyed by the plugin's stable id (its type id, `clap.<id>` or `vst3.<id>`, which
//! does not change when the file moves), never by a path, so a rescan after the plugin moved
//! keeps its star, its tags and its marks. The file has a version field; a file that is missing,
//! corrupt or of another version reads as empty.
//!
//! * **Disabled** hides a plugin from the browser and the menus. A project that uses it still
//!   loads it.
//! * **Quarantine**: a plugin that crashed or failed to load [`QUARANTINE_AFTER`] times in a row
//!   is quarantined (hidden, like a disabled one) until the user presses TRY AGAIN. A success
//!   starts the count again. The last failure's reason is kept with the quarantine, and a
//!   project that uses a quarantined plugin does not load it until the user says so.
//! * **Process**: which child process a plugin's instances go in, when it asks for something
//!   other than the setting (see [`atomik_plugin_host::ProcessChoice`]).
//!
//! The store is plain data with no clock and no file access besides [`PluginsUser::load`] and
//! [`PluginsUser::save`], so every rule here is tested without a plugin.

use atomik_device_sdk::PluginRef;
use atomik_library::{MAX_TAG_CHARS, Tag, TagError};
use atomik_plugin_host::ProcessChoice;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The file name inside the settings folder.
pub const FILE: &str = "plugins_user.json";
/// The version written to the file.
pub const VERSION: u32 = 1;
/// How many failures in a row quarantine a plugin.
pub const QUARANTINE_AFTER: u32 = 3;
/// Most tags one plugin takes.
pub const MAX_TAGS: usize = 16;
/// Longest quarantine reason kept, in characters: the manager's tooltip says "Quarantined: "
/// and the reason in 90 characters at most.
pub const MAX_REASON_CHARS: usize = 77;

/// The file next to a settings file.
pub fn path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(FILE)
}

/// The favourites, tags, disabled plugins and quarantine, by plugin id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginsUser {
    favourites: BTreeSet<String>,
    /// Tags of each plugin in the order they were added, never two that match ignoring case.
    tags: BTreeMap<String, Vec<String>>,
    disabled: BTreeSet<String>,
    quarantined: BTreeSet<String>,
    /// Failures in a row of the plugins that have some and are not quarantined yet.
    failures: BTreeMap<String, u32>,
    /// Why each quarantined plugin is quarantined (the last failure that did it).
    reasons: BTreeMap<String, String>,
    /// The plugins that ask for a process of their own or the shared one.
    process: BTreeMap<String, ProcessChoice>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    #[serde(default)]
    favourites: Vec<String>,
    #[serde(default)]
    tags: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    disabled: Vec<String>,
    #[serde(default)]
    quarantined: Vec<String>,
    #[serde(default)]
    failures: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    reasons: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    process: BTreeMap<String, ProcessChoice>,
}

/// `text` as one line of at most [`MAX_REASON_CHARS`] characters, cut at its first `;` (the
/// sandbox's own explanations after it are long) and ended with an ellipsis when it was cut.
pub fn short_reason(text: &str) -> String {
    let first = text.split(';').next().unwrap_or(text);
    let line = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= MAX_REASON_CHARS {
        return line;
    }
    let mut cut: String = line.chars().take(MAX_REASON_CHARS - 1).collect();
    cut.push('\u{2026}');
    cut
}

impl PluginsUser {
    /// Whether nothing is recorded.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    // ------------------------------------------------------------------ favourites

    /// Whether the plugin has a star.
    pub fn is_favourite(&self, id: &str) -> bool {
        self.favourites.contains(id)
    }

    /// Sets or clears the star; true when it changed.
    pub fn set_favourite(&mut self, id: &str, on: bool) -> bool {
        if on {
            self.favourites.insert(id.to_owned())
        } else {
            self.favourites.remove(id)
        }
    }

    // ------------------------------------------------------------------ tags

    /// The tags of a plugin, in the order they were added.
    pub fn tags_of(&self, id: &str) -> &[String] {
        self.tags.get(id).map_or(&[], Vec::as_slice)
    }

    /// Adds a tag; true when it was new. A tag that matches one the plugin has (ignoring case)
    /// is not added twice.
    ///
    /// # Errors
    /// The name is not a valid tag (see [`atomik_library::Tag`]).
    pub fn add_tag(&mut self, id: &str, name: &str) -> Result<bool, TagError> {
        let tag = Tag::new(name)?;
        let list = self.tags.entry(id.to_owned()).or_default();
        let new = !list
            .iter()
            .any(|t| Tag::new(t).is_ok_and(|have| have == tag))
            && list.len() < MAX_TAGS;
        if new {
            list.push(tag.as_str().to_owned());
        }
        if list.is_empty() {
            self.tags.remove(id);
        }
        Ok(new)
    }

    /// Removes a tag (matched ignoring case); true when the plugin had it.
    pub fn remove_tag(&mut self, id: &str, name: &str) -> bool {
        let Ok(tag) = Tag::new(name) else {
            return false;
        };
        let Some(list) = self.tags.get_mut(id) else {
            return false;
        };
        let before = list.len();
        list.retain(|t| !Tag::new(t).is_ok_and(|have| have == tag));
        let removed = list.len() != before;
        if list.is_empty() {
            self.tags.remove(id);
        }
        removed
    }

    /// Whether the plugin has the tag (ignoring case).
    pub fn has_tag(&self, id: &str, name: &str) -> bool {
        Tag::new(name).is_ok_and(|tag| {
            self.tags_of(id)
                .iter()
                .any(|t| Tag::new(t).is_ok_and(|have| have == tag))
        })
    }

    /// Every tag in use with the number of plugins that have it, by name.
    pub fn all_tags(&self) -> Vec<(String, usize)> {
        let mut found: BTreeMap<String, (String, usize)> = BTreeMap::new();
        for list in self.tags.values() {
            for name in list {
                let Ok(tag) = Tag::new(name) else { continue };
                found
                    .entry(tag.folded())
                    .or_insert_with(|| (tag.as_str().to_owned(), 0))
                    .1 += 1;
            }
        }
        found.into_values().collect()
    }

    // ------------------------------------------------------------------ process

    /// What the plugin asks for ([`ProcessChoice::Default`]: whatever the setting says).
    pub fn process_of(&self, id: &str) -> ProcessChoice {
        self.process.get(id).copied().unwrap_or_default()
    }

    /// Sets what the plugin asks for; true when it changed.
    pub fn set_process(&mut self, id: &str, choice: ProcessChoice) -> bool {
        if choice == ProcessChoice::Default {
            self.process.remove(id).is_some()
        } else {
            self.process.insert(id.to_owned(), choice) != Some(choice)
        }
    }

    /// The choices by plugin, for the sandbox (an id that is not a plugin type id is left out).
    pub fn process_choices(&self) -> BTreeMap<PluginRef, ProcessChoice> {
        self.process
            .iter()
            .filter_map(|(id, c)| PluginRef::from_type_id(id).map(|p| (p, *c)))
            .collect()
    }

    // ------------------------------------------------------------------ disable

    /// Whether the user disabled the plugin.
    pub fn is_disabled(&self, id: &str) -> bool {
        self.disabled.contains(id)
    }

    /// Disables or enables a plugin; true when it changed.
    pub fn set_disabled(&mut self, id: &str, on: bool) -> bool {
        if on {
            self.disabled.insert(id.to_owned())
        } else {
            self.disabled.remove(id)
        }
    }

    // ------------------------------------------------------------------ quarantine

    /// Whether the plugin is quarantined.
    pub fn is_quarantined(&self, id: &str) -> bool {
        self.quarantined.contains(id)
    }

    /// The failures in a row the plugin has now.
    pub fn failures(&self, id: &str) -> u32 {
        self.failures.get(id).copied().unwrap_or(0)
    }

    /// Counts one failure (a crash, or a failure to load). Returns true when this one
    /// quarantined the plugin (the caller says so once).
    pub fn record_failure(&mut self, id: &str) -> bool {
        self.record_failure_because(id, "")
    }

    /// Like [`PluginsUser::record_failure`], keeping `reason` (cut by [`short_reason`]) with the
    /// quarantine when this failure is the one that quarantines the plugin.
    pub fn record_failure_because(&mut self, id: &str, reason: &str) -> bool {
        if self.quarantined.contains(id) {
            return false;
        }
        let count = self.failures.entry(id.to_owned()).or_insert(0);
        *count += 1;
        if *count >= QUARANTINE_AFTER {
            self.failures.remove(id);
            self.quarantined.insert(id.to_owned());
            let reason = short_reason(reason);
            if !reason.is_empty() {
                self.reasons.insert(id.to_owned(), reason);
            }
            return true;
        }
        false
    }

    /// Why the plugin is quarantined, when it is and the reason is known (a quarantine from an
    /// older file has none).
    pub fn quarantine_reason(&self, id: &str) -> Option<&str> {
        self.quarantined
            .contains(id)
            .then(|| self.reasons.get(id).map(String::as_str))
            .flatten()
    }

    /// A success: the count of failures in a row starts again. True when there was one.
    pub fn record_success(&mut self, id: &str) -> bool {
        self.failures.remove(id).is_some()
    }

    /// TRY AGAIN: takes the plugin out of quarantine and clears its count; true when it was
    /// quarantined or had failures.
    pub fn try_again(&mut self, id: &str) -> bool {
        self.reasons.remove(id);
        let was = self.quarantined.remove(id);
        self.failures.remove(id).is_some() || was
    }

    /// Whether the browser and the menus hide the plugin (disabled or quarantined).
    pub fn is_hidden(&self, id: &str) -> bool {
        self.is_disabled(id) || self.is_quarantined(id)
    }

    // ------------------------------------------------------------------ the file

    /// The file text.
    pub fn to_json(&self) -> String {
        let stored = Stored {
            version: VERSION,
            favourites: self.favourites.iter().cloned().collect(),
            tags: self.tags.clone(),
            disabled: self.disabled.iter().cloned().collect(),
            quarantined: self.quarantined.iter().cloned().collect(),
            failures: self.failures.clone(),
            reasons: self.reasons.clone(),
            process: self.process.clone(),
        };
        let mut text = serde_json::to_string_pretty(&stored).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Reads the file text: empty when it is not a file of this version. Tags that are not
    /// valid are dropped.
    pub fn from_json(text: &str) -> Self {
        let Ok(stored) = serde_json::from_str::<Stored>(text) else {
            return Self::default();
        };
        if stored.version != VERSION {
            return Self::default();
        }
        let mut user = Self {
            favourites: stored.favourites.into_iter().collect(),
            disabled: stored.disabled.into_iter().collect(),
            quarantined: stored.quarantined.into_iter().collect(),
            failures: stored
                .failures
                .into_iter()
                .filter(|(_, n)| (1..QUARANTINE_AFTER).contains(n))
                .collect(),
            process: stored
                .process
                .into_iter()
                .filter(|(_, c)| *c != ProcessChoice::Default)
                .collect(),
            ..Self::default()
        };
        for (id, reason) in stored.reasons {
            if user.quarantined.contains(&id) {
                user.reasons.insert(id, short_reason(&reason));
            }
        }
        for (id, names) in stored.tags {
            for name in names {
                let _ = user.add_tag(&id, &name);
            }
        }
        user.failures.retain(|id, _| !user.quarantined.contains(id));
        user
    }

    /// Reads the file at `path`: empty when it is missing or unusable.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|text| Self::from_json(&text))
            .unwrap_or_default()
    }

    /// Writes the file at `path`.
    ///
    /// # Errors
    /// The file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        crate::keymap_store::write_atomic(path, &self.to_json())
    }
}

/// The longest tag, for the field that takes one.
pub const TAG_CHARS: usize = MAX_TAG_CHARS;

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "clap.example.synth";
    const B: &str = "vst3.0123456789ABCDEF0123456789ABCDEF";

    fn busy() -> PluginsUser {
        let mut u = PluginsUser::default();
        u.set_favourite(A, true);
        u.add_tag(A, "Pads").unwrap();
        u.add_tag(A, "warm").unwrap();
        u.add_tag(B, "pads").unwrap();
        u.set_disabled(B, true);
        for _ in 0..2 {
            u.record_failure(B);
        }
        u
    }

    #[test]
    fn the_store_round_trips_through_its_text() {
        let u = busy();
        let back = PluginsUser::from_json(&u.to_json());
        assert_eq!(back, u);
        assert_eq!(back.to_json(), u.to_json(), "the same text every time");
        assert!(back.is_favourite(A) && back.is_disabled(B));
        assert_eq!(back.tags_of(A), ["Pads", "warm"]);
        assert_eq!(back.failures(B), 2);
    }

    #[test]
    fn a_file_on_disk_round_trips_and_a_missing_one_is_empty() {
        let dir = std::env::temp_dir().join(format!("atomik-plugins-user-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(FILE);
        assert!(PluginsUser::load(&file).is_empty(), "missing is empty");
        busy().save(&file).unwrap();
        assert_eq!(PluginsUser::load(&file), busy());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_or_foreign_file_reads_as_empty() {
        for text in [
            "",
            "{ not json",
            "[1, 2]",
            r#"{"favourites": ["clap.a"]}"#,
            r#"{"version": 99, "favourites": ["clap.a"]}"#,
        ] {
            assert!(PluginsUser::from_json(text).is_empty(), "{text:?}");
        }
        // Parts that are missing are empty, bad tags and counts are dropped.
        let u = PluginsUser::from_json(
            r##"{"version": 1, "favourites": ["clap.a"],
                 "tags": {"clap.a": ["ok", "", "a#b"]},
                 "failures": {"clap.a": 7, "clap.b": 2}}"##,
        );
        assert!(u.is_favourite("clap.a"));
        assert_eq!(u.tags_of("clap.a"), ["ok"]);
        assert_eq!(
            u.failures("clap.a"),
            0,
            "a count past the limit is not kept"
        );
        assert_eq!(u.failures("clap.b"), 2);
    }

    #[test]
    fn the_marks_belong_to_the_id_not_to_a_path() {
        // The store knows no path: a plugin that moved is the same id and keeps everything.
        let u = busy();
        assert!(u.is_favourite(A));
        assert_eq!(u.tags_of(A).len(), 2);
        assert!(!u.is_favourite("clap.example.other"));
    }

    #[test]
    fn three_failures_in_a_row_quarantine_a_plugin() {
        let mut u = PluginsUser::default();
        assert!(!u.record_failure(A));
        assert!(!u.record_failure(A));
        assert_eq!(u.failures(A), 2);
        assert!(u.record_failure(A), "the third one quarantines it");
        assert!(u.is_quarantined(A) && u.is_hidden(A));
        assert_eq!(u.failures(A), 0);
        assert!(
            !u.record_failure(A),
            "already quarantined: nothing new to say"
        );
        assert!(!u.is_quarantined(B), "only that plugin");
    }

    #[test]
    fn the_failure_that_quarantines_a_plugin_leaves_its_reason() {
        let mut u = PluginsUser::default();
        assert!(!u.record_failure_because(A, "the first"));
        assert!(!u.record_failure_because(A, "the second"));
        assert_eq!(u.quarantine_reason(A), None, "not quarantined yet");
        assert!(u.record_failure_because(
            A,
            "the plugin process exited with code 3 (0x3); this process hosted a group of 2"
        ));
        assert_eq!(
            u.quarantine_reason(A),
            Some("the plugin process exited with code 3 (0x3)"),
            "cut at the first semicolon"
        );
        let back = PluginsUser::from_json(&u.to_json());
        assert_eq!(back.quarantine_reason(A), u.quarantine_reason(A));
        assert!(u.try_again(A));
        assert_eq!(u.quarantine_reason(A), None, "TRY AGAIN forgets it");
    }

    #[test]
    fn a_reason_is_one_short_line() {
        assert_eq!(short_reason("  a \n b  "), "a b");
        let long = "x".repeat(200);
        let cut = short_reason(&long);
        assert_eq!(cut.chars().count(), MAX_REASON_CHARS);
        assert!(cut.ends_with('\u{2026}'));
        // "Quarantined: " and the reason fit a tooltip.
        assert!("Quarantined: ".len() + cut.len() <= 90 + 2, "{}", cut.len());
    }

    #[test]
    fn a_quarantine_from_an_older_file_has_no_reason_and_still_loads() {
        let u = PluginsUser::from_json(r#"{"version": 1, "quarantined": ["clap.a"]}"#);
        assert!(u.is_quarantined("clap.a"));
        assert_eq!(u.quarantine_reason("clap.a"), None);
        // A reason for a plugin that is not quarantined is dropped.
        let v = PluginsUser::from_json(
            r#"{"version": 1, "reasons": {"clap.b": "x"}, "quarantined": ["clap.a"]}"#,
        );
        assert_eq!(v.quarantine_reason("clap.b"), None);
    }

    #[test]
    fn what_a_plugin_asks_of_the_process_is_kept_and_default_is_not_stored() {
        let mut u = PluginsUser::default();
        assert_eq!(u.process_of(A), ProcessChoice::Default);
        assert!(u.set_process(A, ProcessChoice::Shared));
        assert!(!u.set_process(A, ProcessChoice::Shared), "no change");
        assert!(u.set_process(B, ProcessChoice::Own));
        let back = PluginsUser::from_json(&u.to_json());
        assert_eq!(back.process_of(A), ProcessChoice::Shared);
        assert_eq!(back.process_of(B), ProcessChoice::Own);
        assert_eq!(back.process_choices().len(), 2);
        assert!(u.set_process(A, ProcessChoice::Default));
        assert!(
            !u.to_json().contains(A),
            "a plugin that follows the setting is not in the file"
        );
        assert!(
            !PluginsUser::default().to_json().contains("process"),
            "an empty store writes the same text as before"
        );
    }

    #[test]
    fn a_success_starts_the_count_again() {
        let mut u = PluginsUser::default();
        u.record_failure(A);
        u.record_failure(A);
        assert!(u.record_success(A));
        assert!(!u.record_success(A), "nothing left to reset");
        u.record_failure(A);
        u.record_failure(A);
        assert!(!u.is_quarantined(A), "two again, not three in a row");
        assert!(u.record_failure(A));
        assert!(u.is_quarantined(A));
    }

    #[test]
    fn try_again_clears_the_quarantine_and_the_count() {
        let mut u = PluginsUser::default();
        for _ in 0..QUARANTINE_AFTER {
            u.record_failure(A);
        }
        assert!(u.try_again(A));
        assert!(!u.is_quarantined(A) && !u.is_hidden(A));
        assert_eq!(u.failures(A), 0);
        assert!(!u.try_again(A), "nothing to clear");
        // One more failure counts as the first.
        assert!(!u.record_failure(A));
        assert_eq!(u.failures(A), 1);
    }

    #[test]
    fn disabled_and_quarantined_plugins_are_hidden_and_enabling_shows_them_again() {
        let mut u = PluginsUser::default();
        assert!(u.set_disabled(A, true));
        assert!(!u.set_disabled(A, true), "no change the second time");
        assert!(u.is_hidden(A));
        assert!(u.set_disabled(A, false));
        assert!(!u.is_hidden(A));
    }

    #[test]
    fn tags_are_case_insensitive_bounded_and_counted() {
        let mut u = PluginsUser::default();
        assert_eq!(u.add_tag(A, "  Warm   pad "), Ok(true));
        assert_eq!(u.add_tag(A, "warm PAD"), Ok(false), "the same tag");
        assert_eq!(u.tags_of(A), ["Warm pad"], "the first spelling stays");
        assert!(u.has_tag(A, "WARM pad"));
        assert_eq!(u.add_tag(A, ""), Err(TagError::Empty));
        assert_eq!(u.add_tag(A, "a#b"), Err(TagError::InvalidChar));
        assert_eq!(
            u.add_tag(A, &"x".repeat(TAG_CHARS + 1)),
            Err(TagError::TooLong)
        );
        for n in 0..MAX_TAGS + 4 {
            let _ = u.add_tag(A, &format!("t{n}"));
        }
        assert_eq!(u.tags_of(A).len(), MAX_TAGS);
        u.add_tag(B, "warm pad").unwrap();
        let all = u.all_tags();
        assert_eq!(all[0], ("t0".to_owned(), 1));
        assert!(all.contains(&("Warm pad".to_owned(), 2)));
        assert!(u.remove_tag(A, "WARM PAD"));
        assert!(!u.remove_tag(A, "warm pad"));
        assert!(u.remove_tag(B, "warm pad"));
        assert!(u.tags_of(B).is_empty());
    }
}
