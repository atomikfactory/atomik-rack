//! A real, third-party VST3 plugin through the app core (the real-plugin fix). Not part
//! of CI: `ATOMIK_REAL_VST3=<path to a .vst3>` names a plugin on this machine and the test is
//! `#[ignore]`d. It adds the plugin's first class as the browser does and checks the panel
//! status and a render. The plugin runs in the test process: run it under a timeout.

#![allow(clippy::disallowed_methods)] // Justification: a manual harness; timing is its output.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_model::empty_doc;
use atomik_plugin_host::PluginCache;
use atomik_render::plugins::scan_in_process;

#[test]
#[ignore = "needs ATOMIK_REAL_VST3 (a real plugin on this machine)"]
fn a_real_plugin_added_from_the_browser_becomes_live_with_an_editor() {
    let Some(path) = std::env::var_os("ATOMIK_REAL_VST3").map(PathBuf::from) else {
        return;
    };
    let cache: PluginCache = scan_in_process(&[path]);
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(PluginConfig {
        settings: atomik_plugin_host::PluginSettings {
            enabled: true,
            ..atomik_plugin_host::PluginSettings::default()
        },
        cache,
        paths: PluginPaths::default(),
        runner: None,
        previous_crash: None,
        isolation: None,
    });
    let entry = core
        .plugin_catalog()
        .entries()
        .iter()
        .find(|e| e.plugin.format == atomik_device_sdk::PluginFormat::Vst3)
        .expect("a VST3 plugin in the catalog")
        .clone();
    println!("adding {} ({})", entry.name, entry.plugin.type_id());
    let started = std::time::Instant::now();
    let command = add_command(&core, &entry.plugin.type_id(), "real");
    apply(&mut core, command);
    println!(
        "right after the add: {:?} loading={:?}",
        core.view().plugin_status(&key("real")),
        core.plugin_loading()
    );
    core.wait_plugins();
    println!("live after {:?}", started.elapsed());
    let status = core.view().plugin_status(&key("real"));
    println!("status: {status:?}");
    assert!(core.plugin_instance(&key("real")).is_some(), "{status:?}");
    assert!(!status.loading && status.failed.is_none() && status.missing.is_none());
    assert!(status.has_editor, "OPEN GUI must show");
    settle(&mut core);
    for line in core.plugin_log() {
        println!("log: {line}");
    }
}

/// The same through a live session (a real output stream on the default host, silent) and the
/// platform's 16 ms frame cadence. `ATOMIK_REAL_LIVE=1` opts in (it opens the default audio
/// output for a moment).
#[test]
#[ignore = "needs ATOMIK_REAL_VST3 and ATOMIK_REAL_LIVE=1"]
fn a_real_plugin_goes_live_through_a_live_session_at_the_frame_cadence() {
    use crate::session::{LiveSession, SystemBackend};
    let (Some(path), Some(_)) = (
        std::env::var_os("ATOMIK_REAL_VST3").map(PathBuf::from),
        std::env::var_os("ATOMIK_REAL_LIVE"),
    ) else {
        return;
    };
    let cache: PluginCache = scan_in_process(&[path]);
    let session = LiveSession::new(
        SystemBackend,
        // The worker settings of the user's file that reported the bug (two spinning workers).
        crate::settings::AudioSettings {
            workers: 2,
            worker_linger_spins: 500_000,
            buffer_frames: 128,
            ..crate::settings::AudioSettings::default()
        },
        true,
    );
    let mut core = AppCore::with_doc(session, empty_doc(), None);
    core.configure_plugins(PluginConfig {
        settings: atomik_plugin_host::PluginSettings {
            enabled: true,
            ..atomik_plugin_host::PluginSettings::default()
        },
        cache,
        paths: PluginPaths::default(),
        runner: None,
        previous_crash: None,
        isolation: None,
    });
    println!(
        "audio running={} error={:?}",
        core.sink().is_running(),
        core.sink().status().error
    );
    let entry = core
        .plugin_catalog()
        .entries()
        .iter()
        .find(|e| e.plugin.format == atomik_device_sdk::PluginFormat::Vst3)
        .expect("a VST3 plugin in the catalog")
        .clone();
    let command = add_command(&core, &entry.plugin.type_id(), "real");
    apply(&mut core, command);
    let started = std::time::Instant::now();
    let mut last = String::new();
    while started.elapsed().as_secs() < 30 {
        core.frame();
        core.plugin_tick(started.elapsed().as_secs_f64());
        let s = format!(
            "{:?} | {:?}",
            core.view().plugin_status(&key("real")),
            core.plugin_loading_text()
        );
        if s != last {
            println!("[{:>6.2}s] {s}", started.elapsed().as_secs_f64());
            last = s;
        }
        if core.plugin_instance(&key("real")).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    for _ in 0..30 {
        core.frame();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    let status = core.view().plugin_status(&key("real"));
    println!("final: {status:?}");
    for line in core.plugin_log() {
        println!("log: {line}");
    }
    assert!(status.has_editor && !status.loading && status.failed.is_none());
    core.shutdown_plugins();
}

/// The reported state: the saved host is ASIO and the build has no `asio` feature, so no stream
/// opens and (before the fix) every plugin panel sat on LOADING. Opens no audio device.
#[cfg(not(feature = "asio"))]
#[test]
fn a_saved_asio_host_without_the_asio_feature_leaves_plugins_visibly_waiting() {
    use crate::session::{LiveSession, SystemBackend};
    let session = LiveSession::new(
        SystemBackend,
        crate::settings::AudioSettings {
            host: crate::settings::HostChoice::Asio,
            ..crate::settings::AudioSettings::default()
        },
        true,
    );
    let mut core = AppCore::with_doc(session, empty_doc(), None);
    core.configure_plugins(config(true, &[PKG]));
    assert!(!core.sink().is_running());
    let error = core.sink().status().error.clone().expect("audio failed");
    assert!(error.contains("ASIO"), "{error}");
    let add = add_command(&core, GAIN, "fx");
    apply(&mut core, add);
    core.wait_plugins();
    let status = core.view().plugin_status(&key("fx"));
    assert!(!status.loading, "never LOADING forever: {status:?}");
    assert!(status.has_editor);
    assert!(
        status
            .waiting
            .as_deref()
            .is_some_and(|w| w.contains("ASIO")),
        "{status:?}"
    );
}

#[test]
fn the_platform_loop_sees_loading_painted_one_frame_before_the_creation_blocks() {
    let mut core = plugin_core(OfflineSink::new());
    core.set_plugin_load_paints_first(true);
    let add = add_command(&core, GAIN, "fx");
    apply(&mut core, add);
    core.frame();
    assert!(
        core.view().plugin_status(&key("fx")).loading,
        "painted first"
    );
    assert!(core.plugin_instance(&key("fx")).is_none());
    core.frame();
    assert!(core.plugin_instance(&key("fx")).is_some());
}

/// The size of the user's scan cache (38 MB, 128 plugins with full param tables) must not stall
/// start-up: `ATOMIK_REAL_CACHE=<plugins.json>` times load and catalog registration.
#[test]
#[ignore = "needs ATOMIK_REAL_CACHE"]
fn a_big_scan_cache_loads_and_registers_quickly() {
    let Some(path) = std::env::var_os("ATOMIK_REAL_CACHE").map(PathBuf::from) else {
        return;
    };
    let t = std::time::Instant::now();
    let (cache, status) = PluginCache::load(&path);
    println!("load {:?} in {:?}", status, t.elapsed());
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    let t = std::time::Instant::now();
    core.configure_plugins(PluginConfig {
        settings: atomik_plugin_host::PluginSettings {
            enabled: true,
            ..atomik_plugin_host::PluginSettings::default()
        },
        cache,
        paths: PluginPaths::default(),
        runner: None,
        previous_crash: None,
        isolation: None,
    });
    println!(
        "configure (catalog of {}) in {:?}",
        core.plugin_catalog().entries().len(),
        t.elapsed()
    );
    let t = std::time::Instant::now();
    core.frame();
    println!("first frame (view feed) in {:?}", t.elapsed());
    let t = std::time::Instant::now();
    core.frame();
    println!("second frame in {:?}", t.elapsed());
}
