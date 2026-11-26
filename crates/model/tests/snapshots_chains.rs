//! Mixer snapshots that change the effect chain: a snapshot keeps the effects of each insert
//! chain with their state, and a recall adds the ones the rack lacks, deletes the ones the
//! snapshot does not name and updates the ones in both in place, as one undo step; the project
//! file that keeps them (and the older file that does not), and the lane ids past 64.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, Document, InsertEffect, MAX_SNAPSHOTS, SnapshotScope, SyncOp,
    insert_chain, load_project, save_project,
};
use common::console_ops::assert_console_invariants;
use common::plugin::{add_plugin, fx_layout, fx_ref, plugin_console_registry, sync_state};
use common::*;
use serde_json::Value;

/// The starter with four synths and the effect `comp` on the insert of `channel`.
fn project() -> (Document, Registry) {
    let r = plugin_console_registry();
    let mut doc = legacy_project_doc();
    doc.output_channels = 8;
    let mut d = Document::new(doc);
    run(
        &mut d,
        &r,
        ["a", "b", "c", "d"].map(|n| add(usize::MAX, SYNTH, Some(n))),
    );
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, COMP, Some("comp")),
            set_insert("channel", 0, "comp"),
        ],
    );
    (d, r)
}

fn con(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn store(name: &str) -> Command {
    con(ConsoleCommand::StoreSnapshot { name: name.into() })
}

fn recall(id: u32) -> Command {
    con(ConsoleCommand::RecallSnapshot {
        id,
        scope: SnapshotScope::ALL,
    })
}

fn set_insert(strip: &str, slot: usize, effect: &str) -> Command {
    con(ConsoleCommand::SetInsert {
        channel: key(strip),
        slot,
        effect: Some(InsertEffect::Existing(key(effect))),
    })
}

fn add_insert_new(strip: &str, slot: usize, type_id: &str) -> Command {
    con(ConsoleCommand::AddInsert {
        channel: key(strip),
        slot,
        effect: InsertEffect::New {
            type_id: type_id.into(),
        },
    })
}

fn chain(d: &Document, r: &Registry, strip: &str) -> Vec<String> {
    insert_chain(d.doc(), r, &key(strip))
        .into_iter()
        .map(|k| k.0)
        .collect()
}

fn has(d: &Document, k: &str) -> bool {
    d.doc().device(&key(k)).is_some()
}

/// Applies `c` and checks the step: the document is valid, and one undo puts it back exactly
/// as one redo brings it again. Returns the engine events of the step.
fn one_step(d: &mut Document, r: &Registry, c: Command) -> Vec<SyncOp> {
    let before = d.doc().clone();
    let ops = d
        .apply(c.clone(), r)
        .unwrap_or_else(|e| panic!("{c:?} rejected: {e}"));
    let after = d.doc().clone();
    assert_ne!(after, before, "{c:?} changed nothing");
    assert_console_invariants(&after);
    after.validate().expect("valid after");
    d.undo().expect("undo");
    assert_eq!(d.doc(), &before, "{c:?} undone in one step");
    d.redo().expect("redo");
    assert_eq!(d.doc(), &after, "{c:?} redone");
    ops
}

fn value(d: &Document, r: &Registry, dev: &str, param: &str) -> f32 {
    d.doc().param_value(&key(dev), param, r).unwrap()
}

#[test]
fn a_snapshot_keeps_each_effect_of_a_chain_with_its_state() {
    let (mut d, r) = project();
    run(&mut d, &r, [set_param("comp", "bypass", 1.0), store("A")]);
    let wiring = d.doc().console.snapshots[0].state.wiring.clone().unwrap();
    let kept = wiring[&key("channel")]
        .effects
        .clone()
        .expect("a new snapshot holds the effects of the chain");
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].key, key("comp"));
    assert_eq!(kept[0].type_id, COMP);
    assert_eq!(kept[0].params.get("bypass"), Some(&1.0));
    assert_eq!(wiring[&key("channel-2")].effects, Some(Vec::new()));
}

#[test]
fn a_recall_adds_back_an_effect_the_rack_lacks_in_one_undo_step() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            set_param("comp", "bypass", 1.0),
            set_param("comp", "threshold", -33.0),
            store("A"),
            Command::RemoveDevice { key: key("comp") },
        ],
    );
    assert!(chain(&d, &r, "channel").is_empty());
    let ops = one_step(&mut d, &r, recall(1));
    assert_eq!(chain(&d, &r, "channel"), ["comp"], "back in the chain");
    assert_eq!(value(&d, &r, "comp", "bypass"), 1.0, "with its state");
    assert_eq!(value(&d, &r, "comp", "threshold"), -33.0);
    assert!(ops.contains(&SyncOp::DeviceAdded(key("comp"))));
    assert!(d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
}

#[test]
fn a_recall_adds_an_effect_at_its_place_in_the_chain_and_stacks_it_under_its_strip() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            add_insert_new("channel", 1, DELAY),
            add(usize::MAX, COMP, Some("last")),
            set_insert("channel", 2, "last"),
            store("A"),
        ],
    );
    let before = chain(&d, &r, "channel");
    assert_eq!(before.len(), 3);
    let middle = before[1].clone();
    run(
        &mut d,
        &r,
        [
            con(ConsoleCommand::SetInsert {
                channel: key("channel"),
                slot: 1,
                effect: None,
            }),
            Command::RemoveDevice { key: key(&middle) },
        ],
    );
    assert_eq!(chain(&d, &r, "channel").len(), 2);
    one_step(&mut d, &r, recall(1));
    assert_eq!(chain(&d, &r, "channel"), before, "the same order");
    let order: Vec<String> = d.doc().devices.iter().map(|x| x.key.0.clone()).collect();
    let at = order.iter().position(|k| k == "channel").unwrap();
    assert_eq!(order[at + 1..at + 4], before[..], "stacked under the strip");
}

#[test]
fn a_recall_deletes_the_effects_the_snapshot_does_not_name() {
    let (mut d, r) = project();
    run(&mut d, &r, [store("A")]);
    run(&mut d, &r, [add_insert_new("channel", 1, DELAY)]);
    let extra = chain(&d, &r, "channel")[1].clone();
    run(
        &mut d,
        &r,
        [
            add_insert_new("channel-2", 0, DELAY),
            set_param("channel", "fader", 0.3),
        ],
    );
    let on_two = chain(&d, &r, "channel-2")[0].clone();
    let ops = one_step(&mut d, &r, recall(1));
    assert_eq!(chain(&d, &r, "channel"), ["comp"]);
    assert!(chain(&d, &r, "channel-2").is_empty());
    assert!(!has(&d, &extra) && !has(&d, &on_two), "they are deleted");
    assert!(ops.contains(&SyncOp::DeviceRemoved(key(&extra))));
    assert!(d.doc().snapshot_matches(1, &r, SnapshotScope::ALL));
}

#[test]
fn an_effect_in_both_is_the_same_instance_and_its_params_are_updated_in_place() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            store("A"),
            set_param("comp", "threshold", -40.0),
            store("B"),
            // With the parameters of the effects (the per-snapshot option).
            con(ConsoleCommand::SetSnapshotEffects {
                id: 2,
                effects: true,
            }),
            set_param("comp", "threshold", -10.0),
            // An effect only the working mix has.
            add_insert_new("channel", 1, DELAY),
        ],
    );
    let index = d.doc().devices.iter().position(|x| x.key == key("comp"));
    let ops = one_step(&mut d, &r, recall(2));
    assert_eq!(chain(&d, &r, "channel"), ["comp"]);
    assert_eq!(value(&d, &r, "comp", "threshold"), -40.0, "updated");
    assert_eq!(
        d.doc().devices.iter().position(|x| x.key == key("comp")),
        index,
        "not recreated, not moved"
    );
    assert!(
        !ops.contains(&SyncOp::DeviceAdded(key("comp")))
            && !ops.contains(&SyncOp::DeviceRemoved(key("comp"))),
        "{ops:?}"
    );
    assert!(ops.iter().any(|o| matches!(
        o,
        SyncOp::ParamChanged { key: k, param, .. } if k.0 == "comp" && param == "threshold"
    )));
}

#[test]
fn an_effect_that_moves_to_another_strip_is_moved_not_deleted() {
    let (mut d, r) = project();
    run(&mut d, &r, [store("A")]);
    run(
        &mut d,
        &r,
        [
            con(ConsoleCommand::ClearInserts {
                strip: key("channel"),
            }),
            set_insert("channel-2", 0, "comp"),
            store("B"),
            con(ConsoleCommand::ClearInserts {
                strip: key("channel-2"),
            }),
            set_insert("channel", 0, "comp"),
        ],
    );
    let ops = one_step(&mut d, &r, recall(2));
    assert_eq!(chain(&d, &r, "channel-2"), ["comp"]);
    assert!(chain(&d, &r, "channel").is_empty());
    assert!(
        !ops.contains(&SyncOp::DeviceRemoved(key("comp"))),
        "the same instance moved"
    );
}

#[test]
fn a_snapshot_of_older_keys_only_moves_cables_and_never_adds_or_removes() {
    let (mut d, r) = project();
    run(&mut d, &r, [store("A")]);
    let text = save_project(d.doc());
    let mut json: Value = serde_json::from_str(&text).unwrap();
    // The file of a build that kept the keys of the chain only.
    for strip in json["console"]["snapshots"][0]["wiring"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        strip.as_object_mut().unwrap().remove("effects");
    }
    let old = serde_json::to_string(&json).unwrap();
    let mut d = Document::new(load_project(&old, &r).unwrap());
    assert!(
        d.doc().console.snapshots[0].state.wiring.as_ref().unwrap()[&key("channel")]
            .effects
            .is_none()
    );
    run(&mut d, &r, [Command::RemoveDevice { key: key("comp") }]);
    run(&mut d, &r, [add_insert_new("channel", 0, DELAY)]);
    let extra = chain(&d, &r, "channel")[0].clone();
    let _ = d.apply(recall(1), &r);
    assert!(!has(&d, "comp"), "no state to add it back from");
    assert!(has(&d, &extra), "and nothing is deleted");
    assert_console_invariants(d.doc());
}

#[test]
fn an_effect_whose_key_another_device_took_is_skipped() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [store("A"), Command::RemoveDevice { key: key("comp") }],
    );
    // The key is free again for another device, which is not on the chain.
    run(&mut d, &r, [add(usize::MAX, DELAY, Some("comp"))]);
    let _ = d.apply(recall(1), &r);
    assert_eq!(d.doc().device(&key("comp")).unwrap().type_id, DELAY);
    assert!(chain(&d, &r, "channel").is_empty());
    assert_console_invariants(d.doc());
}

#[test]
fn a_plugin_effect_comes_back_with_its_saved_state_and_layout() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            add_plugin(usize::MAX, &fx_ref(), &fx_layout(), Some("verb")),
            set_insert("channel", 1, "verb"),
            sync_state("verb", b"saved-state"),
            store("A"),
        ],
    );
    run(&mut d, &r, [Command::RemoveDevice { key: key("verb") }]);
    let ops = one_step(&mut d, &r, recall(1));
    assert_eq!(chain(&d, &r, "channel"), ["comp", "verb"]);
    let plugin = d
        .doc()
        .device(&key("verb"))
        .unwrap()
        .plugin
        .clone()
        .unwrap();
    assert_eq!(plugin.state.as_ref().unwrap().0, b"saved-state");
    assert_eq!(plugin.layout, fx_layout());
    assert!(ops.contains(&SyncOp::DeviceAdded(key("verb"))));
}

#[test]
fn the_effects_round_trip_in_the_project_file_and_an_old_reader_skips_them() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            add_plugin(usize::MAX, &fx_ref(), &fx_layout(), Some("verb")),
            set_insert("channel", 1, "verb"),
            sync_state("verb", b"state"),
            set_param("comp", "bypass", 1.0),
            store("A"),
        ],
    );
    let text = save_project(d.doc());
    let back = load_project(&text, &r).unwrap();
    assert_eq!(back.console.snapshots, d.doc().console.snapshots);
    assert_eq!(save_project(&back), text, "a stable text");
    // The keys stay where an older reader looks, the states are one more key of the entry.
    let json: Value = serde_json::from_str(&text).unwrap();
    let entry = &json["console"]["snapshots"][0]["wiring"]["channel"];
    assert_eq!(entry["inserts"], serde_json::json!(["comp", "verb"]));
    assert_eq!(entry["effects"].as_array().unwrap().len(), 2);
    assert_eq!(json["version"], atomik_model::PROJECT_VERSION);
    assert!(json.get("ignorable").is_none_or(|i| {
        !i.to_string().contains("effects") && !i.to_string().contains("wiring")
    }));
}

#[test]
fn a_plugin_that_is_not_installed_keeps_its_state_through_a_load_and_a_save() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            add_plugin(usize::MAX, &fx_ref(), &fx_layout(), Some("verb")),
            set_insert("channel", 1, "verb"),
            sync_state("verb", b"state"),
            store("A"),
            Command::RemoveDevice { key: key("verb") },
        ],
    );
    let text = save_project(d.doc());
    // A machine without the plugin: the project loads, and the snapshot keeps what it held.
    let bare = console_registry();
    let doc = load_project(&text, &bare).unwrap();
    assert_eq!(doc.console.snapshots, d.doc().console.snapshots);
    assert_eq!(save_project(&doc), text);
}

#[test]
fn a_state_that_does_not_match_its_keys_is_dropped_not_refused() {
    let (mut d, r) = project();
    run(&mut d, &r, [store("A")]);
    let mut json: Value = serde_json::from_str(&save_project(d.doc())).unwrap();
    json["console"]["snapshots"][0]["wiring"]["channel"]["effects"] = serde_json::json!([]);
    let doc = load_project(&json.to_string(), &r).unwrap();
    let w = doc.console.snapshots[0].state.wiring.as_ref().unwrap();
    assert!(w[&key("channel")].effects.is_none());
    assert_eq!(w[&key("channel")].inserts, [key("comp")]);
}

#[test]
fn a_compare_flip_adds_and_removes_the_effects_and_loses_nothing() {
    let (mut d, r) = project();
    run(&mut d, &r, [store("A")]);
    run(&mut d, &r, [add_insert_new("channel", 1, DELAY)]);
    let working = chain(&d, &r, "channel");
    assert_eq!(working.len(), 2);
    one_step(&mut d, &r, con(ConsoleCommand::Compare { id: 1 }));
    assert_eq!(chain(&d, &r, "channel"), ["comp"], "the snapshot's side");
    run(&mut d, &r, [con(ConsoleCommand::Compare { id: 1 })]);
    assert_eq!(
        chain(&d, &r, "channel"),
        working,
        "the working side is back"
    );
}

#[test]
fn a_snapshot_id_past_64_is_kept_and_named_by_a_lane_point() {
    let (mut d, r) = project();
    // Store and delete snapshots so the ids climb: an id is not reused while another is higher.
    for n in 0..MAX_SNAPSHOTS + 6 {
        let name = format!("S{n}");
        run(&mut d, &r, [store(&name)]);
        if d.doc().console.snapshots.len() == MAX_SNAPSHOTS {
            let first = d.doc().console.snapshots[0].id;
            run(
                &mut d,
                &r,
                [con(ConsoleCommand::DeleteSnapshot { id: first })],
            );
        }
    }
    let top = d
        .doc()
        .console
        .snapshots
        .iter()
        .map(|s| s.id)
        .max()
        .unwrap();
    assert!(top > 64, "{top}");
    assert_eq!(atomik_model::lane_snapshot(top as f32), top);
    assert_eq!(d.doc().console.next_snapshot_id(), top + 1);
    // The file keeps them.
    let back = load_project(&save_project(d.doc()), &r).unwrap();
    assert_eq!(
        back.console.snapshots.len(),
        d.doc().console.snapshots.len()
    );
}

#[test]
fn a_new_snapshot_never_takes_an_id_past_what_a_lane_can_name() {
    let mut console = atomik_model::Console::default();
    console.snapshots.push(atomik_model::MixerSnapshot {
        id: atomik_model::MAX_LANE_SNAPSHOT_ID,
        name: "Last".into(),
        state: atomik_model::MixerState::default(),
        note: String::new(),
        color: None,
        marker: None,
    });
    assert_eq!(console.next_snapshot_id(), 1, "the lowest free id");
}
