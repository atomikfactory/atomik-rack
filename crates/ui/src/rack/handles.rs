//! Two small aids of cable and insert work in the rack.
//!
//! * The **"+" handles**: on the seam between two devices of a column (and at the top and the
//!   foot of a column) a small "+" shows while the pointer rests there. Pressing it opens the
//!   device browser for an insert at that place: the row chosen (Enter or a double click) is
//!   added at that slot, wired as a device dropped there is. Inside an unfolded container the
//!   seams among its devices (below the INPUT strip, between devices, above the OUTPUT strip)
//!   have one too: the row is added inside that container at that place. Front side only.
//! * The **unfold on hover**: a cable dragged over a folded device unfolds it once the pointer
//!   has rested there for [`UNFOLD_DELAY`], so a socket of its back can be reached. The view
//!   only says which device is due; the host applies the unfold as an ordinary edit.

use super::columns::column_left;
use super::inline::is_inner;
use super::inline::parent_of;
use super::{Drag, Flip, Hit, RackView, UiAction};
use crate::canvas::{Canvas, CanvasExt};
use atomik_device_sdk::{RACK_WIDTH, Registry};
use atomik_model::{DeviceKey, RackDoc, RackSlot};
use vello::kurbo::{Circle, Line, Point, Stroke};

/// Radius of a "+" handle, rack units.
const HANDLE_RADIUS: f64 = 9.0;
/// How far from the middle of a seam, across the panel, the pointer still counts as on it.
const REACH_X: f64 = 28.0;
/// How far above or below a seam the pointer still counts as on it.
const REACH_Y: f64 = 7.0;
/// How long a cable must rest over a folded device before it unfolds, seconds.
pub const UNFOLD_DELAY: f64 = 0.7;

/// A place between two devices of a column (or at an end of it) that can take a device.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Seam {
    /// The column.
    pub column: u16,
    /// Height of the seam, rack coordinates.
    pub y: f64,
    /// For a seam inside an unfolded container: the container and how many of its devices lie
    /// above the seam (the index the add commands take). `None` at the top level.
    pub inside: Option<(DeviceKey, usize)>,
}

impl Seam {
    /// Where the "+" is drawn, rack coordinates.
    fn centre(&self) -> Point {
        Point::new(
            column_left(self.column) + f64::from(RACK_WIDTH) / 2.0,
            self.y,
        )
    }
}

impl RackView {
    /// The seams of the rack: above the first device of each column that can take one, and
    /// below every top-level device block. Empty inside a container and on the back side.
    fn seams(&self) -> Vec<Seam> {
        if !self.scope.is_empty() || self.flip != Flip::Front {
            return Vec::new();
        }
        let mut out = Vec::new();
        for column in 0..self.columns {
            let members: Vec<usize> = self
                .inner_range()
                .filter(|&i| !is_inner(&self.devices[i].key) && self.devices[i].column == column)
                .collect();
            // Nothing goes above the locked Master Section.
            if let Some(&first) = members.first()
                && self.locked.as_ref() != Some(&self.devices[first].key)
            {
                out.push(Seam {
                    column,
                    y: self.devices[first].y,
                    inside: None,
                });
            }
            for &i in &members {
                out.push(Seam {
                    column,
                    y: self.block_bottom(i),
                    inside: None,
                });
            }
        }
        for frame in &self.frames {
            self.inside_seams(&frame.container, &mut out);
        }
        out
    }

    /// The seams inside the unfolded container `container`: below its INPUT strip, then below
    /// each of its devices (the last one is the seam above the OUTPUT strip).
    fn inside_seams(&self, container: &DeviceKey, out: &mut Vec<Seam>) {
        let Some(column) = self
            .devices
            .iter()
            .find(|d| d.key == *container)
            .map(|d| d.column)
        else {
            return;
        };
        let mut index = 0;
        for (i, d) in self
            .devices
            .iter()
            .enumerate()
            .filter(|(_, d)| parent_of(&d.key).as_ref() == Some(container))
        {
            if d.is_interface() {
                // The two strips of the container: the OUTPUT one is the end (its seam is the
                // one below the last device).
                if !d.key.0.ends_with("~out") {
                    out.push(Seam {
                        column,
                        y: d.y + d.height,
                        inside: Some((container.clone(), 0)),
                    });
                }
            } else {
                index += 1;
                out.push(Seam {
                    column,
                    y: self.block_bottom(i),
                    inside: Some((container.clone(), index)),
                });
            }
        }
    }

    /// The centres of the "+" handles, in view coordinates: above the first device of each
    /// column that can take one, then below each device block, column by column.
    pub fn handle_centers(&self) -> Vec<Point> {
        self.seams()
            .into_iter()
            .map(|s| self.rack_to_view(s.centre()))
            .collect()
    }

    /// Whether a "+" shows under the pointer.
    pub fn plus_visible(&self) -> bool {
        self.plus.is_some()
    }

    /// The slot the browser opened by a "+" handle inserts at, until it closes.
    pub fn pending_insert(&self) -> Option<RackSlot> {
        self.insert_at
    }

    /// The container and the index among its devices the browser opened by a "+" handle inside
    /// an unfolded container inserts at, until it closes.
    pub fn pending_insert_inside(&self) -> Option<(&DeviceKey, usize)> {
        self.insert_in.as_ref().map(|(c, i)| (c, *i))
    }

    /// Whether the browser was opened by a "+" handle (at the top level or inside a container).
    pub(super) fn inserting(&self) -> bool {
        self.insert_at.is_some() || self.insert_in.is_some()
    }

    /// The seam under view position `pos`, when no gesture is running.
    pub(super) fn handle_at(&self, pos: Point) -> Option<Seam> {
        if !matches!(self.drag, Drag::None)
            || self.lib_drop
            || !self.rack_area().contains(pos)
            || self.in_browser(pos)
            || self.in_navigator(pos)
        {
            return None;
        }
        let rp = self.view_to_rack(pos);
        self.seams()
            .into_iter()
            .find(|s| (rp.y - s.y).abs() <= REACH_Y && (rp.x - s.centre().x).abs() <= REACH_X)
    }

    /// Follows the pointer: the handle it is on shows (a repaint when that changes).
    pub(super) fn update_plus(&mut self, pos: Point) {
        let plus = self.handle_at(pos);
        if plus != self.plus {
            self.plus = plus;
            self.repaint = true;
        }
    }

    /// A press on a "+" handle: the browser opens (the normal list, its search field focused)
    /// for an insert at that seam. Returns whether the press was on a handle.
    pub(super) fn handle_press(
        &mut self,
        pos: Point,
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) -> bool {
        let Some(seam) = self.handle_at(pos) else {
            return false;
        };
        let slot = match &seam.inside {
            Some(_) => None,
            None => {
                let none = vec![false; self.devices.len()];
                let at = Point::new(column_left(seam.column) + 1.0, seam.y);
                let dp = self.drop_point(at, &none);
                Some(RackSlot {
                    column: dp.column,
                    index: self.slot_index(doc, &dp, &[]),
                })
            }
        };
        self.browser.visible = true;
        self.browser.focused = true;
        self.browser.create = None;
        self.browser.query.clear();
        self.browser.scroll = 0.0;
        self.browser.highlighted =
            crate::browser::Browser::first_selectable(&self.browser.list(registry));
        match slot {
            Some(slot) => self.set_insert(Some(slot)),
            None => self.set_insert_inside(seam.inside),
        }
        self.plus = None;
        self.clamp_scroll();
        self.repaint = true;
        out.push(UiAction::OpenBrowser);
        true
    }

    /// Sets (or clears) the slot the browser's next add goes to.
    pub(super) fn set_insert(&mut self, slot: Option<RackSlot>) {
        self.insert_at = slot;
        self.insert_in = None;
        self.browser.insert = slot.is_some();
    }

    /// Sets (or clears) the place inside a container the browser's next add goes to.
    pub(super) fn set_insert_inside(&mut self, place: Option<(DeviceKey, usize)>) {
        self.insert_at = None;
        self.browser.insert = place.is_some();
        self.insert_in = place;
    }

    /// Draws the "+" the pointer is on (rack coordinates).
    pub(super) fn paint_handle(&self, c: &mut dyn Canvas) {
        let Some(seam) = &self.plus else { return };
        let theme = &self.theme;
        let at = seam.centre();
        let disc = Circle::new(at, HANDLE_RADIUS);
        c.fill_with(disc, theme.highlight.with_alpha(0.95));
        c.stroke(
            &disc.into(),
            &Stroke::new(1.0),
            &theme.on_accent.with_alpha(0.5).into(),
        );
        let arm = HANDLE_RADIUS * 0.5;
        let ink = theme.on_accent;
        c.stroke(
            &Line::new((at.x - arm, at.y), (at.x + arm, at.y)).into(),
            &Stroke::new(2.0),
            &ink.into(),
        );
        c.stroke(
            &Line::new((at.x, at.y - arm), (at.x, at.y + arm)).into(),
            &Stroke::new(2.0),
            &ink.into(),
        );
    }

    // ------------------------------------------------------------------ unfold on hover

    /// The folded device a cable being dragged is over.
    fn unfold_candidate(&self) -> Option<DeviceKey> {
        if !matches!(self.drag, Drag::Cable { .. }) {
            return None;
        }
        let i = self.hit(self.pointer).and_then(Hit::device)?;
        let d = &self.devices[i];
        (d.folded && !d.is_interface() && self.locked.as_ref() != Some(&d.key))
            .then(|| self.abs_key(&d.key))
    }

    /// Whether a cable is over a folded device that is about to unfold (the host keeps calling
    /// [`Self::hover_unfold`] meanwhile).
    pub fn unfold_pending(&self) -> bool {
        self.unfold_candidate().is_some()
    }

    /// The device a cable dragged over a folded device has waited on for [`UNFOLD_DELAY`] at
    /// `now_seconds` (any monotonic clock), once. The host unfolds it (`Command::SetFolded`);
    /// the cable drag goes on over the new layout.
    pub fn hover_unfold(&mut self, now_seconds: f64) -> Option<DeviceKey> {
        let Some(key) = self.unfold_candidate() else {
            self.unfold = None;
            return None;
        };
        match self.unfold.take() {
            Some((held, since)) if held == key => {
                if now_seconds - since >= UNFOLD_DELAY {
                    Some(key)
                } else {
                    self.unfold = Some((held, since));
                    None
                }
            }
            _ => {
                self.unfold = Some((key, now_seconds));
                None
            }
        }
    }
}
