//! The plugin host core: everything about hosting third-party plugins that does not
//! depend on a plugin format.
//!
//! * [`PluginDevice`] is the rack device of every plugin: 64-frame blocks, note and param
//!   translation, transport, the bypass crossfade, sleep and skip rules, the FAILED state and
//!   the panic boundary, the restart hand-shake and the reaper.
//! * [`PluginBackend`], [`PluginInstance`] and [`PluginProcessor`] are the seam a format
//!   implements; [`NullBackend`] implements it for tests.
//! * [`PluginManager`] owns the instances on the main thread: it creates and activates them,
//!   restarts them through the hand-shake, tears them down through the reaper queue and keeps
//!   them alive across engine restarts; [`PluginProvider`] is how the app and the render tool
//!   ask it for devices.
//! * The slot types ([`PluginSlot`], [`plugin_channel`], [`HostMessenger`], [`ReaperQueue`])
//!   carry the wait-free hand-shake between the engine and the main thread.
//! * `threads` and `fpenv` answer the plugin thread-check and set flush-to-zero.
//!
//! The crate depends on `atomik-core` and `atomik-device-sdk` only. `unsafe` is denied
//! workspace-wide; the single exception is `fpenv` (see `tests/unsafe_policy.rs`).

mod backend;
mod cache;
mod catalog;
mod child;
mod device;
mod fpenv;
mod journal;
pub mod layout;
mod manager;
pub mod paths;
mod scan;
mod settings;
mod slot;
mod standin;
pub mod testing;
mod threads;
mod wire;

pub use backend::{
    ActivateConfig, BackendError, EventList, HostHandle, NativeParent, NullBackend, NullBehavior,
    NullPlugin, NullStats, PluginBackend, PluginEvent, PluginEventKind, PluginInstance, PluginIo,
    PluginProcessor, PluginTransport, ProcessStatus, ScannedPlugin, SleepRule, TailLength,
};
pub use cache::{
    BlacklistEntry, CACHE_VERSION, CachedFile, FileStamp, LoadStatus, PluginCache, SCANNER_VERSION,
};
pub use catalog::{FailedFile, PluginCatalog, PluginEntry, PluginStatus, RegisterReport};
pub use child::{ChildScanner, MAX_SCAN_OUTPUT, STDERR_REASON_BYTES, ScannerCommand};
pub use device::{METER_VALUES, PluginDevice};
pub use fpenv::{FLUSH_SUPPORTED, FtzGuard, denormals_flushed};
pub use journal::{CrashJournal, InUsePlugin, RecoveryDir, blacklist_in_use};
pub use manager::{
    InstantiateRequest, ManagerError, ManagerEvent, PluginHandle, PluginManager, PluginProvider,
};
pub use scan::{
    ALL_FORMATS, FailureKind, MAX_PARALLEL_SCANS, RescanMode, ScanEnv, ScanError, ScanFailure,
    ScanJob, ScanProgress, ScanReport, ScanRequest, ScanResult, ScanRunner, rescan,
};
pub use settings::{
    ConsentError, DEFAULT_TIMEOUT_SECS, ENABLE_WARNING, MAX_TIMEOUT_SECS, MIN_TIMEOUT_SECS,
    PluginSettings,
};
pub use slot::{
    DeviceEnd, DeviceState, HostMessenger, InstanceId, LogLevel, MESSAGE_CAPACITY, ManagerEnd,
    ParamPhase, PluginMessage, PluginSlot, ReapItem, ReaperQueue, plugin_channel, warm, warmed,
};
pub use standin::{PluginStandIn, stand_in};
pub use threads::{
    AudioThreadGuard, assert_main_thread, is_audio_thread, is_main_thread, mark_main_thread,
};
