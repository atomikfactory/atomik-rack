//! Controller lanes in note clips: `SetClipControllers` and its undo, validation, what keeps
//! the lanes (split, duplicate, join), the flattening into song time, and project format 19
//! (a version 18 file loads without lanes).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::controller::{ControllerKind, ControllerLane};
use atomik_model::song::flatten_controllers;
use atomik_model::{
    ClipId, Command, Curve, Document, ModelError, PROJECT_VERSION, SongCommand, SyncOp,
    WorkflowCommand, load_project, save_project,
};
use common::song::*;
use common::*;
use serde_json::{Value, json};

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

fn clip(d: &Document, id: u64) -> atomik_model::Clip {
    d.doc().song.clip(ClipId(id)).expect("clip").2.clone()
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

fn lane(kind: ControllerKind, points: Vec<atomik_model::AutomationPoint>) -> ControllerLane {
    ControllerLane::new(kind, points)
}

fn set(
    d: &mut Document,
    id: u64,
    lanes: Vec<ControllerLane>,
    merge: Option<u64>,
    reg: &Registry,
) -> Result<Vec<SyncOp>, ModelError> {
    d.apply(
        sc(SongCommand::SetClipControllers {
            clip: ClipId(id),
            lanes,
            merge,
        }),
        reg,
    )
}

fn geometry(d: &mut Document, reg: &Registry, length: i64, offset: i64, loop_length: Option<i64>) {
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id: ClipId(1),
            start: 0,
            length,
            offset,
            loop_length,
            muted: false,
            merge: None,
        }),
        reg,
    )
    .unwrap();
}

fn ticks(d: &Document) -> Vec<i64> {
    let flat = flatten_controllers(&d.doc().song.tracks[0]);
    flat[0].1.iter().map(|p| p.0).collect()
}

#[test]
fn the_project_version_is_thirty_one() {
    assert_eq!(PROJECT_VERSION, 33);
}

#[test]
fn a_new_clip_has_no_controllers_and_writes_no_field() {
    let reg = registry();
    let d = doc(&reg);
    assert!(clip(&d, 1).controllers.is_empty());
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
        1,
        vec![wheel(), lane(ControllerKind::Cc(7), vec![]), bend()],
        None,
        &reg,
    )
    .unwrap();
    assert_eq!(ops, vec![SyncOp::SongChanged]);
    let kinds: Vec<_> = clip(&d, 1).controllers.iter().map(|l| l.kind).collect();
    assert_eq!(kinds, [ControllerKind::PitchBend, ControllerKind::Cc(1)]);
    // The notes are untouched.
    assert_eq!(
        clip(&d, 1).content,
        before.song.clip(ClipId(1)).unwrap().2.content
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&reg).unwrap();
    assert_eq!(clip(&d, 1).controllers.len(), 2);
}

#[test]
fn drag_edits_with_one_merge_id_are_one_undo_step() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, vec![wheel()], None, &reg).unwrap();
    let before = d.doc().clone();
    for v in [0.3, 0.5, 0.9] {
        let l = lane(ControllerKind::Cc(1), vec![point(10, v, Curve::Step)]);
        set(&mut d, 1, vec![l], Some(7), &reg).unwrap();
    }
    assert_eq!(clip(&d, 1).controllers[0].points[0].value, 0.9);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before, "the whole drag undoes at once");
}

#[test]
fn bad_lanes_are_refused_without_a_change() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    let cc = ControllerKind::Cc(1);
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
        assert!(set(&mut d, 1, lanes, None, &reg).is_err(), "case {i}");
        assert_eq!(d.doc(), &before, "case {i}");
    }
    let e = set(&mut d, 99, vec![wheel()], None, &reg).expect_err("no such clip");
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e:?}");
}

#[test]
fn only_note_clips_take_controllers() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: "filter.cutoff".into(),
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        add_clip(
            "synth",
            1,
            auto_clip(0, BAR, vec![point(0, 0.5, Curve::Linear)]),
        ),
        &reg,
    )
    .unwrap();
    let auto = d.last_clip_id().unwrap();
    let before = d.doc().clone();
    let e = set(&mut d, auto.0, vec![wheel()], None, &reg).expect_err("automation clip");
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    assert_eq!(d.doc(), &before);
    let mut c = auto_clip(BAR, BAR, vec![point(0, 0.5, Curve::Linear)]);
    c.controllers = vec![wheel()];
    assert!(d.apply(add_clip("synth", 1, c), &reg).is_err());
}

#[test]
fn add_clip_keeps_controllers_sorted_and_without_empty_lanes() {
    let reg = registry();
    let mut d = doc(&reg);
    let mut c = notes_clip(2 * BAR, BAR, vec![note(0, PPQ, 62, 0.8)]);
    c.controllers = vec![lane(ControllerKind::Cc(9), vec![]), bend()];
    d.apply(add_clip("synth", 0, c), &reg).unwrap();
    let id = d.last_clip_id().unwrap();
    assert_eq!(clip(&d, id.0).controllers, vec![bend()]);
}

#[test]
fn a_split_and_a_duplicate_keep_the_lanes() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, vec![bend(), wheel()], None, &reg).unwrap();
    d.apply(
        sc(SongCommand::SplitClip {
            id: ClipId(1),
            at: BAR,
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        Command::Workflow(WorkflowCommand::DuplicateClips {
            ids: vec![ClipId(1)],
        }),
        &reg,
    )
    .unwrap();
    let ids = clip_ids(d.doc());
    assert_eq!(ids.len(), 3);
    for id in ids {
        assert_eq!(clip(&d, id.0).controllers.len(), 2, "clip {}", id.0);
    }
}

#[test]
fn a_join_merges_the_lanes_by_time_and_later_clips_win() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(
        add_clip(
            "synth",
            0,
            notes_clip(2 * BAR, BAR, vec![note(0, PPQ, 62, 0.8)]),
        ),
        &reg,
    )
    .unwrap();
    set(&mut d, 1, vec![bend()], None, &reg).unwrap();
    set(
        &mut d,
        2,
        vec![
            lane(
                ControllerKind::PitchBend,
                vec![point(PPQ, 0.75, Curve::Step)],
            ),
            wheel(),
        ],
        None,
        &reg,
    )
    .unwrap();
    d.apply(
        Command::Workflow(WorkflowCommand::JoinClips {
            ids: vec![ClipId(1), ClipId(2)],
        }),
        &reg,
    )
    .unwrap();
    let ids = clip_ids(d.doc());
    assert_eq!(ids.len(), 1);
    let joined = clip(&d, ids[0].0);
    assert_eq!(joined.controllers.len(), 2);
    let pb = &joined.controllers[0];
    assert_eq!(pb.kind, ControllerKind::PitchBend);
    // Clip 2 starts at 2 bars; its point lands there plus PPQ; clip 1's points are kept.
    assert!(
        pb.points
            .iter()
            .any(|p| p.tick == 2 * BAR + PPQ && (p.value - 0.75).abs() < 1e-6)
    );
    assert!(
        pb.points
            .iter()
            .any(|p| p.tick == 0 && p.value.abs() < 1e-6)
    );
    assert_eq!(joined.controllers[1].points[0].tick, 2 * BAR + PPQ / 2);
}

#[test]
fn flatten_follows_offset_loop_and_hiding() {
    let reg = registry();
    let mut d = doc(&reg);
    // Cc1 points at tick 100 (value .5) and at PPQ (value 1).
    let l = lane(
        ControllerKind::Cc(1),
        vec![point(100, 0.5, Curve::Step), point(PPQ, 1.0, Curve::Step)],
    );
    set(&mut d, 1, vec![l], None, &reg).unwrap();
    let flat = flatten_controllers(&d.doc().song.tracks[0]);
    assert_eq!(flat.len(), 1);
    assert_eq!(flat[0].0, ControllerKind::Cc(1));
    // Nothing before the first point; points at their own ticks.
    assert_eq!(
        flat[0].1,
        vec![(100, 0.5, Curve::Step), (PPQ, 1.0, Curve::Step)]
    );
    // An offset into the clip carries the value held there to the start.
    geometry(&mut d, &reg, 2 * BAR, 200, None);
    let flat = flatten_controllers(&d.doc().song.tracks[0]);
    assert_eq!(
        flat[0].1,
        vec![(0, 0.5, Curve::Step), (PPQ - 200, 1.0, Curve::Step)]
    );
    // A loop repeats the window: the second pass lacks the point at the window end.
    geometry(&mut d, &reg, 2 * PPQ, 0, Some(PPQ + 1));
    assert_eq!(ticks(&d), vec![100, PPQ, PPQ + 1 + 100]);
    // A later clip over the middle hides it; the visible tail starts with the held value.
    d.apply(add_clip("synth", 0, notes_clip(PPQ / 2, PPQ, vec![])), &reg)
        .unwrap();
    let flat = flatten_controllers(&d.doc().song.tracks[0]);
    assert_eq!(
        flat[0].1,
        vec![(100, 0.5, Curve::Step), (3 * PPQ / 2, 0.5, Curve::Step)]
    );
}

#[test]
fn a_ramp_cut_by_a_clip_end_gets_a_closing_point() {
    let reg = registry();
    let mut d = doc(&reg);
    let ramp = lane(
        ControllerKind::PitchBend,
        vec![
            point(0, -1.0, Curve::Linear),
            point(4 * PPQ, 1.0, Curve::Step),
        ],
    );
    set(&mut d, 1, vec![ramp], None, &reg).unwrap();
    geometry(&mut d, &reg, 2 * PPQ, 0, None);
    let flat = flatten_controllers(&d.doc().song.tracks[0]);
    let pts = &flat[0].1;
    assert_eq!(pts.len(), 2);
    assert_eq!((pts[0].0, pts[0].1, pts[0].2), (0, -1.0, Curve::Linear));
    assert_eq!((pts[1].0, pts[1].2), (2 * PPQ - 1, Curve::Step));
    assert!(
        pts[1].1 > -0.01 && pts[1].1 < 0.0,
        "about the middle: {}",
        pts[1].1
    );
}

// ---- project format 19 -------------------------------------------------------------------

#[test]
fn lanes_round_trip_canonically_and_name_their_kind() {
    let reg = registry();
    let mut d = doc(&reg);
    let poly = lane(
        ControllerKind::PolyPressure(60),
        vec![point(5, 0.125, Curve::Linear), point(9, 0.5, Curve::Step)],
    );
    let pressure = lane(
        ControllerKind::ChannelPressure,
        vec![point(0, 1.0, Curve::Step)],
    );
    set(&mut d, 1, vec![bend(), pressure, wheel(), poly], None, &reg).unwrap();
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 33);
    let lanes = &v["song"]["tracks"][0]["lanes"][0]["clips"][0]["controllers"];
    assert_eq!(lanes[0]["kind"], "pitch_bend");
    assert!(lanes[0].get("number").is_none());
    assert_eq!(lanes[1]["kind"], "pressure");
    assert_eq!(
        lanes[2],
        json!({"kind": "cc", "number": 1, "points": [[PPQ / 2, 0.25, "step"]]})
    );
    assert_eq!(lanes[3]["kind"], "poly_pressure");
    assert_eq!(lanes[3]["number"], 60);
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text, "canonical");
}

#[test]
fn a_version_18_project_loads_without_lanes_and_resaves_as_19() {
    let reg = registry();
    let text = save_project(doc(&reg).doc());
    let old = text.replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 18",
        1,
    );
    let loaded = load_project(&old, &reg).unwrap();
    assert!(
        loaded
            .song
            .clip(ClipId(1))
            .unwrap()
            .2
            .controllers
            .is_empty()
    );
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn controllers_are_unknown_before_version_19() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, vec![wheel()], None, &reg).unwrap();
    let text = save_project(d.doc()).replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 18",
        1,
    );
    let e = load_project(&text, &reg).expect_err("v18 has no controller lanes");
    assert!(
        e.to_string().contains("controllers") && e.to_string().contains("unknown field"),
        "{e}"
    );
}

#[test]
fn bad_lanes_in_a_file_name_their_json_path() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, vec![bend(), wheel()], None, &reg).unwrap();
    let good = save_project(d.doc());
    let base = "song.tracks[0].lanes[0].clips[0].controllers";
    type Edit = Box<dyn Fn(&mut Value)>;
    fn ctl(v: &mut Value) -> &mut Value {
        &mut v["song"]["tracks"][0]["lanes"][0]["clips"][0]["controllers"]
    }
    let cases: Vec<(Edit, String, &str)> = vec![
        (
            Box::new(|v| ctl(v)[1]["kind"] = json!("mod")),
            format!("{base}[1].kind"),
            "expected",
        ),
        (
            Box::new(|v| {
                ctl(v)[1].as_object_mut().unwrap().remove("number");
            }),
            format!("{base}[1]"),
            "needs a",
        ),
        (
            Box::new(|v| ctl(v)[0]["number"] = json!(3)),
            format!("{base}[0]"),
            "has no",
        ),
        (
            Box::new(|v| ctl(v)[1]["number"] = json!(128)),
            format!("{base}[1].number"),
            "",
        ),
        (
            Box::new(|v| ctl(v)[1]["points"][0][1] = json!(2.0)),
            base.to_owned(),
            "outside",
        ),
        (
            Box::new(|v| ctl(v)[1]["points"][0][2] = json!("ramp")),
            format!("{base}[1].points[0][2]"),
            "linear",
        ),
        (
            Box::new(|v| {
                let first = ctl(v)[0].clone();
                ctl(v)[1] = first;
            }),
            base.to_owned(),
            "sorted",
        ),
        (
            Box::new(|v| ctl(v)[1]["points"] = json!([])),
            base.to_owned(),
            "no points",
        ),
        (
            Box::new(|v| ctl(v)[1]["extra"] = json!(1)),
            format!("{base}[1]"),
            "unknown field",
        ),
    ];
    for (i, (edit, path, needle)) in cases.into_iter().enumerate() {
        let mut v: Value = serde_json::from_str(&good).unwrap();
        edit(&mut v);
        let e = load_project(&v.to_string(), &reg).expect_err("must be rejected");
        let ModelError::Patch(msg) = &e else {
            panic!("case {i}: expected a patch error, got {e:?}")
        };
        assert!(msg.starts_with(&path), "case {i}: {msg} (wanted {path})");
        assert!(msg.contains(needle), "case {i}: {msg} lacks {needle:?}");
    }
}
