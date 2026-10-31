//! The MIDI Out device's port: `SetMidiOutPort` as one undo step, the optional `"midi_out"` field
//! of project format 37 (written only when set, refused on other devices and in older files), and
//! the way copies and old projects treat it.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, DeviceSnapshot, Document, MAX_MIDI_OUT_PORT_CHARS, MIDI_OUT_TYPE, ModelError,
    PROJECT_VERSION, SyncOp, WorkflowCommand, empty_doc, load_project, save_project,
};
use common::workflow::{one_step, rejected, text, wf};
use common::*;

fn set(device: &str, port: Option<&str>) -> Command {
    wf(WorkflowCommand::SetMidiOutPort {
        device: key(device),
        port: port.map(str::to_owned),
    })
}

fn with_device() -> (Document, Registry) {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add(0, MIDI_OUT_TYPE, Some("out")), add(1, SYNTH, Some("s"))],
    );
    (d, r)
}

fn port(d: &Document, k: &str) -> Option<String> {
    d.doc()
        .resolve(&key(k))
        .unwrap()
        .midi_out_port()
        .map(str::to_owned)
}

#[test]
fn a_new_device_has_no_port_and_the_type_id_is_the_registered_one() {
    let (d, r) = with_device();
    assert!(r.get(MIDI_OUT_TYPE).is_some());
    assert_eq!(port(&d, "out"), None);
}

#[test]
fn setting_and_clearing_the_port_are_undo_steps_that_report_a_view_change() {
    let (mut d, r) = with_device();
    let ops = one_step(&mut d, &r, set("out", Some("Example Synth 1")));
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    assert_eq!(port(&d, "out").as_deref(), Some("Example Synth 1"));
    one_step(&mut d, &r, set("out", Some("Example Synth 2")));
    one_step(&mut d, &r, set("out", None));
    assert_eq!(port(&d, "out"), None);
    d.undo(&r).unwrap();
    assert_eq!(port(&d, "out").as_deref(), Some("Example Synth 2"));
    d.undo(&r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(port(&d, "out"), None);
}

#[test]
fn the_command_refuses_what_is_not_a_port_change() {
    let (mut d, r) = with_device();
    // Another device type.
    let e = rejected(&mut d, &r, set("s", Some("Port")));
    assert!(e.to_string().contains("not a MIDI Out"), "{e}");
    // An unknown device.
    rejected(&mut d, &r, set("nope", Some("Port")));
    // Clearing a port that is not there, and setting the one that is there.
    rejected(&mut d, &r, set("out", None));
    run(&mut d, &r, [set("out", Some("Port"))]);
    rejected(&mut d, &r, set("out", Some("Port")));
    // Names that are not trimmed, empty, with a control character or too long.
    for bad in [
        "",
        " Port",
        "Port ",
        "a\nb",
        &"x".repeat(MAX_MIDI_OUT_PORT_CHARS + 1),
    ] {
        let e = rejected(&mut d, &r, set("out", Some(bad)));
        assert!(matches!(e, ModelError::BadScope(_)), "{bad:?}: {e}");
    }
    run(
        &mut d,
        &r,
        [set("out", Some(&"x".repeat(MAX_MIDI_OUT_PORT_CHARS)))],
    );
}

#[test]
fn the_port_is_saved_only_when_set_and_round_trips() {
    let (mut d, r) = with_device();
    let none = text(&d);
    assert!(!none.contains("midi_out"), "no field without a port");
    assert!(none.contains(&format!("\"version\": {PROJECT_VERSION}")));
    run(&mut d, &r, [set("out", Some("Example \"Synth\" 1"))]);
    let saved = text(&d);
    assert!(saved.contains("\"midi_out\": { \"port\": \"Example \\\"Synth\\\" 1\" }"));
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(
        back.resolve(&key("out")).unwrap().midi_out_port(),
        Some("Example \"Synth\" 1")
    );
    assert_eq!(save_project(&back), saved, "the save is canonical");
    // Undoing the port gives the text without it back, byte for byte.
    d.undo(&r).unwrap();
    assert_eq!(text(&d), none);
}

#[test]
fn a_project_without_a_port_differs_from_its_version_36_file_in_the_version_line_alone() {
    let (d, r) = with_device();
    let current = text(&d);
    let old = current.replacen("\"version\": 37", "\"version\": 36", 1);
    let loaded = load_project(&old, &r).expect("a version 36 file loads");
    assert_eq!(save_project(&loaded), current);
}

#[test]
fn the_field_is_refused_in_a_version_36_file_and_on_other_devices_and_when_malformed() {
    let (mut d, r) = with_device();
    run(&mut d, &r, [set("out", Some("Port"))]);
    let saved = text(&d);
    let old = saved.replacen("\"version\": 37", "\"version\": 36", 1);
    let e = load_project(&old, &r).unwrap_err().to_string();
    assert!(e.contains("midi_out") && e.contains("unknown field"), "{e}");
    // On a device that is not a MIDI Out device.
    let wrong = saved.replacen(
        "\"id\": \"out\", \"type\": \"atomik.midi-out\"",
        "\"id\": \"out\", \"type\": \"atomik.poly-synth\"",
        1,
    );
    assert_ne!(wrong, saved);
    let e = load_project(&wrong, &r).unwrap_err().to_string();
    assert!(e.contains("not a MIDI Out device"), "{e}");
    // A malformed object or name.
    for bad in [
        "{ \"port\": \"\" }",
        "{ \"port\": \" x\" }",
        "{ \"port\": 7 }",
        "{}",
        "{ \"port\": \"a\", \"extra\": 1 }",
        "\"Port\"",
    ] {
        let broken = saved.replacen("{ \"port\": \"Port\" }", bad, 1);
        assert!(load_project(&broken, &r).is_err(), "{bad}");
    }
}

#[test]
fn a_copy_of_the_device_keeps_its_port() {
    let (mut d, r) = with_device();
    run(&mut d, &r, [set("out", Some("Port"))]);
    let snapshot = DeviceSnapshot::capture(d.doc(), &[key("out")], false).unwrap();
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::PasteDevices {
            snapshot,
            container: None,
            at: 2,
        })],
    );
    let pasted: Vec<_> = d
        .doc()
        .devices
        .iter()
        .filter(|x| x.type_id == MIDI_OUT_TYPE)
        .collect();
    assert_eq!(pasted.len(), 2);
    assert!(pasted.iter().all(|x| x.midi_out_port() == Some("Port")));
}

#[test]
fn a_device_patch_neither_carries_nor_clears_the_port() {
    let (mut d, r) = with_device();
    run(
        &mut d,
        &r,
        [
            add(2, MIDI_OUT_TYPE, Some("other")),
            set("out", Some("Port")),
            set("other", Some("Other Port")),
            set_param("out", "channel", 5.0),
        ],
    );
    let snapshot = DeviceSnapshot::capture(d.doc(), &[key("out")], false).unwrap();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::ApplyPatch {
            device: key("other"),
            snapshot,
        }),
    );
    assert_eq!(port(&d, "other").as_deref(), Some("Other Port"));
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::ResetDevice {
            device: key("other"),
        }),
    );
    assert_eq!(port(&d, "other").as_deref(), Some("Other Port"));
}
