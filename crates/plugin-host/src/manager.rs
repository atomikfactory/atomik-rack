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
//! Every entry point is `[main-thread]` and checks it in debug builds.

use crate::backend::{
    ActivateConfig, BackendError, HostHandle, NativeParent, PluginBackend, PluginInstance,
    PluginProcessor,
};
use crate::device::PluginDevice;
use crate::layout::truncate_params;
use crate::slot::{
    InstanceId, LogLevel, ManagerEnd, ParamPhase, PluginMessage, PluginSlot, ReaperQueue,
    plugin_channel,
};
use crate::threads::assert_main_thread;
use atomik_core::BATCH;
use atomik_device_sdk::{
    DescriptorError, DeviceDescriptor, PluginFormat, PluginLayout, PluginRef, PluginStateBlob,
    RunMode, descriptor_for,
};
use std::collections::BTreeMap;

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

/// Something that happened to an instance, for the app to act on.
#[derive(Debug, Clone, PartialEq)]
pub enum ManagerEvent {
    /// A parameter edit made inside the plugin's window (`value` is NaN for a gesture's
    /// begin and end markers).
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
}

/// The main-thread owner of the plugin instances.
#[derive(Default)]
pub struct PluginManager {
    backends: Vec<Box<dyn PluginBackend>>,
    instances: BTreeMap<InstanceId, Managed>,
    reaper: ReaperQueue,
    queued: Vec<ManagerEvent>,
    next_id: u64,
}

fn descriptor_of(
    plugin: &PluginRef,
    layout: &PluginLayout,
) -> Result<(&'static DeviceDescriptor, PluginLayout), DescriptorError> {
    let mut layout = layout.clone();
    truncate_params(&mut layout);
    descriptor_for(plugin, &layout).map(|d| (d, layout))
}

impl PluginManager {
    /// A manager with no backends.
    pub fn new() -> Self {
        Self {
            next_id: 1,
            ..Self::default()
        }
    }

    /// Adds a format backend.
    pub fn add_backend(&mut self, backend: Box<dyn PluginBackend>) {
        self.backends.push(backend);
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
        let mut instance =
            backend.instantiate(&request.plugin, HostHandle::new(end.messenger()))?;
        // State first (so a latency that depends on it is right at activation), rack params
        // after (the device re-sends every param the rack sets).
        let restore_error = request
            .state
            .as_ref()
            .and_then(|s| instance.restore_state(s).err())
            .map(|e| e.to_string());
        let (descriptor, layout) = descriptor_of(&request.plugin, instance.layout())?;
        let cfg = ActivateConfig {
            sample_rate: request.sample_rate,
            frames: BATCH as u32,
            mode: request.mode,
        };
        let processor = instance.activate(&cfg)?;
        self.next_id += 1;
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
            },
        );
        Ok(PluginHandle {
            id,
            device,
            layout,
            descriptor,
            restore_error,
        })
    }

    // ----- state, text, editor --------------------------------------------------------------------

    /// Saves the plugin's state.
    ///
    /// # Errors
    /// An unknown instance, or what the plugin said.
    pub fn capture_state(&mut self, id: InstanceId) -> Result<PluginStateBlob, ManagerError> {
        assert_main_thread("PluginManager::capture_state");
        Ok(self.get(id)?.instance.capture_state()?)
    }

    /// Saves every instance's state (before a save a).
    pub fn capture_all(&mut self) -> Vec<(InstanceId, Result<PluginStateBlob, ManagerError>)> {
        assert_main_thread("PluginManager::capture_all");
        self.instances
            .iter_mut()
            .map(|(id, m)| (*id, m.instance.capture_state().map_err(ManagerError::from)))
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
        Ok(self.get(id)?.instance.restore_state(state)?)
    }

    /// The plugin's display text of a param value (never asked on the audio thread).
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

    /// Closes the editor and captures the state it may have changed.
    ///
    /// # Errors
    /// An unknown instance.
    pub fn close_gui(&mut self, id: InstanceId) -> Result<Option<PluginStateBlob>, ManagerError> {
        assert_main_thread("PluginManager::close_gui");
        let m = self.get(id)?;
        m.instance.close_gui();
        Ok(m.instance.capture_state().ok())
    }

    // ----- restarts ---------------------------------------------------------------------------------

    /// Begins a restart: the device hands its processor back at its next batch and
    /// [`PluginManager::poll`] finishes the job (deactivate, activate, resume). A plugin whose
    /// last re-activation failed is re-activated at once.
    ///
    /// # Errors
    /// An unknown instance.
    pub fn restart(&mut self, id: InstanceId) -> Result<(), ManagerError> {
        assert_main_thread("PluginManager::restart");
        self.get(id)?;
        self.begin_restart(id);
        Ok(())
    }

    /// RELOAD: the same as [`PluginManager::restart`]; a FAILED device is reset by the
    /// hand-shake.
    ///
    /// # Errors
    /// An unknown instance.
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
        if m.parked {
            return;
        }
        match m.phase {
            Phase::Running => {
                m.end.request_stop();
                m.phase = Phase::Stopping;
            }
            Phase::Stopping => {}
            Phase::Inactive => {
                // The plugin is inactive and the device has no processor: activate now.
                let mut events = Vec::new();
                Self::activate_and_resume(id, m, &mut events);
                self.queued.extend(events);
            }
        }
    }

    fn activate_and_resume(id: InstanceId, m: &mut Managed, events: &mut Vec<ManagerEvent>) {
        let layout_changed = m.instance.take_layout_changed();
        match m.instance.activate(&m.cfg) {
            Ok(processor) => {
                let latency = m.instance.latency_frames();
                m.end.slot().set_latency(latency, m.cfg.sample_rate);
                if m.end.resume(processor).is_err() {
                    // The ring cannot be full in the protocol; if it is, try again next time.
                    m.phase = Phase::Inactive;
                    events.push(ManagerEvent::RestartFailed {
                        instance: id,
                        reason: "the device did not take the processor".to_string(),
                    });
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
                if layout_changed {
                    m.layout = m.instance.layout().clone();
                    events.push(ManagerEvent::LayoutChanged {
                        instance: id,
                        layout: m.layout.clone(),
                    });
                }
            }
            Err(e) => {
                m.phase = Phase::Inactive;
                events.push(ManagerEvent::RestartFailed {
                    instance: id,
                    reason: e.to_string(),
                });
            }
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
        let (descriptor, layout) = descriptor_of(&m.plugin, m.instance.layout())?;
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
        })
    }

    /// Destroys a parked instance for good.
    ///
    /// # Errors
    /// An unknown instance, or one that is live in the engine ([`ManagerError::InUse`]).
    pub fn remove(&mut self, id: InstanceId) -> Result<(), ManagerError> {
        assert_main_thread("PluginManager::remove");
        if !self.get(id)?.parked {
            return Err(ManagerError::InUse(id));
        }
        self.instances.remove(&id);
        self.queued.push(ManagerEvent::Destroyed(id));
        Ok(())
    }

    // ----- the per-frame work -----------------------------------------------------------------------

    /// Runs the main-thread work of every instance: reaps dropped devices, runs the plugins'
    /// callbacks and timers, translates their messages into events and advances restarts.
    /// Called every UI frame.
    pub fn poll(&mut self) -> Vec<ManagerEvent> {
        assert_main_thread("PluginManager::poll");
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
        let ids: Vec<InstanceId> = self.instances.keys().copied().collect();
        for id in ids {
            self.poll_one(id, &mut events);
        }
        events
    }

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
        if let Some(p) = active {
            m.instance.deactivate(p);
        }
        if m.retained {
            m.parked = true;
            m.phase = Phase::Inactive;
            return;
        }
        self.instances.remove(&id);
        events.push(ManagerEvent::Destroyed(id));
    }

    fn poll_one(&mut self, id: InstanceId, events: &mut Vec<ManagerEvent>) {
        let Some(m) = self.instances.get_mut(&id) else {
            return;
        };
        // A parked instance is polled too: with audio off an instance lives parked and its
        // editor and timers must keep running.
        m.instance.poll();
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
                PluginMessage::GuiClosed => events.push(ManagerEvent::GuiClosed(id)),
                PluginMessage::GuiRequestShow => events.push(ManagerEvent::GuiRequestShow(id)),
                PluginMessage::GuiRequestHide => events.push(ManagerEvent::GuiRequestHide(id)),
                PluginMessage::Log { level, text } => events.push(ManagerEvent::Log {
                    instance: id,
                    level,
                    text,
                }),
                PluginMessage::Failed { reason } => events.push(ManagerEvent::Failed {
                    instance: id,
                    reason: reason.to_string(),
                }),
            }
        }
        if restart && !m.parked && m.phase == Phase::Running {
            m.end.request_stop();
            m.phase = Phase::Stopping;
        }
        if m.phase == Phase::Stopping
            && let Some(processor) = m.end.take_processor()
        {
            m.instance.deactivate(processor);
            Self::activate_and_resume(id, m, events);
        }
    }
}

impl PluginProvider for PluginManager {
    fn instantiate(&mut self, request: &InstantiateRequest) -> Result<PluginHandle, ManagerError> {
        PluginManager::instantiate(self, request)
    }
}
