//! Rack view code of the EQ panel's graph (see [`crate::eq_panel`]): picking a handle, the drag
//! (frequency and gain as one batch under one merge id, or the Q with Alt), the wheel, the
//! double-clicks, the right-click menu of a handle and its painting.

use super::{Drag, Flip, RackView, UiAction, param_cmd};
use crate::action::ContextTarget;
use crate::canvas::{Align, Canvas, TextRun, pt};
use crate::eq_panel::{self, BANDS, Band, KEYS, MenuRow};
use crate::event::{Modifiers, PointerButton};
use crate::fx::FxKind;
use crate::pick_list::CompactList;
use crate::widget;
use atomik_device_sdk::RACK_WIDTH;
use atomik_model::{Command, DeviceKey};
use vello::kurbo::{Point, Rect, Vec2};

/// Height of one row of the menu of a handle.
const ROW_H: f64 = 16.0;
/// Width of the menu of a handle.
const MENU_W: f64 = 120.0;

/// What a drag of a handle changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragMode {
    /// Frequency and gain together (a cut and a notch: the frequency).
    Move,
    /// The Q (the resonance of a cut), by the vertical movement (Alt-drag).
    Q,
}

/// State of the drag of a handle of the EQ graph.
#[derive(Debug, Clone)]
pub(super) struct BandDrag {
    dev: usize,
    band: usize,
    merge: u64,
    mode: DragMode,
    /// Handle position minus pointer position at the press, rack coordinates.
    grab: Vec2,
    /// The press, rack coordinates.
    origin: Point,
    /// The pointer left the 3 px dead zone.
    moved: bool,
    /// The values at the press.
    start: Band,
    /// The values last sent.
    freq: f32,
    gain: f32,
    q: f32,
}

/// The open menu of a handle.
#[derive(Debug, Clone)]
pub(super) struct EqMenu {
    dev: usize,
    rows: Vec<MenuRow>,
    list: CompactList,
}

impl RackView {
    /// The graph of EQ panel `i`: its plot (rack coordinates), range and bands.
    fn eq_state(&self, i: usize) -> Option<(Rect, f64, [Band; BANDS])> {
        let d = self.devices.get(i)?;
        let fx = d.spec.fx.filter(|f| f.kind == FxKind::Eq)?;
        let desc = d.desc?;
        Some((
            fx.plot + d.offset(),
            eq_panel::range_of(&desc, d.device.as_ref()),
            eq_panel::bands_of(&desc, d.device.as_ref()),
        ))
    }

    /// Whether panel `i` is the EQ (not the dynamic EQ).
    pub(super) fn is_eq(&self, i: usize) -> bool {
        self.devices.get(i).and_then(|d| d.fx) == Some(FxKind::Eq)
    }

    /// A command that sets several params of device `i` at once (one `SetParam` for one).
    fn eq_set_cmd(&self, i: usize, sets: &[(&str, f32)], merge: Option<u64>) -> Option<UiAction> {
        let key = &self.devices.get(i)?.key;
        let clamp = |k: &str, v: f32| self.param_of_key(i, k).map_or(v, |p| p.clamp(v));
        let mut cmds: Vec<Command> = sets
            .iter()
            .map(|(k, v)| Command::SetParam {
                key: key.clone(),
                param: (*k).to_string(),
                value: clamp(k, *v),
                merge,
            })
            .collect();
        match cmds.len() {
            0 => None,
            1 => cmds.pop().map(UiAction::Command),
            _ => Some(UiAction::Command(Command::Batch(cmds))),
        }
    }

    /// A press on the graph of an EQ panel: a handle (drag, Alt-drag, double-click resets its
    /// band, right-click opens its menu), or the empty graph (double-click puts the nearest free
    /// band there).
    #[allow(clippy::too_many_arguments, reason = "event fields are passed through")]
    pub(super) fn eq_graph_down(
        &mut self,
        i: usize,
        rp: Point,
        pos: Point,
        button: PointerButton,
        modifiers: Modifiers,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        let Some((plot, range, bands)) = self.eq_state(i) else {
            return;
        };
        let hit = eq_panel::pick(plot, range, &bands, rp);
        match (button, hit) {
            (PointerButton::Secondary, Some(band)) => {
                self.select_for_menu(i, out);
                self.eq_menu_open(i, band, &bands[band], rp);
            }
            (PointerButton::Secondary, None) => {
                self.select_for_menu(i, out);
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::Device,
                    at: pos,
                });
            }
            (PointerButton::Primary, Some(band)) if clicks >= 2 => {
                self.select(i, out);
                let Some(desc) = self.devices[i].desc else {
                    return;
                };
                let sets = eq_panel::reset_sets(&desc, band, false);
                out.extend(self.eq_set_cmd(i, &sets, None));
            }
            (PointerButton::Primary, Some(band)) => {
                let merge = self.next_merge;
                self.next_merge += 1;
                let b = bands[band];
                let q_mode = modifiers.alt;
                self.drag = Drag::EqBand(BandDrag {
                    dev: i,
                    band,
                    merge,
                    mode: if q_mode { DragMode::Q } else { DragMode::Move },
                    grab: eq_panel::handle_pos(plot, range, band, &b) - rp,
                    origin: rp,
                    moved: false,
                    start: b,
                    freq: b.freq as f32,
                    gain: b.gain as f32,
                    q: b.q as f32,
                });
                self.repaint = true;
            }
            (PointerButton::Primary, None) if clicks >= 2 => {
                self.select(i, out);
                let freq = eq_panel::freq_of_x(plot, rp.x);
                let Some(band) = eq_panel::nearest_free_band(&bands, freq) else {
                    return;
                };
                let gain = (eq_panel::gain_of_y(plot, range, rp.y) * 10.0).round() / 10.0;
                // A bell or shelf at exactly 0 dB is off: start at a gain that can be heard.
                let gain = if gain.abs() < 0.1 { 1.0 } else { gain };
                let k = &KEYS[band];
                let mut sets: Vec<(&str, f32)> = vec![(k.on, 1.0), (k.freq, freq as f32)];
                if let Some(g) = k.gain {
                    sets.push((g, gain as f32));
                }
                out.extend(self.eq_set_cmd(i, &sets, None));
            }
            _ => self.select(i, out),
        }
    }

    /// Moves a handle: x sets the frequency and y the gain (one batch under the merge id of the
    /// drag), or with Alt the vertical movement sets the Q.
    pub(super) fn eq_graph_move(&mut self, mut e: BandDrag, rp: Point, out: &mut Vec<UiAction>) {
        let Some((plot, range, _)) = self.eq_state(e.dev) else {
            return;
        };
        if !e.moved {
            if (rp - e.origin).hypot() < 3.0 {
                self.drag = Drag::EqBand(e);
                return;
            }
            e.moved = true;
        }
        let k = KEYS[e.band];
        let mut sets: Vec<(&str, f32)> = Vec::new();
        match e.mode {
            DragMode::Move => {
                let target = rp + e.grab;
                let freq = self
                    .param_of_key(e.dev, k.freq)
                    .map_or(0.0, |p| p.clamp(eq_panel::freq_of_x(plot, target.x) as f32));
                let has_gain = k.gain.is_some() && e.start.shape != 3;
                let gain = if has_gain {
                    let g = (eq_panel::gain_of_y(plot, range, target.y) * 10.0).round() / 10.0;
                    k.gain
                        .and_then(|g_key| self.param_of_key(e.dev, g_key))
                        .map_or(0.0, |p| p.clamp(g as f32))
                } else {
                    e.gain
                };
                if freq.to_bits() != e.freq.to_bits()
                    || (has_gain && gain.to_bits() != e.gain.to_bits())
                {
                    sets.push((k.freq, freq));
                    if let (true, Some(g)) = (has_gain, k.gain) {
                        sets.push((g, gain));
                    }
                    e.freq = freq;
                    e.gain = gain;
                }
            }
            DragMode::Q => {
                let q = eq_panel::q_after_drag(e.start.q, rp.y - e.origin.y);
                let q = self
                    .param_of_key(e.dev, k.q)
                    .map_or(0.0, |p| p.clamp(q as f32));
                if q.to_bits() != e.q.to_bits() {
                    sets.push((k.q, q));
                    e.q = q;
                }
            }
        }
        out.extend(self.eq_set_cmd(e.dev, &sets, Some(e.merge)));
        self.drag = Drag::EqBand(e);
        self.repaint = true;
    }

    /// Esc during the drag of a handle: puts the values at the press back, in the same undo step.
    pub(super) fn eq_graph_cancel(&mut self, e: &BandDrag, out: &mut Vec<UiAction>) {
        let k = KEYS[e.band];
        let mut sets: Vec<(&str, f32)> = Vec::new();
        match e.mode {
            DragMode::Move => {
                if e.freq.to_bits() != (e.start.freq as f32).to_bits() {
                    sets.push((k.freq, e.start.freq as f32));
                }
                if let Some(g) = k.gain
                    && e.gain.to_bits() != (e.start.gain as f32).to_bits()
                {
                    sets.push((g, e.start.gain as f32));
                }
            }
            DragMode::Q => {
                if e.q.to_bits() != (e.start.q as f32).to_bits() {
                    sets.push((k.q, e.start.q as f32));
                }
            }
        }
        out.extend(self.eq_set_cmd(e.dev, &sets, Some(e.merge)));
        out.push(UiAction::EndMerge);
    }

    /// The wheel over a handle steps its Q (the resonance of a cut). Returns whether a handle
    /// took the wheel.
    pub(super) fn eq_graph_wheel(
        &mut self,
        i: usize,
        rp: Point,
        notches: f64,
        fine: bool,
        out: &mut Vec<UiAction>,
    ) -> bool {
        let Some((plot, range, bands)) = self.eq_state(i) else {
            return false;
        };
        let Some(band) = eq_panel::pick(plot, range, &bands, rp) else {
            return false;
        };
        if let Some(p) = self.param_of_key(i, KEYS[band].q) {
            let q = bands[band].q as f32;
            let new = widget::wheel_value(p, q, notches, fine);
            if new.to_bits() != q.to_bits() {
                out.push(param_cmd(&self.devices[i].key, p.key, new, None));
            }
        }
        true
    }

    // ------------------------------------------------------------------ the menu of a handle

    /// Opens the menu of band `band` of panel `i` next to the press `rp`, kept in the visible rack.
    fn eq_menu_open(&mut self, i: usize, band: usize, b: &Band, rp: Point) {
        self.repaint = true;
        let Some(desc) = self.devices[i].desc else {
            return;
        };
        let rows = eq_panel::menu_rows(&desc, band, b);
        let d = &self.devices[i];
        let h = CompactList::height(rows.len(), ROW_H);
        let x0 = (rp.x + 8.0).clamp(d.x, d.x + f64::from(RACK_WIDTH) - 16.0 - MENU_W);
        let (top, bottom) = (
            self.view_to_rack(Point::new(0.0, self.rack_area().y0)).y,
            self.view_to_rack(Point::new(0.0, self.size.height)).y,
        );
        let y0 = (rp.y + 4.0).min(bottom - h).max(top);
        self.eq_menu = Some(EqMenu {
            dev: i,
            rows,
            list: CompactList::new(Rect::new(x0, y0, x0 + MENU_W, y0 + h), ROW_H),
        });
    }

    /// A press while a handle's menu is open: a row sets its params; any press closes the menu
    /// and is consumed. Returns whether a press was consumed.
    pub(super) fn eq_menu_press(&mut self, pos: Point, out: &mut Vec<UiAction>) -> bool {
        let Some(menu) = self.eq_menu.take() else {
            return false;
        };
        self.repaint = true;
        let rp = self.view_to_rack(pos);
        if let Some(row) = menu.list.row_at(menu.rows.len(), rp) {
            out.extend(self.eq_set_cmd(menu.dev, &menu.rows[row].sets, None));
        }
        true
    }

    pub(super) fn eq_menu_hover(&mut self, pos: Point) {
        let rp = self.view_to_rack(pos);
        if let Some(menu) = &mut self.eq_menu {
            let hover = menu.list.row_at(menu.rows.len(), rp);
            if hover != menu.list.hover {
                menu.list.hover = hover;
                self.repaint = true;
            }
        }
    }

    /// Paints the open menu (rack coordinates).
    pub(super) fn paint_eq_menu(&self, c: &mut dyn Canvas) {
        let Some(m) = &self.eq_menu else { return };
        let theme = &self.theme;
        m.list.paint_frame(c, theme);
        for (k, row) in m.rows.iter().enumerate() {
            let r = m.list.row_rect(k);
            m.list.paint_hover(c, theme, k);
            if row.checked {
                c.text(
                    &TextRun::new("\u{2022}", pt(r.x0 + 6.0, r.y1 - 4.5), 9.0, theme.highlight)
                        .weight(700),
                );
            }
            c.text(
                &TextRun::new(
                    row.label.as_str(),
                    pt(r.x0 + 16.0, r.y1 - 4.5),
                    9.0,
                    theme.text,
                )
                .align(Align::Start)
                .weight(if row.checked { 700 } else { 400 }),
            );
        }
    }

    /// The rows of the open menu of a handle (empty when closed).
    pub fn eq_menu_labels(&self) -> Vec<String> {
        self.eq_menu.as_ref().map_or_else(Vec::new, |m| {
            m.rows.iter().map(|r| r.label.clone()).collect()
        })
    }

    /// Centre of row `i` of the open menu of a handle (logical view coordinates).
    pub fn eq_menu_row_center(&self, i: usize) -> Option<Point> {
        let m = self.eq_menu.as_ref()?;
        (i < m.rows.len()).then(|| self.rack_to_view(m.list.row_rect(i).center()))
    }

    /// The plot of an EQ panel (logical view coordinates), front side only.
    pub fn eq_plot_rect(&self, key: &DeviceKey) -> Option<Rect> {
        if self.flip != Flip::Front {
            return None;
        }
        let i = self.device_index(key)?;
        let (plot, _, _) = self.eq_state(i)?;
        let tl = self.rack_to_view(plot.origin());
        let br = self.rack_to_view(Point::new(plot.x1, plot.y1));
        Some(Rect::new(tl.x, tl.y, br.x, br.y))
    }
}
