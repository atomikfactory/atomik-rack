//! The saturation panel's type switch (see [`crate::saturation_panel`]): a press picks the
//! position under the pointer and starts a drag that steps through the others.

use super::*;
use crate::fx::FxKind;
use crate::widget::ValueDrag;
use atomik_core::ParamDescriptor;

impl RackView {
    /// Whether param `key` of device `i` is the saturation panel's type switch.
    pub(super) fn is_sat_type(&self, i: usize, key: &str) -> bool {
        key == "type"
            && self
                .devices
                .get(i)
                .is_some_and(|d| d.fx == Some(FxKind::Saturation))
    }

    /// The tooltip text of the saturation knob while it is dragged: OFF at the bottom.
    pub(super) fn sat_knob_text(&self, i: usize, key: &str, value: f32) -> Option<String> {
        (key == "saturation"
            && self
                .devices
                .get(i)
                .is_some_and(|d| d.fx == Some(FxKind::Saturation)))
        .then(|| crate::saturation_panel::knob_text(value))
    }

    /// A primary press on the type switch of device `i` (control `slot`, stored `value`): the
    /// position under the pointer is chosen, and the drag that follows moves one position per
    /// third of the switch (up is up). The whole gesture is one merged undo step.
    pub(super) fn sat_type_down(
        &mut self,
        i: usize,
        slot: usize,
        p: &'static ParamDescriptor,
        value: f32,
        out: &mut Vec<UiAction>,
    ) {
        let d = &self.devices[i];
        let Some(g) = d.spec.fx.and_then(|f| f.saturation) else {
            return;
        };
        let at = self.view_to_rack(self.pointer) - d.offset();
        let picked = p.clamp(f32::from(crate::saturation_panel::type_at(g.switch, at.y)));
        let merge = self.next_merge;
        self.next_merge += 1;
        let key = self.devices[i].key.clone();
        if picked.to_bits() != value.to_bits() {
            out.push(param_cmd(&key, p.key, picked, Some(merge)));
        }
        // One position per band of the cell: the whole range is two bands long.
        let ppr = g.switch.height() / 3.0 * 2.0 * self.zoom;
        self.drag = Drag::Value {
            dev: i,
            slot,
            drag: ValueDrag::new(p, picked, ppr).reversed(),
            merge,
            last_y: self.pointer.y,
            start: value,
            value: picked,
        };
        if let Some(n) = self.tree.find(|h| *h == Hit::Control(i, slot)) {
            self.tree.set_capture(n);
        }
    }
}
