//! Track heights and the vertical zoom of the [`ArrangementView`].
//!
//! The height of a row is a base height times the vertical zoom, kept within
//! [`MIN_ROW_HEIGHT`]`..=`[`MAX_ROW_HEIGHT`]. The base of a track's first lane is its entry in
//! `Song::track_heights` (saved with the project, [`DEFAULT_TRACK_HEIGHT`] without one), the
//! base of an automation lane is [`LANE_HEIGHT`]. The zoom is the view's own
//! ([`ArrangementView::track_zoom`], saved with the window state, not the project).
//!
//! Dragging the bottom edge of a track header sets that track's base height (one undo step per
//! drag, `DragTrackHeights`); a double-click on the edge puts the default back. Escape during
//! the drag restores the heights it started with.

use super::{ADD_ROW_HEIGHT, ArrangementView, Drag, LANE_HEIGHT, RowKind, TRACK_HEIGHT, song_cmd};
use crate::action::UiAction;
use atomik_model::{DeviceKey, MAX_TRACK_HEIGHT, MIN_TRACK_HEIGHT, SongCommand};
use std::collections::BTreeMap;

/// Shortest row, in pixels.
pub const MIN_ROW_HEIGHT: f64 = MIN_TRACK_HEIGHT as f64;
/// Tallest row, in pixels.
pub const MAX_ROW_HEIGHT: f64 = MAX_TRACK_HEIGHT as f64;
/// Smallest vertical zoom: the default track is as short as a row can be.
pub const MIN_TRACK_ZOOM: f64 = 0.4;
/// Largest vertical zoom.
pub const MAX_TRACK_ZOOM: f64 = 4.0;
/// Zoom factor of one step of the commands and of a wheel notch.
pub const TRACK_ZOOM_STEP: f64 = 1.25;
/// Height in pixels below which a track header drops its second line of controls.
pub const COMPACT_BELOW: f64 = 52.0;
/// Distance from the bottom of a track header in which the resize edge grabs.
pub(super) const EDGE_GRAB: f64 = 4.0;

/// The pixel height of a row with base height `base` at vertical zoom `zoom`.
pub fn scaled_height(base: f64, zoom: f64) -> f64 {
    let z = if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    };
    (base * z).clamp(MIN_ROW_HEIGHT, MAX_ROW_HEIGHT)
}

/// The view's vertical zoom, kept in range (1 for anything that is not a number).
pub fn clamp_track_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() {
        zoom.clamp(MIN_TRACK_ZOOM, MAX_TRACK_ZOOM)
    } else {
        1.0
    }
}

/// A running height drag.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct HeightDrag {
    track: usize,
    device: DeviceKey,
    start_y: f64,
    /// The row height in pixels when the drag started.
    start_h: f64,
    original: BTreeMap<DeviceKey, u16>,
    merge: u64,
    emitted: bool,
    last: Option<u16>,
}

/// Vertical zoom and the height drag.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct HeightUi {
    pub zoom: f64,
    pub drag: Option<HeightDrag>,
}

impl HeightUi {
    /// The track whose edge is being dragged.
    pub fn drag_track(&self) -> Option<usize> {
        self.drag.as_ref().map(|d| d.track)
    }
}

impl Default for HeightUi {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            drag: None,
        }
    }
}

impl ArrangementView {
    /// The vertical zoom: 1 is the default row height ([`TRACK_HEIGHT`], [`LANE_HEIGHT`]).
    pub fn track_zoom(&self) -> f64 {
        self.th.zoom
    }

    /// Sets the vertical zoom (clamped to [`MIN_TRACK_ZOOM`]`..=`[`MAX_TRACK_ZOOM`]) and lays
    /// the rows out again; the scroll offset stays put.
    pub fn set_track_zoom(&mut self, zoom: f64) {
        let zoom = clamp_track_zoom(zoom);
        if (zoom - self.th.zoom).abs() > f64::EPSILON {
            self.th.zoom = zoom;
            self.relayout();
        }
    }

    /// The height in pixels of the first lane of a track (zoom included), `None` for a device
    /// without a track.
    pub fn track_height(&self, device: &DeviceKey) -> Option<f64> {
        self.song
            .tracks
            .iter()
            .any(|t| &t.device == device)
            .then(|| self.track_row_height(device))
    }

    /// Centre of the grab zone on the bottom edge of a track's header, `None` for a device
    /// without a visible track row.
    pub fn track_edge_center(&self, device: &DeviceKey) -> Option<crate::kurbo::Point> {
        let track = self.song.tracks.iter().position(|t| &t.device == device)?;
        let rr = self.row_rect(self.row_of(track, 0)?);
        Some(crate::kurbo::Point::new(
            self.bounds.x0 + 40.0,
            rr.y1 - EDGE_GRAB / 2.0,
        ))
    }

    /// The row height of a track's first lane.
    pub(super) fn track_row_height(&self, device: &DeviceKey) -> f64 {
        let base = self
            .song
            .track_heights
            .get(device)
            .map_or(TRACK_HEIGHT, |h| f64::from(*h));
        scaled_height(base, self.th.zoom)
    }

    /// The row height of an automation lane.
    pub(super) fn lane_row_height(&self) -> f64 {
        scaled_height(LANE_HEIGHT, self.th.zoom)
    }

    /// The height a track row has by default at this zoom (new tracks, the drop placeholder).
    pub(super) fn default_row_height(&self) -> f64 {
        scaled_height(TRACK_HEIGHT, self.th.zoom)
    }

    /// Lays the rows out again with the current heights (the zoom changed).
    pub(super) fn relayout(&mut self) {
        let lane_h = self.lane_row_height();
        let heights: Vec<f64> = self
            .rows
            .iter()
            .map(|r| match &r.kind {
                RowKind::Lane { track, lane: 0 } => self.song.tracks.get(*track).map_or_else(
                    || self.default_row_height(),
                    |t| self.track_row_height(&t.device),
                ),
                RowKind::Lane { .. } => lane_h,
                RowKind::AddTrack(_) | RowKind::AddAudio => ADD_ROW_HEIGHT,
                RowKind::TakeGroup { .. } => super::TAKE_GROUP_HEIGHT,
                RowKind::Take { .. } => super::TAKE_ROW_HEIGHT,
            })
            .collect();
        let mut y = 0.0;
        for (row, h) in self.rows.iter_mut().zip(heights) {
            row.y = y;
            row.h = h;
            y += h;
        }
        self.content_height = y;
        self.clamp_scroll();
        self.repaint = true;
    }

    /// Zooms the rows by `factor` keeping the point of the lanes under `anchor_y` where it is:
    /// the row under it stays under it, at the same fraction of its height.
    pub(super) fn zoom_tracks_at(&mut self, factor: f64, anchor_y: f64) {
        let now = self.th.zoom;
        let next = clamp_track_zoom(now * factor);
        if (next - now).abs() <= f64::EPSILON {
            return;
        }
        let anchor = (anchor_y - self.lanes_top()).max(0.0);
        let at = anchor + self.scroll_y;
        // The row under the anchor and how far down it the anchor is.
        let held = self
            .rows
            .iter()
            .position(|r| at >= r.y && at < r.y + r.h)
            .map(|i| (i, (at - self.rows[i].y) / self.rows[i].h));
        self.th.zoom = next;
        self.relayout();
        if let Some((i, frac)) = held
            && let Some(r) = self.rows.get(i)
        {
            self.scroll_y = r.y + frac * r.h - anchor;
            self.clamp_scroll();
        }
    }

    /// The "Reset track heights" command: zoom 1 and every track at the default height.
    pub(super) fn reset_track_heights(&mut self, out: &mut Vec<UiAction>) {
        self.set_track_zoom(1.0);
        if !self.song.track_heights.is_empty() {
            out.push(song_cmd(SongCommand::SetTrackHeights(BTreeMap::new())));
        }
    }

    /// A press on the bottom edge of a track header: starts a height drag, or with a
    /// double-click puts the default height back.
    pub(super) fn height_down(
        &mut self,
        track: usize,
        pos: crate::kurbo::Point,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        let Some(device) = self.song.tracks.get(track).map(|t| t.device.clone()) else {
            return;
        };
        if clicks >= 2 {
            let mut map = self.song.track_heights.clone();
            if map.remove(&device).is_some() {
                out.push(song_cmd(SongCommand::SetTrackHeights(map)));
            }
            return;
        }
        let merge = self.merge_id();
        self.th.drag = Some(HeightDrag {
            track,
            start_h: self.track_row_height(&device),
            device,
            start_y: pos.y,
            original: self.song.track_heights.clone(),
            merge,
            emitted: false,
            last: None,
        });
        self.drag = Drag::Height;
    }

    /// Pointer motion during a height drag.
    pub(super) fn height_move(&mut self, pos: crate::kurbo::Point, out: &mut Vec<UiAction>) {
        let zoom = self.th.zoom;
        let Some(d) = &mut self.th.drag else {
            return;
        };
        let wanted = d.start_h + (pos.y - d.start_y);
        // Back to the base height the song stores (at zoom 1), whole pixels.
        let base = (wanted / zoom)
            .round()
            .clamp(MIN_ROW_HEIGHT, MAX_ROW_HEIGHT) as u16;
        if d.last == Some(base) {
            return;
        }
        d.last = Some(base);
        let mut map = d.original.clone();
        map.insert(d.device.clone(), base);
        d.emitted = true;
        out.push(song_cmd(SongCommand::DragTrackHeights {
            heights: map,
            merge: d.merge,
        }));
    }

    /// The end of a height drag.
    pub(super) fn height_up(&mut self, out: &mut Vec<UiAction>) {
        if let Some(d) = self.th.drag.take()
            && d.emitted
        {
            out.push(UiAction::EndMerge);
        }
    }

    /// Escape during a height drag: the heights it started with go back.
    pub(super) fn height_cancel(&mut self, out: &mut Vec<UiAction>) {
        self.drag = Drag::None;
        if let Some(d) = self.th.drag.take()
            && d.emitted
        {
            out.push(song_cmd(SongCommand::DragTrackHeights {
                heights: d.original,
                merge: d.merge,
            }));
            out.push(UiAction::EndMerge);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_scale_with_the_zoom_and_stay_inside_the_limits() {
        assert_eq!(scaled_height(60.0, 1.0), 60.0);
        assert_eq!(scaled_height(60.0, 2.0), 120.0);
        assert_eq!(scaled_height(60.0, 0.1), MIN_ROW_HEIGHT);
        assert_eq!(scaled_height(300.0, 4.0), MAX_ROW_HEIGHT);
        assert_eq!(scaled_height(1.0, 1.0), MIN_ROW_HEIGHT);
        for bad in [f64::NAN, f64::INFINITY, -2.0, 0.0] {
            assert_eq!(scaled_height(60.0, bad), 60.0, "{bad}");
        }
    }

    #[test]
    fn the_zoom_is_clamped_and_the_default_track_fits_the_limits() {
        assert_eq!(clamp_track_zoom(100.0), MAX_TRACK_ZOOM);
        assert_eq!(clamp_track_zoom(0.0), MIN_TRACK_ZOOM);
        assert_eq!(clamp_track_zoom(f64::NAN), 1.0);
        assert_eq!(scaled_height(TRACK_HEIGHT, MIN_TRACK_ZOOM), MIN_ROW_HEIGHT);
        assert!(scaled_height(TRACK_HEIGHT, MAX_TRACK_ZOOM) < MAX_ROW_HEIGHT);
    }
}
