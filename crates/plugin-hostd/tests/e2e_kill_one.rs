//! One plugin process killed from outside while two sandboxed plugins play: the manager reports
//! `Crashed` for that instance only, the other one keeps processing, the state the app captured
//! before the kill is still there to restore into a new instance, and no child is left behind.
//! The child is found through the manager (`native_handle` is the process id of a sandboxed
//! instance) and ended by the operating system's own kill command.
// Justification: copied samples are compared bit for bit.
#![allow(clippy::float_cmp)]
// Justification: the test waits for child processes against the wall clock; nothing here is in a
// signal path.
#![allow(clippy::disallowed_methods)]

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::RunMode;
use atomik_plugin_host::{DeviceState, ManagerEvent};
use sandbox_support::{
    CRASH_DEADLINE, METER_STATE, SETTLE, Sandbox, config, effect, meters, null_plugin, out_l,
    process_settled, request, request_with_state, run_to_crash,
};
use std::process::Command;
use std::time::{Duration, Instant};

/// Ends the process `pid` the hard way (Task Manager's "end task", `kill -9`).
fn kill_process(pid: usize) {
    let status = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
            .map(|o| o.status)
    } else {
        Command::new("kill").args(["-9", &pid.to_string()]).status()
    };
    assert!(
        status.expect("the kill command runs").success(),
        "process {pid} was not killed"
    );
}

#[test]
fn killing_one_plugin_process_crashes_only_that_instance_and_its_saved_state_survives() {
    let mut sb = Sandbox::null(config());
    let plugin = null_plugin("null.gain");

    let first = sb
        .manager
        .instantiate(&request(&plugin, RunMode::Realtime))
        .unwrap();
    let second = sb
        .manager
        .instantiate(&request(&plugin, RunMode::Realtime))
        .unwrap();
    let (victim, bystander) = (first.id, second.id);
    let mut hv = effect(first, 0.5);
    let mut hb = effect(second, 0.5);
    assert_eq!(sb.backend.live_children(), 2, "one child for each");

    // The victim passes its input (the param is the "pass" switch), the bystander halves it.
    hv.set_param("p0", 1.0);
    process_settled(&mut hv, 10, SETTLE);
    process_settled(&mut hb, 10, SETTLE);
    assert!(out_l(&hv).iter().all(|s| *s == 0.5));
    assert!(out_l(&hb).iter().all(|s| *s == 0.25));
    // What the app last captured from the victim: the saved state of its device.
    let saved = sb.manager.capture_state(victim).unwrap();
    assert_eq!(saved.0, 1.0f64.to_bits().to_le_bytes().to_vec());

    let pid = sb.manager.native_handle(victim).expect("a known instance");
    assert_ne!(pid, 0, "a sandboxed instance is a process");
    assert_ne!(
        Some(pid),
        sb.manager.native_handle(bystander),
        "two processes"
    );
    kill_process(pid);

    // The device finds out on its audio path: dry from then on.
    run_to_crash(&mut hv, 0.5, CRASH_DEADLINE);
    let mut events = Vec::new();
    let end = Instant::now() + CRASH_DEADLINE;
    while !events
        .iter()
        .any(|e| matches!(e, ManagerEvent::Crashed { .. }))
    {
        events.extend(sb.manager.poll());
        assert!(Instant::now() < end, "no Crashed event: {events:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
    let crashed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ManagerEvent::Crashed { instance, .. } => Some(*instance),
            _ => None,
        })
        .collect();
    assert_eq!(
        crashed,
        vec![victim],
        "only the killed instance: {events:?}"
    );
    assert!(sb.manager.is_lost(victim));
    assert!(!sb.manager.is_lost(bystander));
    assert_eq!(
        meters(&hv)[METER_STATE],
        DeviceState::Crashed.as_f32(),
        "the victim's device is CRASHED"
    );

    // The other instance never noticed: still running, still processing (halved, not dry).
    process_settled(&mut hb, 10, SETTLE);
    assert_eq!(meters(&hb)[METER_STATE], DeviceState::Running.as_f32());
    assert!(out_l(&hb).iter().all(|s| *s == 0.25));
    assert!(sb.manager.capture_state(bystander).is_ok());

    // Nothing more can be asked of the dead process, and the answer is an error, not a hang.
    assert!(sb.manager.capture_state(victim).is_err());
    // What was saved is intact: a new instance restores it and saves it back unchanged.
    drop(hv);
    sb.manager.poll();
    let reloaded = sb
        .manager
        .instantiate(&request_with_state(
            &plugin,
            RunMode::Realtime,
            saved.clone(),
        ))
        .unwrap();
    assert!(reloaded.restore_error.is_none());
    let again = reloaded.id;
    let mut hr = effect(reloaded, 0.5);
    process_settled(&mut hr, 10, SETTLE);
    assert!(out_l(&hr).iter().all(|s| *s == 0.5), "the saved pass state");
    assert_eq!(sb.manager.capture_state(again).unwrap(), saved);

    // The dead child is gone and the live ones are the two that work.
    assert!(
        sb.wait_children(2, Duration::from_secs(30)),
        "{} children",
        sb.backend.live_children()
    );
    drop((hr, hb));
    sb.manager.poll();
    sb.finish();
}
