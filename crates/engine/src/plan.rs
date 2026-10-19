//! Compilation of a [`Graph`] (or a [`crate::Topology`]) into a routing-only
//! [`RoutingPlan`], and the [`RenderPlan`] that adds prepared devices to it.
//!
//! **Split.** A device instance with its per-socket scratch is a [`Node`]; nodes live
//! in the engine's arena ([`crate::arena::Arena`]) and survive routing swaps. Everything that
//! depends on the cables (order, levels, shared slots, per-device routes, `connected` flags)
//! lives in the [`RoutingPlan`], which references nodes by arena slot.

use crate::arena::{Arena, ConsoleRouting, RoutingPlan};
use crate::console::{
    DelayLine, MAX_COMPENSATION_SECONDS, METER_CAPACITY, Meters, PdcReport, adopt_delay_lines,
    meter_key,
};
use crate::graph::rules::{self, CableMap, Lookup};
use crate::graph::{DeviceInstanceId, Endpoint, Graph, GraphError};
use atomik_core::{
    BATCH, Batch, MAX_EVENTS_PER_BATCH, NoteBuffer, NoteEvent, NoteKind, ParamId, TimeInfo,
};
use atomik_device_sdk::{
    Device, DeviceDescriptor, Direction, PrepareInfo, ProcessContext, SocketDescriptor, SocketId,
    SocketKind,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

/// Mixes the patch seed with a device id into the device's `prepare` seed (splitmix64).
pub(crate) fn mix_seed(seed: u64, id: DeviceInstanceId) -> u64 {
    fn splitmix(mut z: u64) -> u64 {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    splitmix(seed ^ splitmix(u64::from(id.0)))
}

/// A `(socket index, slot index)` pair (for interface routes: `(host channel, slot)`).
pub(crate) type Route = (usize, usize);

/// One cable into an Audio/Cv summing input: added to the input's scratch, after an
/// optional PDC delay line (index into `RoutingPlan::delays`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SumRoute {
    pub(crate) socket: usize,
    pub(crate) slot: usize,
    pub(crate) delay: Option<usize>,
}

/// One device instance with its preallocated per-socket scratch buffers. Lives in the arena;
/// knows nothing about cables except the `connected` flags copied from the current routing.
pub(crate) struct Node {
    pub(crate) device: Box<dyn Device>,
    pub(crate) sockets: &'static [SocketDescriptor],
    /// Stage of each socket (§9; empty = every socket is stage 0).
    pub(crate) stages: &'static [u8],
    /// One batch per socket (used by Audio/Cv sockets).
    pub(crate) signals: Vec<Batch>,
    /// One buffer per socket: capacity `MAX_EVENTS_PER_BATCH` for Notes sockets, 0 otherwise.
    pub(crate) notes: Vec<NoteBuffer>,
    /// Per socket, from the routing that last included this node.
    pub(crate) connected: Vec<bool>,
    /// True while the current routing includes this node (only then it receives notes).
    pub(crate) routed: bool,
    /// Audio rendered for this device in the current batch (audio-track clips),
    /// valid while `playback_on`; cleared after the device's last stage.
    pub(crate) playback: [Batch; 2],
    pub(crate) playback_on: bool,
    /// `Device::has_meter_values()` read at construction.
    pub(crate) has_meter_values: bool,
}

/// The cable routes of one stage of one device in one routing (a single-stage
/// device has one, covering every socket).
pub(crate) struct Routes {
    /// Cabled Audio/Cv inputs: copy slot -> scratch before `process`.
    pub(crate) signal_in: Vec<Route>,
    /// Cabled Audio/Cv outputs: copy scratch -> slot after `process`.
    pub(crate) signal_out: Vec<Route>,
    /// Cabled Notes inputs.
    pub(crate) notes_in: Vec<Route>,
    /// Cabled Notes outputs.
    pub(crate) notes_out: Vec<Route>,
    /// Cables into Audio/Cv summing inputs, sorted by (socket, source endpoint): the summing
    /// order.
    pub(crate) sum_in: Vec<SumRoute>,
    /// Metered Audio/Cv outputs: `(socket, meter slot)`.
    pub(crate) meters: Vec<Route>,
}

/// One device of a routing: which arena slot it runs from, and its `connected` flags.
pub(crate) struct Routed {
    pub(crate) id: DeviceInstanceId,
    /// Arena slot. `RenderPlan`s use their position; `EngineHandle::submit_routing` rewrites it
    /// from the handle's id -> slot map.
    pub(crate) slot: usize,
    pub(crate) descriptor: &'static DeviceDescriptor,
    /// `ProcessContext` connected flags, copied into the node when the routing is installed.
    pub(crate) connected: Vec<bool>,
}

/// One (device, stage) node in render order: gather the stage's inputs, run
/// `process_stage`, scatter the stage's outputs. Single-stage devices have one step.
pub(crate) struct Step {
    /// Index into `RoutingPlan::devices`.
    pub(crate) device: usize,
    pub(crate) stage: u8,
    /// The device's last stage: its note inputs are consumed after it.
    pub(crate) last: bool,
    pub(crate) routes: Routes,
}

/// An immutable, compiled graph: a [`RoutingPlan`] plus its prepared devices, every buffer
/// preallocated. Only [`Graph::compile`] constructs one; it has no public mutators.
///
/// **Levels.** A device's dependency level is 0 without device producers,
/// else 1 + the highest level of its producers (cables from the AudioInterface do not count).
/// Devices are stored level-major (by level, then id), which is a topological order; slot
/// liveness is computed on that order. Devices of one level never read each other's outputs,
/// so a level can run in parallel as long as all of its inputs are gathered before any of its
/// outputs are scattered.
///
/// Submitting a `RenderPlan` replaces the engine's whole device arena and routing
/// at a batch boundary; the previous arena (with every device in it) and routing are handed
/// back together as one retired item.
pub struct RenderPlan {
    pub(crate) routing: RoutingPlan,
    /// Prepared devices, `nodes[k]` for `routing.devices[k]` (whose slot is `k`).
    pub(crate) nodes: Vec<Node>,
}

impl std::fmt::Debug for RenderPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderPlan")
            .field("order", &self.order())
            .field("slot_count", &self.slot_count())
            .finish_non_exhaustive()
    }
}

impl RenderPlan {
    /// Devices in processing order (the AudioInterface is not listed: its host inputs are
    /// written before the first device and its host outputs read after the last).
    pub fn order(&self) -> &[DeviceInstanceId] {
        self.routing.order()
    }

    /// Devices grouped by dependency level, in processing order. Level 0 devices
    /// have no device producers; every other device is one level above its highest producer.
    pub fn levels(&self) -> impl Iterator<Item = &[DeviceInstanceId]> {
        self.routing.levels()
    }

    /// Number of shared signal buffers (Audio/Cv slots plus Notes slots) after liveness reuse.
    pub fn slot_count(&self) -> usize {
        self.routing.slot_count()
    }

    /// Delay-compensation report of this plan (see [`crate::RoutingPlan::pdc_report`]).
    pub fn pdc_report(&self) -> &PdcReport {
        self.routing.pdc_report()
    }

    #[cfg(test)]
    pub(crate) fn assert_no_output_aliases_input(&self) {
        self.routing.assert_no_output_aliases_input();
    }
}

/// Non-RT construction of a node (allocates its scratch).
impl Node {
    pub(crate) fn new(device: Box<dyn Device>) -> Self {
        let sockets = device.descriptor().sockets;
        Self {
            has_meter_values: device.has_meter_values(),
            stages: device.descriptor().stages,
            device,
            sockets,
            signals: vec![[0.0; BATCH]; sockets.len()],
            notes: sockets
                .iter()
                .map(|s| {
                    NoteBuffer::with_capacity(if is_notes(s.kind) {
                        MAX_EVENTS_PER_BATCH
                    } else {
                        0
                    })
                })
                .collect(),
            connected: vec![false; sockets.len()],
            routed: false,
            playback: [[0.0; BATCH]; 2],
            playback_on: false,
        }
    }
}

/// Real-time side of a node. Nothing here allocates, frees, locks or blocks.
impl Node {
    /// Sets a param (clamped through the descriptor). Unknown params are ignored.
    pub(crate) fn set_param(&mut self, param: ParamId, plain: f32) {
        if let Some(desc) = self.device.descriptor().params.get(param.0 as usize) {
            self.device.set_param(param, desc.clamp(plain));
        }
    }

    /// Queues a note into a Notes input scratch for the current batch. Returns false (caller
    /// counts a drop) if the target is not a Notes input or the buffer is full. Offsets are
    /// kept non-decreasing even if notes arrive out of order.
    pub(crate) fn deliver_note(&mut self, socket: SocketId, offset: u16, kind: NoteKind) -> bool {
        let i = usize::from(socket.0);
        match self.sockets.get(i) {
            Some(s) if s.direction.is_input() && is_notes(s.kind) => {}
            _ => return false,
        }
        let buf = &mut self.notes[i];
        let floor = buf.as_slice().last().map_or(0, |last| last.offset);
        let offset = offset.clamp(floor, BATCH as u16 - 1);
        buf.push(NoteEvent { offset, kind }).is_ok()
    }

    /// Stage of socket `i` (0 for single-stage devices).
    fn stage_of(&self, i: usize) -> u8 {
        self.stages.get(i).copied().unwrap_or(0)
    }

    /// For stage `stage`: zeroes the stage's signal scratch (outputs arrive zeroed, unconnected
    /// inputs read zeros), clears its note outputs, and copies its cabled inputs in. Inputs of
    /// earlier stages keep what their own stage gathered. Summing inputs receive the left-fold
    /// sum of their cables in source order, each through its PDC delay line if it has one.
    /// Returns the number of note events dropped for lack of capacity.
    pub(crate) fn gather(
        &mut self,
        routes: &Routes,
        stage: u8,
        slots: &[Batch],
        note_slots: &[NoteBuffer],
        spare: &mut NoteBuffer,
        delays: &mut [DelayLine],
    ) -> u64 {
        for (i, s) in self.sockets.iter().enumerate() {
            if self.stage_of(i) != stage {
                continue;
            }
            self.signals[i] = [0.0; BATCH];
            if s.direction == Direction::Output {
                self.notes[i].clear();
            }
        }
        for &(socket, slot) in &routes.signal_in {
            self.signals[socket] = slots[slot];
        }
        for r in &routes.sum_in {
            let mut delayed = [0.0; BATCH];
            let src = match r.delay.and_then(|d| delays.get_mut(d)) {
                Some(line) => {
                    line.process(&slots[r.slot], &mut delayed);
                    &delayed
                }
                None => &slots[r.slot],
            };
            for (acc, x) in self.signals[r.socket].iter_mut().zip(src) {
                *acc += *x;
            }
        }
        let mut overflow = 0;
        for &(socket, slot) in &routes.notes_in {
            overflow += merge_notes(&mut self.notes[socket], &note_slots[slot], spare);
        }
        overflow
    }

    /// Calls `Device::process_stage(stage)` (which is `process` for single-stage devices),
    /// then, after the `last` stage, consumes the note inputs (the next batch starts empty).
    /// Touches only the node itself, so it may run on any thread that owns the node.
    pub(crate) fn run(&mut self, time: TimeInfo, stage: u8, last: bool) {
        let playback: &[Batch] = if self.playback_on {
            &self.playback
        } else {
            &[]
        };
        let mut ctx =
            ProcessContext::new(time, &mut self.signals, &mut self.notes, &self.connected)
                .with_playback(playback);
        self.device.process_stage(stage, &mut ctx);
        if last {
            self.playback_on = false;
            for (i, s) in self.sockets.iter().enumerate() {
                if s.direction.is_input() && is_notes(s.kind) {
                    self.notes[i].clear();
                }
            }
        }
    }

    /// Copies cabled outputs to their slots and publishes metered outputs.
    pub(crate) fn scatter(
        &self,
        routes: &Routes,
        slots: &mut [Batch],
        note_slots: &mut [NoteBuffer],
        meters: &Meters,
    ) {
        for &(socket, slot) in &routes.signal_out {
            slots[slot] = self.signals[socket];
        }
        for &(socket, meter) in &routes.meters {
            meters.publish(meter, &self.signals[socket]);
        }
        for &(socket, slot) in &routes.notes_out {
            let dst = &mut note_slots[slot];
            dst.clear();
            for event in self.notes[socket].as_slice() {
                // Equal capacities: a device's output always fits its slot.
                let _ = dst.push(*event);
            }
        }
    }
}

/// Real-time side of a routing. Nothing here allocates, frees, locks or blocks.
impl RoutingPlan {
    /// Marks this routing's nodes as routed and copies their `connected` flags. Called when
    /// the routing becomes current; touches no device state.
    pub(crate) fn install(&self, arena: &mut Arena) {
        for d in &self.devices {
            if let Some(node) = arena.node_mut(d.slot) {
                node.routed = true;
                if node.connected.len() == d.connected.len() {
                    node.connected.copy_from_slice(&d.connected);
                }
            }
        }
    }

    /// Marks this routing's nodes as no longer routed (they keep their state).
    pub(crate) fn uninstall(&self, arena: &mut Arena) {
        for d in &self.devices {
            if let Some(node) = arena.node_mut(d.slot) {
                node.routed = false;
            }
        }
    }

    /// Takes over the PDC delay-line state of `old` (the routing this one replaces) for every
    /// cable whose source, summing input and delay are unchanged. O(lines), no allocation:
    /// buffers are swapped, and `old` (now holding the fresh buffers) is retired.
    pub(crate) fn adopt_delays(&mut self, old: &mut RoutingPlan) {
        adopt_delay_lines(&mut self.console.delays, &mut old.console.delays);
    }

    /// Processes one batch: host inputs -> slots, every routed device in order, slots -> host
    /// outputs. Single-threaded (`threads = 0`). Arena devices outside the routing are skipped.
    pub(crate) fn process(
        &mut self,
        time: TimeInfo,
        host_in: &[Batch],
        host_out: &mut [Batch],
        dropped: &AtomicU64,
        arena: &mut Arena,
        meters: &Meters,
    ) {
        self.read_host_inputs(host_in, meters);
        self.run_sequential(0, self.steps.len(), time, dropped, arena, meters);
        self.write_host_outputs(host_out);
    }

    /// Gathers, runs and scatters steps `a..b` one after the other on this thread.
    pub(crate) fn run_sequential(
        &mut self,
        a: usize,
        b: usize,
        time: TimeInfo,
        dropped: &AtomicU64,
        arena: &mut Arena,
        meters: &Meters,
    ) {
        let Self {
            devices,
            steps,
            slots,
            note_slots,
            spare_notes,
            console,
            ..
        } = self;
        let delays = &mut console.delays;
        for step in &steps[a..b] {
            let Some(node) = arena.node_mut(devices[step.device].slot) else {
                continue;
            };
            let r = &step.routes;
            let overflow = node.gather(r, step.stage, slots, note_slots, spare_notes, delays);
            count_overflow(dropped, overflow);
            node.run(time, step.stage, step.last);
            node.scatter(r, slots, note_slots, meters);
        }
    }

    /// Host inputs -> their slots (before the first device); publishes metered host inputs.
    pub(crate) fn read_host_inputs(&mut self, host_in: &[Batch], meters: &Meters) {
        for &(channel, slot) in &self.host_in {
            self.slots[slot] = host_in.get(channel).copied().unwrap_or([0.0; BATCH]);
        }
        for &(channel, meter) in &self.console.host_meters {
            meters.publish(meter, host_in.get(channel).unwrap_or(&[0.0; BATCH]));
        }
    }

    /// Slots -> host outputs (after the last device); unrouted outputs are silent.
    pub(crate) fn write_host_outputs(&self, host_out: &mut [Batch]) {
        host_out.iter_mut().for_each(|b| *b = [0.0; BATCH]);
        for &(channel, slot) in &self.host_out {
            if let Some(out) = host_out.get_mut(channel) {
                *out = self.slots[slot];
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn assert_no_output_aliases_input(&self) {
        for step in &self.steps {
            let (r, id) = (&step.routes, self.devices[step.device].id);
            for (_, out) in &r.signal_out {
                assert!(
                    !r.signal_in.iter().any(|(_, s)| s == out),
                    "{id:?}: output slot {out} aliases an input"
                );
            }
            for (_, out) in &r.notes_out {
                assert!(
                    !r.notes_in.iter().any(|(_, s)| s == out),
                    "{id:?}: notes output slot {out} aliases an input"
                );
            }
        }
    }
}

/// Adds `overflow` dropped note events to the engine counter.
pub(crate) fn count_overflow(dropped: &AtomicU64, overflow: u64) {
    if overflow > 0 {
        dropped.fetch_add(overflow, Ordering::Relaxed);
    }
}

/// Merges cable notes into a Notes input that may already hold scheduled notes, keeping
/// offsets non-decreasing (scheduled first on ties). Uses `spare` as the merge target and swaps
/// it in, so nothing allocates. Returns the number of events dropped for lack of capacity.
fn merge_notes(scheduled: &mut NoteBuffer, cable: &NoteBuffer, spare: &mut NoteBuffer) -> u64 {
    let mut overflow = 0;
    if cable.is_empty() {
        return 0;
    }
    if scheduled.is_empty() {
        for event in cable.as_slice() {
            overflow += u64::from(scheduled.push(*event).is_err());
        }
        return overflow;
    }
    spare.clear();
    let (mut a, mut b) = (
        scheduled.as_slice().iter().peekable(),
        cable.as_slice().iter().peekable(),
    );
    loop {
        let next = match (a.peek(), b.peek()) {
            (Some(x), Some(y)) if y.offset < x.offset => b.next(),
            (Some(_), _) => a.next(),
            (None, Some(_)) => b.next(),
            (None, None) => break,
        };
        if let Some(event) = next {
            overflow += u64::from(spare.push(*event).is_err());
        }
    }
    std::mem::swap(scheduled, spare);
    overflow
}

/// Lowest-free-first slot pool (deterministic).
#[derive(Default)]
struct Pool {
    free: BTreeSet<usize>,
    count: usize,
}

impl Pool {
    fn take(&mut self) -> usize {
        self.free.pop_first().unwrap_or_else(|| {
            self.count += 1;
            self.count - 1
        })
    }
}

fn is_notes(kind: SocketKind) -> bool {
    kind == SocketKind::Notes
}

/// Liveness position of the interface sink: after every device.
const SINK: usize = usize::MAX;

/// Slot assignment state shared by the interface sources and every device.
struct Slots {
    last_use: BTreeMap<Endpoint, usize>,
    slot_of: BTreeMap<Endpoint, usize>,
    release: BTreeMap<usize, Vec<(bool, usize)>>,
    signal: Pool,
    notes: Pool,
}

impl Slots {
    /// Allocates a slot for every cabled output of stage `stage` of `producer`; schedules its
    /// release.
    fn assign(&mut self, producer: DeviceInstanceId, descriptor: &DeviceDescriptor, stage: u8) {
        for s in descriptor
            .sockets
            .iter()
            .filter(|s| s.direction == Direction::Output && descriptor.stage_of(s.id) == stage)
        {
            let e = Endpoint {
                device: producer,
                socket: s.id,
            };
            if let Some(&last) = self.last_use.get(&e) {
                let notes = is_notes(s.kind);
                let slot = if notes {
                    self.notes.take()
                } else {
                    self.signal.take()
                };
                self.slot_of.insert(e, slot);
                self.release.entry(last).or_default().push((notes, slot));
            }
        }
    }

    /// Frees the slots whose last consumer runs at `position`.
    fn release(&mut self, position: usize) {
        for (notes, slot) in self.release.remove(&position).unwrap_or_default() {
            let pool = if notes {
                &mut self.notes
            } else {
                &mut self.signal
            };
            pool.free.insert(slot);
        }
    }
}

/// Compile inputs besides the devices and cables.
pub(crate) struct CompileOptions<'a> {
    /// Delay compensation requested.
    pub(crate) compensation: bool,
    /// Session sample rate (for the compensation limit).
    pub(crate) sample_rate: f64,
    /// `Device::latency_frames()` of each device (never asked for the interface).
    pub(crate) latency: &'a dyn Fn(DeviceInstanceId) -> u32,
    /// Metered output endpoints (validated by the caller).
    pub(crate) meters: &'a BTreeSet<Endpoint>,
}

/// PDC outcome of a compile: the report, plus the non-zero delay of each compensated cable,
/// keyed like the cable map (input, source).
struct Compensation {
    report: PdcReport,
    delays: BTreeMap<(Endpoint, Endpoint), u32>,
}

/// Delay compensation over `order` (a topological order of (device, stage) nodes): see the
/// `console` module docs. A stage's latency is the max of its own inputs' source latencies and
/// of its previous stage; the device's `latency_frames()` is added at its last stage.
fn compensate(
    order: &[StageNode],
    lookup: Lookup<'_>,
    cables: &CableMap,
    options: &CompileOptions<'_>,
) -> Result<Compensation, GraphError> {
    let mut node_latency: BTreeMap<StageNode, u64> = BTreeMap::new();
    // The interface (host inputs) has latency 0.
    let source_latency = |known: &BTreeMap<StageNode, u64>, from: Endpoint| {
        known
            .get(&(from.device, stage_of(lookup, from)))
            .copied()
            .unwrap_or(0)
    };
    // (cable key, needed delay) for every cable into an Audio/Cv summing input.
    let mut needed: Vec<((Endpoint, Endpoint), u64)> = Vec::new();
    let mut feeds: BTreeMap<Endpoint, u64> = BTreeMap::new();
    for &(id, stage) in order {
        let descriptor = lookup(id).ok_or(GraphError::UnknownDevice)?;
        let mut input_latency = match stage {
            0 => 0,
            s => node_latency.get(&(id, s - 1)).copied().unwrap_or(0),
        };
        for s in descriptor
            .sockets
            .iter()
            .filter(|s| s.direction.is_input() && descriptor.stage_of(s.id) == stage)
        {
            let to = Endpoint {
                device: id,
                socket: s.id,
            };
            let max = rules::into(cables, to)
                .map(|c| source_latency(&node_latency, c.from))
                .max()
                .unwrap_or(0);
            input_latency = input_latency.max(max);
            if s.direction != Direction::SummingInput {
                continue;
            }
            for c in rules::into(cables, to) {
                let l = source_latency(&node_latency, c.from);
                feeds.insert(c.from, l);
                if !is_notes(s.kind) {
                    needed.push(((c.to, c.from), max - l));
                }
            }
        }
        let own = if stage + 1 == descriptor.stage_count() {
            u64::from((options.latency)(id))
        } else {
            0
        };
        node_latency.insert((id, stage), input_latency.saturating_add(own));
    }
    needed.sort_unstable();
    let limit = (MAX_COMPENSATION_SECONDS * options.sample_rate).floor() as u64;
    let limit_exceeded = options.compensation && needed.iter().any(|(_, d)| *d > limit);
    let active = options.compensation && !limit_exceeded;
    let applied = |d: u64| if active { d as u32 } else { 0 };
    let saturate = |l: u64| u32::try_from(l).unwrap_or(u32::MAX);
    Ok(Compensation {
        report: PdcReport {
            active,
            limit_exceeded,
            cable_delays: needed
                .iter()
                .map(|&((to, from), d)| (from, to, applied(d)))
                .collect(),
            latencies: feeds.into_iter().map(|(e, l)| (e, saturate(l))).collect(),
        },
        delays: needed
            .into_iter()
            .filter(|(_, d)| active && *d > 0)
            .map(|(key, d)| (key, d as u32))
            .collect(),
    })
}

/// Compiles a validated device set and cable map into a routing (shared by [`Graph::compile`]
/// and `Topology::compile`, so both give the same order, levels and slots).
///
/// Re-validates every cable (defence in depth), orders devices topologically (Kahn,
/// ties by ascending id), groups them by dependency level (level-major, then id), assigns
/// buffer slots by liveness and builds every route. Arena slots are set to the position.
/// Computes delay compensation, allocates the PDC delay lines (sized to their
/// delays) and assigns meter slots (the first [`METER_CAPACITY`] metered endpoints, in order).
pub(crate) fn compile_routing(
    interface: &'static DeviceDescriptor,
    input_channels: u16,
    ids: &[DeviceInstanceId],
    lookup: Lookup<'_>,
    cables: &CableMap,
    options: &CompileOptions<'_>,
) -> Result<RoutingPlan, GraphError> {
    // 1. Re-validate every cable.
    rules::revalidate(lookup, cables)?;
    // 2. Deterministic Kahn toposort over (device, stage) nodes (rejects cycles), then
    //    dependency levels. Single-stage devices: one node each, exactly the device order.
    let (nodes, level_starts) =
        level_order(topological_order(ids, lookup, cables)?, lookup, cables);
    let position: BTreeMap<StageNode, usize> =
        nodes.iter().enumerate().map(|(p, n)| (*n, p)).collect();
    // 3. Liveness: position of the last consuming step of every cabled output.
    let mut slots = Slots {
        last_use: BTreeMap::new(),
        slot_of: BTreeMap::new(),
        release: BTreeMap::new(),
        signal: Pool::default(),
        notes: Pool::default(),
    };
    for c in cables.values() {
        let p = position
            .get(&(c.to.device, stage_of(lookup, c.to)))
            .copied()
            .unwrap_or(SINK);
        let e = slots.last_use.entry(c.from).or_insert(p);
        *e = (*e).max(p);
    }
    // 4. Slot assignment: allocate a step's outputs first, then free the inputs that die at
    //    it, so a step's outputs never alias its own inputs.
    slots.assign(Graph::INTERFACE, interface, 0);
    for (p, &(id, stage)) in nodes.iter().enumerate() {
        let descriptor = lookup(id).ok_or(GraphError::UnknownDevice)?;
        slots.assign(id, descriptor, stage);
        slots.release(p);
    }
    // 5. Delay compensation and its delay lines, in cable-map order.
    let Compensation { report, delays } = compensate(&nodes, lookup, cables, options)?;
    let delay_index: BTreeMap<(Endpoint, Endpoint), usize> = delays
        .keys()
        .enumerate()
        .map(|(k, key)| (*key, k))
        .collect();
    let delay_lines = delays
        .iter()
        .map(|(&(to, from), &d)| DelayLine::new(from, to, d))
        .collect();
    // 6. Meter slots: the first METER_CAPACITY metered endpoints, ascending.
    let metered: Vec<Endpoint> = options
        .meters
        .iter()
        .take(METER_CAPACITY)
        .copied()
        .collect();
    let meter_slot: BTreeMap<Endpoint, usize> =
        metered.iter().enumerate().map(|(k, e)| (*e, k)).collect();
    let tables = RouteTables {
        cables,
        slot_of: &slots.slot_of,
        delay_index: &delay_index,
        meter_slot: &meter_slot,
    };
    // 7. Devices (in order of their first stage) and per-stage routes, in render order.
    let mut devices = Vec::new();
    let mut stage_routes: Vec<Vec<Option<Routes>>> = Vec::new();
    let mut index: BTreeMap<DeviceInstanceId, usize> = BTreeMap::new();
    for &(id, stage) in &nodes {
        if stage != 0 {
            continue;
        }
        let descriptor = lookup(id).ok_or(GraphError::UnknownDevice)?;
        let (routes, connected) = build_routes(id, descriptor, &tables);
        index.insert(id, devices.len());
        devices.push(Routed {
            id,
            slot: devices.len(),
            descriptor,
            connected,
        });
        stage_routes.push(routes.into_iter().map(Some).collect());
    }
    let mut steps = Vec::with_capacity(nodes.len());
    for &(id, stage) in &nodes {
        let k = index[&id];
        let routes = stage_routes[k][usize::from(stage)]
            .take()
            .expect("one step per (device, stage)");
        steps.push(Step {
            device: k,
            stage,
            last: usize::from(stage) + 1 == stage_routes[k].len(),
            routes,
        });
    }
    let order = nodes.iter().map(|(id, _)| *id).collect();
    // 8. Interface routes.
    let inputs = usize::from(input_channels);
    let mut host_in = Vec::new();
    let mut host_out = Vec::new();
    let mut host_meters = Vec::new();
    for s in interface.sockets {
        let e = Endpoint {
            device: Graph::INTERFACE,
            socket: s.id,
        };
        let i = usize::from(s.id.0);
        match s.direction {
            Direction::Output => {
                if let Some(slot) = slots.slot_of.get(&e) {
                    host_in.push((i, *slot));
                }
                if let Some(m) = meter_slot.get(&e) {
                    host_meters.push((i, *m));
                }
            }
            Direction::Input | Direction::SummingInput => {
                if let Some(c) = rules::into(cables, e).next() {
                    host_out.push((i - inputs, slots.slot_of[&c.from]));
                }
            }
        }
    }
    Ok(RoutingPlan {
        order,
        level_starts,
        devices,
        steps,
        slots: vec![[0.0; BATCH]; slots.signal.count],
        note_slots: (0..slots.notes.count)
            .map(|_| NoteBuffer::with_capacity(MAX_EVENTS_PER_BATCH))
            .collect(),
        host_in,
        host_out,
        spare_notes: NoteBuffer::with_capacity(MAX_EVENTS_PER_BATCH),
        console: Box::new(ConsoleRouting {
            delays: delay_lines,
            pdc: report,
            meter_keys: metered.into_iter().map(meter_key).collect(),
            host_meters,
        }),
    })
}

impl Graph {
    /// Re-validates the graph, orders devices topologically (ties by ascending id),
    /// calls `prepare` once per device, re-applies stored params, assigns buffer slots by
    /// liveness and preallocates every scratch buffer.
    ///
    /// Each device's `latency_frames()` is read after `prepare` and its params, for
    /// delay compensation (unless [`Graph::set_compensation`] turned it off).
    pub fn compile(self) -> Result<RenderPlan, GraphError> {
        let ids: Vec<DeviceInstanceId> = self.devices.keys().copied().collect();
        // Validate and order first, so devices are prepared only for a valid graph.
        let lookup = |id| self.descriptor(id);
        rules::revalidate(&lookup, &self.cables)?;
        let topo = topological_order(&ids, &lookup, &self.cables)?;
        let (nodes, _) = level_order(topo, &lookup, &self.cables);
        // Devices in the order of their first stage.
        let order: Vec<DeviceInstanceId> = nodes
            .iter()
            .filter(|(_, stage)| *stage == 0)
            .map(|(id, _)| *id)
            .collect();
        let Graph {
            config,
            interface,
            mut devices,
            cables,
            compensation,
            ..
        } = self;
        // prepare once per device (in render order), then re-apply stored params.
        let mut prepared: BTreeMap<DeviceInstanceId, Box<dyn Device>> = BTreeMap::new();
        for id in &order {
            let entry = devices.remove(id).expect("ordered ids come from the graph");
            let mut device = entry.device;
            device.prepare(&PrepareInfo {
                sample_rate: config.sample_rate,
                seed: mix_seed(config.seed, *id),
            });
            for (param, value) in &entry.params {
                device.set_param(*param, *value);
            }
            prepared.insert(*id, device);
        }
        let routing = {
            let lookup = |id: DeviceInstanceId| {
                if id == Graph::INTERFACE {
                    Some(interface)
                } else {
                    prepared.get(&id).map(|d| d.descriptor())
                }
            };
            let latency =
                |id: DeviceInstanceId| prepared.get(&id).map_or(0, |d| d.latency_frames());
            compile_routing(
                interface,
                config.input_channels,
                &ids,
                &lookup,
                &cables,
                &CompileOptions {
                    compensation,
                    sample_rate: config.sample_rate.hz(),
                    latency: &latency,
                    meters: &BTreeSet::new(),
                },
            )?
        };
        let nodes = routing
            .devices
            .iter()
            .map(|d| {
                Node::new(
                    prepared
                        .remove(&d.id)
                        .expect("ordered ids come from the graph"),
                )
            })
            .collect();
        Ok(RenderPlan { routing, nodes })
    }
}

/// A (device, stage) node of the render graph.
pub(crate) type StageNode = (DeviceInstanceId, u8);

/// Stage of an endpoint's socket (0 for the interface and single-stage devices).
fn stage_of(lookup: Lookup<'_>, e: Endpoint) -> u8 {
    lookup(e.device).map_or(0, |d| d.stage_of(e.socket))
}

/// The (device, stage) edge of a device-to-device cable, if it is one.
fn stage_edge(lookup: Lookup<'_>, c: &crate::graph::Cable) -> Option<(StageNode, StageNode)> {
    if c.from.device == Graph::INTERFACE || c.to.device == Graph::INTERFACE {
        return None;
    }
    Some((
        (c.from.device, stage_of(lookup, c.from)),
        (c.to.device, stage_of(lookup, c.to)),
    ))
}

/// Dependency levels over `topo` (a topological order of stage nodes): 1 + the highest level
/// of the node's cable producers and of its own previous stage, else 0. Returns the nodes
/// sorted by (level, id, stage) and the level boundaries (`[0, .., len]`). For single-stage
/// devices this is the (level, id) order.
fn level_order(
    topo: Vec<StageNode>,
    lookup: Lookup<'_>,
    cables: &CableMap,
) -> (Vec<StageNode>, Vec<usize>) {
    let mut producers: BTreeMap<StageNode, Vec<StageNode>> = BTreeMap::new();
    for c in cables.values() {
        if let Some((from, to)) = stage_edge(lookup, c) {
            producers.entry(to).or_default().push(from);
        }
    }
    let mut level: BTreeMap<StageNode, usize> = BTreeMap::new();
    for node in &topo {
        let previous = (node.1 > 0).then(|| (node.0, node.1 - 1));
        let l = producers
            .get(node)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .chain(previous.as_ref())
            .map(|p| level[p] + 1)
            .max()
            .unwrap_or(0);
        level.insert(*node, l);
    }
    let mut order = topo;
    order.sort_by_key(|n| (level[n], *n));
    let mut starts = vec![0];
    for k in 1..order.len() {
        if level[&order[k]] != level[&order[k - 1]] {
            starts.push(k);
        }
    }
    if !order.is_empty() {
        starts.push(order.len());
    }
    (order, starts)
}

/// Kahn's algorithm over (device, stage) nodes: device-to-device cables plus the implicit
/// stage `s` -> `s + 1` edges, with the ready set ordered by (id, stage).
fn topological_order(
    ids: &[DeviceInstanceId],
    lookup: Lookup<'_>,
    cables: &CableMap,
) -> Result<Vec<StageNode>, GraphError> {
    let mut indegree: BTreeMap<StageNode, usize> = BTreeMap::new();
    let mut successors: BTreeMap<StageNode, Vec<StageNode>> = BTreeMap::new();
    for id in ids {
        let count = lookup(*id).ok_or(GraphError::UnknownDevice)?.stage_count();
        for stage in 0..count {
            indegree.insert((*id, stage), usize::from(stage > 0));
            if stage + 1 < count {
                successors
                    .entry((*id, stage))
                    .or_default()
                    .push((*id, stage + 1));
            }
        }
    }
    for c in cables.values() {
        let Some((from, to)) = stage_edge(lookup, c) else {
            continue;
        };
        *indegree.get_mut(&to).ok_or(GraphError::UnknownDevice)? += 1;
        successors.entry(from).or_default().push(to);
    }
    let mut ready: BTreeSet<StageNode> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(n, _)| *n)
        .collect();
    let mut order = Vec::with_capacity(indegree.len());
    while let Some(n) = ready.pop_first() {
        order.push(n);
        for t in successors.get(&n).map(Vec::as_slice).unwrap_or_default() {
            let d = indegree.get_mut(t).ok_or(GraphError::UnknownDevice)?;
            *d -= 1;
            if *d == 0 {
                ready.insert(*t);
            }
        }
    }
    if order.len() == indegree.len() {
        Ok(order)
    } else {
        Err(GraphError::WouldCreateCycle)
    }
}

/// The compile tables a device's routes are built from.
struct RouteTables<'a> {
    cables: &'a CableMap,
    slot_of: &'a BTreeMap<Endpoint, usize>,
    /// PDC delay line per (input, source) cable.
    delay_index: &'a BTreeMap<(Endpoint, Endpoint), usize>,
    meter_slot: &'a BTreeMap<Endpoint, usize>,
}

/// Builds a device's routes, one [`Routes`] per stage, and its `connected` flags.
fn build_routes(
    id: DeviceInstanceId,
    descriptor: &'static DeviceDescriptor,
    t: &RouteTables<'_>,
) -> (Vec<Routes>, Vec<bool>) {
    let empty = || Routes {
        signal_in: Vec::new(),
        signal_out: Vec::new(),
        notes_in: Vec::new(),
        notes_out: Vec::new(),
        sum_in: Vec::new(),
        meters: Vec::new(),
    };
    let sockets = descriptor.sockets;
    let mut stages: Vec<Routes> = (0..descriptor.stage_count()).map(|_| empty()).collect();
    let mut connected = vec![false; sockets.len()];
    for s in sockets {
        let e = Endpoint {
            device: id,
            socket: s.id,
        };
        let i = usize::from(s.id.0);
        let routes = &mut stages[usize::from(descriptor.stage_of(s.id))];
        let notes = is_notes(s.kind);
        let mut cabled = false;
        match s.direction {
            Direction::Output => {
                if let Some(slot) = t.slot_of.get(&e) {
                    cabled = true;
                    if notes {
                        routes.notes_out.push((i, *slot));
                    } else {
                        routes.signal_out.push((i, *slot));
                    }
                }
                if let Some(m) = t.meter_slot.get(&e).filter(|_| !notes) {
                    routes.meters.push((i, *m));
                }
            }
            Direction::Input => {
                if let Some(c) = rules::into(t.cables, e).next() {
                    cabled = true;
                    let route = (i, t.slot_of[&c.from]);
                    if notes {
                        routes.notes_in.push(route);
                    } else {
                        routes.signal_in.push(route);
                    }
                }
            }
            Direction::SummingInput => {
                // Source order: the cable map iterates (input, source) ascending.
                for c in rules::into(t.cables, e) {
                    cabled = true;
                    let slot = t.slot_of[&c.from];
                    if notes {
                        routes.notes_in.push((i, slot));
                    } else {
                        routes.sum_in.push(SumRoute {
                            socket: i,
                            slot,
                            delay: t.delay_index.get(&(e, c.from)).copied(),
                        });
                    }
                }
            }
        }
        // Notes inputs always count as connected: the engine's note scheduler can feed any of
        // them, like a virtual cable (matches `DeviceHarness::push_note`).
        connected[i] = cabled || (s.direction.is_input() && notes);
    }
    (stages, connected)
}

#[cfg(test)]
mod tests {
    use crate::graph::tests::{cfg, ep};
    use crate::graph::{Cable, DeviceInstanceId, Graph, GraphError};
    use atomik_device_sdk::SocketId;
    use atomik_testkit::fakes::{CallLog, ConstantSource, ImpulseOnNote, Passthrough};

    fn out1() -> crate::graph::Endpoint {
        ep(Graph::INTERFACE, SocketId(2))
    }

    #[test]
    fn compile_orders_topologically() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(Passthrough::boxed(1.0)); // 1: last
        let b = g.add_device(Passthrough::boxed(1.0)); // 2: middle
        let c = g.add_device(ConstantSource::boxed(1.0)); // 3: source
        g.connect(
            ep(c, ConstantSource::AUDIO_OUT),
            ep(b, Passthrough::AUDIO_IN),
        )
        .unwrap();
        g.connect(ep(b, Passthrough::AUDIO_OUT), ep(a, Passthrough::AUDIO_IN))
            .unwrap();
        g.connect(ep(a, Passthrough::AUDIO_OUT), out1()).unwrap();
        let plan = g.compile().unwrap();
        assert_eq!(plan.order(), &[c, b, a]);
    }

    #[test]
    fn compile_tie_breaks_by_id() {
        let mut g = Graph::new(&cfg());
        let p4 = DeviceInstanceId(4);
        let ids: Vec<_> = (0..5)
            .map(|_| g.add_device(Passthrough::boxed(1.0)))
            .collect();
        // 5 -> 4; everything else independent: ready set {1,2,3,5} then 4 once 5 ran.
        g.connect(
            ep(ids[4], Passthrough::AUDIO_OUT),
            ep(p4, Passthrough::AUDIO_IN),
        )
        .unwrap();
        let plan = g.compile().unwrap();
        let order: Vec<u32> = plan.order().iter().map(|d| d.0).collect();
        assert_eq!(order, vec![1, 2, 3, 5, 4]);
    }

    #[test]
    fn compile_assigns_dependency_levels_and_orders_level_major() {
        // c1 -> p2 -> p3;  c1 -> p4;  p5 alone (interface-fed);  levels: {1,5} {2,4} {3}.
        let mut g = Graph::new(&cfg());
        let c1 = g.add_device(ConstantSource::boxed(1.0));
        let p: Vec<_> = (0..4)
            .map(|_| g.add_device(Passthrough::boxed(1.0)))
            .collect();
        let (p2, p3, p4, p5) = (p[0], p[1], p[2], p[3]);
        let a = Passthrough::AUDIO_IN;
        g.connect(ep(c1, ConstantSource::AUDIO_OUT), ep(p2, a))
            .unwrap();
        g.connect(ep(p2, Passthrough::AUDIO_OUT), ep(p3, a))
            .unwrap();
        g.connect(ep(c1, ConstantSource::AUDIO_OUT), ep(p4, a))
            .unwrap();
        g.connect(ep(Graph::INTERFACE, SocketId(0)), ep(p5, a))
            .unwrap();
        g.connect(ep(p3, Passthrough::AUDIO_OUT), out1()).unwrap();
        let plan = g.compile().unwrap();
        let levels: Vec<Vec<DeviceInstanceId>> = plan.levels().map(<[_]>::to_vec).collect();
        assert_eq!(levels, vec![vec![c1, p5], vec![p2, p4], vec![p3]]);
        assert_eq!(plan.order(), &[c1, p5, p2, p4, p3]);
        plan.assert_no_output_aliases_input();
    }

    #[test]
    fn compile_levels_are_deterministic() {
        let build = || {
            let mut g = Graph::new(&cfg());
            let ids: Vec<_> = (0..12)
                .map(|_| g.add_device(Passthrough::boxed(1.0)))
                .collect();
            for k in 1..12 {
                if k % 3 != 0 {
                    g.connect(
                        ep(ids[(k * 5) % k], Passthrough::AUDIO_OUT),
                        ep(ids[k], Passthrough::AUDIO_IN),
                    )
                    .unwrap();
                }
            }
            g.compile().unwrap()
        };
        let a: Vec<Vec<_>> = build().levels().map(<[_]>::to_vec).collect();
        let b: Vec<Vec<_>> = build().levels().map(<[_]>::to_vec).collect();
        assert_eq!(a, b);
        assert_eq!(a.iter().map(Vec::len).sum::<usize>(), 12);
    }

    #[test]
    fn compile_calls_prepare_once_per_device() {
        let mut g = Graph::new(&cfg());
        let (d1, c1) = CallLog::boxed();
        let (d2, c2) = CallLog::boxed();
        let a = g.add_device(d1);
        let b = g.add_device(d2);
        g.set_param(b, CallLog::P, 0.25).unwrap();
        g.connect(ep(a, CallLog::AUDIO_OUT), ep(b, CallLog::AUDIO_IN))
            .unwrap();
        let _plan = g.compile().unwrap();
        assert_eq!((c1.prepare(), c2.prepare()), (1, 1));
        assert_eq!((c1.process(), c2.process()), (0, 0));
        assert_ne!(c1.last_seed(), c2.last_seed(), "seeds mix in the id");
        assert_eq!(c1.last_seed(), super::mix_seed(7, a));
        assert_eq!(c1.set_param(), 0);
        assert_eq!(
            c2.set_param(),
            1,
            "stored params are re-applied after prepare"
        );
        assert!((c2.last_param() - 0.25).abs() < 1e-7);
    }

    #[test]
    fn compile_reuses_slots_when_dead() {
        // const -> p1 -> p2 -> p3 -> out_1: a chain needs only two live buffers.
        let mut g = Graph::new(&cfg());
        let c = g.add_device(ConstantSource::boxed(1.0));
        let p: Vec<_> = (0..3)
            .map(|_| g.add_device(Passthrough::boxed(1.0)))
            .collect();
        g.connect(
            ep(c, ConstantSource::AUDIO_OUT),
            ep(p[0], Passthrough::AUDIO_IN),
        )
        .unwrap();
        for w in p.windows(2) {
            g.connect(
                ep(w[0], Passthrough::AUDIO_OUT),
                ep(w[1], Passthrough::AUDIO_IN),
            )
            .unwrap();
        }
        g.connect(ep(p[2], Passthrough::AUDIO_OUT), out1()).unwrap();
        let plan = g.compile().unwrap();
        assert_eq!(plan.slot_count(), 2);
        plan.assert_no_output_aliases_input();
    }

    #[test]
    fn compile_fan_out_keeps_slot_alive_until_last_consumer() {
        // const -> {p1, p2}; p1 -> out_1, p2 -> out_2: const's slot lives until p2 ran.
        let mut g = Graph::new(&cfg());
        let c = g.add_device(ConstantSource::boxed(1.0));
        let p1 = g.add_device(Passthrough::boxed(1.0));
        let p2 = g.add_device(Passthrough::boxed(1.0));
        for p in [p1, p2] {
            g.connect(
                ep(c, ConstantSource::AUDIO_OUT),
                ep(p, Passthrough::AUDIO_IN),
            )
            .unwrap();
        }
        g.connect(ep(p1, Passthrough::AUDIO_OUT), out1()).unwrap();
        g.connect(
            ep(p2, Passthrough::AUDIO_OUT),
            ep(Graph::INTERFACE, SocketId(3)),
        )
        .unwrap();
        let plan = g.compile().unwrap();
        assert_eq!(plan.slot_count(), 3);
        plan.assert_no_output_aliases_input();
    }

    #[test]
    fn compile_uses_separate_note_slots() {
        let mut g = Graph::new(&cfg());
        let (log, _) = CallLog::boxed();
        let l = g.add_device(log);
        let i = g.add_device(ImpulseOnNote::boxed());
        g.connect(ep(i, ImpulseOnNote::AUDIO_OUT), ep(l, CallLog::AUDIO_IN))
            .unwrap();
        let plan = g.compile().unwrap();
        assert_eq!(plan.order(), &[i, l]);
        assert_eq!(plan.slot_count(), 1);
    }

    #[test]
    fn compile_revalidates_corrupted_graph() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(Passthrough::boxed(1.0));
        let b = g.add_device(Passthrough::boxed(1.0));
        g.connect(ep(a, Passthrough::AUDIO_OUT), ep(b, Passthrough::AUDIO_IN))
            .unwrap();
        // Bypass the editing API to plant a cycle.
        let back = Cable {
            from: ep(b, Passthrough::AUDIO_OUT),
            to: ep(a, Passthrough::AUDIO_IN),
        };
        g.cables.insert((back.to, back.from), back);
        assert!(matches!(g.compile(), Err(GraphError::WouldCreateCycle)));

        let mut g = Graph::new(&cfg());
        let c = g.add_device(ConstantSource::boxed(1.0));
        let p = g.add_device(Passthrough::boxed(1.0));
        let bad = Cable {
            from: ep(c, ConstantSource::CV_OUT),
            to: ep(p, Passthrough::AUDIO_IN),
        };
        g.cables.insert((bad.to, bad.from), bad);
        assert!(matches!(g.compile(), Err(GraphError::KindMismatch)));
    }

    #[test]
    fn compile_empty_graph_has_no_devices() {
        let plan = Graph::new(&cfg()).compile().unwrap();
        assert!(plan.order().is_empty());
        assert_eq!(plan.slot_count(), 0);
        assert!(format!("{plan:?}").contains("RenderPlan"));
    }
}
