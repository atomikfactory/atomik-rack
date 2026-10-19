//! (library half): creating, running, restarting and destroying plugin instances in a
//! loop leaves the plugin's own counters, the manager's table, the interned descriptors and
//! the process's threads exactly where they started.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_plugin_host::{ManagerEvent, PluginManager};
use atomik_testkit::harness::DeviceHarness;
use support::*;

const PKG: &str = "atomik-test-clap-leak";
const ID: &str = "atomik.test.leak";

/// The number of threads of this process, where it can be had cheaply.
fn thread_count() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir("/proc/self/task").ok().map(|d| d.count())
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {}).Threads.Count", std::process::id()),
            ])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        None
    }
}

fn cycle(m: &mut PluginManager, restart: bool) -> &'static atomik_device_sdk::DeviceDescriptor {
    let handle = m.instantiate(&request(ID)).unwrap();
    let (id, descriptor) = (handle.id, handle.descriptor);
    let mut h = DeviceHarness::new(Box::new(handle.device), SR);
    *h.input_mut("in_l") = [0.5; 64];
    for _ in 0..20 {
        h.process();
    }
    if restart {
        m.restart(id).unwrap();
        for _ in 0..4 {
            h.process();
            m.poll();
        }
    }
    drop(h);
    let events = m.poll();
    assert!(events.contains(&ManagerEvent::Destroyed(id)), "{events:?}");
    descriptor
}

#[test]
fn five_hundred_instances_leave_every_counter_where_it_started() {
    let _s = serial();
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    // A first round warms everything up (the library is loaded, the descriptor is interned).
    let first = cycle(&mut m, true);
    let threads_before = thread_count();
    let (created, destroyed) = (
        lib.counter(counter::CREATED),
        lib.counter(counter::DESTROYED),
    );
    let (activated, deactivated) = (
        lib.counter(counter::ACTIVATED),
        lib.counter(counter::DEACTIVATED),
    );
    assert_eq!(created, destroyed);
    for i in 0..500 {
        let d = cycle(&mut m, i % 5 == 0);
        assert!(
            std::ptr::eq(d, first),
            "the interned descriptor is reused (round {i})"
        );
    }
    let created_now = lib.counter(counter::CREATED);
    assert_eq!(created_now - created, 500);
    assert_eq!(
        lib.counter(counter::DESTROYED),
        created_now,
        "every instance destroyed"
    );
    assert_eq!(
        lib.counter(counter::ACTIVATED) - activated,
        lib.counter(counter::DEACTIVATED) - deactivated,
        "every activation deactivated"
    );
    assert_eq!(m.instance_count(), 0, "the manager's table is empty");
    if let (Some(before), Some(after)) = (threads_before, thread_count()) {
        assert!(after <= before + 1, "threads {before} -> {after}");
    }
    assert_eq!(
        lib.counter(counter::ENTRY_INIT),
        1,
        "the library entry was initialised once"
    );
}

#[test]
fn parked_and_reattached_instances_are_destroyed_exactly_once() {
    let _s = serial();
    let lib = TestLib::open(PKG);
    let mut m = manager_for(&[PKG]);
    let created = lib.counter(counter::CREATED);
    let destroyed = lib.counter(counter::DESTROYED);
    for _ in 0..50 {
        let handle = m.instantiate(&request(ID)).unwrap();
        let id = handle.id;
        m.retain(id, true);
        drop(handle.device);
        m.poll();
        let again = m.reattach(id, SR).unwrap();
        drop(again.device);
        m.poll();
        m.remove(id).unwrap();
    }
    assert_eq!(lib.counter(counter::CREATED) - created, 50);
    assert_eq!(lib.counter(counter::DESTROYED) - destroyed, 50);
    assert_eq!(m.instance_count(), 0);
}
