//! Project format v11: the per-device `column` and the root `rack_columns`. Both are
//! optional (a single-column rack writes neither, so a v10 file differs from its v11 save only
//! in the version line), versions 1 to 10 load as one column with every device in column 0,
//! and a saved multi-column rack round-trips byte for byte.

mod common;

use atomik_model::{
    Command, Document, MAX_RACK_COLUMNS, PROJECT_VERSION, RackDoc, RackSlot, empty_doc, load_patch,
    load_project, save_patch, save_project,
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

const OLD_FIXTURES: [(&str, u32); 9] = [
    ("demo-v1.atomik", 1),
    ("demo-v6.atomik", 6),
    ("seq-v7.atomik", 7),
    ("container-v8.atomik", 8),
    ("plugin-v9.atomik", 9),
    ("demo-v9.atomik", 9),
    ("demo-v10.atomik", 10),
    ("demo-v11.atomik", 11),
    ("drum-demo-v6.atomik", 6),
];

/// Three columns: `a`, `b` in column 0 (`b` coloured), `c` in column 2, column 1 empty.
fn rack() -> (Document, atomik_device_sdk::Registry) {
    let r = plugin_console_registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [Command::AddRackColumn, Command::AddRackColumn]);
    for (k, c) in [("a", 0), ("b", 0), ("c", 2)] {
        d.apply(
            Command::AddDeviceAt {
                slot: RackSlot {
                    column: c,
                    index: usize::MAX,
                },
                type_id: DELAY.into(),
                key: Some(key(k)),
            },
            &r,
        )
        .unwrap();
    }
    d.apply(
        Command::SetColor {
            key: key("c"),
            color: Some([1, 2, 3]),
        },
        &r,
    )
    .unwrap();
    (d, r)
}

fn edit(text: &str, f: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    f(&mut v);
    v.to_string()
}

#[test]
fn the_format_is_version_11() {
    assert_eq!(PROJECT_VERSION, 36);
}

#[test]
fn a_single_column_rack_writes_neither_field() {
    let r = registry();
    let text = save_project(three_devices(&r).doc());
    assert!(text.contains("\"version\": 36"));
    assert!(!text.contains("rack_columns"));
    assert!(!text.contains("\"column\""));
}

#[test]
fn columns_are_written_in_the_fixed_field_order() {
    let (d, _) = rack();
    let text = save_project(d.doc());
    assert!(
        text.contains("\"input_channels\": 2,\n  \"rack_columns\": 3,\n  \"devices\": ["),
        "{text}"
    );
    // After `name`/`folded`/`color`, before `params` and the other optional objects.
    assert!(
        text.contains("\"folded\": false, \"color\": \"#010203\", \"column\": 2"),
        "{text}"
    );
    assert!(text.contains("\"id\": \"c\""));
    // Column 0 is the default and is never written.
    let a = text.split("\"id\": \"a\"").nth(1).unwrap();
    let a = a.split('}').next().unwrap();
    assert!(!a.contains("column"), "{a}");
}

#[test]
fn a_saved_v11_project_round_trips_exactly_with_its_empty_column() {
    let (d, r) = rack();
    let text = save_project(d.doc());
    let back = load_project(&text, &r).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(back.columns, 3);
    assert_eq!(back.column_range(1), 2..2, "the empty column survives");
    assert_eq!(save_project(&back), text, "saving is canonical");
    back.check_columns().unwrap();
}

#[test]
fn every_older_version_loads_as_one_column_in_column_zero() {
    let r = plugin_console_registry();
    for (name, version) in OLD_FIXTURES {
        let file = fixture(name);
        assert!(file.contains(&format!("\"version\": {version}")), "{name}");
        let doc = load_project(&file, &r).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(doc.columns, 1, "{name}");
        assert!(doc.devices.iter().all(|d| d.column == 0), "{name}");
        doc.check_columns().unwrap();
        let saved = save_project(&doc);
        assert!(saved.contains("\"version\": 36"), "{name}");
        assert!(
            !saved.contains("rack_columns") && !saved.contains("\"column\""),
            "{name}"
        );
        assert_eq!(load_project(&saved, &r).unwrap(), doc, "{name}");
    }
}

#[test]
fn a_v10_file_resaves_as_v11_with_only_the_version_line_changed() {
    let r = plugin_console_registry();
    let file = fixture("demo-v10.atomik");
    let doc = load_project(&file, &r).unwrap();
    assert_eq!(save_project(&doc), as_current(&file, 10));
}

#[test]
fn the_new_fields_need_version_11() {
    let (d, r) = rack();
    let text = save_project(d.doc());
    for v in 3..=10 {
        let old = text.replace("\"version\": 36", &format!("\"version\": {v}"));
        let e = load_project(&old, &r).unwrap_err().to_string();
        assert!(
            e.contains("rack_columns") || e.contains("column"),
            "v{v}: {e}"
        );
    }
    // Only a device column, no root field.
    let only_device = edit(&text, |v| {
        v.as_object_mut().unwrap().remove("rack_columns");
        v["version"] = 10.into();
    });
    let e = load_project(&only_device, &r).unwrap_err().to_string();
    assert!(e.contains("column"), "{e}");
}

#[test]
fn a_column_that_does_not_exist_is_rejected_with_its_path() {
    let (d, r) = rack();
    let text = save_project(d.doc());
    let bad = edit(&text, |v| v["rack_columns"] = 2.into());
    let e = load_project(&bad, &r).unwrap_err().to_string();
    assert!(
        e.contains("devices[2].column") && e.contains("does not exist"),
        "{e}"
    );
    // A column without any rack_columns means one column.
    let none = edit(&text, |v| {
        v.as_object_mut().unwrap().remove("rack_columns");
    });
    assert!(
        load_project(&none, &r)
            .unwrap_err()
            .to_string()
            .contains("devices[2].column")
    );
}

#[test]
fn devices_must_be_sorted_by_column() {
    let (d, r) = rack();
    let text = save_project(d.doc());
    let bad = edit(&text, |v| {
        let devices = v["devices"].as_array_mut().unwrap();
        devices.swap(0, 2);
    });
    let e = load_project(&bad, &r).unwrap_err().to_string();
    assert!(e.contains("sorted by column"), "{e}");
}

#[test]
fn rack_columns_must_be_between_one_and_the_maximum() {
    let (d, r) = rack();
    let text = save_project(d.doc());
    for bad in [
        Value::from(0),
        Value::from(MAX_RACK_COLUMNS + 1),
        Value::from(-1),
        Value::from("2"),
        Value::from(1.5),
    ] {
        let t = edit(&text, |v| v["rack_columns"] = bad.clone());
        let e = load_project(&t, &r).unwrap_err().to_string();
        assert!(e.contains("rack_columns"), "{bad}: {e}");
    }
    let max = edit(&text, |v| v["rack_columns"] = MAX_RACK_COLUMNS.into());
    assert_eq!(load_project(&max, &r).unwrap().columns, MAX_RACK_COLUMNS);
}

#[test]
fn a_device_column_must_be_a_small_integer() {
    let (d, r) = rack();
    let text = save_project(d.doc());
    for bad in [
        Value::from(-1),
        Value::from("1"),
        Value::from(1.5),
        Value::from(MAX_RACK_COLUMNS),
    ] {
        let t = edit(&text, |v| v["devices"][2]["column"] = bad.clone());
        assert!(load_project(&t, &r).is_err(), "{bad}");
    }
}

#[test]
fn a_device_inside_a_container_cannot_have_a_column() {
    let r = plugin_console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add(0, CONTAINER, Some("c")), add_inner("c", 0, DELAY, "x")],
    );
    let text = save_project(d.doc());
    assert!(
        !text.contains("\"column\""),
        "inner devices never write a column"
    );
    let bad = edit(&text, |v| {
        v["devices"][0]["inner"]["devices"][0]["column"] = 0.into()
    });
    let e = load_project(&bad, &r).unwrap_err().to_string();
    assert!(e.contains("inside a container has no column"), "{e}");
}

#[test]
fn a_container_inside_a_later_column_still_saves_its_inside_without_columns() {
    let r = plugin_console_registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [Command::AddRackColumn]);
    run(
        &mut d,
        &r,
        [
            Command::AddDeviceAt {
                slot: RackSlot {
                    column: 1,
                    index: 0,
                },
                type_id: CONTAINER.into(),
                key: Some(key("c")),
            },
            add_inner("c", 0, DELAY, "x"),
        ],
    );
    let text = save_project(d.doc());
    assert_eq!(text.matches("\"column\"").count(), 1);
    assert_eq!(&load_project(&text, &r).unwrap(), d.doc());
}

#[test]
fn patches_carry_no_columns() {
    let (d, r) = rack();
    let patch = save_patch(d.doc());
    assert!(!patch.contains("column"), "{patch}");
    let doc: RackDoc = load_patch(&patch, &r).unwrap();
    assert_eq!(doc.columns, 1);
    assert!(doc.devices.iter().all(|x| x.column == 0));
    let with = edit(&patch, |v| v["devices"][0]["column"] = 1.into());
    assert!(
        load_patch(&with, &r)
            .unwrap_err()
            .to_string()
            .contains("column")
    );
}
