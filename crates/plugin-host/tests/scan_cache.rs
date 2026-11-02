//! (library half): the `plugins.json` cache, incremental and full rescans, the blacklist,
//! the consent gate, cancellation, ordering and the two-at-a-time limit, over a fake scanner.

use atomik_device_sdk::{
    Channels, PluginFormat, PluginKindHint, PluginLayout, PluginParamInfo, PluginRef,
};
use atomik_plugin_host::paths::Os;
use atomik_plugin_host::{
    FailureKind, LoadStatus, PluginCache, PluginSettings, RescanMode, ScanFailure, ScanProgress,
    ScanRequest, ScanResult, ScanRunner, ScannedPlugin, rescan,
};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-hs-cache-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn layout(name: &str) -> PluginLayout {
    PluginLayout {
        name: name.to_string(),
        vendor: "Acme \"Q\"".to_string(),
        version: "1.2.3".to_string(),
        kind_hint: PluginKindHint::Instrument,
        main_in: None,
        main_out: Some(Channels::Stereo),
        sidechain: true,
        notes_in: true,
        notes_out: false,
        params: vec![
            PluginParamInfo {
                id: 7,
                name: "Cut".to_string(),
                module: "Filter/A".to_string(),
                min: -1.5,
                max: 22000.123456789,
                default: 0.1,
                stepped: false,
                steps: 0,
            },
            PluginParamInfo {
                id: u32::MAX,
                name: "Mode".to_string(),
                module: String::new(),
                min: 0.0,
                max: 3.0,
                default: 1.0,
                stepped: true,
                steps: 3,
            },
        ],
    }
}

fn scanned(id: &str) -> ScannedPlugin {
    ScannedPlugin {
        plugin: PluginRef {
            format: PluginFormat::Clap,
            id: id.to_string(),
        },
        layout: layout(id),
        unsupported: None,
        params_truncated: false,
    }
}

type Behavior = Box<dyn Fn(&Path) -> Result<ScanResult, ScanFailure> + Send + Sync>;

struct Fake {
    calls: Mutex<Vec<PathBuf>>,
    behavior: Behavior,
    running: AtomicUsize,
    max_running: AtomicUsize,
}

impl Fake {
    fn new(behavior: Behavior) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            behavior,
            running: AtomicUsize::new(0),
            max_running: AtomicUsize::new(0),
        })
    }
    fn ok() -> Arc<Self> {
        Self::new(Box::new(|p| Ok(good(p))))
    }
    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
    fn take(&self) -> Vec<String> {
        let mut c = std::mem::take(&mut *self.calls.lock().unwrap());
        c.sort();
        c.iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }
}

fn good(path: &Path) -> ScanResult {
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    ScanResult {
        path: path.to_path_buf(),
        format: PluginFormat::Clap,
        plugins: vec![scanned(&format!("test.{stem}"))],
        error: None,
    }
}

impl ScanRunner for Fake {
    fn scan(
        &self,
        _format: PluginFormat,
        path: &Path,
        _timeout: Duration,
        _cancel: &AtomicBool,
    ) -> Result<ScanResult, ScanFailure> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_running.fetch_max(now, Ordering::SeqCst);
        self.calls.lock().unwrap().push(path.to_path_buf());
        let r = (self.behavior)(path);
        self.running.fetch_sub(1, Ordering::SeqCst);
        r
    }
}

fn no_env(_: &str) -> Option<OsString> {
    None
}

fn enabled(dir: &Path) -> PluginSettings {
    let mut s = PluginSettings::default();
    s.enable();
    s.extra_paths = vec![dir.to_path_buf()];
    s
}

fn request(settings: &PluginSettings, mode: RescanMode) -> ScanRequest<'_> {
    ScanRequest {
        settings,
        formats: &[PluginFormat::Clap],
        os: Os::Windows,
        env: &no_env,
        mode,
    }
}

fn run(
    settings: &PluginSettings,
    cache: &mut PluginCache,
    runner: &Fake,
    mode: RescanMode,
) -> atomik_plugin_host::ScanReport {
    let cancel = AtomicBool::new(false);
    rescan(
        &request(settings, mode),
        cache,
        runner,
        &cancel,
        &ScanProgress::default(),
    )
    .unwrap()
}

fn plugin_files(dir: &Path, names: &[&str]) {
    for n in names {
        std::fs::write(dir.join(n), n.as_bytes()).unwrap();
    }
}

fn bump_mtime(path: &Path) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    let t = std::fs::metadata(path).unwrap().modified().unwrap() + Duration::from_secs(5);
    f.set_modified(t).unwrap();
}

// ----- JSON -----------------------------------------------------------------------------------

#[test]
fn a_scan_result_round_trips_through_json_exactly() {
    let r = ScanResult {
        path: PathBuf::from("C:/x/a.clap"),
        format: PluginFormat::Clap,
        plugins: vec![
            scanned("a"),
            ScannedPlugin {
                unsupported: Some("no stereo bus".to_string()),
                params_truncated: true,
                ..scanned("b")
            },
        ],
        error: Some("boom".to_string()),
    };
    let back = ScanResult::from_json(&r.to_json()).unwrap();
    assert_eq!(back, r);
    assert!(ScanResult::from_json("not json").is_err());
    assert!(ScanResult::from_json("{}").is_err());
}

#[test]
fn non_finite_params_are_made_finite_not_lost() {
    let mut p = scanned("n");
    p.layout.params[0].min = f64::NAN;
    p.layout.params[0].max = f64::INFINITY;
    let r = ScanResult {
        path: PathBuf::from("a.clap"),
        format: PluginFormat::Clap,
        plugins: vec![p],
        error: None,
    };
    let back = ScanResult::from_json(&r.to_json()).unwrap();
    let row = &back.plugins[0].layout.params[0];
    assert!(row.min.is_finite() && row.max.is_finite());
}

// ----- cache file -----------------------------------------------------------------------------

#[test]
fn the_cache_round_trips_and_writes_the_same_bytes_every_time() {
    let d = temp("rt");
    plugin_files(&d, &["a.clap", "b.clap"]);
    let mut cache = PluginCache::default();
    run(
        &enabled(&d),
        &mut cache,
        &Fake::ok(),
        RescanMode::Incremental,
    );
    let file = d.join("plugins.json");
    cache.save(&file).unwrap();
    let first = std::fs::read(&file).unwrap();
    let (loaded, status) = PluginCache::load(&file);
    assert_eq!(status, LoadStatus::Loaded);
    assert_eq!(loaded, cache);
    loaded.save(&file).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), first);
    assert!(
        !d.join("plugins.json.tmp").exists(),
        "the temp file is renamed away"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_missing_corrupt_or_old_cache_is_ignored_and_rebuilt() {
    let d = temp("bad");
    let file = d.join("plugins.json");
    let (c, s) = PluginCache::load(&file);
    assert_eq!((c, s), (PluginCache::default(), LoadStatus::Missing));
    for junk in [&b"{ truncated"[..], b"", b"[1,2]", b"\xff\xfe\x00"] {
        std::fs::write(&file, junk).unwrap();
        let (c, s) = PluginCache::load(&file);
        assert_eq!(c, PluginCache::default());
        assert!(matches!(s, LoadStatus::Rebuilt(_)), "{junk:?}");
    }
    std::fs::write(&file, br#"{"version":9999,"files":[],"blacklist":[]}"#).unwrap();
    assert!(matches!(PluginCache::load(&file).1, LoadStatus::Rebuilt(_)));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_truncated_temp_file_never_replaces_the_cache() {
    let d = temp("atomic");
    plugin_files(&d, &["a.clap"]);
    let mut cache = PluginCache::default();
    run(
        &enabled(&d),
        &mut cache,
        &Fake::ok(),
        RescanMode::Incremental,
    );
    let file = d.join("plugins.json");
    cache.save(&file).unwrap();
    std::fs::write(d.join("plugins.json.tmp"), b"{\"version\":1,\"fi").unwrap();
    let (loaded, status) = PluginCache::load(&file);
    assert_eq!((loaded, status), (cache.clone(), LoadStatus::Loaded));
    // A save that cannot finish (the target is a directory) leaves the old file alone.
    let blocked = d.join("blocked.json");
    std::fs::create_dir_all(&blocked).unwrap();
    assert!(cache.save(&blocked).is_err());
    assert!(
        !d.join("blocked.json.tmp").exists(),
        "the temp file is cleaned up"
    );
    cache.save(&file).unwrap();
    assert_eq!(PluginCache::load(&file).1, LoadStatus::Loaded);
    let _ = std::fs::remove_dir_all(&d);
}

// ----- rescans --------------------------------------------------------------------------------

#[test]
fn a_second_scan_is_all_cache_hits_and_spawns_nothing() {
    let d = temp("hit");
    plugin_files(&d, &["a.clap", "b.clap", "c.clap"]);
    let s = enabled(&d);
    let mut cache = PluginCache::default();
    let fake = Fake::ok();
    let r1 = run(&s, &mut cache, &fake, RescanMode::Incremental);
    assert_eq!((r1.scanned, r1.cached, r1.ok, r1.failed), (3, 0, 3, 0));
    assert_eq!(fake.take(), ["a.clap", "b.clap", "c.clap"]);
    let r2 = run(&s, &mut cache, &fake, RescanMode::Incremental);
    assert_eq!((r2.scanned, r2.cached), (0, 3));
    assert_eq!(fake.count(), 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn changed_mtime_or_size_new_and_removed_files_are_handled_incrementally() {
    let d = temp("inc");
    plugin_files(&d, &["a.clap", "b.clap", "c.clap"]);
    let s = enabled(&d);
    let mut cache = PluginCache::default();
    let fake = Fake::ok();
    run(&s, &mut cache, &fake, RescanMode::Incremental);
    fake.take();
    bump_mtime(&d.join("a.clap"));
    std::fs::write(d.join("b.clap"), b"longer content than before").unwrap();
    plugin_files(&d, &["d.clap"]);
    std::fs::remove_file(d.join("c.clap")).unwrap();
    let r = run(&s, &mut cache, &fake, RescanMode::Incremental);
    assert_eq!(fake.take(), ["a.clap", "b.clap", "d.clap"]);
    assert_eq!((r.scanned, r.cached, r.removed), (3, 0, 1));
    let names: Vec<String> = cache
        .files()
        .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["a.clap", "b.clap", "d.clap"]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_full_rescan_visits_every_file() {
    let d = temp("full");
    plugin_files(&d, &["a.clap", "b.clap"]);
    let s = enabled(&d);
    let mut cache = PluginCache::default();
    let fake = Fake::ok();
    run(&s, &mut cache, &fake, RescanMode::Incremental);
    fake.take();
    let r = run(&s, &mut cache, &fake, RescanMode::Full);
    assert_eq!(r.scanned, 2);
    assert_eq!(fake.take(), ["a.clap", "b.clap"]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_folder_that_leaves_the_search_paths_leaves_the_cache() {
    let d = temp("leave");
    plugin_files(&d, &["a.clap"]);
    let mut s = enabled(&d);
    let mut cache = PluginCache::default();
    run(&s, &mut cache, &Fake::ok(), RescanMode::Incremental);
    s.extra_paths.clear();
    let r = run(&s, &mut cache, &Fake::ok(), RescanMode::Incremental);
    assert_eq!(r.removed, 1);
    assert_eq!(cache.files().count(), 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_scanner_version_invalidates_cached_entries() {
    let d = temp("ver");
    plugin_files(&d, &["a.clap"]);
    let s = enabled(&d);
    let mut cache = PluginCache::default();
    let fake = Fake::ok();
    run(&s, &mut cache, &fake, RescanMode::Incremental);
    fake.take();
    cache.set_scanner_version_for_test(0);
    run(&s, &mut cache, &fake, RescanMode::Incremental);
    assert_eq!(fake.take(), ["a.clap"]);
    let _ = std::fs::remove_dir_all(&d);
}

// ----- blacklist ------------------------------------------------------------------------------

fn failing(kind: FailureKind) -> Behavior {
    Box::new(move |p| {
        if p.file_name().unwrap() == "bad.clap" {
            Err(ScanFailure {
                kind,
                reason: format!("{kind:?} happened"),
            })
        } else {
            Ok(good(p))
        }
    })
}

#[test]
fn crashing_hanging_and_garbage_files_are_blacklisted_with_a_reason() {
    for kind in [
        FailureKind::Crash,
        FailureKind::Timeout,
        FailureKind::BadOutput,
    ] {
        let d = temp("bl");
        plugin_files(&d, &["bad.clap", "ok.clap"]);
        let s = enabled(&d);
        let mut cache = PluginCache::default();
        let r = run(
            &s,
            &mut cache,
            &Fake::new(failing(kind)),
            RescanMode::Incremental,
        );
        assert_eq!((r.ok, r.failed), (1, 1));
        let b: Vec<_> = cache.blacklist().collect();
        assert_eq!(b.len(), 1);
        assert!(b[0].path.ends_with("bad.clap"));
        assert!(b[0].reason.contains(&format!("{kind:?}")));
        assert!(b[0].date_secs > 0);
        assert_eq!(cache.files().count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn a_clean_result_that_carries_an_error_or_no_plugin_is_a_failure_too() {
    let d = temp("err");
    plugin_files(&d, &["bad.clap", "ok.clap", "empty.clap"]);
    let fake = Fake::new(Box::new(|p| {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let mut r = good(p);
        match name.as_str() {
            "bad.clap" => r.error = Some("init failed".to_string()),
            "empty.clap" => r.plugins.clear(),
            _ => {}
        }
        Ok(r)
    }));
    let mut cache = PluginCache::default();
    let r = run(&enabled(&d), &mut cache, &fake, RescanMode::Incremental);
    assert_eq!((r.ok, r.failed), (1, 2));
    let reasons: BTreeMap<String, String> = cache
        .blacklist()
        .map(|b| {
            (
                b.path.file_name().unwrap().to_string_lossy().into_owned(),
                b.reason.clone(),
            )
        })
        .collect();
    assert!(reasons["bad.clap"].contains("init failed"));
    assert!(!reasons["empty.clap"].is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn blacklisted_files_are_skipped_until_they_change_or_rescan_failed_runs() {
    let d = temp("retry");
    plugin_files(&d, &["bad.clap", "ok.clap"]);
    let s = enabled(&d);
    let mut cache = PluginCache::default();
    let fake = Fake::new(failing(FailureKind::Crash));
    run(&s, &mut cache, &fake, RescanMode::Incremental);
    fake.take();
    let r = run(&s, &mut cache, &fake, RescanMode::Incremental);
    assert_eq!((r.scanned, r.cached, r.skipped_blacklisted), (0, 1, 1));
    assert_eq!(fake.count(), 0, "a blacklisted file is not spawned again");
    // The file changes: retried.
    bump_mtime(&d.join("bad.clap"));
    run(&s, &mut cache, &fake, RescanMode::Incremental);
    assert_eq!(fake.take(), ["bad.clap"]);
    // RESCAN FAILED retries it even though it did not change, and nothing else.
    let r = run(&s, &mut cache, &fake, RescanMode::FailedOnly);
    assert_eq!(fake.take(), ["bad.clap"]);
    assert_eq!(r.scanned, 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_blacklisted_file_that_scans_cleanly_later_leaves_the_blacklist() {
    let d = temp("heal");
    plugin_files(&d, &["bad.clap"]);
    let s = enabled(&d);
    let mut cache = PluginCache::default();
    run(
        &s,
        &mut cache,
        &Fake::new(failing(FailureKind::Crash)),
        RescanMode::Incremental,
    );
    assert_eq!(cache.blacklist().count(), 1);
    run(&s, &mut cache, &Fake::ok(), RescanMode::FailedOnly);
    assert_eq!(cache.blacklist().count(), 0);
    assert_eq!(cache.files().count(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn unsupported_plugins_are_cached_not_blacklisted() {
    let d = temp("unsup");
    plugin_files(&d, &["u.clap"]);
    let fake = Fake::new(Box::new(|p| {
        let mut r = good(p);
        r.plugins[0].unsupported = Some("surround only".to_string());
        Ok(r)
    }));
    let mut cache = PluginCache::default();
    run(&enabled(&d), &mut cache, &fake, RescanMode::Incremental);
    assert_eq!(cache.blacklist().count(), 0);
    assert_eq!(
        cache.files().next().unwrap().plugins[0]
            .unsupported
            .as_deref(),
        Some("surround only")
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_spawn_failure_or_cancellation_is_not_the_files_fault() {
    for kind in [FailureKind::Spawn, FailureKind::Cancelled] {
        let d = temp("nofault");
        plugin_files(&d, &["a.clap"]);
        let fake = Fake::new(Box::new(move |_| {
            Err(ScanFailure {
                kind,
                reason: "x".to_string(),
            })
        }));
        let mut cache = PluginCache::default();
        run(&enabled(&d), &mut cache, &fake, RescanMode::Incremental);
        assert_eq!(cache.blacklist().count(), 0);
        assert_eq!(cache.files().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn the_user_can_clear_one_blacklist_entry() {
    let d = temp("clear");
    plugin_files(&d, &["bad.clap"]);
    let s = enabled(&d);
    let mut cache = PluginCache::default();
    run(
        &s,
        &mut cache,
        &Fake::new(failing(FailureKind::Crash)),
        RescanMode::Incremental,
    );
    let path = cache.blacklist().next().unwrap().path.clone();
    assert!(cache.unblacklist(&path));
    assert!(!cache.unblacklist(&path));
    let fake = Fake::ok();
    run(&s, &mut cache, &fake, RescanMode::Incremental);
    assert_eq!(fake.take(), ["bad.clap"]);
    let _ = std::fs::remove_dir_all(&d);
}

// ----- consent, cancellation, scheduling ------------------------------------------------------

#[test]
fn plugins_are_off_by_default_and_scanning_needs_the_switch() {
    let d = temp("gate");
    plugin_files(&d, &["a.clap"]);
    let mut s = PluginSettings::default();
    assert!(!s.enabled);
    s.extra_paths = vec![d.clone()];
    let fake = Fake::ok();
    let mut cache = PluginCache::default();
    let cancel = AtomicBool::new(false);
    let err = rescan(
        &request(&s, RescanMode::Full),
        &mut cache,
        fake.as_ref(),
        &cancel,
        &ScanProgress::default(),
    );
    assert_eq!(
        err.unwrap_err(),
        atomik_plugin_host::ScanError::PluginsDisabled
    );
    assert_eq!(fake.count(), 0);
    // Enabling is one call: there is no warning to acknowledge.
    s.enable();
    assert!(s.enabled);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_timeout_is_clamped_to_5_to_300_seconds() {
    let mut s = PluginSettings::default();
    assert_eq!(s.timeout(), Duration::from_secs(30));
    s.set_timeout_secs(1);
    assert_eq!(s.timeout(), Duration::from_secs(5));
    s.set_timeout_secs(9999);
    assert_eq!(s.timeout(), Duration::from_secs(300));
    let json = serde_json::to_string(&s).unwrap();
    let back: PluginSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
    let empty: PluginSettings = serde_json::from_str("{}").unwrap();
    assert!(!empty.enabled);
}

#[test]
fn a_timeout_read_from_a_settings_file_is_clamped_when_used() {
    // The setter is not the only way in: a hand-edited or old settings file carries any number.
    let low: PluginSettings = serde_json::from_str(r#"{"timeout_secs": 1}"#).unwrap();
    assert_eq!(low.timeout_secs, 1, "the field keeps what the file said");
    assert_eq!(low.timeout(), Duration::from_secs(5));
    let high: PluginSettings = serde_json::from_str(r#"{"timeout_secs": 100000}"#).unwrap();
    assert_eq!(high.timeout(), Duration::from_secs(300));
    let zero: PluginSettings = serde_json::from_str(r#"{"timeout_secs": 0}"#).unwrap();
    assert_eq!(
        zero.timeout(),
        Duration::from_secs(5),
        "zero would kill every scan at once"
    );
}

#[test]
fn a_cancelled_scan_starts_nothing_and_blacklists_nothing() {
    let d = temp("cancel");
    plugin_files(&d, &["a.clap", "b.clap"]);
    let s = enabled(&d);
    let mut cache = PluginCache::default();
    let fake = Fake::ok();
    let cancel = AtomicBool::new(true);
    let r = rescan(
        &request(&s, RescanMode::Incremental),
        &mut cache,
        fake.as_ref(),
        &cancel,
        &ScanProgress::default(),
    )
    .unwrap();
    assert!(r.cancelled);
    assert_eq!(fake.count(), 0);
    assert_eq!(cache, PluginCache::default());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn scanners_run_in_parallel_up_to_the_core_count_and_the_cache_order_is_deterministic() {
    let d = temp("par");
    let names: Vec<String> = (0..10).map(|i| format!("p{i}.clap")).collect();
    plugin_files(&d, &names.iter().map(String::as_str).collect::<Vec<_>>());
    let s = enabled(&d);
    let slow = Fake::new(Box::new(|p| {
        // Earlier files take longer, so completion order differs from file order.
        let n: u64 = p.file_stem().unwrap().to_string_lossy()[1..]
            .parse()
            .unwrap();
        std::thread::sleep(Duration::from_millis(40 - 3 * n));
        Ok(good(p))
    }));
    let mut cache = PluginCache::default();
    run(&s, &mut cache, &slow, RescanMode::Incremental);
    let cores = std::thread::available_parallelism().map_or(2, std::num::NonZero::get);
    let expected = atomik_plugin_host::parallel_scans(cores).min(names.len());
    let ran = slow.max_running.load(Ordering::SeqCst);
    // Threads may not all overlap on a loaded machine: at least two did, never more than allowed.
    assert!((2..=expected).contains(&ran), "{ran} of {expected}");
    let paths: Vec<_> = cache.files().map(|f| f.path.clone()).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    let mut again = PluginCache::default();
    run(&s, &mut again, &Fake::ok(), RescanMode::Incremental);
    assert_eq!(again.to_json(), cache.to_json());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn progress_reports_total_and_done() {
    let d = temp("prog");
    plugin_files(&d, &["a.clap", "b.clap", "c.clap"]);
    let s = enabled(&d);
    let mut cache = PluginCache::default();
    let progress = ScanProgress::default();
    let cancel = AtomicBool::new(false);
    rescan(
        &request(&s, RescanMode::Incremental),
        &mut cache,
        Fake::ok().as_ref(),
        &cancel,
        &progress,
    )
    .unwrap();
    assert_eq!(progress.snapshot(), (3, 3));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_bundle_folder_is_stamped_by_everything_inside_it() {
    // A `.vst3` bundle is a folder: replacing the binary deep inside it does not touch the
    // folder's own time, but it must still make a cached scan stale.
    let d = temp("bundle-stamp");
    let bundle = d.join("A.vst3");
    let bin = bundle.join("Contents").join("x86_64-win");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("A.vst3"), b"first build").unwrap();
    use atomik_plugin_host::FileStamp;
    let a = FileStamp::of(&bundle).unwrap();
    assert_eq!(a.size, 11, "the size is the sum of the files inside");
    assert_eq!(
        FileStamp::of(&bundle),
        Some(a),
        "stable while nothing changes"
    );
    // A rebuilt binary of another size.
    std::fs::write(bin.join("A.vst3"), b"second build, longer").unwrap();
    let b = FileStamp::of(&bundle).unwrap();
    assert_ne!(a, b);
    assert_eq!(b.size, 20);
    // The same size but a later time (touch the nested file only).
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(bin.join("A.vst3"))
        .unwrap();
    f.set_modified(
        std::fs::metadata(bin.join("A.vst3"))
            .unwrap()
            .modified()
            .unwrap()
            + Duration::from_secs(30),
    )
    .unwrap();
    drop(f);
    let c = FileStamp::of(&bundle).unwrap();
    assert_eq!(c.size, b.size);
    assert!(
        c.mtime_ns > b.mtime_ns,
        "the newest file inside decides the time"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ----- the report names failures and VST2-only folders ----------------------------------------

#[test]
fn the_report_lists_every_failed_file_with_its_reason() {
    let d = temp("report-failures");
    plugin_files(&d, &["good.clap", "bad.clap", "hang.clap", "junk.clap"]);
    let runner = Fake::new(Box::new(|p| {
        let n = p.file_stem().unwrap().to_string_lossy().into_owned();
        match n.as_str() {
            "bad" => Err(ScanFailure {
                kind: FailureKind::Crash,
                reason: "the scanner exited with code 3".to_string(),
            }),
            "hang" => Err(ScanFailure {
                kind: FailureKind::Timeout,
                reason: "did not finish within 5 s".to_string(),
            }),
            "junk" => Ok(ScanResult {
                path: p.to_path_buf(),
                format: PluginFormat::Clap,
                plugins: Vec::new(),
                error: Some("not a CLAP file".to_string()),
            }),
            _ => Ok(good(p)),
        }
    }));
    let mut cache = PluginCache::default();
    let report = run(&enabled(&d), &mut cache, &runner, RescanMode::Full);
    assert_eq!((report.scanned, report.ok, report.failed), (4, 1, 3));
    let names: Vec<(String, String)> = report
        .failures
        .iter()
        .map(|(p, r)| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                r.clone(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            (
                "bad.clap".to_string(),
                "the scanner exited with code 3".to_string()
            ),
            (
                "hang.clap".to_string(),
                "did not finish within 5 s".to_string()
            ),
            ("junk.clap".to_string(), "not a CLAP file".to_string()),
        ],
        "path order, with the reasons"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_search_folder_with_only_vst2_plugins_is_named_in_the_report() {
    let d = temp("report-vst2");
    let vst2 = d.join("VSTPlugins");
    std::fs::create_dir_all(&vst2).unwrap();
    std::fs::write(vst2.join("old.dll"), b"x").unwrap();
    let ok = d.join("Good");
    std::fs::create_dir_all(&ok).unwrap();
    plugin_files(&ok, &["a.clap"]);
    let mut settings = enabled(&d);
    settings.extra_paths = vec![vst2.clone(), ok];
    let mut cache = PluginCache::default();
    let report = run(&settings, &mut cache, &Fake::ok(), RescanMode::Full);
    assert_eq!(report.vst2_folders, [vst2]);
    assert_eq!(report.ok, 1);
    let _ = std::fs::remove_dir_all(&d);
}
