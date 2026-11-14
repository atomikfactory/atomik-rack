//! A note clip can opt out of the song key: `SetClipFollowKey`, undo, what keeps it, and the
//! ignorable `follow_song_key` field of format 45 (written only when off).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    ClipId, Command, Document, ModelError, SongCommand, SyncOp, WorkflowCommand, load_project,
    save_project,
};
use common::song::*;
use common::*;
use serde_json::Value;

fn doc(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    d.apply(
        add_clip(
            "synth",
            0,
            notes_clip(0, 2 * BAR, vec![note(0, PPQ, 60, 0.8)]),
        ),
        reg,
    )
    .unwrap();
    d
}

fn follows(d: &Document, id: u64) -> bool {
    d.doc().song.clip(ClipId(id)).unwrap().2.follow_song_key
}

fn set(d: &mut Document, id: u64, follow: bool, reg: &Registry) -> Vec<SyncOp> {
    d.apply(
        sc(SongCommand::SetClipFollowKey {
            clip: ClipId(id),
            follow,
        }),
        reg,
    )
    .unwrap()
}

#[test]
fn a_new_clip_follows_the_song_key() {
    let reg = registry();
    assert!(follows(&doc(&reg), 1));
}

#[test]
fn the_command_reports_the_song_and_undoes_as_one_step() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    assert_eq!(set(&mut d, 1, false, &reg), vec![SyncOp::SongChanged]);
    assert!(!follows(&d, 1));
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
    d.redo().unwrap();
    assert!(!follows(&d, 1));
}

#[test]
fn only_note_clips_take_it_and_an_unknown_clip_is_refused() {
    let reg = registry();
    let mut d = doc(&reg);
    let e = d
        .apply(
            sc(SongCommand::SetClipFollowKey {
                clip: ClipId(99),
                follow: false,
            }),
            &reg,
        )
        .expect_err("no such clip");
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e:?}");
    let mut bad = auto_clip(BAR, BAR, vec![point(0, 0.5, atomik_model::Curve::Linear)]);
    bad.follow_song_key = false;
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: "filter.cutoff".into(),
        }),
        &reg,
    )
    .unwrap();
    assert!(d.apply(add_clip("synth", 1, bad), &reg).is_err());
}

#[test]
fn split_duplicate_and_join_keep_the_choice() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, false, &reg);
    d.apply(
        sc(SongCommand::SplitClip {
            id: ClipId(1),
            at: BAR,
        }),
        &reg,
    )
    .unwrap();
    let ids = clip_ids(d.doc());
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|id| !follows(&d, id.0)));
    d.apply(
        Command::Workflow(WorkflowCommand::JoinClips { ids: ids.clone() }),
        &reg,
    )
    .unwrap();
    let ids = clip_ids(d.doc());
    assert_eq!(ids.len(), 1);
    assert!(!follows(&d, ids[0].0));
    d.apply(
        Command::Workflow(WorkflowCommand::DuplicateClips { ids: ids.clone() }),
        &reg,
    )
    .unwrap();
    assert!(clip_ids(d.doc()).iter().all(|id| !follows(&d, id.0)));
}

#[test]
fn the_field_is_written_only_when_off_and_round_trips() {
    let reg = registry();
    let mut d = doc(&reg);
    assert!(!save_project(d.doc()).contains("\"follow_song_key\": "));
    set(&mut d, 1, false, &reg);
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let clip = &v["song"]["tracks"][0]["lanes"][0]["clips"][0];
    assert_eq!(clip["follow_song_key"], Value::Bool(false));
    assert_eq!(v["version"], 45);
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text, "canonical");
}

#[test]
fn a_stored_true_is_refused_and_a_file_without_the_field_follows() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, false, &reg);
    let text = save_project(d.doc());
    let bad = text.replace("\"follow_song_key\": false", "\"follow_song_key\": true");
    assert!(load_project(&bad, &reg).is_err());
    let plain = save_project(doc(&reg).doc());
    let loaded = load_project(&plain, &reg).unwrap();
    assert!(loaded.song.clip(ClipId(1)).unwrap().2.follow_song_key);
}
