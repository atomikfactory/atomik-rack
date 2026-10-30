//! Tying a child's life to its parent's, so that a plugin process never outlives the app.
//!
//! The usual end of a child is the parent closing its stdin, which the child's main loop sees.
//! That is not enough when the child's main thread is stuck inside a plugin call, or the parent
//! died without closing anything: the child would run on, holding its plugin and its audio
//! block, until someone found it. Two independent mechanisms end it:
//!
//! * the parent side, [`bind_child`]: on Windows the child joins a job object that kills its
//!   processes when the app's last handle to the job closes (so also when the app is killed);
//! * the child side, [`watch_parent`] and [`exit_after`]: a thread that ends the process a short
//!   grace after the parent is gone or the control channel closed, whatever the main thread is
//!   doing. On every system this polls or waits for the parent's pid; it does not use
//!   `PR_SET_PDEATHSIG`, which fires when the *thread* that started the child ends, and the
//!   app starts children from threads that come and go.

use std::io;
use std::process::Child;
use std::time::Duration;

/// The exit code of a child that ended itself because its parent was gone.
pub const ORPHAN_EXIT_CODE: u32 = 75;

/// How often the child checks that its parent is alive (Unix).
#[cfg(unix)]
const POLL: Duration = Duration::from_millis(250);

#[cfg(windows)]
mod os {
    use super::ORPHAN_EXIT_CODE;
    use std::io;
    use std::process::Child;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, TerminateProcess,
        WaitForSingleObject,
    };

    /// The job every child joins: one for the whole process, never closed by hand (the OS
    /// closes it when the app ends, which is the point).
    static JOB: OnceLock<Option<usize>> = OnceLock::new();

    fn job() -> Option<usize> {
        *JOB.get_or_init(|| {
            // SAFETY: plain Win32 calls with null optional arguments; the handle is checked
            // before use and kept for the life of the process.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                    BasicLimitInformation:
                        windows_sys::Win32::System::JobObjects::JOBOBJECT_BASIC_LIMIT_INFORMATION {
                            LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                            ..Default::default()
                        },
                    ..Default::default()
                };
                let ok = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    CloseHandle(job);
                    return None;
                }
                Some(job as usize)
            }
        })
    }

    pub fn bind_child(child: &Child) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        let job = job().ok_or_else(|| io::Error::other("cannot create the job object"))?;
        // SAFETY: both handles are valid: the job lives for the whole process and the child
        // handle is owned by `child`, which the caller holds across the call.
        let ok = unsafe { AssignProcessToJobObject(job as _, child.as_raw_handle() as _) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Blocks until the process `pid` ends. False when it cannot be watched.
    pub fn wait_for_parent(pid: u32) -> bool {
        // SAFETY: the handle is opened, waited on and closed here, and never shared.
        unsafe {
            let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if h.is_null() {
                // Already gone (or not ours to see): treated as gone.
                return true;
            }
            let r = WaitForSingleObject(h, INFINITE);
            CloseHandle(h);
            r == WAIT_OBJECT_0
        }
    }

    pub fn terminate_now() -> ! {
        // SAFETY: ends this process without running any exit code, which may be stuck.
        unsafe {
            TerminateProcess(GetCurrentProcess(), ORPHAN_EXIT_CODE);
        }
        std::process::abort()
    }
}

#[cfg(unix)]
mod os {
    use super::{ORPHAN_EXIT_CODE, POLL};
    use std::io;
    use std::process::Child;

    pub fn bind_child(_child: &Child) -> io::Result<()> {
        // The child's own watchdog does the work on this side.
        Ok(())
    }

    fn alive(pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 only checks that the process exists and may be signalled.
        let r = unsafe { libc::kill(pid, 0) };
        r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    /// Blocks until the process `pid` is gone.
    pub fn wait_for_parent(pid: u32) -> bool {
        while alive(pid) {
            std::thread::sleep(POLL);
        }
        true
    }

    pub fn terminate_now() -> ! {
        // SAFETY: `_exit` ends the process at once, without atexit handlers that a stuck plugin
        // thread could block.
        unsafe { libc::_exit(ORPHAN_EXIT_CODE as libc::c_int) }
    }
}

/// Parent side: makes `child` end when this process ends, however that happens. Windows: the
/// child joins a kill-on-close job object. Elsewhere this does nothing (the child watches its
/// parent itself).
///
/// # Errors
/// The job object could not be created or the child could not join it. The child still runs
/// and still watches its parent.
pub fn bind_child(child: &Child) -> io::Result<()> {
    os::bind_child(child)
}

/// Child side: ends this process `grace` after `parent_pid` is gone, whatever any thread of it
/// is doing. Returns at once; a pid of 0 is not watched.
pub fn watch_parent(parent_pid: u32, grace: Duration) {
    if parent_pid == 0 {
        return;
    }
    let started = std::thread::Builder::new()
        .name("parent-watch".to_string())
        .spawn(move || {
            if os::wait_for_parent(parent_pid) {
                std::thread::sleep(grace);
                os::terminate_now();
            }
        });
    // No thread: the stdin end of the channel remains.
    drop(started);
}

/// Child side: ends this process `grace` from now, unless it has ended by itself first. Called
/// when the control channel closes: a main thread that is free exits long before.
pub fn exit_after(grace: Duration) {
    let started = std::thread::Builder::new()
        .name("exit-watch".to_string())
        .spawn(move || {
            std::thread::sleep(grace);
            os::terminate_now();
        });
    drop(started);
}
