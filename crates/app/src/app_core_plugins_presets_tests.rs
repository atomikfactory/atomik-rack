//! Wiring, presets half: the list of presets is asked for once and shown when it lands, a list
//! that failed is asked for again at the next opening (not sooner than a pause), a preset load
//! is a polled step whose undo step keeps its meaning, and the knobs follow the plugin when its
//! values arrive. The instances come from the `NullBackend`, whose "busy" mode answers cosmetic
//! requests only when the test lets it (see `NullStats::set_soft_busy`) and whose preset delay
//! counts polls instead of time, so none of this depends on how fast a machine is.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_model::empty_doc;
use atomik_plugin_host::{NullBackend, NullBehavior, NullPlugin, NullStats};
use atomik_render::plugins::{PluginHost, PluginIsolation};

fn preset_core() -> (AppCore<OfflineSink>, Arc<NullStats>, DeviceKey) {
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
    stats.set_presets(true);
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.plugins.host =
        PluginHost::with_backends(PluginIsolation::InProcess, vec![Box::new(backend)]);
    core.configure_plugins(config);
    add_plugin(&mut core, GAIN, "echo");
    core.frame();
    core.plugins_capture_all();
    (core, stats, key("echo"))
}

fn stored(core: &AppCore<OfflineSink>, device: &DeviceKey) -> Option<Vec<u8>> {
    core.doc()
        .resolve(device)
        .and_then(|d| d.plugin.as_ref())
        .and_then(|p| p.state.as_deref().map(|b| b.0.clone()))
}

fn knob(core: &AppCore<OfflineSink>, device: &DeviceKey, param: &str) -> f32 {
    core.doc()
        .param_value(device, param, core.registry())
        .expect("a plugin param")
}

fn pane_presets(core: &AppCore<OfflineSink>) -> usize {
    core.plugin_params().presets().len()
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

// ------------------------------------------------------------------ the list

#[test]
fn the_pane_opens_at_once_with_a_busy_plugin_and_fills_when_the_list_lands() {
    let (mut core, stats, dev) = preset_core();
    stats.set_soft_busy(true);
    core.open_plugin_params(&dev);
    assert_eq!(pane_presets(&core), 0, "nothing to show yet");
    assert!(core.plugin_presets_loading(&dev));
    assert_eq!(stats.preset_lists(), 1, "asked");
    for _ in 0..10 {
        core.frame();
    }
    assert_eq!(stats.preset_lists(), 1, "asked once, not every frame");
    assert_eq!(pane_presets(&core), 0);
    stats.set_soft_busy(false);
    core.frame();
    assert_eq!(pane_presets(&core), 3, "the list came");
    assert!(!core.plugin_presets_loading(&dev));
    assert_eq!(stats.preset_lists(), 1);
}

#[test]
fn a_list_that_failed_is_not_taken_for_no_presets_and_is_asked_again_at_the_next_opening() {
    let (mut core, stats, dev) = preset_core();
    core.plugin_tick(100.0);
    stats.set_soft_busy(true);
    stats.fail_next_preset_list();
    core.open_plugin_params(&dev);
    assert_eq!(stats.preset_lists(), 1);
    stats.set_soft_busy(false);
    core.frame();
    assert_eq!(pane_presets(&core), 0, "the failure shows nothing");
    assert!(!core.plugin_presets_loading(&dev), "and nothing is awaited");
    for _ in 0..5 {
        core.frame();
    }
    assert_eq!(stats.preset_lists(), 1, "the frames do not ask again");
    // An opening soon after does not ask again (the pause), a later one does.
    core.close_plugin_params();
    core.plugin_tick(101.0);
    core.open_plugin_params(&dev);
    assert_eq!(stats.preset_lists(), 1, "too soon");
    core.close_plugin_params();
    core.plugin_tick(110.0);
    core.open_plugin_params(&dev);
    assert_eq!(stats.preset_lists(), 2, "asked again");
    assert_eq!(pane_presets(&core), 3, "and this time it came");
}

#[test]
fn stepping_before_the_list_has_landed_says_so_and_changes_nothing() {
    let (mut core, stats, dev) = preset_core();
    stats.set_soft_busy(true);
    let before = stored(&core, &dev);
    assert!(!core.step_plugin_preset(&dev, 1));
    assert!(
        core.last_rejection()
            .is_some_and(|r| r.contains("still loading")),
        "{:?}",
        core.last_rejection()
    );
    assert_eq!(stored(&core, &dev), before);
    stats.set_soft_busy(false);
    core.frame();
    assert!(core.step_plugin_preset(&dev, 1), "the list is there now");
}

// ------------------------------------------------------------------ the load

#[test]
fn a_preset_load_that_takes_time_is_a_polled_step_and_one_undo_step() {
    let (mut core, stats, dev) = preset_core();
    assert_eq!(core.plugin_presets(&dev).len(), 3);
    let before = knob(&core, &dev, "p1");
    stats.set_preset_delay(4);
    assert!(core.load_plugin_preset(&dev, "warm"), "started");
    assert_eq!(
        stored(&core, &dev).as_deref(),
        Some(&b""[..]),
        "nothing is stored while the plugin loads"
    );
    assert_eq!(core.plugin_preset_current(&dev), None);
    assert_eq!(knob(&core, &dev, "p1"), before);
    // A second load into the same plugin is refused while one is under way.
    assert!(!core.load_plugin_preset(&dev, "bright"));
    frames_until(&mut core, 50, |c| c.plugin_preset_current(&dev).is_some());
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"preset:warm"[..]));
    assert_eq!(core.plugin_preset_current(&dev), Some("warm"));
    assert!((knob(&core, &dev, "p1") - (-45.6)).abs() < 1e-3);
    assert_eq!(core.document().undo_label(), Some("Change Plugin Patch"));
    // One undo puts the previous state, and the knobs with it, back into the live plugin.
    assert!(core.undo());
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b""[..]));
    assert_eq!(knob(&core, &dev, "p1"), before);
    assert_eq!(core.plugin_preset_current(&dev), None);
    let id = core.plugin_instance(&dev).unwrap();
    assert_eq!(
        core.plugin_host().manager().capture_state(id).unwrap().0,
        b"",
        "undo restored the plugin itself"
    );
}

#[test]
fn a_preset_load_the_plugin_refuses_when_it_answers_changes_nothing_and_says_so() {
    let (mut core, stats, dev) = preset_core();
    stats.set_preset_delay(2);
    assert!(core.load_plugin_preset(&dev, "nope"), "started");
    let before = stored(&core, &dev);
    frames_until(&mut core, 50, |c| c.last_rejection().is_some());
    assert_eq!(stored(&core, &dev), before);
    assert_eq!(core.plugin_preset_current(&dev), None);
    // The next load is not blocked by the refusal.
    stats.set_preset_delay(0);
    assert!(core.load_plugin_preset(&dev, "warm"));
    assert_eq!(core.plugin_preset_current(&dev), Some("warm"));
}

#[test]
fn the_knobs_follow_when_a_busy_plugin_gives_its_values_and_still_join_the_undo_step() {
    let (mut core, stats, dev) = preset_core();
    let before = knob(&core, &dev, "p1");
    stats.set_soft_busy(true);
    assert!(core.load_plugin_preset(&dev, "warm"));
    assert_eq!(stats.param_reads(), 1, "the values were asked for");
    assert_eq!(knob(&core, &dev, "p1"), before, "they have not come");
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b"preset:warm"[..]));
    stats.set_soft_busy(false);
    core.frame();
    assert!((knob(&core, &dev, "p1") - (-45.6)).abs() < 1e-3);
    assert_eq!(
        core.document().undo_label(),
        Some("Change Plugin Patch"),
        "the knob changes are part of the patch step"
    );
    assert!(core.undo());
    assert_eq!(knob(&core, &dev, "p1"), before, "one undo puts them back");
    assert_eq!(stored(&core, &dev).as_deref(), Some(&b""[..]));
}

#[test]
fn values_that_come_after_the_document_changed_do_not_join_an_unrelated_step() {
    let (mut core, stats, dev) = preset_core();
    let before = knob(&core, &dev, "p1");
    stats.set_soft_busy(true);
    assert!(core.load_plugin_preset(&dev, "warm"));
    // The user turns another param while the plugin has not answered.
    apply(
        &mut core,
        Command::SetParam {
            key: dev.clone(),
            param: "p4".to_string(),
            value: 0.5,
            merge: None,
        },
    );
    stats.set_soft_busy(false);
    core.frame();
    assert!((knob(&core, &dev, "p1") - (-45.6)).abs() < 1e-3);
    // The user's own change is still its own step: undo takes the knobs' step, not it.
    assert!(core.undo());
    assert_eq!(knob(&core, &dev, "p1"), before);
    assert!((knob(&core, &dev, "p4") - 0.5).abs() < 1e-6);
}

#[test]
fn a_preset_loaded_into_a_plugin_that_is_removed_meanwhile_is_dropped_quietly() {
    let (mut core, stats, dev) = preset_core();
    stats.set_preset_delay(1000);
    assert!(core.load_plugin_preset(&dev, "warm"));
    apply(&mut core, Command::RemoveDevice { key: dev.clone() });
    for _ in 0..5 {
        core.frame();
    }
    assert_eq!(core.plugin_preset_current(&dev), None);
    assert!(!core.plugin_preset_loading(&dev));
}
