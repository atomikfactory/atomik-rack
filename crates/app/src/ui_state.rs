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
//!   "show_console_devices": true, "show_console_links": false, "show_tempo_lane": false,
//!   "show_marker_strip": true, "show_marker_list": false,
//!   "show_rack_navigator": true,
//!   "track_zoom": 1.0, "piano_roll_key_height": 12.0,
//!   "piano_roll_snap_scale": false, "piano_roll_fold": "off",
//!   "console_notice_seen": false,
//!   "windows": { "rack": { "detached": true, "geom": { "x": 40, "y": 60, "width": 1200.0,
//!                "height": 700.0, "maximised": false, "monitor": "DISPLAY1" } } } }
//! ```

use crate::record::InputQuantize;
use crate::windows::WindowsState;
use atomik_ui::FileBrowserView;
use atomik_ui::arrangement::clamp_track_zoom;
use atomik_ui::key_rows::Fold;
use atomik_ui::piano_roll::{KEY_HEIGHT, MAX_KEY_HEIGHT, MIN_KEY_HEIGHT};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The UI state file name.
pub const UI_STATE_FILE: &str = "ui-state.json";
/// Default share of the content height given to the rack.
pub const DEFAULT_RACK_SPLIT: f64 = 0.5;
/// Default share of the file browser panel's height given to its tree.
pub const DEFAULT_TREE_FRACTION: f64 = 0.4;

/// Window state (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// The arrangement shows the tempo lane.
    pub show_tempo_lane: bool,
    /// The arrangement shows the marker strip under its ruler. An absent key in the file means
    /// shown.
    pub show_marker_strip: bool,
    /// The arrangement shows the marker list at its right edge.
    pub show_marker_list: bool,
    /// The rack navigator (the overview strip at the right of the rack) shows. An absent key in
    /// the file means shown.
    pub show_rack_navigator: bool,
    /// The arrangement's vertical zoom (1 is the default track height; see
    /// `atomik_ui::arrangement::clamp_track_zoom`). An absent key means 1.
    pub track_zoom: f64,
    /// The piano roll's key row height in pixels (`6..=48`). An absent key means 12.
    pub piano_roll_key_height: f64,
    /// The piano roll snaps drawn notes, drags and transposes to the scale of the clip. An
    /// absent key means off.
    pub piano_roll_snap_scale: bool,
    /// Which keys the piano roll shows (`"off"`, `"scale"` or `"used"`). An absent key means
    /// every key.
    pub piano_roll_fold: Fold,
    /// The one-time notice that the console devices are shown now was given.
    pub console_notice_seen: bool,
    /// The file browser panel shows on the left.
    pub file_browser_visible: bool,
    /// The width the user gave the file browser column.
    pub file_browser_width: f64,
    /// The share of the panel's height given to its folder tree (the rest is the file list).
    pub file_browser_tree_fraction: f64,
    /// Selecting a file previews it at once. The sub-folder search is not kept.
    pub file_browser_auto_preview: bool,
    /// The folder the file list showed last.
    pub file_browser_folder: Option<PathBuf>,
    /// Which panes live in their own window and where every window was. An absent key means
    /// everything attached.
    pub windows: WindowsState,
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
            show_tempo_lane: false,
            show_marker_strip: true,
            show_marker_list: false,
            show_rack_navigator: true,
            track_zoom: 1.0,
            piano_roll_key_height: KEY_HEIGHT,
            piano_roll_snap_scale: false,
            piano_roll_fold: Fold::Off,
            console_notice_seen: false,
            file_browser_visible: false,
            file_browser_width: FileBrowserView::DEFAULT_WIDTH,
            file_browser_tree_fraction: DEFAULT_TREE_FRACTION,
            file_browser_auto_preview: true,
            file_browser_folder: None,
            windows: WindowsState::default(),
        }
    }
}

impl UiState {
    /// Reads `path`; the defaults when it is missing or invalid (see [`Self::load_checked`]).
    pub fn load(path: &Path) -> Self {
        Self::load_checked(path).0
    }

    /// Reads `path`. A missing file gives the defaults. A file that cannot be parsed is renamed
    /// `<name>.bad` and gives the defaults and a message to show once.
    pub fn load_checked(path: &Path) -> (Self, Option<String>) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return (Self::default(), None);
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(state) => (state.sanitized(), None),
            Err(why) => {
                let note = match crate::recovery::quarantine(path) {
                    Some(bad) => format!(
                        "window state could not be read ({why}); it was kept as {} and the                          defaults are used",
                        bad.file_name()
                            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                    ),
                    None => {
                        format!("window state could not be read ({why}); the defaults are used")
                    }
                };
                (Self::default(), Some(note))
            }
        }
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

    /// The split clamped into 0.05..=0.95 (0.5 when not finite), the file browser width to at
    /// least its minimum and its tree share into 0.05..=0.95.
    pub fn sanitized(self) -> Self {
        let rack_split = if self.rack_split.is_finite() {
            self.rack_split.clamp(0.05, 0.95)
        } else {
            DEFAULT_RACK_SPLIT
        };
        let file_browser_width = if self.file_browser_width.is_finite() {
            self.file_browser_width.max(FileBrowserView::MIN_WIDTH)
        } else {
            FileBrowserView::DEFAULT_WIDTH
        };
        let file_browser_tree_fraction = if self.file_browser_tree_fraction.is_finite() {
            self.file_browser_tree_fraction.clamp(0.05, 0.95)
        } else {
            DEFAULT_TREE_FRACTION
        };
        let track_zoom = clamp_track_zoom(self.track_zoom);
        let piano_roll_key_height = if self.piano_roll_key_height.is_finite() {
            self.piano_roll_key_height
                .clamp(MIN_KEY_HEIGHT, MAX_KEY_HEIGHT)
        } else {
            KEY_HEIGHT
        };
        Self {
            rack_split,
            file_browser_width,
            file_browser_tree_fraction,
            track_zoom,
            piano_roll_key_height,
            windows: self.windows.sanitized(),
            ..self
        }
    }
}

/// The file browser library file name.
pub const LIBRARY_FILE: &str = "library.json";

/// The file browser library (favourites, tags, bookmarks) next to a settings file: the
/// `<config dir>/library.json` of the default settings path.
pub fn library_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(LIBRARY_FILE)
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
            show_tempo_lane: true,
            show_marker_strip: false,
            show_marker_list: true,
            show_rack_navigator: false,
            console_notice_seen: true,
            track_zoom: 1.75,
            piano_roll_key_height: 30.0,
            piano_roll_snap_scale: true,
            piano_roll_fold: Fold::Used,
            file_browser_visible: true,
            ..UiState::default()
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
        assert!(!old.show_tempo_lane, "an older file has no tempo lane");
        assert!(
            old.show_marker_strip,
            "an older file shows the marker strip"
        );
        assert!(!old.show_marker_list, "an older file has no marker list");
        assert!(!old.show_song);
        assert!(old.show_rack_navigator, "an older file shows the navigator");
        assert!(
            (old.track_zoom - 1.0).abs() < 1e-12
                && (old.piano_roll_key_height - 12.0).abs() < 1e-12,
            "an older file has the default zooms"
        );
        assert!(
            !old.piano_roll_snap_scale && old.piano_roll_fold == Fold::Off,
            "an older file has no snap to scale and no fold"
        );
        std::fs::write(&path, r#"{"piano_roll_fold": "scale"}"#).unwrap();
        assert_eq!(UiState::load(&path).piano_roll_fold, Fold::Scale);
        std::fs::write(&path, r#"{"piano_roll_fold": "sideways"}"#).unwrap();
        assert_eq!(
            UiState::load(&path),
            UiState::default(),
            "an unknown fold: the defaults"
        );
        std::fs::write(&path, r#"{"track_zoom": 99, "piano_roll_key_height": 1}"#).unwrap();
        let wild = UiState::load(&path);
        assert!(
            (wild.track_zoom - 4.0).abs() < 1e-12
                && (wild.piano_roll_key_height - 6.0).abs() < 1e-12
        );
        std::fs::write(
            &path,
            r#"{"track_zoom": null, "piano_roll_key_height": "x"}"#,
        )
        .unwrap();
        assert_eq!(
            UiState::load(&path),
            UiState::default(),
            "unusable values: the defaults"
        );
        std::fs::write(&path, r#"{"show_console_devices": false}"#).unwrap();
        assert!(!UiState::load(&path).show_console_devices);
        std::fs::write(&path, r#"{"show_rack_navigator": false}"#).unwrap();
        assert!(!UiState::load(&path).show_rack_navigator);
        assert_eq!(
            ui_state_path(Path::new("a/settings.json")),
            Path::new("a").join(UI_STATE_FILE)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_file_browser_keys_default_round_trip_and_are_sanitized() {
        let dir = std::env::temp_dir().join(format!("atomik-ui-state-fb-{}", std::process::id()));
        let path = dir.join(UI_STATE_FILE);
        let _ = std::fs::remove_dir_all(&dir);
        // Absent keys are the defaults (an older file keeps loading).
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"show_song": false}"#).unwrap();
        let old = UiState::load(&path);
        assert!(!old.file_browser_visible);
        assert_eq!(old.file_browser_width, FileBrowserView::DEFAULT_WIDTH);
        assert_eq!(old.file_browser_tree_fraction, 0.4);
        assert!(old.file_browser_auto_preview);
        assert_eq!(old.file_browser_folder, None);
        // They round trip, and the recursive search is not among them.
        let s = UiState {
            file_browser_visible: true,
            file_browser_width: 410.0,
            file_browser_tree_fraction: 0.3,
            file_browser_auto_preview: false,
            file_browser_folder: Some(PathBuf::from("samples/drums")),
            ..UiState::default()
        };
        s.save(&path).unwrap();
        assert_eq!(UiState::load(&path), s);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("recursive"), "{text}");
        // Nonsense values are repaired.
        std::fs::write(
            &path,
            r#"{"file_browser_width": 3, "file_browser_tree_fraction": 7}"#,
        )
        .unwrap();
        let bad = UiState::load(&path);
        assert_eq!(bad.file_browser_width, FileBrowserView::MIN_WIDTH);
        assert_eq!(bad.file_browser_tree_fraction, 0.95);
        std::fs::write(
            &path,
            r#"{"file_browser_width": -1e30, "file_browser_tree_fraction": 0}"#,
        )
        .unwrap();
        let low = UiState::load(&path);
        assert_eq!(low.file_browser_tree_fraction, 0.05);
        assert!(low.file_browser_width >= FileBrowserView::MIN_WIDTH);
        std::fs::remove_dir_all(&dir).ok();
    }
}
