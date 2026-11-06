//! The level of a VCA group as an automation lane: the lane lives on the master's automation
//! track under the key `vca.<id>`, holds dB values over the range of the group's level, goes with
//! the group when it is deleted, and shows on the group's fader.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    ClipContent, Command, ConsoleCommand, Curve, Document, LaneKind, SongCommand, VCA_LANE_PARAM,
    load_project, save_project, vca_lane_id, vca_lane_key,
};
use common::song::*;
use common::*;

fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut doc = legacy_project_doc();
    doc.output_channels = 8;
    let mut d = Document::new(doc);
    for (i, k) in ["a", "b"].into_iter().enumerate() {
        d.apply(add(i, SYNTH, Some(k)), &r).unwrap();
    }
    for k in ["a", "b"] {
        d.apply(add_track(k), &r).unwrap();
    }
    d.apply(
        Command::Console(ConsoleCommand::CreateVca {
            name: "Drums".into(),
            members: vec![key("channel"), key("channel-2")],
        }),
        &r,
    )
    .unwrap();
    (d, r)
}

fn master(d: &Document) -> atomik_model::DeviceKey {
    d.doc().console.master.clone().unwrap()
}

fn add_lane(d: &mut Document, r: &Registry, id: u32) -> Result<(), atomik_model::ModelError> {
    let device = master(d);
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device,
            param: vca_lane_key(id),
        }),
        r,
    )
    .map(|_| ())
}

fn with_points(d: &mut Document, r: &Registry, id: u32, points: &[(i64, f32)]) {
    let device = master(d);
    let lane = d
        .doc()
        .song
        .track(&device)
        .unwrap()
        .automation_lane(&vca_lane_key(id))
        .unwrap();
    d.apply(
        add_clip(
            &device.0,
            lane,
            auto_clip(
                0,
                15360 * 4,
                points
                    .iter()
                    .map(|(t, v)| point(*t, *v, Curve::Linear))
                    .collect(),
            ),
        ),
        r,
    )
    .unwrap();
}

#[test]
fn the_key_names_the_group() {
    assert_eq!(vca_lane_key(3), "vca.3");
    assert_eq!(vca_lane_id("vca.3"), Some(3));
    for bad in [
        "vca.", "vca.0", "vca.03", "vca.-1", "vca.x", "fader", "vca.3 ",
    ] {
        assert_eq!(vca_lane_id(bad), None, "{bad}");
    }
    assert_eq!((VCA_LANE_PARAM.min, VCA_LANE_PARAM.max), (-60.0, 12.0));
}

#[test]
fn a_lane_needs_the_master_and_an_existing_group() {
    let (mut d, r) = project();
    assert!(add_lane(&mut d, &r, 2).is_err(), "no group 2");
    let other = d
        .apply(
            sc(SongCommand::AddAutomationLane {
                device: key("a"),
                param: vca_lane_key(1),
            }),
            &r,
        )
        .is_err();
    assert!(other, "a synth has no group lane");
    add_lane(&mut d, &r, 1).unwrap();
    assert!(add_lane(&mut d, &r, 1).is_err(), "one lane per group");
    let m = master(&d);
    let track = d.doc().song.track(&m).unwrap();
    assert!(track.is_automation_only());
    assert_eq!(track.lane_label(0), "Group 1 level");
    d.doc().validate().unwrap();
}

#[test]
fn points_are_clamped_to_the_level_range() {
    let (mut d, r) = project();
    add_lane(&mut d, &r, 1).unwrap();
    with_points(&mut d, &r, 1, &[(0, 40.0), (960, -90.0), (1920, -6.0)]);
    let m = master(&d);
    let lane = &d.doc().song.track(&m).unwrap().lanes[0];
    let ClipContent::Automation(points) = &lane.clips[0].content else {
        panic!("automation");
    };
    let values: Vec<f32> = points.iter().map(|p| p.value).collect();
    assert_eq!(values, vec![12.0, -60.0, -6.0]);
}

#[test]
fn the_lane_gives_the_level_and_shows_on_the_fader() {
    let (mut d, r) = project();
    add_lane(&mut d, &r, 1).unwrap();
    with_points(&mut d, &r, 1, &[(0, 0.0), (15360, -12.0)]);
    let m = master(&d);
    let shown = d.doc().automation_values_at(7680.0);
    let v = shown[&(m.clone(), vca_lane_key(1))];
    assert!((v + 6.0).abs() < 1e-4, "{v}");
    let view = d.doc().view_with_shown(&shown);
    assert!((view.console.vca(1).unwrap().level_db + 6.0).abs() < 1e-4);
    // The stored level is untouched.
    assert_eq!(d.doc().console.vca(1).unwrap().level_db, 0.0);
    assert_eq!(
        d.doc().vca_lane_targets(1).len(),
        2,
        "the lane drives both members"
    );
    assert!(d.doc().vca_lane_targets(9).is_empty());
}

#[test]
fn deleting_the_group_deletes_its_lane_in_one_undo_step() {
    let (mut d, r) = project();
    add_lane(&mut d, &r, 1).unwrap();
    with_points(&mut d, &r, 1, &[(0, 0.0), (960, -3.0)]);
    let before = d.doc().clone();
    d.apply(Command::Console(ConsoleCommand::DeleteVca { vca: 1 }), &r)
        .unwrap();
    let m = master(&d);
    assert!(d.doc().song.track(&m).is_none(), "the lane was the track");
    assert!(d.doc().console.vca(1).is_none());
    d.doc().validate().unwrap();
    // A new group takes the id again and starts without a lane.
    d.apply(
        Command::Console(ConsoleCommand::CreateVca {
            name: "Again".into(),
            members: vec![key("channel")],
        }),
        &r,
    )
    .unwrap();
    assert_eq!(d.doc().console.vcas[0].id, 1);
    assert!(d.doc().vca_lane_of(1).is_none());
    assert!(d.undo().is_some());
    assert!(d.undo().is_some());
    assert_eq!(d.doc(), &before);
    assert!(d.redo().is_some());
}

#[test]
fn deleting_the_group_keeps_the_other_lanes_of_the_master() {
    let (mut d, r) = project();
    let m = master(&d);
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: m.clone(),
            param: "fader".into(),
        }),
        &r,
    )
    .unwrap();
    add_lane(&mut d, &r, 1).unwrap();
    d.apply(Command::Console(ConsoleCommand::DeleteVca { vca: 1 }), &r)
        .unwrap();
    let t = d.doc().song.track(&m).unwrap();
    assert_eq!(t.lanes.len(), 1);
    assert!(matches!(&t.lanes[0].kind, LaneKind::Automation(p) if p == "fader"));
}

#[test]
fn the_lane_is_saved_and_a_lane_of_a_missing_group_is_refused() {
    let (mut d, r) = project();
    add_lane(&mut d, &r, 1).unwrap();
    with_points(&mut d, &r, 1, &[(0, -2.0), (960, 3.0)]);
    let text = save_project(d.doc());
    assert!(text.contains("\"vca.1\""), "{text}");
    let loaded = load_project(&text, &r).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text);
    let orphan = text.replace("\"vca.1\"", "\"vca.5\"");
    assert!(load_project(&orphan, &r).is_err());
}

#[test]
fn a_muted_lane_and_a_static_value_work_like_any_lane() {
    let (mut d, r) = project();
    add_lane(&mut d, &r, 1).unwrap();
    let m = master(&d);
    d.apply(
        sc(SongCommand::SetLaneStatic {
            device: m.clone(),
            param: vca_lane_key(1),
            value: Some(99.0),
            merge: None,
        }),
        &r,
    )
    .unwrap();
    let lane = &d.doc().song.track(&m).unwrap().lanes[0];
    assert_eq!(lane.static_value, Some(12.0));
    d.apply(
        sc(SongCommand::SetLaneMuted {
            device: m,
            lane: 0,
            muted: true,
        }),
        &r,
    )
    .unwrap();
}
