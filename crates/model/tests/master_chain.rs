//! The master chain item: the master's insert effects and send effects saved as a library item
//! and loaded back, replacing what the master has, in one undo step.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateSource, CreateTarget, DeviceKey, Document,
    InsertEffect, RackDoc, WorkflowCommand, capture_master_chain, insert_chain,
    is_master_chain_item, load_master_chain_item, master_stack, new_project_doc,
    save_master_chain_item, send_chain,
};
use common::plugin::*;
use common::workflow::*;
use common::*;

const REVERB: &str = "atomik.reverb";
const EQ: &str = "atomik.eq";
const DISTORTION: &str = "atomik.distortion";

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn rack(r: &Registry) -> (Document, DeviceKey) {
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        r,
        [wf(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key("synth")),
            options: CreateOptions::default(),
        })],
    );
    let master = d.doc().console.master.clone().expect("a master");
    (d, master)
}

fn insert(master: &DeviceKey, slot: usize, t: &str) -> Command {
    cc(ConsoleCommand::AddInsert {
        channel: master.clone(),
        slot,
        effect: InsertEffect::New { type_id: t.into() },
    })
}

fn send(t: &str) -> Command {
    wf(WorkflowCommand::CreateEffect {
        source: CreateSource::Type(t.into()),
        target: CreateTarget::Master,
        key: None,
        options: CreateOptions::default(),
    })
}

fn series(after: &DeviceKey, t: &str) -> Command {
    wf(WorkflowCommand::CreateEffect {
        source: CreateSource::Type(t.into()),
        target: CreateTarget::Selected(after.clone()),
        key: None,
        options: CreateOptions::default(),
    })
}

/// The master with two inserts (the first bypassed and tuned), a send with two effects in
/// series and a second send, and return levels set.
fn chained(r: &Registry) -> (Document, DeviceKey) {
    let (mut d, master) = rack(r);
    run(
        &mut d,
        r,
        [insert(&master, 0, EQ), insert(&master, 1, DISTORTION)],
    );
    let first = insert_chain(d.doc(), r, &master)[0].clone();
    run(
        &mut d,
        r,
        [
            set_param(&first.0, "bypass", 1.0),
            Command::Rename {
                key: first.clone(),
                name: "Tone".into(),
            },
        ],
    );
    run(&mut d, r, [send(REVERB), send(DISTORTION)]);
    let one = send_chain(d.doc(), r, 0)[0].clone();
    run(&mut d, r, [series(&one, EQ)]);
    run(
        &mut d,
        r,
        [
            set_param(&master.0, "return1.level", 0.25),
            set_param(&master.0, "return2.pan", 0.5),
        ],
    );
    (d, master)
}

/// What a chain is, independent of device keys: per device the type, name, params and bypass,
/// in the item's order, plus the send slots and the return params.
fn shape(doc: &RackDoc, r: &Registry) -> Vec<String> {
    let chain = capture_master_chain(doc, r).expect("a master");
    let mut out: Vec<String> = chain
        .snapshot
        .devices
        .iter()
        .map(|d| format!("{} {} {:?}", d.type_id, d.name, d.params))
        .collect();
    out.push(format!("inserts {}", chain.inserts));
    out.push(format!(
        "slots {:?}",
        (0..8)
            .map(|s| send_chain(doc, r, s).len())
            .collect::<Vec<_>>()
    ));
    out.push(format!("returns {:?}", chain.return_params));
    out
}

fn assert_stacked(d: &Document, r: &Registry, master: &DeviceKey) {
    let all: Vec<DeviceKey> = d.doc().devices.iter().map(|x| x.key.clone()).collect();
    let at = all.iter().position(|k| k == master).unwrap();
    let stack = master_stack(d.doc(), r);
    assert_eq!(&all[at + 1..at + 1 + stack.len()], &stack[..]);
}

#[test]
fn saving_then_loading_round_trips_order_params_bypass_sends_and_levels() {
    let mut r = console_registry();
    let (d, _) = chained(&r);
    let chain = capture_master_chain(d.doc(), &r).unwrap();
    assert_eq!(chain.inserts, 2);
    assert_eq!(chain.send_count(), 2);
    let text = save_master_chain_item(&chain, d.doc()).unwrap();
    assert!(is_master_chain_item(&text));
    let back = load_master_chain_item(&text, &mut r).unwrap();
    assert_eq!(back.type_ids(), chain.type_ids());
    assert_eq!(back.return_params, chain.return_params);
    assert_eq!(back.inserts, chain.inserts);
    assert_eq!(
        back.snapshot
            .devices
            .iter()
            .map(|x| (&x.name, &x.params))
            .collect::<Vec<_>>(),
        chain
            .snapshot
            .devices
            .iter()
            .map(|x| (&x.name, &x.params))
            .collect::<Vec<_>>()
    );
    // The same chain gives the same bytes.
    assert_eq!(save_master_chain_item(&back, d.doc()).unwrap(), text);
    // Into a fresh project: the same master section.
    let (mut fresh, master) = rack(&r);
    run(
        &mut fresh,
        &r,
        [wf(WorkflowCommand::ApplyMasterChain {
            chain: Box::new(back),
        })],
    );
    assert_eq!(shape(fresh.doc(), &r), shape(d.doc(), &r));
    assert_stacked(&fresh, &r, &master);
    let tone = insert_chain(fresh.doc(), &r, &master)[0].clone();
    let dev = fresh.doc().device(&tone).unwrap();
    assert_eq!(dev.name, "Tone");
    assert_eq!(dev.params.get("bypass"), Some(&1.0));
    assert_eq!(send_chain(fresh.doc(), &r, 0).len(), 2);
    assert_eq!(send_chain(fresh.doc(), &r, 1).len(), 1);
    assert!(send_chain(fresh.doc(), &r, 2).is_empty());
    assert_eq!(
        fresh
            .doc()
            .device(&master)
            .unwrap()
            .params
            .get("return1.level"),
        Some(&0.25)
    );
}

#[test]
fn loading_replaces_the_masters_effects_and_undo_restores_them() {
    let r = console_registry();
    let (source, _) = chained(&r);
    let chain = capture_master_chain(source.doc(), &r).unwrap();
    // A master with other effects and other levels.
    let (mut d, master) = rack(&r);
    run(
        &mut d,
        &r,
        [insert(&master, 0, REVERB), send(EQ), send(EQ), send(EQ)],
    );
    run(&mut d, &r, [set_param(&master.0, "return3.level", -20.0)]);
    let before = text(&d);
    let old: Vec<DeviceKey> = master_stack(d.doc(), &r);
    assert_eq!(old.len(), 4);
    let devices = d.doc().devices.len();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::ApplyMasterChain {
            chain: Box::new(chain),
        }),
    );
    // The old effects are gone with their cables (their keys may come up again for the new
    // ones), the item's five are there.
    assert_eq!(d.doc().devices.len(), devices - old.len() + 5);
    let there = |k: &DeviceKey| d.doc().device(k).is_some() || k.0 == atomik_model::INTERFACE_KEY;
    assert!(
        d.doc()
            .cables
            .iter()
            .all(|c| there(&c.from.device) && there(&c.to.device))
    );
    assert_eq!(shape(d.doc(), &r), shape(source.doc(), &r));
    assert!(d.doc().console.returns[2].is_none());
    assert!(
        !d.doc()
            .device(&master)
            .unwrap()
            .params
            .contains_key("return3.level"),
        "a return the item does not set goes back to its default"
    );
    assert_stacked(&d, &r, &master);
    // One undo gives the first master back, bit for bit.
    d.undo().unwrap();
    assert_eq!(text(&d), before);
}

#[test]
fn an_item_with_eight_sends_replaces_all_eight_slots() {
    let r = console_registry();
    let (mut source, _) = rack(&r);
    for _ in 0..8 {
        run(&mut source, &r, [send(REVERB)]);
    }
    let chain = capture_master_chain(source.doc(), &r).unwrap();
    assert_eq!(chain.send_count(), 8);
    let (mut d, master) = rack(&r);
    run(&mut d, &r, [send(EQ), send(EQ), insert(&master, 0, EQ)]);
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::ApplyMasterChain {
            chain: Box::new(chain),
        })],
    );
    assert!(d.doc().console.returns.iter().all(Option::is_some));
    assert!(insert_chain(d.doc(), &r, &master).is_empty());
    assert_eq!(
        d.doc()
            .devices
            .iter()
            .filter(|x| x.type_id == REVERB)
            .count(),
        8
    );
    assert_stacked(&d, &r, &master);
}

#[test]
fn an_empty_master_chain_clears_the_master() {
    let mut r = console_registry();
    let (source, _) = rack(&r);
    let chain = capture_master_chain(source.doc(), &r).unwrap();
    assert!(chain.snapshot.devices.is_empty());
    let text = save_master_chain_item(&chain, source.doc()).unwrap();
    let back = load_master_chain_item(&text, &mut r).unwrap();
    let (mut d, _) = chained(&r);
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::ApplyMasterChain {
            chain: Box::new(back),
        }),
    );
    assert!(master_stack(d.doc(), &r).is_empty());
    assert!(d.doc().console.returns.iter().all(Option::is_none));
}

#[test]
fn a_document_without_a_master_rejects_the_chain_and_changes_nothing() {
    let r = console_registry();
    let (source, _) = chained(&r);
    let chain = capture_master_chain(source.doc(), &r).unwrap();
    let mut bare = Document::new(atomik_model::empty_doc());
    rejected(
        &mut bare,
        &r,
        wf(WorkflowCommand::ApplyMasterChain {
            chain: Box::new(chain),
        }),
    );
    assert!(capture_master_chain(bare.doc(), &r).is_err());
}

#[test]
fn an_ordinary_patch_is_not_a_master_chain() {
    let mut r = console_registry();
    let (d, master) = chained(&r);
    let keys = vec![insert_chain(d.doc(), &r, &master)[0].clone()];
    let snapshot = atomik_model::DeviceSnapshot::capture(d.doc(), &keys, false).unwrap();
    let text = atomik_model::save_device_item(&snapshot, d.doc());
    assert!(!is_master_chain_item(&text));
    assert!(load_master_chain_item(&text, &mut r).is_err());
    assert!(!is_master_chain_item("not json"));
}

#[test]
fn plugin_effects_keep_their_state_through_the_item() {
    let mut r = plugin_console_registry();
    let (mut d, master) = rack(&r);
    run(
        &mut d,
        &r,
        [
            add_plugin(usize::MAX, &fx_ref(), &fx_layout(), Some("verb")),
            add_plugin(usize::MAX, &fx_ref(), &fx_layout(), Some("echo")),
            cc(ConsoleCommand::AddInsert {
                channel: master.clone(),
                slot: 0,
                effect: InsertEffect::Existing(key("verb")),
            }),
            cc(ConsoleCommand::SetReturn {
                slot: 0,
                effect: Some(key("echo")),
            }),
            sync_state("verb", b"insert state"),
            sync_state("echo", b"send state"),
        ],
    );
    let chain = capture_master_chain(d.doc(), &r).unwrap();
    assert_eq!(chain.type_ids().len(), 2);
    let item = save_master_chain_item(&chain, d.doc()).unwrap();
    // A machine that does not have the plugin still reads the item (as a stand-in).
    let mut bare = console_registry();
    let stand_in = load_master_chain_item(&item, &mut bare).expect("a stand-in is registered");
    assert_eq!(
        stand_in.snapshot.devices.len(),
        2,
        "both plugin effects are on the chain"
    );
    assert_eq!(stand_in.send_count(), 1);
    let back = load_master_chain_item(&item, &mut r).unwrap();
    let (mut other, _) = rack(&r);
    run(
        &mut other,
        &r,
        [wf(WorkflowCommand::ApplyMasterChain {
            chain: Box::new(back),
        })],
    );
    let m = other.doc().console.master.clone().unwrap();
    let ins = insert_chain(other.doc(), &r, &m);
    let snd = send_chain(other.doc(), &r, 0);
    let state = |k: &DeviceKey| {
        other
            .doc()
            .device(k)
            .and_then(|x| x.plugin.as_ref())
            .and_then(|p| p.state.clone())
    };
    assert_eq!(
        state(&ins[0]).map(|s| s.0.clone()),
        Some(b"insert state".to_vec())
    );
    assert_eq!(
        state(&snd[0]).map(|s| s.0.clone()),
        Some(b"send state".to_vec())
    );
}

#[test]
fn the_browser_summary_of_a_master_chain_item_counts_effects_and_sends() {
    let r = console_registry();
    let (d, _) = chained(&r);
    let chain = capture_master_chain(d.doc(), &r).unwrap();
    let text = save_master_chain_item(&chain, d.doc()).unwrap();
    let s = atomik_model::summarize_item(atomik_model::ItemKind::Patch, &text).unwrap();
    assert_eq!(s.detail, "Master chain: 5 effects, 2 sends");
    assert_eq!(s.device_type, None, "it is no device patch");
}
