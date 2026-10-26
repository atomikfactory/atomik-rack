//! The rack and the console (mixer) in windows of their own: detaching and attaching, the
//! layout of what stays in the main window, the size and scale of each window, and the saved
//! flags. Headless (null audio backend); the platform opens the windows (see `windows`).

use super::*;
use crate::session::NullBackend;
use crate::windows::WindowKind;
use atomik_model::{Command, DeviceKey};

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
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 800.0)));
    s
}

fn shell() -> Shell<NullBackend> {
    shell_at(None)
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-windows-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
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

#[test]
fn detach_rack_main_layout_has_no_rack() {
    let mut s = shell();
    assert!(s.layout().rack.height() > 0.0);
    s.execute(AppCommand::DetachRack);
    let l = s.layout();
    assert_eq!(l.rack.height(), 0.0, "the rack leaves the main window");
    assert!(l.divider.is_none());
    assert!(l.song.is_some(), "the song area takes the room");
    assert_eq!(s.windows_wanted(), [WindowKind::Main, WindowKind::Rack]);
    assert_eq!(s.window_of(Pane::Rack), WindowKind::Rack);
    assert_eq!(s.window_of(Pane::Arrangement), WindowKind::Main);
    let size = s.window_size(WindowKind::Rack).unwrap();
    assert_eq!(
        s.pane_rect(Pane::Rack),
        Some((
            WindowKind::Rack,
            Rect::new(0.0, 0.0, size.width, size.height)
        ))
    );
    assert_eq!(s.pane_rect(Pane::Console), None, "the console is not shown");
}

#[test]
fn detach_rack_keeps_rack_visible_pref() {
    let mut s = shell();
    s.execute(AppCommand::ToggleRack);
    assert!(!s.menu_state().rack);
    s.execute(AppCommand::DetachRack);
    assert!(
        !s.menu_state().rack,
        "detaching does not touch the F6 choice"
    );
    assert!(s.menu_state().rack_detached);
    assert!(s.detached(WindowKind::Rack));
    s.execute(AppCommand::DetachRack);
    assert!(!s.detached(WindowKind::Rack));
    assert!(s.menu_state().rack, "attached again, the rack shows");
    assert!(s.layout().rack.height() > 0.0);
}

#[test]
fn detach_console_lower_pane_never_console() {
    let mut s = shell();
    s.execute(AppCommand::ToggleConsole);
    assert_eq!(s.lower_pane(), Pane::Console);
    s.execute(AppCommand::DetachConsole);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    assert!(s.layout().rack.height() > 0.0);
    assert_eq!(s.window_of(Pane::Console), WindowKind::Console);
    let size = s.window_size(WindowKind::Console).unwrap();
    assert_eq!(
        s.core().console().bounds(),
        Rect::new(0.0, 0.0, size.width, size.height)
    );
    assert!(s.menu_state().console, "a detached console counts as shown");
    assert!(s.menu_state().console_detached);
    s.execute(AppCommand::DetachConsole);
    assert_eq!(
        s.lower_pane(),
        Pane::Console,
        "attached, it is back in the lower pane"
    );
    assert_eq!(Some(s.core().console().bounds()), s.layout().song);
}

#[test]
fn detach_console_meters_still_fed() {
    let mut s = shell();
    add_synth(&mut s, "a");
    let ch = s.core().doc().console.channels[0].channel.clone();
    s.execute(AppCommand::DetachConsole);
    s.frame_at(0.1);
    s.frame_at(0.2);
    assert!(s.core().console().meter(&ch).is_some());
}

#[test]
fn window_closed_attaches_pane() {
    let mut s = shell();
    s.execute(AppCommand::DetachRack);
    s.execute(AppCommand::DetachConsole);
    assert_eq!(
        s.windows_wanted(),
        [WindowKind::Main, WindowKind::Rack, WindowKind::Console]
    );
    s.window_closed(WindowKind::Rack);
    assert_eq!(s.windows_wanted(), [WindowKind::Main, WindowKind::Console]);
    assert!(s.layout().rack.height() > 0.0);
    s.window_closed(WindowKind::Console);
    assert_eq!(s.windows_wanted(), [WindowKind::Main]);
    assert_eq!(
        s.lower_pane(),
        Pane::Console,
        "the mixer comes back to the lower pane"
    );
    // The main window is not a pane: closing it changes nothing here (the platform quits).
    s.window_closed(WindowKind::Main);
    assert_eq!(s.windows_wanted(), [WindowKind::Main]);
}

#[test]
fn scale_changed_in_rack_window_leaves_main_views() {
    let mut s = shell();
    s.execute(AppCommand::DetachRack);
    let main_physical = s.physical_size();
    s.handle_in(WindowKind::Rack, &UiEvent::ScaleChanged(2.0));
    s.handle_in(WindowKind::Rack, &UiEvent::Resize(Size::new(900.0, 500.0)));
    assert_eq!(s.window_scale(WindowKind::Rack), Some(2.0));
    assert_eq!(
        s.window_size(WindowKind::Rack),
        Some(Size::new(900.0, 500.0))
    );
    assert_eq!(s.window_scale(WindowKind::Main), Some(1.0));
    assert_eq!(
        s.window_size(WindowKind::Main),
        Some(Size::new(1200.0, 800.0))
    );
    assert_eq!(s.physical_size(), main_physical);
    assert_eq!(s.physical_size_in(WindowKind::Rack), Some((1800, 1000)));
    assert_eq!(
        s.pane_rect(Pane::Rack),
        Some((WindowKind::Rack, Rect::new(0.0, 0.0, 900.0, 500.0)))
    );
    // The other way round: the main window's scale does not reach the rack window.
    s.handle(&UiEvent::ScaleChanged(1.5));
    assert_eq!(s.window_scale(WindowKind::Rack), Some(2.0));
    assert_eq!(s.window_scale(WindowKind::Main), Some(1.5));
    // An event for a window that is not open is ignored.
    s.handle_in(WindowKind::Console, &UiEvent::Resize(Size::new(10.0, 10.0)));
    assert_eq!(s.window_size(WindowKind::Console), None);
}

#[test]
fn detach_restored_from_ui_state() {
    let path = temp("restore.json");
    let _ = std::fs::remove_file(&path);
    {
        let mut s = shell_at(Some(path.clone()));
        s.execute(AppCommand::DetachRack);
        s.execute(AppCommand::DetachConsole);
    }
    let s = shell_at(Some(path.clone()));
    assert_eq!(
        s.windows_wanted(),
        [WindowKind::Main, WindowKind::Rack, WindowKind::Console]
    );
    assert_eq!(s.layout().rack.height(), 0.0);
    assert_ne!(s.lower_pane(), Pane::Console);
    assert!(
        s.window_size(WindowKind::Rack).is_some(),
        "sized before the platform reports"
    );
    // A fresh state file starts attached.
    let fresh = shell_at(Some(temp("restore-none.json")));
    assert_eq!(fresh.windows_wanted(), [WindowKind::Main]);
}

#[test]
fn menu_detach_items_checked_when_detached() {
    let mut s = shell();
    let checked = |s: &Shell<NullBackend>, label: &str| {
        let model = s.menu_model();
        model
            .iter()
            .flat_map(|m| m.items())
            .find(|i| i.label == label)
            .unwrap_or_else(|| panic!("no menu item {label}"))
            .checked
    };
    assert_eq!(checked(&s, "Detach Rack"), Some(false));
    assert_eq!(checked(&s, "Detach Mixer"), Some(false));
    s.execute(AppCommand::DetachRack);
    s.execute(AppCommand::DetachConsole);
    assert_eq!(checked(&s, "Detach Rack"), Some(true));
    assert_eq!(checked(&s, "Detach Mixer"), Some(true));
    assert!(s.menu_state().rack_detached && s.menu_state().console_detached);
}
