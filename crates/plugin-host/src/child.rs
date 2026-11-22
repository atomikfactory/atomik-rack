//! The real scan runner: `atomik-plugin-scan` child processes, per plugin file and per class.
//!
//! The child loads the file; this process never does. The child announces the classes of the file
//! and prints each class as it finishes it, then a final result line (plugins sometimes print to
//! stdout while they initialise, so lines that are not ours are ignored). The parent reads the
//! child's stdout and stderr on helper threads (so a chatty plugin cannot fill a pipe and stall
//! the child), polls the child, and kills it when a class takes longer than the timeout or the
//! scan is cancelled. A non-zero exit code or a signal is a crash.
//!
//! When the child hangs or crashes inside one class, the classes that answered are kept, that one
//! class is marked as not having answered, and the scanner is started again for the classes it
//! had not reached. Every child lives in a group ([`crate::ScanGroup`]) that is ended with it, so
//! a helper process a plugin started does not outlive the scan, and runs below normal priority.

use crate::backend::ScannedPlugin;
use crate::procgroup::{self, ScanGroup};
use crate::scan::{FailureKind, ScanEvent, ScanFailure, ScanResult, ScanRunner, timeout_reason};
use atomik_device_sdk::PluginFormat;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Most bytes of stdout the parent accepts from one scan (a 2 048-param plugin needs a few hundred KB).
pub const MAX_SCAN_OUTPUT: usize = 16 * 1024 * 1024;
/// How much of the child's stderr goes into a blacklist reason.
pub const STDERR_REASON_BYTES: usize = 2048;
/// How many times one file's scan is started again after a class hung or crashed the scanner.
/// Each restart leaves out the classes already known, so a file of a hundred classes with a few
/// bad ones is finished; a file in which everything is bad is not chased beyond this.
pub const MAX_CLASS_RESTARTS: usize = 8;
/// How the scanner's progress lines begin: the announced classes (`{"classes":`) and each finished
/// class (`{"class":`).
const PROGRESS_LINE: &[u8] = b"{\"class";
/// The scanner argument that names a class id to leave out (repeatable, after `<format> <path>`).
pub const SKIP_FLAG: &str = "--skip";

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
    /// How many of the scanner's own progress lines (`{"class...`) the stream has had so far: its
    /// sign of life. A plugin that prints a lot while it hangs is not a sign of life.
    lines: Arc<AtomicUsize>,
}

/// Keeps the first `keep` bytes of `source` and reads (and drops) the rest to its end, so that
/// a chatty child never blocks on a full pipe; `over` tells the caller that there was more.
fn reader(mut source: impl Read + Send + 'static, keep: usize) -> Captured {
    let (tx, rx) = mpsc::channel();
    let over = Arc::new(AtomicBool::new(false));
    let lines = Arc::new(AtomicUsize::new(0));
    let flag = Arc::clone(&over);
    let counter = Arc::clone(&lines);
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buf = [0u8; 8192];
        let mut total = 0usize;
        let mut head = LineHead::default();
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
                    let progress = head.feed(&buf[..n]);
                    if progress > 0 {
                        counter.fetch_add(progress, Ordering::Release);
                    }
                }
            }
        }
        let _ = tx.send(kept);
    });
    Captured {
        bytes: rx,
        over,
        lines,
    }
}

/// Recognises the scanner's progress lines in a byte stream: a line that begins with
/// [`PROGRESS_LINE`]. Keeps only the first bytes of the current line, across reads.
#[derive(Default)]
struct LineHead {
    head: Vec<u8>,
}

impl LineHead {
    /// Feeds the next bytes; returns how many progress lines ended in them.
    fn feed(&mut self, bytes: &[u8]) -> usize {
        let mut found = 0;
        for &b in bytes {
            if b == b'\n' {
                if self.head.starts_with(PROGRESS_LINE) {
                    found += 1;
                }
                self.head.clear();
            } else if self.head.len() < PROGRESS_LINE.len() {
                self.head.push(b);
            }
        }
        found
    }
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

/// How a scanner child ended.
enum End {
    /// Exit code 0.
    Clean,
    /// A non-zero exit code or a signal (the reason).
    Crash(String),
    /// No line for the whole time limit (the reason).
    Timeout(String),
    /// More output than [`MAX_SCAN_OUTPUT`].
    TooLarge,
}

/// One scanner child, run to its end.
struct Run {
    /// What it printed: all of it for a child that ended by itself, what came before the kill
    /// otherwise.
    stdout: String,
    end: End,
}

// Justification: the scan timeout is wall-clock time on the scan worker thread, not engine time.
#[allow(clippy::disallowed_methods)]
impl ChildScanner {
    /// Starts one scanner child for `path` (leaving out the classes in `skip`) and waits for it.
    ///
    /// The time limit is per class: the clock starts when the child does and starts again each
    /// time the child prints a line (it prints one when it knows the classes of the file and one
    /// as each class is done), so a file with many classes is not judged by one fixed limit,
    /// while a class that does not answer is still given up on after `timeout`.
    ///
    /// # Errors
    /// Only what is not the file's doing: the child could not be started, or the scan was
    /// cancelled. A crash, a timeout and the like are in the returned [`Run`].
    fn run_once(
        &self,
        format: PluginFormat,
        path: &Path,
        skip: &[String],
        timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<Run, ScanFailure> {
        let mut cmd = Command::new(&self.command.program);
        cmd.args(&self.command.args)
            .arg(format.name())
            .arg(path)
            .envs(self.command.envs.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for id in skip {
            cmd.arg(SKIP_FLAG).arg(id);
        }
        // No window, below normal priority, and a group of its own that dies as one.
        procgroup::prepare(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| {
            failure(
                FailureKind::Spawn,
                format!("cannot start {}: {e}", self.command.program.display()),
            )
        })?;
        self.spawned.fetch_add(1, Ordering::SeqCst);
        // From here on, however this function ends, nothing the child started outlives it.
        let group = ScanGroup::attach(&child);
        // Every end of this function signals the group first and reaps the child after: on Unix
        // the group id is safe to signal only while the leader is not reaped (see `procgroup`).
        let end_run = |child: &mut Child| {
            group.kill();
            let _ = child.kill();
            let _ = child.wait();
        };
        let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
            end_run(&mut child);
            return Err(failure(FailureKind::Spawn, "no output pipe"));
        };
        let out_rx = reader(out, MAX_SCAN_OUTPUT);
        let err_rx = reader(err, STDERR_REASON_BYTES);
        let mut window_start = Instant::now();
        let mut lines_seen = 0usize;
        let end = loop {
            match procgroup::leader_exited(&mut child) {
                Ok(true) => {
                    // The helpers a plugin started die with the scanner, so that the pipes close:
                    // signal the group while the leader is still unreaped, then reap it.
                    group.kill();
                    let status = match child.wait() {
                        Ok(status) => status,
                        Err(e) => {
                            return Err(failure(
                                FailureKind::Spawn,
                                format!("waiting failed: {e}"),
                            ));
                        }
                    };
                    if status.success() {
                        break End::Clean;
                    }
                    let mut reason = describe_exit(status);
                    let stderr = stderr_text(&err_rx);
                    if !stderr.is_empty() {
                        reason.push_str(": ");
                        reason.push_str(&stderr);
                    }
                    break End::Crash(reason);
                }
                Ok(false) => {
                    if cancel.load(Ordering::Relaxed) {
                        end_run(&mut child);
                        return Err(failure(FailureKind::Cancelled, "cancelled"));
                    }
                    if out_rx.over.load(Ordering::Acquire) {
                        // Whatever it goes on to print is garbage: do not wait for it.
                        end_run(&mut child);
                        break End::TooLarge;
                    }
                    let lines = out_rx.lines.load(Ordering::Acquire);
                    if lines != lines_seen {
                        lines_seen = lines;
                        window_start = Instant::now();
                    }
                    if window_start.elapsed() >= timeout {
                        end_run(&mut child);
                        break End::Timeout(timeout_reason(timeout.as_secs()));
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => {
                    end_run(&mut child);
                    return Err(failure(FailureKind::Spawn, format!("waiting failed: {e}")));
                }
            }
        };
        let stdout = out_rx
            .bytes
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_default();
        Ok(Run {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            end,
        })
    }
}

/// What a class of the file has come to so far.
enum Standing {
    /// Announced, not answered yet.
    Pending,
    /// Scanned (the plugin is the scanner's answer).
    Answered,
    /// Did not answer: it hung or crashed the scanner, or the scan gave up before it (the reason).
    Failed(String),
}

/// What a scan of one file has learnt, class by class, over all its attempts.
#[derive(Default)]
struct Classes {
    /// In the order the file declares them.
    slots: Vec<(ScannedPlugin, Standing)>,
    /// The ids of the classes the next attempt leaves out (answered or failed).
    skip: Vec<String>,
}

impl Classes {
    fn position(&self, id: &str) -> Option<usize> {
        self.slots.iter().position(|(p, _)| p.plugin.id == id)
    }

    /// Takes in the lines of one attempt: the announced classes (new ones are appended) and the
    /// answered ones. Returns the file's final result line, when the attempt printed one.
    fn absorb(&mut self, format: PluginFormat, stdout: &str) -> Option<ScanResult> {
        let mut result = None;
        for line in stdout.lines().map(str::trim).filter(|l| l.starts_with('{')) {
            match ScanEvent::from_json(line, format) {
                Some(ScanEvent::Classes(all)) => {
                    for p in all {
                        if self.position(&p.plugin.id).is_none() {
                            self.slots.push((p, Standing::Pending));
                        }
                    }
                }
                Some(ScanEvent::Class(p)) => self.answer(p),
                None => {
                    if let Ok(r) = ScanResult::from_json(line) {
                        result = Some(r);
                    }
                }
            }
        }
        result
    }

    fn answer(&mut self, p: ScannedPlugin) {
        self.skip.push(p.plugin.id.clone());
        match self.position(&p.plugin.id) {
            Some(i) => self.slots[i] = (p, Standing::Answered),
            None => self.slots.push((p, Standing::Answered)),
        }
    }

    fn is_answered(&self, id: &str) -> bool {
        self.position(id)
            .is_some_and(|i| matches!(self.slots[i].1, Standing::Answered))
    }

    fn answered(&self) -> usize {
        self.slots
            .iter()
            .filter(|(_, s)| matches!(s, Standing::Answered))
            .count()
    }

    /// Marks the first class that has not answered as the one that did not, and leaves it out of
    /// the next attempt. False when there is none.
    fn blame_first_pending(&mut self, reason: &str) -> bool {
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|(_, s)| matches!(s, Standing::Pending))
        else {
            return false;
        };
        slot.1 = Standing::Failed(reason.to_string());
        self.skip.push(slot.0.plugin.id.clone());
        true
    }

    fn pending(&self) -> bool {
        self.slots
            .iter()
            .any(|(_, s)| matches!(s, Standing::Pending))
    }

    /// The classes as the cache keeps them: the answers, and for each class that did not answer
    /// the announced plugin marked unsupported with the reason.
    fn into_plugins(self) -> Vec<ScannedPlugin> {
        self.slots
            .into_iter()
            .map(|(mut p, s)| {
                match s {
                    Standing::Answered => {}
                    Standing::Failed(reason) => p.unsupported = Some(reason),
                    Standing::Pending => {
                        p.unsupported = Some("not scanned: the scan was given up".to_string());
                    }
                }
                p
            })
            .collect()
    }
}

impl ScanRunner for ChildScanner {
    /// Scans `path` in child processes, class by class. The scanner announces the classes of
    /// the file and prints each class as it finishes it, so when the child hangs or crashes in
    /// one class the parent keeps the classes that answered, marks the class that did not as
    /// unsupported, and starts the scanner again for the classes it has not visited (at most
    /// [`MAX_CLASS_RESTARTS`] times; a file in which two classes fail before any answers is
    /// given up on). The file as a whole fails, as it always did, only when no class answered.
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
        let mut classes = Classes::default();
        let mut first_failure: Option<ScanFailure> = None;
        let mut failures = 0usize;
        for restart in 0..=MAX_CLASS_RESTARTS {
            let run = self.run_once(format, path, &classes.skip, timeout, cancel)?;
            let result = classes.absorb(format, &run.stdout);
            let (kind, reason) = match run.end {
                End::Clean => return finish_clean(path, format, classes, result),
                End::TooLarge => {
                    return Err(failure(
                        FailureKind::BadOutput,
                        "the scan output is too large",
                    ));
                }
                End::Crash(reason) => (FailureKind::Crash, reason),
                End::Timeout(reason) => (FailureKind::Timeout, reason),
            };
            let this = failure(kind, reason);
            let first = first_failure.get_or_insert_with(|| this.clone()).clone();
            failures += 1;
            let class_reason = if kind == FailureKind::Timeout {
                format!(
                    "did not answer within {} s; the scanner was killed",
                    timeout.as_secs()
                )
            } else {
                this.reason
            };
            // A crash or hang before the classes were known, in teardown, or after two classes
            // went wrong with no answer at all: the file is to blame, as a whole.
            if !classes.blame_first_pending(&class_reason)
                || (classes.answered() == 0 && failures >= 2)
            {
                return Err(first);
            }
            if !classes.pending() || restart == MAX_CLASS_RESTARTS {
                break;
            }
        }
        if classes.answered() == 0 {
            return Err(first_failure
                .unwrap_or_else(|| failure(FailureKind::Crash, "the scanner gave no answer")));
        }
        Ok(ScanResult {
            path: path.to_path_buf(),
            format,
            plugins: classes.into_plugins(),
            error: None,
        })
    }
}

/// The scan of a file whose last child ended cleanly: the classes of every attempt, or, for a
/// scanner that does not announce classes, just its result line.
fn finish_clean(
    path: &Path,
    format: PluginFormat,
    mut classes: Classes,
    result: Option<ScanResult>,
) -> Result<ScanResult, ScanFailure> {
    let Some(result) = result else {
        if classes.answered() == 0 {
            return Err(failure(
                FailureKind::BadOutput,
                "the scanner printed no result",
            ));
        }
        return Ok(ScanResult {
            path: path.to_path_buf(),
            format,
            plugins: classes.into_plugins(),
            error: None,
        });
    };
    for p in result.plugins {
        if !classes.is_answered(&p.plugin.id) {
            classes.answer(p);
        }
    }
    let error = if classes.answered() == 0 {
        result.error
    } else {
        None
    };
    Ok(ScanResult {
        path: path.to_path_buf(),
        format,
        plugins: classes.into_plugins(),
        error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_device_sdk::{PluginCategory, PluginKindHint, PluginLayout, PluginRef};

    fn plugin(id: &str) -> ScannedPlugin {
        ScannedPlugin {
            plugin: PluginRef {
                format: PluginFormat::Clap,
                id: id.to_string(),
            },
            layout: PluginLayout {
                name: id.to_string(),
                vendor: String::new(),
                version: String::new(),
                kind_hint: PluginKindHint::Effect,
                main_in: None,
                main_out: None,
                sidechain: false,
                notes_in: false,
                notes_out: false,
                aux_out: 0,
                params: Vec::new(),
            },
            category: PluginCategory::from_kind(PluginKindHint::Effect),
            unsupported: None,
            params_truncated: false,
        }
    }

    fn lines(events: &[ScanEvent]) -> String {
        events
            .iter()
            .map(|e| format!("noise from a plugin\n{}\n", e.to_json()))
            .collect()
    }

    #[test]
    fn only_the_scanners_own_lines_count_as_progress_even_across_reads() {
        let mut head = LineHead::default();
        // A chatty plugin is no sign of life.
        assert_eq!(head.feed(b"loading...\nstill loading\n\n"), 0);
        // A progress line split over two reads, and one that follows a noise line.
        assert_eq!(head.feed(b"{\"clas"), 0);
        assert_eq!(head.feed(b"ses\":[]}\nnoise\n{\"class\":{}}\n"), 2);
        // The final result line is not progress (the scanner ends with it anyway).
        assert_eq!(head.feed(b"{\"path\":\"x\"}\n"), 0);
    }

    #[test]
    fn an_attempt_that_died_keeps_the_classes_that_answered_and_blames_the_next_one() {
        let mut classes = Classes::default();
        let events = [
            ScanEvent::Classes(vec![plugin("a"), plugin("b"), plugin("c")]),
            ScanEvent::Class(plugin("a")),
        ];
        assert!(
            classes
                .absorb(PluginFormat::Clap, &lines(&events))
                .is_none()
        );
        assert_eq!(classes.answered(), 1);
        assert!(classes.blame_first_pending("hung"));
        // The next attempt leaves out what is known: the answer and the class that hung.
        assert_eq!(classes.skip, ["a", "b"]);
        assert!(classes.pending());
        // The restarted scanner announces only `c` and answers it.
        let events = [
            ScanEvent::Classes(vec![plugin("c")]),
            ScanEvent::Class(plugin("c")),
        ];
        classes.absorb(PluginFormat::Clap, &lines(&events));
        assert!(!classes.pending());
        let kept = classes.into_plugins();
        let state: Vec<(&str, Option<&str>)> = kept
            .iter()
            .map(|p| (p.plugin.id.as_str(), p.unsupported.as_deref()))
            .collect();
        assert_eq!(state, [("a", None), ("b", Some("hung")), ("c", None)]);
    }

    #[test]
    fn no_class_to_blame_means_the_file_is_to_blame() {
        let mut classes = Classes::default();
        assert!(!classes.blame_first_pending("hung"));
        classes.absorb(
            PluginFormat::Clap,
            &lines(&[
                ScanEvent::Classes(vec![plugin("a")]),
                ScanEvent::Class(plugin("a")),
            ]),
        );
        assert!(!classes.blame_first_pending("crashed in teardown"));
    }

    #[test]
    fn a_scanner_that_prints_only_a_result_line_still_reads() {
        // The result line of the earlier protocol, with no class lines before it.
        let result = ScanResult {
            path: PathBuf::from("x.clap"),
            format: PluginFormat::Clap,
            plugins: vec![plugin("a")],
            error: None,
        };
        let mut classes = Classes::default();
        let seen = classes.absorb(
            PluginFormat::Clap,
            &format!("hello\n{}\n", result.to_json()),
        );
        let done = finish_clean(Path::new("x.clap"), PluginFormat::Clap, classes, seen).unwrap();
        assert_eq!(done.plugins, result.plugins);
        assert_eq!(done.error, None);
    }

    #[test]
    fn a_clean_exit_with_no_result_line_and_no_class_is_bad_output() {
        let err = finish_clean(
            Path::new("x.clap"),
            PluginFormat::Clap,
            Classes::default(),
            None,
        )
        .unwrap_err();
        assert_eq!(err.kind, FailureKind::BadOutput);
    }
}
