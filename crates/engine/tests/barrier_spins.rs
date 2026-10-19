//! xrun diagnostics: the audio thread's busy-wait at level barriers, counted per render call
//! (`Engine::last_render_barrier_spins`) and as a max that resets on read
//! (`Engine::take_max_barrier_spins`). Plain counters, no time source; telemetry only.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{BATCH, ParamId};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, PrepareInfo, ProcessContext, SocketDescriptor,
    SocketId, SocketKind,
};
use atomik_engine::{Engine, Graph};
use atomik_testkit::fakes::ConstantSource;
use common::*;

static SLOW: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "test.slow",
    name: "Slow",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[SocketDescriptor {
        id: SocketId(0),
        key: "audio_out",
        name: "audio_out",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    }],
};

/// A device that burns CPU in `process` (a busy loop, no time source, no allocation).
struct Slow;

impl Device for Slow {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &SLOW
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, _: &mut ProcessContext<'_>) {
        let mut x = 0u64;
        for i in 0..200_000u64 {
            x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(i));
        }
        std::hint::black_box(x);
    }
}

/// One level of two devices: a fast one (index 0, stays on the audio thread) and a slow one
/// (index 1, handed to the first worker when `threads > 0`).
fn engine(threads: u16) -> Engine {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    g.add_device(ConstantSource::boxed(0.0));
    g.add_device(Box::new(Slow));
    let (engine, mut handle) = engine_with(c, threads, None);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    // The handle may be dropped: the engine keeps rendering its current plan.
    engine
}

#[test]
fn audio_thread_spins_while_a_slow_worker_finishes() {
    let mut engine = engine(1);
    assert_eq!(
        engine.last_render_barrier_spins(),
        0,
        "nothing rendered yet"
    );
    assert_eq!(engine.take_max_barrier_spins(), 0);
    render_block(&mut engine, &[], 1, 2 * BATCH); // guarded: counting never allocates
    let last = engine.last_render_barrier_spins();
    assert!(last > 0, "the audio thread waited for the slow worker");
    let max = engine.take_max_barrier_spins();
    assert!(max >= last, "max {max} < last {last}");
    assert_eq!(engine.take_max_barrier_spins(), 0, "the max resets on read");
    assert_eq!(
        engine.last_render_barrier_spins(),
        last,
        "reading the max leaves the last render's count"
    );
}

#[test]
fn the_last_render_count_covers_only_the_latest_render_call() {
    let mut engine = engine(1);
    render_block(&mut engine, &[], 1, BATCH);
    let first = engine.last_render_barrier_spins();
    assert!(first > 0);
    // Fewer than BATCH frames from a batch boundary: no batch runs, so no barrier is reached.
    render_block(&mut engine, &[], 1, 1);
    assert_eq!(engine.last_render_barrier_spins(), 0);
    assert!(
        engine.take_max_barrier_spins() >= first,
        "the max still holds the earlier render"
    );
}

#[test]
fn single_threaded_engine_never_spins() {
    let mut engine = engine(0);
    render_block(&mut engine, &[], 1, 4 * BATCH);
    assert_eq!(engine.last_render_barrier_spins(), 0);
    assert_eq!(engine.take_max_barrier_spins(), 0);
}
