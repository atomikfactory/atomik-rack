//! The song key and the chord lane in the app core: one undo step per change with a status line,
//! Detect Song Key on a C major and an A minor song, and the key the browser filters by.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::PPQ;
use atomik_model::harmony::{ChordEvent, Harmony, Key};
use atomik_model::theory::{ChordKind, ScaleKind};
use atomik_model::{
    Clip, ClipContent, CreateOptions, CreateSource, Note, WorkflowCommand, new_project_doc,
};

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn clip(start: i64, notes: Vec<Note>) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length: 2 * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(notes.into()),
    }
}

/// A song with one synth track and no clips.
fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(SYNTH.into()),
        key: Some(key("lead")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    c
}

/// Puts a clip playing the keys `semitones` above `root` (one beat each, tonic and fifth longer).
fn play(c: &mut AppCore<RecordingSink>, root: u8, semitones: &[u8]) {
    let notes: Vec<Note> = semitones
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let long = *s == 0 || *s == 7;
            let len = if long { 2 * PPQ } else { PPQ };
            Note::new(i as i64 * 2 * PPQ, len, 60 + root + s, 0.8)
        })
        .collect();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: clip(0, notes),
    }))
    .unwrap();
}

const MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR: [u8; 7] = [0, 2, 3, 5, 7, 8, 10];

#[test]
fn setting_the_key_is_one_undo_step_with_a_status_line() {
    let mut c = core();
    let before = c.document.history_len();
    c.set_song_key(Some(Key::new(9, ScaleKind::NaturalMinor)))
        .unwrap();
    assert_eq!(c.document.history_len(), before + 1);
    assert_eq!(c.harmony().key, Some(Key::new(9, ScaleKind::NaturalMinor)));
    assert_eq!(c.last_rejection(), Some("Song key: A minor"));
    c.set_song_key(None).unwrap();
    assert_eq!(c.last_rejection(), Some("Song key cleared"));
    c.undo();
    c.undo();
    assert!(c.harmony().is_empty());
}

#[test]
fn a_harmony_the_model_refuses_leaves_the_song_alone_and_says_why() {
    let mut c = core();
    let mut h = Harmony::default();
    h.chords.push(ChordEvent::new(0, BAR, 0, ChordKind::Major));
    h.chords
        .push(ChordEvent::new(BAR / 2, BAR, 5, ChordKind::Major));
    assert!(c.set_harmony(h).is_err());
    assert!(c.harmony().is_empty());
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Cannot change the key and chords"))
    );
}

#[test]
fn detecting_a_c_major_song_sets_c_major_in_one_undo_step() {
    let mut c = core();
    play(&mut c, 0, &MAJOR);
    let before = c.document.history_len();
    assert_eq!(c.detect_song_key(), Some(Key::new(0, ScaleKind::Major)));
    assert_eq!(c.document.history_len(), before + 1);
    assert_eq!(c.harmony().key, Some(Key::new(0, ScaleKind::Major)));
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Song key set to C major (a clear fit)")),
        "{:?}",
        c.last_rejection()
    );
    c.undo();
    assert_eq!(c.harmony().key, None);
}

#[test]
fn detecting_an_a_minor_song_sets_a_minor() {
    let mut c = core();
    play(&mut c, 9, &MINOR);
    assert_eq!(
        c.detect_song_key(),
        Some(Key::new(9, ScaleKind::NaturalMinor))
    );
}

#[test]
fn detecting_with_no_notes_or_the_same_key_changes_nothing() {
    let mut c = core();
    let before = c.document.history_len();
    assert_eq!(c.detect_song_key(), None);
    assert_eq!(c.last_rejection(), Some("No notes to read a key from"));
    play(&mut c, 0, &MAJOR);
    c.set_song_key(Some(Key::new(0, ScaleKind::Major))).unwrap();
    let steps = c.document.history_len();
    assert_eq!(c.detect_song_key(), None);
    assert_eq!(
        c.document.history_len(),
        steps,
        "no step when the key is already set"
    );
    assert_eq!(c.last_rejection(), Some("The song key is already C major"));
    assert!(steps > before);
}

#[test]
fn the_key_at_the_playhead_follows_the_changes() {
    let mut c = core();
    let h = Harmony::default()
        .with_key(Some(Key::new(0, ScaleKind::Major)))
        .unwrap()
        .with_key_change(2 * BAR, Some(Key::new(9, ScaleKind::NaturalMinor)))
        .unwrap();
    c.set_harmony(h).unwrap();
    assert_eq!(c.key_at_playhead(), Some(Key::new(0, ScaleKind::Major)));
    c.transport_action(atomik_ui::TransportAction::Locate(3 * BAR));
    assert_eq!(
        c.key_at_playhead(),
        Some(Key::new(9, ScaleKind::NaturalMinor))
    );
}

#[test]
fn the_browser_filters_by_the_tags_of_the_song_key() {
    let mut c = core();
    c.set_song_key(Some(Key::new(10, ScaleKind::NaturalMinor)))
        .unwrap();
    c.sync_song_key_filter();
    assert_eq!(c.file_browser.song_key_tags(), ["A# minor", "Bb minor"]);
    c.set_song_key(None).unwrap();
    c.sync_song_key_filter();
    assert!(c.file_browser.song_key_tags().is_empty());
}

#[test]
fn a_harmony_edit_reaches_the_engine_with_the_song() {
    let mut c = core();
    c.sink_mut().take();
    let h = Harmony {
        key: Some(Key::new(0, ScaleKind::Major)),
        changes: Vec::new(),
        chords: vec![ChordEvent::new(0, BAR, 9, ChordKind::Minor)],
    };
    c.set_harmony(h).unwrap();
    let song = c
        .sink_mut()
        .take()
        .into_iter()
        .rev()
        .find_map(|call| match call {
            SinkCall::Song(input) => Some(input),
            _ => None,
        })
        .expect("the song was sent again");
    assert_eq!(song.harmony.chord_at(0).map(|ch| ch.root), Some(9));
    assert_eq!(song.harmony.key_at(BAR * 4).map(|k| k.root), Some(0));
}

#[test]
fn the_rack_is_told_the_key_and_chord_at_the_playhead() {
    let mut c = core();
    c.frame();
    assert_eq!(c.view().song_harmony(), (None, None));
    c.set_harmony(Harmony {
        key: Some(Key::new(9, ScaleKind::NaturalMinor)),
        changes: Vec::new(),
        chords: vec![ChordEvent::new(0, BAR, 5, ChordKind::MajorSeventh)],
    })
    .unwrap();
    c.frame();
    assert_eq!(c.view().song_harmony(), (Some("Am"), Some("Fmaj7")));
}
