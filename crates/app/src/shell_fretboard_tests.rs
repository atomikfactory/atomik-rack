//! The fretboard through the shell: its commands (toggle, detach, left-handed, voicing, labels),
//! the window it opens and the strip under the Tab Editor staff giving way to it, what the
//! window shows (the tab editor's clip at the cursor), and the saved choices.

use super::*;
use crate::session::NullBackend;
use crate::ui_state::UiState;
use crate::windows::WindowKind;
use atomik_ui::fretboard::live_notes;
use atomik_ui::keymap::script::event_for;
use atomik_ui::{DisplayList, FretLabels, FretboardOptions};

fn make(ui_state_path: Option<PathBuf>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: Some(Profile::Standard),
            in_window_menu: true,
            settings_path: None,
            ui_state_path,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 900.0)));
    s
}

fn shell() -> Shell<NullBackend> {
    make(None)
}

/// A shell with the demo project and its first note clip open in the tab editor.
fn with_tab_editor() -> Shell<NullBackend> {
    let mut s = shell();
    s.core_mut()
        .open_str(include_str!("../../../examples/projects/demo.atomik"), None)
        .unwrap();
    s.execute(AppCommand::ToggleTabEditor);
    assert!(s.core().tab_editor_open());
    s
}

fn tap(s: &mut Shell<NullBackend>, chord: &str) {
    let (key, modifiers) = event_for(Os::Windows, chord).unwrap();
    let window = s.focused_window();
    for pressed in [true, false] {
        s.handle_in(
            window,
            &UiEvent::Key {
                key: key.clone(),
                modifiers,
                pressed,
            },
        );
    }
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-fretboard-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn the_strip_shows_by_default_and_the_toggle_hides_and_shows_it() {
    let mut s = with_tab_editor();
    assert!(s.menu_state().fretboard);
    assert!(!s.menu_state().fretboard_detached);
    assert!(s.core().tab_editor().fretboard_shown());
    assert!(s.core().tab_editor().fretboard_rect().is_some());
    s.execute(AppCommand::ToggleFretboard);
    assert!(!s.menu_state().fretboard);
    assert!(!s.core().tab_editor().fretboard_shown());
    assert!(s.core().tab_editor().fretboard_rect().is_none());
    s.execute(AppCommand::ToggleFretboard);
    assert!(s.core().tab_editor().fretboard_rect().is_some());
}

#[test]
fn detaching_opens_the_window_and_the_strip_gives_way_to_it() {
    let mut s = with_tab_editor();
    assert!(!s.windows_wanted().contains(&WindowKind::Fretboard));
    s.execute(AppCommand::DetachFretboard);
    assert!(s.windows_wanted().contains(&WindowKind::Fretboard));
    assert!(s.menu_state().fretboard_detached);
    let (window, rect) = s.pane_rect(Pane::Fretboard).expect("shown");
    assert_eq!(window, WindowKind::Fretboard);
    assert_eq!(rect.height(), atomik_ui::FRETBOARD_HEIGHT);
    assert_eq!(s.core().fretboard().bounds(), rect);
    assert!(
        s.core().tab_editor().fretboard_rect().is_none(),
        "the neck is in the window, not under the staff"
    );
    assert_eq!(s.focus(), Pane::Fretboard);
    // The OS closing the window attaches the strip again.
    s.window_closed(WindowKind::Fretboard);
    assert!(!s.windows_wanted().contains(&WindowKind::Fretboard));
    assert!(s.pane_rect(Pane::Fretboard).is_none());
    assert!(s.core().tab_editor().fretboard_rect().is_some());
    // The detach command is a toggle, too.
    s.execute(AppCommand::DetachFretboard);
    s.execute(AppCommand::DetachFretboard);
    assert!(!s.windows_wanted().contains(&WindowKind::Fretboard));
    assert!(s.core().tab_editor().fretboard_rect().is_some());
}

#[test]
fn the_toggle_while_detached_closes_the_window_and_hides_the_neck() {
    let mut s = with_tab_editor();
    s.execute(AppCommand::DetachFretboard);
    s.execute(AppCommand::ToggleFretboard);
    assert!(!s.windows_wanted().contains(&WindowKind::Fretboard));
    assert!(!s.menu_state().fretboard);
    assert!(s.core().tab_editor().fretboard_rect().is_none());
}

#[test]
fn the_options_are_commands_that_reach_both_views() {
    let mut s = with_tab_editor();
    s.execute(AppCommand::DetachFretboard);
    assert_eq!(s.core().fretboard_options(), FretboardOptions::default());
    s.execute(AppCommand::FretboardLeftHanded);
    s.execute(AppCommand::FretboardVoicing);
    s.execute(AppCommand::SetFretLabels(FretLabels::Intervals));
    let want = FretboardOptions {
        left_handed: true,
        labels: FretLabels::Intervals,
        chord_voicing: true,
    };
    assert_eq!(s.core().fretboard_options(), want);
    assert_eq!(s.core().fretboard().options(), want);
    let m = s.menu_state();
    assert!(m.fretboard_left_handed && m.fretboard_voicing);
    assert_eq!(m.fretboard_labels, FretLabels::Intervals);
    s.execute(AppCommand::FretboardLeftHanded);
    assert!(!s.menu_state().fretboard_left_handed);
}

#[test]
fn the_commands_run_by_their_registry_ids() {
    let mut s = with_tab_editor();
    for (id, arg) in [
        ("view.fretboard_labels", Some("frets")),
        ("view.fretboard_left_handed", None),
        ("view.fretboard_voicing", None),
        ("view.detach_fretboard", None),
    ] {
        assert!(s.run_command(id, arg).is_some(), "{id}");
    }
    assert!(s.menu_state().fretboard_detached);
    assert_eq!(s.menu_state().fretboard_labels, FretLabels::Frets);
    assert!(s.menu_state().fretboard_left_handed);
    assert!(s.run_command("view.toggle_fretboard", None).is_some());
    assert!(!s.menu_state().fretboard_detached);
    assert!(
        AppCommand::from_command("view.fretboard_labels", Some("nonsense")).is_none(),
        "an unknown argument is refused"
    );
}

#[test]
fn the_window_shows_what_the_tab_cursor_is_on() {
    let mut s = with_tab_editor();
    s.execute(AppCommand::DetachFretboard);
    s.set_focus(Pane::TabEditor);
    // Moving the cursor feeds the window without a frame running.
    tap(&mut s, "Right");
    tap(&mut s, "Right");
    let feed = s.core().tab_editor().fretboard_feed();
    let data = feed.data.expect("the clip's tab");
    let want = live_notes(&data, feed.tick.expect("a tick"));
    assert_eq!(s.core().fretboard().live(), &want);
    assert!(!data.notes.is_empty(), "the demo clip has notes");
}

#[test]
fn the_window_paints_the_neck_and_its_chips_and_repaints_on_a_change() {
    let mut s = with_tab_editor();
    s.execute(AppCommand::DetachFretboard);
    let mut dl = DisplayList::new();
    s.paint_in(WindowKind::Fretboard, &mut dl);
    let json = dl.to_json();
    assert!(json.contains("Left-handed"), "the chips are drawn");
    assert!(!s.needs_repaint_in(WindowKind::Fretboard));
    s.execute(AppCommand::FretboardLeftHanded);
    assert!(s.needs_repaint_in(WindowKind::Fretboard));
    // A click on a chip of the window runs its command.
    let mut dl = DisplayList::new();
    s.paint_in(WindowKind::Fretboard, &mut dl);
    let area = s.core().fretboard().bounds();
    let (chip, _) = s.core().fretboard().items(area)[1];
    let before = s.menu_state().fretboard_left_handed;
    s.handle_in(
        WindowKind::Fretboard,
        &UiEvent::PointerDown {
            pos: chip.center(),
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
    );
    assert_ne!(s.menu_state().fretboard_left_handed, before);
}

#[test]
fn the_choices_and_the_window_are_remembered() {
    let path = temp("ui-state.json");
    let _ = std::fs::remove_file(&path);
    let mut s = make(Some(path.clone()));
    s.execute(AppCommand::DetachFretboard);
    s.execute(AppCommand::FretboardLeftHanded);
    s.execute(AppCommand::SetFretLabels(FretLabels::Frets));
    s.execute(AppCommand::FretboardVoicing);
    s.flush_ui_state();
    let saved = UiState::load(&path);
    assert!(saved.windows.fretboard.detached);
    assert!(saved.fretboard_left_handed && saved.fretboard_voicing);
    assert_eq!(saved.fretboard_labels, FretLabels::Frets);
    assert!(saved.show_fretboard);
    let again = make(Some(path.clone()));
    assert!(again.windows_wanted().contains(&WindowKind::Fretboard));
    assert!(again.menu_state().fretboard_left_handed);
    assert_eq!(
        again.core().fretboard_options(),
        FretboardOptions {
            left_handed: true,
            labels: FretLabels::Frets,
            chord_voicing: true,
        }
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_ui_state_without_the_keys_has_the_defaults_and_the_labels_round_trip() {
    let state: UiState = serde_json::from_str("{}").unwrap();
    assert!(state.show_fretboard);
    assert!(!state.fretboard_left_handed && !state.fretboard_voicing);
    assert_eq!(state.fretboard_labels, FretLabels::Notes);
    assert!(!state.windows.fretboard.detached);
    let json = serde_json::to_string(&UiState {
        fretboard_labels: FretLabels::Intervals,
        ..UiState::default()
    })
    .unwrap();
    assert!(
        json.contains("\"fretboard_labels\":\"intervals\""),
        "{json}"
    );
}

#[test]
fn the_window_kind_has_a_title_and_a_name() {
    assert_eq!(
        WindowKind::Fretboard.title("Atomik Rack \u{2014} Song".to_string()),
        "Fretboard \u{2014} Song"
    );
    assert_eq!(
        WindowKind::Fretboard,
        serde_json::from_str("\"fretboard\"").unwrap()
    );
}
