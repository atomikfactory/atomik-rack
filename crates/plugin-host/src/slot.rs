//! The shared state between a plugin device on the engine side and the manager on the main
//! thread: the slot (atomics), the message ring, the restart hand-shake and the reaper queue.
//!
//! Everything the audio thread touches here is wait-free: relaxed or acquire/release atomics and
//! `rtrb` rings. The only locks guard structures that are used off the audio thread (the
//! reaper queue, the manager's receiving end) or are taken with `try_lock` and never waited on
//! (the host message sender, which several plugin threads share).

use crate::backend::PluginProcessor;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// Capacity of a slot's message ring.
pub const MESSAGE_CAPACITY: usize = 256;

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
    /// The plugin's param table changed.
    RescanParams,
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
}

/// The sender side of the message ring, shared by every thread that may call a host callback.
/// Posting never blocks: when the ring is full or another thread is posting at that instant,
/// the message is dropped and counted.
#[derive(Debug, Clone)]
pub struct HostMessenger {
    inner: Arc<MessengerInner>,
}

#[derive(Debug)]
struct MessengerInner {
    producer: Mutex<Producer<PluginMessage>>,
    dropped: AtomicU64,
}

impl HostMessenger {
    /// Posts a message. Returns false when it was dropped.
    pub fn post(&self, message: PluginMessage) -> bool {
        let accepted = match self.inner.producer.try_lock() {
            Ok(mut producer) => producer.push(message).is_ok(),
            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner().push(message).is_ok(),
            Err(std::sync::TryLockError::WouldBlock) => false,
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
}

impl DeviceState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Sleeping,
            2 => Self::Stopped,
            3 => Self::Failed,
            4 => Self::Restarting,
            _ => Self::Running,
        }
    }

    /// The meter value (0 to 4).
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

/// The main-thread end of an instance.
pub struct ManagerEnd {
    slot: PluginSlot,
    messages: Mutex<Consumer<PluginMessage>>,
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

    /// The next message, if any.
    pub fn try_recv(&self) -> Option<PluginMessage> {
        self.messages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
            .ok()
    }

    /// Messages dropped because the ring was full or busy.
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
            dropped: AtomicU64::new(0),
        }),
    };
    // `std`'s mutex on Apple targets boxes its pthread mutex lazily on the first lock. `post`
    // runs on the audio thread (a plugin may call back from `process`), so take the lock once
    // here, off the audio thread, to make that allocation happen now (HI-1).
    drop(messenger.inner.producer.lock());
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
            messages: Mutex::new(msg_rx),
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
