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

/// The notes of the open clip, as keys in time order.
fn clip_keys(c: &AppCore<RecordingSink>, id: ClipId) -> Vec<u8> {
    match &c.doc().song.clip(id).unwrap().2.content {
        ClipContent::Notes(n) => n.iter().map(|n| n.key).collect(),
        _ => panic!("notes"),
    }
}

#[test]
fn copy_paste_and_select_all_go_to_the_pane_and_the_menus_know_the_selection() {
    let (mut c, id) = core_with_clip(&[(0, 40), (PPQ, 45), (2 * PPQ, 52)]);
    c.open_tab_editor(id);
    assert!(
        !c.has_selection(Pane::TabEditor),
        "no note under the cursor yet"
    );
    assert!(!c.can_paste(Pane::TabEditor));
    assert!(
        c.pane_command(Pane::TabEditor, "edit.select_all", None)
            .is_some()
    );
    assert!(c.has_selection(Pane::TabEditor));
    assert_eq!(c.tab_editor().selected().len(), 3);
    assert!(c.pane_command(Pane::TabEditor, "edit.copy", None).is_some());
    assert!(c.can_paste(Pane::TabEditor));
    // Pasting at the last beat keeps what fits in the four-beat clip: one note.
    c.tab_editor_mut().set_cursor(3 * PPQ, 0);
    c.pane_command(Pane::TabEditor, "edit.paste", None);
    assert_eq!(clip_keys(&c, id), [40, 45, 52, 40]);
    // Cut takes the selection out as one undo step.
    c.pane_command(Pane::TabEditor, "edit.select_all", None);
    c.pane_command(Pane::TabEditor, "edit.cut", None);
    assert!(clip_keys(&c, id).is_empty());
    assert!(c.undo());
    assert_eq!(clip_keys(&c, id), [40, 45, 52, 40]);
}

#[test]
fn techniques_transpose_and_string_changes_are_undoable_commands_of_the_pane() {
    let (mut c, id) = core_with_clip(&[(0, 45), (PPQ, 52)]);
    c.open_tab_editor(id);
    c.pane_command(Pane::TabEditor, "edit.select_all", None);
    assert!(
        c.pane_command(Pane::TabEditor, "editor.tab_technique", Some("palm_mute"))
            .is_some()
    );
    let tab = |c: &AppCore<RecordingSink>| c.doc().song.clip(id).unwrap().2.midi.tab.clone();
    let muted = |c: &AppCore<RecordingSink>| {
        tab(c).map(|t| {
            t.notes
                .iter()
                .filter(|r| r.tech.has(atomik_tab::score::TechFlags::PALM_MUTE))
                .count()
        })
    };
    assert_eq!(muted(&c), Some(2));
    c.pane_command(Pane::TabEditor, "editor.tab_transpose", Some("octave_up"));
    assert_eq!(clip_keys(&c, id), [57, 64]);
    assert!(c.undo());
    assert_eq!(clip_keys(&c, id), [45, 52]);
    // A string change keeps the pitches and moves the notes to the next string down.
    let before = tab(&c).unwrap();
    c.pane_command(Pane::TabEditor, "editor.tab_string_down", None);
    assert_eq!(clip_keys(&c, id), [45, 52]);
    let after = tab(&c).unwrap();
    assert!(
        before
            .notes
            .iter()
            .zip(&after.notes)
            .all(|(a, b)| b.string == a.string + 1 && a.key == b.key)
    );
    assert!(c.undo());
    assert_eq!(tab(&c).unwrap().notes, before.notes);
    // The first undo took the technique back: the clip is as it was, with no sidecar.
    assert!(c.undo());
    assert_eq!(muted(&c), None);
}

#[test]
fn the_staff_menu_has_the_edit_and_technique_items_and_greys_what_needs_a_selection() {
    use crate::context_menus::{MenuEnable, specs};
    use atomik_ui::context_menu::Spec;
    let flat = |e: &MenuEnable| -> Vec<(String, bool)> {
        fn walk(list: &[Spec], out: &mut Vec<(String, bool)>) {
            for s in list {
                match s {
                    Spec::Item {
                        command,
                        arg,
                        enabled,
                        ..
                    } => out.push((format!("{command}:{}", arg.unwrap_or("")), *enabled)),
                    Spec::Submenu { items, .. } => walk(items, out),
                    _ => {}
                }
            }
        }
        let mut v = Vec::new();
        walk(&specs(atomik_ui::ContextTarget::TabStaff, e), &mut v);
        v
    };
    let off = flat(&MenuEnable::default());
    let find = |v: &[(String, bool)], id: &str| v.iter().find(|(n, _)| n == id).map(|(_, e)| *e);
    assert_eq!(find(&off, "editor.tab_technique:hammer_on"), Some(false));
    assert_eq!(find(&off, "editor.tab_technique:slide_legato"), Some(false));
    assert_eq!(find(&off, "editor.tab_technique:bend_half"), Some(false));
    assert_eq!(find(&off, "editor.tab_refret:choose"), Some(false));
    assert_eq!(find(&off, "edit.select_all:"), Some(true));
    assert_eq!(find(&off, "editor.tab_tuning:"), Some(true));
    let on = flat(&MenuEnable {
        selection: true,
        paste: true,
        ..MenuEnable::default()
    });
    assert_eq!(find(&on, "editor.tab_technique:hammer_on"), Some(true));
    assert_eq!(find(&on, "edit.paste:"), Some(true));
    assert_eq!(find(&on, "editor.tab_string_down:"), Some(true));
    assert_eq!(find(&on, "editor.tab_transpose:octave_up"), Some(true));
}

#[test]
fn key_events_are_stamped_on_the_frame_clock_so_an_idle_pause_cannot_join_digits() {
    let (mut c, id) = core_with_clip(&[]);
    c.open_tab_editor(id);
    // The frame clock says 500 s: the stamp agrees with it from then on (a real pause adds to it).
    c.tab_editor_tick(500.0);
    let now = c.tab_clock.now();
    assert!((500.0..501.0).contains(&now), "{now}");
    c.tab_editor_tick(9000.0);
    let now = c.tab_clock.now();
    assert!((9000.0..9001.0).contains(&now), "{now}");
    // A digit then another well after the window: two notes' worth of frets, not "12".
    digit(&mut c, '1');
    c.tab_editor_tick(9000.0 + 5.0);
    digit(&mut c, '2');
    let clip = c.doc().song.clip(id).unwrap().2.clone();
    let fret = clip
        .midi
        .tab
        .as_ref()
        .and_then(|t| t.notes.first().map(|r| r.fret));
    assert_eq!(fret, Some(2), "the second digit is its own fret");
}
