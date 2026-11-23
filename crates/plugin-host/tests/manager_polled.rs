//! (manager half): the live app never waits for a plugin. A creation restores and activates
//! one polled step at a time, a restart and an engine restart re-activate without a blocking
//! call, and an instance that goes away is deactivated and ended across polls. The null backend
//! counts polls instead of time, so none of this depends on how fast a machine is.
// Justification: exact copies of known samples.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::BATCH;
use atomik_device_sdk::{PluginFormat, PluginRef, PluginStateBlob, RunMode};
use atomik_plugin_host::{
    InstanceId, InstantiateRequest, ManagerEvent, NullBackend, NullBehavior, NullPlugin, NullStats,
    PluginHandle, PluginManager, mark_main_thread,
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

fn request(state: Option<Vec<u8>>) -> InstantiateRequest {
    InstantiateRequest {
        plugin: PluginRef {
            format: PluginFormat::Clap,
            id: "null.fx".to_string(),
        },
        state: state.map(PluginStateBlob),
        sample_rate: SR,
        mode: RunMode::Realtime,
        pinned: false,
        powered: true,
    }
}

fn manager() -> (PluginManager, Arc<NullStats>) {
    mark_main_thread();
    let backend = NullBackend::new().with_plugin(plugin("null.fx"));
    let stats = backend.stats();
    let mut m = PluginManager::new();
    m.add_backend(Box::new(backend));
    (m, stats)
}

fn harness_of(handle: PluginHandle) -> (DeviceHarness, InstanceId) {
    let id = handle.id;
    (DeviceHarness::new(Box::new(handle.device), SR), id)
}

/// Polls a creation until it answers (at most `limit` polls); returns the answer and the polls.
fn create(
    m: &mut PluginManager,
    request: &InstantiateRequest,
    limit: usize,
) -> (Result<PluginHandle, String>, usize) {
    let ticket = m.begin_create(request).unwrap();
    for polls in 1..=limit {
        if let Some(answer) = m.poll_create(ticket) {
            return (answer.map_err(|e| e.to_string()), polls);
        }
    }
    panic!("the creation did not end in {limit} polls");
}

#[test]
fn a_creation_restores_and_activates_one_polled_step_at_a_time() {
    let (mut m, stats) = manager();
    stats.set_background_create(Some(2));
    stats.set_restore_delay(3);
    stats.set_activate_delay(3);
    let ticket = m.begin_create(&request(Some(vec![1, 2, 3]))).unwrap();
    let mut polls = 0;
    let handle = loop {
        polls += 1;
        assert!(polls < 100, "the creation never ended");
        if let Some(answer) = m.poll_create(ticket) {
            break answer.unwrap();
        }
        // Until it is active the manager holds nothing the engine could use.
        assert_eq!(m.instance_count(), 0);
    };
    assert!(
        polls >= 6,
        "start, restore and activation each took their polls ({polls})"
    );
    assert!(handle.restore_error.is_none());
    assert_eq!((stats.created(), stats.activated()), (1, 1));
    assert_eq!(m.instance_count(), 1);
    assert_eq!(
        m.slot(handle.id).unwrap().latency(),
        17,
        "the reported latency"
    );
    assert_eq!(
        m.capture_state(handle.id).unwrap(),
        PluginStateBlob(vec![1, 2, 3]),
        "the saved state went in before the activation"
    );
    assert!(
        m.take_stalls().is_empty(),
        "nothing waited on the main thread"
    );
    let (mut h, _) = harness_of(handle);
    *h.input_mut("in_l") = [1.0; BATCH];
    h.process();
    h.process();
    assert_eq!(h.output("out_l")[0], 0.5, "the plugin plays");
}

#[test]
fn a_saved_state_that_cannot_be_restored_is_reported_and_the_plugin_still_activates() {
    let (mut m, stats) = manager();
    stats.set_background_create(Some(1));
    stats.set_restore_delay(2);
    stats.fail_next_restore();
    let (answer, _) = create(&mut m, &request(Some(vec![9])), 50);
    let handle = answer.unwrap();
    assert_eq!(
        handle.restore_error.as_deref(),
        Some("the null plugin could not restore")
    );
    assert_eq!(stats.activated(), 1);
}

#[test]
fn an_activation_that_fails_reports_what_the_blocking_creation_reports() {
    let (mut blocking, stats) = manager();
    stats.fail_next_activation();
    let wanted = blocking
        .instantiate(&request(None))
        .map(|_| ())
        .unwrap_err()
        .to_string();

    let (mut m, stats) = manager();
    stats.set_background_create(Some(1));
    stats.set_activate_delay(3);
    stats.fail_next_activation();
    let (answer, polls) = create(&mut m, &request(None), 50);
    assert_eq!(answer.map(|_| ()).unwrap_err(), wanted);
    assert!(polls > 3, "the failure came with the answer ({polls})");
    assert_eq!(m.instance_count(), 0);
    assert_eq!(stats.destroyed(), 1, "the instance that failed is gone");
}

#[test]
fn the_blocking_instantiation_of_an_offline_render_does_not_poll() {
    let (mut m, stats) = manager();
    stats.set_restore_delay(1_000);
    stats.set_activate_delay(1_000);
    stats.set_background_create(Some(1_000));
    let handle = m.instantiate(&request(Some(vec![5]))).unwrap();
    assert!(handle.restore_error.is_none());
    assert_eq!(
        m.capture_state(handle.id).unwrap(),
        PluginStateBlob(vec![5])
    );
    assert_eq!((stats.created(), stats.activated()), (1, 1));
}

#[test]
fn a_restart_with_a_slow_activation_does_not_block_and_the_device_is_dry_meanwhile() {
    let (mut m, stats) = manager();
    stats.set_activate_delay(4);
    let (mut h, id) = harness_of(m.instantiate(&request(None)).unwrap());
    *h.input_mut("in_l") = [1.0; BATCH];
    h.process();
    h.process();
    m.restart(id).unwrap();
    h.process(); // hands the processor back
    let mut events = Vec::new();
    for _ in 0..3 {
        events.extend(m.poll());
        h.process();
        assert!(!events.contains(&ManagerEvent::Restarted(id)), "{events:?}");
        assert_eq!(h.output("out_l")[0], 1.0, "dry while the plugin activates");
    }
    for _ in 0..10 {
        events.extend(m.poll());
    }
    assert_eq!(
        events
            .iter()
            .filter(|e| **e == ManagerEvent::Restarted(id))
            .count(),
        1,
        "{events:?}"
    );
    h.process();
    h.process();
    assert_eq!(h.output("out_l")[0], 0.5, "the plugin is back");
    assert!(m.take_stalls().is_empty());
}

#[test]
fn a_restart_asked_for_while_the_plugin_activates_restarts_it_again() {
    let (mut m, stats) = manager();
    stats.set_activate_delay(3);
    let (mut h, id) = harness_of(m.instantiate(&request(None)).unwrap());
    h.process();
    m.restart(id).unwrap();
    h.process();
    m.poll(); // deactivated at once, the activation is out
    m.set_sample_rate(44_100.0); // asked for while it activates at the old rate
    let mut restarted = 0;
    for _ in 0..40 {
        h.process();
        restarted += m
            .poll()
            .iter()
            .filter(|e| **e == ManagerEvent::Restarted(id))
            .count();
    }
    assert_eq!(restarted, 2, "once at the old rate, once at the new one");
    assert_eq!(stats.last_sample_rate(), 44_100.0);
}

/// Parks `n` instances behind slow deactivations and activations and reattaches them without
/// waiting; returns the polls it took until every one was active again.
fn rebuild(n: usize, polls_each: usize) -> usize {
    let (mut m, stats) = manager();
    let mut handles = Vec::new();
    for _ in 0..n {
        handles.push(m.instantiate(&request(None)).unwrap());
    }
    stats.set_deactivate_delay(polls_each);
    stats.set_activate_delay(polls_each);
    let ids: Vec<InstanceId> = handles.iter().map(|h| h.id).collect();
    for id in &ids {
        m.retain(*id, true);
    }
    drop(handles); // the engine is torn down
    let mut waiting = ids;
    let mut again = Vec::new();
    for polls in 1..200 {
        m.poll();
        waiting.retain(|id| match m.begin_reattach(*id, SR) {
            Ok(Some(handle)) => {
                again.push(handle);
                false
            }
            Ok(None) => true,
            Err(e) => {
                // Only the first poll can say so: the reaper has not parked the instance yet.
                assert!(
                    matches!(e, atomik_plugin_host::ManagerError::NotParked(_)),
                    "{e}"
                );
                true
            }
        });
        if waiting.is_empty() {
            assert_eq!(again.len(), n);
            assert_eq!(stats.created(), n, "no instance was created again");
            assert!(m.take_stalls().is_empty());
            return polls;
        }
    }
    panic!("the instances were not reattached");
}

#[test]
fn an_engine_restart_with_three_slow_plugins_costs_one_wait_not_three() {
    let one = rebuild(1, 6);
    let three = rebuild(3, 6);
    assert!(one >= 6, "the plugin was waited for ({one} polls)");
    assert_eq!(three, one, "pipelined: the slowest, not the sum");
}

#[test]
fn a_plugin_in_this_process_is_reattached_at_once() {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request(None)).unwrap();
    let id = handle.id;
    m.retain(id, true);
    drop(handle);
    m.poll();
    let again = m
        .begin_reattach(id, 44_100.0)
        .unwrap()
        .expect("activated at once");
    assert_eq!(again.id, id);
    assert_eq!(stats.last_sample_rate(), 44_100.0);
    assert_eq!((stats.created(), stats.activated()), (1, 2));
}

#[test]
fn reattaching_before_the_reaper_parked_the_instance_asks_again_later() {
    let (mut m, _) = manager();
    let handle = m.instantiate(&request(None)).unwrap();
    let id = handle.id;
    m.retain(id, true);
    let err = m.begin_reattach(id, SR).map(|_| ()).unwrap_err();
    assert!(matches!(
        err,
        atomik_plugin_host::ManagerError::NotParked(_)
    ));
    drop(handle);
}

#[test]
fn removing_a_parked_instance_returns_at_once_and_its_end_is_polled() {
    let (mut m, stats) = manager();
    stats.set_shutdown_delay(5);
    let handle = m.instantiate(&request(None)).unwrap();
    let id = handle.id;
    m.retain(id, true);
    drop(handle);
    m.poll();
    m.remove(id).unwrap();
    assert!(!m.contains(id));
    assert_eq!(m.leaving_count(), 1, "the process is still ending");
    assert_eq!(stats.destroyed(), 0);
    let mut events = Vec::new();
    for _ in 0..30 {
        events.extend(m.poll());
        if m.leaving_count() == 0 {
            break;
        }
    }
    assert_eq!(events, vec![ManagerEvent::Destroyed(id)]);
    assert_eq!(m.leaving_count(), 0);
    assert_eq!(stats.destroyed(), 1);
}

#[test]
fn a_dropped_device_is_destroyed_at_once_for_the_app_and_taken_down_across_polls() {
    let (mut m, stats) = manager();
    stats.set_deactivate_delay(3);
    stats.set_shutdown_delay(3);
    let handle = m.instantiate(&request(None)).unwrap();
    let id = handle.id;
    let (mut h, _) = harness_of(handle);
    h.process();
    drop(h);
    let events = m.poll();
    assert_eq!(events, vec![ManagerEvent::Destroyed(id)]);
    assert_eq!(m.instance_count(), 0);
    assert_eq!(m.leaving_count(), 1);
    assert_eq!(stats.destroyed(), 0, "the plugin is still deactivating");
    let mut polls = 0;
    while m.leaving_count() > 0 {
        polls += 1;
        assert!(polls < 50);
        assert!(m.poll().is_empty());
    }
    assert!(polls >= 3, "{polls}");
    assert_eq!((stats.deactivated(), stats.destroyed()), (1, 1));
}

#[test]
fn three_devices_dropped_together_leave_in_the_time_of_the_slowest() {
    let one = leave_together(1);
    let three = leave_together(3);
    assert!(one >= 4, "{one}");
    assert_eq!(three, one);
}

fn leave_together(n: usize) -> usize {
    let (mut m, stats) = manager();
    stats.set_deactivate_delay(4);
    stats.set_shutdown_delay(4);
    let handles: Vec<PluginHandle> = (0..n)
        .map(|_| m.instantiate(&request(None)).unwrap())
        .collect();
    drop(handles);
    m.poll();
    let mut polls = 0;
    while m.leaving_count() > 0 {
        polls += 1;
        assert!(polls < 100);
        m.poll();
    }
    assert_eq!(stats.destroyed(), n);
    polls
}

#[test]
fn a_creation_that_is_cancelled_while_it_restores_ends_across_polls() {
    let (mut m, stats) = manager();
    stats.set_background_create(Some(0));
    stats.set_restore_delay(50);
    stats.set_shutdown_delay(3);
    let ticket = m.begin_create(&request(Some(vec![1]))).unwrap();
    for _ in 0..3 {
        assert!(m.poll_create(ticket).is_none());
    }
    assert_eq!(stats.created(), 1);
    m.cancel_create(ticket);
    assert!(m.poll_create(ticket).is_none(), "the ticket is spent");
    assert_eq!(m.instance_count(), 0);
    assert_eq!(m.leaving_count(), 1);
    assert_eq!(stats.destroyed(), 0);
    for _ in 0..20 {
        m.poll();
    }
    assert_eq!(m.leaving_count(), 0);
    assert_eq!(stats.destroyed(), 1);
}

#[test]
fn dropping_the_manager_ends_every_process_together_within_a_bound() {
    let (mut m, stats) = manager();
    stats.set_shutdown_delay(20);
    let handles: Vec<PluginHandle> = (0..3)
        .map(|_| m.instantiate(&request(None)).unwrap())
        .collect();
    // One of them is already on its way out.
    let gone = handles.len();
    drop(handles);
    m.poll();
    assert_eq!(m.leaving_count(), gone);
    drop(m);
    assert_eq!(stats.destroyed(), gone);
}

#[test]
fn an_offline_instance_restores_activates_and_restarts_without_polling() {
    let (mut m, stats) = manager();
    stats.set_background_create(Some(0));
    stats.set_restore_delay(1_000);
    stats.set_activate_delay(1_000);
    let mut req = request(Some(vec![3]));
    req.mode = RunMode::Offline;
    let (answer, _) = create(&mut m, &req, 3);
    let handle = answer.unwrap();
    let (mut h, id) = harness_of(handle);
    h.process();
    m.restart(id).unwrap();
    h.process(); // hands the processor back
    assert_eq!(
        m.poll(),
        vec![ManagerEvent::Restarted(id)],
        "the restart is done at the batch it was asked for, whatever the plugin takes"
    );
}

/// A restart's activation at 44.1 kHz is in flight when the engine restarts and the instance is
/// parked; the reattach asks for `wanted`. Returns the stats and the polls until the handle.
fn reattach_during_activation(wanted: f64) -> Arc<NullStats> {
    let (mut m, stats) = manager();
    let handle = m.instantiate(&request(None)).unwrap();
    let (mut h, id) = harness_of(handle);
    h.process();
    stats.set_activate_delay(4);
    m.set_sample_rate(44_100.0);
    h.process(); // hands the processor back
    m.poll(); // deactivated at once, the activation at 44.1 kHz is out
    m.retain(id, true);
    drop(h);
    m.poll();
    let mut got = None;
    for _ in 0..60 {
        match m.begin_reattach(id, wanted) {
            Ok(Some(handle)) => {
                got = Some(handle);
                break;
            }
            Ok(None) | Err(atomik_plugin_host::ManagerError::NotParked(_)) => {}
            Err(e) => panic!("{e}"),
        }
        m.poll();
    }
    assert!(got.is_some(), "the instance was reattached");
    assert_eq!(
        stats.last_sample_rate(),
        wanted,
        "the processor handed out was activated at the wanted rate"
    );
    stats
}

#[test]
fn a_reattach_at_another_rate_than_the_activation_in_flight_activates_again_once() {
    let stats = reattach_during_activation(96_000.0);
    // Created, restarted, then once more for the new rate.
    assert_eq!((stats.activated(), stats.deactivated()), (3, 2));
}

#[test]
fn a_reattach_at_the_rate_of_the_activation_in_flight_adds_no_pair() {
    let stats = reattach_during_activation(44_100.0);
    assert_eq!((stats.activated(), stats.deactivated()), (2, 1));
}

#[test]
fn a_held_creation_does_not_stop_the_others_and_a_ready_one_is_installed_only_when_taken() {
    let (mut m, stats) = manager();
    stats.set_background_create(Some(1));
    stats.hold_next_creates(1);
    let held = m.begin_create(&request(None)).unwrap();
    let a = m.begin_create(&request(None)).unwrap();
    let b = m.begin_create(&request(None)).unwrap();
    let mut ready = [false; 3];
    for _ in 0..50 {
        ready = [held, a, b].map(|t| m.creation_ready(t) == Some(true));
    }
    assert_eq!(ready, [false, true, true], "only the held one is not ready");
    assert_eq!(m.instance_count(), 0, "being ready is not being installed");
    assert!(m.poll_create(held).is_none());
    // Taking a ready creation installs it; asking again after the answer finds a spent ticket.
    let first = m.poll_create(a).expect("ready").expect("starts");
    assert_eq!(m.instance_count(), 1);
    assert_eq!(m.creation_ready(a), None, "spent");
    let second = m.poll_create(b).expect("ready").expect("starts");
    assert_eq!(m.instance_count(), 2);
    assert_ne!(first.id, second.id);
    // The held one goes on once released.
    stats.release_held_creates();
    for _ in 0..50 {
        if m.creation_ready(held) == Some(true) {
            break;
        }
    }
    let third = m.poll_create(held).expect("ready").expect("starts");
    assert_eq!(m.instance_count(), 3);
    drop((first, second, third));
}

#[test]
fn a_ready_creation_that_is_cancelled_ends_its_plugin_across_polls() {
    let (mut m, stats) = manager();
    stats.set_background_create(Some(0));
    stats.set_shutdown_delay(3);
    let ticket = m.begin_create(&request(Some(vec![1]))).unwrap();
    for _ in 0..50 {
        if m.creation_ready(ticket) == Some(true) {
            break;
        }
    }
    assert_eq!(m.creation_ready(ticket), Some(true));
    assert_eq!(stats.created(), 1);
    m.cancel_create(ticket);
    assert_eq!(m.instance_count(), 0);
    assert_eq!(m.leaving_count(), 1);
    for _ in 0..50 {
        m.poll();
    }
    assert_eq!(m.leaving_count(), 0);
    assert_eq!(stats.destroyed(), 1);
}

#[test]
fn one_poll_asks_every_instance_before_it_polls_any_of_them() {
    let (mut m, stats) = manager();
    stats.record_polls(true);
    let mut kept = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..3 {
        let (h, id) = harness_of(m.instantiate(&request(None)).unwrap());
        kept.push(h);
        ids.push(id);
    }
    // The last one is parked: its device went away and the engine keeps it for a restart.
    m.retain(ids[2], true);
    drop(kept.pop());
    m.poll();
    let before = stats.poll_log().len();
    m.poll();
    let log = stats.poll_log();
    assert_eq!(
        log[before..],
        [
            (0, true),
            (1, true),
            (2, true),
            (0, false),
            (1, false),
            (2, false)
        ],
        "every ask comes before the first poll, and a parked instance is asked and polled too"
    );
}
