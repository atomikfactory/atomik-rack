//! Setting a grain sampler position, or its spray, by clicking or dragging on its waveform.
//!
//! A press on the waveform sets `pos.position` to the place under the pointer (the left edge of
//! the display is the start of the sample window, the right edge its end) and a drag moves it,
//! as one undo step: every change shares a merge id and the release closes it. Escape restores
//! the value the press found. The value is rounded to a tenth of a percent, so a drag does not
//! flood the document with commands. A press on an edge of the spray band drags `pos.spray`
//! the same way ([`crate::granular_panel::spray_edge_at`]).

use super::{Drag, RackView, UiAction, param_cmd};
use crate::granular_panel::{markers, position_at, spray_at, spray_edge_at, wave_hit_area};
use vello::kurbo::{Point, Rect};

/// The param a press on the waveform sets.
const POSITION: &str = "pos.position";
/// The param an edge of the spray band sets.
const SPRAY: &str = "pos.spray";

/// State of a drag on a granular waveform.
#[derive(Debug, Clone)]
pub(super) struct WaveDrag {
    dev: usize,
    /// The param the drag sets: [`POSITION`] or [`SPRAY`].
    param: &'static str,
    /// The centre of the spray band when the drag moves its edge.
    centre: f32,
    merge: u64,
    /// The value the press found.
    start: f32,
    /// The last value sent.
    value: f32,
}

impl RackView {
    /// The part of device `i`'s waveform that takes a press (rack coordinates).
    pub(super) fn wave_area(&self, i: usize) -> Option<Rect> {
        let d = self.devices.get(i)?;
        let g = d.spec.granular?;
        Some(wave_hit_area(g.wave) + d.offset())
    }

    /// A press on the waveform: selects the device and moves the position under the pointer.
    pub(super) fn wave_down(&mut self, i: usize, rp: Point, out: &mut Vec<UiAction>) {
        self.select(i, out);
        let Some(area) = self.wave_area(i) else {
            return;
        };
        let (param, centre) = self.wave_target(i, area, rp);
        let Some(p) = self.param_of_key(i, param) else {
            return;
        };
        let start = self.devices[i]
            .device
            .as_ref()
            .map_or(p.default, |dev| crate::widget::current_value(dev, p));
        let merge = self.next_merge;
        self.next_merge += 1;
        let value = p.clamp(wave_value(param, area, centre, rp.x));
        out.push(param_cmd(&self.devices[i].key, param, value, Some(merge)));
        self.drag = Drag::Wave(WaveDrag {
            dev: i,
            param,
            centre,
            merge,
            start,
            value,
        });
        self.repaint = true;
    }

    /// What a press at `rp` on the waveform of device `i` sets: the spray when it lands on an
    /// edge of the band drawn now, else the position. The second value is the band centre.
    fn wave_target(&self, i: usize, area: Rect, rp: Point) -> (&'static str, f32) {
        let d = &self.devices[i];
        let (Some(dev), Some(desc)) = (d.device.as_ref(), d.desc) else {
            return (POSITION, 0.0);
        };
        let (position, spray) = markers(dev, &desc, &d.meters);
        if spray_edge_at(area, position, spray, rp) {
            (SPRAY, position)
        } else {
            (POSITION, position)
        }
    }

    /// The pointer moves during a waveform drag.
    pub(super) fn wave_move(&mut self, mut w: WaveDrag, rp: Point, out: &mut Vec<UiAction>) {
        if let (Some(area), Some(p)) = (self.wave_area(w.dev), self.param_of_key(w.dev, w.param)) {
            let value = p.clamp(wave_value(w.param, area, w.centre, rp.x));
            if value.to_bits() != w.value.to_bits() {
                out.push(param_cmd(
                    &self.devices[w.dev].key,
                    w.param,
                    value,
                    Some(w.merge),
                ));
                w.value = value;
                self.repaint = true;
            }
        }
        self.drag = Drag::Wave(w);
    }

    /// Escape during a waveform drag: puts the start value back.
    pub(super) fn wave_cancel(&mut self, w: &WaveDrag, out: &mut Vec<UiAction>) {
        if w.value.to_bits() != w.start.to_bits() {
            out.push(param_cmd(
                &self.devices[w.dev].key,
                w.param,
                w.start,
                Some(w.merge),
            ));
        }
        out.push(UiAction::EndMerge);
    }
}

/// The value of `param` for a pointer at x `x`: the position there, or the spray that puts the
/// edge of the band there.
fn wave_value(param: &str, area: Rect, centre: f32, x: f64) -> f32 {
    if param == SPRAY {
        spray_at(area, centre, x)
    } else {
        position_at(area, x)
    }
}
