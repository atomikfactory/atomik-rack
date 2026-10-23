//! The winit event loop of the child: the main thread of the plugin, and the only thread that
//! makes the editor window. The stdin reader and the ticker post their events through an
//! [`EventLoopProxy`]; window events go to the [`Control`] as [`WindowInput`].
//!
//! Handles come from `raw-window-handle` through winit's safe `HasWindowHandle`: `Win32` gives
//! the `HWND`, `AppKit` the `NSView`, `Xcb` and `Xlib` the X11 window id. A Wayland window has no
//! embedding (neither CLAP nor VST3 has Wayland embedding) and the open fails with a sentence
//! that says so.
//!
//! The windows themselves need a display and are not covered by tests; the pure parts are.

use crate::control::{Control, Event, WindowInput};
use crate::driver::{Post, spawn_reader, spawn_ticker};
use crate::editor::{NewWindow, WindowSystem, owner_hwnd, reported_size};
use atomik_plugin_host::NativeParent;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize, Size};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowAttributes, WindowId};

#[cfg(windows)]
use winit::platform::windows::WindowAttributesExtWindows;

impl Post for EventLoopProxy<Event> {
    fn post(&self, event: Event) -> bool {
        self.send_event(event).is_ok()
    }
}

/// What a Wayland session is told (neither CLAP nor VST3 embeds into a Wayland surface).
const WAYLAND_MESSAGE: &str = "Wayland sessions cannot embed a plugin editor (CLAP and VST3 both need an X11 window); run Atomik under XWayland";

/// The handle a plugin embeds into, from a raw window handle.
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

/// A size in the plugin's units: pixels, or logical points on macOS.
fn plugin_size(w: u32, h: u32) -> Size {
    if cfg!(target_os = "macos") {
        LogicalSize::new(f64::from(w), f64::from(h)).into()
    } else {
        PhysicalSize::new(w, h).into()
    }
}

/// The window system backed by one winit window, valid inside one callback of the loop.
struct Windows<'a> {
    window: &'a mut Option<Window>,
    event_loop: &'a ActiveEventLoop,
}

impl Windows<'_> {
    fn attributes(title: &str, size: (u32, u32)) -> WindowAttributes {
        Window::default_attributes()
            .with_title(title)
            .with_inner_size(plugin_size(size.0, size.1))
            .with_visible(false)
    }
}

impl WindowSystem for Windows<'_> {
    fn create(
        &mut self,
        title: &str,
        owner: Option<NativeParent>,
        size: (u32, u32),
    ) -> Result<NewWindow, String> {
        let attributes = Self::attributes(title, size);
        // Owned by the parent's window on Windows, so the editor stays above it; elsewhere the
        // window is a normal top-level window.
        #[cfg(windows)]
        let created = match owner_hwnd(owner) {
            Some(hwnd) => self
                .event_loop
                .create_window(attributes.with_owner_window(hwnd))
                // An owner that is not (or no longer) a window must not cost the editor.
                .or_else(|_| self.event_loop.create_window(Self::attributes(title, size))),
            None => self.event_loop.create_window(attributes),
        };
        #[cfg(not(windows))]
        let created = {
            let _ = owner_hwnd(owner);
            self.event_loop.create_window(attributes)
        };
        let window = created.map_err(|e| e.to_string())?;
        let handle = window
            .window_handle()
            .map_err(|e| format!("no native window handle: {e}"))?;
        let parent = parent_from_raw(handle.as_raw())?;
        let scale = window.scale_factor();
        *self.window = Some(window);
        Ok(NewWindow { parent, scale })
    }

    fn resize(&mut self, w: u32, h: u32) {
        if let Some(window) = self.window.as_ref() {
            let _ = window.request_inner_size(plugin_size(w, h));
        }
    }

    fn show(&mut self) {
        if let Some(window) = self.window.as_ref() {
            window.set_visible(true);
            window.focus_window();
        }
    }

    fn hide(&mut self) {
        if let Some(window) = self.window.as_ref() {
            window.set_visible(false);
        }
    }

    fn destroy(&mut self) {
        *self.window = None;
    }
}

struct App {
    control: Control,
    window: Option<Window>,
    tick_pending: Arc<AtomicBool>,
    exit: Option<u8>,
}

impl App {
    fn finish(&mut self, event_loop: &ActiveEventLoop, code: Option<u8>) {
        if let Some(code) = code {
            self.exit = Some(code);
            // The window goes before the loop does.
            self.window = None;
            event_loop.exit();
        }
    }
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        if self.exit.is_some() {
            return;
        }
        if matches!(event, Event::Tick) {
            self.tick_pending.store(false, Ordering::Release);
        }
        let code = {
            let mut windows = Windows {
                window: &mut self.window,
                event_loop,
            };
            self.control.on_event(event, &mut windows)
        };
        self.finish(event_loop, code);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.as_ref().filter(|w| w.id() == id) else {
            return;
        };
        let input = match event {
            WindowEvent::CloseRequested => WindowInput::CloseRequested,
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                let (w, h) = reported_size(
                    (size.width, size.height),
                    window.scale_factor(),
                    cfg!(target_os = "macos"),
                );
                WindowInput::Resized(w, h)
            }
            _ => return,
        };
        let code = {
            let mut windows = Windows {
                window: &mut self.window,
                event_loop,
            };
            self.control.on_window(input, &mut windows)
        };
        self.finish(event_loop, code);
    }
}

/// Runs the child on a winit event loop and returns its exit code. When the system cannot make an
/// event loop (no display) the control comes back for the caller to run another way.
///
/// # Errors
/// The control itself, untouched, when no event loop could be made.
pub fn run(control: Control) -> Result<u8, Box<Control>> {
    let event_loop = match EventLoop::<Event>::with_user_event().build() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("atomik-plugin-host: no window system, editors are unavailable: {e}");
            return Err(Box::new(control));
        }
    };
    let proxy = event_loop.create_proxy();
    let tick_pending = Arc::new(AtomicBool::new(false));
    spawn_reader(proxy.clone());
    spawn_ticker(proxy, Arc::clone(&tick_pending));
    let mut app = App {
        control,
        window: None,
        tick_pending,
        exit: None,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("atomik-plugin-host: the event loop failed: {e}");
        app.control_shutdown();
        return Ok(1);
    }
    Ok(app.exit.unwrap_or(0))
}

impl App {
    /// Ends the plugin when the loop failed under it.
    fn control_shutdown(&mut self) {
        let mut none = crate::driver::NoWindows;
        self.control.shutdown(&mut none);
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
    fn xcb_and_xlib_handles_are_the_x11_window_id() {
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
    fn a_wayland_surface_cannot_embed_and_the_message_says_why() {
        let surface = NonNull::new(0x2000usize as *mut c_void).unwrap();
        let e = parent_from_raw(RawWindowHandle::Wayland(WaylandWindowHandle::new(surface)))
            .unwrap_err();
        assert!(e.contains("Wayland") && e.contains("XWayland"), "{e}");
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
