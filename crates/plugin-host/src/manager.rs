//! `PluginManager`: the main-thread owner of the plugin instances.
//!
//! The manager creates an instance through a [`PluginBackend`], activates it and moves its
//! audio half into a [`PluginDevice`] that the app inserts into the engine. From then on the
//! manager and the device talk only through the wait-free pair of the slot: a **restart**
//! (new sample rate, a changed latency or param table, RELOAD) asks the device to hand its
//! processor back at its next batch, deactivates, re-activates and sends the processor back.
//! A dropped device leaves its processor in the [`ReaperQueue`]; [`PluginManager::poll`]
//! deactivates and destroys the plugin on the main thread, or **parks** it when an engine
//! restart wants to keep the instance ([`PluginManager::retain`], [`PluginManager::reattach`]).
//!
//! The live app never waits for a plugin here. A creation is a small state machine that
//! [`PluginManager::poll_create`] moves one step per call (starting, restoring the saved state,
//! activating), a restart or an engine restart re-activates a plugin the same way, and an
//! instance that goes away is handed to a **leaving** list that deactivates it and ends its
//! process across later polls. Offline rendering keeps the blocking calls
//! ([`PluginManager::instantiate`]): its results never depend on how long a plugin takes.
//!
//! Every entry point is `[main-thread]` and checks it in debug builds.

use crate::backend::{
    ActivateConfig, BackendError, EditorKind, HostHandle, NativeParent, PendingInstance,
    PluginBackend, PluginInstance, PluginProcessor,
};
use crate::device::PluginDevice;
use crate::layout::{merge_pinned, truncate_params};
use crate::slot::{
    DeviceEnd, InstanceId, LogLevel, ManagerEnd, ParamPhase, PluginMessage, PluginSlot,
    ReaperQueue, StallOutcome, plugin_channel,
};
use crate::soft::{SoftAnswer, SoftBudget};
use crate::threads::assert_main_thread;
use atomik_core::BATCH;
use atomik_device_sdk::{
    DescriptorError, DeviceDescriptor, PluginFormat, PluginLayout, PluginParamInfo, PluginRef,
    PluginStateBlob, RunMode, descriptor_for,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The error text of a call on an instance whose process is gone.
pub const LOST: &str = "the plugin process stopped";

/// How the reason of a plugin process that stopped answering audio batches begins (the app
/// reloads such a plugin from its last captured state).
pub const HUNG: &str = "the plugin process stopped answering audio batches";

/// How the error of a call begins when the plugin's process is alive but its main thread has not
/// answered an earlier request: the plugin is NOT RESPONDING, calls fail at once instead of
/// waiting, and it answers again by itself when the late answer arrives. The process is not
/// stopped.
pub const NOT_RESPONDING: &str = "the plugin is not responding";

/// What to create.
#[derive(Debug, Clone)]
pub struct InstantiateRequest {
    /// Which plugin.
    pub plugin: PluginRef,
    /// The saved state to restore before the plugin is activated.
    pub state: Option<PluginStateBlob>,
    /// The session sample rate.
    pub sample_rate: f64,
    /// Live or offline (CLAP `render`, VST3 `processMode`).
    pub mode: RunMode,
    /// RUN ON AUDIO THREAD.
    pub pinned: bool,
    /// POWER.
    pub powered: bool,
}

/// An instance ready for the engine: the app inserts `device`.
pub struct PluginHandle {
    /// The instance id.
    pub id: InstanceId,
    /// The engine-side device.
    pub device: PluginDevice,
    /// The plugin's layout now.
    pub layout: PluginLayout,
    /// The interned descriptor of that layout.
    pub descriptor: &'static DeviceDescriptor,
    /// Why the saved state could not be restored, if it could not (the blob must be kept and
    /// the device shown as FAILED).
    pub restore_error: Option<String>,
    /// How long the stages the manager ran itself took (zero for a reattached instance).
    pub timing: CreateTiming,
}

/// How long the stages of a creation that the manager runs itself took, from the request that
/// began each to its answer (the creation is polled, so this includes the time between polls).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CreateTiming {
    /// Restoring the saved state into the new instance.
    pub restore: Duration,
    /// Activating it (the descriptor, the activation and the latency).
    pub activate: Duration,
}

/// The current time, for the load timing of [`CreateTiming`] only: it is logged and never reaches
/// the audio path or the saved state.
// Justification: wall-clock time is the point of a load timing; main thread only (HI-2).
#[allow(clippy::disallowed_methods)]
fn clock() -> Instant {
    Instant::now()
}

/// What the manager can fail with.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ManagerError {
    /// No backend for the format.
    #[error("no backend for {0:?} plugins")]
    NoBackend(PluginFormat),
    /// The backend failed.
    #[error(transparent)]
    Backend(#[from] BackendError),
    /// The layout cannot be turned into a descriptor.
    #[error(transparent)]
    Descriptor(#[from] DescriptorError),
    /// No such instance.
    #[error("unknown plugin instance {0:?}")]
    UnknownInstance(InstanceId),
    /// The instance is live in the engine, not parked.
    #[error("plugin instance {0:?} is not parked")]
    NotParked(InstanceId),
    /// The instance is live in the engine and cannot be removed from here.
    #[error("plugin instance {0:?} is in use")]
    InUse(InstanceId),
}

impl ManagerError {
    /// True when the call failed because the plugin is alive but busy and did not answer (see
    /// [`NOT_RESPONDING`]): nothing is wrong with the plugin, its answer is just not there.
    pub fn is_not_responding(&self) -> bool {
        matches!(self, Self::Backend(e) if e.is_not_responding())
    }
}

/// Something that happened to an instance, for the app to act on.
#[derive(Debug, Clone, PartialEq)]
pub enum ManagerEvent {
    /// A parameter edit made inside the plugin's window. A gesture's begin and end markers
    /// carry no value: NaN from a plugin in this process, 0 from a sandboxed one (the wire has
    /// no NaN).
    ParamEdit {
        /// The instance.
        instance: InstanceId,
        /// The plugin's param id.
        param: u32,
        /// The new plain value.
        value: f64,
        /// Gesture phase.
        phase: ParamPhase,
    },
    /// The plugin's state changed (capture it after a quiet moment).
    StateDirty(InstanceId),
    /// A restart finished: the device is running a re-activated processor.
    Restarted(InstanceId),
    /// A restart could not re-activate the plugin; the device stays dry or silent until
    /// RELOAD.
    RestartFailed {
        /// The instance.
        instance: InstanceId,
        /// Why.
        reason: String,
    },
    /// The reported latency changed (the slot already holds it; the engine's PDC follows).
    LatencyChanged {
        /// The instance.
        instance: InstanceId,
        /// The new latency in frames.
        frames: u32,
    },
    /// The plugin's param table changed: the app registers the new descriptor and builds a
    /// new device (keeping values by key).
    LayoutChanged {
        /// The instance.
        instance: InstanceId,
        /// The new layout.
        layout: PluginLayout,
    },
    /// The plugin asked for its editor to be resized.
    GuiResize {
        /// The instance.
        instance: InstanceId,
        /// Width in pixels.
        w: u32,
        /// Height in pixels.
        h: u32,
    },
    /// An external editor's window moved or changed size (its outer frame, physical pixels).
    GuiFrame {
        /// The instance.
        instance: InstanceId,
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
    GuiClosed(InstanceId),
    /// The plugin asks to show its editor.
    GuiRequestShow(InstanceId),
    /// The plugin asks to hide its editor.
    GuiRequestHide(InstanceId),
    /// A plugin log line.
    Log {
        /// The instance.
        instance: InstanceId,
        /// Severity.
        level: LogLevel,
        /// The text.
        text: String,
    },
    /// The device failed (an error status, a panic, non-finite output).
    Failed {
        /// The instance.
        instance: InstanceId,
        /// Why.
        reason: String,
    },
    /// The instance was deactivated and destroyed.
    Destroyed(InstanceId),
    /// The plugin's process is alive but its main thread did not answer `call` in time (sandbox
    /// only): the plugin counts as NOT RESPONDING, calls on it fail at once, and the process is
    /// left running. Never produced in an offline render.
    NotResponding {
        /// The instance.
        instance: InstanceId,
        /// The name of the call that was not answered.
        call: String,
    },
    /// The plugin answered the call that [`ManagerEvent::NotResponding`] reported: it is
    /// responding again.
    Responding(InstanceId),
    /// The plugin's process ended (sandbox): the device is CRASHED until RELOAD creates a new
    /// instance.
    Crashed {
        /// The instance.
        instance: InstanceId,
        /// The exit code or signal and the last lines of the process's stderr.
        reason: String,
    },
    /// The external editor that [`PluginManager::begin_open_external_editor`] started opening
    /// is open.
    EditorOpened(InstanceId),
    /// The external editor that [`PluginManager::begin_open_external_editor`] started opening
    /// could not open.
    EditorFailed {
        /// The instance.
        instance: InstanceId,
        /// Why.
        reason: String,
    },
}

/// Builds plugin devices; the app and the render tool use it through this trait.
pub trait PluginProvider {
    /// Creates, activates and wraps one plugin instance.
    ///
    /// # Errors
    /// What the backend, or the descriptor of the plugin's layout, said.
    fn instantiate(&mut self, request: &InstantiateRequest) -> Result<PluginHandle, ManagerError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The device holds the processor.
    Running,
    /// A stop was requested; the processor is on its way back.
    Stopping,
    /// The processor is back and the plugin is deactivating (it may take a while: it is
    /// polled, not waited for); it activates again when it is done (a parked instance stays
    /// inactive).
    Deactivating,
    /// The plugin is activating again (polled, not waited for); the device takes the processor
    /// when it is done (for a parked instance that is being reattached, a new device does).
    Activating,
    /// The plugin is inactive and the device has no processor (a failed re-activation).
    Inactive,
}

struct Managed {
    plugin: PluginRef,
    instance: Box<dyn PluginInstance>,
    end: ManagerEnd,
    cfg: ActivateConfig,
    layout: PluginLayout,
    latency: u32,
    phase: Phase,
    retained: bool,
    parked: bool,
    /// An external editor is opening (see [`PluginManager::begin_open_external_editor`]): the
    /// plugin's main thread is busy, so restarts wait until it is done.
    editor_opening: bool,
    /// A restart asked for while the editor was opening.
    deferred_restart: bool,
    /// The background capture of the plugin state (see [`PluginManager::request_capture`]).
    capture: CaptureSlot,
    /// The call the plugin has not answered while its process is alive, as the last poll saw it.
    not_responding: Option<String>,
    /// The plugin's param table changed at the restart that is activating it.
    layout_changed: bool,
    /// A restart was asked for while the plugin was activating: it restarts again when it is
    /// done (the activation in flight used the old settings).
    again: bool,
    /// A parked instance that is being reattached to a new device (see
    /// [`PluginManager::begin_reattach`]).
    reattach: Option<Reattach>,
    /// The pinned params the plugin had when this instance was made.
    pins: Arc<Vec<PluginParamInfo>>,
}

/// A reattach in progress: the instance deactivates, then activates at `rate`.
struct Reattach {
    rate: f64,
    /// The new device, or why there is none, once the activation is over.
    ready: Option<Result<PluginHandle, ManagerError>>,
}

/// The one background capture an instance can have outstanding.
#[derive(Default)]
struct CaptureSlot {
    /// A request is out and has not been collected.
    out: bool,
    /// Another request came while this one was out: the state may have changed since.
    again: bool,
    /// The plugin's state was replaced while the request was out: its answer is out of date.
    stale: bool,
    /// The answer, when it came at the request (a plugin in this process) or the request failed.
    ready: Option<Result<PluginStateBlob, BackendError>>,
}

/// A background capture that has come back, from [`PluginManager::poll_capture`].
#[derive(Debug)]
pub struct CaptureLanded {
    /// The state, or why there is none.
    pub result: Result<PluginStateBlob, ManagerError>,
    /// Another capture was asked for while this one was out: the plugin may have changed again.
    pub again: bool,
    /// The plugin's state was replaced (restored) while the request was out, or the instance is
    /// gone: the answer must not be stored.
    pub stale: bool,
}

/// How many stall records the manager keeps for a caller that does not drain them.
const MAX_STALLS: usize = 64;

/// The most cosmetic answers kept for [`PluginManager::take_soft_answers`] (the newest).
const MAX_SOFT_ANSWERS: usize = 256;

/// A blocking call on a plugin's process that held the main thread up for at least
/// [`crate::STALL_THRESHOLD`]. Kept apart from [`ManagerEvent`]: it depends on wall time, and the
/// event stream must not (an offline bounce polls it after every batch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StallRecord {
    /// The instance.
    pub instance: InstanceId,
    /// The name of the call.
    pub call: String,
    /// How long the main thread waited, in milliseconds.
    pub ms: u64,
    /// How the wait ended.
    pub outcome: StallOutcome,
}

fn push_stall(list: &mut Vec<StallRecord>, record: StallRecord) {
    if list.len() >= MAX_STALLS {
        list.remove(0);
    }
    list.push(record);
}

/// A creation started by [`PluginManager::begin_create`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CreateTicket(u64);

/// A plugin that is starting in another process.
struct Started {
    pending: Box<dyn PendingInstance>,
    device_end: DeviceEnd,
    end: ManagerEnd,
}

/// An instance the backend gave, on its way to being active: the saved state is restored, then
/// the plugin is activated (see [`PluginManager::poll_create`]).
struct Booting {
    instance: Box<dyn PluginInstance>,
    device_end: DeviceEnd,
    end: ManagerEnd,
    /// Why the saved state could not be restored, if it could not.
    restore_error: Option<String>,
    /// When the restore began.
    began: Instant,
    /// How long the restore took (set when it is over).
    restore: Duration,
    /// When the activation began.
    activating_since: Instant,
}

/// The descriptor and layout a device is built from.
type Prepared = (&'static DeviceDescriptor, PluginLayout);

/// Where a creation is.
enum Stage {
    /// The backend creates the instance when the ticket is first polled (and blocks).
    Sync,
    /// The plugin is starting in another process.
    Instantiating(Started),
    /// The saved state is being restored.
    Restoring(Booting),
    /// The plugin is activating.
    Activating(Booting, Prepared),
    /// The plugin is active and waits to be installed (see [`PluginManager::creation_ready`]).
    Ready(Box<Readied>),
    /// The creation failed; the error waits to be handed out.
    Failed(Box<ManagerError>),
}

/// A plugin that is active but not installed yet: the manager has not listed it, so nothing of
/// it is polled or reported until [`PluginManager::poll_create`] hands the handle out.
struct Readied {
    boot: Booting,
    prepared: Prepared,
    processor: Box<dyn PluginProcessor>,
}

struct Creating {
    request: InstantiateRequest,
    stage: Stage,
}

/// What an instance that is going away is waiting for.
enum Leave {
    /// A restart's activation is out: its answer is collected, then deactivated.
    Activating,
    /// A deactivation is out.
    Deactivating,
    /// Only the process that outlives the instance is left (a plugin in this process has none).
    Ending,
}

/// What is going away.
enum Leaver {
    /// A plugin that was starting in another process.
    Pending(Box<dyn PendingInstance>),
    Instance(Box<dyn PluginInstance>),
}

/// An instance that left the manager and is being taken down across polls, so that nobody waits
/// for a plugin that is busy: [`PluginManager::poll`] moves it through the deactivation to the
/// end of its process.
struct Leaving {
    id: InstanceId,
    what: Leaver,
    step: Leave,
    /// The end shutdown was started.
    begun: bool,
    /// What the instance said while it went away (a stall of its own deactivation).
    end: Option<ManagerEnd>,
}

impl Leaving {
    /// A creation that is abandoned: what it started is ended. A plugin that has not been
    /// created yet (a backend that creates it when polled) has nothing to end.
    fn of_creation(stage: Stage) -> Option<Self> {
        let mut step = Leave::Ending;
        let (what, end) = match stage {
            Stage::Sync | Stage::Failed(_) => return None,
            Stage::Instantiating(started) => (Leaver::Pending(started.pending), started.end),
            Stage::Restoring(b) | Stage::Activating(b, _) => (Leaver::Instance(b.instance), b.end),
            // Active but never installed: the processor goes back first, then the process ends.
            Stage::Ready(ready) => {
                let Readied {
                    boot, processor, ..
                } = *ready;
                let mut instance = boot.instance;
                if !instance.begin_deactivate(processor) {
                    step = Leave::Deactivating;
                }
                (Leaver::Instance(instance), boot.end)
            }
        };
        Some(Self {
            id: end.slot().id(),
            what,
            step,
            begun: false,
            end: Some(end),
        })
    }

    /// One poll of the way out; true when it is over and the instance can be dropped.
    fn advance(&mut self, stalls: &mut Vec<StallRecord>) -> bool {
        loop {
            let Leaver::Instance(instance) = &mut self.what else {
                return self.end_step(stalls);
            };
            match self.step {
                Leave::Activating => match instance.poll_activate() {
                    // The activation produced the processor that must go back first.
                    Some(Ok(processor)) => {
                        self.step = if instance.begin_deactivate(processor) {
                            Leave::Ending
                        } else {
                            Leave::Deactivating
                        };
                    }
                    Some(Err(_)) => self.step = Leave::Ending,
                    None if instance.is_lost() => self.step = Leave::Ending,
                    None => return false,
                },
                Leave::Deactivating => {
                    if instance.poll_deactivate() || instance.is_lost() {
                        self.step = Leave::Ending;
                    } else {
                        return false;
                    }
                }
                Leave::Ending => return self.end_step(stalls),
            }
        }
    }

    /// Starts ending the process and says whether it has ended.
    fn end_step(&mut self, stalls: &mut Vec<StallRecord>) -> bool {
        let done = match &mut self.what {
            Leaver::Pending(pending) => {
                if !self.begun {
                    pending.begin_shutdown();
                    self.begun = true;
                }
                pending.poll_shutdown()
            }
            Leaver::Instance(instance) => {
                if !self.begun {
                    instance.begin_shutdown();
                    self.begun = true;
                }
                instance.poll_shutdown()
            }
        };
        if done {
            drain_stalls(self.id, self.end.as_ref(), stalls);
        }
        done
    }
}

/// Moves the stall records an instance posted into `stalls`.
fn drain_stalls(id: InstanceId, end: Option<&ManagerEnd>, stalls: &mut Vec<StallRecord>) {
    while let Some(msg) = end.and_then(ManagerEnd::try_recv) {
        if let PluginMessage::Stall { call, ms, outcome } = msg {
            push_stall(
                stalls,
                StallRecord {
                    instance: id,
                    call,
                    ms,
                    outcome,
                },
            );
        }
    }
}

/// The main-thread owner of the plugin instances.
#[derive(Default)]
pub struct PluginManager {
    backends: Vec<Box<dyn PluginBackend>>,
    instances: BTreeMap<InstanceId, Managed>,
    reaper: ReaperQueue,
    queued: Vec<ManagerEvent>,
    /// Slow-call records waiting for [`PluginManager::take_stalls`] (the newest [`MAX_STALLS`]).
    stalls: Vec<StallRecord>,
    /// Cosmetic answers that arrived late, waiting for [`PluginManager::take_soft_answers`]
    /// (the newest [`MAX_SOFT_ANSWERS`]). Like the stalls they depend on wall time, so they are
    /// not events.
    soft_answers: Vec<(InstanceId, SoftAnswer)>,
    /// The allowance of waiting for cosmetic answers that every instance shares in a frame.
    soft: SoftBudget,
    next_id: u64,
    creating: BTreeMap<u64, Creating>,
    next_ticket: u64,
    /// Instances that left and are being taken down (see [`Leaving`]).
    leaving: Vec<Leaving>,
    /// The params each plugin lists beyond what it reports (see
    /// [`PluginManager::set_pinned_params`]).
    pins: BTreeMap<PluginRef, Arc<Vec<PluginParamInfo>>>,
}

/// `layout` as the rack lists it: what the plugin reports plus the `pins` (see
/// [`PluginManager::set_pinned_params`]), cut to the most params a layout holds.
fn listed_layout(layout: &PluginLayout, pins: &[PluginParamInfo]) -> PluginLayout {
    let mut layout = layout.clone();
    merge_pinned(&mut layout, pins);
    truncate_params(&mut layout);
    layout
}

fn descriptor_of(
    plugin: &PluginRef,
    layout: &PluginLayout,
    pins: &[PluginParamInfo],
) -> Result<(&'static DeviceDescriptor, PluginLayout), DescriptorError> {
    let layout = listed_layout(layout, pins);
    descriptor_for(plugin, &layout).map(|d| (d, layout))
}

/// What an activated instance brings to [`PluginManager::install`].
struct Activated {
    processor: Box<dyn PluginProcessor>,
    descriptor: &'static DeviceDescriptor,
    layout: PluginLayout,
    restore_error: Option<String>,
    timing: CreateTiming,
}

/// Starts the activation; an offline run (a bounce's pre-roll) waits for the plugin so that it
/// sees the activation done at the same batch in every process, whatever the link's timeouts.
fn activate_now_or_later(
    instance: &mut dyn PluginInstance,
    cfg: &ActivateConfig,
) -> Result<Option<Box<dyn PluginProcessor>>, BackendError> {
    if cfg.mode == RunMode::Offline {
        instance.activate(cfg).map(Some)
    } else {
        instance.begin_activate(cfg)
    }
}

fn activate_config(request: &InstantiateRequest) -> ActivateConfig {
    ActivateConfig {
        sample_rate: request.sample_rate,
        frames: BATCH as u32,
        mode: request.mode,
    }
}

impl PluginManager {
    /// A manager with no backends.
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
            instances: BTreeMap::new(),
            reaper: ReaperQueue::default(),
            queued: Vec::new(),
            stalls: Vec::new(),
            soft_answers: Vec::new(),
            soft: SoftBudget::default(),
            next_id: 1,
            creating: BTreeMap::new(),
            next_ticket: 1,
            leaving: Vec::new(),
            pins: BTreeMap::new(),
        }
    }

    /// Adds a format backend.
    pub fn add_backend(&mut self, backend: Box<dyn PluginBackend>) {
        self.backends.push(backend);
    }

    /// Sets the params the rack lists for `plugin` beyond what the plugin reports: a plugin may
    /// leave out parameters it publishes only as stand-ins for MIDI controllers, and a project
    /// that still uses one of them (a lane, a learned control, a stored value) has to find it in
    /// the table. The rows are added to the layout of every instance made, and of every layout
    /// change reported, from now on (see [`crate::layout::merge_pinned`]); instances that exist
    /// keep the table they were made with. An empty list clears the pins of the plugin.
    pub fn set_pinned_params(&mut self, plugin: &PluginRef, rows: Vec<PluginParamInfo>) {
        if rows.is_empty() {
            self.pins.remove(plugin);
        } else {
            self.pins.insert(plugin.clone(), Arc::new(rows));
        }
    }

    /// The pinned params of `plugin` (see [`Self::set_pinned_params`]).
    fn pins_of(&self, plugin: &PluginRef) -> Arc<Vec<PluginParamInfo>> {
        self.pins.get(plugin).cloned().unwrap_or_default()
    }

    /// The reaper queue devices post to (it is already wired into every device the manager
    /// builds).
    pub fn reaper(&self) -> ReaperQueue {
        self.reaper.clone()
    }

    /// Number of instances (live and parked).
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// Number of instances that left and are still being taken down: their plugin is
    /// deactivating or its process has not ended yet (see [`PluginManager::poll`]).
    pub fn leaving_count(&self) -> usize {
        self.leaving.len()
    }

    /// Whether the instance exists.
    pub fn contains(&self, id: InstanceId) -> bool {
        self.instances.contains_key(&id)
    }

    /// The shared slot of an instance.
    pub fn slot(&self, id: InstanceId) -> Option<PluginSlot> {
        self.instances.get(&id).map(|m| m.end.slot().clone())
    }

    /// The plugin's layout now.
    pub fn layout(&self, id: InstanceId) -> Option<&PluginLayout> {
        self.instances.get(&id).map(|m| &m.layout)
    }

    /// The call the instance has not answered although its process is alive (see
    /// [`NOT_RESPONDING`]); `None` when it responds, is gone or is unknown. Answers from the
    /// instance itself, so it is current between two polls.
    pub fn not_responding(&self, id: InstanceId) -> Option<String> {
        self.instances.get(&id)?.instance.not_responding()
    }

    /// Stops the process of an instance that does not respond (see [`NOT_RESPONDING`]) at the
    /// user's request: it is lost from then on, like a crashed one, and the app replaces it.
    /// Returns false when the instance is unknown or does respond.
    pub fn stop_unresponsive(&mut self, id: InstanceId) -> bool {
        assert_main_thread("PluginManager::stop_unresponsive");
        let Some(m) = self.instances.get_mut(&id) else {
            return false;
        };
        if m.instance.not_responding().is_none() {
            return false;
        }
        m.instance.stop_process();
        true
    }

    /// The ids of every instance (live and parked), in creation order.
    pub fn instance_ids(&self) -> Vec<InstanceId> {
        self.instances.keys().copied().collect()
    }

    /// The run mode the instance is activated in (offline for a bounce, realtime when live).
    pub fn run_mode(&self, id: InstanceId) -> Option<RunMode> {
        self.instances.get(&id).map(|m| m.cfg.mode)
    }

    /// The latency the plugin reports, in frames.
    pub fn latency(&self, id: InstanceId) -> Option<u32> {
        self.instances.get(&id).map(|m| m.latency)
    }

    fn get(&mut self, id: InstanceId) -> Result<&mut Managed, ManagerError> {
        self.instances
            .get_mut(&id)
            .ok_or(ManagerError::UnknownInstance(id))
    }

    // ----- creating -----------------------------------------------------------------------------

    /// Creates an instance, restores its state, activates it and wraps it in a device.
    ///
    /// # Errors
    /// [`ManagerError::NoBackend`], the backend's error, or a descriptor error.
    pub fn instantiate(
        &mut self,
        request: &InstantiateRequest,
    ) -> Result<PluginHandle, ManagerError> {
        assert_main_thread("PluginManager::instantiate");
        let backend = self
            .backends
            .iter()
            .find(|b| b.format() == request.plugin.format)
            .ok_or(ManagerError::NoBackend(request.plugin.format))?;
        let id = InstanceId(self.next_id);
        let (device_end, end) = plugin_channel(id);
        let instance = backend.instantiate(
            &request.plugin,
            HostHandle::with_budget(end.messenger(), self.soft.clone()),
        )?;
        let handle = self.complete(request, instance, device_end, end)?;
        self.next_id += 1;
        Ok(handle)
    }

    /// Starts creating an instance for `request` and returns at once: a plugin that loads in
    /// another process loads while the caller goes on, so many plugins load at the same time.
    /// [`PluginManager::poll_create`] moves it on and gives the handle. A backend that cannot
    /// start in the background (a plugin loaded into this process) creates the instance in the
    /// `poll_create` that first asks, which then blocks like [`PluginManager::instantiate`].
    ///
    /// # Errors
    /// [`ManagerError::NoBackend`], or what the backend could not start.
    pub fn begin_create(
        &mut self,
        request: &InstantiateRequest,
    ) -> Result<CreateTicket, ManagerError> {
        assert_main_thread("PluginManager::begin_create");
        let backend = self
            .backends
            .iter()
            .find(|b| b.format() == request.plugin.format)
            .ok_or(ManagerError::NoBackend(request.plugin.format))?;
        let id = InstanceId(self.next_id);
        let (device_end, end) = plugin_channel(id);
        let pending = backend.begin_instantiate(
            &request.plugin,
            HostHandle::with_budget(end.messenger(), self.soft.clone()),
        )?;
        let stage = match pending {
            Some(pending) => {
                // The id belongs to this start: the next one gets another.
                self.next_id += 1;
                Stage::Instantiating(Started {
                    pending,
                    device_end,
                    end,
                })
            }
            None => Stage::Sync,
        };
        let ticket = CreateTicket(self.next_ticket);
        self.next_ticket += 1;
        self.creating.insert(
            ticket.0,
            Creating {
                request: request.clone(),
                stage,
            },
        );
        Ok(ticket)
    }

    /// Moves the creation `ticket` on without blocking (see [`PluginManager::begin_create`]):
    /// `None` while the plugin is not ready, then the handle or the error. The answer spends
    /// the ticket. A plugin in another process goes through starting, restoring its saved
    /// state (when there is one) and activating, each a request that is polled, so a sampler
    /// that restores a big library for minutes does not hold the caller up; every error is the
    /// one the blocking [`PluginManager::instantiate`] gives.
    pub fn poll_create(
        &mut self,
        ticket: CreateTicket,
    ) -> Option<Result<PluginHandle, ManagerError>> {
        assert_main_thread("PluginManager::poll_create");
        let Creating { request, stage } = self.creating.remove(&ticket.0)?;
        match self.advance_creation(&request, stage) {
            Stage::Sync => Some(self.instantiate(&request)),
            Stage::Failed(e) => Some(Err(*e)),
            Stage::Ready(ready) => {
                let Readied {
                    boot,
                    prepared,
                    processor,
                } = *ready;
                Some(Ok(self.finish_boot(&request, boot, prepared, processor)))
            }
            stage => {
                self.creating.insert(ticket.0, Creating { request, stage });
                None
            }
        }
    }

    /// Moves the creation `ticket` on without blocking and without installing it, and says
    /// whether it can be answered now: `true` once the plugin is active (or failed, or is a
    /// backend that creates it when polled), `false` while it loads, `None` for a spent or
    /// unknown ticket. A ready creation is not part of the manager yet (it is not listed, polled
    /// or reported), so the caller can poll every creation each frame and still choose when to
    /// take each handle with [`PluginManager::poll_create`].
    pub fn creation_ready(&mut self, ticket: CreateTicket) -> Option<bool> {
        assert_main_thread("PluginManager::creation_ready");
        let Creating { request, stage } = self.creating.remove(&ticket.0)?;
        let stage = self.advance_creation(&request, stage);
        let ready = matches!(stage, Stage::Sync | Stage::Ready(_) | Stage::Failed(_));
        self.creating.insert(ticket.0, Creating { request, stage });
        Some(ready)
    }

    /// Abandons the creation `ticket`: a plugin that was starting in another process ends (the
    /// end is polled, not waited for).
    pub fn cancel_create(&mut self, ticket: CreateTicket) {
        assert_main_thread("PluginManager::cancel_create");
        let Some(Creating { stage, .. }) = self.creating.remove(&ticket.0) else {
            return;
        };
        self.leaving.extend(Leaving::of_creation(stage));
        self.advance_leaving();
    }

    /// One step of a creation: as far as it can go without waiting. It stops at an active plugin
    /// (the stage `Ready`) and at an error (`Failed`) without installing anything.
    fn advance_creation(&mut self, request: &InstantiateRequest, mut stage: Stage) -> Stage {
        loop {
            stage = match stage {
                Stage::Sync | Stage::Ready(_) | Stage::Failed(_) => return stage,
                Stage::Instantiating(mut started) => match started.pending.poll() {
                    None => return Stage::Instantiating(started),
                    Some(Err(e)) => return Stage::Failed(Box::new(e.into())),
                    Some(Ok(instance)) => {
                        let boot = Booting {
                            instance,
                            device_end: started.device_end,
                            end: started.end,
                            restore_error: None,
                            began: clock(),
                            restore: Duration::ZERO,
                            activating_since: clock(),
                        };
                        self.begin_restore(request, boot)
                    }
                },
                Stage::Restoring(mut boot) => match boot.instance.poll_restore_state() {
                    None => return Stage::Restoring(boot),
                    Some(result) => {
                        boot.restore_error = result.err().map(|e| e.to_string());
                        boot.restore = clock().saturating_duration_since(boot.began);
                        self.begin_activation(request, boot)
                    }
                },
                Stage::Activating(mut boot, prepared) => match boot.instance.poll_activate() {
                    None => return Stage::Activating(boot, prepared),
                    Some(Err(e)) => return Stage::Failed(Box::new(e.into())),
                    Some(Ok(processor)) => Stage::Ready(Box::new(Readied {
                        boot,
                        prepared,
                        processor,
                    })),
                },
            };
        }
    }

    /// Starts restoring the saved state, if there is one: the next stage, or the end of the
    /// creation. State first (so a latency that depends on it is right at activation), rack
    /// params after (the device re-sends every param the rack sets). A state that cannot be
    /// restored is not an error of the creation: the device is shown as FAILED.
    fn begin_restore(&mut self, request: &InstantiateRequest, mut boot: Booting) -> Stage {
        if let Some(state) = &request.state {
            // An offline render waits for the plugin (see `activate_now_or_later`).
            let begun = if request.mode == RunMode::Offline {
                boot.instance.restore_state(state).map(|()| true)
            } else {
                boot.instance.begin_restore_state(state)
            };
            match begun {
                Ok(false) => return Stage::Restoring(boot),
                Ok(true) => {}
                Err(e) => boot.restore_error = Some(e.to_string()),
            }
        }
        boot.restore = clock().saturating_duration_since(boot.began);
        self.begin_activation(request, boot)
    }

    /// Starts the activation: the next stage, or the end of the creation.
    fn begin_activation(&mut self, request: &InstantiateRequest, mut boot: Booting) -> Stage {
        boot.activating_since = clock();
        let prepared = match descriptor_of(
            &request.plugin,
            boot.instance.layout(),
            &self.pins_of(&request.plugin),
        ) {
            Ok(prepared) => prepared,
            Err(e) => return Stage::Failed(Box::new(e.into())),
        };
        match activate_now_or_later(boot.instance.as_mut(), &activate_config(request)) {
            Ok(Some(processor)) => Stage::Ready(Box::new(Readied {
                boot,
                prepared,
                processor,
            })),
            Ok(None) => Stage::Activating(boot, prepared),
            Err(e) => Stage::Failed(Box::new(e.into())),
        }
    }

    /// The instance is active: the device and the handle.
    fn finish_boot(
        &mut self,
        request: &InstantiateRequest,
        boot: Booting,
        (descriptor, layout): Prepared,
        processor: Box<dyn PluginProcessor>,
    ) -> PluginHandle {
        let timing = CreateTiming {
            restore: boot.restore,
            activate: clock().saturating_duration_since(boot.activating_since),
        };
        self.install(
            request,
            boot.instance,
            boot.device_end,
            boot.end,
            Activated {
                processor,
                descriptor,
                layout,
                restore_error: boot.restore_error,
                timing,
            },
        )
    }

    /// The rest of an instantiation once the backend gave the instance: the state, the
    /// descriptor, the activation and the device (all of it waiting for the plugin).
    fn complete(
        &mut self,
        request: &InstantiateRequest,
        mut instance: Box<dyn PluginInstance>,
        device_end: DeviceEnd,
        end: ManagerEnd,
    ) -> Result<PluginHandle, ManagerError> {
        let began = clock();
        let restore_error = request
            .state
            .as_ref()
            .and_then(|s| instance.restore_state(s).err())
            .map(|e| e.to_string());
        let restored = clock();
        let (descriptor, layout) = descriptor_of(
            &request.plugin,
            instance.layout(),
            &self.pins_of(&request.plugin),
        )?;
        let processor = instance.activate(&activate_config(request))?;
        let timing = CreateTiming {
            restore: restored.saturating_duration_since(began),
            activate: clock().saturating_duration_since(restored),
        };
        Ok(self.install(
            request,
            instance,
            device_end,
            end,
            Activated {
                processor,
                descriptor,
                layout,
                restore_error,
                timing,
            },
        ))
    }

    /// Records the active instance and wraps its processor in a device.
    fn install(
        &mut self,
        request: &InstantiateRequest,
        instance: Box<dyn PluginInstance>,
        device_end: DeviceEnd,
        end: ManagerEnd,
        active: Activated,
    ) -> PluginHandle {
        let Activated {
            processor,
            descriptor,
            layout,
            restore_error,
            timing,
        } = active;
        let id = end.slot().id();
        let cfg = activate_config(request);
        let latency = instance.latency_frames();
        let slot = end.slot().clone();
        slot.set_latency(latency, cfg.sample_rate);
        slot.set_pinned(request.pinned);
        slot.set_powered(request.powered);
        let device = PluginDevice::new(descriptor, &layout, processor, device_end, cfg.sample_rate)
            .with_reaper(self.reaper.clone());
        self.instances.insert(
            id,
            Managed {
                plugin: request.plugin.clone(),
                instance,
                end,
                cfg,
                layout: layout.clone(),
                latency,
                phase: Phase::Running,
                retained: false,
                parked: false,
                editor_opening: false,
                deferred_restart: false,
                capture: CaptureSlot::default(),
                not_responding: None,
                layout_changed: false,
                again: false,
                reattach: None,
                pins: self.pins_of(&request.plugin),
            },
        );
        PluginHandle {
            id,
            device,
            layout,
            descriptor,
            restore_error,
            timing,
        }
    }

    // ----- state, text, editor --------------------------------------------------------------------

    /// Saves the plugin's state.
    ///
    /// # Errors
    /// An unknown instance, or what the plugin said.
    pub fn capture_state(&mut self, id: InstanceId) -> Result<PluginStateBlob, ManagerError> {
        assert_main_thread("PluginManager::capture_state");
        let m = self.get(id)?;
        // This answer replaces the background one (a remote instance waits for the request that
        // is out instead of sending another).
        m.capture = CaptureSlot::default();
        Ok(m.instance.capture_state()?)
    }

    /// Sends the capture request to the instances in `ids` without waiting; the
    /// [`PluginManager::capture_state`] of each collects its answer. A remote instance not
    /// collected afterwards just drops the answer. An instance with a background capture out
    /// is left to that one.
    pub fn begin_capture(&mut self, ids: &[InstanceId]) {
        assert_main_thread("PluginManager::begin_capture");
        for id in ids {
            if let Some(m) = self.instances.get_mut(id)
                && !m.capture.out
            {
                m.instance.begin_capture_state();
            }
        }
    }

    /// Asks the instance for its state without waiting for it (the live app's way: a plugin
    /// whose main thread is busy answers when it is free, and nothing here stands still);
    /// [`PluginManager::poll_capture`] collects it. At most one request is out per instance: a
    /// request while one is out only notes that the state may have changed since (the answer
    /// then says `again`). A plugin in this process answers at once.
    ///
    /// # Errors
    /// An unknown instance.
    pub fn request_capture(&mut self, id: InstanceId) -> Result<(), ManagerError> {
        assert_main_thread("PluginManager::request_capture");
        let m = self.get(id)?;
        if m.capture.out {
            m.capture.again = true;
            return Ok(());
        }
        m.capture = CaptureSlot {
            out: true,
            ..CaptureSlot::default()
        };
        if m.instance.background_capture() {
            if let Err(e) = m.instance.request_capture() {
                m.capture.ready = Some(Err(e));
            }
        } else {
            m.capture.ready = Some(m.instance.capture_state());
        }
        Ok(())
    }

    /// True while a [`PluginManager::request_capture`] of the instance has not been collected.
    pub fn capture_out(&self, id: InstanceId) -> bool {
        self.instances.get(&id).is_some_and(|m| m.capture.out)
    }

    /// The answer to [`PluginManager::request_capture`] if it has come; never waits. An
    /// instance that is gone answers at once with a stale error.
    pub fn poll_capture(&mut self, id: InstanceId) -> Option<CaptureLanded> {
        assert_main_thread("PluginManager::poll_capture");
        let Some(m) = self.instances.get_mut(&id) else {
            return Some(CaptureLanded {
                result: Err(ManagerError::UnknownInstance(id)),
                again: false,
                stale: true,
            });
        };
        if !m.capture.out {
            return None;
        }
        let got = match m.capture.ready.take() {
            Some(got) => got,
            None => m.instance.poll_capture()?,
        };
        let slot = std::mem::take(&mut m.capture);
        Some(CaptureLanded {
            result: got.map_err(ManagerError::from),
            again: slot.again,
            stale: slot.stale,
        })
    }

    /// Saves every instance's state (before a save a).
    pub fn capture_all(&mut self) -> Vec<(InstanceId, Result<PluginStateBlob, ManagerError>)> {
        assert_main_thread("PluginManager::capture_all");
        self.instances
            .iter_mut()
            .map(|(id, m)| {
                m.capture = CaptureSlot::default();
                (*id, m.instance.capture_state().map_err(ManagerError::from))
            })
            .collect()
    }

    /// Restores a saved state into a living instance.
    ///
    /// # Errors
    /// An unknown instance, or what the plugin said (the state is kept by the caller).
    pub fn restore_state(
        &mut self,
        id: InstanceId,
        state: &PluginStateBlob,
    ) -> Result<(), ManagerError> {
        assert_main_thread("PluginManager::restore_state");
        let m = self.get(id)?;
        // A capture asked for before this restore describes the state that is being replaced.
        m.capture.stale |= m.capture.out;
        Ok(m.instance.restore_state(state)?)
    }

    /// The presets the plugin offers (empty for an unknown instance).
    pub fn list_presets(&mut self, id: InstanceId) -> Vec<crate::PresetInfo> {
        assert_main_thread("PluginManager::list_presets");
        self.instances
            .get_mut(&id)
            .map(|m| m.instance.list_presets())
            .unwrap_or_default()
    }

    /// Loads a preset into a living instance; read the new state with
    /// [`PluginManager::capture_state`] afterwards.
    ///
    /// # Errors
    /// An unknown instance, or what the plugin said.
    pub fn load_preset(&mut self, id: InstanceId, preset: &str) -> Result<(), ManagerError> {
        assert_main_thread("PluginManager::load_preset");
        Ok(self.get(id)?.instance.load_preset(preset)?)
    }

    /// The param values the plugin holds now (see [`crate::PluginInstance::read_params`]); empty
    /// for an unknown instance.
    pub fn read_params(&mut self, id: InstanceId) -> Vec<(u32, f64)> {
        assert_main_thread("PluginManager::read_params");
        self.instances
            .get_mut(&id)
            .map(|m| m.instance.read_params())
            .unwrap_or_default()
    }

    /// Asks for the presets the plugin offers without waiting (the live app): `Some(list)` when
    /// the answer was there at once, `None` when it arrives later as [`SoftAnswer::Presets`]
    /// through [`PluginManager::take_soft_answers`].
    ///
    /// # Errors
    /// An unknown instance, or a plugin that is lost, stuck or busy with a held call.
    pub fn request_presets(
        &mut self,
        id: InstanceId,
    ) -> Result<Option<Vec<crate::PresetInfo>>, ManagerError> {
        assert_main_thread("PluginManager::request_presets");
        Ok(self.get(id)?.instance.begin_list_presets()?)
    }

    /// Asks for the param values the plugin holds without waiting, in the way of
    /// [`PluginManager::request_presets`] (the answer is a [`SoftAnswer::Params`]).
    ///
    /// # Errors
    /// An unknown instance, or a plugin that is lost, stuck or busy with a held call.
    pub fn request_params(
        &mut self,
        id: InstanceId,
    ) -> Result<Option<Vec<(u32, f64)>>, ManagerError> {
        assert_main_thread("PluginManager::request_params");
        Ok(self.get(id)?.instance.begin_read_params()?)
    }

    /// Starts loading a preset into a living instance without waiting (the live app): true when
    /// it is loaded, false when [`PluginManager::poll_load_preset`] says when it is. Read the
    /// new state afterwards.
    ///
    /// # Errors
    /// An unknown instance, or what the plugin said.
    pub fn begin_load_preset(
        &mut self,
        id: InstanceId,
        preset: &str,
    ) -> Result<bool, ManagerError> {
        assert_main_thread("PluginManager::begin_load_preset");
        let m = self.get(id)?;
        // A capture asked for before the load describes the state that is being replaced.
        m.capture.stale |= m.capture.out;
        Ok(m.instance.begin_load_preset(preset)?)
    }

    /// The outcome of the preset load [`PluginManager::begin_load_preset`] started, once;
    /// `None` while the plugin has not answered.
    pub fn poll_load_preset(&mut self, id: InstanceId) -> Option<Result<(), ManagerError>> {
        assert_main_thread("PluginManager::poll_load_preset");
        let m = self.instances.get_mut(&id)?;
        Some(m.instance.poll_load_preset()?.map_err(ManagerError::from))
    }

    /// The plugin's display text of a param value (never asked on the audio thread). A plugin
    /// in another process that does not answer within the frame's allowance gives `None` and the
    /// text arrives later through [`PluginManager::take_soft_answers`].
    pub fn param_text(&mut self, id: InstanceId, param: u32, plain: f64) -> Option<String> {
        assert_main_thread("PluginManager::param_text");
        self.instances
            .get_mut(&id)?
            .instance
            .param_text(param, plain)
    }

    /// An opaque id of the native instance for diagnostics and tests.
    pub fn native_handle(&self, id: InstanceId) -> Option<usize> {
        self.instances.get(&id).map(|m| m.instance.native_handle())
    }

    /// Whether the plugin has an editor.
    pub fn has_gui(&self, id: InstanceId) -> bool {
        self.instances
            .get(&id)
            .is_some_and(|m| m.instance.has_gui())
    }

    /// The params a panel pins by default (CLAP: the first remote-controls page).
    pub fn default_pins(&mut self, id: InstanceId) -> Vec<u32> {
        assert_main_thread("PluginManager::default_pins");
        self.instances
            .get_mut(&id)
            .map(|m| m.instance.default_pins())
            .unwrap_or_default()
    }

    /// Opens the plugin's editor inside `parent`; returns its size in pixels.
    ///
    /// # Errors
    /// An unknown instance, or what the plugin said.
    pub fn open_gui(
        &mut self,
        id: InstanceId,
        parent: NativeParent,
        scale: f64,
    ) -> Result<(u32, u32), ManagerError> {
        assert_main_thread("PluginManager::open_gui");
        Ok(self.get(id)?.instance.open_gui(parent, scale)?)
    }

    /// How the instance's editor is shown.
    pub fn editor_kind(&self, id: InstanceId) -> Option<EditorKind> {
        self.instances.get(&id).map(|m| m.instance.editor_kind())
    }

    /// Opens (or raises) an [`EditorKind::External`] editor, owned by `owner`.
    ///
    /// # Errors
    /// An unknown instance, or what the plugin said.
    pub fn open_external_editor(
        &mut self,
        id: InstanceId,
        owner: Option<NativeParent>,
    ) -> Result<(), ManagerError> {
        assert_main_thread("PluginManager::open_external_editor");
        Ok(self.get(id)?.instance.open_external_gui(owner)?)
    }

    /// Starts opening (or raising) an [`EditorKind::External`] editor without waiting for it:
    /// true when it is open already, false when [`PluginManager::poll`] will report
    /// [`ManagerEvent::EditorOpened`] or [`ManagerEvent::EditorFailed`]. A plugin may show
    /// licence or sign-in dialogs before its editor opens, which can take minutes.
    ///
    /// # Errors
    /// An unknown instance, or what the plugin said.
    pub fn begin_open_external_editor(
        &mut self,
        id: InstanceId,
        owner: Option<NativeParent>,
    ) -> Result<bool, ManagerError> {
        assert_main_thread("PluginManager::begin_open_external_editor");
        let m = self.get(id)?;
        let open = m.instance.begin_open_external_gui(owner)?;
        m.editor_opening = !open;
        Ok(open)
    }

    /// Starts opening an [`EditorKind::External`] editor **inside** `parent`, a window of the
    /// app (see [`PluginInstance::begin_open_embedded_gui`]); the outcome is reported like that
    /// of [`PluginManager::begin_open_external_editor`].
    ///
    /// # Errors
    /// An unknown instance, or what the plugin said (an editor that cannot be embedded).
    pub fn begin_open_embedded_editor(
        &mut self,
        id: InstanceId,
        parent: NativeParent,
    ) -> Result<bool, ManagerError> {
        assert_main_thread("PluginManager::begin_open_embedded_editor");
        let m = self.get(id)?;
        let open = m.instance.begin_open_embedded_gui(parent)?;
        m.editor_opening = !open;
        Ok(open)
    }

    /// True while an external editor is opening (see
    /// [`PluginManager::begin_open_external_editor`]).
    pub fn is_editor_opening(&self, id: InstanceId) -> bool {
        self.instances.get(&id).is_some_and(|m| m.editor_opening)
    }

    /// True when the instance's process is gone (sandbox): it cannot be restarted, only
    /// replaced by a new instance.
    pub fn is_lost(&self, id: InstanceId) -> bool {
        self.instances
            .get(&id)
            .is_some_and(|m| m.instance.is_lost())
    }

    /// Resizes the plugin's editor (the plugin may adjust the size).
    ///
    /// # Errors
    /// An unknown instance, or what the plugin said.
    pub fn resize_gui(
        &mut self,
        id: InstanceId,
        w: u32,
        h: u32,
    ) -> Result<(u32, u32), ManagerError> {
        assert_main_thread("PluginManager::resize_gui");
        Ok(self.get(id)?.instance.resize_gui(w, h)?)
    }

    /// Asks the plugin's editor to resize without waiting for a plugin that is busy (see
    /// [`PluginInstance::request_resize_gui`]): `Some(size)` is the size it took, `None` means
    /// the answer comes later through [`PluginManager::take_soft_answers`] as a
    /// [`SoftAnswer::GuiSized`].
    ///
    /// # Errors
    /// An unknown instance, or what the plugin said.
    pub fn request_resize_gui(
        &mut self,
        id: InstanceId,
        w: u32,
        h: u32,
    ) -> Result<Option<(u32, u32)>, ManagerError> {
        assert_main_thread("PluginManager::request_resize_gui");
        Ok(self.get(id)?.instance.request_resize_gui(w, h)?)
    }

    /// Closes the editor and captures the state it may have changed.
    ///
    /// # Errors
    /// An unknown instance.
    pub fn close_gui(&mut self, id: InstanceId) -> Result<Option<PluginStateBlob>, ManagerError> {
        assert_main_thread("PluginManager::close_gui");
        let m = self.get(id)?;
        m.instance.close_gui();
        m.capture = CaptureSlot::default();
        Ok(m.instance.capture_state().ok())
    }

    /// Closes the editor without capturing: the live app asks for the state afterwards with
    /// [`PluginManager::request_capture`], so a plugin that is busy does not hold it up.
    ///
    /// # Errors
    /// An unknown instance.
    pub fn close_editor(&mut self, id: InstanceId) -> Result<(), ManagerError> {
        assert_main_thread("PluginManager::close_editor");
        self.get(id)?.instance.close_gui();
        Ok(())
    }

    // ----- restarts ---------------------------------------------------------------------------------

    /// Begins a restart: the device hands its processor back at its next batch and
    /// [`PluginManager::poll`] finishes the job (deactivate, activate, resume). A plugin whose
    /// last re-activation failed is re-activated at once.
    ///
    /// # Errors
    /// An unknown instance, or a lost one (its process is gone: `BackendError::Failed`).
    pub fn restart(&mut self, id: InstanceId) -> Result<(), ManagerError> {
        assert_main_thread("PluginManager::restart");
        if self.get(id)?.instance.is_lost() {
            return Err(BackendError::Failed(LOST.to_string()).into());
        }
        self.begin_restart(id);
        Ok(())
    }

    /// RELOAD: the same as [`PluginManager::restart`]; a FAILED device is reset by the
    /// hand-shake.
    ///
    /// # Errors
    /// An unknown or a lost instance (see [`PluginManager::restart`]).
    pub fn reload(&mut self, id: InstanceId) -> Result<(), ManagerError> {
        self.restart(id)
    }

    /// The session sample rate changed: every instance restarts at the new rate.
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        assert_main_thread("PluginManager::set_sample_rate");
        let ids: Vec<InstanceId> = self.instances.keys().copied().collect();
        for id in ids {
            if let Some(m) = self.instances.get_mut(&id) {
                m.cfg.sample_rate = sample_rate;
            }
            self.begin_restart(id);
        }
    }

    fn begin_restart(&mut self, id: InstanceId) {
        let Some(m) = self.instances.get_mut(&id) else {
            return;
        };
        if m.parked || m.instance.is_lost() {
            return;
        }
        match m.phase {
            Phase::Running => {
                m.end.request_stop();
                m.phase = Phase::Stopping;
            }
            Phase::Stopping | Phase::Deactivating => {}
            // The activation in flight used the settings of before: once it is done the plugin
            // restarts again.
            Phase::Activating => m.again = true,
            Phase::Inactive => {
                // The plugin is inactive and the device has no processor: activate now.
                let mut events = Vec::new();
                Self::activate_and_resume(id, m, &mut events);
                self.queued.extend(events);
            }
        }
    }

    /// Starts re-activating the plugin after a restart: the device resumes now when the plugin
    /// activated at once (a plugin in this process), else when [`PluginManager::poll`] sees the
    /// answer ([`Phase::Activating`]; the device stays as it is, with no processor, until then).
    fn activate_and_resume(id: InstanceId, m: &mut Managed, events: &mut Vec<ManagerEvent>) {
        m.layout_changed = m.instance.take_layout_changed();
        match activate_now_or_later(m.instance.as_mut(), &m.cfg) {
            Ok(Some(processor)) => Self::resume(id, m, processor, events),
            Ok(None) => m.phase = Phase::Activating,
            Err(e) => Self::restart_failed(id, m, e.to_string(), events),
        }
    }

    fn restart_failed(
        id: InstanceId,
        m: &mut Managed,
        reason: String,
        events: &mut Vec<ManagerEvent>,
    ) {
        m.phase = Phase::Inactive;
        m.again = false;
        events.push(ManagerEvent::RestartFailed {
            instance: id,
            reason,
        });
    }

    /// The plugin is active again: the device takes the processor back.
    fn resume(
        id: InstanceId,
        m: &mut Managed,
        processor: Box<dyn PluginProcessor>,
        events: &mut Vec<ManagerEvent>,
    ) {
        let latency = m.instance.latency_frames();
        m.end.slot().set_latency(latency, m.cfg.sample_rate);
        if m.end.resume(processor).is_err() {
            // The ring cannot be full in the protocol; if it is, try again next time.
            Self::restart_failed(
                id,
                m,
                "the device did not take the processor".to_string(),
                events,
            );
            return;
        }
        m.phase = Phase::Running;
        events.push(ManagerEvent::Restarted(id));
        if latency != m.latency {
            m.latency = latency;
            events.push(ManagerEvent::LatencyChanged {
                instance: id,
                frames: latency,
            });
        }
        if std::mem::take(&mut m.layout_changed) {
            m.layout = listed_layout(m.instance.layout(), &m.pins);
            events.push(ManagerEvent::LayoutChanged {
                instance: id,
                layout: m.layout.clone(),
            });
        }
        if std::mem::take(&mut m.again) {
            m.end.request_stop();
            m.phase = Phase::Stopping;
        }
    }

    /// The plugin that [`Phase::Activating`] waited for answered.
    fn activated(
        id: InstanceId,
        m: &mut Managed,
        answer: Result<Box<dyn PluginProcessor>, BackendError>,
        reaper: &ReaperQueue,
        events: &mut Vec<ManagerEvent>,
    ) {
        let processor = match answer {
            Ok(processor) => processor,
            Err(e) => {
                if let Some(job) = &mut m.reattach {
                    job.ready = Some(Err(e.into()));
                    m.phase = Phase::Inactive;
                } else {
                    Self::restart_failed(id, m, e.to_string(), events);
                }
                return;
            }
        };
        if let Some(job) = &m.reattach
            && job.rate.to_bits() != m.cfg.sample_rate.to_bits()
        {
            // The activation that landed used another rate than the one now wanted (it was
            // started by a restart, or by an earlier request): the processor goes back and the
            // plugin activates again with the wanted one. The run mode and block size never
            // change after the instance was created.
            m.phase = Phase::Inactive;
            if !m.instance.begin_deactivate(processor) {
                m.phase = Phase::Deactivating;
            }
        } else if m.reattach.is_some() {
            Self::reattached(id, m, processor, reaper);
        } else if m.parked {
            // The instance was parked while it activated: nobody wants the processor.
            m.phase = Phase::Inactive;
            if !m.instance.begin_deactivate(processor) {
                m.phase = Phase::Deactivating;
            }
        } else {
            Self::resume(id, m, processor, events);
        }
    }

    // ----- engine restarts that keep the plugins ----------------------------------------------------

    /// Keeps the instance alive when its device goes away (an engine restart): the
    /// reaper then only deactivates it, and [`PluginManager::reattach`] builds a new device.
    pub fn retain(&mut self, id: InstanceId, retain: bool) {
        if let Some(m) = self.instances.get_mut(&id) {
            m.retained = retain;
        }
    }

    /// Activates a parked instance at `sample_rate` and wraps it in a new device. The instance
    /// is not created again.
    ///
    /// # Errors
    /// An unknown or live instance, or an activation failure.
    pub fn reattach(
        &mut self,
        id: InstanceId,
        sample_rate: f64,
    ) -> Result<PluginHandle, ManagerError> {
        assert_main_thread("PluginManager::reattach");
        let reaper = self.reaper.clone();
        let m = self.get(id)?;
        if !m.parked {
            return Err(ManagerError::NotParked(id));
        }
        if m.phase == Phase::Activating || m.reattach.is_some() {
            return Err(BackendError::Failed("the plugin is still activating".to_string()).into());
        }
        let (descriptor, layout) = descriptor_of(&m.plugin, m.instance.layout(), &m.pins)?;
        m.cfg.sample_rate = sample_rate;
        let processor = m.instance.activate(&m.cfg)?;
        let (device_end, stale) = m.end.new_device_end();
        drop(stale);
        let latency = m.instance.latency_frames();
        m.latency = latency;
        m.end.slot().set_latency(latency, sample_rate);
        m.layout = layout.clone();
        m.parked = false;
        m.phase = Phase::Running;
        let device = PluginDevice::new(descriptor, &layout, processor, device_end, sample_rate)
            .with_reaper(reaper);
        Ok(PluginHandle {
            id,
            device,
            layout,
            descriptor,
            restore_error: None,
            timing: CreateTiming::default(),
        })
    }

    /// [`PluginManager::reattach`] for the live app, which does not wait for a plugin that is
    /// busy: `Some(handle)` when the instance was activated at once (a plugin in this process),
    /// `None` when its deactivation or activation is still going on. Call it again on a later
    /// frame: [`PluginManager::poll`] moves the instance on in between, and the call that finds
    /// it active gives the handle. Until then the instance stays parked.
    ///
    /// # Errors
    /// An unknown or live instance ([`ManagerError::NotParked`]: the reaper has not parked it
    /// yet, ask again on the next frame), or an activation failure.
    pub fn begin_reattach(
        &mut self,
        id: InstanceId,
        sample_rate: f64,
    ) -> Result<Option<PluginHandle>, ManagerError> {
        assert_main_thread("PluginManager::begin_reattach");
        let reaper = self.reaper.clone();
        let m = self.get(id)?;
        // A reattach that finished has unparked the instance but its device is not collected yet.
        if !m.parked && m.reattach.is_none() {
            return Err(ManagerError::NotParked(id));
        }
        m.reattach
            .get_or_insert(Reattach {
                rate: sample_rate,
                ready: None,
            })
            .rate = sample_rate;
        Self::step_reattach(id, m, &reaper);
        match m.reattach.as_mut().and_then(|job| job.ready.take()) {
            Some(answer) => {
                m.reattach = None;
                answer.map(Some)
            }
            None => Ok(None),
        }
    }

    /// Moves a reattach on: once the plugin is inactive it is activated at the new rate.
    fn step_reattach(id: InstanceId, m: &mut Managed, reaper: &ReaperQueue) {
        let Some(job) = &mut m.reattach else {
            return;
        };
        if m.phase != Phase::Inactive || job.ready.is_some() {
            return;
        }
        // A layout that cannot be a device fails before the plugin is activated.
        if let Err(e) = descriptor_of(&m.plugin, m.instance.layout(), &m.pins) {
            job.ready = Some(Err(e.into()));
            return;
        }
        m.cfg.sample_rate = job.rate;
        match activate_now_or_later(m.instance.as_mut(), &m.cfg) {
            Ok(Some(processor)) => Self::reattached(id, m, processor, reaper),
            Ok(None) => m.phase = Phase::Activating,
            Err(e) => job.ready = Some(Err(e.into())),
        }
    }

    /// The parked plugin is active: a new device takes its processor.
    fn reattached(
        id: InstanceId,
        m: &mut Managed,
        processor: Box<dyn PluginProcessor>,
        reaper: &ReaperQueue,
    ) {
        let Some(job) = &mut m.reattach else {
            return;
        };
        let (descriptor, layout) = match descriptor_of(&m.plugin, m.instance.layout(), &m.pins) {
            Ok(prepared) => prepared,
            Err(e) => {
                job.ready = Some(Err(e.into()));
                m.phase = Phase::Inactive;
                return;
            }
        };
        let (device_end, stale) = m.end.new_device_end();
        drop(stale);
        let latency = m.instance.latency_frames();
        m.latency = latency;
        m.end.slot().set_latency(latency, m.cfg.sample_rate);
        m.layout = layout.clone();
        m.parked = false;
        m.phase = Phase::Running;
        let device = PluginDevice::new(
            descriptor,
            &layout,
            processor,
            device_end,
            m.cfg.sample_rate,
        )
        .with_reaper(reaper.clone());
        job.ready = Some(Ok(PluginHandle {
            id,
            device,
            layout,
            descriptor,
            restore_error: None,
            timing: CreateTiming::default(),
        }));
    }

    /// Destroys a parked instance for good. Its plugin is deactivated and its process ended by
    /// the next polls, not waited for.
    ///
    /// # Errors
    /// An unknown instance, or one that is live in the engine ([`ManagerError::InUse`]).
    pub fn remove(&mut self, id: InstanceId) -> Result<(), ManagerError> {
        assert_main_thread("PluginManager::remove");
        let m = self.get(id)?;
        // A reattach that finished has unparked the instance, but its device is not taken yet.
        if !(m.parked || m.reattach.is_some()) {
            return Err(ManagerError::InUse(id));
        }
        if let Some(Reattach {
            ready: Some(Ok(handle)),
            ..
        }) = m.reattach.take()
        {
            // Its processor is out in that device: the reaper ends the instance on the next poll.
            m.retained = false;
            drop(handle);
            return Ok(());
        }
        if let Some(m) = self.instances.remove(&id) {
            self.leave(id, m, None);
        }
        self.queued.push(ManagerEvent::Destroyed(id));
        self.advance_leaving();
        Ok(())
    }

    /// Hands an instance that is gone from the manager to the leaving list: what it still waits
    /// for (an activation, a deactivation) is carried on by [`PluginManager::poll`]. A plugin
    /// whose deactivation `processor` just started is `deactivating`.
    fn leave(&mut self, id: InstanceId, m: Managed, deactivating: Option<bool>) {
        let step = match (deactivating, m.phase) {
            (Some(true), _) => Leave::Ending,
            (Some(false), _) | (None, Phase::Deactivating) => Leave::Deactivating,
            (None, Phase::Activating) => Leave::Activating,
            (None, _) => Leave::Ending,
        };
        self.leaving.push(Leaving {
            id,
            what: Leaver::Instance(m.instance),
            step,
            begun: false,
            end: Some(m.end),
        });
    }

    /// Moves the instances that are leaving on; the ones that are done are dropped.
    fn advance_leaving(&mut self) {
        let stalls = &mut self.stalls;
        self.leaving.retain_mut(|l| !l.advance(stalls));
    }

    // ----- the per-frame work -----------------------------------------------------------------------

    /// The stall records collected by [`PluginManager::poll`] since the last call, oldest first
    /// (at most the newest 64). Empty, without allocating, when there are none. Only a live app
    /// reads them; offline users never do.
    pub fn take_stalls(&mut self) -> Vec<StallRecord> {
        assert_main_thread("PluginManager::take_stalls");
        std::mem::take(&mut self.stalls)
    }

    /// The cosmetic answers that arrived since the last call, oldest first (at most the newest
    /// 256): the parameter texts and editor sizes a plugin in another process answered after the
    /// call that asked had returned (see [`SoftAnswer`]). Empty, without allocating, when there
    /// are none.
    pub fn take_soft_answers(&mut self) -> Vec<(InstanceId, SoftAnswer)> {
        assert_main_thread("PluginManager::take_soft_answers");
        std::mem::take(&mut self.soft_answers)
    }

    /// The allowance of waiting for cosmetic answers that the instances of this manager share
    /// (a new frame begins at every [`PluginManager::poll`]).
    pub fn soft_budget(&self) -> &SoftBudget {
        &self.soft
    }

    /// Runs the main-thread work of every instance: reaps dropped devices, runs the plugins'
    /// callbacks and timers, translates their messages into events and advances restarts.
    /// Called every UI frame, which is also what begins a frame of the allowance for waiting
    /// for cosmetic answers.
    pub fn poll(&mut self) -> Vec<ManagerEvent> {
        assert_main_thread("PluginManager::poll");
        self.soft.begin_frame();
        let mut events = std::mem::take(&mut self.queued);
        // Everything one device left behind is handled together: its own processor, and one it
        // handed back or was sent but never took.
        let mut left: BTreeMap<InstanceId, Vec<Box<dyn PluginProcessor>>> = BTreeMap::new();
        for item in self.reaper.drain() {
            let entry = left.entry(item.slot.id()).or_default();
            entry.extend(item.processor);
        }
        for (id, processors) in left {
            self.reap(id, processors, &mut events);
        }
        // The instances that ended are deactivated and their processes end together, across
        // polls, so that a project with dozens of plugins waits for none of its slow ones.
        self.advance_leaving();
        let ids: Vec<InstanceId> = self.instances.keys().copied().collect();
        for id in ids {
            self.poll_one(id, &mut events);
        }
        events
    }

    /// A device went away. The instance is parked when an engine restart keeps it, else it is
    /// gone for good ([`ManagerEvent::Destroyed`]) and goes to the leaving list. Its plugin is
    /// deactivated without waiting for it (an offline instance waits: a bounce must see the
    /// same thing in every run).
    fn reap(
        &mut self,
        id: InstanceId,
        mut processors: Vec<Box<dyn PluginProcessor>>,
        events: &mut Vec<ManagerEvent>,
    ) {
        let Some(m) = self.instances.get_mut(&id) else {
            return;
        };
        processors.extend(m.end.take_processor());
        // At most one of them belongs to the active plugin; the others are dropped first so
        // the backend can deactivate with the last one.
        let active = processors.pop();
        drop(processors);
        // `Some(done)`: the deactivation was started and `done` says whether it is over.
        let deactivation = active.map(|p| {
            if m.cfg.mode == RunMode::Offline {
                m.instance.deactivate(p);
                true
            } else {
                m.instance.begin_deactivate(p)
            }
        });
        if m.retained {
            m.parked = true;
            m.phase = match (deactivation, m.phase) {
                (Some(false), _) | (None, Phase::Deactivating) => Phase::Deactivating,
                (None, Phase::Activating) => Phase::Activating,
                _ => Phase::Inactive,
            };
            return;
        }
        if let Some(m) = self.instances.remove(&id) {
            self.leave(id, m, deactivation);
        }
        events.push(ManagerEvent::Destroyed(id));
    }

    fn poll_one(&mut self, id: InstanceId, events: &mut Vec<ManagerEvent>) {
        let reaper = self.reaper.clone();
        let Some(m) = self.instances.get_mut(&id) else {
            return;
        };
        // A parked instance is polled too: with audio off an instance lives parked and its
        // editor and timers must keep running.
        m.instance.poll();
        for answer in m.instance.take_soft_answers() {
            if self.soft_answers.len() >= MAX_SOFT_ANSWERS {
                self.soft_answers.remove(0);
            }
            self.soft_answers.push((id, answer));
        }
        // Only a live plugin that kept running past a timeout is ever in this state: an offline
        // render never gets here, so the event stream of a bounce does not depend on the clock.
        let stuck = m.instance.not_responding();
        if stuck != m.not_responding {
            match (&m.not_responding, &stuck) {
                (None, Some(call)) => events.push(ManagerEvent::NotResponding {
                    instance: id,
                    call: call.clone(),
                }),
                (Some(_), None) => events.push(ManagerEvent::Responding(id)),
                _ => {}
            }
            m.not_responding = stuck;
        }
        let mut restart = false;
        while let Some(msg) = m.end.try_recv() {
            match msg {
                PluginMessage::ParamEdit {
                    id: param,
                    value,
                    phase,
                } => {
                    events.push(ManagerEvent::ParamEdit {
                        instance: id,
                        param,
                        value,
                        phase,
                    });
                }
                PluginMessage::StateDirty => events.push(ManagerEvent::StateDirty(id)),
                PluginMessage::LatencyChanged
                | PluginMessage::RescanParams
                | PluginMessage::RequestRestart => restart = true,
                PluginMessage::RequestCallback => {}
                PluginMessage::GuiResize { w, h } => {
                    events.push(ManagerEvent::GuiResize { instance: id, w, h })
                }
                PluginMessage::GuiFrame { x, y, w, h } => events.push(ManagerEvent::GuiFrame {
                    instance: id,
                    x,
                    y,
                    w,
                    h,
                }),
                PluginMessage::GuiClosed => events.push(ManagerEvent::GuiClosed(id)),
                PluginMessage::GuiRequestShow => events.push(ManagerEvent::GuiRequestShow(id)),
                PluginMessage::GuiRequestHide => events.push(ManagerEvent::GuiRequestHide(id)),
                PluginMessage::Log { level, text } => events.push(ManagerEvent::Log {
                    instance: id,
                    level,
                    text,
                }),
                PluginMessage::Refused { batches } => events.push(ManagerEvent::Log {
                    instance: id,
                    level: LogLevel::Warning,
                    text: format!(
                        "the plugin did not take {batches} audio block(s) (it answered a non-OK result, as plugins do while they load or sign in); the device plays on, silent for those blocks"
                    ),
                }),
                PluginMessage::Stall { call, ms, outcome } => push_stall(
                    &mut self.stalls,
                    StallRecord {
                        instance: id,
                        call,
                        ms,
                        outcome,
                    },
                ),
                PluginMessage::Failed { reason } => events.push(ManagerEvent::Failed {
                    instance: id,
                    reason: reason.to_string(),
                }),
                PluginMessage::ProcessExited { reason } => {
                    events.push(ManagerEvent::Crashed {
                        instance: id,
                        reason,
                    });
                }
            }
        }
        if m.editor_opening
            && let Some(answer) = m.instance.poll_open_external_gui()
        {
            m.editor_opening = false;
            events.push(match answer {
                Ok(()) => ManagerEvent::EditorOpened(id),
                Err(e) => ManagerEvent::EditorFailed {
                    instance: id,
                    reason: e.to_string(),
                },
            });
        }
        if m.instance.is_lost() {
            m.editor_opening = false;
            // Nothing to restart: the process is gone. A processor the device handed back is
            // given to the instance, which only drops it.
            if let Some(processor) = m.end.take_processor() {
                m.instance.deactivate(processor);
            }
            return;
        }
        // While its editor opens the plugin answers nothing on its main thread: a restart
        // waits for it, rather than fail.
        restart |= std::mem::take(&mut m.deferred_restart);
        if m.editor_opening {
            m.deferred_restart = restart;
            return;
        }
        if restart && !m.parked && m.phase == Phase::Running {
            m.end.request_stop();
            m.phase = Phase::Stopping;
        } else if restart && !m.parked && m.phase == Phase::Activating {
            m.again = true;
        }
        if m.phase == Phase::Stopping
            && let Some(processor) = m.end.take_processor()
        {
            // An offline render (a bounce's pre-roll) must see the restart done at the same
            // batch in every process: it waits for the plugin; only a live restart is polled.
            let done = if m.cfg.mode == RunMode::Offline {
                m.instance.deactivate(processor);
                true
            } else {
                m.instance.begin_deactivate(processor)
            };
            if done {
                Self::activate_and_resume(id, m, events);
            } else {
                m.phase = Phase::Deactivating;
            }
        }
        if m.phase == Phase::Deactivating && m.instance.poll_deactivate() {
            if m.parked {
                m.phase = Phase::Inactive;
            } else {
                Self::activate_and_resume(id, m, events);
            }
        }
        if m.phase == Phase::Activating
            && let Some(answer) = m.instance.poll_activate()
        {
            Self::activated(id, m, answer, &reaper, events);
        }
        Self::step_reattach(id, m, &reaper);
    }
}

/// The longest [`shut_down_together`] waits for processes that do not end.
const SHUTDOWN_BOUND: Duration = Duration::from_secs(10);

/// Starts the end of every instance, then waits for all of them at once (the cost is the
/// slowest process, not the sum; a process that does not end when asked is stopped by its
/// instance after the grace it allows), then drops them.
fn shut_down_together(mut doomed: Vec<Managed>, mut leaving: Vec<Leaving>) {
    let began = clock();
    let mut scratch = Vec::new();
    loop {
        let mut all = true;
        for m in &mut doomed {
            m.instance.begin_shutdown();
            all &= m.instance.poll_shutdown();
        }
        for l in &mut leaving {
            all &= l.end_step(&mut scratch);
        }
        if all || clock().saturating_duration_since(began) >= SHUTDOWN_BOUND {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(doomed);
    drop(leaving);
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        let mut leaving = std::mem::take(&mut self.leaving);
        // A plugin that was still starting ends with the rest.
        for (_, creating) in std::mem::take(&mut self.creating) {
            leaving.extend(Leaving::of_creation(creating.stage));
        }
        shut_down_together(
            std::mem::take(&mut self.instances).into_values().collect(),
            leaving,
        );
    }
}

impl PluginProvider for PluginManager {
    fn instantiate(&mut self, request: &InstantiateRequest) -> Result<PluginHandle, ManagerError> {
        PluginManager::instantiate(self, request)
    }
}
