//! The handle of an automation lane's static value in the [`ArrangementView`].
//!
//! A lane that holds a static value (see `atomik_model::song::Lane::static_value`) shows it as
//! a line across the lane with a small square handle at the left edge of the timeline. Pressing
//! the handle with the Select tool and dragging up or down sets the value from the pointer's
//! height, on the same scale as the points of the lane's clips; one drag is one undo step and
//! Esc puts the value back.

use super::{ArrangementView, DRAG_THRESHOLD, Drag, Hit, Row, RowKind};
use crate::action::UiAction;
use crate::canvas::CanvasExt;
use crate::seq_edit;
use crate::seq_paint as sp;
use atomik_core::ParamDescriptor;
use atomik_model::{DeviceKey, SongCommand};
use vello::kurbo::{Point, Rect};
use vello::peniko::Color;

/// Half the width of the handle.
const HANDLE_HALF: f64 = 5.0;
/// Distance of the handle's centre from the left edge of the timeline.
const HANDLE_INSET: f64 = 12.0;
/// Space kept above and below the value range of a lane row.
const MARGIN: f64 = 6.0;

/// A drag of a static value handle.
#[derive(Debug, Clone)]
pub(super) struct StaticDrag {
    start: Point,
    device: DeviceKey,
    param_key: String,
    param: ParamDescriptor,
    /// `[top, top + height]` of the value scale.
    top: f64,
    height: f64,
    /// The value at the press.
    original: f32,
    /// The value last sent.
    last: f32,
    merge: u64,
    moved: bool,
    emitted: bool,
}

/// The top and bottom of the value scale of a lane row.
fn scale_of(row: Rect) -> (f64, f64) {
    let (top, bottom) = (row.y0 + MARGIN, row.y1 - MARGIN);
    if bottom - top >= 4.0 {
        (top, bottom)
    } else {
        let mid = (row.y0 + row.y1) / 2.0;
        (mid - 2.0, mid + 2.0)
    }
}

impl ArrangementView {
    /// The y of static value `value` of lane `lane` of track `track` in row `row`.
    fn static_y(&self, track: usize, lane: usize, row: Rect, value: f32) -> f64 {
        let (top, bottom) = scale_of(row);
        let n = f64::from(seq_edit::value_to_norm(&self.param_of(track, lane), value));
        bottom - n.clamp(0.0, 1.0) * (bottom - top)
    }

    /// The static value of an automation lane that is shown unfolded.
    fn static_value_of(&self, track: usize, lane: usize) -> Option<f32> {
        if !self.is_automation_lane(track, lane) || self.is_lane_folded(track, lane) {
            return None;
        }
        self.song.tracks.get(track)?.lanes.get(lane)?.static_value
    }

    /// The handle of the lane in row `row`.
    fn static_handle_rect(&self, track: usize, lane: usize, row: Rect) -> Option<Rect> {
        let value = self.static_value_of(track, lane)?;
        let y = self.static_y(track, lane, row, value);
        let x = self.timeline_rect().x0 + HANDLE_INSET;
        Some(Rect::new(
            x - HANDLE_HALF,
            y - HANDLE_HALF,
            x + HANDLE_HALF,
            y + HANDLE_HALF,
        ))
    }

    /// The lane whose static value handle is under `pos`.
    pub(super) fn static_handle_at(&self, pos: Point) -> Option<Hit> {
        self.visible_rows().find_map(|(row, rr): (&Row, Rect)| {
            let RowKind::Lane { track, lane } = row.kind else {
                return None;
            };
            let h = self.static_handle_rect(track, lane, rr)?;
            h.inflate(2.0, 2.0)
                .contains(pos)
                .then_some(Hit::StaticHandle(track, lane))
        })
    }

    /// The centre of the static value handle of the lane that shows `param` of `device`, when
    /// the lane is shown and has a static value (hit tests and tools use it).
    pub fn static_handle_center(&self, device: &DeviceKey, param: &str) -> Option<Point> {
        let track = self.song.tracks.iter().position(|t| &t.device == device)?;
        let lane = self.song.tracks[track].automation_lane(param)?;
        let row = self.row_rect(self.row_of(track, lane)?);
        Some(self.static_handle_rect(track, lane, row)?.center())
    }

    /// Paints the line and the handle of the lane in row `rr`, over its clips.
    pub(super) fn paint_static_value(
        &self,
        c: &mut dyn crate::canvas::Canvas,
        rr: Rect,
        timeline: Rect,
        (track, lane): (usize, usize),
        accent: Color,
    ) {
        let Some(value) = self.static_value_of(track, lane) else {
            return;
        };
        let st = &self.st;
        let y = crate::paint::snap_hairline(self.static_y(track, lane, rr, value), self.scale);
        sp::line(c, timeline.x0, y, timeline.x1, y, accent.with_alpha(0.55));
        if let Some(h) = self.static_handle_rect(track, lane, rr) {
            sp::rect(c, h, accent);
            c.stroke_with(h, 1.0, st.base.ink);
        }
    }

    /// A press on the handle of lane `lane`: starts the drag.
    pub(super) fn static_down(&mut self, track: usize, lane: usize, pos: Point) {
        let (Some((device, param_key)), Some(value), Some(row)) = (
            self.lane_id(track, lane),
            self.static_value_of(track, lane),
            self.row_of(track, lane).map(|r| self.row_rect(r)),
        ) else {
            return;
        };
        self.selected_track = None;
        self.point_sel = None;
        let (top, bottom) = scale_of(row);
        let merge = self.merge_id();
        self.drag = Drag::Static(Box::new(StaticDrag {
            start: pos,
            device,
            param_key,
            param: self.param_of(track, lane),
            top,
            height: bottom - top,
            original: value,
            last: value,
            merge,
            moved: false,
            emitted: false,
        }));
    }

    /// The pointer moved during a handle drag: the value follows its height.
    pub(super) fn static_move(&mut self, pos: Point, out: &mut Vec<UiAction>) {
        let Drag::Static(d) = &mut self.drag else {
            return;
        };
        d.moved |= pos.distance(d.start) > DRAG_THRESHOLD;
        if !d.moved {
            return;
        }
        let value = seq_edit::value_from_y(&d.param, pos.y, d.top, d.height);
        if value.to_bits() != d.last.to_bits() && !value.is_nan() {
            out.push(static_cmd(&d.device, &d.param_key, value, d.merge));
            d.last = value;
            d.emitted = true;
        }
        self.repaint = true;
    }

    /// The release: what was sent is one undo step.
    pub(super) fn static_up(&mut self, d: StaticDrag, out: &mut Vec<UiAction>) {
        if d.emitted {
            out.push(UiAction::EndMerge);
        }
    }

    /// Esc: the value goes back to what it was at the press.
    pub(super) fn static_cancel(&mut self, d: StaticDrag, out: &mut Vec<UiAction>) {
        if d.emitted {
            if d.last.to_bits() != d.original.to_bits() {
                out.push(static_cmd(&d.device, &d.param_key, d.original, d.merge));
            }
            out.push(UiAction::EndMerge);
        }
    }
}

fn static_cmd(device: &DeviceKey, param: &str, value: f32, merge: u64) -> UiAction {
    UiAction::Command(atomik_model::Command::Song(SongCommand::SetLaneStatic {
        device: device.clone(),
        param: param.to_owned(),
        value: Some(value),
        merge: Some(merge),
    }))
}
