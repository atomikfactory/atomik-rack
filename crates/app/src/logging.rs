//! The app's log: a rotating file in `<config folder>/logs`, written from the `log` facade.
//!
//! Every session writes `atomik.log`; when it reaches [`MAX_LOG_BYTES`] it becomes `atomik.1.log`
//! (the older ones shift up) and at most [`KEEP_FILES`] files are kept. Records of the app's own
//! crates are kept from `info` up, everything else (the GPU and window libraries, ...) from `warn`;
//! `ATOMIK_LOG` (`error`, `warn`, `info`, `debug`, `trace`) changes the level of the app's crates.
//! Every record is also written to stderr, which is what a developer watching a terminal sees.
//!
//! Nothing here runs on the audio thread: the callback never logs. It only counts, and the UI
//! thread reads the counters and writes the records (see [`crate::status::OverloadStats`]).
//!
//! If the folder cannot be created or the file cannot be opened, the log falls back to stderr
//! alone and says why; logging never panics and never fails the app.

use log::{LevelFilter, Log, Metadata, Record};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

/// The name of the current log file.
pub const LOG_FILE: &str = "atomik.log";
/// The size at which the current log file is rotated.
pub const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
/// How many log files are kept, the current one included.
pub const KEEP_FILES: usize = 3;

/// The folder of the logs and the crash reports inside the config folder `config_dir`.
pub fn log_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("logs")
}

/// Where the log of this run goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogTarget {
    /// The rotating file at this path (and stderr).
    File(PathBuf),
    /// Stderr only, because the file could not be opened (the reason), or because a logger was
    /// installed already.
    Stderr(String),
}

/// A size-rotated set of log files in one folder.
#[derive(Debug)]
pub struct RotatingFile {
    dir: PathBuf,
    file: File,
    size: u64,
    max_bytes: u64,
    keep: usize,
}

impl RotatingFile {
    /// Opens (creating the folder and the file when needed) the current log file in `dir`,
    /// appending to what it holds. `keep` files of at most about `max_bytes` each are kept.
    ///
    /// # Errors
    /// The folder or the file cannot be created.
    pub fn open(dir: &Path, max_bytes: u64, keep: usize) -> std::io::Result<Self> {
        fs::create_dir_all(dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(Self::path_of(dir, 0))?;
        let size = file.metadata().map_or(0, |m| m.len());
        Ok(Self {
            dir: dir.to_path_buf(),
            file,
            size,
            max_bytes: max_bytes.max(1),
            keep: keep.max(1),
        })
    }

    /// The path of log file number `index`: 0 is the current one (`atomik.log`), then
    /// `atomik.1.log`, `atomik.2.log`.
    pub fn path_of(dir: &Path, index: usize) -> PathBuf {
        if index == 0 {
            dir.join(LOG_FILE)
        } else {
            dir.join(format!("atomik.{index}.log"))
        }
    }

    /// Appends `line` (which carries its own newline), rotating first when the file would grow
    /// past the limit. A failed write is dropped: a full disk must not stop the app.
    pub fn write_line(&mut self, line: &str) {
        let len = line.len() as u64;
        if self.size > 0 && self.size + len > self.max_bytes {
            self.rotate();
        }
        if self.file.write_all(line.as_bytes()).is_ok() {
            self.size += len;
        }
    }

    /// Shifts every file up by one (the oldest is deleted) and starts an empty current file.
    fn rotate(&mut self) {
        let _ = self.file.flush();
        let _ = fs::remove_file(Self::path_of(&self.dir, self.keep - 1));
        for index in (0..self.keep - 1).rev() {
            let _ = fs::rename(
                Self::path_of(&self.dir, index),
                Self::path_of(&self.dir, index + 1),
            );
        }
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(Self::path_of(&self.dir, 0))
        {
            self.file = file;
            self.size = 0;
        }
        // On failure the old handle keeps appending to the file that was renamed: the records
        // are not lost, only the rotation is.
    }
}

/// The logger: a file (when it could be opened) and stderr.
#[derive(Debug)]
pub struct Logger {
    file: Mutex<Option<RotatingFile>>,
    own_level: LevelFilter,
}

impl Logger {
    /// A logger writing to `file` (stderr alone when `None`), keeping the app's own crates from
    /// `own_level` up and the others from `warn`.
    pub fn new(file: Option<RotatingFile>, own_level: LevelFilter) -> Self {
        Self {
            file: Mutex::new(file),
            own_level,
        }
    }

    fn level_for(&self, target: &str) -> LevelFilter {
        if target.starts_with("atomik") {
            self.own_level
        } else {
            self.own_level.min(LevelFilter::Warn)
        }
    }

    /// The text of one record: `2026-10-29T12:34:56Z WARN  target message`.
    fn line(record: &Record<'_>) -> String {
        format!(
            "{} {:<5} {} {}\n",
            utc_stamp(unix_seconds()),
            record.level(),
            record.target(),
            record.args()
        )
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level_for(metadata.target())
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = Self::line(record);
        // Not `eprintln!`: that panics when stderr is closed, and a log must never do that.
        let _ = std::io::stderr().write_all(line.as_bytes());
        if let Some(file) = self
            .file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            file.write_line(&line);
        }
    }

    fn flush(&self) {
        if let Some(file) = self
            .file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            let _ = file.file.flush();
        }
    }
}

static LOGGER: OnceLock<Logger> = OnceLock::new();
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Installs the logger once, writing to `<dir>/atomik.log`. Falls back to stderr when the file
/// cannot be opened (the returned target says so, and the reason is the first record), and
/// does nothing when a logger is installed already (a second call, or a test).
pub fn init(dir: &Path) -> LogTarget {
    let level = std::env::var("ATOMIK_LOG")
        .ok()
        .and_then(|v| v.trim().parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::Info);
    let (file, target, problem) = match RotatingFile::open(dir, MAX_LOG_BYTES, KEEP_FILES) {
        Ok(file) => (
            Some(file),
            LogTarget::File(RotatingFile::path_of(dir, 0)),
            None,
        ),
        Err(e) => {
            let reason = format!("cannot open the log in {}: {e}", dir.display());
            (None, LogTarget::Stderr(reason.clone()), Some(reason))
        }
    };
    let logger = LOGGER.get_or_init(|| Logger::new(file, level));
    if log::set_logger(logger).is_err() {
        return LogTarget::Stderr("a logger is installed already".to_string());
    }
    INSTALLED.store(true, Ordering::Relaxed);
    log::set_max_level(level);
    log::info!(
        "atomik {} on {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    if let Some(reason) = problem {
        log::warn!("{reason}; logging to stderr only");
    }
    target
}

/// Whether the logger of this module is installed.
pub fn is_installed() -> bool {
    INSTALLED.load(Ordering::Relaxed)
}

/// Reports an error that ends the run: a record when the logger is installed, else a line on
/// stderr (the command-line tools such as `--print-keymap` run before the log starts).
pub fn fatal(message: &str) {
    if is_installed() {
        log::error!("{message}");
    } else {
        let _ = writeln!(std::io::stderr(), "atomik: {message}");
    }
}

/// The current time in seconds since the Unix epoch.
// Justification: names log lines and crash reports for the user; UI and worker threads only,
// never the engine or anything rendered.
#[allow(clippy::disallowed_methods)]
pub fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `2026-10-29T12:34:56Z` for `secs` since the Unix epoch (UTC).
pub fn utc_stamp(secs: u64) -> String {
    let (y, m, d, hh, mm, ss) = civil(secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// `20261029-123456` for `secs`: the form that names files.
pub fn file_stamp(secs: u64) -> String {
    let (y, m, d, hh, mm, ss) = civil(secs);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

/// Year, month, day, hour, minute and second (UTC) of `secs` since the Unix epoch.
fn civil(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rest = (secs % 86_400) as u32;
    // Days to a civil date (Howard Hinnant's algorithm), proleptic Gregorian.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, rest / 3600, rest / 60 % 60, rest % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("atomik-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn writing_past_the_limit_rotates_and_keeps_three_files() {
        let dir = scratch("rotate");
        let mut log = RotatingFile::open(&dir, 100, KEEP_FILES).unwrap();
        for i in 0..40 {
            log.write_line(&format!("line {i:02} {}\n", "x".repeat(20)));
        }
        let mut names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["atomik.1.log", "atomik.2.log", "atomik.log"]);
        for index in 0..KEEP_FILES {
            let len = fs::metadata(RotatingFile::path_of(&dir, index))
                .unwrap()
                .len();
            assert!(len <= 100, "file {index} is {len} bytes");
        }
        // Newest first: the current file ends with the last line written.
        let current = fs::read_to_string(RotatingFile::path_of(&dir, 0)).unwrap();
        assert!(current.contains("line 39"), "{current}");
        let oldest = fs::read_to_string(RotatingFile::path_of(&dir, 2)).unwrap();
        let newest_of_old = fs::read_to_string(RotatingFile::path_of(&dir, 1)).unwrap();
        assert!(oldest < newest_of_old, "older lines are in the older file");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reopened_log_appends_and_counts_what_it_holds() {
        let dir = scratch("reopen");
        RotatingFile::open(&dir, 1000, 3)
            .unwrap()
            .write_line("first\n");
        let mut again = RotatingFile::open(&dir, 1000, 3).unwrap();
        assert_eq!(again.size, 6);
        again.write_line("second\n");
        assert_eq!(
            fs::read_to_string(RotatingFile::path_of(&dir, 0)).unwrap(),
            "first\nsecond\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_that_cannot_be_made_is_an_error_not_a_panic() {
        let dir = scratch("blocked");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("not-a-folder");
        fs::write(&file, "x").unwrap();
        assert!(RotatingFile::open(&file.join("logs"), MAX_LOG_BYTES, KEEP_FILES).is_err());
        // The logger itself works without a file: stderr only.
        let logger = Logger::new(None, LevelFilter::Info);
        logger.log(
            &Record::builder()
                .args(format_args!("to stderr only"))
                .level(Level::Warn)
                .target("atomik_app")
                .build(),
        );
        logger.flush();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_apps_crates_log_from_info_and_the_others_from_warn() {
        let logger = Logger::new(None, LevelFilter::Info);
        let enabled = |target: &str, level| {
            logger.enabled(&Metadata::builder().target(target).level(level).build())
        };
        assert!(enabled("atomik_app::shell", Level::Info));
        assert!(!enabled("atomik_app::shell", Level::Debug));
        assert!(!enabled("wgpu_core", Level::Info));
        assert!(enabled("wgpu_core", Level::Warn));
    }

    #[test]
    fn the_logger_writes_a_stamped_line_per_record() {
        let dir = scratch("lines");
        let file = RotatingFile::open(&dir, MAX_LOG_BYTES, KEEP_FILES).unwrap();
        let logger = Logger::new(Some(file), LevelFilter::Info);
        logger.log(
            &Record::builder()
                .args(format_args!("hello {}", 7))
                .level(Level::Warn)
                .target("atomik_app")
                .build(),
        );
        logger.log(
            &Record::builder()
                .args(format_args!("too chatty"))
                .level(Level::Debug)
                .target("atomik_app")
                .build(),
        );
        let text = fs::read_to_string(RotatingFile::path_of(&dir, 0)).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        let line = text.lines().next().unwrap();
        assert!(line.ends_with("WARN  atomik_app hello 7"), "{line}");
        assert!(line.as_bytes()[4] == b'-' && line.contains('T'), "{line}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stamps_are_utc_calendar_times() {
        assert_eq!(utc_stamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_stamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_stamp(1_793_277_296), "2026-10-29T12:34:56Z");
        assert_eq!(file_stamp(1_793_277_296), "20261029-123456");
    }
}
