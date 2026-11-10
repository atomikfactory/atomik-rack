//! Quick sidechain in the shell: the Duck from submenu of the track header menu and of the rack
//! device menu (the other tracks by name and colour, at most thirty, then More), the pop-up of
//! every track, one undo step per choice, and the Track menu items.

use super::*;
use crate::session::NullBackend;
use atomik_model::duck_from::duckers_of;
use atomik_model::{Command, DeviceKey, SongCommand};
use atomik_ui::PointerButton;
use atomik_ui::keymap::Os;

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// A headless shell with synth tracks `names`, the arrangement focused.
fn shell(names: &[String]) -> Shell<NullBackend> {
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
    s.handle(&UiEvent::Resize(Size::new(1200.0, 1100.0)));
    for k in names {
        s.core_mut()
            .apply_command(Command::AddDevice {
                index: usize::MAX,
                type_id: SYNTH.to_string(),
                key: Some(key(k)),
            })
            .unwrap();
        s.core_mut()
            .apply_command(Command::Song(SongCommand::AddTrack { device: key(k) }))
            .unwrap();
        s.core_mut()
            .apply_command(Command::Rename {
                key: key(k),
                name: k.clone(),
            })
            .unwrap();
    }
    s.core_mut().after_view_change();
    s.set_focus(Pane::Arrangement);
    s
}

fn four() -> Vec<String> {
    ["a", "b", "c", "d"].map(str::to_owned).to_vec()
}

fn press(s: &mut Shell<NullBackend>, p: Point, button: PointerButton) {
    for down in [true, false] {
        s.handle(&if down {
            UiEvent::PointerDown {
                pos: p,
                button,
                modifiers: Modifiers::NONE,
                clicks: 1,
            }
        } else {
            UiEvent::PointerUp {
                pos: p,
                button,
                modifiers: Modifiers::NONE,
            }
        });
    }
}

fn hover(s: &mut Shell<NullBackend>, p: Point) {
    s.handle(&UiEvent::PointerMove {
        pos: p,
        modifiers: Modifiers::NONE,
    });
}

fn header(s: &Shell<NullBackend>, k: &str) -> Point {
    s.core().arrangement().track_header_center(&key(k)).unwrap()
}

/// Right-clicks the header of track `k` and opens the submenu `label`.
fn open_submenu(s: &mut Shell<NullBackend>, k: &str, label: &str) {
    let at = header(s, k);
    press(s, at, PointerButton::Secondary);
    let sub = s
        .context_menu()
        .and_then(|m| m.submenu_center(label))
        .unwrap_or_else(|| panic!("a {label} submenu"));
    hover(s, sub);
    assert_eq!(s.context_menu().unwrap().open_path(), [label]);
}

fn choose(s: &mut Shell<NullBackend>, command: &str, arg: Option<&str>) {
    let menu = s.context_menu().expect("a menu is open");
    let at = match arg {
        Some(arg) => menu.item_center_with(command, arg),
        None => menu.item_center(command),
    }
    .unwrap_or_else(|| panic!("{command} {arg:?} is in the open menu"));
    press(s, at, PointerButton::Primary);
}

fn picks(s: &Shell<NullBackend>) -> Vec<(String, Option<[u8; 3]>)> {
    s.context_menu()
        .unwrap()
        .all_items()
        .into_iter()
        .filter(|i| i.command == "sequencer.duck_from_pick")
        .map(|i| (i.arg.clone().unwrap(), i.swatch))
        .collect()
}

#[test]
fn the_header_menu_lists_the_other_tracks_in_their_colour_and_a_choice_ducks_in_one_step() {
    let mut s = shell(&four());
    s.core_mut()
        .apply_command(Command::Song(SongCommand::SetTrackColor {
            device: key("a"),
            color: Some([200, 40, 30]),
        }))
        .unwrap();
    open_submenu(&mut s, "b", "Duck from");
    let post: Vec<_> = picks(&s)
        .into_iter()
        .filter(|(arg, _)| arg.starts_with("post:"))
        .collect();
    assert_eq!(
        post,
        [
            ("post:a".to_owned(), Some([200, 40, 30])),
            ("post:c".to_owned(), None),
            ("post:d".to_owned(), None),
        ],
        "the other tracks in song order, with the colour each shows"
    );
    let steps = s.core().document().history_len();
    choose(&mut s, "sequencer.duck_from_pick", Some("post:a"));
    assert!(s.context_menu().is_none());
    assert_eq!(
        s.core().document().history_len(),
        steps + 1,
        "one undo step"
    );
    assert_eq!(s.core().document().undo_label(), Some("Duck from Track"));
    let ducker = duckers_of(s.core().doc(), s.core().registry(), &key("b"));
    assert_eq!(ducker.len(), 1);
    assert_eq!(ducker[0].source, Some(key("a")));
    assert!(!ducker[0].pre_fader);
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.starts_with("b ducks under a. Adjust: Threshold"))
    );
}

#[test]
fn the_pre_fader_submenu_takes_the_key_before_the_fader() {
    let mut s = shell(&four());
    open_submenu(&mut s, "c", "Duck from (pre-fader)");
    choose(&mut s, "sequencer.duck_from_pick", Some("pre:a"));
    let ducker = duckers_of(s.core().doc(), s.core().registry(), &key("c"));
    assert_eq!(ducker.len(), 1);
    assert!(ducker[0].pre_fader);
}

#[test]
fn the_same_pair_again_selects_the_device_and_the_menu_says_it_is_ducking() {
    let mut s = shell(&four());
    open_submenu(&mut s, "b", "Duck from");
    choose(&mut s, "sequencer.duck_from_pick", Some("post:a"));
    let steps = s.core().document().history_len();
    open_submenu(&mut s, "b", "Duck from");
    let labels: Vec<String> = s
        .context_menu()
        .unwrap()
        .all_items()
        .into_iter()
        .filter(|i| i.arg.as_deref() == Some("post:a"))
        .map(|i| i.label.clone())
        .collect();
    assert_eq!(labels, ["a (ducking)"]);
    choose(&mut s, "sequencer.duck_from_pick", Some("post:a"));
    assert_eq!(s.core().document().history_len(), steps, "nothing added");
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.starts_with("Already ducking from a"))
    );
}

#[test]
fn remove_ducking_and_gentle_pump_are_in_the_header_menu() {
    let mut s = shell(&four());
    let at = header(&s, "b");
    press(&mut s, at, PointerButton::Secondary);
    let items: Vec<(String, bool)> = s
        .context_menu()
        .unwrap()
        .items()
        .map(|i| (i.command.clone(), i.enabled))
        .collect();
    let on = |id: &str| items.iter().find(|(c, _)| c == id).map(|(_, e)| *e);
    assert_eq!(on("sequencer.gentle_pump"), Some(true));
    assert_eq!(
        on("sequencer.remove_ducking"),
        Some(false),
        "nothing to remove yet"
    );
    choose(&mut s, "sequencer.gentle_pump", None);
    assert_eq!(s.core().document().undo_label(), Some("Add Gentle Pump"));

    press(&mut s, at, PointerButton::Secondary);
    let items: Vec<(String, bool)> = s
        .context_menu()
        .unwrap()
        .items()
        .map(|i| (i.command.clone(), i.enabled))
        .collect();
    let on = |id: &str| items.iter().find(|(c, _)| c == id).map(|(_, e)| *e);
    assert_eq!(on("sequencer.gentle_pump"), Some(false), "only one");
    assert_eq!(on("sequencer.remove_ducking"), Some(true));
    choose(&mut s, "sequencer.remove_ducking", None);
    assert_eq!(s.core().document().undo_label(), Some("Remove Ducking"));
    assert!(duckers_of(s.core().doc(), s.core().registry(), &key("b")).is_empty());
}

#[test]
fn a_long_list_stops_at_thirty_rows_and_more_opens_the_pop_up_of_every_track() {
    let names: Vec<String> = (0..36).map(|i| format!("t{i:02}")).collect();
    let mut s = shell(&names);
    open_submenu(&mut s, "t00", "Duck from");
    let all = s.context_menu().unwrap().all_items();
    let post = all
        .iter()
        .filter(|i| {
            i.command == "sequencer.duck_from_pick"
                && i.arg.as_deref().unwrap().starts_with("post:")
        })
        .count();
    assert_eq!(post, 30, "thirty rows, not thirty-five");
    assert!(
        all.iter()
            .any(|i| i.command == "sequencer.duck_from" && i.label == "More...")
    );
    choose(&mut s, "sequencer.duck_from", None);
    let popup = s.context_menu().expect("the pop-up of the shared list");
    let rows: Vec<&str> = popup
        .all_items()
        .into_iter()
        .filter(|i| i.command == "sequencer.duck_from_pick")
        .map(|i| i.arg.as_deref().unwrap())
        .collect();
    assert_eq!(rows.len(), 35, "every other track");
    assert_eq!(rows[0], "post:t01");
    assert_eq!(rows[34], "post:t35");
    // A choice in the pop-up ducks the track the menu was opened on.
    let at = popup
        .item_center_with("sequencer.duck_from_pick", "post:t01")
        .unwrap();
    press(&mut s, at, PointerButton::Primary);
    let ducker = duckers_of(s.core().doc(), s.core().registry(), &key("t00"));
    assert_eq!(ducker[0].source, Some(key("t01")));
}

/// A shell with the four tracks and a delay at the top of the rack, the rack focused.
fn rack_shell() -> Shell<NullBackend> {
    let mut s = shell(&four());
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: 0,
            type_id: "atomik.stereo-delay".to_string(),
            key: Some(key("delay")),
        })
        .unwrap();
    // Tall enough for every plate of the rack to show.
    s.handle(&UiEvent::Resize(Size::new(1100.0, 2400.0)));
    s.core_mut().view_mut().set_navigator_visible(false);
    s.core_mut().after_view_change();
    s.set_focus(Pane::Rack);
    s
}

fn plate(s: &Shell<NullBackend>, k: &str) -> Point {
    let p = s.core().view().plate_center(&key(k)).unwrap();
    Point::new(p.x, p.y + s.layout().rack.y0)
}

#[test]
fn the_rack_menu_of_a_track_device_has_the_submenu_and_a_choice_ducks_that_track() {
    let mut s = rack_shell();
    let at = plate(&s, "a");
    press(&mut s, at, PointerButton::Primary);
    press(&mut s, at, PointerButton::Secondary);
    let menu = s.context_menu().expect("the device menu");
    assert!(menu.submenu_center("Duck from (pre-fader)").is_some());
    let sub = menu
        .submenu_center("Duck from")
        .expect("a Duck from submenu");
    let steps = s.core().document().history_len();
    hover(&mut s, sub);
    choose(&mut s, "sequencer.duck_from_pick", Some("post:b"));
    assert_eq!(s.core().document().history_len(), steps + 1);
    assert_eq!(
        duckers_of(s.core().doc(), s.core().registry(), &key("a"))[0].source,
        Some(key("b")),
        "the track whose device the menu was opened on ducks"
    );
}

#[test]
fn the_rack_menu_of_a_device_that_is_not_a_track_has_no_quick_sidechain() {
    let mut s = rack_shell();
    let at = plate(&s, "delay");
    press(&mut s, at, PointerButton::Primary);
    press(&mut s, at, PointerButton::Secondary);
    let menu = s.context_menu().expect("the device menu");
    assert!(
        menu.submenu_center("Duck from").is_none(),
        "a delay is not a track"
    );
    assert!(menu.items().all(|i| i.command != "sequencer.gentle_pump"));
}

#[test]
fn the_track_menu_items_open_the_pop_up_and_follow_the_selection() {
    let mut s = shell(&four());
    let model = s.menu_model();
    let track = model.iter().find(|m| m.title == "Track").unwrap();
    let item = |label: &str| track.items().find(|i| i.label == label).cloned().unwrap();
    // No track is selected yet: nothing to duck.
    assert!(!item("Gentle Pump").enabled);
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("c")));
    let model = s.menu_model();
    let track = model.iter().find(|m| m.title == "Track").unwrap();
    let item = |label: &str| track.items().find(|i| i.label == label).cloned().unwrap();
    assert!(item("Duck from\u{2026}").enabled);
    assert!(item("Duck from (Pre-Fader)\u{2026}").enabled);
    assert!(item("Gentle Pump").enabled);
    assert!(!item("Remove Ducking").enabled);

    s.execute(AppCommand::DuckFrom);
    let popup = s.context_menu().expect("a pop-up of the tracks");
    let rows = popup
        .all_items()
        .into_iter()
        .filter(|i| i.command == "sequencer.duck_from_pick")
        .count();
    assert_eq!(rows, 3);
    s.close_context_menu();
    // With one track alone there is nothing to list: the status line says so.
    let mut alone = shell(&["only".to_owned()]);
    alone
        .core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("only")));
    alone.execute(AppCommand::DuckFrom);
    assert!(alone.context_menu().is_none());
    assert!(
        alone
            .core()
            .last_rejection()
            .is_some_and(|m| m.contains("no other track"))
    );
}

// ------------------------------------------------------------------ golden

fn golden(name: &str, menu: &atomik_ui::context_menu::ContextMenu) {
    let mut list = atomik_ui::DisplayList::new();
    menu.paint(&mut list, &atomik_ui::Theme::dark());
    let actual = list.to_json();
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}_1x.json"));
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| {
            panic!(
                "missing golden {}; run with ATOMIK_UPDATE_GOLDEN=1",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{} differs (regenerate with ATOMIK_UPDATE_GOLDEN=1)",
        path.display()
    );
}

#[test]
fn the_duck_from_list_matches_golden() {
    use atomik_model::duck_from::DuckSource;
    let source = |k: &str, name: &str, color: Option<[u8; 3]>, current: bool| DuckSource {
        device: key(k),
        name: name.to_owned(),
        color,
        current,
    };
    let sources = [
        source("kick", "Kick", Some([220, 70, 60]), false),
        source("bass", "Bass", Some([60, 120, 220]), true),
        source("pad", "Pad", None, false),
        source("lead", "Lead", Some([90, 190, 100]), false),
        source("fx", "Fx", None, false),
    ];
    let rows = crate::context_menus::duck_entries(&sources, false, Some(3), "sequencer.duck_from");
    let menu = atomik_ui::context_menu::ContextMenu::from_entries(
        rows,
        Point::new(20.0, 20.0),
        Size::new(400.0, 300.0),
    );
    golden("duck_from_menu", &menu);
}
