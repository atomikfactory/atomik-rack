//! The plugin parameter pane: a pane that replaces the arrangement and
//! lists **every** param of one plugin device, grouped by module, with a search field. It is
//! also the editor of plugins that have no GUI of their own.
//!
//! Hosting follows [`crate::ContainerEditorView`]: on [`UiAction::OpenPluginParams`] call
//! [`PluginParamsView::open`] with the device's path key and show the view in place of the
//! arrangement; on [`UiAction::ClosePluginParams`] show the arrangement again (also when
//! [`PluginParamsView::is_present`] turns false because the device was deleted). Give it the
//! pane bounds ([`PluginParamsView::set_bounds`]), `sync` it every frame, route pointer and
//! key events to it while it shows, and keep it fed with the display texts:
//!
//! * [`UiAction::PluginParamTexts`] (also [`PluginParamsView::take_text_requests`]) names the
//!   (param id, plain value) pairs of the visible rows that have no fresh display text; the app
//!   asks `PluginManager::param_text` and answers with [`PluginParamsView::set_texts`]. A text
//!   is only shown for the value it was asked for; after an edit the number is shown until the
//!   new text arrives.
//! * A plugin that offers presets gets a preset bar in the search row (`<`, the name, `>`; the
//!   name opens a list): [`PluginParamsView::set_presets`] gives the list and the preset
//!   loaded last, and the pane answers with [`UiAction::LoadPluginPreset`] or
//!   [`UiAction::StepPluginPreset`].
//!
//! ```text
//! PLUGIN PARAMS   Test Synth  ·  Atomik Test  CLAP                          [x]
//! [ Search name, module or key ]                                  12 of 12
//! OSC
//!  Osc 1   [=====o------------------]   0.00         [PIN] [AUTO] [MAP]
//!  Osc 2   ...
//! FILTER
//!  Filter Mode   [-] [+]                  Low pass    [PIN] [AUTO] [MAP]
//! ```
//!
//! # Rows
//!
//! * The list is **virtual**: only the rows in view are laid out, painted and hit-tested, so a
//!   table of 2 048 params costs the same as one of twelve. The search matches the name, the
//!   module and the key `p<id>` (case-insensitive); modules appear in the plugin's own order
//!   and so do the rows inside them.
//! * A continuous param has a **slider** (a relative drag: the whole width is the whole range,
//!   Shift is ten times finer; double-click resets; the wheel steps 1 %). A stepped param has a
//!   **stepper** (`-` and `+`, one step each), a stepped 0/1 param a **toggle**.
//! * **PIN** adds the param to the device's panel knobs or removes it (at most 16, at least
//!   one). **AUTO** asks the app to add an automation lane for the param on the song track of
//!   the selected clip ([`UiAction::AutomatePluginParam`]). **MAP** (only for a device inside a
//!   container) opens the macro menu: `Map to macro 1..8`, or `Edit range` and `Unmap` for a
//!   param this container's macro drives. A param driven by a macro is **locked**.
//!
//! # Gestures
//!
//! A slider drag is one merged undo step: every `SetParam` carries the same merge id (from
//! [`PLUGIN_GESTURE_MERGE_BASE`], which the model's `MergeKey::Param` merges) and the release
//! ends it with [`UiAction::EndMerge`]; Esc during the drag restores the value of the press.
//! Every other gesture is one command. Esc with nothing to cancel clears the search, then
//! closes the pane; Ctrl/Cmd+Z and Y undo and redo.
//!
//! [`PluginGestureMerger`] does the same for edits that come from the plugin's own window:
//! feed it the begin, change and end of each gesture and apply what it returns.

use crate::action::UiAction;
use crate::canvas::Canvas;
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::plugin_panel::{MAX_FEATURED, featured_pins, param_id_of, param_key};
use crate::seq_paint::SeqTheme;
use crate::theme::Theme;
use crate::tooltip::Help;
use atomik_core::ParamDescriptor;
use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ContainerCommand, DeviceKey, MACRO_COUNT, MacroTarget, PLUGIN_GESTURE_MERGE_BASE,
    PluginCommand, PluginDoc, RackDoc,
};
use std::collections::{BTreeMap, BTreeSet};
use vello::kurbo::{Point, Rect};
use vello::peniko::Color;

mod paint;
mod presets;

/// Height of the top bar.
pub const TOP_BAR_HEIGHT: f64 = 28.0;
/// Height of the search row.
pub const SEARCH_HEIGHT: f64 = 34.0;
/// Height of one param row.
pub const ROW_HEIGHT: f64 = 26.0;
/// Height of a module header.
pub const HEADER_HEIGHT: f64 = 22.0;
/// Height of the help line at the bottom of the pane.
pub const FOOTER_HEIGHT: f64 = 22.0;
const PAD: f64 = 14.0;
const NAME_W: f64 = 210.0;
const TEXT_W: f64 = 120.0;
const BTN_GAP: f64 = 6.0;
const PIN_W: f64 = 36.0;
const AUTO_W: f64 = 44.0;
const MAP_W: f64 = 44.0;
const STEP_W: f64 = 24.0;
const MENU_ROW_H: f64 = 18.0;
const MENU_W: f64 = 170.0;
/// First merge id of the pane's own slider drags (the merger of GUI gestures starts half way,
/// so the two never share an id).
const PANE_MERGE_BASE: u64 = PLUGIN_GESTURE_MERGE_BASE;
const MERGER_MERGE_BASE: u64 = PLUGIN_GESTURE_MERGE_BASE + (1 << 39);
const TEXT_CACHE_MAX: usize = 4_096;
const REQUESTED_MAX: usize = 8_192;
/// Fraction of the range a wheel notch moves a slider.
const WHEEL_STEP: f64 = 0.01;
/// Shift divides drags and wheel steps by this.
const FINE: f64 = 10.0;

/// What the pointer is over. The ids are the plugin's own param ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Nothing.
    None,
    /// The close button.
    Close,
    /// The search field.
    Search,
    /// The slider of a continuous param.
    Slider(u32),
    /// The `-` of a stepper.
    StepDec(u32),
    /// The `+` of a stepper.
    StepInc(u32),
    /// The toggle of a 0/1 param.
    Toggle(u32),
    /// The PIN button.
    Pin(u32),
    /// The AUTO button.
    Auto(u32),
    /// The MAP button.
    Map(u32),
    /// A row's free space (name and value text).
    Row(u32),
    /// The `<` of the preset bar.
    PresetPrev,
    /// The `>` of the preset bar.
    PresetNext,
    /// The preset name button (opens the list).
    PresetName,
    /// A row of the open preset list (index into the list).
    PresetRow(usize),
    /// A row of the open macro menu.
    MenuRow(usize),
}

/// How a param is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Widget {
    Slider,
    Stepper,
    Toggle,
}

/// One param of the plugin as the pane lists it.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    id: u32,
    key: String,
    name: String,
    module: String,
    /// Lower-case `name`, `module` and key, for the search.
    hay: String,
    min: f32,
    max: f32,
    default: f32,
    widget: Widget,
}

#[derive(Debug, Clone, PartialEq)]
enum Item {
    Header(String),
    Param(usize),
}

impl Item {
    fn height(&self) -> f64 {
        match self {
            Item::Header(_) => HEADER_HEIGHT,
            Item::Param(_) => ROW_HEIGHT,
        }
    }
}

#[derive(Debug, Clone)]
struct SliderDrag {
    id: u32,
    row: usize,
    start: f32,
    start_x: f64,
    width: f64,
    merge: u64,
    last: f32,
    emitted: bool,
}

#[derive(Debug, Clone)]
enum Drag {
    None,
    Slider(Box<SliderDrag>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuEntry {
    Map(u8),
    Unmap(u8),
    EditRange(u8),
}

#[derive(Debug, Clone)]
struct Menu {
    id: u32,
    entries: Vec<(MenuEntry, String)>,
    rect: Rect,
    hover: Option<usize>,
}

/// The parameter pane. See the module documentation.
#[derive(Debug)]
pub struct PluginParamsView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    device: Option<DeviceKey>,
    present: bool,
    name: String,
    vendor: String,
    badge: &'static str,
    accent: Color,
    /// The device sits inside a container (MAP is offered).
    in_scope: bool,
    /// (type id, layout hash, descriptor identity) the rows were built from.
    sig: (String, u64, usize),
    rows: Vec<Row>,
    items: Vec<Item>,
    /// Prefix sums of the item heights: `offsets[i]` is the top of item `i`.
    offsets: Vec<f64>,
    content_h: f64,
    query: String,
    scroll: f64,
    /// The values of the device as of the last sync (param key to plain value).
    values: BTreeMap<String, f32>,
    /// The values shown instead of the stored ones (the automation at the playhead, see
    /// [`PluginParamsView::set_shown_values`]).
    shown: atomik_model::ShownValues,
    pins: Vec<u32>,
    /// Params a macro drives: param id to (container path key, macro 0-based), outermost first.
    drivers: BTreeMap<u32, (DeviceKey, u8)>,
    /// Name of each macro of the immediate container (menu labels).
    macro_names: [String; MACRO_COUNT],
    /// Display texts: param id to (bits of the value it was asked for, text).
    texts: BTreeMap<u32, (u32, String)>,
    requested: BTreeSet<(u32, u32)>,
    menu: Option<Menu>,
    presets: presets::PresetBar,
    drag: Drag,
    hover: Hit,
    toast: Option<String>,
    next_merge: u64,
    focused: bool,
    repaint: bool,
}

fn norm_of(row: &Row, v: f32) -> f64 {
    let span = f64::from(row.max - row.min);
    if span <= 0.0 {
        0.0
    } else {
        ((f64::from(v) - f64::from(row.min)) / span).clamp(0.0, 1.0)
    }
}

fn clamp_to(row: &Row, v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(row.min, row.max)
    } else {
        row.default
    }
}

impl PluginParamsView {
    /// Switches to `theme`: the colours are derived again and the view repaints (the accent
    /// follows at the next sync).
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.accent = self.st.base.highlight;
        self.repaint = true;
    }

    /// A closed pane; call [`Self::set_bounds`], [`Self::open`] and [`Self::sync`].
    pub fn new(theme: Theme) -> Self {
        let st = SeqTheme::from_theme(&theme);
        let accent = st.base.highlight;
        Self {
            st,
            bounds: Rect::new(0.0, 0.0, 1000.0, 560.0),
            scale: 1.0,
            device: None,
            present: false,
            name: String::new(),
            vendor: String::new(),
            badge: "",
            accent,
            in_scope: false,
            sig: (String::new(), 0, 0),
            rows: Vec::new(),
            items: Vec::new(),
            offsets: vec![0.0],
            content_h: 0.0,
            query: String::new(),
            scroll: 0.0,
            values: BTreeMap::new(),
            shown: Default::default(),
            pins: Vec::new(),
            drivers: BTreeMap::new(),
            macro_names: Default::default(),
            texts: BTreeMap::new(),
            requested: BTreeSet::new(),
            menu: None,
            presets: presets::PresetBar::default(),
            drag: Drag::None,
            hover: Hit::None,
            toast: None,
            next_merge: PANE_MERGE_BASE,
            focused: false,
            repaint: true,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Opens the pane on a plugin device (its path key). The search is cleared when the device
    /// changes.
    pub fn open(&mut self, device: DeviceKey) {
        if self.device.as_ref() != Some(&device) {
            self.query.clear();
            self.scroll = 0.0;
            self.rows.clear();
            self.items.clear();
            self.offsets = vec![0.0];
            self.sig = (String::new(), 0, 0);
            self.texts.clear();
            self.requested.clear();
        }
        self.in_scope = device.0.contains('/');
        self.device = Some(device);
        self.present = false;
        self.drag = Drag::None;
        self.menu = None;
        self.toast = None;
        self.repaint = true;
    }

    /// The device being shown (path key).
    pub fn device(&self) -> Option<&DeviceKey> {
        self.device.as_ref()
    }

    /// True while the device exists in the document and is a plugin (as of the last sync).
    pub fn is_present(&self) -> bool {
        self.present
    }

    /// Whether the view owns the keyboard as a text field: while the pane shows a plugin its
    /// search field takes every letter, digit and Space, so the shell routes plain keys to it
    /// instead of to commands.
    pub fn is_text_owner(&self) -> bool {
        self.present
    }

    /// Places the pane in the window (logical pixels).
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// The pane's rectangle.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Marks the pane as focused (thin top line).
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

    /// The search text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Sets the search text (the host may pre-fill it).
    pub fn set_query(&mut self, q: &str) {
        if self.query != q {
            self.query = q.to_string();
            self.rebuild_items();
            self.scroll = 0.0;
            self.repaint = true;
        }
    }

    /// Params the plugin has.
    pub fn total_count(&self) -> usize {
        self.rows.len()
    }

    /// Params the search shows.
    pub fn shown_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, Item::Param(_)))
            .count()
    }

    /// The list scroll offset in pixels.
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// The message of the last refused gesture (a locked param, the pin limits), if any.
    pub fn toast(&self) -> Option<&str> {
        self.toast.as_deref()
    }

    /// Whether the param is one of the device's panel knobs.
    pub fn is_pinned(&self, id: u32) -> bool {
        self.pins.contains(&id)
    }

    /// The macro (0-based) that drives the param, from this container or one around it.
    pub fn driver_of(&self, id: u32) -> Option<u8> {
        self.drivers.get(&id).map(|(_, m)| *m)
    }

    /// Gives the display texts the app got from the plugin: (param id, the plain value it was
    /// asked for, the text).
    pub fn set_texts(&mut self, texts: &[(u32, f32, String)]) {
        if self.texts.len() + texts.len() > TEXT_CACHE_MAX {
            self.texts.clear();
        }
        for (id, v, t) in texts {
            self.texts.insert(*id, (v.to_bits(), t.clone()));
        }
        self.repaint = true;
    }

    /// The (param id, plain value) pairs of the visible rows that have no fresh text and were
    /// not asked for yet. Each is returned once.
    pub fn take_text_requests(&mut self) -> Vec<(u32, f32)> {
        if self.requested.len() > REQUESTED_MAX {
            self.requested.clear();
        }
        let mut out = Vec::new();
        for ri in self.visible_rows() {
            let row = &self.rows[ri];
            let v = self.value_of(row);
            let bits = v.to_bits();
            if self.texts.get(&row.id).is_some_and(|(b, _)| *b == bits) {
                continue;
            }
            if self.requested.insert((row.id, bits)) {
                out.push((row.id, v));
            }
        }
        out
    }

    // ------------------------------------------------------------------ geometry

    /// The close button.
    pub fn close_rect(&self) -> Rect {
        let b = self.bounds;
        Rect::new(b.x1 - 28.0, b.y0 + 4.0, b.x1 - 8.0, b.y0 + 24.0)
    }

    /// The search field.
    pub fn search_rect(&self) -> Rect {
        let b = self.bounds;
        let y = b.y0 + TOP_BAR_HEIGHT + 5.0;
        Rect::new(b.x0 + PAD, y, b.x0 + PAD + 300.0, y + SEARCH_HEIGHT - 10.0)
    }

    /// The list area (rows are clipped to it).
    pub fn list_rect(&self) -> Rect {
        let b = self.bounds;
        Rect::new(
            b.x0,
            b.y0 + TOP_BAR_HEIGHT + SEARCH_HEIGHT,
            b.x1,
            (b.y1 - FOOTER_HEIGHT).max(b.y0 + TOP_BAR_HEIGHT + SEARCH_HEIGHT),
        )
    }

    fn max_scroll(&self) -> f64 {
        (self.content_h - self.list_rect().height()).max(0.0)
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    /// The items overlapping the viewport: (item index, top in view coordinates).
    fn visible_items(&self) -> Vec<(usize, f64)> {
        let list = self.list_rect();
        let top = self.scroll;
        let bottom = self.scroll + list.height();
        let first = self
            .offsets
            .partition_point(|o| *o <= top)
            .saturating_sub(1)
            .min(self.items.len());
        let mut out = Vec::new();
        for i in first..self.items.len() {
            let y = self.offsets[i];
            if y >= bottom {
                break;
            }
            if y + self.items[i].height() > top {
                out.push((i, list.y0 + y - self.scroll));
            }
        }
        out
    }

    fn visible_rows(&self) -> Vec<usize> {
        self.visible_items()
            .into_iter()
            .filter_map(|(i, _)| match self.items[i] {
                Item::Param(r) => Some(r),
                Item::Header(_) => None,
            })
            .collect()
    }

    /// The plugin param ids of the rows in view (partly visible ones included).
    pub fn visible_ids(&self) -> Vec<u32> {
        self.visible_rows()
            .into_iter()
            .map(|r| self.rows[r].id)
            .collect()
    }

    /// The module headers in view.
    pub fn visible_headers(&self) -> Vec<String> {
        self.visible_items()
            .into_iter()
            .filter_map(|(i, _)| match &self.items[i] {
                Item::Header(h) => Some(h.clone()),
                Item::Param(_) => None,
            })
            .collect()
    }

    fn row_index(&self, id: u32) -> Option<usize> {
        self.rows.iter().position(|r| r.id == id)
    }

    /// The rectangle of a param's row, when it is in view.
    pub fn row_rect(&self, id: u32) -> Option<Rect> {
        let ri = self.row_index(id)?;
        self.visible_items().into_iter().find_map(|(i, y)| {
            matches!(self.items[i], Item::Param(r) if r == ri).then(|| {
                let b = self.bounds;
                Rect::new(b.x0, y, b.x1, y + ROW_HEIGHT)
            })
        })
    }

    /// The columns of a row at `r`: (slider area, value text, pin, auto, map).
    fn parts(&self, r: Rect) -> (Rect, Rect, Rect, Rect, Option<Rect>) {
        let (y0, y1) = (r.y0 + 3.0, r.y1 - 3.0);
        let mut x = r.x1 - PAD;
        let mut take = |w: f64| {
            let rect = Rect::new(x - w, y0, x, y1);
            x -= w + BTN_GAP;
            rect
        };
        let map = self.in_scope.then(|| take(MAP_W));
        let auto = take(AUTO_W);
        let pin = take(PIN_W);
        let text = Rect::new(x - TEXT_W, y0, x, y1);
        let sx0 = r.x0 + PAD + NAME_W + 10.0;
        let slider = Rect::new(sx0, y0, (text.x0 - 10.0).max(sx0 + 60.0), y1);
        (slider, text, pin, auto, map)
    }

    fn part_of(&self, id: u32) -> Option<(Rect, Row)> {
        let r = self.row_rect(id)?;
        Some((r, self.rows[self.row_index(id)?].clone()))
    }

    /// The slider of a continuous param in view.
    pub fn slider_rect(&self, id: u32) -> Option<Rect> {
        let (r, row) = self.part_of(id)?;
        (row.widget == Widget::Slider).then(|| self.parts(r).0)
    }

    /// The `-` of a stepped param in view.
    pub fn dec_rect(&self, id: u32) -> Option<Rect> {
        let (r, row) = self.part_of(id)?;
        (row.widget == Widget::Stepper).then(|| {
            let s = self.parts(r).0;
            Rect::new(s.x0, s.y0, s.x0 + STEP_W, s.y1)
        })
    }

    /// The `+` of a stepped param in view.
    pub fn inc_rect(&self, id: u32) -> Option<Rect> {
        let (r, row) = self.part_of(id)?;
        (row.widget == Widget::Stepper).then(|| {
            let s = self.parts(r).0;
            Rect::new(s.x0 + STEP_W + 4.0, s.y0, s.x0 + 2.0 * STEP_W + 4.0, s.y1)
        })
    }

    /// The toggle of a 0/1 param in view.
    pub fn toggle_rect(&self, id: u32) -> Option<Rect> {
        let (r, row) = self.part_of(id)?;
        (row.widget == Widget::Toggle).then(|| {
            let s = self.parts(r).0;
            Rect::new(s.x0, s.y0, s.x0 + 2.0 * STEP_W + 4.0, s.y1)
        })
    }

    /// The PIN button of a param in view.
    pub fn pin_rect(&self, id: u32) -> Option<Rect> {
        let (r, _) = self.part_of(id)?;
        Some(self.parts(r).2)
    }

    /// The AUTO button of a param in view.
    pub fn auto_rect(&self, id: u32) -> Option<Rect> {
        let (r, _) = self.part_of(id)?;
        Some(self.parts(r).3)
    }

    /// The MAP button of a param in view (only for a device inside a container).
    pub fn map_rect(&self, id: u32) -> Option<Rect> {
        let (r, _) = self.part_of(id)?;
        self.parts(r).4
    }

    /// The rows of the open macro menu: (label, centre).
    pub fn menu_rows(&self) -> Vec<(String, Point)> {
        let Some(m) = &self.menu else {
            return Vec::new();
        };
        m.entries
            .iter()
            .enumerate()
            .map(|(i, (_, l))| (l.clone(), Self::menu_row(m.rect, i).center()))
            .collect()
    }

    fn menu_row(menu: Rect, i: usize) -> Rect {
        let y = menu.y0 + 2.0 + i as f64 * MENU_ROW_H;
        Rect::new(menu.x0 + 2.0, y, menu.x1 - 2.0, y + MENU_ROW_H)
    }

    /// What the button under `pos` does, for its tooltip (`None` during a drag and away from a
    /// button).
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if !matches!(self.drag, Drag::None) {
            return None;
        }
        let help = |rect: Option<Rect>, text: &'static str| Some(Help::new(rect?, text));
        match self.hit(pos) {
            Hit::Close => help(Some(self.close_rect()), "Close the parameter pane"),
            Hit::StepDec(id) => help(self.dec_rect(id), "Previous value"),
            Hit::StepInc(id) => help(self.inc_rect(id), "Next value"),
            Hit::Toggle(id) => help(self.toggle_rect(id), "Switch this parameter on or off"),
            Hit::Pin(id) => help(
                self.pin_rect(id),
                "Pin this parameter to the device panel in the rack",
            ),
            Hit::Auto(id) => help(
                self.auto_rect(id),
                "Show this parameter's automation lane in the arrangement",
            ),
            Hit::Map(id) => help(self.map_rect(id), "Drive this parameter from a macro"),
            Hit::PresetPrev => help(self.preset_prev_rect(), "Load the previous preset"),
            Hit::PresetNext => help(self.preset_next_rect(), "Load the next preset"),
            Hit::PresetName => help(self.preset_name_rect(), "Choose a preset of this plugin"),
            Hit::None
            | Hit::Search
            | Hit::Slider(_)
            | Hit::Row(_)
            | Hit::MenuRow(_)
            | Hit::PresetRow(_) => None,
        }
    }

    /// What is at `pos`.
    pub fn hit(&self, pos: Point) -> Hit {
        if let Some(h) = self.preset_hit(pos) {
            return h;
        }
        if let Some(m) = &self.menu
            && m.rect.contains(pos)
        {
            return (0..m.entries.len())
                .find(|&i| Self::menu_row(m.rect, i).contains(pos))
                .map_or(Hit::None, Hit::MenuRow);
        }
        if self.close_rect().contains(pos) {
            return Hit::Close;
        }
        if self.search_rect().contains(pos) {
            return Hit::Search;
        }
        let list = self.list_rect();
        if !list.contains(pos) {
            return Hit::None;
        }
        for (i, y) in self.visible_items() {
            let Item::Param(ri) = self.items[i] else {
                continue;
            };
            let b = self.bounds;
            let r = Rect::new(b.x0, y, b.x1, y + ROW_HEIGHT);
            if !r.contains(pos) {
                continue;
            }
            let row = &self.rows[ri];
            let (slider, _, pin, auto, map) = self.parts(r);
            let id = row.id;
            if pin.contains(pos) {
                return Hit::Pin(id);
            }
            if auto.contains(pos) {
                return Hit::Auto(id);
            }
            if map.is_some_and(|m| m.contains(pos)) {
                return Hit::Map(id);
            }
            return match row.widget {
                Widget::Slider if slider.contains(pos) => Hit::Slider(id),
                Widget::Stepper => {
                    let dec = Rect::new(slider.x0, slider.y0, slider.x0 + STEP_W, slider.y1);
                    let inc = Rect::new(
                        slider.x0 + STEP_W + 4.0,
                        slider.y0,
                        slider.x0 + 2.0 * STEP_W + 4.0,
                        slider.y1,
                    );
                    if dec.contains(pos) {
                        Hit::StepDec(id)
                    } else if inc.contains(pos) {
                        Hit::StepInc(id)
                    } else {
                        Hit::Row(id)
                    }
                }
                Widget::Toggle
                    if Rect::new(
                        slider.x0,
                        slider.y0,
                        slider.x0 + 2.0 * STEP_W + 4.0,
                        slider.y1,
                    )
                    .contains(pos) =>
                {
                    Hit::Toggle(id)
                }
                _ => Hit::Row(id),
            };
        }
        Hit::None
    }

    // ------------------------------------------------------------------ sync

    fn value_of(&self, row: &Row) -> f32 {
        clamp_to(
            row,
            self.values.get(&row.key).copied().unwrap_or(row.default),
        )
    }

    fn rebuild_items(&mut self) {
        let q = self.query.trim().to_lowercase();
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        let mut at: BTreeMap<&str, usize> = BTreeMap::new();
        for (i, r) in self.rows.iter().enumerate() {
            if !q.is_empty() && !r.hay.contains(&q) {
                continue;
            }
            let g = *at.entry(r.module.as_str()).or_insert_with(|| {
                groups.push((r.module.clone(), Vec::new()));
                groups.len() - 1
            });
            groups[g].1.push(i);
        }
        let mut items = Vec::with_capacity(self.rows.len() + groups.len());
        for (module, rows) in groups {
            if !module.is_empty() {
                items.push(Item::Header(module));
            }
            items.extend(rows.into_iter().map(Item::Param));
        }
        let mut offsets = Vec::with_capacity(items.len() + 1);
        let mut y = 0.0;
        for it in &items {
            offsets.push(y);
            y += it.height();
        }
        offsets.push(y);
        self.items = items;
        self.offsets = offsets;
        self.content_h = y;
        self.clamp_scroll();
    }

    /// Sets the values the rows show instead of the stored ones: the automation at the playhead,
    /// by `(device, param key)` (see [`atomik_model::RackDoc::automation_values_at`]). The next
    /// [`Self::sync`] shows them. Returns whether the set changed.
    pub fn set_shown_values(&mut self, shown: &atomik_model::ShownValues) -> bool {
        if &self.shown == shown {
            return false;
        }
        self.shown.clone_from(shown);
        true
    }

    /// Re-reads the device and the document. Cheap when nothing changed.
    pub fn sync(&mut self, doc: &RackDoc, registry: &Registry) {
        let Some(key) = self.device.clone() else {
            return;
        };
        let dev = doc.resolve(&key);
        let Some((d, p)) = dev.and_then(|d| d.plugin.as_deref().map(|p| (d, p))) else {
            if self.present {
                self.present = false;
                self.drag = Drag::None;
                self.menu = None;
                self.repaint = true;
            }
            return;
        };
        if !self.present {
            self.present = true;
            self.repaint = true;
        }
        let desc = registry.descriptor(&d.type_id);
        let desc_id = desc.map_or(0, |x| std::ptr::from_ref(x) as usize);
        let sig = (d.type_id.clone(), p.layout.layout_hash(), desc_id);
        if sig != self.sig {
            self.rebuild_rows(p, desc);
            self.sig = sig;
            self.rebuild_items();
            self.repaint = true;
        }
        if self.name != p.layout.name || self.vendor != p.layout.vendor {
            self.name.clone_from(&p.layout.name);
            self.vendor.clone_from(&p.layout.vendor);
            self.repaint = true;
        }
        self.badge = crate::plugin_data::format_badge(p.format);
        self.accent = self.st.base.accent_for(d);
        let pins = featured_pins(p);
        if pins != self.pins {
            self.pins = pins;
            self.repaint = true;
        }
        // A param a macro bank drives shows the value the macro gives it.
        let driven = doc.driven_params(&key);
        let shown = self.shown.range((key.clone(), String::new())..);
        let shown: Vec<(&String, f32)> = shown
            .take_while(|((device, _), _)| device == &key)
            .map(|((_, param), v)| (param, *v))
            .collect();
        if driven.is_empty() && shown.is_empty() {
            if d.params != self.values {
                self.values.clone_from(&d.params);
                self.repaint = true;
            }
        } else {
            let mut values = d.params.clone();
            // An automated param shows what its lane gives; a macro's value comes after.
            for (param, v) in shown {
                values.insert(param.clone(), v);
            }
            for (param, _) in &driven {
                if let Some(v) = doc.driven_value(registry, &key, param) {
                    values.insert(param.clone(), v);
                }
            }
            if values != self.values {
                self.values = values;
                self.repaint = true;
            }
        }
        self.sync_drivers(doc, &key);
    }

    fn rebuild_rows(&mut self, p: &PluginDoc, desc: Option<&atomik_device_sdk::DeviceDescriptor>) {
        let rows: Vec<Row> = p
            .layout
            .params
            .iter()
            .map(|info| {
                let key = param_key(info.id);
                let pd: Option<ParamDescriptor> = desc.and_then(|d| d.param_by_key(&key).copied());
                let (min, max, default, stepped) = pd.map_or(
                    (
                        info.min as f32,
                        info.max as f32,
                        info.default as f32,
                        info.stepped,
                    ),
                    |p| (p.min, p.max, p.default, p.discrete),
                );
                let widget = if !stepped {
                    Widget::Slider
                } else if (max - min - 1.0).abs() < 1e-6 {
                    Widget::Toggle
                } else {
                    Widget::Stepper
                };
                let name = if info.name.is_empty() {
                    key.clone()
                } else {
                    info.name.clone()
                };
                Row {
                    id: info.id,
                    hay: format!(
                        "{}\n{}\n{}",
                        name.to_lowercase(),
                        info.module.to_lowercase(),
                        key
                    ),
                    key,
                    name,
                    module: info.module.clone(),
                    min,
                    max,
                    default,
                    widget,
                }
            })
            .collect();
        self.rows = rows;
        self.texts.clear();
        self.requested.clear();
    }

    /// Finds which params a macro of an enclosing container drives.
    fn sync_drivers(&mut self, doc: &RackDoc, key: &DeviceKey) {
        let segs: Vec<&str> = key.0.split('/').collect();
        let mut drivers = BTreeMap::new();
        let mut names: [String; MACRO_COUNT] = Default::default();
        for depth in 1..segs.len() {
            let container = DeviceKey(segs[..depth].join("/"));
            let relative = segs[depth..].join("/");
            let Some(inner) = doc.inner(&container) else {
                continue;
            };
            if depth == segs.len() - 1 {
                for (i, m) in inner.macros.iter().enumerate() {
                    names[i].clone_from(&m.name);
                }
            }
            for (i, m) in inner.macros.iter().enumerate() {
                for t in &m.targets {
                    if t.device.0 == relative
                        && let Some(id) = param_id_of(&t.param)
                    {
                        drivers
                            .entry(id)
                            .or_insert_with(|| (container.clone(), i as u8));
                    }
                }
            }
        }
        // The macro banks drive the params of top-level devices.
        for a in &doc.macros.assigns {
            if &a.device == key
                && doc.macro_assign_live(a)
                && let Some(id) = param_id_of(&a.param)
            {
                drivers
                    .entry(id)
                    .or_insert_with(|| (a.bank.clone(), a.index));
            }
        }
        if drivers != self.drivers || names != self.macro_names {
            self.drivers = drivers;
            self.macro_names = names;
            self.repaint = true;
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
        if self.device.is_none() || !self.present {
            return out;
        }
        match event {
            UiEvent::PointerMove { pos, modifiers } => {
                self.pointer_move(*pos, *modifiers, &mut out);
            }
            UiEvent::PointerDown {
                pos,
                button,
                modifiers,
                clicks,
            } => self.pointer_down(*pos, *button, *modifiers, *clicks, doc, &mut out),
            UiEvent::PointerUp { .. } => self.pointer_up(&mut out),
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
            } => self.wheel(delta.y, *pos, *modifiers, doc, &mut out),
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => self.key(key, *modifiers, &mut out),
            _ => {}
        }
        let wanted = self.take_text_requests();
        if !wanted.is_empty()
            && let Some(device) = self.device.clone()
        {
            out.push(UiAction::PluginParamTexts {
                device,
                params: wanted,
            });
        }
        out
    }

    fn set_cmd(&self, id: u32, value: f32, merge: Option<u64>) -> Option<UiAction> {
        Some(UiAction::Command(Command::SetParam {
            key: self.device.clone()?,
            param: param_key(id),
            value,
            merge,
        }))
    }

    /// Whether `id` is locked by a macro; sets the message when it is.
    fn locked(&mut self, id: u32) -> bool {
        match self.drivers.get(&id) {
            Some((_, m)) => {
                self.toast = Some(format!(
                    "Driven by macro {}: remove it from the macro first",
                    u16::from(*m) + 1
                ));
                self.repaint = true;
                true
            }
            None => false,
        }
    }

    fn pointer_move(&mut self, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        if let Drag::Slider(d) = &mut self.drag {
            let row = &self.rows[d.row];
            let fine = if modifiers.shift { FINE } else { 1.0 };
            let dn = (pos.x - d.start_x) / d.width.max(1.0) / fine;
            let n = (norm_of(row, d.start) + dn).clamp(0.0, 1.0);
            let value = clamp_to(row, row.min + (n * f64::from(row.max - row.min)) as f32);
            if value.to_bits() != d.last.to_bits() {
                d.last = value;
                d.emitted = true;
                let (id, merge) = (d.id, d.merge);
                out.extend(self.set_cmd(id, value, Some(merge)));
                self.repaint = true;
            }
            return;
        }
        if self.preset_hover(pos) {
            self.repaint = true;
        }
        if let Some(m) = &mut self.menu {
            let hover = (0..m.entries.len()).find(|&i| Self::menu_row(m.rect, i).contains(pos));
            if hover != m.hover {
                m.hover = hover;
                self.repaint = true;
            }
        }
        let hit = self.hit(pos);
        if hit != self.hover {
            self.hover = hit;
            self.repaint = true;
        }
    }

    fn pointer_up(&mut self, out: &mut Vec<UiAction>) {
        if let Drag::Slider(d) = std::mem::replace(&mut self.drag, Drag::None) {
            if d.emitted {
                out.push(UiAction::EndMerge);
            }
            self.repaint = true;
        }
    }

    fn pointer_down(
        &mut self,
        pos: Point,
        button: PointerButton,
        modifiers: Modifiers,
        clicks: u8,
        doc: &RackDoc,
        out: &mut Vec<UiAction>,
    ) {
        self.repaint = true;
        self.toast = None;
        let hit = self.hit(pos);
        // The preset list and bar answer first; a press anywhere closes an open list.
        if button == PointerButton::Primary && self.preset_press(hit, out) {
            return;
        }
        // A press while the menu is open applies a row or just closes the menu.
        if let Some(menu) = self.menu.take() {
            if let Hit::MenuRow(i) = hit {
                self.apply_menu(&menu, menu.entries[i].0, out);
            }
            return;
        }
        if let (
            Hit::Slider(id) | Hit::StepDec(id) | Hit::StepInc(id) | Hit::Toggle(id) | Hit::Row(id),
            Some(device),
        ) = (hit, self.device.clone())
        {
            out.push(UiAction::ParamTouched {
                device,
                param: param_key(id),
            });
        }
        // A right click on a param opens its control menu (MIDI learn, macros), locked or not.
        if button == PointerButton::Secondary {
            if let (
                Hit::Slider(id)
                | Hit::StepDec(id)
                | Hit::StepInc(id)
                | Hit::Toggle(id)
                | Hit::Row(id),
                Some(device),
            ) = (hit, self.device.clone())
            {
                out.push(UiAction::ControlMenu {
                    device,
                    param: param_key(id),
                    at: pos,
                });
            }
            return;
        }
        if button != PointerButton::Primary {
            return;
        }
        let _ = modifiers;
        match hit {
            Hit::None
            | Hit::Search
            | Hit::Row(_)
            | Hit::MenuRow(_)
            | Hit::PresetPrev
            | Hit::PresetNext
            | Hit::PresetName
            | Hit::PresetRow(_) => {}
            Hit::Close => out.push(UiAction::ClosePluginParams),
            Hit::Slider(id) => self.slider_down(id, pos, clicks, out),
            Hit::StepDec(id) => self.step(id, -1.0, out),
            Hit::StepInc(id) => self.step(id, 1.0, out),
            Hit::Toggle(id) => self.toggle(id, out),
            Hit::Pin(id) => self.pin(id, out),
            Hit::Auto(id) => {
                if let Some(device) = self.device.clone() {
                    out.push(UiAction::AutomatePluginParam { device, param: id });
                }
            }
            Hit::Map(id) => self.open_menu(id, doc),
        }
    }

    fn slider_down(&mut self, id: u32, pos: Point, clicks: u8, out: &mut Vec<UiAction>) {
        let Some(ri) = self.row_index(id) else { return };
        if self.locked(id) {
            return;
        }
        let row = self.rows[ri].clone();
        if clicks >= 2 {
            out.extend(self.set_cmd(id, row.default, None));
            return;
        }
        let Some(rect) = self.slider_rect(id) else {
            return;
        };
        let start = self.value_of(&row);
        let merge = self.next_merge;
        self.next_merge += 1;
        self.drag = Drag::Slider(Box::new(SliderDrag {
            id,
            row: ri,
            start,
            start_x: pos.x,
            width: rect.width(),
            merge,
            last: start,
            emitted: false,
        }));
    }

    fn step(&mut self, id: u32, dir: f32, out: &mut Vec<UiAction>) {
        let Some(ri) = self.row_index(id) else { return };
        if self.locked(id) {
            return;
        }
        let row = &self.rows[ri];
        let cur = self.value_of(row);
        let next = clamp_to(row, (cur + dir).round());
        if next.to_bits() != cur.to_bits() {
            out.extend(self.set_cmd(id, next, None));
        }
    }

    fn toggle(&mut self, id: u32, out: &mut Vec<UiAction>) {
        let Some(ri) = self.row_index(id) else { return };
        if self.locked(id) {
            return;
        }
        let row = &self.rows[ri];
        let on = self.value_of(row) >= (row.min + row.max) / 2.0;
        out.extend(self.set_cmd(id, if on { row.min } else { row.max }, None));
    }

    fn pin(&mut self, id: u32, out: &mut Vec<UiAction>) {
        let Some(device) = self.device.clone() else {
            return;
        };
        let mut pins = self.pins.clone();
        if pins.contains(&id) {
            if pins.len() <= 1 {
                self.toast = Some("A plugin panel keeps at least one pinned param".into());
                return;
            }
            pins.retain(|p| *p != id);
        } else {
            if pins.len() >= MAX_FEATURED {
                self.toast = Some(format!("At most {MAX_FEATURED} params can be pinned"));
                return;
            }
            pins.push(id);
        }
        out.push(UiAction::Command(Command::Plugin(
            PluginCommand::SetPinned {
                device,
                pinned: pins,
            },
        )));
    }

    fn open_menu(&mut self, id: u32, doc: &RackDoc) {
        let Some(key) = self.device.clone() else {
            return;
        };
        let Some(r) = self.map_rect(id) else { return };
        let _ = doc;
        let parent = key.0.rsplit_once('/').map(|(p, _)| p.to_string());
        let entries: Vec<(MenuEntry, String)> = match self.drivers.get(&id) {
            Some((container, m)) if Some(&container.0) == parent.as_ref() => vec![
                (MenuEntry::EditRange(*m), "Edit range".into()),
                (MenuEntry::Unmap(*m), "Unmap".into()),
            ],
            Some(_) => {
                self.locked(id);
                return;
            }
            None => (0..MACRO_COUNT as u8)
                .map(|m| {
                    let name = &self.macro_names[usize::from(m)];
                    let label = if name.is_empty() {
                        format!("Map to macro {}", m + 1)
                    } else {
                        format!("Map to macro {} ({name})", m + 1)
                    };
                    (MenuEntry::Map(m), label)
                })
                .collect(),
        };
        let h = entries.len() as f64 * MENU_ROW_H + 4.0;
        let b = self.bounds;
        let x0 = (r.x1 - MENU_W).max(b.x0 + 4.0);
        let y0 = if r.y1 + h > b.y1 {
            r.y0 - h
        } else {
            r.y1 + 2.0
        };
        self.menu = Some(Menu {
            id,
            entries,
            rect: Rect::new(x0, y0, x0 + MENU_W, y0 + h),
            hover: None,
        });
    }

    fn apply_menu(&mut self, menu: &Menu, entry: MenuEntry, out: &mut Vec<UiAction>) {
        let Some(key) = self.device.clone() else {
            return;
        };
        let Some((container, local)) = key.0.rsplit_once('/') else {
            return;
        };
        let container = DeviceKey(container.to_string());
        let device = DeviceKey(local.to_string());
        let param = param_key(menu.id);
        let Some(ri) = self.row_index(menu.id) else {
            return;
        };
        let row = &self.rows[ri];
        match entry {
            MenuEntry::Map(index) => {
                out.push(UiAction::Command(Command::Container(
                    ContainerCommand::SetMacro {
                        container,
                        index,
                        target: MacroTarget {
                            device,
                            param,
                            min: row.min,
                            max: row.max,
                        },
                        merge: None,
                    },
                )));
            }
            MenuEntry::Unmap(index) => {
                out.push(UiAction::Command(Command::Container(
                    ContainerCommand::RemoveMacroTarget {
                        container,
                        index,
                        device,
                        param,
                    },
                )));
            }
            MenuEntry::EditRange(index) => out.push(UiAction::OpenContainerEditor {
                device: container,
                macros: Some(index),
            }),
        }
    }

    fn wheel(
        &mut self,
        dy: f64,
        pos: Point,
        modifiers: Modifiers,
        _doc: &RackDoc,
        out: &mut Vec<UiAction>,
    ) {
        if self.preset_wheel(dy, pos) {
            return;
        }
        self.menu = None;
        self.repaint = true;
        let fine = if modifiers.shift { FINE } else { 1.0 };
        let target = match self.hit(pos) {
            Hit::Slider(id) | Hit::StepDec(id) | Hit::StepInc(id) | Hit::Toggle(id) => Some(id),
            _ => None,
        };
        if let Some(id) = target
            && let Some(ri) = self.row_index(id)
        {
            if self.locked(id) {
                return;
            }
            let row = &self.rows[ri];
            let cur = self.value_of(row);
            let next = match row.widget {
                Widget::Slider => {
                    let span = f64::from(row.max - row.min);
                    clamp_to(row, (f64::from(cur) + dy * WHEEL_STEP / fine * span) as f32)
                }
                Widget::Stepper | Widget::Toggle => clamp_to(
                    row,
                    (cur + dy.signum() as f32 * dy.abs().ceil() as f32).round(),
                ),
            };
            if next.to_bits() != cur.to_bits() {
                out.extend(self.set_cmd(id, next, None));
            }
            return;
        }
        self.scroll = (self.scroll - dy * 3.0 * ROW_HEIGHT).clamp(0.0, self.max_scroll());
    }

    fn key(&mut self, key: &Key, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        self.repaint = true;
        if modifiers.command() {
            match key {
                Key::Char(c) if c.eq_ignore_ascii_case(&'z') => out.push(if modifiers.shift {
                    UiAction::Redo
                } else {
                    UiAction::Undo
                }),
                Key::Char(c) if c.eq_ignore_ascii_case(&'y') => out.push(UiAction::Redo),
                _ => {}
            }
            return;
        }
        let page = self.list_rect().height().max(ROW_HEIGHT);
        match key {
            Key::Escape => {
                if let Drag::Slider(d) = std::mem::replace(&mut self.drag, Drag::None) {
                    if d.emitted {
                        out.extend(self.set_cmd(d.id, d.start, Some(d.merge)));
                        out.push(UiAction::EndMerge);
                    }
                } else if self.preset_escape() || self.menu.take().is_some() {
                } else if !self.query.is_empty() {
                    self.query.clear();
                    self.rebuild_items();
                    self.scroll = 0.0;
                } else {
                    out.push(UiAction::ClosePluginParams);
                }
            }
            Key::Char(c) if !c.is_control() => {
                self.query.push(*c);
                self.rebuild_items();
                self.scroll = 0.0;
            }
            Key::Space => {
                self.query.push(' ');
                self.rebuild_items();
                self.scroll = 0.0;
            }
            Key::Backspace => {
                self.query.pop();
                self.rebuild_items();
                self.scroll = 0.0;
            }
            Key::Home => self.scroll = 0.0,
            Key::End => self.scroll = self.max_scroll(),
            Key::PageDown => self.scroll = (self.scroll + page).min(self.max_scroll()),
            Key::PageUp => self.scroll = (self.scroll - page).max(0.0),
            Key::Down => self.scroll = (self.scroll + ROW_HEIGHT).min(self.max_scroll()),
            Key::Up => self.scroll = (self.scroll - ROW_HEIGHT).max(0.0),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ painting

    /// Paints the pane (scaled by the scale factor, clipped to the bounds).
    pub fn paint(&self, canvas: &mut dyn Canvas, doc: &RackDoc, registry: &Registry) {
        let _ = (doc, registry);
        paint::paint(self, canvas);
    }
}

/// Turns the begin, change and end markers of a gesture made in the plugin's own window into
/// commands: every change of one gesture is a `SetParam` with the same merge
/// id (from `PLUGIN_GESTURE_MERGE_BASE`), so the gesture is **one undo step**; [`end`] gives the
/// [`UiAction::EndMerge`] that closes it. A change outside a gesture (a patch change, an
/// internal LFO the plugin reports) is a plain `SetParam` with no merge id.
///
/// The "from plugin" flag of (the sync op must not send the value back to the plugin)
/// is the app's: it knows which commands came from here.
///
/// [`end`]: Self::end
#[derive(Debug, Clone)]
pub struct PluginGestureMerger {
    next: u64,
    open: BTreeMap<(DeviceKey, u32), u64>,
}

impl Default for PluginGestureMerger {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginGestureMerger {
    /// A merger whose ids start clear of the pane's own drags.
    pub fn new() -> Self {
        Self {
            next: MERGER_MERGE_BASE,
            open: BTreeMap::new(),
        }
    }

    /// A gesture begins on param `id` of `device` (path key). Gives nothing to apply.
    pub fn begin(&mut self, device: &DeviceKey, id: u32) -> Vec<UiAction> {
        let merge = self.next;
        self.next += 1;
        self.open.insert((device.clone(), id), merge);
        Vec::new()
    }

    /// The param changes to the plain `value`.
    pub fn change(&mut self, device: &DeviceKey, id: u32, value: f32) -> Vec<UiAction> {
        let merge = self.open.get(&(device.clone(), id)).copied();
        vec![UiAction::Command(Command::SetParam {
            key: device.clone(),
            param: param_key(id),
            value,
            merge,
        })]
    }

    /// The gesture ends: gives [`UiAction::EndMerge`] when one was open.
    pub fn end(&mut self, device: &DeviceKey, id: u32) -> Vec<UiAction> {
        if self.open.remove(&(device.clone(), id)).is_some() {
            vec![UiAction::EndMerge]
        } else {
            Vec::new()
        }
    }

    /// Whether a gesture is open on the param.
    pub fn is_open(&self, device: &DeviceKey, id: u32) -> bool {
        self.open.contains_key(&(device.clone(), id))
    }
}
