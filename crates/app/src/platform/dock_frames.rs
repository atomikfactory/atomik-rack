//! The frames of the plugin editors that sit inside the main window, on the winit thread.
//!
//! A docked editor is the window of the plugin's own process, made a child of an **area** window
//! of the app (see `app_core_plugin_dock.rs`); the area is a child of the song's main window.
//! Above the area sits the **title bar** of the frame: a second child window of the main window
//! with a wgpu surface of its own, drawn by [`atomik_ui::PluginLearnStrip`] (the plugin's name,
//! the Learn buttons, POP OUT and CLOSE). The two windows never overlap, so no surface is
//! composited over a plugin (the source of flicker and focus bugs this layer avoids), and the
//! area has no surface at all.
//!
//! The title bar is also the handle: dragging it moves the frame inside the main window, and
//! dragging the grip at its left end resizes the area (the core asks the plugin, which answers
//! with the size it takes). Positions are in pixels from the top-left corner of the main
//! window's client area, and a frame is kept inside it. A plugin whose process ended hides its
//! area and leaves the title bar saying so.
//!
//! Like the rest of the platform layer the windows are not unit-tested; the placement is.

use super::gpu::{FrameResult, GpuContext, WindowSurface};
use super::learn_strips::logical_bounds;
use super::native_child::child_attributes;
use super::plugin_windows::native_parent;
use crate::app_core::{AppCore, DockRequest};
use crate::session::{LiveSession, SystemBackend};
use crate::windows::DocId;
use atomik_model::DeviceKey;
use atomik_ui::kurbo::Point;
use atomik_ui::{
    DockStripHit, LEARN_STRIP_HEIGHT, Modifiers, PluginDockUi, PluginLearnStrip, PointerButton,
    SceneCanvas, TextEngine, Theme, UiEvent,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use vello::Scene;
use vello::peniko::Color;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

/// The app core the frames serve.
type Core = AppCore<LiveSession<SystemBackend>>;

/// A plugin editor: the song it belongs to and the device it edits.
type Editor = (DocId, DeviceKey);

/// Fewest pixels of a frame that stay inside the main window (so it can always be dragged back).
const KEEP_VISIBLE: i32 = 80;

/// Where the first frame opens inside the main window, and how far each next one is moved.
const FIRST_AT: (i32, i32) = (48, 64);

/// Smallest area a user can drag a frame to.
const MIN_AREA: (u32, u32) = (160, 100);

/// Where a frame's title bar (`x`, `y`) and area are, in pixels of the main window's client
/// area, for an area of `size` and a title bar `bar` pixels high, kept inside `client` (width
/// and height). The bar stays reachable even when the area does not fit.
pub(super) fn frame_place(
    at: (i32, i32),
    size: (u32, u32),
    bar: u32,
    client: (u32, u32),
) -> (i32, i32) {
    let (w, h) = (size.0 as i32, (size.1 + bar) as i32);
    let (cw, ch) = (client.0 as i32, client.1 as i32);
    let max_x = (cw - w).max(0).min(cw - KEEP_VISIBLE).max(0);
    let max_y = (ch - h).max(0).min(ch - bar as i32).max(0);
    (at.0.clamp(0, max_x), at.1.clamp(0, max_y))
}

/// The area a drag of the grip by `delta` pixels makes of `start`, at least [`MIN_AREA`].
pub(super) fn dragged_area(start: (u32, u32), delta: (i32, i32)) -> (u32, u32) {
    let grow = |from: u32, by: i32, least: u32| {
        u32::try_from((i64::from(from) + i64::from(by)).max(i64::from(least))).unwrap_or(least)
    };
    (
        grow(start.0, delta.0, MIN_AREA.0),
        grow(start.1, delta.1, MIN_AREA.1),
    )
}

/// What the pointer is doing on a title bar.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// Moving the frame: where it was, and the pointer's screen position at the start.
    Move { from: (i32, i32), start: (i32, i32) },
    /// Resizing the area: its size at the start, and the pointer's screen position.
    Resize { from: (u32, u32), start: (i32, i32) },
}

/// Where a frame's windows were last put: the title bar's place, the area's size, and whether
/// the plugin stopped.
type Applied = ((i32, i32), (u32, u32), bool);

/// One docked editor's windows.
struct Frame {
    /// Dropped before `bar`: the surface keeps its own handle to the window.
    surface: Option<WindowSurface>,
    bar: Arc<Window>,
    area: Window,
    scene: Scene,
    view: PluginLearnStrip,
    /// The title bar's top-left corner in the main window's client area (physical pixels).
    at: (i32, i32),
    /// The area's size (physical pixels).
    size: (u32, u32),
    scale: f64,
    stopped: bool,
    drag: Option<Drag>,
    /// What was last given to the windows, so only a change calls the window system.
    applied: Option<Applied>,
}

/// The frames that are open.
#[derive(Default)]
pub(super) struct DockFrames {
    frames: BTreeMap<Editor, Frame>,
    ids: BTreeMap<WindowId, Editor>,
}

impl DockFrames {
    /// Whether any frame is open.
    pub(super) fn any(&self) -> bool {
        !self.frames.is_empty()
    }

    /// Lets go of every title bar's surface (the graphics device was lost).
    pub(super) fn drop_surfaces(&mut self) {
        for frame in self.frames.values_mut() {
            frame.surface = None;
        }
    }

    /// Gives every title bar a surface on the new context and paints it (the graphics were built
    /// again); a bar that had none because its surface failed to open gets one too.
    pub(super) fn recreate_surfaces(
        &mut self,
        gpu: &mut GpuContext,
        text: &mut TextEngine,
    ) -> Result<(), String> {
        for frame in self.frames.values_mut() {
            let bar_h = frame.bar_height();
            let surface = gpu.create_surface(Arc::clone(&frame.bar), frame.size.0, bar_h, 1.0)?;
            frame.surface = Some(surface);
            frame.paint(gpu, text);
        }
        Ok(())
    }

    /// The song whose frame window `id` is (a title bar or an area).
    pub(super) fn doc_of(&self, id: WindowId) -> Option<DocId> {
        self.ids.get(&id).map(|(doc, _)| *doc)
    }

    /// Closes the frames of the songs that are not in `songs` (a song closed).
    pub(super) fn keep_songs(&mut self, songs: &[DocId]) {
        let gone: Vec<Editor> = self
            .frames
            .keys()
            .filter(|(d, _)| !songs.contains(d))
            .cloned()
            .collect();
        for editor in gone {
            self.remove(&editor);
        }
    }

    fn remove(&mut self, editor: &Editor) {
        if let Some(frame) = self.frames.remove(editor) {
            self.ids.remove(&frame.bar.id());
            self.ids.remove(&frame.area.id());
        }
    }

    /// Fulfils the requests of the core of song `doc`, then keeps every frame of the song where
    /// the core and the user put it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sync(
        &mut self,
        event_loop: &ActiveEventLoop,
        gpu: &mut GpuContext,
        text: &mut TextEngine,
        theme: &Theme,
        doc: DocId,
        main: &Window,
        scale: f64,
        core: &mut Core,
    ) {
        for request in core.take_plugin_dock_requests() {
            match request {
                DockRequest::Create { device, size, at } => {
                    self.create(
                        event_loop, gpu, theme, doc, main, scale, core, device, size, at,
                    );
                }
                DockRequest::Resize { device, w, h } => {
                    if let Some(f) = self.frames.get_mut(&(doc, device)) {
                        f.size = (w, h);
                    }
                }
                DockRequest::Stopped { device } => {
                    if let Some(f) = self.frames.get_mut(&(doc, device)) {
                        f.stopped = true;
                    }
                }
                DockRequest::Close { device } => self.remove(&(doc, device)),
            }
        }
        let client = main.inner_size();
        let editors: Vec<Editor> = self
            .frames
            .keys()
            .filter(|(d, _)| *d == doc)
            .cloned()
            .collect();
        for editor in editors {
            let Some(frame) = self.frames.get_mut(&editor) else {
                continue;
            };
            frame.sync(
                gpu,
                text,
                (client.width, client.height),
                scale,
                core,
                &editor.1,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create(
        &mut self,
        event_loop: &ActiveEventLoop,
        gpu: &mut GpuContext,
        theme: &Theme,
        doc: DocId,
        main: &Window,
        scale: f64,
        core: &mut Core,
        device: DeviceKey,
        size: (u32, u32),
        at: Option<(i32, i32)>,
    ) {
        let bar_h = (LEARN_STRIP_HEIGHT * scale).round().max(1.0) as u32;
        let place = at.unwrap_or_else(|| {
            let n = self.frames.keys().filter(|(d, _)| *d == doc).count() as i32;
            (FIRST_AT.0 + 28 * n, FIRST_AT.1 + 28 * n)
        });
        let made = (|| {
            let area_attributes = child_attributes(
                Window::default_attributes()
                    .with_title("Plugin editor")
                    .with_decorations(false)
                    .with_active(false)
                    .with_position(PhysicalPosition::new(place.0, place.1 + bar_h as i32))
                    .with_inner_size(PhysicalSize::new(size.0, size.1))
                    .with_visible(true),
                main,
            )?;
            let area = event_loop
                .create_window(area_attributes)
                .map_err(|e| e.to_string())?;
            let bar_attributes = child_attributes(
                Window::default_attributes()
                    .with_title("Plugin editor frame")
                    .with_decorations(false)
                    .with_active(false)
                    .with_position(PhysicalPosition::new(place.0, place.1))
                    .with_inner_size(PhysicalSize::new(size.0, bar_h))
                    .with_visible(true),
                main,
            )?;
            let bar = Arc::new(
                event_loop
                    .create_window(bar_attributes)
                    .map_err(|e| e.to_string())?,
            );
            let parent = native_parent(&area)?;
            Ok::<_, String>((area, bar, parent))
        })();
        let (area, bar, parent) = match made {
            Ok(m) => m,
            Err(reason) => {
                core.plugin_dock_failed(&device, &reason);
                return;
            }
        };
        let surface = match gpu.create_surface(Arc::clone(&bar), size.0, bar_h, 1.0) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("the title bar of a docked plugin editor has no surface: {e}");
                None
            }
        };
        let mut view = PluginLearnStrip::new(theme.clone());
        view.set_scale(scale);
        view.set_bounds(logical_bounds((0, 0, size.0, bar_h), scale));
        self.ids.insert(bar.id(), (doc, device.clone()));
        self.ids.insert(area.id(), (doc, device.clone()));
        self.frames.insert(
            (doc, device.clone()),
            Frame {
                surface,
                bar,
                area,
                scene: Scene::new(),
                view,
                at: place,
                size,
                scale,
                stopped: false,
                drag: None,
                applied: None,
            },
        );
        core.plugin_dock_created(&device, parent);
    }

    /// The main window of song `doc` was clicked: the keyboard goes back to it, from the editor
    /// of a plugin that had it.
    pub(super) fn main_clicked(&self, doc: DocId, main: &Window) {
        if self.frames.keys().any(|(d, _)| *d == doc) {
            super::native_child::take_focus(main);
        }
    }

    /// A frame window's own events: the title bar's buttons and drags, and its repaint.
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
        let Some(frame) = self.frames.get_mut(&editor) else {
            return;
        };
        if id != frame.bar.id() {
            return;
        }
        frame.bar_event(gpu, text, event, core, &editor.1);
    }
}

impl Frame {
    /// The title bar's height in physical pixels.
    fn bar_height(&self) -> u32 {
        (LEARN_STRIP_HEIGHT * self.scale).round().max(1.0) as u32
    }

    /// Puts the windows where the frame is, and repaints the title bar when it changed.
    fn sync(
        &mut self,
        gpu: &mut GpuContext,
        text: &mut TextEngine,
        client: (u32, u32),
        scale: f64,
        core: &mut Core,
        device: &DeviceKey,
    ) {
        if (self.scale - scale).abs() > 1e-6 {
            self.scale = scale;
            self.view.set_scale(scale);
        }
        let bar_h = self.bar_height();
        let at = frame_place(self.at, self.size, bar_h, client);
        if at != self.at {
            self.at = at;
        }
        core.plugin_dock_moved(device, at.0, at.1);
        let want = (at, self.size, self.stopped);
        if self.applied != Some(want) {
            self.applied = Some(want);
            self.bar
                .set_outer_position(PhysicalPosition::new(at.0, at.1));
            let _ = self
                .bar
                .request_inner_size(PhysicalSize::new(self.size.0, bar_h));
            if let Some(surface) = self.surface.as_mut() {
                gpu.resize(surface, self.size.0, bar_h);
            }
            self.view
                .set_bounds(logical_bounds((0, 0, self.size.0, bar_h), scale));
            self.area
                .set_outer_position(PhysicalPosition::new(at.0, at.1 + bar_h as i32));
            let _ = self
                .area
                .request_inner_size(PhysicalSize::new(self.size.0, self.size.1));
            // The area of a plugin that stopped is empty: it goes, the title bar says why.
            self.area.set_visible(!self.stopped);
        }
        self.view.set_state(core.plugin_learn_state(device));
        self.view.set_dock(core.plugin_dock_ui(device));
        if self.view.needs_repaint() {
            self.paint(gpu, text);
        }
    }

    fn paint(&mut self, gpu: &mut GpuContext, text: &mut TextEngine) {
        self.scene.reset();
        {
            let mut canvas = SceneCanvas::new(&mut self.scene, text);
            self.view.paint(&mut canvas);
        }
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        if gpu.render(surface, &self.scene, Color::BLACK) == FrameResult::Presented {
            self.view.painted();
        } else {
            self.bar.request_redraw();
        }
    }

    /// The pointer's position on the screen, from its position in the title bar window.
    fn screen(&self, local: PhysicalPosition<f64>) -> Option<(i32, i32)> {
        let origin = self.bar.outer_position().ok()?;
        Some((
            origin.x + local.x.round() as i32,
            origin.y + local.y.round() as i32,
        ))
    }

    fn bar_event(
        &mut self,
        gpu: &mut GpuContext,
        text: &mut TextEngine,
        event: WindowEvent,
        core: &mut Core,
        device: &DeviceKey,
    ) {
        let scale = self.scale.max(0.1);
        let mut ui = None;
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                if let (Some(drag), Some(now)) = (self.drag, self.screen(position)) {
                    match drag {
                        Drag::Move { from, start } => {
                            self.at = (from.0 + now.0 - start.0, from.1 + now.1 - start.1);
                        }
                        Drag::Resize { from, start } => {
                            let area = dragged_area(from, (now.0 - start.0, now.1 - start.1));
                            if area != self.size {
                                self.size = area;
                                core.plugin_dock_user_resized(device, area.0, area.1);
                            }
                        }
                    }
                } else {
                    ui = Some(UiEvent::PointerMove {
                        pos: Point::new(position.x / scale, position.y / scale),
                        modifiers: Modifiers::NONE,
                    });
                }
            }
            WindowEvent::CursorLeft { .. } => ui = Some(UiEvent::PointerLeave),
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => self.drag = None,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(pos) = self.view.hover() {
                    let here = self.bar.outer_position().ok().map(|o| {
                        (
                            o.x + (pos.x * scale).round() as i32,
                            o.y + (pos.y * scale).round() as i32,
                        )
                    });
                    match (self.view.hit(pos), here) {
                        (DockStripHit::Grip, Some(start)) => {
                            self.drag = Some(Drag::Resize {
                                from: self.size,
                                start,
                            });
                        }
                        (DockStripHit::Bar, Some(start))
                            if self.view.dock() != PluginDockUi::Hidden =>
                        {
                            self.drag = Some(Drag::Move {
                                from: self.at,
                                start,
                            });
                        }
                        _ => {
                            ui = Some(UiEvent::PointerDown {
                                pos,
                                button: PointerButton::Primary,
                                modifiers: Modifiers::NONE,
                                clicks: 1,
                            });
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => self.paint(gpu, text),
            WindowEvent::Resized(size) => {
                if let Some(surface) = self.surface.as_mut() {
                    gpu.resize(surface, size.width, size.height);
                }
                self.paint(gpu, text);
            }
            _ => {}
        }
        if let Some(ui) = ui {
            for action in self.view.handle(&ui) {
                core.handle_action(action);
            }
            self.view.set_state(core.plugin_learn_state(device));
            self.view.set_dock(core.plugin_dock_ui(device));
            if self.view.needs_repaint() {
                self.paint(gpu, text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: (u32, u32) = (1280, 800);

    #[test]
    fn a_frame_that_fits_stays_where_it_is_put() {
        assert_eq!(frame_place((48, 64), (640, 480), 28, CLIENT), (48, 64));
    }

    #[test]
    fn a_frame_is_kept_inside_the_main_window() {
        assert_eq!(frame_place((-30, -5), (640, 480), 28, CLIENT), (0, 0));
        assert_eq!(
            frame_place((900, 700), (640, 480), 28, CLIENT),
            (640, 292),
            "the whole frame stays in when it fits"
        );
    }

    #[test]
    fn a_frame_bigger_than_the_window_keeps_its_title_bar_reachable() {
        let (x, y) = frame_place((500, 500), (2000, 1500), 28, CLIENT);
        assert_eq!(x, 0, "no room to move sideways");
        assert!(y <= 800 - 28, "the title bar is inside: {y}");
        // A window that shrank under a frame that was far out brings its bar back.
        let (x, _) = frame_place((1200, 0), (400, 300), 28, (900, 600));
        assert!(x <= 900 - KEEP_VISIBLE || x == 500, "{x}");
        assert!(x <= 500, "the whole frame fits again: {x}");
    }

    #[test]
    fn dragging_the_grip_grows_and_shrinks_the_area_down_to_a_least_size() {
        assert_eq!(dragged_area((640, 480), (40, -30)), (680, 450));
        assert_eq!(dragged_area((640, 480), (-900, -900)), MIN_AREA);
        assert_eq!(dragged_area((640, 480), (0, 0)), (640, 480));
    }
}
