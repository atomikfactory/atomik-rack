//! The rack view's part: the browser creates through the
//! creation commands, honouring the Workflow setting and the Shift and Alt options, and the rack
//! commands Delete, Disconnect and Auto-route.

mod support;

use atomik_model::{Command, CreateOptions, CreateSource, CreateTarget, WorkflowCommand as W};
use atomik_ui::browser::CreateKind;
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::{Key, Modifiers, UiAction, UiEvent};
use support::song::press;
use support::*;

fn type_text(
    v: &mut atomik_ui::RackView,
    text: &str,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
) {
    for c in text.chars() {
        let _ = v.handle(&press(Key::Char(c), Modifiers::NONE), doc, reg);
    }
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Modifiers::NONE
    }
}

fn alt() -> Modifiers {
    Modifiers {
        alt: true,
        ..Modifiers::NONE
    }
}

fn create_instrument(options: CreateOptions, index: usize) -> Command {
    Command::Workflow(W::CreateInstrument {
        index,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: None,
        options,
    })
}

/// Enter in an instrument create browser filtered to the poly synth.
fn enter_instrument(
    v: &mut atomik_ui::RackView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    modifiers: Modifiers,
) -> Vec<Command> {
    v.open_browser_create(CreateKind::Instrument, reg);
    type_text(v, "poly", doc, reg);
    commands(&v.handle(&press(Key::Enter, modifiers), doc, reg))
}

#[test]
fn enter_in_the_instrument_browser_creates_through_the_chain_command() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let got = enter_instrument(&mut v, &doc, &reg, Modifiers::NONE);
    assert_eq!(
        got,
        vec![create_instrument(
            CreateOptions::default(),
            doc.devices.len()
        )]
    );
    assert!(!v.browser_visible());
}

#[test]
fn the_instrument_goes_below_the_selected_device() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_selected(Some(key("synth")));
    let got = enter_instrument(&mut v, &doc, &reg, Modifiers::NONE);
    let at = doc
        .devices
        .iter()
        .position(|d| d.key == key("synth"))
        .unwrap()
        + 1;
    assert_eq!(got, vec![create_instrument(CreateOptions::default(), at)]);
}

#[test]
fn shift_inverts_the_setting_for_one_device_and_alt_skips_the_track() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let n = doc.devices.len();
    let off = CreateOptions {
        route: false,
        ..CreateOptions::default()
    };
    assert_eq!(
        enter_instrument(&mut v, &doc, &reg, shift()),
        vec![create_instrument(off.clone(), n)],
        "Shift with auto-route on does not route"
    );
    let no_track = CreateOptions {
        skip_track: true,
        ..CreateOptions::default()
    };
    assert_eq!(
        enter_instrument(&mut v, &doc, &reg, alt()),
        vec![create_instrument(no_track, n)]
    );
    // With the setting off the plain action adds only and Shift routes.
    v.set_auto_route(false);
    assert!(!v.auto_route());
    assert_eq!(
        enter_instrument(&mut v, &doc, &reg, Modifiers::NONE),
        vec![create_instrument(off, n)]
    );
    assert_eq!(
        enter_instrument(&mut v, &doc, &reg, shift()),
        vec![create_instrument(CreateOptions::default(), n)]
    );
}

#[test]
fn an_effect_is_created_with_the_selected_device_as_the_target() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let effect = |target| {
        Command::Workflow(W::CreateEffect {
            source: CreateSource::Type("atomik.stereo-delay".into()),
            target,
            key: None,
            options: CreateOptions::default(),
        })
    };
    v.open_browser_create(CreateKind::Effect, &reg);
    type_text(&mut v, "delay", &doc, &reg);
    let got = commands(&v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg));
    assert_eq!(got, vec![effect(CreateTarget::Nothing)]);
    v.set_selected(Some(key("synth")));
    v.open_browser_create(CreateKind::Effect, &reg);
    type_text(&mut v, "delay", &doc, &reg);
    let got = commands(&v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg));
    assert_eq!(got, vec![effect(CreateTarget::Selected(key("synth")))]);
}

#[test]
fn the_normal_browser_creates_instruments_and_effects_through_the_chain_but_adds_the_rest() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_browser_visible(true);
    type_text(&mut v, "poly", &doc, &reg);
    let got = commands(&v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg));
    assert!(matches!(
        got.as_slice(),
        [Command::Workflow(W::CreateInstrument { .. })]
    ));
    assert!(v.browser_visible(), "the normal browser stays open");
    for _ in 0.."poly".len() {
        let _ = v.handle(&press(Key::Backspace, Modifiers::NONE), &doc, &reg);
    }
    type_text(&mut v, "delay", &doc, &reg);
    let got = commands(&v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg));
    assert!(matches!(
        got.as_slice(),
        [Command::Workflow(W::CreateEffect { .. })]
    ));
    for _ in 0.."delay".len() {
        let _ = v.handle(&press(Key::Backspace, Modifiers::NONE), &doc, &reg);
    }
    type_text(&mut v, "line", &doc, &reg); // the line mixer
    let got = commands(&v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg));
    assert!(
        matches!(got.as_slice(), [Command::AddDevice { type_id, .. }] if type_id == "atomik.line-mixer"),
        "{got:?}"
    );
}

#[test]
fn a_double_click_in_the_browser_creates_through_the_chain_and_honours_shift() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.open_browser_create(CreateKind::Instrument, &reg);
    type_text(&mut v, "poly", &doc, &reg);
    let row = v
        .browser_list(&reg)
        .iter()
        .position(|r| matches!(r, atomik_ui::browser::BrowserRow::Device(_)))
        .expect("the synth row");
    let pos = v.browser_row_center(row).expect("a row is shown");
    let click = |v: &mut atomik_ui::RackView, modifiers: Modifiers| {
        commands(&v.handle(
            &UiEvent::PointerDown {
                pos,
                button: atomik_ui::PointerButton::Primary,
                modifiers,
                clicks: 2,
            },
            &doc,
            &reg,
        ))
    };
    let got = click(&mut v, Modifiers::NONE);
    assert!(matches!(
        got.as_slice(),
        [Command::Workflow(W::CreateInstrument { options, .. })] if options.route
    ));
    let got = click(&mut v, shift());
    assert!(matches!(
        got.as_slice(),
        [Command::Workflow(W::CreateInstrument { options, .. })] if !options.route
    ));
}

#[test]
fn delete_bridges_when_auto_routing_is_on_and_removes_plainly_when_it_is_off() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_selection(&[key("synth"), key("delay")], None);
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            W::DeleteDevices {
                keys: vec![key("synth"), key("delay")]
            }
        ))])
    );
    v.set_selected(Some(key("delay")));
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            W::DeleteDevices {
                keys: vec![key("delay")]
            }
        ))])
    );
    v.set_auto_route(false);
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::RemoveDevice {
            key: key("delay")
        })])
    );
}

#[test]
fn disconnect_works_on_several_devices() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_selection(&[key("synth"), key("delay")], None);
    assert_eq!(
        v.command("rack.disconnect", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            W::DisconnectDevices {
                keys: vec![key("synth"), key("delay")]
            }
        ))])
    );
}

#[test]
fn the_auto_route_command_runs_on_the_selection() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    assert_eq!(
        v.command("rack.auto_route", None, &doc, &reg),
        CommandOutcome::Declined,
        "nothing selected"
    );
    v.set_selection(&[key("synth"), key("delay")], None);
    assert_eq!(
        v.command("rack.auto_route", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(W::AutoRoute {
            devices: vec![key("synth"), key("delay")],
            options: CreateOptions::default(),
        }))])
    );
}

#[test]
fn inside_a_container_the_browser_adds_plainly_and_delete_does_not_bridge() {
    use support::container::{layered, scoped_view};
    let (d, reg) = (layered(), registry());
    let mut v = scoped_view(d.doc(), &["layer"]);
    v.open_browser_create(CreateKind::Instrument, &reg);
    type_text(&mut v, "poly", d.doc(), &reg);
    let got = commands(&v.handle(&press(Key::Enter, Modifiers::NONE), d.doc(), &reg));
    assert!(
        matches!(
            got.as_slice(),
            [Command::Container(
                atomik_model::ContainerCommand::AddInner { .. }
            )]
        ),
        "{got:?}"
    );
    v.set_selected(Some(key("delay")));
    let a = v.command("edit.delete", None, d.doc(), &reg);
    assert_eq!(
        commands(a.actions()),
        vec![Command::RemoveDevice {
            key: key("layer/delay")
        }]
    );
    let a = v.command("rack.auto_route", None, d.doc(), &reg);
    assert!(matches!(a, CommandOutcome::NotApplicable(_)));
}

#[test]
fn shift_and_alt_together_add_the_device_only_without_a_track() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let n = doc.devices.len();
    let both = Modifiers {
        shift: true,
        alt: true,
        ..Modifiers::NONE
    };
    let want = CreateOptions {
        route: false,
        skip_track: true,
        disarm: Vec::new(),
    };
    assert_eq!(
        enter_instrument(&mut v, &doc, &reg, both),
        vec![create_instrument(want, n)]
    );
}

#[test]
fn a_utility_is_a_plain_add_except_in_the_effect_create_browser() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    // The normal browser: the arpeggiator is a plain add.
    v.set_browser_visible(true);
    type_text(&mut v, "arpeg", &doc, &reg);
    let got = commands(&v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg));
    assert!(
        matches!(got.as_slice(), [Command::AddDevice { type_id, .. }] if type_id == "atomik.arpeggiator"),
        "{got:?}"
    );
    // The effect create browser lists the utilities and creates them through CreateEffect.
    let mut v = view(&doc, &reg);
    v.open_browser_create(CreateKind::Effect, &reg);
    type_text(&mut v, "arpeg", &doc, &reg);
    let got = commands(&v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg));
    assert!(
        matches!(
            got.as_slice(),
            [Command::Workflow(W::CreateEffect {
                source: CreateSource::Type(t),
                ..
            })] if t == "atomik.arpeggiator"
        ),
        "{got:?}"
    );
}
