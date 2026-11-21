//! Wiring, loading half: opening a project with heavy plugins, restarting the engine and
//! removing a plugin never make the interface wait for a plugin. The instances come from the
//! `NullBackend` in its background mode, which counts polls instead of time (see
//! `NullStats::set_background_create`, `set_restore_delay`, `set_activate_delay`,
//! `set_deactivate_delay` and `set_shutdown_delay`), so none of this depends on how fast a
//! machine is.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_model::empty_doc;
use atomik_plugin_host::{NullBackend, NullBehavior, NullPlugin, NullStats};
use atomik_render::plugins::{PluginHost, PluginIsolation};

/// A core with `n` null effects "p0", "p1", ..., each with a stored state.
fn core_with(n: usize) -> (AppCore<OfflineSink>, Arc<NullStats>) {
    let config = config(true, &[PKG]);
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
    for i in 0..n {
        add_plugin(&mut core, GAIN, &format!("p{i}"));
    }
    core.frame();
    // The state a saved project holds.
    core.plugins_capture_all();
    (core, stats)
}

/// Frames until `done`, at most `limit`; returns how many it took.
fn frames_until(
    core: &mut AppCore<OfflineSink>,
    limit: usize,
    mut done: impl FnMut(&AppCore<OfflineSink>) -> bool,
) -> usize {
    for frames in 1..=limit {
        core.frame();
        if done(core) {
            return frames;
        }
    }
    panic!("not done in {limit} frames");
}

fn log_count(core: &AppCore<OfflineSink>, needle: &str) -> usize {
    core.plugin_log()
        .iter()
        .filter(|l| l.contains(needle))
        .count()
}

fn leaving(core: &AppCore<OfflineSink>) -> usize {
    core.plugins.host.manager_ref().leaving_count()
}

/// Makes the plugins slow, as a sampler restoring a big library is, and starts the next
/// creation in the background.
fn slow(stats: &NullStats, polls: usize) {
    stats.set_background_create(Some(1));
    stats.set_restore_delay(polls);
    stats.set_activate_delay(polls);
}

#[test]
fn a_project_with_saved_state_opens_with_the_device_loading_and_then_live() {
    let (mut core, stats) = core_with(1);
    let dev = key("p0");
    slow(&stats, 6);
    let loaded_before = log_count(&core, "created in");
    // What opening another project does: its instances are created again, from the saved state.
    core.plugins_discard_on_next_reset();
    core.rebuild_engine();
    assert!(core.plugin_instance(&dev).is_none());
    assert!(core.plugins.pending.contains(&dev), "LOADING");
    assert!(core.plugin_loading().is_some());
    let frames = frames_until(&mut core, 200, |c| c.plugin_instance(&dev).is_some());
    assert!(
        frames > 6,
        "restoring and activating took many frames, none of them blocked ({frames})"
    );
    assert!(core.plugins_ready());
    assert_eq!(stats.created(), 2);
    assert_eq!(
        log_count(&core, "created in"),
        loaded_before + 1,
        "one more load line"
    );
    let line = core
        .plugin_log()
        .into_iter()
        .rfind(|l| l.contains("created in"))
        .expect("the load line");
    for stage in ["spawn", "instantiate", "restore", "activate", "attach"] {
        assert!(line.contains(stage), "{line}");
    }
    assert_eq!(
        log_count(&core, "ui stall"),
        0,
        "no call held the interface up"
    );
    assert!(core.plugin_failure(&dev).is_none());
}

#[test]
fn a_saved_state_that_cannot_be_restored_shows_the_same_failure_as_before() {
    let (mut core, stats) = core_with(1);
    let dev = key("p0");
    slow(&stats, 3);
    stats.fail_next_restore();
    core.plugins_discard_on_next_reset();
    core.rebuild_engine();
    frames_until(&mut core, 200, |c| c.plugin_instance(&dev).is_some());
    let why = core.plugin_failure(&dev).expect("FAILED").to_string();
    assert!(
        why.starts_with("its saved state could not be restored:"),
        "{why}"
    );
}

#[test]
fn a_plugin_that_cannot_activate_while_loading_fails_the_device_with_its_error() {
    let (mut core, stats) = core_with(1);
    let dev = key("p0");
    slow(&stats, 3);
    stats.fail_next_activation();
    core.plugins_discard_on_next_reset();
    core.rebuild_engine();
    frames_until(&mut core, 200, |c| !c.plugins.pending.contains(&dev));
    assert!(core.plugin_instance(&dev).is_none());
    assert_eq!(core.plugin_failure(&dev), Some("the null plugin refused"));
}

#[test]
fn removing_a_device_that_is_still_loading_works_and_undo_creates_it_afresh() {
    let (mut core, stats) = core_with(1);
    let dev = key("p0");
    slow(&stats, 1_000);
    core.plugins_discard_on_next_reset();
    core.rebuild_engine();
    for _ in 0..4 {
        core.frame();
    }
    assert!(core.plugins.pending.contains(&dev), "still restoring");
    apply(&mut core, Command::RemoveDevice { key: dev.clone() });
    assert!(core.plugins.pending.is_empty());
    assert_eq!(core.plugin_instance_count(), 0);
    // The plugin that was loading is taken down by the frames, not waited for.
    let before = stats.created();
    stats.set_restore_delay(0);
    stats.set_activate_delay(0);
    assert!(core.undo());
    core.wait_plugins();
    assert!(core.plugin_instance(&dev).is_some(), "a fresh instance");
    assert_eq!(stats.created(), before + 1);
    assert_eq!(leaving(&core), 0);
    assert_eq!(stats.destroyed(), 2, "the old instances are gone");
}

#[test]
fn removing_a_busy_plugin_returns_at_once_and_undo_creates_a_fresh_instance() {
    let (mut core, stats) = core_with(1);
    let dev = key("p0");
    stats.set_deactivate_delay(5);
    stats.set_shutdown_delay(5);
    // The engine retires the device on a render; the manager reaps it on the next frame.
    apply(&mut core, Command::RemoveDevice { key: dev.clone() });
    settle(&mut core);
    assert_eq!(core.plugin_instance_count(), 0, "destroyed for the app");
    assert_eq!(leaving(&core), 1, "the old plugin is still ending");
    assert_eq!(stats.destroyed(), 0);
    // Undo while the old instance is still leaving: a new one is created, not the old one.
    stats.set_deactivate_delay(0);
    stats.set_shutdown_delay(0);
    assert!(core.undo());
    core.wait_plugins();
    assert!(core.plugin_instance(&dev).is_some());
    assert_eq!(stats.created(), 2, "a fresh instance");
    for _ in 0..30 {
        core.frame();
        if leaving(&core) == 0 {
            break;
        }
    }
    assert_eq!(leaving(&core), 0);
    assert_eq!(stats.destroyed(), 1, "the old one is gone in the end");
}

/// An engine restart over `n` slow plugins: the frames it takes until all are live again.
fn rebuild_frames(n: usize) -> usize {
    let (mut core, stats) = core_with(n);
    stats.set_deactivate_delay(6);
    stats.set_activate_delay(6);
    core.rebuild_engine();
    // Nothing was waited for: the devices are stand-ins until the plugins are active.
    assert!(!core.plugins_ready(), "the devices wait for their plugins");
    let dev = key("p0");
    assert!(
        core.plugin_instance(&dev).is_some(),
        "the instance lives on"
    );
    let frames = frames_until(&mut core, 200, AppCore::plugins_ready);
    assert_eq!(stats.created(), n, "nothing was created again");
    assert_eq!(log_count(&core, "ui stall"), 0);
    frames
}

#[test]
fn an_engine_restart_with_three_slow_plugins_costs_one_wait_not_three() {
    let one = rebuild_frames(1);
    let three = rebuild_frames(3);
    assert!(one >= 6, "the plugin was waited for ({one} frames)");
    assert_eq!(three, one, "pipelined: the slowest, not the sum");
}

#[test]
fn a_device_removed_while_its_plugin_is_still_reattaching_leaves_no_instance() {
    let (mut core, stats) = core_with(1);
    let dev = key("p0");
    stats.set_deactivate_delay(5);
    stats.set_activate_delay(5);
    core.rebuild_engine();
    assert!(!core.plugins_ready());
    apply(&mut core, Command::RemoveDevice { key: dev.clone() });
    for _ in 0..40 {
        core.frame();
    }
    assert_eq!(core.plugin_instance_count(), 0);
    assert_eq!(leaving(&core), 0);
    assert_eq!(stats.destroyed(), 1);
    assert!(core.plugins_ready());
}
