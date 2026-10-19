//! The console view: channel strips side by side, the master section on the
//! right, a toolbar on top. It shares the lower pane with the arrangement.
//!
//! Hosting follows [`crate::ArrangementView`]: `new`, `set_bounds`, `set_scale`, `sync`,
//! `handle` → [`UiAction`]s, `paint`, `needs_repaint`, `set_focused`, `is_dragging`. The app
//! also feeds, once per frame:
//! - [`ConsoleView::set_meters`] with the engine meter readings (linear amplitude), then
//!   [`ConsoleView::set_frame_time`] with a monotonic time in seconds; ballistics are computed
//!   here ([`crate::console_meter`]). There is no time source in the view.
//! - [`ConsoleView::set_latency_readouts`] (per channel, in frames) and
//!   [`ConsoleView::set_pdc_warning`] when the routing plan changes.
//!
//! Actions: control edits are `Command::SetParam` (drags carry one merge id per gesture,
//! starting at 3·2⁴⁰, and end with [`UiAction::EndMerge`]); console edits are
//! `Command::Console(..)`; [`UiAction::ToggleConsole`] / [`UiAction::MaximiseConsole`] come
//! from the toolbar and from F5 / Shift+F5 ([`console_shortcut`]).
//!
//! Interactions: drag knobs vertically, send bars and faders along their travel (Shift =
//! fine), double-click resets to the default, wheel steps; click a section header to fold
//! that section on every strip; drag a strip's name plate to reorder, right-click it to
//! remove the channel; click an FX return's name to pick its effect; click an INSERTS slot to
//! select its effect in the rack, its x to remove it, the first empty slot to add one
//! (`ConsoleCommand::SetInsert`); middle-drag or wheel
//! scrolls (Shift+wheel or wheel over the faders scrolls sideways).
//!
//! Solo display: with any channel soloed, every other channel is drawn as silenced (hollow
//! mute button, dimmed name), derived from the document's `solo` params. A soloed channel's
//! mute button is disabled (dimmed, ignores clicks). `internal.*` params are never shown.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::console_layout::{
    self as cl, CtrlKind, Folds, MASTER_W, MasterLayout, PlacedCtrl, STRIP_GAP, STRIP_W, SectionId,
    StripLayout,
};
use crate::console_meter::{ChannelMeters, GR_MAX_DB, MeterBallistics, meter_norm};
use crate::display_list::DisplayList;
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::paint::{self, fit_label};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::widget::{self, ValueDrag};
use atomik_core::{ParamDescriptor, Taper, Unit};
use atomik_device_sdk::{DeviceDescriptor, DeviceKind, Registry};
use atomik_model::{
    Command, ConsoleCommand, DeviceKey, FX_SLOTS, INSERT_SLOTS, InsertEffect, RackDevice, RackDoc,
    insert_chain,
};
use std::collections::BTreeMap;
use vello::kurbo::{Affine, BezPath, Circle, Line, Point, Rect, RoundedRect, Vec2};
use vello::peniko::Color;

/// Toolbar height.
pub const TOOLBAR_H: f64 = 28.0;
/// Horizontal scrollbar height (below the strips).
pub const SCROLLBAR_H: f64 = 8.0;
/// Pixels of drag for the full range of a send bar.
pub const SLIDER_PIXELS_PER_RANGE: f64 = 120.0;
const MERGE_BASE: u64 = 3 << 40;
const CACHE_BASE: u64 = 1 << 48;
const MOVE_THRESHOLD: f64 = 4.0;
const SCROLL_STEP: f64 = 48.0;
const MENU_ROW: f64 = 20.0;
const MENU_W: f64 = 200.0;
const MENU_ROWS: usize = 18;
const LEFT_PAD: f64 = 4.0;
const MASTER_GAP: f64 = 8.0;
/// Fader position of unity gain for position-type faders (cubic taper).
pub const FADER_UNITY_POS: f32 = 0.75;

/// A console button (toolbar or master section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleButton {
    /// Fold every section, or unfold them all when everything is folded.
    FoldAll,
    /// Open the "add channel" menu (instruments without a channel).
    AddChannel,
    /// Delay compensation on/off.
    Compensation,
    /// Switch the pane back to the arrangement ([`UiAction::ToggleConsole`]).
    Arrange,
    /// Maximise / restore the console ([`UiAction::MaximiseConsole`]).
    Maximise,
    /// Master: clear every solo.
    ClearSolos,
    /// Master: clear every mute.
    ClearMutes,
}

const TOOLBAR: [(ConsoleButton, f64); 5] = [
    (ConsoleButton::FoldAll, 86.0),
    (ConsoleButton::AddChannel, 86.0),
    (ConsoleButton::Compensation, 62.0),
    (ConsoleButton::Arrange, 72.0),
    (ConsoleButton::Maximise, 44.0),
];

/// The console shortcuts for an app that routes them globally: F5 toggles the console and
/// the arrangement, Shift+F5 maximises / restores the console.
pub fn console_shortcut(key: &Key, modifiers: Modifiers) -> Option<UiAction> {
    if modifiers.command() || modifiers.alt || *key != Key::F(5) {
        return None;
    }
    Some(if modifiers.shift {
        UiAction::MaximiseConsole
    } else {
        UiAction::ToggleConsole
    })
}

/// The gain a fader value means, in dB, read from its descriptor:
/// - `Unit::Db`: the plain value (−∞ at the bottom of a `Taper::Decibel` range);
/// - a 0..1 position (`Unit::None` / `Unit::Percent`, linear): the cubic amplitude taper,
///   gain = (pos / 0.75)³, so unity sits at ¾ of the travel and the top is +7.5 dB;
/// - anything else: `None` (the value is shown with its own unit).
pub fn fader_db(p: &ParamDescriptor, plain: f32) -> Option<f32> {
    let v = p.clamp(plain);
    match p.unit {
        Unit::Db => Some(if matches!(p.taper, Taper::Decibel) && v <= p.min {
            f32::NEG_INFINITY
        } else {
            v
        }),
        Unit::None | Unit::Percent
            if matches!(p.taper, Taper::Linear)
                && p.min.abs() < 1e-6
                && (p.max - 1.0).abs() < 1e-6 =>
        {
            Some(if v <= 0.0 {
                f32::NEG_INFINITY
            } else {
                60.0 * (v / FADER_UNITY_POS).log10()
            })
        }
        _ => None,
    }
}

/// The normalized fader position showing `db` (for scale marks), found by bisection on
/// [`fader_db`]; `None` when the fader has no dB meaning or `db` is out of its range.
pub fn fader_norm_at_db(p: &ParamDescriptor, db: f32) -> Option<f64> {
    let at = |n: f64| fader_db(p, p.from_normalized(n as f32));
    let (lo_db, hi_db) = (at(0.0)?, at(1.0)?);
    if db < lo_db || db > hi_db {
        return None;
    }
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if at(mid)? < db {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some((lo + hi) / 2.0)
}

/// The fader readout text ("-3.2 dB", "-inf dB", or the value with its unit).
pub fn fader_text(p: &ParamDescriptor, plain: f32) -> String {
    match fader_db(p, plain) {
        Some(db) if db == f32::NEG_INFINITY => "-inf dB".into(),
        Some(db) if db > 0.05 => format!("+{db:.1} dB"),
        Some(db) => format!("{:.1} dB", db + 0.0),
        None => widget::format_value(p, plain),
    }
}

/// Whether `device` is part of the console (a channel strip or the master): referenced by the
/// document's console section, or of a console device type. Unknown types count only when
/// referenced; known types must be [`DeviceKind::Mixer`].
pub fn is_console_device(doc: &RackDoc, registry: &Registry, device: &RackDevice) -> bool {
    let referenced = doc.console.master.as_ref() == Some(&device.key)
        || doc.console.channels.iter().any(|c| c.channel == device.key);
    let typed = device.type_id == cl::CHANNEL_TYPE || device.type_id == cl::MASTER_TYPE;
    match registry.get(&device.type_id) {
        Some(f) => f.descriptor.kind == DeviceKind::Mixer && (referenced || typed),
        None => referenced,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Strip(usize),
    Master,
}

#[derive(Debug, Clone, PartialEq)]
enum Hit {
    Button(ConsoleButton),
    Header(SectionId),
    Ctrl(Target, usize),
    Name(usize),
    Picker(usize),
    /// An insert chain slot (name or "+ add").
    Insert(Target, usize),
    /// The remove (x) part of a filled insert slot.
    InsertRemove(Target, usize),
    Menu(usize),
    MenuArea,
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
enum MenuItem {
    Source(DeviceKey),
    Effect(Option<DeviceKey>),
    /// A new effect device of this type (insert menu).
    NewEffect(String),
    /// One entry of a long choice param (the control-room source): its value.
    Choice(f32),
    Disabled,
}

#[derive(Debug, Clone)]
struct Menu {
    items: Vec<(String, MenuItem)>,
    slot: Option<usize>,
    /// The insert chain slot this menu fills: (channel or master, slot).
    insert: Option<(DeviceKey, usize)>,
    /// The device and param a pick list of a long choice sets.
    choice: Option<(DeviceKey, &'static str)>,
    origin: Point,
    hover: Option<usize>,
}

impl Menu {
    fn rows(&self) -> usize {
        self.items.len().min(MENU_ROWS)
    }
    fn rect(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.origin.x + MENU_W,
            self.origin.y + self.rows() as f64 * MENU_ROW + 8.0,
        )
    }
    fn row_rect(&self, i: usize) -> Option<Rect> {
        (i < self.rows()).then(|| {
            let y = self.origin.y + 4.0 + i as f64 * MENU_ROW;
            Rect::new(
                self.origin.x + 4.0,
                y,
                self.origin.x + MENU_W - 4.0,
                y + MENU_ROW,
            )
        })
    }
}

#[derive(Debug, Clone)]
enum Drag {
    None,
    Value {
        target: Target,
        ctrl: usize,
        drag: ValueDrag,
        merge: u64,
        last: Point,
        start: f32,
        value: f32,
    },
    Move {
        strip: usize,
        start_x: f64,
        x: f64,
    },
    Pan {
        last: Point,
    },
}

/// What a strip's cached lists depend on (besides the layout generation).
#[derive(Debug, Clone, PartialEq)]
struct Sig {
    device: Option<RackDevice>,
    name: String,
    accent: Color,
    latency: Option<u32>,
    silenced: bool,
    extra: Vec<String>,
    generation: u64,
}

#[derive(Debug)]
struct StripView {
    key: DeviceKey,
    device: Option<RackDevice>,
    desc: Option<&'static DeviceDescriptor>,
    name: String,
    accent: Color,
    latency: Option<u32>,
    silenced: bool,
    /// The insert chain (effect device, display name), in signal order.
    inserts: Vec<(DeviceKey, String)>,
    cache_id: u64,
    revision: u64,
    sig: Option<Sig>,
    lists: [Option<DisplayList>; 2],
}

impl StripView {
    fn new(key: DeviceKey, cache_id: u64) -> Self {
        Self {
            key,
            device: None,
            desc: None,
            name: String::new(),
            accent: Color::WHITE,
            latency: None,
            silenced: false,
            inserts: Vec::new(),
            cache_id,
            revision: 0,
            sig: None,
            lists: [None, None],
        }
    }

    fn value(&self, p: &ParamDescriptor) -> f32 {
        self.device
            .as_ref()
            .map_or(p.clamp(p.default), |d| widget::current_value(d, p))
    }

    fn on(&self, key: &str) -> bool {
        self.desc
            .and_then(|d| d.param_by_key(key))
            .is_some_and(|p| self.value(p) >= (p.min + p.max) / 2.0)
    }

    fn update(&mut self, sig: Sig) {
        if self.sig.as_ref() != Some(&sig) {
            self.sig = Some(sig);
            self.revision += 1;
        }
    }
}

/// The insert chain of `device` with display names.
fn chain_names(doc: &RackDoc, registry: &Registry, device: &DeviceKey) -> Vec<(DeviceKey, String)> {
    insert_chain(doc, registry, device)
        .into_iter()
        .map(|k| {
            let name = doc
                .device(&k)
                .map_or_else(|| k.0.clone(), |d| d.name.clone());
            (k, name)
        })
        .collect()
}

/// Width of the remove (x) hotspot at the right end of a filled insert slot.
const INSERT_X_W: f64 = 14.0;

fn insert_x_rect(r: Rect) -> Rect {
    Rect::new(r.x1 - INSERT_X_W, r.y0, r.x1, r.y1)
}

/// The insert slot hit by `p` (in the section's coordinates): a filled slot or the first
/// empty one ("+ add"); later empty slots are inert.
fn insert_hit(slots: &[Rect], used: usize, p: Point, target: Target) -> Option<Hit> {
    let slot = slots.iter().position(|r| r.contains(p))?;
    if slot > used {
        return None;
    }
    Some(if slot < used && insert_x_rect(slots[slot]).contains(p) {
        Hit::InsertRemove(target, slot)
    } else {
        Hit::Insert(target, slot)
    })
}

fn chain_sig(chain: &[(DeviceKey, String)]) -> Vec<String> {
    chain.iter().map(|(k, n)| format!("{k}={n}")).collect()
}

/// The console view. See the module documentation for hosting.
#[derive(Debug)]
pub struct ConsoleView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    folds: Folds,
    generation: u64,
    scroll: Vec2,
    master_scroll: f64,
    strips: Vec<StripView>,
    master: Option<StripView>,
    return_names: Vec<String>,
    layouts: Vec<(&'static str, StripLayout)>,
    master_layout: Option<MasterLayout>,
    layout_height: f64,
    cache_ids: BTreeMap<DeviceKey, u64>,
    next_cache: u64,
    meters: BTreeMap<DeviceKey, MeterBallistics>,
    latencies: BTreeMap<DeviceKey, u32>,
    pdc_warning: bool,
    maximised: bool,
    compensation: bool,
    sample_rate: u32,
    any_solo: bool,
    any_mute: bool,
    last_time: Option<f64>,
    menu: Option<Menu>,
    drag: Drag,
    pointer: Point,
    focus: Option<(Target, usize)>,
    next_merge: u64,
    focused: bool,
    repaint: bool,
}

impl ConsoleView {
    /// An empty view; call [`Self::set_bounds`] and [`Self::sync`].
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 1000.0, 600.0),
            scale: 1.0,
            folds: Folds::default(),
            generation: 0,
            scroll: Vec2::ZERO,
            master_scroll: 0.0,
            strips: Vec::new(),
            master: None,
            return_names: Vec::new(),
            layouts: Vec::new(),
            master_layout: None,
            layout_height: -1.0,
            cache_ids: BTreeMap::new(),
            next_cache: 1,
            meters: BTreeMap::new(),
            latencies: BTreeMap::new(),
            pdc_warning: false,
            maximised: false,
            compensation: true,
            sample_rate: 48_000,
            any_solo: false,
            any_mute: false,
            last_time: None,
            menu: None,
            drag: Drag::None,
            pointer: Point::ZERO,
            focus: None,
            next_merge: MERGE_BASE,
            focused: false,
            repaint: true,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Places the view in the window (logical pixels).
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// The view's rectangle (logical pixels).
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Sets the display scale factor (also set by [`UiEvent::ScaleChanged`]).
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Marks the view as having keyboard focus (drawn as a thin top line).
    pub fn set_focused(&mut self, focused: bool) {
        if focused != self.focused {
            self.focused = focused;
            self.repaint = true;
        }
    }

    /// Whether the view has focus.
    pub fn focused(&self) -> bool {
        self.focused
    }

    /// True while a pointer gesture is in progress: keep routing pointer events here until
    /// the button is released.
    pub fn is_dragging(&self) -> bool {
        !matches!(self.drag, Drag::None)
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Forces a repaint.
    pub fn request_repaint(&mut self) {
        self.repaint = true;
    }

    /// Records this frame's meter readings (channel or master device key → values). Call
    /// once per frame, then [`Self::set_frame_time`]. Unknown keys are ignored.
    pub fn set_meters(&mut self, readings: &[(DeviceKey, ChannelMeters)]) {
        for (key, m) in readings {
            let known = self.strips.iter().any(|s| &s.key == key)
                || self.master.as_ref().is_some_and(|s| &s.key == key);
            if known {
                self.meters.entry(key.clone()).or_default().set_input(*m);
            }
        }
    }

    /// Advances the meter ballistics to `now_seconds` (any monotonic clock; the first call
    /// only sets the origin, a step backwards counts as zero).
    pub fn set_frame_time(&mut self, now_seconds: f64) {
        let dt = self.last_time.map_or(0.0, |t| (now_seconds - t).max(0.0));
        self.last_time = Some(now_seconds);
        let mut changed = false;
        for m in self.meters.values_mut() {
            changed |= m.advance(dt);
        }
        if changed {
            self.repaint = true;
        }
    }

    /// The ballistics of a channel's (or the master's) meter.
    pub fn meter(&self, key: &DeviceKey) -> Option<&MeterBallistics> {
        self.meters.get(key)
    }

    /// Per-channel latency readouts in frames (from the routing plan's PDC report).
    pub fn set_latency_readouts(&mut self, readouts: &[(DeviceKey, u32)]) {
        let map: BTreeMap<DeviceKey, u32> = readouts.iter().cloned().collect();
        if map != self.latencies {
            self.latencies = map;
            self.repaint = true;
            self.layout_height = -1.0;
        }
    }

    /// Tells the view whether the app shows the console maximised (the MAX button is lit).
    pub fn set_maximised(&mut self, maximised: bool) {
        if maximised != self.maximised {
            self.maximised = maximised;
            self.repaint = true;
        }
    }

    /// Whether the console is shown maximised (as last told by the app).
    pub fn maximised(&self) -> bool {
        self.maximised
    }

    /// Shows the "compensation limit exceeded" warning in the toolbar.
    pub fn set_pdc_warning(&mut self, warning: bool) {
        if warning != self.pdc_warning {
            self.pdc_warning = warning;
            self.repaint = true;
        }
    }

    /// The fold state of every section (view state, shared by all strips).
    pub fn folds(&self) -> Folds {
        self.folds
    }

    /// Folds or unfolds one section on every strip.
    pub fn set_folded(&mut self, section: SectionId, folded: bool) {
        let mut f = self.folds;
        f.set(section, folded);
        self.set_folds(f);
    }

    /// Folds (`true`) or unfolds every section.
    pub fn set_all_folded(&mut self, folded: bool) {
        self.set_folds(Folds::all(folded));
    }

    fn set_folds(&mut self, f: Folds) {
        if f != self.folds {
            self.folds = f;
            self.layout_height = -1.0;
            self.relayout();
            self.repaint = true;
        }
    }

    /// Horizontal scroll of the strips (logical pixels).
    pub fn scroll_x(&self) -> f64 {
        self.scroll.x
    }

    /// Sets the horizontal scroll (clamped).
    pub fn set_scroll_x(&mut self, x: f64) {
        self.scroll.x = x;
        self.clamp_scroll();
        self.repaint = true;
    }

    /// Vertical scroll of the strips' upper sections.
    pub fn scroll_y(&self) -> f64 {
        self.scroll.y
    }

    /// Sets the vertical scroll of the upper sections (clamped).
    pub fn set_scroll_y(&mut self, y: f64) {
        self.scroll.y = y;
        self.clamp_scroll();
        self.repaint = true;
    }

    /// Channel device keys, in console order.
    pub fn strip_keys(&self) -> Vec<DeviceKey> {
        self.strips.iter().map(|s| s.key.clone()).collect()
    }

    /// The layout used by channel strips (of the first strip's type).
    pub fn strip_layout(&self) -> Option<&StripLayout> {
        self.layouts.first().map(|(_, l)| l)
    }

    /// The master section layout.
    pub fn master_layout(&self) -> Option<&MasterLayout> {
        self.master_layout.as_ref()
    }

    /// Whether a channel's mute button responds (false while the channel is soloed).
    pub fn mute_enabled(&self, channel: &DeviceKey) -> bool {
        self.strips
            .iter()
            .find(|s| &s.key == channel)
            .is_some_and(|s| !s.on("solo"))
    }

    /// Whether a channel is silenced by another channel's solo.
    pub fn solo_silenced(&self, channel: &DeviceKey) -> bool {
        self.strips
            .iter()
            .find(|s| &s.key == channel)
            .is_some_and(|s| s.silenced)
    }

    // ------------------------------------------------------------------ geometry

    fn area_top(&self) -> f64 {
        self.bounds.y0 + TOOLBAR_H
    }

    fn area_height(&self) -> f64 {
        (self.bounds.height() - TOOLBAR_H - SCROLLBAR_H).max(0.0)
    }

    fn master_x(&self) -> Option<f64> {
        self.master
            .as_ref()
            .map(|_| self.bounds.x1 - MASTER_W - 4.0)
    }

    fn strips_rect(&self) -> Rect {
        let x1 = self
            .master_x()
            .map_or(self.bounds.x1, |x| (x - MASTER_GAP).max(self.bounds.x0));
        Rect::new(
            self.bounds.x0,
            self.area_top(),
            x1,
            self.area_top() + self.area_height(),
        )
    }

    fn strip_x(&self, i: usize) -> f64 {
        self.bounds.x0 + LEFT_PAD + i as f64 * (STRIP_W + STRIP_GAP) - self.scroll.x
    }

    fn content_width(&self) -> f64 {
        LEFT_PAD * 2.0 + (self.strips.len() + 1) as f64 * (STRIP_W + STRIP_GAP)
    }

    /// The "add channel" placeholder after the last strip.
    fn placeholder_rect(&self) -> Rect {
        let x = self.strip_x(self.strips.len());
        let top = self.area_top();
        Rect::new(x + 10.0, top + 6.0, x + STRIP_W - 2.0, top + 6.0 + 90.0)
    }

    fn layout_of(&self, desc: Option<&'static DeviceDescriptor>) -> Option<&StripLayout> {
        let d = desc?;
        self.layouts
            .iter()
            .find(|(t, _)| *t == d.type_id)
            .map(|(_, l)| l)
    }

    fn max_scroll(&self) -> (Vec2, f64) {
        let sr = self.strips_rect();
        let x = (self.content_width() - sr.width()).max(0.0);
        let y = self
            .layouts
            .iter()
            .map(|(_, l)| (l.upper_height - l.pinned_top).max(0.0))
            .fold(0.0, f64::max);
        let m = self
            .master_layout
            .as_ref()
            .map_or(0.0, |l| (l.upper_height - self.area_height()).max(0.0));
        (Vec2::new(x, y), m)
    }

    fn clamp_scroll(&mut self) {
        let (m, ms) = self.max_scroll();
        self.scroll = Vec2::new(self.scroll.x.clamp(0.0, m.x), self.scroll.y.clamp(0.0, m.y));
        self.master_scroll = self.master_scroll.clamp(0.0, ms);
    }

    fn toolbar_rect(&self, button: ConsoleButton) -> Option<Rect> {
        let b = self.bounds;
        let (y0, y1) = (b.y0 + 5.0, b.y0 + TOOLBAR_H - 5.0);
        match button {
            ConsoleButton::FoldAll | ConsoleButton::AddChannel | ConsoleButton::Compensation => {
                let mut x = b.x0 + 70.0;
                for (bt, w) in &TOOLBAR[..3] {
                    if *bt == button {
                        return Some(Rect::new(x, y0, x + w, y1));
                    }
                    x += w + 6.0;
                }
                None
            }
            ConsoleButton::Arrange | ConsoleButton::Maximise => {
                let mut x = b.x1 - 6.0;
                for (bt, w) in TOOLBAR[3..].iter().rev() {
                    if *bt == button {
                        return Some(Rect::new(x - w, y0, x, y1));
                    }
                    x -= w + 6.0;
                }
                None
            }
            ConsoleButton::ClearSolos | ConsoleButton::ClearMutes => {
                let (l, mx) = (self.master_layout.as_ref()?, self.master_x()?);
                let r = if button == ConsoleButton::ClearSolos {
                    l.clear_solo
                } else {
                    l.clear_mute
                };
                Some(r + Vec2::new(mx, self.area_top()))
            }
        }
    }

    /// Centre of a toolbar or master button (logical window coordinates).
    pub fn button_center(&self, button: ConsoleButton) -> Option<Point> {
        self.toolbar_rect(button).map(|r| r.center())
    }

    fn ctrl_rect(&self, target: Target, ctrl: usize) -> Option<Rect> {
        let top = self.area_top();
        match target {
            Target::Strip(i) => {
                let s = self.strips.get(i)?;
                let l = self.layout_of(s.desc)?;
                let c = l.controls.get(ctrl)?;
                let dy = if l.is_pinned(c.section) {
                    0.0
                } else {
                    -self.scroll.y
                };
                Some(c.rect + Vec2::new(self.strip_x(i), top + dy))
            }
            Target::Master => {
                let l = self.master_layout.as_ref()?;
                let c = l.controls.get(ctrl)?;
                let dy = if c.rect.x0 >= l.fader_x {
                    0.0
                } else {
                    -self.master_scroll
                };
                Some(c.rect + Vec2::new(self.master_x()?, top + dy))
            }
        }
    }

    fn find_ctrl(&self, key: &DeviceKey, param: &str) -> Option<(Target, usize)> {
        if let Some(i) = self.strips.iter().position(|s| &s.key == key) {
            let l = self.layout_of(self.strips[i].desc)?;
            let c = l.controls.iter().position(|c| c.param == param)?;
            return Some((Target::Strip(i), c));
        }
        self.master.as_ref().filter(|m| &m.key == key)?;
        let c = self
            .master_layout
            .as_ref()?
            .controls
            .iter()
            .position(|c| c.param == param)?;
        Some((Target::Master, c))
    }

    /// Centre of a control (channel or master device key, param key), when it is shown and
    /// inside its visible region.
    pub fn control_center(&self, key: &DeviceKey, param: &str) -> Option<Point> {
        let (t, c) = self.find_ctrl(key, param)?;
        let p = self.ctrl_rect(t, c)?.center();
        (self.hit(p) == Hit::Ctrl(t, c)).then_some(p)
    }

    /// Scrolls the upper sections (of the strips, or of the master) so that a control is in
    /// view. Returns false when the control is not shown (unknown, hidden or folded).
    pub fn scroll_to_control(&mut self, key: &DeviceKey, param: &str) -> bool {
        let Some((t, c)) = self.find_ctrl(key, param) else {
            return false;
        };
        let (rect, pinned, view_h) = match t {
            Target::Strip(i) => {
                let Some(l) = self.layout_of(self.strips[i].desc) else {
                    return false;
                };
                let pc = &l.controls[c];
                (pc.rect, l.is_pinned(pc.section), l.pinned_top)
            }
            Target::Master => {
                let Some(l) = &self.master_layout else {
                    return false;
                };
                let pc = &l.controls[c];
                (pc.rect, pc.rect.x0 >= l.fader_x, self.area_height())
            }
        };
        if let Target::Strip(i) = t {
            let sr = self.strips_rect();
            let x = self.strip_x(i);
            if x < sr.x0 || x + STRIP_W > sr.x1 {
                self.scroll.x += x - sr.x0 - LEFT_PAD;
            }
        }
        if !pinned {
            let cur = match t {
                Target::Strip(_) => self.scroll.y,
                Target::Master => self.master_scroll,
            };
            let new = if rect.y0 < cur {
                rect.y0 - 8.0
            } else if rect.y1 > cur + view_h {
                rect.y1 - view_h + 8.0
            } else {
                cur
            };
            match t {
                Target::Strip(_) => self.scroll.y = new,
                Target::Master => self.master_scroll = new,
            }
        }
        self.clamp_scroll();
        self.repaint = true;
        true
    }

    /// A channel strip's rectangle (logical window coordinates).
    pub fn strip_rect(&self, channel: &DeviceKey) -> Option<Rect> {
        let i = self.strips.iter().position(|s| &s.key == channel)?;
        let x = self.strip_x(i);
        Some(Rect::new(
            x,
            self.area_top(),
            x + STRIP_W,
            self.area_top() + self.area_height(),
        ))
    }

    /// Centre of a channel's name plate.
    pub fn name_center(&self, channel: &DeviceKey) -> Option<Point> {
        let i = self.strips.iter().position(|s| &s.key == channel)?;
        let l = self.layout_of(self.strips[i].desc)?;
        Some((l.name + Vec2::new(self.strip_x(i), self.area_top())).center())
    }

    /// Centre of a section header on the first strip, when it is in view.
    pub fn header_center(&self, section: SectionId) -> Option<Point> {
        let s = self.strips.first()?;
        let l = self.layout_of(s.desc)?;
        let ps = l.sections.iter().find(|p| p.id == section)?;
        let dy = if l.is_pinned(section) {
            0.0
        } else {
            -self.scroll.y
        };
        let p = (ps.header + Vec2::new(self.strip_x(0), self.area_top() + dy)).center();
        (self.hit(p) == Hit::Header(section)).then_some(p)
    }

    /// Centre of FX return `slot`'s effect picker.
    pub fn return_picker_center(&self, slot: usize) -> Option<Point> {
        let l = self.master_layout.as_ref()?;
        let (_, r) = l.return_pickers.get(slot)?;
        Some((*r + Vec2::new(self.master_x()?, self.area_top() - self.master_scroll)).center())
    }

    fn insert_rect(&self, device: &DeviceKey, slot: usize) -> Option<Rect> {
        if let Some(i) = self.strips.iter().position(|s| &s.key == device) {
            let l = self.layout_of(self.strips[i].desc)?;
            let r = *l.insert_slots.get(slot)?;
            return Some(r + Vec2::new(self.strip_x(i), self.area_top() - self.scroll.y));
        }
        self.master.as_ref().filter(|m| &m.key == device)?;
        let r = *self.master_layout.as_ref()?.insert_slots.get(slot)?;
        Some(r + Vec2::new(self.master_x()?, self.area_top() - self.master_scroll))
    }

    /// Centre of insert slot `slot` of channel (or master) `device`, when in view.
    pub fn insert_slot_center(&self, device: &DeviceKey, slot: usize) -> Option<Point> {
        let p = self.insert_rect(device, slot)?.center();
        matches!(self.hit(p), Hit::Insert(..)).then_some(p)
    }

    /// Centre of the remove (x) of insert slot `slot`, when the slot is filled and in view.
    pub fn insert_remove_center(&self, device: &DeviceKey, slot: usize) -> Option<Point> {
        let p = insert_x_rect(self.insert_rect(device, slot)?).center();
        matches!(self.hit(p), Hit::InsertRemove(..)).then_some(p)
    }

    /// The open menu's rows (empty when closed).
    pub fn menu_items(&self) -> Vec<String> {
        self.menu
            .as_ref()
            .map_or_else(Vec::new, |m| m.items.iter().map(|i| i.0.clone()).collect())
    }

    /// Centre of menu row `i`.
    pub fn menu_row_center(&self, i: usize) -> Option<Point> {
        self.menu.as_ref()?.row_rect(i).map(|r| r.center())
    }

    fn hit(&self, p: Point) -> Hit {
        if let Some(m) = &self.menu {
            if let Some(i) = (0..m.rows()).find(|i| m.row_rect(*i).is_some_and(|r| r.contains(p))) {
                return Hit::Menu(i);
            }
            if m.rect().contains(p) {
                return Hit::MenuArea;
            }
        }
        if !self.bounds.contains(p) {
            return Hit::Empty;
        }
        if p.y < self.area_top() {
            for (bt, _) in TOOLBAR {
                if self.toolbar_rect(bt).is_some_and(|r| r.contains(p)) {
                    return Hit::Button(bt);
                }
            }
            return Hit::Empty;
        }
        let top = self.area_top();
        if let (Some(mx), Some(l)) = (self.master_x(), self.master_layout.as_ref())
            && p.x >= mx
        {
            let lp = Point::new(p.x - mx, p.y - top);
            if lp.x >= l.fader_x {
                if l.clear_solo.contains(lp) {
                    return Hit::Button(ConsoleButton::ClearSolos);
                }
                if l.clear_mute.contains(lp) {
                    return Hit::Button(ConsoleButton::ClearMutes);
                }
                if let Some(i) = l
                    .controls
                    .iter()
                    .position(|c| c.rect.x0 >= l.fader_x && c.rect.contains(lp))
                {
                    return Hit::Ctrl(Target::Master, i);
                }
                return Hit::Empty;
            }
            let up = lp + Vec2::new(0.0, self.master_scroll);
            if let Some((slot, _)) = l.return_pickers.iter().find(|(_, r)| r.contains(up)) {
                return Hit::Picker(*slot);
            }
            if let Some(h) = insert_hit(
                &l.insert_slots,
                self.master.as_ref().map_or(0, |m| m.inserts.len()),
                up,
                Target::Master,
            ) {
                return h;
            }
            if let Some(i) = l
                .controls
                .iter()
                .position(|c| c.rect.x0 < l.fader_x && c.rect.contains(up))
            {
                return Hit::Ctrl(Target::Master, i);
            }
            return Hit::Empty;
        }
        let sr = self.strips_rect();
        if !sr.contains(p) {
            return Hit::Empty;
        }
        if self.placeholder_rect().contains(p) {
            return Hit::Button(ConsoleButton::AddChannel);
        }
        for (i, s) in self.strips.iter().enumerate() {
            let x = self.strip_x(i);
            if p.x < x || p.x >= x + STRIP_W {
                continue;
            }
            let Some(l) = self.layout_of(s.desc) else {
                return Hit::Empty;
            };
            let lp = Point::new(p.x - x, p.y - top);
            let pinned = lp.y >= l.pinned_top;
            let q = if pinned {
                lp
            } else {
                lp + Vec2::new(0.0, self.scroll.y)
            };
            if pinned && l.name.contains(lp) {
                return Hit::Name(i);
            }
            for sec in &l.sections {
                if l.is_pinned(sec.id) == pinned
                    && sec.id != SectionId::Name
                    && sec.header.contains(q)
                {
                    return Hit::Header(sec.id);
                }
            }
            if !pinned
                && let Some(h) = insert_hit(&l.insert_slots, s.inserts.len(), q, Target::Strip(i))
            {
                return h;
            }
            if let Some(c) = l
                .controls
                .iter()
                .position(|c| l.is_pinned(c.section) == pinned && c.rect.contains(q))
            {
                return Hit::Ctrl(Target::Strip(i), c);
            }
            return Hit::Empty;
        }
        Hit::Empty
    }

    // ------------------------------------------------------------------ sync

    fn cache_id(&mut self, key: &DeviceKey) -> u64 {
        if let Some(id) = self.cache_ids.get(key) {
            return *id;
        }
        let id = CACHE_BASE + self.next_cache * 2;
        self.next_cache += 1;
        self.cache_ids.insert(key.clone(), id);
        id
    }

    /// Updates the view from the document. Emits nothing; cheap when nothing changed.
    pub fn sync(&mut self, doc: &RackDoc, registry: &Registry) {
        let console = &doc.console;
        let desc_of = |key: &DeviceKey| {
            doc.device(key)
                .and_then(|d| registry.get(&d.type_id))
                .map(|f| f.descriptor)
        };
        let param_on = |key: &DeviceKey, param: &str| {
            let (Some(d), Some(desc)) = (doc.device(key), desc_of(key)) else {
                return false;
            };
            desc.param_by_key(param)
                .is_some_and(|p| widget::current_value(d, p) >= (p.min + p.max) / 2.0)
        };
        let any_solo = console
            .channels
            .iter()
            .any(|c| param_on(&c.channel, "solo"));
        let any_mute = console
            .channels
            .iter()
            .any(|c| param_on(&c.channel, "mute"));
        if any_solo != self.any_solo || any_mute != self.any_mute {
            self.any_solo = any_solo;
            self.any_mute = any_mute;
            self.repaint = true;
        }
        self.compensation = console.compensation;
        self.sample_rate = doc.sample_rate.max(1);

        // Strip list, keeping existing views (and their caches) by key.
        let same = self.strips.len() == console.channels.len()
            && self
                .strips
                .iter()
                .zip(&console.channels)
                .all(|(s, c)| s.key == c.channel);
        if !same {
            let mut old: Vec<StripView> = std::mem::take(&mut self.strips);
            for c in &console.channels {
                let view = match old.iter().position(|s| s.key == c.channel) {
                    Some(i) => old.swap_remove(i),
                    None => {
                        let id = self.cache_id(&c.channel);
                        StripView::new(c.channel.clone(), id)
                    }
                };
                self.strips.push(view);
            }
            self.meters.retain(|k, _| {
                console.channels.iter().any(|c| &c.channel == k)
                    || console.master.as_ref() == Some(k)
            });
            self.drag = Drag::None;
            self.focus = None;
            self.repaint = true;
        }
        let generation = self.generation;
        for (i, (s, c)) in self.strips.iter_mut().zip(&console.channels).enumerate() {
            let device = doc.device(&c.channel).cloned();
            let source = c.source.as_ref().and_then(|k| doc.device(k));
            // A second channel of one source (a drum pad's own channel) shows its own name.
            let extra =
                c.source.is_some() && console.channels[..i].iter().any(|o| o.source == c.source);
            s.desc = desc_of(&c.channel);
            s.name = source
                .filter(|_| !extra)
                .or(device.as_ref())
                .map_or_else(|| c.channel.0.clone(), |d| d.name.clone());
            s.accent = source
                .or(device.as_ref())
                .map_or(self.st.base.text_muted, |d| self.st.base.accent_for(d));
            s.latency = self.latencies.get(&c.channel).copied();
            s.device = device;
            s.silenced = any_solo && !s.on("solo");
            s.inserts = chain_names(doc, registry, &c.channel);
            let sig = Sig {
                device: s.device.clone(),
                name: s.name.clone(),
                accent: s.accent,
                latency: s.latency,
                silenced: s.silenced,
                extra: chain_sig(&s.inserts),
                generation,
            };
            if s.sig.as_ref() != Some(&sig) {
                self.repaint = true;
            }
            s.update(sig);
        }

        // Master.
        match &console.master {
            Some(k) => {
                if self.master.as_ref().is_none_or(|m| &m.key != k) {
                    let id = self.cache_id(k);
                    self.master = Some(StripView::new(k.clone(), id));
                    self.repaint = true;
                }
                let names: Vec<String> = console
                    .returns
                    .iter()
                    .map(|r| {
                        r.as_ref().map_or_else(String::new, |k| {
                            doc.device(k)
                                .map_or_else(|| k.0.clone(), |d| d.name.clone())
                        })
                    })
                    .collect();
                let desc = desc_of(k);
                let accent = self.st.base.highlight;
                let flags = vec![format!("{any_solo} {any_mute}")];
                if let Some(m) = &mut self.master {
                    m.device = doc.device(k).cloned();
                    m.desc = desc;
                    m.name = "MASTER".into();
                    m.accent = accent;
                    m.inserts = chain_names(doc, registry, k);
                    let mut extra = names.clone();
                    extra.extend(flags);
                    extra.extend(chain_sig(&m.inserts));
                    let sig = Sig {
                        device: m.device.clone(),
                        name: m.name.clone(),
                        accent,
                        latency: None,
                        silenced: false,
                        extra,
                        generation,
                    };
                    if m.sig.as_ref() != Some(&sig) {
                        self.repaint = true;
                    }
                    m.update(sig);
                }
                self.return_names = names;
            }
            None => {
                if self.master.take().is_some() {
                    self.repaint = true;
                }
            }
        }
        self.relayout();
        self.clamp_scroll();
    }

    fn relayout(&mut self) {
        let h = self.area_height();
        let types: Vec<&'static DeviceDescriptor> = {
            let mut v: Vec<&'static DeviceDescriptor> = Vec::new();
            for d in self.strips.iter().filter_map(|s| s.desc) {
                if !v.iter().any(|x| x.type_id == d.type_id) {
                    v.push(d);
                }
            }
            v
        };
        let master_type = self.master.as_ref().and_then(|m| m.desc);
        let stale = (h - self.layout_height).abs() > 1e-9
            || types.len() != self.layouts.len()
            || types
                .iter()
                .zip(&self.layouts)
                .any(|(d, (t, _))| d.type_id != *t)
            || master_type.is_some() != self.master_layout.is_some();
        if !stale {
            return;
        }
        self.layout_height = h;
        self.layouts = types
            .iter()
            .map(|d| (d.type_id, cl::strip_layout(d, self.folds, h)))
            .collect();
        self.master_layout = master_type.map(|d| cl::master_layout(d, h));
        self.generation += 1;
        let generation = self.generation;
        for s in self.strips.iter_mut().chain(self.master.iter_mut()) {
            if let Some(mut sig) = s.sig.clone() {
                sig.generation = generation;
                s.update(sig);
            }
        }
        self.clamp_scroll();
        self.repaint = true;
    }

    // ------------------------------------------------------------------ events

    /// Handles one event and returns what the app must do. Calls [`Self::sync`] first.
    /// `Resize` is ignored (use [`Self::set_bounds`]).
    pub fn handle(&mut self, event: &UiEvent, doc: &RackDoc, registry: &Registry) -> Vec<UiAction> {
        self.sync(doc, registry);
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::Resize(_)
            | UiEvent::ModifiersChanged { .. }
            | UiEvent::FileHover { .. }
            | UiEvent::FileHoverCancel
            | UiEvent::FileDrop { .. } => {}
            UiEvent::PointerMove { pos, modifiers } => {
                self.pointer_move(*pos, *modifiers, &mut out)
            }
            UiEvent::PointerDown {
                pos,
                button,
                clicks,
                ..
            } => self.pointer_down(*pos, *button, *clicks, doc, registry, &mut out),
            UiEvent::PointerUp { pos, .. } => self.pointer_up(*pos, &mut out),
            UiEvent::PointerLeave => {
                if let Some(m) = &mut self.menu {
                    m.hover = None;
                    self.repaint = true;
                }
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => self.wheel(*delta, *pos, *modifiers, &mut out),
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => self.key(key, *modifiers, &mut out),
            UiEvent::Key { .. } => {}
        }
        out
    }

    fn view_of(&self, t: Target) -> Option<&StripView> {
        match t {
            Target::Strip(i) => self.strips.get(i),
            Target::Master => self.master.as_ref(),
        }
    }

    fn ctrl_of(&self, t: Target, c: usize) -> Option<(&PlacedCtrl, &'static ParamDescriptor, f32)> {
        let v = self.view_of(t)?;
        let desc = v.desc?;
        let pc = match t {
            Target::Strip(_) => self.layout_of(v.desc)?.controls.get(c)?,
            Target::Master => self.master_layout.as_ref()?.controls.get(c)?,
        };
        let p: &'static ParamDescriptor = desc.params.iter().find(|p| p.key == pc.param)?;
        Some((pc, p, v.value(p)))
    }

    fn set_param(
        &self,
        t: Target,
        param: &str,
        value: f32,
        merge: Option<u64>,
    ) -> Option<UiAction> {
        let v = self.view_of(t)?;
        Some(UiAction::Command(Command::SetParam {
            key: v.key.clone(),
            param: param.to_string(),
            value,
            merge,
        }))
    }

    fn mute_blocked(&self, t: Target, param: &str) -> bool {
        param == "mute"
            && matches!(t, Target::Strip(_))
            && self.view_of(t).is_some_and(|v| v.on("solo"))
    }

    fn pointer_down(
        &mut self,
        pos: Point,
        button: PointerButton,
        clicks: u8,
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) {
        self.pointer = pos;
        self.repaint = true;
        let hit = self.hit(pos);
        if self.menu.is_some() {
            match hit {
                Hit::Menu(i) => self.choose_menu(i, out),
                Hit::MenuArea => return,
                _ => {}
            }
            self.menu = None;
            return;
        }
        if button == PointerButton::Middle {
            self.drag = Drag::Pan { last: pos };
            return;
        }
        match (hit, button) {
            (Hit::Button(b), PointerButton::Primary) => {
                self.press_button(b, pos, doc, registry, out)
            }
            (Hit::Header(id), PointerButton::Primary) => {
                let folded = self.folds.is_folded(id);
                self.set_folded(id, !folded);
            }
            (Hit::Name(i), PointerButton::Primary) => {
                self.drag = Drag::Move {
                    strip: i,
                    start_x: pos.x,
                    x: pos.x,
                };
            }
            (Hit::Name(i), PointerButton::Secondary) => {
                out.push(UiAction::Command(Command::Console(
                    ConsoleCommand::RemoveChannel {
                        channel: self.strips[i].key.clone(),
                    },
                )));
            }
            (Hit::Insert(t, slot), PointerButton::Primary) => {
                self.insert_click(t, slot, pos, doc, registry, out)
            }
            (Hit::InsertRemove(t, slot), PointerButton::Primary) => {
                if let Some(v) = self.view_of(t) {
                    out.push(UiAction::Command(Command::Console(
                        ConsoleCommand::SetInsert {
                            channel: v.key.clone(),
                            slot,
                            effect: None,
                        },
                    )));
                }
            }
            (Hit::Picker(slot), PointerButton::Primary) => {
                self.open_return_menu(slot, pos, doc, registry);
            }
            (Hit::Ctrl(t, c), _) => self.ctrl_down(t, c, button, clicks, out),
            _ => {}
        }
    }

    fn press_button(
        &mut self,
        b: ConsoleButton,
        pos: Point,
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) {
        match b {
            ConsoleButton::FoldAll => {
                let fold = self.folds.any_unfolded();
                self.set_all_folded(fold);
            }
            ConsoleButton::AddChannel => self.open_add_menu(pos, doc, registry),
            ConsoleButton::Compensation => out.push(UiAction::Command(Command::Console(
                ConsoleCommand::SetCompensation(!doc.console.compensation),
            ))),
            ConsoleButton::Arrange => out.push(UiAction::ToggleConsole),
            ConsoleButton::Maximise => out.push(UiAction::MaximiseConsole),
            ConsoleButton::ClearSolos => {
                out.push(UiAction::Command(Command::Console(
                    ConsoleCommand::ClearSolos,
                )));
            }
            ConsoleButton::ClearMutes => {
                out.push(UiAction::Command(Command::Console(
                    ConsoleCommand::ClearMutes,
                )));
            }
        }
    }

    fn menu_origin(&self, pos: Point, rows: usize) -> Point {
        let h = rows.min(MENU_ROWS) as f64 * MENU_ROW + 8.0;
        let x = pos.x.min(self.bounds.x1 - MENU_W - 2.0).max(self.bounds.x0);
        let y = if pos.y + h > self.bounds.y1 {
            (pos.y - h).max(self.bounds.y0)
        } else {
            pos.y
        };
        Point::new(x, y)
    }

    fn open_add_menu(&mut self, pos: Point, doc: &RackDoc, registry: &Registry) {
        let mut items: Vec<(String, MenuItem)> = doc
            .devices
            .iter()
            .filter(|d| {
                registry
                    .get(&d.type_id)
                    .is_some_and(|f| f.descriptor.kind == DeviceKind::Instrument)
                    && !doc
                        .console
                        .channels
                        .iter()
                        .any(|c| c.source.as_ref() == Some(&d.key))
            })
            .map(|d| (d.name.clone(), MenuItem::Source(d.key.clone())))
            .collect();
        if items.is_empty() {
            items.push(("Every instrument has a channel".into(), MenuItem::Disabled));
        }
        let origin = self.menu_origin(Point::new(pos.x, self.area_top()), items.len());
        self.menu = Some(Menu {
            items,
            slot: None,
            insert: None,
            choice: None,
            origin,
            hover: None,
        });
    }

    /// Opens the pick list of the long choice `p` (the control-room source) under its cell,
    /// listing every entry of the descriptor, the current one first in view.
    fn open_choice_menu(&mut self, t: Target, c: usize, p: &'static ParamDescriptor) {
        let Some(v) = self.view_of(t) else { return };
        let Some(rect) = self.ctrl_rect(t, c) else {
            return;
        };
        let key = v.key.clone();
        let Unit::Choice(names) = p.unit else { return };
        let items: Vec<(String, MenuItem)> = names
            .iter()
            .enumerate()
            .map(|(i, n)| ((*n).to_string(), MenuItem::Choice(i as f32)))
            .collect();
        let at = Point::new(rect.x0, rect.y1);
        let origin = self.menu_origin(at, items.len());
        self.menu = Some(Menu {
            items,
            slot: None,
            insert: None,
            choice: Some((key, p.key)),
            origin,
            hover: None,
        });
    }

    fn open_return_menu(&mut self, slot: usize, pos: Point, doc: &RackDoc, registry: &Registry) {
        let mut items = vec![("(none)".to_string(), MenuItem::Effect(None))];
        items.extend(
            doc.devices
                .iter()
                .filter(|d| {
                    registry
                        .get(&d.type_id)
                        .is_some_and(|f| f.descriptor.kind == DeviceKind::Effect)
                })
                .map(|d| (d.name.clone(), MenuItem::Effect(Some(d.key.clone())))),
        );
        let origin = self.menu_origin(pos, items.len());
        self.menu = Some(Menu {
            items,
            slot: Some(slot),
            insert: None,
            choice: None,
            origin,
            hover: None,
        });
    }

    /// A click on insert slot `slot`: a filled slot selects its effect (the rack scrolls to
    /// it), the first empty one opens the add menu.
    fn insert_click(
        &mut self,
        t: Target,
        slot: usize,
        pos: Point,
        doc: &RackDoc,
        registry: &Registry,
        out: &mut Vec<UiAction>,
    ) {
        let Some(v) = self.view_of(t) else { return };
        if let Some((effect, _)) = v.inserts.get(slot) {
            out.push(UiAction::Select(effect.clone()));
        } else if slot == v.inserts.len() && slot < INSERT_SLOTS {
            let channel = v.key.clone();
            self.open_insert_menu(channel, slot, pos, doc, registry);
        }
    }

    /// The add menu of an insert slot: a new device of each effect type, then the effect
    /// devices already in the rack that are free (not a return, a chain member or a source).
    fn open_insert_menu(
        &mut self,
        channel: DeviceKey,
        slot: usize,
        pos: Point,
        doc: &RackDoc,
        registry: &Registry,
    ) {
        let is_effect = |type_id: &str| {
            registry
                .get(type_id)
                .is_some_and(|f| f.descriptor.kind == DeviceKind::Effect)
        };
        let mut types: Vec<(&'static str, &'static str)> = registry
            .type_ids()
            .filter_map(|t| registry.get(t))
            .filter(|f| f.descriptor.kind == DeviceKind::Effect)
            .map(|f| (f.descriptor.name, f.descriptor.type_id))
            .collect();
        types.sort();
        let mut items: Vec<(String, MenuItem)> = types
            .into_iter()
            .map(|(name, t)| (format!("New {name}"), MenuItem::NewEffect(t.to_owned())))
            .collect();
        let in_chain = |k: &DeviceKey| {
            doc.console
                .channels
                .iter()
                .map(|c| &c.channel)
                .chain(doc.console.master.iter())
                .any(|c| insert_chain(doc, registry, c).contains(k))
        };
        items.extend(
            doc.devices
                .iter()
                .filter(|d| {
                    is_effect(&d.type_id)
                        && doc.console.return_slot(&d.key).is_none()
                        && doc.console.channel_of(&d.key).is_none()
                        && !in_chain(&d.key)
                })
                .map(|d| (d.name.clone(), MenuItem::Effect(Some(d.key.clone())))),
        );
        let origin = self.menu_origin(pos, items.len());
        self.menu = Some(Menu {
            items,
            slot: None,
            insert: Some((channel, slot)),
            choice: None,
            origin,
            hover: None,
        });
    }

    fn choose_menu(&mut self, i: usize, out: &mut Vec<UiAction>) {
        let Some(m) = &self.menu else { return };
        let Some((_, item)) = m.items.get(i) else {
            return;
        };
        if let (MenuItem::Choice(value), Some((key, param))) = (item, &m.choice) {
            out.push(UiAction::Command(Command::SetParam {
                key: key.clone(),
                param: (*param).to_string(),
                value: *value,
                merge: None,
            }));
            return;
        }
        let cmd = match (item, m.slot) {
            (MenuItem::NewEffect(t), _) if m.insert.is_some() => {
                let (channel, slot) = m.insert.clone().expect("checked");
                ConsoleCommand::SetInsert {
                    channel,
                    slot,
                    effect: Some(InsertEffect::New { type_id: t.clone() }),
                }
            }
            (MenuItem::Effect(Some(e)), _) if m.insert.is_some() => {
                let (channel, slot) = m.insert.clone().expect("checked");
                ConsoleCommand::SetInsert {
                    channel,
                    slot,
                    effect: Some(InsertEffect::Existing(e.clone())),
                }
            }
            (MenuItem::Source(k), _) => ConsoleCommand::AddChannel { source: k.clone() },
            (MenuItem::Effect(e), Some(slot)) if slot < FX_SLOTS => ConsoleCommand::SetReturn {
                slot,
                effect: e.clone(),
            },
            _ => return,
        };
        out.push(UiAction::Command(Command::Console(cmd)));
    }

    fn ctrl_down(
        &mut self,
        t: Target,
        c: usize,
        button: PointerButton,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        let Some((pc, p, value)) = self.ctrl_of(t, c) else {
            return;
        };
        let (kind, param, rect) = (pc.kind, pc.param, pc.rect);
        self.focus = Some((t, c));
        match (kind, button) {
            (CtrlKind::Knob | CtrlKind::Slider | CtrlKind::Fader, PointerButton::Primary) => {
                if clicks >= 2 {
                    out.extend(self.set_param(t, param, p.clamp(p.default), None));
                    return;
                }
                let ppr = match kind {
                    CtrlKind::Fader => (rect.height() - 16.0).max(20.0),
                    CtrlKind::Slider => SLIDER_PIXELS_PER_RANGE,
                    _ => widget::KNOB_PIXELS_PER_RANGE,
                };
                let merge = self.next_merge;
                self.next_merge += 1;
                self.drag = Drag::Value {
                    target: t,
                    ctrl: c,
                    drag: ValueDrag::new(p, value, ppr),
                    merge,
                    last: self.pointer,
                    start: value,
                    value,
                };
            }
            (CtrlKind::Button, PointerButton::Primary) => {
                if !self.mute_blocked(t, param) {
                    out.extend(self.set_param(t, param, widget::toggled(p, value), None));
                }
            }
            (CtrlKind::Choice, PointerButton::Primary) if crate::master_panel::is_pick_list(p) => {
                self.open_choice_menu(t, c, p);
            }
            (CtrlKind::Choice, PointerButton::Primary | PointerButton::Secondary) => {
                let v = widget::cycle_choice(p, value, button == PointerButton::Primary);
                out.extend(self.set_param(t, param, v, None));
            }
            _ => {}
        }
    }

    fn pointer_move(&mut self, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        self.pointer = pos;
        match self.drag.clone() {
            Drag::Value {
                target,
                ctrl,
                mut drag,
                merge,
                last,
                start,
                value,
            } => {
                let Some((pc, p, _)) = self.ctrl_of(target, ctrl) else {
                    return;
                };
                let mut dy = last.y - pos.y;
                if pc.kind == CtrlKind::Slider {
                    dy += pos.x - last.x;
                }
                let param = pc.param;
                let v = drag.update(p, dy, modifiers.shift);
                if v.to_bits() != value.to_bits() {
                    out.extend(self.set_param(target, param, v, Some(merge)));
                }
                self.drag = Drag::Value {
                    target,
                    ctrl,
                    drag,
                    merge,
                    last: pos,
                    start,
                    value: v,
                };
                self.repaint = true;
            }
            Drag::Move { strip, start_x, .. } => {
                self.drag = Drag::Move {
                    strip,
                    start_x,
                    x: pos.x,
                };
                self.repaint = true;
            }
            Drag::Pan { last } => {
                self.drag = Drag::Pan { last: pos };
                self.scroll += last - pos;
                self.clamp_scroll();
                self.repaint = true;
            }
            Drag::None => {
                if let Some(m) = &mut self.menu {
                    let hover =
                        (0..m.rows()).find(|i| m.row_rect(*i).is_some_and(|r| r.contains(pos)));
                    if hover != m.hover {
                        m.hover = hover;
                        self.repaint = true;
                    }
                }
            }
        }
    }

    fn pointer_up(&mut self, pos: Point, out: &mut Vec<UiAction>) {
        self.pointer = pos;
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Value { .. } => out.push(UiAction::EndMerge),
            Drag::Move { strip, start_x, x } if (x - start_x).abs() > MOVE_THRESHOLD => {
                let to = self.drop_index(strip, x);
                if to != strip {
                    out.push(UiAction::Command(Command::Console(
                        ConsoleCommand::MoveChannel {
                            channel: self.strips[strip].key.clone(),
                            to,
                        },
                    )));
                }
            }
            _ => {}
        }
        self.repaint = true;
    }

    /// The final index of strip `strip` dropped at window x `x`: the number of other strips
    /// whose centre is left of `x`.
    fn drop_index(&self, strip: usize, x: f64) -> usize {
        (0..self.strips.len())
            .filter(|i| *i != strip && self.strip_x(*i) + STRIP_W / 2.0 < x)
            .count()
    }

    fn step(&self, t: Target, c: usize, notches: f64, fine: bool) -> Option<UiAction> {
        let (pc, p, value) = self.ctrl_of(t, c)?;
        let v = match pc.kind {
            CtrlKind::Knob | CtrlKind::Slider | CtrlKind::Fader => {
                widget::wheel_value(p, value, notches, fine)
            }
            CtrlKind::Choice => widget::cycle_choice(p, value, notches > 0.0),
            CtrlKind::Button => {
                if self.mute_blocked(t, pc.param) {
                    return None;
                }
                widget::toggled(p, value)
            }
        };
        (v.to_bits() != value.to_bits()).then(|| self.set_param(t, pc.param, v, None))?
    }

    fn wheel(&mut self, delta: Vec2, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        if self.menu.is_some() || !self.bounds.contains(pos) {
            return;
        }
        let hit = self.hit(pos);
        if matches!(self.drag, Drag::None)
            && let Hit::Ctrl(t, c) = hit
            && !modifiers.command()
        {
            out.extend(self.step(t, c, delta.y.signum(), modifiers.shift));
            return;
        }
        let horizontal = modifiers.shift || delta.x.abs() > delta.y.abs();
        let amount = if delta.x.abs() > delta.y.abs() {
            -delta.x
        } else {
            -delta.y
        } * SCROLL_STEP;
        let in_master = self.master_x().is_some_and(|x| pos.x >= x);
        let pinned = self
            .strip_layout()
            .is_some_and(|l| pos.y - self.area_top() >= l.pinned_top);
        if in_master {
            self.master_scroll += amount;
        } else if horizontal || pinned {
            self.scroll.x += amount;
        } else {
            self.scroll.y += amount;
        }
        self.clamp_scroll();
        self.repaint = true;
    }

    fn key(&mut self, key: &Key, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        self.repaint = true;
        if let Some(a) = console_shortcut(key, modifiers) {
            out.push(a);
            return;
        }
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
        match key {
            Key::Escape => {
                if self.menu.take().is_some() {
                    return;
                }
                match std::mem::replace(&mut self.drag, Drag::None) {
                    Drag::Value {
                        target,
                        ctrl,
                        merge,
                        start,
                        value,
                        ..
                    } => {
                        if start.to_bits() != value.to_bits()
                            && let Some((pc, _, _)) = self.ctrl_of(target, ctrl)
                        {
                            out.extend(self.set_param(target, pc.param, start, Some(merge)));
                        }
                        out.push(UiAction::EndMerge);
                    }
                    Drag::None => self.focus = None,
                    _ => {}
                }
            }
            Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                if let Some((t, c)) = self.focus {
                    let n = match key {
                        Key::Up => 1.0,
                        Key::Down => -1.0,
                        Key::PageUp => 10.0,
                        _ => -10.0,
                    };
                    out.extend(self.step(t, c, n, modifiers.shift));
                }
            }
            Key::Left => self.set_scroll_x(self.scroll.x - STRIP_W - STRIP_GAP),
            Key::Right => self.set_scroll_x(self.scroll.x + STRIP_W + STRIP_GAP),
            Key::Home => self.set_scroll_x(0.0),
            Key::End => self.set_scroll_x(f64::MAX),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ painting

    /// Paints the view into its bounds. The canvas receives **physical pixels** (everything is
    /// scaled by the scale factor), like [`crate::RackView::paint`]. Strip bodies are cached
    /// display lists (one per strip part, stamped with a revision); meters are drawn live.
    pub fn paint(&mut self, canvas: &mut dyn Canvas, doc: &RackDoc, registry: &Registry) {
        self.sync(doc, registry);
        let b = self.bounds;
        canvas.push_transform(Affine::scale(self.scale));
        canvas.push_clip(&b.into());
        sp::rect(canvas, b, self.st.background);
        self.record_lists();
        self.paint_strips(canvas);
        self.paint_master(canvas);
        self.paint_toolbar(canvas);
        self.paint_scrollbar(canvas);
        if let Some(m) = &self.menu {
            paint_menu(canvas, &self.st, m);
        }
        if let Some(t) = self.drag_tooltip() {
            paint::tooltip(canvas, &self.st.base, self.pointer, &t);
        }
        if self.focused {
            sp::line(
                canvas,
                b.x0,
                b.y0 + 0.5,
                b.x1,
                b.y0 + 0.5,
                self.st.base.focus,
            );
        }
        canvas.pop_clip();
        canvas.pop_transform();
        self.repaint = false;
    }

    fn drag_tooltip(&self) -> Option<String> {
        let Drag::Value {
            target,
            ctrl,
            value,
            ..
        } = &self.drag
        else {
            return None;
        };
        let (pc, p, _) = self.ctrl_of(*target, *ctrl)?;
        let text = if pc.kind == CtrlKind::Fader {
            fader_text(p, *value)
        } else {
            widget::format_value(p, *value)
        };
        Some(format!("{}  {text}", p.name))
    }

    fn record_lists(&mut self) {
        let st = self.st.clone();
        for i in 0..self.strips.len() {
            let Some(layout) = self.layout_of(self.strips[i].desc).cloned() else {
                continue;
            };
            let s = &self.strips[i];
            let stale = s.lists[0]
                .as_ref()
                .is_none_or(|l| l.revision() != s.revision);
            if !stale {
                continue;
            }
            let mut upper = DisplayList::cached(s.cache_id, s.revision);
            let mut pinned = DisplayList::cached(s.cache_id + 1, s.revision);
            record_strip(&mut upper, &mut pinned, &st, s, &layout, i);
            self.strips[i].lists = [Some(upper), Some(pinned)];
        }
        if let (Some(m), Some(l)) = (&self.master, &self.master_layout) {
            let stale = m.lists[0]
                .as_ref()
                .is_none_or(|x| x.revision() != m.revision);
            if stale {
                let mut upper = DisplayList::cached(m.cache_id, m.revision);
                let mut pinned = DisplayList::cached(m.cache_id + 1, m.revision);
                record_master(
                    &mut upper,
                    &mut pinned,
                    &st,
                    m,
                    l,
                    &self.return_names,
                    (self.any_solo, self.any_mute),
                );
                if let Some(m) = &mut self.master {
                    m.lists = [Some(upper), Some(pinned)];
                }
            }
        }
    }

    fn paint_strips(&self, c: &mut dyn Canvas) {
        let sr = self.strips_rect();
        let top = self.area_top();
        let t = &self.st.base;
        c.push_clip(&sr.into());
        let ph = self.placeholder_rect();
        if ph.x0 < sr.x1 {
            let col = t.text_muted.with_alpha(0.55);
            c.stroke(
                &RoundedRect::from_rect(ph, 4.0).into(),
                &vello::kurbo::Stroke::new(1.2).with_dashes(0.0, [4.0, 3.0]),
                &col.into(),
            );
            let cx = ph.center() - Vec2::new(0.0, 8.0);
            c.stroke_with(
                Line::new(cx - Vec2::new(8.0, 0.0), cx + Vec2::new(8.0, 0.0)),
                2.0,
                col,
            );
            c.stroke_with(
                Line::new(cx - Vec2::new(0.0, 8.0), cx + Vec2::new(0.0, 8.0)),
                2.0,
                col,
            );
            let cx = ph.center();
            c.text(
                &TextRun::new("ADD CHANNEL", pt(cx.x, ph.y1 - 12.0), 6.5, col)
                    .align(Align::Center)
                    .weight(600)
                    .spacing(0.8),
            );
        }
        c.pop_clip();
        if self.strips.is_empty() {
            return;
        }
        let pinned_top = self.strip_layout().map_or(sr.height(), |l| l.pinned_top);
        let upper_clip = Rect::new(sr.x0, top, sr.x1, top + pinned_top.min(sr.height()));
        let pinned_clip = Rect::new(sr.x0, top + pinned_top, sr.x1, sr.y1);
        c.push_clip(&sr.into());
        for (i, s) in self.strips.iter().enumerate() {
            let x = self.strip_x(i);
            if x + STRIP_W < sr.x0 || x > sr.x1 {
                continue;
            }
            strip_background(c, &self.st, Rect::new(x, sr.y0, x + STRIP_W, sr.y1));
            let Some(l) = self.layout_of(s.desc) else {
                sp::label(c, &s.name, x + 6.0, sr.y0 + 20.0, 9.0, t.text, Align::Start);
                sp::label(
                    c,
                    "unknown device",
                    x + 6.0,
                    sr.y0 + 34.0,
                    7.5,
                    t.text_muted,
                    Align::Start,
                );
                continue;
            };
            // Upper part (scrolls vertically).
            c.push_clip(&upper_clip.into());
            c.push_transform(Affine::translate((x, top - self.scroll.y)));
            if let Some(list) = &s.lists[0] {
                c.draw_list(list);
            }
            if let (Some(gr), Some(m)) = (l.gr_meter, self.meters.get(&s.key)) {
                gr_bar(c, &self.st, gr, m.gr_db);
            }
            self.focus_ring(c, Target::Strip(i), false, l);
            c.pop_transform();
            c.pop_clip();
            // Pinned part.
            c.push_clip(&pinned_clip.into());
            c.push_transform(Affine::translate((x, top)));
            if let Some(list) = &s.lists[1] {
                c.draw_list(list);
            }
            if let Some(r) = l.meter {
                stereo_meter(c, &self.st, r, self.meters.get(&s.key), s.silenced);
            }
            self.focus_ring(c, Target::Strip(i), true, l);
            c.pop_transform();
            c.pop_clip();
        }
        // Vertical scroll affordance for the upper sections.
        if let Some(l) = self.strip_layout()
            && l.upper_height > pinned_top + 1e-9
        {
            // Just right of the last strip (or at the area edge when strips overflow).
            let bar_x = (self.strip_x(self.strips.len()) - STRIP_GAP + 1.0).min(sr.x1 - 6.0);
            v_scrollbar(
                c,
                &self.st,
                bar_x,
                top,
                pinned_top,
                self.scroll.y,
                l.upper_height,
            );
        }
        // Shadow where the scrolling sections slide under the pinned ones.
        let (x0, x1) = (
            self.strip_x(0).max(sr.x0),
            (self.strip_x(self.strips.len()) - STRIP_GAP).min(sr.x1),
        );
        let y = top + pinned_top;
        if x1 > x0 && pinned_top > 8.0 {
            c.fill(
                &Rect::new(x0, y - 8.0, x1, y).into(),
                &vello::peniko::Brush::Gradient(
                    vello::peniko::Gradient::new_linear(pt(0.0, y - 8.0), pt(0.0, y))
                        .with_stops([Color::BLACK.with_alpha(0.0), Color::BLACK.with_alpha(0.45)]),
                ),
            );
            sp::line(c, x0, y - 0.5, x1, y - 0.5, t.bevel_dark);
        }
        if let Drag::Move { strip, start_x, x } = &self.drag
            && (x - start_x).abs() > MOVE_THRESHOLD
        {
            let sx = self.strip_x(*strip);
            sp::rect(
                c,
                Rect::new(sx, sr.y0, sx + STRIP_W, sr.y1),
                t.dim.with_alpha(0.35),
            );
            let to = self.drop_index(*strip, *x);
            let others: Vec<usize> = (0..self.strips.len()).filter(|i| i != strip).collect();
            let lx = match others.get(to) {
                Some(&i) => self.strip_x(i) - STRIP_GAP / 2.0,
                None => others
                    .last()
                    .map_or(sx, |&i| self.strip_x(i) + STRIP_W + 1.0),
            };
            c.stroke_with(Line::new(pt(lx, sr.y0), pt(lx, sr.y1)), 3.0, t.highlight);
        }
        c.pop_clip();
    }

    fn focus_ring(&self, c: &mut dyn Canvas, t: Target, pinned: bool, l: &StripLayout) {
        if let Some((ft, fc)) = self.focus
            && ft == t
            && self.focused
            && let Some(pc) = l.controls.get(fc)
            && l.is_pinned(pc.section) == pinned
        {
            paint::outline(c, pc.rect.inset(-1.0), 3.0, 1.2, self.st.base.focus);
        }
    }

    fn paint_master(&self, c: &mut dyn Canvas) {
        let (Some(mx), Some(m), Some(l)) = (self.master_x(), &self.master, &self.master_layout)
        else {
            return;
        };
        let top = self.area_top();
        let h = self.area_height();
        let area = Rect::new(mx - 4.0, top, self.bounds.x1, top + h);
        sp::rect(
            c,
            area,
            mix(self.st.base.panel_bottom, self.st.background, 0.5),
        );
        sp::line(c, area.x0, top, area.x0, top + h, self.st.separator);
        let upper = Rect::new(mx, top, mx + l.fader_x - 2.0, top + h);
        c.push_clip(&upper.into());
        c.push_transform(Affine::translate((mx, top - self.master_scroll)));
        if let Some(list) = &m.lists[0] {
            c.draw_list(list);
        }
        if let (Some(gr), Some(mt)) = (l.gr_meter, self.meters.get(&m.key)) {
            gr_bar(c, &self.st, gr, mt.gr_db);
        }
        c.pop_transform();
        c.pop_clip();
        let fader = Rect::new(mx + l.fader_x, top, mx + l.fader_x + STRIP_W, top + h);
        strip_background(c, &self.st, fader);
        c.push_clip(&fader.into());
        c.push_transform(Affine::translate((mx, top)));
        if let Some(list) = &m.lists[1] {
            c.draw_list(list);
        }
        if let Some(r) = l.meter {
            stereo_meter(c, &self.st, r, self.meters.get(&m.key), false);
        }
        self.paint_output(c, m, l);
        c.pop_transform();
        c.pop_clip();
        if l.upper_height > h + 1e-9 {
            let x = mx + l.fader_x - 7.0;
            v_scrollbar(c, &self.st, x, top, h, self.master_scroll, l.upper_height);
        }
    }

    /// The master OUTPUT readouts (live: peak hold, bus gain reduction, status lamps).
    fn paint_output(&self, c: &mut dyn Canvas, m: &StripView, l: &MasterLayout) {
        let st = &self.st;
        let t = &st.base;
        let meter = self.meters.get(&m.key);
        let peak =
            |ch: usize| meter.map_or(crate::console_meter::METER_FLOOR_DB, |b| b.peak[ch].hold_db);
        let db_text = |db: f32| {
            if db <= crate::console_meter::METER_FLOOR_DB {
                "-inf".to_string()
            } else {
                format!("{:.1}", db + 0.0)
            }
        };
        for (item, r) in &l.output {
            match item {
                cl::OutputItem::PeakLeft | cl::OutputItem::PeakRight => {
                    let ch = usize::from(*item == cl::OutputItem::PeakRight);
                    let db = peak(ch);
                    let label = if ch == 0 { "L" } else { "R" };
                    let col = if db >= -0.5 { st.record } else { t.text };
                    lcd_text(c, st, *r, &format!("{label} {}", db_text(db)), col, 7.0);
                }
                cl::OutputItem::BusGr => {
                    let gr = meter.map_or(0.0, |b| b.gr_db);
                    lcd_text(c, st, *r, &format!("GR {gr:.1} dB"), st.mute, 7.0);
                }
                cl::OutputItem::Status => {
                    let lamps = [
                        ("DIM", m.on("monitor.dim"), st.mute),
                        ("SOLO", self.any_solo, st.solo),
                        ("PDC", self.compensation, t.cable_cv),
                    ];
                    let w = r.width() / lamps.len() as f64;
                    for (i, (text, on, col)) in lamps.into_iter().enumerate() {
                        let x = r.x0 + w * i as f64;
                        let lr = Rect::new(x + 1.0, r.y0, x + w - 1.0, r.y1);
                        let face = if on {
                            mix(col, t.button, 0.3)
                        } else {
                            t.led_off
                        };
                        c.fill_with(RoundedRect::from_rect(lr, 2.0), face);
                        let tc = if on { t.plate } else { t.text_muted };
                        c.text(
                            &TextRun::new(text, pt(lr.center().x, lr.center().y + 2.3), 6.0, tc)
                                .align(Align::Center)
                                .weight(700),
                        );
                    }
                }
            }
        }
    }

    fn paint_toolbar(&self, c: &mut dyn Canvas) {
        let b = self.bounds;
        let st = &self.st;
        let t = &st.base;
        let bar = Rect::new(b.x0, b.y0, b.x1, b.y0 + TOOLBAR_H);
        sp::rect(c, bar, st.header);
        sp::line(c, b.x0, bar.y1 - 0.5, b.x1, bar.y1 - 0.5, st.separator);
        c.text(
            &TextRun::new("CONSOLE", pt(b.x0 + 10.0, b.y0 + 18.0), 10.0, t.text)
                .weight(700)
                .spacing(1.2),
        );
        for (bt, _) in TOOLBAR {
            let Some(r) = self.toolbar_rect(bt) else {
                continue;
            };
            let (text, lit) = match bt {
                ConsoleButton::FoldAll => (
                    if self.folds.any_unfolded() {
                        "FOLD ALL"
                    } else {
                        "UNFOLD ALL"
                    },
                    None,
                ),
                ConsoleButton::AddChannel => ("+ CHANNEL", None),
                ConsoleButton::Compensation => ("PDC", self.compensation.then_some(t.cable_cv)),
                ConsoleButton::Arrange => ("ARRANGE", None),
                _ => ("MAX", self.maximised.then_some(t.highlight)),
            };
            small_button(c, st, r, text, lit, true, 8.0);
        }
        // PDC readout and warning.
        if let Some(r) = self.toolbar_rect(ConsoleButton::Compensation) {
            let max = self.latencies.values().copied().max().unwrap_or(0);
            let ms = f64::from(max) * 1000.0 / f64::from(self.sample_rate);
            let text = if self.compensation {
                format!("LATENCY {max} smp  {ms:.1} ms")
            } else {
                format!("LATENCY {max} smp  {ms:.1} ms  (not compensated)")
            };
            sp::label(
                c,
                &text,
                r.x1 + 10.0,
                r.center().y + 3.5,
                8.5,
                t.text_muted,
                Align::Start,
            );
            if self.pdc_warning {
                let x = r.x1 + 10.0 + paint::estimate_width(&text, 8.5) + 12.0;
                let w = 178.0;
                let wr = Rect::new(x, r.y0, x + w, r.y1);
                c.fill_with(RoundedRect::from_rect(wr, 3.0), t.error_bg);
                c.text(
                    &TextRun::new(
                        "COMPENSATION LIMIT EXCEEDED",
                        pt(wr.center().x, wr.center().y + 3.0),
                        8.0,
                        t.text,
                    )
                    .align(Align::Center)
                    .weight(700),
                );
            }
        }
    }

    fn paint_scrollbar(&self, c: &mut dyn Canvas) {
        let sr = self.strips_rect();
        let content = self.content_width();
        if content <= sr.width() + 1e-9 {
            return;
        }
        let track = Rect::new(
            sr.x0 + 4.0,
            sr.y1 + 2.0,
            sr.x1 - 4.0,
            sr.y1 + SCROLLBAR_H - 2.0,
        );
        c.fill_with(RoundedRect::from_rect(track, 2.0), self.st.base.lcd);
        let frac = sr.width() / content;
        let w = (track.width() * frac).max(20.0);
        let x = track.x0 + (track.width() - w) * (self.scroll.x / (content - sr.width()));
        c.fill_with(
            RoundedRect::new(x, track.y0, x + w, track.y1, 2.0),
            self.st.base.text_muted.with_alpha(0.6),
        );
    }
}

// ------------------------------------------------------------------ painting helpers

/// A thin vertical scrollbar: track at `x` from `top` (`view` tall) showing `scroll` of a
/// `content`-tall column.
fn v_scrollbar(
    c: &mut dyn Canvas,
    st: &SeqTheme,
    x: f64,
    top: f64,
    view: f64,
    scroll: f64,
    content: f64,
) {
    if view <= 8.0 || content <= view {
        return;
    }
    let track = Rect::new(x, top + 2.0, x + 4.0, top + view - 2.0);
    c.fill_with(
        RoundedRect::from_rect(track, 2.0),
        st.base.lcd.with_alpha(0.85),
    );
    let h = (track.height() * view / content).max(14.0);
    let y = track.y0 + (track.height() - h) * (scroll / (content - view)).clamp(0.0, 1.0);
    c.fill_with(
        RoundedRect::new(track.x0, y, track.x1, y + h, 2.0),
        st.base.text_muted.with_alpha(0.7),
    );
}

fn strip_background(c: &mut dyn Canvas, st: &SeqTheme, r: Rect) {
    let t = &st.base;
    c.fill(
        &r.into(),
        &vello::peniko::Brush::Gradient(
            vello::peniko::Gradient::new_linear(pt(r.x0, 0.0), pt(r.x1, 0.0))
                .with_stops([mix(t.panel_top, Color::WHITE, 0.02), t.panel_bottom]),
        ),
    );
    sp::line(c, r.x0 + 0.5, r.y0, r.x0 + 0.5, r.y1, t.bevel_light);
    sp::line(c, r.x1 - 0.5, r.y0, r.x1 - 0.5, r.y1, t.bevel_dark);
}

/// A small latching button: label inside, lit with `lit` when active, dimmed when disabled.
fn small_button(
    c: &mut dyn Canvas,
    st: &SeqTheme,
    r: Rect,
    text: &str,
    lit: Option<Color>,
    enabled: bool,
    size: f32,
) {
    let t = &st.base;
    let face = match lit {
        Some(col) => mix(col, t.button, 0.2),
        None => t.button,
    };
    let face = if enabled {
        face
    } else {
        mix(face, t.panel_bottom, 0.6)
    };
    c.fill_with(
        RoundedRect::from_rect(r + Vec2::new(0.0, 1.0), 2.5),
        Color::BLACK.with_alpha(0.45),
    );
    c.fill_with(RoundedRect::from_rect(r, 2.5), face);
    c.stroke_with(
        Line::new(pt(r.x0 + 2.0, r.y0 + 0.5), pt(r.x1 - 2.0, r.y0 + 0.5)),
        1.0,
        t.bevel_light,
    );
    let col = match (lit, enabled) {
        (_, false) => t.text_muted.with_alpha(0.45),
        (Some(_), true) => t.plate,
        (None, true) => t.text,
    };
    let text = fit_label(text, r.width() - 2.0, size);
    c.text(
        &TextRun::new(
            text,
            pt(r.center().x, r.center().y + f64::from(size) * 0.36),
            size,
            col,
        )
        .align(Align::Center)
        .weight(700),
    );
}

fn lcd_text(c: &mut dyn Canvas, st: &SeqTheme, r: Rect, text: &str, color: Color, size: f32) {
    sp::lcd(c, st, r, None);
    let text = fit_label(text, r.width() - 6.0, size);
    c.text(
        &TextRun::new(
            text,
            pt(r.center().x, r.center().y + f64::from(size) * 0.36),
            size,
            color,
        )
        .align(Align::Center)
        .weight(600),
    );
}

fn section_header(c: &mut dyn Canvas, st: &SeqTheme, r: Rect, title: &str, folded: bool) {
    let t = &st.base;
    c.fill_with(r.inset((-2.0, -1.0)), mix(t.plate, t.panel_bottom, 0.45));
    let cx = pt(r.x0 + 9.0, r.center().y);
    let mut p = BezPath::new();
    if folded {
        p.move_to(cx + Vec2::new(-1.5, -3.0));
        p.line_to(cx + Vec2::new(1.8, 0.0));
        p.line_to(cx + Vec2::new(-1.5, 3.0));
    } else {
        p.move_to(cx + Vec2::new(-3.0, -1.5));
        p.line_to(cx + Vec2::new(0.0, 1.8));
        p.line_to(cx + Vec2::new(3.0, -1.5));
    }
    c.stroke_with(p, 1.3, t.text_muted);
    c.text(
        &TextRun::new(
            title,
            pt(r.x0 + 16.0, r.center().y + 2.6),
            6.5,
            t.text_muted,
        )
        .weight(600)
        .spacing(0.9),
    );
}

fn knob_cell(
    c: &mut dyn Canvas,
    st: &SeqTheme,
    pc: &PlacedCtrl,
    p: &ParamDescriptor,
    v: f32,
    accent: Color,
) {
    let r = pc.rect;
    let label_h = if pc.label.is_empty() { 0.0 } else { 12.0 };
    let radius = ((r.width() / 2.0 - 1.0).min((r.height() - label_h) / 2.0 - 1.0)).min(14.0);
    let center = pt(r.center().x, r.y0 + (r.height() - label_h) / 2.0);
    let scale = radius / (paint::KNOB_RING_R + 1.3);
    let bipolar = p.min < 0.0 && p.max > 0.0 && matches!(p.taper, Taper::Linear);
    c.push_transform(
        Affine::translate(center.to_vec2())
            * Affine::scale(scale)
            * Affine::translate(-center.to_vec2()),
    );
    paint::knob(
        c,
        &st.base,
        center,
        f64::from(p.to_normalized(v)),
        bipolar,
        accent,
    );
    c.pop_transform();
    if !pc.label.is_empty() {
        c.text(
            &TextRun::new(
                pc.label.as_str(),
                pt(r.center().x, r.y1 - 3.0),
                6.5,
                st.base.text_muted,
            )
            .align(Align::Center)
            .spacing(0.4),
        );
    }
}

fn slider_cell(
    c: &mut dyn Canvas,
    st: &SeqTheme,
    r: Rect,
    p: &ParamDescriptor,
    v: f32,
    color: Color,
) {
    let t = &st.base;
    let track = Rect::new(r.x0, r.center().y - 4.0, r.x1, r.center().y + 4.0);
    c.fill_with(RoundedRect::from_rect(track, 2.0), t.lcd);
    let n = f64::from(p.to_normalized(v));
    if n > 0.0 {
        c.fill_with(
            RoundedRect::new(
                track.x0 + 1.0,
                track.y0 + 1.0,
                track.x0 + 1.0 + (track.width() - 2.0) * n,
                track.y1 - 1.0,
                1.5,
            ),
            color,
        );
    }
    let x = track.x0 + 1.0 + (track.width() - 2.0) * n;
    c.stroke_with(
        Line::new(pt(x, track.y0 - 1.5), pt(x, track.y1 + 1.5)),
        1.5,
        t.knob_pointer,
    );
}

fn button_color(st: &SeqTheme, param: &str, accent: Color) -> Color {
    match param {
        "mute" => st.mute,
        "solo" => st.solo,
        p if p.ends_with(".bypass") || p == "monitor.dim" => st.mute,
        _ => accent,
    }
}

#[allow(clippy::too_many_arguments, reason = "painting context")]
fn control(
    c: &mut dyn Canvas,
    st: &SeqTheme,
    pc: &PlacedCtrl,
    p: &ParamDescriptor,
    v: f32,
    accent: Color,
    enabled: bool,
    hollow: bool,
) {
    let on = v >= (p.min + p.max) / 2.0;
    match pc.kind {
        CtrlKind::Knob => knob_cell(c, st, pc, p, v, accent),
        CtrlKind::Button => {
            let h = (pc.rect.height() - 4.0).min(15.0);
            let r = Rect::new(
                pc.rect.x0 + 2.0,
                pc.rect.center().y - h / 2.0,
                pc.rect.x1 - 2.0,
                pc.rect.center().y + h / 2.0,
            );
            let col = button_color(st, pc.param, accent);
            small_button(c, st, r, &pc.label, on.then_some(col), enabled, 6.8);
            if hollow && !on {
                paint::outline(c, r.inset(-0.5), 2.5, 1.2, col);
            }
        }
        CtrlKind::Slider => {
            let col = if hollow {
                accent.with_alpha(0.4)
            } else {
                accent
            };
            slider_cell(c, st, pc.rect, p, v, col);
        }
        CtrlKind::Choice => {
            let r = Rect::new(
                pc.rect.x0 + 2.0,
                pc.rect.y0 + 3.0,
                pc.rect.x1 - 2.0,
                pc.rect.y1 - 3.0,
            );
            lcd_text(
                c,
                st,
                r,
                &widget::format_value(p, v),
                mix(accent, Color::WHITE, 0.3),
                7.0,
            );
        }
        CtrlKind::Fader => fader(c, st, pc.rect, p, v, accent),
    }
}

fn fader(c: &mut dyn Canvas, st: &SeqTheme, r: Rect, p: &ParamDescriptor, v: f32, accent: Color) {
    let t = &st.base;
    let x = r.x0 + 13.0;
    let (top, bottom) = (r.y0 + 8.0, r.y1 - 8.0);
    let marks: [(f32, &str); 7] = [
        (6.0, "+6"),
        (0.0, "0"),
        (-6.0, "-6"),
        (-12.0, "-12"),
        (-24.0, "-24"),
        (-48.0, "-48"),
        (f32::NEG_INFINITY, "-∞"),
    ];
    let mut shown = false;
    for (db, text) in marks {
        let n = if db == f32::NEG_INFINITY {
            fader_db(p, p.min)
                .filter(|d| *d == f32::NEG_INFINITY)
                .map(|_| 0.0)
        } else {
            fader_norm_at_db(p, db)
        };
        let Some(n) = n else { continue };
        shown = true;
        let y = bottom - (bottom - top) * n;
        let w = if db == 0.0 { 7.0 } else { 4.0 };
        c.stroke_with(
            Line::new(pt(x + 6.0, y), pt(x + 6.0 + w, y)),
            1.0,
            t.text_muted.with_alpha(0.7),
        );
        c.text(&TextRun::new(text, pt(r.x1 - 1.0, y + 2.5), 6.0, t.text_muted).align(Align::End));
    }
    if !shown {
        for i in 0..=4 {
            let y = top + (bottom - top) * f64::from(i) / 4.0;
            c.stroke_with(
                Line::new(pt(x + 6.0, y), pt(x + 10.0, y)),
                1.0,
                t.text_muted.with_alpha(0.6),
            );
        }
    }
    c.fill_with(
        RoundedRect::new(x - 2.5, top - 2.0, x + 2.5, bottom + 2.0, 2.5),
        t.knob_track,
    );
    let n = f64::from(p.to_normalized(v));
    let y = bottom - (bottom - top) * n;
    c.stroke_with(Line::new(pt(x, bottom), pt(x, y)), 2.0, accent);
    let cap = Rect::new(x - 11.0, y - 6.5, x + 11.0, y + 6.5);
    c.fill_with(
        RoundedRect::from_rect(cap + Vec2::new(0.0, 2.0), 2.0),
        Color::BLACK.with_alpha(0.5),
    );
    c.fill(
        &RoundedRect::from_rect(cap, 2.0).into(),
        &vello::peniko::Brush::Gradient(
            vello::peniko::Gradient::new_linear(pt(0.0, cap.y0), pt(0.0, cap.y1))
                .with_stops([t.knob_top, t.knob_bottom]),
        ),
    );
    c.stroke_with(
        Line::new(pt(cap.x0 + 3.0, y), pt(cap.x1 - 3.0, y)),
        1.4,
        t.knob_pointer,
    );
}

fn meter_color(st: &SeqTheme, db: f32) -> Color {
    if db >= -0.5 {
        st.record
    } else if db >= -9.0 {
        st.mute
    } else {
        st.solo
    }
}

/// Fills a meter bar from the bottom up to `db`, coloured by level zone (green, amber from
/// -9 dB, red from -0.5 dB).
fn zone_fill(c: &mut dyn Canvas, st: &SeqTheme, bar: Rect, db: f32, alpha: f32) {
    let top = meter_norm(db);
    if top <= 0.0 {
        return;
    }
    let zones = [
        (f32::NEG_INFINITY, -9.0f32),
        (-9.0, -0.5),
        (-0.5, f32::INFINITY),
    ];
    for (lo, hi) in zones {
        let n0 = if lo == f32::NEG_INFINITY {
            0.0
        } else {
            meter_norm(lo)
        };
        let n1 = if hi == f32::INFINITY {
            1.0
        } else {
            meter_norm(hi)
        }
        .min(top);
        if n1 <= n0 {
            continue;
        }
        let y0 = bar.y1 - bar.height() * n1;
        let y1 = bar.y1 - bar.height() * n0;
        c.fill_with(
            Rect::new(bar.x0, y0, bar.x1, y1),
            meter_color(st, lo.max(-60.0)).with_alpha(alpha),
        );
    }
}

fn meter_trough(c: &mut dyn Canvas, st: &SeqTheme, r: Rect) {
    for bar in meter_bars(r) {
        c.fill_with(RoundedRect::from_rect(bar, 1.5), st.base.lcd);
    }
    // 0 dB mark.
    let (_, b0) = meter_span(r);
    let y = b0.1 - (b0.1 - b0.0) * meter_norm(0.0);
    c.stroke_with(
        Line::new(pt(r.x0 + 12.0, y), pt(r.x1 - 12.0, y)),
        1.0,
        st.base.text_muted.with_alpha(0.7),
    );
}

/// (clip LED row, (top, bottom) of the bars).
fn meter_span(r: Rect) -> ((f64, f64), (f64, f64)) {
    ((r.y0, r.y0 + 4.0), (r.y0 + 6.0, r.y1))
}

fn meter_bars(r: Rect) -> [Rect; 2] {
    let (_, (t, b)) = meter_span(r);
    [
        Rect::new(r.x0, t, r.x0 + 11.0, b),
        Rect::new(r.x1 - 11.0, t, r.x1, b),
    ]
}

fn stereo_meter(
    c: &mut dyn Canvas,
    st: &SeqTheme,
    r: Rect,
    m: Option<&MeterBallistics>,
    silenced: bool,
) {
    let ((c0, c1), _) = meter_span(r);
    for (ch, bar) in meter_bars(r).into_iter().enumerate() {
        let inner = bar.inset(-1.5);
        let led = Rect::new(bar.x0, c0, bar.x1, c1);
        let clip = m.is_some_and(|m| m.clip(ch));
        c.fill_with(
            RoundedRect::from_rect(led, 1.0),
            if clip { st.record } else { st.base.led_off },
        );
        let Some(m) = m else { continue };
        let h = inner.height();
        let alpha = if silenced { 0.35 } else { 1.0 };
        let peak = m.peak[ch].level_db;
        let rms = m.rms[ch].level_db;
        zone_fill(c, st, inner, peak, 0.38 * alpha);
        zone_fill(c, st, inner, rms, alpha);
        let hold = m.peak[ch].hold_db;
        if meter_norm(hold) > 0.0 {
            let yh = inner.y1 - h * meter_norm(hold);
            c.fill_with(
                Rect::new(inner.x0, yh - 1.0, inner.x1, yh + 1.0),
                meter_color(st, hold).with_alpha(alpha),
            );
        }
    }
}

fn gr_bar(c: &mut dyn Canvas, st: &SeqTheme, r: Rect, gr_db: f32) {
    if gr_db <= 0.05 {
        return;
    }
    let inner = r.inset(-1.0);
    let w = inner.width() * f64::from((gr_db / GR_MAX_DB).clamp(0.0, 1.0)).sqrt();
    c.fill_with(
        Rect::new(inner.x1 - w, inner.y0, inner.x1, inner.y1),
        st.mute,
    );
}

fn gr_trough(c: &mut dyn Canvas, st: &SeqTheme, r: Rect) {
    c.fill_with(RoundedRect::from_rect(r, 1.5), st.base.lcd);
    c.text(&TextRun::new("GR", pt(r.x0 + 2.0, r.y1 - 1.5), 5.5, st.base.text_muted).weight(700));
}

fn record_strip(
    upper: &mut DisplayList,
    pinned: &mut DisplayList,
    st: &SeqTheme,
    s: &StripView,
    l: &StripLayout,
    index: usize,
) {
    let t = &st.base;
    let soloed = s.on("solo");
    for sec in &l.sections {
        if sec.id == SectionId::Name {
            continue;
        }
        let c: &mut DisplayList = if l.is_pinned(sec.id) { pinned } else { upper };
        section_header(c, st, sec.header, sec.id.title(), sec.folded);
    }
    if let Some(desc) = s.desc {
        for pc in &l.controls {
            let Some(p) = desc.params.iter().find(|p| p.key == pc.param) else {
                continue;
            };
            let c: &mut DisplayList = if l.is_pinned(pc.section) {
                pinned
            } else {
                upper
            };
            let v = s.value(p);
            let enabled = !(pc.param == "mute" && soloed);
            let hollow = (pc.param == "mute" && s.silenced)
                || (pc.kind == CtrlKind::Slider && !send_on(s, pc.param));
            control(c, st, pc, p, v, s.accent, enabled, hollow);
        }
        if let (Some(r), Some(p)) = (l.fader_lcd, desc.param_by_key("fader")) {
            lcd_text(pinned, st, r, &fader_text(p, s.value(p)), t.text, 7.5);
        }
    }
    record_inserts(upper, st, &l.insert_slots, &s.inserts, s.accent);
    if let Some(r) = l.gr_meter {
        gr_trough(upper, st, r);
    }
    if let Some(r) = l.meter {
        meter_trough(pinned, st, r);
    }
    name_plate(
        pinned,
        st,
        l.name,
        &s.name,
        &format!("CH {}", index + 1),
        s.accent,
        s.silenced,
    );
    // Latency by the name plate, only when there is some.
    if let (Some(r), Some(n)) = (l.latency, s.latency.filter(|n| *n > 0)) {
        pinned.text(
            &TextRun::new(
                format!("lat {n}"),
                pt(r.x1, r.y1 - 0.5),
                6.0,
                mix(t.cable_cv, Color::WHITE, 0.2),
            )
            .align(Align::End)
            .weight(600),
        );
    }
}

/// Draws an insert chain: one row per slot, a filled slot with its effect name and an x, the
/// first empty slot as "+ add", the rest dim and empty.
fn record_inserts(
    c: &mut dyn Canvas,
    st: &SeqTheme,
    slots: &[Rect],
    inserts: &[(DeviceKey, String)],
    accent: Color,
) {
    let t = &st.base;
    for (i, r) in slots.iter().enumerate() {
        match inserts.get(i) {
            Some((_, name)) => {
                let body = Rect::new(r.x0, r.y0, r.x1 - INSERT_X_W, r.y1);
                lcd_text(c, st, body, name, mix(accent, Color::WHITE, 0.3), 7.0);
                let x = insert_x_rect(*r);
                lcd_text(c, st, x.inset((-1.0, 0.0)), "x", t.text_muted, 7.0);
            }
            None if i == inserts.len() => lcd_text(c, st, *r, "+ add", t.text_muted, 7.0),
            None => lcd_text(c, st, *r, "—", t.text_muted.with_alpha(0.4), 7.0),
        }
    }
}

fn send_on(s: &StripView, level_key: &str) -> bool {
    let on = level_key.replace(".level", ".on");
    s.on(&on)
}

fn name_plate(
    c: &mut dyn Canvas,
    st: &SeqTheme,
    r: Rect,
    name: &str,
    caption: &str,
    accent: Color,
    dim: bool,
) {
    let t = &st.base;
    let r = r.inset((-2.0, -2.0));
    c.fill_with(RoundedRect::from_rect(r, 3.0), t.plate);
    c.fill_with(RoundedRect::new(r.x0, r.y0, r.x1, r.y0 + 3.0, 1.5), accent);
    let col = if dim { t.text_muted } else { t.text };
    c.text(
        &TextRun::new(
            fit_label(name, r.width() - 8.0, 10.0),
            pt(r.x0 + 5.0, r.y0 + 15.0),
            10.0,
            col,
        )
        .weight(700),
    );
    c.text(
        &TextRun::new(caption, pt(r.x0 + 5.0, r.y1 - 3.5), 6.0, t.text_muted)
            .weight(600)
            .spacing(0.8),
    );
    let led = pt(r.x1 - 7.0, r.y1 - 6.0);
    c.fill_with(Circle::new(led, 2.2), if dim { t.led_off } else { accent });
}

fn record_master(
    upper: &mut DisplayList,
    pinned: &mut DisplayList,
    st: &SeqTheme,
    m: &StripView,
    l: &MasterLayout,
    return_names: &[String],
    (any_solo, any_mute): (bool, bool),
) {
    let t = &st.base;
    for sec in &l.sections {
        if sec.id == SectionId::Name {
            continue;
        }
        let c: &mut DisplayList = if sec.rect.x0 >= l.fader_x {
            pinned
        } else {
            upper
        };
        section_header(c, st, sec.header, sec.id.title(), false);
    }
    // Returns legend and slot numbers.
    if let Some(&(_, first)) = l.return_pickers.first().as_ref() {
        let y = first.y0 - 9.0;
        for (text, x) in [
            ("EFFECT", 56.0),
            ("LEVEL", 112.0),
            ("PAN", 142.0),
            ("M", 170.0),
        ] {
            upper.text(
                &TextRun::new(text, pt(x, y), 5.8, t.text_muted)
                    .align(Align::Center)
                    .spacing(0.6),
            );
        }
    }
    for (slot, r) in &l.return_pickers {
        upper.text(
            &TextRun::new(
                format!("{}", slot + 1),
                pt(8.0, r.center().y + 3.0),
                8.0,
                t.text_muted,
            )
            .align(Align::Center)
            .weight(700),
        );
        let name = return_names.get(*slot).map_or("", String::as_str);
        let (text, col) = if name.is_empty() {
            ("—", t.text_muted)
        } else {
            (name, mix(t.highlight, Color::WHITE, 0.2))
        };
        lcd_text(upper, st, *r, text, col, 7.5);
    }
    if let Some(desc) = m.desc {
        for pc in &l.controls {
            let Some(p) = desc.params.iter().find(|p| p.key == pc.param) else {
                continue;
            };
            let c: &mut DisplayList = if pc.rect.x0 >= l.fader_x {
                pinned
            } else {
                upper
            };
            // An FX return without an effect is drawn subdued.
            let unassigned = pc.section == SectionId::Returns
                && pc
                    .param
                    .strip_prefix("return")
                    .and_then(|r| r.split('.').next())
                    .and_then(|n| n.parse::<usize>().ok())
                    .is_some_and(|n| return_names.get(n - 1).is_none_or(String::is_empty));
            let accent = if unassigned {
                mix(t.text_muted, t.panel_bottom, 0.3)
            } else {
                t.highlight
            };
            control(c, st, pc, p, m.value(p), accent, true, false);
        }
        if let (Some(r), Some(p)) = (l.fader_lcd, desc.param_by_key("fader")) {
            lcd_text(pinned, st, r, &fader_text(p, m.value(p)), t.text, 7.5);
        }
    }
    record_inserts(upper, st, &l.insert_slots, &m.inserts, t.highlight);
    if let Some(r) = l.gr_meter {
        gr_trough(upper, st, r);
    }
    if let Some(r) = l.meter {
        meter_trough(pinned, st, r);
    }
    small_button(
        pinned,
        st,
        l.clear_solo,
        "CLR S",
        any_solo.then_some(st.solo),
        true,
        6.8,
    );
    small_button(
        pinned,
        st,
        l.clear_mute,
        "CLR M",
        any_mute.then_some(st.mute),
        true,
        6.8,
    );
    name_plate(pinned, st, l.name, "MASTER", "MAIN OUT", t.highlight, false);
}

fn paint_menu(c: &mut dyn Canvas, st: &SeqTheme, m: &Menu) {
    let t = &st.base;
    let r = m.rect();
    c.fill_with(
        RoundedRect::from_rect(r.inset(2.0) + Vec2::new(0.0, 3.0), 5.0),
        t.cable_shadow,
    );
    c.fill_with(RoundedRect::from_rect(r, 4.0), t.browser_bg);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 4.5),
        1.0,
        t.bevel_dark,
    );
    for i in 0..m.rows() {
        let Some(rr) = m.row_rect(i) else { continue };
        let disabled = m.items[i].1 == MenuItem::Disabled;
        if m.hover == Some(i) && !disabled {
            c.fill_with(RoundedRect::from_rect(rr, 3.0), t.browser_row);
        }
        let text = fit_label(&m.items[i].0, MENU_W - 24.0, 10.0);
        let col = if disabled { t.text_muted } else { t.text };
        sp::label(c, &text, rr.x0 + 8.0, rr.y0 + 14.0, 10.0, col, Align::Start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::ParamId;

    const POS: ParamDescriptor = ParamDescriptor {
        id: ParamId(0),
        key: "fader",
        name: "Fader",
        min: 0.0,
        max: 1.0,
        default: 0.75,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    };
    const DB: ParamDescriptor = ParamDescriptor {
        min: -60.0,
        max: 6.0,
        default: 0.0,
        taper: Taper::Decibel,
        unit: Unit::Db,
        ..POS
    };
    const HZ: ParamDescriptor = ParamDescriptor {
        min: 20.0,
        max: 20_000.0,
        default: 1000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        ..POS
    };

    #[test]
    fn position_faders_use_the_s04_cubic_taper() {
        assert!(fader_db(&POS, 0.75).unwrap().abs() < 1e-5);
        assert!((fader_db(&POS, 1.0).unwrap() - 7.4963).abs() < 1e-3);
        assert_eq!(fader_db(&POS, 0.0), Some(f32::NEG_INFINITY));
        // Half the position below unity is three times the dB of an amplitude halving.
        assert!((fader_db(&POS, 0.375).unwrap() + 18.0618).abs() < 1e-3);
        assert_eq!(fader_text(&POS, 0.75), "0.0 dB");
        assert_eq!(fader_text(&POS, 1.0), "+7.5 dB");
        assert_eq!(fader_text(&POS, 0.0), "-inf dB");
    }

    #[test]
    fn db_faders_show_their_plain_value() {
        assert_eq!(fader_db(&DB, -6.0), Some(-6.0));
        assert_eq!(fader_db(&DB, -60.0), Some(f32::NEG_INFINITY));
        assert_eq!(fader_text(&DB, -6.0), "-6.0 dB");
        assert_eq!(fader_text(&DB, 3.0), "+3.0 dB");
        // Other units fall back to their own formatting.
        assert_eq!(fader_db(&HZ, 440.0), None);
        assert_eq!(fader_text(&HZ, 440.0), "440 Hz");
    }

    #[test]
    fn scale_marks_invert_the_display_mapping() {
        let n = fader_norm_at_db(&POS, 0.0).unwrap();
        assert!((n - 0.75).abs() < 1e-6, "{n}");
        let n = fader_norm_at_db(&POS, -6.0).unwrap();
        assert!((fader_db(&POS, POS.from_normalized(n as f32)).unwrap() + 6.0).abs() < 1e-3);
        let n = fader_norm_at_db(&DB, -6.0).unwrap();
        assert!((n - 54.0 / 66.0).abs() < 1e-6, "{n}");
        assert_eq!(fader_norm_at_db(&POS, 12.0), None);
        assert_eq!(fader_norm_at_db(&HZ, 0.0), None);
    }

    #[test]
    fn f5_toggles_and_shift_f5_maximises() {
        assert_eq!(
            console_shortcut(&Key::F(5), Modifiers::NONE),
            Some(UiAction::ToggleConsole)
        );
        assert_eq!(
            console_shortcut(&Key::F(5), Modifiers::SHIFT),
            Some(UiAction::MaximiseConsole)
        );
        assert_eq!(console_shortcut(&Key::F(5), Modifiers::CTRL), None);
        assert_eq!(console_shortcut(&Key::F(6), Modifiers::NONE), None);
        assert_eq!(console_shortcut(&Key::Char('m'), Modifiers::CTRL), None);
    }
}
