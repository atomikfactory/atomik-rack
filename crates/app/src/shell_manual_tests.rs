//! The manual in a headless `Shell`: the Help menu item and its key open and close it, it takes
//! the keyboard, the "More" link of Help Mode opens it at the section, the command palette lists
//! "Manual: <heading>" and opens it there, a link that leaves the manual opens a fixed address or
//! says it cannot, and a click outside or Esc closes it.

use super::*;
use crate::session::NullBackend;
use atomik_ui::keymap::script::event_for;
use atomik_ui::{ManualAction, Modifiers, PointerButton, TransportButton};

const VIEW: Size = Size::new(1200.0, 900.0);

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: Some(atomik_ui::keymap::Profile::Standard),
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(VIEW));
    s
}

fn tap(s: &mut Shell<NullBackend>, chord: &str) {
    let (key, modifiers) = event_for(Os::Windows, chord).unwrap();
    for pressed in [true, false] {
        s.handle(&UiEvent::Key {
            key: key.clone(),
            modifiers,
            pressed,
        });
    }
}

fn hover(s: &mut Shell<NullBackend>, p: Point) {
    s.handle(&UiEvent::PointerMove {
        pos: p,
        modifiers: Modifiers::NONE,
    });
}

fn press(s: &mut Shell<NullBackend>, p: Point) -> Vec<ShellRequest> {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    })
}

fn type_text(s: &mut Shell<NullBackend>, text: &str) {
    for c in text.chars() {
        let key = if c == ' ' { Key::Space } else { Key::Char(c) };
        for pressed in [true, false] {
            s.handle(&UiEvent::Key {
                key: key.clone(),
                modifiers: Modifiers::NONE,
                pressed,
            });
        }
    }
}

fn title(s: &Shell<NullBackend>) -> String {
    s.manual_view().expect("the manual is open").title()
}

#[test]
fn the_command_is_in_the_help_menu_with_its_key() {
    let s = shell();
    let item = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "Help")
        .and_then(|m| m.items().find(|i| i.command == AppCommand::Manual).cloned())
        .expect("Manual in the Help menu");
    assert_eq!(item.label, "Manual");
    assert_eq!(item.shortcut.as_deref(), Some("Ctrl+F1"));
    let count = s
        .menu_model()
        .iter()
        .flat_map(|m| m.items())
        .filter(|i| i.command == AppCommand::Manual)
        .count();
    assert_eq!(count, 1);
    assert_eq!(AppCommand::Manual.command_id(), Some(("help.manual", None)));
    // Documentation (the project page) stays.
    assert!(
        s.menu_model()
            .iter()
            .flat_map(|m| m.items())
            .any(|i| i.command == AppCommand::Documentation)
    );
}

#[test]
fn the_key_and_the_menu_open_the_manual_and_close_it_again() {
    let mut s = shell();
    assert!(!s.manual_open());
    tap(&mut s, "Ctrl+F1");
    assert!(s.manual_open());
    assert!(
        s.modal_open(),
        "the other windows give way while it is open"
    );
    assert!(s.wants_text_input());
    assert_eq!(title(&s), "Documentation");
    tap(&mut s, "Ctrl+F1");
    assert!(!s.manual_open());
    s.execute(AppCommand::Manual);
    assert!(s.manual_open());
    s.execute(AppCommand::Manual);
    assert!(!s.manual_open());
    s.execute(AppCommand::Manual);
    tap(&mut s, "Escape");
    assert!(!s.manual_open());
}

#[test]
fn it_takes_the_keyboard_and_a_click_outside_closes_it() {
    let mut s = shell();
    s.execute(AppCommand::Manual);
    // Space is text for the search field, not the transport.
    type_text(&mut s, "help mode");
    assert_eq!(s.manual_view().unwrap().query(), "help mode");
    assert!(!s.core().is_playing());
    assert_eq!(s.manual_view().unwrap().hits()[0].title, "Help Mode");
    // Esc clears the search, a second Esc closes.
    tap(&mut s, "Escape");
    assert!(s.manual_open());
    assert_eq!(s.manual_view().unwrap().query(), "");
    tap(&mut s, "Escape");
    assert!(!s.manual_open());
    s.execute(AppCommand::Manual);
    press(&mut s, Point::new(2.0, 400.0));
    assert!(!s.manual_open(), "a click outside gives way");
}

#[test]
fn the_more_link_of_help_mode_opens_the_manual_at_the_section() {
    let mut s = shell();
    s.execute(AppCommand::HelpMode);
    let play = s
        .core()
        .transport_bar()
        .button_center(TransportButton::Play);
    hover(&mut s, play);
    assert_eq!(s.help_card().more.as_deref(), Some("The main window"));
    assert_eq!(s.help_link_url(), None, "the link no longer leaves the app");
    assert_eq!(s.help_link_section(), Some("The main window"));
    let link = s.help_link_rect().expect("the link has a place").center();
    assert!(press(&mut s, link).is_empty());
    assert!(s.manual_open());
    assert_eq!(title(&s), "The main window");
    assert!(!s.core().is_playing());
}

#[test]
fn a_device_card_opens_the_devices_section() {
    let mut s = shell();
    s.execute(AppCommand::HelpMode);
    // The link is the same for every control that names the section.
    s.open_manual(Some("Devices"));
    assert_eq!(title(&s), "Devices");
}

#[test]
fn the_palette_lists_the_headings_and_opens_the_manual_there() {
    let mut s = shell();
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "manual: launcher");
    let v = s.palette_view().unwrap();
    let first = v.entry_at(0).expect("a row");
    assert_eq!(first.name, "Manual: Launcher");
    assert_eq!(first.path, "Help > Manual");
    tap(&mut s, "Enter");
    assert!(!s.palette_open());
    assert!(s.manual_open());
    assert_eq!(title(&s), "Launcher");
    // The keyboard reference is searchable too, by its sections.
    s.execute(AppCommand::Manual);
    s.execute(AppCommand::CommandPalette);
    type_text(&mut s, "manual keyboard standard keys");
    let v = s.palette_view().unwrap();
    assert_eq!(
        v.entry_at(0).unwrap().name,
        "Manual: Keyboard: Standard keys"
    );
}

#[test]
fn a_link_out_of_the_manual_opens_a_fixed_address_or_says_it_cannot() {
    let mut s = shell();
    s.execute(AppCommand::Manual);
    let requests = s.manual_action(ManualAction::Open("DEVICES.md#poly-synth".into()));
    assert_eq!(
        requests,
        [ShellRequest::OpenUrl(
            "https://github.com/atomikfactory/atomik-rack/blob/main/docs/DEVICES.md"
        )]
    );
    assert!(s.manual_open(), "the manual stays open");
    assert!(
        s.manual_action(ManualAction::Open("https://example.org/x".into()))
            .is_empty()
    );
    assert!(s.manual_open());
}

#[test]
fn the_panel_follows_the_window() {
    let mut s = shell();
    s.execute(AppCommand::Manual);
    let before = s.manual_view().unwrap().panel();
    s.handle(&UiEvent::Resize(Size::new(900.0, 600.0)));
    let after = s.manual_view().unwrap().panel();
    assert_ne!(before, after);
    assert!(after.x1 <= 900.0 && after.y1 <= 600.0);
}
