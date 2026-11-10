//! A note track's own controller lanes (`Track::controllers`, `SetTrackControllers`): validation,
//! undo and drag merging, the song end, playing with the controllers of the clips, what the track
//! operations keep, and the project format (the field is new in version 44).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::controller::{ControllerKind, ControllerLane};
use atomik_model::song::{flatten_clip_controllers, flatten_controllers};
use atomik_model::{
    Command, Curve, Document, ModelError, PROJECT_VERSION, SongCommand, SyncOp, WorkflowCommand,
    load_project, save_project,
};
use common::audio::{audio_registry, with_audio_track};
use common::song::*;
use common::*;
use serde_json::Value;

fn doc(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    d
}

fn bend() -> ControllerLane {
    ControllerLane::new(
        ControllerKind::PitchBend,
        vec![
            point(0, 0.0, Curve::Linear),
            point(PPQ, 0.5, Curve::Step),
            point(2 * PPQ, -1.0, Curve::Step),
        ],
    )
}

fn wheel() -> ControllerLane {
    ControllerLane::new(
        ControllerKind::Cc(1),
        vec![point(PPQ / 2, 0.25, Curve::Step)],
    )
}

fn set(
    d: &mut Document,
    device: &str,
    lanes: Vec<ControllerLane>,
    merge: Option<u64>,
    reg: &Registry,
) -> Result<Vec<SyncOp>, ModelError> {
    d.apply(
        sc(SongCommand::SetTrackControllers {
            device: key(device),
            lanes,
            merge,
        }),
        reg,
    )
}

fn own(d: &Document) -> Vec<ControllerLane> {
    d.doc().song.tracks[0].controllers.clone()
}

#[test]
fn the_project_version_is_forty_four() {
    assert_eq!(PROJECT_VERSION, 45);
}

#[test]
fn a_new_track_has_no_own_controllers_and_writes_no_field() {
    let reg = registry();
    let d = doc(&reg);
    assert!(own(&d).is_empty());
    assert!(!save_project(d.doc()).contains("controllers"));
}

#[test]
fn set_stores_sorts_reports_the_song_and_undoes_as_one_step() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    // Unsorted input and an empty lane: stored sorted, the empty lane dropped.
    let ops = set(
        &mut d,
        "synth",
        vec![
            wheel(),
            ControllerLane::new(ControllerKind::Cc(7), vec![]),
            bend(),
        ],
        None,
        &reg,
    )
    .unwrap();
    assert_eq!(ops, vec![SyncOp::SongChanged]);
    let kinds: Vec<_> = own(&d).iter().map(|l| l.kind).collect();
    assert_eq!(kinds, [ControllerKind::PitchBend, ControllerKind::Cc(1)]);
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
    d.redo().unwrap();
    assert_eq!(own(&d).len(), 2);
    // An empty list clears them again, and that is undoable too.
    set(&mut d, "synth", vec![], None, &reg).unwrap();
    assert!(own(&d).is_empty());
    d.undo().unwrap();
    assert_eq!(own(&d).len(), 2);
}

#[test]
fn drag_edits_with_one_merge_id_are_one_undo_step() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, "synth", vec![wheel()], None, &reg).unwrap();
    let before = d.doc().clone();
    for v in [0.3, 0.5, 0.9] {
        let l = ControllerLane::new(ControllerKind::Cc(1), vec![point(10, v, Curve::Step)]);
        set(&mut d, "synth", vec![l], Some(7), &reg).unwrap();
    }
    assert_eq!(own(&d)[0].points[0].value, 0.9);
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "the whole drag undoes at once");
    d.redo().unwrap();
    assert_eq!(own(&d)[0].points[0].value, 0.9);
}

#[test]
fn bad_lanes_and_unknown_tracks_are_refused_without_a_change() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    let cc = ControllerKind::Cc(1);
    let lane = ControllerLane::new;
    let bad: Vec<Vec<ControllerLane>> = vec![
        vec![lane(cc, vec![point(0, 1.5, Curve::Step)])],
        vec![lane(cc, vec![point(0, -0.5, Curve::Step)])],
        vec![lane(
            ControllerKind::PitchBend,
            vec![point(0, -1.5, Curve::Step)],
        )],
        vec![lane(cc, vec![point(-3, 0.5, Curve::Step)])],
        vec![lane(
            ControllerKind::Cc(200),
            vec![point(0, 0.5, Curve::Step)],
        )],
        vec![lane(
            cc,
            vec![point(5, 0.5, Curve::Step), point(5, 0.6, Curve::Step)],
        )],
        vec![wheel(), wheel()],
        vec![lane(cc, vec![point(0, f32::NAN, Curve::Step)])],
    ];
    for (i, lanes) in bad.into_iter().enumerate() {
        assert!(set(&mut d, "synth", lanes, None, &reg).is_err(), "case {i}");
        assert_eq!(d.doc(), &before, "case {i}");
    }
    let e = set(&mut d, "ghost", vec![wheel()], None, &reg).expect_err("no such track");
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
}

#[test]
fn an_audio_track_has_no_controller_lanes() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    let before = d.doc().clone();
    let e = set(&mut d, "audio-track", vec![wheel()], None, &reg).unwrap_err();
    assert!(matches!(e, ModelError::BadSong(_)), "{e}");
    assert_eq!(d.doc(), &before);
    // Clearing nothing is allowed anywhere.
    set(&mut d, "audio-track", vec![], None, &reg).unwrap();
}

#[test]
fn a_point_past_the_song_end_moves_the_end_and_one_undo_puts_both_back() {
    let reg = registry();
    let mut d = doc(&reg);
    let end = d.doc().song.end;
    let late = ControllerLane::new(
        ControllerKind::Cc(74),
        vec![point(end + 3 * BAR + 5, 0.5, Curve::Step)],
    );
    set(&mut d, "synth", vec![late], None, &reg).unwrap();
    assert!(d.doc().song.end > end + 3 * BAR + 5);
    d.undo().unwrap();
    assert_eq!(d.doc().song.end, end);
    assert!(own(&d).is_empty());
}

#[test]
fn the_track_plays_its_own_lanes_with_the_clips_and_wins_an_equal_tick() {
    let reg = registry();
    let mut d = doc(&reg);
    // Only the track's own lanes: they flatten as they are.
    set(&mut d, "synth", vec![bend(), wheel()], None, &reg).unwrap();
    let flat = flatten_controllers(&d.doc().song.tracks[0]);
    let kinds: Vec<_> = flat.iter().map(|f| f.0).collect();
    assert_eq!(kinds, [ControllerKind::PitchBend, ControllerKind::Cc(1)]);
    assert_eq!(
        flat[0].1,
        vec![
            (0, 0.0, Curve::Linear),
            (PPQ, 0.5, Curve::Step),
            (2 * PPQ, -1.0, Curve::Step)
        ]
    );
    assert!(flatten_clip_controllers(&d.doc().song.tracks[0]).is_empty());
    // A clip with a wheel point of its own at the same tick and another of the same kind later.
    let mut c = notes_clip(0, 2 * BAR, vec![note(0, PPQ, 60, 0.8)]);
    c.controllers = vec![ControllerLane::new(
        ControllerKind::Cc(1),
        vec![
            point(PPQ / 2, 0.9, Curve::Step),
            point(PPQ, 0.7, Curve::Step),
        ],
    )];
    d.apply(add_clip("synth", 0, c), &reg).unwrap();
    let flat = flatten_controllers(&d.doc().song.tracks[0]);
    let wheel_points = &flat[1].1;
    assert_eq!(
        wheel_points,
        &vec![(PPQ / 2, 0.25, Curve::Step), (PPQ, 0.7, Curve::Step)],
        "the track's point wins the equal tick, the clip's later point stays"
    );
    // The clip alone is what the merged lane keeps.
    assert_eq!(
        flatten_clip_controllers(&d.doc().song.tracks[0])[0].1.len(),
        2
    );
}

#[test]
fn merging_the_note_lanes_leaves_the_track_lanes_where_they_are() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(
        add_clip("synth", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])),
        &reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::AddNoteLane {
            device: key("synth"),
            name: None,
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        add_clip(
            "synth",
            1,
            notes_clip(0, BAR, vec![note(PPQ, PPQ, 64, 0.8)]),
        ),
        &reg,
    )
    .unwrap();
    set(&mut d, "synth", vec![wheel()], None, &reg).unwrap();
    d.apply(
        sc(SongCommand::MergeNoteLanes {
            devices: vec![key("synth")],
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(own(&d), vec![wheel()]);
    let merged: usize = d.doc().song.tracks[0]
        .lanes
        .iter()
        .flat_map(|l| l.clips.iter())
        .map(|c| c.controllers.len())
        .sum();
    assert_eq!(merged, 0, "the clips did not take the track's own lanes");
}

#[test]
fn duplicating_the_track_copies_the_lanes() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, "synth", vec![bend()], None, &reg).unwrap();
    d.apply(
        Command::Workflow(WorkflowCommand::DuplicateTrack {
            device: key("synth"),
        }),
        &reg,
    )
    .unwrap();
    let copies: Vec<_> = d
        .doc()
        .song
        .tracks
        .iter()
        .filter(|t| !t.controllers.is_empty())
        .collect();
    assert_eq!(copies.len(), 2);
    assert_eq!(copies[0].controllers, copies[1].controllers);
}

#[test]
fn the_lanes_round_trip_in_format_44_and_old_versions_refuse_them() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, "synth", vec![wheel(), bend()], None, &reg).unwrap();
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let lanes = &v["song"]["tracks"][0]["controllers"];
    assert_eq!(lanes[0]["kind"], "pitch_bend");
    assert_eq!(lanes[1]["kind"], "cc");
    assert_eq!(lanes[1]["number"], 1);
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded.song.tracks[0].controllers.len(), 2);
    assert_eq!(save_project(&loaded), text, "canonical");
    // A file older than the field does not know it.
    let old = text.replace("\"version\": 45", "\"version\": 43");
    let e = load_project(&old, &reg).unwrap_err().to_string();
    assert!(e.contains("controllers"), "{e}");
}

#[test]
fn a_file_with_bad_track_lanes_is_refused_with_the_path() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, "synth", vec![bend()], None, &reg).unwrap();
    let text = save_project(d.doc());
    let with = |f: &dyn Fn(&mut Value)| {
        let mut v: Value = serde_json::from_str(&text).unwrap();
        f(&mut v);
        v.to_string()
    };
    let value = with(&|v| v["song"]["tracks"][0]["controllers"][0]["points"][1][1] = 7.into());
    let e = load_project(&value, &reg).unwrap_err().to_string();
    assert!(e.contains("controllers"), "{e}");
    let kind = with(&|v| v["song"]["tracks"][0]["controllers"][0]["kind"] = "tilt".into());
    assert!(load_project(&kind, &reg).is_err());
}

#[test]
fn a_file_gives_an_audio_track_no_controller_lanes() {
    let reg = audio_registry();
    let d = with_audio_track(&reg);
    let text = save_project(d.doc());
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["song"]["tracks"][0]["controllers"] = serde_json::json!([
        {"kind": "cc", "number": 1, "points": [[0, 0.5, "step"]]}
    ]);
    let e = load_project(&v.to_string(), &reg).unwrap_err().to_string();
    assert!(e.contains("controller"), "{e}");
}
