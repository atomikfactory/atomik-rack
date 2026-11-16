//! The target list of a tonal balance panel (see [`crate::tonal_panel`]): the TARGET button opens
//! a list under it (the targets, then learn from files, save, rename and delete), a row picks a
//! target (the `target` param, one undo step) or asks the app for the action
//! ([`UiAction::TonalTarget`]); Save and Rename type the name in place of the button first.

use super::{RackView, Rename, RenameKind, UiAction, param_cmd};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{Key, Modifiers};
use crate::fx::FxKind;
use crate::seq_panel::{self, RenameKey};
use crate::tonal_panel::{self, MENU_W, MenuRow, ROW_H, RowAction, TonalOp};
use atomik_device_sdk::RACK_WIDTH;
use atomik_model::DeviceKey;
use vello::kurbo::{Line, Point, Rect, RoundedRect};

/// An open target list.
#[derive(Debug, Clone)]
pub(super) struct TonalMenu {
    dev: usize,
    rows: Vec<MenuRow>,
    /// Rectangle in rack coordinates.
    rect: Rect,
    hover: Option<usize>,
}

fn row_rect(menu: Rect, i: usize) -> Rect {
    let y = menu.y0 + 2.0 + i as f64 * ROW_H;
    Rect::new(menu.x0 + 2.0, y, menu.x1 - 2.0, y + ROW_H)
}

fn row_at(menu: &TonalMenu, rp: Point) -> Option<usize> {
    (0..menu.rows.len()).find(|&i| row_rect(menu.rect, i).contains(rp))
}

impl RackView {
    /// A press on the TARGET button of tonal balance panel `i`: selects the device and opens (or
    /// closes) the list under the button.
    pub(super) fn tonal_open(&mut self, i: usize, out: &mut Vec<UiAction>) {
        self.select(i, out);
        self.repaint = true;
        if self.tonal_menu.as_ref().is_some_and(|m| m.dev == i) {
            self.tonal_menu = None;
            return;
        }
        let d = &self.devices[i];
        let Some(fx) = d.spec.fx.filter(|f| f.kind == FxKind::TonalBalance) else {
            return;
        };
        let rows = tonal_panel::menu_rows(&d.analyser.tonal);
        let b = tonal_panel::geometry(fx.visual, 2).target + d.offset();
        let h = rows.len() as f64 * ROW_H + 4.0;
        let x0 =
            b.x0.max(d.x)
                .min(d.x + f64::from(RACK_WIDTH) - 16.0 - MENU_W);
        let bottom = self.view_to_rack(Point::new(0.0, self.size.height)).y;
        let y0 = if b.y1 + 2.0 + h > bottom {
            b.y0 - 2.0 - h
        } else {
            b.y1 + 2.0
        };
        self.tonal_menu = Some(TonalMenu {
            dev: i,
            rows,
            rect: Rect::new(x0, y0, x0 + MENU_W, y0 + h),
            hover: None,
        });
    }

    /// A press while the list is open: a row does its action; any press closes the list and is
    /// consumed. Returns whether a press was consumed.
    pub(super) fn tonal_press(&mut self, pos: Point, out: &mut Vec<UiAction>) -> bool {
        let Some(menu) = self.tonal_menu.take() else {
            return false;
        };
        self.repaint = true;
        let rp = self.view_to_rack(pos);
        let Some(row) = row_at(&menu, rp).map(|k| menu.rows[k].clone()) else {
            return true;
        };
        if !row.enabled {
            return true;
        }
        let key = self.devices[menu.dev].key.clone();
        match row.action {
            RowAction::Pick(value) => out.push(param_cmd(&key, "hidden.target", value, None)),
            RowAction::LearnFiles => out.push(UiAction::TonalTarget {
                device: key,
                op: TonalOp::LearnFiles,
            }),
            RowAction::Delete => out.push(UiAction::TonalTarget {
                device: key,
                op: TonalOp::Delete,
            }),
            RowAction::Save => self.begin_target_name(menu.dev, 0, "Learned target"),
            RowAction::Rename => {
                let name = self.devices[menu.dev].analyser.tonal.target_name.clone();
                self.begin_target_name(menu.dev, 1, &name);
            }
        }
        true
    }

    /// Opens the field a target name is typed in (`mode` 0 saves the learned target, 1 renames).
    fn begin_target_name(&mut self, i: usize, mode: u8, text: &str) {
        self.rename = Some(Rename {
            dev: i,
            index: mode,
            field: seq_panel::name_field(text),
            kind: RenameKind::Target,
        });
        if let Some(r) = &mut self.rename {
            r.field.select_all();
        }
        self.bump(i);
    }

    /// One key press while a target name is typed.
    pub(super) fn tonal_rename_key(
        &mut self,
        mut r: Rename,
        key: &Key,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        match seq_panel::rename_key(&mut r.field, key, modifiers) {
            RenameKey::Editing => {
                let dev = r.dev;
                self.rename = Some(r);
                self.bump(dev);
            }
            RenameKey::Commit => {
                self.bump(r.dev);
                let name = r.field.text().trim().to_string();
                if !name.is_empty() {
                    out.push(UiAction::TonalTarget {
                        device: self.devices[r.dev].key.clone(),
                        op: if r.index == 0 {
                            TonalOp::Save(name)
                        } else {
                            TonalOp::Rename(name)
                        },
                    });
                }
            }
            RenameKey::Cancel => self.bump(r.dev),
        }
    }

    pub(super) fn tonal_hover(&mut self, pos: Point) {
        let rp = self.view_to_rack(pos);
        if let Some(menu) = &mut self.tonal_menu {
            let hover = row_at(menu, rp);
            if hover != menu.hover {
                menu.hover = hover;
                self.repaint = true;
            }
        }
    }

    /// Closes the list (Escape).
    pub(super) fn tonal_close(&mut self) -> bool {
        self.tonal_menu.take().is_some()
    }

    /// Paints the open list (rack coordinates).
    pub(super) fn paint_tonal_menu(&self, c: &mut dyn Canvas) {
        let Some(m) = &self.tonal_menu else { return };
        let theme = &self.theme;
        c.fill_with(RoundedRect::from_rect(m.rect, 4.0), theme.tooltip_bg);
        c.stroke_with(
            RoundedRect::from_rect(m.rect, 4.0),
            1.0,
            theme.ink.with_alpha(0.14),
        );
        for (k, row) in m.rows.iter().enumerate() {
            let r = row_rect(m.rect, k);
            if row.rule {
                c.stroke_with(
                    Line::new(pt(r.x0 + 4.0, r.y0), pt(r.x1 - 4.0, r.y0)),
                    1.0,
                    theme.ink.with_alpha(0.14),
                );
            }
            if m.hover == Some(k) && row.enabled {
                c.fill_with(
                    RoundedRect::from_rect(r.inset(-1.0), 3.0),
                    theme.browser_row,
                );
            }
            c.text(
                &TextRun::new(
                    if row.checked { "\u{2022}" } else { "" },
                    pt(r.x0 + 6.0, r.y1 - 4.5),
                    9.5,
                    theme.highlight,
                )
                .weight(700),
            );
            let colour = if row.enabled {
                theme.text
            } else {
                theme.text_muted.with_alpha(0.55)
            };
            let width = r.width() - 24.0;
            let run = if row.verbatim {
                TextRun::verbatim(
                    crate::paint::fit_label_raw(&row.label, width, 9.5),
                    pt(r.x0 + 16.0, r.y1 - 4.5),
                    9.5,
                    colour,
                )
            } else {
                TextRun::new(
                    crate::paint::fit_label_raw(&crate::i18n::tr(&row.label), width, 9.5),
                    pt(r.x0 + 16.0, r.y1 - 4.5),
                    9.5,
                    colour,
                )
            };
            c.text(
                &run.align(Align::Start)
                    .weight(if row.checked { 700 } else { 400 }),
            );
        }
    }

    /// The labels of the open target list (empty when closed).
    pub fn tonal_menu_labels(&self) -> Vec<String> {
        self.tonal_menu.as_ref().map_or_else(Vec::new, |m| {
            m.rows.iter().map(|r| r.label.clone()).collect()
        })
    }

    /// Centre of row `i` of the open target list (logical view coordinates).
    pub fn tonal_menu_row_center(&self, i: usize) -> Option<Point> {
        let m = self.tonal_menu.as_ref()?;
        (i < m.rows.len()).then(|| self.rack_to_view(row_rect(m.rect, i).center()))
    }

    /// Centre of the TARGET button of a tonal balance panel (logical view coordinates).
    pub fn tonal_target_center(&self, key: &DeviceKey) -> Option<Point> {
        if self.flip != super::Flip::Front {
            return None;
        }
        let d = &self.devices[self.device_index(key)?];
        let fx = d.spec.fx.filter(|f| f.kind == FxKind::TonalBalance)?;
        (!d.folded).then(|| {
            self.rack_to_view(tonal_panel::geometry(fx.visual, 2).target.center() + d.offset())
        })
    }

    /// The target name being typed on the panel of `key`.
    pub fn tonal_naming(&self, key: &DeviceKey) -> Option<&str> {
        let r = self
            .rename
            .as_ref()
            .filter(|r| r.kind == RenameKind::Target)?;
        (self.devices.get(r.dev)?.key == self.local_key(key)).then_some(r.field.text())
    }
}
