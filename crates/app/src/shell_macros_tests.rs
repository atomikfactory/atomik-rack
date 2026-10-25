//! Macro banks, the parameter search and automation that follows the selection through the shell:
//! the menu items and their keys, the right-click menu of a control (Assign to Macro and Remove
//! from Macro), the Macros panel, the search panel, and the follow switch with the lane a touched
//! control shows. Headless (null audio backend).

use super::*;
use crate::session::NullBackend;
use atomik_model::{Command, DeviceKey, MACRO_BANK_TYPE};
use atomik_ui::{Key, Modifiers, PointerButton};

const CUTOFF: &str = "filter.cutoff";

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
        },
    );
    s.handle(&UiEvent::Resize(Size::new(1400.0, 900.0)));
    s
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.into())
}

fn add_synth(s: &mut Shell<NullBackend>, k: &str, name: &str) {
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key(k)),
        })
        .unwrap();
    s.core_mut()
        .apply_command(Command::Rename {
            key: key(k),
            name: name.into(),
        })
        .unwrap();
}

fn menu_item(s: &Shell<NullBackend>, menu: &str, command: AppCommand) -> crate::commands::MenuItem {
    s.menu_model()
        .into_iter()
        .find(|m| m.title == menu)
        .and_then(|m| m.items().find(|i| i.command == command).cloned())
        .unwrap_or_else(|| panic!("{command:?} in the {menu} menu"))
}

fn mouse(s: &mut Shell<NullBackend>, p: Point, button: PointerButton) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button,
        modifiers: Modifiers::NONE,
    });
}

fn press(s: &mut Shell<NullBackend>, key: Key, modifiers: Modifiers) {
    s.handle(&UiEvent::Key {
        key: key.clone(),
        modifiers,
        pressed: true,
    });
    s.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: false,
    });
}

fn type_text(s: &mut Shell<NullBackend>, text: &str) {
    for c in text.chars() {
        if c == ' ' {
            press(s, Key::Space, Modifiers::NONE);
        } else {
            press(s, Key::Char(c), Modifiers::NONE);
        }
    }
}

const CTRL: Modifiers = Modifiers {
    shift: false,
    ctrl: true,
    alt: false,
    meta: false,
};

fn knob_at(s: &Shell<NullBackend>, device: &str, param: &str) -> Point {
    let origin = s.layout().rack.origin();
    let at = s
        .core()
        .view()
        .control_center(&key(device), param)
        .expect("the control is in view");
    Point::new(at.x + origin.x, at.y + origin.y)
}

fn value(s: &Shell<NullBackend>, device: &str, p: &str) -> f32 {
    s.core()
        .doc()
        .param_value(&key(device), p, s.core().registry())
        .unwrap()
}

// ---- menus and keys ----------------------------------------------------------------------------

#[test]
fn the_menus_have_the_macro_items_and_the_search_has_its_key() {
    let mut s = shell();
    for (menu, command, label, id) in [
        (
            "Mixer",
            AppCommand::MacroPanel,
            "Macros\u{2026}",
            "macro.panel",
        ),
        (
            "Mixer",
            AppCommand::NewMacroBank,
            "New Macro Bank",
            "macro.new_bank",
        ),
        (
            "View",
            AppCommand::FindParameter,
            "Find Parameter\u{2026}",
            "view.find_parameter",
        ),
        (
            "View",
            AppCommand::FollowSelection,
            "Automation Follows Selection",
            "view.follow_selection",
        ),
        (
            "View",
            AppCommand::ShowParamLane,
            "Show Automation Lane",
            "view.show_param_lane",
        ),
    ] {
        let item = menu_item(&s, menu, command);
        assert_eq!(item.label, label);
        assert!(item.enabled, "{label}");
        assert_eq!(command.command_id(), Some((id, None)));
        assert_eq!(AppCommand::from_command(id, None), Some(command));
    }
    let find = menu_item(&s, "View", AppCommand::FindParameter);
    assert_eq!(find.shortcut.as_deref(), Some("Ctrl+Shift+P"));
    assert_eq!(
        menu_item(&s, "Mixer", AppCommand::MacroPanel).shortcut,
        None
    );
    assert_eq!(
        menu_item(&s, "View", AppCommand::FollowSelection).checked,
        Some(false)
    );
    // The key opens the search.
    press(
        &mut s,
        Key::Char('p'),
        Modifiers {
            shift: true,
            ..CTRL
        },
    );
    assert!(s.param_search_open());
}

#[test]
fn the_follow_item_is_a_switch_that_the_workflow_keeps() {
    let mut s = shell();
    assert!(!s.core().follow_selection());
    s.execute(AppCommand::FollowSelection);
    assert!(s.core().follow_selection());
    assert!(s.workflow().follow_selection);
    assert_eq!(
        menu_item(&s, "View", AppCommand::FollowSelection).checked,
        Some(true)
    );
    s.execute(AppCommand::FollowSelection);
    assert!(!s.core().follow_selection());
    assert!(!s.workflow().follow_selection);
}

#[test]
fn a_press_on_a_control_shows_its_lane_when_following() {
    let mut s = shell();
    add_synth(&mut s, "a", "Lead");
    s.handle(&UiEvent::Resize(Size::new(1400.0, 900.0)));
    let at = knob_at(&s, "a", CUTOFF);
    mouse(&mut s, at, PointerButton::Primary);
    assert!(
        s.core().doc().song.track(&key("a")).is_none(),
        "off: nothing added"
    );
    s.execute(AppCommand::FollowSelection);
    mouse(&mut s, at, PointerButton::Primary);
    let track = s
        .core()
        .doc()
        .song
        .track(&key("a"))
        .expect("a track for the lane");
    let lane = track.automation_lane(CUTOFF).expect("the lane");
    assert!(track.lanes[lane].clips.is_empty(), "no data written");
    assert_eq!(
        s.core().arrangement().followed_lane(),
        Some(&(key("a"), CUTOFF.to_owned()))
    );
}

#[test]
fn show_automation_lane_works_without_the_switch() {
    let mut s = shell();
    add_synth(&mut s, "a", "Lead");
    s.handle(&UiEvent::Resize(Size::new(1400.0, 900.0)));
    // Nothing selected or touched: it says what to do.
    s.execute(AppCommand::ShowParamLane);
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("Select a device"))
    );
    let at = knob_at(&s, "a", CUTOFF);
    mouse(&mut s, at, PointerButton::Primary);
    s.core_mut().select(Some(key("a")));
    s.execute(AppCommand::ShowParamLane);
    assert!(
        s.core()
            .doc()
            .song
            .track(&key("a"))
            .is_some_and(|t| t.automation_lane(CUTOFF).is_some())
    );
}

// ---- the control menu --------------------------------------------------------------------------

#[test]
fn the_control_menu_assigns_a_param_to_a_macro_and_takes_it_out_again() {
    let mut s = shell();
    add_synth(&mut s, "a", "Lead");
    s.handle(&UiEvent::Resize(Size::new(1400.0, 900.0)));
    let at = knob_at(&s, "a", CUTOFF);
    mouse(&mut s, at, PointerButton::Secondary);
    {
        let menu = s.context_menu().expect("a menu");
        assert!(menu.submenu_center("Assign to Macro").is_some());
        assert!(menu.item_center("macro.remove_control").is_some());
    }
    // Remove is greyed while nothing drives the param: clicking it does nothing.
    let remove = s
        .context_menu()
        .unwrap()
        .item_center("macro.remove_control")
        .unwrap();
    mouse(&mut s, remove, PointerButton::Primary);
    assert!(s.core().doc().macros.assigns.is_empty());
    // Open the submenu by hovering and choose Macro 3: a bank is made, in one step.
    let sub = s
        .context_menu()
        .unwrap()
        .submenu_center("Assign to Macro")
        .unwrap();
    s.handle(&UiEvent::PointerMove {
        pos: sub,
        modifiers: Modifiers::NONE,
    });
    let item = s
        .context_menu()
        .unwrap()
        .item_center_with("macro.assign_control", "3")
        .expect("the submenu is open");
    mouse(&mut s, item, PointerButton::Primary);
    assert!(s.context_menu().is_none());
    let doc = s.core().doc();
    assert_eq!(doc.banks().count(), 1);
    let a = &doc.macros.assigns[0];
    assert_eq!(
        (a.index, a.device.0.as_str(), a.param.as_str()),
        (2, "a", CUTOFF)
    );
    // The control is locked now, and its menu offers Remove.
    mouse(&mut s, at, PointerButton::Primary);
    let before = s.core().doc().clone();
    let mut moved = at;
    moved.y -= 40.0;
    s.handle(&UiEvent::PointerMove {
        pos: moved,
        modifiers: Modifiers::NONE,
    });
    s.handle(&UiEvent::PointerUp {
        pos: moved,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
    assert_eq!(s.core().doc(), &before, "a locked control does not move");
    mouse(&mut s, at, PointerButton::Secondary);
    let remove = s
        .context_menu()
        .unwrap()
        .item_center("macro.remove_control")
        .unwrap();
    mouse(&mut s, remove, PointerButton::Primary);
    assert!(s.core().doc().macros.assigns.is_empty());
    // One undo puts it back.
    assert!(s.core_mut().undo());
    assert_eq!(s.core().doc().macros.assigns.len(), 1);
}

#[test]
fn a_second_param_goes_to_the_current_bank_or_to_a_new_one() {
    let mut s = shell();
    add_synth(&mut s, "a", "Lead");
    s.handle(&UiEvent::Resize(Size::new(1400.0, 900.0)));
    s.control_target = Some((key("a"), CUTOFF.into()));
    s.run_command("macro.assign_control", Some("1"));
    s.control_target = Some((key("a"), "filter.resonance".into()));
    s.run_command("macro.assign_control", Some("2"));
    assert_eq!(s.core().doc().banks().count(), 1);
    assert_eq!(s.core().doc().macros.assigns.len(), 2);
    s.control_target = Some((key("channel"), "fader".into()));
    s.run_command("macro.assign_control_new", Some("1"));
    assert_eq!(s.core().doc().banks().count(), 2);
    // A number that is no macro does nothing.
    let n = s.core().doc().macros.assigns.len();
    s.run_command("macro.assign_control", Some("9"));
    s.run_command("macro.assign_control", Some("x"));
    assert_eq!(s.core().doc().macros.assigns.len(), n);
}

// ---- the Macros panel --------------------------------------------------------------------------

#[test]
fn the_macros_panel_makes_a_bank_moves_a_macro_and_names_it_without_running_keys() {
    let mut s = shell();
    add_synth(&mut s, "a", "Lead");
    s.execute(AppCommand::MacroPanel);
    assert!(s.macros_open());
    assert!(s.macros_view().unwrap().banks().is_empty());
    // NEW BANK.
    let new = s.macros_view().unwrap().new_bank_rect().center();
    mouse(&mut s, new, PointerButton::Primary);
    assert_eq!(s.core().doc().banks().count(), 1);
    assert_eq!(s.macros_view().unwrap().banks().len(), 1);
    // A click on the bar of macro 1 sets the value in one undo step.
    let bar = s.macros_view().unwrap().macro_center(0, "bar").unwrap();
    let right = Point::new(bar.x + 80.0, bar.y);
    mouse(&mut s, right, PointerButton::Primary);
    let bank = s.core().macro_bank().unwrap();
    assert!(s.core().doc().bank_macro_value(&bank, 0) > 0.7);
    // Naming: a click on the name edits it, the keys are the panel's (no transport starts).
    let name = s.macros_view().unwrap().macro_center(1, "name").unwrap();
    mouse(&mut s, name, PointerButton::Primary);
    assert!(s.macros_view().unwrap().is_editing());
    for _ in 0..10 {
        press(&mut s, Key::Backspace, Modifiers::NONE);
    }
    type_text(&mut s, "Tone 1");
    press(&mut s, Key::Enter, Modifiers::NONE);
    assert!(!s.core().is_playing(), "Space inside the name did not play");
    assert_eq!(s.core().doc().macro_name(&bank, 1), "Tone 1");
    assert!(s.macros_open());
    // Esc closes the panel.
    press(&mut s, Key::Escape, Modifiers::NONE);
    assert!(!s.macros_open());
}

#[test]
fn the_panel_edits_a_range_and_learn_gives_way_to_the_controller() {
    let mut s = shell();
    add_synth(&mut s, "a", "Lead");
    s.control_target = Some((key("a"), CUTOFF.into()));
    s.run_command("macro.assign_control", Some("1"));
    s.execute(AppCommand::MacroPanel);
    let max = s.macros_view().unwrap().assign_center(0, 0, "max").unwrap();
    let before = s.core().doc().macros.assigns[0].max;
    mouse(&mut s, max, PointerButton::Secondary);
    assert!(
        s.core().doc().macros.assigns[0].max < before,
        "a right click goes down"
    );
    let inv = s
        .macros_view()
        .unwrap()
        .assign_center(0, 0, "invert")
        .unwrap();
    mouse(&mut s, inv, PointerButton::Primary);
    assert!(s.core().doc().macros.assigns[0].invert);
    assert!(s.core_mut().undo());
    assert!(!s.core().doc().macros.assigns[0].invert);
    // LEARN arms the macro and closes the panel.
    s.frame();
    let learn = s.macros_view().unwrap().macro_center(0, "learn").unwrap();
    mouse(&mut s, learn, PointerButton::Primary);
    assert!(!s.macros_open());
    let bank = s.core().macro_bank().unwrap();
    assert_eq!(
        s.core().midi_armed(),
        Some(&atomik_model::MidiTarget::Param {
            device: bank,
            param: "macro1".into()
        })
    );
}

#[test]
fn a_bank_made_from_the_menu_is_a_device_of_the_rack() {
    let mut s = shell();
    s.execute(AppCommand::NewMacroBank);
    let doc = s.core().doc();
    let bank = doc.devices.last().unwrap();
    assert_eq!(bank.type_id, MACRO_BANK_TYPE);
    assert_eq!(s.core().macro_bank(), Some(bank.key.clone()));
    assert!(s.core_mut().undo());
    assert_eq!(s.core().doc().banks().count(), 0);
}

// ---- the search --------------------------------------------------------------------------------

#[test]
fn the_search_filters_as_you_type_and_enter_reveals_the_param() {
    let mut s = shell();
    add_synth(&mut s, "a", "Lead");
    add_synth(&mut s, "b", "Pad");
    s.execute(AppCommand::FindParameter);
    assert!(s.param_search_open());
    let all = s.param_search_view().unwrap().rows().len();
    assert!(all > 20);
    type_text(&mut s, "pad cutoff");
    let rows = s.param_search_view().unwrap().rows().to_vec();
    assert!(rows.len() < all);
    assert_eq!(
        (rows[0].device.as_str(), rows[0].param.as_str()),
        ("Pad", "Cutoff")
    );
    assert_eq!(s.param_search_view().unwrap().query(), "pad cutoff");
    // The keys are the panel's: Space typed no transport, a digit no command.
    assert!(!s.core().is_playing());
    // Enter reveals: the device is selected in the rack and the panel closes.
    press(&mut s, Key::Enter, Modifiers::NONE);
    assert!(!s.param_search_open());
    assert_eq!(s.core().view().selected(), Some(&key("b")));
}

#[test]
fn the_search_adjusts_assigns_and_arms_the_chosen_row() {
    let mut s = shell();
    add_synth(&mut s, "a", "Lead");
    s.execute(AppCommand::FindParameter);
    type_text(&mut s, "lead cutoff");
    let before = value(&s, "a", CUTOFF);
    press(&mut s, Key::Right, CTRL);
    let up = value(&s, "a", CUTOFF);
    assert!(up > before);
    // The row shows the new value.
    assert_eq!(
        s.param_search_view().unwrap().rows()[0].value,
        atomik_ui::widget::format_value(
            s.core()
                .registry()
                .get("atomik.poly-synth")
                .unwrap()
                .descriptor
                .param_by_key(CUTOFF)
                .unwrap(),
            up
        )
    );
    press(&mut s, Key::Left, CTRL);
    assert!(value(&s, "a", CUTOFF) < up);
    // Ctrl+M assigns it to the next free macro of the current bank (a new one here).
    press(&mut s, Key::Char('m'), CTRL);
    assert_eq!(s.core().doc().macros.assigns.len(), 1);
    assert!(s.param_search_open());
    assert_eq!(
        s.param_search_view().unwrap().rows()[0]
            .macro_tag
            .as_deref(),
        Some("M1"),
        "the row says so"
    );
    // A driven param is not learned (the controller belongs on the macro): the panel stays.
    press(&mut s, Key::Char('e'), CTRL);
    assert!(s.param_search_open());
    assert!(s.core().midi_armed().is_none());
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("map the macro instead"))
    );
    // A free param is: Ctrl+E arms it for MIDI learn and closes the panel.
    for _ in 0..6 {
        press(&mut s, Key::Backspace, Modifiers::NONE);
    }
    type_text(&mut s, "resonance");
    press(&mut s, Key::Char('e'), CTRL);
    assert!(!s.param_search_open());
    assert!(s.core().midi_armed().is_some());
}

#[test]
fn the_search_shows_the_lane_of_a_param_and_closes() {
    let mut s = shell();
    add_synth(&mut s, "a", "Lead");
    s.execute(AppCommand::FindParameter);
    type_text(&mut s, "lead cutoff");
    press(&mut s, Key::Char('l'), CTRL);
    assert!(!s.param_search_open());
    assert!(
        s.core()
            .doc()
            .song
            .track(&key("a"))
            .is_some_and(|t| t.automation_lane(CUTOFF).is_some())
    );
    assert_eq!(
        s.core().arrangement().followed_lane(),
        Some(&(key("a"), CUTOFF.to_owned()))
    );
}

#[test]
fn the_search_closes_with_escape_and_from_its_button() {
    let mut s = shell();
    s.execute(AppCommand::FindParameter);
    press(&mut s, Key::Escape, Modifiers::NONE);
    assert!(!s.param_search_open());
    s.execute(AppCommand::FindParameter);
    let close = s.param_search_view().unwrap().close_rect().center();
    mouse(&mut s, close, PointerButton::Primary);
    assert!(!s.param_search_open());
}

#[test]
fn the_search_follows_the_window_size() {
    let mut s = shell();
    s.execute(AppCommand::FindParameter);
    s.handle(&UiEvent::Resize(Size::new(700.0, 500.0)));
    let p = s.param_search_view().unwrap().panel();
    assert!(p.x1 <= 700.0 && p.y1 <= 500.0, "{p:?}");
}
