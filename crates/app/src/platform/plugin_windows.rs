//! Plugin editor windows on the winit thread.
//!
//! Every editor is a **separate top-level winit window** (no wgpu surface), never a child of the
//! rack window: child-window compositing over a GPU surface is the usual source of flicker and
//! focus bugs on all three OSes. The core asks for windows ([`PluginWindowRequest`]); this module
//! creates them, hands the native handle back so the plugin embeds its view
//! ([`AppCore::plugin_window_created`]), and forwards the window's own events (resize, close).
//!
//! Handles come from `raw-window-handle` 0.6 through winit's safe `HasWindowHandle`: `Win32`
//! gives the `HWND`, `AppKit` the `NSView`, `Xcb` and `Xlib` the X11 window id. A **Wayland**
//! window has no embedding (neither CLAP nor VST3 has Wayland embedding); such a session cannot host the editor
//! here and says so: the generic parameter pane is the universal fallback (floating
//! CLAP windows and XWayland are follow-ups).
//!
//! Sizes: CLAP editors are sized in pixels on Windows and X11 and in logical units on macOS, which
//! is how the requests are applied.
//!
//! Like the rest of the platform layer this is not unit-tested (it needs a window); the request
//! protocol it implements is, in `app_core_plugins_gui_tests.rs`.

use crate::app_core::{AppCore, PluginWindowInfo, PluginWindowRequest};
use crate::session::{LiveSession, SystemBackend};
use atomik_model::DeviceKey;
use atomik_plugin_host::NativeParent;
use std::collections::BTreeMap;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowId};

/// The app core the windows serve.
type Core = AppCore<LiveSession<SystemBackend>>;

/// The editor windows that are open.
#[derive(Default)]
pub(super) struct PluginWindows {
    windows: BTreeMap<DeviceKey, Window>,
    ids: BTreeMap<WindowId, DeviceKey>,
}

/// The size a window is created with before the plugin says what it wants (logical pixels).
const DEFAULT_SIZE: (f64, f64) = (640.0, 480.0);

/// The handle a plugin embeds into, from a raw window handle; an error for a window system that
/// has no embedding (Wayland, and the handles that are not a window). CLAP and VST3 embed the
/// same way: `HWND` on Windows, `NSView` on macOS, an X11 window id on Linux.
// Justification: an Xlib window is a `c_ulong`, which is 32 bits on Windows (where this cast is the
// identity) and 64 bits on Linux (where the id still fits X11's 32 bits).
#[allow(clippy::unnecessary_cast)]
fn parent_from_raw(raw: RawWindowHandle) -> Result<NativeParent, String> {
    match raw {
        RawWindowHandle::Win32(h) => Ok(NativeParent::Win32(h.hwnd.get() as usize)),
        RawWindowHandle::AppKit(h) => Ok(NativeParent::Cocoa(h.ns_view.as_ptr() as usize)),
        RawWindowHandle::Xcb(h) => Ok(NativeParent::X11(h.window.get())),
        RawWindowHandle::Xlib(h) => Ok(NativeParent::X11(h.window as u32)),
        RawWindowHandle::Wayland(_) => Err(WAYLAND_MESSAGE.to_string()),
        other => Err(format!(
            "this window system cannot embed a plugin editor ({other:?})"
        )),
    }
}

/// What a Wayland session is told (neither CLAP nor VST3 embeds into a Wayland surface).
const WAYLAND_MESSAGE: &str = "Wayland sessions cannot embed a plugin editor (CLAP and VST3 both need an X11 window); use the PARAMS pane (or run Atomik under XWayland)";

fn native_parent(window: &Window) -> Result<NativeParent, String> {
    let handle = window
        .window_handle()
        .map_err(|e| format!("no native window handle: {e}"))?;
    parent_from_raw(handle.as_raw())
}

impl PluginWindows {
    /// Whether `id` is one of the editor windows.
    pub(super) fn owns(&self, id: WindowId) -> bool {
        self.ids.contains_key(&id)
    }

    /// Whether any editor window is open.
    pub(super) fn any(&self) -> bool {
        !self.windows.is_empty()
    }

    /// Fulfils the core's window requests, in order.
    pub(super) fn pump(&mut self, event_loop: &ActiveEventLoop, core: &mut Core) {
        for request in core.take_plugin_window_requests() {
            match request {
                PluginWindowRequest::Open {
                    device,
                    title,
                    remembered,
                } => self.open(event_loop, core, device, &title, remembered),
                PluginWindowRequest::Resize { device, w, h } => {
                    if let Some(window) = self.windows.get(&device) {
                        let _ = if cfg!(target_os = "macos") {
                            window.request_inner_size(LogicalSize::new(f64::from(w), f64::from(h)))
                        } else {
                            window.request_inner_size(PhysicalSize::new(w, h))
                        };
                    }
                }
                PluginWindowRequest::Show { device } => {
                    if let Some(window) = self.windows.get(&device) {
                        window.set_visible(true);
                        window.focus_window();
                    }
                }
                PluginWindowRequest::Hide { device } => {
                    if let Some(window) = self.windows.get(&device) {
                        window.set_visible(false);
                    }
                }
                PluginWindowRequest::Close { device } => self.remove(&device),
            }
        }
    }

    fn open(
        &mut self,
        event_loop: &ActiveEventLoop,
        core: &mut Core,
        device: DeviceKey,
        title: &str,
        remembered: Option<(u32, u32)>,
    ) {
        let (w, h) = remembered.map_or(DEFAULT_SIZE, |(w, h)| (f64::from(w), f64::from(h)));
        let attributes = Window::default_attributes()
            .with_title(title)
            .with_inner_size(LogicalSize::new(w, h))
            .with_visible(false);
        let window = match event_loop.create_window(attributes) {
            Ok(w) => w,
            Err(e) => {
                core.plugin_window_failed(&device, &e.to_string());
                return;
            }
        };
        let parent = match native_parent(&window) {
            Ok(p) => p,
            Err(reason) => {
                core.plugin_window_failed(&device, &reason);
                return;
            }
        };
        // CLAP applies `set_scale` on Windows and X11 only; macOS views are logical.
        let scale = if cfg!(target_os = "macos") {
            1.0
        } else {
            window.scale_factor()
        };
        self.ids.insert(window.id(), device.clone());
        self.windows.insert(device.clone(), window);
        core.plugin_window_created(&device, PluginWindowInfo { parent, scale });
    }

    fn remove(&mut self, device: &DeviceKey) {
        if let Some(window) = self.windows.remove(device) {
            self.ids.remove(&window.id());
        }
    }

    /// The window's own events: the user resizes or closes it.
    pub(super) fn event(&mut self, id: WindowId, event: WindowEvent, core: &mut Core) {
        let Some(device) = self.ids.get(&id).cloned() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                self.remove(&device);
                core.plugin_window_closed(&device);
            }
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                let (w, h) = if cfg!(target_os = "macos") {
                    let scale = self
                        .windows
                        .get(&device)
                        .map_or(1.0, |window| window.scale_factor());
                    let logical: LogicalSize<f64> = size.to_logical(scale);
                    (logical.width.round() as u32, logical.height.round() as u32)
                } else {
                    (size.width, size.height)
                };
                core.plugin_window_resized(&device, w, h);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use std::num::{NonZeroIsize, NonZeroU32};
    use std::ptr::NonNull;
    use winit::raw_window_handle::{
        AppKitWindowHandle, WaylandWindowHandle, Win32WindowHandle, XcbWindowHandle,
        XlibWindowHandle,
    };

    #[test]
    fn a_win_32_handle_is_the_hwnd_a_plugin_embeds_into() {
        let h = Win32WindowHandle::new(NonZeroIsize::new(0x00A1_1CE0).unwrap());
        assert_eq!(
            parent_from_raw(RawWindowHandle::Win32(h)),
            Ok(NativeParent::Win32(0x00A1_1CE0))
        );
    }

    #[test]
    fn an_appkit_handle_is_the_ns_view() {
        let view = NonNull::new(0x1000usize as *mut c_void).unwrap();
        assert_eq!(
            parent_from_raw(RawWindowHandle::AppKit(AppKitWindowHandle::new(view))),
            Ok(NativeParent::Cocoa(0x1000))
        );
    }

    #[test]
    fn xcb_and_xlib_handles_are_the_x11_window_id_for_both_formats() {
        let id = NonZeroU32::new(0x00A1_1CE0).unwrap();
        assert_eq!(
            parent_from_raw(RawWindowHandle::Xcb(XcbWindowHandle::new(id))),
            Ok(NativeParent::X11(0x00A1_1CE0))
        );
        assert_eq!(
            parent_from_raw(RawWindowHandle::Xlib(XlibWindowHandle::new(0x00A1_1CE0))),
            Ok(NativeParent::X11(0x00A1_1CE0))
        );
    }

    #[test]
    fn a_wayland_surface_cannot_embed_and_the_message_points_to_the_params_pane() {
        let surface = NonNull::new(0x2000usize as *mut c_void).unwrap();
        let e = parent_from_raw(RawWindowHandle::Wayland(WaylandWindowHandle::new(surface)))
            .unwrap_err();
        assert!(
            e.contains("Wayland") && e.contains("PARAMS") && e.contains("XWayland"),
            "{e}"
        );
        assert!(e.contains("VST3") && e.contains("CLAP"), "{e}");
    }

    #[test]
    fn a_handle_that_is_not_a_window_cannot_embed() {
        let e = parent_from_raw(RawWindowHandle::Web(
            winit::raw_window_handle::WebWindowHandle::new(1),
        ))
        .unwrap_err();
        assert!(e.contains("cannot embed"), "{e}");
    }
}
