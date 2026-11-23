//! `Vst3Instance`: the main-thread half of one VST3 plugin. Every method here is `[main-thread]` and starts with `assert_main_thread`.
//!
//! Creation follows the hosting sequence of the VST3 documentation: create the component, initialise
//! it with the host context, find the controller (a separate class or the same object), connect
//! the two through proxies, give the controller the component handler, bring the controller in
//! step with the component's state, negotiate the buses, read the parameters.
//!
//! * From the moment the component initialised, every way out of `create` terminates it (and a
//!   separate controller that initialised) before the references go: [`Initialised`] does it
//!   until the instance owns the plugin, and the instance's `Drop` does it after.
//! * A controller that is a class of its own must initialise: one that refuses fails the plugin
//!   (released without `terminate`, the component terminated). A controller class the factory
//!   cannot create leaves the plugin without a controller. A controller class the factory
//!   answers with the component's own object is a single-object plugin: no second `initialize`,
//!   no proxies, one `terminate`.
//! * Teardown order: editor closed and handler detached, `setActive(0)` when active, each half
//!   `disconnect`ed with the proxy it was connected with, both proxies' destinations cleared (so
//!   the ring component -> proxy -> controller -> proxy -> component is broken on the host's side
//!   whatever the plugin does in `disconnect`), the controller terminated (when it is a class of
//!   its own), then the component.

use crate::host::{ComponentHandler, ConnectionProxy, HostApplication, Hub, PlugFrame};
use crate::ids;
use crate::layout::{self, ClassInfo, Read, owned};
use crate::module::Module;
use crate::presets;
use crate::processor::{self, Vst3Processor};
use crate::stream::MemStream;
use crate::util::utf16_text;
use crate::values;
use atomik_device_sdk::{LayoutChange, PluginLayout, PluginStateBlob, RunMode};
use atomik_plugin_host::{
    ActivateConfig, BackendError, LogLevel, NativeParent, PluginInstance as HostInstance,
    PluginMessage, PluginProcessor, PresetInfo, PresetLister, assert_main_thread, is_main_thread,
};
use std::ffi::{c_char, c_void};
use std::sync::Arc;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{ComPtr, ComWrapper, Interface};

/// Capacity of the ring that carries edits made in the plugin's editor to the processor.
const EDIT_RING: usize = 1_024;

fn failed(what: &str, e: impl std::fmt::Display) -> BackendError {
    BackendError::Failed(format!("{what}: {e}"))
}

fn ok(r: tresult) -> bool {
    r == kResultOk
}

/// The magic that starts a state blob.
const STATE_MAGIC: &[u8; 4] = b"AVS1";

/// Packs the component state and the controller state into one blob: the magic, then each
/// section as a little-endian `u32` length and its bytes.
pub(crate) fn pack_state(component: &[u8], controller: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + component.len() + controller.len());
    out.extend_from_slice(STATE_MAGIC);
    for section in [component, controller] {
        out.extend_from_slice(&(section.len() as u32).to_le_bytes());
        out.extend_from_slice(section);
    }
    out
}

/// The inverse of [`pack_state`]; `None` when the blob is not one of ours or is cut short.
pub(crate) fn unpack_state(blob: &[u8]) -> Option<(&[u8], &[u8])> {
    let rest = blob.strip_prefix(STATE_MAGIC.as_slice())?;
    fn take(data: &[u8]) -> Option<(&[u8], &[u8])> {
        let (len, tail) = data.split_first_chunk::<4>()?;
        let len = u32::from_le_bytes(*len) as usize;
        (tail.len() >= len).then(|| tail.split_at(len))
    }
    let (component, rest) = take(rest)?;
    let (controller, rest) = take(rest)?;
    rest.is_empty().then_some((component, controller))
}

/// Terminates a separate controller (when there is one) and then the component: the end of
/// every path that has initialised them.
fn terminate_halves(controller: Option<&ComPtr<IEditController>>, component: &ComPtr<IComponent>) {
    if let Some(c) = controller {
        // SAFETY: a live controller that initialised, terminated once.
        unsafe { c.terminate() };
    }
    // SAFETY: a live component that initialised, terminated once.
    unsafe { component.terminate() };
}

/// What `create` has initialised and not yet handed to the instance: dropped armed (an early
/// return of any kind), it terminates the halves before the references it sits beside are
/// released. The instance disarms it when it takes over (its own `Drop` terminates from then on).
struct Initialised {
    component: ComPtr<IComponent>,
    /// A separate controller that initialised.
    controller: Option<ComPtr<IEditController>>,
    armed: bool,
}

impl Drop for Initialised {
    fn drop(&mut self) {
        if self.armed {
            terminate_halves(self.controller.as_ref(), &self.component);
        }
    }
}

/// Whether two interface pointers belong to one object (COM identity: the `FUnknown` of each).
fn same_object<A: Interface, B: Interface>(a: &ComPtr<A>, b: &ComPtr<B>) -> bool {
    match (a.cast::<FUnknown>(), b.cast::<FUnknown>()) {
        (Some(x), Some(y)) => x.as_ptr() == y.as_ptr(),
        _ => false,
    }
}

/// The `PluginInstance` of a VST3 plugin.
pub(crate) struct Vst3Instance {
    // Declaration order is drop order: COM pointers first, the module last.
    view: Option<ComPtr<IPlugView>>,
    proxies: Option<(ComWrapper<ConnectionProxy>, ComWrapper<ConnectionProxy>)>,
    controller: Option<ComPtr<IEditController>>,
    processor: ComPtr<IAudioProcessor>,
    component: ComPtr<IComponent>,
    separate_controller: bool,
    frame: ComWrapper<PlugFrame>,
    handler: ComWrapper<ComponentHandler>,
    host_context: ComPtr<FUnknown>,
    _host_app: ComWrapper<HostApplication>,
    hub: Arc<Hub>,
    mirror: Option<rtrb::Consumer<(u32, f64)>>,
    /// The values one drain of the mirror ring pops (never grows past the ring's size).
    mirror_buf: Vec<(u32, u32, f64)>,
    class: ClassInfo,
    read: Read,
    layout_changed: bool,
    latency: u32,
    /// The latency the running activation was made with (`latency` follows a restored state at
    /// once; the device only follows a restart): what a latency request is compared with.
    activated_latency: u32,
    active: bool,
    gui_open: bool,
    has_view: bool,
    pub(crate) preset_roots: Option<Vec<std::path::PathBuf>>,
    _module: Arc<Module>,
}

impl Vst3Instance {
    /// Creates (and initialises) the plugin of `class` in `module`.
    pub(crate) fn create(
        module: &Arc<Module>,
        class: &ClassInfo,
        hub: Arc<Hub>,
    ) -> Result<Self, BackendError> {
        assert_main_thread("creating a VST3 plugin");
        let factory = module
            .factory()
            .ok_or_else(|| BackendError::NotAPlugin("the module has no factory".to_string()))?;
        let create = |cid: &TUID, iid: &[u8; 16]| -> Option<*mut c_void> {
            let mut obj: *mut c_void = std::ptr::null_mut();
            // SAFETY: a live factory; the ids are 16 bytes; the out pointer is valid.
            let r = unsafe {
                factory.createInstance(
                    cid.as_ptr().cast::<c_char>(),
                    iid.as_ptr().cast::<c_char>(),
                    &mut obj,
                )
            };
            (ok(r) && !obj.is_null()).then_some(obj)
        };

        let host_app = ComWrapper::new(HostApplication);
        let host_context = host_app
            .to_com_ptr::<FUnknown>()
            .ok_or_else(|| failed("the host context", "cannot be made"))?;

        let obj = create(&class.cid, &IComponent::IID)
            .ok_or_else(|| failed("the plugin could not be created", "no IComponent"))?;
        // SAFETY: the factory returned one owned IComponent reference.
        let component = unsafe { owned::<IComponent>(obj) }
            .ok_or_else(|| failed("the plugin could not be created", "null component"))?;
        // SAFETY: a live component and a valid host context.
        if !ok(unsafe { component.initialize(host_context.as_ptr()) }) {
            return Err(BackendError::Unsupported(
                "the plugin refused to initialise".to_string(),
            ));
        }
        // From here on the component is initialised: every way out terminates it.
        let mut up = Initialised {
            component: component.clone(),
            controller: None,
            armed: true,
        };
        let processor = component.cast::<IAudioProcessor>().ok_or_else(|| {
            BackendError::Unsupported("the component is not an audio processor".to_string())
        })?;

        // The controller: a class of its own, or the component itself.
        let mut cc: TUID = [0; 16];
        // SAFETY: a live component and a valid out pointer.
        let has_class = ok(unsafe { component.getControllerClassId(&mut cc) }) && cc != [0; 16];
        let mut separate = false;
        let controller = if has_class {
            let made = create(&cc, &IEditController::IID).and_then(|obj| {
                // SAFETY: the factory returned one owned IEditController reference.
                unsafe { owned::<IEditController>(obj) }
            });
            match made {
                // The factory answered with the component's own object: one object, one
                // `initialize`, one `terminate`, like the plugin that is its own controller.
                Some(c) if same_object(&c, &component) => component.cast::<IEditController>(),
                Some(c) => {
                    // SAFETY: a live controller and a valid host context.
                    if !ok(unsafe { c.initialize(host_context.as_ptr()) }) {
                        // Released without `terminate`; the guard terminates the component.
                        return Err(BackendError::Unsupported(
                            "the plugin's controller refused to initialise".to_string(),
                        ));
                    }
                    separate = true;
                    up.controller = Some(c.clone());
                    Some(c)
                }
                None => None,
            }
        } else {
            component.cast::<IEditController>()
        };

        let handler = ComWrapper::new(ComponentHandler {
            hub: Arc::clone(&hub),
        });
        let frame = PlugFrame::new(Arc::clone(&hub));
        let mut proxies = None;
        if let Some(c) = &controller {
            if let Some(h) = handler.as_com_ref::<IComponentHandler>() {
                // SAFETY: a live controller and the handler's own interface pointer.
                unsafe { c.setComponentHandler(h.as_ptr()) };
            }
            if separate
                && let (Some(a), Some(b)) = (
                    component.cast::<IConnectionPoint>(),
                    c.cast::<IConnectionPoint>(),
                )
            {
                let to_controller = ConnectionProxy::new();
                let to_component = ConnectionProxy::new();
                to_controller.set_destination(Some(b.clone()));
                to_component.set_destination(Some(a.clone()));
                if let (Some(pa), Some(pb)) = (
                    to_controller.as_com_ref::<IConnectionPoint>(),
                    to_component.as_com_ref::<IConnectionPoint>(),
                ) {
                    // SAFETY: live connection points and the proxies' own interface pointers.
                    unsafe {
                        a.connect(pa.as_ptr());
                        b.connect(pb.as_ptr());
                    }
                }
                proxies = Some((to_controller, to_component));
            }
            // Bring the controller in step with the component's state.
            let stream = MemStream::new(Vec::new());
            if let Some(s) = stream.as_com_ref::<IBStream>() {
                // SAFETY: a live component and the stream's own interface pointer.
                if ok(unsafe { component.getState(s.as_ptr()) }) {
                    stream.rewind();
                    // SAFETY: a live controller and the same stream.
                    unsafe { c.setComponentState(s.as_ptr()) };
                }
            }
        }

        // From here on the instance owns the plugin, so every failure below tears it down the
        // way `Drop` does (terminate after initialise).
        up.armed = false;
        let mut instance = Self {
            view: None,
            proxies,
            controller,
            processor,
            component,
            separate_controller: separate,
            frame,
            handler,
            host_context,
            _host_app: host_app,
            hub,
            mirror: None,
            mirror_buf: Vec::with_capacity(EDIT_RING),
            class: class.clone(),
            read: layout::empty_read(class),
            layout_changed: false,
            latency: 0,
            activated_latency: 0,
            active: false,
            gui_open: false,
            has_view: false,
            preset_roots: None,
            _module: Arc::clone(module),
        };
        let plan = layout::negotiate(&instance.component, &instance.processor)
            .map_err(BackendError::Unsupported)?;
        let read = layout::read(class, plan, instance.controller.as_ref())
            .map_err(BackendError::Unsupported)?;
        instance.hub.set_steps(read.steps.clone());
        instance.read = read;
        instance.has_view = instance
            .controller
            .as_ref()
            .is_some_and(|c| make_view(c).is_some());
        Ok(instance)
    }

    /// Cuts the connection of a split plugin: each half is asked to disconnect from the proxy it
    /// was connected with (a plugin lets go of its peer only for the object it stored, so a null
    /// there would keep it), then both proxies forget their destinations whatever the plugin
    /// answered, which breaks the ring on the host's side.
    fn disconnect_halves(&mut self) {
        let Some((to_controller, to_component)) = self.proxies.take() else {
            return;
        };
        if let (Some(a), Some(c), Some(pa), Some(pb)) = (
            self.component.cast::<IConnectionPoint>(),
            self.controller
                .as_ref()
                .and_then(|c| c.cast::<IConnectionPoint>()),
            to_controller.as_com_ref::<IConnectionPoint>(),
            to_component.as_com_ref::<IConnectionPoint>(),
        ) {
            // SAFETY: live connection points and the proxies' own interface pointers, the ones
            // `create` connected them with.
            unsafe {
                a.disconnect(pa.as_ptr());
                c.disconnect(pb.as_ptr());
            }
        }
        to_controller.set_destination(None);
        to_component.set_destination(None);
    }

    /// The scan facts of this instance.
    pub(crate) fn read(&self) -> (PluginLayout, bool) {
        (self.read.layout.clone(), self.read.params_truncated)
    }

    /// A warning in the app's plugin log.
    fn say(&self, text: String) {
        self.hub.messenger.post(PluginMessage::Log {
            level: LogLevel::Warning,
            text,
        });
    }

    /// Re-reads buses, params and maps (the plugin is inactive).
    fn refresh_layout(&mut self) {
        let plan = match layout::negotiate(&self.component, &self.processor) {
            Ok(plan) => plan,
            Err(e) => {
                self.say(format!("the plugin's new buses cannot be hosted: {e}"));
                return;
            }
        };
        let read = match layout::read(&self.class, plan, self.controller.as_ref()) {
            Ok(read) => read,
            Err(e) => {
                self.say(format!("the plugin's new layout cannot be read: {e}"));
                return;
            }
        };
        if read.layout != self.read.layout {
            self.layout_changed = true;
        }
        self.hub.set_steps(read.steps.clone());
        self.read = read;
    }

    /// Answers what the plugin asked of the host since the last poll. Only a request that changes
    /// something reaches the manager: a latency that is the one reported, a table that reads the
    /// same and a table whose titles alone changed need no restart (the titles go to the app as
    /// they are). Nothing is asked of a plugin that is not active: its next activation reads
    /// everything again.
    fn answer_asks(&mut self) {
        use std::sync::atomic::Ordering;
        let restart = self.hub.restart_asked.swap(false, Ordering::AcqRel);
        let latency = self.hub.latency_asked.swap(false, Ordering::AcqRel);
        let params = self.hub.params_asked.swap(false, Ordering::AcqRel);
        if !self.active {
            return;
        }
        let mut post = Vec::new();
        if restart {
            post.push(PluginMessage::RequestRestart);
        } else if params {
            match self.reread_params() {
                LayoutChange::Same => {}
                LayoutChange::Titles => {
                    post.push(PluginMessage::ParamTitles(Box::new(
                        self.read.layout.clone(),
                    )));
                }
                LayoutChange::Structure => post.push(PluginMessage::RescanParams),
            }
        }
        // SAFETY: a live processor.
        if latency && unsafe { self.processor.getLatencySamples() } != self.activated_latency {
            post.push(PluginMessage::LatencyChanged);
        }
        for message in post {
            self.hub.messenger.post(message);
        }
    }

    /// Reads the param table of the running plugin again, as far as that needs no new buses:
    /// a table that reads the same or with other titles is taken over (the restart the plugin
    /// asked for is not needed); anything else is left for the restart. A plugin whose buses
    /// moved needs the restart.
    fn reread_params(&mut self) -> LayoutChange {
        if !layout::plan_is_current(&self.component, &self.processor, &self.read.plan) {
            return LayoutChange::Structure;
        }
        let Ok(read) = layout::read(
            &self.class,
            self.read.plan.clone(),
            self.controller.as_ref(),
        ) else {
            return LayoutChange::Structure;
        };
        let change = read.layout.change_from(&self.read.layout);
        if change == LayoutChange::Structure {
            return change;
        }
        self.hub
            .layout_dirty
            .store(false, std::sync::atomic::Ordering::Release);
        if change == LayoutChange::Titles {
            self.hub.set_steps(read.steps.clone());
            self.read = read;
        }
        change
    }

    /// Tells the controller the param values the rack set since the last call (the processor
    /// copied them into the mirror ring), so an open editor shows them. The whole ring is drained
    /// and the controller hears each parameter once, with the latest value the rack set.
    fn apply_mirrored(&mut self) {
        let (Some(ring), Some(c)) = (self.mirror.as_mut(), self.controller.as_ref()) else {
            return;
        };
        drain_latest(ring, &mut self.mirror_buf, |id, n| {
            // SAFETY: a live controller, on the main thread.
            unsafe { c.setParamNormalized(id, n) };
        });
    }

    /// The folders the presets of this plugin are looked for in.
    fn preset_roots(&self) -> Vec<std::path::PathBuf> {
        self.preset_roots
            .clone()
            .unwrap_or_else(|| presets::roots(&self.class.vendor, &self.class.name))
    }

    fn setup(&self, cfg: &ActivateConfig) -> ProcessSetup {
        ProcessSetup {
            processMode: match cfg.mode {
                RunMode::Realtime => ProcessModes_::kRealtime as i32,
                RunMode::Offline => ProcessModes_::kOffline as i32,
            },
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            maxSamplesPerBlock: cfg.frames as i32,
            sampleRate: cfg.sample_rate,
        }
    }

    fn state_of(&self, stream_for: impl Fn(*mut IBStream) -> bool) -> Option<Vec<u8>> {
        let stream = MemStream::new(Vec::new());
        let s = stream.as_com_ref::<IBStream>()?;
        stream_for(s.as_ptr()).then(|| stream.take_bytes())
    }
}

/// Pops everything now in `ring` and calls `apply` once per parameter id with the last value
/// popped for it, ids in ascending order. `buf` holds the popped entries (id, arrival number,
/// value) and keeps its capacity: one made as large as the ring is never grown here.
fn drain_latest(
    ring: &mut rtrb::Consumer<(u32, f64)>,
    buf: &mut Vec<(u32, u32, f64)>,
    mut apply: impl FnMut(u32, f64),
) {
    buf.clear();
    // What is in the ring now: the audio thread may keep pushing while this runs.
    for seq in 0..ring.slots() {
        let Ok((id, n)) = ring.pop() else { break };
        buf.push((id, seq as u32, n));
    }
    buf.sort_unstable_by_key(|&(id, seq, _)| (id, seq));
    let mut i = 0;
    while i < buf.len() {
        let id = buf[i].0;
        let mut last = i;
        while last + 1 < buf.len() && buf[last + 1].0 == id {
            last += 1;
        }
        apply(id, buf[last].2);
        i = last + 1;
    }
}

/// Asks a controller for its editor view (`"editor"`), if it has one.
fn make_view(controller: &ComPtr<IEditController>) -> Option<ComPtr<IPlugView>> {
    // SAFETY: a live controller and a NUL-terminated name.
    let ptr = unsafe { controller.createView(c"editor".as_ptr()) };
    // SAFETY: a non-null result is one owned IPlugView reference.
    unsafe { ComPtr::from_raw(ptr) }
}

impl HostInstance for Vst3Instance {
    fn layout(&self) -> &PluginLayout {
        &self.read.layout
    }

    fn activate(&mut self, cfg: &ActivateConfig) -> Result<Box<dyn PluginProcessor>, BackendError> {
        assert_main_thread("activating a VST3 plugin");
        let dirty = self
            .hub
            .layout_dirty
            .swap(false, std::sync::atomic::Ordering::AcqRel);
        let midi_dirty = self
            .hub
            .midi_dirty
            .swap(false, std::sync::atomic::Ordering::AcqRel);
        // A plugin whose buses moved without a restart request still gets a plan that matches
        // them: the processor's bus arrays must be what the plugin has.
        if dirty
            || midi_dirty
            || !layout::plan_is_current(&self.component, &self.processor, &self.read.plan)
        {
            self.refresh_layout();
        }
        let mut setup = self.setup(cfg);
        // SAFETY: a live processor and a valid setup.
        if !ok(unsafe { self.processor.setupProcessing(&mut setup) }) {
            return Err(failed(
                "the plugin could not be set up",
                "setupProcessing failed",
            ));
        }
        // SAFETY: a live component.
        if !ok(unsafe { self.component.setActive(1) }) {
            return Err(failed(
                "the plugin could not be activated",
                "setActive failed",
            ));
        }
        self.active = true;
        // SAFETY: a live processor.
        self.latency = unsafe { self.processor.getLatencySamples() };
        self.activated_latency = self.latency;
        // What the plugin asked for while it was being activated is what this activation did: it
        // has read the latency just now, and a restart asked for inside the call would only repeat
        // the call.
        self.hub
            .latency_asked
            .store(false, std::sync::atomic::Ordering::Release);
        self.hub
            .restart_asked
            .store(false, std::sync::atomic::Ordering::Release);
        let (producer, consumer) = rtrb::RingBuffer::new(EDIT_RING);
        self.hub.set_edit_sink(Some(producer));
        let (mirror_in, mirror_out) = rtrb::RingBuffer::new(EDIT_RING);
        self.mirror = Some(mirror_out);
        Ok(Box::new(Vst3Processor::new(processor::Setup {
            processor: self.processor.clone(),
            component: self.component.clone(),
            plan: self.read.plan.clone(),
            steps: self.read.steps.clone(),
            midi: self.read.midi.clone(),
            expressions: self.read.expressions,
            mode: cfg.mode,
            param_capacity: self.read.layout.params.len() + crate::layout::MIDI_CONTROLLERS + 16,
            out_ids: self.read.layout.params.iter().map(|p| p.id).collect(),
            hub: Arc::clone(&self.hub),
            edits: consumer,
            mirror: mirror_in,
            module: Arc::clone(&self._module),
        })))
    }

    fn deactivate(&mut self, mut processor: Box<dyn PluginProcessor>) {
        assert_main_thread("deactivating a VST3 plugin");
        processor.stop_processing();
        // Dropping the processor releases its COM references.
        drop(processor);
        self.apply_mirrored();
        self.mirror = None;
        self.hub.set_edit_sink(None);
        if self.active {
            // SAFETY: a live component.
            unsafe { self.component.setActive(0) };
            self.active = false;
        }
        // A table change the plugin asked for while it was running takes effect now.
        if self
            .hub
            .layout_dirty
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            self.refresh_layout();
        }
    }

    fn latency_frames(&self) -> u32 {
        self.latency
    }

    fn capture_state(&mut self) -> Result<PluginStateBlob, BackendError> {
        assert_main_thread("saving the state of a VST3 plugin");
        // The controller state must hold the values the rack set up to now.
        self.apply_mirrored();
        let component = self
            .state_of(|s| {
                // SAFETY: a live component and a valid stream.
                ok(unsafe { self.component.getState(s) })
            })
            .ok_or_else(|| failed("saving the state", "the component refused"))?;
        let controller = match &self.controller {
            Some(c) => self
                .state_of(|s| {
                    // SAFETY: a live controller and a valid stream.
                    let r = unsafe { c.getState(s) };
                    ok(r) || r == kNotImplemented
                })
                .unwrap_or_default(),
            None => Vec::new(),
        };
        Ok(PluginStateBlob(pack_state(&component, &controller)))
    }

    fn restore_state(&mut self, state: &PluginStateBlob) -> Result<(), BackendError> {
        assert_main_thread("loading the state of a VST3 plugin");
        let (component, controller) = unpack_state(&state.0)
            .ok_or_else(|| BackendError::Failed("not a VST3 state".to_string()))?;
        let stream = MemStream::new(component.to_vec());
        let s = stream
            .as_com_ref::<IBStream>()
            .ok_or_else(|| failed("loading the state", "no stream"))?;
        // SAFETY: a live component and a valid stream.
        if !ok(unsafe { self.component.setState(s.as_ptr()) }) {
            return Err(failed("loading the state", "the plugin refused it"));
        }
        if let Some(c) = &self.controller {
            stream.rewind();
            // SAFETY: a live controller and the same stream.
            unsafe { c.setComponentState(s.as_ptr()) };
            if !controller.is_empty() {
                let cs = MemStream::new(controller.to_vec());
                if let Some(p) = cs.as_com_ref::<IBStream>() {
                    // SAFETY: a live controller and a valid stream.
                    unsafe { c.setState(p.as_ptr()) };
                }
            }
        }
        // A state can change the latency; read it again.
        if self.active {
            // SAFETY: a live processor.
            self.latency = unsafe { self.processor.getLatencySamples() };
        }
        Ok(())
    }

    fn read_params(&mut self) -> Vec<(u32, f64)> {
        assert_main_thread("reading the params of a VST3 plugin");
        let Some(c) = self.controller.as_ref() else {
            return Vec::new();
        };
        let steps = self.hub.steps();
        self.read
            .layout
            .params
            .iter()
            .map(|p| {
                // SAFETY: a live controller, on the main thread.
                let n = unsafe { c.getParamNormalized(p.id) };
                (p.id, values::host_value(&steps, p.id, n))
            })
            .collect()
    }

    fn list_presets(&mut self) -> Vec<PresetInfo> {
        assert_main_thread("listing the presets of a VST3 plugin");
        presets::list(&self.preset_roots())
    }

    fn preset_lister(&mut self) -> Option<PresetLister> {
        // The folders are known from the class: the walk needs no plugin call.
        let roots = self.preset_roots();
        Some(Box::new(move || presets::list(&roots)))
    }

    fn load_preset(&mut self, id: &str) -> Result<(), BackendError> {
        assert_main_thread("loading a VST3 preset");
        let roots = self.preset_roots();
        let path = presets::resolve(&roots, id)
            .ok_or_else(|| BackendError::Failed(format!("no preset `{id}`")))?;
        let bytes = presets::read_file(&path).map_err(BackendError::Failed)?;
        let (component, controller) = presets::parse(&bytes, &ids::class_id_hex(&self.class.cid))
            .map_err(BackendError::Failed)?;
        self.restore_state(&PluginStateBlob(pack_state(&component, &controller)))
    }

    fn param_text(&mut self, id: u32, plain: f64) -> Option<String> {
        assert_main_thread("asking a VST3 plugin for a param text");
        let c = self.controller.as_ref()?;
        let n = values::normalized(&self.hub.steps(), id, plain);
        let mut text: String128 = [0; 128];
        // SAFETY: a live controller and a valid String128.
        if !ok(unsafe { c.getParamStringByValue(id, n, &mut text) }) {
            return None;
        }
        Some(utf16_text(&text))
    }

    fn open_gui(&mut self, parent: NativeParent, scale: f64) -> Result<(u32, u32), BackendError> {
        assert_main_thread("opening a VST3 plugin editor");
        if self.gui_open {
            return Err(BackendError::Failed(
                "the editor is already open".to_string(),
            ));
        }
        let controller = self
            .controller
            .as_ref()
            .ok_or_else(|| BackendError::Unsupported("the plugin has no editor".to_string()))?;
        let view = make_view(controller)
            .ok_or_else(|| BackendError::Unsupported("the plugin has no editor".to_string()))?;
        let (kind, handle): (&[u8], *mut c_void) = match parent {
            NativeParent::Win32(hwnd) => (b"HWND\0", hwnd as *mut c_void),
            NativeParent::Cocoa(v) => (b"NSView\0", v as *mut c_void),
            NativeParent::X11(id) => (b"X11EmbedWindowID\0", id as usize as *mut c_void),
        };
        let kind_ptr = kind.as_ptr().cast::<c_char>();
        // SAFETY: a live view and a NUL-terminated platform type.
        if !ok(unsafe { view.isPlatformTypeSupported(kind_ptr) }) {
            return Err(BackendError::Unsupported(
                "the editor cannot be embedded in this window system".to_string(),
            ));
        }
        // The content scale applies on Windows and X11; macOS uses logical sizes.
        if !matches!(parent, NativeParent::Cocoa(_))
            && let Some(s) = view.cast::<IPlugViewContentScaleSupport>()
        {
            // SAFETY: a live scale-support interface.
            unsafe { s.setContentScaleFactor(scale as f32) };
        }
        if let Some(f) = self.frame.as_com_ref::<IPlugFrame>() {
            // SAFETY: a live view and the frame's own interface pointer.
            unsafe { view.setFrame(f.as_ptr()) };
        }
        // SAFETY: the parent handle is the window the caller created for this editor and keeps
        // alive until `close_gui` (the `open_gui` contract); the plugin only embeds into it.
        if !ok(unsafe { view.attached(handle, kind_ptr) }) {
            // SAFETY: a live view.
            unsafe { view.setFrame(std::ptr::null_mut()) };
            return Err(failed("showing the editor", "attached failed"));
        }
        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: a live view and a valid out pointer.
        let size = if ok(unsafe { view.getSize(&mut rect) }) {
            (
                (rect.right - rect.left).max(1) as u32,
                (rect.bottom - rect.top).max(1) as u32,
            )
        } else {
            (640, 480)
        };
        self.view = Some(view);
        self.gui_open = true;
        Ok(size)
    }

    fn close_gui(&mut self) {
        assert_main_thread("closing a VST3 plugin editor");
        if let Some(view) = self.view.take() {
            // SAFETY: a live view; `removed` then dropping the frame is the documented close.
            unsafe {
                view.removed();
                view.setFrame(std::ptr::null_mut());
            }
        }
        self.frame.clear();
        self.gui_open = false;
    }

    fn resize_gui(&mut self, w: u32, h: u32) -> Result<(u32, u32), BackendError> {
        assert_main_thread("resizing a VST3 plugin editor");
        let view = self
            .view
            .as_ref()
            .ok_or_else(|| BackendError::Failed("the editor is not open".to_string()))?;
        // SAFETY: a live view.
        if unsafe { view.canResize() } != kResultTrue {
            return Err(BackendError::Unsupported(
                "this plugin's editor cannot be resized".to_string(),
            ));
        }
        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: w as i32,
            bottom: h as i32,
        };
        // SAFETY: a live view and a valid rect.
        unsafe {
            view.checkSizeConstraint(&mut rect);
        }
        // SAFETY: as above.
        if !ok(unsafe { view.onSize(&mut rect) }) {
            return Err(failed("resizing the editor", "onSize failed"));
        }
        Ok((
            (rect.right - rect.left).max(1) as u32,
            (rect.bottom - rect.top).max(1) as u32,
        ))
    }

    fn has_gui(&self) -> bool {
        self.has_view
    }

    fn take_layout_changed(&mut self) -> bool {
        std::mem::take(&mut self.layout_changed)
    }

    fn native_handle(&self) -> usize {
        self.component.as_ptr() as usize
    }

    fn poll(&mut self) {
        assert_main_thread("polling a VST3 plugin");
        self.answer_asks();
        self.apply_mirrored();
        self.frame.run_event_handlers();
        self.frame.run_timers();
    }
}

impl Drop for Vst3Instance {
    fn drop(&mut self) {
        let main = is_main_thread();
        // The editor must be gone and the plugin deactivated before it is destroyed.
        if self.gui_open && main {
            self.close_gui();
        }
        if let Some(c) = &self.controller {
            // SAFETY: a live controller; a null handler detaches it.
            unsafe { c.setComponentHandler(std::ptr::null_mut()) };
        }
        if self.active {
            // SAFETY: a live component.
            unsafe { self.component.setActive(0) };
            self.active = false;
        }
        self.disconnect_halves();
        terminate_halves(
            self.controller
                .as_ref()
                .filter(|_| self.separate_controller),
            &self.component,
        );
        let _ = (&self.handler, &self.host_context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drained_ring_gives_each_id_once_with_its_last_value() {
        let (mut tx, mut rx) = rtrb::RingBuffer::new(16_384);
        for i in 0..10_000u32 {
            tx.push((i % 3, f64::from(i))).unwrap();
        }
        let mut buf = Vec::with_capacity(EDIT_RING);
        let mut seen = Vec::new();
        drain_latest(&mut rx, &mut buf, |id, n| seen.push((id, n)));
        assert_eq!(seen, [(0, 9_999.0), (1, 9_997.0), (2, 9_998.0)]);
        assert!(rx.is_empty());
        seen.clear();
        drain_latest(&mut rx, &mut buf, |id, n| seen.push((id, n)));
        assert!(seen.is_empty(), "nothing is told twice");
    }

    #[test]
    fn a_drain_of_a_ring_sized_buffer_never_grows_it() {
        let (mut tx, mut rx) = rtrb::RingBuffer::new(EDIT_RING);
        let mut buf = Vec::with_capacity(EDIT_RING);
        let (ptr, cap) = (buf.as_ptr(), buf.capacity());
        for round in 0..4u32 {
            while tx.push((round % 7, 1.0)).is_ok() {}
            drain_latest(&mut rx, &mut buf, |_, _| {});
        }
        assert_eq!((buf.as_ptr(), buf.capacity()), (ptr, cap));
    }

    #[test]
    fn a_state_blob_round_trips_both_sections() {
        let blob = pack_state(b"component bytes", b"controller");
        assert_eq!(
            unpack_state(&blob),
            Some((&b"component bytes"[..], &b"controller"[..]))
        );
        let empty = pack_state(&[], &[]);
        assert_eq!(unpack_state(&empty), Some((&[][..], &[][..])));
        assert_eq!(empty.len(), 12);
    }

    #[test]
    fn a_damaged_blob_is_not_a_state() {
        let blob = pack_state(b"abc", b"de");
        assert_eq!(unpack_state(&blob[..blob.len() - 1]), None, "cut short");
        let mut extra = blob.clone();
        extra.push(0);
        assert_eq!(unpack_state(&extra), None, "trailing bytes");
        assert_eq!(unpack_state(b"XXXX\0\0\0\0\0\0\0\0"), None, "wrong magic");
        assert_eq!(unpack_state(b""), None);
        let mut lying = blob;
        lying[4] = 0xFF; // a length longer than the blob
        assert_eq!(unpack_state(&lying), None);
    }
}
