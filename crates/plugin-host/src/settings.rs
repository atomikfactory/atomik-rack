//! The plugin settings the app owns and persists: the consent switch, the search
//! folders, the scan timeout and the per-plugin disable list. Plugins are **off by default**;
//! nothing in this crate scans, registers or loads while [`PluginSettings::enabled`] is false.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

/// The one-time warning the Settings dialog shows when the user turns plugins on.
pub const ENABLE_WARNING: &str = "Plugins are native programs made by other companies. They run inside Atomik with the same access as Atomik, and a faulty plugin can crash it. Only scan folders you trust.";

/// Shortest scan timeout in seconds.
pub const MIN_TIMEOUT_SECS: u32 = 5;
/// Longest scan timeout in seconds.
pub const MAX_TIMEOUT_SECS: u32 = 300;
/// The default scan timeout in seconds.
pub const DEFAULT_TIMEOUT_SECS: u32 = 30;

/// Why plugins could not be switched on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConsentError {
    /// The user has not confirmed [`ENABLE_WARNING`].
    #[error("the user has not accepted the third-party plugin warning")]
    WarningNotAcknowledged,
}

/// The plugin section of the app settings. Serialises with defaults for every missing field, so
/// an old settings file loads with plugins off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginSettings {
    /// Third-party plugins are enabled (scanning, catalog, loading).
    pub enabled: bool,
    /// Folders the user added to the platform defaults.
    pub extra_paths: Vec<PathBuf>,
    /// Scan timeout per file in seconds (clamped to 5 to 300 when read).
    pub timeout_secs: u32,
    /// Type ids (`clap.<id>`) the user disabled: hidden from the browser, existing devices MISSING.
    pub disabled: BTreeSet<String>,
}

impl Default for PluginSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            extra_paths: Vec::new(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            disabled: BTreeSet::new(),
        }
    }
}

impl PluginSettings {
    /// Switches plugins on. `warning_acknowledged` is the user's answer to [`ENABLE_WARNING`].
    ///
    /// # Errors
    /// [`ConsentError::WarningNotAcknowledged`] (and nothing changes) when it is false.
    pub fn enable(&mut self, warning_acknowledged: bool) -> Result<(), ConsentError> {
        if warning_acknowledged {
            self.enabled = true;
            Ok(())
        } else {
            Err(ConsentError::WarningNotAcknowledged)
        }
    }

    /// Switches plugins off.
    pub fn disable_all(&mut self) {
        self.enabled = false;
    }

    /// Sets the timeout, clamped to the allowed range.
    pub fn set_timeout_secs(&mut self, secs: u32) {
        self.timeout_secs = secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
    }

    /// The scan timeout, always within 5 to 300 seconds.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(u64::from(
            self.timeout_secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS),
        ))
    }

    /// True when the user disabled this plugin type.
    pub fn is_plugin_disabled(&self, type_id: &str) -> bool {
        self.disabled.contains(type_id)
    }
}
