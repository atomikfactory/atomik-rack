//! Both backends load a plugin file and describe every plugin in it: exact layouts of the test plugins through the same `PluginBackend`
//! trait, then what only VST3 has (split plugins, bus arrangements, unsupported cases).

mod support;

use atomik_device_sdk::{Channels, PluginKindHint, PluginLayout};
use atomik_plugin_host::PluginBackend;
use atomik_plugin_host::testing::vst3_test_plugin;
use atomik_plugin_host::{BackendError, ScannedPlugin, mark_main_thread};
use atomik_plugin_vst3::Vst3Backend;
use support::*;

fn scan(format: Format) -> (Kit, Vec<ScannedPlugin>) {
    let k = kit(format);
    let found = k.backend.scan_file(&k.path).unwrap();
    (k, found)
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
    for format in FORMATS {
        let (k, found) = scan(format);
        let ids: Vec<String> = found.iter().map(|p| p.plugin.id.clone()).collect();
        let want: Vec<String> = [Plug::Gain, Plug::Synth, Plug::Shift, Plug::Params]
            .iter()
            .map(|p| k.id(*p))
            .collect();
        assert_eq!(ids, want, "{}", format.name());
        assert!(
            found.iter().all(|p| p.unsupported.is_none()),
            "{}",
            format.name()
        );
        assert!(found.iter().all(|p| !p.params_truncated));
        assert_eq!(
            found[0].plugin.type_id(),
            format!("{}.{}", format.name(), want[0])
        );
    }
}

#[test]
fn the_vst3_type_id_is_the_class_id_in_hex() {
    let (_, found) = scan(Format::Vst3);
    assert_eq!(
        found[0].plugin.type_id(),
        "vst3.a70f1c014a5b4c018d2e1f0000000001"
    );
    assert_eq!(found[0].plugin.id.len(), 32);
    assert!(
        found[0]
            .plugin
            .id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
}

#[test]
fn the_effect_layout_is_exact_in_both_formats() {
    for format in FORMATS {
        let (k, found) = scan(format);
        let l: &PluginLayout = &find(&found, &k.id(Plug::Gain)).layout;
        assert_eq!(l.name, "Atomik Test Gain");
        assert_eq!(l.vendor, "Atomik");
        assert_eq!(l.version, "1.0.0");
        assert_eq!(l.kind_hint, PluginKindHint::Effect);
        assert_eq!(l.main_in, Some(Channels::Stereo));
        assert_eq!(l.main_out, Some(Channels::Stereo));
        assert!(l.sidechain, "an aux input is the sidechain");
        assert!(!l.notes_in && !l.notes_out);
        let rows: Vec<(u32, &str, &str, bool)> = l
            .params
            .iter()
            .map(|p| (p.id, p.name.as_str(), p.module.as_str(), p.stepped))
            .collect();
        assert_eq!(
            rows,
            [
                (1, "Gain", "Main", false),
                (2, "Mute", "Main", true),
                (3, "Latency", "Advanced", true),
                (4, "Duck", "Advanced", false),
            ],
            "{}",
            format.name()
        );
    }
}

#[test]
fn the_effect_param_values_are_the_formats_own() {
    // CLAP: plain min, max, default. VST3: normalised default and the step count.
    let (_, found) = scan(Format::Clap);
    let clap = &find(&found, "atomik.test.gain").layout.params;
    assert_eq!(
        (clap[0].min, clap[0].max, clap[0].default),
        (-60.0, 12.0, 0.0)
    );
    assert!(clap.iter().all(|p| p.steps == 0));
    let (k, found) = scan(Format::Vst3);
    let vst = &find(&found, &k.id(Plug::Gain)).layout.params;
    assert_eq!(
        vst[0].default, 0.5,
        "gain 0 dB is the middle of -64 to 64 dB"
    );
    assert_eq!(
        (vst[1].stepped, vst[1].steps, vst[1].default),
        (true, 1, 0.0)
    );
    assert_eq!(
        (vst[2].stepped, vst[2].steps, vst[2].default),
        (true, 512, 0.0)
    );
    assert_eq!(vst[3].default, 1.0);
    assert_eq!(vst[0].steps, 0);
}

#[test]
fn the_instrument_and_the_note_effect_layouts_are_exact() {
    for format in FORMATS {
        let (k, found) = scan(format);
        let l = &find(&found, &k.id(Plug::Synth)).layout;
        assert_eq!(l.kind_hint, PluginKindHint::Instrument, "{}", format.name());
        assert_eq!(l.main_in, None);
        assert_eq!(l.main_out, Some(Channels::Stereo));
        assert!(!l.sidechain);
        assert!(l.notes_in && !l.notes_out);
        let ids: Vec<u32> = l.params.iter().map(|p| p.id).collect();
        assert_eq!(ids, [1, 2], "hidden MIDI params are not listed");
        let l = &find(&found, &k.id(Plug::Shift)).layout;
        assert_eq!(l.kind_hint, PluginKindHint::NoteEffect);
        assert_eq!((l.main_in, l.main_out), (None, None));
        assert!(l.notes_in && l.notes_out);
        assert_eq!(l.params.len(), 1);
    }
}

#[test]
fn the_param_plugin_lists_two_thousand_params_in_the_plugins_own_order() {
    for format in FORMATS {
        let (k, found) = scan(format);
        let p = find(&found, &k.id(Plug::Params));
        assert_eq!(p.layout.params.len(), 2_000, "{}", format.name());
        assert!(
            p.layout
                .params
                .iter()
                .enumerate()
                .all(|(i, q)| q.id == i as u32)
        );
        assert_eq!(p.layout.params[1_999].name, "Grow");
        assert_eq!(p.layout.params[100].module, "Bank/Group 1");
        assert_eq!(p.layout.params[0].module, "Bank/Group 0");
        assert!(!p.params_truncated);
    }
}

#[test]
fn scanning_registers_the_file_of_every_plugin_so_instantiate_needs_no_scan_result() {
    for format in FORMATS {
        let (k, _) = scan(format);
        let (instance, _m) = bare(&*k.backend, &k.plugin(Plug::Synth));
        assert_eq!(instance.layout().name, "Atomik Test Synth");
    }
}

#[test]
fn a_missing_file_and_a_file_that_is_not_a_plugin_are_not_plugins() {
    mark_main_thread();
    let dir = std::env::temp_dir().join(format!("atomik-vst3-scan-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let junk = dir.join("junk.vst3");
    std::fs::write(&junk, b"not a library").unwrap();
    let b = Vst3Backend::new();
    assert!(matches!(
        b.scan_file(&dir.join("missing.vst3")),
        Err(BackendError::NotAPlugin(_))
    ));
    assert!(matches!(
        b.scan_file(&junk),
        Err(BackendError::NotAPlugin(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unknown_plugin_cannot_be_instantiated() {
    mark_main_thread();
    let b = Vst3Backend::new();
    let (_d, m) = atomik_plugin_host::plugin_channel(atomik_plugin_host::InstanceId(1));
    let err = b
        .instantiate(
            &vst3_ref(&"0".repeat(32)),
            atomik_plugin_host::HostHandle::new(m.messenger()),
        )
        .err()
        .unwrap();
    assert!(matches!(err, BackendError::UnknownPlugin(_)), "{err:?}");
}

// ----- VST3 specifics ----------------------------------------------------------------------

fn variants() -> (Vst3Backend, Vec<ScannedPlugin>) {
    mark_main_thread();
    let path = vst3_test_plugin("atomik-test-vst3-variants").unwrap();
    let b = Vst3Backend::new();
    let found = b.scan_file(&path).unwrap();
    (b, found)
}

#[test]
fn a_split_plugin_gets_its_params_from_the_controller_class() {
    let (_, found) = variants();
    let p = find(&found, &variant_id(V_SPLIT));
    assert_eq!(p.layout.name, "Atomik Test Split Gain");
    assert!(p.unsupported.is_none(), "{:?}", p.unsupported);
    let ids: Vec<u32> = p.layout.params.iter().map(|q| q.id).collect();
    assert_eq!(ids, [1, 3]);
    assert_eq!(p.layout.params[1].steps, 512);
    assert!(p.layout.sidechain);
}

#[test]
fn a_mono_only_main_bus_is_negotiated_and_reported_as_mono() {
    let (_, found) = variants();
    let l = &find(&found, &variant_id(V_MONO)).layout;
    assert_eq!(l.main_in, Some(Channels::Mono));
    assert_eq!(l.main_out, Some(Channels::Mono));
    assert!(find(&found, &variant_id(V_MONO)).unsupported.is_none());
}

#[test]
fn a_surround_only_plugin_is_listed_as_unsupported_with_the_reason() {
    let (_, found) = variants();
    let p = find(&found, &variant_id(V_SURROUND));
    let why = p.unsupported.as_deref().expect("unsupported");
    assert!(why.contains("6 channels"), "{why}");
    assert!(why.contains("mono or stereo"), "{why}");
}

#[test]
fn plugins_that_fail_to_initialise_or_set_up_are_listed_as_unsupported() {
    let (_, found) = variants();
    for n in [V_INIT, V_ACTIVATE] {
        let p = find(&found, &variant_id(n));
        assert!(p.unsupported.is_some(), "variant {n}");
    }
    // A plugin that only fails when it starts processing still scans clean.
    for n in [V_START, V_ERROR, V_NAN] {
        assert!(
            find(&found, &variant_id(n)).unsupported.is_none(),
            "variant {n}"
        );
    }
}

#[test]
fn the_classes_of_the_split_plugin_are_not_listed_twice() {
    let (_, found) = variants();
    assert_eq!(
        found.len(),
        9,
        "nine plugins, the controller class is not one"
    );
}

#[cfg(windows)]
#[test]
fn a_single_file_dot_vst3_dll_loads_like_a_bundle() {
    use atomik_plugin_host::testing::vst3_test_plugin_file;
    mark_main_thread();
    let file = vst3_test_plugin_file("atomik-test-vst3").unwrap();
    assert!(file.is_file());
    let b = Vst3Backend::new();
    let found = b.scan_file(&file).unwrap();
    assert_eq!(found.len(), 4);
}
