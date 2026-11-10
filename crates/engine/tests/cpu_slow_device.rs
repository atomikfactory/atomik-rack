//! A slow device shows the biggest share of the CPU readout, serial and on workers, and the
//! measuring never allocates (every render below runs under the allocation guard). Own binary:
//! the clock and the switch are process-wide, and this one uses the real clock. The cost of the
//! measuring itself (two clock reads per device call) is pinned with a fake clock in
//! `cpu_timing.rs`.
#![allow(
    clippy::disallowed_methods,
    reason = "a busy-wait device and the clock of the readout read the wall clock by design"
)]
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::BATCH;
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, PrepareInfo, ProcessContext, SocketDescriptor,
    SocketId, SocketKind,
};
use atomik_engine::timing::{self, load_percent};
use atomik_engine::{
    DeviceInstanceId, Engine, EngineHandle, EngineOptions, Graph, Topology, new_engine_with_arena,
};
use atomik_testkit::fakes::{CONSTANT_SOURCE, ConstantSource};
use common::*;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn real_clock() -> u64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// How long a call of the slow device spins.
const SPIN: Duration = Duration::from_micros(600);

static SLOW: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.slow-device",
    name: "Slow Device",
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

/// Spins for [`SPIN`] in every call, writing silence.
struct Slow;

impl Device for Slow {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &SLOW
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: atomik_core::ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        ctx.output(SocketId(0)).fill(0.0);
        let until = Instant::now() + SPIN;
        while Instant::now() < until {
            std::hint::spin_loop();
        }
    }
}

fn id(n: u32) -> DeviceInstanceId {
    DeviceInstanceId(n)
}

/// Devices 1 and 3 are cheap sources, device 2 is the slow one.
fn build(threads: u16) -> (Engine, EngineHandle) {
    let c = cfg(0, 2);
    let (engine, mut handle) = if threads == 0 {
        new_engine_with_arena(c, EngineOptions::default(), 8)
    } else {
        engine_with(c, threads, None)
    };
    handle
        .insert_device(id(1), ConstantSource::boxed(0.1), &[])
        .unwrap();
    handle.insert_device(id(2), Box::new(Slow), &[]).unwrap();
    handle
        .insert_device(id(3), ConstantSource::boxed(0.3), &[])
        .unwrap();
    let mut t = Topology::new(&c);
    t.add_node(id(1), &CONSTANT_SOURCE).unwrap();
    t.add_node(id(2), &SLOW).unwrap();
    t.add_node(id(3), &CONSTANT_SOURCE).unwrap();
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

#[test]
fn the_slow_device_has_the_biggest_share_and_measuring_never_allocates() {
    assert!(timing::install_clock(real_clock));
    timing::set_enabled(true);
    for threads in [0u16, 2] {
        let (mut engine, handle) = build(threads);
        // `render_blocks` runs every batch under the allocation guard.
        let _ = render_blocks(&mut engine, &[], 2, BATCH, BATCH);
        let first: Vec<_> = (1..=3).map(|n| handle.meters().device_cpu(id(n))).collect();
        let batches = 12;
        let _ = render_blocks(&mut engine, &[], 2, batches * BATCH, BATCH);
        let last: Vec<_> = (1..=3)
            .map(|n| handle.meters().device_cpu(id(n)).unwrap())
            .collect();
        let busy = |n: usize| last[n].busy_ns;
        assert!(
            busy(1) >= batches as u64 * SPIN.as_nanos() as u64,
            "threads {threads}: the slow device spun in every call ({} ns)",
            busy(1)
        );
        assert!(
            busy(1) > busy(0) && busy(1) > busy(2),
            "threads {threads}: {} vs {} and {}",
            busy(1),
            busy(0),
            busy(2)
        );
        // The load is the busy share of the audio time rendered between two readings; it is
        // the biggest for the slow device whatever the host speed.
        let load = |n: usize| {
            let from = first[n].unwrap_or_default();
            load_percent(from, last[n]).unwrap()
        };
        assert!(load(1) > load(0) && load(1) > load(2), "threads {threads}");
    }
    timing::set_enabled(false);
}
