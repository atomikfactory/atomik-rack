//! The container editor: a pane that replaces the arrangement and edits the
//! note splits and the macros of one container.
//!
//! Hosting follows [`crate::SeqEditorView`]: on [`UiAction::OpenContainerEditor`] call
//! [`ContainerEditorView::open`] (and [`ContainerEditorView::set_tab`] /
//! [`ContainerEditorView::select_macro`] when `macros` is `Some`) and show the view in place of
//! the arrangement; on [`UiAction::CloseContainerEditor`] show the arrangement again (also when
//! [`ContainerEditorView::is_present`] turns false because the container was deleted). Every
//! edit arrives as a `Command::Container` (`SetSplits`, `SetMacro`, `RemoveMacroTarget`,
//! `RenameMacro`) or a `SetParam` of the container's `macroN`; drags use merge ids from 7 * 2^40
//! and end with [`UiAction::EndMerge`]; previews are [`UiAction::AuditionVelocity`] on the
//! container (the app plays it through the splits). Container and inner device keys are the
//! **path keys** the model uses (`layer`, `layer/fx`); inside the commands the inner devices are
//! addressed relative to the container.
//!
//! # SPLITS tab
//!
//! ```text
//! +--map: key 0..127 across, velocity 127..1 up---------------------------+
//! |        [==== synth ====]                                               |
//! |                        [===== pad =====]                               |
//! +------------------------------------------------------------------------+
//! |  keyboard strip (press to audition through the splits)                 |
//! +------------------------------------------------------------------------+
//! SPLITS                                                              [ADD]
//! (o) [ synth        ]  C-1 - B3     vel 1-127    [-] +0 [+]  [x]
//! (o) [ pad          ]  C4 - G9      vel 1-127    [-] +12 [+] [x]
//! ```
//!
//! * **Map.** One coloured bar per split. Drag its left or right edge for `key_lo` or `key_hi`,
//!   its top or bottom edge for `vel_hi` or `vel_lo`, its body to move the whole zone. A press
//!   on empty map space auditions the key at the clicked velocity.
//! * **List.** The target selector steps to the next free device with a Notes input; the
//!   stepper changes the transpose by 1 (Shift: 12) and the wheel steps it too; `x` deletes;
//!   ADD adds a full-range split to the next free device (a device takes one split only).
//!
//! # MACROS tab
//!
//! The eight macros as rows (name, value knob, target count) and, for the selected one, its
//! target list: device and param, `min` and `max` fields (drag, wheel, double-click resets to
//! the param's range) and a delete button. A double-click on a name renames it in place (Enter
//! commits, Esc cancels). Mapping a param is done from the container's inside (the right-click
//! menu of a control, see [`crate::RackView`]).
//!
//! # Gestures
//!
//! Every drag is one merged undo step ended with [`UiAction::EndMerge`]; Esc during a drag
//! restores the state of the press. Esc with nothing to cancel and the close button emit
//! [`UiAction::CloseContainerEditor`]; Ctrl/Cmd+Z / Y undo and redo.

use crate::action::UiAction;
use crate::canvas::Canvas;
use crate::container_edit::{self as ce, Bound, Edge};
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::seq_paint::SeqTheme;
use crate::seq_panel::{self, RenameKey};
use crate::theme::Theme;
use crate::tooltip::Help;
use crate::widget::{self, ValueDrag};
use atomik_core::ParamDescriptor;
use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ContainerCommand, ContainerDoc, DeviceKey, MACRO_COUNT, MAX_MACRO_NAME_CHARS,
    MacroTarget, NoteSplit, RackDoc,
};
use vello::kurbo::{Point, Rect};
use vello::peniko::Color;

mod paint;

/// Height of the top bar.
pub const TOP_BAR_HEIGHT: f64 = 28.0;
/// Height of the tab row.
pub const TAB_HEIGHT: f64 = 28.0;
/// Height of the key and velocity map.
pub const MAP_HEIGHT: f64 = 190.0;
/// Height of the keyboard strip under the map.
pub const KEYS_HEIGHT: f64 = 36.0;
/// Height of one split row.
pub const ROW_HEIGHT: f64 = 26.0;
/// Height of one macro row.
pub const MACRO_ROW_HEIGHT: f64 = 32.0;
/// Height of one target row.
pub const TARGET_ROW_HEIGHT: f64 = 26.0;
/// Width of the velocity labels left of the map.
const LABEL_W: f64 = 56.0;
const PAD: f64 = 14.0;
/// Pointer travel that picks an edge of a bar.
const EDGE_PX: f64 = 5.0;
const MERGE_BASE: u64 = 7 << 40;
/// Velocity of the audition from the keyboard strip.
pub const STRIP_VELOCITY: u8 = 100;
const KNOB_PIXELS: f64 = 150.0;

/// The tabs of the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerTab {
    /// The note splits.
    Splits,
    /// The macros.
    Macros,
}

impl ContainerTab {
    const ALL: [Self; 2] = [Self::Splits, Self::Macros];

    fn label(self) -> &'static str {
        match self {
            Self::Splits => "SPLITS",
            Self::Macros => "MACROS",
        }
    }
}

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Nothing.
    None,
    /// The close button.
    Close,
    /// A tab.
    Tab(ContainerTab),
    /// A split bar of the map (split index, part).
    Bar(usize, Edge),
    /// Empty map space.
    Map,
    /// The keyboard strip (key).
    Keys(u8),
    /// The ADD button.
    Add,
    /// A split row's colour chip or free space (selects the split).
    Row(usize),
    /// A split row's target selector.
    Target(usize),
    /// A split row's transpose minus button.
    TransposeDec(usize),
    /// A split row's transpose plus button.
    TransposeInc(usize),
    /// A split row's delete button.
    Delete(usize),
    /// A macro row (selects the macro).
    MacroRow(usize),
    /// A macro's name.
    MacroName(usize),
    /// A macro's value knob.
    MacroKnob(usize),
    /// The `min` or `max` field of a target row (row, end).
    Field(usize, Bound),
    /// A target row's delete button.
    TargetDelete(usize),
}

/// One inner device as the editor lists it.
#[derive(Debug, Clone, PartialEq)]
struct DevInfo {
    key: DeviceKey,
    name: String,
    accent: Color,
}

/// One macro target resolved for display.
#[derive(Debug, Clone, PartialEq)]
struct TargetInfo {
    target: MacroTarget,
    device_name: String,
    param: ParamDescriptor,
}

#[derive(Debug, Clone)]
struct SplitDrag {
    index: usize,
    edge: Edge,
    base: Vec<NoteSplit>,
    press: Point,
    merge: u64,
    emitted: bool,
}

#[derive(Debug, Clone)]
struct KnobDrag {
    index: usize,
    drag: ValueDrag,
    merge: u64,
    last_y: f64,
    start: f32,
    value: f32,
}

#[derive(Debug, Clone)]
struct BoundDrag {
    macro_index: usize,
    row: usize,
    bound: Bound,
    base: MacroTarget,
    start: f32,
    press_y: f64,
    merge: u64,
    emitted: bool,
}

#[derive(Debug, Clone)]
enum Drag {
    None,
    /// Auditioning a key (strip or map held).
    Audition(u8),
    Split(Box<SplitDrag>),
    Knob(Box<KnobDrag>),
    Bound(Box<BoundDrag>),
}

/// The container editor. See the module documentation.
#[derive(Debug)]
pub struct ContainerEditorView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    container: Option<DeviceKey>,
    present: bool,
    name: String,
    accent: Color,
    tab: ContainerTab,
    inner: ContainerDoc,
    devices: Vec<DevInfo>,
    /// The container's own `macroN` values.
    macro_values: [f32; MACRO_COUNT],
    macro_params: [Option<ParamDescriptor>; MACRO_COUNT],
    targets: Vec<Vec<TargetInfo>>,
    /// The inner devices a new split may target (see [`ce::eligible_targets`]).
    eligible: Vec<DeviceKey>,
    selected_split: Option<usize>,
    selected_macro: usize,
    scroll: f64,
    rename: Option<String>,
    drag: Drag,
    hover: Hit,
    next_merge: u64,
    focused: bool,
    repaint: bool,
}

impl ContainerEditorView {
    /// A closed editor; call [`Self::set_bounds`], [`Self::open`] and [`Self::sync`].
    pub fn new(theme: Theme) -> Self {
        let st = SeqTheme::from_theme(&theme);
        let accent = st.base.highlight;
        Self {
            st,
            bounds: Rect::new(0.0, 0.0, 1000.0, 560.0),
            scale: 1.0,
            container: None,
            present: false,
            name: String::new(),
            accent,
            tab: ContainerTab::Splits,
            inner: ContainerDoc::default(),
            devices: Vec::new(),
            macro_values: [0.0; MACRO_COUNT],
            macro_params: [None; MACRO_COUNT],
            targets: vec![Vec::new(); MACRO_COUNT],
            eligible: Vec::new(),
            selected_split: None,
            selected_macro: 0,
            scroll: 0.0,
            rename: None,
            drag: Drag::None,
            hover: Hit::None,
            next_merge: MERGE_BASE,
            focused: false,
            repaint: true,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Opens the editor on a container (its path key), on the SPLITS tab.
    pub fn open(&mut self, container: DeviceKey) {
        if self.container.as_ref() != Some(&container) {
            self.selected_split = None;
            self.selected_macro = 0;
            self.scroll = 0.0;
            self.inner = ContainerDoc::default();
            self.devices.clear();
        }
        self.container = Some(container);
        self.tab = ContainerTab::Splits;
        self.present = false;
        self.drag = Drag::None;
        self.rename = None;
        self.repaint = true;
    }

    /// The container being edited (path key).
    pub fn container(&self) -> Option<&DeviceKey> {
        self.container.as_ref()
    }

    /// True while the container exists in the document (as of the last sync).
    pub fn is_present(&self) -> bool {
        self.present
    }

    /// Places the view in the window (logical pixels).
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// The view's rectangle.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Marks the view as focused (thin top line).
    pub fn set_focused(&mut self, focused: bool) {
        if focused != self.focused {
            self.focused = focused;
            self.repaint = true;
        }
    }

    /// True while a pointer gesture is in progress (keep routing pointer events here).
    pub fn is_dragging(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// The shown tab.
    pub fn tab(&self) -> ContainerTab {
        self.tab
    }

    /// Shows a tab.
    pub fn set_tab(&mut self, tab: ContainerTab) {
        if self.tab != tab {
            self.tab = tab;
            self.drag = Drag::None;
            self.rename = None;
            self.repaint = true;
        }
    }

    /// The selected macro (0-based).
    pub fn selected_macro(&self) -> usize {
        self.selected_macro
    }

    /// Selects a macro of the MACROS tab (0-based, clamped).
    pub fn select_macro(&mut self, index: usize) {
        let index = index.min(MACRO_COUNT - 1);
        if index != self.selected_macro {
            self.selected_macro = index;
            self.scroll = 0.0;
            self.rename = None;
            self.repaint = true;
        }
    }

    /// The selected split (0-based).
    pub fn selected_split(&self) -> Option<usize> {
        self.selected_split
    }

    /// The splits as last synced.
    pub fn splits(&self) -> &[NoteSplit] {
        &self.inner.splits
    }

    /// The macro name being edited in place.
    pub fn renaming(&self) -> Option<&str> {
        self.rename.as_deref()
    }

    /// Whether the view owns the keyboard as a text field (a macro name is being edited): the
    /// shell then routes plain keys to it instead of to commands.
    pub fn is_text_owner(&self) -> bool {
        self.rename.is_some()
    }

    // ------------------------------------------------------------------ geometry

    fn body(&self) -> Rect {
        let b = self.bounds;
        Rect::new(
            b.x0,
            b.y0 + TOP_BAR_HEIGHT + TAB_HEIGHT,
            b.x1,
            b.y1.max(b.y0 + TOP_BAR_HEIGHT + TAB_HEIGHT),
        )
    }

    /// The close button.
    pub fn close_rect(&self) -> Rect {
        let b = self.bounds;
        Rect::new(b.x1 - 28.0, b.y0 + 4.0, b.x1 - 8.0, b.y0 + 24.0)
    }

    /// A tab's rectangle.
    pub fn tab_rect(&self, tab: ContainerTab) -> Rect {
        let i = ContainerTab::ALL
            .iter()
            .position(|t| *t == tab)
            .unwrap_or(0) as f64;
        let x = self.bounds.x0 + PAD + i * 86.0;
        let y = self.bounds.y0 + TOP_BAR_HEIGHT + 3.0;
        Rect::new(x, y, x + 80.0, y + TAB_HEIGHT - 6.0)
    }

    /// The key and velocity map of the SPLITS tab.
    pub fn map_rect(&self) -> Rect {
        let body = self.body();
        Rect::new(
            body.x0 + LABEL_W + PAD,
            body.y0 + 12.0,
            body.x1 - PAD,
            body.y0 + 12.0 + MAP_HEIGHT,
        )
    }

    /// The keyboard strip under the map.
    pub fn keys_rect(&self) -> Rect {
        let m = self.map_rect();
        Rect::new(m.x0, m.y1 + 2.0, m.x1, m.y1 + 2.0 + KEYS_HEIGHT)
    }

    fn list_top(&self) -> f64 {
        self.keys_rect().y1 + 28.0
    }

    /// The ADD button.
    pub fn add_rect(&self) -> Rect {
        let top = self.list_top();
        Rect::new(
            self.bounds.x1 - PAD - 60.0,
            top - 4.0,
            self.bounds.x1 - PAD,
            top + 16.0,
        )
    }

    /// Row `i` of the split list.
    pub fn row_rect(&self, i: usize) -> Rect {
        let y = self.list_top() + 22.0 + i as f64 * ROW_HEIGHT;
        Rect::new(
            self.bounds.x0 + PAD,
            y,
            self.bounds.x1 - PAD,
            y + ROW_HEIGHT - 2.0,
        )
    }

    /// The target selector of split row `i`.
    pub fn target_rect(&self, i: usize) -> Rect {
        let r = self.row_rect(i);
        Rect::new(r.x0 + 22.0, r.y0 + 2.0, r.x0 + 192.0, r.y1 - 2.0)
    }

    /// The transpose minus button of split row `i`.
    pub fn dec_rect(&self, i: usize) -> Rect {
        let r = self.row_rect(i);
        Rect::new(r.x0 + 470.0, r.y0 + 2.0, r.x0 + 490.0, r.y1 - 2.0)
    }

    /// The transpose value window of split row `i`.
    pub fn transpose_rect(&self, i: usize) -> Rect {
        let r = self.row_rect(i);
        Rect::new(r.x0 + 492.0, r.y0 + 2.0, r.x0 + 544.0, r.y1 - 2.0)
    }

    /// The transpose plus button of split row `i`.
    pub fn inc_rect(&self, i: usize) -> Rect {
        let r = self.row_rect(i);
        Rect::new(r.x0 + 546.0, r.y0 + 2.0, r.x0 + 566.0, r.y1 - 2.0)
    }

    /// The delete button of split row `i`.
    pub fn delete_rect(&self, i: usize) -> Rect {
        let r = self.row_rect(i);
        Rect::new(r.x0 + 580.0, r.y0 + 2.0, r.x0 + 600.0, r.y1 - 2.0)
    }

    fn key_x(&self, key: i32) -> f64 {
        let m = self.map_rect();
        m.x0 + f64::from(key) * m.width() / 128.0
    }

    fn vel_unit(&self) -> f64 {
        self.map_rect().height() / 127.0
    }

    /// The rectangle of split `i`'s bar on the map.
    pub fn bar_rect(&self, i: usize) -> Option<Rect> {
        let s = self.inner.splits.get(i)?;
        let m = self.map_rect();
        let u = self.vel_unit();
        Some(Rect::new(
            self.key_x(i32::from(s.key_lo)),
            m.y1 - f64::from(s.vel_hi) * u,
            self.key_x(i32::from(s.key_hi) + 1),
            m.y1 - f64::from(s.vel_lo - 1) * u,
        ))
    }

    /// The map key under an x position.
    fn key_at(&self, x: f64) -> u8 {
        let m = self.map_rect();
        (((x - m.x0) / (m.width() / 128.0)).floor().clamp(0.0, 127.0)) as u8
    }

    /// The map velocity under a y position.
    fn vel_at(&self, y: f64) -> u8 {
        let m = self.map_rect();
        (((m.y1 - y) / self.vel_unit()).ceil().clamp(1.0, 127.0)) as u8
    }

    /// Centre of the keyboard strip's key (for tests and hosts).
    pub fn key_center(&self, key: u8) -> Point {
        let k = self.keys_rect();
        Point::new(
            (self.key_x(i32::from(key)) + self.key_x(i32::from(key) + 1)) / 2.0,
            k.center().y,
        )
    }

    /// Row `i` of the macro list.
    pub fn macro_row_rect(&self, i: usize) -> Rect {
        let body = self.body();
        let y = body.y0 + 12.0 + i as f64 * MACRO_ROW_HEIGHT;
        Rect::new(
            body.x0 + PAD,
            y,
            body.x0 + PAD + 340.0,
            y + MACRO_ROW_HEIGHT - 2.0,
        )
    }

    /// The name field of macro row `i`.
    pub fn macro_name_rect(&self, i: usize) -> Rect {
        let r = self.macro_row_rect(i);
        Rect::new(r.x0 + 26.0, r.y0 + 3.0, r.x0 + 196.0, r.y1 - 3.0)
    }

    /// The knob cell of macro row `i`.
    pub fn macro_knob_rect(&self, i: usize) -> Rect {
        let r = self.macro_row_rect(i);
        Rect::new(r.x0 + 206.0, r.y0 + 1.0, r.x0 + 236.0, r.y1 - 1.0)
    }

    fn targets_x(&self) -> f64 {
        self.body().x0 + PAD + 364.0
    }

    /// Row `j` of the selected macro's target list (scrolled).
    pub fn target_row_rect(&self, j: usize) -> Rect {
        let body = self.body();
        let y = body.y0 + 40.0 + j as f64 * TARGET_ROW_HEIGHT - self.scroll;
        Rect::new(
            self.targets_x(),
            y,
            self.bounds.x1 - PAD,
            y + TARGET_ROW_HEIGHT - 2.0,
        )
    }

    /// The `min` or `max` field of target row `j`.
    pub fn field_rect(&self, j: usize, bound: Bound) -> Rect {
        let r = self.target_row_rect(j);
        let x = match bound {
            Bound::Min => r.x1 - 200.0,
            Bound::Max => r.x1 - 124.0,
        };
        Rect::new(x, r.y0 + 2.0, x + 72.0, r.y1 - 2.0)
    }

    /// The delete button of target row `j`.
    pub fn target_delete_rect(&self, j: usize) -> Rect {
        let r = self.target_row_rect(j);
        Rect::new(r.x1 - 40.0, r.y0 + 2.0, r.x1 - 20.0, r.y1 - 2.0)
    }

    fn targets_view(&self) -> Rect {
        let body = self.body();
        Rect::new(
            self.targets_x(),
            body.y0 + 38.0,
            self.bounds.x1 - PAD,
            self.bounds.y1.max(body.y0 + 40.0),
        )
    }

    fn max_scroll(&self) -> f64 {
        let n = self.targets.get(self.selected_macro).map_or(0, Vec::len);
        (n as f64 * TARGET_ROW_HEIGHT - self.targets_view().height()).max(0.0)
    }

    // ------------------------------------------------------------------ sync

    /// Reads the container from the document: its devices, splits, macros and the container's
    /// own macro values. A container that is gone makes the editor not present.
    pub fn sync(&mut self, doc: &RackDoc, registry: &Registry) {
        let Some(key) = &self.container else { return };
        let Some(inner) = doc.inner(key) else {
            if self.present || !self.devices.is_empty() {
                self.present = false;
                self.devices.clear();
                self.drag = Drag::None;
                self.rename = None;
                self.repaint = true;
            }
            return;
        };
        let dev = doc.resolve(key);
        let mut changed = !self.present;
        self.present = true;
        let name = dev.map_or_else(String::new, |d| d.name.clone());
        if name != self.name {
            self.name = name;
            changed = true;
        }
        if let Some(d) = dev {
            self.accent = self.st.base.accent_for(d);
            let desc = registry.get(&d.type_id).map(|f| f.descriptor);
            for i in 0..MACRO_COUNT {
                let p = desc.and_then(|x| x.param_by_key(&format!("macro{}", i + 1)).copied());
                let v = p.map_or(0.0, |p| widget::current_value(d, &p));
                if p != self.macro_params[i] || v.to_bits() != self.macro_values[i].to_bits() {
                    self.macro_params[i] = p;
                    self.macro_values[i] = v;
                    changed = true;
                }
            }
        }
        if *inner != self.inner {
            self.inner = inner.clone();
            changed = true;
        }
        let devices: Vec<DevInfo> = inner
            .devices
            .iter()
            .map(|d| DevInfo {
                key: d.key.clone(),
                name: d.name.clone(),
                accent: self.st.base.accent_for(d),
            })
            .collect();
        if devices != self.devices {
            self.devices = devices;
            changed = true;
        }
        let targets: Vec<Vec<TargetInfo>> = inner
            .macros
            .iter()
            .map(|m| {
                m.targets
                    .iter()
                    .map(|t| {
                        let abs = DeviceKey(format!("{}/{}", key.0, t.device.0));
                        let dev = doc.resolve(&abs);
                        TargetInfo {
                            target: t.clone(),
                            device_name: dev.map_or_else(|| t.device.0.clone(), |d| d.name.clone()),
                            param: dev
                                .and_then(|d| registry.get(&d.type_id))
                                .and_then(|f| f.descriptor.param_by_key(&t.param).copied())
                                .unwrap_or(crate::seq_paint::FALLBACK_PARAM),
                        }
                    })
                    .collect()
            })
            .collect();
        if targets != self.targets {
            self.targets = targets;
            changed = true;
        }
        self.eligible = ce::eligible_targets(&self.inner, registry);
        if let Some(i) = self.selected_split
            && i >= self.inner.splits.len()
        {
            self.selected_split = None;
            changed = true;
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
        if changed {
            self.repaint = true;
        }
    }

    // ------------------------------------------------------------------ hit testing

    fn bar_hit(&self, p: Point) -> Option<Hit> {
        for i in (0..self.inner.splits.len()).rev() {
            let Some(r) = self.bar_rect(i) else { continue };
            if !r.inflate(0.5, 0.5).contains(p) {
                continue;
            }
            let band = EDGE_PX.min(r.width() / 3.0);
            let vband = EDGE_PX.min(r.height() / 3.0);
            let edge = if p.x <= r.x0 + band {
                Edge::Left
            } else if p.x >= r.x1 - band {
                Edge::Right
            } else if p.y <= r.y0 + vband {
                Edge::Top
            } else if p.y >= r.y1 - vband {
                Edge::Bottom
            } else {
                Edge::Body
            };
            return Some(Hit::Bar(i, edge));
        }
        None
    }

    /// What is under `p`.
    pub fn hit(&self, p: Point) -> Hit {
        if !self.bounds.contains(p) {
            return Hit::None;
        }
        if self.close_rect().contains(p) {
            return Hit::Close;
        }
        for t in ContainerTab::ALL {
            if self.tab_rect(t).contains(p) {
                return Hit::Tab(t);
            }
        }
        match self.tab {
            ContainerTab::Splits => self.hit_splits(p),
            ContainerTab::Macros => self.hit_macros(p),
        }
    }

    /// What the button or field under `p` does, for its tooltip (`None` during a gesture and
    /// away from a button).
    pub fn help_at(&self, p: Point) -> Option<Help> {
        if !matches!(self.drag, Drag::None) {
            return None;
        }
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        match self.hit(p) {
            Hit::Close => help(self.close_rect(), "Close the container editor"),
            Hit::Tab(t) => help(
                self.tab_rect(t),
                match t {
                    ContainerTab::Splits => "Split the keyboard between the devices inside",
                    ContainerTab::Macros => {
                        "Macros: knobs that drive parameters of the devices inside"
                    }
                },
            ),
            Hit::Add => help(self.add_rect(), "Add a note split"),
            Hit::Target(i) => help(self.target_rect(i), "Choose the device this split plays"),
            Hit::TransposeDec(i) => help(
                self.dec_rect(i),
                "Transpose this split down a semitone (Shift: an octave)",
            ),
            Hit::TransposeInc(i) => help(
                self.inc_rect(i),
                "Transpose this split up a semitone (Shift: an octave)",
            ),
            Hit::Delete(i) => help(self.delete_rect(i), "Delete this split"),
            Hit::TargetDelete(j) => help(
                self.target_delete_rect(j),
                "Stop this macro driving the parameter",
            ),
            Hit::Field(j, b) => help(
                self.field_rect(j, b),
                match b {
                    Bound::Min => "Parameter value when the macro is at its minimum",
                    Bound::Max => "Parameter value when the macro is at its maximum",
                },
            ),
            Hit::None
            | Hit::Bar(..)
            | Hit::Map
            | Hit::Keys(_)
            | Hit::Row(_)
            | Hit::MacroRow(_)
            | Hit::MacroName(_)
            | Hit::MacroKnob(_) => None,
        }
    }

    fn hit_splits(&self, p: Point) -> Hit {
        if let Some(h) = self.bar_hit(p) {
            return h;
        }
        if self.map_rect().contains(p) {
            return Hit::Map;
        }
        if self.keys_rect().contains(p) {
            return Hit::Keys(self.key_at(p.x));
        }
        if self.add_rect().contains(p) {
            return Hit::Add;
        }
        for i in 0..self.inner.splits.len() {
            if !self.row_rect(i).contains(p) {
                continue;
            }
            return if self.delete_rect(i).contains(p) {
                Hit::Delete(i)
            } else if self.target_rect(i).contains(p) {
                Hit::Target(i)
            } else if self.dec_rect(i).contains(p) {
                Hit::TransposeDec(i)
            } else if self.inc_rect(i).contains(p) {
                Hit::TransposeInc(i)
            } else {
                Hit::Row(i)
            };
        }
        Hit::None
    }

    fn hit_macros(&self, p: Point) -> Hit {
        for i in 0..MACRO_COUNT {
            if !self.macro_row_rect(i).contains(p) {
                continue;
            }
            return if self.macro_knob_rect(i).contains(p) {
                Hit::MacroKnob(i)
            } else if self.macro_name_rect(i).contains(p) {
                Hit::MacroName(i)
            } else {
                Hit::MacroRow(i)
            };
        }
        let view = self.targets_view();
        if view.contains(p) {
            let n = self.targets.get(self.selected_macro).map_or(0, Vec::len);
            for j in 0..n {
                if !self.target_row_rect(j).contains(p) {
                    continue;
                }
                if self.target_delete_rect(j).contains(p) {
                    return Hit::TargetDelete(j);
                }
                for b in [Bound::Min, Bound::Max] {
                    if self.field_rect(j, b).contains(p) {
                        return Hit::Field(j, b);
                    }
                }
            }
        }
        Hit::None
    }

    // ------------------------------------------------------------------ commands

    fn container_key(&self) -> DeviceKey {
        self.container
            .clone()
            .unwrap_or_else(|| DeviceKey(String::new()))
    }

    fn set_splits(&self, splits: Vec<NoteSplit>, merge: Option<u64>) -> UiAction {
        UiAction::Command(Command::Container(ContainerCommand::SetSplits {
            container: self.container_key(),
            splits,
            merge,
        }))
    }

    fn set_macro(&self, index: usize, target: MacroTarget, merge: Option<u64>) -> UiAction {
        UiAction::Command(Command::Container(ContainerCommand::SetMacro {
            container: self.container_key(),
            index: index as u8,
            target,
            merge,
        }))
    }

    fn macro_param(&self, i: usize, value: f32, merge: Option<u64>) -> UiAction {
        UiAction::Command(Command::SetParam {
            key: self.container_key(),
            param: format!("macro{}", i + 1),
            value,
            merge,
        })
    }

    fn merge_id(&mut self) -> u64 {
        let id = self.next_merge;
        self.next_merge += 1;
        id
    }

    fn audition(&self, key: u8, velocity: u8, on: bool) -> UiAction {
        UiAction::AuditionVelocity {
            device: self.container_key(),
            key,
            velocity,
            on,
        }
    }

    // ------------------------------------------------------------------ events

    /// Handles one event and returns what the app must do. Calls [`Self::sync`] first.
    pub fn handle(&mut self, event: &UiEvent, doc: &RackDoc, registry: &Registry) -> Vec<UiAction> {
        self.sync(doc, registry);
        let mut out = Vec::new();
        if let UiEvent::ScaleChanged(s) = event {
            self.set_scale(*s);
            return out;
        }
        if self.container.is_none() {
            return out;
        }
        match event {
            UiEvent::PointerMove { pos, modifiers } => {
                self.pointer_move(*pos, *modifiers, &mut out)
            }
            UiEvent::PointerDown {
                pos,
                button,
                modifiers,
                clicks,
            } => self.pointer_down(*pos, *button, *modifiers, *clicks, &mut out),
            UiEvent::PointerUp { pos, .. } => self.pointer_up(*pos, &mut out),
            UiEvent::PointerLeave => {
                if self.hover != Hit::None {
                    self.hover = Hit::None;
                    self.repaint = true;
                }
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => self.wheel(delta.y, *pos, *modifiers, &mut out),
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => self.key(key, *modifiers, &mut out),
            _ => {}
        }
        out
    }

    fn pointer_down(
        &mut self,
        pos: Point,
        button: PointerButton,
        modifiers: Modifiers,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        self.repaint = true;
        if button != PointerButton::Primary {
            return;
        }
        let hit = self.hit(pos);
        if self.rename.is_some() && !matches!(hit, Hit::MacroName(i) if i == self.selected_macro) {
            self.rename = None;
        }
        match hit {
            Hit::None => {}
            Hit::Close => out.push(UiAction::CloseContainerEditor),
            Hit::Tab(t) => self.set_tab(t),
            Hit::Bar(i, edge) => {
                self.selected_split = Some(i);
                let merge = self.merge_id();
                self.drag = Drag::Split(Box::new(SplitDrag {
                    index: i,
                    edge,
                    base: self.inner.splits.clone(),
                    press: pos,
                    merge,
                    emitted: false,
                }));
            }
            Hit::Map => {
                self.selected_split = None;
                let (k, v) = (self.key_at(pos.x), self.vel_at(pos.y));
                out.push(self.audition(k, v, true));
                self.drag = Drag::Audition(k);
            }
            Hit::Keys(k) => {
                out.push(self.audition(k, STRIP_VELOCITY, true));
                self.drag = Drag::Audition(k);
            }
            Hit::Add => {
                if let Some(next) = self.added_list() {
                    out.push(self.set_splits(next, None));
                }
            }
            Hit::Row(i) => self.selected_split = Some(i),
            Hit::Target(i) => {
                self.selected_split = Some(i);
                if let Some(next) = self.retarget_list(i) {
                    out.push(self.set_splits(next, None));
                }
            }
            Hit::TransposeDec(i) | Hit::TransposeInc(i) => {
                self.selected_split = Some(i);
                let step = if modifiers.shift { 12 } else { 1 };
                let by = if matches!(hit, Hit::TransposeInc(_)) {
                    step
                } else {
                    -step
                };
                self.step_transpose(i, by, out);
            }
            Hit::Delete(i) => {
                out.push(self.set_splits(ce::without_split(&self.inner.splits, i), None));
            }
            Hit::MacroRow(i) => self.select_macro(i),
            Hit::MacroName(i) => {
                self.select_macro(i);
                if clicks >= 2 {
                    self.rename = Some(self.inner.macros[i].name.clone());
                }
            }
            Hit::MacroKnob(i) => {
                self.select_macro(i);
                let Some(p) = self.macro_params[i] else {
                    return;
                };
                if clicks >= 2 {
                    out.push(self.macro_param(i, p.clamp(p.default), None));
                } else {
                    let merge = self.merge_id();
                    let value = self.macro_values[i];
                    self.drag = Drag::Knob(Box::new(KnobDrag {
                        index: i,
                        drag: ValueDrag::new(&p, value, KNOB_PIXELS),
                        merge,
                        last_y: pos.y,
                        start: value,
                        value,
                    }));
                }
            }
            Hit::Field(j, bound) => {
                let m = self.selected_macro;
                let Some(info) = self.targets.get(m).and_then(|t| t.get(j)).cloned() else {
                    return;
                };
                if clicks >= 2 {
                    let t = ce::reset_bound(&info.target, &info.param, bound);
                    out.push(self.set_macro(m, t, None));
                } else {
                    let merge = self.merge_id();
                    let start = match bound {
                        Bound::Min => info.target.min,
                        Bound::Max => info.target.max,
                    };
                    self.drag = Drag::Bound(Box::new(BoundDrag {
                        macro_index: m,
                        row: j,
                        bound,
                        base: info.target,
                        start,
                        press_y: pos.y,
                        merge,
                        emitted: false,
                    }));
                }
            }
            Hit::TargetDelete(j) => {
                let m = self.selected_macro;
                if let Some(info) = self.targets.get(m).and_then(|t| t.get(j)) {
                    out.push(UiAction::Command(Command::Container(
                        ContainerCommand::RemoveMacroTarget {
                            container: self.container_key(),
                            index: m as u8,
                            device: info.target.device.clone(),
                            param: info.target.param.clone(),
                        },
                    )));
                }
            }
        }
    }

    fn added_list(&self) -> Option<Vec<NoteSplit>> {
        if self.inner.splits.len() >= atomik_model::MAX_SPLITS {
            return None;
        }
        let mut v = self.inner.splits.clone();
        v.push(NoteSplit::full(self.eligible.first()?.clone()));
        Some(v)
    }

    fn retarget_list(&self, i: usize) -> Option<Vec<NoteSplit>> {
        let current = &self.inner.splits.get(i)?.target;
        let order: Vec<&DeviceKey> = self.devices.iter().map(|d| &d.key).collect();
        let at = order.iter().position(|k| *k == current).unwrap_or(0);
        let next = (1..=order.len())
            .map(|s| order[(at + s) % order.len()])
            .find(|k| self.eligible.contains(k))?
            .clone();
        let mut s = self.inner.splits[i].clone();
        s.target = next;
        Some(ce::with_split(&self.inner.splits, i, s))
    }

    fn step_transpose(&self, i: usize, by: i32, out: &mut Vec<UiAction>) {
        let Some(s) = self.inner.splits.get(i) else {
            return;
        };
        let t = ce::transposed(s, by);
        if t != *s {
            out.push(self.set_splits(ce::with_split(&self.inner.splits, i, t), None));
        }
    }

    fn pointer_move(&mut self, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Split(mut d) => {
                let cw = self.map_rect().width() / 128.0;
                let dk = ((pos.x - d.press.x) / cw).round() as i32;
                let dv = (-(pos.y - d.press.y) / self.vel_unit()).round() as i32;
                if let Some(base) = d.base.get(d.index) {
                    let s = ce::drag_split(base, d.edge, dk, dv);
                    if s != d.base[d.index] || d.emitted {
                        let list = ce::with_split(&d.base, d.index, s);
                        out.push(self.set_splits(list, Some(d.merge)));
                        d.emitted = true;
                    }
                }
                self.drag = Drag::Split(d);
                self.repaint = true;
            }
            Drag::Knob(mut d) => {
                if let Some(p) = self.macro_params[d.index] {
                    let v = d.drag.update(&p, d.last_y - pos.y, modifiers.shift);
                    if v.to_bits() != d.value.to_bits() {
                        out.push(self.macro_param(d.index, v, Some(d.merge)));
                    }
                    d.value = v;
                    d.last_y = pos.y;
                }
                self.drag = Drag::Knob(d);
                self.repaint = true;
            }
            Drag::Bound(mut d) => {
                if let Some(info) = self
                    .targets
                    .get(d.macro_index)
                    .and_then(|t| t.get(d.row))
                    .cloned()
                {
                    let t = ce::dragged_bound(
                        &d.base,
                        &info.param,
                        d.bound,
                        d.start,
                        d.press_y - pos.y,
                        modifiers.shift,
                    );
                    if t != d.base || d.emitted {
                        out.push(self.set_macro(d.macro_index, t, Some(d.merge)));
                        d.emitted = true;
                    }
                }
                self.drag = Drag::Bound(d);
                self.repaint = true;
            }
            other => {
                self.drag = other;
                let hover = self.hit(pos);
                if hover != self.hover {
                    self.hover = hover;
                    self.repaint = true;
                }
            }
        }
    }

    fn pointer_up(&mut self, pos: Point, out: &mut Vec<UiAction>) {
        self.repaint = true;
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None => {}
            Drag::Audition(k) => out.push(self.audition(k, STRIP_VELOCITY, false)),
            Drag::Split(d) => {
                if d.emitted {
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Knob(_) => out.push(UiAction::EndMerge),
            Drag::Bound(d) => {
                if d.emitted {
                    out.push(UiAction::EndMerge);
                }
            }
        }
        self.hover = self.hit(pos);
    }

    fn wheel(&mut self, dy: f64, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        if !matches!(self.drag, Drag::None) {
            return;
        }
        let notch = dy.signum();
        if notch == 0.0 {
            return;
        }
        self.repaint = true;
        match (self.tab, self.hit(pos)) {
            (
                ContainerTab::Splits,
                Hit::Row(i) | Hit::Target(i) | Hit::TransposeInc(i) | Hit::TransposeDec(i),
            ) => {
                if self.transpose_rect(i).inflate(30.0, 0.0).contains(pos) {
                    let step = if modifiers.shift { 12.0 } else { 1.0 };
                    self.step_transpose(i, (notch * step) as i32, out);
                }
            }
            (ContainerTab::Macros, Hit::Field(j, bound)) => {
                let m = self.selected_macro;
                if let Some(info) = self.targets.get(m).and_then(|t| t.get(j)).cloned() {
                    let start = match bound {
                        Bound::Min => info.target.min,
                        Bound::Max => info.target.max,
                    };
                    let t = ce::dragged_bound(
                        &info.target,
                        &info.param,
                        bound,
                        start,
                        notch * 6.0,
                        modifiers.shift,
                    );
                    if t != info.target {
                        out.push(self.set_macro(m, t, None));
                    }
                }
            }
            (ContainerTab::Macros, Hit::MacroKnob(i)) => {
                if let Some(p) = self.macro_params[i] {
                    let v = widget::wheel_value(&p, self.macro_values[i], notch, modifiers.shift);
                    if v.to_bits() != self.macro_values[i].to_bits() {
                        out.push(self.macro_param(i, v, None));
                    }
                }
            }
            (ContainerTab::Macros, _) if self.targets_view().contains(pos) => {
                self.scroll = (self.scroll - notch * 36.0).clamp(0.0, self.max_scroll());
            }
            _ => {}
        }
    }

    fn key(&mut self, key: &Key, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        self.repaint = true;
        // Command chords (undo, redo, clipboard) are registry commands the shell resolves;
        // what no command took is not a key of the view.
        if modifiers.command() {
            return;
        }
        if let Some(mut text) = self.rename.take() {
            match seq_panel::rename_key(&mut text, key) {
                RenameKey::Editing => {
                    self.rename = Some(text.chars().take(MAX_MACRO_NAME_CHARS).collect());
                }
                RenameKey::Commit => {
                    let name = text.trim().to_string();
                    if name != self.inner.macros[self.selected_macro].name {
                        out.push(UiAction::Command(Command::Container(
                            ContainerCommand::RenameMacro {
                                container: self.container_key(),
                                index: self.selected_macro as u8,
                                name,
                            },
                        )));
                    }
                }
                RenameKey::Cancel => {}
            }
            return;
        }
        if *key == Key::Escape {
            self.cancel(out);
        }
    }

    /// Esc: cancels the gesture, or closes the editor when there is none.
    fn cancel(&mut self, out: &mut Vec<UiAction>) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None => out.push(UiAction::CloseContainerEditor),
            Drag::Audition(k) => out.push(self.audition(k, STRIP_VELOCITY, false)),
            Drag::Split(d) => {
                if d.emitted {
                    out.push(self.set_splits(d.base, Some(d.merge)));
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Knob(d) => {
                if d.start.to_bits() != d.value.to_bits() {
                    out.push(self.macro_param(d.index, d.start, Some(d.merge)));
                }
                out.push(UiAction::EndMerge);
            }
            Drag::Bound(d) => {
                if d.emitted {
                    out.push(self.set_macro(d.macro_index, d.base, Some(d.merge)));
                    out.push(UiAction::EndMerge);
                }
            }
        }
    }

    // ------------------------------------------------------------------ painting

    /// Paints the editor (scaled by the scale factor, drawing only inside the bounds).
    pub fn paint(&mut self, canvas: &mut dyn Canvas, doc: &RackDoc, registry: &Registry) {
        self.sync(doc, registry);
        paint::paint(self, canvas);
        self.repaint = false;
    }
}
