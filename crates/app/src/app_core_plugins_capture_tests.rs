//! Wiring, state capture half: the live app asks a plugin for its state in the background and
//! stores it when it arrives, so a plugin whose main thread is busy holds nothing up. The
//! instances come from the `NullBackend` in its background mode (see
//! `NullStats::set_background_capture`), where a held capture is not answered and every
//! synchronous capture is counted: a path that never waits leaves that count where it was.

use super::*;
use crate::app_core::plugins::SNAPSHOT_GRACE_SECS;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_model::{ConsoleCommand, SongCommand, WorkflowCommand, empty_doc};
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
    // The first tick of a loaded project snapshots: the plugin answered at once.
    assert_eq!(snapshots(&dir), 1);
    // The next one: the plugin is busy, the snapshot waits for the grace, then goes without it.
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
