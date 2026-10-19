//! Window state that belongs to the app, not to a project: the rack/arrangement
//! split, whether the arrangement shows, and the recording input quantize; it also holds
//! whether the console shows in the lower pane and whether it is maximised.
//!
//! Saved as `ui-state.json` next to `settings.json` (see [`crate::settings`]) whenever it
//! changes; a missing or unreadable file gives the defaults. Unlike `settings.json` it is
//! written without asking: it holds no device choices.
//!
//! ```json
//! { "rack_split": 0.5, "show_song": true, "input_quantize": "1/16",
//!   "show_console": false, "console_maximised": false,
//!   "show_console_devices": true, "show_console_links": false,
//!   "console_notice_seen": false }
//! ```

use crate::record::InputQuantize;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The UI state file name.
pub const UI_STATE_FILE: &str = "ui-state.json";
/// Default share of the content height given to the rack.
pub const DEFAULT_RACK_SPLIT: f64 = 0.5;

/// Window state (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /// Share of the rack + song height given to the rack (0..1).
    pub rack_split: f64,
    /// The arrangement (or piano roll) area is shown.
    pub show_song: bool,
    /// Input quantize applied to recorded notes.
    pub input_quantize: InputQuantize,
    /// The console shows in the lower pane instead of the arrangement.
    pub show_console: bool,
    /// The console is maximised over the rack.
    pub console_maximised: bool,
    /// The channel and master devices show in the rack. An absent key in
    /// the file means shown.
    pub show_console_devices: bool,
    /// The channel to master links are drawn as cables in the rack.
    pub show_console_links: bool,
    /// The one-time notice that the console devices are shown now was given.
    pub console_notice_seen: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            rack_split: DEFAULT_RACK_SPLIT,
            show_song: true,
            input_quantize: InputQuantize::Off,
            show_console: false,
            console_maximised: false,
            show_console_devices: true,
            show_console_links: false,
            console_notice_seen: false,
        }
    }
}

impl UiState {
    /// Reads `path`; the defaults when it is missing or invalid.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Self>(&t).ok())
            .map(Self::sanitized)
            .unwrap_or_default()
    }

    /// Writes `path` (creating its directory).
    ///
    /// # Errors
    /// The I/O error.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text + "\n")
    }

    /// The split clamped into 0.05..=0.95 (0.5 when not finite).
    pub fn sanitized(self) -> Self {
        let rack_split = if self.rack_split.is_finite() {
            self.rack_split.clamp(0.05, 0.95)
        } else {
            DEFAULT_RACK_SPLIT
        };
        Self { rack_split, ..self }
    }
}

/// The UI state file next to a settings file.
pub fn ui_state_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(UI_STATE_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_bad_files_give_defaults_and_values_round_trip() {
        let dir = std::env::temp_dir().join(format!("atomik-ui-state-{}", std::process::id()));
        let path = dir.join(UI_STATE_FILE);
        let _ = std::fs::remove_file(&path);
        assert_eq!(UiState::load(&path), UiState::default());
        let s = UiState {
            rack_split: 0.3,
            show_song: false,
            input_quantize: InputQuantize::Eighth,
            show_console: true,
            console_maximised: true,
            show_console_devices: false,
            show_console_links: true,
            console_notice_seen: true,
        };
        s.save(&path).unwrap();
        assert_eq!(UiState::load(&path), s);
        std::fs::write(&path, "{ nope").unwrap();
        assert_eq!(UiState::load(&path), UiState::default());
        std::fs::write(&path, r#"{"rack_split": 7}"#).unwrap();
        assert!((UiState::load(&path).rack_split - 0.95).abs() < 1e-12);
        // The console keys: absent means shown (devices) and off (links); a file from an older version
        // keeps its other values.
        std::fs::write(&path, r#"{"rack_split": 0.4, "show_song": false}"#).unwrap();
        let old = UiState::load(&path);
        assert!(old.show_console_devices && !old.show_console_links);
        assert!(!old.show_song);
        std::fs::write(&path, r#"{"show_console_devices": false}"#).unwrap();
        assert!(!UiState::load(&path).show_console_devices);
        assert_eq!(
            ui_state_path(Path::new("a/settings.json")),
            Path::new("a").join(UI_STATE_FILE)
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
