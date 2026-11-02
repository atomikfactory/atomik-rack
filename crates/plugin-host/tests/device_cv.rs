//! The CV programmer of a plugin device: eight CV inputs, each with a destination (a plugin
//! param) and a bipolar amount, read once per 64-frame batch and sent as ordinary param events
//! at the batch start. Nothing here allocates (the harness runs `process` under the guard).
// Justification: the params and the CV are exact values copied through f64.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::BATCH;
use atomik_plugin_host::{PluginEvent, PluginEventKind as K};
use atomik_testkit::harness::DeviceHarness;
use std::sync::Arc;
use support::*;

/// The synth with params Wave (id 1, stepped 0..1, descriptor index 1) and Level (id 2, 0..1,
/// descriptor index 2).
fn synth(id: &str) -> (DeviceHarness, Rest, Arc<ProbeShared>) {
    let (probe, shared) = Probe::new();
    let (h, r) = harness(rig(id, &synth_layout(), Box::new(probe)));
    (h, r, shared)
}

fn params_of(shared: &ProbeShared, call: usize) -> Vec<(u32, f64)> {
    shared
        .log
        .lock()
        .unwrap()
        .events_of_call(call)
        .iter()
        .filter_map(|e: &PluginEvent| match e.kind {
            K::Param { id, value } => Some((id, value)),
            _ => None,
        })
        .collect()
}

fn cv(h: &mut DeviceHarness, socket: &str, value: f32) {
    h.input_mut(socket).fill(value);
}

#[test]
fn a_cabled_cv_moves_its_destination_by_amount_times_cv_times_range() {
    let (mut h, _r, shared) = synth("cv.basic");
    // Level (descriptor index 2) rack value 0.5, CV 1 at +0.5 with amount 0.5.
    h.set_param("p2", 0.5);
    h.set_param("cv.dest_1", 2.0);
    h.set_param("cv.amount_1", 0.5);
    cv(&mut h, "cv_in_1", 0.5);
    h.process();
    assert_eq!(params_of(&shared, 0), [(2, 0.75)]);
    // A steady CV is not sent again.
    h.process();
    assert!(params_of(&shared, 1).is_empty());
    // A new CV level is.
    cv(&mut h, "cv_in_1", -1.0);
    h.process();
    assert_eq!(
        params_of(&shared, 2),
        [(2, 0.0)],
        "0.5 - 0.5 is the range floor"
    );
    // A bipolar amount inverts.
    h.set_param("cv.amount_1", -0.5);
    cv(&mut h, "cv_in_1", 0.5);
    h.process();
    assert_eq!(params_of(&shared, 3), [(2, 0.25)]);
}

#[test]
fn the_cv_of_each_batch_is_the_batch_mean_and_arrives_in_that_batch() {
    let (mut h, _r, shared) = synth("cv.mean");
    h.set_param("p2", 0.0);
    h.set_param("cv.dest_1", 2.0);
    h.set_param("cv.amount_1", 1.0);
    // Half the batch at 1.0, half at 0.0: the mean is 0.5. Non-finite samples count as 0 and
    // samples beyond 1 are clamped.
    {
        let b = h.input_mut("cv_in_1");
        for (i, s) in b.iter_mut().enumerate() {
            *s = if i < BATCH / 2 { 3.0 } else { f32::NAN };
        }
        b[BATCH - 1] = 0.0;
    }
    h.process();
    assert_eq!(params_of(&shared, 0), [(2, 0.5)]);
}

#[test]
fn an_unconnected_cv_input_does_nothing_and_a_cable_pulled_restores_the_rack_value() {
    let (mut h, _r, shared) = synth("cv.cable");
    h.set_param("p2", 0.25);
    h.set_param("cv.dest_1", 2.0);
    h.set_param("cv.amount_1", 1.0);
    h.process();
    // The rack value goes out once (it was set), no CV event.
    assert_eq!(params_of(&shared, 0), [(2, 0.25)]);
    cv(&mut h, "cv_in_1", 0.5);
    h.process();
    assert_eq!(params_of(&shared, 1), [(2, 0.75)]);
    h.set_connected("cv_in_1", false);
    h.process();
    assert_eq!(params_of(&shared, 2), [(2, 0.25)], "back to the rack value");
    h.process();
    assert!(params_of(&shared, 3).is_empty());
}

#[test]
fn an_amount_of_zero_or_no_destination_leaves_the_plugin_alone() {
    let (mut h, _r, shared) = synth("cv.idle");
    cv(&mut h, "cv_in_1", 1.0);
    cv(&mut h, "cv_in_2", 1.0);
    h.set_param("cv.amount_1", 1.0); // no destination
    h.set_param("cv.dest_2", 2.0); // amount 0
    for call in 0..3 {
        h.process();
        assert!(params_of(&shared, call).is_empty(), "batch {call}");
    }
}

#[test]
fn cvs_that_share_a_destination_add_and_the_sum_is_clamped_to_the_range() {
    let (mut h, _r, shared) = synth("cv.sum");
    h.set_param("p2", 0.5);
    for k in 1..=3 {
        h.set_param(&format!("cv.dest_{k}"), 2.0);
        h.set_param(&format!("cv.amount_{k}"), 0.25);
    }
    cv(&mut h, "cv_in_1", 1.0);
    cv(&mut h, "cv_in_2", 1.0);
    h.process();
    assert_eq!(params_of(&shared, 0), [(2, 1.0)], "0.5 + 0.25 + 0.25");
    cv(&mut h, "cv_in_3", 1.0);
    h.process();
    assert!(
        params_of(&shared, 1).is_empty(),
        "a sum past the range is clamped to what was already sent"
    );
    cv(&mut h, "cv_in_1", -1.0);
    cv(&mut h, "cv_in_2", -1.0);
    cv(&mut h, "cv_in_3", -1.0);
    h.process();
    assert_eq!(params_of(&shared, 2), [(2, 0.0)]);
}

#[test]
fn a_stepped_destination_is_rounded() {
    let (mut h, _r, shared) = synth("cv.stepped");
    // Wave (descriptor index 1) is stepped 0..1.
    h.set_param("cv.dest_1", 1.0);
    h.set_param("cv.amount_1", 1.0);
    cv(&mut h, "cv_in_1", 0.4);
    h.process();
    assert!(
        params_of(&shared, 0).is_empty(),
        "0.4 rounds to the value it has"
    );
    cv(&mut h, "cv_in_1", 0.6);
    h.process();
    assert_eq!(params_of(&shared, 1), [(1, 1.0)]);
}

#[test]
fn changing_the_destination_hands_the_old_one_back() {
    let (mut h, _r, shared) = synth("cv.move");
    h.set_param("p2", 0.5);
    h.set_param("cv.dest_1", 2.0);
    h.set_param("cv.amount_1", 0.5);
    cv(&mut h, "cv_in_1", 1.0);
    h.process();
    assert_eq!(params_of(&shared, 0), [(2, 1.0)]);
    h.set_param("cv.dest_1", 1.0);
    h.process();
    let e = params_of(&shared, 1);
    assert!(
        e.contains(&(2, 0.5)),
        "the old destination returns to the rack value: {e:?}"
    );
    assert!(
        e.contains(&(1, 1.0)),
        "the new one moves from its default by 0.5 of a range of 1, then rounds up: {e:?}"
    );
}

#[test]
fn a_destination_past_the_plugin_params_is_no_destination() {
    let (mut h, _r, shared) = synth("cv.range");
    // The plugin has two params: 3 and up name none (a value saved for a longer table).
    h.set_param("cv.dest_1", 9.0);
    h.set_param("cv.amount_1", 1.0);
    cv(&mut h, "cv_in_1", 1.0);
    for call in 0..2 {
        h.process();
        assert!(params_of(&shared, call).is_empty(), "batch {call}");
    }
    h.set_param("cv.dest_1", 2.0);
    h.process();
    assert_eq!(params_of(&shared, 2), [(2, 1.0)]);
}

#[test]
fn the_descriptor_carries_the_programmer_after_the_plugin_params() {
    let (h, _r, _s) = synth("cv.shape");
    let d = h.device().descriptor();
    for k in 1..=8 {
        let s = d.socket_by_key(&format!("cv_in_{k}")).expect("CV input");
        assert_eq!(s.kind, atomik_device_sdk::SocketKind::Cv);
        assert!(d.param_by_key(&format!("cv.dest_{k}")).is_some());
        assert!(d.param_by_key(&format!("cv.amount_{k}")).is_some());
    }
    assert_eq!(d.params.len(), 1 + 2 + 16);
}
