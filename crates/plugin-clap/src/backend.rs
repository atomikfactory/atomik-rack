//! `ClapBackend`: loads `.clap` files, scans them and creates instances.
//!
//! The backend keeps an index from plugin id to the file that holds it. `scan_file` fills it;
//! the app can also fill it from the scan cache with [`ClapBackend::register_plugin_file`]
//! without loading anything (opening a project loads no plugin code).

use crate::hub::Hub;
use crate::instance::ClapInstance;
use crate::layout::DescInfo;
use atomik_core::BATCH;
use atomik_device_sdk::{PluginFormat, PluginRef, RunMode};
use atomik_plugin_host::{
    ActivateConfig, BackendError, HostHandle, PluginBackend, PluginInstance as HostInstance,
    ScannedPlugin, assert_main_thread, plugin_channel,
};
use clack_host::prelude::*;
use std::collections::BTreeMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// The CLAP backend.
pub struct ClapBackend {
    host_info: HostInfo,
    index: Mutex<BTreeMap<String, PathBuf>>,
    entries: Mutex<BTreeMap<PathBuf, PluginEntry>>,
}

impl std::fmt::Debug for ClapBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClapBackend")
            .field("plugins", &self.index.lock().map(|i| i.len()).unwrap_or(0))
            .finish()
    }
}

impl Default for ClapBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn describe(descriptor: &clack_host::plugin::PluginDescriptor) -> DescInfo {
    let text = |s: Option<&std::ffi::CStr>| {
        s.map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    DescInfo {
        name: text(descriptor.name()),
        vendor: text(descriptor.vendor()),
        version: text(descriptor.version()),
        features: descriptor
            .features()
            .map(|f| f.to_string_lossy().into_owned())
            .collect(),
    }
}

impl ClapBackend {
    /// A backend that knows no plugin files yet.
    pub fn new() -> Self {
        Self {
            host_info: HostInfo::new(
                "Atomik Rack",
                "Atomik Factory",
                "https://github.com/atomikfactory/atomik-rack",
                env!("CARGO_PKG_VERSION"),
            )
            .unwrap_or_else(|_| {
                HostInfo::new_from_cstring(
                    c"Atomik Rack".to_owned(),
                    c"".to_owned(),
                    c"".to_owned(),
                    c"0".to_owned(),
                )
            }),
            index: Mutex::new(BTreeMap::new()),
            entries: Mutex::new(BTreeMap::new()),
        }
    }

    /// Records that plugin `id` lives in `path`, without loading the file (the scan cache's
    /// way in).
    pub fn register_plugin_file(&self, id: &str, path: &Path) {
        self.index
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_string(), path.to_path_buf());
    }

    fn load(&self, path: &Path) -> Result<PluginEntry, BackendError> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = entries.get(path) {
            return Ok(entry.clone());
        }
        if !path.exists() {
            return Err(BackendError::NotAPlugin(format!(
                "{}: no such file",
                path.display()
            )));
        }
        // SAFETY: loading a plugin library runs its code; this is the point of hosting. The
        // app only loads files the scanner process already loaded without a crash.
        let entry = unsafe { PluginEntry::load(path) }.map_err(|e| match e {
            clack_host::entry::PluginEntryError::LibraryLoadingError(_)
            | clack_host::entry::PluginEntryError::ResolveFailed
            | clack_host::entry::PluginEntryError::NullEntryPointer => {
                BackendError::NotAPlugin(format!("{}: {e}", path.display()))
            }
            other => BackendError::Failed(format!("{}: {other}", path.display())),
        })?;
        entries.insert(path.to_path_buf(), entry.clone());
        Ok(entry)
    }
}

impl PluginBackend for ClapBackend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Clap
    }

    fn scan_file(&self, path: &Path) -> Result<Vec<ScannedPlugin>, BackendError> {
        assert_main_thread("scanning a CLAP file");
        let entry = self.load(path)?;
        let factory = entry.get_plugin_factory().ok_or_else(|| {
            BackendError::NotAPlugin(format!("{}: no plugin factory", path.display()))
        })?;
        let mut found = Vec::new();
        for descriptor in factory.plugin_descriptors() {
            let Some(id) = descriptor.id() else { continue };
            let info = describe(descriptor);
            let plugin = PluginRef {
                format: PluginFormat::Clap,
                id: id.to_string_lossy().into_owned(),
            };
            // The scan creates the plugin on a throw-away channel and drops it again.
            let (_device_end, manager) = plugin_channel(atomik_plugin_host::InstanceId(0));
            let hub = Hub::new(manager.messenger());
            let mut layout_info = atomik_device_sdk::PluginLayout {
                name: info.name.clone(),
                vendor: info.vendor.clone(),
                version: info.version.clone(),
                kind_hint: atomik_device_sdk::PluginKindHint::Effect,
                main_in: None,
                main_out: None,
                sidechain: false,
                notes_in: false,
                notes_out: false,
                params: Vec::new(),
            };
            let (unsupported, truncated) =
                match ClapInstance::create(&entry, &self.host_info, id, info, hub) {
                    Ok(mut instance) => {
                        let (layout, truncated) = instance.read();
                        layout_info = layout;
                        // The plugin must also activate at the session's block size.
                        let cfg = ActivateConfig {
                            sample_rate: 48_000.0,
                            frames: BATCH as u32,
                            mode: RunMode::Offline,
                        };
                        match instance.activate(&cfg) {
                            Ok(processor) => {
                                instance.deactivate(processor);
                                (None, truncated)
                            }
                            Err(e) => (Some(e.to_string()), truncated),
                        }
                    }
                    Err(BackendError::Unsupported(reason)) => (Some(reason), false),
                    Err(e) => (Some(e.to_string()), false),
                };
            self.register_plugin_file(&plugin.id, path);
            found.push(ScannedPlugin {
                plugin,
                layout: layout_info,
                unsupported,
                params_truncated: truncated,
            });
        }
        if found.is_empty() {
            return Err(BackendError::NotAPlugin(format!(
                "{}: the file holds no plugin",
                path.display()
            )));
        }
        Ok(found)
    }

    fn instantiate(
        &self,
        plugin: &PluginRef,
        host: HostHandle,
    ) -> Result<Box<dyn HostInstance>, BackendError> {
        assert_main_thread("creating a CLAP plugin");
        let path = self
            .index
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&plugin.id)
            .cloned()
            .ok_or_else(|| BackendError::UnknownPlugin(plugin.id.clone()))?;
        let entry = self.load(&path)?;
        let factory = entry.get_plugin_factory().ok_or_else(|| {
            BackendError::NotAPlugin(format!("{}: no plugin factory", path.display()))
        })?;
        let id = CString::new(plugin.id.as_str())
            .map_err(|_| BackendError::UnknownPlugin(plugin.id.clone()))?;
        let descriptor = factory
            .plugin_descriptors()
            .find(|d| d.id() == Some(id.as_c_str()))
            .ok_or_else(|| BackendError::UnknownPlugin(plugin.id.clone()))?;
        let info = describe(descriptor);
        let hub = Hub::new(host.messenger().clone());
        let instance = ClapInstance::create(&entry, &self.host_info, &id, info, hub)?;
        Ok(Box::new(instance))
    }
}
