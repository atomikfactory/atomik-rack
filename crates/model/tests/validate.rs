//! `RackDoc::validate` and `save_project_checked`: the document invariants are checked, so a
//! document that breaks one is an error before a file is written, not a file that cannot be read.

mod common;

use atomik_model::{ModelError, RackDoc, SongCommand, empty_doc, save_project_checked};
use common::song::*;
use common::*;

fn song_doc() -> RackDoc {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    d.apply(
        add_clip(
            "lead",
            0,
            notes_clip(0, BAR, vec![note(0, PPQ, 60, 0.8), note(PPQ, PPQ, 62, 0.5)]),
        ),
        &reg,
    )
    .unwrap();
    d.doc().clone()
}

fn first_violation(doc: &RackDoc) -> String {
    doc.validate().unwrap_err()[0].to_string()
}

#[test]
fn a_document_built_by_commands_is_valid() {
    song_doc().validate().unwrap();
    empty_doc().validate().unwrap();
    atomik_model::new_project_doc().validate().unwrap();
}

#[test]
fn a_nan_in_a_stored_param_is_refused_before_the_file_is_written() {
    let mut doc = song_doc();
    let device = doc.devices.iter_mut().find(|d| d.key.0 == "lead").unwrap();
    device.params.insert("volume".into(), f32::NAN);
    let text = first_violation(&doc);
    assert!(
        text.contains("lead") && text.contains("not a finite"),
        "{text}"
    );
    let err = save_project_checked(&doc).unwrap_err();
    assert!(matches!(err, ModelError::Invalid(_)), "{err:?}");
}

#[test]
fn an_infinite_tempo_or_a_nan_note_velocity_is_refused() {
    let mut doc = song_doc();
    doc.tempo_bpm = f64::INFINITY;
    doc.song_mut().tempo[0].bpm = f64::INFINITY;
    assert!(first_violation(&doc).contains("finite"));

    let mut doc = song_doc();
    let lane = &mut doc.song_mut().tracks[0].lanes[0];
    if let atomik_model::ClipContent::Notes(notes) = &mut lane.clips[0].content {
        notes[0].velocity = f32::NAN;
    }
    assert!(first_violation(&doc).contains("velocity"));
}

#[test]
fn the_tempo_mirror_is_checked() {
    let mut doc = song_doc();
    doc.song_mut().tempo[0].bpm += 1.0;
    assert!(first_violation(&doc).contains("rack tempo"));
    doc.song_mut().tempo.clear();
    assert!(first_violation(&doc).contains("no event"));
}

#[test]
fn a_clip_or_note_past_the_last_tick_is_refused() {
    let mut doc = song_doc();
    doc.song_mut().tracks[0].lanes[0].clips[0].length = atomik_model::MAX_TICK + 1;
    assert!(first_violation(&doc).contains("clips[0]"));

    let mut doc = song_doc();
    if let atomik_model::ClipContent::Notes(notes) =
        &mut doc.song_mut().tracks[0].lanes[0].clips[0].content
    {
        notes[1].tick = atomik_model::MAX_TICK + 1;
    }
    assert!(first_violation(&doc).contains("notes[1]"));
}

#[test]
fn a_lane_out_of_order_is_refused() {
    let reg = registry();
    let mut d = song_start(&reg);
    d.apply(add_track("lead"), &reg).unwrap();
    for start in [0, BAR] {
        d.apply(
            add_clip("lead", 0, notes_clip(start, BAR, Vec::new())),
            &reg,
        )
        .unwrap();
    }
    let mut doc = d.doc().clone();
    doc.song_mut().tracks[0].lanes[0].clips.reverse();
    assert!(first_violation(&doc).contains("sorted"));
}

#[test]
fn the_console_may_only_name_devices_the_rack_has() {
    let reg = console_registry();
    let mut d = atomik_model::Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    let mut doc = d.doc().clone();
    doc.validate().unwrap();
    assert!(
        !doc.console.channels.is_empty(),
        "the instrument got a strip"
    );
    doc.console.channels[0].source = Some(key("ghost"));
    assert!(first_violation(&doc).contains("ghost"));
    doc.console.channels[0].source = None;
    doc.console.master = Some(key("nowhere"));
    assert!(first_violation(&doc).contains("nowhere"));
}

#[test]
fn the_column_order_is_checked() {
    let reg = registry();
    let mut d = three_devices(&reg);
    d.apply(common::song::sc(SongCommand::SetSongEnd(BAR)), &reg)
        .unwrap();
    let mut doc = d.doc().clone();
    doc.validate().unwrap();
    doc.devices[0].column = 3;
    assert!(first_violation(&doc).contains("column"));
}

#[test]
fn a_saved_project_of_a_valid_document_is_what_save_project_writes() {
    let doc = song_doc();
    assert_eq!(
        save_project_checked(&doc).unwrap(),
        atomik_model::save_project(&doc)
    );
}
