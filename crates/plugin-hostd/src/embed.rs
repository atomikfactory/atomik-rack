//! Turns the attributes of an editor window into those of a **child** of a window the app made
//! (the app's frame of an editor that sits inside the app window, Windows and X11).
//!
//! The one `unsafe` call of the child: winit cannot know that a native handle that crossed a
//! process boundary is a live window, so it asks the caller to vouch for it.
#![allow(unsafe_code)] // Justification: `with_parent_window` takes a raw handle on trust.

use atomik_plugin_host::NativeParent;
use std::num::NonZeroIsize;
use winit::raw_window_handle::{RawWindowHandle, Win32WindowHandle, XlibWindowHandle};
use winit::window::WindowAttributes;

/// The raw handle of the app's window `parent`; an error where the platform cannot put a window
/// of this process inside one of another (macOS) or the handle is nothing.
fn raw_parent(parent: NativeParent) -> Result<RawWindowHandle, String> {
    match parent {
        NativeParent::Win32(hwnd) => {
            let hwnd = NonZeroIsize::new(hwnd as isize)
                .ok_or_else(|| "the app's window handle is empty".to_string())?;
            Ok(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd)))
        }
        NativeParent::X11(id) if id != 0 => Ok(RawWindowHandle::Xlib(XlibWindowHandle::new(
            std::ffi::c_ulong::from(id),
        ))),
        NativeParent::X11(_) => Err("the app's window handle is empty".to_string()),
        NativeParent::Cocoa(_) => Err(
            "a window of another process cannot be put inside the app window on macOS".to_string(),
        ),
    }
}

/// `attributes` as the attributes of a frameless child of `parent`.
///
/// # Errors
/// The reason `parent` cannot hold a child of this process.
pub fn as_child(
    attributes: WindowAttributes,
    parent: NativeParent,
) -> Result<WindowAttributes, String> {
    let raw = raw_parent(parent)?;
    // SAFETY: the handle is the one the app sent for a window it made for this editor and keeps
    // until the editor is closed; a stale one only makes the window creation fail, which is
    // reported to the app as the error of the open.
    Ok(unsafe { attributes.with_parent_window(Some(raw)) }.with_decorations(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_win_32_and_an_x11_window_can_hold_a_child() {
        assert!(matches!(
            raw_parent(NativeParent::Win32(0x00A1_1CE0)),
            Ok(RawWindowHandle::Win32(_))
        ));
        assert!(matches!(
            raw_parent(NativeParent::X11(0x00A1_1CE0)),
            Ok(RawWindowHandle::Xlib(_))
        ));
    }

    #[test]
    fn an_empty_handle_and_a_mac_view_cannot() {
        assert!(raw_parent(NativeParent::Win32(0)).is_err());
        assert!(raw_parent(NativeParent::X11(0)).is_err());
        let e = raw_parent(NativeParent::Cocoa(0x1000)).unwrap_err();
        assert!(e.contains("macOS"), "{e}");
    }
}
