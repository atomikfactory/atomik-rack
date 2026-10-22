//! `Shell` console hosting: the console in the lower pane, F5 /
//! Shift+F5, the Mixer menu and the per-frame meter feed. Headless (null audio backend).

use super::*;
use crate::commands::MenuEntry;
use crate::session::NullBackend;
use atomik_model::{Command, ConsoleCommand, DeviceKey};
use atomik_ui::{ConsoleButton, Key, Modifiers, PointerButton};

fn shell_at(ui_state_path: Option<PathBuf>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path,
            library_path: None,
        },
    );
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    s
}

fn shell() -> Shell<NullBackend> {
    shell_at(None)
}

fn key(key: Key, modifiers: Modifiers) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    }
}

/// A key press and its release (a held key would be a repeat).
fn tap(s: &mut Shell<NullBackend>, k: Key, modifiers: Modifiers) {
    s.handle(&key(k.clone(), modifiers));
    s.handle(&UiEvent::Key {
        key: k,
        modifiers,
        pressed: false,
    });
}

fn click(s: &mut Shell<NullBackend>, p: Point) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

fn add_synth(s: &mut Shell<NullBackend>, k: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(DeviceKey(k.into())),
        })
        .unwrap();
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-console-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn f5_swaps_the_arrangement_and_the_console_in_the_lower_pane() {
    let mut s = shell();
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    tap(&mut s, Key::F(5), Modifiers::NONE);
    assert_eq!(s.lower_pane(), Pane::Console);
    let l = s.layout();
    assert_eq!(Some(s.core().console().bounds()), l.song);
    assert!(l.rack.height() > 0.0, "the rack stays above");
    assert!(s.menu_state().console);
    tap(&mut s, Key::F(5), Modifiers::NONE);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    assert!(!s.menu_state().console);
}

#[test]
fn f5_shows_the_song_area_when_it_was_hidden() {
    let mut s = shell();
    s.execute(AppCommand::ToggleSong);
    assert!(s.layout().song.is_none());
    tap(&mut s, Key::F(5), Modifiers::NONE);
    assert!(s.layout().song.is_some());
    assert_eq!(s.lower_pane(), Pane::Console);
}

#[test]
fn shift_f5_maximises_the_console_over_the_rack_and_restores_it() {
    let mut s = shell();
    tap(&mut s, Key::F(5), Modifiers::SHIFT);
    assert_eq!(s.lower_pane(), Pane::Console);
    assert!(s.menu_state().console_maximised);
    let l = s.layout();
    let song = l.song.unwrap();
    assert_eq!(l.rack.height(), 0.0);
    assert!(l.divider.is_none());
    assert_eq!(song.y0, MENU_BAR_HEIGHT, "from the menu bar down");
    assert_eq!(song.y1, l.transport.y0, "to the transport bar");
    assert_eq!(s.core().console().bounds(), song);
    // Clicks at the top of the window reach the console, not the rack.
    assert_eq!(
        s.hit_pane(Point::new(300.0, MENU_BAR_HEIGHT + 20.0)),
        Some(Pane::Console)
    );
    tap(&mut s, Key::F(5), Modifiers::SHIFT);
    assert!(!s.menu_state().console_maximised);
    assert!(s.layout().rack.height() > 0.0);
    assert_eq!(s.lower_pane(), Pane::Console, "restored, still showing");
    // F5 while maximised hides the console and un-maximises.
    tap(&mut s, Key::F(5), Modifiers::SHIFT);
    tap(&mut s, Key::F(5), Modifiers::NONE);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    assert!(!s.menu_state().console_maximised);
}

#[test]
fn the_console_toolbar_buttons_toggle_and_maximise() {
    let mut s = shell();
    add_synth(&mut s, "a");
    s.execute(AppCommand::ToggleConsole);
    let max = s
        .core()
        .console()
        .button_center(ConsoleButton::Maximise)
        .unwrap();
    click(&mut s, max);
    assert!(s.menu_state().console_maximised);
    let arrange = s
        .core()
        .console()
        .button_center(ConsoleButton::Arrange)
        .unwrap();
    click(&mut s, arrange);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    assert!(!s.menu_state().console_maximised);
}

#[test]
fn clicking_the_console_focuses_it_and_keys_go_there() {
    let mut s = shell();
    add_synth(&mut s, "a");
    s.execute(AppCommand::ToggleConsole);
    let song = s.layout().song.unwrap();
    click(&mut s, Point::new(song.x0 + 40.0, song.y1 - 40.0));
    assert_eq!(s.focus(), Pane::Console);
    assert!(s.core().console().focused());
    s.execute(AppCommand::ToggleConsole);
    assert_ne!(s.focus(), Pane::Console, "a hidden pane loses the focus");
}

#[test]
fn the_mixer_menu_and_the_console_devices_toggle() {
    let mut s = shell();
    let model = s.menu_model();
    let titles: Vec<_> = model.iter().map(|m| m.title).collect();
    assert_eq!(
        titles,
        [
            "File",
            "Edit",
            "View",
            "Transport",
            "Mixer",
            "Create",
            "Audio"
        ]
    );
    let mixer: Vec<_> = model[4]
        .items()
        .map(|i| (i.label, i.shortcut.as_deref()))
        .collect();
    assert_eq!(
        mixer,
        [
            ("Console", Some("F5")),
            ("Maximise Console", Some("Shift+F5")),
            ("Add Channel\u{2026}", None),
            ("Delay Compensation", None),
        ]
    );
    assert!(model[4].entries.contains(&MenuEntry::Separator));
    let view: Vec<_> = model[2].items().map(|i| i.label).collect();
    assert!(view.contains(&"Show Console Devices in Rack"), "{view:?}");
    assert!(view.contains(&"Show Console Links"), "{view:?}");
    // Shown by default by default; the toggle hides them.
    assert!(s.core().view().show_console_devices());
    assert!(s.menu_state().console_devices);
    s.execute(AppCommand::ToggleConsoleDevices);
    assert!(!s.core().view().show_console_devices());
    assert!(!s.menu_state().console_devices);
}

#[test]
fn the_console_display_choices_persist_in_ui_state_and_an_absent_key_means_shown() {
    let path = temp("ui-state-notice.json");
    let _ = std::fs::remove_file(&path);
    let mut s = shell_at(Some(path.clone()));
    assert!(s.core().view().show_console_devices());
    assert!(!s.core().view().show_console_links());
    assert!(!s.menu_state().console_links);
    s.execute(AppCommand::ToggleConsoleLinks);
    assert!(s.core().view().show_console_links());
    assert!(s.menu_state().console_links);
    s.execute(AppCommand::ToggleConsoleDevices);
    let saved = UiState::load(&path);
    assert!(saved.show_console_links && !saved.show_console_devices);

    // A new start reads them back.
    let s2 = shell_at(Some(path.clone()));
    assert!(s2.core().view().show_console_links());
    assert!(!s2.core().view().show_console_devices());

    // A file from before the update has no keys: devices shown, links off.
    std::fs::write(&path, r#"{"rack_split": 0.4}"#).unwrap();
    let s3 = shell_at(Some(path.clone()));
    assert!(s3.core().view().show_console_devices());
    assert!(!s3.core().view().show_console_links());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_view_toggles_are_registry_commands_with_menu_items() {
    let mut s = shell();
    let links = s
        .menu_model()
        .iter()
        .flat_map(|m| m.items())
        .find(|i| i.label == "Show Console Links")
        .map(|i| (i.command, i.checked));
    assert_eq!(links, Some((AppCommand::ToggleConsoleLinks, Some(false))));
    assert_eq!(
        AppCommand::ToggleConsoleLinks.command_id(),
        Some(("view.toggle_console_links", None))
    );
    // The command runs through the key and menu path (no default key, so by id).
    s.execute(AppCommand::ToggleConsoleLinks);
    s.execute(AppCommand::ToggleConsoleLinks);
    assert!(!s.core().view().show_console_links());
}

#[test]
fn the_rack_navigator_toggles_from_the_view_menu_and_persists_in_ui_state() {
    let path = temp("ui-state-navigator.json");
    let _ = std::fs::remove_file(&path);
    let mut s = shell_at(Some(path.clone()));
    // On by default: the strip is there and the View menu item is checked.
    assert!(s.core().view().navigator_visible());
    assert!(s.menu_state().rack_navigator);
    let item = s
        .menu_model()
        .iter()
        .flat_map(|m| m.items())
        .find(|i| i.label == "Rack Navigator")
        .map(|i| (i.command, i.checked));
    assert_eq!(item, Some((AppCommand::ToggleRackNavigator, Some(true))));
    assert_eq!(
        AppCommand::ToggleRackNavigator.command_id(),
        Some(("view.toggle_rack_navigator", None))
    );
    s.execute(AppCommand::ToggleRackNavigator);
    assert!(!s.core().view().navigator_visible());
    assert!(!s.menu_state().rack_navigator);
    assert!(!UiState::load(&path).show_rack_navigator);
    // A new start reads it back; a file from before the update shows the strip.
    assert!(
        !shell_at(Some(path.clone()))
            .core()
            .view()
            .navigator_visible()
    );
    std::fs::write(&path, r#"{"rack_split": 0.4}"#).unwrap();
    assert!(
        shell_at(Some(path.clone()))
            .core()
            .view()
            .navigator_visible()
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn deleting_the_master_says_why_and_changes_nothing() {
    let mut s = shell();
    add_synth(&mut s, "a");
    let master = s.core().doc().console.master.clone().unwrap();
    let before = s.core().doc().clone();
    s.core_mut().view_mut().set_selected(Some(master));
    s.execute(AppCommand::Delete);
    assert_eq!(
        s.core().last_rejection(),
        Some("The Master Section cannot be deleted")
    );
    assert_eq!(s.core().doc(), &before);
}

#[test]
fn the_context_menu_gates_the_master_and_use_as_send_effect() {
    let mut s = shell();
    add_synth(&mut s, "a");
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.reverb".into(),
            key: Some(DeviceKey("verb".into())),
        })
        .unwrap();
    let master = s.core().doc().console.master.clone().unwrap();
    s.core_mut().view_mut().set_selected(Some(master));
    let e = s.menu_enable(Pane::Rack);
    assert!(e.master_only, "the master alone cannot be cut or deleted");
    assert!(!e.assign_send, "the master is not an effect");
    s.core_mut()
        .view_mut()
        .set_selected(Some(DeviceKey("verb".into())));
    let e = s.menu_enable(Pane::Rack);
    assert!(!e.master_only && e.assign_send, "{e:?}");
    // Once it is on a return the item is disabled.
    s.core_mut()
        .apply_command(Command::Console(ConsoleCommand::SetReturn {
            slot: 0,
            effect: Some(DeviceKey("verb".into())),
        }))
        .unwrap();
    assert!(!s.menu_enable(Pane::Rack).assign_send);
}

#[test]
fn delay_compensation_toggles_the_document_switch() {
    let mut s = shell();
    add_synth(&mut s, "a");
    assert!(s.menu_state().compensation);
    s.execute(AppCommand::ToggleCompensation);
    assert!(!s.core().doc().console.compensation);
    assert!(!s.core().topology().compensation());
    assert!(!s.menu_state().compensation);
    s.execute(AppCommand::Undo);
    assert!(s.core().doc().console.compensation);
}

#[test]
fn add_channel_shows_the_console_and_opens_its_picker() {
    let mut s = shell();
    add_synth(&mut s, "a");
    let channel = s.core().doc().console.channels[0].channel.clone();
    s.core_mut()
        .apply_command(Command::Console(ConsoleCommand::RemoveChannel { channel }))
        .unwrap();
    s.execute(AppCommand::AddChannel);
    assert_eq!(s.lower_pane(), Pane::Console);
    let items = s.core().console().menu_items();
    assert!(!items.is_empty(), "the add-channel picker is open");
}

#[test]
fn frames_feed_the_meters_only_while_the_console_shows() {
    let mut s = shell();
    add_synth(&mut s, "a");
    let ch = s.core().doc().console.channels[0].channel.clone();
    s.frame_at(0.0);
    assert!(s.core().console().meter(&ch).is_none(), "hidden: not fed");
    s.execute(AppCommand::ToggleConsole);
    s.frame_at(0.1);
    s.frame_at(0.2);
    assert!(s.core().console().meter(&ch).is_some());
}

#[test]
fn the_console_state_is_saved_in_the_ui_state() {
    let path = temp("ui-state.json");
    let _ = std::fs::remove_file(&path);
    let mut s = shell_at(Some(path.clone()));
    s.execute(AppCommand::MaximiseConsole);
    let t = shell_at(Some(path.clone()));
    assert_eq!(t.lower_pane(), Pane::Console);
    assert!(t.menu_state().console_maximised);
    std::fs::remove_file(&path).ok();
}

#[test]
fn the_shell_paints_the_console_pane() {
    let mut s = shell();
    add_synth(&mut s, "a");
    s.execute(AppCommand::ToggleConsole);
    let mut list = atomik_ui::DisplayList::new();
    s.paint(&mut list);
    assert!(!s.needs_repaint());
    s.execute(AppCommand::MaximiseConsole);
    let mut list = atomik_ui::DisplayList::new();
    s.paint(&mut list);
}

#[test]
fn the_console_view_knows_when_it_is_maximised() {
    let path = temp("ui-state-max.json");
    let _ = std::fs::remove_file(&path);
    let mut s = shell_at(Some(path.clone()));
    assert!(!s.core().console().maximised());
    tap(&mut s, Key::F(5), Modifiers::SHIFT);
    assert!(s.core().console().maximised(), "MAX is lit");
    tap(&mut s, Key::F(5), Modifiers::SHIFT);
    assert!(!s.core().console().maximised());
    s.execute(AppCommand::MaximiseConsole);
    s.execute(AppCommand::ToggleConsole);
    assert!(!s.core().console().maximised(), "hiding un-maximises");
    s.execute(AppCommand::MaximiseConsole);
    // Restored from ui-state.json at start.
    let t = shell_at(Some(path.clone()));
    assert!(t.core().console().maximised());
    // Hiding the song area shows the normal layout.
    let mut t = t;
    t.execute(AppCommand::ToggleSong);
    assert!(!t.core().console().maximised());
    std::fs::remove_file(&path).ok();
}

// ------------------------------------------------------------------ the one-time notice

#[test]
fn the_first_start_after_4m_says_console_devices_are_shown_and_only_once() {
    let path = temp("notice-ui-state.json");
    let _ = std::fs::remove_file(&path);
    // An older ui-state.json: no notice flag in it.
    std::fs::write(&path, r#"{"rack_split": 0.4}"#).unwrap();
    let s = shell_at(Some(path.clone()));
    let text = s.core().last_rejection().expect("the notice").to_string();
    assert!(
        text.contains("Console devices are now shown in the rack")
            && text.contains("View > Show Console Devices in Rack"),
        "{text}"
    );
    assert!(s.ui_state().console_notice_seen);
    // The flag is saved, with the user's other values kept.
    let saved = UiState::load(&path);
    assert!(saved.console_notice_seen && (saved.rack_split - 0.4).abs() < 1e-9);
    // The next start is quiet.
    let again = shell_at(Some(path.clone()));
    assert_eq!(again.core().last_rejection(), None);
    // Without a state file (tests, a failed config dir) nothing is said and nothing written.
    assert_eq!(shell().core().last_rejection(), None);
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_user_who_hid_the_console_devices_is_not_told_they_are_shown() {
    let path = temp("notice-hidden-ui-state.json");
    std::fs::write(&path, r#"{"show_console_devices": false}"#).unwrap();
    let s = shell_at(Some(path.clone()));
    assert_eq!(s.core().last_rejection(), None);
    assert!(
        s.ui_state().console_notice_seen,
        "nothing to say, never will be"
    );
    std::fs::remove_file(&path).ok();
}

#[test]
fn file_new_in_the_shell_makes_the_starter_project_and_is_confirmed_when_dirty() {
    let mut s = shell();
    add_synth(&mut s, "lead");
    assert!(s.core().is_dirty());
    // Dirty: New asks first.
    let requests = s.execute(AppCommand::New);
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, ShellRequest::ConfirmDiscard(AppCommand::New)))
    );
    s.confirmed(AppCommand::New);
    assert_eq!(
        atomik_model::save_project(s.core().doc()),
        atomik_model::save_project(&atomik_model::new_project_doc())
    );
    assert!(!s.core().is_dirty());
    // The master shows in the rack, first after Audio I/O.
    assert!(
        s.core()
            .view()
            .plate_center(&DeviceKey("master".into()))
            .is_some()
    );
    assert_eq!(s.core().doc().devices.len(), 1);
    assert_eq!(s.core().doc().devices[0].key, DeviceKey("master".into()));
}

// ---------------------------------------------------------------- keys and the text owner

use atomik_ui::keymap::{Context, ContextStack};

/// A console with `n` synth channels on show and focused.
fn console_with(n: usize) -> Shell<NullBackend> {
    let mut s = shell();
    for i in 0..n {
        add_synth(&mut s, &format!("s{i}"));
    }
    s.execute(AppCommand::ToggleConsole);
    s.set_focus(Pane::Console);
    s
}

fn first_channel(s: &Shell<NullBackend>) -> DeviceKey {
    s.core().doc().console.channels[0].channel.clone()
}

fn param(s: &Shell<NullBackend>, device: &DeviceKey, name: &str) -> f32 {
    // An untouched parameter is absent from the document: the fader sits at unity.
    s.core()
        .doc()
        .device(device)
        .unwrap()
        .params
        .get(name)
        .copied()
        .unwrap_or(0.75)
}

#[test]
fn the_console_context_is_mixer_until_a_name_is_typed() {
    let mut s = console_with(1);
    assert_eq!(s.context_stack(), ContextStack::new(&[Context::Mixer]));
    let channel = first_channel(&s);
    assert!(s.core_mut().console_mut().begin_rename(&channel));
    assert_eq!(
        s.context_stack(),
        ContextStack::new(&[Context::Text, Context::Mixer])
    );
    assert!(s.context_stack().is_text());
    tap(&mut s, Key::Escape, Modifiers::NONE);
    assert_eq!(s.context_stack(), ContextStack::new(&[Context::Mixer]));
}

#[test]
fn space_and_delete_do_not_leak_out_of_a_console_name() {
    let mut s = console_with(1);
    let channel = first_channel(&s);
    let devices = s.core().doc().devices.len();
    assert!(s.core_mut().console_mut().begin_rename(&channel));
    s.core_mut().select(Some(DeviceKey("s0".into())));
    for k in [Key::Space, Key::Delete, Key::Backspace, Key::Char('r')] {
        tap(&mut s, k, Modifiers::NONE);
    }
    assert!(!s.core().is_playing(), "Space is a typed space, not play");
    assert!(!s.core().is_recording());
    assert_eq!(
        s.core().doc().devices.len(),
        devices,
        "Delete deleted nothing"
    );
    assert!(
        s.core().console().is_text_owner(),
        "typing does not end the edit"
    );
}

#[test]
fn enter_renames_and_escape_cancels_through_the_shell() {
    let mut s = console_with(1);
    let channel = first_channel(&s);
    let name = |s: &Shell<NullBackend>| {
        s.core()
            .doc()
            .device(&DeviceKey("s0".into()))
            .unwrap()
            .name
            .clone()
    };
    assert!(s.core_mut().console_mut().begin_rename(&channel));
    for _ in 0..40 {
        tap(&mut s, Key::Backspace, Modifiers::NONE);
    }
    for c in "Lead".chars() {
        tap(&mut s, Key::Char(c), Modifiers::NONE);
    }
    tap(&mut s, Key::Escape, Modifiers::NONE);
    assert!(!s.core().console().is_text_owner());
    assert_ne!(name(&s), "Lead", "Escape cancels");
    assert!(s.core_mut().console_mut().begin_rename(&channel));
    for _ in 0..40 {
        tap(&mut s, Key::Backspace, Modifiers::NONE);
    }
    for c in "Lead".chars() {
        tap(&mut s, Key::Char(c), Modifiers::NONE);
    }
    tap(&mut s, Key::Enter, Modifiers::NONE);
    assert!(!s.core().console().is_text_owner());
    assert_eq!(name(&s), "Lead", "Enter commits");
    assert!(s.core().document().can_undo());
}

#[test]
fn a_command_chord_no_binding_took_is_not_typed_into_a_console_name() {
    let mut s = console_with(1);
    let channel = first_channel(&s);
    assert!(s.core_mut().console_mut().begin_rename(&channel));
    let before = s.core().console().rename_text().unwrap().to_string();
    // Ctrl+Alt+Q has no binding in any profile.
    tap(
        &mut s,
        Key::Char('q'),
        Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::NONE
        },
    );
    assert_eq!(s.core().console().rename_text(), Some(before.as_str()));
}

#[test]
fn a_command_chord_still_runs_while_a_console_name_is_typed() {
    let mut s = console_with(1);
    let channel = first_channel(&s);
    assert!(s.core_mut().console_mut().begin_rename(&channel));
    let shown = s.menu_state().console;
    tap(&mut s, Key::F(5), Modifiers::NONE);
    assert_ne!(s.menu_state().console, shown, "F5 is not typing");
}

#[test]
fn the_arrows_are_mixer_commands_that_step_the_focused_control_and_scroll() {
    let mut s = console_with(12);
    let channel = first_channel(&s);
    let p = s
        .core()
        .console()
        .control_center(&channel, "fader")
        .unwrap();
    click(&mut s, p);
    let v0 = param(&s, &channel, "fader");
    tap(&mut s, Key::Up, Modifiers::NONE);
    let v1 = param(&s, &channel, "fader");
    assert!(v1 > v0, "Up raises the focused fader");
    tap(&mut s, Key::Down, Modifiers::NONE);
    assert!(param(&s, &channel, "fader") < v1);
    tap(&mut s, Key::Down, Modifiers::SHIFT);
    tap(&mut s, Key::PageUp, Modifiers::NONE);
    assert_eq!(s.core().console().scroll_x(), 0.0);
    tap(&mut s, Key::Right, Modifiers::NONE);
    let one = s.core().console().scroll_x();
    assert!(one > 0.0, "Right scrolls the strips");
    tap(&mut s, Key::End, Modifiers::NONE);
    assert!(s.core().console().scroll_x() > one);
    tap(&mut s, Key::Home, Modifiers::NONE);
    assert_eq!(s.core().console().scroll_x(), 0.0);
}

#[test]
fn the_mixer_keys_only_run_in_the_console() {
    let mut s = console_with(12);
    s.set_focus(Pane::Rack);
    tap(&mut s, Key::Right, Modifiers::NONE);
    assert_eq!(s.core().console().scroll_x(), 0.0);
}
