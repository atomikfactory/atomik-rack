//! Third-party plugins in a bounce, and the glue the app shares.
//!
//! [`PluginHost`] is the one object that turns the scan cache into usable device types and
//! plugin instances: it owns the [`PluginManager`] (with the CLAP and VST3 backends), the
//! [`PluginCatalog`] built from the cache and the user's settings, and registers every usable
//! plugin in a [`Registry`]. The bounce ([`crate::project::render_doc_with`]) and the app use the
//! same type, so both build plugin devices through the same provider.
//!
//! Nothing here loads a plugin file until an instance is created: the catalog is built from the
//! cache alone.
//!
//! **Determinism.** A bounce gives a plugin the same inputs for any host block size and
//! worker count; the output is bit-identical only for plugins that are deterministic functions of
//! those inputs (the in-repo test plugins are). [`determinism_note`] is the line the CLI prints.

use atomik_device_sdk::{PluginFormat, PluginRef, Registry, RunMode};
use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::{
    BackendError, CachedFile, FileStamp, HostHandle, LoadStatus, ManagerEvent, PluginBackend,
    PluginCache, PluginCatalog, PluginInstance, PluginManager, PluginSettings, RegisterReport,
    SCANNER_VERSION, ScannedPlugin, mark_main_thread,
};
use atomik_plugin_vst3::Vst3Backend;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a bounce does with a plugin the machine does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingPlugins {
    /// The bounce fails with an error that names the plugin (the default).
    #[default]
    Error,
    /// The device is built from the project's saved layout: silent for an instrument, the dry
    /// input for an effect.
    StandIn,
}

/// A backend shared between the manager (which instantiates) and the host (which tells it where
/// the files are, from the cache, without loading them).
struct Shared<B>(Arc<B>);

impl<B: PluginBackend> PluginBackend for Shared<B> {
    fn format(&self) -> PluginFormat {
        self.0.format()
    }

    fn scan_file(&self, path: &Path) -> Result<Vec<ScannedPlugin>, BackendError> {
        self.0.scan_file(path)
    }

    fn instantiate(
        &self,
        plugin: &PluginRef,
        host: HostHandle,
    ) -> Result<Box<dyn PluginInstance>, BackendError> {
        self.0.instantiate(plugin, host)
    }
}

/// The plugin format of a file, from its extension (`.vst3` files and bundles are VST3, anything
/// else is CLAP).
pub fn format_of_file(path: &Path) -> PluginFormat {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("vst3"))
    {
        PluginFormat::Vst3
    } else {
        PluginFormat::Clap
    }
}

/// The plugin manager, the catalog and the registry glue.
///
/// The thread that creates a host is the plugin **main thread** (it is marked): create it
/// on the thread that will also poll it and create the instances.
pub struct PluginHost {
    manager: PluginManager,
    clap: Arc<ClapBackend>,
    vst3: Arc<Vst3Backend>,
    catalog: PluginCatalog,
}

impl std::fmt::Debug for PluginHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginHost")
            .field("plugins", &self.catalog.entries().len())
            .field("instances", &self.manager.instance_count())
            .finish()
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginHost {
    /// A host with the CLAP and VST3 backends and an empty catalog. The calling thread becomes the plugin
    /// main thread.
    pub fn new() -> Self {
        mark_main_thread();
        let clap = Arc::new(ClapBackend::new());
        let mut manager = PluginManager::new();
        let vst3 = Arc::new(Vst3Backend::new());
        manager.add_backend(Box::new(Shared(Arc::clone(&clap))));
        manager.add_backend(Box::new(Shared(Arc::clone(&vst3))));
        Self {
            manager,
            clap,
            vst3,
            catalog: PluginCatalog::default(),
        }
    }

    /// The manager (create instances, capture state, open editors).
    pub fn manager(&mut self) -> &mut PluginManager {
        &mut self.manager
    }

    /// The manager, read only.
    pub fn manager_ref(&self) -> &PluginManager {
        &self.manager
    }

    /// The catalog of the installed plugins.
    pub fn catalog(&self) -> &PluginCatalog {
        &self.catalog
    }

    /// Replaces the catalog: removes the types of the old one from `registry`, builds the new one
    /// from `cache` and `settings` (empty while plugins are off) and registers every usable
    /// plugin. No plugin file is opened.
    pub fn set_catalog(
        &mut self,
        cache: &PluginCache,
        settings: &PluginSettings,
        registry: &mut Registry,
    ) -> RegisterReport {
        self.catalog.unregister_from(registry);
        let mut catalog = PluginCatalog::from_cache(cache, settings);
        let (clap, vst3) = (&self.clap, &self.vst3);
        let report = catalog.register_into(registry, &mut |plugin, path| match plugin.format {
            PluginFormat::Clap => clap.register_plugin_file(&plugin.id, path),
            PluginFormat::Vst3 => vst3.register_plugin_file(&plugin.id, path),
        });
        self.catalog = catalog;
        report
    }

    /// Whether `plugin` is installed and usable (listed `Ok` in the catalog).
    pub fn is_installed(&self, plugin: &PluginRef) -> bool {
        self.catalog
            .entry(plugin)
            .is_some_and(|e| e.status == atomik_plugin_host::PluginStatus::Ok)
    }

    /// The file that holds `plugin`, from the catalog.
    pub fn path_of(&self, plugin: &PluginRef) -> Option<PathBuf> {
        self.catalog.entry(plugin).map(|e| e.path.clone())
    }

    /// The per-frame main-thread work of the manager (reaping, callbacks, restarts).
    pub fn poll(&mut self) -> Vec<ManagerEvent> {
        self.manager.poll()
    }

    /// The CLAP backend, for the code that scans in this process (tests, the demo generator).
    pub fn clap_backend(&self) -> &ClapBackend {
        &self.clap
    }

    /// The VST3 backend, for the code that scans in this process.
    pub fn vst3_backend(&self) -> &Vst3Backend {
        &self.vst3
    }
}

/// The VST trademark notice shown wherever the app has an about or credits surface (the command
/// line help of `atomik` and `atomik-render`; the README carries it too). Atomik Rack uses no VST
/// logo, and the name VST3 only says which plugin format is hosted.
pub const VST_TRADEMARK_NOTICE: &str = "VST is a trademark of Steinberg Media Technologies GmbH, registered in Europe and other countries.";

/// The line the CLI prints after a bounce that used plugins.
pub fn determinism_note(count: usize) -> String {
    format!("plugins: {count} (determinism not guaranteed)")
}

/// The scan cache of the app's config directory (`plugins.json` next to the settings file), for
/// this OS and process environment; `None` when the needed environment variable is missing.
pub fn default_cache_path() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let dir = if cfg!(windows) {
        var("APPDATA").map(|d| d.join("Atomik Rack"))
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Application Support/Atomik Rack"))
    } else {
        var("XDG_CONFIG_HOME")
            .or_else(|| var("HOME").map(|h| h.join(".config")))
            .map(|d| d.join("atomik-rack"))
    };
    dir.map(|d| d.join("plugins.json"))
}

/// Reads the scan cache at `path` (or the default location); a missing or unusable cache is an
/// empty one, with the reason in the status.
pub fn load_cache(path: Option<&Path>) -> (PluginCache, LoadStatus) {
    match path.map(Path::to_path_buf).or_else(default_cache_path) {
        Some(p) => PluginCache::load(&p),
        None => (PluginCache::default(), LoadStatus::Missing),
    }
}

/// Scans `files` **in this process** (`.vst3` files and bundles with the VST3 backend, anything
/// else with the CLAP one) and returns the cache they would give.
///
/// This loads the files into the calling process, so it is only for files you trust and built
/// yourself: the in-repo test plugins, and the generator of the `plugin-demo` example. The app
/// scans out of process.
pub fn scan_in_process(files: &[PathBuf]) -> PluginCache {
    // Backends insist on the main thread; the calling thread is it for the scan.
    mark_main_thread();
    let clap = ClapBackend::new();
    let vst3 = Vst3Backend::new();
    let mut cache = PluginCache::default();
    for path in files {
        let format = format_of_file(path);
        let backend: &dyn PluginBackend = match format {
            PluginFormat::Clap => &clap,
            PluginFormat::Vst3 => &vst3,
        };
        let Ok(plugins) = backend.scan_file(path) else {
            continue;
        };
        let Some(stamp) = FileStamp::of(path) else {
            continue;
        };
        cache.insert_file(CachedFile {
            path: path.clone(),
            format,
            stamp,
            scanner_version: SCANNER_VERSION,
            plugins,
        });
    }
    cache
}

/// The settings a bounce uses: plugins are on (the user gave `--allow-plugins`), nothing is
/// disabled.
pub fn bounce_settings() -> PluginSettings {
    PluginSettings {
        enabled: true,
        ..PluginSettings::default()
    }
}

/// The `RunMode` for the CLI's `--plugin-mode` text.
pub fn parse_plugin_mode(text: &str) -> Option<RunMode> {
    match text {
        "offline" => Some(RunMode::Offline),
        "realtime" => Some(RunMode::Realtime),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plugin_mode_text_is_offline_or_realtime_and_nothing_else() {
        assert_eq!(parse_plugin_mode("offline"), Some(RunMode::Offline));
        assert_eq!(parse_plugin_mode("realtime"), Some(RunMode::Realtime));
        assert_eq!(parse_plugin_mode("Realtime"), None);
        assert_eq!(parse_plugin_mode(""), None);
    }

    #[test]
    fn the_trademark_notice_is_the_steinberg_wording() {
        assert_eq!(
            VST_TRADEMARK_NOTICE,
            "VST is a trademark of Steinberg Media Technologies GmbH, registered in Europe and other countries."
        );
    }

    #[test]
    fn the_report_line_names_the_count_and_the_limit() {
        assert_eq!(
            determinism_note(3),
            "plugins: 3 (determinism not guaranteed)"
        );
    }

    #[test]
    fn the_default_cache_is_plugins_json_in_the_app_config_folder() {
        // The environment of the test run decides whether there is one; when there is, it is the
        // `plugins.json` next to the settings file, in the folder named for the app.
        if let Some(path) = default_cache_path() {
            assert_eq!(path.file_name().unwrap(), "plugins.json");
            let folder = path
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy();
            assert!(folder.to_lowercase().contains("atomik"), "{folder}");
        }
    }

    #[test]
    fn loading_a_cache_reports_missing_and_rebuilt_files_and_never_fails() {
        let dir = std::env::temp_dir().join(format!("atomik-render-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let absent = dir.join("absent.json");
        let (cache, status) = load_cache(Some(&absent));
        assert!(matches!(status, LoadStatus::Missing));
        assert_eq!(cache.files().count(), 0);
        let corrupt = dir.join("corrupt.json");
        std::fs::write(&corrupt, "{ not a cache").unwrap();
        let (cache, status) = load_cache(Some(&corrupt));
        assert!(matches!(status, LoadStatus::Rebuilt(_)), "{status:?}");
        assert_eq!(cache.files().count(), 0);
        let good = dir.join("good.json");
        PluginCache::default().save(&good).unwrap();
        assert!(matches!(load_cache(Some(&good)).1, LoadStatus::Loaded));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_in_process_scan_skips_files_that_are_not_plugins() {
        let dir = std::env::temp_dir().join(format!("atomik-render-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let junk = dir.join("junk.clap");
        std::fs::write(&junk, b"not a library").unwrap();
        let cache = scan_in_process(&[junk, dir.join("gone.clap")]);
        assert_eq!(
            cache.files().count(),
            0,
            "nothing listed, nothing blacklisted"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_is_vst3_by_its_extension_and_clap_otherwise() {
        assert_eq!(format_of_file(Path::new("a/Gain.vst3")), PluginFormat::Vst3);
        assert_eq!(format_of_file(Path::new("a/GAIN.VST3")), PluginFormat::Vst3);
        assert_eq!(format_of_file(Path::new("a/gain.clap")), PluginFormat::Clap);
        assert_eq!(format_of_file(Path::new("a/gain")), PluginFormat::Clap);
    }

    #[test]
    fn an_in_process_scan_of_both_formats_registers_both_and_knows_both_files() {
        let clap = atomik_plugin_host::testing::clap_test_plugin("atomik-test-clap").unwrap();
        let vst3 = atomik_plugin_host::testing::vst3_test_plugin("atomik-test-vst3").unwrap();
        let cache = scan_in_process(&[clap.clone(), vst3.clone()]);
        let formats: Vec<PluginFormat> = cache.files().map(|f| f.format).collect();
        assert!(formats.contains(&PluginFormat::Clap) && formats.contains(&PluginFormat::Vst3));
        let mut registry = Registry::new();
        let mut host = PluginHost::new();
        let report = host.set_catalog(&cache, &bounce_settings(), &mut registry);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let types: Vec<String> = host
            .catalog()
            .entries()
            .iter()
            .map(|e| e.plugin.type_id())
            .collect();
        assert!(types.iter().any(|t| t.starts_with("clap.atomik.test.")));
        assert!(types.iter().any(|t| t.starts_with("vst3.")), "{types:?}");
        for e in host.catalog().entries() {
            assert!(registry.is_plugin(&e.plugin.type_id()));
            assert!(host.is_installed(&e.plugin), "{:?}", e.plugin);
            let expected = if e.plugin.format == PluginFormat::Vst3 {
                &vst3
            } else {
                &clap
            };
            assert_eq!(host.path_of(&e.plugin).as_ref(), Some(expected));
        }
    }

    #[test]
    fn a_host_made_only_from_a_cache_instantiates_the_plugins_of_both_formats() {
        // The scan ran in other backends; the host under test knows its files from the cache
        // alone, so `set_catalog` must hand each file to the backend of its own format.
        let clap = atomik_plugin_host::testing::clap_test_plugin("atomik-test-clap").unwrap();
        let vst3 = atomik_plugin_host::testing::vst3_test_plugin("atomik-test-vst3").unwrap();
        let cache = scan_in_process(&[clap, vst3]);
        mark_main_thread();
        let mut registry = Registry::new();
        let mut host = PluginHost::new();
        host.set_catalog(&cache, &bounce_settings(), &mut registry);
        for format in [PluginFormat::Clap, PluginFormat::Vst3] {
            let entry = host
                .catalog()
                .entries()
                .iter()
                .find(|e| e.plugin.format == format && e.name.contains("Gain"))
                .unwrap_or_else(|| panic!("a {format:?} gain effect is listed"))
                .plugin
                .clone();
            let request = atomik_plugin_host::InstantiateRequest {
                plugin: entry,
                state: None,
                sample_rate: 48_000.0,
                mode: RunMode::Offline,
                pinned: false,
                powered: true,
            };
            let handle = host
                .manager()
                .instantiate(&request)
                .unwrap_or_else(|e| panic!("{format:?}: {e}"));
            assert_eq!(handle.layout.name, "Atomik Test Gain");
        }
        assert_eq!(host.manager_ref().instance_count(), 2);
    }

    #[test]
    fn a_fresh_host_has_no_plugins_and_knows_no_file() {
        let host = PluginHost::default();
        assert_eq!(host.catalog().entries().len(), 0);
        assert_eq!(host.manager_ref().instance_count(), 0);
        let plugin = PluginRef::from_type_id("clap.atomik.test.synth").unwrap();
        assert!(!host.is_installed(&plugin));
        assert_eq!(host.path_of(&plugin), None);
    }
}
