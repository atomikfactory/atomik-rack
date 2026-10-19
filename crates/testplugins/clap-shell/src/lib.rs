//! A minimal CLAP plugin shell over `clap-sys` for the Atomik Rack test plugins.
//!
//! The test plugins are written against the plain DSP of `atomik-test-plugin-core`; this crate
//! is the part that speaks the CLAP ABI, so the host backend is tested against a plugin that was
//! not built with the host's own CLAP library. A plugin library implements [`Dsp`] for each
//! plugin, lists the plugins in a [`LibraryDef`] and calls [`clap_library!`], which exports the
//! `clap_entry` symbol and the test hooks.
//!
//! The shell implements the extensions the host cares about: `audio-ports`, `note-ports`,
//! `params`, `latency`, `tail`, `state`, `render`, `remote-controls`, `timer-support` and a
//! headless `gui` (it records what the host asked for and creates no window).
//!
//! **Test hooks.** Every library also exports `atomik_test_probe(plugin, index)` (what the plugin
//! saw: constant masks, transport, GUI calls, thread-check answers, ...), `atomik_test_command`
//! (make the plugin call back into the host from the calling thread) and
//! `atomik_test_counter(index)` (process-wide instance counters for the leak tests).
//!
//! Every `unsafe` block here is at the C boundary and carries a `SAFETY:` comment.
// Justification: the functions flagged by this lint are the C-ABI glue behind the `extern "C"`
// functions the host calls with the pointers the CLAP contract promises are valid; making each
// of them `unsafe` would only move the same `SAFETY` argument one call up.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use atomik_test_plugin_core::{BLOCK, Block, Event, ExprKind, TimedEvent};
pub use clap_sys as sys;
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_CHOKE, CLAP_EVENT_NOTE_EXPRESSION,
    CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_GESTURE_BEGIN,
    CLAP_EVENT_PARAM_GESTURE_END, CLAP_EVENT_PARAM_VALUE, CLAP_NOTE_EXPRESSION_BRIGHTNESS,
    CLAP_NOTE_EXPRESSION_PAN, CLAP_NOTE_EXPRESSION_PRESSURE, CLAP_NOTE_EXPRESSION_TUNING,
    CLAP_NOTE_EXPRESSION_VOLUME, CLAP_TRANSPORT_IS_LOOP_ACTIVE, clap_event_header, clap_event_midi,
    clap_event_note, clap_event_note_expression, clap_event_param_gesture, clap_event_param_value,
    clap_event_transport, clap_input_events, clap_output_events,
};
use clap_sys::ext::audio_ports::{
    CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS, CLAP_PORT_MONO, CLAP_PORT_STEREO,
    clap_audio_port_info, clap_plugin_audio_ports,
};
use clap_sys::ext::gui::{
    CLAP_EXT_GUI, CLAP_WINDOW_API_COCOA, CLAP_WINDOW_API_WIN32, CLAP_WINDOW_API_X11,
    clap_gui_resize_hints, clap_host_gui, clap_plugin_gui, clap_window,
};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_host_latency, clap_plugin_latency};
use clap_sys::ext::log::{CLAP_EXT_LOG, clap_host_log};
use clap_sys::ext::note_ports::{
    CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI, clap_note_port_info,
    clap_plugin_note_ports,
};
use clap_sys::ext::params::{
    CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_STEPPED, CLAP_PARAM_RESCAN_ALL,
    clap_host_params, clap_param_info, clap_plugin_params,
};
use clap_sys::ext::remote_controls::{
    CLAP_EXT_REMOTE_CONTROLS, clap_plugin_remote_controls, clap_remote_controls_page,
};
use clap_sys::ext::render::{CLAP_EXT_RENDER, clap_plugin_render};
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
use clap_sys::ext::tail::{CLAP_EXT_TAIL, clap_plugin_tail};
use clap_sys::ext::thread_check::{CLAP_EXT_THREAD_CHECK, clap_host_thread_check};
use clap_sys::ext::timer_support::{
    CLAP_EXT_TIMER_SUPPORT, clap_host_timer_support, clap_plugin_timer_support,
};
use clap_sys::host::clap_host;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::{
    CLAP_PROCESS_CONTINUE, CLAP_PROCESS_CONTINUE_IF_NOT_QUIET, CLAP_PROCESS_ERROR,
    CLAP_PROCESS_SLEEP, CLAP_PROCESS_TAIL, clap_process, clap_process_status,
};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

// ----- the plugin author's side ---------------------------------------------------------------

/// Static facts about one plugin.
#[derive(Debug)]
pub struct Info {
    /// CLAP id.
    pub id: &'static CStr,
    /// Name.
    pub name: &'static CStr,
    /// Features (`instrument`, `audio-effect`, ...).
    pub features: &'static [&'static CStr],
    /// The editor is offered (the headless fake GUI).
    pub gui: bool,
    /// The plugin has a remote-controls page.
    pub remote_page: Option<[u32; 8]>,
}

/// Which ports a plugin has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    /// Main input channels (0, 1 or 2).
    pub audio_in: u32,
    /// A stereo sidechain input exists.
    pub sidechain: bool,
    /// Main output channels (0, 1 or 2).
    pub audio_out: u32,
    /// Supported note dialects of the note input (0 for no input port).
    pub notes_in: u32,
    /// The preferred note dialect.
    pub notes_in_preferred: u32,
    /// A note output port exists.
    pub notes_out: bool,
}

/// The note dialect bits.
pub mod dialect {
    /// The CLAP note dialect.
    pub const CLAP: u32 = super::CLAP_NOTE_DIALECT_CLAP;
    /// MIDI 1.0.
    pub const MIDI: u32 = super::CLAP_NOTE_DIALECT_MIDI;
}

impl Ports {
    /// A stereo effect, with a sidechain when `sidechain`.
    pub const fn effect(sidechain: bool) -> Self {
        Self {
            audio_in: 2,
            sidechain,
            audio_out: 2,
            notes_in: 0,
            notes_in_preferred: 0,
            notes_out: false,
        }
    }

    /// A stereo instrument accepting `dialects`.
    pub const fn instrument(dialects: u32, preferred: u32) -> Self {
        Self {
            audio_in: 0,
            sidechain: false,
            audio_out: 2,
            notes_in: dialects,
            notes_in_preferred: preferred,
            notes_out: false,
        }
    }

    /// A note effect.
    pub const fn note_effect() -> Self {
        Self {
            audio_in: 0,
            sidechain: false,
            audio_out: 0,
            notes_in: dialect::CLAP | dialect::MIDI,
            notes_in_preferred: dialect::CLAP,
            notes_out: true,
        }
    }
}

/// One parameter of a plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    /// Id.
    pub id: u32,
    /// Name.
    pub name: String,
    /// Module (group).
    pub module: String,
    /// Minimum.
    pub min: f64,
    /// Maximum.
    pub max: f64,
    /// Default.
    pub default: f64,
    /// Stepped.
    pub stepped: bool,
}

/// What `process` answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Keep going.
    Continue,
    /// Keep going unless quiet.
    ContinueIfNotQuiet,
    /// Producing a tail.
    Tail,
    /// Sleep until new input.
    Sleep,
    /// Fail.
    Error,
}

/// What a plugin asks the host for after a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Requests {
    /// `request_restart`.
    pub restart: bool,
    /// `request_callback`.
    pub callback: bool,
}

/// What a plugin does in its main-thread callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MainAction {
    /// Call `params.rescan(ALL)` (the plugin must not be active).
    pub rescan_params: bool,
}

/// One 64-frame block.
pub struct Io<'a> {
    /// Events in order.
    pub events: &'a [TimedEvent],
    /// Output events (capacity-limited; never grows).
    pub out_events: &'a mut Vec<TimedEvent>,
    /// Main input (a mono input is copied to both).
    pub input: [Block; 2],
    /// The host flagged the main input constant (silent).
    pub input_constant: bool,
    /// The sidechain, when the plugin has one.
    pub sidechain: Option<[Block; 2]>,
    /// The host flagged the sidechain constant (silent).
    pub sidechain_constant: bool,
    /// Main output.
    pub output: [Block; 2],
}

/// The DSP and state of one plugin.
pub trait Dsp: Send {
    /// The ports.
    fn ports(&self) -> Ports;
    /// The parameter table now.
    fn params(&self) -> Vec<ParamSpec>;
    /// `init`; false fails the plugin's init.
    fn init(&mut self) -> bool {
        true
    }
    /// `activate`; false fails activation.
    fn activate(&mut self, _sample_rate: f64) -> bool {
        true
    }
    /// Called by `deactivate`; true asks the host for a main-thread callback.
    fn deactivate(&mut self) -> bool {
        false
    }
    /// `start_processing`; false fails it.
    fn start_processing(&mut self) -> bool {
        true
    }
    /// Sets a param.
    fn set_param(&mut self, id: u32, value: f64);
    /// A param value.
    fn param(&self, id: u32) -> f64;
    /// Latency in frames.
    fn latency(&self) -> u32 {
        0
    }
    /// Tail in frames.
    fn tail(&self) -> u32 {
        0
    }
    /// Saves the state.
    fn save(&self) -> Vec<u8> {
        Vec::new()
    }
    /// Loads a state; false fails.
    fn load(&mut self, _bytes: &[u8]) -> bool {
        true
    }
    /// Resets the audio state.
    fn reset(&mut self) {}
    /// Processes one block.
    fn process(&mut self, io: &mut Io<'_>) -> Status;
    /// What the plugin wants after `process` or `deactivate` (read and cleared).
    fn take_requests(&mut self) -> Requests {
        Requests::default()
    }
    /// The main-thread callback.
    fn on_main_thread(&mut self) -> MainAction {
        MainAction::default()
    }
}

/// A stereo effect with no params that copies its input (the plugin of the isolation test
/// libraries).
#[derive(Debug, Default)]
pub struct Passthrough;

impl Dsp for Passthrough {
    fn ports(&self) -> Ports {
        Ports::effect(false)
    }

    fn params(&self) -> Vec<ParamSpec> {
        Vec::new()
    }

    fn set_param(&mut self, _id: u32, _value: f64) {}

    fn param(&self, _id: u32) -> f64 {
        0.0
    }

    fn process(&mut self, io: &mut Io<'_>) -> Status {
        io.output = io.input;
        Status::Continue
    }
}

/// One plugin of a library.
pub struct PluginDef {
    /// Static facts.
    pub info: &'static Info,
    /// Makes the DSP.
    pub create: fn() -> Box<dyn Dsp>,
}

/// The plugins of a library and its entry behaviour.
pub struct LibraryDef {
    /// The plugins.
    pub plugins: &'static [PluginDef],
    /// Entry `init`; false fails loading the library.
    pub init: fn() -> bool,
    descriptors: OnceLock<Vec<Descriptor>>,
}

impl LibraryDef {
    /// A library of `plugins` whose entry `init` is `init`.
    pub const fn new(plugins: &'static [PluginDef], init: fn() -> bool) -> Self {
        Self {
            plugins,
            init,
            descriptors: OnceLock::new(),
        }
    }
}

// ----- counters and probes ---------------------------------------------------------------------

static COUNTERS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

/// Indexes of `atomik_test_counter`.
pub mod counter {
    /// Plugin instances created.
    pub const CREATED: u32 = 0;
    /// Plugin instances destroyed.
    pub const DESTROYED: u32 = 1;
    /// Activations.
    pub const ACTIVATED: u32 = 2;
    /// Deactivations.
    pub const DEACTIVATED: u32 = 3;
    /// Entry `init` calls.
    pub const ENTRY_INIT: u32 = 4;
    /// Entry `deinit` calls.
    pub const ENTRY_DEINIT: u32 = 5;
}

fn bump(i: u32) {
    COUNTERS[i as usize].fetch_add(1, Ordering::Relaxed);
}

/// Indexes of `atomik_test_probe`.
pub mod probe {
    /// `process` calls.
    pub const PROCESS_CALLS: u32 = 0;
    /// Calls where the host's thread-check said "audio thread".
    pub const AUDIO_THREAD_TRUE: u32 = 1;
    /// Calls where the host's thread-check said "main thread".
    pub const MAIN_THREAD_TRUE: u32 = 2;
    /// `constant_mask` of the main input of the last call.
    pub const MASK_MAIN: u32 = 3;
    /// `constant_mask` of the sidechain of the last call (`u64::MAX` when absent).
    pub const MASK_SIDECHAIN: u32 = 4;
    /// Transport flags of the last call (`u64::MAX` when no transport was given).
    pub const TRANSPORT_FLAGS: u32 = 5;
    /// Tempo bits.
    pub const TEMPO: u32 = 6;
    /// Song position in beats, bits.
    pub const BEATS: u32 = 7;
    /// Song position in seconds, bits.
    pub const SECONDS: u32 = 8;
    /// `steady_time` of the last call (`u64::MAX` for none).
    pub const STEADY: u32 = 9;
    /// Time signature numerator.
    pub const TSIG_NUM: u32 = 10;
    /// Time signature denominator.
    pub const TSIG_DEN: u32 = 11;
    /// Loop start in beats, bits.
    pub const LOOP_START: u32 = 12;
    /// Loop end in beats, bits.
    pub const LOOP_END: u32 = 13;
    /// Frames of the last call.
    pub const FRAMES: u32 = 14;
    /// Timer ticks received.
    pub const TIMER_TICKS: u32 = 15;
    /// Bit set of GUI calls seen: 1 create, 2 set_scale, 4 get_size, 8 set_parent, 16 show,
    /// 32 hide, 64 destroy, 128 set_size.
    pub const GUI_CALLS: u32 = 16;
    /// The parent window handle given to `set_parent`.
    pub const GUI_PARENT: u32 = 17;
    /// The scale given to `set_scale`, bits.
    pub const GUI_SCALE: u32 = 18;
    /// Current GUI width.
    pub const GUI_WIDTH: u32 = 19;
    /// Current GUI height.
    pub const GUI_HEIGHT: u32 = 20;
    /// `state.save` calls.
    pub const STATE_SAVES: u32 = 21;
    /// `state.load` calls.
    pub const STATE_LOADS: u32 = 22;
    /// The render mode last set (0 real time, 1 offline, `u64::MAX` never).
    pub const RENDER_MODE: u32 = 23;
    /// `min_frames_count` of the last activation.
    pub const MIN_FRAMES: u32 = 24;
    /// `max_frames_count` of the last activation.
    pub const MAX_FRAMES: u32 = 25;
    /// Sample rate of the last activation, bits.
    pub const SAMPLE_RATE: u32 = 26;
    /// Input events seen in the last call.
    pub const EVENTS_IN: u32 = 27;
    /// Whether the last events were MIDI (1) or CLAP note events (2): bit set over the call.
    pub const EVENT_DIALECT: u32 = 28;
    /// Main-thread callbacks received.
    pub const MAIN_CALLBACKS: u32 = 29;
    /// `activate` calls.
    pub const ACTIVATIONS: u32 = 30;
    /// `deactivate` calls.
    pub const DEACTIVATIONS: u32 = 31;
    /// Number of probe slots.
    pub const COUNT: usize = 40;
}

struct Probe {
    slots: [AtomicU64; probe::COUNT],
}

impl Probe {
    fn new() -> Self {
        let p = Self {
            slots: [const { AtomicU64::new(0) }; probe::COUNT],
        };
        p.set(probe::RENDER_MODE, u64::MAX);
        p.set(probe::MASK_SIDECHAIN, u64::MAX);
        p.set(probe::TRANSPORT_FLAGS, u64::MAX);
        p
    }

    fn set(&self, i: u32, v: u64) {
        self.slots[i as usize].store(v, Ordering::Relaxed);
    }

    fn get(&self, i: u32) -> u64 {
        self.slots[i as usize].load(Ordering::Relaxed)
    }

    fn add(&self, i: u32, v: u64) {
        self.slots[i as usize].fetch_add(v, Ordering::Relaxed);
    }

    fn or(&self, i: u32, v: u64) {
        self.slots[i as usize].fetch_or(v, Ordering::Relaxed);
    }
}

// ----- descriptors -----------------------------------------------------------------------------

struct Descriptor {
    raw: clap_plugin_descriptor,
    // Keeps the strings behind `raw` alive (they live as long as the library).
    _strings: Vec<CString>,
    _features: Vec<*const c_char>,
}

// SAFETY: a descriptor is built once, never mutated, and only holds pointers to immutable
// 'static strings and to its own immutable vectors.
unsafe impl Send for Descriptor {}
// SAFETY: see above; shared reads only.
unsafe impl Sync for Descriptor {}

fn descriptors(lib: &'static LibraryDef) -> &'static [Descriptor] {
    lib.descriptors.get_or_init(|| {
        lib.plugins
            .iter()
            .map(|p| {
                let strings: Vec<CString> = vec![
                    c"Atomik".to_owned(),
                    c"https://example.invalid".to_owned(),
                    c"1.0.0".to_owned(),
                    c"Atomik Rack test plugin".to_owned(),
                ];
                let mut features: Vec<*const c_char> =
                    p.info.features.iter().map(|f| f.as_ptr()).collect();
                features.push(std::ptr::null());
                let raw = clap_plugin_descriptor {
                    clap_version: CLAP_VERSION,
                    id: p.info.id.as_ptr(),
                    name: p.info.name.as_ptr(),
                    vendor: strings[0].as_ptr(),
                    url: strings[1].as_ptr(),
                    manual_url: strings[1].as_ptr(),
                    support_url: strings[1].as_ptr(),
                    version: strings[2].as_ptr(),
                    description: strings[3].as_ptr(),
                    features: features.as_ptr(),
                };
                Descriptor {
                    raw,
                    _strings: strings,
                    _features: features,
                }
            })
            .collect()
    })
}

/// Entry `init` (called by `clap_entry.init`).
pub fn lib_init(lib: &'static LibraryDef, _path: *const c_char) -> bool {
    bump(counter::ENTRY_INIT);
    (lib.init)()
}

/// Entry `deinit`.
pub fn lib_deinit(_lib: &'static LibraryDef) {
    bump(counter::ENTRY_DEINIT);
}

/// Entry `get_factory`: the plugin factory for its id, null otherwise.
pub fn lib_factory(id: *const c_char, factory: *const c_void) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    // SAFETY: the host passes a NUL-terminated factory id (CLAP entry contract).
    let id = unsafe { CStr::from_ptr(id) };
    if id == clap_sys::factory::plugin_factory::CLAP_PLUGIN_FACTORY_ID {
        factory
    } else {
        std::ptr::null()
    }
}

/// Factory `get_plugin_descriptor`.
pub fn factory_descriptor(lib: &'static LibraryDef, index: u32) -> *const clap_plugin_descriptor {
    descriptors(lib)
        .get(index as usize)
        .map_or(std::ptr::null(), |d| &raw const d.raw)
}

/// Factory `create_plugin`.
pub fn factory_create(
    lib: &'static LibraryDef,
    host: *const clap_host,
    id: *const c_char,
) -> *const clap_plugin {
    if id.is_null() {
        return std::ptr::null();
    }
    // SAFETY: the host passes a NUL-terminated plugin id (CLAP factory contract).
    let id = unsafe { CStr::from_ptr(id) };
    let Some((index, def)) = lib
        .plugins
        .iter()
        .enumerate()
        .find(|(_, p)| p.info.id == id)
    else {
        return std::ptr::null();
    };
    let desc = &descriptors(lib)[index].raw;
    let dsp = (def.create)();
    let ports = dsp.ports();
    let params = dsp.params();
    let instance = Box::new(Instance {
        plugin: clap_plugin {
            desc,
            plugin_data: std::ptr::null_mut(),
            init: Some(plugin_init),
            destroy: Some(plugin_destroy),
            activate: Some(plugin_activate),
            deactivate: Some(plugin_deactivate),
            start_processing: Some(plugin_start_processing),
            stop_processing: Some(plugin_stop_processing),
            reset: Some(plugin_reset),
            process: Some(plugin_process),
            get_extension: Some(plugin_get_extension),
            on_main_thread: Some(plugin_on_main_thread),
        },
        host,
        info: def.info,
        ports,
        probe: Probe::new(),
        ext: OnceLock::new(),
        inner: Mutex::new(Inner {
            dsp,
            params,
            events: Vec::with_capacity(8_192),
            out_events: Vec::with_capacity(1_024),
            gui_edits: Vec::with_capacity(64),
            gui: Gui {
                width: 320,
                height: 200,
                created: false,
                timer: None,
            },
        }),
    });
    let raw = Box::into_raw(instance);
    // SAFETY: `raw` comes from `Box::into_raw` just above and is not shared yet.
    unsafe {
        (*raw).plugin.plugin_data = raw.cast();
        bump(counter::CREATED);
        &raw const (*raw).plugin
    }
}

// ----- the instance ----------------------------------------------------------------------------

struct HostExt {
    thread_check: *const clap_host_thread_check,
    params: *const clap_host_params,
    latency: *const clap_host_latency,
    gui: *const clap_host_gui,
    timer: *const clap_host_timer_support,
    log: *const clap_host_log,
    state: *const clap_sys::ext::state::clap_host_state,
}

// SAFETY: the pointers are the host's extension tables, immutable and valid for the life of the
// host object, which outlives the plugin instance.
unsafe impl Send for HostExt {}
// SAFETY: see above.
unsafe impl Sync for HostExt {}

struct Gui {
    width: u32,
    height: u32,
    created: bool,
    timer: Option<u32>,
}

struct Inner {
    dsp: Box<dyn Dsp>,
    params: Vec<ParamSpec>,
    events: Vec<TimedEvent>,
    out_events: Vec<TimedEvent>,
    gui_edits: Vec<(u32, f64)>,
    gui: Gui,
}

struct Instance {
    plugin: clap_plugin,
    host: *const clap_host,
    info: &'static Info,
    ports: Ports,
    probe: Probe,
    ext: OnceLock<HostExt>,
    inner: Mutex<Inner>,
}

// SAFETY: `host` is only used to call the host's thread-safe functions and its extension tables;
// everything else in the instance is `Sync` (atomics, a mutex).
unsafe impl Send for Instance {}
// SAFETY: see above.
unsafe impl Sync for Instance {}

fn instance<'a>(plugin: *const clap_plugin) -> &'a Instance {
    // SAFETY: the host only calls a plugin with the pointer this shell returned from `create`,
    // whose `plugin_data` points to the live boxed `Instance` until `destroy`.
    unsafe { &*(*plugin).plugin_data.cast::<Instance>() }
}

fn lock(i: &Instance) -> MutexGuard<'_, Inner> {
    i.inner.lock().unwrap_or_else(PoisonError::into_inner)
}

fn host_ext(i: &Instance) -> Option<&HostExt> {
    i.ext.get()
}

fn request_restart(i: &Instance) {
    // SAFETY: `host` is the valid host the plugin was created with; `request_restart` is
    // thread-safe by the CLAP spec.
    unsafe {
        if let Some(f) = (*i.host).request_restart {
            f(i.host);
        }
    }
}

fn request_callback(i: &Instance) {
    // SAFETY: as above; `request_callback` is thread-safe.
    unsafe {
        if let Some(f) = (*i.host).request_callback {
            f(i.host);
        }
    }
}

extern "C" fn plugin_init(plugin: *const clap_plugin) -> bool {
    let i = instance(plugin);
    let get = |name: &CStr| -> *const c_void {
        // SAFETY: `host` is valid; `get_extension` takes a NUL-terminated id.
        unsafe {
            (*i.host)
                .get_extension
                .map_or(std::ptr::null(), |f| f(i.host, name.as_ptr()))
        }
    };
    let _ = i.ext.set(HostExt {
        thread_check: get(CLAP_EXT_THREAD_CHECK).cast(),
        params: get(CLAP_EXT_PARAMS).cast(),
        latency: get(CLAP_EXT_LATENCY).cast(),
        gui: get(CLAP_EXT_GUI).cast(),
        timer: get(CLAP_EXT_TIMER_SUPPORT).cast(),
        log: get(CLAP_EXT_LOG).cast(),
        state: get(CLAP_EXT_STATE).cast(),
    });
    let mut inner = lock(i);
    let ok = inner.dsp.init();
    inner.params = inner.dsp.params();
    ok
}

extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    // SAFETY: `destroy` is the last call on the plugin; the pointer came from `Box::into_raw`
    // in `factory_create` and is not used again.
    drop(unsafe { Box::from_raw((*plugin).plugin_data.cast::<Instance>()) });
    bump(counter::DESTROYED);
}

extern "C" fn plugin_activate(
    plugin: *const clap_plugin,
    sample_rate: f64,
    min_frames: u32,
    max_frames: u32,
) -> bool {
    let i = instance(plugin);
    i.probe.set(probe::MIN_FRAMES, u64::from(min_frames));
    i.probe.set(probe::MAX_FRAMES, u64::from(max_frames));
    i.probe.set(probe::SAMPLE_RATE, sample_rate.to_bits());
    i.probe.add(probe::ACTIVATIONS, 1);
    let ok = lock(i).dsp.activate(sample_rate);
    if ok {
        bump(counter::ACTIVATED);
    }
    ok
}

extern "C" fn plugin_deactivate(plugin: *const clap_plugin) {
    let i = instance(plugin);
    i.probe.add(probe::DEACTIVATIONS, 1);
    bump(counter::DEACTIVATED);
    let wants_callback = lock(i).dsp.deactivate();
    if wants_callback {
        request_callback(i);
    }
}

extern "C" fn plugin_start_processing(plugin: *const clap_plugin) -> bool {
    lock(instance(plugin)).dsp.start_processing()
}

extern "C" fn plugin_stop_processing(_plugin: *const clap_plugin) {}

extern "C" fn plugin_reset(plugin: *const clap_plugin) {
    lock(instance(plugin)).dsp.reset();
}

extern "C" fn plugin_on_main_thread(plugin: *const clap_plugin) {
    let i = instance(plugin);
    i.probe.add(probe::MAIN_CALLBACKS, 1);
    let action = {
        let mut inner = lock(i);
        let action = inner.dsp.on_main_thread();
        inner.params = inner.dsp.params();
        action
    };
    if action.rescan_params
        && let Some(ext) = host_ext(i)
        && !ext.params.is_null()
    {
        // SAFETY: the host's params table is valid; `rescan` is called on the main thread, as
        // this callback is.
        unsafe {
            if let Some(f) = (*ext.params).rescan {
                f(i.host, CLAP_PARAM_RESCAN_ALL);
            }
        }
    }
}

// ----- process ---------------------------------------------------------------------------------

const FIX_BEATS: f64 = (1u64 << 31) as f64;

fn read_events(list: *const clap_input_events, out: &mut Vec<TimedEvent>, probe: &Probe) {
    out.clear();
    if list.is_null() {
        return;
    }
    // SAFETY: the host passes a valid input event list for the duration of `process`.
    let (size, get) = unsafe { ((*list).size, (*list).get) };
    let (Some(size), Some(get)) = (size, get) else {
        return;
    };
    // SAFETY: `size` is the list's own function.
    let n = unsafe { size(list) };
    probe.set(probe::EVENTS_IN, u64::from(n));
    for k in 0..n {
        // SAFETY: `get` is the list's own function and `k < size`.
        let header = unsafe { get(list, k) };
        if header.is_null() {
            continue;
        }
        // SAFETY: a non-null event pointer is valid for the call and starts with a header.
        let h = unsafe { *header };
        if h.space_id != CLAP_CORE_EVENT_SPACE_ID || out.len() >= out.capacity() {
            continue;
        }
        let offset = h.time.min(BLOCK as u32 - 1) as u16;
        let mut push = |event: Event| out.push(TimedEvent { offset, event });
        match h.type_ {
            CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF | CLAP_EVENT_NOTE_CHOKE => {
                // SAFETY: a note event is a `clap_event_note` (type tag checked above).
                let e = unsafe { *header.cast::<clap_event_note>() };
                let key = e.key.clamp(0, 127) as u8;
                probe.or(probe::EVENT_DIALECT, 2);
                if h.type_ == CLAP_EVENT_NOTE_ON {
                    push(Event::NoteOn {
                        key,
                        velocity: e.velocity as f32,
                        id: e.note_id,
                    });
                } else {
                    push(Event::NoteOff { key, id: e.note_id });
                }
            }
            CLAP_EVENT_NOTE_EXPRESSION => {
                // SAFETY: a note expression event is a `clap_event_note_expression`.
                let e = unsafe { *header.cast::<clap_event_note_expression>() };
                let value = e.value as f32;
                let (kind, value) = match e.expression_id {
                    CLAP_NOTE_EXPRESSION_TUNING => (ExprKind::Tuning, value),
                    CLAP_NOTE_EXPRESSION_PRESSURE => (ExprKind::Pressure, value),
                    CLAP_NOTE_EXPRESSION_BRIGHTNESS => (ExprKind::Slide, value),
                    CLAP_NOTE_EXPRESSION_VOLUME => (ExprKind::Volume, value),
                    CLAP_NOTE_EXPRESSION_PAN => (ExprKind::Pan, value * 2.0 - 1.0),
                    _ => continue,
                };
                probe.or(probe::EVENT_DIALECT, 2);
                push(Event::Expr {
                    key: e.key.clamp(0, 127) as u8,
                    id: e.note_id,
                    kind,
                    value,
                });
            }
            CLAP_EVENT_PARAM_VALUE => {
                // SAFETY: a param value event is a `clap_event_param_value`.
                let e = unsafe { *header.cast::<clap_event_param_value>() };
                push(Event::Param {
                    id: e.param_id,
                    value: e.value,
                });
            }
            CLAP_EVENT_MIDI => {
                // SAFETY: a MIDI event is a `clap_event_midi`.
                let e = unsafe { *header.cast::<clap_event_midi>() };
                probe.or(probe::EVENT_DIALECT, 1);
                let [status, d1, d2] = e.data;
                let key = d1 & 0x7F;
                let unit = |v: u8| f32::from(v & 0x7F) / 127.0;
                match status & 0xF0 {
                    0x90 if d2 > 0 => push(Event::NoteOn {
                        key,
                        velocity: unit(d2),
                        id: -1,
                    }),
                    0x90 | 0x80 => push(Event::NoteOff { key, id: -1 }),
                    0xA0 => push(Event::PolyPressure {
                        key,
                        id: -1,
                        value: unit(d2),
                    }),
                    0xB0 => push(Event::Cc {
                        controller: key,
                        value: unit(d2),
                    }),
                    0xD0 => push(Event::ChannelPressure(unit(d1))),
                    0xE0 => {
                        let v14 = i32::from(d1 & 0x7F) | (i32::from(d2 & 0x7F) << 7);
                        push(Event::PitchBend((v14 - 8192) as f32 / 8192.0));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn record_transport(t: *const clap_event_transport, probe: &Probe) {
    if t.is_null() {
        probe.set(probe::TRANSPORT_FLAGS, u64::MAX);
        return;
    }
    // SAFETY: a non-null transport pointer is valid for the call.
    let t = unsafe { *t };
    probe.set(probe::TRANSPORT_FLAGS, u64::from(t.flags));
    probe.set(probe::TEMPO, t.tempo.to_bits());
    probe.set(
        probe::BEATS,
        (t.song_pos_beats as f64 / FIX_BEATS).to_bits(),
    );
    probe.set(
        probe::SECONDS,
        (t.song_pos_seconds as f64 / FIX_BEATS).to_bits(),
    );
    probe.set(probe::TSIG_NUM, u64::from(t.tsig_num));
    probe.set(probe::TSIG_DEN, u64::from(t.tsig_denom));
    if t.flags & CLAP_TRANSPORT_IS_LOOP_ACTIVE != 0 {
        probe.set(
            probe::LOOP_START,
            (t.loop_start_beats as f64 / FIX_BEATS).to_bits(),
        );
        probe.set(
            probe::LOOP_END,
            (t.loop_end_beats as f64 / FIX_BEATS).to_bits(),
        );
    }
}

fn read_channels(buf: &clap_audio_buffer, frames: usize) -> [Block; 2] {
    let mut out = [[0.0; BLOCK]; 2];
    if buf.data32.is_null() || buf.channel_count == 0 {
        return out;
    }
    for (ch, dst) in out.iter_mut().enumerate() {
        let src = ch.min(buf.channel_count as usize - 1);
        // SAFETY: `data32` has `channel_count` valid channel pointers, each with `frames`
        // samples (CLAP process contract); `src < channel_count`.
        let p = unsafe { *buf.data32.add(src) };
        if !p.is_null() {
            // SAFETY: the channel holds `frames` readable samples.
            let s = unsafe { std::slice::from_raw_parts(p, frames) };
            dst[..frames].copy_from_slice(s);
        }
    }
    out
}

fn write_channels(buf: &clap_audio_buffer, frames: usize, data: &[Block; 2]) {
    if buf.data32.is_null() {
        return;
    }
    for (ch, block) in data
        .iter()
        .enumerate()
        .take((buf.channel_count as usize).min(2))
    {
        // SAFETY: `data32` has `channel_count` valid channel pointers.
        let p = unsafe { *buf.data32.add(ch) };
        if !p.is_null() {
            // SAFETY: the channel holds `frames` writable samples (CLAP process contract).
            let d = unsafe { std::slice::from_raw_parts_mut(p, frames) };
            d.copy_from_slice(&block[..frames]);
        }
    }
}

fn push_out_event(list: *const clap_output_events, header: *const clap_event_header) {
    // SAFETY: the host passes a valid output event list for the duration of `process`.
    unsafe {
        if let Some(f) = (*list).try_push {
            f(list, header);
        }
    }
}

fn note_header(size: usize, time: u32, type_: u16) -> clap_event_header {
    clap_event_header {
        size: size as u32,
        time,
        space_id: CLAP_CORE_EVENT_SPACE_ID,
        type_,
        flags: 0,
    }
}

fn write_out_events(list: *const clap_output_events, events: &[TimedEvent]) {
    if list.is_null() {
        return;
    }
    for e in events {
        let (type_, key, id, velocity) = match e.event {
            Event::NoteOn { key, velocity, id } => (CLAP_EVENT_NOTE_ON, key, id, velocity),
            Event::NoteOff { key, id } => (CLAP_EVENT_NOTE_OFF, key, id, 0.0),
            _ => continue,
        };
        let note = clap_event_note {
            header: note_header(
                std::mem::size_of::<clap_event_note>(),
                u32::from(e.offset),
                type_,
            ),
            note_id: id,
            port_index: 0,
            channel: 0,
            key: i16::from(key),
            velocity: f64::from(velocity),
        };
        push_out_event(list, &raw const note.header);
    }
}

fn write_gui_edits(list: *const clap_output_events, edits: &[(u32, f64)]) {
    if list.is_null() {
        return;
    }
    for &(id, value) in edits {
        let gesture = |type_| clap_event_param_gesture {
            header: note_header(std::mem::size_of::<clap_event_param_gesture>(), 0, type_),
            param_id: id,
        };
        let begin = gesture(CLAP_EVENT_PARAM_GESTURE_BEGIN);
        push_out_event(list, &raw const begin.header);
        let v = clap_event_param_value {
            header: note_header(
                std::mem::size_of::<clap_event_param_value>(),
                0,
                CLAP_EVENT_PARAM_VALUE,
            ),
            param_id: id,
            cookie: std::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value,
        };
        push_out_event(list, &raw const v.header);
        let end = gesture(CLAP_EVENT_PARAM_GESTURE_END);
        push_out_event(list, &raw const end.header);
    }
}

fn status_code(s: Status) -> clap_process_status {
    match s {
        Status::Continue => CLAP_PROCESS_CONTINUE,
        Status::ContinueIfNotQuiet => CLAP_PROCESS_CONTINUE_IF_NOT_QUIET,
        Status::Tail => CLAP_PROCESS_TAIL,
        Status::Sleep => CLAP_PROCESS_SLEEP,
        Status::Error => CLAP_PROCESS_ERROR,
    }
}

extern "C" fn plugin_process(
    plugin: *const clap_plugin,
    process: *const clap_process,
) -> clap_process_status {
    let i = instance(plugin);
    // SAFETY: the host passes a valid `clap_process` for the call.
    let p = unsafe { &*process };
    let frames = p.frames_count as usize;
    i.probe.add(probe::PROCESS_CALLS, 1);
    i.probe.set(probe::FRAMES, frames as u64);
    if let Some(ext) = host_ext(i)
        && !ext.thread_check.is_null()
    {
        // SAFETY: the host's thread-check table is valid.
        unsafe {
            if let Some(f) = (*ext.thread_check).is_audio_thread
                && f(i.host)
            {
                i.probe.add(probe::AUDIO_THREAD_TRUE, 1);
            }
            if let Some(f) = (*ext.thread_check).is_main_thread
                && f(i.host)
            {
                i.probe.add(probe::MAIN_THREAD_TRUE, 1);
            }
        }
    }
    record_transport(p.transport, &i.probe);
    i.probe.set(
        probe::STEADY,
        if p.steady_time < 0 {
            u64::MAX
        } else {
            p.steady_time as u64
        },
    );
    if frames != BLOCK {
        return CLAP_PROCESS_ERROR;
    }
    let mut guard = lock(i);
    let inner = &mut *guard;
    read_events(p.in_events, &mut inner.events, &i.probe);
    inner.out_events.clear();

    let mut index = 0usize;
    let (mut input, mut input_constant) = ([[0.0; BLOCK]; 2], true);
    if i.ports.audio_in > 0 && (index as u32) < p.audio_inputs_count {
        // SAFETY: `audio_inputs` has `audio_inputs_count` valid buffers.
        let b = unsafe { &*p.audio_inputs.add(index) };
        input = read_channels(b, frames);
        input_constant = b.constant_mask != 0;
        i.probe.set(probe::MASK_MAIN, b.constant_mask);
        index += 1;
    }
    let mut sidechain = None;
    let mut sidechain_constant = true;
    if i.ports.sidechain && (index as u32) < p.audio_inputs_count {
        // SAFETY: as above.
        let b = unsafe { &*p.audio_inputs.add(index) };
        sidechain = Some(read_channels(b, frames));
        sidechain_constant = b.constant_mask != 0;
        i.probe.set(probe::MASK_SIDECHAIN, b.constant_mask);
    } else {
        i.probe.set(probe::MASK_SIDECHAIN, u64::MAX);
    }

    let mut io = Io {
        events: &inner.events,
        out_events: &mut inner.out_events,
        input,
        input_constant,
        sidechain,
        sidechain_constant,
        output: [[0.0; BLOCK]; 2],
    };
    // GUI edits are applied by the plugin and reported to the host as output events.
    for &(id, value) in &inner.gui_edits {
        inner.dsp.set_param(id, value);
    }
    let status = inner.dsp.process(&mut io);
    if p.audio_outputs_count > 0 && !p.audio_outputs.is_null() {
        // SAFETY: `audio_outputs` has `audio_outputs_count` valid buffers.
        let out = unsafe { &*p.audio_outputs };
        write_channels(out, frames, &io.output);
    }
    write_out_events(p.out_events, io.out_events);
    write_gui_edits(p.out_events, &inner.gui_edits);
    inner.gui_edits.clear();
    let requests = inner.dsp.take_requests();
    drop(guard);
    if requests.restart {
        request_restart(i);
    }
    if requests.callback {
        request_callback(i);
    }
    status_code(status)
}

// ----- extensions ------------------------------------------------------------------------------

extern "C" fn audio_ports_count(plugin: *const clap_plugin, is_input: bool) -> u32 {
    let ports = instance(plugin).ports;
    if is_input {
        u32::from(ports.audio_in > 0) + u32::from(ports.sidechain)
    } else {
        u32::from(ports.audio_out > 0)
    }
}

fn copy_name<const N: usize>(dst: &mut [c_char; N], text: &str) {
    dst.fill(0);
    for (d, b) in dst.iter_mut().zip(text.bytes().take(N - 1)) {
        *d = b as c_char;
    }
}

extern "C" fn audio_ports_get(
    plugin: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    let ports = instance(plugin).ports;
    let main = if is_input {
        ports.audio_in
    } else {
        ports.audio_out
    };
    let (id, name, channels, flags) = match (is_input, main > 0, index) {
        (_, true, 0) => (
            0,
            if is_input { "Main In" } else { "Main Out" },
            main,
            CLAP_AUDIO_PORT_IS_MAIN,
        ),
        (true, has_main, n) if ports.sidechain && n == u32::from(has_main) => {
            (1, "Sidechain", 2, 0)
        }
        _ => return false,
    };
    // SAFETY: the host passes a valid, writable `clap_audio_port_info`.
    let info = unsafe { &mut *info };
    info.id = id;
    copy_name(&mut info.name, name);
    info.flags = flags;
    info.channel_count = channels;
    info.port_type = if channels == 1 {
        CLAP_PORT_MONO.as_ptr()
    } else {
        CLAP_PORT_STEREO.as_ptr()
    };
    info.in_place_pair = u32::MAX;
    true
}

static AUDIO_PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(audio_ports_count),
    get: Some(audio_ports_get),
};

extern "C" fn note_ports_count(plugin: *const clap_plugin, is_input: bool) -> u32 {
    let ports = instance(plugin).ports;
    if is_input {
        u32::from(ports.notes_in != 0)
    } else {
        u32::from(ports.notes_out)
    }
}

extern "C" fn note_ports_get(
    plugin: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_note_port_info,
) -> bool {
    let ports = instance(plugin).ports;
    if index != 0 {
        return false;
    }
    let (dialects, preferred) = if is_input && ports.notes_in != 0 {
        (ports.notes_in, ports.notes_in_preferred)
    } else if !is_input && ports.notes_out {
        (CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_CLAP)
    } else {
        return false;
    };
    // SAFETY: the host passes a valid, writable `clap_note_port_info`.
    let info = unsafe { &mut *info };
    info.id = 0;
    info.supported_dialects = dialects;
    info.preferred_dialect = preferred;
    copy_name(
        &mut info.name,
        if is_input { "Notes In" } else { "Notes Out" },
    );
    true
}

static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports {
    count: Some(note_ports_count),
    get: Some(note_ports_get),
};

extern "C" fn params_count(plugin: *const clap_plugin) -> u32 {
    lock(instance(plugin)).params.len() as u32
}

extern "C" fn params_get_info(
    plugin: *const clap_plugin,
    index: u32,
    info: *mut clap_param_info,
) -> bool {
    let inner = lock(instance(plugin));
    let Some(p) = inner.params.get(index as usize) else {
        return false;
    };
    // SAFETY: the host passes a valid, writable `clap_param_info`.
    let info = unsafe { &mut *info };
    info.id = p.id;
    info.flags = CLAP_PARAM_IS_AUTOMATABLE | if p.stepped { CLAP_PARAM_IS_STEPPED } else { 0 };
    info.cookie = std::ptr::null_mut();
    copy_name(&mut info.name, &p.name);
    copy_name(&mut info.module, &p.module);
    info.min_value = p.min;
    info.max_value = p.max;
    info.default_value = p.default;
    true
}

extern "C" fn params_get_value(plugin: *const clap_plugin, id: u32, value: *mut f64) -> bool {
    let inner = lock(instance(plugin));
    if !inner.params.iter().any(|p| p.id == id) {
        return false;
    }
    // SAFETY: the host passes a valid, writable `f64`.
    unsafe { *value = inner.dsp.param(id) };
    true
}

extern "C" fn params_value_to_text(
    plugin: *const clap_plugin,
    id: u32,
    value: f64,
    out: *mut c_char,
    capacity: u32,
) -> bool {
    let inner = lock(instance(plugin));
    let Some(p) = inner.params.iter().find(|p| p.id == id) else {
        return false;
    };
    let text = if p.stepped {
        format!("{} {}", value.round() as i64, p.name)
    } else {
        format!("{value:.2}")
    };
    if capacity == 0 {
        return false;
    }
    // SAFETY: the host passes a writable buffer of `capacity` bytes.
    let dst = unsafe { std::slice::from_raw_parts_mut(out.cast::<u8>(), capacity as usize) };
    dst.fill(0);
    for (d, b) in dst.iter_mut().zip(text.bytes().take(capacity as usize - 1)) {
        *d = b;
    }
    true
}

extern "C" fn params_text_to_value(
    _plugin: *const clap_plugin,
    _id: u32,
    _text: *const c_char,
    _value: *mut f64,
) -> bool {
    false
}

extern "C" fn params_flush(
    plugin: *const clap_plugin,
    in_events: *const clap_input_events,
    _out: *const clap_output_events,
) {
    let i = instance(plugin);
    let mut guard = lock(i);
    let inner = &mut *guard;
    read_events(in_events, &mut inner.events, &i.probe);
    for e in &inner.events {
        if let Event::Param { id, value } = e.event {
            inner.dsp.set_param(id, value);
        }
    }
}

static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: Some(params_value_to_text),
    text_to_value: Some(params_text_to_value),
    flush: Some(params_flush),
};

extern "C" fn latency_get(plugin: *const clap_plugin) -> u32 {
    lock(instance(plugin)).dsp.latency()
}

static LATENCY: clap_plugin_latency = clap_plugin_latency {
    get: Some(latency_get),
};

extern "C" fn tail_get(plugin: *const clap_plugin) -> u32 {
    lock(instance(plugin)).dsp.tail()
}

static TAIL: clap_plugin_tail = clap_plugin_tail {
    get: Some(tail_get),
};

extern "C" fn state_save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let i = instance(plugin);
    i.probe.add(probe::STATE_SAVES, 1);
    let bytes = lock(i).dsp.save();
    // SAFETY: the host passes a valid output stream.
    let Some(write) = (unsafe { (*stream).write }) else {
        return false;
    };
    let mut done = 0usize;
    while done < bytes.len() {
        // SAFETY: the pointer and length describe the unwritten tail of `bytes`.
        let n = unsafe {
            write(
                stream,
                bytes[done..].as_ptr().cast(),
                (bytes.len() - done) as u64,
            )
        };
        if n <= 0 {
            return false;
        }
        done += n as usize;
    }
    true
}

extern "C" fn state_load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    let i = instance(plugin);
    i.probe.add(probe::STATE_LOADS, 1);
    // SAFETY: the host passes a valid input stream.
    let Some(read) = (unsafe { (*stream).read }) else {
        return false;
    };
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        // SAFETY: `chunk` is a writable buffer of the given length.
        let n = unsafe { read(stream, chunk.as_mut_ptr().cast(), chunk.len() as u64) };
        if n < 0 {
            return false;
        }
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n as usize]);
    }
    lock(i).dsp.load(&bytes)
}

static STATE: clap_plugin_state = clap_plugin_state {
    save: Some(state_save),
    load: Some(state_load),
};

extern "C" fn render_hard_realtime(_plugin: *const clap_plugin) -> bool {
    false
}

extern "C" fn render_set(plugin: *const clap_plugin, mode: i32) -> bool {
    instance(plugin).probe.set(probe::RENDER_MODE, mode as u64);
    true
}

static RENDER: clap_plugin_render = clap_plugin_render {
    has_hard_realtime_requirement: Some(render_hard_realtime),
    set: Some(render_set),
};

extern "C" fn remote_count(plugin: *const clap_plugin) -> u32 {
    u32::from(instance(plugin).info.remote_page.is_some())
}

extern "C" fn remote_get(
    plugin: *const clap_plugin,
    index: u32,
    page: *mut clap_remote_controls_page,
) -> bool {
    let Some(ids) = instance(plugin).info.remote_page else {
        return false;
    };
    if index != 0 {
        return false;
    }
    // SAFETY: the host passes a valid, writable page.
    let page = unsafe { &mut *page };
    copy_name(&mut page.section_name, "Main");
    page.page_id = 1;
    copy_name(&mut page.page_name, "Page 1");
    page.param_ids = ids;
    page.is_for_preset = false;
    true
}

static REMOTE: clap_plugin_remote_controls = clap_plugin_remote_controls {
    count: Some(remote_count),
    get: Some(remote_get),
};

extern "C" fn timer_on_timer(plugin: *const clap_plugin, _id: u32) {
    instance(plugin).probe.add(probe::TIMER_TICKS, 1);
}

static TIMER: clap_plugin_timer_support = clap_plugin_timer_support {
    on_timer: Some(timer_on_timer),
};

fn native_api() -> &'static CStr {
    if cfg!(windows) {
        CLAP_WINDOW_API_WIN32
    } else if cfg!(target_os = "macos") {
        CLAP_WINDOW_API_COCOA
    } else {
        CLAP_WINDOW_API_X11
    }
}

extern "C" fn gui_is_api_supported(
    _plugin: *const clap_plugin,
    api: *const c_char,
    floating: bool,
) -> bool {
    // SAFETY: the host passes a NUL-terminated API name.
    !floating && unsafe { CStr::from_ptr(api) } == native_api()
}

extern "C" fn gui_get_preferred_api(
    _plugin: *const clap_plugin,
    api: *mut *const c_char,
    floating: *mut bool,
) -> bool {
    // SAFETY: the host passes valid out pointers.
    unsafe {
        *api = native_api().as_ptr();
        *floating = false;
    }
    true
}

extern "C" fn gui_create(plugin: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
    if !gui_is_api_supported(plugin, api, floating) {
        return false;
    }
    let i = instance(plugin);
    i.probe.or(probe::GUI_CALLS, 1);
    let mut inner = lock(i);
    inner.gui.created = true;
    if let Some(ext) = host_ext(i)
        && !ext.timer.is_null()
    {
        let mut id = 0u32;
        // SAFETY: the host's timer table is valid; `id` is a writable `u32`.
        let ok = unsafe {
            (*ext.timer)
                .register_timer
                .is_some_and(|f| f(i.host, 10, &raw mut id))
        };
        inner.gui.timer = ok.then_some(id);
    }
    true
}

extern "C" fn gui_destroy(plugin: *const clap_plugin) {
    let i = instance(plugin);
    i.probe.or(probe::GUI_CALLS, 64);
    let mut inner = lock(i);
    inner.gui.created = false;
    if let Some(id) = inner.gui.timer.take()
        && let Some(ext) = host_ext(i)
        && !ext.timer.is_null()
    {
        // SAFETY: the host's timer table is valid.
        unsafe {
            if let Some(f) = (*ext.timer).unregister_timer {
                f(i.host, id);
            }
        }
    }
}

extern "C" fn gui_set_scale(plugin: *const clap_plugin, scale: f64) -> bool {
    let i = instance(plugin);
    i.probe.or(probe::GUI_CALLS, 2);
    i.probe.set(probe::GUI_SCALE, scale.to_bits());
    true
}

extern "C" fn gui_get_size(plugin: *const clap_plugin, w: *mut u32, h: *mut u32) -> bool {
    let i = instance(plugin);
    i.probe.or(probe::GUI_CALLS, 4);
    let inner = lock(i);
    // SAFETY: the host passes valid out pointers.
    unsafe {
        *w = inner.gui.width;
        *h = inner.gui.height;
    }
    true
}

extern "C" fn gui_can_resize(_plugin: *const clap_plugin) -> bool {
    true
}

extern "C" fn gui_get_resize_hints(
    _plugin: *const clap_plugin,
    hints: *mut clap_gui_resize_hints,
) -> bool {
    // SAFETY: the host passes a valid, writable hints struct.
    let hints = unsafe { &mut *hints };
    hints.can_resize_horizontally = true;
    hints.can_resize_vertically = true;
    hints.preserve_aspect_ratio = false;
    hints.aspect_ratio_width = 0;
    hints.aspect_ratio_height = 0;
    true
}

extern "C" fn gui_adjust_size(_plugin: *const clap_plugin, w: *mut u32, h: *mut u32) -> bool {
    // SAFETY: the host passes valid in/out pointers.
    unsafe {
        *w = (*w).clamp(100, 2_000) & !1;
        *h = (*h).clamp(80, 2_000) & !1;
    }
    true
}

extern "C" fn gui_set_size(plugin: *const clap_plugin, w: u32, h: u32) -> bool {
    let i = instance(plugin);
    i.probe.or(probe::GUI_CALLS, 128);
    let mut inner = lock(i);
    inner.gui.width = w;
    inner.gui.height = h;
    i.probe.set(probe::GUI_WIDTH, u64::from(w));
    i.probe.set(probe::GUI_HEIGHT, u64::from(h));
    true
}

extern "C" fn gui_set_parent(plugin: *const clap_plugin, window: *const clap_window) -> bool {
    let i = instance(plugin);
    i.probe.or(probe::GUI_CALLS, 8);
    // SAFETY: the host passes a valid window; every variant of the union is pointer sized or
    // smaller, so reading the pointer member reads the handle as the host wrote it.
    let handle = unsafe { (*window).specific.ptr } as usize as u64;
    i.probe.set(probe::GUI_PARENT, handle);
    true
}

extern "C" fn gui_set_transient(_plugin: *const clap_plugin, _window: *const clap_window) -> bool {
    false
}

extern "C" fn gui_suggest_title(_plugin: *const clap_plugin, _title: *const c_char) {}

extern "C" fn gui_show(plugin: *const clap_plugin) -> bool {
    instance(plugin).probe.or(probe::GUI_CALLS, 16);
    true
}

extern "C" fn gui_hide(plugin: *const clap_plugin) -> bool {
    instance(plugin).probe.or(probe::GUI_CALLS, 32);
    true
}

static GUI: clap_plugin_gui = clap_plugin_gui {
    is_api_supported: Some(gui_is_api_supported),
    get_preferred_api: Some(gui_get_preferred_api),
    create: Some(gui_create),
    destroy: Some(gui_destroy),
    set_scale: Some(gui_set_scale),
    get_size: Some(gui_get_size),
    can_resize: Some(gui_can_resize),
    get_resize_hints: Some(gui_get_resize_hints),
    adjust_size: Some(gui_adjust_size),
    set_size: Some(gui_set_size),
    set_parent: Some(gui_set_parent),
    set_transient: Some(gui_set_transient),
    suggest_title: Some(gui_suggest_title),
    show: Some(gui_show),
    hide: Some(gui_hide),
};

extern "C" fn plugin_get_extension(plugin: *const clap_plugin, id: *const c_char) -> *const c_void {
    let i = instance(plugin);
    // SAFETY: the host passes a NUL-terminated extension id.
    let id = unsafe { CStr::from_ptr(id) };
    let ports = i.ports;
    if id == CLAP_EXT_AUDIO_PORTS {
        (&raw const AUDIO_PORTS).cast()
    } else if id == CLAP_EXT_NOTE_PORTS && (ports.notes_in != 0 || ports.notes_out) {
        (&raw const NOTE_PORTS).cast()
    } else if id == CLAP_EXT_PARAMS {
        (&raw const PARAMS).cast()
    } else if id == CLAP_EXT_LATENCY {
        (&raw const LATENCY).cast()
    } else if id == CLAP_EXT_TAIL {
        (&raw const TAIL).cast()
    } else if id == CLAP_EXT_STATE {
        (&raw const STATE).cast()
    } else if id == CLAP_EXT_RENDER {
        (&raw const RENDER).cast()
    } else if id == CLAP_EXT_REMOTE_CONTROLS && i.info.remote_page.is_some() {
        (&raw const REMOTE).cast()
    } else if id == CLAP_EXT_TIMER_SUPPORT && i.info.gui {
        (&raw const TIMER).cast()
    } else if id == CLAP_EXT_GUI && i.info.gui {
        (&raw const GUI).cast()
    } else {
        std::ptr::null()
    }
}

// ----- test hooks (exported by `clap_library!`) ----------------------------------------------------

/// `atomik_test_counter`: a process-wide counter of this library.
pub fn counter_read(index: u32) -> u64 {
    COUNTERS
        .get(index as usize)
        .map_or(0, |c| c.load(Ordering::Relaxed))
}

/// `atomik_test_probe`: what the plugin instance saw.
pub fn probe_read(plugin: *const clap_plugin, index: u32) -> u64 {
    if plugin.is_null() || index as usize >= probe::COUNT {
        return 0;
    }
    instance(plugin).probe.get(index)
}

/// Commands of `atomik_test_command`.
pub mod command {
    /// `gui.request_resize(a, b)` from the calling thread.
    pub const GUI_REQUEST_RESIZE: u32 = 1;
    /// `latency.changed()` (call it on the main thread).
    pub const LATENCY_CHANGED: u32 = 2;
    /// `params.request_flush()`.
    pub const REQUEST_FLUSH: u32 = 3;
    /// `state.mark_dirty()`.
    pub const MARK_DIRTY: u32 = 4;
    /// `request_restart()`.
    pub const REQUEST_RESTART: u32 = 5;
    /// `request_callback()`.
    pub const REQUEST_CALLBACK: u32 = 6;
    /// `request_process()`.
    pub const REQUEST_PROCESS: u32 = 7;
    /// `gui.closed(a != 0)`.
    pub const GUI_CLOSED: u32 = 8;
    /// `log(severity a, "test log")`.
    pub const LOG: u32 = 9;
    /// `gui.request_show()`.
    pub const GUI_REQUEST_SHOW: u32 = 10;
    /// `gui.request_hide()`.
    pub const GUI_REQUEST_HIDE: u32 = 11;
    /// A GUI edit: param `a` to the value whose bits are `b`, reported as gesture events by the
    /// next `process`.
    pub const GUI_EDIT: u32 = 12;
}

/// `atomik_test_command`: makes the plugin call into the host from the calling thread.
pub fn command(plugin: *const clap_plugin, cmd: u32, a: u64, b: u64) -> u64 {
    if plugin.is_null() {
        return 0;
    }
    let i = instance(plugin);
    let Some(ext) = host_ext(i) else {
        return 0;
    };
    // SAFETY: every table pointer used below is checked for null first and is the host's own
    // extension table; the host pointer is the one the plugin was created with.
    unsafe {
        match cmd {
            command::GUI_REQUEST_RESIZE if !ext.gui.is_null() => u64::from(
                (*ext.gui)
                    .request_resize
                    .is_some_and(|f| f(i.host, a as u32, b as u32)),
            ),
            command::LATENCY_CHANGED if !ext.latency.is_null() => {
                if let Some(f) = (*ext.latency).changed {
                    f(i.host);
                }
                1
            }
            command::REQUEST_FLUSH if !ext.params.is_null() => {
                if let Some(f) = (*ext.params).request_flush {
                    f(i.host);
                }
                1
            }
            command::MARK_DIRTY if !ext.state.is_null() => {
                if let Some(f) = (*ext.state).mark_dirty {
                    f(i.host);
                }
                1
            }
            command::REQUEST_RESTART => {
                request_restart(i);
                1
            }
            command::REQUEST_CALLBACK => {
                request_callback(i);
                1
            }
            command::REQUEST_PROCESS => {
                if let Some(f) = (*i.host).request_process {
                    f(i.host);
                }
                1
            }
            command::GUI_CLOSED if !ext.gui.is_null() => {
                if let Some(f) = (*ext.gui).closed {
                    f(i.host, a != 0);
                }
                1
            }
            command::LOG if !ext.log.is_null() => {
                if let Some(f) = (*ext.log).log {
                    f(i.host, a as i32, c"test log".as_ptr());
                }
                1
            }
            command::GUI_REQUEST_SHOW if !ext.gui.is_null() => {
                u64::from((*ext.gui).request_show.is_some_and(|f| f(i.host)))
            }
            command::GUI_REQUEST_HIDE if !ext.gui.is_null() => {
                u64::from((*ext.gui).request_hide.is_some_and(|f| f(i.host)))
            }
            command::GUI_EDIT => {
                let mut inner = lock(i);
                if inner.gui_edits.len() < inner.gui_edits.capacity() {
                    inner.gui_edits.push((a as u32, f64::from_bits(b)));
                }
                1
            }
            _ => 0,
        }
    }
}

/// Declares a plugin library: the `clap_entry` symbol and the test hooks. `$lib` is a
/// `LibraryDef` constant expression.
#[macro_export]
macro_rules! clap_library {
    ($lib:expr) => {
        static __ATOMIK_LIB: $crate::LibraryDef = $lib;

        extern "C" fn __atomik_init(path: *const ::std::ffi::c_char) -> bool {
            $crate::lib_init(&__ATOMIK_LIB, path)
        }

        extern "C" fn __atomik_deinit() {
            $crate::lib_deinit(&__ATOMIK_LIB)
        }

        extern "C" fn __atomik_count(
            _factory: *const $crate::sys::factory::plugin_factory::clap_plugin_factory,
        ) -> u32 {
            __ATOMIK_LIB.plugins.len() as u32
        }

        extern "C" fn __atomik_descriptor(
            _factory: *const $crate::sys::factory::plugin_factory::clap_plugin_factory,
            index: u32,
        ) -> *const $crate::sys::plugin::clap_plugin_descriptor {
            $crate::factory_descriptor(&__ATOMIK_LIB, index)
        }

        extern "C" fn __atomik_create(
            _factory: *const $crate::sys::factory::plugin_factory::clap_plugin_factory,
            host: *const $crate::sys::host::clap_host,
            id: *const ::std::ffi::c_char,
        ) -> *const $crate::sys::plugin::clap_plugin {
            $crate::factory_create(&__ATOMIK_LIB, host, id)
        }

        static __ATOMIK_FACTORY: $crate::sys::factory::plugin_factory::clap_plugin_factory =
            $crate::sys::factory::plugin_factory::clap_plugin_factory {
                get_plugin_count: Some(__atomik_count),
                get_plugin_descriptor: Some(__atomik_descriptor),
                create_plugin: Some(__atomik_create),
            };

        extern "C" fn __atomik_get_factory(
            id: *const ::std::ffi::c_char,
        ) -> *const ::std::ffi::c_void {
            $crate::lib_factory(id, (&raw const __ATOMIK_FACTORY).cast())
        }

        /// The CLAP entry point.
        #[unsafe(no_mangle)]
        #[used]
        pub static clap_entry: $crate::sys::entry::clap_plugin_entry =
            $crate::sys::entry::clap_plugin_entry {
                clap_version: $crate::sys::version::CLAP_VERSION,
                init: Some(__atomik_init),
                deinit: Some(__atomik_deinit),
                get_factory: Some(__atomik_get_factory),
            };

        /// Test hook: what the plugin instance saw (see `atomik_test_clap_shell::probe`).
        #[unsafe(no_mangle)]
        pub extern "C" fn atomik_test_probe(
            plugin: *const $crate::sys::plugin::clap_plugin,
            index: u32,
        ) -> u64 {
            $crate::probe_read(plugin, index)
        }

        /// Test hook: make the plugin call into the host (see `atomik_test_clap_shell::command`).
        #[unsafe(no_mangle)]
        pub extern "C" fn atomik_test_command(
            plugin: *const $crate::sys::plugin::clap_plugin,
            cmd: u32,
            a: u64,
            b: u64,
        ) -> u64 {
            $crate::command(plugin, cmd, a, b)
        }

        /// Test hook: process-wide instance counters (see `atomik_test_clap_shell::counter`).
        #[unsafe(no_mangle)]
        pub extern "C" fn atomik_test_counter(index: u32) -> u64 {
            $crate::counter_read(index)
        }
    };
}
