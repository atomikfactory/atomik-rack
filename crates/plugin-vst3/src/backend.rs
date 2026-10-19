//! `Vst3Backend`: loads `.vst3` modules, scans them and creates instances.
//!
//! The backend keeps an index from class id to the file that holds it. `scan_file` fills it;
//! the app can also fill it from the scan cache with [`Vst3Backend::register_plugin_file`]
//! without loading anything (opening a project loads no plugin code). Modules are loaded
//! once per path and stay loaded while the backend lives.

use crate::host::Hub;
use crate::ids::{class_id_hex, tuid_of_hex};
use crate::instance::Vst3Instance;
use crate::layout::{self, ClassInfo};
use crate::module::Module;
use atomik_core::BATCH;
use atomik_device_sdk::{PluginFormat, PluginKindHint, PluginLayout, PluginRef, RunMode};
use atomik_plugin_host::{
    ActivateConfig, BackendError, HostHandle, PluginBackend, PluginInstance as HostInstance,
    ScannedPlugin, assert_main_thread, plugin_channel,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// The VST3 backend.
#[derive(Default)]
pub struct Vst3Backend {
    index: Mutex<BTreeMap<String, PathBuf>>,
    modules: Mutex<BTreeMap<PathBuf, Arc<Module>>>,
}

impl std::fmt::Debug for Vst3Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vst3Backend")
            .field("plugins", &self.index.lock().map(|i| i.len()).unwrap_or(0))
            .finish()
    }
}

impl Vst3Backend {
    /// A backend that knows no plugin files yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that the plugin with class id `id` (32 hex digits) lives in `path`, without
    /// loading the file (the scan cache's way in).
    pub fn register_plugin_file(&self, id: &str, path: &Path) {
        self.index
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_ascii_lowercase(), path.to_path_buf());
    }

    fn load(&self, path: &Path) -> Result<Arc<Module>, BackendError> {
        let mut modules = self.modules.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(m) = modules.get(path) {
            return Ok(Arc::clone(m));
        }
        let module = Module::load(path)?;
        modules.insert(path.to_path_buf(), Arc::clone(&module));
        Ok(module)
    }

    fn classes_of(module: &Arc<Module>, path: &Path) -> Result<Vec<ClassInfo>, BackendError> {
        let factory = module.factory().ok_or_else(|| {
            BackendError::NotAPlugin(format!("{}: no plugin factory", path.display()))
        })?;
        Ok(layout::audio_classes(factory))
    }
}

impl PluginBackend for Vst3Backend {
    fn format(&self) -> PluginFormat {
        PluginFormat::Vst3
    }

    fn scan_file(&self, path: &Path) -> Result<Vec<ScannedPlugin>, BackendError> {
        assert_main_thread("scanning a VST3 file");
        let module = self.load(path)?;
        let classes = Self::classes_of(&module, path)?;
        let mut found = Vec::new();
        for class in classes {
            let plugin = PluginRef {
                format: PluginFormat::Vst3,
                id: class_id_hex(&class.cid),
            };
            // The scan creates the plugin on a throw-away channel and drops it again.
            let (_device_end, manager) = plugin_channel(atomik_plugin_host::InstanceId(0));
            let hub = Hub::new(manager.messenger());
            let mut layout_info = PluginLayout {
                name: class.name.clone(),
                vendor: class.vendor.clone(),
                version: class.version.clone(),
                kind_hint: PluginKindHint::Effect,
                main_in: None,
                main_out: None,
                sidechain: false,
                notes_in: false,
                notes_out: false,
                params: Vec::new(),
            };
            let (unsupported, truncated) = match Vst3Instance::create(&module, &class, hub) {
                Ok(mut instance) => {
                    let (layout, truncated) = instance.read();
                    layout_info = layout;
                    // The plugin must also activate at the session block size.
                    let cfg = ActivateConfig {
                        sample_rate: 48_000.0,
                        frames: BATCH as u32,
                        mode: RunMode::Offline,
                    };
                    match HostInstance::activate(&mut instance, &cfg) {
                        Ok(processor) => {
                            HostInstance::deactivate(&mut instance, processor);
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
                "{}: the file holds no audio plugin",
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
        assert_main_thread("creating a VST3 plugin");
        let id = plugin.id.to_ascii_lowercase();
        let path = self
            .index
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned()
            .ok_or_else(|| BackendError::UnknownPlugin(plugin.id.clone()))?;
        let want =
            tuid_of_hex(&id).ok_or_else(|| BackendError::UnknownPlugin(plugin.id.clone()))?;
        let module = self.load(&path)?;
        let class = Self::classes_of(&module, &path)?
            .into_iter()
            .find(|c| c.cid == want)
            .ok_or_else(|| BackendError::UnknownPlugin(plugin.id.clone()))?;
        let hub = Hub::new(host.messenger().clone());
        Ok(Box::new(Vst3Instance::create(&module, &class, hub)?))
    }
}
