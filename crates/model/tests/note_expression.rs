//! Per-note expression carried by note clips: what the checks refuse, the project file (optional
//! and ignorable, canonical round trip, an older project unchanged), the flattening into song
//! time (offset, loop, mute, hiding), what a join keeps, and the commands that clear it.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::song::flatten_expression;
use atomik_model::{
    ClipId, Command, Document, ExpressionPoint, MidiData, NoteExpression, SongCommand,
    WorkflowCommand, load_project, load_project_with_warnings, save_project,
};
use common::song::*;
use common::*;
use serde_json::Value;

fn pt(tick: i64, value: f32) -> ExpressionPoint {
    ExpressionPoint::new(tick, value)
}

/// The expression of the first note (tick 0, key 60) of a clip.
fn bend_entry() -> NoteExpression {
    let mut e = NoteExpression::new(0, 60);
    e.pitch = vec![pt(0, 0.0), pt(PPQ / 2, 2.0)];
    e.pressure = vec![pt(0, 0.25)];
    e.timbre = vec![pt(PPQ / 4, 0.5), pt(PPQ / 2, 0.75)];
    e
}

fn data() -> MidiData {
    MidiData {
        expression: vec![bend_entry()],
        ..Default::default()
    }
}

fn with_data(reg: &Registry, length: i64, midi: MidiData) -> Document {
    let mut d = song_start(reg);
    d.apply(add_track("synth"), reg).unwrap();
    let mut c = notes_clip(0, length, vec![note(0, PPQ, 60, 0.8)]);
    c.midi = midi.into();
    d.apply(add_clip("synth", 0, c), reg).unwrap();
    d
}

fn midi_of(d: &Document, id: u64) -> MidiData {
    (*d.doc().song.clip(ClipId(id)).expect("clip").2.midi).clone()
}

#[test]
fn a_plain_clip_writes_nothing_and_a_clip_with_expression_round_trips() {
    let reg = registry();
    let plain = with_data(&reg, 2 * BAR, MidiData::default());
    let text = save_project(plain.doc());
    assert!(!text.contains("expression"), "{text}");

    let d = with_data(&reg, 2 * BAR, data());
    let text = save_project(d.doc());
    let v: Value = serde_json::from_str(&text).unwrap();
    let clip = &v["song"]["tracks"][0]["lanes"][0]["clips"][0];
    assert_eq!(
        clip["expression"],
        serde_json::json!([[
            0,
            60,
            [[0, 0], [PPQ / 2, 2]],
            [[0, 0.25]],
            [[PPQ / 4, 0.5], [PPQ / 2, 0.75]]
        ]])
    );
    assert_eq!(v["version"], 44, "no format bump");
    let (loaded, warnings) = load_project_with_warnings(&text, &reg).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(&loaded, d.doc());
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn a_file_without_the_field_loads_unchanged_and_its_text_does_not_move() {
    let reg = registry();
    let d = with_data(&reg, 2 * BAR, MidiData::default());
    let text = save_project(d.doc());
    let loaded = load_project(&text, &reg).unwrap();
    assert!(loaded.song.tracks[0].lanes[0].clips[0].midi.is_empty());
    assert_eq!(save_project(&loaded), text);
}

#[test]
fn an_entry_for_a_note_the_clip_no_longer_has_is_not_saved() {
    let reg = registry();
    let mut orphan = bend_entry();
    orphan.tick = PPQ; // no note starts there
    let d = with_data(
        &reg,
        2 * BAR,
        MidiData {
            expression: vec![orphan],
            ..Default::default()
        },
    );
    let text = save_project(d.doc());
    assert!(!text.contains("expression"), "{text}");
}

#[test]
fn bad_data_is_refused_in_a_command_and_named_in_a_file() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("synth"), &reg).unwrap();
    let before = d.doc().clone();
    let mut bad = NoteExpression::new(0, 60);
    bad.pressure = vec![pt(0, 1.5)];
    let mut c = notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8)]);
    c.midi = MidiData {
        expression: vec![bad],
        ..Default::default()
    }
    .into();
    // `canonical` clamps a value, so the clip is accepted with the value held in range.
    d.apply(add_clip("synth", 0, c), &reg).unwrap();
    assert_eq!(midi_of(&d, 1).expression[0].pressure, vec![pt(0, 1.0)]);
    let _ = before;

    let good = with_data(&reg, 2 * BAR, data());
    let text = save_project(good.doc());
    let broken = text.replace("[0, 0.25]", "[0, 7.5]");
    let e = load_project(&broken, &reg).unwrap_err().to_string();
    assert!(e.contains("expression[0][3][0][1]"), "{e}");
    let broken = text.replace("\"expression\": [", "\"expression\": [[1, 2, 3], ");
    assert!(load_project(&broken, &reg).is_err());
}

#[test]
fn flattening_moves_the_curves_to_song_ticks_with_offset_and_loop() {
    let reg = registry();
    let mut d = with_data(&reg, 4 * PPQ, data());
    let flat = flatten_expression(&d.doc().song, &d.doc().song.tracks[0]);
    assert_eq!(flat.len(), 1);
    assert_eq!((flat[0].tick, flat[0].key), (0, 60));
    assert_eq!(flat[0].pitch, vec![(0, 0.0), (PPQ / 2, 2.0)]);
    assert_eq!(flat[0].pressure, vec![(0, 0.25)]);

    // The clip starts later in the song: the curves follow the note.
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id: ClipId(1),
            start: 3 * PPQ,
            length: 4 * PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let flat = flatten_expression(&d.doc().song, &d.doc().song.tracks[0]);
    assert_eq!(flat[0].tick, 3 * PPQ);
    assert_eq!(flat[0].pitch[1], (3 * PPQ + PPQ / 2, 2.0));

    // A loop repeats the note, and its expression with it.
    d.apply(
        sc(SongCommand::SetClipGeometry {
            id: ClipId(1),
            start: 0,
            length: 4 * PPQ,
            offset: 0,
            loop_length: Some(2 * PPQ),
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let flat = flatten_expression(&d.doc().song, &d.doc().song.tracks[0]);
    let ticks: Vec<i64> = flat.iter().map(|e| e.tick).collect();
    assert_eq!(ticks, [0, 2 * PPQ]);

    // A clip over the first repetition hides it.
    d.apply(add_clip("synth", 0, notes_clip(0, 2 * PPQ, vec![])), &reg)
        .unwrap();
    let flat = flatten_expression(&d.doc().song, &d.doc().song.tracks[0]);
    assert_eq!(flat.iter().map(|e| e.tick).collect::<Vec<_>>(), [2 * PPQ]);
}

#[test]
fn a_muted_note_carries_no_expression() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("synth"), &reg).unwrap();
    let mut muted = note(0, PPQ, 60, 0.8);
    muted.muted = true;
    let mut c = notes_clip(0, BAR, vec![muted]);
    c.midi = data().into();
    d.apply(add_clip("synth", 0, c), &reg).unwrap();
    assert!(flatten_expression(&d.doc().song, &d.doc().song.tracks[0]).is_empty());
}

#[test]
fn a_join_keeps_the_expression_of_both_clips() {
    let reg = registry();
    let mut d = with_data(&reg, 2 * PPQ, data());
    let mut second = notes_clip(2 * PPQ, 2 * PPQ, vec![note(0, PPQ, 60, 0.8)]);
    second.midi = data().into();
    d.apply(add_clip("synth", 0, second), &reg).unwrap();
    d.apply(
        Command::Workflow(WorkflowCommand::JoinClips {
            ids: vec![ClipId(1), ClipId(2)],
        }),
        &reg,
    )
    .unwrap();
    let ids = clip_ids(d.doc());
    let m = midi_of(&d, ids[0].0);
    let at: Vec<(i64, u8)> = m.expression.iter().map(|e| (e.tick, e.key)).collect();
    assert_eq!(at, [(0, 60), (2 * PPQ, 60)]);
    let text = save_project(d.doc());
    load_project(&text, &reg).expect("the joined project loads");
}

#[test]
fn clearing_an_entry_is_one_undo_step() {
    let reg = registry();
    let mut d = with_data(&reg, 2 * BAR, data());
    let cleared = midi_of(&d, 1).without_expression(0, 60).expect("an entry");
    d.apply(
        sc(SongCommand::SetClipMidi {
            clip: ClipId(1),
            midi: cleared,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    assert!(midi_of(&d, 1).expression.is_empty());
    d.undo().unwrap();
    assert_eq!(midi_of(&d, 1).expression.len(), 1);
}

#[test]
fn a_split_keeps_the_expression_of_the_notes_each_half_shows() {
    let reg = registry();
    let mut c = notes_clip(
        0,
        4 * PPQ,
        vec![note(0, PPQ, 60, 0.8), note(2 * PPQ, PPQ, 62, 0.8)],
    );
    let mut second = NoteExpression::new(2 * PPQ, 62);
    second.pitch = vec![pt(0, -1.0)];
    c.midi = MidiData {
        expression: vec![bend_entry(), second],
        ..Default::default()
    }
    .into();
    let mut d = song_start(&reg);
    d.apply(add_track("synth"), &reg).unwrap();
    d.apply(add_clip("synth", 0, c), &reg).unwrap();
    d.apply(
        sc(SongCommand::SplitClip {
            id: ClipId(1),
            at: 2 * PPQ,
        }),
        &reg,
    )
    .unwrap();
    // A split keeps the content of both halves (they only show different windows), so each note's
    // expression plays once, from the half that shows it.
    assert_eq!(clip_ids(d.doc()).len(), 2);
    let flat = flatten_expression(&d.doc().song, &d.doc().song.tracks[0]);
    let mut at: Vec<(i64, u8)> = flat.iter().map(|e| (e.tick, e.key)).collect();
    at.sort_unstable();
    assert_eq!(at, [(0, 60), (2 * PPQ, 62)]);
    assert_eq!(
        flat.iter().find(|e| e.key == 62).unwrap().pitch,
        vec![(2 * PPQ, -1.0)]
    );
}

#[test]
fn layering_a_take_keeps_the_old_expression_and_a_replaced_note_takes_the_new_one() {
    use atomik_model::layered_clip;
    let mut old = notes_clip(
        0,
        2 * BAR,
        vec![note(0, PPQ, 60, 0.8), note(PPQ, PPQ, 62, 0.8)],
    );
    let mut e60 = NoteExpression::new(0, 60);
    e60.pitch = vec![pt(0, 1.0)];
    let mut e62 = NoteExpression::new(PPQ, 62);
    e62.pitch = vec![pt(0, -1.0)];
    old.midi = MidiData {
        expression: vec![e60, e62.clone()],
        ..Default::default()
    }
    .into();
    // The take plays key 60 again at its start (replacing the old note, with other curves and one
    // without) and a new note on 64.
    let mut take = notes_clip(
        0,
        BAR,
        vec![note(0, PPQ, 60, 0.9), note(2 * PPQ, PPQ, 64, 0.9)],
    );
    let mut t64 = NoteExpression::new(2 * PPQ, 64);
    t64.pressure = vec![pt(0, 0.5)];
    take.midi = MidiData {
        expression: vec![t64.clone()],
        ..Default::default()
    }
    .into();
    let merged = layered_clip(&old, &take).unwrap();
    let m = &merged.midi;
    assert!(
        m.expression_of(0, 60).is_none(),
        "the replaced note has none of its own"
    );
    assert_eq!(m.expression_of(PPQ, 62), Some(&e62));
    assert_eq!(m.expression_of(2 * PPQ, 64), Some(&t64));
}
