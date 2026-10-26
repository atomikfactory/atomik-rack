//! The slice editor: a pane that replaces the arrangement
//! and edits the slices of one loop slicer (`atomik.loop-slicer`).
//!
//! Hosting follows [`crate::SampleEditorView`]: on [`UiAction::OpenSliceEditor`] call
//! [`SliceEditorView::open`] and show the view in place of the arrangement; on
//! [`UiAction::CloseSliceEditor`] show the arrangement again (also when
//! [`SliceEditorView::is_present`] turns false because the slicer was deleted). Peaks of the
//! loop asset come from the app ([`SliceEditorView::set_peaks`], the same data as the
//! arrangement's) and assets whose file is missing are reported with
//! [`SliceEditorView::set_missing_assets`].
//!
//! # Layout (logical pixels, inside the bounds)
//!
//! ```text
//! top bar     SLICE EDITOR   <device> · <n slices> · <loop name>                [Load...] [x]
//! toolbar     MODE [Transients][Grid]  SENS <0-100 %>  DIV [1/4][1/8][1/16][1/32]  BEATS <n>  [Slice] [To track]
//! strip       one cell per slice: its number and key (C4 = 60), reversed slices marked "R"
//! waveform    the loop with a marker at every slice start; the selected slice is highlighted
//! fields      LEVEL  PAN  PITCH (semitones)  TUNE (cents)  [Reverse]      of the selected slice
//! ```
//!
//! # Interaction
//!
//! * **Select.** A click on the waveform or on the strip selects the slice under the pointer and
//!   auditions it ([`UiAction::AuditionVelocity`] at velocity 100, note on at the press, note off at
//!   the release).
//! * **Move a marker.** Drag a marker (the thin line at a slice start, grabbable anywhere on
//!   the strip and the waveform). A marker stays at least 1 ms (at the document sample rate)
//!   away from its neighbours, so a drag never changes the slice count and every slice keeps its
//!   fields. **The first marker (frame 0) cannot be moved**: the loop is always covered from its
//!   start. Esc during the drag restores the markers.
//! * **Add a marker.** Double-click on the waveform: the new slice becomes the selected one. Not
//!   possible at 64 slices or closer than 1 ms to a marker.
//! * **Delete.** Delete / Backspace removes the start marker of the selected slice (it merges
//!   into the previous slice, which becomes the selected one). **The first slice cannot be
//!   removed.** Per-slice fields move with their index, as [`atomik_model::slices_from_markers`]
//!   documents.
//! * **Zoom.** Ctrl/Cmd+wheel zooms the waveform around the pointer, the wheel scrolls it.
//! * **Fields.** Drag a field vertically (Shift = fine), double-click resets it, the wheel steps
//!   it; **Reverse** toggles. Every edit is one `Command::SetZones` with the whole list in which
//!   only that slice's field changed (clamped valid by [`crate::zone_edit::sanitize`]).
//! * **Slicing.** The mode selector chooses transients or an even grid. Dragging the sensitivity
//!   field (0-100 %) selects transients, a division button selects the grid and its division.
//!   **Slice** emits [`UiAction::SliceLoop`] (the app computes the markers: the UI has no
//!   samples) and **To track** emits [`UiAction::SlicesToTrack`]; both are ignored without a loop.
//!   The **beats** field edits the device's `loop.beats` with `Command::SetParam` (drag, wheel,
//!   double-click resets).
//! * **Edits.** Marker, field and beats drags use one merge id for the whole gesture (ids from
//!   6·2⁴⁰, so they never collide with the other views: sample editor 4·2⁴⁰, step editor 5·2⁴⁰)
//!   and end with [`UiAction::EndMerge`]; every other edit is one standalone step.
//! * **Drop.** [`UiEvent::FileHover`] / [`UiEvent::FileDrop`] inside the bounds emit
//!   [`UiAction::ImportSamples`] with `at_key: None` (the app replaces the loop and slices it);
//!   an indicator shows the target meanwhile. **Load...** emits [`UiAction::LoadSamplesDialog`].
//! * **Empty state.** Without a loop the pane shows "Drop a loop here" and a Load button.
//! * Esc (with nothing to cancel or deselect) and the close button emit
//!   [`UiAction::CloseSliceEditor`]; Ctrl/Cmd+Z / Y undo and redo.
//!
//! The sensitivity, mode and division are view state ([`SliceEditorView::set_sensitivity`]): the
//! app may copy the rack panel's value in on open.

use crate::action::{SliceMode, UiAction};
use crate::autolayout::SLICER_TYPE;
use crate::canvas::Canvas;
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::keymap::CommandOutcome;
use crate::sample_editor::{FIELD_HEIGHT, FIELD_WIDTH, TOP_BAR_HEIGHT};
use crate::seq_paint::SeqTheme;
use crate::theme::Theme;
use crate::tooltip::Help;
use crate::wave_paint::max_wave_zoom;
use crate::zone_edit::{self, Field};
use atomik_audio_file::PeakPyramid;
use atomik_device_sdk::Registry;
use atomik_model::{
    AssetMeta, Command, DeviceKey, MAX_SLICES, RackDoc, SLICE_KEY_BASE, ZoneDoc, markers_of,
    slices_from_markers,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use vello::kurbo::{Point, Rect};
use vello::peniko::Color;

mod paint;

/// Height of the toolbar band.
pub const TOOLBAR_HEIGHT: f64 = 32.0;
/// Height of the slice strip above the waveform.
pub const STRIP_HEIGHT: f64 = 26.0;
/// The grid divisions of the toolbar: slices per beat and label (1/4, 1/8, 1/16, 1/32).
pub const DIVISIONS: [(u32, &str); 4] = [(1, "1/4"), (2, "1/8"), (4, "1/16"), (8, "1/32")];
/// Default sensitivity in percent.
pub const DEFAULT_SENSITIVITY: u8 = 50;
/// The per-slice fields, in panel order (level, pan, pitch semitones, tune cents).
pub const SLICE_FIELDS: [Field; 4] = [Field::Gain, Field::Pan, Field::Transpose, Field::Tune];
/// Velocity of the audition of a slice.
pub const AUDITION_VELOCITY: u8 = 100;

const PAD: f64 = 6.0;
const EDGE_PX: f64 = 5.0;
const DRAG_THRESHOLD: f64 = 2.0;
const SCROLL_STEP: f64 = 36.0;
const ZOOM_STEP: f64 = 1.15;
const MERGE_BASE: u64 = 6 << 40;
const FINE_FACTOR: f64 = 0.1;
const SENS_PER_PIXEL: f64 = 0.8;
const BEATS_PER_PIXEL: f64 = 0.25;
const BEATS_KEY: &str = "loop.beats";

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    None,
    Close,
    Load,
    EmptyLoad,
    ModeTransients,
    ModeGrid,
    Sens,
    Div(u32),
    Beats,
    SliceButton,
    ToTrack,
    /// The marker at the start of slice `i` (`i >= 1`).
    Marker(usize),
    /// A slice (waveform or strip).
    Slice(usize),
    Field(Field),
    Reverse,
}

#[derive(Debug, Clone)]
struct ZoneDrag {
    /// Index of the slice (not of the zone list).
    slice: usize,
    kind: ZoneDragKind,
    start: Point,
    orig: Vec<ZoneDoc>,
    last: Vec<ZoneDoc>,
    merge: u64,
    moved: bool,
    emitted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ZoneDragKind {
    /// A marker and the offset from the pointer to it at the press.
    Marker(f64),
    /// A field and its value at the press.
    Field(Field, f64),
}

#[derive(Debug, Clone)]
enum Drag {
    None,
    /// Auditioning a slice key.
    Audition {
        key: u8,
    },
    Zone(Box<ZoneDrag>),
    /// Dragging the sensitivity: press y and value.
    Sens {
        start_y: f64,
        start: u8,
    },
    /// Dragging `loop.beats`: press y, value at the press, merge id, whether a value was sent.
    Beats {
        start_y: f64,
        start: u32,
        last: u32,
        merge: u64,
        emitted: bool,
    },
}

/// The rectangles of the view, from its bounds.
#[derive(Debug, Clone, Copy)]
struct Layout {
    top: Rect,
    close: Rect,
    load: Rect,
    bar: Rect,
    mode_transients: Rect,
    mode_grid: Rect,
    sens: Rect,
    divs: [Rect; 4],
    beats: Rect,
    slice_button: Rect,
    to_track: Rect,
    strip: Rect,
    wave: Rect,
    fields: Rect,
    empty_load: Rect,
}

/// The slice editor. See the module documentation.
#[derive(Debug)]
pub struct SliceEditorView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    device: Option<DeviceKey>,
    present: bool,
    name: String,
    accent: Color,
    zones: Vec<ZoneDoc>,
    /// Indices into `zones` of the slice zones, in key order (slice 1 first).
    order: Vec<usize>,
    meta: Option<AssetMeta>,
    frames: u64,
    doc_rate: u32,
    beats: u32,
    beats_default: u32,
    peaks: BTreeMap<String, Arc<PeakPyramid>>,
    /// Samples of the assets held in RAM (the waveform reads them below the finest peak level).
    samples: BTreeMap<String, Arc<[Vec<f32>]>>,
    missing: BTreeSet<String>,
    selected: Option<usize>,
    pick_first: bool,
    wave_zoom: f64,
    wave_scroll: f64,
    grid_mode: bool,
    sensitivity: u8,
    division: u32,
    drag: Drag,
    hover: Hit,
    drop: Option<usize>,
    next_merge: u64,
    focused: bool,
    repaint: bool,
}

impl SliceEditorView {
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
            order: Vec::new(),
            meta: None,
            frames: 0,
            doc_rate: 48_000,
            beats: 4,
            beats_default: 4,
            peaks: BTreeMap::new(),
            samples: BTreeMap::new(),
            missing: BTreeSet::new(),
            selected: None,
            pick_first: false,
            wave_zoom: 1.0,
            wave_scroll: 0.0,
            grid_mode: false,
            sensitivity: DEFAULT_SENSITIVITY,
            division: 4,
            drag: Drag::None,
            hover: Hit::None,
            drop: None,
            next_merge: MERGE_BASE,
            focused: false,
            repaint: true,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Opens the editor on a loop slicer (the first slice is selected on the next sync). The
    /// peaks and missing-asset set the host handed in stay.
    pub fn open(&mut self, device: DeviceKey) {
        if self.device.as_ref() != Some(&device) {
            self.selected = None;
            self.zones.clear();
            self.order.clear();
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
        self.order.clear();
        self.meta = None;
        self.frames = 0;
        self.selected = None;
        self.drag = Drag::None;
        self.drop = None;
        self.repaint = true;
    }

    /// The loop slicer being edited.
    pub fn device(&self) -> Option<&DeviceKey> {
        self.device.as_ref()
    }

    /// True while the open device exists in the document and is a loop slicer. When it turns
    /// false (the slicer was deleted), the host shows the arrangement again.
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

    /// Sets the waveform peaks of an asset (its content hash), replacing earlier ones.
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

    /// Whether the samples of an asset are held (see [`Self::set_samples`]).
    pub fn has_samples(&self, asset: &str) -> bool {
        self.samples.contains_key(asset)
    }

    /// Forgets the peaks of an asset.
    pub fn clear_peaks(&mut self, asset: &str) {
        let had_samples = self.samples.remove(asset).is_some();
        if self.peaks.remove(asset).is_some() || had_samples {
            self.repaint = true;
        }
    }

    /// Sets the assets whose audio is missing: the slices stay editable but have no waveform.
    pub fn set_missing_assets(&mut self, assets: impl IntoIterator<Item = String>) {
        let set: BTreeSet<String> = assets.into_iter().collect();
        if set != self.missing {
            self.missing = set;
            self.repaint = true;
        }
    }

    /// Whether `asset` is reported missing (see [`Self::set_missing_assets`]).
    pub fn is_asset_missing(&self, asset: &str) -> bool {
        self.missing.contains(asset)
    }

    /// The zones of the open slicer, as last synced.
    pub fn zones(&self) -> &[ZoneDoc] {
        &self.zones
    }

    /// The number of slices.
    pub fn slice_count(&self) -> usize {
        self.order.len()
    }

    /// The slice start markers (frames), slice 1 first.
    pub fn markers(&self) -> Vec<u64> {
        markers_of(&self.zones)
    }

    /// The selected slice (0-based, slice 1 is 0).
    pub fn selected_slice(&self) -> Option<usize> {
        self.selected
    }

    /// Selects a slice (or none).
    pub fn set_selected_slice(&mut self, slice: Option<usize>) {
        let slice = slice.filter(|i| *i < self.order.len());
        if slice != self.selected {
            self.selected = slice;
            self.repaint = true;
        }
    }

    /// The slicing mode the Slice button uses now.
    pub fn slice_mode(&self) -> SliceMode {
        if self.grid_mode {
            SliceMode::Grid {
                division: self.division,
            }
        } else {
            SliceMode::Transients {
                sensitivity: self.sensitivity,
            }
        }
    }

    /// The sensitivity in percent.
    pub fn sensitivity(&self) -> u8 {
        self.sensitivity
    }

    /// Sets the sensitivity (percent, clamped to 0..=100; no action emitted).
    pub fn set_sensitivity(&mut self, percent: u8) {
        let p = percent.min(100);
        if p != self.sensitivity {
            self.sensitivity = p;
            self.repaint = true;
        }
    }

    /// The grid division in slices per beat (1, 2, 4 or 8).
    pub fn division(&self) -> u32 {
        self.division
    }

    /// The `loop.beats` value as last synced.
    pub fn beats(&self) -> u32 {
        self.beats
    }

    /// The drop indicator while files hover over the view (the number of files).
    pub fn drop_hover(&self) -> Option<usize> {
        self.drop
    }

    // ------------------------------------------------------------------ geometry (public, for tests and hosts)

    /// The waveform.
    pub fn waveform_rect(&self) -> Rect {
        self.layout().wave
    }

    /// The slice strip above the waveform.
    pub fn strip_rect(&self) -> Rect {
        self.layout().strip
    }

    /// Centre of the close button.
    pub fn close_button_center(&self) -> Point {
        self.layout().close.center()
    }

    /// Centre of the Load button of the top bar.
    pub fn load_button_center(&self) -> Point {
        self.layout().load.center()
    }

    /// Centre of the Load button of the empty state.
    pub fn empty_load_center(&self) -> Point {
        self.layout().empty_load.center()
    }

    /// Centre of the Transients (`false`) or Grid (`true`) mode button.
    pub fn mode_button_center(&self, grid: bool) -> Point {
        let l = self.layout();
        if grid { l.mode_grid } else { l.mode_transients }.center()
    }

    /// Centre of the sensitivity field.
    pub fn sensitivity_center(&self) -> Point {
        self.layout().sens.center()
    }

    /// Centre of the button of a grid division (slices per beat: 1, 2, 4 or 8).
    pub fn division_button_center(&self, division: u32) -> Option<Point> {
        let i = DIVISIONS.iter().position(|(d, _)| *d == division)?;
        Some(self.layout().divs[i].center())
    }

    /// Centre of the `loop.beats` field.
    pub fn beats_center(&self) -> Point {
        self.layout().beats.center()
    }

    /// Centre of the Slice button.
    pub fn slice_button_center(&self) -> Point {
        self.layout().slice_button.center()
    }

    /// Centre of the To track button.
    pub fn to_track_button_center(&self) -> Point {
        self.layout().to_track.center()
    }

    /// Centre of a per-slice field's value box (one of [`SLICE_FIELDS`]).
    pub fn field_center(&self, field: Field) -> Point {
        self.field_value_rect(field).center()
    }

    /// Centre of the Reverse button.
    pub fn reverse_button_center(&self) -> Point {
        self.reverse_rect().center()
    }

    /// Logical x of a frame on the waveform.
    pub fn frame_x(&self, frame: u64) -> f64 {
        let (x0, scroll, fpp) = self.wave_geo();
        x0 + (frame as f64 - scroll) / fpp
    }

    /// The frame under a logical x of the waveform.
    pub fn x_to_frame(&self, x: f64) -> f64 {
        let (x0, scroll, fpp) = self.wave_geo();
        (x - x0) * fpp + scroll
    }

    /// Where the marker at the start of slice `slice` sits (x at its frame, y on the waveform
    /// body), `None` for a slice that does not exist.
    pub fn marker_point(&self, slice: usize) -> Option<Point> {
        let start = *self.markers().get(slice)?;
        Some(Point::new(
            self.frame_x(start),
            self.layout().wave.center().y,
        ))
    }

    /// A point inside slice `slice` on the waveform (the middle of its visible part).
    pub fn slice_point(&self, slice: usize) -> Option<Point> {
        let (a, b) = self.slice_range(slice)?;
        let w = self.layout().wave;
        let (x0, x1) = (self.frame_x(a).max(w.x0), self.frame_x(b).min(w.x1));
        Some(Point::new((x0 + x1) / 2.0, w.center().y))
    }

    /// A point inside slice `slice` on the strip.
    pub fn strip_point(&self, slice: usize) -> Option<Point> {
        let p = self.slice_point(slice)?;
        Some(Point::new(p.x, self.layout().strip.center().y))
    }

    // ------------------------------------------------------------------ sync

    /// Reads the open slicer from the document. Emits nothing.
    pub fn sync(&mut self, doc: &RackDoc, registry: &Registry) {
        let Some(key) = &self.device else { return };
        let Some(d) = doc.resolve(key).filter(|d| d.type_id == SLICER_TYPE) else {
            if self.present {
                self.present = false;
                self.zones.clear();
                self.order.clear();
                self.meta = None;
                self.frames = 0;
                self.selected = None;
                self.drag = Drag::None;
                self.repaint = true;
            }
            return;
        };
        let keys = SLICE_KEY_BASE..SLICE_KEY_BASE + MAX_SLICES as u8;
        let mut order: Vec<usize> = (0..d.zones.len())
            .filter(|i| {
                let z = &d.zones[*i];
                z.key_lo == z.key_hi && keys.contains(&z.key_lo)
            })
            .collect();
        order.sort_by_key(|i| d.zones[*i].key_lo);
        let meta = order
            .first()
            .and_then(|i| doc.assets.get(&d.zones[*i].asset))
            .cloned();
        let frames = meta.as_ref().map_or_else(
            || order.iter().map(|i| d.zones[*i].end).max().unwrap_or(0),
            |m| m.frames,
        );
        let default = registry
            .get(&d.type_id)
            .and_then(|f| f.descriptor.params.iter().find(|p| p.key == BEATS_KEY))
            .map_or(4.0, |p| p.default);
        let beats_default = default.round().clamp(1.0, 64.0) as u32;
        let beats = d
            .params
            .get(BEATS_KEY)
            .map_or(beats_default, |v| v.round().clamp(1.0, 64.0) as u32);
        let accent = self.st.base.accent_for(d);
        if !self.present
            || self.zones != d.zones
            || self.meta != meta
            || self.beats != beats
            || self.name != d.name
            || self.accent != accent
            || self.doc_rate != doc.sample_rate
        {
            self.present = true;
            self.zones.clone_from(&d.zones);
            self.order = order;
            self.meta = meta;
            self.frames = frames;
            self.beats = beats;
            self.beats_default = beats_default;
            self.name.clone_from(&d.name);
            self.accent = accent;
            self.doc_rate = doc.sample_rate;
            self.repaint = true;
        }
        let n = self.order.len();
        if let Some(s) = self.selected
            && s >= n
        {
            self.selected = n.checked_sub(1);
        }
        if self.pick_first && self.selected.is_none() && n > 0 {
            self.selected = Some(0);
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
        let bar = rect(b.x0, top.y1, b.x1, top.y1 + TOOLBAR_HEIGHT);
        let (y0, y1) = (bar.y0 + 5.0, bar.y1 - 5.0);
        let mut x = b.x0 + PAD + 34.0;
        let mode_transients = Rect::new(x, y0, x + 78.0, y1);
        x = mode_transients.x1 + 4.0;
        let mode_grid = Rect::new(x, y0, x + 44.0, y1);
        x = mode_grid.x1 + 14.0 + 30.0;
        let sens = Rect::new(x, y0, x + 54.0, y1);
        x = sens.x1 + 14.0 + 24.0;
        let mut divs = [Rect::ZERO; 4];
        for d in &mut divs {
            *d = Rect::new(x, y0, x + 34.0, y1);
            x += 38.0;
        }
        x += 10.0 + 38.0;
        let beats = Rect::new(x, y0, x + 44.0, y1);
        x = beats.x1 + 14.0;
        let slice_button = Rect::new(x, y0, x + 56.0, y1);
        let to_track = Rect::new(slice_button.x1 + 4.0, y0, slice_button.x1 + 80.0, y1);
        let fields_h = FIELD_HEIGHT + 10.0;
        let strip = rect(
            b.x0 + PAD,
            bar.y1 + 4.0,
            b.x1 - PAD,
            bar.y1 + 4.0 + STRIP_HEIGHT,
        );
        let fields = rect(b.x0, (b.y1 - fields_h).max(strip.y1), b.x1, b.y1);
        let wave = rect(strip.x0, strip.y1, strip.x1, fields.y0 - 4.0);
        let c = wave.center();
        let empty_load = Rect::new(c.x - 40.0, c.y + 12.0, c.x + 40.0, c.y + 34.0);
        Layout {
            top,
            close,
            load,
            bar,
            mode_transients,
            mode_grid,
            sens,
            divs,
            beats,
            slice_button,
            to_track,
            strip,
            wave,
            fields,
            empty_load,
        }
    }

    /// `(x of the waveform's left edge, scroll in frames, frames per pixel)`.
    fn wave_geo(&self) -> (f64, f64, f64) {
        let w = self.layout().wave;
        let fpp = self.frames.max(1) as f64 / (w.width().max(1.0) * self.wave_zoom);
        (w.x0, self.wave_scroll, fpp)
    }

    fn clamp_scrolls(&mut self) {
        let width = self.layout().wave.width();
        self.wave_zoom = self
            .wave_zoom
            .clamp(1.0, max_wave_zoom(self.frames as f64, width));
        let span = self.frames as f64 / self.wave_zoom;
        let max = (self.frames as f64 - span).max(0.0);
        self.wave_scroll = self.wave_scroll.clamp(0.0, max);
    }

    fn field_rect(&self, field: Field) -> Rect {
        let l = self.layout();
        let i = SLICE_FIELDS.iter().position(|f| *f == field).unwrap_or(0);
        let x = l.fields.x0 + PAD + i as f64 * FIELD_WIDTH;
        let y = l.fields.y0 + 5.0;
        Rect::new(x, y, x + FIELD_WIDTH - 6.0, y + FIELD_HEIGHT - 4.0)
    }

    fn field_value_rect(&self, field: Field) -> Rect {
        let r = self.field_rect(field);
        Rect::new(r.x0, r.y0 + 12.0, r.x1, r.y1)
    }

    fn reverse_rect(&self) -> Rect {
        let l = self.layout();
        let x = l.fields.x0 + PAD + SLICE_FIELDS.len() as f64 * FIELD_WIDTH + 6.0;
        let y = l.fields.y0 + 5.0 + 14.0;
        Rect::new(x, y, x + 76.0, y + FIELD_HEIGHT - 16.0)
    }

    /// The zone of slice `i`.
    fn zone(&self, slice: usize) -> Option<&ZoneDoc> {
        self.zones.get(*self.order.get(slice)?)
    }

    /// The frame range of slice `i`.
    fn slice_range(&self, slice: usize) -> Option<(u64, u64)> {
        self.zone(slice).map(|z| (z.start, z.end))
    }

    /// One millisecond of frames at the document sample rate (at least 1).
    fn min_len(&self) -> u64 {
        u64::from(self.doc_rate / 1000).max(1)
    }

    /// Edits need the loop asset in the pool (the model validates the zones against it).
    fn editable(&self) -> bool {
        self.present && self.meta.is_some() && !self.order.is_empty()
    }

    fn asset(&self) -> Option<&str> {
        self.zone(0).map(|z| z.asset.as_str())
    }

    fn slice_key(&self, slice: usize) -> Option<u8> {
        self.zone(slice).map(|z| z.key_lo)
    }

    /// The list for `markers` over the loop.
    fn zones_for(&self, markers: &[u64], previous: &[ZoneDoc]) -> Option<Vec<ZoneDoc>> {
        let asset = self.asset()?;
        Some(slices_from_markers(
            asset,
            self.frames,
            markers,
            previous,
            self.min_len(),
        ))
    }

    /// What the button or field under `pos` does, for its tooltip (`None` during a gesture and
    /// away from a button).
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if !matches!(self.drag, Drag::None) {
            return None;
        }
        let l = self.layout();
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        match self.hit(pos) {
            Hit::Close => help(l.close, "Close the slice editor"),
            Hit::Load => help(l.load, "Load another audio file to slice"),
            Hit::EmptyLoad => help(l.empty_load, "Load an audio file to slice"),
            Hit::ModeTransients => help(
                l.mode_transients,
                "Slice at the transients the sensitivity finds",
            ),
            Hit::ModeGrid => help(l.mode_grid, "Slice on an even grid"),
            Hit::Sens => help(
                l.sens,
                "Sensitivity: drag or scroll to change, double-click to reset",
            ),
            Hit::Div(d) => {
                let i = DIVISIONS.iter().position(|(x, _)| *x == d)?;
                help(l.divs[i], "Grid division: slices per beat")
            }
            Hit::Beats => help(
                l.beats,
                "Length of the loop in beats: drag or scroll to change, double-click to reset",
            ),
            Hit::SliceButton => help(l.slice_button, "Slice the loop and map the slices to keys"),
            Hit::ToTrack => help(l.to_track, "Lay the slices out as notes on a new track"),
            Hit::Field(f) => Some(Help::new(
                self.field_rect(f),
                format!(
                    "{}: drag or scroll to change, double-click to reset",
                    f.label()
                ),
            )),
            Hit::Reverse => help(self.reverse_rect(), "Play the selected slice backwards"),
            Hit::None | Hit::Marker(_) | Hit::Slice(_) => None,
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
        if l.bar.contains(pos) {
            if l.mode_transients.contains(pos) {
                return Hit::ModeTransients;
            }
            if l.mode_grid.contains(pos) {
                return Hit::ModeGrid;
            }
            if l.sens.contains(pos) {
                return Hit::Sens;
            }
            for (i, (d, _)) in DIVISIONS.iter().enumerate() {
                if l.divs[i].contains(pos) {
                    return Hit::Div(*d);
                }
            }
            if l.beats.contains(pos) {
                return Hit::Beats;
            }
            if l.slice_button.contains(pos) {
                return Hit::SliceButton;
            }
            if l.to_track.contains(pos) {
                return Hit::ToTrack;
            }
            return Hit::None;
        }
        if self.order.is_empty() {
            return if l.empty_load.contains(pos) {
                Hit::EmptyLoad
            } else {
                Hit::None
            };
        }
        if pos.y >= l.strip.y0 && pos.y <= l.wave.y1 {
            if let Some(i) = self.marker_at(pos) {
                return Hit::Marker(i);
            }
            if pos.x >= l.wave.x0
                && pos.x <= l.wave.x1
                && let Some(i) = self.slice_at(pos.x)
            {
                return Hit::Slice(i);
            }
            return Hit::None;
        }
        if l.fields.contains(pos) {
            for f in SLICE_FIELDS {
                if self.field_rect(f).contains(pos) {
                    return Hit::Field(f);
                }
            }
            if self.reverse_rect().contains(pos) {
                return Hit::Reverse;
            }
        }
        Hit::None
    }

    /// The marker (slice index >= 1) within [`EDGE_PX`] of `pos`.
    fn marker_at(&self, pos: Point) -> Option<usize> {
        let w = self.layout().wave;
        if pos.x < w.x0 - EDGE_PX || pos.x > w.x1 + EDGE_PX {
            return None;
        }
        self.markers()
            .iter()
            .enumerate()
            .skip(1)
            .filter_map(|(i, m)| {
                let d = (self.frame_x(*m) - pos.x).abs();
                (d <= EDGE_PX).then_some((i, d))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// The slice under a logical x.
    fn slice_at(&self, x: f64) -> Option<usize> {
        let frame = self.x_to_frame(x);
        if frame < 0.0 || frame >= self.frames as f64 {
            return None;
        }
        let f = frame as u64;
        let markers = self.markers();
        markers.iter().rposition(|m| *m <= f)
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

    fn set_beats(&self, beats: u32, merge: Option<u64>) -> Option<UiAction> {
        Some(UiAction::Command(Command::SetParam {
            key: self.device.clone()?,
            param: BEATS_KEY.to_string(),
            value: beats as f32,
            merge,
        }))
    }

    fn audition(&self, key: u8, on: bool) -> Option<UiAction> {
        Some(UiAction::AuditionVelocity {
            device: self.device.clone()?,
            key,
            velocity: AUDITION_VELOCITY,
            on,
        })
    }

    /// `zones` with slice `slice` replaced by `z`.
    fn replaced(&self, slice: usize, z: ZoneDoc) -> Option<Vec<ZoneDoc>> {
        let i = *self.order.get(slice)?;
        let mut list = self.zones.clone();
        list[i] = z;
        Some(list)
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
            UiEvent::FileHover { pos, paths } => {
                let hover = (!paths.is_empty() && self.bounds.contains(*pos) && self.present)
                    .then_some(paths.len());
                self.set_drop(hover);
            }
            UiEvent::FileHoverCancel => self.set_drop(None),
            UiEvent::FileDrop { pos, paths } => {
                self.set_drop(None);
                if self.bounds.contains(*pos)
                    && self.present
                    && !paths.is_empty()
                    && let Some(device) = self.device.clone()
                {
                    out.push(UiAction::ImportSamples {
                        paths: paths.clone(),
                        device,
                        at_key: None,
                    });
                }
            }
            _ => {}
        }
        out
    }

    fn set_drop(&mut self, hover: Option<usize>) {
        if self.drop != hover {
            self.drop = hover;
            self.repaint = true;
        }
    }

    fn pointer_down(
        &mut self,
        pos: Point,
        button: PointerButton,
        _modifiers: Modifiers,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        self.repaint = true;
        if !matches!(self.drag, Drag::None) || button != PointerButton::Primary {
            return;
        }
        let Some(device) = self.device.clone() else {
            return;
        };
        match self.hit(pos) {
            Hit::Close => out.push(UiAction::CloseSliceEditor),
            Hit::Load | Hit::EmptyLoad => out.push(UiAction::LoadSamplesDialog { device }),
            Hit::ModeTransients => self.grid_mode = false,
            Hit::ModeGrid => self.grid_mode = true,
            Hit::Sens => {
                self.grid_mode = false;
                if clicks >= 2 {
                    self.sensitivity = DEFAULT_SENSITIVITY;
                } else {
                    self.drag = Drag::Sens {
                        start_y: pos.y,
                        start: self.sensitivity,
                    };
                }
            }
            Hit::Div(d) => {
                self.grid_mode = true;
                self.division = d;
            }
            Hit::Beats => {
                if clicks >= 2 {
                    if self.beats != self.beats_default {
                        out.extend(self.set_beats(self.beats_default, None));
                    }
                } else {
                    let merge = self.merge_id();
                    self.drag = Drag::Beats {
                        start_y: pos.y,
                        start: self.beats,
                        last: self.beats,
                        merge,
                        emitted: false,
                    };
                }
            }
            Hit::SliceButton => {
                if self.editable() {
                    out.push(UiAction::SliceLoop {
                        device,
                        mode: self.slice_mode(),
                    });
                }
            }
            Hit::ToTrack => {
                if self.editable() {
                    out.push(UiAction::SlicesToTrack { device });
                }
            }
            Hit::Marker(i) => {
                self.selected = Some(i);
                if !self.editable() {
                    return;
                }
                let markers = self.markers();
                let x = self.frame_x(markers[i]);
                let merge = self.merge_id();
                self.drag = Drag::Zone(Box::new(ZoneDrag {
                    slice: i,
                    kind: ZoneDragKind::Marker(x - pos.x),
                    start: pos,
                    orig: self.zones.clone(),
                    last: self.zones.clone(),
                    merge,
                    moved: false,
                    emitted: false,
                }));
            }
            Hit::Slice(i) => {
                self.selected = Some(i);
                if clicks >= 2 {
                    self.add_marker(pos.x, out);
                } else if let Some(key) = self.slice_key(i) {
                    out.extend(self.audition(key, true));
                    self.drag = Drag::Audition { key };
                }
            }
            Hit::Field(field) => {
                let (Some(s), true) = (self.selected, self.editable()) else {
                    return;
                };
                let Some(z) = self.zone(s).cloned() else {
                    return;
                };
                if clicks >= 2 {
                    let nz = zone_edit::with_field(
                        &z,
                        field,
                        zone_edit::field_default(&z, field, self.frames),
                        self.frames,
                    );
                    if nz != z {
                        let list = self.replaced(s, nz);
                        out.extend(list.and_then(|l| self.set_zones(l, None)));
                    }
                    return;
                }
                let merge = self.merge_id();
                self.drag = Drag::Zone(Box::new(ZoneDrag {
                    slice: s,
                    kind: ZoneDragKind::Field(field, zone_edit::field_value(&z, field)),
                    start: pos,
                    orig: self.zones.clone(),
                    last: self.zones.clone(),
                    merge,
                    moved: false,
                    emitted: false,
                }));
            }
            Hit::Reverse => {
                let (Some(s), true) = (self.selected, self.editable()) else {
                    return;
                };
                if let Some(mut z) = self.zone(s).cloned() {
                    z.reverse = !z.reverse;
                    let list = self.replaced(s, z);
                    out.extend(list.and_then(|l| self.set_zones(l, None)));
                }
            }
            Hit::None => {}
        }
    }

    /// Adds a marker at the logical x of the waveform: one `SetZones`, the new slice selected.
    fn add_marker(&mut self, x: f64, out: &mut Vec<UiAction>) {
        if !self.editable() || self.order.len() >= MAX_SLICES {
            return;
        }
        let frame = self.x_to_frame(x).round().max(0.0) as u64;
        let mut markers = self.markers();
        markers.push(frame);
        let Some(list) = self.zones_for(&markers, &self.zones) else {
            return;
        };
        let new = markers_of(&list);
        if new == self.markers() {
            return;
        }
        if let Some(i) = new.iter().position(|m| *m == frame) {
            self.selected = Some(i);
        }
        out.extend(self.set_zones(list, None));
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
        let (x0, scroll, fpp) = self.wave_geo();
        let min_len = self.min_len();
        let frames = self.frames;
        let device = self.device.clone();
        let asset = self.asset().map(str::to_string);
        match &mut self.drag {
            Drag::None | Drag::Audition { .. } => {}
            Drag::Sens { start_y, start } => {
                let v = f64::from(*start) + (*start_y - pos.y) * SENS_PER_PIXEL;
                self.sensitivity = v.round().clamp(0.0, 100.0) as u8;
            }
            Drag::Beats {
                start_y,
                start,
                last,
                merge,
                emitted,
            } => {
                let v = f64::from(*start) + (*start_y - pos.y) * BEATS_PER_PIXEL;
                let beats = v.round().clamp(1.0, 64.0) as u32;
                if beats != *last {
                    *last = beats;
                    *emitted = true;
                    if let Some(key) = device {
                        out.push(UiAction::Command(Command::SetParam {
                            key,
                            param: BEATS_KEY.to_string(),
                            value: beats as f32,
                            merge: Some(*merge),
                        }));
                    }
                }
            }
            Drag::Zone(e) => {
                e.moved |= pos.distance(e.start) > DRAG_THRESHOLD;
                if !e.moved {
                    return;
                }
                let list = match e.kind {
                    ZoneDragKind::Marker(grab) => {
                        let Some(asset) = asset else { return };
                        let markers = markers_of(&e.orig);
                        let prev = markers.get(e.slice.wrapping_sub(1)).copied().unwrap_or(0);
                        let next = markers.get(e.slice + 1).copied().unwrap_or(frames);
                        let (lo, hi) = (prev + min_len, next.saturating_sub(min_len));
                        if lo > hi {
                            return;
                        }
                        let frame = ((pos.x + grab - x0) * fpp + scroll).round().max(0.0) as u64;
                        let mut moved = markers;
                        moved[e.slice] = frame.clamp(lo, hi);
                        slices_from_markers(&asset, frames, &moved, &e.orig, min_len)
                    }
                    ZoneDragKind::Field(f, v0) => {
                        let Some(i) = slice_zone_index(&e.orig, e.slice) else {
                            return;
                        };
                        let k = if modifiers.shift { FINE_FACTOR } else { 1.0 };
                        let v = v0 + (e.start.y - pos.y) * f.per_pixel(self.doc_rate) * k;
                        let mut list = e.orig.clone();
                        list[i] = zone_edit::with_field(&e.orig[i], f, v, frames);
                        list
                    }
                };
                if list != e.last {
                    e.last.clone_from(&list);
                    e.emitted = true;
                    if let Some(device) = device {
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
            Drag::None | Drag::Sens { .. } => {}
            Drag::Audition { key } => out.extend(self.audition(key, false)),
            Drag::Beats { emitted, .. } => {
                if emitted {
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Zone(e) => {
                if e.emitted {
                    out.push(UiAction::EndMerge);
                }
            }
        }
    }

    fn cancel_drag(&mut self, out: &mut Vec<UiAction>) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None => {}
            Drag::Audition { key } => out.extend(self.audition(key, false)),
            Drag::Sens { start, .. } => self.sensitivity = start,
            Drag::Beats {
                start,
                merge,
                emitted,
                ..
            } => {
                if emitted {
                    out.extend(self.set_beats(start, Some(merge)));
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Zone(e) => {
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
        if (l.wave.contains(pos) || l.strip.contains(pos)) && self.frames > 0 {
            let frames = self.frames as f64;
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
        } else if l.bar.contains(pos) {
            let notches = dy.signum() * dy.abs().ceil();
            match self.hit(pos) {
                Hit::Sens => {
                    let step = if modifiers.shift { 1.0 } else { 5.0 };
                    let v = f64::from(self.sensitivity) + step * notches;
                    self.sensitivity = v.round().clamp(0.0, 100.0) as u8;
                    self.grid_mode = false;
                }
                Hit::Beats => {
                    let v = (f64::from(self.beats) + notches).round().clamp(1.0, 64.0) as u32;
                    if v != self.beats {
                        out.extend(self.set_beats(v, None));
                    }
                }
                _ => {}
            }
        } else if l.fields.contains(pos)
            && let Hit::Field(f) = self.hit(pos)
            && let Some(s) = self.selected
            && self.editable()
            && let Some(z) = self.zone(s).cloned()
        {
            let step = f.wheel_step(self.doc_rate) * dy.signum() * dy.abs().ceil();
            let nz =
                zone_edit::with_field(&z, f, zone_edit::field_value(&z, f) + step, self.frames);
            if nz != z {
                let list = self.replaced(s, nz);
                out.extend(list.and_then(|l| self.set_zones(l, None)));
            }
        }
        self.clamp_scrolls();
    }

    /// Runs a registry command the shell offers to this view. `edit.delete` merges the
    /// selected slice into the previous one (the first slice has no marker to remove); when
    /// nothing can be removed, or while a drag is in progress, it is declined so the next
    /// binding runs. Every other command is declined.
    pub fn command(
        &mut self,
        id: &str,
        _arg: Option<&str>,
        doc: &RackDoc,
        registry: &Registry,
    ) -> CommandOutcome {
        self.sync(doc, registry);
        if id != "edit.delete" || !matches!(self.drag, Drag::None) {
            return CommandOutcome::Declined;
        }
        let Some(s) = self.selected.filter(|s| *s >= 1 && self.editable()) else {
            return CommandOutcome::Declined;
        };
        let mut markers = self.markers();
        if s >= markers.len() {
            return CommandOutcome::Declined;
        }
        markers.remove(s);
        let Some(list) = self.zones_for(&markers, &self.zones) else {
            return CommandOutcome::Declined;
        };
        self.selected = Some(s - 1);
        self.repaint = true;
        CommandOutcome::Handled(self.set_zones(list, None).into_iter().collect())
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
                out.push(UiAction::CloseSliceEditor);
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
}

/// The zone list index of slice `slice` (key order) in `zones`.
fn slice_zone_index(zones: &[ZoneDoc], slice: usize) -> Option<usize> {
    let keys = SLICE_KEY_BASE..SLICE_KEY_BASE + MAX_SLICES as u8;
    let mut order: Vec<usize> = (0..zones.len())
        .filter(|i| zones[*i].key_lo == zones[*i].key_hi && keys.contains(&zones[*i].key_lo))
        .collect();
    order.sort_by_key(|i| zones[*i].key_lo);
    order.get(slice).copied()
}
