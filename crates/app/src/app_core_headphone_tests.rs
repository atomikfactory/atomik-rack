//! Headphone Correction in the app core: importing a file is one undo step that stores the data
//! in the document, the engine gets it on insert and on every change, and a bad file changes
//! nothing and says why.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_model::empty_doc;

const TYPE: &str = "atomik.headphone-correction";
const SYNTH: &str = "atomik.poly-synth";

const PARAMETRIC: &str = "Preamp: -4 dB\nFilter 1: ON PK Fc 1000 Hz Gain 3 dB Q 1\n";
const RESPONSE: &str = "freq, dB\n20, 2\n1000, 0\n20000, -4\n";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn core() -> AppCore<RecordingSink> {
    AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None)
}

fn add(c: &mut AppCore<RecordingSink>, type_id: &str, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.to_string(),
        key: Some(key(k)),
    })
    .unwrap();
}

fn file(dir: &std::path::Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// The headphone data calls of the recording: `(engine id, data present, is a response)`.
fn sent(c: &AppCore<RecordingSink>) -> Vec<(DeviceInstanceId, bool)> {
    c.sink()
        .calls
        .iter()
        .filter_map(|x| match x {
            SinkCall::PlayerData { id, data } => Some((
                *id,
                data.as_deref().is_some_and(|d| d.headphone().is_some()),
            )),
            _ => None,
        })
        .collect()
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-headphone-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn importing_a_file_stores_the_data_sends_it_to_the_engine_and_undoes() {
    let dir = temp_dir("import");
    let mut c = core();
    add(&mut c, TYPE, "hp");
    let id = *c.ids.get(&key("hp")).unwrap();
    assert_eq!(c.headphone_data_of(&key("hp")), Some(false));
    // A device without data sends none to the engine.
    assert!(sent(&c).iter().all(|(_, present)| !present));
    let path = file(&dir, "My Cans.csv", RESPONSE);
    assert!(c.import_headphone_file(&key("hp"), &path));
    assert_eq!(c.headphone_data_of(&key("hp")), Some(true));
    let data = c.headphone_data(&key("hp")).unwrap();
    assert_eq!(data.name, "My Cans.csv");
    assert!(data.is_response());
    assert!(sent(&c).contains(&(id, true)), "{:?}", sent(&c));
    // The data lives in the project: removing the file changes nothing.
    std::fs::remove_file(&path).unwrap();
    assert!(c.headphone_data(&key("hp")).is_some());
    // One undo step.
    c.undo();
    assert_eq!(c.headphone_data_of(&key("hp")), Some(false));
    c.redo();
    assert_eq!(c.headphone_data_of(&key("hp")), Some(true));
    // A second import replaces it, one more step.
    let second = file(&dir, "eq.txt", PARAMETRIC);
    assert!(c.import_headphone_file(&key("hp"), &second));
    assert!(!c.headphone_data(&key("hp")).unwrap().is_response());
    c.undo();
    assert!(c.headphone_data(&key("hp")).unwrap().is_response());
    // Removing is a step too.
    c.remove_headphone_data(&key("hp"));
    assert_eq!(c.headphone_data_of(&key("hp")), Some(false));
    c.undo();
    assert_eq!(c.headphone_data_of(&key("hp")), Some(true));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_bad_file_changes_nothing_and_says_why() {
    let dir = temp_dir("bad");
    let mut c = core();
    add(&mut c, TYPE, "hp");
    add(&mut c, SYNTH, "synth");
    let dirty = c.is_dirty();
    for (name, text, expect) in [
        ("rising.csv", "20, 1\n10, 2\n", "line 2"),
        ("words.txt", "hello\nworld\n", "no frequency response"),
        ("eq.txt", "Filter 1: ON PK Fc 100 Hz Q 1\n", "Gain"),
        ("empty.txt", "", "no frequency response"),
    ] {
        let path = file(&dir, name, text);
        assert!(!c.import_headphone_file(&key("hp"), &path), "{name}");
        let said = c.last_rejection().unwrap_or_default().to_owned();
        assert!(said.contains(expect), "{name}: {said}");
        assert!(said.contains(name), "{said}");
        assert_eq!(c.headphone_data_of(&key("hp")), Some(false));
    }
    assert_eq!(c.is_dirty(), dirty, "a refused import is not an edit");
    // A file that is not there, and a device that is not a headphone device.
    assert!(!c.import_headphone_file(&key("hp"), &dir.join("missing.csv")));
    assert!(c.last_rejection().unwrap().contains("Cannot read"));
    let ok = file(&dir, "ok.csv", RESPONSE);
    assert!(!c.import_headphone_file(&key("synth"), &ok));
    assert!(
        c.last_rejection()
            .unwrap()
            .contains("Select a Headphone Correction device")
    );
    assert!(!c.import_headphone_file(&key("nope"), &ok));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_engine_gets_the_data_when_the_device_is_inserted_again() {
    let dir = temp_dir("undo-remove");
    let mut c = core();
    add(&mut c, TYPE, "hp");
    let path = file(&dir, "a.csv", RESPONSE);
    assert!(c.import_headphone_file(&key("hp"), &path));
    c.apply_command(Command::RemoveDevice { key: key("hp") })
        .unwrap();
    c.undo();
    let id = *c.ids.get(&key("hp")).unwrap();
    let calls = sent(&c);
    // The last call for the (re-inserted) device carries the data.
    assert_eq!(calls.last(), Some(&(id, true)), "{calls:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_target_is_the_selected_headphone_device_else_the_first_one() {
    let mut c = core();
    add(&mut c, SYNTH, "synth");
    assert_eq!(c.headphone_target(), None);
    add(&mut c, TYPE, "hp");
    assert_eq!(c.headphone_target(), Some(key("hp")));
    add(&mut c, TYPE, "hp2");
    c.view_mut().set_selected(Some(key("hp2")));
    assert_eq!(c.headphone_target(), Some(key("hp2")));
    c.view_mut().set_selected(Some(key("synth")));
    assert_eq!(c.headphone_target(), Some(key("hp")));
}

#[test]
fn the_panel_gets_the_meters_and_the_imported_data_through_the_view() {
    let mut c = core();
    add(&mut c, TYPE, "hp");
    let id = *c.ids.get(&key("hp")).unwrap();
    assert_eq!(
        atomik_ui::fx::FxKind::HeadphoneCorrection.meter_count(),
        atomik_devices::headphone_correction::METERS
    );
    // Loaded, the preamp, the boost, busy: the dB ones keep their sign.
    c.sink_mut()
        .device_values
        .insert(id, (vec![1.0, -4.24, 4.24, 1.0], 1));
    c.frame();
    let shown = c.view().device_meters(&key("hp")).to_vec();
    let want = [1.0, -4.2, 4.2, 1.0];
    assert!(
        shown.len() == 4 && shown.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4),
        "{shown:?}"
    );
    // Data imported through the core shows in the panel: its name and format are painted, and
    // the empty-state message is gone.
    let painted = |c: &mut AppCore<RecordingSink>| {
        let mut dl = atomik_ui::DisplayList::new();
        c.paint(&mut dl);
        dl.texts()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let before = painted(&mut c);
    assert!(
        before.iter().any(|t| t == "Import headphone data..."),
        "{before:?}"
    );
    assert!(c.import_headphone_text(&key("hp"), RESPONSE, "My Cans.csv"));
    let after = painted(&mut c);
    assert!(after.iter().any(|t| t == "My Cans.csv"), "{after:?}");
    assert!(
        after.iter().any(|t| t == "Frequency response, 3 points"),
        "{after:?}"
    );
    assert!(!after.iter().any(|t| t == "Import headphone data..."));
    // Undo takes it off the panel again.
    c.undo();
    let undone = painted(&mut c);
    assert!(
        undone.iter().any(|t| t == "Import headphone data..."),
        "{undone:?}"
    );
}
