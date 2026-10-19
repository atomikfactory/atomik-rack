//! The key rule (key rule): a cable into the master's `key_in` switches
//! `comp.key` on in the same undo step; removing the cable leaves it on; channels are
//! unchanged; loading a document never changes a param.

mod common;

use atomik_model::{
    Command, DeviceSnapshot, Document, SyncOp, WorkflowCommand, load_project, new_project_doc,
    save_project,
};
use common::workflow::*;
use common::*;

fn project() -> (Document, atomik_device_sdk::Registry) {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("synth")),
            add(usize::MAX, DELAY, Some("fx")),
        ],
    );
    (d, r)
}

fn key_on(d: &Document, r: &atomik_device_sdk::Registry) -> f32 {
    d.doc()
        .param_value(&key("master"), "comp.key", r)
        .expect("the master has comp.key")
}

fn stored(d: &Document) -> bool {
    d.doc()
        .device(&key("master"))
        .unwrap()
        .params
        .contains_key("comp.key")
}

#[test]
fn a_cable_into_key_in_switches_the_key_on_in_one_undo_step() {
    let (mut d, r) = project();
    assert_eq!(key_on(&d, &r), 0.0);
    let ops = one_step(&mut d, &r, connect("fx:out_l", "master:key_in"));
    assert_eq!(key_on(&d, &r), 1.0);
    assert!(d.doc().cables.contains(&cable("fx:out_l", "master:key_in")));
    assert!(ops.contains(&SyncOp::RoutingChanged), "{ops:?}");
    assert!(
        ops.contains(&SyncOp::ParamChanged {
            key: key("master"),
            param: "comp.key".into(),
            value: 1.0
        }),
        "{ops:?}"
    );
}

#[test]
fn undo_restores_both_the_cable_and_the_switch_byte_for_byte() {
    let (mut d, r) = project();
    let before = text(&d);
    d.apply(connect("fx:out_l", "master:key_in"), &r).unwrap();
    let after = text(&d);
    assert_ne!(before, after);
    d.undo(&r).unwrap();
    assert_eq!(text(&d), before);
    assert_eq!(key_on(&d, &r), 0.0);
    assert!(!stored(&d), "the default is not stored again");
    d.redo(&r).unwrap();
    assert_eq!(text(&d), after);
}

#[test]
fn removing_the_cable_leaves_the_switch_on() {
    let (mut d, r) = project();
    run(&mut d, &r, [connect("fx:out_l", "master:key_in")]);
    run(
        &mut d,
        &r,
        [Command::Disconnect {
            to: sock("master:key_in"),
        }],
    );
    assert!(d.doc().cables.iter().all(|c| c.to.socket != "key_in"));
    assert_eq!(key_on(&d, &r), 1.0, "the switch stays as the user left it");
}

#[test]
fn a_key_already_on_is_not_touched_again() {
    let (mut d, r) = project();
    run(&mut d, &r, [set_param("master", "comp.key", 1.0)]);
    let steps = text(&d);
    let ops = d.apply(connect("fx:out_l", "master:key_in"), &r).unwrap();
    assert!(
        ops.iter()
            .all(|o| !matches!(o, SyncOp::ParamChanged { .. })),
        "{ops:?}"
    );
    d.undo(&r).unwrap();
    assert_eq!(
        text(&d),
        steps,
        "undo of the cable leaves the earlier setting"
    );
    assert_eq!(key_on(&d, &r), 1.0);
}

#[test]
fn a_user_who_switched_the_key_off_again_gets_it_on_with_the_next_cable() {
    let (mut d, r) = project();
    run(&mut d, &r, [connect("fx:out_l", "master:key_in")]);
    run(
        &mut d,
        &r,
        [
            Command::Disconnect {
                to: sock("master:key_in"),
            },
            set_param("master", "comp.key", 0.0),
            connect("fx:out_r", "master:key_in"),
        ],
    );
    assert_eq!(key_on(&d, &r), 1.0);
}

#[test]
fn a_rejected_cable_does_not_switch_anything() {
    let (mut d, r) = project();
    // `key_in` is an input: an input cannot be a source; nothing changes.
    let e = rejected(&mut d, &r, connect("master:key_in", "fx:in_l"));
    assert!(matches!(e, atomik_model::ModelError::BadCable(_)), "{e:?}");
    assert_eq!(key_on(&d, &r), 0.0);
}

#[test]
fn a_cable_into_any_other_master_jack_leaves_the_key_alone() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            connect("fx:out_l", "master:return1_l"),
            connect("fx:out_r", "master:insert_return_r"),
        ],
    );
    assert_eq!(key_on(&d, &r), 0.0);
}

#[test]
fn a_cable_into_a_channel_key_in_changes_no_param() {
    let (mut d, r) = project();
    let before: Vec<_> = d.doc().devices.iter().map(|x| x.params.clone()).collect();
    run(&mut d, &r, [connect("fx:out_l", "channel:key_in")]);
    let after: Vec<_> = d.doc().devices.iter().map(|x| x.params.clone()).collect();
    assert_eq!(before, after);
    assert_eq!(key_on(&d, &r), 0.0);
}

#[test]
fn a_cable_into_the_key_of_a_master_outside_the_console_changes_nothing() {
    let r = console_registry();
    let mut d = Document::new(atomik_model::empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, MASTER, Some("stray")),
            add(1, DELAY, Some("fx")),
            connect("fx:out_l", "stray:key_in"),
        ],
    );
    assert!(d.doc().device(&key("stray")).unwrap().params.is_empty());
}

#[test]
fn the_rule_also_holds_inside_a_batch() {
    let (mut d, r) = project();
    let before = text(&d);
    run(
        &mut d,
        &r,
        [Command::Batch(vec![connect("fx:out_l", "master:key_in")])],
    );
    assert_eq!(key_on(&d, &r), 1.0);
    d.undo(&r).unwrap();
    assert_eq!(text(&d), before);
}

#[test]
fn loading_a_document_never_changes_params() {
    let (mut d, r) = project();
    // The cable is there and the key is off: a document the user made that way (patched, then
    // switched the key off again) stays as it is.
    run(
        &mut d,
        &r,
        [
            connect("fx:out_l", "master:key_in"),
            set_param("master", "comp.key", 0.0),
        ],
    );
    let text = save_project(d.doc());
    let back = load_project(&text, &r).unwrap();
    assert_eq!(back, *d.doc());
    assert_eq!(key_on(&Document::new(back), &r), 0.0);
}

#[test]
fn a_paste_never_reaches_the_master_so_the_key_stays_put() {
    let (mut d, r) = project();
    let s = DeviceSnapshot::capture(d.doc(), &keys_of(&["fx", "synth"]), false).unwrap();
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::PasteDevices {
            snapshot: s,
            container: None,
            at: 9,
        })],
    );
    assert_eq!(key_on(&d, &r), 0.0);
}
