//! A test-only guard for the audio-thread rules: no locks, no blocking calls.
//!
//! The engine marks every device `process` call with [`audio_thread`]. Code that may run there
//! takes its locks through [`RtMutex`] and calls [`forbid`] before a blocking host call (file or
//! socket I/O, sleeping, joining), and a test build panics when either happens inside a marked
//! call, which the engine contains as a device fault the test asserts on.
//!
//! Everything here is compiled out unless the `rt-guard` feature is on (the test kit turns it
//! on for test builds only): without it [`audio_thread`] and [`forbid`] do nothing and
//! [`RtMutex`] is a plain `std::sync::Mutex` behind the same API, so release builds carry no
//! detector code and no cost.

use std::sync::{LockResult, Mutex, MutexGuard};

#[cfg(feature = "rt-guard")]
mod on {
    use std::cell::Cell;

    thread_local! {
        static DEPTH: Cell<u32> = const { Cell::new(0) };
    }

    pub(super) fn enter() {
        DEPTH.with(|d| d.set(d.get() + 1));
    }

    pub(super) fn leave() {
        DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }

    pub(super) fn active() -> bool {
        DEPTH.with(|d| d.get() > 0)
    }
}

/// Marks the current thread as running audio-thread code until it is dropped. Nests.
#[derive(Debug)]
#[must_use = "the thread is marked until the scope is dropped"]
pub struct AudioThreadScope(());

/// Marks the current thread as running audio-thread code (see the module docs).
pub fn audio_thread() -> AudioThreadScope {
    #[cfg(feature = "rt-guard")]
    on::enter();
    AudioThreadScope(())
}

impl Drop for AudioThreadScope {
    fn drop(&mut self) {
        #[cfg(feature = "rt-guard")]
        on::leave();
    }
}

/// True while the current thread is inside an [`audio_thread`] scope (always false without the
/// `rt-guard` feature).
pub fn on_audio_thread() -> bool {
    #[cfg(feature = "rt-guard")]
    {
        on::active()
    }
    #[cfg(not(feature = "rt-guard"))]
    {
        false
    }
}

/// Panics, naming `what`, when called on an audio thread. Call it before a blocking host call.
/// Does nothing without the `rt-guard` feature.
#[inline]
pub fn forbid(what: &str) {
    #[cfg(feature = "rt-guard")]
    if on::active() {
        panic!("audio-thread rule violated: {what}");
    }
    #[cfg(not(feature = "rt-guard"))]
    let _ = what;
}

/// A mutex for code that must stay off the audio thread: `lock` is [`forbid`]den there in a
/// guarded build and is `std::sync::Mutex::lock` otherwise.
#[derive(Debug, Default)]
pub struct RtMutex<T>(Mutex<T>);

impl<T> RtMutex<T> {
    /// A new mutex holding `value`.
    pub const fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }

    /// Locks the mutex; panics on an audio thread in a guarded build.
    pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        forbid("a mutex was locked");
        self.0.lock()
    }
}
