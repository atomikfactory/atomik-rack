//! Lost: every call fails at once with the contract's text and does no I/O; a dropped instance
//! ends its child, killing it when it does not leave.

use super::*;
use crate::clock::now;
use crate::fake::Action;
use atomik_device_sdk::PluginStateBlob;
use atomik_plugin_host::{NativeParent, PluginInstance};
use atomik_plugin_ipc::ToChild;
use std::sync::atomic::Ordering;

#[test]
fn after_the_child_ends_every_call_fails_at_once_and_sends_nothing() {
    let mut r = rig(FakeOptions::default());
    r.fake
        .ctl
        .exit("the plugin process exited with code 9 (0x9)");
    eventually("the instance to be lost", || r.instance.is_lost());
    let before = r.fake.ctl.requests().len();
    let t = now();
    assert_eq!(
        r.instance
            .activate(&cfg(48_000.0, RunMode::Realtime))
            .map(|_| ())
            .unwrap_err(),
        lost_error()
    );
    assert_eq!(r.instance.capture_state().unwrap_err(), lost_error());
    assert_eq!(
        r.instance
            .restore_state(&PluginStateBlob(vec![1]))
            .unwrap_err(),
        lost_error()
    );
    assert_eq!(
        r.instance
            .open_external_gui(Some(NativeParent::Win32(1)))
            .unwrap_err(),
        lost_error()
    );
    assert_eq!(r.instance.param_text(1, 0.5), None);
    assert!(r.instance.default_pins().is_empty());
    assert!(!r.instance.take_layout_changed());
    r.instance.close_gui();
    r.instance.poll();
    assert!(
        now().saturating_duration_since(t) < Duration::from_millis(1000),
        "no waiting"
    );
    assert_eq!(r.fake.ctl.requests().len(), before, "no I/O");
}

#[test]
fn a_lost_instance_still_takes_its_processor_back_without_talking_to_the_child() {
    let mut r = rig(FakeOptions::default());
    let p = r
        .instance
        .activate(&cfg(48_000.0, RunMode::Realtime))
        .unwrap();
    r.fake
        .ctl
        .exit("the plugin process exited with code 9 (0x9)");
    eventually("the instance to be lost", || r.instance.is_lost());
    let before = r.fake.ctl.requests().len();
    r.instance.deactivate(p);
    assert_eq!(r.fake.ctl.requests().len(), before);
}

#[test]
fn dropping_the_instance_sends_shutdown_and_the_child_leaves_within_the_second() {
    let r = rig(FakeOptions::default());
    let ctl = Arc::clone(&r.fake.ctl);
    let live = Arc::clone(&r.live);
    let t = now();
    drop(r.instance);
    assert!(now().saturating_duration_since(t) < Duration::from_secs(1));
    assert!(matches!(ctl.requests().last(), Some(ToChild::Shutdown)));
    assert!(!ctl.was_killed());
    assert_eq!(live.load(Ordering::SeqCst), 0);
}

#[test]
fn a_child_that_ignores_shutdown_is_killed_after_one_second() {
    let r = rig(FakeOptions::default()
        .script(|req, _| matches!(req, ToChild::Shutdown).then_some(Action::Silent)));
    let ctl = Arc::clone(&r.fake.ctl);
    let live = Arc::clone(&r.live);
    let t = now();
    drop(r.instance);
    let took = now().saturating_duration_since(t);
    assert!(took >= Duration::from_millis(900), "{took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert!(ctl.was_killed());
    assert_eq!(live.load(Ordering::SeqCst), 0);
}

#[test]
fn a_child_that_ends_before_start_processing_still_yields_a_crashed_event() {
    use atomik_device_sdk::{PluginFormat, PluginRef};
    use atomik_plugin_host::{
        HostHandle, InstantiateRequest, ManagerEvent, PluginBackend, PluginManager,
        mark_main_thread,
    };
    use std::sync::Mutex;

    type Parts = (crate::transport::Transport, atomik_plugin_ipc::ParentPort);
    struct OneShot(Mutex<Option<Parts>>);
    impl PluginBackend for OneShot {
        fn format(&self) -> PluginFormat {
            PluginFormat::Clap
        }
        fn scan_file(
            &self,
            _: &std::path::Path,
        ) -> Result<Vec<atomik_plugin_host::ScannedPlugin>, BackendError> {
            Err(BackendError::Unsupported(String::new()))
        }
        fn instantiate(
            &self,
            _: &PluginRef,
            host: HostHandle,
        ) -> Result<Box<dyn PluginInstance>, BackendError> {
            let (transport, port) = self.0.lock().unwrap().take().expect("once");
            let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let instance = crate::instance::RemoteInstance::start(
                transport,
                port,
                crate::fake::test_timeouts(),
                host.messenger().clone(),
                crate::link::LiveGuard::new(&live),
            )?;
            Ok(Box::new(instance))
        }
    }

    mark_main_thread();
    let (transport, port, fake) =
        crate::fake::fake_transport(FakeOptions::default(), crate::fake::make_ports());
    let mut m = PluginManager::new();
    m.add_backend(Box::new(OneShot(Mutex::new(Some((transport, port))))));
    let handle = m
        .instantiate(&InstantiateRequest {
            plugin: PluginRef {
                format: PluginFormat::Clap,
                id: "fake".to_string(),
            },
            state: None,
            sample_rate: 48_000.0,
            mode: RunMode::Realtime,
            pinned: false,
            powered: true,
        })
        .map_err(|e| e.to_string())
        .unwrap();
    let id = handle.id;
    let mut h = atomik_testkit::harness::DeviceHarness::new(Box::new(handle.device), 48_000.0);
    fake.ctl.exit("the plugin process exited with code 9 (0x9)");
    eventually("the instance to be lost", || m.is_lost(id));
    h.process();
    let mut crashed = None;
    eventually("a Crashed event", || {
        for e in m.poll() {
            if let ManagerEvent::Crashed { instance, reason } = e {
                crashed = Some((instance, reason));
            }
        }
        crashed.is_some()
    });
    let (instance, reason) = crashed.unwrap();
    assert_eq!(instance, id);
    assert!(reason.contains("code 9"), "{reason}");
}
