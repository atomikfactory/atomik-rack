//! Find Repetition in the app core: the search lists the ideas with their track, place and three
//! suggestions, a suggestion that is an edit is one undo step that says what it did and is marked
//! done, advice never changes the song, and locating moves the playhead to the start of the run.

use super::*;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::song::flatten_track_notes;
use atomik_model::{
    Clip, ClipContent, CreateOptions, CreateSource, Note, WorkflowCommand, new_project_doc,
};

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn pattern() -> Vec<Note> {
    vec![
        Note::new(0, PPQ / 2, 36, 0.9),
        Note::new(PPQ, PPQ / 2, 38, 0.8),
        Note::new(2 * PPQ, PPQ / 2, 36, 0.9),
        Note::new(3 * PPQ, PPQ / 2, 38, 0.7),
    ]
}

fn clip(start: i64, looped: bool) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length: if looped { 4 * BAR } else { BAR },
        offset: 0,
        loop_length: looped.then_some(BAR),
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(pattern().into()),
    }
}

/// A synth `drums` that plays the pattern in four clips starting at bar 2.
fn core(looped: bool) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(SYNTH.into()),
        key: Some(key("drums")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    let clips: Vec<Clip> = if looped {
        vec![clip(BAR, true)]
    } else {
        (1..5).map(|b| clip(b * BAR, false)).collect()
    };
    for clip in clips {
        c.apply_command(Command::Song(SongCommand::AddClip {
            device: key("drums"),
            lane: 0,
            clip,
        }))
        .unwrap();
    }
    c
}

fn played(c: &AppCore<RecordingSink>) -> usize {
    let song = &c.doc().song;
    flatten_track_notes(song, song.track(&key("drums")).unwrap()).len()
}

#[test]
fn nothing_is_listed_before_the_first_search() {
    let c = core(false);
    assert!(!c.repetition_searched());
    assert!(c.repetition_data().is_none());
}

#[test]
fn the_search_lists_the_idea_with_track_place_and_three_suggestions() {
    let mut c = core(false);
    let before = c.doc().clone();
    assert_eq!(c.find_repetition(), 1);
    assert_eq!(*c.doc(), before, "searching never changes the song");
    let data = c.repetition_data().expect("searched");
    assert_eq!(data.rows.len(), 1);
    let row = &data.rows[0];
    assert_eq!(row.what, "a 1-bar pattern");
    assert_eq!((row.repeats, row.bars.as_str()), (4, "bars 2-5"));
    assert!(!row.track.is_empty());
    let labels: Vec<_> = row.choices.iter().map(|k| k.label.as_str()).collect();
    assert_eq!(labels, ["Drop a hit", "Filter the last bar", "Cut a beat"]);
    assert!(row.choices.iter().all(|k| !k.done));
    assert!(row.choices[0].can_apply && row.choices[2].can_apply);
}

#[test]
fn a_song_without_repeats_lists_nothing_but_was_searched() {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    assert_eq!(c.find_repetition(), 0);
    assert!(c.repetition_searched());
    assert!(c.repetition_data().unwrap().rows.is_empty());
}

#[test]
fn dropping_a_hit_is_one_undo_step_that_says_what_it_did_and_is_marked_done() {
    let mut c = core(false);
    c.find_repetition();
    assert_eq!(played(&c), 16);
    c.apply_repetition(0, 0)
        .expect("a plain clip can be edited");
    assert_eq!(played(&c), 15);
    let said = c.last_rejection().unwrap_or("");
    assert!(
        said.contains("bars 2-5") && said.contains("mute the last hit"),
        "{said}"
    );
    // The song was searched again: the run is three repeats and a changed bar now, and the
    // variation shows as made for the idea at the same start.
    let data = c.repetition_data().unwrap();
    assert_eq!(data.rows[0].repeats, 3);
    assert!(data.rows[0].choices[0].done);
    assert!(!data.rows[0].choices[1].done);
    // One undo brings the hit back.
    assert!(c.undo());
    assert_eq!(played(&c), 16);
}

#[test]
fn the_filter_variation_adds_the_sweep_on_the_channel_in_one_step() {
    let mut c = core(false);
    c.find_repetition();
    let depth = c.doc().song.tracks.len();
    c.apply_repetition(0, 1).expect("the track has a channel");
    assert_eq!(
        c.doc().song.tracks.len(),
        depth + 1,
        "an automation track for the strip"
    );
    assert!(c.undo());
    assert_eq!(c.doc().song.tracks.len(), depth);
}

#[test]
fn advice_never_changes_the_song() {
    let mut c = core(true);
    c.find_repetition();
    let data = c.repetition_data().unwrap();
    assert!(
        !data.rows[0].choices[0].can_apply,
        "a looped clip cannot take a plain edit"
    );
    let before = c.doc().clone();
    let why = c.apply_repetition(0, 0).unwrap_err();
    assert!(!why.is_empty());
    assert_eq!(*c.doc(), before);
    assert_eq!(c.last_rejection(), Some(why.as_str()));
    // An idea or a suggestion that is not there is refused.
    assert!(c.apply_repetition(7, 0).is_err());
    assert!(c.apply_repetition(0, 9).is_err());
}

#[test]
fn locating_moves_the_playhead_to_the_start_of_the_run() {
    let mut c = core(false);
    c.find_repetition();
    c.locate_repetition(0);
    assert_eq!(c.position(), BAR);
    c.locate_repetition(5);
    assert_eq!(c.position(), BAR, "an idea that is not there does nothing");
}

#[test]
fn a_new_project_forgets_the_search() {
    let mut c = core(false);
    c.find_repetition();
    c.apply_repetition(0, 0).unwrap();
    c.new_document();
    assert!(!c.repetition_searched());
    assert!(c.repetition_data().is_none());
}
