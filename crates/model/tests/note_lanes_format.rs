//! Project format 30: note lanes (several per track, with a name and a mute flag).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Document, MAX_NOTE_LANES, PROJECT_VERSION, SongCommand, load_project, save_project,
};
use common::audio::*;
use common::song::*;
use common::*;
use serde_json::{Value, json};

/// Track `a` with three note lanes (the second named "Bass", the third muted) and a cutoff
/// lane, a clip on each note lane; a plain track `b`.
fn laned(reg: &Registry) -> Document {
    let mut d = Document::new(atomik_model::empty_doc());
    run(
        &mut d,
        reg,
        [
            add(0, SYNTH, Some("a")),
            add(1, SYNTH, Some("b")),
            add_track("a"),
            add_track("b"),
            sc(SongCommand::AddAutomationLane {
                device: key("a"),
                param: "filter.cutoff".into(),
            }),
            sc(SongCommand::AddNoteLane {
                device: key("a"),
                name: Some("Bass".into()),
            }),
            sc(SongCommand::AddNoteLane {
                device: key("a"),
                name: None,
            }),
            sc(SongCommand::SetLaneMuted {
                device: key("a"),
                lane: 2,
                muted: true,
            }),
            add_clip("a", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])),
            add_clip("a", 1, notes_clip(BAR, BAR, vec![note(0, PPQ, 64, 0.8)])),
            add_clip("a", 2, notes_clip(0, BAR, vec![note(0, PPQ, 67, 0.8)])),
        ],
    );
    d
}

fn at_version(text: &str, n: u32) -> String {
    text.replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        &format!("\"version\": {n}"),
        1,
    )
}

/// The saved text with the lanes of track `a` rewritten by `f` (as JSON).
fn with_lanes(text: &str, f: impl FnOnce(&mut Vec<Value>)) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap();
    let lanes = v["song"]["tracks"][0]["lanes"].as_array_mut().unwrap();
    f(lanes);
    serde_json::to_string_pretty(&v).unwrap()
}

fn refused(reg: &Registry, text: &str) -> String {
    load_project(text, reg)
        .expect_err("the file must be refused")
        .to_string()
}

#[test]
fn the_project_version_is_thirty_one() {
    assert_eq!(PROJECT_VERSION, 35);
}

#[test]
fn save_project_writes_name_and_muted_after_kind_only_when_set() {
    let reg = registry();
    let text = save_project(laned(&reg).doc());
    assert!(
        text.contains("\"kind\": \"notes\", \"name\": \"Bass\",\n"),
        "{text}"
    );
    assert!(
        text.contains("\"kind\": \"notes\", \"muted\": true,\n"),
        "{text}"
    );
    assert_eq!(text.matches("\"name\": \"Bass\"").count(), 1);
    assert_eq!(text.matches("\"muted\": true").count(), 1);
}

#[test]
fn save_project_one_unnamed_lane_writes_neither_field() {
    let reg = registry();
    let mut d = Document::new(atomik_model::empty_doc());
    run(&mut d, &reg, [add(0, SYNTH, Some("a")), add_track("a")]);
    let text = save_project(d.doc());
    assert!(
        text.contains(
            "\"kind\": \"notes\",
"
        ),
        "{text}"
    );
    assert!(!text.contains("\"muted\": true"));
    assert!(!text.contains("\"kind\": \"notes\", \""));
}

#[test]
fn project_round_trip_keeps_lane_names_mute_and_order_byte_exact() {
    let reg = registry();
    let d = laned(&reg);
    let text = save_project(d.doc());
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(loaded.song, d.doc().song);
    assert_eq!(save_project(&loaded), text);
    let t = loaded.song.track(&key("a")).unwrap();
    assert_eq!(t.note_lane_count(), 3);
    assert_eq!(t.lanes[1].name.as_deref(), Some("Bass"));
    assert!(t.lanes[2].muted && !t.lanes[0].muted);
}

#[test]
fn load_project_version_29_file_loads_one_note_lane_and_resaves_as_30() {
    let reg = registry();
    let mut d = Document::new(atomik_model::empty_doc());
    run(
        &mut d,
        &reg,
        [
            add(0, SYNTH, Some("a")),
            add_track("a"),
            add_clip("a", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])),
        ],
    );
    let new = save_project(d.doc());
    let old = at_version(&new, 29);
    let loaded = load_project(&old, &reg).unwrap();
    assert_eq!(loaded.song.tracks[0].note_lane_count(), 1);
    // A project of single unnamed lanes differs from its version 29 file in the version line.
    assert_eq!(save_project(&loaded), new);
    assert_eq!(new.replacen("\"version\": 35", "\"version\": 29", 1), old);
}

#[test]
fn load_project_version_29_refuses_a_second_note_lane_and_the_new_fields() {
    let reg = registry();
    let text = save_project(laned(&reg).doc());
    let e = refused(&reg, &at_version(&text, 29));
    assert!(
        e.contains("lanes after the first must be \"automation\""),
        "{e}"
    );
    let named = with_lanes(&text, |l| l.truncate(1));
    let named = with_lanes(&named, |l| {
        l[0]["name"] = json!("Lead");
    });
    let e = refused(&reg, &at_version(&named, 29));
    assert!(e.contains("name") && e.contains("unknown field"), "{e}");
    let muted = with_lanes(&text, |l| {
        l.truncate(1);
        l[0]["muted"] = json!(true);
    });
    let e = refused(&reg, &at_version(&muted, 29));
    assert!(e.contains("muted") && e.contains("unknown field"), "{e}");
}

#[test]
fn load_project_refuses_a_note_lane_after_an_automation_lane() {
    let reg = registry();
    let text = save_project(laned(&reg).doc());
    let bad = with_lanes(&text, |l| {
        let n = l.remove(1);
        l.push(n);
    });
    let e = refused(&reg, &bad);
    assert!(e.contains("cannot follow an automation lane"), "{e}");
}

#[test]
fn load_project_refuses_more_than_the_most_note_lanes() {
    let reg = registry();
    let text = save_project(laned(&reg).doc());
    let ok = with_lanes(&text, |l| {
        let extra = l[0].clone();
        while l.iter().filter(|x| x["kind"] == "notes").count() < MAX_NOTE_LANES {
            l.insert(0, extra.clone());
        }
    });
    // The clips repeat across the copies, so give each copy its own ids by dropping them.
    let ok = with_lanes(&ok, |l| {
        for lane in l.iter_mut() {
            lane["clips"] = json!([]);
        }
    });
    let loaded = load_project(&ok, &reg).unwrap();
    assert_eq!(loaded.song.tracks[0].note_lane_count(), MAX_NOTE_LANES);
    let bad = with_lanes(&ok, |l| {
        l.insert(0, l[0].clone());
    });
    let e = refused(&reg, &bad);
    assert!(e.contains("at most 64 note lanes"), "{e}");
}

#[test]
fn load_project_refuses_a_bad_lane_name() {
    let reg = registry();
    let text = save_project(laned(&reg).doc());
    for name in ["", " padded ", &"x".repeat(33), "tab\there"] {
        let bad = with_lanes(&text, |l| l[1]["name"] = json!(name));
        let e = refused(&reg, &bad);
        assert!(e.contains("lanes[1].name"), "{name:?}: {e}");
        assert!(e.contains("trimmed"), "{name:?}: {e}");
    }
    let bad = with_lanes(&text, |l| l[1]["name"] = json!(3));
    assert!(refused(&reg, &bad).contains("lanes[1].name"));
}

#[test]
fn load_project_refuses_muted_false_and_a_non_boolean() {
    let reg = registry();
    let text = save_project(laned(&reg).doc());
    for value in [json!(false), json!(1), json!("true")] {
        let bad = with_lanes(&text, |l| l[2]["muted"] = value.clone());
        let e = refused(&reg, &bad);
        assert!(e.contains("lanes[2].muted"), "{value}: {e}");
    }
}

#[test]
fn load_project_refuses_name_and_muted_on_automation_and_audio_lanes() {
    let reg = registry();
    let text = save_project(laned(&reg).doc());
    for field in ["name", "muted"] {
        let bad = with_lanes(&text, |l| {
            l[3][field] = if field == "name" {
                json!("x")
            } else {
                json!(true)
            };
        });
        let e = refused(&reg, &bad);
        assert!(e.contains(field) && e.contains("unknown field"), "{e}");
    }
    let reg = audio_registry();
    let d = with_audio_track(&reg);
    let text = save_project(d.doc());
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["song"]["tracks"][0]["lanes"][0]["muted"] = json!(true);
    let e = load_project(&serde_json::to_string(&v).unwrap(), &reg)
        .expect_err("muted audio lane")
        .to_string();
    assert!(e.contains("muted") && e.contains("unknown field"), "{e}");
}
