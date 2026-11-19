//! The system's cursor position, for the drags that send no cursor events.
#![allow(unsafe_code)] // Justification: `GetCursorPos` is a raw Win32 call with no safe API.

use winit::window::Window;

/// The system's cursor in `window`'s logical units (`scale` is the window's scale factor times
/// the application zoom). Winit sends no cursor events during an operating-system file drag on
/// Windows, so the platform layer reads the cursor itself while files hover and drop. `None` on
/// the platforms whose events carry the cursor during a drag, and when the system does not tell.
#[cfg(windows)]
pub(super) fn os_cursor(window: &Window, scale: f64) -> Option<atomik_ui::kurbo::Point> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut at = POINT { x: 0, y: 0 };
    // SAFETY: `at` is a valid, writable POINT for the whole call.
    if unsafe { GetCursorPos(&mut at) } == 0 {
        return None;
    }
    let origin = window.inner_position().ok()?;
    Some(crate::translate::logical(
        winit::dpi::PhysicalPosition::new(
            f64::from(at.x) - f64::from(origin.x),
            f64::from(at.y) - f64::from(origin.y),
        ),
        scale,
    ))
}

/// See the Windows version: no other platform needs the cursor from the system.
#[cfg(not(windows))]
pub(super) fn os_cursor(_window: &Window, _scale: f64) -> Option<atomik_ui::kurbo::Point> {
    None
}
