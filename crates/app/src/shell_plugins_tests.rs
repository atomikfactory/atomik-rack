//! Plugin hosting in the `Shell` (app side): the
//! project banner above the rack, the Plugins settings modal with consent and persistence, the
//! parameter pane in place of the arrangement, and the editor window requests, headless.

use super::*;
use crate::plugin_test_support::*;
use crate::session::NullBackend;
use atomik_ui::{DisplayList, Key, Modifiers, PointerButton};

fn demo_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/plugin-demo/plugin-demo.atomik")
}

fn shell_with(settings_path: Option<PathBuf>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path,
            ui_state_path: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1100.0, 760.0)));
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

fn click(s: &mut Shell<NullBackend>, p: Point) {
    s.handle(&down(p));
    s.handle(&up(p));
}

fn press(key: Key) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

// ------------------------------------------------------------------ the project banner

#[test]
fn the_banner_shows_above_the_rack_and_open_without_hides_it() {
    let mut s = shell_with(None);
    s.core_mut().configure_plugins(config(false, &[PKG]));
    let before = s.layout().rack;
    s.open_file(&demo_file());
    s.frame();
    assert!(
        s.banner_view().is_visible(),
        "the project uses plugins and they are off"
    );
    let after = s.layout().rack;
    assert!(
        after.y0 > before.y0 && after.y0 - before.y0 >= s.banner_view().height() - 0.5,
        "the rack moved down by the banner: {before:?} -> {after:?}"
    );
    assert_eq!(s.banner_view().height(), after.y0 - before.y0);
    // The banner is painted and takes the pointer inside its strip.
    let mut list = DisplayList::new();
    s.paint(&mut list);
    let p = s.banner_view().open_without_rect().unwrap().center();
    click(&mut s, p);
    assert!(!s.banner_view().is_visible());
    assert!(s.core().plugin_banner().is_none());
    assert_eq!(s.layout().rack.y0, before.y0, "the rack is back at the top");
    assert_eq!(s.core().plugin_instance_count(), 0, "the stand-ins stay");
}

#[test]
fn load_in_the_banner_enables_plugins_after_the_warning() {
    let path = temp("shell-banner").join("settings.json");
    let mut s = shell_with(Some(path.clone()));
    s.core_mut().configure_plugins(config(false, &[PKG]));
    s.open_file(&demo_file());
    s.frame();
    let load = s.banner_view().load_rect().unwrap().center();
    click(&mut s, load);
    assert!(
        s.banner_view().is_warning_open(),
        "the native-code warning comes first"
    );
    assert!(!s.core().plugin_settings().enabled);
    let accept = s.banner_view().accept_rect().unwrap().center();
    click(&mut s, accept);
    assert!(s.core().plugin_settings().enabled);
    assert!(!s.banner_view().is_visible());
    // The setting is saved with the audio settings.
    s.frame();
    let (saved, err) = settings::load(&path);
    assert!(err.is_none());
    assert!(saved.plugins.enabled, "the choice is saved");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

// ------------------------------------------------------------------ the Plugins settings modal

#[test]
fn the_plugins_modal_enables_plugins_after_the_warning_lists_them_and_saves_the_choice() {
    let path = temp("shell-modal").join("settings.json");
    let mut s = shell_with(Some(path.clone()));
    s.core_mut().configure_plugins(config(false, &[PKG]));
    assert!(!s.plugin_settings_open());
    s.execute(AppCommand::PluginSettings);
    assert!(s.plugin_settings_open());
    // Modal: shortcuts do not run.
    assert!(
        s.handle(&UiEvent::Key {
            key: Key::Char('o'),
            modifiers: Modifiers::CTRL,
            pressed: true
        })
        .is_empty()
    );
    let enable = s.plugin_settings_view().unwrap().enable_rect().center();
    click(&mut s, enable);
    assert!(s.plugin_settings_view().unwrap().is_warning_open());
    assert!(
        !s.core().plugin_settings().enabled,
        "nothing before the answer"
    );
    let accept = s
        .plugin_settings_view()
        .unwrap()
        .warning_accept_rect()
        .unwrap()
        .center();
    click(&mut s, accept);
    assert!(s.core().plugin_settings().enabled);
    s.frame();
    let shown = s.plugin_settings_view().unwrap().data();
    assert!(shown.enabled);
    assert!(
        shown
            .plugins
            .iter()
            .any(|p| p.plugin.id == "atomik.test.synth"),
        "the found plugins are listed: {:?}",
        shown.plugins.len()
    );
    let (saved, _) = settings::load(&path);
    assert!(saved.plugins.enabled);
    // Esc closes the modal.
    s.handle(&press(Key::Escape));
    assert!(!s.plugin_settings_open());
    // A painted frame with the modal open does not panic.
    s.execute(AppCommand::PluginSettings);
    let mut list = DisplayList::new();
    s.paint(&mut list);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn add_folder_asks_the_platform_for_a_folder_dialog() {
    let mut s = shell_with(None);
    s.core_mut().configure_plugins(config(true, &[PKG]));
    s.execute(AppCommand::PluginSettings);
    let add = s
        .plugin_settings_view()
        .unwrap()
        .add_folder_rect()
        .unwrap()
        .center();
    let requests = s.handle(&down(add));
    let mut requests: Vec<ShellRequest> = requests;
    requests.extend(s.handle(&up(add)));
    requests.extend(s.take_requests());
    assert!(
        requests.contains(&ShellRequest::PickPluginFolder),
        "{requests:?}"
    );
    s.add_plugin_folder(PathBuf::from("D:/Plugins/CLAP"));
    assert_eq!(
        s.core().plugin_settings().extra_paths,
        vec![PathBuf::from("D:/Plugins/CLAP")]
    );
}

// ------------------------------------------------------------------ the parameter pane

#[test]
fn the_parameter_pane_takes_the_place_of_the_arrangement_and_closes_again() {
    let mut s = shell_with(None);
    s.core_mut().configure_plugins(config(true, &[PKG]));
    let add = add_command(s.core(), SYNTH, "lead");
    s.core_mut().apply_command(add).unwrap();
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    s.open_plugin_params(&key("lead"));
    assert_eq!(s.lower_pane(), Pane::PluginParams);
    assert_eq!(s.focus(), Pane::PluginParams);
    assert!(s.core().plugin_params().is_present());
    assert_eq!(s.core().plugin_params().total_count(), 2, "wave and level");
    let mut list = DisplayList::new();
    s.paint(&mut list);
    // A click on the pane goes to the pane, in window coordinates.
    let song = s.layout().song.unwrap();
    s.handle(&down(song.center()));
    s.handle(&up(song.center()));
    assert_eq!(s.hit_pane(song.center()), Some(Pane::PluginParams));
    // Close from the pane's own button.
    let close = s.core().plugin_params().close_rect().center();
    click(&mut s, close);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    // Removing the device closes the pane too.
    s.open_plugin_params(&key("lead"));
    s.core_mut()
        .apply_command(Command::RemoveDevice { key: key("lead") })
        .unwrap();
    assert_eq!(s.lower_pane(), Pane::Arrangement);
}

#[test]
fn the_loading_status_and_window_requests_reach_the_platform_layer() {
    let mut s = shell_with(None);
    s.core_mut().configure_plugins(config(true, &[PKG]));
    assert!(s.take_plugin_window_requests().is_empty());
    // With no audio engine the instance is still created (parked); no progress is shown.
    let add = add_command(s.core(), SYNTH, "lead");
    s.core_mut().apply_command(add).unwrap();
    s.core_mut().wait_plugins();
    s.poll_status();
    assert_eq!(s.core().plugin_loading_text(), None);
    assert!(s.core().plugin_instance(&key("lead")).is_some());
}

#[test]
fn the_parameter_pane_owns_plain_keys_for_its_search() {
    // letters, digits and Space typed in the pane reach its search field; none of them
    // is a transport command (the legacy profile has L for the loop and Space for play).
    let mut s = shell_with(None);
    s.core_mut().configure_plugins(config(true, &[PKG]));
    let add = add_command(s.core(), SYNTH, "lead");
    s.core_mut().apply_command(add).unwrap();
    s.open_plugin_params(&key("lead"));
    assert!(s.context_stack().is_text());
    for k in [Key::Char('l'), Key::Space, Key::Char('r'), Key::Char('1')] {
        s.handle(&press(k.clone()));
        s.handle(&UiEvent::Key {
            key: k,
            modifiers: Modifiers::NONE,
            pressed: false,
        });
    }
    assert_eq!(s.core().plugin_params().query(), "l r1");
    assert!(!s.core().is_playing());
    assert!(!s.core().doc().song.loop_enabled);
    assert!(!s.core().is_recording());
    // A command chord still runs while typing.
    s.handle(&UiEvent::Key {
        key: Key::F(5),
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(s.menu_state().console);
}
