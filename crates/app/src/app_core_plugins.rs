//! Third-party plugins in the app core.
//!
//! # Who owns what
//!
//! The winit (main) thread owns the [`PluginHost`] (the manager with the CLAP and VST3 backends and the
//! catalog); the engine gets only the audio half of each instance inside a `PluginDevice`. Every
//! method here runs on that thread. [`AppCore::frame`] calls [`AppCore::plugins_frame`]: it polls
//! the manager (reaping, plugin callbacks, restarts), creates **one** pending instance, advances
//! the scan job and feeds the views.
//!
//! # Catalog and stand-ins
//!
//! [`AppCore::configure_plugins`] loads the settings and the scan cache; the catalog registers
//! every usable plugin as a device type without loading a file, and
//! [`atomik_model::register_stand_ins`] covers the plugins a document uses that are not installed.
//! A project never carries a path: a plugin device is resolved by `(format, id)` against the
//! catalog only. After every load and every rescan [`AppCore::refresh_plugin_catalog`] runs.
//!
//! # The insert path
//!
//! `insert_flat` never builds a plugin itself: for a plugin device it inserts the **stand-in**
//! (`Registry::create`), or the live device of a parked instance after an engine restart
//! (`reattach`, the plugin is not created again). A stand-in whose plugin is installed is queued,
//! and each frame creates one instance and swaps it in: the new device is inserted under a fresh
//! id, the routing and the song are resubmitted for it, and the stand-in is removed after that.
//! So opening a project with eight plugins shows "Loading plugins 3/8" and plays (silent stand-ins)
//! meanwhile. Order for the live device is the usual one: insert, params, routing, song.
//!
//! # State
//!
//! The doc's `PluginDoc::state` is refreshed by `SyncState` (never an undo step) before every
//! save and Save As, before a recovery snapshot, before any command or undo that could remove a
//! device (instances flagged state-dirty only), before an engine restart, when the editor
//! closes and one quiet second after the plugin called `state.mark_dirty`
//! ([`AppCore::plugin_tick`]). An instance whose saved state failed to restore is never captured
//! (the saved blob is kept) until RELOAD restores it.
//!
//! # GUI edits
//!
//! `ManagerEvent::ParamEdit` markers become `SetParam`s through the UI's gesture merger: one
//! gesture is one undo step; the commands carry a "from plugin" flag so the sync op does not echo
//! the value back. A param change outside a gesture is dropped. A patch change made in the
//! plugin window (a preset picked in it, say) is stored as one undo step (`SetState`) at the
//! next quiet capture, when the editor closes and before an undo, unless the user turned a param
//! in the window meanwhile (that gesture is its own step); undo hands the old state back to the
//! live plugin.
//!
//! # Presets
//!
//! [`AppCore::plugin_presets`] lists what the plugin offers (asked for once per instance and
//! shown when the answer lands, never waited for), [`AppCore::load_plugin_preset`] and
//! [`AppCore::step_plugin_preset`] load one into the live plugin (a polled step: the frames carry
//! it on) and store the new state as one undo step. The parameter pane shows the list and
//! the one loaded last.
//!
//! # Window geometry
//!
//! Position and size of each editor window are kept per plugin type when it closes
//! ([`AppCore::plugin_window_geoms`], saved in `ui-state.json`); the next window of that type
//! opens there, on a screen that still exists.
//!
//! # Where plugins run
//!
//! [`PluginConfig::isolation`] (from `PluginSettings::sandbox` at start) decides whether the
//! host builds in-process backends or one child process per instance; a change in Settings
//! applies at the next start. A sandboxed instance whose process ends is **CRASHED**
//! (`ManagerEvent::Crashed`): its device passes the dry signal or silence, the panel shows the
//! reason, and RELOAD replaces the instance (the old one cannot be restarted) with a new one
//! built from the document's last captured state and the rack's params. A sandboxed plugin
//! cannot crash the app, so it is not journaled.
//!
//! An [`EditorKind::External`] editor is a window of the plugin's own process: OPEN GUI calls
//! `open_external_editor` with the app window's native handle ([`AppCore::set_plugin_editor_owner`])
//! and asks the platform for nothing.
//!
//! # Crash handling
//!
//! A [`CrashJournal`] lists the loaded plugins and is removed on a clean exit; at the next start
//! [`PluginConfig::previous_crash`] shows the unclean-exit banner. [`AppCore::plugin_tick`] writes
//! a recovery snapshot of a changed project every autosave period (default 5 minutes, every 60 s
//! while a plugin is loaded) and after each plugin state capture, to `<config>/recovery`,
//! keeping the last three, never over the user's file. The snapshots are written by one thread
//! ([`crate::recovery::RecoveryWriter`]). A session marker in the same folder tells the next start
//! that a run did not quit cleanly, which shows the banner for any project, plugins or not.

use super::AppCore;
use crate::plugin_cache_writer::CacheWriter;
use crate::plugins_user::PluginsUser;
use crate::recovery::{RecoveryWriter, SessionMarker};
use crate::sink::EngineSink;
use crate::ui_state::PluginWindowGeom;
use atomik_device_sdk::{Device, DeviceDescriptor, PluginRef, RunMode};
use atomik_engine::DeviceInstanceId;
use atomik_model::{
    Command, DeviceKey, Document, ExpandedDevice, ModelError, PLUGIN_GESTURE_MERGE_BASE,
    PluginCommand, PluginDoc, RackDoc, SyncOp, register_stand_ins,
};
use atomik_plugin_host::ScanReport;
use atomik_plugin_host::{
    CrashJournal, CreateTicket, EditorKind, HUNG, InUsePlugin, InstanceId, InstantiateRequest,
    LOST, LogLevel, ManagerError, ManagerEvent, NativeParent, ParamPhase, PluginCache,
    PluginCatalog, PluginHandle, PluginSettings, PluginStatus, PresetInfo, RecoveryDir, RescanMode,
    ScanJob, ScanRunner, SoftAnswer, blacklist_in_use,
};
use atomik_render::plugins::{PluginHost, PluginIsolation};
use atomik_ui::plugin_data::{
    FailedFileInfo, PluginBannerAction, PluginDeviceStatus, PluginEditorHome, PluginEntryInfo,
    PluginEntryStatus, PluginMarks, PluginProcessMode, PluginRescan, PluginSettingsAction,
    PluginSettingsData, PresetItem, ScanProgressInfo, SettingsPluginRow,
};
use atomik_ui::{BannerKind, PluginGestureMerger, PluginParamsView, UiAction};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[path = "app_core_plugin_dock.rs"]
mod dock;
pub use dock::{DockRequest, DockedEditor, EditorPlacement};
#[path = "app_core_plugins_keep.rs"]
mod keep;
#[path = "app_core_plugin_learn.rs"]
mod learn;
pub(super) use learn::TOUCH_GAP_SECS;

/// Seconds between recovery snapshots of a changed project while a plugin is loaded.
pub const RECOVERY_PERIOD_SECS: f64 = 60.0;
/// The autosave period until the preference says otherwise.
pub const DEFAULT_AUTOSAVE_MINUTES: f64 = 5.0;
/// How many recovery snapshots are kept.
pub const RECOVERY_KEEP: usize = 3;
/// The quiet time after `state.mark_dirty` before the state is captured.
pub const STATE_QUIET_SECS: f64 = 1.0;
/// How long a recovery snapshot waits for the plugin states it asked for before it is written
/// with the states stored so far.
pub const SNAPSHOT_GRACE_SECS: f64 = 2.0;
/// Lines of the bounded plugin log shown in Settings.
pub const LOG_CAPACITY: usize = 200;

/// How long [`AppCore::wait_plugins`] waits for pending instances.
const WAIT_PLUGINS_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);
/// Merge ids of the undo steps that capture a patch change made in a plugin window.
const STATE_MERGE_BASE: u64 = PLUGIN_GESTURE_MERGE_BASE + (1 << 38);
/// Captures further apart than this are separate undo steps.
const STATE_STEP_GAP_SECS: f64 = 5.0;
/// How many times in a row a plugin that stopped answering audio is reloaded without the user.
const MAX_AUTO_RELOADS: u8 = 2;
/// An instance is created again for a port mismatch at most once per this long, per plugin and
/// port shape (a guard against a recreate loop).
const RECREATE_WINDOW_SECS: u64 = 30;

/// True when both descriptors have the same sockets (the port shape; params are not compared).
fn same_shape(a: &DeviceDescriptor, b: &DeviceDescriptor) -> bool {
    std::ptr::eq(a, b)
        || a.sockets
            .iter()
            .map(|s| s.key)
            .eq(b.sockets.iter().map(|s| s.key))
}

/// The files the plugin machinery keeps in the config directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginPaths {
    /// `plugins.json`, the scan cache.
    pub cache: Option<PathBuf>,
    /// `plugins-in-use.json`, the crash journal.
    pub journal: Option<PathBuf>,
    /// `recovery/`, the recovery snapshots and the session marker.
    pub recovery: Option<PathBuf>,
    /// The folder that holds the session scratch bundles, scanned at start for unfinished
    /// recordings of a session that ended unexpectedly (the OS temp folder).
    pub scratch: Option<PathBuf>,
}

impl PluginPaths {
    /// The three files inside the config directory `dir`.
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            cache: Some(dir.join("plugins.json")),
            journal: Some(dir.join("plugins-in-use.json")),
            recovery: Some(dir.join(crate::recovery::RECOVERY_FOLDER)),
            scratch: Some(std::env::temp_dir()),
        }
    }
}

/// What [`AppCore::configure_plugins`] needs (the platform layer reads the files).
#[derive(Default)]
pub struct PluginConfig {
    /// The user's plugin settings (off by default).
    pub settings: PluginSettings,
    /// The scan cache read from `plugins.json`.
    pub cache: PluginCache,
    /// Where the cache, the journal and the snapshots live.
    pub paths: PluginPaths,
    /// What scans one file (the real one starts `atomik-plugin-scan` children).
    pub runner: Option<Arc<dyn ScanRunner>>,
    /// The plugins the previous run left in the journal (it ended while they were loaded).
    pub previous_crash: Option<Vec<InUsePlugin>>,
    /// Where plugins run, read once at start (`None`: the core's host as it is, in process).
    pub isolation: Option<PluginIsolation>,
}

impl std::fmt::Debug for PluginConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginConfig")
            .field("settings", &self.settings)
            .field("paths", &self.paths)
            .finish_non_exhaustive()
    }
}

/// A request for the platform layer about a plugin editor window. The core never
/// touches a window: the platform fulfils these and answers with the `plugin_window_*` calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginWindowRequest {
    /// Create a top-level window for the editor of `device`, then call
    /// [`AppCore::plugin_window_created`] with its native handle.
    Open {
        /// The plugin device (path key).
        device: DeviceKey,
        /// The window title: `<plugin> - <device name>`.
        title: String,
        /// The last size of this device's window (logical pixels), when it was open before.
        remembered: Option<(u32, u32)>,
        /// Where this plugin's window was last (physical pixels), when it was open before.
        at: Option<(i32, i32)>,
    },
    /// Resize the window's content area to this size (pixels), because the plugin asked or
    /// because its view reported another size.
    Resize {
        /// The plugin device.
        device: DeviceKey,
        /// Width in pixels.
        w: u32,
        /// Height in pixels.
        h: u32,
    },
    /// Show the window.
    Show {
        /// The plugin device.
        device: DeviceKey,
    },
    /// Hide the window.
    Hide {
        /// The plugin device.
        device: DeviceKey,
    },
    /// Destroy the window (the view is already destroyed).
    Close {
        /// The plugin device.
        device: DeviceKey,
    },
}

/// The native window a platform layer made for an editor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PluginWindowInfo {
    /// The handle the plugin embeds its view into.
    pub parent: atomik_plugin_host::NativeParent,
    /// The window's scale factor (CLAP `set_scale`, 1.0 where it does not apply).
    pub scale: f64,
}

/// An open or opening editor window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorWindow {
    /// The platform was asked to create it.
    Opening,
    /// The view is embedded.
    Open,
    /// The plugin's own process shows the window (an [`EditorKind::External`] editor).
    External,
    /// The plugin's own process was asked to open its window and has not answered yet: it may
    /// be checking its licence or showing its own dialogs first (minutes, not seconds).
    ExternalOpening,
}

/// The toast while an external editor opens.
pub(crate) const EDITOR_WAITING: &str = "waiting for the plugin to open its editor (it may show its own licence or sign-in dialogs first)";

/// The app log line of a plugin process that ended on its own or was stopped: the reason holds
/// why, the exit code and the last lines the process wrote to stderr.
pub(crate) fn crash_log_line(device: &str, reason: &str) -> String {
    format!("plugin [{device}] process ended: {reason}")
}

/// The app log line of a plugin whose process is alive but did not answer a call in time: it is
/// left running.
pub(crate) fn not_responding_log_line(device: &str, call: &str) -> String {
    format!(
        "plugin [{device}] not responding: the process is alive but did not answer {call}; it is left running and asked no more until it answers"
    )
}

/// The app log line of a plugin that answered again after [`not_responding_log_line`].
pub(crate) fn responding_log_line(device: &str) -> String {
    format!("plugin [{device}] responding again")
}

/// The notice after a save, when the plugin of one device was alive but did not hand over its
/// current state (`stored`: the document held a state from an earlier capture).
pub(crate) fn not_handed_notice(names: &[String], stored: bool) -> String {
    match names {
        [one] if stored => format!(
            "{one} did not hand over its current settings; its last captured settings were saved."
        ),
        [one] => format!(
            "{one} did not hand over its current settings and none were captured before; it was saved without them."
        ),
        many => format!(
            "{} did not hand over their current settings; their last captured settings were saved.",
            many.join(", ")
        ),
    }
}

/// Why a plugin's state cannot be stored: it is over a size cap. The device carries one of these
/// (see `PluginState::too_large`) from the capture that was refused until a capture fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TooLarge {
    /// The size that is over the cap: the state, or the total of the project's plugin states.
    pub bytes: u64,
    /// The cap, in bytes.
    pub limit: u64,
    /// The cap is the project's total, not the device's own (or the channel's).
    pub project: bool,
    /// The plugin could not hand the state over at all: it does not fit one frame of the channel
    /// of a plugin in its own process.
    pub frame: bool,
}

impl TooLarge {
    /// The situation, without the size: what a log line is written again for.
    fn situation(self) -> (u64, bool, bool) {
        (self.limit, self.project, self.frame)
    }
}

/// A size in megabytes, rounded up so that a state over its limit never reads as equal to it.
pub(crate) fn megabytes(bytes: u64) -> u64 {
    bytes.div_ceil(1 << 20)
}

/// The notice after a save, when the state of these devices (each with what was refused) was
/// too large to store (`stored`: the document holds an earlier state that fitted for all of
/// them). `names` and `sizes` run in step.
pub(crate) fn too_large_notice(names: &[String], sizes: &[TooLarge], stored: bool) -> String {
    match (names, sizes) {
        ([one], [t]) => {
            let (size, limit) = (megabytes(t.bytes), megabytes(t.limit));
            match (t.project, stored) {
                (true, true) => format!(
                    "{one} is too large to save (the plugin states of the project would total {size} MB, the limit is {limit} MB); its last state that fitted was saved."
                ),
                (true, false) => format!(
                    "{one} is too large to save (the plugin states of the project would total {size} MB, the limit is {limit} MB); it was saved without a state."
                ),
                (false, true) => format!(
                    "{one} is too large to save ({size} MB, the limit is {limit} MB); its last state that fitted was saved."
                ),
                (false, false) => format!(
                    "{one} is too large to save ({size} MB, the limit is {limit} MB); it was saved without a state."
                ),
            }
        }
        (many, _) => format!(
            "{} are too large to save; their last states that fitted were saved.",
            many.join(", ")
        ),
    }
}

/// The notice after a save when some plugins did not hand over their state and the state of
/// others was too large: one text for both, so that a save still says one thing.
pub(crate) fn unsaved_settings_notice(names: &[String]) -> String {
    format!(
        "The current settings of {} could not be saved; their last saved settings were kept.",
        names.join(", ")
    )
}

/// The app log line of a state that cannot be stored (written when the situation changes).
pub(crate) fn too_large_log_line(device: &str, t: &TooLarge) -> String {
    if t.frame {
        format!(
            "[{device}] cannot hand over its state: {} bytes, the plugin channel carries at most {}; the plugin keeps running and the stored state is kept",
            t.bytes, t.limit
        )
    } else if t.project {
        format!(
            "[{device}] state not stored: the plugin states of the project would total {} bytes, the limit is {}; the last state that fitted is kept",
            t.bytes, t.limit
        )
    } else {
        format!(
            "[{device}] state not stored: {} bytes, the limit is {}; the last state that fitted is kept",
            t.bytes, t.limit
        )
    }
}

/// The app log line of a state that fits again.
pub(crate) fn fits_again_log_line(device: &str, bytes: usize) -> String {
    format!("[{device}] state is stored again: {bytes} bytes fit")
}

/// The app log line of a plugin device that went FAILED, with the full reason.
pub(crate) fn failed_log_line(device: &str, reason: &str) -> String {
    format!("plugin [{device}] failed: {reason}")
}

/// The app log line of an editor that was asked for: the device key, the instance id and the
/// plugin's process (`again`: the app already knew the editor as open or opening).
pub(crate) fn editor_requested_line(
    device: &str,
    instance: InstanceId,
    process: Option<usize>,
    again: bool,
) -> String {
    let process = process.map_or_else(|| "none".to_string(), |p| p.to_string());
    let again = if again {
        " (already open or opening)"
    } else {
        ""
    };
    format!(
        "plugin [{device}] editor requested: instance {}, process {process}{again}",
        instance.0
    )
}

/// The app log line of an editor that is open.
pub(crate) fn editor_opened_line(device: &str, instance: InstanceId) -> String {
    format!("plugin [{device}] editor opened: instance {}", instance.0)
}

/// The app log line of an editor that did not open.
pub(crate) fn editor_failed_line(device: &str, instance: InstanceId, reason: &str) -> String {
    format!(
        "plugin [{device}] editor did not open: instance {}: {reason}",
        instance.0
    )
}

/// The app log line of an editor that closed.
pub(crate) fn editor_closed_line(device: &str, instance: Option<InstanceId>) -> String {
    let instance = instance.map_or_else(|| "none".to_string(), |i| i.0.to_string());
    format!("plugin [{device}] editor closed: instance {instance}")
}

/// The stages of a plugin creation, for the load line.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct CreatedStages {
    /// Starting the plugin process (`begin_create`).
    pub spawn: std::time::Duration,
    /// The process loading the plugin until the instance was ready (polled once a frame, and a
    /// plugin waits here for the ones queued before it).
    pub instantiate: std::time::Duration,
    /// Restoring the saved state.
    pub restore: std::time::Duration,
    /// Activating the plugin.
    pub activate: std::time::Duration,
    /// Attaching the instance to the rack and the engine.
    pub attach: std::time::Duration,
}

/// `created in 2.31 s (spawn 0.04, instantiate 1.90, restore 0.00, activate 0.31, attach 0.06)`:
/// how long a plugin took to load and where the time went.
pub(crate) fn created_text(took: std::time::Duration, s: &CreatedStages) -> String {
    let secs = std::time::Duration::as_secs_f64;
    format!(
        "created in {:.2} s (spawn {:.2}, instantiate {:.2}, restore {:.2}, activate {:.2}, attach {:.2})",
        secs(&took),
        secs(&s.spawn),
        secs(&s.instantiate),
        secs(&s.restore),
        secs(&s.activate),
        secs(&s.attach)
    )
}

/// The app log line of a plugin that was created.
pub(crate) fn created_line(device: &str, took: std::time::Duration, s: &CreatedStages) -> String {
    format!("plugin [{device}] {}", created_text(took, s))
}

/// The app log line of something a plugin (or its host process) said at warning level or worse.
/// A panic of the plugin in its host process reads `plugin [<key>] panicked in the plugin host
/// during <op>: <message> at <file>:<line>:<col>`, followed by the child's last stderr lines.
pub(crate) fn plugin_said_line(device: &str, text: &str) -> String {
    format!("plugin [{device}] {text}")
}

/// The recovery snapshot state.
#[derive(Debug)]
struct Recovery {
    dir: RecoveryDir,
    /// When the last snapshot was queued (the first tick starts the clock).
    last: Option<f64>,
    wanted: bool,
    /// The sequence number of the last snapshot name (names only ever grow).
    sequence: u64,
    /// The one thread that serialises, writes and prunes.
    writer: Option<RecoveryWriter>,
    /// The document revision of the last snapshot: no change since means nothing to write.
    revision: Option<u64>,
    /// Tells the next start whether this run ended cleanly.
    marker: Option<SessionMarker>,
}

/// What a background state capture is for when it arrives.
#[derive(Debug, Default, Clone)]
pub(super) struct Capturing {
    /// A state that differs from the stored one becomes an undo step (the user changed the patch in
    /// the plugin's window, or closed it) instead of being stored silently.
    undoable: bool,
    /// The state is the result of loading this preset: it becomes the one undo step of the
    /// load (see [`AppCore::load_plugin_preset`]).
    preset: Option<String>,
}

/// What the app knows of the presets a live plugin offers.
#[derive(Debug, Clone)]
pub(super) enum PresetList {
    /// They were asked for and the list has not come yet.
    Asking,
    /// The plugin's presets (none for a plugin that has none).
    Ready(Vec<PresetInfo>),
    /// The plugin did not list them, at this time of the frame clock: they are asked for again
    /// when the pane or the browser opens, not sooner than [`PRESET_RETRY_SECS`] later.
    Failed(f64),
}

/// How long a failed list of presets is left alone before the next opening asks again.
const PRESET_RETRY_SECS: f64 = 5.0;

/// Where a preset load is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadStage {
    /// The state the plugin holds is being captured, so that the step goes back to it.
    Before,
    /// The plugin is loading the preset.
    Loading,
}

/// A preset being loaded into a live plugin without the interface waiting for it.
#[derive(Debug, Clone)]
struct PresetLoad {
    preset: String,
    stage: LoadStage,
}

/// How many plugin processes load at the same time when a project opens.
pub(super) const MAX_STARTING: usize = 8;

/// A pending device whose plugin is loading.
struct Starting {
    ticket: CreateTicket,
    plugin: PluginRef,
    began: std::time::Instant,
    /// How long `begin_create` took (the process starts).
    spawn: std::time::Duration,
}

/// Plugin state of the app core.
pub(super) struct PluginState {
    pub(super) host: PluginHost,
    pub(super) settings: PluginSettings,
    pub(super) cache: PluginCache,
    /// The sandbox of the host as a bounce uses it (the freeze renders with plugins need it);
    /// `None` while the plugins run in the app process.
    pub(super) freeze_isolation: Option<PluginIsolation>,
    paths: PluginPaths,
    runner: Option<Arc<dyn ScanRunner>>,
    job: Option<ScanJob>,
    /// The environment scans read the default folders from (`None`: the process environment).
    scan_env: Option<atomik_plugin_host::ScanEnv>,
    scan_note: Option<String>,
    /// Flat key to the live instance behind it.
    pub(super) instances: BTreeMap<DeviceKey, InstanceId>,
    keys: BTreeMap<InstanceId, DeviceKey>,
    loaded: BTreeMap<InstanceId, InUsePlugin>,
    /// Devices whose instance is waiting to be created, in order.
    pub(super) pending: VecDeque<DeviceKey>,
    /// The pending devices whose plugin process has been started: every one of them loads, and
    /// the ones that are ready attach in queue order (see [`AppCore::instantiate_one`]).
    starting: BTreeMap<DeviceKey, Starting>,
    /// Devices whose creation the user skipped: stand-ins that keep their saved state, nothing
    /// queues them again until RELOAD (a skipped device is not crashed and not quarantined).
    skipped: BTreeSet<DeviceKey>,
    /// How many instances the current batch of creations started with (for "3/8").
    batch_total: usize,
    /// Seconds between autosave snapshots of a changed project (0: off).
    autosave_secs: f64,
    batch_done: usize,
    /// Why a device is FAILED (an error, a restore that failed, a creation that failed).
    failed: BTreeMap<DeviceKey, String>,
    /// Why a device is CRASHED: its plugin process ended (sandbox).
    crashed: BTreeMap<DeviceKey, String>,
    /// The devices whose plugin was alive but did not hand over its state to a capture since a
    /// save began: what they hold in the document is the last state captured (see
    /// [`AppCore::plugins_capture_for_save`]).
    pub(super) not_handed: Vec<DeviceKey>,
    /// The devices whose state is over a size cap and cannot be stored: what was refused. Set by
    /// the capture that was refused, cleared by the first one that fits (the stored state is
    /// the last one that fitted meanwhile); the device panel shows it.
    pub(super) too_large: BTreeMap<DeviceKey, TooLarge>,
    /// The devices whose capture was refused for size since a save began (see
    /// [`AppCore::plugins_capture_for_save`]).
    pub(super) too_large_hit: Vec<DeviceKey>,
    /// How many times a hung plugin was reloaded by itself since the user last pressed RELOAD.
    auto_reloads: BTreeMap<DeviceKey, u8>,
    /// Devices whose saved state failed to restore: the blob is kept, never captured over.
    no_capture: BTreeSet<DeviceKey>,
    /// The descriptor each live device was inserted with.
    live_desc: BTreeMap<DeviceKey, &'static DeviceDescriptor>,
    /// When each instance was last created again for a port mismatch, by device, plugin and port
    /// shape.
    recreated: BTreeMap<(DeviceKey, String, String), u64>,
    /// Devices whose instance was created while no engine ran: it lives parked (editor and
    /// params work) and is attached to the engine as soon as one exists.
    offline: BTreeSet<DeviceKey>,
    /// A platform loop that paints between frames asks to show LOADING one frame before the
    /// (blocking) creation of an instance starts.
    paint_first: bool,
    shown: BTreeMap<DeviceKey, PluginDeviceStatus>,
    catalog_dirty: bool,
    /// The scan cache learned a layout from a live plugin: `plugins.json` is saved at the next frame.
    cache_dirty: bool,
    merger: PluginGestureMerger,
    open_gestures: BTreeSet<(DeviceKey, u32)>,
    from_plugin: bool,
    /// Devices whose params the user edited in the plugin window since the last state capture:
    /// their state change is already an undo step of its own (the param gesture).
    pub(super) gesture_touched: BTreeSet<DeviceKey>,
    /// The presets each live device's plugin offers (asked for when the pane, the browser or a
    /// step needs them, and used when the answer lands).
    presets: BTreeMap<DeviceKey, PresetList>,
    /// The presets being loaded into live plugins.
    preset_loads: BTreeMap<DeviceKey, PresetLoad>,
    /// The devices whose param values were asked for after a patch change, with the document
    /// revision the answer may still join the undo step of.
    knob_reads: BTreeMap<DeviceKey, u64>,
    /// The thread that writes the scan cache (started when the first write is due).
    cache_writer: Option<CacheWriter>,
    /// The preset loaded last into each device, until anything else changes its state.
    preset_current: BTreeMap<DeviceKey, String>,
    /// Merge id of the state captures that make one undo step, and when the last one was.
    state_merge: (u64, f64),
    /// The next engine restart drops the instances instead of keeping them (a new document).
    discard: bool,
    /// Instances whose state the plugin flagged dirty, with the time of the last flag.
    dirty: BTreeMap<InstanceId, f64>,
    /// The instances whose state was asked for in the background and has not come back, with
    /// what its arrival is for.
    pub(super) capturing: BTreeMap<InstanceId, Capturing>,
    /// When a periodic recovery snapshot began to wait for the states it asked for.
    snapshot_wait: Option<f64>,
    clock: f64,
    journal: Option<CrashJournal>,
    recovery: Option<Recovery>,
    previous_crash: Option<Vec<InUsePlugin>>,
    banner: Option<BannerKind>,
    banner_changed: bool,
    log: VecDeque<String>,
    windows: BTreeMap<DeviceKey, EditorWindow>,
    remembered: BTreeMap<DeviceKey, (u32, u32)>,
    /// The outer position of each open editor window, as the platform reported it.
    window_pos: BTreeMap<DeviceKey, (i32, i32)>,
    /// Where each plugin type's window was last (kept in `ui-state.json`).
    geoms: BTreeMap<String, PluginWindowGeom>,
    /// A geometry changed since the shell last read them.
    geoms_changed: bool,
    window_requests: Vec<PluginWindowRequest>,
    /// The app window's native handle: the owner of external editors.
    editor_owner: Option<NativeParent>,
    /// The Learn modes of the editor windows (see `app_core_plugin_learn.rs`).
    learn: learn::PluginLearn,
    /// The editors shown inside the app window (see `app_core_plugin_dock.rs`).
    dock: dock::PluginDock,
    /// The editor window the user brought forward last.
    focused_window: Option<DeviceKey>,
    /// The outer frame of each external editor's window (physical pixels), as its process said.
    frames: BTreeMap<DeviceKey, (i32, i32, u32, u32)>,
    settings_changed: bool,
    folder_dialog: bool,
    /// What the user decided about each plugin: stars, tags, disabled, quarantine.
    pub(super) user: PluginsUser,
    /// `user` changed since the shell last saved it.
    user_changed: bool,
    /// Plugins whose last failure was a failed load: a load that works ends the streak.
    load_failed: BTreeSet<String>,
    /// Instances that crashed or failed: their end is not a success.
    bad: BTreeSet<InstanceId>,
    /// Devices whose quarantined plugin the user chose to load anyway (until the document is
    /// replaced).
    load_anyway: BTreeSet<DeviceKey>,
}

impl PluginState {
    pub(super) fn new() -> Self {
        Self {
            host: PluginHost::new(),
            settings: PluginSettings::default(),
            cache: PluginCache::default(),
            freeze_isolation: None,
            paths: PluginPaths::default(),
            runner: None,
            job: None,
            scan_env: None,
            scan_note: None,
            instances: BTreeMap::new(),
            keys: BTreeMap::new(),
            loaded: BTreeMap::new(),
            pending: VecDeque::new(),
            starting: BTreeMap::new(),
            skipped: BTreeSet::new(),
            batch_total: 0,
            autosave_secs: DEFAULT_AUTOSAVE_MINUTES * 60.0,
            batch_done: 0,
            failed: BTreeMap::new(),
            crashed: BTreeMap::new(),
            not_handed: Vec::new(),
            too_large: BTreeMap::new(),
            too_large_hit: Vec::new(),
            auto_reloads: BTreeMap::new(),
            no_capture: BTreeSet::new(),
            live_desc: BTreeMap::new(),
            recreated: BTreeMap::new(),
            offline: BTreeSet::new(),
            paint_first: false,
            shown: BTreeMap::new(),
            catalog_dirty: true,
            cache_dirty: false,
            merger: PluginGestureMerger::new(),
            open_gestures: BTreeSet::new(),
            from_plugin: false,
            gesture_touched: BTreeSet::new(),
            presets: BTreeMap::new(),
            preset_loads: BTreeMap::new(),
            knob_reads: BTreeMap::new(),
            cache_writer: None,
            preset_current: BTreeMap::new(),
            state_merge: (STATE_MERGE_BASE, f64::NEG_INFINITY),
            discard: false,
            dirty: BTreeMap::new(),
            capturing: BTreeMap::new(),
            snapshot_wait: None,
            clock: 0.0,
            journal: None,
            recovery: None,
            previous_crash: None,
            banner: None,
            banner_changed: false,
            log: VecDeque::new(),
            windows: BTreeMap::new(),
            remembered: BTreeMap::new(),
            window_pos: BTreeMap::new(),
            geoms: BTreeMap::new(),
            geoms_changed: false,
            window_requests: Vec::new(),
            editor_owner: None,
            learn: learn::PluginLearn::default(),
            dock: dock::PluginDock::default(),
            focused_window: None,
            frames: BTreeMap::new(),
            settings_changed: false,
            folder_dialog: false,
            user: PluginsUser::default(),
            user_changed: false,
            load_failed: BTreeSet::new(),
            bad: BTreeSet::new(),
            load_anyway: BTreeSet::new(),
        }
    }

    fn log(&mut self, line: impl Into<String>) {
        if self.log.len() == LOG_CAPACITY {
            self.log.pop_front();
        }
        self.log.push_back(line.into());
    }

    fn show_banner(&mut self, kind: BannerKind) {
        self.banner = Some(kind);
        self.banner_changed = true;
    }

    fn hide_banner(&mut self) {
        if self.banner.take().is_some() {
            self.banner_changed = true;
        }
    }
}

/// The flat plugin devices of an expansion.
pub(super) fn plugin_keys(expanded: &atomik_model::ExpandedRack) -> Vec<DeviceKey> {
    expanded
        .devices
        .iter()
        .filter(|d| d.plugin.is_some())
        .map(|d| d.key.clone())
        .collect()
}

/// The flat plugin devices of an expansion that publish meter values.
pub(super) fn metered_with_plugins(expanded: &atomik_model::ExpandedRack) -> Vec<DeviceKey> {
    let mut keys = super::containers::metered_keys(expanded);
    keys.extend(
        expanded
            .devices
            .iter()
            .filter(|d| d.plugin.is_some())
            .map(|d| d.key.clone()),
    );
    keys
}

fn entry_status(status: &PluginStatus) -> PluginEntryStatus {
    match status {
        PluginStatus::Ok => PluginEntryStatus::Ok,
        PluginStatus::Unsupported(r) => PluginEntryStatus::Unsupported(r.clone()),
        PluginStatus::Disabled => PluginEntryStatus::Disabled,
        PluginStatus::Failed(r) => PluginEntryStatus::Failed(r.clone()),
    }
}

/// What the browser and the menus show of the catalog: a usable plugin the user disabled or that
/// is quarantined is listed as such (so they hide it), and the star and tags come along. The
/// catalog itself is not touched: a project that uses the plugin still loads it.
fn entry_infos(catalog: &PluginCatalog, user: &PluginsUser) -> Vec<PluginEntryInfo> {
    catalog
        .entries()
        .iter()
        .map(|e| {
            let id = e.plugin.type_id();
            let status = match entry_status(&e.status) {
                PluginEntryStatus::Ok if user.is_quarantined(&id) => {
                    PluginEntryStatus::Quarantined(
                        user.quarantine_reason(&id).unwrap_or_default().to_owned(),
                    )
                }
                PluginEntryStatus::Ok if user.is_disabled(&id) => PluginEntryStatus::Disabled,
                status => status,
            };
            PluginEntryInfo {
                plugin: e.plugin.clone(),
                name: e.name.clone(),
                vendor: e.vendor.clone(),
                version: e.version.clone(),
                kind: e.kind,
                category: e.category,
                status,
                path: Some(e.path.to_string_lossy().into_owned()),
                layout: e.layout.clone(),
                marks: PluginMarks {
                    favourite: user.is_favourite(&id),
                    tags: user.tags_of(&id).to_vec(),
                },
            }
        })
        .collect()
}

/// Unix seconds, the sequence of recovery snapshot names (never used by the engine).
#[allow(
    clippy::disallowed_methods,
    reason = "the wall clock only orders recovery file names; it never reaches audio"
)]
fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// True when the snapshot at `snapshot` is newer than the project file (or there is no project
/// file, or a time cannot be read): only then is it worth offering.
fn newer_than_project(snapshot: &Path, project: Option<&Path>) -> bool {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (modified(snapshot), project.and_then(modified)) {
        (Some(s), Some(p)) => s > p,
        _ => true,
    }
}

fn rescan_mode(mode: PluginRescan) -> RescanMode {
    match mode {
        PluginRescan::Incremental => RescanMode::Incremental,
        PluginRescan::Full => RescanMode::Full,
        PluginRescan::FailedOnly => RescanMode::FailedOnly,
    }
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ configuration

    /// Sets up the plugin machinery: the user's settings and scan cache, where the journal and
    /// recovery snapshots go, and what scans files. The catalog is registered from the cache
    /// (no plugin file is opened); a document already open with plugins is rebuilt.
    pub fn configure_plugins(&mut self, config: PluginConfig) {
        // The host is built once, before any instance exists: a setting changed later applies at
        // the next start. The old host's plugin types leave the registry first.
        if let Some(isolation) = config.isolation
            && self.plugins.host.manager_ref().instance_count() == 0
        {
            self.plugins.freeze_isolation = match &isolation {
                PluginIsolation::Sandbox { command, .. } => Some(PluginIsolation::Sandbox {
                    command: command.clone(),
                    bounce: true,
                }),
                PluginIsolation::InProcess => None,
            };
            self.plugins.host.set_catalog(
                &PluginCache::default(),
                &PluginSettings::default(),
                &mut self.registry,
            );
            self.plugins.host = PluginHost::with_isolation(isolation);
        }
        let p = &mut self.plugins;
        p.settings = config.settings;
        p.cache = config.cache;
        p.journal = config.paths.journal.as_ref().map(CrashJournal::new);
        let mut unclean_exit = false;
        p.recovery = config.paths.recovery.as_ref().map(|dir| {
            let (marker, unclean) = SessionMarker::start(dir);
            unclean_exit = unclean;
            Recovery {
                dir: RecoveryDir::new(dir),
                last: None,
                wanted: false,
                sequence: 0,
                writer: RecoveryWriter::new(),
                revision: None,
                marker,
            }
        });
        p.paths = config.paths;
        p.runner = config.runner;
        p.previous_crash = config.previous_crash;
        self.apply_process_grouping();
        self.refresh_plugin_catalog();
        if let Some(crashed) = self.plugins.previous_crash.clone() {
            let has_recovery = self
                .plugins
                .recovery
                .as_ref()
                .is_some_and(|r| r.dir.newest().is_some());
            let plugins = crashed
                .iter()
                .map(|c| {
                    self.plugins
                        .host
                        .catalog()
                        .entry(&c.plugin)
                        .map_or_else(|| c.plugin.id.clone(), |e| e.name.clone())
                })
                .collect();
            self.plugins.show_banner(BannerKind::UncleanExit {
                plugins,
                has_recovery,
            });
        } else if unclean_exit
            && self.plugins.recovery.as_ref().is_some_and(|r| {
                r.dir
                    .newest()
                    .is_some_and(|snapshot| newer_than_project(&snapshot, self.file.as_deref()))
            })
        {
            // No plugin was loaded, but the last run still did not end cleanly and left a copy.
            self.plugins.show_banner(BannerKind::UncleanExit {
                plugins: Vec::new(),
                has_recovery: true,
            });
        }
        if !self.doc().plugin_devices().is_empty() {
            self.rebuild_engine();
        }
        self.recover_orphan_recordings();
    }

    /// The user's plugin settings.
    pub fn plugin_settings(&self) -> &PluginSettings {
        &self.plugins.settings
    }

    /// The installed plugins as the scan cache and the settings say.
    pub fn plugin_catalog(&self) -> &PluginCatalog {
        self.plugins.host.catalog()
    }

    /// The plugin host (manager and catalog), for tests and the platform layer.
    pub fn plugin_host(&mut self) -> &mut PluginHost {
        &mut self.plugins.host
    }

    /// The scan cache.
    pub fn plugin_cache(&self) -> &PluginCache {
        &self.plugins.cache
    }

    /// The settings after a change the user made, once: the platform layer saves them.
    pub fn take_plugin_settings_change(&mut self) -> Option<PluginSettings> {
        std::mem::take(&mut self.plugins.settings_changed).then(|| self.plugins.settings.clone())
    }

    /// True once after the user asked to add a search folder: show the folder dialog and call
    /// [`Self::add_plugin_folder`].
    pub fn take_plugin_folder_request(&mut self) -> bool {
        std::mem::take(&mut self.plugins.folder_dialog)
    }

    /// Adds a search folder to the settings (from the folder dialog).
    pub fn add_plugin_folder(&mut self, folder: PathBuf) {
        if !self.plugins.settings.extra_paths.contains(&folder) {
            self.plugins.settings.extra_paths.push(folder);
            self.plugins.settings_changed = true;
        }
    }

    /// Rebuilds the catalog from the cache and the settings, registers it, registers the
    /// stand-ins for what the document uses and is not installed, and queues the stand-ins whose
    /// plugin is installed now (a rescan found it, or plugins were just enabled). A live device
    /// whose plugin went away (disabled, file gone) makes the engine rebuild so it shows MISSING.
    pub fn refresh_plugin_catalog(&mut self) {
        if self.register_catalog() {
            self.rebuild_engine();
        }
    }

    /// Tells the plugin host which params the document uses that a plugin's own table leaves out
    /// (the controller stand-ins some plugins publish as parameters; see
    /// [`PluginHost::set_pinned_params`]). Those params stay in the table of the plugin's
    /// instances and in the descriptor of its type, so every lane, learned control, macro target
    /// and stored value that names one keeps working, and the layout the document keeps for the
    /// device does not lose its row when the live layout replaces it. Does nothing when the
    /// answer is what the host has; otherwise the descriptors are registered again, without
    /// touching any device (the engine is rebuilt or replaced by the caller as it was).
    pub(super) fn plugins_pin_document_params(&mut self) {
        let pins = self.document.doc().plugin_pinned_rows();
        if &pins == self.plugins.host.pinned_params() {
            return;
        }
        self.plugins.host.set_pinned_params(pins);
        let types: Vec<String> = self
            .registry
            .plugin_type_ids()
            .map(str::to_string)
            .collect();
        for type_id in types {
            let _ = self.registry.unregister_plugin(&type_id);
        }
        let report = self.plugins.host.set_catalog(
            &self.plugins.cache,
            &self.plugins.settings,
            &mut self.registry,
        );
        for (plugin, reason) in report.errors {
            self.reject(format!("plugin {}: {reason}", plugin.id));
        }
        if let Err(e) = register_stand_ins(self.document.doc(), &mut self.registry) {
            self.reject(e.to_string());
        }
    }

    /// The work of [`Self::refresh_plugin_catalog`] without the engine rebuild: true when a live
    /// device needs one (its plugin went away or its descriptor changed).
    fn register_catalog(&mut self) -> bool {
        // A project saved after a plugin grew new ports teaches the scan about them first, so
        // the catalog and the registry agree with the devices the project holds.
        self.adopt_document_ports();
        // The params the project uses that a plugin's own table leaves out stay listed.
        self.plugins
            .host
            .set_pinned_params(self.document.doc().plugin_pinned_rows());
        // The stand-ins of the last round go too: an installed plugin must not collide with its
        // own stand-in type (the ones still needed are registered again below).
        let types: Vec<String> = self
            .registry
            .plugin_type_ids()
            .map(str::to_string)
            .collect();
        for type_id in types {
            let _ = self.registry.unregister_plugin(&type_id);
        }
        let report = self.plugins.host.set_catalog(
            &self.plugins.cache,
            &self.plugins.settings,
            &mut self.registry,
        );
        for (plugin, reason) in report.errors {
            self.reject(format!("plugin {}: {reason}", plugin.id));
        }
        if let Err(e) = register_stand_ins(self.document.doc(), &mut self.registry) {
            self.reject(e.to_string());
        }
        self.plugins.catalog_dirty = true;
        let mut rebuild = false;
        let devices: Vec<(DeviceKey, PluginRef)> = self
            .doc()
            .plugin_devices()
            .into_iter()
            .filter_map(|(k, d)| Some((k, d.plugin.as_ref()?.plugin_ref())))
            .collect();
        for (key, plugin) in devices {
            let installed = self.plugin_usable(&plugin);
            let live = self.plugins.instances.contains_key(&key);
            if live && !installed {
                rebuild = true;
            } else if live {
                // Same plugin, maybe a new descriptor (the plugin changed since the last scan).
                let now = self.registry.get(&plugin.type_id()).map(|f| f.descriptor);
                let then = self.plugins.live_desc.get(&key).copied();
                let alike = match (now, then) {
                    (Some(a), Some(b)) => same_shape(a, b),
                    (None, None) => true,
                    _ => false,
                };
                if !alike {
                    rebuild = true;
                }
            } else if installed
                && !self.plugins.pending.contains(&key)
                && !self.plugins.skipped.contains(&key)
                && !self.quarantine_blocks(&plugin, &key)
            {
                self.plugins.failed.remove(&key);
                self.plugins.pending.push_back(key);
                self.plugins.batch_total += 1;
            }
        }
        rebuild
    }

    /// True when `plugin` can be used now: plugins are on and the catalog lists it OK.
    fn plugin_usable(&self, plugin: &PluginRef) -> bool {
        self.plugins.settings.enabled && self.plugins.host.is_installed(plugin)
    }

    /// True when the quarantine keeps the instance of `key` from being created: the plugin is
    /// quarantined and the user has not chosen LOAD ANYWAY for this device. The device stays a
    /// stand-in that says QUARANTINED, and the rest of the project opens as usual.
    fn quarantine_blocks(&self, plugin: &PluginRef, key: &DeviceKey) -> bool {
        self.plugins.user.is_quarantined(&plugin.type_id())
            && !self.plugins.load_anyway.contains(key)
    }

    /// Tells the sandbox how instances created from now on share child processes: the setting,
    /// and what single plugins ask for in the plugin manager.
    pub(crate) fn apply_process_grouping(&self) {
        use atomik_plugin_host::ProcessMode;
        let mode = if self.plugins.settings.sandbox {
            self.plugins.settings.process
        } else {
            ProcessMode::PerPlugin
        };
        self.plugins
            .host
            .set_process_grouping(mode, &self.plugins.user.process_choices());
    }

    // ------------------------------------------------------------------ insert path

    /// What `insert_flat` inserts for a plugin device: the live device of a parked instance (an
    /// engine restart), else the stand-in; in that case the instance is queued when it can be
    /// created. `None` when the registry does not know the type.
    pub(super) fn plugin_device_for_insert(
        &mut self,
        device: &ExpandedDevice,
        pd: &PluginDoc,
    ) -> Option<(Box<dyn Device>, &'static DeviceDescriptor)> {
        let plugin = pd.plugin_ref();
        let type_id = plugin.type_id();
        let usable = self.plugin_usable(&plugin);
        let key = &device.key;
        if let Some(&id) = self.plugins.instances.get(key) {
            let sample_rate = f64::from(self.document.doc().sample_rate);
            // The parked instance must still be the plugin the device names (a relink changed it).
            // ... and must still have the layout the registry holds: a plugin that changed its
            // buses in another instance is created again (its state is in the document) instead
            // of being given a device that no longer matches the registered type.
            let current = self.registry.get(&type_id).map(|f| f.descriptor);
            let fits = self
                .plugins
                .host
                .manager_ref()
                .layout(id)
                .and_then(|l| atomik_device_sdk::descriptor_for(&plugin, l).ok())
                .is_some_and(|d| current.is_some_and(|c| same_shape(c, d)));
            let fits = fits || self.keep_despite_mismatch(&plugin, key, id, current);
            let same = fits
                && self
                    .plugins
                    .loaded
                    .get(&id)
                    .is_some_and(|i| i.plugin == plugin);
            if usable && same && !self.sink.has_engine() {
                // No engine to take a device: the instance stays parked behind a stand-in.
                self.plugins.offline.insert(key.clone());
                let factory = self.registry.get(&type_id)?;
                let descriptor = factory.descriptor;
                let standin = self.registry.create(&type_id)?;
                return Some((standin, descriptor));
            }
            if usable && same {
                match self.plugins.host.manager().begin_reattach(id, sample_rate) {
                    Ok(Some(handle)) => {
                        self.plugins.offline.remove(key);
                        self.adopt_descriptor(&type_id, handle.descriptor);
                        self.plugins
                            .live_desc
                            .insert(key.clone(), handle.descriptor);
                        return Some((Box::new(handle.device), handle.descriptor));
                    }
                    Ok(None) => {
                        // The plugin is still deactivating or activating (a busy one: the
                        // restart does not wait for it): the stand-in plays until it is active
                        // again, and `adopt_offline` swaps its device in.
                        self.plugins.offline.insert(key.clone());
                        let factory = self.registry.get(&type_id)?;
                        let descriptor = factory.descriptor;
                        let standin = self.registry.create(&type_id)?;
                        return Some((standin, descriptor));
                    }
                    Err(e) => {
                        self.plugins.offline.remove(key);
                        self.plugins.failed.insert(key.clone(), e.to_string());
                    }
                }
            }
            // The plugin is gone, or could not be reattached: let the instance go.
            let _ = self.plugins.host.manager().remove(id);
            self.release_instance_maps(key);
        }
        let factory = self.registry.get(&type_id)?;
        let descriptor = factory.descriptor;
        let standin = self.registry.create(&type_id)?;
        self.plugins.live_desc.remove(key);
        if usable
            && !self.plugins.failed.contains_key(key)
            && !self.plugins.pending.contains(key)
            && !self.plugins.skipped.contains(key)
            && !self.quarantine_blocks(&plugin, key)
        {
            self.plugins.pending.push_back(key.clone());
            self.plugins.batch_total += 1;
        }
        Some((standin, descriptor))
    }

    /// Loop guard: the instance `id` of `key` has other ports than the registry holds, so it would
    /// be created again. The first time per plugin, device and port shape that is allowed (false);
    /// a second one within [`RECREATE_WINDOW_SECS`] is refused with a warning (true: keep it).
    fn keep_despite_mismatch(
        &mut self,
        plugin: &PluginRef,
        key: &DeviceKey,
        id: InstanceId,
        current: Option<&'static DeviceDescriptor>,
    ) -> bool {
        let Some(current) = current else {
            return false;
        };
        if self.plugins.host.manager_ref().layout(id).is_none() {
            return false;
        }
        let shape = current
            .sockets
            .iter()
            .map(|s| s.key)
            .collect::<Vec<_>>()
            .join(",");
        let entry = (key.clone(), plugin.type_id(), shape);
        let now = unix_seconds();
        if let Some(at) = self.plugins.recreated.get(&entry)
            && now.saturating_sub(*at) < RECREATE_WINDOW_SECS
        {
            log::warn!(
                "[{key}] {} was already created again for the ports it has now: kept as it is",
                entry.1
            );
            return true;
        }
        self.plugins.recreated.insert(entry, now);
        false
    }

    /// Makes `descriptor` the registry's descriptor of `type_id` when the live plugin's layout
    /// gave another one than the scan did (the plugin changed since the last scan).
    fn adopt_descriptor(&mut self, type_id: &str, descriptor: &'static DeviceDescriptor) {
        let same = self
            .registry
            .get(type_id)
            .is_some_and(|f| std::ptr::eq(f.descriptor, descriptor));
        if !same {
            let _ = self.registry.unregister_plugin(type_id);
            if let Err(e) = self
                .registry
                .register_plugin(type_id.to_string(), descriptor)
            {
                self.reject(format!("plugin {type_id}: {e}"));
            }
        }
    }

    fn release_instance_maps(&mut self, key: &DeviceKey) {
        if let Some(id) = self.plugins.instances.remove(key) {
            self.plugins.keys.remove(&id);
            self.plugins.dirty.remove(&id);
            self.plugins.capturing.remove(&id);
        }
        self.plugins.preset_loads.remove(key);
        self.plugins.knob_reads.remove(key);
        self.plugins.live_desc.remove(key);
        self.plugins.crashed.remove(key);
        self.plugins.too_large.remove(key);
    }

    /// Called by `rebuild_engine` before the engine is reset: captures the states and keeps (or,
    /// for a new document, drops) the living instances, so an audio restart does not reload them.
    pub(super) fn plugins_before_reset(&mut self) {
        // A plugin that does not answer keeps the state stored for it (logged); the restart goes
        // on.
        self.plugins_capture_all();
        self.plugins.not_handed.clear();
        self.plugins.too_large_hit.clear();
        let keep = !self.plugins.discard;
        let ids: Vec<InstanceId> = self.plugins.instances.values().copied().collect();
        for id in ids {
            self.plugins.host.manager().retain(id, keep);
        }
        self.plugins.pending.clear();
        self.cancel_starting(None);
        self.plugins.batch_total = 0;
        self.plugins.batch_done = 0;
        self.plugins.shown.clear();
        if !keep {
            self.plugins.instances.clear();
            self.plugins.keys.clear();
            self.plugins.dirty.clear();
            self.plugins.preset_loads.clear();
            self.plugins.knob_reads.clear();
            self.plugins.capturing.clear();
            self.plugins.snapshot_wait = None;
            self.plugins.failed.clear();
            self.plugins.crashed.clear();
            self.plugins.load_anyway.clear();
            self.plugins.skipped.clear();
            self.plugins.no_capture.clear();
            self.plugins.live_desc.clear();
        }
        // Editors stay open across an audio restart (the instance lives on); a new document
        // closes them before its instances are destroyed.
        if !keep {
            self.close_all_plugin_windows();
        }
    }

    /// Called by `rebuild_engine` right after the engine was reset: the old devices are gone, so
    /// the manager parks the retained instances and destroys the others.
    pub(super) fn plugins_after_reset(&mut self) {
        self.plugins_poll_events();
        self.plugins.discard = false;
    }

    /// Called by `rebuild_engine` after every device was inserted: instances nobody reattached
    /// (their device left the document) are destroyed.
    pub(super) fn plugins_after_rebuild(&mut self) {
        // A frozen plugin is not built, so its instance (if one was left) goes.
        let frozen = atomik_model::frozen_devices(self.doc());
        let wanted: BTreeSet<DeviceKey> = self
            .doc()
            .plugin_devices()
            .into_iter()
            .map(|(k, _)| k)
            .filter(|k| !frozen.contains(k))
            .collect();
        let stale: Vec<DeviceKey> = self
            .plugins
            .instances
            .keys()
            .filter(|k| !wanted.contains(*k))
            .cloned()
            .collect();
        for key in stale {
            if let Some(id) = self.plugins.instances.get(&key).copied() {
                let _ = self.plugins.host.manager().remove(id);
            }
            self.release_instance_maps(&key);
        }
        self.plugins_poll_events();
    }

    /// Marks that the next engine restart is for a new document: instances are dropped.
    pub(super) fn plugins_discard_on_next_reset(&mut self) {
        self.plugins.discard = true;
    }

    /// A removed device's instance leaves with its engine device (the engine retires the device
    /// and the manager reaps it on a later poll).
    pub(super) fn plugins_device_removed(&mut self, flat: &DeviceKey) {
        self.close_plugin_window(flat);
        let parked = if self.plugins.offline.remove(flat) {
            self.plugins.instances.get(flat).copied()
        } else {
            None
        };
        if self.plugins.instances.contains_key(flat) {
            self.release_instance_maps(flat);
        }
        self.plugins.pending.retain(|k| k != flat);
        self.cancel_starting(Some(flat));
        if let Some(id) = parked {
            // A parked instance has no engine device to leave with: destroy it.
            self.plugins_poll_events();
            let _ = self.plugins.host.manager().remove(id);
            self.plugins_poll_events();
        }
        self.plugins.failed.remove(flat);
        self.plugins.crashed.remove(flat);
        self.plugins.auto_reloads.remove(flat);
        self.plugins.load_anyway.remove(flat);
        self.plugins.skipped.remove(flat);
        self.plugins.no_capture.remove(flat);
        self.plugins.shown.remove(flat);
        self.view
            .set_plugin_status(flat, PluginDeviceStatus::default());
    }

    // ------------------------------------------------------------------ per frame

    /// Per-frame plugin work, called from [`AppCore::frame`]: the manager's main-thread poll,
    /// one pending instance, the scan job, the parameter texts and the view feeds.
    pub(super) fn plugins_frame(&mut self) {
        self.plugins_poll_events();
        self.apply_soft_answers();
        self.plugins_poll_captures();
        self.plugins_poll_preset_loads();
        self.instantiate_one();
        self.adopt_offline();
        self.poll_scan();
        self.save_learned_cache();
        self.answer_param_texts();
        self.feed_plugin_views();
        if self.plugin_params_open
            && let Some(device) = self.plugin_params.device().cloned()
        {
            let mode = self.plugin_learn_mode(&device);
            self.plugin_params.set_learn_mode(mode);
        }
    }

    /// Runs the clock-driven plugin work: the quiet-period state capture and the
    /// recovery snapshot. `now` is a monotonic time in seconds (the shell passes the
    /// frame clock). Nothing here waits for a plugin: states are asked for in the background and
    /// stored as they arrive.
    pub fn plugin_tick(&mut self, now: f64) {
        self.plugins.clock = now;
        self.plugins_poll_captures();
        let due: Vec<InstanceId> = self
            .plugins
            .dirty
            .iter()
            .filter(|(_, at)| now - **at >= STATE_QUIET_SECS)
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            let key = self.plugins.keys.get(&id).cloned();
            // A plugin that is loading a preset is not asked for its state: the load asks for
            // it when it is done, and that is the step.
            if key
                .as_ref()
                .is_some_and(|k| self.plugins.preset_loads.contains_key(k))
            {
                continue;
            }
            self.plugins.dirty.remove(&id);
            if let Some(key) = key {
                self.request_capture_for(&key, true);
            }
        }
        // A plugin in this process has answered already.
        self.plugins_poll_captures();
        let loaded = !self.plugins.instances.is_empty();
        let revision = self.document.revision();
        // A loaded plugin snapshots on its period whatever happened (its own state moves
        // without a command); otherwise only a project that differs from its saved state and
        // from the last snapshot is worth a write.
        let changed = loaded
            || (self
                .plugins
                .recovery
                .as_ref()
                .is_some_and(|r| r.revision != Some(revision))
                && self.is_dirty());
        let autosave = self.plugins.autosave_secs;
        let (wanted, periodic) = match &mut self.plugins.recovery {
            Some(r) => {
                let first = r.last.is_none();
                if first || (!changed && !loaded) {
                    // The period counts from the first change after a snapshot, not from
                    // the last time the project was looked at.
                    r.last = Some(now);
                }
                let last = r.last.unwrap_or(now);
                // A loaded plugin has state the user cannot recreate, so it snapshots sooner.
                let period = if loaded {
                    RECOVERY_PERIOD_SECS.min(autosave.max(1.0))
                } else {
                    autosave
                };
                let due = (first && loaded) || now - last >= period;
                (r.wanted, autosave > 0.0 && changed && due)
            }
            None => (false, false),
        };
        if self.plugins.snapshot_wait.is_some() {
            self.finish_snapshot_wait(now);
        } else if wanted {
            // A capture just changed a stored state (or the app asked for a snapshot): write
            // what is stored, without asking the plugins again.
            self.write_recovery_snapshot(now);
        } else if periodic {
            self.begin_periodic_snapshot(now);
        }
    }

    /// Instances waiting to be created and instances created in this batch, for the status line
    /// "Loading plugins 3/8"; `None` when nothing is loading. Without an engine (audio off,
    /// `--no-audio`) nothing loads, since a device handed to no engine is dropped at once: the
    /// instances wait for the engine, and [`Self::plugin_loading_text`] says so instead of a
    /// progress that never moves.
    pub fn plugin_loading(&self) -> Option<(usize, usize)> {
        let pending = self.plugins.pending.len();
        (pending > 0).then(|| {
            let total = self
                .plugins
                .batch_total
                .max(self.plugins.batch_done + pending);
            (self.plugins.batch_done, total)
        })
    }

    /// The status line text of a running load, if any: "Loading plugins 3/8".
    pub fn plugin_loading_text(&self) -> Option<String> {
        self.plugin_loading()
            .map(|(done, total)| format!("Loading plugins {}/{total}", done + 1))
    }

    /// A platform loop that paints between frames asks for LOADING to show one frame before an
    /// instance is created (the creation blocks the UI thread, plugins need the main thread).
    pub fn set_plugin_load_paints_first(&mut self, on: bool) {
        self.plugins.paint_first = on;
    }

    /// True when no instance is waiting to be created or to be given its device again after an
    /// engine restart.
    pub fn plugins_ready(&self) -> bool {
        self.plugins.pending.is_empty() && !self.reattaching()
    }

    /// An engine restart left instances behind stand-ins until their plugins are active again.
    fn reattaching(&self) -> bool {
        self.sink.has_engine() && !self.plugins.offline.is_empty()
    }

    /// Runs frames until every pending instance is created (tests and scripts), for at most
    /// [`WAIT_PLUGINS_LIMIT`]: instances still pending after that are logged once and left to
    /// the normal frames.
    pub fn wait_plugins(&mut self) {
        #[allow(
            clippy::disallowed_methods,
            reason = "a wall-clock bound for tests and scripts; it never reaches the engine"
        )]
        let started = std::time::Instant::now();
        while !self.plugins_ready() {
            if started.elapsed() >= WAIT_PLUGINS_LIMIT {
                let left = self.plugins.pending.len() + self.plugins.offline.len();
                log::warn!(
                    "{left} plugin(s) were not created within {} s; loading goes on in the background",
                    WAIT_PLUGINS_LIMIT.as_secs()
                );
                self.plugins
                    .log(format!("{left} plugin(s) still loading after waiting"));
                break;
            }
            self.frame();
        }
        // One more poll lets restarts and the reaper settle; the views show the result.
        self.plugins_poll_events();
        self.feed_plugin_views();
    }

    fn plugins_poll_events(&mut self) {
        let events = self.plugins.host.poll();
        // Before the events: a destroyed instance still has its key until its Destroyed event.
        self.plugin_stalls();
        for event in events {
            self.plugin_event(event);
        }
    }

    /// Writes the slow plugin calls the manager collected: a warning in the log file, a line in
    /// the plugin log and the counters (see [`crate::ui_stall`]).
    fn plugin_stalls(&mut self) {
        for s in self.plugins.host.manager().take_stalls() {
            let name = self
                .plugins
                .keys
                .get(&s.instance)
                .map_or_else(|| "plugin".to_string(), |k| k.0.clone());
            let line = crate::ui_stall::plugin_stall_line(&s.call, s.ms, &name, s.outcome);
            crate::ui_stall::count_plugin_stall(s.ms);
            log::warn!("{line}");
            self.plugins.log(line);
        }
    }

    pub(super) fn plugin_event(&mut self, event: ManagerEvent) {
        match event {
            ManagerEvent::ParamEdit {
                instance,
                param,
                value,
                phase,
            } => self.plugin_param_edit(instance, param, value, phase),
            ManagerEvent::StateDirty(id) => {
                let now = self.plugins.clock;
                self.plugins.dirty.insert(id, now);
            }
            ManagerEvent::Restarted(_) | ManagerEvent::LatencyChanged { .. } => {}
            ManagerEvent::RestartFailed { instance, reason } => {
                self.plugins.bad.insert(instance);
                if let Some(key) = self.plugins.keys.get(&instance).cloned() {
                    log::warn!("{}", failed_log_line(&key.0, &reason));
                    self.plugins.failed.insert(key, reason);
                }
            }
            ManagerEvent::Failed { instance, reason } => {
                // The device's own Failed that goes with a lost process says nothing more than
                // the Crashed event does.
                if self.plugins.host.manager_ref().is_lost(instance) {
                    return;
                }
                self.plugins.bad.insert(instance);
                if let Some(key) = self.plugins.keys.get(&instance).cloned() {
                    log::warn!("{}", failed_log_line(&key.0, &reason));
                    self.plugins.log(format!("[{key}] failed: {reason}"));
                    self.plugins.failed.insert(key, reason);
                }
            }
            ManagerEvent::Crashed { instance, reason } => {
                let name = self
                    .plugins
                    .keys
                    .get(&instance)
                    .map_or_else(|| "plugin".to_string(), |k| k.0.clone());
                log::warn!("{}", crash_log_line(&name, &reason));
                if let Some(key) = self.plugins.keys.get(&instance).cloned() {
                    self.plugins.log(format!("[{key}] crashed: {reason}"));
                    if self.plugins.windows.get(&key) == Some(&EditorWindow::ExternalOpening) {
                        self.plugins.windows.remove(&key);
                    }
                    self.plugin_dock_stopped(&key);
                    self.plugins.failed.remove(&key);
                    if let Some(type_id) = self
                        .doc()
                        .resolve(&key)
                        .and_then(|d| d.plugin.as_ref())
                        .map(|p| p.type_id())
                    {
                        // Kept in the settings: CRASHED in the plugin list until RETRY.
                        self.plugins.settings.mark_crashed(&type_id, &reason);
                        self.plugins.settings_changed = true;
                        self.plugins.bad.insert(instance);
                        // Three real crashes in a row quarantine it (see the plugin manager).
                        // A process the hang guard stopped does not count: it never died by
                        // itself, and a slow machine must not cost the user the plugin.
                        if !reason.starts_with(HUNG) {
                            self.plugin_failed_in_a_row(&type_id, "Crashed", &reason);
                        }
                    }
                    let hung = reason.starts_with(HUNG);
                    self.plugins.crashed.insert(key.clone(), reason);
                    if hung {
                        self.reload_hung_plugin(&key);
                    }
                }
            }
            ManagerEvent::NotResponding { instance, call } => {
                // Alive, so not CRASHED: it is not in the crashed settings and does not count
                // toward the quarantine, and the state stored for it stays as it is.
                let name = self
                    .plugins
                    .keys
                    .get(&instance)
                    .map_or_else(|| "plugin".to_string(), |k| k.0.clone());
                log::warn!("{}", not_responding_log_line(&name, &call));
                if let Some(key) = self.plugins.keys.get(&instance).cloned() {
                    self.plugins.log(not_responding_log_line(&key.0, &call));
                }
            }
            ManagerEvent::Responding(instance) => {
                let name = self
                    .plugins
                    .keys
                    .get(&instance)
                    .map_or_else(|| "plugin".to_string(), |k| k.0.clone());
                log::info!("{}", responding_log_line(&name));
                if let Some(key) = self.plugins.keys.get(&instance).cloned() {
                    self.plugins.log(responding_log_line(&key.0));
                }
            }
            ManagerEvent::LayoutChanged { instance, layout } => {
                self.plugin_layout_changed(instance, layout);
            }
            ManagerEvent::GuiResize { instance, w, h } => {
                if let Some(key) = self.plugins.keys.get(&instance).cloned()
                    && self.plugin_editor_docked(&key)
                {
                    self.plugin_dock_plugin_resized(&key, w, h);
                } else if let Some(key) = self.plugins.keys.get(&instance).cloned()
                    && self.plugins.windows.contains_key(&key)
                {
                    self.plugins
                        .window_requests
                        .push(PluginWindowRequest::Resize { device: key, w, h });
                }
            }
            ManagerEvent::GuiFrame {
                instance,
                x,
                y,
                w,
                h,
            } => {
                if let Some(key) = self.plugins.keys.get(&instance).cloned()
                    && self.plugins.windows.contains_key(&key)
                {
                    self.plugins.frames.insert(key, (x, y, w, h));
                }
            }
            ManagerEvent::EditorOpened(instance) => {
                if let Some(key) = self.plugins.keys.get(&instance).cloned() {
                    match self.plugins.windows.get(&key) {
                        Some(EditorWindow::ExternalOpening) => {
                            self.plugins
                                .windows
                                .insert(key.clone(), EditorWindow::External);
                            self.plugin_dock_opened(&key);
                            self.plugins.log(format!("[{key}] editor opened"));
                            log::info!("{}", editor_opened_line(&key.0, instance));
                        }
                        None => {
                            // The editor was closed while the plugin still opened it: the
                            // editor that just appeared is closed again, not left open unseen.
                            log::info!(
                                "plugin [{key}] editor opened after it was closed: instance {}, closing it",
                                instance.0
                            );
                            let _ = self.plugins.host.manager().close_editor(instance);
                        }
                        Some(_) => {}
                    }
                }
            }
            ManagerEvent::EditorFailed { instance, reason } => {
                if let Some(key) = self.plugins.keys.get(&instance).cloned()
                    && self.plugins.windows.get(&key) == Some(&EditorWindow::ExternalOpening)
                {
                    self.plugins.windows.remove(&key);
                    self.plugin_dock_closed(&key);
                    self.plugins
                        .log(format!("[{key}] editor did not open: {reason}"));
                    log::info!("{}", editor_failed_line(&key.0, instance, &reason));
                    self.reject(format!("cannot open the plugin editor: {reason}"));
                }
            }
            ManagerEvent::GuiClosed(instance) => {
                if let Some(key) = self.plugins.keys.get(&instance).cloned() {
                    // An external editor has no host window to destroy: the close path knows.
                    self.close_plugin_gui(&key);
                }
            }
            ManagerEvent::GuiRequestShow(instance) => {
                if let Some(key) = self.plugins.keys.get(&instance).cloned()
                    && self.plugins.windows.contains_key(&key)
                {
                    self.plugins
                        .window_requests
                        .push(PluginWindowRequest::Show { device: key });
                }
            }
            ManagerEvent::GuiRequestHide(instance) => {
                if let Some(key) = self.plugins.keys.get(&instance).cloned()
                    && self.plugins.windows.contains_key(&key)
                {
                    self.plugins
                        .window_requests
                        .push(PluginWindowRequest::Hide { device: key });
                }
            }
            ManagerEvent::Log {
                instance,
                level,
                text,
            } => {
                let name = self
                    .plugins
                    .keys
                    .get(&instance)
                    .map_or_else(|| "plugin".to_string(), |k| k.0.clone());
                if matches!(level, LogLevel::Warning | LogLevel::Error) {
                    log::warn!("{}", plugin_said_line(&name, &text));
                }
                self.plugins.log(format!("[{name}] {level:?}: {text}"));
            }
            ManagerEvent::Destroyed(id) => {
                let ended_well = !self.plugins.bad.remove(&id);
                if let Some(info) = self.plugins.loaded.remove(&id) {
                    if ended_well {
                        // An instance that lived and ended without a fault is a success.
                        self.plugin_worked(&info.plugin.type_id());
                    }
                    if !self.plugins.host.is_sandboxed()
                        && let Some(journal) = &mut self.plugins.journal
                    {
                        let _ = journal.unloaded(&info);
                    }
                }
                if let Some(key) = self.plugins.keys.remove(&id) {
                    // Destroyed while the document still lists it (the engine dropped the
                    // device, for example with audio off): the device is a stand-in again.
                    if self.plugins.instances.get(&key) == Some(&id) {
                        self.plugins.instances.remove(&key);
                        self.plugins.live_desc.remove(&key);
                    }
                }
                self.plugins.dirty.remove(&id);
            }
        }
    }

    // ------------------------------------------------------------------ creating instances

    /// Moves the pending creations on: starts the plugin process of every pending device (up to
    /// [`MAX_STARTING`] at a time, so they all load at the same time), moves every started
    /// creation on, then attaches the oldest one that is ready (one per frame: an attach does
    /// real work). A plugin that takes minutes to load does not hold the ones queued after it:
    /// they attach as soon as they are ready, and what the project sounds like and how its
    /// devices are keyed do not depend on the order.
    fn instantiate_one(&mut self) {
        if self.plugins.paint_first
            && let Some(front) = self.plugins.pending.front()
            && !self.plugins.shown.get(front).is_some_and(|s| s.loading)
        {
            // LOADING is fed (and painted) before the creation blocks the thread.
            return;
        }
        self.start_pending();
        let Some(key) = self.next_ready_creation() else {
            return;
        };
        let Some(ticket) = self.plugins.starting.get(&key).map(|s| s.ticket) else {
            return;
        };
        let Some(answer) = self.plugins.host.manager().poll_create(ticket) else {
            // The ticket is spent without an answer: start the device over.
            self.plugins.starting.remove(&key);
            return;
        };
        let Some(Starting {
            plugin,
            began,
            spawn,
            ..
        }) = self.plugins.starting.remove(&key)
        else {
            return;
        };
        self.plugins.pending.retain(|k| k != &key);
        self.finish_batch_step();
        match answer {
            Ok(handle) => {
                let polled = began.elapsed();
                let (restore, activate) = (handle.timing.restore, handle.timing.activate);
                #[allow(
                    clippy::disallowed_methods,
                    reason = "UI-thread load time for the log, not engine behaviour"
                )]
                let attaching = std::time::Instant::now();
                self.attach_instance(&key, &plugin, handle);
                let stages = CreatedStages {
                    spawn,
                    instantiate: polled
                        .saturating_sub(spawn)
                        .saturating_sub(restore)
                        .saturating_sub(activate),
                    restore,
                    activate,
                    attach: attaching.elapsed(),
                };
                let took = began.elapsed();
                log::info!("{}", created_line(&key.0, took, &stages));
                self.plugins
                    .log(format!("[{key}] {}", created_text(took, &stages)));
            }
            Err(e) => {
                let message = e.to_string();
                self.plugins.log(format!("[{key}] cannot load: {message}"));
                self.plugins.failed.insert(key, message.clone());
                // A host process that could not be started is the app's or the machine's
                // fault, not the plugin's: the device says FAILED (RELOAD retries) but the
                // quarantine does not count it.
                if !e.is_spawn_failure() {
                    let type_id = plugin.type_id();
                    self.plugins.load_failed.insert(type_id.clone());
                    self.plugin_failed_in_a_row(&type_id, "Could not load", &message);
                }
            }
        }
    }

    /// Moves every started creation on without installing any, and says which is the oldest one
    /// (in queue order) that is ready to attach.
    fn next_ready_creation(&mut self) -> Option<DeviceKey> {
        let keys: Vec<DeviceKey> = self.plugins.pending.iter().cloned().collect();
        let mut first = None;
        for key in keys {
            let Some(ticket) = self.plugins.starting.get(&key).map(|s| s.ticket) else {
                continue;
            };
            if self.plugins.host.manager().creation_ready(ticket) != Some(false) && first.is_none()
            {
                first = Some(key);
            }
        }
        first
    }

    /// One creation of the batch is over.
    fn finish_batch_step(&mut self) {
        self.plugins.batch_done += 1;
        if self.plugins.pending.is_empty() {
            self.plugins.batch_total = 0;
            self.plugins.batch_done = 0;
        }
    }

    /// Starts the creation of the pending devices that have not started yet, in queue order
    /// (see [`MAX_STARTING`]). A device that no longer needs an instance leaves the queue.
    fn start_pending(&mut self) {
        // A device that came into the document since (a paste, a patch) may use params the host
        // does not know to keep yet.
        self.plugins_pin_document_params();
        let keys: Vec<DeviceKey> = self.plugins.pending.iter().cloned().collect();
        for key in keys {
            if self.plugins.starting.contains_key(&key) {
                continue;
            }
            if self.plugins.starting.len() >= MAX_STARTING {
                break;
            }
            let usable = self
                .instantiate_request(&key)
                .filter(|(plugin, _)| self.plugin_usable(plugin));
            let Some((plugin, request)) = usable else {
                self.plugins.pending.retain(|k| k != &key);
                continue;
            };
            #[allow(
                clippy::disallowed_methods,
                reason = "UI-thread load time for the log, not engine behaviour"
            )]
            let began = std::time::Instant::now();
            match self.plugins.host.manager().begin_create(&request) {
                Ok(ticket) => {
                    let spawn = began.elapsed();
                    self.plugins.starting.insert(
                        key,
                        Starting {
                            ticket,
                            plugin,
                            began,
                            spawn,
                        },
                    );
                }
                Err(e) => {
                    let message = e.to_string();
                    self.plugins.log(format!("[{key}] cannot load: {message}"));
                    self.plugins.failed.insert(key.clone(), message);
                    self.plugins.pending.retain(|k| k != &key);
                    self.finish_batch_step();
                }
            }
        }
    }

    /// Abandons the creations that were started for `key` (all of them with `None`): the plugin
    /// processes that were loading end.
    fn cancel_starting(&mut self, key: Option<&DeviceKey>) {
        let gone: Vec<DeviceKey> = self
            .plugins
            .starting
            .keys()
            .filter(|k| key.is_none_or(|only| only == *k))
            .cloned()
            .collect();
        for k in gone {
            if let Some(s) = self.plugins.starting.remove(&k) {
                self.plugins.host.manager().cancel_create(s.ticket);
            }
        }
    }

    fn instantiate_request(&self, key: &DeviceKey) -> Option<(PluginRef, InstantiateRequest)> {
        let device = self.doc().resolve(key)?;
        let pd = device.plugin.as_ref()?;
        if self.plugins.instances.contains_key(key) {
            return None;
        }
        let plugin = pd.plugin_ref();
        Some((
            plugin.clone(),
            InstantiateRequest {
                plugin,
                state: pd.state.as_deref().cloned(),
                sample_rate: f64::from(self.doc().sample_rate),
                mode: RunMode::Realtime,
                pinned: pd.run_on_audio_thread,
                powered: pd.powered,
            },
        ))
    }

    /// The instance exists: records it, syncs the layout and the descriptor, and swaps its device
    /// into the engine in place of the stand-in.
    fn attach_instance(&mut self, key: &DeviceKey, plugin: &PluginRef, handle: PluginHandle) {
        let type_id = plugin.type_id();
        if self.plugins.load_failed.remove(&type_id) {
            // It loads now: the failures before were not a streak.
            self.plugin_worked(&type_id);
        }
        let PluginHandle {
            id,
            device,
            layout,
            descriptor,
            restore_error,
            ..
        } = handle;
        self.plugins.instances.insert(key.clone(), id);
        self.plugins.presets.remove(key);
        self.plugins.preset_current.remove(key);
        self.plugins.keys.insert(id, key.clone());
        // The live plugin has another layout than the scan the registry came from (it changed
        // its buses since, or after a sign-in): the scan cache and the catalog learn it below.
        let scan_is_stale = self
            .registry
            .get(&type_id)
            .is_none_or(|f| !same_shape(f.descriptor, descriptor))
            || self
                .plugins
                .host
                .catalog()
                .entry(plugin)
                .is_some_and(|e| !e.layout.same_ports(&layout));
        if scan_is_stale {
            self.report_cables_off(plugin, &layout);
        }
        self.adopt_descriptor(&type_id, descriptor);
        let live_layout = layout.clone();
        // The full param table and the real vendor and version; not an undo step.
        if let Err(e) = self.sync_silent(Command::Plugin(PluginCommand::SyncLayout {
            device: key.clone(),
            layout,
        })) {
            self.reject(e.to_string());
        }
        if let Some(reason) = restore_error {
            let message = format!("its saved state could not be restored: {reason}");
            self.plugins.log(format!("[{key}] {message}"));
            self.plugins.failed.insert(key.clone(), message);
            self.plugins.no_capture.insert(key.clone());
        } else {
            self.plugins.failed.remove(key);
        }
        let info = InUsePlugin {
            plugin: plugin.clone(),
            path: self.plugins.host.path_of(plugin).unwrap_or_default(),
        };
        // A sandboxed plugin cannot take the app down, so there is nothing to journal for it.
        if !self.plugins.host.is_sandboxed()
            && let Some(journal) = &mut self.plugins.journal
        {
            let _ = journal.loaded(info.clone());
        }
        self.plugins.loaded.insert(id, info);
        self.plugins.live_desc.insert(key.clone(), descriptor);
        if self.sink.has_engine() {
            self.swap_in_device(key, Box::new(device));
        } else {
            // No engine (audio is off): a device handed to none is dropped, so the instance is
            // parked instead. Its editor and params work; the engine gets it later.
            self.plugins.host.manager().retain(id, true);
            drop(device);
            self.plugins.offline.insert(key.clone());
        }
        self.plugins.log(format!("[{key}] loaded"));
        if scan_is_stale && self.learn_layout(plugin, &live_layout) {
            self.rebuild_engine();
        }
    }

    /// Attaches the instances created while no engine ran, now that one exists, and the ones an
    /// engine restart left behind stand-ins while their plugins deactivated and activated.
    fn adopt_offline(&mut self) {
        if self.plugins.offline.is_empty() || !self.sink.has_engine() {
            return;
        }
        let sample_rate = f64::from(self.document.doc().sample_rate);
        for key in self.plugins.offline.clone() {
            let Some(id) = self.plugins.instances.get(&key).copied() else {
                self.plugins.offline.remove(&key);
                continue;
            };
            match self.plugins.host.manager().begin_reattach(id, sample_rate) {
                Ok(Some(handle)) => {
                    self.plugins.offline.remove(&key);
                    self.plugins
                        .live_desc
                        .insert(key.clone(), handle.descriptor);
                    self.swap_in_device(&key, Box::new(handle.device));
                }
                // The plugin is not active yet: next frame.
                Ok(None) => {}
                // The reaper has not parked it yet: next frame.
                Err(atomik_plugin_host::ManagerError::NotParked(_)) => {}
                Err(e) => {
                    self.plugins.offline.remove(&key);
                    self.plugins.failed.insert(key.clone(), e.to_string());
                }
            }
        }
    }

    /// Inserts `device` for the flat device `key` under a new id, resubmits routing and song for
    /// it and removes the stand-in (insert, params, routing, then the removal).
    fn swap_in_device(&mut self, key: &DeviceKey, device: Box<dyn Device>) {
        let Some(old) = self.ids.get(key).copied() else {
            return;
        };
        let expanded = self.doc().expand(&self.registry);
        let Some(flat) = expanded.devices.iter().find(|d| &d.key == key) else {
            return;
        };
        let Some(factory) = self.registry.get(&flat.type_id) else {
            return;
        };
        let desc = factory.descriptor;
        let params: Vec<(atomik_core::ParamId, f32)> = flat
            .params
            .iter()
            .filter_map(|(k, v)| desc.param_by_key(k).map(|p| (p.id, *v)))
            .collect();
        let id = DeviceInstanceId(self.next_id);
        self.next_id += 1;
        self.ids.insert(key.clone(), id);
        let result = self.sink.insert_device(id, device, &params);
        self.engine_result(result);
        self.fx_meter_seq.remove(key);
        self.submit_routing_with(&expanded);
        self.submit_song(false);
        let result = self.sink.remove_device(old);
        self.engine_result(result);
    }

    // ------------------------------------------------------------------ state capture

    /// Captures the state of the instance behind `key` and stores it in the document
    /// (`SyncState`, not an undo step). A failed capture or an instance whose restore failed
    /// keeps the saved blob.
    pub(super) fn capture_instance(&mut self, key: &DeviceKey) -> bool {
        self.capture_instance_as(key, false)
    }

    /// [`Self::capture_instance`]; with `undoable` a state that differs from the stored one
    /// becomes one undo step (`SetState`, the captures of one burst folded together) unless
    /// the user turned a param in the plugin window since the last capture (that gesture is
    /// its own step). A device with no stored state yet has nothing to go back to and is
    /// stored silently.
    fn capture_instance_as(&mut self, key: &DeviceKey, undoable: bool) -> bool {
        let Some(id) = self.plugins.instances.get(key).copied() else {
            return false;
        };
        // This capture waits for the request that is out and takes its place (a preset load
        // whose result it was is over: the state is stored without a step of its own).
        if let Some(out) = self.plugins.capturing.remove(&id)
            && out.preset.is_some()
        {
            self.plugins.preset_loads.remove(key);
        }
        if self.plugins.no_capture.contains(key) {
            return false;
        }
        let blob = match self.plugins.host.manager().capture_state(id) {
            Ok(blob) => blob,
            Err(e) => {
                if e.is_not_responding() {
                    // Alive but busy: the document keeps the last state it holds, and the save
                    // that asked tells the user.
                    if !self.plugins.not_handed.contains(key) {
                        self.plugins.not_handed.push(key.clone());
                    }
                    self.plugins.log(format!(
                        "[{key}] did not hand over its state (not responding): the stored state is kept"
                    ));
                    return false;
                }
                if self.note_state_too_large(key, &e) {
                    return false;
                }
                self.plugins
                    .log(format!("[{key}] cannot capture state: {e}"));
                return false;
            }
        };
        self.store_captured(key, blob, undoable)
    }

    /// Asks the plugin behind `key` for its state without waiting: [`Self::plugins_poll_captures`]
    /// stores it when it arrives (see [`Self::capture_instance_as`] for `undoable`). A plugin
    /// whose main thread is busy answers when it is free, and nothing here stands still; a
    /// request while one is out only asks again when it lands. False when nothing was asked (no
    /// instance, a state that must not be captured over, a request that failed).
    fn request_capture_for(&mut self, key: &DeviceKey, undoable: bool) -> bool {
        let Some(id) = self.plugins.instances.get(key).copied() else {
            return false;
        };
        if self.plugins.no_capture.contains(key) {
            return false;
        }
        if self.plugins.host.manager_ref().not_responding(id).is_some() {
            // Nothing is sent to a plugin that does not answer; it asks for its state again
            // when it is back (the stored state stays meanwhile).
            return false;
        }
        if let Err(e) = self.plugins.host.manager().request_capture(id) {
            self.plugins
                .log(format!("[{key}] cannot capture state: {e}"));
            return false;
        }
        self.plugins.capturing.entry(id).or_default().undoable |= undoable;
        true
    }

    /// Stores the states that arrived since the last call. An answer that is out of date (the
    /// plugin's state was replaced after it was asked for), a failed one and one for a state
    /// that must not be captured over leave the stored state as it is.
    pub(super) fn plugins_poll_captures(&mut self) {
        if self.plugins.capturing.is_empty() {
            return;
        }
        let ids: Vec<InstanceId> = self.plugins.capturing.keys().copied().collect();
        for id in ids {
            let Some(landed) = self.plugins.host.manager().poll_capture(id) else {
                continue;
            };
            let Some(wanted) = self.plugins.capturing.remove(&id) else {
                continue;
            };
            let Some(key) = self.plugins.keys.get(&id).cloned() else {
                continue;
            };
            if let Some(preset) = wanted.preset {
                // The result of a preset load: its own undo step, whatever else was due.
                self.plugins.preset_loads.remove(&key);
                match landed.result {
                    Ok(blob) if !landed.stale => self.finish_preset_load(&key, id, blob, preset),
                    Ok(_) => {}
                    Err(e) => self.reject(format!("cannot read the plugin after loading: {e}")),
                }
                continue;
            }
            if landed.stale {
                continue;
            }
            match landed.result {
                Ok(blob) => {
                    if !self.plugins.no_capture.contains(&key) {
                        self.store_captured(&key, blob, wanted.undoable);
                    }
                }
                Err(e) if e.is_not_responding() => {}
                Err(e) => {
                    if !self.note_state_too_large(&key, &e) {
                        self.plugins
                            .log(format!("[{key}] cannot capture state: {e}"));
                    }
                }
            }
            if landed.again {
                // It changed again while the request was out: the next capture follows its
                // quiet period.
                let now = self.plugins.clock;
                self.plugins.dirty.entry(id).or_insert(now);
            }
        }
    }

    /// Notes that the state of `key` is over a cap: the marker the panel shows, the device named in
    /// the save's notice, and one warning line when the situation changes (not one per capture).
    fn state_refused(&mut self, key: &DeviceKey, t: TooLarge) {
        let before = self.plugins.too_large.insert(key.clone(), t);
        if before.is_none_or(|b| b.situation() != t.situation()) {
            let line = too_large_log_line(&key.0, &t);
            log::warn!("{line}");
            self.plugins.log(line);
        }
        if !self.plugins.too_large_hit.contains(key) {
            self.plugins.too_large_hit.push(key.clone());
        }
    }

    /// When `e` says the plugin could not hand its state over because it does not fit its channel:
    /// the same as a state over a cap (the plugin itself is fine and keeps running). False for any
    /// other error.
    fn note_state_too_large(&mut self, key: &DeviceKey, e: &ManagerError) -> bool {
        let Some((bytes, limit)) = e.state_too_large() else {
            return false;
        };
        self.state_refused(
            key,
            TooLarge {
                bytes,
                limit,
                project: false,
                frame: true,
            },
        );
        true
    }

    /// Puts a state the live plugin gave into the document (see [`Self::capture_instance_as`]).
    fn store_captured(
        &mut self,
        key: &DeviceKey,
        blob: atomik_device_sdk::PluginStateBlob,
        undoable: bool,
    ) -> bool {
        // A state over a cap is not offered to the document (it would refuse it at every
        // capture): the device is marked and keeps the last state that fitted.
        if let Some(r) = self.doc().state_refusal(key, blob.0.len() as u64) {
            self.state_refused(
                key,
                TooLarge {
                    bytes: r.bytes,
                    limit: r.limit,
                    project: r.project,
                    frame: false,
                },
            );
            return false;
        }
        if self.plugins.too_large.remove(key).is_some() {
            self.plugins.log(fits_again_log_line(&key.0, blob.0.len()));
        }
        let stored = self
            .doc()
            .resolve(key)
            .and_then(|d| d.plugin.as_ref())
            .map(|p| p.state.clone());
        let changed = stored.as_ref().is_none_or(|s| s.as_deref() != Some(&blob));
        let touched = self.plugins.gesture_touched.remove(key);
        if !changed {
            return true;
        }
        if undoable && !touched && matches!(stored, Some(Some(_))) {
            return self.apply_state_step(key, blob);
        }
        match self.sync_silent(Command::Plugin(PluginCommand::SyncState {
            device: key.clone(),
            state: blob,
        })) {
            Ok(()) => {
                if let Some(r) = &mut self.plugins.recovery {
                    r.wanted = true;
                }
                true
            }
            Err(e) => {
                self.reject(e.to_string());
                false
            }
        }
    }

    /// Stores `blob` as the device's state in one undo step. The live instance already holds
    /// it, so the sync op that follows must not restore it into the plugin again.
    fn apply_state_step(
        &mut self,
        key: &DeviceKey,
        blob: atomik_device_sdk::PluginStateBlob,
    ) -> bool {
        let clock = self.plugins.clock;
        let (id, last) = self.plugins.state_merge;
        let id = if clock - last > STATE_STEP_GAP_SECS {
            id + 1
        } else {
            id
        };
        self.plugins.state_merge = (id, clock);
        self.plugins.from_plugin = true;
        let done = self.apply_command(Command::Plugin(PluginCommand::SetState {
            device: key.clone(),
            state: blob,
            merge: Some(id),
        }));
        self.plugins.from_plugin = false;
        if done.is_ok() {
            self.refresh_knobs(key);
        }
        self.plugins.preset_current.remove(key);
        if let Some(r) = &mut self.plugins.recovery {
            r.wanted = true;
        }
        done.is_ok()
    }

    // ------------------------------------------------------------------ presets

    /// Puts the param values the live plugin holds into the document, so the rack's knobs show
    /// them: a preset or a patch change moves the plugin's own params and the document still
    /// holds the old ones. The changes join the undo step the state change just made (undo
    /// puts the old knobs back together with the old state), and nothing is sent to the
    /// plugin, which has the values already.
    ///
    /// The values are asked for without waiting: a plugin in another process that is busy
    /// answers when it is free, and the knobs move then ([`Self::apply_soft_answers`]). That
    /// answer still joins the undo step when nothing was done to the document meanwhile.
    fn refresh_knobs(&mut self, key: &DeviceKey) {
        let Some(id) = self.plugins.instances.get(key).copied() else {
            return;
        };
        match self.plugins.host.manager().request_params(id) {
            Ok(Some(values)) => self.apply_param_values(key, values, true),
            Ok(None) => {
                self.plugins
                    .knob_reads
                    .insert(key.clone(), self.document.revision());
            }
            Err(e) => self
                .plugins
                .log(format!("[{key}] cannot read the params: {e}")),
        }
    }

    /// Puts `values` (what the plugin holds) into the document as the knobs' values. With
    /// `joins` they join the undo step the state change made; without (the document changed
    /// since) they make one step of their own.
    fn apply_param_values(&mut self, key: &DeviceKey, values: Vec<(u32, f64)>, joins: bool) {
        let changes: Vec<(String, f32)> = values
            .into_iter()
            .filter(|(param, _)| self.param_known(key, *param))
            .filter_map(|(param, value)| {
                let name = format!("p{param}");
                let wanted = value as f32;
                let current = self.doc().param_value(key, &name, &self.registry)?;
                // A NaN from a plugin must not become a knob value.
                (wanted.is_finite() && (current - wanted).abs() > 1e-6).then_some((name, wanted))
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        self.plugins.from_plugin = true;
        for (n, (param, value)) in changes.into_iter().enumerate() {
            self.join_next = joins || n > 0;
            let _ = self.apply_command(Command::SetParam {
                key: key.clone(),
                param,
                value,
                merge: None,
            });
            self.join_next = false;
        }
        self.plugins.from_plugin = false;
    }

    /// The presets the plugin behind `device` offers, asked for once per instance and never
    /// waited for: empty for a device without a live instance, for a plugin with none, and
    /// until the plugin's answer lands (see [`Self::plugin_presets_loading`]). A list the plugin
    /// failed to give is not taken for "no presets": it is asked for again the next time the
    /// pane or the browser opens (see [`Self::retry_failed_presets`]).
    pub fn plugin_presets(&mut self, device: &DeviceKey) -> Vec<PresetInfo> {
        match self.plugins.presets.get(device) {
            Some(PresetList::Ready(list)) => return list.clone(),
            Some(PresetList::Asking | PresetList::Failed(_)) => return Vec::new(),
            None => {}
        }
        self.ask_presets(device);
        match self.plugins.presets.get(device) {
            Some(PresetList::Ready(list)) => list.clone(),
            _ => Vec::new(),
        }
    }

    /// True while the presets of `device` were asked for and the plugin has not answered.
    pub fn plugin_presets_loading(&self, device: &DeviceKey) -> bool {
        matches!(self.plugins.presets.get(device), Some(PresetList::Asking))
    }

    /// Asks the plugin behind `device` for its presets: the list is kept when it comes, at
    /// once for a plugin in this process and on a later frame for a plugin in another process.
    fn ask_presets(&mut self, device: &DeviceKey) {
        let Some(id) = self.plugins.instances.get(device).copied() else {
            return;
        };
        let now = self.plugins.clock;
        let entry = match self.plugins.host.manager().request_presets(id) {
            Ok(Some(list)) => PresetList::Ready(list),
            Ok(None) => PresetList::Asking,
            Err(e) => {
                self.plugins
                    .log(format!("[{device}] cannot list the presets: {e}"));
                PresetList::Failed(now)
            }
        };
        self.plugins.presets.insert(device.clone(), entry);
    }

    /// Asks again for the presets of `device` when the last answer was a failure that is at
    /// least [`PRESET_RETRY_SECS`] old.
    pub(super) fn retry_failed_presets(&mut self, device: &DeviceKey) {
        if let Some(PresetList::Failed(at)) = self.plugins.presets.get(device)
            && self.plugins.clock - *at >= PRESET_RETRY_SECS
        {
            self.plugins.presets.remove(device);
            self.ask_presets(device);
        }
    }

    /// The presets of `device` came: a patch browser aimed at it lists them.
    fn presets_arrived(&mut self, device: &DeviceKey) {
        let any = matches!(
            self.plugins.presets.get(device),
            Some(PresetList::Ready(list)) if !list.is_empty()
        );
        if any && self.browsing(device) {
            self.browse_patches(device);
        }
    }

    /// The id of the preset loaded last into `device` and still in place.
    pub fn plugin_preset_current(&self, device: &DeviceKey) -> Option<&str> {
        self.plugins.preset_current.get(device).map(String::as_str)
    }

    /// Loads the preset `preset` into the plugin behind `device` as one undo step: undo puts
    /// the previous state back into the live plugin. A refusal shows as a toast and changes
    /// nothing.
    ///
    /// A plugin in another process may take long to load (a sampler reading its library), so
    /// the load is a polled step: this call starts it and the frames carry it on
    /// ([`Self::plugins_poll_preset_loads`]); the undo step is made, the knobs move and the
    /// preset shows as the one loaded when the plugin has answered. True when the load went
    /// through or is under way. A plugin in this process answers at once, and so does an
    /// offline render.
    pub fn load_plugin_preset(&mut self, device: &DeviceKey, preset: &str) -> bool {
        if !self.plugins.instances.contains_key(device) {
            self.reject("the plugin is not running".to_string());
            return false;
        }
        if self.plugins.preset_loads.contains_key(device) {
            self.reject("a preset is still loading into this plugin".to_string());
            return false;
        }
        self.plugins.preset_loads.insert(
            device.clone(),
            PresetLoad {
                preset: preset.to_string(),
                stage: LoadStage::Before,
            },
        );
        self.advance_preset_load(device)
    }

    /// Moves the preset loads on one step each (once per frame).
    pub(super) fn plugins_poll_preset_loads(&mut self) {
        if self.plugins.preset_loads.is_empty() {
            return;
        }
        let keys: Vec<DeviceKey> = self.plugins.preset_loads.keys().cloned().collect();
        for key in keys {
            self.advance_preset_load(&key);
        }
    }

    /// Takes the load of `key` as far as it goes without waiting: capturing the state the step
    /// goes back to, loading, and asking for the state that results (which
    /// [`Self::plugins_poll_captures`] turns into the undo step). False when it failed, which
    /// is told to the user.
    fn advance_preset_load(&mut self, key: &DeviceKey) -> bool {
        let Some(load) = self.plugins.preset_loads.get(key).cloned() else {
            return true;
        };
        let Some(id) = self.plugins.instances.get(key).copied() else {
            // The instance went (crashed, reloaded, removed): nothing to finish.
            self.plugins.preset_loads.remove(key);
            return false;
        };
        if load.stage == LoadStage::Before {
            // The step must go back to what the plugin holds now, whatever it did since the
            // last capture: the capture is asked for in the background and waited for here.
            if self.plugins.dirty.contains_key(&id) {
                self.plugins.dirty.remove(&id);
                self.request_capture_for(key, false);
            }
            self.plugins_poll_captures();
            if self.plugins.capturing.contains_key(&id) {
                return true;
            }
            match self
                .plugins
                .host
                .manager()
                .begin_load_preset(id, &load.preset)
            {
                Err(e) => {
                    self.plugins.preset_loads.remove(key);
                    self.reject(e.to_string());
                    return false;
                }
                Ok(true) => {}
                Ok(false) => {
                    if let Some(l) = self.plugins.preset_loads.get_mut(key) {
                        l.stage = LoadStage::Loading;
                    }
                    return true;
                }
            }
        } else {
            match self.plugins.host.manager().poll_load_preset(id) {
                None => return true,
                Some(Err(e)) => {
                    self.plugins.preset_loads.remove(key);
                    self.reject(e.to_string());
                    return false;
                }
                Some(Ok(())) => {}
            }
        }
        // Loaded: the plugin's new state is asked for, and the step is made when it lands.
        if let Err(e) = self.plugins.host.manager().request_capture(id) {
            self.plugins.preset_loads.remove(key);
            self.reject(format!("cannot read the plugin after loading: {e}"));
            return false;
        }
        self.plugins.capturing.entry(id).or_default().preset = Some(load.preset);
        self.plugins_poll_captures();
        true
    }

    /// The state the plugin held after a preset load arrived: one undo step (no merge id), the
    /// knobs follow, and the preset shows as the one loaded.
    fn finish_preset_load(
        &mut self,
        key: &DeviceKey,
        id: InstanceId,
        blob: atomik_device_sdk::PluginStateBlob,
        preset: String,
    ) {
        self.plugins.dirty.retain(|i, _| *i != id);
        self.plugins.no_capture.remove(key);
        self.plugins.from_plugin = true;
        let done = self.apply_command(Command::Plugin(PluginCommand::SetState {
            device: key.clone(),
            state: blob,
            merge: None,
        }));
        self.plugins.from_plugin = false;
        match done {
            Ok(()) => {
                self.refresh_knobs(key);
                self.plugins.preset_current.insert(key.clone(), preset);
            }
            Err(e) => self.reject(e.to_string()),
        }
    }

    /// Loads the preset `delta` places after (negative: before) the one loaded last, wrapping
    /// round the list; with none loaded yet a step forward loads the first and a step back the
    /// last.
    pub fn step_plugin_preset(&mut self, device: &DeviceKey, delta: i32) -> bool {
        let list = self.plugin_presets(device);
        if list.is_empty() {
            let text = if self.plugin_presets_loading(device) {
                "the presets of this plugin are still loading"
            } else {
                "this plugin offers no presets"
            };
            self.reject(text.to_string());
            return false;
        }
        let len = list.len() as i64;
        let at = self
            .plugin_preset_current(device)
            .and_then(|c| list.iter().position(|p| p.id == c));
        let next = match at {
            Some(i) => (i as i64 + i64::from(delta)).rem_euclid(len),
            None if delta >= 0 => 0,
            None => len - 1,
        };
        let id = list[next as usize].id.clone();
        self.load_plugin_preset(device, &id)
    }

    /// Captures every live instance (before a save, a recovery snapshot, an engine restart).
    /// The requests go to every sandboxed plugin first and the answers are collected after, so
    /// the wait is the slowest plugin's, not the sum.
    pub fn plugins_capture_all(&mut self) {
        let keys: Vec<DeviceKey> = self.plugins.instances.keys().cloned().collect();
        let ids: Vec<InstanceId> = keys
            .iter()
            .filter(|key| !self.plugins.no_capture.contains(*key))
            .filter_map(|key| self.plugins.instances.get(key).copied())
            .collect();
        self.plugins.host.manager().begin_capture(&ids);
        for key in keys {
            self.capture_instance(&key);
        }
        self.plugins.dirty.clear();
    }

    /// [`Self::plugins_capture_all`] for a save, an archive, a share, a template or a version:
    /// a plugin that is alive but does not hand over its state in time is not stopped, and what
    /// the document holds for it (the last state captured) is what gets written; the user is told
    /// once, with one notice for all of them, so a save never goes stale silently.
    pub(super) fn plugins_capture_for_save(&mut self) {
        self.plugins.not_handed.clear();
        self.plugins.too_large_hit.clear();
        self.plugins_capture_all();
        let keys = std::mem::take(&mut self.plugins.not_handed);
        let large = std::mem::take(&mut self.plugins.too_large_hit);
        if keys.is_empty() && large.is_empty() {
            return;
        }
        let holds_state = |core: &Self, k: &DeviceKey| {
            core.doc()
                .resolve(k)
                .and_then(|d| d.plugin.as_ref())
                .is_some_and(|p| p.state.is_some())
        };
        let text = if large.is_empty() {
            let names: Vec<String> = keys.iter().map(|k| self.device_label(k)).collect();
            let stored = keys.iter().all(|k| holds_state(self, k));
            not_handed_notice(&names, stored)
        } else if keys.is_empty() {
            let refused: Vec<(DeviceKey, TooLarge)> = large
                .iter()
                .filter_map(|k| Some((k.clone(), *self.plugins.too_large.get(k)?)))
                .collect();
            let names: Vec<String> = refused.iter().map(|(k, _)| self.device_label(k)).collect();
            let sizes: Vec<TooLarge> = refused.iter().map(|(_, t)| *t).collect();
            let stored = refused.iter().all(|(k, _)| holds_state(self, k));
            too_large_notice(&names, &sizes, stored)
        } else {
            let mut all = keys.clone();
            all.extend(large.iter().filter(|k| !keys.contains(k)).cloned());
            let names: Vec<String> = all.iter().map(|k| self.device_label(k)).collect();
            unsaved_settings_notice(&names)
        };
        log::warn!("{text}");
        self.plugins.log(text.clone());
        self.reject(text);
    }

    /// Captures, now and waiting for the plugin, the instances whose plugin flagged its state
    /// dirty or whose background capture has not come back: before a command that could remove or
    /// replace a device (so the removal's undo brings the latest state back) and before an undo
    /// or redo. With `undoable` (an undo) a patch change made in the plugin window becomes its
    /// own step first. Any other command leaves the plugin dirty for the background capture, and
    /// so does a change that comes from a plugin's own window (it runs inside a capture).
    pub(super) fn plugins_flush_dirty(&mut self, undoable: bool) {
        if self.plugins.from_plugin {
            return;
        }
        let mut ids: BTreeMap<InstanceId, bool> = self
            .plugins
            .dirty
            .keys()
            .map(|id| (*id, undoable))
            .collect();
        for (id, wanted) in &self.plugins.capturing {
            // A step the arrival would have made (a closed editor's) is made here instead.
            *ids.entry(*id).or_insert(false) |= undoable || wanted.undoable;
        }
        if ids.is_empty() {
            return;
        }
        self.plugins.dirty.clear();
        for (id, undoable) in ids {
            if let Some(key) = self.plugins.keys.get(&id).cloned() {
                self.capture_instance_as(&key, undoable);
            }
        }
    }

    /// True for a command that leaves every plugin device in place: the live app then does not wait
    /// for a plugin's state before it (see [`Self::plugins_flush_dirty`] and `app_core_plugins_keep.rs`).
    pub(super) fn command_keeps_plugins(command: &Command) -> bool {
        keep::keeps_plugins(command)
    }

    /// Applies a data sync (`SyncState`, `SyncLayout`): it plans nothing and sends nothing to the
    /// engine. A layout sync is mirrored into the saved baseline so loading a plugin does not make
    /// the project look changed; a state sync is a change only when the bytes changed.
    fn sync_silent(&mut self, command: Command) -> Result<(), ModelError> {
        let layout = matches!(command, Command::Plugin(PluginCommand::SyncLayout { .. }));
        self.document.apply(command.clone(), &self.registry)?;
        if layout {
            let mut base = Document::new(self.saved.clone());
            if base.apply(command, &self.registry).is_ok() {
                self.set_saved(base.doc().clone());
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ sync ops

    /// The plugin part of a `PluginChanged` sync op: pins and the panel are view only; power and
    /// run mode go to the live instance; a relink (another plugin behind the device) rebuilds.
    pub(super) fn plugin_changed(&mut self, device: &DeviceKey) {
        let Some(pd) = self.doc().resolve(device).and_then(|d| d.plugin.as_deref()) else {
            return;
        };
        let (powered, pinned, plugin) = (pd.powered, pd.run_on_audio_thread, pd.plugin_ref());
        let Some(id) = self.plugins.instances.get(device).copied() else {
            // A stand-in whose plugin changed (a relink): queue the new one.
            // A relink is a new plugin: the skip belonged to the old one.
            self.plugins.skipped.remove(device);
            if self.plugin_usable(&plugin)
                && !self.plugins.pending.contains(device)
                && !self.quarantine_blocks(&plugin, device)
            {
                self.plugins.failed.remove(device);
                self.plugins.pending.push_back(device.clone());
                self.plugins.batch_total += 1;
            }
            return;
        };
        let live = self.plugins.loaded.get(&id).map(|i| i.plugin.clone());
        if live.as_ref() != Some(&plugin) {
            // Another plugin now stands behind this device: start over with the engine.
            self.rebuild_engine();
            return;
        }
        if let Some(slot) = self.plugins.host.manager_ref().slot(id) {
            slot.set_powered(powered);
            slot.set_pinned(pinned);
        }
    }

    /// A patch (or its undo or redo) replaced the state blob of the plugin behind `device`: the
    /// live instance restores it. A device without an instance takes the document's state when
    /// one is created, and a state of `None` (a reset) leaves the instance as it is.
    pub(super) fn plugin_state_changed(&mut self, device: &DeviceKey) {
        self.plugins.preset_current.remove(device);
        if self.plugins.from_plugin {
            // The state came from the live plugin: it already holds it.
            return;
        }
        let blob = self
            .doc()
            .resolve(device)
            .and_then(|d| d.plugin.as_deref())
            .and_then(|p| p.state.as_deref().cloned());
        let (Some(blob), Some(id)) = (blob, self.plugins.instances.get(device).copied()) else {
            return;
        };
        match self.plugins.host.manager().restore_state(id, &blob) {
            Ok(()) => {
                self.plugins.no_capture.remove(device);
            }
            Err(e) => {
                self.plugins
                    .failed
                    .insert(device.clone(), format!("restore failed: {e}"));
            }
        }
    }

    /// True while the commands being applied came from the plugin's own window.
    pub(super) fn echo_suppressed(&self) -> bool {
        self.plugins.from_plugin
    }

    // ------------------------------------------------------------------ GUI edits

    fn plugin_param_edit(
        &mut self,
        instance: InstanceId,
        param: u32,
        value: f64,
        phase: ParamPhase,
    ) {
        let Some(key) = self.plugins.keys.get(&instance).cloned() else {
            return;
        };
        let actions = match phase {
            ParamPhase::Begin => {
                // Learn acts on the touch first; the edit then goes through as usual.
                self.plugin_learn_begin(&key, param);
                // A touch is a click for the camera too, whether or not Learn is armed.
                self.camera_plugin_touch(&key, param, false, self.plugins.clock);
                self.plugins.gesture_touched.insert(key.clone());
                self.plugins.open_gestures.insert((key.clone(), param));
                self.plugins.merger.begin(&key, param)
            }
            ParamPhase::Change => {
                if !self.plugins.open_gestures.contains(&(key.clone(), param)) {
                    // A plugin that sends no gesture begin: its first change is the touch.
                    self.plugin_learn_change(&key, param);
                    self.camera_plugin_touch(&key, param, true, self.plugins.clock);
                    // Not a gesture: a patch change or an internal modulation. Not stored.
                    return;
                }
                if !self.param_known(&key, param) {
                    return;
                }
                self.plugins.merger.change(&key, param, value as f32)
            }
            ParamPhase::End => {
                self.plugins.open_gestures.remove(&(key.clone(), param));
                self.plugins.merger.end(&key, param)
            }
        };
        self.plugins.from_plugin = true;
        for action in actions {
            match action {
                UiAction::Command(c) => {
                    let _ = self.apply_command(c);
                }
                UiAction::EndMerge => {
                    self.document.end_merge();
                    self.release_automation();
                }
                _ => {}
            }
        }
        self.plugins.from_plugin = false;
    }

    fn param_known(&self, key: &DeviceKey, id: u32) -> bool {
        self.doc()
            .resolve(key)
            .and_then(|d| self.registry.get(&d.type_id))
            .is_some_and(|f| f.descriptor.param_by_key(&format!("p{id}")).is_some())
    }

    /// The plugin's param table or ports changed: register the new descriptor and rebuild the
    /// device of that plugin alone, keeping values by key ([`Self::swap_device_layout`]). The
    /// whole engine is rebuilt only when another live instance of the same plugin has to follow.
    fn plugin_layout_changed(
        &mut self,
        instance: InstanceId,
        layout: atomik_device_sdk::PluginLayout,
    ) {
        let Some(key) = self.plugins.keys.get(&instance).cloned() else {
            return;
        };
        let Some(plugin) = self.plugins.loaded.get(&instance).map(|i| i.plugin.clone()) else {
            return;
        };
        // The document already holds exactly this layout: nothing changed for the engine.
        if self
            .doc()
            .resolve(&key)
            .and_then(|d| d.plugin.as_deref())
            .is_some_and(|p| p.layout == layout)
        {
            return;
        }
        self.report_cables_off(&plugin, &layout);
        let _ = self.sync_silent(Command::Plugin(PluginCommand::SyncLayout {
            device: key.clone(),
            layout: layout.clone(),
        }));
        // The scan cache, the catalog and the registry all learn the new layout: a plugin added
        // later, a project opened later and the browser's rows agree with the live one.
        self.learn_layout(&plugin, &layout);
        if let Ok(descriptor) = atomik_device_sdk::descriptor_for(&plugin, &layout) {
            self.adopt_descriptor(&plugin.type_id(), descriptor);
        }
        if !self.swap_device_layout(&key, &plugin, instance) {
            self.rebuild_engine();
        }
    }

    /// Gives the engine the device of `key` for the layout its plugin has now, without
    /// restarting the engine (the transport, the other devices and their tails keep going): a
    /// stand-in of the new shape takes the device's place, the instance is parked when the old
    /// device goes, and [`Self::adopt_offline`] puts the new live device in as soon as the
    /// plugin is active again (the way a plugin that is busy deactivating is waited for after
    /// an engine restart). True when that was done or there was nothing to replace.
    ///
    /// False when the engine has to be rebuilt after all: another live instance of the same
    /// plugin was built with the old shape, and the registry describes the new one now, so only
    /// a restart makes them agree (that instance is created again, as after any restart).
    fn swap_device_layout(&mut self, key: &DeviceKey, plugin: &PluginRef, id: InstanceId) -> bool {
        let type_id = plugin.type_id();
        let Some(shape) = self.registry.get(&type_id).map(|f| f.descriptor) else {
            return false;
        };
        let follows = self.plugins.live_desc.iter().any(|(other, built)| {
            other != key
                && self.plugins.instances.contains_key(other)
                && !same_shape(built, shape)
                && self
                    .doc()
                    .resolve(other)
                    .and_then(|d| d.plugin.as_ref())
                    .is_some_and(|p| p.plugin_ref() == *plugin)
        });
        if follows {
            return false;
        }
        if !self.sink.has_engine() || self.plugins.offline.contains(key) {
            // No device of its own in the engine: the one that is made when the instance is
            // attached again takes the new shape from the registry.
            return true;
        }
        if !self.ids.contains_key(key) {
            return false;
        }
        let Some(standin) = self.registry.create(&type_id) else {
            return false;
        };
        self.plugins.host.manager().retain(id, true);
        self.plugins.offline.insert(key.clone());
        self.plugins.live_desc.insert(key.clone(), shape);
        self.swap_in_device(key, standin);
        true
    }

    /// The live plugin `plugin` reported `layout`, which is not what the scan cache holds: the
    /// cache (saved to `plugins.json` at the next frame), the catalog and the registry take it.
    /// True when a live device needs an engine rebuild.
    fn learn_layout(
        &mut self,
        plugin: &PluginRef,
        layout: &atomik_device_sdk::PluginLayout,
    ) -> bool {
        if self.plugins.cache.update_layout(plugin, layout) {
            self.plugins.cache_dirty = true;
        }
        self.register_catalog()
    }

    /// Tells the user which cables go because the plugin no longer has the ports they use.
    fn report_cables_off(&mut self, plugin: &PluginRef, layout: &atomik_device_sdk::PluginLayout) {
        let Ok(desc) = atomik_device_sdk::descriptor_for(plugin, layout) else {
            return;
        };
        let off = atomik_model::cables_off_layout(self.doc(), &plugin.type_id(), desc);
        if off.is_empty() {
            return;
        }
        let ports: BTreeSet<String> = off
            .iter()
            .flat_map(|(_, c)| [c.from.socket.clone(), c.to.socket.clone()])
            .filter(|s| desc.socket_by_key(s).is_none())
            .collect();
        let name = layout.name.clone();
        let ports = ports.into_iter().collect::<Vec<_>>().join(", ");
        let message = format!(
            "{name} no longer has the ports {ports}: {} cable(s) on them were removed",
            off.len()
        );
        self.plugins.log(message.clone());
        self.report(message);
    }

    /// The saved project's plugin layouts may have ports the scan cache lacks (the plugin changed
    /// its buses after the scan, and the project was saved with cables on them): the cache gets
    /// the union of the ports, so the project loads and the plugin can be added again. The live
    /// plugin has the last word when it is created.
    fn adopt_document_ports(&mut self) {
        let saved: Vec<(PluginRef, atomik_device_sdk::PluginLayout)> = self
            .doc()
            .plugin_devices()
            .into_iter()
            .filter_map(|(_, d)| {
                d.plugin
                    .as_ref()
                    .map(|p| (p.plugin_ref(), p.layout.clone()))
            })
            .collect();
        for (plugin, saved) in saved {
            let Some(scanned) = self.plugins.cache.layout_of(&plugin) else {
                continue;
            };
            let (Ok(have), Ok(want)) = (
                atomik_device_sdk::descriptor_for(&plugin, scanned),
                atomik_device_sdk::descriptor_for(&plugin, &saved),
            ) else {
                continue;
            };
            if want
                .sockets
                .iter()
                .all(|s| have.socket_by_key(s.key).is_some())
            {
                continue;
            }
            let mut merged = scanned.clone();
            merged.main_in = merged.main_in.or(saved.main_in);
            merged.main_out = merged.main_out.or(saved.main_out);
            merged.sidechain |= saved.sidechain;
            merged.notes_in |= saved.notes_in;
            merged.notes_out |= saved.notes_out;
            merged.aux_out = merged.aux_out.max(saved.aux_out);
            if self.plugins.cache.update_layout(&plugin, &merged) {
                self.plugins.cache_dirty = true;
            }
        }
    }

    /// Before a command is planned: every plugin it names gets the layout the catalog holds now
    /// (the browser row the user dragged may predate a layout the plugin reported since), and the
    /// registry is brought in line with the catalog, so the creation cannot fail on a stale one.
    pub(super) fn refresh_plugin_command(&mut self, command: &mut Command) {
        let mut stale = false;
        let catalog = self.plugins.host.catalog();
        let registry = &self.registry;
        atomik_model::map_plugin_layouts(command, &mut |plugin, layout| {
            let Some(entry) = catalog.entry(plugin) else {
                return;
            };
            if &entry.layout != layout {
                *layout = entry.layout.clone();
            }
            let want = atomik_device_sdk::descriptor_for(plugin, &entry.layout).ok();
            let have = registry.get(&plugin.type_id()).map(|f| f.descriptor);
            if let (Some(w), Some(h)) = (want, have) {
                stale |= !same_shape(w, h);
            }
        });
        if stale {
            self.refresh_plugin_catalog();
        }
    }

    // ------------------------------------------------------------------ actions

    /// Handles the plugin actions of the views. Returns false for an action that is not one.
    pub(super) fn plugin_action(&mut self, action: &UiAction) -> bool {
        match action {
            UiAction::OpenPluginGui { device } => self.open_plugin_gui(device),
            UiAction::OpenPluginParams { device } => self.open_plugin_params(device),
            UiAction::ClosePluginParams => self.close_plugin_params(),
            UiAction::ReloadPlugin { device } => {
                self.plugins.auto_reloads.remove(device);
                // RELOAD on a quarantined plugin is LOAD ANYWAY.
                self.plugins.load_anyway.insert(device.clone());
                self.reload_plugin(device);
            }
            UiAction::SkipPlugin { device } => {
                self.skip_plugin(device);
            }
            UiAction::LoadPluginPreset { device, preset } => {
                self.load_plugin_preset(device, preset);
                self.feed_preset_bar();
            }
            UiAction::StepPluginPreset { device, delta } => {
                self.step_plugin_preset(device, *delta);
                self.feed_preset_bar();
            }
            UiAction::AutomatePluginParam { device, param } => {
                self.automate_plugin_param(device, *param);
            }
            // The pane's text requests are answered every frame ([`Self::answer_param_texts`]).
            UiAction::PluginParamTexts { .. } => {}
            UiAction::PluginSettings(a) => self.plugin_settings_action(a),
            UiAction::PluginBanner(a) => self.plugin_banner_action(a),
            UiAction::PluginLearn { device, request } => {
                self.plugin_learn_request(device, *request);
            }
            UiAction::PluginDock { device, request } => {
                self.plugin_dock_request(device, *request);
            }
            _ => return false,
        }
        true
    }

    /// A plugin whose process stopped answering audio was ended: it is reloaded from the
    /// document's last captured state, as RELOAD does, [`MAX_AUTO_RELOADS`] times in a row; a
    /// plugin that keeps hanging is left CRASHED for the user to decide.
    fn reload_hung_plugin(&mut self, device: &DeviceKey) {
        let tries = self.plugins.auto_reloads.entry(device.clone()).or_insert(0);
        if *tries >= MAX_AUTO_RELOADS {
            self.plugins.log(format!(
                "[{device}] hung again: not reloaded by itself; press RELOAD to try once more"
            ));
            return;
        }
        *tries += 1;
        self.plugins
            .log(format!("[{device}] hung: reloading it from its last state"));
        self.reload_plugin(device);
    }

    /// RELOAD: a stand-in whose plugin is installed gets its instance; a live instance restores
    /// the saved state and restarts (a FAILED device is reset by the hand-shake); a CRASHED one
    /// (its process is gone) is replaced by a new instance.
    fn reload_plugin(&mut self, device: &DeviceKey) {
        let Some(pd) = self.doc().resolve(device).and_then(|d| d.plugin.as_deref()) else {
            return;
        };
        let plugin = pd.plugin_ref();
        let state = pd.state.as_deref().cloned();
        self.plugins.failed.remove(device);
        self.plugins.skipped.remove(device);
        if let Some(id) = self.plugins.instances.get(device).copied()
            && self.plugins.host.manager().stop_unresponsive(id)
        {
            // The user chose to give up on a plugin that did not respond: its process is
            // stopped, which makes it a lost instance, replaced from the stored state below.
            self.plugins.log(format!(
                "[{device}] did not respond: stopped at your request, reloading it from its last state"
            ));
        }
        match self.plugins.instances.get(device).copied() {
            Some(id) if self.plugins.host.manager_ref().is_lost(id) => {
                // Its process is gone and nothing can restart it: forget it (the engine keeps
                // the crashed device until the new one is swapped in; the manager reaps the old
                // instance then) and create a new one from the document's last captured state
                // and the rack's params, through the normal path.
                if self.plugins.offline.remove(device) {
                    let _ = self.plugins.host.manager().remove(id);
                }
                self.plugins.no_capture.remove(device);
                self.plugins.windows.remove(device);
                self.release_instance_maps(device);
                self.queue_instance(device, &plugin);
            }
            Some(id) => {
                if self.plugins.no_capture.contains(device) {
                    if let Some(blob) = &state {
                        match self.plugins.host.manager().restore_state(id, blob) {
                            Ok(()) => {
                                self.plugins.no_capture.remove(device);
                            }
                            Err(e) => {
                                self.plugins
                                    .failed
                                    .insert(device.clone(), format!("restore failed: {e}"));
                                return;
                            }
                        }
                    } else {
                        self.plugins.no_capture.remove(device);
                    }
                } else {
                    self.capture_instance(device);
                }
                if let Err(e) = self.plugins.host.manager().reload(id) {
                    self.plugins.failed.insert(device.clone(), e.to_string());
                }
            }
            None => self.queue_instance(device, &plugin),
        }
    }

    /// SKIP: stops creating the plugin of `device` (a plugin process that was loading ends, not
    /// waited for) and leaves the device a stand-in that keeps its saved state and says SKIPPED.
    /// It is not a crash: nothing is recorded in the crashed settings and no quarantine count
    /// moves. RELOAD tries again. False when the device is not loading.
    pub fn skip_plugin(&mut self, device: &DeviceKey) -> bool {
        if !self.plugins.pending.contains(device) {
            return false;
        }
        self.plugins.pending.retain(|k| k != device);
        self.cancel_starting(Some(device));
        self.finish_batch_step();
        self.plugins.skipped.insert(device.clone());
        self.plugins.failed.remove(device);
        self.plugins.log(format!(
            "[{device}] skipped: loading stopped at your request"
        ));
        true
    }

    /// The command `plugin.skip_loading`: skips the selected plugin device while it loads.
    pub fn skip_selected_plugin(&mut self) {
        let loading = self
            .view
            .primary()
            .filter(|d| self.plugins.pending.contains(d));
        match loading {
            Some(device) => {
                self.skip_plugin(&device);
            }
            None => self.report("Select a plugin device that is loading first"),
        }
    }

    /// Queues the creation of an instance for `device` (one per frame), or says why not.
    fn queue_instance(&mut self, device: &DeviceKey, plugin: &PluginRef) {
        if self.quarantine_blocks(plugin, device) {
            // The device says QUARANTINED; LOAD ANYWAY is the user's decision.
            return;
        }
        if self.plugin_usable(plugin) {
            if !self.plugins.pending.contains(device) {
                self.plugins.pending.push_back(device.clone());
                self.plugins.batch_total += 1;
            }
        } else {
            let why = self.missing_reason(plugin);
            self.reject(format!("cannot load the plugin: {why}"));
        }
    }

    /// Adds the automation lane for plugin param `param` on the song track of the selected clip.
    fn automate_plugin_param(&mut self, device: &DeviceKey, param: u32) {
        let selected = self.arrangement.selected_clips();
        let track = selected.first().and_then(|clip| {
            self.doc()
                .song
                .tracks
                .iter()
                .find(|t| {
                    t.lanes
                        .iter()
                        .any(|l| l.clips.iter().any(|c| c.id == *clip))
                })
                .map(|t| t.device.clone())
        });
        match track {
            Some(t) if &t == device => {
                let _ = self.apply_command(Command::Song(
                    atomik_model::SongCommand::AddAutomationLane {
                        device: device.clone(),
                        param: format!("p{param}"),
                    },
                ));
            }
            _ => self.reject(
                "select a clip on this plugin's song track to add an automation lane".to_string(),
            ),
        }
    }

    // ------------------------------------------------------------------ the parameter pane

    /// Opens the plugin parameter pane for `device` in place of the arrangement.
    pub fn open_plugin_params(&mut self, device: &DeviceKey) {
        let is_plugin = self
            .doc()
            .resolve(device)
            .is_some_and(|d| d.plugin.is_some());
        if !is_plugin {
            return;
        }
        self.plugin_params.open(device.clone());
        self.plugin_params_open = true;
        self.plugin_params.sync(self.document.doc(), &self.registry);
        self.retry_failed_presets(device);
        self.feed_preset_bar();
    }

    /// Closes the parameter pane.
    pub fn close_plugin_params(&mut self) {
        self.plugin_params_open = false;
        self.plugin_params.set_focused(false);
    }

    /// The device whose parameter pane is open.
    pub fn plugin_params_device(&self) -> Option<&DeviceKey> {
        self.plugin_params_open
            .then(|| self.plugin_params.device())
            .flatten()
    }

    /// The parameter pane.
    pub fn plugin_params(&self) -> &PluginParamsView {
        &self.plugin_params
    }

    /// The parameter pane, mutably.
    pub fn plugin_params_mut(&mut self) -> &mut PluginParamsView {
        &mut self.plugin_params
    }

    /// Gives the parameter pane the presets of the plugin it shows and the one loaded last.
    pub(super) fn feed_preset_bar(&mut self) {
        if !self.plugin_params_open {
            return;
        }
        let Some(device) = self.plugin_params.device().cloned() else {
            return;
        };
        let list = self
            .plugin_presets(&device)
            .into_iter()
            .map(|p| PresetItem {
                id: p.id,
                name: p.name,
                group: p.group,
            })
            .collect();
        let current = self.plugin_preset_current(&device).map(str::to_string);
        self.plugin_params.set_presets(list, current);
    }

    /// Puts to use the cosmetic answers that plugins in other processes gave after the call that
    /// asked: a parameter text goes to the pane when its row still shows the value it was asked
    /// for, and the size an editor took goes where the resize that asked for it was made.
    fn apply_soft_answers(&mut self) {
        let mut texts: Vec<(u32, f32, String)> = Vec::new();
        for (id, answer) in self.plugins.host.manager().take_soft_answers() {
            let Some(device) = self.plugins.keys.get(&id).cloned() else {
                continue;
            };
            match answer {
                SoftAnswer::ParamText { param, plain, text } => {
                    if self.plugin_params_open && self.plugin_params.device() == Some(&device) {
                        texts.push((param, plain as f32, text));
                    }
                }
                SoftAnswer::GuiSized { asked, took } => self.plugin_gui_sized(&device, asked, took),
                // Nothing in the app asks for the default pins of a plugin that is already
                // running.
                SoftAnswer::DefaultPins(_) => {}
                SoftAnswer::Presets(list) => {
                    let now = self.plugins.clock;
                    let entry = match list {
                        Some(list) => PresetList::Ready(list),
                        None => {
                            self.plugins
                                .log(format!("[{device}] the plugin did not list its presets"));
                            PresetList::Failed(now)
                        }
                    };
                    self.plugins.presets.insert(device.clone(), entry);
                    self.presets_arrived(&device);
                }
                SoftAnswer::Params(values) => {
                    // Only the read a patch change asked for is used, and only while the
                    // document is as that change left it can it join the change's undo step.
                    let Some(revision) = self.plugins.knob_reads.remove(&device) else {
                        continue;
                    };
                    match values {
                        Some(values) => {
                            let joins = self.document.revision() == revision;
                            self.apply_param_values(&device, values, joins);
                        }
                        None => self
                            .plugins
                            .log(format!("[{device}] the plugin did not give its params")),
                    }
                }
            }
        }
        if !texts.is_empty() {
            self.plugin_params.offer_texts(&texts);
        }
    }

    /// Answers the pane's display-text requests from the plugin (main thread, once per request).
    /// A plugin that is too busy to answer in the frame's allowance gives nothing now: its text
    /// comes later (see [`Self::apply_soft_answers`]), and the pane shows the number until then.
    fn answer_param_texts(&mut self) {
        if !self.plugin_params_open {
            return;
        }
        let requests = self.plugin_params.take_text_requests();
        if requests.is_empty() {
            return;
        }
        let Some(device) = self.plugin_params.device().cloned() else {
            return;
        };
        let Some(id) = self.plugins.instances.get(&device).copied() else {
            return;
        };
        let texts: Vec<(u32, f32, String)> = requests
            .into_iter()
            .filter_map(|(param, plain)| {
                let text = self
                    .plugins
                    .host
                    .manager()
                    .param_text(id, param, f64::from(plain))?;
                Some((param, plain, text))
            })
            .collect();
        if !texts.is_empty() {
            self.plugin_params.set_texts(&texts);
        }
    }

    // ------------------------------------------------------------------ view feeds

    fn missing_reason(&self, plugin: &PluginRef) -> String {
        if !self.plugins.settings.enabled {
            return "third-party plugins are off in Settings".to_string();
        }
        match self.plugins.host.catalog().entry(plugin).map(|e| &e.status) {
            Some(PluginStatus::Unsupported(r)) => format!("unsupported: {r}"),
            Some(PluginStatus::Disabled) => "disabled in Settings".to_string(),
            Some(PluginStatus::Failed(r)) => r.clone(),
            Some(PluginStatus::Ok) => "audio is off".to_string(),
            None => "not found by the last scan".to_string(),
        }
    }

    fn plugin_status_of(&self, key: &DeviceKey, plugin: &PluginRef) -> PluginDeviceStatus {
        let failed = self.plugins.failed.get(key).cloned();
        let waiting = (!self.sink.has_engine()).then(|| {
            self.sink.engine_error().map_or_else(
                || "audio is off".to_string(),
                |e| format!("audio is off: {e}"),
            )
        });
        match self.plugins.instances.get(key) {
            Some(id) => PluginDeviceStatus {
                has_editor: self.plugins.host.manager_ref().has_gui(*id),
                missing: None,
                failed,
                crashed: self.plugins.crashed.get(key).cloned(),
                not_responding: self.plugins.host.manager_ref().not_responding(*id),
                skipped: false,
                quarantined: None,
                // After an engine restart the device is a stand-in until the plugin is active.
                loading: self.sink.has_engine() && self.plugins.offline.contains(key),
                waiting,
                state_too_large: self.plugins.too_large.get(key).map(|t| (t.bytes, t.limit)),
            },
            None if self.plugins.pending.contains(key) => PluginDeviceStatus {
                has_editor: false,
                missing: None,
                failed,
                crashed: None,
                not_responding: None,
                skipped: false,
                quarantined: None,
                loading: true,
                waiting: None,
                state_too_large: None,
            },
            // The user skipped the load: a stand-in with its saved state, RELOAD tries again.
            None if self.plugins.skipped.contains(key) => PluginDeviceStatus {
                skipped: true,
                ..PluginDeviceStatus::default()
            },
            // The plugin is installed but quarantined: the project did not load it.
            None if self.plugin_usable(plugin) && self.quarantine_blocks(plugin, key) => {
                PluginDeviceStatus {
                    has_editor: false,
                    missing: None,
                    failed,
                    crashed: None,
                    not_responding: None,
                    skipped: false,
                    quarantined: Some(
                        self.plugins
                            .user
                            .quarantine_reason(&plugin.type_id())
                            .unwrap_or_default()
                            .to_owned(),
                    ),
                    loading: false,
                    waiting: None,
                    state_too_large: None,
                }
            }
            // The plugin is installed but its instance could not be created: FAILED with the
            // reason (RELOAD tries again), not MISSING.
            None if failed.is_some() && self.plugin_usable(plugin) => PluginDeviceStatus {
                has_editor: false,
                missing: None,
                failed,
                crashed: None,
                not_responding: None,
                skipped: false,
                quarantined: None,
                loading: false,
                waiting: None,
                state_too_large: None,
            },
            None => PluginDeviceStatus {
                has_editor: false,
                missing: Some(self.missing_reason(plugin)),
                failed,
                crashed: None,
                not_responding: None,
                skipped: false,
                quarantined: None,
                loading: false,
                waiting: None,
                state_too_large: None,
            },
        }
    }

    /// Feeds the views what the app knows: the catalog, the blacklist and each plugin device's
    /// status (only what changed).
    fn feed_plugin_views(&mut self) {
        self.feed_preset_bar();
        if self.plugins.catalog_dirty {
            self.plugins.catalog_dirty = false;
            let catalog = self.plugins.host.catalog();
            let infos = entry_infos(catalog, &self.plugins.user);
            let failed: Vec<FailedFileInfo> = catalog
                .failed()
                .iter()
                .map(|f| FailedFileInfo {
                    path: f.path.to_string_lossy().into_owned(),
                    reason: f.reason.clone(),
                })
                .collect();
            self.view.set_plugin_catalog(infos);
            self.view.set_plugin_failed_files(failed);
        }
        let devices: Vec<(DeviceKey, PluginRef)> = self
            .doc()
            .plugin_devices()
            .into_iter()
            .filter_map(|(k, d)| Some((k, d.plugin.as_ref()?.plugin_ref())))
            .collect();
        for (key, plugin) in devices {
            let mut status = self.plugin_status_of(&key, &plugin);
            if let Some(id) = self.plugins.instances.get(&key)
                && let Some(slot) = self.plugins.host.manager_ref().slot(*id)
                && slot.device_state() == atomik_plugin_host::DeviceState::Failed
                && status.failed.is_none()
            {
                status.failed = Some("the plugin failed; press RELOAD".to_string());
            }
            if let Some(id) = self.plugins.instances.get(&key)
                && let Some(slot) = self.plugins.host.manager_ref().slot(*id)
                && slot.device_state() == atomik_plugin_host::DeviceState::Crashed
                && status.crashed.is_none()
            {
                // The device knows before the process's exit reason arrives.
                status.crashed = Some(LOST.to_string());
            }
            if self.plugins.shown.get(&key) != Some(&status) {
                self.view.set_plugin_status(&key, status.clone());
                self.plugins.shown.insert(key, status);
            }
        }
    }

    // ------------------------------------------------------------------ settings and scanning

    /// Where plugins run now: `Some("sandbox")` (child processes) or `Some("in-process")`,
    /// `None` while plugins are off. Shown in the status line and the Plugins settings.
    pub fn plugin_mode(&self) -> Option<&'static str> {
        self.plugins.settings.enabled.then(|| {
            if self.plugin_sandboxed() {
                "sandbox"
            } else {
                "in-process"
            }
        })
    }

    /// True when plugins run in child processes (the host was built with the sandbox).
    pub fn plugin_sandboxed(&self) -> bool {
        self.plugins.host.is_sandboxed()
    }

    /// Says once that the sandbox the user asked for is not available and plugins run inside
    /// the app instead: in the log and as a message.
    pub fn warn_plugin_isolation(&mut self, warning: &str) {
        log::warn!("{warning}");
        self.plugins.log(format!("[sandbox] {warning}"));
        self.reject(warning.to_string());
    }

    /// What the Plugins section of the settings dialog shows now.
    pub fn plugin_settings_data(&self) -> PluginSettingsData {
        let catalog = self.plugins.host.catalog();
        PluginSettingsData {
            enabled: self.plugins.settings.enabled,
            extra_paths: self
                .plugins
                .settings
                .extra_paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            timeout_secs: self.plugins.settings.timeout_secs,
            plugins: catalog
                .entries()
                .iter()
                .map(|e| SettingsPluginRow {
                    plugin: e.plugin.clone(),
                    name: e.name.clone(),
                    vendor: e.vendor.clone(),
                    version: e.version.clone(),
                    status: match (
                        &e.status,
                        self.plugins.settings.crashed.get(&e.plugin.type_id()),
                    ) {
                        (PluginStatus::Ok, Some(reason)) => {
                            PluginEntryStatus::Crashed(reason.clone())
                        }
                        (status, _) => entry_status(status),
                    },
                })
                .collect(),
            failed: catalog
                .failed()
                .iter()
                .map(|f| FailedFileInfo {
                    path: f.path.to_string_lossy().into_owned(),
                    reason: f.reason.clone(),
                })
                .collect(),
            scan: self.plugins.job.as_ref().map(|j| {
                let (done, total) = j.progress();
                ScanProgressInfo { done, total }
            }),
            scanner_available: self.plugins.runner.is_some(),
            last_scan: self.plugins.scan_note.clone(),
            log: self.plugins.log.iter().cloned().collect(),
            sandbox: self.plugins.settings.sandbox,
            process: match self.plugins.settings.process {
                atomik_plugin_host::ProcessMode::PerPlugin => PluginProcessMode::PerPlugin,
                atomik_plugin_host::ProcessMode::PerFile => PluginProcessMode::PerFile,
                atomik_plugin_host::ProcessMode::Shared => PluginProcessMode::Shared,
            },
            running_sandboxed: self.plugin_sandboxed(),
            editor_home: match self.plugins.settings.editors {
                atomik_plugin_host::EditorHome::OwnWindow => PluginEditorHome::OwnWindow,
                atomik_plugin_host::EditorHome::InApp => PluginEditorHome::InApp,
            },
            docking_available: self.plugin_docking_available(),
        }
    }

    fn plugin_settings_action(&mut self, action: &PluginSettingsAction) {
        match action {
            PluginSettingsAction::SetEnabled { enabled: true } => {
                self.plugins.settings.enable();
                self.settings_changed();
            }
            PluginSettingsAction::SetEnabled { enabled: false } => {
                self.plugins.settings.disable_all();
                self.settings_changed();
            }
            PluginSettingsAction::AddFolderDialog => self.plugins.folder_dialog = true,
            PluginSettingsAction::RemoveFolder(folder) => {
                self.plugins
                    .settings
                    .extra_paths
                    .retain(|p| p.to_string_lossy() != folder.as_str());
                self.settings_changed();
            }
            PluginSettingsAction::Scan(mode) => self.start_scan(rescan_mode(*mode)),
            PluginSettingsAction::CancelScan => {
                if let Some(job) = &self.plugins.job {
                    job.cancel();
                }
            }
            PluginSettingsAction::SetTimeout(secs) => {
                self.plugins.settings.set_timeout_secs(*secs);
                self.plugins.settings_changed = true;
            }
            PluginSettingsAction::SetSandbox(on) => {
                // Saved now, applied at the next start: this run's host is not touched.
                self.plugins.settings.sandbox = *on;
                self.plugins.settings_changed = true;
            }
            PluginSettingsAction::SetProcessMode(mode) => {
                self.plugins.settings.process = match mode {
                    PluginProcessMode::PerPlugin => atomik_plugin_host::ProcessMode::PerPlugin,
                    PluginProcessMode::PerFile => atomik_plugin_host::ProcessMode::PerFile,
                    PluginProcessMode::Shared => atomik_plugin_host::ProcessMode::Shared,
                };
                self.plugins.settings_changed = true;
                // Plugins loaded from now on follow it; running ones keep their process.
                self.apply_process_grouping();
            }
            PluginSettingsAction::SetEditorHome(home) => self.set_editor_home(match home {
                PluginEditorHome::OwnWindow => atomik_plugin_host::EditorHome::OwnWindow,
                PluginEditorHome::InApp => atomik_plugin_host::EditorHome::InApp,
            }),
            PluginSettingsAction::SetPluginDisabled { type_id, disabled } => {
                if *disabled {
                    self.plugins.settings.disabled.insert(type_id.clone());
                } else {
                    self.plugins.settings.disabled.remove(type_id);
                }
                self.settings_changed();
            }
            PluginSettingsAction::RetryPlugin(type_id) => {
                if self.plugins.settings.clear_crashed(type_id) {
                    self.plugins.settings_changed = true;
                }
            }
            PluginSettingsAction::RetryFile(path) => {
                if self.plugins.cache.unblacklist(Path::new(path)) {
                    self.start_scan(RescanMode::Incremental);
                }
            }
        }
    }

    /// The user's plugin settings changed: save them and rebuild the catalog.
    fn settings_changed(&mut self) {
        self.plugins.settings_changed = true;
        self.refresh_plugin_catalog();
    }

    /// Starts a scan job (one at a time) when plugins are on and a scanner is configured.
    pub fn start_scan(&mut self, mode: RescanMode) {
        if self.plugins.job.is_some() {
            return;
        }
        if !self.plugins.settings.enabled {
            self.scan_refused("enable third-party plugins (ENABLE switch) before scanning");
            return;
        }
        let Some(runner) = self.plugins.runner.clone() else {
            self.scan_refused(
                "no plugin scanner can be run: the app could not start a scanner process",
            );
            return;
        };
        let cache = std::mem::take(&mut self.plugins.cache);
        self.plugins.scan_note = None;
        let env = self.plugins.scan_env.clone();
        let (settings, formats, os) = (
            self.plugins.settings.clone(),
            atomik_plugin_host::ALL_FORMATS.to_vec(),
            atomik_plugin_host::paths::Os::current(),
        );
        self.plugins.job = Some(match env {
            Some(env) => ScanJob::start_with_env(settings, formats, os, mode, cache, runner, env),
            None => ScanJob::start(settings, formats, os, mode, cache, runner),
        });
    }

    /// Replaces the environment scans read the default folders from (`CLAP_PATH`, the OS folder
    /// variables). For tests: a lookup that finds nothing makes a scan visit only the folders
    /// the settings list.
    pub fn set_plugin_scan_env(&mut self, env: atomik_plugin_host::ScanEnv) {
        self.plugins.scan_env = Some(env);
    }

    /// A scan was asked for but cannot run: the reason is shown at once (status line) and stays
    /// in the settings view (last scan line) and the plugin log.
    fn scan_refused(&mut self, why: &str) {
        self.plugins.log(format!("scan: {why}"));
        self.plugins.scan_note = Some(why.to_string());
        self.reject(why.to_string());
    }

    /// True while a scan job runs.
    pub fn plugin_scan_running(&self) -> bool {
        self.plugins.job.is_some()
    }

    /// The summary of the last finished scan (or its error).
    pub fn plugin_scan_note(&self) -> Option<&str> {
        self.plugins.scan_note.as_deref()
    }

    /// Writes `plugins.json` when a live plugin taught the cache a layout, and tells the user
    /// when an earlier write failed. The learning of several layouts in one frame is one write.
    fn save_learned_cache(&mut self) {
        if std::mem::take(&mut self.plugins.cache_dirty) {
            self.queue_cache_save();
        }
        if let Some(error) = self
            .plugins
            .cache_writer
            .as_ref()
            .and_then(CacheWriter::take_error)
        {
            self.reject(error);
        }
    }

    /// Hands the scan cache to the writer thread, which serialises it and writes the file: the
    /// interface thread only makes the copy. Where no thread can be had the file is written here,
    /// as it always was.
    fn queue_cache_save(&mut self) {
        let Some(path) = self.plugins.paths.cache.clone() else {
            return;
        };
        if self.plugins.cache_writer.is_none() {
            self.plugins.cache_writer = CacheWriter::new();
        }
        match &self.plugins.cache_writer {
            Some(writer) => writer.submit(self.plugins.cache.clone(), path),
            None => {
                if let Err(e) = self.plugins.cache.save(&path) {
                    self.reject(format!("{}: {e}", path.display()));
                }
            }
        }
    }

    /// Waits for the scan cache writes that were handed over to reach the disk (tests, and the
    /// exit path, which waits a bounded time).
    pub fn wait_plugin_cache_writes(&mut self) {
        if let Some(writer) = &self.plugins.cache_writer {
            writer.wait(crate::plugin_cache_writer::EXIT_WAIT);
        }
    }

    fn poll_scan(&mut self) {
        if !self.plugins.job.as_ref().is_some_and(ScanJob::is_finished) {
            return;
        }
        let Some(job) = self.plugins.job.take() else {
            return;
        };
        let (cache, result) = job.finish();
        self.plugins.cache = cache;
        let note = match result {
            Ok(report) => {
                for line in scan_detail_lines(&report) {
                    self.plugins.log(line);
                }
                scan_summary(&report)
            }
            Err(e) => format!("scan failed: {e}"),
        };
        self.plugins.log(note.clone());
        self.plugins.scan_note = Some(note);
        // The scan's own result is the file's content now: a layout learned before it ended
        // is in this cache too, so the pending write is not needed.
        self.plugins.cache_dirty = false;
        self.queue_cache_save();
        self.refresh_plugin_catalog();
    }

    /// Waits for a running scan and applies it, and for the cache file to be written (tests and
    /// scripts).
    pub fn wait_scan(&mut self) {
        while self.plugins.job.is_some() {
            if self.plugins.job.as_ref().is_some_and(ScanJob::is_finished) {
                self.poll_scan();
            } else {
                std::thread::yield_now();
            }
        }
        self.wait_plugin_cache_writes();
    }

    // ------------------------------------------------------------------ banners

    /// Called after a project opened: shows the "uses N plugins" banner when plugins are off or
    /// some are missing (the document opened with stand-ins).
    pub(super) fn plugins_after_open(&mut self) {
        self.feed_plugin_views();
        let devices = self.doc().plugin_devices();
        if devices.is_empty() {
            if matches!(
                self.plugins.banner,
                Some(BannerKind::ProjectUsesPlugins { .. })
            ) {
                self.plugins.hide_banner();
            }
            return;
        }
        let missing = devices
            .iter()
            .filter_map(|(_, d)| d.plugin.as_ref())
            .filter(|p| !self.plugin_usable(&p.plugin_ref()))
            .count();
        if !self.plugins.settings.enabled || missing > 0 {
            let count = devices.len();
            self.plugins
                .show_banner(BannerKind::ProjectUsesPlugins { count });
        } else if matches!(
            self.plugins.banner,
            Some(BannerKind::ProjectUsesPlugins { .. })
        ) {
            self.plugins.hide_banner();
        }
    }

    /// The banner to show now, once per change (`Some(None)` hides it).
    pub fn take_plugin_banner_change(&mut self) -> Option<Option<BannerKind>> {
        std::mem::take(&mut self.plugins.banner_changed).then(|| self.plugins.banner.clone())
    }

    fn plugin_banner_action(&mut self, action: &PluginBannerAction) {
        match action {
            PluginBannerAction::Load => {
                self.plugins.settings.enable();
                self.plugins.hide_banner();
                self.settings_changed();
            }
            PluginBannerAction::OpenWithout => self.plugins.hide_banner(),
            PluginBannerAction::DisableCrashed => {
                if let Some(crashed) = self.plugins.previous_crash.take() {
                    blacklist_in_use(&mut self.plugins.cache, &crashed);
                    self.queue_cache_save();
                    self.refresh_plugin_catalog();
                }
                self.plugins.hide_banner();
            }
            PluginBannerAction::KeepCrashed => {
                self.plugins.previous_crash = None;
                self.plugins.hide_banner();
            }
            PluginBannerAction::OpenRecovery => {
                if let Some(path) = self.plugins.recovery.as_ref().and_then(|r| r.dir.newest()) {
                    if let Err(e) = self.open_recovery(&path) {
                        self.reject(e.to_string());
                    }
                } else {
                    self.reject("no recovery snapshot found".to_string());
                }
            }
            // The internal-error notice belongs to the shell: it saves, and hides its own strip.
            PluginBannerAction::SaveProject | PluginBannerAction::DismissError => {}
        }
    }

    /// Opens a recovery snapshot as an unsaved project (its name is not the user's file; saving
    /// asks for a name).
    pub fn open_recovery(&mut self, path: &Path) -> Result<(), super::AppError> {
        let text =
            atomik_model::read_document_text(path).map_err(|source| super::AppError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        // The audio the snapshot was taken with: the bundle it noted is copied into the new
        // session's own bundle, like a template's (the snapshot never owns a bundle itself).
        self.audio.adopt_from = RecoveryDir::bundle_of(path);
        let opened = self.open_str(&text, None);
        // `audio_opened` takes it; an open that failed before that must not leave it behind.
        self.audio.adopt_from = None;
        opened?;
        // Not clean: the user has not saved it anywhere.
        self.set_saved(atomik_model::empty_doc());
        Ok(())
    }

    // ------------------------------------------------------------------ recovery snapshots

    /// Starts a periodic snapshot: every loaded plugin is asked for its state in the background
    /// and the snapshot is written when they have all arrived or after [`SNAPSHOT_GRACE_SECS`],
    /// with the states stored by then (see [`Self::finish_snapshot_wait`]). A project with no
    /// plugin to ask is written at once.
    fn begin_periodic_snapshot(&mut self, now: f64) {
        let keys: Vec<DeviceKey> = self.plugins.instances.keys().cloned().collect();
        let mut asked = false;
        for key in keys {
            // A plugin flagged dirty is captured as the change it is, as the quiet period would.
            let undoable = self
                .plugins
                .instances
                .get(&key)
                .is_some_and(|id| self.plugins.dirty.remove(id).is_some());
            asked |= self.request_capture_for(&key, undoable);
        }
        if !asked {
            self.write_recovery_snapshot(now);
            return;
        }
        self.plugins.snapshot_wait = Some(now);
        self.plugins_poll_captures();
        self.finish_snapshot_wait(now);
    }

    /// Writes the snapshot a periodic one waits for once no capture is out any more, or the
    /// grace has passed.
    fn finish_snapshot_wait(&mut self, now: f64) {
        let Some(since) = self.plugins.snapshot_wait else {
            return;
        };
        if self.plugins.capturing.is_empty() || now - since >= SNAPSHOT_GRACE_SECS {
            self.plugins.snapshot_wait = None;
            self.write_recovery_snapshot(now);
        }
    }

    /// Writes a snapshot of the document as it is, with the plugin states stored so far: it never
    /// asks a plugin (see [`Self::begin_periodic_snapshot`]), so a plugin that flags its state
    /// dirty again after each capture cannot make the app capture in a loop.
    fn write_recovery_snapshot(&mut self, now: f64) {
        let revision = self.document.revision();
        let name = self.file.as_ref().and_then(|p| p.file_stem()).map_or_else(
            || "untitled".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        // The frame only clones the document; the writer thread serialises and writes it.
        let doc: RackDoc = self.document.doc().clone();
        let bundle = self.bundle_root().map(Path::to_path_buf);
        let Some(recovery) = self.plugins.recovery.as_mut() else {
            return;
        };
        recovery.last = Some(now);
        recovery.wanted = false;
        recovery.revision = Some(revision);
        recovery.sequence = unix_seconds().max(recovery.sequence + 1);
        if let Some(writer) = &recovery.writer {
            writer.submit(
                doc,
                recovery.dir.clone(),
                name,
                recovery.sequence,
                RECOVERY_KEEP,
                bundle,
            );
        }
    }

    /// Asks for a recovery snapshot of the document at the next frame (after an internal error:
    /// the periodic one may be minutes away). The writer thread does the work.
    pub(crate) fn request_recovery_snapshot(&mut self) {
        if let Some(r) = &mut self.plugins.recovery {
            r.wanted = true;
        }
    }

    /// Waits for the queued snapshots to reach the disk (tests, and the exit path).
    pub fn wait_recovery_writes(&mut self) {
        if let Some(writer) = self
            .plugins
            .recovery
            .as_ref()
            .and_then(|r| r.writer.as_ref())
        {
            writer.wait();
        }
    }

    /// Waits up to `limit` for the queued snapshots to reach the disk: true when they have (or
    /// there is no writer), false when the writer is still busy at the limit.
    pub fn wait_recovery_writes_for(&mut self, limit: std::time::Duration) -> bool {
        self.plugins
            .recovery
            .as_ref()
            .and_then(|r| r.writer.as_ref())
            .is_none_or(|writer| writer.wait_for(limit))
    }

    /// Sets the autosave period in minutes (0 turns the periodic snapshot of a changed project
    /// off; a snapshot after a plugin's state changed is still written).
    pub fn set_autosave_minutes(&mut self, minutes: u32) {
        self.plugins.autosave_secs = f64::from(minutes) * 60.0;
    }

    /// The recovery folder, when the app has one (where the session markers live too).
    pub(crate) fn recovery_folder(&self) -> Option<PathBuf> {
        self.plugins
            .recovery
            .as_ref()
            .map(|r| r.dir.path().to_path_buf())
    }

    /// The files the diagnostics report summarises: the plugin scan cache and the plugin journal.
    pub fn plugin_state_files(&self) -> (Option<PathBuf>, Option<PathBuf>) {
        (
            self.plugins.paths.cache.clone(),
            self.plugins.paths.journal.clone(),
        )
    }

    /// The folder holding the session scratch bundles to scan for unfinished recordings.
    pub(super) fn scratch_scan_folder(&self) -> Option<PathBuf> {
        self.audio_scratch_base()
            .or_else(|| self.plugins.paths.scratch.clone())
    }

    // ------------------------------------------------------------------ editor windows

    /// The window requests for the platform layer, in order.
    pub fn take_plugin_window_requests(&mut self) -> Vec<PluginWindowRequest> {
        std::mem::take(&mut self.plugins.window_requests)
    }

    /// OPEN GUI: asks the platform for a window; the editor is embedded when it answers with
    /// [`Self::plugin_window_created`].
    pub fn open_plugin_gui(&mut self, device: &DeviceKey) {
        let Some(id) = self.plugins.instances.get(device).copied() else {
            self.reject("the plugin is not loaded".to_string());
            return;
        };
        if !self.plugins.host.manager_ref().has_gui(id) {
            self.reject("this plugin has no editor window".to_string());
            return;
        }
        if self.plugins.host.manager_ref().editor_kind(id) == Some(EditorKind::External) {
            if self.plugin_editor_docked(device)
                || (self.editor_placement(device) == EditorPlacement::Docked
                    && !self.plugins.windows.contains_key(device))
            {
                self.open_docked_editor(device);
            } else {
                self.open_external_editor(device, id);
            }
            return;
        }
        if self.plugins.windows.contains_key(device) {
            self.plugins
                .window_requests
                .push(PluginWindowRequest::Show {
                    device: device.clone(),
                });
            return;
        }
        let Some(d) = self.doc().resolve(device) else {
            return;
        };
        let geom = d
            .plugin
            .as_ref()
            .and_then(|p| self.plugins.geoms.get(&p.type_id()))
            .copied();
        let size = self
            .plugins
            .remembered
            .get(device)
            .copied()
            .or(geom.map(|g| (g.w, g.h)));
        let at = geom.map(|g| (g.x, g.y));
        let title = format!(
            "{} - {}",
            d.plugin.as_ref().map_or("plugin", |p| p.name()),
            d.name
        );
        if let Some(s) = size {
            self.plugins.remembered.insert(device.clone(), s);
        }
        self.plugins
            .windows
            .insert(device.clone(), EditorWindow::Opening);
        self.plugins
            .window_requests
            .push(PluginWindowRequest::Open {
                device: device.clone(),
                title,
                remembered: size,
                at,
            });
    }

    /// OPEN GUI of an external editor: the plugin's own process shows its window, owned by the
    /// app window; asking again raises it. No host window is involved.
    ///
    /// The plugin may take minutes to answer (an online licence check, its own sign-in
    /// dialogs): the request is started and [`ManagerEvent::EditorOpened`] or
    /// [`ManagerEvent::EditorFailed`] finishes it, while the app stays responsive.
    fn open_external_editor(&mut self, device: &DeviceKey, id: InstanceId) {
        if self.plugins.windows.get(device) == Some(&EditorWindow::ExternalOpening) {
            self.reject(EDITOR_WAITING.to_string());
            return;
        }
        let owner = self.plugins.editor_owner;
        let process = self.plugins.host.manager_ref().native_handle(id);
        let again = self.plugins.windows.contains_key(device);
        log::info!("{}", editor_requested_line(&device.0, id, process, again));
        match self
            .plugins
            .host
            .manager()
            .begin_open_external_editor(id, owner)
        {
            Ok(true) => {
                self.plugins
                    .windows
                    .insert(device.clone(), EditorWindow::External);
                log::info!("{}", editor_opened_line(&device.0, id));
            }
            Ok(false) => {
                self.plugins
                    .windows
                    .insert(device.clone(), EditorWindow::ExternalOpening);
                self.reject(EDITOR_WAITING.to_string());
            }
            Err(e) => {
                log::info!("{}", editor_failed_line(&device.0, id, &e.to_string()));
                self.reject(format!("cannot open the plugin editor: {e}"));
            }
        }
    }

    /// The app window's native handle, the owner of the editors that live in the plugin's own
    /// process (the platform layer sets it once its window exists).
    pub fn set_plugin_editor_owner(&mut self, owner: Option<NativeParent>) {
        self.plugins.editor_owner = owner;
    }

    /// The owner handle of external editors.
    pub fn plugin_editor_owner(&self) -> Option<NativeParent> {
        self.plugins.editor_owner
    }

    /// The platform made the window: embeds the plugin's view in it and asks for its size.
    pub fn plugin_window_created(&mut self, device: &DeviceKey, info: PluginWindowInfo) {
        if self.plugins.windows.get(device) != Some(&EditorWindow::Opening) {
            return;
        }
        let Some(id) = self.plugins.instances.get(device).copied() else {
            self.plugins.windows.remove(device);
            self.plugins
                .window_requests
                .push(PluginWindowRequest::Close {
                    device: device.clone(),
                });
            return;
        };
        match self
            .plugins
            .host
            .manager()
            .open_gui(id, info.parent, info.scale)
        {
            Ok((w, h)) => {
                self.plugins
                    .windows
                    .insert(device.clone(), EditorWindow::Open);
                let (w, h) = self
                    .plugins
                    .remembered
                    .get(device)
                    .copied()
                    .unwrap_or((w, h));
                self.plugins
                    .window_requests
                    .push(PluginWindowRequest::Resize {
                        device: device.clone(),
                        w,
                        h,
                    });
                self.plugins
                    .window_requests
                    .push(PluginWindowRequest::Show {
                        device: device.clone(),
                    });
            }
            Err(e) => {
                self.plugins.windows.remove(device);
                self.plugins
                    .window_requests
                    .push(PluginWindowRequest::Close {
                        device: device.clone(),
                    });
                self.reject(format!("cannot open the plugin editor: {e}"));
            }
        }
    }

    /// The platform could not make the window.
    pub fn plugin_window_failed(&mut self, device: &DeviceKey, reason: &str) {
        self.plugins.windows.remove(device);
        self.reject(format!("cannot open the plugin editor window: {reason}"));
    }

    /// The user moved the window: `x` and `y` are the outer frame's top-left corner in physical
    /// pixels. It is kept for the next window of this plugin type.
    pub fn plugin_window_moved(&mut self, device: &DeviceKey, x: i32, y: i32) {
        if self.plugins.windows.get(device) == Some(&EditorWindow::Open) {
            self.plugins.window_pos.insert(device.clone(), (x, y));
        }
    }

    /// Writes the position and size of `device`'s window into the geometry of its plugin type.
    fn commit_window_geometry(&mut self, device: &DeviceKey) {
        let (Some((x, y)), Some((w, h))) = (
            self.plugins.window_pos.get(device).copied(),
            self.plugins.remembered.get(device).copied(),
        ) else {
            return;
        };
        let Some(type_id) = self
            .doc()
            .resolve(device)
            .and_then(|d| d.plugin.as_ref())
            .map(|p| p.type_id())
        else {
            return;
        };
        let geom = PluginWindowGeom { x, y, w, h };
        if self.plugins.geoms.get(&type_id) != Some(&geom) {
            self.plugins.geoms.insert(type_id, geom);
            self.plugins.geoms_changed = true;
        }
    }

    /// Where each plugin type's editor window was last, the open windows included (what
    /// `ui-state.json` keeps).
    pub fn plugin_window_geoms(&mut self) -> &BTreeMap<String, PluginWindowGeom> {
        let open: Vec<DeviceKey> = self.plugins.window_pos.keys().cloned().collect();
        for device in open {
            self.commit_window_geometry(&device);
        }
        &self.plugins.geoms
    }

    /// Restores the geometries read from `ui-state.json`.
    pub fn set_plugin_window_geoms(&mut self, geoms: BTreeMap<String, PluginWindowGeom>) {
        self.plugins.geoms = geoms;
        self.plugins.geoms_changed = false;
    }

    /// True once (and clears) when a window closed with a geometry other than the one kept.
    pub fn take_plugin_geoms_changed(&mut self) -> bool {
        std::mem::take(&mut self.plugins.geoms_changed)
    }

    /// The user resized the window: the plugin may adjust the size; the adjusted size is asked
    /// of the platform when it differs.
    pub fn plugin_window_resized(&mut self, device: &DeviceKey, w: u32, h: u32) {
        if self.plugins.windows.get(device) != Some(&EditorWindow::Open) {
            return;
        }
        let Some(id) = self.plugins.instances.get(device).copied() else {
            return;
        };
        match self.plugins.host.manager().request_resize_gui(id, w, h) {
            Ok(Some(took)) => self.plugin_window_sized(device, (w, h), Some(took)),
            // The plugin is busy: the size it takes comes later, see `plugin_gui_sized`.
            Ok(None) => {}
            Err(_) => self.plugin_window_sized(device, (w, h), None),
        }
    }

    /// The editor of `device` answered the resize to `asked` with the size it `took` (`None`: it
    /// cannot be resized): the window follows when the plugin adjusted the size.
    pub(super) fn plugin_window_sized(
        &mut self,
        device: &DeviceKey,
        asked: (u32, u32),
        took: Option<(u32, u32)>,
    ) {
        let Some((aw, ah)) = took else {
            self.plugins.remembered.insert(device.clone(), asked);
            return;
        };
        self.plugins.remembered.insert(device.clone(), (aw, ah));
        if (aw, ah) != asked {
            self.plugins
                .window_requests
                .push(PluginWindowRequest::Resize {
                    device: device.clone(),
                    w: aw,
                    h: ah,
                });
        }
    }

    /// The user closed the window (or the plugin closed its editor): the view is destroyed, the
    /// state it may have changed is captured, the window goes away.
    pub fn plugin_window_closed(&mut self, device: &DeviceKey) {
        self.close_plugin_gui_inner(device, false);
    }

    /// Closes the editor of `device` and asks the platform to destroy its window.
    pub fn close_plugin_gui(&mut self, device: &DeviceKey) {
        self.close_plugin_gui_inner(device, true);
    }

    fn close_plugin_gui_inner(&mut self, device: &DeviceKey, destroy_window: bool) {
        let Some(window) = self.plugins.windows.remove(device) else {
            return;
        };
        self.plugin_learn_window_closed(device);
        self.plugin_dock_closed(device);
        self.plugins.frames.remove(device);
        self.commit_window_geometry(device);
        self.plugins.window_pos.remove(device);
        // An external editor is a window of the plugin's process: nothing to destroy here.
        let destroy_window = destroy_window
            && !matches!(
                window,
                EditorWindow::External | EditorWindow::ExternalOpening
            );
        if matches!(
            window,
            EditorWindow::External | EditorWindow::ExternalOpening
        ) {
            log::info!(
                "{}",
                editor_closed_line(&device.0, self.plugins.instances.get(device).copied())
            );
        }
        if let Some(id) = self.plugins.instances.get(device).copied()
            && self.plugins.host.manager().close_editor(id).is_ok()
        {
            // The user closing the window ends a patch change made in it: its own undo step
            // when the state arrives. A close the app asked for (a new project, a rebuild) only
            // keeps the state.
            self.request_capture_for(device, !destroy_window);
            // A plugin in this process has answered already.
            self.plugins_poll_captures();
        }
        if destroy_window {
            self.plugins
                .window_requests
                .push(PluginWindowRequest::Close {
                    device: device.clone(),
                });
        }
    }

    pub(super) fn close_plugin_window(&mut self, device: &DeviceKey) {
        self.close_plugin_gui_inner(device, true);
    }

    fn close_all_plugin_windows(&mut self) {
        let open: Vec<DeviceKey> = self.plugins.windows.keys().cloned().collect();
        for key in open {
            self.close_plugin_gui_inner(&key, true);
        }
    }

    /// The editors the plugins' own processes show, with the outer frame of each window
    /// (x, y, width, height in physical pixels) as the process last said: the platform docks
    /// their Learn strips there.
    pub fn external_editor_frames(&self) -> Vec<(DeviceKey, (i32, i32, u32, u32))> {
        self.plugins
            .frames
            .iter()
            .filter(|(d, _)| {
                self.plugins.windows.get(*d) == Some(&EditorWindow::External)
                    && !self.plugin_editor_docked(d)
            })
            .map(|(d, f)| (d.clone(), *f))
            .collect()
    }

    /// True when the editor window of `device` is open or opening.
    pub fn plugin_gui_open(&self, device: &DeviceKey) -> bool {
        self.plugins.windows.contains_key(device)
    }

    /// True while the plugin's own process is still opening its editor (see
    /// [`ManagerEvent::EditorOpened`]).
    pub fn plugin_gui_opening(&self, device: &DeviceKey) -> bool {
        self.plugins.windows.get(device) == Some(&EditorWindow::ExternalOpening)
    }

    // ------------------------------------------------------------------ shutdown

    /// Clean exit: closes editors, captures nothing more, removes the crash journal. The platform
    /// layer calls it after the engine was shut down.
    pub fn shutdown_plugins(&mut self) {
        self.close_all_plugin_windows();
        // A stalled disk must not hang the exit: the wait for the last snapshot is bounded.
        if !self.wait_recovery_writes_for(crate::recovery::EXIT_WAIT) {
            log::warn!("a recovery snapshot is still being written at exit; not waiting for it");
        }
        // A layout learned in the last frames is still written; the wait is bounded.
        self.save_learned_cache();
        self.wait_plugin_cache_writes();
        self.plugins_poll_events();
        if let Some(journal) = &mut self.plugins.journal {
            let _ = journal.clean_exit();
        }
        // The last thing: a run that ends before this point leaves its marker behind.
        if let Some(marker) = self
            .plugins
            .recovery
            .as_mut()
            .and_then(|r| r.marker.as_mut())
        {
            marker.clean_exit();
        }
    }

    /// Plugin instances alive in the manager (tests of the leak and restart rules).
    pub fn plugin_instance_count(&self) -> usize {
        self.plugins.host.manager_ref().instance_count()
    }

    /// The live instance behind a plugin device.
    pub fn plugin_instance(&self, device: &DeviceKey) -> Option<InstanceId> {
        self.plugins.instances.get(device).copied()
    }

    /// True while the instance behind `device` is parked and its device is not in the engine
    /// (audio is off, or the plugin is being given a device of its new layout).
    pub fn plugin_parked(&self, device: &DeviceKey) -> bool {
        self.plugins.offline.contains(device)
    }

    /// The descriptor the engine device of `device` was built with.
    pub fn plugin_built_descriptor(&self, device: &DeviceKey) -> Option<&'static DeviceDescriptor> {
        self.plugins.live_desc.get(device).copied()
    }

    /// How many scan cache writes the writer thread has finished.
    pub fn plugin_cache_writes(&self) -> u64 {
        self.plugins
            .cache_writer
            .as_ref()
            .map_or(0, CacheWriter::written)
    }

    /// True while a preset is being loaded into the plugin behind `device`.
    pub fn plugin_preset_loading(&self, device: &DeviceKey) -> bool {
        self.plugins.preset_loads.contains_key(device)
    }

    /// Why a plugin device is FAILED, if it is.
    pub fn plugin_failure(&self, device: &DeviceKey) -> Option<&str> {
        self.plugins.failed.get(device).map(String::as_str)
    }

    /// Why a plugin device is CRASHED (its process ended), if it is.
    pub fn plugin_crash(&self, device: &DeviceKey) -> Option<&str> {
        self.plugins.crashed.get(device).map(String::as_str)
    }

    /// What the unclean-exit and project banners should show now.
    pub fn plugin_banner(&self) -> Option<&BannerKind> {
        self.plugins.banner.as_ref()
    }

    /// The bounded plugin log, oldest first.
    pub fn plugin_log(&self) -> Vec<String> {
        self.plugins.log.iter().cloned().collect()
    }

    /// Records a sync op's plugin part, used by `sync_ops`.
    pub(super) fn plugins_sync_op(&mut self, op: &SyncOp) {
        if let SyncOp::PluginChanged { device } = op {
            self.plugin_changed(device);
        }
    }
}

#[path = "app_core_plugin_manager.rs"]
mod manager;

/// Most failed files named one by one in the plugin log after a scan (the FAILED FILES list has
/// them all).
const MAX_FAILURE_LINES: usize = 20;

/// The log lines of a finished scan that come before its summary: one line per failed file with
/// the reason, the VST2 hints and a scanner that could not be started.
fn scan_detail_lines(report: &ScanReport) -> Vec<String> {
    let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut lines = Vec::new();
    for (path, reason) in report.failures.iter().take(MAX_FAILURE_LINES) {
        let name = path.file_name().unwrap_or(path.as_os_str());
        lines.push(format!(
            "failed: {}: {}",
            name.to_string_lossy(),
            one_line(reason)
        ));
    }
    if report.failures.len() > MAX_FAILURE_LINES {
        lines.push(format!(
            "failed: and {} more files (see FAILED FILES)",
            report.failures.len() - MAX_FAILURE_LINES
        ));
    }
    for folder in &report.vst2_folders {
        lines.push(format!(
            "{} holds only VST2 plugins; VST2 is not supported (use their VST3 or CLAP versions)",
            folder.display()
        ));
    }
    if let Some(e) = &report.spawn_error {
        lines.push(format!("the scanner could not be started: {}", one_line(e)));
    }
    lines
}

fn scan_summary(report: &ScanReport) -> String {
    let searched = report.scanned + report.cached + report.skipped_blacklisted;
    format!(
        "scan: {} scanned, {} cached, {} ok, {} failed{}{}",
        report.scanned,
        report.cached,
        report.ok,
        report.failed,
        if report.cancelled { " (cancelled)" } else { "" },
        if searched == 0 && report.vst2_folders.is_empty() && report.spawn_error.is_none() {
            " (no .clap or .vst3 files in the search folders)"
        } else {
            ""
        }
    )
}
