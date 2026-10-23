//! Plain data the app feeds to the plugin views.
//!
//! `atomik-ui` depends on neither the plugin host nor a backend: the app converts its
//! `PluginCatalog`, `PluginSettings`, `ScanProgress` and manager state into these structs and
//! hands them to [`crate::RackView`], [`crate::PluginSettingsView`], [`crate::PluginParamsView`]
//! and [`crate::PluginBannerView`]. Everything here is cloneable and comparable so a view can
//! tell whether anything changed.

use atomik_device_sdk::{PluginFormat, PluginKindHint, PluginLayout, PluginRef};

/// The help line of the parameter pane: what undo covers.
pub const PARAMS_HELP: &str = "Undo covers parameter edits, one gesture each. A patch or preset change made inside the plugin's own window is not undoable.";

/// Index of the state in a plugin device's meter values (0 running, 1 sleeping, 2 stopped, 3
/// failed, 4 restarting, 5 crashed).
pub const METER_STATE: usize = 2;
/// Index of the reported latency (frames) in a plugin device's meter values.
pub const METER_LATENCY: usize = 3;
/// Index of the dropped-event count in a plugin device's meter values.
pub const METER_DROPPED: usize = 4;

/// Meter state value of a failed plugin device.
pub const STATE_FAILED: f32 = 3.0;
/// Meter state value of a plugin device whose process is gone (sandbox).
pub const STATE_CRASHED: f32 = 5.0;

/// The short badge of a format.
pub fn format_badge(format: PluginFormat) -> &'static str {
    match format {
        PluginFormat::Clap => "CLAP",
        PluginFormat::Vst3 => "VST3",
    }
}

/// Whether a plugin can be used (mirror of the host's `PluginStatus`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginEntryStatus {
    /// Usable.
    Ok,
    /// Listed but cannot be hosted (the reason).
    Unsupported(String),
    /// The user disabled it.
    Disabled,
    /// Found but unusable (the reason).
    Failed(String),
}

impl PluginEntryStatus {
    /// The status word of the settings list.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Unsupported(_) => "UNSUPPORTED",
            Self::Disabled => "DISABLED",
            Self::Failed(_) => "FAILED",
        }
    }

    /// The reason, for the states that have one.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Unsupported(r) | Self::Failed(r) => Some(r),
            _ => None,
        }
    }
}

/// One installed plugin as the browser and the settings list show it (mirror of the host's
/// `PluginEntry`).
#[derive(Debug, Clone, PartialEq)]
pub struct PluginEntryInfo {
    /// Format and id.
    pub plugin: PluginRef,
    /// Display name.
    pub name: String,
    /// Vendor.
    pub vendor: String,
    /// Version text.
    pub version: String,
    /// Instrument, effect or note effect.
    pub kind: PluginKindHint,
    /// Usable or why not.
    pub status: PluginEntryStatus,
    /// The file that holds it (shown, never used by the UI).
    pub path: Option<String>,
    /// The scanned layout: what `PluginCommand::Add` and `Relink` carry.
    pub layout: PluginLayout,
}

impl PluginEntryInfo {
    /// The registry type id.
    pub fn type_id(&self) -> String {
        self.plugin.type_id()
    }

    /// The row text of the browser: `name (vendor)`.
    pub fn title(&self) -> String {
        if self.vendor.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.vendor)
        }
    }

    /// Case-insensitive match on name, vendor and format badge (`clap`, `vst3`); an empty query
    /// matches everything.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || self.name.to_lowercase().contains(&q)
            || self.vendor.to_lowercase().contains(&q)
            || format_badge(self.plugin.format).to_lowercase().contains(&q)
    }
}

/// A plugin file that failed to scan (the blacklist).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedFileInfo {
    /// The file.
    pub path: String,
    /// Why it was blacklisted.
    pub reason: String,
}

/// What the app knows about the live instance behind a plugin device (not in the document).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginDeviceStatus {
    /// The plugin has an editor window (OPEN GUI is shown only then).
    pub has_editor: bool,
    /// The plugin is not installed or not loadable: the device is a stand-in. The reason.
    pub missing: Option<String>,
    /// The instance failed (error status, non-finite output, failed restore). The reason.
    pub failed: Option<String>,
    /// The plugin's process ended (sandbox): the reason, the exit code or signal and the last
    /// lines it wrote. RELOAD creates a new instance.
    pub crashed: Option<String>,
    /// The instance is being created.
    pub loading: bool,
    /// The instance waits for the audio engine (audio is off): the reason. The editor and the
    /// params work; the device makes no sound until audio runs.
    pub waiting: Option<String>,
}

/// How a scan walks the files (mirror of the host's `RescanMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginRescan {
    /// New and changed files.
    Incremental,
    /// Every file.
    Full,
    /// The blacklisted files, plus whatever an incremental scan finds.
    FailedOnly,
}

/// Progress of a running scan (`ScanProgress::snapshot`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanProgressInfo {
    /// Files done.
    pub done: usize,
    /// Files in the scan.
    pub total: usize,
}

/// A plugin row of the settings list.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsPluginRow {
    /// Format and id.
    pub plugin: PluginRef,
    /// Name.
    pub name: String,
    /// Vendor.
    pub vendor: String,
    /// Version.
    pub version: String,
    /// Usable or why not. A user-disabled plugin is `Disabled`.
    pub status: PluginEntryStatus,
}

/// Everything the Plugins section of the settings dialog shows.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginSettingsData {
    /// Third-party plugins are enabled.
    pub enabled: bool,
    /// The folders the user added.
    pub extra_paths: Vec<String>,
    /// Scan timeout per file, seconds.
    pub timeout_secs: u32,
    /// The found plugins.
    pub plugins: Vec<SettingsPluginRow>,
    /// The blacklist.
    pub failed: Vec<FailedFileInfo>,
    /// A scan is running, with its progress.
    pub scan: Option<ScanProgressInfo>,
    /// A scanner program can be run (false: the view says so, persistently, and SCAN does nothing).
    pub scanner_available: bool,
    /// The summary line of the last finished scan (shown while no scan runs).
    pub last_scan: Option<String>,
    /// The bounded plugin log, oldest first.
    pub log: Vec<String>,
    /// Plugins run in a separate process (a change applies after a restart of the app).
    pub sandbox: bool,
}

impl Default for PluginSettingsData {
    fn default() -> Self {
        Self {
            enabled: false,
            extra_paths: Vec::new(),
            timeout_secs: 30,
            plugins: Vec::new(),
            failed: Vec::new(),
            scan: None,
            scanner_available: true,
            last_scan: None,
            log: Vec::new(),
            sandbox: true,
        }
    }
}

/// Shortest and longest scan timeout the settings stepper offers.
pub const MIN_TIMEOUT_SECS: u32 = 5;
/// See [`MIN_TIMEOUT_SECS`].
pub const MAX_TIMEOUT_SECS: u32 = 300;

/// What the Plugins section of the settings dialog asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSettingsAction {
    /// Switch plugins on or off.
    SetEnabled {
        /// The new setting.
        enabled: bool,
    },
    /// Show the folder dialog and add the folder to the search paths.
    AddFolderDialog,
    /// Remove a folder from the search paths.
    RemoveFolder(String),
    /// Start a scan.
    Scan(PluginRescan),
    /// Cancel the running scan.
    CancelScan,
    /// Set the scan timeout (seconds, 5 to 300).
    SetTimeout(u32),
    /// Run plugins in a separate process (applies after a restart of the app).
    SetSandbox(bool),
    /// Disable or enable one plugin (type id).
    SetPluginDisabled {
        /// The type id (`clap.<id>`).
        type_id: String,
        /// The new setting.
        disabled: bool,
    },
    /// Take one failed file off the blacklist and scan it again.
    RetryFile(String),
}

/// What the project banner asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginBannerAction {
    /// "This project uses N third-party plugins": LOAD (turns plugins on if they are off).
    Load,
    /// OPEN WITHOUT: keep the stand-ins.
    OpenWithout,
    /// After an unclean exit: DISABLE the plugins that were loaded (blacklist them).
    DisableCrashed,
    /// After an unclean exit: KEEP them.
    KeepCrashed,
    /// After an unclean exit: open the newest recovery snapshot.
    OpenRecovery,
}
