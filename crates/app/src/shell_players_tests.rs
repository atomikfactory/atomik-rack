//! `Shell` tests for the note players: the sequencer grid editor in the lower pane,
//! its panel's Edit button and a click on its grid.

use super::*;
use crate::session::NullBackend;
use atomik_ui::{Modifiers, PointerButton};

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 1500.0)));
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn shell_with_sequencer() -> Shell<NullBackend> {
    let mut s = shell();
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.step-sequencer".into(),
            key: Some(key("seq")),
        })
        .unwrap();
    s
}

fn open(s: &mut Shell<NullBackend>) {
    let open = UiAction::OpenSeqEditor { device: key("seq") };
    s.core_mut().handle_action(open.clone());
    s.pane_actions(&[open]);
}

#[test]
fn opening_the_editor_shows_it_in_the_lower_pane_with_the_keys() {
    let mut s = shell_with_sequencer();
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    open(&mut s);
    assert_eq!(s.lower_pane(), Pane::SeqEditor);
    assert_eq!(s.focus, Pane::SeqEditor);
    assert!(
        s.core().seq_editor().bounds().height() > 100.0,
        "it got the pane's bounds"
    );
    let close = UiAction::CloseSeqEditor;
    s.core_mut().handle_action(close.clone());
    s.pane_actions(&[close]);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    assert_eq!(s.focus, Pane::Arrangement);
}

#[test]
fn the_editor_replaces_the_console_and_a_click_on_its_grid_edits_the_pattern() {
    let mut s = shell_with_sequencer();
    s.execute(AppCommand::ToggleConsole);
    assert_eq!(s.lower_pane(), Pane::Console);
    open(&mut s);
    assert_eq!(s.lower_pane(), Pane::SeqEditor, "the console gave way");
    let at = s.core().seq_editor().note_cell_center(2, 62);
    for event in [
        UiEvent::PointerMove {
            pos: at,
            modifiers: Modifiers::NONE,
        },
        UiEvent::PointerDown {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        UiEvent::PointerUp {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        },
    ] {
        s.handle(&event);
    }
    let step = s
        .core()
        .doc()
        .device(&key("seq"))
        .unwrap()
        .steps
        .as_ref()
        .unwrap()
        .patterns[0]
        .steps[2];
    assert!(step.on && step.key == 62, "{step:?}");
}

#[test]
fn removing_the_sequencer_closes_the_editor_pane() {
    let mut s = shell_with_sequencer();
    open(&mut s);
    s.core_mut()
        .apply_command(Command::RemoveDevice { key: key("seq") })
        .unwrap();
    assert_eq!(s.lower_pane(), Pane::Arrangement);
}

#[test]
fn a_pattern_name_being_typed_owns_plain_keys() {
    // letters, digits and Space reach the name; none is a transport command.
    let mut s = shell_with_sequencer();
    open(&mut s);
    assert_eq!(
        s.context_stack().contexts(),
        [Context::Editor, Context::Global]
    );
    let p = s.core().seq_editor().slot_center(0);
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
    assert!(
        s.core().seq_editor().is_text_owner(),
        "a double-click renames"
    );
    assert_eq!(
        s.context_stack().contexts(),
        [Context::Text, Context::Editor, Context::Global]
    );
    for k in [Key::Char('l'), Key::Space, Key::Char('r'), Key::Char('1')] {
        for pressed in [true, false] {
            s.handle(&UiEvent::Key {
                key: k.clone(),
                modifiers: Modifiers::NONE,
                pressed,
            });
        }
    }
    assert!(
        s.core()
            .seq_editor()
            .renaming()
            .is_some_and(|t| t.ends_with("l r1")),
        "{:?}",
        s.core().seq_editor().renaming()
    );
    assert!(!s.core().is_playing() && !s.core().is_recording());
    assert!(!s.core().doc().song.loop_enabled);
}
