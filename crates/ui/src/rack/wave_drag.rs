//! Setting a grain sampler's position by clicking or dragging on its waveform.
//!
//! A press on the waveform sets `pos.position` to the place under the pointer (the left edge of
//! the display is the start of the sample window, the right edge its end) and a drag moves it,
//! as one undo step: every change shares a merge id and the release closes it. Escape restores
//! the value the press found. The value is rounded to a tenth of a percent, so a drag does not
//! flood the document with commands.

use super::{Drag, RackView, UiAction, param_cmd};
use crate::granular_panel::{position_at, wave_hit_area};
use vello::kurbo::{Point, Rect};

/// The param the waveform sets.
const POSITION: &str = "pos.position";

/// State of a drag on a granular waveform.
#[derive(Debug, Clone)]
pub(super) struct WaveDrag {
    dev: usize,
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
        let (Some(area), Some(p)) = (self.wave_area(i), self.param_of_key(i, POSITION)) else {
            return;
        };
        let start = self.devices[i]
            .device
            .as_ref()
            .map_or(p.default, |dev| crate::widget::current_value(dev, p));
        let merge = self.next_merge;
        self.next_merge += 1;
        let value = p.clamp(position_at(area, rp.x));
        out.push(param_cmd(
            &self.devices[i].key,
            POSITION,
            value,
            Some(merge),
        ));
        self.drag = Drag::Wave(WaveDrag {
            dev: i,
            merge,
            start,
            value,
        });
        self.repaint = true;
    }

    /// The pointer moves during a waveform drag.
    pub(super) fn wave_move(&mut self, mut w: WaveDrag, rp: Point, out: &mut Vec<UiAction>) {
        if let (Some(area), Some(p)) = (self.wave_area(w.dev), self.param_of_key(w.dev, POSITION)) {
            let value = p.clamp(position_at(area, rp.x));
            if value.to_bits() != w.value.to_bits() {
                out.push(param_cmd(
                    &self.devices[w.dev].key,
                    POSITION,
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
                POSITION,
                w.start,
                Some(w.merge),
            ));
        }
        out.push(UiAction::EndMerge);
    }
}
