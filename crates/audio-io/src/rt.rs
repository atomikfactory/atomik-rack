//! One-time real-time priority promotion of the audio callback thread.
//!
//! The promotion runs on the first callback, before the guarded render. It is the one
//! documented HI-1 exception and is never repeated. The callback never prints: a failure is
//! recorded in an atomic and reported once by the control thread.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

const PENDING: u8 = 0;
const PROMOTED: u8 = 1;
const FAILED: u8 = 2;

/// Promotion state shared between the audio callback and the control thread.
#[derive(Debug, Default)]
pub(crate) struct Promotion {
    state: AtomicU8,
    warned: AtomicBool,
}

impl Promotion {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Runs `promote` the first time it is called; later calls are a single atomic load.
    pub(crate) fn promote_once(&self, promote: impl FnOnce() -> bool) {
        if self.state.load(Ordering::Acquire) != PENDING {
            return;
        }
        let state = if promote() { PROMOTED } else { FAILED };
        self.state.store(state, Ordering::Release);
    }

    /// `None` until the first callback, then whether the promotion succeeded.
    pub(crate) fn result(&self) -> Option<bool> {
        match self.state.load(Ordering::Acquire) {
            PROMOTED => Some(true),
            FAILED => Some(false),
            _ => None,
        }
    }

    /// `true` exactly once, the first time it is called after a failed promotion.
    pub(crate) fn take_warning(&self) -> bool {
        self.result() == Some(false) && !self.warned.swap(true, Ordering::Relaxed)
    }
}

/// Promotes the current thread: `TimeCritical` on Windows (thread-priority has no MMCSS
/// support), `ThreadPriority::Max` elsewhere. Returns whether it succeeded.
pub(crate) fn promote_current_thread() -> bool {
    #[cfg(windows)]
    {
        use thread_priority::windows::{
            WinAPIThreadPriority, set_winapi_thread_priority, thread_native_id,
        };
        set_winapi_thread_priority(thread_native_id(), WinAPIThreadPriority::TimeCritical).is_ok()
    }
    #[cfg(not(windows))]
    {
        thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Max).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn promote_once_runs_the_promotion_exactly_once() {
        let p = Promotion::new();
        let calls = Cell::new(0);
        assert_eq!(p.result(), None);
        for _ in 0..5 {
            p.promote_once(|| {
                calls.set(calls.get() + 1);
                true
            });
        }
        assert_eq!(calls.get(), 1);
        assert_eq!(p.result(), Some(true));
        assert!(!p.take_warning());
    }

    #[test]
    fn failed_promotion_warns_exactly_once() {
        let p = Promotion::new();
        assert!(!p.take_warning());
        p.promote_once(|| false);
        p.promote_once(|| true); // never retried
        assert_eq!(p.result(), Some(false));
        assert!(p.take_warning());
        assert!(!p.take_warning());
    }

    #[test]
    fn later_callbacks_do_not_allocate() {
        let p = Promotion::new();
        p.promote_once(|| true);
        atomik_testkit::no_alloc::assert_no_alloc(|| p.promote_once(|| unreachable!()));
    }

    #[test]
    fn promote_current_thread_on_a_scratch_thread_does_not_panic() {
        // Best effort: may fail without privileges, must never panic.
        let _ = std::thread::spawn(promote_current_thread).join().unwrap();
    }
}
