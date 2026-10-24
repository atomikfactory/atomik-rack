//! The quantize panel: grid, strength, swing, groove, humanize and the groove library.
//!
//! ```text
//! QUANTIZE                                                               [ CLOSE ]
//! Applies to: 3 clips
//! Grid             [-]  1/16          [+]
//! Strength         [-]  100 %         [+]
//! Swing            [-]  50 %          [+]
//! Groove           [-]  Swing 66%     [+]
//! Groove velocity  [-]  100 %         [+]
//! Humanize timing  [-]  0 %           [+]
//! Humanize velocity[-]  0 %           [+]
//! Seed             [-]  0             [+]
//! Snap note ends   [-]  OFF           [+]
//! [ APPLY ]
//! GROOVES IN THIS SONG
//!   Late 2nd           1/16  2 slots                                  [ DELETE ]
//! Measure with        [-]  1/16          [+]
//! Slots               [-]  2             [+]
//! [ EXTRACT GROOVE FROM CLIP ]
//! ```
//!
//! A modal view the app places over the window like the relink panel. It has no document: the
//! app tells it what a click on APPLY would work on ([`QuantizeView::set_target`]) and the
//! grooves it can pick ([`QuantizeView::set_grooves`]), and reads the settings back
//! ([`QuantizeView::params`], [`QuantizeView::values`]) when [`UiAction::Quantize`] arrives:
//!
//! * `APPLY` emits [`QuantizeAction::Apply`] (not while there is nothing to apply it to).
//! * `EXTRACT GROOVE FROM CLIP` emits [`QuantizeAction::Extract`] (not without a clip to measure).
//! * `DELETE` on a groove row emits [`QuantizeAction::Delete`].
//! * `CLOSE` emits [`QuantizeAction::Close`].
//!
//! Every value has a `-` and a `+` button and answers the wheel. The settings live in
//! [`QuantizeValues`], which the app keeps between openings. The groove row cycles through
//! "None", the built-in grooves and the song's; a groove sets the grid, so the grid row is dimmed
//! while one is chosen.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::Theme;
use atomik_core::PPQ;
use atomik_model::groove::{MAX_GROOVE_SLOTS, SWING_MAX, SWING_MIN};
use atomik_model::{QuantizeParams, StepResolution};
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const TARGET_H: f64 = 24.0;
const ROW_H: f64 = 26.0;
const APPLY_H: f64 = 40.0;
const LIB_HEAD_H: f64 = 24.0;
const LIB_ROW_H: f64 = 22.0;
const LIB_ROWS: usize = 4;
const EXTRACT_H: f64 = 42.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 460.0;
const BTN_H: f64 = 20.0;
const STEP_BTN_W: f64 = 24.0;
const VALUE_W: f64 = 150.0;
/// The seeds the panel offers.
const SEED_MAX: u32 = 999;
/// Fewest slots an extracted groove gets.
const SLOTS_MIN: usize = 1;

/// The grids of the grid row: a label and the length in ticks.
pub const GRIDS: [(&str, i64); 8] = [
    ("1/4", PPQ),
    ("1/8", PPQ / 2),
    ("1/8 triplet", PPQ / 3),
    ("1/16", PPQ / 4),
    ("1/16 triplet", PPQ / 6),
    ("1/32", PPQ / 8),
    ("1/32 triplet", PPQ / 12),
    ("1 bar", 4 * PPQ),
];

/// The grid row's choice a quantize starts with: 1/16.
const DEFAULT_GRID: usize = 3;

/// The step the extract row measures against.
const EXTRACT_STEPS: [StepResolution; 3] = [
    StepResolution::Eighth,
    StepResolution::Sixteenth,
    StepResolution::ThirtySecond,
];

/// The text of a step resolution.
pub fn step_label(step: StepResolution) -> &'static str {
    match step {
        StepResolution::Eighth => "1/8",
        StepResolution::Sixteenth => "1/16",
        StepResolution::ThirtySecond => "1/32",
    }
}

/// The settings of the panel; the app keeps them between openings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuantizeValues {
    /// Index into [`GRIDS`].
    pub grid: usize,
    /// Strength in percent, 0..=100.
    pub strength: u8,
    /// Swing in percent, 50..=75.
    pub swing: u8,
    /// The groove by name (`None`: no groove).
    pub groove: Option<String>,
    /// How much of the groove's velocity applies, percent.
    pub groove_velocity: u8,
    /// Humanize timing, percent.
    pub humanize_time: u8,
    /// Humanize velocity, percent.
    pub humanize_velocity: u8,
    /// The humanize seed, 0..=999.
    pub seed: u32,
    /// Also snap note ends.
    pub snap_ends: bool,
    /// The step an extracted groove is measured against.
    pub extract_step: StepResolution,
    /// The slots of an extracted groove.
    pub extract_slots: usize,
}

impl Default for QuantizeValues {
    fn default() -> Self {
        Self {
            grid: DEFAULT_GRID,
            strength: 100,
            swing: SWING_MIN,
            groove: None,
            groove_velocity: 100,
            humanize_time: 0,
            humanize_velocity: 0,
            seed: 0,
            snap_ends: false,
            extract_step: StepResolution::Sixteenth,
            extract_slots: 2,
        }
    }
}

impl QuantizeValues {
    /// The model's parameters for these settings.
    pub fn params(&self) -> QuantizeParams {
        let pct = |v: u8| f32::from(v) / 100.0;
        QuantizeParams {
            grid: GRIDS[self.grid.min(GRIDS.len() - 1)].1,
            strength: pct(self.strength),
            snap_ends: self.snap_ends,
            swing: self.swing,
            groove: self.groove.clone(),
            groove_velocity: pct(self.groove_velocity),
            humanize_time: pct(self.humanize_time),
            humanize_velocity: pct(self.humanize_velocity),
            seed: self.seed,
        }
    }
}

/// One groove the picker offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrooveEntry {
    /// The groove's name.
    pub name: String,
    /// Its step.
    pub step: StepResolution,
    /// Its slot count.
    pub slots: usize,
    /// A built-in groove (listed in the picker, not in the song's library, never deleted).
    pub builtin: bool,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuantizeAction {
    /// Quantize the target with the panel's settings.
    Apply,
    /// Measure a groove from the clip and add it to the song.
    Extract,
    /// Remove the song's groove of this name.
    Delete(String),
    /// Close the panel.
    Close,
}

/// A row of values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantizeRow {
    /// The grid.
    Grid,
    /// How far notes move.
    Strength,
    /// Swing.
    Swing,
    /// The groove template.
    Groove,
    /// How much of the groove's velocity applies.
    GrooveVelocity,
    /// Random timing.
    HumanizeTime,
    /// Random velocity.
    HumanizeVelocity,
    /// Humanize seed.
    Seed,
    /// Also snap the ends of notes.
    SnapEnds,
    /// The step an extracted groove is measured against.
    ExtractStep,
    /// The slots of an extracted groove.
    ExtractSlots,
}

impl QuantizeRow {
    /// The rows above the APPLY button.
    const TOP: [QuantizeRow; 9] = [
        QuantizeRow::Grid,
        QuantizeRow::Strength,
        QuantizeRow::Swing,
        QuantizeRow::Groove,
        QuantizeRow::GrooveVelocity,
        QuantizeRow::HumanizeTime,
        QuantizeRow::HumanizeVelocity,
        QuantizeRow::Seed,
        QuantizeRow::SnapEnds,
    ];

    fn title(self) -> &'static str {
        match self {
            QuantizeRow::Grid => "Grid",
            QuantizeRow::Strength => "Strength",
            QuantizeRow::Swing => "Swing",
            QuantizeRow::Groove => "Groove",
            QuantizeRow::GrooveVelocity => "Groove velocity",
            QuantizeRow::HumanizeTime => "Humanize timing",
            QuantizeRow::HumanizeVelocity => "Humanize velocity",
            QuantizeRow::Seed => "Seed",
            QuantizeRow::SnapEnds => "Snap note ends",
            QuantizeRow::ExtractStep => "Measure against",
            QuantizeRow::ExtractSlots => "Slots",
        }
    }
}

/// The quantize panel. See the module documentation.
#[derive(Debug)]
pub struct QuantizeView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    values: QuantizeValues,
    grooves: Vec<GrooveEntry>,
    target: String,
    can_apply: bool,
    can_extract: bool,
    scroll: usize,
    hover: Option<Point>,
    repaint: bool,
}

impl QuantizeView {
    /// A panel with these settings; call [`Self::set_bounds`], [`Self::set_target`] and
    /// [`Self::set_grooves`].
    pub fn new(theme: Theme, values: QuantizeValues) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            values,
            grooves: Vec::new(),
            target: String::new(),
            can_apply: false,
            can_extract: false,
            scroll: 0,
            hover: None,
            repaint: true,
        }
    }

    /// Places the panel: the area it may use (logical pixels). The panel is centred in it.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Says what APPLY would work on (`text`, shown under the title) and whether it can, and
    /// whether there is a clip to extract a groove from.
    pub fn set_target(&mut self, text: String, can_apply: bool, can_extract: bool) {
        if (&text, can_apply, can_extract) != (&self.target, self.can_apply, self.can_extract) {
            self.target = text;
            self.can_apply = can_apply;
            self.can_extract = can_extract;
            self.repaint = true;
        }
    }

    /// Replaces the grooves the picker offers (built-in ones first, then the song's). A chosen
    /// groove that is gone is dropped from the settings.
    pub fn set_grooves(&mut self, grooves: Vec<GrooveEntry>) {
        if grooves == self.grooves {
            return;
        }
        self.grooves = grooves;
        if let Some(name) = &self.values.groove
            && !self.grooves.iter().any(|g| &g.name == name)
        {
            self.values.groove = None;
        }
        self.clamp_scroll();
        self.repaint = true;
    }

    /// Chooses the groove `name` (ignored when the panel does not offer it).
    pub fn select_groove(&mut self, name: &str) {
        if self.grooves.iter().any(|g| g.name == name) {
            self.values.groove = Some(name.to_owned());
            self.repaint = true;
        }
    }

    /// The settings.
    pub fn values(&self) -> &QuantizeValues {
        &self.values
    }

    /// The model's parameters for the settings.
    pub fn params(&self) -> QuantizeParams {
        self.values.params()
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    // ------------------------------------------------------------------ values

    /// The text of a row's value.
    pub fn value_text(&self, row: QuantizeRow) -> String {
        let v = &self.values;
        match row {
            QuantizeRow::Grid => match self.chosen_groove() {
                Some(g) => format!("{} (groove)", step_label(g.step)),
                None => GRIDS[v.grid.min(GRIDS.len() - 1)].0.to_owned(),
            },
            QuantizeRow::Strength => format!("{} %", v.strength),
            QuantizeRow::Swing => format!("{} %", v.swing),
            QuantizeRow::Groove => v.groove.clone().unwrap_or_else(|| "None".to_owned()),
            QuantizeRow::GrooveVelocity => format!("{} %", v.groove_velocity),
            QuantizeRow::HumanizeTime => format!("{} %", v.humanize_time),
            QuantizeRow::HumanizeVelocity => format!("{} %", v.humanize_velocity),
            QuantizeRow::Seed => v.seed.to_string(),
            QuantizeRow::SnapEnds => if v.snap_ends { "ON" } else { "OFF" }.to_owned(),
            QuantizeRow::ExtractStep => step_label(v.extract_step).to_owned(),
            QuantizeRow::ExtractSlots => v.extract_slots.to_string(),
        }
    }

    fn chosen_groove(&self) -> Option<&GrooveEntry> {
        let name = self.values.groove.as_ref()?;
        self.grooves.iter().find(|g| &g.name == name)
    }

    /// Whether a row answers clicks (the grid does not while a groove sets it; the groove's
    /// velocity does not without a groove).
    pub fn row_enabled(&self, row: QuantizeRow) -> bool {
        match row {
            QuantizeRow::Grid => self.values.groove.is_none(),
            QuantizeRow::GrooveVelocity => self.values.groove.is_some(),
            _ => true,
        }
    }

    /// Steps a row by `dir` (+1 or -1): a click on `+` or `-`, or a wheel notch.
    pub fn step(&mut self, row: QuantizeRow, dir: i32) {
        if !self.row_enabled(row) {
            return;
        }
        let up =
            |v: u8, step: i32, lo: i32, hi: i32| (i32::from(v) + dir * step).clamp(lo, hi) as u8;
        let v = &mut self.values;
        match row {
            QuantizeRow::Grid => {
                v.grid = (v.grid as i32 + dir).clamp(0, GRIDS.len() as i32 - 1) as usize;
            }
            QuantizeRow::Strength => v.strength = up(v.strength, 5, 0, 100),
            QuantizeRow::Swing => {
                v.swing = up(v.swing, 1, i32::from(SWING_MIN), i32::from(SWING_MAX));
            }
            QuantizeRow::Groove => {
                // None, then the grooves in order, wrapping around.
                let n = self.grooves.len() as i32 + 1;
                let at = v
                    .groove
                    .as_ref()
                    .and_then(|name| self.grooves.iter().position(|g| &g.name == name))
                    .map_or(0, |i| i as i32 + 1);
                let next = (at + dir).rem_euclid(n);
                v.groove = (next > 0).then(|| self.grooves[next as usize - 1].name.clone());
            }
            QuantizeRow::GrooveVelocity => v.groove_velocity = up(v.groove_velocity, 5, 0, 100),
            QuantizeRow::HumanizeTime => v.humanize_time = up(v.humanize_time, 5, 0, 100),
            QuantizeRow::HumanizeVelocity => {
                v.humanize_velocity = up(v.humanize_velocity, 5, 0, 100)
            }
            QuantizeRow::Seed => {
                v.seed = (i64::from(v.seed) + i64::from(dir)).clamp(0, i64::from(SEED_MAX)) as u32;
            }
            QuantizeRow::SnapEnds => v.snap_ends = !v.snap_ends,
            QuantizeRow::ExtractStep => {
                let at = EXTRACT_STEPS
                    .iter()
                    .position(|s| *s == v.extract_step)
                    .unwrap_or(1) as i32;
                v.extract_step = EXTRACT_STEPS[(at + dir).clamp(0, 2) as usize];
            }
            QuantizeRow::ExtractSlots => {
                v.extract_slots = (v.extract_slots as i32 + dir)
                    .clamp(SLOTS_MIN as i32, MAX_GROOVE_SLOTS as i32)
                    as usize;
            }
        }
        self.repaint = true;
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + Self::height().min(room))
    }

    fn height() -> f64 {
        HEADER_H
            + TARGET_H
            + QuantizeRow::TOP.len() as f64 * ROW_H
            + APPLY_H
            + LIB_HEAD_H
            + LIB_ROWS as f64 * LIB_ROW_H
            + 2.0 * ROW_H
            + EXTRACT_H
            + PAD
    }

    fn rows_top(&self) -> f64 {
        self.panel().y0 + HEADER_H + TARGET_H
    }

    /// The row of a value.
    pub fn row_rect(&self, row: QuantizeRow) -> Rect {
        let p = self.panel();
        let y = match row {
            QuantizeRow::ExtractStep => self.extract_top(),
            QuantizeRow::ExtractSlots => self.extract_top() + ROW_H,
            _ => {
                let i = QuantizeRow::TOP.iter().position(|r| *r == row).unwrap_or(0);
                self.rows_top() + i as f64 * ROW_H
            }
        };
        Rect::new(p.x0, y, p.x1, y + ROW_H)
    }

    fn cluster_x(&self) -> f64 {
        self.panel().x0 + PAD + 138.0
    }

    /// The `-` button of a row.
    pub fn minus_rect(&self, row: QuantizeRow) -> Rect {
        let r = self.row_rect(row);
        let x = self.cluster_x();
        let y = r.y0 + (ROW_H - BTN_H) / 2.0;
        Rect::new(x, y, x + STEP_BTN_W, y + BTN_H)
    }

    /// The value window of a row (a click on the snap row's toggles it).
    pub fn value_rect(&self, row: QuantizeRow) -> Rect {
        let m = self.minus_rect(row);
        Rect::new(m.x1 + 3.0, m.y0, m.x1 + 3.0 + VALUE_W, m.y1)
    }

    /// The `+` button of a row.
    pub fn plus_rect(&self, row: QuantizeRow) -> Rect {
        let v = self.value_rect(row);
        Rect::new(v.x1 + 3.0, v.y0, v.x1 + 3.0 + STEP_BTN_W, v.y1)
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 80.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H + 2.0,
        )
    }

    /// The APPLY button.
    pub fn apply_rect(&self) -> Rect {
        let p = self.panel();
        let top = self.rows_top() + QuantizeRow::TOP.len() as f64 * ROW_H;
        let y = top + (APPLY_H - BTN_H - 4.0) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 120.0, y + BTN_H + 4.0)
    }

    fn lib_top(&self) -> f64 {
        self.rows_top() + QuantizeRow::TOP.len() as f64 * ROW_H + APPLY_H
    }

    fn extract_top(&self) -> f64 {
        self.lib_top() + LIB_HEAD_H + LIB_ROWS as f64 * LIB_ROW_H
    }

    /// The song's grooves, in library order (the picker also lists the built-in ones).
    pub fn library(&self) -> Vec<&GrooveEntry> {
        self.grooves.iter().filter(|g| !g.builtin).collect()
    }

    fn clamp_scroll(&mut self) {
        let max = self.library().len().saturating_sub(LIB_ROWS);
        self.scroll = self.scroll.min(max);
    }

    /// The row of the `i`th library groove, `None` when it is scrolled out of the list.
    pub fn library_row_rect(&self, i: usize) -> Option<Rect> {
        let p = self.panel();
        let k = i.checked_sub(self.scroll)?;
        if k >= LIB_ROWS || i >= self.library().len() {
            return None;
        }
        let y = self.lib_top() + LIB_HEAD_H + k as f64 * LIB_ROW_H;
        Some(Rect::new(p.x0, y, p.x1, y + LIB_ROW_H))
    }

    /// The DELETE button of the `i`th library groove.
    pub fn delete_rect(&self, i: usize) -> Option<Rect> {
        let row = self.library_row_rect(i)?;
        let y = row.y0 + (LIB_ROW_H - BTN_H) / 2.0;
        Some(Rect::new(row.x1 - PAD - 80.0, y, row.x1 - PAD, y + BTN_H))
    }

    /// The EXTRACT GROOVE FROM CLIP button.
    pub fn extract_rect(&self) -> Rect {
        let p = self.panel();
        let top = self.extract_top() + 2.0 * ROW_H;
        let y = top + (EXTRACT_H - BTN_H - 4.0) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 230.0, y + BTN_H + 4.0)
    }

    // ------------------------------------------------------------------ events

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
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                self.repaint = true;
                if let Some(action) = self.click(*pos) {
                    out.push(UiAction::Quantize(action));
                }
            }
            UiEvent::Wheel { pos, delta, .. } => {
                self.repaint = true;
                let dir = if delta.y > 0.0 { 1 } else { -1 };
                let rows = QuantizeRow::TOP
                    .into_iter()
                    .chain([QuantizeRow::ExtractStep, QuantizeRow::ExtractSlots]);
                if let Some(row) = rows.into_iter().find(|r| self.row_rect(*r).contains(*pos)) {
                    self.step(row, dir);
                } else if delta.y != 0.0 {
                    let max = self.library().len().saturating_sub(LIB_ROWS);
                    self.scroll =
                        (self.scroll as i64 - i64::from(dir)).clamp(0, max as i64) as usize;
                }
            }
            _ => {}
        }
        out
    }

    fn click(&mut self, pos: Point) -> Option<QuantizeAction> {
        if self.close_rect().contains(pos) {
            return Some(QuantizeAction::Close);
        }
        if self.apply_rect().contains(pos) {
            return self.can_apply.then_some(QuantizeAction::Apply);
        }
        if self.extract_rect().contains(pos) {
            return self.can_extract.then_some(QuantizeAction::Extract);
        }
        let rows = QuantizeRow::TOP
            .into_iter()
            .chain([QuantizeRow::ExtractStep, QuantizeRow::ExtractSlots]);
        for row in rows {
            if self.minus_rect(row).contains(pos) {
                self.step(row, -1);
                return None;
            }
            if self.plus_rect(row).contains(pos)
                || (row == QuantizeRow::SnapEnds && self.value_rect(row).contains(pos))
            {
                self.step(row, 1);
                return None;
            }
        }
        let library = self.library();
        (0..library.len())
            .find(|i| self.delete_rect(*i).is_some_and(|r| r.contains(pos)))
            .map(|i| QuantizeAction::Delete(library[i].name.clone()))
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
            "QUANTIZE",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        let width = p.width() - 2.0 * PAD;
        let (target_text, target_color) = if self.target.is_empty() {
            ("nothing selected", t.text_muted)
        } else {
            (self.target.as_str(), t.text)
        };
        sp::label(
            c,
            &fit_label(&format!("Applies to: {target_text}"), width, 10.0),
            p.x0 + PAD,
            p.y0 + HEADER_H + 15.0,
            10.0,
            target_color,
            Align::Start,
        );
        let rows = QuantizeRow::TOP
            .into_iter()
            .chain([QuantizeRow::ExtractStep, QuantizeRow::ExtractSlots]);
        for row in rows {
            let r = self.row_rect(row);
            let enabled = self.row_enabled(row);
            let ink = if enabled { t.text } else { t.text_muted };
            sp::label(
                c,
                row.title(),
                r.x0 + PAD,
                r.y0 + 17.0,
                10.5,
                ink,
                Align::Start,
            );
            let (minus, value, plus) = (
                self.minus_rect(row),
                self.value_rect(row),
                self.plus_rect(row),
            );
            sp::button(c, st, minus, "-", None, enabled && hovered(minus));
            sp::lcd(c, st, value, None);
            sp::label(
                c,
                &fit_label(&self.value_text(row), VALUE_W - 12.0, 10.5),
                value.x0 + 8.0,
                value.y0 + 14.0,
                10.5,
                if enabled { t.text } else { t.text_muted },
                Align::Start,
            );
            sp::button(c, st, plus, "+", None, enabled && hovered(plus));
        }
        let apply = self.apply_rect();
        sp::button(
            c,
            st,
            apply,
            "APPLY",
            self.can_apply.then_some(st.loop_on),
            self.can_apply && hovered(apply),
        );
        let lib = self.lib_top();
        sp::line(c, p.x0, lib + 0.5, p.x1, lib + 0.5, st.separator);
        sp::label(
            c,
            "GROOVES IN THIS SONG",
            p.x0 + PAD,
            lib + 16.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        let library = self.library();
        if library.is_empty() {
            sp::label(
                c,
                "None yet: extract one from a clip.",
                p.x0 + PAD,
                lib + LIB_HEAD_H + 15.0,
                10.0,
                t.text_muted,
                Align::Start,
            );
        }
        for (i, g) in library.iter().enumerate() {
            let Some(row) = self.library_row_rect(i) else {
                continue;
            };
            if (i - self.scroll) % 2 == 1 {
                sp::rect(c, row, st.lane_alt);
            }
            sp::label(
                c,
                &fit_label(&g.name, width - 230.0, 10.5),
                row.x0 + PAD,
                row.y0 + 15.0,
                10.5,
                t.text,
                Align::Start,
            );
            let slots = if g.slots == 1 { "slot" } else { "slots" };
            sp::label(
                c,
                &format!("{}  {} {slots}", step_label(g.step), g.slots),
                row.x1 - PAD - 90.0,
                row.y0 + 15.0,
                10.0,
                t.text_muted,
                Align::End,
            );
            if let Some(del) = self.delete_rect(i) {
                sp::button(c, st, del, "DELETE", None, hovered(del));
            }
        }
        let extract = self.extract_rect();
        sp::line(
            c,
            p.x0,
            self.extract_top() + 0.5,
            p.x1,
            self.extract_top() + 0.5,
            st.separator,
        );
        sp::button(
            c,
            st,
            extract,
            "EXTRACT GROOVE FROM CLIP",
            None,
            self.can_extract && hovered(extract),
        );
        c.pop_clip();
        c.pop_transform();
    }
}
