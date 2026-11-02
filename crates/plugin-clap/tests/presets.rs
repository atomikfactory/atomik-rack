//! Against the real CLAP backend: the presets a plugin lists through preset discovery, loading
//! one through `preset-load`, and the param values read back afterwards.
// Justification: values set by a preset and read back are compared exactly.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_test_plugin_core as core;
use support::*;

const PKG: &str = "atomik-test-clap";

#[test]
fn a_plugin_lists_the_presets_it_offers_and_only_its_own() {
    let mut m = manager_for(&[PKG]);
    let gain = m.instantiate(&request("atomik.test.gain")).unwrap();
    let list = m.list_presets(gain.id);
    let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Quiet", "Loud"], "{list:?}");
    assert!(list.iter().all(|p| p.group == "Factory"));
    assert!(list.iter().all(|p| p.id.len() <= 20), "ids stay short");
    // A second listing gives the same ids.
    assert_eq!(list, m.list_presets(gain.id));

    let synth = m.instantiate(&request("atomik.test.synth")).unwrap();
    let names: Vec<String> = m
        .list_presets(synth.id)
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["Square Lead"]);
    // A plugin of the library that no preset names has none.
    let shift = m.instantiate(&request("atomik.test.noteshift")).unwrap();
    assert!(m.list_presets(shift.id).is_empty());
}

#[test]
fn loading_a_preset_changes_the_state_and_the_param_values_follow() {
    let mut m = manager_for(&[PKG]);
    let gain = m.instantiate(&request("atomik.test.gain")).unwrap();
    let id = gain.id;
    let before = m.read_params(id);
    assert!(before.contains(&(core::GainFx::GAIN, 0.0)), "{before:?}");
    let quiet = m
        .list_presets(id)
        .into_iter()
        .find(|p| p.name == "Quiet")
        .unwrap();
    m.load_preset(id, &quiet.id).unwrap();

    let after = m.read_params(id);
    assert!(after.contains(&(core::GainFx::GAIN, -12.0)), "{after:?}");
    let mut reference = core::GainFx::new(SR);
    reference.set_param(core::GainFx::GAIN, -12.0);
    assert_eq!(
        m.capture_state(id).unwrap().0,
        reference.save_state(),
        "the plugin saves what the preset set"
    );

    // Another preset moves another set of values.
    let loud = m
        .list_presets(id)
        .into_iter()
        .find(|p| p.name == "Loud")
        .unwrap();
    m.load_preset(id, &loud.id).unwrap();
    let loud_values = m.read_params(id);
    assert!(loud_values.contains(&(core::GainFx::GAIN, 6.0)));
    assert!(loud_values.contains(&(core::GainFx::DUCK, 0.25)));
}

#[test]
fn an_id_the_plugin_never_listed_is_refused() {
    let mut m = manager_for(&[PKG]);
    let gain = m.instantiate(&request("atomik.test.gain")).unwrap();
    for bad in ["", "c0000000000000000", "../../x", "quiet"] {
        let e = m.load_preset(gain.id, bad).expect_err("refused");
        assert!(e.to_string().contains("no preset"), "{bad}: {e}");
    }
    // The plugin kept its state.
    assert!(m.read_params(gain.id).contains(&(core::GainFx::GAIN, 0.0)));
}
