//! Child windows of the main window, for the plugin editors that sit inside it.
//!
//! The two native calls of the platform layer that winit leaves to the caller: making a window
//! a child of another, and giving the keyboard back to the main window after a plugin's editor
//! (a window of another process) took it.
#![allow(unsafe_code)] // Justification: `with_parent_window` and `SetFocus` have no safe form.

use winit::raw_window_handle::HasWindowHandle;
use winit::window::{Window, WindowAttributes};

/// `attributes` as those of a child of `parent` (Windows and X11; elsewhere the window cannot
/// be created and the caller is told so).
pub(super) fn child_attributes(
    attributes: WindowAttributes,
    parent: &Window,
) -> Result<WindowAttributes, String> {
    let raw = parent
        .window_handle()
        .map_err(|e| format!("no native window handle: {e}"))?
        .as_raw();
    // SAFETY: `raw` is the handle of `parent`, which the caller keeps alive for as long as the
    // child exists (the frames are dropped before their song's window).
    Ok(unsafe { attributes.with_parent_window(Some(raw)) })
}

/// Gives the keyboard focus to `window`: a click on the main window must take it back from the
/// editor of a plugin that sits inside it, which is a window of another process.
#[cfg(windows)]
pub(super) fn take_focus(window: &Window) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use winit::raw_window_handle::RawWindowHandle;
    if let Ok(handle) = window.window_handle()
        && let RawWindowHandle::Win32(h) = handle.as_raw()
    {
        // SAFETY: the handle is a live window of this thread; the call only moves the focus.
        unsafe {
            SetFocus(h.hwnd.get() as _);
        }
    }
}

/// See the Windows version: a child window of another process there is the only one that takes
/// the keyboard away from the app, and the X11 editors give it back on their own.
#[cfg(not(windows))]
pub(super) fn take_focus(_window: &Window) {}
