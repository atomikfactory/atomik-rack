//! OLE on the plugin main thread (Windows).
//!
//! A host's UI thread has OLE initialised (single-threaded apartment): drag and drop needs it,
//! and so does every editor built on VSTGUI, which creates its Direct2D, DirectWrite and WIC
//! factories through COM when the plugin module loads. Without it those factories are null and
//! opening the editor crashes. This module and `fpenv` are the only places in the host crate
//! that use `unsafe` (pinned by name in `tests/unsafe_policy.rs`).
#![allow(unsafe_code)] // Justification: `OleInitialize` is a raw Win32 call.
#![deny(clippy::undocumented_unsafe_blocks)]

use std::cell::Cell;
use windows_sys::Win32::System::Ole::OleInitialize;

thread_local! {
    static DONE: Cell<bool> = const { Cell::new(false) };
}

/// Initialises OLE (STA) on the calling thread, once per thread.
///
/// A thread that already has OLE (`S_FALSE`) or is already in another apartment mode
/// (`RPC_E_CHANGED_MODE`, a multi-threaded apartment) keeps what it has. It is never
/// uninitialised: the plugin main thread lives as long as the process.
pub fn init_on_this_thread() {
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    // SAFETY: `OleInitialize` takes a reserved pointer that must be null and has no other
    // preconditions; it only affects the calling thread.
    let _ = unsafe { OleInitialize(std::ptr::null()) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::S_FALSE;

    #[test]
    fn a_marked_thread_has_ole_initialised() {
        std::thread::spawn(|| {
            init_on_this_thread();
            // SAFETY: as in `init_on_this_thread`.
            let again = unsafe { OleInitialize(std::ptr::null()) };
            assert_eq!(again, S_FALSE, "OLE was not initialised on this thread");
        })
        .join()
        .unwrap();
    }

    #[test]
    fn initialising_twice_is_fine() {
        std::thread::spawn(|| {
            init_on_this_thread();
            init_on_this_thread();
            assert!(DONE.with(Cell::get));
        })
        .join()
        .unwrap();
    }
}
