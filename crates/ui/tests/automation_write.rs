//! The automation lane header: the arm button, the write-mode button and the record indicator.

mod support;

use atomik_model::{SongCommand, WriteMode};
use atomik_ui::kurbo::Point;
use atomik_ui::{DisplayList, Key, LaneButton, Modifiers, UiAction, UiEvent};
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
        doc.song_mut()
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
    doc.song_mut()
        .lane_write
        .insert((key("bass"), CUTOFF.into()), WriteMode::Overwrite);
    v.sync(&doc, &reg);
    let t = texts(&mut v, &doc);
    assert!(t.iter().any(|s| s == "OVR") && !t.iter().any(|s| s == "TCH"));
    doc.song_mut()
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

// ---------------------------------------------------------------- delete, mute, pick

#[test]
fn the_delete_button_removes_the_lane_and_the_mute_button_mutes_it() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let x = v
        .lane_button_center(&key("bass"), CUTOFF, LaneButton::Delete)
        .unwrap();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(x), up(x)]);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::RemoveAutomationLane {
            device: key("bass"),
            param: CUTOFF.into(),
        }]
    );
    let lane = doc
        .song
        .track(&key("bass"))
        .unwrap()
        .automation_lane(CUTOFF);
    let m = v
        .lane_button_center(&key("bass"), CUTOFF, LaneButton::Mute)
        .unwrap();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(m), up(m)]);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::SetLaneMuted {
            device: key("bass"),
            lane: lane.unwrap(),
            muted: true,
        }]
    );
}

#[test]
fn the_buttons_do_not_overlap_and_stay_inside_the_header() {
    let (doc, reg) = (song_doc(), registry());
    let v = arrangement(&doc, &reg);
    let xs: Vec<f64> = [
        LaneButton::Mode,
        LaneButton::Arm,
        LaneButton::Mute,
        LaneButton::Delete,
    ]
    .into_iter()
    .map(|b| v.lane_button_center(&key("bass"), CUTOFF, b).unwrap().x)
    .collect();
    assert!(xs.windows(2).all(|w| w[1] - w[0] >= 18.0), "{xs:?}");
    assert!(*xs.last().unwrap() < atomik_ui::arrangement::TRACK_LIST_WIDTH);
}

#[test]
fn a_press_on_a_lane_button_picks_the_lane_the_menu_items_act_on() {
    let (mut doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    assert_eq!(v.picked_automation_lane(), None);
    assert_eq!(v.automation_lane_menu(), (false, false, false));
    let p = v
        .lane_button_center(&key("bass"), CUTOFF, LaneButton::Arm)
        .unwrap();
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert_eq!(
        v.picked_automation_lane(),
        Some(&(key("bass"), CUTOFF.to_string()))
    );
    assert_eq!(v.automation_lane_menu(), (true, false, false));
    let t = doc
        .song_mut()
        .tracks
        .iter_mut()
        .find(|t| t.device == key("bass"))
        .unwrap();
    let i = t.automation_lane(CUTOFF).unwrap();
    t.lanes[i].muted = true;
    t.lanes[i].static_value = Some(1000.0);
    v.sync(&doc, &reg);
    assert_eq!(v.automation_lane_menu(), (true, true, true));
    // The pick goes with the lane.
    let t = doc
        .song_mut()
        .tracks
        .iter_mut()
        .find(|t| t.device == key("bass"))
        .unwrap();
    t.lanes.remove(i);
    v.sync(&doc, &reg);
    assert_eq!(v.picked_automation_lane(), None);
}

#[test]
fn a_muted_lane_lights_its_mute_button() {
    let (mut doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    let paint = |v: &mut atomik_ui::ArrangementView, doc: &atomik_model::RackDoc| {
        let mut dl = DisplayList::new();
        v.paint(&mut dl, doc, &registry());
        dl.to_json()
    };
    let idle = paint(&mut v, &doc);
    let t = doc
        .song_mut()
        .tracks
        .iter_mut()
        .find(|t| t.device == key("bass"))
        .unwrap();
    let i = t.automation_lane(CUTOFF).unwrap();
    t.lanes[i].muted = true;
    v.sync(&doc, &reg);
    assert_ne!(idle, paint(&mut v, &doc));
}

// ------------------------------------------------------------------ the static value handle

fn static_doc(value: Option<f32>) -> atomik_model::RackDoc {
    let mut doc = song_doc();
    let t = doc
        .song_mut()
        .tracks
        .iter_mut()
        .find(|t| t.device == key("bass"))
        .unwrap();
    let i = t.automation_lane(CUTOFF).unwrap();
    t.lanes[i].static_value = value;
    doc
}

fn statics(actions: &[UiAction]) -> Vec<(f32, Option<u64>)> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetLaneStatic { value, merge, .. } => Some((value?, merge)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_lane_without_a_static_value_has_no_handle() {
    let (doc, reg) = (static_doc(None), registry());
    let v = arrangement(&doc, &reg);
    assert!(v.static_handle_center(&key("bass"), CUTOFF).is_none());
}

#[test]
fn dragging_the_static_value_handle_sets_the_value_from_the_height_in_one_merge_group() {
    let (doc, reg) = (static_doc(Some(1000.0)), registry());
    let mut v = arrangement(&doc, &reg);
    let h = v.static_handle_center(&key("bass"), CUTOFF).unwrap();
    let up_to = Point::new(h.x, h.y - 12.0);
    let higher = Point::new(h.x, h.y - 20.0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(h), mv(up_to), mv(higher), up(higher)],
    );
    let values = statics(&a);
    assert!(values.len() >= 2, "a command per new value: {values:?}");
    assert!(
        values.windows(2).all(|w| w[0].0 < w[1].0),
        "rises: {values:?}"
    );
    assert!(values[0].0 > 1000.0);
    let merge = values[0].1.expect("a merge id");
    assert!(values.iter().all(|(_, m)| *m == Some(merge)), "one group");
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    // Nothing else changed: no clip was selected or drawn.
    assert_eq!(song_commands(&a).len(), values.len());
}

#[test]
fn a_click_on_the_handle_changes_nothing_and_escape_puts_the_value_back() {
    let (doc, reg) = (static_doc(Some(1000.0)), registry());
    let mut v = arrangement(&doc, &reg);
    let h = v.static_handle_center(&key("bass"), CUTOFF).unwrap();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(h), up(h)]);
    assert!(a.is_empty(), "{a:?}");
    let away = Point::new(h.x, h.y - 20.0);
    let _ = run(|e| v.handle(e, &doc, &reg), &[down(h), mv(away)]);
    let escape = UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    let a = v.handle(&escape, &doc, &reg);
    let values = statics(&a);
    assert_eq!(values.last().map(|v| v.0), Some(1000.0), "{a:?}");
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}
