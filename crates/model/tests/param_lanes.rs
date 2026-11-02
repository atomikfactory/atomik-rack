//! Device-parameter lanes in note clips and bent automation segments: `SetClipParamLanes` and its
//! undo and validation, what keeps the lanes (split, duplicate, join, convert loop, reverse), the
//! flattening into song time, bends in the project file, and a project without lanes or bends
//! that saves with no new field.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::song::{flatten_automation, flatten_param_lanes};
use atomik_model::{
    ClipId, Command, Curve, Document, LoopConvert, ModelError, PROJECT_VERSION, ParamLane,
    SongCommand, SyncOp, WorkflowCommand, load_project, save_project,
};
use common::song::*;
use common::*;

const CUTOFF: &str = "filter.cutoff";

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

fn sweep() -> ParamLane {
    ParamLane::new(
        key("synth"),
        CUTOFF,
        vec![
            point(0, 200.0, Curve::Bend(32)),
            point(PPQ, 4000.0, Curve::Linear),
            point(2 * PPQ, 800.0, Curve::Step),
        ],
    )
}

fn set(
    d: &mut Document,
    id: u64,
    lanes: Vec<ParamLane>,
    merge: Option<u64>,
    reg: &Registry,
) -> Result<Vec<SyncOp>, ModelError> {
    d.apply(
        sc(SongCommand::SetClipParamLanes {
            clip: ClipId(id),
            lanes,
            merge,
        }),
        reg,
    )
}

fn flat(d: &Document) -> Vec<(i64, f32, Curve)> {
    flatten_param_lanes(&d.doc().song.tracks[0]).remove(0).2
}

#[test]
fn the_project_format_is_still_forty_one() {
    assert_eq!(PROJECT_VERSION, 41);
}

#[test]
fn a_clip_without_lanes_writes_no_field() {
    let reg = registry();
    let d = doc(&reg);
    assert!(clip(&d, 1).param_lanes.is_empty());
    let text = save_project(d.doc());
    assert!(!text.contains("param_lanes"));
    assert!(!text.contains("bend"));
}

#[test]
fn set_stores_sorts_clamps_reports_the_song_and_undoes_as_one_step() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    let delay = ParamLane::new(
        key("synth"),
        "filter.resonance",
        vec![point(0, 0.2, Curve::Step)],
    );
    let loud = ParamLane::new(
        key("synth"),
        CUTOFF,
        vec![point(0, 99_999.0, Curve::Step), point(10, 1.0, Curve::Step)],
    );
    // Unsorted input, an empty lane and a value past the range.
    let empty = ParamLane::new(key("synth"), "filter.drive", vec![]);
    let ops = set(&mut d, 1, vec![delay, empty, loud], None, &reg).unwrap();
    assert_eq!(ops, vec![SyncOp::SongChanged]);
    let lanes = clip(&d, 1).param_lanes;
    let params: Vec<_> = lanes.iter().map(|l| l.param.as_str()).collect();
    assert_eq!(params, [CUTOFF, "filter.resonance"]);
    assert!(
        lanes[0].points[0].value <= 20_000.0,
        "{:?}",
        lanes[0].points
    );
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
    d.redo().unwrap();
    assert_eq!(clip(&d, 1).param_lanes.len(), 2);
}

#[test]
fn a_drag_with_one_merge_id_is_one_undo_step() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, vec![sweep()], None, &reg).unwrap();
    let before = d.doc().clone();
    for v in [300.0, 500.0, 900.0] {
        let l = ParamLane::new(key("synth"), CUTOFF, vec![point(10, v, Curve::Step)]);
        set(&mut d, 1, vec![l], Some(7), &reg).unwrap();
    }
    assert_eq!(clip(&d, 1).param_lanes[0].points[0].value, 900.0);
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "the whole drag undoes at once");
}

#[test]
fn bad_lanes_are_refused_without_a_change() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    let lane = |device: &str, param: &str, points| ParamLane::new(key(device), param, points);
    let one = |tick, value| vec![point(tick, value, Curve::Step)];
    let bad: Vec<Vec<ParamLane>> = vec![
        vec![lane("nobody", CUTOFF, one(0, 1000.0))],
        vec![lane("synth", "no.such.param", one(0, 1.0))],
        vec![lane("synth", CUTOFF, one(-3, 1000.0))],
        vec![lane("synth", CUTOFF, one(0, f32::NAN))],
        vec![lane(
            "synth",
            CUTOFF,
            vec![point(5, 500.0, Curve::Step), point(5, 600.0, Curve::Step)],
        )],
        vec![lane(
            "synth",
            CUTOFF,
            vec![point(0, 500.0, Curve::Bend(100))],
        )],
        vec![
            lane("synth", CUTOFF, one(0, 500.0)),
            lane("synth", CUTOFF, one(0, 600.0)),
        ],
    ];
    for (i, lanes) in bad.into_iter().enumerate() {
        assert!(set(&mut d, 1, lanes, None, &reg).is_err(), "case {i}");
        assert_eq!(d.doc(), &before, "case {i}");
    }
    let e = set(&mut d, 99, vec![sweep()], None, &reg).expect_err("no such clip");
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e:?}");
}

#[test]
fn only_note_clips_take_lanes() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: CUTOFF.into(),
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        add_clip(
            "synth",
            1,
            auto_clip(0, BAR, vec![point(0, 1.0, Curve::Step)]),
        ),
        &reg,
    )
    .unwrap();
    let id = clip_ids(d.doc())
        .into_iter()
        .find(|id| id.0 != 1)
        .expect("the automation clip");
    assert!(set(&mut d, id.0, vec![sweep()], None, &reg).is_err());
    // An empty list is a no-op on any clip.
    assert!(set(&mut d, id.0, Vec::new(), None, &reg).is_ok());
}

#[test]
fn the_lanes_follow_a_split_a_duplicate_and_a_loop_conversion() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, vec![sweep()], None, &reg).unwrap();
    // Duplicate: the copy has the lane and plays it from its own start.
    d.apply(
        sc(SongCommand::DuplicateClip {
            id: ClipId(1),
            start: 4 * BAR,
        }),
        &reg,
    )
    .unwrap();
    let copy = clip_ids(d.doc()).into_iter().find(|i| i.0 != 1).unwrap();
    assert_eq!(clip(&d, copy.0).param_lanes, clip(&d, 1).param_lanes);
    assert_eq!(flat(&d).iter().filter(|p| p.0 == 4 * BAR).count(), 1);
    // Split at the first beat: both halves keep the lane (the content stays aligned).
    d.apply(
        sc(SongCommand::SplitClip {
            id: ClipId(1),
            at: PPQ,
        }),
        &reg,
    )
    .unwrap();
    let halves: Vec<_> = clip_ids(d.doc())
        .into_iter()
        .filter(|i| *i != copy)
        .collect();
    assert_eq!(halves.len(), 2);
    for h in halves {
        assert_eq!(clip(&d, h.0).param_lanes.len(), 1);
    }
}

#[test]
fn a_loop_converted_to_a_plain_clip_keeps_its_lane_for_every_repetition() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, vec![sweep()], None, &reg).unwrap();
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id: ClipId(1),
            start: 0,
            length: 4 * PPQ,
            offset: 0,
            loop_length: Some(2 * PPQ),
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let looped = flat(&d);
    d.apply(
        Command::Workflow(WorkflowCommand::ConvertLoop {
            ids: vec![ClipId(1)],
            mode: LoopConvert::Single,
        }),
        &reg,
    )
    .unwrap();
    let c = clip(&d, 1);
    assert_eq!(c.loop_length, None);
    assert_eq!(c.param_lanes.len(), 1);
    assert_eq!(flat(&d), looped, "the flattened lane is unchanged");
}

#[test]
fn a_clip_lane_in_a_muted_note_lane_plays_nothing() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, vec![sweep()], None, &reg).unwrap();
    assert!(!flatten_param_lanes(&d.doc().song.tracks[0]).is_empty());
    d.apply(
        sc(SongCommand::SetLaneMuted {
            device: key("synth"),
            lane: 0,
            muted: true,
        }),
        &reg,
    )
    .unwrap();
    assert!(flatten_param_lanes(&d.doc().song.tracks[0]).is_empty());
}

#[test]
fn a_whole_bent_segment_is_kept_and_a_cut_one_follows_the_curve() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, vec![sweep()], None, &reg).unwrap();
    // The whole clip: the bend is kept as a bend.
    assert_eq!(flat(&d)[0], (0, 200.0, Curve::Bend(32)));
    // The clip starts inside the bent segment: a chain of straight ramps takes its place.
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id: ClipId(1),
            start: 0,
            length: 2 * BAR,
            offset: PPQ / 2,
            loop_length: None,
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let cut = flat(&d);
    assert!(cut.iter().all(|p| !matches!(p.2, Curve::Bend(_))));
    assert!(cut.len() > 8, "{} points", cut.len());
    // The chain stays near the curve: k = 2 at bend 32.
    let at = |tick: i64| {
        let p = cut.partition_point(|p| p.0 <= tick) - 1;
        let (t0, v0, _) = cut[p];
        let (t1, v1, _) = cut[p + 1];
        f64::from(v0) + (f64::from(v1) - f64::from(v0)) * (tick - t0) as f64 / (t1 - t0) as f64
    };
    for tick in [10, 400, 1000, PPQ / 2 - 5] {
        let x = (tick + PPQ / 2) as f64 / PPQ as f64;
        let want = 200.0 + (4000.0 - 200.0) * x * x;
        assert!(
            (at(tick) - want).abs() < 40.0,
            "tick {tick}: {} vs {want}",
            at(tick)
        );
    }
}

#[test]
fn a_bent_automation_clip_flattens_the_bend_and_a_reverse_flips_it() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("synth"), &reg).unwrap();
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: CUTOFF.into(),
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        add_clip(
            "synth",
            1,
            auto_clip(
                0,
                2 * PPQ,
                vec![
                    point(0, 200.0, Curve::Bend(20)),
                    point(PPQ, 2000.0, Curve::Step),
                ],
            ),
        ),
        &reg,
    )
    .unwrap();
    let f = flatten_automation(&d.doc().song.tracks[0], 1);
    assert_eq!(f[0], (0, 200.0, Curve::Bend(20)));
    let id = clip_ids(d.doc())[0];
    let reverse = Command::Workflow(WorkflowCommand::ReverseClips {
        ids: vec![id],
        graphical: false,
    });
    d.apply(reverse.clone(), &reg).unwrap();
    let curves = |d: &Document| -> Vec<Curve> {
        let atomik_model::ClipContent::Automation(p) = &d.doc().song.clip(id).unwrap().2.content
        else {
            panic!("automation")
        };
        p.iter().map(|p| p.curve).collect()
    };
    assert_eq!(curves(&d), [Curve::Bend(-20), Curve::Step]);
    d.apply(reverse, &reg).unwrap();
    assert_eq!(
        curves(&d),
        [Curve::Bend(20), Curve::Step],
        "twice is the original"
    );
}

#[test]
fn bends_and_lanes_round_trip_through_the_project_file() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, vec![sweep()], None, &reg).unwrap();
    let text = save_project(d.doc());
    assert!(text.contains("\"param_lanes\""));
    assert!(text.contains("\"bend\", 32]"), "{text}");
    let back = load_project(&text, &reg).unwrap();
    assert_eq!(&back, d.doc());
    assert_eq!(save_project(&back), text);
}

#[test]
fn a_bend_outside_its_range_or_without_an_amount_is_refused_on_load() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, vec![sweep()], None, &reg).unwrap();
    let text = save_project(d.doc());
    for (from, to) in [
        ("\"bend\", 32]", "\"bend\", 99]"),
        ("\"bend\", 32]", "\"bend\", 1.5]"),
        ("\"bend\", 32]", "\"bend\"]"),
    ] {
        let broken = text.replacen(from, to, 1);
        assert_ne!(broken, text);
        assert!(load_project(&broken, &reg).is_err(), "{to}");
    }
}

#[test]
fn a_lane_whose_device_is_gone_loads_and_plays_nothing() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, vec![sweep()], None, &reg).unwrap();
    let text = save_project(d.doc()).replace(
        "\"device\": \"synth\", \"param\"",
        "\"device\": \"ghost\", \"param\"",
    );
    let back = load_project(&text, &reg).unwrap();
    assert_eq!(
        clip(&Document::new(back), 1).param_lanes[0].device,
        key("ghost")
    );
}
