//! Help tooltips in the shell: the view under the pointer says what its button does
//! ([`atomik_ui::Help`]), and a [`TipTracker`] turns a pointer that rests on one button into a
//! tooltip after [`atomik_ui::HELP_DELAY`] seconds of the `frame_at` clock.
//!
//! The Workflow preference `show_tooltips` (Settings > Workflow, default on) switches them off.
//! What it does not touch: the value readout of a knob or fader being dragged, the names of
//! jacks and presets and the other tooltips that show information rather than explain a button.

use super::*;
use atomik_ui::{Help, TipTracker};

/// The tooltip state of the shell.
#[derive(Debug, Default)]
pub(super) struct Tips {
    /// The hover-delay state machine.
    pub(super) tracker: TipTracker,
    /// The window the pointer is in.
    pub(super) window: Option<WindowKind>,
}

/// A tooltip that is on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Tooltip {
    /// The window it is shown in.
    pub window: WindowKind,
    /// The button it explains (window logical pixels).
    pub rect: Rect,
    /// The text, with the shortcut of the button's command when it has one.
    pub text: String,
}

impl<B: AudioBackend> Shell<B> {
    /// The tooltip on screen, if any.
    pub fn tooltip(&self) -> Option<Tooltip> {
        let help = self.tips.tracker.shown()?;
        Some(Tooltip {
            window: self.tips.window?,
            rect: help.rect,
            text: help.text_with(self.router.keymap()),
        })
    }

    /// Whether a tooltip waits for the pointer to rest (the platform loop keeps ticking).
    pub fn tooltip_pending(&self) -> bool {
        self.tips.tracker.pending()
    }

    /// Whether the tooltips are switched on (Settings > Workflow).
    pub fn tooltips_enabled(&self) -> bool {
        self.workflow.show_tooltips
    }

    /// Advances the hover delay to `now` (the `frame_at` clock).
    pub(super) fn tick_tooltip(&mut self, now: f64) {
        if self.tips.tracker.tick(now) {
            self.repaint = true;
        }
    }

    /// Follows the pointer and the keys: a move asks the view under the pointer for the help of
    /// its button; a press, a wheel turn or a key hides the tooltip; leaving the window drops it.
    pub(super) fn track_tooltip(&mut self, window: WindowKind, event: &UiEvent) {
        let changed = match event {
            // A button held down is a gesture: the tooltip stays dismissed through it.
            UiEvent::PointerMove { .. } if self.capture.is_some() => self.tips.tracker.dismiss(),
            UiEvent::PointerMove { pos, .. } => {
                let help = self.help_at(window, *pos);
                self.tips.window = Some(window);
                self.tips.tracker.pointer(help)
            }
            UiEvent::PointerLeave | UiEvent::Resize(_) | UiEvent::ScaleChanged(_) => {
                self.tips.tracker.clear()
            }
            UiEvent::PointerDown { .. }
            | UiEvent::Wheel { .. }
            | UiEvent::Key { pressed: true, .. }
            | UiEvent::FileHover { .. } => self.tips.tracker.dismiss(),
            _ => false,
        };
        if changed {
            self.repaint = true;
        }
    }

    /// Drops the tooltip (the preference went off, the window lost the focus).
    pub(super) fn clear_tooltip(&mut self) {
        if self.tips.tracker.clear() {
            self.repaint = true;
        }
    }

    /// What the button under `pos` of `window` does, `None` where there is none or no tooltip
    /// may show (switched off, a gesture or a menu in progress).
    fn help_at(&self, window: WindowKind, pos: Point) -> Option<Help> {
        if !self.workflow.show_tooltips
            || self.dialog.is_some()
            || self.context_menu.is_some()
            || self.capture.is_some()
            || self.file_drag.is_some()
            || self.menubar.as_ref().is_some_and(MenuBar::is_open)
        {
            return None;
        }
        if window != WindowKind::Main && self.modal_open() {
            return None;
        }
        // The modals take the whole window, in the order they take the events.
        if let Some(v) = &self.plugin_settings {
            return v.help_at(pos);
        }
        if let Some(v) = &self.macros_panel {
            return v.help_at(pos);
        }
        if let Some(v) = &self.param_search {
            return v.help_at(pos);
        }
        if let Some(v) = &self.midi_maps {
            return v.help_at(pos);
        }
        if let Some(v) = &self.relink {
            return v.help_at(pos);
        }
        if let Some(v) = &self.warp {
            return v.help_at(pos);
        }
        if let Some(v) = &self.notes {
            return v.help_at(pos);
        }
        if let Some(v) = &self.silence {
            return v.help_at(pos);
        }
        if let Some(v) = &self.quantize {
            return v.help_at(pos);
        }
        if window == WindowKind::Main && self.banner.is_visible() {
            let bar = self.bar_height();
            let banner_x = self.layout().file_browser.map_or(0.0, |r| r.x1);
            if pos.x >= banner_x && pos.y >= bar && pos.y < bar + self.banner.height() {
                return self.banner.help_at(pos);
            }
        }
        let Ok(pane) = self.hit_in(window, pos)? else {
            return None;
        };
        if pane == Pane::Rack {
            // The rack view works in its own coordinates, its origin at the top left of the
            // rack area.
            let (_, area) = self.pane_rect(Pane::Rack)?;
            let origin = area.origin().to_vec2();
            let mut help = self.core.help_in_pane(pane, pos - origin)?;
            help.rect = help.rect + origin;
            return Some(help);
        }
        self.core.help_in_pane(pane, pos)
    }

    /// Paints the tooltip of `window` over everything else (logical pixels, the canvas scaled).
    pub(super) fn paint_tooltip(
        &self,
        window: WindowKind,
        canvas: &mut dyn Canvas,
        theme: &atomik_ui::Theme,
        size: Size,
    ) {
        if let Some(tip) = self.tooltip()
            && tip.window == window
        {
            let view = Rect::new(0.0, 0.0, size.width, size.height);
            atomik_ui::tooltip::paint(canvas, theme, tip.rect, &tip.text, view);
        }
    }
}
