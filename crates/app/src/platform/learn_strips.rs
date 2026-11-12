//! The Learn strips of the plugin editor windows, on the winit thread.
//!
//! Every open editor gets a thin app-drawn strip ([`atomik_ui::PluginLearnStrip`]) **docked to
//! the top edge of its window**: a small borderless window of the app's own, with a wgpu surface
//! of its own, placed right above the editor's frame and following it (moved and sized every
//! frame from the editor's outer frame). The same strip serves both kinds of editor:
//!
//! * an **embedded** editor (our own top-level window, see `plugin_windows.rs`): the frame is
//!   read from that window;
//! * an **external** editor (the window of the plugin's own process): the process reports its
//!   window's frame ([`AppCore::external_editor_frames`]).
//!
//! The strip never overlaps the plugin's view, so no surface is ever composited over a plugin
//! (the source of flicker and focus bugs this layer avoids), and the plugin's view keeps its
//! window to itself (a child area inside the editor window would need a native child window,
//! which the app cannot make without `unsafe`). On Windows the strip is owned by the editor's
//! window (embedded) or the app window (external), so it stays above them and minimises with
//! them, and it is shown without being activated: it takes the focus only when it is clicked.
//! Where the frame is unknown (Wayland gives no window positions) there is no strip; the
//! parameter pane's LEARN toggles are the fallback there.
//!
//! When the editor's top is too close to the top of its screen for the strip, it docks to the
//! bottom edge instead ([`strip_rect`]).
//!
//! Like the rest of the platform layer the windows are not unit-tested; the placement is.

use super::gpu::{FrameResult, GpuContext, WindowSurface};
use crate::app_core::AppCore;
use crate::session::{LiveSession, SystemBackend};
use crate::windows::DocId;
use atomik_model::DeviceKey;
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    LEARN_STRIP_HEIGHT, Modifiers, PluginLearnStrip, PointerButton, SceneCanvas, TextEngine, Theme,
    UiEvent,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use vello::Scene;
use vello::peniko::Color;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

#[cfg(windows)]
use winit::platform::windows::WindowAttributesExtWindows;

/// The app core the strips serve.
type Core = AppCore<LiveSession<SystemBackend>>;

/// A plugin editor: the song it belongs to and the device it edits.
type Editor = (DocId, DeviceKey);

/// A rectangle on the screen in physical pixels: x, y, width, height.
pub(super) type Frame = (i32, i32, u32, u32);

/// Where an editor window is, for its strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Dock {
    /// The editor window's outer frame.
    pub frame: Frame,
    /// The scale factor of the editor's screen.
    pub scale: f64,
    /// The window that owns the strip (a `HWND`, Windows only).
    pub owner: Option<isize>,
}

/// The strip's place for an editor whose outer frame is `editor`: right above it, as wide as
/// it, `height` pixels high; below it when the space above is off every screen of `screens`
/// (the strip would be lost under the top of the screen). A frame on no known screen keeps the
/// place above.
pub(super) fn strip_rect(editor: Frame, height: u32, screens: &[Frame]) -> Frame {
    let (x, y, w, h) = editor;
    let above = y - height as i32;
    let on_screen = |px: i32, py: i32| {
        screens.iter().any(|&(sx, sy, sw, sh)| {
            px >= sx && py >= sy && px < sx + sw as i32 && py < sy + sh as i32
        })
    };
    if !on_screen(x.max(0), above) && on_screen(x.max(0), y) {
        (x, y + h as i32, w, height)
    } else {
        (x, above, w, height)
    }
}

/// One strip window.
struct Strip {
    /// Dropped before `window`: the surface keeps its own handle to the window.
    surface: Option<WindowSurface>,
    window: Arc<Window>,
    scene: Scene,
    view: PluginLearnStrip,
    at: Frame,
    scale: f64,
}

/// The strips that are open, one per open editor whose frame is known.
#[derive(Default)]
pub(super) struct LearnStrips {
    strips: BTreeMap<Editor, Strip>,
    ids: BTreeMap<WindowId, Editor>,
}

impl LearnStrips {
    /// Whether any strip is open.
    pub(super) fn any(&self) -> bool {
        !self.strips.is_empty()
    }

    /// Opens, moves, closes and repaints the strips of song `doc` so that each editor of `docks`
    /// has one, with what `core` says about its Learn mode.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sync(
        &mut self,
        event_loop: &ActiveEventLoop,
        gpu: &mut GpuContext,
        text: &mut TextEngine,
        theme: &Theme,
        doc: DocId,
        core: &Core,
        docks: &[(DeviceKey, Dock)],
    ) {
        let gone: Vec<Editor> = self
            .strips
            .keys()
            .filter(|(d, device)| *d == doc && !docks.iter().any(|(k, _)| k == device))
            .cloned()
            .collect();
        for editor in gone {
            self.remove(&editor);
        }
        let screens: Vec<Frame> = event_loop
            .available_monitors()
            .map(|m| {
                let (p, s) = (m.position(), m.size());
                (p.x, p.y, s.width, s.height)
            })
            .collect();
        for (device, dock) in docks {
            let height = (LEARN_STRIP_HEIGHT * dock.scale).round().max(1.0) as u32;
            let at = strip_rect(dock.frame, height, &screens);
            let editor = (doc, device.clone());
            if !self.strips.contains_key(&editor) {
                self.open(event_loop, gpu, theme, editor.clone(), at, dock);
            }
            let Some(strip) = self.strips.get_mut(&editor) else {
                continue;
            };
            if strip.at != at || (strip.scale - dock.scale).abs() > 1e-6 {
                strip
                    .window
                    .set_outer_position(PhysicalPosition::new(at.0, at.1));
                let _ = strip
                    .window
                    .request_inner_size(PhysicalSize::new(at.2, at.3));
                if let Some(surface) = strip.surface.as_mut() {
                    gpu.resize(surface, at.2, at.3);
                }
                strip.at = at;
                strip.scale = dock.scale;
                strip.view.set_scale(dock.scale);
                strip.view.set_bounds(logical_bounds(at, dock.scale));
            }
            strip.view.set_state(core.plugin_learn_state(device));
            if strip.view.needs_repaint() {
                paint(gpu, text, strip);
            }
        }
    }

    fn open(
        &mut self,
        event_loop: &ActiveEventLoop,
        gpu: &mut GpuContext,
        theme: &Theme,
        editor: Editor,
        at: Frame,
        dock: &Dock,
    ) {
        let attributes = Window::default_attributes()
            .with_title("Learn")
            .with_decorations(false)
            .with_resizable(false)
            .with_active(false)
            .with_position(PhysicalPosition::new(at.0, at.1))
            .with_inner_size(PhysicalSize::new(at.2, at.3));
        #[cfg(windows)]
        let attributes = {
            let attributes = attributes.with_skip_taskbar(true);
            match dock.owner {
                Some(hwnd) => attributes.with_owner_window(hwnd),
                None => attributes,
            }
        };
        #[cfg(all(unix, not(target_os = "macos")))]
        let attributes = {
            use winit::platform::x11::{WindowAttributesExtX11, WindowType};
            attributes.with_x11_window_type(vec![WindowType::Utility])
        };
        #[cfg(not(windows))]
        let _ = dock.owner;
        let window = match event_loop.create_window(attributes) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::warn!("the Learn strip of a plugin window could not open: {e}");
                return;
            }
        };
        let surface = match gpu.create_surface(Arc::clone(&window), at.2, at.3) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("the Learn strip of a plugin window has no surface: {e}");
                None
            }
        };
        let mut view = PluginLearnStrip::new(theme.clone());
        view.set_scale(dock.scale);
        view.set_bounds(logical_bounds(at, dock.scale));
        self.ids.insert(window.id(), editor.clone());
        self.strips.insert(
            editor,
            Strip {
                surface,
                window,
                scene: Scene::new(),
                view,
                at,
                scale: dock.scale,
            },
        );
    }

    fn remove(&mut self, editor: &Editor) {
        if let Some(strip) = self.strips.remove(editor) {
            self.ids.remove(&strip.window.id());
        }
    }

    /// Closes the strips of the songs that are not in `songs` (a song closed).
    pub(super) fn keep_songs(&mut self, songs: &[DocId]) {
        let keys: Vec<Editor> = self
            .strips
            .keys()
            .filter(|(d, _)| !songs.contains(d))
            .cloned()
            .collect();
        for editor in keys {
            self.remove(&editor);
        }
    }

    /// The song whose strip window `id` is.
    pub(super) fn doc_of(&self, id: WindowId) -> Option<DocId> {
        self.ids.get(&id).map(|(doc, _)| *doc)
    }

    /// A strip window's own events: the pointer over its buttons, a repaint.
    pub(super) fn event(
        &mut self,
        gpu: &mut GpuContext,
        text: &mut TextEngine,
        id: WindowId,
        event: WindowEvent,
        core: &mut Core,
    ) {
        let Some(editor) = self.ids.get(&id).cloned() else {
            return;
        };
        let Some(strip) = self.strips.get_mut(&editor) else {
            return;
        };
        let scale = strip.scale.max(0.1);
        let ui = match event {
            WindowEvent::CursorMoved { position, .. } => Some(UiEvent::PointerMove {
                pos: Point::new(position.x / scale, position.y / scale),
                modifiers: Modifiers::NONE,
            }),
            WindowEvent::CursorLeft { .. } => Some(UiEvent::PointerLeave),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => strip.view.hover().map(|pos| UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
                clicks: 1,
            }),
            WindowEvent::RedrawRequested => {
                paint(gpu, text, strip);
                None
            }
            WindowEvent::Resized(size) => {
                if let Some(surface) = strip.surface.as_mut() {
                    gpu.resize(surface, size.width, size.height);
                }
                paint(gpu, text, strip);
                None
            }
            _ => None,
        };
        if let Some(ui) = ui {
            for action in strip.view.handle(&ui) {
                core.handle_action(action);
            }
            strip.view.set_state(core.plugin_learn_state(&editor.1));
            if strip.view.needs_repaint() {
                paint(gpu, text, strip);
            }
        }
    }
}

/// The strip's area in its own logical units.
fn logical_bounds(at: Frame, scale: f64) -> Rect {
    let scale = scale.max(0.1);
    Rect::new(0.0, 0.0, f64::from(at.2) / scale, f64::from(at.3) / scale)
}

fn paint(gpu: &mut GpuContext, text: &mut TextEngine, strip: &mut Strip) {
    strip.scene.reset();
    {
        let mut canvas = SceneCanvas::new(&mut strip.scene, text);
        strip.view.paint(&mut canvas);
    }
    let Some(surface) = strip.surface.as_mut() else {
        return;
    };
    if gpu.render(surface, &strip.scene, Color::BLACK) == FrameResult::Presented {
        strip.view.painted();
    } else {
        strip.window.request_redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Frame = (0, 0, 1920, 1080);

    #[test]
    fn the_strip_docks_right_above_the_editor_as_wide_as_it() {
        assert_eq!(
            strip_rect((100, 200, 640, 480), 28, &[SCREEN]),
            (100, 172, 640, 28)
        );
    }

    #[test]
    fn an_editor_at_the_top_of_its_screen_gets_the_strip_below() {
        assert_eq!(
            strip_rect((100, 10, 640, 480), 28, &[SCREEN]),
            (100, 490, 640, 28)
        );
        // A second screen above the first has room for it.
        let above = (0, -1080, 1920, 1080);
        assert_eq!(
            strip_rect((100, 10, 640, 480), 28, &[SCREEN, above]),
            (100, -18, 640, 28)
        );
    }

    #[test]
    fn a_frame_on_no_known_screen_keeps_the_place_above() {
        assert_eq!(
            strip_rect((5000, 5000, 300, 200), 42, &[]),
            (5000, 4958, 300, 42)
        );
    }
}
