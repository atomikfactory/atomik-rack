//! (device, sandbox results) `ProcessStatus::Late` and `ProcessStatus::Lost`: a late batch has no
//! valid wet signal and is counted; a lost process crashes the device for good.
// Justification: the dry path copies known samples; the comparisons are exact.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::BATCH;
use atomik_plugin_host::{METER_VALUES, PluginMessage, ProcessStatus};
use support::*;

fn drain_messages(m: &atomik_plugin_host::ManagerEnd) -> Vec<PluginMessage> {
    std::iter::from_fn(|| m.try_recv()).collect()
}

fn meters(h: &atomik_testkit::harness::DeviceHarness) -> [f32; 8] {
    let mut m = [0.0f32; 8];
    h.device().meter_values(&mut m);
    m
}

// ----- Late ---------------------------------------------------------------------------------

#[test]
fn the_late_batches_are_the_last_meter_value() {
    assert_eq!(METER_VALUES, 6);
    let (probe, shared) = Probe::new();
    shared.set_status(ProcessStatus::Late);
    let (mut h, _r) = harness(rig("sb.meter", &fx_layout(), Box::new(probe)));
    let mut out = [0.0f32; 8];
    assert_eq!(h.device().meter_values(&mut out), METER_VALUES);
    assert_eq!(out[5], 0.0);
    for _ in 0..3 {
        h.process();
    }
    assert_eq!(meters(&h)[5], 3.0);
}

#[test]
fn a_late_batch_passes_the_dry_input_of_an_effect_and_is_not_a_failure() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Constant(0.5));
    shared.set_status(ProcessStatus::Late);
    let (mut h, r) = harness(rig("sb.late-fx", &fx_layout(), Box::new(probe)));
    *h.input_mut("in_l") = [0.75; BATCH];
    *h.input_mut("in_r") = [0.25; BATCH];
    for _ in 0..4 {
        h.process();
        assert!(h.output("out_l").iter().all(|s| *s == 0.75), "dry");
        assert!(h.output("out_r").iter().all(|s| *s == 0.25), "dry");
    }
    assert_eq!(shared.calls(), 4, "called every batch: nothing failed");
    assert!(drain_messages(&r.manager).is_empty());
    let m = meters(&h);
    assert_eq!(m[2], 0.0, "state unchanged: running");
    assert_eq!(m[5], 4.0, "late batches");
    // The next on-time batch is wet again.
    shared.set_status(ProcessStatus::Continue);
    h.process();
    assert!(h.output("out_l").iter().all(|s| *s == 0.5), "wet again");
    assert_eq!(meters(&h)[5], 4.0);
}

#[test]
fn a_late_batch_of_an_instrument_is_silent() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Constant(0.5));
    shared.set_status(ProcessStatus::Late);
    let (mut h, r) = harness(rig("sb.late-synth", &synth_layout(), Box::new(probe)));
    for _ in 0..3 {
        h.process();
        assert!(h.output("out_l").iter().all(|s| *s == 0.0));
        assert!(h.output("out_r").iter().all(|s| *s == 0.0));
    }
    assert!(drain_messages(&r.manager).is_empty());
    assert_eq!(meters(&h)[5], 3.0);
}

// ----- Lost ---------------------------------------------------------------------------------

#[test]
fn a_lost_process_crashes_an_effect_to_dry_and_posts_one_message() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Constant(0.5));
    shared.set_status(ProcessStatus::Lost);
    let (mut h, r) = harness(rig("sb.lost-fx", &fx_layout(), Box::new(probe)));
    *h.input_mut("in_l") = [0.75; BATCH];
    for _ in 0..4 {
        h.process();
        assert!(h.output("out_l").iter().all(|s| *s == 0.75), "dry");
    }
    assert_eq!(shared.calls(), 1, "the processor is never called again");
    let msgs = drain_messages(&r.manager);
    assert_eq!(
        msgs,
        vec![PluginMessage::Failed {
            reason: atomik_plugin_host::LOST
        }]
    );
    let m = meters(&h);
    assert_eq!(m[2], 5.0, "crashed");
    assert_eq!(m[5], 0.0, "lost is not late");
    assert_eq!(
        r.manager.slot().device_state(),
        atomik_plugin_host::DeviceState::Crashed
    );
}

#[test]
fn a_lost_instrument_is_silent_from_then_on() {
    let (probe, shared) = Probe::new();
    shared.set_out(ProbeOut::Constant(0.5));
    shared.set_status(ProcessStatus::Lost);
    let (mut h, _r) = harness(rig("sb.lost-synth", &synth_layout(), Box::new(probe)));
    for _ in 0..3 {
        h.process();
        assert!(h.output("out_l").iter().all(|s| *s == 0.0));
    }
    assert_eq!(shared.calls(), 1);
}
