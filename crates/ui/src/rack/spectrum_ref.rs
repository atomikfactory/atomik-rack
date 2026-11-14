//! The second curve of a Spectrum device: the REF button in the corner of its plot and the list
//! it opens (Off, every console strip, the reference track of a song that has one).
//!
//! The app feeds the choices with the analysis ([`crate::analyser::AnalyserData`]: the sources,
//! the chosen one and the curve itself); a row of the list is answered as
//! [`UiAction::SpectrumCurve`], and the app saves the choice with the device (one undo step).

use super::{RackView, SC_MAX_ROWS, SC_ROW_H, UiAction};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx::FxKind;
use crate::paint;
use atomik_device_sdk::RACK_WIDTH;
use atomik_model::DeviceKey;
use vello::kurbo::{Point, Rect, RoundedRect};

/// Width of the REF button, px.
pub const BUTTON_W: f64 = 30.0;
/// Height of the REF button, px.
pub const BUTTON_H: f64 = 14.0;
/// Width of the open list, px.
const LIST_W: f64 = 170.0;

/// The REF button in the top right corner of the plot `plot` (panel coordinates).
pub fn button(plot: Rect) -> Rect {
    let top = plot.y0 + 5.0;
    Rect::new(plot.x1 - 6.0 - BUTTON_W, top, plot.x1 - 6.0, top + BUTTON_H)
}

/// An open list.
#[derive(Debug, Clone)]
pub(super) struct RefMenu {
    dev: usize,
    labels: Vec<String>,
    current: usize,
    /// Rectangle in rack coordinates.
    rect: Rect,
    hover: Option<usize>,
}

fn row_rect(menu: Rect, i: usize) -> Rect {
    let y = menu.y0 + 2.0 + i as f64 * SC_ROW_H;
    Rect::new(menu.x0 + 2.0, y, menu.x1 - 2.0, y + SC_ROW_H)
}

fn row_at(menu: &RefMenu, rp: Point) -> Option<usize> {
    (0..menu.labels.len().min(SC_MAX_ROWS)).find(|&i| row_rect(menu.rect, i).contains(rp))
}

impl RackView {
    /// A press on the REF button of Spectrum panel `i`: selects the device and opens (or closes)
    /// the list under the button.
    pub(super) fn ref_open(&mut self, i: usize, out: &mut Vec<UiAction>) {
        self.select(i, out);
        self.repaint = true;
        if self.ref_menu.as_ref().is_some_and(|m| m.dev == i) {
            self.ref_menu = None;
            return;
        }
        let d = &self.devices[i];
        let Some(fx) = d.spec.fx.filter(|f| f.kind == FxKind::Spectrum) else {
            return;
        };
        let labels: Vec<String> = d
            .analyser
            .second_sources
            .iter()
            .map(|s| s.label())
            .collect();
        if labels.is_empty() {
            return;
        }
        let b = button(fx.plot) + d.offset();
        let rows = labels.len().min(SC_MAX_ROWS);
        let h = rows as f64 * SC_ROW_H + 4.0;
        let x0 = (b.x1 - LIST_W)
            .max(d.x)
            .min(d.x + f64::from(RACK_WIDTH) - 16.0 - LIST_W);
        let bottom = self.view_to_rack(Point::new(0.0, self.size.height)).y;
        let y0 = if b.y1 + 2.0 + h > bottom {
            b.y0 - 2.0 - h
        } else {
            b.y1 + 2.0
        };
        self.ref_menu = Some(RefMenu {
            dev: i,
            labels,
            current: d.analyser.second_source,
            rect: Rect::new(x0, y0, x0 + LIST_W, y0 + h),
            hover: None,
        });
    }

    /// A press while the list is open: a row is answered as [`UiAction::SpectrumCurve`]; any press
    /// closes the list and is consumed. Returns whether a press was consumed.
    pub(super) fn ref_press(&mut self, pos: Point, out: &mut Vec<UiAction>) -> bool {
        let Some(menu) = self.ref_menu.take() else {
            return false;
        };
        self.repaint = true;
        let rp = self.view_to_rack(pos);
        if let Some(index) = row_at(&menu, rp) {
            out.push(UiAction::SpectrumCurve {
                device: self.devices[menu.dev].key.clone(),
                index,
            });
        }
        true
    }

    pub(super) fn ref_hover(&mut self, pos: Point) {
        let rp = self.view_to_rack(pos);
        if let Some(menu) = &mut self.ref_menu {
            let hover = row_at(menu, rp);
            if hover != menu.hover {
                menu.hover = hover;
                self.repaint = true;
            }
        }
    }

    /// Paints the open list (rack coordinates).
    pub(super) fn paint_ref_menu(&self, c: &mut dyn Canvas) {
        let Some(m) = &self.ref_menu else { return };
        let theme = &self.theme;
        c.fill_with(RoundedRect::from_rect(m.rect, 4.0), theme.tooltip_bg);
        c.stroke_with(
            RoundedRect::from_rect(m.rect, 4.0),
            1.0,
            theme.ink.with_alpha(0.14),
        );
        for (k, label) in m.labels.iter().take(SC_MAX_ROWS).enumerate() {
            let r = row_rect(m.rect, k);
            if m.hover == Some(k) {
                c.fill_with(
                    RoundedRect::from_rect(r.inset(-1.0), 3.0),
                    theme.browser_row,
                );
            }
            let chosen = k == m.current;
            c.text(
                &TextRun::new(
                    if chosen { "\u{2022}" } else { "" },
                    pt(r.x0 + 6.0, r.y1 - 5.5),
                    9.5,
                    theme.highlight,
                )
                .weight(700),
            );
            c.text(
                &TextRun::verbatim(
                    paint::fit_label_raw(label, r.width() - 24.0, 9.5),
                    pt(r.x0 + 16.0, r.y1 - 5.5),
                    9.5,
                    theme.text,
                )
                .align(Align::Start)
                .weight(if chosen { 700 } else { 400 }),
            );
        }
    }

    /// Closes the list (another press elsewhere, Escape, a rebuild).
    pub(super) fn ref_close(&mut self) -> bool {
        self.ref_menu.take().is_some()
    }

    /// The labels of the open REF list (empty when closed).
    pub fn spectrum_ref_labels(&self) -> Vec<String> {
        self.ref_menu
            .as_ref()
            .map_or_else(Vec::new, |m| m.labels.clone())
    }

    /// Centre of row `i` of the open REF list (logical view coordinates).
    pub fn spectrum_ref_row_center(&self, i: usize) -> Option<Point> {
        let m = self.ref_menu.as_ref()?;
        (i < m.labels.len().min(SC_MAX_ROWS))
            .then(|| self.rack_to_view(row_rect(m.rect, i).center()))
    }

    /// Centre of the REF button of a Spectrum panel (logical view coordinates).
    pub fn spectrum_ref_button_center(&self, key: &DeviceKey) -> Option<Point> {
        if self.flip != super::Flip::Front {
            return None;
        }
        let d = &self.devices[self.device_index(key)?];
        let fx = d.spec.fx.filter(|f| f.kind == FxKind::Spectrum)?;
        (!d.folded).then(|| self.rack_to_view(button(fx.plot).center() + d.offset()))
    }
}
