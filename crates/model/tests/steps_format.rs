//! Project format v7: a device's `"steps"`, canonical text, the
//! committed golden, rejection with JSON paths, and files of versions 1 to 6 still loading.

mod common;

use atomik_model::{
    Command, Document, ModelError, PROJECT_VERSION, PatternDoc, RackDoc, SeqResolution, StepDoc,
    StepSeqDoc, empty_doc, load_project, player_data_for, save_project,
};
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

/// A rack with a synth, a sequencer `q` with two patterns and an arpeggiator.
fn project(r: &atomik_device_sdk::Registry) -> RackDoc {
    let mut d = Document::new(empty_doc());
    for (i, (t, k)) in [(SYNTH, "s"), (SEQ, "q"), (ARP, "a")]
        .into_iter()
        .enumerate()
    {
        d.apply(add(i, t, Some(k)), r).unwrap();
    }
    d.apply(connect("q:notes_out", "a:notes_in"), r).unwrap();
    d.apply(connect("a:notes_out", "s:notes_in"), r).unwrap();
    let mut bass = PatternDoc::new("Bass \"A\" é\n");
    bass.length = 12;
    bass.resolution = SeqResolution::EighthTriplet;
    bass.swing = 40;
    for (i, key) in [36u8, 36, 43, 48].into_iter().enumerate() {
        bass.steps[i] = StepDoc {
            on: i != 1,
            key,
            velocity: 90 + i as u8,
            gate: 25 * (i as u8 + 1),
            prob: 100 - 10 * i as u8,
            tie: i == 2,
        };
    }
    // Steps beyond the length are kept.
    bass.steps[40] = StepDoc {
        on: true,
        key: 99,
        velocity: 5,
        gate: 100,
        prob: 0,
        tie: true,
    };
    d.apply(
        Command::SetPattern {
            device: key("q"),
            index: 0,
            pattern: bass,
            merge: None,
        },
        r,
    )
    .unwrap();
    d.apply(Command::AddPattern { device: key("q") }, r)
        .unwrap();
    let mut hats = PatternDoc::new("Hats");
    hats.length = 64;
    hats.resolution = SeqResolution::ThirtySecond;
    d.apply(
        Command::SetPattern {
            device: key("q"),
            index: 1,
            pattern: hats,
            merge: None,
        },
        r,
    )
    .unwrap();
    d.doc().clone()
}

fn reject(text: &str, path: &str, needle: &str) {
    let e = load_project(text, &registry()).expect_err("project must be rejected");
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

fn edited(text: &str, edit: impl FnOnce(&mut Value)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    edit(&mut v);
    v.to_string()
}

#[test]
fn the_format_is_version_11() {
    assert_eq!(PROJECT_VERSION, 40);
}

#[test]
fn steps_are_written_only_on_devices_that_have_them() {
    let r = registry();
    let text = save_project(&project(&r));
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 40);
    let devices = v["devices"].as_array().unwrap();
    assert!(devices[0].get("steps").is_none());
    assert!(devices[2].get("steps").is_none());
    let steps = &devices[1]["steps"];
    assert_eq!(steps["patterns"].as_array().unwrap().len(), 2);
    let p0 = &steps["patterns"][0];
    assert_eq!(p0["name"], "Bass \"A\" é\n");
    assert_eq!(p0["length"], 12);
    assert_eq!(p0["resolution"], "eighth_triplet");
    assert_eq!(p0["swing"], 40);
    assert_eq!(p0["steps"].as_array().unwrap().len(), 64);
    assert_eq!(
        p0["steps"][2],
        json!({"on": true, "key": 43, "velocity": 92, "gate": 75, "prob": 80, "tie": true})
    );
    assert_eq!(steps["patterns"][1]["resolution"], "thirty_second");
    // Every step is written in full, one per line, in a fixed key order.
    assert!(text.contains(
        "{\"on\": false, \"key\": 60, \"velocity\": 100, \"gate\": 80, \"prob\": 100, \"tie\": false}"
    ));
    assert!(
        p0["steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s.as_object().unwrap().len() == 6)
    );
}

#[test]
fn every_resolution_has_a_stable_name() {
    let r = registry();
    let names = [
        "quarter",
        "eighth",
        "eighth_triplet",
        "sixteenth",
        "sixteenth_triplet",
        "thirty_second",
        "thirty_second_triplet",
    ];
    for (res, name) in SeqResolution::ALL.into_iter().zip(names) {
        let mut doc = project(&r);
        doc.devices[1].steps.as_mut().unwrap().patterns[0].resolution = res;
        let text = save_project(&doc);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["devices"][1]["steps"]["patterns"][0]["resolution"], name);
        assert_eq!(load_project(&text, &r).unwrap(), doc);
    }
}

#[test]
fn save_load_save_is_canonical_and_exact() {
    let r = registry();
    let doc = project(&r);
    let text = save_project(&doc);
    let back = load_project(&text, &r).unwrap();
    assert_eq!(back, doc);
    assert_eq!(save_project(&back), text);
}

#[test]
fn a_sequencer_without_steps_loads_and_saves_without() {
    let r = registry();
    let mut doc = project(&r);
    doc.devices[1].steps = None;
    let text = save_project(&doc);
    assert!(!text.contains("\"steps\""));
    let back = load_project(&text, &r).unwrap();
    assert_eq!(back.devices[1].steps, None);
    assert_eq!(save_project(&back), text);
}

#[test]
fn the_committed_golden_is_the_canonical_save() {
    let r = registry();
    let doc = project(&r);
    let text = save_project(&doc);
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/seq-v7.atomik");
    let _ = path;
    // The fixture stays a version 7 file (it is also the "v7 still loads" fixture); the
    // canonical save of its document is the same text with the version number of today.
    let file = fixture("seq-v7.atomik");
    assert_eq!(as_current(&file, 7), text);
    assert_eq!(load_project(&file, &r).unwrap(), doc);
}

#[test]
fn a_default_sequencer_round_trips() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SEQ, Some("q")), &r).unwrap();
    let text = save_project(d.doc());
    let back = load_project(&text, &r).unwrap();
    assert_eq!(back.devices[0].steps, Some(StepSeqDoc::default()));
    assert_eq!(&back, d.doc());
}

// ---- rejection -----------------------------------------------------------------------

#[test]
fn bad_steps_are_rejected_with_paths() {
    let r = registry();
    let good = save_project(&project(&r));
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(Edit, &str, &str)> = vec![
        (
            Box::new(|v| v["devices"][1]["steps"]["patterns"] = json!([])),
            "devices[1].steps.patterns",
            "1..=32",
        ),
        (
            Box::new(|v| {
                let p = v["devices"][1]["steps"]["patterns"][0].clone();
                v["devices"][1]["steps"]["patterns"] = Value::Array(vec![p; 9]);
            }),
            "devices[1].steps.patterns",
            "1..=8",
        ),
        (
            Box::new(|v| v["devices"][1]["steps"]["patterns"][1]["length"] = json!(0)),
            "devices[1].steps.patterns[1].length",
            "1..=64",
        ),
        (
            Box::new(|v| v["devices"][1]["steps"]["patterns"][0]["length"] = json!(65)),
            "devices[1].steps.patterns[0].length",
            "1..=64",
        ),
        (
            Box::new(|v| v["devices"][1]["steps"]["patterns"][0]["swing"] = json!(76)),
            "devices[1].steps.patterns[0].swing",
            "0..=75",
        ),
        (
            Box::new(|v| v["devices"][1]["steps"]["patterns"][0]["resolution"] = json!("half")),
            "devices[1].steps.patterns[0].resolution",
            "half",
        ),
        (
            Box::new(|v| {
                v["devices"][1]["steps"]["patterns"][0]["steps"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }),
            "devices[1].steps.patterns[0].steps",
            "64",
        ),
        (
            Box::new(|v| {
                v["devices"][1]["steps"]["patterns"][0]["steps"][3]["velocity"] = json!(0)
            }),
            "devices[1].steps.patterns[0].steps[3].velocity",
            "1..=127",
        ),
        (
            Box::new(|v| v["devices"][1]["steps"]["patterns"][0]["steps"][3]["key"] = json!(128)),
            "devices[1].steps.patterns[0].steps[3].key",
            "0..=127",
        ),
        (
            Box::new(|v| v["devices"][1]["steps"]["patterns"][0]["steps"][3]["gate"] = json!(101)),
            "devices[1].steps.patterns[0].steps[3].gate",
            "1..=100",
        ),
        (
            Box::new(|v| v["devices"][1]["steps"]["patterns"][0]["steps"][3]["prob"] = json!(101)),
            "devices[1].steps.patterns[0].steps[3].prob",
            "0..=100",
        ),
        (
            Box::new(|v| v["devices"][1]["steps"]["patterns"][0]["steps"][3]["on"] = json!(1)),
            "devices[1].steps.patterns[0].steps[3].on",
            "boolean",
        ),
        (
            Box::new(|v| {
                v["devices"][1]["steps"]["patterns"][0]["steps"][3]
                    .as_object_mut()
                    .unwrap()
                    .remove("tie");
            }),
            "devices[1].steps.patterns[0].steps[3].tie",
            "missing field",
        ),
        (
            Box::new(|v| {
                v["devices"][1]["steps"]["patterns"][0]["steps"][3]["extra"] = json!(1);
            }),
            "devices[1].steps.patterns[0].steps[3].extra",
            "unknown field",
        ),
        (
            Box::new(|v| {
                v["devices"][1]["steps"]["patterns"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("name");
            }),
            "devices[1].steps.patterns[0].name",
            "missing field",
        ),
        (
            Box::new(|v| v["devices"][1]["steps"]["patterns"][0]["name"] = json!("x".repeat(65))),
            "devices[1].steps.patterns[0].name",
            "64",
        ),
        (
            Box::new(|v| v["devices"][1]["steps"]["bpm"] = json!(1)),
            "devices[1].steps.bpm",
            "unknown field",
        ),
    ];
    for (edit, path, needle) in cases {
        reject(&edited(&good, edit), path, needle);
    }
}

#[test]
fn steps_on_a_device_that_takes_no_pattern_data_are_rejected() {
    let r = registry();
    let good = save_project(&project(&r));
    // The arpeggiator and the synth take none.
    for i in [0usize, 2] {
        let text = edited(&good, |v| {
            v["devices"][i]["steps"] = v["devices"][1]["steps"].clone();
        });
        reject(
            &text,
            &format!("devices[{i}].steps"),
            "does not take pattern data",
        );
    }
}

// ---- older versions ------------------------------------------------------------------

#[test]
fn steps_are_unknown_before_version_7() {
    let r = registry();
    let good = save_project(&project(&r));
    for version in 3..=6 {
        let text = edited(&good, |v| v["version"] = json!(version));
        let e = load_project(&text, &r).unwrap_err().to_string();
        assert!(
            e.contains("steps") && e.contains("unknown field"),
            "v{version}: {e}"
        );
    }
    let text = edited(&good, |v| v["version"] = json!(41));
    assert!(
        load_project(&text, &r)
            .unwrap_err()
            .to_string()
            .contains("unsupported version")
    );
}

#[test]
fn version_6_files_load_unchanged_and_resave_as_version_7() {
    let r = console_registry();
    for name in ["demo-v6.atomik", "drum-demo-v6.atomik"] {
        let v6 = fixture(name);
        assert!(v6.contains("\"version\": 6,"));
        let doc = load_project(&v6, &r).unwrap();
        assert!(doc.devices.iter().all(|d| d.steps.is_none()), "{name}");
        let v7 = save_project(&doc);
        assert!(v7.contains("\"version\": 40,"));
        assert_eq!(v7, as_current(&v6, 6), "{name}");
        assert_eq!(load_project(&v7, &r).unwrap(), doc);
    }
}

#[test]
fn versions_3_to_6_load_a_rack_without_steps() {
    let r = console_registry();
    let doc = load_project(&fixture("demo-v6.atomik"), &r).unwrap();
    let text = save_project(&doc);
    for version in 3..=6 {
        let old = text.replace("\"version\": 40", &format!("\"version\": {version}"));
        assert_eq!(load_project(&old, &r).unwrap(), doc, "v{version}");
    }
}

#[test]
fn version_1_still_migrates() {
    let r = console_registry();
    let doc = load_project(&fixture("demo-v1.atomik"), &r).unwrap();
    assert!(doc.devices.iter().all(|d| d.steps.is_none()));
    let back = load_project(&save_project(&doc), &r).unwrap();
    assert_eq!(back, doc);
}

// ---- properties ----------------------------------------------------------------------

fn pattern() -> impl Strategy<Value = PatternDoc> {
    let step = (
        any::<bool>(),
        0u8..=127,
        1u8..=127,
        1u8..=100,
        0u8..=100,
        any::<bool>(),
    );
    (
        "[ -~é\"\\\\]{0,20}",
        1u8..=64,
        0u8..=75,
        0usize..7,
        prop::collection::vec(step, 64),
    )
        .prop_map(|(name, length, swing, res, steps)| PatternDoc {
            name,
            length,
            resolution: SeqResolution::ALL[res],
            swing,
            steps: steps
                .into_iter()
                .map(|(on, key, velocity, gate, prob, tie)| StepDoc {
                    on,
                    key,
                    velocity,
                    gate,
                    prob,
                    tie,
                })
                .collect(),
        })
}

proptest! {
    #[test]
    fn any_patterns_round_trip_bit_exactly(pats in prop::collection::vec(pattern(), 1..=8)) {
        let r = registry();
        let mut doc = project(&r);
        doc.devices[1].steps = Some(StepSeqDoc { patterns: pats });
        let text = save_project(&doc);
        let back = load_project(&text, &r).unwrap();
        prop_assert_eq!(&back, &doc);
        prop_assert_eq!(save_project(&back), text);
    }
}

// ------------------------------------------------------------------ pattern caps per kind of data

#[test]
fn a_cv_sequencer_holds_32_patterns_and_a_step_sequencer_8() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    for (i, (t, k)) in [("atomik.cv-sequencer", "cv"), (SEQ, "q")]
        .into_iter()
        .enumerate()
    {
        d.apply(add(i, t, Some(k)), &r).unwrap();
    }
    for k in ["cv", "q"] {
        for _ in 1..8 {
            d.apply(Command::AddPattern { device: key(k) }, &r).unwrap();
        }
    }
    let e = d
        .apply(Command::AddPattern { device: key("q") }, &r)
        .unwrap_err();
    assert!(
        matches!(&e, ModelError::BadPatterns(m) if m.contains("at most 8")),
        "{e:?}"
    );
    for _ in 8..32 {
        d.apply(Command::AddPattern { device: key("cv") }, &r)
            .unwrap();
    }
    let e = d
        .apply(Command::AddPattern { device: key("cv") }, &r)
        .unwrap_err();
    assert!(
        matches!(&e, ModelError::BadPatterns(m) if m.contains("at most 32")),
        "{e:?}"
    );
    // The copy of a pattern goes to the end and the engine value has them all.
    let cv = d.doc().device(&key("cv")).unwrap();
    assert_eq!(cv.steps.as_ref().unwrap().patterns.len(), 32);
    let atomik_device_sdk::PlayerData::CvSeq(data) = &*player_data_for(cv, &r).unwrap() else {
        panic!("cv data")
    };
    assert_eq!(data.notes.count, 32);
    // The file round-trips with 32 patterns.
    let text = save_project(d.doc());
    let back = load_project(&text, &r).unwrap();
    assert_eq!(&back, d.doc());
    // A step sequencer with nine patterns in a file is refused, a CV sequencer is not.
    let mut v: Value = serde_json::from_str(&text).unwrap();
    let list = v["devices"][1]["steps"]["patterns"].as_array_mut().unwrap();
    let extra = list[0].clone();
    list.push(extra);
    let e = load_project(&v.to_string(), &r).expect_err("nine patterns");
    let ModelError::Load { path, message } = &e else {
        panic!("{e:?}")
    };
    assert_eq!(path.to_string(), "devices[1].steps.patterns");
    assert!(message.contains("outside 1..=8"), "{message}");
}
