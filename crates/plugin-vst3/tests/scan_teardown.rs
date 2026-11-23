//! Scanning creates and destroys every instance, even the ones that fail halfway (a plugin that
//! refuses the bus layout, one that refuses to initialise or to set up). Alone in its test binary
//! because the instance counters of the test library are process-wide.

mod support;

use atomik_plugin_host::testing::vst3_test_plugin;
use atomik_plugin_host::{PluginBackend, mark_main_thread};
use atomik_plugin_vst3::Vst3Backend;
use support::*;

#[test]
fn scanning_creates_and_destroys_every_instance_even_the_ones_that_fail_halfway() {
    mark_main_thread();
    let path = vst3_test_plugin("atomik-test-vst3-variants").unwrap();
    let lib = TestLib::open_path(&path);
    let b = Vst3Backend::new();
    let found = b.scan_file(&path).unwrap();
    assert_eq!(found.len(), 24);
    assert!(found.iter().filter(|p| p.unsupported.is_some()).count() >= 5);
    assert_eq!(lib.counter(counter::CREATED), 24, "one instance per class");
    assert_eq!(
        lib.counter(counter::DESTROYED),
        24,
        "the unsupported and the failed ones are torn down, too"
    );
    assert_eq!(
        lib.counter(counter::CONTROLLERS_CREATED),
        lib.counter(counter::CONTROLLERS_DESTROYED),
        "every controller class object is destroyed, too"
    );
    assert_eq!(
        lib.counter(counter::COMPONENT_INITS) - lib.counter(counter::COMPONENT_TERMS),
        1,
        "every component that initialised was terminated: the one that refused to is the one left"
    );
}
