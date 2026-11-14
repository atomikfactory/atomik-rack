//! Meter panels: a spectrum or a phase meter that floats over the main window for one console
//! strip (a channel, a bus or the master) or one plugin device, opened from its right-click menu
//! without inserting a device.
//!
//! The panels are pure views. The app feeds a list of [`MeterPanelState`] once a frame (the
//! trace of the spectrum, the frames of the goniometer, the second curve, the choices of the
//! reference list) and acts on the [`UiAction::MeterPanel`] actions the view returns; the view
//! owns only what is about looking: where each panel sits (a panel is dragged by its title), which
//! source list is open and the pointer. A panel is drawn like the front panel of the device it
//! stands in for, larger.
//!
//! # The spectrum panel
//!
//! The trace over a log frequency axis (20 Hz to 20 kHz) and a 90 dB range, the loudest bin as a
//! readout, **SIZE** arrows for the FFT size and **REF** with a list: another strip, or the
//! reference track, drawn as a second curve over the trace for comparison. The **PRE/POST** chip
//! beside the size taps the strip before its fader or after it (the default).
//!
//! # The phase panel
//!
//! The goniometer of the recent frames, the correlation as a bar and a number, and the level of
//! each side, and the same PRE/POST chip under the readouts.

use crate::action::UiAction;
use crate::analyser::{
    ScopeMode, correlation_text, freq_x, grid_step_db, level_text, level_y, peak_text,
};
use crate::analyser_paint::{PhaseLook, centre_bar, corr_color, goniometer};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{PointerButton, UiEvent};
use crate::paint::fit_label_raw;
use crate::pick_list::CompactList;
use crate::theme::Theme;
use crate::tooltip::Help;
use vello::kurbo::{BezPath, Line, Point, Rect, RoundedRect};

/// Width of a panel, logical pixels.
pub const PANEL_WIDTH: f64 = 380.0;
/// Height of a panel, logical pixels.
pub const PANEL_HEIGHT: f64 = 250.0;
/// The dB the spectrum plot shows below 0 dB.
pub const RANGE_DB: f32 = 90.0;
/// Rows the source list shows at once.
const LIST_ROWS: usize = 10;
/// Height of a row of the source list.
const ROW_H: f64 = 18.0;
/// Width of the source list.
const LIST_W: f64 = 190.0;
const HEADER: f64 = 22.0;
const PAD: f64 = 8.0;
const FONT: f32 = 10.0;
/// The gap the panels keep from the window edge.
const EDGE: f64 = 4.0;

/// Which meter a panel is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterKind {
    /// The spectrum.
    Spectrum,
    /// The phase meter: goniometer and correlation.
    Phase,
}

impl MeterKind {
    /// The word in the title.
    pub fn word(self) -> &'static str {
        match self {
            MeterKind::Spectrum => "SPECTRUM",
            MeterKind::Phase => "PHASE",
        }
    }
}

/// What the app feeds one panel.
#[derive(Debug, Clone, PartialEq)]
pub struct MeterPanelState {
    /// The panel's id (the app's, stable while it is open).
    pub id: u32,
    /// Which meter.
    pub kind: MeterKind,
    /// The name of the strip or device it looks at (drawn as typed).
    pub name: String,
    /// The spectrum trace in dB, one value per column of the plot.
    pub trace: Vec<f32>,
    /// Frequency of the loudest bin, Hz (0 for silence).
    pub peak_hz: f32,
    /// Level of the loudest bin, dB.
    pub peak_db: f32,
    /// The FFT size in points.
    pub size: usize,
    /// The panel taps the signal before the fader (PRE) instead of after it (POST).
    pub pre: bool,
    /// The second curve in dB, one value per column of the plot (empty: none).
    pub second: Vec<f32>,
    /// The choices of the reference list, the first being "no second curve"; the other rows are
    /// strip names (drawn as typed) except the last of a song with a reference track.
    pub sources: Vec<CurveSource>,
    /// The chosen row of `sources`.
    pub source: usize,
    /// Left channel of the recent frames, oldest first (the goniometer).
    pub scope_l: Vec<f32>,
    /// Right channel of the recent frames.
    pub scope_r: Vec<f32>,
    /// The correlation of the recent frames, `-1..=1`.
    pub correlation: f32,
    /// The peak level of the left side in dB.
    pub left_db: f32,
    /// The peak level of the right side in dB.
    pub right_db: f32,
}

/// One row of the reference list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurveSource {
    /// No second curve.
    Off,
    /// A strip, by name.
    Strip(String),
    /// The reference track of the song.
    Reference,
}

impl CurveSource {
    /// The row's label: a strip name is the user's and drawn as typed.
    pub(crate) fn label(&self) -> String {
        match self {
            CurveSource::Off => crate::i18n::tr("Off").into_owned(),
            CurveSource::Strip(n) => n.clone(),
            CurveSource::Reference => crate::i18n::tr("Reference track").into_owned(),
        }
    }
}

/// What a panel asks the app for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeterPanelAction {
    /// Close panel `id`.
    Close(u32),
    /// The FFT size of panel `id` one step up or down.
    Size {
        /// The panel.
        id: u32,
        /// Up (a larger size) or down.
        up: bool,
    },
    /// Switches panel `id` between the point before the fader (PRE) and the one after it (POST).
    Point {
        /// The panel.
        id: u32,
    },
    /// Row `index` of the reference list of panel `id`.
    Source {
        /// The panel.
        id: u32,
        /// The row.
        index: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Close,
    SizeDown,
    SizeUp,
    Point,
    Source,
    Title,
}

#[derive(Debug, Clone)]
struct Slot {
    state: MeterPanelState,
    origin: Point,
    /// The source list is open, scrolled to this first row.
    list: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    id: u32,
    grab: Point,
}

/// The floating meter panels. See the module documentation.
#[derive(Debug)]
pub struct MeterPanelsView {
    /// Bottom to top.
    slots: Vec<Slot>,
    area: (f64, f64),
    hover: Option<Point>,
    list_hover: Option<usize>,
    drag: Option<Drag>,
    repaint: bool,
}

impl Default for MeterPanelsView {
    fn default() -> Self {
        Self::new()
    }
}

impl MeterPanelsView {
    /// No panels.
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            area: (1280.0, 800.0),
            hover: None,
            list_hover: None,
            drag: None,
            repaint: true,
        }
    }

    /// Asks for a repaint (the theme changed).
    pub fn invalidate(&mut self) {
        self.repaint = true;
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The size of the window the panels float in; panels are kept inside it.
    pub fn set_area(&mut self, width: f64, height: f64) {
        if self.area != (width, height) {
            self.area = (width, height);
            for i in 0..self.slots.len() {
                let o = self.slots[i].origin;
                self.slots[i].origin = self.clamped(o);
            }
            self.repaint = true;
        }
    }

    fn clamped(&self, o: Point) -> Point {
        Point::new(
            o.x.min(self.area.0 - PANEL_WIDTH - EDGE).max(EDGE),
            o.y.min(self.area.1 - PANEL_HEIGHT - EDGE).max(EDGE),
        )
    }

    /// Shows `states`, in the order of the list. A panel that is new opens cascaded from the
    /// top left; one that exists keeps its place and its source list; one that is gone is
    /// dropped. A repaint only when something changed.
    pub fn set_panels(&mut self, states: Vec<MeterPanelState>) {
        let before: Vec<MeterPanelState> = self.slots.iter().map(|s| s.state.clone()).collect();
        let mut slots: Vec<Slot> = Vec::with_capacity(states.len());
        // Keep the stacking order of the panels that exist; new ones go on top.
        for old in &self.slots {
            if let Some(state) = states.iter().find(|s| s.id == old.state.id) {
                slots.push(Slot {
                    state: state.clone(),
                    ..old.clone()
                });
            }
        }
        for state in &states {
            if !slots.iter().any(|s| s.state.id == state.id) {
                let n = slots.len() as f64;
                let origin = self.clamped(Point::new(72.0 + 30.0 * n, 96.0 + 30.0 * n));
                slots.push(Slot {
                    state: state.clone(),
                    origin,
                    list: None,
                });
            }
        }
        self.slots = slots;
        let after: Vec<&MeterPanelState> = self.slots.iter().map(|s| &s.state).collect();
        if after.len() != before.len() || after.iter().zip(&before).any(|(a, b)| *a != b) {
            self.repaint = true;
        }
    }

    /// Whether a panel is being dragged by its title.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Whether any panel shows.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The ids of the panels, bottom to top.
    pub fn ids(&self) -> Vec<u32> {
        self.slots.iter().map(|s| s.state.id).collect()
    }

    fn slot(&self, id: u32) -> Option<&Slot> {
        self.slots.iter().find(|s| s.state.id == id)
    }

    /// The rectangle of panel `id`, in window logical pixels.
    pub fn bounds(&self, id: u32) -> Option<Rect> {
        let o = self.slot(id)?.origin;
        Some(Rect::new(o.x, o.y, o.x + PANEL_WIDTH, o.y + PANEL_HEIGHT))
    }

    /// Moves panel `id` so that its top left is `origin` (kept inside the window).
    pub fn move_to(&mut self, id: u32, origin: Point) {
        let o = self.clamped(origin);
        if let Some(s) = self.slots.iter_mut().find(|s| s.state.id == id)
            && s.origin != o
        {
            s.origin = o;
            self.repaint = true;
        }
    }

    /// The `x` that closes panel `id`.
    pub fn close_rect(&self, id: u32) -> Option<Rect> {
        let b = self.bounds(id)?;
        Some(Rect::new(b.x1 - 20.0, b.y0 + 3.0, b.x1 - 4.0, b.y0 + 19.0))
    }

    /// The plot (the spectrum, or the goniometer square of the phase meter).
    pub fn plot_rect(&self, id: u32) -> Option<Rect> {
        let b = self.bounds(id)?;
        let kind = self.slot(id)?.state.kind;
        Some(match kind {
            MeterKind::Spectrum => {
                Rect::new(b.x0 + PAD, b.y0 + HEADER + 4.0, b.x1 - PAD, b.y1 - 54.0)
            }
            MeterKind::Phase => {
                let side = PANEL_HEIGHT - HEADER - 4.0 - PAD;
                Rect::new(
                    b.x0 + PAD,
                    b.y0 + HEADER + 4.0,
                    b.x0 + PAD + side,
                    b.y0 + HEADER + 4.0 + side,
                )
            }
        })
    }

    /// The FFT size arrow: down (`false`) or up (`true`).
    pub fn size_rect(&self, id: u32, up: bool) -> Option<Rect> {
        let b = self.bounds(id)?;
        let y = b.y1 - 22.0;
        Some(if up {
            Rect::new(b.x0 + PAD + 92.0, y, b.x0 + PAD + 108.0, y + 16.0)
        } else {
            Rect::new(b.x0 + PAD + 36.0, y, b.x0 + PAD + 52.0, y + 16.0)
        })
    }

    /// The PRE/POST chip: beside the size arrows of the spectrum, under the readouts of the phase
    /// meter.
    pub fn point_rect(&self, id: u32) -> Option<Rect> {
        let b = self.bounds(id)?;
        Some(match self.slot(id)?.state.kind {
            MeterKind::Spectrum => {
                let y = b.y1 - 22.0;
                Rect::new(b.x0 + PAD + 114.0, y, b.x0 + PAD + 146.0, y + 16.0)
            }
            MeterKind::Phase => {
                let r = self.plot_rect(id)?;
                Rect::new(r.x1 + 14.0, r.y1 - 18.0, r.x1 + 58.0, r.y1 - 2.0)
            }
        })
    }

    /// The button that opens the reference list.
    pub fn source_rect(&self, id: u32) -> Option<Rect> {
        let b = self.bounds(id)?;
        let y = b.y1 - 22.0;
        Some(Rect::new(b.x0 + PAD + 150.0, y, b.x1 - PAD, y + 16.0))
    }

    /// The panel of the open source list of panel `id`.
    pub fn list_rect(&self, id: u32) -> Option<Rect> {
        let slot = self.slot(id)?;
        slot.list?;
        let button = self.source_rect(id)?;
        let rows = slot.state.sources.len().min(LIST_ROWS);
        let h = CompactList::height(rows, ROW_H);
        let x1 = button.x1.min(self.area.0 - EDGE);
        let x0 = (x1 - LIST_W).max(EDGE);
        let y1 = button.y0 - 2.0;
        Some(Rect::new(x0, (y1 - h).max(EDGE), x1, y1.max(EDGE + h)))
    }

    /// The rectangle of row `row` (an index into the sources) of the open list of `id`.
    pub fn list_row_rect(&self, id: u32, row: usize) -> Option<Rect> {
        let slot = self.slot(id)?;
        let top = slot.list?;
        if row < top || row >= top + LIST_ROWS {
            return None;
        }
        let list = CompactList::new(self.list_rect(id)?, ROW_H);
        Some(list.row_rect(row - top))
    }

    fn part_at(&self, id: u32, pos: Point) -> Option<Part> {
        let b = self.bounds(id)?;
        if !b.contains(pos) {
            return None;
        }
        if self.close_rect(id)?.contains(pos) {
            return Some(Part::Close);
        }
        if self.point_rect(id)?.contains(pos) {
            return Some(Part::Point);
        }
        if self.slot(id)?.state.kind == MeterKind::Spectrum {
            if self.size_rect(id, false)?.contains(pos) {
                return Some(Part::SizeDown);
            }
            if self.size_rect(id, true)?.contains(pos) {
                return Some(Part::SizeUp);
            }
            if self.source_rect(id)?.contains(pos) {
                return Some(Part::Source);
            }
        }
        if pos.y < b.y0 + HEADER {
            return Some(Part::Title);
        }
        None
    }

    /// The topmost panel under `pos`.
    fn panel_at(&self, pos: Point) -> Option<u32> {
        self.slots
            .iter()
            .rev()
            .map(|s| s.state.id)
            .find(|id| self.bounds(*id).is_some_and(|b| b.contains(pos)))
    }

    /// The open list under `pos`: `(panel, row index)`.
    fn list_row_at(&self, pos: Point) -> Option<(u32, usize)> {
        for slot in self.slots.iter().rev() {
            let id = slot.state.id;
            let (Some(top), Some(rect)) = (slot.list, self.list_rect(id)) else {
                continue;
            };
            if !rect.contains(pos) {
                continue;
            }
            let rows = slot.state.sources.len().min(LIST_ROWS);
            let list = CompactList::new(rect, ROW_H);
            if let Some(i) = list.row_at(rows, pos) {
                return Some((id, top + i));
            }
            return Some((id, usize::MAX));
        }
        None
    }

    fn raise(&mut self, id: u32) {
        if let Some(at) = self.slots.iter().position(|s| s.state.id == id)
            && at + 1 != self.slots.len()
        {
            let slot = self.slots.remove(at);
            self.slots.push(slot);
            self.repaint = true;
        }
    }

    fn close_lists(&mut self) -> bool {
        let mut any = false;
        for s in &mut self.slots {
            any |= s.list.take().is_some();
        }
        any
    }

    /// The tooltip of the part under `pos`.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let id = self.panel_at(pos)?;
        let b = self.bounds(id)?;
        let kind = self.slot(id)?.state.kind;
        let help = |r: Rect, text: &'static str| Some(Help::new(r, text));
        match self.part_at(id, pos)? {
            Part::Close => help(self.close_rect(id)?, "Close this meter"),
            Part::SizeDown => help(self.size_rect(id, false)?, "A smaller FFT: faster, coarser"),
            Part::SizeUp => help(
                self.size_rect(id, true)?,
                "A larger FFT: finer, slower to move",
            ),
            Part::Point => help(
                self.point_rect(id)?,
                "Tap the strip before its fader (PRE) or after it (POST)",
            ),
            Part::Source => help(
                self.source_rect(id)?,
                "Draw another strip or the reference track as a second curve",
            ),
            Part::Title => help(
                Rect::new(b.x0, b.y0, b.x1, b.y0 + HEADER),
                match kind {
                    MeterKind::Spectrum => "Spectrum of this strip after its fader: drag to move",
                    MeterKind::Phase => "Phase meter of this strip after its fader: drag to move",
                },
            ),
        }
    }

    /// Handles one event (window logical pixels). `(taken, actions)`: a press on a panel or on an
    /// open list is taken; a move is never taken (the tooltip and the hover below still work).
    pub fn handle(&mut self, event: &UiEvent) -> (bool, Vec<UiAction>) {
        match event {
            UiEvent::PointerMove { pos, .. } => {
                if let Some(d) = self.drag {
                    self.move_to(d.id, Point::new(pos.x - d.grab.x, pos.y - d.grab.y));
                    return (true, Vec::new());
                }
                let hover = self.panel_at(*pos).map(|_| *pos);
                let row = self.list_row_at(*pos).map(|(_, r)| r);
                if hover != self.hover || row != self.list_hover {
                    self.hover = hover;
                    self.list_hover = row;
                    self.repaint = true;
                }
                (hover.is_some() || row.is_some(), Vec::new())
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.list_hover = None;
                self.repaint = true;
                (false, Vec::new())
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => {
                let was = self.drag.take().is_some();
                (was, Vec::new())
            }
            UiEvent::Wheel { delta, pos, .. } => {
                if let Some((id, _)) = self.list_row_at(*pos)
                    && let Some(slot) = self.slots.iter_mut().find(|s| s.state.id == id)
                    && let Some(top) = &mut slot.list
                {
                    let max = slot.state.sources.len().saturating_sub(LIST_ROWS);
                    let next = if delta.y > 0.0 {
                        top.saturating_sub(1)
                    } else {
                        (*top + 1).min(max)
                    };
                    if next != *top {
                        *top = next;
                        self.repaint = true;
                    }
                    return (true, Vec::new());
                }
                (false, Vec::new())
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => self.press(*pos),
            _ => (false, Vec::new()),
        }
    }

    fn press(&mut self, pos: Point) -> (bool, Vec<UiAction>) {
        // An open list takes the press first; one outside it closes the lists.
        if let Some((id, row)) = self.list_row_at(pos) {
            self.close_lists();
            self.repaint = true;
            let n = self.slot(id).map_or(0, |s| s.state.sources.len());
            return if row < n {
                (
                    true,
                    vec![UiAction::MeterPanel(MeterPanelAction::Source {
                        id,
                        index: row,
                    })],
                )
            } else {
                (true, Vec::new())
            };
        }
        let closed = self.close_lists();
        if closed {
            self.repaint = true;
        }
        let Some(id) = self.panel_at(pos) else {
            return (false, Vec::new());
        };
        self.raise(id);
        let part = self.part_at(id, pos);
        let action = |a| vec![UiAction::MeterPanel(a)];
        let out = match part {
            Some(Part::Close) => action(MeterPanelAction::Close(id)),
            Some(Part::SizeDown) => action(MeterPanelAction::Size { id, up: false }),
            Some(Part::SizeUp) => action(MeterPanelAction::Size { id, up: true }),
            Some(Part::Point) => action(MeterPanelAction::Point { id }),
            Some(Part::Source) => {
                // Opens (the press that closed it does not reopen it).
                if !closed && let Some(slot) = self.slots.iter_mut().find(|s| s.state.id == id) {
                    let top = slot
                        .state
                        .source
                        .saturating_sub(LIST_ROWS - 1)
                        .min(slot.state.sources.len().saturating_sub(LIST_ROWS));
                    slot.list = Some(top);
                }
                self.repaint = true;
                Vec::new()
            }
            Some(Part::Title) => {
                if let Some(b) = self.bounds(id) {
                    self.drag = Some(Drag {
                        id,
                        grab: Point::new(pos.x - b.x0, pos.y - b.y0),
                    });
                }
                Vec::new()
            }
            None => Vec::new(),
        };
        (true, out)
    }

    /// Paints every panel, then the open list on top.
    pub fn paint(&mut self, c: &mut dyn Canvas, t: &Theme) {
        for i in 0..self.slots.len() {
            self.paint_panel(c, t, i);
        }
        for i in 0..self.slots.len() {
            self.paint_list(c, t, i);
        }
        self.repaint = false;
    }

    fn paint_panel(&self, c: &mut dyn Canvas, t: &Theme, i: usize) {
        let slot = &self.slots[i];
        let st = &slot.state;
        let id = st.id;
        let Some(b) = self.bounds(id) else { return };
        c.fill_with(
            RoundedRect::from_rect(b.inset(2.0) + vello::kurbo::Vec2::new(0.0, 3.0), 8.0),
            t.cable_shadow,
        );
        let card = RoundedRect::from_rect(b, 8.0);
        c.fill_with(card, t.browser_bg);
        c.stroke_with(card, 1.0, t.section_line);
        // Title: the kind, then the strip's name as typed.
        c.text(
            &TextRun::new(st.kind.word(), pt(b.x0 + PAD, b.y0 + 15.0), FONT, t.text).weight(700),
        );
        let name_x = b.x0 + PAD + 62.0;
        c.text(&TextRun::verbatim(
            fit_label_raw(&st.name, b.x1 - 28.0 - name_x, FONT),
            pt(name_x, b.y0 + 15.0),
            FONT,
            t.text_muted,
        ));
        if let Some(close) = self.close_rect(id) {
            let hot = self.hover.is_some_and(|p| close.contains(p));
            c.text(
                &TextRun::new(
                    "\u{d7}",
                    pt(close.center().x, close.y0 + 13.0),
                    13.0,
                    if hot { t.text } else { t.text_muted },
                )
                .align(Align::Center),
            );
        }
        match st.kind {
            MeterKind::Spectrum => self.paint_spectrum(c, t, i),
            MeterKind::Phase => self.paint_phase(c, t, i),
        }
        self.paint_point(c, t, i);
    }

    /// The PRE/POST chip: lit while the strip is tapped before its fader.
    fn paint_point(&self, c: &mut dyn Canvas, t: &Theme, i: usize) {
        let st = &self.slots[i].state;
        let Some(a) = self.point_rect(st.id) else {
            return;
        };
        let hot = self.hover.is_some_and(|p| a.contains(p));
        let face = if st.pre {
            t.focus.with_alpha(0.3)
        } else if hot {
            t.browser_row
        } else {
            t.background
        };
        c.fill_with(RoundedRect::from_rect(a, 3.0), face);
        c.text(
            &TextRun::new(
                if st.pre { "PRE" } else { "POST" },
                pt(a.center().x, a.y0 + 12.0),
                9.0,
                if st.pre { t.text } else { t.text_muted },
            )
            .align(Align::Center)
            .weight(600),
        );
    }

    fn paint_spectrum(&self, c: &mut dyn Canvas, t: &Theme, i: usize) {
        let st = &self.slots[i].state;
        let id = st.id;
        let (Some(b), Some(r)) = (self.bounds(id), self.plot_rect(id)) else {
            return;
        };
        c.fill_with(RoundedRect::from_rect(r, 3.0), t.lcd);
        c.push_clip(&r.into());
        let step = grid_step_db(RANGE_DB);
        let mut db = 0.0f32;
        while db <= RANGE_DB + 0.01 {
            let y = level_y(r, -db, RANGE_DB).round() + 0.5;
            c.stroke_with(
                Line::new(pt(r.x0, y), pt(r.x1, y)),
                1.0,
                t.ink.with_alpha(if db == 0.0 { 0.2 } else { 0.06 }),
            );
            db += step;
        }
        for f in [
            30.0, 50.0, 100.0, 200.0, 300.0, 500.0, 1000.0, 2000.0, 3000.0, 5000.0, 10_000.0,
        ] {
            let x = freq_x(r, f).round() + 0.5;
            let major = matches!(f as u32, 100 | 1000 | 10_000);
            c.stroke_with(
                Line::new(pt(x, r.y0), pt(x, r.y1)),
                1.0,
                t.ink.with_alpha(if major { 0.14 } else { 0.05 }),
            );
        }
        // The second curve under the trace, so the trace stays the one to read.
        self.curve(c, r, &st.second, t.warning.with_alpha(0.9), 1.5);
        self.curve(c, r, &st.trace, t.focus, 1.4);
        c.pop_clip();
        for (f, label) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
            c.text(
                &TextRun::verbatim(label, pt(freq_x(r, f) + 2.0, r.y1 - 3.0), 8.0, t.text_muted)
                    .weight(600),
            );
        }
        // Readouts under the plot: the loudest bin, and the legend of the second curve.
        let row_y = r.y1 + 13.0;
        c.text(&TextRun::new("PEAK", pt(r.x0, row_y), 9.0, t.text_muted).weight(600));
        let peak = peak_text(st.peak_hz, st.peak_db);
        let value = peak.strip_prefix("PEAK").unwrap_or(&peak).trim().to_owned();
        c.text(&TextRun::verbatim(value, pt(r.x0 + 34.0, row_y), 9.0, t.text).weight(600));
        if !st.second.is_empty() {
            let x = r.x1 - 90.0;
            c.stroke_with(
                Line::new(pt(x, row_y - 3.0), pt(x + 14.0, row_y - 3.0)),
                2.0,
                t.warning,
            );
            c.text(&TextRun::new("REF", pt(x + 18.0, row_y), 9.0, t.text_muted).weight(600));
        }
        // SIZE arrows and the REF button.
        let y = b.y1 - 22.0 + 12.0;
        c.text(&TextRun::new("SIZE", pt(b.x0 + PAD, y), 9.0, t.text_muted).weight(600));
        for up in [false, true] {
            if let Some(a) = self.size_rect(id, up) {
                let hot = self.hover.is_some_and(|p| a.contains(p));
                c.fill_with(
                    RoundedRect::from_rect(a, 3.0),
                    if hot { t.browser_row } else { t.background },
                );
                c.text(
                    &TextRun::verbatim(
                        if up { "\u{25b8}" } else { "\u{25c2}" },
                        pt(a.center().x, a.y0 + 12.0),
                        10.0,
                        t.text,
                    )
                    .align(Align::Center),
                );
            }
        }
        if let (Some(lo), Some(hi)) = (self.size_rect(id, false), self.size_rect(id, true)) {
            c.text(
                &TextRun::verbatim(
                    st.size.to_string(),
                    pt((lo.x1 + hi.x0) / 2.0, lo.y0 + 12.0),
                    9.5,
                    t.text,
                )
                .align(Align::Center)
                .weight(600),
            );
        }
        if let Some(a) = self.source_rect(id) {
            let hot = self.hover.is_some_and(|p| a.contains(p)) || self.slots[i].list.is_some();
            c.fill_with(
                RoundedRect::from_rect(a, 3.0),
                if hot { t.browser_row } else { t.background },
            );
            c.text(
                &TextRun::new("REF", pt(a.x0 + 6.0, a.y0 + 12.0), 9.0, t.text_muted).weight(600),
            );
            let label = st
                .sources
                .get(st.source)
                .map_or_else(|| crate::i18n::tr("Off").into_owned(), CurveSource::label);
            let fitted = fit_label_raw(&label, a.width() - 52.0, 9.5);
            let colour = if st.source == 0 { t.text_muted } else { t.text };
            c.text(&TextRun::verbatim(
                fitted,
                pt(a.x0 + 30.0, a.y0 + 12.0),
                9.5,
                colour,
            ));
            c.text(
                &TextRun::verbatim("\u{25be}", pt(a.x1 - 8.0, a.y0 + 12.0), 9.0, t.text_muted)
                    .align(Align::Center),
            );
        }
    }

    /// One polyline of `values` (dB per column) across the plot.
    fn curve(
        &self,
        c: &mut dyn Canvas,
        r: Rect,
        values: &[f32],
        colour: vello::peniko::Color,
        w: f64,
    ) {
        let n = values.len();
        if n < 2 {
            return;
        }
        let mut path = BezPath::new();
        let mut down = false;
        for (k, v) in values.iter().enumerate() {
            if *v <= -150.0 {
                down = false;
                continue;
            }
            let x = r.x0 + (k as f64 + 0.5) / n as f64 * r.width();
            let p = pt(x, level_y(r, *v, RANGE_DB));
            if down {
                path.line_to(p);
            } else {
                path.move_to(p);
                down = true;
            }
        }
        if !path.elements().is_empty() {
            c.stroke_with(path, w, colour);
        }
    }

    fn paint_phase(&self, c: &mut dyn Canvas, t: &Theme, i: usize) {
        let st = &self.slots[i].state;
        let id = st.id;
        let (Some(b), Some(r)) = (self.bounds(id), self.plot_rect(id)) else {
            return;
        };
        let data = crate::analyser::AnalyserData {
            scope_l: st.scope_l.clone(),
            scope_r: st.scope_r.clone(),
            ..Default::default()
        };
        let look = PhaseLook {
            mode: ScopeMode::Goniometer,
            gain_db: 0.0,
            loudness: false,
            history: false,
            target: -14.0,
            bypassed: false,
        };
        goniometer(c, t, r, &data, look, t.focus);
        // The readouts to the right of the square.
        let x0 = r.x1 + 14.0;
        let x1 = b.x1 - PAD;
        c.text(&TextRun::new("CORRELATION", pt(x0, r.y0 + 12.0), 9.0, t.text_muted).weight(600));
        let bar = Rect::new(x0, r.y0 + 20.0, x1, r.y0 + 30.0);
        centre_bar(c, bar, st.correlation, corr_color(st.correlation));
        for (label, x, align) in [("-1", x0, Align::Start), ("+1", x1, Align::End)] {
            c.text(&TextRun::verbatim(label, pt(x, r.y0 + 44.0), 8.0, t.text_muted).align(align));
        }
        c.text(
            &TextRun::verbatim(
                correlation_text(st.correlation),
                pt((x0 + x1) / 2.0, r.y0 + 46.0),
                14.0,
                t.text,
            )
            .align(Align::Center)
            .weight(700),
        );
        for (k, (label, db)) in [("L", st.left_db), ("R", st.right_db)]
            .into_iter()
            .enumerate()
        {
            let y = r.y0 + 82.0 + k as f64 * 34.0;
            c.text(&TextRun::new(label, pt(x0, y), 9.0, t.text_muted).weight(600));
            c.text(
                &TextRun::verbatim(format!("{} dB", level_text(db)), pt(x1, y), 11.0, t.text)
                    .align(Align::End)
                    .weight(600),
            );
            let level = f64::from(((db + 60.0) / 60.0).clamp(0.0, 1.0));
            let track = Rect::new(x0, y + 6.0, x1, y + 12.0);
            c.fill_with(RoundedRect::from_rect(track, 2.0), t.background);
            if level > 0.0 {
                c.fill_with(
                    RoundedRect::from_rect(
                        Rect::new(
                            track.x0,
                            track.y0,
                            track.x0 + track.width() * level,
                            track.y1,
                        ),
                        2.0,
                    ),
                    t.focus,
                );
            }
        }
    }

    fn paint_list(&self, c: &mut dyn Canvas, t: &Theme, i: usize) {
        let slot = &self.slots[i];
        let id = slot.state.id;
        let (Some(top), Some(rect)) = (slot.list, self.list_rect(id)) else {
            return;
        };
        let mut list = CompactList::new(rect, ROW_H);
        list.paint_frame(c, t);
        let rows = slot.state.sources.len().min(LIST_ROWS);
        let hover = self
            .list_hover
            .and_then(|r| r.checked_sub(top))
            .filter(|r| *r < rows);
        list.hover = hover;
        if let Some(h) = hover {
            list.paint_hover(c, t, h);
        }
        for k in 0..rows {
            let Some(source) = slot.state.sources.get(top + k) else {
                break;
            };
            let row = list.row_rect(k);
            let label = source.label();
            let chosen = top + k == slot.state.source;
            c.text(&TextRun::verbatim(
                fit_label_raw(&label, row.width() - 20.0, 10.0),
                pt(row.x0 + 14.0, row.y0 + 13.0),
                10.0,
                if chosen { t.text } else { t.text_muted },
            ));
            if chosen {
                c.text(&TextRun::verbatim(
                    "\u{2713}",
                    pt(row.x0 + 3.0, row.y0 + 13.0),
                    10.0,
                    t.focus,
                ));
            }
        }
    }
}
