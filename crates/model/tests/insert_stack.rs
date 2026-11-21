//! The insert stack: the effects of a channel's insert chain sit in the rack right under it, in
//! chain order. Every gesture (create, add at a position, reorder, take out, delete, move the
//! channel) is one undo step, keeps the cables of the chain valid and keeps the stack.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateSource, CreateTarget, DeviceKey, Document,
    InsertEffect, RackSlot, WorkflowCommand, insert_chain, new_project_doc,
};
use common::workflow::*;
use common::*;
use proptest::prelude::*;

const EFFECTS: [&str; 4] = [
    "atomik.reverb",
    "atomik.eq",
    "atomik.compressor",
    "atomik.distortion",
];

fn typ(t: &str) -> CreateSource {
    CreateSource::Type(t.into())
}

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn add_insert(channel: &DeviceKey, slot: usize, t: &str) -> Command {
    cc(ConsoleCommand::AddInsert {
        channel: channel.clone(),
        slot,
        effect: InsertEffect::New { type_id: t.into() },
    })
}

fn add_existing(channel: &DeviceKey, slot: usize, k: &DeviceKey) -> Command {
    cc(ConsoleCommand::AddInsert {
        channel: channel.clone(),
        slot,
        effect: InsertEffect::Existing(k.clone()),
    })
}

fn extract(channel: &DeviceKey, slot: usize, to: Option<RackSlot>) -> Command {
    cc(ConsoleCommand::ExtractInsert {
        channel: channel.clone(),
        slot,
        to,
    })
}

/// A project with one instrument, its channel and `n` reverb-family inserts in the chain.
fn rack(n: usize) -> (Document, Registry, DeviceKey) {
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
    let channel = d
        .doc()
        .console
        .channel_of(&key("synth"))
        .expect("a channel")
        .channel
        .clone();
    for i in 0..n {
        run(
            &mut d,
            &r,
            [add_insert(&channel, i, EFFECTS[i % EFFECTS.len()])],
        );
    }
    (d, r, channel)
}

fn chain(d: &Document, r: &Registry, channel: &DeviceKey) -> Vec<DeviceKey> {
    insert_chain(d.doc(), r, channel)
}

fn order(d: &Document) -> Vec<DeviceKey> {
    d.doc().devices.iter().map(|x| x.key.clone()).collect()
}

/// Every strip's chain sits right under it, in chain order, in its column.
fn assert_stacked(d: &Document, r: &Registry) {
    let all = order(d);
    let strips: Vec<&DeviceKey> = d
        .doc()
        .console
        .channels
        .iter()
        .map(|c| &c.channel)
        .chain(d.doc().console.buses.iter().map(|b| &b.bus))
        .collect();
    for strip in strips {
        let members = insert_chain(d.doc(), r, strip);
        let at = all.iter().position(|k| k == strip).expect("the strip");
        let column = d.doc().devices[at].column;
        for (i, m) in members.iter().enumerate() {
            assert_eq!(
                all.get(at + 1 + i),
                Some(m),
                "{strip}: insert {i} follows it"
            );
            assert_eq!(d.doc().device(m).unwrap().column, column, "{m} column");
        }
    }
}

#[test]
fn adding_in_the_middle_shifts_the_chain_and_stacks_the_effect_in_one_step() {
    let (mut d, r, ch) = rack(2);
    let before = chain(&d, &r, &ch);
    one_step(&mut d, &r, add_insert(&ch, 1, "atomik.distortion"));
    let now = chain(&d, &r, &ch);
    assert_eq!(now.len(), 3);
    assert_eq!(now[0], before[0]);
    assert_eq!(now[2], before[1]);
    assert_eq!(
        d.doc().device(&now[1]).unwrap().type_id,
        "atomik.distortion"
    );
    assert_stacked(&d, &r);
}

#[test]
fn adding_at_the_top_and_the_end() {
    let (mut d, r, ch) = rack(2);
    let before = chain(&d, &r, &ch);
    run(&mut d, &r, [add_insert(&ch, 0, "atomik.eq")]);
    run(&mut d, &r, [add_insert(&ch, 3, "atomik.reverb")]);
    let now = chain(&d, &r, &ch);
    assert_eq!(now.len(), 4);
    assert_eq!(&now[1..3], &before[..]);
    assert_stacked(&d, &r);
    let pos = order(&d).iter().position(|k| k == &ch).unwrap();
    assert_eq!(
        order(&d)[pos + 1],
        now[0],
        "the first insert is right under the channel"
    );
}

#[test]
fn an_existing_device_is_added_at_a_position_and_comes_under_the_channel() {
    let (mut d, r, ch) = rack(2);
    // A loose effect elsewhere in the rack.
    run(&mut d, &r, [add(usize::MAX, "atomik.eq", Some("loose"))]);
    let before = chain(&d, &r, &ch);
    one_step(&mut d, &r, add_existing(&ch, 1, &key("loose")));
    let now = chain(&d, &r, &ch);
    assert_eq!(
        now,
        vec![before[0].clone(), key("loose"), before[1].clone()]
    );
    assert_stacked(&d, &r);
}

#[test]
fn adding_to_a_full_chain_is_refused_with_the_reason() {
    let (mut d, r, ch) = rack(16);
    let e = rejected(&mut d, &r, add_insert(&ch, 3, "atomik.eq"));
    assert!(e.to_string().contains("full"), "{e}");
}

#[test]
fn a_refused_existing_device_says_why() {
    let (mut d, r, ch) = rack(1);
    let e = rejected(&mut d, &r, add_existing(&ch, 0, &key("synth")));
    assert!(!e.to_string().is_empty());
    let first = chain(&d, &r, &ch)[0].clone();
    let e = rejected(&mut d, &r, add_existing(&ch, 1, &first));
    assert!(e.to_string().contains("MoveInsert"), "{e}");
}

#[test]
fn taking_an_insert_out_closes_the_chain_and_leaves_the_effect_uncabled_in_the_rack() {
    let (mut d, r, ch) = rack(3);
    let before = chain(&d, &r, &ch);
    one_step(&mut d, &r, extract(&ch, 1, None));
    let now = chain(&d, &r, &ch);
    assert_eq!(now, vec![before[0].clone(), before[2].clone()]);
    let out = &before[1];
    assert!(
        d.doc().device(out).is_some(),
        "the effect stays in the rack"
    );
    assert!(
        !d.doc()
            .cables
            .iter()
            .any(|c| &c.from.device == out || &c.to.device == out),
        "and has no cable"
    );
}

#[test]
fn taking_an_insert_out_to_a_rack_slot_moves_it_there_in_the_same_step() {
    let (mut d, r, ch) = rack(2);
    let out = chain(&d, &r, &ch)[0].clone();
    run(&mut d, &r, [add(usize::MAX, "atomik.eq", Some("other"))]);
    let slot = RackSlot {
        column: 0,
        index: d.doc().devices.len() - 1,
    };
    one_step(&mut d, &r, extract(&ch, 0, Some(slot)));
    let all = order(&d);
    assert_eq!(all.last(), Some(&out), "dropped at the end of the column");
    assert_eq!(chain(&d, &r, &ch).len(), 1);
    assert_stacked(&d, &r);
}

#[test]
fn an_empty_slot_cannot_be_taken_out() {
    let (mut d, r, ch) = rack(1);
    rejected(&mut d, &r, extract(&ch, 1, None));
}

#[test]
fn moving_an_insert_keeps_the_stack_in_chain_order() {
    let (mut d, r, ch) = rack(3);
    let before = chain(&d, &r, &ch);
    one_step(
        &mut d,
        &r,
        cc(ConsoleCommand::MoveInsert {
            channel: ch.clone(),
            from: 0,
            to: 2,
        }),
    );
    assert_eq!(
        chain(&d, &r, &ch),
        vec![before[1].clone(), before[2].clone(), before[0].clone()]
    );
    assert_stacked(&d, &r);
}

#[test]
fn deleting_an_insert_reconnects_its_neighbours() {
    let (mut d, r, ch) = rack(3);
    let before = chain(&d, &r, &ch);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DeleteDevices {
            keys: vec![before[1].clone()],
        }),
    );
    assert_eq!(
        chain(&d, &r, &ch),
        vec![before[0].clone(), before[2].clone()]
    );
    assert_stacked(&d, &r);
}

#[test]
fn deleting_the_only_insert_leaves_an_empty_valid_chain() {
    let (mut d, r, ch) = rack(1);
    let only = chain(&d, &r, &ch)[0].clone();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::DeleteDevices { keys: vec![only] }),
    );
    assert!(chain(&d, &r, &ch).is_empty());
}

#[test]
fn creating_an_effect_with_an_insert_selected_puts_it_after_that_insert() {
    let (mut d, r, ch) = rack(2);
    let before = chain(&d, &r, &ch);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::CreateEffect {
            source: typ("atomik.distortion"),
            target: CreateTarget::Selected(before[0].clone()),
            key: None,
            options: CreateOptions::default(),
        }),
    );
    let now = chain(&d, &r, &ch);
    assert_eq!(now.len(), 3);
    assert_eq!(now[0], before[0]);
    assert_eq!(now[2], before[1]);
    assert_eq!(
        d.doc().device(&now[1]).unwrap().type_id,
        "atomik.distortion"
    );
    assert_stacked(&d, &r);
}

#[test]
fn creating_an_effect_with_the_channel_selected_appends_it() {
    let (mut d, r, ch) = rack(2);
    let before = chain(&d, &r, &ch);
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::CreateEffect {
            source: typ("atomik.distortion"),
            target: CreateTarget::Selected(ch.clone()),
            key: None,
            options: CreateOptions::default(),
        })],
    );
    let now = chain(&d, &r, &ch);
    assert_eq!(&now[..2], &before[..]);
    assert_eq!(now.len(), 3);
    assert_stacked(&d, &r);
}

fn slot_after(d: &Document, k: &DeviceKey) -> RackSlot {
    let at = d.doc().devices.iter().position(|x| &x.key == k).unwrap();
    let column = d.doc().devices[at].column;
    RackSlot {
        column,
        index: at + 1 - d.doc().column_range(column).start,
    }
}

#[test]
fn a_browser_drop_under_the_channel_or_an_insert_joins_the_stack_at_that_place() {
    let (mut d, r, ch) = rack(2);
    let before = chain(&d, &r, &ch);
    let create_at = |slot| {
        wf(WorkflowCommand::CreateEffectAt {
            slot,
            source: typ("atomik.distortion"),
            key: None,
            options: CreateOptions::default(),
        })
    };
    let command = create_at(slot_after(&d, &ch));
    one_step(&mut d, &r, command);
    let now = chain(&d, &r, &ch);
    assert_eq!(now.len(), 3);
    assert_eq!(&now[1..], &before[..], "created at the top of the stack");
    let command = create_at(slot_after(&d, &now[1]));
    one_step(&mut d, &r, command);
    let now2 = chain(&d, &r, &ch);
    assert_eq!(now2.len(), 4);
    assert_eq!(now2[0], now[0]);
    assert_eq!(now2[1], now[1]);
    assert_eq!(now2[3], now[2]);
    assert_stacked(&d, &r);
}

#[test]
fn moving_the_channel_moves_its_inserts_along_in_order() {
    let (mut d, r, ch) = rack(3);
    let before = chain(&d, &r, &ch);
    run(&mut d, &r, [add(usize::MAX, "atomik.eq", Some("tail"))]);
    let end = RackSlot {
        column: 0,
        index: d.doc().devices.len(),
    };
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::MoveDevicesToSlot {
            keys: vec![ch.clone()],
            slot: end,
            route: true,
        }),
    );
    assert_eq!(chain(&d, &r, &ch), before, "the chain is untouched");
    assert_stacked(&d, &r);
    let all = order(&d);
    assert_eq!(
        all[all.len() - 1],
        before[2],
        "the stack came along to the end"
    );
}

#[test]
fn a_flat_move_of_the_channel_takes_the_inserts_too() {
    let (mut d, r, ch) = rack(2);
    let before = chain(&d, &r, &ch);
    run(&mut d, &r, [add(usize::MAX, "atomik.eq", Some("tail"))]);
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::MoveDevices {
            keys: vec![ch.clone()],
            to: usize::MAX,
            route: false,
        })],
    );
    assert_eq!(chain(&d, &r, &ch), before);
    assert_stacked(&d, &r);
}

#[test]
fn a_bus_has_the_same_stack() {
    let (mut d, r, _) = rack(0);
    run(
        &mut d,
        &r,
        [cc(ConsoleCommand::CreateBus {
            name: "Drums".into(),
            dest: atomik_model::OutputDest::Master,
        })],
    );
    let bus = d.doc().console.buses[0].bus.clone();
    run(&mut d, &r, [add_insert(&bus, 0, "atomik.reverb")]);
    run(&mut d, &r, [add_insert(&bus, 0, "atomik.eq")]);
    assert_eq!(chain(&d, &r, &bus).len(), 2);
    assert_stacked(&d, &r);
}

// ----------------------------------------------------------------------------- property

#[derive(Debug, Clone)]
enum Step {
    Add(usize, usize),
    Existing(usize),
    Take(usize, bool),
    Reorder(usize, usize),
    Delete(usize),
    CreateAfter(usize, usize),
    MoveChannel(usize),
    Loose,
    Undo,
}

fn steps() -> impl Strategy<Value = Vec<Step>> {
    let step = prop_oneof![
        (0usize..6, 0usize..4).prop_map(|(s, t)| Step::Add(s, t)),
        (0usize..6).prop_map(Step::Existing),
        (0usize..6, any::<bool>()).prop_map(|(s, o)| Step::Take(s, o)),
        (0usize..6, 0usize..6).prop_map(|(a, b)| Step::Reorder(a, b)),
        (0usize..6).prop_map(Step::Delete),
        (0usize..6, 0usize..4).prop_map(|(a, t)| Step::CreateAfter(a, t)),
        (0usize..40).prop_map(Step::MoveChannel),
        Just(Step::Loose),
        Just(Step::Undo),
    ];
    proptest::collection::vec(step, 1..24)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Whatever gestures run, the chain read back from the cables is the stack drawn: every
    /// chain sits right under its channel in chain order, and every accepted gesture is one undo
    /// step (an undo of it restores the document).
    #[test]
    fn random_gestures_keep_the_stack_equal_to_the_chain(plan in steps()) {
        let r = console_registry();
        let mut d = Document::new(new_project_doc());
        run(&mut d, &r, [
            wf(WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: typ(SYNTH),
                key: Some(key("one")),
                options: CreateOptions::default(),
            }),
            wf(WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: typ(SYNTH),
                key: Some(key("two")),
                options: CreateOptions::default(),
            }),
        ]);
        let channels: Vec<DeviceKey> =
            d.doc().console.channels.iter().map(|c| c.channel.clone()).collect();
        let mut loose = 0usize;
        // Steps of the plan that can still be undone (the set-up is not one of them).
        let mut undoable = 0usize;
        for step in plan {
            let ch = channels[match &step {
                Step::Add(s, _) | Step::Take(s, _) | Step::Delete(s) | Step::CreateAfter(s, _)
                    if s % 2 == 1 => 1,
                _ => 0,
            }].clone();
            let members = insert_chain(d.doc(), &r, &ch);
            let nth = |n: usize| members.get(n % members.len().max(1)).cloned();
            let before = text(&d);
            let command = match step {
                Step::Add(s, t) => Some(add_insert(&ch, s % (members.len() + 1), EFFECTS[t])),
                Step::Existing(n) if loose > 0 => {
                    let k = key(&format!("loose{}", (n % loose) + 1));
                    Some(add_existing(&ch, n % (members.len() + 1), &k))
                }
                Step::Existing(_) => None,
                Step::Take(n, out) => nth(n).map(|_| {
                    let to = out.then_some(RackSlot { column: 0, index: usize::MAX });
                    extract(&ch, n % members.len(), to)
                }),
                Step::Reorder(a, b) => (members.len() > 1).then(|| {
                    cc(ConsoleCommand::MoveInsert {
                        channel: ch.clone(),
                        from: a % members.len(),
                        to: b % members.len(),
                    })
                }),
                Step::Delete(n) => nth(n).map(|k| wf(WorkflowCommand::DeleteDevices { keys: vec![k] })),
                Step::CreateAfter(n, t) => {
                    let above = if members.is_empty() || n % 2 == 0 { ch.clone() } else { nth(n).unwrap() };
                    Some(wf(WorkflowCommand::CreateEffectAt {
                        slot: slot_after(&d, &above),
                        source: typ(EFFECTS[t]),
                        key: None,
                        options: CreateOptions::default(),
                    }))
                }
                Step::MoveChannel(i) => Some(wf(WorkflowCommand::MoveDevicesToSlot {
                    keys: vec![ch.clone()],
                    slot: RackSlot { column: 0, index: i },
                    route: i % 2 == 0,
                })),
                Step::Loose => {
                    loose += 1;
                    Some(add(usize::MAX, "atomik.eq", Some(&format!("loose{loose}"))))
                }
                Step::Undo => {
                    if undoable > 0 && d.undo().is_some() {
                        undoable -= 1;
                    }
                    None
                }
            };
            if let Some(command) = command
                && d.apply(command, &r).is_ok()
            {
                let after = text(&d);
                if before != after {
                    undoable += 1;
                    d.undo().expect("one undo step");
                    prop_assert_eq!(&text(&d), &before, "undo restores the document");
                    d.redo().expect("redo");
                    prop_assert_eq!(&text(&d), &after);
                }
            }
            assert_stacked(&d, &r);
        }
    }
}

#[test]
fn an_insert_dropped_on_another_channel_moves_chains_in_one_step() {
    let (mut d, r, ch) = rack(2);
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: typ(SYNTH),
            key: Some(key("other")),
            options: CreateOptions::default(),
        })],
    );
    let second = d
        .doc()
        .console
        .channel_of(&key("other"))
        .unwrap()
        .channel
        .clone();
    let moved = chain(&d, &r, &ch)[0].clone();
    let stays = chain(&d, &r, &ch)[1].clone();
    one_step(&mut d, &r, add_existing(&second, 0, &moved));
    assert_eq!(chain(&d, &r, &ch), vec![stays]);
    assert_eq!(chain(&d, &r, &second), vec![moved]);
    assert_stacked(&d, &r);
}

#[test]
fn moving_an_instrument_to_another_column_takes_its_channel_and_inserts() {
    let (mut d, r, ch) = rack(2);
    let before = chain(&d, &r, &ch);
    run(&mut d, &r, [Command::AddRackColumn]);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::MoveDevicesToSlot {
            keys: vec![key("synth")],
            slot: RackSlot {
                column: 1,
                index: 0,
            },
            route: true,
        }),
    );
    let column = |d: &Document, k: &DeviceKey| d.doc().device(k).unwrap().column;
    assert_eq!(column(&d, &key("synth")), 1);
    assert_eq!(column(&d, &ch), 1, "the channel came along");
    let all = order(&d);
    let at = all.iter().position(|k| k == &key("synth")).unwrap();
    assert_eq!(all[at + 1], ch, "right under its instrument");
    assert_eq!(chain(&d, &r, &ch), before, "the chain is untouched");
    assert_stacked(&d, &r);
    // One undo puts all of it back.
    assert!(d.undo().is_some());
    assert_eq!(column(&d, &key("synth")), 0);
    assert_eq!(column(&d, &ch), 0);
}

#[test]
fn moving_an_instrument_along_its_column_keeps_its_channel_under_it() {
    let (mut d, r, ch) = rack(1);
    run(&mut d, &r, [add(usize::MAX, "atomik.eq", Some("tail"))]);
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::MoveDevices {
            keys: vec![key("synth")],
            to: usize::MAX,
            route: false,
        })],
    );
    let all = order(&d);
    let at = all.iter().position(|k| k == &key("synth")).unwrap();
    assert_eq!(all[at + 1], ch, "{all:?}");
    assert!(
        all.iter().position(|k| k == &key("tail")).unwrap() < at,
        "the block went below the other device: {all:?}"
    );
    assert_stacked(&d, &r);
}

#[test]
fn a_channel_moved_by_itself_leaves_its_instrument() {
    let (mut d, r, ch) = rack(0);
    run(&mut d, &r, [Command::AddRackColumn]);
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::MoveDevicesToSlot {
            keys: vec![ch.clone()],
            slot: RackSlot {
                column: 1,
                index: 0,
            },
            route: false,
        })],
    );
    assert_eq!(d.doc().device(&ch).unwrap().column, 1);
    assert_eq!(d.doc().device(&key("synth")).unwrap().column, 0);
}

/// `move_changes_nothing` answers as the model does: for every place an instrument can be
/// dropped at, it says "nothing" exactly when the model refuses the move as one that changes
/// nothing. The instrument takes its channel along, so a place that is not the instrument's own
/// can still be where the two already are.
#[test]
fn move_changes_nothing_answers_as_the_model_does_for_an_instrument_with_its_channel() {
    let (mut d, r, ch) = rack(2);
    let synth_at = |d: &Document| order(d).iter().position(|k| k == &key("synth")).unwrap();
    let check = |d: &Document| {
        let mut nothing = 0;
        let mut own_place_only = true;
        for index in 0..=d.doc().devices.len() {
            let command = wf(WorkflowCommand::MoveDevicesToSlot {
                keys: vec![key("synth")],
                slot: RackSlot { column: 0, index },
                route: true,
            });
            let said = atomik_model::move_changes_nothing(d.doc(), &r, &command);
            let refused = match Document::new(d.doc().clone()).apply(command, &r) {
                Ok(_) => false,
                Err(e) => e.to_string().contains("already there"),
            };
            assert_eq!(said, refused, "index {index} of {:?}", order(d));
            if said {
                nothing += 1;
                own_place_only &= index == synth_at(d);
            }
        }
        (nothing, own_place_only)
    };
    // The channel right under its instrument, as created.
    let (nothing, _) = check(&d);
    assert!(nothing >= 1, "some place leaves the block where it is");
    // The channel moved above its instrument: the place above the channel is not the
    // instrument's own, and the two are already there once the channel follows.
    let at = synth_at(&d);
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::MoveDevicesToSlot {
            keys: vec![ch.clone()],
            slot: RackSlot {
                column: 0,
                index: at,
            },
            route: false,
        })],
    );
    assert!(
        order(&d).iter().position(|k| k == &ch).unwrap() < synth_at(&d),
        "{:?}",
        order(&d)
    );
    let (nothing, own_place_only) = check(&d);
    assert!(nothing >= 1);
    assert!(
        !own_place_only,
        "a place that is not the instrument's own changes nothing"
    );
    // Another command is never a move that changes nothing.
    assert!(!atomik_model::move_changes_nothing(
        d.doc(),
        &r,
        &Command::AddRackColumn
    ));
}
