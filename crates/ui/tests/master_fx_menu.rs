//! "Add Send Effect" and "Add Insert Effect" of the rack's menu: the effect browser opens bound
//! to the strip, so what is chosen lands on the master's next send, the end of the master's
//! insert chain or the end of a channel's or a bus's insert chain, in one undo step.

mod support;

use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateSource, CreateTarget, DeviceKey, Document,
    OutputDest, RackDoc, WorkflowCommand, insert_chain, send_chain,
};
use atomik_ui::browser::{CreateKind, SendEntry};
use atomik_ui::keymap::{CommandOutcome, Registry as Commands};
use atomik_ui::{Key, Modifiers, RackView};
use support::song::press;
use support::starter::*;
use support::*;

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn with_bus(reg: &atomik_device_sdk::Registry) -> (Document, DeviceKey) {
    let mut d = two_instruments(reg);
    run(
        &mut d,
        reg,
        [cc(ConsoleCommand::CreateBus {
            name: "Drums".into(),
            dest: OutputDest::Master,
        })],
    );
    let bus = d.doc().console.buses[0].bus.clone();
    (d, bus)
}

fn run_command(
    v: &mut RackView,
    id: &str,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
) -> CommandOutcome {
    v.command(id, None, doc, reg)
}

fn not_applicable(outcome: CommandOutcome) -> String {
    match outcome {
        CommandOutcome::NotApplicable(m) => m,
        other => panic!("expected NotApplicable, got {other:?}"),
    }
}

/// Types `name` in the open browser and presses Enter; the commands that came out.
fn choose(
    v: &mut RackView,
    name: &str,
    modifiers: Modifiers,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
) -> Vec<Command> {
    for c in name.chars() {
        let _ = v.handle(&press(Key::Char(c), Modifiers::NONE), doc, reg);
    }
    commands(&v.handle(&press(Key::Enter, modifiers), doc, reg))
}

#[test]
fn the_commands_are_in_the_registry_with_their_labels() {
    let r = Commands::bundled();
    for (id, label) in [
        ("rack.add_send_effect", "Add Send Effect..."),
        ("rack.add_insert_effect", "Add Insert Effect..."),
        ("rack.save_master_chain", "Save Master Chain as Patch..."),
        ("rack.load_master_chain", "Load Master Chain Patch..."),
    ] {
        let c = r.get(id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(c.label, label);
        assert!(!c.menu, "{id} is a context-menu command");
    }
}

#[test]
fn add_send_effect_on_the_master_opens_the_effect_browser_bound_to_the_next_send() {
    let reg = registry();
    let d = with_effects(&reg);
    let doc = d.doc().clone();
    let mut v = view(&doc, &reg);
    v.set_selected(Some(key("master")));
    // Bound to the sends even though the master's drop row was the last click.
    assert!(matches!(
        run_command(&mut v, "rack.add_send_effect", &doc, &reg),
        CommandOutcome::Handled(_)
    ));
    assert_eq!(v.browser_create(), Some(CreateKind::Effect));
    assert_eq!(v.effect_create_target(), CreateTarget::Master);
    assert_eq!(v.browser_send_entry(&doc), Some(SendEntry::Slot(1)));
    let cmds = choose(&mut v, "atomik.reverb", Modifiers::NONE, &doc, &reg);
    assert_eq!(
        cmds,
        [Command::Workflow(WorkflowCommand::CreateEffect {
            source: CreateSource::Type("atomik.reverb".into()),
            target: CreateTarget::Master,
            key: None,
            options: CreateOptions::default(),
        })]
    );
    assert_eq!(v.browser_create(), None, "the browser closed");
    assert_eq!(
        v.effect_create_target(),
        CreateTarget::Selected(key("master"))
    );
    let mut d = d;
    d.apply(cmds[0].clone(), &reg).unwrap();
    assert!(
        d.doc().console.returns[1].is_some(),
        "send 2, the next free one"
    );
    assert_eq!(send_chain(d.doc(), &reg, 1).len(), 1);
}

#[test]
fn add_insert_effect_on_the_master_goes_to_the_end_of_its_insert_chain() {
    let reg = registry();
    let d = with_effects(&reg);
    let doc = d.doc().clone();
    let mut v = view(&doc, &reg);
    v.set_selected(Some(key("master")));
    assert!(matches!(
        run_command(&mut v, "rack.add_insert_effect", &doc, &reg),
        CommandOutcome::Handled(_)
    ));
    assert_eq!(v.effect_create_target(), CreateTarget::MasterInserts);
    assert_eq!(v.browser_send_entry(&doc), None, "no send is made");
    let cmds = choose(&mut v, "atomik.eq", Modifiers::NONE, &doc, &reg);
    let mut d = d;
    for c in cmds {
        d.apply(c, &reg).unwrap();
    }
    let m = key("master");
    assert_eq!(insert_chain(d.doc(), &reg, &m).len(), 1);
    assert_eq!(
        d.doc().console.returns.iter().flatten().count(),
        1,
        "no new send"
    );
}

#[test]
fn add_insert_effect_on_a_channel_or_a_bus_goes_to_the_end_of_its_chain() {
    let reg = registry();
    let (mut d, bus) = with_bus(&reg);
    for strip in [key("channel"), bus] {
        let doc = d.doc().clone();
        let mut v = view(&doc, &reg);
        v.set_selected(Some(strip.clone()));
        assert!(matches!(
            run_command(&mut v, "rack.add_insert_effect", &doc, &reg),
            CommandOutcome::Handled(_)
        ));
        assert_eq!(
            v.effect_create_target(),
            CreateTarget::Selected(strip.clone())
        );
        // Shift would make it a plain add; a bound effect is always routed.
        let shift = Modifiers {
            shift: true,
            ..Modifiers::NONE
        };
        let cmds = choose(&mut v, "atomik.reverb", shift, &doc, &reg);
        match &cmds[..] {
            [Command::Workflow(WorkflowCommand::CreateEffect { options, .. })] => {
                assert!(options.route, "routed whatever the setting and Shift say");
            }
            other => panic!("{other:?}"),
        }
        for c in cmds {
            d.apply(c, &reg).unwrap();
        }
        assert_eq!(insert_chain(d.doc(), &reg, &strip).len(), 1, "{strip}");
        // A second one goes after it.
        let doc = d.doc().clone();
        let mut v = view(&doc, &reg);
        v.set_selected(Some(strip.clone()));
        let _ = run_command(&mut v, "rack.add_insert_effect", &doc, &reg);
        for c in choose(&mut v, "atomik.eq", Modifiers::NONE, &doc, &reg) {
            d.apply(c, &reg).unwrap();
        }
        let types: Vec<String> = insert_chain(d.doc(), &reg, &strip)
            .iter()
            .map(|k| d.doc().device(k).unwrap().type_id.clone())
            .collect();
        assert_eq!(types, ["atomik.reverb", "atomik.eq"], "{strip}");
    }
    assert!(d.doc().console.returns.iter().all(Option::is_none));
}

#[test]
fn the_commands_say_why_not_for_other_selections() {
    let reg = registry();
    let (d, bus) = with_bus(&reg);
    let doc = d.doc().clone();
    let mut v = view(&doc, &reg);
    // Nothing selected.
    let m = not_applicable(run_command(&mut v, "rack.add_insert_effect", &doc, &reg));
    assert!(m.contains("Select"), "{m}");
    // A send needs the master.
    for strip in [key("channel"), bus.clone()] {
        v.set_selected(Some(strip));
        let m = not_applicable(run_command(&mut v, "rack.add_send_effect", &doc, &reg));
        assert!(m.contains("master section"), "{m}");
    }
    // A plain device is no mixer strip.
    v.set_selected(Some(key("synth")));
    let m = not_applicable(run_command(&mut v, "rack.add_insert_effect", &doc, &reg));
    assert!(m.contains("master section, a channel or a bus"), "{m}");
    assert_eq!(v.browser_create(), None, "no browser opened");
    assert!(v.add_effect_target(&doc, true).is_err());
    assert_eq!(
        v.add_effect_target(&doc, false),
        Err("Add Insert Effect works on the master section, a channel or a bus".to_string())
    );
    v.set_selected(Some(key("master")));
    assert_eq!(v.add_effect_target(&doc, true), Ok(CreateTarget::Master));
    assert_eq!(
        v.add_effect_target(&doc, false),
        Ok(CreateTarget::MasterInserts)
    );
}

#[test]
fn the_binding_ends_with_the_browser() {
    let reg = registry();
    let d = with_effects(&reg);
    let doc = d.doc().clone();
    let mut v = view(&doc, &reg);
    v.set_selected(Some(key("channel")));
    let _ = run_command(&mut v, "rack.add_insert_effect", &doc, &reg);
    assert_eq!(
        v.effect_create_target(),
        CreateTarget::Selected(key("channel"))
    );
    // Escape cancels; a browser opened again later is the plain create browser.
    let _ = v.handle(&press(Key::Escape, Modifiers::NONE), &doc, &reg);
    v.set_selected(Some(key("master")));
    v.open_browser_create(CreateKind::Effect, &reg);
    assert_eq!(
        v.effect_create_target(),
        CreateTarget::Selected(key("master"))
    );
    assert_eq!(v.browser_send_entry(&doc), Some(SendEntry::Slot(1)));
}
