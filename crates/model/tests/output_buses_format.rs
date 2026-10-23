//! Project format v12: the console's optional `"buses"`. A project without buses saves exactly
//! as a v11 project but for the version line, v11 files (and every older one) load with no
//! buses, and a saved project with buses round-trips byte for byte. Every error names its path.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::console::OutputDest;
use atomik_model::{
    Command, ConsoleBus, ConsoleCommand, Document, PROJECT_VERSION, load_patch, load_project,
    new_project_doc, save_patch, save_project,
};
use common::plugin::plugin_console_registry;
use common::*;
use serde_json::Value;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
    .replace("\r\n", "\n")
}

/// Two synths, a bus "Drums" with a nested bus "Sub \"A\" é" under it, channel 1 on the sub.
fn project() -> (Document, Registry) {
    let r = plugin_console_registry();
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("a")),
            add(usize::MAX, SYNTH, Some("b")),
            Command::Console(ConsoleCommand::CreateBus {
                name: "Drums".into(),
                dest: OutputDest::Master,
            }),
            Command::Console(ConsoleCommand::CreateBus {
                name: "Sub \"A\" é".into(),
                dest: OutputDest::Master,
            }),
            Command::Console(ConsoleCommand::SetOutput {
                strip: key("bus-2"),
                dest: OutputDest::Bus(key("bus")),
            }),
            Command::Console(ConsoleCommand::SetOutput {
                strip: key("channel"),
                dest: OutputDest::Bus(key("bus-2")),
            }),
        ],
    );
    (d, r)
}

fn edit(text: &str, f: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    f(&mut v);
    serde_json::to_string(&v).unwrap()
}

fn error(text: &str, r: &Registry) -> String {
    load_project(text, r).unwrap_err().to_string()
}

#[test]
fn the_format_is_version_12() {
    assert_eq!(PROJECT_VERSION, 15);
    let (d, _) = project();
    assert!(save_project(d.doc()).contains("\"version\": 15,"));
}

#[test]
fn a_project_with_buses_round_trips_byte_for_byte() {
    let (d, r) = project();
    let text = save_project(d.doc());
    let back = load_project(&text, &r).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(
        back.console.buses,
        [
            ConsoleBus {
                bus: key("bus"),
                name: "Drums".into()
            },
            ConsoleBus {
                bus: key("bus-2"),
                name: "Sub \"A\" é".into()
            }
        ]
    );
    assert_eq!(save_project(&back), text, "saving is canonical");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v["console"]["buses"][0],
        serde_json::json!({"bus": "bus", "name": "Drums"})
    );
}

#[test]
fn the_bus_order_is_kept() {
    let (mut d, r) = project();
    d.apply(
        Command::Console(ConsoleCommand::ReorderBus {
            bus: key("bus"),
            to: 1,
        }),
        &r,
    )
    .unwrap();
    let back = load_project(&save_project(d.doc()), &r).unwrap();
    assert_eq!(back.console.buses[0].bus, key("bus-2"));
}

#[test]
fn a_project_without_buses_writes_no_buses_field() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            Command::Console(ConsoleCommand::DeleteBus { bus: key("bus-2") }),
            Command::Console(ConsoleCommand::DeleteBus { bus: key("bus") }),
        ],
    );
    let text = save_project(d.doc());
    assert!(!text.contains("buses"), "{text}");
    assert!(load_project(&text, &r).unwrap().console.buses.is_empty());
}

#[test]
fn a_v11_file_loads_without_buses_and_resaves_with_only_the_version_line_changed() {
    let r = plugin_console_registry();
    let file = fixture("demo-v11.atomik");
    assert!(file.contains("\"version\": 11,"));
    let doc = load_project(&file, &r).unwrap();
    assert!(doc.console.buses.is_empty());
    assert!(!doc.console.channels.is_empty());
    assert_eq!(save_project(&doc), as_current(&file, 11));
}

#[test]
fn the_current_example_projects_load_without_buses() {
    let r = plugin_console_registry();
    let file = as_current(&fixture("demo-v11.atomik"), 11);
    assert!(load_project(&file, &r).unwrap().console.buses.is_empty());
}

#[test]
fn buses_need_version_12() {
    let (d, r) = project();
    let text = save_project(d.doc());
    for v in 3..=11 {
        let old = text.replace("\"version\": 15", &format!("\"version\": {v}"));
        let e = error(&old, &r);
        assert!(e.contains("console") && e.contains("buses"), "v{v}: {e}");
    }
}

#[test]
fn a_bad_bus_entry_is_rejected_with_its_path() {
    let (d, r) = project();
    let text = save_project(d.doc());
    type Case = (&'static str, Box<dyn Fn(&mut Value)>, &'static str);
    let cases: [Case; 11] = [
        (
            "missing device",
            Box::new(|v| v["console"]["buses"][0]["bus"] = "nope".into()),
            "console.buses[0].bus",
        ),
        (
            "not a bus",
            Box::new(|v| v["console"]["buses"][0]["bus"] = "channel".into()),
            "not an atomik.bus",
        ),
        (
            "listed twice",
            Box::new(|v| v["console"]["buses"][1]["bus"] = "bus".into()),
            "listed twice",
        ),
        (
            "same name",
            Box::new(|v| v["console"]["buses"][1]["name"] = "drums".into()),
            "console.buses[1].name",
        ),
        (
            "empty name",
            Box::new(|v| v["console"]["buses"][0]["name"] = "".into()),
            "not a valid bus name",
        ),
        (
            "padded name",
            Box::new(|v| v["console"]["buses"][0]["name"] = " Drums".into()),
            "not a valid bus name",
        ),
        (
            "long name",
            Box::new(|v| v["console"]["buses"][0]["name"] = "x".repeat(33).into()),
            "at most 32",
        ),
        (
            "unknown field",
            Box::new(|v| v["console"]["buses"][0]["color"] = 1.into()),
            "console.buses[0]",
        ),
        (
            "missing name",
            Box::new(|v| {
                v["console"]["buses"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("name");
            }),
            "name",
        ),
        (
            "not an array",
            Box::new(|v| v["console"]["buses"] = "x".into()),
            "console.buses",
        ),
        (
            "no master",
            Box::new(|v| {
                v["console"]["master"] = Value::Null;
                v["console"]["returns"] =
                    serde_json::json!([null, null, null, null, null, null, null, null]);
            }),
            "no master",
        ),
    ];
    for (what, f, expect) in cases {
        let e = error(&edit(&text, f), &r);
        assert!(e.contains(expect), "{what}: {e}");
    }
}

#[test]
fn a_channel_source_or_return_cannot_be_a_bus() {
    let (d, r) = project();
    let text = save_project(d.doc());
    let as_source = edit(&text, |v| {
        v["console"]["channels"][0]["source"] = "bus".into()
    });
    assert!(error(&as_source, &r).contains("console device"));
    let as_return = edit(&text, |v| v["console"]["returns"][0] = "bus".into());
    assert!(error(&as_return, &r).contains("console device"));
}

#[test]
fn patches_carry_no_buses() {
    let (d, r) = project();
    let patch = save_patch(d.doc());
    assert!(!patch.contains("buses"));
    let back = load_patch(&patch, &r).unwrap();
    assert!(back.console.buses.is_empty());
    // The bus devices and their cables are plain rack devices in a patch.
    assert!(back.device(&key("bus")).is_some());
}

#[test]
fn undo_and_redo_keep_the_saved_bytes_in_step() {
    let (mut d, r) = project();
    let with = save_project(d.doc());
    d.undo(&r).unwrap();
    assert_ne!(save_project(d.doc()), with);
    d.redo(&r).unwrap();
    assert_eq!(save_project(d.doc()), with);
}
