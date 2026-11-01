//! The preset bar of the parameter pane: `[<] preset name [>]` in the search row, and the list
//! the name button opens. The app gives the list and the preset loaded last
//! ([`PluginParamsView::set_presets`]); a click asks it to load one
//! ([`UiAction::LoadPluginPreset`]) or to step ([`UiAction::StepPluginPreset`]). The bar shows
//! only for a plugin that offers presets.

use super::{Hit, PAD, PluginParamsView, SEARCH_HEIGHT, TOP_BAR_HEIGHT};
use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::fit_label;
use crate::plugin_data::PresetItem;
use crate::seq_paint as sp;
use vello::kurbo::{Point, Rect, RoundedRect};

const STEP_BTN_W: f64 = 22.0;
const NAME_W: f64 = 230.0;
const ROW_H: f64 = 18.0;
/// Rows the list shows before it scrolls.
pub const LIST_ROWS: usize = 16;
/// The bar starts right of the "N of M params" text.
const BAR_X: f64 = PAD + 300.0 + 150.0;

/// The preset state of the pane.
#[derive(Debug, Default)]
pub(super) struct PresetBar {
    pub(super) list: Vec<PresetItem>,
    pub(super) current: Option<String>,
    pub(super) open: bool,
    /// Index of the first row in view.
    pub(super) first: usize,
    pub(super) hover: Option<usize>,
}

impl PluginParamsView {
    /// Sets the presets the plugin offers and the id of the one loaded last (if it is still
    /// in place). An empty list hides the bar.
    pub fn set_presets(&mut self, list: Vec<PresetItem>, current: Option<String>) {
        if self.presets.list == list && self.presets.current == current {
            return;
        }
        self.presets.list = list;
        self.presets.current = current;
        let last = self.presets.list.len().saturating_sub(1);
        self.presets.first = self.presets.first.min(last);
        if self.presets.list.is_empty() {
            self.presets.open = false;
        }
        self.repaint = true;
    }

    /// The presets the bar lists.
    pub fn presets(&self) -> &[PresetItem] {
        &self.presets.list
    }

    /// True while the preset list is open.
    pub fn preset_list_open(&self) -> bool {
        self.presets.open
    }

    /// The name of the preset loaded last, when it is one of the list.
    pub fn preset_name(&self) -> Option<&str> {
        let id = self.presets.current.as_deref()?;
        self.presets
            .list
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.as_str())
    }

    fn bar_row(&self) -> Option<Rect> {
        let b = self.bounds;
        let x0 = b.x0 + BAR_X;
        let width = 2.0 * STEP_BTN_W + NAME_W + 8.0;
        let fits = x0 + width < self.close_rect().x0;
        (!self.presets.list.is_empty() && fits).then(|| {
            let y = b.y0 + TOP_BAR_HEIGHT + 5.0;
            Rect::new(x0, y, x0 + width, y + SEARCH_HEIGHT - 10.0)
        })
    }

    /// The `<` button (previous preset).
    pub fn preset_prev_rect(&self) -> Option<Rect> {
        let r = self.bar_row()?;
        Some(Rect::new(r.x0, r.y0, r.x0 + STEP_BTN_W, r.y1))
    }

    /// The name button that opens the list.
    pub fn preset_name_rect(&self) -> Option<Rect> {
        let r = self.bar_row()?;
        Some(Rect::new(
            r.x0 + STEP_BTN_W + 4.0,
            r.y0,
            r.x1 - STEP_BTN_W - 4.0,
            r.y1,
        ))
    }

    /// The `>` button (next preset).
    pub fn preset_next_rect(&self) -> Option<Rect> {
        let r = self.bar_row()?;
        Some(Rect::new(r.x1 - STEP_BTN_W, r.y0, r.x1, r.y1))
    }

    /// The open list's outline.
    pub fn preset_list_rect(&self) -> Option<Rect> {
        if !self.presets.open {
            return None;
        }
        let name = self.preset_name_rect()?;
        let rows = self.presets.list.len().min(LIST_ROWS) as f64;
        Some(Rect::new(
            name.x0,
            name.y1 + 2.0,
            name.x1 + STEP_BTN_W + 4.0,
            name.y1 + 2.0 + rows * ROW_H + 4.0,
        ))
    }

    /// Row `k` (0 is the first in view) of the open list.
    pub fn preset_row_rect(&self, k: usize) -> Option<Rect> {
        let l = self.preset_list_rect()?;
        let y = l.y0 + 2.0 + k as f64 * ROW_H;
        Some(Rect::new(l.x0 + 2.0, y, l.x1 - 2.0, y + ROW_H))
    }

    /// The indexes of the list rows in view.
    fn preset_rows_in_view(&self) -> std::ops::Range<usize> {
        let first = self.presets.first;
        first..(first + LIST_ROWS).min(self.presets.list.len())
    }

    /// The preset control under `pos`.
    pub(super) fn preset_hit(&self, pos: Point) -> Option<Hit> {
        if let Some(l) = self.preset_list_rect()
            && l.contains(pos)
        {
            return Some(
                self.preset_rows_in_view()
                    .enumerate()
                    .find(|(k, _)| self.preset_row_rect(*k).is_some_and(|r| r.contains(pos)))
                    .map_or(Hit::None, |(_, i)| Hit::PresetRow(i)),
            );
        }
        let at = |r: Option<Rect>| r.is_some_and(|r| r.contains(pos));
        if at(self.preset_prev_rect()) {
            Some(Hit::PresetPrev)
        } else if at(self.preset_next_rect()) {
            Some(Hit::PresetNext)
        } else if at(self.preset_name_rect()) {
            Some(Hit::PresetName)
        } else {
            None
        }
    }

    /// A press while the list is open or on a preset control. Returns true when it was one
    /// (the list closes on any press).
    pub(super) fn preset_press(&mut self, hit: Hit, out: &mut Vec<UiAction>) -> bool {
        let was_open = std::mem::take(&mut self.presets.open);
        let Some(device) = self.device.clone() else {
            return was_open;
        };
        match hit {
            Hit::PresetPrev => out.push(UiAction::StepPluginPreset { device, delta: -1 }),
            Hit::PresetNext => out.push(UiAction::StepPluginPreset { device, delta: 1 }),
            Hit::PresetName => {
                self.presets.open = !was_open;
                if self.presets.open {
                    self.scroll_current_into_view();
                }
            }
            Hit::PresetRow(i) if was_open => {
                if let Some(p) = self.presets.list.get(i) {
                    out.push(UiAction::LoadPluginPreset {
                        device,
                        preset: p.id.clone(),
                    });
                }
            }
            _ => return was_open,
        }
        true
    }

    fn scroll_current_into_view(&mut self) {
        let at = self
            .presets
            .current
            .as_deref()
            .and_then(|c| self.presets.list.iter().position(|p| p.id == c));
        let first = match at {
            Some(i) if i >= LIST_ROWS => i + 1 - LIST_ROWS / 2,
            _ => 0,
        };
        self.presets.first = first.min(self.presets.list.len().saturating_sub(LIST_ROWS));
    }

    /// The wheel over the open list scrolls it. Returns true when it did.
    pub(super) fn preset_wheel(&mut self, dy: f64, pos: Point) -> bool {
        if !self.preset_list_rect().is_some_and(|l| l.contains(pos)) {
            return false;
        }
        let max = self.presets.list.len().saturating_sub(LIST_ROWS);
        let first = if dy > 0.0 {
            self.presets.first + 3
        } else {
            self.presets.first.saturating_sub(3)
        };
        self.presets.first = first.min(max);
        self.repaint = true;
        true
    }

    /// Tracks the hovered row of the open list. Returns true when it changed.
    pub(super) fn preset_hover(&mut self, pos: Point) -> bool {
        let hover = match self.preset_hit(pos) {
            Some(Hit::PresetRow(i)) => Some(i),
            _ => None,
        };
        if hover == self.presets.hover {
            return false;
        }
        self.presets.hover = hover;
        true
    }

    /// Esc closes the open list. Returns true when there was one.
    pub(super) fn preset_escape(&mut self) -> bool {
        std::mem::take(&mut self.presets.open)
    }

    /// Draws the bar in the search row.
    pub(super) fn paint_preset_bar(&self, c: &mut dyn Canvas) {
        let (Some(prev), Some(name), Some(next)) = (
            self.preset_prev_rect(),
            self.preset_name_rect(),
            self.preset_next_rect(),
        ) else {
            return;
        };
        let t = &self.st.base;
        let hot = |h: Hit| self.hover == h;
        sp::button(c, &self.st, prev, "<", None, hot(Hit::PresetPrev));
        sp::button(c, &self.st, next, ">", None, hot(Hit::PresetNext));
        let lit = self.presets.current.as_ref().map(|_| self.accent);
        sp::lcd(c, &self.st, name, lit);
        let text = self.preset_name().unwrap_or("Presets");
        sp::label(
            c,
            &fit_label(text, name.width() - 14.0, 10.0),
            name.x0 + 8.0,
            name.y0 + 15.0,
            10.0,
            if lit.is_some() { t.text } else { t.text_muted },
            Align::Start,
        );
    }

    /// Draws the open list, last so it covers the rows.
    pub(super) fn paint_preset_list(&self, c: &mut dyn Canvas) {
        let Some(l) = self.preset_list_rect() else {
            return;
        };
        let t = &self.st.base;
        c.fill_with(RoundedRect::from_rect(l, 4.0), t.tooltip_bg);
        c.stroke_with(RoundedRect::from_rect(l, 4.0), 1.0, t.ink.with_alpha(0.14));
        for (k, i) in self.preset_rows_in_view().enumerate() {
            let Some(r) = self.preset_row_rect(k) else {
                continue;
            };
            let p = &self.presets.list[i];
            if self.presets.hover == Some(i) {
                c.fill_with(RoundedRect::from_rect(r.inset(-1.0), 3.0), t.browser_row);
            }
            let label = if p.group.is_empty() {
                p.name.clone()
            } else {
                format!("{}  \u{b7}  {}", p.group, p.name)
            };
            let current = self.presets.current.as_deref() == Some(p.id.as_str());
            c.text(
                &TextRun::new(
                    fit_label(&label, r.width() - 14.0, 9.5),
                    pt(r.x0 + 8.0, r.y1 - 5.5),
                    9.5,
                    if current { self.accent } else { t.text },
                )
                .align(Align::Start),
            );
        }
    }
}
