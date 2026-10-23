//! Test support for the plugin glue: the in-repo CLAP test plugins as a scan
//! cache, an app core with them enabled, helpers to add them and to post the messages a plugin's
//! callbacks would post. No `unsafe`: probes into the plugin libraries live in the backend's own
//! tests; here everything goes through the manager and the slot's message ring, like the backend.

use crate::app_core::{AppCore, PluginConfig, PluginPaths};
use crate::sink::EngineSink;
use atomik_device_sdk::PluginRef;
use atomik_model::{Command, DeviceKey, PluginCommand, empty_doc};
use atomik_plugin_host::testing::{clap_test_plugin, vst3_test_plugin};
use atomik_plugin_host::{PluginCache, PluginMessage, PluginSettings};
use atomik_render::plugins::scan_in_process;
use std::path::PathBuf;

/// The package of the main test plugin library.
pub const PKG: &str = "atomik-test-clap";
/// The type ids of its plugins.
pub const SYNTH: &str = "clap.atomik.test.synth";
/// The test effect.
pub const GAIN: &str = "clap.atomik.test.gain";
/// The test note effect.
pub const SHIFT: &str = "clap.atomik.test.noteshift";

/// The package of the VST3 test plugin library (the same four plugins as the CLAP one).
pub const PKG_VST3: &str = "atomik-test-vst3";
/// The VST3 variants library (a split plugin, a mono effect and plugins that fail).
pub const PKG_VST3_VARIANTS: &str = "atomik-test-vst3-variants";

/// The installed `.clap` file or `.vst3` bundle of a test plugin package (built on first use):
/// a package with `vst3` in its name is a VST3 library.
pub fn plugin_file(package: &str) -> PathBuf {
    if package.contains("vst3") {
        vst3_test_plugin(package).expect("the VST3 test plugin builds")
    } else {
        clap_test_plugin(package).expect("the test plugin builds")
    }
}

/// The scan cache of the given packages (an in-process scan of trusted, in-repo files).
pub fn cache_of(packages: &[&str]) -> PluginCache {
    let files: Vec<PathBuf> = packages.iter().map(|p| plugin_file(p)).collect();
    scan_in_process(&files)
}

/// A config with the plugins of `packages` scanned and enabled (or not), and no files kept.
pub fn config(enabled: bool, packages: &[&str]) -> PluginConfig {
    PluginConfig {
        settings: PluginSettings {
            enabled,
            ..PluginSettings::default()
        },
        cache: cache_of(packages),
        paths: PluginPaths::default(),
        runner: None,
        previous_crash: None,
        isolation: None,
    }
}

/// A core over `sink` with the main test plugins scanned and enabled.
pub fn plugin_core<S: EngineSink>(sink: S) -> AppCore<S> {
    let mut core = AppCore::with_doc(sink, empty_doc(), None);
    core.configure_plugins(config(true, &[PKG]));
    core
}

/// The type id of the VST3 test plugin called `name` (`Atomik Test Gain`, `Atomik Test Synth`,
/// `Atomik Test Note Shift`) in the core's catalog.
pub fn vst3_id<S: EngineSink>(core: &AppCore<S>, name: &str) -> String {
    core.plugin_catalog()
        .entries()
        .iter()
        .find(|e| e.plugin.format == atomik_device_sdk::PluginFormat::Vst3 && e.name == name)
        .map(|e| e.plugin.type_id())
        .unwrap_or_else(|| panic!("the VST3 plugin {name} is in the catalog"))
}

pub fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// `PluginCommand::Add` for the plugin of type id `type_id` (`SYNTH`...), with the scanned
/// layout from the core's catalog.
pub fn add_command<S: EngineSink>(core: &AppCore<S>, type_id: &str, k: &str) -> Command {
    let plugin = PluginRef::from_type_id(type_id).expect("a plugin type id");
    let entry = core
        .plugin_catalog()
        .entry(&plugin)
        .unwrap_or_else(|| panic!("{type_id} is in the catalog"));
    Command::Plugin(PluginCommand::Add {
        index: usize::MAX,
        container: None,
        plugin,
        layout: entry.layout.clone(),
        path_hint: None,
        key: Some(key(k)),
        auto_split: false,
    })
}

/// Adds a plugin and waits until its instance is live.
pub fn add_plugin<S: EngineSink>(core: &mut AppCore<S>, type_id: &str, k: &str) {
    let command = add_command(core, type_id, k);
    core.apply_command(command)
        .unwrap_or_else(|e| panic!("add {type_id}: {e}"));
    core.wait_plugins();
}

/// Applies a command, failing the test with the command named when it is rejected.
pub fn apply<S: EngineSink>(c: &mut AppCore<S>, command: Command) {
    c.apply_command(command.clone())
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
}

/// `SetParam` on a device (path key) without a merge id.
pub fn set_param<S: EngineSink>(c: &mut AppCore<S>, device: &str, param: &str, value: f32) {
    apply(
        c,
        Command::SetParam {
            key: key(device),
            param: param.into(),
            value,
            merge: None,
        },
    );
}

/// Renders a little between frames so the engine applies queued edits and hands retired devices
/// back (an offline engine only moves when it renders).
pub fn settle(core: &mut AppCore<crate::offline_sink::OfflineSink>) {
    for _ in 0..4 {
        core.sink_mut().render(128);
        core.frame();
    }
}

/// Steps the engine one 64-frame batch at a time (without playing unless the transport already
/// plays), running the app's per-frame work after each batch, and returns what the engine
/// rendered. This is the cadence of the bounce's plugin loop (`atomik_render::plugin_pass`: one
/// manager poll and one latency recompile per batch), so a restart a plugin asks for lands on the
/// same batch live and in the bounce.
pub fn stepped(
    core: &mut AppCore<crate::offline_sink::OfflineSink>,
    frames: usize,
) -> Vec<Vec<f32>> {
    let mut out: Vec<Vec<f32>> = Vec::new();
    let mut done = 0;
    while done < frames {
        let n = 64.min(frames - done);
        let block = core.sink_mut().render_blocks(n, 64);
        if out.is_empty() {
            out = vec![Vec::new(); block.len()];
        }
        for (all, b) in out.iter_mut().zip(&block) {
            all.extend_from_slice(b);
        }
        core.frame();
        done += n;
    }
    out
}

/// Steps 16 batches ([`stepped`]) so every restart a plugin asked for has finished and the
/// routing carries its latency, as a live project that has been open for a moment (and as the
/// bounce's pre-roll leaves the plugins).
pub fn settle_steps(core: &mut AppCore<crate::offline_sink::OfflineSink>) {
    stepped(core, 64 * 16);
}

/// A fresh empty directory under the system temp dir for one test.
pub fn temp(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("atomik-app-plugins-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Posts to the instance behind `device` what a plugin callback would (a GUI edit, a dirty
/// state, a resize request): through the slot's wait-free message ring, like the backend.
pub fn post<S: EngineSink>(core: &mut AppCore<S>, device: &DeviceKey, message: PluginMessage) {
    let id = core
        .plugin_instance(device)
        .unwrap_or_else(|| panic!("{device} has a live instance"));
    let slot = core
        .plugin_host()
        .manager_ref()
        .slot(id)
        .expect("the instance has a slot");
    assert!(slot.messenger().post(message), "the ring took the message");
}
