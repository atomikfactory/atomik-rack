//! The Piano Keys strip through the shell: F4, the focus, the `keys` context (letters are
//! notes), octave and velocity keys, Shift as the sustain pedal, and no stuck notes.

use super::*;
use crate::session::NullBackend;
use atomik_ui::KeysMode;
use atomik_ui::keymap::script::event_for;

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: Some(Profile::Standard),
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

fn press(s: &mut Shell<NullBackend>, chord: &str, pressed: bool) {
    let (key, modifiers) = event_for(Os::Windows, chord).unwrap();
    s.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed,
    });
}

fn tap(s: &mut Shell<NullBackend>, chord: &str) {
    press(s, chord, true);
    press(s, chord, false);
}

fn opened() -> Shell<NullBackend> {
    let mut s = shell();
    tap(&mut s, "F4");
    s
}

fn sounding(s: &Shell<NullBackend>) -> Vec<u8> {
    s.core().piano_keys().sounding()
}

#[test]
fn f4_shows_the_strip_with_the_focus_and_f9_is_the_file_browser() {
    let mut s = shell();
    assert!(s.layout().piano_keys.is_none());
    tap(&mut s, "F4");
    let strip = s.layout().piano_keys.expect("shown");
    assert_eq!(s.focus(), Pane::PianoKeys);
    assert!(s.core().piano_keys().is_focused());
    assert_eq!(strip.height(), PIANO_KEYS_HEIGHT);
    assert_eq!(s.core().piano_keys().bounds(), strip);
    assert!(s.menu_state().piano_keys);
    assert!(
        s.layout().song.is_some_and(|r| r.y1 <= strip.y0 + 0.5),
        "the song area ends above the strip"
    );
    assert!(!s.menu_state().file_browser);
    tap(&mut s, "F9");
    assert!(s.menu_state().file_browser);
    tap(&mut s, "F4");
    assert!(s.layout().piano_keys.is_none());
}

#[test]
fn letters_are_notes_while_the_strip_has_the_focus_and_not_commands() {
    let mut s = opened();
    assert_eq!(
        s.context_stack(),
        ContextStack::new(&[Context::Keys]),
        "the keys context"
    );
    let loop_before = s.core().doc().song.loop_enabled;
    press(&mut s, "A", true);
    assert_eq!(sounding(&s), vec![60]);
    press(&mut s, "L", true);
    assert_eq!(sounding(&s), vec![60, 74]);
    assert_eq!(s.core().doc().song.loop_enabled, loop_before, "L is a note");
    press(&mut s, "A", false);
    press(&mut s, "L", false);
    assert!(sounding(&s).is_empty());
}

#[test]
fn z_and_x_move_the_octave_and_the_number_keys_set_the_velocity() {
    let mut s = opened();
    tap(&mut s, "X");
    assert_eq!(s.core().piano_keys().octave(), 6);
    tap(&mut s, "Z");
    tap(&mut s, "Z");
    assert_eq!(s.core().piano_keys().octave(), 4);
    press(&mut s, "A", true);
    assert_eq!(sounding(&s), vec![48]);
    press(&mut s, "A", false);
    tap(&mut s, "1");
    assert_eq!(s.core().piano_keys().velocity(), 1);
    tap(&mut s, "0");
    assert_eq!(s.core().piano_keys().velocity(), 127);
}

#[test]
fn shift_sustains_and_does_not_change_the_note() {
    let mut s = opened();
    press(&mut s, "Shift+A", true);
    assert_eq!(sounding(&s), vec![60], "Shift+A plays the A key");
    press(&mut s, "Shift+A", false);
    assert_eq!(sounding(&s), vec![60], "sustained");
    s.handle(&UiEvent::ModifiersChanged {
        modifiers: Modifiers::NONE,
    });
    assert!(sounding(&s).is_empty());
}

#[test]
fn losing_the_focus_leaves_no_note_hanging() {
    let mut s = opened();
    press(&mut s, "A", true);
    press(&mut s, "S", true);
    assert_eq!(sounding(&s).len(), 2);
    s.focus_lost();
    assert!(sounding(&s).is_empty());
    // Clicking another pane does the same.
    press(&mut s, "A", true);
    s.set_focus(Pane::Rack);
    assert!(sounding(&s).is_empty());
    assert_eq!(s.context_stack(), ContextStack::new(&[Context::Rack]));
}

#[test]
fn closing_the_strip_releases_the_notes_and_gives_the_focus_back() {
    let mut s = opened();
    press(&mut s, "A", true);
    tap(&mut s, "F4");
    assert!(sounding(&s).is_empty());
    assert_ne!(s.focus(), Pane::PianoKeys);
}

#[test]
fn mouse_mode_keeps_the_commands_of_the_letters() {
    let mut s = opened();
    let off = s.core_mut().piano_keys_mut().set_mode(KeysMode::Mouse);
    assert!(off.is_empty());
    assert_eq!(s.context_stack(), ContextStack::global());
    let loop_before = s.core().doc().song.loop_enabled;
    tap(&mut s, "L");
    assert_ne!(s.core().doc().song.loop_enabled, loop_before);
    assert!(sounding(&s).is_empty());
}

#[test]
fn a_press_on_a_key_plays_it_and_the_release_ends_it() {
    let mut s = opened();
    let strip = s.layout().piano_keys.unwrap();
    // The lowest white key, low on its face (loud).
    let pos = Point::new(strip.center().x - 7.0 * 49.0, strip.y1 - 6.0);
    let pos = Point::new(pos.x.max(strip.x0 + 20.0), pos.y);
    s.handle(&UiEvent::PointerDown {
        pos,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    assert_eq!(sounding(&s).len(), 1);
    s.handle(&UiEvent::PointerUp {
        pos: Point::new(5.0, 5.0),
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
    assert!(sounding(&s).is_empty());
}

#[test]
fn the_keys_are_remappable_and_the_strip_shows_the_characters_of_the_keymap() {
    let s = opened();
    let keymap = s.keymap();
    let chords = keymap.chords_of("keys.note", Some("c"));
    assert_eq!(chords.len(), 1);
    assert_eq!(chords[0].0, Context::Keys);
    assert!(atomik_ui::keymap::find_conflicts(keymap).is_empty());
}
