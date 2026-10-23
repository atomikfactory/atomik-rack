//! Wiring, sandbox half: where plugins run (read once at start), a crashed device (CRASHED with
//! its reason, never FAILED, never a crash of the app), RELOAD of a device whose process is
//! gone, external editors (the plugin's own window, owned by the app window), and the Settings
//! row. The instances come from the `NullBackend`, so no process is started.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_model::empty_doc;
use atomik_plugin_host::{
    NativeParent, NullBackend, NullBehavior, NullPlugin, NullStats, PluginMessage,
};
use atomik_plugin_sandbox::HostCommand;
use atomik_render::plugins::{PluginHost, PluginIsolation};
use atomik_ui::plugin_data::PluginSettingsAction;

fn sandbox() -> PluginIsolation {
    PluginIsolation::Sandbox {
        command: HostCommand::new("atomik-plugin-host"),
        bounce: false,
    }
}

/// A core whose plugin backend is a `NullBackend` holding the test gain effect, reporting
/// `isolation` as where it runs, configured with `paths`.
fn null_core(
    isolation: PluginIsolation,
    paths: PluginPaths,
) -> (AppCore<OfflineSink>, Arc<NullStats>) {
    let mut config = config(true, &[PKG]);
    config.paths = paths;
    let scanned = config
        .cache
        .files()
        .flat_map(|f| f.plugins.iter())
        .find(|p| p.plugin.type_id() == GAIN)
        .expect("the test gain effect is scanned")
        .clone();
    let backend = NullBackend::new().with_plugin(NullPlugin {
        path: PathBuf::from("null.clap"),
        plugin: scanned.plugin,
        layout: scanned.layout,
        behavior: NullBehavior::Gain(0.5),
        latency: 0,
    });
    let stats = backend.stats();
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.plugins.host = PluginHost::with_backends(isolation, vec![Box::new(backend)]);
    core.configure_plugins(config);
    (core, stats)
}

fn lost_core() -> (AppCore<OfflineSink>, Arc<NullStats>, DeviceKey) {
    let (mut core, stats) = null_core(sandbox(), PluginPaths::default());
    add_plugin(&mut core, GAIN, "echo");
    core.frame();
    (core, stats, key("echo"))
}

/// What the device and the manager post when the plugin's process ends.
fn lose(core: &mut AppCore<OfflineSink>, stats: &NullStats, device: &DeviceKey, reason: &str) {
    stats.set_lost(true);
    post(
        core,
        device,
        PluginMessage::Failed {
            reason: atomik_plugin_host::LOST,
        },
    );
    post(
        core,
        device,
        PluginMessage::ProcessExited {
            reason: reason.to_string(),
        },
    );
    core.frame();
}

// ------------------------------------------------------------------ where plugins run

#[test]
fn the_host_follows_the_isolation_the_start_up_chose_and_keeps_it_when_none_is_given() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    assert!(!core.plugin_host().is_sandboxed(), "in process until told");
    let mut c = config(true, &[PKG]);
    c.isolation = Some(sandbox());
    core.configure_plugins(c);
    assert!(core.plugin_host().is_sandboxed());
    core.configure_plugins(config(true, &[PKG]));
    assert!(
        core.plugin_host().is_sandboxed(),
        "no isolation in the config: the host stays"
    );
    let mut c = config(true, &[PKG]);
    c.isolation = Some(PluginIsolation::InProcess);
    core.configure_plugins(c);
    assert!(!core.plugin_host().is_sandboxed());
}

#[test]
fn the_switch_in_settings_is_saved_and_changes_nothing_until_the_next_start() {
    let (mut core, _) = null_core(sandbox(), PluginPaths::default());
    assert!(core.plugin_settings_data().sandbox, "on by default");
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::SetSandbox(
        false,
    )));
    assert!(!core.plugin_settings().sandbox);
    assert!(!core.plugin_settings_data().sandbox);
    assert!(
        !core.take_plugin_settings_change().unwrap().sandbox,
        "the platform layer saves it"
    );
    assert!(core.plugin_host().is_sandboxed(), "applies after a restart");
}

// ------------------------------------------------------------------ the journal

#[test]
fn sandboxed_instances_are_not_journaled_but_in_process_ones_still_are() {
    let dir = temp("sandbox-journal");
    let journal = dir.join("plugins-in-use.json");
    let (mut core, _) = null_core(sandbox(), PluginPaths::in_dir(&dir));
    add_plugin(&mut core, GAIN, "echo");
    assert_eq!(core.plugin_instance_count(), 1);
    assert!(!journal.exists(), "a sandboxed plugin cannot crash the app");
    let _ = std::fs::remove_dir_all(&dir);
    let dir = temp("sandbox-journal-in-process");
    let journal = dir.join("plugins-in-use.json");
    let (mut core, _) = null_core(PluginIsolation::InProcess, PluginPaths::in_dir(&dir));
    add_plugin(&mut core, GAIN, "echo");
    assert!(journal.exists(), "in process: unchanged");
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ crashed

#[test]
fn a_crash_marks_the_device_crashed_with_its_reason_logs_one_line_and_is_not_failed() {
    let (mut core, stats, dev) = lost_core();
    assert!(core.view().plugin_status(&dev).crashed.is_none());
    lose(&mut core, &stats, &dev, "exited with code 3");
    let status = core.view().plugin_status(&dev);
    assert_eq!(status.crashed.as_deref(), Some("exited with code 3"));
    assert!(status.failed.is_none(), "{status:?}");
    assert!(status.missing.is_none() && !status.loading);
    assert_eq!(core.plugin_failure(&dev), None);
    assert_eq!(core.plugin_crash(&dev), Some("exited with code 3"));
    let log = core.plugin_log();
    assert_eq!(
        log.iter().filter(|l| l.contains("crashed")).count(),
        1,
        "{log:?}"
    );
    assert!(
        log.contains(&"[echo] crashed: exited with code 3".to_string()),
        "{log:?}"
    );
    assert!(
        !log.iter().any(|l| l.contains("failed")),
        "the Failed that goes with it is not a second line: {log:?}"
    );
    // The app is fine: no banner, and a later frame changes nothing.
    assert!(core.plugin_banner().is_none());
    core.frame();
    assert_eq!(core.plugin_log().len(), log.len());
}

#[test]
fn a_crash_leaves_no_journal_entry_to_offer_disable_at_the_next_start() {
    let dir = temp("sandbox-crash-journal");
    let (mut core, stats) = null_core(sandbox(), PluginPaths::in_dir(&dir));
    add_plugin(&mut core, GAIN, "echo");
    lose(&mut core, &stats, &key("echo"), "exited with code 3");
    // The app ends without a clean exit (the journal is not removed by it).
    let next = crate::plugin_setup::plugin_config(
        atomik_plugin_host::PluginSettings::default(),
        Some(&dir),
        None,
    );
    assert!(next.previous_crash.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ RELOAD of a lost instance

#[test]
fn reload_replaces_a_crashed_instance_with_a_new_one_and_clears_the_crash() {
    let (mut core, stats, dev) = lost_core();
    let before = core.plugin_instance(&dev).unwrap();
    let state_before = core
        .doc()
        .resolve(&dev)
        .and_then(|d| d.plugin.as_deref())
        .and_then(|p| p.state.as_deref().cloned());
    lose(&mut core, &stats, &dev, "exited with code 3");
    core.handle_action(UiAction::ReloadPlugin {
        device: dev.clone(),
    });
    // The new process starts fine (the null backend has one flag for all its instances).
    stats.set_lost(false);
    core.wait_plugins();
    settle(&mut core);
    let after = core.plugin_instance(&dev).expect("a new instance");
    assert_ne!(after, before);
    assert_eq!(stats.created(), 2);
    assert!(!core.plugin_host().manager_ref().is_lost(after));
    assert_eq!(core.plugin_crash(&dev), None);
    assert_eq!(core.plugin_failure(&dev), None);
    assert!(core.view().plugin_status(&dev).crashed.is_none());
    assert_eq!(core.plugin_instance_count(), 1, "the old one was reaped");
    assert_eq!(stats.destroyed(), 1);
    let state_after = core
        .doc()
        .resolve(&dev)
        .and_then(|d| d.plugin.as_deref())
        .and_then(|p| p.state.as_deref().cloned());
    assert_eq!(
        state_after, state_before,
        "nothing was captured from the lost instance"
    );
}

#[test]
fn a_reloaded_instance_that_crashes_again_shows_crashed_again() {
    let (mut core, stats, dev) = lost_core();
    lose(&mut core, &stats, &dev, "exited with code 3");
    core.handle_action(UiAction::ReloadPlugin {
        device: dev.clone(),
    });
    stats.set_lost(false);
    core.wait_plugins();
    lose(&mut core, &stats, &dev, "killed by signal 9");
    assert_eq!(core.plugin_crash(&dev), Some("killed by signal 9"));
}

// ------------------------------------------------------------------ external editors

fn owner() -> NativeParent {
    if cfg!(windows) {
        NativeParent::Win32(0xA11C_E000)
    } else if cfg!(target_os = "macos") {
        NativeParent::Cocoa(0xA11C_E000)
    } else {
        NativeParent::X11(0x00A1_1CE0)
    }
}

#[test]
fn open_gui_of_an_external_editor_asks_no_host_window_and_opens_the_plugins_own() {
    let (mut core, stats, dev) = lost_core();
    stats.set_external_editor(true);
    core.set_plugin_editor_owner(Some(owner()));
    assert_eq!(core.plugin_editor_owner(), Some(owner()));
    core.frame();
    assert!(core.view().plugin_status(&dev).has_editor);
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    assert_eq!(core.take_plugin_window_requests(), vec![]);
    assert_eq!(stats.external_opens(), 1);
    assert!(core.plugin_gui_open(&dev), "the panel knows it is open");
    // OPEN GUI again raises the plugin's window.
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    assert_eq!(stats.external_opens(), 2);
    assert_eq!(core.take_plugin_window_requests(), vec![]);
}

#[test]
fn the_child_closing_its_editor_updates_the_panel_without_destroying_a_host_window() {
    let (mut core, stats, dev) = lost_core();
    stats.set_external_editor(true);
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    post(&mut core, &dev, PluginMessage::GuiClosed);
    core.frame();
    assert!(!core.plugin_gui_open(&dev));
    assert_eq!(
        core.take_plugin_window_requests(),
        vec![],
        "there is no host window to destroy"
    );
}

#[test]
fn closing_an_external_editor_from_the_app_closes_it_and_removing_the_device_does_too() {
    let (mut core, stats, dev) = lost_core();
    stats.set_external_editor(true);
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    core.close_plugin_gui(&dev);
    assert!(!core.plugin_gui_open(&dev));
    assert_eq!(core.take_plugin_window_requests(), vec![]);
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    assert!(core.plugin_gui_open(&dev));
    apply(&mut core, Command::RemoveDevice { key: dev.clone() });
    assert!(!core.plugin_gui_open(&dev));
    assert_eq!(core.take_plugin_window_requests(), vec![]);
}

#[test]
fn a_plugin_without_an_editor_asks_for_no_window_and_no_external_open() {
    let (mut core, stats, dev) = lost_core();
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    assert_eq!(stats.external_opens(), 0);
    assert_eq!(core.take_plugin_window_requests(), vec![]);
    assert!(core.last_rejection().is_some());
}
