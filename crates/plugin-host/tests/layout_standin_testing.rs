//! Descriptor helpers, the stand-in and the test-plugin locator
//! stubs (`testing.rs`).

mod support;

use atomik_core::BATCH;
use atomik_device_sdk::{
    Channels, DeviceKind, PluginFormat, PluginKindHint, PluginRef, Registry, descriptor_for,
};
use atomik_plugin_host::layout::{
    PluginSockets, layout_matches, param_ids, param_key, parse_param_key, truncate_params,
};
use atomik_plugin_host::stand_in;
use atomik_plugin_host::testing::{cdylib_file_name, clap_file_name};
use support::*;

#[test]
fn param_keys_are_p_followed_by_the_decimal_plugin_id() {
    assert_eq!(param_key(0), "p0");
    assert_eq!(param_key(42), "p42");
    assert_eq!(param_key(u32::MAX), "p4294967295");
    assert_eq!(parse_param_key("p42"), Some(42));
    assert_eq!(parse_param_key("p0"), Some(0));
    assert_eq!(parse_param_key("p4294967295"), Some(u32::MAX));
    for bad in [
        "bypass",
        "p",
        "p-1",
        "p4x",
        "P42",
        "42",
        "p4294967296",
        " p1",
    ] {
        assert_eq!(parse_param_key(bad), None, "{bad:?}");
    }
}

#[test]
fn the_plugin_param_ids_of_a_descriptor_skip_bypass_and_keep_the_plugins_order() {
    let layout = fx_layout();
    let d = descriptor("lay.ids", &layout);
    assert_eq!(param_ids(d), vec![1, 2, 3, 4]);
    let mut shuffled = layout.clone();
    shuffled.params.swap(0, 3);
    let d2 = descriptor("lay.ids2", &shuffled);
    assert_eq!(param_ids(d2), vec![4, 2, 3, 1]);
}

#[test]
fn a_long_param_table_is_cut_to_the_cap_and_the_cut_is_reported() {
    let mut l = fx_layout();
    assert!(!truncate_params(&mut l));
    assert_eq!(l.params.len(), 4);
    l.params = (0..2_100u32)
        .map(|i| param(i, "P", 0.0, 1.0, 0.5, false))
        .collect();
    assert!(truncate_params(&mut l));
    assert_eq!(l.params.len(), atomik_device_sdk::MAX_PLUGIN_PARAMS);
    assert_eq!(l.params[2_047].id, 2_047, "the first params stay");
    // A cut layout is accepted by `descriptor_for`.
    descriptor_for(
        &PluginRef {
            format: PluginFormat::Clap,
            id: "lay.cut".to_string(),
        },
        &l,
    )
    .unwrap();
}

#[test]
fn the_socket_ids_of_a_descriptor_are_found_by_key() {
    let d = descriptor("lay.sockets", &fx_layout());
    let s = PluginSockets::of(d);
    assert_eq!(d.sockets[usize::from(s.in_l.unwrap().0)].key, "in_l");
    assert_eq!(d.sockets[usize::from(s.sc_r.unwrap().0)].key, "sc_in_r");
    assert_eq!(d.sockets[usize::from(s.out_r.unwrap().0)].key, "out_r");
    assert!(s.notes_in.is_none() && s.notes_out.is_none());
    let synth = PluginSockets::of(descriptor("lay.sockets2", &synth_layout()));
    assert!(synth.in_l.is_none() && synth.sc_l.is_none());
    assert!(synth.notes_in.is_some() && synth.out_l.is_some());
}

#[test]
fn a_layout_matches_the_descriptor_built_from_it_and_no_other() {
    let l = fx_layout();
    let d = descriptor("lay.match", &l);
    assert!(layout_matches(&l, d));
    let mut other = l.clone();
    other.sidechain = false;
    assert!(!layout_matches(&other, d));
    let mut other = l.clone();
    other.notes_in = true;
    assert!(!layout_matches(&other, d));
    let mut other = l.clone();
    other.params.pop();
    assert!(!layout_matches(&other, d));
    let mut other = l;
    other.main_out = None;
    assert!(!layout_matches(&other, d));
}

#[test]
fn the_stand_in_of_an_effect_is_dry_and_of_an_instrument_silent() {
    let d = descriptor("lay.standin", &fx_layout());
    let dev = stand_in(d);
    assert_eq!(dev.descriptor().type_id, "clap.lay.standin");
    assert_eq!(dev.latency_frames(), 0);
    let s = descriptor("lay.standin-synth", &synth_layout());
    assert_eq!(s.kind, DeviceKind::Instrument);
    // The registry builds the same kind of stand-in for registered plugin types.
    let mut r = Registry::new();
    r.register_plugin(d.type_id.to_string(), d).unwrap();
    assert_eq!(r.create(d.type_id).unwrap().descriptor().type_id, d.type_id);
    let _ = (BATCH, PluginKindHint::Effect, Channels::Stereo);
}

#[test]
fn the_cdylib_file_names_follow_the_platform_convention() {
    let name = cdylib_file_name("atomik_test_clap");
    if cfg!(windows) {
        assert_eq!(name, "atomik_test_clap.dll");
    } else if cfg!(target_os = "macos") {
        assert_eq!(name, "libatomik_test_clap.dylib");
    } else {
        assert_eq!(name, "libatomik_test_clap.so");
    }
    assert_eq!(clap_file_name("atomik_test_clap"), "atomik_test_clap.clap");
}
