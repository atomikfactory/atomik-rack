//! The MIDI controller proxies a plugin publishes as parameters (one per channel and controller
//! number) stay out of the parameter table, the real parameters around them stay in, the
//! controllers still reach the plugin, and a plugin that answers `IMidiMapping` too slowly keeps
//! its full table.

mod support;

use atomik_core::{BATCH, Batch};
use atomik_device_sdk::{MAX_PLUGIN_PARAMS, PluginLayout, RunMode};
use atomik_plugin_host::{
    EventList, PluginEvent, PluginEventKind as K, PluginIo, PluginProcessor, PluginTransport,
    ScannedPlugin,
};
use support::*;

fn scanned(n: u32) -> ScannedPlugin {
    let k = kit_variants(Format::Vst3);
    let found = k.backend.scan_file(&k.path).unwrap();
    let id = variant_id(n);
    found
        .into_iter()
        .find(|p| p.plugin.id == id)
        .expect("the variants library lists it")
}

fn ids(layout: &PluginLayout) -> Vec<u32> {
    layout.params.iter().map(|p| p.id).collect()
}

/// Runs one block of an effect with the given events; returns how many the plugin dropped.
fn run(p: &mut dyn PluginProcessor, events: &[PluginEvent]) -> u32 {
    let mut input = [[0.0; BATCH]; 2];
    input[0][0] = 0.5;
    let mut output: [Batch; 2] = [[0.0; BATCH]; 2];
    let mut out_events = EventList::with_capacity(64);
    let mut io = PluginIo {
        frames: BATCH,
        main_in: &input,
        main_in_silent: false,
        sidechain: &[],
        sidechain_silent: true,
        main_out: &mut output,
        aux_out: &mut [],
        events_in: events,
        events_out: &mut out_events,
        transport: PluginTransport::default(),
        dropped: 0,
    };
    p.process(&mut io);
    io.dropped
}

#[test]
fn the_proxies_are_not_listed_and_the_real_params_after_them_are() {
    let p = scanned(V_PROXIES);
    assert!(p.unsupported.is_none(), "{:?}", p.unsupported);
    assert_eq!(ids(&p.layout), [1, 2, 3, 4]);
    assert!(
        !p.params_truncated,
        "2 048 proxies no longer push the real params over the cap"
    );
}

#[test]
fn a_live_instance_lists_the_same_table_as_the_scan() {
    let k = kit_variants(Format::Vst3);
    let (instance, _m) = bare(&*k.backend, &vst3_ref(&variant_id(V_PROXIES)));
    assert_eq!(ids(instance.layout()), [1, 2, 3, 4]);
}

#[test]
fn the_controllers_still_reach_the_plugin_through_the_mapping() {
    let k = kit_variants(Format::Vst3);
    let lib = TestLib::open_path(&k.path);
    let (mut inst, _m) = bare(&*k.backend, &vst3_ref(&variant_id(V_PROXIES)));
    let mut p = inst.activate(&activate_config(RunMode::Realtime)).unwrap();
    assert!(p.start_processing());
    let cc = |controller: u8| PluginEvent {
        offset: 1,
        kind: K::Cc {
            controller,
            value: 0.5,
        },
    };
    let dropped = run(&mut *p, &[cc(7), cc(74), cc(1)]);
    assert_eq!(dropped, 0, "every controller has an assignment");
    assert_eq!(
        lib.probe(&*inst, probe::PARAM_POINTS),
        3,
        "the plugin saw all three as parameter changes"
    );
    inst.deactivate(p);
}

#[test]
fn a_plugin_that_answers_the_mapping_slowly_keeps_its_full_table() {
    let p = scanned(V_PROXIES_SLOW);
    assert!(p.unsupported.is_none(), "{:?}", p.unsupported);
    assert_eq!(
        p.layout.params.len(),
        MAX_PLUGIN_PARAMS,
        "the host stopped asking and listed everything, up to the cap"
    );
    assert_eq!(p.layout.params[0].id, 1);
    assert_eq!(p.layout.params[1].id, PROXY_BASE);
    assert!(
        p.params_truncated,
        "the real params after the proxies are cut, as before"
    );
}
