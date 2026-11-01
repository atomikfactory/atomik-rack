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

// ------------------------------------------------------------------ a hung plugin

/// What the manager posts when the app ended a process that stopped answering audio.
fn hang(core: &mut AppCore<OfflineSink>, device: &DeviceKey) {
    post(
        core,
        device,
        PluginMessage::ProcessExited {
            reason: format!("{} and was stopped", atomik_plugin_host::HUNG),
        },
    );
    core.frame();
}

#[test]
fn a_hung_plugin_is_reloaded_by_itself_from_its_last_state() {
    let (mut core, stats, dev) = lost_core();
    let before = core.plugin_instance(&dev).unwrap();
    stats.set_lost(true);
    hang(&mut core, &dev);
    core.wait_plugins();
    assert_eq!(stats.created(), 2, "a new instance was asked for");
    assert_ne!(core.plugin_instance(&dev), Some(before));
}

#[test]
fn a_plugin_that_keeps_hanging_is_left_crashed_after_two_reloads_until_the_user_decides() {
    let (mut core, stats, dev) = lost_core();
    stats.set_lost(true);
    for _ in 0..4 {
        hang(&mut core, &dev);
        core.wait_plugins();
    }
    assert_eq!(stats.created(), 3, "the original and two reloads");
    assert!(
        core.plugin_crash(&dev)
            .is_some_and(|r| r.contains("answering"))
    );
    core.handle_action(UiAction::ReloadPlugin {
        device: dev.clone(),
    });
    core.wait_plugins();
    assert_eq!(stats.created(), 4, "RELOAD still works");
    hang(&mut core, &dev);
    core.wait_plugins();
    assert_eq!(stats.created(), 5, "and the count starts again");
}

#[test]
fn a_plugin_that_ended_for_another_reason_is_not_reloaded_by_itself() {
    let (mut core, stats, dev) = lost_core();
    lose(&mut core, &stats, &dev, "exited with code 3");
    core.wait_plugins();
    assert_eq!(stats.created(), 1);
    assert!(core.plugin_crash(&dev).is_some());
}

// ------------------------------------------------------------------ presets and patch steps

fn stored(core: &AppCore<OfflineSink>, device: &DeviceKey) -> Option<Vec<u8>> {
    core.doc()
        .resolve(device)
        .and_then(|d| d.plugin.as_ref())
        .and_then(|p| p.state.as_deref().map(|b| b.0.clone()))
}

fn live(core: &mut AppCore<OfflineSink>, device: &DeviceKey) -> Vec<u8> {
    let id = core.plugin_instance(device).unwrap();
    core.plugin_host().manager().capture_state(id).unwrap().0
}

fn preset_core() -> (AppCore<OfflineSink>, Arc<NullStats>, DeviceKey) {
    let (mut core, stats) = null_core(PluginIsolation::InProcess, PluginPaths::default());
    stats.set_presets(true);
    add_plugin(&mut core, GAIN, "echo");
    core.frame();
    core.plugins_capture_all();
    (core, stats, key("echo"))
}

#[test]
fn a_loaded_preset_is_one_undo_step_that_reaches_the_live_plugin() {
    let (mut core, _stats, dev) = preset_core();
    assert_eq!(core.plugin_presets(&dev).len(), 3);
    assert!(core.load_plugin_preset(&dev, "warm"));
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"preset:warm"[..]));
    assert_eq!(live(&mut core, &dev), b"preset:warm");
    assert_eq!(core.plugin_preset_current(&dev), Some("warm"));
    assert_eq!(core.document().undo_label(), Some("Change Plugin Patch"));
    assert!(core.undo());
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b""[..]));
    assert_eq!(
        live(&mut core, &dev),
        b"",
        "undo restored the plugin itself"
    );
    assert_eq!(core.plugin_preset_current(&dev), None);
    assert!(core.redo());
    assert_eq!(live(&mut core, &dev), b"preset:warm");
}

#[test]
fn stepping_goes_round_the_list_and_a_refused_preset_changes_nothing() {
    let (mut core, stats, dev) = preset_core();
    assert!(core.step_plugin_preset(&dev, 1));
    assert_eq!(core.plugin_preset_current(&dev), Some("init"));
    assert!(core.step_plugin_preset(&dev, -1));
    assert_eq!(
        core.plugin_preset_current(&dev),
        Some("bright"),
        "wraps back"
    );
    assert!(core.step_plugin_preset(&dev, 1));
    assert_eq!(
        core.plugin_preset_current(&dev),
        Some("init"),
        "and forward"
    );
    let before = stored(&core, &dev);
    assert!(!core.load_plugin_preset(&dev, "nope"));
    assert!(core.last_rejection().is_some());
    assert_eq!(stored(&core, &dev), before);
    stats.set_presets(false);
    let (mut bare, _s) = null_core(PluginIsolation::InProcess, PluginPaths::default());
    let d = key("echo");
    add_plugin(&mut bare, GAIN, "echo");
    assert!(!bare.step_plugin_preset(&d, 1), "no presets: refused");
    assert!(bare.last_rejection().is_some());
}

#[test]
fn a_patch_changed_in_the_plugin_window_becomes_an_undo_step() {
    let (mut core, _stats, dev) = preset_core();
    let id = core.plugin_instance(&dev).unwrap();
    let patched = atomik_device_sdk::PluginStateBlob(b"patched".to_vec());
    core.plugin_host()
        .manager()
        .restore_state(id, &patched)
        .unwrap();
    post(&mut core, &dev, PluginMessage::StateDirty);
    core.frame();
    core.plugin_tick(10.0);
    core.plugin_tick(12.0);
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"patched"[..]));
    assert_eq!(core.document().undo_label(), Some("Change Plugin Patch"));
    assert!(core.undo());
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b""[..]));
    assert_eq!(live(&mut core, &dev), b"", "the plugin went back with it");
}

#[test]
fn a_param_gesture_state_is_stored_without_a_second_step() {
    let (mut core, _stats, dev) = preset_core();
    let id = core.plugin_instance(&dev).unwrap();
    core.plugins.gesture_touched.insert(dev.clone());
    core.plugin_host()
        .manager()
        .restore_state(id, &atomik_device_sdk::PluginStateBlob(b"turned".to_vec()))
        .unwrap();
    post(&mut core, &dev, PluginMessage::StateDirty);
    core.frame();
    core.plugin_tick(10.0);
    core.plugin_tick(12.0);
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"turned"[..]));
    assert_ne!(
        core.document().undo_label(),
        Some("Change Plugin Patch"),
        "the gesture's own step is the only one"
    );
}

#[test]
fn a_crash_marks_the_plugin_in_the_settings_list_and_retry_clears_the_mark() {
    use atomik_ui::plugin_data::PluginEntryStatus;
    let (mut core, stats, dev) = lost_core();
    let status =
        |core: &AppCore<OfflineSink>| core.plugin_settings_data().plugins[0].status.clone();
    assert_eq!(status(&core), PluginEntryStatus::Ok);
    lose(
        &mut core,
        &stats,
        &dev,
        "the plugin process exited with code 3 (0x3)",
    );
    assert!(matches!(status(&core), PluginEntryStatus::Crashed(r) if r.contains("code 3")));
    assert_eq!(status(&core).label(), "CRASHED");
    let saved = core
        .take_plugin_settings_change()
        .expect("the mark is saved");
    assert!(saved.crashed.contains_key(GAIN));

    // A new run reads the mark from the settings; RETRY clears it and saves.
    let mut again = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    let mut c = config(true, &[PKG]);
    c.settings = saved;
    again.configure_plugins(c);
    assert!(matches!(
        again
            .plugin_settings_data()
            .plugins
            .iter()
            .find(|p| p.plugin.type_id() == GAIN)
            .unwrap()
            .status,
        PluginEntryStatus::Crashed(_)
    ));
    again.handle_action(UiAction::PluginSettings(PluginSettingsAction::RetryPlugin(
        GAIN.into(),
    )));
    let after = again.take_plugin_settings_change().expect("saved");
    assert!(after.crashed.is_empty());
    assert!(
        again
            .plugin_settings_data()
            .plugins
            .iter()
            .all(|p| p.status == PluginEntryStatus::Ok)
    );
}

#[test]
fn the_parameter_pane_lists_the_presets_and_its_actions_load_them() {
    let (mut core, _stats, dev) = preset_core();
    core.open_plugin_params(&dev);
    let names: Vec<String> = core
        .plugin_params()
        .presets()
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert_eq!(names, ["Init", "Warm", "Bright"]);
    assert_eq!(core.plugin_params().preset_name(), None);
    core.handle_action(UiAction::StepPluginPreset {
        device: dev.clone(),
        delta: 1,
    });
    assert_eq!(core.plugin_params().preset_name(), Some("Init"));
    core.handle_action(UiAction::LoadPluginPreset {
        device: dev.clone(),
        preset: "bright".into(),
    });
    assert_eq!(core.plugin_params().preset_name(), Some("Bright"));
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"preset:bright"[..]));
    assert!(core.undo());
    assert_eq!(live(&mut core, &dev), b"preset:init");
    core.frame();
    assert_eq!(
        core.plugin_params().preset_name(),
        None,
        "after an undo the state is not known to be a preset"
    );
}
