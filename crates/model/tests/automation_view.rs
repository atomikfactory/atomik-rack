//! The value a control shows for an automated param: the lane's value at a song position
//! ([`RackDoc::automation_values_at`]), which tracks play it, the lanes a param has
//! ([`RackDoc::automated_params`]) and the view copy with those values in place.

mod common;

use atomik_model::song::automation_value_at;
use atomik_model::{Curve, DeviceKey, Document, SongCommand};
use common::song::*;
use common::*;

const CUTOFF: &str = "filter.cutoff";

/// A synth `lead` with a track and a cutoff lane holding one clip at bar 2 (points 100 -> 600
/// linear over a beat, then 600 held).
fn doc_with_lane() -> (Document, atomik_device_sdk::Registry) {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("lead"),
            param: CUTOFF.into(),
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        add_clip(
            "lead",
            1,
            auto_clip(
                BAR,
                BAR,
                vec![
                    point(0, 100.0, Curve::Linear),
                    point(PPQ, 600.0, Curve::Step),
                ],
            ),
        ),
        &reg,
    )
    .unwrap();
    (d, reg)
}

fn lead() -> (DeviceKey, String) {
    (key("lead"), CUTOFF.to_string())
}

#[test]
fn the_value_at_a_position_is_held_before_linear_between_and_held_after() {
    let pts = [(1000, 100.0, Curve::Linear), (2000, 600.0, Curve::Step)];
    assert_eq!(automation_value_at(&pts, 0.0), 100.0);
    assert_eq!(automation_value_at(&pts, 1000.0), 100.0);
    assert_eq!(automation_value_at(&pts, 1500.0), 350.0);
    assert_eq!(automation_value_at(&pts, 2000.0), 600.0);
    assert_eq!(automation_value_at(&pts, 9000.0), 600.0);
    assert_eq!(automation_value_at(&[], 5.0), 0.0);
    let steps = [(0, 1.0, Curve::Step), (100, 2.0, Curve::Step)];
    assert_eq!(automation_value_at(&steps, 99.9), 1.0);
    assert_eq!(automation_value_at(&steps, 100.0), 2.0);
}

#[test]
fn a_lane_gives_its_value_at_the_position() {
    let (d, _) = doc_with_lane();
    let doc = d.doc();
    let at = |tick: f64| doc.automation_values_at(tick).get(&lead()).copied();
    // Before the clip the first value holds; in the clip it moves; after it holds the last.
    assert_eq!(at(0.0), Some(100.0));
    assert_eq!(at((BAR + PPQ / 2) as f64), Some(350.0));
    assert_eq!(at((BAR + PPQ) as f64), Some(600.0));
    assert_eq!(at((10 * BAR) as f64), Some(600.0));
}

#[test]
fn a_lane_without_points_and_a_muted_track_give_no_value() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("lead"),
            param: CUTOFF.into(),
        }),
        &reg,
    )
    .unwrap();
    assert!(
        d.doc().automation_values_at(0.0).is_empty(),
        "an empty lane plays nothing"
    );
    assert!(
        d.doc().automated_params().contains(&lead()),
        "but it exists"
    );

    let (mut d, reg) = doc_with_lane();
    d.apply(
        sc(SongCommand::SetTrackFlags {
            device: key("lead"),
            muted: true,
            solo: false,
            armed: false,
        }),
        &reg,
    )
    .unwrap();
    assert!(
        d.doc().automation_values_at(0.0).is_empty(),
        "a muted track plays no automation"
    );
}

#[test]
fn the_values_match_what_the_flattened_lane_plays_with_clip_offsets_and_loops() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("lead"),
            param: CUTOFF.into(),
        }),
        &reg,
    )
    .unwrap();
    let mut c = auto_clip(
        0,
        4 * PPQ,
        vec![
            point(0, 0.0, Curve::Linear),
            point(2 * PPQ, 1000.0, Curve::Linear),
        ],
    );
    c.loop_length = Some(2 * PPQ);
    d.apply(add_clip("lead", 1, c), &reg).unwrap();
    let track = d.doc().song.track(&key("lead")).unwrap();
    let flat = atomik_model::song::flatten_automation(track, 1);
    for tick in [0.0, 100.0, 960.0, 1919.0, 2000.0, 3000.0, 5000.0] {
        assert_eq!(
            d.doc().automation_values_at(tick).get(&lead()).copied(),
            Some(automation_value_at(&flat, tick)),
            "tick {tick}"
        );
    }
}

#[test]
fn the_view_copy_carries_the_shown_values_and_leaves_the_document_alone() {
    let (d, reg) = doc_with_lane();
    let doc = d.doc();
    let before = doc.clone();
    let mut shown = std::collections::BTreeMap::new();
    shown.insert(lead(), 321.0);
    let view = doc.view_with_shown(&shown);
    assert_eq!(view.param_value(&key("lead"), CUTOFF, &reg), Some(321.0));
    assert_eq!(doc, &before, "the document is not touched");
    assert_ne!(doc.param_value(&key("lead"), CUTOFF, &reg), Some(321.0));
    // Nothing shown: the document itself, no copy.
    assert!(matches!(
        doc.view_with_shown(&Default::default()),
        std::borrow::Cow::Borrowed(_)
    ));
    // A value for a device or param that does not exist is ignored.
    shown.insert((key("nope"), "x".into()), 1.0);
    let view = doc.view_with_shown(&shown);
    assert!(view.device(&key("nope")).is_none());
}

#[test]
fn a_param_with_a_lane_is_listed_as_automated() {
    let (d, _) = doc_with_lane();
    let set = d.doc().automated_params();
    assert_eq!(set.len(), 1);
    assert!(set.contains(&lead()));
}
