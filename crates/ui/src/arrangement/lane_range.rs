//! A time range selected on one lane, or on several lanes, of the [`ArrangementView`].
//!
//! Dragging on a lane with the lane-range role (Alt+Shift by default) shades the time range
//! the drag covers, snapped to the grid. A drag that moves up or down onto other lanes (note
//! and automation lanes of one track or of several) takes every lane between the one it began
//! on and the one under the pointer. While the range is shown, Copy takes the parts of the
//! clips of its lanes inside the range, Cut also removes them, Delete removes them, and Paste
//! puts what was copied at the playhead (the parts of a range over several lanes go back on
//! lanes of their own kind). The range is view state: any other press or Esc forgets it.

use super::{ArrangementView, DRAG_THRESHOLD, Drag, Hit, RowKind, Tool};
use crate::event::Modifiers;
use crate::keymap::{CommandOutcome, GestureClass, Role};
use crate::seq_paint as sp;
use crate::timeline::snap_round;
use atomik_model::{ClipId, DeviceKey, WorkflowCommand};
use vello::kurbo::{Point, Rect};

/// The lanes of a range as `(track, lane)`, in display order.
pub type RangeLanes = Vec<(DeviceKey, usize)>;

/// A range of one or more lanes, in song ticks (`start < end`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LaneRange {
    /// The lanes in display order, as `(track, lane)`; the first is the one that names the
    /// range for the commands that take a single lane.
    pub(super) lanes: Vec<(DeviceKey, usize)>,
    pub(super) start: i64,
    pub(super) end: i64,
}

/// The range being dragged.
#[derive(Debug, Clone)]
pub(super) struct RangeDrag {
    start: Point,
    /// The lane the drag began on.
    device: DeviceKey,
    lane: usize,
    /// The lanes from there to the one under the pointer, in display order.
    lanes: Vec<(DeviceKey, usize)>,
    t0: i64,
    t1: i64,
    moved: bool,
}

impl ArrangementView {
    /// The selected lane range as `(device, lane, start, end)` in song ticks: the first lane of
    /// the range, which is the only one for a range on a single lane (see
    /// [`ArrangementView::lane_ranges`] for all of them).
    pub fn lane_range(&self) -> Option<(DeviceKey, usize, i64, i64)> {
        let (lanes, start, end) = self.lane_ranges()?;
        let (device, lane) = lanes.into_iter().next()?;
        Some((device, lane, start, end))
    }

    /// The selected lane range as `(lanes, start, end)`: the lanes it covers in display order
    /// (those that still exist) and the song ticks. `None` when no range is selected.
    pub fn lane_ranges(&self) -> Option<(RangeLanes, i64, i64)> {
        let r = self.range_sel.as_ref()?;
        // A lane may have gone since: the range keeps the others.
        let lanes: RangeLanes = r
            .lanes
            .iter()
            .filter(|(device, lane)| {
                self.song
                    .track(device)
                    .is_some_and(|t| t.lanes.get(*lane).is_some())
            })
            .cloned()
            .collect();
        (!lanes.is_empty()).then_some((lanes, r.start, r.end))
    }

    /// Selects the time range `[start, end)` of lane `lane` of track `device` (what a drag with
    /// the lane-range role does). The clip selection is left empty.
    pub fn set_lane_range(&mut self, device: DeviceKey, lane: usize, start: i64, end: i64) {
        self.set_lane_ranges(vec![(device, lane)], start, end);
    }

    /// Selects the time range `[start, end)` of several lanes, given as `(track, lane)` (what a
    /// drag across lanes with the lane-range role does). The clip selection is left empty; an
    /// empty range or no lane selects nothing.
    pub fn set_lane_ranges(&mut self, lanes: Vec<(DeviceKey, usize)>, start: i64, end: i64) {
        self.selection.clear();
        self.range_sel =
            (start < end && !lanes.is_empty()).then_some(LaneRange { lanes, start, end });
        self.repaint = true;
    }

    /// Forgets the selected lane range.
    pub fn clear_lane_range(&mut self) {
        if self.range_sel.take().is_some() {
            self.repaint = true;
        }
    }

    /// Whether `modifiers` hold the lane-range role.
    fn range_role(&self, modifiers: Modifiers) -> bool {
        self.vp
            .roles()
            .is_active(Role::RangeDrag, modifiers, GestureClass::DragItem)
    }

    /// A press on a lane that starts a range, when the role is held with the Select tool.
    /// False leaves the press to the other gestures.
    pub(super) fn range_press(
        &mut self,
        hit: &Hit,
        pos: Point,
        modifiers: Modifiers,
        tool: Tool,
    ) -> bool {
        if tool != Tool::Select || !self.range_role(modifiers) {
            return false;
        }
        let (Hit::Lane { track, lane } | Hit::Clip { track, lane, .. }) = hit else {
            return false;
        };
        let Some(t) = self.song.tracks.get(*track) else {
            return false;
        };
        let device = t.device.clone();
        let lanes = vec![(device.clone(), *lane)];
        let tick = snap_round(self.axis.x_to_tick(pos.x), self.step(modifiers)).max(0);
        self.range_sel = None;
        self.selection.clear();
        self.point_sel = None;
        self.selected_track = None;
        self.drag = Drag::Range(Box::new(RangeDrag {
            start: pos,
            device,
            lane: *lane,
            lanes,
            t0: tick,
            t1: tick,
            moved: false,
        }));
        true
    }

    /// The pointer moved during a range drag.
    pub(super) fn range_move(&mut self, pos: Point, modifiers: Modifiers) {
        let tick = snap_round(self.axis.x_to_tick(pos.x), self.step(modifiers)).max(0);
        let Drag::Range(d) = &self.drag else {
            return;
        };
        let lanes = self.lanes_between(&d.device, d.lane, pos.y);
        if let Drag::Range(d) = &mut self.drag {
            d.moved |= pos.distance(d.start) > DRAG_THRESHOLD;
            d.t1 = tick;
            if !lanes.is_empty() {
                d.lanes = lanes;
            }
        }
        self.repaint = true;
    }

    /// The lanes from lane `lane` of track `device` to the lane row at height `y` (above or
    /// below every row, the first or the last), in display order: the lane rows of both ends
    /// and of every row between them (folder headers, take rows and the add buttons are not
    /// lanes).
    fn lanes_between(&self, device: &DeviceKey, lane: usize, y: f64) -> Vec<(DeviceKey, usize)> {
        let Some(track) = self.song.tracks.iter().position(|t| &t.device == device) else {
            return Vec::new();
        };
        let Some(from) = self
            .rows
            .iter()
            .position(|r| r.kind == RowKind::Lane { track, lane })
        else {
            return Vec::new();
        };
        let to = self
            .rows
            .iter()
            .position(|r| {
                let rr = self.row_rect(r);
                y >= rr.y0 && y < rr.y1
            })
            .unwrap_or_else(|| match self.rows.first() {
                Some(first) if y < self.row_rect(first).y0 => 0,
                _ => self.rows.len().saturating_sub(1),
            });
        let (lo, hi) = (from.min(to), from.max(to));
        self.rows[lo..=hi]
            .iter()
            .filter_map(|r| match r.kind {
                RowKind::Lane { track, lane } => self
                    .song
                    .tracks
                    .get(track)
                    .map(|t| (t.device.clone(), lane)),
                _ => None,
            })
            .collect()
    }

    /// The release: a range that has length stays selected.
    pub(super) fn range_up(&mut self, d: RangeDrag) {
        if d.moved && d.t0 != d.t1 {
            self.range_sel = Some(LaneRange {
                lanes: d.lanes,
                start: d.t0.min(d.t1),
                end: d.t0.max(d.t1),
            });
        }
    }

    /// `edit.delete` on a range: the parts of the lane's clips inside it go, in one undo step.
    /// `None` when no range is selected.
    pub(super) fn range_delete(&mut self) -> Option<CommandOutcome> {
        let (lanes, start, end) = self.lane_ranges()?;
        let mut ids: Vec<ClipId> = Vec::new();
        for (device, lane) in &lanes {
            let Some(l) = self.song.track(device).and_then(|t| t.lanes.get(*lane)) else {
                continue;
            };
            ids.extend(
                l.clips
                    .iter()
                    .filter(|c| c.start < end && c.end() > start)
                    .map(|c| c.id),
            );
        }
        if ids.is_empty() {
            return Some(CommandOutcome::NotApplicable(
                "No clip lies inside the selected range".into(),
            ));
        }
        self.range_sel = None;
        Some(CommandOutcome::Handled(vec![super::workflow_cmd(
            WorkflowCommand::CutOut {
                ids,
                start,
                end,
                copy: false,
            },
        )]))
    }

    /// The range as a rectangle over each of its lanes, the one being dragged first.
    fn lane_range_rects(&self) -> Vec<Rect> {
        let (lanes, a, z) = match &self.drag {
            Drag::Range(d) if d.moved && d.t0 != d.t1 => {
                (d.lanes.clone(), d.t0.min(d.t1), d.t0.max(d.t1))
            }
            _ => match self.lane_ranges() {
                Some(r) => r,
                None => return Vec::new(),
            },
        };
        lanes
            .iter()
            .filter_map(|(device, lane)| {
                let track = self.song.tracks.iter().position(|t| &t.device == device)?;
                let row = self.row_rect(self.row_of(track, *lane)?);
                Some(Rect::new(
                    self.axis.tick_to_x(a),
                    row.y0,
                    self.axis.tick_to_x(z),
                    row.y1,
                ))
            })
            .collect()
    }

    /// Paints the range over the lanes.
    pub(super) fn paint_lane_range(&self, c: &mut dyn crate::canvas::Canvas, timeline: Rect) {
        let col = self.st.selection;
        for r in self.lane_range_rects() {
            let r = Rect::new(r.x0.max(timeline.x0), r.y0, r.x1.min(timeline.x1), r.y1);
            if r.width() <= 0.0 {
                continue;
            }
            sp::rect(c, r, col.with_alpha(0.22));
            sp::line(c, r.x0, r.y0, r.x0, r.y1, col);
            sp::line(c, r.x1, r.y0, r.x1, r.y1, col);
        }
    }
}
