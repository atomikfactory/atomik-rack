//! Automation lanes: removing a lane, muting a lane (it holds its value), the static value and
//! project format 36.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::song::flatten_automation;
use atomik_model::{
    Command, ConsoleCommand, Curve, Document, LaneKind, PROJECT_VERSION, SongCommand, load_project,
    save_project,
};
use common::song::*;
use common::*;
use serde_json::Value;

const CUTOFF: &str = "filter.cutoff";
const FADER: &str = "fader";

fn add_lane(device: &str, param: &str) -> Command {
    sc(SongCommand::AddAutomationLane {
        device: key(device),
        param: param.into(),
    })
}

fn remove_lane(device: &str, param: &str) -> Command {
    sc(SongCommand::RemoveAutomationLane {
        device: key(device),
        param: param.into(),
    })
}

fn set_static(device: &str, param: &str, value: Option<f32>) -> Command {
    sc(SongCommand::SetLaneStatic {
        device: key(device),
        param: param.into(),
        value,
    })
}

/// Track `lead` (a note track) with a cutoff lane (lane 1) holding one clip.
fn lead(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("lead"), reg).unwrap();
    d.apply(add_lane("lead", CUTOFF), reg).unwrap();
    d.apply(
        add_clip(
            "lead",
            1,
            auto_clip(
                4 * BAR,
                2 * BAR,
                vec![
                    point(0, 500.0, Curve::Linear),
                    point(BAR, 900.0, Curve::Linear),
                ],
            ),
        ),
        reg,
    )
    .unwrap();
    d.end_merge();
    d
}

fn kinds(d: &Document, device: &str) -> Vec<LaneKind> {
    d.doc()
        .song
        .track(&key(device))
        .map(|t| t.lanes.iter().map(|l| l.kind.clone()).collect())
        .unwrap_or_default()
}

// ---- RemoveAutomationLane ---------------------------------------------------------------

#[test]
fn removing_a_lane_takes_its_clips_with_it_and_one_undo_brings_them_back() {
    let reg = registry();
    let mut d = lead(&reg);
    let before = d.doc().clone();
    d.apply(remove_lane("lead", CUTOFF), &reg).unwrap();
    assert_eq!(kinds(&d, "lead"), [LaneKind::Notes]);
    assert!(
        d.doc()
            .song
            .clip(before.song.tracks[0].lanes[1].clips[0].id)
            .is_none()
    );
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&reg).unwrap();
    assert_eq!(kinds(&d, "lead"), [LaneKind::Notes]);
}

#[test]
fn removing_a_lane_drops_its_write_mode_and_fold_and_undo_restores_them() {
    let reg = registry();
    let mut d = lead(&reg);
    d.apply(
        sc(SongCommand::SetLaneWriteMode {
            device: key("lead"),
            param: CUTOFF.into(),
            mode: atomik_model::automation_rec::WriteMode::Latch,
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::SetLaneFolded {
            device: key("lead"),
            param: CUTOFF.into(),
            folded: true,
        }),
        &reg,
    )
    .unwrap();
    let before = d.doc().clone();
    d.apply(remove_lane("lead", CUTOFF), &reg).unwrap();
    let song = &d.doc().song;
    assert!(
        song.lane_write.is_empty(),
        "the write mode goes with the lane"
    );
    assert!(
        song.org.folded_lanes.is_empty(),
        "the fold goes with the lane"
    );
    assert!(d.doc().validate().is_ok());
    // A lane added again starts from the defaults.
    d.apply(add_lane("lead", CUTOFF), &reg).unwrap();
    assert_eq!(
        d.doc().song.write_mode(&key("lead"), CUTOFF),
        atomik_model::automation_rec::WriteMode::Touch
    );
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn removing_the_last_lane_of_an_automation_track_removes_the_track() {
    let reg = console_registry();
    let mut d = Document::new(chain(&registry()).doc().clone());
    d.apply(
        Command::Console(ConsoleCommand::AddChannel {
            source: key("delay"),
        }),
        &reg,
    )
    .unwrap();
    d.apply(add_lane("channel", FADER), &reg).unwrap();
    d.apply(add_lane("channel", "pan"), &reg).unwrap();
    let before = d.doc().clone();
    d.apply(remove_lane("channel", FADER), &reg).unwrap();
    assert_eq!(kinds(&d, "channel"), [LaneKind::Automation("pan".into())]);
    d.apply(remove_lane("channel", "pan"), &reg).unwrap();
    assert!(d.doc().song.track(&key("channel")).is_none());
    assert!(d.doc().validate().is_ok());
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn removing_a_lane_that_is_not_there_is_refused_and_changes_nothing() {
    let reg = registry();
    let mut d = lead(&reg);
    let before = d.doc().clone();
    d.apply(remove_lane("lead", "filter.resonance"), &reg)
        .expect_err("no such lane");
    d.apply(remove_lane("nope", CUTOFF), &reg)
        .expect_err("no such track");
    assert_eq!(d.doc(), &before);
}

// ---- a muted lane -----------------------------------------------------------------------

fn mute(lane: usize, muted: bool) -> Command {
    sc(SongCommand::SetLaneMuted {
        device: key("lead"),
        lane,
        muted,
    })
}

#[test]
fn a_muted_automation_lane_flattens_to_nothing_and_unmuting_brings_it_back() {
    let reg = registry();
    let mut d = lead(&reg);
    let playing = flatten_automation(d.doc().song.track(&key("lead")).unwrap(), 1);
    assert!(!playing.is_empty());
    d.apply(mute(1, true), &reg).unwrap();
    let track = d.doc().song.track(&key("lead")).unwrap();
    assert!(track.lanes[1].muted);
    assert!(flatten_automation(track, 1).is_empty());
    d.apply(mute(1, true), &reg).expect_err("already muted");
    d.apply(mute(1, false), &reg).unwrap();
    let track = d.doc().song.track(&key("lead")).unwrap();
    assert_eq!(flatten_automation(track, 1), playing);
    // The audio lane of an audio track and a lane that does not exist take no mute.
    d.apply(mute(7, true), &reg).expect_err("no such lane");
}

#[test]
fn muting_with_the_held_value_is_one_undo_step() {
    let reg = registry();
    let mut d = lead(&reg);
    let before = d.doc().clone();
    d.apply(
        Command::Batch(vec![
            mute(1, true),
            Command::SetParam {
                key: key("lead"),
                param: CUTOFF.into(),
                value: 640.0,
                merge: None,
            },
        ]),
        &reg,
    )
    .unwrap();
    let doc = d.doc();
    assert!(doc.song.tracks[0].lanes[1].muted);
    assert_eq!(doc.device(&key("lead")).unwrap().params[CUTOFF], 640.0);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

// ---- the static value -------------------------------------------------------------------

#[test]
fn a_static_value_holds_before_between_and_after_the_clips() {
    let reg = registry();
    let mut d = lead(&reg);
    d.apply(
        add_clip(
            "lead",
            1,
            auto_clip(8 * BAR, BAR, vec![point(0, 700.0, Curve::Linear)]),
        ),
        &reg,
    )
    .unwrap();
    d.apply(set_static("lead", CUTOFF, Some(300.0)), &reg)
        .unwrap();
    let track = d.doc().song.track(&key("lead")).unwrap();
    let flat = flatten_automation(track, 1);
    // Strictly increasing ticks, a leading point at 0, the static value after each clip.
    assert!(flat.windows(2).all(|w| w[0].0 < w[1].0), "{flat:?}");
    assert_eq!(flat[0], (0, 300.0, Curve::Step));
    let value = |tick: i64| atomik_model::song::automation_value_at(&flat, tick as f64);
    assert_eq!(value(BAR), 300.0, "before the first clip");
    assert_eq!(value(4 * BAR), 500.0, "the clip plays");
    assert_eq!(value(6 * BAR), 300.0, "between the clips");
    assert_eq!(value(8 * BAR + 10), 700.0);
    assert_eq!(value(20 * BAR), 300.0, "after the last clip");
}

#[test]
fn a_lane_with_a_static_value_and_no_clips_holds_it_for_the_whole_song() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    d.apply(add_lane("lead", CUTOFF), &reg).unwrap();
    d.apply(set_static("lead", CUTOFF, Some(1234.0)), &reg)
        .unwrap();
    let track = d.doc().song.track(&key("lead")).unwrap();
    assert_eq!(flatten_automation(track, 1), [(0, 1234.0, Curve::Step)]);
    d.apply(set_static("lead", CUTOFF, None), &reg).unwrap();
    let track = d.doc().song.track(&key("lead")).unwrap();
    assert!(flatten_automation(track, 1).is_empty());
}

#[test]
fn the_static_value_is_clamped_checked_and_undoable() {
    let reg = registry();
    let mut d = lead(&reg);
    let before = d.doc().clone();
    d.apply(set_static("lead", CUTOFF, Some(1.0e9)), &reg)
        .unwrap();
    let v = d.doc().song.tracks[0].lanes[1].static_value.unwrap();
    assert!(v < 1.0e9, "clamped into the param's range, got {v}");
    d.apply(set_static("lead", CUTOFF, Some(v)), &reg)
        .expect_err("the lane has that value already");
    d.apply(set_static("lead", CUTOFF, Some(f32::NAN)), &reg)
        .expect_err("not a number");
    d.apply(set_static("lead", "filter.resonance", Some(0.5)), &reg)
        .expect_err("no such lane");
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

// ---- project format 36 ------------------------------------------------------------------

fn at_version(text: &str, n: u32) -> String {
    text.replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        &format!("\"version\": {n}"),
        1,
    )
}

#[test]
fn the_project_version_is_thirty_six_and_a_plain_lane_writes_neither_field() {
    let reg = registry();
    assert_eq!(PROJECT_VERSION, 38);
    let text = save_project(lead(&reg).doc());
    assert!(!text.contains("\"static\""));
    assert!(!text.contains("\"muted\": true"));
    // A file without the new fields differs from its version 35 form in the version line alone.
    let old = at_version(&text, 35);
    let loaded = load_project(&old, &reg).unwrap();
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_muted_and_static_lane_round_trips_byte_for_byte() {
    let reg = registry();
    let mut d = lead(&reg);
    d.apply(mute(1, true), &reg).unwrap();
    d.apply(set_static("lead", CUTOFF, Some(250.0)), &reg)
        .unwrap();
    let text = save_project(d.doc());
    assert!(
        text.contains("\"param\": \"filter.cutoff\", \"muted\": true, \"static\": 250"),
        "{text}"
    );
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded.song, d.doc().song);
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_version_35_file_refuses_the_new_lane_fields_and_36_refuses_bad_values() {
    let reg = registry();
    let mut d = lead(&reg);
    d.apply(mute(1, true), &reg).unwrap();
    d.apply(set_static("lead", CUTOFF, Some(250.0)), &reg)
        .unwrap();
    let text = save_project(d.doc());
    let err = load_project(&at_version(&text, 35), &reg).expect_err("unknown field in 35");
    assert!(err.to_string().contains("muted") || err.to_string().contains("static"));
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut v: Value = serde_json::from_str(&text).unwrap();
        let lane = &mut v["song"]["tracks"][0]["lanes"][1];
        f(lane);
        serde_json::to_string_pretty(&v).unwrap()
    };
    let false_mute = edit(&|l| l["muted"] = Value::Bool(false));
    assert!(load_project(&false_mute, &reg).is_err());
    let text_static = edit(&|l| l["static"] = Value::String("x".into()));
    assert!(load_project(&text_static, &reg).is_err());
    // A note lane keeps its own muted field; an audio lane never had one.
    let ok = edit(&|_| {});
    assert!(load_project(&ok, &reg).is_ok());
}
