//! Wiring, state capture half: the live app asks a plugin for its state in the background and
//! stores it when it arrives, so a plugin whose main thread is busy holds nothing up. The
//! instances come from the `NullBackend` in its background mode (see
//! `NullStats::set_background_capture`), where a held capture is not answered and every
//! synchronous capture is counted: a path that never waits leaves that count where it was.

use super::*;
use crate::app_core::plugins::SNAPSHOT_GRACE_SECS;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_model::{ConsoleCommand, PluginCommand, SongCommand, WorkflowCommand, empty_doc};
use atomik_plugin_host::{NullBackend, NullBehavior, NullPlugin, NullStats, PluginMessage};
use atomik_render::plugins::{PluginHost, PluginIsolation};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-capture-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A core with one test effect "echo" whose plugin captures in the background, with its state
/// stored once, and (when given) a recovery folder.
fn bg_core(recovery: Option<&Path>) -> (AppCore<OfflineSink>, Arc<NullStats>, DeviceKey) {
    let mut config = config(true, &[PKG]);
    config.paths = PluginPaths {
        recovery: recovery.map(Path::to_path_buf),
        ..PluginPaths::default()
    };
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
    core.plugins.host =
        PluginHost::with_backends(PluginIsolation::InProcess, vec![Box::new(backend)]);
    core.configure_plugins(config);
    add_plugin(&mut core, GAIN, "echo");
    core.frame();
    stats.set_background_capture(true);
    // The state the document holds to begin with (what a saved project has).
    core.plugins_capture_all();
    (core, stats, key("echo"))
}

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

/// The plugin changes its own state (a patch picked in its window) and says so.
fn patch(core: &mut AppCore<OfflineSink>, stats: &NullStats, device: &DeviceKey, state: &[u8]) {
    set_live(stats, state);
    post(core, device, PluginMessage::StateDirty);
    core.frame();
}

fn set_live(stats: &NullStats, state: &[u8]) {
    stats.edit_state(state);
}

fn snapshots(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".recover.atomik"))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn a_plugin_too_busy_to_answer_holds_nothing_up_and_its_state_is_stored_when_it_does() {
    let (mut core, stats, dev) = bg_core(None);
    let syncs = stats.sync_captures();
    let depth = core.document().undo_depth();
    patch(&mut core, &stats, &dev, b"patched");
    stats.hold_captures(true);
    for t in 0..8 {
        core.plugin_tick(10.0 + f64::from(t) * 2.0);
        core.frame();
    }
    assert_eq!(
        stats.sync_captures(),
        syncs,
        "nothing waited for the plugin"
    );
    assert_eq!(stats.capture_requests(), 1, "one request, not one per tick");
    assert_eq!(
        stored(&core, &dev).as_deref(),
        Some(&b""[..]),
        "nothing to store yet"
    );
    let id = core.plugin_instance(&dev).unwrap();
    assert!(!core.plugin_host().manager_ref().is_lost(id));
    assert!(
        core.plugin_log()
            .iter()
            .all(|l| !l.contains("cannot capture"))
    );
    stats.hold_captures(false);
    core.frame();
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"patched"[..]));
    assert_eq!(core.document().undo_label(), Some("Change Plugin Patch"));
    assert_eq!(core.document().undo_depth(), depth + 1, "one step");
    assert!(core.undo());
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b""[..]));
    assert_eq!(live(&mut core, &dev), b"", "the plugin went back with it");
}

#[test]
fn a_change_while_a_capture_is_out_is_asked_for_again_and_folds_into_the_same_step() {
    let (mut core, stats, dev) = bg_core(None);
    let depth = core.document().undo_depth();
    patch(&mut core, &stats, &dev, b"first");
    stats.hold_captures(true);
    core.plugin_tick(10.0);
    // The plugin changes again while the first request is out.
    set_live(&stats, b"second");
    post(&mut core, &dev, PluginMessage::StateDirty);
    core.frame();
    core.plugin_tick(12.0);
    assert_eq!(
        stats.capture_requests(),
        1,
        "a second request only asks again"
    );
    stats.hold_captures(false);
    core.frame();
    // The answer (the plugin's state when it answered) is stored; the change that came in
    // meanwhile is asked for once its quiet period is over.
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"second"[..]));
    core.plugin_tick(14.0);
    core.plugin_tick(16.0);
    core.frame();
    assert_eq!(stats.capture_requests(), 2, "asked again once it landed");
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"second"[..]));
    assert_eq!(
        core.document().undo_depth(),
        depth + 1,
        "bursts within the step gap are one step"
    );
}

#[test]
fn a_capture_that_never_came_keeps_the_stored_state() {
    let (mut core, stats, dev) = bg_core(None);
    patch(&mut core, &stats, &dev, b"patched");
    stats.hold_captures(true);
    core.plugin_tick(10.0);
    // The device goes away before the answer: nothing is stored over anything.
    let before = stored(&core, &dev);
    core.plugins_device_removed(&dev);
    stats.hold_captures(false);
    core.frame();
    core.plugin_tick(12.0);
    assert_eq!(stored(&core, &dev), before);
    assert!(core.plugins.capturing.is_empty());
}

#[test]
fn a_plugin_that_flags_its_state_dirty_on_every_frame_has_one_capture_in_flight() {
    let dir = temp("loop");
    let (mut core, stats, dev) = bg_core(Some(&dir));
    let syncs = stats.sync_captures();
    stats.hold_captures(true);
    for t in 0..20 {
        post(&mut core, &dev, PluginMessage::StateDirty);
        core.frame();
        core.plugin_tick(10.0 + f64::from(t) * 2.0);
    }
    assert_eq!(stats.capture_requests(), 1, "at most one in flight");
    assert_eq!(
        stats.sync_captures(),
        syncs,
        "the snapshots capture nothing synchronously"
    );
    core.wait_recovery_writes();
    assert!(
        snapshots(&dir) >= 1,
        "the snapshot is written after the grace"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_snapshot_after_a_capture_changed_a_state_is_written_from_the_stored_states_alone() {
    let dir = temp("wanted");
    let (mut core, stats, dev) = bg_core(Some(&dir));
    let syncs = stats.sync_captures();
    patch(&mut core, &stats, &dev, b"patched");
    core.plugin_tick(10.0);
    core.wait_recovery_writes();
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"patched"[..]));
    assert_eq!(snapshots(&dir), 1);
    assert_eq!(stats.sync_captures(), syncs);
    assert_eq!(stats.capture_requests(), 1, "the dirty capture only");
    // The snapshot holds the state that arrived.
    let newest = atomik_plugin_host::RecoveryDir::new(&dir).newest().unwrap();
    let mut reopened = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    reopened.open_recovery(&newest).unwrap();
    assert_eq!(stored(&reopened, &dev).as_deref(), Some(&b"patched"[..]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_periodic_snapshot_asks_in_the_background_and_goes_without_a_plugin_that_is_busy() {
    let dir = temp("periodic");
    let (mut core, stats, dev) = bg_core(Some(&dir));
    let syncs = stats.sync_captures();
    core.plugin_tick(0.0);
    core.wait_recovery_writes();
    // The first tick of a loaded project with an edit nobody saved snapshots: the plugin
    // answered at once.
    assert_eq!(snapshots(&dir), 1);
    // The project changes and the plugin is busy: the snapshot waits for the grace, then goes
    // without it.
    apply(&mut core, Command::SetTempo(100.0));
    stats.hold_captures(true);
    core.plugin_tick(1_000.0);
    core.wait_recovery_writes();
    assert_eq!(snapshots(&dir), 1, "waiting for the plugin");
    core.plugin_tick(1_000.0 + SNAPSHOT_GRACE_SECS - 0.5);
    core.wait_recovery_writes();
    assert_eq!(snapshots(&dir), 1, "still inside the grace");
    core.plugin_tick(1_000.0 + SNAPSHOT_GRACE_SECS + 0.5);
    core.wait_recovery_writes();
    assert_eq!(snapshots(&dir), 2, "written with what is stored");
    assert_eq!(stats.sync_captures(), syncs);
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b""[..]));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The sequence number of the newest snapshot: it moves with every snapshot that is queued.
fn queued(core: &AppCore<OfflineSink>) -> u64 {
    core.recovery_sequence()
}

/// Ticks `periods` periods apart from `from` (each tick one frame): returns the time after.
fn periods(core: &mut AppCore<OfflineSink>, from: f64, n: u32) -> f64 {
    let mut t = from;
    for _ in 0..n {
        core.frame();
        core.plugin_tick(t);
        t += 1_000.0;
    }
    t
}

#[test]
fn a_loaded_plugin_that_nobody_touches_writes_no_snapshot_in_any_period() {
    let dir = temp("untouched");
    let (mut core, stats, dev) = bg_core(Some(&dir));
    // The first period writes the unsaved project once; after that nothing changes.
    let mut t = periods(&mut core, 0.0, 1);
    core.wait_recovery_writes();
    let written = queued(&core);
    assert_eq!(snapshots(&dir), 1);
    let asked = stats.capture_requests();
    t = periods(&mut core, t, 6);
    core.wait_recovery_writes();
    assert_eq!(queued(&core), written, "six quiet periods queued nothing");
    assert_eq!(snapshots(&dir), 1);
    assert!(
        stats.capture_requests() > asked,
        "the plugin is still asked in every period (it may have moved without saying so)"
    );
    // A command on the project: the next period writes one.
    apply(&mut core, Command::SetTempo(101.0));
    t = periods(&mut core, t, 1);
    core.wait_recovery_writes();
    assert!(queued(&core) > written, "a document edit is snapshotted");
    let after_edit = queued(&core);
    // Quiet again.
    t = periods(&mut core, t, 3);
    core.wait_recovery_writes();
    assert_eq!(queued(&core), after_edit);
    // A plugin that says its state changed: stored by the capture, written by the next tick.
    patch(&mut core, &stats, &dev, b"patched");
    t = periods(&mut core, t, 1);
    core.wait_recovery_writes();
    assert!(
        queued(&core) > after_edit,
        "a flagged state change is snapshotted"
    );
    let after_flag = queued(&core);
    // A plugin that changes its state without saying so: the period's own capture finds it.
    set_live(&stats, b"moved quietly");
    periods(&mut core, t, 1);
    core.wait_recovery_writes();
    assert!(
        queued(&core) > after_flag,
        "an unflagged state change is snapshotted"
    );
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"moved quietly"[..]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_parameter_command_with_a_dirty_plugin_captures_nothing_and_a_removal_does() {
    let (mut core, stats, dev) = bg_core(None);
    let syncs = stats.sync_captures();
    patch(&mut core, &stats, &dev, b"patched");
    // Dirty and not yet asked for, then asked for and out: neither makes a command wait.
    apply(&mut core, Command::SetTempo(100.0));
    assert_eq!(
        stats.sync_captures(),
        syncs,
        "dirty: left to the background"
    );
    stats.hold_captures(true);
    core.plugin_tick(10.0);
    assert_eq!(stats.capture_requests(), 1);
    apply(&mut core, Command::SetTempo(110.0));
    assert_eq!(stats.sync_captures(), syncs, "out: left to the background");
    // A removal takes the latest state into the undo history first.
    apply(&mut core, Command::RemoveDevice { key: dev.clone() });
    assert_eq!(stats.sync_captures(), syncs + 1);
    assert!(core.undo());
    core.wait_plugins();
    assert_eq!(
        live(&mut core, &dev),
        b"patched",
        "undo restored the latest state"
    );
}

#[test]
fn undo_with_a_dirty_plugin_captures_first_and_makes_the_patch_its_own_step() {
    let (mut core, stats, dev) = bg_core(None);
    apply(&mut core, Command::SetTempo(100.0));
    let syncs = stats.sync_captures();
    patch(&mut core, &stats, &dev, b"patched");
    assert!(core.undo(), "the patch");
    assert_eq!(
        stats.sync_captures(),
        syncs + 1,
        "undo waits for the state it must keep"
    );
    // The change made in the window became its own step first, and this undo took it back.
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b""[..]));
    assert_eq!(live(&mut core, &dev), b"");
    assert!(core.redo());
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"patched"[..]));
    assert_eq!(live(&mut core, &dev), b"patched");
}

#[test]
fn a_save_takes_the_capture_that_is_out_and_leaves_nothing_behind() {
    let (mut core, stats, dev) = bg_core(None);
    patch(&mut core, &stats, &dev, b"patched");
    stats.hold_captures(true);
    core.plugin_tick(10.0);
    assert_eq!(stats.capture_requests(), 1);
    let syncs = stats.sync_captures();
    core.plugins_capture_all();
    assert_eq!(stats.capture_requests(), 1, "no second request");
    assert_eq!(stats.sync_captures(), syncs + 1);
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"patched"[..]));
    assert!(core.plugins.capturing.is_empty());
    let id = core.plugin_instance(&dev).unwrap();
    assert!(!core.plugin_host().manager_ref().capture_out(id));
}

fn open_editor(core: &mut AppCore<OfflineSink>, device: &DeviceKey, stats: &NullStats) {
    stats.set_external_editor(true);
    core.frame();
    core.handle_action(UiAction::OpenPluginGui {
        device: device.clone(),
    });
    assert!(core.plugin_gui_open(device));
}

#[test]
fn closing_the_editor_stores_the_state_when_it_arrives_as_one_undo_step() {
    let (mut core, stats, dev) = bg_core(None);
    open_editor(&mut core, &dev, &stats);
    let syncs = stats.sync_captures();
    let depth = core.document().undo_depth();
    stats.hold_captures(true);
    set_live(&stats, b"edited");
    core.plugin_window_closed(&dev);
    assert!(!core.plugin_gui_open(&dev));
    assert_eq!(stats.sync_captures(), syncs, "closing waits for nothing");
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b""[..]));
    stats.hold_captures(false);
    core.frame();
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"edited"[..]));
    assert_eq!(core.document().undo_label(), Some("Change Plugin Patch"));
    assert_eq!(core.document().undo_depth(), depth + 1);
}

#[test]
fn a_song_edit_and_a_console_edit_with_a_dirty_plugin_capture_nothing() {
    let (mut core, stats, dev) = bg_core(None);
    let syncs = stats.sync_captures();
    patch(&mut core, &stats, &dev, b"patched");
    apply(
        &mut core,
        Command::Song(SongCommand::SetLoop {
            range: (0, 1920),
            enabled: true,
        }),
    );
    assert_eq!(stats.sync_captures(), syncs, "a song edit leaves it dirty");
    apply(
        &mut core,
        Command::Console(ConsoleCommand::SetCompensation(true)),
    );
    assert_eq!(
        stats.sync_captures(),
        syncs,
        "a console edit leaves it dirty"
    );
    core.plugin_tick(10.0);
    core.frame();
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"patched"[..]));
}

#[test]
fn a_workflow_removal_of_a_plugin_captures_once_and_undo_restores_the_latest_state() {
    let (mut core, stats, dev) = bg_core(None);
    let syncs = stats.sync_captures();
    patch(&mut core, &stats, &dev, b"patched");
    apply(
        &mut core,
        Command::Workflow(WorkflowCommand::RemoveDevices {
            keys: vec![dev.clone()],
        }),
    );
    assert_eq!(stats.sync_captures(), syncs + 1);
    assert!(core.undo());
    core.wait_plugins();
    assert_eq!(live(&mut core, &dev), b"patched");
}

#[test]
fn the_predicate_follows_what_a_command_can_remove() {
    type Core = AppCore<OfflineSink>;
    let remove = Command::RemoveDevice { key: key("echo") };
    assert!(!Core::command_keeps_plugins(&remove));
    assert!(Core::command_keeps_plugins(&Command::SetTempo(100.0)));
    assert!(Core::command_keeps_plugins(&Command::Batch(vec![
        Command::SetParam {
            key: key("echo"),
            param: "p1".into(),
            value: 0.5,
            merge: None,
        }
    ])));
    assert!(!Core::command_keeps_plugins(&Command::Batch(vec![
        Command::SetTempo(100.0),
        remove,
    ])));
    assert!(!Core::command_keeps_plugins(&Command::Song(
        SongCommand::UnfreezeTrack {
            device: key("echo"),
            keep_audio: false,
        }
    )));
}

#[test]
fn duplicating_a_dirty_plugin_captures_once_and_the_copy_carries_the_latest_state() {
    let (mut core, stats, dev) = bg_core(None);
    let syncs = stats.sync_captures();
    patch(&mut core, &stats, &dev, b"patched");
    assert_eq!(
        stored(&core, &dev).as_deref(),
        Some(&b""[..]),
        "not stored yet"
    );
    apply(
        &mut core,
        Command::Workflow(WorkflowCommand::DuplicateDevices {
            keys: vec![dev.clone()],
            route: false,
        }),
    );
    core.wait_plugins();
    assert_eq!(stats.sync_captures(), syncs + 1);
    let copy = core
        .doc()
        .plugin_devices()
        .into_iter()
        .map(|(k, _)| k)
        .find(|k| *k != dev)
        .expect("the copy");
    assert_eq!(stored(&core, &copy).as_deref(), Some(&b"patched"[..]));
}

#[test]
fn the_predicate_sends_commands_that_copy_a_device_to_the_synchronous_side() {
    type Core = AppCore<OfflineSink>;
    for w in [
        WorkflowCommand::DuplicateDevices {
            keys: vec![key("echo")],
            route: false,
        },
        WorkflowCommand::DuplicateTrack {
            device: key("echo"),
        },
        WorkflowCommand::DuplicateTracks {
            devices: vec![key("echo")],
            place: None,
        },
    ] {
        assert!(!Core::command_keeps_plugins(&Command::Workflow(w.clone())));
        assert!(!Core::command_keeps_plugins(&Command::Batch(vec![
            Command::Workflow(w)
        ])));
    }
}

// ----- a plugin that is alive but does not answer --------------------------------------------------

fn log_count(core: &AppCore<OfflineSink>, needle: &str) -> usize {
    core.plugin_log()
        .iter()
        .filter(|l| l.contains(needle))
        .count()
}

#[test]
fn a_plugin_that_does_not_answer_is_not_responding_and_it_clears_when_it_answers() {
    let (mut core, stats, dev) = bg_core(None);
    assert!(core.view().plugin_status(&dev).not_responding.is_none());
    stats.set_not_responding(true);
    core.frame();
    let status = core.view().plugin_status(&dev);
    assert_eq!(status.not_responding.as_deref(), Some("CaptureState"));
    // Alive: not crashed, not failed, not quarantined, not in the settings, and the log says so
    // once.
    assert!(status.crashed.is_none() && status.failed.is_none() && status.quarantined.is_none());
    assert!(core.plugin_crash(&dev).is_none());
    assert!(core.plugins.settings.crashed.is_empty());
    core.frame();
    assert_eq!(log_count(&core, "not responding"), 1);
    let id = core.plugin_instance(&dev).unwrap();
    assert!(!core.plugin_host().manager_ref().is_lost(id));
    stats.set_not_responding(false);
    core.frame();
    assert!(core.view().plugin_status(&dev).not_responding.is_none());
    assert_eq!(log_count(&core, "responding again"), 1);
    assert!(core.plugins.settings.crashed.is_empty());
}

#[test]
fn a_save_while_a_plugin_does_not_answer_keeps_its_stored_state_and_tells_the_user_once() {
    let (mut core, stats, dev) = bg_core(None);
    set_live(&stats, b"one");
    core.plugins_capture_all();
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"one"[..]));
    set_live(&stats, b"two");
    stats.set_not_responding(true);
    core.frame();
    let dir = temp("busy-save");
    let file = dir.join("song.atomik");
    core.save_as_interactive(&file)
        .expect("the save completes without the plugin");
    assert!(file.exists());
    assert_eq!(
        stored(&core, &dev).as_deref(),
        Some(&b"one"[..]),
        "the last captured state was kept and saved"
    );
    let notice = "Atomik Test Gain did not hand over its current settings; its last captured settings were saved.";
    assert_eq!(core.last_rejection(), Some(notice));
    assert_eq!(
        log_count(&core, "did not hand over its current settings"),
        1
    );
    assert!(core.plugins.settings.crashed.is_empty());
    // The plugin answers again: the next save asks it normally and says nothing more.
    stats.set_not_responding(false);
    core.frame();
    core.save_as_interactive(&file).expect("the second save");
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"two"[..]));
    assert_eq!(
        log_count(&core, "did not hand over its current settings"),
        1
    );
}

#[test]
fn removing_a_plugin_that_does_not_answer_goes_on_with_the_stored_state_and_logs_it() {
    let (mut core, stats, dev) = bg_core(None);
    patch(&mut core, &stats, &dev, b"patched");
    stats.set_not_responding(true);
    core.frame();
    apply(&mut core, Command::RemoveDevice { key: dev.clone() });
    assert!(core.doc().device(&dev).is_none(), "the device is gone");
    assert_eq!(log_count(&core, "did not hand over its state"), 1);
    assert!(core.plugins.settings.crashed.is_empty());
    assert!(core.undo());
    core.wait_plugins();
    assert_eq!(
        stored(&core, &dev).as_deref(),
        Some(&b""[..]),
        "undo brings back the state that was stored"
    );
}

#[test]
fn an_engine_restart_with_a_plugin_that_does_not_answer_goes_on_and_says_nothing_to_the_user() {
    let (mut core, stats, dev) = bg_core(None);
    stats.set_not_responding(true);
    core.frame();
    let before = core.last_rejection().map(str::to_owned);
    core.rebuild_engine();
    core.wait_plugins();
    assert_eq!(core.last_rejection().map(str::to_owned), before);
    assert!(log_count(&core, "did not hand over its state") >= 1);
    assert!(core.plugins.not_handed.is_empty());
    assert!(
        core.plugin_instance(&dev).is_some(),
        "the instance lives on"
    );
    assert!(core.plugins.settings.crashed.is_empty());
}

#[test]
fn a_background_capture_is_not_asked_of_a_plugin_that_does_not_answer() {
    let (mut core, stats, dev) = bg_core(None);
    patch(&mut core, &stats, &dev, b"patched");
    stats.set_not_responding(true);
    core.frame();
    let asked = stats.capture_requests();
    for t in 0..4 {
        core.plugin_tick(10.0 + f64::from(t) * 2.0);
        core.frame();
    }
    assert_eq!(stats.capture_requests(), asked);
    assert_eq!(log_count(&core, "cannot capture"), 0, "no line per tick");
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b""[..]));
}

#[test]
fn the_notice_names_the_devices_and_whether_a_state_was_stored() {
    use crate::app_core::plugins::not_handed_notice;
    let one = vec!["Lead".to_string()];
    assert_eq!(
        not_handed_notice(&one, true),
        "Lead did not hand over its current settings; its last captured settings were saved."
    );
    assert_eq!(
        not_handed_notice(&one, false),
        "Lead did not hand over its current settings and none were captured before; it was saved without them."
    );
    let two = vec!["Lead".to_string(), "Pad".to_string()];
    assert_eq!(
        not_handed_notice(&two, true),
        "Lead, Pad did not hand over their current settings; their last captured settings were saved."
    );
}

/// A state of `n` MiB.
fn mib(n: usize) -> Vec<u8> {
    vec![7u8; n << 20]
}

#[test]
fn a_state_over_the_cap_keeps_the_stored_one_marks_the_device_and_clears_when_one_fits() {
    let (mut core, stats, dev) = bg_core(None);
    set_live(&stats, b"fits");
    core.plugins_capture_all();
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"fits"[..]));
    assert_eq!(core.view().plugin_status(&dev).state_too_large, None);

    // 17 MiB: over the 16 MiB a device may store.
    set_live(&stats, &mib(17));
    core.plugins_capture_all();
    core.plugins_capture_all();
    assert_eq!(
        stored(&core, &dev).as_deref(),
        Some(&b"fits"[..]),
        "the last state that fitted stays"
    );
    core.frame();
    let status = core.view().plugin_status(&dev);
    assert_eq!(status.state_too_large, Some((17 << 20, 16 << 20)));
    assert!(
        status.failed.is_none() && status.crashed.is_none(),
        "the plugin itself is fine"
    );
    assert_eq!(
        log_count(&core, "state not stored"),
        1,
        "one line for the situation, not one per capture"
    );
    assert!(
        core.last_rejection().is_none(),
        "no notice per capture, only at save"
    );

    // A state that fits again clears it.
    set_live(&stats, b"small again");
    core.plugins_capture_all();
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"small again"[..]));
    core.frame();
    assert_eq!(core.view().plugin_status(&dev).state_too_large, None);
    assert_eq!(log_count(&core, "state is stored again"), 1);
}

#[test]
fn a_save_with_a_state_over_the_cap_says_so_once_and_writes_the_last_state_that_fitted() {
    let (mut core, stats, dev) = bg_core(None);
    set_live(&stats, b"one");
    core.plugins_capture_all();
    set_live(&stats, &mib(17));
    let dir = temp("too-large-save");
    let file = dir.join("song.atomik");
    core.save_as_interactive(&file).expect("the save completes");
    assert!(file.exists());
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"one"[..]));
    assert_eq!(
        core.last_rejection(),
        Some(
            "Atomik Test Gain is too large to save (17 MB, the limit is 16 MB); its last state that fitted was saved."
        )
    );
    // The next save, still too large, says it again (it is still true); a save after the state
    // fits says nothing.
    set_live(&stats, b"two");
    core.save_as_interactive(&file).expect("the second save");
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"two"[..]));
    assert_eq!(
        core.last_rejection(),
        Some(
            "Atomik Test Gain is too large to save (17 MB, the limit is 16 MB); its last state that fitted was saved."
        ),
        "no new notice for a state that fitted"
    );
    assert_eq!(core.view().plugin_status(&dev).state_too_large, None);
}

#[test]
fn a_state_the_plugin_cannot_hand_over_is_marked_and_the_plugin_is_left_running() {
    let (mut core, stats, dev) = bg_core(None);
    set_live(&stats, b"one");
    core.plugins_capture_all();
    // The channel to a plugin in its own process carries 64 MiB; a smaller limit stands in.
    stats.set_state_limit(1000);
    set_live(&stats, &[1u8; 2000]);
    core.plugins_capture_all();
    core.plugins_capture_all();
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"one"[..]));
    core.frame();
    let status = core.view().plugin_status(&dev);
    assert_eq!(status.state_too_large, Some((2000, 1000)));
    let id = core.plugin_instance(&dev).unwrap();
    assert!(!core.plugin_host().manager_ref().is_lost(id));
    assert!(status.failed.is_none() && status.crashed.is_none());
    assert_eq!(log_count(&core, "cannot hand over its state"), 1);
    assert_eq!(log_count(&core, "cannot capture state"), 0);
    stats.set_state_limit(0);
    core.plugins_capture_all();
    assert_eq!(stored(&core, &dev).as_deref(), Some(&[1u8; 2000][..]));
    core.frame();
    assert_eq!(core.view().plugin_status(&dev).state_too_large, None);
}

#[test]
fn a_background_capture_over_the_cap_is_marked_like_a_synchronous_one() {
    let (mut core, stats, dev) = bg_core(None);
    patch(&mut core, &stats, &dev, &mib(17));
    for t in 0..8 {
        core.plugin_tick(10.0 + f64::from(t) * 2.0);
        core.frame();
    }
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b""[..]));
    assert_eq!(
        core.view().plugin_status(&dev).state_too_large,
        Some((17 << 20, 16 << 20))
    );
    assert!(core.last_rejection().is_none());
}

#[test]
fn the_too_large_notices_name_the_devices_the_sizes_and_whether_a_state_was_stored() {
    use crate::app_core::plugins::{TooLarge, too_large_notice, unsaved_settings_notice};
    let device = TooLarge {
        bytes: 23 << 20,
        limit: 16 << 20,
        project: false,
        frame: false,
    };
    let project = TooLarge {
        bytes: 140 << 20,
        limit: 128 << 20,
        project: true,
        frame: false,
    };
    let one = vec!["Lead".to_string()];
    assert_eq!(
        too_large_notice(&one, &[device], false),
        "Lead is too large to save (23 MB, the limit is 16 MB); it was saved without a state."
    );
    assert_eq!(
        too_large_notice(&one, &[project], true),
        "Lead is too large to save (the plugin states of the project would total 140 MB, the limit is 128 MB); its last state that fitted was saved."
    );
    let two = vec!["Lead".to_string(), "Pad".to_string()];
    assert_eq!(
        too_large_notice(&two, &[device, device], true),
        "Lead, Pad are too large to save; their last states that fitted were saved."
    );
    assert_eq!(
        unsaved_settings_notice(&two),
        "The current settings of Lead, Pad could not be saved; their last saved settings were kept."
    ); // Every one of them has its French text, with the names and the sizes kept.
    use atomik_ui::i18n::{Lang, tr, with_language};
    let french = |text: String| with_language(Lang::French, || tr(&text).into_owned());
    assert_eq!(
        french(too_large_notice(&one, &[device], false)),
        "Lead est trop volumineux pour être enregistré (23 Mo, la limite est de 16 Mo) ; il a été enregistré sans état."
    );
    assert_eq!(
        french(too_large_notice(&one, &[device], true)),
        "Lead est trop volumineux pour être enregistré (23 Mo, la limite est de 16 Mo) ; son dernier état qui tenait a été enregistré."
    );
    assert_eq!(
        french(too_large_notice(&one, &[project], true)),
        "Lead est trop volumineux pour être enregistré (les états des plugins du projet totaliseraient 140 Mo, la limite est de 128 Mo) ; son dernier état qui tenait a été enregistré."
    );
    assert_eq!(
        french(too_large_notice(&one, &[project], false)),
        "Lead est trop volumineux pour être enregistré (les états des plugins du projet totaliseraient 140 Mo, la limite est de 128 Mo) ; il a été enregistré sans état."
    );
    assert_eq!(
        french(too_large_notice(&two, &[device, device], true)),
        "Lead, Pad sont trop volumineux pour être enregistrés ; leurs derniers états qui tenaient ont été enregistrés."
    );
    assert_eq!(
        french(unsaved_settings_notice(&two)),
        "Les réglages actuels de Lead, Pad n'ont pas pu être enregistrés ; leurs derniers réglages enregistrés ont été conservés."
    );
}

/// Pins the first param of the test plugin's layout on `device`.
fn pin_first_param(core: &mut AppCore<OfflineSink>, device: &DeviceKey) {
    let id = core
        .doc()
        .resolve(device)
        .unwrap()
        .plugin
        .as_ref()
        .unwrap()
        .layout
        .params[0]
        .id;
    apply(
        core,
        Command::Plugin(PluginCommand::SetPinned {
            device: device.clone(),
            pinned: vec![id],
        }),
    );
}

fn pinned_rows(core: &AppCore<OfflineSink>) -> usize {
    core.plugins
        .host
        .pinned_params()
        .values()
        .map(Vec::len)
        .sum()
}

#[test]
fn a_second_project_is_pinned_and_fed_whatever_revision_the_first_had() {
    let (mut a, _stats, _dev) = bg_core(None);
    add_plugin(&mut a, GAIN, "second");
    pin_first_param(&mut a, &key("second"));
    a.frame();
    assert_eq!(pinned_rows(&a), 1);
    let text = atomik_model::save_project(a.doc());
    let (mut b, _stats_b, _dev_b) = bg_core(None);
    b.frame();
    assert_eq!(pinned_rows(&b), 0, "the second core starts without pins");
    let before = b.document().revision();
    b.open_str(&text, None).unwrap();
    assert_ne!(
        b.document().revision(),
        before,
        "a replaced document has a new revision"
    );
    b.frame();
    assert_eq!(
        pinned_rows(&b),
        1,
        "the pins of the opened project are registered"
    );
    let (revision, devices) = b.plugins.view_devices.clone().expect("the views were fed");
    assert_eq!(revision, b.document().revision());
    let keys: Vec<&str> = devices.iter().map(|(k, _)| k.0.as_str()).collect();
    assert_eq!(keys, ["echo", "second"], "the list is the opened project's");
}

#[test]
fn a_host_that_is_replaced_by_a_configuration_has_the_pins_at_once_and_after_the_frame() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(true, &[PKG]));
    add_plugin(&mut core, GAIN, "echo");
    pin_first_param(&mut core, &key("echo"));
    core.plugins_pin_document_params();
    assert_eq!(pinned_rows(&core), 1);
    // No instance exists yet, so a configuration that names an isolation builds a new host.
    let mut again = config(true, &[PKG]);
    again.isolation = Some(PluginIsolation::InProcess);
    core.configure_plugins(again);
    // The configuration registers the catalog from the document again, which writes the pins to
    // the new host; the frame's gate must not undo that or hold on to the old host's state.
    assert_eq!(pinned_rows(&core), 1, "the new host has the pins at once");
    assert!(
        core.plugins.view_devices.is_none(),
        "the views are fed again"
    );
    core.frame();
    assert_eq!(pinned_rows(&core), 1);
    assert!(core.plugins.view_devices.is_some());
}
