//! A plugin in its own process that is slow to start, refuses to start or never starts, through
//! the real child and the real device: the start rides on the first batch, so a slow start is a
//! run of late batches and never a failure, a refusal fails the device once with the message an
//! in-process plugin gets, and a start that never returns is a hung process like any batch that
//! never returns. Nothing here is timed: conditions are waited for with generous bounds.
// Justification: copied samples are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::RunMode;
use atomik_plugin_host::{DeviceState, ManagerEvent};
use atomik_plugin_ipc::Deadline;
use atomik_testkit::harness::DeviceHarness;
use sandbox_support::{
    CRASH_DEADLINE, METER_LATE, METER_STATE, SETTLE, Sandbox, config, config_with_env, effect,
    meters, null_plugin, out_l, request, run_to_crash,
};
use std::time::Duration;

const MESSAGE: &str = "the plugin could not start processing";

fn live(sb: &mut Sandbox, id: &str, level: f32) -> DeviceHarness {
    let handle = sb
        .manager
        .instantiate(&request(&null_plugin(id), RunMode::Realtime))
        .expect("the null plugin starts in a child");
    effect(handle, level)
}

/// Processes until the output is the plugin's (half the input), failing at once if the device
/// fails, and giving up after `within`.
fn until_wet(h: &mut DeviceHarness, level: f32, within: Duration) {
    let limit = Deadline::after(within);
    loop {
        h.process();
        let state = meters(h)[METER_STATE];
        assert_eq!(
            state,
            DeviceState::Running.as_f32(),
            "the device must not fail while the plugin is slow to start"
        );
        if out_l(h).iter().all(|s| *s == level * 0.5) {
            return;
        }
        assert!(!limit.passed(), "the plugin never started");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_plugin_that_takes_a_tenth_of_a_second_to_start_is_late_and_then_plays() {
    let mut sb = Sandbox::null(config_with_env("ATOMIK_NULL_START_MS", "100"));
    let mut h = live(&mut sb, "null.gain", 0.25);
    until_wet(&mut h, 0.25, SETTLE);
    assert!(
        meters(&h)[METER_LATE] >= 1.0,
        "the batches that waited for the start were late, not lost to a failure"
    );
    // It goes on playing.
    for _ in 0..20 {
        h.process();
    }
    assert_eq!(meters(&h)[METER_STATE], DeviceState::Running.as_f32());
    drop(h);
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_plugin_that_starts_at_once_plays_without_a_failure() {
    let mut sb = Sandbox::null(config());
    let mut h = live(&mut sb, "null.gain", 0.25);
    until_wet(&mut h, 0.25, SETTLE);
    assert_eq!(meters(&h)[METER_STATE], DeviceState::Running.as_f32());
    drop(h);
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_plugin_that_refuses_to_start_fails_the_device_once_with_the_usual_message() {
    let mut sb = Sandbox::null(config());
    let mut h = live(&mut sb, "null.refuse-start", 0.25);
    let limit = Deadline::after(SETTLE);
    while meters(&h)[METER_STATE] != DeviceState::Failed.as_f32() {
        h.process();
        assert!(!limit.passed(), "the device never failed");
        std::thread::sleep(Duration::from_millis(1));
    }
    for _ in 0..10 {
        h.process();
    }
    let failures: Vec<String> = sb
        .manager
        .poll()
        .into_iter()
        .filter_map(|e| match e {
            ManagerEvent::Failed { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(failures, vec![MESSAGE.to_string()]);
    drop(h);
    sb.manager.poll();
    sb.finish();
}

#[test]
fn a_plugin_whose_start_never_returns_is_a_hung_process_and_is_ended() {
    let mut cfg = config();
    cfg.hung_after = Duration::from_millis(500);
    let mut sb = Sandbox::null(cfg);
    let mut h = live(&mut sb, "null.hang-start", 0.25);
    let batches = run_to_crash(&mut h, 0.25, CRASH_DEADLINE);
    println!("crashed after {batches} batches");
    let mut events = Vec::new();
    let limit = Deadline::after(SETTLE);
    while !events
        .iter()
        .any(|e| matches!(e, ManagerEvent::Crashed { .. }))
    {
        events.extend(sb.manager.poll());
        assert!(!limit.passed(), "{events:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(h);
    sb.manager.poll();
    sb.finish();
}
