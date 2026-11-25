//! The Headphone Correction device's imported data: `SetHeadphoneData` as one undo step, the
//! optional root field `"headphone"` of project format 45 (written only when a device has data,
//! ignorable, so the file keeps its `"compat"`), and the way copies, patches, containers and the
//! engine's hand-over treat it.

mod common;

use atomik_device_sdk::PlayerData;
use atomik_device_sdk::Registry;
use atomik_device_sdk::headphone::{HeadphoneBody, HeadphoneData, ResponseData, parse};
use atomik_model::{
    Command, DeviceSnapshot, Document, ModelError, PROJECT_VERSION, SyncOp, WorkflowCommand,
    empty_doc, load_project, player_data_for, save_project,
};
use common::workflow::{one_step, rejected, text, wf};
use common::*;

const TYPE: &str = "atomik.headphone-correction";

const PARAMETRIC: &str = "Preamp: -4.5 dB\n\
    Filter 1: ON LSC Fc 105 Hz Gain 4.2 dB Q 0.70\n\
    Filter 2: ON PK Fc 2900 Hz Gain -3.1 dB Q 1.8\n\
    Filter 3: ON HP Fc 22 Hz\n";

const RESPONSE: &str = "Frequency, Left, Right\n20, 1.5, 1.25\n100, 0.5, 0.75\n1000.5, 0, 0.1\n\
    10000, -3.25, -2.9\n20000, -8, -7.5\n";

fn data(text: &str, name: &str) -> HeadphoneData {
    parse(text, name).unwrap()
}

fn set(device: &str, data: Option<HeadphoneData>) -> Command {
    wf(WorkflowCommand::SetHeadphoneData {
        device: key(device),
        data,
    })
}

fn with_device() -> (Document, Registry) {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add(0, TYPE, Some("hp")), add(1, SYNTH, Some("s"))],
    );
    (d, r)
}

fn held(d: &Document, k: &str) -> Option<HeadphoneData> {
    d.doc().resolve(&key(k)).unwrap().headphone_data().cloned()
}

#[test]
fn a_new_device_has_no_data_and_the_type_takes_headphone_data() {
    let (d, r) = with_device();
    assert!(r.takes_headphone(TYPE));
    assert!(r.takes_player_data(TYPE));
    assert!(!r.takes_drums(TYPE));
    assert_eq!(held(&d, "hp"), None);
    // No data, nothing for the engine; no pattern data either.
    let device = d.doc().resolve(&key("hp")).unwrap();
    assert!(player_data_for(device, &r).is_none());
    assert!(device.steps.is_none());
}

#[test]
fn importing_replacing_and_removing_are_undo_steps_that_report_a_data_change() {
    let (mut d, r) = with_device();
    let a = data(PARAMETRIC, "a.txt");
    let b = data(RESPONSE, "b.csv");
    let ops = one_step(&mut d, &r, set("hp", Some(a.clone())));
    assert_eq!(ops, vec![SyncOp::PatternsChanged { device: key("hp") }]);
    assert_eq!(held(&d, "hp"), Some(a.clone()));
    one_step(&mut d, &r, set("hp", Some(b.clone())));
    one_step(&mut d, &r, set("hp", None));
    assert_eq!(held(&d, "hp"), None);
    d.undo().unwrap();
    assert_eq!(held(&d, "hp"), Some(b));
    d.undo().unwrap();
    assert_eq!(held(&d, "hp"), Some(a));
    d.undo().unwrap();
    assert_eq!(held(&d, "hp"), None);
    d.redo().unwrap();
    assert!(held(&d, "hp").is_some());
}

#[test]
fn the_engine_gets_the_stored_data() {
    let (mut d, r) = with_device();
    let a = data(PARAMETRIC, "a.txt");
    run(&mut d, &r, [set("hp", Some(a.clone()))]);
    let device = d.doc().resolve(&key("hp")).unwrap();
    let handed = player_data_for(device, &r).expect("data for the engine");
    assert_eq!(handed.headphone(), Some(&a));
    assert!(matches!(&*handed, PlayerData::Headphone(_)));
    // Another device type gets nothing.
    let s = d.doc().resolve(&key("s")).unwrap();
    assert!(player_data_for(s, &r).is_none());
}

#[test]
fn the_command_refuses_what_is_not_a_data_change() {
    let (mut d, r) = with_device();
    let a = data(PARAMETRIC, "a.txt");
    // Another device type, an unknown device.
    let e = rejected(&mut d, &r, set("s", Some(a.clone())));
    assert!(e.to_string().contains("not a Headphone Correction"), "{e}");
    rejected(&mut d, &r, set("nope", Some(a.clone())));
    // Removing what is not there, and importing what is there.
    rejected(&mut d, &r, set("hp", None));
    run(&mut d, &r, [set("hp", Some(a.clone()))]);
    rejected(&mut d, &r, set("hp", Some(a)));
    // Data that does not validate.
    let bad = HeadphoneData {
        name: "bad".into(),
        body: HeadphoneBody::Response(ResponseData {
            freq: vec![100.0, 50.0],
            left: vec![0.0, 1.0],
            right: None,
        }),
    };
    let e = rejected(&mut d, &r, set("hp", Some(bad)));
    assert!(matches!(e, ModelError::BadScope(_)), "{e}");
    let unnamed = HeadphoneData {
        name: " x".into(),
        ..data(RESPONSE, "ok")
    };
    rejected(&mut d, &r, set("hp", Some(unnamed)));
}

#[test]
fn the_data_is_saved_only_when_set_and_round_trips_bit_for_bit() {
    let (mut d, r) = with_device();
    let none = text(&d);
    assert!(!none.contains("\"headphone\": {"), "no field without data");
    assert!(none.contains(&format!("\"version\": {PROJECT_VERSION}")));
    assert!(none.contains("\"compat\": 44"));
    let a = data(PARAMETRIC, "My \"cans\".txt");
    run(&mut d, &r, [set("hp", Some(a.clone()))]);
    let saved = text(&d);
    assert!(saved.contains("\"headphone\": {"));
    // The field is ignorable: the header still says a reader of 44 can open it.
    assert!(saved.contains("\"compat\": 44"));
    assert!(saved.contains("\"headphone\": 45"));
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(back.resolve(&key("hp")).unwrap().headphone_data(), Some(&a));
    assert_eq!(save_project(&back), saved, "the save is canonical");
    // Undoing the import gives the text without it back, byte for byte.
    d.undo().unwrap();
    assert_eq!(text(&d), none);

    // A response with a right channel.
    let b = data(RESPONSE, "b.csv");
    run(&mut d, &r, [set("hp", Some(b.clone()))]);
    let saved = text(&d);
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(back.resolve(&key("hp")).unwrap().headphone_data(), Some(&b));
    assert_eq!(save_project(&back), saved);
}

#[test]
fn what_was_imported_survives_without_the_file() {
    // The project holds the numbers: nothing of the import's path is kept but the name.
    let (mut d, r) = with_device();
    let original = parse(RESPONSE, "C:\\Users\\me\\My Cans.csv").unwrap();
    assert_eq!(original.name, "My Cans.csv");
    run(&mut d, &r, [set("hp", Some(original.clone()))]);
    let saved = text(&d);
    assert!(!saved.contains("Users"));
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(held(&Document::new(back), "hp"), Some(original));
}

#[test]
fn a_long_response_round_trips() {
    let mut csv = String::new();
    for i in 0..3000 {
        let f = 20.0 * (1000.0f64).powf(f64::from(i) / 2999.0);
        csv.push_str(&format!("{f:.6}, {:.5}\n", (f.ln() * 3.0).sin() * 4.0));
    }
    let big = data(&csv, "big.csv");
    let (mut d, r) = with_device();
    run(&mut d, &r, [set("hp", Some(big.clone()))]);
    let saved = text(&d);
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(
        back.resolve(&key("hp")).unwrap().headphone_data(),
        Some(&big)
    );
}

#[test]
fn the_field_is_refused_on_other_devices_unknown_keys_and_when_malformed() {
    let (mut d, r) = with_device();
    run(&mut d, &r, [set("hp", Some(data(PARAMETRIC, "a.txt")))]);
    let saved = text(&d);
    // An entry for a device that is not a headphone device.
    let wrong = saved.replacen(
        "\"headphone\": {\n    \"hp\"",
        "\"headphone\": {\n    \"s\"",
        1,
    );
    assert_ne!(wrong, saved);
    let e = load_project(&wrong, &r).unwrap_err().to_string();
    assert!(e.contains("does not take headphone data"), "{e}");
    // An entry for a device that does not exist.
    let gone = saved.replacen(
        "\"headphone\": {\n    \"hp\"",
        "\"headphone\": {\n    \"zz\"",
        1,
    );
    let e = load_project(&gone, &r).unwrap_err().to_string();
    assert!(e.contains("no device"), "{e}");
    // Broken objects.
    let parametric = "\"filters\"";
    assert!(saved.contains(parametric));
    for (from, to) in [
        ("\"type\": \"PK\"", "\"type\": \"XX\""),
        ("\"q\": 0.7 }", "\"q\": 0 }"),
        ("\"name\": \"a.txt\"", "\"name\": \" a.txt\""),
        ("\"name\": \"a.txt\",", ""),
        ("\"filters\"", "\"response\""),
        ("\"preamp\": -4.5", "\"preamp\": \"loud\""),
        ("\"filters\"", "\"extra\": 1, \"filters\""),
    ] {
        let broken = saved.replacen(from, to, 1);
        assert_ne!(broken, saved, "{from}");
        assert!(load_project(&broken, &r).is_err(), "{from} -> {to}");
    }
    // Both bodies at once.
    let both = saved.replacen(
        "\"filters\"",
        "\"response\": { \"freq\": [1, 2], \"left\": [0, 0] }, \"filters\"",
        1,
    );
    assert!(load_project(&both, &r).is_err());
}

#[test]
fn an_older_file_without_the_field_still_loads() {
    let (d, r) = with_device();
    let current = text(&d);
    let old = current.replacen("\"version\": 46", "\"version\": 44", 1);
    let loaded = load_project(&old, &r).expect("a version 44 file loads");
    assert!(loaded.devices.iter().all(|x| x.headphone_data().is_none()));
}

#[test]
fn a_device_inside_a_container_keeps_its_data() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("box")),
            add_inner("box", 0, TYPE, "hp"),
        ],
    );
    let a = data(PARAMETRIC, "a.txt");
    run(&mut d, &r, [set("box/hp", Some(a.clone()))]);
    let saved = text(&d);
    assert!(saved.contains("\"box/hp\""));
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(
        back.resolve(&key("box/hp")).unwrap().headphone_data(),
        Some(&a)
    );
    assert_eq!(save_project(&back), saved);
    // The engine gets it with the path key.
    let device = d.doc().resolve(&key("box/hp")).unwrap();
    assert_eq!(player_data_for(device, &r).unwrap().headphone(), Some(&a));
}

#[test]
fn a_copy_of_the_device_keeps_its_data() {
    let (mut d, r) = with_device();
    let a = data(PARAMETRIC, "a.txt");
    run(&mut d, &r, [set("hp", Some(a.clone()))]);
    let snapshot = DeviceSnapshot::capture(d.doc(), &[key("hp")], false).unwrap();
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::PasteDevices {
            route: false,
            snapshot,
            container: None,
            at: 2,
        })],
    );
    let copies: Vec<_> = d
        .doc()
        .devices
        .iter()
        .filter(|x| x.type_id == TYPE)
        .collect();
    assert_eq!(copies.len(), 2);
    assert!(copies.iter().all(|x| x.headphone_data() == Some(&a)));
    // Both are saved, each under its own key.
    let saved = text(&d);
    let back = load_project(&saved, &r).unwrap();
    assert_eq!(
        back.devices
            .iter()
            .filter(|x| x.headphone_data().is_some())
            .count(),
        2
    );
}

#[test]
fn removing_the_device_and_undoing_brings_the_data_back() {
    let (mut d, r) = with_device();
    let a = data(PARAMETRIC, "a.txt");
    run(&mut d, &r, [set("hp", Some(a.clone()))]);
    run(&mut d, &r, [Command::RemoveDevice { key: key("hp") }]);
    assert!(!text(&d).contains("\"headphone\": {"));
    d.undo().unwrap();
    assert_eq!(held(&d, "hp"), Some(a));
}

#[test]
fn a_device_patch_neither_carries_nor_clears_the_data() {
    let (mut d, r) = with_device();
    let a = data(PARAMETRIC, "a.txt");
    run(
        &mut d,
        &r,
        [
            add(2, TYPE, Some("other")),
            set("hp", Some(a.clone())),
            set_param("hp", "amount", 1.5),
        ],
    );
    let snapshot = DeviceSnapshot::capture(d.doc(), &[key("hp")], false).unwrap();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::ApplyPatch {
            device: key("other"),
            snapshot,
        }),
    );
    assert_eq!(held(&d, "other"), None);
    run(&mut d, &r, [set("other", Some(data(RESPONSE, "b.csv")))]);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::ResetDevice {
            device: key("other"),
        }),
    );
    assert!(held(&d, "other").is_some());
    assert_eq!(held(&d, "hp"), Some(a));
}

/// A project with a parametric device, a response device (left and right) and a device in a
/// container, saved by this build and checked in: it must load, and saving it again gives the
/// same bytes.
#[test]
fn the_checked_in_project_loads_and_saves_the_same_bytes() {
    let r = registry();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/headphone-v45.atomik");
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, TYPE, Some("eq-file")),
            add(1, TYPE, Some("csv-file")),
            add(2, CONTAINER, Some("box")),
            add_inner("box", 0, TYPE, "inner"),
            set("eq-file", Some(data(PARAMETRIC, "AutoEQ style.txt"))),
            set("csv-file", Some(data(RESPONSE, "Left and right.csv"))),
            set("box/inner", Some(data("20 1\n20000 -1\n", "Short.csv"))),
        ],
    );
    let current = text(&d);
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &current).unwrap();
    }
    let on_disk = std::fs::read_to_string(&path)
        .expect("tests/fixtures/headphone-v45.atomik (regenerate with ATOMIK_UPDATE_GOLDEN=1)");
    assert_eq!(on_disk, current, "regenerate with ATOMIK_UPDATE_GOLDEN=1");
    let loaded = load_project(&on_disk, &r).unwrap();
    assert_eq!(save_project(&loaded), on_disk);
    assert_eq!(
        loaded.resolve(&key("csv-file")).unwrap().headphone_data(),
        Some(&data(RESPONSE, "Left and right.csv"))
    );
    assert!(
        loaded
            .resolve(&key("box/inner"))
            .unwrap()
            .headphone_data()
            .is_some()
    );
}
