//! Hot editing: a persistent device arena plus routing-only
//! plans, so structural edits never reset device state.
//!
//! - [`Topology`] is the non-RT routing model: device ids with their descriptors, plus
//!   cables. It is validated exactly like [`crate::Graph`].
//! - [`Topology::compile`] produces a [`RoutingPlan`]: order, levels, slots and cable copies.
//!   It owns no devices.
//! - [`crate::EngineHandle::insert_device`] prepares a device off the RT thread and queues it
//!   into the engine's arena. [`crate::EngineHandle::remove_device`] queues its removal, and the
//!   engine hands the instance back for dropping on the non-RT side.
//! - Insert, routing and remove messages travel through ONE ordered queue, so a sequence such
//!   as "insert A, routing that uses A" or "routing without B, remove B" is applied in order at
//!   batch boundaries.
//!
//! Public signatures are a stable contract: changes need review. The `Graph` / `RenderPlan` / `submit_plan`
//! API keeps working, implemented on top of this if convenient.

use crate::console::{DelayLine, Latencies, MAX_METER_VALUES, Meters, PdcReport};
use crate::graph::{Cable, DeviceInstanceId, Endpoint, EngineConfig, Graph, GraphError, rules};
use crate::plan::{CompileOptions, Node, Route, Routed, Step};
use atomik_core::{Batch, NoteBuffer, NoteKind, ParamId};
use atomik_device_sdk::{
    DeviceDescriptor, Direction, PlayerDataRef, SampleMapRef, SocketId, SocketKind,
};
use std::collections::{BTreeMap, BTreeSet};

/// Non-RT routing model: device ids with descriptors, plus cables. Contains no device
/// instances. Includes the AudioInterface as `Graph::INTERFACE`.
#[derive(Clone)]
pub struct Topology {
    config: EngineConfig,
    interface: &'static DeviceDescriptor,
    nodes: BTreeMap<DeviceInstanceId, &'static DeviceDescriptor>,
    cables: rules::CableMap,
    /// Delay compensation for compile (on by default).
    compensation: bool,
    /// `Device::latency_frames()` per node (0 unless set).
    latencies: BTreeMap<DeviceInstanceId, u32>,
    /// Metered output endpoints.
    meters: BTreeSet<Endpoint>,
}

impl std::fmt::Debug for Topology {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Topology")
            .field("nodes", &self.nodes.keys().collect::<Vec<_>>())
            .field("cables", &self.cables.values().collect::<Vec<_>>())
            .finish()
    }
}

impl Topology {
    /// An empty topology containing only the AudioInterface for `config`.
    pub fn new(config: &EngineConfig) -> Self {
        Self {
            config: *config,
            interface: crate::interface::interface_descriptor(
                config.input_channels,
                config.output_channels,
            ),
            nodes: BTreeMap::new(),
            cables: BTreeMap::new(),
            compensation: true,
            latencies: BTreeMap::new(),
            meters: BTreeSet::new(),
        }
    }

    /// Adds a node with the given id (unique; not `Graph::INTERFACE`) and descriptor.
    ///
    /// # Errors
    /// [`GraphError::DuplicateDevice`] if the id is taken (the interface's included).
    pub fn add_node(
        &mut self,
        id: DeviceInstanceId,
        descriptor: &'static DeviceDescriptor,
    ) -> Result<(), GraphError> {
        if id == Graph::INTERFACE || self.nodes.contains_key(&id) {
            return Err(GraphError::DuplicateDevice);
        }
        self.nodes.insert(id, descriptor);
        Ok(())
    }

    /// Removes a node and every cable attached to it.
    ///
    /// # Errors
    /// [`GraphError::CannotRemoveInterface`] or [`GraphError::UnknownDevice`].
    pub fn remove_node(&mut self, id: DeviceInstanceId) -> Result<(), GraphError> {
        if id == Graph::INTERFACE {
            return Err(GraphError::CannotRemoveInterface);
        }
        self.nodes.remove(&id).ok_or(GraphError::UnknownDevice)?;
        rules::detach(&mut self.cables, id);
        self.latencies.remove(&id);
        self.meters.retain(|e| e.device != id);
        Ok(())
    }

    /// Connects `from` (output) to `to` (input), with the same rules as `Graph::connect`
    /// (a summing input accepts any number of distinct cables). On error the topology is
    /// unchanged.
    pub fn connect(&mut self, from: Endpoint, to: Endpoint) -> Result<(), GraphError> {
        let cable = Cable { from, to };
        rules::check_connect(&|id| self.descriptor(id), &self.cables, &cable)?;
        self.cables.insert((to, from), cable);
        Ok(())
    }

    /// Disconnects the cable plugged into input `to` (every cable, for a summing input).
    pub fn disconnect(&mut self, to: Endpoint) -> Result<(), GraphError> {
        rules::check_disconnect(&|id| self.descriptor(id), &self.cables, to)?;
        rules::remove_into(&mut self.cables, to);
        Ok(())
    }

    /// Disconnects the one cable `from -> to` (for summing inputs).
    ///
    /// # Errors
    /// [`GraphError::UnknownDevice`]/[`GraphError::UnknownSocket`] for a bad `to`, or
    /// [`GraphError::NotConnected`] if there is no such cable.
    pub fn disconnect_cable(&mut self, from: Endpoint, to: Endpoint) -> Result<(), GraphError> {
        rules::socket(&|id| self.descriptor(id), to)?;
        self.cables
            .remove(&(to, from))
            .map(|_| ())
            .ok_or(GraphError::NotConnected)
    }

    /// Compiles the routing (deterministic order and levels, slot assignment). Does not
    /// touch any device instance.
    ///
    /// Computes delay compensation from the latencies set with
    /// [`Topology::set_latency`] (0 by default; see [`crate::EngineHandle::fill_latencies`]),
    /// allocates the PDC delay lines and assigns the meter slots.
    pub fn compile(&self) -> Result<RoutingPlan, GraphError> {
        let ids: Vec<DeviceInstanceId> = self.nodes.keys().copied().collect();
        let latency = |id: DeviceInstanceId| self.latencies.get(&id).copied().unwrap_or(0);
        crate::plan::compile_routing(
            self.interface,
            self.config.input_channels,
            &ids,
            &|id| self.descriptor(id),
            &self.cables,
            &CompileOptions {
                compensation: self.compensation,
                sample_rate: self.config.sample_rate.hz(),
                latency: &latency,
                meters: &self.meters,
            },
        )
    }

    /// Enables or disables delay compensation for subsequent compiles (on by
    /// default). Off means every automatic delay is 0.
    pub fn set_compensation(&mut self, enabled: bool) {
        self.compensation = enabled;
    }

    /// Whether compiles compensate latency.
    pub fn compensation(&self) -> bool {
        self.compensation
    }

    /// Sets the latency (`Device::latency_frames()`) compile uses for node `id`.
    ///
    /// # Errors
    /// [`GraphError::UnknownDevice`] if `id` is not a node (the interface included).
    pub fn set_latency(&mut self, id: DeviceInstanceId, frames: u32) -> Result<(), GraphError> {
        if !self.nodes.contains_key(&id) {
            return Err(GraphError::UnknownDevice);
        }
        self.latencies.insert(id, frames);
        Ok(())
    }

    /// The latency compile uses for node `id` (0 unless set); `None` if `id` is not a node.
    pub fn latency(&self, id: DeviceInstanceId) -> Option<u32> {
        self.nodes
            .contains_key(&id)
            .then(|| self.latencies.get(&id).copied().unwrap_or(0))
    }

    /// Meters an output endpoint (peak/RMS published lock-free each batch). Idempotent. Any
    /// Audio/Cv output can be metered, cabled or not, the interface's host inputs included.
    /// Only the first [`crate::METER_CAPACITY`] metered endpoints (ascending) are published.
    ///
    /// # Errors
    /// [`GraphError::UnknownDevice`], [`GraphError::UnknownSocket`],
    /// [`GraphError::DirectionMismatch`] (not an output) or [`GraphError::KindMismatch`]
    /// (a Notes socket).
    pub fn add_meter(&mut self, endpoint: Endpoint) -> Result<(), GraphError> {
        let s = rules::socket(&|id| self.descriptor(id), endpoint)?;
        if s.direction != Direction::Output {
            return Err(GraphError::DirectionMismatch);
        }
        if s.kind == SocketKind::Notes {
            return Err(GraphError::KindMismatch);
        }
        self.meters.insert(endpoint);
        Ok(())
    }

    /// Stops metering an endpoint.
    ///
    /// # Errors
    /// [`GraphError::NotConnected`] if it was not metered.
    pub fn remove_meter(&mut self, endpoint: Endpoint) -> Result<(), GraphError> {
        if self.meters.remove(&endpoint) {
            Ok(())
        } else {
            Err(GraphError::NotConnected)
        }
    }

    /// Metered endpoints, ascending.
    pub fn meters(&self) -> impl Iterator<Item = Endpoint> + '_ {
        self.meters.iter().copied()
    }

    /// The configuration the topology was created with.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// The descriptor of a node (the AudioInterface's for `Graph::INTERFACE`).
    pub fn descriptor(&self, id: DeviceInstanceId) -> Option<&'static DeviceDescriptor> {
        if id == Graph::INTERFACE {
            Some(self.interface)
        } else {
            self.nodes.get(&id).copied()
        }
    }

    /// Node ids (the interface excluded), ascending.
    pub fn node_ids(&self) -> impl Iterator<Item = DeviceInstanceId> + '_ {
        self.nodes.keys().copied()
    }

    /// Every cable, sorted by its input endpoint.
    pub fn cables(&self) -> impl Iterator<Item = &Cable> {
        self.cables.values()
    }
}

/// Routing-only render plan: which arena devices run in which order and how their sockets
/// are wired. Owns no device instances.
///
/// It owns the shared signal slots (scratch between devices) and copies of the cables as
/// per-device routes. Swapping it in at a batch boundary only copies each routed node's
/// `connected` flags; device state is never touched. Arena devices it does not list are not
/// processed (silent, their state frozen) and do not receive notes until a later routing
/// lists them again.
pub struct RoutingPlan {
    /// Device id of each step, in render order (a multi-stage device appears once per stage).
    pub(crate) order: Vec<DeviceInstanceId>,
    /// Level boundaries into `order`/`steps`: level `k` is `level_starts[k]..level_starts[k+1]`.
    pub(crate) level_starts: Vec<usize>,
    /// One entry per device, in the order of their first stage (`RenderPlan` arena slot `k`).
    pub(crate) devices: Vec<Routed>,
    /// One (device, stage) step per render-graph node, level-major.
    pub(crate) steps: Vec<Step>,
    pub(crate) slots: Vec<Batch>,
    pub(crate) note_slots: Vec<NoteBuffer>,
    /// Interface `in_k` routes: `(host channel k-1, slot)`.
    pub(crate) host_in: Vec<Route>,
    /// Interface `out_k` routes: `(host channel k-1, slot)`.
    pub(crate) host_out: Vec<Route>,
    /// Merge buffer for Notes inputs that receive both a cable and scheduled notes.
    pub(crate) spare_notes: NoteBuffer,
    /// Console state (boxed so `HotEdit::Routing` stays small).
    pub(crate) console: Box<ConsoleRouting>,
}

/// The console part of a routing: PDC delay lines and report, and meter assignments.
pub(crate) struct ConsoleRouting {
    /// PDC delay lines, sorted by (summing input, source).
    pub(crate) delays: Vec<DelayLine>,
    /// PDC outcome.
    pub(crate) pdc: PdcReport,
    /// Meter slot `i` meters the endpoint with key `meter_keys[i]`.
    pub(crate) meter_keys: Vec<u64>,
    /// Metered interface host inputs: `(host channel, meter slot)`.
    pub(crate) host_meters: Vec<Route>,
}

impl std::fmt::Debug for RoutingPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutingPlan")
            .field("order", &self.order)
            .field("slot_count", &self.slot_count())
            .finish_non_exhaustive()
    }
}

impl RoutingPlan {
    /// Delay-compensation report of this routing.
    pub fn pdc_report(&self) -> &crate::PdcReport {
        &self.console.pdc
    }

    /// Device ids in render order (the interface excluded). Render order is over
    /// (device, stage) nodes, so a multi-stage device appears once per stage; single-stage
    /// devices appear once.
    pub fn order(&self) -> &[DeviceInstanceId] {
        &self.order
    }

    /// Devices grouped by dependency level, in render order (see [`crate::RenderPlan::levels`]).
    pub fn levels(&self) -> impl Iterator<Item = &[DeviceInstanceId]> {
        self.level_starts
            .windows(2)
            .map(|w| &self.order[w[0]..w[1]])
    }

    /// Number of shared signal buffers (Audio/Cv plus Notes slots) after liveness reuse.
    pub fn slot_count(&self) -> usize {
        self.slots.len() + self.note_slots.len()
    }
}

/// The engine-side device arena: a fixed number of slots, each holding at most one node, plus
/// an id -> slot index sorted by id. Both vectors are allocated once (non-RT) at their full
/// capacity, so inserting and removing on the audio thread never allocates or frees: nodes
/// are moved in and out as boxes, and the index is shifted within its capacity.
pub(crate) struct Arena {
    pub(crate) nodes: Vec<Option<Box<Node>>>,
    /// `(id, slot)` sorted by id, capacity = slot count.
    index: Vec<(DeviceInstanceId, usize)>,
}

/// Non-RT construction.
impl Arena {
    /// An empty arena with `capacity` slots.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            nodes: (0..capacity).map(|_| None).collect(),
            index: Vec::with_capacity(capacity),
        }
    }

    /// An arena of `capacity` slots holding a plan's nodes at slots `0..n`.
    pub(crate) fn from_plan(capacity: usize, routing: &RoutingPlan, nodes: Vec<Node>) -> Self {
        let mut arena = Self::new(capacity);
        for (k, (node, d)) in nodes.into_iter().zip(&routing.devices).enumerate() {
            arena.nodes[k] = Some(Box::new(node));
            arena.index.push((d.id, k));
        }
        arena.index.sort_unstable();
        arena
    }
}

/// Real-time side. Nothing here allocates, frees, locks or blocks.
impl Arena {
    /// True if no device is in the arena.
    pub(crate) fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// The node in `slot`, if any.
    pub(crate) fn node_mut(&mut self, slot: usize) -> Option<&mut Node> {
        self.nodes.get_mut(slot).and_then(|n| n.as_deref_mut())
    }

    /// The node of device `id` (binary search; no allocation).
    fn node_by_id(&mut self, id: DeviceInstanceId) -> Option<&mut Node> {
        let k = self.index.binary_search_by_key(&id, |e| e.0).ok()?;
        let slot = self.index[k].1;
        self.node_mut(slot)
    }

    /// Puts `node` into `slot` under `id`. Returns a node that must be retired instead of
    /// dropped: whatever occupied the slot, or `node` itself if it cannot be stored (slot out
    /// of range, id already present, index full). The handle never sends such inserts.
    pub(crate) fn insert(
        &mut self,
        id: DeviceInstanceId,
        slot: usize,
        node: Box<Node>,
    ) -> Option<Box<Node>> {
        let Err(k) = self.index.binary_search_by_key(&id, |e| e.0) else {
            return Some(node);
        };
        if self.index.len() == self.index.capacity() {
            return Some(node);
        }
        let Some(cell) = self.nodes.get_mut(slot) else {
            return Some(node);
        };
        if cell.is_some() {
            return Some(node);
        }
        *cell = Some(node);
        self.index.insert(k, (id, slot)); // within capacity: no reallocation
        None
    }

    /// The arena slot of device `id`.
    pub(crate) fn slot_of(&self, id: DeviceInstanceId) -> Option<usize> {
        let k = self.index.binary_search_by_key(&id, |e| e.0).ok()?;
        Some(self.index[k].1)
    }

    /// After a batch, publishes the `Device::meter_values` of every device that opted
    /// in, in ascending id order (deterministic). Other devices cost one bool test.
    pub(crate) fn publish_device_values(&self, meters: &Meters) {
        let mut buf = [0.0f32; MAX_METER_VALUES];
        for &(id, slot) in &self.index {
            if let Some(node) = self.nodes.get(slot).and_then(|n| n.as_deref())
                && node.has_meter_values
            {
                let n = node.device.meter_values(&mut buf).min(MAX_METER_VALUES);
                meters.publish_device_values(slot, id, &buf[..n]);
            }
        }
    }

    /// Takes device `id` out of the arena.
    pub(crate) fn remove(&mut self, id: DeviceInstanceId) -> Option<Box<Node>> {
        let k = self.index.binary_search_by_key(&id, |e| e.0).ok()?;
        let (_, slot) = self.index.remove(k);
        self.nodes.get_mut(slot).and_then(Option::take)
    }

    /// Publishes every arena device's `latency_frames()` into its slot of `bank`.
    /// One virtual call and one atomic swap per device; never allocates.
    pub(crate) fn publish_latencies(&self, bank: &Latencies) {
        for &(_, slot) in &self.index {
            if let Some(node) = self.nodes.get(slot).and_then(|n| n.as_deref()) {
                bank.publish(slot, node.device.latency_frames());
            }
        }
    }

    /// Sets a param of device `id` (clamped). Unknown targets are ignored. Applies whether or
    /// not the device is routed.
    pub(crate) fn set_param(&mut self, id: DeviceInstanceId, param: ParamId, plain: f32) {
        if let Some(node) = self.node_by_id(id) {
            node.set_param(param, plain);
        }
    }

    /// Hands a sample map to device `id` and returns what must be retired: the map
    /// the device gives back, or `map` itself for an unknown id. Applies whether or not the
    /// device is routed.
    pub(crate) fn swap_samples(
        &mut self,
        id: DeviceInstanceId,
        map: Option<SampleMapRef>,
    ) -> Option<SampleMapRef> {
        match self.node_by_id(id) {
            Some(node) => node.device.swap_samples(map),
            None => map,
        }
    }

    /// Hands pattern data to device `id` and returns what must be retired: the data
    /// the device gives back, or `data` itself for an unknown id. Applies whether or not the
    /// device is routed.
    pub(crate) fn swap_player_data(
        &mut self,
        id: DeviceInstanceId,
        data: Option<PlayerDataRef>,
    ) -> Option<PlayerDataRef> {
        match self.node_by_id(id) {
            Some(node) => node.device.swap_player_data(data),
            None => data,
        }
    }

    /// Hands `pb` to a routed device as this batch's playback (see `Node::playback`).
    pub(crate) fn set_playback(&mut self, id: DeviceInstanceId, pb: &[Batch; 2]) {
        if let Some(node) = self.node_by_id(id)
            && node.routed
        {
            node.playback = *pb;
            node.playback_on = true;
        }
    }

    /// Delivers a note to a routed device. Returns false (the caller counts a drop) for unknown
    /// or unrouted devices, non-Notes sockets and full buffers.
    pub(crate) fn deliver_note(
        &mut self,
        id: DeviceInstanceId,
        socket: SocketId,
        offset: u16,
        kind: NoteKind,
    ) -> bool {
        match self.node_by_id(id) {
            Some(node) if node.routed => node.deliver_note(socket, offset, kind),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Graph;
    use crate::graph::tests::{cfg, ep};
    use atomik_device_sdk::{Device, SocketId};
    use atomik_testkit::fakes::{
        CALL_LOG, CONSTANT_SOURCE, CallLog, ConstantSource, IMPULSE_ON_NOTE, INSERT_LOOP,
        ImpulseOnNote, InsertLoop, LATENCY_PASSTHROUGH, LatencyPassthrough, NOTE_THRU, NoteThru,
        PASSTHROUGH, Passthrough, SUMMING_BUS, SummingBus,
    };
    use proptest::prelude::*;

    fn id(n: u32) -> DeviceInstanceId {
        DeviceInstanceId(n)
    }

    fn out1() -> Endpoint {
        ep(Graph::INTERFACE, SocketId(2))
    }

    #[test]
    fn topology_new_has_only_the_interface() {
        let t = Topology::new(&cfg());
        assert_eq!(t.node_ids().count(), 0);
        assert_eq!(t.cables().count(), 0);
        let d = t.descriptor(Graph::INTERFACE).unwrap();
        assert_eq!(d.type_id, crate::INTERFACE_TYPE_ID);
        assert_eq!(d.sockets.len(), 4);
        assert!(t.descriptor(id(1)).is_none());
        assert_eq!(t.config(), &cfg());
        assert!(format!("{t:?}").contains("Topology"));
    }

    #[test]
    fn topology_add_node_rejects_duplicates_and_the_interface_id() {
        let mut t = Topology::new(&cfg());
        assert_eq!(t.add_node(id(5), &PASSTHROUGH), Ok(()));
        assert_eq!(
            t.add_node(id(5), &CONSTANT_SOURCE),
            Err(GraphError::DuplicateDevice)
        );
        assert_eq!(
            t.add_node(Graph::INTERFACE, &PASSTHROUGH),
            Err(GraphError::DuplicateDevice)
        );
        assert_eq!(t.descriptor(id(5)).unwrap().type_id, PASSTHROUGH.type_id);
        assert_eq!(t.node_ids().collect::<Vec<_>>(), vec![id(5)]);
    }

    #[test]
    fn topology_remove_node_removes_its_cables_and_errors() {
        let mut t = Topology::new(&cfg());
        t.add_node(id(1), &CONSTANT_SOURCE).unwrap();
        t.add_node(id(2), &PASSTHROUGH).unwrap();
        t.connect(
            ep(id(1), ConstantSource::AUDIO_OUT),
            ep(id(2), Passthrough::AUDIO_IN),
        )
        .unwrap();
        t.connect(ep(id(2), Passthrough::AUDIO_OUT), out1())
            .unwrap();
        t.connect(
            ep(id(1), ConstantSource::AUDIO_OUT),
            ep(Graph::INTERFACE, SocketId(3)),
        )
        .unwrap();
        assert_eq!(t.remove_node(id(2)), Ok(()));
        let left: Vec<_> = t.cables().copied().collect();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].from.device, id(1));
        assert_eq!(t.remove_node(id(2)), Err(GraphError::UnknownDevice));
        assert_eq!(
            t.remove_node(Graph::INTERFACE),
            Err(GraphError::CannotRemoveInterface)
        );
    }

    #[test]
    fn topology_connect_errors_match_graph_rules() {
        let mut t = Topology::new(&cfg());
        t.add_node(id(1), &CONSTANT_SOURCE).unwrap();
        t.add_node(id(2), &PASSTHROUGH).unwrap();
        t.add_node(id(3), &IMPULSE_ON_NOTE).unwrap();
        t.add_node(id(4), &PASSTHROUGH).unwrap();
        let (c, p, i, q) = (id(1), id(2), id(3), id(4));
        let cases = [
            (
                ep(id(9), SocketId(0)),
                ep(p, Passthrough::AUDIO_IN),
                GraphError::UnknownDevice,
            ),
            (
                ep(c, ConstantSource::AUDIO_OUT),
                ep(id(9), SocketId(0)),
                GraphError::UnknownDevice,
            ),
            (
                ep(c, SocketId(7)),
                ep(p, Passthrough::AUDIO_IN),
                GraphError::UnknownSocket,
            ),
            (
                ep(c, ConstantSource::AUDIO_OUT),
                ep(Graph::INTERFACE, SocketId(40)),
                GraphError::UnknownSocket,
            ),
            (
                ep(p, Passthrough::AUDIO_IN),
                ep(q, Passthrough::AUDIO_IN),
                GraphError::DirectionMismatch,
            ),
            (
                ep(p, Passthrough::AUDIO_OUT),
                ep(q, Passthrough::AUDIO_OUT),
                GraphError::DirectionMismatch,
            ),
            (
                ep(c, ConstantSource::AUDIO_OUT),
                ep(i, ImpulseOnNote::NOTES_IN),
                GraphError::KindMismatch,
            ),
            (
                ep(c, ConstantSource::CV_OUT),
                ep(p, Passthrough::AUDIO_IN),
                GraphError::KindMismatch,
            ),
            (
                ep(p, Passthrough::AUDIO_OUT),
                ep(p, Passthrough::AUDIO_IN),
                GraphError::WouldCreateCycle,
            ),
        ];
        for (from, to, err) in cases {
            assert_eq!(t.connect(from, to), Err(err), "{from:?} -> {to:?}");
        }
        assert_eq!(t.cables().count(), 0);
        t.connect(ep(p, Passthrough::AUDIO_OUT), ep(q, Passthrough::AUDIO_IN))
            .unwrap();
        assert_eq!(
            t.connect(ep(q, Passthrough::AUDIO_OUT), ep(p, Passthrough::AUDIO_IN)),
            Err(GraphError::WouldCreateCycle)
        );
        assert_eq!(
            t.connect(
                ep(c, ConstantSource::AUDIO_OUT),
                ep(q, Passthrough::AUDIO_IN)
            ),
            Err(GraphError::InputAlreadyConnected)
        );
        // Through the interface is never a cycle.
        t.connect(
            ep(Graph::INTERFACE, SocketId(0)),
            ep(p, Passthrough::AUDIO_IN),
        )
        .unwrap();
        t.connect(ep(q, Passthrough::AUDIO_OUT), out1()).unwrap();
        assert_eq!(t.cables().count(), 3);
    }

    #[test]
    fn topology_disconnect_removes_cable_or_errors() {
        let mut t = Topology::new(&cfg());
        t.add_node(id(1), &CONSTANT_SOURCE).unwrap();
        t.connect(ep(id(1), ConstantSource::AUDIO_OUT), out1())
            .unwrap();
        assert_eq!(t.disconnect(out1()), Ok(()));
        assert_eq!(t.disconnect(out1()), Err(GraphError::NotConnected));
        assert_eq!(
            t.disconnect(ep(id(42), SocketId(0))),
            Err(GraphError::UnknownDevice)
        );
        assert_eq!(
            t.disconnect(ep(id(1), SocketId(9))),
            Err(GraphError::UnknownSocket)
        );
        assert_eq!(t.cables().count(), 0);
    }

    #[test]
    fn topology_compile_orders_level_major_by_id_with_sparse_ids() {
        // 40 -> 7 -> 3; 40 -> 12; 25 alone. Levels {7?} no: {25, 40}, {7, 12}, {3}.
        let mut t = Topology::new(&cfg());
        for (n, d) in [
            (3, &PASSTHROUGH),
            (7, &PASSTHROUGH),
            (12, &PASSTHROUGH),
            (25, &PASSTHROUGH),
            (40, &CONSTANT_SOURCE),
        ] {
            t.add_node(id(n), d).unwrap();
        }
        let a = Passthrough::AUDIO_IN;
        t.connect(ep(id(40), ConstantSource::AUDIO_OUT), ep(id(7), a))
            .unwrap();
        t.connect(ep(id(7), Passthrough::AUDIO_OUT), ep(id(3), a))
            .unwrap();
        t.connect(ep(id(40), ConstantSource::AUDIO_OUT), ep(id(12), a))
            .unwrap();
        t.connect(ep(Graph::INTERFACE, SocketId(0)), ep(id(25), a))
            .unwrap();
        t.connect(ep(id(3), Passthrough::AUDIO_OUT), out1())
            .unwrap();
        let plan = t.compile().unwrap();
        let levels: Vec<Vec<_>> = plan.levels().map(<[_]>::to_vec).collect();
        assert_eq!(
            levels,
            vec![vec![id(25), id(40)], vec![id(7), id(12)], vec![id(3)]]
        );
        assert_eq!(plan.order(), &[id(25), id(40), id(7), id(12), id(3)]);
        assert!(format!("{plan:?}").contains("RoutingPlan"));
        // Compiling is repeatable and leaves the topology usable.
        assert_eq!(t.compile().unwrap().order(), plan.order());
    }

    #[test]
    fn topology_compile_empty_has_no_devices() {
        let plan = Topology::new(&cfg()).compile().unwrap();
        assert!(plan.order().is_empty());
        assert_eq!(plan.levels().count(), 0);
        assert_eq!(plan.slot_count(), 0);
    }

    #[test]
    fn topology_compile_revalidates_corrupted_cables() {
        let mut t = Topology::new(&cfg());
        t.add_node(id(1), &PASSTHROUGH).unwrap();
        t.add_node(id(2), &PASSTHROUGH).unwrap();
        t.connect(
            ep(id(1), Passthrough::AUDIO_OUT),
            ep(id(2), Passthrough::AUDIO_IN),
        )
        .unwrap();
        let back = Cable {
            from: ep(id(2), Passthrough::AUDIO_OUT),
            to: ep(id(1), Passthrough::AUDIO_IN),
        };
        t.cables.insert((back.to, back.from), back);
        assert!(matches!(t.compile(), Err(GraphError::WouldCreateCycle)));
    }

    // ---- Parity with Graph: the same random op sequence gives the same results. ----

    #[derive(Debug, Clone)]
    enum Op {
        Connect(u8, u8, u8, u8),
        Disconnect(u8, u8),
        Remove(u8),
        Add(u8),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            6 => (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>())
                .prop_map(|(a, b, c, d)| Op::Connect(a, b, c, d)),
            1 => (any::<u8>(), any::<u8>()).prop_map(|(a, b)| Op::Disconnect(a, b)),
            1 => any::<u8>().prop_map(Op::Remove),
            2 => any::<u8>().prop_map(Op::Add),
        ]
    }

    fn make(kind: u8) -> (Box<dyn Device>, &'static DeviceDescriptor) {
        match kind % 8 {
            0 => (ConstantSource::boxed(0.5), &CONSTANT_SOURCE),
            1 => (Passthrough::boxed(1.0), &PASSTHROUGH),
            2 => (ImpulseOnNote::boxed(), &IMPULSE_ON_NOTE),
            3 => (NoteThru::boxed(), &NOTE_THRU),
            4 => (SummingBus::boxed(), &SUMMING_BUS),
            5 => (
                LatencyPassthrough::boxed(u32::from(kind)),
                &LATENCY_PASSTHROUGH,
            ),
            6 => (InsertLoop::boxed(), &INSERT_LOOP),
            _ => (CallLog::boxed().0, &CALL_LOG),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]
        #[test]
        fn topology_matches_graph_for_random_op_sequences(
            initial in proptest::collection::vec(any::<u8>(), 2..8),
            ops in proptest::collection::vec(op(), 0..80),
        ) {
            let c = cfg();
            let mut g = Graph::new(&c);
            let mut t = Topology::new(&c);
            let add = |g: &mut Graph, t: &mut Topology, k: u8| {
                let (device, desc) = make(k);
                let id = g.add_device(device);
                prop_assert_eq!(t.add_node(id, desc), Ok(()));
                Ok(id.0 + 1)
            };
            let mut next = 1u32;
            for k in initial {
                next = add(&mut g, &mut t, k)?;
            }
            for o in ops {
                let max = next;
                let id = |x: u8| DeviceInstanceId(u32::from(x) % max);
                match o {
                    Op::Connect(a, sa, b, sb) => {
                        let (from, to) = (
                            ep(id(a), SocketId(u16::from(sa % 7))),
                            ep(id(b), SocketId(u16::from(sb % 7))),
                        );
                        prop_assert_eq!(g.connect(from, to), t.connect(from, to));
                    }
                    Op::Disconnect(a, s) => {
                        let to = ep(id(a), SocketId(u16::from(s % 7)));
                        prop_assert_eq!(g.disconnect(to).map(|_| ()), t.disconnect(to));
                    }
                    Op::Remove(a) => {
                        prop_assert_eq!(g.remove_device(id(a)).map(|_| ()), t.remove_node(id(a)));
                    }
                    Op::Add(k) => next = add(&mut g, &mut t, k)?,
                }
                prop_assert_eq!(
                    g.cables().copied().collect::<Vec<_>>(),
                    t.cables().copied().collect::<Vec<_>>()
                );
                prop_assert_eq!(g.device_ids().collect::<Vec<_>>(), t.node_ids().collect::<Vec<_>>());
            }
            let routing = t.compile().unwrap();
            let plan = g.compile().unwrap();
            prop_assert_eq!(routing.order(), plan.order());
            let a: Vec<Vec<_>> = routing.levels().map(<[_]>::to_vec).collect();
            let b: Vec<Vec<_>> = plan.levels().map(<[_]>::to_vec).collect();
            prop_assert_eq!(a, b);
            prop_assert_eq!(routing.slot_count(), plan.slot_count());
            // Topology latencies default to 0, so only Graph-side device latency can differ.
            prop_assert_eq!(routing.pdc_report().cable_delays.len(), plan.pdc_report().cable_delays.len());
        }
    }
}
