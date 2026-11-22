//! What a scanner child needs from the operating system so that a bad plugin file costs nothing
//! once the scan is over: a group that dies as one, a lower priority, and no error dialogs.
//!
//! * [`prepare`] (before the spawn) puts the child below normal priority and, on Unix, in a
//!   process group of its own; [`ScanGroup::attach`] (after it) joins it to a kill-on-close job
//!   object (Windows) or lowers its niceness (Unix). A helper process a plugin starts while it
//!   loads inherits the group and the priority.
//! * [`ScanGroup::kill`], also run when the group is dropped, ends every process of the group: a
//!   scan that timed out, crashed, was cancelled or even finished leaves nothing behind. A group
//!   is signalled at most once, and on Unix only while its leader is not yet reaped: a process
//!   group id is held only while the group has a member or its leader is a zombie, so a signal
//!   after the reap could reach an unrelated process that was given the number. Callers order it
//!   as [`leader_exited`] (looks, does not reap), [`ScanGroup::kill`], then `Child::wait`. On
//!   Windows the job handle cannot be recycled under us, so `try_wait` and the handle are enough.
//! * [`suppress_error_dialogs`] runs at the top of the scanner process: a crash is then a crash
//!   at once, not a system dialog that waits for the user until the timeout.
//!
//! This is one of the few places in the host crate that use `unsafe` (pinned by name in
//! `tests/unsafe_policy.rs`). The sandbox has a similar job object in `plugin-ipc`, which this
//! crate cannot use (that crate depends on this one); that one is process-wide and ends only
//! with the app, this one is one per scan.
#![allow(unsafe_code)] // Justification: job objects, error modes and process groups are raw OS calls.
#![deny(clippy::undocumented_unsafe_blocks)]

use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};

/// CREATE_NO_WINDOW: no console flashes up for every scanned file.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// BELOW_NORMAL_PRIORITY_CLASS.
#[cfg(windows)]
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

/// The niceness a scanner child gets on Unix (positive is lower priority).
pub const SCAN_NICE: i32 = 10;

/// The Windows process creation flags of a scan child: no window, below normal priority. Zero
/// elsewhere.
pub fn scan_creation_flags() -> u32 {
    #[cfg(windows)]
    {
        CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// Configures `cmd` for a scan child (see the module docs): the creation flags on Windows, a
/// process group of its own on Unix.
pub fn prepare(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(scan_creation_flags());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = cmd;
    }
}

/// Everything a scan child started, as one thing that can be ended.
#[derive(Debug)]
pub struct ScanGroup {
    #[cfg(windows)]
    job: Option<usize>,
    #[cfg(unix)]
    pgid: libc::pid_t,
    /// True until the group has been signalled (or when there is no group to signal).
    armed: AtomicBool,
}

impl ScanGroup {
    /// Joins the freshly started `child` (prepared with [`prepare`]) to a group. Never fails: a
    /// group that could not be made only means [`ScanGroup::kill`] has less to end, and the
    /// caller still kills the child itself.
    pub fn attach(child: &Child) -> Self {
        #[cfg(windows)]
        {
            let job = os::new_job_with(child);
            Self {
                armed: AtomicBool::new(job.is_some()),
                job,
            }
        }
        #[cfg(unix)]
        {
            let pgid = libc::pid_t::try_from(child.id()).unwrap_or(0);
            if pgid > 0 {
                os::lower_priority(pgid);
            }
            Self {
                armed: AtomicBool::new(pgid > 1),
                pgid,
            }
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = child;
            Self {
                armed: AtomicBool::new(false),
            }
        }
    }

    /// Runs `send` the first time only; false (and nothing run) every time after.
    fn signal_once(&self, send: impl FnOnce()) -> bool {
        if self.armed.swap(false, Ordering::AcqRel) {
            send();
            true
        } else {
            false
        }
    }

    /// Ends every process of the group, the child and whatever it started. Only the first call
    /// signals; later calls and the drop do nothing (see the module docs for why).
    pub fn kill(&self) {
        self.signal_once(|| {
            #[cfg(windows)]
            if let Some(job) = self.job {
                os::terminate_job(job);
            }
            #[cfg(unix)]
            os::kill_group(self.pgid);
        });
    }
}

impl Drop for ScanGroup {
    fn drop(&mut self) {
        // A group dropped without a kill still has its leader unreaped (the caller waits for the
        // child only after the group is gone), so this signal is safe.
        self.kill();
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            os::close_job(job);
        }
    }
}

/// Whether the child has ended, without reaping it on Unix (the zombie keeps the group id
/// reserved until the caller has signalled the group and calls `Child::wait`). Windows reads
/// the exit with `try_wait`: its job handle cannot be recycled.
///
/// # Errors
/// The system call failed.
pub fn leader_exited(child: &mut Child) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        os::peek_exited(child.id())
    }
    #[cfg(not(unix))]
    {
        child.try_wait().map(|s| s.is_some())
    }
}

/// Stops the system error dialogs for this process: the critical-error box, the
/// general-protection-fault box and the open-file box (Windows). Called at the top of the
/// scanner process, so a plugin that crashes while it loads ends the scanner at once. Elsewhere
/// there is nothing to switch off.
pub fn suppress_error_dialogs() {
    #[cfg(windows)]
    os::suppress_error_dialogs();
}

#[cfg(windows)]
mod os {
    use std::process::Child;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };

    pub fn suppress_error_dialogs() {
        // SAFETY: `SetErrorMode` only changes this process's error-mode flags; it has no
        // preconditions and the returned previous mode is not needed.
        unsafe {
            SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
        }
    }

    /// A kill-on-close job with `child` in it; `None` when either step fails.
    pub fn new_job_with(child: &Child) -> Option<usize> {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: plain Win32 calls with null optional arguments; the job handle is checked
        // before use and is closed here on every failure path, or by `ScanGroup::drop`. The
        // child handle is owned by `child`, which the caller holds across the call.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    ..Default::default()
                },
                ..Default::default()
            };
            let set = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if set == 0 || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
                CloseHandle(job);
                return None;
            }
            Some(job as usize)
        }
    }

    pub fn terminate_job(job: usize) {
        // SAFETY: `job` is a job handle this module made and has not closed (it is closed only
        // by `close_job`, from the owner's drop).
        unsafe {
            TerminateJobObject(job as _, 1);
        }
    }

    pub fn close_job(job: usize) {
        // SAFETY: `job` is a job handle this module made; the owner gives it up here, once.
        unsafe {
            CloseHandle(job as _);
        }
    }
}

#[cfg(unix)]
mod os {
    pub fn peek_exited(pid: u32) -> std::io::Result<bool> {
        let Ok(id) = libc::id_t::try_from(pid) else {
            return Ok(false);
        };
        // SAFETY: an all-zero `siginfo_t` is a valid value for `waitid` to fill in.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `waitid` with WNOWAIT and WNOHANG only looks at our own child `id`, which the
        // caller has not reaped, and writes into `info`; it leaves the zombie in place.
        let r = unsafe {
            libc::waitid(
                libc::P_PID,
                id,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            // SAFETY: `info` was zeroed and then filled in by a successful `waitid`.
            Ok(unsafe { info.si_pid() } != 0)
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            Ok(info.si_pid != 0)
        }
    }

    pub fn lower_priority(pid: libc::pid_t) {
        let Ok(who) = libc::id_t::try_from(pid) else {
            return;
        };
        // SAFETY: `setpriority` on a process id only changes that process's niceness; a child
        // that has already ended just makes it fail, which is ignored.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, who, super::SCAN_NICE);
        }
    }

    pub fn kill_group(pgid: libc::pid_t) {
        if pgid <= 1 {
            return;
        }
        // SAFETY: a negative pid addresses the process group `pgid`, which the scan child
        // leads (see `prepare`); a group with no member left makes this fail harmlessly.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scan_child_is_asked_for_no_window_and_a_lower_priority() {
        #[cfg(windows)]
        {
            let flags = scan_creation_flags();
            assert_ne!(flags & CREATE_NO_WINDOW, 0);
            assert_ne!(flags & BELOW_NORMAL_PRIORITY_CLASS, 0);
        }
        #[cfg(not(windows))]
        assert_eq!(scan_creation_flags(), 0);
        // Niceness is positive: lower priority than the app.
        const { assert!(SCAN_NICE > 0) };
    }
}

#[cfg(test)]
mod once_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A group that signals nothing real (no process, no job) but can be armed.
    fn armed_group() -> ScanGroup {
        ScanGroup {
            #[cfg(windows)]
            job: None,
            #[cfg(unix)]
            pgid: 0,
            armed: AtomicBool::new(true),
        }
    }

    #[test]
    fn a_group_is_signalled_once_and_never_again_by_a_second_kill_or_the_drop() {
        let sent = AtomicUsize::new(0);
        let group = armed_group();
        assert!(group.signal_once(|| {
            sent.fetch_add(1, Ordering::SeqCst);
        }));
        for _ in 0..3 {
            assert!(!group.signal_once(|| {
                sent.fetch_add(1, Ordering::SeqCst);
            }));
        }
        group.kill();
        assert_eq!(sent.load(Ordering::SeqCst), 1);
        // The explicit kill took the id: the drop has nothing left to signal.
        assert!(!group.armed.load(Ordering::SeqCst));
        drop(group);
    }

    #[test]
    fn a_group_that_was_never_killed_is_still_armed_for_its_drop() {
        let group = armed_group();
        assert!(group.armed.load(Ordering::SeqCst));
    }

    #[test]
    fn a_group_with_no_process_signals_nothing() {
        let group = ScanGroup {
            #[cfg(windows)]
            job: None,
            #[cfg(unix)]
            pgid: 0,
            armed: AtomicBool::new(false),
        };
        assert!(!group.signal_once(|| panic!("nothing to signal")));
    }
}
