//! The plugin settings the app owns and persists: the on/off switch, the search
//! folders, the scan timeout and the per-plugin disable list. Plugins are **off by default**;
//! nothing in this crate scans, registers or loads while [`PluginSettings::enabled`] is false.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

/// Shortest scan timeout in seconds.
pub const MIN_TIMEOUT_SECS: u32 = 5;
/// Longest scan timeout in seconds.
pub const MAX_TIMEOUT_SECS: u32 = 300;
/// The default scan timeout in seconds.
pub const DEFAULT_TIMEOUT_SECS: u32 = 30;

/// The plugin section of the app settings. Serialises with defaults for every missing field, so
/// an old settings file loads with plugins off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginSettings {
    /// Third-party plugins are enabled (scanning, catalog, loading).
    pub enabled: bool,
    /// Folders the user added to the platform defaults.
    pub extra_paths: Vec<PathBuf>,
    /// Scan timeout per file in seconds (clamped to 5 to 300 when read).
    pub timeout_secs: u32,
    /// Type ids (`clap.<id>`) the user disabled: hidden from the browser, existing devices MISSING.
    pub disabled: BTreeSet<String>,
    /// Run each plugin instance in its own process, so a plugin crash cannot take the app down
    /// (on by default; an old settings file loads with it on).
    pub sandbox: bool,
    /// Plugins whose process ended on its own while in use, by type id, with the reason the
    /// last time. The settings list shows them as CRASHED until the user presses RETRY. An old
    /// settings file has none.
    pub crashed: BTreeMap<String, String>,
}

impl Default for PluginSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            extra_paths: Vec::new(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            disabled: BTreeSet::new(),
            sandbox: true,
            crashed: BTreeMap::new(),
        }
    }
}

impl PluginSettings {
    /// Switches plugins on.
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Switches plugins off.
    pub fn disable_all(&mut self) {
        self.enabled = false;
    }

    /// Sets the timeout, clamped to the allowed range.
    pub fn set_timeout_secs(&mut self, secs: u32) {
        self.timeout_secs = secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
    }

    /// The scan timeout, always within 5 to 300 seconds.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(u64::from(
            self.timeout_secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS),
        ))
    }

    /// Longest crash reason kept, in bytes.
    pub const MAX_CRASH_REASON: usize = 300;

    /// Marks a plugin type CRASHED with `reason` (cut to one line of at most
    /// [`Self::MAX_CRASH_REASON`] bytes).
    pub fn mark_crashed(&mut self, type_id: &str, reason: &str) {
        let mut line = reason.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.len() > Self::MAX_CRASH_REASON {
            let mut end = Self::MAX_CRASH_REASON;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
        }
        self.crashed.insert(type_id.to_string(), line);
    }

    /// Clears the CRASHED mark of a plugin type; true when there was one.
    pub fn clear_crashed(&mut self, type_id: &str) -> bool {
        self.crashed.remove(type_id).is_some()
    }

    /// True when the user disabled this plugin type.
    pub fn is_plugin_disabled(&self, type_id: &str) -> bool {
        self.disabled.contains(type_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_settings_file_loads_with_no_crash_marks_and_the_new_field_round_trips() {
        let old: PluginSettings =
            serde_json::from_str(r#"{"enabled": true, "timeout_secs": 40}"#).unwrap();
        assert!(old.enabled && old.crashed.is_empty() && old.sandbox);
        let mut s = PluginSettings::default();
        s.mark_crashed(
            "clap.a",
            "the plugin process exited with code 3\n  stderr text",
        );
        let back: PluginSettings =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!(
            back.crashed["clap.a"],
            "the plugin process exited with code 3 stderr text"
        );
    }

    #[test]
    fn a_crash_reason_is_one_bounded_line_and_retry_clears_the_mark() {
        let mut s = PluginSettings::default();
        s.mark_crashed("vst3.x", &"é".repeat(400));
        assert!(s.crashed["vst3.x"].len() <= PluginSettings::MAX_CRASH_REASON);
        assert!(s.clear_crashed("vst3.x"));
        assert!(!s.clear_crashed("vst3.x"));
        assert!(s.crashed.is_empty());
    }
}
