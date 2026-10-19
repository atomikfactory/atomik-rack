//! What the test plugin saw, for the host tests: per-instance probes, process-wide counters and
//! commands that make a plugin call the host from the calling thread. The three exported test
//! hooks (`atomik_test_probe`, `atomik_test_command`, `atomik_test_counter`) have the same shape
//! as the CLAP test plugins', keyed by the instance's `IComponent` pointer.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use vst3::{
    ComPtr,
    Steinberg::IPlugFrame,
    Steinberg::Vst::{IComponentHandler, IComponentHandlerTrait},
};

/// Indexes of `atomik_test_counter`.
pub mod counter {
    /// Instances created.
    pub const CREATED: u32 = 0;
    /// Instances destroyed.
    pub const DESTROYED: u32 = 1;
    /// `setActive(true)` calls.
    pub const ACTIVATED: u32 = 2;
    /// `setActive(false)` calls.
    pub const DEACTIVATED: u32 = 3;
    /// Module entry calls.
    pub const ENTRY_INIT: u32 = 4;
    /// Module exit calls.
    pub const ENTRY_DEINIT: u32 = 5;
    /// Number of counters.
    pub const COUNT: usize = 8;
}

/// Indexes of `atomik_test_probe` (the first 32 follow the CLAP test plugins').
pub mod probe {
    /// `process` calls.
    pub const PROCESS_CALLS: u32 = 0;
    /// Silence flags of the main input bus (all channel bits when silent).
    pub const MASK_MAIN: u32 = 3;
    /// Silence flags of the sidechain bus.
    pub const MASK_SIDECHAIN: u32 = 4;
    /// `ProcessContext::state`.
    pub const TRANSPORT_FLAGS: u32 = 5;
    /// Tempo (f64 bits).
    pub const TEMPO: u32 = 6;
    /// Project time in quarter notes (f64 bits).
    pub const BEATS: u32 = 7;
    /// `projectTimeSamples`.
    pub const SECONDS: u32 = 8;
    /// `continousTimeSamples`.
    pub const STEADY: u32 = 9;
    /// Time signature numerator.
    pub const TSIG_NUM: u32 = 10;
    /// Time signature denominator.
    pub const TSIG_DEN: u32 = 11;
    /// Cycle start in quarter notes (f64 bits).
    pub const LOOP_START: u32 = 12;
    /// Cycle end in quarter notes (f64 bits).
    pub const LOOP_END: u32 = 13;
    /// `numSamples` of the last process call.
    pub const FRAMES: u32 = 14;
    /// Bit set of view calls: 1 attached, 2 removed, 4 getSize, 8 onSize, 16 setFrame, 32 scale,
    /// 64 canResize, 128 checkSizeConstraint.
    pub const GUI_CALLS: u32 = 16;
    /// The parent handle the view was attached to.
    pub const GUI_PARENT: u32 = 17;
    /// The content scale factor (f64 bits).
    pub const GUI_SCALE: u32 = 18;
    /// The width of the last `onSize`.
    pub const GUI_WIDTH: u32 = 19;
    /// The height of the last `onSize`.
    pub const GUI_HEIGHT: u32 = 20;
    /// `IComponent::getState` calls.
    pub const STATE_SAVES: u32 = 21;
    /// `IComponent::setState` calls.
    pub const STATE_LOADS: u32 = 22;
    /// The process mode of `setupProcessing` (0 realtime, 2 offline).
    pub const RENDER_MODE: u32 = 23;
    /// `maxSamplesPerBlock` of `setupProcessing`.
    pub const MAX_FRAMES: u32 = 25;
    /// The sample rate of `setupProcessing` (f64 bits).
    pub const SAMPLE_RATE: u32 = 26;
    /// Events seen by the last process call (note events plus parameter points).
    pub const EVENTS_IN: u32 = 27;
    /// `setActive(true)` calls.
    pub const ACTIVATIONS: u32 = 30;
    /// `setActive(false)` calls.
    pub const DEACTIVATIONS: u32 = 31;
    /// `setBusArrangements` calls.
    pub const ARRANGEMENT_CALLS: u32 = 32;
    /// `IConnectionPoint::connect` calls on either half.
    pub const CONNECTS: u32 = 33;
    /// Messages received through the connection.
    pub const MESSAGES: u32 = 34;
    /// `IEditController::getState` calls.
    pub const CTRL_STATE_SAVES: u32 = 35;
    /// `IEditController::setState` calls.
    pub const CTRL_STATE_LOADS: u32 = 36;
    /// `IEditController::setComponentState` calls.
    pub const CTRL_COMPONENT_STATES: u32 = 37;
    /// Parameter points seen in all process calls.
    pub const PARAM_POINTS: u32 = 38;
    /// The sample offset of the last parameter point seen.
    pub const LAST_POINT_OFFSET: u32 = 39;
    /// Which buses the host activated, as bits: audio in 0 = bit 0, audio in 1 = bit 1, audio
    /// out 0 = bit 2, event in = bit 3, event out = bit 4 (cleared when deactivated).
    pub const BUS_ACTIVE: u32 = 40;
    /// `IPlugView` objects created.
    pub const VIEWS_CREATED: u32 = 41;
    /// The note id of the last note-on event seen.
    pub const LAST_NOTE_ID: u32 = 42;
    /// `setProcessing(true)` calls.
    pub const PROCESSING_STARTS: u32 = 43;
    /// `setProcessing(false)` calls.
    pub const PROCESSING_STOPS: u32 = 44;
    /// `IEditController::setParamNormalized` calls.
    pub const CTRL_SET_PARAMS: u32 = 45;
    /// The param id of the last `setParamNormalized`.
    pub const LAST_SET_ID: u32 = 46;
    /// The value of the last `setParamNormalized` (f64 bits).
    pub const LAST_SET_VALUE: u32 = 47;
    /// Number of probe slots.
    pub const COUNT: usize = 48;
}

/// Commands of `atomik_test_command`.
pub mod command {
    /// `IPlugFrame::resizeView(a x b)`.
    pub const GUI_REQUEST_RESIZE: u32 = 1;
    /// `restartComponent(kLatencyChanged)`.
    pub const LATENCY_CHANGED: u32 = 2;
    /// `IComponentHandler2::setDirty(true)`.
    pub const MARK_DIRTY: u32 = 4;
    /// `restartComponent(kReloadComponent)`.
    pub const REQUEST_RESTART: u32 = 5;
    /// A `beginEdit`, `performEdit(a, f64::from_bits(b))`, `endEdit` of param `a`.
    pub const GUI_EDIT: u32 = 12;
    /// Makes the run-time param appear, then `restartComponent(kParamTitlesChanged)`.
    pub const RESCAN_PARAMS: u32 = 13;
    /// `restartComponent(kMidiCCAssignmentChanged)`.
    pub const MIDI_MAPPING_CHANGED: u32 = 14;
    /// `restartComponent(kParamValuesChanged)`.
    pub const PARAM_VALUES_CHANGED: u32 = 15;
    /// `restartComponent(kIoChanged)`.
    pub const IO_CHANGED: u32 = 16;
}

static COUNTERS: [AtomicU64; counter::COUNT] = [const { AtomicU64::new(0) }; counter::COUNT];

/// Adds one to a process-wide counter.
pub(crate) fn bump(i: u32) {
    COUNTERS[i as usize].fetch_add(1, Ordering::Relaxed);
}

/// Reads a process-wide counter.
pub fn counter_read(i: u32) -> u64 {
    COUNTERS
        .get(i as usize)
        .map_or(0, |c| c.load(Ordering::Relaxed))
}

/// State shared by the halves of one plugin instance and its probes.
pub(crate) struct Shared {
    pub number: u64,
    slots: [AtomicU64; probe::COUNT],
    pub handler: Mutex<Option<ComPtr<IComponentHandler>>>,
    pub frame: Mutex<Option<ComPtr<IPlugFrame>>>,
    /// The address of the view's `IPlugView` interface (0 when there is none).
    pub view: AtomicU64,
    grown: AtomicBool,
}

impl Shared {
    pub fn new(number: u64) -> Arc<Self> {
        let s = Self {
            number,
            slots: [const { AtomicU64::new(0) }; probe::COUNT],
            handler: Mutex::new(None),
            frame: Mutex::new(None),
            view: AtomicU64::new(0),
            grown: AtomicBool::new(false),
        };
        s.set(probe::RENDER_MODE, u64::MAX);
        s.set(probe::MASK_SIDECHAIN, u64::MAX);
        s.set(probe::TRANSPORT_FLAGS, u64::MAX);
        Arc::new(s)
    }

    pub fn set(&self, i: u32, v: u64) {
        self.slots[i as usize].store(v, Ordering::Relaxed);
    }

    pub fn get(&self, i: u32) -> u64 {
        self.slots[i as usize].load(Ordering::Relaxed)
    }

    pub fn add(&self, i: u32, v: u64) {
        self.slots[i as usize].fetch_add(v, Ordering::Relaxed);
    }

    pub fn or(&self, i: u32, v: u64) {
        self.slots[i as usize].fetch_or(v, Ordering::Relaxed);
    }

    /// `IComponentHandler::restartComponent(kLatencyChanged)`, when the host gave a handler.
    pub fn announce_latency(&self) {
        let handler = self
            .handler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(h) = handler {
            // SAFETY: a live handler the host gave the controller.
            unsafe { h.restartComponent(vst3::Steinberg::Vst::RestartFlags_::kLatencyChanged) };
        }
    }

    /// The run-time param was asked for.
    pub fn grown(&self) -> bool {
        self.grown.load(Ordering::Acquire)
    }

    pub fn set_grown(&self) {
        self.grown.store(true, Ordering::Release);
    }

    pub fn and_not(&self, i: u32, v: u64) {
        self.slots[i as usize].fetch_and(!v, Ordering::Relaxed);
    }
}

/// The live instances by the address of their `IComponent` interface.
static REGISTRY: Mutex<Vec<(usize, Arc<Shared>)>> = Mutex::new(Vec::new());

pub(crate) fn register(ptr: usize, shared: Arc<Shared>) {
    REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((ptr, shared));
}

pub(crate) fn unregister(number: u64) {
    REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|(_, s)| s.number != number);
}

pub(crate) fn find_by_number(number: u64) -> Option<Arc<Shared>> {
    REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(_, s)| s.number == number)
        .map(|(_, s)| Arc::clone(s))
}

pub(crate) fn find_by_ptr(ptr: usize) -> Option<Arc<Shared>> {
    REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(p, _)| *p == ptr)
        .map(|(_, s)| Arc::clone(s))
}

/// `atomik_test_probe`: what the instance saw (0 for an unknown instance).
pub fn probe_read(handle: usize, index: u32) -> u64 {
    find_by_ptr(handle).map_or(0, |s| {
        if (index as usize) < probe::COUNT {
            s.get(index)
        } else {
            0
        }
    })
}
