//! The shared state between a plugin device on the engine side and the manager on the main
//! thread: the slot (atomics), the message ring, the restart hand-shake and the reaper queue.
//!
//! Everything the audio thread touches here is wait-free: relaxed or acquire/release atomics and
//! `rtrb` rings. The only locks guard structures that are used off the audio thread (the
//! reaper queue, the manager's receiving end, the message queue) or are taken with `try_lock`
//! and never waited on (the ring's producer, which several plugin threads share).
//!
//! What a plugin tells the app reaches [`ManagerEnd::try_recv`] by three roads, and every
//! message other than a parameter `Change` and a log line is impossible to lose:
//!
//! * **Flags.** `StateDirty`, `LatencyChanged`, `RescanParams`, `RequestRestart` and
//!   `RequestCallback` carry nothing and mean the same said once or twice: posting one sets a
//!   bit, from any thread, always accepted. `try_recv` hands the set flags out first, in a fixed
//!   order, each once.
//! * **The queue** takes every other message from a thread that is not inside a plugin's
//!   `process` (the plugin's UI thread, the main thread, the sandbox's reader and watcher). It is
//!   a list under a mutex that those threads lock, never `try_lock`, so two of them posting at
//!   the same instant lose nothing. A `Change` that follows a `Change` of the same parameter
//!   replaces it (a drag holds one message). From [`QUEUE_SOFT_CAP`] messages on, a `Change` and
//!   a log line are dropped and counted, everything else is still queued; at [`QUEUE_HARD_CAP`]
//!   everything is dropped and counted, so that a rogue plugin cannot grow the list without
//!   bound. The audio thread never touches this mutex.
//! * **The ring** takes the messages posted inside a plugin's `process` (a thread holding an
//!   [`AudioThreadGuard`](crate::AudioThreadGuard)), wait-free. A full ring, or a ring another
//!   thread is pushing to at that instant, drops the message and counts it; a `Change` is only
//!   pushed while more than [`CHANGE_RESERVE`] slots are free, so a flood of changes cannot take
//!   the room of a gesture's begin or end, a log line or a failure.
//!
//! Every message of the ring and of the queue carries the number it was stamped with when it was
//! posted, and `try_recv` hands out the older of the two heads: one order, the posting order.

use crate::backend::PluginProcessor;
use crate::threads::is_audio_thread;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// Capacity of a slot's message ring (the messages posted inside a plugin's `process`).
pub const MESSAGE_CAPACITY: usize = 256;

/// Free slots of the ring that a parameter `Change` never takes: a `Change` posted from the audio
/// thread is pushed only while more than this many are free, so the room is left for a gesture's
/// begin and end, a log line or a failure.
pub const CHANGE_RESERVE: usize = 64;

/// How many messages the queue of the threads outside `process` holds before it refuses a
/// parameter `Change` and a log line (every other message is still queued).
pub const QUEUE_SOFT_CAP: usize = 1024;

/// How many messages the queue holds at most: beyond it every message is dropped and counted.
pub const QUEUE_HARD_CAP: usize = 65_536;

/// Identifies one plugin instance for the manager and the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(pub u64);

/// The phase of a GUI parameter gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamPhase {
    /// The user grabbed the control.
    Begin,
    /// The value moved.
    Change,
    /// The user let go.
    End,
}

/// Severity of a plugin log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Debug detail.
    Debug,
    /// Information.
    Info,
    /// Something is odd.
    Warning,
    /// Something failed.
    Error,
}

/// How long a blocking call on a plugin's process may take before it is recorded as a stall of
/// the UI thread (a [`PluginMessage::Stall`]). Faster calls leave no trace.
pub const STALL_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(50);

/// How a blocking call on a plugin's process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallOutcome {
    /// The plugin answered, late.
    Answered,
    /// The wait ran out.
    TimedOut,
    /// The plugin's process ended during the wait.
    ChildGone,
}

impl StallOutcome {
    /// The words of the log line.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::TimedOut => "timed out",
            Self::ChildGone => "child gone",
        }
    }
}

/// What a plugin or a device tells the app. Drained by the manager on the main thread.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginMessage {
    /// A parameter edit made inside the plugin's own window.
    ParamEdit {
        /// The plugin's param id.
        id: u32,
        /// The new plain value.
        value: f64,
        /// Gesture phase.
        phase: ParamPhase,
    },
    /// The plugin's state changed (`state.mark_dirty`).
    StateDirty,
    /// The plugin's latency changed.
    LatencyChanged,
    /// The plugin's param table changed in what the host caches: the device restarts.
    RescanParams,
    /// The plugin renamed params (titles, groups) and nothing else changed: the new layout, to
    /// show without a restart.
    ParamTitles(Box<atomik_device_sdk::PluginLayout>),
    /// The plugin or the device wants a restart (a new sample rate, a changed layout).
    RequestRestart,
    /// The plugin asked for a main-thread callback.
    RequestCallback,
    /// The plugin wants its editor resized.
    GuiResize {
        /// Width in pixels.
        w: u32,
        /// Height in pixels.
        h: u32,
    },
    /// The editor window of the plugin's own process moved or changed size: its outer frame on
    /// the screen, in physical pixels.
    GuiFrame {
        /// Left edge.
        x: i32,
        /// Top edge.
        y: i32,
        /// Outer width.
        w: u32,
        /// Outer height.
        h: u32,
    },
    /// The plugin closed its editor.
    GuiClosed,
    /// The plugin asks to show its editor.
    GuiRequestShow,
    /// The plugin asks to hide its editor.
    GuiRequestHide,
    /// A plugin log line.
    Log {
        /// Severity.
        level: LogLevel,
        /// The text.
        text: String,
    },
    /// The device failed and now outputs silence or the dry signal.
    Failed {
        /// Why.
        reason: &'static str,
    },
    /// The plugin did not take some batches (see [`crate::ProcessStatus::Refused`]): the device
    /// plays on and says so, first at once, then now and then.
    Refused {
        /// How many batches the plugin has refused so far.
        batches: u64,
    },
    /// The plugin's process ended (sandbox only).
    ProcessExited {
        /// The exit code or signal and the last lines the process wrote to stderr.
        reason: String,
    },
    /// A call the main thread made on the plugin's process blocked it for at least
    /// [`STALL_THRESHOLD`] (sandbox only).
    Stall {
        /// The name of the call.
        call: String,
        /// How long the main thread waited, in milliseconds.
        ms: u64,
        /// How the wait ended.
        outcome: StallOutcome,
    },
}

/// Takes `m`'s lock once, so that its first real lock cannot allocate.
///
/// `std`'s mutex on Apple targets boxes its pthread mutex lazily on the first lock. A mutex that
/// the audio thread may lock (a host callback a plugin makes from `process`) must be warmed here,
/// off the audio thread, when it is created, or that first lock allocates inside the render path
/// (HI-1). Use [`warmed`] where the mutex is built, or `warm` on one already built.
pub fn warm<T>(m: &Mutex<T>) {
    drop(m.lock());
}

/// `m`, with its lock already taken once: see [`warm`].
pub fn warmed<T>(m: Mutex<T>) -> Mutex<T> {
    warm(&m);
    m
}

/// A message with the number it was stamped with when it was posted.
#[derive(Debug)]
struct Stamped {
    seq: u64,
    message: PluginMessage,
}

/// The bit of a message that is a flag (no data, saying it twice is saying it once).
fn flag_bit(message: &PluginMessage) -> Option<u8> {
    match message {
        PluginMessage::StateDirty => Some(1),
        PluginMessage::LatencyChanged => Some(1 << 1),
        PluginMessage::RescanParams => Some(1 << 2),
        PluginMessage::RequestRestart => Some(1 << 3),
        PluginMessage::RequestCallback => Some(1 << 4),
        _ => None,
    }
}

/// The message of a flag bit; the bits come out lowest first.
fn flag_message(bit: u8) -> Option<PluginMessage> {
    Some(match bit {
        1 => PluginMessage::StateDirty,
        2 => PluginMessage::LatencyChanged,
        4 => PluginMessage::RescanParams,
        8 => PluginMessage::RequestRestart,
        16 => PluginMessage::RequestCallback,
        _ => return None,
    })
}

fn is_change(message: &PluginMessage) -> bool {
    matches!(
        message,
        PluginMessage::ParamEdit {
            phase: ParamPhase::Change,
            ..
        }
    )
}

/// The sender side of the message channel, shared by every thread that may call a host callback.
/// Posting never blocks the audio thread: see the module doc for what each road does with a
/// message it cannot take.
#[derive(Debug, Clone)]
pub struct HostMessenger {
    inner: Arc<MessengerInner>,
}

#[derive(Debug)]
struct MessengerInner {
    /// The ring of the audio thread's messages (`try_lock`, wait-free).
    producer: Mutex<Producer<Stamped>>,
    /// The messages of every other thread. Never locked on the audio path.
    queue: Mutex<VecDeque<Stamped>>,
    /// The flags posted and not yet taken.
    flags: AtomicU8,
    /// The next number to stamp a message with.
    seq: AtomicU64,
    dropped: AtomicU64,
}

impl MessengerInner {
    fn stamp(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    /// The audio thread's road: wait-free, refuses when the ring is full or busy.
    fn post_ring(&self, message: PluginMessage) -> bool {
        let push = |producer: &mut Producer<Stamped>, message: PluginMessage| {
            if is_change(&message) && producer.slots() <= CHANGE_RESERVE {
                return false;
            }
            let seq = self.stamp();
            producer.push(Stamped { seq, message }).is_ok()
        };
        match self.producer.try_lock() {
            Ok(mut producer) => push(&mut producer, message),
            Err(std::sync::TryLockError::Poisoned(p)) => push(&mut p.into_inner(), message),
            Err(std::sync::TryLockError::WouldBlock) => false,
        }
    }

    /// The road of every other thread: waits for the lock, which nobody holds for long.
    fn post_queue(&self, message: PluginMessage) -> bool {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if let PluginMessage::ParamEdit {
            id: new_id,
            value: new_value,
            phase: ParamPhase::Change,
        } = &message
            && let Some(tail) = queue.back_mut()
            && let PluginMessage::ParamEdit {
                id,
                value,
                phase: ParamPhase::Change,
            } = &mut tail.message
            && id == new_id
            // Nobody posted anything since the tail, on either road: the newer value takes its
            // place and keeps the order of the messages around it.
            && self
                .seq
                .compare_exchange(
                    tail.seq + 1,
                    tail.seq + 2,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
        {
            *value = *new_value;
            tail.seq += 1;
            return true;
        }
        let len = queue.len();
        let sheddable = is_change(&message) || matches!(message, PluginMessage::Log { .. });
        if len >= QUEUE_HARD_CAP || (len >= QUEUE_SOFT_CAP && sheddable) {
            return false;
        }
        let seq = self.stamp();
        queue.push_back(Stamped { seq, message });
        true
    }
}

impl HostMessenger {
    /// Posts a message. Returns false when it was dropped (and counted): outside a plugin's
    /// `process` only a parameter `Change` or a log line can be; inside it, anything the ring has
    /// no room for.
    pub fn post(&self, message: PluginMessage) -> bool {
        if let Some(bit) = flag_bit(&message) {
            self.inner.flags.fetch_or(bit, Ordering::Release);
            return true;
        }
        let accepted = if is_audio_thread() {
            self.inner.post_ring(message)
        } else {
            self.inner.post_queue(message)
        };
        if !accepted {
            self.inner.dropped.fetch_add(1, Ordering::Relaxed);
        }
        accepted
    }

    /// Messages dropped so far.
    pub fn dropped(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }
}

/// The state the device publishes to the manager and the meters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    /// Processing.
    Running = 0,
    /// Skipping `process` until something needs it.
    Sleeping = 1,
    /// Powered off (bypassed with processing stopped).
    Stopped = 2,
    /// The plugin failed; the device outputs silence or the dry signal.
    Failed = 3,
    /// The processor is with the manager for a restart.
    Restarting = 4,
    /// The plugin's process is gone (sandbox); the device outputs silence or the dry signal
    /// until RELOAD creates a new instance.
    Crashed = 5,
}

impl DeviceState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Sleeping,
            2 => Self::Stopped,
            3 => Self::Failed,
            4 => Self::Restarting,
            5 => Self::Crashed,
            _ => Self::Running,
        }
    }

    /// The meter value (0 to 5).
    pub fn as_f32(self) -> f32 {
        f32::from(self as u8)
    }
}

#[derive(Debug)]
struct SlotInner {
    id: InstanceId,
    stop: AtomicBool,
    latency: AtomicU32,
    state: AtomicU8,
    pinned: AtomicBool,
    powered: AtomicBool,
    messenger: HostMessenger,
}

/// The shared handle of one plugin instance: cheap to clone, safe to use from any thread.
#[derive(Debug, Clone)]
pub struct PluginSlot {
    inner: Arc<SlotInner>,
}

impl PluginSlot {
    /// The instance id.
    pub fn id(&self) -> InstanceId {
        self.inner.id
    }

    /// The sender of this instance's message ring.
    pub fn messenger(&self) -> HostMessenger {
        self.inner.messenger.clone()
    }

    /// Main thread: asks the device to hand its processor back at the next batch boundary.
    pub fn request_stop(&self) {
        self.inner.stop.store(true, Ordering::Release);
    }

    /// Whether a stop is requested (read by the device on the audio thread).
    pub fn stop_requested(&self) -> bool {
        self.inner.stop.load(Ordering::Acquire)
    }

    /// Main thread: clears the stop request. Done before the new processor is sent, so the
    /// device never sees "stop" together with a fresh processor.
    pub fn resume_run(&self) {
        self.inner.stop.store(false, Ordering::Release);
    }

    /// Sets the latency the plugin reports, in frames, clamped to one second at `sample_rate`.
    /// Returns true when the value was clamped.
    pub fn set_latency(&self, frames: u32, sample_rate: f64) -> bool {
        let max = sample_rate.max(1.0) as u32;
        let clamped = frames > max;
        self.inner.latency.store(frames.min(max), Ordering::Relaxed);
        clamped
    }

    /// The latency in frames (what `Device::latency_frames` returns).
    pub fn latency(&self) -> u32 {
        self.inner.latency.load(Ordering::Relaxed)
    }

    /// RUN ON AUDIO THREAD: the engine keeps the device on the audio callback thread.
    pub fn set_pinned(&self, pinned: bool) {
        self.inner.pinned.store(pinned, Ordering::Relaxed);
    }

    /// Whether the device is pinned to the audio thread.
    pub fn pinned(&self) -> bool {
        self.inner.pinned.load(Ordering::Relaxed)
    }

    /// POWER: false bypasses and then stops processing.
    pub fn set_powered(&self, powered: bool) {
        self.inner.powered.store(powered, Ordering::Relaxed);
    }

    /// Whether the device is powered on.
    pub fn powered(&self) -> bool {
        self.inner.powered.load(Ordering::Relaxed)
    }

    /// The state the device last published.
    pub fn device_state(&self) -> DeviceState {
        DeviceState::from_u8(self.inner.state.load(Ordering::Relaxed))
    }

    pub(crate) fn publish_state(&self, state: DeviceState) {
        self.inner.state.store(state as u8, Ordering::Relaxed);
    }
}

type ProcBox = Box<dyn PluginProcessor>;

/// The engine-side end of an instance: the slot and the hand-shake rings.
pub struct DeviceEnd {
    pub(crate) slot: PluginSlot,
    pub(crate) to_manager: Producer<ProcBox>,
    pub(crate) from_manager: Consumer<ProcBox>,
}

impl DeviceEnd {
    /// The slot.
    pub fn slot(&self) -> &PluginSlot {
        &self.slot
    }
}

/// What `try_recv` reads the ring and the flags from.
struct Inbox {
    ring: Consumer<Stamped>,
    /// Flags taken from the atomic and not yet handed out.
    flags: u8,
}

/// The main-thread end of an instance.
pub struct ManagerEnd {
    slot: PluginSlot,
    messages: Mutex<Inbox>,
    from_device: Consumer<ProcBox>,
    to_device: Producer<ProcBox>,
}

impl ManagerEnd {
    /// The slot.
    pub fn slot(&self) -> &PluginSlot {
        &self.slot
    }

    /// The sender plugins use to reach the host.
    pub fn messenger(&self) -> HostMessenger {
        self.slot.messenger()
    }

    /// The next message, if any: the flags that are set first (one each, in a fixed order), then
    /// the messages of the ring and of the queue in the order they were posted.
    pub fn try_recv(&self) -> Option<PluginMessage> {
        let inner = &self.slot.inner.messenger.inner;
        let mut inbox = self.messages.lock().unwrap_or_else(PoisonError::into_inner);
        if inbox.flags == 0 {
            inbox.flags = inner.flags.swap(0, Ordering::AcqRel);
        }
        if inbox.flags != 0 {
            let bit = 1u8 << inbox.flags.trailing_zeros();
            inbox.flags &= !bit;
            return flag_message(bit);
        }
        // The queue is locked before the ring is looked at: a message of the queue posted after
        // a message of the ring cannot appear while the two heads are compared, so the older one
        // is never overtaken.
        let mut queue = inner.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let from_queue = match (inbox.ring.peek().ok(), queue.front()) {
            (None, None) => return None,
            (Some(_), None) => false,
            (None, Some(_)) => true,
            (Some(ring), Some(queued)) => queued.seq < ring.seq,
        };
        if from_queue {
            queue.pop_front().map(|s| s.message)
        } else {
            inbox.ring.pop().ok().map(|s| s.message)
        }
    }

    /// Messages dropped: a parameter `Change` or a log line the queue had no room for, and what
    /// the ring refused because it was full or busy.
    pub fn dropped_messages(&self) -> u64 {
        self.slot.inner.messenger.dropped()
    }

    /// Starts a restart: the device hands its processor back at the next batch boundary.
    pub fn request_stop(&mut self) {
        self.slot.request_stop();
    }

    /// The processor the device handed back, if it has.
    pub fn take_processor(&mut self) -> Option<ProcBox> {
        self.from_device.pop().ok()
    }

    /// Starts a new device on the same instance (an engine restart that keeps the plugin):
    /// fresh hand-shake rings, the same slot and message ring. Whatever the old rings
    /// still held is returned for the manager to dispose of.
    pub fn new_device_end(&mut self) -> (DeviceEnd, Vec<ProcBox>) {
        let mut leftovers = Vec::new();
        while let Ok(p) = self.from_device.pop() {
            leftovers.push(p);
        }
        self.slot.resume_run();
        let (to_manager, from_device) = RingBuffer::new(2);
        let (to_device, from_manager) = RingBuffer::new(2);
        self.from_device = from_device;
        self.to_device = to_device;
        (
            DeviceEnd {
                slot: self.slot.clone(),
                to_manager,
                from_manager,
            },
            leftovers,
        )
    }

    /// Ends a restart: sends a (re-activated) processor to the device, which resumes at its
    /// next batch. Gives the processor back if the ring is full.
    ///
    /// # Errors
    /// The processor itself, when it could not be sent.
    pub fn resume(&mut self, processor: ProcBox) -> Result<(), ProcBox> {
        self.slot.resume_run();
        match self.to_device.push(processor) {
            Ok(()) => Ok(()),
            Err(PushError::Full(p)) => Err(p),
        }
    }
}

/// Creates the two ends of one plugin instance.
pub fn plugin_channel(id: InstanceId) -> (DeviceEnd, ManagerEnd) {
    let (msg_tx, msg_rx) = RingBuffer::new(MESSAGE_CAPACITY);
    let messenger = HostMessenger {
        inner: Arc::new(MessengerInner {
            producer: Mutex::new(msg_tx),
            queue: Mutex::new(VecDeque::new()),
            flags: AtomicU8::new(0),
            seq: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }),
    };
    // `post` runs on the audio thread (a plugin may call back from `process`): see `warm`. The
    // queue's mutex is never locked there, so it needs no warming.
    warm(&messenger.inner.producer);
    let slot = PluginSlot {
        inner: Arc::new(SlotInner {
            id,
            stop: AtomicBool::new(false),
            latency: AtomicU32::new(0),
            state: AtomicU8::new(DeviceState::Running as u8),
            pinned: AtomicBool::new(false),
            powered: AtomicBool::new(true),
            messenger,
        }),
    };
    let (to_manager, from_device) = RingBuffer::new(2);
    let (to_device, from_manager) = RingBuffer::new(2);
    (
        DeviceEnd {
            slot: slot.clone(),
            to_manager,
            from_manager,
        },
        ManagerEnd {
            slot,
            messages: Mutex::new(Inbox {
                ring: msg_rx,
                flags: 0,
            }),
            from_device,
            to_device,
        },
    )
}

/// What a dropped device leaves for the main thread: the slot and the processor it still held.
/// The main thread deactivates and destroys the plugin (CLAP and VST3 both require it).
pub struct ReapItem {
    /// The instance.
    pub slot: PluginSlot,
    /// The processor, if the device still owned it.
    pub processor: Option<Box<dyn PluginProcessor>>,
}

/// Where dropped devices post their processors. Posting happens on whatever non-audio thread
/// drops the retired device, so a plain mutex is fine; the main thread drains it in
/// `PluginManager::poll`.
#[derive(Clone, Default)]
pub struct ReaperQueue {
    items: Arc<Mutex<Vec<ReapItem>>>,
}

impl ReaperQueue {
    /// An empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an item.
    pub fn post(&self, item: ReapItem) {
        self.items
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(item);
    }

    /// Takes every item.
    pub fn drain(&self) -> Vec<ReapItem> {
        std::mem::take(&mut *self.items.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
