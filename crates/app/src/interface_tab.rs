//! The Interface tab of the settings dialog: how the application looks and how its controls
//! feel. The preferences live in `workflow.json` with the Workflow ones ([`WorkflowPrefs`]);
//! this tab edits its four fields in a draft and the dialog merges them back when it is applied.
//! Nothing is saved until the dialog's **Apply**; the View menu applies the theme and the zoom at
//! once.

use crate::keyboard_tab::{PAD, TITLE, TabOutcome};
use crate::keymap_store::{WorkflowPrefs, ZOOM_STEPS};
use atomik_ui::kurbo::{Point, Rect, RoundedRect};
use atomik_ui::widget::KnobRange;
use atomik_ui::{
    Align, Canvas, CanvasExt, Key, PointerButton, TextRun, Theme, ThemeChoice, UiEvent,
};

const ROW: f64 = 44.0;
const GAP: f64 = 6.0;
const BUTTON_W: f64 = 120.0;

/// The preferences of the tab.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiPrefs {
    /// The colour theme.
    pub theme: ThemeChoice,
    /// The zoom of every window in percent.
    pub app_zoom: u16,
    /// How far a drag has to travel.
    pub knob_range: KnobRange,
    /// Hovering a control shows its value.
    pub value_tooltips: bool,
}

impl UiPrefs {
    /// The interface preferences of `prefs`.
    pub fn of(prefs: &WorkflowPrefs) -> Self {
        Self {
            theme: prefs.theme,
            app_zoom: prefs.app_zoom,
            knob_range: prefs.knob_range,
            value_tooltips: prefs.value_tooltips,
        }
    }

    /// Writes them into `prefs`.
    pub fn apply_to(self, prefs: &mut WorkflowPrefs) {
        prefs.theme = self.theme;
        prefs.app_zoom = self.app_zoom;
        prefs.knob_range = self.knob_range;
        prefs.value_tooltips = self.value_tooltips;
    }
}

/// Which preference a row edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiRow {
    /// The colour theme (steps through the choices).
    Theme,
    /// The application zoom (steps through [`ZOOM_STEPS`]).
    Zoom,
    /// The knob range (steps through the ranges).
    KnobRange,
    /// The value tooltips (a switch).
    ValueTooltips,
}

/// One row of the tab.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The whole line.
    pub rect: Rect,
    /// The preference.
    pub which: UiRow,
    /// Its label.
    pub label: &'static str,
    /// One line saying what it does.
    pub description: &'static str,
    /// The current choice as shown on the button (empty for a switch).
    pub value: String,
    /// The button that steps to the next choice (for a switch: the check box).
    pub button: Rect,
    /// The switch is on (always false for a stepping row).
    pub on: bool,
}

/// Where the tab's parts are.
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceLayout {
    /// The rows, top to bottom.
    pub rows: Vec<Row>,
    /// The note under them.
    pub note: (Point, String),
}

/// The choice after `percent` in [`ZOOM_STEPS`] (wrapping), `back` going the other way.
pub fn step_zoom(percent: u16, back: bool) -> u16 {
    let at = ZOOM_STEPS.iter().position(|&z| z == percent).unwrap_or(1);
    let n = ZOOM_STEPS.len();
    ZOOM_STEPS[if back { (at + n - 1) % n } else { (at + 1) % n }]
}

/// The Interface tab state.
#[derive(Debug, Clone)]
pub struct InterfaceTab {
    prefs: UiPrefs,
    hover: Option<Point>,
}

impl InterfaceTab {
    /// A tab editing `prefs`.
    pub fn new(prefs: UiPrefs) -> Self {
        Self { prefs, hover: None }
    }

    /// The edited preferences.
    pub fn prefs(&self) -> UiPrefs {
        self.prefs
    }

    /// Lays the tab out inside `panel`.
    pub fn layout(&self, panel: Rect) -> InterfaceLayout {
        let x0 = panel.x0 + PAD;
        let inner = panel.width() - 2.0 * PAD;
        let mut y = panel.y0 + TITLE;
        let mut rows = Vec::new();
        for (which, label, description, value, on) in [
            (
                UiRow::Theme,
                "Theme",
                "Dark, light, or the system's setting. Applies as soon as you press Apply.",
                self.prefs.theme.label().to_string(),
                false,
            ),
            (
                UiRow::Zoom,
                "Application zoom",
                "The size of everything in every window, 75 % to 200 %.",
                format!("{} %", self.prefs.app_zoom),
                false,
            ),
            (
                UiRow::KnobRange,
                "Knob range",
                "The drag a full sweep takes; Precise takes twice the distance.",
                self.prefs.knob_range.label().to_string(),
                false,
            ),
            (
                UiRow::ValueTooltips,
                "Value tooltips",
                "Resting on a control shows its name and value, not only while it is automated or driven.",
                String::new(),
                self.prefs.value_tooltips,
            ),
        ] {
            let rect = Rect::new(x0, y, x0 + inner, y + ROW);
            let button = if which == UiRow::ValueTooltips {
                Rect::new(rect.x0 + 8.0, rect.y0 + 6.0, rect.x0 + 24.0, rect.y0 + 22.0)
            } else {
                Rect::new(rect.x1 - BUTTON_W, y + 8.0, rect.x1, y + 36.0)
            };
            rows.push(Row {
                rect,
                which,
                label,
                description,
                value,
                button,
                on,
            });
            y += ROW + GAP;
        }
        InterfaceLayout {
            rows,
            note: (
                Point::new(x0, y + 12.0),
                "Saved in workflow.json next to settings.json when you press Apply.".to_string(),
            ),
        }
    }

    fn step(&mut self, which: UiRow, back: bool) {
        match which {
            UiRow::Theme => {
                let p = &mut self.prefs;
                p.theme = if back {
                    p.theme.next().next()
                } else {
                    p.theme.next()
                };
            }
            UiRow::Zoom => self.prefs.app_zoom = step_zoom(self.prefs.app_zoom, back),
            UiRow::KnobRange => {
                let p = &mut self.prefs;
                p.knob_range = if back {
                    p.knob_range.next().next()
                } else {
                    p.knob_range.next()
                };
            }
            UiRow::ValueTooltips => self.prefs.value_tooltips = !self.prefs.value_tooltips,
        }
    }

    /// Handles an event inside `panel`.
    pub(crate) fn handle(&mut self, event: &UiEvent, panel: Rect) -> TabOutcome {
        match event {
            UiEvent::Key {
                key, pressed: true, ..
            } => match key {
                Key::Escape => return TabOutcome::Cancel,
                Key::Enter => return TabOutcome::Apply,
                _ => {}
            },
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::PointerDown {
                pos,
                button: button @ (PointerButton::Primary | PointerButton::Secondary),
                ..
            } => {
                let back = *button == PointerButton::Secondary;
                let hit = self.layout(panel).rows.into_iter().find(|r| match r.which {
                    UiRow::ValueTooltips => r.rect.contains(*pos),
                    _ => r.button.contains(*pos),
                });
                if let Some(r) = hit {
                    self.step(r.which, back);
                }
            }
            _ => {}
        }
        TabOutcome::Open
    }

    /// Paints the tab inside `panel`.
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, panel: Rect) {
        let l = self.layout(panel);
        for row in &l.rows {
            let switch = row.which == UiRow::ValueTooltips;
            if switch && self.hover.is_some_and(|p| row.rect.contains(p)) {
                canvas.fill_with(RoundedRect::from_rect(row.rect, 6.0), theme.browser_row);
            }
            if switch {
                let shape = RoundedRect::from_rect(row.button, 3.0);
                if row.on {
                    canvas.fill_with(shape, theme.focus);
                }
                canvas.stroke_with(shape, 1.2, theme.text_muted);
            }
            canvas.text(&TextRun::new(
                row.label,
                Point::new(row.rect.x0 + 36.0, row.rect.y0 + 18.0),
                13.0,
                theme.text,
            ));
            canvas.text(&TextRun::new(
                row.description,
                Point::new(row.rect.x0 + 36.0, row.rect.y0 + 36.0),
                11.0,
                theme.text_muted,
            ));
            if !switch {
                let r = row.button;
                let lit = self.hover.is_some_and(|p| r.contains(p));
                canvas.fill_with(
                    RoundedRect::from_rect(r, 6.0),
                    if lit {
                        theme.selection
                    } else {
                        theme.browser_row
                    },
                );
                canvas.text(
                    &TextRun::new(
                        row.value.clone(),
                        Point::new(r.center().x, r.y0 + 16.5),
                        12.0,
                        theme.text,
                    )
                    .align(Align::Center),
                );
            }
        }
        canvas.text(&TextRun::new(
            l.note.1.clone(),
            l.note.0,
            11.5,
            theme.text_muted,
        ));
    }
}
