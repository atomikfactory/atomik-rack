//! Macro banks in the views: a control a macro drives is locked (it shows the macro's value, wears
//! an `M<n>` tag and refuses a drag) but keeps its right-click menu, every press on a control says
//! which param is being touched, and the Macros panel and the parameter search lay out, hit-test
//! and emit what they promise.

mod support;

use atomik_model::{DeviceKey, MacroAssign, RackDoc};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, Key, MacroAssignRow, MacroBankRow, MacroRow, MacrosAction, MacrosView, Modifiers,
    ParamSearchAction, ParamSearchRow, ParamSearchView, PluginParamsView, PointerButton, RackView,
    Theme, UiAction, UiEvent,
};
use support::console::*;
use support::plugin::*;
use support::*;

fn assign(device: &str, param: &str, min: f32, max: f32, invert: bool) -> MacroAssign {
    MacroAssign {
        id: 1,
        bank: key("bank"),
        index: 0,
        device: key(device),
        param: param.into(),
        min,
        max,
        invert,
    }
}

/// The reference rack plus a macro bank at 0.5 that drives the cutoff of the synth over 200..2000.
fn driven_doc() -> RackDoc {
    let mut doc = reference_doc();
    let mut bank = device("bank", "atomik.macro-bank", "Macro Bank");
    bank.params.insert("macro1".into(), 0.5);
    doc.devices.push(bank);
    doc.macros
        .assigns
        .push(assign("synth", "filter.cutoff", 200.0, 2000.0, false));
    doc
}

fn texts(dl: &DisplayList) -> Vec<String> {
    dl.texts().into_iter().map(str::to_string).collect()
}

/// What the panel says about a control when the pointer rests on it.
fn hover_text(v: &mut RackView, doc: &RackDoc, device: &str, param: &str) -> Option<String> {
    let at = v.control_center(&key(device), param).unwrap();
    let _ = v.handle(&mv(at), doc, &registry());
    v.tooltip()
}

fn paint_rack(v: &mut RackView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    texts(&dl)
}

fn touched(actions: &[UiAction]) -> Vec<(DeviceKey, String)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::ParamTouched { device, param } => Some((device.clone(), param.clone())),
            _ => None,
        })
        .collect()
}

fn menus(actions: &[UiAction]) -> usize {
    actions
        .iter()
        .filter(|a| matches!(a, UiAction::ControlMenu { .. }))
        .count()
}

// ---- The rack ------------------------------------------------------------------------------------

#[test]
fn a_driven_rack_control_shows_the_macro_value_and_a_tag() {
    let doc = driven_doc();
    let mut v = plain_view(&doc, &registry());
    // 0.5 over 200..2000 is 1.10 kHz; the stored 2400 Hz is not what the panel says.
    let tip = hover_text(&mut v, &doc, "synth", "filter.cutoff").unwrap();
    assert!(tip.contains("1.10 kHz") && tip.contains("macro 1"), "{tip}");
    assert!(paint_rack(&mut v, &doc).iter().any(|t| t == "M1"));
    // A free control has no such tooltip at rest.
    assert_eq!(hover_text(&mut v, &doc, "synth", "filter.resonance"), None);
    // A rack with no assignment has no tag, and its control rests without a tooltip.
    let plain = reference_doc();
    let mut p = plain_view(&plain, &registry());
    assert!(!paint_rack(&mut p, &plain).iter().any(|t| t == "M1"));
    assert_eq!(hover_text(&mut p, &plain, "synth", "filter.cutoff"), None);
}

#[test]
fn the_value_follows_the_macro() {
    let mut doc = driven_doc();
    let mut v = plain_view(&doc, &registry());
    let tip = hover_text(&mut v, &doc, "synth", "filter.cutoff").unwrap();
    assert!(tip.contains("1.10 kHz"), "{tip}");
    doc.devices
        .iter_mut()
        .find(|d| d.key == key("bank"))
        .unwrap()
        .params
        .insert("macro1".into(), 1.0);
    v.sync(&doc, &registry());
    let tip = hover_text(&mut v, &doc, "synth", "filter.cutoff").unwrap();
    assert!(tip.contains("2.00 kHz"), "{tip}");
}

#[test]
fn a_driven_rack_control_refuses_a_press_but_keeps_its_menu() {
    let doc = driven_doc();
    let mut v = plain_view(&doc, &registry());
    let at = v.control_center(&key("synth"), "filter.cutoff").unwrap();
    let left = v.handle(&down(at), &doc, &registry());
    assert!(commands(&left).is_empty());
    assert_eq!(menus(&left), 0);
    // It is still the param being touched.
    assert_eq!(touched(&left), [(key("synth"), "filter.cutoff".to_owned())]);
    // Dragging it does nothing either.
    let drag = v.handle(&mv(Point::new(at.x, at.y - 40.0)), &doc, &registry());
    assert!(commands(&drag).is_empty());
    let _ = v.handle(&up(at), &doc, &registry());
    // A double click does not reset it.
    let twice = v.handle(&down_with(at, PointerButton::Primary, 2), &doc, &registry());
    assert!(commands(&twice).is_empty());
    // The right click opens the control menu.
    let right = v.handle(
        &down_with(at, PointerButton::Secondary, 1),
        &doc,
        &registry(),
    );
    assert_eq!(menus(&right), 1);
    assert!(commands(&right).is_empty());
    assert_eq!(
        touched(&right),
        [(key("synth"), "filter.cutoff".to_owned())]
    );
}

#[test]
fn a_free_control_next_to_a_driven_one_still_works() {
    let doc = driven_doc();
    let mut v = plain_view(&doc, &registry());
    let at = v.control_center(&key("synth"), "filter.resonance").unwrap();
    let actions = v.handle(&down_with(at, PointerButton::Primary, 2), &doc, &registry());
    assert_eq!(commands(&actions).len(), 1, "a double click resets it");
    assert_eq!(
        touched(&actions),
        [(key("synth"), "filter.resonance".to_owned())]
    );
}

#[test]
fn a_dormant_assignment_leaves_the_control_alone() {
    let mut doc = driven_doc();
    // The bank is gone: nothing drives the param and it is free again.
    doc.devices.retain(|d| d.key != key("bank"));
    let mut v = plain_view(&doc, &registry());
    assert!(!paint_rack(&mut v, &doc).iter().any(|t| t == "M1"));
    assert_eq!(hover_text(&mut v, &doc, "synth", "filter.cutoff"), None);
    let at = v.control_center(&key("synth"), "filter.cutoff").unwrap();
    let actions = v.handle(&down_with(at, PointerButton::Primary, 2), &doc, &registry());
    assert_eq!(commands(&actions).len(), 1);
}

#[test]
fn every_press_on_a_rack_control_says_which_param_is_touched() {
    let doc = reference_doc();
    let mut v = plain_view(&doc, &registry());
    for param in ["filter.cutoff", "osc2.wave"] {
        let at = v.control_center(&key("synth"), param).unwrap();
        let actions = v.handle(&down(at), &doc, &registry());
        assert_eq!(
            touched(&actions),
            [(key("synth"), param.to_owned())],
            "{param}"
        );
        let _ = v.handle(&up(at), &doc, &registry());
    }
}

// ---- The console ---------------------------------------------------------------------------------

fn driven_console() -> (RackDoc, atomik_device_sdk::Registry) {
    let mut doc = console_doc();
    let mut bank = device("bank", "atomik.macro-bank", "Macro Bank");
    bank.params.insert("macro1".into(), 0.25);
    doc.devices.push(bank);
    doc.macros
        .assigns
        .push(assign("ch-bass", "fader", 0.2, 0.8, false));
    let mut reg = console_registry();
    reg.register(atomik_devices::macro_bank::factory()).unwrap();
    (doc, reg)
}

#[test]
fn a_driven_console_control_is_locked_and_keeps_its_menu() {
    let (doc, reg) = driven_console();
    let mut v = console_view(&doc, &reg);
    let at = v.control_center(&key("ch-bass"), "fader").unwrap();
    let left = v.handle(&down(at), &doc, &reg);
    assert!(commands(&left).is_empty());
    assert_eq!(touched(&left), [(key("ch-bass"), "fader".to_owned())]);
    assert!(
        v.notice().is_some_and(|n| n.contains("macro 1")),
        "{:?}",
        v.notice()
    );
    let _ = v.handle(&mv(Point::new(at.x, at.y - 30.0)), &doc, &reg);
    let _ = v.handle(&up(at), &doc, &reg);
    let right = v.handle(&down_with(at, PointerButton::Secondary, 1), &doc, &reg);
    assert_eq!(menus(&right), 1);
    // A control the macro does not drive is free.
    let pan = v.control_center(&key("ch-bass"), "pan").unwrap();
    let actions = v.handle(&down_with(pan, PointerButton::Primary, 2), &doc, &reg);
    assert_eq!(commands(&actions).len(), 1);
}

#[test]
fn a_driven_console_control_wears_the_tag() {
    let (doc, reg) = driven_console();
    let mut v = console_view(&doc, &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(texts(&dl).iter().any(|t| t == "M1"), "{:?}", texts(&dl));
    let plain = console_doc();
    let mut p = console_view(&plain, &console_registry());
    let mut dl = DisplayList::new();
    p.paint(&mut dl, &plain, &console_registry());
    assert!(!texts(&dl).iter().any(|t| t == "M1"));
}

// ---- The Macros panel ----------------------------------------------------------------------------

fn assign_row(id: u32, target: &str, min: f32, max: f32, invert: bool) -> MacroAssignRow {
    MacroAssignRow {
        id,
        device: key("synth"),
        param: "filter.cutoff".into(),
        target: target.into(),
        min,
        max,
        invert,
        param_min: 20.0,
        param_max: 20_000.0,
        step: 400.0,
        min_text: format!("{min:.0} Hz"),
        max_text: format!("{max:.0} Hz"),
        missing: false,
    }
}

fn macro_row(index: u8, name: &str, value: f32, assigns: Vec<MacroAssignRow>) -> MacroRow {
    MacroRow {
        index,
        name: name.into(),
        value,
        mapped: false,
        assigns,
    }
}

fn bank_row(k: &str, name: &str, with: bool) -> MacroBankRow {
    MacroBankRow {
        key: key(k),
        name: name.into(),
        macros: (0..8)
            .map(|i| {
                let assigns = if with && i == 0 {
                    vec![
                        assign_row(1, "Lead Cutoff", 200.0, 2000.0, false),
                        assign_row(2, "Pad Cutoff", 400.0, 4000.0, true),
                    ]
                } else {
                    Vec::new()
                };
                macro_row(i, &format!("Macro {}", i + 1), 0.5, assigns)
            })
            .collect(),
    }
}

fn macros_view(banks: Vec<MacroBankRow>) -> MacrosView {
    let mut v = MacrosView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 1100.0, 800.0));
    v.set_banks(banks, None);
    v
}

fn macros_of(actions: Vec<UiAction>) -> Vec<MacrosAction> {
    actions
        .into_iter()
        .filter_map(|a| match a {
            UiAction::Macros(m) => Some(m),
            _ => None,
        })
        .collect()
}

fn click_at(v: &mut MacrosView, p: Point, button: PointerButton) -> Vec<MacrosAction> {
    macros_of(v.handle(&UiEvent::PointerDown {
        pos: p,
        button,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }))
}

fn key_event(v: &mut MacrosView, key: Key, modifiers: Modifiers) -> Vec<MacrosAction> {
    macros_of(v.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    }))
}

#[test]
fn the_macros_panel_lists_the_macros_and_their_assignments() {
    let v = macros_view(vec![bank_row("bank", "Macro Bank", true)]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    let shown = texts(&dl);
    for want in [
        "MACROS",
        "Macro Bank",
        "M1",
        "M8",
        "Lead Cutoff",
        "Pad Cutoff",
        "MIN 200 Hz",
        "MAX 2000 Hz",
    ] {
        assert!(shown.iter().any(|t| t == want), "{want}: {shown:?}");
    }
    assert_eq!(v.bank().unwrap().key, key("bank"));
    // Nothing yet: the panel says how to start.
    let empty = macros_view(Vec::new());
    let mut dl = DisplayList::new();
    empty.paint(&mut dl);
    assert!(texts(&dl).iter().any(|t| t.contains("No macro bank")));
}

#[test]
fn a_click_on_min_and_max_moves_the_range_by_a_step() {
    let mut v = macros_view(vec![bank_row("bank", "Macro Bank", true)]);
    let min = v.assign_center(0, 0, "min").unwrap();
    assert_eq!(
        click_at(&mut v, min, PointerButton::Primary),
        [MacrosAction::SetRange {
            id: 1,
            min: 600.0,
            max: 2000.0,
            invert: false
        }]
    );
    assert_eq!(
        click_at(&mut v, min, PointerButton::Secondary),
        [MacrosAction::SetRange {
            id: 1,
            min: 20.0,
            max: 2000.0,
            invert: false
        }]
    );
    let max = v.assign_center(0, 1, "max").unwrap();
    assert_eq!(
        click_at(&mut v, max, PointerButton::Primary),
        [MacrosAction::SetRange {
            id: 2,
            min: 400.0,
            max: 4400.0,
            invert: true
        }]
    );
    // The wheel moves it too, Shift a tenth of a step.
    let wheel = |v: &mut MacrosView, dy: f64, shift: bool, p: Point| {
        macros_of(v.handle(&UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, dy),
            pos: p,
            modifiers: Modifiers {
                shift,
                ..Modifiers::NONE
            },
        }))
    };
    assert_eq!(
        wheel(&mut v, 1.0, false, max),
        [MacrosAction::SetRange {
            id: 2,
            min: 400.0,
            max: 4400.0,
            invert: true
        }]
    );
    assert_eq!(
        wheel(&mut v, -1.0, true, max),
        [MacrosAction::SetRange {
            id: 2,
            min: 400.0,
            max: 3960.0,
            invert: true
        }]
    );
}

#[test]
fn a_range_stays_inside_the_param() {
    let mut banks = vec![bank_row("bank", "Macro Bank", true)];
    banks[0].macros[0].assigns[0] = assign_row(1, "Lead Cutoff", 20.0, 20_000.0, false);
    let mut v = macros_view(banks);
    // At the ends a click changes nothing.
    let max = v.assign_center(0, 0, "max").unwrap();
    assert!(click_at(&mut v, max, PointerButton::Primary).is_empty());
    let min = v.assign_center(0, 0, "min").unwrap();
    assert!(click_at(&mut v, min, PointerButton::Secondary).is_empty());
}

#[test]
fn invert_remove_reveal_and_clear_emit_their_actions() {
    let mut v = macros_view(vec![bank_row("bank", "Macro Bank", true)]);
    let p = v.assign_center(0, 0, "invert").unwrap();
    assert_eq!(
        click_at(&mut v, p, PointerButton::Primary),
        [MacrosAction::SetRange {
            id: 1,
            min: 200.0,
            max: 2000.0,
            invert: true
        }]
    );
    let p = v.assign_center(0, 1, "invert").unwrap();
    assert_eq!(
        click_at(&mut v, p, PointerButton::Primary),
        [MacrosAction::SetRange {
            id: 2,
            min: 400.0,
            max: 4000.0,
            invert: false
        }]
    );
    let p = v.assign_center(0, 1, "remove").unwrap();
    assert_eq!(
        click_at(&mut v, p, PointerButton::Primary),
        [MacrosAction::Remove(2)]
    );
    let p = v.assign_center(0, 0, "target").unwrap();
    assert_eq!(
        click_at(&mut v, p, PointerButton::Primary),
        [MacrosAction::Reveal {
            device: key("synth"),
            param: "filter.cutoff".into()
        }]
    );
    let p = v.macro_center(0, "clear").unwrap();
    assert_eq!(
        click_at(&mut v, p, PointerButton::Primary),
        [MacrosAction::Clear {
            bank: key("bank"),
            index: 0
        }]
    );
    let p = v.macro_center(3, "learn").unwrap();
    assert_eq!(
        click_at(&mut v, p, PointerButton::Primary),
        [MacrosAction::Learn {
            bank: key("bank"),
            index: 3
        }]
    );
}

#[test]
fn the_bar_sets_the_macro_and_the_wheel_nudges_it() {
    let mut v = macros_view(vec![bank_row("bank", "Macro Bank", true)]);
    let bar = v.macro_center(0, "bar").unwrap();
    let first = click_at(&mut v, bar, PointerButton::Primary);
    let [MacrosAction::SetValue { bank, index, value }] = first.as_slice() else {
        panic!("one value expected");
    };
    assert_eq!((bank.clone(), *index), (key("bank"), 0));
    assert!((value - 0.5).abs() < 0.02, "{value}");
    let left = Point::new(bar.x - 100.0, bar.y);
    let second = click_at(&mut v, left, PointerButton::Primary);
    let [MacrosAction::SetValue { value, .. }] = second.as_slice() else {
        panic!("one value expected");
    };
    assert!(*value < 0.05, "{value}");
    let moved = macros_of(v.handle(&UiEvent::Wheel {
        delta: atomik_ui::kurbo::Vec2::new(0.0, 1.0),
        pos: bar,
        modifiers: Modifiers::NONE,
    }));
    assert_eq!(
        moved,
        [MacrosAction::SetValue {
            bank: key("bank"),
            index: 0,
            value: 0.52
        }]
    );
}

#[test]
fn a_macro_is_named_in_place() {
    let mut v = macros_view(vec![bank_row("bank", "Macro Bank", false)]);
    let at = v.macro_center(1, "name").unwrap();
    assert!(click_at(&mut v, at, PointerButton::Primary).is_empty());
    assert!(v.is_editing());
    // The old name is the start of the text: clear it and type a new one.
    for _ in 0..10 {
        key_event(&mut v, Key::Backspace, Modifiers::NONE);
    }
    for c in "Drive".chars() {
        key_event(&mut v, Key::Char(c), Modifiers::NONE);
    }
    key_event(&mut v, Key::Space, Modifiers::NONE);
    key_event(&mut v, Key::Char('2'), Modifiers::NONE);
    assert_eq!(
        key_event(&mut v, Key::Enter, Modifiers::NONE),
        [MacrosAction::Rename {
            bank: key("bank"),
            index: 1,
            name: "Drive 2".into()
        }]
    );
    assert!(!v.is_editing());
    // Esc drops the edit, and a name left as it was is no action.
    click_at(&mut v, at, PointerButton::Primary);
    key_event(&mut v, Key::Char('x'), Modifiers::NONE);
    assert!(key_event(&mut v, Key::Escape, Modifiers::NONE).is_empty());
    assert!(!v.is_editing());
    click_at(&mut v, at, PointerButton::Primary);
    assert!(key_event(&mut v, Key::Enter, Modifiers::NONE).is_empty());
    // A name has a limit.
    click_at(&mut v, at, PointerButton::Primary);
    for _ in 0..40 {
        key_event(&mut v, Key::Char('w'), Modifiers::NONE);
    }
    let done = key_event(&mut v, Key::Enter, Modifiers::NONE);
    let [MacrosAction::Rename { name, .. }] = done.as_slice() else {
        panic!("a rename expected");
    };
    assert_eq!(name.chars().count(), atomik_model::MAX_MACRO_NAME_CHARS);
}

#[test]
fn a_click_elsewhere_keeps_the_name_being_edited() {
    let mut v = macros_view(vec![bank_row("bank", "Macro Bank", false)]);
    let at = v.macro_center(1, "name").unwrap();
    click_at(&mut v, at, PointerButton::Primary);
    key_event(&mut v, Key::Char('!'), Modifiers::NONE);
    let elsewhere = v.macro_center(4, "bar").unwrap();
    let actions = click_at(&mut v, elsewhere, PointerButton::Primary);
    assert!(matches!(
        actions.first(),
        Some(MacrosAction::Rename { index: 1, .. })
    ));
    assert!(matches!(
        actions.get(1),
        Some(MacrosAction::SetValue { index: 4, .. })
    ));
}

#[test]
fn banks_are_chosen_made_and_closed_from_the_header() {
    let mut v = macros_view(vec![
        bank_row("bank", "Macro Bank", true),
        bank_row("bank-2", "Macro Bank 2", false),
    ]);
    let chip = v.bank_rect(1).center();
    assert_eq!(
        click_at(&mut v, chip, PointerButton::Primary),
        [MacrosAction::SelectBank(key("bank-2"))]
    );
    // The app answers with the same banks and the new choice: the panel follows it.
    v.set_banks(v.banks().to_vec(), Some(&key("bank-2")));
    assert_eq!(v.bank().unwrap().key, key("bank-2"));
    assert!(
        v.assign_center(0, 0, "min").is_none(),
        "bank 2 has no assignments"
    );
    let new = v.new_bank_rect().center();
    assert_eq!(
        click_at(&mut v, new, PointerButton::Primary),
        [MacrosAction::NewBank]
    );
    let close = v.close_rect().center();
    assert_eq!(
        click_at(&mut v, close, PointerButton::Primary),
        [MacrosAction::Close]
    );
    assert_eq!(
        key_event(&mut v, Key::Escape, Modifiers::NONE),
        [MacrosAction::Close]
    );
}

#[test]
fn a_long_list_scrolls_and_only_paints_what_is_in_view() {
    let mut banks = vec![bank_row("bank", "Macro Bank", true)];
    banks[0].macros[0].assigns = (0..40)
        .map(|i| assign_row(i + 1, &format!("Target {i}"), 200.0, 2000.0, false))
        .collect();
    let mut v = macros_view(banks);
    v.set_bounds(Rect::new(0.0, 0.0, 1100.0, 400.0));
    assert!(v.assign_center(0, 39, "min").is_none(), "scrolled out");
    let wheel = |v: &mut MacrosView| {
        v.handle(&UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, -5.0),
            pos: Point::new(550.0, 250.0),
            modifiers: Modifiers::NONE,
        })
    };
    for _ in 0..20 {
        wheel(&mut v);
    }
    assert!(v.scroll() > 0.0);
    assert!(v.assign_center(0, 39, "min").is_some(), "scrolled in");
    assert!(v.assign_center(0, 0, "min").is_none());
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    let shown = texts(&dl);
    assert!(shown.iter().any(|t| t == "Target 39"));
    assert!(!shown.iter().any(|t| t == "Target 0"));
}

#[test]
fn an_assignment_whose_target_is_gone_says_so() {
    let mut banks = vec![bank_row("bank", "Macro Bank", true)];
    banks[0].macros[0].assigns[0].missing = true;
    let v = macros_view(banks);
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    assert!(texts(&dl).iter().any(|t| t == "Lead Cutoff (missing)"));
}

// ---- The parameter search ------------------------------------------------------------------------

fn search_row(track: &str, device: &str, param: &str, value: &str) -> ParamSearchRow {
    ParamSearchRow {
        track: track.into(),
        device: device.into(),
        param: param.into(),
        value: value.into(),
        macro_tag: None,
        midi: false,
        lane: false,
        can_lane: true,
        can_macro: true,
        can_adjust: true,
    }
}

fn search_view(n: usize) -> ParamSearchView {
    let mut v = ParamSearchView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 1100.0, 700.0));
    let rows: Vec<_> = (0..n)
        .map(|i| {
            search_row(
                "Lead",
                "Poly Synth",
                &format!("Param {i}"),
                &format!("{i} Hz"),
            )
        })
        .collect();
    v.set_rows(rows, n);
    v
}

fn search_of(actions: Vec<UiAction>) -> Vec<ParamSearchAction> {
    actions
        .into_iter()
        .filter_map(|a| match a {
            UiAction::ParamSearch(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn press(v: &mut ParamSearchView, key: Key, modifiers: Modifiers) -> Vec<ParamSearchAction> {
    search_of(v.handle(&UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    }))
}

const CTRL: Modifiers = Modifiers {
    shift: false,
    ctrl: true,
    alt: false,
    meta: false,
};

#[test]
fn typing_edits_the_query_and_asks_for_new_rows() {
    let mut v = search_view(5);
    assert_eq!(
        press(&mut v, Key::Char('l'), Modifiers::NONE),
        [ParamSearchAction::Query("l".into())]
    );
    press(&mut v, Key::Char('e'), Modifiers::NONE);
    press(&mut v, Key::Space, Modifiers::NONE);
    assert_eq!(
        press(&mut v, Key::Char('>'), Modifiers::NONE),
        [ParamSearchAction::Query("le >".into())]
    );
    assert_eq!(
        press(&mut v, Key::Backspace, Modifiers::NONE),
        [ParamSearchAction::Query("le ".into())]
    );
    assert_eq!(v.query(), "le ");
    assert_eq!(
        press(&mut v, Key::Delete, Modifiers::NONE),
        [ParamSearchAction::Query(String::new())]
    );
    assert!(
        press(&mut v, Key::Backspace, Modifiers::NONE).is_empty(),
        "nothing to delete"
    );
}

#[test]
fn the_arrows_move_the_choice_and_enter_reveals_it() {
    let mut v = search_view(60);
    assert_eq!(v.selected(), Some(0));
    press(&mut v, Key::Down, Modifiers::NONE);
    press(&mut v, Key::Down, Modifiers::NONE);
    assert_eq!(v.selected(), Some(2));
    press(&mut v, Key::Up, Modifiers::NONE);
    assert_eq!(v.selected(), Some(1));
    press(&mut v, Key::End, Modifiers::NONE);
    assert_eq!(v.selected(), Some(59));
    assert!(v.row_rect(59).is_some(), "the choice scrolled into view");
    press(&mut v, Key::Home, Modifiers::NONE);
    assert_eq!(v.selected(), Some(0));
    press(&mut v, Key::PageDown, Modifiers::NONE);
    assert!(v.selected().unwrap() > 5);
    press(&mut v, Key::Down, Modifiers::NONE);
    let at = v.selected().unwrap();
    assert_eq!(
        press(&mut v, Key::Enter, Modifiers::NONE),
        [ParamSearchAction::Reveal(at)]
    );
    assert_eq!(
        press(&mut v, Key::Escape, Modifiers::NONE),
        [ParamSearchAction::Close]
    );
}

#[test]
fn a_new_query_starts_at_the_first_row() {
    let mut v = search_view(30);
    press(&mut v, Key::End, Modifiers::NONE);
    press(&mut v, Key::Char('p'), Modifiers::NONE);
    assert_eq!(v.selected(), Some(0));
}

#[test]
fn the_chords_run_the_actions_of_the_chosen_row() {
    let mut v = search_view(4);
    press(&mut v, Key::Down, Modifiers::NONE);
    assert_eq!(
        press(&mut v, Key::Char('l'), CTRL),
        [ParamSearchAction::Lane(1)]
    );
    assert_eq!(
        press(&mut v, Key::Char('m'), CTRL),
        [ParamSearchAction::Assign(1)]
    );
    assert_eq!(
        press(&mut v, Key::Char('e'), CTRL),
        [ParamSearchAction::Learn(1)]
    );
    assert_eq!(
        press(&mut v, Key::Right, CTRL),
        [ParamSearchAction::Adjust {
            row: 1,
            steps: 1,
            fine: false
        }]
    );
    assert_eq!(
        press(
            &mut v,
            Key::Left,
            Modifiers {
                shift: true,
                ..CTRL
            }
        ),
        [ParamSearchAction::Adjust {
            row: 1,
            steps: -1,
            fine: true
        }]
    );
    // A chord is not typed into the field, and nothing is chosen in an empty list.
    assert_eq!(v.query(), "");
    let mut empty = search_view(0);
    assert!(press(&mut empty, Key::Enter, Modifiers::NONE).is_empty());
    assert!(press(&mut empty, Key::Char('l'), CTRL).is_empty());
    assert_eq!(empty.selected(), None);
}

#[test]
fn the_mouse_chooses_reveals_adjusts_and_uses_the_buttons() {
    let mut v = search_view(6);
    let click = |v: &mut ParamSearchView, p: Point, clicks: u8| {
        search_of(v.handle(&UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks,
        }))
    };
    let at = v.row_center(3, false).unwrap();
    assert!(click(&mut v, at, 1).is_empty());
    assert_eq!(v.selected(), Some(3));
    assert_eq!(click(&mut v, at, 2), [ParamSearchAction::Reveal(3)]);
    let value = v.row_center(2, true).unwrap();
    let wheel = search_of(v.handle(&UiEvent::Wheel {
        delta: atomik_ui::kurbo::Vec2::new(0.0, 1.0),
        pos: value,
        modifiers: Modifiers::SHIFT,
    }));
    assert_eq!(
        wheel,
        [ParamSearchAction::Adjust {
            row: 2,
            steps: 1,
            fine: true
        }]
    );
    assert_eq!(v.selected(), Some(2));
    for (what, want) in [
        ("reveal", ParamSearchAction::Reveal(2)),
        ("lane", ParamSearchAction::Lane(2)),
        ("macro", ParamSearchAction::Assign(2)),
        ("learn", ParamSearchAction::Learn(2)),
    ] {
        let p = v.button_center(what).unwrap();
        assert_eq!(click(&mut v, p, 1), [want], "{what}");
    }
    let close = search_of(v.handle(&UiEvent::PointerDown {
        pos: v.close_rect().center(),
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }));
    assert_eq!(close, [ParamSearchAction::Close]);
}

#[test]
fn the_rows_read_track_device_param_with_their_value_and_marks() {
    let mut v = search_view(0);
    let mut driven = search_row("Lead", "Poly Synth", "Cutoff", "1.10 kHz");
    driven.macro_tag = Some("M2".into());
    driven.midi = true;
    driven.lane = true;
    driven.can_adjust = false;
    v.set_rows(
        vec![driven, search_row("Master", "Master", "Fader", "-3.0 dB")],
        9,
    );
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    let shown = texts(&dl);
    for want in [
        "FIND PARAMETER",
        "Lead > Poly Synth > Cutoff",
        "Master > Master > Fader",
        "1.10 kHz",
        "-3.0 dB",
        "M2",
        "MIDI",
        "LANE",
        "2 of 9",
    ] {
        assert!(shown.iter().any(|t| t == want), "{want}: {shown:?}");
    }
    // Nothing found says so.
    let mut none = search_view(0);
    none.set_rows(Vec::new(), 0);
    let mut dl = DisplayList::new();
    none.paint(&mut dl);
    assert!(texts(&dl).iter().any(|t| t == "No parameter matches."));
}

#[test]
fn the_search_stays_inside_a_small_window() {
    let mut v = ParamSearchView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 600.0, 300.0));
    v.set_rows(
        (0..200)
            .map(|i| search_row("T", "D", &format!("P{i}"), "0"))
            .collect(),
        200,
    );
    let p = v.panel();
    assert!(
        p.x0 >= 0.0 && p.x1 <= 600.0 && p.y0 >= 0.0 && p.y1 <= 300.0,
        "{p:?}"
    );
    let visible = (0..200).filter(|i| v.row_rect(*i).is_some()).count();
    assert!(visible > 0 && visible < 20, "{visible}");
}

// ---- The parameter pane --------------------------------------------------------------------------

fn plugin_pane(doc: &RackDoc) -> PluginParamsView {
    let mut v = PluginParamsView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 560.0));
    v.open(key("synth"));
    v.sync(doc, &registry());
    v
}

fn plugin_doc_with_bank() -> RackDoc {
    let mut doc = plugin_rack().doc().clone();
    let mut bank = device("bank", "atomik.macro-bank", "Macro Bank");
    bank.params.insert("macro1".into(), 0.5);
    doc.devices.push(bank);
    doc.macros
        .assigns
        .push(assign("synth", "p10", 0.0, 1.0, false));
    doc
}

#[test]
fn a_right_click_on_a_plugin_param_opens_its_control_menu() {
    let doc = plugin_rack().doc().clone();
    let mut v = plugin_pane(&doc);
    let at = v.slider_rect(10).unwrap().center();
    let actions = v.handle(
        &down_with(at, PointerButton::Secondary, 1),
        &doc,
        &registry(),
    );
    assert_eq!(menus(&actions), 1);
    assert!(actions.iter().any(|a| matches!(a,
        UiAction::ControlMenu { device, param, .. } if *device == key("synth") && param == "p10")));
    assert_eq!(touched(&actions), [(key("synth"), "p10".to_owned())]);
    assert!(commands(&actions).is_empty());
}

#[test]
fn a_plugin_param_a_bank_drives_is_locked_and_keeps_its_menu() {
    let doc = plugin_doc_with_bank();
    let mut v = plugin_pane(&doc);
    assert_eq!(v.driver_of(10), Some(0));
    assert_eq!(v.driver_of(11), None);
    let at = v.slider_rect(10).unwrap().center();
    let mut out = v.handle(&down(at), &doc, &registry());
    out.extend(v.handle(&mv(Point::new(at.x + 40.0, at.y)), &doc, &registry()));
    out.extend(v.handle(&up(at), &doc, &registry()));
    assert!(commands(&out).is_empty(), "{out:?}");
    assert!(
        v.toast().is_some_and(|t| t.contains("macro 1")),
        "{:?}",
        v.toast()
    );
    assert_eq!(touched(&out), [(key("synth"), "p10".to_owned())]);
    let right = v.handle(
        &down_with(at, PointerButton::Secondary, 1),
        &doc,
        &registry(),
    );
    assert_eq!(menus(&right), 1);
    // The next param is free.
    let free = v.slider_rect(11).unwrap().center();
    let mut out = v.handle(&down(free), &doc, &registry());
    out.extend(v.handle(&mv(Point::new(free.x + 40.0, free.y)), &doc, &registry()));
    assert!(!commands(&out).is_empty());
}
