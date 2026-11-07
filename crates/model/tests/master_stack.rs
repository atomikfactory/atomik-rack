//! The master section's stack: its insert effects sit right under the master and the effects of
//! the shared sends follow them, in send order. Creating, adding, reordering, taking out and
//! moving keep that order and are each one undo step.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateSource, CreateTarget, DeviceKey, Document,
    InsertEffect, RackSlot, RouteNote, WorkflowCommand, effect_route_note, insert_chain,
    master_stack, new_project_doc, send_chain, send_slot_of,
};
use common::workflow::*;
use common::*;

const REVERB: &str = "atomik.reverb";
const EQ: &str = "atomik.eq";
const DELAY: &str = "atomik.distortion";

fn typ(t: &str) -> CreateSource {
    CreateSource::Type(t.into())
}

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn rack() -> (Document, Registry, DeviceKey) {
    let r = console_registry();
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: typ(SYNTH),
            key: Some(key("synth")),
            options: CreateOptions::default(),
        })],
    );
    let master = d.doc().console.master.clone().expect("a master");
    (d, r, master)
}

fn create(target: CreateTarget, t: &str) -> Command {
    wf(WorkflowCommand::CreateEffect {
        source: typ(t),
        target,
        key: None,
        options: CreateOptions::default(),
    })
}

fn add_insert(master: &DeviceKey, slot: usize, t: &str) -> Command {
    cc(ConsoleCommand::AddInsert {
        channel: master.clone(),
        slot,
        effect: InsertEffect::New { type_id: t.into() },
    })
}

fn order(d: &Document) -> Vec<DeviceKey> {
    d.doc().devices.iter().map(|x| x.key.clone()).collect()
}

fn inserts(d: &Document, r: &Registry, m: &DeviceKey) -> Vec<DeviceKey> {
    insert_chain(d.doc(), r, m)
}

/// The master is followed by its inserts and then the effects of the sends in use, in order.
fn assert_master_stacked(d: &Document, r: &Registry, master: &DeviceKey) {
    let all = order(d);
    let at = all.iter().position(|k| k == master).expect("the master");
    let mut expected = inserts(d, r, master);
    for slot in 0..8 {
        expected.extend(send_chain(d.doc(), r, slot));
    }
    assert_eq!(
        &all[at + 1..at + 1 + expected.len()],
        &expected[..],
        "inserts, then the send effects, right under the master"
    );
    assert_eq!(master_stack(d.doc(), r), expected);
}

#[test]
fn a_master_insert_is_stacked_right_under_the_master() {
    let (mut d, r, master) = rack();
    one_step(&mut d, &r, add_insert(&master, 0, REVERB));
    one_step(&mut d, &r, add_insert(&master, 1, EQ));
    one_step(&mut d, &r, add_insert(&master, 0, DELAY));
    assert_eq!(inserts(&d, &r, &master).len(), 3);
    assert_master_stacked(&d, &r, &master);
}

#[test]
fn master_inserts_reorder_and_leave_in_one_step() {
    let (mut d, r, master) = rack();
    run(
        &mut d,
        &r,
        [
            add_insert(&master, 0, REVERB),
            add_insert(&master, 1, EQ),
            add_insert(&master, 2, DELAY),
        ],
    );
    let before = inserts(&d, &r, &master);
    one_step(
        &mut d,
        &r,
        cc(ConsoleCommand::MoveInsert {
            channel: master.clone(),
            from: 0,
            to: 2,
        }),
    );
    let now = inserts(&d, &r, &master);
    assert_eq!(
        now,
        vec![before[1].clone(), before[2].clone(), before[0].clone()]
    );
    assert_master_stacked(&d, &r, &master);
    one_step(
        &mut d,
        &r,
        cc(ConsoleCommand::ExtractInsert {
            channel: master.clone(),
            slot: 1,
            to: None,
        }),
    );
    assert_eq!(inserts(&d, &r, &master).len(), 2);
    assert_master_stacked(&d, &r, &master);
}

#[test]
fn creating_with_the_master_selected_makes_a_send_and_with_its_inserts_an_insert() {
    let (mut d, r, master) = rack();
    one_step(
        &mut d,
        &r,
        create(CreateTarget::Selected(master.clone()), REVERB),
    );
    assert_eq!(d.doc().console.returns.iter().flatten().count(), 1);
    assert!(inserts(&d, &r, &master).is_empty());
    // The insert area selected: an insert of the master section, put before the send effect.
    one_step(&mut d, &r, create(CreateTarget::MasterInserts, EQ));
    assert_eq!(inserts(&d, &r, &master).len(), 1);
    assert_eq!(d.doc().console.returns.iter().flatten().count(), 1);
    assert_master_stacked(&d, &r, &master);
    // An insert selected: the new effect follows it in the chain.
    let first = inserts(&d, &r, &master)[0].clone();
    one_step(
        &mut d,
        &r,
        create(CreateTarget::Selected(first.clone()), DELAY),
    );
    let now = inserts(&d, &r, &master);
    assert_eq!(now.len(), 2);
    assert_eq!(now[0], first);
    assert_master_stacked(&d, &r, &master);
}

#[test]
fn a_new_send_goes_after_the_master_inserts_and_the_sends_in_use() {
    let (mut d, r, master) = rack();
    run(&mut d, &r, [add_insert(&master, 0, EQ)]);
    for _ in 0..3 {
        run(
            &mut d,
            &r,
            [create(CreateTarget::Selected(master.clone()), REVERB)],
        );
    }
    let returns: Vec<DeviceKey> = d.doc().console.returns.iter().flatten().cloned().collect();
    assert_eq!(returns.len(), 3);
    assert_master_stacked(&d, &r, &master);
    for (slot, e) in returns.iter().enumerate() {
        assert_eq!(send_slot_of(d.doc(), &r, e), Some(slot));
    }
    let all = order(&d);
    let at = all.iter().position(|k| k == &master).unwrap();
    assert_eq!(all[at + 1], inserts(&d, &r, &master)[0], "the insert first");
    assert_eq!(all[at + 2], returns[0]);
}

#[test]
fn an_insert_added_after_the_sends_goes_above_them() {
    let (mut d, r, master) = rack();
    for _ in 0..2 {
        run(
            &mut d,
            &r,
            [create(CreateTarget::Selected(master.clone()), REVERB)],
        );
    }
    one_step(&mut d, &r, add_insert(&master, 0, EQ));
    assert_master_stacked(&d, &r, &master);
    let all = order(&d);
    let at = all.iter().position(|k| k == &master).unwrap();
    assert_eq!(all[at + 1], inserts(&d, &r, &master)[0]);
}

#[test]
fn eight_sends_are_taken_and_the_ninth_is_refused_with_a_status_text() {
    let (mut d, r, master) = rack();
    for n in 0..8 {
        run(
            &mut d,
            &r,
            [create(CreateTarget::Selected(master.clone()), REVERB)],
        );
        assert_eq!(d.doc().console.returns.iter().flatten().count(), n + 1);
    }
    assert_master_stacked(&d, &r, &master);
    assert!(d.doc().console.returns.iter().all(Option::is_some));
    let note = effect_route_note(
        d.doc(),
        &r,
        &typ(REVERB),
        &CreateTarget::Selected(master.clone()),
        &CreateOptions::default(),
    );
    assert_eq!(note, Some(RouteNote::SendSlotsFull));
    assert_eq!(
        RouteNote::SendSlotsFull.text(),
        atomik_model::SEND_SLOTS_FULL
    );
    assert!(atomik_model::SEND_SLOTS_FULL.contains("8"));
    // The master insert area still takes effects.
    run(&mut d, &r, [create(CreateTarget::MasterInserts, EQ)]);
    assert_eq!(inserts(&d, &r, &master).len(), 1);
    assert_master_stacked(&d, &r, &master);
}

#[test]
fn a_browser_drop_right_under_the_master_is_a_master_insert() {
    let (mut d, r, master) = rack();
    run(
        &mut d,
        &r,
        [create(CreateTarget::Selected(master.clone()), REVERB)],
    );
    let at = order(&d).iter().position(|k| k == &master).unwrap();
    let column = d.doc().device(&master).unwrap().column;
    let slot = RackSlot {
        column,
        index: at + 1 - d.doc().column_range(column).start,
    };
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::CreateEffectAt {
            slot,
            source: typ(EQ),
            key: None,
            options: CreateOptions::default(),
        }),
    );
    assert_eq!(inserts(&d, &r, &master).len(), 1);
    assert_master_stacked(&d, &r, &master);
}

#[test]
fn a_device_moved_into_the_master_stack_goes_past_it() {
    let (mut d, r, master) = rack();
    run(&mut d, &r, [add_insert(&master, 0, EQ)]);
    run(
        &mut d,
        &r,
        [create(CreateTarget::Selected(master.clone()), REVERB)],
    );
    run(&mut d, &r, [add(usize::MAX, DELAY, Some("loose"))]);
    let stack = master_stack(d.doc(), &r);
    assert_eq!(stack.len(), 2);
    let at = order(&d).iter().position(|k| k == &master).unwrap();
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::MoveDevices {
            keys: vec![key("loose")],
            to: at + 1,
            route: false,
        })],
    );
    let all = order(&d);
    assert_eq!(&all[at + 1..at + 3], &stack[..], "the stack stays together");
    assert_eq!(all[at + 3], key("loose"));
}

#[test]
fn use_as_send_effect_brings_the_effect_under_the_master() {
    let (mut d, r, master) = rack();
    run(&mut d, &r, [add_insert(&master, 0, EQ)]);
    run(&mut d, &r, [add(usize::MAX, DELAY, Some("loose"))]);
    run(&mut d, &r, [add(usize::MAX, REVERB, Some("last"))]);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::AssignSend {
            effect: key("loose"),
        }),
    );
    assert_eq!(d.doc().console.returns[0], Some(key("loose")));
    assert_master_stacked(&d, &r, &master);
    let all = order(&d);
    let at = all.iter().position(|k| k == &master).unwrap();
    assert_eq!(all[at + 2], key("loose"), "right after the master's insert");
}

#[test]
fn send_chains_follow_the_cables_in_series() {
    let (mut d, r, master) = rack();
    run(
        &mut d,
        &r,
        [create(CreateTarget::Selected(master.clone()), REVERB)],
    );
    let first = send_chain(d.doc(), &r, 0)[0].clone();
    run(
        &mut d,
        &r,
        [create(CreateTarget::Selected(first.clone()), DELAY)],
    );
    let chain = send_chain(d.doc(), &r, 0);
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0], first);
    assert_eq!(
        d.doc().console.returns[0].as_ref(),
        chain.first(),
        "the return slot names the effect cabled from the send"
    );
    assert!(send_chain(d.doc(), &r, 1).is_empty());
    assert_eq!(send_slot_of(d.doc(), &r, &chain[1]), Some(0));
    assert_master_stacked(&d, &r, &master);
}
