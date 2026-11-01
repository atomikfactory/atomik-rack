//! The per-track Direct Record flag: default, command, undo, rejection on an audio track and the
//! optional `direct_record` field of project format 39 (written only when set).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{Document, SongCommand, SyncOp, load_project, save_project};
use common::audio::{audio_registry, with_audio_track};
use common::song::*;
use common::*;

fn doc(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    d.apply(add_track("lead"), reg).unwrap();
    d
}

fn set(k: &str, on: bool) -> atomik_model::Command {
    sc(SongCommand::SetDirectRecord { device: key(k), on })
}

fn flag(d: &Document, k: &str) -> bool {
    d.doc().song.track(&key(k)).unwrap().direct_record
}

#[test]
fn a_new_track_records_what_is_played() {
    let reg = registry();
    let d = doc(&reg);
    assert!(!flag(&d, "synth") && !flag(&d, "lead"));
}

#[test]
fn the_command_changes_one_track_in_one_undo_step() {
    let reg = registry();
    let mut d = doc(&reg);
    let ops = d.apply(set("synth", true), &reg).unwrap();
    assert_eq!(ops, vec![SyncOp::SongChanged]);
    assert!(flag(&d, "synth") && !flag(&d, "lead"));
    d.undo(&reg).unwrap();
    assert!(!flag(&d, "synth"));
    d.redo(&reg).unwrap();
    assert!(flag(&d, "synth"));
}

#[test]
fn an_unknown_track_is_rejected_and_changes_nothing() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    d.apply(set("nope", true), &reg).expect_err("no such track");
    assert_eq!(d.doc(), &before);
}

#[test]
fn an_audio_track_has_no_note_lane_to_record_into() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    let audio = d.doc().song.tracks[0].device.0.clone();
    d.apply(set(&audio, true), &reg)
        .expect_err("an audio track records audio");
}

#[test]
fn the_field_is_written_only_when_set_and_round_trips() {
    let reg = registry();
    let mut d = doc(&reg);
    assert!(!save_project(d.doc()).contains("direct_record"));
    d.apply(set("lead", true), &reg).unwrap();
    let text = save_project(d.doc());
    assert_eq!(text.matches("\"direct_record\": true").count(), 1);
    let loaded = load_project(&text, &reg).unwrap();
    assert!(loaded.song.track(&key("lead")).unwrap().direct_record);
    assert!(!loaded.song.track(&key("synth")).unwrap().direct_record);
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_non_boolean_field_names_its_path() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(set("lead", true), &reg).unwrap();
    let text = save_project(d.doc()).replace("\"direct_record\": true", "\"direct_record\": 3");
    let e = load_project(&text, &reg).expect_err("not a boolean");
    assert!(e.to_string().contains("direct_record"), "{e}");
}
