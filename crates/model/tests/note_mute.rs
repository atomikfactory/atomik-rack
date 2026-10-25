//! Note mute: the `muted` flag on notes, project format v10 (the optional fifth element of a
//! note), playback flattening and `WorkflowCommand::SetClipsMuted`.

mod common;

use atomik_model::song::flatten_notes;
use atomik_model::{
    ClipContent, ClipId, Curve, Document, ModelError, Note, PROJECT_VERSION, SongCommand, SyncOp,
    WorkflowCommand, empty_doc, load_project, save_project,
};
use common::song::*;
use common::workflow::*;
use common::*;

fn muted(tick: i64, key: u8) -> Note {
    Note {
        muted: true,
        ..Note::new(tick, PPQ, key, 0.7)
    }
}

/// One synth track with one note clip holding a muted and an unmuted note.
fn doc_with_muted_note(r: &atomik_device_sdk::Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("synth")),
            add_track("synth"),
            add_clip(
                "synth",
                0,
                notes_clip(0, BAR, vec![muted(0, 60), note(PPQ, PPQ, 64, 0.6)]),
            ),
        ],
    );
    d
}

fn all_notes(doc: &atomik_model::RackDoc) -> Vec<Note> {
    doc.song
        .tracks
        .iter()
        .flat_map(|t| &t.lanes)
        .flat_map(|l| &l.clips)
        .filter_map(|c| match &c.content {
            ClipContent::Notes(n) => Some(n.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

// ---- model ------------------------------------------------------------------------------

#[test]
fn a_new_note_is_unmuted() {
    assert!(!Note::new(0, 1, 60, 0.5).muted);
}

// ---- playback ---------------------------------------------------------------------------

#[test]
fn flatten_notes_skips_muted_notes() {
    let r = registry();
    let d = doc_with_muted_note(&r);
    let flat = flatten_notes(&d.doc().song.tracks[0]);
    assert_eq!(flat, vec![(PPQ, PPQ, 64, 0.6)]);
}

// ---- format v10 -------------------------------------------------------------------------

#[test]
fn the_project_version_is_eleven() {
    assert_eq!(PROJECT_VERSION, 27);
}

#[test]
fn only_muted_notes_get_a_fifth_element_and_it_round_trips_byte_for_byte() {
    let r = registry();
    let d = doc_with_muted_note(&r);
    let text = save_project(d.doc());
    assert!(text.contains("\"version\": 27"));
    assert!(
        text.contains(&format!("[0, {PPQ}, 60, 0.7, true]")),
        "{text}"
    );
    assert!(text.contains(&format!("[{PPQ}, {PPQ}, 64, 0.6]")), "{text}");
    assert_eq!(text.matches("true]").count(), 1);
    let back = load_project(&text, &r).unwrap();
    assert_eq!(all_notes(&back), all_notes(d.doc()));
    assert_eq!(save_project(&back), text);
}

#[test]
fn a_v10_note_may_say_false_but_the_fifth_element_must_be_a_boolean() {
    let r = registry();
    let text = save_project(doc_with_muted_note(&r).doc());
    let explicit = text.replace("0.6]", "0.6, false]");
    let back = load_project(&explicit, &r).unwrap();
    assert!(all_notes(&back).iter().any(|n| !n.muted && n.key == 64));
    for bad in ["1", "\"true\"", "null"] {
        let t = text.replace("0.7, true]", &format!("0.7, {bad}]"));
        let e = load_project(&t, &r).unwrap_err().to_string();
        assert!(e.contains("muted") || e.contains("boolean"), "{bad}: {e}");
    }
    let six = text.replace("0.7, true]", "0.7, true, true]");
    assert!(load_project(&six, &r).is_err());
}

#[test]
fn versions_before_ten_refuse_a_five_element_note_and_name_the_version() {
    let r = registry();
    let text = save_project(doc_with_muted_note(&r).doc());
    for v in 3..=9 {
        let old = text.replace("\"version\": 27", &format!("\"version\": {v}"));
        let e = load_project(&old, &r).unwrap_err().to_string();
        assert!(e.contains(&format!("version {v}")), "v{v}: {e}");
    }
}

#[test]
fn a_v9_project_loads_with_every_note_unmuted_and_saves_as_v11() {
    let r = console_registry();
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo-v9.atomik");
    let file = std::fs::read_to_string(path).unwrap();
    assert!(file.contains("\"version\": 9"));
    let doc = load_project(&file, &r).unwrap();
    let notes = all_notes(&doc);
    assert!(!notes.is_empty());
    assert!(notes.iter().all(|n| !n.muted));
    let saved = save_project(&doc);
    assert_eq!(saved, as_current(&file, 9));
}

#[test]
fn a_version_above_the_current_one_is_refused() {
    let r = registry();
    let text = save_project(&empty_doc()).replace("\"version\": 27", "\"version\": 28");
    let e = load_project(&text, &r).unwrap_err().to_string();
    assert!(e.contains("unsupported version 28"), "{e}");
}

// ---- SetClipsMuted ----------------------------------------------------------------------

fn clip_muted(d: &Document, id: ClipId) -> bool {
    d.doc().song.clip(id).unwrap().2.muted
}

/// Two note clips and an automation clip.
fn mixed(r: &atomik_device_sdk::Registry) -> (Document, Vec<ClipId>) {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, SYNTH, Some("synth")),
            add_track("synth"),
            add_clip("synth", 0, notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)])),
            add_clip(
                "synth",
                0,
                notes_clip(BAR, BAR, vec![note(0, PPQ, 62, 0.8)]),
            ),
            sc(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "filter.cutoff".into(),
            }),
            add_clip(
                "synth",
                1,
                auto_clip(0, BAR, vec![point(0, 0.5, Curve::Linear)]),
            ),
        ],
    );
    let ids = clip_ids(d.doc());
    (d, ids)
}

#[test]
fn set_clips_muted_is_one_undo_step_over_every_clip_kind() {
    let r = registry();
    let (mut d, ids) = mixed(&r);
    assert_eq!(ids.len(), 3);
    let ops = one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::SetClipsMuted {
            ids: ids.clone(),
            muted: true,
        }),
    );
    assert_eq!(ops, [SyncOp::SongChanged]);
    assert!(ids.iter().all(|&i| clip_muted(&d, i)));
    d.undo(&r).unwrap();
    assert!(ids.iter().all(|&i| !clip_muted(&d, i)));
    d.redo(&r).unwrap();
    assert!(ids.iter().all(|&i| clip_muted(&d, i)));
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::SetClipsMuted {
            ids: ids.clone(),
            muted: false,
        }),
    );
    assert!(ids.iter().all(|&i| !clip_muted(&d, i)));
}

#[test]
fn set_clips_muted_changes_only_the_listed_clips_and_skips_those_already_set() {
    let r = registry();
    let (mut d, ids) = mixed(&r);
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::SetClipsMuted {
            ids: vec![ids[0]],
            muted: true,
        })],
    );
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::SetClipsMuted {
            ids: vec![ids[0], ids[1]],
            muted: true,
        }),
    );
    assert!(clip_muted(&d, ids[0]) && clip_muted(&d, ids[1]) && !clip_muted(&d, ids[2]));
}

#[test]
fn set_clips_muted_that_changes_nothing_or_names_an_unknown_clip_is_rejected_without_history() {
    let r = registry();
    let (mut d, ids) = mixed(&r);
    let nothing = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::SetClipsMuted {
            ids: ids.clone(),
            muted: false,
        }),
    );
    assert!(matches!(nothing, ModelError::BadSong(_)), "{nothing:?}");
    let bad = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::SetClipsMuted {
            ids: vec![ids[0], ClipId(999)],
            muted: true,
        }),
    );
    assert!(matches!(bad, ModelError::UnknownKey(_)), "{bad:?}");
    let none = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::SetClipsMuted {
            ids: vec![],
            muted: true,
        }),
    );
    assert!(matches!(none, ModelError::BadSong(_)), "{none:?}");
}
