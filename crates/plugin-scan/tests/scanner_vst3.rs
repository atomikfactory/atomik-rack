#![allow(clippy::disallowed_methods)] // Justification: tests time real child processes with the wall clock.
//! The real scanner binary on the in-repo VST3 test plugins, through the
//! real child runner, the rescan driver, the cache and the catalog. Good bundles give exact
//! layouts; bad, hanging and crashing modules and non-plugin files become blacklist entries; this
//! process survives all of them; descriptors register as `vst3.<hex>` from the cache without
//! touching the bundles.

use atomik_device_sdk::{PluginFormat, PluginRef, Registry};
use atomik_plugin_host::paths::Os;
use atomik_plugin_host::testing::{build_cdylib, vst3_bundle_binary};
use atomik_plugin_host::{
    ChildScanner, FailureKind, PluginCache, PluginCatalog, PluginSettings, PluginStatus,
    RescanMode, ScanProgress, ScanRequest, ScanRunner, ScannerCommand, mark_main_thread, rescan,
};
use atomik_plugin_vst3::Vst3Backend;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

fn scanner_command() -> ScannerCommand {
    let mut c = ScannerCommand::new(env!("CARGO_BIN_EXE_atomik-plugin-scan"));
    c.envs.push((
        OsString::from("ATOMIK_TEST_CRASH_ON_CREATE"),
        OsString::from("1"),
    ));
    c
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-hv-scan-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Installs the built library of `package` as the bundle `<stem>.vst3` in `dir`, with the
/// layout of this operating system.
fn install(package: &str, dir: &Path, stem: &str) -> PathBuf {
    let built = build_cdylib(package).unwrap();
    let bundle = dir.join(format!("{stem}.vst3"));
    let binary = bundle
        .join(vst3_bundle_binary(Os::current(), std::env::consts::ARCH, stem).expect("a layout"));
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::copy(built, &binary).unwrap();
    if cfg!(target_os = "macos") {
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            format!("<key>CFBundleExecutable</key><string>{stem}</string>"),
        )
        .unwrap();
    }
    bundle
}

fn no_env(_: &str) -> Option<OsString> {
    None
}

fn scan_dir(
    dir: &Path,
    cache: &mut PluginCache,
    runner: &dyn ScanRunner,
    mode: RescanMode,
) -> atomik_plugin_host::ScanReport {
    let mut settings = PluginSettings::default();
    settings.enable(true).unwrap();
    settings.extra_paths = vec![dir.to_path_buf()];
    settings.set_timeout_secs(5);
    rescan(
        &ScanRequest {
            settings: &settings,
            formats: &[PluginFormat::Vst3],
            os: Os::Windows,
            env: &no_env,
            mode,
        },
        cache,
        runner,
        &AtomicBool::new(false),
        &ScanProgress::default(),
    )
    .unwrap()
}

#[test]
fn the_scanner_returns_the_exact_layouts_the_backend_reads_in_process() {
    mark_main_thread();
    let dir = temp("exact");
    let bundle = install("atomik-test-vst3", &dir, "good");
    let runner = ChildScanner::new(scanner_command());
    let result = runner
        .scan(
            PluginFormat::Vst3,
            &bundle,
            Duration::from_secs(60),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(result.error, None);
    assert_eq!(runner.spawned(), 1);
    assert_eq!(result.format, PluginFormat::Vst3);
    let ids: Vec<&str> = result
        .plugins
        .iter()
        .map(|p| p.plugin.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "a70f1c014a5b4c018d2e1f0000000001",
            "a70f1c014a5b4c018d2e1f0000000002",
            "a70f1c014a5b4c018d2e1f0000000003",
            "a70f1c014a5b4c018d2e1f0000000004"
        ]
    );
    // The same bundle read in this process (the backend test suite pins the layouts themselves).
    let direct =
        atomik_plugin_host::PluginBackend::scan_file(&Vst3Backend::new(), &bundle).unwrap();
    assert_eq!(result.plugins, direct);
    let params = &result.plugins[3];
    assert!(params.layout.params.len() >= 2000 && !params.params_truncated);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_hanging_module_is_killed_at_the_timeout() {
    let dir = temp("hang");
    let bundle = install("atomik-test-vst3-hang", &dir, "hang");
    let runner = ChildScanner::new(scanner_command());
    let t = Instant::now();
    let err = runner
        .scan(
            PluginFormat::Vst3,
            &bundle,
            Duration::from_secs(2),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert_eq!(err.kind, FailureKind::Timeout);
    let took = t.elapsed();
    assert!(took >= Duration::from_secs(2), "{took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_module_that_crashes_while_it_is_created_is_a_crash_not_a_dead_app() {
    let dir = temp("crash");
    let bundle = install("atomik-test-vst3-crash", &dir, "crash");
    let runner = ChildScanner::new(scanner_command());
    let err = runner
        .scan(
            PluginFormat::Vst3,
            &bundle,
            Duration::from_secs(60),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert_eq!(err.kind, FailureKind::Crash);
    // Without the env var the same bundle scans fine.
    let calm = ChildScanner::new(ScannerCommand::new(env!(
        "CARGO_BIN_EXE_atomik-plugin-scan"
    )));
    let ok = calm
        .scan(
            PluginFormat::Vst3,
            &bundle,
            Duration::from_secs(60),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(ok.plugins.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_missing_and_non_plugin_files_give_an_error_result() {
    let dir = temp("errors");
    let bad = install("atomik-test-vst3-bad", &dir, "bad");
    let junk = dir.join("junk.vst3");
    std::fs::write(&junk, b"this is not a library").unwrap();
    let runner = ChildScanner::new(scanner_command());
    for file in [bad, junk, dir.join("missing.vst3")] {
        let r = runner
            .scan(
                PluginFormat::Vst3,
                &file,
                Duration::from_secs(60),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(r.plugins.is_empty(), "{}", file.display());
        assert!(r.error.is_some(), "{}", file.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_full_scan_of_a_folder_with_bundles_survives_bad_ones_and_the_catalog_follows() {
    mark_main_thread();
    let dir = temp("full");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    install("atomik-test-vst3", &plugins, "good");
    install("atomik-test-vst3-bad", &plugins, "bad");
    install("atomik-test-vst3-hang", &plugins, "hang");
    install("atomik-test-vst3-crash", &plugins, "crash");
    std::fs::write(plugins.join("junk.vst3"), b"not a plugin").unwrap();

    let runner = ChildScanner::new(scanner_command());
    let mut cache = PluginCache::default();
    let t = Instant::now();
    let report = scan_dir(&plugins, &mut cache, &runner, RescanMode::Incremental);
    assert_eq!((report.scanned, report.ok, report.failed), (5, 1, 4));
    assert!(t.elapsed() < Duration::from_secs(60));
    assert_eq!(runner.spawned(), 5);
    let failed: Vec<String> = cache
        .blacklist()
        .map(|b| b.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(failed, ["bad.vst3", "crash.vst3", "hang.vst3", "junk.vst3"]);

    // A second pass is all cache hits and blacklist skips: no child starts.
    let again = scan_dir(&plugins, &mut cache, &runner, RescanMode::Incremental);
    assert_eq!(
        (again.scanned, again.cached, again.skipped_blacklisted),
        (0, 1, 4)
    );
    assert_eq!(runner.spawned(), 5);

    // Replacing the binary inside a bundle makes the cached scan stale (the bundle folder is
    // stamped by what is inside it).
    let binary = plugins
        .join("good.vst3")
        .join(vst3_bundle_binary(Os::current(), std::env::consts::ARCH, "good").unwrap());
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(&binary)
        .unwrap();
    f.set_modified(
        std::fs::metadata(&binary).unwrap().modified().unwrap() + Duration::from_secs(9),
    )
    .unwrap();
    drop(f);
    let changed = scan_dir(&plugins, &mut cache, &runner, RescanMode::Incremental);
    assert_eq!(
        (changed.scanned, changed.cached),
        (1, 0),
        "the bundle was scanned again"
    );
    assert_eq!(runner.spawned(), 6);

    // Descriptors register from the cache without touching the bundles: delete them first.
    std::fs::remove_dir_all(&plugins).unwrap();
    let mut settings = PluginSettings::default();
    settings.enable(true).unwrap();
    let mut catalog = PluginCatalog::from_cache(&cache, &settings);
    assert_eq!(catalog.entries().len(), 4);
    assert!(
        catalog
            .entries()
            .iter()
            .all(|e| e.status == PluginStatus::Ok)
    );
    assert_eq!(catalog.failed().len(), 4);
    let backend = Vst3Backend::new();
    let mut registry = Registry::new();
    let r = catalog.register_into(&mut registry, &mut |p: &PluginRef, path| {
        backend.register_plugin_file(&p.id, path);
    });
    assert_eq!((r.registered, r.errors.len()), (4, 0));
    assert!(registry.is_plugin("vst3.a70f1c014a5b4c018d2e1f0000000001"));
    let d = registry
        .descriptor("vst3.a70f1c014a5b4c018d2e1f0000000004")
        .unwrap();
    assert!(d.params.len() >= 2000);
    // The gain effect has a sidechain: its descriptor has the sc sockets, and a stepped VST3
    // param is a discrete 0 to steps param.
    let gain = registry
        .descriptor("vst3.a70f1c014a5b4c018d2e1f0000000001")
        .unwrap();
    assert!(gain.socket_by_key("sc_in_l").is_some());
    let latency = gain.param_by_key("p3").unwrap();
    assert!(latency.discrete);
    assert_eq!((latency.min, latency.max), (0.0, 512.0));
    let _ = std::fs::remove_dir_all(&dir);
}
