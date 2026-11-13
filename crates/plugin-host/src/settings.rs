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

/// How sandboxed plugin instances share child processes.
///
/// Only the sandbox reads it: in process there is no child. A crash or a hang of any plugin in a
/// child takes every instance of that child down, so a wider sharing is a trade of isolation for
/// fewer processes. A group holds at most 16 instances; the 17th starts another child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessMode {
    /// Every instance has a child of its own (the default).
    #[default]
    PerPlugin,
    /// The instances of one plugin file share a child.
    PerFile,
    /// Every sandboxed instance shares one child (per plugin format).
    Shared,
}

impl ProcessMode {
    /// Every mode, in the order a button steps through them.
    pub const ALL: [Self; 3] = [Self::PerPlugin, Self::PerFile, Self::Shared];

    /// The next mode, wrapping around.
    pub fn next(self) -> Self {
        match self {
            Self::PerPlugin => Self::PerFile,
            Self::PerFile => Self::Shared,
            Self::Shared => Self::PerPlugin,
        }
    }
}

/// What one plugin asks for, over the [`ProcessMode`] setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessChoice {
    /// Whatever the setting says.
    #[default]
    Default,
    /// A child of its own, whatever the setting says.
    Own,
    /// The shared child, whatever the setting says.
    Shared,
}

impl ProcessChoice {
    /// The next choice, wrapping around.
    pub fn next(self) -> Self {
        match self {
            Self::Default => Self::Own,
            Self::Own => Self::Shared,
            Self::Shared => Self::Default,
        }
    }
}

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
    /// How sandboxed instances share child processes (one per plugin unless changed). A change
    /// applies to the instances loaded after it.
    #[serde(rename = "plugin_process")]
    pub process: ProcessMode,
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
            process: ProcessMode::PerPlugin,
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
    fn the_process_setting_defaults_to_one_child_per_plugin_and_round_trips() {
        assert_eq!(PluginSettings::default().process, ProcessMode::PerPlugin);
        let old: PluginSettings = serde_json::from_str(r#"{"sandbox": true}"#).unwrap();
        assert_eq!(
            old.process,
            ProcessMode::PerPlugin,
            "an old file keeps today's behaviour"
        );
        for mode in ProcessMode::ALL {
            let s = PluginSettings {
                process: mode,
                ..PluginSettings::default()
            };
            let back: PluginSettings =
                serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
            assert_eq!(back.process, mode);
        }
        let text = serde_json::to_string(&PluginSettings {
            process: ProcessMode::PerFile,
            ..PluginSettings::default()
        })
        .unwrap();
        assert!(text.contains(r#""plugin_process":"per_file""#), "{text}");
    }

    #[test]
    fn the_buttons_step_through_every_mode_and_choice_and_come_back() {
        let mut m = ProcessMode::default();
        for _ in 0..3 {
            m = m.next();
        }
        assert_eq!(m, ProcessMode::PerPlugin);
        let mut c = ProcessChoice::default();
        let seen: Vec<_> = (0..3)
            .map(|_| {
                c = c.next();
                c
            })
            .collect();
        assert_eq!(
            seen,
            [
                ProcessChoice::Own,
                ProcessChoice::Shared,
                ProcessChoice::Default
            ]
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
