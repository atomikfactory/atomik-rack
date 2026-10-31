//! The MIDI Mappings panel: every mapping of the project, with its source, target, range, mode
//! and switches, to edit or delete.
//!
//! ```text
//! MIDI MAPPINGS                                     [ EXPORT ] [ IMPORT ] [ CLEAR ] [ CLOSE ]
//! 3 mappings. Click a mode to change it, click or scroll a range to move it, Learn to pick
//! another control. Moves of a mapped control are consumed: they play no notes.
//!   SOURCE            TARGET               MIN   MAX  MODE        INV PICK CONS
//!   CC 7 ch 1         channel fader         0%  100%  Absolute    ·   ●    ●   [LEARN] [x]
//! ```
//!
//! A modal view the app places over the window like the relink panel. It has no document: the
//! app hands it the rows with [`MidiMapsView::set_rows`] (every frame is fine) and what the user
//! does arrives as [`UiAction::MidiMaps`]:
//!
//! * a click on a **mode** cycles it forward, a right click backward (the relative encoders are
//!   skipped for a source that is not a 7 bit controller); the **invert**, **pickup** and
//!   **consume** chips toggle; a click on a **range** cell moves it up by 5 %, a right click down,
//!   and the wheel moves it too (Shift: 1 %). Each is [`MidiMapsAction::Edit`] with the whole
//!   mapping changed, which the app turns into one undo step;
//! * **LEARN** is [`MidiMapsAction::Relearn`] (the app arms the mapping's target), the cross is
//!   [`MidiMapsAction::Remove`];
//! * the header buttons are [`MidiMapsAction::Export`], [`MidiMapsAction::Import`] (preset
//!   files), [`MidiMapsAction::Clear`] and [`MidiMapsAction::Close`].
//!
//! A mapping whose target is gone is painted in the warning colour and marked missing. The list
//! scrolls with the wheel over the rows; only the rows in view are painted.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::Theme;
use crate::tooltip::Help;
use atomik_model::{MapMode, MidiMapping, RelativeKind};
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const HINT_H: f64 = 36.0;
const COLUMNS_H: f64 = 20.0;
const ROW_H: f64 = 26.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 980.0;
const MIN_WIDTH: f64 = 640.0;
const BTN_H: f64 = 20.0;
const RANGE_STEP: f64 = 0.05;
const RANGE_STEP_FINE: f64 = 0.01;

/// One row of the panel: a mapping, its target as text and whether the target is gone.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiMapRow {
    /// The mapping.
    pub mapping: MidiMapping,
    /// The target as text (`channel fader`, `VCA 2`, `Tempo`).
    pub target: String,
    /// The target no longer exists (the mapping does nothing).
    pub missing: bool,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum MidiMapsAction {
    /// Replace the mapping (same id) with this edited one.
    Edit(MidiMapping),
    /// Delete the mapping.
    Remove(u32),
    /// Arm the mapping's target: the next control moved becomes its source.
    Relearn(u32),
    /// Pick a file and save the mappings as a preset.
    Export,
    /// Pick a preset file and add its mappings.
    Import,
    /// Delete every mapping.
    Clear,
    /// Close the panel.
    Close,
}

/// A place in the panel a pointer can hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Close,
    Export,
    Import,
    Clear,
    Min(usize),
    Max(usize),
    Mode(usize),
    Invert(usize),
    Pickup(usize),
    Consume(usize),
    Learn(usize),
    Remove(usize),
}

/// The next mode in the cycle for a source that may or may not be a 7 bit controller.
pub fn cycle_mode(mode: MapMode, cc: bool, forward: bool) -> MapMode {
    let all = [
        MapMode::Absolute,
        MapMode::Relative(RelativeKind::TwosComplement),
        MapMode::Relative(RelativeKind::BinaryOffset),
        MapMode::Relative(RelativeKind::SignMagnitude),
        MapMode::Toggle,
        MapMode::Momentary,
    ];
    let usable: Vec<MapMode> = all
        .into_iter()
        .filter(|m| cc || !matches!(m, MapMode::Relative(_)))
        .collect();
    let at = usable.iter().position(|m| *m == mode).unwrap_or(0);
    let n = usable.len();
    usable[if forward {
        (at + 1) % n
    } else {
        (at + n - 1) % n
    }]
}

fn mode_text(mode: MapMode) -> &'static str {
    match mode {
        MapMode::Absolute => "Absolute",
        MapMode::Relative(RelativeKind::TwosComplement) => "Rel 2's",
        MapMode::Relative(RelativeKind::BinaryOffset) => "Rel offset",
        MapMode::Relative(RelativeKind::SignMagnitude) => "Rel sign",
        MapMode::Toggle => "Toggle",
        MapMode::Momentary => "Momentary",
    }
}

fn percent(x: f32) -> String {
    format!("{}%", (x * 100.0).round() as i32)
}

/// The MIDI Mappings panel. See the module documentation.
#[derive(Debug)]
pub struct MidiMapsView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    rows: Vec<MidiMapRow>,
    scroll: f64,
    hover: Option<Point>,
    repaint: bool,
}

impl MidiMapsView {
    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// An empty panel; call [`Self::set_bounds`] and [`Self::set_rows`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 1000.0, 600.0),
            scale: 1.0,
            rows: Vec::new(),
            scroll: 0.0,
            hover: None,
            repaint: true,
        }
    }

    /// Places the panel: the area it may use (logical pixels).
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Replaces the rows. Cheap when nothing changed.
    pub fn set_rows(&mut self, rows: Vec<MidiMapRow>) {
        if rows == self.rows {
            return;
        }
        self.rows = rows;
        self.clamp_scroll();
        self.repaint = true;
    }

    /// The listed rows.
    pub fn rows(&self) -> &[MidiMapRow] {
        &self.rows
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The list's scroll offset in pixels.
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN)
            .clamp(MIN_WIDTH.min(self.bounds.width()), MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let wanted = HEADER_H + HINT_H + COLUMNS_H + self.rows.len().max(1) as f64 * ROW_H + PAD;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + wanted.min(room))
    }

    fn list_area(&self) -> Rect {
        let p = self.panel();
        Rect::new(p.x0, p.y0 + HEADER_H + HINT_H + COLUMNS_H, p.x1, p.y1 - PAD)
    }

    fn clamp_scroll(&mut self) {
        let content = self.rows.len() as f64 * ROW_H;
        let max = (content - self.list_area().height()).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// A header button, counted from the right edge.
    fn header_button(&self, from_right: usize, w: f64) -> Rect {
        let p = self.panel();
        let right = p.x1 - PAD - from_right as f64 * 90.0;
        Rect::new(right - w, p.y0 + 7.0, right, p.y0 + 7.0 + BTN_H)
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        self.header_button(0, 80.0)
    }

    /// The CLEAR button.
    pub fn clear_rect(&self) -> Rect {
        self.header_button(1, 80.0)
    }

    /// The IMPORT button.
    pub fn import_rect(&self) -> Rect {
        self.header_button(2, 80.0)
    }

    /// The EXPORT button.
    pub fn export_rect(&self) -> Rect {
        self.header_button(3, 80.0)
    }

    /// The row of entry `i`, `None` when it is scrolled out of the list.
    pub fn row_rect(&self, i: usize) -> Option<Rect> {
        if i >= self.rows.len() {
            return None;
        }
        let area = self.list_area();
        let y = area.y0 + i as f64 * ROW_H - self.scroll;
        let row = Rect::new(area.x0, y, area.x1, y + ROW_H);
        (row.y0 >= area.y0 - 0.5 && row.y1 <= area.y1 + 0.5).then_some(row)
    }

    /// The x extents of the fixed columns, from the right edge of a row: remove, learn,
    /// consume, pickup, invert, mode, max, min.
    fn columns(&self, row: Rect) -> Columns {
        let mut right = row.x1 - PAD;
        let mut take = |w: f64, gap: f64| {
            let r = (right - w, right);
            right = r.0 - gap;
            r
        };
        let remove = take(22.0, 6.0);
        let learn = take(54.0, 10.0);
        let consume = take(54.0, 4.0);
        let pickup = take(46.0, 4.0);
        let invert = take(44.0, 10.0);
        let mode = take(86.0, 10.0);
        let max = take(40.0, 4.0);
        let min = take(40.0, 12.0);
        let flex = (min.0 - 12.0 - (row.x0 + PAD)).max(40.0);
        let source_w = (flex * 0.42).floor();
        Columns {
            source: (row.x0 + PAD, row.x0 + PAD + source_w),
            target: (row.x0 + PAD + source_w + 8.0, min.0 - 12.0),
            min,
            max,
            mode,
            invert,
            pickup,
            consume,
            learn,
            remove,
        }
    }

    fn cell(&self, row: Rect, x: (f64, f64)) -> Rect {
        let y = row.y0 + (ROW_H - BTN_H) / 2.0;
        Rect::new(x.0, y, x.1, y + BTN_H)
    }

    fn hit(&self, pos: Point) -> Option<Hit> {
        for (hit, r) in [
            (Hit::Close, self.close_rect()),
            (Hit::Clear, self.clear_rect()),
            (Hit::Import, self.import_rect()),
            (Hit::Export, self.export_rect()),
        ] {
            if r.contains(pos) {
                return Some(hit);
            }
        }
        if !self.list_area().contains(pos) {
            return None;
        }
        for i in 0..self.rows.len() {
            let Some(row) = self.row_rect(i) else {
                continue;
            };
            if !row.contains(pos) {
                continue;
            }
            let c = self.columns(row);
            let at = |x: (f64, f64)| self.cell(row, x).contains(pos);
            return [
                (Hit::Remove(i), c.remove),
                (Hit::Learn(i), c.learn),
                (Hit::Consume(i), c.consume),
                (Hit::Pickup(i), c.pickup),
                (Hit::Invert(i), c.invert),
                (Hit::Mode(i), c.mode),
                (Hit::Max(i), c.max),
                (Hit::Min(i), c.min),
            ]
            .into_iter()
            .find(|(_, x)| at(*x))
            .map(|(h, _)| h);
        }
        None
    }

    // ------------------------------------------------------------------ events

    /// What the button under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let hit = self.hit(pos)?;
        let header = |r: Rect, text: &'static str| Some(Help::new(r, text));
        let (i, text) = match hit {
            Hit::Close => return header(self.close_rect(), "Close the panel"),
            Hit::Clear => return header(self.clear_rect(), "Remove every mapping of the project"),
            Hit::Import => {
                return header(
                    self.import_rect(),
                    "Apply a mapping preset file to this project",
                );
            }
            Hit::Export => {
                return header(self.export_rect(), "Save these mappings as a preset file");
            }
            Hit::Remove(i) => (i, "Remove this mapping"),
            Hit::Learn(i) => (i, "Learn a new source for this control: move a knob or key"),
            Hit::Consume(i) => (i, "Consume: the message is not passed on to other mappings"),
            Hit::Pickup(i) => (
                i,
                "Pickup: the control waits until the source passes its value",
            ),
            Hit::Invert(i) => (i, "Invert the direction of the control"),
            Hit::Mode(i) => (i, "Mode: click for the next, right-click for the previous"),
            Hit::Max(i) => (
                i,
                "Highest value the control reaches: click to raise, right-click to lower",
            ),
            Hit::Min(i) => (
                i,
                "Lowest value the control reaches: click to raise, right-click to lower",
            ),
        };
        let row = self.row_rect(i)?;
        let c = self.columns(row);
        let x = match hit {
            Hit::Remove(_) => c.remove,
            Hit::Learn(_) => c.learn,
            Hit::Consume(_) => c.consume,
            Hit::Pickup(_) => c.pickup,
            Hit::Invert(_) => c.invert,
            Hit::Mode(_) => c.mode,
            Hit::Max(_) => c.max,
            _ => c.min,
        };
        Some(Help::new(self.cell(row, x), text))
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerDown { pos, button, .. } => {
                self.repaint = true;
                if let Some(hit) = self.hit(*pos)
                    && let Some(action) = self.press(hit, *button)
                {
                    out.push(UiAction::MidiMaps(action));
                }
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => {
                self.repaint = true;
                match self.hit(*pos) {
                    Some(Hit::Min(i) | Hit::Max(i)) if delta.y != 0.0 => {
                        let max = matches!(self.hit(*pos), Some(Hit::Max(_)));
                        let step = if modifiers.shift {
                            RANGE_STEP_FINE
                        } else {
                            RANGE_STEP
                        };
                        let up = delta.y > 0.0;
                        if let Some(edit) = self.nudge_range(i, max, if up { step } else { -step })
                        {
                            out.push(UiAction::MidiMaps(MidiMapsAction::Edit(edit)));
                        }
                    }
                    _ => {
                        self.scroll -= delta.y * ROW_H;
                        self.clamp_scroll();
                    }
                }
            }
            _ => {}
        }
        out
    }

    fn nudge_range(&self, i: usize, max: bool, by: f64) -> Option<MidiMapping> {
        let mut m = self.rows.get(i)?.mapping.clone();
        let field = if max {
            &mut m.options.max
        } else {
            &mut m.options.min
        };
        let moved = (f64::from(*field) + by).clamp(0.0, 1.0);
        // Round to whole percent so the cells read cleanly.
        *field = ((moved * 100.0).round() / 100.0) as f32;
        (m != self.rows[i].mapping).then_some(m)
    }

    fn press(&self, hit: Hit, button: PointerButton) -> Option<MidiMapsAction> {
        let forward = button != PointerButton::Secondary;
        let edit = |i: usize, f: &dyn Fn(&mut MidiMapping)| {
            let mut m = self.rows.get(i)?.mapping.clone();
            f(&mut m);
            Some(MidiMapsAction::Edit(m))
        };
        match hit {
            Hit::Close => Some(MidiMapsAction::Close),
            Hit::Export => Some(MidiMapsAction::Export),
            Hit::Import => Some(MidiMapsAction::Import),
            Hit::Clear => Some(MidiMapsAction::Clear),
            Hit::Remove(i) => self
                .rows
                .get(i)
                .map(|r| MidiMapsAction::Remove(r.mapping.id)),
            Hit::Learn(i) => self
                .rows
                .get(i)
                .map(|r| MidiMapsAction::Relearn(r.mapping.id)),
            Hit::Mode(i) => edit(i, &|m| {
                m.options.mode = cycle_mode(m.options.mode, m.source.is_cc(), forward);
            }),
            Hit::Invert(i) => edit(i, &|m| m.options.invert = !m.options.invert),
            Hit::Pickup(i) => edit(i, &|m| m.options.pickup = !m.options.pickup),
            Hit::Consume(i) => edit(i, &|m| m.options.consume = !m.options.consume),
            Hit::Min(i) | Hit::Max(i) => {
                let max = matches!(hit, Hit::Max(_));
                let by = if forward { RANGE_STEP } else { -RANGE_STEP };
                self.nudge_range(i, max, by).map(MidiMapsAction::Edit)
            }
        }
    }

    // ------------------------------------------------------------------ painting

    /// Paints the panel (scaled by the scale factor).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let hovered = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&p.inflate(1.0, 1.0).into());
        sp::rect(c, p, st.background);
        sp::rect(c, Rect::new(p.x0, p.y0, p.x1, p.y0 + HEADER_H), st.header);
        sp::line(c, p.x0, p.y0 + 0.5, p.x1, p.y0 + 0.5, t.highlight);
        sp::line(c, p.x0, p.y1 - 0.5, p.x1, p.y1 - 0.5, st.separator);
        sp::line(c, p.x0 + 0.5, p.y0, p.x0 + 0.5, p.y1, st.separator);
        sp::line(c, p.x1 - 0.5, p.y0, p.x1 - 0.5, p.y1, st.separator);
        sp::label(
            c,
            "MIDI MAPPINGS",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        for (r, text) in [
            (self.export_rect(), "EXPORT"),
            (self.import_rect(), "IMPORT"),
            (self.clear_rect(), "CLEAR"),
            (self.close_rect(), "CLOSE"),
        ] {
            sp::button(c, st, r, text, None, hovered(r));
        }
        let count = self.rows.len();
        let width = p.width() - 2.0 * PAD;
        if count == 0 {
            sp::label(
                c,
                &fit_label(
                    "No mappings yet. Turn on MIDI Learn, click a control, then move a knob or a key.",
                    width,
                    10.0,
                ),
                p.x0 + PAD,
                p.y0 + HEADER_H + 24.0,
                10.0,
                t.text_muted,
                Align::Start,
            );
            c.pop_clip();
            c.pop_transform();
            return;
        }
        let plural = if count == 1 { "" } else { "s" };
        let hint = [
            format!("{count} mapping{plural}. Click a mode or a range to change it (right click goes back, the wheel moves a range), LEARN picks another control."),
            "A mapped control is consumed: it plays no notes. PICKUP waits for the control to reach the value; INVERT swaps the ends.".to_owned(),
        ];
        for (n, line) in hint.iter().enumerate() {
            sp::label(
                c,
                &fit_label(line, width, 9.5),
                p.x0 + PAD,
                p.y0 + HEADER_H + 14.0 + n as f64 * 13.0,
                9.5,
                t.text_muted,
                Align::Start,
            );
        }
        let head = Rect::new(
            p.x0,
            p.y0 + HEADER_H + HINT_H,
            p.x1,
            p.y0 + HEADER_H + HINT_H + COLUMNS_H,
        );
        sp::rect(c, head, st.header);
        let hc = self.columns(head);
        let hy = head.y0 + 14.0;
        for (text, x, align) in [
            ("SOURCE", hc.source.0, Align::Start),
            ("TARGET", hc.target.0, Align::Start),
            ("MIN", (hc.min.0 + hc.min.1) / 2.0, Align::Center),
            ("MAX", (hc.max.0 + hc.max.1) / 2.0, Align::Center),
            ("MODE", (hc.mode.0 + hc.mode.1) / 2.0, Align::Center),
            ("INV", (hc.invert.0 + hc.invert.1) / 2.0, Align::Center),
            ("PICK", (hc.pickup.0 + hc.pickup.1) / 2.0, Align::Center),
            ("CONS", (hc.consume.0 + hc.consume.1) / 2.0, Align::Center),
        ] {
            sp::label(c, text, x, hy, 8.5, t.text_muted, align);
        }
        sp::line(c, p.x0, head.y1 - 0.5, p.x1, head.y1 - 0.5, st.separator);
        let area = self.list_area();
        c.push_clip(&area.into());
        for (i, r) in self.rows.iter().enumerate() {
            let y = area.y0 + i as f64 * ROW_H - self.scroll;
            if y + ROW_H < area.y0 || y > area.y1 {
                continue;
            }
            let row = Rect::new(area.x0, y, area.x1, y + ROW_H);
            if i % 2 == 1 {
                sp::rect(c, row, st.lane_alt);
            }
            let col = self.columns(row);
            let m = &r.mapping;
            let text_y = row.y0 + 17.0;
            sp::label(
                c,
                &fit_label(&m.source.describe(), col.source.1 - col.source.0, 10.5),
                col.source.0,
                text_y,
                10.5,
                t.text,
                Align::Start,
            );
            let (target, colour) = if r.missing {
                (format!("{} (missing)", r.target), t.warning)
            } else {
                (r.target.clone(), t.text)
            };
            sp::label(
                c,
                &fit_label(&target, col.target.1 - col.target.0, 10.5),
                col.target.0,
                text_y,
                10.5,
                colour,
                Align::Start,
            );
            let cell = |x: (f64, f64)| self.cell(row, x);
            for (x, text) in [
                (col.min, percent(m.options.min)),
                (col.max, percent(m.options.max)),
            ] {
                let r = cell(x);
                sp::button(c, st, r, &text, None, hovered(r));
            }
            let r = cell(col.mode);
            sp::button(c, st, r, mode_text(m.options.mode), None, hovered(r));
            for (x, text, on) in [
                (col.invert, "INV", m.options.invert),
                (col.pickup, "PICK", m.options.pickup),
                (col.consume, "CONS", m.options.consume),
            ] {
                let r = cell(x);
                sp::button(c, st, r, text, on.then_some(t.highlight), hovered(r));
            }
            let r = cell(col.learn);
            sp::button(c, st, r, "LEARN", None, hovered(r));
            let r = cell(col.remove);
            sp::button(c, st, r, "x", None, hovered(r));
        }
        c.pop_clip();
        c.pop_clip();
        c.pop_transform();
    }
}

/// The x extents (left, right) of the columns of a row.
struct Columns {
    source: (f64, f64),
    target: (f64, f64),
    min: (f64, f64),
    max: (f64, f64),
    mode: (f64, f64),
    invert: (f64, f64),
    pickup: (f64, f64),
    consume: (f64, f64),
    learn: (f64, f64),
    remove: (f64, f64),
}

impl MidiMapsView {
    /// The centre of one of the controls of row `i`, for tests and the headless driver.
    pub fn control_center(&self, i: usize, what: &str) -> Option<Point> {
        let row = self.row_rect(i)?;
        let c = self.columns(row);
        let x = match what {
            "min" => c.min,
            "max" => c.max,
            "mode" => c.mode,
            "invert" => c.invert,
            "pickup" => c.pickup,
            "consume" => c.consume,
            "learn" => c.learn,
            "remove" => c.remove,
            _ => return None,
        };
        Some(self.cell(row, x).center())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Modifiers;
    use crate::{DisplayList, Theme};
    use atomik_model::{DeviceKey, MapOptions, MidiSource, MidiTarget, SourceKind};
    use vello::kurbo::Vec2;

    fn mapping(id: u32, source: SourceKind, mode: MapMode) -> MidiMapping {
        MidiMapping {
            id,
            source: MidiSource::any_port(Some(0), source),
            target: MidiTarget::Param {
                device: DeviceKey("ch".into()),
                param: "fader".into(),
            },
            options: MapOptions {
                mode,
                ..MapOptions::default()
            },
        }
    }

    fn view(rows: Vec<MidiMapRow>) -> MidiMapsView {
        let mut v = MidiMapsView::new(Theme::dark());
        v.set_bounds(Rect::new(0.0, 0.0, 1100.0, 700.0));
        v.set_rows(rows);
        v
    }

    fn row(id: u32, source: SourceKind, mode: MapMode, missing: bool) -> MidiMapRow {
        MidiMapRow {
            mapping: mapping(id, source, mode),
            target: format!("ch fader {id}"),
            missing,
        }
    }

    fn click(v: &mut MidiMapsView, p: Point, button: PointerButton) -> Vec<MidiMapsAction> {
        v.handle(&UiEvent::PointerDown {
            pos: p,
            button,
            modifiers: Modifiers::NONE,
            clicks: 1,
        })
        .into_iter()
        .filter_map(|a| match a {
            UiAction::MidiMaps(a) => Some(a),
            _ => None,
        })
        .collect()
    }

    fn one_edit(v: &mut MidiMapsView, p: Point, button: PointerButton) -> MidiMapping {
        match click(v, p, button).as_slice() {
            [MidiMapsAction::Edit(m)] => m.clone(),
            other => panic!("one edit expected, got {other:?}"),
        }
    }

    fn two() -> MidiMapsView {
        view(vec![
            row(1, SourceKind::Cc(7), MapMode::Absolute, false),
            row(2, SourceKind::Note(36), MapMode::Toggle, true),
        ])
    }

    #[test]
    fn the_mode_cycles_and_skips_encoders_for_a_button() {
        use RelativeKind::*;
        let mut m = MapMode::Absolute;
        let mut seen = vec![m];
        for _ in 0..5 {
            m = cycle_mode(m, true, true);
            seen.push(m);
        }
        assert_eq!(
            seen,
            [
                MapMode::Absolute,
                MapMode::Relative(TwosComplement),
                MapMode::Relative(BinaryOffset),
                MapMode::Relative(SignMagnitude),
                MapMode::Toggle,
                MapMode::Momentary
            ]
        );
        assert_eq!(
            cycle_mode(MapMode::Momentary, true, true),
            MapMode::Absolute
        );
        assert_eq!(
            cycle_mode(MapMode::Absolute, true, false),
            MapMode::Momentary
        );
        // A source that is not a 7 bit controller has no relative modes.
        assert_eq!(cycle_mode(MapMode::Absolute, false, true), MapMode::Toggle);
        assert_eq!(cycle_mode(MapMode::Toggle, false, true), MapMode::Momentary);
        assert_eq!(
            cycle_mode(MapMode::Absolute, false, false),
            MapMode::Momentary
        );
    }

    #[test]
    fn clicks_edit_the_mapping_and_name_it_by_id() {
        let mut v = two();
        let p = v.control_center(0, "mode").unwrap();
        let m = one_edit(&mut v, p, PointerButton::Primary);
        assert_eq!(m.id, 1);
        assert_eq!(
            m.options.mode,
            MapMode::Relative(RelativeKind::TwosComplement)
        );
        let p = v.control_center(0, "invert").unwrap();
        let m = one_edit(&mut v, p, PointerButton::Primary);
        assert!(m.options.invert);
        let p = v.control_center(0, "pickup").unwrap();
        let m = one_edit(&mut v, p, PointerButton::Primary);
        assert!(!m.options.pickup, "pickup starts on");
        let p = v.control_center(1, "consume").unwrap();
        let m = one_edit(&mut v, p, PointerButton::Primary);
        assert_eq!(m.id, 2);
        assert!(!m.options.consume);
        // A button's mode goes to momentary and never to an encoder.
        let p = v.control_center(1, "mode").unwrap();
        let m = one_edit(&mut v, p, PointerButton::Primary);
        assert_eq!(m.options.mode, MapMode::Momentary);
        let m = one_edit(&mut v, p, PointerButton::Secondary);
        assert_eq!(m.options.mode, MapMode::Absolute, "a right click goes back");
    }

    #[test]
    fn the_range_moves_by_clicks_and_the_wheel_and_stays_in_bounds() {
        let mut v = two();
        let p = v.control_center(0, "max").unwrap();
        // The max is at the top: a click up changes nothing, a right click lowers it by 5 %.
        assert!(click(&mut v, p, PointerButton::Primary).is_empty());
        let m = one_edit(&mut v, p, PointerButton::Secondary);
        assert!((m.options.max - 0.95).abs() < 1e-6);
        let p = v.control_center(0, "min").unwrap();
        let m = one_edit(&mut v, p, PointerButton::Primary);
        assert!((m.options.min - 0.05).abs() < 1e-6);
        let wheel = |v: &mut MidiMapsView, dy: f64, shift: bool| {
            v.handle(&UiEvent::Wheel {
                delta: Vec2::new(0.0, dy),
                pos: p,
                modifiers: Modifiers {
                    shift,
                    ..Modifiers::NONE
                },
            })
        };
        let out = wheel(&mut v, 1.0, false);
        assert!(
            matches!(&out[..], [UiAction::MidiMaps(MidiMapsAction::Edit(m))] if (m.options.min - 0.05).abs() < 1e-6)
        );
        let out = wheel(&mut v, 1.0, true);
        assert!(
            matches!(&out[..], [UiAction::MidiMaps(MidiMapsAction::Edit(m))] if (m.options.min - 0.01).abs() < 1e-6)
        );
        // Down from the bottom is nothing.
        assert!(wheel(&mut v, -1.0, false).is_empty());
    }

    #[test]
    fn learn_remove_and_the_header_buttons_are_requests() {
        let mut v = two();
        let p = v.control_center(1, "learn").unwrap();
        assert_eq!(
            click(&mut v, p, PointerButton::Primary),
            [MidiMapsAction::Relearn(2)]
        );
        let p = v.control_center(0, "remove").unwrap();
        assert_eq!(
            click(&mut v, p, PointerButton::Primary),
            [MidiMapsAction::Remove(1)]
        );
        for (r, want) in [
            (v.export_rect(), MidiMapsAction::Export),
            (v.import_rect(), MidiMapsAction::Import),
            (v.clear_rect(), MidiMapsAction::Clear),
            (v.close_rect(), MidiMapsAction::Close),
        ] {
            assert_eq!(click(&mut v, r.center(), PointerButton::Primary), [want]);
        }
        // Outside every control nothing happens.
        let panel = v.panel();
        assert!(
            click(
                &mut v,
                Point::new(panel.x0 + 3.0, panel.y1 - 3.0),
                PointerButton::Primary
            )
            .is_empty()
        );
    }

    #[test]
    fn the_rows_never_overlap_the_panel_edges_and_scroll_when_there_are_many() {
        let rows: Vec<MidiMapRow> = (1..=60)
            .map(|i| row(i, SourceKind::Cc(7), MapMode::Absolute, false))
            .collect();
        let mut v = view(rows);
        let p = v.panel();
        assert!(p.y1 <= 700.0 - MARGIN + 0.5);
        let top = v.row_rect(0).unwrap();
        assert!(top.y0 >= v.list_area().y0 - 0.5);
        let before = v.scroll();
        v.handle(&UiEvent::Wheel {
            delta: Vec2::new(0.0, -5.0),
            pos: Point::new(p.x0 + 5.0, p.y0 + 150.0),
            modifiers: Modifiers::NONE,
        });
        assert!(v.scroll() > before);
        assert!(v.row_rect(0).is_none(), "scrolled out");
        let c = v.control_center(10, "learn");
        assert!(c.is_some() || v.row_rect(10).is_none());
        for i in 0..60 {
            if let Some(r) = v.row_rect(i) {
                assert!(r.y0 >= v.list_area().y0 - 0.5 && r.y1 <= v.list_area().y1 + 0.5);
            }
        }
    }

    #[test]
    fn painting_shows_the_sources_targets_and_marks_a_missing_target() {
        let v = two();
        let mut list = DisplayList::new();
        v.paint(&mut list);
        let texts = list.texts();
        for t in [
            "MIDI MAPPINGS",
            "EXPORT",
            "CLOSE",
            "CC 7 ch 1",
            "Note C1 ch 1",
            "ch fader 1",
            "ch fader 2 (missing)",
            "Absolute",
            "Toggle",
            "0%",
            "100%",
            "LEARN",
        ] {
            assert!(texts.contains(&t), "{t} in {texts:?}");
        }
        let mut empty = MidiMapsView::new(Theme::dark());
        empty.set_rows(Vec::new());
        let mut list = DisplayList::new();
        empty.paint(&mut list);
        assert!(
            list.texts()
                .iter()
                .any(|t| t.starts_with("No mappings yet"))
        );
    }

    #[test]
    fn identical_rows_do_not_repaint() {
        let mut v = two();
        let mut list = DisplayList::new();
        v.paint(&mut list);
        v.repaint = false;
        v.set_rows(v.rows().to_vec());
        assert!(!v.needs_repaint());
        v.set_rows(Vec::new());
        assert!(v.needs_repaint());
    }
}
