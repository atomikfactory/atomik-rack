//! The CLAP backend loads a plugin file and describes every plugin in it: exact layouts of the test plugins, loud failures for broken files.

mod support;

use atomik_device_sdk::{Channels, PluginKindHint, PluginLayout};
use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::testing::clap_test_plugin;
use atomik_plugin_host::{BackendError, PluginBackend, ScannedPlugin, mark_main_thread};
use support::*;

fn scan(package: &str) -> Vec<ScannedPlugin> {
    let (backend, path) = backend_for(package);
    backend.scan_file(&path).unwrap()
}

fn find<'a>(found: &'a [ScannedPlugin], id: &str) -> &'a ScannedPlugin {
    found.iter().find(|p| p.plugin.id == id).unwrap_or_else(|| {
        panic!(
            "{id} not found in {:?}",
            found.iter().map(|p| &p.plugin.id).collect::<Vec<_>>()
        )
    })
}

#[test]
fn the_test_library_lists_exactly_its_four_plugins_in_order() {
    let found = scan("atomik-test-clap");
    let ids: Vec<&str> = found.iter().map(|p| p.plugin.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "atomik.test.gain",
            "atomik.test.synth",
            "atomik.test.noteshift",
            "atomik.test.params"
        ]
    );
    assert!(found.iter().all(|p| p.unsupported.is_none()));
    assert!(found.iter().all(|p| !p.params_truncated));
    assert_eq!(found[0].plugin.type_id(), "clap.atomik.test.gain");
}

#[test]
fn the_effect_layout_is_exact() {
    let found = scan("atomik-test-clap");
    let l: &PluginLayout = &find(&found, "atomik.test.gain").layout;
    assert_eq!(l.name, "Atomik Test Gain");
    assert_eq!(l.vendor, "Atomik");
    assert_eq!(l.version, "1.0.0");
    assert_eq!(l.kind_hint, PluginKindHint::Effect);
    assert_eq!(l.main_in, Some(Channels::Stereo));
    assert_eq!(l.main_out, Some(Channels::Stereo));
    assert!(l.sidechain, "an aux input is the sidechain");
    assert!(!l.notes_in && !l.notes_out);
    let rows: Vec<(u32, &str, &str, f64, f64, f64, bool)> = l
        .params
        .iter()
        .map(|p| {
            (
                p.id,
                p.name.as_str(),
                p.module.as_str(),
                p.min,
                p.max,
                p.default,
                p.stepped,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (1, "Gain", "Main", -60.0, 12.0, 0.0, false),
            (2, "Mute", "Main", 0.0, 1.0, 0.0, true),
            (3, "Latency", "Advanced", 0.0, 512.0, 0.0, true),
            (4, "Duck", "Advanced", 0.0, 1.0, 1.0, false),
        ]
    );
    assert!(l.params.iter().all(|p| p.steps == 0));
}

#[test]
fn the_instrument_layout_is_exact() {
    let found = scan("atomik-test-clap");
    let l = &find(&found, "atomik.test.synth").layout;
    assert_eq!(l.kind_hint, PluginKindHint::Instrument);
    assert_eq!(l.main_in, None);
    assert_eq!(l.main_out, Some(Channels::Stereo));
    assert!(!l.sidechain);
    assert!(l.notes_in && !l.notes_out);
    let ids: Vec<u32> = l.params.iter().map(|p| p.id).collect();
    assert_eq!(ids, [1, 2]);
}

#[test]
fn the_note_effect_layout_is_exact() {
    let found = scan("atomik-test-clap");
    let l = &find(&found, "atomik.test.noteshift").layout;
    assert_eq!(l.kind_hint, PluginKindHint::NoteEffect);
    assert_eq!((l.main_in, l.main_out), (None, None));
    assert!(l.notes_in && l.notes_out);
    assert_eq!(l.params.len(), 1);
    assert_eq!((l.params[0].min, l.params[0].max), (-24.0, 24.0));
}

#[test]
fn the_param_plugin_lists_two_thousand_params_in_the_plugins_own_order() {
    let found = scan("atomik-test-clap");
    let p = find(&found, "atomik.test.params");
    assert_eq!(p.layout.params.len(), 2_000);
    assert!(
        p.layout
            .params
            .iter()
            .enumerate()
            .all(|(i, q)| q.id == i as u32)
    );
    assert_eq!(p.layout.params[1_999].name, "Grow");
    assert_eq!(p.layout.params[100].module, "Bank/Group 1");
    assert!(!p.params_truncated, "2 000 fits under the cap of 2 048");
}

#[test]
fn mono_buses_and_a_midi_only_synth_are_described() {
    let found = scan("atomik-test-clap-variants");
    let mono = &find(&found, "atomik.test.mono").layout;
    assert_eq!(mono.main_in, Some(Channels::Mono));
    assert_eq!(mono.main_out, Some(Channels::Mono));
    let midi = &find(&found, "atomik.test.midisynth").layout;
    assert_eq!(midi.kind_hint, PluginKindHint::Instrument);
    assert!(midi.notes_in);
}

#[test]
fn plugins_that_cannot_be_created_or_activated_are_listed_as_unsupported_with_the_reason() {
    let found = scan("atomik-test-clap-variants");
    let init = find(&found, "atomik.test.init-fails");
    let reason = init.unsupported.as_deref().expect("init fails");
    assert!(reason.contains("created"), "{reason}");
    let activate = find(&found, "atomik.test.activate-fails");
    let reason = activate.unsupported.as_deref().expect("activation fails");
    assert!(reason.contains("activated"), "{reason}");
    for good in [
        "atomik.test.mono",
        "atomik.test.midisynth",
        "atomik.test.error",
    ] {
        assert!(find(&found, good).unsupported.is_none(), "{good}");
    }
}

#[test]
fn a_library_whose_entry_init_fails_is_an_error_and_the_backend_carries_on() {
    mark_main_thread();
    let path = clap_test_plugin("atomik-test-clap-bad").unwrap();
    let backend = ClapBackend::new();
    let err = backend.scan_file(&path).unwrap_err();
    assert!(matches!(err, BackendError::Failed(_)), "{err:?}");
    // The same backend still scans a good file.
    let good = clap_test_plugin("atomik-test-clap-leak").unwrap();
    assert_eq!(backend.scan_file(&good).unwrap().len(), 1);
}

#[test]
fn a_file_that_is_not_a_plugin_is_not_a_plugin() {
    mark_main_thread();
    let dir = std::env::temp_dir().join("atomik-clap-scan-test");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("readme.clap");
    std::fs::write(&file, b"this is not a library").unwrap();
    let err = ClapBackend::new().scan_file(&file).unwrap_err();
    assert!(matches!(err, BackendError::NotAPlugin(_)), "{err:?}");
    let missing = dir.join("does-not-exist.clap");
    let err = ClapBackend::new().scan_file(&missing).unwrap_err();
    assert!(matches!(err, BackendError::NotAPlugin(_)), "{err:?}");
}

#[test]
fn instantiating_a_plugin_that_was_never_scanned_is_unknown() {
    mark_main_thread();
    let backend = ClapBackend::new();
    let (_device_end, manager) =
        atomik_plugin_host::plugin_channel(atomik_plugin_host::InstanceId(1));
    let err = backend
        .instantiate(
            &clap_ref("atomik.test.gain"),
            atomik_plugin_host::HostHandle::new(manager.messenger()),
        )
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(err, BackendError::UnknownPlugin(_)), "{err:?}");
}

#[test]
fn a_plugin_whose_init_fails_cannot_be_instantiated_but_scans_as_unsupported_or_failed() {
    let (backend, _path) = backend_for("atomik-test-clap-variants");
    let (_d, manager) = atomik_plugin_host::plugin_channel(atomik_plugin_host::InstanceId(2));
    let err = backend
        .instantiate(
            &clap_ref("atomik.test.init-fails"),
            atomik_plugin_host::HostHandle::new(manager.messenger()),
        )
        .map(|_| ())
        .unwrap_err();
    assert!(matches!(err, BackendError::Failed(_)), "{err:?}");
}
