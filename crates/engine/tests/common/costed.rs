//! Devices whose cost the test chooses, for the load rule tests. Costs are read from a fake clock
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
    /// This thread is a render worker (set when it starts).
    static WORKER: Cell<bool> = const { Cell::new(false) };
}

/// Start hook of the rig's workers: guarded against allocation, and marked as a worker so a
/// device can cost more there, as a node does once its data has moved to another core.
fn start_worker() {
    forbid_on_this_thread();
    WORKER.with(|w| w.set(true));
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
/// on by that much (the test changes the cost while the engine runs). A pinned one never
/// leaves the audio thread.
struct Costed {
    cost: Arc<AtomicU64>,
    /// Percent of `cost` the device takes on a worker thread (100: the same).
    on_worker: Arc<AtomicU64>,
    value: f32,
    pinned: bool,
}

impl Device for Costed {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &COSTED
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn pinned_to_audio_thread(&self) -> bool {
        self.pinned
    }
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let input = *ctx.input(SocketId(0));
        for (o, i) in ctx.output(SocketId(1)).iter_mut().zip(input) {
            *o = i * 0.5 + self.value;
        }
        let mut cost = self.cost.load(Ordering::Relaxed);
        if WORKER.with(Cell::get) {
            cost = cost * self.on_worker.load(Ordering::Relaxed) / 100;
        }
        NOW.with(|n| n.set(n.get() + cost));
    }
}

/// Outputs of the rig (one per device of its second level, up to this many).
const OUTPUTS: usize = 4;
/// Batches within which the load rule must notice a rise: the plan is sampled on every batch
/// while calm and the cost is smoothed half way per sample.
pub const ENTER_BOUND_BATCHES: usize = 3;
/// Batches after which a plan that became light must be calm: a second of audio plus a margin,
/// and for a new engine another second, the hold after the free-running transport starts.
pub const CALM_BOUND_BATCHES: usize = 2_000;
/// Batches during which a plan that became light must still be busy (well under a second).
pub const STILL_BUSY_BATCHES: usize = 500;

/// Nanoseconds of audio in one batch at the rig's 48 kHz.
pub fn budget_ns() -> u64 {
    (BATCH as f64 * 1e9 / 48_000.0) as u64
}

/// The cost of each node that makes `nodes` nodes take `percent` of the batch budget in all.
pub fn node_cost_for(nodes: usize, percent: u64) -> u64 {
    budget_ns() * percent / 100 / nodes as u64
}

pub struct Rig {
    pub engine: Engine,
    /// Kept alive: dropping the handle would end the engine's control side.
    _handle: EngineHandle,
    /// The cost each device of level 0 and of level 1 takes, in nanoseconds.
    pub costs: [Arc<AtomicU64>; 2],
    /// Percent of its cost a device takes on a worker thread (shared by every device).
    pub on_worker: Arc<AtomicU64>,
    /// Devices in the rig (`2 * width`).
    pub nodes: usize,
}

/// Two levels of `width` devices each: the sources (level 0) and one device after each source
/// (level 1); the first `OUTPUTS` of those feed an output. All start at `cost` nanoseconds.
/// `realtime` is the engine option; `gate` overrides the mode when given; `pinned` pins every
/// device to the audio thread.
pub fn rig(
    threads: u16,
    width: usize,
    cost: u64,
    realtime: bool,
    gate: Option<GateMode>,
    pinned: bool,
) -> Rig {
    let c = cfg(0, OUTPUTS as u16);
    let costs = [
        Arc::new(AtomicU64::new(cost)),
        Arc::new(AtomicU64::new(cost)),
    ];
    let on_worker = Arc::new(AtomicU64::new(100));
    let mut g = Graph::new(&c);
    for k in 0..width {
        let src = g.add_device(Box::new(Costed {
            cost: Arc::clone(&costs[0]),
            on_worker: Arc::clone(&on_worker),
            value: 0.1 * (k % 7 + 1) as f32,
            pinned,
        }));
        let next = g.add_device(Box::new(Costed {
            cost: Arc::clone(&costs[1]),
            on_worker: Arc::clone(&on_worker),
            value: 0.02 * (k % 5 + 1) as f32,
            pinned,
        }));
        g.connect(ep(src, SocketId(1)), ep(next, SocketId(0)))
            .unwrap();
        if k < OUTPUTS {
            let out = iface(&g, ["out_1", "out_2", "out_3", "out_4"][k]);
            g.connect(ep(next, SocketId(1)), out).unwrap();
        }
    }
    let (mut engine, mut handle) = new_engine_with_options(
        c,
        EngineOptions {
            workers: WorkerConfig {
                threads,
                on_thread_start: Some(start_worker),
            },
            realtime,
            ..EngineOptions::default()
        },
    );
    engine.set_level_cost_clock(fake_clock);
    if let Some(mode) = gate {
        engine.set_gate_mode(mode);
    }
    handle.submit_plan(g.compile().unwrap()).unwrap();
    Rig {
        engine,
        _handle: handle,
        costs,
        on_worker,
        nodes: 2 * width,
    }
}

impl Rig {
    /// Sets the cost of every device.
    pub fn set_cost(&self, ns: u64) {
        self.costs
            .iter()
            .for_each(|c| c.store(ns, Ordering::Relaxed));
    }
}

pub fn batch(engine: &mut Engine) -> Vec<Vec<f32>> {
    render_block(engine, &[], OUTPUTS, BATCH)
}

/// Renders batches until `done` holds, up to `bound`; returns how many it took.
pub fn until(
    engine: &mut Engine,
    bound: usize,
    what: &str,
    done: impl Fn(&Engine) -> bool,
) -> usize {
    for k in 1..=bound {
        batch(engine);
        if done(engine) {
            return k;
        }
    }
    panic!("not within {bound} batches: {what}");
}
