//! The Tab Editor in the app core: opening and closing with the other editors, the switch with
//! the piano roll, typing through the pane (notes and sidecar in one undo step) and the audition.

use super::tests::rack_only_core;
use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::{NoteKind, PPQ};
use atomik_model::{Clip, ClipContent, ClipId, Command, DeviceKey, Note, SongCommand};
use atomik_ui::{Key, Modifiers, UiAction, UiEvent};

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn clip_with(notes: &[(i64, u8)]) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: 4 * PPQ,
        offset: 0,
        loop_length: None,
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
        content: ClipContent::Notes(
            notes
                .iter()
                .map(|&(t, k)| Note::new(t, PPQ / 2, k, 0.8))
                .collect(),
        ),
    }
}

/// A core with a synth track and one note clip.
fn core_with_clip(notes: &[(i64, u8)]) -> (AppCore<RecordingSink>, ClipId) {
    let mut c = rack_only_core();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.to_string(),
        key: Some(key("a")),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key("a") }))
        .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("a"),
        lane: 0,
        clip: clip_with(notes),
    }))
    .unwrap();
    let id = c.doc().song.tracks[0].lanes[0].clips[0].id;
    c.sink_mut().take();
    (c, id)
}

fn digit(c: &mut AppCore<RecordingSink>, d: char) {
    let actions = c.handle_pane_event(
        Pane::TabEditor,
        &UiEvent::Key {
            key: Key::Char(d),
            modifiers: Modifiers::NONE,
            pressed: true,
        },
    );
    for a in actions {
        c.handle_action(a);
    }
}

#[test]
fn opening_the_tab_editor_closes_the_piano_roll_and_the_other_way_round() {
    let (mut c, id) = core_with_clip(&[(0, 40)]);
    c.open_piano_roll(id);
    assert!(c.piano_roll_open());
    c.open_tab_editor(id);
    assert!(c.tab_editor_open() && !c.piano_roll_open());
    assert_eq!(c.tab_editor_clip(), Some(id));
    c.open_piano_roll(id);
    assert!(c.piano_roll_open() && !c.tab_editor_open());
    c.open_tab_editor(id);
    c.handle_action(UiAction::ClosePianoRoll);
    assert!(
        !c.tab_editor_open(),
        "the close action closes either editor"
    );
}

#[test]
fn the_default_clip_is_the_first_note_clip_and_none_without_one() {
    let (mut c, id) = core_with_clip(&[(0, 40)]);
    assert!(c.open_tab_editor_default());
    assert_eq!(c.tab_editor_clip(), Some(id));
    let mut empty = rack_only_core();
    assert!(!empty.open_tab_editor_default(), "no clip, no editor");
}

#[test]
fn the_switch_command_moves_the_clip_between_the_editors() {
    let (mut c, id) = core_with_clip(&[(0, 40)]);
    assert!(
        c.toggle_tab_or_piano_roll(),
        "with neither open: the tab editor"
    );
    assert_eq!(c.tab_editor_clip(), Some(id));
    assert!(c.toggle_tab_or_piano_roll());
    assert!(c.piano_roll_open() && !c.tab_editor_open());
    assert!(c.toggle_tab_or_piano_roll());
    assert!(c.tab_editor_open());
}

#[test]
fn removing_the_clip_closes_the_tab_editor() {
    let (mut c, id) = core_with_clip(&[(0, 40)]);
    c.open_tab_editor(id);
    c.apply_command(Command::Song(SongCommand::RemoveClip { id }))
        .unwrap();
    assert!(!c.tab_editor_open());
}

#[test]
fn a_typed_fret_is_one_undo_step_and_sounds_the_note() {
    let (mut c, id) = core_with_clip(&[]);
    c.open_tab_editor(id);
    // The default instrument is the guitar: put the cursor on the G string (index 2).
    c.tab_editor_mut().set_cursor(PPQ, 2);
    c.sink_mut().take();
    digit(&mut c, '5');
    let calls = c.sink_mut().take();
    let clip = c.doc().song.clip(id).unwrap().2.clone();
    let tab = clip.midi.tab.as_ref().expect("sidecar");
    assert_eq!(tab.instrument_id, "guitar6");
    let r = tab.note_of(PPQ, 60).expect("C4 on the G string");
    assert_eq!((r.string, r.fret), (2, 5));
    assert!(
        calls.iter().any(|call| matches!(
            call,
            SinkCall::LiveNote {
                kind: NoteKind::On { key: 60, .. },
                ..
            }
        )),
        "{calls:?}"
    );
    c.tab_editor_tick(1.0);
    c.tab_editor_tick(1.5);
    assert!(
        c.sink_mut().take().iter().any(|call| matches!(
            call,
            SinkCall::LiveNote {
                kind: NoteKind::Off { key: 60 },
                ..
            }
        )),
        "the clock releases the note"
    );
    assert!(c.undo());
    assert!(c.doc().song.clip(id).unwrap().2.midi.tab.is_none());
}

#[test]
fn closing_the_editor_releases_a_sounding_note() {
    let (mut c, id) = core_with_clip(&[]);
    c.open_tab_editor(id);
    digit(&mut c, '5');
    c.sink_mut().take();
    c.close_tab_editor();
    assert!(c.sink_mut().take().iter().any(|call| matches!(
        call,
        SinkCall::LiveNote {
            kind: NoteKind::Off { .. },
            ..
        }
    )));
}

#[test]
fn the_pane_takes_its_commands_and_declines_the_others() {
    let (mut c, id) = core_with_clip(&[]);
    c.open_tab_editor(id);
    assert!(
        c.pane_command(Pane::TabEditor, "editor.tab_cursor_right", None)
            .is_some()
    );
    assert_eq!(c.tab_editor().cursor_cell().0, PPQ);
    assert!(
        c.pane_command(Pane::TabEditor, "editor.chordify", None)
            .is_none()
    );
}
