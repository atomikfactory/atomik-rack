//! The machine's physical memory, for the limits that follow it (the undo history, an import, a
//! bounce). The one file of the application that asks the system for it.
#![allow(unsafe_code)] // Justification: `GlobalMemoryStatusEx` and `sysctlbyname` are raw system calls.

use std::sync::OnceLock;

/// What the machine has, in bytes. A figure the system did not give is `None`, and every limit
/// then behaves as it did before limits followed the machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryInfo {
    /// Physical memory in total.
    pub total: Option<u64>,
    /// Physical memory that can be had now without paging hard.
    pub available: Option<u64>,
}

impl MemoryInfo {
    /// A machine that tells nothing: every limit stays at its default.
    pub const UNKNOWN: Self = Self {
        total: None,
        available: None,
    };

    /// The figures of this machine: the total is read once, the available figure now.
    pub fn read() -> Self {
        Self {
            total: physical_memory(),
            available: available_memory(),
        }
    }
}

/// Total physical memory in bytes, read once. `None` when the system does not tell.
pub fn physical_memory() -> Option<u64> {
    static TOTAL: OnceLock<Option<u64>> = OnceLock::new();
    *TOTAL.get_or_init(read_total)
}

/// Bytes of physical memory that can be had now without paging hard, read on every call.
/// `None` when the system does not tell (always on macOS, which has no one-call figure).
pub fn available_memory() -> Option<u64> {
    read_available()
}

#[cfg(windows)]
fn status() -> Option<windows_sys::Win32::System::SystemInformation::MEMORYSTATUSEX> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut s = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `s` is a valid, writable MEMORYSTATUSEX whose `dwLength` is set, for the call.
    if unsafe { GlobalMemoryStatusEx(&mut s) } == 0 {
        return None;
    }
    Some(s)
}

#[cfg(windows)]
fn read_total() -> Option<u64> {
    status().map(|s| s.ullTotalPhys)
}

#[cfg(windows)]
fn read_available() -> Option<u64> {
    status().map(|s| s.ullAvailPhys)
}

#[cfg(target_os = "macos")]
fn read_total() -> Option<u64> {
    let mut bytes: u64 = 0;
    let mut len: libc::size_t = std::mem::size_of::<u64>();
    // SAFETY: the name is a NUL-terminated string; `bytes` is writable for `len` bytes and `len`
    // holds that size; no new value is given (null pointer, zero length).
    let rc = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut bytes as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && len == std::mem::size_of::<u64>() && bytes > 0).then_some(bytes)
}

/// macOS has no single call for "free without paging": the figure is left unknown.
#[cfg(target_os = "macos")]
fn read_available() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn read_total() -> Option<u64> {
    meminfo_kib(&std::fs::read_to_string("/proc/meminfo").ok()?, "MemTotal")
}

#[cfg(target_os = "linux")]
fn read_available() -> Option<u64> {
    meminfo_kib(
        &std::fs::read_to_string("/proc/meminfo").ok()?,
        "MemAvailable",
    )
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn read_total() -> Option<u64> {
    None
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn read_available() -> Option<u64> {
    None
}

/// The value of `key` in the text of `/proc/meminfo`, in bytes (the file counts in kibibytes).
/// Linux reads the file and needs no `unsafe`; the parser is plain text so it is tested anywhere.
#[cfg(any(target_os = "linux", test))]
fn meminfo_kib(text: &str, key: &str) -> Option<u64> {
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))?;
    let mut parts = line.split_whitespace();
    let kib: u64 = parts.next()?.parse().ok()?;
    let unit = parts.next().unwrap_or("kB");
    (unit == "kB").then(|| kib.checked_mul(1024)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn the_host_reports_a_plausible_total() {
        let total = physical_memory().expect("the host tells its memory");
        assert!((GIB..=16 * 1024 * GIB).contains(&total), "{total}");
        assert_eq!(physical_memory(), Some(total), "the cached total is stable");
    }

    #[test]
    fn available_is_at_most_the_total() {
        let total = physical_memory().expect("total");
        match available_memory() {
            Some(free) => assert!(free <= total, "{free} > {total}"),
            // macOS has no one-call figure (see `read_available`); every other host tells.
            None => assert!(cfg!(target_os = "macos"), "the host tells its free memory"),
        }
        assert_eq!(MemoryInfo::read().total, Some(total));
    }

    #[test]
    fn meminfo_reads_kibibytes_as_bytes() {
        let text = "MemTotal:       16384000 kB\nMemFree:  100 kB\nMemAvailable:    8192 kB\n";
        assert_eq!(meminfo_kib(text, "MemTotal"), Some(16_384_000 * 1024));
        assert_eq!(meminfo_kib(text, "MemAvailable"), Some(8192 * 1024));
        assert_eq!(meminfo_kib(text, "SwapTotal"), None);
        assert_eq!(meminfo_kib("MemTotal: lots kB\n", "MemTotal"), None);
        assert_eq!(meminfo_kib("MemTotal: 5 MB\n", "MemTotal"), None);
    }
}
