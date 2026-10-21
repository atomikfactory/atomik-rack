//! Rack columns in the view: where a column is, which one a point falls in, the zone that
//! creates a new column while a device is dragged, the auto-scroll of that drag, the
//! Add / Remove Column commands, and the read-only accessors the rack navigator uses.

use super::{Drag, MOVE_THRESHOLD, RACK_MARGIN, RackView, UiAction};
use crate::keymap::CommandOutcome;
use atomik_device_sdk::RACK_WIDTH;
use atomik_model::{Command, DeviceKey, MAX_RACK_COLUMNS, RackDoc, RackSlot};
use vello::kurbo::{Point, Rect, Size, Vec2};
use vello::peniko::Color;

/// Space between two columns, rack units of length (logical pixels at zoom 1).
pub const COLUMN_GAP: f64 = 32.0;
/// Width of the zone that creates a new column while a device is dragged.
pub(super) const NEW_COLUMN_W: f64 = 200.0;
/// How close to an edge of the rack area (logical pixels) a drag starts to scroll.
const EDGE: f64 = 48.0;
/// Scroll speed at the very edge, logical pixels per second.
const SCROLL_SPEED: f64 = 700.0;
/// The longest time step one tick may scroll for, so a stalled frame does not jump.
const MAX_STEP: f64 = 0.05;

/// The left edge of column `column` in rack coordinates.
pub(super) fn column_left(column: u16) -> f64 {
    f64::from(column) * (f64::from(RACK_WIDTH) + COLUMN_GAP)
}

/// The width of `columns` columns side by side.
pub(super) fn columns_width(columns: u16) -> f64 {
    column_left(columns.max(1) - 1) + f64::from(RACK_WIDTH)
}

/// Scroll speed (logical pixels per second) for a pointer `depth` pixels inside an edge zone.
fn ramp(depth: f64) -> f64 {
    SCROLL_SPEED * (depth / EDGE).clamp(0.0, 1.0)
}

impl RackView {
    // ------------------------------------------------------------------ geometry

    /// The column of rack x `x`, among the columns that exist.
    pub(super) fn column_at(&self, x: f64) -> u16 {
        let step = f64::from(RACK_WIDTH) + COLUMN_GAP;
        (((x + COLUMN_GAP / 2.0) / step).floor().max(0.0) as u16).min(self.columns - 1)
    }

    /// The column a dragged device would land in at rack x `x`: past the last column it is
    /// the new one (`self.columns`) when the rack may still grow.
    pub(super) fn drop_column(&self, x: f64) -> u16 {
        let step = f64::from(RACK_WIDTH) + COLUMN_GAP;
        let c = ((x + COLUMN_GAP / 2.0) / step).floor().max(0.0) as u16;
        if c >= self.columns && self.can_add_column() {
            self.columns
        } else {
            c.min(self.columns - 1)
        }
    }

    fn can_add_column(&self) -> bool {
        self.scope.is_empty() && self.columns < MAX_RACK_COLUMNS
    }

    /// Whether a drag that can drop a device is in progress (a device moved past the
    /// threshold, or a browser row dragged out).
    pub(super) fn drop_drag_active(&self) -> bool {
        match &self.drag {
            Drag::Move { start, at, .. } => (*at - *start).hypot() > MOVE_THRESHOLD,
            Drag::Browser { active, .. } => *active,
            _ => false,
        }
    }

    /// Whether the new-column zone is drawn: only while such a drag runs.
    pub(super) fn zone_visible(&self) -> bool {
        self.can_add_column() && self.drop_drag_active()
    }

    /// The rack-space rectangle of the new-column zone.
    pub(super) fn zone_rect(&self) -> Rect {
        let x = column_left(self.columns);
        Rect::new(
            x,
            0.0,
            x + NEW_COLUMN_W,
            self.content_height
                .max(f64::from(atomik_device_sdk::RACK_UNIT) * 3.0),
        )
    }

    /// Width the scroll range covers: the columns, plus the new-column zone while it shows.
    pub(super) fn extent_width(&self) -> f64 {
        self.content_width()
            + if self.zone_visible() {
                COLUMN_GAP + NEW_COLUMN_W
            } else {
                0.0
            }
    }

    // ------------------------------------------------------------------ auto-scroll

    /// The scroll velocity (logical pixels per second) the pointer asks for while a device is
    /// dragged: zero away from the edges of the rack area, and over the browser for a browser
    /// drag (there the drag is about to be cancelled).
    fn autoscroll_velocity(&self) -> Vec2 {
        if !self.drop_drag_active() {
            return Vec2::ZERO;
        }
        let (area, p) = (self.rack_area(), self.pointer);
        if matches!(self.drag, Drag::Browser { .. }) && p.x < area.x0 {
            return Vec2::ZERO;
        }
        Vec2::new(
            ramp(p.x - (area.x1 - EDGE)) - ramp(area.x0 + EDGE - p.x),
            ramp(p.y - (area.y1 - EDGE)) - ramp(area.y0 + EDGE - p.y),
        )
    }

    /// True while the pointer of a device drag sits near an edge, so the host should keep
    /// calling [`Self::tick`] even without input.
    pub fn is_auto_scrolling(&self) -> bool {
        self.autoscroll_velocity() != Vec2::ZERO
    }

    /// Advances the auto-scroll of a drag to `now_seconds` (any monotonic clock): the rack
    /// scrolls by the edge speed times the time since the last tick (at most 50 ms), so a
    /// device can be dragged to a column or a place that is off screen. The first tick of a
    /// drag only starts the clock. Returns whether the view scrolled.
    pub fn tick(&mut self, now_seconds: f64) -> bool {
        let v = self.autoscroll_velocity();
        if v == Vec2::ZERO {
            self.tick_clock = None;
            return false;
        }
        let Some(last) = self.tick_clock.replace(now_seconds) else {
            return false;
        };
        let before = self.scroll;
        self.scroll_by(v * (now_seconds - last).clamp(0.0, MAX_STEP));
        self.scroll != before
    }

    // ------------------------------------------------------------------ commands

    /// `rack.add_column` and `rack.remove_column` (the column of the last right click on the
    /// empty rack).
    pub(super) fn column_command(
        &mut self,
        id: &str,
        out: &mut Vec<UiAction>,
    ) -> Option<CommandOutcome> {
        let refuse = |text: &str| Some(CommandOutcome::NotApplicable(text.to_string()));
        match id {
            "rack.add_column" | "rack.remove_column" if !self.scope.is_empty() => {
                refuse("Columns belong to the top level of the rack")
            }
            "rack.add_column" if self.columns >= MAX_RACK_COLUMNS => {
                refuse(&format!("A rack has at most {MAX_RACK_COLUMNS} columns"))
            }
            "rack.add_column" => {
                out.push(UiAction::Command(Command::AddRackColumn));
                None
            }
            "rack.remove_column" if self.columns <= 1 => {
                refuse("The last column cannot be removed")
            }
            "rack.remove_column" => {
                let column = self.column_target.min(self.columns - 1);
                out.push(UiAction::Command(Command::RemoveRackColumn { column }));
                None
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------------ paste target

    /// The slot of the empty rack at rack point `rp`, for a paste: the column under the point
    /// (the last one past the end) and the place among its devices. `None` inside a container.
    pub(super) fn slot_at(&self, rp: Point, doc: &RackDoc) -> Option<RackSlot> {
        if !self.scope.is_empty() {
            return None;
        }
        let none = vec![false; self.devices.len()];
        let mut dp = self.drop_point(rp, &none);
        if dp.new_column {
            dp = self.drop_point(Point::new(column_left(self.columns - 1), rp.y), &none);
        }
        Some(RackSlot {
            column: dp.column,
            index: self.slot_index(doc, &dp, &[]),
        })
    }

    /// The slot under the last right click on the empty rack, once: a paste from the rack's
    /// menu lands there. Any other click or key clears it.
    pub fn take_context_slot(&mut self) -> Option<RackSlot> {
        self.context_slot.take()
    }

    // ------------------------------------------------------------------ navigator seam

    /// How many columns the view shows (1 inside a container).
    pub(crate) fn column_count(&self) -> u16 {
        self.columns
    }

    /// The left edge of a column in rack coordinates.
    pub(crate) fn column_x(&self, column: u16) -> f64 {
        column_left(column)
    }

    /// The panels of a column, top to bottom, as (key, rack-space rectangle, accent colour):
    /// column 0 starts with the Audio I/O strip (or the container's input strip).
    pub(crate) fn column_device_rects(&self, column: u16) -> Vec<(DeviceKey, Rect, Color)> {
        self.devices
            .iter()
            .filter(|d| d.column == column)
            .map(|d| (d.key.clone(), d.rect(), d.accent))
            .collect()
    }

    /// The size of the whole rack in rack coordinates: all the columns by the tallest one.
    pub(crate) fn content_size(&self) -> Size {
        Size::new(self.content_width(), self.content_height)
    }

    /// The part of the rack the rack area shows now, in rack coordinates.
    pub(crate) fn viewport_rack_rect(&self) -> Rect {
        let area = self.rack_area();
        Rect::from_points(
            self.view_to_rack(Point::new(area.x0, area.y0)),
            self.view_to_rack(Point::new(area.x1, area.y1)),
        )
    }

    /// Scrolls so the top-left of the rack area shows rack point `top_left` (clamped to the
    /// content).
    pub(crate) fn scroll_to_rack(&mut self, top_left: Point) {
        self.scroll = Vec2::new(
            top_left.x * self.zoom + RACK_MARGIN,
            top_left.y * self.zoom + RACK_MARGIN,
        );
        self.clamp_scroll();
        self.repaint = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Theme, UiEvent};
    use atomik_device_sdk::Registry;
    use atomik_model::RackDevice;
    use std::collections::BTreeMap;

    fn device(key: &str, column: u16) -> RackDevice {
        RackDevice {
            key: DeviceKey(key.to_string()),
            type_id: "atomik.stereo-delay".to_string(),
            name: key.to_string(),
            params: BTreeMap::new(),
            folded: false,
            color: None,
            zones: Vec::new(),
            steps: None,
            inner: None,
            plugin: None,
            column,
        }
    }

    fn setup() -> (RackView, RackDoc, Registry) {
        let mut reg = Registry::new();
        atomik_devices::register_all(&mut reg).expect("register");
        let doc = RackDoc {
            sample_rate: 48_000,
            tempo_bpm: 120.0,
            seed: 0,
            output_channels: 2,
            input_channels: 0,
            assets: BTreeMap::new(),
            devices: vec![device("a", 0), device("b", 0), device("c", 2)],
            columns: 3,
            cables: Vec::new(),
            song: atomik_model::empty_song(120.0),
            console: atomik_model::Console::default(),
        };
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(&UiEvent::Resize(Size::new(1200.0, 200.0)), &doc, &reg);
        v.sync(&doc, &reg);
        (v, doc, reg)
    }

    #[test]
    fn the_navigator_reads_columns_sizes_and_rects() {
        let (v, _, _) = setup();
        assert_eq!(v.column_count(), 3);
        let step = f64::from(RACK_WIDTH) + COLUMN_GAP;
        assert!((v.column_x(2) - 2.0 * step).abs() < 1e-9);
        let size = v.content_size();
        assert!((size.width - (2.0 * step + f64::from(RACK_WIDTH))).abs() < 1e-9);
        let col0 = v.column_device_rects(0);
        assert_eq!(col0.len(), 3, "the interface and two devices");
        assert!(
            col0.windows(2)
                .all(|w| (w[0].1.y1 - w[1].1.y0).abs() < 1e-9)
        );
        assert!(v.column_device_rects(1).is_empty());
        let col2 = v.column_device_rects(2);
        assert_eq!(col2.len(), 1);
        assert!((col2[0].1.x0 - v.column_x(2)).abs() < 1e-9 && col2[0].1.y0.abs() < 1e-9);
        assert!((size.height - col0.last().unwrap().1.y1).abs() < 1e-9);
    }

    #[test]
    fn scroll_to_rack_and_the_viewport_agree() {
        let (mut v, _, _) = setup();
        v.scroll_to_rack(Point::new(700.0, 30.0));
        let r = v.viewport_rack_rect();
        assert!((r.x0 - 700.0).abs() < 1e-6, "{r:?}");
        assert!((r.y0 - 30.0).abs() < 1e-6, "{r:?}");
        assert!((r.width() - (1200.0 - 2.0 * 0.0) / v.zoom()).abs() < 1e-6);
        let off = v.scroll;
        assert!((off.x - (700.0 + RACK_MARGIN)).abs() < 1e-6);
        // Clamped at the far end and at the start.
        v.scroll_to_rack(Point::new(1e9, 1e9));
        let end = v.viewport_rack_rect();
        assert!(end.x1 >= v.content_size().width - 1e-6);
        assert!(end.y1 >= v.content_size().height - 1e-6);
        v.scroll_to_rack(Point::new(-1e9, -1e9));
        assert!(v.scroll.hypot() < 1e-9);
    }

    #[test]
    fn a_point_belongs_to_the_nearest_column() {
        let (v, _, _) = setup();
        let step = f64::from(RACK_WIDTH) + COLUMN_GAP;
        assert_eq!(v.column_at(-500.0), 0);
        assert_eq!(v.column_at(100.0), 0);
        assert_eq!(v.column_at(step - 1.0), 1);
        assert_eq!(v.column_at(2.0 * step + 100.0), 2);
        assert_eq!(v.column_at(1e6), 2, "never past the last column");
        // A drag may go one column further, to the new column.
        assert_eq!(
            v.drop_column(2.0 * step + f64::from(RACK_WIDTH) + COLUMN_GAP),
            3
        );
        assert_eq!(v.drop_column(1e6), 3);
    }
}
