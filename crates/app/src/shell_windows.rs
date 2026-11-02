//! The shell's windows: the rack and the console (mixer) can leave the main window for windows
//! of their own (View > Detach Rack / Detach Mixer).
//!
//! There is still one [`Shell`], one document and one view per pane; a window is a place where
//! panes are laid out. The shell owns each open window's size and scale ([`WinState`]) so it works
//! headless; the platform opens and closes the real windows to match [`Shell::windows_wanted`] and
//! reports their size and scale with [`Shell::handle_in`]. A detached pane fills its window from
//! the top left corner; the main window's layout leaves it out ([`Shell::layout`]).
//!
//! The detached flags are saved in `ui-state.json` ([`UiState::windows`]) and restored at start.
//! Detaching does not change the F6 choice or the console's place in the lower pane: attaching
//! again (the command, or closing the window: [`Shell::window_closed`]) shows the pane in the
//! main window.

use super::*;
use crate::windows::{WindowGeom, WindowKind};

/// The main window's size until the platform reports the real one.
pub(super) const MAIN_SIZE: Size = Size::new(1280.0, 800.0);
/// The size a detached rack window opens with when no geometry was saved.
const RACK_SIZE: Size = Size::new(1280.0, 720.0);
/// The size a detached console window opens with when no geometry was saved.
const CONSOLE_SIZE: Size = Size::new(1280.0, 520.0);
/// The size the Spectrum EQ window opens with when no geometry was saved.
const SPECTRUM_EQ_SIZE: (f64, f64) = atomik_ui::spectrum_eq::DEFAULT_SIZE;

/// The longest time between two writes of the window geometry caused by moving or resizing.
const GEOM_SAVE_SECONDS: f64 = 2.0;

/// What the shell keeps per open window.
#[derive(Debug, Clone)]
pub(super) struct WinState {
    /// The inner size in logical units.
    pub(super) size: Size,
    /// The window's scale factor (windows on different monitors differ).
    pub(super) scale: f64,
    /// The pane the pointer is over in this window.
    pub(super) hover: Option<Pane>,
    /// The pane that had the keys the last time this window did.
    pub(super) last_focus: Pane,
    /// The window has changes to paint.
    pub(super) dirty: bool,
}

impl WinState {
    pub(super) fn new(size: Size, scale: f64, last_focus: Pane) -> Self {
        Self {
            size,
            scale,
            hover: None,
            last_focus,
            dirty: true,
        }
    }
}

/// The pane a detached window holds.
fn pane_of(kind: WindowKind) -> Pane {
    match kind {
        WindowKind::Console => Pane::Console,
        WindowKind::SpectrumEq => Pane::SpectrumEq,
        WindowKind::Main | WindowKind::Rack => Pane::Rack,
    }
}

impl<B: AudioBackend> Shell<B> {
    fn win(&self, kind: WindowKind) -> Option<&WinState> {
        self.wins.get(&kind)
    }

    pub(super) fn win_mut(&mut self, kind: WindowKind) -> Option<&mut WinState> {
        self.wins.get_mut(&kind)
    }

    pub(super) fn hover_in(&self, window: WindowKind) -> Option<Pane> {
        self.win(window).and_then(|w| w.hover)
    }

    pub(super) fn set_hover_in(&mut self, window: WindowKind, hover: Option<Pane>) {
        if let Some(w) = self.win_mut(window) {
            w.hover = hover;
        }
    }

    /// The pointer left the pane it was over in the main window.
    pub(super) fn leave_hover(&mut self) {
        self.leave_hover_in(WindowKind::Main);
    }

    /// The pointer left the pane it was over in `window`: that pane gets a `PointerLeave`.
    pub(super) fn leave_hover_in(&mut self, window: WindowKind) {
        let hover = self.win_mut(window).and_then(|w| w.hover.take());
        if let Some(pane) = hover {
            self.forward(pane, &UiEvent::PointerLeave);
        }
    }

    pub(super) fn leave_all_hover(&mut self) {
        for window in self.wins.keys().copied().collect::<Vec<_>>() {
            self.leave_hover_in(window);
        }
    }

    /// The main window's logical size.
    pub(super) fn size(&self) -> Size {
        self.win(WindowKind::Main).map_or(MAIN_SIZE, |w| w.size)
    }

    /// The main window's scale factor.
    pub(super) fn scale(&self) -> f64 {
        self.win(WindowKind::Main).map_or(1.0, |w| w.scale)
    }

    /// The logical size of an open window (`None`: it is not open).
    pub fn window_size(&self, kind: WindowKind) -> Option<Size> {
        self.win(kind).map(|w| w.size)
    }

    /// The scale factor of an open window (`None`: it is not open).
    pub fn window_scale(&self, kind: WindowKind) -> Option<f64> {
        self.win(kind).map(|w| w.scale)
    }

    /// The physical size of an open window's content (`None`: it is not open).
    pub fn physical_size_in(&self, kind: WindowKind) -> Option<(u32, u32)> {
        self.win(kind).map(|w| {
            (
                (w.size.width * w.scale).ceil().max(1.0) as u32,
                (w.size.height * w.scale).ceil().max(1.0) as u32,
            )
        })
    }

    /// The windows that should be open: the main window, then each detached pane's. The platform
    /// opens and closes windows to match it after every call into the shell.
    pub fn windows_wanted(&self) -> Vec<WindowKind> {
        self.ui.windows.wanted()
    }

    /// Whether the pane of `kind` is in a window of its own.
    pub fn detached(&self, kind: WindowKind) -> bool {
        self.ui.windows.detached(kind)
    }

    pub(super) fn rack_detached(&self) -> bool {
        self.detached(WindowKind::Rack)
    }

    pub(super) fn console_detached(&self) -> bool {
        self.detached(WindowKind::Console)
    }

    /// The window that shows `pane`: its own while detached, else the main window.
    pub fn window_of(&self, pane: Pane) -> WindowKind {
        match pane {
            Pane::Rack if self.rack_detached() => WindowKind::Rack,
            Pane::Console if self.console_detached() => WindowKind::Console,
            Pane::SpectrumEq => WindowKind::SpectrumEq,
            _ => WindowKind::Main,
        }
    }

    /// Where `pane` is drawn: its window and its rectangle in that window's logical units (`None`
    /// while it is not shown). A detached pane fills its window.
    pub fn pane_rect(&self, pane: Pane) -> Option<(WindowKind, Rect)> {
        let window = self.window_of(pane);
        if window != WindowKind::Main {
            let size = self.window_size(window)?;
            return Some((window, Rect::new(0.0, 0.0, size.width, size.height)));
        }
        let l = self.layout();
        let rect = match pane {
            Pane::Rack => self.rack_in_main().then_some(l.rack),
            Pane::Transport => Some(l.transport),
            Pane::FileBrowser => l.file_browser,
            Pane::Console => (self.lower_pane() == Pane::Console)
                .then_some(l.song)
                .flatten(),
            song_pane => (self.lower_pane() == song_pane).then_some(l.song).flatten(),
        };
        rect.map(|r| (WindowKind::Main, r))
    }

    /// The rack shows in the main window.
    pub(super) fn rack_in_main(&self) -> bool {
        !self.rack_detached()
            && !self.maximised()
            && !(self.ui.show_song && !self.workflow.rack_visible)
    }

    /// The console shows in the main window's lower pane.
    pub(super) fn console_in_main(&self) -> bool {
        !self.console_detached() && self.ui.show_song && self.ui.show_console
    }

    /// The console shows somewhere: in its own window or in the lower pane. Its meters run then.
    pub(super) fn console_visible(&self) -> bool {
        self.console_detached() || self.console_in_main()
    }

    /// The lower pane is the console.
    pub(super) fn lower_is_console(&self) -> bool {
        self.ui.show_console && !self.console_detached()
    }

    /// Opens the state of the windows saved in `ui-state.json`: each detached pane gets its
    /// window state, sized from the saved geometry. Called once, before the first layout.
    pub(super) fn restore_windows(&mut self) {
        for kind in [
            WindowKind::Rack,
            WindowKind::Console,
            WindowKind::SpectrumEq,
        ] {
            if self.detached(kind) {
                self.open_window(kind);
            }
        }
        self.settle_detached_layout();
    }

    /// The layout flags a detached pane needs: the song area shows while the rack is away (the
    /// main window would be empty), and a maximised console is the main window's own state.
    fn settle_detached_layout(&mut self) {
        if self.rack_detached() {
            self.ui.show_song = true;
        }
        if self.console_detached() {
            self.ui.console_maximised = false;
        }
    }

    /// The state of a window that opens now: the saved size or the default, the main scale.
    fn open_window(&mut self, kind: WindowKind) {
        let saved = self
            .ui
            .windows
            .geom(kind)
            .filter(|g| g.usable())
            .map(|g| Size::new(g.width, g.height));
        let size = saved.unwrap_or(match kind {
            WindowKind::Console => CONSOLE_SIZE,
            WindowKind::SpectrumEq => Size::new(SPECTRUM_EQ_SIZE.0, SPECTRUM_EQ_SIZE.1),
            WindowKind::Main | WindowKind::Rack => RACK_SIZE,
        });
        let scale = self.scale();
        self.wins
            .insert(kind, WinState::new(size, scale, pane_of(kind)));
        self.apply_scale(kind);
    }

    /// Tells the views in `kind`'s window the window's scale.
    pub(super) fn apply_scale(&mut self, kind: WindowKind) {
        let Some(scale) = self.window_scale(kind) else {
            return;
        };
        match kind {
            WindowKind::Rack => {
                self.core.handle_event(&UiEvent::ScaleChanged(scale));
            }
            WindowKind::Console => self.core.console_mut().set_scale(scale),
            WindowKind::SpectrumEq => self.core.spectrum_eq_mut().set_scale(scale),
            WindowKind::Main => {
                self.core.arrangement_mut().set_scale(scale);
                self.core.piano_roll_mut().set_scale(scale);
                self.core.sample_editor_mut().set_scale(scale);
                self.core.step_editor_mut().set_scale(scale);
                self.core.slice_editor_mut().set_scale(scale);
                self.core.audio_editor_mut().set_scale(scale);
                self.core.seq_editor_mut().set_scale(scale);
                self.core.container_editor_mut().set_scale(scale);
                self.core.plugin_params_mut().set_scale(scale);
                self.banner.set_scale(scale);
                if let Some(v) = &mut self.plugin_settings {
                    v.set_scale(scale);
                }
                if let Some(v) = &mut self.relink {
                    v.set_scale(scale);
                }
                if let Some(v) = &mut self.export {
                    v.set_scale(scale);
                }
                if let Some(v) = &mut self.midi_maps {
                    v.set_scale(scale);
                }
                if let Some(v) = &mut self.macros_panel {
                    v.set_scale(scale);
                }
                if let Some(v) = &mut self.param_search {
                    v.set_scale(scale);
                }
                if let Some(v) = &mut self.quantize {
                    v.set_scale(scale);
                }
                if let Some(v) = &mut self.silence {
                    v.set_scale(scale);
                }
                if let Some(v) = &mut self.song_info {
                    v.set_scale(scale);
                }
                if let Some(v) = &mut self.notes {
                    v.set_scale(scale);
                }
                if let Some(v) = &mut self.warp {
                    v.set_scale(scale);
                }
                self.core.transport_bar_mut().set_scale(scale);
                self.core.file_browser_mut().set_scale(scale);
                self.core.piano_keys_mut().set_scale(scale);
                // The panes that sit in the main window while attached.
                if !self.rack_detached() {
                    self.core.handle_event(&UiEvent::ScaleChanged(scale));
                }
                if !self.console_detached() {
                    self.core.console_mut().set_scale(scale);
                }
            }
        }
        self.repaint = true;
    }

    /// Detaches or attaches the pane of `kind` (the commands and [`Self::window_closed`]).
    pub(super) fn set_detached(&mut self, kind: WindowKind, detached: bool) {
        if kind == WindowKind::Main || self.detached(kind) == detached {
            return;
        }
        let pane = pane_of(kind);
        // A menu opened in the window that goes away, or over the pane that moves, is stale.
        self.context_menu = None;
        self.capture = None;
        self.leave_all_hover();
        self.ui.windows.set_detached(kind, detached);
        if kind == WindowKind::SpectrumEq {
            // A floating window with no place in the main layout: it opens and closes on its own.
            if detached {
                self.open_window(kind);
            } else {
                self.wins.remove(&kind);
                if self.focus == pane {
                    let back = self.live_pane(Pane::Arrangement);
                    self.set_focus(back);
                }
            }
            self.layout_view();
            self.core.set_spectrum_eq_shown(detached);
            self.focused = self.window_of(self.focus);
            self.write_ui_state();
            self.repaint = true;
            return;
        }
        if detached {
            self.settle_detached_layout();
            self.open_window(kind);
        } else {
            self.wins.remove(&kind);
            match kind {
                WindowKind::Rack => self.workflow.rack_visible = true,
                WindowKind::Console => {
                    self.ui.show_song = true;
                    self.ui.show_console = true;
                }
                WindowKind::Main | WindowKind::SpectrumEq => {}
            }
            self.seq_only_prev = None;
            self.apply_scale(WindowKind::Main);
            self.save_workflow();
        }
        if self.focus == pane {
            self.set_focus(pane);
        } else {
            self.set_focus(self.live_pane(self.focus));
        }
        self.focused = self.window_of(self.focus);
        self.layout_view();
        self.write_ui_state();
    }

    /// The OS closed the window of `kind`: its pane is attached to the main window again.
    /// Closing the main window is the platform's quit; nothing happens here.
    pub fn window_closed(&mut self, kind: WindowKind) {
        self.set_detached(kind, false);
    }

    // ------------------------------------------------------------------ focus

    /// Remembers that `pane` had the keys, for the window it is in.
    pub(super) fn remember_focus(&mut self, pane: Pane) {
        let window = self.window_of(pane);
        if let Some(w) = self.win_mut(window) {
            w.last_focus = pane;
        }
    }

    /// The window the open context menu belongs to (`None`: no menu is open).
    pub fn context_menu_window(&self) -> Option<WindowKind> {
        self.context_menu.as_ref().map(|o| o.window)
    }

    /// The window that has the OS focus as far as the shell knows.
    pub fn focused_window(&self) -> WindowKind {
        self.focused
    }

    /// The OS gave the keyboard focus to `kind`'s window: the keys go to the pane that last had
    /// them there (the rack in the rack window, the console in the console window), and the
    /// keymap context follows. Global shortcuts work from every window.
    pub fn window_focused(&mut self, kind: WindowKind) {
        // A window coming to the front shows the keys as `keymap.json` has them now.
        self.reload_keymap_file();
        let Some(last) = self.win(kind).map(|w| w.last_focus) else {
            return;
        };
        self.focused = kind;
        let live = self.live_pane(last);
        let pane = if self.window_of(live) == kind {
            live
        } else {
            match kind {
                WindowKind::Main => self.live_pane(Pane::Arrangement),
                WindowKind::Rack => Pane::Rack,
                WindowKind::Console => Pane::Console,
                WindowKind::SpectrumEq => Pane::SpectrumEq,
            }
        };
        self.set_focus(pane);
        self.repaint = true;
    }

    /// Some modal panel or the Settings dialog is open (they live in the main window).
    pub(super) fn modal_open(&self) -> bool {
        self.dialog.is_some()
            || self.about.is_some()
            || self.plugin_settings.is_some()
            || self.relink.is_some()
            || self.export.is_some()
            || self.midi_maps.is_some()
            || self.macros_panel.is_some()
            || self.param_search.is_some()
            || self.quantize.is_some()
            || self.silence.is_some()
            || self.song_info.is_some()
            || self.notes.is_some()
            || self.warp.is_some()
    }

    /// What follows every call that can change the shell: a modal that opened brings the main
    /// window forward (when other windows exist), and the repaint flag is handed to the windows.
    pub(super) fn after_event(&mut self, modal_before: bool, requests: &mut Vec<ShellRequest>) {
        let raise = ShellRequest::FocusWindow(WindowKind::Main);
        if !modal_before && self.modal_open() && self.wins.len() > 1 && !requests.contains(&raise) {
            requests.push(raise);
        }
        self.fold_repaint();
    }

    /// Marks every open window as needing a paint when `repaint` was raised.
    pub(super) fn fold_repaint(&mut self) {
        if self.repaint {
            self.repaint = false;
            for w in self.wins.values_mut() {
                w.dirty = true;
            }
        }
    }

    /// The frame of `window` was painted but not presented (the surface timed out or was
    /// hidden): paint it again at the next tick.
    pub fn repaint_window(&mut self, window: WindowKind) {
        if let Some(w) = self.win_mut(window) {
            w.dirty = true;
        }
    }

    /// `window` was painted.
    pub(super) fn mark_painted(&mut self, window: WindowKind) {
        if let Some(w) = self.win_mut(window) {
            w.dirty = false;
        }
    }

    // ------------------------------------------------------------------ pointer

    /// A pointer event (or another event with no position) in a detached window: its pane takes
    /// everything; there is no menu bar, banner or divider.
    pub(super) fn pointer_in_detached(
        &mut self,
        window: WindowKind,
        event: &UiEvent,
    ) -> Vec<ShellRequest> {
        let pane = pane_of(window);
        if matches!(event, UiEvent::PointerLeave) {
            self.leave_hover_in(window);
            return Vec::new();
        }
        let Some(pos) = super::pointer_pos(event) else {
            self.forward(pane, event);
            return Vec::new();
        };
        match self.hit_in(window, pos) {
            Some(Ok(hit)) => {
                if self.hover_in(window) != Some(hit) {
                    self.leave_hover_in(window);
                    self.set_hover_in(window, Some(hit));
                }
                if matches!(event, UiEvent::PointerDown { .. }) {
                    self.focused = window;
                    self.set_focus(hit);
                    self.capture = Some((window, Capture::Pane(hit)));
                }
                self.forward(hit, event);
            }
            _ => self.leave_hover_in(window),
        }
        Vec::new()
    }

    /// A drag that can end in another window is in progress (files from the file browser or the
    /// library). While it is, the platform resolves the window under the pointer and calls
    /// [`Self::handle_routed`].
    pub fn cross_window_drag(&self) -> bool {
        self.capture.is_some_and(|(_, c)| c == Capture::FileDrag)
            && self.file_drag.as_ref().is_some_and(|d| !d.cancelled)
    }

    // ------------------------------------------------------------------ geometry

    /// Where the window of `kind` was last, as saved in `ui-state.json` (kept after the window
    /// closes, so the next detach opens it there). The platform places a window with it
    /// ([`crate::windows::place`]).
    pub fn saved_geom(&self, kind: WindowKind) -> Option<WindowGeom> {
        self.ui.windows.geom(kind).cloned()
    }

    /// The platform moved, resized, maximised or restored a window. Kept in memory; written to
    /// `ui-state.json` when a pane is detached or attached, on [`Self::shutdown`] and, from
    /// [`Self::frame_at`], at most every two seconds. A maximised window keeps its restored
    /// bounds: only the flag changes.
    pub fn window_moved(&mut self, kind: WindowKind, geom: WindowGeom) {
        if !self.wins.contains_key(&kind) {
            return;
        }
        let geom = match self.ui.windows.geom(kind) {
            Some(old) if geom.maximised => WindowGeom {
                maximised: true,
                monitor: geom.monitor.or_else(|| old.monitor.clone()),
                ..old.clone()
            },
            _ => geom,
        };
        if self.ui.windows.geom(kind) != Some(&geom) {
            self.ui.windows.set_geom(kind, geom);
            self.geom_dirty = true;
        }
    }

    /// Writes the window geometry if it changed and the last write is two seconds old.
    pub(super) fn flush_geometry(&mut self, now_seconds: f64) {
        if self.geom_dirty && now_seconds - self.geom_saved_at >= GEOM_SAVE_SECONDS {
            self.geom_dirty = false;
            self.geom_saved_at = now_seconds;
            self.write_ui_state();
        }
    }

    // ------------------------------------------------------------------ painting

    /// The title of `window`'s OS window: the main window's is [`Self::title`]; a detached pane
    /// names itself first (see [`WindowKind::title`], which serves every window of an event
    /// from one call of [`Self::title`]).
    pub fn title_in(&self, window: WindowKind) -> String {
        window.title(self.core.title())
    }

    /// A detached pane in its window: the pane from the top left corner, then the overlays.
    pub(super) fn paint_detached(&mut self, window: WindowKind, canvas: &mut dyn Canvas) {
        let Some((size, scale)) = self.win(window).map(|w| (w.size, w.scale)) else {
            return;
        };
        self.core.paint_pane(pane_of(window), canvas);
        let theme = self.core.view().theme().clone();
        canvas.push_transform(Affine::scale(scale));
        self.paint_overlays(window, canvas, &theme, size);
        canvas.pop_transform();
    }

    /// What floats over a window in logical units (the canvas is scaled): the context menu if it
    /// is open there, and the label of a drag of files while the pointer is over it.
    pub(super) fn paint_overlays(
        &self,
        window: WindowKind,
        canvas: &mut dyn Canvas,
        theme: &atomik_ui::Theme,
        size: Size,
    ) {
        if let Some(open) = &self.context_menu
            && open.window == window
        {
            open.menu.paint(canvas, theme);
        }
        if let Some(drag) = &self.file_drag
            && !drag.cancelled
            && let Some((over, pos)) = drag.pos
            && over == window
        {
            let text = drag_label(&drag.paths);
            let width = text.chars().count() as f64 * 7.0 + 16.0;
            let at = Point::new(
                (pos.x + 14.0).min((size.width - width).max(0.0)),
                (pos.y + 16.0).min(size.height - 24.0),
            );
            let label = Rect::new(at.x, at.y, at.x + width, at.y + 20.0);
            canvas.fill_with(RoundedRect::from_rect(label, 4.0), theme.browser_row);
            canvas.text(&TextRun::new(
                text,
                Point::new(at.x + 8.0, at.y + 14.0),
                12.0,
                theme.text,
            ));
        }
        self.paint_tooltip(window, canvas, theme, size);
    }
}
