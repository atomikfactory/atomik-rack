//! A project with many cables, notes and points is read in bulk, and a fault in it is still
//! reported with the exact JSON path and message of the one item that is wrong.
#![allow(missing_docs)]

use atomik_device_sdk::Registry;
use atomik_model::stress::{lanes_song_doc, large_song_doc, stress_doc};
use atomik_model::{ModelError, load_project, save_project};
use serde_json::{Value, json};

fn registry() -> Registry {
    let mut r = Registry::new();
    atomik_devices::register_all(&mut r).expect("built-in devices register");
    r
}

fn load_err(value: &Value, reg: &Registry) -> (String, String) {
    match load_project(&value.to_string(), reg) {
        Err(ModelError::Load { path, message }) => (path, message),
        other => panic!("expected a load error, got {other:?}"),
    }
}

#[test]
fn a_rack_with_hundreds_of_cables_loads_to_the_same_document() {
    let reg = registry();
    let doc = stress_doc(&reg, 100, 300);
    let text = save_project(&doc);
    let back = load_project(&text, &reg).unwrap();
    assert_eq!(back.cables, doc.cables);
    assert_eq!(save_project(&back), text);
}

#[test]
fn a_bad_cable_among_many_names_its_own_index() {
    let reg = registry();
    let text = save_project(&stress_doc(&reg, 100, 300));
    let mut value: Value = serde_json::from_str(&text).unwrap();
    let cables = value["cables"].as_array_mut().unwrap();
    let n = cables.len();
    assert!(n > 100);

    // A second cable into a plain input.
    let mut twice = value.clone();
    let first = twice["cables"][0].clone();
    twice["cables"][n - 1] = first;
    let (path, message) = load_err(&twice, &reg);
    assert_eq!(path, format!("cables[{}].to", n - 1), "{message}");
    assert!(message.contains("already connected"), "{message}");

    // A socket that does not exist.
    let mut unknown = value.clone();
    unknown["cables"][7]["to"] = json!("t001:nope");
    let (path, message) = load_err(&unknown, &reg);
    assert_eq!(path, "cables[7].to", "{message}");

    // A cable with a stray field.
    let mut stray = value.clone();
    stray["cables"][3]["gain"] = json!(1);
    let (path, message) = load_err(&stray, &reg);
    assert_eq!(path, "cables[3].gain", "{message}");
    assert!(message.starts_with("unknown field"), "{message}");

    // A loop: a synth's LFO output into its own cutoff input.
    let mut looped = value.clone();
    looped["cables"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from": "d001:lfo_cv_out", "to": "d001:cutoff_cv_in"}));
    let (path, message) = load_err(&looped, &reg);
    assert_eq!(path, format!("cables[{n}]"), "{message}");
    assert!(message.contains("would create a cycle"), "{message}");
    // The unchanged file still loads.
    load_project(&value.to_string(), &reg).unwrap();
}

fn first_notes(value: &mut Value) -> &mut Value {
    &mut value["song"]["tracks"][0]["lanes"][0]["clips"][0]["notes"]
}

#[test]
fn a_bad_note_among_many_names_its_own_index_and_field() {
    let reg = registry();
    let text = save_project(&large_song_doc(&reg, 2, 2, 400));
    let value: Value = serde_json::from_str(&text).unwrap();
    let base = "song.tracks[0].lanes[0].clips[0].notes";

    let mut v = value.clone();
    first_notes(&mut v)[150][3] = json!(1.5);
    let (path, message) = load_err(&v, &reg);
    assert_eq!(path, format!("{base}[150][3]"), "{message}");

    let mut v = value.clone();
    first_notes(&mut v)[20][2] = json!(200);
    let (path, message) = load_err(&v, &reg);
    assert_eq!(path, format!("{base}[20][2]"), "{message}");
    assert!(message.contains("outside 0..=127"), "{message}");

    let mut v = value.clone();
    first_notes(&mut v)[9] = json!([1, 2, 3]);
    let (path, message) = load_err(&v, &reg);
    assert_eq!(path, format!("{base}[9]"), "{message}");
    assert!(message.starts_with("expected a note"), "{message}");

    let mut v = value.clone();
    let earlier = first_notes(&mut v)[4].clone();
    first_notes(&mut v)[5] = earlier;
    let (path, message) = load_err(&v, &reg);
    assert_eq!(path, format!("{base}[5]"), "{message}");
    assert_eq!(
        message,
        "notes must be sorted by (tick, key) without duplicates"
    );
}

#[test]
fn a_bad_point_among_many_names_its_own_index_and_field() {
    let reg = registry();
    let text = save_project(&lanes_song_doc(&reg, 3));
    let value: Value = serde_json::from_str(&text).unwrap();
    let base = "song.tracks[0].lanes[1].clips[0].points";

    let mut v = value.clone();
    let p = lane_points(&mut v);
    p[2][2] = json!("smooth");
    let (path, message) = load_err(&v, &reg);
    assert_eq!(path, format!("{base}[2][2]"), "{message}");
    assert!(
        message.contains("expected \"linear\" or \"step\""),
        "{message}"
    );

    let mut v = value.clone();
    let p = lane_points(&mut v);
    let earlier = p[1].clone();
    p[2] = earlier;
    let (path, message) = load_err(&v, &reg);
    assert_eq!(path, format!("{base}[2]"), "{message}");
    assert_eq!(message, "points must be sorted by tick without duplicates");
}

fn lane_points(v: &mut Value) -> &mut Value {
    &mut v["song"]["tracks"][0]["lanes"][1]["clips"][0]["points"]
}
