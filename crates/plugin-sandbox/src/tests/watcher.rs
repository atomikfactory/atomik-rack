//! The watcher of the process: a child that ends while something still holds its pipes open is
//! noticed dead at once, once, and a child the app asked to end is never reported.

use super::*;
use crate::fake::{FakeOptions, idle_transport};
use crate::group::{Attach, Hub};
use crate::link::LiveGuard;
use atomik_plugin_host::PluginInstance;
use std::io::Read;
use std::sync::atomic::Ordering;

#[test]
fn a_process_that_ends_with_its_pipes_open_is_noticed_dead_and_reported_once() {
    let r = rig(FakeOptions::default());
    eventually("the process to be watched", || r.fake.ctl.watching() == 1);
    r.fake.ctl.stderr("last words\n");
    r.fake
        .ctl
        .exit_silently("the plugin process exited with code 5 (0x5)");
    let PluginMessage::ProcessExited { reason } = next_message(&r.manager) else {
        panic!("not an exit");
    };
    assert!(reason.contains("code 5"), "{reason}");
    assert!(r.instance.is_lost());
    eventually("the child to be counted gone", || {
        r.live.load(Ordering::SeqCst) == 0
    });
    eventually("the watcher to end", || r.fake.ctl.watching() == 0);
    // The pipe closing late (the helper ends) says nothing more.
    r.fake.ctl.exit("a second description");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(r.manager.try_recv(), None, "exactly one exit message");
}

#[test]
fn the_pipe_and_the_process_ending_together_report_one_death() {
    for _ in 0..20 {
        let r = rig(FakeOptions::default());
        r.fake
            .ctl
            .exit("the plugin process exited with code 7 (0x7)");
        let PluginMessage::ProcessExited { reason } = next_message(&r.manager) else {
            panic!("not an exit");
        };
        assert!(reason.contains("code 7"), "{reason}");
        eventually("the child to be counted gone", || {
            r.live.load(Ordering::SeqCst) == 0
        });
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(r.manager.try_recv(), None, "exactly one exit message");
    }
}

#[test]
fn a_shutdown_the_app_asked_for_is_not_a_crash_and_leaves_no_watcher() {
    let Rig {
        instance,
        manager,
        live,
        fake,
    } = rig(FakeOptions::default());
    eventually("the process to be watched", || fake.ctl.watching() == 1);
    drop(instance);
    assert_eq!(live.load(Ordering::SeqCst), 0);
    assert_eq!(manager.try_recv(), None);
    eventually("the watcher to end", || fake.ctl.watching() == 0);
}

#[test]
fn a_killed_process_that_leaves_its_pipes_open_is_reported_by_its_watcher() {
    let r = rig(FakeOptions::default());
    r.fake.ctl.exit_silently("the fake process was killed");
    eventually("the instance to be lost", || r.instance.is_lost());
    assert!(matches!(
        next_message(&r.manager),
        PluginMessage::ProcessExited { .. }
    ));
}

#[test]
fn a_group_whose_process_ends_with_its_pipes_open_loses_every_member() {
    let live = Arc::new(AtomicUsize::new(0));
    let (transport, fake) = idle_transport();
    let hub = Hub::start(transport, LiveGuard::new(&live));
    let attach = |n: u32| Attach {
        plugin_file: "a.clap".to_string(),
        plugin_id: format!("plugin.{n}"),
        block: format!("block-{n}"),
        doorbell: format!("bell-{n}"),
    };
    let mut members: Vec<_> = (0..3)
        .map(|n| hub.join(attach(n)).expect("a member joins"))
        .collect();
    assert!(hub.alive());
    fake.ctl
        .exit_silently("the plugin process exited with code 9 (0x9)");
    eventually("the group to end", || !hub.alive());
    for m in &mut members {
        let mut buf = [0u8; 8];
        assert_eq!(m.reader.read(&mut buf).expect("end of file"), 0);
        eventually("the member to see the exit", || {
            m.child.try_wait().is_some()
        });
        assert!(m.child.try_wait().unwrap_or_default().contains("code 9"));
        let note = m.child.note().unwrap_or_default();
        assert!(note.contains("group of 3 plugins"), "{note}");
    }
    drop(members);
    drop(hub);
    eventually("the watcher to end", || fake.ctl.watching() == 0);
}
