//! Additions: the `LayoutChanged` rebuild path (a plugin whose param table grows at run time), a live relink to
//! another plugin, and the state capture one quiet second after `state.mark_dirty`.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_device_sdk::PluginRef;
use atomik_model::{Command, PluginCommand, empty_doc};
use atomik_plugin_host::PluginMessage;

const MIDI_SYNTH: &str = "clap.atomik.test.midisynth";
const VARIANTS: &str = "atomik-test-clap-variants";

/// The name the live plugin behind `device` reports (from its instance, not from the document).
fn live_name(core: &mut AppCore<OfflineSink>, device: &str) -> String {
    let id = core
        .plugin_instance(&key(device))
        .unwrap_or_else(|| panic!("{device} has a live instance"));
    core.plugin_host()
        .manager_ref()
        .layout(id)
        .expect("a live layout")
        .name
        .clone()
}

/// The test plugin whose table grows: setting its `Grow` param (id 1999) to 0.5 or more makes it
/// ask for a restart, add an `Extra` param (id 2000) while deactivated and rescan its params.
const PARAMS: &str = "clap.atomik.test.params";
const EXTRA: u32 = 2_000;

/// Adds the bank, sets `p5` and then the `Grow` param, and runs frames until the plugin's own
/// restart and rescan have come back through the manager as a layout change.
fn grown_bank() -> AppCore<OfflineSink> {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, PARAMS, "bank");
    set_param(&mut core, "bank", "p5", 0.25);
    set_param(&mut core, "bank", "p1999", 1.0);
    for _ in 0..40 {
        settle(&mut core);
        let has_extra = core
            .doc()
            .resolve(&key("bank"))
            .unwrap()
            .plugin
            .as_ref()
            .unwrap()
            .layout
            .params
            .iter()
            .any(|p| p.id == EXTRA);
        if has_extra {
            break;
        }
    }
    core.wait_plugins();
    settle(&mut core);
    core
}

#[test]
fn a_layout_change_swaps_the_descriptor_and_keeps_values_and_the_instance() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, PARAMS, "bank");
    let before = core.registry().descriptor(PARAMS).unwrap();
    assert!(before.param_by_key(&format!("p{EXTRA}")).is_none());
    drop(core);

    let mut core = grown_bank();
    // The document's snapshot has the new row, the registry the new descriptor.
    let layout = &core
        .doc()
        .resolve(&key("bank"))
        .unwrap()
        .plugin
        .as_ref()
        .unwrap()
        .layout;
    assert!(
        layout.params.iter().any(|p| p.id == EXTRA),
        "the document learned the new row"
    );
    let descriptor = core.registry().descriptor(PARAMS).unwrap();
    assert!(
        descriptor.param_by_key(&format!("p{EXTRA}")).is_some(),
        "the registry has the new descriptor"
    );
    assert!(
        !std::ptr::eq(descriptor, before),
        "another descriptor than the scan's"
    );
    // One instance, still live, and the values the rack set are kept by key.
    assert_eq!(core.plugin_instance_count(), 1);
    assert!(core.plugin_instance(&key("bank")).is_some());
    let params = &core.doc().resolve(&key("bank")).unwrap().params;
    assert_eq!(params.get("p5"), Some(&0.25));
    assert_eq!(params.get("p1999"), Some(&1.0));
    // The new param can be set and reaches the (rebuilt) device without an error.
    set_param(&mut core, "bank", &format!("p{EXTRA}"), 0.5);
    settle(&mut core);
    assert_eq!(core.plugin_instance_count(), 1);
}

#[test]
fn a_layout_change_adds_no_undo_step_and_undo_keeps_the_layout() {
    let mut core = grown_bank();
    let value = |core: &AppCore<OfflineSink>, k: &str| {
        core.doc()
            .resolve(&key("bank"))
            .unwrap()
            .params
            .get(k)
            .copied()
    };
    // The undo steps are the two SetParams only: the layout sync and the rebuild added none.
    assert!(core.undo());
    assert_eq!(value(&core, "p1999"), None);
    assert_eq!(value(&core, "p5"), Some(0.25));
    assert!(core.undo());
    assert_eq!(value(&core, "p5"), None);
    let layout = &core
        .doc()
        .resolve(&key("bank"))
        .unwrap()
        .plugin
        .as_ref()
        .unwrap()
        .layout;
    assert!(
        layout.params.iter().any(|p| p.id == EXTRA),
        "undo leaves the synced layout"
    );
    assert_eq!(core.plugin_instance_count(), 1);
}

#[test]
fn a_relink_to_another_plugin_rebuilds_the_engine_and_the_instance_follows() {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(true, &[PKG, VARIANTS]));
    add_plugin(&mut core, SYNTH, "lead");
    let first = core.plugin_instance(&key("lead")).unwrap();
    let synth_name = live_name(&mut core, "lead");

    let plugin = PluginRef::from_type_id(MIDI_SYNTH).unwrap();
    let entry = core
        .plugin_catalog()
        .entry(&plugin)
        .expect("in the catalog");
    let relink = Command::Plugin(PluginCommand::Relink {
        device: key("lead"),
        plugin: plugin.clone(),
        layout: entry.layout.clone(),
        path_hint: None,
    });
    apply(&mut core, relink);
    core.wait_plugins();
    settle(&mut core);
    assert_eq!(
        core.doc().resolve(&key("lead")).unwrap().type_id,
        MIDI_SYNTH,
        "the document points at the other plugin"
    );
    let second = core
        .plugin_instance(&key("lead"))
        .expect("a live instance again");
    assert_ne!(first, second, "a new instance stands behind the device");
    assert_eq!(live_name(&mut core, "lead"), "atomik.test.midisynth");
    assert_eq!(core.plugin_instance_count(), 1, "the old instance is gone");

    // Undo points the device back at the first plugin, again through a rebuild.
    assert!(core.undo());
    core.wait_plugins();
    settle(&mut core);
    assert_eq!(core.doc().resolve(&key("lead")).unwrap().type_id, SYNTH);
    assert_eq!(live_name(&mut core, "lead"), synth_name);
    assert_eq!(core.plugin_instance_count(), 1);
}

#[test]
fn the_state_is_captured_one_quiet_second_after_the_plugin_marks_it_dirty() {
    let mut core = plugin_core(OfflineSink::new());
    add_plugin(&mut core, SYNTH, "lead");
    let state = |core: &AppCore<OfflineSink>| {
        core.doc()
            .resolve(&key("lead"))
            .unwrap()
            .plugin
            .as_ref()
            .unwrap()
            .state
            .is_some()
    };
    assert!(!state(&core), "nothing captured yet");
    core.plugin_tick(10.0);
    post(&mut core, &key("lead"), PluginMessage::StateDirty);
    core.frame();
    core.plugin_tick(10.5);
    assert!(!state(&core), "half a second is not quiet enough");
    core.plugin_tick(10.0 + STATE_QUIET_SECS);
    assert!(state(&core), "captured after the quiet second");
}
