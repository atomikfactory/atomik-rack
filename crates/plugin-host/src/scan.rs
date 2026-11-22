//! Scanning: the scanner's result type, the runner seam, and the
//! rescan driver that walks the search paths, decides what needs scanning from the cache,
//! runs the scanners (see [`parallel_scans`]) and records results and failures.
//!
//! The consent gate lives here: [`rescan`] refuses to do anything while the user has not
//! enabled plugins. No function of this module loads plugin code in the calling process; the
//! real runner ([`crate::ChildScanner`]) starts the `atomik-plugin-scan` program per file.

use crate::backend::ScannedPlugin;
use crate::cache::{CachedFile, FileStamp, PluginCache, SCANNER_VERSION};
use crate::paths::{Os, find_plugin_files, search_roots, vst2_only_folders};
use crate::settings::PluginSettings;
use crate::wire::{PluginDto, ResultDto, format_name, parse_format};
use atomik_device_sdk::PluginFormat;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// The most scanner children that run at the same time.
pub const MAX_PARALLEL_SCANS: usize = 8;

/// The most logical cores of a machine that runs its scanners one at a time.
pub const SERIAL_SCAN_CORES: usize = 4;

/// How many scanner children run at the same time on a machine with `cores` logical cores: one
/// on a machine with [`SERIAL_SCAN_CORES`] or fewer (a scan must not take a core from the audio
/// thread, which may be playing), otherwise half of them, at least two and at most
/// [`MAX_PARALLEL_SCANS`]. The scanners load plugin libraries, which is mostly waiting on the
/// disk and on the plugins' own start-up, so more than one per core would only fight the audio
/// thread. A scan started while audio plays follows the same rule.
pub fn parallel_scans(cores: usize) -> usize {
    if cores <= SERIAL_SCAN_CORES {
        1
    } else {
        (cores / 2).clamp(2, MAX_PARALLEL_SCANS)
    }
}

/// The `timeout` that means "no limit" (the user's retry of a file that timed out).
pub const NO_TIME_LIMIT: Duration = Duration::MAX;

/// How a timeout reason starts (see [`timeout_reason`] and [`is_timeout_reason`]).
const TIMEOUT_REASON_START: &str = "did not finish within ";

/// The blacklist reason of a file whose scan ran out of time after `secs` seconds.
pub fn timeout_reason(secs: u64) -> String {
    format!("{TIMEOUT_REASON_START}{secs} s; the scanner was killed")
}

/// True when a blacklist `reason` is a timeout (what the plugin manager offers a retry with no
/// time limit for), as opposed to a crash or an error.
pub fn is_timeout_reason(reason: &str) -> bool {
    reason.starts_with(TIMEOUT_REASON_START)
}

/// Every plugin format the backends support, in the order a scan visits them.
pub const ALL_FORMATS: [PluginFormat; 2] = [PluginFormat::Clap, PluginFormat::Vst3];

/// What one scanner run reports for one file (the JSON the scanner prints).
#[derive(Debug, Clone, PartialEq)]
pub struct ScanResult {
    /// The file that was scanned.
    pub path: PathBuf,
    /// The format it was scanned as.
    pub format: PluginFormat,
    /// The plugins found.
    pub plugins: Vec<ScannedPlugin>,
    /// Why the file could not be scanned (the scanner still exits cleanly).
    pub error: Option<String>,
}

impl ScanResult {
    /// The result as one line of JSON.
    pub fn to_json(&self) -> String {
        let dto = ResultDto {
            path: self.path.to_string_lossy().into_owned(),
            format: format_name(self.format),
            plugins: self.plugins.iter().map(PluginDto::from_scanned).collect(),
            error: self.error.clone(),
        };
        serde_json::to_string(&dto).unwrap_or_default()
    }

    /// Parses a scanner's JSON.
    ///
    /// # Errors
    /// [`ScanError::BadResult`] when it is not a result.
    pub fn from_json(text: &str) -> Result<Self, ScanError> {
        let bad = |e: String| ScanError::BadResult(e);
        let dto: ResultDto = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
        let format = parse_format(&dto.format).map_err(bad)?;
        let plugins = dto
            .plugins
            .into_iter()
            .map(|p| p.into_scanned(format))
            .collect::<Result<Vec<_>, _>>()
            .map_err(bad)?;
        Ok(Self {
            path: PathBuf::from(dto.path),
            format,
            plugins,
            error: dto.error,
        })
    }
}

/// One line the scanner prints before its final result line, so that the parent keeps what a
/// scan found even when the scanner is killed half way through a file.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanEvent {
    /// The classes the scanner is about to visit, in order, as the file declares them (each
    /// with the little that is known without creating it).
    Classes(Vec<ScannedPlugin>),
    /// One class, finished: created, read and activated once (or why that did not work).
    Class(ScannedPlugin),
}

impl ScanEvent {
    /// The event as one line of JSON.
    pub fn to_json(&self) -> String {
        let value = match self {
            ScanEvent::Classes(all) => serde_json::json!({
                "classes": all.iter().map(PluginDto::from_scanned).collect::<Vec<_>>()
            }),
            ScanEvent::Class(one) => serde_json::json!({ "class": PluginDto::from_scanned(one) }),
        };
        value.to_string()
    }

    /// Parses an event line of a scan of `format`; `None` for anything else (the final result
    /// line, a line a plugin printed).
    pub fn from_json(text: &str, format: PluginFormat) -> Option<Self> {
        let mut value: serde_json::Value = serde_json::from_str(text).ok()?;
        let object = value.as_object_mut()?;
        if let Some(classes) = object.remove("classes") {
            let dtos: Vec<PluginDto> = serde_json::from_value(classes).ok()?;
            let all = dtos
                .into_iter()
                .map(|d| d.into_scanned(format))
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            return Some(ScanEvent::Classes(all));
        }
        let dto: PluginDto = serde_json::from_value(object.remove("class")?).ok()?;
        dto.into_scanned(format).ok().map(ScanEvent::Class)
    }
}

/// What can go wrong with a scan as a whole.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    /// The user has not enabled third-party plugins.
    #[error("third-party plugins are disabled")]
    PluginsDisabled,
    /// The scanner's output is not a scan result.
    #[error("unreadable scan result: {0}")]
    BadResult(String),
}

/// Why one file's scan did not give a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureKind {
    /// The scanner exited with a non-zero code or a signal.
    Crash,
    /// The scanner did not finish in time and was killed.
    Timeout,
    /// The scanner exited cleanly but its output is not a result.
    BadOutput,
    /// The scanner reported an error or found no plugin.
    Error,
    /// The scanner could not be started (not the file's fault).
    Spawn,
    /// The scan was cancelled (not the file's fault).
    Cancelled,
}

impl FailureKind {
    /// True when the file is to blame and goes on the blacklist.
    pub fn blames_file(self) -> bool {
        !matches!(self, FailureKind::Spawn | FailureKind::Cancelled)
    }
}

/// A scan that gave no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanFailure {
    /// What happened.
    pub kind: FailureKind,
    /// The details for the blacklist and the settings list.
    pub reason: String,
}

/// Something that scans one file. The real one starts a child process; tests use fakes.
pub trait ScanRunner: Send + Sync {
    /// Scans `path` as `format`, giving up after `timeout` or when `cancel` becomes true.
    ///
    /// # Errors
    /// A [`ScanFailure`] when the file crashed, hung or produced garbage, or the scan could not
    /// run.
    fn scan(
        &self,
        format: PluginFormat,
        path: &Path,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<ScanResult, ScanFailure>;
}

/// What a rescan visits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RescanMode {
    /// New files, changed files (size, time or scanner version) and blacklisted files that changed.
    Incremental,
    /// Every file.
    Full,
    /// RESCAN FAILED: the blacklisted files (changed or not) plus whatever an incremental scan finds.
    FailedOnly,
}

/// What to scan and how.
pub struct ScanRequest<'a> {
    /// The user's plugin settings (consent, extra folders, timeout).
    pub settings: &'a PluginSettings,
    /// The formats to look for (the app passes [`ALL_FORMATS`]).
    pub formats: &'a [PluginFormat],
    /// The OS whose default folders apply.
    pub os: Os,
    /// Environment lookup (`CLAP_PATH`, `HOME`, ...).
    pub env: &'a (dyn Fn(&str) -> Option<OsString> + Sync),
    /// What to visit.
    pub mode: RescanMode,
}

/// Progress of a running scan, readable from the UI thread.
#[derive(Debug, Default)]
pub struct ScanProgress {
    total: AtomicUsize,
    done: AtomicUsize,
}

impl ScanProgress {
    /// `(done, total)` files of the running scan.
    pub fn snapshot(&self) -> (usize, usize) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}

/// What a rescan did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Files handed to a scanner.
    pub scanned: usize,
    /// Files whose cached scan was still valid.
    pub cached: usize,
    /// Files that scanned cleanly.
    pub ok: usize,
    /// Files that failed and were blacklisted.
    pub failed: usize,
    /// Blacklisted files skipped because they did not change.
    pub skipped_blacklisted: usize,
    /// Cache and blacklist entries dropped (file gone or folder no longer searched).
    pub removed: usize,
    /// The scan was cancelled before it finished.
    pub cancelled: bool,
    /// The first scanner start failure, if any (the scanner program is missing).
    pub spawn_error: Option<String>,
    /// Every file that failed, in path order, with the reason (the blacklist reasons).
    pub failures: Vec<(PathBuf, String)>,
    /// Search folders of the user that hold VST2 plugins only (VST2 is not supported).
    pub vst2_folders: Vec<PathBuf>,
}

type JobResult = Result<ScanResult, ScanFailure>;

/// Walks the search paths and scans what the cache says needs scanning, updating `cache`.
/// Files are visited in path order; the cache is the same whatever order the (up to
/// [`parallel_scans`]) scanners finish in.
///
/// # Errors
/// [`ScanError::PluginsDisabled`] when the user has not enabled plugins; nothing is touched.
pub fn rescan(
    req: &ScanRequest<'_>,
    cache: &mut PluginCache,
    runner: &dyn ScanRunner,
    cancel: &AtomicBool,
    progress: &ScanProgress,
) -> Result<ScanReport, ScanError> {
    if !req.settings.enabled {
        return Err(ScanError::PluginsDisabled);
    }
    let mut listed: Vec<(PluginFormat, PathBuf)> = Vec::new();
    for &format in req.formats {
        let roots = search_roots(format, req.os, req.env, &req.settings.extra_paths);
        listed.extend(
            find_plugin_files(format, &roots)
                .into_iter()
                .map(|p| (format, p)),
        );
    }
    listed.sort_by(|a, b| a.1.cmp(&b.1));
    listed.dedup_by(|a, b| a.1 == b.1);
    let paths: Vec<PathBuf> = listed.iter().map(|(_, p)| p.clone()).collect();
    let mut report = ScanReport {
        removed: cache.retain_listed(&paths),
        vst2_folders: vst2_only_folders(&req.settings.extra_paths),
        ..ScanReport::default()
    };

    // Decide what to scan.
    let mut jobs: Vec<(PluginFormat, PathBuf, FileStamp)> = Vec::new();
    for (format, path) in &listed {
        let Some(stamp) = FileStamp::of(path) else {
            continue;
        };
        let scan = match (req.mode, cache.blacklisted(path), cache.file(path)) {
            (RescanMode::Full, _, _) => true,
            (_, Some(b), _) => {
                if b.stamp != stamp || req.mode == RescanMode::FailedOnly {
                    true
                } else {
                    report.skipped_blacklisted += 1;
                    false
                }
            }
            (_, None, Some(f)) => {
                if f.stamp == stamp && f.scanner_version == SCANNER_VERSION {
                    report.cached += 1;
                    false
                } else {
                    true
                }
            }
            (_, None, None) => true,
        };
        if scan {
            jobs.push((*format, path.clone(), stamp));
        }
    }
    progress.total.store(jobs.len(), Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);

    // Run them in parallel (see [`parallel_scans`]), collecting by job index.
    let results: Vec<Mutex<Option<JobResult>>> = jobs.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let timeout = req.settings.timeout();
    let cores = std::thread::available_parallelism().map_or(2, std::num::NonZero::get);
    let workers = parallel_scans(cores).min(jobs.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some((format, path, _)) = jobs.get(i) else {
                        return;
                    };
                    let r = runner.scan(*format, path, timeout, cancel);
                    *results[i].lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
                    progress.done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    report.cancelled = cancel.load(Ordering::Relaxed);

    // Apply in job (path) order.
    for (slot, (format, path, stamp)) in results.into_iter().zip(&jobs) {
        let Some(outcome) = slot.into_inner().unwrap_or_else(PoisonError::into_inner) else {
            continue;
        };
        apply_outcome(&mut report, cache, (*format, path, *stamp), outcome);
    }
    Ok(report)
}

/// Records what the scan of one file gave: a cache entry for a file that scanned, a blacklist
/// entry for one that is to blame, nothing for a scan that was cancelled or could not start.
fn apply_outcome(
    report: &mut ScanReport,
    cache: &mut PluginCache,
    (format, path, stamp): (PluginFormat, &Path, FileStamp),
    outcome: JobResult,
) {
    match outcome {
        Ok(result) => {
            report.scanned += 1;
            let problem = match (&result.error, result.plugins.is_empty()) {
                (Some(e), _) => Some(e.clone()),
                (None, true) => Some("the file holds no plugin".to_string()),
                (None, false) => None,
            };
            match problem {
                None => {
                    report.ok += 1;
                    cache.insert_file(CachedFile {
                        path: path.to_path_buf(),
                        format,
                        stamp,
                        scanner_version: SCANNER_VERSION,
                        scanned_secs: crate::cache::now_secs(),
                        plugins: result.plugins,
                    });
                }
                Some(reason) => {
                    report.failed += 1;
                    report.failures.push((path.to_path_buf(), reason.clone()));
                    cache.blacklist_file(path, format, stamp, reason);
                }
            }
        }
        Err(f) if f.kind.blames_file() => {
            report.scanned += 1;
            report.failed += 1;
            report.failures.push((path.to_path_buf(), f.reason.clone()));
            cache.blacklist_file(path, format, stamp, f.reason);
        }
        Err(f) => {
            if f.kind == FailureKind::Spawn && report.spawn_error.is_none() {
                report.spawn_error = Some(f.reason);
            }
        }
    }
}

/// Scans one file again with `timeout` (the user's retry of a file that timed out passes
/// [`NO_TIME_LIMIT`]), updating `cache` the way a rescan does: a file that scans is cached and
/// leaves the blacklist, a file that does not is blacklisted again with the new reason, a
/// cancelled scan changes nothing.
///
/// # Errors
/// [`ScanError::PluginsDisabled`] when the user has not enabled plugins; nothing is touched.
pub fn rescan_file(
    settings: &PluginSettings,
    (format, path): (PluginFormat, &Path),
    timeout: Duration,
    cache: &mut PluginCache,
    runner: &dyn ScanRunner,
    cancel: &AtomicBool,
    progress: &ScanProgress,
) -> Result<ScanReport, ScanError> {
    if !settings.enabled {
        return Err(ScanError::PluginsDisabled);
    }
    let mut report = ScanReport::default();
    let Some(stamp) = FileStamp::of(path) else {
        report.removed = usize::from(cache.forget(path));
        return Ok(report);
    };
    progress.total.store(1, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);
    let outcome = runner.scan(format, path, timeout, cancel);
    progress.done.store(1, Ordering::Relaxed);
    report.cancelled = cancel.load(Ordering::Relaxed);
    apply_outcome(&mut report, cache, (format, path, stamp), outcome);
    Ok(report)
}

/// A rescan running on its own worker thread (the scan never blocks the UI).
pub struct ScanJob {
    handle: std::thread::JoinHandle<(PluginCache, Result<ScanReport, ScanError>)>,
    cancel: std::sync::Arc<AtomicBool>,
    progress: std::sync::Arc<ScanProgress>,
}

/// An environment lookup a scan job owns (`CLAP_PATH`, `HOME`, ...).
pub type ScanEnv = std::sync::Arc<dyn Fn(&str) -> Option<OsString> + Send + Sync>;

impl ScanJob {
    /// Starts a rescan of `cache` (moved into the worker and handed back by [`ScanJob::finish`]),
    /// reading the process environment.
    pub fn start(
        settings: PluginSettings,
        formats: Vec<PluginFormat>,
        os: Os,
        mode: RescanMode,
        cache: PluginCache,
        runner: std::sync::Arc<dyn ScanRunner>,
    ) -> Self {
        let env: ScanEnv = std::sync::Arc::new(|k: &str| std::env::var_os(k));
        Self::start_with_env(settings, formats, os, mode, cache, runner, env)
    }

    /// As [`ScanJob::start`], with the environment lookup given (tests pass one that finds no
    /// default folders, so a scan only visits the folders it is told about).
    pub fn start_with_env(
        settings: PluginSettings,
        formats: Vec<PluginFormat>,
        os: Os,
        mode: RescanMode,
        cache: PluginCache,
        runner: std::sync::Arc<dyn ScanRunner>,
        env: ScanEnv,
    ) -> Self {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let progress = std::sync::Arc::new(ScanProgress::default());
        let (c, p) = (cancel.clone(), progress.clone());
        let handle = std::thread::spawn(move || {
            let mut cache = cache;
            let req = ScanRequest {
                settings: &settings,
                formats: &formats,
                os,
                env: env.as_ref(),
                mode,
            };
            let report = rescan(&req, &mut cache, runner.as_ref(), &c, &p);
            (cache, report)
        });
        Self {
            handle,
            cancel,
            progress,
        }
    }

    /// Starts scanning the one file `path` again with no time limit, on its own worker (see
    /// [`rescan_file`]); [`ScanJob::cancel`] stops it. `cache` is moved into the worker and
    /// handed back by [`ScanJob::finish`].
    pub fn start_file_without_limit(
        settings: PluginSettings,
        (format, path): (PluginFormat, PathBuf),
        cache: PluginCache,
        runner: std::sync::Arc<dyn ScanRunner>,
    ) -> Self {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let progress = std::sync::Arc::new(ScanProgress::default());
        let (c, p) = (cancel.clone(), progress.clone());
        let handle = std::thread::spawn(move || {
            let mut cache = cache;
            let report = rescan_file(
                &settings,
                (format, &path),
                NO_TIME_LIMIT,
                &mut cache,
                runner.as_ref(),
                &c,
                &p,
            );
            (cache, report)
        });
        Self {
            handle,
            cancel,
            progress,
        }
    }

    /// Asks the scan to stop: running children are killed, pending files are not started.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// `(done, total)` so far.
    pub fn progress(&self) -> (usize, usize) {
        self.progress.snapshot()
    }

    /// True when the worker is done and [`ScanJob::finish`] will not block.
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// Waits for the worker and returns the updated cache and the report. A worker that
    /// panicked gives back an empty cache and a `BadResult` error.
    pub fn finish(self) -> (PluginCache, Result<ScanReport, ScanError>) {
        self.handle.join().unwrap_or_else(|_| {
            (
                PluginCache::default(),
                Err(ScanError::BadResult("the scan worker panicked".to_string())),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_reason_is_told_apart_from_a_crash() {
        assert!(is_timeout_reason(&timeout_reason(30)));
        assert!(timeout_reason(30).contains("30 s") && timeout_reason(30).contains("killed"));
        assert!(!is_timeout_reason("the scanner exited with code 3 (0x3)"));
        assert!(!is_timeout_reason("the file holds no plugin"));
    }

    #[test]
    fn scan_events_round_trip_as_single_lines() {
        let plugin = |id: &str| ScannedPlugin {
            plugin: atomik_device_sdk::PluginRef {
                format: PluginFormat::Clap,
                id: id.to_string(),
            },
            layout: atomik_device_sdk::PluginLayout {
                name: "N".to_string(),
                vendor: "V".to_string(),
                version: "1".to_string(),
                kind_hint: atomik_device_sdk::PluginKindHint::Instrument,
                main_in: None,
                main_out: None,
                sidechain: false,
                notes_in: true,
                notes_out: false,
                aux_out: 0,
                params: Vec::new(),
            },
            category: atomik_device_sdk::PluginCategory::from_kind(
                atomik_device_sdk::PluginKindHint::Instrument,
            ),
            unsupported: Some("why".to_string()),
            params_truncated: false,
        };
        for event in [
            ScanEvent::Classes(vec![plugin("a"), plugin("b")]),
            ScanEvent::Class(plugin("a")),
        ] {
            let line = event.to_json();
            assert!(!line.contains('\n'));
            assert_eq!(ScanEvent::from_json(&line, PluginFormat::Clap), Some(event));
        }
        // The result line and noise are not events.
        let result = ScanResult {
            path: PathBuf::from("x.clap"),
            format: PluginFormat::Clap,
            plugins: Vec::new(),
            error: None,
        };
        assert_eq!(
            ScanEvent::from_json(&result.to_json(), PluginFormat::Clap),
            None
        );
        assert_eq!(
            ScanEvent::from_json("{\"class\": 3}", PluginFormat::Clap),
            None
        );
        assert_eq!(ScanEvent::from_json("hello", PluginFormat::Clap), None);
    }
}
