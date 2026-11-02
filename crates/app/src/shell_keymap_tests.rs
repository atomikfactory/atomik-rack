//! The user keymap and the workflow preferences end to end in the shell:
//! load at start, hot reload, the Settings dialog applying and
//! cancelling, persistence across a restart, export and import. Every test uses its own
//! temporary config directory; nothing touches the real one.

use super::*;
use crate::keymap_store::{KEYMAP_FILE, WORKFLOW_FILE};
use crate::session::NullBackend;
use crate::settings_dialog::Tab;
use atomik_model::Command;
use atomik_ui::TransportAction;
use atomik_ui::keymap::script::event_for;
use std::path::PathBuf;

fn config(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-shell-keymap-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn shell_in(dir: &Path, os: Os, profile: Option<Profile>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(os),
            profile,
            in_window_menu: true,
            settings_path: Some(dir.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

fn key_event(os: Os, chord: &str, pressed: bool) -> UiEvent {
    let (key, modifiers) = event_for(os, chord).unwrap();
    UiEvent::Key {
        key,
        modifiers,
        pressed,
    }
}

fn tap(s: &mut Shell<NullBackend>, os: Os, chord: &str) -> Vec<ShellRequest> {
    let r = s.handle(&key_event(os, chord, true));
    s.handle(&key_event(os, chord, false));
    r
}

fn click(s: &mut Shell<NullBackend>, p: Point) -> Vec<ShellRequest> {
    let r = s.handle(&UiEvent::PointerDown {
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
    r
}

const VIEW: Size = Size::new(1000.0, 800.0);

fn open_tab(s: &mut Shell<NullBackend>, tab: Tab) {
    s.execute(AppCommand::AudioSettings);
    assert!(s.dialog_open());
    let r = s
        .dialog()
        .unwrap()
        .layout(VIEW)
        .tabs
        .iter()
        .find(|(_, t)| *t == tab)
        .unwrap()
        .0;
    click(s, r.center());
}

/// Rebinds `transport.loop` slot 0 to `chord` through the dialog (the dialog stays open). The
/// list is scrolled until the row shows.
fn learn_loop(s: &mut Shell<NullBackend>, os: Os, chord: &str) {
    let chip = loop {
        let l = s.dialog().unwrap().keyboard_layout(VIEW).unwrap();
        let chip = l
            .items
            .iter()
            .find_map(|i| match i {
                crate::keyboard_tab::Item::Row { row, chips, .. }
                    if row.key.command == "transport.loop"
                        && row.key.context == Context::Global =>
                {
                    Some(chips[0].rect.center())
                }
                _ => None,
            })
            .unwrap();
        if l.list.contains(chip) {
            break chip;
        }
        s.handle(&UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, -1.0),
            pos: l.list.center(),
            modifiers: Modifiers::NONE,
        });
    };
    click(s, chip);
    s.handle(&key_event(os, chord, true));
}

fn press_apply(s: &mut Shell<NullBackend>) {
    let p = s.dialog().unwrap().buttons(VIEW).0.center();
    click(s, p);
}

fn press_cancel(s: &mut Shell<NullBackend>) {
    let p = s.dialog().unwrap().buttons(VIEW).1.center();
    click(s, p);
}

fn looping(s: &Shell<NullBackend>) -> bool {
    s.core().doc().song.loop_enabled
}

#[test]
fn without_files_the_shell_runs_the_app_default_keys_and_default_preferences() {
    let dir = config("fresh");
    let s = shell_in(&dir, Os::Windows, None);
    assert_eq!(s.profile(), APP_DEFAULT_PROFILE);
    assert!(s.user_keymap().is_empty());
    // The one-time notice of the moved keys is remembered in workflow.json; every
    // preference is its default.
    let expected = WorkflowPrefs {
        keys_notice_seen: true,
        ..WorkflowPrefs::default()
    };
    assert_eq!(*s.workflow(), expected);
    assert!(s.workflow().auto_route);
    assert!(
        !dir.join(KEYMAP_FILE).exists(),
        "no keymap is written until the user applies"
    );
    assert!(dir.join(WORKFLOW_FILE).exists(), "the notice flag is kept");
}

#[test]
fn a_keymap_json_next_to_settings_is_loaded_and_remaps_the_key() {
    let dir = config("load");
    std::fs::write(
        dir.join(KEYMAP_FILE),
        r#"{"bindings": [{"command": "transport.loop", "context": "global", "chords": ["Cmd+Alt+L"]}]}"#,
    )
    .unwrap();
    let mut s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    tap(&mut s, Os::Windows, "L");
    assert!(!looping(&s), "the old key no longer toggles the loop");
    tap(&mut s, Os::Windows, "Cmd+Alt+L");
    assert!(looping(&s));
    assert_eq!(
        s.menu_model()
            .iter()
            .flat_map(Menu::items)
            .find(|i| i.command == AppCommand::Transport(TransportAction::ToggleLoop))
            .and_then(|i| i.shortcut.clone())
            .as_deref(),
        Some("Ctrl+Alt+L"),
        "the menu shows the remapped chord"
    );
}

#[test]
fn the_chosen_profile_in_the_file_wins_over_the_app_default() {
    let dir = config("profile");
    std::fs::write(dir.join(KEYMAP_FILE), r#"{"profile": "reference"}"#).unwrap();
    let s = shell_in(&dir, Os::Windows, Some(Profile::Legacy));
    assert_eq!(s.profile(), Profile::Standard);
    assert_eq!(
        s.keymap()
            .shortcut_text("file.import_audio", None)
            .as_deref(),
        Some("Ctrl+Shift+I")
    );
}

#[test]
fn a_corrupt_keymap_is_moved_aside_and_the_defaults_run_with_a_message() {
    let dir = config("corrupt");
    std::fs::write(dir.join(KEYMAP_FILE), "{ not json").unwrap();
    let s = shell_in(&dir, Os::Windows, None);
    assert!(s.user_keymap().is_empty());
    assert!(dir.join("keymap.json.bad").exists());
    assert!(!dir.join(KEYMAP_FILE).exists());
    assert!(s.core().last_rejection().unwrap().contains("keymap.json"));
}

#[test]
fn a_conflict_in_the_file_is_reported_at_start() {
    let dir = config("conflict");
    std::fs::write(
        dir.join(KEYMAP_FILE),
        r#"{"bindings": [{"command": "transport.metronome", "context": "global", "chords": ["L"]}]}"#,
    )
    .unwrap();
    let s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    let msg = s.core().last_rejection().unwrap();
    assert!(msg.contains("keymap.json") && msg.contains("L"), "{msg}");
}

#[test]
fn rebinding_in_settings_and_applying_saves_only_the_change_and_takes_effect_at_once() {
    let dir = config("apply");
    let mut s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    open_tab(&mut s, Tab::Keyboard);
    learn_loop(&mut s, Os::Windows, "Cmd+Alt+L");
    press_apply(&mut s);
    assert!(!s.dialog_open());
    // Hot reload: the running shell already uses the new binding.
    tap(&mut s, Os::Windows, "L");
    assert!(!looping(&s));
    tap(&mut s, Os::Windows, "Cmd+Alt+L");
    assert!(looping(&s));
    // Only the changed binding is in the file.
    let text = std::fs::read_to_string(dir.join(KEYMAP_FILE)).unwrap();
    assert!(
        text.contains("transport.loop") && text.contains("Cmd+Alt+L"),
        "{text}"
    );
    assert_eq!(text.matches("\"command\"").count(), 1, "{text}");
    assert!(!dir.join("keymap.json.tmp").exists());
    // A restart picks it up.
    let mut again = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    tap(&mut again, Os::Windows, "Cmd+Alt+L");
    assert!(looping(&again));
}

#[test]
fn cancel_discards_the_keyboard_and_workflow_drafts() {
    let dir = config("cancel");
    let mut s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    open_tab(&mut s, Tab::Keyboard);
    learn_loop(&mut s, Os::Windows, "Cmd+Alt+L");
    press_cancel(&mut s);
    assert!(!s.dialog_open());
    assert!(!dir.join(KEYMAP_FILE).exists());
    tap(&mut s, Os::Windows, "L");
    assert!(looping(&s), "the old binding is unchanged");
}

#[test]
fn the_profile_choice_made_in_settings_persists() {
    let dir = config("choose-profile");
    let mut s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    open_tab(&mut s, Tab::Keyboard);
    let legacy = s
        .dialog()
        .unwrap()
        .keyboard_layout(VIEW)
        .unwrap()
        .profiles
        .iter()
        .find(|(_, p, _)| *p == Profile::Legacy)
        .unwrap()
        .0;
    click(&mut s, legacy.center());
    press_apply(&mut s);
    assert_eq!(s.profile(), Profile::Legacy);
    assert_eq!(
        s.keymap()
            .shortcut_text("file.import_audio", None)
            .as_deref(),
        Some("Ctrl+I")
    );
    let again = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    assert_eq!(again.profile(), Profile::Legacy, "kept across a restart");
}

#[test]
fn the_workflow_tab_saves_workflow_json_and_survives_a_restart() {
    let dir = config("workflow");
    let mut s = shell_in(&dir, Os::Windows, None);
    open_tab(&mut s, Tab::Workflow);
    let toggle = s.dialog().unwrap().workflow_layout(VIEW).unwrap().toggles[0]
        .rect
        .center();
    click(&mut s, toggle);
    press_apply(&mut s);
    assert!(!s.workflow().auto_route);
    let text = std::fs::read_to_string(dir.join(WORKFLOW_FILE)).unwrap();
    assert!(text.contains("\"auto_route\": false"), "{text}");
    let again = shell_in(&dir, Os::Windows, None);
    assert!(!again.workflow().auto_route);
    assert!(
        !dir.join(KEYMAP_FILE).exists(),
        "an unchanged keymap is not written"
    );
}

#[test]
fn an_edited_keymap_file_is_hot_reloaded_while_the_app_runs() {
    let dir = config("hot");
    let mut s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    tap(&mut s, Os::Windows, "L");
    assert!(looping(&s));
    tap(&mut s, Os::Windows, "L");
    assert!(!looping(&s));
    std::fs::write(
        dir.join(KEYMAP_FILE),
        r#"{"bindings": [{"command": "transport.loop", "context": "global", "chords": ["Cmd+Alt+L"]}]}"#,
    )
    .unwrap();
    assert!(s.reload_keymap_file(), "a changed file is picked up");
    tap(&mut s, Os::Windows, "L");
    assert!(!looping(&s));
    tap(&mut s, Os::Windows, "Cmd+Alt+L");
    assert!(looping(&s));
    assert!(!s.reload_keymap_file(), "nothing changed since");
    // A window that gets the focus notices an edit; the status poll does not look.
    std::fs::write(dir.join(KEYMAP_FILE), r#"{"profile": "legacy"}"#).unwrap();
    s.poll_status();
    assert_ne!(
        s.profile(),
        Profile::Legacy,
        "the poll does not stat the file"
    );
    s.window_focused(WindowKind::Main);
    assert_eq!(s.profile(), Profile::Legacy);
}

#[test]
fn deleting_the_keymap_file_while_running_goes_back_to_the_defaults() {
    let dir = config("hot-delete");
    std::fs::write(
        dir.join(KEYMAP_FILE),
        r#"{"bindings": [{"command": "transport.loop", "context": "global", "chords": ["Cmd+Alt+L"]}]}"#,
    )
    .unwrap();
    let mut s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    std::fs::remove_file(dir.join(KEYMAP_FILE)).unwrap();
    assert!(s.reload_keymap_file());
    tap(&mut s, Os::Windows, "L");
    assert!(looping(&s));
}

#[test]
fn a_numpad_chord_is_learned_as_a_numpad_chord_not_as_a_digit() {
    let dir = config("numpad");
    let mut s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    open_tab(&mut s, Tab::Keyboard);
    learn_loop(&mut s, Os::Windows, "Shift+Numpad6");
    let user = s.dialog().unwrap().preferences().unwrap().0;
    assert!(
        user.to_json().contains("Shift+Numpad6"),
        "{}",
        user.to_json()
    );
}

#[test]
fn export_and_import_go_through_the_platform_and_the_file_picked() {
    let dir = config("export");
    let mut s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    open_tab(&mut s, Tab::Keyboard);
    learn_loop(&mut s, Os::Windows, "Cmd+Alt+L");
    let l = s.dialog().unwrap().keyboard_layout(VIEW).unwrap();
    assert_eq!(
        click(&mut s, l.export.center()),
        vec![ShellRequest::PickExportKeymap]
    );
    let out = dir.join("export").join("mine.json");
    s.export_keymap_to(&out);
    let exported = std::fs::read_to_string(&out).unwrap();
    assert!(
        exported.contains("Cmd+Alt+L"),
        "the dialog draft is exported: {exported}"
    );
    assert!(!dir.join(KEYMAP_FILE).exists(), "exporting does not apply");
    press_cancel(&mut s);
    // Import into the open dialog replaces the draft; applying saves it.
    let mut t = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    open_tab(&mut t, Tab::Keyboard);
    let l = t.dialog().unwrap().keyboard_layout(VIEW).unwrap();
    assert_eq!(
        click(&mut t, l.import.center()),
        vec![ShellRequest::PickImportKeymap]
    );
    t.import_keymap_from(&out);
    press_apply(&mut t);
    tap(&mut t, Os::Windows, "Cmd+Alt+L");
    assert!(looping(&t));
    // Importing with no dialog open applies at once.
    let mut u = shell_in(
        &config("export-direct"),
        Os::Windows,
        Some(Profile::Standard),
    );
    u.import_keymap_from(&out);
    tap(&mut u, Os::Windows, "Cmd+Alt+L");
    assert!(looping(&u));
    // A file that is not a keymap is refused with a message and changes nothing.
    let bad = dir.join("bad.json");
    std::fs::write(&bad, "[1]").unwrap();
    u.import_keymap_from(&bad);
    assert!(u.core().last_rejection().unwrap().contains("bad.json"));
    tap(&mut u, Os::Windows, "Cmd+Alt+L");
    assert!(!looping(&u), "still the imported keymap");
}

#[test]
fn macos_chords_are_shown_and_saved_with_the_logical_modifier() {
    let dir = config("mac");
    let mut s = shell_in(&dir, Os::Mac, Some(Profile::Standard));
    open_tab(&mut s, Tab::Keyboard);
    learn_loop(&mut s, Os::Mac, "Cmd+Alt+L");
    press_apply(&mut s);
    let text = std::fs::read_to_string(dir.join(KEYMAP_FILE)).unwrap();
    assert!(text.contains("Cmd+Alt+L"), "{text}");
    // The same file on Windows means Ctrl+Alt+L.
    let mut w = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    tap(&mut w, Os::Windows, "Cmd+Alt+L");
    assert!(looping(&w));
}

#[test]
fn the_roles_of_the_file_are_available_to_the_shell() {
    let dir = config("roles");
    std::fs::write(
        dir.join(KEYMAP_FILE),
        r#"{"roles": {"alternate_tool": "Ctrl+Shift"}}"#,
    )
    .unwrap();
    let s = shell_in(&dir, Os::Windows, None);
    let b = s
        .roles()
        .binding(atomik_ui::keymap::Role::AlternateTool)
        .unwrap();
    assert_eq!(b.mods.to_string(), "Ctrl+Shift");
}

#[test]
fn text_owners_get_plain_keys_and_the_stack_says_so() {
    // The rack browser filter, a container macro rename, a pattern rename and the plugin
    // parameter search all own the keyboard.
    let dir = config("text-owner");
    let mut s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    s.execute(AppCommand::ToggleBrowser);
    assert!(s.core().view().is_text_owner());
    assert!(s.context_stack().is_text());
}

#[test]
fn the_keymap_flag_uses_another_file_and_saves_to_it() {
    let dir = config("flag");
    let other = config("flag-other").join("mine.json");
    std::fs::write(
        &other,
        r#"{"bindings": [{"command": "transport.loop", "context": "global", "chords": ["Cmd+Alt+L"]}]}"#,
    )
    .unwrap();
    let mut s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    s.use_keymap_file(other.clone());
    tap(&mut s, Os::Windows, "Cmd+Alt+L");
    assert!(looping(&s));
    // A change in Settings is saved to that file, not next to settings.json.
    open_tab(&mut s, Tab::Keyboard);
    learn_loop(&mut s, Os::Windows, "Cmd+Alt+K");
    press_apply(&mut s);
    assert!(
        std::fs::read_to_string(&other)
            .unwrap()
            .contains("Cmd+Alt+K")
    );
    assert!(!dir.join(KEYMAP_FILE).exists());
}

#[test]
fn a_mixer_key_can_be_rebound_and_the_old_key_stops_working() {
    let dir = config("mixer-rebind");
    std::fs::write(
        dir.join(KEYMAP_FILE),
        r#"{"bindings": [{"command": "mixer.scroll_right", "context": "mixer", "chords": ["Cmd+Alt+Right"]}]}"#,
    )
    .unwrap();
    let mut s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    for i in 0..12 {
        s.core_mut()
            .apply_command(Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.poly-synth".into(),
                key: Some(DeviceKey(format!("s{i}"))),
            })
            .unwrap();
    }
    s.execute(AppCommand::ToggleConsole);
    s.set_focus(Pane::Console);
    tap(&mut s, Os::Windows, "Right");
    assert_eq!(s.core().console().scroll_x(), 0.0, "the old key is unbound");
    tap(&mut s, Os::Windows, "Cmd+Alt+Right");
    assert!(s.core().console().scroll_x() > 0.0, "the new key scrolls");
    assert_eq!(
        s.keymap()
            .shortcut_text("mixer.scroll_right", None)
            .as_deref(),
        Some("Ctrl+Alt+Right")
    );
}

#[test]
fn a_mixer_key_that_collides_in_the_context_is_a_conflict() {
    let dir = config("mixer-conflict");
    std::fs::write(
        dir.join(KEYMAP_FILE),
        r#"{"bindings": [{"command": "mixer.value_up", "context": "mixer", "chords": ["Down"]}]}"#,
    )
    .unwrap();
    let s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    let msg = s.core().last_rejection().unwrap();
    assert!(msg.contains("keymap.json") && msg.contains("Down"), "{msg}");
}

#[test]
fn ctrl_p_toggles_the_pre_count_and_saves_it_with_the_settings() {
    let dir = config("precount");
    let mut s = shell_in(&dir, Os::Windows, Some(Profile::Standard));
    assert_eq!(s.core().pre_count_bars(), 0);
    tap(&mut s, Os::Windows, "Cmd+P");
    assert_eq!(s.core().pre_count_bars(), 1);
    assert_eq!(s.core().sink().settings().pre_count_bars, 1);
    let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    assert!(text.contains("\"pre_count_bars\": 1"), "{text}");
    tap(&mut s, Os::Windows, "Cmd+P");
    assert_eq!(s.core().pre_count_bars(), 0);
    let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    assert!(text.contains("\"pre_count_bars\": 0"), "{text}");
}
