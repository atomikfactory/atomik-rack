//! The backend seam: everything a plugin format has to provide, as traits, plus the
//! plain data that crosses it. Nothing outside this crate names a format's types.
//!
//! * [`PluginBackend`] loads files and creates instances (main thread).
//! * [`PluginInstance`] is the main-thread half of one plugin: activation, state, parameter
//!   text, editor. Activating it yields the audio half.
//! * [`PluginProcessor`] is the audio half: the only part that runs on engine threads. It is
//!   called with [`PluginIo`], a format-neutral view of one 64-frame block.
//! * [`NullBackend`] is an in-repo implementation for tests: it needs no plugin file.

use crate::slot::HostMessenger;
use atomik_core::{BATCH, Batch, ExprKind};
use atomik_device_sdk::{PluginFormat, PluginLayout, PluginRef, PluginStateBlob, RunMode};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// What a backend call can fail with.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackendError {
    /// The file is not a plugin of this format (or holds none).
    #[error("not a plugin: {0}")]
    NotAPlugin(String),
    /// The plugin is not in the file or the backend does not know it.
    #[error("unknown plugin `{0}`")]
    UnknownPlugin(String),
    /// The plugin cannot be hosted (no stereo or mono bus, 64-bit only, ...), or the call is
    /// not supported by it (no editor).
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The plugin or the backend failed.
    #[error("{0}")]
    Failed(String),
}

// ----- events and the process view ----------------------------------------------------------

/// A format-neutral plugin event. Note ids are assigned by the host.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PluginEventKind {
    /// Start a note. `velocity` is in (0, 1].
    NoteOn {
        /// MIDI key.
        key: u8,
        /// Velocity in (0, 1].
        velocity: f32,
        /// Host note id.
        note_id: i32,
    },
    /// Release a note.
    NoteOff {
        /// MIDI key.
        key: u8,
        /// The id given at the On.
        note_id: i32,
    },
    /// Pitch bend, -1 to 1.
    PitchBend {
        /// Amount.
        value: f32,
    },
    /// A continuous controller (123 is all notes off).
    Cc {
        /// Controller number 0 to 127.
        controller: u8,
        /// Value 0 to 1.
        value: f32,
    },
    /// Channel pressure, 0 to 1.
    ChannelPressure {
        /// Pressure.
        value: f32,
    },
    /// Pressure of one note.
    PolyPressure {
        /// MIDI key.
        key: u8,
        /// The note's id.
        note_id: i32,
        /// Pressure 0 to 1.
        value: f32,
    },
    /// A note expression.
    NoteExpr {
        /// MIDI key.
        key: u8,
        /// The note's id.
        note_id: i32,
        /// Which expression.
        kind: ExprKind,
        /// The value.
        value: f32,
    },
    /// A raw MIDI 1.0 channel message (status byte first, `len` of the three bytes used). Only
    /// the device's MPE output sends these, to a plugin that takes MIDI: member channels carry
    /// one note each (the note ids of the other events do not apply to them).
    Midi {
        /// Status, then the data bytes.
        bytes: [u8; 3],
        /// How many of the bytes are used, 2 or 3.
        len: u8,
    },
    /// A parameter change by the plugin's own id, in the format's plain value.
    Param {
        /// The plugin's param id.
        id: u32,
        /// Plain value.
        value: f64,
    },
}

/// An event at a frame offset inside the batch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PluginEvent {
    /// Frame offset, `< 64`, non-decreasing within a list.
    pub offset: u16,
    /// The event.
    pub kind: PluginEventKind,
}

/// A fixed-capacity event list for one batch: never allocates after construction, keeps offsets
/// non-decreasing and inside the batch, and counts what did not fit.
#[derive(Debug)]
pub struct EventList {
    events: Vec<PluginEvent>,
    last_offset: u16,
    dropped: u32,
}

impl EventList {
    /// An empty list able to hold `capacity` events.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            events: Vec::with_capacity(capacity),
            last_offset: 0,
            dropped: 0,
        }
    }

    /// Appends an event at `offset`, clamped to the batch and to the previous offset. Returns
    /// false (and counts a drop) when the list is full. Never allocates.
    pub fn push(&mut self, offset: u16, kind: PluginEventKind) -> bool {
        if self.events.len() >= self.events.capacity() {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        let offset = offset.min(BATCH as u16 - 1).max(self.last_offset);
        self.last_offset = offset;
        self.events.push(PluginEvent { offset, kind });
        true
    }

    /// Removes the events (the drop count stays until [`EventList::take_dropped`]).
    pub fn clear(&mut self) {
        self.events.clear();
        self.last_offset = 0;
    }

    /// The events in order.
    pub fn as_slice(&self) -> &[PluginEvent] {
        &self.events
    }

    /// Number of events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// True when there are no events.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Capacity.
    pub fn capacity(&self) -> usize {
        self.events.capacity()
    }

    /// Events refused because the list was full, since the last [`EventList::take_dropped`].
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Returns the drop count and resets it.
    pub fn take_dropped(&mut self) -> u32 {
        std::mem::take(&mut self.dropped)
    }
}

/// Transport and tempo for one batch. Positions are in quarter notes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PluginTransport {
    /// The transport is playing.
    pub playing: bool,
    /// Tempo in BPM.
    pub tempo_bpm: f64,
    /// Position in quarter notes at the first frame of the batch (tick / PPQ).
    pub song_pos_beats: f64,
    /// Position in seconds at the current tempo (`beats * 60 / tempo`).
    pub song_pos_seconds: f64,
    /// The engine's frame counter at the first frame of the batch (CLAP `steady_time`).
    pub steady_frames: u64,
    /// Time signature `(numerator, denominator)`.
    pub time_signature: (u8, u8),
    /// A loop region is active.
    pub loop_active: bool,
    /// Loop start in quarter notes.
    pub loop_start_beats: f64,
    /// Loop end in quarter notes.
    pub loop_end_beats: f64,
    /// Sample rate in Hz.
    pub sample_rate: f64,
}

impl Default for PluginTransport {
    fn default() -> Self {
        Self {
            playing: false,
            tempo_bpm: 120.0,
            song_pos_beats: 0.0,
            song_pos_seconds: 0.0,
            steady_frames: 0,
            time_signature: (4, 4),
            loop_active: false,
            loop_start_beats: 0.0,
            loop_end_beats: 0.0,
            sample_rate: 48_000.0,
        }
    }
}

/// One 64-frame block as the plugin sees it. All slices are preallocated by the device.
pub struct PluginIo<'a> {
    /// Frames in this block: always [`BATCH`] (CLAP `min == max == 64`).
    pub frames: usize,
    /// Main input channels (0 for none, 1 mono, 2 stereo).
    pub main_in: &'a [Batch],
    /// No main input is cabled: the buffers are silent and the plugin's silent flag is set
    /// (CLAP `constant_mask`, VST3 `silenceFlags`).
    pub main_in_silent: bool,
    /// Sidechain channels (0 or 2).
    pub sidechain: &'a [Batch],
    /// No sidechain is cabled.
    pub sidechain_silent: bool,
    /// Main output channels (0, 1 or 2); arrive zeroed.
    pub main_out: &'a mut [Batch],
    /// Aux output channels, two per pair (left, right) of the pairs the layout's `aux_out`
    /// counts; arrive zeroed. A processor fills both channels of a pair (a mono aux bus is
    /// copied to both), or leaves the pair silent.
    pub aux_out: &'a mut [Batch],
    /// Events for this block, offsets non-decreasing: param changes first, then notes.
    pub events_in: &'a [PluginEvent],
    /// Where a note effect puts its notes (the host copies On and Off to `notes_out`).
    pub events_out: &'a mut EventList,
    /// Transport and tempo.
    pub transport: PluginTransport,
    /// The backend adds events it had to drop (a VST3 controller with no assignment, say);
    /// the device shows the total as its `dropped` meter.
    pub dropped: u32,
}

/// What `process` answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    /// Keep going.
    Continue,
    /// Keep going unless the input is quiet.
    ContinueIfNotQuiet,
    /// Producing a tail (for example reverb decay): keep calling.
    Tail,
    /// Nothing to do until new input, events or param changes arrive (CLAP only).
    Sleep,
    /// The plugin failed: the device becomes FAILED.
    Error,
    /// The plugin did not take this batch (a VST3 plugin answered a result other than OK, as
    /// many do while they load content, sign in or change their buses; the child had no
    /// processor for a moment). Not a failure: the device has no wet signal this batch, counts
    /// it and carries on. Only a panic, a crash or a lost process fails a device.
    Refused,
    /// Sandbox: the result did not come back in time for this batch. Not a failure: the
    /// device has no valid wet signal this batch (dry for effects, silence otherwise).
    Late,
    /// Sandbox: the plugin's process is gone (crashed, killed or stopped by the watchdog).
    /// The device becomes CRASHED.
    Lost,
}

/// How a plugin's editor is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditorKind {
    /// Embedded in a window the host creates ([`PluginInstance::open_gui`]).
    #[default]
    Embedded,
    /// In a top-level window the plugin's own process creates
    /// ([`PluginInstance::open_external_gui`]).
    External,
}

/// How long the plugin keeps sounding after its input stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailLength {
    /// A tail of this many frames (0 for none).
    Finite(u32),
    /// Never ends (VST3 `kInfiniteTail`).
    Infinite,
}

/// Which skipping rule the device applies for this plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepRule {
    /// CLAP: the plugin says [`ProcessStatus::Sleep`]; the device then skips `process` until an
    /// event, a param change or a non-silent input needs it.
    PluginDecides,
    /// VST3: skip only when the tail is zero, all inputs are silent and nothing is pending.
    ZeroTail,
}

/// The audio half of a plugin. Called on whichever engine thread runs the device (the audio
/// callback, a worker, or the audio thread when pinned). Must not allocate, lock or block; a
/// third-party implementation may, and the host cannot prevent it.
pub trait PluginProcessor: Send {
    /// `[audio-thread]` Called once before the first `process` after activation. False fails
    /// the device.
    fn start_processing(&mut self) -> bool;
    /// `[audio-thread]` Called before the processor goes back to the main thread.
    fn stop_processing(&mut self);
    /// `[audio-thread]` Clears the plugin's audio state without allocating.
    fn reset(&mut self);
    /// `[audio-thread]` Processes one block.
    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus;
    /// The skipping rule of this plugin's format.
    fn sleep_rule(&self) -> SleepRule {
        SleepRule::PluginDecides
    }
    /// The plugin's tail (used by [`SleepRule::ZeroTail`]).
    fn tail_samples(&self) -> TailLength {
        TailLength::Finite(0)
    }
    /// True once (and clears) when the plugin asked to be processed although the device would
    /// let it sleep (CLAP `request_process` and `params.request_flush`, for example a knob
    /// turned in its editor while it sleeps). Must be wait-free.
    fn wake_requested(&self) -> bool {
        false
    }
}

// ----- the main-thread half ---------------------------------------------------------------------

/// Parameters of one activation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActivateConfig {
    /// Session sample rate.
    pub sample_rate: f64,
    /// Block size: always 64.
    pub frames: u32,
    /// Live or offline (CLAP `render` extension, VST3 `processMode`).
    pub mode: RunMode,
}

/// The native window a plugin editor is embedded in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeParent {
    /// A Windows `HWND`.
    Win32(usize),
    /// A macOS `NSView`.
    Cocoa(usize),
    /// An X11 window id.
    X11(u32),
}

/// What a backend hands its instances so they can reach the host from any thread.
#[derive(Debug, Clone)]
pub struct HostHandle {
    messenger: HostMessenger,
}

impl HostHandle {
    /// A handle posting to `messenger`.
    pub fn new(messenger: HostMessenger) -> Self {
        Self { messenger }
    }

    /// The thread-safe message sender.
    pub fn messenger(&self) -> &HostMessenger {
        &self.messenger
    }
}

/// One preset a plugin offers: a CLAP preset found by discovery, a VST3 program of a unit's
/// program list or a `.vstpreset` file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PresetInfo {
    /// Opaque id the backend understands (given back to [`PluginInstance::load_preset`]).
    pub id: String,
    /// The name to show.
    pub name: String,
    /// The folder or program list it belongs to (empty for none).
    pub group: String,
}

/// The main-thread half of one plugin. Every method is `[main-thread]`.
pub trait PluginInstance {
    /// The plugin's layout.
    fn layout(&self) -> &PluginLayout;
    /// Activates at `cfg` and returns the audio half.
    fn activate(&mut self, cfg: &ActivateConfig) -> Result<Box<dyn PluginProcessor>, BackendError>;
    /// Deactivates; the processor must have been stopped and handed back first.
    fn deactivate(&mut self, processor: Box<dyn PluginProcessor>);
    /// Starts [`PluginInstance::deactivate`] without waiting for the plugin: true when it is
    /// done, false when [`PluginInstance::poll_deactivate`] will tell. A restart (a bus layout
    /// change, a new sample rate) uses this so the app does not stand still while a busy
    /// plugin finishes; the default deactivates at once.
    fn begin_deactivate(&mut self, processor: Box<dyn PluginProcessor>) -> bool {
        self.deactivate(processor);
        true
    }
    /// True once a [`PluginInstance::begin_deactivate`] that returned false is done (or the
    /// plugin is gone).
    fn poll_deactivate(&mut self) -> bool {
        true
    }
    /// The latency the plugin reports now, in frames.
    fn latency_frames(&self) -> u32;
    /// Starts saving the plugin state without waiting for it: the next
    /// [`PluginInstance::capture_state`] collects it. A remote plugin gets its request out so a
    /// burst over many plugins waits for the slowest one, not for the sum; the default does
    /// nothing and `capture_state` does the whole job.
    fn begin_capture_state(&mut self) {}
    /// Saves the plugin state.
    fn capture_state(&mut self) -> Result<PluginStateBlob, BackendError>;
    /// Restores a saved state.
    fn restore_state(&mut self, state: &PluginStateBlob) -> Result<(), BackendError>;
    /// The presets the plugin offers, in its own order (empty when it has none or the format
    /// cannot list them).
    fn list_presets(&mut self) -> Vec<PresetInfo> {
        Vec::new()
    }
    /// Loads the preset `id` (from [`PluginInstance::list_presets`]) into the live instance.
    /// The new state is read with [`PluginInstance::capture_state`] afterwards.
    fn load_preset(&mut self, _id: &str) -> Result<(), BackendError> {
        Err(BackendError::Unsupported(
            "this plugin has no presets the host can load".to_string(),
        ))
    }
    /// The param values the plugin holds now as `(id, host value)` pairs for the params of its
    /// layout (the plain value for CLAP; for VST3 the step index of a stepped param and the
    /// normalised value otherwise): what the rack's knobs show after the plugin changed its own
    /// values (a preset or a patch change). Empty when the plugin cannot be asked.
    fn read_params(&mut self) -> Vec<(u32, f64)> {
        Vec::new()
    }
    /// The display text of a param value, from the plugin.
    fn param_text(&mut self, id: u32, plain: f64) -> Option<String>;
    /// Opens the editor inside `parent`; returns its size in pixels.
    fn open_gui(&mut self, parent: NativeParent, scale: f64) -> Result<(u32, u32), BackendError>;
    /// Closes the editor.
    fn close_gui(&mut self);
    /// Runs the plugin's main-thread work (timers, callbacks); called every UI frame.
    fn poll(&mut self);
    /// Asks the plugin to resize its editor to (about) `w` x `h` pixels; returns the size it
    /// took. The window follows the plugin's answer.
    fn resize_gui(&mut self, _w: u32, _h: u32) -> Result<(u32, u32), BackendError> {
        Err(BackendError::Unsupported(
            "this plugin's editor cannot be resized".to_string(),
        ))
    }
    /// Whether the plugin has an editor the host can embed.
    fn has_gui(&self) -> bool {
        false
    }
    /// The param ids a panel should pin by default (CLAP: the first remote-controls page);
    /// empty when the plugin has none.
    fn default_pins(&mut self) -> Vec<u32> {
        Vec::new()
    }
    /// True once (and clears) when the plugin's param table changed since the last call
    /// (a CLAP `rescan`); [`PluginInstance::layout`] then holds the new table.
    fn take_layout_changed(&mut self) -> bool {
        false
    }
    /// An opaque id of the native instance for diagnostics and tests (0 when there is none).
    fn native_handle(&self) -> usize {
        0
    }
    /// How the editor is shown.
    fn editor_kind(&self) -> EditorKind {
        EditorKind::Embedded
    }
    /// [`EditorKind::External`] editors: opens (or raises) the plugin's own window, owned by
    /// `owner` (the app window) where the platform allows it.
    fn open_external_gui(&mut self, _owner: Option<NativeParent>) -> Result<(), BackendError> {
        Err(BackendError::Unsupported(
            "this plugin's editor is embedded".to_string(),
        ))
    }
    /// Starts [`PluginInstance::open_external_gui`] without waiting for the plugin: true when the
    /// editor is open already, false when [`PluginInstance::poll_open_external_gui`] will tell.
    /// An editor may take minutes to open (a licence check online, sign-in dialogs), and the
    /// app must not wait for it. The default opens at once.
    fn begin_open_external_gui(
        &mut self,
        owner: Option<NativeParent>,
    ) -> Result<bool, BackendError> {
        self.open_external_gui(owner).map(|()| true)
    }
    /// The outcome of the editor opening that [`PluginInstance::begin_open_external_gui`]
    /// started, once (`None` while it goes on or when none is going on). Main thread.
    fn poll_open_external_gui(&mut self) -> Option<Result<(), BackendError>> {
        None
    }
    /// True when the instance can no longer be used: its process is gone. Every other call
    /// then fails without doing anything; recovery is a new instance.
    fn is_lost(&self) -> bool {
        false
    }
    /// The instance is about to be dropped: starts ending whatever outlives a call (a child
    /// process) without waiting for it, so that the manager can start every instance's end
    /// first and then wait for all of them at once. Dropping still works without this call.
    fn begin_shutdown(&mut self) {}
}

/// One plugin found in a file.
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedPlugin {
    /// Which plugin.
    pub plugin: PluginRef,
    /// Its layout.
    pub layout: PluginLayout,
    /// Why the plugin cannot be hosted (no stereo or mono main bus, ...); it is listed but
    /// cannot be added.
    pub unsupported: Option<String>,
    /// The param table was longer than `MAX_PLUGIN_PARAMS` and was cut.
    pub params_truncated: bool,
}

/// An instance that is still starting (see [`PluginBackend::begin_instantiate`]). Dropping it
/// abandons the start and ends the plugin's process.
pub trait PendingInstance {
    /// Moves the start on without blocking: `None` while the plugin is not ready, then the
    /// instance or why it could not be made.
    fn poll(&mut self) -> Option<Result<Box<dyn PluginInstance>, BackendError>>;
}

/// A plugin format. The sandbox (`atomik-plugin-sandbox`) implements the same traits.
pub trait PluginBackend {
    /// The format this backend loads.
    fn format(&self) -> PluginFormat;
    /// Loads one file and describes every plugin in it.
    fn scan_file(&self, path: &Path) -> Result<Vec<ScannedPlugin>, BackendError>;
    /// Creates an instance of a plugin.
    fn instantiate(
        &self,
        plugin: &PluginRef,
        host: HostHandle,
    ) -> Result<Box<dyn PluginInstance>, BackendError>;
    /// Starts creating an instance without waiting for it: a backend whose plugins load in
    /// another process returns the instance while it starts, so many of them load at the same
    /// time. `None` (the default) means the backend creates instances only with
    /// [`PluginBackend::instantiate`], which blocks.
    fn begin_instantiate(
        &self,
        _plugin: &PluginRef,
        _host: HostHandle,
    ) -> Result<Option<Box<dyn PendingInstance>>, BackendError> {
        Ok(None)
    }
}

// ----- the null backend ---------------------------------------------------------------------------

/// What a [`NullPlugin`]'s processor does with its input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NullBehavior {
    /// Output silence.
    Silent,
    /// Copy the input.
    Passthrough,
    /// Copy the input times a gain.
    Gain(f32),
}

/// The presets a [`NullStats::set_presets`] null instance offers: `(id, name, group)`.
pub const NULL_PRESETS: [(&str, &str, &str); 3] = [
    ("init", "Init", "Factory"),
    ("warm", "Warm", "Factory"),
    ("bright", "Bright", "Factory"),
];

/// A virtual plugin of the [`NullBackend`].
#[derive(Debug, Clone)]
pub struct NullPlugin {
    /// The "file" the scanner finds it in.
    pub path: PathBuf,
    /// Its id.
    pub plugin: PluginRef,
    /// Its layout.
    pub layout: PluginLayout,
    /// What its processor does.
    pub behavior: NullBehavior,
    /// The latency it reports.
    pub latency: u32,
}

/// Lifecycle counters of a [`NullBackend`] (the instance counters of the leak and restart
/// tests).
#[derive(Debug, Default)]
pub struct NullStats {
    created: AtomicUsize,
    destroyed: AtomicUsize,
    activated: AtomicUsize,
    deactivated: AtomicUsize,
    last_rate_bits: AtomicU64,
    latency_override: AtomicU32,
    fail_activation: AtomicBool,
    lost: AtomicBool,
    external_editor: AtomicBool,
    external_opens: AtomicUsize,
    external_closes: AtomicUsize,
    external_delay: AtomicUsize,
    presets: AtomicBool,
    deactivate_delay: AtomicUsize,
    /// The layout the instances take at their next restart (see [`NullStats::change_layout`]).
    pending_layout: Mutex<Option<PluginLayout>>,
    /// The layout the plugin has since: new instances start with it.
    changed_layout: Mutex<Option<PluginLayout>>,
}

impl NullStats {
    /// Makes a restart's deactivation take `polls` polls of the instance (a plugin that is busy
    /// but alive): [`PluginInstance::begin_deactivate`] answers "not yet" and
    /// [`PluginInstance::poll_deactivate`] says when it is done.
    pub fn set_deactivate_delay(&self, polls: usize) {
        self.deactivate_delay.store(polls, Ordering::Relaxed);
    }
    /// Makes the null plugin change its layout (its buses, as a plugin does after a sign-in) at
    /// the next restart of an instance: [`PluginInstance::take_layout_changed`] answers true once
    /// and [`PluginInstance::layout`] is `layout` from then on, for every instance created
    /// afterwards too.
    pub fn change_layout(&self, layout: PluginLayout) {
        *self
            .pending_layout
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(layout);
    }

    /// Instances created.
    pub fn created(&self) -> usize {
        self.created.load(Ordering::Relaxed)
    }
    /// Instances destroyed.
    pub fn destroyed(&self) -> usize {
        self.destroyed.load(Ordering::Relaxed)
    }
    /// Activations.
    pub fn activated(&self) -> usize {
        self.activated.load(Ordering::Relaxed)
    }
    /// Deactivations.
    pub fn deactivated(&self) -> usize {
        self.deactivated.load(Ordering::Relaxed)
    }
    /// The sample rate of the last activation (0 before the first).
    pub fn last_sample_rate(&self) -> f64 {
        f64::from_bits(self.last_rate_bits.load(Ordering::Relaxed))
    }
    /// Makes every null instance report `frames` of latency from now on.
    pub fn set_latency(&self, frames: u32) {
        // Stored as `frames + 1` so that 0 (the default) means "no override".
        self.latency_override
            .store(frames.saturating_add(1), Ordering::Relaxed);
    }
    /// Makes the next activation fail once.
    pub fn fail_next_activation(&self) {
        self.fail_activation.store(true, Ordering::Relaxed);
    }
    /// Makes every null instance report its process as gone (`is_lost`), as a crashed
    /// sandboxed plugin does.
    pub fn set_lost(&self, lost: bool) {
        self.lost.store(lost, Ordering::Relaxed);
    }
    /// Makes every null instance have an [`EditorKind::External`] editor.
    pub fn set_external_editor(&self, external: bool) {
        self.external_editor.store(external, Ordering::Relaxed);
    }
    /// Makes every null instance offer [`NULL_PRESETS`]; loading one sets the state to
    /// `preset:<id>`.
    pub fn set_presets(&self, on: bool) {
        self.presets.store(on, Ordering::Relaxed);
    }
    /// Makes an external editor take `polls` polls of the instance to open (a plugin that
    /// checks its licence or shows its own dialogs first): `begin_open_external_gui` answers
    /// "not yet" and `poll_open_external_gui` says when it is open.
    pub fn set_external_editor_delay(&self, polls: usize) {
        self.external_delay.store(polls, Ordering::Relaxed);
    }
    /// How many times an external editor was opened.
    pub fn external_opens(&self) -> usize {
        self.external_opens.load(Ordering::Relaxed)
    }
    /// How many times an open external editor was closed.
    pub fn external_closes(&self) -> usize {
        self.external_closes.load(Ordering::Relaxed)
    }
}

/// A backend with no plugin files: its plugins are described in memory. It exercises the whole
/// seam in tests and is what the manager tests use before a real format exists.
#[derive(Debug, Default)]
pub struct NullBackend {
    plugins: Vec<NullPlugin>,
    stats: Arc<NullStats>,
}

impl NullBackend {
    /// A backend with no plugins.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a plugin.
    #[must_use]
    pub fn with_plugin(mut self, plugin: NullPlugin) -> Self {
        self.plugins.push(plugin);
        self
    }

    /// The lifecycle counters.
    pub fn stats(&self) -> Arc<NullStats> {
        Arc::clone(&self.stats)
    }
}

impl PluginBackend for NullBackend {
    fn format(&self) -> PluginFormat {
        self.plugins
            .first()
            .map_or(PluginFormat::Clap, |p| p.plugin.format)
    }

    fn scan_file(&self, path: &Path) -> Result<Vec<ScannedPlugin>, BackendError> {
        let found: Vec<ScannedPlugin> = self
            .plugins
            .iter()
            .filter(|p| p.path == path)
            .map(|p| ScannedPlugin {
                plugin: p.plugin.clone(),
                layout: p.layout.clone(),
                unsupported: None,
                params_truncated: false,
            })
            .collect();
        if found.is_empty() {
            Err(BackendError::NotAPlugin(path.display().to_string()))
        } else {
            Ok(found)
        }
    }

    fn instantiate(
        &self,
        plugin: &PluginRef,
        _host: HostHandle,
    ) -> Result<Box<dyn PluginInstance>, BackendError> {
        let p = self
            .plugins
            .iter()
            .find(|p| &p.plugin == plugin)
            .ok_or_else(|| BackendError::UnknownPlugin(plugin.id.clone()))?;
        self.stats.created.fetch_add(1, Ordering::Relaxed);
        let changed = self
            .stats
            .changed_layout
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Ok(Box::new(NullInstance {
            layout: changed.unwrap_or_else(|| p.layout.clone()),
            behavior: p.behavior,
            latency: p.latency,
            state: PluginStateBlob::default(),
            held: BTreeMap::new(),
            stats: Arc::clone(&self.stats),
            opening: None,
            deactivating: None,
        }))
    }
}

struct NullInstance {
    layout: PluginLayout,
    behavior: NullBehavior,
    latency: u32,
    state: PluginStateBlob,
    held: BTreeMap<u32, f64>,
    stats: Arc<NullStats>,
    /// Polls left before a slow external editor is open.
    opening: Option<usize>,
    /// Polls left before a slow deactivation is done.
    deactivating: Option<usize>,
}

impl Drop for NullInstance {
    fn drop(&mut self) {
        self.stats.destroyed.fetch_add(1, Ordering::Relaxed);
    }
}

impl NullInstance {
    /// A preset moves every param: the k-th preset puts each at (k + 1) tenths of its range;
    /// no preset leaves every param at its default.
    fn hold_preset(&mut self, index: Option<usize>) {
        self.held = match index {
            Some(k) => {
                let share = (k + 1) as f64 * 0.1;
                self.layout
                    .params
                    .iter()
                    .map(|p| (p.id, p.min + (p.max - p.min) * share))
                    .collect()
            }
            None => BTreeMap::new(),
        };
    }
}

impl PluginInstance for NullInstance {
    fn layout(&self) -> &PluginLayout {
        &self.layout
    }

    fn activate(&mut self, cfg: &ActivateConfig) -> Result<Box<dyn PluginProcessor>, BackendError> {
        if self.stats.fail_activation.swap(false, Ordering::Relaxed) {
            return Err(BackendError::Failed("the null plugin refused".to_string()));
        }
        self.stats
            .last_rate_bits
            .store(cfg.sample_rate.to_bits(), Ordering::Relaxed);
        self.stats.activated.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(NullProcessor {
            behavior: self.behavior,
        }))
    }

    fn deactivate(&mut self, _processor: Box<dyn PluginProcessor>) {
        self.stats.deactivated.fetch_add(1, Ordering::Relaxed);
    }

    fn begin_deactivate(&mut self, processor: Box<dyn PluginProcessor>) -> bool {
        self.deactivate(processor);
        let polls = self.stats.deactivate_delay.load(Ordering::Relaxed);
        self.deactivating = (polls > 0).then_some(polls);
        self.deactivating.is_none()
    }

    fn poll_deactivate(&mut self) -> bool {
        match self.deactivating {
            Some(n) if n > 1 => {
                self.deactivating = Some(n - 1);
                false
            }
            _ => {
                self.deactivating = None;
                true
            }
        }
    }

    fn take_layout_changed(&mut self) -> bool {
        let pending = self
            .stats
            .pending_layout
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(layout) = pending else {
            return false;
        };
        *self
            .stats
            .changed_layout
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(layout.clone());
        self.layout = layout;
        true
    }

    fn latency_frames(&self) -> u32 {
        match self.stats.latency_override.load(Ordering::Relaxed) {
            0 => self.latency,
            n => n - 1,
        }
    }

    fn capture_state(&mut self) -> Result<PluginStateBlob, BackendError> {
        Ok(self.state.clone())
    }

    fn restore_state(&mut self, state: &PluginStateBlob) -> Result<(), BackendError> {
        self.state = state.clone();
        let index = std::str::from_utf8(&state.0)
            .ok()
            .and_then(|s| s.strip_prefix("preset:"))
            .and_then(|id| NULL_PRESETS.iter().position(|(p, _, _)| *p == id));
        self.hold_preset(index);
        Ok(())
    }

    fn list_presets(&mut self) -> Vec<PresetInfo> {
        if !self.stats.presets.load(Ordering::Relaxed) {
            return Vec::new();
        }
        NULL_PRESETS
            .iter()
            .map(|(id, name, group)| PresetInfo {
                id: (*id).to_string(),
                name: (*name).to_string(),
                group: (*group).to_string(),
            })
            .collect()
    }

    fn load_preset(&mut self, id: &str) -> Result<(), BackendError> {
        if !self.stats.presets.load(Ordering::Relaxed) {
            return Err(BackendError::Unsupported(
                "the null plugin has no presets".to_string(),
            ));
        }
        let Some(index) = NULL_PRESETS.iter().position(|(p, _, _)| *p == id) else {
            return Err(BackendError::Failed(format!("no preset `{id}`")));
        };
        self.state = PluginStateBlob(format!("preset:{id}").into_bytes());
        self.hold_preset(Some(index));
        Ok(())
    }

    fn read_params(&mut self) -> Vec<(u32, f64)> {
        self.layout
            .params
            .iter()
            .map(|p| (p.id, self.held.get(&p.id).copied().unwrap_or(p.default)))
            .collect()
    }

    fn param_text(&mut self, _id: u32, plain: f64) -> Option<String> {
        Some(format!("{plain}"))
    }

    fn open_gui(&mut self, _parent: NativeParent, _scale: f64) -> Result<(u32, u32), BackendError> {
        Err(BackendError::Unsupported(
            "the null plugin has no editor".to_string(),
        ))
    }

    fn close_gui(&mut self) {
        if self.editor_kind() == EditorKind::External {
            self.stats.external_closes.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn poll(&mut self) {}

    fn editor_kind(&self) -> EditorKind {
        if self.stats.external_editor.load(Ordering::Relaxed) {
            EditorKind::External
        } else {
            EditorKind::Embedded
        }
    }

    fn has_gui(&self) -> bool {
        self.stats.external_editor.load(Ordering::Relaxed)
    }

    fn open_external_gui(&mut self, _owner: Option<NativeParent>) -> Result<(), BackendError> {
        if self.editor_kind() != EditorKind::External {
            return Err(BackendError::Unsupported(
                "the null plugin has no editor".to_string(),
            ));
        }
        self.stats.external_opens.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn begin_open_external_gui(
        &mut self,
        owner: Option<NativeParent>,
    ) -> Result<bool, BackendError> {
        let delay = self.stats.external_delay.load(Ordering::Relaxed);
        if delay == 0 || self.editor_kind() != EditorKind::External {
            return self.open_external_gui(owner).map(|()| true);
        }
        if self.opening.is_none() {
            self.opening = Some(delay);
        }
        Ok(false)
    }

    fn poll_open_external_gui(&mut self) -> Option<Result<(), BackendError>> {
        let left = self.opening?.saturating_sub(1);
        if left > 0 {
            self.opening = Some(left);
            return None;
        }
        self.opening = None;
        Some(self.open_external_gui(None))
    }

    fn is_lost(&self) -> bool {
        self.stats.lost.load(Ordering::Relaxed)
    }
}

/// The processor of a [`NullPlugin`]: silence, a copy or a scaled copy of its main input.
struct NullProcessor {
    behavior: NullBehavior,
}

impl PluginProcessor for NullProcessor {
    fn start_processing(&mut self) -> bool {
        true
    }

    fn stop_processing(&mut self) {}

    fn reset(&mut self) {}

    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        let gain = match self.behavior {
            NullBehavior::Silent => 0.0,
            NullBehavior::Passthrough => 1.0,
            NullBehavior::Gain(g) => g,
        };
        for (ch, out) in io.main_out.iter_mut().enumerate() {
            match io.main_in.get(ch).or_else(|| io.main_in.first()) {
                Some(src) => {
                    for (o, s) in out.iter_mut().zip(src) {
                        *o = s * gain;
                    }
                }
                None => *out = [0.0; BATCH],
            }
        }
        ProcessStatus::Continue
    }
}
