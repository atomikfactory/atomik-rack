//! Pattern lanes: the automation lane of a pattern device's `pattern` param, labelled "Pattern",
//! held as stepped pattern numbers, and the pieces recording uses (downbeat, stepped clips).

mod common;

use atomik_core::MeterTable;
use atomik_model::automation_rec::{
    PassRecorder, Segment, WriteMode, downbeat_at_or_after, segment_clips_with,
};
use atomik_model::song::{automation_value_at, flatten_automation};
use atomik_model::{AutomationPoint, ClipContent, Curve, Document, PATTERN_PARAM, SongCommand};
use common::song::*;
use common::*;

fn doc(reg: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(empty_doc_with_seq(reg));
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("seq"),
            param: PATTERN_PARAM.into(),
        }),
        reg,
    )
    .unwrap();
    d
}

fn empty_doc_with_seq(reg: &atomik_device_sdk::Registry) -> atomik_model::RackDoc {
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(add(0, SEQ, Some("seq")), reg).unwrap();
    d.doc().clone()
}

#[test]
fn a_pattern_lane_is_labelled_and_found() {
    let reg = registry();
    let d = doc(&reg);
    let t = d.doc().song.track(&key("seq")).unwrap();
    let lane = t.pattern_lane().expect("the lane exists");
    assert_eq!(t.lane_label(lane), "Pattern");
    assert_eq!(t.automation_lane("pattern"), Some(lane));
}

#[test]
fn a_pattern_clip_switches_at_its_start_and_the_static_value_takes_over_at_its_end() {
    let reg = registry();
    let mut d = doc(&reg);
    let step = |tick: i64, value: f32| point(tick, value, Curve::Step);
    d.apply(
        add_clip("seq", 1, auto_clip(2 * BAR, 2 * BAR, vec![step(0, 3.0)])),
        &reg,
    )
    .unwrap();
    d.apply(
        add_clip(
            "seq",
            1,
            auto_clip(6 * BAR, 2 * BAR, vec![step(0, 5.0), step(BAR, 2.0)]),
        ),
        &reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::SetLaneStatic {
            device: key("seq"),
            param: PATTERN_PARAM.into(),
            value: Some(1.0),
        }),
        &reg,
    )
    .unwrap();
    let t = d.doc().song.track(&key("seq")).unwrap();
    let flat = flatten_automation(t, 1);
    assert!(flat.windows(2).all(|w| w[0].0 < w[1].0), "{flat:?}");
    assert!(flat.iter().all(|p| p.2 == Curve::Step), "{flat:?}");
    let at = |tick: i64| automation_value_at(&flat, tick as f64);
    assert_eq!(at(BAR), 1.0, "before the first clip");
    assert_eq!(at(2 * BAR), 3.0, "the first clip switches at its start");
    assert_eq!(at(4 * BAR - 1), 3.0);
    assert_eq!(at(4 * BAR), 1.0, "the static value at its end");
    assert_eq!(at(6 * BAR), 5.0);
    assert_eq!(at(7 * BAR), 2.0, "a second step inside the clip");
    assert_eq!(at(8 * BAR), 1.0);
}

#[test]
fn a_recorded_stretch_can_be_stepped_so_no_pattern_between_two_is_played() {
    let seg = Segment {
        start: 0,
        end: 4 * BAR,
        points: vec![(0, 1.0), (BAR, 4.0), (2 * BAR, 2.0)],
    };
    let clips = segment_clips_with(&[seg], 0.5, Curve::Step);
    let (start, length, points) = &clips[0];
    assert_eq!((*start, *length), (0, 4 * BAR));
    let want: Vec<AutomationPoint> = vec![
        point(0, 1.0, Curve::Step),
        point(BAR, 4.0, Curve::Step),
        point(2 * BAR, 2.0, Curve::Step),
    ];
    assert_eq!(points, &want, "thinning keeps every stepped point");
}

#[test]
fn a_latch_pass_writes_from_the_first_change_to_the_end() {
    let mut r = PassRecorder::new(WriteMode::Latch, 0, 1.0);
    r.change(BAR, 3.0, false);
    r.change(3 * BAR, 2.0, false);
    let segs = r.finish(8 * BAR);
    assert_eq!(segs.len(), 1);
    assert_eq!((segs[0].start, segs[0].end), (BAR, 8 * BAR));
    assert_eq!(segs[0].points, vec![(BAR, 3.0), (3 * BAR, 2.0)]);
}

#[test]
fn a_change_takes_effect_on_the_next_downbeat() {
    let table = MeterTable::constant(4, 4);
    let map = table.map();
    assert_eq!(downbeat_at_or_after(map, 0), 0);
    assert_eq!(downbeat_at_or_after(map, 1), BAR);
    assert_eq!(downbeat_at_or_after(map, BAR), BAR, "on the line already");
    assert_eq!(downbeat_at_or_after(map, BAR + 1), 2 * BAR);
    assert_eq!(downbeat_at_or_after(map, -50), 0);
}

#[test]
fn a_pattern_clip_survives_undo_and_a_project_round_trip() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    d.apply(
        add_clip(
            "seq",
            1,
            auto_clip(0, BAR, vec![point(0, 4.0, Curve::Step)]),
        ),
        &reg,
    )
    .unwrap();
    let text = atomik_model::save_project(d.doc());
    let back = atomik_model::load_project(&text, &reg).unwrap();
    let t = back.song.track(&key("seq")).unwrap();
    assert!(matches!(
        &t.lanes[1].clips[0].content,
        ClipContent::Automation(p) if p == &vec![point(0, 4.0, Curve::Step)]
    ));
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
}
