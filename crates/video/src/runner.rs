//! The one seam to the programs: [`VideoTool`] runs `ffmpeg` and `ffprobe`.
//!
//! Every call the crate makes goes through [`VideoTool::run`]: the real [`SystemTool`] starts the
//! process, [`NoVideoTools`] refuses (a headless run), and the fake of [`crate::fake`] answers
//! with canned text and frames, so no test needs FFmpeg.
//!
//! [`SystemTool`] never goes through a shell: the arguments are a list, a path is one argument,
//! the input is closed, no console window opens on Windows, the error output is kept bounded, and
//! the child is killed when the job is cancelled, when the output consumer stops, and when the
//! call unwinds.

use crate::VideoError;
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// The most of the error output kept for a message, in bytes (the end is kept).
pub const STDERR_LIMIT: usize = 4096;

/// How a process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    /// The process ended with a success code (or was stopped on purpose).
    pub success: bool,
    /// Its exit code, when it has one.
    pub code: Option<i32>,
    /// The end of its error output (at most [`STDERR_LIMIT`] bytes).
    pub stderr: String,
    /// The output consumer asked to stop and the process was killed.
    pub stopped: bool,
}

impl Exit {
    /// A success with nothing to say.
    pub fn ok() -> Self {
        Self {
            success: true,
            code: Some(0),
            stderr: String::new(),
            stopped: false,
        }
    }

    /// The failure as an error naming the program of `exe`.
    pub fn failure(&self, exe: &Path) -> VideoError {
        VideoError::Failed {
            tool: tool_name(exe),
            code: self
                .code
                .map_or_else(String::new, |c| format!(" (exit code {c})")),
            stderr: if self.stderr.is_empty() {
                "it printed nothing".to_owned()
            } else {
                self.stderr.clone()
            },
        }
    }
}

/// `ffmpeg` for a program at `tools/ffmpeg.exe`.
pub fn tool_name(exe: &Path) -> String {
    exe.file_stem().map_or_else(
        || "the program".to_owned(),
        |s| s.to_string_lossy().into_owned(),
    )
}

/// What runs the programs.
pub trait VideoTool: Send + Sync {
    /// Runs `exe` with `args` (no shell, each a separate argument) and hands its standard output
    /// to `stdout` in chunks, in order. `stdout` returns `false` to stop the process.
    ///
    /// # Errors
    /// [`VideoError::Spawn`] when the program cannot start and [`VideoError::Cancelled`] when
    /// `cancel` is set (the process is killed first). A process that ends with a failure code is
    /// not an error here: the [`Exit`] says so.
    fn run(
        &self,
        exe: &Path,
        args: &[OsString],
        stdout: &mut dyn FnMut(&[u8]) -> bool,
        cancel: &AtomicBool,
    ) -> Result<Exit, VideoError>;
}

impl<T: VideoTool + ?Sized> VideoTool for std::sync::Arc<T> {
    fn run(
        &self,
        exe: &Path,
        args: &[OsString],
        stdout: &mut dyn FnMut(&[u8]) -> bool,
        cancel: &AtomicBool,
    ) -> Result<Exit, VideoError> {
        (**self).run(exe, args, stdout, cancel)
    }
}

/// Runs a program to the end and returns its standard output as text, at most `limit` bytes.
///
/// # Errors
/// As [`VideoTool::run`]; a failing program is [`VideoError::Failed`] with the end of its error
/// output, and output beyond `limit` is [`VideoError::Probe`].
pub fn capture(
    tool: &dyn VideoTool,
    exe: &Path,
    args: &[OsString],
    limit: usize,
    cancel: &AtomicBool,
) -> Result<String, VideoError> {
    let mut out = Vec::new();
    let mut too_long = false;
    let exit = tool.run(
        exe,
        args,
        &mut |chunk| {
            out.extend_from_slice(chunk);
            too_long = out.len() > limit;
            !too_long
        },
        cancel,
    )?;
    if too_long {
        return Err(VideoError::Probe(format!(
            "{} printed more than {limit} bytes",
            tool_name(exe)
        )));
    }
    if !exit.success {
        return Err(exit.failure(exe));
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// The tool of a run without video: every call is refused.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoVideoTools;

impl VideoTool for NoVideoTools {
    fn run(
        &self,
        _: &Path,
        _: &[OsString],
        _: &mut dyn FnMut(&[u8]) -> bool,
        _: &AtomicBool,
    ) -> Result<Exit, VideoError> {
        Err(VideoError::Unavailable)
    }
}

/// The real programs, started as child processes.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemTool;

/// Kills and reaps the child when dropped (an early return, a panic, the end of the call).
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Reads `source` to the end and returns the last [`STDERR_LIMIT`] bytes as text.
fn read_tail(mut source: impl Read) -> String {
    let mut tail: Vec<u8> = Vec::new();
    let mut buf = [0u8; 2048];
    while let Ok(n) = source.read(&mut buf) {
        if n == 0 {
            break;
        }
        tail.extend_from_slice(&buf[..n]);
        if tail.len() > 2 * STDERR_LIMIT {
            tail.drain(..tail.len() - STDERR_LIMIT);
        }
    }
    if tail.len() > STDERR_LIMIT {
        tail.drain(..tail.len() - STDERR_LIMIT);
    }
    String::from_utf8_lossy(&tail).trim().to_owned()
}

impl VideoTool for SystemTool {
    fn run(
        &self,
        exe: &Path,
        args: &[OsString],
        stdout: &mut dyn FnMut(&[u8]) -> bool,
        cancel: &AtomicBool,
    ) -> Result<Exit, VideoError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(VideoError::Cancelled);
        }
        let mut command = Command::new(exe);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console flashes up for a program of a windowed app.
            command.creation_flags(0x0800_0000);
        }
        let child = command.spawn().map_err(|source| VideoError::Spawn {
            tool: tool_name(exe),
            source,
        })?;
        let mut guard = ChildGuard(child);
        let out = guard.0.stdout.take();
        let err = guard.0.stderr.take();
        let stderr_reader = std::thread::Builder::new()
            .name("atomik-video-stderr".into())
            .spawn(move || err.map_or_else(String::new, read_tail))?;
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(8);
        let stdout_reader = std::thread::Builder::new()
            .name("atomik-video-stdout".into())
            .spawn(move || {
                let Some(mut out) = out else { return };
                let mut buf = vec![0u8; 64 * 1024];
                while let Ok(n) = out.read(&mut buf) {
                    if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            })?;
        let mut stopped = false;
        let mut cancelled = false;
        loop {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(chunk) => {
                    if !stdout(&chunk) {
                        stopped = true;
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        if cancelled || stopped {
            let _ = guard.0.kill();
        }
        drop(rx);
        let status = guard.0.wait();
        let _ = stdout_reader.join();
        let stderr = stderr_reader.join().unwrap_or_default();
        if cancelled {
            return Err(VideoError::Cancelled);
        }
        let status = status?;
        Ok(Exit {
            success: status.success() || stopped,
            code: status.code(),
            stderr,
            stopped,
        })
    }
}
