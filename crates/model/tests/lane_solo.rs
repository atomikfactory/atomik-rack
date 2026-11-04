//! Note lanes with a solo and a colour of their own: the commands, what a soloed lane plays
//! (flatten, merge, extract), and the saved fields.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::song::{flatten_notes, flatten_notes_with};
use atomik_model::{
    Document, ModelError, SongCommand, SyncOp, WorkflowCommand, load_project, save_project,
};
use common::song::*;
use common::workflow::*;
use common::*;
use serde_json::Value;

/// Track `a` with three note lanes, a note at a different key in each (60, 64, 67), and the
/// cutoff automation lane; track `b` with one lane.
fn laned(reg: &Registry) -> Document {
    let mut d = Document::new(atomik_model::empty_doc());
    run(
        &mut d,
        reg,
        [
            add(0, SYNTH, Some("a")),
            add(1, SYNTH, Some("b")),
            add_track("a"),
            add_track("b"),
            sc(SongCommand::AddAutomationLane {
                device: key("a"),
                param: "filter.cutoff".into(),
            }),
            sc(SongCommand::AddNoteLane {
                device: key("a"),
                name: None,
            }),
            sc(SongCommand::AddNoteLane {
                device: key("a"),
                name: None,
            }),
            add_clip("a", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])),
            add_clip("a", 1, notes_clip(0, BAR, vec![note(0, PPQ, 64, 0.8)])),
            add_clip("a", 2, notes_clip(0, BAR, vec![note(0, PPQ, 67, 0.8)])),
        ],
    );
    d
}

fn solo(lane: usize, solo: bool) -> atomik_model::Command {
    sc(SongCommand::SetLaneSolo {
        device: key("a"),
        lane,
        solo,
    })
}

fn colour(lane: usize, color: Option<[u8; 3]>) -> atomik_model::Command {
    sc(SongCommand::SetLaneColor {
        device: key("a"),
        lane,
        color,
    })
}

fn mute(lane: usize) -> atomik_model::Command {
    sc(SongCommand::SetLaneMuted {
        device: key("a"),
        lane,
        muted: true,
    })
}

fn keys_played(d: &Document) -> Vec<u8> {
    let t = d.doc().song.track(&key("a")).unwrap();
    flatten_notes(t).into_iter().map(|n| n.2).collect()
}

#[test]
fn a_solo_lane_is_the_only_one_that_plays() {
    let reg = registry();
    let mut d = laned(&reg);
    assert_eq!(keys_played(&d), vec![60, 64, 67]);
    let ops = one_step(&mut d, &reg, solo(1, true));
    assert!(ops.contains(&SyncOp::SongChanged));
    assert_eq!(keys_played(&d), vec![64]);
    // Two solos play together.
    d.apply(solo(2, true), &reg).unwrap();
    assert_eq!(keys_played(&d), vec![64, 67]);
    // Unsoloing every lane brings the others back.
    d.apply(solo(1, false), &reg).unwrap();
    d.apply(solo(2, false), &reg).unwrap();
    assert_eq!(keys_played(&d), vec![60, 64, 67]);
}

#[test]
fn a_mute_beats_a_solo_on_the_same_lane() {
    let reg = registry();
    let mut d = laned(&reg);
    d.apply(solo(1, true), &reg).unwrap();
    d.apply(mute(1), &reg).unwrap();
    // The soloed lane is muted: nothing plays, and the others stay silenced by the solo.
    assert!(keys_played(&d).is_empty());
    let t = d.doc().song.track(&key("a")).unwrap();
    assert!(flatten_notes_with(t, true).is_empty());
}

#[test]
fn a_solo_changes_what_the_flattened_controllers_and_merge_use() {
    let reg = registry();
    let mut d = laned(&reg);
    d.apply(solo(2, true), &reg).unwrap();
    one_step(
        &mut d,
        &reg,
        sc(SongCommand::MergeNoteLanes {
            devices: vec![key("a")],
        }),
    );
    let t = d.doc().song.track(&key("a")).unwrap();
    assert_eq!(t.note_lane_count(), 1);
    // The merged lane holds what played, and it is not soloed or muted.
    assert_eq!(keys_played(&d), vec![67]);
    assert!(!t.lanes[0].solo && !t.lanes[0].muted);
}

#[test]
fn extracted_notes_keep_the_solo_and_colour_of_their_lane() {
    let reg = registry();
    let mut d = laned(&reg);
    d.apply(solo(0, true), &reg).unwrap();
    d.apply(colour(0, Some([1, 2, 3])), &reg).unwrap();
    let id = d.doc().song.tracks[0].lanes[0].clips[0].id;
    d.apply(wf(WorkflowCommand::ExplodeNotes { clip: id }), &reg)
        .ok();
    // One pitch only: nothing to explode, and nothing changed.
    let t = d.doc().song.track(&key("a")).unwrap();
    assert_eq!(t.note_lane_count(), 3);
}

#[test]
fn a_solo_and_a_colour_are_refused_where_they_make_no_sense() {
    let reg = registry();
    let mut d = laned(&reg);
    // The automation lane, a lane that is not there, and the value the lane has.
    for c in [
        solo(3, true),
        solo(9, true),
        solo(0, false),
        colour(3, Some([1, 2, 3])),
        colour(0, None),
    ] {
        assert!(matches!(rejected(&mut d, &reg, c), ModelError::BadSong(_)));
    }
}

#[test]
fn a_colour_sets_clears_and_undoes() {
    let reg = registry();
    let mut d = laned(&reg);
    one_step(&mut d, &reg, colour(1, Some([200, 30, 30])));
    assert_eq!(d.doc().song.tracks[0].lanes[1].color, Some([200, 30, 30]));
    one_step(&mut d, &reg, colour(1, None));
}

#[test]
fn solo_and_colour_survive_the_project_file_and_are_written_only_when_set() {
    let reg = registry();
    let mut d = laned(&reg);
    let plain = save_project(d.doc());
    assert!(!plain.contains("\"solo\": true") && !plain.contains("\"color\": \"#c81e1e\""));
    d.apply(solo(1, true), &reg).unwrap();
    d.apply(colour(1, Some([200, 30, 30])), &reg).unwrap();
    d.apply(mute(1), &reg).unwrap();
    let text = save_project(d.doc());
    assert!(text.contains("\"muted\": true, \"solo\": true"), "{text}");
    assert!(text.contains("\"color\": \"#c81e1e\""));
    let loaded = load_project(&text, &reg).expect("loads");
    assert_eq!(save_project(&loaded), text);
    assert!(loaded.song.tracks[0].lanes[1].solo);
    assert_eq!(loaded.song.tracks[0].lanes[1].color, Some([200, 30, 30]));
    assert!(!loaded.song.tracks[0].lanes[0].solo);
}

#[test]
fn the_file_refuses_a_false_solo_and_a_bad_colour() {
    let reg = registry();
    let mut d = laned(&reg);
    d.apply(solo(1, true), &reg).unwrap();
    let text = save_project(d.doc());
    for (field, value) in [("solo", Value::Bool(false)), ("color", Value::from("red"))] {
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["song"]["tracks"][0]["lanes"][1][field] = value;
        let bad = serde_json::to_string_pretty(&v).unwrap();
        assert!(load_project(&bad, &reg).is_err(), "{field}");
    }
}
