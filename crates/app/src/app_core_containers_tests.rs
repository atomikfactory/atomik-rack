//! The engine gets the expanded rack (ids, macro values,
//! split params, routing), container edits reach it one op at a time, notes for a container go
//! through its split, inner devices' editors, meters and data work by path key, the container
//! editor pane, and the live twin of the `container-demo` bounce.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::{RecordingSink, SinkCall};
use atomik_model::{
    Cable, ConsoleCommand, ContainerCommand, InsertEffect, MacroTarget, SPLIT_TYPE, SocketRef,
    empty_doc,
};
use atomik_ui::kurbo::Rect;
use atomik_ui::{Modifiers, PointerButton, UiEvent};

const CONTAINER: &str = "atomik.container";
const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";
const SAMPLER: &str = "atomik.sampler";
const LATENCY: usize = atomik_core::BATCH;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn demo_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/container-demo/container-demo.atomik")
}

fn apply<S: EngineSink>(c: &mut AppCore<S>, command: Command) {
    c.apply_command(command.clone())
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
}

fn add<S: EngineSink>(c: &mut AppCore<S>, type_id: &str, k: &str) {
    apply(
        c,
        Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
        },
    );
}

fn add_inner<S: EngineSink>(
    c: &mut AppCore<S>,
    container: &str,
    type_id: &str,
    k: &str,
    auto_split: bool,
) {
    apply(
        c,
        Command::Container(ContainerCommand::AddInner {
            container: key(container),
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
            auto_split,
        }),
    );
}

fn connect<S: EngineSink>(c: &mut AppCore<S>, from: (&str, &str), to: (&str, &str)) {
    let r = |(d, s): (&str, &str)| SocketRef {
        device: key(d),
        socket: s.to_string(),
    };
    apply(
        c,
        Command::Connect(Cable {
            from: r(from),
            to: r(to),
        }),
    );
}

fn param_id(type_id: &str, param: &str) -> atomik_core::ParamId {
    default_registry()
        .get(type_id)
        .unwrap()
        .descriptor
        .param_by_key(param)
        .unwrap()
        .id
}

fn socket_id(type_id: &str, socket: &str) -> atomik_device_sdk::SocketId {
    default_registry()
        .get(type_id)
        .unwrap()
        .descriptor
        .socket_by_key(socket)
        .unwrap()
        .id
}

fn demo_core() -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    c.open(&demo_file()).unwrap();
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    c
}

fn params_of(calls: &[SinkCall]) -> Vec<(u32, u32, f32)> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Param { id, param, plain } => Some((id.0, param.0, *plain)),
            _ => None,
        })
        .collect()
}

fn inserts_of(calls: &[SinkCall]) -> Vec<(u32, &'static str)> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Insert { id, type_id, .. } => Some((id.0, *type_id)),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------ the engine gets the expansion

#[test]
fn a_loaded_project_inserts_the_expanded_devices_in_order_then_routes_once() {
    let mut c = AppCore::new(RecordingSink::new());
    c.sink_mut().take();
    c.open(&demo_file()).unwrap();
    let calls = c.sink_mut().take();
    let expanded = c.doc().expand(c.registry());
    let inserts = inserts_of(&calls);
    assert_eq!(inserts.len(), expanded.devices.len());
    for (i, (id, type_id)) in inserts.iter().enumerate() {
        assert_eq!(*id as usize, i + 1, "ids are 1, 2, 3... in expansion order");
        assert_eq!(*type_id, expanded.devices[i].type_id);
        assert_eq!(
            c.device_id(&expanded.devices[i].key),
            Some(DeviceInstanceId(*id))
        );
    }
    // The container itself has no engine device; its alias is its split.
    assert_eq!(c.device_id(&key("layer")), None);
    assert_eq!(c.flat_id(&key("layer")), c.device_id(&key("layer/~split")));
    assert!(c.device_id(&key("tone/space/rev")).is_some());
    // Insert, then data, then routing: every insert precedes the (last) routing.
    let last_insert = calls
        .iter()
        .rposition(|x| matches!(x, SinkCall::Insert { .. }))
        .unwrap();
    let routing = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Routing(_)))
        .unwrap();
    assert!(last_insert < routing);
    // The routing's nodes include every flat device (and the interface).
    let SinkCall::Routing(order) = &calls[routing] else {
        unreachable!()
    };
    for d in &expanded.devices {
        assert!(order.contains(&c.device_id(&d.key).unwrap()), "{}", d.key);
    }
}

#[test]
fn inserts_carry_macro_values_and_the_split_params() {
    let mut c = AppCore::new(RecordingSink::new());
    c.sink_mut().take();
    c.open(&demo_file()).unwrap();
    let calls = c.sink_mut().take();
    let insert_params = |k: &str| -> Vec<(u32, f32)> {
        let id = c.device_id(&key(k)).unwrap();
        calls
            .iter()
            .find_map(|x| match x {
                SinkCall::Insert { id: i, params, .. } if *i == id => {
                    Some(params.iter().map(|(p, v)| (p.0, *v)).collect())
                }
                _ => None,
            })
            .unwrap()
    };
    let cutoff = param_id(SYNTH, "filter.cutoff").0;
    // Macro 1 is 0.5: Low 400..3200 gives 1800, High 6000..800 (inverted) gives 3400.
    assert!(insert_params("layer/low").contains(&(cutoff, 1800.0)));
    assert!(insert_params("layer/high").contains(&(cutoff, 3400.0)));
    let split = insert_params("layer/~split");
    for (p, v) in [
        ("s1.on", 1.0),
        ("s1.key_hi", 59.0),
        ("s2.key_lo", 60.0),
        ("s2.transpose", 12.0),
    ] {
        assert!(
            split.contains(&(param_id(SPLIT_TYPE, p).0, v)),
            "{p}: {split:?}"
        );
    }
}

#[test]
fn a_container_added_and_filled_inserts_each_piece_and_sends_the_split_once() {
    let mut c = AppCore::new(RecordingSink::new());
    c.sink_mut().take();
    add(&mut c, CONTAINER, "c");
    let calls = c.sink_mut().take();
    assert_eq!(inserts_of(&calls), vec![(1, SPLIT_TYPE)]);
    assert!(c.device_id(&key("c/~split")).is_some());
    add_inner(&mut c, "c", SYNTH, "s", true);
    let calls = c.sink_mut().take();
    let split = c.device_id(&key("c/~split")).unwrap();
    let synth = c.device_id(&key("c/s")).unwrap();
    assert_eq!(inserts_of(&calls), vec![(synth.0, SYNTH)]);
    // The auto split switches slot 1 on: the only split param that differs from the default.
    assert_eq!(
        params_of(&calls),
        vec![(split.0, param_id(SPLIT_TYPE, "s1.on").0, 1.0)]
    );
    // The routing cables the split's first output to the synth's notes input.
    let routing = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Routing(_)))
        .unwrap();
    assert!(
        routing
            > calls
                .iter()
                .position(|x| matches!(x, SinkCall::Insert { .. }))
                .unwrap()
    );
    let topo = c.topology();
    assert!(topo.cables().any(|cable| {
        cable.from.device == split
            && cable.from.socket == socket_id(SPLIT_TYPE, "out_1")
            && cable.to.device == synth
            && cable.to.socket == socket_id(SYNTH, "notes_in")
    }));
}

#[test]
fn split_edits_send_only_the_changed_params_and_unused_slots_return_to_defaults() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, CONTAINER, "c");
    add_inner(&mut c, "c", SYNTH, "a", true);
    add_inner(&mut c, "c", SYNTH, "b", true);
    c.sink_mut().take();
    let split = c.device_id(&key("c/~split")).unwrap().0;
    let mut splits = c.doc().inner(&key("c")).unwrap().splits.clone();
    assert_eq!(splits.len(), 2);
    splits[0].key_hi = 59;
    splits[1].key_lo = 60;
    splits[1].transpose = -12;
    apply(
        &mut c,
        Command::Container(ContainerCommand::SetSplits {
            container: key("c"),
            splits: splits.clone(),
            merge: None,
        }),
    );
    let mut sent = params_of(&c.sink_mut().take());
    sent.sort_by_key(|(_, p, _)| *p);
    let mut expect = vec![
        (split, param_id(SPLIT_TYPE, "s1.key_hi").0, 59.0),
        (split, param_id(SPLIT_TYPE, "s2.key_lo").0, 60.0),
        (split, param_id(SPLIT_TYPE, "s2.transpose").0, -12.0),
    ];
    expect.sort_by_key(|(_, p, _)| *p);
    assert_eq!(sent, expect);
    // Dropping slot 2 puts all its params back to the defaults.
    splits.truncate(1);
    apply(
        &mut c,
        Command::Container(ContainerCommand::SetSplits {
            container: key("c"),
            splits,
            merge: None,
        }),
    );
    let sent = params_of(&c.sink_mut().take());
    assert!(sent.contains(&(split, param_id(SPLIT_TYPE, "s2.on").0, 0.0)));
    assert!(sent.contains(&(split, param_id(SPLIT_TYPE, "s2.key_lo").0, 0.0)));
    assert!(sent.contains(&(split, param_id(SPLIT_TYPE, "s2.transpose").0, 0.0)));
    // Undo restores slot 2.
    assert!(c.undo());
    let sent = params_of(&c.sink_mut().take());
    assert!(sent.contains(&(split, param_id(SPLIT_TYPE, "s2.on").0, 1.0)));
    assert!(sent.contains(&(split, param_id(SPLIT_TYPE, "s2.transpose").0, -12.0)));
}

// ------------------------------------------------------------------ macros

#[test]
fn a_macro_value_is_one_set_param_per_target_and_undo_redo_follow() {
    let mut c = demo_core();
    c.sink_mut().take();
    apply(
        &mut c,
        Command::SetParam {
            key: key("layer"),
            param: "macro1".into(),
            value: 1.0,
            merge: None,
        },
    );
    let sent = params_of(&c.sink_mut().take());
    let (low, high, echo) = (
        c.device_id(&key("layer/low")).unwrap().0,
        c.device_id(&key("layer/high")).unwrap().0,
        c.device_id(&key("layer/echo")).unwrap().0,
    );
    let cutoff = param_id(SYNTH, "filter.cutoff").0;
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert!(sent.contains(&(low, cutoff, 3200.0)));
    assert!(sent.contains(&(high, cutoff, 800.0)));
    assert!(sent.contains(&(echo, param_id(DELAY, "mix").0, 0.5)));
    assert!(c.undo());
    let sent = params_of(&c.sink_mut().take());
    assert!(sent.contains(&(low, cutoff, 1800.0)));
    assert!(c.redo());
    assert!(params_of(&c.sink_mut().take()).contains(&(low, cutoff, 3200.0)));
}

#[test]
fn unmapping_a_target_gives_the_param_its_stored_value_back_and_undo_maps_it_again() {
    let mut c = demo_core();
    c.sink_mut().take();
    apply(
        &mut c,
        Command::Container(ContainerCommand::RemoveMacroTarget {
            container: key("layer"),
            index: 0,
            device: key("low"),
            param: "filter.cutoff".into(),
        }),
    );
    let low = c.device_id(&key("layer/low")).unwrap().0;
    let cutoff = param_id(SYNTH, "filter.cutoff").0;
    let sent = params_of(&c.sink_mut().take());
    // The stored value of Low's cutoff (900) returns; the other targets are re-sent as they are.
    assert!(sent.contains(&(low, cutoff, 900.0)), "{sent:?}");
    assert!(c.undo());
    let sent = params_of(&c.sink_mut().take());
    assert!(sent.contains(&(low, cutoff, 1800.0)), "{sent:?}");
}

#[test]
fn mapping_a_new_target_and_a_range_edit_reach_the_engine() {
    let mut c = demo_core();
    c.sink_mut().take();
    apply(
        &mut c,
        Command::Container(ContainerCommand::SetMacro {
            container: key("layer"),
            index: 1,
            target: MacroTarget {
                device: key("high"),
                param: "master.level".into(),
                min: -30.0,
                max: 0.0,
            },
            merge: None,
        }),
    );
    let high = c.device_id(&key("layer/high")).unwrap().0;
    let sent = params_of(&c.sink_mut().take());
    // Macro 2 is 0.4: -30 + 30 * 0.4 = -18.
    assert!(
        sent.contains(&(high, param_id(SYNTH, "master.level").0, -18.0)),
        "{sent:?}"
    );
}

#[test]
fn a_nested_macro_chain_reaches_the_inner_device() {
    let mut c = demo_core();
    c.sink_mut().take();
    apply(
        &mut c,
        Command::SetParam {
            key: key("tone"),
            param: "macro1".into(),
            value: 1.0,
            merge: None,
        },
    );
    let sent = params_of(&c.sink_mut().take());
    let comp = c.device_id(&key("tone/comp")).unwrap().0;
    let rev = c.device_id(&key("tone/space/rev")).unwrap().0;
    assert!(sent.contains(&(comp, param_id("atomik.compressor", "threshold").0, -30.0)));
    assert!(sent.contains(&(rev, param_id("atomik.reverb", "mix").0, 0.35)));
}

// ------------------------------------------------------------------ removing and restoring

#[test]
fn removing_a_container_removes_every_inner_id_and_undo_inserts_the_inside_again() {
    let mut c = demo_core();
    let inner: Vec<DeviceKey> = c
        .doc()
        .expand(c.registry())
        .devices
        .iter()
        .filter(|d| d.key.0.starts_with("layer/"))
        .map(|d| d.key.clone())
        .collect();
    assert_eq!(inner.len(), 5);
    let ids: Vec<DeviceInstanceId> = inner.iter().map(|k| c.device_id(k).unwrap()).collect();
    c.sink_mut().take();
    apply(&mut c, Command::RemoveDevice { key: key("layer") });
    let calls = c.sink_mut().take();
    let removed: Vec<DeviceInstanceId> = calls
        .iter()
        .filter_map(|x| match x {
            SinkCall::Remove(id) => Some(*id),
            _ => None,
        })
        .collect();
    for id in &ids {
        assert!(removed.contains(id), "{id:?} not removed");
    }
    assert!(inner.iter().all(|k| c.device_id(k).is_none()));
    // Routing before the removals.
    let routing = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Routing(_)))
        .unwrap();
    let first_remove = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Remove(_)))
        .unwrap();
    assert!(routing < first_remove);
    assert!(c.undo());
    let calls = c.sink_mut().take();
    let inserted = inserts_of(&calls);
    assert!(inserted.len() >= 5, "{inserted:?}");
    assert!(inner.iter().all(|k| c.device_id(k).is_some()));
    // The restored devices have fresh ids and the macro values again.
    let low = c.device_id(&key("layer/low")).unwrap();
    assert!(!ids.contains(&low));
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
}

#[test]
fn an_inner_device_removal_and_undo_are_incremental() {
    let mut c = demo_core();
    c.sink_mut().take();
    apply(
        &mut c,
        Command::RemoveDevice {
            key: key("layer/high"),
        },
    );
    let calls = c.sink_mut().take();
    assert!(c.device_id(&key("layer/high")).is_none());
    assert_eq!(
        calls
            .iter()
            .filter(|x| matches!(x, SinkCall::Remove(_)))
            .count(),
        1
    );
    assert!(c.undo());
    let calls = c.sink_mut().take();
    assert_eq!(
        inserts_of(&calls).len(),
        1,
        "only the device comes back: {calls:?}"
    );
    assert!(c.device_id(&key("layer/high")).is_some());
}

// ------------------------------------------------------------------ notes

#[test]
fn live_notes_for_a_container_go_through_its_split_and_inner_devices_play_directly() {
    let mut c = demo_core();
    c.sink_mut().take();
    c.audition(&key("layer"), 60, true);
    c.audition(&key("layer/low"), 40, true);
    let calls = c.sink_mut().take();
    let notes: Vec<_> = calls
        .iter()
        .filter_map(|x| match x {
            SinkCall::LiveNote { target, kind } => Some((*target, *kind)),
            _ => None,
        })
        .collect();
    assert_eq!(notes.len(), 2);
    let split = c.device_id(&key("layer/~split")).unwrap();
    assert_eq!(notes[0].0, (split, socket_id(SPLIT_TYPE, "notes_in")));
    let low = c.device_id(&key("layer/low")).unwrap();
    assert_eq!(notes[1].0, (low, socket_id(SYNTH, "notes_in")));
}

#[test]
fn the_keyboard_follows_the_selection_into_a_container() {
    let mut c = demo_core();
    c.select(Some(key("layer")));
    let (id, socket) = c.notes_target().unwrap();
    assert_eq!(Some(id), c.device_id(&key("layer/~split")));
    assert_eq!(socket, socket_id(SPLIT_TYPE, "notes_in"));
    assert_eq!(c.notes_target_device().unwrap().key, key("layer"));
    c.select(Some(key("layer/high")));
    assert_eq!(
        c.notes_target().unwrap().0,
        c.device_id(&key("layer/high")).unwrap()
    );
    assert_eq!(c.notes_target_device().unwrap().key, key("high"));
}

#[test]
fn a_song_track_on_a_container_plays_its_split_and_macro_lanes_drive_the_targets() {
    let mut c = AppCore::new(RecordingSink::new());
    c.sink_mut().take();
    c.open(&demo_file()).unwrap();
    let calls = c.sink_mut().take();
    let SinkCall::Song(input) = calls
        .iter()
        .rev()
        .find(|x| matches!(x, SinkCall::Song(_)))
        .unwrap()
    else {
        unreachable!()
    };
    let split = c.device_id(&key("layer/~split")).unwrap();
    assert_eq!(input.tracks[0].device, split);
    assert_eq!(
        input.tracks[0].notes_socket,
        socket_id(SPLIT_TYPE, "notes_in")
    );
    let lane_devices: Vec<_> = input.tracks[1..].iter().map(|t| t.device).collect();
    assert_eq!(
        lane_devices,
        [
            c.device_id(&key("layer/low")).unwrap(),
            c.device_id(&key("layer/high")).unwrap(),
            c.device_id(&key("layer/echo")).unwrap()
        ]
    );
}

// ------------------------------------------------------------------ console

#[test]
fn to_ch_and_the_insert_menu_accept_a_container() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, CONTAINER, "voice");
    add_inner(&mut c, "voice", SYNTH, "s", true);
    apply(
        &mut c,
        Command::Console(ConsoleCommand::AddChannel {
            source: key("voice"),
        }),
    );
    let channel = c.doc().console.channels[0].channel.clone();
    add(&mut c, CONTAINER, "fx");
    add_inner(&mut c, "fx", DELAY, "d", false);
    connect(&mut c, ("fx/boundary", "in_l"), ("fx/d", "in_l"));
    connect(&mut c, ("fx/d", "out_l"), ("fx/boundary", "out_l"));
    c.sink_mut().take();
    apply(
        &mut c,
        Command::Console(ConsoleCommand::SetInsert {
            channel,
            slot: 0,
            effect: Some(InsertEffect::Existing(key("fx"))),
        }),
    );
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    // The chain's cables are spliced into the flat graph: a cable ends at the inner delay.
    assert!(c.topology().cables().any(|cable| {
        Some(cable.to.device) == c.device_id(&key("fx/d"))
            && cable.to.socket == socket_id(DELAY, "in_l")
    }));
}

// ------------------------------------------------------------------ editors and path keys

fn write_wav(dir: &Path, name: &str) -> PathBuf {
    let data: Vec<f32> = (0..4_800).map(|i| (i as f32 / 4_800.0) * 0.5).collect();
    let path = dir.join(name);
    atomik_audio_file::write_wav_f32(
        &path,
        &atomik_audio_file::AudioData::new(48_000, vec![data]).unwrap(),
    )
    .unwrap();
    path
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "atomik-app-containers-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn an_inner_sampler_gets_its_zones_and_map_by_path_key() {
    let dir = temp_dir("sampler");
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, CONTAINER, "c");
    add_inner(&mut c, "c", SAMPLER, "smp", true);
    c.sink_mut().take();
    c.handle_action(UiAction::ImportSamples {
        paths: vec![write_wav(&dir, "tone_C3.wav")],
        device: key("c/smp"),
        at_key: None,
    });
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    assert_eq!(c.doc().resolve(&key("c/smp")).unwrap().zones.len(), 1);
    let calls = c.sink_mut().take();
    let id = c.device_id(&key("c/smp")).unwrap();
    assert!(calls.iter().any(
        |x| matches!(x, SinkCall::Samples { id: i, map: Some(m) } if *i == id && m.zones().len() == 1)
    ));
    // The editor opens on the path key, and the default helper finds the inner sampler.
    c.handle_action(UiAction::OpenSampleEditor(key("c/smp")));
    assert_eq!(c.sample_editor_device(), Some(&key("c/smp")));
    c.close_sample_editor();
    assert!(c.open_sample_editor_default());
    assert_eq!(c.sample_editor_device(), Some(&key("c/smp")));
    // A restart sends the map again after the insert.
    c.sink_mut().take();
    c.rebuild_engine();
    let calls = c.sink_mut().take();
    let id = c.device_id(&key("c/smp")).unwrap();
    let insert = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Insert { id: i, .. } if *i == id))
        .unwrap();
    assert!(matches!(&calls[insert + 1], SinkCall::Samples { id: i, .. } if *i == id));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_inner_sequencer_gets_its_data_and_opens_the_grid_editor_by_path_key() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, CONTAINER, "c");
    c.sink_mut().take();
    add_inner(&mut c, "c", "atomik.step-sequencer", "seq", false);
    let calls = c.sink_mut().take();
    let id = c.device_id(&key("c/seq")).unwrap();
    let insert = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Insert { id: i, .. } if *i == id))
        .unwrap();
    assert!(matches!(
        &calls[insert + 1],
        SinkCall::PlayerData { id: i, data: Some(_) } if *i == id
    ));
    // A pattern edit through the path key is one swap.
    let mut p = atomik_model::PatternDoc::new("P");
    p.steps[0].on = true;
    apply(
        &mut c,
        Command::SetPattern {
            device: key("c/seq"),
            index: 0,
            pattern: p,
            merge: None,
        },
    );
    let calls = c.sink_mut().take();
    assert_eq!(
        calls
            .iter()
            .filter(|x| matches!(x, SinkCall::PlayerData { .. }))
            .count(),
        1
    );
    c.handle_action(UiAction::OpenSeqEditor {
        device: key("c/seq"),
    });
    assert_eq!(c.seq_editor_device(), Some(&key("c/seq")));
    c.close_seq_editor();
    assert!(c.open_seq_editor_default());
    assert_eq!(c.seq_editor_device(), Some(&key("c/seq")));
}

#[test]
fn an_inner_loop_slicer_opens_its_editor_by_path_key() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, CONTAINER, "c");
    add_inner(&mut c, "c", "atomik.loop-slicer", "sl", false);
    c.handle_action(UiAction::OpenSliceEditor(key("c/sl")));
    assert_eq!(c.slice_editor_device(), Some(&key("c/sl")));
    c.close_slice_editor();
    assert!(c.open_slice_editor_default());
}

#[test]
fn song_track_actions_for_inner_devices_are_refused_with_a_message() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, CONTAINER, "c");
    add_inner(&mut c, "c", "atomik.drum-machine", "drums", false);
    add_inner(&mut c, "c", "atomik.loop-slicer", "sl", false);
    for action in [
        UiAction::CreateStepClip {
            device: key("c/drums"),
        },
        UiAction::OpenStepEditor {
            device: key("c/drums"),
            clip: None,
        },
        UiAction::SlicesToTrack {
            device: key("c/sl"),
        },
    ] {
        c.handle_action(action.clone());
        let message = c.last_rejection().unwrap_or_default().to_string();
        assert!(
            message.contains("inside a container"),
            "{action:?}: {message}"
        );
        c.reject(String::new());
    }
    assert!(c.step_editor_device().is_none());
    assert!(c.doc().song.tracks.is_empty());
}

// ------------------------------------------------------------------ the container editor pane

#[test]
fn the_container_editor_opens_on_the_panels_request_and_closes_in_every_case() {
    let mut c = demo_core();
    c.handle_action(UiAction::OpenContainerEditor {
        device: key("layer"),
        macros: Some(1),
    });
    assert_eq!(c.container_editor_device(), Some(&key("layer")));
    assert_eq!(c.container_editor().tab(), atomik_ui::ContainerTab::Macros);
    assert_eq!(c.container_editor().selected_macro(), 1);
    assert!(c.container_editor().is_present());
    c.handle_action(UiAction::CloseContainerEditor);
    assert!(c.container_editor_device().is_none());
    // The default helper takes the selected container, else the first one.
    c.select(Some(key("tone")));
    assert!(c.open_container_editor_default());
    assert_eq!(c.container_editor_device(), Some(&key("tone")));
    assert_eq!(c.container_editor().tab(), atomik_ui::ContainerTab::Splits);
    // Another editor replaces it.
    c.open_sample_editor(key("layer/low"));
    assert!(c.open_seq_editor_default() || c.container_editor_device().is_some());
    c.close_container_editor();
    // The container vanishing closes it.
    c.handle_action(UiAction::OpenContainerEditor {
        device: key("layer"),
        macros: None,
    });
    apply(&mut c, Command::RemoveDevice { key: key("layer") });
    assert!(c.container_editor_device().is_none());
    assert!(c.undo());
    // A new document closes it.
    c.handle_action(UiAction::OpenContainerEditor {
        device: key("layer"),
        macros: None,
    });
    c.new_document();
    assert!(c.container_editor_device().is_none());
    // A device that is no container is ignored.
    c.open_container_editor(key("nothing"), None);
    assert!(c.container_editor_device().is_none());
}

#[test]
fn the_editors_pointer_events_edit_the_document_through_the_pane() {
    let mut c = demo_core();
    c.handle_action(UiAction::OpenContainerEditor {
        device: key("layer"),
        macros: None,
    });
    c.container_editor_mut()
        .set_bounds(Rect::new(0.0, 0.0, 900.0, 420.0));
    c.container_editor_mut().set_focused(true);
    c.after_view_change();
    let before = c.doc().inner(&key("layer")).unwrap().splits.clone();
    // Delete the first split with its row button.
    let at = c.container_editor().delete_rect(0).center();
    for (down, pos) in [(true, at), (false, at)] {
        let event = if down {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
                clicks: 1,
            }
        } else {
            UiEvent::PointerUp {
                pos,
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
            }
        };
        c.handle_pane_event(Pane::ContainerEditor, &event);
    }
    let after = c.doc().inner(&key("layer")).unwrap().splits.clone();
    assert_eq!(after.len(), before.len() - 1, "{after:?}");
    assert!(c.undo());
    assert_eq!(c.doc().inner(&key("layer")).unwrap().splits, before);
}

#[test]
fn a_project_load_returns_the_rack_view_to_the_top_level() {
    let mut c = demo_core();
    c.view_mut().set_scope(&[key("layer")]);
    c.after_view_change();
    assert_eq!(c.view().scope(), [key("layer")]);
    c.open(&demo_file()).unwrap();
    assert!(c.view().scope().is_empty());
    c.view_mut().set_scope(&[key("tone")]);
    c.new_document();
    assert!(c.view().scope().is_empty());
}

// ------------------------------------------------------------------ meters

#[test]
fn the_inner_compressors_meter_values_are_fed_by_path_key() {
    let mut c = AppCore::new(OfflineSink::new());
    c.open(&demo_file()).unwrap();
    c.wait_background();
    c.transport_action(atomik_ui::TransportAction::Play);
    c.sink_mut().render_blocks(24_000, 512);
    let mut buf = [0.0f32; 32];
    let (n, _) = c
        .device_meter_values(&key("tone/comp"), &mut buf)
        .expect("the inner compressor publishes");
    assert!(n > 0);
    // The per-frame feed reaches the view without error for the path key.
    c.frame();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
}

// ------------------------------------------------------------------ live equals bounce

fn bounce_of(doc: &atomik_model::RackDoc) -> Vec<Vec<f32>> {
    atomik_render::project::render_doc_in(
        doc,
        &default_registry(),
        &atomik_render::project::ProjectOptions::default(),
        None,
    )
    .unwrap()
    .channels
}

fn live_core() -> AppCore<OfflineSink> {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.open(&demo_file()).unwrap();
    core.wait_background();
    assert!(
        core.last_rejection().is_none(),
        "{:?}",
        core.last_rejection()
    );
    core
}

fn play(core: &mut AppCore<OfflineSink>, frames: usize, block: usize) -> Vec<Vec<f32>> {
    core.transport_action(atomik_ui::TransportAction::Play);
    core.sink_mut().render_blocks(frames + LATENCY, block)
}

/// The live render equals the bounce bit for bit, delayed by the engine batch (both sides run the
/// same engine: the inner compressor's look-ahead is in both, so like is compared with like).
fn same_bits(live: &[Vec<f32>], bounce: &[Vec<f32>]) -> bool {
    live.iter().zip(bounce).all(|(a, b)| {
        a[LATENCY..]
            .iter()
            .zip(b)
            .all(|(x, y)| x.to_bits() == y.to_bits())
            && a[..LATENCY].iter().all(|x| *x == 0.0)
    })
}

#[test]
fn the_live_path_plays_the_container_demo_exactly_like_the_bounce() {
    let reference = bounce_of(live_core().doc());
    let frames = reference[0].len();
    assert!(reference[0].iter().any(|s| s.abs() > 1e-3));
    for block in [64, 480, 512] {
        let mut core = live_core();
        let live = play(&mut core, frames, block);
        assert!(same_bits(&live, &reference), "block {block}");
    }
}

#[test]
fn live_equals_bounce_after_split_macro_and_mapping_edits_undo_and_redo() {
    // A split edit.
    let mut core = live_core();
    let mut splits = core.doc().inner(&key("layer")).unwrap().splits.clone();
    splits[0].key_hi = 48;
    splits[1].key_lo = 49;
    splits[1].transpose = 7;
    apply(
        &mut core,
        Command::Container(ContainerCommand::SetSplits {
            container: key("layer"),
            splits,
            merge: None,
        }),
    );
    let reference = bounce_of(&core.doc().clone());
    assert!(
        reference != bounce_of(live_core().doc()),
        "the edit is audible"
    );
    let live = play(&mut core, reference[0].len(), 512);
    assert!(same_bits(&live, &reference), "split edit");
    // A macro value and an unmapped target.
    let mut core = live_core();
    apply(
        &mut core,
        Command::SetParam {
            key: key("layer"),
            param: "macro2".into(),
            value: 0.9,
            merge: None,
        },
    );
    apply(
        &mut core,
        Command::Container(ContainerCommand::RemoveMacroTarget {
            container: key("layer"),
            index: 0,
            device: key("high"),
            param: "filter.cutoff".into(),
        }),
    );
    let reference = bounce_of(&core.doc().clone());
    let live = play(&mut core, reference[0].len(), 480);
    assert!(same_bits(&live, &reference), "macro edits");
    // Undo returns to the original sound, redo to the edited one.
    let mut core = live_core();
    let original = bounce_of(core.doc());
    apply(
        &mut core,
        Command::SetParam {
            key: key("tone"),
            param: "macro1".into(),
            value: 0.1,
            merge: None,
        },
    );
    assert!(core.undo());
    let live = play(&mut core, original[0].len(), 64);
    assert!(same_bits(&live, &original), "undo");
    let mut core = live_core();
    apply(
        &mut core,
        Command::SetParam {
            key: key("tone"),
            param: "macro1".into(),
            value: 0.1,
            merge: None,
        },
    );
    assert!(core.undo());
    assert!(core.redo());
    let reference = bounce_of(&core.doc().clone());
    let live = play(&mut core, reference[0].len(), 512);
    assert!(same_bits(&live, &reference), "redo");
}

#[test]
fn live_equals_bounce_after_removing_and_restoring_the_container_and_an_engine_restart() {
    let mut core = live_core();
    let original = bounce_of(core.doc());
    apply(&mut core, Command::RemoveDevice { key: key("tone") });
    assert!(core.undo());
    core.rebuild_engine();
    let live = play(&mut core, original[0].len(), 512);
    assert!(
        same_bits(&live, &original),
        "restart after undo of a removal"
    );
}

#[test]
fn inner_device_edits_made_live_match_the_bounce_of_the_edited_document() {
    let mut core = live_core();
    apply(
        &mut core,
        Command::SetParam {
            key: key("layer/low"),
            param: "filter.resonance".into(),
            value: 0.5,
            merge: None,
        },
    );
    add_inner(&mut core, "tone", "atomik.stereo-delay", "extra", false);
    let reference = bounce_of(&core.doc().clone());
    let live = play(&mut core, reference[0].len(), 512);
    assert!(same_bits(&live, &reference));
}

// ------------------------------------------------------------------ lint

/// Everything that builds or feeds the engine reads `RackDoc::expand`; a loop over the raw
/// `devices` or `cables` of the document in engine-facing code would silently miss the insides of
/// containers. The few raw uses left are top-level searches and a debug line.
#[test]
fn engine_facing_code_never_walks_the_raw_document_lists() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let render = Path::new(env!("CARGO_MANIFEST_DIR")).join("../render/src/project.rs");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&src)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            name.starts_with("app_core") && name.ends_with(".rs") && !name.contains("_tests")
        })
        .collect();
    files.push(render);
    // (file name, a substring a raw use may contain).
    let allowed = [
        (
            "app_core.rs",
            ".field(\"devices\", &self.doc().devices.len())",
        ),
        (
            "app_core.rs",
            "doc.devices.iter().find_map(|d| self.target_of(&d.key))",
        ),
        // The device above a top-level slot of `CreateEffectAt`.
        ("app_core_clipboard.rs", "doc.devices[at.index - 1]"),
        ("app_core_containers.rs", "doc.devices"),
        // All Notes Off reaches every top-level device; `target_of` resolves a container to the
        // notes input of its split device.
        ("app_core_workflow.rs", "for d in &self.doc().devices"),
        // What a freeze render depends on: the document's own cables around the chain, compared
        // when the render is done (nothing is sent to the engine).
        ("app_core_freeze.rs", "doc.cables"),
        (
            "app_core_drums.rs",
            "doc.devices.iter().find(|d| is_drum(d))",
        ),
    ];
    let forbidden = ["doc().devices", "doc.devices", "doc().cables", "doc.cables"];
    let mut bad = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        // The unit tests at the end of a source file are not engine-facing code.
        let code = text.split("#[cfg(test)]").next().unwrap();
        for (n, line) in code.lines().enumerate() {
            if line.trim_start().starts_with("//") || !forbidden.iter().any(|f| line.contains(f)) {
                continue;
            }
            if !allowed
                .iter()
                .any(|(f, sub)| *f == name && line.contains(sub))
            {
                bad.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "raw document lists in engine-facing code: {bad:#?}"
    );
}

/// QA: duplicating a container inserts the copy's split and inside as fresh engine ids
/// (macro values and split params included), leaves the original's ids alone, and undo removes
/// every id of the copy again.
#[test]
fn duplicating_a_container_inserts_the_copys_inside_and_undo_removes_it() {
    let mut c = demo_core();
    let before: Vec<(DeviceKey, DeviceInstanceId)> = ["layer/low", "layer/high", "layer/~split"]
        .iter()
        .map(|k| {
            (
                key(k),
                c.device_id(&key(k)).or_else(|| c.flat_id(&key(k))).unwrap(),
            )
        })
        .collect();
    c.sink_mut().take();
    apply(
        &mut c,
        Command::Container(ContainerCommand::DuplicateContainer {
            container: key("layer"),
        }),
    );
    let calls = c.sink_mut().take();
    let copy = c
        .doc()
        .devices
        .iter()
        .find(|d| d.name == "Layer copy")
        .map(|d| d.key.clone())
        .expect("the copy is named `<name> copy`");
    let copy_keys: Vec<DeviceKey> = c
        .doc()
        .expand(c.registry())
        .devices
        .iter()
        .filter(|d| d.key.0.starts_with(&format!("{}/", copy.0)))
        .map(|d| d.key.clone())
        .collect();
    assert_eq!(copy_keys.len(), 5, "{copy_keys:?}");
    assert_eq!(inserts_of(&calls).len(), 5);
    for k in &copy_keys {
        assert!(c.flat_id(k).is_some(), "{k}");
    }
    // The original's ids did not move.
    for (k, id) in &before {
        assert_eq!(c.flat_id(k), Some(*id), "{k}");
    }
    // The copy's macro-driven params and split slots went in with its inserts.
    let insert_params = |k: &DeviceKey| -> Vec<(u32, f32)> {
        let id = c.flat_id(k).unwrap();
        calls
            .iter()
            .find_map(|x| match x {
                SinkCall::Insert { id: i, params, .. } if *i == id => {
                    Some(params.iter().map(|(p, v)| (p.0, *v)).collect())
                }
                _ => None,
            })
            .unwrap()
    };
    let cutoff = param_id(SYNTH, "filter.cutoff").0;
    assert!(insert_params(&key(&format!("{}/low", copy.0))).contains(&(cutoff, 1800.0)));
    assert!(
        insert_params(&key(&format!("{}/~split", copy.0)))
            .contains(&(param_id(SPLIT_TYPE, "s1.on").0, 1.0))
    );
    assert!(c.undo());
    assert!(copy_keys.iter().all(|k| c.flat_id(k).is_none()));
    for (k, id) in &before {
        assert_eq!(c.flat_id(k), Some(*id), "{k}");
    }
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
}
