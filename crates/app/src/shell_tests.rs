//! `Shell` tests: layout, routing, commands and requests, headless (null audio backend).

use super::*;
use crate::session::NullBackend;
use atomik_model::{Command, DeviceKey};
use atomik_ui::{CableVisibility, DisplayList, Key, Modifiers, PointerButton};

fn shell_with(in_window_menu: bool, settings_path: Option<PathBuf>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu,
            settings_path,
            ui_state_path: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(2.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 700.0)));
    s
}

fn shell() -> Shell<NullBackend> {
    shell_with(true, None)
}

fn ctrl(c: char) -> UiEvent {
    UiEvent::Key {
        key: Key::Char(c),
        modifiers: Modifiers::CTRL,
        pressed: true,
    }
}

/// The release that matches a press (a held key would be a repeat).
fn released(event: UiEvent) -> UiEvent {
    match event {
        UiEvent::Key { key, modifiers, .. } => UiEvent::Key {
            key,
            modifiers,
            pressed: false,
        },
        other => other,
    }
}

fn press(key: Key) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
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

fn add(s: &mut Shell<NullBackend>, k: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(DeviceKey(k.into())),
        })
        .unwrap();
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-shell-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn the_rack_view_gets_the_top_pane_below_the_menu_bar() {
    let s = shell();
    let rack = s.layout().rack;
    assert_eq!(rack.y0, MENU_BAR_HEIGHT);
    assert_eq!(s.view_size(), rack.size());
    assert_eq!(s.physical_size(), (2000, 1400));
    let (w, h) = s.core().view().physical_size();
    assert_eq!((w, h), (2000, (rack.height() * 2.0).ceil() as u32));
    let native = shell_with(false, None);
    assert_eq!(native.layout().rack.y0, 0.0);
}

#[test]
fn file_shortcuts_ask_the_platform_for_dialogs() {
    let mut s = shell();
    assert_eq!(s.handle(&ctrl('s')), vec![ShellRequest::PickSaveFile]);
    s.handle(&released(ctrl('s')));
    assert_eq!(s.handle(&ctrl('o')), vec![ShellRequest::PickOpenFile]);
    s.handle(&released(ctrl('o')));
    assert_eq!(
        s.handle(&UiEvent::Key {
            key: Key::Char('S'),
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::CTRL
            },
            pressed: true,
        }),
        vec![ShellRequest::PickSaveFile]
    );
    assert_eq!(s.handle(&ctrl('q')), vec![ShellRequest::Quit]);
}

#[test]
fn unsaved_changes_are_confirmed_before_they_are_discarded() {
    let mut s = shell();
    add(&mut s, "a");
    for (c, cmd) in [
        ('o', AppCommand::Open),
        ('n', AppCommand::New),
        ('q', AppCommand::Quit),
    ] {
        assert_eq!(s.handle(&ctrl(c)), vec![ShellRequest::ConfirmDiscard(cmd)]);
    }
    assert_eq!(
        s.execute(AppCommand::StressRack),
        vec![ShellRequest::ConfirmDiscard(AppCommand::StressRack)]
    );
    assert_eq!(
        s.confirmed(AppCommand::Open),
        vec![ShellRequest::PickOpenFile]
    );
    assert_eq!(s.confirmed(AppCommand::Quit), vec![ShellRequest::Quit]);
    assert_eq!(s.confirmed(AppCommand::New), vec![]);
    // The starter project: the master and nothing else.
    assert_eq!(s.core().doc().devices.len(), 1);
    assert!(s.core().doc().console.master.is_some());
}

#[test]
fn save_then_open_round_trips_through_files() {
    let path = temp("shell.atomik");
    let mut s = shell();
    add(&mut s, "synth");
    s.save_to(&path);
    assert_eq!(s.title(), "Atomik Rack \u{2014} shell.atomik");
    assert_eq!(s.handle(&ctrl('s')), vec![], "saves in place now");
    let mut t = shell();
    t.open_file(&path);
    assert_eq!(t.core().doc(), s.core().doc());
    t.open_file(&temp("missing.json"));
    assert!(t.core().last_rejection().is_some(), "errors are shown");
    // The synth, its automatic channel and the master.
    assert_eq!(t.core().doc().devices.len(), 3);
    std::fs::remove_file(&path).ok();
}

#[test]
fn undo_shortcuts_step_exactly_once() {
    let mut s = shell();
    add(&mut s, "a");
    add(&mut s, "b");
    // Each synth comes with its channel (the first also with the master): one undo step.
    assert_eq!(s.core().doc().devices.len(), 5);
    assert!(s.handle(&ctrl('z')).is_empty());
    assert_eq!(s.core().doc().devices.len(), 3);
    s.handle(&ctrl('y'));
    assert_eq!(s.core().doc().devices.len(), 5);
}

#[test]
fn view_commands_change_the_view() {
    let mut s = shell();
    s.handle(&press(Key::Tab));
    assert_eq!(s.core().view().flip(), Flip::Back);
    assert_eq!(s.menu_state().flip, Flip::Back);
    s.execute(AppCommand::Cables(CableVisibility::Hide));
    assert_eq!(s.core().view().cable_visibility(), CableVisibility::Hide);
    s.execute(AppCommand::ZoomIn);
    assert!(s.core().view().zoom() > 1.0);
    s.execute(AppCommand::ZoomReset);
    assert!((s.core().view().zoom() - 1.0).abs() < 1e-9);
    s.execute(AppCommand::ZoomOut);
    assert!(s.core().view().zoom() < 1.0);
    s.execute(AppCommand::ToggleBrowser);
    assert!(s.core().view().browser_visible());
    assert!(s.menu_state().browser);
}

#[test]
fn menu_bar_clicks_run_commands_and_never_reach_the_rack() {
    let mut s = shell();
    let m = s.menu_model();
    let view = MenuBar::title_rects(&m)[2].center();
    assert!(s.handle(&down(view)).is_empty());
    let (_, rows) = MenuBar::dropdown(&m, 2).unwrap();
    let back = rows[0].center();
    s.handle(&UiEvent::PointerMove {
        pos: back,
        modifiers: Modifiers::NONE,
    });
    s.handle(&up(back));
    assert_eq!(s.core().view().flip(), Flip::Back);
}

#[test]
fn rack_events_are_offset_below_the_menu_bar() {
    let mut s = shell();
    add(&mut s, "synth");
    let key = DeviceKey("synth".into());
    // The plate centre in view coordinates, moved to window coordinates.
    let plate = s.core().view().plate_center(&key).unwrap();
    let window = Point::new(plate.x, plate.y + MENU_BAR_HEIGHT);
    s.handle(&down(window));
    s.handle(&up(window));
    assert_eq!(s.core().view().selected(), Some(&key));
}

#[test]
fn the_status_bar_swallows_pointer_events() {
    let mut s = shell();
    let p = Point::new(500.0, 700.0 - STATUS_BAR_HEIGHT / 2.0);
    assert!(s.handle(&down(p)).is_empty());
    assert!(s.handle(&up(p)).is_empty());
}

#[test]
fn audio_settings_open_a_modal_that_applies_and_saves() {
    let path = temp("settings/settings.json");
    let _ = std::fs::remove_file(&path);
    let mut s = shell_with(true, Some(path.clone()));
    s.execute(AppCommand::AudioSettings);
    assert!(s.dialog_open());
    // Modal: shortcuts do not run.
    assert!(s.handle(&ctrl('o')).is_empty());
    s.handle(&press(Key::Escape));
    assert!(!s.dialog_open());
    assert!(!path.exists(), "cancel saves nothing");
    s.handle(&UiEvent::Key {
        key: Key::Char(','),
        modifiers: Modifiers::CTRL,
        pressed: true,
    });
    assert!(s.dialog_open());
    s.handle(&press(Key::Enter));
    assert!(!s.dialog_open());
    let (saved, err) = settings::load(&path);
    assert!(err.is_none());
    assert_eq!(saved, AudioSettings::default());
}

#[test]
fn the_stress_rack_opens_from_the_menu() {
    let mut s = shell();
    assert!(s.execute(AppCommand::StressRack).is_empty());
    assert_eq!(s.core().doc().devices.len(), STRESS_DEVICES);
    assert_eq!(s.core().doc().cables.len(), STRESS_CABLES);
}

#[test]
fn restart_audio_enables_the_session_and_rebuilds() {
    let mut s = shell();
    add(&mut s, "a");
    assert!(!s.core().sink().is_enabled());
    s.execute(AppCommand::RestartAudio);
    assert!(s.core().sink().is_enabled());
    // The null backend cannot start: the status says why.
    s.poll_status();
    assert!(s.status_text().contains("audio off"), "{}", s.status_text());
    assert!(
        s.status_text().contains("null backend"),
        "{}",
        s.status_text()
    );
}

#[test]
fn polling_reports_status_changes_once() {
    let mut s = shell();
    s.poll_status();
    assert!(!s.poll_status(), "unchanged");
    assert!(
        s.status_text()
            .starts_with(&s.core().sink().settings().host.label().to_string())
    );
}

#[test]
fn painting_draws_view_status_menu_and_dialog() {
    let mut s = shell();
    s.poll_status();
    let mut list = DisplayList::new();
    s.paint(&mut list);
    let texts = list.texts();
    assert!(texts.contains(&"File"), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("audio off")));
    assert!(!s.needs_repaint());
    s.execute(AppCommand::AudioSettings);
    assert!(s.needs_repaint());
    let mut list = DisplayList::new();
    s.paint(&mut list);
    assert!(list.texts().contains(&"Audio settings"));
}

#[test]
fn the_native_menu_layout_has_no_in_window_bar() {
    let mut s = shell_with(false, None);
    let mut list = DisplayList::new();
    s.paint(&mut list);
    assert!(!list.texts().contains(&"File"));
}
