//! Opening a file written by a newer build: only what the file itself marks as skippable is
//! dropped, with a warning, and anything else is still refused.

mod common;

use atomik_model::{
    ModelError, PROJECT_VERSION, load_project, load_project_with_warnings, save_project,
};
use common::console_registry;
use serde_json::{Value, json};

fn current() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/demo.atomik");
    let text = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The current demo (the canonical save of the newest format) as a file one version ahead that allows `future` on clips to be skipped.
fn newer() -> Value {
    let mut v = current();
    v["version"] = json!(PROJECT_VERSION + 1);
    v["compat"] = json!(PROJECT_VERSION);
    v["ignorable"] = json!({ "song.tracks[].lanes[].clips[].future": PROJECT_VERSION + 1 });
    v["song"]["tracks"][0]["lanes"][0]["clips"][0]["future"] = json!({ "any": ["shape"] });
    v
}

fn message(e: &ModelError) -> String {
    let ModelError::Patch(m) = e else {
        panic!("expected a patch error, got {e:?}")
    };
    m.clone()
}

#[test]
fn an_ignorable_field_of_a_newer_file_is_dropped_with_one_warning() {
    let reg = console_registry();
    let (doc, warnings) = load_project_with_warnings(&newer().to_string(), &reg).unwrap();
    let plain = load_project(&current().to_string(), &reg).unwrap();
    assert_eq!(
        warnings,
        ["song.tracks[0].lanes[0].clips[0].future: ignored, a field of a newer project format"]
    );
    assert_eq!(
        save_project(&doc),
        save_project(&plain),
        "nothing else changed"
    );
}

#[test]
fn a_field_that_is_not_listed_as_ignorable_is_still_refused_in_a_newer_file() {
    let mut v = newer();
    v["song"]["tracks"][0]["lanes"][0]["clips"][0]["surprise"] = json!(1);
    let e = load_project(&v.to_string(), &console_registry()).unwrap_err();
    let m = message(&e);
    assert!(
        m.starts_with("song.tracks[0].lanes[0].clips[0].surprise: unknown field"),
        "{m}"
    );
}

#[test]
fn a_newer_file_needs_its_compat_version_to_be_readable() {
    let reg = console_registry();
    let mut v = newer();
    v.as_object_mut().unwrap().remove("compat");
    let m = message(&load_project(&v.to_string(), &reg).unwrap_err());
    assert!(
        m.starts_with(&format!(
            "version: unsupported version {}",
            PROJECT_VERSION + 1
        )),
        "{m}"
    );
    let mut v = newer();
    v["compat"] = json!(PROJECT_VERSION + 1);
    let m = message(&load_project(&v.to_string(), &reg).unwrap_err());
    assert!(
        m.contains(&format!(
            "needs a reader of version {}",
            PROJECT_VERSION + 1
        )),
        "{m}"
    );
}

#[test]
fn the_current_format_writes_no_compat_fields() {
    let text = save_project(&load_project(&current().to_string(), &console_registry()).unwrap());
    assert!(!text.contains("\"compat\"") && !text.contains("\"ignorable\""));
}
