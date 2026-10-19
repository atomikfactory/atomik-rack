//! Which thread is which. The host implements the plugin thread-check from two
//! thread-local facts, so the answers are right whatever thread the engine runs a device on:
//!
//! * the **main thread** is the thread that called [`mark_main_thread`] (the winit thread);
//! * a thread is an **audio thread** while it is inside [`AudioThreadGuard`], which
//!   `PluginDevice::process` holds around every plugin call, on the audio callback or on any
//!   render worker.
//!
//! Both are const-initialised thread-locals: reading or setting them never allocates.

use std::cell::Cell;
use std::marker::PhantomData;

thread_local! {
    static MAIN: Cell<bool> = const { Cell::new(false) };
    static AUDIO_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Marks the calling thread as the plugin main thread. Call once, on the winit thread.
pub fn mark_main_thread() {
    MAIN.with(|m| m.set(true));
}

/// Whether the calling thread is the plugin main thread.
pub fn is_main_thread() -> bool {
    MAIN.with(Cell::get)
}

/// Checks (in debug builds) that a `[main-thread]` plugin call is made from the main thread;
/// every backend wrapper calls it first.
///
/// # Panics
/// In a debug build, when the calling thread is not the main thread.
pub fn assert_main_thread(what: &str) {
    debug_assert!(
        is_main_thread(),
        "{what} must be called on the main thread, but this is not the main thread"
    );
}

/// Whether the calling thread is inside a plugin `process` call.
pub fn is_audio_thread() -> bool {
    AUDIO_DEPTH.with(|d| d.get() > 0)
}

/// RAII marker: the calling thread is an audio thread until it is dropped. Nests, and is tied to
/// the thread that made it.
#[derive(Debug)]
pub struct AudioThreadGuard {
    _not_send: PhantomData<*const ()>,
}

impl AudioThreadGuard {
    /// Enters an audio section on the calling thread.
    pub fn enter() -> Self {
        AUDIO_DEPTH.with(|d| d.set(d.get() + 1));
        Self {
            _not_send: PhantomData,
        }
    }
}

impl Drop for AudioThreadGuard {
    fn drop(&mut self) {
        AUDIO_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}
