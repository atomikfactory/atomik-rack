//! The Macros panel: the macro banks of the project, each with its eight macros and the params
//! every macro drives.
//!
//! ```text
//! MACROS                                        [ NEW BANK ] [ CLOSE ]
//! Bank  [Macro Bank] [Macro Bank 2]   New assignments from the right-click menus go to this bank.
//!  MACRO 1  Brightness      [=======-----] 0.62    [ LEARN ] [ CLEAR ]
//!      Lead Cutoff               MIN 20 Hz   MAX 4.00 kHz   [ INV ] [ x ]
//!      Pad Filter 2 cutoff       MIN 200 Hz  MAX 2.00 kHz   [ INV ] [ x ]
//! ```
//!
//! A modal view the app places over the window like the MIDI Mappings panel. It has no
//! document: the app hands it the banks with [`MacrosView::set_banks`] (every frame is fine) and
//! what the user does arrives as [`UiAction::Macros`]:
//!
//! * a click on a **bank chip** selects the bank ([`MacrosAction::SelectBank`]); NEW BANK makes one;
//! * a click on a macro's **bar** sets it ([`MacrosAction::SetValue`]), the wheel over it moves it
//!   by 2 % (Shift: 0.5 %); a click on its **name** edits the name in place (Enter keeps it, Esc
//!   drops it, an empty name goes back to `Macro n`); **LEARN** arms the macro for a hardware
//!   control and **CLEAR** removes its assignments;
//! * on an assignment row a click on **MIN** or **MAX** moves it up by a fiftieth of the param's
//!   range (a step for a stepped param), a right click down, the wheel too (Shift: a tenth of that);
//!   **INV** runs the macro backwards ([`MacrosAction::SetRange`] with the whole assignment
//!   changed), the cross removes the assignment and a click on the target selects it in the rack;
//!   a **drag** sideways on MIN or MAX moves the value continuously (the whole param range
//!   over 300 px, Shift a tenth of that; [`MacrosAction::DragRange`] steps and one
//!   [`MacrosAction::EndDrag`], so a drag is one undo step), and a click that does not travel is
//!   the step described above; a **middle click** on MIN or MAX, or Enter with the pointer on it,
//!   types the value in place (Enter keeps it, Esc drops it; the text is a number in the param's
//!   unit, with `k` for kilohertz, `s` for seconds and a percent as 0 to 100; the value is held
//!   to the param's range and rounded for a stepped param);
//! * **CURVE** cycles the shape of the travel (linear, exp, log; a right click goes back) and
//!   **SM** the smoothing time of a live move (a click goes to the next of a few times, a right
//!   click or the wheel steps it; [`MacrosAction::SetShape`]).
//!
//! An assignment whose bank or target is gone is painted in the warning colour and marked missing.
//! The list scrolls with the wheel over the rows; only the rows in view are painted.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::Theme;
use crate::tooltip::Help;
use atomik_model::{DeviceKey, MAX_MACRO_NAME_CHARS, MacroCurve};
use std::cell::Cell;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const BANKS_H: f64 = 30.0;
const MACRO_H: f64 = 30.0;
const ROW_H: f64 = 24.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 900.0;
const MIN_WIDTH: f64 = 620.0;
const BTN_H: f64 = 20.0;
const BAR_STEP: f32 = 0.02;
const BAR_STEP_FINE: f32 = 0.005;
/// The first merge id of a range drag (a base no other view uses).
const MERGE_BASE: u64 = 20 << 40;
/// How far the pointer must travel sideways before a press on MIN or MAX is a drag, pixels.
const DRAG_THRESHOLD: f64 = 3.0;
/// Pixels of travel for the whole range of a param.
const DRAG_PIXELS: f64 = 300.0;
/// The most characters a typed range value holds.
const ENTRY_CHARS: usize = 12;
/// The smoothing times a click steps through, milliseconds.
pub const SMOOTH_STEPS: [u16; 10] = [0, 10, 25, 50, 100, 250, 500, 1000, 2000, 5000];

/// How the text typed for MIN or MAX maps to a plain value of the param.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EntryUnit {
    /// The number is the value (decibels, semitones, a count or a choice index).
    #[default]
    Number,
    /// A percentage: 50 is 0.5.
    Percent,
    /// Hertz; a trailing `k` makes it kilohertz.
    Hz,
    /// Milliseconds; a trailing `s` makes it seconds.
    Ms,
}

impl EntryUnit {
    /// The text an edit starts from: the value as it is typed (a percentage as 0 to 100), without
    /// a unit and without trailing zeros.
    pub fn text(self, value: f32) -> String {
        let shown = if self == EntryUnit::Percent {
            value * 100.0
        } else {
            value
        };
        let text = format!("{shown:.3}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }

    /// The plain value typed text means: a number, then an optional unit (see the variants);
    /// `None` when it is not a number.
    pub fn parse(self, text: &str) -> Option<f32> {
        let t = text.trim().to_lowercase();
        let end = t
            .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+')))
            .unwrap_or(t.len());
        let (number, suffix) = t.split_at(end);
        let n: f32 = number.parse().ok()?;
        let suffix = suffix.trim();
        let value = match self {
            EntryUnit::Percent => n / 100.0,
            EntryUnit::Hz if suffix.starts_with('k') => n * 1000.0,
            EntryUnit::Ms if suffix == "s" => n * 1000.0,
            _ => n,
        };
        value.is_finite().then_some(value)
    }
}

/// One assignment of a macro.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroAssignRow {
    /// The assignment's id.
    pub id: u32,
    /// The driven device (what a click on the target selects in the rack).
    pub device: DeviceKey,
    /// The driven param's key.
    pub param: String,
    /// The target as text (`Lead Cutoff`).
    pub target: String,
    /// The value at macro 0.
    pub min: f32,
    /// The value at macro 1.
    pub max: f32,
    /// The macro runs backwards.
    pub invert: bool,
    /// How the macro's travel maps onto the range.
    pub curve: MacroCurve,
    /// The glide time of a live move, in milliseconds.
    pub smooth_ms: u16,
    /// The param's own range and the step of one click.
    pub param_min: f32,
    /// The param's own maximum.
    pub param_max: f32,
    /// The size of one click on MIN or MAX (a step for a stepped param).
    pub step: f32,
    /// `min` as text with its unit.
    pub min_text: String,
    /// `max` as text with its unit.
    pub max_text: String,
    /// The bank or the target is gone (the assignment does nothing).
    pub missing: bool,
    /// How typed text for MIN and MAX maps to a value of the param.
    pub unit: EntryUnit,
}

/// One macro of a bank.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroRow {
    /// The macro's number, 0-based.
    pub index: u8,
    /// Its name (`Macro 1` when it has none).
    pub name: String,
    /// The macro's value, 0 to 1.
    pub value: f32,
    /// A hardware control is mapped to it.
    pub mapped: bool,
    /// The params it drives.
    pub assigns: Vec<MacroAssignRow>,
}

/// One bank with its eight macros.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroBankRow {
    /// The bank device.
    pub key: DeviceKey,
    /// The bank's name.
    pub name: String,
    /// The macros, in order.
    pub macros: Vec<MacroRow>,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub enum MacrosAction {
    /// Make this bank the one new assignments go to.
    SelectBank(DeviceKey),
    /// Add a macro bank.
    NewBank,
    /// Set the macro's value.
    SetValue {
        /// The bank.
        bank: DeviceKey,
        /// The macro, 0-based.
        index: u8,
        /// The new value, 0 to 1.
        value: f32,
    },
    /// Replace the range and direction of assignment `id`.
    SetRange {
        /// The assignment.
        id: u32,
        /// The value at macro 0.
        min: f32,
        /// The value at macro 1.
        max: f32,
        /// The macro runs backwards.
        invert: bool,
    },
    /// A step of a drag on MIN or MAX: the range of assignment `id`, merged with the other steps
    /// of the drag (`merge`) into one undo step.
    DragRange {
        /// The assignment.
        id: u32,
        /// The value at macro 0.
        min: f32,
        /// The value at macro 1.
        max: f32,
        /// The macro runs backwards.
        invert: bool,
        /// The drag's merge id.
        merge: u64,
    },
    /// The drag on MIN or MAX ended.
    EndDrag,
    /// Remove assignment `id`.
    Remove(u32),
    /// Change the curve and the smoothing time of assignment `id`.
    SetShape {
        /// The assignment.
        id: u32,
        /// How the macro's travel maps onto the range.
        curve: atomik_model::MacroCurve,
        /// The glide time of a live move in milliseconds.
        smooth_ms: u16,
    },
    /// Drive `param` of `device` with macro `index` of `bank`: a knob dropped on a macro knob of
    /// the bank's rack panel (the Macros panel itself never sends it).
    Assign {
        /// The bank.
        bank: DeviceKey,
        /// The macro, 0-based.
        index: u8,
        /// The device that owns the knob.
        device: DeviceKey,
        /// The knob's param key.
        param: String,
    },
    /// Remove every assignment of the macro.
    Clear {
        /// The bank.
        bank: DeviceKey,
        /// The macro, 0-based.
        index: u8,
    },
    /// Name the macro (empty: back to the default).
    Rename {
        /// The bank.
        bank: DeviceKey,
        /// The macro, 0-based.
        index: u8,
        /// The new name.
        name: String,
    },
    /// Arm the macro for a hardware control (the panel gives way).
    Learn {
        /// The bank.
        bank: DeviceKey,
        /// The macro, 0-based.
        index: u8,
    },
    /// Select the target of an assignment in the rack.
    Reveal {
        /// The driven device.
        device: DeviceKey,
        /// The driven param.
        param: String,
    },
    /// Close the panel.
    Close,
}

/// A place in the panel a pointer can hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Close,
    NewBank,
    Bank(usize),
    Name(u8),
    Bar(u8),
    Learn(u8),
    Clear(u8),
    Target(u8, usize),
    Min(u8, usize),
    Max(u8, usize),
    Curve(u8, usize),
    Smooth(u8, usize),
    Invert(u8, usize),
    Remove(u8, usize),
}

/// One line of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Macro(u8),
    Assign(u8, usize),
}

impl Item {
    fn height(self) -> f64 {
        match self {
            Item::Macro(_) => MACRO_H,
            Item::Assign(..) => ROW_H,
        }
    }
}

/// A name being edited in place.
#[derive(Debug, Clone)]
struct Rename {
    index: u8,
    field: TextField,
}

/// A range value being typed in place.
#[derive(Debug, Clone)]
struct RangeEntry {
    /// The assignment.
    id: u32,
    /// MAX (true) or MIN.
    max: bool,
    /// The text so far (all selected at first: the first character replaces it).
    field: TextField,
}

/// A press on MIN or MAX that may become a drag.
#[derive(Debug, Clone, PartialEq)]
struct RangeDrag {
    /// The assignment.
    id: u32,
    /// MAX (true) or MIN.
    max: bool,
    /// Where the press was.
    start_x: f64,
    /// The pointer position the value follows from (the press, then the fine switch).
    anchor_x: f64,
    /// The value being dragged at `anchor_x`.
    base: f32,
    /// The value the last step asked for.
    last: f32,
    /// The drag's merge id.
    merge: u64,
    /// The pointer has travelled: the release is not a click.
    moved: bool,
    /// The click this press is when it does not travel.
    click: Option<MacrosAction>,
}

/// The Macros panel. See the module documentation.
#[derive(Debug)]
pub struct MacrosView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    banks: Vec<MacroBankRow>,
    selected: usize,
    scroll: f64,
    hover: Option<Point>,
    rename: Option<Rename>,
    range_drag: Option<RangeDrag>,
    entry: Option<RangeEntry>,
    /// Where the name or value being typed was painted (a press inside it places the caret).
    field_rect: Cell<Rect>,
    /// A pointer drag is selecting in the field being typed.
    field_drag: bool,
    drags: u64,
    repaint: bool,
}

fn nudge(value: f32, by: f32, a: &MacroAssignRow) -> f32 {
    let moved = (value + by).clamp(a.param_min.min(a.param_max), a.param_min.max(a.param_max));
    if a.step >= 1.0 { moved.round() } else { moved }
}

impl MacrosView {
    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// An empty panel; call [`Self::set_bounds`] and [`Self::set_banks`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 1000.0, 600.0),
            scale: 1.0,
            banks: Vec::new(),
            selected: 0,
            scroll: 0.0,
            hover: None,
            rename: None,
            range_drag: None,
            entry: None,
            field_rect: Cell::new(Rect::ZERO),
            field_drag: false,
            drags: 0,
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

    /// Replaces the banks and keeps the selected one by key. Cheap when nothing changed.
    pub fn set_banks(&mut self, banks: Vec<MacroBankRow>, selected: Option<&DeviceKey>) {
        let at = selected
            .and_then(|k| banks.iter().position(|b| &b.key == k))
            .unwrap_or_else(|| self.selected.min(banks.len().saturating_sub(1)));
        if banks == self.banks && at == self.selected {
            return;
        }
        self.banks = banks;
        self.selected = at;
        if self.rename.as_ref().is_some_and(|r| {
            self.bank()
                .is_none_or(|b| usize::from(r.index) >= b.macros.len())
        }) {
            self.rename = None;
        }
        self.clamp_scroll();
        self.repaint = true;
    }

    /// The listed banks.
    pub fn banks(&self) -> &[MacroBankRow] {
        &self.banks
    }

    /// The selected bank.
    pub fn bank(&self) -> Option<&MacroBankRow> {
        self.banks.get(self.selected)
    }

    /// True while a macro name or a range value is being edited (the panel is a text owner).
    pub fn is_editing(&self) -> bool {
        self.rename.is_some() || self.entry.is_some()
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

    fn items(&self) -> Vec<Item> {
        let Some(bank) = self.bank() else {
            return Vec::new();
        };
        let mut items = Vec::new();
        for m in &bank.macros {
            items.push(Item::Macro(m.index));
            items.extend((0..m.assigns.len()).map(|j| Item::Assign(m.index, j)));
        }
        items
    }

    fn content_height(&self) -> f64 {
        self.items().iter().map(|i| i.height()).sum()
    }

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN)
            .clamp(MIN_WIDTH.min(self.bounds.width()), MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let wanted = HEADER_H + BANKS_H + self.content_height().max(MACRO_H) + PAD;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + wanted.min(room))
    }

    fn list_area(&self) -> Rect {
        let p = self.panel();
        Rect::new(p.x0, p.y0 + HEADER_H + BANKS_H, p.x1, p.y1 - PAD)
    }

    fn clamp_scroll(&mut self) {
        let max = (self.content_height() - self.list_area().height()).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    fn header_button(&self, from_right: usize, w: f64) -> Rect {
        let p = self.panel();
        let right = p.x1 - PAD - from_right as f64 * 100.0;
        Rect::new(right - w, p.y0 + 7.0, right, p.y0 + 7.0 + BTN_H)
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        self.header_button(0, 80.0)
    }

    /// The NEW BANK button.
    pub fn new_bank_rect(&self) -> Rect {
        self.header_button(1, 90.0)
    }

    /// The chip of bank `i`.
    pub fn bank_rect(&self, i: usize) -> Rect {
        let p = self.panel();
        let mut x = p.x0 + PAD + 40.0;
        for (n, b) in self.banks.iter().enumerate() {
            let w = (b.name.chars().count() as f64 * 6.5 + 20.0).clamp(60.0, 170.0);
            if n == i {
                let y = p.y0 + HEADER_H + 4.0;
                return Rect::new(x, y, x + w, y + BTN_H);
            }
            x += w + 6.0;
        }
        Rect::ZERO
    }

    /// The rectangle of an item in view, `None` when it is scrolled out of the list.
    fn item_rect(&self, at: usize) -> Option<Rect> {
        let area = self.list_area();
        let items = self.items();
        let item = items.get(at)?;
        let y = area.y0 + items[..at].iter().map(|i| i.height()).sum::<f64>() - self.scroll;
        let row = Rect::new(area.x0, y, area.x1, y + item.height());
        (row.y0 >= area.y0 - 0.5 && row.y1 <= area.y1 + 0.5).then_some(row)
    }

    fn cell(&self, row: Rect, x: (f64, f64)) -> Rect {
        let y = row.y0 + (row.height() - BTN_H) / 2.0;
        Rect::new(x.0, y, x.1, y + BTN_H)
    }

    /// The columns of a macro line: name, bar, value, learn, clear.
    fn macro_columns(&self, row: Rect) -> MacroColumns {
        let mut right = row.x1 - PAD;
        let mut take = |w: f64, gap: f64| {
            let r = (right - w, right);
            right = r.0 - gap;
            r
        };
        let clear = take(58.0, 6.0);
        let learn = take(58.0, 12.0);
        let value = take(44.0, 6.0);
        let bar = take(220.0, 12.0);
        let name = (row.x0 + PAD, (right).max(row.x0 + PAD + 60.0));
        MacroColumns {
            name,
            bar,
            value,
            learn,
            clear,
        }
    }

    /// The columns of an assignment line: target, min, max, invert, remove.
    fn assign_columns(&self, row: Rect) -> AssignColumns {
        let mut right = row.x1 - PAD;
        let mut take = |w: f64, gap: f64| {
            let r = (right - w, right);
            right = r.0 - gap;
            r
        };
        let remove = take(22.0, 8.0);
        let invert = take(40.0, 8.0);
        let smooth = take(70.0, 8.0);
        let curve = take(40.0, 12.0);
        let max = take(108.0, 6.0);
        let min = take(108.0, 12.0);
        let target = (row.x0 + PAD + 26.0, right.max(row.x0 + PAD + 60.0));
        AssignColumns {
            target,
            min,
            max,
            curve,
            smooth,
            invert,
            remove,
        }
    }

    fn hit(&self, pos: Point) -> Option<Hit> {
        for (hit, r) in [
            (Hit::Close, self.close_rect()),
            (Hit::NewBank, self.new_bank_rect()),
        ] {
            if r.contains(pos) {
                return Some(hit);
            }
        }
        for i in 0..self.banks.len() {
            if self.bank_rect(i).contains(pos) {
                return Some(Hit::Bank(i));
            }
        }
        if !self.list_area().contains(pos) {
            return None;
        }
        for (at, item) in self.items().into_iter().enumerate() {
            let Some(row) = self.item_rect(at) else {
                continue;
            };
            if !row.contains(pos) {
                continue;
            }
            return match item {
                Item::Macro(n) => {
                    let c = self.macro_columns(row);
                    let inside = |x: (f64, f64)| self.cell(row, x).contains(pos);
                    [
                        (Hit::Clear(n), c.clear),
                        (Hit::Learn(n), c.learn),
                        (Hit::Bar(n), c.bar),
                        (Hit::Name(n), c.name),
                    ]
                    .into_iter()
                    .find(|(_, x)| inside(*x))
                    .map(|(h, _)| h)
                }
                Item::Assign(n, j) => {
                    let c = self.assign_columns(row);
                    let inside = |x: (f64, f64)| self.cell(row, x).contains(pos);
                    [
                        (Hit::Remove(n, j), c.remove),
                        (Hit::Invert(n, j), c.invert),
                        (Hit::Smooth(n, j), c.smooth),
                        (Hit::Curve(n, j), c.curve),
                        (Hit::Max(n, j), c.max),
                        (Hit::Min(n, j), c.min),
                        (Hit::Target(n, j), c.target),
                    ]
                    .into_iter()
                    .find(|(_, x)| inside(*x))
                    .map(|(h, _)| h)
                }
            };
        }
        None
    }

    fn assign(&self, n: u8, j: usize) -> Option<&MacroAssignRow> {
        self.bank()?.macros.get(usize::from(n))?.assigns.get(j)
    }

    fn macro_row(&self, n: u8) -> Option<&MacroRow> {
        self.bank()?.macros.get(usize::from(n))
    }

    // ------------------------------------------------------------------ events

    /// What the button under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if self.is_editing() {
            return None;
        }
        let hit = self.hit(pos)?;
        let help = |r: Rect, text: &'static str| Some(Help::new(r, text));
        match hit {
            Hit::Close => help(self.close_rect(), "Close the panel"),
            Hit::NewBank => help(self.new_bank_rect(), "Start a new bank of eight macros"),
            Hit::Bank(i) => help(self.bank_rect(i), "Show this bank of macros"),
            Hit::Name(n) | Hit::Bar(n) | Hit::Learn(n) | Hit::Clear(n) => {
                let at = self.items().iter().position(|i| *i == Item::Macro(n))?;
                let row = self.item_rect(at)?;
                let c = self.macro_columns(row);
                let (x, text) = match hit {
                    Hit::Name(_) => (c.name, "Click to rename this macro"),
                    Hit::Bar(_) => (c.bar, "Click to set the macro's value"),
                    Hit::Learn(_) => (c.learn, "Learn: move a knob or key to control this macro"),
                    _ => (c.clear, "Remove every parameter this macro drives"),
                };
                help(self.cell(row, x), text)
            }
            Hit::Target(n, j)
            | Hit::Min(n, j)
            | Hit::Max(n, j)
            | Hit::Curve(n, j)
            | Hit::Smooth(n, j)
            | Hit::Invert(n, j)
            | Hit::Remove(n, j) => {
                let at = self.items().iter().position(|i| *i == Item::Assign(n, j))?;
                let row = self.item_rect(at)?;
                let c = self.assign_columns(row);
                let (x, text) = match hit {
                    Hit::Target(..) => (c.target, "Show this parameter in the rack"),
                    Hit::Min(..) => (
                        c.min,
                        "Value at macro minimum: drag; click raises, right-click lowers; middle-click types",
                    ),
                    Hit::Max(..) => (
                        c.max,
                        "Value at macro maximum: drag; click raises, right-click lowers; middle-click types",
                    ),
                    Hit::Curve(..) => (
                        c.curve,
                        "Shape of the travel: linear, exp (slow start) or log (fast start)",
                    ),
                    Hit::Smooth(..) => (
                        c.smooth,
                        "Glide time of a live move: click for longer, right-click or wheel to step",
                    ),
                    Hit::Invert(..) => (c.invert, "Invert the direction of the parameter"),
                    _ => (c.remove, "Stop this macro driving the parameter"),
                };
                help(self.cell(row, x), text)
            }
        }
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, modifiers } => {
                self.hover = Some(*pos);
                self.repaint = true;
                if self.field_drag {
                    match (&mut self.rename, &mut self.entry) {
                        (Some(r), _) => r.field.drag(*pos),
                        (None, Some(e)) => e.field.drag(*pos),
                        _ => {}
                    }
                }
                out.extend(self.drag_move(pos.x, modifiers.shift));
            }
            UiEvent::PointerUp { .. } => {
                self.field_drag = false;
                if let Some(d) = self.range_drag.take() {
                    self.repaint = true;
                    match (d.moved, d.click) {
                        (true, _) => out.push(UiAction::Macros(MacrosAction::EndDrag)),
                        (false, Some(click)) => out.push(UiAction::Macros(click)),
                        (false, None) => {}
                    }
                }
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerDown {
                pos,
                button,
                modifiers,
                clicks,
            } => {
                self.repaint = true;
                if *button == PointerButton::Secondary && self.field_rect.get().contains(*pos) {
                    let field = match (&self.rename, &self.entry) {
                        (Some(r), _) => Some(&r.field),
                        (None, Some(e)) => Some(&e.field),
                        _ => None,
                    };
                    if field.is_some_and(|f| f.right_click(*pos)) {
                        return out;
                    }
                }
                // A press on the name or value being typed places the caret.
                if *button == PointerButton::Primary && self.field_rect.get().contains(*pos) {
                    let field = match (&mut self.rename, &mut self.entry) {
                        (Some(r), _) => Some(&mut r.field),
                        (None, Some(e)) => Some(&mut e.field),
                        _ => None,
                    };
                    if let Some(field) = field {
                        field.press(*pos, *clicks, modifiers.shift);
                        self.field_drag = true;
                        return out;
                    }
                }
                // A press anywhere keeps an edited name (a click elsewhere is "done").
                if self.rename.is_some() {
                    out.extend(self.finish_rename(true));
                }
                if self.entry.is_some() {
                    out.extend(self.finish_entry(true));
                }
                if let Some(hit) = self.hit(*pos) {
                    out.extend(self.press(hit, *pos, *button));
                }
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => {
                self.repaint = true;
                out.extend(self.wheel(delta.y, *pos, *modifiers));
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => out.extend(self.key(key, *modifiers)),
            UiEvent::Ime(ime) => {
                let field = match (&mut self.rename, &mut self.entry) {
                    (Some(r), _) => Some(&mut r.field),
                    (None, Some(e)) => Some(&mut e.field),
                    _ => None,
                };
                if let Some(field) = field {
                    field.handle_ime(ime);
                    self.repaint = true;
                }
            }
            _ => {}
        }
        out
    }

    fn wheel(&mut self, dy: f64, pos: Point, modifiers: Modifiers) -> Vec<UiAction> {
        let up = dy > 0.0;
        let act = |a: MacrosAction| vec![UiAction::Macros(a)];
        match self.hit(pos) {
            Some(Hit::Bar(n)) if dy != 0.0 => {
                let (Some(bank), Some(m)) = (self.bank(), self.macro_row(n)) else {
                    return Vec::new();
                };
                let step = if modifiers.shift {
                    BAR_STEP_FINE
                } else {
                    BAR_STEP
                };
                let value = (m.value + if up { step } else { -step }).clamp(0.0, 1.0);
                act(MacrosAction::SetValue {
                    bank: bank.key.clone(),
                    index: n,
                    value,
                })
            }
            Some(Hit::Smooth(n, j)) if dy != 0.0 => self.assign(n, j).map_or_else(Vec::new, |a| {
                smooth_action(a, step_smooth(a.smooth_ms, up)).map_or_else(Vec::new, act)
            }),
            Some(Hit::Min(n, j) | Hit::Max(n, j)) if dy != 0.0 => {
                let max = matches!(self.hit(pos), Some(Hit::Max(..)));
                let Some(a) = self.assign(n, j) else {
                    return Vec::new();
                };
                let fine = if modifiers.shift { 0.1 } else { 1.0 };
                let by = a.step * fine * if up { 1.0 } else { -1.0 };
                self.range_edit(a, max, by).map_or_else(Vec::new, act)
            }
            _ => {
                self.scroll -= dy * ROW_H;
                self.clamp_scroll();
                Vec::new()
            }
        }
    }

    /// A move of the pointer during a press on MIN or MAX: past the threshold the value follows
    /// the pointer sideways. Shift makes it a tenth as fast from where it was switched on.
    fn drag_move(&mut self, x: f64, fine: bool) -> Vec<UiAction> {
        let Some(d) = self.range_drag.as_mut() else {
            return Vec::new();
        };
        if !d.moved && (x - d.start_x).abs() < DRAG_THRESHOLD {
            return Vec::new();
        }
        d.moved = true;
        let (id, max, merge) = (d.id, d.max, d.merge);
        let (base, anchor, last) = (d.base, d.anchor_x, d.last);
        let Some(a) = self.assign_by_id(id).cloned() else {
            self.range_drag = None;
            return Vec::new();
        };
        let span = f64::from(a.param_max - a.param_min);
        let speed = if fine { 0.1 } else { 1.0 };
        let moved = base + ((x - anchor) / DRAG_PIXELS * span * speed) as f32;
        let value = nudge(moved, 0.0, &a);
        if value.to_bits() == last.to_bits() {
            return Vec::new();
        }
        if let Some(d) = self.range_drag.as_mut() {
            d.last = value;
        }
        let (min, max_v) = if max { (a.min, value) } else { (value, a.max) };
        vec![UiAction::Macros(MacrosAction::DragRange {
            id,
            min,
            max: max_v,
            invert: a.invert,
            merge,
        })]
    }

    fn assign_by_id(&self, id: u32) -> Option<&MacroAssignRow> {
        self.bank()?
            .macros
            .iter()
            .flat_map(|m| m.assigns.iter())
            .find(|a| a.id == id)
    }

    fn range_edit(&self, a: &MacroAssignRow, max: bool, by: f32) -> Option<MacrosAction> {
        let (mut min_v, mut max_v) = (a.min, a.max);
        if max {
            max_v = nudge(a.max, by, a);
        } else {
            min_v = nudge(a.min, by, a);
        }
        ((min_v, max_v) != (a.min, a.max)).then_some(MacrosAction::SetRange {
            id: a.id,
            min: min_v,
            max: max_v,
            invert: a.invert,
        })
    }

    fn press(&mut self, hit: Hit, pos: Point, button: PointerButton) -> Vec<UiAction> {
        let forward = button != PointerButton::Secondary;
        let act = |a: MacrosAction| vec![UiAction::Macros(a)];
        let bank = self.bank().map(|b| b.key.clone());
        match hit {
            Hit::Close => act(MacrosAction::Close),
            Hit::NewBank => act(MacrosAction::NewBank),
            Hit::Bank(i) => self
                .banks
                .get(i)
                .map_or_else(Vec::new, |b| act(MacrosAction::SelectBank(b.key.clone()))),
            Hit::Name(n) => {
                if let Some(m) = self.macro_row(n) {
                    self.rename = Some(Rename {
                        index: n,
                        field: TextField::new(&m.name, MAX_MACRO_NAME_CHARS, false),
                    });
                }
                Vec::new()
            }
            Hit::Bar(n) => {
                let Some(bank) = bank else {
                    return Vec::new();
                };
                // The bar's own rectangle gives the position of the click.
                let Some(at) = self
                    .items()
                    .iter()
                    .position(|i| *i == Item::Macro(n))
                    .and_then(|at| self.item_rect(at))
                else {
                    return Vec::new();
                };
                let bar = self.macro_columns(at).bar;
                let value = (((pos.x - bar.0) / (bar.1 - bar.0)) as f32).clamp(0.0, 1.0);
                act(MacrosAction::SetValue {
                    bank,
                    index: n,
                    value,
                })
            }
            Hit::Learn(n) => {
                bank.map_or_else(Vec::new, |bank| act(MacrosAction::Learn { bank, index: n }))
            }
            Hit::Clear(n) => {
                bank.map_or_else(Vec::new, |bank| act(MacrosAction::Clear { bank, index: n }))
            }
            Hit::Target(n, j) => self.assign(n, j).map_or_else(Vec::new, |a| {
                act(MacrosAction::Reveal {
                    device: a.device.clone(),
                    param: a.param.clone(),
                })
            }),
            Hit::Min(n, j) | Hit::Max(n, j) => {
                let max = matches!(hit, Hit::Max(..));
                if button == PointerButton::Middle {
                    self.open_entry(n, j, max);
                    return Vec::new();
                }
                let Some(a) = self.assign(n, j) else {
                    return Vec::new();
                };
                let by = if forward { a.step } else { -a.step };
                let click = self.range_edit(a, max, by);
                if !forward {
                    return click.map_or_else(Vec::new, act);
                }
                // A press with the main button waits: a drag moves the value, a click that did
                // not travel is the step.
                let (id, base) = (a.id, if max { a.max } else { a.min });
                self.drags += 1;
                self.range_drag = Some(RangeDrag {
                    id,
                    max,
                    start_x: pos.x,
                    anchor_x: pos.x,
                    base,
                    last: base,
                    merge: MERGE_BASE + self.drags,
                    moved: false,
                    click,
                });
                Vec::new()
            }
            Hit::Curve(n, j) => self.assign(n, j).map_or_else(Vec::new, |a| {
                let curve = if forward {
                    a.curve.next()
                } else {
                    a.curve.next().next()
                };
                act(shape_of(a, curve, a.smooth_ms))
            }),
            Hit::Smooth(n, j) => self.assign(n, j).map_or_else(Vec::new, |a| {
                let ms = if forward {
                    cycle_smooth(a.smooth_ms)
                } else {
                    step_smooth(a.smooth_ms, false)
                };
                smooth_action(a, ms).map_or_else(Vec::new, act)
            }),
            Hit::Invert(n, j) => self.assign(n, j).map_or_else(Vec::new, |a| {
                act(MacrosAction::SetRange {
                    id: a.id,
                    min: a.min,
                    max: a.max,
                    invert: !a.invert,
                })
            }),
            Hit::Remove(n, j) => self
                .assign(n, j)
                .map_or_else(Vec::new, |a| act(MacrosAction::Remove(a.id))),
        }
    }

    /// Starts typing the MIN or MAX value of assignment `j` of macro `n`.
    fn open_entry(&mut self, n: u8, j: usize, max: bool) {
        let Some(a) = self.assign(n, j) else { return };
        let mut field = TextField::new(
            &a.unit.text(if max { a.max } else { a.min }),
            ENTRY_CHARS,
            false,
        );
        field.select_all();
        self.entry = Some(RangeEntry {
            id: a.id,
            max,
            field,
        });
        self.repaint = true;
    }

    /// Ends the typed value; applies it when `keep` and the text is a number that changes the
    /// range (held to the param's range, rounded for a stepped param).
    fn finish_entry(&mut self, keep: bool) -> Vec<UiAction> {
        let Some(e) = self.entry.take() else {
            return Vec::new();
        };
        self.repaint = true;
        if !keep {
            return Vec::new();
        }
        let Some(a) = self.assign_by_id(e.id) else {
            return Vec::new();
        };
        let Some(typed) = a.unit.parse(e.field.text()) else {
            return Vec::new();
        };
        let value = nudge(typed, 0.0, a);
        let (min, max) = if e.max {
            (a.min, value)
        } else {
            (value, a.max)
        };
        if (min, max) == (a.min, a.max) {
            return Vec::new();
        }
        vec![UiAction::Macros(MacrosAction::SetRange {
            id: a.id,
            min,
            max,
            invert: a.invert,
        })]
    }

    /// Ends the name edit; keeps the text when `keep`.
    fn finish_rename(&mut self, keep: bool) -> Vec<UiAction> {
        let Some(r) = self.rename.take() else {
            return Vec::new();
        };
        self.repaint = true;
        let (Some(bank), true) = (self.bank(), keep) else {
            return Vec::new();
        };
        let unchanged = self
            .macro_row(r.index)
            .is_some_and(|m| m.name == r.field.text());
        if unchanged {
            return Vec::new();
        }
        vec![UiAction::Macros(MacrosAction::Rename {
            bank: bank.key.clone(),
            index: r.index,
            name: r.field.text().to_owned(),
        })]
    }

    fn key(&mut self, key: &Key, modifiers: Modifiers) -> Vec<UiAction> {
        self.repaint = true;
        if let Some(r) = &mut self.rename {
            match key {
                Key::Enter => return self.finish_rename(true),
                Key::Escape => return self.finish_rename(false),
                _ => {
                    r.field.handle_key(key, modifiers);
                }
            }
            return Vec::new();
        }
        if let Some(e) = &mut self.entry {
            match key {
                Key::Enter => return self.finish_entry(true),
                Key::Escape => return self.finish_entry(false),
                _ => {
                    e.field.handle_key(key, modifiers);
                }
            }
            return Vec::new();
        }
        match key {
            Key::Enter => {
                // Enter with the pointer on MIN or MAX types the value in place.
                let under = self.hover.and_then(|p| self.hit(p));
                if let Some(Hit::Min(n, j) | Hit::Max(n, j)) = under {
                    self.open_entry(n, j, matches!(under, Some(Hit::Max(..))));
                }
                Vec::new()
            }
            Key::Escape => vec![UiAction::Macros(MacrosAction::Close)],
            Key::PageUp | Key::PageDown => {
                let page = self.list_area().height() * 0.9;
                self.scroll += if matches!(key, Key::PageDown) {
                    page
                } else {
                    -page
                };
                self.clamp_scroll();
                Vec::new()
            }
            _ => Vec::new(),
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
            "MACROS",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        for (r, text) in [
            (self.new_bank_rect(), "NEW BANK"),
            (self.close_rect(), "CLOSE"),
        ] {
            sp::button(c, st, r, text, None, hovered(r));
        }
        if self.banks.is_empty() {
            sp::label(
                c,
                &fit_label(
                    "No macro bank yet. NEW BANK adds one, or right-click a control and choose Assign to Macro.",
                    p.width() - 2.0 * PAD,
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
        sp::label(
            c,
            "BANK",
            p.x0 + PAD,
            p.y0 + HEADER_H + 18.0,
            8.5,
            t.text_muted,
            Align::Start,
        );
        for (i, b) in self.banks.iter().enumerate() {
            let r = self.bank_rect(i);
            if r.x1 > p.x1 - PAD {
                break;
            }
            let lit = (i == self.selected).then_some(t.highlight);
            sp::button(
                c,
                st,
                r,
                &fit_label(&b.name, r.width() - 10.0, 9.5),
                lit,
                hovered(r),
            );
        }
        sp::line(
            c,
            p.x0,
            p.y0 + HEADER_H + BANKS_H - 0.5,
            p.x1,
            p.y0 + HEADER_H + BANKS_H - 0.5,
            st.separator,
        );
        let area = self.list_area();
        c.push_clip(&area.into());
        for (at, item) in self.items().into_iter().enumerate() {
            let y =
                area.y0 + self.items()[..at].iter().map(|i| i.height()).sum::<f64>() - self.scroll;
            if y + item.height() < area.y0 || y > area.y1 {
                continue;
            }
            let row = Rect::new(area.x0, y, area.x1, y + item.height());
            match item {
                Item::Macro(n) => self.paint_macro(c, row, n),
                Item::Assign(n, j) => self.paint_assign(c, row, n, j),
            }
        }
        c.pop_clip();
        c.pop_clip();
        c.pop_transform();
    }

    fn paint_macro(&self, c: &mut dyn Canvas, row: Rect, n: u8) {
        let st = &self.st;
        let t = &st.base;
        let Some(m) = self.macro_row(n) else { return };
        let hovered = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        sp::rect(c, row, st.header);
        let col = self.macro_columns(row);
        let text_y = row.y0 + 19.0;
        sp::label(
            c,
            &format!("M{}", u16::from(n) + 1),
            row.x0 + PAD,
            text_y,
            9.5,
            t.highlight,
            Align::Start,
        );
        let name_cell = self.cell(row, (col.name.0 + 28.0, col.name.1));
        if let Some(r) = self.rename.as_ref().filter(|r| r.index == n) {
            sp::rect(c, name_cell, st.background);
            self.field_rect.set(name_cell);
            r.field.paint(
                c,
                name_cell,
                10.5,
                (name_cell.height() - 6.0).max(8.0),
                FieldColors::of(t),
                true,
            );
        } else {
            let colour = if hovered(name_cell) {
                t.text
            } else {
                t.text_muted
            };
            sp::label(
                c,
                &fit_label(&m.name, name_cell.width() - 6.0, 10.5),
                name_cell.x0 + 4.0,
                text_y,
                10.5,
                colour,
                Align::Start,
            );
        }
        let bar = self.cell(row, col.bar);
        sp::rect(c, bar, st.background);
        sp::rect(
            c,
            Rect::new(
                bar.x0,
                bar.y0,
                bar.x0 + bar.width() * f64::from(m.value.clamp(0.0, 1.0)),
                bar.y1,
            ),
            t.highlight.with_alpha(0.7),
        );
        sp::label(
            c,
            &format!("{:.2}", m.value),
            (col.value.0 + col.value.1) / 2.0,
            text_y,
            9.5,
            t.text,
            Align::Center,
        );
        let learn = self.cell(row, col.learn);
        sp::button(
            c,
            st,
            learn,
            "LEARN",
            m.mapped.then_some(t.highlight),
            hovered(learn),
        );
        let clear = self.cell(row, col.clear);
        sp::button(c, st, clear, "CLEAR", None, hovered(clear));
    }

    fn paint_assign(&self, c: &mut dyn Canvas, row: Rect, n: u8, j: usize) {
        let st = &self.st;
        let t = &st.base;
        let Some(a) = self.assign(n, j) else { return };
        let hovered = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        if j % 2 == 1 {
            sp::rect(c, row, st.lane_alt);
        }
        let col = self.assign_columns(row);
        let text_y = row.y0 + 16.0;
        let (target, colour) = if a.missing {
            (format!("{} (missing)", a.target), t.warning)
        } else {
            (a.target.clone(), t.text)
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
        for (x, name, text, max) in [
            (col.min, "MIN", &a.min_text, false),
            (col.max, "MAX", &a.max_text, true),
        ] {
            let r = self.cell(row, x);
            if let Some(e) = self.entry.as_ref().filter(|e| e.id == a.id && e.max == max) {
                sp::rect(c, r, st.background);
                sp::label(
                    c,
                    name,
                    r.x0 + 4.0,
                    text_y,
                    10.5,
                    t.text_muted,
                    Align::Start,
                );
                let typed = Rect::new(r.x0 + 26.0, r.y0, r.x1, r.y1);
                self.field_rect.set(typed);
                e.field.paint(
                    c,
                    typed,
                    10.5,
                    (typed.height() - 6.0).max(8.0),
                    FieldColors::of(t),
                    true,
                );
            } else {
                sp::button(c, st, r, &format!("{name} {text}"), None, hovered(r));
            }
        }
        let curve = self.cell(row, col.curve);
        sp::button(
            c,
            st,
            curve,
            &curve_label(a.curve),
            (a.curve != MacroCurve::Linear).then_some(t.highlight),
            hovered(curve),
        );
        let smooth = self.cell(row, col.smooth);
        sp::button(
            c,
            st,
            smooth,
            &smooth_label(a.smooth_ms),
            (a.smooth_ms > 0).then_some(t.highlight),
            hovered(smooth),
        );
        let inv = self.cell(row, col.invert);
        sp::button(
            c,
            st,
            inv,
            "INV",
            a.invert.then_some(t.highlight),
            hovered(inv),
        );
        let rm = self.cell(row, col.remove);
        sp::button(c, st, rm, "x", None, hovered(rm));
    }
}

struct MacroColumns {
    name: (f64, f64),
    bar: (f64, f64),
    value: (f64, f64),
    learn: (f64, f64),
    clear: (f64, f64),
}

struct AssignColumns {
    target: (f64, f64),
    min: (f64, f64),
    max: (f64, f64),
    curve: (f64, f64),
    smooth: (f64, f64),
    invert: (f64, f64),
    remove: (f64, f64),
}

fn shape_of(a: &MacroAssignRow, curve: MacroCurve, smooth_ms: u16) -> MacrosAction {
    MacrosAction::SetShape {
        id: a.id,
        curve,
        smooth_ms,
    }
}

/// The action that gives `a` the smoothing time `ms`; `None` when it already has it.
fn smooth_action(a: &MacroAssignRow, ms: u16) -> Option<MacrosAction> {
    (ms != a.smooth_ms).then(|| shape_of(a, a.curve, ms))
}

fn curve_label(curve: MacroCurve) -> String {
    curve.name().to_uppercase().chars().take(3).collect()
}

fn smooth_label(ms: u16) -> String {
    if ms == 0 {
        "SM off".to_owned()
    } else {
        format!("SM {ms}ms")
    }
}

/// The smoothing time one step up (`up`) or down from `ms`, within [`SMOOTH_STEPS`].
fn step_smooth(ms: u16, up: bool) -> u16 {
    if up {
        SMOOTH_STEPS.iter().copied().find(|s| *s > ms)
    } else {
        SMOOTH_STEPS.iter().rev().copied().find(|s| *s < ms)
    }
    .unwrap_or(ms)
}

/// The next smoothing time of a click: one step up, and back to none after the longest.
fn cycle_smooth(ms: u16) -> u16 {
    let up = step_smooth(ms, true);
    if up == ms { 0 } else { up }
}

impl MacrosView {
    /// The centre of a control of the macro line `n`, for tests and the headless driver
    /// (`name`, `bar`, `learn`, `clear`).
    pub fn macro_center(&self, n: u8, what: &str) -> Option<Point> {
        let at = self.items().iter().position(|i| *i == Item::Macro(n))?;
        let row = self.item_rect(at)?;
        let c = self.macro_columns(row);
        let x = match what {
            "name" => (c.name.0 + 28.0, c.name.1),
            "bar" => c.bar,
            "learn" => c.learn,
            "clear" => c.clear,
            _ => return None,
        };
        Some(self.cell(row, x).center())
    }

    /// The centre of a control of assignment `j` of macro `n` (`target`, `min`, `max`, `curve`,
    /// `smooth`, `invert`, `remove`).
    pub fn assign_center(&self, n: u8, j: usize, what: &str) -> Option<Point> {
        let at = self.items().iter().position(|i| *i == Item::Assign(n, j))?;
        let row = self.item_rect(at)?;
        let c = self.assign_columns(row);
        let x = match what {
            "target" => c.target,
            "min" => c.min,
            "max" => c.max,
            "curve" => c.curve,
            "smooth" => c.smooth,
            "invert" => c.invert,
            "remove" => c.remove,
            _ => return None,
        };
        Some(self.cell(row, x).center())
    }
}
