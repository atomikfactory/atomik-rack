//! The buttons of the Headphone Correction panel (see [`crate::headphone_panel`]): IMPORT, the
//! empty graph (a click on it imports) and REMOVE. What a press does depends on whether the
//! device holds data when it happens, so the hit regions are always there.

use super::*;

impl RackView {
    /// A primary press on a part of the Headphone Correction panel of device `i`: selects the
    /// device and, when the part works now, asks the app.
    pub(super) fn headphone_down(&mut self, i: usize, hit: Hit, out: &mut Vec<UiAction>) {
        self.select(i, out);
        let key = self.devices[i].key.clone();
        let has_data = self.devices[i]
            .device
            .as_ref()
            .is_some_and(|d| d.headphone.is_some());
        match hit {
            Hit::HeadphoneImport(_) => out.push(UiAction::HeadphoneImport(key)),
            Hit::HeadphoneGraph(_) if !has_data => out.push(UiAction::HeadphoneImport(key)),
            Hit::HeadphoneRemove(_) if has_data => out.push(UiAction::HeadphoneRemove(key)),
            _ => {}
        }
    }
}
