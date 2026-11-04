//! A time range selected on one lane of the [`ArrangementView`].
//!
//! Dragging on a lane with the lane-range role (Alt+Shift by default) shades the time range
//! the drag covers, snapped to the grid. While it is shown, Copy takes the parts of the lane's
//! clips inside the range, Cut also removes them, Delete removes them, and Paste puts what was
//! copied at the playhead. The range is view state: any other press or Esc forgets it.

use super::{ArrangementView, DRAG_THRESHOLD, Drag, Hit, Tool};
use crate::event::Modifiers;
use crate::keymap::{CommandOutcome, GestureClass, Role};
use crate::seq_paint as sp;
use crate::timeline::snap_round;
use atomik_model::{ClipId, DeviceKey, WorkflowCommand};
use vello::kurbo::{Point, Rect};

/// A range of one lane, in song ticks (`start < end`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LaneRange {
    pub(super) device: DeviceKey,
    pub(super) lane: usize,
    pub(super) start: i64,
    pub(super) end: i64,
}

/// The range being dragged.
#[derive(Debug, Clone)]
pub(super) struct RangeDrag {
    start: Point,
    device: DeviceKey,
    lane: usize,
    t0: i64,
    t1: i64,
    moved: bool,
}

impl ArrangementView {
    /// The selected lane range as `(device, lane, start, end)` in song ticks.
    pub fn lane_range(&self) -> Option<(DeviceKey, usize, i64, i64)> {
        let r = self.range_sel.as_ref()?;
        // The lane may have gone since: no range then.
        let t = self.song.track(&r.device)?;
        t.lanes.get(r.lane)?;
        Some((r.device.clone(), r.lane, r.start, r.end))
    }

    /// Selects the time range `[start, end)` of lane `lane` of track `device` (what a drag with
    /// the lane-range role does). The clip selection is left empty.
    pub fn set_lane_range(&mut self, device: DeviceKey, lane: usize, start: i64, end: i64) {
        self.selection.clear();
        self.range_sel = (start < end).then_some(LaneRange {
            device,
            lane,
            start,
            end,
        });
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
        let tick = snap_round(self.axis.x_to_tick(pos.x), self.step(modifiers)).max(0);
        self.range_sel = None;
        self.selection.clear();
        self.point_sel = None;
        self.selected_track = None;
        self.drag = Drag::Range(Box::new(RangeDrag {
            start: pos,
            device,
            lane: *lane,
            t0: tick,
            t1: tick,
            moved: false,
        }));
        true
    }

    /// The pointer moved during a range drag.
    pub(super) fn range_move(&mut self, pos: Point, modifiers: Modifiers) {
        let tick = snap_round(self.axis.x_to_tick(pos.x), self.step(modifiers)).max(0);
        if let Drag::Range(d) = &mut self.drag {
            d.moved |= pos.distance(d.start) > DRAG_THRESHOLD;
            d.t1 = tick;
        }
        self.repaint = true;
    }

    /// The release: a range that has length stays selected.
    pub(super) fn range_up(&mut self, d: RangeDrag) {
        if d.moved && d.t0 != d.t1 {
            self.range_sel = Some(LaneRange {
                device: d.device,
                lane: d.lane,
                start: d.t0.min(d.t1),
                end: d.t0.max(d.t1),
            });
        }
    }

    /// `edit.delete` on a range: the parts of the lane's clips inside it go, in one undo step.
    /// `None` when no range is selected.
    pub(super) fn range_delete(&mut self) -> Option<CommandOutcome> {
        let (device, lane, start, end) = self.lane_range()?;
        let ids: Vec<ClipId> = self
            .song
            .track(&device)?
            .lanes
            .get(lane)?
            .clips
            .iter()
            .filter(|c| c.start < end && c.end() > start)
            .map(|c| c.id)
            .collect();
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

    /// The range as a rectangle over its lane, the one being dragged first.
    fn lane_range_rect(&self) -> Option<Rect> {
        let (device, lane, a, z) = match &self.drag {
            Drag::Range(d) if d.moved && d.t0 != d.t1 => {
                (d.device.clone(), d.lane, d.t0.min(d.t1), d.t0.max(d.t1))
            }
            _ => self.lane_range()?,
        };
        let track = self.song.tracks.iter().position(|t| t.device == device)?;
        let row = self.row_rect(self.row_of(track, lane)?);
        Some(Rect::new(
            self.axis.tick_to_x(a),
            row.y0,
            self.axis.tick_to_x(z),
            row.y1,
        ))
    }

    /// Paints the range over the lanes.
    pub(super) fn paint_lane_range(&self, c: &mut dyn crate::canvas::Canvas, timeline: Rect) {
        let Some(r) = self.lane_range_rect() else {
            return;
        };
        let r = Rect::new(r.x0.max(timeline.x0), r.y0, r.x1.min(timeline.x1), r.y1);
        if r.width() <= 0.0 {
            return;
        }
        let col = self.st.selection;
        sp::rect(c, r, col.with_alpha(0.22));
        sp::line(c, r.x0, r.y0, r.x0, r.y1, col);
        sp::line(c, r.x1, r.y0, r.x1, r.y1, col);
    }
}
