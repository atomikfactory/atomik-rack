//! The floating meter panels in the shell: a spectrum or phase meter over the main window for one
//! console strip or plugin device ([`atomik_ui::MeterPanelsView`]).
//!
//! The core owns the panels ([`crate::AppCore::open_meter_panel`]) and feeds the view once a
//! frame; the view takes only the events that land on a panel or on its open list and never opens
//! a window of its own. **Show Spectrum** and **Show Phase Meter** (`mixer.show_spectrum`,
//! `mixer.show_phase_meter`) open a panel on the strip selected in the console, or on the primary
//! device of the rack when that is the pane in use.

use super::*;
use atomik_ui::MeterKind;

impl<B: AudioBackend> Shell<B> {
    /// The floating meter panels (for tests and the paint).
    pub fn meter_panels_view(&self) -> &atomik_ui::MeterPanelsView {
        &self.meter_panels
    }

    /// Hands the view the core's panels; called once a frame.
    pub(super) fn sync_meter_panels(&mut self) {
        if self.meter_panels.is_empty() && self.core.meter_panel_ids().is_empty() {
            return;
        }
        let size = self.size();
        self.meter_panels
            .set_area(size.width, size.height - STATUS_BAR_HEIGHT);
        self.meter_panels.set_panels(self.core.meter_panel_states());
        if self.meter_panels.needs_repaint() {
            self.repaint = true;
        }
    }

    /// An event for the panels: `Some` when one took it.
    pub(super) fn meter_panels_event(
        &mut self,
        window: WindowKind,
        event: &UiEvent,
    ) -> Option<Vec<ShellRequest>> {
        if window != WindowKind::Main
            || self.meter_panels.is_empty()
            || self.capture.is_some()
            || self.modal_open()
        {
            return None;
        }
        if !matches!(
            event,
            UiEvent::PointerMove { .. }
                | UiEvent::PointerDown { .. }
                | UiEvent::PointerUp { .. }
                | UiEvent::PointerLeave
                | UiEvent::Wheel { .. }
        ) {
            return None;
        }
        let (taken, actions) = self.meter_panels.handle(event);
        if self.meter_panels.needs_repaint() {
            self.repaint = true;
        }
        for action in actions {
            if let UiAction::MeterPanel(a) = action {
                self.core.meter_panel_action(a);
            }
        }
        // A move over a panel is also the pointer being somewhere else's (the tooltip, the
        // hover): only a press, a wheel, a release and a drag are the panel's own.
        let own = match event {
            UiEvent::PointerMove { .. } => self.meter_panels.is_dragging(),
            _ => true,
        };
        (taken && own).then(Vec::new)
    }

    /// Paints the panels.
    pub(super) fn paint_meter_panels(&mut self, canvas: &mut dyn Canvas, theme: &atomik_ui::Theme) {
        if !self.meter_panels.is_empty() {
            self.meter_panels.paint(canvas, theme);
        }
    }

    /// `mixer.show_spectrum` and `mixer.show_phase_meter`: opens a panel on the strip selected in
    /// the console, or on the primary device of the rack (a plugin device, or a strip shown
    /// there) when the rack is the pane in use.
    pub(super) fn show_meter(&mut self, kind: MeterKind) {
        // The master is selected apart from the channels and buses.
        let master = self
            .core
            .console()
            .master_selected()
            .then(|| self.core.doc().console.master.clone())
            .flatten();
        let console = master
            .or_else(|| self.core.console().primary_strip())
            .or_else(|| self.core.console().selected_strips().into_iter().next());
        let rack = self.core.view().primary();
        let target = if self.live_pane(self.focus) == Pane::Console {
            console.or(rack)
        } else {
            rack.filter(|k| self.core.can_open_meter(k)).or(console)
        };
        match target {
            Some(key) => {
                if self.core.open_meter_panel(&key, kind).is_ok() {
                    self.close_menubars();
                }
            }
            None => self
                .core
                .report("Select a console strip or a plugin device first"),
        }
    }
}
