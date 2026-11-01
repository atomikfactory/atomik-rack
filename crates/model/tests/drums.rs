//! Project format v33: a drum generator's `"drums"`, canonical text, rejection with JSON paths,
//! the drum commands and their undo, generated patterns and files of version 32 still loading.

mod common;

use atomik_device_sdk::{DrumRole, PlayerData, Registry};
use atomik_devices::drum_generator::{DrumStyle, generate_pattern};
use atomik_model::{
    Command, Document, DrumDoc, DrumLaneDoc, DrumPatternDoc, DrumStepDoc, ModelError,
    PROJECT_VERSION, PatternDoc, RackDoc, SyncOp, empty_doc, load_patch, load_project,
    player_data_for, save_patch, save_project,
};
use common::*;
use serde_json::{Value, json};

const DRUMS: &str = "atomik.drum-generator";

fn step(velocity: u8, prob: u8) -> DrumStepDoc {
    DrumStepDoc::hit(velocity, prob)
}

/// A pattern with a kick on every fourth step, a snare line with a probability, a renamed lane
/// and a muted one.
fn groove() -> DrumPatternDoc {
    let mut p = DrumPatternDoc::new("Groove");
    for i in (0..16).step_by(4) {
        p.steps[0][i] = step(110, 100);
    }
    p.steps[2][4] = step(96, 70);
    p.steps[2][12] = step(127, 100);
    p.steps[6][63] = step(1, 0);
    p.lanes[3] = DrumLaneDoc {
        key: 60,
        role: DrumRole::Percussion,
        mute: true,
    };
    p.swing = 20;
    p
}

/// A rack with a drum machine, a drum generator `g` with two patterns, a step sequencer `q`.
fn project(r: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    for (i, (t, k)) in [("atomik.drum-machine", "m"), (DRUMS, "g"), (SEQ, "q")]
        .into_iter()
        .enumerate()
    {
        d.apply(add(i, t, Some(k)), r).unwrap();
    }
    d.apply(connect("g:notes_out", "m:notes_in"), r).unwrap();
    d.apply(Command::AddDrumPattern { device: key("g") }, r)
        .unwrap();
    for index in 0..2u8 {
        let mut p = groove();
        p.name = format!("Groove {index}");
        p.steps[1][usize::from(index)] = step(64 + index, 100);
        d.apply(
            Command::SetDrumPattern {
                device: key("g"),
                index,
                pattern: p,
                merge: None,
            },
            r,
        )
        .unwrap();
    }
    d
}

fn device<'a>(doc: &'a RackDoc, k: &str) -> &'a atomik_model::RackDevice {
    doc.device(&key(k)).unwrap()
}

fn drums<'a>(d: &'a Document, k: &str) -> &'a DrumDoc {
    device(d.doc(), k).drums.as_ref().unwrap()
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

fn set_step(k: &str, index: u8, lane: u8, step: u8, velocity: u8, merge: Option<u64>) -> Command {
    Command::SetDrumStep {
        device: key(k),
        index,
        lane,
        step,
        velocity,
        prob: 100,
        merge,
    }
}

#[test]
fn the_format_is_version_33() {
    assert_eq!(PROJECT_VERSION, 39);
}

#[test]
fn drum_doc_round_trips_in_project_and_patch() {
    let r = registry();
    let d = project(&r);
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 39);
    let devices = v["devices"].as_array().unwrap();
    assert!(
        devices[0].get("drums").is_none(),
        "the drum machine has none"
    );
    assert!(devices[2].get("drums").is_none(), "nor the step sequencer");
    assert!(
        devices[1].get("steps").is_none(),
        "the generator has no steps"
    );
    let patterns = devices[1]["drums"]["patterns"].as_array().unwrap();
    assert_eq!(patterns.len(), 2);
    let p = &patterns[1];
    assert_eq!(p["name"], "Groove 1");
    assert_eq!(
        (p["length"].as_u64(), p["swing"].as_u64()),
        (Some(16), Some(20))
    );
    assert_eq!(p["resolution"], "sixteenth");
    let lanes = p["lanes"].as_array().unwrap();
    assert_eq!(lanes.len(), 16);
    assert_eq!(lanes[0]["key"], 36);
    assert_eq!(lanes[0]["role"], "kick");
    assert_eq!(lanes[0]["mute"], false);
    assert_eq!(lanes[0]["velocity"][4], 110);
    assert_eq!(lanes[2]["prob"][4], 70);
    assert_eq!(lanes[3]["key"], 60);
    assert_eq!(lanes[3]["role"], "percussion");
    assert_eq!(lanes[3]["mute"], true);
    assert_eq!(lanes[6]["velocity"][63], 1);
    assert_eq!(lanes[6]["prob"][63], 0);
    assert_eq!(lanes[0]["velocity"].as_array().unwrap().len(), 64);
    let back = load_project(&text, &r).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(save_project(&back), text, "the text is canonical");

    // A patch holds params only: no drums in the text, and the device loads without any (the
    // engine then gets one empty pattern, like a step sequencer loaded from a patch).
    let patch = save_patch(d.doc());
    assert!(!patch.contains("\"drums\""));
    let loaded = load_patch(&patch, &r).unwrap();
    assert_eq!(device(&loaded, "g").drums, None);
    let data = player_data_for(device(&loaded, "g"), &r).unwrap();
    assert_eq!(data.drums().unwrap().count, 1);
}

#[test]
fn a_new_drum_generator_has_default_drums_and_no_steps() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, DRUMS, Some("g")), &r).unwrap();
    d.apply(add(1, SEQ, Some("q")), &r).unwrap();
    assert_eq!(device(d.doc(), "g").drums, Some(DrumDoc::default()));
    assert_eq!(device(d.doc(), "g").steps, None);
    assert_eq!(device(d.doc(), "q").drums, None);
    assert!(device(d.doc(), "q").steps.is_some());
    assert_eq!(DrumPatternDoc::default().lanes[0].key, 36);
    assert_eq!(DrumPatternDoc::default().lanes[15].key, 51);
    assert_eq!(DrumPatternDoc::default().lanes[2].role, DrumRole::Snare);
}

#[test]
fn drums_only_on_types_that_take_them() {
    let r = registry();
    let text = save_project(project(&r).doc());
    // A step sequencer takes steps, not drums; neither does the drum machine.
    for i in [0, 2] {
        let bad = edited(&text, |v| {
            v["devices"][i]["drums"] = v["devices"][1]["drums"].clone();
        });
        reject(
            &bad,
            &format!("devices[{i}].drums"),
            "does not take drum data",
        );
    }
    // And the generator takes no steps.
    let bad = edited(&text, |v| {
        v["devices"][1]["steps"] = json!({"patterns": []});
    });
    reject(&bad, "devices[1].steps", "takes drum data, not steps");
    // The commands refuse the wrong kind of device too, both ways round.
    let mut d = project(&r);
    let before = d.doc().clone();
    for c in [
        Command::AddDrumPattern { device: key("q") },
        set_step("m", 0, 0, 0, 100, None),
        Command::SetPattern {
            device: key("g"),
            index: 0,
            pattern: PatternDoc::default(),
            merge: None,
        },
        Command::AddPattern { device: key("g") },
    ] {
        let e = d.apply(c, &r).unwrap_err();
        assert!(matches!(e, ModelError::BadPatterns(_)), "{e:?}");
    }
    assert_eq!(d.doc(), &before);
}

#[test]
fn old_projects_load_without_drums() {
    let r = registry();
    let mut plain = Document::new(empty_doc());
    for (i, (t, k)) in [("atomik.drum-machine", "m"), (SEQ, "q")]
        .into_iter()
        .enumerate()
    {
        plain.apply(add(i, t, Some(k)), &r).unwrap();
    }
    let text = save_project(plain.doc());
    let old = text.replacen("\"version\": 39", "\"version\": 32", 1);
    assert_ne!(old, text);
    let doc = load_project(&old, &r).unwrap();
    assert_eq!(device(&doc, "m").drums, None);
    assert_eq!(
        save_project(&doc),
        text,
        "re-saving changes the version line only"
    );
    // A version 32 file has no "drums" field.
    let with = edited(&save_project(project(&r).doc()), |v| {
        v["version"] = json!(32);
    });
    reject(&with, "devices[1].drums", "unknown field");
}

/// A loader case: the JSON path of the error, the edit that causes it and what its message says.
type Case = (&'static str, Box<dyn Fn(&mut Value)>, &'static str);

#[test]
fn the_loader_checks_drum_shapes_and_ranges() {
    let r = registry();
    let text = save_project(project(&r).doc());
    let cases: Vec<Case> = vec![
        (
            "devices[1].drums.patterns",
            Box::new(|v| v["devices"][1]["drums"]["patterns"] = json!([])),
            "outside 1..=32",
        ),
        (
            "devices[1].drums.patterns[0].lanes",
            Box::new(|v| {
                v["devices"][1]["drums"]["patterns"][0]["lanes"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }),
            "exactly 16",
        ),
        (
            "devices[1].drums.patterns[0].lanes[2].role",
            Box::new(|v| {
                v["devices"][1]["drums"]["patterns"][0]["lanes"][2]["role"] = json!("gong")
            }),
            "unknown role",
        ),
        (
            "devices[1].drums.patterns[0].lanes[0].key",
            Box::new(|v| v["devices"][1]["drums"]["patterns"][0]["lanes"][0]["key"] = json!(128)),
            "outside 0..=127",
        ),
        (
            "devices[1].drums.patterns[0].lanes[0].velocity",
            Box::new(|v| {
                v["devices"][1]["drums"]["patterns"][0]["lanes"][0]["velocity"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }),
            "exactly 64",
        ),
        (
            "devices[1].drums.patterns[0].lanes[0].velocity[3]",
            Box::new(|v| {
                v["devices"][1]["drums"]["patterns"][0]["lanes"][0]["velocity"][3] = json!(128);
            }),
            "outside 0..=127",
        ),
        (
            "devices[1].drums.patterns[0].lanes[1].prob[9]",
            Box::new(|v| {
                v["devices"][1]["drums"]["patterns"][0]["lanes"][1]["prob"][9] = json!(101);
            }),
            "outside 0..=100",
        ),
        (
            "devices[1].drums.patterns[0].swing",
            Box::new(|v| v["devices"][1]["drums"]["patterns"][0]["swing"] = json!(76)),
            "outside 0..=75",
        ),
        (
            "devices[1].drums.patterns[0].length",
            Box::new(|v| v["devices"][1]["drums"]["patterns"][0]["length"] = json!(0)),
            "outside 1..=64",
        ),
        (
            "devices[1].drums.patterns[0].lanes[0].mute",
            Box::new(|v| {
                v["devices"][1]["drums"]["patterns"][0]["lanes"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("mute");
            }),
            "missing field",
        ),
    ];
    for (path, edit, needle) in cases {
        reject(&edited(&text, edit), path, needle);
    }
}

#[test]
fn set_drum_step_validates_undoes_and_merges_drags() {
    let r = registry();
    let mut d = project(&r);
    let before = d.doc().clone();
    for (c, needle) in [
        (set_step("g", 2, 0, 0, 100, None), "index 2"),
        (set_step("g", 0, 16, 0, 100, None), "lane 16"),
        (set_step("g", 0, 0, 64, 100, None), "step 64"),
        (set_step("g", 0, 0, 0, 128, None), "velocity 128"),
    ] {
        let e = d.apply(c, &r).unwrap_err();
        assert!(
            matches!(&e, ModelError::BadPatterns(m) if m.contains(needle)),
            "{e:?}"
        );
    }
    let e = d
        .apply(
            Command::SetDrumStep {
                device: key("g"),
                index: 0,
                lane: 0,
                step: 0,
                velocity: 90,
                prob: 101,
                merge: None,
            },
            &r,
        )
        .unwrap_err();
    assert!(matches!(e, ModelError::BadPatterns(_)), "{e:?}");
    assert_eq!(d.doc(), &before, "a rejected command changes nothing");

    // Two drag steps with one merge id are one undo step.
    for v in [10, 20] {
        let ops = d.apply(set_step("g", 0, 5, 3, v, Some(7)), &r).unwrap();
        assert!(
            ops.iter()
                .any(|o| matches!(o, SyncOp::PatternsChanged { .. })),
            "the engine data is re-sent"
        );
    }
    assert_eq!(drums(&d, "g").patterns[0].steps[5][3].velocity, 20);
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&r).unwrap();
    assert_eq!(drums(&d, "g").patterns[0].steps[5][3].velocity, 20);
    // A step with another merge id is its own undo step.
    d.apply(set_step("g", 0, 5, 3, 30, Some(8)), &r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(drums(&d, "g").patterns[0].steps[5][3].velocity, 20);
}

#[test]
fn set_drum_lane_and_pattern_edit_one_undo_step_each() {
    let r = registry();
    let mut d = project(&r);
    let before = d.doc().clone();
    let lane = DrumLaneDoc {
        key: 70,
        role: DrumRole::Cymbal,
        mute: true,
    };
    d.apply(
        Command::SetDrumLane {
            device: key("g"),
            index: 1,
            lane: 7,
            setting: lane,
        },
        &r,
    )
    .unwrap();
    assert_eq!(drums(&d, "g").patterns[1].lanes[7], lane);
    assert_eq!(
        drums(&d, "g").patterns[0].lanes[7].key,
        43,
        "pattern 0 is untouched"
    );
    for bad in [
        Command::SetDrumLane {
            device: key("g"),
            index: 1,
            lane: 16,
            setting: lane,
        },
        Command::SetDrumLane {
            device: key("g"),
            index: 1,
            lane: 0,
            setting: DrumLaneDoc { key: 200, ..lane },
        },
        Command::SetDrumPattern {
            device: key("g"),
            index: 0,
            pattern: DrumPatternDoc {
                length: 65,
                ..groove()
            },
            merge: None,
        },
        Command::SetDrumPattern {
            device: key("g"),
            index: 0,
            pattern: DrumPatternDoc {
                steps: Vec::new(),
                ..groove()
            },
            merge: None,
        },
    ] {
        assert!(matches!(
            d.apply(bad, &r).unwrap_err(),
            ModelError::BadPatterns(_)
        ));
    }
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    // Clearing a pattern keeps its name, grid and lanes.
    let cleared = groove().cleared();
    assert_eq!(cleared.name, "Groove");
    assert_eq!(cleared.lanes, groove().lanes);
    assert!(cleared.steps.iter().flatten().all(|s| s.velocity == 0));
}

#[test]
fn add_duplicate_and_remove_pattern_move_the_param_in_one_undo_step() {
    let r = registry();
    let mut d = project(&r);
    d.apply(
        Command::DuplicateDrumPattern {
            device: key("g"),
            index: 1,
        },
        &r,
    )
    .unwrap();
    let doc = drums(&d, "g");
    assert_eq!(doc.patterns.len(), 3);
    assert_eq!(doc.patterns[2].name, "Groove 1 copy");
    assert_eq!(doc.patterns[2].steps, doc.patterns[1].steps);
    for _ in 0..29 {
        d.apply(Command::AddDrumPattern { device: key("g") }, &r)
            .unwrap();
    }
    assert_eq!(drums(&d, "g").patterns.len(), 32);
    assert_eq!(drums(&d, "g").patterns[31].name, "Pattern 32");
    for c in [
        Command::AddDrumPattern { device: key("g") },
        Command::DuplicateDrumPattern {
            device: key("g"),
            index: 0,
        },
    ] {
        let e = d.apply(c, &r).unwrap_err();
        assert!(
            matches!(&e, ModelError::BadPatterns(m) if m.contains("at most 32")),
            "{e:?}"
        );
    }
    // Removing the pattern the param points at lowers the param in the same step.
    d.apply(set_param("g", "pattern", 32.0), &r).unwrap();
    let before = d.doc().clone();
    d.apply(
        Command::RemoveDrumPattern {
            device: key("g"),
            index: 31,
        },
        &r,
    )
    .unwrap();
    assert_eq!(drums(&d, "g").patterns.len(), 31);
    assert_eq!(device(d.doc(), "g").params.get("pattern"), Some(&31.0));
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    // The last pattern stays.
    let mut one = Document::new(empty_doc());
    one.apply(add(0, DRUMS, Some("g")), &r).unwrap();
    let e = one
        .apply(
            Command::RemoveDrumPattern {
                device: key("g"),
                index: 0,
            },
            &r,
        )
        .unwrap_err();
    assert!(matches!(e, ModelError::BadPatterns(_)));
}

#[test]
fn player_data_carries_the_drums_and_none_for_other_types() {
    let r = registry();
    let d = project(&r);
    let data = player_data_for(device(d.doc(), "g"), &r).unwrap();
    let PlayerData::Drums(data) = &*data else {
        panic!("a drum generator gets drum data")
    };
    assert_eq!(data.count, 2);
    let p = &data.patterns[1];
    assert_eq!((p.length, p.swing), (16, 20));
    assert_eq!(p.lanes[3].key, 60);
    assert_eq!(p.lanes[3].role, DrumRole::Percussion);
    assert!(p.lanes[3].mute);
    assert_eq!(p.steps[0][4].velocity, 110);
    assert_eq!(p.steps[2][4].prob, 70);
    assert_eq!(p.steps[1][1].velocity, 65);
    assert_eq!(data.patterns[5], atomik_device_sdk::DrumPattern::default());
    // The step sequencer still gets step data, the drum machine none.
    let plain = player_data_for(device(d.doc(), "q"), &r).unwrap();
    assert!(matches!(&*plain, PlayerData::StepSeq(_)));
    assert!(player_data_for(device(d.doc(), "m"), &r).is_none());
    // A generator without stored drums gets one empty pattern.
    let mut bare = device(d.doc(), "g").clone();
    bare.drums = None;
    let none = player_data_for(&bare, &r).unwrap();
    assert_eq!(none.drums().unwrap().count, 1);
}

#[test]
fn generate_is_deterministic_for_a_seed_and_undoes() {
    let r = registry();
    let mut d = project(&r);
    let before = d.doc().clone();
    let generate = |d: &mut Document, seed: u64| {
        let current = drums(d, "g").patterns[0].clone();
        let written = generate_pattern(&current.to_data(), DrumStyle::BrokenBeat, 0.3, seed);
        let pattern = DrumPatternDoc::from_data(&written, current.name);
        d.apply(
            Command::SetDrumPattern {
                device: key("g"),
                index: 0,
                pattern: pattern.clone(),
                merge: None,
            },
            &r,
        )
        .unwrap();
        pattern
    };
    let a = generate(&mut d, 5);
    assert_ne!(
        a,
        before
            .device(&key("g"))
            .unwrap()
            .drums
            .as_ref()
            .unwrap()
            .patterns[0]
    );
    a.check().unwrap();
    assert_eq!(a.name, "Groove 0");
    assert_eq!(a.lanes, groove().lanes, "lanes and grid are kept");
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before, "one undo step restores the pattern");
    let again = generate(&mut d, 5);
    assert_eq!(again, a, "the same seed writes the same pattern");
    d.undo(&r).unwrap();
    let other = generate(&mut d, 6);
    assert_ne!(other.steps, a.steps, "another seed writes another one");
}

#[test]
fn drum_doc_check_names_the_field() {
    let mut p = groove();
    p.steps[3][9].prob = 101;
    assert!(p.check().unwrap_err().contains("steps[3][9].prob"));
    let mut p = groove();
    p.lanes[1].key = 128;
    assert!(p.check().unwrap_err().contains("lanes[1].key"));
    let mut p = groove();
    p.steps[4].pop();
    assert!(p.check().unwrap_err().contains("steps[4]"));
    let doc = DrumDoc {
        patterns: Vec::new(),
    };
    assert!(doc.check().unwrap_err().contains("outside 1..=32"));
    assert!(DrumDoc::default().check().is_ok());
}

fn fx_command(k: &str, index: u8, lane: u8, step: u8, flam: u8, ratchet: u8) -> Command {
    Command::SetDrumStepFx {
        device: key(k),
        index,
        lane,
        step,
        flam,
        ratchet,
        merge: None,
    }
}

#[test]
fn flam_and_ratchet_are_saved_only_where_a_lane_uses_them() {
    let r = registry();
    let mut d = project(&r);
    let plain = save_project(d.doc());
    assert!(
        !plain.contains("\"flam\"") && !plain.contains("\"ratchet\""),
        "a project without them differs from its older file in the version line alone"
    );
    d.apply(fx_command("g", 0, 0, 4, 30, 0), &r).unwrap();
    d.apply(fx_command("g", 1, 2, 12, 0, 6), &r).unwrap();
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let lanes = &v["devices"][1]["drums"]["patterns"][0]["lanes"];
    assert_eq!(lanes[0]["flam"][4], 30);
    assert!(lanes[0].get("ratchet").is_none());
    assert!(lanes[1].get("flam").is_none(), "only the lane that uses it");
    let second = &v["devices"][1]["drums"]["patterns"][1]["lanes"];
    assert_eq!(second[2]["ratchet"][12], 6);
    assert_eq!(second[2]["ratchet"].as_array().unwrap().len(), 64);
    let back = load_project(&text, &r).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(save_project(&back), text, "canonical text");
    // The engine value carries them.
    let PlayerData::Drums(data) = &*player_data_for(device(d.doc(), "g"), &r).unwrap() else {
        panic!("drum data")
    };
    assert_eq!(data.patterns[0].steps[0][4].flam, 30);
    assert_eq!(data.patterns[1].steps[2][12].ratchet, 6);
}

#[test]
fn flam_and_ratchet_commands_are_one_undo_step_and_keep_the_hit() {
    let r = registry();
    let mut d = project(&r);
    let before = d.doc().clone();
    d.apply(fx_command("g", 0, 0, 0, 12, 3), &r).unwrap();
    let s = drums(&d, "g").patterns[0].steps[0][0];
    assert_eq!((s.velocity, s.prob, s.flam, s.ratchet), (110, 100, 12, 3));
    // Setting the velocity afterwards keeps the flam and the ratchet.
    d.apply(set_step("g", 0, 0, 0, 80, None), &r).unwrap();
    let s = drums(&d, "g").patterns[0].steps[0][0];
    assert_eq!((s.velocity, s.flam, s.ratchet), (80, 12, 3));
    d.undo(&r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    for (flam, ratchet) in [(51, 0), (0, 9)] {
        let e = d
            .apply(fx_command("g", 0, 0, 0, flam, ratchet), &r)
            .unwrap_err();
        assert!(matches!(e, ModelError::BadPatterns(_)), "{e:?}");
    }
    assert!(d.apply(fx_command("g", 0, 16, 0, 0, 2), &r).is_err());
    assert!(d.apply(fx_command("g", 9, 0, 0, 0, 2), &r).is_err());
    assert!(d.apply(fx_command("q", 0, 0, 0, 0, 2), &r).is_err());
    assert_eq!(d.doc(), &before, "a rejected command changes nothing");
}

#[test]
fn the_loader_checks_flam_and_ratchet_rows() {
    let r = registry();
    let mut d = project(&r);
    d.apply(fx_command("g", 0, 0, 4, 30, 2), &r).unwrap();
    let text = save_project(d.doc());
    reject(
        &edited(&text, |v| {
            v["devices"][1]["drums"]["patterns"][0]["lanes"][0]["flam"][4] = json!(51)
        }),
        "devices[1].drums.patterns[0].lanes[0].flam[4]",
        "outside 0..=50",
    );
    reject(
        &edited(&text, |v| {
            v["devices"][1]["drums"]["patterns"][0]["lanes"][0]["ratchet"][1] = json!(9)
        }),
        "devices[1].drums.patterns[0].lanes[0].ratchet[1]",
        "outside 0..=8",
    );
    reject(
        &edited(&text, |v| {
            v["devices"][1]["drums"]["patterns"][0]["lanes"][0]["ratchet"]
                .as_array_mut()
                .unwrap()
                .pop();
        }),
        "devices[1].drums.patterns[0].lanes[0].ratchet",
        "exactly 64",
    );
}

#[test]
fn a_generator_holds_thirty_two_patterns_and_they_survive_a_round_trip() {
    let r = registry();
    let mut d = project(&r);
    for _ in 2..32 {
        d.apply(Command::AddDrumPattern { device: key("g") }, &r)
            .unwrap();
    }
    for i in 0..32u8 {
        d.apply(set_step("g", i, 5, i, 100 - i, None), &r).unwrap();
    }
    let text = save_project(d.doc());
    let back = load_project(&text, &r).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(drums(&d, "g").patterns.len(), 32);
    let PlayerData::Drums(data) = &*player_data_for(device(d.doc(), "g"), &r).unwrap() else {
        panic!("drum data")
    };
    assert_eq!(data.count, 32);
    assert_eq!(data.patterns[31].steps[5][31].velocity, 69);
    let e = d
        .apply(Command::AddDrumPattern { device: key("g") }, &r)
        .unwrap_err();
    assert!(matches!(&e, ModelError::BadPatterns(m) if m.contains("at most 32")));
    // A 33rd pattern in a file is refused.
    reject(
        &edited(&text, |v| {
            let list = v["devices"][1]["drums"]["patterns"].as_array_mut().unwrap();
            let extra = list[0].clone();
            list.push(extra);
        }),
        "devices[1].drums.patterns",
        "outside 1..=32",
    );
}
