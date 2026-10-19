//! (backend half): a plugin file registered from the scan cache is not loaded until an
//! instance is created, and then loaded once. Alone in its test binary because the entry
//! counter is process-wide.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::testing::clap_test_plugin;
use atomik_plugin_host::{HostHandle, InstanceId, PluginBackend, mark_main_thread, plugin_channel};
use support::*;

#[test]
fn registering_a_plugin_file_loads_nothing_and_the_first_instance_loads_it_once() {
    mark_main_thread();
    let path = clap_test_plugin("atomik-test-clap").unwrap();
    let lib = TestLib::open("atomik-test-clap");
    let backend = ClapBackend::new();
    backend.register_plugin_file("atomik.test.gain", &path);
    backend.register_plugin_file("atomik.test.synth", &path);
    assert_eq!(
        lib.counter(counter::ENTRY_INIT),
        0,
        "registering from the cache runs no plugin code"
    );
    let (_d, manager) = plugin_channel(InstanceId(1));
    let a = backend
        .instantiate(
            &clap_ref("atomik.test.gain"),
            HostHandle::new(manager.messenger()),
        )
        .expect("loads on demand");
    assert_eq!(lib.counter(counter::ENTRY_INIT), 1);
    let b = backend
        .instantiate(
            &clap_ref("atomik.test.synth"),
            HostHandle::new(manager.messenger()),
        )
        .expect("the same file");
    assert_eq!(
        lib.counter(counter::ENTRY_INIT),
        1,
        "the file is loaded once"
    );
    assert_eq!(a.layout().name, "Atomik Test Gain");
    assert_eq!(b.layout().name, "Atomik Test Synth");
}
