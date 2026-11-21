//! The shell's answer to "do you want fast frames": it turns true for a tooltip that waits for
//! the pointer and false again once it shows, and a window with no area is never laid out.

use super::*;
use crate::session::NullBackend;
use atomik_ui::{DisplayList, HELP_DELAY, TransportButton};

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
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

#[test]
fn a_shell_at_rest_wants_no_fast_frames() {
    let mut s = shell();
    s.frame_at(1.0);
    assert!(!s.wants_fast_frames(1.0));
}

#[test]
fn a_tooltip_that_waits_for_the_pointer_wants_frames_until_it_shows() {
    let mut s = shell();
    let play = s
        .core()
        .transport_bar()
        .button_center(TransportButton::Play);
    s.handle(&UiEvent::PointerMove {
        pos: play,
        modifiers: Modifiers::NONE,
    });
    s.frame_at(1.0);
    assert!(s.wants_fast_frames(1.0), "the delay is counted in frames");
    s.frame_at(1.0 + HELP_DELAY + 0.05);
    assert!(s.tooltip().is_some());
    assert!(
        !s.wants_fast_frames(2.0 + HELP_DELAY),
        "shown: nothing left to wait for"
    );
}

#[test]
fn playing_wants_frames_and_stopping_lets_them_go() {
    let mut s = shell();
    s.core_mut().handle_action(atomik_ui::UiAction::Transport(
        atomik_ui::TransportAction::Play,
    ));
    s.frame_at(1.0);
    assert!(s.wants_fast_frames(1.0));
    s.core_mut().handle_action(atomik_ui::UiAction::Transport(
        atomik_ui::TransportAction::Stop,
    ));
    for i in 0..5 {
        s.frame_at(2.0 + f64::from(i));
    }
    assert!(!s.wants_fast_frames(8.0));
}

/// Every pane open, in the main window or in windows of its own.
fn every_pane_open(s: &mut Shell<NullBackend>) {
    for command in [
        AppCommand::ToggleConsole,
        AppCommand::ToggleFileBrowser,
        AppCommand::ToggleSpectrumEq,
        AppCommand::TogglePianoKeys,
        AppCommand::DetachRack,
        AppCommand::DetachConsole,
        AppCommand::DetachBrowser,
        AppCommand::DetachFretboard,
    ] {
        s.execute(command);
    }
}

const WINDOWS: [WindowKind; 8] = [
    WindowKind::Main,
    WindowKind::Rack,
    WindowKind::Console,
    WindowKind::SpectrumEq,
    WindowKind::Browser,
    WindowKind::PianoKeys,
    WindowKind::Video,
    WindowKind::Fretboard,
];

#[test]
fn a_resize_to_no_area_is_not_laid_out_and_every_other_size_is_survived() {
    let mut s = shell();
    every_pane_open(&mut s);
    let open: Vec<_> = WINDOWS
        .into_iter()
        .filter(|k| s.window_size(*k).is_some())
        .collect();
    assert!(open.len() >= 3, "panes of their own are open: {open:?}");
    for kind in open {
        let before = s.window_size(kind).expect("open");
        for (w, h) in [(0.0, 0.0), (0.0, 600.0), (800.0, 0.0)] {
            s.handle_in(kind, &UiEvent::Resize(Size::new(w, h)));
            assert_eq!(s.window_size(kind), Some(before), "{kind:?} at {w}x{h}");
            let mut list = DisplayList::new();
            s.paint_in(kind, &mut list);
        }
        for (w, h) in [(1.0, 1.0), (50.0, 50.0), (8192.0, 8192.0)] {
            s.handle_in(kind, &UiEvent::Resize(Size::new(w, h)));
            assert_eq!(s.window_size(kind), Some(Size::new(w, h)), "{kind:?}");
            let mut list = DisplayList::new();
            s.paint_in(kind, &mut list);
        }
    }
}
