//! A plugin whose layout changes at run time (new aux outputs) and a built-in instrument replaced
//! by a plugin: the same creation path as a plain add, and one layout everywhere.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_device_sdk::PluginRef;
use atomik_model::{CreateOptions, CreateSource, WorkflowCommand};

const BUILTIN: &str = "atomik.poly-synth";

fn source_of(core: &AppCore<OfflineSink>, type_id: &str) -> CreateSource {
    let plugin = PluginRef::from_type_id(type_id).expect("a plugin type id");
    let entry = core
        .plugin_catalog()
        .entry(&plugin)
        .expect("in the catalog");
    CreateSource::Plugin {
        plugin,
        layout: entry.layout.clone(),
        path_hint: Some(entry.path.to_string_lossy().into_owned()),
    }
}

fn builtin_core() -> AppCore<OfflineSink> {
    let mut core = plugin_core(OfflineSink::new());
    apply(
        &mut core,
        Command::Workflow(WorkflowCommand::CreateInstrument {
            index: 0,
            source: CreateSource::Type(BUILTIN.into()),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        }),
    );
    core
}

#[test]
fn replacing_a_built_in_instrument_by_a_plugin_gives_a_live_instance_and_undo_restores_it() {
    let mut core = builtin_core();
    let source = source_of(&core, SYNTH);
    core.replace_device_with(key("lead"), source);
    core.wait_plugins();
    settle(&mut core);
    let d = core.doc().resolve(&key("lead")).unwrap();
    assert_eq!(d.type_id, SYNTH);
    assert!(d.plugin.is_some());
    assert!(
        core.plugin_instance(&key("lead")).is_some(),
        "the replacement is live, not a stand-in"
    );
    assert!(core.undo());
    core.wait_plugins();
    settle(&mut core);
    assert_eq!(core.doc().resolve(&key("lead")).unwrap().type_id, BUILTIN);
    assert!(core.plugin_instance(&key("lead")).is_none());
}

// ------------------------------------------------------------------ a layout that changes

use atomik_device_sdk::PluginLayout;
use atomik_model::{Cable, SocketRef, empty_doc};
use atomik_plugin_host::{NullBackend, NullBehavior, NullPlugin, NullStats, PluginMessage};
use atomik_plugin_sandbox::HostCommand;
use atomik_render::plugins::{PluginHost, PluginIsolation};
use std::sync::Arc;

/// The test gain effect as the scan found it.
fn scanned_gain() -> atomik_plugin_host::ScannedPlugin {
    config(true, &[PKG])
        .cache
        .files()
        .flat_map(|f| f.plugins.iter())
        .find(|p| p.plugin.type_id() == GAIN)
        .expect("the test gain effect is scanned")
        .clone()
}

/// The layout after the plugin changed its buses: a sidechain input and two aux output pairs.
fn with_aux(layout: &PluginLayout) -> PluginLayout {
    let mut grown = layout.clone();
    grown.sidechain = true;
    grown.aux_out = 2;
    grown
}

/// A core whose plugin is a virtual gain effect with `live` as its layout, over a scan cache that
/// holds `cache_layout` for it (the scan an older plugins.json has).
fn null_core(
    live: PluginLayout,
    cache_layout: PluginLayout,
) -> (AppCore<OfflineSink>, Arc<NullStats>) {
    let mut config = config(true, &[PKG]);
    let scanned = scanned_gain();
    config.cache.update_layout(&scanned.plugin, &cache_layout);
    let backend = NullBackend::new().with_plugin(NullPlugin {
        path: PathBuf::from("null.clap"),
        plugin: scanned.plugin,
        layout: live,
        behavior: NullBehavior::Gain(0.5),
        latency: 0,
    });
    let stats = backend.stats();
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.plugins.host = PluginHost::with_backends(
        PluginIsolation::Sandbox {
            command: HostCommand::new("atomik-plugin-host"),
            bounce: false,
        },
        vec![Box::new(backend)],
    );
    core.configure_plugins(config);
    (core, stats)
}

fn socket(device: &str, socket: &str) -> SocketRef {
    SocketRef {
        device: key(device),
        socket: socket.to_string(),
    }
}

fn cable(from: (&str, &str), to: (&str, &str)) -> Command {
    Command::Connect(Cable {
        from: socket(from.0, from.1),
        to: socket(to.0, to.1),
    })
}

fn has_cable(core: &AppCore<OfflineSink>, from: (&str, &str), to: (&str, &str)) -> bool {
    core.doc().cables.contains(&Cable {
        from: socket(from.0, from.1),
        to: socket(to.0, to.1),
    })
}

fn registered_has(core: &AppCore<OfflineSink>, socket: &str) -> bool {
    core.registry()
        .descriptor(GAIN)
        .is_some_and(|d| d.socket_by_key(socket).is_some())
}

fn catalog_aux(core: &AppCore<OfflineSink>) -> u8 {
    let plugin = PluginRef::from_type_id(GAIN).unwrap();
    core.plugin_catalog().entry(&plugin).unwrap().layout.aux_out
}

/// Posts a restart request to `device`'s instance and runs frames until `done`.
fn restart_until(
    core: &mut AppCore<OfflineSink>,
    device: &str,
    done: impl Fn(&AppCore<OfflineSink>) -> bool,
) {
    post(core, &key(device), PluginMessage::RequestRestart);
    for _ in 0..60 {
        settle(core);
        if done(core) {
            break;
        }
    }
    core.wait_plugins();
    settle(core);
    assert!(
        done(core),
        "the restart did not finish: {:?} {:?}",
        core.last_rejection(),
        core.plugin_log()
    );
}

/// A core with two gain effects, `a` cabled into `b`, whose plugin then grew aux outputs.
fn grown_core() -> (AppCore<OfflineSink>, Arc<NullStats>, PluginLayout) {
    let base = scanned_gain().layout;
    let (mut core, stats) = null_core(base.clone(), base.clone());
    add_plugin(&mut core, GAIN, "a");
    add_plugin(&mut core, GAIN, "b");
    apply(&mut core, cable(("a", "out_l"), ("b", "in_l")));
    stats.change_layout(with_aux(&base));
    restart_until(&mut core, "a", |c| registered_has(c, "out_aux_1_l"));
    (core, stats, base)
}

#[test]
fn a_plugin_that_grows_aux_outputs_is_learned_by_the_catalog_and_the_cache() {
    let (core, _stats, base) = grown_core();
    assert_eq!(catalog_aux(&core), 2, "the catalog has the live layout");
    let plugin = PluginRef::from_type_id(GAIN).unwrap();
    let cached = core.plugin_cache().layout_of(&plugin).unwrap();
    assert_eq!(
        cached.aux_out, 2,
        "and so has the cache that plugins.json is saved from"
    );
    assert!(cached.sidechain);
    assert_ne!(cached, &base);
    assert!(registered_has(&core, "out_aux_2_r"));
    assert!(registered_has(&core, "sc_in_l"));
}

#[test]
fn a_second_instance_of_a_plugin_that_changed_its_layout_is_created() {
    let (mut core, _stats, _) = grown_core();
    assert_eq!(core.plugin_instance_count(), 2);
    // The row the user drags is the catalog's: it has the new layout.
    add_plugin(&mut core, GAIN, "c");
    assert_eq!(core.plugin_instance_count(), 3);
    assert!(core.plugin_instance(&key("c")).is_some());
    assert!(
        core.last_rejection().is_none(),
        "{:?}",
        core.last_rejection()
    );
}

#[test]
fn an_add_that_carries_the_layout_from_before_the_change_is_still_created() {
    let base = scanned_gain().layout;
    let (mut core, stats) = null_core(base.clone(), base.clone());
    add_plugin(&mut core, GAIN, "a");
    // A browser row (or a script) built before the plugin changed its layout.
    let stale = add_command(&core, GAIN, "late");
    stats.change_layout(with_aux(&base));
    restart_until(&mut core, "a", |c| registered_has(c, "out_aux_1_l"));
    apply(&mut core, stale);
    core.wait_plugins();
    settle(&mut core);
    assert!(
        core.plugin_instance(&key("late")).is_some(),
        "created from the freshest layout"
    );
    let late = core.doc().resolve(&key("late")).unwrap();
    assert_eq!(late.plugin.as_ref().unwrap().layout.aux_out, 2);
}

#[test]
fn the_cables_of_a_device_survive_a_layout_change_on_the_ports_that_remain() {
    let (mut core, _stats, _) = grown_core();
    assert!(has_cable(&core, ("a", "out_l"), ("b", "in_l")));
    // The new aux port can be cabled and the plugin stays live.
    apply(&mut core, cable(("a", "out_aux_1_l"), ("b", "in_r")));
    settle(&mut core);
    assert!(has_cable(&core, ("a", "out_aux_1_l"), ("b", "in_r")));
    assert!(core.plugin_instance(&key("a")).is_some());
}

#[test]
fn a_project_saved_with_a_cable_on_a_new_port_reopens_over_an_older_scan() {
    let (mut core, _stats, base) = grown_core();
    apply(&mut core, cable(("a", "out_aux_1_l"), ("b", "in_r")));
    settle(&mut core);
    let text = atomik_model::save_project(core.doc());
    drop(core);

    // A new run: the scan cache is the old one (no aux outputs), the plugin has them.
    let (mut reopened, _stats) = null_core(with_aux(&base), base.clone());
    assert!(!registered_has(&reopened, "out_aux_1_l"));
    reopened.open_str(&text, None).expect("the project opens");
    reopened.wait_plugins();
    settle(&mut reopened);
    assert!(has_cable(&reopened, ("a", "out_l"), ("b", "in_l")));
    assert!(
        has_cable(&reopened, ("a", "out_aux_1_l"), ("b", "in_r")),
        "the cable on the new port is kept"
    );
    assert_eq!(reopened.plugin_instance_count(), 2);
    assert!(registered_has(&reopened, "out_aux_1_l"));
    assert_eq!(catalog_aux(&reopened), 2);
    // And the plugin can be added once more.
    add_plugin(&mut reopened, GAIN, "c");
    assert!(reopened.plugin_instance(&key("c")).is_some());
}

#[test]
fn a_port_that_vanishes_takes_only_its_own_cables_and_says_so() {
    let (mut core, stats, base) = grown_core();
    apply(&mut core, cable(("a", "out_aux_1_l"), ("b", "in_r")));
    settle(&mut core);
    // The plugin goes back to its base buses.
    stats.change_layout(base.clone());
    restart_until(&mut core, "a", |c| !registered_has(c, "out_aux_1_l"));
    assert!(!has_cable(&core, ("a", "out_aux_1_l"), ("b", "in_r")));
    assert!(
        has_cable(&core, ("a", "out_l"), ("b", "in_l")),
        "the other cable stays"
    );
    assert!(
        core.last_rejection()
            .is_some_and(|m| m.contains("out_aux_1_l")),
        "{:?}",
        core.last_rejection()
    );
    assert_eq!(core.plugin_instance_count(), 2);
}
