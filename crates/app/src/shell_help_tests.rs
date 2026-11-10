//! Help Mode in a headless `Shell`: Shift+F1 and the menu item switch it on and off, the Help
//! panel docks above the status line and the song gives way, the control under the pointer is
//! explained (a transport button with its live shortcut, a knob with its range and its device),
//! clicks still reach the controls, the "More" link opens the manual, the tooltips stay quiet,
//! and the coverage listing names the controls that fall back to their tooltip.

use super::*;
use crate::help;
use crate::session::NullBackend;
use atomik_model::{Command, DeviceKey};
use atomik_ui::help_panel::HELP_PANEL_HEIGHT;
use atomik_ui::keymap::script::event_for;
use atomik_ui::{DisplayList, HELP_DELAY, Modifiers, PointerButton, TransportButton};

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

fn release(s: &mut Shell<NullBackend>, p: Point) {
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

fn play_button(s: &Shell<NullBackend>) -> Point {
    s.core()
        .transport_bar()
        .button_center(TransportButton::Play)
}

fn add_synth(s: &mut Shell<NullBackend>) -> DeviceKey {
    let key = DeviceKey("syn".into());
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key.clone()),
        })
        .unwrap();
    s.handle(&UiEvent::Resize(VIEW));
    key
}

fn control_point(s: &Shell<NullBackend>, key: &DeviceKey, param: &str) -> Point {
    let p = s.core().view().control_center(key, param).unwrap();
    let r = s.layout().rack;
    Point::new(r.x0 + p.x, r.y0 + p.y)
}

#[test]
fn shift_f1_and_the_menu_item_switch_help_mode_and_the_menu_follows() {
    let mut s = shell();
    assert!(!s.help_mode());
    assert!(!s.menu_state().help_mode);
    assert_eq!(s.help_panel_rect(), None);
    tap(&mut s, "Shift+F1");
    assert!(s.help_mode());
    assert!(s.menu_state().help_mode);
    assert!(s.help_panel_rect().is_some());
    tap(&mut s, "Shift+F1");
    assert!(!s.help_mode());
    s.execute(AppCommand::HelpMode);
    assert!(s.help_mode());
    s.execute(AppCommand::HelpMode);
    assert!(!s.help_mode());
    // F1 alone is still the shortcut list.
    tap(&mut s, "F1");
    assert!(s.dialog_open());
    assert!(!s.help_mode());
}

#[test]
fn the_panel_docks_above_the_status_line_and_the_song_gives_way() {
    let mut s = shell();
    let before = s.layout();
    s.execute(AppCommand::HelpMode);
    let after = s.layout();
    let panel = s.help_panel_rect().unwrap();
    assert_eq!(panel.height(), HELP_PANEL_HEIGHT);
    assert_eq!(panel.y1, VIEW.height - STATUS_BAR_HEIGHT);
    assert_eq!(panel.width(), VIEW.width);
    let song = after.song.unwrap();
    assert_eq!(song.y1, panel.y0, "the song ends where the panel starts");
    assert_eq!(
        before.song.unwrap().y1 - song.y1,
        HELP_PANEL_HEIGHT,
        "by the height of the panel"
    );
    s.execute(AppCommand::HelpMode);
    assert_eq!(s.layout().song, before.song, "and takes it back");
}

#[test]
fn the_idle_panel_names_the_key_that_turns_the_mode_off() {
    let mut s = shell();
    s.execute(AppCommand::HelpMode);
    let card = s.help_card();
    assert_eq!(card.title, "Help Mode");
    assert!(card.text.contains("Shift+F1"), "{}", card.text);
    assert_eq!(s.help_link_url(), None);
}

#[test]
fn hovering_the_play_button_shows_its_help_and_a_click_still_plays() {
    let mut s = shell();
    s.execute(AppCommand::HelpMode);
    let play = play_button(&s);
    hover(&mut s, play);
    let card = s.help_card();
    assert_eq!(card.title, "Play");
    assert_eq!(card.shortcut.as_deref(), Some("Space"));
    assert_eq!(card.more.as_deref(), Some("The main window"));
    assert!(card.text.contains("Starts the song"), "{}", card.text);
    assert!(s.help_link_url().is_some_and(|u| u.starts_with("https://")));
    // The mode does not block the control.
    assert!(!s.core().is_playing());
    press(&mut s, play);
    release(&mut s, play);
    assert!(s.core().is_playing(), "the click played");
    assert_eq!(s.help_card().title, "Play");
}

#[test]
fn the_panel_paints_the_card_and_tooltips_stay_quiet() {
    let mut s = shell();
    s.execute(AppCommand::HelpMode);
    let play = play_button(&s);
    hover(&mut s, play);
    s.frame_at(1.0);
    s.frame_at(1.0 + HELP_DELAY + 0.1);
    assert_eq!(s.tooltip(), None, "the panel replaces the tooltip");
    assert!(!s.tooltip_pending());
    let mut dl = DisplayList::new();
    s.paint_in(WindowKind::Main, &mut dl);
    let texts = dl.texts();
    assert!(texts.contains(&"Play"), "the title is drawn");
    assert!(texts.contains(&"Shortcut: Space"));
    assert!(texts.contains(&"More: The main window"));
    // Off again: the tooltips come back (the panes moved back too, so find the button again).
    s.execute(AppCommand::HelpMode);
    let play = play_button(&s);
    hover(&mut s, Point::new(1.0, 1.0));
    hover(&mut s, play);
    s.frame_at(5.0);
    s.frame_at(5.0 + HELP_DELAY + 0.1);
    assert!(s.tooltip().is_some());
}

#[test]
fn the_panel_keeps_its_card_over_empty_space_and_over_itself() {
    let mut s = shell();
    s.execute(AppCommand::HelpMode);
    let play = play_button(&s);
    hover(&mut s, play);
    let title = s.help_card().title;
    hover(&mut s, Point::new(VIEW.width - 2.0, 2.0));
    assert_eq!(s.help_card().title, title, "nothing under the pointer");
    let panel = s.help_panel_rect().unwrap();
    hover(&mut s, panel.center());
    assert_eq!(s.help_card().title, title, "over the panel itself");
    // Another control replaces it.
    let stop = s
        .core()
        .transport_bar()
        .button_center(TransportButton::Stop);
    hover(&mut s, stop);
    assert_eq!(s.help_card().title, "Stop");
}

#[test]
fn the_more_link_opens_the_manual_and_the_rest_of_the_panel_takes_the_press() {
    let mut s = shell();
    s.execute(AppCommand::HelpMode);
    let play = play_button(&s);
    hover(&mut s, play);
    let url = s.help_link_url().expect("the card links the manual");
    let link = s
        .help_link_rect()
        .expect("and the link has a place")
        .center();
    assert!(s.help_panel_rect().unwrap().contains(link));
    assert_eq!(press(&mut s, link), [ShellRequest::OpenUrl(url)]);
    assert!(!s.core().is_playing(), "the press did not reach a control");
    // A press elsewhere on the panel does nothing.
    let panel = s.help_panel_rect().unwrap();
    assert!(press(&mut s, Point::new(panel.x1 - 5.0, panel.y1 - 5.0)).is_empty());
}

#[test]
fn a_knob_explains_its_range_and_its_device_and_a_press_still_turns_it() {
    let mut s = shell();
    let key = add_synth(&mut s);
    s.execute(AppCommand::HelpMode);
    let at = control_point(&s, &key, "osc.mix");
    hover(&mut s, at);
    let card = s.help_card();
    let desc = s.core().registry().descriptor("atomik.poly-synth").unwrap();
    let param = desc.params.iter().find(|p| p.key == "osc.mix").unwrap();
    assert_eq!(card.title, param.name);
    assert!(card.text.contains("From "), "the range: {}", card.text);
    assert!(
        card.text.contains("Poly Synth: "),
        "the device: {}",
        card.text
    );
    assert_eq!(card.more.as_deref(), Some("Devices"));
    assert!(s.help_link_url().is_some_and(|u| u.ends_with("DEVICES.md")));
    // The mode does not block the knob: a drag changes the value.
    let before = s
        .core()
        .doc()
        .device(&key)
        .unwrap()
        .params
        .get("osc.mix")
        .copied();
    press(&mut s, at);
    s.handle(&UiEvent::PointerMove {
        pos: Point::new(at.x, at.y - 30.0),
        modifiers: Modifiers::NONE,
    });
    release(&mut s, Point::new(at.x, at.y - 30.0));
    let after = s
        .core()
        .doc()
        .device(&key)
        .unwrap()
        .params
        .get("osc.mix")
        .copied();
    assert_ne!(before, after, "the drag turned the knob");
}

/// Every control id the visible window answers with, over a grid of pointer positions.
fn ids_in_the_window(s: &mut Shell<NullBackend>) -> Vec<(String, bool)> {
    let mut seen: Vec<(String, bool)> = Vec::new();
    let mut y = 2.0;
    while y < VIEW.height {
        let mut x = 2.0;
        while x < VIEW.width {
            if let Some(h) = s.help_lookup(WindowKind::Main, Point::new(x, y), true) {
                let id = h.id.as_deref().unwrap_or(&h.text).to_owned();
                let known = h.id.as_deref().is_some_and(|i| {
                    help::book().control(i).is_some() || i.starts_with(help::PARAM_PREFIX)
                });
                if !seen.iter().any(|(i, _)| *i == id) {
                    seen.push((id, known));
                }
            }
            x += 6.0;
        }
        y += 6.0;
    }
    seen
}

#[test]
fn the_coverage_listing_names_the_controls_that_fall_back_to_their_tooltip() {
    let mut s = shell();
    add_synth(&mut s);
    let seen = ids_in_the_window(&mut s);
    assert!(seen.len() > 10, "a window full of controls: {}", seen.len());
    let gaps: Vec<&str> = seen
        .iter()
        .filter(|(_, known)| !known)
        .map(|(i, _)| i.as_str())
        .collect();
    // The controls with no entry still answer with their tooltip: the list is where the
    // coverage can grow, and the controls that matter most have none missing.
    println!(
        "{} of {} controls in the window have no help entry yet:",
        gaps.len(),
        seen.len()
    );
    for gap in &gaps {
        println!("  {gap}");
    }
    for (id, known) in &seen {
        if id.starts_with("transport.") || id.starts_with("arrangement.tool_") {
            assert!(known, "{id} has no help entry");
        }
    }
}

#[test]
fn every_transport_button_has_help_in_the_panel() {
    let mut s = shell();
    s.execute(AppCommand::HelpMode);
    for (button, title) in [
        (TransportButton::ReturnToZero, "Return to start"),
        (TransportButton::Stop, "Stop"),
        (TransportButton::Play, "Play"),
        (TransportButton::Record, "Record"),
        (TransportButton::AutoLanes, "Auto-create lanes"),
        (TransportButton::Loop, "Loop"),
        (TransportButton::Metronome, "Metronome"),
    ] {
        let at = s.core().transport_bar().button_center(button);
        hover(&mut s, at);
        let card = s.help_card();
        assert_eq!(card.title, title, "{button:?}");
        assert!(!card.text.is_empty(), "{button:?} says what it does");
        assert!(card.more.is_some(), "{button:?} links the manual");
    }
}
