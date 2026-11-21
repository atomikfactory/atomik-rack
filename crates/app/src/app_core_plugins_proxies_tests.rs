//! A plugin that publishes MIDI controller stand-ins as parameters has them left out of its
//! table, and a project that still uses one keeps it: the stored value, a lane added on it, the
//! row in the saved layout and the descriptor all survive the plugin going live, a save and a
//! load, and a rescan.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_device_sdk::PluginParamInfo;
use atomik_model::{MidiMapping, MidiSource, MidiTarget, SongCommand, SourceKind, save_project};

/// The controller stand-in for controller 5 on channel 1 of the proxies test plugin.
const PROXY: u32 = 1_005;
/// A stand-in nothing uses.
const OTHER_PROXY: u32 = 1_006;
const NAME: &str = "Atomik Test Proxies";

fn proxies_core() -> AppCore<OfflineSink> {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(true, &[PKG_VST3_VARIANTS]));
    core
}

fn row(id: u32) -> PluginParamInfo {
    PluginParamInfo {
        id,
        name: format!("MIDI CC {} Chan 1", id - 1_000),
        module: "MIDI Params".to_string(),
        min: 0.0,
        max: 1.0,
        default: 0.5,
        stepped: false,
        steps: 0,
    }
}

fn type_id(core: &AppCore<OfflineSink>) -> String {
    vst3_id(core, NAME)
}

fn ids(core: &AppCore<OfflineSink>, k: &str) -> Vec<u32> {
    core.doc()
        .resolve(&key(k))
        .and_then(|d| d.plugin.as_deref())
        .map(|p| p.layout.params.iter().map(|r| r.id).collect())
        .unwrap_or_default()
}

fn listed(core: &AppCore<OfflineSink>, param: &str) -> bool {
    core.registry()
        .get(&type_id(core))
        .is_some_and(|f| f.descriptor.param_by_key(param).is_some())
}

/// A project saved by a scanner that listed every stand-in: the device `px` has a stored value
/// on `PROXY` and the layout rows of the real params and of that stand-in (the way a save
/// keeps them), and the plugin is not live yet.
fn project_using_a_proxy(core: &mut AppCore<OfflineSink>) -> atomik_model::RackDoc {
    let id = type_id(core);
    add_plugin(core, &id, "px");
    let mut doc = core.doc().clone();
    let d = doc
        .devices
        .iter_mut()
        .find(|d| d.key == key("px"))
        .expect("the device");
    d.params.insert(format!("p{PROXY}"), 0.75);
    d.plugin.as_mut().unwrap().layout.params.push(row(PROXY));
    doc
}

#[test]
fn a_project_that_uses_no_stand_in_lists_the_plugins_own_params_only() {
    let mut core = proxies_core();
    let id = type_id(&core);
    add_plugin(&mut core, &id, "px");
    assert_eq!(core.plugin_instance_count(), 1);
    assert_eq!(
        ids(&core, "px"),
        [1, 2, 3, 4],
        "no stand-ins in the document"
    );
    assert!(!listed(&core, &format!("p{OTHER_PROXY}")));
    assert!(!listed(&core, &format!("p{PROXY}")));
    assert!(
        core.last_rejection().is_none(),
        "{:?}",
        core.last_rejection()
    );
}

#[test]
fn a_stand_in_the_project_uses_stays_listed_before_and_after_the_plugin_is_live() {
    let mut core = proxies_core();
    let doc = project_using_a_proxy(&mut core);
    core.open_doc(doc);
    // Before the instance exists the descriptor already has it: the stored value is not dead.
    assert!(listed(&core, &format!("p{PROXY}")));
    assert!(
        !listed(&core, &format!("p{OTHER_PROXY}")),
        "only what is used"
    );
    core.wait_plugins();
    assert_eq!(core.plugin_instance_count(), 1);
    assert_eq!(
        ids(&core, "px"),
        [1, 2, 3, 4, PROXY],
        "the live layout replaced the snapshot and kept the row"
    );
    let px = core.doc().resolve(&key("px")).unwrap();
    assert_eq!(px.params.get(&format!("p{PROXY}")), Some(&0.75));
    assert!(listed(&core, &format!("p{PROXY}")));
    // The value is a live parameter of the plugin: it can be set and a lane can be added.
    set_param(&mut core, "px", &format!("p{PROXY}"), 0.5);
    apply(
        &mut core,
        Command::Song(SongCommand::AddAutomationLane {
            device: key("px"),
            param: format!("p{PROXY}"),
        }),
    );
    assert!(
        core.last_rejection().is_none(),
        "{:?}",
        core.last_rejection()
    );
}

#[test]
fn the_lane_the_value_and_the_row_survive_a_save_a_load_and_a_rescan() {
    let mut core = proxies_core();
    let doc = project_using_a_proxy(&mut core);
    core.open_doc(doc);
    core.wait_plugins();
    apply(
        &mut core,
        Command::Song(SongCommand::AddAutomationLane {
            device: key("px"),
            param: format!("p{PROXY}"),
        }),
    );
    let saved = save_project(core.doc());

    // A fresh session loads the file: the lane is not dead.
    let mut later = proxies_core();
    later.open_str(&saved, None).unwrap();
    assert!(
        listed(&later, &format!("p{PROXY}")),
        "listed as soon as the project is open"
    );
    later.wait_plugins();
    assert_eq!(later.plugin_instance_count(), 1);
    assert_eq!(ids(&later, "px"), [1, 2, 3, 4, PROXY]);
    let px = later.doc().resolve(&key("px")).unwrap();
    assert_eq!(px.params.get(&format!("p{PROXY}")), Some(&0.75));
    assert!(
        later
            .doc()
            .automated_params()
            .contains(&(key("px"), format!("p{PROXY}"))),
        "the lane is still there"
    );
    assert_eq!(
        save_project(later.doc()),
        saved,
        "save, load, save is stable"
    );

    // A rescan (the cache comes back the way a scan makes it) keeps the descriptor and the row.
    later.configure_plugins(config(true, &[PKG_VST3_VARIANTS]));
    later.wait_plugins();
    assert!(listed(&later, &format!("p{PROXY}")));
    assert_eq!(ids(&later, "px"), [1, 2, 3, 4, PROXY]);
    assert_eq!(save_project(later.doc()), saved);
}

#[test]
fn a_learned_midi_control_on_a_stand_in_keeps_it() {
    let mut core = proxies_core();
    let mut doc = project_using_a_proxy(&mut core);
    // The value is not what keeps it here: the mapping is.
    doc.devices
        .iter_mut()
        .find(|d| d.key == key("px"))
        .unwrap()
        .params
        .clear();
    doc.midi_maps.push(MidiMapping {
        id: 1,
        source: MidiSource::any_port(Some(0), SourceKind::Cc(7)),
        target: MidiTarget::Param {
            device: key("px"),
            param: format!("p{PROXY}"),
        },
        options: atomik_model::MapOptions::default(),
    });
    core.open_doc(doc);
    core.wait_plugins();
    assert_eq!(ids(&core, "px"), [1, 2, 3, 4, PROXY]);
    assert!(listed(&core, &format!("p{PROXY}")));
    let saved = save_project(core.doc());
    let mut later = proxies_core();
    later.open_str(&saved, None).unwrap();
    later.wait_plugins();
    assert_eq!(ids(&later, "px"), [1, 2, 3, 4, PROXY]);
    assert_eq!(later.doc().midi_maps.len(), 1);
}
