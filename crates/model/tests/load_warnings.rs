//! What a project load leaves out of the document is reported in the warnings, never dropped
//! silently: one test per case.

mod common;

use atomik_model::{load_project, load_project_with_warnings, save_project};
use common::console_registry;
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn a_clean_file_loads_without_warnings() {
    let reg = console_registry();
    for name in ["demo-v1.atomik", "demo-v12.atomik", "demo-v36.atomik"] {
        let (_, warnings) = load_project_with_warnings(&fixture(name).to_string(), &reg).unwrap();
        assert!(warnings.is_empty(), "{name}: {warnings:?}");
    }
}

#[test]
fn the_height_of_a_device_without_a_track_is_reported() {
    let reg = console_registry();
    let mut v = fixture("demo-v36.atomik");
    v["song"]["track_heights"] = json!({ "ghost": 80 });
    let (doc, warnings) = load_project_with_warnings(&v.to_string(), &reg).unwrap();
    assert_eq!(
        warnings,
        [
            "song.track_heights.ghost: the height of \"ghost\" was ignored, the song has no track for that device"
        ]
    );
    assert!(doc.song.track_heights.is_empty());
}

#[test]
fn a_version_1_instrument_that_cannot_get_a_channel_is_reported() {
    let reg = console_registry();
    let mut v = fixture("demo-v1.atomik");
    v["devices"].as_array_mut().unwrap().push(json!({
        "id": "quiet", "type": "test.silent-synth", "params": {}
    }));
    let (doc, warnings) = load_project_with_warnings(&v.to_string(), &reg).unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].starts_with("console: \"quiet\" was left without a channel (")
            && warnings[0].contains("no audio output"),
        "{}",
        warnings[0]
    );
    assert!(
        doc.console
            .channels
            .iter()
            .all(|c| c.source.as_ref().is_none_or(|s| s.0 != "quiet")),
        "the instrument has no channel"
    );
    // The other instruments still got theirs.
    let plain = load_project(&fixture("demo-v1.atomik").to_string(), &reg).unwrap();
    assert_eq!(doc.console.channels.len(), plain.console.channels.len());
    assert!(
        save_project(&doc).contains("\"quiet\""),
        "the device itself is kept"
    );
}
