//! A library device or patch dropped on a track header: it replaces the track's device in
//! place (the same replacement the rack's device panels take), keeping the track, its clips,
//! its console channel and the cables that still fit. The header is outlined while a fitting
//! item hovers over it; an item the model would refuse shows nothing and is not taken.

use super::ArrangementView;
use crate::canvas::{Canvas, CanvasExt};
use crate::library_drop;
use atomik_device_sdk::Registry;
use atomik_model::{CreateSource, DeviceKey, RackDoc};
use std::path::PathBuf;
use vello::kurbo::{Point, Rect, RoundedRect, Stroke};

impl ArrangementView {
    /// The track whose header (the track list, any of its lane rows) is under `pos`.
    fn header_track_at(&self, pos: Point) -> Option<usize> {
        if !self.bounds.contains(pos) || pos.y < self.lanes_top() || pos.x >= self.axis.x0 {
            return None;
        }
        match self.row_at(pos.y).map(|r| &r.kind) {
            Some(super::RowKind::Lane { track, .. }) => Some(*track),
            _ => None,
        }
    }

    /// The device a library item dropped at `pos` replaces, with the item: the device row or
    /// the one-device patch of `paths` over a track header whose device the model accepts to
    /// replace by it.
    pub(super) fn header_replace_at(
        &mut self,
        paths: &[PathBuf],
        pos: Point,
        doc: &RackDoc,
        registry: &Registry,
    ) -> Option<(PathBuf, DeviceKey)> {
        let path = library_drop::replacement_item(paths)?;
        let track = self.header_track_at(pos)?;
        let device = self.song.tracks[track].device.clone();
        let type_id = match atomik_library::device_type_of(path) {
            Some(t) => t.to_string(),
            None => {
                if self.lib_peek.as_ref().is_none_or(|(p, _)| p != path) {
                    self.lib_peek = Some((path.clone(), library_drop::patch_device_type(path)));
                }
                self.lib_peek.as_ref()?.1.clone()?
            }
        };
        atomik_model::replace_reason(doc, registry, &device, &CreateSource::Type(type_id))
            .is_none()
            .then(|| (path.clone(), device))
    }

    /// Sets the track whose header is outlined as a drop target.
    pub(super) fn set_header_hover(&mut self, device: Option<DeviceKey>) {
        if self.header_hover != device {
            self.header_hover = device;
            self.repaint = true;
        }
    }

    /// The outline of the header a fitting library item hovers over.
    pub(super) fn paint_header_drop(&self, c: &mut dyn Canvas) {
        let Some(device) = &self.header_hover else {
            return;
        };
        let Some(track) = self.song.tracks.iter().position(|t| &t.device == device) else {
            return;
        };
        let Some(row) = self.row_of(track, 0) else {
            return;
        };
        let rr = self.row_rect(row);
        let r = Rect::new(rr.x0 + 1.0, rr.y0 + 1.0, self.axis.x0 - 1.0, rr.y1 - 1.0);
        let shape = RoundedRect::from_rect(r, 3.0);
        c.fill_with(shape, self.st.selection.with_alpha(0.14));
        c.stroke(&shape.into(), &Stroke::new(2.0), &self.st.selection.into());
    }
}
