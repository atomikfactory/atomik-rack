#![allow(clippy::disallowed_methods)] // Justification: tests time real child processes with the wall clock.
//! End to end: the real scanner binary on the in-repo test plugins, through the real child
//! runner, the rescan driver, the cache and the catalog. Good files give exact layouts; bad,
//! hanging and crashing files and non-plugin files become blacklist entries; this process
//! survives all of them; a cache hit starts no child; descriptors register from the cache
//! without touching the plugin files.

use atomik_device_sdk::{PluginFormat, PluginRef, Registry};
use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::paths::Os;
use atomik_plugin_host::testing::clap_test_plugin;
use atomik_plugin_host::{
    ChildScanner, FailureKind, LoadStatus, PluginCache, PluginCatalog, PluginSettings,
    PluginStatus, RescanMode, ScanJob, ScanProgress, ScanRequest, ScanRunner, ScannerCommand,
    mark_main_thread, rescan,
};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    let d = std::env::temp_dir().join(format!("atomik-hs-scan-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn install(package: &str, dir: &Path, name: &str) -> PathBuf {
    let built = clap_test_plugin(package).unwrap();
    let target = dir.join(name);
    std::fs::copy(built, &target).unwrap();
    target
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
            formats: &[PluginFormat::Clap],
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
    let file = install("atomik-test-clap", &dir, "good.clap");
    let runner = ChildScanner::new(scanner_command());
    let result = runner
        .scan(
            PluginFormat::Clap,
            &file,
            Duration::from_secs(60),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(result.error, None);
    assert_eq!(runner.spawned(), 1);
    let ids: Vec<&str> = result
        .plugins
        .iter()
        .map(|p| p.plugin.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "atomik.test.gain",
            "atomik.test.synth",
            "atomik.test.noteshift",
            "atomik.test.params"
        ]
    );
    // The same file read in this process (the backend test suite pins the layouts themselves).
    let direct = atomik_plugin_host::PluginBackend::scan_file(&ClapBackend::new(), &file).unwrap();
    assert_eq!(result.plugins, direct);
    // the 2000-param plugin keeps all its rows, the params plugin is not truncated.
    let params = &result.plugins[3];
    assert!(params.layout.params.len() >= 2000 && !params.params_truncated);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_hanging_file_is_killed_at_the_timeout() {
    let dir = temp("hang");
    let file = install("atomik-test-clap-hang", &dir, "hang.clap");
    let runner = ChildScanner::new(scanner_command());
    let t = Instant::now();
    let err = runner
        .scan(
            PluginFormat::Clap,
            &file,
            Duration::from_secs(2),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert_eq!(err.kind, FailureKind::Timeout);
    assert!(
        err.reason.contains("killed") && err.reason.contains("2 s"),
        "{}",
        err.reason
    );
    // Killed at the timeout: not early, and not at some multiple of it (a mutation that waited
    // three times as long survived the old 20 s bound).
    let took = t.elapsed();
    assert!(took >= Duration::from_secs(2), "{took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_crashes_while_it_is_created_is_a_crash_not_a_dead_app() {
    let dir = temp("crash");
    let file = install("atomik-test-clap-crash", &dir, "crash.clap");
    let runner = ChildScanner::new(scanner_command());
    let err = runner
        .scan(
            PluginFormat::Clap,
            &file,
            Duration::from_secs(60),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert_eq!(err.kind, FailureKind::Crash);
    assert!(err.reason.contains("scanner"), "{}", err.reason);
    // Without the env var the same file scans fine (the crash plugin only crashes on request).
    let calm = ChildScanner::new(ScannerCommand::new(env!(
        "CARGO_BIN_EXE_atomik-plugin-scan"
    )));
    let ok = calm
        .scan(
            PluginFormat::Clap,
            &file,
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
    let bad = install("atomik-test-clap-bad", &dir, "bad.clap");
    let junk = dir.join("junk.clap");
    std::fs::write(&junk, b"this is not a library").unwrap();
    let runner = ChildScanner::new(scanner_command());
    for file in [bad, junk, dir.join("missing.clap")] {
        let r = runner
            .scan(
                PluginFormat::Clap,
                &file,
                Duration::from_secs(60),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(r.plugins.is_empty(), "{}", file.display());
        assert!(r.error.is_some(), "{}", file.display());
    }
    // A VST3 file that is not there: a clean error result, no crash.
    let v = runner
        .scan(
            PluginFormat::Vst3,
            &dir.join("x.vst3"),
            Duration::from_secs(60),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(v.error.is_some() && v.plugins.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_binary_rejects_bad_arguments_with_exit_code_2() {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_atomik-plugin-scan"))
        .arg("nonsense")
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}

#[test]
fn a_missing_scanner_program_is_a_spawn_failure_not_a_blacklist_entry() {
    let dir = temp("nospawn");
    std::fs::write(dir.join("a.clap"), b"x").unwrap();
    let runner = ChildScanner::new(ScannerCommand::new(dir.join("no-such-scanner")));
    let mut cache = PluginCache::default();
    let report = scan_dir(&dir, &mut cache, &runner, RescanMode::Incremental);
    assert!(report.spawn_error.is_some());
    assert_eq!(cache.blacklist().count(), 0);
    assert_eq!(runner.spawned(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_full_scan_survives_bad_files_and_the_cache_and_catalog_follow() {
    mark_main_thread();
    let dir = temp("full");
    let plugins = dir.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    install("atomik-test-clap", &plugins, "good.clap");
    install("atomik-test-clap-bad", &plugins, "bad.clap");
    install("atomik-test-clap-hang", &plugins, "hang.clap");
    install("atomik-test-clap-crash", &plugins, "crash.clap");
    std::fs::write(plugins.join("junk.clap"), b"not a plugin").unwrap();

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
    assert_eq!(failed, ["bad.clap", "crash.clap", "hang.clap", "junk.clap"]);
    for b in cache.blacklist() {
        assert!(!b.reason.is_empty(), "{}", b.path.display());
    }
    assert_eq!(cache.files().count(), 1);

    // A second pass is all cache hits and blacklist skips: no child starts.
    let again = scan_dir(&plugins, &mut cache, &runner, RescanMode::Incremental);
    assert_eq!(
        (again.scanned, again.cached, again.skipped_blacklisted),
        (0, 1, 4)
    );
    assert_eq!(runner.spawned(), 5);

    // A changed file is scanned again, alone.
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(plugins.join("junk.clap"))
        .unwrap();
    f.set_modified(
        std::fs::metadata(plugins.join("junk.clap"))
            .unwrap()
            .modified()
            .unwrap()
            + Duration::from_secs(9),
    )
    .unwrap();
    drop(f);
    scan_dir(&plugins, &mut cache, &runner, RescanMode::Incremental);
    assert_eq!(runner.spawned(), 6);

    // The cache survives a round trip through disk.
    let cache_file = dir.join("plugins.json");
    cache.save(&cache_file).unwrap();
    let (loaded, status) = PluginCache::load(&cache_file);
    assert_eq!(status, LoadStatus::Loaded);
    assert_eq!(loaded, cache);

    // Descriptors register from the cache without touching the plugin files: delete them first.
    std::fs::remove_dir_all(&plugins).unwrap();
    let mut settings = PluginSettings::default();
    settings.enable(true).unwrap();
    let mut catalog = PluginCatalog::from_cache(&loaded, &settings);
    assert_eq!(catalog.entries().len(), 4);
    assert!(
        catalog
            .entries()
            .iter()
            .all(|e| e.status == PluginStatus::Ok)
    );
    assert_eq!(catalog.failed().len(), 4);
    let backend = ClapBackend::new();
    let mut registry = Registry::new();
    let r = catalog.register_into(&mut registry, &mut |p: &PluginRef, path| {
        backend.register_plugin_file(&p.id, path);
    });
    assert_eq!((r.registered, r.errors.len()), (4, 0));
    assert!(registry.is_plugin("clap.atomik.test.gain"));
    assert!(registry.is_plugin("clap.atomik.test.synth"));
    // The 2000-param plugin's descriptor has bypass plus every param.
    let d = registry.descriptor("clap.atomik.test.params").unwrap();
    assert!(d.params.len() >= 2000);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancelling_kills_the_running_child_and_blacklists_nothing() {
    let dir = temp("cancel");
    let plugins = dir.join("p");
    std::fs::create_dir_all(&plugins).unwrap();
    install("atomik-test-clap-hang", &plugins, "hang.clap");
    let mut settings = PluginSettings::default();
    settings.enable(true).unwrap();
    settings.extra_paths = vec![plugins.clone()];
    settings.set_timeout_secs(300);
    let runner = Arc::new(ChildScanner::new(scanner_command()));
    let job = ScanJob::start(
        settings,
        vec![PluginFormat::Clap],
        Os::Windows,
        RescanMode::Incremental,
        PluginCache::default(),
        runner.clone(),
    );
    let t = Instant::now();
    while runner.spawned() == 0 && t.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(runner.spawned(), 1);
    assert!(!job.is_finished());
    job.cancel();
    let (cache, report) = job.finish();
    let report = report.unwrap();
    assert!(report.cancelled);
    assert!(
        t.elapsed() < Duration::from_secs(60),
        "the 300 s timeout did not run out"
    );
    assert_eq!(cache.blacklist().count(), 0);
    assert_eq!(cache.files().count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_scanners_run_in_parallel_on_good_files() {
    let dir = temp("par");
    let plugins = dir.join("p");
    std::fs::create_dir_all(&plugins).unwrap();
    for i in 0..4 {
        install("atomik-test-clap", &plugins, &format!("good{i}.clap"));
    }
    let runner = ChildScanner::new(scanner_command());
    let mut cache = PluginCache::default();
    let report = scan_dir(&plugins, &mut cache, &runner, RescanMode::Incremental);
    assert_eq!((report.ok, report.failed), (4, 0));
    assert_eq!(runner.spawned(), 4);
    let _ = std::fs::remove_dir_all(&dir);
}
