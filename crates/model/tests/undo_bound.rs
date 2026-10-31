//! The undo history is bounded by step count and by an estimate of the memory it holds, the
//! assets of forgotten steps leave the pool at the next prune, and a copy of a document shares
//! the notes of its clips until one of the copies changes them.

mod common;

use atomik_model::{
    ClipContent, Command, Document, MAX_UNDO_BYTES, MAX_UNDO_STEPS, Note, NoteList, SongCommand,
    empty_doc,
};
use common::audio::*;
use common::song::*;
use common::*;

fn set_level(d: &mut Document, reg: &atomik_device_sdk::Registry, key: &str, value: f32) {
    d.apply(
        Command::SetParam {
            key: common::key(key),
            param: "master.level".into(),
            value,
            merge: None,
        },
        reg,
    )
    .unwrap();
}

fn level(d: &Document, key: &str) -> f32 {
    d.doc().device(&common::key(key)).unwrap().params["master.level"]
}

#[test]
fn the_history_keeps_the_latest_steps_and_forgets_the_oldest() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("lead")), &reg).unwrap();
    let steps = MAX_UNDO_STEPS + 200;
    for i in 0..steps {
        set_level(&mut d, &reg, "lead", -40.0 + (i % 30) as f32);
    }
    assert_eq!(d.undo_depth(), MAX_UNDO_STEPS);
    let mut undone = 0;
    while d.undo(&reg).is_some() {
        undone += 1;
    }
    assert_eq!(undone, MAX_UNDO_STEPS);
    // The oldest 201 steps (the device and 200 levels) are gone: the document went back to
    // the state after the 200th level.
    assert!(d.doc().device(&common::key("lead")).is_some());
    assert!((level(&d, "lead") - (-40.0 + ((200 - 1) % 30) as f32)).abs() < 1e-6);
    // Redo walks the whole way forward again.
    let mut redone = 0;
    while d.redo(&reg).is_some() {
        redone += 1;
    }
    assert_eq!(redone, MAX_UNDO_STEPS);
    assert!((level(&d, "lead") - (-40.0 + ((steps - 1) % 30) as f32)).abs() < 1e-6);
}

#[test]
fn a_memory_limit_forgets_old_steps_but_always_keeps_the_latest() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("lead")), &reg).unwrap();
    d.apply(
        sc(SongCommand::AddTrack {
            device: key("lead"),
        }),
        &reg,
    )
    .unwrap();
    // A clip of 2 000 notes is about 64 KB of history for the step that adds it.
    d.set_undo_limits(MAX_UNDO_STEPS, 300_000);
    let notes: Vec<Note> = (0..2000).map(|n| note(n * 10, 5, 60, 0.5)).collect();
    for i in 0..30 {
        d.apply(
            add_clip("lead", 0, notes_clip(i * BAR, BAR, notes.clone())),
            &reg,
        )
        .unwrap();
    }
    let kept = d.undo_depth();
    assert!((2..15).contains(&kept), "kept {kept} of 32 steps");
    let mut undone = 0;
    while d.undo(&reg).is_some() {
        undone += 1;
    }
    assert_eq!(undone, kept);
    assert_eq!(clip_ids(d.doc()).len(), 30 - kept.min(30));

    // A step bigger than the whole limit is still kept (and undoable).
    d.set_undo_limits(MAX_UNDO_STEPS, 1000);
    d.apply(add_clip("lead", 0, notes_clip(40 * BAR, BAR, notes)), &reg)
        .unwrap();
    assert_eq!(d.undo_depth(), 1);
    assert!(d.undo(&reg).is_some());
}

#[test]
fn lowering_the_limit_forgets_at_once_and_a_merged_drag_counts_as_one_step() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("lead")), &reg).unwrap();
    for i in 0..10 {
        set_level(&mut d, &reg, "lead", -30.0 + i as f32);
    }
    assert_eq!(d.undo_depth(), 11);
    d.set_undo_limits(4, MAX_UNDO_BYTES);
    assert_eq!(d.undo_depth(), 4);
    for i in 0..50 {
        d.apply(
            Command::SetParam {
                key: key("lead"),
                param: "master.level".into(),
                value: -20.0 + i as f32 * 0.1,
                merge: Some(1),
            },
            &reg,
        )
        .unwrap();
    }
    assert_eq!(
        d.undo_depth(),
        4,
        "the drag is one step, the oldest was forgotten"
    );
}

#[test]
fn the_assets_of_forgotten_steps_leave_the_pool_at_the_next_prune() {
    let reg = audio_registry();
    let mut d = with_audio_track(&reg);
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let id = last(&d);
    d.apply(sc(SongCommand::RemoveClip { id }), &reg).unwrap();
    d.prune_unreferenced_assets();
    assert!(
        d.doc().assets.contains_key(&hash(1)),
        "the history can still bring the clip back"
    );
    d.set_undo_limits(2, MAX_UNDO_BYTES);
    d.apply(
        Command::SetParam {
            key: key("audio-track"),
            param: "monitor".into(),
            value: 2.0,
            merge: None,
        },
        &reg,
    )
    .unwrap();
    d.apply(
        Command::SetParam {
            key: key("audio-track"),
            param: "monitor".into(),
            value: 0.0,
            merge: None,
        },
        &reg,
    )
    .unwrap();
    assert!(d.referenced_assets().is_empty());
    d.prune_unreferenced_assets();
    assert!(
        d.doc().assets.is_empty(),
        "nothing references the assets any more"
    );
}

#[test]
fn a_copy_of_a_document_shares_the_notes_until_one_copy_changes_them() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("lead")), &reg).unwrap();
    d.apply(
        sc(SongCommand::AddTrack {
            device: key("lead"),
        }),
        &reg,
    )
    .unwrap();
    let notes: Vec<Note> = (0..100).map(|n| note(n * 10, 5, 60, 0.5)).collect();
    d.apply(add_clip("lead", 0, notes_clip(0, BAR, notes)), &reg)
        .unwrap();

    let original = d.doc().clone();
    let mut copy = original.clone();
    let list = |doc: &atomik_model::RackDoc| -> NoteList {
        match &doc.song.tracks[0].lanes[0].clips[0].content {
            ClipContent::Notes(n) => n.clone(),
            _ => panic!("a note clip"),
        }
    };
    assert!(
        NoteList::ptr_eq(&list(&original), &list(&copy)),
        "no note was copied"
    );
    assert!(std::sync::Arc::ptr_eq(&original.song, &copy.song));

    if let ClipContent::Notes(n) = &mut copy.song_mut().tracks[0].lanes[0].clips[0].content {
        n.push(note(2000, 5, 61, 0.5));
    }
    assert_eq!(list(&copy).len(), 101);
    assert_eq!(
        list(&original).len(),
        100,
        "the original is not changed by the copy"
    );
    assert_eq!(list(d.doc()).len(), 100);
}

#[test]
fn a_cancelled_trial_does_not_push_the_oldest_step_out_of_a_full_history() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("lead")), &reg).unwrap();
    d.set_undo_limits(5, MAX_UNDO_BYTES);
    for i in 0..5 {
        set_level(&mut d, &reg, "lead", -30.0 + i as f32);
    }
    assert_eq!(d.undo_depth(), 5);
    for v in [-10.0, -11.0] {
        d.apply_trial(
            Command::SetParam {
                key: key("lead"),
                param: "master.level".into(),
                value: v,
                merge: None,
            },
            &reg,
        )
        .unwrap();
    }
    assert_eq!(d.undo_depth(), 5, "a trial step is not a kept step");
    d.cancel_trial().unwrap();
    assert_eq!(d.undo_depth(), 5);
    let mut undone = 0;
    while d.undo(&reg).is_some() {
        undone += 1;
    }
    assert_eq!(undone, 5);
    // Every one of the five steps, the oldest included, undid: the level is back to its default.
    let lead = d.doc().device(&key("lead")).unwrap();
    assert!(!lead.params.contains_key("master.level"));
}

#[test]
fn a_committed_trial_is_trimmed_like_any_step() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("lead")), &reg).unwrap();
    d.set_undo_limits(3, MAX_UNDO_BYTES);
    for i in 0..3 {
        set_level(&mut d, &reg, "lead", -30.0 + i as f32);
    }
    d.apply_trial(
        Command::SetParam {
            key: key("lead"),
            param: "master.level".into(),
            value: -5.0,
            merge: None,
        },
        &reg,
    )
    .unwrap();
    d.commit_trial();
    assert_eq!(d.undo_depth(), 3);
}
