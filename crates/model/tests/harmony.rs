//! The song's harmony in the model: the song key, key changes and chord lane as commands and their
//! undo, the optional fields of project format 45 (written only when set, an old file without
//! them), and the strict reading of a bad one.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::theory::{ChordKind, ScaleKind};
use atomik_model::{
    ChordEvent, Document, Harmony, Key, KeyChange, SongCommand, SyncOp, load_project, save_project,
};
use common::song::*;
use common::*;
use serde_json::Value;

fn doc(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    d
}

fn set(d: &mut Document, reg: &Registry, h: Harmony) -> Vec<SyncOp> {
    d.apply(sc(SongCommand::SetHarmony(h)), reg).unwrap()
}

fn key(root: u8, scale: ScaleKind) -> Key {
    Key::new(root, scale)
}

fn chord(bar: i64, bars: i64, root: u8, quality: ChordKind) -> ChordEvent {
    ChordEvent::new(bar * BAR, bars * BAR, root, quality)
}

fn full() -> Harmony {
    let mut slash = chord(2, 1, 0, ChordKind::Major);
    slash.bass = Some(7);
    Harmony {
        key: Some(key(9, ScaleKind::NaturalMinor)),
        changes: vec![KeyChange {
            tick: 8 * BAR,
            key: key(2, ScaleKind::Dorian),
        }],
        chords: vec![
            chord(0, 2, 9, ChordKind::MinorSeventh),
            slash,
            chord(4, 4, 5, ChordKind::MajorSeventh),
        ],
    }
}

fn text(d: &Document) -> Value {
    serde_json::from_str(&save_project(d.doc())).unwrap()
}

#[test]
fn a_song_starts_without_a_key_or_chords() {
    let reg = registry();
    let d = doc(&reg);
    assert!(d.doc().song.harmony.is_empty());
    let v = text(&d);
    for field in ["key", "key_changes", "chords"] {
        assert!(
            v["song"].get(field).is_none(),
            "{field} is written only when set"
        );
    }
}

#[test]
fn setting_the_harmony_is_one_undo_step_that_reports_a_view_change() {
    let reg = registry();
    let mut d = doc(&reg);
    let ops = set(&mut d, &reg, full());
    assert_eq!(ops, vec![SyncOp::ViewChanged]);
    assert_eq!(d.doc().song.harmony, full());
    d.undo().unwrap();
    assert!(d.doc().song.harmony.is_empty());
    d.redo().unwrap();
    assert_eq!(d.doc().song.harmony, full());
    assert_eq!(d.undo_label(), Some("Edit Key and Chords"));
}

#[test]
fn the_key_at_a_tick_follows_the_changes_in_the_document() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, &reg, full());
    let h = &d.doc().song.harmony;
    assert_eq!(h.key_at(0), Some(key(9, ScaleKind::NaturalMinor)));
    assert_eq!(h.key_at(8 * BAR - 1), Some(key(9, ScaleKind::NaturalMinor)));
    assert_eq!(h.key_at(8 * BAR), Some(key(2, ScaleKind::Dorian)));
}

#[test]
fn a_drag_is_one_undo_step() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, &reg, full());
    for bar in 5..9 {
        let mut h = full();
        h.chords[2].tick = bar * BAR;
        d.apply(
            sc(SongCommand::DragHarmony {
                harmony: h,
                merge: 7,
            }),
            &reg,
        )
        .unwrap();
    }
    assert_eq!(d.doc().song.harmony.chords[2].tick, 8 * BAR);
    d.end_merge();
    d.undo().unwrap();
    assert_eq!(
        d.doc().song.harmony,
        full(),
        "one undo undoes the whole drag"
    );
}

#[test]
fn a_harmony_with_overlapping_chords_or_a_chromatic_key_is_refused() {
    let reg = registry();
    let mut d = doc(&reg);
    let mut overlap = full();
    overlap.chords[1].tick = BAR;
    assert!(d.apply(sc(SongCommand::SetHarmony(overlap)), &reg).is_err());
    let mut chromatic = full();
    chromatic.key = Some(key(0, ScaleKind::Chromatic));
    assert!(
        d.apply(sc(SongCommand::SetHarmony(chromatic)), &reg)
            .is_err()
    );
    let mut short = full();
    short.chords[0].length = 0;
    assert!(d.apply(sc(SongCommand::SetHarmony(short)), &reg).is_err());
    assert!(d.doc().song.harmony.is_empty(), "nothing changed");
}

#[test]
fn the_harmony_round_trips_through_the_project_file() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, &reg, full());
    let saved = save_project(d.doc());
    let back = load_project(&saved, &reg).unwrap();
    assert_eq!(back.song.harmony, full());
    assert_eq!(save_project(&back), saved, "the file is canonical");
    let v: Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(v["song"]["key"]["scale"], "natural_minor");
    assert_eq!(v["song"]["key_changes"][0]["tick"], 8 * BAR);
    assert_eq!(v["song"]["chords"][1]["bass"], 7);
    assert_eq!(v["song"]["chords"][0]["quality"], "m7");
    assert!(v["song"]["chords"][0].get("bass").is_none());
}

#[test]
fn a_key_alone_is_written_without_the_other_fields() {
    let reg = registry();
    let mut d = doc(&reg);
    set(
        &mut d,
        &reg,
        Harmony {
            key: Some(key(0, ScaleKind::Major)),
            ..Harmony::default()
        },
    );
    let v = text(&d);
    assert!(v["song"].get("key").is_some());
    assert!(v["song"].get("key_changes").is_none());
    assert!(v["song"].get("chords").is_none());
}

#[test]
fn the_new_fields_are_ignorable_so_an_older_reader_drops_them() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, &reg, full());
    let v = text(&d);
    assert_eq!(v["compat"], 44);
    for path in ["song.key", "song.key_changes", "song.chords"] {
        assert_eq!(v["ignorable"][path], 45, "{path}");
    }
}

#[test]
fn a_file_from_before_the_harmony_opens_with_none() {
    let reg = registry();
    let d = doc(&reg);
    let mut v = text(&d);
    v["song"].as_object_mut().unwrap().remove("key");
    let back = load_project(&v.to_string(), &reg).unwrap();
    assert!(back.song.harmony.is_empty());
}

fn with_song_field(field: &str, value: Value) -> String {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, &reg, full());
    let mut v = text(&d);
    v["song"][field] = value;
    v.to_string()
}

fn load_error(text: &str) -> String {
    match load_project(text, &registry()) {
        Ok(_) => panic!("the file should not load"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn a_bad_harmony_in_a_file_is_refused_with_its_place() {
    let cases = [
        (
            "key",
            serde_json::json!({"root": 12, "scale": "major"}),
            "song.key.root",
        ),
        (
            "key",
            serde_json::json!({"root": 0, "scale": "chromatic"}),
            "song.key.scale",
        ),
        (
            "key",
            serde_json::json!({"root": 0, "scale": "nope"}),
            "song.key.scale",
        ),
        (
            "key",
            serde_json::json!({"root": 0, "scale": "major", "mode": 1}),
            "song.key.mode",
        ),
        (
            "key_changes",
            serde_json::json!([{"tick": -1, "root": 0, "scale": "major"}]),
            "song.key_changes[0].tick",
        ),
        (
            "chords",
            serde_json::json!([{"tick": 0, "length": 100, "root": 0, "quality": "wobble"}]),
            "song.chords[0].quality",
        ),
        (
            "chords",
            serde_json::json!([{"tick": 0, "length": 100, "root": 0, "quality": "major", "bass": 40}]),
            "song.chords[0].bass",
        ),
        (
            "chords",
            serde_json::json!([
                {"tick": 0, "length": 100, "root": 0, "quality": "major"},
                {"tick": 50, "length": 100, "root": 5, "quality": "major"}
            ]),
            "song",
        ),
        ("chords", serde_json::json!("C"), "song.chords"),
    ];
    for (field, value, place) in cases {
        let message = load_error(&with_song_field(field, value.clone()));
        assert!(message.contains(place), "{value}: {message}");
    }
}
