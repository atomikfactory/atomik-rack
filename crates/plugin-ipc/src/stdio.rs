//! A private stdout for the control channel.
//!
//! Plugins print to stdout (`printf`, `println!`, `std::cout`), which would corrupt the frames
//! the child writes there. [`take_frame_stdout`] moves the real stdout into a private handle
//! and points the process's stdout at stderr.

use std::fs::File;
use std::io;

/// Takes the real stdout for the control channel and redirects the process's stdout to its
/// stderr, so anything printed afterwards, by Rust or by C and C++ code, lands on stderr.
/// Call it once, at child start, before any plugin is loaded.
///
/// * unix: `dup(1)` (close-on-exec) is returned; then `dup2(2, 1)`.
/// * Windows: the stdout handle is duplicated into a private, non-inheritable handle that is
///   returned; the C runtime's fd 1 is pointed at fd 2 (`_dup2`, which also moves the Win32
///   standard output handle); and the Win32 standard output handle is then set to the
///   standard error handle explicitly, which is what Rust's `stdout()` reads on each write.
pub fn take_frame_stdout() -> io::Result<File> {
    imp::take()
}

#[cfg(unix)]
mod imp {
    use std::fs::File;
    use std::io;
    use std::os::fd::FromRawFd;

    pub fn take() -> io::Result<File> {
        // SAFETY: `dup` on a standard descriptor has no memory effects; the result is checked.
        let fd = unsafe { libc::dup(1) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is the descriptor just returned; setting its close-on-exec flag.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            let err = io::Error::last_os_error();
            // SAFETY: `fd` is ours and not used again.
            unsafe { libc::close(fd) };
            return Err(err);
        }
        // SAFETY: descriptors 1 and 2 are the standard ones; the result is checked.
        if unsafe { libc::dup2(2, 1) } < 0 {
            let err = io::Error::last_os_error();
            // SAFETY: `fd` is ours and not used again.
            unsafe { libc::close(fd) };
            return Err(err);
        }
        // SAFETY: `fd` is a valid descriptor that nothing else owns.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::fs::File;
    use std::io;
    use std::os::windows::io::AsHandle;

    /// `STD_OUTPUT_HANDLE` and `STD_ERROR_HANDLE` (`-11` and `-12` as `DWORD`).
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;

    // Declared here rather than through windows-sys so the crate needs no console feature.
    // SAFETY: the signatures are those of `processenv.h`; each call site documents its use.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn SetStdHandle(which: u32, handle: *mut c_void) -> i32;
    }

    // SAFETY: `_dup2` is exported by the C runtime Rust links on MSVC and GNU targets.
    unsafe extern "C" {
        fn _dup2(from: i32, to: i32) -> i32;
    }

    pub fn take() -> io::Result<File> {
        // `try_clone_to_owned` is `DuplicateHandle` into a non-inheritable handle.
        let private = std::io::stdout().as_handle().try_clone_to_owned()?;
        // The C runtime first, so `printf` follows. It fails when the process has no CRT fd 2
        // (a GUI process): the Win32 handle below still covers Rust.
        // SAFETY: plain integer arguments; the result is not needed.
        unsafe { _dup2(2, 1) };
        // SAFETY: `GetStdHandle` has no preconditions; the handle it returns stays valid for
        // the life of the process and is only passed to `SetStdHandle`.
        let err = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        // SAFETY: `err` came from `GetStdHandle` above.
        if unsafe { SetStdHandle(STD_OUTPUT_HANDLE, err) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(File::from(private))
    }
}
