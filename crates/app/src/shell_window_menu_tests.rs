//! The menu bar in the detached rack and console windows: the same bar over the same menu
//! model as the main window, commands that act on the pane of the window they were chosen in,
//! one open menu across all windows, and the layout below the bar. Headless (null audio
//! backend). The tool windows and a native macOS menu bar keep no bar.

use super::*;
use crate::commands::{MenuEntry, MenuItem};
use crate::session::NullBackend;
use crate::windows::WindowKind;
use atomik_model::{Command, DeviceKey};
use atomik_ui::keymap::Os;
use atomik_ui::{DisplayList, Key, Modifiers, Op, PointerButton};

const RACK: Size = Size::new(1000.0, 1200.0);
const CONSOLE: Size = Size::new(1000.0, 1200.0);

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

fn add_synth(s: &mut Shell<NullBackend>, k: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(DeviceKey(k.into())),
        })
        .unwrap();
}

/// A shell with two synths and both panes in windows of their own.
fn both_windows() -> Shell<NullBackend> {
    let mut s = shell_with(Os::Windows, true);
    add_synth(&mut s, "a");
    add_synth(&mut s, "b");
    s.execute(AppCommand::DetachRack);
    s.execute(AppCommand::DetachConsole);
    s.handle_in(WindowKind::Rack, &UiEvent::Resize(RACK));
    s.handle_in(WindowKind::Console, &UiEvent::Resize(CONSOLE));
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

fn key(k: Key, modifiers: Modifiers, pressed: bool) -> UiEvent {
    UiEvent::Key {
        key: k,
        modifiers,
        pressed,
    }
}

fn tap(s: &mut Shell<NullBackend>, window: WindowKind, k: Key, modifiers: Modifiers) {
    s.handle_in(window, &key(k.clone(), modifiers, true));
    s.handle_in(window, &key(k, modifiers, false));
}

fn title_index(s: &Shell<NullBackend>, title: &str) -> usize {
    s.menu_model()
        .iter()
        .position(|m| m.title == title)
        .unwrap()
}

fn title_centre(s: &Shell<NullBackend>, title: &str) -> Point {
    MenuBar::title_rects(&s.menu_model())[title_index(s, title)].center()
}

/// Presses the title `title` of the bar in `window`: its menu opens.
fn open_title(s: &mut Shell<NullBackend>, window: WindowKind, title: &str) {
    let p = title_centre(s, title);
    s.handle_in(window, &moved(p));
    s.handle_in(window, &down(p));
}

fn entry_row(entries: &[MenuEntry], label: &str) -> usize {
    entries
        .iter()
        .position(|e| match e {
            MenuEntry::Item(i) => i.label == label,
            MenuEntry::Submenu(m) => m.label == label,
            MenuEntry::Separator => false,
        })
        .unwrap_or_else(|| panic!("no entry {label}"))
}

/// Chooses `path` (a title, an optional submenu, then an item) in the bar of `window` with the
/// pointer: press the title, move to the entry, release on it.
fn choose(s: &mut Shell<NullBackend>, window: WindowKind, path: &[&str]) {
    let menus = s.menu_model();
    let ti = title_index(s, path[0]);
    open_title(s, window, path[0]);
    let (_, rows) = MenuBar::dropdown(&menus, ti).unwrap();
    let row = entry_row(&menus[ti].entries, path[1]);
    let first = rows[row].center();
    if path.len() == 2 {
        s.handle_in(window, &moved(first));
        s.handle_in(window, &up(first));
        return;
    }
    s.handle_in(window, &moved(first));
    s.handle_in(window, &down(first));
    s.handle_in(window, &up(first));
    let MenuEntry::Submenu(sub) = &menus[ti].entries[row] else {
        panic!("{} is not a submenu", path[1]);
    };
    let (_, fly) = MenuBar::flyout(&menus, ti, row).unwrap();
    let inner = entry_row(&sub.entries, path[2]);
    let target = fly[inner].center();
    s.handle_in(window, &moved(target));
    s.handle_in(window, &up(target));
}

fn item(menus: &[crate::commands::Menu], command: AppCommand) -> &MenuItem {
    menus
        .iter()
        .flat_map(|m| m.items())
        .find(|i| i.command == command)
        .unwrap()
}

fn paint(s: &mut Shell<NullBackend>, window: WindowKind) -> DisplayList {
    let mut list = DisplayList::new();
    s.paint_in(window, &mut list);
    list
}

/// The ops of the menu bar in a window's display list: from the scale transform that opens the
/// logical-unit overlay up to the last title.
fn bar_ops(list: &DisplayList) -> String {
    let ops = list.ops();
    let end = ops
        .iter()
        .rposition(|o| matches!(o, Op::Text(t) if t.text == "Help"))
        .expect("the Help title");
    let start = ops[..end]
        .iter()
        .rposition(|o| matches!(o, Op::PushTransform(_)))
        .expect("the scale transform");
    let mut out: String = ops[start..=end]
        .iter()
        .map(|o| format!("{o:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

fn golden(name: &str, actual: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::write(&path, actual).unwrap();
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

/// The menu titles a display list draws: their baseline is the bar's.
fn titles(list: &DisplayList) -> Vec<String> {
    list.ops()
        .iter()
        .filter_map(|o| match o {
            Op::Text(t) if (t.pos.y - 17.5).abs() < 1e-9 => Some(t.text.clone()),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------ painting

#[test]
fn each_detached_window_paints_the_menu_bar_over_the_one_menu_model() {
    let mut s = both_windows();
    let model: Vec<String> = s.menu_model().iter().map(|m| m.title.to_owned()).collect();
    let main = titles(&paint(&mut s, WindowKind::Main));
    assert_eq!(main, model);
    for window in [WindowKind::Rack, WindowKind::Console] {
        assert_eq!(titles(&paint(&mut s, window)), model, "{window:?}");
    }
}

#[test]
fn golden_the_rack_window_menu_bar_at_1x() {
    let mut s = both_windows();
    golden(
        "rack_window_menu_1x.txt",
        &bar_ops(&paint(&mut s, WindowKind::Rack)),
    );
}

#[test]
fn golden_the_console_window_menu_bar_at_1x() {
    let mut s = both_windows();
    golden(
        "console_window_menu_1x.txt",
        &bar_ops(&paint(&mut s, WindowKind::Console)),
    );
}

#[test]
fn golden_the_rack_window_menu_bar_at_2x() {
    let mut s = both_windows();
    s.handle_in(WindowKind::Rack, &UiEvent::ScaleChanged(2.0));
    golden(
        "rack_window_menu_2x.txt",
        &bar_ops(&paint(&mut s, WindowKind::Rack)),
    );
}

#[test]
fn an_open_menu_is_painted_over_the_pane_of_its_window() {
    let mut s = both_windows();
    let closed = paint(&mut s, WindowKind::Console);
    assert!(!closed.texts().contains(&"Select All"));
    open_title(&mut s, WindowKind::Console, "Edit");
    let open = paint(&mut s, WindowKind::Console);
    assert!(open.texts().contains(&"Select All"));
    let rack = paint(&mut s, WindowKind::Rack);
    assert!(
        !rack.texts().contains(&"Select All"),
        "the menu opens in the window it was pressed in"
    );
}

#[test]
fn the_tool_windows_and_the_browser_keep_no_menu_bar() {
    let mut s = shell_with(Os::Windows, true);
    for window in [
        WindowKind::SpectrumEq,
        WindowKind::PianoKeys,
        WindowKind::Video,
        WindowKind::Browser,
    ] {
        assert_eq!(s.bar_height_in(window), 0.0, "{window:?}");
        assert_eq!(s.menu_bar_width(window), 0.0, "{window:?}");
    }
    s.execute(AppCommand::ToggleSpectrumEq);
    s.execute(AppCommand::TogglePianoKeys);
    s.execute(AppCommand::DetachBrowser);
    for window in [
        WindowKind::SpectrumEq,
        WindowKind::PianoKeys,
        WindowKind::Browser,
    ] {
        let (_, rect) = s.pane_rect(pane_of_window(window)).unwrap();
        assert_eq!(rect.y0, 0.0, "{window:?} fills its window");
        assert!(
            !paint(&mut s, window).texts().contains(&"File"),
            "{window:?}"
        );
    }
    assert!(s.bar_height_in(WindowKind::Rack) > 0.0);
    assert!(s.bar_height_in(WindowKind::Console) > 0.0);
}

fn pane_of_window(window: WindowKind) -> Pane {
    match window {
        WindowKind::SpectrumEq => Pane::SpectrumEq,
        WindowKind::PianoKeys => Pane::PianoKeys,
        WindowKind::Browser => Pane::FileBrowser,
        WindowKind::Video => Pane::Video,
        WindowKind::Console => Pane::Console,
        WindowKind::Main | WindowKind::Rack => Pane::Rack,
    }
}

#[test]
fn a_native_menu_bar_leaves_every_window_as_it_was() {
    // macOS uses the system's menu bar: the app draws none in any window.
    let mut s = shell_with(Os::Mac, false);
    add_synth(&mut s, "a");
    s.execute(AppCommand::DetachRack);
    s.execute(AppCommand::DetachConsole);
    for window in [WindowKind::Main, WindowKind::Rack, WindowKind::Console] {
        assert_eq!(s.bar_height_in(window), 0.0, "{window:?}");
        assert_eq!(s.menu_bar_width(window), 0.0, "{window:?}");
    }
    let size = s.window_size(WindowKind::Rack).unwrap();
    assert_eq!(
        s.pane_rect(Pane::Rack),
        Some((
            WindowKind::Rack,
            Rect::new(0.0, 0.0, size.width, size.height)
        ))
    );
    for window in [WindowKind::Rack, WindowKind::Console] {
        assert!(
            !paint(&mut s, window).texts().contains(&"File"),
            "{window:?}"
        );
    }
}

// ------------------------------------------------------------------ commands

#[test]
fn edit_select_all_in_the_rack_window_selects_the_rack_devices() {
    let mut s = both_windows();
    // The keys are in the main window's arrangement, not the rack.
    s.window_focused(WindowKind::Main);
    assert_eq!(s.core().view().selection_len(), 0);
    choose(&mut s, WindowKind::Rack, &["Edit", "Select All"]);
    assert!(s.core().view().selection_len() >= 2, "both devices");
    assert_eq!(
        s.focus(),
        Pane::Rack,
        "the commands act on the window's pane"
    );
    assert_eq!(s.focused_window(), WindowKind::Rack);
    assert!(s.open_menu_window().is_none(), "choosing closes the menu");
    assert!(s.core().console().selected_strips().is_empty());
}

#[test]
fn edit_select_all_in_the_console_window_selects_the_strips() {
    let mut s = both_windows();
    s.window_focused(WindowKind::Main);
    assert!(s.core().console().selected_strips().is_empty());
    choose(&mut s, WindowKind::Console, &["Edit", "Select All"]);
    assert!(s.core().console().selected_strips().len() >= 2);
    assert_eq!(s.focus(), Pane::Console);
    assert_eq!(
        s.core().view().selection_len(),
        0,
        "the rack is not the pane of this window"
    );
}

#[test]
fn a_view_toggle_chosen_in_one_window_updates_the_check_mark_in_all() {
    let mut s = both_windows();
    assert_eq!(
        item(&s.menu_model(), AppCommand::ToggleTrackPanel).checked,
        Some(false)
    );
    choose(&mut s, WindowKind::Rack, &["View", "Panels", "Track Panel"]);
    assert!(s.menu_state().track_panel);
    assert_eq!(
        item(&s.menu_model(), AppCommand::ToggleTrackPanel).checked,
        Some(true)
    );
    // The same model in the console window and in the main window: the item is checked there.
    for window in [WindowKind::Console, WindowKind::Main] {
        s.close_menubars();
        open_title(&mut s, window, "View");
        let menus = s.menu_model();
        let ti = title_index(&s, "View");
        let row = entry_row(&menus[ti].entries, "Panels");
        let (_, rows) = MenuBar::dropdown(&menus, ti).unwrap();
        s.handle_in(window, &moved(rows[row].center()));
        s.handle_in(window, &down(rows[row].center()));
        let MenuEntry::Submenu(sub) = &menus[ti].entries[row] else {
            panic!("Panels is a submenu");
        };
        let checked = sub.entries.iter().any(|e| {
            matches!(e, MenuEntry::Item(i)
                if i.command == AppCommand::ToggleTrackPanel && i.checked == Some(true))
        });
        assert!(checked, "{window:?}");
        assert!(paint(&mut s, window).texts().contains(&"Track Panel"));
    }
}

#[test]
fn a_click_on_the_bar_never_reaches_the_pane_below() {
    let mut s = both_windows();
    s.window_focused(WindowKind::Rack);
    let away = Point::new(900.0, 10.0);
    s.handle_in(WindowKind::Rack, &down(away));
    s.handle_in(WindowKind::Rack, &up(away));
    assert_eq!(s.core().view().selection_len(), 0);
    assert!(
        s.open_menu_window().is_none(),
        "off the titles nothing opens"
    );
    s.handle_in(WindowKind::Console, &down(away));
    s.handle_in(WindowKind::Console, &up(away));
    assert!(s.core().console().selected_strips().is_empty());
}

// ------------------------------------------------------------------ one open menu

#[test]
fn only_one_menu_is_open_across_the_windows() {
    let mut s = both_windows();
    open_title(&mut s, WindowKind::Rack, "File");
    assert_eq!(s.open_menu_window(), Some(WindowKind::Rack));
    open_title(&mut s, WindowKind::Console, "Edit");
    assert_eq!(s.open_menu_window(), Some(WindowKind::Console));
    assert!(
        !paint(&mut s, WindowKind::Rack).texts().contains(&"Save"),
        "the menu of the rack window closed"
    );
    s.handle_in(WindowKind::Main, &moved(title_centre(&s, "File")));
    open_title(&mut s, WindowKind::Main, "View");
    assert_eq!(s.open_menu_window(), Some(WindowKind::Main));
    assert!(!s.bar_ref(WindowKind::Console).unwrap().is_open());
    assert!(!s.bar_ref(WindowKind::Rack).unwrap().is_open());
}

#[test]
fn a_menu_closes_when_another_window_gets_the_focus_or_the_app_loses_it() {
    let mut s = both_windows();
    open_title(&mut s, WindowKind::Rack, "File");
    s.window_focused(WindowKind::Console);
    assert_eq!(s.open_menu_window(), None, "another window took the focus");
    open_title(&mut s, WindowKind::Console, "File");
    s.focus_lost();
    assert_eq!(s.open_menu_window(), None, "the window lost the focus");
    open_title(&mut s, WindowKind::Rack, "File");
    s.handle_in(WindowKind::Console, &down(Point::new(500.0, 300.0)));
    assert_eq!(s.open_menu_window(), None, "a press in another window");
}

#[test]
fn a_window_that_closes_takes_its_open_menu_with_it() {
    let mut s = both_windows();
    open_title(&mut s, WindowKind::Rack, "File");
    s.window_closed(WindowKind::Rack);
    assert_eq!(s.open_menu_window(), None);
    assert!(s.bar_ref(WindowKind::Rack).is_none());
}

#[test]
fn the_keyboard_walks_the_menu_of_the_window_it_is_open_in() {
    let mut s = both_windows();
    open_title(&mut s, WindowKind::Rack, "File");
    assert_eq!(
        s.bar_ref(WindowKind::Rack).unwrap().highlighted_item(),
        None
    );
    tap(&mut s, WindowKind::Rack, Key::Down, Modifiers::NONE);
    assert!(
        s.bar_ref(WindowKind::Rack)
            .unwrap()
            .highlighted_item()
            .is_some(),
        "Down lights the first entry"
    );
    let first = s.bar_ref(WindowKind::Rack).unwrap().open_menu();
    tap(&mut s, WindowKind::Rack, Key::Right, Modifiers::NONE);
    assert_ne!(
        s.bar_ref(WindowKind::Rack).unwrap().open_menu(),
        first,
        "Right moves to the next menu"
    );
    tap(&mut s, WindowKind::Rack, Key::Escape, Modifiers::NONE);
    assert_eq!(s.open_menu_window(), None, "Escape closes it");
    // The console window's bar answers the keys the same way.
    open_title(&mut s, WindowKind::Console, "Edit");
    tap(&mut s, WindowKind::Console, Key::Down, Modifiers::NONE);
    assert!(
        s.bar_ref(WindowKind::Console)
            .unwrap()
            .highlighted_item()
            .is_some()
    );
    tap(&mut s, WindowKind::Console, Key::Escape, Modifiers::NONE);
    assert_eq!(s.open_menu_window(), None);
}

#[test]
fn keys_while_a_menu_is_open_never_reach_the_pane() {
    let mut s = both_windows();
    s.window_focused(WindowKind::Rack);
    open_title(&mut s, WindowKind::Rack, "Edit");
    tap(&mut s, WindowKind::Rack, Key::Char('a'), Modifiers::CTRL);
    assert_eq!(
        s.core().view().selection_len(),
        0,
        "the open menu has the keys"
    );
}

// ------------------------------------------------------------------ layout

#[test]
fn the_pane_of_a_window_starts_below_the_bar_and_hits_are_in_window_units() {
    let s = both_windows();
    let bar = s.bar_height_in(WindowKind::Rack);
    assert_eq!(bar, MENU_BAR_HEIGHT);
    let size = s.window_size(WindowKind::Rack).unwrap();
    assert_eq!(
        s.pane_rect(Pane::Rack),
        Some((
            WindowKind::Rack,
            Rect::new(0.0, bar, size.width, size.height)
        ))
    );
    assert_eq!(
        s.hit_in(WindowKind::Rack, Point::new(500.0, bar - 1.0)),
        None,
        "the bar is not the rack"
    );
    assert_eq!(
        s.hit_in(WindowKind::Rack, Point::new(500.0, bar + 1.0)),
        Some(Ok(Pane::Rack))
    );
    assert_eq!(
        s.hit_in(WindowKind::Console, Point::new(500.0, bar + 1.0)),
        Some(Ok(Pane::Console))
    );
    assert_eq!(
        s.core().console().bounds(),
        Rect::new(0.0, bar, size.width, size.height)
    );
}

#[test]
fn a_press_on_a_device_lands_with_the_bar_and_a_cross_window_drag_follows() {
    let mut s = both_windows();
    s.window_focused(WindowKind::Main);
    let key = DeviceKey("a".into());
    let r = s.core().view().device_rect(&key).unwrap();
    let in_view = s
        .core()
        .view()
        .rack_to_view(Point::new(r.x0 + 6.0, r.center().y));
    let window_point = Point::new(in_view.x, in_view.y + MENU_BAR_HEIGHT);
    s.handle_in(WindowKind::Rack, &down(window_point));
    s.handle_in(WindowKind::Rack, &up(window_point));
    assert_eq!(s.core().view().primary(), Some(key));
    // A drop of files from another window onto the bar is no drop on the rack.
    let before = s.core().doc().devices.len();
    s.foreign_file_drag(
        WindowKind::Rack,
        Point::new(500.0, 5.0),
        vec![PathBuf::from("x.wav")],
        true,
    );
    assert_eq!(s.core().doc().devices.len(), before);
}

#[test]
fn the_menu_bar_sets_the_minimum_width_of_a_window_that_carries_it() {
    let s = both_windows();
    let width = s.menu_bar_width(WindowKind::Rack);
    let titles = MenuBar::title_rects(&s.menu_model());
    assert!(width > titles.last().unwrap().x1, "every title fits");
    assert!(width < 1280.0, "the default window is wide enough");
    assert_eq!(width, s.menu_bar_width(WindowKind::Console));
    assert_eq!(width, s.menu_bar_width(WindowKind::Main));
}

#[test]
fn a_dropdown_stays_inside_a_short_window() {
    let mut s = both_windows();
    s.handle_in(
        WindowKind::Console,
        &UiEvent::Resize(Size::new(1000.0, 300.0)),
    );
    open_title(&mut s, WindowKind::Console, "View");
    let ti = title_index(&s, "View");
    let layout = MenuBar::dropdown_in(&s.menu_model(), ti, Size::new(1000.0, 300.0), 0.0).unwrap();
    assert!(layout.panel.y1 <= 300.0);
    assert!(
        paint(&mut s, WindowKind::Console)
            .texts()
            .contains(&"Panels")
    );
}

// ------------------------------------------------------------------ key profiles

#[test]
fn select_all_by_key_in_the_rack_window_with_the_windows_profile() {
    let mut s = both_windows();
    s.window_focused(WindowKind::Rack);
    tap(&mut s, WindowKind::Rack, Key::Char('a'), Modifiers::CTRL);
    assert!(s.core().view().selection_len() >= 2);
}

#[test]
fn select_all_by_key_in_the_rack_and_console_windows_with_the_macos_profile() {
    let mut s = shell_with(Os::Mac, false);
    add_synth(&mut s, "a");
    add_synth(&mut s, "b");
    s.execute(AppCommand::DetachRack);
    s.execute(AppCommand::DetachConsole);
    let command = Modifiers {
        meta: true,
        ..Modifiers::NONE
    };
    s.window_focused(WindowKind::Rack);
    tap(&mut s, WindowKind::Rack, Key::Char('a'), command);
    assert!(s.core().view().selection_len() >= 2);
    s.window_focused(WindowKind::Console);
    tap(&mut s, WindowKind::Console, Key::Char('a'), command);
    assert!(s.core().console().selected_strips().len() >= 2);
}
