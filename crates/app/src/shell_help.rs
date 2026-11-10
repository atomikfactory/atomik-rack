//! Help Mode in the shell (Help > Help Mode, Shift+F1): while it is on, a Help panel is docked
//! above the status line and shows the long help of the control under the pointer
//! ([`crate::help`]): its title, a few sentences, the shortcut of the command it runs and a
//! "More" link into the manual.
//!
//! The mode never blocks a control: pointer events go where they always go, and only a press on
//! the panel's own link is taken (it opens the manual the way Help > Documentation opens the
//! project page). The panel keeps the last control it explained while the pointer is over empty
//! space or over the panel itself, so the link can be reached. The ordinary tooltips are off
//! while the mode is on, because the panel replaces them. The mode is not saved: a new session
//! starts with it off.

use super::*;
use crate::help::Resolved;
use atomik_ui::HelpCard;
use atomik_ui::help_panel;

/// The state of Help Mode.
#[derive(Debug, Default)]
pub(super) struct HelpMode {
    /// The mode is on: the panel is docked.
    pub(super) on: bool,
    /// What the panel shows (`None`: the idle card, before any control was pointed at).
    shown: Option<Resolved>,
}

impl<B: AudioBackend> Shell<B> {
    /// Whether Help Mode is on.
    pub fn help_mode(&self) -> bool {
        self.help_mode.on
    }

    /// The card the Help panel shows now: the control last pointed at, or the idle card.
    pub fn help_card(&self) -> HelpCard {
        match &self.help_mode.shown {
            Some(resolved) => resolved.card.clone(),
            None => self.idle_card(),
        }
    }

    /// Where the "More" link of the panel opens the manual (`None`: the card has no link).
    pub fn help_link_url(&self) -> Option<&'static str> {
        self.help_mode.shown.as_ref().and_then(|r| r.url)
    }

    /// The docked panel, in the main window (`None` while the mode is off).
    pub fn help_panel_rect(&self) -> Option<Rect> {
        self.help_mode.on.then(|| {
            let size = self.size();
            let y1 = size.height - STATUS_BAR_HEIGHT;
            Rect::new(
                0.0,
                (y1 - help_panel::HELP_PANEL_HEIGHT).max(0.0),
                size.width,
                y1,
            )
        })
    }

    /// The link of the panel: where a press opens the manual (`None` without a link).
    pub fn help_link_rect(&self) -> Option<Rect> {
        help_panel::link_rect(self.help_panel_rect()?, &self.help_card())
            .filter(|_| self.help_link_url().is_some())
    }

    /// The bottom edge of the song, rack and side panels: the top of the status line, or of the
    /// Help panel above it.
    pub(super) fn content_bottom(&self) -> f64 {
        let panel = if self.help_mode.on {
            help_panel::HELP_PANEL_HEIGHT
        } else {
            0.0
        };
        self.size().height - STATUS_BAR_HEIGHT - panel
    }

    /// Switches Help Mode on or off (the panel docks or goes, and the panes are laid out again).
    pub(super) fn toggle_help_mode(&mut self) {
        self.help_mode.on = !self.help_mode.on;
        self.help_mode.shown = None;
        self.clear_tooltip();
        self.layout_view();
        self.repaint = true;
    }

    /// The card before any control was pointed at.
    fn idle_card(&self) -> HelpCard {
        let entry = crate::help::book().control("help.idle");
        let title = entry.map_or("Help Mode", |e| e.title.as_str());
        let mut text = entry.map_or_else(String::new, |e| e.text.clone());
        if let Some(chord) = self.router.keymap().shortcut_text("help.hover_help", None) {
            text.push_str(&format!(" Press {chord} to turn Help Mode off."));
        }
        HelpCard::new(title, text)
    }

    /// Follows the pointer: over a control that has help (or a modal view that has) the panel
    /// shows it; over nothing, or over the panel, it keeps what it shows.
    pub(super) fn track_help(&mut self, window: WindowKind, event: &UiEvent) {
        if !self.help_mode.on {
            return;
        }
        let UiEvent::PointerMove { pos, .. } = event else {
            return;
        };
        if window == WindowKind::Main
            && self
                .help_panel_rect()
                .is_some_and(|panel| panel.contains(*pos))
        {
            return;
        }
        let Some(help) = self.help_lookup(window, *pos, true) else {
            return;
        };
        let resolved = crate::help::resolve(&help, self.router.keymap(), self.core.registry());
        if self.help_mode.shown.as_ref() != Some(&resolved) {
            self.help_mode.shown = Some(resolved);
            self.repaint = true;
        }
    }

    /// A press on the panel: the link opens the manual, the rest of the panel takes the press
    /// and does nothing. `None` for every event that is not the panel's.
    pub(super) fn help_link_event(
        &mut self,
        window: WindowKind,
        event: &UiEvent,
    ) -> Option<Vec<ShellRequest>> {
        if window != WindowKind::Main
            || !self.help_mode.on
            || self.capture.is_some()
            || self.modal_open()
            || self.dialog.is_some()
        {
            return None;
        }
        let UiEvent::PointerDown { pos, button, .. } = event else {
            return None;
        };
        if !self.help_panel_rect()?.contains(*pos) {
            return None;
        }
        self.repaint = true;
        let on_link = *button == PointerButton::Primary
            && self.help_link_rect().is_some_and(|r| r.contains(*pos));
        match (on_link, self.help_link_url()) {
            (true, Some(url)) => Some(vec![ShellRequest::OpenUrl(url)]),
            _ => Some(Vec::new()),
        }
    }

    /// Paints the panel above the status line (logical pixels, the canvas scaled).
    pub(super) fn paint_help_panel(&self, canvas: &mut dyn Canvas, theme: &atomik_ui::Theme) {
        if let Some(rect) = self.help_panel_rect() {
            help_panel::paint(canvas, theme, rect, &self.help_card());
        }
    }
}
