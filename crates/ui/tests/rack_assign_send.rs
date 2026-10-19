//! "Use as Send Effect" in the rack view and the send entry of the create browser
//! (UI half). Goldens regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test rack_assign_send`.

mod support;

use atomik_model::{Command, ConsoleCommand, Document, RackDoc, WorkflowCommand};
use atomik_ui::RackView;
use atomik_ui::browser::{CreateKind, SendEntry};
use atomik_ui::keymap::{CommandOutcome, Registry as Commands};
use support::starter::*;
use support::*;

fn assign(v: &mut RackView, doc: &RackDoc, reg: &atomik_device_sdk::Registry) -> CommandOutcome {
    v.command("rack.assign_send", None, doc, reg)
}

fn not_applicable(outcome: CommandOutcome) -> String {
    match outcome {
        CommandOutcome::NotApplicable(m) => m,
        other => panic!("expected NotApplicable, got {other:?}"),
    }
}

/// The starter with every send/return slot used by a reverb.
fn all_slots_used(reg: &atomik_device_sdk::Registry) -> Document {
    let mut d = two_instruments(reg);
    for k in 0..8 {
        let name = format!("verb{k}");
        run(
            &mut d,
            reg,
            [
                add_end("atomik.reverb", &name),
                Command::Console(ConsoleCommand::SetReturn {
                    slot: k,
                    effect: Some(key(&name)),
                }),
            ],
        );
    }
    d
}

#[test]
fn the_new_commands_are_in_the_registry() {
    let r = Commands::bundled();
    let c = r.get("rack.assign_send").expect("rack.assign_send");
    assert_eq!(c.label, "Use as Send Effect");
    assert!(!c.menu, "a context-menu command, not a menu bar item");
    let c = r
        .get("view.toggle_console_links")
        .expect("view.toggle_console_links");
    assert_eq!(c.label, "Show Console Links");
    assert!(c.menu);
    assert!(r.get("view.toggle_console_devices").unwrap().menu);
}

#[test]
fn an_unrouted_effect_goes_to_assign_send() {
    let reg = registry();
    let d = with_effects(&reg);
    let doc = d.doc();
    let mut v = view(doc, &reg);
    v.set_selected(Some(key("verb")));
    let CommandOutcome::Handled(actions) = assign(&mut v, doc, &reg) else {
        panic!("handled")
    };
    assert_eq!(
        commands(&actions),
        [Command::Workflow(WorkflowCommand::AssignSend {
            effect: key("verb")
        })]
    );
    assert_eq!(v.assign_send_reason(doc, &reg), None);
}

#[test]
fn a_rejected_plan_reads_as_not_applicable_with_the_model_message() {
    let reg = registry();
    let d = with_effects(&reg);
    let doc = d.doc();
    let mut v = view(doc, &reg);

    v.set_selected(Some(key("echo")));
    let message = not_applicable(assign(&mut v, doc, &reg));
    assert!(
        message.contains("already the effect of send/return 1"),
        "{message}"
    );
    assert_eq!(v.assign_send_reason(doc, &reg), Some(message));

    v.set_selected(Some(key("master")));
    assert!(not_applicable(assign(&mut v, doc, &reg)).contains("console"));

    v.set_selected(Some(key("synth")));
    assert!(not_applicable(assign(&mut v, doc, &reg)).contains("instrument"));
}

#[test]
fn nothing_selected_declines_and_several_devices_are_not_applicable() {
    let reg = registry();
    let d = with_effects(&reg);
    let doc = d.doc();
    let mut v = view(doc, &reg);
    assert!(matches!(
        assign(&mut v, doc, &reg),
        CommandOutcome::Declined
    ));
    assert_eq!(
        v.assign_send_reason(doc, &reg),
        Some("Nothing is selected".to_string())
    );
    let _ = v.command("edit.select_all", None, doc, &reg);
    assert!(not_applicable(assign(&mut v, doc, &reg)).contains("one effect"));
}

#[test]
fn with_all_eight_slots_used_the_command_is_not_applicable() {
    let reg = registry();
    let mut d = all_slots_used(&reg);
    run(&mut d, &reg, [add_end("atomik.reverb", "ninth")]);
    let doc = d.doc();
    let mut v = view(doc, &reg);
    v.set_selected(Some(key("ninth")));
    let message = not_applicable(assign(&mut v, doc, &reg));
    assert_eq!(message, "All 8 send and return slots are in use");
}

#[test]
fn the_send_entry_shows_the_first_free_slot_with_the_master_selected() {
    let reg = registry();
    let d = with_effects(&reg);
    let doc = d.doc();
    let mut v = view(doc, &reg);
    assert_eq!(v.browser_send_entry(doc), None, "no create mode");
    v.set_selected(Some(key("master")));
    v.open_browser_create(CreateKind::Effect, &reg);
    assert_eq!(
        v.browser_send_entry(doc),
        Some(SendEntry::Slot(1)),
        "slot 1 is used by the delay"
    );
    v.set_selected(Some(key("channel")));
    assert_eq!(
        v.browser_send_entry(doc),
        None,
        "a channel gets an insert, not a send"
    );
    v.set_selected(Some(key("master")));
    v.open_browser_create(CreateKind::Instrument, &reg);
    assert_eq!(
        v.browser_send_entry(doc),
        None,
        "instruments have no send entry"
    );
}

#[test]
fn the_send_entry_is_greyed_when_all_eight_slots_are_used() {
    let reg = registry();
    let d = all_slots_used(&reg);
    let doc = d.doc();
    let mut v = view(doc, &reg);
    v.set_selected(Some(key("master")));
    v.open_browser_create(CreateKind::Effect, &reg);
    assert_eq!(v.browser_send_entry(doc), Some(SendEntry::Full));
    assert_eq!(
        SendEntry::Full.text(),
        "All 8 send and return slots are in use"
    );
}

#[test]
fn browser_create_send_entry_matches_goldens() {
    let reg = registry();
    let d = with_effects(&reg);
    let doc = d.doc();
    let mut v = view(doc, &reg);
    v.set_selected(Some(key("master")));
    v.open_browser_create(CreateKind::Effect, &reg);
    check_golden(
        "browser_create_send_slot_1x.json",
        &paint_json(&mut v, doc, &reg),
    );

    let d = all_slots_used(&reg);
    let doc = d.doc();
    let mut v = view(doc, &reg);
    v.set_selected(Some(key("master")));
    v.open_browser_create(CreateKind::Effect, &reg);
    let full = paint_json(&mut v, doc, &reg);
    assert!(full.contains("All 8 send and return slots are in use"));
    check_golden("browser_create_send_full_1x.json", &full);
}

#[test]
fn enter_in_the_effect_browser_with_the_master_selected_reaches_the_send_rule() {
    use atomik_model::{CreateOptions, CreateSource, CreateTarget};
    use atomik_ui::{Key, Modifiers};
    use support::song::press;
    let reg = registry();
    let d = two_instruments(&reg);
    let doc = d.doc().clone();
    let mut v = view(&doc, &reg);
    v.set_selected(Some(key("master")));
    v.open_browser_create(CreateKind::Effect, &reg);
    for c in "reverb".chars() {
        let _ = v.handle(&press(Key::Char(c), Modifiers::NONE), &doc, &reg);
    }
    let out = v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg);
    let cmds = commands(&out);
    assert_eq!(
        cmds,
        [Command::Workflow(WorkflowCommand::CreateEffect {
            source: CreateSource::Type("atomik.reverb".into()),
            target: CreateTarget::Selected(key("master")),
            key: None,
            options: CreateOptions::default(),
        })]
    );
    // The model routes it to the first free slot in one undo step.
    let mut d = d;
    d.apply(cmds[0].clone(), &reg).unwrap();
    assert!(d.doc().console.returns[0].is_some());
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &doc);
}
