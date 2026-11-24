//! (memory): the heap one hosted instance of the test plugins holds in each format, on this
//! thread: the instance (its layout and the format's own tables), the activated processor with its
//! event lists and per-param queues, and the device. The gain effect has 4 params and the params
//! plugin 2 000, so the difference is the cost of a param. The figures are printed
//! (`cargo test -p atomik-plugin-vst3 --test memory_figures -- --nocapture`) and bounded.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_device_sdk::RunMode;
use atomik_testkit::no_alloc::held_heap_of;
use support::*;

fn kib(bytes: isize) -> String {
    format!("{:.1} KiB", bytes as f64 / 1024.0)
}

#[test]
fn an_instance_of_each_format_holds_this_much() {
    let _serial = serial();
    for format in FORMATS {
        let k = kit(format);
        // The first instance of a library loads it: not an instance's cost.
        drop(rig_with(&k, Plug::Gain, RunMode::Realtime));
        drop(rig_with(&k, Plug::Params, RunMode::Realtime));
        let (small, small_bytes) = held_heap_of(|| rig(&k, Plug::Gain));
        let (large, large_bytes) = held_heap_of(|| rig(&k, Plug::Params));
        let (small_n, large_n) = (small.layout.params.len(), large.layout.params.len());
        let per_param = (large_bytes - small_bytes) as f64 / (large_n - small_n) as f64;
        println!(
            "{}: {small_n} params {}, {large_n} params {} = {per_param:.0} B per param, {} fixed",
            format.name(),
            kib(small_bytes),
            kib(large_bytes),
            kib(small_bytes - (per_param * small_n as f64) as isize),
        );
        // CLAP: the host library's event buffer reserves about 11 MiB per instance that is never
        // written (it sizes its header vector by events times an event's byte size). VST3: the
        // event list and the param queues are written when made.
        let (small_ceiling, large_ceiling) = match format {
            Format::Clap => (12 * 1024, 12 * 1024 + 800),
            Format::Vst3 => (700, 1_900),
        };
        assert!(small_bytes <= small_ceiling * 1024, "{}", kib(small_bytes));
        assert!(large_bytes <= large_ceiling * 1024, "{}", kib(large_bytes));
        assert!(per_param <= 650.0, "{per_param:.0} B per param");
        drop((small, large));
    }
}
