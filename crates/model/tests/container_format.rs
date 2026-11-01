//! Project format v8: a container's `"inner"`, canonical text, the
//! committed golden, rejection with JSON paths, files of versions 1 to 7 still loading, and the
//! `boundary` rename of old files.

mod common;

use atomik_model::{
    Command, ConsoleCommand, Document, ModelError, PROJECT_VERSION, RackDoc, SongCommand, ZoneDoc,
    empty_doc, load_project, load_project_with_warnings, save_project,
};
use common::container_ops::{Op, assert_invariants, op, resolve_op};
use common::*;
use proptest::prelude::*;
use serde_json::{Value, json};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
    .replace("\r\n", "\n")
}

const ASSET: &str = "4242424242424242424242424242424242424242424242424242424242424242";

/// The document of the golden: a synth at the top, a container `layer` holding two synths, a
/// nested effect container `fx`, a sampler with a zone and a sequencer with a pattern; two
/// splits and two macros (one nesting).
fn project(r: &atomik_device_sdk::Registry) -> RackDoc {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            Command::RegisterAsset {
                hash: ASSET.into(),
                meta: atomik_model::AssetMeta {
                    channels: 2,
                    sample_rate: 48_000,
                    frames: 48_000,
                    original_name: "kick \"1\".wav".into(),
                },
            },
            add(0, DELAY, Some("lead")),
            add(1, CONTAINER, Some("layer")),
            add(2, DELAY, Some("echo")),
            add_inner("layer", 0, SYNTH, "lo"),
            add_inner("layer", 1, SYNTH, "hi"),
            add_inner("layer", 2, CONTAINER, "fx"),
            add_inner("layer", 3, SAMPLER, "smp"),
            add_inner("layer", 4, SEQ, "seq"),
            add_inner("layer/fx", 0, COMP, "comp"),
            add_inner("layer/fx", 1, DELAY, "delay"),
            connect("layer/fx/boundary:in_l", "layer/fx/comp:in_l"),
            connect("layer/fx/boundary:in_r", "layer/fx/comp:in_r"),
            connect("layer/fx/comp:out_l", "layer/fx/delay:in_l"),
            connect("layer/fx/comp:out_r", "layer/fx/delay:in_r"),
            connect("layer/fx/delay:out_l", "layer/fx/boundary:out_l"),
            connect("layer/fx/delay:out_r", "layer/fx/boundary:out_r"),
            connect("layer/lo:out_l", "layer/fx:in_l"),
            connect("layer/lo:out_r", "layer/fx:in_r"),
            connect("layer/fx:out_l", "layer/boundary:out_l"),
            connect("layer/fx:out_r", "layer/boundary:out_r"),
            connect("layer/seq:notes_out", "layer/hi:notes_in"),
            connect("lead:out_l", "layer:in_l"),
            connect("layer:out_l", "echo:in_l"),
            set_splits(
                "layer",
                vec![split("lo", 0, 59, 0), split("smp", 60, 127, -12)],
            ),
            set_macro("layer", 0, target("lo", "filter.cutoff", 200.0, 8000.0)),
            set_macro("layer", 0, target("hi", "filter.cutoff", 8000.0, 200.0)),
            set_macro("layer", 0, target("fx/delay", "mix", 0.0, 1.0)),
            set_macro("layer", 1, target("fx", "macro1", 0.0, 1.0)),
            set_macro("layer/fx", 0, target("comp", "threshold", -40.0, -10.0)),
            Command::Container(atomik_model::ContainerCommand::RenameMacro {
                container: key("layer"),
                index: 0,
                name: "Tone \"é\"".into(),
            }),
            set_param("layer", "macro1", 0.3),
            set_param("layer/lo", "osc1.semi", -12.0),
            Command::Rename {
                key: key("layer/hi"),
                name: "High".into(),
            },
            Command::SetColor {
                key: key("layer/hi"),
                color: Some([0xaa, 0xbb, 0xcc]),
            },
            Command::SetFolded {
                key: key("layer/fx"),
                folded: true,
            },
            Command::SetZones {
                device: key("layer/smp"),
                zones: vec![ZoneDoc::new(ASSET, 48_000)],
                merge: None,
            },
            set_param("layer/seq", "pattern", 1.0),
        ],
    );
    let mut doc = d.doc().clone();
    // A pattern inside the sequencer.
    let seq = &mut doc
        .devices
        .iter_mut()
        .find(|d| d.key == key("layer"))
        .unwrap()
        .inner
        .as_mut()
        .unwrap()
        .devices[4];
    seq.steps.as_mut().unwrap().patterns[0].steps[2].on = true;
    doc
}

fn text(r: &atomik_device_sdk::Registry) -> String {
    save_project(&project(r))
}

fn rejects(text: &str, r: &atomik_device_sdk::Registry, path: &str, needle: &str) {
    let e = load_project(text, r).expect_err("must be rejected");
    let ModelError::Load { path: at, message } = e else {
        panic!("not a load error: {e}");
    };
    let msg = &format!("{at}: {message}");
    assert!(
        msg.starts_with(&format!("{path}: ")),
        "{msg} lacks path {path}"
    );
    assert!(msg.contains(needle), "{msg} lacks {needle:?}");
}

fn edited(text: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    edit(&mut v);
    v.to_string()
}

#[test]
fn the_format_is_version_11() {
    assert_eq!(PROJECT_VERSION, 39);
}

#[test]
fn inner_is_written_inside_the_container_after_steps() {
    let r = registry();
    let t = text(&r);
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(v["version"], 39);
    let devices = v["devices"].as_array().unwrap();
    assert!(
        devices[0].get("inner").is_none(),
        "only containers have an inside"
    );
    assert!(devices[2].get("inner").is_none());
    let inner = &devices[1]["inner"];
    let mut keys: Vec<&String> = inner.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["cables", "devices", "macros", "splits"]);
    let inside = inner["devices"].as_array().unwrap();
    assert_eq!(inside.len(), 5);
    assert_eq!(inside[0]["id"], "lo", "inner keys are local");
    assert_eq!(inside[1]["name"], "High");
    assert_eq!(inside[1]["color"], "#aabbcc");
    assert_eq!(inside[2]["inner"]["devices"].as_array().unwrap().len(), 2);
    assert!(inside[3]["zones"].as_array().unwrap().len() == 1);
    assert!(inside[4].get("steps").is_some());
    // Cables use local keys; the boundary is `boundary`.
    let cables = inner["cables"].as_array().unwrap();
    assert!(cables.contains(&json!({"from": "fx:out_l", "to": "boundary:out_l"})));
    assert!(cables.contains(&json!({"from": "lo:out_l", "to": "fx:in_l"})));
    // Always 8 macros, each with its name and targets.
    let macros = inner["macros"].as_array().unwrap();
    assert_eq!(macros.len(), 8);
    assert_eq!(macros[0]["name"], "Tone \"é\"");
    let hi = &macros[0]["targets"][1];
    assert_eq!(hi["device"], "hi");
    assert_eq!(hi["param"], "filter.cutoff");
    assert_eq!(hi["min"].as_f64(), Some(8000.0));
    assert_eq!(hi["max"].as_f64(), Some(200.0));
    assert_eq!(macros[0]["targets"][2]["device"], "fx/delay");
    assert_eq!(macros[1]["targets"][0]["param"], "macro1");
    assert_eq!(macros[2], json!({"name": "", "targets": []}));
    assert_eq!(
        inner["splits"],
        json!([
            {"target": "lo", "key_lo": 0, "key_hi": 59, "vel_lo": 1, "vel_hi": 127, "transpose": 0},
            {"target": "smp", "key_lo": 60, "key_hi": 127, "vel_lo": 1, "vel_hi": 127, "transpose": -12}
        ])
    );
    // An empty container still writes its eight macros.
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CONTAINER, Some("c")), &r).unwrap();
    let t = save_project(d.doc());
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(
        v["devices"][0]["inner"]["macros"].as_array().unwrap().len(),
        8
    );
    assert_eq!(v["devices"][0]["inner"]["devices"], json!([]));
    assert_eq!(v["devices"][0]["inner"]["splits"], json!([]));
}

#[test]
fn save_load_save_is_canonical_and_exact() {
    let r = registry();
    let doc = project(&r);
    let t = save_project(&doc);
    let back = load_project(&t, &r).unwrap();
    assert_eq!(back, doc);
    assert_eq!(save_project(&back), t);
    assert!(t.is_ascii() || t.contains('é'));
}

#[test]
fn a_document_without_containers_has_no_inner_anywhere() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add(0, SYNTH, Some("s")), add(1, SEQ, Some("q"))],
    );
    let t = save_project(d.doc());
    assert!(!t.contains("inner"));
    assert!(t.contains("\"version\": 39"));
    assert_eq!(load_project(&t, &r).unwrap(), *d.doc());
}

#[test]
fn the_committed_golden_is_the_canonical_save() {
    let r = registry();
    let doc = project(&r);
    let t = save_project(&doc);
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/container-v8.atomik");
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, as_old(&t, 8)).unwrap();
    }
    // The golden is the v8 file: it differs from the current save in the version number only.
    let file = fixture("container-v8.atomik");
    assert_eq!(
        as_current(&file, 8),
        t,
        "regenerate with ATOMIK_UPDATE_GOLDEN=1"
    );
    assert_eq!(load_project(&file, &r).unwrap(), doc);
}

#[test]
fn versions_one_to_seven_still_load_and_resave_as_v11() {
    let r = registry();
    let cr = console_registry();
    for (name, reg, from) in [
        ("demo-v1.atomik", &cr, 1),
        ("demo-v6.atomik", &cr, 6),
        ("drum-demo-v6.atomik", &cr, 6),
        ("seq-v7.atomik", &r, 7),
    ] {
        let t = fixture(name);
        let doc = load_project(&t, reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let again = save_project(&doc);
        assert!(again.contains("\"version\": 39"), "{name}");
        assert_eq!(load_project(&again, reg).unwrap(), doc, "{name}");
        assert_eq!(save_project(&load_project(&again, reg).unwrap()), again);
        if from >= 6 {
            // Only the version number differs.
            assert_eq!(again, as_current(&t, from), "{name}");
        }
    }
}

// ---- rejection -------------------------------------------------------------------------

#[test]
fn inner_needs_version_8_and_a_container() {
    let r = registry();
    let good = text(&r);
    rejects(
        &edited(&good, |v| v["version"] = json!(7)),
        &r,
        "devices[1].inner",
        "unknown field",
    );
    rejects(
        &edited(&good, |v| {
            v["devices"][0]["inner"] = v["devices"][1]["inner"].clone()
        }),
        &r,
        "devices[0].inner",
        "does not have an inside",
    );
    // A container without `inner` is a container with an empty inside.
    let t = edited(&good, |v| {
        v["devices"][1].as_object_mut().unwrap().remove("inner");
    });
    let doc = load_project(&t, &r).unwrap();
    assert!(doc.devices[1].inner.as_ref().unwrap().devices.is_empty());
}

#[test]
fn structure_violations_name_their_json_path() {
    let r = console_registry();
    let good = text(&r);
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(Edit, &str, &str)> = vec![
        (
            Box::new(|v| {
                // Depth 4: layer > fx > deep > deeper.
                let deeper = json!({"id": "deeper", "type": CONTAINER, "name": "d", "folded": false,
                    "inner": {"devices": [], "cables": [], "macros": macros8(), "splits": []}});
                let deep = json!({"id": "deep", "type": CONTAINER, "name": "d", "folded": false,
                    "inner": {"devices": [deeper], "cables": [], "macros": macros8(), "splits": []}});
                v["devices"][1]["inner"]["devices"][2]["inner"]["devices"]
                    .as_array_mut()
                    .unwrap()
                    .push(deep);
            }),
            "devices[1].inner.devices[2].inner.devices[2].inner.devices[0]",
            "nest",
        ),
        (
            Box::new(|v| {
                let list = v["devices"][1]["inner"]["devices"].as_array_mut().unwrap();
                let template = list[0].clone();
                for i in 0..28 {
                    let mut d = template.clone();
                    d["id"] = json!(format!("extra{i}"));
                    list.push(d);
                }
            }),
            "devices[1].inner.devices",
            "32",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["devices"][1]["id"] = json!("lo")),
            "devices[1].inner.devices[1].id",
            "duplicate",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["devices"][1]["id"] = json!("boundary")),
            "devices[1].inner.devices[1].id",
            "reserved",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["devices"][1]["id"] = json!("a/b")),
            "devices[1].inner.devices[1].id",
            "[a-z]",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["devices"][1]["id"] = json!("interface")),
            "devices[1].inner.devices[1].id",
            "reserved",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["devices"][0]["type"] = json!("atomik.channel")),
            "devices[1].inner.devices[0].type",
            "not allowed",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["devices"][0]["type"] = json!(SPLIT)),
            "devices[1].inner.devices[0].type",
            "not allowed",
        ),
        (
            Box::new(|v| {
                v["devices"][1]["inner"]["devices"][0]["zones"] = json!([{
                    "asset": ASSET, "key_lo": 0, "key_hi": 127, "vel_lo": 1, "vel_hi": 127,
                    "root_key": 60, "transpose": 0, "tune_cents": 0.0, "gain_db": 0.0, "pan": 0.0,
                    "start": 0, "end": 48000, "loop_mode": "off", "loop_start": 0,
                    "loop_end": 48000, "crossfade": 0, "trigger": "gate", "reverse": false
                }]);
            }),
            "devices[1].inner.devices[0].zones",
            "does not take samples",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["devices"][0]["steps"] = json!({"patterns": []})),
            "devices[1].inner.devices[0].steps",
            "does not take pattern data",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["cables"][0]["from"] = json!("nope:out_l")),
            "devices[1].inner.cables[0].from",
            "no device has id",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["cables"][0]["from"] = json!("fx/comp:out_l")),
            "devices[1].inner.cables[0].from",
            "no device has id",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["cables"][0]["to"] = json!("lo:nope")),
            "devices[1].inner.cables[0].to",
            "no socket",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["cables"][0]["to"] = json!("boundary:in_l")),
            "devices[1].inner.cables[0].to",
            "output",
        ),
        (
            Box::new(|v| {
                // The boundary of a nested container used inside its parent.
                v["devices"][1]["inner"]["cables"][0]["to"] = json!("fx/boundary:out_l");
            }),
            "devices[1].inner.cables[0].to",
            "no device has id",
        ),
        (
            Box::new(|v| {
                let c = v["devices"][1]["inner"]["cables"][0].clone();
                v["devices"][1]["inner"]["cables"]
                    .as_array_mut()
                    .unwrap()
                    .push(c);
            }),
            "devices[1].inner.cables[",
            "already connected",
        ),
    ];
    for (edit, path, needle) in cases {
        let t = edited(&good, edit);
        let e = load_project(&t, &r).expect_err("must be rejected");
        let msg = e.to_string();
        assert!(msg.contains(path), "{msg} lacks path {path}");
        assert!(msg.contains(needle), "{msg} lacks {needle:?}");
    }
}

fn macros8() -> Value {
    Value::Array(vec![json!({"name": "", "targets": []}); 8])
}

#[test]
fn split_and_macro_violations_name_their_json_path() {
    let r = registry();
    let good = text(&r);
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(Edit, &str, &str)> = vec![
        (
            Box::new(|v| v["devices"][1]["inner"]["splits"][0]["target"] = json!("nope")),
            "devices[1].inner.splits[0].target",
            "no device",
        ),
        (
            // `fx` is a container: it has a notes input; `comp` is inside it, not here. A delay
            // has none.
            Box::new(|v| v["devices"][1]["inner"]["splits"][0]["target"] = json!("fx/comp")),
            "devices[1].inner.splits[0].target",
            "no device",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["splits"][1]["target"] = json!("lo")),
            "devices[1].inner.splits[1].target",
            "already",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["splits"][0]["key_lo"] = json!(60)),
            "devices[1].inner.splits[0]",
            "key_lo",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["splits"][0]["key_hi"] = json!(128)),
            "devices[1].inner.splits[0].key_hi",
            "128",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["splits"][0]["vel_lo"] = json!(0)),
            "devices[1].inner.splits[0].vel_lo",
            "1..=127",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["splits"][0]["transpose"] = json!(49)),
            "devices[1].inner.splits[0].transpose",
            "-48..=48",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["splits"][0]["extra"] = json!(1)),
            "devices[1].inner.splits[0].extra",
            "unknown field",
        ),
        (
            Box::new(|v| {
                let list = v["devices"][1]["inner"]["splits"].as_array_mut().unwrap();
                for _ in 0..7 {
                    let s = list[0].clone();
                    list.push(s);
                }
            }),
            "devices[1].inner.splits",
            "8",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["macros"] = json!([])),
            "devices[1].inner.macros",
            "8",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["macros"][2]["name"] = json!("x".repeat(25))),
            "devices[1].inner.macros[2].name",
            "24",
        ),
        (
            Box::new(|v| {
                v["devices"][1]["inner"]["macros"][0]["targets"][0]["device"] = json!("zz")
            }),
            "devices[1].inner.macros[0].targets[0].device",
            "no device",
        ),
        (
            Box::new(|v| {
                v["devices"][1]["inner"]["macros"][0]["targets"][0]["param"] = json!("zz")
            }),
            "devices[1].inner.macros[0].targets[0].param",
            "no param",
        ),
        (
            // The same param driven twice.
            Box::new(|v| {
                let t = v["devices"][1]["inner"]["macros"][0]["targets"][0].clone();
                v["devices"][1]["inner"]["macros"][3]["targets"] = json!([t]);
            }),
            "devices[1].inner.macros[3].targets[0]",
            "already driven",
        ),
        (
            Box::new(|v| {
                let t = v["devices"][1]["inner"]["macros"][0]["targets"][0].clone();
                let list = v["devices"][1]["inner"]["macros"][4]["targets"]
                    .as_array_mut()
                    .unwrap();
                for _ in 0..33 {
                    list.push(t.clone());
                }
            }),
            "devices[1].inner.macros[4].targets",
            "32",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["macros"][0]["targets"][0]["min"] = json!("a")),
            "devices[1].inner.macros[0].targets[0].min",
            "number",
        ),
        (
            // A container target must name its macro param, nothing else of a container.
            Box::new(|v| {
                v["devices"][1]["inner"]["macros"][1]["targets"][0]["param"] = json!("mix")
            }),
            "devices[1].inner.macros[1].targets[0].param",
            "no param",
        ),
        (
            Box::new(|v| v["devices"][1]["inner"]["macros"][0]["extra"] = json!(1)),
            "devices[1].inner.macros[0].extra",
            "unknown field",
        ),
    ];
    for (edit, path, needle) in cases {
        let t = edited(&good, edit);
        rejects(&t, &r, path, needle);
    }
}

#[test]
fn project_wide_and_cycle_rules_apply_at_load() {
    let r = registry();
    // 256 inner devices in the project.
    let mut d = Document::new(empty_doc());
    for c in 0..9 {
        d.apply(add(c, CONTAINER, Some(&format!("c{c}"))), &r)
            .unwrap();
    }
    for c in 0..8 {
        for i in 0..32 {
            d.apply(add_inner(&format!("c{c}"), i, DELAY, &format!("d{i}")), &r)
                .unwrap();
        }
    }
    let good = save_project(d.doc());
    assert!(load_project(&good, &r).is_ok());
    let t = edited(&good, |v| {
        v["devices"][8]["inner"]["devices"] =
            json!([{"id": "d", "type": DELAY, "name": "n", "folded": false}]);
    });
    rejects(&t, &r, "devices", "256");
    // A loop through the expanded graph.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, DELAY, Some("pre")),
            add(1, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "fx"),
            connect("c/boundary:in_l", "c/fx:in_l"),
            connect("c/fx:out_l", "c/boundary:out_l"),
            connect("pre:out_l", "c:in_l"),
        ],
    );
    let good = save_project(d.doc());
    let t = edited(&good, |v| {
        v["cables"]
            .as_array_mut()
            .unwrap()
            .push(json!({"from": "c:out_l", "to": "pre:in_l"}));
    });
    let msg = load_project(&t, &r).unwrap_err().to_string();
    assert!(msg.contains("cycle"), "{msg}");
    // A cycle among inner cables alone.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "a"),
            add_inner("c", 1, DELAY, "b"),
            connect("c/a:out_l", "c/b:in_l"),
        ],
    );
    let good = save_project(d.doc());
    let t = edited(&good, |v| {
        v["devices"][0]["inner"]["cables"]
            .as_array_mut()
            .unwrap()
            .push(json!({"from": "b:out_l", "to": "a:in_l"}));
    });
    let msg = load_project(&t, &r).unwrap_err().to_string();
    assert!(msg.contains("devices[0].inner.cables"), "{msg}");
    assert!(msg.contains("cycle"), "{msg}");
    // The reserved key at the top level, and the internal split device.
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("synth")), &r).unwrap();
    let good = save_project(d.doc());
    rejects(
        &edited(&good, |v| v["devices"][0]["id"] = json!("boundary")),
        &r,
        "devices[0].id",
        "reserved",
    );
    rejects(
        &edited(&good, |v| v["devices"][0]["type"] = json!(SPLIT)),
        &r,
        "devices[0].type",
        "not allowed",
    );
}

#[test]
fn inner_zones_are_checked_against_the_pool() {
    let r = registry();
    let good = text(&r);
    rejects(
        &edited(&good, |v| {
            v["devices"][1]["inner"]["devices"][3]["zones"][0]["asset"] = json!("ab".repeat(32));
        }),
        &r,
        "devices[1].inner.devices[3].zones[0]",
        "assets pool",
    );
    rejects(
        &edited(&good, |v| {
            v["devices"][1]["inner"]["devices"][3]["zones"][0]["end"] = json!(99_999_999);
        }),
        &r,
        "devices[1].inner.devices[3].zones[0]",
        "",
    );
    // A project with inner zones keeps the asset in `referenced_assets`.
    let doc = load_project(&good, &r).unwrap();
    assert!(doc.referenced_assets().contains(ASSET));
}

// ---- the `boundary` rename ---------------------------------------------------------------

/// A v7 project whose synth is keyed `boundary` (a key the new format reserves), cabled, with a
/// track and a console channel.
fn old_boundary_project(reg: &atomik_device_sdk::Registry) -> String {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        reg,
        [
            add(0, SYNTH, Some("bnd")),
            add(1, DELAY, Some("fx")),
            connect("bnd:out_l", "fx:in_l"),
            Command::Song(SongCommand::AddTrack { device: key("bnd") }),
        ],
    );
    let t = save_project(d.doc());
    // Rename every use of `bnd` and make it a v7 file.
    edited(&t, |v| {
        v["version"] = json!(7);
        let s = serde_json::to_string(v)
            .unwrap()
            .replace("\"bnd\"", "\"boundary\"")
            .replace("\"bnd:", "\"boundary:");
        *v = serde_json::from_str(&s).unwrap();
    })
}

#[test]
fn a_device_keyed_boundary_in_an_old_file_is_renamed_with_a_warning() {
    let r = registry();
    let t = old_boundary_project(&r);
    assert!(t.contains("\"boundary\""));
    let (doc, warnings) = load_project_with_warnings(&t, &r).unwrap();
    let keys: Vec<_> = doc.devices.iter().map(|d| d.key.0.as_str()).collect();
    assert_eq!(keys, ["boundary-1", "fx"]);
    assert_eq!(doc.cables[0].from, sock("boundary-1:out_l"));
    assert_eq!(doc.song.tracks[0].device, key("boundary-1"));
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].contains("boundary") && warnings[0].contains("boundary-1"),
        "{warnings:?}"
    );
    // The plain loader gives the same document.
    assert_eq!(load_project(&t, &r).unwrap(), doc);
    // Saving writes the new key and a v8 file.
    let saved = save_project(&doc);
    assert!(saved.contains("boundary-1") && !saved.contains("\"boundary\""));
    assert!(load_project_with_warnings(&saved, &r).unwrap().1.is_empty());
}

#[test]
fn the_rename_avoids_taken_keys_and_follows_console_references() {
    let r = console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, DELAY, Some("bnd")),
            add(1, DELAY, Some("boundary-1")),
            Command::Console(ConsoleCommand::AddChannel { source: key("bnd") }),
            Command::Console(ConsoleCommand::SetReturn {
                slot: 0,
                effect: Some(key("boundary-1")),
            }),
        ],
    );
    let t = edited(&save_project(d.doc()), |v| {
        v["version"] = json!(6);
        // Strip v7+ only content: none here.
        let s = serde_json::to_string(v)
            .unwrap()
            .replace("\"bnd\"", "\"boundary\"")
            .replace("\"bnd:", "\"boundary:");
        *v = serde_json::from_str(&s).unwrap();
        // v6 zones have `reverse`; nothing else changed.
    });
    let (doc, warnings) = load_project_with_warnings(&t, &r).unwrap();
    assert!(doc.device(&key("boundary-2")).is_some());
    assert!(doc.device(&key("boundary-1")).is_some());
    assert_eq!(doc.console.channels[0].source, Some(key("boundary-2")));
    assert_eq!(doc.console.returns[0], Some(key("boundary-1")));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("boundary-2"), "{warnings:?}");
}

#[test]
fn boundary_is_reserved_in_a_v8_file() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("bnd")), &r).unwrap();
    let t = edited(&save_project(d.doc()), |v| {
        v["devices"][0]["id"] = json!("boundary")
    });
    rejects(&t, &r, "devices[0].id", "reserved");
}

// ---- properties -------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_reachable_document_saves_and_loads_canonically(ops in proptest::collection::vec(op(), 1..50)) {
        let r = registry();
        let mut d = Document::new(empty_doc());
        for o in &ops {
            match o {
                Op::Undo => { let _ = d.undo(&r); }
                Op::Redo => { let _ = d.redo(&r); }
                _ => {
                    if let Some(c) = resolve_op(o, d.doc(), &r) {
                        let _ = d.apply(c, &r);
                    }
                }
            }
        }
        assert_invariants(d.doc(), &r);
        let t = save_project(d.doc());
        let back = load_project(&t, &r).unwrap_or_else(|e| panic!("{e}\n{t}"));
        prop_assert_eq!(&back, d.doc());
        prop_assert_eq!(save_project(&back), t);
    }
}

#[test]
fn an_old_project_without_the_effect_bypass_param_loads_with_bypass_off() {
    // The v8 golden predates the `bypass` param of built-in effects: its compressor stores no
    // value for it, so it takes the descriptor default and the file saves back unchanged.
    let r = registry();
    let t = fixture("container-v8.atomik");
    let doc = load_project(&t, &r).unwrap();
    let comp = key("layer/fx/comp");
    let device = doc.resolve(&comp).unwrap();
    assert!(!device.params.contains_key("bypass"));
    assert_eq!(doc.param_value(&comp, "bypass", &r), Some(0.0));
    assert_eq!(save_project(&doc), as_current(&t, 8));
}
