//! Tests of text owners in every pane, release and repeat details, the
//! grids that pin the legacy profile): added as a second review pass.

use super::*;
use crate::session::NullBackend;
use atomik_model::Command;
use atomik_ui::keymap::script::event_for;
use atomik_ui::{Modifiers, PointerButton};

fn shell_for(os: Os, profile: Profile) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(os),
            profile: Some(profile),
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 1500.0)));
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn tap(s: &mut Shell<NullBackend>, os: Os, chord: &str) -> Vec<ShellRequest> {
    let (k, modifiers) = event_for(os, chord).unwrap();
    let mut requests = s.handle(&UiEvent::Key {
        key: k.clone(),
        modifiers,
        pressed: true,
    });
    requests.extend(s.handle(&UiEvent::Key {
        key: k,
        modifiers,
        pressed: false,
    }));
    requests
}

fn double_click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) {
    for clicks in [1, 2] {
        s.handle(&UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks,
        });
        s.handle(&UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        });
    }
}

/// Everything a stray command could change, for before-and-after comparison.
fn snapshot(s: &Shell<NullBackend>) -> String {
    format!(
        "{:?} playing={} recording={} undo={} redo={} lower={:?} focus={:?}",
        s.menu_state(),
        s.core().is_playing(),
        s.core().is_recording(),
        s.core().document().can_undo(),
        s.core().document().can_redo(),
        s.lower_pane(),
        s.focus(),
    )
}

/// Letters (both cases), digits, Space, punctuation and the editing keys of a typing session.
const PLAIN: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789,.;/[]=-_+";

fn type_everything(s: &mut Shell<NullBackend>, os: Os) -> Vec<ShellRequest> {
    let mut requests = Vec::new();
    let mut press = |s: &mut Shell<NullBackend>, k: Key, shift: bool| {
        for pressed in [true, false] {
            requests.extend(s.handle(&UiEvent::Key {
                key: k.clone(),
                modifiers: Modifiers {
                    shift,
                    ..Modifiers::NONE
                },
                pressed,
            }));
        }
    };
    for c in PLAIN.chars() {
        press(s, Key::Char(c), c.is_ascii_uppercase());
    }
    press(s, Key::Space, false);
    for k in [
        Key::Left,
        Key::Right,
        Key::Up,
        Key::Down,
        Key::Delete,
        Key::Home,
        Key::End,
    ] {
        press(s, k, false);
    }
    let _ = os;
    requests
}

/// While any text owner has the keyboard, no plain key is a command, in either profile
/// on every OS (the standard profile binds nearly every letter).
fn assert_owner_swallows_plain_keys(
    prepare: impl Fn(&mut Shell<NullBackend>),
    owns: impl Fn(&Shell<NullBackend>) -> bool,
    what: &str,
) {
    for os in Os::ALL {
        for profile in Profile::ALL {
            let mut s = shell_for(os, profile);
            prepare(&mut s);
            assert!(owns(&s), "{what}: {os:?} {profile:?} owns the keyboard");
            assert!(s.context_stack().is_text(), "{what}: the stack says Text");
            let before = snapshot(&s);
            let requests = type_everything(&mut s, os);
            assert!(
                requests.is_empty(),
                "{what}: {os:?} {profile:?}: {requests:?}"
            );
            assert_eq!(
                snapshot(&s),
                before,
                "{what}: {os:?} {profile:?}: a plain key ran a command"
            );
            assert!(owns(&s), "{what}: typing does not end the edit");
        }
    }
}

#[test]
fn the_rack_browser_filter_swallows_every_plain_key_in_both_profiles_on_every_os() {
    assert_owner_swallows_plain_keys(
        |s| {
            s.execute(AppCommand::ToggleBrowser);
        },
        |s| s.core().view().is_text_owner(),
        "browser filter",
    );
}

#[test]
fn a_pattern_name_swallows_every_plain_key_in_both_profiles_on_every_os() {
    assert_owner_swallows_plain_keys(
        |s| {
            s.core_mut()
                .apply_command(Command::AddDevice {
                    index: usize::MAX,
                    type_id: "atomik.step-sequencer".into(),
                    key: Some(key("seq")),
                })
                .unwrap();
            let open = UiAction::OpenSeqEditor { device: key("seq") };
            s.core_mut().handle_action(open.clone());
            s.pane_actions(&[open]);
            let p = s.core().seq_editor().slot_center(0);
            double_click(s, p);
        },
        |s| s.core().seq_editor().is_text_owner(),
        "pattern rename",
    );
}

#[test]
fn a_macro_name_in_the_container_editor_swallows_every_plain_key_in_both_profiles_on_every_os() {
    assert_owner_swallows_plain_keys(
        |s| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/projects/container-demo/container-demo.atomik");
            s.core_mut().open(&path).unwrap();
            s.core_mut().wait_background();
            let open = UiAction::OpenContainerEditor {
                device: key("layer"),
                macros: Some(0),
            };
            s.core_mut().handle_action(open.clone());
            s.pane_actions(&[open]);
            assert_eq!(s.lower_pane(), Pane::ContainerEditor);
            let p = s.core().container_editor().macro_name_rect(0).center();
            double_click(s, p);
        },
        |s| s.core().container_editor().is_text_owner(),
        "macro rename",
    );
}

#[test]
fn a_console_name_swallows_every_plain_key_in_both_profiles_on_every_os() {
    assert_owner_swallows_plain_keys(
        |s| {
            s.core_mut()
                .apply_command(Command::AddDevice {
                    index: usize::MAX,
                    type_id: "atomik.poly-synth".into(),
                    key: Some(key("synth")),
                })
                .unwrap();
            s.execute(AppCommand::ToggleConsole);
            s.set_focus(Pane::Console);
            let channel = s.core().doc().console.channels[0].channel.clone();
            assert!(s.core_mut().console_mut().begin_rename(&channel));
        },
        |s| s.core().console().is_text_owner(),
        "console rename",
    );
}

#[test]
fn a_tempo_being_typed_takes_every_key_before_the_router_in_both_profiles() {
    for os in Os::ALL {
        for profile in Profile::ALL {
            let mut s = shell_for(os, profile);
            let p = s.core().transport_bar().tempo_center();
            double_click(&mut s, p);
            assert!(s.core().transport_bar().is_editing_tempo(), "{os:?}");
            let before = snapshot(&s);
            for chord in ["Space", "R", "L", "Q", "Tab", "1", "F5"] {
                tap(&mut s, os, chord);
            }
            assert_eq!(snapshot(&s), before, "{os:?} {profile:?}");
            assert!(s.core().transport_bar().is_editing_tempo());
        }
    }
}

#[test]
fn command_chords_still_run_in_every_text_owner() {
    // The primary modifier or a function key is never typing.
    for os in Os::ALL {
        let mut s = shell_for(os, Profile::Legacy);
        s.execute(AppCommand::ToggleBrowser);
        assert!(s.core().view().is_text_owner());
        assert!(!s.menu_state().console);
        tap(&mut s, os, "F5");
        assert!(s.menu_state().console, "{os:?}");
        let requests = tap(&mut s, os, "Cmd+S");
        assert_eq!(requests, vec![ShellRequest::PickSaveFile], "{os:?}");
    }
}

#[test]
fn a_repeat_of_a_key_a_command_took_never_reaches_the_focused_view_as_a_second_press() {
    // The arrangement also maps Space to play and stop itself: if the swallowed repeats were
    // forwarded, holding Space would toggle playback again and again.
    for os in Os::ALL {
        let mut s = shell_for(os, Profile::Legacy);
        s.set_focus(Pane::Arrangement);
        let (k, modifiers) = event_for(os, "Space").unwrap();
        for _ in 0..4 {
            s.handle(&UiEvent::Key {
                key: k.clone(),
                modifiers,
                pressed: true,
            });
        }
        assert!(
            s.core().is_playing(),
            "{os:?}: one press, three swallowed repeats"
        );
        s.handle(&UiEvent::Key {
            key: k,
            modifiers,
            pressed: false,
        });
    }
}
