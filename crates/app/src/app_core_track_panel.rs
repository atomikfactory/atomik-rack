//! The Track Panel in the core: the view of the selected track's chain, kept in step with the
//! document while it shows.
//!
//! The panel follows the track selected in the arrangement. It is rebuilt only when the
//! document revision or the track changes, because each move and removal it offers is planned
//! on a scratch copy of the document.

use super::AppCore;
use crate::sink::EngineSink;
use atomik_model::DeviceKey;
use atomik_ui::TrackPanelView;

impl<S: EngineSink> AppCore<S> {
    /// The Track Panel.
    pub fn track_panel(&self) -> &TrackPanelView {
        &self.track_panel
    }

    /// The Track Panel, mutably (bounds, scale).
    pub fn track_panel_mut(&mut self) -> &mut TrackPanelView {
        &mut self.track_panel
    }

    /// Whether the Track Panel is shown.
    pub fn track_panel_visible(&self) -> bool {
        self.track_panel_on
    }

    /// Shows or hides the Track Panel; showing it builds it for the selected track.
    pub fn set_track_panel_visible(&mut self, visible: bool) {
        self.track_panel_on = visible;
        self.track_panel_stamp = None;
        self.sync_track_panel();
    }

    /// The track the panel shows: the one selected in the arrangement.
    fn panel_track(&self) -> Option<DeviceKey> {
        self.arrangement.selected_track().cloned()
    }

    /// Rebuilds the panel when it shows and the document or the selected track changed since
    /// the last build.
    pub fn sync_track_panel(&mut self) {
        if !self.track_panel_on {
            return;
        }
        let stamp = (self.document.revision(), self.panel_track());
        if self.track_panel_stamp.as_ref() == Some(&stamp) {
            return;
        }
        self.track_panel
            .sync(self.document.doc(), &self.registry, stamp.1.as_ref());
        self.track_panel_stamp = Some(stamp);
    }
}
