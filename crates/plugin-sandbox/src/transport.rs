//! What a remote instance talks through: the child's stdin, stdout and stderr and a handle to
//! the process. Real children are spawned here; the tests plug in a fake one.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};

/// The process behind a transport.
pub(crate) trait ChildProcess: Send {
    /// Ends the process. Does not wait for it.
    fn kill(&mut self);
    /// `Some(description)` once the process has ended (and is reaped), `None` while it runs.
    fn try_wait(&mut self) -> Option<String>;
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

struct OsChild(Child);

impl ChildProcess for OsChild {
    fn kill(&mut self) {
        // A process that already ended refuses to be killed; that is fine.
        let _ = self.0.kill();
    }

    fn try_wait(&mut self) -> Option<String> {
        match self.0.try_wait() {
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
        Some(code) => format!(
            "the plugin process exited with code {code} ({:#x})",
            code as u32
        ),
        None => "the plugin process ended abnormally".to_string(),
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
        // CREATE_NO_WINDOW: no console flashes up for every plugin process.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn()?;
    let missing = |what: &str| io::Error::other(format!("no {what} pipe"));
    let writer = child.stdin.take().ok_or_else(|| missing("stdin"))?;
    let reader = child.stdout.take().ok_or_else(|| missing("stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| missing("stderr"))?;
    Ok(Transport {
        writer: Box::new(writer),
        reader: Box::new(reader),
        stderr: Some(Box::new(stderr)),
        child: Box::new(OsChild(child)),
    })
}
