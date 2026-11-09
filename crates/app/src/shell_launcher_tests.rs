//! The Launcher in the shell: the View menu, the key that switches the view, the lower pane
//! that swaps with the arrangement, Enter launching the slot under the cursor, a double click
//! that opens a clip in the piano roll, and typing a name that must not leak into commands.
//! Headless (null audio backend).

use super::*;
use crate::session::NullBackend;
use atomik_model::{Command, LauncherCommand, SongCommand};
use atomik_ui::{Key, Modifiers, PointerButton};

const SYNTH: &str = "atomik.poly-synth";

fn shell() -> Shell<NullBackend> {
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
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    s
}

fn key_of(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// Two synths on tracks with a one-bar clip in slot 0 of the first.
fn with_slots() -> Shell<NullBackend> {
    let mut s = shell();
    let core = s.core_mut();
    for k in ["a", "b"] {
        core.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key_of(k)),
        })
        .unwrap();
        core.apply_command(Command::Song(SongCommand::AddTrack { device: key_of(k) }))
            .unwrap();
    }
    core.apply_command(Command::Song(SongCommand::Launcher(
        LauncherCommand::NewClip {
            device: key_of("a"),
            row: 0,
            length: 4 * atomik_core::PPQ,
        },
    )))
    .unwrap();
    core.after_view_change();
    s
}

fn key(s: &mut Shell<NullBackend>, key: Key, modifiers: Modifiers) {
    s.handle(&UiEvent::Key {
        key: key.clone(),
        modifiers,
        pressed: true,
    });
    s.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: false,
    });
}

fn click(s: &mut Shell<NullBackend>, pos: Point, clicks: u8) {
    s.handle(&UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks,
    });
    s.handle(&UiEvent::PointerUp {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

fn slot_point(s: &Shell<NullBackend>, col: usize, row: usize) -> Point {
    let r = s.core().launcher().slot_rect(col, row).expect("shows");
    Point::new(r.x1 - 8.0, r.center().y)
}

#[test]
fn the_view_menu_has_a_launcher_submenu_and_the_command_switches_the_lower_pane() {
    let mut s = with_slots();
    assert_eq!(
        AppCommand::ToggleLauncher.command_id(),
        Some(("view.toggle_launcher", None))
    );
    let view = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "View")
        .unwrap();
    let labels: Vec<_> = view.items().map(|i| i.label.clone()).collect();
    for want in [
        "Back to Arrangement",
        "Stop All Clips",
        "Record Launches",
        "New Scene",
        "Scene Stops Empty Tracks",
        "Clip to Launcher",
        "Launcher Clip to Arrangement",
    ] {
        assert!(labels.iter().any(|l| l == want), "{want} in {labels:?}");
    }
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    assert!(!s.menu_state().launcher.shown);
    s.execute(AppCommand::ToggleLauncher);
    assert_eq!(s.lower_pane(), Pane::Launcher);
    assert!(s.menu_state().launcher.shown);
    let song = s.layout().song.expect("the lower pane shows");
    assert_eq!(s.pane_rect(Pane::Launcher).map(|(_, r)| r), Some(song));
    assert_eq!(s.core().launcher().bounds(), song);
    assert_eq!(s.hit_pane(song.center()), Some(Pane::Launcher));
    s.execute(AppCommand::ToggleLauncher);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
}

#[test]
fn f10_switches_the_launcher_and_the_arrangement_comes_back() {
    let mut s = with_slots();
    key(&mut s, Key::F(10), Modifiers::NONE);
    assert_eq!(s.lower_pane(), Pane::Launcher);
    key(&mut s, Key::F(10), Modifiers::NONE);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
}

#[test]
fn a_double_click_opens_the_clip_and_closing_the_roll_shows_the_launcher_again() {
    let mut s = with_slots();
    s.execute(AppCommand::ToggleLauncher);
    let at = slot_point(&s, 0, 0);
    click(&mut s, at, 1);
    click(&mut s, at, 2);
    assert!(
        s.core().piano_roll_open(),
        "the clip opens in the piano roll"
    );
    assert_eq!(s.lower_pane(), Pane::PianoRoll);
    s.core_mut().close_piano_roll();
    assert_eq!(s.lower_pane(), Pane::Launcher);
    // An empty slot: a double click makes a clip and opens it.
    let at = slot_point(&s, 1, 2);
    click(&mut s, at, 2);
    assert!(s.core().piano_roll_open());
    assert!(s.core().doc().song.launcher.slot(&key_of("b"), 2).is_some());
}

#[test]
fn enter_launches_the_slot_under_the_cursor_and_starts_the_transport() {
    let mut s = with_slots();
    s.execute(AppCommand::ToggleLauncher);
    assert!(!s.core().is_playing());
    let at = slot_point(&s, 0, 0);
    click(&mut s, at, 1);
    key(&mut s, Key::Enter, Modifiers::NONE);
    assert!(
        s.core().is_playing(),
        "a launch with the transport stopped plays"
    );
}

#[test]
fn typing_a_scene_name_does_not_run_commands() {
    let mut s = with_slots();
    s.execute(AppCommand::ToggleLauncher);
    let scene = s.core().launcher().scene_rect(0).expect("scene 1").center();
    click(&mut s, scene, 2);
    assert!(s.core().launcher().is_text_owner());
    assert_eq!(
        s.context_stack().contexts()[0],
        atomik_ui::keymap::Context::Text
    );
    // Space plays, F follows, K cycles the cables, Delete clears a slot, F10 switches the view:
    // all of them are text now.
    for k in [
        Key::Char('f'),
        Key::Char('k'),
        Key::Space,
        Key::Char('x'),
        Key::Delete,
    ] {
        key(&mut s, k, Modifiers::NONE);
    }
    assert!(!s.core().is_playing(), "Space typed a space");
    assert!(
        s.core().doc().song.launcher.slot(&key_of("a"), 0).is_some(),
        "Delete did not clear the slot"
    );
    assert_eq!(s.lower_pane(), Pane::Launcher);
    key(&mut s, Key::Enter, Modifiers::NONE);
    assert!(!s.core().launcher().is_text_owner());
    let name = s.core().doc().song.launcher.scenes[0].name.clone();
    assert!(
        name.contains('f') && name.contains('k') && name.contains('x'),
        "{name:?}"
    );
    // After the Enter the keys are commands again.
    key(&mut s, Key::Space, Modifiers::NONE);
    assert!(s.core().is_playing());
}

#[test]
fn the_launcher_edit_keys_act_on_the_cursor_slot_and_undo_together_with_the_menu_commands() {
    let mut s = with_slots();
    s.execute(AppCommand::ToggleLauncher);
    let at = slot_point(&s, 0, 0);
    click(&mut s, at, 1);
    key(&mut s, Key::Delete, Modifiers::NONE);
    assert!(s.core().doc().song.launcher.slot(&key_of("a"), 0).is_none());
    s.execute(AppCommand::Undo);
    assert!(s.core().doc().song.launcher.slot(&key_of("a"), 0).is_some());
    // New Scene adds a row; Record Launches and Stop Empty flip.
    let scenes = s.core().doc().song.launcher.scenes.len();
    s.execute(AppCommand::LauncherNewScene);
    assert_eq!(s.core().doc().song.launcher.scenes.len(), scenes + 1);
    assert!(!s.menu_state().launcher.record);
    s.execute(AppCommand::LauncherRecord);
    assert!(s.menu_state().launcher.record);
    s.execute(AppCommand::LauncherStopEmpty);
    assert!(!s.menu_state().launcher.stop_empty);
    s.execute(AppCommand::LauncherQuantize(
        atomik_model::LaunchQuantize::Quarter,
    ));
    assert_eq!(
        s.menu_state().launcher.quantize,
        atomik_model::LaunchQuantize::Quarter
    );
}
