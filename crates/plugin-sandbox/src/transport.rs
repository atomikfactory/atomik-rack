//! What a remote instance talks through: the child's stdin, stdout and stderr and a handle to
//! the process. Real children are spawned here; the tests plug in a fake one.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};

/// A blocking wait for the end of a process, to run on a thread of its own: true once the
/// process has ended, false when it could not be waited for (the pipes then stay the only
/// witness). It does not reap the process.
pub(crate) type ExitWaiter = Box<dyn FnOnce() -> bool + Send>;

/// The process behind a transport.
pub(crate) trait ChildProcess: Send {
    /// Ends the process. Does not wait for it.
    fn kill(&mut self);
    /// `Some(description)` once the process has ended (and is reaped), `None` while it runs.
    fn try_wait(&mut self) -> Option<String>;
    /// What the reason of a lost instance should add about the process: a child that is shared
    /// with other instances names its group. `None` for a child of one instance.
    fn note(&self) -> Option<String> {
        None
    }
    /// A wait for the end of the process itself, for a process that can be waited for apart
    /// from its pipes: a plugin that started a helper process holding the pipes open is then
    /// noticed dead when it dies, not when the helper does. `None` when there is none (a
    /// member of a group learns of the end from its group).
    fn exit_waiter(&mut self) -> Option<ExitWaiter> {
        None
    }
}

/// The channels to one child process.
pub(crate) struct Transport {
    /// The child's stdin.
    pub writer: Box<dyn Write + Send>,
    /// The child's stdout: the control frames.
    pub reader: Box<dyn Read + Send>,
    /// The child's stderr, when there is one.
    pub stderr: Option<Box<dyn Read + Send>>,
    /// The process.
    pub child: Box<dyn ChildProcess>,
}

/// The child object is shared with the exit watcher so that it stays alive (and its handle
/// open) for as long as the watcher waits; the watcher does not hold the lock while it waits.
struct OsChild(Arc<Mutex<Child>>);

impl OsChild {
    fn child(&self) -> std::sync::MutexGuard<'_, Child> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ChildProcess for OsChild {
    fn kill(&mut self) {
        // A process that already ended refuses to be killed; that is fine.
        let _ = self.child().kill();
    }

    fn exit_waiter(&mut self) -> Option<ExitWaiter> {
        let watch = atomik_plugin_ipc::lifetime::ExitWatch::of(&self.child());
        let keep_alive = Arc::clone(&self.0);
        Some(Box::new(move || {
            let ended = watch.wait();
            drop(keep_alive);
            ended
        }))
    }

    fn try_wait(&mut self) -> Option<String> {
        let waited = self.child().try_wait();
        match waited {
            Ok(Some(status)) => Some(describe_exit(status)),
            Ok(None) => None,
            Err(e) => Some(format!("the plugin process cannot be waited for: {e}")),
        }
    }
}

/// What an exit status means: the exit code, or the signal on Unix.
pub(crate) fn describe_exit(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return format!("the plugin process was killed by signal {sig}");
        }
    }
    match status.code() {
        Some(code) => describe_code(code),
        None => "the plugin process ended abnormally".to_string(),
    }
}

/// An exit code in words: the number, in hex as well, and what a Windows crash code means.
pub(crate) fn describe_code(code: i32) -> String {
    let hex = code as u32;
    let meaning = match hex {
        0xC000_0005 => Some("access violation"),
        0xC000_001D => Some("illegal instruction"),
        0xC000_0094 => Some("integer division by zero"),
        0xC000_00FD => Some("stack overflow"),
        0xC000_0374 => Some("heap corruption"),
        0xC000_0409 => Some("fail-fast or stack buffer overrun"),
        0x8000_0003 => Some("breakpoint"),
        _ => None,
    };
    match meaning {
        Some(m) => format!("the plugin process exited with code {code} ({hex:#x}, {m})"),
        None => format!("the plugin process exited with code {code} ({hex:#x})"),
    }
}

/// Starts `program` with `args` and `envs`, its three standard streams piped.
pub(crate) fn spawn_process(
    program: &Path,
    args: &[OsString],
    envs: &[(OsString, OsString)],
) -> io::Result<Transport> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .envs(envs.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS: the child has no console, so the system starts no console host for
        // it (CREATE_NO_WINDOW would give it a hidden one: a second process per plugin). The
        // three standard streams are pipes and work the same without a console.
        cmd.creation_flags(0x0000_0008);
    }
    let mut child = cmd.spawn()?;
    // The child ends with the app, however the app ends. A child that cannot be bound still
    // watches its parent itself.
    let _ = atomik_plugin_ipc::lifetime::bind_child(&child);
    let missing = |what: &str| io::Error::other(format!("no {what} pipe"));
    let writer = child.stdin.take().ok_or_else(|| missing("stdin"))?;
    let reader = child.stdout.take().ok_or_else(|| missing("stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| missing("stderr"))?;
    Ok(Transport {
        writer: Box::new(writer),
        reader: Box::new(reader),
        stderr: Some(Box::new(stderr)),
        child: Box::new(OsChild(Arc::new(Mutex::new(child)))),
    })
}

#[cfg(test)]
mod tests {
    use super::describe_code;

    #[test]
    fn a_windows_crash_code_is_named_and_an_exit_code_is_given_as_is() {
        assert_eq!(
            describe_code(3),
            "the plugin process exited with code 3 (0x3)"
        );
        let av = describe_code(0xC000_0005_u32 as i32);
        assert!(av.contains("access violation"), "{av}");
        assert!(av.contains("0xc0000005"), "{av}");
        assert!(describe_code(0xC000_00FD_u32 as i32).contains("stack overflow"));
    }
}
