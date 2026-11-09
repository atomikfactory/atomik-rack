//! What the program remembers about the check: the answer to the question, when the last request
//! was made, and the version the user chose to skip.
//!
//! The answer lives with the other preferences; the rest lives in a small file of its own,
//! `updates.json`, next to them. Nothing in it identifies the user or the machine: a time and a
//! version number.

use crate::version::Version;
use serde_json::{Map, Value};
use std::io;
use std::path::Path;

/// One day, in seconds: the least time between two automatic requests.
pub const DAY_SECS: u64 = 24 * 60 * 60;

/// The file name next to `settings.json`.
pub const FILE: &str = "updates.json";

/// The answer to "Check for new versions once a day?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Consent {
    /// The question has not been answered: the program makes no request.
    #[default]
    Unasked,
    /// Yes: one request a day, at the start.
    Yes,
    /// No: no automatic request.
    No,
}

impl Consent {
    /// The saved form: `None` for a question not answered.
    pub fn from_saved(saved: Option<bool>) -> Self {
        match saved {
            None => Self::Unasked,
            Some(true) => Self::Yes,
            Some(false) => Self::No,
        }
    }

    /// The form to save.
    pub fn to_saved(self) -> Option<bool> {
        match self {
            Self::Unasked => None,
            Self::Yes => Some(true),
            Self::No => Some(false),
        }
    }
}

/// The remembered facts of the check.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateState {
    /// When the last request was made (seconds since 1970), whatever its outcome.
    pub last_check: Option<u64>,
    /// The version the user chose to skip, as text (`0.4.1`).
    pub skipped: Option<String>,
}

impl UpdateState {
    /// Whether an automatic request may be made at `now`: none was made yet, a day has passed, or
    /// the clock went back before the last one (it would otherwise wait for the clock to catch up).
    pub fn due(&self, now: u64) -> bool {
        match self.last_check {
            None => true,
            Some(last) => now < last || now - last >= DAY_SECS,
        }
    }

    /// Notes that a request is made at `now`.
    pub fn checked(&mut self, now: u64) {
        self.last_check = Some(now);
    }

    /// Chooses to skip `version`: its notice no longer shows by itself (a later version does).
    pub fn skip(&mut self, version: &Version) {
        self.skipped = Some(version.to_string());
    }

    /// Whether the user skipped exactly `version`.
    pub fn skips(&self, version: &Version) -> bool {
        self.skipped
            .as_deref()
            .and_then(|s| Version::parse(s).ok())
            .is_some_and(|s| s == *version)
    }

    /// Reads the file text; a wrongly typed or missing field takes its default.
    pub fn parse(text: &str) -> Self {
        let Ok(Value::Object(object)) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        Self {
            last_check: object.get("last_check").and_then(Value::as_u64),
            skipped: object
                .get("skipped")
                .and_then(Value::as_str)
                .filter(|s| Version::parse(s).is_ok())
                .map(str::to_string),
        }
    }

    /// The file text.
    pub fn to_json(&self) -> String {
        let mut map = Map::new();
        if let Some(last) = self.last_check {
            map.insert("last_check".into(), Value::from(last));
        }
        if let Some(skipped) = &self.skipped {
            map.insert("skipped".into(), Value::String(skipped.clone()));
        }
        // Plain data with string keys: serialization cannot fail.
        let mut text = serde_json::to_string_pretty(&Value::Object(map)).expect("state serializes");
        text.push('\n');
        text
    }

    /// Reads `path`; a missing or unreadable file is the empty state.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }

    /// Writes `path` (through a temporary file beside it, so a crash leaves the old text).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, self.to_json())?;
        std::fs::rename(&temp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    /// A temporary folder that removes itself.
    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("atomik-update-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_saved_answer_round_trips() {
        for consent in [Consent::Unasked, Consent::Yes, Consent::No] {
            assert_eq!(Consent::from_saved(consent.to_saved()), consent);
        }
        assert_eq!(Consent::default(), Consent::Unasked);
        assert_eq!(Consent::Unasked.to_saved(), None);
    }

    #[test]
    fn the_first_request_is_due_and_the_next_waits_a_day() {
        let mut state = UpdateState::default();
        assert!(state.due(1_000_000));
        state.checked(1_000_000);
        assert!(!state.due(1_000_000));
        assert!(!state.due(1_000_000 + DAY_SECS - 1));
        assert!(state.due(1_000_000 + DAY_SECS));
        assert!(state.due(1_000_000 + 10 * DAY_SECS));
    }

    #[test]
    fn a_clock_that_went_back_does_not_block_the_check() {
        let mut state = UpdateState::default();
        state.checked(5_000_000);
        assert!(state.due(4_000_000));
    }

    #[test]
    fn a_skipped_version_is_skipped_but_not_a_later_one() {
        let mut state = UpdateState::default();
        assert!(!state.skips(&v("0.4.1")));
        state.skip(&v("0.4.1"));
        assert!(state.skips(&v("0.4.1")));
        assert!(state.skips(&v("v0.4.1")));
        assert!(!state.skips(&v("0.4.2")));
        assert!(!state.skips(&v("0.4.1-rc.1")));
    }

    #[test]
    fn the_text_round_trips_and_keeps_to_two_fields() {
        let mut state = UpdateState::default();
        state.checked(1_700_000_000);
        state.skip(&v("0.5.0-rc.1"));
        let text = state.to_json();
        assert_eq!(UpdateState::parse(&text), state);
        let value: Value = serde_json::from_str(&text).unwrap();
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["last_check", "skipped"]);
        assert_eq!(UpdateState::default().to_json().trim(), "{}");
    }

    #[test]
    fn a_damaged_file_is_the_empty_state() {
        for text in [
            "",
            "garbage",
            "[]",
            r#"{"last_check":"yesterday","skipped":5}"#,
            r#"{"skipped":"latest"}"#,
        ] {
            assert_eq!(UpdateState::parse(text), UpdateState::default(), "{text:?}");
        }
        assert_eq!(
            UpdateState::parse(r#"{"last_check":12,"extra":true}"#).last_check,
            Some(12)
        );
    }

    #[test]
    fn the_state_is_saved_and_loaded_through_a_file() {
        let dir = Dir::new("state");
        let path = dir.0.join(FILE);
        assert_eq!(UpdateState::load(&path), UpdateState::default());
        let mut state = UpdateState::default();
        state.checked(42);
        state.save(&path).unwrap();
        assert_eq!(UpdateState::load(&path), state);
        assert!(!dir.0.join("updates.json.tmp").exists());
    }
}
