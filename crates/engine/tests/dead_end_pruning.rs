//! Dead-end pruning: a device whose output reaches nothing that can be observed is left out of
//! the plan. What keeps a device live (each observer alone), what a pruned device costs and
//! keeps, and what happens when a later routing runs it again: it starts from silence, with the
//! parameters set meanwhile, and nothing is allocated on the way.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{BATCH, NoteKind, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, MidiTap, PrepareInfo, ProcessContext,
    ScopeTap, SocketDescriptor, SocketId, SocketKind,
};
use atomik_engine::{
    DeviceInstanceId, Endpoint, Engine, EngineHandle, Graph, ScheduledNote, TapChannel,
    TapControls, Topology, new_engine, without_pruning,
};
use common::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

const AUDIO_IN: SocketId = SocketId(0);
const CV_IN: SocketId = SocketId(1);
const NOTES_IN: SocketId = SocketId(2);
const AUDIO_OUT: SocketId = SocketId(3);
const CV_OUT: SocketId = SocketId(4);
const NOTES_OUT: SocketId = SocketId(5);

const GAIN: ParamId = ParamId(0);
const LEVEL: ParamId = ParamId(1);

const fn sock(
    key: &'static str,
    id: u16,
    kind: SocketKind,
    direction: Direction,
) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name: key,
        kind,
        direction,
    }
}

const fn param(id: u32, key: &'static str, max: f32, default: f32) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name: key,
        min: 0.0,
        max,
        default,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    }
}

static PARAMS: [ParamDescriptor; 2] = [param(0, "gain", 4.0, 1.0), param(1, "level", 1.0, 0.0)];

static ECHO: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "test.dead-end-echo",
    name: "Echo",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &[
        sock("audio_in", 0, SocketKind::Audio, Direction::Input),
        sock("cv_in", 1, SocketKind::Cv, Direction::Input),
        sock("notes_in", 2, SocketKind::Notes, Direction::Input),
        sock("audio_out", 3, SocketKind::Audio, Direction::Output),
        sock("cv_out", 4, SocketKind::Cv, Direction::Output),
        sock("notes_out", 5, SocketKind::Notes, Direction::Output),
    ],
};

/// What a test reads from a device.
#[derive(Default)]
struct Counts {
    process: AtomicU64,
    reset: AtomicU64,
}

/// What a device publishes on its own (and so keeps it live).
#[derive(Clone, Copy, PartialEq)]
enum Own {
    Nothing,
    Values,
    Scope,
    Midi,
    Latency,
    Pinned,
}

const LINE: usize = 128;

/// `out = (in + level + 0.9 * line[t - 128]) * gain`: it holds sound for a long while after its
/// input stops. `level` is a constant it adds (a source when nothing is cabled in). Notes pass
/// from `notes_in` to `notes_out`; the CV output carries the audio.
struct Echo {
    counts: Arc<Counts>,
    own: Own,
    scope: Arc<ScopeTap>,
    midi: Arc<MidiTap>,
    line: [f32; LINE],
    pos: usize,
    gain: f32,
    level: f32,
}

impl Echo {
    fn boxed(own: Own) -> (Box<dyn Device>, Arc<Counts>) {
        let counts = Arc::new(Counts::default());
        let device = Self {
            counts: Arc::clone(&counts),
            own,
            scope: Arc::new(ScopeTap::new()),
            midi: Arc::new(MidiTap::new()),
            line: [0.0; LINE],
            pos: 0,
            gain: 1.0,
            level: 0.0,
        };
        (Box::new(device), counts)
    }
}

impl Device for Echo {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &ECHO
    }
    fn prepare(&mut self, _: &PrepareInfo) {
        self.line = [0.0; LINE];
        self.pos = 0;
    }
    fn reset(&mut self) {
        self.counts.reset.fetch_add(1, Relaxed);
        self.line = [0.0; LINE];
        self.pos = 0;
    }
    fn set_param(&mut self, id: ParamId, plain: f32) {
        match id {
            GAIN => self.gain = plain,
            LEVEL => self.level = plain,
            _ => {}
        }
    }
    fn has_meter_values(&self) -> bool {
        self.own == Own::Values
    }
    fn scope_tap(&self) -> Option<Arc<ScopeTap>> {
        (self.own == Own::Scope).then(|| Arc::clone(&self.scope))
    }
    fn midi_tap(&self) -> Option<Arc<MidiTap>> {
        (self.own == Own::Midi).then(|| Arc::clone(&self.midi))
    }
    fn variable_latency(&self) -> bool {
        self.own == Own::Latency
    }
    fn pinned_to_audio_thread(&self) -> bool {
        self.own == Own::Pinned
    }
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        self.counts.process.fetch_add(1, Relaxed);
        let input = *ctx.input(AUDIO_IN);
        let mut out = [0.0f32; BATCH];
        for (o, x) in out.iter_mut().zip(input) {
            let y = x + self.level + 0.9 * self.line[self.pos];
            self.line[self.pos] = y;
            self.pos = (self.pos + 1) % LINE;
            *o = y * self.gain;
        }
        *ctx.output(AUDIO_OUT) = out;
        *ctx.output(CV_OUT) = out;
        let _ = ctx.input(CV_IN);
        for i in 0..ctx.notes_in(NOTES_IN).len() {
            let event = ctx.notes_in(NOTES_IN)[i];
            let _ = ctx.notes_out(NOTES_OUT).push(event);
        }
    }
}

fn id(n: u32) -> DeviceInstanceId {
    DeviceInstanceId(n)
}

fn out_of(socket: SocketId, n: u32) -> Endpoint {
    ep(id(n), socket)
}

fn into(socket: SocketId, n: u32) -> Endpoint {
    ep(id(n), socket)
}

/// An engine with echo devices `1..=n` of the given kinds inserted, and the counts of each.
struct Rig {
    engine: Engine,
    handle: EngineHandle,
    topology: Topology,
    counts: Vec<Arc<Counts>>,
}

impl Rig {
    fn new(own: &[Own]) -> Self {
        let config = cfg(0, 1);
        let (engine, mut handle) = new_engine(config);
        let mut topology = Topology::new(&config);
        let mut counts = Vec::new();
        for (k, &o) in own.iter().enumerate() {
            let n = k as u32 + 1;
            let (device, c) = Echo::boxed(o);
            handle.insert_device(id(n), device, &[]).unwrap();
            topology.add_node(id(n), &ECHO).unwrap();
            counts.push(c);
        }
        topology.set_prune_dead_ends(true);
        Self {
            engine,
            handle,
            topology,
            counts,
        }
    }

    fn out(&self) -> Endpoint {
        let s = self
            .topology
            .descriptor(Graph::INTERFACE)
            .unwrap()
            .socket_by_key("out_1")
            .unwrap();
        ep(Graph::INTERFACE, s.id)
    }

    /// Compiles the topology the way the app does and returns the devices it leaves out.
    fn compile(&mut self) -> Vec<u32> {
        self.handle.fill_latencies(&mut self.topology);
        let plan = self.topology.compile().unwrap();
        let pruned = plan.pruned_devices().iter().map(|d| d.0).collect();
        self.handle.submit_routing(plan).unwrap();
        pruned
    }

    fn render(&mut self, batches: usize) -> Vec<f32> {
        render_blocks(&mut self.engine, &[], 1, batches * BATCH, BATCH).remove(0)
    }

    fn process_calls(&self, n: u32) -> u64 {
        self.counts[n as usize - 1].process.load(Relaxed)
    }

    fn resets(&self, n: u32) -> u64 {
        self.counts[n as usize - 1].reset.load(Relaxed)
    }
}

// ---------------------------------------------------------------------------------------------
// The rule: what keeps a device live
// ---------------------------------------------------------------------------------------------

#[test]
fn a_chain_into_the_interface_is_live_and_a_chain_into_nothing_is_not() {
    let mut rig = Rig::new(&[Own::Nothing; 5]);
    // 1 -> 2 -> interface; 3 -> 4 feeds nothing; 5 is alone.
    rig.topology
        .connect(out_of(AUDIO_OUT, 1), into(AUDIO_IN, 2))
        .unwrap();
    let out = rig.out();
    rig.topology.connect(out_of(AUDIO_OUT, 2), out).unwrap();
    rig.topology
        .connect(out_of(AUDIO_OUT, 3), into(AUDIO_IN, 4))
        .unwrap();
    assert_eq!(rig.compile(), vec![3, 4, 5]);
}

#[test]
fn a_meter_alone_keeps_a_device_and_its_chain_live() {
    let mut rig = Rig::new(&[Own::Nothing; 3]);
    rig.topology
        .connect(out_of(AUDIO_OUT, 1), into(AUDIO_IN, 2))
        .unwrap();
    rig.topology.add_meter(out_of(AUDIO_OUT, 2)).unwrap();
    assert_eq!(rig.compile(), vec![3]);
}

#[test]
fn a_cv_cable_and_a_notes_cable_keep_their_source_live() {
    let mut rig = Rig::new(&[Own::Nothing; 5]);
    let out = rig.out();
    // 3 is live (interface); 1 feeds its CV input, 2 its notes input; 4 and 5 are strays.
    rig.topology.connect(out_of(AUDIO_OUT, 3), out).unwrap();
    rig.topology
        .connect(out_of(CV_OUT, 1), into(CV_IN, 3))
        .unwrap();
    rig.topology
        .connect(out_of(NOTES_OUT, 2), into(NOTES_IN, 3))
        .unwrap();
    rig.topology
        .connect(out_of(NOTES_OUT, 4), into(NOTES_IN, 5))
        .unwrap();
    assert_eq!(rig.compile(), vec![4, 5]);
}

#[test]
fn every_device_that_publishes_something_itself_is_live() {
    let kinds = [
        Own::Values,
        Own::Scope,
        Own::Midi,
        Own::Latency,
        Own::Pinned,
    ];
    for kind in kinds {
        // Device 2 publishes; 1 feeds it; 3 is a stray with no cables.
        let mut rig = Rig::new(&[Own::Nothing, kind, Own::Nothing]);
        rig.topology
            .connect(out_of(AUDIO_OUT, 1), into(AUDIO_IN, 2))
            .unwrap();
        assert_eq!(rig.compile(), vec![3], "a device that publishes is live");
    }
}

#[test]
fn a_note_tap_and_a_record_tap_source_keep_a_device_live() {
    let mut rig = Rig::new(&[Own::Nothing; 4]);
    assert_eq!(rig.compile(), vec![1, 2, 3, 4]);
    let _tap = rig.handle.tap_notes(id(2)).unwrap();
    let controls = Arc::new(TapControls::new(1));
    let channels = [TapChannel::source(id(3), AUDIO_OUT, 0)];
    let _rec = rig
        .handle
        .record_tap_gained(&channels, 4096, controls)
        .unwrap();
    assert_eq!(rig.compile(), vec![1, 4]);
    rig.handle.untap_notes(id(2)).unwrap();
    rig.handle.stop_record_tap().unwrap();
    assert_eq!(rig.compile(), vec![1, 2, 3, 4]);
}

#[test]
fn a_tap_on_a_pruned_device_asks_for_a_new_routing() {
    let mut rig = Rig::new(&[Own::Nothing; 2]);
    rig.compile();
    let epoch = rig.handle.latency_epoch();
    let _tap = rig.handle.tap_notes(id(1)).unwrap();
    assert_ne!(rig.handle.latency_epoch(), epoch, "the app compiles again");
    // A tap on a live device changes nothing.
    let out = rig.out();
    rig.topology.connect(out_of(AUDIO_OUT, 2), out).unwrap();
    rig.compile();
    let epoch = rig.handle.latency_epoch();
    let _tap = rig.handle.tap_notes(id(2)).unwrap();
    assert_eq!(rig.handle.latency_epoch(), epoch);
}

#[test]
fn a_graph_pins_the_devices_it_is_told_to_and_the_ones_that_publish() {
    let build = |pin: bool| {
        let mut g = Graph::new(&cfg(0, 1));
        let (stray, _) = Echo::boxed(Own::Nothing);
        let (seen, _) = Echo::boxed(Own::Values);
        let (tapped, _) = Echo::boxed(Own::Nothing);
        g.add_device(stray);
        g.add_device(seen);
        let tapped = g.add_device(tapped);
        if pin {
            g.pin(tapped).unwrap();
        }
        g.set_prune_dead_ends(true);
        let topology = g.topology();
        let plan = g.compile().unwrap();
        (plan.pruned_devices(), topology.pinned().count())
    };
    assert_eq!(build(true), (vec![id(1)], 2));
    assert_eq!(build(false), (vec![id(1), id(3)], 1));
}

#[test]
fn nothing_is_left_out_unless_the_compile_asks_for_it() {
    let mut g = Graph::new(&cfg(0, 1));
    let (stray, _) = Echo::boxed(Own::Nothing);
    g.add_device(stray);
    assert!(g.compile().unwrap().pruned_devices().is_empty());
    let mut g = Graph::new(&cfg(0, 1));
    let (stray, _) = Echo::boxed(Own::Nothing);
    g.add_device(stray);
    g.set_prune_dead_ends(true);
    assert_eq!(g.compile().unwrap().pruned_devices(), vec![id(1)]);
    // The test switch turns it off on the calling thread.
    let mut g = Graph::new(&cfg(0, 1));
    let (stray, _) = Echo::boxed(Own::Nothing);
    g.add_device(stray);
    g.set_prune_dead_ends(true);
    let plan = without_pruning(|| g.compile().unwrap());
    assert!(plan.pruned_devices().is_empty());
}

// ---------------------------------------------------------------------------------------------
// What a pruned device costs and keeps
// ---------------------------------------------------------------------------------------------

#[test]
fn a_pruned_device_is_in_no_step_and_is_never_processed() {
    let mut rig = Rig::new(&[Own::Nothing; 3]);
    let out = rig.out();
    rig.topology.connect(out_of(AUDIO_OUT, 1), out).unwrap();
    rig.topology
        .connect(out_of(AUDIO_OUT, 2), into(AUDIO_IN, 3))
        .unwrap();
    let plan = {
        rig.handle.fill_latencies(&mut rig.topology);
        rig.topology.compile().unwrap()
    };
    assert_eq!(plan.order(), &[id(1)]);
    assert_eq!(plan.pruned_devices(), vec![id(2), id(3)]);
    rig.handle.submit_routing(plan).unwrap();
    rig.render(20);
    assert_eq!(rig.engine.pruned_devices(), vec![id(2), id(3)]);
    assert_eq!(rig.process_calls(1), 20);
    assert_eq!(rig.process_calls(2), 0);
    assert_eq!(rig.process_calls(3), 0);
    assert!(rig.handle.contains_device(id(3)), "the arena still owns it");
}

#[test]
fn notes_for_a_pruned_device_are_discarded_without_counting_a_drop() {
    let mut rig = Rig::new(&[Own::Nothing; 2]);
    let out = rig.out();
    rig.topology.connect(out_of(AUDIO_OUT, 1), out).unwrap();
    rig.compile();
    rig.render(2);
    for frame in [0u64, 70, 140] {
        rig.handle
            .send_note(ScheduledNote {
                frame,
                device: id(2),
                socket: NOTES_IN,
                kind: NoteKind::On {
                    key: 60,
                    velocity: 0.5,
                },
            })
            .unwrap();
    }
    rig.render(4);
    assert_eq!(rig.handle.dropped_events(), 0);
    // A device that is not in the arena at all still counts.
    rig.handle
        .send_note(ScheduledNote {
            frame: 1000,
            device: id(9),
            socket: NOTES_IN,
            kind: NoteKind::Off { key: 60 },
        })
        .unwrap();
    rig.render(20);
    assert_eq!(rig.handle.dropped_events(), 1);
}

#[test]
fn parameters_set_while_pruned_are_in_force_when_the_device_runs_again() {
    let mut rig = Rig::new(&[Own::Nothing]);
    rig.compile();
    rig.render(4);
    rig.handle.set_param(id(1), GAIN, 0.5).unwrap();
    rig.handle.set_param(id(1), LEVEL, 0.25).unwrap();
    rig.render(4);
    assert_eq!(rig.process_calls(1), 0);
    let out = rig.out();
    rig.topology.connect(out_of(AUDIO_OUT, 1), out).unwrap();
    rig.compile();
    // The first batch is the engine's output latency; the second one is the device's first:
    // level 0.25 with the (reset) line empty, times gain 0.5.
    let heard = rig.render(3);
    assert!((heard[BATCH] - 0.125).abs() < 1e-6, "{}", heard[BATCH]);
}

// ---------------------------------------------------------------------------------------------
// The return rule
// ---------------------------------------------------------------------------------------------

/// A device holding sound, left out while it rings, plays no stale sound when it comes back.
#[test]
fn a_device_that_returns_plays_no_stale_sound() {
    let mut rig = Rig::new(&[Own::Nothing; 2]);
    let out = rig.out();
    rig.topology.connect(out_of(AUDIO_OUT, 1), out).unwrap();
    rig.topology
        .connect(out_of(AUDIO_OUT, 2), into(AUDIO_IN, 1))
        .unwrap();
    assert!(rig.compile().is_empty());
    // A burst goes into the chain, then rings in the echo (device 1).
    rig.handle.set_param(id(2), LEVEL, 0.5).unwrap();
    rig.render(1);
    rig.handle.set_param(id(2), LEVEL, 0.0).unwrap();
    let ringing = rig.render(12);
    assert!(ringing.iter().any(|x| x.abs() > 0.1), "it rings");
    // Unplug its output: both devices are dead ends now.
    rig.topology.disconnect(out).unwrap();
    assert_eq!(rig.compile(), vec![1, 2]);
    rig.render(300);
    let before = (rig.process_calls(1), rig.process_calls(2));
    rig.render(10);
    assert_eq!((rig.process_calls(1), rig.process_calls(2)), before);
    // Plug it back: nothing of the old sound comes out, ever.
    rig.topology.connect(out_of(AUDIO_OUT, 1), out).unwrap();
    assert!(rig.compile().is_empty());
    let heard = rig.render(40);
    assert!(
        heard.iter().all(|x| x.to_bits() == 0),
        "no stale echo after the device returns"
    );
    assert_eq!((rig.resets(1), rig.resets(2)), (1, 1));
    // And a device that did not return is not reset again.
    rig.topology.disconnect(out).unwrap();
    rig.compile();
    rig.render(4);
    rig.topology.connect(out_of(AUDIO_OUT, 1), out).unwrap();
    rig.compile();
    rig.render(4);
    assert_eq!((rig.resets(1), rig.resets(2)), (2, 2));
}

/// The sound rings on when nothing prunes (the control of the test above).
#[test]
fn without_the_pruning_the_same_edit_leaves_the_echo_running() {
    let mut rig = Rig::new(&[Own::Nothing; 2]);
    let out = rig.out();
    rig.topology.set_prune_dead_ends(false);
    rig.topology.connect(out_of(AUDIO_OUT, 1), out).unwrap();
    rig.topology
        .connect(out_of(AUDIO_OUT, 2), into(AUDIO_IN, 1))
        .unwrap();
    rig.compile();
    rig.handle.set_param(id(2), LEVEL, 0.5).unwrap();
    rig.render(1);
    rig.handle.set_param(id(2), LEVEL, 0.0).unwrap();
    rig.render(12);
    rig.topology.disconnect(out).unwrap();
    assert!(rig.compile().is_empty());
    rig.render(20);
    assert!(rig.process_calls(1) > 30, "it kept running");
    assert_eq!(rig.resets(1), 0);
}

#[test]
fn a_device_that_never_ran_is_not_reset_when_it_first_runs() {
    let mut rig = Rig::new(&[Own::Nothing]);
    assert_eq!(rig.compile(), vec![1]);
    rig.render(8);
    let out = rig.out();
    rig.topology.connect(out_of(AUDIO_OUT, 1), out).unwrap();
    assert!(rig.compile().is_empty());
    rig.render(2);
    assert_eq!(rig.resets(1), 0);
    assert_eq!(rig.process_calls(1), 2);
}

/// The first batch of a device that returns is timed again (its old cost is not trusted), and a
/// worker count changes nothing about what is pruned or what is heard.
#[test]
fn zero_and_four_workers_prune_the_same_devices_and_hear_the_same_bits() {
    let run = |threads: u16| {
        let config = cfg(0, 1);
        let (mut engine, mut handle) = engine_with(config, threads, None);
        let mut topology = Topology::new(&config);
        topology.set_prune_dead_ends(true);
        for n in 1..=8u32 {
            let (device, _) = Echo::boxed(Own::Nothing);
            handle.insert_device(id(n), device, &[]).unwrap();
            topology.add_node(id(n), &ECHO).unwrap();
        }
        let s = topology
            .descriptor(Graph::INTERFACE)
            .unwrap()
            .socket_by_key("out_1")
            .unwrap();
        let out = ep(Graph::INTERFACE, s.id);
        // 1 and 2 feed the interface through 3; 4..=8 are dead ends playing into each other.
        topology
            .connect(out_of(AUDIO_OUT, 1), into(AUDIO_IN, 3))
            .unwrap();
        topology.connect(out_of(AUDIO_OUT, 3), out).unwrap();
        for n in 4..8u32 {
            topology
                .connect(out_of(AUDIO_OUT, n), into(AUDIO_IN, n + 1))
                .unwrap();
        }
        handle.set_param(id(1), LEVEL, 0.5).unwrap();
        handle.set_param(id(4), LEVEL, 0.5).unwrap();
        let mut heard = Vec::new();
        let mut pruned = Vec::new();
        for round in 0..4 {
            handle.fill_latencies(&mut topology);
            let plan = topology.compile().unwrap();
            pruned.push(plan.pruned_devices());
            handle.submit_routing(plan).unwrap();
            heard.extend(render_blocks(&mut engine, &[], 1, 20 * BATCH, 100).remove(0));
            // The last dead end of the chain goes to the interface for a round, then back again.
            topology.disconnect(out).unwrap();
            let live = if round % 2 == 0 { 8 } else { 3 };
            topology.connect(out_of(AUDIO_OUT, live), out).unwrap();
        }
        (
            pruned,
            heard.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        )
    };
    let (pruned0, bits0) = run(0);
    let (pruned4, bits4) = run(4);
    assert_eq!(pruned0, pruned4);
    assert_eq!(bits0, bits4);
    assert_eq!(pruned0[0], [2, 4, 5, 6, 7, 8].map(id).to_vec());
    assert_ne!(pruned0[0], pruned0[1], "the edits change what is left out");
}

// ---------------------------------------------------------------------------------------------
// Allocation: a swap that prunes and a swap that revives
// ---------------------------------------------------------------------------------------------

#[test]
fn a_swap_that_prunes_and_one_that_revives_allocate_nothing() {
    // Every render is guarded; the routing is built before the render, off the audio thread.
    let mut rig = Rig::new(&[Own::Nothing; 6]);
    let out = rig.out();
    rig.topology.connect(out_of(AUDIO_OUT, 1), out).unwrap();
    for n in 2..6u32 {
        rig.topology
            .connect(out_of(AUDIO_OUT, n), into(AUDIO_IN, n + 1))
            .unwrap();
    }
    rig.handle.set_param(id(2), LEVEL, 0.3).unwrap();
    rig.handle.set_param(id(1), LEVEL, 0.3).unwrap();
    assert_eq!(rig.compile(), vec![2, 3, 4, 5, 6]);
    rig.render(6);
    // Revive: 2..=6 become live through 6 -> interface; device 1 is the dead end now.
    rig.topology.disconnect(out).unwrap();
    rig.topology.connect(out_of(AUDIO_OUT, 6), out).unwrap();
    assert_eq!(rig.compile(), vec![1]);
    rig.render(6);
    // Prune the chain again, then revive it again.
    rig.topology.disconnect(out).unwrap();
    rig.topology.connect(out_of(AUDIO_OUT, 1), out).unwrap();
    assert_eq!(rig.compile(), vec![2, 3, 4, 5, 6]);
    rig.render(6);
    rig.topology.disconnect(out).unwrap();
    rig.topology.connect(out_of(AUDIO_OUT, 6), out).unwrap();
    assert_eq!(rig.compile(), vec![1]);
    rig.render(6);
    assert_eq!(
        rig.resets(4),
        1,
        "reset when it returned after it had run, not before"
    );
}
