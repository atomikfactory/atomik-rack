//! The scale of a note clip: `SetClipScale`, undo, which clips take it, what keeps it (split,
//! join, duplicate), and project format 16 (older files load with none).

mod common;

use atomik_device_sdk::Registry;
use atomik_model::theory::{Scale, ScaleKind};
use atomik_model::{
    ClipId, Command, Document, ModelError, PROJECT_VERSION, SongCommand, SyncOp, WorkflowCommand,
    load_project, save_project,
};
use common::song::*;
use common::*;
use serde_json::{Value, json};

/// A synth track with one note clip (id 1), the clip placed at bar 1.
fn doc(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    d.apply(
        add_clip(
            "synth",
            0,
            notes_clip(
                0,
                2 * BAR,
                vec![note(0, PPQ, 60, 0.8), note(PPQ, PPQ, 64, 0.8)],
            ),
        ),
        reg,
    )
    .unwrap();
    d
}

fn clip(d: &Document, id: u64) -> atomik_model::Clip {
    d.doc().song.clip(ClipId(id)).expect("clip").2.clone()
}

fn set(d: &mut Document, id: u64, scale: Option<Scale>, reg: &Registry) -> Vec<SyncOp> {
    d.apply(
        sc(SongCommand::SetClipScale {
            clip: ClipId(id),
            scale,
        }),
        reg,
    )
    .unwrap()
}

fn a_minor() -> Scale {
    Scale::new(9, ScaleKind::NaturalMinor)
}

#[test]
fn the_project_version_is_sixteen() {
    assert_eq!(PROJECT_VERSION, 39);
}

#[test]
fn a_new_clip_has_no_scale() {
    let reg = registry();
    let d = doc(&reg);
    assert_eq!(clip(&d, 1).scale, None);
}

#[test]
fn set_clip_scale_stores_it_reports_the_song_and_undoes_as_one_step() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    let ops = set(&mut d, 1, Some(a_minor()), &reg);
    assert_eq!(ops, vec![SyncOp::SongChanged]);
    assert_eq!(clip(&d, 1).scale, Some(a_minor()));
    // The notes are untouched.
    assert_eq!(
        clip(&d, 1).content,
        before.song.clip(ClipId(1)).unwrap().2.content
    );
    let d2 = Scale::new(2, ScaleKind::Dorian);
    set(&mut d, 1, Some(d2), &reg);
    assert_eq!(clip(&d, 1).scale, Some(d2));
    d.undo(&reg).unwrap();
    assert_eq!(clip(&d, 1).scale, Some(a_minor()));
    d.undo(&reg).unwrap();
    assert_eq!(
        d.doc(),
        &before,
        "two commands, two steps, back to the start"
    );
    d.redo(&reg).unwrap();
    d.redo(&reg).unwrap();
    assert_eq!(clip(&d, 1).scale, Some(d2));
}

#[test]
fn clearing_and_a_chromatic_scale_both_mean_no_scale() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, Some(a_minor()), &reg);
    set(&mut d, 1, None, &reg);
    assert_eq!(clip(&d, 1).scale, None);
    set(&mut d, 1, Some(a_minor()), &reg);
    set(&mut d, 1, Some(Scale::new(5, ScaleKind::Chromatic)), &reg);
    assert_eq!(clip(&d, 1).scale, None, "chromatic is stored as none");
}

#[test]
fn only_note_clips_take_a_scale_and_an_unknown_clip_is_refused() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = d.doc().clone();
    let e = d
        .apply(
            sc(SongCommand::SetClipScale {
                clip: ClipId(99),
                scale: Some(a_minor()),
            }),
            &reg,
        )
        .expect_err("no such clip");
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e:?}");
    assert_eq!(d.doc(), &before);
    // An automation clip is not a note clip.
    d.apply(
        sc(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: "filter.cutoff".into(),
        }),
        &reg,
    )
    .unwrap();
    d.apply(
        add_clip(
            "synth",
            1,
            auto_clip(0, BAR, vec![point(0, 0.5, atomik_model::Curve::Linear)]),
        ),
        &reg,
    )
    .unwrap();
    let auto = d.last_clip_id().unwrap();
    let before = d.doc().clone();
    let e = d
        .apply(
            sc(SongCommand::SetClipScale {
                clip: auto,
                scale: Some(a_minor()),
            }),
            &reg,
        )
        .expect_err("automation clips have no scale");
    assert!(matches!(e, ModelError::BadSong(_)), "{e:?}");
    assert_eq!(d.doc(), &before);
    let mut bad = auto_clip(BAR, BAR, vec![point(0, 0.5, atomik_model::Curve::Linear)]);
    bad.scale = Some(a_minor());
    assert!(d.apply(add_clip("synth", 1, bad), &reg).is_err());
}

#[test]
fn add_clip_keeps_a_scale_and_drops_a_chromatic_one() {
    let reg = registry();
    let mut d = doc(&reg);
    let mut c = notes_clip(2 * BAR, BAR, vec![note(0, PPQ, 62, 0.8)]);
    c.scale = Some(a_minor());
    d.apply(add_clip("synth", 0, c.clone()), &reg).unwrap();
    let id = d.last_clip_id().unwrap();
    assert_eq!(clip(&d, id.0).scale, Some(a_minor()));
    c.start = 3 * BAR;
    c.scale = Some(Scale::new(0, ScaleKind::Chromatic));
    d.apply(add_clip("synth", 0, c), &reg).unwrap();
    let id = d.last_clip_id().unwrap();
    assert_eq!(clip(&d, id.0).scale, None);
}

#[test]
fn a_split_keeps_the_scale_on_both_halves() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, Some(a_minor()), &reg);
    d.apply(
        sc(SongCommand::SplitClip {
            id: ClipId(1),
            at: BAR,
        }),
        &reg,
    )
    .unwrap();
    let ids = clip_ids(d.doc());
    assert_eq!(ids.len(), 2);
    for id in ids {
        assert_eq!(clip(&d, id.0).scale, Some(a_minor()), "clip {}", id.0);
    }
}

#[test]
fn a_duplicated_clip_keeps_its_scale() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, Some(a_minor()), &reg);
    d.apply(
        Command::Workflow(WorkflowCommand::DuplicateClips {
            ids: vec![ClipId(1)],
        }),
        &reg,
    )
    .unwrap();
    let ids = clip_ids(d.doc());
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|id| clip(&d, id.0).scale == Some(a_minor())));
}

#[test]
fn a_joined_clip_takes_the_scale_of_the_first() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(
        add_clip(
            "synth",
            0,
            notes_clip(2 * BAR, BAR, vec![note(0, PPQ, 62, 0.8)]),
        ),
        &reg,
    )
    .unwrap();
    set(&mut d, 1, Some(a_minor()), &reg);
    d.apply(
        Command::Workflow(WorkflowCommand::JoinClips {
            ids: vec![ClipId(1), ClipId(2)],
        }),
        &reg,
    )
    .unwrap();
    let ids = clip_ids(d.doc());
    assert_eq!(ids.len(), 1);
    assert_eq!(clip(&d, ids[0].0).scale, Some(a_minor()));
}

// ---- project format 16 -------------------------------------------------------------------

#[test]
fn a_clip_without_a_scale_writes_no_field() {
    let reg = registry();
    assert!(!save_project(doc(&reg).doc()).contains("\"scale\""));
}

#[test]
fn a_scale_round_trips_canonically_and_names_its_kind() {
    let reg = registry();
    let mut d = doc(&reg);
    set(
        &mut d,
        1,
        Some(Scale::new(11, ScaleKind::PhrygianDominant)),
        &reg,
    );
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], 39);
    let clip = &v["song"]["tracks"][0]["lanes"][0]["clips"][0];
    assert_eq!(
        clip["scale"],
        json!({"root": 11, "kind": "phrygian_dominant"})
    );
    let loaded = load_project(&text, &reg).unwrap();
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text, "canonical");
}

#[test]
fn every_scale_kind_but_chromatic_round_trips() {
    let reg = registry();
    let mut d = doc(&reg);
    for kind in ScaleKind::ALL
        .into_iter()
        .filter(|k| *k != ScaleKind::Chromatic)
    {
        for root in [0u8, 7, 11] {
            set(&mut d, 1, Some(Scale::new(root, kind)), &reg);
            let text = save_project(d.doc());
            let loaded = load_project(&text, &reg).unwrap();
            assert_eq!(
                loaded.song.clip(ClipId(1)).unwrap().2.scale,
                Some(Scale::new(root, kind)),
                "{kind:?} {root}"
            );
        }
    }
}

#[test]
fn a_version_15_project_loads_with_none_and_resaves_as_16() {
    let reg = registry();
    let text = save_project(doc(&reg).doc());
    let old = text.replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 15",
        1,
    );
    let loaded = load_project(&old, &reg).unwrap();
    assert_eq!(loaded.song.clip(ClipId(1)).unwrap().2.scale, None);
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_scale_is_unknown_before_version_16() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, Some(a_minor()), &reg);
    let text = save_project(d.doc()).replacen(
        &format!("\"version\": {PROJECT_VERSION}"),
        "\"version\": 15",
        1,
    );
    let e = load_project(&text, &reg).expect_err("v15 has no clip scales");
    assert!(
        e.to_string().contains("scale") && e.to_string().contains("unknown field"),
        "{e}"
    );
}

#[test]
fn bad_scales_name_their_json_path() {
    let reg = registry();
    let mut d = doc(&reg);
    set(&mut d, 1, Some(a_minor()), &reg);
    let good = save_project(d.doc());
    let base = "song.tracks[0].lanes[0].clips[0].scale";
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(Edit, String, &str)> = vec![
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["scale"]["root"] = json!(12)
            }),
            format!("{base}.root"),
            "",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["scale"]["root"] = json!(-1)
            }),
            format!("{base}.root"),
            "",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["scale"]["kind"] = json!("lydian_b7")
            }),
            format!("{base}.kind"),
            "unknown scale",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["scale"]["kind"] = json!("chromatic")
            }),
            format!("{base}.kind"),
            "chromatic",
        ),
        (
            Box::new(|v| {
                v["song"]["tracks"][0]["lanes"][0]["clips"][0]["scale"]["mode"] = json!("x")
            }),
            base.to_owned(),
            "unknown field",
        ),
        (
            Box::new(|v| v["song"]["tracks"][0]["lanes"][0]["clips"][0]["scale"] = json!("major")),
            base.to_owned(),
            "",
        ),
    ];
    for (edit, path, needle) in cases {
        let mut v: Value = serde_json::from_str(&good).unwrap();
        edit(&mut v);
        let e = load_project(&v.to_string(), &reg).expect_err("must be rejected");
        let ModelError::Load { path: at, message } = &e else {
            panic!("expected a load error, got {e:?}")
        };
        let msg = &format!("{at}: {message}");
        assert!(msg.starts_with(&path), "{msg} (wanted {path})");
        assert!(msg.contains(needle), "{msg} lacks {needle:?}");
    }
}
