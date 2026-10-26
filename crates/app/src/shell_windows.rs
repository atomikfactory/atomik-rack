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
use crate::windows::WindowKind;

/// The main window's size until the platform reports the real one.
pub(super) const MAIN_SIZE: Size = Size::new(1280.0, 800.0);
/// The size a detached rack window opens with when no geometry was saved.
const RACK_SIZE: Size = Size::new(1280.0, 720.0);
/// The size a detached console window opens with when no geometry was saved.
const CONSOLE_SIZE: Size = Size::new(1280.0, 520.0);

/// What the shell keeps per open window.
#[derive(Debug, Clone)]
pub(super) struct WinState {
    /// The inner size in logical units.
    pub(super) size: Size,
    /// The window's scale factor (windows on different monitors differ).
    pub(super) scale: f64,
    /// The pane the pointer is over in this window.
    pub(super) hover: Option<Pane>,
}

impl WinState {
    pub(super) fn new(size: Size, scale: f64) -> Self {
        Self {
            size,
            scale,
            hover: None,
        }
    }
}

/// The pane a detached window holds.
fn pane_of(kind: WindowKind) -> Pane {
    match kind {
        WindowKind::Console => Pane::Console,
        WindowKind::Main | WindowKind::Rack => Pane::Rack,
    }
}

impl<B: AudioBackend> Shell<B> {
    fn win(&self, kind: WindowKind) -> Option<&WinState> {
        self.wins.get(&kind)
    }

    fn win_mut(&mut self, kind: WindowKind) -> Option<&mut WinState> {
        self.wins.get_mut(&kind)
    }

    pub(super) fn main_hover(&self) -> Option<Pane> {
        self.win(WindowKind::Main).and_then(|w| w.hover)
    }

    pub(super) fn set_main_hover(&mut self, hover: Option<Pane>) {
        if let Some(w) = self.win_mut(WindowKind::Main) {
            w.hover = hover;
        }
    }

    pub(super) fn take_main_hover(&mut self) -> Option<Pane> {
        self.win_mut(WindowKind::Main).and_then(|w| w.hover.take())
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
        for kind in [WindowKind::Rack, WindowKind::Console] {
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
            WindowKind::Main | WindowKind::Rack => RACK_SIZE,
        });
        let scale = self.scale();
        self.wins.insert(kind, WinState::new(size, scale));
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
            WindowKind::Main => {
                self.core.arrangement_mut().set_scale(scale);
                self.core.piano_roll_mut().set_scale(scale);
                self.core.sample_editor_mut().set_scale(scale);
                self.core.step_editor_mut().set_scale(scale);
                self.core.slice_editor_mut().set_scale(scale);
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
                if let Some(v) = &mut self.notes {
                    v.set_scale(scale);
                }
                if let Some(v) = &mut self.warp {
                    v.set_scale(scale);
                }
                self.core.transport_bar_mut().set_scale(scale);
                self.core.file_browser_mut().set_scale(scale);
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
        self.leave_hover();
        self.ui.windows.set_detached(kind, detached);
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
                WindowKind::Main => {}
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
        self.layout_view();
        self.save_ui_state();
    }

    /// The OS closed the window of `kind`: its pane is attached to the main window again.
    /// Closing the main window is the platform's quit; nothing happens here.
    pub fn window_closed(&mut self, kind: WindowKind) {
        self.set_detached(kind, false);
    }

    /// Handles one event of the window `window` (logical units of that window). The main
    /// window's events are [`Self::handle`].
    pub fn handle_in(&mut self, window: WindowKind, event: &UiEvent) -> Vec<ShellRequest> {
        if window == WindowKind::Main {
            return self.handle(event);
        }
        if self.win(window).is_none() {
            return Vec::new();
        }
        match event {
            UiEvent::ScaleChanged(scale) => {
                if let Some(w) = self.win_mut(window) {
                    w.scale = *scale;
                }
                self.apply_scale(window);
            }
            UiEvent::Resize(size) => {
                if let Some(w) = self.win_mut(window) {
                    w.size = *size;
                }
                self.context_menu = None;
                self.layout_view();
            }
            _ => {}
        }
        Vec::new()
    }
}
