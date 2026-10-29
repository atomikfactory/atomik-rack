//! `AppCore` unit tests with the recording sink.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_engine::{EngineError, Graph};
use atomik_model::{Cable, Command, DeviceKey, SocketRef};
use atomik_ui::UiAction;

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";
const MIXER: &str = "atomik.line-mixer";

fn id(n: u32) -> DeviceInstanceId {
    DeviceInstanceId(n)
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(device: &str, socket: &str) -> SocketRef {
    SocketRef {
        device: key(device),
        socket: socket.to_string(),
    }
}

fn cable(from: (&str, &str), to: (&str, &str)) -> Cable {
    Cable {
        from: sref(from.0, from.1),
        to: sref(to.0, to.1),
    }
}

fn add(type_id: &str, k: &str) -> Command {
    Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.to_string(),
        key: Some(key(k)),
    }
}

/// A core with the start-up calls already taken.
fn core() -> AppCore<RecordingSink> {
    let mut c = rack_only_core();
    c.sink_mut().take();
    c
}

fn apply(c: &mut AppCore<RecordingSink>, cmd: Command) -> Vec<SinkCall> {
    c.apply_command(cmd).expect("command accepted");
    c.sink_mut().take()
}

fn param_id(c: &AppCore<RecordingSink>, type_id: &str, param: &str) -> ParamId {
    c.registry()
        .get(type_id)
        .unwrap()
        .descriptor
        .param_by_key(param)
        .unwrap()
        .id
}

fn routing_contains(call: &SinkCall, ids: &[DeviceInstanceId]) -> bool {
    matches!(call, SinkCall::Routing(order) if ids.iter().all(|i| order.contains(i)))
}

fn demo_v2() -> &'static str {
    include_str!("../../../examples/patches/slice-demo.v2.json")
}

fn demo_v1() -> &'static str {
    include_str!("../../../examples/patches/slice-demo.json")
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

// ------------------------------------------------------------------ start-up

#[test]
fn new_core_resets_the_engine_for_the_empty_document_and_routes_once() {
    let mut c = rack_only_core();
    let mut calls = c.sink_mut().take();
    // The song, the engine loop and the metronome follow (app_core_song_tests).
    calls.truncate(2);
    assert_eq!(
        calls,
        vec![
            SinkCall::Reset {
                sample_rate: 48_000.0,
                tempo_bpm: 120.0,
                output_channels: 2,
                seed: 0,
            },
            SinkCall::Routing(vec![]),
        ]
    );
    assert!(c.doc().devices.is_empty());
    assert!(!c.is_dirty());
}

// ------------------------------------------------------------------ SyncOp paths

#[test]
fn adding_a_device_inserts_it_with_id_1_then_routes_it() {
    let mut c = core();
    let calls = apply(&mut c, add(SYNTH, "synth"));
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(
        calls[0],
        SinkCall::Insert {
            id: id(1),
            type_id: SYNTH,
            params: vec![],
        }
    );
    assert!(routing_contains(&calls[1], &[id(1)]), "{calls:?}");
    assert_eq!(c.device_id(&key("synth")), Some(id(1)));
    assert_eq!(c.topology().node_ids().collect::<Vec<_>>(), vec![id(1)]);
}

#[test]
fn ids_are_monotonic_and_never_reused() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "a"));
    apply(&mut c, add(DELAY, "b"));
    apply(&mut c, Command::RemoveDevice { key: key("a") });
    let calls = apply(&mut c, add(MIXER, "c"));
    assert!(matches!(calls[0], SinkCall::Insert { id: i, .. } if i == id(3)));
    assert_eq!(c.device_id(&key("a")), None);
    assert_eq!(c.device_id(&key("b")), Some(id(2)));
    assert_eq!(c.device_id(&key("c")), Some(id(3)));
}

#[test]
fn removing_a_device_submits_routing_without_it_before_removing_it() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "synth"));
    apply(&mut c, add(DELAY, "delay"));
    let calls = apply(&mut c, Command::RemoveDevice { key: key("synth") });
    assert_eq!(
        calls,
        vec![SinkCall::Routing(vec![id(2)]), SinkCall::Remove(id(1))]
    );
    assert_eq!(c.topology().node_ids().collect::<Vec<_>>(), vec![id(2)]);
}

#[test]
fn connecting_a_cable_submits_routing_and_mirrors_it_in_the_topology() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "synth"));
    apply(&mut c, add(MIXER, "mixer"));
    let calls = apply(
        &mut c,
        Command::Connect(cable(("synth", "out_l"), ("mixer", "ch1_in_l"))),
    );
    assert_eq!(calls.len(), 1);
    assert!(routing_contains(&calls[0], &[id(1), id(2)]));
    let cables: Vec<_> = c.topology().cables().copied().collect();
    assert_eq!(cables.len(), 1);
    assert_eq!(cables[0].from.device, id(1));
    assert_eq!(cables[0].to.device, id(2));
    let synth = c.registry().get(SYNTH).unwrap().descriptor;
    assert_eq!(
        cables[0].from.socket,
        synth.socket_by_key("out_l").unwrap().id
    );
}

#[test]
fn a_cable_to_the_interface_targets_the_interface_endpoint() {
    let mut c = core();
    apply(&mut c, add(MIXER, "mixer"));
    let calls = apply(
        &mut c,
        Command::Connect(cable(("mixer", "master_l"), ("interface", "out_1"))),
    );
    assert_eq!(calls.len(), 1);
    let cables: Vec<_> = c.topology().cables().copied().collect();
    assert_eq!(cables.len(), 1);
    assert_eq!(cables[0].to.device, Graph::INTERFACE);
    // The default document has two host inputs (`in_1`, `in_2`) before `out_1`.
    let iface = atomik_engine::interface_descriptor(2, 2);
    assert_eq!(
        cables[0].to.socket,
        iface.socket_by_key("out_1").unwrap().id
    );
}

#[test]
fn disconnecting_a_cable_submits_routing_without_it() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "synth"));
    apply(&mut c, add(MIXER, "mixer"));
    apply(
        &mut c,
        Command::Connect(cable(("synth", "out_l"), ("mixer", "ch1_in_l"))),
    );
    let calls = apply(
        &mut c,
        Command::Disconnect {
            to: sref("mixer", "ch1_in_l"),
        },
    );
    assert_eq!(calls.len(), 1);
    assert!(matches!(calls[0], SinkCall::Routing(_)));
    assert_eq!(c.topology().cables().count(), 0);
}

#[test]
fn setting_a_param_sends_it_to_the_device_id() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "synth"));
    let calls = apply(
        &mut c,
        Command::SetParam {
            key: key("synth"),
            param: "filter.cutoff".into(),
            value: 1000.0,
            merge: None,
        },
    );
    assert_eq!(
        calls,
        vec![SinkCall::Param {
            id: id(1),
            param: param_id(&c, SYNTH, "filter.cutoff"),
            plain: 1000.0,
        }]
    );
}

#[test]
fn setting_the_tempo_sends_it() {
    let mut c = core();
    let calls = apply(&mut c, Command::SetTempo(140.0));
    // Then the song is recompiled at the new tempo (app_core_song_tests).
    assert_eq!(calls[0], SinkCall::Tempo(140.0));
}

#[test]
fn view_only_edits_send_nothing() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "a"));
    apply(&mut c, add(DELAY, "b"));
    for cmd in [
        Command::SetFolded {
            key: key("a"),
            folded: true,
        },
        Command::Rename {
            key: key("a"),
            name: "Lead".into(),
        },
        Command::SetColor {
            key: key("a"),
            color: Some([1, 2, 3]),
        },
        Command::MoveDevice {
            key: key("a"),
            to: 1,
        },
    ] {
        assert_eq!(apply(&mut c, cmd), vec![]);
    }
}

// ------------------------------------------------------------------ undo / redo

#[test]
fn undoing_an_add_routes_then_removes() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "synth"));
    assert!(c.undo());
    assert_eq!(
        c.sink_mut().take(),
        vec![SinkCall::Routing(vec![]), SinkCall::Remove(id(1))]
    );
    assert!(c.doc().devices.is_empty());
}

#[test]
fn redoing_an_add_inserts_under_a_fresh_id() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "synth"));
    c.undo();
    c.sink_mut().take();
    assert!(c.redo());
    let calls = c.sink_mut().take();
    assert!(
        matches!(calls[0], SinkCall::Insert { id: i, .. } if i == id(2)),
        "{calls:?}"
    );
    assert!(routing_contains(&calls[1], &[id(2)]));
}

#[test]
fn undoing_a_remove_reinserts_with_stored_params_and_restores_cables() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "synth"));
    apply(&mut c, add(MIXER, "mixer"));
    apply(
        &mut c,
        Command::SetParam {
            key: key("synth"),
            param: "filter.cutoff".into(),
            value: 1000.0,
            merge: None,
        },
    );
    apply(
        &mut c,
        Command::Connect(cable(("synth", "out_l"), ("mixer", "ch1_in_l"))),
    );
    apply(&mut c, Command::RemoveDevice { key: key("synth") });
    assert!(c.undo());
    let calls = c.sink_mut().take();
    assert_eq!(calls.len(), 2, "params travel with the insert: {calls:?}");
    assert_eq!(
        calls[0],
        SinkCall::Insert {
            id: id(3),
            type_id: SYNTH,
            params: vec![(param_id(&c, SYNTH, "filter.cutoff"), 1000.0)],
        }
    );
    assert!(routing_contains(&calls[1], &[id(2), id(3)]));
    let cables: Vec<_> = c.topology().cables().copied().collect();
    assert_eq!(cables.len(), 1);
    assert_eq!(cables[0].from.device, id(3));
}

#[test]
fn undoing_a_first_param_set_sends_the_default() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "synth"));
    apply(
        &mut c,
        Command::SetParam {
            key: key("synth"),
            param: "filter.cutoff".into(),
            value: 1000.0,
            merge: None,
        },
    );
    c.undo();
    let default = c
        .registry()
        .get(SYNTH)
        .unwrap()
        .descriptor
        .param_by_key("filter.cutoff")
        .unwrap()
        .default;
    assert_eq!(
        c.sink_mut().take(),
        vec![SinkCall::Param {
            id: id(1),
            param: param_id(&c, SYNTH, "filter.cutoff"),
            plain: default,
        }]
    );
}

#[test]
fn undo_and_redo_report_false_when_history_is_empty() {
    let mut c = core();
    assert!(!c.undo());
    assert!(!c.redo());
    assert_eq!(c.sink_mut().take(), vec![]);
}

#[test]
fn a_drag_merges_into_one_undo_step_until_end_merge() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "synth"));
    for v in [500.0, 600.0, 700.0] {
        c.handle_action(UiAction::Command(Command::SetParam {
            key: key("synth"),
            param: "filter.cutoff".into(),
            value: v,
            merge: Some(7),
        }));
    }
    c.handle_action(UiAction::EndMerge);
    c.sink_mut().take();
    assert!(c.undo());
    // One step back restores the value before the drag (the default); the add stays.
    assert_eq!(c.doc().devices.len(), 1);
    assert!(c.doc().devices[0].params.is_empty());
}

#[test]
fn ui_undo_and_redo_actions_sync_the_engine() {
    let mut c = core();
    apply(&mut c, Command::SetTempo(90.0));
    c.handle_action(UiAction::Undo);
    assert_eq!(c.sink_mut().take()[0], SinkCall::Tempo(120.0));
    c.handle_action(UiAction::Redo);
    assert_eq!(c.sink_mut().take()[0], SinkCall::Tempo(90.0));
}

// ------------------------------------------------------------------ rejections and errors

#[test]
fn a_rejected_command_notifies_the_view_and_sends_nothing() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "synth"));
    c.handle_action(UiAction::Command(Command::Connect(cable(
        ("synth", "out_l"),
        ("synth", "out_r"),
    ))));
    assert_eq!(c.sink_mut().take(), vec![]);
    let msg = c.last_rejection().expect("rejection recorded");
    assert!(msg.contains("cable rejected"), "{msg}");
    assert!(c.view().needs_repaint());
}

#[test]
fn apply_command_returns_the_model_error() {
    let mut c = core();
    let err = c
        .apply_command(Command::RemoveDevice { key: key("nope") })
        .unwrap_err();
    assert!(matches!(err, ModelError::UnknownDevice(_)));
}

#[test]
fn an_engine_error_is_reported_but_the_edit_stands() {
    let mut c = core();
    c.sink_mut().fail_next = Some(EngineError::QueueFull);
    c.apply_command(add(SYNTH, "synth")).unwrap();
    assert_eq!(c.doc().devices.len(), 1);
    let msg = c.last_rejection().expect("engine error reported");
    assert!(msg.contains("queue is full"), "{msg}");
}

// ------------------------------------------------------------------ files

#[test]
fn opening_a_v2_patch_inserts_ids_in_rack_order_with_params_then_routes_once() {
    let mut c = core();
    c.open_str(demo_v2(), None).unwrap();
    let calls = c.sink_mut().take();
    assert_eq!(
        calls[0],
        SinkCall::Reset {
            sample_rate: 48_000.0,
            tempo_bpm: 120.0,
            output_channels: 2,
            seed: 1,
        }
    );
    let types: Vec<_> = calls[1..5]
        .iter()
        .map(|call| match call {
            SinkCall::Insert { id, type_id, .. } => (id.0, *type_id),
            other => panic!("expected an insert, got {other:?}"),
        })
        .collect();
    assert_eq!(types, vec![(1, SYNTH), (2, DELAY), (3, SYNTH), (4, MIXER)]);
    assert!(routing_contains(&calls[5], &[id(1), id(2), id(3), id(4)]));
    // Then the (empty) song, the engine loop and the metronome.
    assert_eq!(calls.len(), 9);
    assert!(matches!(calls[6], SinkCall::Song(_)));
    let SinkCall::Insert { params, .. } = &calls[1] else {
        unreachable!()
    };
    assert!(params.contains(&(param_id(&c, SYNTH, "filter.cutoff"), 700.0)));
    assert_eq!(params.len(), c.doc().devices[0].params.len());
    assert_eq!(c.topology().cables().count(), 8);
    assert!(!c.is_dirty());
}

#[test]
fn opening_a_v1_patch_works_too() {
    let mut c = core();
    c.open_str(demo_v1(), None).unwrap();
    let calls = c.sink_mut().take();
    assert_eq!(
        calls
            .iter()
            .filter(|x| matches!(x, SinkCall::Insert { .. }))
            .count(),
        4
    );
    assert_eq!(
        calls
            .iter()
            .filter(|x| matches!(x, SinkCall::Routing(_)))
            .count(),
        1
    );
}

#[test]
fn opening_restarts_ids_at_1_and_clears_history() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "x"));
    apply(&mut c, add(SYNTH, "y"));
    c.open_str(demo_v2(), None).unwrap();
    assert_eq!(c.device_id(&key("bass")), Some(id(1)));
    assert_eq!(c.device_id(&key("mixer")), Some(id(4)));
    assert_eq!(c.device_id(&key("x")), None);
    assert!(!c.document().can_undo());
}

#[test]
fn a_bad_file_is_rejected_and_the_document_is_kept() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "synth"));
    let err = c.open_str("{ not json", None).unwrap_err();
    assert!(matches!(err, AppError::Model(_)), "{err}");
    assert_eq!(c.doc().devices.len(), 1);
    assert_eq!(c.sink_mut().take(), vec![]);
}

#[test]
fn opening_a_missing_file_is_an_io_error() {
    let mut c = core();
    let err = c.open(&temp_path("does-not-exist.json")).unwrap_err();
    assert!(matches!(err, AppError::Io { .. }), "{err}");
}

#[test]
fn save_without_a_path_asks_for_one() {
    let mut c = core();
    assert!(matches!(c.save(), Err(AppError::NoPath)));
}

#[test]
fn save_as_then_reopen_reloads_identically() {
    let path = temp_path("roundtrip.json");
    let mut c = core();
    c.open_str(demo_v2(), None).unwrap();
    apply(
        &mut c,
        Command::SetParam {
            key: key("lead"),
            param: "filter.cutoff".into(),
            value: 1234.5678,
            merge: None,
        },
    );
    assert!(c.is_dirty());
    c.save_as(&path).unwrap();
    assert!(!c.is_dirty());
    assert_eq!(c.file(), Some(path.as_path()));
    let before = c.doc().clone();
    c.sink_mut().take();
    let mut other = core();
    other.open(&path).unwrap();
    assert_eq!(other.doc(), &before, "E2-1: the file reloads identically");
    // And the engine is rebuilt the same way from the reloaded file.
    c.rebuild_engine();
    assert_eq!(other.sink_mut().take(), c.sink_mut().take());
    std::fs::remove_file(&path).ok();
}

#[test]
fn save_writes_to_the_current_file() {
    let path = temp_path("save.atomik");
    let mut c = core();
    c.save_as(&path).unwrap();
    apply(&mut c, add(DELAY, "delay"));
    assert!(c.is_dirty());
    c.save().unwrap();
    assert!(!c.is_dirty());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("atomik.stereo-delay"));
    std::fs::remove_file(&path).ok();
}

#[test]
fn new_document_resets_to_the_starter_project() {
    let mut c = core();
    c.open_str(demo_v2(), Some(temp_path("demo.json"))).unwrap();
    c.sink_mut().take();
    c.new_document();
    let calls = c.sink_mut().take();
    assert!(matches!(calls[0], SinkCall::Reset { seed: 0, .. }));
    // File, New is the starter project, Audio I/O and the master under it.
    assert_eq!(c.doc().devices.len(), 1);
    assert_eq!(c.doc().console.master, Some(DeviceKey("master".into())));
    assert_eq!(c.file(), None);
    assert_eq!(c.title(), "Atomik Rack \u{2014} Untitled");
}

#[test]
fn the_title_names_the_file_and_marks_unsaved_changes() {
    let mut c = core();
    c.open_str(
        demo_v2(),
        Some(std::path::PathBuf::from("patches/slice-demo.v2.json")),
    )
    .unwrap();
    assert_eq!(c.title(), "Atomik Rack \u{2014} slice-demo.v2.json");
    apply(&mut c, Command::SetTempo(100.0));
    assert_eq!(c.title(), "Atomik Rack \u{2014} slice-demo.v2.json *");
    c.undo();
    assert_eq!(c.title(), "Atomik Rack \u{2014} slice-demo.v2.json");
}

#[test]
fn rebuilding_reinserts_the_current_rack_with_ids_from_1() {
    let mut c = core();
    apply(&mut c, add(SYNTH, "a"));
    apply(&mut c, add(DELAY, "b"));
    apply(&mut c, Command::RemoveDevice { key: key("a") });
    apply(&mut c, Command::SetTempo(133.0));
    c.rebuild_engine();
    let calls = c.sink_mut().take();
    assert!(
        matches!(calls[0], SinkCall::Reset { tempo_bpm, .. } if (tempo_bpm - 133.0).abs() < 1e-9)
    );
    assert!(matches!(calls[1], SinkCall::Insert { id: i, type_id: DELAY, .. } if i == id(1)));
    assert_eq!(calls[2], SinkCall::Routing(vec![id(1)]));
    assert_eq!(c.device_id(&key("b")), Some(id(1)));
}

#[test]
fn each_frame_collects_garbage() {
    let mut c = core();
    c.frame();
    c.frame();
    assert_eq!(c.sink().garbage_collections, 2);
}

// ------------------------------------------------------------------ live MIDI target

#[test]
fn the_notes_target_is_the_first_device_with_a_notes_input() {
    let mut c = core();
    assert_eq!(c.notes_target(), None);
    apply(&mut c, add(DELAY, "delay"));
    assert_eq!(c.notes_target(), None);
    apply(&mut c, add(SYNTH, "synth"));
    let notes_in = c
        .registry()
        .get(SYNTH)
        .unwrap()
        .descriptor
        .socket_by_key("notes_in")
        .unwrap()
        .id;
    assert_eq!(c.notes_target(), Some((id(2), notes_in)));
    assert_eq!(c.note_target_cell().get(), Some((id(2), notes_in)));
}

#[test]
fn selecting_a_device_with_notes_retargets_live_notes() {
    let mut c = core();
    c.open_str(demo_v2(), None).unwrap();
    let first = c.notes_target().unwrap();
    assert_eq!(first.0, id(1), "bass is first");
    c.select(Some(key("lead")));
    assert_eq!(c.notes_target().unwrap().0, id(3));
    assert_eq!(c.note_target_cell().get().unwrap().0, id(3));
    // A device without notes input keeps the default (first) target.
    c.select(Some(key("echo")));
    assert_eq!(c.notes_target().unwrap().0, id(1));
}

#[test]
fn the_target_follows_rebuilds_and_removals() {
    let mut c = core();
    c.open_str(demo_v2(), None).unwrap();
    c.select(Some(key("lead")));
    apply(&mut c, Command::RemoveDevice { key: key("lead") });
    assert_eq!(c.notes_target().unwrap().0, id(1));
    c.select(Some(key("bass")));
    c.rebuild_engine();
    assert_eq!(c.notes_target().unwrap().0, id(1));
}

/// A core whose registry has no console devices, so instruments get no automatic channel:
/// these tests pin the sync rules device by device. The console sync
/// is tested with the full registry in `app_core_console_tests.rs`.
pub(super) fn rack_only_core() -> AppCore<RecordingSink> {
    AppCore::with_registry(
        RecordingSink::new(),
        without_console(&default_registry()),
        atomik_model::empty_doc(),
        None,
    )
}

#[test]
fn saving_and_opening_a_project_keep_the_recent_list() {
    let list = temp_path("recent-list.json");
    let a = temp_path("recent-a.atomik");
    let b = temp_path("recent-b.atomik");
    let patch = temp_path("recent-patch.json");
    let mut c = core();
    c.set_recent_path(list.clone());
    assert!(c.recent().is_empty());
    c.save_as(&a).unwrap();
    c.save_as(&b).unwrap();
    assert_eq!(c.recent(), [b.clone(), a.clone()]);
    c.open(&a).unwrap();
    assert_eq!(c.recent(), [a.clone(), b.clone()], "opening moves it first");
    // A failed open and a patch leave the list alone.
    assert!(c.open(&temp_path("recent-missing.atomik")).is_err());
    c.open_str(demo_v2(), None).unwrap();
    c.export_patch(&patch).unwrap();
    c.open(&patch).unwrap();
    assert_eq!(c.recent(), [a.clone(), b.clone()]);
    // The list is kept in its file for the next run.
    let mut next = core();
    next.set_recent_path(list.clone());
    assert_eq!(next.recent(), [a.clone(), b.clone()]);
    next.forget_recent(&a);
    assert_eq!(next.recent(), [b.clone()]);
    next.clear_recent();
    let mut after = core();
    after.set_recent_path(list.clone());
    assert!(after.recent().is_empty());
    for f in [&list, &a, &b, &patch] {
        std::fs::remove_file(f).ok();
    }
}
