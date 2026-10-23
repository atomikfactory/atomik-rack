//! Track heights of the arrangement: the commands, undo and drag merging, the validation, and
//! project format 15 (older files load with none).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    DEFAULT_TRACK_HEIGHT, DeviceKey, Document, MAX_TRACK_HEIGHT, MIN_TRACK_HEIGHT, ModelError,
    PROJECT_VERSION, SongCommand, SyncOp, load_project, save_project,
};
use common::song::*;
use common::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn heights(pairs: &[(&str, u16)]) -> BTreeMap<DeviceKey, u16> {
    pairs.iter().map(|(k, h)| (key(k), *h)).collect()
}

/// Two tracks, `synth` and `lead`.
fn doc(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    d.apply(add_track("lead"), reg).unwrap();
    d
}

fn rejects(d: &mut Document, cmd: SongCommand, reg: &Registry) -> ModelError {
    let before = d.doc().clone();
    let undo = d.can_undo();
    let e = d.apply(sc(cmd), reg).expect_err("must be rejected");
    assert_eq!(d.doc(), &before, "a rejected command changes nothing");
    assert_eq!(d.can_undo(), undo);
    e
}

#[test]
fn the_limits_and_the_default_are_in_order() {
    assert_eq!(
        (MIN_TRACK_HEIGHT, DEFAULT_TRACK_HEIGHT, MAX_TRACK_HEIGHT),
        (24, 60, 400)
    );
}

#[test]
fn a_new_song_has_no_track_heights() {
    let reg = registry();
    assert!(doc(&reg).doc().song.track_heights.is_empty());
}

#[test]
fn set_track_heights_replaces_the_map_reports_a_view_change_and_undoes() {
    let reg = registry();
    let mut d = doc(&reg);
    let ops = d
        .apply(
            sc(SongCommand::SetTrackHeights(heights(&[("synth", 120)]))),
            &reg,
        )
        .unwrap();
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    assert_eq!(d.doc().song.track_heights, heights(&[("synth", 120)]));
    d.apply(
        sc(SongCommand::SetTrackHeights(heights(&[
            ("lead", 40),
            ("synth", 90),
        ]))),
        &reg,
    )
    .unwrap();
    assert_eq!(d.doc().song.track_heights.len(), 2);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().song.track_heights, heights(&[("synth", 120)]));
    d.undo(&reg).unwrap();
    assert!(d.doc().song.track_heights.is_empty());
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().song.track_heights, heights(&[("synth", 120)]));
}

#[test]
fn heights_outside_the_limits_are_rejected_and_the_limits_are_accepted() {
    let reg = registry();
    let mut d = doc(&reg);
    for bad in [0, MIN_TRACK_HEIGHT - 1, MAX_TRACK_HEIGHT + 1, u16::MAX] {
        let e = rejects(
            &mut d,
            SongCommand::SetTrackHeights(heights(&[("synth", bad)])),
            &reg,
        );
        assert!(e.to_string().contains("height"), "{bad}: {e}");
    }
    for ok in [MIN_TRACK_HEIGHT, MAX_TRACK_HEIGHT] {
        d.apply(
            sc(SongCommand::SetTrackHeights(heights(&[("synth", ok)]))),
            &reg,
        )
        .unwrap();
    }
}

#[test]
fn entries_of_devices_without_a_track_are_dropped() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(
        sc(SongCommand::SetTrackHeights(heights(&[
            ("synth", 80),
            ("delay", 80),
            ("nobody", 80),
        ]))),
        &reg,
    )
    .unwrap();
    assert_eq!(d.doc().song.track_heights, heights(&[("synth", 80)]));
}

#[test]
fn a_drag_is_one_undo_step() {
    let reg = registry();
    let mut d = doc(&reg);
    for h in [70, 90, 110, 130] {
        d.apply(
            sc(SongCommand::DragTrackHeights {
                heights: heights(&[("lead", h)]),
                merge: 7,
            }),
            &reg,
        )
        .unwrap();
    }
    assert_eq!(d.doc().song.track_heights, heights(&[("lead", 130)]));
    d.undo(&reg).unwrap();
    assert!(
        d.doc().song.track_heights.is_empty(),
        "one step undone it all"
    );
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().song.track_heights, heights(&[("lead", 130)]));
    // Another drag is another step.
    d.apply(
        sc(SongCommand::DragTrackHeights {
            heights: heights(&[("lead", 60)]),
            merge: 8,
        }),
        &reg,
    )
    .unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().song.track_heights, heights(&[("lead", 130)]));
}

// ---- project format 15 -------------------------------------------------------------------

#[test]
fn a_song_without_heights_writes_no_field() {
    let reg = registry();
    assert!(!save_project(doc(&reg).doc()).contains("track_heights"));
}

#[test]
fn heights_round_trip_canonically() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(
        sc(SongCommand::SetTrackHeights(heights(&[
            ("synth", 150),
            ("lead", 24),
        ]))),
        &reg,
    )
    .unwrap();
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 17);
    assert_eq!(
        v["song"]["track_heights"],
        json!({"lead": 24, "synth": 150})
    );
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text, "canonical");
}

#[test]
fn a_version_14_project_loads_with_none_and_resaves_as_15() {
    let reg = registry();
    let text = save_project(doc(&reg).doc());
    let old = text.replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 14",
        1,
    );
    let loaded = load_project(&old, &reg).unwrap();
    assert!(loaded.song.track_heights.is_empty());
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn heights_are_unknown_before_version_15() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(
        sc(SongCommand::SetTrackHeights(heights(&[("synth", 100)]))),
        &reg,
    )
    .unwrap();
    let text = save_project(d.doc()).replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 14",
        1,
    );
    let e = load_project(&text, &reg).expect_err("v14 has no track heights");
    assert!(e.to_string().contains("track_heights") && e.to_string().contains("unknown field"));
}

#[test]
fn bad_heights_name_their_json_path_and_unknown_tracks_are_ignored() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(
        sc(SongCommand::SetTrackHeights(heights(&[("synth", 100)]))),
        &reg,
    )
    .unwrap();
    let good = save_project(d.doc());
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(Edit, &str, &str)> = vec![
        (
            Box::new(|v| v["song"]["track_heights"]["synth"] = json!(23)),
            "song.track_heights.synth",
            "outside 24..=400",
        ),
        (
            Box::new(|v| v["song"]["track_heights"]["synth"] = json!(401)),
            "song.track_heights.synth",
            "outside",
        ),
        (
            Box::new(|v| v["song"]["track_heights"]["synth"] = json!(60.5)),
            "song.track_heights.synth",
            "",
        ),
        (
            Box::new(|v| v["song"]["track_heights"]["synth"] = json!("tall")),
            "song.track_heights.synth",
            "",
        ),
        (
            Box::new(|v| v["song"]["track_heights"] = json!([100])),
            "song.track_heights",
            "object",
        ),
    ];
    for (edit, path, needle) in cases {
        let mut v: Value = serde_json::from_str(&good).unwrap();
        edit(&mut v);
        let e = load_project(&v.to_string(), &reg).expect_err("must be rejected");
        let ModelError::Patch(msg) = &e else {
            panic!("expected a patch error, got {e:?}")
        };
        assert!(
            msg.starts_with(&format!("{path}: ")),
            "{msg} (wanted {path})"
        );
        assert!(msg.contains(needle), "{msg} lacks {needle:?}");
    }
    // The key of a device without a track is ignored, so a saved file never lists one.
    let mut v: Value = serde_json::from_str(&good).unwrap();
    v["song"]["track_heights"]["ghost"] = json!(80);
    let loaded = load_project(&v.to_string(), &reg).unwrap();
    assert_eq!(loaded.song.track_heights, heights(&[("synth", 100)]));
}

#[test]
fn a_removed_track_leaves_no_height_in_the_file() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(
        sc(SongCommand::SetTrackHeights(heights(&[
            ("synth", 100),
            ("lead", 200),
        ]))),
        &reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::RemoveTrack {
            device: key("lead"),
        }),
        &reg,
    )
    .unwrap();
    let text = save_project(d.doc());
    assert!(
        text.contains("\"synth\": 100") && !text.contains("\"lead\": 200"),
        "{text}"
    );
    // Undoing the removal brings the track back, and its height with it (the map was never
    // touched).
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().song.track_heights.get(&key("lead")), Some(&200));
}
