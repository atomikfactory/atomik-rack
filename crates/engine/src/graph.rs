//! The editable, non-RT device graph.

use crate::interface::interface_descriptor;
use atomik_core::{ParamId, SampleRate, Tempo};
#[cfg(test)]
use atomik_device_sdk::SocketDescriptor;
use atomik_device_sdk::{Device, DeviceDescriptor, SocketId};
use std::collections::BTreeMap;

/// Identifies a device instance in a graph. `DeviceInstanceId(0)` is the AudioInterface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceInstanceId(pub u32);

/// One socket of one device instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Endpoint {
    /// The device instance.
    pub device: DeviceInstanceId,
    /// The socket within that device.
    pub socket: SocketId,
}

/// A cable from an output socket to an input socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cable {
    /// Output endpoint (source).
    pub from: Endpoint,
    /// Input endpoint (destination).
    pub to: Endpoint,
}

/// Engine-wide settings shared by the graph, compile and the engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineConfig {
    /// Sample rate devices are prepared at.
    pub sample_rate: SampleRate,
    /// Tempo reported in `TimeInfo`.
    pub tempo: Tempo,
    /// Host input channels (`in_1..in_N` on the AudioInterface), at most 32.
    pub input_channels: u16,
    /// Host output channels (`out_1..out_M` on the AudioInterface), at most 32.
    pub output_channels: u16,
    /// Patch seed; each device is prepared with a seed mixed from this and its id.
    pub seed: u64,
}

/// Why a graph edit or compile was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    /// No device with that id exists.
    #[error("unknown device")]
    UnknownDevice,
    /// The device has no socket with that id.
    #[error("unknown socket")]
    UnknownSocket,
    /// The device has no param with that id.
    #[error("unknown param")]
    UnknownParam,
    /// `from` is not an output or `to` is not an input.
    #[error("a cable must go from an output socket to an input socket")]
    DirectionMismatch,
    /// The two sockets carry different signal kinds.
    #[error("socket kinds do not match")]
    KindMismatch,
    /// The input already has a cable.
    #[error("the input socket already has a cable")]
    InputAlreadyConnected,
    /// The cable would close a loop.
    #[error("the cable would create a cycle")]
    WouldCreateCycle,
    /// No cable is attached to the input.
    #[error("the input socket has no cable")]
    NotConnected,
    /// The AudioInterface cannot be removed.
    #[error("the audio interface cannot be removed")]
    CannotRemoveInterface,
    /// A device with that id already exists (`Topology::add_node`; the interface id is taken).
    #[error("a device with that id already exists")]
    DuplicateDevice,
}

pub(crate) struct Entry {
    pub(crate) device: Box<dyn Device>,
    /// Params explicitly set through [`Graph::set_param`] (already clamped). Re-applied after
    /// `prepare` in compile.
    pub(crate) params: BTreeMap<ParamId, f32>,
}

/// The editable model: devices, cables and param values. Lives on the non-RT side.
///
/// Every edit is validated, so a `Graph` is always acyclic and kind-correct.
/// The AudioInterface is a pseudo-device: it is a pure source through `in_k` and a pure sink
/// through `out_k`, so cabling `in_k` (directly or through devices) to `out_j` is not a cycle.
pub struct Graph {
    pub(crate) config: EngineConfig,
    pub(crate) interface: &'static DeviceDescriptor,
    pub(crate) devices: BTreeMap<DeviceInstanceId, Entry>,
    /// Keyed by (input, source): one cable per plain input, any number of distinct cables per
    /// summing input; deterministic order.
    pub(crate) cables: rules::CableMap,
    /// Delay compensation for compile (on by default).
    pub(crate) compensation: bool,
    next_id: u32,
}

impl std::fmt::Debug for Graph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Graph")
            .field("devices", &self.devices.keys().collect::<Vec<_>>())
            .field("cables", &self.cables.values().collect::<Vec<_>>())
            .finish()
    }
}

impl Graph {
    /// The AudioInterface pseudo-device.
    pub const INTERFACE: DeviceInstanceId = DeviceInstanceId(0);

    /// An empty graph holding only the AudioInterface (`in_1..in_N`, `out_1..out_M`).
    ///
    /// # Panics
    /// If `cfg` asks for more than 32 input or output channels.
    pub fn new(cfg: &EngineConfig) -> Self {
        Self {
            config: *cfg,
            interface: interface_descriptor(cfg.input_channels, cfg.output_channels),
            devices: BTreeMap::new(),
            cables: BTreeMap::new(),
            compensation: true,
            next_id: 1,
        }
    }

    /// The configuration the graph was created with.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Adds a device. Ids increase monotonically and are never reused.
    pub fn add_device(&mut self, device: Box<dyn Device>) -> DeviceInstanceId {
        let id = DeviceInstanceId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("device id space exhausted");
        self.devices.insert(
            id,
            Entry {
                device,
                params: BTreeMap::new(),
            },
        );
        id
    }

    /// Removes a device and every cable attached to it, returning the device.
    pub fn remove_device(&mut self, id: DeviceInstanceId) -> Result<Box<dyn Device>, GraphError> {
        if id == Self::INTERFACE {
            return Err(GraphError::CannotRemoveInterface);
        }
        let entry = self.devices.remove(&id).ok_or(GraphError::UnknownDevice)?;
        rules::detach(&mut self.cables, id);
        Ok(entry.device)
    }

    /// Gives device `id` its sample map before compile. Non-RT: it calls
    /// `Device::swap_samples` directly and drops what comes back (the previous map, or `map`
    /// itself for a device that takes no samples). `compile` calls `prepare`, which resets
    /// voices but not the map, so the compiled plan renders exactly like the same device given
    /// its map through [`crate::EngineHandle::set_device_samples`] after insertion.
    ///
    /// For [`crate::EngineHandle::insert_device`] there is no graph: call
    /// `Device::swap_samples` on the `Box<dyn Device>` before inserting it.
    ///
    /// # Errors
    /// [`GraphError::UnknownDevice`].
    pub fn set_device_samples(
        &mut self,
        id: DeviceInstanceId,
        map: Option<atomik_device_sdk::SampleMapRef>,
    ) -> Result<(), GraphError> {
        let entry = self.devices.get_mut(&id).ok_or(GraphError::UnknownDevice)?;
        drop(entry.device.swap_samples(map));
        Ok(())
    }

    /// Gives device `id` its pattern data before compile, like
    /// [`Graph::set_device_samples`]: non-RT, drops what `Device::swap_player_data` returns.
    ///
    /// # Errors
    /// [`GraphError::UnknownDevice`].
    pub fn set_device_player_data(
        &mut self,
        id: DeviceInstanceId,
        data: Option<atomik_device_sdk::PlayerDataRef>,
    ) -> Result<(), GraphError> {
        let entry = self.devices.get_mut(&id).ok_or(GraphError::UnknownDevice)?;
        drop(entry.device.swap_player_data(data));
        Ok(())
    }

    /// Connects an output to an input after validating ids, direction, kind, input
    /// uniqueness and acyclicity. On error the graph is unchanged.
    ///
    /// A `Direction::SummingInput` accepts any number of distinct cables (the same
    /// cable twice is [`GraphError::InputAlreadyConnected`]); a plain input still takes one.
    pub fn connect(&mut self, from: Endpoint, to: Endpoint) -> Result<(), GraphError> {
        let cable = Cable { from, to };
        rules::check_connect(&|id| self.descriptor(id), &self.cables, &cable)?;
        self.cables.insert((to, from), cable);
        Ok(())
    }

    /// Removes the cable attached to input `to`, returning it. For a summing input, removes
    /// every cable into it and returns the first one (lowest source endpoint).
    pub fn disconnect(&mut self, to: Endpoint) -> Result<Cable, GraphError> {
        rules::check_disconnect(&|id| self.descriptor(id), &self.cables, to)?;
        rules::remove_into(&mut self.cables, to).ok_or(GraphError::NotConnected)
    }

    /// Removes the one cable `from -> to` (for summing inputs), returning it.
    ///
    /// # Errors
    /// [`GraphError::UnknownDevice`]/[`GraphError::UnknownSocket`] for a bad `to`, or
    /// [`GraphError::NotConnected`] if there is no such cable.
    pub fn disconnect_cable(&mut self, from: Endpoint, to: Endpoint) -> Result<Cable, GraphError> {
        rules::socket(&|id| self.descriptor(id), to)?;
        self.cables
            .remove(&(to, from))
            .ok_or(GraphError::NotConnected)
    }

    /// Enables or disables delay compensation for [`Graph::compile`] (on by
    /// default). See the engine's console module docs for the rules.
    pub fn set_compensation(&mut self, enabled: bool) {
        self.compensation = enabled;
    }

    /// Whether [`Graph::compile`] compensates latency.
    pub fn compensation(&self) -> bool {
        self.compensation
    }

    /// Stores a param value (clamped through the descriptor). It is applied to the device
    /// after `prepare` when the graph is compiled.
    pub fn set_param(
        &mut self,
        id: DeviceInstanceId,
        param: ParamId,
        plain: f32,
    ) -> Result<(), GraphError> {
        if id == Self::INTERFACE {
            return Err(GraphError::UnknownParam);
        }
        let entry = self.devices.get_mut(&id).ok_or(GraphError::UnknownDevice)?;
        let desc = entry
            .device
            .descriptor()
            .params
            .get(param.0 as usize)
            .ok_or(GraphError::UnknownParam)?;
        entry.params.insert(param, desc.clamp(plain));
        Ok(())
    }

    /// The stored (clamped) value of a param set through [`Graph::set_param`], if any.
    pub fn param(&self, id: DeviceInstanceId, param: ParamId) -> Option<f32> {
        self.devices.get(&id)?.params.get(&param).copied()
    }

    /// The descriptor of a device (the AudioInterface's for [`Graph::INTERFACE`]).
    pub fn descriptor(&self, id: DeviceInstanceId) -> Option<&'static DeviceDescriptor> {
        if id == Self::INTERFACE {
            Some(self.interface)
        } else {
            self.devices.get(&id).map(|e| e.device.descriptor())
        }
    }

    /// Ids of every device except the interface, ascending.
    pub fn device_ids(&self) -> impl Iterator<Item = DeviceInstanceId> + '_ {
        self.devices.keys().copied()
    }

    /// Every cable, sorted by its input endpoint (then, for summing inputs, by source).
    pub fn cables(&self) -> impl Iterator<Item = &Cable> {
        self.cables.values()
    }

    /// The routing model of this graph: the same devices (by id and descriptor), cables and
    /// compensation setting, with every latency at 0. A caller that built its engine from a
    /// graph (the offline renderer) uses it to recompile the routing when a device's latency
    /// changes at run time ([`crate::EngineHandle::fill_latencies`], then
    /// [`crate::EngineHandle::submit_routing`]), the way the app does.
    pub fn topology(&self) -> crate::Topology {
        let mut topology = crate::Topology::new(&self.config);
        for (&id, entry) in &self.devices {
            topology
                .add_node(id, entry.device.descriptor())
                .expect("graph ids are unique and never the interface");
        }
        for cable in self.cables.values() {
            topology
                .connect(cable.from, cable.to)
                .expect("a graph's cables obey the rules the topology enforces");
        }
        topology.set_compensation(self.compensation);
        topology
    }

    #[cfg(test)]
    pub(crate) fn socket(&self, e: Endpoint) -> Result<&'static SocketDescriptor, GraphError> {
        rules::socket(&|id| self.descriptor(id), e)
    }
}

/// The HI-3 rules shared by [`Graph`] and [`crate::Topology`]: one implementation, so the two
/// can never disagree. Devices are looked up through a descriptor function that also answers
/// for the AudioInterface.
pub(crate) mod rules {
    use super::{Cable, DeviceInstanceId, Endpoint, Graph, GraphError};
    use atomik_device_sdk::{DeviceDescriptor, Direction, SocketDescriptor, SocketId};
    use std::collections::{BTreeMap, BTreeSet};

    /// Descriptor lookup (the interface included); `None` for unknown ids.
    pub(crate) type Lookup<'a> = &'a dyn Fn(DeviceInstanceId) -> Option<&'static DeviceDescriptor>;

    /// Cables keyed by (input endpoint, source endpoint): sorted by input, then source.
    pub(crate) type CableMap = BTreeMap<(Endpoint, Endpoint), Cable>;

    const LOWEST: Endpoint = Endpoint {
        device: DeviceInstanceId(0),
        socket: SocketId(0),
    };
    const HIGHEST: Endpoint = Endpoint {
        device: DeviceInstanceId(u32::MAX),
        socket: SocketId(u16::MAX),
    };

    /// Every cable into input `to`, by ascending source endpoint.
    pub(crate) fn into(cables: &CableMap, to: Endpoint) -> impl Iterator<Item = &Cable> {
        cables.range((to, LOWEST)..=(to, HIGHEST)).map(|(_, c)| c)
    }

    /// Removes every cable into `to`; returns the first one removed.
    pub(crate) fn remove_into(cables: &mut CableMap, to: Endpoint) -> Option<Cable> {
        let keys: Vec<_> = into(cables, to).map(|c| (c.to, c.from)).collect();
        let first = keys.first().and_then(|k| cables.get(k)).copied();
        for k in keys {
            cables.remove(&k);
        }
        first
    }

    /// The socket behind an endpoint.
    pub(crate) fn socket(
        lookup: Lookup<'_>,
        e: Endpoint,
    ) -> Result<&'static SocketDescriptor, GraphError> {
        lookup(e.device)
            .ok_or(GraphError::UnknownDevice)?
            .sockets
            .get(usize::from(e.socket.0))
            .ok_or(GraphError::UnknownSocket)
    }

    /// Checks ids, sockets, direction and kind of a cable (not uniqueness or cycles).
    pub(crate) fn validate_cable(lookup: Lookup<'_>, cable: &Cable) -> Result<(), GraphError> {
        let from = socket(lookup, cable.from)?;
        let to = socket(lookup, cable.to)?;
        if from.direction != Direction::Output || !to.direction.is_input() {
            return Err(GraphError::DirectionMismatch);
        }
        if from.kind != to.kind {
            return Err(GraphError::KindMismatch);
        }
        Ok(())
    }

    /// Every check of a new cable: validity, one cable per plain input (no duplicate cable
    /// into a summing input), acyclicity.
    pub(crate) fn check_connect(
        lookup: Lookup<'_>,
        cables: &CableMap,
        cable: &Cable,
    ) -> Result<(), GraphError> {
        validate_cable(lookup, cable)?;
        let summing = socket(lookup, cable.to)?.direction == Direction::SummingInput;
        let taken = if summing {
            cables.contains_key(&(cable.to, cable.from))
        } else {
            into(cables, cable.to).next().is_some()
        };
        if taken {
            return Err(GraphError::InputAlreadyConnected);
        }
        if would_create_cycle(lookup, cables, cable) {
            return Err(GraphError::WouldCreateCycle);
        }
        Ok(())
    }

    /// Checks that input `to` exists and has a cable.
    pub(crate) fn check_disconnect(
        lookup: Lookup<'_>,
        cables: &CableMap,
        to: Endpoint,
    ) -> Result<(), GraphError> {
        socket(lookup, to)?;
        if into(cables, to).next().is_some() {
            Ok(())
        } else {
            Err(GraphError::NotConnected)
        }
    }

    /// Re-validates every stored cable (defence in depth before compiling).
    pub(crate) fn revalidate(lookup: Lookup<'_>, cables: &CableMap) -> Result<(), GraphError> {
        let mut previous: Option<Endpoint> = None;
        for (&(to, from), cable) in cables {
            if (to, from) != (cable.to, cable.from) {
                return Err(GraphError::InputAlreadyConnected);
            }
            validate_cable(lookup, cable)?;
            let plain = socket(lookup, to)?.direction == Direction::Input;
            if plain && previous == Some(to) {
                return Err(GraphError::InputAlreadyConnected);
            }
            previous = Some(to);
        }
        Ok(())
    }

    /// Removes every cable attached to `id`.
    pub(crate) fn detach(cables: &mut CableMap, id: DeviceInstanceId) {
        cables.retain(|_, c| c.from.device != id && c.to.device != id);
    }

    /// True if adding `cable` closes a loop. Loops are checked over
    /// (device, stage) nodes, with cable edges from (source device, source socket stage) to
    /// (destination device, destination socket stage) and implicit edges from stage `s` to
    /// `s + 1` of the same device. The interface is a pure source (`in_k`) and a pure sink
    /// (`out_k`), so cables touching it can never close one.
    fn would_create_cycle(lookup: Lookup<'_>, cables: &CableMap, cable: &Cable) -> bool {
        let (src, dst) = (cable.from.device, cable.to.device);
        if src == Graph::INTERFACE || dst == Graph::INTERFACE {
            return false;
        }
        let stage = |e: Endpoint| lookup(e.device).map_or(0, |d| d.stage_of(e.socket));
        let target = (src, stage(cable.from));
        // Is (src, source stage) reachable from (dst, destination stage)?
        let mut stack = vec![(dst, stage(cable.to))];
        let mut visited = BTreeSet::new();
        while let Some(node) = stack.pop() {
            if node == target {
                return true;
            }
            if !visited.insert(node) {
                continue;
            }
            let (device, s) = node;
            if lookup(device).is_some_and(|d| s + 1 < d.stage_count()) {
                stack.push((device, s + 1));
            }
            stack.extend(
                cables
                    .values()
                    .filter(|c| {
                        c.from.device == device
                            && stage(c.from) == s
                            && c.to.device != Graph::INTERFACE
                    })
                    .map(|c| (c.to.device, stage(c.to))),
            );
        }
        false
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use atomik_device_sdk::{Direction, SocketKind};
    use atomik_testkit::fakes::{
        CallLog, ConstantSource, ImpulseOnNote, InsertLoop, Passthrough, RecordingSink, SummingBus,
    };
    use proptest::prelude::*;

    pub(crate) fn cfg() -> EngineConfig {
        EngineConfig {
            sample_rate: SampleRate::new(48_000.0).unwrap(),
            tempo: Tempo::new(120.0).unwrap(),
            input_channels: 2,
            output_channels: 2,
            seed: 7,
        }
    }

    pub(crate) fn ep(device: DeviceInstanceId, socket: SocketId) -> Endpoint {
        Endpoint { device, socket }
    }

    fn iface_out(k: u16) -> Endpoint {
        // out_k follows the 2 inputs in the socket table.
        ep(Graph::INTERFACE, SocketId(2 + k - 1))
    }

    #[test]
    fn graph_new_has_interface_with_configured_channels() {
        let mut c = cfg();
        c.input_channels = 3;
        c.output_channels = 5;
        let g = Graph::new(&c);
        let d = g.descriptor(Graph::INTERFACE).unwrap();
        assert_eq!(Graph::INTERFACE, DeviceInstanceId(0));
        assert_eq!(d.sockets.len(), 8);
        for k in 1..=3 {
            let s = d.socket_by_key(&format!("in_{k}")).unwrap();
            assert_eq!(
                (s.kind, s.direction),
                (SocketKind::Audio, Direction::Output)
            );
        }
        for k in 1..=5 {
            let s = d.socket_by_key(&format!("out_{k}")).unwrap();
            assert_eq!((s.kind, s.direction), (SocketKind::Audio, Direction::Input));
        }
        assert_eq!(g.cables().count(), 0);
        assert_eq!(g.device_ids().count(), 0);
    }

    #[test]
    fn add_device_returns_increasing_ids_and_descriptor() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(ConstantSource::boxed(1.0));
        let b = g.add_device(Passthrough::boxed(1.0));
        assert_eq!(a, DeviceInstanceId(1));
        assert_eq!(b, DeviceInstanceId(2));
        assert_eq!(g.descriptor(b).unwrap().type_id, "testkit.passthrough");
        assert!(g.descriptor(DeviceInstanceId(9)).is_none());
    }

    #[test]
    fn connect_valid_cable_is_listed() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(ConstantSource::boxed(1.0));
        let from = ep(a, ConstantSource::AUDIO_OUT);
        assert_eq!(g.connect(from, iface_out(1)), Ok(()));
        assert_eq!(
            g.cables().copied().collect::<Vec<_>>(),
            vec![Cable {
                from,
                to: iface_out(1)
            }]
        );
    }

    #[test]
    fn the_topology_of_a_graph_has_its_devices_cables_and_compensation_setting() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(ConstantSource::boxed(1.0));
        let b = g.add_device(Passthrough::boxed(1.0));
        g.connect(
            ep(a, ConstantSource::AUDIO_OUT),
            ep(b, Passthrough::AUDIO_IN),
        )
        .unwrap();
        g.connect(ep(b, Passthrough::AUDIO_OUT), iface_out(1))
            .unwrap();
        g.set_compensation(false);
        let t = g.topology();
        let mut by_hand = crate::Topology::new(&cfg());
        by_hand.add_node(a, g.descriptor(a).unwrap()).unwrap();
        by_hand.add_node(b, g.descriptor(b).unwrap()).unwrap();
        by_hand
            .connect(
                ep(a, ConstantSource::AUDIO_OUT),
                ep(b, Passthrough::AUDIO_IN),
            )
            .unwrap();
        by_hand
            .connect(ep(b, Passthrough::AUDIO_OUT), iface_out(1))
            .unwrap();
        assert_eq!(format!("{t:?}"), format!("{by_hand:?}"));
        assert!(!t.compensation(), "the compensation setting is carried");
        assert_eq!(t.latency(a), Some(0), "every latency starts at 0");
        // It compiles, and the graph compiles to a plan with the same devices.
        assert!(t.compile().is_ok());
    }

    #[test]
    fn connect_kind_mismatch_errors_and_leaves_graph_unchanged() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(ConstantSource::boxed(1.0));
        let b = g.add_device(ImpulseOnNote::boxed());
        // Audio output -> Notes input.
        assert_eq!(
            g.connect(
                ep(a, ConstantSource::AUDIO_OUT),
                ep(b, ImpulseOnNote::NOTES_IN)
            ),
            Err(GraphError::KindMismatch)
        );
        // Cv output -> Audio input.
        let p = g.add_device(Passthrough::boxed(1.0));
        assert_eq!(
            g.connect(ep(a, ConstantSource::CV_OUT), ep(p, Passthrough::AUDIO_IN)),
            Err(GraphError::KindMismatch)
        );
        assert_eq!(g.cables().count(), 0);
    }

    #[test]
    fn connect_direction_mismatch_errors() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(Passthrough::boxed(1.0));
        let b = g.add_device(Passthrough::boxed(1.0));
        // input -> input
        assert_eq!(
            g.connect(ep(a, Passthrough::AUDIO_IN), ep(b, Passthrough::AUDIO_IN)),
            Err(GraphError::DirectionMismatch)
        );
        // output -> output
        assert_eq!(
            g.connect(ep(a, Passthrough::AUDIO_OUT), ep(b, Passthrough::AUDIO_OUT)),
            Err(GraphError::DirectionMismatch)
        );
        assert_eq!(g.cables().count(), 0);
    }

    #[test]
    fn connect_unknown_device_or_socket_errors() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(Passthrough::boxed(1.0));
        let ghost = DeviceInstanceId(99);
        assert_eq!(
            g.connect(ep(ghost, SocketId(1)), ep(a, Passthrough::AUDIO_IN)),
            Err(GraphError::UnknownDevice)
        );
        assert_eq!(
            g.connect(ep(a, Passthrough::AUDIO_OUT), ep(ghost, SocketId(0))),
            Err(GraphError::UnknownDevice)
        );
        assert_eq!(
            g.connect(ep(a, SocketId(7)), iface_out(1)),
            Err(GraphError::UnknownSocket)
        );
        assert_eq!(
            g.connect(
                ep(a, Passthrough::AUDIO_OUT),
                ep(Graph::INTERFACE, SocketId(40))
            ),
            Err(GraphError::UnknownSocket)
        );
        assert_eq!(g.cables().count(), 0);
    }

    #[test]
    fn connect_input_twice_errors() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(ConstantSource::boxed(1.0));
        let b = g.add_device(ConstantSource::boxed(2.0));
        g.connect(ep(a, ConstantSource::AUDIO_OUT), iface_out(1))
            .unwrap();
        assert_eq!(
            g.connect(ep(b, ConstantSource::AUDIO_OUT), iface_out(1)),
            Err(GraphError::InputAlreadyConnected)
        );
        assert_eq!(g.cables().next().unwrap().from.device, a);
        assert_eq!(g.cables().count(), 1);
    }

    #[test]
    fn connect_creating_cycle_errors() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(Passthrough::boxed(1.0));
        let b = g.add_device(Passthrough::boxed(1.0));
        let c = g.add_device(Passthrough::boxed(1.0));
        g.connect(ep(a, Passthrough::AUDIO_OUT), ep(b, Passthrough::AUDIO_IN))
            .unwrap();
        // A->B, then B->A.
        assert_eq!(
            g.connect(ep(b, Passthrough::AUDIO_OUT), ep(a, Passthrough::AUDIO_IN)),
            Err(GraphError::WouldCreateCycle)
        );
        // Longer path: A->B->C, then C->A.
        g.connect(ep(b, Passthrough::AUDIO_OUT), ep(c, Passthrough::AUDIO_IN))
            .unwrap();
        assert_eq!(
            g.connect(ep(c, Passthrough::AUDIO_OUT), ep(a, Passthrough::AUDIO_IN)),
            Err(GraphError::WouldCreateCycle)
        );
        assert_eq!(g.cables().count(), 2);
    }

    #[test]
    fn connect_self_loop_errors() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(Passthrough::boxed(1.0));
        assert_eq!(
            g.connect(ep(a, Passthrough::AUDIO_OUT), ep(a, Passthrough::AUDIO_IN)),
            Err(GraphError::WouldCreateCycle)
        );
        assert_eq!(g.cables().count(), 0);
    }

    #[test]
    fn connect_through_interface_is_not_a_cycle() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(Passthrough::boxed(1.0));
        // interface in_1 -> a -> interface out_1, and interface in_2 -> out_2 directly.
        g.connect(
            ep(Graph::INTERFACE, SocketId(0)),
            ep(a, Passthrough::AUDIO_IN),
        )
        .unwrap();
        g.connect(ep(a, Passthrough::AUDIO_OUT), iface_out(1))
            .unwrap();
        assert_eq!(
            g.connect(ep(Graph::INTERFACE, SocketId(1)), iface_out(2)),
            Ok(())
        );
    }

    #[test]
    fn cables_are_sorted_by_input_endpoint() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(ConstantSource::boxed(1.0));
        let p = g.add_device(Passthrough::boxed(1.0));
        g.connect(
            ep(a, ConstantSource::AUDIO_OUT),
            ep(p, Passthrough::AUDIO_IN),
        )
        .unwrap();
        g.connect(ep(a, ConstantSource::AUDIO_OUT), iface_out(2))
            .unwrap();
        g.connect(ep(p, Passthrough::AUDIO_OUT), iface_out(1))
            .unwrap();
        let tos: Vec<_> = g.cables().map(|c| c.to).collect();
        let mut sorted = tos.clone();
        sorted.sort();
        assert_eq!(tos, sorted);
        assert_eq!(tos.len(), 3);
    }

    #[test]
    fn disconnect_removes_cable_or_errors() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(ConstantSource::boxed(1.0));
        let from = ep(a, ConstantSource::AUDIO_OUT);
        g.connect(from, iface_out(1)).unwrap();
        assert_eq!(
            g.disconnect(iface_out(1)),
            Ok(Cable {
                from,
                to: iface_out(1)
            })
        );
        assert_eq!(g.disconnect(iface_out(1)), Err(GraphError::NotConnected));
        assert_eq!(
            g.disconnect(ep(DeviceInstanceId(42), SocketId(0))),
            Err(GraphError::UnknownDevice)
        );
        assert_eq!(
            g.disconnect(ep(a, SocketId(9))),
            Err(GraphError::UnknownSocket)
        );
        assert_eq!(g.cables().count(), 0);
    }

    #[test]
    fn remove_device_removes_its_cables() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(ConstantSource::boxed(1.0));
        let p = g.add_device(Passthrough::boxed(1.0));
        let (sink, _) = RecordingSink::boxed(1);
        let s = g.add_device(sink);
        g.connect(
            ep(a, ConstantSource::AUDIO_OUT),
            ep(p, Passthrough::AUDIO_IN),
        )
        .unwrap();
        g.connect(ep(p, Passthrough::AUDIO_OUT), iface_out(1))
            .unwrap();
        g.connect(
            ep(a, ConstantSource::AUDIO_OUT),
            ep(s, RecordingSink::AUDIO_IN),
        )
        .unwrap();
        let Ok(removed) = g.remove_device(p) else {
            panic!("removable")
        };
        assert_eq!(removed.descriptor().type_id, "testkit.passthrough");
        let left: Vec<_> = g.cables().copied().collect();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].to.device, s);
        assert!(g.descriptor(p).is_none());
        assert!(matches!(g.remove_device(p), Err(GraphError::UnknownDevice)));
    }

    #[test]
    fn remove_interface_errors() {
        let mut g = Graph::new(&cfg());
        assert!(matches!(
            g.remove_device(Graph::INTERFACE),
            Err(GraphError::CannotRemoveInterface)
        ));
        assert!(g.descriptor(Graph::INTERFACE).is_some());
    }

    #[test]
    fn ids_are_never_reused() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(Passthrough::boxed(1.0));
        let b = g.add_device(Passthrough::boxed(1.0));
        assert!(g.remove_device(b).is_ok());
        assert!(g.remove_device(a).is_ok());
        let c = g.add_device(Passthrough::boxed(1.0));
        assert!(c > b && c > a, "{c:?}");
        assert_ne!(c, Graph::INTERFACE);
    }

    #[test]
    fn set_param_clamps_through_descriptor_and_errors_on_unknowns() {
        let mut g = Graph::new(&cfg());
        let a = g.add_device(ConstantSource::boxed(0.0));
        assert_eq!(g.set_param(a, ConstantSource::VALUE, 500.0), Ok(()));
        assert!((g.param(a, ConstantSource::VALUE).unwrap() - 10.0).abs() < 1e-7);
        assert_eq!(
            g.set_param(a, ParamId(3), 0.0),
            Err(GraphError::UnknownParam)
        );
        assert_eq!(
            g.set_param(DeviceInstanceId(77), ParamId(0), 0.0),
            Err(GraphError::UnknownDevice)
        );
        assert_eq!(
            g.set_param(Graph::INTERFACE, ParamId(0), 0.0),
            Err(GraphError::UnknownParam)
        );
        let (log, _) = CallLog::boxed();
        let l = g.add_device(log);
        assert!(g.param(l, CallLog::P).is_none());
    }

    /// Independent HI-3 oracle: every cable is output->input of equal kind, at most one cable
    /// per plain input (any number of distinct cables per summing input), and the device graph
    /// (interface split into source and sink) is acyclic.
    pub(crate) fn assert_valid(g: &Graph) {
        let mut inputs = std::collections::BTreeSet::new();
        let mut cables = std::collections::BTreeSet::new();
        for c in g.cables() {
            let (f, t) = (g.socket(c.from).unwrap(), g.socket(c.to).unwrap());
            assert_eq!(f.direction, Direction::Output);
            assert!(t.direction.is_input());
            assert_eq!(f.kind, t.kind);
            assert!(cables.insert(*c), "duplicate cable {c:?}");
            if t.direction == Direction::Input {
                assert!(inputs.insert(c.to), "two cables into {:?}", c.to);
            }
        }
        // Kahn over non-interface (device, stage) nodes, with the implicit s -> s+1 edges.
        let stage = |e: Endpoint| g.descriptor(e.device).unwrap().stage_of(e.socket);
        let mut ids = Vec::new();
        let mut edges = Vec::new();
        for d in g.device_ids() {
            let n = g.descriptor(d).unwrap().stage_count();
            for s in 0..n {
                ids.push((d, s));
                if s + 1 < n {
                    edges.push(((d, s), (d, s + 1)));
                }
            }
        }
        let mut indeg: BTreeMap<(DeviceInstanceId, u8), usize> =
            ids.iter().map(|i| (*i, 0)).collect();
        edges.extend(
            g.cables()
                .filter(|c| c.from.device != Graph::INTERFACE && c.to.device != Graph::INTERFACE)
                .map(|c| ((c.from.device, stage(c.from)), (c.to.device, stage(c.to)))),
        );
        for (_, t) in &edges {
            *indeg.get_mut(t).unwrap() += 1;
        }
        let mut ready: Vec<_> = indeg
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(i, _)| *i)
            .collect();
        let mut seen = 0;
        while let Some(n) = ready.pop() {
            seen += 1;
            for (f, t) in &edges {
                if *f == n {
                    let d = indeg.get_mut(t).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        ready.push(*t);
                    }
                }
            }
        }
        assert_eq!(seen, ids.len(), "graph has a cycle");
    }

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

    fn make(kind: u8) -> Box<dyn Device> {
        match kind % 6 {
            0 => ConstantSource::boxed(0.5),
            1 => Passthrough::boxed(1.0),
            2 => ImpulseOnNote::boxed(),
            3 => SummingBus::boxed(),
            4 => InsertLoop::boxed(),
            _ => CallLog::boxed().0,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]
        #[test]
        fn random_connect_sequences_keep_graph_valid(
            initial in proptest::collection::vec(any::<u8>(), 2..8),
            ops in proptest::collection::vec(op(), 0..80),
        ) {
            let mut g = Graph::new(&cfg());
            for k in initial {
                g.add_device(make(k));
            }
            for o in ops {
                let max = g.next_id;
                let id = |x: u8| DeviceInstanceId(u32::from(x) % max);
                let before: Vec<Cable> = g.cables().copied().collect();
                match o {
                    Op::Connect(a, sa, b, sb) => {
                        let r = g.connect(
                            ep(id(a), SocketId(u16::from(sa % 7))),
                            ep(id(b), SocketId(u16::from(sb % 7))),
                        );
                        if r.is_err() {
                            prop_assert_eq!(g.cables().copied().collect::<Vec<_>>(), before);
                        }
                    }
                    Op::Disconnect(a, s) => {
                        let _ = g.disconnect(ep(id(a), SocketId(u16::from(s % 7))));
                    }
                    Op::Remove(a) => {
                        let _ = g.remove_device(id(a));
                    }
                    Op::Add(k) => {
                        g.add_device(make(k));
                    }
                }
                assert_valid(&g);
            }
        }
    }
}
