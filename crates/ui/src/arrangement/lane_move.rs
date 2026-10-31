//! Dragging a note lane by its header label: the lane moves to another place among the note
//! lanes of its track or of another note track, in one undo step; with the copy role held it is
//! copied there instead.
//!
//! The pointer picks an insertion place between note lane rows: the upper half of a row is
//! before that lane, the lower half after it. A place the model would refuse (a frozen track, an
//! audio or automation track, the lane limit, no change) shows no line and the release does
//! nothing. Escape cancels.

use super::{ArrangementView, DRAG_THRESHOLD, Drag, RowKind, song_cmd};
use crate::action::UiAction;
use crate::event::Modifiers;
use crate::keymap::{GestureClass, Role};
use atomik_model::{DeviceKey, SongCommand};
use vello::kurbo::{Circle, Line, Point};

/// Where a lane drag would drop, and where its line is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneDrop {
    /// The track the lane goes to.
    pub device: DeviceKey,
    /// The place the command takes (among the target's lanes, as the command counts them).
    pub index: usize,
    /// The y of the insertion line.
    pub y: f64,
}

/// A note lane header drag.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct LaneMoveDrag {
    start: Point,
    device: DeviceKey,
    lane: usize,
    moved: bool,
    copy: bool,
    drop: Option<LaneDrop>,
}

impl ArrangementView {
    /// Starts a drag of note lane `lane` of `device`'s track from a press on its label.
    pub(super) fn begin_lane_move(&mut self, device: DeviceKey, lane: usize, at: Point) {
        self.og.lane_mv = Some(LaneMoveDrag {
            start: at,
            device,
            lane,
            moved: false,
            copy: false,
            drop: None,
        });
        self.drag = Drag::LaneMove;
    }

    /// The place a running lane drag would drop at (`None` outside a drag, or over a place the
    /// model refuses).
    pub fn lane_drop(&self) -> Option<&LaneDrop> {
        self.og.lane_mv.as_ref().and_then(|d| d.drop.as_ref())
    }

    /// Pointer motion during a lane drag.
    pub(super) fn lane_move_move(&mut self, pos: Point, modifiers: Modifiers) {
        let copy = self
            .vp
            .roles()
            .is_active(Role::CopyDrag, modifiers, GestureClass::DragItem);
        let Some(d) = &mut self.og.lane_mv else {
            return;
        };
        if !d.moved && pos.distance(d.start) <= DRAG_THRESHOLD {
            return;
        }
        d.moved = true;
        d.copy = copy;
        let (device, lane) = (d.device.clone(), d.lane);
        let drop = self.lane_drop_at(&device, lane, copy, pos.y);
        if let Some(d) = &mut self.og.lane_mv {
            d.drop = drop;
        }
        self.repaint = true;
    }

    /// The release of a lane drag: the move (or copy) is one command.
    pub(super) fn lane_move_up(&mut self, out: &mut Vec<UiAction>) {
        self.drag = Drag::None;
        let Some(d) = self.og.lane_mv.take() else {
            return;
        };
        self.repaint = true;
        let Some(drop) = d.drop.filter(|_| d.moved) else {
            return;
        };
        // The lane stays the selected one wherever it lands.
        self.set_selected_track(Some(drop.device.clone()));
        self.note_lane_sel.insert(drop.device.clone(), drop.index);
        out.push(song_cmd(SongCommand::MoveNoteLane {
            device: d.device,
            lane: d.lane,
            to_device: drop.device,
            to_index: drop.index,
            copy: d.copy,
        }));
    }

    /// Escape during a lane drag.
    pub(super) fn lane_move_cancel(&mut self) {
        self.drag = Drag::None;
        self.og.lane_mv = None;
        self.repaint = true;
    }

    /// The drop of lane `lane` of `device` with the pointer at `y`, `None` where nothing can be
    /// dropped.
    fn lane_drop_at(
        &self,
        device: &DeviceKey,
        lane: usize,
        copy: bool,
        y: f64,
    ) -> Option<LaneDrop> {
        let row = self.rows.iter().find(|r| {
            let rect = self.row_rect(r);
            y >= rect.y0 && y < rect.y1
        })?;
        let RowKind::Lane { track, lane: over } = row.kind else {
            return None;
        };
        let target = self.song.tracks.get(track)?;
        if !target.is_note_lane(over) {
            return None;
        }
        let rect = self.row_rect(row);
        let upper = y < rect.y0 + rect.height() / 2.0;
        let place = if upper { over } else { over + 1 };
        let same = &target.device == device;
        let index = if !copy && same && lane < place {
            place - 1
        } else {
            place
        };
        self.song
            .moves_note_lane(device, lane, (&target.device, index), copy)
            .then(|| LaneDrop {
                device: target.device.clone(),
                index,
                y: if upper { rect.y0 } else { rect.y1 },
            })
    }

    /// The insertion line of a running lane drag.
    pub(super) fn paint_lane_drop(&self, c: &mut dyn crate::canvas::Canvas) {
        use crate::canvas::CanvasExt;
        let Some(drop) = self.lane_drop() else {
            return;
        };
        let color = self.st.selection;
        let x0 = self.bounds.x0 + 16.0;
        let y = drop.y.round();
        c.stroke_with(Line::new((x0, y), (self.bounds.x1, y)), 2.0, color);
        c.fill_with(Circle::new((x0 + 3.0, y), 3.0), color);
    }
}
