//! (backend half) The sandbox backend's registry and its refusals, through the public API.

use atomik_device_sdk::{PluginFormat, PluginRef};
use atomik_plugin_host::{
    BackendError, HostHandle, InstanceId, PluginBackend, mark_main_thread, plugin_channel,
};
use atomik_plugin_sandbox::{HostCommand, SandboxBackend, SandboxConfig};
use std::path::Path;

fn clap(id: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: id.to_string(),
    }
}

fn host() -> HostHandle {
    let (_device, manager) = plugin_channel(InstanceId(1));
    HostHandle::new(manager.messenger())
}

fn backend(program: &str) -> SandboxBackend {
    SandboxBackend::new(
        PluginFormat::Clap,
        SandboxConfig::new(HostCommand::new(program)),
    )
}

#[test]
fn the_backend_reports_its_format_and_its_configuration() {
    let b = backend("no-such-program");
    assert_eq!(b.format(), PluginFormat::Clap);
    assert_eq!(b.config().command.program, Path::new("no-such-program"));
    assert_eq!(b.live_children(), 0);
}

#[test]
fn instantiating_a_plugin_nobody_registered_is_unknown() {
    mark_main_thread();
    let b = backend("no-such-program");
    let err = b.instantiate(&clap("x.y"), host()).map(|_| ()).unwrap_err();
    assert_eq!(err, BackendError::UnknownPlugin("x.y".to_string()));
}

#[test]
fn scanning_is_unsupported_because_the_scanner_does_it() {
    let b = backend("no-such-program");
    let err = b.scan_file(Path::new("x.clap")).map(|_| ()).unwrap_err();
    assert!(matches!(err, BackendError::Unsupported(_)), "{err:?}");
}

#[test]
fn a_registered_plugin_whose_program_cannot_start_is_a_clear_failure() {
    mark_main_thread();
    let b = backend("definitely-no-such-atomik-program");
    b.register_plugin_file(Path::new("a.clap"), &[clap("a.one"), clap("a.two")]);
    let err = b
        .instantiate(&clap("a.two"), host())
        .map(|_| ())
        .unwrap_err();
    match err {
        BackendError::Failed(text) => assert!(text.contains("cannot start"), "{text}"),
        other => panic!("{other:?}"),
    }
    // An id of another file stays unknown.
    let err = b
        .instantiate(&clap("b.one"), host())
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(err, BackendError::UnknownPlugin(_)));
    assert_eq!(b.live_children(), 0);
}

#[test]
fn registering_a_file_again_replaces_its_plugins_path() {
    mark_main_thread();
    let b = backend("definitely-no-such-atomik-program");
    b.register_plugin_file(Path::new("a.clap"), &[clap("a.one")]);
    b.register_plugin_file(Path::new("moved.clap"), &[clap("a.one")]);
    let err = b
        .instantiate(&clap("a.one"), host())
        .map(|_| ())
        .unwrap_err();
    match err {
        BackendError::Failed(text) => assert!(text.contains("moved.clap"), "{text}"),
        other => panic!("{other:?}"),
    }
}
