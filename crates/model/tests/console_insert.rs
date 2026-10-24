//! Insert chains: `SetInsert` / `MoveInsert` wire up to four effects
//! between a channel's (or the master's) insert send and return, derived from cables.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Cable, Command, ConsoleCommand, Document, INSERT_SLOTS, InsertEffect, ModelError,
    bypass_inserts_commands, insert_bypass_states, insert_chain, inserts_bypassed,
};
use common::*;

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn set(device: &str, slot: usize, effect: Option<InsertEffect>) -> Command {
    cc(ConsoleCommand::SetInsert {
        channel: key(device),
        slot,
        effect,
    })
}

fn existing(k: &str) -> Option<InsertEffect> {
    Some(InsertEffect::Existing(key(k)))
}

fn new_fx(t: &str) -> Option<InsertEffect> {
    Some(InsertEffect::New { type_id: t.into() })
}

fn mv(device: &str, from: usize, to: usize) -> Command {
    cc(ConsoleCommand::MoveInsert {
        channel: key(device),
        from,
        to,
    })
}

/// synth, delay, mixer + compressor, eq, reverb, distortion; a channel for the synth.
fn rack(reg: &Registry) -> Document {
    let mut d = Document::new(three_devices(&registry()).doc().clone());
    for (i, (t, k)) in [
        ("atomik.compressor", "comp"),
        ("atomik.eq", "eq"),
        ("atomik.reverb", "verb"),
        ("atomik.distortion", "dist"),
    ]
    .into_iter()
    .enumerate()
    {
        d.apply(add(3 + i, t, Some(k)), reg).unwrap();
    }
    d.apply(
        cc(ConsoleCommand::AddChannel {
            source: key("synth"),
        }),
        reg,
    )
    .unwrap();
    d
}

fn chain_of(d: &Document, reg: &Registry, dev: &str) -> Vec<String> {
    insert_chain(d.doc(), reg, &key(dev))
        .into_iter()
        .map(|k| k.0)
        .collect()
}

fn rejects(d: &mut Document, cmd: Command, reg: &Registry) -> ModelError {
    let before = d.doc().clone();
    let can_undo = d.can_undo();
    let e = d.apply(cmd, reg).expect_err("must be rejected");
    assert_eq!(d.doc(), &before, "nothing changes on rejection");
    assert_eq!(d.can_undo(), can_undo);
    e
}

fn cables_of(d: &Document, dev: &str) -> Vec<Cable> {
    d.doc()
        .cables
        .iter()
        .filter(|c| c.from.device.0 == dev || c.to.device.0 == dev)
        .cloned()
        .collect()
}

#[test]
fn adding_existing_effects_wires_send_fx_return_in_order() {
    let reg = console_registry();
    let mut d = rack(&reg);
    d.apply(set("channel", 0, existing("comp")), &reg).unwrap();
    assert_eq!(chain_of(&d, &reg, "channel"), ["comp"]);
    d.apply(set("channel", 1, existing("eq")), &reg).unwrap();
    assert_eq!(chain_of(&d, &reg, "channel"), ["comp", "eq"]);
    let cables = &d.doc().cables;
    for (f, t) in [
        ("channel:insert_send_l", "comp:in_l"),
        ("channel:insert_send_r", "comp:in_r"),
        ("comp:out_l", "eq:in_l"),
        ("comp:out_r", "eq:in_r"),
        ("eq:out_l", "channel:insert_return_l"),
        ("eq:out_r", "channel:insert_return_r"),
    ] {
        assert!(cables.contains(&cable(f, t)), "{f} -> {t}");
    }
    assert!(!cables.contains(&cable("comp:out_l", "channel:insert_return_l")));
}

#[test]
fn new_effect_is_created_after_the_previous_stage_and_is_one_undo_step() {
    let reg = console_registry();
    let mut d = rack(&reg);
    let before = d.doc().clone();
    d.apply(set("channel", 0, new_fx("atomik.compressor")), &reg)
        .unwrap();
    let chain = chain_of(&d, &reg, "channel");
    assert_eq!(chain.len(), 1);
    let order = keys(&d);
    let ch = order.iter().position(|k| k == "channel").unwrap();
    assert_eq!(order[ch + 1], chain[0], "placed right after the channel");
    let after_one = d.doc().clone();
    d.apply(set("channel", 1, new_fx("atomik.eq")), &reg)
        .unwrap();
    let chain = chain_of(&d, &reg, "channel");
    assert_eq!(chain.len(), 2);
    let order = keys(&d);
    let c0 = order.iter().position(|k| *k == chain[0]).unwrap();
    assert_eq!(order[c0 + 1], chain[1]);
    assert!(d.undo(&reg).is_some());
    assert_eq!(d.doc(), &after_one);
    assert!(d.undo(&reg).is_some());
    assert_eq!(d.doc(), &before);
    assert!(d.redo(&reg).is_some());
    assert_eq!(d.doc(), &after_one);
}

#[test]
fn removing_a_slot_closes_the_chain_and_keeps_the_device() {
    let reg = console_registry();
    let mut d = rack(&reg);
    for (i, k) in ["comp", "eq", "verb"].into_iter().enumerate() {
        d.apply(set("channel", i, existing(k)), &reg).unwrap();
    }
    let full = d.doc().clone();
    d.apply(set("channel", 1, None), &reg).unwrap();
    assert_eq!(chain_of(&d, &reg, "channel"), ["comp", "verb"]);
    assert!(
        d.doc().device(&key("eq")).is_some(),
        "device stays in the rack"
    );
    assert!(cables_of(&d, "eq").is_empty());
    assert!(d.doc().cables.contains(&cable("comp:out_l", "verb:in_l")));
    d.apply(set("channel", 0, None), &reg).unwrap();
    d.apply(set("channel", 0, None), &reg).unwrap();
    assert!(chain_of(&d, &reg, "channel").is_empty());
    assert!(
        !d.doc()
            .cables
            .iter()
            .any(|c| c.from.socket.starts_with("insert_send")
                || c.to.socket.starts_with("insert_return")),
        "an empty chain leaves the insert unpatched"
    );
    let e = rejects(&mut d, set("channel", 0, None), &reg);
    assert!(matches!(e, ModelError::BadConsole(_)));
    for _ in 0..3 {
        assert!(d.undo(&reg).is_some());
    }
    assert_eq!(d.doc(), &full);
}

#[test]
fn replacing_a_slot_swaps_the_effect() {
    let reg = console_registry();
    let mut d = rack(&reg);
    d.apply(set("channel", 0, existing("comp")), &reg).unwrap();
    d.apply(set("channel", 1, existing("eq")), &reg).unwrap();
    d.apply(set("channel", 0, existing("verb")), &reg).unwrap();
    assert_eq!(chain_of(&d, &reg, "channel"), ["verb", "eq"]);
    assert!(cables_of(&d, "comp").is_empty());
    let before = d.doc().clone();
    d.apply(set("channel", 0, existing("verb")), &reg).unwrap();
    assert_eq!(d.doc(), &before, "the same effect again changes nothing");
}

#[test]
fn move_insert_reorders_and_rewires() {
    let reg = console_registry();
    let mut d = rack(&reg);
    for (i, k) in ["comp", "eq", "verb"].into_iter().enumerate() {
        d.apply(set("channel", i, existing(k)), &reg).unwrap();
    }
    let before = d.doc().clone();
    d.apply(mv("channel", 2, 0), &reg).unwrap();
    assert_eq!(chain_of(&d, &reg, "channel"), ["verb", "comp", "eq"]);
    assert!(
        d.doc()
            .cables
            .contains(&cable("eq:out_r", "channel:insert_return_r"))
    );
    assert!(d.undo(&reg).is_some());
    assert_eq!(d.doc(), &before);
    let e = rejects(&mut d, mv("channel", 0, 3), &reg);
    assert!(matches!(e, ModelError::BadConsole(_)));
}

#[test]
fn at_most_four_inserts() {
    let reg = console_registry();
    let mut d = rack(&reg);
    for (i, k) in ["comp", "eq", "verb", "dist"].into_iter().enumerate() {
        d.apply(set("channel", i, existing(k)), &reg).unwrap();
    }
    assert_eq!(chain_of(&d, &reg, "channel").len(), INSERT_SLOTS);
    let e = rejects(&mut d, set("channel", 4, new_fx("atomik.eq")), &reg);
    assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
    let e = rejects(&mut d, set("channel", 9, None), &reg);
    assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
}

#[test]
fn rejections_report_and_change_nothing() {
    let reg = console_registry();
    let mut d = rack(&reg);
    let e = rejects(&mut d, set("channel", 1, existing("comp")), &reg);
    assert!(
        matches!(e, ModelError::BadConsole(_)),
        "slot past the end: {e:?}"
    );
    for dev in ["synth", "delay", "ghost"] {
        rejects(&mut d, set(dev, 0, existing("comp")), &reg);
    }
    for fx in ["channel", "master", "synth", "mixer", "ghost"] {
        rejects(&mut d, set("channel", 0, existing(fx)), &reg);
    }
    rejects(&mut d, set("channel", 0, new_fx("atomik.poly-synth")), &reg);
    rejects(&mut d, set("channel", 0, new_fx("atomik.nope")), &reg);
    // A return effect cannot be an insert.
    d.apply(
        cc(ConsoleCommand::SetReturn {
            slot: 0,
            effect: Some(key("verb")),
        }),
        &reg,
    )
    .unwrap();
    let e = rejects(&mut d, set("channel", 0, existing("verb")), &reg);
    assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
    // Double membership: this chain at another slot, and another chain.
    d.apply(set("channel", 0, existing("comp")), &reg).unwrap();
    let e = rejects(&mut d, set("channel", 1, existing("comp")), &reg);
    assert!(
        matches!(&e, ModelError::BadConsole(m) if m.contains("MoveInsert")),
        "{e:?}"
    );
    d.apply(
        cc(ConsoleCommand::AddChannel {
            source: key("delay"),
        }),
        &reg,
    )
    .unwrap();
    let other = d.doc().console.channels[1].channel.0.clone();
    let e = rejects(&mut d, set(&other, 0, existing("comp")), &reg);
    assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
    // A channel's own source cannot be one of its inserts.
    let e = rejects(&mut d, set("channel", 1, existing("delay")), &reg);
    assert!(matches!(e, ModelError::BadConsole(_)), "{e:?}");
}

#[test]
fn a_cable_cycle_through_the_chain_is_rejected_and_reported() {
    let reg = console_registry();
    let mut d = rack(&reg);
    // The compressor's sidechain is fed by the channel output (after the insert return):
    // comp as an insert would make comp -> return -> out -> comp, a loop.
    d.apply(connect("channel:out_l", "comp:sc_in_l"), &reg)
        .unwrap();
    let e = rejects(&mut d, set("channel", 0, existing("comp")), &reg);
    assert!(matches!(e, ModelError::BadCable(_)), "{e:?}");
}

#[test]
fn master_has_an_insert_chain_too() {
    let reg = console_registry();
    let mut d = rack(&reg);
    d.apply(set("master", 0, existing("comp")), &reg).unwrap();
    d.apply(set("master", 1, new_fx("atomik.eq")), &reg)
        .unwrap();
    let chain = chain_of(&d, &reg, "master");
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0], "comp");
    assert!(
        d.doc()
            .cables
            .contains(&cable("master:insert_send_l", "comp:in_l"))
    );
}

#[test]
fn the_chain_is_read_back_from_hand_cabled_loops() {
    let reg = console_registry();
    let mut d = rack(&reg);
    for (f, t) in [
        ("channel:insert_send_l", "delay:in_l"),
        ("channel:insert_send_r", "delay:in_r"),
        ("delay:out_l", "eq:in_l"),
        ("delay:out_r", "eq:in_r"),
        ("eq:out_l", "channel:insert_return_l"),
        ("eq:out_r", "channel:insert_return_r"),
    ] {
        d.apply(connect(f, t), &reg).unwrap();
    }
    assert_eq!(chain_of(&d, &reg, "channel"), ["delay", "eq"]);
    d.apply(set("channel", 2, existing("comp")), &reg).unwrap();
    assert_eq!(chain_of(&d, &reg, "channel"), ["delay", "eq", "comp"]);
}

// ------------------------------------------------------------------ bypassing the inserts

fn bypass_of(d: &Document, dev: &str) -> f32 {
    d.doc()
        .device(&key(dev))
        .unwrap()
        .params
        .get("bypass")
        .copied()
        .unwrap_or(0.0)
}

fn two_inserts(reg: &Registry) -> Document {
    let mut d = rack(reg);
    d.apply(set("channel", 0, existing("comp")), reg).unwrap();
    d.apply(set("channel", 1, existing("eq")), reg).unwrap();
    d
}

#[test]
fn a_chain_reports_its_bypass_state_and_nothing_to_bypass_without_effects() {
    let reg = console_registry();
    let mut d = two_inserts(&reg);
    let chain = chain_of(&d, &reg, "channel");
    assert_eq!(chain, ["comp", "eq"]);
    assert_eq!(
        inserts_bypassed(d.doc(), &reg, &key("channel")),
        Some(false)
    );
    d.apply(set_param("comp", "bypass", 1.0), &reg).unwrap();
    assert_eq!(
        inserts_bypassed(d.doc(), &reg, &key("channel")),
        Some(false),
        "one still plays"
    );
    d.apply(set_param("eq", "bypass", 1.0), &reg).unwrap();
    assert_eq!(inserts_bypassed(d.doc(), &reg, &key("channel")), Some(true));
    let states = insert_bypass_states(d.doc(), &reg, &key("channel"));
    assert_eq!(states, [(key("comp"), true), (key("eq"), true)]);
    // A strip without a chain has nothing to bypass.
    let d = rack(&reg);
    assert_eq!(inserts_bypassed(d.doc(), &reg, &key("channel")), None);
    assert!(bypass_inserts_commands(d.doc(), &reg, &key("channel"), true).is_empty());
}

#[test]
fn bypass_all_is_one_undo_step_that_sets_every_effect_and_restores_them() {
    let reg = console_registry();
    let mut d = two_inserts(&reg);
    d.apply(set_param("eq", "bypass", 1.0), &reg).unwrap();
    // Only what changes: the eq is already bypassed.
    let on = bypass_inserts_commands(d.doc(), &reg, &key("channel"), true);
    assert_eq!(on.len(), 1);
    d.apply(Command::Batch(on), &reg).unwrap();
    assert_eq!((bypass_of(&d, "comp"), bypass_of(&d, "eq")), (1.0, 1.0));
    assert!(bypass_inserts_commands(d.doc(), &reg, &key("channel"), true).is_empty());
    let off = bypass_inserts_commands(d.doc(), &reg, &key("channel"), false);
    assert_eq!(off.len(), 2);
    d.apply(Command::Batch(off), &reg).unwrap();
    assert_eq!((bypass_of(&d, "comp"), bypass_of(&d, "eq")), (0.0, 0.0));
    assert_eq!(
        inserts_bypassed(d.doc(), &reg, &key("channel")),
        Some(false)
    );
    // Each batch is one undo step, and redo walks forward again.
    d.undo(&reg).unwrap();
    assert_eq!((bypass_of(&d, "comp"), bypass_of(&d, "eq")), (1.0, 1.0));
    d.undo(&reg).unwrap();
    assert_eq!((bypass_of(&d, "comp"), bypass_of(&d, "eq")), (0.0, 1.0));
    d.redo(&reg).unwrap();
    assert_eq!((bypass_of(&d, "comp"), bypass_of(&d, "eq")), (1.0, 1.0));
}

#[test]
fn the_legacy_strip_bypass_counts_as_bypassed_and_is_cleared_on_restore() {
    let reg = console_registry();
    let mut d = two_inserts(&reg);
    d.apply(set_param("channel", "insert.bypass", 1.0), &reg)
        .unwrap();
    assert_eq!(inserts_bypassed(d.doc(), &reg, &key("channel")), Some(true));
    let restore = bypass_inserts_commands(d.doc(), &reg, &key("channel"), false);
    assert_eq!(
        restore.len(),
        1,
        "the effects are not bypassed, the strip switch is"
    );
    d.apply(Command::Batch(restore), &reg).unwrap();
    assert_eq!(
        inserts_bypassed(d.doc(), &reg, &key("channel")),
        Some(false)
    );
}
