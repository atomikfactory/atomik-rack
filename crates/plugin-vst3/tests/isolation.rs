//! A module that fails, hangs or crashes cannot take the host down where it can be avoided:
//! a module whose entry refuses is a failed scan, one that never returns blocks
//! only the thread that loaded it (which is why the app scans out of process), and a plugin that
//! crashes takes down only the process it runs in (a child process of this test binary).
// Justification: a copied sample is compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_plugin_host::testing::vst3_test_plugin;
use atomik_plugin_host::{BackendError, PluginBackend, mark_main_thread};
use atomik_plugin_vst3::Vst3Backend;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;
use support::*;

#[test]
fn a_module_whose_entry_refuses_is_a_failed_scan_and_the_next_scan_works() {
    mark_main_thread();
    let bad = vst3_test_plugin("atomik-test-vst3-bad").unwrap();
    let backend = Vst3Backend::new();
    let err = backend.scan_file(&bad).unwrap_err();
    assert!(matches!(err, BackendError::Failed(_)), "{err:?}");
    assert!(err.to_string().contains("refused"), "{err}");
    // The host carries on: a good file scans afterwards.
    let good = vst3_test_plugin("atomik-test-vst3").unwrap();
    assert_eq!(backend.scan_file(&good).unwrap().len(), 4);
}

#[test]
fn a_module_whose_entry_never_returns_blocks_only_the_thread_that_loaded_it() {
    let path = vst3_test_plugin("atomik-test-vst3-hang").unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        mark_main_thread();
        let _ = tx.send(Vst3Backend::new().scan_file(&path).is_ok());
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(800)).is_err(),
        "the hang plugin must not return"
    );
    // This thread is fine.
    mark_main_thread();
    let good = vst3_test_plugin("atomik-test-vst3").unwrap();
    assert!(Vst3Backend::new().scan_file(&good).is_ok());
}

#[cfg(windows)]
fn no_crash_dialog() {
    // SAFETY: the declaration matches `kernel32!SetErrorMode`; the call only changes the error
    // mode of this process so a deliberate crash does not open a Windows error dialog.
    unsafe extern "system" {
        fn SetErrorMode(mode: u32) -> u32;
    }
    // SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX
    // SAFETY: see above.
    unsafe { SetErrorMode(0x0001 | 0x0002 | 0x8000) };
}

#[cfg(not(windows))]
fn no_crash_dialog() {}

fn crash_ref() -> atomik_device_sdk::PluginRef {
    vst3_ref(&format!(
        "{:08x}{:08x}{:08x}{:08x}",
        0xA70F_1C06u32, 0x4A5B_4C01u32, 0x8D2E_1F00u32, 1u32
    ))
}

/// The child: with `ATOMIK_CRASH_CHILD` set it processes a batch of the crash plugin armed to
/// crash; otherwise it does nothing and passes.
#[test]
fn child_process_that_crashes() {
    if std::env::var_os("ATOMIK_CRASH_CHILD").is_none() {
        return;
    }
    no_crash_dialog();
    mark_main_thread();
    let path = vst3_test_plugin("atomik-test-vst3-crash").unwrap();
    let backend = Vst3Backend::new();
    backend.scan_file(&path).unwrap();
    let mut rig = rig_for(&backend, &crash_ref(), atomik_device_sdk::RunMode::Realtime);
    let mut h = rig.harness();
    *h.input_mut("in_l") = [0.5; 64];
    h.process(); // the plugin writes through a null pointer here
    unreachable!("the crash plugin must have taken the process down");
}

#[test]
fn the_crash_plugin_scans_and_runs_when_not_armed() {
    mark_main_thread();
    let path = vst3_test_plugin("atomik-test-vst3-crash").unwrap();
    let backend = Vst3Backend::new();
    assert_eq!(
        backend
            .scan_file(&path)
            .expect("scanning never crashes")
            .len(),
        1
    );
    let mut rig = rig_for(&backend, &crash_ref(), atomik_device_sdk::RunMode::Realtime);
    let mut h = rig.harness();
    *h.input_mut("in_l") = [0.5; 64];
    for _ in 0..4 {
        h.process();
    }
    assert!(h.output("out_l").iter().all(|s| *s == 0.5));
}

#[test]
fn an_armed_crash_plugin_takes_down_only_its_own_process() {
    let exe = std::env::current_exe().unwrap();
    let status = Command::new(exe)
        .args(["--exact", "child_process_that_crashes", "--nocapture"])
        .env("ATOMIK_CRASH_CHILD", "1")
        .env("ATOMIK_TEST_CRASH", "1")
        .status()
        .expect("the child starts");
    assert!(!status.success(), "the child crashed: {status}");
    // This process is untouched and keeps working.
    mark_main_thread();
    let good = vst3_test_plugin("atomik-test-vst3").unwrap();
    assert!(Vst3Backend::new().scan_file(&good).is_ok());
}
