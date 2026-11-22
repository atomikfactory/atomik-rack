//! What a scanner child needs from the operating system so that a bad plugin file costs nothing
//! once the scan is over: a group that dies as one, a lower priority, and no error dialogs.
//!
//! * [`prepare`] (before the spawn) puts the child below normal priority and, on Unix, in a
//!   process group of its own; [`ScanGroup::attach`] (after it) joins it to a kill-on-close job
//!   object (Windows) or lowers its niceness (Unix). A helper process a plugin starts while it
//!   loads inherits the group and the priority.
//! * [`ScanGroup::kill`], also run when the group is dropped, ends every process of the group: a
//!   scan that timed out, crashed, was cancelled or even finished leaves nothing behind.
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
    pgid: Option<libc::pid_t>,
}

impl ScanGroup {
    /// Joins the freshly started `child` (prepared with [`prepare`]) to a group. Never fails: a
    /// group that could not be made only means [`ScanGroup::kill`] has less to end, and the
    /// caller still kills the child itself.
    pub fn attach(child: &Child) -> Self {
        #[cfg(windows)]
        {
            Self {
                job: os::new_job_with(child),
            }
        }
        #[cfg(unix)]
        {
            let pgid = libc::pid_t::try_from(child.id()).ok();
            if let Some(pid) = pgid {
                os::lower_priority(pid);
            }
            Self { pgid }
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = child;
            Self {}
        }
    }

    /// Ends every process of the group, the child and whatever it started. Safe to call twice.
    pub fn kill(&self) {
        #[cfg(windows)]
        if let Some(job) = self.job {
            os::terminate_job(job);
        }
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            os::kill_group(pgid);
        }
    }
}

impl Drop for ScanGroup {
    fn drop(&mut self) {
        self.kill();
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            os::close_job(job);
        }
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
