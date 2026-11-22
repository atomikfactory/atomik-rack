//! The retry of a file that timed out, with no time limit: it runs that one file, a success takes
//! the file off the blacklist, a failure keeps it there with the new reason, a cancel changes
//! nothing; and a cache that holds a class that did not answer reads back as it was written.

use atomik_device_sdk::{PluginCategory, PluginFormat, PluginKindHint, PluginLayout, PluginRef};
use atomik_plugin_host::{
    FailureKind, FileStamp, NO_TIME_LIMIT, PluginCache, PluginSettings, ScanError, ScanFailure,
    ScanJob, ScanProgress, ScanResult, ScanRunner, ScannedPlugin, is_timeout_reason, rescan_file,
    timeout_reason,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn plugin(id: &str, unsupported: Option<&str>) -> ScannedPlugin {
    ScannedPlugin {
        plugin: PluginRef {
            format: PluginFormat::Clap,
            id: id.to_string(),
        },
        layout: PluginLayout {
            name: id.to_string(),
            vendor: "V".to_string(),
            version: "1".to_string(),
            kind_hint: PluginKindHint::Effect,
            main_in: None,
            main_out: None,
            sidechain: false,
            notes_in: false,
            notes_out: false,
            aux_out: 0,
            params: Vec::new(),
        },
        category: PluginCategory::default(),
        unsupported: unsupported.map(str::to_string),
        params_truncated: false,
    }
}

/// A runner that gives `answer` and remembers the time limit it was asked with.
struct Slow {
    answer: Result<ScanResult, ScanFailure>,
    limits: Mutex<Vec<Duration>>,
}

impl Slow {
    fn new(answer: Result<ScanResult, ScanFailure>) -> Arc<Self> {
        Arc::new(Self {
            answer,
            limits: Mutex::new(Vec::new()),
        })
    }
}

impl ScanRunner for Slow {
    fn scan(
        &self,
        _format: PluginFormat,
        _path: &Path,
        timeout: Duration,
        _cancel: &AtomicBool,
    ) -> Result<ScanResult, ScanFailure> {
        self.limits.lock().unwrap().push(timeout);
        self.answer.clone()
    }
}

fn fail(kind: FailureKind, reason: &str) -> Result<ScanResult, ScanFailure> {
    Err(ScanFailure {
        kind,
        reason: reason.to_string(),
    })
}

fn good(path: &Path) -> Result<ScanResult, ScanFailure> {
    Ok(ScanResult {
        path: path.to_path_buf(),
        format: PluginFormat::Clap,
        plugins: vec![plugin("slow.one", None)],
        error: None,
    })
}

fn enabled() -> PluginSettings {
    let mut s = PluginSettings::default();
    s.enable();
    s
}

/// A real file (its size and time are stamped) that is on the blacklist for `reason`.
fn blacklisted(tag: &str, reason: &str) -> (PathBuf, PluginCache) {
    let dir = std::env::temp_dir().join(format!("atomik-hs-retry-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("slow.clap");
    std::fs::write(&path, b"x").unwrap();
    let mut cache = PluginCache::default();
    cache.blacklist_file(
        &path,
        PluginFormat::Clap,
        FileStamp::of(&path).unwrap(),
        reason,
    );
    (path, cache)
}

#[test]
fn a_timed_out_file_retried_with_no_limit_leaves_the_blacklist_when_it_scans() {
    let (path, mut cache) = blacklisted("ok", &timeout_reason(30));
    assert!(is_timeout_reason(&cache.blacklisted(&path).unwrap().reason));
    let runner = Slow::new(good(&path));
    let report = rescan_file(
        &enabled(),
        (PluginFormat::Clap, &path),
        NO_TIME_LIMIT,
        &mut cache,
        runner.as_ref(),
        &AtomicBool::new(false),
        &ScanProgress::default(),
    )
    .unwrap();
    assert_eq!((report.scanned, report.ok, report.failed), (1, 1, 0));
    assert_eq!(*runner.limits.lock().unwrap(), [NO_TIME_LIMIT]);
    assert!(cache.blacklisted(&path).is_none());
    assert_eq!(cache.file(&path).unwrap().plugins.len(), 1);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_retry_that_fails_keeps_the_file_blacklisted_with_the_new_reason() {
    let (path, mut cache) = blacklisted("fail", &timeout_reason(30));
    let runner = Slow::new(fail(
        FailureKind::Crash,
        "the scanner exited with code 3 (0x3)",
    ));
    let report = rescan_file(
        &enabled(),
        (PluginFormat::Clap, &path),
        NO_TIME_LIMIT,
        &mut cache,
        runner.as_ref(),
        &AtomicBool::new(false),
        &ScanProgress::default(),
    )
    .unwrap();
    assert_eq!((report.ok, report.failed), (0, 1));
    let entry = cache.blacklisted(&path).unwrap();
    assert!(!is_timeout_reason(&entry.reason), "{}", entry.reason);
    assert!(cache.file(&path).is_none());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_cancelled_retry_changes_nothing() {
    let (path, mut cache) = blacklisted("cancel", &timeout_reason(30));
    let before = cache.clone();
    let runner = Slow::new(fail(FailureKind::Cancelled, "cancelled"));
    let cancel = AtomicBool::new(true);
    let report = rescan_file(
        &enabled(),
        (PluginFormat::Clap, &path),
        NO_TIME_LIMIT,
        &mut cache,
        runner.as_ref(),
        &cancel,
        &ScanProgress::default(),
    )
    .unwrap();
    assert!(report.cancelled);
    assert_eq!((report.scanned, report.failed), (0, 0));
    assert_eq!(cache, before);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_retry_needs_plugins_to_be_on() {
    let (path, mut cache) = blacklisted("off", &timeout_reason(30));
    let before = cache.clone();
    let runner = Slow::new(good(&path));
    let err = rescan_file(
        &PluginSettings::default(),
        (PluginFormat::Clap, &path),
        NO_TIME_LIMIT,
        &mut cache,
        runner.as_ref(),
        &AtomicBool::new(false),
        &ScanProgress::default(),
    )
    .unwrap_err();
    assert_eq!(err, ScanError::PluginsDisabled);
    assert!(runner.limits.lock().unwrap().is_empty());
    assert_eq!(cache, before);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn the_retry_runs_on_a_worker_and_hands_the_cache_back() {
    let (path, cache) = blacklisted("job", &timeout_reason(30));
    let runner = Slow::new(good(&path));
    let job = ScanJob::start_file_without_limit(
        enabled(),
        (PluginFormat::Clap, path.clone()),
        cache,
        runner.clone(),
    );
    let (cache, report) = job.finish();
    assert_eq!(report.unwrap().ok, 1);
    assert_eq!(*runner.limits.lock().unwrap(), [NO_TIME_LIMIT]);
    assert!(cache.blacklisted(&path).is_none());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_class_that_did_not_answer_is_cached_as_unsupported_and_reads_back() {
    let dir = std::env::temp_dir().join(format!("atomik-hs-retry-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shell.clap");
    std::fs::write(&path, b"x").unwrap();
    let mut cache = PluginCache::default();
    cache.insert_file(atomik_plugin_host::CachedFile {
        path: path.clone(),
        format: PluginFormat::Clap,
        stamp: FileStamp::of(&path).unwrap(),
        scanner_version: atomik_plugin_host::SCANNER_VERSION,
        scanned_secs: 1,
        plugins: vec![
            plugin("a", None),
            plugin(
                "b",
                Some("did not answer within 30 s; the scanner was killed"),
            ),
        ],
    });
    let (loaded, status) = {
        let file = dir.join("plugins.json");
        cache.save(&file).unwrap();
        PluginCache::load(&file)
    };
    assert_eq!(status, atomik_plugin_host::LoadStatus::Loaded);
    assert_eq!(loaded, cache);
    assert!(cache.blacklisted(&path).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
