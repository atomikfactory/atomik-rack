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
use crate::soft::{SoftAnswer, SoftBudget};
use atomik_core::{BATCH, Batch, ExprKind};
use atomik_device_sdk::{
    PluginCategory, PluginFormat, PluginLayout, PluginRef, PluginStateBlob, RunMode,
};
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

impl BackendError {
    /// True when the call failed because the plugin is alive but busy and did not answer (see
    /// [`crate::NOT_RESPONDING`]).
    pub fn is_not_responding(&self) -> bool {
        matches!(self, Self::Failed(text) if text.starts_with(crate::NOT_RESPONDING))
    }
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
    budget: SoftBudget,
}

impl HostHandle {
    /// A handle posting to `messenger`, with an allowance of waiting for cosmetic answers of
    /// its own.
    pub fn new(messenger: HostMessenger) -> Self {
        Self::with_budget(messenger, SoftBudget::default())
    }

    /// A handle posting to `messenger` whose instance draws on `budget`, the allowance the
    /// manager shares between all its instances.
    pub fn with_budget(messenger: HostMessenger, budget: SoftBudget) -> Self {
        Self { messenger, budget }
    }

    /// The thread-safe message sender.
    pub fn messenger(&self) -> &HostMessenger {
        &self.messenger
    }

    /// The allowance of waiting for cosmetic answers (see [`SoftBudget`]).
    pub fn budget(&self) -> &SoftBudget {
        &self.budget
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

/// What lists a plugin's presets from files, on any thread (see
/// [`PluginInstance::preset_lister`]).
pub type PresetLister = Box<dyn FnOnce() -> Vec<PresetInfo> + Send>;

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
    /// Starts [`PluginInstance::activate`] without waiting for the plugin: `Some` is the audio
    /// half when the activation was done at once, `None` means [`PluginInstance::poll_activate`]
    /// will tell. A plugin that loads a big library as it activates must not hold up the
    /// interface; the default activates at once.
    ///
    /// # Errors
    /// What [`PluginInstance::activate`] fails with.
    fn begin_activate(
        &mut self,
        cfg: &ActivateConfig,
    ) -> Result<Option<Box<dyn PluginProcessor>>, BackendError> {
        self.activate(cfg).map(Some)
    }
    /// The audio half of the activation that [`PluginInstance::begin_activate`] started, or why
    /// it failed, once; `None` while the plugin has not answered (or none is going on). Never
    /// waits.
    fn poll_activate(&mut self) -> Option<Result<Box<dyn PluginProcessor>, BackendError>> {
        None
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
    /// True when [`PluginInstance::request_capture`] really goes out without waiting (a plugin
    /// in another process, whose main thread may be busy). The default is false: the manager
    /// then captures at the request, which is cheap for a plugin in this process.
    fn background_capture(&self) -> bool {
        false
    }
    /// Asks for the plugin state without waiting; [`PluginInstance::poll_capture`] collects it
    /// when it has arrived. At most one is outstanding (the manager keeps to that). While one is
    /// out, [`PluginInstance::capture_state`] waits for it instead of sending a second request.
    ///
    /// # Errors
    /// The request could not be sent (the plugin is gone).
    fn request_capture(&mut self) -> Result<(), BackendError> {
        Ok(())
    }
    /// The state asked for by [`PluginInstance::request_capture`], or `None` while it has not
    /// come. Never waits and never stops the plugin for being slow.
    fn poll_capture(&mut self) -> Option<Result<PluginStateBlob, BackendError>> {
        None
    }
    /// Restores a saved state.
    fn restore_state(&mut self, state: &PluginStateBlob) -> Result<(), BackendError>;
    /// Starts [`PluginInstance::restore_state`] without waiting for the plugin: true when the
    /// state is restored already, false when [`PluginInstance::poll_restore_state`] will tell.
    /// A sampler that loads its library as it restores may take minutes; the default restores at
    /// once.
    ///
    /// # Errors
    /// What [`PluginInstance::restore_state`] fails with.
    fn begin_restore_state(&mut self, state: &PluginStateBlob) -> Result<bool, BackendError> {
        self.restore_state(state).map(|()| true)
    }
    /// The outcome of the restore that [`PluginInstance::begin_restore_state`] started, once;
    /// `None` while the plugin has not answered. Never waits.
    fn poll_restore_state(&mut self) -> Option<Result<(), BackendError>> {
        Some(Ok(()))
    }
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
    /// A function that lists the presets without the plugin, to run on another thread (the folder
    /// walk of a format that keeps presets as files): the plugin's main thread may be busy while
    /// the files are listed. `None` when listing needs the plugin itself.
    fn preset_lister(&mut self) -> Option<PresetLister> {
        None
    }
    /// [`PluginInstance::list_presets`] for the live app, which does not wait for a plugin that
    /// is busy: `Some(list)` when the answer was there at once (a plugin in this process), `None`
    /// when it comes later as [`SoftAnswer::Presets`] through
    /// [`PluginInstance::take_soft_answers`].
    ///
    /// # Errors
    /// The link to the plugin is lost, stuck on an earlier call or busy with a held one.
    fn begin_list_presets(&mut self) -> Result<Option<Vec<PresetInfo>>, BackendError> {
        Ok(Some(self.list_presets()))
    }
    /// [`PluginInstance::read_params`] for the live app, in the way of
    /// [`PluginInstance::begin_list_presets`]: the values come later as [`SoftAnswer::Params`].
    ///
    /// # Errors
    /// The link to the plugin is lost, stuck on an earlier call or busy with a held one.
    fn begin_read_params(&mut self) -> Result<Option<Vec<(u32, f64)>>, BackendError> {
        Ok(Some(self.read_params()))
    }
    /// [`PluginInstance::load_preset`] for the live app, which does not wait for a plugin that is
    /// loading (a sampler reading its library): true when the preset is loaded, false when it is
    /// being loaded and [`PluginInstance::poll_load_preset`] says when it is done. Offline
    /// renders keep the waiting call.
    ///
    /// # Errors
    /// What [`PluginInstance::load_preset`] fails with.
    fn begin_load_preset(&mut self, id: &str) -> Result<bool, BackendError> {
        self.load_preset(id).map(|()| true)
    }
    /// The outcome of the load that [`PluginInstance::begin_load_preset`] started, once; `None`
    /// while the plugin has not answered. Never waits.
    fn poll_load_preset(&mut self) -> Option<Result<(), BackendError>> {
        Some(Ok(()))
    }
    /// The display text of a param value, from the plugin. A plugin in another process that
    /// does not answer at once gives `None` and the text comes later as a
    /// [`SoftAnswer::ParamText`].
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
    /// [`PluginInstance::resize_gui`] for the interface thread, which must not wait for a plugin
    /// that is busy: `Some(size)` is the answer ([`PluginInstance::resize_gui`] gives it at
    /// once for a plugin in this process), `None` means the request is out (or merged into the
    /// one that is) and the size the editor took comes later as a [`SoftAnswer::GuiSized`]. At
    /// most one request is out at a time; while it is, only the latest size asked for is kept.
    ///
    /// # Errors
    /// What [`PluginInstance::resize_gui`] fails with.
    fn request_resize_gui(&mut self, w: u32, h: u32) -> Result<Option<(u32, u32)>, BackendError> {
        self.resize_gui(w, h).map(Some)
    }
    /// The cosmetic answers that arrived since the last call, oldest first (see [`SoftAnswer`]).
    /// Never waits. Only a plugin in another process has any: for the others every cosmetic
    /// call is answered at once.
    fn take_soft_answers(&mut self) -> Vec<SoftAnswer> {
        Vec::new()
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
    /// Like [`PluginInstance::begin_open_external_gui`], but the editor goes **inside** `parent`,
    /// a window of the app, as a frameless child window instead of a top-level window of its
    /// own. Windows and X11 only; the answer to [`PluginInstance::poll_open_external_gui`] is
    /// the same. After it the editor is sized with [`PluginInstance::resize_gui`], and the
    /// plugin's own resize requests arrive as [`PluginMessage::GuiResize`].
    fn begin_open_embedded_gui(&mut self, _parent: NativeParent) -> Result<bool, BackendError> {
        Err(BackendError::Unsupported(
            "this plugin's editor cannot be shown inside the app window".to_string(),
        ))
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
    /// The name of the call the plugin has not answered although its process is alive (a
    /// sandboxed plugin whose main thread is busy): until the answer comes every call fails at
    /// once with an error that [`BackendError::is_not_responding`] recognises, and the answer
    /// ends the state by itself. `None` when the plugin responds.
    fn not_responding(&self) -> Option<String> {
        None
    }
    /// Stops the plugin's process because the user chose to, after it did not respond (RELOAD):
    /// the instance is lost from then on, as after a crash. Nothing for a plugin in this process.
    fn stop_process(&mut self) {}
    /// The instance is about to be dropped: starts ending whatever outlives a call (a child
    /// process) without waiting for it, so that the manager can start every instance's end
    /// first and then wait for all of them at once. Dropping still works without this call.
    fn begin_shutdown(&mut self) {}
    /// True once what [`PluginInstance::begin_shutdown`] started is over: the child process has
    /// ended (or was stopped after the grace the sandbox allows). Never waits, and starts the
    /// shutdown itself when nobody did. Dropping the instance then does not wait any more. The
    /// default is true: a plugin in this process has nothing that outlives it.
    fn poll_shutdown(&mut self) -> bool {
        true
    }
}

/// One plugin found in a file.
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedPlugin {
    /// Which plugin.
    pub plugin: PluginRef,
    /// Its layout.
    pub layout: PluginLayout,
    /// The category the plugin declares (its CLAP features or VST3 sub-categories);
    /// [`PluginCategory::UNKNOWN`] for an entry scanned before categories were read.
    pub category: PluginCategory,
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
    /// Starts abandoning the start without waiting for it (see
    /// [`PluginInstance::begin_shutdown`]); dropping still works without this call.
    fn begin_shutdown(&mut self) {}
    /// True once the plugin's process that was starting has ended (see
    /// [`PluginInstance::poll_shutdown`]).
    fn poll_shutdown(&mut self) -> bool {
        true
    }
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
    embedded_opens: AtomicUsize,
    /// The app window the last embedded editor went into.
    embedded_parent: Mutex<Option<NativeParent>>,
    /// The sizes the app asked an editor for, in order.
    gui_sizes: Mutex<Vec<(u32, u32)>>,
    /// A size the null editors keep whatever they are asked for (a fixed-size editor).
    fixed_gui_size: Mutex<Option<(u32, u32)>>,
    presets: AtomicBool,
    deactivate_delay: AtomicUsize,
    /// Polls a restore, an activation and a shutdown of a null instance take (a plugin that is
    /// busy but alive), and the polls a start in the background takes plus one (0: instances
    /// are created at once, as a plugin in this process is).
    restore_delay: AtomicUsize,
    activate_delay: AtomicUsize,
    shutdown_delay: AtomicUsize,
    background_create: AtomicUsize,
    /// The next restore of a null instance fails.
    fail_restore: AtomicBool,
    /// The null instances capture in the background, as a plugin in another process does.
    background_capture: AtomicBool,
    /// A background capture is not answered while this is set (a plugin whose main thread is
    /// busy).
    captures_held: AtomicBool,
    /// Background capture requests received.
    capture_requests: AtomicUsize,
    /// Synchronous `capture_state` calls received.
    sync_captures: AtomicUsize,
    /// The null instances are alive but do not answer (see [`NullStats::set_not_responding`]).
    not_responding: AtomicBool,
    /// A state the plugin gives itself (a patch picked in its own window), taken by the null
    /// instances at their next call.
    own_edit: Mutex<Option<Vec<u8>>>,
    /// The layout the instances take at their next restart (see [`NullStats::change_layout`]).
    pending_layout: Mutex<Option<PluginLayout>>,
    /// The layout the plugin has since: new instances start with it.
    changed_layout: Mutex<Option<PluginLayout>>,
    /// The null instances answer no cosmetic request while this is set (see
    /// [`NullStats::set_soft_busy`]).
    soft_busy: AtomicBool,
    /// Polls a preset load of a null instance takes (see [`NullStats::set_preset_delay`]).
    preset_delay: AtomicUsize,
    /// The next list of presets a busy null instance answers fails (see
    /// [`NullStats::fail_next_preset_list`]).
    fail_preset_list: AtomicBool,
    /// Preset lists asked for and param reads asked for through the live-app calls.
    preset_lists: AtomicUsize,
    param_reads: AtomicUsize,
}

impl NullStats {
    /// Makes a preset load take `polls` polls of the instance (a sampler reading its library):
    /// [`PluginInstance::begin_load_preset`] answers "not yet" and
    /// [`PluginInstance::poll_load_preset`] says when it is done.
    pub fn set_preset_delay(&self, polls: usize) {
        self.preset_delay.store(polls, Ordering::Relaxed);
    }
    /// Makes the next preset list that a busy instance (see [`NullStats::set_soft_busy`])
    /// answers a failed one.
    pub fn fail_next_preset_list(&self) {
        self.fail_preset_list.store(true, Ordering::Relaxed);
    }
    /// How many times the presets were asked for with [`PluginInstance::begin_list_presets`].
    pub fn preset_lists(&self) -> usize {
        self.preset_lists.load(Ordering::Relaxed)
    }
    /// How many times the params were asked for with [`PluginInstance::begin_read_params`].
    pub fn param_reads(&self) -> usize {
        self.param_reads.load(Ordering::Relaxed)
    }
    /// Makes a restart's deactivation take `polls` polls of the instance (a plugin that is busy
    /// but alive): [`PluginInstance::begin_deactivate`] answers "not yet" and
    /// [`PluginInstance::poll_deactivate`] says when it is done.
    pub fn set_deactivate_delay(&self, polls: usize) {
        self.deactivate_delay.store(polls, Ordering::Relaxed);
    }
    /// Makes a restore take `polls` polls of the instance: [`PluginInstance::begin_restore_state`]
    /// answers "not yet" and [`PluginInstance::poll_restore_state`] says when it is done.
    pub fn set_restore_delay(&self, polls: usize) {
        self.restore_delay.store(polls, Ordering::Relaxed);
    }
    /// Makes an activation take `polls` polls of the instance (see
    /// [`PluginInstance::begin_activate`]).
    pub fn set_activate_delay(&self, polls: usize) {
        self.activate_delay.store(polls, Ordering::Relaxed);
    }
    /// Makes the end of an instance take `polls` polls (see [`PluginInstance::poll_shutdown`]).
    pub fn set_shutdown_delay(&self, polls: usize) {
        self.shutdown_delay.store(polls, Ordering::Relaxed);
    }
    /// Makes the backend create instances in the background, as a plugin in another process is:
    /// [`PluginBackend::begin_instantiate`] answers a pending start that is ready after `polls`
    /// polls. `None` (the default) creates them at once.
    pub fn set_background_create(&self, polls: Option<usize>) {
        self.background_create
            .store(polls.map_or(0, |p| p + 1), Ordering::Relaxed);
    }
    /// Makes the next restore of a null instance fail.
    pub fn fail_next_restore(&self) {
        self.fail_restore.store(true, Ordering::Relaxed);
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

    /// While set, the null instances are busy with their own work as a plugin in another
    /// process is while it loads a preset: [`PluginInstance::param_text`] gives `None` and
    /// [`PluginInstance::request_resize_gui`] gives `None` (merging the sizes asked for), and
    /// when it is cleared the answers come through [`PluginInstance::take_soft_answers`].
    pub fn set_soft_busy(&self, busy: bool) {
        self.soft_busy.store(busy, Ordering::Relaxed);
    }

    /// Makes the null instances capture in the background like a plugin in another process:
    /// [`PluginInstance::request_capture`] sends nothing to wait for and
    /// [`PluginInstance::poll_capture`] answers when [`NullStats::hold_captures`] lets it.
    pub fn set_background_capture(&self, on: bool) {
        self.background_capture.store(on, Ordering::Relaxed);
    }
    /// While held, a background capture is not answered (the plugin's main thread is busy); the
    /// state it then gives is the one the plugin holds when the hold ends.
    pub fn hold_captures(&self, hold: bool) {
        self.captures_held.store(hold, Ordering::Relaxed);
    }
    /// Makes the null plugin change its own state to `state` (not a restore from the app): the
    /// instances hold it from their next poll, capture or request.
    pub fn edit_state(&self, state: &[u8]) {
        *self.own_edit.lock().unwrap_or_else(PoisonError::into_inner) = Some(state.to_vec());
    }
    /// Makes every null instance alive but not answering, as a sandboxed plugin whose main
    /// thread is busy is once a call has timed out: [`PluginInstance::not_responding`] names a
    /// call and the calls that need an answer (capture, restore, preset load) fail at once with
    /// an error [`BackendError::is_not_responding`] recognises. False ends it, as the plugin's
    /// late answer does.
    pub fn set_not_responding(&self, on: bool) {
        self.not_responding.store(on, Ordering::Relaxed);
    }
    /// Background capture requests the null instances received.
    pub fn capture_requests(&self) -> usize {
        self.capture_requests.load(Ordering::Relaxed)
    }
    /// Synchronous state captures the null instances served.
    pub fn sync_captures(&self) -> usize {
        self.sync_captures.load(Ordering::Relaxed)
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
    /// How many times an external editor was opened inside an app window
    /// ([`PluginInstance::begin_open_embedded_gui`]); each also counts as an external open.
    pub fn embedded_opens(&self) -> usize {
        self.embedded_opens.load(Ordering::Relaxed)
    }
    /// The app window the last embedded editor went into.
    pub fn embedded_parent(&self) -> Option<NativeParent> {
        *self
            .embedded_parent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
    /// The sizes the editors were asked to take ([`PluginInstance::resize_gui`]), in order.
    pub fn gui_sizes(&self) -> Vec<(u32, u32)> {
        self.gui_sizes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    /// Makes every null editor keep `size` whatever it is asked for (`None`: it takes the size
    /// asked for).
    pub fn set_fixed_gui_size(&self, size: Option<(u32, u32)>) {
        *self
            .fixed_gui_size
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = size;
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
                category: PluginCategory::from_kind(p.layout.kind_hint),
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
            capture_out: false,
            opening: None,
            opening_in: None,
            deactivating: None,
            restoring: None,
            activating: None,
            ending: None,
            texts_asked: Vec::new(),
            resize_asked: None,
            presets_asked: false,
            params_asked: false,
            loading: None,
        }))
    }

    fn begin_instantiate(
        &self,
        plugin: &PluginRef,
        host: HostHandle,
    ) -> Result<Option<Box<dyn PendingInstance>>, BackendError> {
        let polls = self.stats.background_create.load(Ordering::Relaxed);
        if polls == 0 {
            return Ok(None);
        }
        let instance = self.instantiate(plugin, host)?;
        Ok(Some(Box::new(NullPending {
            instance: Some(instance),
            polls: polls - 1,
        })))
    }
}

/// A null instance that is "starting in the background" (see
/// [`NullStats::set_background_create`]).
struct NullPending {
    instance: Option<Box<dyn PluginInstance>>,
    polls: usize,
}

impl PendingInstance for NullPending {
    fn poll(&mut self) -> Option<Result<Box<dyn PluginInstance>, BackendError>> {
        if self.polls > 0 {
            self.polls -= 1;
            return None;
        }
        self.instance.take().map(Ok)
    }
}

/// What a slow null activation ends with.
type ActivationResult = Result<Box<dyn PluginProcessor>, BackendError>;

struct NullInstance {
    layout: PluginLayout,
    behavior: NullBehavior,
    latency: u32,
    state: PluginStateBlob,
    held: BTreeMap<u32, f64>,
    stats: Arc<NullStats>,
    /// A background capture was requested and not collected.
    capture_out: bool,
    /// Polls left before a slow external editor is open.
    opening: Option<usize>,
    /// The app window a slow editor that is opening goes into, when it goes into one.
    opening_in: Option<NativeParent>,
    /// Polls left before a slow deactivation is done.
    deactivating: Option<usize>,
    /// A slow restore: the polls left and what it ends with.
    restoring: Option<(usize, Result<(), BackendError>)>,
    /// A slow activation: the polls left and what it ends with.
    activating: Option<(usize, ActivationResult)>,
    /// A slow end: the polls left.
    ending: Option<usize>,
    /// The values whose text was asked for while the instance was busy.
    texts_asked: Vec<(u32, f64)>,
    /// The latest size asked for while the instance was busy.
    resize_asked: Option<(u32, u32)>,
    /// The presets were asked for while the instance was busy.
    presets_asked: bool,
    /// The param values were asked for while the instance was busy.
    params_asked: bool,
    /// A slow preset load: the polls left and what it ends with.
    loading: Option<(usize, Result<(), BackendError>)>,
}

impl Drop for NullInstance {
    fn drop(&mut self) {
        self.stats.destroyed.fetch_add(1, Ordering::Relaxed);
    }
}

impl NullInstance {
    /// The error of a call made while the null plugin is set not to respond (see
    /// [`NullStats::set_not_responding`]).
    fn stuck_error(&self, what: &str) -> Result<(), BackendError> {
        if self.stats.not_responding.load(Ordering::Relaxed) {
            return Err(BackendError::Failed(format!(
                "{}: it has not answered CaptureState yet; {what} was not sent",
                crate::NOT_RESPONDING
            )));
        }
        Ok(())
    }

    /// Takes the state the plugin gave itself since the last call.
    fn take_own_edit(&mut self) {
        let edit = self
            .stats
            .own_edit
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(state) = edit {
            self.state = PluginStateBlob(state);
        }
    }

    /// Counts an editor opened inside `parent`.
    fn note_embedded(&self, parent: NativeParent) {
        self.stats.embedded_opens.fetch_add(1, Ordering::Relaxed);
        *self
            .stats
            .embedded_parent
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(parent);
    }

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

    fn begin_activate(
        &mut self,
        cfg: &ActivateConfig,
    ) -> Result<Option<Box<dyn PluginProcessor>>, BackendError> {
        let done = self.activate(cfg);
        let polls = self.stats.activate_delay.load(Ordering::Relaxed);
        if polls == 0 {
            return done.map(Some);
        }
        self.activating = Some((polls, done));
        Ok(None)
    }

    fn poll_activate(&mut self) -> Option<Result<Box<dyn PluginProcessor>, BackendError>> {
        let (left, done) = self.activating.take()?;
        if left > 1 {
            self.activating = Some((left - 1, done));
            return None;
        }
        Some(done)
    }

    fn begin_restore_state(&mut self, state: &PluginStateBlob) -> Result<bool, BackendError> {
        let done = self.restore_state(state);
        let polls = self.stats.restore_delay.load(Ordering::Relaxed);
        if polls == 0 {
            return done.map(|()| true);
        }
        self.restoring = Some((polls, done));
        Ok(false)
    }

    fn poll_restore_state(&mut self) -> Option<Result<(), BackendError>> {
        let Some((left, done)) = self.restoring.take() else {
            return Some(Ok(()));
        };
        if left > 1 {
            self.restoring = Some((left - 1, done));
            return None;
        }
        Some(done)
    }

    fn poll_shutdown(&mut self) -> bool {
        let left = self
            .ending
            .unwrap_or_else(|| self.stats.shutdown_delay.load(Ordering::Relaxed));
        if left == 0 {
            return true;
        }
        self.ending = Some(left - 1);
        false
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

    fn not_responding(&self) -> Option<String> {
        self.stats
            .not_responding
            .load(Ordering::Relaxed)
            .then(|| "CaptureState".to_string())
    }

    fn capture_state(&mut self) -> Result<PluginStateBlob, BackendError> {
        self.stuck_error("CaptureState")?;
        self.take_own_edit();
        self.stats.sync_captures.fetch_add(1, Ordering::Relaxed);
        self.capture_out = false;
        Ok(self.state.clone())
    }

    fn background_capture(&self) -> bool {
        self.stats.background_capture.load(Ordering::Relaxed)
    }

    fn request_capture(&mut self) -> Result<(), BackendError> {
        self.stuck_error("a request")?;
        self.take_own_edit();
        self.stats.capture_requests.fetch_add(1, Ordering::Relaxed);
        self.capture_out = true;
        Ok(())
    }

    fn poll_capture(&mut self) -> Option<Result<PluginStateBlob, BackendError>> {
        self.take_own_edit();
        if !self.capture_out || self.stats.captures_held.load(Ordering::Relaxed) {
            return None;
        }
        self.capture_out = false;
        Some(Ok(self.state.clone()))
    }

    fn restore_state(&mut self, state: &PluginStateBlob) -> Result<(), BackendError> {
        self.stuck_error("RestoreState")?;
        if self.stats.fail_restore.swap(false, Ordering::Relaxed) {
            return Err(BackendError::Failed(
                "the null plugin could not restore".to_string(),
            ));
        }
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
        self.stuck_error("LoadPreset")?;
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

    fn begin_list_presets(&mut self) -> Result<Option<Vec<PresetInfo>>, BackendError> {
        self.stuck_error("ListPresets")?;
        self.stats.preset_lists.fetch_add(1, Ordering::Relaxed);
        if self.stats.soft_busy.load(Ordering::Relaxed) {
            self.presets_asked = true;
            return Ok(None);
        }
        Ok(Some(self.list_presets()))
    }

    fn begin_read_params(&mut self) -> Result<Option<Vec<(u32, f64)>>, BackendError> {
        self.stuck_error("ReadParams")?;
        self.stats.param_reads.fetch_add(1, Ordering::Relaxed);
        if self.stats.soft_busy.load(Ordering::Relaxed) {
            self.params_asked = true;
            return Ok(None);
        }
        Ok(Some(self.read_params()))
    }

    fn begin_load_preset(&mut self, id: &str) -> Result<bool, BackendError> {
        let done = self.load_preset(id);
        let polls = self.stats.preset_delay.load(Ordering::Relaxed);
        if polls == 0 {
            return done.map(|()| true);
        }
        self.loading = Some((polls, done));
        Ok(false)
    }

    fn poll_load_preset(&mut self) -> Option<Result<(), BackendError>> {
        let Some((left, done)) = self.loading.take() else {
            return Some(Ok(()));
        };
        if left > 1 {
            self.loading = Some((left - 1, done));
            return None;
        }
        Some(done)
    }

    fn param_text(&mut self, id: u32, plain: f64) -> Option<String> {
        if self.stats.soft_busy.load(Ordering::Relaxed) {
            if !self.texts_asked.contains(&(id, plain)) {
                self.texts_asked.push((id, plain));
            }
            return None;
        }
        Some(format!("{plain}"))
    }

    fn request_resize_gui(&mut self, w: u32, h: u32) -> Result<Option<(u32, u32)>, BackendError> {
        if !self.stats.soft_busy.load(Ordering::Relaxed) {
            return self.resize_gui(w, h).map(Some);
        }
        if self.editor_kind() != EditorKind::External {
            return Err(BackendError::Unsupported(
                "the null plugin has no editor".to_string(),
            ));
        }
        self.resize_asked = Some((w, h));
        Ok(None)
    }

    fn take_soft_answers(&mut self) -> Vec<SoftAnswer> {
        if self.stats.soft_busy.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let mut out: Vec<SoftAnswer> = std::mem::take(&mut self.texts_asked)
            .into_iter()
            .map(|(param, plain)| SoftAnswer::ParamText {
                param,
                plain,
                text: format!("{plain}"),
            })
            .collect();
        if std::mem::take(&mut self.presets_asked) {
            let failed = self.stats.fail_preset_list.swap(false, Ordering::Relaxed);
            out.push(SoftAnswer::Presets((!failed).then(|| self.list_presets())));
        }
        if std::mem::take(&mut self.params_asked) {
            out.push(SoftAnswer::Params(Some(self.read_params())));
        }
        if let Some((w, h)) = self.resize_asked.take() {
            out.push(SoftAnswer::GuiSized {
                asked: (w, h),
                took: self.resize_gui(w, h).ok(),
            });
        }
        out
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

    fn poll(&mut self) {
        self.take_own_edit();
    }

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

    fn begin_open_embedded_gui(&mut self, parent: NativeParent) -> Result<bool, BackendError> {
        if self.editor_kind() != EditorKind::External {
            return Err(BackendError::Unsupported(
                "the null plugin has no editor".to_string(),
            ));
        }
        let delay = self.stats.external_delay.load(Ordering::Relaxed);
        if delay == 0 {
            self.note_embedded(parent);
            return self.open_external_gui(None).map(|()| true);
        }
        if self.opening.is_none() {
            self.opening = Some(delay);
            self.opening_in = Some(parent);
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
        if let Some(parent) = self.opening_in.take() {
            self.note_embedded(parent);
        }
        Some(self.open_external_gui(None))
    }

    fn resize_gui(&mut self, w: u32, h: u32) -> Result<(u32, u32), BackendError> {
        if self.editor_kind() != EditorKind::External {
            return Err(BackendError::Unsupported(
                "the null plugin has no editor".to_string(),
            ));
        }
        self.stats
            .gui_sizes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((w, h));
        let fixed = *self
            .stats
            .fixed_gui_size
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Ok(fixed.unwrap_or((w, h)))
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
