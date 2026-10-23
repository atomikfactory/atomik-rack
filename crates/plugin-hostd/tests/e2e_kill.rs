//! A child killed from outside, as a user does from Task Manager, and RELOAD from the last
//! captured state (AC-5, AC-6, AC-16 without the window). This test binary has one test at a
//! time that spawns children, because the kill takes every child of the test process.

atomik_testkit::install_alloc_guard!();

mod sandbox_support;

use atomik_device_sdk::RunMode;
use atomik_plugin_host::{DeviceState, ManagerEvent};
use sandbox_support::{
    FORMATS, METER_STATE, Plug, Sandbox, bits, effect, meters, out_l, request, request_with_state,
    run_to_crash,
};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

/// Only one test at a time may have children: the kill is by parent process.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Kills every child process of this process.
fn kill_children() {
    let pid = std::process::id();
    let status = if cfg!(windows) {
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Get-CimInstance Win32_Process -Filter 'ParentProcessId={pid}' | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force }}"
                ),
            ])
            .status()
    } else {
        Command::new("pkill")
            .args(["-9", "-P", &pid.to_string()])
            .status()
    };
    status.expect("the kill command runs");
}

#[test]
fn a_child_killed_from_outside_crashes_the_device_and_a_reload_from_the_captured_state_works() {
    let _one = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for format in FORMATS {
        let mut sb = Sandbox::real(format);
        let plugin = format.plugin(Plug::Gain);
        let first = sb
            .manager
            .instantiate(&request(&plugin, RunMode::Realtime))
            .unwrap();
        let id = first.id;
        let mut h = effect(first, 0.25);
        // The app's last captured state: a gain of -6 dB, heard on the output.
        h.set_param("p1", format.gain_value(-6.0));
        for _ in 0..10 {
            h.process();
        }
        let blob = sb.manager.capture_state(id).unwrap();
        let heard = out_l(&h);
        assert!(heard.iter().any(|s| s.abs() > 0.001));

        kill_children();
        let batches = run_to_crash(&mut h, 0.25, 1000);
        println!(
            "{}: crashed {batches} batches after the kill",
            format.name()
        );
        let mut events = Vec::new();
        for _ in 0..200 {
            events.extend(sb.manager.poll());
            if events
                .iter()
                .any(|e| matches!(e, ManagerEvent::Crashed { .. }))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ManagerEvent::Crashed { instance, .. } if *instance == id)),
            "{}: {events:?}",
            format.name()
        );
        drop(h);
        sb.manager.poll();

        // RELOAD: a new instance from the state.
        let second = sb
            .manager
            .instantiate(&request_with_state(&plugin, RunMode::Realtime, blob))
            .unwrap();
        assert!(second.restore_error.is_none());
        let mut h2 = effect(second, 0.25);
        for _ in 0..10 {
            h2.process();
        }
        assert_eq!(meters(&h2)[METER_STATE], DeviceState::Running.as_f32());
        assert_eq!(
            bits(&out_l(&h2)),
            bits(&heard),
            "{}: the new instance has the saved gain",
            format.name()
        );
        drop(h2);
        sb.manager.poll();
        sb.finish();
    }
}
