//! The modulation matrix panel: eight rows of source, target and via pickers and an amount bar
//! for a device that declares modulation tables.
//!
//! ```text
//! MODULATION MATRIX  Poly Synth                                         [ CLOSE ]
//!      SOURCE            TARGET            VIA            AMOUNT
//!  1   [ LFO 1      v]  [ Cutoff      v]  [ Mod Wheel v]  [-----|=======]  +45 %   [ x ]
//!  2   [ None       v]  [ None        v]  [ None      v]  [-----|-------]   0 %
//! ```
//!
//! A modal view the app places over the window like the Macros panel. It has no document: the
//! app hands it the device's tables and slots with [`ModMatrixView::set_info`] (every frame is
//! fine) and what the user does comes back as ordinary commands, so the edits are undoable and
//! reach the engine like any other:
//!
//! * a click on a **picker** opens a [`ContextMenu`] of the entries (the current one checked);
//!   choosing one sets the slot with [`WorkflowCommand::SetModSlot`]. A slot needs a source and
//!   a target: until both are chosen the row is a draft the panel keeps, and clearing either one
//!   of a slot in use clears the slot (its amount goes back to 0);
//! * a press on the **amount bar** sets the amount (`mod<n>_amount`) and a drag follows it as one
//!   undo step ([`UiAction::EndMerge`] on release); a right click or double click puts it back to
//!   0, the wheel moves it by 2 % (Shift: 0.5 %). The bar only works while the slot is in use;
//! * the cross clears the slot, Esc closes the open menu, then the panel.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::context_menu::{ContextMenu, Entry, MenuItemDef, MenuOutcome};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::Theme;
use crate::tooltip::Help;
use atomik_device_sdk::{MOD_SLOTS, ModField, mod_param_key};
use atomik_model::{Command, DeviceKey, ModSlot, WorkflowCommand};
use vello::kurbo::{Affine, Point, Rect, Size};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const COLUMNS_H: f64 = 22.0;
const ROW_H: f64 = 28.0;
const FOOT_H: f64 = 30.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 820.0;
const MIN_WIDTH: f64 = 560.0;
const BTN_H: f64 = 20.0;
const NUM_W: f64 = 26.0;
const PICK_W: f64 = 150.0;
const GAP: f64 = 8.0;
const CLEAR_W: f64 = 22.0;
const BAR_STEP: f32 = 0.02;
const BAR_STEP_FINE: f32 = 0.005;
/// The merge ids of an amount drag start here (a base no other view uses).
const MERGE_BASE: u64 = 17 << 40;
/// The command of a picker menu row; its argument is `<field><slot>:<choice>`.
pub const PICK_COMMAND: &str = "modmatrix.pick";

/// The matrix of one device, as the panel shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ModMatrixInfo {
    /// The device (path key) the slots belong to.
    pub device: DeviceKey,
    /// Its name on the plate.
    pub name: String,
    /// The names of the sources, `id == index`.
    pub sources: Vec<String>,
    /// The names of the targets, `id == index`.
    pub targets: Vec<String>,
    /// The slots in use (shorter than the slot count when the last ones are free).
    pub slots: Vec<Option<ModSlot>>,
    /// The amount of every slot (`-1..=1`), by slot.
    pub amounts: Vec<f32>,
}

/// What the panel asks the app to do besides the commands it emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModMatrixAction {
    /// Close the panel.
    Close,
}

/// The three pickers of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Source,
    Target,
    Via,
}

impl Field {
    fn tag(self) -> char {
        match self {
            Field::Source => 's',
            Field::Target => 't',
            Field::Via => 'v',
        }
    }

    fn from_tag(c: char) -> Option<Self> {
        match c {
            's' => Some(Field::Source),
            't' => Some(Field::Target),
            'v' => Some(Field::Via),
            _ => None,
        }
    }
}

/// What a row holds while it is being filled in: a slot needs a source and a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Draft {
    source: Option<u16>,
    target: Option<u16>,
    via: Option<u16>,
}

impl Draft {
    fn of(slot: Option<ModSlot>) -> Self {
        slot.map_or(Self::default(), |s| Self {
            source: Some(s.source),
            target: Some(s.target),
            via: s.via,
        })
    }

    fn slot(self) -> Option<ModSlot> {
        Some(ModSlot {
            source: self.source?,
            target: self.target?,
            via: self.via,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct AmountDrag {
    slot: usize,
    merge: u64,
}

/// An open picker: the menu and the row and field it chooses for.
#[derive(Debug)]
struct Picker {
    menu: ContextMenu,
}

/// The modulation matrix panel. See the module documentation.
#[derive(Debug)]
pub struct ModMatrixView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    info: Option<ModMatrixInfo>,
    drafts: [Draft; MOD_SLOTS],
    picker: Option<Picker>,
    drag: Option<AmountDrag>,
    next_merge: u64,
    hover: Option<Point>,
    repaint: bool,
}

impl ModMatrixView {
    /// A panel with nothing in it; call [`Self::set_info`] and [`Self::set_bounds`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            info: None,
            drafts: [Draft::default(); MOD_SLOTS],
            picker: None,
            drag: None,
            next_merge: MERGE_BASE,
            hover: None,
            repaint: true,
        }
    }

    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// Places the panel: the area it may use (logical pixels). The panel is centred in it.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.picker = None;
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Whether the panel takes typed text: it is modal and every key is its own.
    pub fn is_text_owner(&self) -> bool {
        true
    }

    /// The device the panel shows.
    pub fn device(&self) -> Option<&DeviceKey> {
        self.info.as_ref().map(|i| &i.device)
    }

    /// Feeds the panel the device's matrix. A row the document has no slot for keeps its draft
    /// (a source chosen, the target not yet) unless the draft was a slot in use that is gone now.
    pub fn set_info(&mut self, info: ModMatrixInfo) {
        if self.info.as_ref() == Some(&info) {
            return;
        }
        for (i, draft) in self.drafts.iter_mut().enumerate() {
            let slot = info.slots.get(i).copied().flatten();
            match slot {
                Some(s) => *draft = Draft::of(Some(s)),
                None if draft.slot().is_some() => *draft = Draft::default(),
                None => {}
            }
        }
        self.info = Some(info);
        self.repaint = true;
    }

    /// Whether a picker menu is open.
    pub fn picker_open(&self) -> bool {
        self.picker.is_some()
    }

    /// The centre of the row labelled `label` in the open picker (a test seam for the app).
    pub fn picker_center(&self, label: &str) -> Option<Point> {
        let menu = &self.picker.as_ref()?.menu;
        let n = menu.all_items().iter().position(|i| i.label == label)?;
        menu.row_rects().get(n).map(|r| r.center())
    }

    /// The draft (or slot) of row `slot` as `(source, target, via)` ids.
    pub fn row(&self, slot: usize) -> Option<(Option<u16>, Option<u16>, Option<u16>)> {
        let d = self.drafts.get(slot)?;
        Some((d.source, d.target, d.via))
    }

    fn in_use(&self, slot: usize) -> bool {
        self.info
            .as_ref()
            .is_some_and(|i| i.slots.get(slot).copied().flatten().is_some())
    }

    fn amount(&self, slot: usize) -> f32 {
        self.info
            .as_ref()
            .and_then(|i| i.amounts.get(slot).copied())
            .unwrap_or(0.0)
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN)
            .clamp(MIN_WIDTH.min(self.bounds.width()), MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let height = HEADER_H + COLUMNS_H + MOD_SLOTS as f64 * ROW_H + FOOT_H;
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + height.min(room))
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 80.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H,
        )
    }

    fn row_top(&self, slot: usize) -> f64 {
        self.panel().y0 + HEADER_H + COLUMNS_H + slot as f64 * ROW_H
    }

    /// The picker button of `field` in row `slot`.
    pub fn picker_rect(&self, slot: usize, field: char) -> Rect {
        let p = self.panel();
        let index = match Field::from_tag(field) {
            Some(Field::Source) | None => 0.0,
            Some(Field::Target) => 1.0,
            Some(Field::Via) => 2.0,
        };
        let x = p.x0 + PAD + NUM_W + index * (PICK_W + GAP);
        let y = self.row_top(slot) + (ROW_H - BTN_H) / 2.0;
        Rect::new(x, y, x + PICK_W, y + BTN_H)
    }

    /// The amount bar of row `slot`.
    pub fn bar_rect(&self, slot: usize) -> Rect {
        let p = self.panel();
        let x = p.x0 + PAD + NUM_W + 3.0 * (PICK_W + GAP);
        let right = p.x1 - PAD - CLEAR_W - GAP - 52.0;
        let y = self.row_top(slot) + (ROW_H - BTN_H) / 2.0;
        Rect::new(x, y, right.max(x + 40.0), y + BTN_H)
    }

    /// The cross that clears row `slot`.
    pub fn clear_rect(&self, slot: usize) -> Rect {
        let p = self.panel();
        let y = self.row_top(slot) + (ROW_H - BTN_H) / 2.0;
        Rect::new(p.x1 - PAD - CLEAR_W, y, p.x1 - PAD, y + BTN_H)
    }

    // ------------------------------------------------------------------ commands

    fn merge_id(&mut self) -> u64 {
        let m = self.next_merge;
        self.next_merge += 1;
        m
    }

    fn slot_command(&self, slot: usize, value: Option<ModSlot>) -> Option<UiAction> {
        let info = self.info.as_ref()?;
        if info.slots.get(slot).copied().flatten() == value {
            return None;
        }
        Some(UiAction::Command(Command::Workflow(
            WorkflowCommand::SetModSlot {
                device: info.device.clone(),
                slot: slot as u8,
                value,
            },
        )))
    }

    fn amount_command(&self, slot: usize, value: f32, merge: Option<u64>) -> Option<UiAction> {
        let info = self.info.as_ref()?;
        let value = value.clamp(-1.0, 1.0);
        if value.to_bits() == self.amount(slot).to_bits() {
            return None;
        }
        Some(UiAction::Command(Command::SetParam {
            key: info.device.clone(),
            param: mod_param_key(slot, ModField::Amount),
            value,
            merge,
        }))
    }

    /// A choice made in a picker: `choice` 0 is none, `n` is entry `n - 1`.
    fn choose(&mut self, slot: usize, field: Field, choice: usize) -> Vec<UiAction> {
        let Some(draft) = self.drafts.get_mut(slot) else {
            return Vec::new();
        };
        let id = choice.checked_sub(1).map(|n| n as u16);
        match field {
            Field::Source => draft.source = id,
            Field::Target => draft.target = id,
            Field::Via => draft.via = id,
        }
        let value = draft.slot();
        self.repaint = true;
        // A draft that is not a slot yet changes nothing in the document, unless it was one.
        if value.is_none() && !self.in_use(slot) {
            return Vec::new();
        }
        self.slot_command(slot, value).into_iter().collect()
    }

    fn clear(&mut self, slot: usize) -> Vec<UiAction> {
        if let Some(d) = self.drafts.get_mut(slot) {
            *d = Draft::default();
        }
        self.repaint = true;
        self.slot_command(slot, None).into_iter().collect()
    }

    fn open_picker(&mut self, slot: usize, field: Field) {
        let Some(info) = &self.info else { return };
        let names = match field {
            Field::Target => &info.targets,
            _ => &info.sources,
        };
        let draft = self.drafts[slot];
        let current = match field {
            Field::Source => draft.source,
            Field::Target => draft.target,
            Field::Via => draft.via,
        };
        let mut entries = Vec::with_capacity(names.len() + 1);
        let mut row = |choice: usize, label: &str, on: bool| {
            entries.push(Entry::Item(MenuItemDef {
                command: PICK_COMMAND.to_owned(),
                arg: Some(format!("{}{slot}:{choice}", field.tag())),
                label: label.to_owned(),
                shortcut: None,
                enabled: true,
                checked: Some(on),
                swatch: None,
            }));
        };
        row(0, "None", current.is_none());
        for (i, name) in names.iter().enumerate() {
            row(i + 1, name, current == Some(i as u16));
        }
        let r = self.picker_rect(slot, field.tag());
        let at = Point::new(r.x0, r.y1 + 1.0);
        let size = Size::new(self.bounds.x1, self.bounds.y1);
        self.picker = Some(Picker {
            menu: ContextMenu::from_entries(entries, at, size),
        });
        self.repaint = true;
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.picker.is_some() {
            return None;
        }
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the matrix");
        }
        for slot in 0..MOD_SLOTS {
            for (tag, text) in [
                ('s', "What moves the target: a source of the device"),
                ('t', "What the source moves: a parameter of the device"),
                (
                    'v',
                    "A source that scales the amount (a mod wheel that fades an LFO in)",
                ),
            ] {
                let r = self.picker_rect(slot, tag);
                if r.contains(pos) {
                    return help(r, text);
                }
            }
            let bar = self.bar_rect(slot);
            if bar.contains(pos) {
                return help(
                    bar,
                    "How far the source moves the target, -100 to +100 %; right click: 0",
                );
            }
            let x = self.clear_rect(slot);
            if x.contains(pos) {
                return help(x, "Clear this slot");
            }
        }
        None
    }

    fn amount_at(&self, slot: usize, x: f64) -> f32 {
        let bar = self.bar_rect(slot);
        let t = ((x - bar.x0) / bar.width()).clamp(0.0, 1.0);
        let v = (t * 2.0 - 1.0) as f32;
        // The middle snaps to 0 so a slot can be left at exactly nothing.
        if v.abs() < 0.02 { 0.0 } else { v }
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        self.repaint = true;
        if let Some(picker) = &mut self.picker {
            if picker.menu.wants_close(event) {
                self.picker = None;
                // A press outside also lands on what is under it, except the panel's own
                // background, so a second click is not needed to reach a button.
                if !matches!(event, UiEvent::PointerDown { .. }) {
                    return out;
                }
            } else {
                if let MenuOutcome::Run { command, arg } = picker.menu.handle(event) {
                    self.picker = None;
                    if command == PICK_COMMAND
                        && let Some((field, slot, choice)) = arg.as_deref().and_then(parse_pick)
                    {
                        out.extend(self.choose(slot, field, choice));
                    }
                }
                return out;
            }
        }
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                if let Some(d) = self.drag {
                    let v = self.amount_at(d.slot, pos.x);
                    out.extend(self.amount_command(d.slot, v, Some(d.merge)));
                }
            }
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => {
                if self.drag.take().is_some() {
                    out.push(UiAction::EndMerge);
                }
            }
            UiEvent::PointerDown {
                pos,
                button,
                clicks,
                ..
            } => out.extend(self.press(*pos, *button, *clicks)),
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => {
                if let Some(slot) = (0..MOD_SLOTS).find(|s| self.bar_rect(*s).contains(*pos))
                    && self.in_use(slot)
                {
                    let step = if modifiers.shift {
                        BAR_STEP_FINE
                    } else {
                        BAR_STEP
                    };
                    let steps = delta.y.signum() as f32;
                    let now = self.amount(slot);
                    out.extend(self.amount_command(slot, now + steps * step, None));
                }
            }
            UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => out.push(UiAction::ModMatrix(ModMatrixAction::Close)),
            _ => {}
        }
        out
    }

    fn press(&mut self, pos: Point, button: PointerButton, clicks: u8) -> Vec<UiAction> {
        if self.close_rect().contains(pos) && button == PointerButton::Primary {
            return vec![UiAction::ModMatrix(ModMatrixAction::Close)];
        }
        for slot in 0..MOD_SLOTS {
            if button == PointerButton::Primary {
                for field in [Field::Source, Field::Target, Field::Via] {
                    if self.picker_rect(slot, field.tag()).contains(pos) {
                        self.open_picker(slot, field);
                        return Vec::new();
                    }
                }
                if self.clear_rect(slot).contains(pos) {
                    return self.clear(slot);
                }
            }
            if self.bar_rect(slot).contains(pos) && self.in_use(slot) {
                if button == PointerButton::Secondary || clicks >= 2 {
                    return self.amount_command(slot, 0.0, None).into_iter().collect();
                }
                if button == PointerButton::Primary {
                    let merge = self.merge_id();
                    self.drag = Some(AmountDrag { slot, merge });
                    let v = self.amount_at(slot, pos.x);
                    return self
                        .amount_command(slot, v, Some(merge))
                        .into_iter()
                        .collect();
                }
            }
        }
        Vec::new()
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
        let name = self.info.as_ref().map_or("", |i| i.name.as_str());
        sp::label(
            c,
            &fit_label(
                &format!("MODULATION MATRIX  {name}"),
                p.width() - 2.0 * PAD - 100.0,
                12.0,
            ),
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        let head_y = p.y0 + HEADER_H + 14.0;
        for (i, text) in ["SOURCE", "TARGET", "VIA", "AMOUNT"]
            .into_iter()
            .enumerate()
        {
            let x = p.x0 + PAD + NUM_W + i as f64 * (PICK_W + GAP);
            sp::label(c, text, x + 2.0, head_y, 9.0, t.text_muted, Align::Start);
        }
        for slot in 0..MOD_SLOTS {
            self.paint_row(c, slot, &hovered);
        }
        sp::label(
            c,
            &fit_label(
                "A slot needs a source and a target; the amount bar works once it is in use.",
                p.width() - 2.0 * PAD,
                9.5,
            ),
            p.x0 + PAD,
            p.y1 - 11.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        if let Some(picker) = &self.picker {
            picker.menu.paint(c, t);
        }
        c.pop_transform();
    }

    fn paint_row(&self, c: &mut dyn Canvas, slot: usize, hovered: &dyn Fn(Rect) -> bool) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let top = self.row_top(slot);
        let used = self.in_use(slot);
        sp::label(
            c,
            &(slot + 1).to_string(),
            p.x0 + PAD,
            top + ROW_H / 2.0 + 3.5,
            10.0,
            t.text_muted,
            Align::Start,
        );
        let draft = self.drafts[slot];
        let empty = Vec::new();
        let (sources, targets) = self
            .info
            .as_ref()
            .map_or((&empty, &empty), |i| (&i.sources, &i.targets));
        for (field, names, id) in [
            (Field::Source, sources, draft.source),
            (Field::Target, targets, draft.target),
            (Field::Via, sources, draft.via),
        ] {
            let r = self.picker_rect(slot, field.tag());
            let text = id
                .and_then(|i| names.get(usize::from(i)))
                .map_or("None", String::as_str);
            let lit = id.is_some().then_some(st.loop_on);
            sp::button(c, st, r, "", lit, hovered(r));
            sp::label(
                c,
                &fit_label(text, r.width() - 22.0, 9.5),
                r.x0 + 8.0,
                r.center().y + 3.5,
                9.5,
                if id.is_some() { t.on_accent } else { t.text },
                Align::Start,
            );
            sp::label(
                c,
                "\u{25be}",
                r.x1 - 10.0,
                r.center().y + 3.5,
                9.0,
                t.text_muted,
                Align::Center,
            );
        }
        let bar = self.bar_rect(slot);
        sp::lcd(c, st, bar, None);
        let amount = self.amount(slot);
        let mid = bar.center().x;
        let x = mid + f64::from(amount) * bar.width() / 2.0;
        if used {
            let (a, b) = if x < mid { (x, mid) } else { (mid, x) };
            sp::rect(
                c,
                Rect::new(a, bar.y0 + 3.0, b.max(a + 0.5), bar.y1 - 3.0),
                st.loop_on,
            );
        }
        sp::line(c, mid, bar.y0 + 2.0, mid, bar.y1 - 2.0, t.text_muted);
        sp::label(
            c,
            &if used {
                format!("{:+.0} %", amount * 100.0)
            } else {
                String::new()
            },
            bar.x1 + 8.0,
            bar.center().y + 3.5,
            9.5,
            t.text,
            Align::Start,
        );
        let x_rect = self.clear_rect(slot);
        if used || draft != Draft::default() {
            sp::button(c, st, x_rect, "x", None, hovered(x_rect));
        }
    }
}

/// `s3:5` to the field, the slot and the choice.
fn parse_pick(arg: &str) -> Option<(Field, usize, usize)> {
    let mut chars = arg.chars();
    let field = Field::from_tag(chars.next()?)?;
    let (slot, choice) = chars.as_str().split_once(':')?;
    let slot: usize = slot.parse().ok()?;
    (slot < MOD_SLOTS).then_some(())?;
    Some((field, slot, choice.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Modifiers;

    fn info(slots: Vec<Option<ModSlot>>, amounts: Vec<f32>) -> ModMatrixInfo {
        ModMatrixInfo {
            device: DeviceKey("synth".into()),
            name: "Synth".into(),
            sources: vec!["LFO 1".into(), "Envelope".into(), "Velocity".into()],
            targets: vec!["Cutoff".into(), "Pitch".into()],
            slots,
            amounts,
        }
    }

    fn view(info: ModMatrixInfo) -> ModMatrixView {
        let mut v = ModMatrixView::new(Theme::default());
        v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 700.0));
        v.set_info(info);
        v
    }

    fn click(v: &mut ModMatrixView, at: Point) -> Vec<UiAction> {
        let mut out = v.handle(&UiEvent::PointerDown {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        });
        out.extend(v.handle(&UiEvent::PointerUp {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        }));
        out
    }

    /// Opens the picker of a field and chooses the row labelled `label`.
    fn pick(v: &mut ModMatrixView, slot: usize, field: char, label: &str) -> Vec<UiAction> {
        let r = v.picker_rect(slot, field);
        click(v, r.center());
        let menu = &v.picker.as_ref().expect("the picker opens").menu;
        let at = menu
            .all_items()
            .iter()
            .position(|i| i.label == label)
            .and_then(|n| menu.row_rects().get(n).map(|r| r.center()))
            .expect("the entry");
        click(v, at)
    }

    fn slot_cmd(actions: &[UiAction]) -> Option<(u8, Option<ModSlot>)> {
        actions.iter().find_map(|a| match a {
            UiAction::Command(Command::Workflow(WorkflowCommand::SetModSlot {
                slot,
                value,
                ..
            })) => Some((*slot, *value)),
            _ => None,
        })
    }

    #[test]
    fn a_slot_needs_both_a_source_and_a_target() {
        let mut v = view(info(vec![], vec![]));
        let a = pick(&mut v, 0, 's', "Envelope");
        assert!(slot_cmd(&a).is_none(), "a source alone is a draft: {a:?}");
        assert_eq!(v.row(0), Some((Some(1), None, None)));
        let a = pick(&mut v, 0, 't', "Pitch");
        assert_eq!(slot_cmd(&a), Some((0, Some(ModSlot::new(1, 1)))));
    }

    #[test]
    fn a_via_is_part_of_the_slot_and_clearing_a_side_clears_the_slot() {
        let slot = Some(ModSlot::new(0, 0));
        let mut v = view(info(vec![slot], vec![0.5]));
        let a = pick(&mut v, 0, 'v', "Velocity");
        assert_eq!(slot_cmd(&a), Some((0, Some(ModSlot::new(0, 0).via(2)))));
        let a = pick(&mut v, 0, 's', "None");
        assert_eq!(slot_cmd(&a), Some((0, None)), "no source: no slot");
    }

    #[test]
    fn choosing_what_a_slot_already_holds_is_not_a_command() {
        let mut v = view(info(vec![Some(ModSlot::new(0, 0))], vec![0.0]));
        assert!(pick(&mut v, 0, 's', "LFO 1").is_empty());
    }

    #[test]
    fn the_amount_bar_drags_as_one_gesture_and_resets_on_a_right_click() {
        let mut v = view(info(vec![Some(ModSlot::new(0, 0))], vec![0.0]));
        let bar = v.bar_rect(0);
        let at = |t: f64| Point::new(bar.x0 + bar.width() * t, bar.center().y);
        let down = v.handle(&UiEvent::PointerDown {
            pos: at(0.75),
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        });
        let merge = |a: &[UiAction]| match a.first() {
            Some(UiAction::Command(Command::SetParam {
                param,
                value,
                merge,
                ..
            })) => Some((param.clone(), *value, *merge)),
            _ => None,
        };
        let (param, value, m1) = merge(&down).expect("a set");
        assert_eq!(param, "mod1_amount");
        assert!((value - 0.5).abs() < 1e-4, "{value}");
        let drag = v.handle(&UiEvent::PointerMove {
            pos: at(1.0),
            modifiers: Modifiers::NONE,
        });
        let (_, value, m2) = merge(&drag).expect("a move");
        assert_eq!((value, m2), (1.0, m1), "one merge id for the gesture");
        let up = v.handle(&UiEvent::PointerUp {
            pos: at(1.0),
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        });
        assert_eq!(up, vec![UiAction::EndMerge]);
    }

    #[test]
    fn the_amount_of_a_free_slot_cannot_be_dragged() {
        let mut v = view(info(vec![], vec![]));
        let bar = v.bar_rect(0);
        assert!(click(&mut v, bar.center()).is_empty());
    }

    #[test]
    fn the_cross_clears_a_slot_and_a_draft_survives_a_refresh() {
        let mut v = view(info(vec![Some(ModSlot::new(0, 1))], vec![0.3]));
        let at = v.clear_rect(0).center();
        let a = click(&mut v, at);
        assert_eq!(slot_cmd(&a), Some((0, None)));
        pick(&mut v, 1, 's', "Velocity");
        v.set_info(info(vec![], vec![]));
        assert_eq!(v.row(1), Some((Some(2), None, None)), "the draft stays");
        assert_eq!(v.row(0), Some((None, None, None)));
    }

    #[test]
    fn escape_closes_the_menu_first_and_then_the_panel() {
        let mut v = view(info(vec![], vec![]));
        let at = v.picker_rect(0, 't').center();
        click(&mut v, at);
        assert!(v.picker.is_some());
        let esc = UiEvent::Key {
            key: Key::Escape,
            modifiers: Modifiers::NONE,
            pressed: true,
        };
        assert!(v.handle(&esc).is_empty() && v.picker.is_none());
        assert_eq!(
            v.handle(&esc),
            vec![UiAction::ModMatrix(ModMatrixAction::Close)]
        );
    }

    #[test]
    fn pick_arguments_round_trip() {
        assert_eq!(parse_pick("t3:7"), Some((Field::Target, 3, 7)));
        assert_eq!(parse_pick("x3:7"), None);
        assert_eq!(parse_pick("s9:1"), None);
    }
}
