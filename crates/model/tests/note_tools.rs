//! Note tools: `ApplyNoteTool` over clips and selections, one undo step, audio skipped, and
//! the same seed giving the same notes in two separate documents.

#![allow(clippy::float_cmp)]

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    ClipContent, ClipId, Command, Document, ModelError, Note, NoteTool, SongCommand, SyncOp,
    save_project,
};
use common::song::*;
use common::*;

const S: i64 = PPQ / 4;

fn doc(reg: &Registry) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("lead"), reg).unwrap();
    d.apply(add_track("synth"), reg).unwrap();
    let many: Vec<Note> = (0..16)
        .map(|i| note(i * S, S, 48 + (i % 5) as u8 * 2, 0.5))
        .collect();
    d.apply(add_clip("lead", 0, notes_clip(0, 4 * BAR, many)), reg)
        .unwrap();
    d.apply(
        add_clip(
            "synth",
            0,
            notes_clip(BAR, BAR, vec![note(0, S, 64, 0.5), note(S, S, 65, 0.5)]),
        ),
        reg,
    )
    .unwrap();
    d
}

fn notes_of(d: &Document, id: u64) -> Vec<Note> {
    let (_, _, c) = d.doc().song.clip(ClipId(id)).unwrap();
    match &c.content {
        ClipContent::Notes(n) => n.to_vec(),
        _ => panic!("not notes"),
    }
}

fn tool(clips: &[u64], selection: Option<Vec<usize>>, tool: NoteTool) -> Command {
    sc(SongCommand::ApplyNoteTool {
        clips: clips.iter().map(|&i| ClipId(i)).collect(),
        selection,
        tool,
    })
}

#[test]
fn a_tool_edits_many_clips_in_one_undo_step() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = save_project(d.doc());
    let ops = d
        .apply(tool(&[1, 2], None, NoteTool::SetVelocity(0.9)), &reg)
        .unwrap();
    assert_eq!(ops, [SyncOp::SongChanged]);
    assert!(notes_of(&d, 1).iter().all(|n| n.velocity == 0.9));
    assert!(notes_of(&d, 2).iter().all(|n| n.velocity == 0.9));
    assert!(d.undo(&reg).is_some());
    assert_eq!(save_project(d.doc()), before);
    d.redo(&reg).unwrap();
    assert_eq!(notes_of(&d, 2)[0].velocity, 0.9);
}

#[test]
fn a_selection_limits_the_edit() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(
        tool(&[1], Some(vec![0, 3]), NoteTool::SetVelocity(1.0)),
        &reg,
    )
    .unwrap();
    let n = notes_of(&d, 1);
    assert_eq!(n[0].velocity, 1.0);
    assert_eq!(n[1].velocity, 0.5);
    assert_eq!(n[3].velocity, 1.0);
}

#[test]
fn bad_requests_change_nothing() {
    let reg = registry();
    let mut d = doc(&reg);
    let before = save_project(d.doc());
    for cmd in [
        tool(&[1, 2], Some(vec![0]), NoteTool::SetVelocity(1.0)),
        tool(&[1], Some(vec![99]), NoteTool::SetVelocity(1.0)),
        tool(&[1], None, NoteTool::SetVelocity(2.0)),
        tool(&[1], None, NoteTool::SetVelocity(0.5)),
        tool(&[77], None, NoteTool::SetVelocity(0.9)),
    ] {
        let e: Result<_, ModelError> = d.apply(cmd, &reg);
        assert!(e.is_err(), "{e:?}");
        assert_eq!(save_project(d.doc()), before);
    }
    assert!(!d.can_undo() || d.undo_label() != Some("Set Velocity"));
}

#[test]
fn the_same_seed_gives_the_same_notes_in_another_document() {
    let reg = registry();
    let run = |seed: u32| {
        let mut d = doc(&reg);
        for t in [
            NoteTool::RandomizeVelocity { amount: 0.5, seed },
            NoteTool::AlterNotes { chance: 0.6, seed },
            NoteTool::RandomizePitch { range: 7, seed },
        ] {
            d.apply(tool(&[1], None, t), &reg).unwrap();
        }
        notes_of(&d, 1)
    };
    assert_eq!(run(5), run(5));
    assert_ne!(run(5), run(6));
}

#[test]
fn labels_name_the_tool() {
    let reg = registry();
    let mut d = doc(&reg);
    d.apply(tool(&[1], None, NoteTool::ScaleVelocity(0.5)), &reg)
        .unwrap();
    assert_eq!(d.undo_label(), Some("Scale Velocity"));
}

#[test]
fn audio_clips_are_skipped_and_alone_are_an_error() {
    use common::audio::{add_audio_clip, add_audio_track, audio_registry_no_console, register};
    let reg = audio_registry_no_console();
    let mut d = doc(&reg);
    d.apply(register(1, 480_000), &reg).unwrap();
    d.apply(add_audio_track(), &reg).unwrap();
    d.apply(add_audio_clip("audio-track", 0, BAR, 1), &reg)
        .unwrap();
    let audio = d.last_clip_id().unwrap().0;
    let before_audio = save_project(d.doc());
    // Mixed with a note clip: the notes change, the audio clip is left alone.
    d.apply(tool(&[1, audio], None, NoteTool::SetVelocity(0.9)), &reg)
        .unwrap();
    assert!(notes_of(&d, 1).iter().all(|n| n.velocity == 0.9));
    d.undo(&reg).unwrap();
    assert_eq!(save_project(d.doc()), before_audio);
    // Audio alone changes nothing, so the command is refused.
    assert!(
        d.apply(tool(&[audio], None, NoteTool::SetVelocity(0.9)), &reg)
            .is_err()
    );
    assert_eq!(save_project(d.doc()), before_audio);
}
