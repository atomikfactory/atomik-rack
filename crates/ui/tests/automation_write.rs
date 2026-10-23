//! The automation lane header: the arm button, the write-mode button and the record indicator.

mod support;

use atomik_model::{SongCommand, WriteMode};
use atomik_ui::{DisplayList, LaneButton, UiAction};
use std::collections::BTreeSet;
use support::song::*;
use support::*;

const CUTOFF: &str = "filter.cutoff";

fn texts(v: &mut atomik_ui::ArrangementView, doc: &atomik_model::RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn armed(device: &str) -> BTreeSet<(atomik_model::DeviceKey, String)> {
    BTreeSet::from([(key(device), CUTOFF.to_string())])
}

#[test]
fn the_arm_button_asks_the_app_to_toggle_the_lane() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let p = v
        .lane_button_center(&key("bass"), CUTOFF, LaneButton::Arm)
        .unwrap();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert_eq!(
        a,
        vec![UiAction::ToggleLaneArm {
            device: key("bass"),
            param: CUTOFF.into()
        }]
    );
}

#[test]
fn the_mode_button_cycles_overwrite_touch_latch() {
    let (mut doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let p = v
        .lane_button_center(&key("bass"), CUTOFF, LaneButton::Mode)
        .unwrap();
    let mut seen = Vec::new();
    for _ in 0..3 {
        let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
        let cmds = song_commands(&a);
        let [
            SongCommand::SetLaneWriteMode {
                device,
                param,
                mode,
            },
        ] = cmds.as_slice()
        else {
            panic!("one SetLaneWriteMode, got {cmds:?}");
        };
        assert_eq!((device, param.as_str()), (&key("bass"), CUTOFF));
        seen.push(*mode);
        // The app applies it; the view follows the document.
        doc.song
            .lane_write
            .insert((key("bass"), CUTOFF.into()), *mode);
        v.sync(&doc, &reg);
    }
    assert_eq!(
        seen,
        [WriteMode::Latch, WriteMode::Overwrite, WriteMode::Touch]
    );
}

#[test]
fn the_buttons_are_only_on_automation_lanes() {
    let (doc, reg) = (song_doc(), registry());
    let v = arrangement(&doc, &reg);
    assert!(
        v.lane_button_center(&key("bass"), "filter.resonance", LaneButton::Arm)
            .is_none()
    );
    assert!(
        v.lane_button_center(&key("lead"), CUTOFF, LaneButton::Arm)
            .is_none(),
        "lead has no cutoff lane"
    );
}

#[test]
fn the_header_shows_the_mode_of_the_lane() {
    let (mut doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let t = texts(&mut v, &doc);
    assert!(t.iter().any(|s| s == "TCH"), "{t:?}");
    doc.song
        .lane_write
        .insert((key("bass"), CUTOFF.into()), WriteMode::Overwrite);
    v.sync(&doc, &reg);
    let t = texts(&mut v, &doc);
    assert!(t.iter().any(|s| s == "OVR") && !t.iter().any(|s| s == "TCH"));
    doc.song
        .lane_write
        .insert((key("bass"), CUTOFF.into()), WriteMode::Latch);
    v.sync(&doc, &reg);
    assert!(texts(&mut v, &doc).iter().any(|s| s == "LTC"));
}

#[test]
fn an_armed_lane_changes_the_display_and_a_take_marks_it() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let paint = |v: &mut atomik_ui::ArrangementView| {
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &registry());
        dl.to_json()
    };
    let idle = paint(&mut v);
    v.set_armed_lanes(&armed("bass"));
    let lit = paint(&mut v);
    assert_ne!(idle, lit, "arming lights the arm button");
    v.set_recording(true);
    let rec = paint(&mut v);
    assert_ne!(lit, rec, "recording marks the armed lane");
    // A lane that is not armed looks the same with or without a take.
    v.set_armed_lanes(&BTreeSet::new());
    v.set_recording(false);
    let idle2 = paint(&mut v);
    v.set_recording(true);
    let rec2 = paint(&mut v);
    assert_eq!(idle, idle2);
    assert_ne!(idle2, rec2, "the playhead itself turns red");
}
