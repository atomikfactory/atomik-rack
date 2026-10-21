//! `Shell` key routing: the router, the context stack,
//! text ownership, held keys and key repeat, strict Ctrl versus Cmd per OS, numpad keys, the
//! modifier event, derived menu text. End-to-end key scripts: chord text in, shell state out.

use super::*;
use crate::session::NullBackend;
use atomik_model::{Command, DeviceKey};
use atomik_ui::keymap::script::event_for;
use atomik_ui::{DisplayList, TransportAction};

fn shell_for(os: Os, profile: Option<Profile>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(os),
            profile,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

fn shell() -> Shell<NullBackend> {
    // These tests characterise the earlier keys (and the routing under them): the legacy
    // profile. The standard keys are exercised in `shell_workflow_tests`.
    shell_for(Os::Windows, Some(Profile::Legacy))
}

fn add_synth(s: &mut Shell<NullBackend>, key: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(DeviceKey(key.into())),
        })
        .unwrap();
}

/// A key script step on a shell: `down CHORD`, `up CHORD` (the OS of the shell's keymap).
struct Keys<'a> {
    shell: &'a mut Shell<NullBackend>,
    os: Os,
}

impl Keys<'_> {
    fn event(&self, chord: &str, pressed: bool) -> UiEvent {
        let (key, modifiers) = event_for(self.os, chord).unwrap();
        UiEvent::Key {
            key,
            modifiers,
            pressed,
        }
    }

    fn down(&mut self, chord: &str) -> Vec<ShellRequest> {
        let e = self.event(chord, true);
        self.shell.handle(&e)
    }

    fn up(&mut self, chord: &str) {
        let e = self.event(chord, false);
        self.shell.handle(&e);
    }

    fn tap(&mut self, chord: &str) -> Vec<ShellRequest> {
        let r = self.down(chord);
        self.up(chord);
        r
    }
}

fn keys(s: &mut Shell<NullBackend>) -> Keys<'_> {
    Keys {
        shell: s,
        os: Os::Windows,
    }
}

#[test]
fn the_legacy_profile_is_selectable_and_shows_the_phase_4p_keys() {
    // The app default is `standard` (shell_workflow_tests); the earlier keys of
    // The earlier keys stay one profile away.
    assert_eq!(APP_DEFAULT_PROFILE, Profile::Standard);
    let s = shell_for(Os::Windows, Some(Profile::Legacy));
    assert_eq!(s.keymap().os(), Os::Windows);
    assert_eq!(
        s.keymap()
            .shortcut_text("file.import_audio", None)
            .as_deref(),
        Some("Ctrl+I"),
        "the shipped keys are the earlier keys"
    );
}

#[test]
fn the_context_stack_follows_the_focused_pane() {
    let mut s = shell();
    let ctx = |s: &Shell<NullBackend>| s.context_stack().contexts().to_vec();
    assert_eq!(ctx(&s), [Context::Rack, Context::Global]);
    s.set_focus(Pane::Transport);
    assert_eq!(ctx(&s), [Context::Global]);
    s.set_focus(Pane::Arrangement);
    assert_eq!(ctx(&s), [Context::Sequencer, Context::Global]);
    // The piano roll is an editor that shares the sequencer's time axis and transport keys.
    s.core_mut()
        .open_str(include_str!("../../../examples/projects/demo.atomik"), None)
        .unwrap();
    s.execute(AppCommand::TogglePianoRoll);
    s.set_focus(Pane::PianoRoll);
    assert_eq!(
        ctx(&s),
        [Context::Editor, Context::Sequencer, Context::Global]
    );
    // The console in the lower pane.
    s.execute(AppCommand::ToggleConsole);
    s.set_focus(Pane::Console);
    assert_eq!(ctx(&s), [Context::Mixer, Context::Global]);
    s.set_focus(Pane::Rack);
    assert_eq!(ctx(&s), [Context::Rack, Context::Global]);
}

#[test]
fn the_browser_filter_owns_plain_keys_but_command_chords_still_run() {
    let mut s = shell();
    s.execute(AppCommand::ToggleBrowser);
    assert!(s.core().view().is_text_owner());
    assert!(s.context_stack().is_text());
    let mut k = keys(&mut s);
    for chord in ["R", "L", "Space"] {
        k.tap(chord);
    }
    assert!(!s.core().is_playing(), "Space is typed into the filter");
    assert!(
        !s.core().doc().song.loop_enabled,
        "L is typed into the filter"
    );
    assert!(!s.core().is_recording(), "R is typed into the filter");
    // Command chords and function keys still run while typing.
    assert!(!s.menu_state().console);
    keys(&mut s).tap("F5");
    assert!(s.menu_state().console, "F5 runs in a text field");
    assert_eq!(keys(&mut s).tap("Cmd+S"), vec![ShellRequest::PickSaveFile]);
}

#[test]
fn transport_keys_work_in_the_rack_without_text_and_not_while_a_control_has_focus() {
    let mut s = shell();
    keys(&mut s).tap("Space");
    assert!(s.core().is_playing());
    keys(&mut s).tap("Space");
    assert!(!s.core().is_playing());
    keys(&mut s).tap("L");
    assert!(s.core().doc().song.loop_enabled);
    // R is today's record key (legacy profile); without an armed track it is refused.
    keys(&mut s).tap("R");
    assert!(!s.core().is_recording());
}

#[test]
fn a_held_key_is_a_repeat_and_only_repeatable_commands_see_it() {
    let mut s = shell();
    let mut k = keys(&mut s);
    k.down("Space");
    k.down("Space");
    k.down("Space");
    assert!(
        s.core().is_playing(),
        "one press, the repeats are swallowed"
    );
    keys(&mut s).up("Space");
    keys(&mut s).tap("Space");
    assert!(
        !s.core().is_playing(),
        "after the release it is a press again"
    );
}

#[test]
fn undo_repeats_while_the_keys_are_held() {
    let mut s = shell();
    for k in ["a", "b", "c"] {
        add_synth(&mut s, k);
    }
    keys(&mut s).down("Cmd+Z");
    let after_one = s.core().doc().devices.len();
    keys(&mut s).down("Cmd+Z");
    assert!(
        s.core().doc().devices.len() < after_one,
        "the held key undid a second step: undo is repeatable"
    );
}

#[test]
fn losing_focus_forgets_the_held_keys_and_modifiers() {
    let mut s = shell();
    keys(&mut s).down("Space");
    assert!(s.core().is_playing());
    s.handle(&UiEvent::ModifiersChanged {
        modifiers: Modifiers::CTRL,
    });
    assert_eq!(s.modifiers(), Modifiers::CTRL);
    s.focus_lost();
    assert_eq!(s.modifiers(), Modifiers::NONE);
    keys(&mut s).down("Space");
    assert!(
        !s.core().is_playing(),
        "the press after the focus loss is a press"
    );
}

#[test]
fn a_modifier_change_alone_does_not_repaint() {
    let mut s = shell();
    s.paint(&mut DisplayList::new());
    assert!(!s.needs_repaint());
    s.handle(&UiEvent::ModifiersChanged {
        modifiers: Modifiers::SHIFT,
    });
    assert!(
        !s.needs_repaint(),
        "nothing depends on the held modifiers yet"
    );
    assert_eq!(s.modifiers(), Modifiers::SHIFT);
}

#[test]
fn releases_do_not_run_commands_and_reach_the_view() {
    let mut s = shell();
    keys(&mut s).down("Cmd+S");
    let requests = s.handle(&UiEvent::Key {
        key: Key::Char('s'),
        modifiers: Modifiers::CTRL,
        pressed: false,
    });
    assert!(requests.is_empty(), "a release is not a second save");
}

#[test]
fn strict_ctrl_versus_cmd_per_os() {
    let meta = Modifiers {
        meta: true,
        ..Modifiers::NONE
    };
    let send = |s: &mut Shell<NullBackend>, m: Modifiers| {
        let down = UiEvent::Key {
            key: Key::Char('s'),
            modifiers: m,
            pressed: true,
        };
        let r = s.handle(&down);
        s.handle(&UiEvent::Key {
            key: Key::Char('s'),
            modifiers: m,
            pressed: false,
        });
        r
    };
    // Windows and Linux: Ctrl is the primary, Super is not.
    for os in [Os::Windows, Os::Linux] {
        let mut s = shell_for(os, None);
        assert_eq!(
            send(&mut s, Modifiers::CTRL),
            vec![ShellRequest::PickSaveFile]
        );
        assert!(send(&mut s, meta).is_empty(), "Super is not Cmd on {os:?}");
    }
    // macOS: Command is the primary, Control is not.
    let mut s = shell_for(Os::Mac, None);
    assert_eq!(send(&mut s, meta), vec![ShellRequest::PickSaveFile]);
    assert!(
        send(&mut s, Modifiers::CTRL).is_empty(),
        "Control is not Command on macOS"
    );
}

#[test]
fn numpad_keys_are_not_commands_in_legacy_and_type_their_characters() {
    let mut s = shell();
    let tempo = s.core().transport_bar().tempo_center();
    let down = |clicks| UiEvent::PointerDown {
        pos: tempo,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks,
    };
    let up = UiEvent::PointerUp {
        pos: tempo,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    };
    s.handle(&down(1));
    s.handle(&up);
    s.handle(&down(2));
    s.handle(&up);
    assert!(s.core().transport_bar().is_editing_tempo());
    let numpad = |n: NumpadKey| UiEvent::Key {
        key: Key::Numpad(n),
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    for n in [NumpadKey::Digit(9), NumpadKey::Digit(5), NumpadKey::Enter] {
        s.handle(&numpad(n));
        s.handle(&UiEvent::Key {
            key: Key::Numpad(n),
            modifiers: Modifiers::NONE,
            pressed: false,
        });
    }
    assert!(
        (s.core().doc().tempo_bpm - 95.0).abs() < 1e-9,
        "numpad 9 5 Enter typed the tempo"
    );
}

#[test]
fn the_reference_profile_binds_its_own_keys_end_to_end() {
    let mut s = shell_for(Os::Windows, Some(Profile::Standard));
    // Import audio moved to Ctrl+Shift+I; Ctrl+I is create instrument (not built yet: no
    // shell command, so the key falls through to the view and nothing happens).
    assert_eq!(
        keys(&mut s).tap("Cmd+Shift+I"),
        vec![ShellRequest::PickImportFiles]
    );
    assert!(keys(&mut s).tap("Cmd+I").is_empty());
    // F5 keeps swapping the console; Numpad0 (stop) and Numpad Enter (play) are distinct.
    keys(&mut s).tap("F5");
    assert!(s.menu_state().console);
    keys(&mut s).tap("NumpadEnter");
    assert!(s.core().is_playing(), "numpad Enter plays");
    keys(&mut s).tap("Numpad0");
    assert!(!s.core().is_playing(), "numpad 0 stops");
    // Space still toggles, and the metronome has C and Numpad9.
    keys(&mut s).tap("C");
    assert!(s.menu_state().metronome);
    keys(&mut s).tap("Numpad9");
    assert!(!s.menu_state().metronome);
}

#[test]
fn switching_the_keymap_changes_keys_and_menu_text() {
    let mut s = shell();
    let import = |s: &Shell<NullBackend>| {
        s.menu_model()
            .iter()
            .flat_map(|m| m.items().cloned().collect::<Vec<_>>())
            .find(|i| i.command == AppCommand::ImportAudio)
            .and_then(|i| i.shortcut)
    };
    assert_eq!(import(&s).as_deref(), Some("Ctrl+I"));
    s.set_keymap(Keymap::bundled(Profile::Standard, Os::Mac));
    assert_eq!(import(&s).as_deref(), Some("Cmd+Opt+I"));
    s.set_keymap(Keymap::bundled(Profile::Standard, Os::Windows));
    assert_eq!(import(&s).as_deref(), Some("Ctrl+Shift+I"));
}

#[test]
fn macos_menus_show_cmd_text() {
    let s = shell_for(Os::Mac, None);
    let save = s
        .menu_model()
        .iter()
        .flat_map(|m| m.items().cloned().collect::<Vec<_>>())
        .find(|i| i.command == AppCommand::Save)
        .and_then(|i| i.shortcut);
    assert_eq!(save.as_deref(), Some("Cmd+S"));
}

#[test]
fn legacy_keys_still_do_what_they_did_through_the_shell() {
    let mut s = shell();
    assert_eq!(
        keys(&mut s).tap("Cmd+I"),
        vec![ShellRequest::PickImportFiles]
    );
    assert_eq!(keys(&mut s).tap("Cmd+Q"), vec![ShellRequest::Quit]);
    keys(&mut s).tap("Tab");
    assert_eq!(s.core().view().flip(), Flip::Back);
    keys(&mut s).tap("Cmd+B");
    assert!(s.core().view().browser_visible());
    // Ctrl+Space and R record (refused without an armed track, but routed).
    keys(&mut s).tap("Cmd+Space");
    s.execute(AppCommand::Transport(TransportAction::Stop));
}
