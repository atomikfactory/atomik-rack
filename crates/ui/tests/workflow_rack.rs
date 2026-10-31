//! The rack view's part: the device browser in create mode and the edit
//! commands the shell offers to it.

mod support;

use atomik_device_sdk::DeviceKind;
use atomik_model::Command;
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

#[test]
fn create_mode_lists_only_the_kind_and_the_filter_is_focused() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.open_browser_create(CreateKind::Instrument, &reg);
    assert!(v.browser_visible() && v.is_text_owner());
    assert_eq!(v.browser_create(), Some(CreateKind::Instrument));
    let rows = v.browser_rows(&reg);
    assert!(rows.contains(&"atomik.poly-synth"));
    assert!(!rows.contains(&"atomik.stereo-delay"));
    for t in &rows {
        assert_eq!(
            reg.get(t).unwrap().descriptor.kind,
            DeviceKind::Instrument,
            "{t}"
        );
    }
    v.open_browser_create(CreateKind::Effect, &reg);
    let rows = v.browser_rows(&reg);
    assert!(rows.contains(&"atomik.stereo-delay"));
    assert!(!rows.contains(&"atomik.poly-synth"));
    for t in &rows {
        let kind = reg.get(t).unwrap().descriptor.kind;
        assert!(
            matches!(kind, DeviceKind::Effect | DeviceKind::Utility),
            "{t}"
        );
    }
    // The normal browser lists everything again.
    v.set_browser_visible(false);
    v.set_browser_visible(true);
    assert_eq!(v.browser_create(), None);
    assert!(v.browser_rows(&reg).len() > 10);
}

#[test]
fn enter_creates_the_highlighted_device_and_closes_the_browser() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.open_browser_create(CreateKind::Instrument, &reg);
    type_text(&mut v, "poly", &doc, &reg);
    assert_eq!(v.browser_rows(&reg), vec!["atomik.poly-synth"]);
    let a = v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg);
    let cmds = commands(&a);
    assert!(
        matches!(
            cmds.as_slice(),
            [Command::Workflow(atomik_model::WorkflowCommand::CreateInstrument {
                source: atomik_model::CreateSource::Type(type_id),
                ..
            })] if type_id == "atomik.poly-synth"
        ),
        "{cmds:?}"
    );
    assert!(
        !v.browser_visible(),
        "a created device closes the create browser"
    );
    assert!(!v.is_text_owner());
}

#[test]
fn the_browser_stays_open_when_the_workflow_says_so() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_create_keeps_browser_open(true);
    v.open_browser_create(CreateKind::Effect, &reg);
    type_text(&mut v, "delay", &doc, &reg);
    let a = v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg);
    assert_eq!(commands(&a).len(), 1);
    assert!(v.browser_visible() && v.browser_create() == Some(CreateKind::Effect));
}

#[test]
fn escape_cancels_the_create_browser_and_removes_nothing() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.open_browser_create(CreateKind::Instrument, &reg);
    type_text(&mut v, "pol", &doc, &reg);
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert!(a.is_empty());
    assert!(!v.browser_visible());
    assert_eq!(v.browser_create(), None);
}

#[test]
fn the_normal_browser_keeps_its_behaviour() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_browser_visible(true);
    type_text(&mut v, "delay", &doc, &reg);
    let a = v.handle(&press(Key::Enter, Modifiers::NONE), &doc, &reg);
    assert_eq!(commands(&a).len(), 1);
    assert!(v.browser_visible(), "the normal browser stays open");
}

#[test]
fn delete_and_duplicate_are_commands_on_the_selected_device() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Declined,
        "nothing selected"
    );
    v.set_selected(Some(key("synth")));
    v.set_auto_route(false);
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::RemoveDevice {
            key: key("synth")
        })])
    );
    assert_eq!(
        v.command("edit.duplicate", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
            atomik_model::WorkflowCommand::DuplicateDevices {
                route: false,
                keys: vec![key("synth")]
            }
        ))]),
        "a plain device duplicates through the workflow command"
    );
    assert_eq!(
        v.command("transport.play", None, &doc, &reg),
        CommandOutcome::Declined
    );
    // The key itself no longer deletes: it is a command now.
    let a = v.handle(
        &UiEvent::Key {
            key: Key::Delete,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        &doc,
        &reg,
    );
    assert!(commands(&a).is_empty());
}
