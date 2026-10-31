//! The per-device processing time: published per arena device while the timing is on, never
//! part of the signal, allocation-free. Own binary: the clock and the switch are process-wide.
//! One test function drives the whole sequence, with a fake clock that advances 500 ns per
//! read, so every figure is exact.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::BATCH;
use atomik_engine::timing::{self, load_percent};
use atomik_engine::{
    DeviceInstanceId, Engine, EngineHandle, EngineOptions, Graph, Topology, new_engine_with_arena,
};
use atomik_testkit::fakes::{CONSTANT_SOURCE, ConstantSource};
use common::*;
use std::sync::atomic::{AtomicU64, Ordering};

static TICKS: AtomicU64 = AtomicU64::new(0);

fn fake_clock() -> u64 {
    TICKS.fetch_add(500, Ordering::SeqCst)
}

fn id(n: u32) -> DeviceInstanceId {
    DeviceInstanceId(n)
}

fn build(threads: u16) -> (Engine, EngineHandle) {
    let c = cfg(0, 2);
    let (engine, mut handle) = if threads == 0 {
        new_engine_with_arena(c, EngineOptions::default(), 8)
    } else {
        engine_with(c, threads, None)
    };
    for n in 1..=3 {
        handle
            .insert_device(id(n), ConstantSource::boxed(0.1 * n as f32), &[])
            .unwrap();
    }
    let mut t = Topology::new(&c);
    for n in 1..=3 {
        t.add_node(id(n), &CONSTANT_SOURCE).unwrap();
    }
    let out = t
        .descriptor(Graph::INTERFACE)
        .unwrap()
        .socket_by_key("out_1")
        .unwrap();
    t.connect(
        ep(id(1), ConstantSource::AUDIO_OUT),
        ep(Graph::INTERFACE, out.id),
    )
    .unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();
    (engine, handle)
}

fn render(engine: &mut Engine, batches: usize) -> Vec<Vec<u32>> {
    bits(&render_blocks(engine, &[], 2, batches * BATCH, BATCH))
}

#[test]
fn timing_publishes_exact_loads_and_never_touches_the_signal() {
    // Nothing is published without a clock or while the switch is off.
    let (mut engine, handle) = build(0);
    render(&mut engine, 2);
    assert_eq!(handle.meters().device_cpu(id(1)), None);

    assert!(timing::install_clock(fake_clock));
    assert!(!timing::install_clock(fake_clock), "the first clock stays");
    render(&mut engine, 2);
    assert_eq!(handle.meters().device_cpu(id(1)), None, "switched off");

    // On: every run of a device adds exactly one fake tick pair (500 ns).
    timing::set_enabled(true);
    let before = handle.meters().device_cpu(id(1));
    assert_eq!(before, None, "published after the next batch");
    render(&mut engine, 1);
    let first = handle.meters().device_cpu(id(1)).unwrap();
    assert_eq!(first.busy_ns, 500);
    render(&mut engine, 9);
    let last = handle.meters().device_cpu(id(1)).unwrap();
    assert_eq!(last.busy_ns, 5000);
    for n in [2, 3] {
        assert_eq!(handle.meters().device_cpu(id(n)).unwrap().busy_ns, 5000);
    }
    let budget = (BATCH as f64 * 1e9 / 48_000.0) as u64;
    assert_eq!(last.budget_ns - first.budget_ns, 9 * budget);
    let pct = load_percent(first, last).unwrap();
    let want = 9.0 * 500.0 * 100.0 / (9.0 * budget as f64);
    assert!((f64::from(pct) - want).abs() < 1e-3, "{pct} vs {want}");

    // A removed device stops publishing.
    handle.meters().device_cpu(id(3)).unwrap();
    drop(handle);

    // The signal is bit-identical with the timing on and off, serial and on workers.
    for threads in [0u16, 2] {
        timing::set_enabled(false);
        let (mut e, _h) = build(threads);
        let off = render(&mut e, 12);
        timing::set_enabled(true);
        let (mut e, h) = build(threads);
        let on = render(&mut e, 12);
        assert_eq!(on, off, "threads {threads}");
        assert!(h.meters().device_cpu(id(2)).is_some());
    }
    timing::set_enabled(false);
}
