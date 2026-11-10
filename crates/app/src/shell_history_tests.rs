//! The Undo History panel in the shell: the Edit menu item that docks it, what its rows show,
//! that a click or Up and Down go to any state, that it follows every edit, undo and redo
//! (also the ones made with the keyboard while it is open) and that a song window has its own.
//! Headless (null audio backend).

use super::*;
use crate::session::NullBackend;
use atomik_model::Command;
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

fn add_a() -> Command {
    Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.to_string(),
        key: Some(DeviceKey("a".to_string())),
    }
}

/// Three edits: a device, its name and the tempo.
fn three_edits(s: &mut Shell<NullBackend>) {
    let core = s.core_mut();
    core.apply_command(add_a()).unwrap();
    core.apply_command(Command::Rename {
        key: DeviceKey("a".to_string()),
        name: "Lead".to_string(),
    })
    .unwrap();
    core.apply_command(Command::SetTempo(97.0)).unwrap();
    core.after_view_change();
}

fn press(s: &mut Shell<NullBackend>, pos: Point) {
    s.handle(&UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

/// A click on row `i` of the panel (0 is the starting row).
fn click_row(s: &mut Shell<NullBackend>, i: usize) {
    let at = s
        .core()
        .history()
        .row_rect(i)
        .expect("the row shows")
        .center();
    press(s, at);
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

fn position(s: &Shell<NullBackend>) -> (usize, usize) {
    let d = s.core().document();
    (d.history_position(), d.history_len())
}

fn labels(s: &Shell<NullBackend>) -> Vec<String> {
    s.core()
        .history()
        .rows()
        .iter()
        .map(|r| r.label.clone())
        .collect()
}

#[test]
fn the_edit_menu_has_undo_history_under_redo_and_it_docks_the_panel() {
    let mut s = shell();
    assert_eq!(
        AppCommand::ToggleHistory.command_id(),
        Some(("edit.history", None))
    );
    let edit = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "Edit")
        .unwrap();
    let labels: Vec<_> = edit.items().map(|i| i.label.clone()).collect();
    let redo = labels.iter().position(|l| l.starts_with("Redo")).unwrap();
    assert_eq!(labels[redo + 1], "Undo History");
    assert!(!s.menu_state().history_panel);
    assert!(s.layout().history.is_none());
    let before = s.layout();
    s.execute(AppCommand::ToggleHistory);
    assert!(s.menu_state().history_panel);
    let l = s.layout();
    let panel = l.history.expect("the panel shows");
    assert!((panel.x1 - s.size().width).abs() < 1.0, "on the right");
    assert!(l.rack.x1 <= panel.x0 + 0.5 && l.transport.x1 <= panel.x0 + 0.5);
    assert!(l.rack.width() < before.rack.width());
    assert_eq!(s.hit_pane(panel.center()), Some(Pane::History));
    assert_eq!(s.pane_rect(Pane::History).map(|(_, r)| r), Some(panel));
    s.execute(AppCommand::ToggleHistory);
    assert!(s.layout().history.is_none());
    assert_eq!(s.layout().rack, before.rack);
}

#[test]
fn the_history_column_sits_left_of_the_track_panel() {
    let mut s = shell();
    s.execute(AppCommand::ToggleTrackPanel);
    let track = s.layout().track_panel.unwrap();
    s.execute(AppCommand::ToggleHistory);
    let l = s.layout();
    let history = l.history.unwrap();
    assert!((history.x1 - track.x0).abs() < 0.5);
    assert!(l.rack.x1 <= history.x0 + 0.5);
    assert_eq!(s.hit_pane(track.center()), Some(Pane::TrackPanel));
    assert_eq!(s.hit_pane(history.center()), Some(Pane::History));
}

#[test]
fn the_panel_lists_the_steps_with_the_current_row_after_every_edit_undo_and_redo() {
    let mut s = shell();
    s.core_mut().new_document();
    s.execute(AppCommand::ToggleHistory);
    assert_eq!(s.core().history().start_label(), "New song");
    assert!(labels(&s).is_empty());
    three_edits(&mut s);
    assert_eq!(labels(&s), ["Add Device", "Rename", "Change Tempo"]);
    assert_eq!(s.core().history().position(), 3);
    // Edit > Undo and Redo from the menu and the keys keep the list in step.
    s.execute(AppCommand::Undo);
    s.core_mut().after_view_change();
    assert_eq!(s.core().history().position(), 2);
    key(&mut s, Key::Char('z'), Modifiers::CTRL);
    assert_eq!(s.core().history().position(), 1);
    key(
        &mut s,
        Key::Char('z'),
        Modifiers {
            shift: true,
            ..Modifiers::CTRL
        },
    );
    assert_eq!(s.core().history().position(), 2);
    assert_eq!(labels(&s).len(), 3, "an undone step stays listed");
    // A new edit drops the steps that could have been redone.
    s.core_mut().apply_command(Command::SetTempo(88.0)).unwrap();
    s.core_mut().after_view_change();
    assert_eq!(labels(&s), ["Add Device", "Rename", "Change Tempo"]);
    assert_eq!(s.core().history().position(), 3);
}

#[test]
fn a_click_on_a_row_goes_to_the_state_after_that_step_and_back() {
    let mut s = shell();
    s.core_mut().new_document();
    let start = s.core().doc().clone();
    s.execute(AppCommand::ToggleHistory);
    three_edits(&mut s);
    let last = s.core().doc().clone();
    // The state after the first edit, taken from a second run of the same command.
    let mut other = shell();
    other.core_mut().new_document();
    other.core_mut().apply_command(add_a()).unwrap();
    let after_first = other.core().doc().clone();

    click_row(&mut s, 1);
    assert_eq!(s.core().doc(), &after_first);
    assert_eq!(position(&s), (1, 3), "two redo steps remain");
    assert_eq!(s.core().history().position(), 1);
    assert!(s.core().is_dirty());
    click_row(&mut s, 3);
    assert_eq!(s.core().doc(), &last);
    assert_eq!(position(&s), (3, 3));
    click_row(&mut s, 0);
    assert_eq!(s.core().doc(), &start, "the Opened row undoes everything");
    assert!(!s.core().is_dirty(), "the song is clean again");
    assert_eq!(position(&s), (0, 3));
    // Forward again to the middle.
    click_row(&mut s, 2);
    assert_eq!(position(&s), (2, 3));
    assert_eq!(s.core().history().position(), 2);
}

#[test]
fn the_starting_row_says_opened_for_a_song_that_came_from_a_file() {
    let mut s = shell();
    s.core_mut().new_document();
    let json = atomik_model::save_project(s.core().doc());
    s.core_mut()
        .open_str(&json, Some(PathBuf::from("song.atomik")))
        .unwrap();
    s.execute(AppCommand::ToggleHistory);
    assert_eq!(s.core().history().start_label(), "Opened");
    s.core_mut().new_document();
    s.core_mut().after_view_change();
    assert_eq!(s.core().history().start_label(), "New song");
}

#[test]
fn up_and_down_in_the_focused_list_undo_and_redo_one_step() {
    let mut s = shell();
    s.core_mut().new_document();
    s.execute(AppCommand::ToggleHistory);
    three_edits(&mut s);
    // A click on the current row gives the panel the keys without moving anywhere.
    click_row(&mut s, 3);
    assert_eq!(s.focus(), Pane::History);
    key(&mut s, Key::Up, Modifiers::NONE);
    assert_eq!(position(&s), (2, 3));
    key(&mut s, Key::Up, Modifiers::NONE);
    assert_eq!(position(&s), (1, 3));
    key(&mut s, Key::Down, Modifiers::NONE);
    assert_eq!(position(&s), (2, 3));
    assert_eq!(s.core().history().position(), 2);
    key(&mut s, Key::Home, Modifiers::NONE);
    assert_eq!(position(&s), (0, 3));
    key(&mut s, Key::End, Modifiers::NONE);
    assert_eq!(position(&s), (3, 3));
}

#[test]
fn hiding_the_panel_gives_the_keys_back_to_the_rack() {
    let mut s = shell();
    s.execute(AppCommand::ToggleHistory);
    click_row(&mut s, 0);
    assert_eq!(s.focus(), Pane::History);
    s.execute(AppCommand::ToggleHistory);
    assert_ne!(s.focus(), Pane::History);
}

#[test]
fn the_rows_show_how_long_ago_the_steps_were_made() {
    let mut s = shell();
    s.core_mut().new_document();
    s.core_mut().set_history_clock(Some(100.0));
    s.execute(AppCommand::ToggleHistory);
    s.core_mut().apply_command(Command::SetTempo(90.0)).unwrap();
    s.core_mut().set_history_clock(Some(160.0));
    s.core_mut().apply_command(Command::SetTempo(91.0)).unwrap();
    let end = 100.0 + 3.0 * 3600.0;
    s.core_mut().set_history_clock(Some(end));
    s.core_mut().after_view_change();
    let ages: Vec<_> = s
        .core()
        .history()
        .rows()
        .iter()
        .map(|r| r.age_secs)
        .collect();
    assert_eq!(
        ages,
        [Some((end - 100.0) as u64), Some((end - 160.0) as u64)]
    );
    assert_eq!(atomik_ui::history_panel::ago(ages[0].unwrap()), "3 h ago");
    assert_eq!(atomik_ui::history_panel::ago(ages[1].unwrap()), "2 h ago");
}

#[test]
fn a_new_song_starts_a_new_list() {
    let mut s = shell();
    s.core_mut().new_document();
    s.execute(AppCommand::ToggleHistory);
    three_edits(&mut s);
    assert_eq!(labels(&s).len(), 3);
    s.core_mut().new_document();
    s.core_mut().after_view_change();
    assert!(labels(&s).is_empty(), "the history went with the old song");
    assert_eq!(s.core().history().position(), 0);
}

#[test]
fn every_song_window_has_its_own_panel_and_history() {
    let mut a = shell();
    let mut b = shell();
    a.core_mut().new_document();
    b.core_mut().new_document();
    a.execute(AppCommand::ToggleHistory);
    three_edits(&mut a);
    assert!(!b.menu_state().history_panel);
    assert!(b.core().history().rows().is_empty());
    assert_eq!(a.core().history().rows().len(), 3);
}
