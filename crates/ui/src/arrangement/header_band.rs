//! The rubber band over the track headers: dragging down or up the header column selects the
//! tracks the band touches.
//!
//! The band starts from a press on the empty part of the column below the last track, or from a
//! press with Alt on any header (a plain press on a header picks the track, and a drag from its
//! first row moves it). Shift or Ctrl keeps the tracks that were selected and adds the ones the
//! band touches. A track is touched when the band reaches any of its rows (its lanes and take
//! rows too); folder rows and tracks hidden in a collapsed folder are not tracks of the band. A
//! press that goes nowhere clears the track selection unless Shift or Ctrl is held. Escape puts
//! the selection back as it was.

use super::{ArrangementView, DRAG_THRESHOLD, Drag, RowKind};
use atomik_model::DeviceKey;
use std::collections::BTreeSet;
use vello::kurbo::{Point, Rect};

/// A running rubber band over the headers.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct HeaderBand {
    start: Point,
    end: Point,
    moved: bool,
    /// Shift or Ctrl: the tracks that were selected stay selected.
    add: bool,
    /// The selection before the press: the anchor track and the others.
    before: (Option<DeviceKey>, BTreeSet<DeviceKey>),
}

impl ArrangementView {
    /// Starts a band at `pos`; `add` keeps the tracks selected now.
    pub(super) fn begin_header_band(&mut self, pos: Point, add: bool) {
        self.og.band = Some(HeaderBand {
            start: pos,
            end: pos,
            moved: false,
            add,
            before: (self.selected_track.clone(), self.og.extra.clone()),
        });
        self.drag = Drag::HeaderBand;
    }

    /// The band being dragged, as a rectangle over the header column (`None` outside a drag, or
    /// before the pointer has gone past the drag threshold).
    pub fn header_band(&self) -> Option<Rect> {
        let b = self.og.band.as_ref().filter(|b| b.moved)?;
        Some(Rect::new(
            self.bounds.x0,
            b.start.y.min(b.end.y),
            self.axis.x0,
            b.start.y.max(b.end.y),
        ))
    }

    /// The tracks (by index) with a row the vertical span `y0..y1` reaches.
    fn tracks_in_span(&self, y0: f64, y1: f64) -> BTreeSet<usize> {
        self.rows
            .iter()
            .filter_map(|r| match r.kind {
                RowKind::Lane { track, .. }
                | RowKind::TakeGroup { track, .. }
                | RowKind::Take { track, .. } => Some((track, self.row_rect(r))),
                _ => None,
            })
            .filter(|(_, rect)| rect.y1 > y0 && rect.y0 < y1)
            .map(|(track, _)| track)
            .collect()
    }

    /// Pointer motion during a band: the selection follows the tracks it touches.
    pub(super) fn header_band_move(&mut self, pos: Point) {
        let Some(b) = &mut self.og.band else {
            return;
        };
        if !b.moved && pos.distance(b.start) <= DRAG_THRESHOLD {
            return;
        }
        b.moved = true;
        b.end = pos;
        let (y0, y1) = (b.start.y.min(pos.y), b.start.y.max(pos.y));
        let (add, before) = (b.add, b.before.clone());
        let touched = self.tracks_in_span(y0, y1);
        let kept: BTreeSet<DeviceKey> = if add {
            before.0.into_iter().chain(before.1).collect()
        } else {
            BTreeSet::new()
        };
        let picked: Vec<DeviceKey> = self
            .song
            .tracks
            .iter()
            .enumerate()
            .filter(|(i, t)| touched.contains(i) || kept.contains(&t.device))
            .map(|(_, t)| t.device.clone())
            .collect();
        self.selection.clear();
        self.point_sel = None;
        self.og.group = None;
        let mut picked = picked.into_iter();
        self.selected_track = picked.next();
        self.og.extra = picked.collect();
        self.repaint = true;
    }

    /// The release of a band.
    pub(super) fn header_band_up(&mut self) {
        self.drag = Drag::None;
        let Some(b) = self.og.band.take() else {
            return;
        };
        self.repaint = true;
        if !b.moved && !b.add {
            // A press on nothing: no track stays selected.
            self.selected_track = None;
            self.og.extra.clear();
        }
    }

    /// Escape during a band: the selection goes back to what it was.
    pub(super) fn header_band_cancel(&mut self) {
        self.drag = Drag::None;
        if let Some(b) = self.og.band.take() {
            (self.selected_track, self.og.extra) = b.before;
        }
        self.repaint = true;
    }

    /// The band over the header column.
    pub(super) fn paint_header_band(&self, c: &mut dyn crate::canvas::Canvas) {
        use crate::canvas::CanvasExt;
        let Some(r) = self.header_band() else {
            return;
        };
        c.fill_with(r, self.st.selection.with_alpha(0.18));
        c.stroke_with(r.inset(-0.5), 1.0, self.st.selection);
    }
}
