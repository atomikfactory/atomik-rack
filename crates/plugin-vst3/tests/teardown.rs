//! How a VST3 plugin is brought up and taken down, against strict test plugins: one that lets go
//! of its peer only for the object it was connected with, one that talks while it is torn down,
//! a component that is not an audio processor, a controller that refuses to initialise, a plugin
//! that is its own controller, and the host context a factory is given. The instance counters of
//! the test library are process-wide, so each test reads them as a change from its own start.

mod support;

use atomik_device_sdk::RunMode;
use atomik_plugin_host::testing::vst3_test_plugin;
use atomik_plugin_host::{BackendError, PluginBackend, PluginInstance};
use atomik_plugin_vst3::Vst3Backend;
use std::path::PathBuf;
use support::*;

const VARIANTS: &str = "atomik-test-vst3-variants";

/// The counters of a test library, read as the change since `Delta::start`.
struct Delta<'a> {
    lib: &'a TestLib,
    base: Vec<u64>,
}

impl<'a> Delta<'a> {
    fn start(lib: &'a TestLib) -> Self {
        Self {
            lib,
            base: (0..24).map(|i| lib.counter(i)).collect(),
        }
    }

    fn of(&self, index: u32) -> u64 {
        self.lib.counter(index) - self.base[index as usize]
    }
}

/// A backend that knows the variants library, and the library loaded a second time to read its
/// counters.
fn variants() -> (Vst3Backend, TestLib, PathBuf) {
    atomik_plugin_host::mark_main_thread();
    let path = Format::Vst3.install(VARIANTS);
    let lib = TestLib::open_path(&path);
    let backend = Vst3Backend::new();
    backend.scan_file(&path).expect("the library scans");
    (backend, lib, path)
}

fn plugin(n: u32) -> atomik_device_sdk::PluginRef {
    vst3_ref(&variant_id(n))
}

fn instantiate(
    backend: &Vst3Backend,
    n: u32,
) -> Result<(Box<dyn PluginInstance>, atomik_plugin_host::ManagerEnd), BackendError> {
    let id = atomik_plugin_host::InstanceId(900 + u64::from(n));
    let (_device_end, manager) = atomik_plugin_host::plugin_channel(id);
    let instance = backend.instantiate(
        &plugin(n),
        atomik_plugin_host::HostHandle::new(manager.messenger()),
    )?;
    Ok((instance, manager))
}

/// Runs `body` on a thread of its own, marked as the main thread, and fails when it does not
/// finish within a minute (a teardown that deadlocks).
fn within_a_minute(body: impl FnOnce() + Send + 'static) {
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        atomik_plugin_host::mark_main_thread();
        body();
        let _ = done.send(());
    });
    wait.recv_timeout(std::time::Duration::from_secs(60))
        .expect("the body finished (a panic inside it, or a deadlock, shows here)");
}

#[test]
fn a_plugin_is_taken_down_deactivate_disconnect_each_half_with_its_proxy_terminate_controller_then_component()
 {
    let _s = serial();
    let (backend, lib, _) = variants();
    let d = Delta::start(&lib);
    let (mut instance, _manager) = instantiate(&backend, V_STRICT).expect("instantiates");
    let processor = instance
        .activate(&activate_config(RunMode::Offline))
        .expect("activates");
    assert_eq!(
        d.of(counter::DISCONNECTS_OWN) + d.of(counter::DISCONNECTS_OTHER),
        0
    );
    // Dropped while active: the instance deactivates it first.
    drop(instance);
    drop(processor);
    assert_eq!(
        d.of(counter::DISCONNECTS_OWN),
        2,
        "each half was disconnected with the proxy it was connected with"
    );
    assert_eq!(
        d.of(counter::DISCONNECTS_OTHER),
        0,
        "never with null or the other half's proxy"
    );
    let at = |i| lib.counter(i);
    let order = [
        at(counter::SEQ_DEACTIVATE),
        at(counter::SEQ_DISCONNECT_COMPONENT).min(at(counter::SEQ_DISCONNECT_CONTROLLER)),
    ];
    assert!(
        order[0] < order[1],
        "deactivated before the halves were disconnected: {order:?}"
    );
    let last_disconnect =
        at(counter::SEQ_DISCONNECT_COMPONENT).max(at(counter::SEQ_DISCONNECT_CONTROLLER));
    assert!(
        last_disconnect < at(counter::SEQ_TERMINATE_CONTROLLER),
        "both disconnected before the controller is terminated"
    );
    assert!(
        at(counter::SEQ_TERMINATE_CONTROLLER) < at(counter::SEQ_TERMINATE_COMPONENT),
        "the controller is terminated before the component"
    );
    assert_eq!(d.of(counter::COMPONENT_INITS), 1);
    assert_eq!(d.of(counter::COMPONENT_TERMS), 1);
    assert_eq!(d.of(counter::CONTROLLER_INITS), 1);
    assert_eq!(d.of(counter::CONTROLLER_TERMS), 1);
    assert_eq!(d.of(counter::CREATED), 1);
    assert_eq!(d.of(counter::DESTROYED), 1, "the component is destroyed");
    assert_eq!(d.of(counter::CONTROLLERS_CREATED), 1);
    assert_eq!(
        d.of(counter::CONTROLLERS_DESTROYED),
        1,
        "so is the controller"
    );
}

#[test]
fn a_plugin_that_talks_to_its_peer_while_it_is_torn_down_neither_hangs_nor_crashes() {
    let _s = serial();
    within_a_minute(|| {
        let (backend, lib, _) = variants();
        let d = Delta::start(&lib);
        for _ in 0..20 {
            let (mut instance, _manager) = instantiate(&backend, V_STRICT_CHATTY).expect("creates");
            let processor = instance
                .activate(&activate_config(RunMode::Offline))
                .expect("activates");
            drop(instance);
            drop(processor);
        }
        assert_eq!(
            d.of(counter::TEARDOWN_MESSAGES_DELIVERED),
            40,
            "each half's last word from inside `disconnect` reached its peer; the ones from \
             `terminate` met a cut proxy and were not delivered"
        );
        assert_eq!(d.of(counter::CREATED), 20);
        assert_eq!(d.of(counter::DESTROYED), 20);
        assert_eq!(d.of(counter::CONTROLLERS_DESTROYED), 20);
    });
}

#[test]
fn a_component_that_is_not_an_audio_processor_fails_the_plugin_and_is_terminated_once() {
    let _s = serial();
    let (backend, lib, _) = variants();
    let d = Delta::start(&lib);
    let err = instantiate(&backend, V_NOT_PROC).err().expect("fails");
    assert!(
        matches!(&err, BackendError::Unsupported(why) if why.contains("not an audio processor")),
        "{err:?}"
    );
    assert_eq!(d.of(counter::COMPONENT_INITS), 1);
    assert_eq!(d.of(counter::COMPONENT_TERMS), 1, "terminated exactly once");
    assert_eq!(d.of(counter::CREATED), 1);
    assert_eq!(d.of(counter::DESTROYED), 1, "and destroyed");
}

#[test]
fn a_controller_that_refuses_to_initialise_fails_the_plugin_without_being_terminated() {
    let _s = serial();
    let (backend, lib, _) = variants();
    let d = Delta::start(&lib);
    let err = instantiate(&backend, V_CTRL_REFUSES).err().expect("fails");
    assert!(
        matches!(
            &err,
            BackendError::Unsupported(why) if why == "the plugin's controller refused to initialise"
        ),
        "{err:?}"
    );
    assert_eq!(d.of(counter::CONTROLLER_INITS), 1);
    assert_eq!(
        d.of(counter::CONTROLLER_TERMS),
        0,
        "never terminated: it never initialised"
    );
    assert_eq!(d.of(counter::CONTROLLERS_CREATED), 1);
    assert_eq!(d.of(counter::CONTROLLERS_DESTROYED), 1, "but destroyed");
    assert_eq!(d.of(counter::COMPONENT_INITS), 1);
    assert_eq!(
        d.of(counter::COMPONENT_TERMS),
        1,
        "the component is terminated once"
    );
    assert_eq!(d.of(counter::CREATED), 1);
    assert_eq!(
        d.of(counter::DESTROYED),
        1,
        "and destroyed: nothing is left alive"
    );
    assert_eq!(
        d.of(counter::DISCONNECTS_OWN) + d.of(counter::DISCONNECTS_OTHER),
        0
    );
}

#[test]
fn a_plugin_that_is_its_own_controller_is_one_object_with_one_initialize_and_one_terminate() {
    let _s = serial();
    let (backend, lib, _) = variants();
    let d = Delta::start(&lib);
    let (mut instance, _manager) = instantiate(&backend, V_CTRL_IS_COMPONENT).expect("loads");
    assert_eq!(d.of(counter::COMPONENT_INITS), 1, "one initialize");
    assert_eq!(d.of(counter::CONTROLLER_INITS), 0, "no second one");
    assert_eq!(
        d.of(counter::CONTROLLERS_CREATED),
        0,
        "no controller object of its own"
    );
    assert_eq!(
        lib.probe(&*instance, probe::CONNECTS),
        0,
        "no connection, no proxies"
    );
    // Parameters and state work as for a component that casts to its controller.
    let params: Vec<u32> = instance.layout().params.iter().map(|p| p.id).collect();
    assert_eq!(params, [1, 3]);
    assert_eq!(instance.read_params().len(), 2);
    let blob = instance.capture_state().expect("saves");
    instance.restore_state(&blob).expect("loads");
    assert_eq!(lib.probe(&*instance, probe::STATE_SAVES), 2);
    assert_eq!(lib.probe(&*instance, probe::CTRL_STATE_SAVES), 1);
    assert_eq!(lib.probe(&*instance, probe::STATE_LOADS), 1);
    assert_eq!(lib.probe(&*instance, probe::CTRL_COMPONENT_STATES), 2);
    drop(instance);
    assert_eq!(d.of(counter::COMPONENT_TERMS), 1, "one terminate");
    assert_eq!(d.of(counter::CONTROLLER_TERMS), 0);
    assert_eq!(d.of(counter::CREATED), 1);
    assert_eq!(d.of(counter::DESTROYED), 1);
}

#[test]
fn the_factory_gets_the_host_context_once_per_module_load_before_anything_is_created() {
    let _s = serial();
    atomik_plugin_host::mark_main_thread();
    let path = vst3_test_plugin("atomik-test-vst3").expect("builds");
    let lib = TestLib::open_path(&path);
    let d = Delta::start(&lib);
    let backend = Vst3Backend::new();
    let found = backend.scan_file(&path).expect("scans");
    assert_eq!(
        d.of(counter::HOST_CONTEXT_CALLS),
        1,
        "once, with the module"
    );
    // Creating the instances (the scan did) and instantiating again reuses the loaded module.
    let k = [Plug::Gain, Plug::Synth];
    for p in k {
        let id = vst3_ref(&vst3_id(p));
        let (_device_end, manager) =
            atomik_plugin_host::plugin_channel(atomik_plugin_host::InstanceId(1));
        let _ = backend
            .instantiate(
                &id,
                atomik_plugin_host::HostHandle::new(manager.messenger()),
            )
            .expect("instantiates");
    }
    assert_eq!(found.len(), 4);
    assert_eq!(
        d.of(counter::HOST_CONTEXT_CALLS),
        1,
        "not again per instance"
    );
    assert_eq!(
        d.of(counter::CREATES_BEFORE_CONTEXT),
        0,
        "nothing was created before the factory had the context"
    );
    drop(backend);
    assert_eq!(
        lib.counter(counter::CONTEXT_REFS_AT_FACTORY_DROP),
        1,
        "the context outlived the factory: only the module's own reference was left"
    );
}

#[test]
fn a_factory_that_refuses_the_host_context_is_loaded_and_used_all_the_same() {
    let _s = serial();
    let (backend, lib, _) = variants();
    let d = Delta::start(&lib);
    // The library of the variants refuses the context; `variants()` loaded it already, so a
    // second backend loads the module again and the factory is asked again.
    let path = Format::Vst3.install(VARIANTS);
    let again = Vst3Backend::new();
    let found = again.scan_file(&path).expect("scans");
    assert_eq!(
        d.of(counter::HOST_CONTEXT_CALLS),
        1,
        "asked once, and said no"
    );
    assert_eq!(found.len(), 24);
    let (instance, _manager) = instantiate(&backend, V_SPLIT).expect("instantiates");
    assert!(!instance.layout().params.is_empty());
}
