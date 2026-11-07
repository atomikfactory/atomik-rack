//! The energy lane of the [`ArrangementView`]: one row above the track lanes that shows, for every
//! section of the song, how much energy it has (a bar as tall as the 0-1 value) and marks the
//! sections the energy map flags (see [`atomik_model::energy_map`]).
//!
//! The lane is a view option, off by default. It only paints what the app hands it: the map comes
//! from a worker that renders the master ([`ArrangementView::set_energy_map`]), a progress while
//! that runs and a short note for the header. A map that no longer matches the song is drawn
//! faded and says so in the header.

use super::ArrangementView;
use crate::canvas::{Align, Canvas};
use crate::seq_paint as sp;
use crate::theme::mix;
use atomik_model::energy_map::{EnergyMap, Flag};
use std::sync::Arc;
use vello::kurbo::Rect;

/// Height of the energy lane.
pub const ENERGY_LANE_HEIGHT: f64 = 46.0;

/// What the lane shows.
#[derive(Debug, Clone, Default)]
pub(super) struct EnergyUi {
    /// The map to draw.
    map: Option<Arc<EnergyMap>>,
    /// The map is out of date: the song changed after it was measured.
    stale: bool,
    /// How far a running measurement is, 0 to 1.
    progress: Option<f32>,
    /// A line for the header cell (why there is no map, for instance).
    note: Option<String>,
}

/// The short word a flag is drawn with.
fn flag_word(flag: &Flag) -> &'static str {
    match flag {
        Flag::BuildLouderThanDrop { .. } => "build > drop",
        Flag::DropNotTop { .. } => "drop not top",
        Flag::FlatRun { .. } => "flat",
    }
}

impl ArrangementView {
    // ------------------------------------------------------------------ host API

    /// Whether the energy lane is shown above the track lanes.
    pub fn energy_lane_visible(&self) -> bool {
        self.energy_lane
    }

    /// Shows or hides the energy lane.
    pub fn set_energy_lane(&mut self, on: bool) {
        if on != self.energy_lane {
            self.energy_lane = on;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// The lane's rectangle (empty when it is hidden).
    pub fn energy_lane_rect(&self) -> Rect {
        self.energy_rect()
    }

    /// The map the lane draws, and whether it is out of date.
    pub fn energy_map(&self) -> Option<(&EnergyMap, bool)> {
        self.en.map.as_deref().map(|m| (m, self.en.stale))
    }

    /// Hands the lane a map (`None` clears it) and whether it is out of date.
    pub fn set_energy_map(&mut self, map: Option<Arc<EnergyMap>>, stale: bool) {
        let same = match (&self.en.map, &map) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || **a == **b,
            (None, None) => true,
            _ => false,
        };
        if !same || self.en.stale != stale {
            self.en.map = map;
            self.en.stale = stale;
            self.repaint = true;
        }
    }

    /// Shows the progress of a running measurement (`None`: not running).
    pub fn set_energy_progress(&mut self, progress: Option<f32>) {
        let progress = progress.map(|p| p.clamp(0.0, 1.0));
        if self.en.progress != progress {
            self.en.progress = progress;
            self.repaint = true;
        }
    }

    /// Sets the line shown in the lane's header cell under its name.
    pub fn set_energy_note(&mut self, note: Option<String>) {
        if self.en.note != note {
            self.en.note = note;
            self.repaint = true;
        }
    }

    // ------------------------------------------------------------------ painting

    /// The energy lane and its header cell.
    pub(super) fn paint_energy_lane(&self, c: &mut dyn Canvas) {
        if !self.energy_lane {
            return;
        }
        let st = &self.st;
        let t = &st.base;
        let rect = self.energy_rect();
        let header = Rect::new(self.bounds.x0, rect.y0, rect.x0, rect.y1);
        sp::rect(c, header, st.header_lane);
        sp::label(
            c,
            "ENERGY",
            header.x0 + 12.0,
            header.y0 + 17.0,
            8.0,
            t.text_muted,
            Align::Start,
        );
        let line = if let Some(p) = self.en.progress {
            format!("measuring {:.0}%", p * 100.0)
        } else if let Some(note) = &self.en.note {
            note.clone()
        } else if let Some(map) = &self.en.map {
            let flags = map.flags.len();
            match (self.en.stale, flags) {
                (true, _) => "out of date".to_owned(),
                (false, 0) => format!("{} sections", map.sections.len()),
                (false, 1) => "1 flag".to_owned(),
                (false, n) => format!("{n} flags"),
            }
        } else {
            "not measured".to_owned()
        };
        sp::label(
            c,
            &line,
            header.x0 + 12.0,
            header.y0 + 34.0,
            10.0,
            t.highlight,
            Align::Start,
        );
        sp::rect(c, rect, st.lane_auto);
        c.push_clip(&rect.into());
        if let Some(map) = &self.en.map {
            self.paint_energy_sections(c, map, rect);
        }
        if let Some(p) = self.en.progress {
            let w = (rect.width() * f64::from(p)).max(0.0);
            sp::rect(
                c,
                Rect::new(rect.x0, rect.y1 - 2.0, rect.x0 + w, rect.y1),
                t.highlight,
            );
        }
        c.pop_clip();
        sp::line(
            c,
            header.x0,
            rect.y1 - 0.5,
            rect.x1,
            rect.y1 - 0.5,
            st.separator,
        );
        sp::line(
            c,
            rect.x0 - 0.5,
            rect.y0,
            rect.x0 - 0.5,
            rect.y1,
            st.separator,
        );
    }

    /// The sections: a bar per section as tall as its energy, its name and value, and the flags.
    fn paint_energy_sections(&self, c: &mut dyn Canvas, map: &EnergyMap, rect: Rect) {
        let st = &self.st;
        let t = &st.base;
        let fade = if self.en.stale { 0.45 } else { 1.0 };
        for (i, s) in map.sections.iter().enumerate() {
            let x0 = self.axis.tick_to_x(s.section.start).max(rect.x0);
            let x1 = self.axis.tick_to_x(s.section.end).min(rect.x1);
            if x1 <= x0 {
                continue;
            }
            let flagged = map.flags_of(i).next().is_some();
            let cell = Rect::new(x0, rect.y0 + 1.0, x1, rect.y1 - 1.0);
            let e = f64::from(s.energy.clamp(0.0, 1.0));
            let tall = (cell.height() * e).max(1.0);
            let bar = Rect::new(cell.x0 + 1.0, cell.y1 - tall, cell.x1 - 1.0, cell.y1);
            let color = mix(t.text_muted, t.highlight, s.energy.clamp(0.0, 1.0)).with_alpha(fade);
            sp::rect(c, bar, color);
            let edge = if flagged { st.mute } else { st.separator };
            sp::line(c, x0, cell.y0, x0, cell.y1, edge);
            c.push_clip(&cell.into());
            sp::label(
                c,
                &format!("{} {:.2}", s.section.name, s.energy),
                cell.x0 + 4.0,
                cell.y0 + 10.0,
                8.5,
                t.text,
                Align::Start,
            );
            if let Some(flag) = map.flags_of(i).next() {
                sp::label(
                    c,
                    &format!("! {}", flag_word(flag)),
                    cell.x0 + 4.0,
                    cell.y1 - 4.0,
                    8.0,
                    st.mute,
                    Align::Start,
                );
            }
            c.pop_clip();
        }
    }
}
