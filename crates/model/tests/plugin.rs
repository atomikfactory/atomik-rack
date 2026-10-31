//! Plugin devices in the document model:
//! `PluginCommand` rules and undo steps, auto channels, insert chains, containers,
//! params and gesture merging, `SyncState`, pins, power, relink, batches and the expansion.

mod common;

use atomik_device_sdk::{PluginKindHint, PluginStateBlob};
use atomik_model::{
    Command, ConsoleCommand, ContainerCommand, Document, InsertEffect, MAX_PINNED_PARAMS,
    MAX_PLUGIN_STATE_BYTES, ModelError, PLUGIN_GESTURE_MERGE_BASE, PluginCommand, SongCommand,
    SyncOp, empty_doc,
};
use common::plugin::*;
use common::*;

fn rejects(d: &mut Document, cmd: Command, reg: &atomik_device_sdk::Registry) -> ModelError {
    let before = d.doc().clone();
    let can_undo = d.can_undo();
    let can_redo = d.can_redo();
    let e = d.apply(cmd, reg).expect_err("must be rejected");
    assert_eq!(d.doc(), &before, "nothing changes on rejection");
    assert_eq!(d.can_undo(), can_undo);
    assert_eq!(d.can_redo(), can_redo);
    e
}

fn bad_plugin(e: &ModelError) -> &str {
    match e {
        ModelError::BadPlugin(m) => m,
        other => panic!("expected BadPlugin, got {other:?}"),
    }
}

fn undo_all(d: &mut Document, reg: &atomik_device_sdk::Registry) -> usize {
    let mut n = 0;
    while d.undo(reg).is_some() {
        n += 1;
    }
    n
}

// ---- constants -------------------------------------------------------------------------

#[test]
fn the_constants_match_the_plan() {
    assert_eq!(PLUGIN_GESTURE_MERGE_BASE, 8 << 40);
    let base = std::hint::black_box(PLUGIN_GESTURE_MERGE_BASE);
    assert!(base > 7 << 40, "clear of the macro drags");
    assert_eq!(MAX_PLUGIN_STATE_BYTES, 16 * 1024 * 1024);
    assert_eq!(MAX_PINNED_PARAMS, 16);
}

// ---- add ---------------------------------------------------------------------------------

#[test]
fn add_builds_the_plugin_doc_and_one_undo_step() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    let ops = d
        .apply(add_plugin(0, &synth_ref(), &synth_layout(), None), &r)
        .unwrap();
    assert_eq!(ops[0], SyncOp::DeviceAdded(key("test-synth")));
    assert_eq!(*ops.last().unwrap(), SyncOp::RoutingChanged);
    let dev = &d.doc().devices[0];
    assert_eq!(dev.type_id, CLAP_SYNTH);
    assert_eq!(dev.name, "Test Synth");
    assert!(dev.params.is_empty(), "params are stored sparsely");
    let p = plugin_of(d.doc(), "test-synth");
    assert_eq!(p.format, atomik_device_sdk::PluginFormat::Clap);
    assert_eq!(p.id, "test.synth");
    assert_eq!(p.layout, synth_layout());
    assert_eq!(p.name(), "Test Synth");
    assert_eq!(p.vendor(), "Atomik Test");
    assert_eq!(p.version(), "1.2.3");
    assert_eq!(p.path_hint, None);
    assert!(p.state.is_none());
    assert!(p.pinned.is_empty());
    assert!(!p.run_on_audio_thread);
    assert!(p.powered);
    assert_eq!(p.type_id(), CLAP_SYNTH);

    assert_eq!(undo_all(&mut d, &r), 1, "one undo step");
    assert!(d.doc().devices.is_empty());
    d.redo(&r).unwrap();
    assert_eq!(d.doc().devices[0].type_id, CLAP_SYNTH);
}

#[test]
fn add_keeps_the_path_hint_and_generates_keys_and_names_from_the_plugin_name() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    let mut add = add_plugin(0, &synth_ref(), &synth_layout(), None);
    if let Command::Plugin(PluginCommand::Add { path_hint, .. }) = &mut add {
        *path_hint = Some("C:/CLAP/test.clap".into());
    }
    d.apply(add, &r).unwrap();
    run(
        &mut d,
        &r,
        [
            add_plugin(1, &synth_ref(), &synth_layout(), None),
            add_plugin(2, &vst3_ref(), &vst3_layout(), None),
        ],
    );
    assert_eq!(keys(&d), ["test-synth", "test-synth-2", "vee-fx"]);
    assert_eq!(d.doc().devices[1].name, "Test Synth 2");
    assert_eq!(
        plugin_of(d.doc(), "test-synth").path_hint.as_deref(),
        Some("C:/CLAP/test.clap")
    );
    assert_eq!(d.doc().devices[2].type_id, VST3_FX);
}

#[test]
fn add_clamps_the_index() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add(0, SYNTH, Some("a"))]);
    d.apply(add_plugin(99, &fx_ref(), &fx_layout(), Some("f")), &r)
        .unwrap();
    assert_eq!(keys(&d), ["a", "f"]);
}

#[test]
fn add_refuses_an_unregistered_plugin_a_bad_key_and_a_bad_layout() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    let e = rejects(
        &mut d,
        add_plugin(0, &clap_ref("nope"), &fx_layout(), None),
        &r,
    );
    assert_eq!(e, ModelError::UnknownType("clap.nope".into()));
    let e = rejects(
        &mut d,
        add_plugin(0, &fx_ref(), &fx_layout(), Some("Bad Key")),
        &r,
    );
    assert!(matches!(e, ModelError::BadKey(_)));
    // A layout that cannot be built into a descriptor.
    let mut huge = fx_layout();
    huge.params = (0..3000).map(|i| param(i, "p", 0.0, 1.0, 0.0)).collect();
    let e = rejects(&mut d, add_plugin(0, &fx_ref(), &huge, None), &r);
    assert!(!bad_plugin(&e).is_empty());
    // The key must be free.
    run(
        &mut d,
        &r,
        [add_plugin(0, &fx_ref(), &fx_layout(), Some("f"))],
    );
    let e = rejects(
        &mut d,
        add_plugin(1, &fx_ref(), &fx_layout(), Some("f")),
        &r,
    );
    assert_eq!(e, ModelError::BadKey("f".into()));
}

#[test]
fn generic_add_device_and_add_inner_refuse_plugin_types() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    let e = rejects(&mut d, add(0, CLAP_SYNTH, None), &r);
    assert!(bad_plugin(&e).contains("PluginCommand::Add"), "{e}");
    run(&mut d, &r, [add(0, CONTAINER, Some("c"))]);
    let e = rejects(&mut d, add_inner("c", 0, CLAP_FX, "x"), &r);
    assert!(bad_plugin(&e).contains("PluginCommand::Add"), "{e}");
}

#[test]
fn an_instrument_plugin_gets_a_console_channel_in_the_same_step() {
    let r = plugin_console_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add_plugin(0, &synth_ref(), &synth_layout(), Some("s")), &r)
        .unwrap();
    let c = &d.doc().console;
    assert_eq!(c.channels.len(), 1);
    assert_eq!(c.channels[0].source, Some(key("s")));
    assert!(c.master.is_some());
    assert_eq!(undo_all(&mut d, &r), 1, "channel and device are one step");
    assert!(d.doc().devices.is_empty());
    assert!(d.doc().console.channels.is_empty());
}

#[test]
fn effect_and_note_effect_plugins_get_no_channel() {
    let r = plugin_console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &fx_ref(), &fx_layout(), Some("f")),
            add_plugin(1, &notes_ref(), &notes_layout(), Some("n")),
        ],
    );
    assert!(d.doc().console.channels.is_empty());
    assert_eq!(
        r.get(CLAP_NOTES).unwrap().descriptor.kind,
        atomik_device_sdk::DeviceKind::Utility
    );
    assert_eq!(notes_layout().kind_hint, PluginKindHint::NoteEffect);
}

#[test]
fn removing_a_plugin_instrument_removes_its_channel_and_undo_brings_state_back() {
    let r = plugin_console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add_plugin(0, &synth_ref(), &synth_layout(), Some("s"))],
    );
    run(&mut d, &r, [sync_state("s", b"patch one")]);
    d.apply(Command::RemoveDevice { key: key("s") }, &r)
        .unwrap();
    assert!(d.doc().devices.is_empty() || d.doc().device(&key("s")).is_none());
    assert!(d.doc().console.channels.is_empty());
    d.undo(&r).unwrap();
    assert_eq!(
        plugin_of(d.doc(), "s").state.as_deref(),
        Some(&PluginStateBlob(b"patch one".to_vec()))
    );
    assert_eq!(d.doc().console.channels.len(), 1);
}

// ---- insert chains and returns ---------------------------------------------------------------

#[test]
fn a_plugin_effect_works_in_an_insert_chain_and_on_a_return() {
    let r = plugin_console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &synth_ref(), &synth_layout(), Some("s")),
            add_plugin(1, &fx_ref(), &fx_layout(), Some("f")),
            add_plugin(2, &vst3_ref(), &vst3_layout(), Some("v")),
        ],
    );
    let channel = d.doc().console.channels[0].channel.0.clone();
    run(
        &mut d,
        &r,
        [Command::Console(ConsoleCommand::SetInsert {
            channel: key(&channel),
            slot: 0,
            effect: Some(InsertEffect::Existing(key("f"))),
        })],
    );
    assert_eq!(
        atomik_model::insert_chain(d.doc(), &r, &key(&channel)),
        [key("f")]
    );
    run(
        &mut d,
        &r,
        [Command::Console(ConsoleCommand::SetReturn {
            slot: 0,
            effect: Some(key("v")),
        })],
    );
    assert_eq!(d.doc().console.returns[0], Some(key("v")));
}

#[test]
fn a_plugin_cannot_be_created_through_an_insert_slot() {
    let r = plugin_console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [add_plugin(0, &synth_ref(), &synth_layout(), Some("s"))],
    );
    let channel = d.doc().console.channels[0].channel.0.clone();
    let e = rejects(
        &mut d,
        Command::Console(ConsoleCommand::SetInsert {
            channel: key(&channel),
            slot: 0,
            effect: Some(InsertEffect::New {
                type_id: CLAP_FX.into(),
            }),
        }),
        &r,
    );
    assert!(bad_plugin(&e).contains("PluginCommand::Add"), "{e}");
}

#[test]
fn cables_between_plugin_devices_obey_the_graph_rules() {
    let r = plugin_registry();
    let mut d = plugin_chain(&r);
    run(
        &mut d,
        &r,
        [add_plugin(2, &fx_ref(), &fx_layout(), Some("f2"))],
    );
    run(
        &mut d,
        &r,
        [
            connect("fx:out_l", "f2:in_l"),
            connect("fx:out_r", "f2:in_r"),
            connect("synth:out_l", "f2:sc_in_l"),
        ],
    );
    // Feedback is refused, and so is a cable to an output.
    let e = rejects(&mut d, connect("f2:out_l", "fx:sc_in_l"), &r);
    assert!(matches!(e, ModelError::BadCable(m) if m.contains("cycle")));
    let e = rejects(&mut d, connect("synth:out_l", "f2:out_l"), &r);
    assert!(matches!(e, ModelError::BadCable(_)));
    // A socket the plugin does not have.
    let e = rejects(&mut d, connect("synth:out_l", "synth:sc_in_l"), &r);
    assert!(matches!(
        e,
        ModelError::UnknownKey(_) | ModelError::BadCable(_)
    ));
}

// ---- params, gestures and lanes ----------------------------------------------------------------

#[test]
fn params_use_the_p_id_keys_and_are_stored_sparsely() {
    let r = plugin_registry();
    let mut d = plugin_chain(&r);
    run(&mut d, &r, [set_param("synth", "p10", 0.7)]);
    let dev = d.doc().device(&key("synth")).unwrap();
    assert_eq!(dev.params.len(), 1, "only keys set by SetParam");
    assert_eq!(dev.params["p10"], 0.7);
    run(&mut d, &r, [set_param("synth", "p10", 5.0)]);
    assert_eq!(d.doc().device(&key("synth")).unwrap().params["p10"], 1.0);
    run(&mut d, &r, [set_param("synth", "bypass", 1.0)]);
    assert_eq!(d.doc().device(&key("synth")).unwrap().params["bypass"], 1.0);
    // A param the plugin does not have is unknown.
    let before = d.doc().clone();
    assert!(matches!(
        d.apply(set_param("synth", "p99", 0.5), &r),
        Err(ModelError::UnknownKey(_))
    ));
    assert_eq!(d.doc(), &before);
    // VST3 stepped params are indices.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &vst3_ref(), &vst3_layout(), Some("v")),
            set_param("v", "p7", 9.0),
        ],
    );
    assert_eq!(d.doc().device(&key("v")).unwrap().params["p7"], 4.0);
}

#[test]
fn a_gesture_is_one_undo_step() {
    let r = plugin_registry();
    let mut d = plugin_chain(&r);
    let before = d.doc().clone();
    let gesture = PLUGIN_GESTURE_MERGE_BASE + 7;
    for v in [0.1, 0.2, 0.3, 0.4, 0.5] {
        d.apply(
            Command::SetParam {
                key: key("synth"),
                param: "p10".into(),
                value: v,
                merge: Some(gesture),
            },
            &r,
        )
        .unwrap();
    }
    assert_eq!(d.doc().device(&key("synth")).unwrap().params["p10"], 0.5);
    d.undo(&r).unwrap();
    assert!(
        d.doc().device(&key("synth")).unwrap().params.is_empty(),
        "the whole gesture is undone at once"
    );
    d.redo(&r).unwrap();
    assert_eq!(d.doc().device(&key("synth")).unwrap().params["p10"], 0.5);
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn gestures_on_different_params_or_ids_are_separate_steps() {
    let r = plugin_registry();
    let mut d = plugin_chain(&r);
    let gesture = |param: &str, v: f32, id: u64| Command::SetParam {
        key: key("synth"),
        param: param.into(),
        value: v,
        merge: Some(PLUGIN_GESTURE_MERGE_BASE + id),
    };
    run(
        &mut d,
        &r,
        [
            gesture("p10", 0.1, 1),
            gesture("p10", 0.2, 1),
            gesture("p11", 0.3, 1),
            gesture("p11", 0.4, 2),
        ],
    );
    d.undo(&r).unwrap();
    assert_eq!(d.doc().device(&key("synth")).unwrap().params["p11"], 0.3);
    d.undo(&r).unwrap();
    assert!(
        !d.doc()
            .device(&key("synth"))
            .unwrap()
            .params
            .contains_key("p11")
    );
    assert_eq!(d.doc().device(&key("synth")).unwrap().params["p10"], 0.2);
    d.undo(&r).unwrap();
    assert!(d.doc().device(&key("synth")).unwrap().params.is_empty());
}

#[test]
fn automation_lanes_use_the_p_id_keys() {
    let r = plugin_registry();
    let mut d = plugin_chain(&r);
    run(
        &mut d,
        &r,
        [Command::Song(SongCommand::AddAutomationLane {
            device: key("synth"),
            param: "p10".into(),
        })],
    );
    let t = d.doc().song.track(&key("synth")).unwrap();
    assert!(matches!(
        &t.lanes[1].kind,
        atomik_model::song::LaneKind::Automation(p) if p == "p10"
    ));
    let before = d.doc().clone();
    assert!(matches!(
        d.apply(
            Command::Song(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "p99".into(),
            }),
            &r
        ),
        Err(ModelError::UnknownKey(_))
    ));
    assert_eq!(d.doc(), &before);
}

// ---- SyncState and SyncLayout --------------------------------------------------------------------

#[test]
fn sync_state_stores_the_blob_and_is_not_an_undo_step() {
    let r = plugin_registry();
    let mut d = plugin_chain(&r);
    let steps = {
        let mut probe = d.clone();
        undo_all(&mut probe, &r)
    };
    let ops = d.apply(sync_state("synth", b"patch"), &r).unwrap();
    assert!(
        ops.is_empty(),
        "no sync op: the plugin already has its state"
    );
    assert_eq!(
        plugin_of(d.doc(), "synth").state.as_deref(),
        Some(&PluginStateBlob(b"patch".to_vec()))
    );
    assert_eq!(undo_all(&mut d, &r), steps, "the same undo depth as before");
    // An empty blob is a state too.
    let mut d = plugin_chain(&r);
    d.apply(sync_state("fx", b""), &r).unwrap();
    assert_eq!(
        plugin_of(d.doc(), "fx").state.as_deref(),
        Some(&PluginStateBlob(Vec::new()))
    );
}

#[test]
fn sync_state_leaves_redo_and_the_drag_group_alone() {
    let r = plugin_registry();
    let mut d = plugin_chain(&r);
    let gesture = |v: f32| Command::SetParam {
        key: key("synth"),
        param: "p10".into(),
        value: v,
        merge: Some(PLUGIN_GESTURE_MERGE_BASE + 3),
    };
    run(&mut d, &r, [set_param("synth", "p11", 0.9), gesture(0.1)]);
    d.apply(sync_state("synth", b"x"), &r).unwrap();
    d.apply(gesture(0.2), &r).unwrap();
    d.undo(&r).unwrap();
    assert!(
        !d.doc()
            .device(&key("synth"))
            .unwrap()
            .params
            .contains_key("p10"),
        "the gesture stayed one step across the state sync"
    );
    assert!(d.can_redo());
    d.apply(sync_state("synth", b"y"), &r).unwrap();
    assert!(d.can_redo(), "a state sync does not clear redo");
    d.redo(&r).unwrap();
    assert_eq!(d.doc().device(&key("synth")).unwrap().params["p10"], 0.2);
}

#[test]
fn sync_state_rules_and_size_caps() {
    let r = plugin_registry();
    let mut d = plugin_chain(&r);
    run(&mut d, &r, [add(2, DELAY, Some("delay"))]);
    let e = rejects(&mut d, sync_state("ghost", b"x"), &r);
    assert_eq!(e, ModelError::UnknownDevice("ghost".into()));
    let e = rejects(&mut d, sync_state("delay", b"x"), &r);
    assert!(bad_plugin(&e).contains("not a plugin"), "{e}");

    // The per-blob cap is exact.
    let max = usize::try_from(MAX_PLUGIN_STATE_BYTES).unwrap();
    d.apply(
        Command::Plugin(PluginCommand::SyncState {
            device: key("synth"),
            state: PluginStateBlob(vec![0; max]),
        }),
        &r,
    )
    .unwrap();
    let before = d.doc().clone();
    let e = d
        .apply(
            Command::Plugin(PluginCommand::SyncState {
                device: key("fx"),
                state: PluginStateBlob(vec![0; max + 1]),
            }),
            &r,
        )
        .expect_err("over the cap");
    assert!(bad_plugin(&e).contains("16 MiB"), "{e}");
    assert_eq!(d.doc(), &before);
}

#[test]
fn the_project_total_of_states_is_capped() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    let max = usize::try_from(MAX_PLUGIN_STATE_BYTES).unwrap();
    for i in 0..9 {
        d.apply(
            add_plugin(i, &fx_ref(), &fx_layout(), Some(&format!("f{i}"))),
            &r,
        )
        .unwrap();
    }
    for i in 0..8 {
        d.apply(
            Command::Plugin(PluginCommand::SyncState {
                device: key(&format!("f{i}")),
                state: PluginStateBlob(vec![0; max]),
            }),
            &r,
        )
        .unwrap();
    }
    let e = d
        .apply(
            Command::Plugin(PluginCommand::SyncState {
                device: key("f8"),
                state: PluginStateBlob(vec![0; 1]),
            }),
            &r,
        )
        .expect_err("128 MiB reached");
    assert!(bad_plugin(&e).contains("128 MiB"), "{e}");
    // Replacing a blob counts the new size, not the sum of old and new.
    d.apply(
        Command::Plugin(PluginCommand::SyncState {
            device: key("f0"),
            state: PluginStateBlob(vec![1; 10]),
        }),
        &r,
    )
    .unwrap();
}

#[test]
fn sync_layout_replaces_the_snapshot_without_an_undo_step() {
    let r = plugin_registry();
    let mut d = plugin_chain(&r);
    let mut newer = fx_layout();
    newer.version = "2.1.0".into();
    newer.params.push(param(4, "Width", 0.0, 1.0, 0.5));
    let ops = d
        .apply(
            Command::Plugin(PluginCommand::SyncLayout {
                device: key("fx"),
                layout: newer.clone(),
            }),
            &r,
        )
        .unwrap();
    assert!(ops.is_empty());
    assert_eq!(plugin_of(d.doc(), "fx").layout, newer);
    assert_eq!(plugin_of(d.doc(), "fx").version(), "2.1.0");
    let mut huge = fx_layout();
    huge.params = (0..3000).map(|i| param(i, "p", 0.0, 1.0, 0.0)).collect();
    let before = d.doc().clone();
    assert!(matches!(
        d.apply(
            Command::Plugin(PluginCommand::SyncLayout {
                device: key("fx"),
                layout: huge
            }),
            &r
        ),
        Err(ModelError::BadPlugin(_))
    ));
    assert_eq!(d.doc(), &before);
    assert!(matches!(
        d.apply(
            Command::Plugin(PluginCommand::SyncLayout {
                device: key("ghost"),
                layout: fx_layout()
            }),
            &r
        ),
        Err(ModelError::UnknownDevice(_))
    ));
}

// ---- pins, run mode, power ---------------------------------------------------------------------------

#[test]
fn pins_run_mode_and_power_are_one_undo_step_each_with_a_sync_op() {
    let r = plugin_registry();
    let mut d = plugin_chain(&r);
    let changed = SyncOp::PluginChanged {
        device: key("synth"),
    };
    let ops = d.apply(set_pinned("synth", &[11, 10]), &r).unwrap();
    assert_eq!(ops.as_slice(), std::slice::from_ref(&changed));
    assert_eq!(plugin_of(d.doc(), "synth").pinned, [11, 10], "order kept");
    let ops = d
        .apply(
            Command::Plugin(PluginCommand::SetRunMode {
                device: key("synth"),
                run_on_audio_thread: true,
            }),
            &r,
        )
        .unwrap();
    assert_eq!(ops.as_slice(), std::slice::from_ref(&changed));
    assert!(plugin_of(d.doc(), "synth").run_on_audio_thread);
    let ops = d
        .apply(
            Command::Plugin(PluginCommand::SetPower {
                device: key("synth"),
                powered: false,
            }),
            &r,
        )
        .unwrap();
    assert_eq!(ops.as_slice(), std::slice::from_ref(&changed));
    assert!(!plugin_of(d.doc(), "synth").powered);

    let ops = d.undo(&r).unwrap();
    assert_eq!(ops.as_slice(), std::slice::from_ref(&changed));
    assert!(plugin_of(d.doc(), "synth").powered);
    d.undo(&r).unwrap();
    assert!(!plugin_of(d.doc(), "synth").run_on_audio_thread);
    d.undo(&r).unwrap();
    assert!(plugin_of(d.doc(), "synth").pinned.is_empty());
    d.redo(&r).unwrap();
    assert_eq!(plugin_of(d.doc(), "synth").pinned, [11, 10]);
}

#[test]
fn pin_rules() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    let mut big = synth_layout();
    big.params = (0..20)
        .map(|i| param(100 + i, "P", 0.0, 1.0, 0.0))
        .collect();
    let mut reg = plugin_registry();
    register_plugin(&mut reg, &clap_ref("test.big"), &big);
    run(
        &mut d,
        &reg,
        [
            add_plugin(0, &clap_ref("test.big"), &big, Some("big")),
            add(1, DELAY, Some("delay")),
        ],
    );
    let sixteen: Vec<u32> = (100..116).collect();
    d.apply(set_pinned("big", &sixteen), &reg).unwrap();
    assert_eq!(plugin_of(d.doc(), "big").pinned.len(), 16);
    let seventeen: Vec<u32> = (100..117).collect();
    let e = rejects(&mut d, set_pinned("big", &seventeen), &reg);
    assert!(bad_plugin(&e).contains("16"), "{e}");
    let e = rejects(&mut d, set_pinned("big", &[100, 100]), &reg);
    assert!(bad_plugin(&e).contains("100"), "{e}");
    let e = rejects(&mut d, set_pinned("big", &[999]), &reg);
    assert!(bad_plugin(&e).contains("999"), "{e}");
    let e = rejects(&mut d, set_pinned("delay", &[1]), &reg);
    assert!(bad_plugin(&e).contains("not a plugin"), "{e}");
    let e = rejects(&mut d, set_pinned("ghost", &[1]), &reg);
    assert_eq!(e, ModelError::UnknownDevice("ghost".into()));
    // Clearing is fine.
    d.apply(set_pinned("big", &[]), &reg).unwrap();
    assert!(plugin_of(d.doc(), "big").pinned.is_empty());
    let _ = r;
}

#[test]
fn run_mode_and_power_need_a_plugin_device() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add(0, DELAY, Some("delay"))]);
    for c in [
        PluginCommand::SetRunMode {
            device: key("delay"),
            run_on_audio_thread: true,
        },
        PluginCommand::SetPower {
            device: key("delay"),
            powered: false,
        },
    ] {
        let e = rejects(&mut d, Command::Plugin(c), &r);
        assert!(bad_plugin(&e).contains("not a plugin"), "{e}");
    }
}

// ---- relink ---------------------------------------------------------------------------------------------

/// `old.synth` and `old.fx` stand in for plugins that are not installed; the real ones are
/// `test.synth` and `test.fx`.
fn relink_registry() -> atomik_device_sdk::Registry {
    let mut r = plugin_registry();
    register_plugin(&mut r, &clap_ref("old.synth"), &synth_layout());
    register_plugin(&mut r, &clap_ref("old.fx"), &fx_layout());
    r
}

#[test]
fn relink_points_a_stand_in_at_an_installed_plugin_keeping_everything() {
    let r = relink_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &clap_ref("old.synth"), &synth_layout(), Some("synth")),
            add_plugin(1, &clap_ref("old.fx"), &fx_layout(), Some("fx")),
            add(2, DELAY, Some("delay")),
            connect("synth:out_l", "fx:in_l"),
            connect("synth:out_r", "fx:in_r"),
            connect("synth:out_l", "fx:sc_in_l"),
            connect("fx:out_l", "delay:in_l"),
            set_param("fx", "p1", -6.0),
            set_param("synth", "p10", 0.25),
            sync_state("fx", b"saved patch"),
            set_pinned("fx", &[3, 1]),
            Command::Song(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "p10".into(),
            }),
        ],
    );
    let before = d.doc().clone();
    let ops = d
        .apply(
            relink("fx", &fx_ref(), &fx_layout(), Some("C:/CLAP/test.clap")),
            &r,
        )
        .unwrap();
    assert_eq!(
        ops,
        [SyncOp::PluginChanged { device: key("fx") }],
        "the app swaps the instance"
    );
    let fx = d.doc().device(&key("fx")).unwrap();
    assert_eq!(fx.type_id, CLAP_FX);
    let p = fx.plugin.as_ref().unwrap();
    assert_eq!(p.id, "test.fx");
    assert_eq!(p.path_hint.as_deref(), Some("C:/CLAP/test.clap"));
    assert_eq!(fx.params["p1"], -6.0, "params kept");
    assert_eq!(
        p.state,
        before
            .device(&key("fx"))
            .unwrap()
            .plugin
            .as_ref()
            .unwrap()
            .state
    );
    assert_eq!(p.pinned, [3, 1]);
    assert_eq!(d.doc().cables, before.cables, "cables kept");
    assert_eq!(d.doc().song, before.song, "automation kept");

    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before, "one undo step restores type and layout");
    d.redo(&r).unwrap();
    assert_eq!(d.doc().device(&key("fx")).unwrap().type_id, CLAP_FX);
}

#[test]
fn relink_names_a_cabled_socket_the_new_plugin_lacks() {
    let r = relink_registry();
    let mut reg = r;
    register_plugin(&mut reg, &clap_ref("test.fxold"), &fx_layout_old());
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &reg,
        [
            add_plugin(0, &clap_ref("old.synth"), &synth_layout(), Some("synth")),
            add_plugin(1, &clap_ref("old.fx"), &fx_layout(), Some("fx")),
            connect("synth:out_l", "fx:sc_in_l"),
        ],
    );
    let e = rejects(
        &mut d,
        relink("fx", &clap_ref("test.fxold"), &fx_layout_old(), None),
        &reg,
    );
    let m = bad_plugin(&e);
    assert!(m.contains("fx:sc_in_l") || m.contains("sc_in_l"), "{m}");
    assert!(m.contains("cable"), "{m}");
}

#[test]
fn relink_names_an_automated_or_macro_mapped_param_the_new_plugin_lacks() {
    let mut r = relink_registry();
    let mut cut = synth_layout();
    cut.params.retain(|p| p.id != 10);
    register_plugin(&mut r, &clap_ref("test.cut"), &cut);
    // Automated.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &clap_ref("old.synth"), &synth_layout(), Some("synth")),
            Command::Song(SongCommand::AddAutomationLane {
                device: key("synth"),
                param: "p10".into(),
            }),
        ],
    );
    let e = rejects(
        &mut d,
        relink("synth", &clap_ref("test.cut"), &cut, None),
        &r,
    );
    let m = bad_plugin(&e);
    assert!(m.contains("p10") && m.contains("automat"), "{m}");
    // Macro mapped, inside a container.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_plugin_inner(
                "c",
                0,
                &clap_ref("old.synth"),
                &synth_layout(),
                Some("syn"),
                false,
            ),
            set_macro("c", 0, target("syn", "p10", 0.0, 1.0)),
        ],
    );
    let e = rejects(
        &mut d,
        relink("c/syn", &clap_ref("test.cut"), &cut, None),
        &r,
    );
    let m = bad_plugin(&e);
    assert!(m.contains("p10") && m.contains("macro"), "{m}");
    // A param that is only stored (not automated or mapped) does not block; it stays as a
    // dead key.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &clap_ref("old.synth"), &synth_layout(), Some("synth")),
            set_param("synth", "p10", 0.3),
        ],
    );
    d.apply(relink("synth", &clap_ref("test.cut"), &cut, None), &r)
        .unwrap();
    assert_eq!(d.doc().device(&key("synth")).unwrap().params["p10"], 0.3);
}

#[test]
fn relink_refuses_other_kinds_unknown_plugins_and_non_plugins() {
    let r = relink_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &clap_ref("old.fx"), &fx_layout(), Some("fx")),
            add(1, DELAY, Some("delay")),
        ],
    );
    let e = rejects(
        &mut d,
        relink("fx", &synth_ref(), &synth_layout(), None),
        &r,
    );
    assert!(
        bad_plugin(&e).contains("kind") || bad_plugin(&e).contains("instrument"),
        "{e}"
    );
    let e = rejects(
        &mut d,
        relink("fx", &clap_ref("nope"), &fx_layout(), None),
        &r,
    );
    assert_eq!(e, ModelError::UnknownType("clap.nope".into()));
    let e = rejects(&mut d, relink("delay", &fx_ref(), &fx_layout(), None), &r);
    assert!(bad_plugin(&e).contains("not a plugin"), "{e}");
    let e = rejects(&mut d, relink("ghost", &fx_ref(), &fx_layout(), None), &r);
    assert_eq!(e, ModelError::UnknownDevice("ghost".into()));
}

#[test]
fn relink_drops_pins_the_new_layout_lacks_and_undo_brings_them_back() {
    let mut r = relink_registry();
    register_plugin(&mut r, &clap_ref("test.fxold"), &fx_layout_old());
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &clap_ref("old.fx"), &fx_layout(), Some("fx")),
            set_pinned("fx", &[3, 1]),
        ],
    );
    d.apply(
        relink("fx", &clap_ref("test.fxold"), &fx_layout_old(), None),
        &r,
    )
    .unwrap();
    assert_eq!(plugin_of(d.doc(), "fx").pinned, [1]);
    d.undo(&r).unwrap();
    assert_eq!(plugin_of(d.doc(), "fx").pinned, [3, 1]);
}

// ---- batches ---------------------------------------------------------------------------------------------

#[test]
fn a_batch_accepts_pins_power_and_relink_as_one_step() {
    let mut r = relink_registry();
    register_plugin(&mut r, &clap_ref("test.fxold"), &fx_layout_old());
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &clap_ref("old.fx"), &fx_layout(), Some("fx")),
            add_plugin(1, &synth_ref(), &synth_layout(), Some("synth")),
        ],
    );
    let before = d.doc().clone();
    let ops = d
        .apply(
            Command::Batch(vec![
                set_pinned("fx", &[1]),
                Command::Plugin(PluginCommand::SetPower {
                    device: key("fx"),
                    powered: false,
                }),
                relink("fx", &clap_ref("test.fxold"), &fx_layout_old(), None),
                set_pinned("synth", &[10]),
            ]),
            &r,
        )
        .unwrap();
    assert_eq!(
        ops.iter()
            .filter(|o| matches!(o, SyncOp::PluginChanged { device } if *device == key("fx")))
            .count(),
        1,
        "one report per device: {ops:?}"
    );
    assert!(ops.contains(&SyncOp::PluginChanged {
        device: key("synth")
    }));
    assert_eq!(
        d.doc().device(&key("fx")).unwrap().type_id,
        "clap.test.fxold"
    );
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_batch_refuses_add_state_and_the_other_plugin_commands_and_fails_whole() {
    let r = plugin_registry();
    let mut d = plugin_chain(&r);
    for c in [
        add_plugin(0, &fx_ref(), &fx_layout(), None),
        sync_state("synth", b"x"),
        Command::Plugin(PluginCommand::SyncLayout {
            device: key("synth"),
            layout: synth_layout(),
        }),
        Command::Plugin(PluginCommand::SetRunMode {
            device: key("synth"),
            run_on_audio_thread: true,
        }),
    ] {
        let e = rejects(
            &mut d,
            Command::Batch(vec![set_pinned("synth", &[10]), c]),
            &r,
        );
        assert!(matches!(e, ModelError::BadBatch(_)), "{e:?}");
    }
    // One bad pin fails the whole batch.
    rejects(
        &mut d,
        Command::Batch(vec![set_pinned("synth", &[10]), set_pinned("fx", &[999])]),
        &r,
    );
}

// ---- containers ----------------------------------------------------------------------------------------------

#[test]
fn plugins_work_inside_containers_with_path_keys_macros_and_auto_split() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("layer")),
            add_plugin_inner("layer", 0, &synth_ref(), &synth_layout(), None, true),
            add_plugin_inner("layer", 1, &fx_ref(), &fx_layout(), Some("verb"), false),
            connect("layer/test-synth:out_l", "layer/verb:in_l"),
            connect("layer/test-synth:out_r", "layer/verb:in_r"),
            connect("layer/verb:out_l", "layer/boundary:out_l"),
            connect("layer/verb:out_r", "layer/boundary:out_r"),
            set_param("layer/test-synth", "p10", 0.6),
            set_macro("layer", 0, target("test-synth", "p10", 0.0, 1.0)),
            set_macro("layer", 1, target("verb", "p1", -60.0, 12.0)),
        ],
    );
    let inner = d.doc().inner(&key("layer")).unwrap();
    assert_eq!(inner.devices[0].key, key("test-synth"));
    assert!(inner.devices[0].plugin.is_some());
    assert_eq!(inner.splits.len(), 1, "auto split for the instrument");
    assert_eq!(inner.splits[0].target, key("test-synth"));
    // A driven param is locked.
    let e = rejects(&mut d, set_param("layer/test-synth", "p10", 0.2), &r);
    assert!(matches!(e, ModelError::MacroDriven(_)));
    // The device can be removed; its macro targets go with it, in one undo step.
    d.apply(
        Command::RemoveDevice {
            key: key("layer/test-synth"),
        },
        &r,
    )
    .unwrap();
    assert!(
        d.doc().inner(&key("layer")).unwrap().macros[0]
            .targets
            .is_empty()
    );
    d.undo(&r).unwrap();
    assert_eq!(
        d.doc().inner(&key("layer")).unwrap().macros[0]
            .targets
            .len(),
        1
    );
}

#[test]
fn plugins_count_against_the_container_limits() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(&mut d, &r, [add(0, CONTAINER, Some("c"))]);
    for i in 0..32 {
        d.apply(
            add_plugin_inner(
                "c",
                i,
                &fx_ref(),
                &fx_layout(),
                Some(&format!("f{i}")),
                false,
            ),
            &r,
        )
        .unwrap();
    }
    let e = rejects(
        &mut d,
        add_plugin_inner(
            "c",
            32,
            &fx_ref(),
            &fx_layout(),
            Some("one-too-many"),
            false,
        ),
        &r,
    );
    assert!(matches!(e, ModelError::ContainerFull(_)));
    // A nested container is the usual way on.
    assert!(matches!(
        d.apply(
            add_plugin_inner("nope", 0, &fx_ref(), &fx_layout(), None, false),
            &r
        ),
        Err(ModelError::UnknownDevice(_))
    ));
}

#[test]
fn duplicating_a_container_copies_its_plugin_docs_and_state() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_plugin_inner("c", 0, &fx_ref(), &fx_layout(), Some("f"), false),
            sync_state("c/f", b"deep state"),
        ],
    );
    d.apply(
        Command::Container(ContainerCommand::DuplicateContainer {
            container: key("c"),
        }),
        &r,
    )
    .unwrap();
    let copy = d.doc().devices[1].inner.as_ref().unwrap();
    let a = d.doc().inner(&key("c")).unwrap().devices[0].plugin.clone();
    assert_eq!(copy.devices[0].plugin, a);
    assert_eq!(
        a.unwrap().state.as_deref(),
        Some(&PluginStateBlob(b"deep state".to_vec()))
    );
}

#[test]
fn sync_state_reaches_a_plugin_inside_a_container() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("c")),
            add_plugin_inner("c", 0, &fx_ref(), &fx_layout(), Some("f"), false),
        ],
    );
    d.apply(sync_state("c/f", b"abc"), &r).unwrap();
    d.apply(set_pinned("c/f", &[1]), &r).unwrap();
    let p = d
        .doc()
        .resolve(&key("c/f"))
        .unwrap()
        .plugin
        .as_ref()
        .unwrap();
    assert_eq!(p.pinned, [1]);
    assert_eq!(p.state.as_deref(), Some(&PluginStateBlob(b"abc".to_vec())));
}

// ---- expansion -------------------------------------------------------------------------------------------------

#[test]
fn the_expansion_carries_the_plugin_doc() {
    let r = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add_plugin(0, &synth_ref(), &synth_layout(), Some("lead")),
            add(1, CONTAINER, Some("layer")),
            add_plugin_inner("layer", 0, &fx_ref(), &fx_layout(), Some("verb"), false),
            add_inner("layer", 1, DELAY, "delay"),
            sync_state("lead", b"lead patch"),
            sync_state("layer/verb", b"verb patch"),
            set_param("lead", "p10", 0.9),
        ],
    );
    let flat = d.doc().expand(&r);
    let by = |k: &str| flat.devices.iter().find(|x| x.key == key(k)).unwrap();
    let lead = by("lead")
        .plugin
        .as_ref()
        .expect("top-level plugin carried");
    assert_eq!(
        lead.state.as_deref(),
        Some(&PluginStateBlob(b"lead patch".to_vec()))
    );
    assert_eq!(by("lead").params["p10"], 0.9);
    let verb = by("layer/verb")
        .plugin
        .as_ref()
        .expect("inner plugin carried");
    assert_eq!(verb.id, "test.fx");
    assert_eq!(by("layer/verb").type_id, CLAP_FX);
    assert!(by("layer/delay").plugin.is_none());
    assert!(by("layer/~split").plugin.is_none());
}

// ---- stand-ins registered from the document --------------------------------------------------------------------------------

#[test]
fn register_stand_ins_adds_the_missing_plugin_types_from_the_saved_layouts() {
    let full = plugin_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &full,
        [
            add_plugin(0, &synth_ref(), &synth_layout(), Some("s")),
            add(1, CONTAINER, Some("c")),
            add_plugin_inner("c", 0, &fx_ref(), &fx_layout(), Some("f"), false),
        ],
    );
    let mut bare = registry();
    assert!(!bare.is_plugin(CLAP_SYNTH));
    let added = atomik_model::register_stand_ins(d.doc(), &mut bare).unwrap();
    assert_eq!(added, [CLAP_FX.to_owned(), CLAP_SYNTH.to_owned()], "sorted");
    assert!(bare.is_plugin(CLAP_SYNTH) && bare.is_plugin(CLAP_FX));
    // Already registered types are left alone, and a second call adds nothing.
    assert!(
        atomik_model::register_stand_ins(d.doc(), &mut bare)
            .unwrap()
            .is_empty()
    );
    let mut plugged = plugin_registry();
    assert!(
        atomik_model::register_stand_ins(d.doc(), &mut plugged)
            .unwrap()
            .is_empty()
    );
}
