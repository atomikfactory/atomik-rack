//! The real scan runner: one `atomik-plugin-scan` child process per plugin file.
//!
//! The child loads the file; this process never does. The parent reads the child's stdout and
//! stderr on helper threads (so a chatty plugin cannot fill a pipe and stall the child), polls
//! the child, and kills it when the timeout passes or the scan is cancelled. A non-zero exit
//! code or a signal is a crash; the last line of stdout that starts with `{` is the JSON
//! result (plugins sometimes print to stdout while they initialise).

use crate::scan::{FailureKind, ScanFailure, ScanResult, ScanRunner};
use atomik_device_sdk::PluginFormat;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Most bytes of stdout the parent accepts from one scan (a 2 048-param plugin needs a few hundred KB).
pub const MAX_SCAN_OUTPUT: usize = 16 * 1024 * 1024;
/// How much of the child's stderr goes into a blacklist reason.
pub const STDERR_REASON_BYTES: usize = 2048;

/// The program to start for a scan, with the arguments that come before `<format> <path>` and
/// extra environment variables (the app passes just the path to `atomik-plugin-scan`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannerCommand {
    /// The scanner executable.
    pub program: PathBuf,
    /// Arguments before the format and the file.
    pub args: Vec<OsString>,
    /// Environment variables set for the child.
    pub envs: Vec<(OsString, OsString)>,
}

impl ScannerCommand {
    /// A command that is just the scanner program.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            envs: Vec::new(),
        }
    }
}

/// Runs scans in child processes.
#[derive(Debug)]
pub struct ChildScanner {
    command: ScannerCommand,
    spawned: AtomicUsize,
}

impl ChildScanner {
    /// A runner that starts `command` for each file.
    pub fn new(command: ScannerCommand) -> Self {
        Self {
            command,
            spawned: AtomicUsize::new(0),
        }
    }

    /// How many children this runner has started (a cache hit starts none).
    pub fn spawned(&self) -> usize {
        self.spawned.load(Ordering::SeqCst)
    }
}

fn failure(kind: FailureKind, reason: impl Into<String>) -> ScanFailure {
    ScanFailure {
        kind,
        reason: reason.into(),
    }
}

/// What a reader thread keeps of a stream, and whether the stream was longer.
struct Captured {
    bytes: mpsc::Receiver<Vec<u8>>,
    /// Set as soon as the stream is known to be longer than the cap.
    over: Arc<AtomicBool>,
}

/// Keeps the first `keep` bytes of `source` and reads (and drops) the rest to its end, so that
/// a chatty child never blocks on a full pipe; `over` tells the caller that there was more.
fn reader(mut source: impl Read + Send + 'static, keep: usize) -> Captured {
    let (tx, rx) = mpsc::channel();
    let over = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&over);
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buf = [0u8; 8192];
        let mut total = 0usize;
        loop {
            match source.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    total = total.saturating_add(n);
                    if kept.len() < keep {
                        let room = keep - kept.len();
                        kept.extend_from_slice(&buf[..n.min(room)]);
                    }
                    if total > keep {
                        flag.store(true, Ordering::Release);
                    }
                }
            }
        }
        let _ = tx.send(kept);
    });
    Captured { bytes: rx, over }
}

fn describe_exit(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return format!("the scanner was killed by signal {sig}");
        }
    }
    match status.code() {
        Some(code) => format!("the scanner exited with code {code} ({:#x})", code as u32),
        None => "the scanner ended abnormally".to_string(),
    }
}

fn stderr_text(rx: &Captured) -> String {
    let bytes = rx
        .bytes
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(STDERR_REASON_BYTES)]);
    text.trim().to_string()
}

// Justification: the scan timeout is wall-clock time on the scan worker thread, not engine time.
#[allow(clippy::disallowed_methods)]
impl ScanRunner for ChildScanner {
    fn scan(
        &self,
        format: PluginFormat,
        path: &Path,
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<ScanResult, ScanFailure> {
        if cancel.load(Ordering::Relaxed) {
            return Err(failure(FailureKind::Cancelled, "cancelled"));
        }
        let mut cmd = Command::new(&self.command.program);
        cmd.args(&self.command.args)
            .arg(format.name())
            .arg(path)
            .envs(self.command.envs.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console flashes up for every scanned file.
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().map_err(|e| {
            failure(
                FailureKind::Spawn,
                format!("cannot start {}: {e}", self.command.program.display()),
            )
        })?;
        self.spawned.fetch_add(1, Ordering::SeqCst);
        let out_rx = child
            .stdout
            .take()
            .map(|s| reader(s, MAX_SCAN_OUTPUT))
            .ok_or_else(|| failure(FailureKind::Spawn, "no stdout pipe"))?;
        let err_rx = child
            .stderr
            .take()
            .map(|s| reader(s, STDERR_REASON_BYTES))
            .ok_or_else(|| failure(FailureKind::Spawn, "no stderr pipe"))?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if cancel.load(Ordering::Relaxed) {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(failure(FailureKind::Cancelled, "cancelled"));
                    }
                    if out_rx.over.load(Ordering::Acquire) {
                        // Whatever it goes on to print is garbage: do not wait for it.
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(failure(
                            FailureKind::BadOutput,
                            "the scan output is too large",
                        ));
                    }
                    if started.elapsed() >= timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(failure(
                            FailureKind::Timeout,
                            format!(
                                "did not finish within {} s; the scanner was killed",
                                timeout.as_secs()
                            ),
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(failure(FailureKind::Spawn, format!("waiting failed: {e}")));
                }
            }
        };
        let stderr = stderr_text(&err_rx);
        if !status.success() {
            let mut reason = describe_exit(status);
            if !stderr.is_empty() {
                reason.push_str(": ");
                reason.push_str(&stderr);
            }
            return Err(failure(FailureKind::Crash, reason));
        }
        let stdout = out_rx
            .bytes
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_default();
        if stdout.len() >= MAX_SCAN_OUTPUT {
            return Err(failure(
                FailureKind::BadOutput,
                "the scan output is too large",
            ));
        }
        let text = String::from_utf8_lossy(&stdout);
        let line = text
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| l.starts_with('{'))
            .ok_or_else(|| failure(FailureKind::BadOutput, "the scanner printed no result"))?;
        let mut result = ScanResult::from_json(line)
            .map_err(|e| failure(FailureKind::BadOutput, e.to_string()))?;
        result.path = path.to_path_buf();
        Ok(result)
    }
}
