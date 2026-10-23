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
//! the value back. A change outside a gesture is dropped (a patch change inside the plugin is
//! not undoable and not stored).
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
//! [`PluginConfig::previous_crash`] shows the unclean-exit banner. While any plugin is loaded
//! [`AppCore::plugin_tick`] writes a recovery snapshot of the project every 60 s and after each
//! state capture, to `<config>/recovery`, keeping the last three, never over the user's file.

use super::AppCore;
use crate::sink::EngineSink;
use atomik_device_sdk::{Device, DeviceDescriptor, PluginRef, RunMode};
use atomik_engine::DeviceInstanceId;
use atomik_model::{
    Command, DeviceKey, Document, ExpandedDevice, ModelError, PluginCommand, PluginDoc, RackDoc,
    SyncOp, register_stand_ins, save_project,
};
use atomik_plugin_host::ScanReport;
use atomik_plugin_host::{
    CrashJournal, EditorKind, InUsePlugin, InstanceId, InstantiateRequest, LOST, ManagerEvent,
    NativeParent, ParamPhase, PluginCache, PluginCatalog, PluginHandle, PluginSettings,
    PluginStatus, RecoveryDir, RescanMode, ScanJob, ScanRunner, blacklist_in_use,
};
use atomik_render::plugins::{PluginHost, PluginIsolation};
use atomik_ui::plugin_data::{
    FailedFileInfo, PluginBannerAction, PluginDeviceStatus, PluginEntryInfo, PluginEntryStatus,
    PluginRescan, PluginSettingsAction, PluginSettingsData, ScanProgressInfo, SettingsPluginRow,
};
use atomik_ui::{BannerKind, PluginGestureMerger, PluginParamsView, UiAction};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Seconds between recovery snapshots while a plugin is loaded.
pub const RECOVERY_PERIOD_SECS: f64 = 60.0;
/// How many recovery snapshots are kept.
pub const RECOVERY_KEEP: usize = 3;
/// The quiet time after `state.mark_dirty` before the state is captured.
pub const STATE_QUIET_SECS: f64 = 1.0;
/// Lines of the bounded plugin log shown in Settings.
pub const LOG_CAPACITY: usize = 200;

/// The files the plugin machinery keeps in the config directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginPaths {
    /// `plugins.json`, the scan cache.
    pub cache: Option<PathBuf>,
    /// `plugins-in-use.json`, the crash journal.
    pub journal: Option<PathBuf>,
    /// `recovery/`, the recovery snapshots.
    pub recovery: Option<PathBuf>,
}

impl PluginPaths {
    /// The three files inside the config directory `dir`.
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            cache: Some(dir.join("plugins.json")),
            journal: Some(dir.join("plugins-in-use.json")),
            recovery: Some(dir.join("recovery")),
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
}

/// The recovery snapshot writer state.
#[derive(Debug)]
struct Recovery {
    dir: RecoveryDir,
    last: Option<f64>,
    wanted: bool,
    /// The sequence number of the last snapshot name (names only ever grow).
    sequence: u64,
    writers: Vec<std::thread::JoinHandle<()>>,
}

/// Plugin state of the app core.
pub(super) struct PluginState {
    pub(super) host: PluginHost,
    pub(super) settings: PluginSettings,
    pub(super) cache: PluginCache,
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
    /// How many instances the current batch of creations started with (for "3/8").
    batch_total: usize,
    batch_done: usize,
    /// Why a device is FAILED (an error, a restore that failed, a creation that failed).
    failed: BTreeMap<DeviceKey, String>,
    /// Why a device is CRASHED: its plugin process ended (sandbox).
    crashed: BTreeMap<DeviceKey, String>,
    /// Devices whose saved state failed to restore: the blob is kept, never captured over.
    no_capture: BTreeSet<DeviceKey>,
    /// The descriptor each live device was inserted with.
    live_desc: BTreeMap<DeviceKey, &'static DeviceDescriptor>,
    /// Devices whose instance was created while no engine ran: it lives parked (editor and
    /// params work) and is attached to the engine as soon as one exists.
    offline: BTreeSet<DeviceKey>,
    /// A platform loop that paints between frames asks to show LOADING one frame before the
    /// (blocking) creation of an instance starts.
    paint_first: bool,
    shown: BTreeMap<DeviceKey, PluginDeviceStatus>,
    catalog_dirty: bool,
    merger: PluginGestureMerger,
    open_gestures: BTreeSet<(DeviceKey, u32)>,
    from_plugin: bool,
    /// The next engine restart drops the instances instead of keeping them (a new document).
    discard: bool,
    /// Instances whose state the plugin flagged dirty, with the time of the last flag.
    dirty: BTreeMap<InstanceId, f64>,
    clock: f64,
    journal: Option<CrashJournal>,
    recovery: Option<Recovery>,
    previous_crash: Option<Vec<InUsePlugin>>,
    banner: Option<BannerKind>,
    banner_changed: bool,
    log: VecDeque<String>,
    windows: BTreeMap<DeviceKey, EditorWindow>,
    remembered: BTreeMap<DeviceKey, (u32, u32)>,
    window_requests: Vec<PluginWindowRequest>,
    /// The app window's native handle: the owner of external editors.
    editor_owner: Option<NativeParent>,
    settings_changed: bool,
    folder_dialog: bool,
}

impl PluginState {
    pub(super) fn new() -> Self {
        Self {
            host: PluginHost::new(),
            settings: PluginSettings::default(),
            cache: PluginCache::default(),
            paths: PluginPaths::default(),
            runner: None,
            job: None,
            scan_env: None,
            scan_note: None,
            instances: BTreeMap::new(),
            keys: BTreeMap::new(),
            loaded: BTreeMap::new(),
            pending: VecDeque::new(),
            batch_total: 0,
            batch_done: 0,
            failed: BTreeMap::new(),
            crashed: BTreeMap::new(),
            no_capture: BTreeSet::new(),
            live_desc: BTreeMap::new(),
            offline: BTreeSet::new(),
            paint_first: false,
            shown: BTreeMap::new(),
            catalog_dirty: true,
            merger: PluginGestureMerger::new(),
            open_gestures: BTreeSet::new(),
            from_plugin: false,
            discard: false,
            dirty: BTreeMap::new(),
            clock: 0.0,
            journal: None,
            recovery: None,
            previous_crash: None,
            banner: None,
            banner_changed: false,
            log: VecDeque::new(),
            windows: BTreeMap::new(),
            remembered: BTreeMap::new(),
            window_requests: Vec::new(),
            editor_owner: None,
            settings_changed: false,
            folder_dialog: false,
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

fn entry_infos(catalog: &PluginCatalog) -> Vec<PluginEntryInfo> {
    catalog
        .entries()
        .iter()
        .map(|e| PluginEntryInfo {
            plugin: e.plugin.clone(),
            name: e.name.clone(),
            vendor: e.vendor.clone(),
            version: e.version.clone(),
            kind: e.kind,
            status: entry_status(&e.status),
            path: Some(e.path.to_string_lossy().into_owned()),
            layout: e.layout.clone(),
        })
        .collect()
}

/// Unix seconds, the sequence of recovery snapshot names (never used by the engine).
#[allow(clippy::disallowed_methods)]
// Justification: the wall clock only orders recovery file names; it never reaches audio.
fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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
        p.recovery = config.paths.recovery.as_ref().map(|dir| Recovery {
            dir: RecoveryDir::new(dir),
            last: None,
            wanted: false,
            sequence: 0,
            writers: Vec::new(),
        });
        p.paths = config.paths;
        p.runner = config.runner;
        p.previous_crash = config.previous_crash;
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
        }
        if !self.doc().plugin_devices().is_empty() {
            self.rebuild_engine();
        }
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
                let now = self
                    .registry
                    .get(&plugin.type_id())
                    .map(|f| f.descriptor as *const DeviceDescriptor);
                let then = self
                    .plugins
                    .live_desc
                    .get(&key)
                    .map(|d| *d as *const DeviceDescriptor);
                if now != then {
                    rebuild = true;
                }
            } else if installed && !self.plugins.pending.contains(&key) {
                self.plugins.failed.remove(&key);
                self.plugins.pending.push_back(key);
                self.plugins.batch_total += 1;
            }
        }
        if rebuild {
            self.rebuild_engine();
        }
    }

    /// True when `plugin` can be used now: plugins are on and the catalog lists it OK.
    fn plugin_usable(&self, plugin: &PluginRef) -> bool {
        self.plugins.settings.enabled && self.plugins.host.is_installed(plugin)
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
            let same = self
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
                self.plugins.offline.remove(key);
                match self.plugins.host.manager().reattach(id, sample_rate) {
                    Ok(handle) => {
                        self.adopt_descriptor(&type_id, handle.descriptor);
                        self.plugins
                            .live_desc
                            .insert(key.clone(), handle.descriptor);
                        return Some((Box::new(handle.device), handle.descriptor));
                    }
                    Err(e) => {
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
        if usable && !self.plugins.failed.contains_key(key) && !self.plugins.pending.contains(key) {
            self.plugins.pending.push_back(key.clone());
            self.plugins.batch_total += 1;
        }
        Some((standin, descriptor))
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
        }
        self.plugins.live_desc.remove(key);
        self.plugins.crashed.remove(key);
    }

    /// Called by `rebuild_engine` before the engine is reset: captures the states and keeps (or,
    /// for a new document, drops) the living instances, so an audio restart does not reload them.
    pub(super) fn plugins_before_reset(&mut self) {
        self.plugins_capture_all();
        let keep = !self.plugins.discard;
        let ids: Vec<InstanceId> = self.plugins.instances.values().copied().collect();
        for id in ids {
            self.plugins.host.manager().retain(id, keep);
        }
        self.plugins.pending.clear();
        self.plugins.batch_total = 0;
        self.plugins.batch_done = 0;
        self.plugins.shown.clear();
        if !keep {
            self.plugins.instances.clear();
            self.plugins.keys.clear();
            self.plugins.dirty.clear();
            self.plugins.failed.clear();
            self.plugins.crashed.clear();
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
        let wanted: BTreeSet<DeviceKey> = self
            .doc()
            .plugin_devices()
            .into_iter()
            .map(|(k, _)| k)
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
        if let Some(id) = parked {
            // A parked instance has no engine device to leave with: destroy it.
            self.plugins_poll_events();
            let _ = self.plugins.host.manager().remove(id);
            self.plugins_poll_events();
        }
        self.plugins.failed.remove(flat);
        self.plugins.crashed.remove(flat);
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
        self.instantiate_one();
        self.adopt_offline();
        self.poll_scan();
        self.answer_param_texts();
        self.feed_plugin_views();
    }

    /// Runs the clock-driven plugin work: the quiet-period state capture and the
    /// recovery snapshot. `now` is a monotonic time in seconds (the shell passes the
    /// frame clock).
    pub fn plugin_tick(&mut self, now: f64) {
        self.plugins.clock = now;
        let due: Vec<InstanceId> = self
            .plugins
            .dirty
            .iter()
            .filter(|(_, at)| now - **at >= STATE_QUIET_SECS)
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            self.plugins.dirty.remove(&id);
            if let Some(key) = self.plugins.keys.get(&id).cloned() {
                self.capture_instance(&key);
            }
        }
        let loaded = !self.plugins.instances.is_empty();
        let write = match &self.plugins.recovery {
            Some(r) if loaded => {
                r.wanted || r.last.is_none_or(|last| now - last >= RECOVERY_PERIOD_SECS)
            }
            _ => false,
        };
        if write {
            self.write_recovery_snapshot(now);
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

    /// True when no instance is waiting to be created.
    pub fn plugins_ready(&self) -> bool {
        self.plugins.pending.is_empty()
    }

    /// Runs frames until every pending instance is created (tests and scripts).
    pub fn wait_plugins(&mut self) {
        for _ in 0..10_000 {
            if self.plugins.pending.is_empty() {
                break;
            }
            self.frame();
        }
        // One more poll lets restarts and the reaper settle; the views show the result.
        self.plugins_poll_events();
        self.feed_plugin_views();
    }

    fn plugins_poll_events(&mut self) {
        for event in self.plugins.host.poll() {
            self.plugin_event(event);
        }
    }

    fn plugin_event(&mut self, event: ManagerEvent) {
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
                if let Some(key) = self.plugins.keys.get(&instance).cloned() {
                    self.plugins.failed.insert(key, reason);
                }
            }
            ManagerEvent::Failed { instance, reason } => {
                // The device's own Failed that goes with a lost process says nothing more than
                // the Crashed event does.
                if self.plugins.host.manager_ref().is_lost(instance) {
                    return;
                }
                if let Some(key) = self.plugins.keys.get(&instance).cloned() {
                    self.plugins.log(format!("[{key}] failed: {reason}"));
                    self.plugins.failed.insert(key, reason);
                }
            }
            ManagerEvent::Crashed { instance, reason } => {
                if let Some(key) = self.plugins.keys.get(&instance).cloned() {
                    self.plugins.log(format!("[{key}] crashed: {reason}"));
                    self.plugins.failed.remove(&key);
                    self.plugins.crashed.insert(key, reason);
                }
            }
            ManagerEvent::LayoutChanged { instance, layout } => {
                self.plugin_layout_changed(instance, layout);
            }
            ManagerEvent::GuiResize { instance, w, h } => {
                if let Some(key) = self.plugins.keys.get(&instance).cloned()
                    && self.plugins.windows.contains_key(&key)
                {
                    self.plugins
                        .window_requests
                        .push(PluginWindowRequest::Resize { device: key, w, h });
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
                self.plugins.log(format!("[{name}] {level:?}: {text}"));
            }
            ManagerEvent::Destroyed(id) => {
                if let Some(info) = self.plugins.loaded.remove(&id)
                    && !self.plugins.host.is_sandboxed()
                    && let Some(journal) = &mut self.plugins.journal
                {
                    let _ = journal.unloaded(&info);
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

    /// Creates one pending instance and swaps its device in (one per frame).
    fn instantiate_one(&mut self) {
        if self.plugins.paint_first
            && let Some(front) = self.plugins.pending.front()
            && !self.plugins.shown.get(front).is_some_and(|s| s.loading)
        {
            // LOADING is fed (and painted) before the creation blocks the thread.
            return;
        }
        while let Some(key) = self.plugins.pending.pop_front() {
            let Some((plugin, request)) = self.instantiate_request(&key) else {
                continue;
            };
            if !self.plugin_usable(&plugin) {
                continue;
            }
            self.plugins.batch_done += 1;
            if self.plugins.pending.is_empty() {
                self.plugins.batch_total = 0;
                self.plugins.batch_done = 0;
            }
            #[allow(clippy::disallowed_methods)]
            // Justification: UI-thread load time for the log, not engine behaviour.
            let began = std::time::Instant::now();
            match self.plugins.host.manager().instantiate(&request) {
                Ok(handle) => {
                    let took = began.elapsed();
                    self.attach_instance(&key, &plugin, handle);
                    self.plugins
                        .log(format!("[{key}] created in {:.1} s", took.as_secs_f64()));
                }
                Err(e) => {
                    let message = e.to_string();
                    self.plugins.log(format!("[{key}] cannot load: {message}"));
                    self.plugins.failed.insert(key, message);
                }
            }
            return;
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
        let PluginHandle {
            id,
            device,
            layout,
            descriptor,
            restore_error,
        } = handle;
        self.plugins.instances.insert(key.clone(), id);
        self.plugins.keys.insert(id, key.clone());
        self.adopt_descriptor(&type_id, descriptor);
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
    }

    /// Attaches the instances created while no engine ran, now that one exists.
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
            match self.plugins.host.manager().reattach(id, sample_rate) {
                Ok(handle) => {
                    self.plugins.offline.remove(&key);
                    self.plugins
                        .live_desc
                        .insert(key.clone(), handle.descriptor);
                    self.swap_in_device(&key, Box::new(handle.device));
                }
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
        let Some(id) = self.plugins.instances.get(key).copied() else {
            return false;
        };
        if self.plugins.no_capture.contains(key) {
            return false;
        }
        let blob = match self.plugins.host.manager().capture_state(id) {
            Ok(blob) => blob,
            Err(e) => {
                self.plugins
                    .log(format!("[{key}] cannot capture state: {e}"));
                return false;
            }
        };
        let changed = self
            .doc()
            .resolve(key)
            .and_then(|d| d.plugin.as_ref())
            .is_none_or(|p| p.state.as_deref() != Some(&blob));
        if !changed {
            return true;
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

    /// Captures every live instance (before a save, a recovery snapshot, an engine restart).
    pub fn plugins_capture_all(&mut self) {
        let keys: Vec<DeviceKey> = self.plugins.instances.keys().cloned().collect();
        for key in keys {
            self.capture_instance(&key);
        }
        self.plugins.dirty.clear();
    }

    /// Captures the instances whose plugin flagged its state dirty (before a command or an undo
    /// that could remove a device, so the removal's undo brings the latest state back).
    pub(super) fn plugins_flush_dirty(&mut self) {
        if self.plugins.dirty.is_empty() {
            return;
        }
        let ids: Vec<InstanceId> = self.plugins.dirty.keys().copied().collect();
        self.plugins.dirty.clear();
        for id in ids {
            if let Some(key) = self.plugins.keys.get(&id).cloned() {
                self.capture_instance(&key);
            }
        }
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
                self.saved = base.doc().clone();
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
            if self.plugin_usable(&plugin) && !self.plugins.pending.contains(device) {
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
                self.plugins.open_gestures.insert((key.clone(), param));
                self.plugins.merger.begin(&key, param)
            }
            ParamPhase::Change => {
                if !self.plugins.open_gestures.contains(&(key.clone(), param)) {
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
                UiAction::EndMerge => self.document.end_merge(),
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

    /// The plugin's param table changed: register the new descriptor and restart the engine with
    /// the reattached instance, so the device is rebuilt keeping values by key.
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
        let _ = self.sync_silent(Command::Plugin(PluginCommand::SyncLayout {
            device: key,
            layout: layout.clone(),
        }));
        if let Ok(descriptor) = atomik_device_sdk::descriptor_for(&plugin, &layout) {
            self.adopt_descriptor(&plugin.type_id(), descriptor);
        }
        self.rebuild_engine();
    }

    // ------------------------------------------------------------------ actions

    /// Handles the plugin actions of the views. Returns false for an action that is not one.
    pub(super) fn plugin_action(&mut self, action: &UiAction) -> bool {
        match action {
            UiAction::OpenPluginGui { device } => self.open_plugin_gui(device),
            UiAction::OpenPluginParams { device } => self.open_plugin_params(device),
            UiAction::ClosePluginParams => self.close_plugin_params(),
            UiAction::ReloadPlugin { device } => self.reload_plugin(device),
            UiAction::AutomatePluginParam { device, param } => {
                self.automate_plugin_param(device, *param);
            }
            // The pane's text requests are answered every frame ([`Self::answer_param_texts`]).
            UiAction::PluginParamTexts { .. } => {}
            UiAction::PluginSettings(a) => self.plugin_settings_action(a),
            UiAction::PluginBanner(a) => self.plugin_banner_action(a),
            _ => return false,
        }
        true
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

    /// Queues the creation of an instance for `device` (one per frame), or says why not.
    fn queue_instance(&mut self, device: &DeviceKey, plugin: &PluginRef) {
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

    /// Answers the pane's display-text requests from the plugin (main thread, once per request).
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
                loading: false,
                waiting,
            },
            None if self.plugins.pending.contains(key) => PluginDeviceStatus {
                has_editor: false,
                missing: None,
                failed,
                crashed: None,
                loading: true,
                waiting: None,
            },
            // The plugin is installed but its instance could not be created: FAILED with the
            // reason (RELOAD tries again), not MISSING.
            None if failed.is_some() && self.plugin_usable(plugin) => PluginDeviceStatus {
                has_editor: false,
                missing: None,
                failed,
                crashed: None,
                loading: false,
                waiting: None,
            },
            None => PluginDeviceStatus {
                has_editor: false,
                missing: Some(self.missing_reason(plugin)),
                failed,
                crashed: None,
                loading: false,
                waiting: None,
            },
        }
    }

    /// Feeds the views what the app knows: the catalog, the blacklist and each plugin device's
    /// status (only what changed).
    fn feed_plugin_views(&mut self) {
        if self.plugins.catalog_dirty {
            self.plugins.catalog_dirty = false;
            let catalog = self.plugins.host.catalog();
            let infos = entry_infos(catalog);
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
                    status: entry_status(&e.status),
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
            PluginSettingsAction::SetPluginDisabled { type_id, disabled } => {
                if *disabled {
                    self.plugins.settings.disabled.insert(type_id.clone());
                } else {
                    self.plugins.settings.disabled.remove(type_id);
                }
                self.settings_changed();
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
        if let Some(path) = &self.plugins.paths.cache
            && let Err(e) = self.plugins.cache.save(path)
        {
            self.reject(format!("{}: {e}", path.display()));
        }
        self.refresh_plugin_catalog();
    }

    /// Waits for a running scan and applies it (tests and scripts).
    pub fn wait_scan(&mut self) {
        while self.plugins.job.is_some() {
            if self.plugins.job.as_ref().is_some_and(ScanJob::is_finished) {
                self.poll_scan();
            } else {
                std::thread::yield_now();
            }
        }
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
                    if let Some(path) = &self.plugins.paths.cache {
                        let _ = self.plugins.cache.save(path);
                    }
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
        }
    }

    /// Opens a recovery snapshot as an unsaved project (its name is not the user's file; saving
    /// asks for a name).
    pub fn open_recovery(&mut self, path: &Path) -> Result<(), super::AppError> {
        let text = std::fs::read_to_string(path).map_err(|source| super::AppError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        self.open_str(&text, None)?;
        // Not clean: the user has not saved it anywhere.
        self.saved = atomik_model::empty_doc();
        Ok(())
    }

    // ------------------------------------------------------------------ recovery snapshots

    fn write_recovery_snapshot(&mut self, now: f64) {
        self.plugins_capture_all();
        let Some(recovery) = self.plugins.recovery.as_mut() else {
            return;
        };
        recovery.last = Some(now);
        recovery.wanted = false;
        recovery.writers.retain(|h| !h.is_finished());
        let name = self.file.as_ref().and_then(|p| p.file_stem()).map_or_else(
            || "untitled".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        recovery.sequence = unix_seconds().max(recovery.sequence + 1);
        let path = recovery.dir.next_path(&name, recovery.sequence);
        let dir = recovery.dir.clone();
        let mut doc: RackDoc = self.document.doc().clone();
        doc.prune_unreferenced_assets();
        let text = save_project(&doc);
        let handle = std::thread::spawn(move || {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut tmp = path.as_os_str().to_owned();
            tmp.push(".tmp");
            let tmp = PathBuf::from(tmp);
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
            let _ = dir.prune(RECOVERY_KEEP);
        });
        if let Some(r) = self.plugins.recovery.as_mut() {
            r.writers.push(handle);
        }
    }

    /// Waits for the snapshot writers (tests, and the exit path).
    pub fn wait_recovery_writes(&mut self) {
        if let Some(r) = self.plugins.recovery.as_mut() {
            for h in r.writers.drain(..) {
                let _ = h.join();
            }
        }
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
            self.open_external_editor(device, id);
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
        let title = format!(
            "{} - {}",
            d.plugin.as_ref().map_or("plugin", |p| p.name()),
            d.name
        );
        self.plugins
            .windows
            .insert(device.clone(), EditorWindow::Opening);
        self.plugins
            .window_requests
            .push(PluginWindowRequest::Open {
                device: device.clone(),
                title,
                remembered: self.plugins.remembered.get(device).copied(),
            });
    }

    /// OPEN GUI of an external editor: the plugin's own process shows its window, owned by the
    /// app window; asking again raises it. No host window is involved.
    fn open_external_editor(&mut self, device: &DeviceKey, id: InstanceId) {
        let owner = self.plugins.editor_owner;
        match self.plugins.host.manager().open_external_editor(id, owner) {
            Ok(()) => {
                self.plugins
                    .windows
                    .insert(device.clone(), EditorWindow::External);
            }
            Err(e) => self.reject(format!("cannot open the plugin editor: {e}")),
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

    /// The user resized the window: the plugin may adjust the size; the adjusted size is asked
    /// of the platform when it differs.
    pub fn plugin_window_resized(&mut self, device: &DeviceKey, w: u32, h: u32) {
        if self.plugins.windows.get(device) != Some(&EditorWindow::Open) {
            return;
        }
        let Some(id) = self.plugins.instances.get(device).copied() else {
            return;
        };
        match self.plugins.host.manager().resize_gui(id, w, h) {
            Ok((aw, ah)) => {
                self.plugins.remembered.insert(device.clone(), (aw, ah));
                if (aw, ah) != (w, h) {
                    self.plugins
                        .window_requests
                        .push(PluginWindowRequest::Resize {
                            device: device.clone(),
                            w: aw,
                            h: ah,
                        });
                }
            }
            Err(_) => {
                self.plugins.remembered.insert(device.clone(), (w, h));
            }
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
        // An external editor is a window of the plugin's process: nothing to destroy here.
        let destroy_window = destroy_window && window != EditorWindow::External;
        if let Some(id) = self.plugins.instances.get(device).copied() {
            match self.plugins.host.manager().close_gui(id) {
                Ok(Some(blob)) if !self.plugins.no_capture.contains(device) => {
                    let changed = self
                        .doc()
                        .resolve(device)
                        .and_then(|d| d.plugin.as_ref())
                        .is_none_or(|p| p.state.as_deref() != Some(&blob));
                    if changed
                        && let Err(e) =
                            self.sync_silent(Command::Plugin(PluginCommand::SyncState {
                                device: device.clone(),
                                state: blob,
                            }))
                    {
                        self.reject(e.to_string());
                    }
                    if let Some(r) = &mut self.plugins.recovery {
                        r.wanted = true;
                    }
                }
                _ => {}
            }
        }
        if destroy_window {
            self.plugins
                .window_requests
                .push(PluginWindowRequest::Close {
                    device: device.clone(),
                });
        }
    }

    fn close_plugin_window(&mut self, device: &DeviceKey) {
        self.close_plugin_gui_inner(device, true);
    }

    fn close_all_plugin_windows(&mut self) {
        let open: Vec<DeviceKey> = self.plugins.windows.keys().cloned().collect();
        for key in open {
            self.close_plugin_gui_inner(&key, true);
        }
    }

    /// True when the editor window of `device` is open or opening.
    pub fn plugin_gui_open(&self, device: &DeviceKey) -> bool {
        self.plugins.windows.contains_key(device)
    }

    // ------------------------------------------------------------------ shutdown

    /// Clean exit: closes editors, captures nothing more, removes the crash journal. The platform
    /// layer calls it after the engine was shut down.
    pub fn shutdown_plugins(&mut self) {
        self.close_all_plugin_windows();
        self.wait_recovery_writes();
        self.plugins_poll_events();
        if let Some(journal) = &mut self.plugins.journal {
            let _ = journal.clean_exit();
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
