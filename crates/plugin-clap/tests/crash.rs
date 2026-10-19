//! A plugin that crashes the process cannot be survived in-process, but it must not
//! crash while it is scanned, and a test can show that the crash only takes down the process it
//! runs in. The crashing run happens in a child process (this test binary again).
// Justification: a copied sample is compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::testing::clap_test_plugin;
use atomik_plugin_host::{PluginBackend, mark_main_thread};
use std::process::Command;
use support::*;

const PKG: &str = "atomik-test-clap-crash";
const ID: &str = "atomik.test.crash";

#[cfg(windows)]
fn no_crash_dialog() {
    // SAFETY: the declaration matches `kernel32!SetErrorMode`; the call only changes this
    // process's error mode so a deliberate crash does not open a Windows error dialog.
    unsafe extern "system" {
        fn SetErrorMode(mode: u32) -> u32;
    }
    // SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX
    // SAFETY: see above.
    unsafe { SetErrorMode(0x0001 | 0x0002 | 0x8000) };
}

#[cfg(not(windows))]
fn no_crash_dialog() {}

/// The child: with `ATOMIK_CRASH_CHILD` set it processes a batch of the crash plugin armed to
/// crash; otherwise it does nothing and passes.
#[test]
fn child_process_that_crashes() {
    if std::env::var_os("ATOMIK_CRASH_CHILD").is_none() {
        return;
    }
    no_crash_dialog();
    let (backend, _) = backend_for(PKG);
    let mut rig = rig(&backend, ID);
    let mut h = rig.harness();
    h.process(); // the plugin writes through a null pointer here
    unreachable!("the crash plugin must have taken the process down");
}

#[test]
fn the_crash_plugin_scans_and_runs_when_not_armed() {
    mark_main_thread();
    let path = clap_test_plugin(PKG).unwrap();
    let found = ClapBackend::new()
        .scan_file(&path)
        .expect("scanning never crashes");
    assert_eq!(found.len(), 1);
    let (backend, _) = backend_for(PKG);
    let mut rig = rig(&backend, ID);
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
    let (backend, _) = backend_for("atomik-test-clap-leak");
    assert!(
        backend
            .instantiate(
                &clap_ref("atomik.test.leak"),
                atomik_plugin_host::HostHandle::new(
                    atomik_plugin_host::plugin_channel(atomik_plugin_host::InstanceId(1))
                        .1
                        .messenger()
                )
            )
            .is_ok()
    );
}
