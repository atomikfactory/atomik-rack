//! `.vstpreset` files: written from a saved state, listed from a preset folder and loaded into
//! a live plugin, which then saves the same component state.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_device_sdk::PluginStateBlob;
use atomik_plugin_host::{BackendError, PluginBackend};
use atomik_plugin_vst3::{Vst3Backend, vstpreset_from_state};
use atomik_test_plugin_core as core;
use atomik_testkit::harness::DeviceHarness;
use std::path::PathBuf;
use support::*;

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("atomik-vst3-presets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn backend_with(roots: Vec<PathBuf>, kit: &Kit) -> Vst3Backend {
    let backend = Vst3Backend::new().with_preset_roots(roots);
    backend.scan_file(&kit.path).expect("the library scans");
    backend
}

fn reference_state() -> PluginStateBlob {
    let mut fx = core::GainFx::new(SR);
    fx.set_param(core::GainFx::GAIN, -9.0);
    fx.set_param(core::GainFx::LATENCY, 13.0);
    wrap_state(Format::Vst3, &fx.save_state())
}

#[test]
fn a_preset_file_is_listed_and_loads_into_the_plugin() {
    let _serial = serial();
    let k = kit(Format::Vst3);
    let plugin = k.plugin(Plug::Gain);
    let root = scratch("load");
    let folder = root.join("Factory");
    std::fs::create_dir_all(&folder).unwrap();
    let saved = reference_state();
    let file = vstpreset_from_state(&plugin.id, &saved).unwrap();
    std::fs::write(folder.join("Quiet.vstpreset"), &file).unwrap();
    std::fs::write(folder.join("notes.txt"), b"not a preset").unwrap();

    let backend = backend_with(vec![root.clone()], &k);
    let mut rig = rig_for(&backend, &plugin, atomik_device_sdk::RunMode::Realtime);
    let presets = rig.instance.list_presets();
    assert_eq!(presets.len(), 1, "{presets:?}");
    assert_eq!(presets[0].name, "Quiet");
    assert_eq!(presets[0].group, "Factory");

    let before = rig.instance.capture_state().unwrap();
    assert_ne!(
        component_state(Format::Vst3, &before),
        component_state(Format::Vst3, &saved)
    );
    rig.instance.load_preset(&presets[0].id).unwrap();
    let after = rig.instance.capture_state().unwrap();
    assert_eq!(
        component_state(Format::Vst3, &after),
        component_state(Format::Vst3, &saved),
        "the plugin now holds the preset's component state"
    );
    let _ = DeviceHarness::new(Box::new(rig.device.take().unwrap()), SR);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_preset_of_another_plugin_or_outside_the_folder_is_refused() {
    let _serial = serial();
    let k = kit(Format::Vst3);
    let plugin = k.plugin(Plug::Gain);
    let root = scratch("refuse");
    let other = k.plugin(Plug::Synth);
    let wrong = vstpreset_from_state(&other.id, &reference_state()).unwrap();
    std::fs::write(root.join("Other.vstpreset"), wrong).unwrap();
    std::fs::write(root.join("Broken.vstpreset"), b"VST3 broken").unwrap();

    let backend = backend_with(vec![root.clone()], &k);
    let mut rig = rig_for(&backend, &plugin, atomik_device_sdk::RunMode::Realtime);
    let presets = rig.instance.list_presets();
    assert_eq!(presets.len(), 2);
    for p in &presets {
        let e = rig.instance.load_preset(&p.id).expect_err("refused");
        assert!(matches!(e, BackendError::Failed(_)), "{e}");
    }
    let e = rig
        .instance
        .load_preset("0/../x.vstpreset")
        .expect_err("outside");
    assert!(matches!(e, BackendError::Failed(_)), "{e}");
    let _ = std::fs::remove_dir_all(&root);
}
