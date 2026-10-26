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

use atomik_ui::keymap::UserKeymap;
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
    /// Fields this build does not know, kept as they were.
    pub extra: BTreeMap<String, Value>,
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
    let fail = |e: std::io::Error| format!("{}: {e}", path.display());
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text).map_err(fail)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        fail(e)
    })
}

/// Moves an unusable file out of the way as `<name>.bad`.
fn quarantine(path: &Path) -> Option<PathBuf> {
    let mut bad = path.as_os_str().to_owned();
    bad.push(".bad");
    let bad = PathBuf::from(bad);
    std::fs::rename(path, &bad).ok().map(|()| bad)
}

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
}
