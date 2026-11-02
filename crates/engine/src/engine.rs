//! The real-time [`Engine`] and its non-RT [`EngineHandle`].
//!
//! The two halves talk only through wait-free `rtrb` SPSC ring buffers, all allocated in
//! [`new_engine`]:
//!
//! | queue | direction | capacity |
//! |---|---|---|
//! | control: inserts, removals, routings, plans, timed params, tempo, songs and transport commands, sample maps | handle → engine | [`CONTROL_QUEUE_CAPACITY`] |
//! | retired plans, routings, song snapshots, record taps and replaced sample maps | engine → handle | [`RETIRE_QUEUE_CAPACITY`] |
//! | removed devices | engine → handle | arena capacity |
//! | param messages | handle → engine | [`PARAM_QUEUE_CAPACITY`] |
//! | scheduled notes | handle → engine | [`NOTE_QUEUE_CAPACITY`] |
//! | live notes | [`LiveNoteSender`] → engine | [`NOTE_QUEUE_CAPACITY`] |
//!
//! # Song playback
//!
//! The RT song player (`player.rs`) runs at every batch boundary after the control messages
//! and the queued params: it publishes the [`crate::TransportState`], applies automation and
//! emits the batch's song notes, which are merged with scheduled and live notes by frame (ties:
//! song, scheduled, live). The metronome is added to interface outputs 1/2 after the routing.
//! See the `player` module docs for the design and the determinism argument.
//!
//! # Device arena and control queue
//!
//! * **Arena.** Device instances (a `Node`: the device plus its per-socket scratch) live in a
//!   fixed-capacity arena owned by the engine ([`DEFAULT_ARENA_CAPACITY`] slots unless
//!   [`new_engine_with_arena`] says otherwise), with an id -> slot index sorted by id. Both are
//!   allocated up front; the audio thread only moves boxes in and out and shifts the index
//!   within its capacity.
//! * **Slots are assigned by the handle.** The handle mirrors the arena in queue order
//!   (id -> slot, free slots, routed ids), validates every edit against it, and sends slot
//!   indices. Because control messages are applied strictly in FIFO order, the mirror is
//!   exactly what the engine will hold once it catches up.
//! * **One ordered queue.** Inserts, removals, routings, `submit_plan`, timed params and tempo
//!   changes share one queue and are applied at batch boundaries, in order. A message carries
//!   a frame and waits for the first boundary whose batch starts at or after it (0 = next).
//! * **Nothing is freed on the audio thread.** Every applied message hands back at most one
//!   item (a retired plan with its old arena, a retired routing, or a removed device). If the
//!   return queue is full it is parked in a single holder and no further message is applied
//!   until it fits, like the plan retirement.
//! * **Determinism.** Edits land at batch boundaries chosen by frame, devices are
//!   prepared on the handle with the same seed as `Graph::compile`, and the routing's order is
//!   the deterministic (level, id) order; so the same edits give the same output for any host
//!   block size and worker count, provided `collect_garbage` runs often enough that the return
//!   queues never fill.

use crate::arena::{Arena, RoutingPlan};
use crate::console::{Latencies, Meters};
use crate::fault::FaultState;
use crate::graph::{DeviceInstanceId, EngineConfig, Graph};
use crate::live::LiveNoteSender;
use crate::plan::{Node, RenderPlan, mix_seed};
use crate::player::{AutomationHold, Player, TransportCell};
use crate::record::{RecordTap, TapChannel, TapControls, TapRt};
use crate::stream::{REQUEST_CAPACITY, StreamControl, StreamStats};
use crate::tune::Glide;
use crate::workers::Pool;
use atomik_core::{BATCH, Batch, NoteKind, ParamId, Tempo, TimeInfo};
use atomik_device_sdk::{DeviceDescriptor, PlayerDataRef, PrepareInfo, SampleMapRef, SocketId};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Former capacity of the plan submission queue. Plans travel through the
/// ordered control queue ([`CONTROL_QUEUE_CAPACITY`]); kept for compatibility.
pub const PLAN_QUEUE_CAPACITY: usize = 64;
/// Capacity of the retired plan and routing return queue. When it is full the engine parks one
/// item in a single-slot "pending retire" holder and postpones further control messages; it
/// never drops one.
pub const RETIRE_QUEUE_CAPACITY: usize = 256;
/// Capacity of the transport lane ([`EngineHandle::transport`]).
pub const TRANSPORT_QUEUE_CAPACITY: usize = 64;
/// Capacity of the param message queue.
pub const PARAM_QUEUE_CAPACITY: usize = 4096;
/// Capacity of the scheduled note queue (4 × `MAX_EVENTS_PER_BATCH`).
pub const NOTE_QUEUE_CAPACITY: usize = 4096;

/// A note event for one Notes input socket at an absolute timeline frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduledNote {
    /// Absolute frame. It lands in batch `frame / 64` at offset `frame % 64`; a frame in the
    /// past lands at offset 0 of the next batch processed.
    pub frame: u64,
    /// Target device.
    pub device: DeviceInstanceId,
    /// Target Notes input socket.
    pub socket: SocketId,
    /// The event.
    pub kind: NoteKind,
}

/// Errors returned by [`EngineHandle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// The queue to the engine is full; retry after the engine has rendered.
    #[error("the engine queue is full")]
    QueueFull,
    /// The device arena has no free slot; remove a device or use a larger
    /// arena ([`new_engine_with_arena`]). Nothing was queued.
    #[error("the device arena is full")]
    ArenaFull,
    /// A device with that id is already in the arena (or the id is `Graph::INTERFACE`).
    #[error("a device with that id is already in the arena")]
    DuplicateDevice,
    /// No device with that id is in the arena (in queue order).
    #[error("no device with that id is in the arena")]
    UnknownDevice,
    /// The device is used by the latest submitted routing; submit a routing without it first.
    #[error("the device is used by the current routing")]
    DeviceInUse,
    /// A routing names a device whose type differs from the arena instance with that id.
    #[error("the routing's device type differs from the arena device")]
    TypeMismatch,
}

/// Default number of device slots in the engine's arena. Slots are
/// preallocated (a pointer each, plus an 8-byte index entry); a plan or insert that would exceed
/// it is rejected with [`EngineError::ArenaFull`].
pub const DEFAULT_ARENA_CAPACITY: usize = 4096;

/// Song note events the RT player can emit per batch: notes, loop-wrap releases and
/// transport AllOffs. Events beyond it are dropped and counted in `dropped_events`; a dropped
/// release stays pending (the key stays held) and is retried at the next release point.
pub const SONG_EVENT_CAPACITY: usize = 8192;

/// Capacity of the ordered control queue (inserts, removals, routings, plans, timed params and
/// tempo changes).
pub const CONTROL_QUEUE_CAPACITY: usize = 1024;

/// One hot edit for [`EngineHandle::schedule`]. Each variant behaves like the
/// matching handle method, but is applied at a given timeline frame.
pub enum HotEdit {
    /// [`EngineHandle::insert_device`].
    Insert {
        /// New, unused id.
        id: DeviceInstanceId,
        /// The device (prepared by the handle).
        device: Box<dyn atomik_device_sdk::Device>,
        /// Params applied after `prepare` (clamped; unknown ids ignored).
        params: Vec<(ParamId, f32)>,
    },
    /// [`EngineHandle::remove_device`].
    Remove(DeviceInstanceId),
    /// [`EngineHandle::submit_routing`].
    Routing(crate::RoutingPlan),
    /// A param change (clamped); like [`EngineHandle::set_param`] but ordered with the other
    /// control messages.
    Param {
        /// Target device.
        device: DeviceInstanceId,
        /// Param.
        param: ParamId,
        /// Plain value.
        plain: f32,
    },
    /// [`EngineHandle::set_tempo`].
    Tempo(atomik_core::Tempo),
    /// [`EngineHandle::submit_song`].
    Song(crate::SongSnapshot),
    /// [`EngineHandle::transport`].
    Transport(crate::TransportCommand),
    /// [`EngineHandle::set_device_samples`].
    Samples {
        /// Target device.
        device: DeviceInstanceId,
        /// The new sample map, or `None` to clear it.
        map: Option<SampleMapRef>,
    },
    /// [`EngineHandle::set_device_player_data`].
    PlayerData {
        /// Target device.
        device: DeviceInstanceId,
        /// The new pattern data, or `None` to clear it.
        data: Option<PlayerDataRef>,
    },
}

impl std::fmt::Debug for HotEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Insert { id, device, params } => f
                .debug_struct("Insert")
                .field("id", id)
                .field("type_id", &device.descriptor().type_id)
                .field("params", params)
                .finish(),
            Self::Remove(id) => f.debug_tuple("Remove").field(id).finish(),
            Self::Routing(r) => f.debug_tuple("Routing").field(r).finish(),
            Self::Param {
                device,
                param,
                plain,
            } => f
                .debug_struct("Param")
                .field("device", device)
                .field("param", param)
                .field("plain", plain)
                .finish(),
            Self::Tempo(t) => f.debug_tuple("Tempo").field(t).finish(),
            Self::Song(s) => f.debug_tuple("Song").field(s).finish(),
            Self::Transport(c) => f.debug_tuple("Transport").field(c).finish(),
            Self::Samples { device, map } => f
                .debug_struct("Samples")
                .field("device", device)
                .field("zones", &map.as_ref().map(|m| m.zones().len()))
                .finish(),
            Self::PlayerData { device, data } => f
                .debug_struct("PlayerData")
                .field("device", device)
                .field("data", &data.as_ref().map(|d| &**d))
                .finish(),
        }
    }
}

/// A [`HotEdit`] at a timeline frame (for scripted offline renders).
#[derive(Debug)]
pub struct TimedEdit {
    /// Applied at the first batch boundary at or after this frame.
    pub frame: u64,
    /// The edit.
    pub edit: HotEdit,
}

#[derive(Debug, Clone, Copy)]
struct ParamMessage {
    device: DeviceInstanceId,
    param: ParamId,
    plain: f32,
}

/// Queue capacities used by [`new_engine`] (and enlarged by `render_offline` when needed).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Capacities {
    pub(crate) params: usize,
    pub(crate) notes: usize,
    /// Device arena slots.
    pub(crate) arena: usize,
}

impl Default for Capacities {
    fn default() -> Self {
        Self {
            params: PARAM_QUEUE_CAPACITY,
            notes: NOTE_QUEUE_CAPACITY,
            arena: DEFAULT_ARENA_CAPACITY,
        }
    }
}

/// One message of the ordered control queue: applied at the first batch boundary whose batch
/// start frame is `>= frame` (0 = the next boundary), never before the messages queued ahead.
struct Control {
    frame: u64,
    op: ControlOp,
}

/// One message of the transport lane: a transport command that waits only for the `after`
/// control messages queued before it, and applies even while a retirement is parked.
#[derive(Clone, Copy)]
struct TransportMsg {
    after: u64,
    command: crate::TransportCommand,
}

enum ControlOp {
    /// Put a prepared node into a free arena slot.
    Insert {
        id: DeviceInstanceId,
        slot: usize,
        node: Box<Node>,
    },
    /// Take a device out of the arena and hand it back.
    Remove(DeviceInstanceId),
    /// Swap the routing; the old one is handed back.
    Routing(Box<RoutingPlan>),
    /// `submit_plan`: swap the whole arena storage and the routing.
    Plan(Box<PlanSwap>),
    /// A param change ordered with the other control messages.
    Param(ParamMessage),
    /// Tempo for `TimeInfo` from now on.
    Tempo(Tempo),
    /// The master tune for `TimeInfo::tune_cents`: `glide` moves there over a few batches,
    /// otherwise the value is in force from this boundary on.
    Tune { cents: f32, glide: bool },
    /// Swap the song snapshot; the old one is handed back.
    Song(Box<crate::SongSnapshot>),
    /// A transport command.
    Transport(crate::TransportCommand),
    /// Install (or, with `None`, remove) the record tap.
    Tap(Option<Box<TapRt>>),
    /// Hand a sample map to a device; the map it hands back is retired.
    Samples {
        id: DeviceInstanceId,
        map: Option<SampleMapRef>,
    },
    /// Hand pattern data to a device; the data it hands back is retired.
    PlayerData {
        id: DeviceInstanceId,
        data: Option<PlayerDataRef>,
    },
    /// Start a preview stream (the current one fades out).
    PreviewStart {
        stream: crate::PreviewStream,
        gain: f32,
    },
    /// Fade the preview out.
    PreviewStop,
    /// Put a tap on the notes device `id` receives (or, with `None`, take it off); the tap it
    /// replaces is retired.
    NoteTap {
        id: DeviceInstanceId,
        tap: Option<std::sync::Arc<atomik_device_sdk::MidiTap>>,
    },
}

/// A complete arena plus routing. Travels to the engine as the new state and back to the handle
/// carrying the old state, so a plan swap is two pointer swaps and no allocation or free.
pub(crate) struct PlanSwap {
    arena: Arena,
    routing: Option<Box<RoutingPlan>>,
}

/// A retired plan (the old arena with every device in it, plus the old routing) or routing.
pub(crate) enum Retired {
    Plan(#[allow(dead_code)] Box<PlanSwap>), // held only to be dropped on the handle side
    Routing(#[allow(dead_code)] Box<RoutingPlan>),
    Song(#[allow(dead_code)] Box<crate::SongSnapshot>),
    Tap(#[allow(dead_code)] Box<TapRt>),
    Samples(#[allow(dead_code)] SampleMapRef),
    PlayerData(#[allow(dead_code)] PlayerDataRef),
    Preview(#[allow(dead_code)] crate::PreviewStream),
    NoteTap(#[allow(dead_code)] std::sync::Arc<atomik_device_sdk::MidiTap>),
}

/// Something the engine must hand back instead of dropping.
enum Garbage {
    Retired(Retired),
    Device(Box<Node>),
}

/// The handle's view of the arena, in queue order (what the engine will hold once it has
/// applied every queued control message).
struct Mirror {
    capacity: usize,
    devices: BTreeMap<DeviceInstanceId, (usize, &'static DeviceDescriptor)>,
    /// Slots freed by removals, reused lowest first.
    free: BTreeSet<usize>,
    /// Slots `next_unused..capacity` have never been used since the last plan.
    next_unused: usize,
    /// Devices referenced by the latest queued routing or plan.
    routed: BTreeSet<DeviceInstanceId>,
}

impl Mirror {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            devices: BTreeMap::new(),
            free: BTreeSet::new(),
            next_unused: 0,
            routed: BTreeSet::new(),
        }
    }

    /// The slot the next insert gets, if any (deterministic: lowest freed slot, else the next
    /// never-used one).
    fn peek_free(&self) -> Option<usize> {
        self.free
            .first()
            .copied()
            .or((self.next_unused < self.capacity).then_some(self.next_unused))
    }

    fn take(&mut self, slot: usize) {
        if !self.free.remove(&slot) {
            self.next_unused += 1;
        }
    }
}

/// The real-time half. Owned by the audio thread; [`Engine::render`] never allocates, frees,
/// locks or blocks.
pub struct Engine {
    config: EngineConfig,
    /// Tempo reported in `TimeInfo` (starts at `config.tempo`; changed by control messages).
    tempo: Tempo,
    /// The master tune the devices are told this batch, and the one it glides to
    /// ([`EngineHandle::set_master_tune`]).
    tune: Glide,
    /// The persistent device arena.
    arena: Arena,
    routing: Option<Box<RoutingPlan>>,
    /// The engine's initial (empty) arena storage after the first plan swap: kept here instead
    /// of being handed back, and freed when the engine is dropped (non-RT).
    spare: Option<Box<PlanSwap>>,
    /// A garbage item that did not fit its return queue; control messages wait until it does.
    pending_retire: Option<Garbage>,
    control_rx: Consumer<Control>,
    /// The transport lane and the number of control messages applied so far (its ordering).
    transport_rx: Consumer<TransportMsg>,
    control_applied: u64,
    /// Batches that began with a retirement parked (shared with the handle).
    stalled: Arc<AtomicU64>,
    retire_tx: Producer<Retired>,
    removed_tx: Producer<Box<Node>>,
    param_rx: Consumer<ParamMessage>,
    note_rx: Consumer<ScheduledNote>,
    live_rx: Consumer<ScheduledNote>,
    dropped: Arc<AtomicU64>,
    /// The first device that panicked while rendering (sticky), shared with the handle.
    fault: Arc<FaultState>,
    host_frames: u64,
    in_fifo: Vec<Batch>,
    out_fifo: Vec<Batch>,
    fifo_pos: usize,
    frame: u64,
    /// Worker threads; `None` when `threads = 0` (the single-threaded path).
    pool: Option<Pool>,
    /// Barrier spins of the latest `render` call (xrun diagnostics; telemetry only).
    last_render_spins: u64,
    /// Highest `last_render_spins` since the last `take_max_barrier_spins`.
    max_render_spins: u64,
    /// Song player and transport.
    player: Player,
    /// Meter bank, shared with the handle.
    meters: Arc<Meters>,
    /// Device latencies per arena slot, shared with the handle.
    latencies: Arc<Latencies>,
    /// The record tap, if one is installed.
    tap: Option<Box<TapRt>>,
    /// The preview voice (outside the graph).
    preview: crate::preview::PreviewVoice,
    /// Whether the preview voice is sounding, published once per batch.
    preview_active: Arc<AtomicBool>,
}

/// The non-RT half: submits plans, routings, devices, params and notes, and drops whatever the
/// engine hands back.
pub struct EngineHandle {
    config: EngineConfig,
    control_tx: Producer<Control>,
    transport_tx: Producer<TransportMsg>,
    /// Control messages pushed so far; a transport message carries it as its place in line.
    control_sent: u64,
    stalled: Arc<AtomicU64>,
    retire_rx: Consumer<Retired>,
    removed_rx: Consumer<Box<Node>>,
    param_tx: Producer<ParamMessage>,
    note_tx: Producer<ScheduledNote>,
    live_tx: Option<Producer<ScheduledNote>>,
    dropped: Arc<AtomicU64>,
    fault: Arc<FaultState>,
    mirror: Mirror,
    /// Published transport state.
    transport: Arc<TransportCell>,
    /// The automation lanes the player leaves alone (a take is writing them).
    hold: Arc<AutomationHold>,
    /// MIDI clock out: its settings and the queue of its messages.
    sync: Arc<crate::SyncOut>,
    /// The clock source the player follows when asked to.
    external: Arc<crate::ExternalClock>,
    /// Meter bank.
    meters: Arc<Meters>,
    /// Device latencies per arena slot.
    latencies: Arc<Latencies>,
    /// Disk streaming for audio clips: the streamer thread and its counters.
    streams: StreamControl,
    /// Whether the preview voice is sounding (published by the engine each batch).
    preview_active: Arc<AtomicBool>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("frame", &self.frame)
            .field("has_plan", &self.routing.is_some())
            .field("workers", &self.worker_count())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHandle")
            .field("dropped_events", &self.dropped_events())
            .field("devices", &self.mirror.devices.len())
            .finish_non_exhaustive()
    }
}

/// Creates a connected engine (RT side) and handle (non-RT side), allocating every queue, FIFO
/// and the device arena ([`DEFAULT_ARENA_CAPACITY`] slots) up front. The engine starts with no
/// plan and renders silence.
pub fn new_engine(cfg: EngineConfig) -> (Engine, EngineHandle) {
    new_engine_with(cfg, Capacities::default())
}

/// Like [`new_engine`], with multicore options. `threads = 0` behaves
/// exactly like [`new_engine`]. Worker threads are spawned here (non-RT) and joined when the
/// [`Engine`] is dropped.
pub fn new_engine_with_options(
    cfg: EngineConfig,
    options: crate::live::EngineOptions,
) -> (Engine, EngineHandle) {
    new_engine_with_arena(cfg, options, DEFAULT_ARENA_CAPACITY)
}

/// Like [`new_engine_with_options`], with an arena of `arena_capacity` device slots instead of
/// [`DEFAULT_ARENA_CAPACITY`]. The arena, its id index and the removed-device return
/// queue are all sized by it and allocated here.
pub fn new_engine_with_arena(
    cfg: EngineConfig,
    options: crate::live::EngineOptions,
    arena_capacity: usize,
) -> (Engine, EngineHandle) {
    let caps = Capacities {
        arena: arena_capacity,
        ..Capacities::default()
    };
    let (mut engine, handle) = new_engine_with(cfg, caps);
    let workers = options.workers;
    if workers.threads > 0 {
        let linger = match options.worker_linger_spins {
            0 => crate::workers::WORKER_SPIN_LIMIT,
            n => n,
        };
        // If the OS refuses every worker thread the engine renders single-threaded: the
        // output does not depend on the worker count.
        engine.pool = Pool::new(workers.threads, workers.on_thread_start, linger).ok();
    }
    (engine, handle)
}

pub(crate) fn new_engine_with(cfg: EngineConfig, caps: Capacities) -> (Engine, EngineHandle) {
    let (control_tx, control_rx) = RingBuffer::new(CONTROL_QUEUE_CAPACITY);
    let (transport_tx, transport_rx) = RingBuffer::new(TRANSPORT_QUEUE_CAPACITY);
    let stalled = Arc::new(AtomicU64::new(0));
    let (retire_tx, retire_rx) = RingBuffer::new(RETIRE_QUEUE_CAPACITY);
    let (removed_tx, removed_rx) = RingBuffer::new(caps.arena.max(1));
    let (param_tx, param_rx) = RingBuffer::new(caps.params);
    let (note_tx, note_rx) = RingBuffer::new(caps.notes);
    let (live_tx, live_rx) = RingBuffer::new(NOTE_QUEUE_CAPACITY);
    let dropped = Arc::new(AtomicU64::new(0));
    let fault = Arc::new(FaultState::default());
    let (request_tx, request_rx) = RingBuffer::new(REQUEST_CAPACITY);
    let stream_stats = Arc::new(StreamStats::default());
    let player = Player::new(cfg.sample_rate.hz(), request_tx, Arc::clone(&stream_stats));
    let transport = player.shared();
    let hold = player.hold();
    let sync = player.sync_out();
    let external = player.external();
    let meters = Arc::new(Meters::new(caps.arena));
    let latencies = Arc::new(Latencies::new(caps.arena));
    let preview_active = Arc::new(AtomicBool::new(false));
    let engine = Engine {
        config: cfg,
        tempo: cfg.tempo,
        tune: Glide::new(cfg.sample_rate.hz()),
        arena: Arena::new(caps.arena),
        routing: None,
        spare: None,
        pending_retire: None,
        control_rx,
        transport_rx,
        control_applied: 0,
        stalled: Arc::clone(&stalled),
        retire_tx,
        removed_tx,
        param_rx,
        note_rx,
        live_rx,
        dropped: Arc::clone(&dropped),
        fault: Arc::clone(&fault),
        host_frames: 0,
        in_fifo: vec![[0.0; BATCH]; usize::from(cfg.input_channels)],
        out_fifo: vec![[0.0; BATCH]; usize::from(cfg.output_channels)],
        fifo_pos: 0,
        frame: 0,
        pool: None,
        last_render_spins: 0,
        max_render_spins: 0,
        player,
        meters: Arc::clone(&meters),
        latencies: Arc::clone(&latencies),
        tap: None,
        preview: crate::preview::PreviewVoice::new(cfg.sample_rate.hz()),
        preview_active: Arc::clone(&preview_active),
    };
    let handle = EngineHandle {
        config: cfg,
        control_tx,
        transport_tx,
        control_sent: 0,
        stalled,
        retire_rx,
        removed_rx,
        param_tx,
        note_tx,
        live_tx: Some(live_tx),
        dropped,
        fault,
        mirror: Mirror::new(caps.arena),
        transport,
        hold,
        sync,
        external,
        meters,
        latencies,
        streams: StreamControl::new(stream_stats, request_rx),
        preview_active,
    };
    (engine, handle)
}

impl EngineHandle {
    /// Queues a plan; the engine swaps it in at the next batch boundary.
    ///
    /// The plan's devices replace the whole device arena (every device inserted
    /// before is handed back with the old plan), and its routing becomes current. Devices
    /// inserted afterwards join the plan's devices.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`], or [`EngineError::ArenaFull`] if the plan has more devices
    /// than the arena has slots. On error the plan is dropped here, on the non-RT thread.
    pub fn submit_plan(&mut self, plan: RenderPlan) -> Result<(), EngineError> {
        let capacity = self.mirror.capacity;
        if plan.nodes.len() > capacity {
            return Err(EngineError::ArenaFull);
        }
        if self.control_tx.is_full() {
            return Err(EngineError::QueueFull);
        }
        let RenderPlan { routing, nodes } = plan;
        self.meters.clear_scopes();
        self.meters.clear_midis();
        for (k, node) in nodes.iter().enumerate() {
            self.latencies.publish(k, node.device.latency_frames());
            if let (Some(tap), Some(routed)) = (node.device.scope_tap(), routing.devices.get(k)) {
                self.meters.register_scope(routed.id, tap);
            }
            if let (Some(tap), Some(routed)) = (node.device.midi_tap(), routing.devices.get(k)) {
                self.meters.register_midi(routed.id, tap);
            }
        }
        let devices: BTreeMap<_, _> = routing
            .devices
            .iter()
            .map(|d| (d.id, (d.slot, d.descriptor)))
            .collect();
        let routed = devices.keys().copied().collect();
        let swap = PlanSwap {
            arena: Arena::from_plan(capacity, &routing, nodes),
            routing: Some(Box::new(routing)),
        };
        self.push(0, ControlOp::Plan(Box::new(swap)))?;
        self.mirror = Mirror {
            next_unused: devices.len(),
            devices,
            routed,
            ..Mirror::new(capacity)
        };
        Ok(())
    }

    /// Queues a param change (clamped by the engine); applied at the next batch boundary,
    /// after that boundary's control messages. Addresses arena devices by id, routed or not;
    /// unknown targets are ignored.
    pub fn set_param(
        &mut self,
        device: DeviceInstanceId,
        param: ParamId,
        plain: f32,
    ) -> Result<(), EngineError> {
        self.param_tx
            .push(ParamMessage {
                device,
                param,
                plain,
            })
            .map_err(|_| EngineError::QueueFull)
    }

    /// Queues a note. Notes must be sent in non-decreasing frame order. Notes for devices that
    /// are not in the current routing are dropped and counted.
    pub fn send_note(&mut self, note: ScheduledNote) -> Result<(), EngineError> {
        self.note_tx.push(note).map_err(|_| EngineError::QueueFull)
    }

    /// Drops, on this (non-RT) thread, everything the engine handed back: retired plans
    /// (each with the devices of its arena), retired routings and removed devices. Returns how
    /// many items were dropped. Call it regularly (e.g. once per UI frame): while a return
    /// queue is full the engine postpones further control messages.
    pub fn collect_garbage(&mut self) -> usize {
        let mut count = 0;
        while let Ok(item) = self.retire_rx.pop() {
            drop(item);
            count += 1;
        }
        while let Ok(node) = self.removed_rx.pop() {
            drop(node);
            count += 1;
        }
        count
    }

    /// Prepares `device` (non-RT, on this thread), applies `params` (clamped), and queues it
    /// into the engine's device arena under `id`. The id must be unused.
    /// Takes effect at the next batch boundary, in queue order with routing and removals.
    ///
    /// `prepare` gets the engine's sample rate and the seed `Graph::compile` would give the
    /// same id, so an inserted device renders exactly like the same device in a plan. Unknown
    /// param ids are ignored. The device stays silent until a routing includes it.
    ///
    /// # Errors
    /// [`EngineError::DuplicateDevice`], [`EngineError::ArenaFull`] (checked before
    /// `prepare`), or [`EngineError::QueueFull`]. On error the device is dropped here.
    pub fn insert_device(
        &mut self,
        id: crate::DeviceInstanceId,
        device: Box<dyn atomik_device_sdk::Device>,
        params: &[(atomik_core::ParamId, f32)],
    ) -> Result<(), EngineError> {
        self.insert_at(0, id, device, params)
    }

    /// Queues the removal of device `id` from the arena. The engine hands the instance back;
    /// it is dropped on this side by `collect_garbage` (never on the RT thread).
    ///
    /// # Errors
    /// [`EngineError::UnknownDevice`], [`EngineError::DeviceInUse`] if the latest submitted
    /// routing still uses it (submit a routing without it first), or
    /// [`EngineError::QueueFull`].
    pub fn remove_device(&mut self, id: crate::DeviceInstanceId) -> Result<(), EngineError> {
        self.remove_at(0, id)
    }

    /// Queues a routing-only plan. Devices it references must have been inserted earlier in
    /// queue order; devices it no longer references keep their state (and stay silent) until
    /// removed or routed again.
    ///
    /// # Errors
    /// [`EngineError::UnknownDevice`] if it names a device not in the arena,
    /// [`EngineError::TypeMismatch`] if a device's type differs from the arena instance, or
    /// [`EngineError::QueueFull`]. On error nothing is queued.
    pub fn submit_routing(&mut self, plan: crate::RoutingPlan) -> Result<(), EngineError> {
        self.routing_at(0, plan)
    }

    /// Queues a tempo change through the control queue; from the next batch boundary on,
    /// `TimeInfo::tempo` reports it. Tempo-synced devices follow it.
    ///
    /// While a song is loaded, its tempo map wins (positions and `TimeInfo::tempo`
    /// follow the map); `set_tempo` is for song-less racks, where it also sets the transport
    /// tempo (a playing song-less transport keeps its tick across the change).
    pub fn set_tempo(&mut self, tempo: atomik_core::Tempo) -> Result<(), EngineError> {
        self.push(0, ControlOp::Tempo(tempo))
    }

    /// Queues the master tune through the control queue: from the next batch boundary on,
    /// `TimeInfo::tune_cents` reports it (every batch carries one value, so a bounce is the same
    /// for any host block size). With `glide` the value moves there over about 20 ms, so held
    /// notes do not step; without it the value is in force at once (a fresh engine, a render).
    /// The engine starts at 0.
    pub fn set_master_tune(&mut self, cents: f32, glide: bool) -> Result<(), EngineError> {
        self.push(0, ControlOp::Tune { cents, glide })
    }

    /// Queues `edit` to be applied at the first batch boundary at or after timeline `frame`,
    /// so scripted edits land at the same batch for any host block size.
    ///
    /// Control messages (this, `insert_device`, `remove_device`, `submit_routing`,
    /// `submit_plan`, `set_tempo`) are applied strictly in queue order: a message also waits
    /// for every message queued before it, so frames should be non-decreasing. The untimed
    /// methods use frame 0 (the next boundary). Errors are those of the matching method.
    pub fn schedule(&mut self, frame: u64, edit: HotEdit) -> Result<(), EngineError> {
        match edit {
            HotEdit::Insert { id, device, params } => self.insert_at(frame, id, device, &params),
            HotEdit::Remove(id) => self.remove_at(frame, id),
            HotEdit::Routing(plan) => self.routing_at(frame, plan),
            HotEdit::Param {
                device,
                param,
                plain,
            } => self.push(
                frame,
                ControlOp::Param(ParamMessage {
                    device,
                    param,
                    plain,
                }),
            ),
            HotEdit::Samples { device, map } => self.samples_at(frame, device, map),
            HotEdit::PlayerData { device, data } => self.player_data_at(frame, device, data),
            HotEdit::Tempo(tempo) => self.push(frame, ControlOp::Tempo(tempo)),
            HotEdit::Song(song) => self.song_at(frame, song),
            HotEdit::Transport(command) => self.push(frame, ControlOp::Transport(command)),
        }
    }

    /// Number of device slots in the engine's arena.
    pub fn arena_capacity(&self) -> usize {
        self.mirror.capacity
    }

    /// True if `id` is in the arena, in queue order (inserted and not removed since).
    pub fn contains_device(&self, id: DeviceInstanceId) -> bool {
        self.mirror.devices.contains_key(&id)
    }

    /// Queues a compiled song snapshot. It replaces the current one at the
    /// next batch boundary in control-queue order; the old snapshot comes back through
    /// `collect_garbage`. Playback position and transport state are kept across swaps; notes
    /// sounding from the old snapshot are released (AllOff on affected devices) if needed.
    ///
    /// Notes held by a track whose content (device input and events) is unchanged keep
    /// sounding; held notes of removed or changed tracks get explicit `Off`s at the swap. If
    /// the tempo map differs, the musical position (tick) is kept. While a song is loaded its
    /// tempo map drives the transport and `TimeInfo::tempo`; `set_tempo` then only matters
    /// for song-less racks. There is no "unload": submit an empty song instead.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`]; the snapshot is then dropped here.
    pub fn submit_song(&mut self, song: crate::SongSnapshot) -> Result<(), EngineError> {
        self.song_at(0, song)
    }

    /// Frames of streamed audio the engine could not deliver in time so far: the audio thread counts one per segment read of a streamed region that found
    /// its data missing (the missing frames play as silence). Never blocks.
    pub fn stream_underruns(&self) -> u64 {
        self.streams.stats.underruns.load(Ordering::Relaxed)
    }

    /// Starts previewing `stream` (a file at `source_rate` Hz) at linear `gain`, live only.
    /// Replaces a preview that is playing (it fades out over 5 ms).
    ///
    /// # Errors
    /// [`EngineError::QueueFull`]; the stream is then dropped here (its feed sees it abandoned).
    pub fn preview_start(
        &mut self,
        stream: crate::PreviewStream,
        gain: f32,
    ) -> Result<(), EngineError> {
        let gain = if gain.is_finite() { gain.max(0.0) } else { 0.0 };
        // On a full queue the message, and the stream in it, is dropped here (see `push`).
        self.push(0, ControlOp::PreviewStart { stream, gain })
    }

    /// Stops the preview with a 5 ms fade-out. No-op when nothing plays.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`].
    pub fn preview_stop(&mut self) -> Result<(), EngineError> {
        self.push(0, ControlOp::PreviewStop)
    }

    /// True while a preview voice is sounding (including its fade-out). Never blocks.
    pub fn preview_active(&self) -> bool {
        self.preview_active.load(Ordering::Relaxed)
    }

    /// True when every streamed audio track has at least 1 s of audio buffered for the current
    /// transport position. The engine only requests data while it renders, so call it after
    /// some `Engine::render` calls; apps wait for it (briefly) before `Play` to avoid a start
    /// underrun. True when no song streams anything.
    pub fn stream_primed(&self) -> bool {
        self.streams.stats.is_primed()
    }

    /// Starts recording host input channels: from the next batch
    /// boundary the audio thread copies host input `channels[k]` (indices into the interface
    /// inputs; missing ones read as silence) into a preallocated lock-free ring of
    /// `capacity_frames` frames (rounded up to whole 64-frame batches), tagged with the engine
    /// frame of each batch. Drain it from any non-RT thread with [`RecordTap::drain`]; frames
    /// that did not fit are dropped and counted ([`RecordTap::overflow_frames`]). One tap at a
    /// time: a new tap replaces the previous one (which the engine hands back for dropping).
    ///
    /// # Errors
    /// [`EngineError::QueueFull`].
    pub fn record_tap(
        &mut self,
        channels: &[usize],
        capacity_frames: usize,
    ) -> Result<RecordTap, EngineError> {
        let (rt, tap) = crate::record::new_tap(channels, capacity_frames);
        self.push(0, ControlOp::Tap(Some(rt)))?;
        Ok(tap)
    }

    /// [`EngineHandle::record_tap`] with a gain and a clip-safe limiter per track: channel `k`
    /// reads host input `channels[k].host` and goes through strip `channels[k].strip` of
    /// `controls` before it is written (see [`TapControls`]). The caller keeps the `Arc` and
    /// changes the strips' settings while the tap runs.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`].
    pub fn record_tap_gained(
        &mut self,
        channels: &[TapChannel],
        capacity_frames: usize,
        controls: Arc<TapControls>,
    ) -> Result<RecordTap, EngineError> {
        let (rt, tap) = crate::record::new_gained_tap(channels, controls, capacity_frames);
        self.push(0, ControlOp::Tap(Some(rt)))?;
        Ok(tap)
    }

    /// Taps the notes device `id` receives from now on: every note on and off of its notes input
    /// is queued with its host frame (as MIDI messages, see [`atomik_device_sdk::MidiTap`]). The
    /// tap sits on the device's node, so a new plan (`submit_plan`) drops it. Applied at the next
    /// batch boundary; call [`Self::untap_notes`] to stop it.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`].
    pub fn tap_notes(
        &mut self,
        id: DeviceInstanceId,
    ) -> Result<std::sync::Arc<atomik_device_sdk::MidiTap>, EngineError> {
        let tap = std::sync::Arc::new(atomik_device_sdk::MidiTap::new());
        self.push(
            0,
            ControlOp::NoteTap {
                id,
                tap: Some(tap.clone()),
            },
        )?;
        Ok(tap)
    }

    /// Takes the note tap off device `id` (the engine retires it).
    ///
    /// # Errors
    /// [`EngineError::QueueFull`].
    pub fn untap_notes(&mut self, id: DeviceInstanceId) -> Result<(), EngineError> {
        self.push(0, ControlOp::NoteTap { id, tap: None })
    }

    /// Stops the record tap at the next batch boundary (the engine hands it back). Frames
    /// already in the ring stay drainable.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`].
    pub fn stop_record_tap(&mut self) -> Result<(), EngineError> {
        self.push(0, ControlOp::Tap(None))
    }

    /// Hands `map` (or, with `None`, no map) to arena device `id` through the control queue:
    /// applied at a batch boundary in queue order, so "insert, then
    /// samples" works and samples queued before a removal are fine. The device's
    /// `Device::swap_samples` runs on the audio thread (RT-safe, allocation-free); the map it
    /// hands back (the replaced one, or the input itself for a device that takes no samples)
    /// travels to this side with the other retired items and is dropped by
    /// [`EngineHandle::collect_garbage`], never on the audio thread. The same `Arc` may be sent
    /// to several devices.
    ///
    /// # Errors
    /// [`EngineError::UnknownDevice`] if `id` is not in the arena (in queue order), or
    /// [`EngineError::QueueFull`]. On error `map` is dropped here, on the non-RT thread.
    pub fn set_device_samples(
        &mut self,
        id: DeviceInstanceId,
        map: Option<SampleMapRef>,
    ) -> Result<(), EngineError> {
        self.samples_at(0, id, map)
    }

    fn samples_at(
        &mut self,
        frame: u64,
        id: DeviceInstanceId,
        map: Option<SampleMapRef>,
    ) -> Result<(), EngineError> {
        if !self.mirror.devices.contains_key(&id) {
            return Err(EngineError::UnknownDevice);
        }
        self.push(frame, ControlOp::Samples { id, map })
    }

    /// Hands pattern `data` (or, with `None`, no data) to arena device `id` through the control
    /// queue, exactly like [`EngineHandle::set_device_samples`]: applied at
    /// a batch boundary in queue order (insert, then data, then routing), `Device::
    /// swap_player_data` runs on the audio thread (RT-safe, allocation-free), and the data it
    /// hands back (the replaced data, or the input itself for a device that takes none) is
    /// dropped by [`EngineHandle::collect_garbage`], never on the audio thread. The same `Arc`
    /// may be sent to several devices.
    ///
    /// # Errors
    /// [`EngineError::UnknownDevice`] if `id` is not in the arena (in queue order), or
    /// [`EngineError::QueueFull`]. On error `data` is dropped here, on the non-RT thread.
    pub fn set_device_player_data(
        &mut self,
        id: DeviceInstanceId,
        data: Option<PlayerDataRef>,
    ) -> Result<(), EngineError> {
        self.player_data_at(0, id, data)
    }

    fn player_data_at(
        &mut self,
        frame: u64,
        id: DeviceInstanceId,
        data: Option<PlayerDataRef>,
    ) -> Result<(), EngineError> {
        if !self.mirror.devices.contains_key(&id) {
            return Err(EngineError::UnknownDevice);
        }
        self.push(frame, ControlOp::PlayerData { id, data })
    }

    fn song_at(&mut self, frame: u64, mut song: crate::SongSnapshot) -> Result<(), EngineError> {
        if self.control_tx.is_full() {
            return Err(EngineError::QueueFull);
        }
        self.streams.attach(&mut song, self.config.sample_rate.hz());
        self.push(frame, ControlOp::Song(Box::new(song)))
    }

    /// Queues a transport command, applied at the next batch boundary in
    /// control-queue order. Use [`EngineHandle::schedule`] with [`HotEdit::Transport`] for a
    /// frame-exact command. Without a song the transport still runs (at the engine tempo),
    /// which drives the metronome.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`].
    pub fn transport(&mut self, command: crate::TransportCommand) -> Result<(), EngineError> {
        // A lane of its own: it keeps its place behind the control messages queued before it,
        // but a parked retirement cannot hold it back.
        self.transport_tx
            .push(TransportMsg {
                after: self.control_sent,
                command,
            })
            .map_err(|_| EngineError::QueueFull)
    }

    /// The engine's lock-free meter bank. Meter endpoints with
    /// `Topology::add_meter`; values appear once a routing that meters them is live.
    pub fn meters(&self) -> std::sync::Arc<crate::Meters> {
        Arc::clone(&self.meters)
    }

    /// The values device `id` last published through `Device::meter_values`, see
    /// [`Meters::device_values`]: `(count, publish sequence)`, `None` for devices that do not
    /// publish. Never blocks the engine.
    pub fn device_meter_values(
        &self,
        id: crate::DeviceInstanceId,
        out: &mut [f32],
    ) -> Option<(usize, u64)> {
        self.meters.device_values(id, out)
    }

    /// The scope tap of device `id` (see [`Meters::scope_tap`]); `None` for a device without one.
    pub fn device_scope_tap(
        &self,
        id: crate::DeviceInstanceId,
    ) -> Option<std::sync::Arc<atomik_device_sdk::ScopeTap>> {
        self.meters.scope_tap(id)
    }

    /// The MIDI taps of the devices in the engine (see [`Meters::midi_taps`]).
    pub fn device_midi_taps(
        &self,
    ) -> Vec<(
        crate::DeviceInstanceId,
        std::sync::Arc<atomik_device_sdk::MidiTap>,
    )> {
        self.meters.midi_taps()
    }

    /// The latest known `Device::latency_frames()` of arena device `id`:
    /// read on this side when the device is inserted (after `prepare` and its params), then
    /// re-published by the engine at every batch boundary, after params and automation. So a
    /// param that changes a device's latency (such as a channel's `latency.adjust`) shows up
    /// here once the engine has applied it. `None` if `id` is not in the arena (queue order).
    pub fn device_latency(&self, id: DeviceInstanceId) -> Option<u32> {
        let &(slot, _) = self.mirror.devices.get(&id)?;
        Some(self.latencies.get(slot))
    }

    /// Counter of device latency changes published by the engine. When it moves,
    /// call [`EngineHandle::fill_latencies`] on the topology and resubmit its routing.
    pub fn latency_epoch(&self) -> u64 {
        self.latencies.epoch()
    }

    /// Copies [`EngineHandle::device_latency`] into every node of `topology` that is in the
    /// arena; call it before `Topology::compile`.
    pub fn fill_latencies(&self, topology: &mut crate::Topology) {
        let ids: Vec<_> = topology.node_ids().collect();
        for id in ids {
            if let Some(frames) = self.device_latency(id) {
                let _ = topology.set_latency(id, frames);
            }
        }
    }

    /// The latest published transport state (lock-free; never blocks the engine). Published
    /// at the start of every batch, after that boundary's control messages; the default value
    /// before the first batch.
    pub fn transport_state(&self) -> crate::TransportState {
        self.transport.read()
    }

    /// The MIDI clock out of the song player: set what it sends with
    /// [`crate::SyncOut::configure`] and read its messages from [`crate::SyncOut::tap`]. Every
    /// message is stamped with the engine host frame it belongs to, like a device's MIDI tap.
    pub fn sync_out(&self) -> Arc<crate::SyncOut> {
        Arc::clone(&self.sync)
    }

    /// The cell an external clock input writes. While it is [followed](crate::ExternalClock::set_following)
    /// the player takes its tempo, start, stop and locates from it and ignores the transport
    /// commands.
    pub fn external_clock(&self) -> Arc<crate::ExternalClock> {
        Arc::clone(&self.external)
    }

    /// Names the automation lanes the song player must not apply (`(device, param)` pairs; an
    /// empty slice releases them all). A take that writes a lane holds it, so the value being
    /// moved is not overwritten by the lane's old points at every batch. Takes effect within a
    /// batch, needs no queue space and never blocks the audio thread.
    ///
    /// # Errors
    /// [`EngineError::QueueFull`] when more than [`crate::HOLD_CAPACITY`] lanes are named (nothing
    /// changes).
    pub fn hold_automation(
        &self,
        lanes: &[(DeviceInstanceId, atomik_core::ParamId)],
    ) -> Result<(), EngineError> {
        self.hold.set(lanes).ok_or(EngineError::QueueFull)
    }

    /// The live note producer. Returns `Some` exactly once.
    pub fn live_notes(&mut self) -> Option<crate::live::LiveNoteSender> {
        self.live_tx.take().map(|tx| LiveNoteSender {
            tx,
            dropped: Arc::clone(&self.dropped),
        })
    }

    /// The first device that panicked while rendering, if any (sticky; see [`Engine::fault`]).
    /// Cheap and lock-free, so a UI can poll it every frame.
    pub fn fault(&self) -> Option<DeviceInstanceId> {
        self.fault.get()
    }

    /// Note events dropped so far (per-batch overflow or undeliverable targets).
    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn push(&mut self, frame: u64, op: ControlOp) -> Result<(), EngineError> {
        // On failure the message (and anything it owns) is dropped here, on the non-RT thread.
        self.control_tx
            .push(Control { frame, op })
            .map_err(|_| EngineError::QueueFull)?;
        self.control_sent += 1;
        Ok(())
    }

    /// Batches the engine began with a retirement parked: its return queue was full, so the
    /// control queue waited for [`EngineHandle::collect_garbage`]. Transport commands still
    /// applied. A UI shows it as a stalled control path.
    pub fn stalled_batches(&self) -> u64 {
        self.stalled.load(Ordering::Relaxed)
    }

    fn insert_at(
        &mut self,
        frame: u64,
        id: DeviceInstanceId,
        mut device: Box<dyn atomik_device_sdk::Device>,
        params: &[(ParamId, f32)],
    ) -> Result<(), EngineError> {
        if id == Graph::INTERFACE || self.mirror.devices.contains_key(&id) {
            return Err(EngineError::DuplicateDevice);
        }
        let slot = self.mirror.peek_free().ok_or(EngineError::ArenaFull)?;
        if self.control_tx.is_full() {
            return Err(EngineError::QueueFull);
        }
        device.prepare(&PrepareInfo {
            sample_rate: self.config.sample_rate,
            seed: mix_seed(self.config.seed, id),
        });
        let descriptor = device.descriptor();
        for &(param, plain) in params {
            if let Some(d) = descriptor.params.get(param.0 as usize) {
                device.set_param(param, d.clamp(plain));
            }
        }
        let latency = device.latency_frames();
        if let Some(tap) = device.scope_tap() {
            self.meters.register_scope(id, tap);
        }
        if let Some(tap) = device.midi_tap() {
            self.meters.register_midi(id, tap);
        }
        let node = Box::new(Node::new(device));
        self.push(frame, ControlOp::Insert { id, slot, node })?;
        self.latencies.publish(slot, latency);
        self.mirror.take(slot);
        self.mirror.devices.insert(id, (slot, descriptor));
        Ok(())
    }

    fn remove_at(&mut self, frame: u64, id: DeviceInstanceId) -> Result<(), EngineError> {
        let &(slot, _) = self
            .mirror
            .devices
            .get(&id)
            .ok_or(EngineError::UnknownDevice)?;
        if self.mirror.routed.contains(&id) {
            return Err(EngineError::DeviceInUse);
        }
        self.push(frame, ControlOp::Remove(id))?;
        self.meters.unregister_scope(id);
        self.meters.unregister_midi(id);
        self.mirror.devices.remove(&id);
        self.mirror.free.insert(slot);
        Ok(())
    }

    fn routing_at(&mut self, frame: u64, mut plan: RoutingPlan) -> Result<(), EngineError> {
        for d in &mut plan.devices {
            let &(slot, descriptor) = self
                .mirror
                .devices
                .get(&d.id)
                .ok_or(EngineError::UnknownDevice)?;
            let same = std::ptr::eq(descriptor, d.descriptor)
                || (descriptor.type_id == d.descriptor.type_id
                    && descriptor.sockets.len() == d.descriptor.sockets.len());
            if !same {
                return Err(EngineError::TypeMismatch);
            }
            d.slot = slot;
        }
        let routed = plan.order.iter().copied().collect();
        self.push(frame, ControlOp::Routing(Box::new(plan)))?;
        self.mirror.routed = routed;
        Ok(())
    }
}

impl Engine {
    /// Renders `outputs.len()` host output channels from `inputs.len()` host input channels.
    /// All slices must have the same length, which may be anything including 0. Missing host
    /// channels read as silence; extra ones are ignored (inputs) or zero-filled (outputs).
    ///
    /// Devices always process exactly [`BATCH`] frames: an internal FIFO adds
    /// [`Engine::latency_frames`] of latency, which makes the output independent of the host
    /// block size.
    pub fn render(&mut self, inputs: &[&[f32]], outputs: &mut [&mut [f32]]) {
        let frames = inputs
            .iter()
            .map(|c| c.len())
            .chain(outputs.iter().map(|c| c.len()))
            .min()
            .unwrap_or(0);
        debug_assert!(
            inputs.iter().all(|c| c.len() == frames) && outputs.iter().all(|c| c.len() == frames),
            "render: every channel slice must have the same length"
        );
        self.host_frames += frames as u64;
        self.last_render_spins = 0;
        let mut done = 0;
        while done < frames {
            let n = (BATCH - self.fifo_pos).min(frames - done);
            let fifo = self.fifo_pos..self.fifo_pos + n;
            let host = done..done + n;
            for (ch, buf) in self.in_fifo.iter_mut().enumerate() {
                match inputs.get(ch) {
                    Some(src) => buf[fifo.clone()].copy_from_slice(&src[host.clone()]),
                    None => buf[fifo.clone()].fill(0.0),
                }
            }
            for (ch, dst) in outputs.iter_mut().enumerate() {
                match self.out_fifo.get(ch) {
                    Some(buf) => dst[host.clone()].copy_from_slice(&buf[fifo.clone()]),
                    None => dst[host.clone()].fill(0.0),
                }
            }
            self.fifo_pos += n;
            done += n;
            if self.fifo_pos == BATCH {
                self.run_batch();
                self.fifo_pos = 0;
            }
        }
        self.max_render_spins = self.max_render_spins.max(self.last_render_spins);
    }

    /// The first device that panicked while rendering, if any. Sticky: the device is silent
    /// from then on and the engine keeps rendering everything else; rebuild the engine to clear
    /// it. See [`EngineHandle::fault`] for the non-RT view.
    pub fn fault(&self) -> Option<DeviceInstanceId> {
        self.fault.get()
    }

    /// Total host frames rendered so far (the timeline position of the next `render` call's
    /// first input frame). Published with [`crate::FrameClock`] by audio hosts.
    pub fn host_frames(&self) -> u64 {
        self.host_frames
    }

    /// The tempo reported to devices for the next batch (changed by
    /// [`EngineHandle::set_tempo`] and scheduled tempo edits).
    pub fn tempo(&self) -> Tempo {
        self.tempo
    }

    /// Number of worker threads rendering alongside the audio thread (`WorkerConfig::threads`;
    /// 0 for [`new_engine`]).
    pub fn worker_count(&self) -> usize {
        self.pool.as_ref().map_or(0, Pool::len)
    }

    /// Busy-wait iterations the workers have performed while idle so far, counted when a
    /// worker parks (test hook; not a stable API).
    #[doc(hidden)]
    pub fn worker_idle_spins(&self) -> u64 {
        self.pool.as_ref().map_or(0, Pool::idle_spins)
    }

    /// Times a worker parked so far (diagnostics and linger tests; not a stable API).
    #[doc(hidden)]
    pub fn worker_parks(&self) -> u64 {
        self.pool.as_ref().map_or(0, Pool::parks)
    }

    /// Idle spins a worker performs before parking (`EngineOptions::worker_linger_spins`, or
    /// [`crate::WORKER_SPIN_LIMIT`] when that is 0); 0 without workers.
    pub fn worker_linger_spins(&self) -> u64 {
        self.pool.as_ref().map_or(0, Pool::linger)
    }

    /// Busy-wait iterations the audio thread performed at level barriers during the latest
    /// [`Engine::render`] call, waiting for workers to hand back their devices (xrun
    /// diagnostics). Always 0 without workers, and 0 for a call that completed no batch.
    ///
    /// A plain counter, not a time: one spin is one poll of the done queues plus one
    /// `spin_loop` hint (tens of nanoseconds, CPU-dependent). It is telemetry only and never
    /// influences rendering.
    pub fn last_render_barrier_spins(&self) -> u64 {
        self.last_render_spins
    }

    /// The highest [`Engine::last_render_barrier_spins`] over the `render` calls since the
    /// previous call of this method, which resets it to 0. Call it from the audio thread (it
    /// takes `&mut self`); never allocates.
    pub fn take_max_barrier_spins(&mut self) -> u64 {
        std::mem::take(&mut self.max_render_spins)
    }

    /// Fixed latency in frames: [`BATCH`] (FIFO). Device latency is not compensated in this slice.
    pub fn latency_frames(&self) -> u32 {
        BATCH as u32
    }

    /// Output channel count (for the offline helpers).
    pub fn output_channels(&self) -> usize {
        usize::from(self.config.output_channels)
    }

    /// Input channel count (for the offline helpers).
    pub fn input_channels(&self) -> usize {
        usize::from(self.config.input_channels)
    }

    /// Processes one batch: boundary work (control messages, params, the song player,
    /// notes), then the routing, then the metronome.
    fn run_batch(&mut self) {
        self.player.begin_batch();
        self.apply_control();
        self.apply_params();
        let bt = self.player.run(
            &mut self.arena,
            self.frame,
            self.config.sample_rate.hz(),
            self.tempo,
            &self.dropped,
        );
        self.arena.publish_latencies(&self.latencies);
        self.deliver_notes();
        let time = TimeInfo {
            frame: self.frame,
            sample_rate: self.config.sample_rate,
            tempo: bt.tempo,
            playing: self.player.time_playing(),
            tick: bt.tick,
            ticks_per_frame: bt.ticks_per_frame,
            loop_wrap: bt.loop_wrap,
            loop_ticks: bt.loop_ticks,
            time_signature: bt.time_signature,
            tune_cents: self.tune.next(),
        };
        match (&mut self.routing, &mut self.pool) {
            (Some(routing), None) => routing.process(
                time,
                &self.in_fifo,
                &mut self.out_fifo,
                &self.dropped,
                &mut self.arena,
                &self.meters,
                &self.fault,
            ),
            (Some(routing), Some(pool)) => {
                let spins = routing.process_parallel(
                    time,
                    &self.in_fifo,
                    &mut self.out_fifo,
                    &self.dropped,
                    pool,
                    &mut self.arena,
                    &self.meters,
                    &self.fault,
                );
                self.last_render_spins = self.last_render_spins.saturating_add(spins);
            }
            (None, _) => self.out_fifo.iter_mut().for_each(|b| b.fill(0.0)),
        }
        // The record tap reads the host inputs of this batch and, for a resampling channel, the
        // outputs the routing has just written (so it needs the whole batch to have run).
        if let Some(tap) = &mut self.tap {
            tap.capture(self.frame, &self.in_fifo, &self.arena);
        }
        self.arena.publish_device_values(&self.meters);
        if crate::timing::enabled() {
            self.arena.publish_cpu(&self.meters);
            self.meters
                .add_cpu_budget(crate::timing::batch_budget_ns(self.config.sample_rate.hz()));
        }
        let click_delay = self.routing.as_mut().and_then(|r| r.click_delay_mut());
        self.player.mix_metronome(&mut self.out_fifo, click_delay);
        self.preview.mix(&mut self.out_fifo);
        self.preview_active
            .store(self.preview.is_active(), Ordering::Relaxed);
        self.frame += BATCH as u64;
    }

    /// Applies the due control messages at a batch boundary, in FIFO order. Each message
    /// produces at most one item to hand back; while one is parked (its return queue was full)
    /// no further message is applied, so nothing is ever dropped on this thread.
    fn apply_control(&mut self) {
        if let Some(old) = self.pending_retire.take() {
            self.retire(old);
        }
        while self.pending_retire.is_none() {
            // Transport commands sent before the next control message go first.
            self.apply_transport_lane();
            // Streams the preview voice finished since the last boundary go back first, so
            // the voice's dead list is empty when a control message is applied.
            self.drain_preview();
            if self.pending_retire.is_some() {
                break;
            }
            match self.control_rx.peek() {
                Ok(c) if c.frame <= self.frame => {}
                _ => break,
            }
            let Ok(Control { op, .. }) = self.control_rx.pop() else {
                break;
            };
            self.apply(op);
            self.control_applied += 1;
        }
        // The rest of the lane: in order behind the control queue, or at once while a parked
        // retirement holds the queue, so Play and Stop never wait for the UI to collect garbage.
        self.apply_transport_lane();
        if self.pending_retire.is_some() {
            self.stalled.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Applies the transport lane's due messages: those queued behind no control message that
    /// is still waiting, or all of them while a retirement is parked.
    fn apply_transport_lane(&mut self) {
        loop {
            match self.transport_rx.peek() {
                Ok(m) if self.pending_retire.is_some() || m.after <= self.control_applied => {}
                _ => break,
            }
            let Ok(TransportMsg { command, .. }) = self.transport_rx.pop() else {
                break;
            };
            self.player.invalidate_lanes();
            let sr = self.config.sample_rate.hz();
            self.player.command(command, self.tempo, sr, &self.dropped);
        }
    }

    fn apply(&mut self, op: ControlOp) {
        // Any message may change a device or its parameters behind the automation lanes' back.
        self.player.invalidate_lanes();
        match op {
            ControlOp::Insert { id, slot, node } => {
                if let Some(rejected) = self.arena.insert(id, slot, node) {
                    self.retire(Garbage::Device(rejected));
                }
            }
            ControlOp::Remove(id) => {
                let slot = self.arena.slot_of(id);
                if let Some(node) = self.arena.remove(id) {
                    if let Some(slot) = slot {
                        self.meters.clear_device_values(slot);
                        self.meters.clear_cpu(slot);
                    }
                    self.retire(Garbage::Device(node));
                }
            }
            ControlOp::Routing(mut new) => {
                if let Some(old) = &mut self.routing {
                    old.uninstall(&mut self.arena);
                    new.adopt_delays(old);
                }
                new.install(&mut self.arena);
                self.meters.assign(&new.console.meter_keys);
                if let Some(old) = self.routing.replace(new) {
                    self.retire(Garbage::Retired(Retired::Routing(old)));
                }
            }
            ControlOp::Plan(mut swap) => {
                std::mem::swap(&mut self.arena, &mut swap.arena);
                std::mem::swap(&mut self.routing, &mut swap.routing);
                self.meters.clear_all_device_values();
                self.meters.clear_all_cpu();
                if let Some(routing) = &self.routing {
                    routing.install(&mut self.arena);
                    self.meters.assign(&routing.console.meter_keys);
                }
                // `swap` now carries the previous arena and routing.
                if swap.routing.is_none() && swap.arena.is_empty() && self.spare.is_none() {
                    self.spare = Some(swap);
                } else {
                    self.retire(Garbage::Retired(Retired::Plan(swap)));
                }
            }
            ControlOp::Param(msg) => self.arena.set_param(msg.device, msg.param, msg.plain),
            ControlOp::Tempo(tempo) => {
                let sr = self.config.sample_rate.hz();
                self.player.engine_tempo_changed(self.tempo, tempo, sr);
                self.tempo = tempo;
            }
            ControlOp::Tune { cents, glide } => self.tune.set(cents, glide),
            ControlOp::Song(song) => {
                let sr = self.config.sample_rate.hz();
                if let Some(old) = self.player.swap_song(song, self.tempo, sr, &self.dropped) {
                    self.retire(Garbage::Retired(Retired::Song(old)));
                }
            }
            ControlOp::Transport(command) => {
                let sr = self.config.sample_rate.hz();
                self.player.command(command, self.tempo, sr, &self.dropped);
            }
            ControlOp::Samples { id, map } => {
                // Control messages are applied at a batch boundary, before the routing runs:
                // every node is then home in its arena slot (workers borrow nodes only inside
                // `process_parallel` and hand them back before it returns), so the device is
                // always reachable here.
                if let Some(old) = self.arena.swap_samples(id, map) {
                    self.retire(Garbage::Retired(Retired::Samples(old)));
                }
            }
            ControlOp::PlayerData { id, data } => {
                // Same boundary argument as for `Samples` above.
                if let Some(old) = self.arena.swap_player_data(id, data) {
                    self.retire(Garbage::Retired(Retired::PlayerData(old)));
                }
            }
            ControlOp::PreviewStart { stream, gain } => {
                if let Some(cut) = self.preview.start(stream, gain) {
                    self.retire(Garbage::Retired(Retired::Preview(cut)));
                }
            }
            ControlOp::PreviewStop => {
                if let Some(cut) = self.preview.stop() {
                    self.retire(Garbage::Retired(Retired::Preview(cut)));
                }
            }
            ControlOp::NoteTap { id, tap } => {
                if let Some(old) = self.arena.set_note_tap(id, tap) {
                    self.retire(Garbage::Retired(Retired::NoteTap(old)));
                }
            }
            ControlOp::Tap(new) => {
                if let Some(old) = std::mem::replace(&mut self.tap, new) {
                    self.retire(Garbage::Retired(Retired::Tap(old)));
                }
            }
        }
    }

    /// Retires the preview streams that ended, one by one, until the return queue is full.
    fn drain_preview(&mut self) {
        while self.pending_retire.is_none() {
            let Some(stream) = self.preview.take_dead() else {
                break;
            };
            self.retire(Garbage::Retired(Retired::Preview(stream)));
        }
    }

    /// Pushes an item to its return queue, or parks it if the queue is full (never drops it).
    fn retire(&mut self, item: Garbage) {
        let parked = match item {
            Garbage::Retired(r) => match self.retire_tx.push(r) {
                Err(PushError::Full(r)) => Some(Garbage::Retired(r)),
                Ok(()) => None,
            },
            Garbage::Device(n) => match self.removed_tx.push(n) {
                Err(PushError::Full(n)) => Some(Garbage::Device(n)),
                Ok(()) => None,
            },
        };
        if parked.is_some() {
            self.pending_retire = parked;
        }
    }

    /// Applies every queued param message to the arena devices.
    fn apply_params(&mut self) {
        while let Ok(msg) = self.param_rx.pop() {
            self.player.invalidate_lanes();
            self.arena.set_param(msg.device, msg.param, msg.plain);
        }
    }

    /// Moves the notes due in the current batch from the song player and both note queues
    /// into the routed arena devices, merged in frame order. A queued frame in the past counts
    /// as the batch start (offset 0); on equal effective frames song events come first, then
    /// scheduled notes, then live notes. Deterministic for given queue contents.
    fn deliver_notes(&mut self) {
        let start = self.frame;
        let end = start + BATCH as u64;
        let due = |rx: &Consumer<ScheduledNote>| {
            rx.peek()
                .ok()
                .filter(|n| n.frame < end)
                .map(|n| n.frame.max(start))
        };
        loop {
            let song = self
                .player
                .events
                .get(self.player.next_event)
                .map(|e| start + u64::from(e.offset));
            let (sched, live) = (due(&self.note_rx), due(&self.live_rx));
            let take_song =
                song.is_some_and(|f| sched.is_none_or(|s| f <= s) && live.is_none_or(|l| f <= l));
            let (device, socket, offset, kind) = if take_song {
                let e = self.player.events[self.player.next_event];
                self.player.next_event += 1;
                (e.device, e.socket, e.offset, e.kind)
            } else {
                let rx = match (sched, live) {
                    (Some(s), Some(l)) if l < s => &mut self.live_rx,
                    (Some(_), _) => &mut self.note_rx,
                    (None, Some(_)) => &mut self.live_rx,
                    (None, None) => break,
                };
                let Ok(note) = rx.pop() else {
                    break;
                };
                let offset = note.frame.saturating_sub(start) as u16;
                (note.device, note.socket, offset, note.kind)
            };
            if !self.arena.deliver_note(device, socket, offset, kind) {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
