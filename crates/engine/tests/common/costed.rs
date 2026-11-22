//! Devices whose cost the test chooses, for the level gate tests. Costs are read from a fake clock
//! that only the devices advance, so no test depends on how fast the machine is.
use super::*;
use atomik_core::{BATCH, ParamId};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, PrepareInfo, ProcessContext, SocketDescriptor,
    SocketId, SocketKind,
};
use atomik_engine::GateMode;
use atomik_testkit::no_alloc::forbid_on_this_thread;
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

thread_local! {
    /// The fake clock of the thread: it moves only when a [`Costed`] device runs.
    static NOW: Cell<u64> = const { Cell::new(0) };
}

fn fake_clock() -> u64 {
    NOW.with(Cell::get)
}

static COSTED: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "test.costed",
    name: "Costed",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[
        SocketDescriptor {
            id: SocketId(0),
            key: "audio_in",
            name: "audio_in",
            kind: SocketKind::Audio,
            direction: Direction::Input,
        },
        SocketDescriptor {
            id: SocketId(1),
            key: "audio_out",
            name: "audio_out",
            kind: SocketKind::Audio,
            direction: Direction::Output,
        },
    ],
};

/// A device that "takes" `cost` nanoseconds: it advances the fake clock of the thread it runs
/// on by that much (the test changes the cost while the engine runs).
struct Costed {
    cost: Arc<AtomicU64>,
    value: f32,
}

impl Device for Costed {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &COSTED
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let input = *ctx.input(SocketId(0));
        for (o, i) in ctx.output(SocketId(1)).iter_mut().zip(input) {
            *o = i * 0.5 + self.value;
        }
        NOW.with(|n| n.set(n.get() + self.cost.load(Ordering::Relaxed)));
    }
}

pub const WIDTH: usize = 4;
/// Batches within which a real change of load must flip a level: the cost is sampled once in 32
/// batches and smoothed over four samples at most, plus the dwell, with a margin.
pub const FLIP_BOUND_BATCHES: usize = 320;
pub const CHEAP: u64 = 500;
pub const HEAVY: u64 = 40_000;

pub struct Rig {
    pub engine: Engine,
    /// Kept alive: dropping the handle would end the engine's control side.
    _handle: EngineHandle,
    /// The cost each device of level 0 and of level 1 takes, in nanoseconds.
    pub costs: [Arc<AtomicU64>; 2],
}

/// Two levels of `WIDTH` devices each: the sources (level 0) and one device after each source
/// (level 1), each feeding an output.
pub fn rig(threads: u16, gate: Option<GateMode>) -> Rig {
    let c = cfg(0, WIDTH as u16);
    let costs = [
        Arc::new(AtomicU64::new(CHEAP)),
        Arc::new(AtomicU64::new(CHEAP)),
    ];
    let mut g = Graph::new(&c);
    for k in 0..WIDTH {
        let src = g.add_device(Box::new(Costed {
            cost: Arc::clone(&costs[0]),
            value: 0.1 * (k + 1) as f32,
        }));
        let next = g.add_device(Box::new(Costed {
            cost: Arc::clone(&costs[1]),
            value: 0.02 * (k + 1) as f32,
        }));
        g.connect(ep(src, SocketId(1)), ep(next, SocketId(0)))
            .unwrap();
        let out = iface(&g, ["out_1", "out_2", "out_3", "out_4"][k]);
        g.connect(ep(next, SocketId(1)), out).unwrap();
    }
    let (mut engine, mut handle) = engine_with_gate(c, threads, Some(forbid_on_this_thread));
    engine.set_level_cost_clock(fake_clock);
    if let Some(mode) = gate {
        engine.set_level_gate_mode(mode);
    }
    handle.submit_plan(g.compile().unwrap()).unwrap();
    Rig {
        engine,
        _handle: handle,
        costs,
    }
}

pub fn batch(engine: &mut Engine) -> Vec<Vec<f32>> {
    render_block(engine, &[], WIDTH, BATCH)
}

/// Renders batches until `done` holds, up to the flip bound.
pub fn until(engine: &mut Engine, what: &str, done: impl Fn(&Engine) -> bool) {
    for _ in 0..FLIP_BOUND_BATCHES {
        batch(engine);
        if done(engine) {
            return;
        }
    }
    panic!("not within {FLIP_BOUND_BATCHES} batches: {what}");
}
