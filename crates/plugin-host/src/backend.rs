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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

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

/// The main-thread half of one plugin. Every method is `[main-thread]`.
pub trait PluginInstance {
    /// The plugin's layout.
    fn layout(&self) -> &PluginLayout;
    /// Activates at `cfg` and returns the audio half.
    fn activate(&mut self, cfg: &ActivateConfig) -> Result<Box<dyn PluginProcessor>, BackendError>;
    /// Deactivates; the processor must have been stopped and handed back first.
    fn deactivate(&mut self, processor: Box<dyn PluginProcessor>);
    /// The latency the plugin reports now, in frames.
    fn latency_frames(&self) -> u32;
    /// Saves the plugin state.
    fn capture_state(&mut self) -> Result<PluginStateBlob, BackendError>;
    /// Restores a saved state.
    fn restore_state(&mut self, state: &PluginStateBlob) -> Result<(), BackendError>;
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

/// A plugin format. The future out-of-process backend implements the same traits.
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
}

impl NullStats {
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
        Ok(Box::new(NullInstance {
            layout: p.layout.clone(),
            behavior: p.behavior,
            latency: p.latency,
            state: PluginStateBlob::default(),
            stats: Arc::clone(&self.stats),
        }))
    }
}

struct NullInstance {
    layout: PluginLayout,
    behavior: NullBehavior,
    latency: u32,
    state: PluginStateBlob,
    stats: Arc<NullStats>,
}

impl Drop for NullInstance {
    fn drop(&mut self) {
        self.stats.destroyed.fetch_add(1, Ordering::Relaxed);
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
        Ok(())
    }

    fn param_text(&mut self, _id: u32, plain: f64) -> Option<String> {
        Some(format!("{plain}"))
    }

    fn open_gui(&mut self, _parent: NativeParent, _scale: f64) -> Result<(u32, u32), BackendError> {
        Err(BackendError::Unsupported(
            "the null plugin has no editor".to_string(),
        ))
    }

    fn close_gui(&mut self) {}

    fn poll(&mut self) {}
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
