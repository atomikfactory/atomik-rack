//! The files the keyboard and workflow settings live in:
//! `keymap.json` (the user's changes over a profile, see [`atomik_ui::keymap::UserKeymap`]) and
//! `workflow.json` (the workflow preferences), both next to `settings.json`.
//!
//! - Loading is lenient. A missing file is the defaults. `keymap.json` that is not a JSON object
//!   is renamed `keymap.json.bad` (the same for `workflow.json`) and the defaults are used with a
//!   status message; entries of the wrong shape inside a readable file are skipped.
//! - Saving is atomic (a temporary file next to the target, then a rename) and canonical (sorted,
//!   no timestamps), so a crash never leaves half a file and an unchanged setting writes
//!   identical bytes.
//! - `workflow.json` keeps fields this build does not know (a newer build or a later step of the
//!   phase, such as the rack layout flags), so saving never drops them.

use crate::import_warp::WarpImports;
use crate::master_keyboard::MasterKeyboard;
use crate::record_over::{KeepEvents, RecordMode};
use atomik_model::ThinLevel;
use atomik_ui::ThemeChoice;
use atomik_ui::keymap::UserKeymap;
use atomik_ui::widget::KnobRange;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The user keymap file name.
pub const KEYMAP_FILE: &str = "keymap.json";
/// The workflow preferences file name.
pub const WORKFLOW_FILE: &str = "workflow.json";

/// The keymap file next to a settings file.
pub fn keymap_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(KEYMAP_FILE)
}

/// The workflow file next to a settings file.
pub fn workflow_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name(WORKFLOW_FILE)
}

/// A loaded value and what to tell the user about it, if anything.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded<T> {
    /// The value (the defaults when the file was missing or unusable).
    pub value: T,
    /// A message for the status bar.
    pub status: Option<String>,
}

/// Workflow preferences: what is saved in `workflow.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowPrefs {
    /// Creating a device wires it up (device, channel, song track, keyboard target) in one step
    /// (default on).
    pub auto_route: bool,
    /// The device browser stays open after creating a device (default off).
    pub create_keeps_browser_open: bool,
    /// Views follow the playhead while playing.
    pub follow_song: bool,
    /// The arrangement shows the automation lane of the parameter being touched (default off).
    pub follow_selection: bool,
    /// Buttons show a short help text after the pointer rests on them (default on).
    pub show_tooltips: bool,
    /// The rack is shown (the F6 layout flag; view state, not part of the project).
    pub rack_visible: bool,
    /// The one-time notice of the keys that moved with the standard keymap was shown.
    pub keys_notice_seen: bool,
    /// The library folder the user chose (`None`: the default, see
    /// [`crate::settings::default_library_dir`]).
    pub library_folder: Option<PathBuf>,
    /// Minutes between autosave snapshots of a changed project (0: off, default 5).
    pub autosave_minutes: u32,
    /// Start with the song that was open last time, when no project is named on the command
    /// line (default off).
    pub open_last_project: bool,
    /// Stop returns the playhead to where playback started; a second Stop goes to the start
    /// (default off).
    pub return_to_start_on_stop: bool,
    /// What a finished MIDI take does with the clips already on its lane (default Replace).
    pub record_mode: RecordMode,
    /// What a note edit does with notes that start past their clip's end (default Keep).
    pub keep_events: KeepEvents,
    /// Which imported audio files are warped to the song tempo (default Loops only).
    pub warp_imports: WarpImports,
    /// How the master keyboard picks its track (default Standard: it follows the selection).
    pub master_keyboard: MasterKeyboard,
    /// The colour theme: dark, light, or whichever the operating system uses (default dark).
    pub theme: ThemeChoice,
    /// The size of every window's content in percent: 75 to 200 in steps of 25 (default 100).
    pub app_zoom: u16,
    /// How far a knob or fader has to be dragged to sweep its range (default Normal).
    pub knob_range: KnobRange,
    /// Hovering any control shows its name and value (default off: only an automated or driven
    /// control says so).
    pub value_tooltips: bool,
    /// How hard recorded and drawn automation is thinned (default Normal, 0.5 % of the range).
    pub automation_thinning: ThinLevel,
    /// Fields this build does not know, kept as they were.
    pub extra: BTreeMap<String, Value>,
}

/// The application zoom steps, in percent.
pub const ZOOM_STEPS: [u16; 6] = [75, 100, 125, 150, 175, 200];

/// `percent` as the nearest zoom step (the saved value of a hand-edited file may be anything).
pub fn nearest_zoom(percent: u64) -> u16 {
    ZOOM_STEPS
        .into_iter()
        .min_by_key(|s| u64::from(*s).abs_diff(percent))
        .unwrap_or(100)
}

impl Default for WorkflowPrefs {
    fn default() -> Self {
        Self {
            auto_route: true,
            create_keeps_browser_open: false,
            follow_song: false,
            follow_selection: false,
            show_tooltips: true,
            rack_visible: true,
            keys_notice_seen: false,
            library_folder: None,
            autosave_minutes: 5,
            open_last_project: false,
            return_to_start_on_stop: false,
            record_mode: RecordMode::default(),
            keep_events: KeepEvents::default(),
            warp_imports: WarpImports::default(),
            master_keyboard: MasterKeyboard::default(),
            theme: ThemeChoice::default(),
            app_zoom: 100,
            knob_range: KnobRange::default(),
            value_tooltips: false,
            automation_thinning: ThinLevel::default(),
            extra: BTreeMap::new(),
        }
    }
}

impl WorkflowPrefs {
    /// Parses `workflow.json` leniently: a missing or wrongly typed field takes its default,
    /// unknown fields are kept. An error means the text is not a JSON object.
    pub fn from_json(text: &str) -> Result<WorkflowPrefs, String> {
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let obj = value
            .as_object()
            .ok_or_else(|| "the workflow file must be a JSON object".to_string())?;
        let defaults = WorkflowPrefs::default();
        let flag =
            |name: &str, default: bool| obj.get(name).and_then(Value::as_bool).unwrap_or(default);
        Ok(WorkflowPrefs {
            auto_route: flag("auto_route", defaults.auto_route),
            create_keeps_browser_open: flag(
                "create_keeps_browser_open",
                defaults.create_keeps_browser_open,
            ),
            follow_song: flag("follow_song", defaults.follow_song),
            follow_selection: flag("follow_selection", defaults.follow_selection),
            show_tooltips: flag("show_tooltips", defaults.show_tooltips),
            rack_visible: flag("rack_visible", defaults.rack_visible),
            keys_notice_seen: flag("keys_notice_seen", defaults.keys_notice_seen),
            library_folder: obj
                .get("library_folder")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
            autosave_minutes: obj
                .get("autosave_minutes")
                .and_then(Value::as_u64)
                .map_or(defaults.autosave_minutes, |m| m.min(240) as u32),
            open_last_project: flag("open_last_project", defaults.open_last_project),
            return_to_start_on_stop: flag(
                "return_to_start_on_stop",
                defaults.return_to_start_on_stop,
            ),
            record_mode: obj
                .get("record_mode")
                .and_then(Value::as_str)
                .and_then(RecordMode::from_id)
                .unwrap_or(defaults.record_mode),
            keep_events: obj
                .get("keep_events")
                .and_then(Value::as_str)
                .and_then(KeepEvents::from_id)
                .unwrap_or(defaults.keep_events),
            warp_imports: obj
                .get("warp_imports")
                .and_then(Value::as_str)
                .and_then(WarpImports::from_id)
                .unwrap_or(defaults.warp_imports),
            master_keyboard: obj
                .get("master_keyboard")
                .and_then(Value::as_str)
                .and_then(MasterKeyboard::from_id)
                .unwrap_or(defaults.master_keyboard),
            theme: obj
                .get("theme")
                .and_then(Value::as_str)
                .and_then(ThemeChoice::from_id)
                .unwrap_or(defaults.theme),
            app_zoom: obj
                .get("app_zoom")
                .and_then(Value::as_u64)
                .map_or(defaults.app_zoom, nearest_zoom),
            knob_range: obj
                .get("knob_range")
                .and_then(Value::as_str)
                .and_then(KnobRange::from_id)
                .unwrap_or(defaults.knob_range),
            value_tooltips: flag("value_tooltips", defaults.value_tooltips),
            automation_thinning: obj
                .get("automation_thinning")
                .and_then(Value::as_str)
                .and_then(ThinLevel::from_id)
                .unwrap_or(defaults.automation_thinning),
            extra: obj
                .iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        "auto_route"
                            | "show_tooltips"
                            | "create_keeps_browser_open"
                            | "follow_song"
                            | "follow_selection"
                            | "rack_visible"
                            | "keys_notice_seen"
                            | "library_folder"
                            | "autosave_minutes"
                            | "open_last_project"
                            | "return_to_start_on_stop"
                            | "record_mode"
                            | "keep_events"
                            | "warp_imports"
                            | "master_keyboard"
                            | "theme"
                            | "app_zoom"
                            | "knob_range"
                            | "value_tooltips"
                            | "automation_thinning"
                    )
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        })
    }

    /// The canonical text: the known fields in a fixed order, then the unknown ones sorted.
    pub fn to_json(&self) -> String {
        let mut map = Map::new();
        map.insert("auto_route".into(), Value::Bool(self.auto_route));
        map.insert(
            "create_keeps_browser_open".into(),
            Value::Bool(self.create_keeps_browser_open),
        );
        map.insert("follow_song".into(), Value::Bool(self.follow_song));
        map.insert(
            "follow_selection".into(),
            Value::Bool(self.follow_selection),
        );
        map.insert("show_tooltips".into(), Value::Bool(self.show_tooltips));
        map.insert("rack_visible".into(), Value::Bool(self.rack_visible));
        map.insert(
            "keys_notice_seen".into(),
            Value::Bool(self.keys_notice_seen),
        );
        map.insert(
            "autosave_minutes".into(),
            Value::from(self.autosave_minutes),
        );
        map.insert(
            "open_last_project".into(),
            Value::Bool(self.open_last_project),
        );
        map.insert(
            "return_to_start_on_stop".into(),
            Value::Bool(self.return_to_start_on_stop),
        );
        map.insert(
            "record_mode".into(),
            Value::String(self.record_mode.id().into()),
        );
        map.insert(
            "keep_events".into(),
            Value::String(self.keep_events.id().into()),
        );
        map.insert(
            "warp_imports".into(),
            Value::String(self.warp_imports.id().into()),
        );
        map.insert(
            "master_keyboard".into(),
            Value::String(self.master_keyboard.id().into()),
        );
        map.insert("theme".into(), Value::String(self.theme.id().into()));
        map.insert("app_zoom".into(), Value::from(self.app_zoom));
        map.insert(
            "knob_range".into(),
            Value::String(self.knob_range.id().into()),
        );
        map.insert("value_tooltips".into(), Value::Bool(self.value_tooltips));
        map.insert(
            "automation_thinning".into(),
            Value::String(self.automation_thinning.id().into()),
        );
        if let Some(folder) = &self.library_folder {
            map.insert(
                "library_folder".into(),
                Value::String(folder.to_string_lossy().into_owned()),
            );
        }
        for (k, v) in &self.extra {
            map.insert(k.clone(), v.clone());
        }
        // Plain data with string keys: serialization cannot fail.
        let mut text = serde_json::to_string_pretty(&Value::Object(map)).expect("prefs serialize");
        text.push('\n');
        text
    }
}

/// Writes `text` to `path` atomically: a temporary file in the same folder, then a rename.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    crate::recovery::atomic_write(path, text.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

use crate::recovery::quarantine;

fn read(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn unusable(path: &Path, why: &str) -> String {
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    match quarantine(path) {
        Some(bad) => format!(
            "{name} could not be read ({why}); it was kept as {} and the defaults are used",
            bad.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        ),
        None => format!("{name} could not be read ({why}); the defaults are used"),
    }
}

/// Loads `keymap.json`: the defaults when missing, the defaults and a message when unusable
/// (the file is renamed `.bad`), the entries that are usable otherwise.
pub fn load_user_keymap(path: &Path) -> Loaded<UserKeymap> {
    let text = match read(path) {
        Ok(Some(t)) => t,
        Ok(None) => {
            return Loaded {
                value: UserKeymap::default(),
                status: None,
            };
        }
        Err(e) => {
            return Loaded {
                value: UserKeymap::default(),
                status: Some(e),
            };
        }
    };
    match UserKeymap::from_json(&text) {
        Ok((user, notes)) => Loaded {
            value: user,
            status: (!notes.is_empty()).then(|| format!("keymap.json: {}", notes.join("; "))),
        },
        Err(why) => Loaded {
            value: UserKeymap::default(),
            status: Some(unusable(path, &why)),
        },
    }
}

/// Saves `keymap.json` (atomic, canonical).
pub fn save_user_keymap(path: &Path, user: &UserKeymap) -> Result<(), String> {
    write_atomic(path, &user.to_json())
}

/// Loads `workflow.json` (see [`load_user_keymap`] for the rules).
pub fn load_workflow(path: &Path) -> Loaded<WorkflowPrefs> {
    let text = match read(path) {
        Ok(Some(t)) => t,
        Ok(None) => {
            return Loaded {
                value: WorkflowPrefs::default(),
                status: None,
            };
        }
        Err(e) => {
            return Loaded {
                value: WorkflowPrefs::default(),
                status: Some(e),
            };
        }
    };
    match WorkflowPrefs::from_json(&text) {
        Ok(p) => Loaded {
            value: p,
            status: None,
        },
        Err(why) => Loaded {
            value: WorkflowPrefs::default(),
            status: Some(unusable(path, &why)),
        },
    }
}

/// Saves `workflow.json` (atomic, canonical).
pub fn save_workflow(path: &Path, prefs: &WorkflowPrefs) -> Result<(), String> {
    write_atomic(path, &prefs.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_ui::keymap::{BindingDef, Context, Profile};

    /// A fresh directory under the system temp dir (never the real config directory).
    fn dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("atomik-keymap-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn user() -> UserKeymap {
        UserKeymap {
            profile: Some(Profile::Legacy),
            bindings: vec![BindingDef {
                command: "edit.duplicate".into(),
                arg: None,
                context: Context::Global,
                chords: vec!["Cmd+Shift+D".into()],
                os: None,
            }],
            ..UserKeymap::default()
        }
    }

    #[test]
    fn the_files_sit_next_to_settings_json() {
        let s = Path::new("cfg").join("settings.json");
        assert_eq!(keymap_path(&s), Path::new("cfg").join("keymap.json"));
        assert_eq!(workflow_path(&s), Path::new("cfg").join("workflow.json"));
    }

    #[test]
    fn a_missing_keymap_file_is_the_defaults_without_a_message() {
        let d = dir("missing");
        let l = load_user_keymap(&d.join(KEYMAP_FILE));
        assert_eq!(l.value, UserKeymap::default());
        assert_eq!(l.status, None);
        let w = load_workflow(&d.join(WORKFLOW_FILE));
        assert_eq!(w.value, WorkflowPrefs::default());
        assert_eq!(w.status, None);
    }

    #[test]
    fn the_keymap_round_trips_through_an_atomic_canonical_save() {
        let d = dir("roundtrip");
        let p = d.join(KEYMAP_FILE);
        save_user_keymap(&p, &user()).unwrap();
        assert!(
            !d.join("keymap.json.tmp").exists(),
            "the temporary file is renamed away"
        );
        let first = std::fs::read(&p).unwrap();
        let l = load_user_keymap(&p);
        assert_eq!(l.value, user());
        assert_eq!(l.status, None);
        save_user_keymap(&p, &l.value).unwrap();
        assert_eq!(
            std::fs::read(&p).unwrap(),
            first,
            "saving again is byte-identical"
        );
        // Overwriting replaces the old content.
        save_user_keymap(&p, &UserKeymap::default()).unwrap();
        assert_eq!(load_user_keymap(&p).value, UserKeymap::default());
    }

    #[test]
    fn a_corrupt_keymap_is_kept_as_bad_and_the_defaults_are_used() {
        let d = dir("corrupt");
        let p = d.join(KEYMAP_FILE);
        std::fs::write(&p, "{ not json").unwrap();
        let l = load_user_keymap(&p);
        assert_eq!(l.value, UserKeymap::default());
        let status = l.status.unwrap();
        assert!(status.contains("keymap.json.bad"), "{status}");
        assert!(!p.exists());
        assert_eq!(
            std::fs::read_to_string(d.join("keymap.json.bad")).unwrap(),
            "{ not json"
        );
        // A second corrupt file replaces the first .bad.
        std::fs::write(&p, "[]").unwrap();
        assert!(load_user_keymap(&p).status.is_some());
        assert_eq!(
            std::fs::read_to_string(d.join("keymap.json.bad")).unwrap(),
            "[]"
        );
    }

    #[test]
    fn bad_entries_in_a_readable_keymap_are_skipped_with_a_message() {
        let d = dir("entries");
        let p = d.join(KEYMAP_FILE);
        std::fs::write(
            &p,
            r#"{"bindings": [{"command": "edit.undo", "context": "global", "chords": ["Cmd+U"]}, 5]}"#,
        )
        .unwrap();
        let l = load_user_keymap(&p);
        assert_eq!(l.value.bindings.len(), 1);
        assert!(l.status.unwrap().contains("bindings[1]"));
        assert!(p.exists(), "a readable file stays");
    }

    #[test]
    fn workflow_defaults_are_auto_route_on_and_the_browser_closing() {
        let w = WorkflowPrefs::default();
        assert!(w.auto_route && !w.create_keeps_browser_open);
        let parsed = WorkflowPrefs::from_json("{}").unwrap();
        assert_eq!(parsed, w, "an empty file is the defaults");
    }

    #[test]
    fn workflow_loading_is_lenient_and_unknown_fields_survive_a_save() {
        let w = WorkflowPrefs::from_json(
            r#"{"auto_route": false, "follow_song": "yes", "rack_visible": false, "zoom": {"a": 1}}"#,
        )
        .unwrap();
        assert!(!w.auto_route);
        assert!(!w.follow_song, "a wrongly typed field takes its default");
        assert!(!w.rack_visible, "the rack flag is a known field now");
        assert_eq!(w.extra.len(), 1);
        let text = w.to_json();
        assert!(
            text.contains("rack_visible") && text.contains("zoom"),
            "{text}"
        );
        assert_eq!(WorkflowPrefs::from_json(&text).unwrap(), w);
        assert!(WorkflowPrefs::from_json("[1]").is_err());
        assert!(WorkflowPrefs::from_json("nope").is_err());
    }

    #[test]
    fn the_layout_flag_and_the_notice_flag_round_trip() {
        let d = WorkflowPrefs::default();
        assert!(d.rack_visible && !d.keys_notice_seen);
        let w = WorkflowPrefs {
            rack_visible: false,
            keys_notice_seen: true,
            ..d
        };
        assert_eq!(WorkflowPrefs::from_json(&w.to_json()).unwrap(), w);
        assert!(w.to_json().contains("\"keys_notice_seen\": true"));
    }

    #[test]
    fn the_interface_preferences_default_to_how_the_app_always_looked_and_round_trip() {
        let d = WorkflowPrefs::default();
        assert_eq!(d.theme, ThemeChoice::Dark);
        assert_eq!(d.app_zoom, 100);
        assert_eq!(d.knob_range, KnobRange::Normal);
        assert!(!d.value_tooltips);
        let w = WorkflowPrefs {
            theme: ThemeChoice::System,
            app_zoom: 150,
            knob_range: KnobRange::VeryPrecise,
            value_tooltips: true,
            ..d
        };
        let text = w.to_json();
        for field in [
            "\"theme\": \"system\"",
            "\"app_zoom\": 150",
            "\"knob_range\": \"very_precise\"",
            "\"value_tooltips\": true",
        ] {
            assert!(text.contains(field), "{field} in {text}");
        }
        assert_eq!(WorkflowPrefs::from_json(&text).unwrap(), w);
        // Saving again is byte-identical.
        assert_eq!(WorkflowPrefs::from_json(&text).unwrap().to_json(), text);
    }

    #[test]
    fn the_automation_thinning_preference_round_trips_and_falls_back_to_normal() {
        let d = WorkflowPrefs::default();
        assert_eq!(d.automation_thinning, ThinLevel::Normal);
        let w = WorkflowPrefs {
            automation_thinning: ThinLevel::Heavy,
            ..d
        };
        let text = w.to_json();
        assert!(
            text.contains("\"automation_thinning\": \"heavy\""),
            "{text}"
        );
        assert_eq!(WorkflowPrefs::from_json(&text).unwrap(), w);
        let bad = WorkflowPrefs::from_json(r#"{"automation_thinning": "extreme"}"#).unwrap();
        assert_eq!(bad.automation_thinning, ThinLevel::Normal);
    }

    #[test]
    fn a_workflow_file_from_before_the_interface_preferences_loads_with_their_defaults() {
        // What an older build wrote: no interface fields, and a field this build knows nothing of.
        let old = r#"{
  "auto_route": false,
  "autosave_minutes": 10,
  "follow_song": true,
  "future_option": [1, 2]
}"#;
        let w = WorkflowPrefs::from_json(old).unwrap();
        assert!(!w.auto_route && w.follow_song && w.autosave_minutes == 10);
        assert_eq!(
            (w.theme, w.app_zoom, w.knob_range, w.value_tooltips),
            (ThemeChoice::Dark, 100, KnobRange::Normal, false)
        );
        assert!(w.extra.contains_key("future_option"), "unknown fields stay");
        let saved = w.to_json();
        assert!(saved.contains("future_option"));
        assert_eq!(WorkflowPrefs::from_json(&saved).unwrap(), w);
    }

    #[test]
    fn a_hand_edited_interface_preference_falls_back_or_snaps_to_a_choice() {
        let w = WorkflowPrefs::from_json(
            r#"{"theme": "sepia", "app_zoom": 130, "knob_range": 3, "value_tooltips": "yes"}"#,
        )
        .unwrap();
        assert_eq!(
            w.theme,
            ThemeChoice::Dark,
            "an unknown theme is the default"
        );
        assert_eq!(w.app_zoom, 125, "a zoom snaps to the nearest step");
        assert_eq!(w.knob_range, KnobRange::Normal);
        assert!(!w.value_tooltips);
        for (given, want) in [(0, 75), (74, 75), (87, 75), (88, 100), (10_000, 200)] {
            assert_eq!(nearest_zoom(given), want, "{given}");
        }
    }

    #[test]
    fn the_library_folder_defaults_to_none_and_round_trips() {
        assert_eq!(WorkflowPrefs::default().library_folder, None);
        assert_eq!(WorkflowPrefs::from_json("{}").unwrap().library_folder, None);
        let w = WorkflowPrefs {
            library_folder: Some(PathBuf::from("D:/Sounds/Library")),
            ..WorkflowPrefs::default()
        };
        let text = w.to_json();
        assert!(text.contains("\"library_folder\""), "{text}");
        assert_eq!(WorkflowPrefs::from_json(&text).unwrap(), w);
        // Default is not written; a wrongly typed or empty value means the default.
        assert!(
            !WorkflowPrefs::default()
                .to_json()
                .contains("library_folder")
        );
        for bad in [r#"{"library_folder": 3}"#, r#"{"library_folder": ""}"#] {
            assert_eq!(WorkflowPrefs::from_json(bad).unwrap().library_folder, None);
        }
        assert!(
            WorkflowPrefs::from_json(r#"{"library_folder": 3}"#)
                .unwrap()
                .extra
                .is_empty()
        );
    }

    #[test]
    fn workflow_saves_canonically_and_persists() {
        let d = dir("workflow");
        let p = d.join(WORKFLOW_FILE);
        let w = WorkflowPrefs {
            auto_route: false,
            create_keeps_browser_open: true,
            ..WorkflowPrefs::default()
        };
        save_workflow(&p, &w).unwrap();
        let first = std::fs::read(&p).unwrap();
        let l = load_workflow(&p);
        assert_eq!(l.value, w);
        save_workflow(&p, &l.value).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), first);
        let text = String::from_utf8(first).unwrap();
        assert!(text.ends_with("}\n"));
        assert!(text.find("auto_route").unwrap() < text.find("follow_song").unwrap());
    }

    #[test]
    fn a_corrupt_workflow_file_is_kept_as_bad_too() {
        let d = dir("workflow-bad");
        let p = d.join(WORKFLOW_FILE);
        std::fs::write(&p, "{{").unwrap();
        let l = load_workflow(&p);
        assert_eq!(l.value, WorkflowPrefs::default());
        assert!(l.status.unwrap().contains("workflow.json.bad"));
        assert!(d.join("workflow.json.bad").exists());
    }

    #[test]
    fn show_tooltips_is_on_by_default_and_round_trips_through_workflow_json() {
        assert!(WorkflowPrefs::default().show_tooltips);
        // A file from before the preference has it on.
        assert!(WorkflowPrefs::from_json("{}").unwrap().show_tooltips);
        // A wrongly typed value is the default too.
        assert!(
            WorkflowPrefs::from_json(r#"{"show_tooltips": 3}"#)
                .unwrap()
                .show_tooltips
        );
        let d = dir("tooltips");
        let p = d.join(WORKFLOW_FILE);
        let off = WorkflowPrefs {
            show_tooltips: false,
            ..WorkflowPrefs::default()
        };
        save_workflow(&p, &off).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("\"show_tooltips\": false"), "{text}");
        assert_eq!(load_workflow(&p).value, off);
        // The unknown fields of a newer build are kept and this one is not one of them.
        assert!(off.extra.is_empty());
    }

    #[test]
    fn saving_creates_the_directory() {
        let d = dir("mkdir");
        let p = d.join("deeper").join("still").join(KEYMAP_FILE);
        save_user_keymap(&p, &user()).unwrap();
        assert!(p.exists());
    }

    #[test]
    fn warp_on_import_defaults_to_loops_only_and_round_trips() {
        use crate::import_warp::WarpImports;
        assert_eq!(WorkflowPrefs::default().warp_imports, WarpImports::Loops);
        assert_eq!(
            WorkflowPrefs::from_json("{}").unwrap().warp_imports,
            WarpImports::Loops
        );
        let p = WorkflowPrefs::from_json(r#"{"warp_imports": 7}"#).unwrap();
        assert_eq!(p.warp_imports, WarpImports::Loops);
        for mode in WarpImports::ALL {
            let set = WorkflowPrefs {
                warp_imports: mode,
                ..WorkflowPrefs::default()
            };
            let text = set.to_json();
            assert!(text.contains(&format!("\"warp_imports\": \"{}\"", mode.id())));
            let back = WorkflowPrefs::from_json(&text).unwrap();
            assert_eq!(back, set);
            assert!(back.extra.is_empty());
        }
    }

    #[test]
    fn the_record_mode_and_keep_events_round_trip_and_default_to_replace_and_keep() {
        let d = WorkflowPrefs::default();
        assert_eq!(
            (d.record_mode, d.keep_events),
            (RecordMode::Replace, KeepEvents::Keep)
        );
        let p = WorkflowPrefs::from_json("{}").unwrap();
        assert_eq!(
            (p.record_mode, p.keep_events),
            (RecordMode::Replace, KeepEvents::Keep)
        );
        // An unknown name or a wrongly typed value is the default.
        let p = WorkflowPrefs::from_json(r#"{"record_mode": "nope", "keep_events": 3}"#).unwrap();
        assert_eq!(
            (p.record_mode, p.keep_events),
            (RecordMode::Replace, KeepEvents::Keep)
        );
        let set = WorkflowPrefs {
            record_mode: RecordMode::Layer,
            keep_events: KeepEvents::NewClip,
            ..WorkflowPrefs::default()
        };
        let text = set.to_json();
        assert!(text.contains("\"record_mode\": \"layer\""), "{text}");
        assert!(text.contains("\"keep_events\": \"new_clip\""), "{text}");
        let back = WorkflowPrefs::from_json(&text).unwrap();
        assert_eq!(back, set);
        assert!(back.extra.is_empty());
    }

    #[test]
    fn autosave_minutes_round_trips_defaults_to_five_and_is_clamped() {
        assert_eq!(WorkflowPrefs::default().autosave_minutes, 5);
        assert_eq!(WorkflowPrefs::from_json("{}").unwrap().autosave_minutes, 5);
        let off = WorkflowPrefs::from_json("{\"autosave_minutes\": 0}").unwrap();
        assert_eq!(off.autosave_minutes, 0);
        assert_eq!(WorkflowPrefs::from_json(&off.to_json()).unwrap(), off);
        let big = WorkflowPrefs::from_json("{\"autosave_minutes\": 99999}").unwrap();
        assert_eq!(big.autosave_minutes, 240);
        assert!(!big.extra.contains_key("autosave_minutes"));
    }

    #[test]
    fn open_last_project_round_trips_and_defaults_to_off() {
        assert!(!WorkflowPrefs::default().open_last_project);
        assert!(!WorkflowPrefs::from_json("{}").unwrap().open_last_project);
        let on = WorkflowPrefs::from_json("{\"open_last_project\": true}").unwrap();
        assert!(on.open_last_project);
        assert!(on.to_json().contains("\"open_last_project\": true"));
        assert_eq!(WorkflowPrefs::from_json(&on.to_json()).unwrap(), on);
        assert!(!on.extra.contains_key("open_last_project"));
        // A wrongly typed value is the default.
        let bad = WorkflowPrefs::from_json("{\"open_last_project\": \"yes\"}").unwrap();
        assert!(!bad.open_last_project);
    }
}

#[cfg(test)]
mod master_keyboard_tests {
    use super::*;
    use crate::master_keyboard::MasterKeyboard;

    #[test]
    fn the_master_keyboard_mode_is_saved_and_defaults_to_standard() {
        assert_eq!(
            WorkflowPrefs::default().master_keyboard,
            MasterKeyboard::Standard
        );
        // An older file has no such field; a wrong value is the default.
        let old = WorkflowPrefs::from_json("{}").unwrap();
        assert_eq!(old.master_keyboard, MasterKeyboard::Standard);
        let bad = WorkflowPrefs::from_json(r#"{"master_keyboard": 3}"#).unwrap();
        assert_eq!(bad.master_keyboard, MasterKeyboard::Standard);
        let set = WorkflowPrefs {
            master_keyboard: MasterKeyboard::Separated,
            ..WorkflowPrefs::default()
        };
        let text = set.to_json();
        assert!(
            text.contains("\"master_keyboard\": \"separated\""),
            "{text}"
        );
        let back = WorkflowPrefs::from_json(&text).unwrap();
        assert_eq!(back, set);
        assert!(
            back.extra.is_empty(),
            "the field is known, not kept as extra"
        );
    }
}
