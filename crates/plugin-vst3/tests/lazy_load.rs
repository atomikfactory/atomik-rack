//! (backend half): a plugin file registered from the scan cache is not loaded until an
//! instance is created, and then loaded once; the module exit runs when the backend goes. Alone
//! in its test binary because the entry counter is process-wide.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_plugin_host::testing::vst3_test_plugin;
use atomik_plugin_host::{HostHandle, InstanceId, PluginBackend, mark_main_thread, plugin_channel};
use atomik_plugin_vst3::Vst3Backend;
use support::*;

#[test]
fn registering_a_plugin_file_loads_nothing_and_the_first_instance_loads_it_once() {
    mark_main_thread();
    let path = vst3_test_plugin("atomik-test-vst3").unwrap();
    let lib = TestLib::open_path(&path);
    let backend = Vst3Backend::new();
    backend.register_plugin_file(&vst3_id(Plug::Gain), &path);
    backend.register_plugin_file(&vst3_id(Plug::Synth), &path);
    assert_eq!(
        lib.counter(counter::ENTRY_INIT),
        0,
        "registering from the cache runs no plugin code"
    );
    let (_d, manager) = plugin_channel(InstanceId(1));
    let a = backend
        .instantiate(
            &vst3_ref(&vst3_id(Plug::Gain)),
            HostHandle::new(manager.messenger()),
        )
        .expect("loads on demand");
    assert_eq!(lib.counter(counter::ENTRY_INIT), 1);
    let b = backend
        .instantiate(
            &vst3_ref(&vst3_id(Plug::Synth)),
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
    assert_eq!(lib.counter(counter::ENTRY_DEINIT), 0);
    // The module stays loaded while an instance lives; the exit runs when the last user goes.
    drop(a);
    drop(b);
    assert_eq!(
        lib.counter(counter::ENTRY_DEINIT),
        0,
        "the backend still holds the module"
    );
    drop(backend);
    assert_eq!(
        lib.counter(counter::ENTRY_DEINIT),
        1,
        "the module exit ran once"
    );
}
