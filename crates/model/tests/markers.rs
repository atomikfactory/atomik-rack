//! Song markers and range markers: the commands, undo and drag merging, the validation, and
//! project format 14 (older files load with none).

mod common;

use atomik_core::PPQ;
use atomik_device_sdk::Registry;
use atomik_model::{
    Document, MAX_TICK, Marker, ModelError, PROJECT_VERSION, RangeMarker, SongCommand, SyncOp,
    load_project, save_project,
};
use common::song::*;
use common::*;
use serde_json::{Value, json};

const BAR: i64 = PPQ * 4;

fn marker(id: u32, tick: i64, name: &str) -> Marker {
    Marker {
        id,
        tick,
        name: name.into(),
        color: None,
    }
}

fn range(id: u32, start: i64, end: i64, name: &str) -> RangeMarker {
    RangeMarker {
        id,
        start,
        end,
        name: name.into(),
        color: None,
    }
}

fn doc(reg: &Registry) -> Document {
    song_start(reg)
}

fn set_markers(d: &mut Document, reg: &Registry, m: Vec<Marker>) -> Vec<SyncOp> {
    d.apply(sc(SongCommand::SetMarkers(m)), reg).unwrap()
}

fn set_ranges(d: &mut Document, reg: &Registry, r: Vec<RangeMarker>) -> Vec<SyncOp> {
    d.apply(sc(SongCommand::SetRanges(r)), reg).unwrap()
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
fn a_new_song_has_no_markers_or_ranges() {
    let d = doc(&registry());
    assert!(d.doc().song.markers.is_empty() && d.doc().song.ranges.is_empty());
}

#[test]
fn set_markers_replaces_the_list_sorted_by_tick_and_reports_a_view_change() {
    let reg = registry();
    let mut d = doc(&reg);
    let ops = set_markers(
        &mut d,
        &reg,
        vec![marker(2, 3 * BAR, "B"), marker(1, BAR, "A")],
    );
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    let m = &d.doc().song.markers;
    assert_eq!(
        m.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
        ["A", "B"]
    );
}

#[test]
fn markers_at_one_tick_keep_their_id_order() {
    let reg = registry();
    let mut d = doc(&reg);
    set_markers(
        &mut d,
        &reg,
        vec![marker(5, BAR, "late id"), marker(3, BAR, "early id")],
    );
    let ids: Vec<u32> = d.doc().song.markers.iter().map(|m| m.id).collect();
    assert_eq!(ids, [3, 5]);
}

#[test]
fn set_markers_is_one_undo_step_and_redoes() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    set_markers(&mut d, &reg, vec![marker(1, 0, "Start")]);
    let after = d.doc().clone();
    assert_eq!(d.undo(&reg), Some(vec![SyncOp::ViewChanged]));
    assert_eq!(d.doc(), &before);
    assert_eq!(d.redo(&reg), Some(vec![SyncOp::ViewChanged]));
    assert_eq!(d.doc(), &after);
    assert!(!d.can_redo());
}

#[test]
fn set_ranges_sorts_by_start_and_undoes() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    let ops = set_ranges(
        &mut d,
        &reg,
        vec![
            range(2, 4 * BAR, 8 * BAR, "Verse"),
            range(1, 0, 4 * BAR, "Intro"),
        ],
    );
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    let names: Vec<&str> = d
        .doc()
        .song
        .ranges
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(names, ["Intro", "Verse"]);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn drags_merge_into_one_step_per_group_and_markers_and_ranges_do_not_mix() {
    let reg = registry();
    let mut d = doc(&reg);
    set_markers(&mut d, &reg, vec![marker(1, BAR, "A")]);
    let base = d.doc().clone();
    for x in 1..=5 {
        d.apply(
            sc(SongCommand::DragMarkers {
                markers: vec![marker(1, BAR + x * 10, "A")],
                merge: 7,
            }),
            &reg,
        )
        .unwrap();
    }
    assert_eq!(d.doc().song.markers[0].tick, BAR + 50);
    // A drag of ranges with the same id is a different group.
    d.apply(
        sc(SongCommand::DragRanges {
            ranges: vec![range(1, 0, BAR, "R")],
            merge: 7,
        }),
        &reg,
    )
    .unwrap();
    d.undo(&reg).unwrap();
    assert!(d.doc().song.ranges.is_empty());
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &base, "five marker drags were one undo step");
    d.undo(&reg).unwrap();
    assert!(d.doc().song.markers.is_empty());
    assert!(!d.can_undo());
}

#[test]
fn a_range_drag_merges_across_its_moves() {
    let reg = registry();
    let mut d = doc(&reg);
    set_ranges(&mut d, &reg, vec![range(1, 0, BAR, "R")]);
    for x in 1..=3 {
        d.apply(
            sc(SongCommand::DragRanges {
                ranges: vec![range(1, x, BAR + x, "R")],
                merge: 1,
            }),
            &reg,
        )
        .unwrap();
    }
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().song.ranges[0].start, 0);
}

#[test]
fn bad_markers_are_rejected_without_a_trace() {
    let reg = registry();
    let mut d = doc(&reg);
    set_markers(&mut d, &reg, vec![marker(1, 0, "keep")]);
    let cases: Vec<(Vec<Marker>, &str)> = vec![
        (vec![marker(1, -1, "x")], "tick"),
        (vec![marker(1, MAX_TICK + 1, "x")], "tick"),
        (vec![marker(1, 0, "")], "name"),
        (vec![marker(1, 0, "   ")], "name"),
        (vec![marker(1, 0, &"x".repeat(65))], "name"),
        (vec![marker(1, 0, "a"), marker(1, 5, "b")], "id"),
        (vec![marker(1, 0, " padded ")], "name"),
    ];
    for (list, needle) in cases {
        let e = rejects(&mut d, SongCommand::SetMarkers(list), &reg);
        assert!(e.to_string().contains(needle), "{e} lacks {needle}");
    }
}

#[test]
fn bad_ranges_are_rejected_without_a_trace() {
    let reg = registry();
    let mut d = doc(&reg);
    let cases: Vec<(Vec<RangeMarker>, &str)> = vec![
        (vec![range(1, 5, 5, "x")], "end"),
        (vec![range(1, 9, 5, "x")], "end"),
        (vec![range(1, -1, 5, "x")], "start"),
        (vec![range(1, 0, MAX_TICK + 1, "x")], "end"),
        (vec![range(1, 0, 5, "")], "name"),
        (vec![range(1, 0, 5, "a"), range(1, 6, 9, "b")], "id"),
    ];
    for (list, needle) in cases {
        let e = rejects(&mut d, SongCommand::SetRanges(list), &reg);
        assert!(e.to_string().contains(needle), "{e} lacks {needle}");
    }
}

#[test]
fn ranges_may_overlap_and_nest() {
    let reg = registry();
    let mut d = doc(&reg);
    set_ranges(
        &mut d,
        &reg,
        vec![range(1, 0, 8 * BAR, "All"), range(2, BAR, 2 * BAR, "Inner")],
    );
    assert_eq!(d.doc().song.ranges.len(), 2);
}

#[test]
fn next_ids_are_above_every_existing_id() {
    let reg = registry();
    let mut d = doc(&reg);
    assert_eq!(d.doc().song.next_marker_id(), 1);
    assert_eq!(d.doc().song.next_range_id(), 1);
    set_markers(&mut d, &reg, vec![marker(4, 0, "a"), marker(9, 5, "b")]);
    set_ranges(&mut d, &reg, vec![range(3, 0, 5, "r")]);
    assert_eq!(d.doc().song.next_marker_id(), 10);
    assert_eq!(d.doc().song.next_range_id(), 4);
}

#[test]
fn marker_neighbours_are_strictly_before_and_after_a_tick() {
    let reg = registry();
    let mut d = doc(&reg);
    set_markers(
        &mut d,
        &reg,
        vec![marker(1, BAR, "a"), marker(2, 3 * BAR, "b")],
    );
    let s = &d.doc().song;
    assert_eq!(s.prev_marker_tick(BAR), None);
    assert_eq!(s.prev_marker_tick(BAR + 1), Some(BAR));
    assert_eq!(s.prev_marker_tick(10 * BAR), Some(3 * BAR));
    assert_eq!(s.next_marker_tick(0), Some(BAR));
    assert_eq!(s.next_marker_tick(BAR), Some(3 * BAR));
    assert_eq!(s.next_marker_tick(3 * BAR), None);
}

#[test]
fn a_tempo_change_leaves_markers_on_their_ticks() {
    let reg = registry();
    let mut d = doc(&reg);
    set_markers(&mut d, &reg, vec![marker(1, 2 * BAR, "a")]);
    d.apply(
        sc(SongCommand::SetTempoMap(vec![
            atomik_model::TempoEvent::step(0, 60.0),
        ])),
        &reg,
    )
    .unwrap();
    assert_eq!(d.doc().song.markers[0].tick, 2 * BAR);
}

// ---- project format 14 -------------------------------------------------------------------

fn with_marks(reg: &Registry) -> Document {
    let mut d = doc(reg);
    d.apply(
        sc(SongCommand::SetMarkers(vec![
            Marker {
                id: 1,
                tick: 0,
                name: "Start \"here\"".into(),
                color: Some([0xe0, 0x5a, 0x4f]),
            },
            marker(2, 2 * BAR + 17, "Drop"),
        ])),
        reg,
    )
    .unwrap();
    d.apply(
        sc(SongCommand::SetRanges(vec![
            RangeMarker {
                id: 1,
                start: 0,
                end: 4 * BAR,
                name: "Intro".into(),
                color: Some([0x3f, 0x8f, 0xd2]),
            },
            range(2, 4 * BAR, 12 * BAR, "Verse é"),
        ])),
        reg,
    )
    .unwrap();
    d
}

#[test]
fn markers_and_ranges_round_trip_canonically() {
    let reg = registry();
    let d = with_marks(&reg);
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 25);
    assert_eq!(
        v["song"]["markers"],
        json!([
            {"tick": 0, "name": "Start \"here\"", "color": "#e05a4f"},
            {"tick": 2 * BAR + 17, "name": "Drop"}
        ])
    );
    assert_eq!(
        v["song"]["ranges"],
        json!([
            {"start": 0, "end": 4 * BAR, "name": "Intro", "color": "#3f8fd2"},
            {"start": 4 * BAR, "end": 12 * BAR, "name": "Verse é"}
        ])
    );
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded, *d.doc(), "ids are assigned 1.. in file order");
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_song_without_markers_writes_neither_field() {
    let reg = registry();
    let text = save_project(doc(&reg).doc());
    assert!(!text.contains("\"markers\"") && !text.contains("\"ranges\""));
}

#[test]
fn a_version_13_project_loads_with_none_and_resaves_as_14() {
    let reg = registry();
    let text = save_project(doc(&reg).doc());
    let old = text.replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 13",
        1,
    );
    let loaded = load_project(&old, &reg).unwrap();
    assert!(loaded.song.markers.is_empty() && loaded.song.ranges.is_empty());
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn markers_are_unknown_before_version_14() {
    let reg = registry();
    let text = save_project(with_marks(&reg).doc()).replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 13",
        1,
    );
    let e = load_project(&text, &reg).expect_err("v13 has no markers");
    assert!(e.to_string().contains("song") && e.to_string().contains("unknown field"));
}

#[test]
fn unsorted_markers_load_sorted_and_equal_ticks_keep_file_order() {
    let reg = registry();
    let mut v: Value = serde_json::from_str(&save_project(with_marks(&reg).doc())).unwrap();
    v["song"]["markers"] = json!([
        {"tick": 50, "name": "b"},
        {"tick": 10, "name": "a"},
        {"tick": 50, "name": "c"}
    ]);
    let doc = load_project(&v.to_string(), &reg).unwrap();
    let names: Vec<&str> = doc.song.markers.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["a", "b", "c"]);
    let mut ids: Vec<u32> = doc.song.markers.iter().map(|m| m.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 3);
}

#[test]
fn marker_errors_name_their_json_path() {
    let reg = registry();
    let good = save_project(with_marks(&reg).doc());
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(Edit, &str, &str)> = vec![
        (
            Box::new(|v| v["song"]["markers"][0]["tick"] = json!(-4)),
            "song.markers[0].tick",
            "",
        ),
        (
            Box::new(|v| v["song"]["markers"][1]["name"] = json!("  ")),
            "song.markers[1].name",
            "name",
        ),
        (
            Box::new(|v| v["song"]["markers"][1]["color"] = json!("red")),
            "song.markers[1].color",
            "colour",
        ),
        (
            Box::new(|v| v["song"]["markers"][1]["extra"] = json!(1)),
            "song.markers[1].extra",
            "unknown field",
        ),
        (
            Box::new(|v| v["song"]["ranges"][0]["end"] = json!(0)),
            "song.ranges[0].end",
            "end",
        ),
        (
            Box::new(|v| {
                v["song"]["ranges"][1]
                    .as_object_mut()
                    .unwrap()
                    .remove("name");
            }),
            "song.ranges[1].name",
            "missing field",
        ),
        (
            Box::new(|v| v["song"]["ranges"] = json!({})),
            "song.ranges",
            "array",
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
}
