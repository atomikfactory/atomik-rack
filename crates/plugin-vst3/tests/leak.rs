//! (library half) for both formats: creating, running, restarting and destroying plugin
//! instances in a loop leaves the plugin's own counters, the manager table, the interned
//! descriptors and the process threads exactly where they started.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_device_sdk::PluginRef;
use atomik_plugin_host::{ManagerEvent, PluginManager};
use atomik_testkit::harness::DeviceHarness;
use support::*;

fn package(format: Format) -> &'static str {
    match format {
        Format::Clap => "atomik-test-clap-leak",
        Format::Vst3 => "atomik-test-vst3-leak",
    }
}

fn leak_ref(format: Format) -> PluginRef {
    match format {
        Format::Clap => clap_ref("atomik.test.leak"),
        Format::Vst3 => vst3_ref(&format!(
            "{:08x}{:08x}{:08x}{:08x}",
            0xA70F_1C05u32, 0x4A5B_4C01u32, 0x8D2E_1F00u32, 1u32
        )),
    }
}

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

fn cycle(
    m: &mut PluginManager,
    plugin: &PluginRef,
    restart: bool,
) -> &'static atomik_device_sdk::DeviceDescriptor {
    let handle = m.instantiate(&request(plugin)).unwrap();
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

/// As [`leak_setup`], also returning the library's entry-call count from before the manager
/// touched it. The counters live in the library image: where the OS keeps an unloaded image
/// resident (macOS does for dylibs with thread-locals), an earlier test in this process has
/// already counted its entry call, so a test counts its own from this base.
fn leak_setup_counted(format: Format) -> (TestLib, PluginManager, PluginRef, u64) {
    let path = format.install(package(format));
    let lib = TestLib::open_path(&path);
    let entry_base = lib.counter(counter::ENTRY_INIT);
    let m = manager_for(format, &[path]);
    (lib, m, leak_ref(format), entry_base)
}

fn leak_setup(format: Format) -> (TestLib, PluginManager, PluginRef) {
    let (lib, m, plugin, _) = leak_setup_counted(format);
    (lib, m, plugin)
}

#[test]
fn five_hundred_instances_leave_every_counter_where_it_started() {
    let _s = serial();
    for format in FORMATS {
        let (lib, mut m, plugin, entry_base) = leak_setup_counted(format);
        // A first round warms everything up (the library is loaded, the descriptor is interned).
        let first = cycle(&mut m, &plugin, true);
        let threads_before = thread_count();
        let (created, destroyed) = (
            lib.counter(counter::CREATED),
            lib.counter(counter::DESTROYED),
        );
        let (activated, deactivated) = (
            lib.counter(counter::ACTIVATED),
            lib.counter(counter::DEACTIVATED),
        );
        assert_eq!(created, destroyed, "{}", format.name());
        for i in 0..500 {
            let d = cycle(&mut m, &plugin, i % 5 == 0);
            assert!(
                std::ptr::eq(d, first),
                "{}: the interned descriptor is reused (round {i})",
                format.name()
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
        assert_eq!(m.instance_count(), 0, "the manager table is empty");
        if let (Some(before), Some(after)) = (threads_before, thread_count()) {
            assert!(
                after <= before + 1,
                "{}: threads {before} -> {after}",
                format.name()
            );
        }
        assert_eq!(
            lib.counter(counter::ENTRY_INIT) - entry_base,
            1,
            "{}: the library entry ran once",
            format.name()
        );
    }
}

#[test]
fn parked_and_reattached_instances_are_destroyed_exactly_once() {
    let _s = serial();
    for format in FORMATS {
        let (lib, mut m, plugin) = leak_setup(format);
        let created = lib.counter(counter::CREATED);
        let destroyed = lib.counter(counter::DESTROYED);
        for _ in 0..50 {
            let handle = m.instantiate(&request(&plugin)).unwrap();
            let id = handle.id;
            m.retain(id, true);
            drop(handle.device);
            m.poll();
            let again = m.reattach(id, SR).unwrap();
            drop(again.device);
            m.poll();
            m.remove(id).unwrap();
        }
        assert_eq!(
            lib.counter(counter::CREATED) - created,
            50,
            "{}",
            format.name()
        );
        assert_eq!(lib.counter(counter::DESTROYED) - destroyed, 50);
        assert_eq!(m.instance_count(), 0);
    }
}
