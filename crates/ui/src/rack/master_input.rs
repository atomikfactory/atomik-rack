//! Rack view code of the master panel: the control-room source pick
//! list and the EDIT button of each return.

use super::{Flip, Hit, RackView, UiAction, param_cmd};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::master_panel;
use atomik_device_sdk::RACK_WIDTH;
use atomik_model::DeviceKey;
use vello::kurbo::{Point, Rect, RoundedRect, Vec2};
use vello::peniko::Color;

/// Height of one row of a pick list.
const ROW_H: f64 = 16.0;
/// Width of a pick list.
const MENU_W: f64 = 112.0;

/// An open pick list of a long choice param (the control-room source).
#[derive(Debug, Clone)]
pub(super) struct ChoiceMenu {
    dev: usize,
    param: &'static str,
    names: &'static [&'static str],
    current: usize,
    /// The list rectangle, rack coordinates.
    rect: Rect,
    hover: Option<usize>,
}

fn row_rect(menu: Rect, i: usize) -> Rect {
    let y = menu.y0 + 2.0 + i as f64 * ROW_H;
    Rect::new(menu.x0 + 2.0, y, menu.x1 - 2.0, y + ROW_H)
}

fn row_at(menu: &ChoiceMenu, rp: Point) -> Option<usize> {
    (0..menu.names.len()).find(|&i| row_rect(menu.rect, i).contains(rp))
}

impl RackView {
    // ------------------------------------------------------------------ pick list

    /// Opens (or closes) the pick list of the choice control `slot` of device `i`, under its
    /// display (above it when there is no room below), clamped to the visible rack.
    pub(super) fn choice_open(&mut self, i: usize, slot: usize) {
        self.repaint = true;
        if self.choice_menu.as_ref().is_some_and(|m| m.dev == i) {
            self.choice_menu = None;
            return;
        }
        let Some((p, _, value)) = self.param_of(i, slot) else {
            return;
        };
        let atomik_core::Unit::Choice(names) = p.unit else {
            return;
        };
        let d = &self.devices[i];
        let cell = d.spec.front[slot].rect + Vec2::new(0.0, d.y);
        let (lcd_top, lcd_bottom) = (cell.y0 + 3.0, cell.y0 + 19.0);
        let h = names.len() as f64 * ROW_H + 4.0;
        let x0 = (cell.center().x - MENU_W / 2.0).clamp(0.0, f64::from(RACK_WIDTH) - 16.0 - MENU_W);
        let (top, bottom) = (
            self.view_to_rack(Point::new(0.0, self.rack_area().y0)).y,
            self.view_to_rack(Point::new(0.0, self.size.height)).y,
        );
        let below = lcd_bottom + 2.0;
        let mut y0 = if below + h > bottom {
            lcd_top - 2.0 - h
        } else {
            below
        };
        y0 = y0.min(bottom - h).max(top);
        let current = (value.round().max(0.0) as usize).min(names.len() - 1);
        self.choice_menu = Some(ChoiceMenu {
            dev: i,
            param: p.key,
            names,
            current,
            rect: Rect::new(x0, y0, x0 + MENU_W, y0 + h),
            hover: None,
        });
    }

    /// A press while a pick list is open: a row sets the param; any press closes the list and
    /// is consumed. Returns whether a press was consumed.
    pub(super) fn choice_press(&mut self, pos: Point, out: &mut Vec<UiAction>) -> bool {
        let Some(menu) = self.choice_menu.take() else {
            return false;
        };
        self.repaint = true;
        let rp = self.view_to_rack(pos);
        if let Some(row) = row_at(&menu, rp) {
            let key = self.devices[menu.dev].key.clone();
            out.push(param_cmd(&key, menu.param, row as f32, None));
        }
        true
    }

    pub(super) fn choice_hover(&mut self, pos: Point) {
        let rp = self.view_to_rack(pos);
        if let Some(menu) = &mut self.choice_menu {
            let hover = row_at(menu, rp);
            if hover != menu.hover {
                menu.hover = hover;
                self.repaint = true;
            }
        }
    }

    /// Paints the open pick list (rack coordinates).
    pub(super) fn paint_choice_menu(&self, c: &mut dyn Canvas) {
        let Some(m) = &self.choice_menu else { return };
        let theme = &self.theme;
        c.fill_with(RoundedRect::from_rect(m.rect, 4.0), theme.tooltip_bg);
        c.stroke_with(
            RoundedRect::from_rect(m.rect, 4.0),
            1.0,
            Color::WHITE.with_alpha(0.14),
        );
        for (k, name) in m.names.iter().enumerate() {
            let r = row_rect(m.rect, k);
            if m.hover == Some(k) {
                c.fill_with(
                    RoundedRect::from_rect(r.inset(-1.0), 3.0),
                    theme.browser_row,
                );
            }
            let current = m.current == k;
            if current {
                c.text(
                    &TextRun::new("\u{2022}", pt(r.x0 + 6.0, r.y1 - 4.5), 9.0, theme.highlight)
                        .weight(700),
                );
            }
            c.text(
                &TextRun::new(*name, pt(r.x0 + 16.0, r.y1 - 4.5), 9.0, theme.text)
                    .align(Align::Start)
                    .weight(if current { 700 } else { 400 }),
            );
        }
    }

    /// The entries of the open pick list (empty when closed).
    pub fn choice_menu_labels(&self) -> Vec<String> {
        self.choice_menu.as_ref().map_or_else(Vec::new, |m| {
            m.names.iter().map(|n| (*n).to_string()).collect()
        })
    }

    /// Centre of row `i` of the open pick list (logical view coordinates).
    pub fn choice_menu_row_center(&self, i: usize) -> Option<Point> {
        let m = self.choice_menu.as_ref()?;
        (i < m.names.len()).then(|| self.rack_to_view(row_rect(m.rect, i).center()))
    }

    /// Centre of the display of the choice param `param` of device `key` (logical view
    /// coordinates), front side only.
    pub fn choice_center(&self, key: &DeviceKey, param: &str) -> Option<Point> {
        if self.flip != Flip::Front {
            return None;
        }
        let d = &self.devices[self.device_index(key)?];
        let desc = d.desc?;
        let pc = d
            .spec
            .front
            .iter()
            .find(|pc| desc.params[d.spec.param_index(&desc, pc, d.pad)].key == param)?;
        let r = pc.rect + Vec2::new(0.0, d.y);
        Some(self.rack_to_view(Point::new(r.center().x, r.y0 + 11.0)))
    }

    // ------------------------------------------------------------------ return EDIT

    /// The device on return slot `slot` of the master panel `i`.
    fn return_device(&self, i: usize, slot: usize) -> Option<usize> {
        let (key, _) = self.devices[i].master.returns.get(slot)?.as_ref()?;
        self.devices.iter().position(|d| d.key == *key)
    }

    /// A press on a return's EDIT button selects the effect on that slot and scrolls the rack
    /// to it. A free slot does nothing.
    pub(super) fn return_edit_down(&mut self, i: usize, slot: usize, out: &mut Vec<UiAction>) {
        if let Some(j) = self.return_device(i, slot) {
            self.select(j, out);
            self.reveal_device(j);
        }
    }

    /// Scrolls the rack the least that shows device `i` from its top (all of it when it fits).
    pub(super) fn reveal_device(&mut self, i: usize) {
        let area = self.rack_area();
        let d = &self.devices[i];
        let (top, bottom) = (d.y, d.y + d.height);
        let view_top = area.y0 + super::RACK_MARGIN + top * self.zoom - self.scroll.y;
        let view_bottom = area.y0 + super::RACK_MARGIN + bottom * self.zoom - self.scroll.y;
        if view_top < area.y0 || view_bottom > area.y1 {
            self.scroll.y = area.y0 + super::RACK_MARGIN + top * self.zoom - (area.y0 + 8.0);
            self.clamp_scroll();
        }
        self.repaint = true;
    }

    /// Centre of the EDIT button of return `slot` of the master `key` (logical view
    /// coordinates), front side only.
    pub fn return_edit_center(&self, key: &DeviceKey, slot: usize) -> Option<Point> {
        if self.flip != Flip::Front {
            return None;
        }
        let d = &self.devices[self.device_index(key)?];
        let desc = d.desc?;
        let r = master_panel::edit_rects(&d.spec, &desc)
            .into_iter()
            .find(|(s, _)| *s == slot)?
            .1;
        (!d.folded).then(|| self.rack_to_view(r.center() + Vec2::new(0.0, d.y)))
    }

    /// The tooltip of a return's EDIT button.
    pub(super) fn return_edit_tooltip(&self, hit: Hit) -> Option<String> {
        let Hit::ReturnEdit(i, slot) = hit else {
            return None;
        };
        let (_, name) = self.devices[i].master.returns.get(slot)?.as_ref()?;
        Some(format!("Edit {name}"))
    }
}
