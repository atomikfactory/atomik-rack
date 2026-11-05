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
//! Humanize length  [-]  0 %           [+]
//! Seed             [-]  0             [+]
//! Snap note ends   [-]  OFF           [+]
//! Legato           [-]  OFF           [+]
//! Groove name      [ Late 2nd                 ] [ RENAME ]
//! [ APPLY ] [ LIVE ON CLIPS ] [ CLEAR LIVE ]
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
//! * `LIVE ON CLIPS` emits [`QuantizeAction::SetLive`] (the chosen groove, at the strength of the
//!   panel, becomes the live groove of the selected clips: their notes are not edited) and `CLEAR
//!   LIVE` emits [`QuantizeAction::ClearLive`].
//! * The groove name is a [`TextField`]: it shows the name of the chosen groove of the song (a
//!   built-in groove has no name to edit); `RENAME` or Enter emits [`QuantizeAction::Rename`].
//! * `EXTRACT GROOVE FROM CLIP` emits [`QuantizeAction::Extract`] (not without a clip to measure).
//! * `DELETE` on a groove row emits [`QuantizeAction::Delete`].
//! * `CLOSE` emits [`QuantizeAction::Close`].
//!
//! The piano roll's QUANT button and the quantize key emit [`QuantizeAction::ApplyLast`] (apply
//! the last settings without the panel); its `...` button and a right click on QUANT emit
//! [`QuantizeAction::Options`] (open the panel). The QUANT button of the arrangement's corner
//! emits [`QuantizeAction::ApplyClips`] (the same, on the arrangement's selected clips) and
//! opens the panel on a right click.
//!
//! Every value has a `-` and a `+` button and answers the wheel. The settings live in
//! [`QuantizeValues`], which the app keeps between openings. The groove row cycles through
//! "None", the built-in grooves and the song's; a groove sets the grid, so the grid row is dimmed
//! while one is chosen.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::step_drag::{PX_PER_STEP, StepDrag, wheel_steps};
use crate::text_field::{FieldColors, TextField};
use crate::theme::{Theme, mix};
use crate::tooltip::{Help, stepper_help};
use atomik_core::PPQ;
use atomik_model::groove::{MAX_GROOVE_NAME, MAX_GROOVE_SLOTS, SWING_MAX, SWING_MIN};
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
    /// Humanize note lengths, percent.
    pub humanize_length: u8,
    /// Stretch the notes to reach the next one.
    pub legato: bool,
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
            humanize_length: 0,
            legato: false,
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
            humanize_length: pct(self.humanize_length),
            legato: self.legato,
            seed: self.seed,
            ..QuantizeParams::default()
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
    /// Quantize with the settings the panel last had (from the piano roll's QUANT button and
    /// the quantize key, not from the open panel).
    ApplyLast,
    /// Quantize the arrangement's selected clips with the settings the panel last had (from the
    /// arrangement corner's QUANT button).
    ApplyClips,
    /// Open the panel (from the piano roll's options button or a right click on QUANT).
    Options,
    /// Give the selected clips the chosen groove, at the panel's strength, as their live groove.
    SetLive,
    /// Hand the selected clips back to their track's groove.
    ClearLive,
    /// Rename the song's groove `from` to `to`.
    Rename {
        /// The groove's name now.
        from: String,
        /// The name typed in the field.
        to: String,
    },
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
    /// Random note length.
    HumanizeLength,
    /// Humanize seed.
    Seed,
    /// Also snap the ends of notes.
    SnapEnds,
    /// Stretch each note to reach the next.
    Legato,
    /// The step an extracted groove is measured against.
    ExtractStep,
    /// The slots of an extracted groove.
    ExtractSlots,
}

impl QuantizeRow {
    /// The rows above the APPLY button.
    const TOP: [QuantizeRow; 11] = [
        QuantizeRow::Grid,
        QuantizeRow::Strength,
        QuantizeRow::Swing,
        QuantizeRow::Groove,
        QuantizeRow::GrooveVelocity,
        QuantizeRow::HumanizeTime,
        QuantizeRow::HumanizeVelocity,
        QuantizeRow::HumanizeLength,
        QuantizeRow::Seed,
        QuantizeRow::SnapEnds,
        QuantizeRow::Legato,
    ];

    /// A row that is an ON / OFF switch (a click on its value flips it; it is not dragged).
    fn is_toggle(self) -> bool {
        matches!(self, QuantizeRow::SnapEnds | QuantizeRow::Legato)
    }

    fn title(self) -> &'static str {
        match self {
            QuantizeRow::Grid => "Grid",
            QuantizeRow::Strength => "Strength",
            QuantizeRow::Swing => "Swing",
            QuantizeRow::Groove => "Groove",
            QuantizeRow::GrooveVelocity => "Groove velocity",
            QuantizeRow::HumanizeTime => "Humanize timing",
            QuantizeRow::HumanizeVelocity => "Humanize velocity",
            QuantizeRow::HumanizeLength => "Humanize length",
            QuantizeRow::Seed => "Seed",
            QuantizeRow::SnapEnds => "Snap note ends",
            QuantizeRow::Legato => "Legato",
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
    /// A drag on a value: the row and the vertical travel (up steps the value up).
    drag: Option<(QuantizeRow, StepDrag)>,
    /// The groove name being typed.
    name: TextField,
    /// The groove the name field was last filled for.
    name_for: Option<String>,
    /// The name field has the keys.
    name_focus: bool,
    /// A pointer drag is selecting in the name field.
    name_drag: bool,
    repaint: bool,
}

impl QuantizeView {
    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

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
            drag: None,
            name: TextField::new("", MAX_GROOVE_NAME, false),
            name_for: None,
            name_focus: false,
            name_drag: false,
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
        self.sync_name();
        self.repaint = true;
    }

    /// Chooses the groove `name` (ignored when the panel does not offer it).
    pub fn select_groove(&mut self, name: &str) {
        if self.grooves.iter().any(|g| g.name == name) {
            self.values.groove = Some(name.to_owned());
            self.sync_name();
            self.repaint = true;
        }
    }

    /// The chosen groove when it is one of the song's (the only kind that has a name to edit).
    fn renamable(&self) -> Option<&GrooveEntry> {
        self.chosen_groove().filter(|g| !g.builtin)
    }

    /// Fills the name field with the chosen groove's name when the choice changed since the last
    /// time (typing is not overwritten).
    fn sync_name(&mut self) {
        let want = self.renamable().map(|g| g.name.clone());
        if want != self.name_for {
            self.name.set_text(want.as_deref().unwrap_or(""));
            self.name_focus = self.name_focus && want.is_some();
            self.name_for = want;
        }
    }

    /// Whether the name field has the keys (the shell then offers the input method to it).
    pub fn wants_text(&self) -> bool {
        self.name_focus && self.renamable().is_some()
    }

    /// The text of the name field.
    pub fn name_text(&self) -> &str {
        self.name.text()
    }

    /// The rename the field asks for: the chosen groove and the typed name, when they differ.
    fn rename_action(&self) -> Option<QuantizeAction> {
        let from = self.renamable()?.name.clone();
        let to = self.name.text().trim().to_owned();
        (!to.is_empty() && to != from).then_some(QuantizeAction::Rename { from, to })
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
            QuantizeRow::HumanizeLength => format!("{} %", v.humanize_length),
            QuantizeRow::Seed => v.seed.to_string(),
            QuantizeRow::SnapEnds => if v.snap_ends { "ON" } else { "OFF" }.to_owned(),
            QuantizeRow::Legato => if v.legato { "ON" } else { "OFF" }.to_owned(),
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
            QuantizeRow::HumanizeLength => v.humanize_length = up(v.humanize_length, 5, 0, 100),
            QuantizeRow::Seed => {
                v.seed = (i64::from(v.seed) + i64::from(dir)).clamp(0, i64::from(SEED_MAX)) as u32;
            }
            QuantizeRow::SnapEnds => v.snap_ends = !v.snap_ends,
            QuantizeRow::Legato => v.legato = !v.legato,
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
        self.sync_name();
        self.repaint = true;
    }

    /// Steps a row by `n` places (negative: down), one place at a time.
    fn step_by(&mut self, row: QuantizeRow, n: i32) {
        for _ in 0..n.abs() {
            self.step(row, n.signum());
        }
    }

    /// The row whose value box is under `pos`, when dragging it makes sense (not the toggle).
    fn draggable_value_at(&self, pos: Point) -> Option<QuantizeRow> {
        QuantizeRow::TOP
            .into_iter()
            .chain([QuantizeRow::ExtractStep, QuantizeRow::ExtractSlots])
            .find(|r| !r.is_toggle() && self.row_enabled(*r) && self.value_rect(*r).contains(pos))
    }

    /// True while a value is being dragged.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
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
            + (QuantizeRow::TOP.len() + 1) as f64 * ROW_H
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

    /// The row of the groove name, under the value rows.
    fn name_row(&self) -> Rect {
        let p = self.panel();
        let y = self.rows_top() + QuantizeRow::TOP.len() as f64 * ROW_H;
        Rect::new(p.x0, y, p.x1, y + ROW_H)
    }

    /// The groove name field.
    pub fn name_rect(&self) -> Rect {
        let r = self.name_row();
        let x = self.cluster_x();
        let y = r.y0 + (ROW_H - BTN_H) / 2.0;
        Rect::new(x, y, x + 2.0 * STEP_BTN_W + VALUE_W + 6.0, y + BTN_H)
    }

    /// The RENAME button, right of the name field.
    pub fn rename_rect(&self) -> Rect {
        let f = self.name_rect();
        Rect::new(f.x1 + 6.0, f.y0, self.panel().x1 - PAD, f.y1)
    }

    fn apply_top(&self) -> f64 {
        self.rows_top() + (QuantizeRow::TOP.len() + 1) as f64 * ROW_H
    }

    /// The APPLY button.
    pub fn apply_rect(&self) -> Rect {
        let p = self.panel();
        let y = self.apply_top() + (APPLY_H - BTN_H - 4.0) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 120.0, y + BTN_H + 4.0)
    }

    /// The LIVE ON CLIPS button.
    pub fn live_rect(&self) -> Rect {
        let a = self.apply_rect();
        Rect::new(a.x1 + 8.0, a.y0, a.x1 + 8.0 + 150.0, a.y1)
    }

    /// The CLEAR LIVE button.
    pub fn clear_live_rect(&self) -> Rect {
        let l = self.live_rect();
        Rect::new(l.x1 + 8.0, l.y0, l.x1 + 8.0 + 100.0, l.y1)
    }

    fn lib_top(&self) -> f64 {
        self.apply_top() + APPLY_H
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

    /// What the button or value under `pos` does, for its tooltip (`None` during a drag and
    /// away from a button).
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if self.drag.is_some() {
            return None;
        }
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the panel; the settings are kept");
        }
        if self.apply_rect().contains(pos) {
            return help(
                self.apply_rect(),
                "Quantize the selection with these settings (one undo step)",
            );
        }
        if self.live_rect().contains(pos) {
            return help(
                self.live_rect(),
                "Play the selected clips with the chosen groove at the strength above; their notes stay as they are",
            );
        }
        if self.clear_live_rect().contains(pos) {
            return help(
                self.clear_live_rect(),
                "Give the selected clips back to their track's groove",
            );
        }
        if self.rename_rect().contains(pos) {
            return help(
                self.rename_rect(),
                "Rename the chosen groove of this song (Enter does it too)",
            );
        }
        if self.name_rect().contains(pos) {
            return help(
                self.name_rect(),
                "The name of the chosen groove of this song; type a new one",
            );
        }
        if self.extract_rect().contains(pos) {
            return help(
                self.extract_rect(),
                "Measure the feel of the selected clip as a new groove",
            );
        }
        for i in 0..self.library().len() {
            if let Some(r) = self.delete_rect(i)
                && r.contains(pos)
            {
                return help(r, "Delete this groove from the song");
            }
        }
        for row in QuantizeRow::TOP
            .into_iter()
            .chain([QuantizeRow::ExtractStep, QuantizeRow::ExtractSlots])
        {
            let rects = (
                self.minus_rect(row),
                self.value_rect(row),
                self.plus_rect(row),
            );
            if let Some(h) = stepper_help(pos, row.title(), rects, !row.is_toggle()) {
                return Some(h);
            }
        }
        None
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                self.repaint = true;
                if self.name_drag {
                    self.name.drag(*pos);
                }
                if let Some((row, steps)) = &mut self.drag {
                    let (row, n) = (*row, steps.update(pos.y, PX_PER_STEP));
                    if n != 0 {
                        self.step_by(row, n);
                    }
                }
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => {
                self.drag = None;
                self.name_drag = false;
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                clicks,
            } => {
                self.repaint = true;
                self.name_focus = false;
                if self.name_rect().contains(*pos) {
                    if self.renamable().is_some() {
                        self.name_focus = true;
                        self.name_drag = true;
                        self.name.press(*pos, *clicks, modifiers.shift);
                    }
                } else if let Some(action) = self.click(*pos) {
                    out.push(UiAction::Quantize(action));
                } else if let Some(row) = self.draggable_value_at(*pos) {
                    self.drag = Some((row, StepDrag::new(pos.y)));
                }
            }
            UiEvent::Ime(ime) if self.wants_text() => {
                if self.name.handle_ime(ime) {
                    self.repaint = true;
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } if self.wants_text() => {
                if self.name.handle_key(key, *modifiers).handled() {
                    self.repaint = true;
                } else if *key == Key::Enter {
                    out.extend(self.rename_action().map(UiAction::Quantize));
                }
            }
            UiEvent::Wheel { pos, delta, .. } => {
                self.repaint = true;
                let dir = wheel_steps(delta.y);
                let rows = QuantizeRow::TOP
                    .into_iter()
                    .chain([QuantizeRow::ExtractStep, QuantizeRow::ExtractSlots]);
                if let Some(row) = rows.into_iter().find(|r| self.row_rect(*r).contains(*pos)) {
                    self.step_by(row, dir);
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
        if self.live_rect().contains(pos) {
            return (self.can_apply && self.chosen_groove().is_some())
                .then_some(QuantizeAction::SetLive);
        }
        if self.clear_live_rect().contains(pos) {
            return self.can_apply.then_some(QuantizeAction::ClearLive);
        }
        if self.rename_rect().contains(pos) {
            return self.rename_action();
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
                || (row.is_toggle() && self.value_rect(row).contains(pos))
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
        let name_row = self.name_row();
        let can_rename = self.renamable().is_some();
        sp::label(
            c,
            "Groove name",
            name_row.x0 + PAD,
            name_row.y0 + 17.0,
            10.5,
            if can_rename { t.text } else { t.text_muted },
            Align::Start,
        );
        let field = self.name_rect();
        let focused = self.name_focus && can_rename;
        sp::lcd(c, st, field, focused.then_some(t.focus));
        if can_rename {
            let colors = FieldColors {
                text: t.text,
                caret: t.text,
                selection: mix(t.lcd, t.focus, 0.5),
            };
            self.name.paint(c, field, 10.5, 14.0, colors, focused);
        } else {
            sp::label(
                c,
                &fit_label("pick a groove of this song", field.width() - 12.0, 10.0),
                field.x0 + 8.0,
                field.y0 + 14.0,
                10.0,
                t.text_muted,
                Align::Start,
            );
        }
        let rename = self.rename_rect();
        sp::button(
            c,
            st,
            rename,
            "RENAME",
            None,
            self.rename_action().is_some() && hovered(rename),
        );
        let apply = self.apply_rect();
        sp::button(
            c,
            st,
            apply,
            "APPLY",
            self.can_apply.then_some(st.loop_on),
            self.can_apply && hovered(apply),
        );
        let live = self.live_rect();
        let can_live = self.can_apply && self.chosen_groove().is_some();
        sp::button(
            c,
            st,
            live,
            "LIVE ON CLIPS",
            None,
            can_live && hovered(live),
        );
        let clear = self.clear_live_rect();
        sp::button(
            c,
            st,
            clear,
            "CLEAR LIVE",
            None,
            self.can_apply && hovered(clear),
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
