//! Project format v32: a CV sequencer's `"curves"`, canonical text, rejection with JSON paths,
//! the curve commands and their undo, and files of version 31 still loading.

mod common;

use atomik_device_sdk::{PlayerData, Registry};
use atomik_model::{
    Command, CurveDoc, CurvePatternDoc, CurveStepDoc, Document, ModelError, PatternDoc, RackDoc,
    StepDoc, empty_doc, load_project, player_data_for, save_project,
};
use common::*;
use serde_json::{Value, json};

const CV: &str = "atomik.cv-sequencer";

fn step(value: i8, slide: bool) -> CurveStepDoc {
    CurveStepDoc { value, slide }
}

/// A pattern whose row A is a ramp with slides on every third step and row B a few holds.
fn ramp() -> CurvePatternDoc {
    let mut p = CurvePatternDoc::default();
    for i in 0..64usize {
        p.rows[0][i] = step((-100 + (i as i32 * 3) % 201) as i8, i % 3 == 1);
    }
    p.rows[1][2] = step(100, false);
    p.rows[1][5] = step(-37, true);
    p
}

/// A rack with a synth, a CV sequencer `c` with two patterns and a step sequencer `q`; the CV
/// sequencer has curves on both patterns.
fn project(r: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    for (i, (t, k)) in [(SYNTH, "s"), (CV, "c"), (SEQ, "q")]
        .into_iter()
        .enumerate()
    {
        d.apply(add(i, t, Some(k)), r).unwrap();
    }
    d.apply(connect("c:notes_out", "s:notes_in"), r).unwrap();
    d.apply(Command::AddPattern { device: key("c") }, r)
        .unwrap();
    for index in 0..2u8 {
        let mut p = ramp();
        p.rows[1][0] = step(i8::try_from(index).unwrap() * 50, true);
        d.apply(
            Command::SetCurves {
                device: key("c"),
                index,
                curves: p,
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

fn reject(text: &str, path: &str, needle: &str) {
    let e = load_project(text, &registry()).expect_err("project must be rejected");
    let ModelError::Patch(msg) = &e else {
        panic!("expected a patch error, got {e:?}")
    };
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
fn curves_round_trip() {
    let r = registry();
    let d = project(&r);
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 36);
    let devices = v["devices"].as_array().unwrap();
    assert!(devices[0].get("curves").is_none(), "the synth has none");
    assert!(devices[2].get("curves").is_none(), "nor the step sequencer");
    let patterns = devices[1]["curves"]["patterns"].as_array().unwrap();
    assert_eq!(patterns.len(), 2);
    assert_eq!(patterns[0]["a"].as_array().unwrap().len(), 64);
    assert_eq!(patterns[0]["a"][0], json!({"value": -100, "slide": false}));
    assert_eq!(patterns[0]["a"][1], json!({"value": -97, "slide": true}));
    assert_eq!(patterns[1]["b"][0], json!({"value": 50, "slide": true}));
    let back = load_project(&text, &r).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(save_project(&back), text, "the text is canonical");
}

#[test]
fn a_new_cv_sequencer_has_no_curves_until_one_is_set() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CV, Some("c")), &r).unwrap();
    assert_eq!(device(d.doc(), "c").curves, None);
    assert!(device(d.doc(), "c").steps.is_some());
    let text = save_project(d.doc());
    assert!(!text.contains("\"curves\""), "flat curves are not written");
}

#[test]
fn curves_only_on_types_that_take_them() {
    let r = registry();
    let text = save_project(project(&r).doc());
    // A step sequencer takes pattern data but no curves.
    let bad = edited(&text, |v| {
        v["devices"][2]["curves"] = v["devices"][1]["curves"].clone();
    });
    reject(&bad, "devices[2].curves", "does not take curves");
    // Neither does a synth.
    let bad = edited(&text, |v| {
        v["devices"][0]["curves"] = v["devices"][1]["curves"].clone();
    });
    reject(&bad, "devices[0].curves", "does not take curves");
    // The command refuses them too.
    let mut d = project(&r);
    let e = d
        .apply(
            Command::SetCurves {
                device: key("q"),
                index: 0,
                curves: CurvePatternDoc::default(),
                merge: None,
            },
            &r,
        )
        .unwrap_err();
    assert!(
        matches!(&e, ModelError::BadPatterns(m) if m.contains("does not take curves")),
        "{e:?}"
    );
}

#[test]
fn old_projects_load_without_curves() {
    let r = registry();
    let mut plain = Document::new(empty_doc());
    for (i, (t, k)) in [(SYNTH, "s"), (CV, "c"), (SEQ, "q")]
        .into_iter()
        .enumerate()
    {
        plain.apply(add(i, t, Some(k)), &r).unwrap();
    }
    let text = save_project(plain.doc());
    let old = text.replacen("\"version\": 36", "\"version\": 31", 1);
    assert_ne!(old, text);
    let doc = load_project(&old, &r).unwrap();
    assert_eq!(device(&doc, "c").curves, None);
    assert_eq!(
        save_project(&doc),
        text,
        "re-saving changes the version line only"
    );
    // A version 31 file has no "curves" field.
    let with = edited(&save_project(project(&r).doc()), |v| {
        v["version"] = json!(31);
    });
    reject(&with, "devices[1].curves", "unknown field");
}

#[test]
fn the_loader_checks_curve_shapes_and_ranges() {
    let r = registry();
    let text = save_project(project(&r).doc());
    reject(
        &edited(&text, |v| {
            v["devices"][1]["curves"]["patterns"][0]["a"][3]["value"] = json!(101);
        }),
        "devices[1].curves.patterns[0].a[3].value",
        "outside",
    );
    reject(
        &edited(&text, |v| {
            v["devices"][1]["curves"]["patterns"][1]["b"]
                .as_array_mut()
                .unwrap()
                .pop();
        }),
        "devices[1].curves.patterns[1].b",
        "exactly 64",
    );
    reject(
        &edited(&text, |v| {
            v["devices"][1]["curves"]["patterns"] = json!([]);
        }),
        "devices[1].curves.patterns",
        "outside 1..=8",
    );
    reject(
        &edited(&text, |v| {
            v["devices"][1]["curves"]["patterns"][0]["c"] = json!([]);
        }),
        "devices[1].curves.patterns[0].c",
        "unknown field",
    );
}

#[test]
fn set_curves_validates_undoes_and_merges_drags() {
    let r = registry();
    let mut d = project(&r);
    let before = d.doc().clone();
    let mut bad = ramp();
    bad.rows[0][7].value = -101;
    let e = d
        .apply(
            Command::SetCurves {
                device: key("c"),
                index: 0,
                curves: bad,
                merge: None,
            },
            &r,
        )
        .unwrap_err();
    assert!(matches!(e, ModelError::BadPatterns(_)), "{e:?}");
    let e = d
        .apply(
            Command::SetCurves {
                device: key("c"),
                index: 2,
                curves: ramp(),
                merge: None,
            },
            &r,
        )
        .unwrap_err();
    assert!(
        matches!(&e, ModelError::BadPatterns(m) if m.contains("index 2")),
        "{e:?}"
    );
    assert_eq!(d.doc(), &before, "a rejected command changes nothing");

    // Two drag steps with one merge id are one undo step.
    for v in [10, 20] {
        let mut p = ramp();
        p.rows[0][0].value = v;
        let ops = d
            .apply(
                Command::SetCurves {
                    device: key("c"),
                    index: 0,
                    curves: p,
                    merge: Some(7),
                },
                &r,
            )
            .unwrap();
        assert!(
            ops.iter()
                .any(|o| matches!(o, atomik_model::SyncOp::PatternsChanged { .. })),
            "the engine data is re-sent"
        );
    }
    assert_eq!(
        device(d.doc(), "c").curves.as_ref().unwrap().patterns[0].rows[0][0].value,
        20
    );
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&r).unwrap();
    assert_eq!(
        device(d.doc(), "c").curves.as_ref().unwrap().patterns[0].rows[0][0].value,
        20
    );
}

#[test]
fn set_curves_on_a_device_without_curves_starts_from_flat_and_undoes_to_none() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CV, Some("c")), &r).unwrap();
    d.apply(Command::AddPattern { device: key("c") }, &r)
        .unwrap();
    d.apply(
        Command::SetCurves {
            device: key("c"),
            index: 1,
            curves: ramp(),
            merge: None,
        },
        &r,
    )
    .unwrap();
    let c = device(d.doc(), "c").curves.clone().unwrap();
    assert_eq!(c.patterns.len(), 2, "flat pattern 0 is filled in below it");
    assert!(c.patterns[0].is_flat());
    assert_eq!(c.patterns[1], ramp());
    d.undo(&r).unwrap();
    assert_eq!(device(d.doc(), "c").curves, None);
}

#[test]
fn duplicate_and_remove_pattern_keep_the_curves_with_their_pattern() {
    let r = registry();
    let mut d = project(&r);
    // Pattern 0 is `ramp` with B[0] = 0 slide, pattern 1 has B[0] = 50 slide.
    let marker = |d: &Document, i: usize| {
        device(d.doc(), "c").curves.as_ref().unwrap().patterns[i].rows[1][0].value
    };
    d.apply(
        Command::DuplicatePattern {
            device: key("c"),
            index: 1,
        },
        &r,
    )
    .unwrap();
    let c = device(d.doc(), "c").curves.clone().unwrap();
    assert_eq!(c.patterns.len(), 3);
    assert_eq!(c.patterns[2], c.patterns[1], "the copy follows its pattern");
    d.apply(
        Command::RemovePattern {
            device: key("c"),
            index: 0,
        },
        &r,
    )
    .unwrap();
    assert_eq!(
        device(d.doc(), "c").curves.as_ref().unwrap().patterns.len(),
        2
    );
    assert_eq!(marker(&d, 0), 50, "the later patterns move up with theirs");
    // One undo step puts the pattern and its curves back.
    d.undo(&r).unwrap();
    assert_eq!(marker(&d, 0), 0);
    assert_eq!(
        device(d.doc(), "c").steps.as_ref().unwrap().patterns.len(),
        3
    );
    // Removing a pattern the curves do not reach changes nothing there.
    let mut e = Document::new(empty_doc());
    e.apply(add(0, CV, Some("c")), &r).unwrap();
    e.apply(Command::AddPattern { device: key("c") }, &r)
        .unwrap();
    e.apply(
        Command::RemovePattern {
            device: key("c"),
            index: 1,
        },
        &r,
    )
    .unwrap();
    assert_eq!(device(e.doc(), "c").curves, None);
}

#[test]
fn player_data_carries_the_curves_of_a_cv_sequencer_and_none_for_the_step_sequencer() {
    let r = registry();
    let d = project(&r);
    let PlayerData::CvSeq(data) = &*player_data_for(device(d.doc(), "c"), &r).unwrap() else {
        panic!("a CV sequencer gets CvSeq data")
    };
    assert_eq!(data.notes.count, 2);
    assert_eq!(data.curves[0][0][0].value, -100);
    assert!(data.curves[0][0][1].slide);
    assert_eq!(data.curves[1][1][0].value, 50);
    assert_eq!(
        data.curves[2][0][0].value, 0,
        "slots past the count are flat"
    );
    let plain = player_data_for(device(d.doc(), "q"), &r).unwrap();
    assert!(matches!(&*plain, PlayerData::StepSeq(_)));
    assert!(player_data_for(device(d.doc(), "s"), &r).is_none());
    // Without stored curves the data is flat.
    let mut e = Document::new(empty_doc());
    e.apply(add(0, CV, Some("c")), &r).unwrap();
    let PlayerData::CvSeq(flat) = &*player_data_for(device(e.doc(), "c"), &r).unwrap() else {
        panic!()
    };
    assert_eq!(flat.curves[0][0][0].value, 0);
}

#[test]
fn the_note_row_of_a_cv_sequencer_edits_like_a_step_sequencer() {
    let r = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CV, Some("c")), &r).unwrap();
    let mut p = PatternDoc::new("Lead");
    p.steps[0] = StepDoc {
        on: true,
        key: 64,
        velocity: 90,
        gate: 50,
        prob: 100,
        tie: false,
    };
    d.apply(
        Command::SetPattern {
            device: key("c"),
            index: 0,
            pattern: p.clone(),
            merge: None,
        },
        &r,
    )
    .unwrap();
    assert_eq!(device(d.doc(), "c").steps.as_ref().unwrap().patterns[0], p);
}

#[test]
fn curve_doc_check_names_the_field() {
    assert_eq!(CurveDoc::default().check(), Ok(()));
    let mut c = CurveDoc::default();
    c.patterns[0].rows[1][3].value = 120;
    assert_eq!(
        c.check().unwrap_err(),
        "pattern 0: rows[1][3].value 120 is outside -100..=100"
    );
    c.patterns[0].rows[1][3].value = 0;
    c.patterns.push(CurvePatternDoc::default());
    c.patterns[1].rows[0].pop();
    assert!(c.check().unwrap_err().contains("exactly 64"));
}
