//! The sequencer grid editor: a pane that replaces the arrangement and edits
//! the patterns of one step sequencer or CV sequencer as a grid of steps (the CV sequencer also
//! shows its two curve lanes).
//!
//! Hosting follows [`crate::StepEditorView`]: on [`UiAction::OpenSeqEditor`] call
//! [`SeqEditorView::open`] (then [`SeqEditorView::set_pattern_index`] with the panel's
//! [`crate::RackView::edited_pattern`]) and show the view in place of the arrangement; on
//! [`UiAction::CloseSeqEditor`] show the arrangement again (also when
//! [`SeqEditorView::is_present`] turns false because the device was deleted). Mirror
//! [`UiAction::SelectPattern`] into the other view. Every minute of editing arrives as
//! `Command::SetPattern` (and the pattern commands); drags use merge ids from 6·2⁴⁰, previews
//! as [`UiAction::AuditionVelocity`]. Feed the device's meter values with
//! [`SeqEditorView::set_meter_values`] for the playing-step cursor.
//!
//! # Layout (logical pixels, inside the bounds)
//!
//! ```text
//! top bar    SEQ EDITOR  <device> · <pattern> · <steps> x <res> · swing                      [x]
//! patterns   PATTERN [1]..[8] [ADD][DUP][DEL] [COPY][PASTE][CLEAR]
//! settings   LENGTH [-] 16 [+]   RES [1/4]..[1/32T]   SWING [ 0 % ]
//! ruler      (key column)  step numbers, beats and bars marked
//! notes      key axis (C4 = 60), one column per step, scrollable
//! tie        a bridge toggle between neighbouring steps
//! VEL        bar per step
//! GATE       bar per step
//! PROB       bar per step
//! CURVE A    bar per step from the zero line, -100..100 %   (CV sequencer only)
//! CURVE B    bar per step from the zero line, -100..100 %   (CV sequencer only)
//! ```
//!
//! # Interaction
//!
//! * **Note lane.** A click on an off step switches it on at the clicked key (the other values
//!   stay); on an on step it moves the key (same key: switches it off on release). A vertical
//!   drag in the pressed column moves the key. The key is auditioned while the button is
//!   down. The key header auditions too. The lane scrolls vertically (wheel) over the whole
//!   keyboard, starting centred on the pattern's range ([`SeqEditorView::key_range`]).
//! * **Bar lanes.** Press sets the step's value from the pointer height, a drag paints every
//!   column it passes (velocity 1..=127, gate 1..=100 %, probability 0..=100 %).
//! * **Curve lanes** (CV sequencer): press sets the step's value from the pointer height
//!   (-100 % at the bottom, 0 on the centre line, 100 % at the top), a drag paints every column
//!   it passes; Alt-click toggles the step's slide (the value ramps to the next step's over its
//!   length; a slide shows as a line to the next bar). Every edit is a `Command::SetCurves`.
//! * **Tie row.** A click on the junction between two steps toggles the tie of the left one.
//! * **Toolbar.** `-`/`+`, a drag (4 px per step) or the wheel set the length 1..=64 (steps
//!   past the length keep their data); the resolution buttons set the step length; the swing
//!   field drags (1 px = 1 %, Shift = 0.1 %), wheels and resets on a double-click. The pattern
//!   slots select the edited pattern (double-click renames in place), ADD / DUP / DEL / COPY /
//!   PASTE / CLEAR work like the rack panel's.
//! * **Gestures.** Every drag is one merged undo step ended with [`UiAction::EndMerge`]; Esc
//!   during a drag restores the pattern of the press. Esc with nothing to cancel and the close
//!   button emit [`UiAction::CloseSeqEditor`]; Ctrl/Cmd+Z / Y undo and redo.

use crate::action::UiAction;
use crate::canvas::Canvas;
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::seq_paint::SeqTheme;
use crate::seq_panel::{self, PlayState, RenameKey};
use crate::theme::Theme;
use crate::tooltip::Help;
use atomik_device_sdk::Registry;
use atomik_model::{
    CURVE_VALUE_MAX, Command, CurveDoc, CurvePatternDoc, DeviceKey, MAX_PATTERNS, MAX_STEPS,
    PatternDoc, RackDoc, SeqResolution, StepDoc,
};
use vello::kurbo::{Point, Rect};
use vello::peniko::Color;

mod paint;

/// The step sequencer type the grid editor edits.
pub const SEQ_TYPE: &str = seq_panel::SEQ_TYPE;
/// The CV sequencer type: edited like the step sequencer, with two more lanes.
pub const CV_SEQ_TYPE: &str = seq_panel::CV_SEQ_TYPE;
/// Velocity levels and bar lanes: height of one bar lane.
pub const LANE_HEIGHT: f64 = 40.0;
/// Height of the tie row.
pub const TIE_HEIGHT: f64 = 16.0;
/// Height of the top bar.
pub const TOP_BAR_HEIGHT: f64 = 28.0;
/// Height of each of the two toolbar rows.
pub const TOOLBAR_HEIGHT: f64 = 28.0;
/// Height of the step ruler.
pub const RULER_HEIGHT: f64 = 16.0;
/// Width of the key and lane-label column.
pub const HEADER_WIDTH: f64 = 52.0;
/// Height of one key row of the note lane.
pub const KEY_ROW_HEIGHT: f64 = 12.0;
/// Pixels of horizontal drag per step of the length field.
pub const LENGTH_DRAG_PIXELS: f64 = 4.0;
const MIN_CELL: f64 = 14.0;
const MAX_CELL: f64 = 44.0;
const PAD: f64 = 8.0;
const SCROLL_STEP: f64 = 36.0;
const MERGE_BASE: u64 = 6 << 40;
const FINE_FACTOR: f64 = 0.1;
const SWING_MAX: u8 = 75;
const MIN_ROWS: u8 = 24;
const MAX_KEY: u8 = 127;

/// The pattern buttons of the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternButton {
    /// Append an empty pattern.
    Add,
    /// Append a copy of the edited pattern.
    Duplicate,
    /// Remove the edited pattern.
    Remove,
    /// Copy the edited pattern to the clipboard.
    Copy,
    /// Paste the clipboard into the edited pattern.
    Paste,
    /// Clear the steps of the edited pattern.
    Clear,
}

impl PatternButton {
    const ALL: [Self; 6] = [
        Self::Add,
        Self::Duplicate,
        Self::Remove,
        Self::Copy,
        Self::Paste,
        Self::Clear,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Add => "ADD",
            Self::Duplicate => "DUP",
            Self::Remove => "DEL",
            Self::Copy => "COPY",
            Self::Paste => "PASTE",
            Self::Clear => "CLEAR",
        }
    }
}

/// A bar lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lane {
    Velocity,
    Gate,
    Probability,
}

impl Lane {
    fn max(self) -> f64 {
        match self {
            Self::Velocity => 127.0,
            Self::Gate | Self::Probability => 100.0,
        }
    }

    fn min(self) -> f64 {
        match self {
            Self::Probability => 0.0,
            _ => 1.0,
        }
    }

    fn get(self, s: &StepDoc) -> u8 {
        match self {
            Self::Velocity => s.velocity,
            Self::Gate => s.gate,
            Self::Probability => s.prob,
        }
    }

    fn set(self, s: &mut StepDoc, v: u8) {
        match self {
            Self::Velocity => s.velocity = v,
            Self::Gate => s.gate = v,
            Self::Probability => s.prob = v,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Velocity => "VEL",
            Self::Gate => "GATE",
            Self::Probability => "PROB",
        }
    }
}

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    None,
    Close,
    Slot(u8),
    Button(PatternButton),
    LengthDec,
    LengthInc,
    LengthField,
    Resolution(SeqResolution),
    Swing,
    Key(u8),
    Note(usize, u8),
    Lane(Lane, usize),
    /// Curve row (0 = A, 1 = B) and step.
    Curve(u8, usize),
    Tie(usize),
}

#[derive(Debug, Clone)]
struct NoteDrag {
    step: usize,
    base: PatternDoc,
    merge: u64,
    key: u8,
    velocity: u8,
    /// Pressed an on step at its own key: a release without a move switches it off.
    pending_off: bool,
    emitted: bool,
}

#[derive(Debug, Clone)]
struct LaneDrag {
    lane: Lane,
    base: PatternDoc,
    last: PatternDoc,
    merge: u64,
    last_step: usize,
    emitted: bool,
}

#[derive(Debug, Clone)]
struct CurveDrag {
    row: usize,
    base: CurvePatternDoc,
    last: CurvePatternDoc,
    merge: u64,
    last_step: usize,
    emitted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Length,
    Swing,
}

#[derive(Debug, Clone)]
struct FieldDrag {
    field: Field,
    base: PatternDoc,
    start_x: f64,
    v0: f64,
    merge: u64,
    last: u8,
    emitted: bool,
}

#[derive(Debug, Clone)]
enum Drag {
    None,
    /// Auditioning a key (header held).
    Audition(u8),
    Note(Box<NoteDrag>),
    Lane(Box<LaneDrag>),
    Curve(Box<CurveDrag>),
    Field(Box<FieldDrag>),
}

/// The rectangles of the view, from its bounds.
#[derive(Debug, Clone, Copy)]
struct Layout {
    top: Rect,
    close: Rect,
    patterns: Rect,
    settings: Rect,
    ruler: Rect,
    notes: Rect,
    tie: Rect,
    velocity: Rect,
    gate: Rect,
    probability: Rect,
    /// Curve rows A and B (empty for a step sequencer).
    curves: [Rect; 2],
    avail_w: f64,
    cell_w: f64,
}

/// An in-place rename of the edited pattern.
#[derive(Debug, Clone)]
struct Rename {
    text: String,
}

/// The sequencer grid editor. See the module documentation.
#[derive(Debug)]
pub struct SeqEditorView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    device: Option<DeviceKey>,
    present: bool,
    name: String,
    accent: Color,
    index: usize,
    /// The patterns of the device, as last synced.
    patterns: Vec<PatternDoc>,
    /// The `pattern` param of the device (1-based plain value).
    pattern_param: f32,
    /// The device is a CV sequencer: the curve lanes are shown.
    is_cv: bool,
    /// The curves of the device, as last synced (`None` is flat).
    curves: Option<CurveDoc>,
    meters: Vec<f32>,
    clipboard: Option<PatternDoc>,
    rename: Option<Rename>,
    /// A pattern that was just added or duplicated and becomes the edited one as soon as the
    /// document shows it.
    want: Option<usize>,
    scroll_x: f64,
    scroll_y: f64,
    drag: Drag,
    hover: Hit,
    next_merge: u64,
    focused: bool,
    repaint: bool,
}

impl SeqEditorView {
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
            index: 0,
            patterns: Vec::new(),
            pattern_param: 1.0,
            is_cv: false,
            curves: None,
            meters: Vec::new(),
            clipboard: None,
            rename: None,
            want: None,
            scroll_x: 0.0,
            scroll_y: 0.0,
            drag: Drag::None,
            hover: Hit::None,
            next_merge: MERGE_BASE,
            focused: false,
            repaint: true,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Opens the editor on a step sequencer, on its first pattern.
    pub fn open(&mut self, device: DeviceKey) {
        if self.device.as_ref() != Some(&device) {
            self.index = 0;
            self.patterns.clear();
            self.is_cv = false;
            self.curves = None;
            self.meters.clear();
            self.scroll_x = 0.0;
            self.scroll_y = 0.0;
        }
        self.device = Some(device);
        self.present = false;
        self.drag = Drag::None;
        self.rename = None;
        self.repaint = true;
    }

    /// Closes the editor (it then ignores input and paints an empty state).
    pub fn close(&mut self) {
        self.device = None;
        self.patterns.clear();
        self.is_cv = false;
        self.curves = None;
        self.present = false;
        self.drag = Drag::None;
        self.rename = None;
        self.repaint = true;
    }

    /// The sequencer being edited.
    pub fn device(&self) -> Option<&DeviceKey> {
        self.device.as_ref()
    }

    /// True while the open device exists in the document and takes patterns. When it turns
    /// false the host shows the arrangement again.
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

    /// The edited pattern, 0-based.
    pub fn pattern_index(&self) -> usize {
        self.index
    }

    /// Selects the edited pattern (clamped to the pattern count once the document is read).
    pub fn set_pattern_index(&mut self, index: usize) {
        let index = if self.patterns.is_empty() {
            index
        } else {
            index.min(self.patterns.len() - 1)
        };
        if index != self.index {
            self.index = index;
            self.rename = None;
            self.repaint = true;
            self.scroll_to_range();
        }
    }

    /// The number of patterns of the device, as last synced.
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }

    /// The edited pattern, as last synced.
    pub fn pattern(&self) -> Option<&PatternDoc> {
        self.patterns.get(self.index)
    }

    /// Whether the curve lanes are shown (the open device is a CV sequencer).
    pub fn has_curve_lanes(&self) -> bool {
        self.present && self.is_cv
    }

    /// The curve rows of the edited pattern, as last synced (flat when the device has none).
    pub fn curve_pattern(&self) -> CurvePatternDoc {
        self.curves
            .as_ref()
            .map_or_else(CurvePatternDoc::default, |c| c.pattern(self.index))
    }

    /// The pattern clipboard (COPY, PASTE).
    pub fn clipboard(&self) -> Option<&PatternDoc> {
        self.clipboard.as_ref()
    }

    /// Sets the pattern clipboard (the app mirrors it with the rack panel's).
    pub fn set_clipboard(&mut self, clip: Option<PatternDoc>) {
        self.clipboard = clip;
    }

    /// The pattern name being edited in place.
    pub fn renaming(&self) -> Option<&str> {
        self.rename.as_ref().map(|r| r.text.as_str())
    }

    /// Whether the view owns the keyboard as a text field (a pattern name is being edited):
    /// the shell then routes plain keys to it instead of to commands.
    pub fn is_text_owner(&self) -> bool {
        self.rename.is_some()
    }

    /// Feeds the device's published meter values `[playing pattern, step, running]` (see
    /// `docs/DEVICES.md`); the cursor is drawn only while the edited pattern plays.
    pub fn set_meter_values(&mut self, values: &[f32]) {
        let q: Vec<f32> = values
            .iter()
            .take(8)
            .map(|v| {
                if v.is_finite() {
                    v.round().clamp(0.0, 255.0)
                } else {
                    0.0
                }
            })
            .collect();
        if q != self.meters {
            self.meters = q;
            self.repaint = true;
        }
    }

    fn play_state(&self) -> PlayState {
        seq_panel::play_state(self.pattern_param, &self.meters, self.patterns.len().max(1))
    }

    /// The step the sequencer started last, while it runs this pattern.
    pub fn cursor(&self) -> Option<usize> {
        let state = self.play_state();
        let len = usize::from(self.pattern()?.length);
        (state.playing == self.index)
            .then_some(state.cursor)
            .flatten()
            .filter(|c| *c < len)
    }

    /// The part of the keyboard the pattern uses, with room around it (at least two octaves):
    /// the note lane starts centred on it. The lane scrolls over all 128 keys.
    pub fn key_range(&self) -> (u8, u8) {
        auto_range(self.pattern())
    }

    /// Vertical scroll of the note lane in pixels.
    pub fn scroll_y(&self) -> f64 {
        self.scroll_y
    }

    /// Horizontal scroll of the grid in pixels.
    pub fn scroll_x(&self) -> f64 {
        self.scroll_x
    }

    // ------------------------------------------------------------------ geometry (public, for tests and hosts)

    fn grid_len(&self) -> usize {
        usize::from(self.pattern().map_or(16, |p| p.length)).clamp(1, MAX_STEPS)
    }

    fn cell_x(&self, step: usize) -> f64 {
        self.bounds.x0 + HEADER_WIDTH + step as f64 * self.layout().cell_w - self.scroll_x
    }

    fn key_y(&self, key: u8) -> f64 {
        self.layout().notes.y0 + f64::from(MAX_KEY - key.min(MAX_KEY)) * KEY_ROW_HEIGHT
            - self.scroll_y
    }

    /// Centre of the note cell of `step` at `key` (may lie outside the visible lane when
    /// scrolled).
    pub fn note_cell_center(&self, step: usize, key: u8) -> Point {
        let x = self.cell_x(step) + self.layout().cell_w / 2.0;
        Point::new(x, self.key_y(key) + KEY_ROW_HEIGHT / 2.0)
    }

    /// Centre of the key header of `key`.
    pub fn key_header_center(&self, key: u8) -> Point {
        Point::new(
            self.bounds.x0 + HEADER_WIDTH / 2.0,
            self.key_y(key) + KEY_ROW_HEIGHT / 2.0,
        )
    }

    fn lane_rect(&self, lane: Lane) -> Rect {
        let l = self.layout();
        match lane {
            Lane::Velocity => l.velocity,
            Lane::Gate => l.gate,
            Lane::Probability => l.probability,
        }
    }

    fn lane_point(&self, lane: Lane, step: usize, frac: f64) -> Point {
        let r = self.lane_rect(lane).inset(-2.0);
        let x = self.cell_x(step) + self.layout().cell_w / 2.0;
        Point::new(x, r.y1 - frac.clamp(0.0, 1.0) * r.height())
    }

    /// The point of the velocity lane at `frac` (0 bottom, 1 top) of `step`.
    pub fn velocity_point(&self, step: usize, frac: f64) -> Point {
        self.lane_point(Lane::Velocity, step, frac)
    }

    /// The point of the gate lane at `frac` of `step`.
    pub fn gate_point(&self, step: usize, frac: f64) -> Point {
        self.lane_point(Lane::Gate, step, frac)
    }

    /// The point of the probability lane at `frac` of `step`.
    pub fn probability_point(&self, step: usize, frac: f64) -> Point {
        self.lane_point(Lane::Probability, step, frac)
    }

    fn curve_inner(&self, row: usize) -> Rect {
        self.layout().curves[row.min(1)].inset(-2.0)
    }

    /// The point of curve row `row` (0 = A, 1 = B) at `step` for a value in percent
    /// (-100..=100; may lie outside the visible lanes when scrolled).
    pub fn curve_point(&self, row: usize, step: usize, value: f64) -> Point {
        let r = self.curve_inner(row);
        let x = self.cell_x(step) + self.layout().cell_w / 2.0;
        Point::new(
            x,
            r.center().y - value.clamp(-100.0, 100.0) / 100.0 * r.height() / 2.0,
        )
    }

    /// The tie toggle between `step` and the next one.
    pub fn tie_center(&self, step: usize) -> Point {
        let l = self.layout();
        Point::new(self.cell_x(step + 1), l.tie.center().y)
    }

    fn btn(&self, row: Rect, x: f64, w: f64) -> Rect {
        Rect::new(x, row.y0 + 4.0, x + w, row.y0 + 24.0)
    }

    fn slot_rect(&self, i: usize) -> Rect {
        let l = self.layout();
        self.btn(
            l.patterns,
            self.bounds.x0 + PAD + 56.0 + i as f64 * 26.0,
            24.0,
        )
    }

    fn pattern_button_rect(&self, b: PatternButton) -> Rect {
        let l = self.layout();
        let widths = [34.0, 34.0, 34.0, 42.0, 48.0, 48.0];
        let i = PatternButton::ALL.iter().position(|x| *x == b).unwrap_or(0);
        let x0 = self.bounds.x0 + PAD + 56.0 + MAX_PATTERNS as f64 * 26.0 + 10.0;
        let x = x0 + widths[..i].iter().map(|w| w + 4.0).sum::<f64>();
        self.btn(l.patterns, x, widths[i])
    }

    fn length_rect(&self, which: u8) -> Rect {
        let l = self.layout();
        let x = self.bounds.x0 + PAD + 56.0;
        match which {
            0 => self.btn(l.settings, x, 20.0),
            1 => self.btn(l.settings, x + 22.0, 44.0),
            _ => self.btn(l.settings, x + 68.0, 20.0),
        }
    }

    fn res_rect(&self, res: SeqResolution) -> Rect {
        let l = self.layout();
        let i = SeqResolution::ALL
            .iter()
            .position(|r| *r == res)
            .unwrap_or(0) as f64;
        self.btn(l.settings, self.bounds.x0 + PAD + 200.0 + i * 40.0, 38.0)
    }

    fn swing_rect(&self) -> Rect {
        let l = self.layout();
        self.btn(l.settings, self.bounds.x0 + PAD + 540.0, 76.0)
    }

    /// Centre of pattern slot `i` (0..8).
    pub fn slot_center(&self, i: usize) -> Point {
        self.slot_rect(i.min(MAX_PATTERNS - 1)).center()
    }

    /// Centre of a pattern button.
    pub fn pattern_button_center(&self, b: PatternButton) -> Point {
        self.pattern_button_rect(b).center()
    }

    /// Centre of the length `+` (`up`) or `-` button.
    pub fn length_button_center(&self, up: bool) -> Point {
        self.length_rect(if up { 2 } else { 0 }).center()
    }

    /// Centre of the length field.
    pub fn length_field_center(&self) -> Point {
        self.length_rect(1).center()
    }

    /// Centre of a resolution button.
    pub fn resolution_button_center(&self, res: SeqResolution) -> Point {
        self.res_rect(res).center()
    }

    /// Centre of the swing field.
    pub fn swing_field_center(&self) -> Point {
        self.swing_rect().center()
    }

    /// Centre of the close button.
    pub fn close_button_center(&self) -> Point {
        self.layout().close.center()
    }

    // ------------------------------------------------------------------ sync

    /// Reads the open step sequencer and its edited pattern from the document. Emits nothing.
    pub fn sync(&mut self, doc: &RackDoc, _registry: &Registry) {
        let Some(key) = &self.device else { return };
        let dev = doc
            .resolve(key)
            .filter(|d| seq_panel::takes_step_doc(&d.type_id));
        let Some(d) = dev else {
            if self.present || !self.patterns.is_empty() {
                self.present = false;
                self.patterns.clear();
                self.drag = Drag::None;
                self.rename = None;
                self.repaint = true;
            }
            return;
        };
        let patterns = d.steps.clone().unwrap_or_default().patterns;
        let is_cv = d.type_id == CV_SEQ_TYPE;
        let curves = d.curves.clone().filter(|_| is_cv);
        let mut index = self.index;
        if let Some(w) = self.want
            && w < patterns.len()
        {
            index = w;
            self.want = None;
        }
        let index = index.min(patterns.len().max(1) - 1);
        let accent = self.st.base.accent_for(d);
        let param = d.params.get("pattern").copied().unwrap_or(1.0);
        let first = self.patterns.is_empty();
        if !self.present
            || self.patterns != patterns
            || self.is_cv != is_cv
            || self.curves != curves
            || self.index != index
            || self.name != d.name
            || self.accent != accent
            || (self.pattern_param - param).abs() > f32::EPSILON
        {
            self.present = true;
            self.index = index;
            self.patterns = patterns;
            self.is_cv = is_cv;
            self.curves = curves;
            self.name.clone_from(&d.name);
            self.accent = accent;
            self.pattern_param = param;
            self.repaint = true;
        }
        if first && !self.patterns.is_empty() {
            self.scroll_to_range();
        }
        self.clamp_scrolls();
    }

    /// Centres the note lane on the pattern's key range.
    fn scroll_to_range(&mut self) {
        let (lo, hi) = self.key_range();
        let mid = (f64::from(lo) + f64::from(hi) + 1.0) / 2.0;
        let h = self.layout().notes.height();
        self.scroll_y = (f64::from(MAX_KEY) + 1.0 - mid) * KEY_ROW_HEIGHT - h / 2.0;
        self.clamp_scrolls();
    }

    // ------------------------------------------------------------------ geometry

    fn layout(&self) -> Layout {
        let b = self.bounds;
        let rect = |x0: f64, y0: f64, x1: f64, y1: f64| Rect::new(x0, y0, x1.max(x0), y1.max(y0));
        let top = rect(b.x0, b.y0, b.x1, b.y0 + TOP_BAR_HEIGHT);
        let close = Rect::new(b.x1 - 30.0, b.y0 + 5.0, b.x1 - 8.0, b.y0 + 23.0);
        let patterns = rect(b.x0, top.y1, b.x1, top.y1 + TOOLBAR_HEIGHT);
        let settings = rect(b.x0, patterns.y1, b.x1, patterns.y1 + TOOLBAR_HEIGHT);
        let ruler = rect(b.x0, settings.y1, b.x1, settings.y1 + RULER_HEIGHT);
        let curve_h = if self.present && self.is_cv {
            LANE_HEIGHT
        } else {
            0.0
        };
        let curve_b = rect(b.x0, b.y1 - curve_h, b.x1, b.y1);
        let curve_a = rect(b.x0, curve_b.y0 - curve_h, b.x1, curve_b.y0);
        let probability = rect(b.x0, curve_a.y0 - LANE_HEIGHT, b.x1, curve_a.y0);
        let gate = rect(b.x0, probability.y0 - LANE_HEIGHT, b.x1, probability.y0);
        let velocity = rect(b.x0, gate.y0 - LANE_HEIGHT, b.x1, gate.y0);
        let tie = rect(b.x0, velocity.y0 - TIE_HEIGHT, b.x1, velocity.y0);
        let notes = rect(b.x0, ruler.y1, b.x1, tie.y0);
        let avail_w = (b.width() - HEADER_WIDTH - PAD).max(1.0);
        let cell_w = (avail_w / self.grid_len() as f64).clamp(MIN_CELL, MAX_CELL);
        Layout {
            top,
            close,
            patterns,
            settings,
            ruler,
            notes,
            tie,
            velocity,
            gate,
            probability,
            curves: [curve_a, curve_b],
            avail_w,
            cell_w,
        }
    }

    fn clamp_scrolls(&mut self) {
        let l = self.layout();
        let content_w = self.grid_len() as f64 * l.cell_w;
        self.scroll_x = self.scroll_x.clamp(0.0, (content_w - l.avail_w).max(0.0));
        let content_h = (f64::from(MAX_KEY) + 1.0) * KEY_ROW_HEIGHT;
        self.scroll_y = self
            .scroll_y
            .clamp(0.0, (content_h - l.notes.height()).max(0.0));
    }

    /// The step column under a logical x, clamped into the pattern.
    fn step_at_x(&self, x: f64) -> usize {
        let k =
            ((x - self.bounds.x0 - HEADER_WIDTH + self.scroll_x) / self.layout().cell_w).floor();
        k.clamp(0.0, (self.grid_len() - 1) as f64) as usize
    }

    fn key_at_y(&self, y: f64) -> u8 {
        let row = ((y - self.layout().notes.y0 + self.scroll_y) / KEY_ROW_HEIGHT).floor();
        (f64::from(MAX_KEY) - row).clamp(0.0, f64::from(MAX_KEY)) as u8
    }

    fn value_at_y(&self, lane: Lane, y: f64) -> u8 {
        value_in(&self.layout(), lane, y)
    }

    /// What the button or field under `pos` does, for its tooltip (`None` during a gesture and
    /// away from a button).
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if !matches!(self.drag, Drag::None) {
            return None;
        }
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        match self.hit(pos) {
            Hit::Close => help(self.layout().close, "Close the sequencer editor"),
            Hit::Slot(i) => Some(Help::new(
                self.slot_rect(usize::from(i)),
                format!("Edit pattern {}", u32::from(i) + 1),
            )),
            Hit::Button(b) => help(
                self.pattern_button_rect(b),
                match b {
                    PatternButton::Add => "Add an empty pattern",
                    PatternButton::Duplicate => "Add a copy of the edited pattern",
                    PatternButton::Remove => "Remove the edited pattern",
                    PatternButton::Copy => "Copy the edited pattern",
                    PatternButton::Paste => "Paste the copied pattern into the edited one",
                    PatternButton::Clear => "Clear the steps of the edited pattern",
                },
            ),
            Hit::LengthDec => help(self.length_rect(0), "One step shorter"),
            Hit::LengthInc => help(self.length_rect(2), "One step longer"),
            Hit::LengthField => help(
                self.length_rect(1),
                "Pattern length in steps: drag or scroll to change, double-click to reset",
            ),
            Hit::Resolution(r) => help(self.res_rect(r), "Step length of the pattern"),
            Hit::Swing => help(
                self.swing_rect(),
                "Swing: drag or scroll to change, double-click to reset",
            ),
            Hit::None
            | Hit::Key(_)
            | Hit::Note(..)
            | Hit::Lane(..)
            | Hit::Curve(..)
            | Hit::Tie(_) => None,
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
            } else {
                Hit::None
            };
        }
        if !self.present || self.pattern().is_none() {
            return Hit::None;
        }
        if l.patterns.contains(pos) {
            for i in 0..MAX_PATTERNS {
                if self.slot_rect(i).contains(pos) {
                    return Hit::Slot(i as u8);
                }
            }
            for b in PatternButton::ALL {
                if self.pattern_button_rect(b).contains(pos) {
                    return Hit::Button(b);
                }
            }
            return Hit::None;
        }
        if l.settings.contains(pos) {
            if self.length_rect(0).contains(pos) {
                return Hit::LengthDec;
            }
            if self.length_rect(2).contains(pos) {
                return Hit::LengthInc;
            }
            if self.length_rect(1).contains(pos) {
                return Hit::LengthField;
            }
            for r in SeqResolution::ALL {
                if self.res_rect(r).contains(pos) {
                    return Hit::Resolution(r);
                }
            }
            if self.swing_rect().contains(pos) {
                return Hit::Swing;
            }
            return Hit::None;
        }
        let grid_x0 = self.bounds.x0 + HEADER_WIDTH;
        let grid_x1 =
            (grid_x0 + self.grid_len() as f64 * l.cell_w - self.scroll_x).min(grid_x0 + l.avail_w);
        let in_grid = pos.x >= grid_x0 && pos.x < grid_x1;
        if l.notes.contains(pos) {
            if pos.x < grid_x0 {
                return Hit::Key(self.key_at_y(pos.y));
            }
            return if in_grid {
                Hit::Note(self.step_at_x(pos.x), self.key_at_y(pos.y))
            } else {
                Hit::None
            };
        }
        if l.tie.contains(pos) {
            let half = (l.cell_w * 0.35).max(6.0);
            return (0..self.grid_len())
                .find(|k| (pos.x - self.cell_x(k + 1)).abs() <= half && pos.x >= grid_x0)
                .map_or(Hit::None, Hit::Tie);
        }
        for lane in [Lane::Velocity, Lane::Gate, Lane::Probability] {
            if self.lane_rect(lane).contains(pos) && in_grid {
                return Hit::Lane(lane, self.step_at_x(pos.x));
            }
        }
        if self.is_cv {
            for (row, r) in l.curves.iter().enumerate() {
                if r.contains(pos) && in_grid {
                    return Hit::Curve(row as u8, self.step_at_x(pos.x));
                }
            }
        }
        Hit::None
    }

    fn merge_id(&mut self) -> u64 {
        let m = self.next_merge;
        self.next_merge += 1;
        m
    }

    fn set(&self, pattern: PatternDoc, merge: Option<u64>) -> Option<UiAction> {
        Some(UiAction::Command(Command::SetPattern {
            device: self.device.clone()?,
            index: self.index as u8,
            pattern,
            merge,
        }))
    }

    fn set_curves(&self, curves: CurvePatternDoc, merge: Option<u64>) -> Option<UiAction> {
        Some(UiAction::Command(Command::SetCurves {
            device: self.device.clone()?,
            index: self.index as u8,
            curves,
            merge,
        }))
    }

    fn audition(&self, key: u8, velocity: u8, on: bool) -> Option<UiAction> {
        Some(UiAction::AuditionVelocity {
            device: self.device.clone()?,
            key,
            velocity: velocity.clamp(1, 127),
            on,
        })
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
        self.rename = None;
        self.want = None;
        if !matches!(self.drag, Drag::None) || button != PointerButton::Primary {
            return;
        }
        let Some(p) = self.pattern().cloned() else {
            if self.hit(pos) == Hit::Close {
                out.push(UiAction::CloseSeqEditor);
            }
            return;
        };
        match self.hit(pos) {
            Hit::Close => out.push(UiAction::CloseSeqEditor),
            Hit::Slot(i) => self.slot_down(usize::from(i), clicks, out),
            Hit::Button(b) => self.button_down(b, &p, out),
            Hit::LengthDec => self.set_length(&p, i32::from(p.length) - 1, out),
            Hit::LengthInc => self.set_length(&p, i32::from(p.length) + 1, out),
            Hit::LengthField => {
                if clicks >= 2 {
                    self.set_length(&p, 16, out);
                } else {
                    self.field_down(Field::Length, &p, pos, out);
                }
            }
            Hit::Resolution(res) => {
                if res != p.resolution {
                    out.extend(self.set(
                        PatternDoc {
                            resolution: res,
                            ..p
                        },
                        None,
                    ));
                }
            }
            Hit::Swing => {
                if clicks >= 2 {
                    if p.swing != 0 {
                        out.extend(self.set(PatternDoc { swing: 0, ..p }, None));
                    }
                } else {
                    self.field_down(Field::Swing, &p, pos, out);
                }
            }
            Hit::Key(key) => {
                out.extend(self.audition(key, 100, true));
                self.drag = Drag::Audition(key);
            }
            Hit::Note(step, key) => self.note_down(&p, step, key, out),
            Hit::Lane(lane, step) => self.lane_down(&p, lane, step, pos, out),
            Hit::Curve(row, step) => {
                self.curve_down(usize::from(row), step, pos, modifiers.alt, out);
            }
            Hit::Tie(step) => {
                let mut q = p;
                q.steps[step].tie = !q.steps[step].tie;
                out.extend(self.set(q, None));
            }
            Hit::None => {}
        }
    }

    fn slot_down(&mut self, i: usize, clicks: u8, out: &mut Vec<UiAction>) {
        if i >= self.patterns.len() {
            return;
        }
        if i != self.index {
            self.set_pattern_index(i);
            if let Some(device) = self.device.clone() {
                out.push(UiAction::SelectPattern {
                    device,
                    index: i as u8,
                });
            }
        }
        if clicks >= 2 {
            self.rename = Some(Rename {
                text: self.patterns[i].name.clone(),
            });
        }
    }

    fn button_down(&mut self, b: PatternButton, p: &PatternDoc, out: &mut Vec<UiAction>) {
        let Some(device) = self.device.clone() else {
            return;
        };
        let (count, edit) = (self.patterns.len(), self.index);
        let select = |this: &mut Self, index: usize, out: &mut Vec<UiAction>| {
            this.set_pattern_index_unclamped(index);
            out.push(UiAction::SelectPattern {
                device: device.clone(),
                index: index as u8,
            });
        };
        match b {
            PatternButton::Add => {
                if count < MAX_PATTERNS {
                    out.push(UiAction::Command(Command::AddPattern {
                        device: device.clone(),
                    }));
                    select(self, count, out);
                }
            }
            PatternButton::Duplicate => {
                if count < MAX_PATTERNS {
                    out.push(UiAction::Command(Command::DuplicatePattern {
                        device: device.clone(),
                        index: edit as u8,
                    }));
                    select(self, count, out);
                }
            }
            PatternButton::Remove => {
                if count > 1 {
                    out.push(UiAction::Command(Command::RemovePattern {
                        device: device.clone(),
                        index: edit as u8,
                    }));
                    let next = edit.min(count - 2);
                    if next != edit {
                        select(self, next, out);
                    }
                }
            }
            PatternButton::Copy => self.clipboard = Some(p.clone()),
            PatternButton::Paste => {
                if let Some(clip) = &self.clipboard {
                    let q = seq_panel::pasted(p, clip);
                    if q != *p {
                        out.extend(self.set(q, None));
                    }
                }
            }
            PatternButton::Clear => {
                let q = seq_panel::cleared(p);
                if q != *p {
                    out.extend(self.set(q, None));
                }
            }
        }
    }

    /// Selects a pattern that may not exist yet (it was just added): the clamp happens at the
    /// next sync.
    fn set_pattern_index_unclamped(&mut self, index: usize) {
        if index >= self.patterns.len() {
            self.want = Some(index);
        }
        if index != self.index {
            self.index = index;
            self.rename = None;
            self.repaint = true;
        }
    }

    fn set_length(&self, p: &PatternDoc, length: i32, out: &mut Vec<UiAction>) {
        let length = length.clamp(1, MAX_STEPS as i32) as u8;
        if length != p.length {
            out.extend(self.set(
                PatternDoc {
                    length,
                    ..p.clone()
                },
                None,
            ));
        }
    }

    fn field_down(&mut self, field: Field, p: &PatternDoc, pos: Point, _out: &mut Vec<UiAction>) {
        let merge = self.merge_id();
        let v0 = match field {
            Field::Length => f64::from(p.length),
            Field::Swing => f64::from(p.swing),
        };
        self.drag = Drag::Field(Box::new(FieldDrag {
            field,
            base: p.clone(),
            start_x: pos.x,
            v0,
            merge,
            last: v0 as u8,
            emitted: false,
        }));
    }

    fn note_down(&mut self, p: &PatternDoc, step: usize, key: u8, out: &mut Vec<UiAction>) {
        let merge = self.merge_id();
        let s = p.steps[step];
        let pending_off = s.on && s.key == key;
        let mut drag = NoteDrag {
            step,
            base: p.clone(),
            merge,
            key,
            velocity: s.velocity,
            pending_off,
            emitted: false,
        };
        out.extend(self.audition(key, s.velocity, true));
        if !pending_off {
            out.extend(self.set(with_note(p, step, key), Some(merge)));
            drag.emitted = true;
        }
        self.drag = Drag::Note(Box::new(drag));
    }

    fn lane_down(
        &mut self,
        p: &PatternDoc,
        lane: Lane,
        step: usize,
        pos: Point,
        out: &mut Vec<UiAction>,
    ) {
        let merge = self.merge_id();
        let v = self.value_at_y(lane, pos.y);
        let q = with_value(p, lane, step, step, v);
        let mut drag = LaneDrag {
            lane,
            base: p.clone(),
            last: q.clone(),
            merge,
            last_step: step,
            emitted: false,
        };
        if q != *p {
            out.extend(self.set(q, Some(merge)));
            drag.emitted = true;
        }
        self.drag = Drag::Lane(Box::new(drag));
    }

    /// A press in a curve lane: Alt toggles the step's slide, else it sets the value and starts
    /// a painting drag.
    fn curve_down(
        &mut self,
        row: usize,
        step: usize,
        pos: Point,
        alt: bool,
        out: &mut Vec<UiAction>,
    ) {
        let base = self.curve_pattern();
        if alt {
            let mut q = base;
            q.rows[row][step].slide = !q.rows[row][step].slide;
            out.extend(self.set_curves(q, None));
            return;
        }
        let merge = self.merge_id();
        let v = curve_value_in(self.curve_inner(row), pos.y);
        let q = with_curve(&base, row, step, step, v);
        let mut drag = CurveDrag {
            row,
            base: base.clone(),
            last: q.clone(),
            merge,
            last_step: step,
            emitted: false,
        };
        if q != base {
            out.extend(self.set_curves(q, Some(merge)));
            drag.emitted = true;
        }
        self.drag = Drag::Curve(Box::new(drag));
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
        let step_here = self.step_at_x(pos.x);
        let key_here = self.key_at_y(pos.y);
        let layout = self.layout();
        let mut emit: Option<(PatternDoc, u64)> = None;
        let mut emit_curves: Option<(CurvePatternDoc, u64)> = None;
        let mut audition: Option<(u8, u8, u8)> = None;
        let curve_inners = [self.curve_inner(0), self.curve_inner(1)];
        match &mut self.drag {
            Drag::None | Drag::Audition(_) => {}
            Drag::Curve(d) => {
                let v = curve_value_in(curve_inners[d.row], pos.y);
                let (a, b) = match step_here.cmp(&d.last_step) {
                    std::cmp::Ordering::Greater => (d.last_step + 1, step_here),
                    std::cmp::Ordering::Less => (step_here, d.last_step - 1),
                    std::cmp::Ordering::Equal => (step_here, step_here),
                };
                let q = with_curve(&d.last, d.row, a, b, v);
                d.last_step = step_here;
                if q != d.last {
                    d.last = q.clone();
                    d.emitted = true;
                    emit_curves = Some((q, d.merge));
                }
            }
            Drag::Note(d) => {
                if key_here != d.key {
                    audition = Some((d.key, key_here, d.velocity));
                    d.key = key_here;
                    d.pending_off = false;
                    d.emitted = true;
                    emit = Some((with_note(&d.base, d.step, key_here), d.merge));
                }
            }
            Drag::Lane(d) => {
                let v = value_in(&layout, d.lane, pos.y);
                // The columns entered since the last move (the one the drag leaves keeps its
                // value); a move inside one column only updates that column.
                let (a, b) = match step_here.cmp(&d.last_step) {
                    std::cmp::Ordering::Greater => (d.last_step + 1, step_here),
                    std::cmp::Ordering::Less => (step_here, d.last_step - 1),
                    std::cmp::Ordering::Equal => (step_here, step_here),
                };
                let q = with_value(&d.last, d.lane, a, b, v);
                d.last_step = step_here;
                if q != d.last {
                    d.last = q.clone();
                    d.emitted = true;
                    emit = Some((q, d.merge));
                }
            }
            Drag::Field(d) => {
                let (v, per_px) = match d.field {
                    Field::Length => (d.v0, 1.0 / LENGTH_DRAG_PIXELS),
                    Field::Swing => (d.v0, if modifiers.shift { FINE_FACTOR } else { 1.0 }),
                };
                let (lo, hi) = match d.field {
                    Field::Length => (1.0, MAX_STEPS as f64),
                    Field::Swing => (0.0, f64::from(SWING_MAX)),
                };
                let n = (v + (pos.x - d.start_x) * per_px).round().clamp(lo, hi) as u8;
                if n != d.last {
                    d.last = n;
                    d.emitted = true;
                    let q = match d.field {
                        Field::Length => PatternDoc {
                            length: n,
                            ..d.base.clone()
                        },
                        Field::Swing => PatternDoc {
                            swing: n,
                            ..d.base.clone()
                        },
                    };
                    emit = Some((q, d.merge));
                }
            }
        }
        if let Some((old, new, vel)) = audition {
            out.extend(self.audition(old, vel, false));
            out.extend(self.audition(new, vel, true));
        }
        if let Some((q, merge)) = emit {
            out.extend(self.set(q, Some(merge)));
        }
        if let Some((q, merge)) = emit_curves {
            out.extend(self.set_curves(q, Some(merge)));
        }
    }

    fn pointer_up(&mut self, out: &mut Vec<UiAction>) {
        self.repaint = true;
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None => {}
            Drag::Audition(key) => out.extend(self.audition(key, 100, false)),
            Drag::Note(d) => {
                let mut emitted = d.emitted;
                if d.pending_off {
                    let mut q = d.base.clone();
                    q.steps[d.step].on = false;
                    out.extend(self.set(q, Some(d.merge)));
                    emitted = true;
                }
                out.extend(self.audition(d.key, d.velocity, false));
                if emitted {
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Lane(d) => {
                if d.emitted {
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Curve(d) => {
                if d.emitted {
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Field(d) => {
                if d.emitted {
                    out.push(UiAction::EndMerge);
                }
            }
        }
    }

    fn cancel_drag(&mut self, out: &mut Vec<UiAction>) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None => {}
            Drag::Audition(key) => out.extend(self.audition(key, 100, false)),
            Drag::Note(d) => {
                out.extend(self.audition(d.key, d.velocity, false));
                if d.emitted {
                    out.extend(self.set(d.base, Some(d.merge)));
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Lane(d) => {
                if d.emitted {
                    out.extend(self.set(d.base, Some(d.merge)));
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Curve(d) => {
                if d.emitted {
                    out.extend(self.set_curves(d.base, Some(d.merge)));
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Field(d) => {
                if d.emitted {
                    out.extend(self.set(d.base, Some(d.merge)));
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
        let Some(p) = self.pattern().cloned() else {
            return;
        };
        let step = (dy.signum() * dy.abs().ceil()) as i32;
        match self.hit(pos) {
            Hit::LengthField | Hit::LengthDec | Hit::LengthInc => {
                self.set_length(&p, i32::from(p.length) + step, out);
                return;
            }
            Hit::Swing => {
                let swing = (i32::from(p.swing) + step).clamp(0, i32::from(SWING_MAX)) as u8;
                if swing != p.swing {
                    out.extend(self.set(PatternDoc { swing, ..p }, None));
                }
                return;
            }
            _ => {}
        }
        let l = self.layout();
        let in_grid_area = [l.notes, l.ruler, l.tie, l.velocity, l.gate, l.probability]
            .iter()
            .chain(&l.curves)
            .any(|r| r.contains(pos));
        if in_grid_area {
            if modifiers.shift || dx.abs() > 1e-9 {
                let d = if dx.abs() > 1e-9 { dx } else { dy };
                self.scroll_x -= d * SCROLL_STEP;
            } else if l.notes.contains(pos) {
                self.scroll_y -= dy * SCROLL_STEP;
            }
        }
        self.clamp_scrolls();
    }

    fn key(&mut self, key: &Key, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        self.repaint = true;
        // Command chords (undo, redo, clipboard) are registry commands the shell resolves;
        // what no command took is not a key of the view.
        if modifiers.command() {
            return;
        }
        if let Some(mut r) = self.rename.take() {
            match seq_panel::rename_key(&mut r.text, key) {
                RenameKey::Editing => self.rename = Some(r),
                RenameKey::Commit => {
                    if let Some(q) = self.pattern().and_then(|p| seq_panel::renamed(p, &r.text)) {
                        out.extend(self.set(q, None));
                    }
                }
                RenameKey::Cancel => {}
            }
            return;
        }
        if *key == Key::Escape {
            if matches!(self.drag, Drag::None) {
                out.push(UiAction::CloseSeqEditor);
            } else {
                self.cancel_drag(out);
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

/// The lane value at a logical y (0 at the bottom of the lane, the maximum at the top).
fn value_in(l: &Layout, lane: Lane, y: f64) -> u8 {
    let r = match lane {
        Lane::Velocity => l.velocity,
        Lane::Gate => l.gate,
        Lane::Probability => l.probability,
    }
    .inset(-2.0);
    let frac = ((r.y1 - y) / r.height().max(1.0)).clamp(0.0, 1.0);
    (frac * lane.max()).round().clamp(lane.min(), lane.max()) as u8
}

/// The curve value in percent at a logical y of a lane's inner rectangle (0 on the centre line,
/// the maximum at the top, the minimum at the bottom).
fn curve_value_in(inner: Rect, y: f64) -> i8 {
    let frac = (inner.center().y - y) / (inner.height() / 2.0).max(1.0);
    let max = f64::from(CURVE_VALUE_MAX);
    (frac * max).round().clamp(-max, max) as i8
}

/// `p` with the value of curve row `row` at steps `a..=b` set to `v`.
fn with_curve(p: &CurvePatternDoc, row: usize, a: usize, b: usize, v: i8) -> CurvePatternDoc {
    let mut q = p.clone();
    for s in &mut q.rows[row][a..=b.min(MAX_STEPS - 1)] {
        s.value = v;
    }
    q
}

/// The auto-scaled key range of a pattern: its used keys with room around them, at least
/// [`MIN_ROWS`] rows, clamped to the keyboard.
fn auto_range(p: Option<&PatternDoc>) -> (u8, u8) {
    let (lo, hi) = p
        .and_then(seq_panel::key_span)
        .map_or((60, 60), |(lo, hi)| {
            (lo.saturating_sub(3), hi.saturating_add(3).min(MAX_KEY))
        });
    let rows = hi - lo + 1;
    let mut lo = i16::from(lo);
    let mut hi = i16::from(hi);
    if rows < MIN_ROWS {
        let extra = i16::from(MIN_ROWS - rows);
        lo -= extra / 2;
        hi += extra - extra / 2;
    }
    if lo < 0 {
        hi -= lo;
        lo = 0;
    }
    if hi > i16::from(MAX_KEY) {
        lo -= hi - i16::from(MAX_KEY);
        hi = i16::from(MAX_KEY);
    }
    (lo.max(0) as u8, hi as u8)
}

/// `p` with `step` on at `key` (the other values stay).
fn with_note(p: &PatternDoc, step: usize, key: u8) -> PatternDoc {
    let mut q = p.clone();
    q.steps[step].on = true;
    q.steps[step].key = key;
    q
}

/// `p` with the lane value of steps `a..=b` set to `v`.
fn with_value(p: &PatternDoc, lane: Lane, a: usize, b: usize, v: u8) -> PatternDoc {
    let mut q = p.clone();
    for s in &mut q.steps[a..=b.min(MAX_STEPS - 1)] {
        if lane.get(s) != v {
            lane.set(s, v);
        }
    }
    q
}

/// Steps per quarter note of a resolution, at least 1 (beat lines).
pub(crate) fn beat_steps(r: SeqResolution) -> usize {
    (15_360 / r.step_ticks() as usize).max(1)
}
