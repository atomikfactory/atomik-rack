//! Wiring, sandbox half: where plugins run (read once at start), a crashed device (CRASHED with
//! its reason, never FAILED, never a crash of the app), RELOAD of a device whose process is
//! gone, external editors (the plugin's own window, owned by the app window), and the Settings
//! row. The instances come from the `NullBackend`, so no process is started.

use super::*;
use crate::app_core::plugins::{
    CreatedStages, EDITOR_WAITING, crash_log_line, created_line, created_text, failed_log_line,
    plugin_said_line,
};
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

/// An editor that takes long to open (an online licence check, the plugin's own dialogs) is
/// waited for without blocking the app: the panel says so, OPEN GUI again only repeats it, and
/// the editor counts as open once the plugin says so.
#[test]
fn a_slow_external_editor_is_waited_for_and_then_open() {
    let (mut core, stats, dev) = lost_core();
    stats.set_external_editor(true);
    stats.set_external_editor_delay(3);
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    assert_eq!(core.last_rejection(), Some(EDITOR_WAITING));
    assert_eq!(stats.external_opens(), 0, "not open yet");
    assert!(core.plugin_gui_opening(&dev));
    assert!(
        core.plugin_gui_open(&dev),
        "the panel knows it is on its way"
    );
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    assert_eq!(core.last_rejection(), Some(EDITOR_WAITING));
    for _ in 0..4 {
        core.frame();
    }
    assert_eq!(stats.external_opens(), 1);
    assert!(!core.plugin_gui_opening(&dev));
    assert!(core.plugin_gui_open(&dev));
    assert_eq!(core.take_plugin_window_requests(), vec![]);
}

#[test]
fn a_crash_is_logged_with_its_reason_and_exit_code() {
    let line = crash_log_line(
        "rack.synth",
        "the plugin process exited with code -1073741819 (0xc0000005): boom",
    );
    assert!(line.contains("rack.synth"), "{line}");
    assert!(line.contains("0xc0000005"), "{line}");
    assert!(line.contains("boom"), "{line}");
}

#[test]
fn a_panic_of_the_plugin_in_its_host_is_logged_with_the_key_the_message_and_the_place() {
    let (mut core, _stats, dev) = lost_core();
    let text = "panicked in the plugin host during Process: boom at crates/x/src/a.rs:3:9 [child stderr: thread 'host-audio' panicked]";
    post(
        &mut core,
        &dev,
        PluginMessage::Log {
            level: atomik_plugin_host::LogLevel::Warning,
            text: text.to_string(),
        },
    );
    core.frame();
    assert_eq!(
        plugin_said_line("echo", text),
        format!("plugin [echo] {text}")
    );
    assert!(
        core.plugin_log().iter().any(|l| l.contains(text)),
        "{:?}",
        core.plugin_log()
    );
}

#[test]
fn a_device_that_went_failed_is_logged_with_its_full_reason() {
    assert_eq!(
        failed_log_line("rack.synth", "the plugin returned an error status"),
        "plugin [rack.synth] failed: the plugin returned an error status"
    );
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

#[test]
fn a_process_the_hang_guard_stopped_never_counts_toward_quarantine() {
    let (mut core, stats, dev) = lost_core();
    stats.set_lost(true);
    for _ in 0..(2 * crate::plugins_user::QUARANTINE_AFTER) {
        hang(&mut core, &dev);
        core.wait_plugins();
    }
    assert_eq!(core.plugins_user().failures(GAIN), 0);
    assert!(!core.plugins_user().is_quarantined(GAIN));
}

#[test]
fn real_crashes_in_a_row_still_quarantine_the_plugin() {
    let (mut core, stats, dev) = lost_core();
    for n in 1..=crate::plugins_user::QUARANTINE_AFTER {
        lose(&mut core, &stats, &dev, "exited with code 3");
        core.wait_plugins();
        if n < crate::plugins_user::QUARANTINE_AFTER {
            assert_eq!(core.plugins_user().failures(GAIN), n);
        }
        core.handle_action(UiAction::ReloadPlugin {
            device: dev.clone(),
        });
        core.wait_plugins();
    }
    assert!(core.plugins_user().is_quarantined(GAIN));
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

fn knob(core: &AppCore<OfflineSink>, device: &DeviceKey, param: &str) -> f32 {
    core.doc()
        .param_value(device, param, core.registry())
        .expect("a plugin param")
}

#[test]
fn a_loaded_preset_moves_the_knobs_in_the_same_undo_step() {
    let (mut core, _stats, dev) = preset_core();
    let before = knob(&core, &dev, "p1");
    assert!(core.load_plugin_preset(&dev, "warm"));
    // The second preset puts every param at 20 % of its range (the gain spans -60 to 12 dB).
    assert!((knob(&core, &dev, "p1") - (-45.6)).abs() < 1e-3);
    assert_ne!(knob(&core, &dev, "p1"), before);
    assert_eq!(
        core.document().undo_label(),
        Some("Change Plugin Patch"),
        "the knob changes are part of the patch step"
    );
    assert!(core.undo());
    assert_eq!(
        knob(&core, &dev, "p1"),
        before,
        "one undo puts the knobs back"
    );
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b""[..]));
    assert!(core.redo());
    assert!((knob(&core, &dev, "p1") - (-45.6)).abs() < 1e-3);
    assert_eq!(live(&mut core, &dev), b"preset:warm");
}

#[test]
fn a_patch_changed_in_the_plugin_window_moves_the_knobs_too() {
    let (mut core, _stats, dev) = preset_core();
    let before = knob(&core, &dev, "p1");
    let id = core.plugin_instance(&dev).unwrap();
    core.plugin_host()
        .manager()
        .load_preset(id, "bright")
        .unwrap();
    post(&mut core, &dev, PluginMessage::StateDirty);
    core.frame();
    core.plugin_tick(10.0);
    core.plugin_tick(12.0);
    assert!((knob(&core, &dev, "p1") - (-60.0 + 72.0 * 0.3)).abs() < 1e-3);
    assert!(core.undo());
    assert_eq!(knob(&core, &dev, "p1"), before);
}

#[test]
fn a_patch_browser_aimed_at_a_plugin_lists_its_presets_and_loads_one_on_activation() {
    use atomik_library::{FileKind, preset_of};
    let (mut core, _stats, dev) = preset_core();
    let before = knob(&core, &dev, "p1");
    core.begin_patch_browse(&dev);
    let rows = |core: &AppCore<OfflineSink>| -> Vec<atomik_library::FileRow> {
        let mut out = Vec::new();
        for call in &core.browser.trace {
            if let super::browser::ViewCall::SetRows(r) = call {
                out.clone_from(r);
            }
        }
        out
    };
    let listed = rows(&core);
    let presets: Vec<_> = listed
        .iter()
        .filter(|r| r.kind == FileKind::Preset)
        .collect();
    let names: Vec<&str> = presets.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Init", "Warm", "Bright"], "{listed:?}");
    assert_eq!(presets[1].item.as_ref().unwrap().detail, "Factory");
    assert_eq!(preset_of(&presets[1].path), Some(("echo", "warm")));

    core.browse(atomik_ui::BrowseAction::Load(presets[1].path.clone()));
    assert_eq!(live(&mut core, &dev), b"preset:warm");
    assert!((knob(&core, &dev, "p1") - (-45.6)).abs() < 1e-3);
    assert_eq!(core.document().undo_label(), Some("Change Plugin Patch"));
    assert_eq!(
        core.patch_browse_device(),
        Some(&dev),
        "the browser stays aimed at the plugin"
    );
    assert!(core.undo());
    assert_eq!(knob(&core, &dev, "p1"), before);

    // A preset of a plugin that is not there changes nothing and says so.
    let gone = atomik_library::preset_uri("nobody", "warm");
    core.browse(atomik_ui::BrowseAction::Load(gone));
    assert!(core.last_rejection().is_some());
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

#[test]
fn the_mode_plugins_really_run_in_is_shown_and_a_missing_sandbox_is_said_once() {
    let (mut core, _) = null_core(sandbox(), PluginPaths::default());
    assert_eq!(core.plugin_mode(), Some("sandbox"));
    assert!(core.plugin_settings_data().running_sandboxed);
    let (mut bare, _) = null_core(PluginIsolation::InProcess, PluginPaths::default());
    assert_eq!(bare.plugin_mode(), Some("in-process"));
    // The switch still says on: the settings tell what runs, not what was asked for.
    assert!(bare.plugin_settings_data().sandbox);
    assert!(!bare.plugin_settings_data().running_sandboxed);
    bare.warn_plugin_isolation("The plugin sandbox is on but cannot start");
    assert!(
        bare.last_rejection()
            .is_some_and(|m| m.contains("cannot start"))
    );
    assert!(
        bare.plugin_settings_data()
            .log
            .iter()
            .any(|l| l.contains("cannot start")),
        "also in the plugin log"
    );
    // Plugins off: nothing to say about where they run.
    core.handle_action(UiAction::PluginSettings(PluginSettingsAction::SetEnabled {
        enabled: false,
    }));
    assert_eq!(core.plugin_mode(), None);
}

// ------------------------------------------------------------------ two instances of one plugin

/// A core with two instances of the test plugin ("echo" and "echo2"), both with an external
/// editor.
fn two_instances() -> (AppCore<OfflineSink>, Arc<NullStats>, DeviceKey, DeviceKey) {
    let (mut core, stats, a) = lost_core();
    add_plugin(&mut core, GAIN, "echo2");
    core.frame();
    stats.set_external_editor(true);
    core.frame();
    (core, stats, a, key("echo2"))
}

fn open_gui(core: &mut AppCore<OfflineSink>, device: &DeviceKey) {
    core.handle_action(UiAction::OpenPluginGui {
        device: device.clone(),
    });
}

/// The editors of two instances of one plugin, both slow to open, are opened one request each:
/// the second does not wait for, repeat or answer for the first.
#[test]
fn two_slow_editors_of_one_plugin_open_once_each() {
    let (mut core, stats, a, b) = two_instances();
    stats.set_external_editor_delay(3);
    open_gui(&mut core, &a);
    assert!(core.plugin_gui_opening(&a));
    assert!(!core.plugin_gui_opening(&b), "B was not asked");
    open_gui(&mut core, &b);
    assert!(core.plugin_gui_opening(&a) && core.plugin_gui_opening(&b));
    assert_eq!(stats.external_opens(), 0);
    for _ in 0..10 {
        core.frame();
    }
    assert_eq!(
        stats.external_opens(),
        2,
        "one open per instance, no repeats"
    );
    assert!(!core.plugin_gui_opening(&a) && !core.plugin_gui_opening(&b));
    assert!(core.plugin_gui_open(&a) && core.plugin_gui_open(&b));
    assert_eq!(core.take_plugin_window_requests(), vec![]);
}

/// B is asked while A still opens: A's pending open is not asked again, B's opens by itself.
#[test]
fn asking_for_b_while_a_opens_neither_repeats_a_nor_waits_for_it() {
    let (mut core, stats, a, b) = two_instances();
    stats.set_external_editor_delay(3);
    open_gui(&mut core, &a);
    core.frame();
    open_gui(&mut core, &b);
    // A is asked again while it opens: only the toast, no second open.
    open_gui(&mut core, &a);
    assert_eq!(core.last_rejection(), Some(EDITOR_WAITING));
    for _ in 0..10 {
        core.frame();
    }
    assert_eq!(stats.external_opens(), 2);
    assert!(core.plugin_gui_open(&a) && core.plugin_gui_open(&b));
}

/// A fast editor A is open; B is slow: A stays open and can be raised while B opens.
#[test]
fn a_slow_second_editor_does_not_disturb_the_open_first() {
    let (mut core, stats, a, b) = two_instances();
    open_gui(&mut core, &a);
    assert_eq!(stats.external_opens(), 1);
    stats.set_external_editor_delay(3);
    open_gui(&mut core, &b);
    assert!(core.plugin_gui_opening(&b));
    assert!(!core.plugin_gui_opening(&a));
    assert!(core.plugin_gui_open(&a));
    core.frame();
    assert!(core.plugin_gui_open(&a), "A stays open while B opens");
    for _ in 0..5 {
        core.frame();
    }
    assert_eq!(stats.external_opens(), 2);
    assert!(core.plugin_gui_open(&b) && !core.plugin_gui_opening(&b));
}

/// OPEN GUI of an editor that is open raises it: one more request for that instance, none for
/// the other.
#[test]
fn open_gui_of_an_open_editor_raises_it_and_leaves_the_other_alone() {
    let (mut core, stats, a, b) = two_instances();
    open_gui(&mut core, &a);
    open_gui(&mut core, &b);
    assert_eq!(stats.external_opens(), 2);
    open_gui(&mut core, &b);
    assert_eq!(stats.external_opens(), 3, "B raised, A untouched");
    core.frame();
    core.frame();
    assert_eq!(stats.external_opens(), 3, "no request without a click");
    assert!(core.plugin_gui_open(&a) && core.plugin_gui_open(&b));
}

/// What one plugin says about its editor is routed to its own device: A closing its editor
/// leaves B open, and B closing leaves A open.
#[test]
fn an_editor_closing_in_one_instance_leaves_the_other_open() {
    let (mut core, _stats, a, b) = two_instances();
    open_gui(&mut core, &a);
    open_gui(&mut core, &b);
    post(&mut core, &a, PluginMessage::GuiClosed);
    core.frame();
    assert!(!core.plugin_gui_open(&a));
    assert!(core.plugin_gui_open(&b), "B stays open");
    open_gui(&mut core, &a);
    assert!(core.plugin_gui_open(&a));
    post(&mut core, &b, PluginMessage::GuiClosed);
    core.frame();
    assert!(core.plugin_gui_open(&a), "A stays open");
    assert!(!core.plugin_gui_open(&b));
}

/// Closing A from the app (a removed device) closes A's editor only.
#[test]
fn removing_a_device_closes_its_editor_and_not_the_others() {
    let (mut core, stats, a, b) = two_instances();
    open_gui(&mut core, &a);
    open_gui(&mut core, &b);
    core.close_plugin_gui(&a);
    assert_eq!(stats.external_closes(), 1);
    assert!(!core.plugin_gui_open(&a));
    assert!(core.plugin_gui_open(&b));
    apply(&mut core, Command::RemoveDevice { key: a.clone() });
    assert!(core.plugin_gui_open(&b));
}

/// The editor that opens after the user closed its device's editor is closed again: it is
/// not left open without the app knowing.
#[test]
fn an_editor_that_opens_after_it_was_closed_is_closed_again() {
    let (mut core, stats, a, b) = two_instances();
    stats.set_external_editor_delay(3);
    open_gui(&mut core, &a);
    open_gui(&mut core, &b);
    core.close_plugin_gui(&a);
    assert!(!core.plugin_gui_open(&a));
    for _ in 0..10 {
        core.frame();
    }
    assert!(!core.plugin_gui_open(&a), "A is closed");
    assert!(core.plugin_gui_open(&b), "B opened as asked");
    assert!(
        stats.external_closes() == 2,
        "A closed by the user, then its late editor closed again"
    );
}

/// The lines that tell a real run which editor was asked for, opened, failed and closed.
#[test]
fn the_editor_log_lines_carry_the_device_key_the_instance_and_the_process() {
    use crate::app_core::plugins::{
        editor_closed_line, editor_failed_line, editor_opened_line, editor_requested_line,
    };
    use atomik_plugin_host::InstanceId;
    let id = InstanceId(4);
    assert_eq!(
        editor_requested_line("echo2", id, Some(1234), false),
        "plugin [echo2] editor requested: instance 4, process 1234"
    );
    assert_eq!(
        editor_requested_line("echo2", id, None, true),
        "plugin [echo2] editor requested: instance 4, process none (already open or opening)"
    );
    assert_eq!(
        editor_opened_line("echo2", id),
        "plugin [echo2] editor opened: instance 4"
    );
    assert_eq!(
        editor_failed_line("echo2", id, "busy"),
        "plugin [echo2] editor did not open: instance 4: busy"
    );
    assert_eq!(
        editor_closed_line("echo2", Some(id)),
        "plugin [echo2] editor closed: instance 4"
    );
}

// ------------------------------------------------------------------ learn in the window

/// An external editor (the plugin's own process shows it): its gesture begin (0, the sandbox
/// marker) is the touch of LEARN MIDI, and the child closing its editor disarms the window.
#[test]
fn learn_midi_takes_the_touch_of_an_external_editor_and_its_close_disarms_it() {
    let (mut core, stats, dev) = lost_core();
    stats.set_external_editor(true);
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    assert!(core.plugin_gui_open(&dev));
    assert!(core.toggle_plugin_learn(&dev, atomik_ui::PluginLearnMode::Midi));
    let param = {
        let d = core.doc().resolve(&dev).unwrap();
        let desc = core.registry().get(&d.type_id).unwrap().descriptor;
        desc.params
            .iter()
            .find(|p| {
                p.key
                    .strip_prefix('p')
                    .is_some_and(|n| n.parse::<u32>().is_ok())
            })
            .expect("a plugin param")
            .key
            .to_owned()
    };
    let id: u32 = param[1..].parse().unwrap();
    post(
        &mut core,
        &dev,
        PluginMessage::ParamEdit {
            id,
            value: 0.0,
            phase: atomik_plugin_host::ParamPhase::Begin,
        },
    );
    core.frame();
    assert_eq!(
        core.midi_armed(),
        Some(&atomik_model::MidiTarget::Param {
            device: dev.clone(),
            param,
        })
    );
    post(&mut core, &dev, PluginMessage::GuiClosed);
    core.frame();
    assert_eq!(core.plugin_learn_mode(&dev), None);
    assert!(core.midi_armed().is_none(), "the waiting target goes too");
}

/// The frame the plugin's process says its editor window has is where the Learn strip docks;
/// it goes with the editor.
#[test]
fn an_external_editors_frame_is_kept_while_it_is_open() {
    let (mut core, stats, dev) = lost_core();
    stats.set_external_editor(true);
    post(
        &mut core,
        &dev,
        PluginMessage::GuiFrame {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
        },
    );
    core.frame();
    assert!(core.external_editor_frames().is_empty(), "no editor yet");
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
    post(
        &mut core,
        &dev,
        PluginMessage::GuiFrame {
            x: -8,
            y: 100,
            w: 656,
            h: 519,
        },
    );
    core.frame();
    assert_eq!(
        core.external_editor_frames(),
        vec![(dev.clone(), (-8, 100, 656, 519))]
    );
    post(&mut core, &dev, PluginMessage::GuiClosed);
    core.frame();
    assert!(core.external_editor_frames().is_empty());
}

// ------------------------------------------------------------------ editors inside the app window

use atomik_ui::plugin_data::PluginEditorHome;
use atomik_ui::{PluginDockRequest, PluginDockUi};

/// A core whose gain plugin has an external editor, on a system that can dock.
fn dock_core(home: PluginEditorHome) -> (AppCore<OfflineSink>, Arc<NullStats>, DeviceKey) {
    let (mut core, stats, dev) = lost_core();
    stats.set_external_editor(true);
    core.set_plugin_docking_available(true);
    core.handle_action(UiAction::PluginSettings(
        PluginSettingsAction::SetEditorHome(home),
    ));
    core.frame();
    (core, stats, dev)
}

fn open(core: &mut AppCore<OfflineSink>, dev: &DeviceKey) {
    core.handle_action(UiAction::OpenPluginGui {
        device: dev.clone(),
    });
}

#[test]
fn where_the_system_cannot_dock_every_editor_opens_in_its_own_window_whatever_the_setting() {
    let (mut core, stats, dev) = lost_core();
    stats.set_external_editor(true);
    core.handle_action(UiAction::PluginSettings(
        PluginSettingsAction::SetEditorHome(PluginEditorHome::InApp),
    ));
    assert!(!core.plugin_docking_available());
    assert_eq!(core.editor_placement(&dev), EditorPlacement::OwnWindow);
    open(&mut core, &dev);
    assert_eq!(stats.external_opens(), 1);
    assert_eq!(stats.embedded_opens(), 0);
    assert_eq!(core.take_plugin_dock_requests(), vec![]);
    assert_eq!(core.plugin_dock_ui(&dev), PluginDockUi::Hidden);
    // The settings row says it is not available.
    assert!(!core.plugin_settings_data().docking_available);
    // DOCK IN APP says why it does nothing.
    core.move_plugin_editor(&dev, EditorPlacement::Docked);
    assert!(
        core.last_rejection()
            .is_some_and(|m| m.contains("macOS and Wayland")),
        "{:?}",
        core.last_rejection()
    );
}

#[test]
fn the_setting_is_saved_and_an_editor_follows_it_when_it_opens() {
    let (mut core, _stats, dev) = dock_core(PluginEditorHome::OwnWindow);
    assert_eq!(core.editor_placement(&dev), EditorPlacement::OwnWindow);
    assert_eq!(
        core.plugin_settings_data().editor_home,
        PluginEditorHome::OwnWindow
    );
    core.take_plugin_settings_change();
    core.handle_action(UiAction::PluginSettings(
        PluginSettingsAction::SetEditorHome(PluginEditorHome::InApp),
    ));
    assert_eq!(core.editor_placement(&dev), EditorPlacement::Docked);
    let saved = core.take_plugin_settings_change().expect("saved");
    assert_eq!(saved.editors, atomik_plugin_host::EditorHome::InApp);
}

#[test]
fn opening_a_docked_editor_asks_for_a_frame_then_opens_the_editor_inside_it() {
    let (mut core, stats, dev) = dock_core(PluginEditorHome::InApp);
    open(&mut core, &dev);
    assert_eq!(
        core.take_plugin_dock_requests(),
        vec![DockRequest::Create {
            device: dev.clone(),
            size: (640, 480),
            at: None
        }]
    );
    assert_eq!(stats.external_opens(), 0, "nothing opens before the frame");
    assert!(core.plugin_gui_open(&dev));
    assert_eq!(core.docked_editor(&dev), None, "no frame yet");
    core.plugin_dock_created(&dev, owner());
    assert_eq!(stats.embedded_opens(), 1);
    assert_eq!(stats.embedded_parent(), Some(owner()));
    assert!(core.plugin_editor_docked(&dev));
    assert_eq!(core.plugin_dock_ui(&dev), PluginDockUi::Docked);
    assert_eq!(core.docked_editor(&dev).map(|d| d.size), Some((640, 480)));
    // No companion window for the Learn strip: the frame carries it.
    assert!(core.external_editor_frames().is_empty());
    // Asking again opens nothing new.
    open(&mut core, &dev);
    assert_eq!(stats.embedded_opens(), 1);
}

#[test]
fn a_slow_docked_editor_is_waited_for_inside_its_frame() {
    let (mut core, stats, dev) = dock_core(PluginEditorHome::InApp);
    stats.set_external_editor_delay(3);
    open(&mut core, &dev);
    core.take_plugin_dock_requests();
    core.plugin_dock_created(&dev, owner());
    assert!(core.plugin_gui_opening(&dev));
    assert_eq!(stats.embedded_opens(), 0, "not open yet");
    for _ in 0..4 {
        core.frame();
    }
    assert!(!core.plugin_gui_opening(&dev));
    assert_eq!(stats.embedded_opens(), 1);
    assert_eq!(stats.embedded_parent(), Some(owner()));
    assert!(core.plugin_editor_docked(&dev), "the frame stays");
}

#[test]
fn the_plugin_asking_for_a_size_resizes_the_frame() {
    let (mut core, _stats, dev) = dock_core(PluginEditorHome::InApp);
    open(&mut core, &dev);
    core.take_plugin_dock_requests();
    core.plugin_dock_created(&dev, owner());
    post(&mut core, &dev, PluginMessage::GuiResize { w: 900, h: 500 });
    core.frame();
    assert_eq!(
        core.take_plugin_dock_requests(),
        vec![DockRequest::Resize {
            device: dev.clone(),
            w: 900,
            h: 500
        }]
    );
    assert_eq!(core.docked_editor(&dev).map(|d| d.size), Some((900, 500)));
    assert_eq!(
        core.take_plugin_window_requests(),
        vec![],
        "no window of the app's own is involved"
    );
}

#[test]
fn dragging_the_frame_asks_the_plugin_and_a_fixed_size_editor_springs_back() {
    let (mut core, stats, dev) = dock_core(PluginEditorHome::InApp);
    open(&mut core, &dev);
    core.take_plugin_dock_requests();
    core.plugin_dock_created(&dev, owner());
    core.plugin_dock_user_resized(&dev, 700, 520);
    assert_eq!(stats.gui_sizes(), vec![(700, 520)]);
    assert_eq!(core.docked_editor(&dev).map(|d| d.size), Some((700, 520)));
    core.take_plugin_dock_requests();
    stats.set_fixed_gui_size(Some((640, 480)));
    core.plugin_dock_user_resized(&dev, 800, 600);
    assert_eq!(
        core.take_plugin_dock_requests(),
        vec![DockRequest::Resize {
            device: dev.clone(),
            w: 640,
            h: 480
        }]
    );
    assert_eq!(core.docked_editor(&dev).map(|d| d.size), Some((640, 480)));
}

#[test]
fn closing_a_docked_editor_removes_its_frame_and_the_frame_comes_back_where_it_was() {
    let (mut core, stats, dev) = dock_core(PluginEditorHome::InApp);
    open(&mut core, &dev);
    core.take_plugin_dock_requests();
    core.plugin_dock_created(&dev, owner());
    core.plugin_dock_moved(&dev, 120, 90);
    post(&mut core, &dev, PluginMessage::GuiResize { w: 900, h: 500 });
    core.frame();
    core.take_plugin_dock_requests();
    core.handle_action(UiAction::PluginDock {
        device: dev.clone(),
        request: PluginDockRequest::Close,
    });
    assert!(!core.plugin_gui_open(&dev));
    assert!(!core.plugin_editor_docked(&dev));
    assert_eq!(stats.external_closes(), 1);
    assert_eq!(
        core.take_plugin_dock_requests(),
        vec![DockRequest::Close {
            device: dev.clone()
        }]
    );
    open(&mut core, &dev);
    assert_eq!(
        core.take_plugin_dock_requests(),
        vec![DockRequest::Create {
            device: dev.clone(),
            size: (900, 500),
            at: Some((120, 90))
        }]
    );
}

#[test]
fn the_child_closing_a_docked_editor_removes_the_frame() {
    let (mut core, _stats, dev) = dock_core(PluginEditorHome::InApp);
    open(&mut core, &dev);
    core.take_plugin_dock_requests();
    core.plugin_dock_created(&dev, owner());
    post(&mut core, &dev, PluginMessage::GuiClosed);
    core.frame();
    assert!(!core.plugin_editor_docked(&dev));
    assert_eq!(
        core.take_plugin_dock_requests(),
        vec![DockRequest::Close {
            device: dev.clone()
        }]
    );
}

#[test]
fn a_plugin_that_stopped_leaves_a_frame_that_says_so_until_it_is_closed() {
    let (mut core, stats, dev) = dock_core(PluginEditorHome::InApp);
    open(&mut core, &dev);
    core.take_plugin_dock_requests();
    core.plugin_dock_created(&dev, owner());
    lose(&mut core, &stats, &dev, "exited with code 3");
    assert_eq!(
        core.take_plugin_dock_requests(),
        vec![DockRequest::Stopped {
            device: dev.clone()
        }]
    );
    assert_eq!(core.plugin_dock_ui(&dev), PluginDockUi::Stopped);
    assert_eq!(core.docked_editor(&dev).map(|d| d.stopped), Some(true));
    // A frame that stopped is not asked to resize.
    core.plugin_dock_user_resized(&dev, 700, 500);
    assert!(stats.gui_sizes().is_empty());
    core.handle_action(UiAction::PluginDock {
        device: dev.clone(),
        request: PluginDockRequest::Close,
    });
    assert_eq!(
        core.take_plugin_dock_requests(),
        vec![DockRequest::Close {
            device: dev.clone()
        }]
    );
}

#[test]
fn an_editor_moves_between_its_own_window_and_the_app_window_and_the_choice_is_remembered() {
    let (mut core, stats, dev) = dock_core(PluginEditorHome::OwnWindow);
    open(&mut core, &dev);
    assert_eq!(stats.external_opens(), 1);
    assert_eq!(core.plugin_dock_ui(&dev), PluginDockUi::CanDock);
    // DOCK IN APP closes the window of its own and asks for a frame.
    core.handle_action(UiAction::PluginDock {
        device: dev.clone(),
        request: PluginDockRequest::Dock,
    });
    assert_eq!(stats.external_closes(), 1);
    assert!(matches!(
        core.take_plugin_dock_requests().as_slice(),
        [DockRequest::Create { .. }]
    ));
    core.plugin_dock_created(&dev, owner());
    assert_eq!(stats.embedded_opens(), 1);
    // POP OUT removes the frame and opens the editor in a window of its own again.
    core.handle_action(UiAction::PluginDock {
        device: dev.clone(),
        request: PluginDockRequest::PopOut,
    });
    assert_eq!(
        core.take_plugin_dock_requests(),
        vec![DockRequest::Close {
            device: dev.clone()
        }]
    );
    assert_eq!(stats.external_opens(), 3, "own, docked, own again");
    assert_eq!(stats.embedded_opens(), 1);
    assert_eq!(core.editor_placement(&dev), EditorPlacement::OwnWindow);
    // The choice survives the editor closing, whatever the setting says.
    core.handle_action(UiAction::PluginSettings(
        PluginSettingsAction::SetEditorHome(PluginEditorHome::InApp),
    ));
    core.close_plugin_gui(&dev);
    assert_eq!(core.editor_placement(&dev), EditorPlacement::OwnWindow);
    open(&mut core, &dev);
    assert_eq!(core.take_plugin_dock_requests(), vec![]);
}

#[test]
fn moving_a_closed_editor_only_remembers_where_it_opens_next() {
    let (mut core, stats, dev) = dock_core(PluginEditorHome::OwnWindow);
    core.move_plugin_editor(&dev, EditorPlacement::Docked);
    assert_eq!(stats.external_opens(), 0);
    open(&mut core, &dev);
    assert!(matches!(
        core.take_plugin_dock_requests().as_slice(),
        [DockRequest::Create { .. }]
    ));
}

#[test]
fn a_frame_the_platform_could_not_make_leaves_nothing_open() {
    let (mut core, stats, dev) = dock_core(PluginEditorHome::InApp);
    open(&mut core, &dev);
    core.take_plugin_dock_requests();
    core.plugin_dock_failed(&dev, "no handle");
    assert!(!core.plugin_gui_open(&dev));
    assert!(!core.plugin_editor_docked(&dev));
    assert_eq!(stats.embedded_opens(), 0);
    assert!(
        core.last_rejection()
            .is_some_and(|m| m.contains("no handle")),
        "{:?}",
        core.last_rejection()
    );
    // It can be opened again.
    open(&mut core, &dev);
    assert!(matches!(
        core.take_plugin_dock_requests().as_slice(),
        [DockRequest::Close { .. }, DockRequest::Create { .. }]
    ));
}

#[test]
fn removing_the_device_removes_the_frame() {
    let (mut core, _stats, dev) = dock_core(PluginEditorHome::InApp);
    open(&mut core, &dev);
    core.take_plugin_dock_requests();
    core.plugin_dock_created(&dev, owner());
    apply(&mut core, Command::RemoveDevice { key: dev.clone() });
    assert!(!core.plugin_editor_docked(&dev));
    assert_eq!(
        core.take_plugin_dock_requests(),
        vec![DockRequest::Close {
            device: dev.clone()
        }]
    );
}

#[test]
fn a_stall_record_of_a_plugin_call_becomes_a_line_of_the_plugin_log() {
    let (mut core, _stats, dev) = lost_core();
    let before = crate::ui_stall::snapshot().plugin_stalls;
    post(
        &mut core,
        &dev,
        PluginMessage::Stall {
            call: "CaptureState".to_string(),
            ms: 10_012,
            outcome: atomik_plugin_host::StallOutcome::TimedOut,
        },
    );
    core.frame();
    let want = "ui stall 10012 ms: CaptureState [echo] (timed out)";
    assert!(
        core.plugin_log().iter().any(|l| l == want),
        "{:?}",
        core.plugin_log()
    );
    assert!(crate::ui_stall::snapshot().plugin_stalls > before);
    assert!(crate::ui_stall::snapshot().plugin_longest_ms >= 10_012);
}

#[test]
fn the_created_line_names_the_stages_of_a_load() {
    use std::time::Duration;
    let ms = Duration::from_millis;
    let stages = CreatedStages {
        spawn: ms(40),
        instantiate: ms(1_900),
        restore: ms(0),
        activate: ms(310),
        attach: ms(60),
    };
    assert_eq!(
        created_text(ms(2_310), &stages),
        "created in 2.31 s (spawn 0.04, instantiate 1.90, restore 0.00, activate 0.31, attach 0.06)"
    );
    assert_eq!(
        created_line("echo", ms(2_310), &stages),
        "plugin [echo] created in 2.31 s (spawn 0.04, instantiate 1.90, restore 0.00, activate 0.31, attach 0.06)"
    );
}

// ------------------------------------------------------------------ cosmetic answers that come late

/// The ids of the params the pane shows a plugin text for now.
fn texted(core: &AppCore<OfflineSink>) -> Vec<u32> {
    (0..64)
        .filter(|id| core.plugin_params().shown_text(*id).is_some())
        .collect()
}

#[test]
fn the_parameter_pane_shows_a_late_text_when_it_lands_and_never_a_text_for_another_value() {
    let (mut core, stats, dev) = lost_core();
    stats.set_soft_busy(true);
    core.handle_action(UiAction::OpenPluginParams {
        device: dev.clone(),
    });
    core.frame();
    assert!(
        texted(&core).is_empty(),
        "the plugin is busy: the pane shows the numbers"
    );
    // The plugin is free: the texts it owes arrive in the next frame.
    stats.set_soft_busy(false);
    core.frame();
    assert_eq!(texted(&core), vec![1, 2, 3, 4], "every row got its text");
    assert_eq!(core.plugin_params().shown_text(1), Some("0"));
    // Busy again, the gain changes twice before the plugin answers: the answers come in order
    // and only the one for the value the row shows now is kept.
    stats.set_soft_busy(true);
    set_param(&mut core, "echo", "p1", -6.0);
    core.frame();
    assert_eq!(core.plugin_params().shown_text(1), None, "not the old text");
    set_param(&mut core, "echo", "p1", -12.0);
    core.frame();
    assert_eq!(core.plugin_params().shown_text(1), None);
    stats.set_soft_busy(false);
    core.frame();
    assert_eq!(
        core.plugin_params().shown_text(1),
        Some("-12"),
        "the text of the value the row shows"
    );
}

#[test]
fn a_busy_plugin_leaves_no_parameter_text_or_resize_in_the_stall_log() {
    let (mut core, stats, dev) = dock_core(PluginEditorHome::InApp);
    open(&mut core, &dev);
    core.take_plugin_dock_requests();
    core.plugin_dock_created(&dev, owner());
    stats.set_soft_busy(true);
    core.handle_action(UiAction::OpenPluginParams {
        device: dev.clone(),
    });
    // Drain what the setting up logged, then work against a plugin that answers nothing.
    core.frame();
    let _ = core.plugin_log();
    for n in 0..30 {
        core.plugin_dock_user_resized(&dev, 600 + n, 400);
        set_param(&mut core, "echo", "p1", -(n as f32));
        core.frame();
    }
    let stalls: Vec<String> = core
        .plugin_log()
        .into_iter()
        .filter(|l| l.contains("ui stall"))
        .collect();
    assert!(
        stalls
            .iter()
            .all(|l| !l.contains("ParamText") && !l.contains("ResizeGui")),
        "{stalls:?}"
    );
    // Only the size the user stopped at was asked of the plugin, once it was free.
    assert!(stats.gui_sizes().is_empty());
    stats.set_soft_busy(false);
    core.frame();
    assert_eq!(stats.gui_sizes(), vec![(629, 400)]);
    assert_eq!(core.docked_editor(&dev).map(|d| d.size), Some((629, 400)));
}

#[test]
fn a_size_that_a_busy_editor_takes_later_is_applied_and_a_fixed_one_springs_the_frame_back() {
    let (mut core, stats, dev) = dock_core(PluginEditorHome::InApp);
    open(&mut core, &dev);
    core.take_plugin_dock_requests();
    core.plugin_dock_created(&dev, owner());
    let start = core.docked_editor(&dev).map(|d| d.size);
    stats.set_soft_busy(true);
    core.plugin_dock_user_resized(&dev, 700, 520);
    core.plugin_dock_user_resized(&dev, 800, 600);
    assert_eq!(
        core.docked_editor(&dev).map(|d| d.size),
        start,
        "no answer yet"
    );
    assert_eq!(core.take_plugin_dock_requests(), vec![]);
    stats.set_fixed_gui_size(Some((640, 480)));
    stats.set_soft_busy(false);
    core.frame();
    assert_eq!(
        core.take_plugin_dock_requests(),
        vec![DockRequest::Resize {
            device: dev.clone(),
            w: 640,
            h: 480
        }]
    );
    assert_eq!(core.docked_editor(&dev).map(|d| d.size), Some((640, 480)));
}

// ------------------------------------------------------------------ one reload after a crash

/// The process of `device` ends at the frame-clock time `at` (the device and the manager post
/// what they do); the frame then answers it, and the plugin the null backend makes next is a
/// healthy one.
fn crash_at(
    core: &mut AppCore<OfflineSink>,
    stats: &NullStats,
    device: &DeviceKey,
    at: f64,
    reason: &str,
) {
    core.plugin_tick(at);
    lose(core, stats, device, reason);
    stats.set_lost(false);
    core.wait_plugins();
    settle(core);
}

fn crash_lines(core: &AppCore<OfflineSink>) -> usize {
    core.plugin_log()
        .iter()
        .filter(|l| l.contains("] crashed: "))
        .count()
}

fn log_has(core: &AppCore<OfflineSink>, text: &str) -> usize {
    core.plugin_log()
        .iter()
        .filter(|l| l.contains(text))
        .count()
}

#[test]
fn a_plugin_that_crashes_after_running_a_while_is_reloaded_once_with_its_state() {
    let (mut core, stats, dev) = lost_core();
    let before = core.plugin_instance(&dev).unwrap();
    let patched = atomik_device_sdk::PluginStateBlob(b"patched".to_vec());
    core.plugin_host()
        .manager()
        .restore_state(before, &patched)
        .unwrap();
    post(&mut core, &dev, PluginMessage::StateDirty);
    core.frame();
    core.plugin_tick(10.0);
    core.plugin_tick(12.0);
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"patched"[..]));

    crash_at(&mut core, &stats, &dev, 20.0, "exited with code 3");
    let after = core.plugin_instance(&dev).expect("a new instance");
    assert_ne!(after, before);
    assert_eq!(stats.created(), 2, "reloaded once");
    assert!(!core.plugin_host().manager_ref().is_lost(after));
    assert_eq!(core.plugin_crash(&dev), None, "it runs again");
    assert!(core.view().plugin_status(&dev).crashed.is_none());
    assert_eq!(live(&mut core, &dev), b"patched", "with its last state");
    assert_eq!(crash_lines(&core), 1, "one line per death");
    assert_eq!(log_has(&core, "reloading it once from its last state"), 1);
    let notice = core.last_rejection().unwrap_or_default().to_owned();
    assert!(
        notice.ends_with("crashed and was reloaded from its last state."),
        "{notice}"
    );
    // Nothing is said or done again by later frames.
    let lines = core.plugin_log().len();
    settle(&mut core);
    assert_eq!(core.plugin_log().len(), lines);
    assert_eq!(stats.created(), 2);
    // Quarantine: the crash was counted once, the reload added nothing.
    assert_eq!(core.plugins_user().failures(GAIN), 1);
}

#[test]
fn a_second_crash_inside_the_window_stays_crashed_until_reload_gives_the_allowance_back() {
    let (mut core, stats, dev) = lost_core();
    stats.set_lost(true);
    crash_at(&mut core, &stats, &dev, 20.0, "exited with code 3");
    assert_eq!(stats.created(), 2);

    // Ready again at clock 20: a crash 30 s later is inside the window.
    stats.set_lost(true);
    core.plugin_tick(50.0);
    lose(&mut core, &stats, &dev, "killed by signal 9");
    assert_eq!(stats.created(), 2, "not reloaded again");
    assert_eq!(core.plugin_crash(&dev), Some("killed by signal 9"));
    assert_eq!(log_has(&core, "crashed again within 5 minutes"), 1);
    let notice = core.last_rejection().unwrap_or_default().to_owned();
    assert!(notice.contains("stays crashed"), "{notice}");
    assert_eq!(crash_lines(&core), 2);

    // RELOAD works and gives the allowance back.
    core.handle_action(UiAction::ReloadPlugin {
        device: dev.clone(),
    });
    stats.set_lost(false);
    core.wait_plugins();
    settle(&mut core);
    assert_eq!(stats.created(), 3);
    assert_eq!(core.plugin_crash(&dev), None);
    // (Two crashes are counted; a third would quarantine it, which another test covers.)
    core.plugin_worked(GAIN);
    stats.set_lost(true);
    crash_at(&mut core, &stats, &dev, 80.0, "exited with code 3");
    assert_eq!(stats.created(), 4, "reloaded by itself once more");
}

#[test]
fn a_crash_after_the_window_is_reloaded_again() {
    let (mut core, stats, dev) = lost_core();
    crash_at(&mut core, &stats, &dev, 20.0, "exited with code 3");
    assert_eq!(stats.created(), 2);
    stats.set_lost(true);
    let later = 20.0 + crate::app_core::plugins::CRASH_RELOAD_WINDOW_SECS + 1.0;
    crash_at(&mut core, &stats, &dev, later, "exited with code 3");
    assert_eq!(stats.created(), 3);
}

#[test]
fn a_crash_before_the_minimum_alive_time_is_not_reloaded() {
    let (mut core, stats, dev) = lost_core();
    // The instance was ready at clock 0: five seconds is too early.
    core.plugin_tick(5.0);
    lose(&mut core, &stats, &dev, "exited with code 3");
    core.wait_plugins();
    assert_eq!(stats.created(), 1);
    assert_eq!(core.plugin_crash(&dev), Some("exited with code 3"));
    assert_eq!(log_has(&core, "had run for less than 10 s"), 1);
    assert_eq!(log_has(&core, "reloading it once"), 0);

    // A reload that dies at once is not followed by another one.
    core.handle_action(UiAction::ReloadPlugin {
        device: dev.clone(),
    });
    stats.set_lost(false);
    core.wait_plugins();
    settle(&mut core);
    assert_eq!(stats.created(), 2);
    stats.set_lost(true);
    core.plugin_tick(8.0);
    lose(&mut core, &stats, &dev, "exited with code 3");
    core.wait_plugins();
    assert_eq!(stats.created(), 2);
}

#[test]
fn the_crashes_count_toward_quarantine_and_a_quarantined_plugin_is_not_reloaded() {
    let (mut core, stats, dev) = lost_core();
    let n = crate::plugins_user::QUARANTINE_AFTER;
    // Crash 1: reloaded by itself. Crash 2 (inside the window): stays crashed; RELOAD.
    // Crash 3: the one that quarantines it, so nothing reloads it.
    stats.set_lost(true);
    crash_at(&mut core, &stats, &dev, 20.0, "exited with code 3");
    assert_eq!(core.plugins_user().failures(GAIN), 1);
    stats.set_lost(true);
    core.plugin_tick(50.0);
    lose(&mut core, &stats, &dev, "exited with code 3");
    assert_eq!(core.plugins_user().failures(GAIN), 2);
    core.handle_action(UiAction::ReloadPlugin {
        device: dev.clone(),
    });
    stats.set_lost(false);
    core.wait_plugins();
    settle(&mut core);
    assert_eq!(
        stats.created(),
        3,
        "the original, one by itself, one by RELOAD"
    );
    stats.set_lost(true);
    core.plugin_tick(80.0);
    lose(&mut core, &stats, &dev, "exited with code 3");
    core.wait_plugins();
    assert!(
        core.plugins_user().is_quarantined(GAIN),
        "after {n} crashes"
    );
    assert_eq!(
        stats.created(),
        3,
        "the last crash quarantined it: no reload"
    );
    assert_eq!(log_has(&core, "the plugin is quarantined"), 1);
}

#[test]
fn no_reload_by_itself_while_an_export_renders() {
    use atomik_model::{Clip, ClipContent, ClipId, CreateOptions, CreateSource, Note};
    use atomik_model::{SongCommand, WorkflowCommand};
    let (mut core, stats, dev) = lost_core();
    core.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(key("lead")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    core.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length: 4 * atomik_core::PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            follow_song_key: true,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(vec![Note::new(0, atomik_core::PPQ, 64, 0.8)].into()),
        },
    }))
    .unwrap();
    let dir = temp("crash-reload-export");
    core.start_audio_export(
        atomik_ui::ExportScope::Song,
        dir.join("song.wav"),
        atomik_ui::ExportValues::default(),
    )
    .unwrap();
    assert!(core.audio_export_busy());
    let id = core.plugin_instance(&dev).unwrap();
    core.plugin_tick(20.0);
    stats.set_lost(true);
    core.plugin_event(atomik_plugin_host::ManagerEvent::Crashed {
        instance: id,
        reason: "exited with code 3".to_string(),
    });
    assert!(core.audio_export_busy(), "the export was still running");
    assert_eq!(stats.created(), 1, "no reload while it renders");
    assert_eq!(log_has(&core, "an export or a bounce is rendering"), 1);
    assert!(core.plugin_crash(&dev).is_some());
    core.wait_background();
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ an engine fault

#[test]
fn a_crashed_plugin_is_left_to_the_crash_path_when_the_engine_reports_a_fault() {
    let (mut core, stats, dev) = lost_core();
    lose(&mut core, &stats, &dev, "exited with code 3");
    let id = core.instance_id_of(&dev).unwrap();
    assert!(core.fault_belongs_to_plugin_path(id));
    assert_eq!(
        core.bypass_faulted_device(Some(id)),
        crate::app_core::FaultBypass::PluginPath("Atomik Test Gain".to_string())
    );
    let value = core.doc().device(&dev).and_then(|d| d.params.get("bypass"));
    assert_eq!(value, None, "the document was not touched");
}
