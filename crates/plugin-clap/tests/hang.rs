//! A plugin library whose entry `init` never returns blocks its caller for good (clack holds
//! its entry cache lock while the entry initialises). This is why the app never loads an
//! unscanned file in its own process and why the scanner runs out of process with a timeout.
//! It lives alone in this test binary because it poisons the entry cache of the
//! process it runs in.

mod support;

use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::testing::clap_test_plugin;
use atomik_plugin_host::{PluginBackend, mark_main_thread};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn a_library_whose_entry_never_returns_never_returns_to_the_caller() {
    // The scanner process kills such a child at its timeout; here the in-process backend
    // is shown to block, which is why the app never loads an unscanned plugin itself.
    let path = clap_test_plugin("atomik-test-clap-hang").unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        mark_main_thread();
        let _ = tx.send(ClapBackend::new().scan_file(&path).is_ok());
    });
    assert!(
        rx.recv_timeout(Duration::from_millis(800)).is_err(),
        "the hang plugin must not return"
    );
}
