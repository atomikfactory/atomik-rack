#![allow(clippy::disallowed_methods)] // Justification: tests time real child processes with the wall clock.
//! End to end: what the scanner leaves behind and how it copes with one bad class. A file of
//! four classes (`atomik-test-clap-mix`) is scanned through the real scanner program: a class
//! that hangs or crashes costs only that class, a helper process a plugin starts never outlives
//! the scan (finished, timed out, crashed or cancelled), the helper runs below normal priority,
//! and the time limit is per class. Processes are named by id; the tests only look at the helper
//! the test plugin itself started and never end any process by hand.

use atomik_device_sdk::PluginFormat;
use atomik_plugin_clap::ClapBackend;
use atomik_plugin_host::paths::Os;
use atomik_plugin_host::testing::clap_test_plugin;
use atomik_plugin_host::{
    ChildScanner, FailureKind, NO_TIME_LIMIT, PluginCache, PluginSettings, RescanMode,
    ScanProgress, ScanReport, ScanRequest, ScanResult, ScanRunner, ScannerCommand,
    is_timeout_reason, rescan,
};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-hs-robust-{tag}-{}", std::process::id()));
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

/// A runner whose scanner child sees `envs` (the `clap-mix` and `clap-crash` libraries read them).
fn runner(envs: &[(&str, &str)]) -> ChildScanner {
    let mut c = ScannerCommand::new(env!("CARGO_BIN_EXE_atomik-plugin-scan"));
    for (k, v) in envs {
        c.envs.push((OsString::from(k), OsString::from(v)));
    }
    ChildScanner::new(c)
}

fn scan(
    r: &ChildScanner,
    file: &Path,
    secs: u64,
) -> Result<ScanResult, atomik_plugin_host::ScanFailure> {
    r.scan(
        PluginFormat::Clap,
        file,
        Duration::from_secs(secs),
        &AtomicBool::new(false),
    )
}

/// Waits (generously) until the file holds a process id and returns it.
fn wait_for_pid(file: &Path) -> u32 {
    let t = Instant::now();
    loop {
        if let Some(pid) = std::fs::read_to_string(file)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            return pid;
        }
        assert!(t.elapsed() < Duration::from_secs(60), "no helper started");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether a process with this id is still running (a zombie is not).
fn process_alive(pid: u32) -> bool {
    if cfg!(windows) {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
    } else {
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&out.stdout);
        let stat = stat.trim();
        !stat.is_empty() && !stat.starts_with('Z')
    }
}

/// Waits (generously) until the process is gone.
fn assert_process_ends(pid: u32) {
    let t = Instant::now();
    while process_alive(pid) {
        assert!(
            t.elapsed() < Duration::from_secs(60),
            "the helper process {pid} outlived the scan"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// True when the (running) process has a priority below normal.
fn priority_is_lowered(pid: u32) -> bool {
    if cfg!(windows) {
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {pid}).PriorityClass"),
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim() == "BelowNormal"
    } else {
        let out = std::process::Command::new("ps")
            .args(["-o", "ni=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<i32>()
            .is_ok_and(|n| n >= 5)
    }
}

fn ids(r: &ScanResult) -> Vec<&str> {
    r.plugins.iter().map(|p| p.plugin.id.as_str()).collect()
}

#[test]
fn a_healthy_multi_class_file_gives_every_class_in_one_child() {
    let dir = temp("ok");
    let file = install("atomik-test-clap-mix", &dir, "mix.clap");
    let runner = runner(&[]);
    let r = scan(&runner, &file, 60).unwrap();
    assert_eq!(
        ids(&r),
        [
            "atomik.test.mix.a",
            "atomik.test.mix.helper",
            "atomik.test.mix.stuck",
            "atomik.test.mix.b"
        ]
    );
    assert!(r.plugins.iter().all(|p| p.unsupported.is_none()));
    assert_eq!(r.error, None);
    assert_eq!(runner.spawned(), 1);
    // The same file read in this process gives the same classes.
    atomik_plugin_host::mark_main_thread();
    let direct = atomik_plugin_host::PluginBackend::scan_file(&ClapBackend::new(), &file).unwrap();
    assert_eq!(r.plugins, direct);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_class_that_hangs_costs_only_that_class() {
    let dir = temp("hang");
    let file = install("atomik-test-clap-mix", &dir, "mix.clap");
    let runner = runner(&[("ATOMIK_TEST_MIX_STUCK", "1")]);
    let r = scan(&runner, &file, 3).unwrap();
    // All four classes are in the result, in the file's order; only the hung one is unsupported.
    assert_eq!(r.plugins.len(), 4);
    for p in &r.plugins {
        if p.plugin.id == "atomik.test.mix.stuck" {
            let why = p.unsupported.as_deref().unwrap_or_default();
            assert!(why.contains("did not answer within 3 s"), "{why}");
        } else {
            assert_eq!(p.unsupported, None, "{}", p.plugin.id);
        }
    }
    // One child to the hung class, one more for what came after it.
    assert_eq!(runner.spawned(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_class_that_crashes_costs_only_that_class_and_is_found_at_once() {
    let dir = temp("crash");
    let file = install("atomik-test-clap-mix", &dir, "mix.clap");
    let runner = runner(&[("ATOMIK_TEST_MIX_CRASH", "1")]);
    let t = Instant::now();
    // A long timeout: a crash must not be mistaken for a hang that runs it out.
    let r = scan(&runner, &file, 120).unwrap();
    assert!(t.elapsed() < Duration::from_secs(60), "{:?}", t.elapsed());
    let bad: Vec<&str> = r
        .plugins
        .iter()
        .filter(|p| p.unsupported.is_some())
        .map(|p| p.plugin.id.as_str())
        .collect();
    assert_eq!(bad, ["atomik.test.mix.stuck"]);
    let why = r.plugins[2].unsupported.as_deref().unwrap_or_default();
    assert!(why.contains("scanner"), "{why}");
    assert_eq!(runner.spawned(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_crashes_at_load_is_a_crash_within_a_short_bound() {
    let dir = temp("crash-bound");
    let file = install("atomik-test-clap-crash", &dir, "crash.clap");
    // This test does not turn the system error dialogs off for itself: the scanner does.
    let runner = runner(&[("ATOMIK_TEST_CRASH_ON_CREATE", "1")]);
    let t = Instant::now();
    let err = scan(&runner, &file, 120).unwrap_err();
    assert_eq!(err.kind, FailureKind::Crash, "{}", err.reason);
    assert!(t.elapsed() < Duration::from_secs(60), "{:?}", t.elapsed());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_time_limit_is_per_class_so_a_slow_but_healthy_file_is_not_judged_as_a_whole() {
    let dir = temp("slow");
    let file = install("atomik-test-clap-mix", &dir, "mix.clap");
    // Four classes of two seconds each: eight seconds in all, against a limit of five. The limit
    // is per class, so every class answers.
    let runner = runner(&[("ATOMIK_TEST_MIX_SLOW_MS", "2000")]);
    let t = Instant::now();
    let r = scan(&runner, &file, 5).unwrap();
    assert!(t.elapsed() > Duration::from_secs(5), "{:?}", t.elapsed());
    assert_eq!(r.plugins.len(), 4);
    assert!(r.plugins.iter().all(|p| p.unsupported.is_none()));
    assert_eq!(runner.spawned(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn with_no_time_limit_a_slow_file_finishes() {
    let dir = temp("nolimit");
    let file = install("atomik-test-clap-mix", &dir, "mix.clap");
    let runner = runner(&[("ATOMIK_TEST_MIX_SLOW_MS", "300")]);
    let r = runner
        .scan(
            PluginFormat::Clap,
            &file,
            NO_TIME_LIMIT,
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(r.plugins.len(), 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_helper_process_does_not_outlive_a_scan_that_finished() {
    let dir = temp("helper-ok");
    let file = install("atomik-test-clap-mix", &dir, "mix.clap");
    let pid_file = dir.join("helper.pid");
    let runner = runner(&[(
        "ATOMIK_TEST_MIX_HELPER_PID_FILE",
        pid_file.to_str().unwrap(),
    )]);
    let r = scan(&runner, &file, 60).unwrap();
    assert_eq!(r.plugins.len(), 4);
    assert_process_ends(wait_for_pid(&pid_file));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_helper_process_does_not_outlive_a_scan_that_timed_out() {
    let dir = temp("helper-timeout");
    let file = install("atomik-test-clap-mix", &dir, "mix.clap");
    let pid_file = dir.join("helper.pid");
    let runner = runner(&[
        (
            "ATOMIK_TEST_MIX_HELPER_PID_FILE",
            pid_file.to_str().unwrap(),
        ),
        ("ATOMIK_TEST_MIX_STUCK", "1"),
    ]);
    let r = scan(&runner, &file, 3).unwrap();
    assert!(r.plugins[2].unsupported.is_some());
    assert_process_ends(wait_for_pid(&pid_file));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_helper_process_does_not_outlive_a_scan_that_crashed() {
    let dir = temp("helper-crash");
    let file = install("atomik-test-clap-mix", &dir, "mix.clap");
    let pid_file = dir.join("helper.pid");
    let runner = runner(&[
        (
            "ATOMIK_TEST_MIX_HELPER_PID_FILE",
            pid_file.to_str().unwrap(),
        ),
        ("ATOMIK_TEST_MIX_CRASH", "1"),
    ]);
    let r = scan(&runner, &file, 120).unwrap();
    assert!(r.plugins[2].unsupported.is_some());
    assert_process_ends(wait_for_pid(&pid_file));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancelled_scan_leaves_no_helper_and_the_helper_runs_below_normal_priority() {
    let dir = temp("helper-cancel");
    let file = install("atomik-test-clap-mix", &dir, "mix.clap");
    let pid_file = dir.join("helper.pid");
    let runner = Arc::new(runner(&[
        (
            "ATOMIK_TEST_MIX_HELPER_PID_FILE",
            pid_file.to_str().unwrap(),
        ),
        ("ATOMIK_TEST_MIX_STUCK", "1"),
    ]));
    let cancel = Arc::new(AtomicBool::new(false));
    let worker = {
        let (runner, cancel, file) = (runner.clone(), cancel.clone(), file.clone());
        std::thread::spawn(move || runner.scan(PluginFormat::Clap, &file, NO_TIME_LIMIT, &cancel))
    };
    let pid = wait_for_pid(&pid_file);
    // The helper inherited the scanner's lowered priority. The scanner is stuck in the next
    // class with no time limit, so the helper is alive for as long as this takes.
    assert!(
        priority_is_lowered(pid),
        "the helper runs at normal priority"
    );
    cancel.store(true, Ordering::Relaxed);
    let err = worker.join().unwrap().unwrap_err();
    assert_eq!(err.kind, FailureKind::Cancelled);
    assert_process_ends(pid);
    let _ = std::fs::remove_dir_all(&dir);
}

fn scan_dir(dir: &Path, cache: &mut PluginCache, runner: &dyn ScanRunner, secs: u32) -> ScanReport {
    let mut settings = PluginSettings::default();
    settings.enable();
    settings.extra_paths = vec![dir.to_path_buf()];
    settings.set_timeout_secs(secs);
    rescan(
        &ScanRequest {
            settings: &settings,
            formats: &[PluginFormat::Clap],
            os: Os::Windows,
            env: &no_env,
            mode: RescanMode::Incremental,
        },
        cache,
        runner,
        &AtomicBool::new(false),
        &ScanProgress::default(),
    )
    .unwrap()
}

#[test]
fn a_partly_answered_file_is_cached_and_a_wholly_hung_one_is_blacklisted_as_a_timeout() {
    let dir = temp("driver");
    let plugins = dir.join("p");
    std::fs::create_dir_all(&plugins).unwrap();
    install("atomik-test-clap-mix", &plugins, "mix.clap");
    install("atomik-test-clap-hang", &plugins, "hang.clap");
    let runner = runner(&[("ATOMIK_TEST_MIX_STUCK", "1")]);
    let mut cache = PluginCache::default();
    let report = scan_dir(&plugins, &mut cache, &runner, 5);
    assert_eq!((report.ok, report.failed), (1, 1));
    // The classes that answered are in the cache; the file is not blacklisted.
    let mix = cache.file(&plugins.join("mix.clap")).unwrap();
    assert_eq!(mix.plugins.len(), 4);
    assert_eq!(
        mix.plugins
            .iter()
            .filter(|p| p.unsupported.is_some())
            .count(),
        1
    );
    assert!(cache.blacklisted(&plugins.join("mix.clap")).is_none());
    // The file that never got as far as its first class is blacklisted, as a timeout.
    let hang = cache.blacklisted(&plugins.join("hang.clap")).unwrap();
    assert!(is_timeout_reason(&hang.reason), "{}", hang.reason);
    let _ = std::fs::remove_dir_all(&dir);
}
