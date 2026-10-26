//! Automation recording, model side: the write modes, the pass recorder, thinning, the lane
//! write-mode command and project format 18.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::automation_rec::{PassRecorder, Segment, WriteMode, segment_clips, thin_points};
use atomik_model::{
    AutomationPoint, ClipContent, Command, Curve, Document, PROJECT_VERSION, SongCommand, SyncOp,
    load_project, save_project,
};
use common::song::*;
use common::*;
use serde_json::Value;

const CUTOFF: &str = "filter.cutoff";

fn p(tick: i64, value: f32) -> AutomationPoint {
    point(tick, value, Curve::Linear)
}

fn doc(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("lead"), reg).unwrap();
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("lead"),
            param: CUTOFF.into(),
        }),
        reg,
    )
    .unwrap();
    d
}

fn set_mode(mode: WriteMode) -> Command {
    sc(SongCommand::SetLaneWriteMode {
        device: key("lead"),
        param: CUTOFF.into(),
        mode,
    })
}

fn mode_of(d: &Document) -> WriteMode {
    d.doc().song.write_mode(&key("lead"), CUTOFF)
}

// ---- WriteMode ------------------------------------------------------------------------

#[test]
fn write_mode_names_round_trip_and_cycle_through_all_three() {
    for m in WriteMode::ALL {
        assert_eq!(WriteMode::from_name(m.name()), Some(m));
    }
    assert_eq!(WriteMode::from_name("Touch"), None);
    assert_eq!(WriteMode::default(), WriteMode::Touch);
    let mut m = WriteMode::Overwrite;
    let seen: Vec<_> = (0..4)
        .map(|_| {
            let now = m;
            m = m.next();
            now
        })
        .collect();
    assert_eq!(
        seen,
        [
            WriteMode::Overwrite,
            WriteMode::Touch,
            WriteMode::Latch,
            WriteMode::Overwrite
        ]
    );
}

// ---- PassRecorder ---------------------------------------------------------------------

#[test]
fn overwrite_writes_the_whole_pass_from_the_value_at_its_start() {
    let mut r = PassRecorder::new(WriteMode::Overwrite, 100, 0.25);
    r.change(300, 0.5, true);
    r.change(500, 0.75, false);
    let segs = r.finish(900);
    assert_eq!(
        segs,
        vec![Segment {
            start: 100,
            end: 900,
            points: vec![(100, 0.25), (300, 0.5), (500, 0.75)],
        }]
    );
}

#[test]
fn overwrite_without_a_move_holds_the_start_value_over_the_pass() {
    let segs = PassRecorder::new(WriteMode::Overwrite, 0, 0.4).finish(1000);
    assert_eq!(
        segs,
        vec![Segment {
            start: 0,
            end: 1000,
            points: vec![(0, 0.4)]
        }]
    );
}

#[test]
fn touch_writes_only_while_held_and_each_touch_is_its_own_stretch() {
    let mut r = PassRecorder::new(WriteMode::Touch, 0, 0.0);
    assert!(!r.is_writing());
    r.change(100, 0.1, true);
    r.change(200, 0.2, true);
    assert!(r.is_writing());
    r.release(250);
    assert!(!r.is_writing());
    r.change(600, 0.9, true);
    let segs = r.finish(1000);
    assert_eq!(
        segs,
        vec![
            Segment {
                start: 100,
                end: 250,
                points: vec![(100, 0.1), (200, 0.2)]
            },
            Segment {
                start: 600,
                end: 1000,
                points: vec![(600, 0.9)]
            },
        ]
    );
}

#[test]
fn touch_with_no_touch_writes_nothing() {
    assert!(
        PassRecorder::new(WriteMode::Touch, 0, 0.5)
            .finish(500)
            .is_empty()
    );
}

#[test]
fn touch_treats_a_move_that_is_not_held_as_an_instant_touch() {
    let mut r = PassRecorder::new(WriteMode::Touch, 0, 0.0);
    r.change(400, 0.6, false);
    assert!(!r.is_writing());
    let segs = r.finish(800);
    assert_eq!(
        segs,
        vec![Segment {
            start: 400,
            end: 401,
            points: vec![(400, 0.6)]
        }]
    );
}

#[test]
fn latch_starts_at_the_first_touch_keeps_writing_after_the_release_and_runs_to_the_end() {
    let mut r = PassRecorder::new(WriteMode::Latch, 0, 0.0);
    assert!(!r.is_writing());
    r.change(300, 0.3, true);
    r.release(350);
    assert!(r.is_writing(), "a release does not end a latch");
    let segs = r.finish(1000);
    assert_eq!(
        segs,
        vec![Segment {
            start: 300,
            end: 1000,
            points: vec![(300, 0.3)]
        }]
    );
}

#[test]
fn latch_with_no_touch_writes_nothing() {
    assert!(
        PassRecorder::new(WriteMode::Latch, 0, 0.5)
            .finish(500)
            .is_empty()
    );
}

#[test]
fn the_last_move_in_a_tick_wins_and_an_earlier_tick_reads_as_the_latest() {
    let mut r = PassRecorder::new(WriteMode::Latch, 0, 0.0);
    r.change(100, 0.1, true);
    r.change(100, 0.2, true);
    r.change(50, 0.3, true);
    let segs = r.finish(200);
    assert_eq!(segs[0].points, vec![(100, 0.3)]);
}

#[test]
fn a_stretch_always_ends_after_its_last_point() {
    let mut r = PassRecorder::new(WriteMode::Latch, 0, 0.0);
    r.change(500, 0.5, true);
    let segs = r.finish(500);
    assert_eq!((segs[0].start, segs[0].end), (500, 501));
}

// ---- segment_clips and thin_points ----------------------------------------------------

#[test]
fn segment_clips_make_relative_linear_points_and_thin_them() {
    let seg = Segment {
        start: 1000,
        end: 1400,
        points: vec![(1000, 0.0), (1100, 0.25), (1200, 0.5), (1300, 0.75)],
    };
    let clips = segment_clips(&[seg], 0.01);
    assert_eq!(clips.len(), 1);
    let (start, length, points) = &clips[0];
    assert_eq!((*start, *length), (1000, 400));
    assert_eq!(points, &vec![p(0, 0.0), p(300, 0.75)]);
}

#[test]
fn thin_points_keeps_a_ramp_as_its_two_ends() {
    let pts: Vec<_> = (0..=10).map(|i| p(i * 10, i as f32 / 10.0)).collect();
    assert_eq!(thin_points(&pts, 0.001), vec![p(0, 0.0), p(100, 1.0)]);
}

#[test]
fn thin_points_keeps_the_corners_of_a_shape() {
    // Up, hold, down: a trapezoid with redundant points along its slopes.
    let pts = vec![
        p(0, 0.0),
        p(10, 0.25),
        p(20, 0.5),
        p(30, 0.75),
        p(40, 1.0),
        p(50, 1.0),
        p(60, 1.0),
        p(70, 0.5),
        p(80, 0.0),
    ];
    assert_eq!(
        thin_points(&pts, 0.01),
        vec![p(0, 0.0), p(40, 1.0), p(60, 1.0), p(80, 0.0)]
    );
}

#[test]
fn thin_points_error_stays_within_the_tolerance() {
    // A sine sampled finely: whatever is dropped stays within the tolerance of the line
    // through the kept neighbours.
    let pts: Vec<_> = (0..200)
        .map(|i| p(i * 5, 0.5 + 0.4 * (i as f32 * 0.1).sin()))
        .collect();
    let tol = 0.02;
    let kept = thin_points(&pts, tol);
    assert!(kept.len() < pts.len() / 2, "{} points kept", kept.len());
    assert_eq!((kept[0], kept[kept.len() - 1]), (pts[0], pts[199]));
    for orig in &pts {
        let i = kept.partition_point(|k| k.tick <= orig.tick) - 1;
        let a = kept[i];
        let b = kept.get(i + 1).copied().unwrap_or(a);
        let t = if b.tick > a.tick {
            (orig.tick - a.tick) as f32 / (b.tick - a.tick) as f32
        } else {
            0.0
        };
        let line = a.value + (b.value - a.value) * t;
        assert!(
            (orig.value - line).abs() <= tol + 1e-5,
            "tick {}",
            orig.tick
        );
    }
}

#[test]
fn thin_points_with_no_tolerance_or_few_points_changes_nothing() {
    let pts = vec![p(0, 0.0), p(10, 0.5), p(20, 1.0)];
    assert_eq!(thin_points(&pts, 0.0), pts);
    assert_eq!(thin_points(&pts, -1.0), pts);
    assert_eq!(thin_points(&pts, f32::NAN), pts);
    assert_eq!(thin_points(&pts[..2], 1.0), pts[..2].to_vec());
    assert!(thin_points(&[], 1.0).is_empty());
}

#[test]
fn thin_points_keeps_step_points_and_thins_between_them() {
    let step = |t, v| point(t, v, Curve::Step);
    let pts = vec![
        p(0, 0.0),
        p(10, 0.1),
        p(20, 0.2),
        step(30, 0.3),
        p(40, 0.9),
        p(50, 0.9),
        p(60, 0.9),
    ];
    let kept = thin_points(&pts, 0.01);
    assert_eq!(kept, vec![p(0, 0.0), step(30, 0.3), p(40, 0.9), p(60, 0.9)]);
}

// ---- SetLaneWriteMode -----------------------------------------------------------------

#[test]
fn a_lane_defaults_to_touch_and_the_command_changes_it_undoably() {
    let reg = registry();
    let mut d = doc(&reg);
    assert_eq!(mode_of(&d), WriteMode::Touch);
    let ops = d.apply(set_mode(WriteMode::Latch), &reg).unwrap();
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    assert_eq!(mode_of(&d), WriteMode::Latch);
    d.apply(set_mode(WriteMode::Overwrite), &reg).unwrap();
    assert_eq!(mode_of(&d), WriteMode::Overwrite);
    d.undo(&reg).unwrap();
    assert_eq!(mode_of(&d), WriteMode::Latch);
    d.undo(&reg).unwrap();
    assert_eq!(mode_of(&d), WriteMode::Touch);
    assert!(d.doc().song.lane_write.is_empty());
    d.redo(&reg).unwrap();
    assert_eq!(mode_of(&d), WriteMode::Latch);
}

#[test]
fn setting_touch_stores_no_entry() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(set_mode(WriteMode::Latch), &reg).unwrap();
    d.apply(set_mode(WriteMode::Touch), &reg).unwrap();
    assert!(d.doc().song.lane_write.is_empty());
}

#[test]
fn the_command_needs_an_existing_automation_lane() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    let no_lane = sc(SongCommand::SetLaneWriteMode {
        device: key("lead"),
        param: "filter.resonance".into(),
        mode: WriteMode::Latch,
    });
    d.apply(no_lane, &reg).expect_err("no such lane");
    let no_track = sc(SongCommand::SetLaneWriteMode {
        device: key("nope"),
        param: CUTOFF.into(),
        mode: WriteMode::Latch,
    });
    d.apply(no_track, &reg).expect_err("no such track");
    assert_eq!(d.doc(), &before);
}

// ---- a take is one undo step ----------------------------------------------------------

#[test]
fn a_take_of_several_clips_in_a_batch_is_one_undo_step_and_hides_the_old_points() {
    let reg = registry();
    let mut d = doc(&reg);
    let old = auto_clip(0, BAR, vec![p(0, 200.0), p(BAR - 1, 200.0)]);
    d.apply(add_clip("lead", 1, old), &reg).unwrap();
    d.end_merge();
    let segs = vec![
        Segment {
            start: 100,
            end: 400,
            points: vec![(100, 900.0), (300, 800.0)],
        },
        Segment {
            start: 800,
            end: 1000,
            points: vec![(800, 150.0)],
        },
    ];
    let batch: Vec<Command> = segment_clips(&segs, 0.0)
        .into_iter()
        .map(|(start, length, points)| add_clip("lead", 1, auto_clip(start, length, points)))
        .collect();
    d.apply(Command::Batch(batch), &reg).unwrap();
    d.end_merge();
    let track = d.doc().song.track(&key("lead")).unwrap();
    assert_eq!(track.lanes[1].clips.len(), 3);
    // The recorded stretch wins over the old clip where they overlap.
    let flat = atomik_model::song::flatten_automation(track, 1);
    let at = |t: i64| flat.iter().find(|f| f.0 == t).map(|f| f.1);
    assert_eq!(at(100), Some(900.0));
    assert_eq!(at(800), Some(150.0));
    assert_eq!(
        at(0),
        Some(200.0),
        "the old clip still plays outside the take"
    );
    d.undo(&reg).unwrap();
    let lane = &d.doc().song.track(&key("lead")).unwrap().lanes[1];
    assert_eq!(lane.clips.len(), 1, "one undo removes the whole take");
    let ClipContent::Automation(pts) = &lane.clips[0].content else {
        panic!("automation clip");
    };
    assert_eq!(pts.len(), 2);
}

// ---- project format 18 ----------------------------------------------------------------

#[test]
fn the_project_version_is_eighteen_and_a_touch_lane_writes_no_field() {
    let reg = registry();
    let d = doc(&reg);
    let text = save_project(d.doc());
    assert_eq!(PROJECT_VERSION, 32);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 32);
    assert!(!text.contains("\"write\""));
}

#[test]
fn a_lane_write_mode_round_trips_byte_for_byte() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(set_mode(WriteMode::Latch), &reg).unwrap();
    let text = save_project(d.doc());
    assert!(text.contains("\"write\": \"latch\""), "{text}");
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(
        loaded.song.write_mode(&key("lead"), CUTOFF),
        WriteMode::Latch
    );
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_version_17_file_loads_with_every_lane_on_touch_and_resaves_as_18() {
    let reg = registry();
    let d = doc(&reg);
    let v17 = save_project(d.doc()).replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 17",
        1,
    );
    let loaded = load_project(&v17, &reg).unwrap();
    assert!(loaded.song.lane_write.is_empty());
    assert_eq!(save_project(&loaded), as_current(&v17, 17));
}

#[test]
fn write_before_version_18_is_an_unknown_field() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(set_mode(WriteMode::Overwrite), &reg).unwrap();
    let text = save_project(d.doc()).replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 17",
        1,
    );
    let s = load_project(&text, &reg).unwrap_err().to_string();
    assert!(s.contains("write") && s.contains("unknown field"), "{s}");
}

#[test]
fn a_bad_or_default_write_name_is_refused_with_its_path() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(set_mode(WriteMode::Latch), &reg).unwrap();
    let text = save_project(d.doc());
    for bad in ["touch", "ltach", ""] {
        let edited = text.replace("\"write\": \"latch\"", &format!("\"write\": \"{bad}\""));
        let s = load_project(&edited, &reg).unwrap_err().to_string();
        assert!(s.contains("write"), "{bad}: {s}");
    }
    let number = text.replace("\"write\": \"latch\"", "\"write\": 3");
    assert!(load_project(&number, &reg).is_err());
}

#[test]
fn write_on_a_note_lane_is_an_unknown_field() {
    let reg = registry();
    let d = doc(&reg);
    let text = save_project(d.doc()).replacen(
        "\"kind\": \"notes\",",
        "\"kind\": \"notes\", \"write\": \"latch\",",
        1,
    );
    let s = load_project(&text, &reg).unwrap_err().to_string();
    assert!(s.contains("write"), "{s}");
}

#[test]
fn entries_of_removed_lanes_are_not_saved() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(set_mode(WriteMode::Latch), &reg).unwrap();
    d.apply(
        sc(SongCommand::RemoveTrack {
            device: key("lead"),
        }),
        &reg,
    )
    .unwrap();
    assert!(!save_project(d.doc()).contains("latch"));
    d.undo(&reg).unwrap();
    assert!(save_project(d.doc()).contains("latch"));
}
