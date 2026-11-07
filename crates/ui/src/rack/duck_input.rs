//! Pointer handling of the Sidechain Tool's curve editor (the maths is in
//! [`crate::duck_panel`]).
//!
//! A drag of a point or of a segment is one undo step: every change shares a merge id and the
//! release closes it. A point drag always sends the same two params (`x` and `y` of the point) as
//! one batch, which is what lets the model fold the batches into one step; a bend drag sends the
//! one bend param. Escape puts the values of the press back. The first edit of a device that has no
//! stored points writes the curve that was on screen first (its own undo step). A click on empty
//! plot adds a point and a double-click on a point removes it, each one undo step.

use super::{Drag, RackView, UiAction};
use crate::duck_panel::{
    bend_for, bent, changes, from_px, inner, inserted, learn_button, moved, pick_point,
    pick_segment, points_of, removed, to_px,
};
use atomik_dsp::duck_curve::{COUNT_KEY, POINT_KEYS, Point};
use atomik_model::{Command, DeviceKey};
use vello::kurbo::{Point as Pt, Vec2};

/// What a drag on the curve moves.
#[derive(Debug, Clone)]
pub(super) enum DuckKind {
    /// Point `n`.
    Point(usize),
    /// Segment `seg` (it starts at point `seg`), grabbed at curve position `at`.
    Bend { seg: usize, at: f32 },
}

/// State of a drag on the curve.
#[derive(Debug, Clone)]
pub(super) struct DuckDrag {
    dev: usize,
    kind: DuckKind,
    merge: u64,
    /// The curve at the press.
    start: Vec<Point>,
    /// The curve as last sent.
    last: Vec<Point>,
    /// Point position minus pointer position at the press, rack coordinates.
    grab: Vec2,
}

fn set(key: &DeviceKey, param: &str, value: f32, merge: Option<u64>) -> Command {
    Command::SetParam {
        key: key.clone(),
        param: param.to_owned(),
        value,
        merge,
    }
}

/// One command (or a batch) for `commands`; `None` for nothing.
fn pack(mut commands: Vec<Command>) -> Option<UiAction> {
    match commands.len() {
        0 => None,
        1 => Some(UiAction::Command(commands.remove(0))),
        _ => Some(UiAction::Command(Command::Batch(commands))),
    }
}

impl RackView {
    /// The plot's drawing area of device `i`'s curve editor, rack coordinates.
    fn duck_area(&self, i: usize) -> Option<vello::kurbo::Rect> {
        let d = self.devices.get(i)?;
        let fx = d.spec.fx.filter(|f| f.kind == crate::fx::FxKind::Duck)?;
        Some(inner(fx.plot) + d.offset())
    }

    /// The curve of device `i` and whether it has stored points.
    fn duck_curve(&self, i: usize) -> Option<(Vec<Point>, bool)> {
        let d = self.devices.get(i)?;
        Some(points_of(d.device.as_ref(), &d.desc?))
    }

    /// Sends the params that turn `old` into `new` as one command (or batch).
    fn duck_send(
        &self,
        i: usize,
        old: &[Point],
        stored: bool,
        new: &[Point],
        out: &mut Vec<UiAction>,
    ) {
        let key = &self.devices[i].key;
        let commands: Vec<Command> = changes(old, stored, new)
            .into_iter()
            .map(|(param, value)| set(key, param, value, None))
            .collect();
        out.extend(pack(commands));
    }

    /// A press on the curve plot of device `i` (module docs).
    pub(super) fn duck_down(&mut self, i: usize, rp: Pt, clicks: u8, out: &mut Vec<UiAction>) {
        self.select(i, out);
        let (Some(area), Some((points, stored))) = (self.duck_area(i), self.duck_curve(i)) else {
            return;
        };
        if let Some(k) = pick_point(area, &points, rp) {
            if clicks >= 2 {
                if let Some(new) = removed(&points, k) {
                    self.duck_send(i, &points, stored, &new, out);
                }
                return;
            }
            self.duck_start(i, DuckKind::Point(k), &points, stored, rp, out);
        } else if let Some(seg) = pick_segment(area, &points, rp) {
            let (at, _) = from_px(area, rp);
            self.duck_start(i, DuckKind::Bend { seg, at }, &points, stored, rp, out);
        } else {
            let (x, y) = from_px(area, rp);
            if let Some(new) = inserted(&points, x, y) {
                self.duck_send(i, &points, stored, &new, out);
            }
        }
    }

    /// Starts a drag; a device without stored points gets its curve written first.
    fn duck_start(
        &mut self,
        i: usize,
        kind: DuckKind,
        points: &[Point],
        stored: bool,
        rp: Pt,
        out: &mut Vec<UiAction>,
    ) {
        let Some(area) = self.duck_area(i) else {
            return;
        };
        if !stored {
            self.duck_send(i, points, false, points, out);
        }
        let grab = match kind {
            DuckKind::Point(k) => to_px(area, f64::from(points[k].x), f64::from(points[k].y)) - rp,
            DuckKind::Bend { .. } => Vec2::ZERO,
        };
        let merge = self.next_merge;
        self.next_merge += 1;
        self.drag = Drag::Duck(DuckDrag {
            dev: i,
            kind,
            merge,
            start: points.to_vec(),
            last: points.to_vec(),
            grab,
        });
        self.repaint = true;
    }

    /// The pointer moves during a curve drag.
    pub(super) fn duck_move(&mut self, mut d: DuckDrag, rp: Pt, out: &mut Vec<UiAction>) {
        let Some(area) = self.duck_area(d.dev) else {
            self.drag = Drag::Duck(d);
            return;
        };
        let key = self.devices[d.dev].key.clone();
        match d.kind {
            DuckKind::Point(k) => {
                let (x, y) = from_px(area, rp + d.grab);
                let new = moved(&d.last, k, x, y);
                let (a, b) = (new[k], d.last[k]);
                if a.x.to_bits() != b.x.to_bits() || a.y.to_bits() != b.y.to_bits() {
                    // Always the same two params, so the model folds the batches into one step.
                    let m = Some(d.merge);
                    out.extend(pack(vec![
                        set(&key, POINT_KEYS[k][0], a.x, m),
                        set(&key, POINT_KEYS[k][1], a.y, m),
                    ]));
                    d.last = new;
                    self.repaint = true;
                }
            }
            DuckKind::Bend { seg, at } => {
                let (_, y) = from_px(area, rp);
                let bend = bend_for(&d.start, seg, at, y);
                if bend != d.last[seg].bend {
                    out.push(UiAction::Command(set(
                        &key,
                        POINT_KEYS[seg][2],
                        f32::from(bend),
                        Some(d.merge),
                    )));
                    d.last = bent(&d.last, seg, bend);
                    self.repaint = true;
                }
            }
        }
        self.drag = Drag::Duck(d);
    }

    /// Escape during a curve drag: puts the values of the press back.
    pub(super) fn duck_cancel(&mut self, d: &DuckDrag, out: &mut Vec<UiAction>) {
        let key = self.devices[d.dev].key.clone();
        let m = Some(d.merge);
        match d.kind {
            DuckKind::Point(k) => {
                let p = d.start[k];
                if d.last[k] != p {
                    out.extend(pack(vec![
                        set(&key, POINT_KEYS[k][0], p.x, m),
                        set(&key, POINT_KEYS[k][1], p.y, m),
                    ]));
                }
            }
            DuckKind::Bend { seg, .. } => {
                if d.last[seg].bend != d.start[seg].bend {
                    out.push(UiAction::Command(set(
                        &key,
                        POINT_KEYS[seg][2],
                        f32::from(d.start[seg].bend),
                        m,
                    )));
                }
            }
        }
        out.push(UiAction::EndMerge);
    }

    /// A press on the LEARN button of device `i`: selects it and asks the app.
    pub(super) fn duck_learn_down(&mut self, i: usize, out: &mut Vec<UiAction>) {
        self.select(i, out);
        out.push(UiAction::LearnDuck(self.devices[i].key.clone()));
    }

    // ------------------------------------------------------------------ geometry for hosts and tests

    /// Centre of point `n` of a Sidechain Tool's curve (logical view coordinates), front side
    /// only.
    pub fn duck_point_center(&self, key: &DeviceKey, n: usize) -> Option<Pt> {
        if self.flip != super::Flip::Front {
            return None;
        }
        let i = self.device_index(key)?;
        let area = self.duck_area(i)?;
        let (points, _) = self.duck_curve(i)?;
        let p = points.get(n)?;
        Some(self.rack_to_view(to_px(area, f64::from(p.x), f64::from(p.y))))
    }

    /// The point of the curve at position `x` and dip `y` (logical view coordinates), front side
    /// only: where to press to add a point there.
    pub fn duck_curve_at(&self, key: &DeviceKey, x: f64, y: f64) -> Option<Pt> {
        if self.flip != super::Flip::Front {
            return None;
        }
        let i = self.device_index(key)?;
        Some(self.rack_to_view(to_px(self.duck_area(i)?, x, y)))
    }

    /// Centre of the LEARN button of a Sidechain Tool (logical view coordinates).
    pub fn duck_learn_center(&self, key: &DeviceKey) -> Option<Pt> {
        if self.flip != super::Flip::Front {
            return None;
        }
        let d = &self.devices[self.device_index(key)?];
        let fx = d.spec.fx.filter(|f| f.kind == crate::fx::FxKind::Duck)?;
        (!d.folded).then(|| self.rack_to_view(learn_button(fx.plot).center() + d.offset()))
    }

    /// The points of a Sidechain Tool's curve as the panel draws them.
    pub fn duck_points(&self, key: &DeviceKey) -> Vec<Point> {
        self.device_index(key)
            .and_then(|i| self.duck_curve(i))
            .map(|(p, _)| p)
            .unwrap_or_default()
    }

    /// The name of the param that holds the number of points of the curve.
    pub const fn duck_count_key() -> &'static str {
        COUNT_KEY
    }
}
