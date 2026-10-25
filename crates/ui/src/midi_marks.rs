//! Which controls have a MIDI mapping, for the views to mark.
//!
//! The app builds a [`MidiMarks`] from the project's mappings and the learn state and hands it to
//! the rack and the console ([`crate::RackView::set_midi_marks`],
//! [`crate::ConsoleView::set_midi_marks`]). A mapped control carries a small dot in its top right
//! corner; while MIDI Learn is on every mapped control is outlined, and the control waiting for a
//! hardware control is outlined in the warning colour.

use crate::canvas::{Canvas, CanvasExt};
use crate::theme::Theme;
use atomik_model::DeviceKey;
use std::collections::BTreeSet;
use vello::kurbo::{Circle, Point, Rect, RoundedRect};

/// The mapped controls and the learn state, as the views need them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiMarks {
    /// The `(device, param)` pairs that have a mapping.
    pub mapped: BTreeSet<(DeviceKey, String)>,
    /// The VCA groups whose level has a mapping.
    pub vcas: BTreeSet<u32>,
    /// The control waiting for a hardware control (the learn target).
    pub armed: Option<(DeviceKey, String)>,
    /// MIDI Learn mode is on.
    pub learning: bool,
}

impl MidiMarks {
    /// True when nothing is marked and learning is off (the views then paint nothing extra).
    pub fn is_empty(&self) -> bool {
        self.mapped.is_empty() && self.vcas.is_empty() && self.armed.is_none() && !self.learning
    }

    /// Paints the mark of the control `param` of `device` whose cell is `rect` (in the
    /// coordinates `c` is in).
    pub fn paint_control(
        &self,
        c: &mut dyn Canvas,
        theme: &Theme,
        device: &DeviceKey,
        param: &str,
        rect: Rect,
    ) {
        if self.is_empty() {
            return;
        }
        let key = (device.clone(), param.to_owned());
        let mapped = self.mapped.contains(&key);
        let armed = self.armed.as_ref() == Some(&key);
        self.paint(c, theme, mapped, armed, rect);
    }

    /// Paints the mark of a VCA group's plate or fader cell.
    pub fn paint_vca(&self, c: &mut dyn Canvas, theme: &Theme, id: u32, rect: Rect) {
        if self.is_empty() {
            return;
        }
        self.paint(c, theme, self.vcas.contains(&id), false, rect);
    }

    fn paint(&self, c: &mut dyn Canvas, theme: &Theme, mapped: bool, armed: bool, rect: Rect) {
        if armed {
            c.stroke_with(
                RoundedRect::from_rect(rect.inset(-1.5), 4.0),
                2.0,
                theme.warning,
            );
        } else if mapped && self.learning {
            c.stroke_with(
                RoundedRect::from_rect(rect.inset(-1.0), 3.0),
                1.2,
                theme.highlight.with_alpha(0.8),
            );
        }
        if mapped {
            let at = Point::new(rect.x1 - 3.5, rect.y0 + 3.5);
            c.fill_with(Circle::new(at, 3.0), theme.background);
            c.fill_with(Circle::new(at, 2.2), theme.highlight);
        }
    }
}
