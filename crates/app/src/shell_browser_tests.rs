//! `Shell` tests for the docked file browser: layout, routing and focus, the toggle command,
//! the panel's keys, persistence and the drag to other panes. They assert on geometry,
//! routing, actions and the document, never on what the core lists or previews.

use super::*;
use crate::session::NullBackend;
use atomik_ui::keymap::Os;
use atomik_ui::{FileBrowserView, Modifiers, PointerButton};

fn shell_with(os: Os, in_window_menu: bool) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(os),
            profile: None,
            in_window_menu,
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

fn shell() -> Shell<NullBackend> {
    shell_with(Os::Windows, true)
}

fn shown() -> Shell<NullBackend> {
    let mut s = shell();
    s.execute(AppCommand::ToggleFileBrowser);
    s
}

fn down(p: Point) -> UiEvent {
    UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }
}

fn up(p: Point) -> UiEvent {
    UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

fn moved(p: Point) -> UiEvent {
    UiEvent::PointerMove {
        pos: p,
        modifiers: Modifiers::NONE,
    }
}

// ------------------------------------------------------------------ layout

#[test]
fn hidden_panel_leaves_the_layout_as_it_was() {
    let s = shell();
    let l = s.layout();
    assert_eq!(l.file_browser, None);
    assert_eq!(l.rack.x0, 0.0);
    assert_eq!(l.transport.x0, 0.0);
    assert_eq!(l.song.unwrap().x0, 0.0);
}

#[test]
fn shown_panel_is_a_full_height_left_column_and_everything_else_shifts_right() {
    let s = shown();
    let l = s.layout();
    let fb = l.file_browser.expect("column");
    let width = FileBrowserView::DEFAULT_WIDTH;
    assert_eq!(fb.x0, 0.0);
    assert!((fb.width() - width).abs() < 1e-9, "{fb:?}");
    assert_eq!(fb.y0, MENU_BAR_HEIGHT, "below the menu bar");
    assert_eq!(fb.y1, 800.0 - STATUS_BAR_HEIGHT, "down to the status bar");
    assert_eq!(l.rack.x0, fb.x1);
    assert_eq!(l.divider.unwrap().x0, fb.x1);
    assert_eq!(l.transport.x0, fb.x1);
    assert_eq!(l.song.unwrap().x0, fb.x1);
    assert_eq!(l.rack.x1, 1200.0);
    assert_eq!(l.song.unwrap().x1, 1200.0);
}

#[test]
fn the_rest_of_the_window_keeps_at_least_400_px() {
    let mut s = shown();
    s.core_mut().file_browser_mut().set_width(900.0);
    s.handle(&UiEvent::Resize(Size::new(1000.0, 700.0)));
    let l = s.layout();
    assert_eq!(l.file_browser.unwrap().width(), 600.0);
    assert!(l.rack.width() >= 400.0);
    // A window too narrow for both still gives the panel what is left.
    s.handle(&UiEvent::Resize(Size::new(450.0, 700.0)));
    let l = s.layout();
    assert_eq!(l.file_browser.unwrap().width(), 50.0);
    assert_eq!(l.rack.width(), 400.0);
}

#[test]
fn the_panel_never_gets_narrower_than_its_minimum() {
    let mut s = shown();
    s.core_mut().file_browser_mut().set_width(10.0);
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    assert_eq!(
        s.layout().file_browser.unwrap().width(),
        FileBrowserView::MIN_WIDTH
    );
}

#[test]
fn the_other_layouts_shift_right_too() {
    let mut s = shown();
    let fb = s.layout().file_browser.unwrap();
    // No song area.
    s.execute(AppCommand::ToggleSong);
    let l = s.layout();
    assert_eq!((l.rack.x0, l.transport.x0), (fb.x1, fb.x1));
    s.execute(AppCommand::ToggleSong);
    // Maximised console.
    s.execute(AppCommand::MaximiseConsole);
    let l = s.layout();
    assert_eq!((l.song.unwrap().x0, l.transport.x0), (fb.x1, fb.x1));
    s.execute(AppCommand::MaximiseConsole);
    s.execute(AppCommand::ToggleConsole);
    // Rack hidden (F6).
    s.execute(AppCommand::ToggleRack);
    let l = s.layout();
    assert_eq!((l.transport.x0, l.song.unwrap().x0), (fb.x1, fb.x1));
}

#[test]
fn without_an_in_window_menu_the_column_starts_at_the_top() {
    let mut s = shell_with(Os::Mac, false);
    s.execute(AppCommand::ToggleFileBrowser);
    assert_eq!(s.layout().file_browser.unwrap().y0, 0.0);
}

#[test]
fn the_rack_view_is_sized_and_shifted_with_the_column() {
    let mut s = shown();
    let l = s.layout();
    assert_eq!(s.view_size(), l.rack.size());
    assert!((s.view_size().width - (1200.0 - FileBrowserView::DEFAULT_WIDTH)).abs() < 1e-9);
    s.execute(AppCommand::ToggleFileBrowser);
    assert!((s.view_size().width - 1200.0).abs() < 1e-9);
}

// ------------------------------------------------------------------ hit, routing, focus

fn column(s: &Shell<NullBackend>) -> Rect {
    s.layout().file_browser.expect("panel shown")
}

#[test]
fn the_column_is_its_own_pane_and_its_edge_belongs_to_the_rack() {
    let s = shown();
    let fb = column(&s);
    assert_eq!(
        s.hit_pane(Point::new(fb.x1 - 1.0, fb.center().y)),
        Some(Pane::FileBrowser)
    );
    assert_eq!(
        s.hit_pane(Point::new(fb.x0 + 1.0, fb.y0 + 1.0)),
        Some(Pane::FileBrowser)
    );
    assert_eq!(
        s.hit_pane(Point::new(fb.x1 + 1.0, s.layout().rack.center().y)),
        Some(Pane::Rack)
    );
    // The menu bar above the column and the status bar below are not panes.
    assert_eq!(s.hit_pane(Point::new(fb.x0 + 5.0, fb.y0 - 2.0)), None);
    assert_eq!(s.hit_pane(Point::new(fb.x0 + 5.0, fb.y1 + 2.0)), None);
}

#[test]
fn a_hidden_panel_is_not_hit_and_never_the_live_pane() {
    let s = shell();
    assert_ne!(s.hit_pane(Point::new(10.0, 300.0)), Some(Pane::FileBrowser));
    assert_eq!(s.live_pane(Pane::FileBrowser), Pane::Rack);
}

#[test]
fn clicking_the_panel_focuses_it_and_clicking_elsewhere_takes_the_focus_back() {
    let mut s = shown();
    let fb = column(&s);
    let inside = Point::new(fb.center().x, fb.center().y);
    s.handle(&down(inside));
    s.handle(&up(inside));
    assert_eq!(s.focus(), Pane::FileBrowser);
    let song = s.layout().song.unwrap().center();
    s.handle(&down(song));
    s.handle(&up(song));
    assert_eq!(s.focus(), Pane::Arrangement);
}

#[test]
fn the_rack_still_gets_its_clicks_with_the_column_shifted() {
    let mut s = shown();
    s.core_mut()
        .apply_command(atomik_model::Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(atomik_model::DeviceKey("synth".into())),
        })
        .unwrap();
    let key = atomik_model::DeviceKey("synth".into());
    let plate = s.core().view().plate_center(&key).unwrap();
    let o = s.layout().rack.origin();
    let window = Point::new(plate.x + o.x, plate.y + o.y);
    s.handle(&down(window));
    s.handle(&up(window));
    assert_eq!(s.core().view().selected(), Some(&key));
}

#[test]
fn dragging_the_panel_edge_resizes_the_column_live_and_beyond_its_bounds() {
    let mut s = shown();
    let fb = column(&s);
    let grip = Point::new(fb.x1 - 1.0, fb.center().y);
    s.handle(&down(grip));
    // The pointer leaves the panel: the drag keeps going.
    s.handle(&moved(Point::new(fb.x1 + 150.0, grip.y)));
    let wide = column(&s).width();
    assert!((wide - (fb.x1 + 150.0)).abs() < 1e-6, "{wide}");
    assert_eq!(s.layout().rack.x0, column(&s).x1, "everything follows");
    // The views were laid out again, not only the geometry computed.
    assert_eq!(s.core().arrangement().bounds(), s.layout().song.unwrap());
    assert_eq!(s.core().file_browser().width(), wide);
    s.handle(&up(Point::new(fb.x1 + 150.0, grip.y)));
    assert_eq!(column(&s).width(), wide);
}

#[test]
fn the_resize_drag_cannot_squeeze_the_rest_below_400_px() {
    let mut s = shown();
    let fb = column(&s);
    let grip = Point::new(fb.x1 - 1.0, fb.center().y);
    s.handle(&down(grip));
    s.handle(&moved(Point::new(1190.0, grip.y)));
    assert_eq!(column(&s).width(), 800.0);
    assert_eq!(s.layout().rack.width(), 400.0);
    assert_eq!(s.core().arrangement().bounds().width(), 400.0);
    s.handle(&up(Point::new(1190.0, grip.y)));
}

// ------------------------------------------------------------------ toggle, menu

fn tap(s: &mut Shell<NullBackend>, os: Os, chord: &str) {
    let (key, modifiers) = atomik_ui::keymap::script::event_for(os, chord).unwrap();
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

fn click_in_panel(s: &mut Shell<NullBackend>) {
    let fb = column(s);
    let p = Point::new(fb.center().x, fb.center().y);
    s.handle(&down(p));
    s.handle(&up(p));
}

fn view_menu_item(s: &Shell<NullBackend>) -> crate::commands::MenuItem {
    s.menu_model()
        .iter()
        .flat_map(|m| m.items())
        .find(|i| i.command == AppCommand::ToggleFileBrowser)
        .expect("View > File Browser")
        .clone()
}

#[test]
fn the_command_toggles_the_column_and_the_menu_check_follows() {
    let mut s = shell();
    assert_eq!(view_menu_item(&s).checked, Some(false));
    assert_eq!(view_menu_item(&s).label, "File Browser");
    assert!(!s.menu_state().file_browser);
    s.execute(AppCommand::ToggleFileBrowser);
    assert!(s.layout().file_browser.is_some());
    assert_eq!(view_menu_item(&s).checked, Some(true));
    assert!(s.ui_state().file_browser_visible);
    s.execute(AppCommand::ToggleFileBrowser);
    assert!(s.layout().file_browser.is_none());
    assert_eq!(view_menu_item(&s).checked, Some(false));
}

#[test]
fn the_view_menu_shows_f9_for_it() {
    let s = shell();
    let want = if s.profile() == Profile::Standard {
        "F9"
    } else {
        "F4"
    };
    assert_eq!(view_menu_item(&s).shortcut.as_deref(), Some(want));
}

#[test]
fn the_file_browser_key_toggles_it_on_every_os_and_profile() {
    for os in [Os::Windows, Os::Linux, Os::Mac] {
        for profile in Profile::ALL {
            let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
            let mut s = Shell::new(
                AppCore::new(session),
                ShellOptions {
                    os: Some(os),
                    profile: Some(profile),
                    in_window_menu: true,
                    ..ShellOptions::default()
                },
            );
            s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
            let f = if profile == Profile::Standard {
                "F9"
            } else {
                "F4"
            };
            tap(&mut s, os, f);
            assert!(s.layout().file_browser.is_some(), "{os:?} {profile:?}");
            tap(&mut s, os, f);
            assert!(s.layout().file_browser.is_none(), "{os:?} {profile:?}");
        }
    }
}

#[test]
fn hiding_the_panel_moves_the_focus_off_it() {
    let mut s = shown();
    click_in_panel(&mut s);
    assert_eq!(s.focus(), Pane::FileBrowser);
    s.execute(AppCommand::ToggleFileBrowser);
    assert_ne!(s.focus(), Pane::FileBrowser);
    assert_eq!(s.focus(), Pane::Rack);
    // Showing it again does not steal the focus.
    s.execute(AppCommand::ToggleFileBrowser);
    assert_eq!(s.focus(), Pane::Rack);
}

#[test]
fn hiding_the_panel_leaves_another_focus_alone() {
    let mut s = shown();
    let song = s.layout().song.unwrap().center();
    s.handle(&down(song));
    s.handle(&up(song));
    s.execute(AppCommand::ToggleFileBrowser);
    assert_eq!(s.focus(), Pane::Arrangement);
}

#[test]
fn the_toggle_key_is_a_command_even_while_the_panel_has_the_focus() {
    let mut s = shown();
    click_in_panel(&mut s);
    let f = if s.profile() == Profile::Standard {
        "F9"
    } else {
        "F4"
    };
    tap(&mut s, Os::Windows, f);
    assert!(s.layout().file_browser.is_none());
    assert_ne!(s.focus(), Pane::FileBrowser);
}

// ------------------------------------------------------------------ keys and context

fn focused_panel(os: Os) -> Shell<NullBackend> {
    let mut s = shell_with(os, true);
    s.execute(AppCommand::ToggleFileBrowser);
    click_in_panel(&mut s);
    assert_eq!(s.focus(), Pane::FileBrowser);
    s
}

#[test]
fn the_focused_panel_is_the_files_context_and_a_text_field_puts_text_on_top() {
    let mut s = focused_panel(Os::Windows);
    assert_eq!(s.context_stack(), ContextStack::new(&[Context::Files]));
    s.core_mut().file_browser_mut().focus_search();
    assert!(s.core().file_browser().is_text_owner());
    assert_eq!(
        s.context_stack(),
        ContextStack::new(&[Context::Text, Context::Files])
    );
}

#[test]
fn space_previews_instead_of_playing_while_the_panel_has_the_focus() {
    let mut s = focused_panel(Os::Windows);
    tap(&mut s, Os::Windows, "Space");
    assert!(!s.core().is_playing(), "Space is the panel's, not play");
    // Control: the same key plays with the rack focused.
    let mut r = shell();
    tap(&mut r, Os::Windows, "Space");
    assert!(r.core().is_playing());
}

#[test]
fn global_chords_still_run_with_the_panel_focused() {
    let mut s = focused_panel(Os::Windows);
    tap(&mut s, Os::Windows, "F5");
    assert!(s.menu_state().console, "F5 still toggles the console");
    s.core_mut()
        .apply_command(atomik_model::Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(atomik_model::DeviceKey("synth".into())),
        })
        .unwrap();
    assert!(s.menu_state().can_undo);
    tap(&mut s, Os::Windows, "Cmd+Z");
    assert!(!s.menu_state().can_undo, "Ctrl+Z undoes");
    let requests = tap_requests(&mut s, Os::Windows, "Cmd+S");
    assert_eq!(requests, vec![ShellRequest::PickSaveFile], "Ctrl+S saves");
}

fn tap_requests(s: &mut Shell<NullBackend>, os: Os, chord: &str) -> Vec<ShellRequest> {
    let (key, modifiers) = atomik_ui::keymap::script::event_for(os, chord).unwrap();
    let r = s.handle(&UiEvent::Key {
        key: key.clone(),
        modifiers,
        pressed: true,
    });
    s.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: false,
    });
    r
}

#[test]
fn the_search_chord_focuses_the_panels_search_instead_of_the_device_browser() {
    for os in [Os::Windows, Os::Mac] {
        let mut s = focused_panel(os);
        let device_browser = s.core().view().browser_visible();
        tap(&mut s, os, "Cmd+F");
        assert!(s.core().file_browser().is_text_owner(), "{os:?}");
        assert_eq!(s.core().view().browser_visible(), device_browser, "{os:?}");
    }
}

#[test]
fn escape_reaches_the_search_field_first_and_leaves_it() {
    let mut s = focused_panel(Os::Windows);
    s.core_mut().file_browser_mut().focus_search();
    assert!(s.core().file_browser().is_text_owner());
    tap(&mut s, Os::Windows, "Escape");
    assert!(
        !s.core().file_browser().is_text_owner(),
        "Esc left the field"
    );
    assert_eq!(
        s.focus(),
        Pane::FileBrowser,
        "and the panel keeps the focus"
    );
}

#[test]
fn typing_in_the_search_field_is_not_a_shortcut_but_chords_and_function_keys_are() {
    let mut s = focused_panel(Os::Windows);
    s.core_mut().file_browser_mut().focus_search();
    tap(&mut s, Os::Windows, "Space");
    tap(&mut s, Os::Windows, "F");
    tap(&mut s, Os::Windows, "Backspace");
    assert!(!s.core().is_playing(), "Space was typed, not play");
    assert!(s.core().file_browser().is_text_owner());
    // Chords with the primary modifier and function keys still run.
    let requests = tap_requests(&mut s, Os::Windows, "Cmd+S");
    assert_eq!(requests, vec![ShellRequest::PickSaveFile]);
    tap(&mut s, Os::Windows, "F5");
    assert!(s.menu_state().console);
}

#[test]
fn every_files_command_is_taken_by_the_shell_and_declines_nothing() {
    let mut s = focused_panel(Os::Windows);
    for id in [
        "files.up",
        "files.down",
        "files.left",
        "files.right",
        "files.page_up",
        "files.page_down",
        "files.home",
        "files.end",
        "files.open",
        "files.stop",
        "files.toggle_preview",
        "files.parent",
        "files.favorite",
        "files.rename",
        "files.search",
        "files.select_all",
    ] {
        assert_eq!(s.run_command(id, None), Some(Vec::new()), "{id}");
    }
}

#[test]
fn the_panel_keys_follow_a_remap_because_commands_not_chords_are_forwarded() {
    // The user moves "Search Files" to a letter: the shell forwards the same panel key.
    let mut s = focused_panel(Os::Windows);
    s.run_command("files.search", None);
    assert!(s.core().file_browser().is_text_owner());
}

// ------------------------------------------------------------------ persistence

fn state_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "atomik-shell-browser-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn shell_at(path: &Path) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: Some(path.to_path_buf()),
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    s
}

fn saved(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn a_fresh_start_hides_the_panel_with_the_default_layout_values() {
    let dir = state_dir("fresh");
    let s = shell_at(&dir.join("ui-state.json"));
    assert!(s.layout().file_browser.is_none());
    let v = s.core().file_browser();
    assert_eq!(v.width(), FileBrowserView::DEFAULT_WIDTH);
    assert!((v.tree_fraction() - 0.4).abs() < 1e-9);
    assert!(v.auto_preview());
    assert!(!v.recursive());
}

#[test]
fn the_saved_panel_state_is_restored_at_start() {
    let dir = state_dir("restore");
    let path = dir.join("ui-state.json");
    std::fs::write(
        &path,
        r#"{"file_browser_visible": true, "file_browser_width": 380.0,
            "file_browser_tree_fraction": 0.25, "file_browser_auto_preview": false,
            "file_browser_folder": "C:/samples/drums"}"#,
    )
    .unwrap();
    let s = shell_at(&path);
    assert_eq!(s.layout().file_browser.unwrap().width(), 380.0);
    assert!(s.menu_state().file_browser);
    let v = s.core().file_browser();
    assert_eq!(v.width(), 380.0);
    assert!((v.tree_fraction() - 0.25).abs() < 1e-9);
    assert!(!v.auto_preview());
    assert!(!v.recursive(), "the sub-folder search is never persisted");
    assert_eq!(
        s.ui_state().file_browser_folder.as_deref(),
        Some(Path::new("C:/samples/drums"))
    );
}

#[test]
fn a_state_file_from_before_the_panel_still_loads() {
    let dir = state_dir("old");
    let path = dir.join("ui-state.json");
    std::fs::write(&path, r#"{"rack_split": 0.3, "show_song": false}"#).unwrap();
    let s = shell_at(&path);
    assert!(s.layout().file_browser.is_none());
    assert!(!s.ui_state().show_song);
    assert_eq!(
        s.core().file_browser().width(),
        FileBrowserView::DEFAULT_WIDTH
    );
}

#[test]
fn toggling_saves_the_visibility() {
    let dir = state_dir("toggle");
    let path = dir.join("ui-state.json");
    let mut s = shell_at(&path);
    s.execute(AppCommand::ToggleFileBrowser);
    assert_eq!(saved(&path)["file_browser_visible"], true);
    s.execute(AppCommand::ToggleFileBrowser);
    assert_eq!(saved(&path)["file_browser_visible"], false);
}

#[test]
fn ending_a_resize_drag_saves_width_and_split() {
    let dir = state_dir("drag");
    let path = dir.join("ui-state.json");
    let mut s = shell_at(&path);
    s.execute(AppCommand::ToggleFileBrowser);
    let fb = column(&s);
    let grip = Point::new(fb.x1 - 1.0, fb.center().y);
    s.handle(&down(grip));
    s.handle(&moved(Point::new(420.0, grip.y)));
    s.handle(&up(Point::new(420.0, grip.y)));
    let json = saved(&path);
    assert_eq!(json["file_browser_width"], 420.0);
    assert!(json["file_browser_tree_fraction"].is_number());
    assert!(json.get("file_browser_recursive").is_none());
    // The next run opens at that width.
    assert_eq!(
        shell_at(&path).layout().file_browser.unwrap().width(),
        420.0
    );
}

#[test]
fn a_width_the_window_could_not_show_is_saved_as_shown() {
    let dir = state_dir("clamped");
    let path = dir.join("ui-state.json");
    let mut s = shell_at(&path);
    s.execute(AppCommand::ToggleFileBrowser);
    let fb = column(&s);
    let grip = Point::new(fb.x1 - 1.0, fb.center().y);
    s.handle(&down(grip));
    s.handle(&moved(Point::new(1190.0, grip.y)));
    s.handle(&up(Point::new(1190.0, grip.y)));
    assert_eq!(saved(&path)["file_browser_width"], 800.0);
}

#[test]
fn the_auto_preview_switch_is_saved() {
    let dir = state_dir("auto");
    let path = dir.join("ui-state.json");
    let mut s = shell_at(&path);
    s.core_mut().file_browser_mut().set_auto_preview(false);
    s.pane_actions(&[UiAction::Browse(atomik_ui::BrowseAction::SetAutoPreview(
        false,
    ))]);
    assert_eq!(saved(&path)["file_browser_auto_preview"], false);
    assert!(!shell_at(&path).core().file_browser().auto_preview());
}

#[test]
fn showing_a_folder_saves_it_as_the_last_folder() {
    let dir = state_dir("folder");
    let path = dir.join("ui-state.json");
    let mut s = shell_at(&path);
    let folder = dir.join("kicks");
    s.pane_actions(&[UiAction::Browse(atomik_ui::BrowseAction::Show(
        atomik_ui::Place::Folder(folder.clone()),
    ))]);
    assert_eq!(
        saved(&path)["file_browser_folder"],
        folder.to_string_lossy().as_ref()
    );
    // Other places (favourites, a tag) are not folders: the last folder stays.
    s.pane_actions(&[UiAction::Browse(atomik_ui::BrowseAction::Show(
        atomik_ui::Place::Favorites,
    ))]);
    assert_eq!(
        saved(&path)["file_browser_folder"],
        folder.to_string_lossy().as_ref()
    );
}

// ------------------------------------------------------------------ drag to other panes

fn wav(name: &str, frames: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-shell-browser-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let data: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
    atomik_audio_file::write_wav_f32(
        &path,
        &atomik_audio_file::AudioData::new(48_000, vec![data]).unwrap(),
    )
    .unwrap();
    path
}

fn clip_count(s: &Shell<NullBackend>) -> usize {
    s.core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .map(|l| l.clips.len())
        .sum()
}

/// A window point over an audio lane of the arrangement.
fn lane_point(s: &Shell<NullBackend>) -> Point {
    let song = s.layout().song.unwrap();
    let mut y = song.y0;
    while y < song.y1 {
        let p = Point::new(song.x0 + 500.0, y);
        if matches!(
            s.core().arrangement().drop_target_at(p),
            Some((atomik_ui::ImportTarget::ExistingTrack(_), _))
        ) {
            return p;
        }
        y += 2.0;
    }
    panic!("no audio lane found");
}

/// The shell with the panel shown and one audio track with a clip to drop on.
fn drag_shell(tag: &str) -> Shell<NullBackend> {
    let mut s = shown();
    s.import_files(vec![wav(&format!("first-{tag}.wav"), 24_000)]);
    s.core_mut().wait_background();
    assert_eq!(clip_count(&s), 1);
    s
}

/// Starts a drag the way the panel does: a press in it, then its `DragFiles` action.
fn start_drag(s: &mut Shell<NullBackend>, paths: Vec<PathBuf>) {
    let fb = column(s);
    let from = Point::new(fb.center().x, fb.center().y);
    s.handle(&down(from));
    s.pane_actions(&[UiAction::Browse(atomik_ui::BrowseAction::DragFiles(paths))]);
}

#[test]
fn dropping_a_browser_drag_on_a_lane_is_the_same_as_an_os_drop_there() {
    let paths = vec![wav("second.wav", 12_000)];
    let mut dragged = drag_shell("lane");
    let mut dropped = drag_shell("lane");
    let at = lane_point(&dragged);
    assert_eq!(at, lane_point(&dropped));

    start_drag(&mut dragged, paths.clone());
    dragged.handle(&moved(Point::new(at.x - 30.0, at.y)));
    dragged.handle(&moved(at));
    assert!(
        dragged.core().arrangement().drop_hover().is_some(),
        "hover indicator"
    );
    dragged.handle(&up(at));
    assert!(dragged.core().arrangement().drop_hover().is_none());
    dragged.core_mut().wait_background();

    dropped.handle(&UiEvent::FileDrop {
        pos: at,
        paths: paths.clone(),
    });
    dropped.core_mut().wait_background();

    assert_eq!(clip_count(&dragged), 2, "the file landed on the lane");
    assert_eq!(
        format!("{:?}", dragged.core().doc().song),
        format!("{:?}", dropped.core().doc().song),
        "same document change as the OS drop"
    );
    assert_eq!(dragged.core().doc().song.tracks.len(), 1, "no new track");
}

#[test]
fn several_files_drop_like_an_os_drop_of_the_same_files() {
    let paths = vec![wav("a.wav", 4_800), wav("b.wav", 4_800)];
    let mut dragged = drag_shell("several");
    let mut dropped = drag_shell("several");
    let at = lane_point(&dragged);
    start_drag(&mut dragged, paths.clone());
    dragged.handle(&moved(at));
    dragged.handle(&up(at));
    dragged.core_mut().wait_background();
    dropped.handle(&UiEvent::FileDrop { pos: at, paths });
    dropped.core_mut().wait_background();
    assert_eq!(clip_count(&dragged), 3);
    assert_eq!(
        format!("{:?}", dragged.core().doc().song),
        format!("{:?}", dropped.core().doc().song)
    );
}

#[test]
fn a_browser_drag_onto_a_sampler_panel_loads_zones_like_an_os_drop() {
    let key = atomik_model::DeviceKey("smp".into());
    let make = || {
        let mut s = shown();
        s.core_mut()
            .apply_command(atomik_model::Command::AddDevice {
                index: 0,
                type_id: "atomik.sampler".into(),
                key: Some(key.clone()),
            })
            .unwrap();
        s
    };
    let panel_point = |s: &Shell<NullBackend>| {
        let r = s.core().view().device_rect(&key).unwrap();
        let p = s
            .core()
            .view()
            .rack_to_view(Point::new(r.x0 + 6.0, r.center().y));
        let o = s.layout().rack.origin();
        Point::new(p.x + o.x, p.y + o.y)
    };
    let paths = vec![wav("tone_C3.wav", 2_400), wav("tone_E3.wav", 2_400)];
    let mut dragged = make();
    let mut dropped = make();
    let at = panel_point(&dragged);
    start_drag(&mut dragged, paths.clone());
    dragged.handle(&moved(at));
    assert_eq!(dragged.core().view().drop_hover(), Some(&key));
    dragged.handle(&up(at));
    dragged.core_mut().wait_background();
    dropped.handle(&UiEvent::FileDrop { pos: at, paths });
    dropped.core_mut().wait_background();
    let roots = |s: &Shell<NullBackend>| -> Vec<u8> {
        s.core()
            .doc()
            .device(&key)
            .unwrap()
            .zones
            .iter()
            .map(|z| z.root_key)
            .collect()
    };
    assert_eq!(roots(&dragged), [48, 52]);
    assert_eq!(roots(&dragged), roots(&dropped));
    assert!(
        dragged.core().doc().song.tracks.is_empty(),
        "no tracks made"
    );
}

#[test]
fn leaving_the_target_or_going_over_the_panel_cancels_the_hover() {
    let mut s = drag_shell("leave");
    let at = lane_point(&s);
    start_drag(&mut s, vec![wav("x.wav", 100)]);
    s.handle(&moved(at));
    assert!(s.core().arrangement().drop_hover().is_some());
    // The rack takes files too, so the arrangement lets go of the hover.
    s.handle(&moved(s.layout().rack.center()));
    assert!(s.core().arrangement().drop_hover().is_none());
    s.handle(&moved(at));
    assert!(s.core().arrangement().drop_hover().is_some());
    // Back over the panel: no target at all.
    let fb = column(&s);
    let over_panel = Point::new(fb.center().x, fb.center().y);
    s.handle(&moved(over_panel));
    assert!(s.core().arrangement().drop_hover().is_none());
    s.handle(&up(over_panel));
}

#[test]
fn releasing_over_the_panel_or_outside_any_target_drops_nothing() {
    for release in [0, 1, 2] {
        let mut s = drag_shell("release");
        let at = lane_point(&s);
        let fb = column(&s);
        let nowhere = match release {
            0 => Point::new(fb.center().x, fb.center().y),
            1 => s.layout().transport.center(),
            _ => Point::new(5.0, 790.0), // the status bar
        };
        start_drag(&mut s, vec![wav("y.wav", 100)]);
        s.handle(&moved(at));
        s.handle(&moved(nowhere));
        s.handle(&up(nowhere));
        s.core_mut().wait_background();
        assert_eq!(clip_count(&s), 1, "release {release}");
        assert_eq!(s.core().doc().song.tracks.len(), 1, "release {release}");
        assert!(s.core().arrangement().drop_hover().is_none());
    }
}

#[test]
fn escape_cancels_the_drag_and_the_release_that_follows_is_swallowed() {
    let mut s = drag_shell("esc");
    let at = lane_point(&s);
    start_drag(&mut s, vec![wav("z.wav", 100)]);
    s.handle(&moved(at));
    assert!(s.core().arrangement().drop_hover().is_some());
    tap(&mut s, Os::Windows, "Escape");
    assert!(s.core().arrangement().drop_hover().is_none());
    s.handle(&moved(at));
    assert!(
        s.core().arrangement().drop_hover().is_none(),
        "no hover after the cancel"
    );
    s.handle(&up(at));
    s.core_mut().wait_background();
    assert_eq!(clip_count(&s), 1);
    // The drag is over: an ordinary click works again.
    assert_eq!(s.capture, None);
}

#[test]
fn the_drag_label_names_one_file_or_counts_several() {
    assert_eq!(drag_label(&[PathBuf::from("a/b/kick.wav")]), "kick.wav");
    assert_eq!(
        drag_label(&[PathBuf::from("a.wav"), PathBuf::from("b.wav")]),
        "2 files"
    );
    assert_eq!(drag_label(&vec![PathBuf::from("x.wav"); 3]), "3 files");
}

#[test]
fn the_label_shows_while_dragging_and_goes_with_the_drag() {
    let mut s = drag_shell("label");
    let at = lane_point(&s);
    start_drag(&mut s, vec![wav("snare.wav", 100)]);
    s.handle(&moved(at));
    let mut list = atomik_ui::DisplayList::new();
    s.paint(&mut list);
    assert!(list.texts().contains(&"snare.wav"), "{:?}", list.texts());
    s.handle(&up(s.layout().transport.center()));
    let mut list = atomik_ui::DisplayList::new();
    s.paint(&mut list);
    assert!(!list.texts().contains(&"snare.wav"));
}

#[test]
fn a_press_in_the_panel_alone_is_not_a_drag() {
    let mut s = drag_shell("press");
    let fb = column(&s);
    let p = Point::new(fb.center().x, fb.center().y);
    s.handle(&down(p));
    s.handle(&moved(lane_point(&s)));
    assert!(s.core().arrangement().drop_hover().is_none());
    s.handle(&up(p));
}

// ------------------------------------------------------------------ library path, shutdown

#[test]
fn the_library_path_option_reaches_the_core_and_shutdown_flushes_it() {
    let dir = state_dir("library");
    let library = dir.join("library.json");
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            in_window_menu: true,
            library_path: Some(library.clone()),
            library_root: None,
            ..ShellOptions::default()
        },
    );
    let song = dir.join("kick.wav");
    s.core_mut()
        .browse(atomik_ui::BrowseAction::ToggleFavorite(vec![song]));
    s.shutdown();
    assert!(library.exists(), "the favourite was written on exit");
}

#[test]
fn shutdown_saves_the_window_state_with_the_panel_values() {
    let dir = state_dir("shutdown");
    let path = dir.join("ui-state.json");
    let mut s = shell_at(&path);
    s.core_mut().file_browser_mut().set_tree_fraction(0.6);
    s.shutdown();
    assert_eq!(saved(&path)["file_browser_tree_fraction"], 0.6);
}

#[test]
fn the_default_library_file_sits_next_to_the_settings_file() {
    assert_eq!(
        crate::ui_state::library_path(Path::new("cfg/settings.json")),
        Path::new("cfg").join("library.json")
    );
}

#[test]
fn the_platform_tick_keeps_running_while_the_core_is_busy() {
    // The loop in platform/app.rs keeps a frame deadline while `is_busy`; the shell delegates.
    let s = shell();
    assert_eq!(s.is_busy(), s.core().is_busy());
}

// ------------------------------------------------------------------ show in folder

#[test]
fn show_in_folder_becomes_a_request_for_the_platform() {
    let mut s = shown();
    s.take_requests();
    let file = PathBuf::from("/music/kick.wav");
    s.pane_actions(&[UiAction::Browse(atomik_ui::BrowseAction::Reveal(
        file.clone(),
    ))]);
    assert_eq!(s.take_requests(), vec![ShellRequest::Reveal(file)]);
}

#[test]
fn the_panels_path_bar_goes_back_and_forward_through_the_places_shown() {
    let mut s = shown();
    let fb = column(&s);
    // Three places, shown by the core as the panel asks for them.
    for place in [Place::Favorites, Place::Tag("808".into()), Place::Favorites] {
        s.core_mut().file_browser_mut().set_location(place);
    }
    s.core_mut()
        .file_browser_mut()
        .set_location(Place::Tag("kick".into()));
    // The back button is at the left of the path bar, below the tree: find it by its tooltip.
    let view = s.core().file_browser();
    assert_eq!(view.back_place(), Some(&Place::Favorites));
    assert_eq!(view.forward_place(), None);
    let probe = (0..fb.height() as usize)
        .map(|dy| Point::new(fb.x0 + 12.0, fb.y0 + dy as f64))
        .find(|p| {
            s.core()
                .file_browser()
                .help_at(*p)
                .is_some_and(|h| h.text.contains("previous"))
        })
        .expect("the back button");
    s.take_requests();
    s.handle(&down(probe));
    s.handle(&up(probe));
    // The core showed the place the button asked for, and the history kept the rest.
    let view = s.core().file_browser();
    assert_eq!(view.location(), Some(&Place::Favorites));
    assert_eq!(view.forward_place(), Some(&Place::Tag("kick".into())));
    assert_eq!(view.back_place(), Some(&Place::Tag("808".into())));
}
