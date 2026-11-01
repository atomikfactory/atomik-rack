//! The sequencer and note editor nudge keys and the note editor join and legato keys, driven the
//! way the window delivers them: winit's
//! logical key and modifier state through `translate`, then the shell's router. Ctrl nudges by the
//! snap step, Ctrl+Shift by a beat and Ctrl+Shift+Alt by a tick.

use super::*;
use crate::session::NullBackend;
use crate::translate;
use atomik_model::{Clip, ClipContent, ClipId, Command, DeviceKey, Note, SongCommand};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey};

const SYNTH: &str = "atomik.poly-synth";
const PPQ: i64 = 15_360;
const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// A shell with one track and one note clip (id 1) at the start of bar 2, the arrangement focused.
fn shell() -> Shell<NullBackend> {
    shell_with(vec![
        Note::new(0, PPQ, 60, 0.8),
        Note::new(2 * PPQ, PPQ, 64, 0.8),
    ])
}

/// The same with the notes `notes` in the clip.
fn shell_with(notes: Vec<Note>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 1500.0)));
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key("a")),
        })
        .unwrap();
    let clip = Clip {
        id: ClipId(0),
        start: BAR,
        length: BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(notes.into()),
    };
    for c in [
        SongCommand::AddTrack { device: key("a") },
        SongCommand::AddClip {
            device: key("a"),
            lane: 0,
            clip,
        },
    ] {
        s.core_mut().apply_command(Command::Song(c)).unwrap();
    }
    s.core_mut().after_view_change();
    s.set_focus(Pane::Arrangement);
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[ClipId(1)]);
    s
}

/// The key event winit delivers for an arrow with `mods` held on Windows.
fn arrow(s: &mut Shell<NullBackend>, named: NamedKey, mods: ModifiersState) {
    let modifiers = translate::modifiers(mods);
    let physical =
        winit::keyboard::PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
    let k = translate::key_event(&WinitKey::Named(named), &physical, modifiers, Os::Windows);
    for pressed in [true, false] {
        s.handle(&UiEvent::Key {
            key: k.clone(),
            modifiers,
            pressed,
        });
    }
}

fn clip_start(s: &Shell<NullBackend>) -> i64 {
    s.core().doc().song.clip(ClipId(1)).unwrap().2.start
}

const CTRL: ModifiersState = ModifiersState::CONTROL;

#[test]
fn ctrl_arrows_nudge_by_the_snap_step() {
    let mut s = shell();
    arrow(&mut s, NamedKey::ArrowRight, CTRL);
    let moved = clip_start(&s) - BAR;
    assert!(moved > 0, "the clip moved right by the step: {moved}");
    arrow(&mut s, NamedKey::ArrowLeft, CTRL);
    assert_eq!(clip_start(&s), BAR);
}

#[test]
fn ctrl_shift_arrows_nudge_by_a_beat() {
    let mut s = shell();
    arrow(&mut s, NamedKey::ArrowRight, CTRL | ModifiersState::SHIFT);
    assert_eq!(clip_start(&s), BAR + PPQ);
    arrow(&mut s, NamedKey::ArrowLeft, CTRL | ModifiersState::SHIFT);
    assert_eq!(clip_start(&s), BAR);
}

#[test]
fn ctrl_shift_alt_arrows_nudge_by_a_tick() {
    let mut s = shell();
    let all = CTRL | ModifiersState::SHIFT | ModifiersState::ALT;
    arrow(&mut s, NamedKey::ArrowRight, all);
    assert_eq!(clip_start(&s), BAR + 1);
    arrow(&mut s, NamedKey::ArrowLeft, all);
    assert_eq!(clip_start(&s), BAR);
}

/// The note starts of the open clip.
fn note_starts(s: &Shell<NullBackend>) -> Vec<i64> {
    match &s.core().doc().song.clip(ClipId(1)).unwrap().2.content {
        ClipContent::Notes(n) => n.iter().map(|n| n.tick).collect(),
        _ => Vec::new(),
    }
}

/// The shell with the note editor open on the clip, both notes selected.
fn editor() -> Shell<NullBackend> {
    let mut s = shell();
    s.core_mut().open_piano_roll(ClipId(1));
    s.set_focus(Pane::PianoRoll);
    s.core_mut().piano_roll_mut().set_selected_notes(&[0, 1]);
    s
}

#[test]
fn the_note_editor_nudges_by_the_step_a_beat_and_a_tick() {
    let mut s = editor();
    arrow(&mut s, NamedKey::ArrowRight, CTRL | ModifiersState::SHIFT);
    assert_eq!(note_starts(&s), vec![PPQ, 3 * PPQ]);
    arrow(&mut s, NamedKey::ArrowLeft, CTRL | ModifiersState::SHIFT);
    assert_eq!(note_starts(&s), vec![0, 2 * PPQ]);
    let all = CTRL | ModifiersState::SHIFT | ModifiersState::ALT;
    arrow(&mut s, NamedKey::ArrowRight, all);
    assert_eq!(note_starts(&s), vec![1, 2 * PPQ + 1]);
    arrow(&mut s, NamedKey::ArrowLeft, all);
    assert_eq!(note_starts(&s), vec![0, 2 * PPQ]);
    arrow(&mut s, NamedKey::ArrowRight, CTRL);
    assert!(note_starts(&s)[0] > 0);
}

/// A character key as winit reports it with `mods` held (upper case with Shift).
fn letter(s: &mut Shell<NullBackend>, c: &str, mods: ModifiersState) {
    let modifiers = translate::modifiers(mods);
    let physical =
        winit::keyboard::PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
    let k = translate::key_event(
        &WinitKey::Character(c.into()),
        &physical,
        modifiers,
        Os::Windows,
    );
    for pressed in [true, false] {
        s.handle(&UiEvent::Key {
            key: k.clone(),
            modifiers,
            pressed,
        });
    }
}

/// The notes of the open clip as `(tick, length, key)`.
fn notes(s: &Shell<NullBackend>) -> Vec<(i64, i64, u8)> {
    match &s.core().doc().song.clip(ClipId(1)).unwrap().2.content {
        ClipContent::Notes(n) => n.iter().map(|n| (n.tick, n.length, n.key)).collect(),
        _ => Vec::new(),
    }
}

fn editor_with(notes: Vec<Note>) -> Shell<NullBackend> {
    let mut s = shell_with(notes);
    s.core_mut().open_piano_roll(ClipId(1));
    s.set_focus(Pane::PianoRoll);
    s
}

#[test]
fn ctrl_j_in_the_note_editor_joins_the_selected_notes_of_one_key_across_a_gap() {
    let mut s = editor_with(vec![
        Note::new(0, PPQ, 60, 0.8),
        Note::new(PPQ, PPQ, 64, 0.8),
        Note::new(2 * PPQ, PPQ, 60, 0.8),
    ]);
    s.core_mut().piano_roll_mut().set_selected_notes(&[0, 1, 2]);
    letter(&mut s, "j", CTRL);
    let mut got = notes(&s);
    got.sort();
    assert_eq!(got, vec![(0, 3 * PPQ, 60), (PPQ, PPQ, 64)]);
    assert!(s.core_mut().undo(), "one undo step");
    assert_eq!(notes(&s).len(), 3);
}

#[test]
fn the_join_key_and_the_menu_item_run_the_same_command() {
    let start = vec![Note::new(0, PPQ, 60, 0.8), Note::new(3 * PPQ, PPQ, 60, 0.8)];
    let mut by_key = editor_with(start.clone());
    by_key
        .core_mut()
        .piano_roll_mut()
        .set_selected_notes(&[0, 1]);
    letter(&mut by_key, "j", CTRL);
    let mut by_menu = editor_with(start);
    by_menu
        .core_mut()
        .piano_roll_mut()
        .set_selected_notes(&[0, 1]);
    assert!(by_menu.run_command("editor.join_notes", None).is_some());
    assert_eq!(notes(&by_key), notes(&by_menu));
    assert_eq!(notes(&by_key), vec![(0, 4 * PPQ, 60)]);
}

#[test]
fn ctrl_shift_j_makes_the_selected_notes_legato() {
    let mut s = editor_with(vec![
        Note::new(0, PPQ / 2, 60, 0.8),
        Note::new(2 * PPQ, PPQ / 2, 62, 0.8),
        Note::new(4 * PPQ, PPQ / 2, 64, 0.8),
    ]);
    s.core_mut().piano_roll_mut().set_selected_notes(&[0, 1, 2]);
    letter(&mut s, "J", CTRL | ModifiersState::SHIFT);
    assert_eq!(
        notes(&s),
        vec![
            (0, 2 * PPQ, 60),
            (2 * PPQ, 2 * PPQ, 62),
            (4 * PPQ, PPQ / 2, 64)
        ]
    );
}
