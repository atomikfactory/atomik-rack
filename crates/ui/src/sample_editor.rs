//! The sample editor: a pane that replaces the arrangement and edits one
//! sampler's zones.
//!
//! Hosting follows [`crate::PianoRollView`]: on [`UiAction::OpenSampleEditor`] call
//! [`SampleEditorView::open`] and show the view in place of the arrangement; on
//! [`UiAction::CloseSampleEditor`] show the arrangement again (also when
//! [`SampleEditorView::is_present`] turns false because the sampler was deleted). Peaks of the
//! zones' assets come from the app ([`SampleEditorView::set_peaks`], the same data as the
//! arrangement's); assets whose file is missing are reported with
//! [`SampleEditorView::set_missing_assets`] and drawn as missing.
//!
//! # Layout (logical pixels, inside the bounds)
//!
//! ```text
//! top bar          SAMPLE EDITOR   <device> · <n zones> · <size>          [Load...] [x]
//! key x velocity map (keys 0..=127 left to right, velocity 127 at the top down to 1)   zone list
//! keyboard strip (click to audition)                                                   (rows)
//! loop mode / trigger selectors                                         <asset info>
//! waveform of the selected zone with start / end / loop / crossfade markers
//! numeric fields (root, transpose, tune, gain, pan, key lo/hi, vel lo/hi, crossfade)
//! ```
//!
//! # Interaction
//!
//! * **Map.** A zone is a rectangle (key range × velocity range; layered zones are
//!   translucent, the selected one is outlined and drawn last, the root key is a line with a
//!   small tip, a missing asset is drawn in the record colour). Drag an edge to change the key
//!   or velocity range (corners move two), drag the body to move the key range keeping its
//!   width. Click selects and auditions the key under the pointer at the velocity of the
//!   pointer height ([`UiAction::AuditionVelocity`]); a click on empty map or on the keyboard
//!   strip auditions (the strip at velocity 100); moving the pointer while held glides to the
//!   next key. The key axis fits 0..=127 at first; Ctrl/Cmd+wheel zooms it around the pointer
//!   and the wheel scrolls it.
//! * **Zone list.** One row per zone (file name, key range, velocity range, root); click
//!   selects, Delete or Backspace removes the selected zone, the wheel scrolls.
//! * **Waveform.** Drag the start / end markers (top flags) and loop start / loop end markers
//!   (bottom flags, shown when the loop is on). The crossfade (the region before the loop
//!   start) has a handle at its left edge once it is longer than 0; Alt+drag on the loop start
//!   marker grows it from 0. The whole asset is visible at first; Ctrl/Cmd+wheel zooms and the
//!   wheel scrolls. Buttons set the loop mode (off, forward, ping-pong) and the trigger mode
//!   (gate, one-shot).
//! * **Fields.** Drag a field vertically to change it (Shift = fine), double-click resets it to
//!   the [`ZoneDoc::new`] default, the wheel steps it.
//! * **Edits.** Every change emits `Command::SetZones` with the full, valid new list ([`crate::zone_edit`]
//!   clamps every edit into the SDK's rules). A drag uses one merge id for the whole gesture
//!   (ids from 4·2⁴⁰, so they never collide with the other views) and ends with
//!   [`UiAction::EndMerge`]; Esc cancels a drag, restoring the start values.
//! * **Drop.** [`UiEvent::FileHover`] / [`UiEvent::FileDrop`] inside the bounds emit
//!   [`UiAction::ImportSamples`] with `at_key` set over the map and the keyboard strip and
//!   `None` elsewhere; a drop indicator shows the target meanwhile.
//! * The **Load...** button in the top bar emits [`UiAction::LoadSamplesDialog`] (the host
//!   shows the file dialog and imports like a drop with `at_key: None`).
//! * Esc (with nothing to cancel or deselect) and the close button emit
//!   [`UiAction::CloseSampleEditor`]; Ctrl/Cmd+Z / Y undo and redo.

use crate::action::UiAction;
use crate::canvas::Canvas;
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::keymap::CommandOutcome;
use crate::seq_paint::SeqTheme;
use crate::theme::Theme;
use crate::wave_paint::max_wave_zoom;
use crate::zone_edit::{self, Edges, Field, Marker};
use atomik_audio_file::PeakPyramid;
use atomik_device_sdk::{LoopMode, Registry, TriggerMode};
use atomik_model::{AssetMeta, Command, DeviceKey, RackDoc, ZoneDoc};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use vello::kurbo::{Point, Rect};
use vello::peniko::Color;

mod paint;

/// Height of the top bar.
pub const TOP_BAR_HEIGHT: f64 = 28.0;
/// Width of the velocity axis left of the map and the waveform.
pub const AXIS_WIDTH: f64 = 30.0;
/// Height of the keyboard strip under the map.
pub const KEYBOARD_HEIGHT: f64 = 24.0;
/// Width of the zone list.
pub const LIST_WIDTH: f64 = 232.0;
/// Height of one zone list row.
pub const ROW_HEIGHT: f64 = 30.0;
/// Height of the list header.
pub const LIST_HEAD: f64 = 20.0;
/// Height of the loop / trigger selector band above the waveform.
pub const HEAD_HEIGHT: f64 = 26.0;
/// Width of one numeric field cell.
pub const FIELD_WIDTH: f64 = 78.0;
/// Height of one row of numeric fields.
pub const FIELD_HEIGHT: f64 = 40.0;
const PAD: f64 = 6.0;
const EDGE_PX: f64 = 5.0;
const DRAG_THRESHOLD: f64 = 2.0;
const SCROLL_STEP: f64 = 36.0;
const ZOOM_STEP: f64 = 1.15;
const MAX_KEY_PX: f64 = 48.0;
const MERGE_BASE: u64 = 4 << 40;
/// Velocity of an audition from the keyboard strip.
const STRIP_VELOCITY: u8 = 100;
const FINE_FACTOR: f64 = 0.1;

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    None,
    Close,
    Load,
    Zone(usize, Edges),
    Map(u8, u8),
    Keys(u8),
    Row(usize),
    Loop(LoopMode),
    Trigger(TriggerMode),
    Marker(Marker),
    Field(Field),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EditKind {
    Zone(Edges),
    /// A marker and the offset from the pointer to it at the press.
    Marker(Marker, f64),
    /// A field and its value at the press.
    Field(Field, f64),
}

#[derive(Debug, Clone)]
struct EditDrag {
    kind: EditKind,
    index: usize,
    start: Point,
    orig: Vec<ZoneDoc>,
    last: Vec<ZoneDoc>,
    merge: u64,
    frames: u64,
    rate: u32,
    moved: bool,
    emitted: bool,
    sounding: Option<(u8, u8)>,
}

#[derive(Debug, Clone)]
enum Drag {
    None,
    /// Auditioning a key (keyboard strip or empty map).
    Keys {
        key: u8,
        velocity: Option<u8>,
    },
    Edit(Box<EditDrag>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DropHover {
    at_key: Option<u8>,
    count: usize,
}

/// The rectangles of the view, from its bounds.
#[derive(Debug, Clone, Copy)]
struct Layout {
    top: Rect,
    close: Rect,
    load: Rect,
    map: Rect,
    keys: Rect,
    list: Rect,
    head: Rect,
    wave: Rect,
    fields: Rect,
    per_row: usize,
}

/// Numbers the drags need, read before borrowing the drag state.
#[derive(Debug, Clone, Copy)]
struct Geo {
    map_x0: f64,
    ppk: f64,
    scroll_x: f64,
    ppv: f64,
    wave_x0: f64,
    wave_scroll: f64,
    fpp: f64,
}

impl Geo {
    fn key_at(&self, x: f64) -> u8 {
        ((x - self.map_x0 + self.scroll_x) / self.ppk)
            .floor()
            .clamp(0.0, 127.0) as u8
    }
}

/// The sample editor. See the module documentation.
#[derive(Debug)]
pub struct SampleEditorView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    device: Option<DeviceKey>,
    present: bool,
    name: String,
    accent: Color,
    zones: Vec<ZoneDoc>,
    metas: BTreeMap<String, AssetMeta>,
    peaks: BTreeMap<String, Arc<PeakPyramid>>,
    /// Samples of the assets held in RAM (the waveform reads them below the finest peak level).
    samples: BTreeMap<String, Arc<[Vec<f32>]>>,
    missing: BTreeSet<String>,
    selected: Option<usize>,
    pick_first: bool,
    zoom: f64,
    scroll_x: f64,
    wave_zoom: f64,
    wave_scroll: f64,
    list_scroll: f64,
    drag: Drag,
    hover: Hit,
    drop: Option<DropHover>,
    next_merge: u64,
    focused: bool,
    repaint: bool,
}

impl SampleEditorView {
    /// A closed editor; call [`Self::set_bounds`], [`Self::open`] and [`Self::sync`].
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
            accent,
            zones: Vec::new(),
            metas: BTreeMap::new(),
            peaks: BTreeMap::new(),
            samples: BTreeMap::new(),
            missing: BTreeSet::new(),
            selected: None,
            pick_first: false,
            zoom: 1.0,
            scroll_x: 0.0,
            wave_zoom: 1.0,
            wave_scroll: 0.0,
            list_scroll: 0.0,
            drag: Drag::None,
            hover: Hit::None,
            drop: None,
            next_merge: MERGE_BASE,
            focused: false,
            repaint: true,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Opens the editor on a sampler (the first zone is selected on the next sync). The
    /// peaks and missing-asset set the host handed in stay.
    pub fn open(&mut self, device: DeviceKey) {
        if self.device.as_ref() != Some(&device) {
            self.selected = None;
            self.zones.clear();
            self.zoom = 1.0;
            self.scroll_x = 0.0;
            self.list_scroll = 0.0;
        }
        self.device = Some(device);
        self.present = false;
        self.pick_first = true;
        self.drag = Drag::None;
        self.drop = None;
        self.reset_wave();
        self.repaint = true;
    }

    /// Closes the editor (it then ignores input and paints an empty state).
    pub fn close(&mut self) {
        self.device = None;
        self.present = false;
        self.zones.clear();
        self.metas.clear();
        self.selected = None;
        self.drag = Drag::None;
        self.drop = None;
        self.repaint = true;
    }

    /// The sampler being edited.
    pub fn device(&self) -> Option<&DeviceKey> {
        self.device.as_ref()
    }

    /// True while the open device exists in the document and takes samples. When it turns
    /// false (the sampler was deleted), the host shows the arrangement again.
    pub fn is_present(&self) -> bool {
        self.present
    }

    /// Places the view in the window (logical pixels).
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.clamp_scrolls();
            self.repaint = true;
        }
    }

    /// The view's rectangle.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Sets the display scale factor (also set by [`UiEvent::ScaleChanged`]).
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

    /// Sets the waveform peaks of an asset (its content hash), replacing earlier ones; shared
    /// with the arrangement's [`crate::ArrangementView::set_peaks`] data.
    pub fn set_peaks(&mut self, asset: &str, peaks: Arc<PeakPyramid>) {
        self.peaks.insert(asset.to_string(), peaks);
        self.repaint = true;
    }

    /// Sets the samples of an asset held in RAM (planar, one `Vec` per channel, shared with the
    /// engine), which the waveform reads below the finest peak level. Assets without samples
    /// (streamed from disk) keep the finest level at every zoom.
    pub fn set_samples(&mut self, asset: &str, samples: Arc<[Vec<f32>]>) {
        self.samples.insert(asset.to_string(), samples);
        self.repaint = true;
    }

    /// Forgets the peaks of an asset.
    pub fn clear_peaks(&mut self, asset: &str) {
        let had_samples = self.samples.remove(asset).is_some();
        if self.peaks.remove(asset).is_some() || had_samples {
            self.repaint = true;
        }
    }

    /// Sets the assets whose audio is missing: their zones stay, are drawn as
    /// missing and have no waveform. An asset that is not in the document's pool counts as
    /// missing too.
    pub fn set_missing_assets(&mut self, assets: impl IntoIterator<Item = String>) {
        let set: BTreeSet<String> = assets.into_iter().collect();
        if set != self.missing {
            self.missing = set;
            self.repaint = true;
        }
    }

    /// The zones of the open sampler, as last synced.
    pub fn zones(&self) -> &[ZoneDoc] {
        &self.zones
    }

    /// The selected zone (index into [`Self::zones`]).
    pub fn selected_zone(&self) -> Option<usize> {
        self.selected
    }

    /// Selects a zone (or none).
    pub fn set_selected_zone(&mut self, zone: Option<usize>) {
        let zone = zone.filter(|i| *i < self.zones.len());
        if zone != self.selected {
            self.selected = zone;
            self.reset_wave();
            self.repaint = true;
        }
    }

    /// The drop indicator while files hover over the view: `Some(Some(key))` over the key map
    /// and the keyboard strip, `Some(None)` elsewhere in the view.
    pub fn drop_hover(&self) -> Option<Option<u8>> {
        self.drop.map(|d| d.at_key)
    }

    // ------------------------------------------------------------------ geometry (public, for tests and hosts)

    /// The key × velocity map.
    pub fn map_rect(&self) -> Rect {
        self.layout().map
    }

    /// The keyboard strip.
    pub fn keyboard_rect(&self) -> Rect {
        self.layout().keys
    }

    /// The zone list.
    pub fn list_rect(&self) -> Rect {
        self.layout().list
    }

    /// The waveform.
    pub fn waveform_rect(&self) -> Rect {
        self.layout().wave
    }

    /// Centre of the close button.
    pub fn close_button_center(&self) -> Point {
        self.layout().close.center()
    }

    /// Centre of the Load button.
    pub fn load_button_center(&self) -> Point {
        self.layout().load.center()
    }

    /// Logical x of the left edge of a key.
    pub fn key_x(&self, key: u8) -> f64 {
        let g = self.geo();
        g.map_x0 + f64::from(key) * g.ppk - g.scroll_x
    }

    /// Logical x of the centre of a key.
    pub fn key_center_x(&self, key: u8) -> f64 {
        self.key_x(key) + self.geo().ppk / 2.0
    }

    /// The key under a logical x (clamped to 0..=127).
    pub fn key_at(&self, x: f64) -> u8 {
        self.geo().key_at(x)
    }

    /// Logical y of the centre of a velocity step (1..=127).
    pub fn velocity_y(&self, velocity: u8) -> f64 {
        let m = self.layout().map;
        m.y0 + (127.5 - f64::from(velocity.clamp(1, 127))) / 127.0 * m.height()
    }

    /// The velocity under a logical y (clamped to 1..=127).
    pub fn velocity_at(&self, y: f64) -> u8 {
        let m = self.layout().map;
        (127.0 - ((y - m.y0) / m.height().max(1.0) * 127.0).floor()).clamp(1.0, 127.0) as u8
    }

    /// A point on the keyboard strip for `key`.
    pub fn keyboard_point(&self, key: u8) -> Point {
        Point::new(self.key_center_x(key), self.layout().keys.center().y)
    }

    /// A zone's rectangle in the map.
    pub fn zone_rect(&self, index: usize) -> Option<Rect> {
        let z = self.zones.get(index)?;
        Some(self.rect_of(z))
    }

    /// Centre of a row of the zone list.
    pub fn list_row_center(&self, index: usize) -> Option<Point> {
        (index < self.zones.len()).then(|| self.row_rect(index).center())
    }

    /// Centre of a numeric field's value box.
    pub fn field_center(&self, field: Field) -> Point {
        self.field_value_rect(field).center()
    }

    /// Centre of a loop mode button.
    pub fn loop_button_center(&self, mode: LoopMode) -> Point {
        self.loop_rect(mode).center()
    }

    /// Centre of a trigger mode button.
    pub fn trigger_button_center(&self, mode: TriggerMode) -> Point {
        self.trigger_rect(mode).center()
    }

    /// Where a marker of the selected zone sits on the waveform (x at its frame, y at its
    /// flag), `None` without a selected zone with a known asset.
    pub fn marker_point(&self, marker: Marker) -> Option<Point> {
        let (_, z, _) = self.selected_with_meta()?;
        let l = self.layout();
        let x = self.frame_x(zone_edit::marker_frame(z, marker));
        let y = match marker {
            Marker::Start | Marker::End => l.wave.y0 + 6.0,
            _ => l.wave.y1 - 6.0,
        };
        Some(Point::new(x, y))
    }

    /// The frame under a logical x of the waveform.
    pub fn x_to_frame(&self, x: f64) -> f64 {
        let g = self.geo();
        (x - g.wave_x0) * g.fpp + g.wave_scroll
    }

    // ------------------------------------------------------------------ sync

    /// Reads the open sampler from the document. Emits nothing.
    pub fn sync(&mut self, doc: &RackDoc, registry: &Registry) {
        let Some(key) = &self.device else { return };
        let dev = doc
            .resolve(key)
            .filter(|d| registry.takes_samples(&d.type_id));
        let Some(d) = dev else {
            if self.present {
                self.present = false;
                self.zones.clear();
                self.metas.clear();
                self.selected = None;
                self.drag = Drag::None;
                self.repaint = true;
            }
            return;
        };
        let metas: BTreeMap<String, AssetMeta> = d
            .zones
            .iter()
            .filter_map(|z| {
                doc.assets
                    .get(&z.asset)
                    .map(|m| (z.asset.clone(), m.clone()))
            })
            .collect();
        let accent = self.st.base.accent_for(d);
        if !self.present
            || self.zones != d.zones
            || self.metas != metas
            || self.name != d.name
            || self.accent != accent
        {
            self.present = true;
            self.zones.clone_from(&d.zones);
            self.metas = metas;
            self.name.clone_from(&d.name);
            self.accent = accent;
            self.repaint = true;
        }
        let n = self.zones.len();
        if let Some(s) = self.selected
            && s >= n
        {
            self.selected = n.checked_sub(1);
            self.reset_wave();
        }
        if self.pick_first && self.selected.is_none() && n > 0 {
            self.selected = Some(0);
            self.reset_wave();
        }
        if n > 0 {
            self.pick_first = false;
        }
        self.clamp_scrolls();
    }

    fn reset_wave(&mut self) {
        self.wave_zoom = 1.0;
        self.wave_scroll = 0.0;
    }

    // ------------------------------------------------------------------ geometry

    fn layout(&self) -> Layout {
        let b = self.bounds;
        let rect = |x0: f64, y0: f64, x1: f64, y1: f64| Rect::new(x0, y0, x1.max(x0), y1.max(y0));
        let top = rect(b.x0, b.y0, b.x1, b.y0 + TOP_BAR_HEIGHT);
        let close = Rect::new(b.x1 - 30.0, b.y0 + 5.0, b.x1 - 8.0, b.y0 + 23.0);
        let load = Rect::new(close.x0 - 66.0, b.y0 + 5.0, close.x0 - 6.0, b.y0 + 23.0);
        let per_row = (((b.width() - 2.0 * PAD) / FIELD_WIDTH).floor() as usize).max(1);
        let rows = Field::ALL.len().div_ceil(per_row);
        let fields_h = rows as f64 * FIELD_HEIGHT + 10.0;
        let body0 = top.y1;
        let body1 = (b.y1 - fields_h).max(body0);
        let body_h = body1 - body0;
        let map_total = (body_h * 0.42).clamp(96.0, 260.0).min(body_h * 0.7);
        let list_w = LIST_WIDTH.min(b.width() / 3.0);
        let list = rect(b.x1 - list_w, body0, b.x1, body0 + map_total);
        let map = rect(
            b.x0 + AXIS_WIDTH,
            body0 + 4.0,
            list.x0 - PAD,
            body0 + map_total - KEYBOARD_HEIGHT,
        );
        let keys = rect(map.x0, map.y1, map.x1, map.y1 + KEYBOARD_HEIGHT);
        let head = rect(
            b.x0,
            body0 + map_total,
            b.x1,
            body0 + map_total + HEAD_HEIGHT,
        );
        let wave = rect(b.x0 + AXIS_WIDTH, head.y1, b.x1 - PAD, body1 - 4.0);
        let fields = rect(b.x0, body1, b.x1, b.y1);
        Layout {
            top,
            close,
            load,
            map,
            keys,
            list,
            head,
            wave,
            fields,
            per_row,
        }
    }

    fn fit_ppk(map: &Rect) -> f64 {
        (map.width() / 128.0).max(0.5)
    }

    fn geo(&self) -> Geo {
        let l = self.layout();
        let fit = Self::fit_ppk(&l.map);
        let frames = self.selected_with_meta().map_or(1, |(_, _, m)| m.frames);
        Geo {
            map_x0: l.map.x0,
            ppk: fit * self.zoom,
            scroll_x: self.scroll_x,
            ppv: (l.map.height() / 127.0).max(0.1),
            wave_x0: l.wave.x0,
            wave_scroll: self.wave_scroll,
            fpp: frames as f64 / (l.wave.width().max(1.0) * self.wave_zoom),
        }
    }

    fn clamp_scrolls(&mut self) {
        let l = self.layout();
        let fit = Self::fit_ppk(&l.map);
        self.zoom = self.zoom.clamp(1.0, (MAX_KEY_PX / fit).max(1.0));
        let max = (128.0 * fit * self.zoom - l.map.width()).max(0.0);
        self.scroll_x = self.scroll_x.clamp(0.0, max);
        let frames = self.selected_with_meta().map_or(0, |(_, _, m)| m.frames);
        self.wave_zoom = self
            .wave_zoom
            .clamp(1.0, max_wave_zoom(frames as f64, l.wave.width()));
        let span = frames as f64 / self.wave_zoom;
        let max = (frames as f64 - span).max(0.0);
        self.wave_scroll = self.wave_scroll.clamp(0.0, max);
        let content = self.zones.len() as f64 * ROW_HEIGHT;
        let view = (l.list.height() - LIST_HEAD).max(0.0);
        self.list_scroll = self.list_scroll.clamp(0.0, (content - view).max(0.0));
    }

    fn rect_of(&self, z: &ZoneDoc) -> Rect {
        let l = self.layout();
        let g = self.geo();
        let x = |k: f64| g.map_x0 + k * g.ppk - g.scroll_x;
        let h = l.map.height();
        Rect::new(
            x(f64::from(z.key_lo)),
            l.map.y0 + (127.0 - f64::from(z.vel_hi)) / 127.0 * h,
            x(f64::from(z.key_hi) + 1.0),
            l.map.y0 + (128.0 - f64::from(z.vel_lo)) / 127.0 * h,
        )
    }

    fn row_rect(&self, i: usize) -> Rect {
        let l = self.layout();
        let y = l.list.y0 + LIST_HEAD + i as f64 * ROW_HEIGHT - self.list_scroll;
        Rect::new(l.list.x0 + 2.0, y, l.list.x1 - 2.0, y + ROW_HEIGHT - 2.0)
    }

    fn field_rect(&self, field: Field) -> Rect {
        let l = self.layout();
        let i = Field::ALL.iter().position(|f| *f == field).unwrap_or(0);
        let (col, row) = (i % l.per_row, i / l.per_row);
        let x = l.fields.x0 + PAD + col as f64 * FIELD_WIDTH;
        let y = l.fields.y0 + 5.0 + row as f64 * FIELD_HEIGHT;
        Rect::new(x, y, x + FIELD_WIDTH - 6.0, y + FIELD_HEIGHT - 4.0)
    }

    fn field_value_rect(&self, field: Field) -> Rect {
        let r = self.field_rect(field);
        Rect::new(r.x0, r.y0 + 12.0, r.x1, r.y1)
    }

    fn loop_rect(&self, mode: LoopMode) -> Rect {
        let l = self.layout();
        let i = match mode {
            LoopMode::Off => 0.0,
            LoopMode::Forward => 1.0,
            LoopMode::PingPong => 2.0,
        };
        let x = l.head.x0 + AXIS_WIDTH + 34.0 + i * 44.0;
        Rect::new(x, l.head.y0 + 4.0, x + 40.0, l.head.y1 - 3.0)
    }

    fn trigger_rect(&self, mode: TriggerMode) -> Rect {
        let l = self.layout();
        let i = match mode {
            TriggerMode::Gate => 0.0,
            TriggerMode::OneShot => 1.0,
        };
        let x = l.head.x0 + AXIS_WIDTH + 228.0 + i * 56.0;
        Rect::new(x, l.head.y0 + 4.0, x + 52.0, l.head.y1 - 3.0)
    }

    /// The selected zone with its asset (known, and not reported missing).
    fn selected_with_meta(&self) -> Option<(usize, &ZoneDoc, &AssetMeta)> {
        let i = self.selected?;
        let z = self.zones.get(i)?;
        let m = self.metas.get(&z.asset)?;
        (!self.missing.contains(&z.asset)).then_some((i, z, m))
    }

    fn is_missing(&self, z: &ZoneDoc) -> bool {
        self.missing.contains(&z.asset) || !self.metas.contains_key(&z.asset)
    }

    /// Frames of a zone's asset (0 when unknown, which skips the frame clamps).
    fn frames_of(&self, z: &ZoneDoc) -> u64 {
        self.metas.get(&z.asset).map_or(0, |m| m.frames)
    }

    fn rate_of(&self, z: &ZoneDoc) -> u32 {
        self.metas.get(&z.asset).map_or(48_000, |m| m.sample_rate)
    }

    fn frame_x(&self, frame: u64) -> f64 {
        let g = self.geo();
        g.wave_x0 + (frame as f64 - g.wave_scroll) / g.fpp
    }

    fn marker_visible(z: &ZoneDoc, marker: Marker) -> bool {
        match marker {
            Marker::Start | Marker::End => true,
            Marker::LoopStart | Marker::LoopEnd => z.loop_mode != LoopMode::Off,
            Marker::Crossfade => z.loop_mode != LoopMode::Off && z.crossfade > 0,
        }
    }

    fn hit(&self, pos: Point) -> Hit {
        let l = self.layout();
        if !self.bounds.contains(pos) {
            return Hit::None;
        }
        if l.top.contains(pos) {
            return if l.close.contains(pos) {
                Hit::Close
            } else if l.load.contains(pos) && self.present {
                Hit::Load
            } else {
                Hit::None
            };
        }
        if !self.present {
            return Hit::None;
        }
        if l.map.contains(pos) {
            return self.hit_map(pos);
        }
        if l.keys.contains(pos) {
            return Hit::Keys(self.key_at(pos.x));
        }
        if l.list.contains(pos) {
            let first = l.list.y0 + LIST_HEAD;
            if pos.y >= first {
                let i = ((pos.y - first + self.list_scroll) / ROW_HEIGHT).floor() as usize;
                if i < self.zones.len() {
                    return Hit::Row(i);
                }
            }
            return Hit::None;
        }
        if l.head.contains(pos) {
            for m in [LoopMode::Off, LoopMode::Forward, LoopMode::PingPong] {
                if self.loop_rect(m).contains(pos) {
                    return Hit::Loop(m);
                }
            }
            for m in [TriggerMode::Gate, TriggerMode::OneShot] {
                if self.trigger_rect(m).contains(pos) {
                    return Hit::Trigger(m);
                }
            }
            return Hit::None;
        }
        if l.wave.inflate(EDGE_PX, 0.0).contains(pos)
            && let Some(m) = self.marker_at(pos)
        {
            return Hit::Marker(m);
        }
        if l.fields.contains(pos) {
            for f in Field::ALL {
                if self.field_rect(f).contains(pos) {
                    return Hit::Field(f);
                }
            }
        }
        Hit::None
    }

    fn hit_map(&self, pos: Point) -> Hit {
        let order = self.selected.into_iter().chain(
            (0..self.zones.len())
                .rev()
                .filter(|i| Some(*i) != self.selected),
        );
        for i in order {
            let r = self.rect_of(&self.zones[i]);
            if !r.inflate(EDGE_PX / 2.0, EDGE_PX / 2.0).contains(pos) {
                continue;
            }
            let ex = EDGE_PX.min(r.width() / 3.0);
            let ey = EDGE_PX.min(r.height() / 3.0);
            let edges = Edges {
                left: pos.x <= r.x0 + ex,
                right: pos.x >= r.x1 - ex,
                top: pos.y <= r.y0 + ey,
                bottom: pos.y >= r.y1 - ey,
            };
            return Hit::Zone(i, edges);
        }
        Hit::Map(self.key_at(pos.x), self.velocity_at(pos.y))
    }

    fn marker_at(&self, pos: Point) -> Option<Marker> {
        let (_, z, _) = self.selected_with_meta()?;
        let l = self.layout();
        let bottom_half = pos.y > l.wave.center().y;
        [
            Marker::Start,
            Marker::End,
            Marker::LoopStart,
            Marker::LoopEnd,
            Marker::Crossfade,
        ]
        .into_iter()
        .filter(|m| Self::marker_visible(z, *m))
        .filter_map(|m| {
            let d = (self.frame_x(zone_edit::marker_frame(z, m)) - pos.x).abs();
            let loop_type = !matches!(m, Marker::Start | Marker::End);
            (d <= EDGE_PX).then_some((loop_type != bottom_half, d, m))
        })
        .min_by(|a, b| {
            (a.0, a.1)
                .partial_cmp(&(b.0, b.1))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(_, _, m)| m)
    }

    fn merge_id(&mut self) -> u64 {
        let m = self.next_merge;
        self.next_merge += 1;
        m
    }

    fn set_zones(&self, zones: Vec<ZoneDoc>, merge: Option<u64>) -> Option<UiAction> {
        Some(UiAction::Command(Command::SetZones {
            device: self.device.clone()?,
            zones,
            merge,
        }))
    }

    fn audition(&self, key: u8, velocity: u8, on: bool) -> Option<UiAction> {
        Some(UiAction::AuditionVelocity {
            device: self.device.clone()?,
            key,
            velocity,
            on,
        })
    }

    /// `zones` with zone `i` replaced.
    fn replaced(&self, i: usize, z: ZoneDoc) -> Vec<ZoneDoc> {
        let mut list = self.zones.clone();
        list[i] = z;
        list
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
        if self.device.is_none() {
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
            UiEvent::PointerUp { .. } => self.pointer_up(&mut out),
            UiEvent::PointerLeave => {
                self.hover = Hit::None;
                self.repaint = true;
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => self.wheel(delta.x, delta.y, *pos, *modifiers, &mut out),
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => self.key(key, *modifiers, &mut out),
            UiEvent::FileHover { pos, paths } => self.file_hover(*pos, paths.len()),
            UiEvent::FileHoverCancel => self.set_drop(None),
            UiEvent::FileDrop { pos, paths } => {
                let target = self.drop_target(*pos);
                self.set_drop(None);
                if let (Some(at_key), Some(device)) = (target, self.device.clone())
                    && !paths.is_empty()
                    && self.present
                {
                    out.push(UiAction::ImportSamples {
                        paths: paths.clone(),
                        device,
                        at_key,
                    });
                }
            }
            _ => {}
        }
        out
    }

    /// `Some(at_key)` when files dropped at `pos` go to this editor.
    fn drop_target(&self, pos: Point) -> Option<Option<u8>> {
        if !self.bounds.contains(pos) || !self.present {
            return None;
        }
        let l = self.layout();
        Some((l.map.contains(pos) || l.keys.contains(pos)).then(|| self.key_at(pos.x)))
    }

    fn file_hover(&mut self, pos: Point, count: usize) {
        let hover = if count == 0 {
            None
        } else {
            self.drop_target(pos)
                .map(|at_key| DropHover { at_key, count })
        };
        self.set_drop(hover);
    }

    fn set_drop(&mut self, hover: Option<DropHover>) {
        if self.drop != hover {
            self.drop = hover;
            self.repaint = true;
        }
    }

    fn select(&mut self, i: usize) {
        if self.selected != Some(i) {
            self.selected = Some(i);
            self.reset_wave();
            self.clamp_scrolls();
        }
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
        if !matches!(self.drag, Drag::None) || button != PointerButton::Primary {
            return;
        }
        let geo = self.geo();
        match self.hit(pos) {
            Hit::Close => out.push(UiAction::CloseSampleEditor),
            Hit::Load => {
                if let Some(device) = self.device.clone() {
                    out.push(UiAction::LoadSamplesDialog { device });
                }
            }
            Hit::Row(i) => self.select(i),
            Hit::Loop(mode) => {
                if let Some(i) = self.selected {
                    let z = &self.zones[i];
                    let nz = zone_edit::with_loop_mode(z, mode, self.frames_of(z));
                    if nz != *z {
                        out.extend(self.set_zones(self.replaced(i, nz), None));
                    }
                }
            }
            Hit::Trigger(mode) => {
                if let Some(i) = self.selected
                    && self.zones[i].trigger != mode
                {
                    let mut nz = self.zones[i].clone();
                    nz.trigger = mode;
                    out.extend(self.set_zones(self.replaced(i, nz), None));
                }
            }
            Hit::Field(field) => {
                let Some(i) = self.selected else { return };
                let z = self.zones[i].clone();
                let frames = self.frames_of(&z);
                if clicks >= 2 {
                    let nz = zone_edit::with_field(
                        &z,
                        field,
                        zone_edit::field_default(&z, field, frames),
                        frames,
                    );
                    if nz != z {
                        out.extend(self.set_zones(self.replaced(i, nz), None));
                    }
                    return;
                }
                let value = zone_edit::field_value(&z, field);
                self.start_edit(i, EditKind::Field(field, value), pos, None);
            }
            Hit::Marker(m) => {
                let Some(i) = self.selected else { return };
                let m = if modifiers.alt && m == Marker::LoopStart {
                    Marker::Crossfade
                } else {
                    m
                };
                let x = self.frame_x(zone_edit::marker_frame(&self.zones[i], m));
                self.start_edit(i, EditKind::Marker(m, x - pos.x), pos, None);
            }
            Hit::Zone(i, edges) => {
                self.select(i);
                if edges.is_none() {
                    let key = geo.key_at(pos.x);
                    let vel = self.velocity_at(pos.y);
                    out.extend(self.audition(key, vel, true));
                    self.start_edit(i, EditKind::Zone(edges), pos, Some((key, vel)));
                } else {
                    self.start_edit(i, EditKind::Zone(edges), pos, None);
                }
            }
            Hit::Map(key, vel) => {
                out.extend(self.audition(key, vel, true));
                self.drag = Drag::Keys {
                    key,
                    velocity: Some(vel),
                };
            }
            Hit::Keys(key) => {
                out.extend(self.audition(key, STRIP_VELOCITY, true));
                self.drag = Drag::Keys {
                    key,
                    velocity: None,
                };
            }
            Hit::None => {}
        }
    }

    fn start_edit(
        &mut self,
        index: usize,
        kind: EditKind,
        start: Point,
        sounding: Option<(u8, u8)>,
    ) {
        let z = &self.zones[index];
        let (frames, rate) = (self.frames_of(z), self.rate_of(z));
        let merge = self.merge_id();
        self.drag = Drag::Edit(Box::new(EditDrag {
            kind,
            index,
            start,
            orig: self.zones.clone(),
            last: self.zones.clone(),
            merge,
            frames,
            rate,
            moved: false,
            emitted: false,
            sounding,
        }));
    }

    fn pointer_move(&mut self, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        if matches!(self.drag, Drag::None) {
            let h = self.hit(pos);
            if h != self.hover {
                self.hover = h;
                self.repaint = true;
            }
            return;
        }
        self.repaint = true;
        let geo = self.geo();
        let vel_here = self.velocity_at(pos.y);
        let device = self.device.clone();
        let audition = |key: u8, velocity: u8, on: bool| {
            device.clone().map(|device| UiAction::AuditionVelocity {
                device,
                key,
                velocity,
                on,
            })
        };
        match &mut self.drag {
            Drag::None => {}
            Drag::Keys { key, velocity } => {
                let here = geo.key_at(pos.x);
                if here != *key {
                    let v = velocity.unwrap_or(STRIP_VELOCITY);
                    out.extend(audition(*key, v, false));
                    let nv = velocity.map(|_| vel_here).unwrap_or(STRIP_VELOCITY);
                    out.extend(audition(here, nv, true));
                    *key = here;
                    *velocity = velocity.map(|_| nv);
                }
            }
            Drag::Edit(e) => {
                e.moved |= pos.distance(e.start) > DRAG_THRESHOLD;
                if !e.moved {
                    return;
                }
                let orig = &e.orig[e.index];
                let nz = match e.kind {
                    EditKind::Zone(edges) => {
                        let dkey = ((pos.x - e.start.x) / geo.ppk).round() as i32;
                        let dvel = (-(pos.y - e.start.y) / geo.ppv).round() as i32;
                        zone_edit::drag_edges(orig, edges, dkey, dvel, e.frames)
                    }
                    EditKind::Marker(m, grab) => {
                        let frame = ((pos.x + grab - geo.wave_x0) * geo.fpp + geo.wave_scroll)
                            .round() as i64;
                        zone_edit::with_marker(orig, m, frame, e.frames)
                    }
                    EditKind::Field(f, v0) => {
                        let k = if modifiers.shift { FINE_FACTOR } else { 1.0 };
                        let v = v0 + (e.start.y - pos.y) * f.per_pixel(e.rate) * k;
                        zone_edit::with_field(orig, f, v, e.frames)
                    }
                };
                let mut list = e.orig.clone();
                list[e.index] = nz;
                if list != e.last {
                    e.last.clone_from(&list);
                    e.emitted = true;
                    if let Some(device) = device.clone() {
                        out.push(UiAction::Command(Command::SetZones {
                            device,
                            zones: list,
                            merge: Some(e.merge),
                        }));
                    }
                }
            }
        }
    }

    fn pointer_up(&mut self, out: &mut Vec<UiAction>) {
        self.repaint = true;
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None => {}
            Drag::Keys { key, velocity } => {
                out.extend(self.audition(key, velocity.unwrap_or(STRIP_VELOCITY), false));
            }
            Drag::Edit(e) => {
                if let Some((key, vel)) = e.sounding {
                    out.extend(self.audition(key, vel, false));
                }
                if e.emitted {
                    out.push(UiAction::EndMerge);
                }
            }
        }
    }

    fn cancel_drag(&mut self, out: &mut Vec<UiAction>) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None => {}
            Drag::Keys { key, velocity } => {
                out.extend(self.audition(key, velocity.unwrap_or(STRIP_VELOCITY), false));
            }
            Drag::Edit(e) => {
                if let Some((key, vel)) = e.sounding {
                    out.extend(self.audition(key, vel, false));
                }
                if e.emitted {
                    out.extend(self.set_zones(e.orig, Some(e.merge)));
                    out.push(UiAction::EndMerge);
                }
            }
        }
    }

    fn wheel(
        &mut self,
        dx: f64,
        dy: f64,
        pos: Point,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        self.repaint = true;
        let l = self.layout();
        if l.map.contains(pos) || l.keys.contains(pos) {
            if modifiers.command() {
                let fit = Self::fit_ppk(&l.map);
                let old = fit * self.zoom;
                let anchor = (pos.x - l.map.x0 + self.scroll_x) / old;
                self.zoom =
                    (self.zoom * ZOOM_STEP.powf(dy)).clamp(1.0, (MAX_KEY_PX / fit).max(1.0));
                self.scroll_x = anchor * fit * self.zoom - (pos.x - l.map.x0);
            } else {
                let d = if dx.abs() > 1e-9 { dx } else { dy };
                self.scroll_x -= d * SCROLL_STEP;
            }
        } else if l.list.contains(pos) {
            self.list_scroll -= dy * SCROLL_STEP;
        } else if l.wave.contains(pos) {
            let Some((_, _, m)) = self.selected_with_meta() else {
                return;
            };
            let frames = m.frames as f64;
            let w = l.wave.width().max(1.0);
            if modifiers.command() {
                let old_fpp = frames / (w * self.wave_zoom);
                let anchor = (pos.x - l.wave.x0) * old_fpp + self.wave_scroll;
                self.wave_zoom =
                    (self.wave_zoom * ZOOM_STEP.powf(dy)).clamp(1.0, max_wave_zoom(frames, w));
                let fpp = frames / (w * self.wave_zoom);
                self.wave_scroll = anchor - (pos.x - l.wave.x0) * fpp;
            } else {
                let d = if dx.abs() > 1e-9 { dx } else { dy };
                self.wave_scroll -= d * SCROLL_STEP * frames / (w * self.wave_zoom);
            }
        } else if l.fields.contains(pos)
            && let Hit::Field(f) = self.hit(pos)
            && let Some(i) = self.selected
        {
            let z = self.zones[i].clone();
            let step = f.wheel_step(self.rate_of(&z)) * dy.signum() * dy.abs().ceil();
            let nz = zone_edit::with_field(
                &z,
                f,
                zone_edit::field_value(&z, f) + step,
                self.frames_of(&z),
            );
            if nz != z {
                out.extend(self.set_zones(self.replaced(i, nz), None));
            }
        }
        self.clamp_scrolls();
    }

    /// Runs a registry command the shell offers to this view. `edit.delete` removes the
    /// selected zone (the next one takes the selection); with no selection, or while a drag
    /// is in progress, it is declined so the next binding runs. Every other command is
    /// declined: the editor has no clipboard, no keyboard zoom and no tools.
    pub fn command(
        &mut self,
        id: &str,
        _arg: Option<&str>,
        doc: &RackDoc,
        registry: &Registry,
    ) -> CommandOutcome {
        self.sync(doc, registry);
        if id != "edit.delete" || self.device.is_none() || !matches!(self.drag, Drag::None) {
            return CommandOutcome::Declined;
        }
        let Some(i) = self.selected.filter(|i| *i < self.zones.len()) else {
            return CommandOutcome::Declined;
        };
        let mut zones = self.zones.clone();
        zones.remove(i);
        self.selected = (!zones.is_empty()).then(|| i.min(zones.len() - 1));
        self.reset_wave();
        self.repaint = true;
        CommandOutcome::Handled(self.set_zones(zones, None).into_iter().collect())
    }

    fn key(&mut self, key: &Key, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        self.repaint = true;
        // Command chords (undo, redo, clipboard) are registry commands the shell resolves;
        // what no command took is not a key of the view.
        if modifiers.command() {
            return;
        }
        if *key == Key::Escape {
            if !matches!(self.drag, Drag::None) {
                self.cancel_drag(out);
            } else if self.selected.is_some() {
                self.selected = None;
            } else {
                out.push(UiAction::CloseSampleEditor);
            }
        }
    }

    // ------------------------------------------------------------------ painting

    /// Paints the editor into its bounds (physical pixels, scaled like the other views).
    pub fn paint(&mut self, canvas: &mut dyn Canvas, doc: &RackDoc, registry: &Registry) {
        self.sync(doc, registry);
        paint::paint(self, canvas);
        self.repaint = false;
    }

    /// Text of a zone list row's second line, for tests and the paint code: the key range as
    /// note names, the velocity range and the root key.
    pub fn zone_summary(z: &ZoneDoc) -> String {
        format!(
            "{}-{}  v{}-{}  root {}",
            zone_edit::note_name(z.key_lo),
            zone_edit::note_name(z.key_hi),
            z.vel_lo,
            z.vel_hi,
            zone_edit::note_name(z.root_key)
        )
    }
}
