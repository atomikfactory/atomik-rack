//! Which panels are on screen: the app switches the work a device does only for its display
//! (the EQ's 16-band analyser) on for these and off for every other panel.

use super::{Flip, RackView};
use crate::fx::FxKind;
use atomik_model::DeviceKey;
use vello::kurbo::Point;

impl RackView {
    /// The full keys (scope path included) of the devices of `kind` whose unfolded front panel
    /// is at least partly inside the rack area right now. A panel on the back side, a folded
    /// one, or one scrolled out of view does not count; the same cut the painter uses, so a
    /// panel that shows its display is always in the list. Cheap: one pass over the panels.
    pub fn visible_panels(&self, kind: FxKind) -> Vec<DeviceKey> {
        if self.flip != Flip::Front || self.size.width <= 0.0 || self.size.height <= 0.0 {
            return Vec::new();
        }
        let area = self.rack_area();
        let top = self.view_to_rack(Point::new(area.x0, area.y0)).y;
        let bottom = self.view_to_rack(Point::new(area.x0, self.size.height)).y;
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
}
