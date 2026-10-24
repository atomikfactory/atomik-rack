//! The per-track chase flag: default, command, undo, and project format 17 (older files chase).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{Document, PROJECT_VERSION, SongCommand, SyncOp, load_project, save_project};
use common::song::*;
use common::*;
use serde_json::Value;

fn doc(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    d.apply(add_track("lead"), reg).unwrap();
    d
}

fn set_chase(k: &str, chase: bool) -> atomik_model::Command {
    sc(SongCommand::SetTrackChase {
        device: key(k),
        chase,
    })
}

fn chase_of(d: &Document, k: &str) -> bool {
    d.doc().song.track(&key(k)).unwrap().chase
}

#[test]
fn add_track_new_track_chases_by_default() {
    let reg = registry();
    let d = doc(&reg);
    assert!(chase_of(&d, "synth") && chase_of(&d, "lead"));
}

#[test]
fn set_track_chase_off_changes_one_track_reports_song_changed_and_undoes() {
    let reg = registry();
    let mut d = doc(&reg);
    let ops = d.apply(set_chase("synth", false), &reg).unwrap();
    assert_eq!(ops, vec![SyncOp::SongChanged]);
    assert!(!chase_of(&d, "synth") && chase_of(&d, "lead"));
    d.undo(&reg).unwrap();
    assert!(chase_of(&d, "synth"));
    d.redo(&reg).unwrap();
    assert!(!chase_of(&d, "synth"));
}

#[test]
fn set_track_chase_unknown_track_is_rejected() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    d.apply(set_chase("nope", false), &reg)
        .expect_err("no such track");
    assert_eq!(d.doc(), &before);
}

#[test]
fn set_track_flags_leaves_chase_alone() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(set_chase("synth", false), &reg).unwrap();
    d.apply(
        sc(SongCommand::SetTrackFlags {
            device: key("synth"),
            muted: true,
            solo: false,
            armed: false,
        }),
        &reg,
    )
    .unwrap();
    assert!(!chase_of(&d, "synth"));
    d.undo(&reg).unwrap();
    assert!(!chase_of(&d, "synth"));
}

#[test]
fn save_project_chasing_track_writes_no_field() {
    let reg = registry();
    let d = doc(&reg);
    let text = save_project(d.doc());
    assert!(!text.contains("no_chase"));
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 27);
    assert_eq!(PROJECT_VERSION, 27);
}

#[test]
fn project_round_trip_keeps_a_track_without_chase() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(set_chase("lead", false), &reg).unwrap();
    let text = save_project(d.doc());
    assert!(text.contains("\"no_chase\": true"));
    let loaded = load_project(&text, &reg).unwrap();
    assert!(!loaded.song.track(&key("lead")).unwrap().chase);
    assert!(loaded.song.track(&key("synth")).unwrap().chase);
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn load_project_version_16_file_chases_and_resaves_as_17() {
    let reg = registry();
    let d = doc(&reg);
    let old = save_project(d.doc()).replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 16",
        1,
    );
    let loaded = load_project(&old, &reg).unwrap();
    assert!(loaded.song.tracks.iter().all(|t| t.chase));
    assert_eq!(save_project(&loaded), as_current(&old, 16));
}

#[test]
fn load_project_no_chase_before_version_17_is_an_unknown_field() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(set_chase("synth", false), &reg).unwrap();
    let text = save_project(d.doc()).replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 16",
        1,
    );
    let e = load_project(&text, &reg).expect_err("v16 has no chase flag");
    let s = e.to_string();
    assert!(s.contains("no_chase") && s.contains("unknown field"), "{s}");
}

#[test]
fn load_project_non_boolean_no_chase_names_its_path() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(set_chase("synth", false), &reg).unwrap();
    let text = save_project(d.doc()).replace("\"no_chase\": true", "\"no_chase\": 3");
    let e = load_project(&text, &reg).expect_err("not a boolean");
    assert!(e.to_string().contains("no_chase"), "{e}");
}
