//! The plugin manager in the `Shell`: Options > Plugin Manager opens the modal table over the
//! installed plugins, a star or a tag is saved to `plugins_user.json` beside the settings and
//! read back by the next start, Esc closes it, a failed plugin's file can be shown in its folder,
//! and the browser's Plugins section follows the stars. Headless, over the in-repo test plugins.

use super::*;
use crate::plugin_test_support::*;
use crate::plugins_user::PluginsUser;
use crate::session::NullBackend;
use atomik_ui::{DisplayList, Key, Modifiers, PointerButton};

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
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1100.0, 760.0)));
    s
}

/// A click; what the shell asks of the platform because of it.
fn click(s: &mut Shell<NullBackend>, p: Point) -> Vec<ShellRequest> {
    let mut requests = s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    requests.extend(s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }));
    requests.extend(s.take_requests());
    requests
}

fn key(key: Key) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

/// A shell with the test plugins scanned, and its settings in a folder of its own.
fn plugin_shell(name: &str) -> (Shell<NullBackend>, PathBuf) {
    let path = temp(name).join("settings.json");
    let mut s = shell_with(Some(path.clone()));
    s.core_mut().configure_plugins(config(true, &[PKG]));
    (s, path)
}

fn names(s: &Shell<NullBackend>) -> Vec<String> {
    s.plugin_manager_view()
        .expect("the manager is open")
        .shown()
        .iter()
        .map(|r| r.name.clone())
        .collect()
}

#[test]
fn the_options_menu_opens_the_manager_and_escape_closes_it() {
    let (mut s, path) = plugin_shell("pm-open");
    let menu = s.menu_model();
    let options = menu.iter().find(|m| m.title == "Options").expect("Options");
    assert!(
        options.items().any(
            |i| i.command == AppCommand::PluginManager && i.label.starts_with("Plugin Manager")
        ),
        "Options > Plugin Manager"
    );
    assert!(!s.plugin_manager_open());
    s.execute(AppCommand::PluginManager);
    assert!(s.plugin_manager_open());
    assert!(s.wants_text_input(), "the search field takes typed text");
    let listed = names(&s);
    assert!(listed.len() >= 3, "the test plugins are listed: {listed:?}");
    // The panel is modal: a shortcut does not reach the song.
    assert!(
        s.handle(&UiEvent::Key {
            key: Key::Char('o'),
            modifiers: Modifiers::CTRL,
            pressed: true
        })
        .is_empty()
    );
    assert!(s.plugin_manager_open());
    let mut list = DisplayList::new();
    s.paint(&mut list);
    assert!(list.texts().contains(&"PLUGIN MANAGER"));
    s.handle(&key(Key::Escape));
    assert!(!s.plugin_manager_open());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_star_and_a_tag_are_saved_beside_the_settings_and_read_back_at_the_next_start() {
    let (mut s, path) = plugin_shell("pm-save");
    s.execute(AppCommand::PluginManager);
    let first = s.plugin_manager_view().unwrap().star_center(0).unwrap();
    click(&mut s, first);
    s.frame();
    let file = crate::plugins_user::path(&path);
    assert!(file.exists(), "saved next to the settings");
    let saved = PluginsUser::load(&file);
    let id = s.core().plugin_manager_data().rows[0].type_id.clone();
    let starred: Vec<String> = s
        .core()
        .plugin_manager_data()
        .rows
        .iter()
        .filter(|r| r.favourite)
        .filter_map(|r| r.type_id.clone())
        .collect();
    assert_eq!(starred.len(), 1, "one star");
    assert!(saved.is_favourite(&starred[0]));
    assert!(id.is_some());
    // The row at the top is starred in the open panel.
    assert!(s.plugin_manager_view().unwrap().shown()[0].favourite);
    // Tag the chosen row: choose it, type in the tag field, press ADD TAG.
    let row = s.plugin_manager_view().unwrap().row_center(0).unwrap();
    click(&mut s, row);
    let tag_field = s.plugin_manager_view().unwrap().tag_rect().center();
    click(&mut s, tag_field);
    for c in "bass".chars() {
        s.handle(&key(Key::Char(c)));
    }
    let add = s.plugin_manager_view().unwrap().add_tag_rect().center();
    click(&mut s, add);
    s.frame();
    assert_eq!(s.plugin_manager_view().unwrap().shown()[0].tags, ["bass"]);
    // A new shell on the same settings folder starts with both.
    let mut again = shell_with(Some(path.clone()));
    again.core_mut().configure_plugins(config(true, &[PKG]));
    let user = again.core().plugins_user();
    assert!(user.is_favourite(&starred[0]));
    assert!(user.has_tag(&starred[0], "BASS"));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_shell_without_a_settings_folder_keeps_the_choices_in_memory_only() {
    let mut s = shell_with(None);
    s.core_mut().configure_plugins(config(true, &[PKG]));
    s.execute(AppCommand::PluginManager);
    let star = s.plugin_manager_view().unwrap().star_center(0).unwrap();
    click(&mut s, star);
    s.frame();
    assert!(s.plugin_manager_view().unwrap().shown()[0].favourite);
}

#[test]
fn show_in_folder_asks_the_platform_to_reveal_the_chosen_file() {
    let (mut s, path) = plugin_shell("pm-reveal");
    s.execute(AppCommand::PluginManager);
    let row = s.plugin_manager_view().unwrap().row_center(0).unwrap();
    click(&mut s, row);
    let want = PathBuf::from(&s.plugin_manager_view().unwrap().shown()[0].path);
    let button = s.plugin_manager_view().unwrap().folder_rect().center();
    assert_eq!(click(&mut s, button), vec![ShellRequest::Reveal(want)]);
    assert!(s.plugin_manager_open(), "the panel stays open");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn close_and_the_search_field_work_from_the_keyboard() {
    let (mut s, path) = plugin_shell("pm-keys");
    s.execute(AppCommand::PluginManager);
    let all = names(&s).len();
    for c in "gain".chars() {
        s.handle(&key(Key::Char(c)));
    }
    let found = names(&s);
    assert!(found.len() < all && !found.is_empty(), "{found:?}");
    assert!(found.iter().all(|n| n.to_lowercase().contains("gain")));
    let close = s.plugin_manager_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.plugin_manager_open());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn the_browser_lists_the_starred_plugins_first_and_hides_the_disabled_ones() {
    let (mut s, path) = plugin_shell("pm-browser");
    let synth = SYNTH.to_owned();
    let gain = GAIN.to_owned();
    s.core_mut()
        .plugin_manager_action(&atomik_ui::PluginManagerAction::Favourite {
            type_id: synth.clone(),
            on: true,
        });
    s.core_mut()
        .plugin_manager_action(&atomik_ui::PluginManagerAction::Disable {
            type_ids: vec![gain],
            disabled: true,
        });
    s.frame();
    let registry = crate::app_core::default_registry();
    let rows = s.core().view().browser_list(&registry);
    let heads: Vec<String> = rows
        .iter()
        .filter_map(|r| match r {
            atomik_ui::browser::BrowserRow::Heading(h) => Some(h.clone()),
            _ => None,
        })
        .collect();
    assert!(heads.contains(&"Favourites".to_owned()), "{heads:?}");
    let listed: Vec<String> = rows
        .iter()
        .filter_map(|r| match r {
            // The browser indexes the catalog in the order the core lists it.
            atomik_ui::browser::BrowserRow::Plugin(i) => {
                Some(s.core().plugin_catalog().entries()[*i].plugin.type_id())
            }
            _ => None,
        })
        .collect();
    assert_eq!(listed.first(), Some(&synth), "the star comes first");
    assert!(!listed.iter().any(|t| t == GAIN), "disabled is hidden");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
