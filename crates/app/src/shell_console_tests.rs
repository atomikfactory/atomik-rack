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
            library_root: None,
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
            "Track",
            "Audio",
            "Options",
            "Help"
        ]
    );
    let mixer: Vec<_> = model[4]
        .items()
        .map(|i| (i.label.as_ref(), i.shortcut.as_deref()))
        .collect();
    assert_eq!(
        mixer,
        [
            ("Console", Some("F5")),
            ("Maximise Console", Some("Shift+F5")),
            ("Add Channel\u{2026}", None),
            ("Route to New Output Bus", Some("Ctrl+G")),
            ("Create VCA from Selection", None),
            ("Add to VCA", None),
            ("Remove from VCA", None),
            ("Delete VCA", None),
            ("Toggle Polarity", None),
            ("Bypass All Inserts", None),
            ("Store Snapshot", None),
            ("Update Snapshot", None),
            ("Recall Snapshot", None),
            ("Recall Next Snapshot", None),
            ("Recall Previous Snapshot", None),
            ("Compare A/B", None),
            ("End Compare", None),
            ("Delete Snapshot", None),
            ("Delay Compensation", None),
            ("MIDI Learn", None),
            ("MIDI Mappings\u{2026}", None),
            ("Export MIDI Mapping Preset\u{2026}", None),
            ("Import MIDI Mapping Preset\u{2026}", None),
            ("Clear MIDI Mappings", None),
            ("Macros\u{2026}", None),
            ("New Macro Bank", None),
        ]
    );
    assert!(model[4].entries.contains(&MenuEntry::Separator));
    let view: Vec<_> = model[2].items().map(|i| i.label.as_ref()).collect();
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
    assert!(requests.iter().any(|r| matches!(
        r,
        ShellRequest::ConfirmSave {
            then: PendingAction::Command(AppCommand::New)
        }
    )));
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

#[test]
fn ctrl_g_routes_the_selected_instruments_to_a_new_output_bus() {
    use atomik_model::console::OutputDest;
    let mut s = shell();
    add_synth(&mut s, "a");
    add_synth(&mut s, "b");
    s.core_mut()
        .view_mut()
        .set_selection(&[DeviceKey("a".into())], Some(&DeviceKey("a".into())));
    tap(&mut s, Key::Char('g'), Modifiers::CTRL);
    let doc = s.core().doc();
    assert_eq!(doc.console.buses.len(), 1);
    let ch = doc
        .console
        .channel_of(&DeviceKey("a".into()))
        .unwrap()
        .channel
        .clone();
    assert_eq!(
        atomik_model::console::output_dest(doc, &ch),
        OutputDest::Bus(doc.console.buses[0].bus.clone())
    );
    let other = doc
        .console
        .channel_of(&DeviceKey("b".into()))
        .unwrap()
        .channel
        .clone();
    assert_eq!(
        atomik_model::console::output_dest(doc, &other),
        OutputDest::Master
    );
}

#[test]
fn route_to_new_bus_is_a_mixer_menu_item_with_the_shortcut() {
    let s = shell();
    let menus = s.menu_model();
    let mixer = menus.iter().find(|m| m.title == "Mixer").unwrap();
    let item = mixer
        .items()
        .find(|i| i.command == AppCommand::RouteToNewBus)
        .expect("in the Mixer menu");
    assert_eq!(item.label, "Route to New Output Bus");
    assert_eq!(item.shortcut.as_deref(), Some("Ctrl+G"));
    assert_eq!(
        AppCommand::RouteToNewBus.command_id(),
        Some(("mixer.route_to_new_bus", None))
    );
    assert_eq!(
        AppCommand::from_command("mixer.route_to_new_bus", None),
        Some(AppCommand::RouteToNewBus)
    );
}

#[test]
fn the_console_is_a_text_context_while_a_bus_name_is_edited() {
    let mut s = shell();
    add_synth(&mut s, "a");
    s.core_mut()
        .apply_command(Command::Console(ConsoleCommand::CreateBus {
            name: "Drums".into(),
            dest: atomik_model::console::OutputDest::Master,
        }))
        .unwrap();
    tap(&mut s, Key::F(5), Modifiers::NONE);
    let bus = DeviceKey("bus".into());
    let p = s
        .core()
        .console()
        .name_center(&bus)
        .expect("bus strip shown");
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 2,
    });
    assert!(s.core().console().is_text_owner());
    assert!(s.context_stack().is_text());
    // Letters go to the name, not to a command; Enter commits through the core.
    for ch in "xy".chars() {
        tap(&mut s, Key::Char(ch), Modifiers::NONE);
    }
    assert_eq!(s.core().console().rename_text(), Some("Drumsxy"));
    tap(&mut s, Key::Enter, Modifiers::NONE);
    assert_eq!(s.core().doc().console.buses[0].name, "Drumsxy");
    assert!(!s.core().console().is_text_owner());
}

fn click_with(s: &mut Shell<NullBackend>, p: Point, modifiers: Modifiers, clicks: u8) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers,
        clicks,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers,
    });
}

/// A shell with the console shown and synths `a` and `b`; their channel keys.
fn console_with_two_channels() -> (Shell<NullBackend>, DeviceKey, DeviceKey) {
    let mut s = shell();
    add_synth(&mut s, "a");
    add_synth(&mut s, "b");
    s.execute(AppCommand::ToggleConsole);
    let doc = s.core().doc();
    let ch = |k: &str| {
        doc.console
            .channel_of(&DeviceKey(k.into()))
            .unwrap()
            .channel
            .clone()
    };
    let (a, b) = (ch("a"), ch("b"));
    (s, a, b)
}

fn click_name(s: &mut Shell<NullBackend>, strip: &DeviceKey, modifiers: Modifiers, clicks: u8) {
    let p = s.core().console().name_center(strip).expect("strip shown");
    click_with(s, p, modifiers, clicks);
}

#[test]
fn a_plain_click_on_one_strip_moves_the_focus_to_the_rack() {
    let (mut s, a, _) = console_with_two_channels();
    click_name(&mut s, &a, Modifiers::NONE, 1);
    assert_eq!(s.core().console().selected_strips(), [a]);
    assert_eq!(s.focus(), Pane::Rack);
    assert!(!s.core().console().focused());
}

#[test]
fn a_multi_select_click_keeps_the_console_focus_and_ctrl_g_routes_every_strip() {
    use atomik_model::console::OutputDest;
    let (mut s, a, b) = console_with_two_channels();
    click_name(&mut s, &a, Modifiers::CTRL, 1);
    assert_eq!(
        s.focus(),
        Pane::Console,
        "a modifier click keeps the console"
    );
    click_name(&mut s, &b, Modifiers::CTRL, 1);
    assert_eq!(s.core().console().selected_strips().len(), 2);
    assert_eq!(s.focus(), Pane::Console);
    tap(&mut s, Key::Char('g'), Modifiers::CTRL);
    let doc = s.core().doc();
    assert_eq!(doc.console.buses.len(), 1);
    let bus = doc.console.buses[0].bus.clone();
    for ch in [&a, &b] {
        assert_eq!(
            atomik_model::console::output_dest(doc, ch),
            OutputDest::Bus(bus.clone())
        );
    }
}

#[test]
fn shift_range_and_double_click_rename_keep_the_console_focus() {
    let (mut s, a, b) = console_with_two_channels();
    click_name(&mut s, &a, Modifiers::SHIFT, 1);
    click_name(&mut s, &b, Modifiers::SHIFT, 1);
    assert_eq!(s.focus(), Pane::Console);
    // Back to one plain click (rack focus), then a double-click edits the name in the console.
    click_name(&mut s, &a, Modifiers::NONE, 1);
    assert_eq!(s.focus(), Pane::Rack);
    click_name(&mut s, &a, Modifiers::NONE, 2);
    assert_eq!(s.focus(), Pane::Console);
    assert!(s.core().console().is_text_owner());
}

#[test]
fn the_vca_commands_are_mixer_menu_items_without_keys_and_name_a_group_by_id() {
    let s = shell();
    let menus = s.menu_model();
    let mixer = menus.iter().find(|m| m.title == "Mixer").unwrap();
    for (label, command, id) in [
        (
            "Create VCA from Selection",
            AppCommand::CreateVca,
            "mixer.create_vca",
        ),
        ("Add to VCA", AppCommand::AddToVca(None), "mixer.add_to_vca"),
        (
            "Remove from VCA",
            AppCommand::RemoveFromVca,
            "mixer.remove_from_vca",
        ),
        (
            "Delete VCA",
            AppCommand::DeleteVca(None),
            "mixer.delete_vca",
        ),
    ] {
        let item = mixer
            .items()
            .find(|i| i.command == command)
            .unwrap_or_else(|| panic!("{label} is in the Mixer menu"));
        assert_eq!(item.label, label);
        assert_eq!(item.shortcut, None, "no default key");
        assert!(!item.enabled, "nothing is selected, and there is no group");
        assert_eq!(command.command_id(), Some((id, None)));
        assert_eq!(AppCommand::from_command(id, None), Some(command));
    }
    // The group is the argument; a command that names one has no static id.
    assert_eq!(
        AppCommand::from_command("mixer.add_to_vca", Some("3")),
        Some(AppCommand::AddToVca(Some(3)))
    );
    assert_eq!(
        AppCommand::from_command("mixer.delete_vca", Some("7")),
        Some(AppCommand::DeleteVca(Some(7)))
    );
    assert_eq!(
        AppCommand::from_command("mixer.add_to_vca", Some("x")),
        None
    );
    assert_eq!(AppCommand::AddToVca(Some(3)).command_id(), None);
}

#[test]
fn the_vca_menu_items_enable_with_the_selection_and_run_through_the_shell() {
    let mut s = shell();
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(DeviceKey("a".into())),
        })
        .unwrap();
    s.core_mut()
        .view_mut()
        .set_selection(&[DeviceKey("a".into())], Some(&DeviceKey("a".into())));
    let enabled = |s: &Shell<NullBackend>, command: AppCommand| {
        s.menu_model()
            .iter()
            .flat_map(|m| m.items())
            .find(|i| i.command == command)
            .map(|i| i.enabled)
    };
    assert_eq!(enabled(&s, AppCommand::CreateVca), Some(true));
    assert_eq!(enabled(&s, AppCommand::AddToVca(None)), Some(false));
    s.execute(AppCommand::CreateVca);
    assert_eq!(s.core().doc().console.vcas.len(), 1);
    assert_eq!(enabled(&s, AppCommand::AddToVca(None)), Some(true));
    assert_eq!(enabled(&s, AppCommand::RemoveFromVca), Some(true));
    assert_eq!(enabled(&s, AppCommand::DeleteVca(None)), Some(true));
    s.execute(AppCommand::RemoveFromVca);
    assert!(s.core().doc().console.vcas[0].members.is_empty());
    s.execute(AppCommand::DeleteVca(Some(1)));
    assert!(s.core().doc().console.vcas.is_empty());
}

#[test]
fn polarity_and_insert_bypass_are_mixer_menu_items_without_keys_that_enable_with_the_selection() {
    let mut s = shell();
    let mixer_item = |s: &Shell<NullBackend>, command: AppCommand| {
        s.menu_model()
            .into_iter()
            .find(|m| m.title == "Mixer")
            .and_then(|m| m.items().find(|i| i.command == command).cloned())
            .expect("in the Mixer menu")
    };
    for (label, command, id) in [
        (
            "Toggle Polarity",
            AppCommand::TogglePolarity,
            "mixer.toggle_polarity",
        ),
        (
            "Bypass All Inserts",
            AppCommand::BypassAllInserts,
            "mixer.bypass_all_inserts",
        ),
    ] {
        let item = mixer_item(&s, command);
        assert_eq!(item.label, label);
        assert_eq!(item.shortcut, None, "no default key");
        assert!(!item.enabled, "nothing is selected");
        assert_eq!(command.command_id(), Some((id, None)));
        assert_eq!(AppCommand::from_command(id, None), Some(command));
    }
    add_synth(&mut s, "a");
    let a = DeviceKey("a".into());
    s.core_mut()
        .view_mut()
        .set_selection(std::slice::from_ref(&a), Some(&a));
    assert!(mixer_item(&s, AppCommand::TogglePolarity).enabled);
    assert!(
        !mixer_item(&s, AppCommand::BypassAllInserts).enabled,
        "no insert effect yet"
    );
    s.execute(AppCommand::TogglePolarity);
    let strip = s
        .core()
        .doc()
        .console
        .channel_of(&a)
        .unwrap()
        .channel
        .clone();
    let phase = |s: &Shell<NullBackend>| {
        let device = s.core().doc().device(&strip).unwrap();
        device.params.get("input.phase").copied().unwrap_or(0.0)
    };
    assert_eq!(phase(&s), 1.0);
    s.execute(AppCommand::TogglePolarity);
    assert_eq!(phase(&s), 0.0);
}

#[test]
fn the_snapshot_menu_items_run_through_the_shell_and_enable_with_the_snapshots() {
    let mut s = shell();
    for (label, command, id) in [
        (
            "Store Snapshot",
            AppCommand::StoreSnapshot,
            "mixer.store_snapshot",
        ),
        (
            "Update Snapshot",
            AppCommand::UpdateSnapshot(None),
            "mixer.update_snapshot",
        ),
        (
            "Recall Snapshot",
            AppCommand::RecallSnapshot(None),
            "mixer.recall_snapshot",
        ),
        (
            "Recall Next Snapshot",
            AppCommand::RecallNextSnapshot,
            "mixer.recall_next_snapshot",
        ),
        (
            "Recall Previous Snapshot",
            AppCommand::RecallPreviousSnapshot,
            "mixer.recall_previous_snapshot",
        ),
        (
            "Compare A/B",
            AppCommand::CompareSnapshot(None),
            "mixer.compare_snapshot",
        ),
        ("End Compare", AppCommand::EndCompare, "mixer.end_compare"),
        (
            "Delete Snapshot",
            AppCommand::DeleteSnapshot(None),
            "mixer.delete_snapshot",
        ),
    ] {
        let item = mixer_item(&s, command);
        assert_eq!(item.label, label);
        assert_eq!(item.shortcut, None, "no default key");
        assert_eq!(item.enabled, id == "mixer.store_snapshot", "{label}");
        assert_eq!(command.command_id(), Some((id, None)));
        assert_eq!(AppCommand::from_command(id, None), Some(command));
    }
    // A snapshot is the argument; a command that names one has no static id.
    assert_eq!(
        AppCommand::from_command("mixer.recall_snapshot", Some("3")),
        Some(AppCommand::RecallSnapshot(Some(3)))
    );
    assert_eq!(
        AppCommand::from_command("mixer.delete_snapshot", Some("x")),
        None
    );
    assert_eq!(AppCommand::RecallSnapshot(Some(3)).command_id(), None);

    add_synth(&mut s, "a");
    s.execute(AppCommand::StoreSnapshot);
    assert_eq!(s.core().doc().console.snapshots.len(), 1);
    for command in [
        AppCommand::RecallSnapshot(None),
        AppCommand::RecallNextSnapshot,
        AppCommand::RecallPreviousSnapshot,
        AppCommand::CompareSnapshot(None),
        AppCommand::DeleteSnapshot(None),
    ] {
        assert!(mixer_item(&s, command).enabled, "{command:?}");
    }
    assert!(!mixer_item(&s, AppCommand::EndCompare).enabled);
    assert!(
        !mixer_item(&s, AppCommand::UpdateSnapshot(None)).enabled,
        "the mix is the snapshot"
    );
    s.execute(AppCommand::CompareSnapshot(None));
    assert!(s.core().doc().console.compare.is_some());
    assert!(mixer_item(&s, AppCommand::EndCompare).enabled);
    s.execute(AppCommand::EndCompare);
    assert!(s.core().doc().console.compare.is_none());
    s.execute(AppCommand::DeleteSnapshot(None));
    assert!(s.core().doc().console.snapshots.is_empty());
}

#[test]
fn the_snapshot_bar_of_the_console_stores_recalls_and_compares_through_the_shell() {
    let mut s = shell();
    add_synth(&mut s, "a");
    s.execute(AppCommand::ToggleConsole);
    let channel = s.core().doc().console.channels[0].channel.clone();
    let fader = |s: &Shell<NullBackend>| {
        s.core()
            .doc()
            .param_value(&channel, "fader", s.core().registry())
            .unwrap()
    };
    let store = s
        .core()
        .console()
        .button_center(ConsoleButton::StoreSnapshot)
        .unwrap();
    click(&mut s, store);
    assert_eq!(s.core().doc().console.snapshots[0].name, "A");
    s.core_mut()
        .apply_command(Command::SetParam {
            key: channel.clone(),
            param: "fader".into(),
            value: 0.2,
            merge: None,
        })
        .unwrap();
    // The chip recalls (one undo step), the A/B button compares and flips.
    let chip = s.core().console().snapshot_center(1).unwrap();
    click(&mut s, chip);
    assert_ne!(fader(&s), 0.2);
    let original = fader(&s);
    s.core_mut().undo();
    assert_eq!(fader(&s), 0.2);
    let ab = s
        .core()
        .console()
        .button_center(ConsoleButton::Compare)
        .unwrap();
    click(&mut s, ab);
    assert_eq!(fader(&s), original);
    assert_eq!(s.core().console().compare_state(), Some((1, true)));
    click(&mut s, ab);
    assert_eq!(fader(&s), 0.2);
    let end = s
        .core()
        .console()
        .button_center(ConsoleButton::EndCompare)
        .unwrap();
    click(&mut s, end);
    assert!(s.core().doc().console.compare.is_none());
    assert_eq!(fader(&s), 0.2);
    // Double-click names the snapshot: the console owns the keyboard while it is typed.
    let chip = s.core().console().snapshot_center(1).unwrap();
    click_with(&mut s, chip, Modifiers::NONE, 2);
    assert!(s.core().console().is_text_owner());
    for c in "Chorus".chars() {
        tap(&mut s, Key::Char(c), Modifiers::NONE);
    }
    tap(&mut s, Key::Enter, Modifiers::NONE);
    assert!(s.core().doc().console.snapshots[0].name.ends_with("Chorus"));
}

fn mixer_item(s: &Shell<NullBackend>, command: AppCommand) -> crate::commands::MenuItem {
    s.menu_model()
        .into_iter()
        .find(|m| m.title == "Mixer")
        .and_then(|m| m.items().find(|i| i.command == command).cloned())
        .expect("in the Mixer menu")
}

#[test]
fn the_audio_track_meters_are_fed_every_frame_without_the_console_and_idle_ones_repaint_nothing() {
    let mut s = shell();
    s.core_mut()
        .apply_command(Command::Song(atomik_model::SongCommand::AddAudioTrack {
            name: Some("Mic".to_owned()),
        }))
        .unwrap();
    let track = s.core().doc().song.tracks.last().unwrap().device.clone();
    assert!(!s.console_visible());
    s.frame_at(0.0);
    let mut list = atomik_ui::DisplayList::new();
    s.paint(&mut list);
    assert!(
        list.texts().contains(&"-inf"),
        "the header shows the readout: {:?}",
        list.texts()
    );
    assert!(s.core().arrangement().track_meter(&track).is_some());
    // Silence frame after frame: the main window is not asked to repaint for the meters.
    for t in [0.1, 0.2, 0.3] {
        s.frame_at(t);
        assert!(!s.needs_repaint(), "an idle meter costs no frame at {t}");
    }
    // A peak on the track is fed from the same frame and does ask for one, in the main window
    // only (the meter is on the arrangement's header, whatever has the focus).
    s.core_mut().arrangement_mut().feed_track_levels(
        &[atomik_ui::TrackLevel {
            device: track,
            source: atomik_ui::MeterSource::Playback,
            peaks: [0.5, 0.5],
            stereo: true,
        }],
        0.4,
    );
    assert!(s.needs_repaint_in(WindowKind::Main));
}
