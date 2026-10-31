//! Patch v1/v2 I/O (model half): load v1 and v2, save v2, round trips,
//! JSON-path errors.

mod common;

use atomik_model::{Command, Document, ModelError, RackDoc, empty_doc, load_patch, save_patch};
use common::*;
use proptest::prelude::*;
use serde_json::{Value, json};

fn example(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/patches")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn edited(text: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    edit(&mut v);
    v.to_string()
}

fn assert_rejects(text: &str, path: &str, needle: &str) {
    let e = load_patch(text, &registry()).expect_err("patch must be rejected");
    let ModelError::Load { path: at, message } = &e else {
        panic!("expected a load error, got {e:?}")
    };
    let msg = &format!("{at}: {message}");
    assert!(
        msg.starts_with(&format!("{path}: ")),
        "{msg} (wanted path {path})"
    );
    assert!(msg.contains(needle), "{msg} lacks {needle:?}");
}

#[test]
fn load_v1_slice_demo_uses_layout_defaults() {
    let reg = registry();
    let doc = load_patch(&example("slice-demo.json"), &reg).unwrap();
    assert_eq!(doc.sample_rate, 48_000);
    assert!((doc.tempo_bpm - 120.0).abs() < 1e-12);
    assert_eq!((doc.seed, doc.output_channels), (1, 2));
    let keys: Vec<_> = doc.devices.iter().map(|d| d.key.as_str()).collect();
    assert_eq!(keys, ["bass", "echo", "lead", "mixer"]);
    let names: Vec<_> = doc.devices.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        ["Poly Synth", "Stereo Delay", "Poly Synth 2", "Line Mixer"]
    );
    assert!(doc.devices.iter().all(|d| !d.folded && d.color.is_none()));
    assert_eq!(doc.devices[0].type_id, SYNTH);
    assert_eq!(doc.devices[0].params.len(), 20);
    assert_eq!(doc.devices[1].params["mix"].to_bits(), 0.22f32.to_bits());
    assert_eq!(doc.cables.len(), 8);
    let mut sorted = doc.cables.clone();
    sorted.sort();
    assert_eq!(doc.cables, sorted, "cables are canonical after loading");
    assert!(
        doc.cables
            .contains(&cable("mixer:master_r", "interface:out_2"))
    );
}

#[test]
fn load_clamps_params_through_descriptor() {
    let text = edited(&example("minimal.json"), |v| {
        v["devices"][0]["params"]["filter.cutoff"] = json!(50_000);
        v["devices"][0]["params"]["osc1.wave"] = json!(1.4);
    });
    let doc = load_patch(&text, &registry()).unwrap();
    assert_eq!(
        doc.devices[0].params["filter.cutoff"].to_bits(),
        20_000f32.to_bits()
    );
    assert_eq!(doc.devices[0].params["osc1.wave"].to_bits(), 1f32.to_bits());
}

#[test]
fn save_writes_v2_header_and_rack_order() {
    let reg = registry();
    let doc = load_patch(&example("slice-demo.json"), &reg).unwrap();
    let text = save_patch(&doc);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["format"], "atomik-patch");
    assert_eq!(v["version"], 2);
    assert_eq!(v["sample_rate"], 48_000);
    assert_eq!(v["tempo_bpm"], 120);
    assert_eq!(v["seed"], 1);
    assert_eq!(v["output_channels"], 2);
    assert!(
        v["length_beats"].as_f64().unwrap() > 0.0,
        "renderable: v1 fields present"
    );
    assert!(v.get("notes").is_none(), "notes are not part of the model");
    let ids: Vec<_> = v["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].clone())
        .collect();
    assert_eq!(
        ids,
        [json!("bass"), json!("echo"), json!("lead"), json!("mixer")]
    );
    assert_eq!(v["devices"][2]["name"], "Poly Synth 2");
    assert_eq!(v["devices"][2]["folded"], false);
    assert!(
        v["devices"][2].get("color").is_none(),
        "no colour, no field"
    );
    assert_eq!(
        v["devices"][1]["params"]["mix"],
        json!(0.22),
        "shortest f32 text"
    );
    assert_eq!(v["cables"].as_array().unwrap().len(), 8);
    // Saving is deterministic.
    assert_eq!(text, save_patch(&doc));
}

#[test]
fn ac_u2_v1_load_save_load_is_identical() {
    let reg = registry();
    for name in ["slice-demo.json", "minimal.json"] {
        let doc = load_patch(&example(name), &reg).unwrap();
        let again = load_patch(&save_patch(&doc), &reg).unwrap();
        assert_eq!(again, doc, "{name}");
    }
}

#[test]
fn v2_layout_round_trips() {
    let reg = registry();
    let mut d = Document::new(load_patch(&example("slice-demo.json"), &reg).unwrap());
    let cmds = [
        Command::Rename {
            key: key("echo"),
            name: "Ping \"Pong\" \\ é ✓".into(),
        },
        Command::SetFolded {
            key: key("lead"),
            folded: true,
        },
        Command::SetColor {
            key: key("mixer"),
            color: Some([0, 128, 255]),
        },
        Command::MoveDevice {
            key: key("mixer"),
            to: 0,
        },
        Command::SetTempo(97.25),
    ];
    for c in cmds {
        d.apply(c, &reg).unwrap();
    }
    let text = save_patch(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["devices"][0]["color"], "#0080ff");
    assert_eq!(load_patch(&text, &reg).unwrap(), *d.doc());
}

#[test]
fn empty_doc_round_trips() {
    let reg = registry();
    // Patches carry no inputs, no pool and no audio: they load with `input_channels` 0.
    let doc = RackDoc {
        input_channels: 0,
        ..empty_doc()
    };
    assert_eq!(load_patch(&save_patch(&doc), &reg).unwrap(), doc);
}

#[test]
fn v2_notes_are_accepted_and_ignored() {
    let reg = registry();
    let v1 = example("slice-demo.json");
    let v2 = edited(&v1, |v| v["version"] = json!(2));
    assert_eq!(
        load_patch(&v2, &reg).unwrap(),
        load_patch(&v1, &reg).unwrap()
    );
}

#[test]
fn v2_example_file_matches_v1() {
    let reg = registry();
    let v1 = load_patch(&example("slice-demo.json"), &reg).unwrap();
    let v2 = load_patch(&example("slice-demo.v2.json"), &reg).unwrap();
    assert_eq!(v2.cables, v1.cables);
    assert_eq!(
        (v2.sample_rate, v2.seed, v2.output_channels),
        (v1.sample_rate, v1.seed, v1.output_channels)
    );
    assert_eq!(v2.tempo_bpm.to_bits(), v1.tempo_bpm.to_bits());
    for (a, b) in v2.devices.iter().zip(&v1.devices) {
        assert_eq!(
            (&a.key, &a.type_id, &a.params),
            (&b.key, &b.type_id, &b.params)
        );
    }
    assert!(
        v2.devices.iter().any(|d| d.folded || d.color.is_some()),
        "v2 carries layout"
    );
}

#[test]
fn load_rejects_bad_header_with_paths() {
    let base = example("minimal.json");
    let cases: [(&str, Value, &str); 10] = [
        ("format", json!("other"), "atomik-patch"),
        ("version", json!(3), "version 3"),
        ("version", json!(0), "version 0"),
        ("sample_rate", json!(22050), "44100, 48000 or 96000"),
        ("tempo_bpm", json!(19.5), "20..=999"),
        ("tempo_bpm", json!("fast"), "expected a number"),
        ("length_beats", json!(0), "greater than 0"),
        ("output_channels", json!(0), "1..=32"),
        ("output_channels", json!(33), "1..=32"),
        ("seed", json!(-1), "unsigned integer"),
    ];
    for (field, value, needle) in cases {
        assert_rejects(&edited(&base, |v| v[field] = value), field, needle);
    }
    assert_rejects("{ oops", "$", "invalid JSON");
    assert_rejects("[]", "$", "expected an object");
    assert_rejects(
        &edited(&base, |v| v["colour"] = json!(1)),
        "colour",
        "unknown field",
    );
    assert_rejects(
        &edited(&base, |v| {
            v.as_object_mut().unwrap().remove("devices");
        }),
        "devices",
        "missing field",
    );
}

#[test]
fn layout_fields_are_v2_only_and_typed() {
    let base = example("minimal.json");
    for field in ["name", "folded", "color"] {
        let text = edited(&base, |v| v["devices"][0][field] = json!(1));
        assert_rejects(&text, &format!("devices[0].{field}"), "unknown field");
    }
    let v2 = edited(&base, |v| v["version"] = json!(2));
    let cases: [(&str, Value, &str); 5] = [
        ("name", json!(1), "expected a string"),
        ("folded", json!("yes"), "expected a boolean"),
        ("color", json!("#12345"), "#rrggbb"),
        ("color", json!("#gg0000"), "#rrggbb"),
        ("color", json!([1, 2, 3]), "#rrggbb"),
    ];
    for (field, value, needle) in cases {
        let text = edited(&v2, |v| v["devices"][0][field] = value);
        assert_rejects(&text, &format!("devices[0].{field}"), needle);
    }
    let ok = edited(&v2, |v| {
        v["devices"][0]["color"] = json!("#A0b1C2");
        v["devices"][0]["name"] = json!("");
    });
    let doc = load_patch(&ok, &registry()).unwrap();
    assert_eq!(doc.devices[0].color, Some([0xa0, 0xb1, 0xc2]));
    assert_eq!(doc.devices[0].name, "");
    let null_color = edited(&v2, |v| v["devices"][0]["color"] = Value::Null);
    assert_eq!(
        load_patch(&null_color, &registry()).unwrap().devices[0].color,
        None
    );
}

#[test]
fn load_rejects_bad_devices_with_paths() {
    let base = example("slice-demo.json");
    for bad in ["Synth", "1x", "", "interface"] {
        let text = edited(&base, |v| v["devices"][1]["id"] = json!(bad));
        assert_rejects(
            &text,
            "devices[1].id",
            if bad == "interface" {
                "reserved"
            } else {
                "[a-z][a-z0-9_-]*"
            },
        );
    }
    let text = edited(&base, |v| v["devices"][2]["id"] = json!("bass"));
    assert_rejects(&text, "devices[2].id", "devices[0].id");
    let text = edited(&base, |v| v["devices"][1]["type"] = json!("atomik.flanger"));
    assert_rejects(&text, "devices[1].type", "atomik.flanger");
    let text = edited(&base, |v| v["devices"][1]["params"]["mixx"] = json!(1));
    assert_rejects(&text, "devices[1].params.mixx", "no param");
    let text = edited(&base, |v| {
        v["devices"][0]["params"]["filter.cutoff"] = json!("hi")
    });
    assert_rejects(
        &text,
        "devices[0].params.\"filter.cutoff\"",
        "expected a number",
    );
}

#[test]
fn load_rejects_bad_cables_with_paths() {
    let base = example("slice-demo.json");
    let cable = |i: usize, end: &'static str, value: &'static str| {
        edited(&base, move |v| v["cables"][i][end] = json!(value))
    };
    let cases = [
        (
            cable(0, "from", "bass-out_l"),
            "cables[0].from",
            "device:socket",
        ),
        (cable(0, "from", "nope:out_l"), "cables[0].from", "nope"),
        (cable(0, "from", "bass:out_x"), "cables[0].from", "out_x"),
        (cable(6, "to", "interface:out_3"), "cables[6].to", "out_3"),
        (
            cable(0, "from", "bass:notes_in"),
            "cables[0].from",
            "is an input",
        ),
        (cable(0, "to", "echo:out_l"), "cables[0].to", "is an output"),
        (cable(0, "to", "echo:time_cv_in"), "cables[0]", "audio"),
        (
            cable(1, "to", "echo:in_l"),
            "cables[1].to",
            "already connected",
        ),
        (cable(0, "from", "mixer:master_l"), "cables[2]", "cycle"),
    ];
    for (text, path, needle) in cases {
        assert_rejects(&text, path, needle);
    }
}

proptest! {
    /// Any params, names, colours and tempo survive save → load bit-exactly.
    #[test]
    fn save_load_round_trips_values(
        values in prop::collection::vec(any::<f32>(), 6),
        name in "\\PC{0,12}",
        color in proptest::option::of(any::<[u8; 3]>()),
        tempo in 20f64..=999f64,
        seed in any::<u64>(),
    ) {
        let reg = registry();
        let mut d = chain(&reg);
        let params = [("synth", "filter.cutoff"), ("synth", "osc.mix"), ("delay", "mix"),
                      ("delay", "feedback"), ("mixer", "ch1.pan"), ("mixer", "master.level")];
        for ((k, p), v) in params.iter().zip(values) {
            if !v.is_nan() {
                d.apply(Command::SetParam { key: key(k), param: (*p).into(), value: v, merge: None }, &reg).unwrap();
            }
        }
        d.apply(Command::Rename { key: key("delay"), name }, &reg).unwrap();
        d.apply(Command::SetColor { key: key("synth"), color }, &reg).unwrap();
        d.apply(Command::SetTempo(tempo), &reg).unwrap();
        let mut doc = d.doc().clone();
        doc.seed = seed;
        doc.input_channels = 0; // patches carry no inputs
        let again = load_patch(&save_patch(&doc), &reg).unwrap();
        prop_assert_eq!(again, doc);
    }
}
