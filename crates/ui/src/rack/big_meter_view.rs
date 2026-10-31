//! The Big Meter overlay of the rack view: where it sits, the peaks it is fed, its clock and its
//! buttons. The state and the maths are in [`crate::big_meter`].

use super::RackView;
use crate::big_meter::{self, BigHit, BigMeter, WIDTH};
use crate::canvas::Canvas;
use crate::event::PointerButton;
use crate::interface::MAX_OUTPUTS;
use vello::kurbo::{Point, Rect};

/// Space between the overlay and the edge of the rack area.
const MARGIN: f64 = 12.0;

impl RackView {
    /// Shows or hides the Big Meter (hidden by default).
    pub fn set_big_meter_visible(&mut self, visible: bool) {
        if self.big.visible != visible {
            self.big.visible = visible;
            self.repaint = true;
        }
    }

    /// Whether the Big Meter is switched on.
    pub fn big_meter_visible(&self) -> bool {
        self.big.visible
    }

    /// The Big Meter's state.
    pub fn big_meter(&self) -> &BigMeter {
        &self.big
    }

    /// The channel counts the pairs are made from: host inputs and outputs, as the panel caps them.
    fn big_channels(&self) -> (usize, usize) {
        (
            usize::from(self.input_channels).min(MAX_OUTPUTS),
            usize::from(self.output_channels).min(MAX_OUTPUTS),
        )
    }

    /// The overlay, view coordinates: at the top right of the rack area, left of the navigator.
    /// `None` while it is off or the area is too narrow for it.
    pub fn big_meter_rect(&self) -> Option<Rect> {
        if !self.big.visible {
            return None;
        }
        let right = self.navigator_rect().map_or(self.size.width, |r| r.x0);
        let x0 = right - MARGIN - WIDTH;
        (x0 >= self.browser_edge() + MARGIN).then(|| {
            Rect::new(
                x0,
                self.bar_height() + MARGIN,
                x0 + WIDTH,
                self.bar_height() + MARGIN + big_meter::HEIGHT,
            )
        })
    }

    /// The pair shown, kept inside the interface's pairs.
    fn big_pair_index(&self) -> usize {
        let (i, o) = self.big_channels();
        let n = i.div_ceil(2) + o.div_ceil(2);
        self.big.pair.min(n.saturating_sub(1))
    }

    /// Hands the per-socket peaks of this frame to the meter (called by `set_io_peaks`).
    pub(super) fn big_meter_feed(&mut self, peaks: &[f32]) {
        if !self.big.visible {
            return;
        }
        let (i, o) = self.big_channels();
        if let Some(sockets) = big_meter::pair_sockets(i, o, self.big_pair_index()) {
            self.big.feed(peaks, sockets);
        }
    }

    /// Moves the meter's ballistics on to `now` (seconds on any monotonic clock). Returns whether
    /// it changed what it shows (a repaint is then already requested).
    pub(super) fn big_meter_tick(&mut self, now_seconds: f64) -> bool {
        if !self.big.visible {
            self.big.pause();
            return false;
        }
        let changed = self.big.advance(now_seconds);
        if changed {
            self.repaint = true;
        }
        changed
    }

    /// A press on the overlay. Returns whether it was on it (it takes every button).
    pub(super) fn big_meter_press(&mut self, pos: Point, button: PointerButton) -> bool {
        let Some(rect) = self.big_meter_rect() else {
            return false;
        };
        let Some(hit) = self.big.hit(rect.origin(), pos) else {
            return false;
        };
        if button != PointerButton::Primary {
            return true;
        }
        let (i, o) = self.big_channels();
        let pairs = i.div_ceil(2) + o.div_ceil(2);
        let at = self.big_pair_index();
        match hit {
            BigHit::Prev if pairs > 0 => self.big.select((at + pairs - 1) % pairs),
            BigHit::Next if pairs > 0 => self.big.select((at + 1) % pairs),
            BigHit::Mode(m) => self.big.mode = m,
            BigHit::Hold(h) => self.big.hold = h,
            BigHit::ReferenceDown => self.big.nudge_reference(-1.0),
            BigHit::ReferenceUp => self.big.nudge_reference(1.0),
            BigHit::Clip(ch) => self.big.reset_clip(Some(ch)),
            BigHit::Bar(ch) => self.big.reset_hold(Some(ch)),
            BigHit::Prev | BigHit::Next | BigHit::Body => {}
        }
        self.repaint = true;
        true
    }

    /// Paints the overlay (after the rack, before the tooltips).
    pub(super) fn paint_big_meter(&self, c: &mut dyn Canvas) {
        let Some(rect) = self.big_meter_rect() else {
            return;
        };
        let (i, o) = self.big_channels();
        let pairs = big_meter::pairs(i, o);
        let label = pairs.get(self.big_pair_index()).map(|p| p.label.as_str());
        big_meter::paint(c, &self.theme, &self.big, rect.origin(), label);
    }
}
