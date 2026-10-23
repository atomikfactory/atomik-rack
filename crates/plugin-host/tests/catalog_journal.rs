//! The catalog built from the cache (nothing loaded), its
//! registration in the `Registry`, and the crash journal with the recovery snapshot helpers.

use atomik_device_sdk::{
    Channels, DeviceKind, PluginFormat, PluginKindHint, PluginLayout, PluginParamInfo, PluginRef,
    Registry,
};
use atomik_plugin_host::{
    CachedFile, CrashJournal, FileStamp, InUsePlugin, PluginCache, PluginCatalog, PluginSettings,
    PluginStatus, RecoveryDir, SCANNER_VERSION, ScannedPlugin,
};
use std::path::{Path, PathBuf};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-hs-cat-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn layout(name: &str, kind: PluginKindHint) -> PluginLayout {
    let instrument = kind == PluginKindHint::Instrument;
    let note_fx = kind == PluginKindHint::NoteEffect;
    PluginLayout {
        name: name.to_string(),
        vendor: "Acme".to_string(),
        version: "1.0".to_string(),
        kind_hint: kind,
        main_in: (!instrument && !note_fx).then_some(Channels::Stereo),
        main_out: (!note_fx).then_some(Channels::Stereo),
        sidechain: kind == PluginKindHint::Effect,
        notes_in: instrument || note_fx,
        notes_out: note_fx,
        params: vec![PluginParamInfo {
            id: 1,
            name: "Gain".to_string(),
            module: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            stepped: false,
            steps: 0,
        }],
    }
}

fn plugin(id: &str, name: &str, kind: PluginKindHint) -> ScannedPlugin {
    ScannedPlugin {
        plugin: PluginRef {
            format: PluginFormat::Clap,
            id: id.to_string(),
        },
        layout: layout(name, kind),
        unsupported: None,
        params_truncated: false,
    }
}

fn stamp() -> FileStamp {
    FileStamp {
        size: 10,
        mtime_ns: 20,
    }
}

fn add_file(cache: &mut PluginCache, path: &str, plugins: Vec<ScannedPlugin>) {
    cache.insert_file(CachedFile {
        path: PathBuf::from(path),
        format: PluginFormat::Clap,
        stamp: stamp(),
        scanner_version: SCANNER_VERSION,
        plugins,
    });
}

fn enabled() -> PluginSettings {
    let mut s = PluginSettings::default();
    s.enable();
    s
}

fn sample_cache() -> PluginCache {
    let mut c = PluginCache::default();
    add_file(
        &mut c,
        "/nowhere/b.clap",
        vec![
            plugin("b.verb", "Verb", PluginKindHint::Effect),
            ScannedPlugin {
                unsupported: Some("surround only".to_string()),
                ..plugin("b.surround", "Surround", PluginKindHint::Effect)
            },
        ],
    );
    add_file(
        &mut c,
        "/nowhere/a.clap",
        vec![
            plugin("a.synth", "Synth", PluginKindHint::Instrument),
            plugin("a.arp", "Arp", PluginKindHint::NoteEffect),
        ],
    );
    c.blacklist_file(
        Path::new("/nowhere/bad.clap"),
        PluginFormat::Clap,
        stamp(),
        "crashed",
    );
    c
}

#[test]
fn nothing_is_listed_while_plugins_are_off() {
    let cat = PluginCatalog::from_cache(&sample_cache(), &PluginSettings::default());
    assert!(cat.entries().is_empty());
    assert!(cat.failed().is_empty());
}

#[test]
fn entries_are_sorted_with_status_kind_and_features() {
    let mut s = enabled();
    s.disabled.insert("clap.a.arp".to_string());
    let cat = PluginCatalog::from_cache(&sample_cache(), &s);
    let rows: Vec<(&str, &PluginStatus)> = cat
        .entries()
        .iter()
        .map(|e| (e.name.as_str(), &e.status))
        .collect();
    assert_eq!(
        rows,
        [
            ("Arp", &PluginStatus::Disabled),
            (
                "Surround",
                &PluginStatus::Unsupported("surround only".to_string())
            ),
            ("Synth", &PluginStatus::Ok),
            ("Verb", &PluginStatus::Ok),
        ]
    );
    let synth = cat
        .entry(&PluginRef {
            format: PluginFormat::Clap,
            id: "a.synth".to_string(),
        })
        .unwrap();
    assert_eq!(synth.kind, PluginKindHint::Instrument);
    assert_eq!(synth.vendor, "Acme");
    assert!(synth.features.contains(&"instrument".to_string()));
    assert!(synth.features.contains(&"notes-in".to_string()));
    assert_eq!(synth.path, PathBuf::from("/nowhere/a.clap"));
    let verb = &cat.entries()[3];
    assert!(verb.features.contains(&"audio-effect".to_string()));
    assert!(verb.features.contains(&"sidechain".to_string()));
    assert_eq!(cat.failed().len(), 1);
    assert_eq!(cat.failed()[0].reason, "crashed");
}

#[test]
fn the_same_plugin_in_two_files_is_listed_once() {
    let mut c = PluginCache::default();
    add_file(
        &mut c,
        "/x/a.clap",
        vec![plugin("dup", "Dup", PluginKindHint::Effect)],
    );
    add_file(
        &mut c,
        "/y/b.clap",
        vec![plugin("dup", "Dup", PluginKindHint::Effect)],
    );
    let cat = PluginCatalog::from_cache(&c, &enabled());
    assert_eq!(cat.entries().len(), 2);
    assert_eq!(cat.entries()[0].status, PluginStatus::Ok);
    assert!(matches!(&cat.entries()[1].status, PluginStatus::Failed(r) if r.contains("duplicate")));
}

#[test]
fn registration_uses_only_cache_data_and_never_touches_the_files() {
    // The paths do not exist: registering must not open, stat or load anything.
    let cache = sample_cache();
    let mut cat = PluginCatalog::from_cache(&cache, &enabled());
    let mut reg = Registry::new();
    let mut fed: Vec<(String, PathBuf)> = Vec::new();
    let report = cat.register_into(&mut reg, &mut |p, path| {
        fed.push((p.id.clone(), path.to_path_buf()));
    });
    assert_eq!(report.registered, 3);
    assert!(report.errors.is_empty());
    assert!(
        reg.is_plugin("clap.a.synth")
            && reg.is_plugin("clap.a.arp")
            && reg.is_plugin("clap.b.verb")
    );
    assert!(
        !reg.is_plugin("clap.b.surround"),
        "unsupported plugins cannot be added"
    );
    assert_eq!(
        reg.descriptor("clap.a.synth").unwrap().kind,
        DeviceKind::Instrument
    );
    assert_eq!(
        reg.descriptor("clap.b.verb").unwrap().kind,
        DeviceKind::Effect
    );
    assert_eq!(
        reg.descriptor("clap.a.arp").unwrap().kind,
        DeviceKind::Utility
    );
    fed.sort();
    assert_eq!(
        fed,
        [
            ("a.arp".to_string(), PathBuf::from("/nowhere/a.clap")),
            ("a.synth".to_string(), PathBuf::from("/nowhere/a.clap")),
            ("b.verb".to_string(), PathBuf::from("/nowhere/b.clap")),
        ]
    );
}

#[test]
fn disabled_plugins_are_not_registered_and_a_rescan_swaps_the_set() {
    let cache = sample_cache();
    let mut s = enabled();
    s.disabled.insert("clap.b.verb".to_string());
    let mut cat = PluginCatalog::from_cache(&cache, &s);
    let mut reg = Registry::new();
    cat.register_into(&mut reg, &mut |_, _| {});
    assert!(!reg.is_plugin("clap.b.verb"));
    // The user re-enables it: unregister the old set, register the new one.
    cat.unregister_from(&mut reg);
    assert_eq!(reg.plugin_type_ids().count(), 0);
    let mut cat2 = PluginCatalog::from_cache(&cache, &enabled());
    let r = cat2.register_into(&mut reg, &mut |_, _| {});
    assert_eq!(r.registered, 3);
    assert!(reg.is_plugin("clap.b.verb"));
    // Registering again over a live set reports duplicates instead of panicking.
    let mut cat3 = PluginCatalog::from_cache(&cache, &enabled());
    let r = cat3.register_into(&mut reg, &mut |_, _| {});
    assert_eq!(r.registered, 0);
    assert_eq!(r.errors.len(), 3);
}

#[test]
fn a_plugin_that_cannot_make_a_descriptor_is_marked_failed_and_the_rest_still_register() {
    let mut c = PluginCache::default();
    let mut bad = plugin("too.wide", "Wide", PluginKindHint::Effect);
    let row = bad.layout.params[0].clone();
    bad.layout.params = (0..3000)
        .map(|i| PluginParamInfo {
            id: i,
            ..row.clone()
        })
        .collect();
    let mut nosock = plugin("no.sockets", "None", PluginKindHint::Effect);
    nosock.layout.main_in = None;
    nosock.layout.main_out = None;
    nosock.layout.sidechain = false;
    add_file(
        &mut c,
        "/x/w.clap",
        vec![bad, nosock, plugin("fine", "Fine", PluginKindHint::Effect)],
    );
    let mut cat = PluginCatalog::from_cache(&c, &enabled());
    let mut reg = Registry::new();
    let report = cat.register_into(&mut reg, &mut |_, _| {});
    assert_eq!(report.registered, 1);
    assert_eq!(report.errors.len(), 2);
    assert!(reg.is_plugin("clap.fine"));
    let failed = cat
        .entries()
        .iter()
        .filter(|e| matches!(e.status, PluginStatus::Failed(_)))
        .count();
    assert_eq!(failed, 2);
}

// ----- crash journal and recovery -------------------------------------------------------------

fn in_use(id: &str, path: &str) -> InUsePlugin {
    InUsePlugin {
        plugin: PluginRef {
            format: PluginFormat::Clap,
            id: id.to_string(),
        },
        path: PathBuf::from(path),
    }
}

#[test]
fn the_journal_exists_only_while_plugins_are_loaded_and_never_after_a_clean_exit() {
    let d = temp("journal");
    let file = d.join("plugins-in-use.json");
    let mut j = CrashJournal::new(&file);
    assert!(!file.exists());
    j.loaded(in_use("a", "/p/a.clap")).unwrap();
    assert!(file.exists());
    j.loaded(in_use("b", "/p/b.clap")).unwrap();
    j.loaded(in_use("a", "/p/a.clap")).unwrap(); // a second instance of the same plugin
    j.unloaded(&in_use("a", "/p/a.clap")).unwrap();
    assert!(file.exists());
    j.unloaded(&in_use("b", "/p/b.clap")).unwrap();
    assert!(file.exists(), "one instance of a is still loaded");
    j.unloaded(&in_use("a", "/p/a.clap")).unwrap();
    assert!(!file.exists());
    j.loaded(in_use("c", "/p/c.clap")).unwrap();
    j.clean_exit().unwrap();
    assert!(!file.exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_unclean_exit_is_detected_once_and_can_blacklist_the_plugins() {
    let d = temp("unclean");
    let file = d.join("plugins-in-use.json");
    assert_eq!(CrashJournal::take_previous(&file), None);
    {
        let mut j = CrashJournal::new(&file);
        j.loaded(in_use("a", "/p/a.clap")).unwrap();
        j.loaded(in_use("b", "/p/b.clap")).unwrap();
        // The process "crashes": the journal is simply never cleaned up.
    }
    let prev = CrashJournal::take_previous(&file).expect("an unclean exit");
    assert_eq!(prev, [in_use("a", "/p/a.clap"), in_use("b", "/p/b.clap")]);
    assert_eq!(CrashJournal::take_previous(&file), None, "reported once");
    let mut cache = PluginCache::default();
    atomik_plugin_host::blacklist_in_use(&mut cache, &prev);
    let reasons: Vec<_> = cache.blacklist().map(|b| b.reason.clone()).collect();
    assert_eq!(reasons.len(), 2);
    assert!(reasons[0].contains("unexpectedly"));
    // Garbage in the journal is not an unclean exit report, and does not fail.
    std::fs::write(&file, b"{ not json").unwrap();
    assert_eq!(CrashJournal::take_previous(&file), None);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn recovery_snapshots_rotate_to_three_and_the_newest_is_found() {
    let d = temp("recover");
    let dir = RecoveryDir::new(d.join("recovery"));
    assert_eq!(dir.newest(), None);
    let mut paths = Vec::new();
    for n in 1..=5u64 {
        let p = dir.next_path("My Song", n);
        assert!(p.to_string_lossy().ends_with(".recover.atomik"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, format!("snapshot {n}")).unwrap();
        dir.prune(3).unwrap();
        paths.push(p);
    }
    assert!(!paths[0].exists() && !paths[1].exists());
    assert!(paths[2].exists() && paths[3].exists() && paths[4].exists());
    assert_eq!(dir.newest(), Some(paths[4].clone()));
    // Unrelated files are left alone.
    std::fs::write(d.join("recovery/notes.txt"), b"x").unwrap();
    dir.prune(1).unwrap();
    assert!(d.join("recovery/notes.txt").exists());
    assert_eq!(dir.newest(), Some(paths[4].clone()));
    let _ = std::fs::remove_dir_all(&d);
}
