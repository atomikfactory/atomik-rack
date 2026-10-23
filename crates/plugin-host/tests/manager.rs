//! (manager half): the main-thread owner of the plugin
//! instances over the `NullBackend`: instantiation, restart hand-shake, reaper, state, engine
//! restarts that keep the instances, and the messages of the plugins.
// Justification: exact copies of known samples.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_device_sdk::{Device, PluginFormat, PluginRef, PluginStateBlob, RunMode};
use atomik_plugin_host::{
    HostMessenger, InstantiateRequest, LogLevel, ManagerError, ManagerEvent, NullBackend,
    NullBehavior, NullPlugin, NullStats, ParamPhase, PluginHandle, PluginManager, PluginMessage,
    PluginProvider, mark_main_thread,
};
use atomik_testkit::harness::DeviceHarness;
use std::path::PathBuf;
use std::sync::Arc;
use support::*;

fn plugin(id: &str) -> NullPlugin {
    NullPlugin {
        path: PathBuf::from(format!("{id}.clap")),
        plugin: PluginRef {
            format: PluginFormat::Clap,
            id: id.to_string(),
        },
        layout: fx_layout(),
        behavior: NullBehavior::Gain(0.5),
        latency: 17,
    }
}

fn request(id: &str) -> InstantiateRequest {
    InstantiateRequest {
        plugin: PluginRef {
            format: PluginFormat::Clap,
            id: id.to_string(),
        },
        state: None,
        sample_rate: SR,
        mode: RunMode::Realtime,
        pinned: false,
        powered: true,
    }
}

fn manager() -> (PluginManager, Arc<NullStats>) {
    mark_main_thread();
    let backend = NullBackend::new()
        .with_plugin(plugin("null.fx"))
        .with_plugin(plugin("null.fx2"));
    let stats = backend.stats();
    let mut m = PluginManager::new();
    m.add_backend(Box::new(backend));
    (m, stats)
}

fn run(h: &mut DeviceHarness, n: usize) {
    for _ in 0..n {
        h.process();
    }
}

fn harness_of(handle: PluginHandle) -> (DeviceHarness, atomik_plugin_host::InstanceId) {
    let id = handle.id;
    (DeviceHarness::new(Box::new(handle.device), SR), id)
}

// ----- instantiate -------------------------------------------------------------------------------

#[test]
fn instantiating_gives_a_device_a_layout_and_a_live_slot() {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    assert_eq!(handle.layout, fx_layout());
    assert_eq!(
        handle.descriptor.sockets.len(),
        6,
        "in, sidechain and out sockets of an effect"
    );
    assert_eq!((stats.created(), stats.activated()), (1, 1));
    assert_eq!(m.instance_count(), 1);
    assert_eq!(
        m.slot(handle.id).unwrap().latency(),
        17,
        "the reported latency"
    );
    assert!(handle.restore_error.is_none());
    let id = handle.id;
    let (mut h, _) = harness_of(handle);
    *h.input_mut("in_l") = [1.0; BATCH];
    h.process();
    assert_eq!(
        h.output("out_l")[0],
        0.5,
        "the plugin runs behind the device"
    );
    assert!(m.contains(id));
}

#[test]
fn each_instance_gets_its_own_id() {
    let (mut m, _) = manager();
    let a = m.instantiate(&request("null.fx")).unwrap();
    let b = m.instantiate(&request("null.fx")).unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(m.instance_count(), 2);
}

#[test]
fn a_format_without_a_backend_and_an_unknown_plugin_are_errors() {
    let (mut m, stats) = manager();
    let mut vst = request("null.fx");
    vst.plugin.format = PluginFormat::Vst3;
    let err = m.instantiate(&vst).map(|_| ()).unwrap_err();
    assert!(
        matches!(err, ManagerError::NoBackend(PluginFormat::Vst3)),
        "{err:?}"
    );
    let err = m.instantiate(&request("no.such")).map(|_| ()).unwrap_err();
    assert!(matches!(err, ManagerError::Backend(_)), "{err:?}");
    assert_eq!(stats.created(), 0);
    assert_eq!(m.instance_count(), 0);
}

#[test]
fn the_pinned_and_powered_settings_reach_the_slot() {
    let (mut m, _) = manager();
    let mut r = request("null.fx");
    r.pinned = true;
    r.powered = false;
    let handle = m.instantiate(&r).unwrap();
    assert!(handle.device.pinned_to_audio_thread());
    assert!(!m.slot(handle.id).unwrap().powered());
}

#[test]
fn the_provider_trait_is_the_manager() {
    fn make(p: &mut dyn PluginProvider) -> PluginHandle {
        p.instantiate(&request("null.fx")).unwrap()
    }
    let (mut m, _) = manager();
    let handle = make(&mut m);
    assert!(m.contains(handle.id));
}

// ----- state -----------------------------------------------------------------------------------------

#[test]
fn a_saved_state_is_restored_before_the_device_is_built_and_captured_back() {
    let (mut m, _) = manager();
    let mut r = request("null.fx");
    r.state = Some(PluginStateBlob(vec![1, 2, 3, 4]));
    let handle = m.instantiate(&r).unwrap();
    assert!(handle.restore_error.is_none());
    assert_eq!(
        m.capture_state(handle.id).unwrap(),
        PluginStateBlob(vec![1, 2, 3, 4])
    );
    m.restore_state(handle.id, &PluginStateBlob(vec![9]))
        .unwrap();
    let all = m.capture_all();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0, handle.id);
    assert_eq!(all[0].1.as_ref().unwrap(), &PluginStateBlob(vec![9]));
}

#[test]
fn the_state_of_an_unknown_instance_is_an_error() {
    let (mut m, _) = manager();
    let err = m
        .capture_state(atomik_plugin_host::InstanceId(99))
        .unwrap_err();
    assert!(matches!(err, ManagerError::UnknownInstance(_)), "{err:?}");
}

#[test]
fn param_text_comes_from_the_plugin() {
    let (mut m, _) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    assert_eq!(m.param_text(handle.id, 1, 0.5).as_deref(), Some("0.5"));
    assert_eq!(
        m.param_text(atomik_plugin_host::InstanceId(99), 1, 0.5),
        None
    );
}

// ----- restart ---------------------------------------------------------------------------------------

#[test]
fn a_restart_hands_the_processor_back_reactivates_it_and_the_device_resumes() {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let (mut h, id) = harness_of(handle);
    *h.input_mut("in_l") = [1.0; BATCH];
    run(&mut h, 2);
    m.restart(id).unwrap();
    // The device answers at its next batch; until then nothing has happened.
    assert!(m.poll().is_empty());
    h.process(); // hands the processor back, outputs dry
    assert_eq!(h.output("out_l")[0], 1.0, "dry while restarting");
    let events = m.poll();
    assert_eq!(events, vec![ManagerEvent::Restarted(id)]);
    assert_eq!((stats.activated(), stats.deactivated()), (2, 1));
    run(&mut h, 2);
    assert_eq!(h.output("out_l")[0], 0.5, "the plugin is back");
    assert_eq!(stats.created(), 1, "the instance was not recreated");
}

#[test]
fn a_new_sample_rate_restarts_every_instance_at_that_rate() {
    let (mut m, stats) = manager();
    let (mut h1, id1) = harness_of(m.instantiate(&request("null.fx")).unwrap());
    let (mut h2, id2) = harness_of(m.instantiate(&request("null.fx2")).unwrap());
    m.set_sample_rate(44_100.0);
    run(&mut h1, 2);
    run(&mut h2, 2);
    let mut events = m.poll();
    events.sort_by_key(|e| match e {
        ManagerEvent::Restarted(i) => i.0,
        _ => u64::MAX,
    });
    assert_eq!(
        events,
        vec![ManagerEvent::Restarted(id1), ManagerEvent::Restarted(id2)]
    );
    assert_eq!(stats.last_sample_rate(), 44_100.0);
    assert_eq!(stats.created(), 2);
}

#[test]
fn a_restart_request_from_the_plugin_or_the_device_restarts_it() {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let messenger: HostMessenger = m.slot(handle.id).unwrap().messenger();
    let (mut h, id) = harness_of(handle);
    h.process();
    messenger.post(PluginMessage::RequestRestart);
    assert!(m.poll().is_empty(), "the restart begins");
    run(&mut h, 2);
    assert_eq!(m.poll(), vec![ManagerEvent::Restarted(id)]);
    assert_eq!(stats.activated(), 2);
}

#[test]
fn a_latency_change_is_published_after_the_restart() {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let messenger = m.slot(handle.id).unwrap().messenger();
    let (mut h, id) = harness_of(handle);
    h.process();
    stats.set_latency(250);
    messenger.post(PluginMessage::LatencyChanged);
    m.poll();
    run(&mut h, 2);
    let events = m.poll();
    assert!(events.contains(&ManagerEvent::Restarted(id)), "{events:?}");
    assert!(
        events.contains(&ManagerEvent::LatencyChanged {
            instance: id,
            frames: 250
        }),
        "{events:?}"
    );
    assert_eq!(m.slot(id).unwrap().latency(), 250);
}

#[test]
fn reload_restarts_a_failed_device() {
    let (mut m, _) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let (mut h, id) = harness_of(handle);
    h.process();
    m.reload(id).unwrap();
    run(&mut h, 2);
    assert_eq!(m.poll(), vec![ManagerEvent::Restarted(id)]);
}

#[test]
fn a_restart_that_cannot_reactivate_is_reported() {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let (mut h, id) = harness_of(handle);
    h.process();
    stats.fail_next_activation();
    m.restart(id).unwrap();
    run(&mut h, 2);
    let events = m.poll();
    assert!(
        matches!(events.as_slice(), [ManagerEvent::RestartFailed { instance, .. }] if *instance == id),
        "{events:?}"
    );
    // The device keeps outputting dry while it has no processor.
    *h.input_mut("in_l") = [0.25; BATCH];
    run(&mut h, 2);
    assert_eq!(h.output("out_l")[0], 0.25);
}

// ----- the reaper -------------------------------------------------------------------------------------

#[test]
fn a_dropped_device_is_deactivated_and_destroyed_on_the_next_poll() {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let id = handle.id;
    let (mut h, _) = harness_of(handle);
    h.process();
    drop(h);
    assert_eq!(stats.destroyed(), 0, "dropping the device destroys nothing");
    let events = m.poll();
    assert_eq!(events, vec![ManagerEvent::Destroyed(id)]);
    assert_eq!((stats.deactivated(), stats.destroyed()), (1, 1));
    assert_eq!(m.instance_count(), 0);
}

#[test]
fn a_device_dropped_while_restarting_is_still_torn_down() {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let id = handle.id;
    let (mut h, _) = harness_of(handle);
    h.process();
    m.restart(id).unwrap();
    h.process(); // the processor is now with the manager's ring
    drop(h);
    let events = m.poll();
    assert!(events.contains(&ManagerEvent::Destroyed(id)), "{events:?}");
    assert_eq!(stats.destroyed(), 1);
    assert_eq!(stats.activated(), stats.deactivated());
}

// The guard is a `debug_assert!` (`threads::assert_main_thread`): release builds do not check.
#[cfg(debug_assertions)]
#[test]
fn every_manager_entry_point_refuses_to_run_off_the_main_thread() {
    // A thread that never called `mark_main_thread`: the guard trips (debug builds).
    let result = std::thread::spawn(|| {
        let mut m = PluginManager::new();
        m.poll();
    })
    .join();
    assert!(
        result.is_err(),
        "poll must refuse to run off the main thread"
    );
}

// ----- engine restarts keep the instances -------------------------------------------------------------

#[test]
fn a_retained_instance_survives_its_device_and_is_reattached_without_recreation() {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let id = handle.id;
    let (mut h, _) = harness_of(handle);
    h.process();
    m.retain(id, true);
    drop(h); // the engine is torn down
    assert!(m.poll().is_empty(), "retained: no Destroyed event");
    assert_eq!(stats.deactivated(), 1);
    assert_eq!(stats.destroyed(), 0);
    assert!(m.contains(id));
    let again = m.reattach(id, 44_100.0).unwrap();
    assert_eq!(again.id, id);
    assert_eq!(stats.created(), 1, "the instance counter is unchanged");
    assert_eq!(stats.activated(), 2);
    assert_eq!(stats.last_sample_rate(), 44_100.0);
    let (mut h, _) = harness_of(again);
    *h.input_mut("in_l") = [1.0; BATCH];
    h.process();
    assert_eq!(h.output("out_l")[0], 0.5);
}

#[test]
fn reattaching_a_live_instance_or_an_unknown_one_is_an_error() {
    let (mut m, _) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let err = m.reattach(handle.id, SR).map(|_| ()).unwrap_err();
    assert!(matches!(err, ManagerError::NotParked(_)), "{err:?}");
    let err = m
        .reattach(atomik_plugin_host::InstanceId(77), SR)
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(err, ManagerError::UnknownInstance(_)), "{err:?}");
}

#[test]
fn a_parked_instance_can_be_removed_for_good() {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let id = handle.id;
    m.retain(id, true);
    drop(handle.device);
    m.poll();
    m.remove(id).unwrap();
    assert_eq!(stats.destroyed(), 1);
    assert!(!m.contains(id));
}

// ----- messages --------------------------------------------------------------------------------------------

#[test]
fn plugin_messages_become_manager_events_tagged_with_the_instance() {
    let (mut m, _) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let id = handle.id;
    let messenger = m.slot(id).unwrap().messenger();
    for msg in [
        PluginMessage::ParamEdit {
            id: 3,
            value: 0.25,
            phase: ParamPhase::Change,
        },
        PluginMessage::StateDirty,
        PluginMessage::GuiResize { w: 400, h: 300 },
        PluginMessage::GuiClosed,
        PluginMessage::GuiRequestShow,
        PluginMessage::GuiRequestHide,
        PluginMessage::Log {
            level: LogLevel::Warning,
            text: "hello".to_string(),
        },
        PluginMessage::Failed { reason: "boom" },
    ] {
        assert!(messenger.post(msg));
    }
    let events = m.poll();
    assert_eq!(
        events,
        vec![
            ManagerEvent::ParamEdit {
                instance: id,
                param: 3,
                value: 0.25,
                phase: ParamPhase::Change
            },
            ManagerEvent::StateDirty(id),
            ManagerEvent::GuiResize {
                instance: id,
                w: 400,
                h: 300
            },
            ManagerEvent::GuiClosed(id),
            ManagerEvent::GuiRequestShow(id),
            ManagerEvent::GuiRequestHide(id),
            ManagerEvent::Log {
                instance: id,
                level: LogLevel::Warning,
                text: "hello".to_string()
            },
            ManagerEvent::Failed {
                instance: id,
                reason: "boom".to_string()
            },
        ]
    );
    assert!(m.poll().is_empty(), "drained");
}

#[test]
fn notes_still_reach_a_managed_device() {
    // A smoke test over the whole stack: a managed synth device plays a note.
    mark_main_thread();
    let mut p = plugin("null.synth");
    p.layout = synth_layout();
    p.behavior = NullBehavior::Gain(1.0);
    let backend = NullBackend::new().with_plugin(p);
    let mut m = PluginManager::new();
    m.add_backend(Box::new(backend));
    let handle = m.instantiate(&request("null.synth")).unwrap();
    let (mut h, _) = harness_of(handle);
    h.push_note(
        "notes_in",
        NoteEvent {
            offset: 0,
            kind: NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        },
    );
    h.process();
}

// ----- sandboxed (lost) instances ------------------------------------------------------------------

#[test]
fn a_process_exit_message_becomes_a_crashed_event_with_its_reason() {
    let (mut m, _) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let id = handle.id;
    let messenger = m.slot(id).unwrap().messenger();
    messenger.post(PluginMessage::ProcessExited {
        reason: "exited with code 3".to_string(),
    });
    assert_eq!(
        m.poll(),
        vec![ManagerEvent::Crashed {
            instance: id,
            reason: "exited with code 3".to_string()
        }]
    );
}

#[test]
fn restarting_or_reloading_a_lost_instance_is_an_error_and_nothing_happens() {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let (mut h, id) = harness_of(handle);
    h.process();
    assert!(!m.is_lost(id));
    stats.set_lost(true);
    assert!(m.is_lost(id));
    for result in [m.restart(id), m.reload(id)] {
        let err = result.unwrap_err();
        assert!(
            matches!(&err, ManagerError::Backend(e) if e.to_string() == atomik_plugin_host::LOST),
            "{err:?}"
        );
    }
    run(&mut h, 3);
    assert!(m.poll().is_empty(), "no restart began");
    assert_eq!(stats.activated(), 1);
    assert!(!m.slot(id).unwrap().stop_requested());
}

#[test]
fn a_lost_instance_is_not_restarted_by_a_latency_change_or_a_new_sample_rate() {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let messenger = m.slot(handle.id).unwrap().messenger();
    let (mut h, id) = harness_of(handle);
    h.process();
    stats.set_lost(true);
    messenger.post(PluginMessage::LatencyChanged);
    messenger.post(PluginMessage::RequestRestart);
    m.poll();
    m.set_sample_rate(44_100.0);
    run(&mut h, 3);
    m.poll();
    assert_eq!(stats.activated(), 1, "never re-activated");
    assert!(!m.slot(id).unwrap().stop_requested());
}

#[test]
fn the_editor_kind_and_the_external_editor_route_to_the_instance() {
    use atomik_plugin_host::{EditorKind, NativeParent};
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request("null.fx")).unwrap();
    let id = handle.id;
    assert_eq!(m.editor_kind(id), Some(EditorKind::Embedded));
    assert!(m.open_external_editor(id, None).is_err());
    stats.set_external_editor(true);
    assert_eq!(m.editor_kind(id), Some(EditorKind::External));
    m.open_external_editor(id, Some(NativeParent::Win32(7)))
        .unwrap();
    assert_eq!(stats.external_opens(), 1);
    assert_eq!(m.editor_kind(atomik_plugin_host::InstanceId(999)), None);
}
