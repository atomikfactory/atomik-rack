//! `plugins.json`: the scan cache and the blacklist.
//!
//! Per scanned file the cache keeps the path, size, modification time (nanoseconds), the scanner
//! version and the plugins with their layouts; the blacklist keeps the files whose scan crashed,
//! hung or failed, with the reason and the date. The app reads it at start and registers
//! descriptors from it without loading any plugin. A cache that is missing, corrupt or of
//! another version is ignored and rebuilt by the next scan; it is written atomically (temp file,
//! fsync, rename), so a crash while saving never damages the old file.

use crate::backend::ScannedPlugin;
use crate::wire::{BlacklistDto, CacheDto, FileDto, PluginDto, format_name, parse_format};
use atomik_device_sdk::PluginFormat;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The schema version of `plugins.json`. A file of another version is rebuilt.
pub const CACHE_VERSION: u32 = 1;

/// The version of the scanner's output. Entries scanned by another version are rescanned.
pub const SCANNER_VERSION: u32 = 1;

/// Size and modification time of a file: what decides whether a cached scan is still valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    /// Size in bytes (a bundle folder: the total of the files inside it).
    pub size: u64,
    /// Modification time, nanoseconds since the Unix epoch (saturating).
    pub mtime_ns: i64,
}

/// The most files of a bundle folder that count towards its stamp.
const STAMP_MAX_FILES: usize = 512;
/// The deepest folder level of a bundle that counts towards its stamp.
const STAMP_MAX_DEPTH: usize = 6;

fn nanos(t: std::time::SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_nanos()).map_or(i64::MIN, |n| -n),
    }
}

/// Adds up the files below `dir` (a bundle): total size, newest file time.
fn fold_dir(dir: &Path, depth: usize, left: &mut usize, size: &mut u64, newest: &mut i64) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for path in entries {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            if depth < STAMP_MAX_DEPTH {
                fold_dir(&path, depth + 1, left, size, newest);
            }
        } else if *left > 0 {
            *left -= 1;
            *size = size.saturating_add(meta.len());
            if let Ok(t) = meta.modified() {
                *newest = (*newest).max(nanos(t));
            }
        }
    }
}

impl FileStamp {
    /// The stamp of `path` now, `None` when it cannot be read. A bundle folder is stamped by the
    /// files inside it (their total size and newest time, up to a bound), because replacing the
    /// binary of a bundle does not change the folder's own time.
    pub fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let folder_ns = nanos(meta.modified().ok()?);
        if meta.is_file() {
            return Some(Self {
                size: meta.len(),
                mtime_ns: folder_ns,
            });
        }
        // Only the files count (a copy that keeps an older file time must still be seen); an
        // empty folder falls back to its own time.
        let (mut size, mut newest, mut left) = (0, i64::MIN, STAMP_MAX_FILES);
        fold_dir(path, 0, &mut left, &mut size, &mut newest);
        Some(Self {
            size,
            mtime_ns: if newest == i64::MIN {
                folder_ns
            } else {
                newest
            },
        })
    }
}

/// One successfully scanned file.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedFile {
    /// The plugin file or bundle.
    pub path: PathBuf,
    /// Its format.
    pub format: PluginFormat,
    /// Size and time when it was scanned.
    pub stamp: FileStamp,
    /// The scanner version that produced the entry.
    pub scanner_version: u32,
    /// The plugins found in it (unsupported ones included, with their reason).
    pub plugins: Vec<ScannedPlugin>,
}

/// A file that crashed, hung or failed when scanned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlacklistEntry {
    /// The plugin file or bundle.
    pub path: PathBuf,
    /// Its format.
    pub format: PluginFormat,
    /// Size and time when it failed; a change of either retries the file.
    pub stamp: FileStamp,
    /// Why (an exit code, the timeout, the error the scanner reported, the first 2 KiB of stderr).
    pub reason: String,
    /// When, in seconds since the Unix epoch.
    pub date_secs: u64,
}

/// How a cache file was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadStatus {
    /// Read as it is.
    Loaded,
    /// There was no file: an empty cache.
    Missing,
    /// The file was corrupt or of another version and is ignored (the reason): an empty cache.
    Rebuilt(String),
}

/// The scan cache and the blacklist.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PluginCache {
    files: BTreeMap<PathBuf, CachedFile>,
    blacklist: BTreeMap<PathBuf, BlacklistEntry>,
}

// Justification: the blacklist records the wall-clock date of a failure for the user; this runs on
// the main or scan thread, never on an engine thread (is about the audio path).
#[allow(clippy::disallowed_methods)]
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn path_text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

impl PluginCache {
    /// The scanned files in path order.
    pub fn files(&self) -> impl Iterator<Item = &CachedFile> {
        self.files.values()
    }

    /// One scanned file.
    pub fn file(&self, path: &Path) -> Option<&CachedFile> {
        self.files.get(path)
    }

    /// The blacklisted files in path order.
    pub fn blacklist(&self) -> impl Iterator<Item = &BlacklistEntry> {
        self.blacklist.values()
    }

    /// One blacklist entry.
    pub fn blacklisted(&self, path: &Path) -> Option<&BlacklistEntry> {
        self.blacklist.get(path)
    }

    /// Records a successful scan (and clears a blacklist entry of the same file).
    pub fn insert_file(&mut self, file: CachedFile) {
        self.blacklist.remove(&file.path);
        self.files.insert(file.path.clone(), file);
    }

    /// Blacklists `path` with a reason (and drops its cached scan).
    pub fn blacklist_file(
        &mut self,
        path: &Path,
        format: PluginFormat,
        stamp: FileStamp,
        reason: impl Into<String>,
    ) {
        self.files.remove(path);
        self.blacklist.insert(
            path.to_path_buf(),
            BlacklistEntry {
                path: path.to_path_buf(),
                format,
                stamp,
                reason: reason.into(),
                date_secs: now_secs(),
            },
        );
    }

    /// Removes a blacklist entry (the user's choice). Returns whether there was one.
    pub fn unblacklist(&mut self, path: &Path) -> bool {
        self.blacklist.remove(path).is_some()
    }

    /// Keeps only the entries whose path is in `listed` (sorted or not); returns how many
    /// entries (cached files and blacklist entries) were dropped.
    pub fn retain_listed(&mut self, listed: &[PathBuf]) -> usize {
        let before = self.files.len() + self.blacklist.len();
        self.files.retain(|p, _| listed.contains(p));
        self.blacklist.retain(|p, _| listed.contains(p));
        before - self.files.len() - self.blacklist.len()
    }

    /// Test hook: pretends every cached entry was made by scanner `version`.
    #[doc(hidden)]
    pub fn set_scanner_version_for_test(&mut self, version: u32) {
        for f in self.files.values_mut() {
            f.scanner_version = version;
        }
    }

    /// The cache as JSON text (deterministic: files and blacklist in path order).
    pub fn to_json(&self) -> String {
        let dto = CacheDto {
            version: CACHE_VERSION,
            files: self
                .files
                .values()
                .map(|f| FileDto {
                    path: path_text(&f.path),
                    format: format_name(f.format),
                    size: f.stamp.size,
                    mtime_ns: f.stamp.mtime_ns,
                    scanner_version: f.scanner_version,
                    plugins: f.plugins.iter().map(PluginDto::from_scanned).collect(),
                })
                .collect(),
            blacklist: self
                .blacklist
                .values()
                .map(|b| BlacklistDto {
                    path: path_text(&b.path),
                    format: format_name(b.format),
                    size: b.stamp.size,
                    mtime_ns: b.stamp.mtime_ns,
                    reason: b.reason.clone(),
                    date: b.date_secs,
                })
                .collect(),
        };
        let mut text = serde_json::to_string_pretty(&dto).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Parses cache text.
    ///
    /// # Errors
    /// The reason, when the text is not a cache of this version.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let dto: CacheDto = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if dto.version != CACHE_VERSION {
            return Err(format!(
                "cache version {} (this build reads {CACHE_VERSION})",
                dto.version
            ));
        }
        let mut cache = Self::default();
        for f in dto.files {
            let format = parse_format(&f.format)?;
            let plugins = f
                .plugins
                .into_iter()
                .map(|p| p.into_scanned(format))
                .collect::<Result<Vec<_>, _>>()?;
            let path = PathBuf::from(f.path);
            cache.files.insert(
                path.clone(),
                CachedFile {
                    path,
                    format,
                    stamp: FileStamp {
                        size: f.size,
                        mtime_ns: f.mtime_ns,
                    },
                    scanner_version: f.scanner_version,
                    plugins,
                },
            );
        }
        for b in dto.blacklist {
            let path = PathBuf::from(b.path);
            cache.blacklist.insert(
                path.clone(),
                BlacklistEntry {
                    path,
                    format: parse_format(&b.format)?,
                    stamp: FileStamp {
                        size: b.size,
                        mtime_ns: b.mtime_ns,
                    },
                    reason: b.reason,
                    date_secs: b.date,
                },
            );
        }
        Ok(cache)
    }

    /// Reads the cache at `path`. Never fails: a missing, corrupt or old file gives an empty
    /// cache and says why.
    pub fn load(path: &Path) -> (Self, LoadStatus) {
        let text = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), LoadStatus::Missing);
            }
            Err(e) => return (Self::default(), LoadStatus::Rebuilt(e.to_string())),
        };
        match String::from_utf8(text)
            .map_err(|e| e.to_string())
            .and_then(|t| Self::from_json(&t))
        {
            Ok(cache) => (cache, LoadStatus::Loaded),
            Err(reason) => (Self::default(), LoadStatus::Rebuilt(reason)),
        }
    }

    /// Writes the cache to `path` atomically: a temp file next to it (`<name>.tmp`) is written
    /// and synced, then renamed over the target. On any failure the temp file is removed and
    /// the old file is untouched.
    ///
    /// # Errors
    /// The I/O error.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let result = (|| {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(self.to_json().as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}
