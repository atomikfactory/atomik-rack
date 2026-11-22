//! The watcher of the process: a child that ends while something still holds its pipes open is
//! noticed dead at once, once, and a child the app asked to end is never reported.

use super::*;
use crate::fake::{FakeOptions, idle_transport};
use crate::group::{Attach, Hub};
use crate::link::LiveGuard;
use atomik_plugin_host::PluginInstance;
use std::io::Read;
use std::sync::atomic::Ordering;

/// The reason of the next death the plugin reports. A diagnostic message (a slow call on a
/// loaded machine) may come first and is not a report.
fn next_exit(m: &ManagerEnd) -> String {
    loop {
        if let PluginMessage::ProcessExited { reason } = next_message(m) {
            return reason;
        }
    }
}

/// No death is reported (now): anything that is waiting is a diagnostic, not a second report.
fn assert_no_exit(m: &ManagerEnd) {
    while let Some(msg) = m.try_recv() {
        assert!(
            !matches!(msg, PluginMessage::ProcessExited { .. }),
            "a second report: {msg:?}"
        );
    }
}

#[test]
fn a_process_that_ends_with_its_pipes_open_is_noticed_dead_and_reported_once() {
    let r = rig(FakeOptions::default());
    eventually("the process to be watched", || r.fake.ctl.watching() == 1);
    r.fake.ctl.stderr("last words\n");
    r.fake
        .ctl
        .exit_silently("the plugin process exited with code 5 (0x5)");
    let reason = next_exit(&r.manager);
    assert!(reason.contains("code 5"), "{reason}");
    assert!(r.instance.is_lost());
    eventually("the child to be counted gone", || {
        r.live.load(Ordering::SeqCst) == 0
    });
    eventually("the watcher to end", || r.fake.ctl.watching() == 0);
    // The pipe closing late (the helper ends) says nothing more.
    r.fake.ctl.exit("a second description");
    std::thread::sleep(Duration::from_millis(100));
    assert_no_exit(&r.manager);
}

#[test]
fn the_pipe_and_the_process_ending_together_report_one_death() {
    for _ in 0..20 {
        let r = rig(FakeOptions::default());
        r.fake
            .ctl
            .exit("the plugin process exited with code 7 (0x7)");
        let reason = next_exit(&r.manager);
        assert!(reason.contains("code 7"), "{reason}");
        eventually("the child to be counted gone", || {
            r.live.load(Ordering::SeqCst) == 0
        });
        std::thread::sleep(Duration::from_millis(20));
        assert_no_exit(&r.manager);
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
    assert_no_exit(&manager);
    eventually("the watcher to end", || fake.ctl.watching() == 0);
}

#[test]
fn a_killed_process_that_leaves_its_pipes_open_is_reported_by_its_watcher() {
    let r = rig(FakeOptions::default());
    r.fake.ctl.exit_silently("the fake process was killed");
    eventually("the instance to be lost", || r.instance.is_lost());
    let _ = next_exit(&r.manager);
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

/// Runs `f` on a thread and says whether it returned within a generous bound.
fn returns_in_time(f: impl FnOnce() + Send + 'static) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(20)).is_ok()
}

#[test]
fn dropping_a_link_whose_pipes_a_helper_still_holds_does_not_wait_for_the_helper() {
    let Rig {
        instance,
        manager,
        live,
        fake,
    } = rig(FakeOptions::default());
    eventually("the process to be watched", || fake.ctl.watching() == 1);
    // The process ends, the pipes stay open (the fake keeps its ends): the reader is blocked.
    fake.ctl
        .exit_silently("the plugin process exited with code 5 (0x5)");
    let _ = next_exit(&manager);
    eventually("the child to be counted gone", || {
        live.load(Ordering::SeqCst) == 0
    });
    assert!(
        returns_in_time(move || drop(instance)),
        "the drop waited for the blocked reader"
    );
    // The helper ends: the blocked reader ends with it, and nothing more is reported.
    fake.ctl.exit("the pipes close");
    std::thread::sleep(Duration::from_millis(100));
    assert_no_exit(&manager);
}

#[test]
fn dropping_a_group_whose_pipes_a_helper_still_holds_does_not_wait_for_the_helper() {
    let live = Arc::new(AtomicUsize::new(0));
    let (transport, fake) = idle_transport();
    let hub = Hub::start(transport, LiveGuard::new(&live));
    let member = hub
        .join(Attach {
            plugin_file: "a.clap".to_string(),
            plugin_id: "plugin.0".to_string(),
            block: "block-0".to_string(),
            doorbell: "bell-0".to_string(),
        })
        .expect("a member joins");
    fake.ctl
        .exit_silently("the plugin process exited with code 9 (0x9)");
    eventually("the group to end", || !hub.alive());
    assert!(
        returns_in_time(move || {
            drop(member);
            drop(hub);
        }),
        "the drop waited for the blocked reader"
    );
    fake.ctl.exit("the pipes close");
}
