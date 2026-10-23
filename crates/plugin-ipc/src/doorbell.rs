//! The doorbell: how the parent wakes a child that is waiting for a request.
//!
//! One word in the block header (`child_parked`) holds the state, shared by both processes:
//!
//! ```text
//! IDLE   --child about to sleep--> PARKED
//! PARKED --parent rings----------> RUNG   (and wakes the OS wait)
//! IDLE   --parent rings----------> RUNG   (nobody to wake; the next wait returns at once)
//! RUNG   --child consumes--------> IDLE
//! ```
//!
//! `ring` is one atomic swap, plus one wake-up call only when the child was parked, so a child
//! that keeps up (it spins for ~50 us before parking) costs the parent no system call at all.
//! The child announces PARKED with a compare-exchange from IDLE; if the parent rang in between
//! the exchange fails and sees RUNG, so a wake-up cannot be lost: either the child sees the
//! ring, or the parent sees PARKED and wakes it. The OS wait is told to sleep only while the
//! word still reads PARKED, which closes the remaining gap.
//!
//! The OS part per platform: a named auto-reset event on Windows (the word decides whether to
//! call `SetEvent`; the event is what the child sleeps on), a non-private futex on the word on
//! Linux, `__ulock_wait`/`__ulock_wake` (`UL_COMPARE_AND_WAIT_SHARED`) on macOS, and a
//! short-sleep poll on any other unix.

use crate::block::SharedBlock;
use crate::deadline::now;
use crate::layout::{DOORBELL_IDLE, DOORBELL_PARKED, DOORBELL_RUNG};
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// The OS name of the doorbell (a Windows event name; ignored elsewhere, where the word in the
/// block is the doorbell).
#[derive(Clone, Debug)]
pub struct DoorbellName(pub String);

impl DoorbellName {
    /// The name for one instance of one app process: `Local\atomik-sandbox-<pid>-<instance>`.
    pub fn for_instance(parent_pid: u32, instance: u64) -> Self {
        Self(format!("Local\\atomik-sandbox-{parent_pid}-{instance}"))
    }
}

/// A wait never lasts longer than this in one OS call (and the clamp keeps every conversion
/// below in range).
const MAX_WAIT: Duration = Duration::from_secs(3600);

/// Cross-process wake-up. `ring` never blocks and never allocates.
#[derive(Debug)]
pub struct Doorbell {
    // A handle on the mapping keeps the word alive as long as the doorbell.
    block: SharedBlock,
    #[cfg(windows)]
    event: os::Event,
}

impl Doorbell {
    /// Parent: makes the doorbell of `block`.
    pub fn create(name: &DoorbellName, block: &SharedBlock) -> io::Result<Self> {
        Self::make(name, block, true)
    }

    /// Child: opens the doorbell the parent made for `block`.
    pub fn open(name: &DoorbellName, block: &SharedBlock) -> io::Result<Self> {
        Self::make(name, block, false)
    }

    #[cfg_attr(not(windows), allow(unused_variables))]
    fn make(name: &DoorbellName, block: &SharedBlock, create: bool) -> io::Result<Self> {
        Ok(Self {
            block: block.share(),
            #[cfg(windows)]
            event: if create {
                os::Event::create(&name.0)?
            } else {
                os::Event::open(&name.0)?
            },
        })
    }

    fn word(&self) -> &AtomicU32 {
        &self.block.header().child_parked
    }

    /// Wakes the child if it waits, or leaves a ring for its next wait. Never blocks.
    pub fn ring(&self) {
        if self.word().swap(DOORBELL_RUNG, Ordering::AcqRel) == DOORBELL_PARKED {
            self.os_wake();
        }
    }

    /// Child only: waits for a ring, at most `timeout`. True when rung. A ring that came before
    /// the call counts; rings that came together count once. The caller re-checks its own
    /// condition afterwards, because a ring can be stale (it answered a request already seen).
    pub fn wait(&self, timeout: Duration) -> bool {
        let word = self.word();
        if word
            .compare_exchange(
                DOORBELL_RUNG,
                DOORBELL_IDLE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            return true;
        }
        match word.compare_exchange(
            DOORBELL_IDLE,
            DOORBELL_PARKED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {}
            Err(DOORBELL_RUNG) => {
                word.store(DOORBELL_IDLE, Ordering::Release);
                return true;
            }
            Err(_) => {}
        }
        let timeout = timeout.min(MAX_WAIT);
        let end = now() + timeout;
        loop {
            let remaining = end.saturating_duration_since(now());
            if !remaining.is_zero() {
                self.os_wait(remaining);
            }
            if word
                .compare_exchange(
                    DOORBELL_RUNG,
                    DOORBELL_IDLE,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return true;
            }
            if now() >= end {
                // Timed out. Leave PARKED unless the parent rang just now.
                return match word.compare_exchange(
                    DOORBELL_PARKED,
                    DOORBELL_IDLE,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => false,
                    Err(_) => {
                        word.store(DOORBELL_IDLE, Ordering::Release);
                        true
                    }
                };
            }
        }
    }

    /// Sleeps in the OS while the word reads PARKED, for at most `remaining`. May return early
    /// (a wake-up, a signal, a stale event); the caller re-reads the word.
    fn os_wait(&self, remaining: Duration) {
        #[cfg(windows)]
        self.event.wait(remaining);
        #[cfg(any(target_os = "linux", target_os = "android"))]
        os::futex_wait(self.word(), remaining);
        #[cfg(target_os = "macos")]
        os::ulock_wait(self.word(), remaining);
        #[cfg(all(
            unix,
            not(any(target_os = "linux", target_os = "android", target_os = "macos"))
        ))]
        os::poll_sleep(remaining);
    }

    /// Wakes the child's OS wait. One non-blocking call.
    fn os_wake(&self) {
        #[cfg(windows)]
        self.event.set();
        #[cfg(any(target_os = "linux", target_os = "android"))]
        os::futex_wake(self.word());
        #[cfg(target_os = "macos")]
        os::ulock_wake(self.word());
        // Any other unix polls the word, so there is nothing to wake.
    }
}

#[cfg(windows)]
mod os {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForSingleObject,
    };

    /// The `SYNCHRONIZE` access right (`winnt.h`).
    const SYNCHRONIZE: u32 = 0x0010_0000;

    /// A named auto-reset event.
    #[derive(Debug)]
    pub struct Event(HANDLE);

    // SAFETY: a Windows event handle can be used and closed from any thread.
    unsafe impl Send for Event {}
    // SAFETY: `SetEvent` and `WaitForSingleObject` are thread safe on one handle.
    unsafe impl Sync for Event {}

    fn wide(name: &str) -> Vec<u16> {
        std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    impl Event {
        pub fn create(name: &str) -> io::Result<Self> {
            let wide = wide(name);
            // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call; no
            // security attributes (default, not inheritable); auto-reset, initially not set.
            let handle = unsafe { CreateEventW(null(), 0, 0, wide.as_ptr()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            // The name existed already: another process squats on it. Never share it.
            // SAFETY: reads the calling thread's last-error value, set by `CreateEventW`.
            let existed =
                unsafe { windows_sys::Win32::Foundation::GetLastError() } == ERROR_ALREADY_EXISTS;
            if existed {
                // SAFETY: `handle` is the valid handle just returned and is not used again.
                unsafe { CloseHandle(handle) };
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("the doorbell event `{name}` already exists"),
                ));
            }
            Ok(Self(handle))
        }

        pub fn open(name: &str) -> io::Result<Self> {
            let wide = wide(name);
            // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
            let handle = unsafe { OpenEventW(SYNCHRONIZE | EVENT_MODIFY_STATE, 0, wide.as_ptr()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(handle))
        }

        pub fn set(&self) {
            // SAFETY: `self.0` is a valid event handle for the life of `self`.
            unsafe { SetEvent(self.0) };
        }

        /// Waits for the event, at most `remaining` (rounded up to whole milliseconds).
        pub fn wait(&self, remaining: Duration) {
            let ms = remaining.as_millis().saturating_add(u128::from(
                !remaining.subsec_nanos().is_multiple_of(1_000_000),
            ));
            let ms = u32::try_from(ms).unwrap_or(u32::MAX - 1).min(u32::MAX - 1);
            // SAFETY: `self.0` is a valid event handle for the life of `self`.
            unsafe { WaitForSingleObject(self.0, ms) };
        }
    }

    impl Drop for Event {
        fn drop(&mut self) {
            // SAFETY: the handle is valid and closed exactly once, here.
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod os {
    use crate::layout::DOORBELL_PARKED;
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    /// Sleeps while `*word == PARKED`, at most `remaining`. The futex is not the private
    /// variant: the word lives in memory shared with another process.
    pub fn futex_wait(word: &AtomicU32, remaining: Duration) {
        let ts = libc::timespec {
            tv_sec: remaining.as_secs() as libc::time_t,
            tv_nsec: libc::c_long::from(remaining.subsec_nanos() as i32),
        };
        // SAFETY: `word` points to a live, aligned u32 in the mapping, `ts` lives for the call,
        // and FUTEX_WAIT only reads them. All arguments are passed as `c_long`, as the raw
        // syscall expects. Errors (EAGAIN, EINTR, ETIMEDOUT) all mean "re-read the word".
        unsafe {
            libc::syscall(
                libc::SYS_futex,
                word.as_ptr(),
                libc::FUTEX_WAIT as libc::c_long,
                DOORBELL_PARKED as libc::c_long,
                &ts as *const libc::timespec,
                std::ptr::null::<u32>(),
                0 as libc::c_long,
            );
        }
    }

    /// Wakes one waiter on `word`.
    pub fn futex_wake(word: &AtomicU32) {
        // SAFETY: `word` points to a live, aligned u32 in the mapping; FUTEX_WAKE does not
        // read through the null pointers.
        unsafe {
            libc::syscall(
                libc::SYS_futex,
                word.as_ptr(),
                libc::FUTEX_WAKE as libc::c_long,
                1 as libc::c_long,
                std::ptr::null::<libc::timespec>(),
                std::ptr::null::<u32>(),
                0 as libc::c_long,
            );
        }
    }
}

#[cfg(target_os = "macos")]
mod os {
    use crate::layout::DOORBELL_PARKED;
    use std::ffi::{c_int, c_void};
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    /// `UL_COMPARE_AND_WAIT_SHARED`: compare a 32-bit word, shared between processes.
    const UL_COMPARE_AND_WAIT_SHARED: u32 = 3;
    /// Return a negative errno instead of setting `errno`.
    const ULF_NO_ERRNO: u32 = 0x0100_0000;

    // These two are the kernel entry points libc++ and the Go runtime use for futex-like
    // waits; they are declared in XNU's `bsd/sys/ulock.h` and exported by libsystem_kernel.
    // SAFETY: declarations only; the signatures match `ulock.h`, and each call site documents
    // why its arguments are valid.
    unsafe extern "C" {
        fn __ulock_wait(operation: u32, addr: *mut c_void, value: u64, timeout_us: u32) -> c_int;
        fn __ulock_wake(operation: u32, addr: *mut c_void, wake_value: u64) -> c_int;
    }

    /// Sleeps while `*word == PARKED`, at most `remaining` (at least 1 us: 0 means forever).
    pub fn ulock_wait(word: &AtomicU32, remaining: Duration) {
        let us = remaining.as_micros().clamp(1, u128::from(u32::MAX)) as u32;
        // SAFETY: `word` points to a live, aligned u32 in the mapping; the call only reads it.
        // Errors (EINTR, ETIMEDOUT) all mean "re-read the word".
        unsafe {
            __ulock_wait(
                UL_COMPARE_AND_WAIT_SHARED | ULF_NO_ERRNO,
                word.as_ptr().cast::<c_void>(),
                u64::from(DOORBELL_PARKED),
                us,
            );
        }
    }

    /// Wakes one waiter on `word`.
    pub fn ulock_wake(word: &AtomicU32) {
        // SAFETY: `word` points to a live, aligned u32 in the mapping; no waiter is required.
        unsafe {
            __ulock_wake(
                UL_COMPARE_AND_WAIT_SHARED | ULF_NO_ERRNO,
                word.as_ptr().cast::<c_void>(),
                0,
            );
        }
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_os = "macos"))
))]
mod os {
    use std::time::Duration;

    /// No futex here: sleep a short while and let the caller re-read the word.
    pub fn poll_sleep(remaining: Duration) {
        std::thread::sleep(remaining.min(Duration::from_micros(200)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn bell() -> (SharedBlock, Doorbell, Doorbell) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path =
            crate::block::default_block_dir().join(crate::block::block_file_name(pid, 900_000 + n));
        let name = DoorbellName::for_instance(pid, 900_000 + n);
        let parent_block = SharedBlock::create(&path).unwrap();
        let child_block = SharedBlock::open(&path).unwrap();
        let parent = Doorbell::create(&name, &parent_block).unwrap();
        let child = Doorbell::open(&name, &child_block).unwrap();
        // The child block must outlive its doorbell only through the doorbell's own handle.
        drop(child_block);
        (parent_block, parent, child)
    }

    #[test]
    fn the_name_carries_the_pid_and_the_instance() {
        let n = DoorbellName::for_instance(12, 34);
        assert_eq!(n.0, "Local\\atomik-sandbox-12-34");
    }

    #[test]
    fn a_ring_before_the_wait_returns_at_once() {
        let (_b, parent, child) = bell();
        parent.ring();
        let t = now();
        assert!(child.wait(Duration::from_secs(5)));
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn several_rings_count_once() {
        let (_b, parent, child) = bell();
        parent.ring();
        parent.ring();
        parent.ring();
        assert!(child.wait(Duration::from_secs(5)));
        assert!(
            !child.wait(Duration::from_millis(20)),
            "the rings were consumed together"
        );
    }

    #[test]
    fn a_wait_times_out_when_nobody_rings() {
        let (_b, _parent, child) = bell();
        let t = now();
        assert!(!child.wait(Duration::from_millis(30)));
        assert!(t.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn a_timed_out_wait_leaves_the_doorbell_usable() {
        let (_b, parent, child) = bell();
        assert!(!child.wait(Duration::from_millis(10)));
        parent.ring();
        assert!(child.wait(Duration::from_secs(5)));
    }

    #[test]
    fn a_ring_wakes_a_parked_waiter() {
        let (_b, parent, child) = bell();
        let waiter = std::thread::spawn(move || {
            let t = now();
            let rung = child.wait(Duration::from_secs(10));
            (rung, t.elapsed())
        });
        std::thread::sleep(Duration::from_millis(100));
        parent.ring();
        let (rung, took) = waiter.join().unwrap();
        assert!(rung);
        assert!(took < Duration::from_secs(5), "{took:?}");
    }

    #[test]
    fn a_wait_and_a_ring_race_without_losing_a_wake_up() {
        let (_b, parent, child) = bell();
        let waiter = std::thread::spawn(move || {
            let mut rung = 0;
            for _ in 0..2000 {
                if child.wait(Duration::from_secs(10)) {
                    rung += 1;
                }
            }
            rung
        });
        // Each ring is awaited before the next so none coalesces: the waiter needs 2000.
        for _ in 0..2000 {
            parent.ring();
            // Let the waiter consume it: the word returns to IDLE.
            while parent.word().load(Ordering::Acquire) == DOORBELL_RUNG {
                std::thread::yield_now();
            }
        }
        assert_eq!(waiter.join().unwrap(), 2000);
    }

    #[cfg(windows)]
    #[test]
    fn creating_a_doorbell_under_a_taken_name_fails() {
        let (b, _parent, _child) = bell();
        let clash = DoorbellName(format!(
            "Local\\atomik-sandbox-clash-{}",
            std::process::id()
        ));
        let first = Doorbell::create(&clash, &b).unwrap();
        let err = Doorbell::create(&clash, &b).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        drop(first);
    }
}
