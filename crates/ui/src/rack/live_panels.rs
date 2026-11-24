//! Which panels are on screen: the app switches the work a device does only for its display
//! (the EQ's 16-band analyser, the analysis of a spectrum or loudness panel) on for these and
//! off for every other panel.

use super::{Flip, RackView};
use crate::fx::FxKind;
use atomik_model::DeviceKey;
use vello::kurbo::Point;

impl RackView {
    /// Tells the view whether the window that holds it is on screen: the host says no while that
    /// window is minimized or the rack pane is not drawn at all (another pane covers it). The
    /// default is yes, so a view nobody told (a test, a headless session) behaves as before.
    /// While it is no, [`Self::visible_panels`] and [`Self::is_panel_visible`] list nothing.
    pub fn set_window_shown(&mut self, shown: bool) {
        self.window_shown = shown;
    }

    /// Whether the window of the rack is on screen (see [`Self::set_window_shown`]).
    pub fn window_shown(&self) -> bool {
        self.window_shown
    }

    /// The full keys (scope path included) of the devices of `kind` whose unfolded front panel
    /// is at least partly inside the rack area right now. A panel on the back side, a folded
    /// one, or one scrolled out of view does not count; the same cut the painter uses, so a
    /// panel that shows its display is always in the list. Cheap: one pass over the panels.
    pub fn visible_panels(&self, kind: FxKind) -> Vec<DeviceKey> {
        if !self.shows_front() {
            return Vec::new();
        }
        let (top, bottom) = self.visible_span();
        let scope = self.scope_path();
        self.devices
            .iter()
            .filter(|d| d.fx == Some(kind) && !d.folded)
            .filter(|d| d.y + d.height >= top && d.y <= bottom)
            .map(|d| match &scope {
                Some(p) => DeviceKey(format!("{}/{}", p.0, d.key.0)),
                None => d.key.clone(),
            })
            .collect()
    }

    /// Whether the unfolded front panel of the device `key` (full key, scope path included) is
    /// at least partly inside the rack area right now: the cut of [`Self::visible_panels`] for
    /// one device, asked once per frame by the analyser feed. Allocation-free at the top level
    /// of the rack (a device inside a container builds its local key).
    pub fn is_panel_visible(&self, key: &DeviceKey) -> bool {
        if !self.shows_front() {
            return false;
        }
        let index = if self.scope.is_empty() {
            self.devices.iter().position(|d| d.key == *key)
        } else {
            self.device_index(key)
        };
        let Some(d) = index.and_then(|i| self.devices.get(i)) else {
            return false;
        };
        let (top, bottom) = self.visible_span();
        !d.folded && d.y + d.height >= top && d.y <= bottom
    }

    /// The front face is drawn: the window is on screen, the view has an area and the rack is not
    /// flipped to its back.
    fn shows_front(&self) -> bool {
        self.window_shown
            && self.flip == Flip::Front
            && self.size.width > 0.0
            && self.size.height > 0.0
    }

    /// The span of the rack, in rack coordinates, that the rack area shows.
    fn visible_span(&self) -> (f64, f64) {
        let area = self.rack_area();
        (
            self.view_to_rack(Point::new(area.x0, area.y0)).y,
            self.view_to_rack(Point::new(area.x0, self.size.height)).y,
        )
    }
}
