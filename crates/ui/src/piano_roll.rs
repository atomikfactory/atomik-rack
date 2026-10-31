//! The piano roll: one note clip's content on a key × time grid, with a keyboard
//! strip, a ruler and a velocity lane.
//!
//! Hosting follows [`crate::ArrangementView`]: open a clip with [`PianoRollView::open`] when
//! the arrangement emits [`UiAction::OpenPianoRoll`]; every edit emits
//! `SongCommand::SetClipContent` with the full, sorted, valid note list (one merge id per drag,
//! ids from 2·2⁴⁰), and clicks on keys or notes emit [`UiAction::Audition`] on/off pairs.
//! The x axis is in **content ticks** (bar 1 = content tick 0).

use crate::action::{ContextTarget, TransportAction, UiAction};
use crate::arrangement::snap_from_arg;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::chord_edit::{
    self, ARP_GATES, ARP_OCTAVES, ArpOptions, ArpPattern, STRUM_AMOUNTS, Stamp, StrumDirection,
    StrumOptions, strum_ticks,
};
use crate::controller_lane::{
    self as cl, CTL_AREA_HEIGHT, CTL_HEADER_HEIGHT, CTL_PAD, Chip, LaneEditor, Part,
};
use crate::cursor::CursorHint;
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::key_rows::{Fold, KeyRows};
use crate::keymap::{CommandOutcome, GestureClass, ROOT_ARGS, Role, Roles};
use crate::paint::fit_label;
use crate::quantize_panel::QuantizeAction;
use crate::seq_edit::{self, Selection, SpanPart};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::step_drag::{PX_PER_STEP, StepDrag, wheel_steps};
use crate::theme::{Theme, mix};
use crate::timeline::{Bars, Meter, PPQ, Snap, TimeAxis, snap_floor, snap_round};
use crate::tooltip::Help;
use crate::viewport::{Tool, ViewportState};
mod inspector;
pub use inspector::{NOTE_INSPECTOR_HEIGHT, NoteField};

use atomik_device_sdk::Registry;
use atomik_model::theory::{Scale, ScaleKind, detect_scale, pitch_name};
use atomik_model::{
    Clip, ClipContent, ClipId, Command, ControllerKind, ControllerLane, Curve, DeviceKey, Note,
    RackDoc, SongCommand, WorkflowCommand,
};
use std::collections::BTreeSet;
use vello::kurbo::{Affine, Point, Rect, RoundedRect};
use vello::peniko::Color;

/// Height of the top bar (title, snap, quantize, close).
pub const TOP_BAR_HEIGHT: f64 = 28.0;
/// Height of the scale bar under the top bar (scale, fold and chord controls).
pub const SCALE_BAR_HEIGHT: f64 = 22.0;
/// Height of the ruler.
pub const ROLL_RULER_HEIGHT: f64 = 20.0;
/// Width of the keyboard strip.
pub const KEYBOARD_WIDTH: f64 = 56.0;
/// The most characters of a key label the strip shows.
const KEY_LABEL_CHARS: usize = 9;
/// Height of the velocity lane.
pub const VELOCITY_HEIGHT: f64 = 76.0;
/// Height of one key row at the default zoom.
pub const KEY_HEIGHT: f64 = 12.0;
/// Shortest key row of the key zoom, in pixels.
pub const MIN_KEY_HEIGHT: f64 = 6.0;
/// Tallest key row of the key zoom, in pixels.
pub const MAX_KEY_HEIGHT: f64 = 48.0;
/// Zoom factor of one step of the key zoom commands and of a wheel notch.
pub const KEY_ZOOM_STEP: f64 = 1.25;
/// Everything above the ruler: the top bar and the scale bar.
const HEADER_HEIGHT: f64 = TOP_BAR_HEIGHT + SCALE_BAR_HEIGHT;
const VEL_PAD: f64 = 8.0;
const EDGE_PX: f64 = 5.0;
/// How far from the playhead's line a press still grabs it.
const PLAYHEAD_GRAB: f64 = 3.0;
const DRAG_THRESHOLD: f64 = 3.0;
const SCROLL_STEP: f64 = 36.0;
const ZOOM_STEP: f64 = 1.15;
/// Zoom factor of the zoom commands (H and G).
const ZOOM_KEY: f64 = 1.25;
const MERGE_BASE: u64 = 2 << 40;
/// How long a note stepped onto sounds, in seconds.
const STEP_AUDITION_SECS: f64 = 0.25;
const DEFAULT_VELOCITY: f32 = 0.8;
/// Note length / minimum with snapping off (a sixteenth / a 64th).
const OFF_LENGTH: i64 = PPQ / 4;
const MIN_TICKS: i64 = PPQ / 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopItem {
    Snap,
    Quantize,
    QuantizeOptions,
    Close,
    Tool(Tool),
    // The scale bar.
    ScaleRoot,
    ScaleKind,
    ScaleDetect,
    ScaleSnap,
    Fold,
    Chord,
    Inversion,
    /// Shows or hides the controller area under the velocity lane.
    Controllers,
}

impl TopItem {
    /// The items of the scale bar, left to right.
    const SCALE_BAR: [TopItem; 8] = [
        TopItem::ScaleRoot,
        TopItem::ScaleKind,
        TopItem::ScaleDetect,
        TopItem::ScaleSnap,
        TopItem::Fold,
        TopItem::Chord,
        TopItem::Inversion,
        TopItem::Controllers,
    ];
}

/// The fields of the scale bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleBarField {
    /// The root note of the scale (opens a menu of the twelve).
    Root,
    /// The kind of scale (opens a menu).
    Kind,
    /// Detect the scale from the notes.
    Detect,
    /// Snap to scale on or off.
    SnapScale,
    /// The fold (click cycles).
    Fold,
    /// The chord stamp (opens a menu).
    Chord,
    /// The inversion of the stamp (click cycles).
    Inversion,
}

impl ScaleBarField {
    fn item(self) -> TopItem {
        match self {
            ScaleBarField::Root => TopItem::ScaleRoot,
            ScaleBarField::Kind => TopItem::ScaleKind,
            ScaleBarField::Detect => TopItem::ScaleDetect,
            ScaleBarField::SnapScale => TopItem::ScaleSnap,
            ScaleBarField::Fold => TopItem::Fold,
            ScaleBarField::Chord => TopItem::Chord,
            ScaleBarField::Inversion => TopItem::Inversion,
        }
    }
}

/// What the scale, chord and fold menus need to know about the roll: the checks of their items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NoteTools {
    /// The clip's scale (`None`: chromatic).
    pub scale: Option<Scale>,
    /// The root shown: the scale's, or the one a new scale would get.
    pub root: u8,
    /// Snap to scale is on.
    pub snap_scale: bool,
    /// The fold.
    pub fold: Fold,
    /// The chord stamp.
    pub stamp: Stamp,
    /// The stamp's inversion.
    pub inversion: u8,
    /// The arpeggio settings.
    pub arp: ArpOptions,
    /// The strum settings.
    pub strum: StrumOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Top(Option<TopItem>),
    Ruler,
    /// The playhead's line in the grid (a drag locates).
    Playhead,
    Keys(u8),
    Note(usize, SpanPart),
    Grid,
    Velocity(Option<usize>),
    /// A chip of the controller area's header row (with the stepper part under the pointer).
    CtlChip(Chip, Part),
    /// The controller lane's body.
    CtlBody,
    None,
}

/// The note a Left/Right step plays: released after [`STEP_AUDITION_SECS`] (the clock starts at
/// the first [`PianoRollView::tick`] after the step), at the next step or press, or on close.
#[derive(Debug, Clone)]
struct StepNote {
    device: DeviceKey,
    key: u8,
    off_at: Option<f64>,
}

#[derive(Debug, Clone)]
enum Drag {
    None,
    /// Dragging the snap selector: up steps through the resolutions, a press without a drag
    /// cycles forward on release.
    Snap {
        steps: StepDrag,
    },
    Draw {
        start: Point,
        base: Vec<Note>,
        /// The notes being drawn: one, or the chord of the stamp (same start and length).
        notes: Vec<Note>,
        merge: u64,
        last: Vec<Note>,
    },
    Move {
        start: Point,
        originals: Vec<Note>,
        sel: BTreeSet<usize>,
        grabbed: usize,
        /// The row under the pointer at the press (rows count down from the top).
        anchor_row: i32,
        merge: u64,
        moved: bool,
        emitted: bool,
        last: Vec<Note>,
        sounding: u8,
        reselect: Option<usize>,
        /// The copy role was held at the press: a drag adds moved copies, the notes stay.
        copy: bool,
        /// A press with the copy role on a selected note: a click without a drag deselects it.
        toggle_off: Option<usize>,
    },
    Resize {
        start: Point,
        originals: Vec<Note>,
        sel: BTreeSet<usize>,
        grabbed: usize,
        merge: u64,
        moved: bool,
        emitted: bool,
        last: Vec<Note>,
    },
    /// The left edge: the starts move, the ends stay.
    ResizeStart {
        start: Point,
        originals: Vec<Note>,
        sel: BTreeSet<usize>,
        grabbed: usize,
        merge: u64,
        moved: bool,
        emitted: bool,
        last: Vec<Note>,
    },
    Velocity {
        originals: Vec<Note>,
        sel: BTreeSet<usize>,
        grabbed: usize,
        merge: u64,
        last: Vec<Note>,
    },
    /// A stroke on the empty velocity lane: freehand sets every note the pointer passes, the
    /// line role draws a straight ramp from the press. `only` limits it to the selected notes.
    VelocityDraw {
        originals: Vec<Note>,
        only: Option<BTreeSet<usize>>,
        line: bool,
        from: (i64, f32),
        prev: (i64, f32),
        merge: u64,
        last: Vec<Note>,
    },
    Band {
        start: Point,
        pos: Point,
        base: BTreeSet<usize>,
    },
    /// A point drag or a freehand draw in the controller lane (`grabbed` is the dragged point,
    /// `None` when drawing).
    Ctl {
        /// The clip's lanes at the press (a point drag derives every step from them).
        original: Vec<ControllerLane>,
        /// The lanes as last sent.
        current: Vec<ControllerLane>,
        kind: ControllerKind,
        grabbed: Option<usize>,
        start: Point,
        merge: u64,
        moved: bool,
    },
    /// Dragging the playhead: every new snapped tick is located.
    Playhead {
        /// The pointer's offset from the playhead (content ticks) at the press.
        grab: i64,
        last: i64,
    },
    Keys {
        key: u8,
    },
    Pan {
        last: Point,
    },
}

/// The piano roll. See the module documentation.
#[derive(Debug)]
pub struct PianoRollView {
    st: SeqTheme,
    /// The roll without the inspector strip.
    bounds: Rect,
    /// The whole rectangle the host gave the view.
    outer: Rect,
    insp: inspector::NoteInspectorUi,
    scale: f64,
    axis: TimeAxis,
    scroll_y: f64,
    /// Height of one key row (the key zoom).
    key_h: f64,
    /// The visible keys (all of them unless a fold hides some).
    rows: KeyRows,
    rows_dirty: bool,
    fold: Fold,
    snap_scale: bool,
    /// The root a scale gets when it is chosen while the clip has none.
    root_pref: u8,
    stamp: Stamp,
    inversion: u8,
    arp: ArpOptions,
    strum: StrumOptions,
    snap: Snap,
    vp: ViewportState,
    clip_id: Option<ClipId>,
    clip: Option<Clip>,
    /// The document revision and clip id [`Self::sync_at`] last read the clip at.
    clip_rev: Option<(u64, Option<ClipId>)>,
    /// The revision of the document the host hands the next calls (see [`Self::set_doc_revision`]).
    doc_rev: Option<u64>,
    device: Option<DeviceKey>,
    title: String,
    accent: Color,
    meter: Meter,
    /// The grid and ruler of the clip: [`Self::meter`] from bar 1 (content ticks).
    bars: Bars,
    fit_pending: bool,
    selection: Selection<usize>,
    drag: Drag,
    hover: Hit,
    /// The controller area: its state and the point selected in the lane being edited.
    ctl: LaneEditor,
    ctl_sel: Option<usize>,
    next_merge: u64,
    /// Counts the uses of the random note tools: each run is its own take.
    tool_seed: u32,
    /// Names for some keys (the pads of a drum machine), shown on the keyboard strip in place of
    /// the note names; `None` for a track that plays pitches.
    key_labels: Option<Vec<(u8, String)>>,
    /// The anchor and the active end (note indices) of the selection built by stepping with
    /// Shift+Left/Right; only trusted while the selection still has that shape.
    step_sel: Option<(usize, usize)>,
    /// The note a step is sounding, until its release.
    step_note: Option<StepNote>,
    last_length: Option<i64>,
    playhead: i64,
    focused: bool,
    repaint: bool,
}

impl PianoRollView {
    /// Switches to `theme`: the colours are derived again and the view repaints (the accent
    /// follows at the next sync).
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.accent = self.st.base.highlight;
        self.clip_rev = None;
        self.repaint = true;
    }

    /// The device of the track the open clip is on.
    pub fn track_device(&self) -> Option<&DeviceKey> {
        self.device.as_ref()
    }

    /// Names keys on the keyboard strip (a drum machine's pads), or `None` to show note names.
    /// Only the display changes: the keys of the notes are the same.
    pub fn set_key_labels(&mut self, labels: Option<Vec<(u8, String)>>) {
        if self.key_labels != labels {
            self.key_labels = labels;
            self.repaint = true;
        }
    }

    /// The names shown on the keyboard strip, if any.
    pub fn key_labels(&self) -> Option<&[(u8, String)]> {
        self.key_labels.as_deref()
    }

    /// A closed piano roll; call [`Self::set_bounds`], [`Self::open`] and [`Self::sync`].
    pub fn new(theme: Theme) -> Self {
        let st = SeqTheme::from_theme(&theme);
        let accent = st.base.highlight;
        Self {
            st,
            bounds: Rect::new(0.0, 0.0, 800.0, 400.0),
            outer: Rect::new(0.0, 0.0, 800.0, 400.0),
            insp: inspector::NoteInspectorUi::default(),
            scale: 1.0,
            axis: TimeAxis::new(KEYBOARD_WIDTH, 200.0),
            scroll_y: 0.0,
            key_h: KEY_HEIGHT,
            rows: KeyRows::all(),
            rows_dirty: false,
            fold: Fold::Off,
            snap_scale: false,
            root_pref: 0,
            stamp: Stamp::None,
            inversion: 0,
            arp: ArpOptions::default(),
            strum: StrumOptions::default(),
            snap: Snap::default(),
            vp: ViewportState::new(),
            clip_id: None,
            clip_rev: None,
            doc_rev: None,
            clip: None,
            device: None,
            title: String::new(),
            accent,
            meter: Meter::default(),
            bars: Bars::default(),
            fit_pending: false,
            selection: Selection::new(),
            drag: Drag::None,
            hover: Hit::None,
            ctl: LaneEditor::default(),
            ctl_sel: None,
            next_merge: MERGE_BASE,
            tool_seed: 0,
            key_labels: None,
            step_sel: None,
            step_note: None,
            last_length: None,
            playhead: 0,
            focused: false,
            repaint: true,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Opens a note clip (zooms to fit it on the next sync).
    pub fn open(&mut self, id: ClipId) {
        if self.clip_id != Some(id) {
            self.selection.clear();
        }
        self.clip_id = Some(id);
        self.clip = None;
        self.clip_rev = None;
        self.drag = Drag::None;
        self.fit_pending = true;
        self.rows_dirty = true;
        self.repaint = true;
    }

    /// Closes the roll (it then ignores input and paints an empty state).
    pub fn close(&mut self) {
        self.clip_id = None;
        self.clip = None;
        self.clip_rev = None;
        self.device = None;
        self.drag = Drag::None;
        self.selection.clear();
        self.repaint = true;
    }

    /// The open clip.
    pub fn clip(&self) -> Option<ClipId> {
        self.clip_id
    }

    /// Places the view in the window (logical pixels).
    pub fn set_bounds(&mut self, outer: Rect) {
        self.outer = outer;
        let strip = if self.insp.visible {
            inspector::NOTE_INSPECTOR_HEIGHT.min(outer.height() / 2.0)
        } else {
            0.0
        };
        let bounds = Rect::new(outer.x0, outer.y0, outer.x1, outer.y1 - strip);
        if bounds != self.bounds {
            self.bounds = bounds;
            self.axis.x0 = bounds.x0 + KEYBOARD_WIDTH;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// The view's rectangle (the inspector strip included).
    #[allow(
        clippy::misnamed_getters,
        reason = "`bounds` is the roll without the inspector strip"
    )]
    pub fn bounds(&self) -> Rect {
        self.outer
    }

    /// Sets the display scale factor (also set by [`UiEvent::ScaleChanged`]).
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Sets the transport position (song ticks), drawn where the clip plays it.
    pub fn set_playhead(&mut self, tick: i64) {
        if tick != self.playhead {
            let before = self.playhead_content();
            self.playhead = tick;
            // The line is drawn on a whole logical pixel: nothing to paint while it stays put.
            let after = self.playhead_content();
            let px = |t: Option<i64>| t.map(|t| self.axis.tick_to_x(t).round() as i64);
            if px(before) != px(after) {
                self.repaint = true;
            }
            if self.vp.following()
                && let Some(c) = after
                && self.axis.page_to(c, self.bounds.x1)
            {
                self.repaint = true;
            }
        }
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

    /// The snap resolution.
    pub fn snap(&self) -> Snap {
        self.snap
    }

    /// Sets the snap resolution.
    pub fn set_snap(&mut self, snap: Snap) {
        self.snap = snap;
        self.repaint = true;
    }

    /// The mouse cursor to show: a horizontal resize cursor over a note's edge or the playhead's
    /// line (and during an edge or playhead drag), a move cursor over a note body with Select
    /// (and during a move), the tool's own cursor (pen, eraser, blade) over the grid, and a
    /// crosshair while banding.
    pub fn cursor(&self) -> CursorHint {
        match &self.drag {
            Drag::Resize { .. } | Drag::ResizeStart { .. } => return CursorHint::ResizeHorizontal,
            Drag::Move { .. } => return CursorHint::Move,
            Drag::Playhead { .. } => return CursorHint::ResizeHorizontal,
            Drag::Draw { .. } => return CursorHint::Pen,
            Drag::Band { .. } => return CursorHint::Crosshair,
            Drag::None => {}
            _ => return CursorHint::Default,
        }
        let tool = self.shown_tool();
        match self.hover {
            Hit::Note(_, SpanPart::Start | SpanPart::End) if tool == Tool::Select => {
                CursorHint::ResizeHorizontal
            }
            Hit::Note(_, SpanPart::Body) if tool == Tool::Select => CursorHint::Move,
            Hit::Playhead => CursorHint::ResizeHorizontal,
            // The edit tools show their own cursor over the grid; a note edge resizes only
            // with Select, and the pencil does nothing on a note.
            Hit::Grid if tool != Tool::Select => CursorHint::of_tool(tool),
            Hit::Note(..) if matches!(tool, Tool::Eraser | Tool::Razor) => {
                CursorHint::of_tool(tool)
            }
            _ => CursorHint::Default,
        }
    }

    /// The active tool.
    pub fn tool(&self) -> Tool {
        self.vp.tool()
    }

    /// The tool the toolbar shows: the active one, or its alternate while the alternate-tool
    /// role is held.
    pub fn shown_tool(&self) -> Tool {
        self.vp.shown_tool()
    }

    /// Selects a tool (the `tool.*` commands and the toolbar buttons do this).
    pub fn set_tool(&mut self, tool: Tool) {
        // The speaker is not a note tool.
        if Tool::NOTE.contains(&tool) && self.vp.set_tool(tool) {
            self.repaint = true;
        }
    }

    /// Sets the modifier role table (the shell pushes it when the keymap changes).
    pub fn set_roles(&mut self, roles: Roles) {
        if self.vp.set_roles(roles) {
            self.repaint = true;
        }
    }

    /// Whether follow is on.
    pub fn follow(&self) -> bool {
        self.vp.follow()
    }

    /// Switches follow on or off (the workflow preference sets the start state).
    pub fn set_follow(&mut self, on: bool) {
        self.vp.set_follow(on);
        self.repaint = true;
    }

    /// The transport runs or is stopped: follow only pages while it runs.
    pub fn set_playing(&mut self, playing: bool) {
        self.vp.set_playing(playing);
    }

    /// Centre of a toolbar button in the top bar.
    pub fn tool_button_center(&self, tool: Tool) -> Point {
        self.top_rect(TopItem::Tool(tool)).center()
    }

    /// Zoom, logical pixels per tick.
    pub fn px_per_tick(&self) -> f64 {
        self.axis.px_per_tick
    }

    /// Selected notes (indices into the clip's sorted note list).
    pub fn selected_notes(&self) -> Vec<usize> {
        self.selection.items().iter().copied().collect()
    }

    /// Replaces the note selection.
    pub fn set_selected_notes(&mut self, indices: &[usize]) {
        self.selection.set(indices.iter().copied());
        self.repaint = true;
    }

    /// Logical x of a content tick.
    pub fn tick_to_x(&self, tick: i64) -> f64 {
        self.axis.tick_to_x(tick)
    }

    /// The content tick at a logical x.
    pub fn x_to_tick(&self, x: f64) -> i64 {
        self.axis.x_to_tick(x)
    }

    /// Height of one key row in pixels (the key zoom, [`KEY_HEIGHT`] by default).
    pub fn key_height(&self) -> f64 {
        self.key_h
    }

    /// Sets the key row height (clamped to [`MIN_KEY_HEIGHT`]`..=`[`MAX_KEY_HEIGHT`], 12 for
    /// anything that is not a number), keeping the key at the middle of the grid where it is.
    pub fn set_key_height(&mut self, height: f64) {
        let mid = {
            let g = self.grid_rect();
            (g.y0 + g.y1) / 2.0
        };
        self.zoom_keys_to(height, mid);
    }

    /// Sets the key row height keeping the key under `anchor_y` under it.
    fn zoom_keys_to(&mut self, height: f64, anchor_y: f64) {
        let h = if height.is_finite() {
            height.clamp(MIN_KEY_HEIGHT, MAX_KEY_HEIGHT)
        } else {
            KEY_HEIGHT
        };
        if (h - self.key_h).abs() <= f64::EPSILON {
            return;
        }
        let y0 = self.grid_rect().y0;
        // The (fractional) key row under the anchor, counted from the top of the keyboard.
        let row = (anchor_y - y0 + self.scroll_y) / self.key_h;
        self.key_h = h;
        self.scroll_y = row * h - (anchor_y - y0);
        self.clamp_scroll();
        self.repaint = true;
    }

    /// Vertical centre of a key row.
    pub fn key_center_y(&self, key: u8) -> f64 {
        self.key_top(key) + self.key_h / 2.0
    }

    /// The key at a logical y (clamped to the visible rows).
    pub fn key_at(&self, y: f64) -> u8 {
        self.rows.keys()[self.row_at(y)]
    }

    /// A point on the keyboard strip for `key`.
    pub fn keyboard_point(&self, key: u8) -> Point {
        Point::new(
            self.bounds.x0 + KEYBOARD_WIDTH - 8.0,
            self.key_center_y(key),
        )
    }

    /// A note's rectangle.
    pub fn note_rect(&self, index: usize) -> Option<Rect> {
        self.notes().get(index).map(|n| self.rect_of(n))
    }

    /// A point in the velocity lane on note `index`'s bar at velocity `v`.
    pub fn velocity_point(&self, index: usize, v: f32) -> Option<Point> {
        let n = self.notes().get(index)?;
        let (top, h) = self.velocity_range();
        Some(Point::new(
            self.axis.tick_to_x(n.tick) + 1.0,
            top + (1.0 - f64::from(v)) * h,
        ))
    }

    /// The velocity lane.
    pub fn velocity_rect(&self) -> Rect {
        let b = self.bounds;
        let y1 = (b.y1 - self.ctl_height()).max(b.y0);
        Rect::new(self.axis.x0, (y1 - VELOCITY_HEIGHT).max(b.y0), b.x1, y1)
    }

    /// Whether the controller area (pitch bend, aftertouch and CC lanes) is shown.
    pub fn controllers_open(&self) -> bool {
        self.ctl.open
    }

    /// Shows or hides the controller area under the velocity lane.
    pub fn set_controllers_open(&mut self, open: bool) {
        if open != self.ctl.open {
            self.ctl.open = open;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// The controller being edited.
    pub fn controller_kind(&self) -> ControllerKind {
        self.ctl.kind
    }

    /// Chooses the controller to edit (the matching stepper follows the number).
    pub fn set_controller_kind(&mut self, kind: ControllerKind) {
        self.ctl.select(kind);
        self.ctl_sel = None;
        self.repaint = true;
    }

    /// The curve of the points the pencil adds.
    pub fn controller_curve(&self) -> Curve {
        self.ctl.curve
    }

    /// The selected point of the controller lane being edited.
    pub fn controller_selection(&self) -> Option<usize> {
        self.ctl_sel
    }

    /// Centre of a chip of the controller area's header row (tests click it).
    pub fn controller_chip_center(&self, chip: Chip) -> Point {
        cl::chip_rect(self.axis.x0, self.controller_area().y0, chip).center()
    }

    /// The point of the controller area at content `tick` and `value` of `kind`.
    pub fn controller_point(&self, kind: ControllerKind, tick: i64, value: f32) -> Point {
        let (top, h) = self.ctl_range();
        Point::new(
            self.axis.tick_to_x(tick),
            cl::y_of_value(kind, value, top, h),
        )
    }

    /// The height the controller area takes at the bottom of the roll (0 while it is hidden).
    fn ctl_height(&self) -> f64 {
        if self.ctl.open { CTL_AREA_HEIGHT } else { 0.0 }
    }

    /// The controller area: the chip row and the lane, with the side panel (an empty rectangle
    /// while the area is hidden).
    pub fn controller_area(&self) -> Rect {
        let b = self.bounds;
        let h = self.ctl_height();
        Rect::new(b.x0, (b.y1 - h).max(b.y0), b.x1, b.y1)
    }

    /// The lane body of the controller area, on the roll's time axis.
    pub fn controller_rect(&self) -> Rect {
        let a = self.controller_area();
        Rect::new(
            self.axis.x0,
            (a.y0 + CTL_HEADER_HEIGHT).min(a.y1),
            a.x1,
            a.y1,
        )
    }

    /// `(top, height)` of the value range inside the lane body (the top is the highest value).
    fn ctl_range(&self) -> (f64, f64) {
        let r = self.controller_rect();
        (r.y0 + CTL_PAD, (r.height() - 2.0 * CTL_PAD).max(1.0))
    }

    /// The y of the ruler (click to locate).
    pub fn ruler_y(&self) -> f64 {
        self.bounds.y0 + HEADER_HEIGHT + ROLL_RULER_HEIGHT / 2.0
    }

    /// Centre of the close button.
    pub fn close_button_center(&self) -> Point {
        self.top_rect(TopItem::Close).center()
    }

    /// Centre of the quantize button.
    pub fn quantize_button_center(&self) -> Point {
        self.top_rect(TopItem::Quantize).center()
    }

    /// Centre of the quantize options button.
    pub fn quantize_options_button_center(&self) -> Point {
        self.top_rect(TopItem::QuantizeOptions).center()
    }

    /// Centre of the snap selector.
    pub fn snap_button_center(&self) -> Point {
        self.top_rect(TopItem::Snap).center()
    }

    // ------------------------------------------------------------------ scale and chords

    /// The scale of the open clip (`None`: chromatic, no scale).
    pub fn scale(&self) -> Option<Scale> {
        self.clip.as_ref().and_then(|c| c.scale)
    }

    /// Whether pencil-drawn notes and vertical drags land on in-scale keys only and the
    /// transpose commands move by scale degree (it needs a scale to do anything).
    pub fn snap_to_scale(&self) -> bool {
        self.snap_scale
    }

    /// Switches snap to scale on or off (saved in the window state).
    pub fn set_snap_to_scale(&mut self, on: bool) {
        if on != self.snap_scale {
            self.snap_scale = on;
            self.repaint = true;
        }
    }

    /// Which keys the roll shows.
    pub fn fold(&self) -> Fold {
        self.fold
    }

    /// Sets the fold (saved in the window state), keeping the key at the middle of the grid
    /// where it is.
    pub fn set_fold(&mut self, fold: Fold) {
        if fold != self.fold {
            self.fold = fold;
            self.rows_dirty = true;
            self.refresh_rows(true);
            self.repaint = true;
        }
    }

    /// The visible keys, top row first.
    pub fn rows(&self) -> &KeyRows {
        &self.rows
    }

    /// The chord the pencil drops.
    pub fn stamp(&self) -> Stamp {
        self.stamp
    }

    /// Sets the chord the pencil drops (and chordify builds).
    pub fn set_stamp(&mut self, stamp: Stamp) {
        self.stamp = stamp;
        self.repaint = true;
    }

    /// The inversion of the stamp, 0 (root position) to 3.
    pub fn inversion(&self) -> u8 {
        self.inversion
    }

    /// Sets the inversion of the stamp (limited to 0..=3).
    pub fn set_inversion(&mut self, inversion: u8) {
        self.inversion = inversion.min(3);
        self.repaint = true;
    }

    /// The arpeggio settings.
    pub fn arp_options(&self) -> ArpOptions {
        self.arp
    }

    /// The strum settings.
    pub fn strum_options(&self) -> StrumOptions {
        self.strum
    }

    /// The state the scale, chord and fold menus show their checks from.
    pub fn tools(&self) -> NoteTools {
        NoteTools {
            scale: self.scale(),
            root: self.scale().map_or(self.root_pref, |s| s.root),
            snap_scale: self.snap_scale,
            fold: self.fold,
            stamp: self.stamp,
            inversion: self.inversion,
            arp: self.arp,
            strum: self.strum,
        }
    }

    /// Centre of one of the scale bar fields (tests click it).
    pub fn scale_bar_center(&self, field: ScaleBarField) -> Point {
        self.top_rect(field.item()).center()
    }

    /// The row under a logical y, counted from the top row (clamped to the rows).
    pub fn row_at(&self, y: f64) -> usize {
        let r = ((y - self.grid_rect().y0 + self.scroll_y) / self.key_h).floor();
        r.clamp(0.0, (self.rows.len() - 1) as f64) as usize
    }

    /// Rebuilds the rows when the clip, the scale or the fold changed. A drag keeps the rows it
    /// started with (the keys under the pointer must not move while notes are being moved), so
    /// the rebuild waits for the end of the gesture. With `anchor` the key at the middle of the
    /// grid stays there.
    fn refresh_rows(&mut self, anchor: bool) {
        if !self.rows_dirty || !matches!(self.drag, Drag::None) {
            return;
        }
        self.rows_dirty = false;
        let used: Vec<u8> = self.notes().iter().map(|n| n.key).collect();
        let rows = KeyRows::build(self.fold, self.scale(), &used);
        if rows == self.rows {
            return;
        }
        let g = self.grid_rect();
        let mid = self.key_at((g.y0 + g.y1) / 2.0);
        self.rows = rows;
        if anchor {
            let r = self.rows.nearest_row(mid) as f64;
            self.scroll_y = r * self.key_h + self.key_h / 2.0 - g.height() / 2.0;
        }
        self.clamp_scroll();
        self.repaint = true;
    }

    // ------------------------------------------------------------------ sync

    /// Reads the open clip from the document. Emits nothing.
    pub fn sync(&mut self, doc: &RackDoc, registry: &Registry) {
        self.clip_rev = None;
        self.sync_from(doc, registry, None);
    }

    /// Tells the view the revision ([`atomik_model::Document::revision`]) of the document the
    /// next `handle`, `command` and `paint` calls get, so their own sync can skip the search
    /// of the open clip while it is unchanged; `None` (the default) reads it every time. The
    /// host sets it right before such a call and clears it right after: a stale number would
    /// hide a change.
    pub fn set_doc_revision(&mut self, revision: Option<u64>) {
        self.doc_rev = revision;
    }

    /// The sync `handle`, `command` and `paint` start with.
    fn sync_current(&mut self, doc: &RackDoc, registry: &Registry) {
        match self.doc_rev {
            Some(revision) => self.sync_at(doc, registry, revision),
            None => self.sync(doc, registry),
        }
    }

    /// [`Self::sync`] for a document known by its revision ([`atomik_model::Document::revision`]):
    /// while the revision and the open clip are the ones the clip was last read at, the document
    /// is not searched or compared for it again.
    pub fn sync_at(&mut self, doc: &RackDoc, registry: &Registry, revision: u64) {
        self.sync_from(doc, registry, Some(revision));
    }

    fn sync_from(&mut self, doc: &RackDoc, _registry: &Registry, revision: Option<u64>) {
        let Some(id) = self.clip_id else {
            return;
        };
        if let Some(rev) = revision
            && self.clip_rev == Some((rev, Some(id)))
        {
            self.sync_rest(doc);
            return;
        }
        self.clip_rev = revision.map(|rev| (rev, Some(id)));
        let found = doc.song.tracks.iter().find_map(|t| {
            t.lanes
                .iter()
                .find_map(|l| l.clips.iter().find(|c| c.id == id))
                .map(|c| (t, c))
        });
        // The clip grid counts content ticks from the clip start: the meter in effect there.
        let meter = Meter::at(&doc.song, found.map_or(0, |(_, c)| c.start));
        // Compared in place: the clip (every note and controller lane) is cloned only when it
        // differs from the one the view holds.
        let same_clip = match found {
            Some((t, c)) => {
                self.clip.as_ref() == Some(c) && self.device.as_ref() == Some(&t.device)
            }
            None => self.clip.is_none() && self.device.is_none(),
        };
        if !same_clip || meter != self.meter {
            self.rows_dirty = true;
            let (clip, device) = match found {
                Some((t, c)) => (Some(c.clone()), Some(t.device.clone())),
                None => (None, None),
            };
            self.clip = clip;
            self.device = device;
            self.meter = meter;
            self.bars = Bars::single(meter);
            let n = self.notes().len();
            self.selection.retain(|i| *i < n);
            if let Some(s) = self.scale() {
                self.root_pref = s.root;
            }
            self.repaint = true;
        }
        self.sync_rest(doc);
    }

    /// The part of the sync that follows the view's own state: rows, accent, title, fit.
    fn sync_rest(&mut self, doc: &RackDoc) {
        self.refresh_rows(false);
        if let Some(d) = &self.device {
            let accent = sp::device_accent(&self.st.base, doc, d);
            let title = sp::track_name(doc, d);
            if accent != self.accent || title != self.title {
                self.accent = accent;
                self.title = title;
                self.repaint = true;
            }
        }
        if self.fit_pending && self.clip.is_some() {
            self.fit();
            self.fit_pending = false;
        }
    }

    fn fit(&mut self) {
        let (_, z) = self.region();
        let end = self
            .notes()
            .iter()
            .map(|n| n.tick + n.length)
            .max()
            .unwrap_or(0)
            .max(z);
        let width = (self.bounds.x1 - self.axis.x0 - 24.0).max(40.0);
        let span = end.max(PPQ) as f64;
        self.axis.px_per_tick = (width / span).clamp(
            crate::timeline::MIN_PX_PER_TICK,
            crate::timeline::MAX_PX_PER_TICK,
        );
        self.axis.scroll = 0.0;
        let notes = self.notes();
        let mid = if notes.is_empty() {
            60.0
        } else {
            let lo = notes.iter().map(|n| n.key).min().unwrap_or(60);
            let hi = notes.iter().map(|n| n.key).max().unwrap_or(60);
            (f64::from(lo) + f64::from(hi)) / 2.0
        };
        let g = self.grid_rect();
        let row = self.rows.nearest_row(mid.round().clamp(0.0, 127.0) as u8) as f64;
        self.scroll_y = row * self.key_h + self.key_h / 2.0 - g.height() / 2.0;
        self.clamp_scroll();
    }

    // ------------------------------------------------------------------ geometry

    /// The notes of the open clip, sorted (empty when none is open).
    pub fn notes(&self) -> &[Note] {
        match self.clip.as_ref().map(|c| &c.content) {
            Some(ClipContent::Notes(n)) => n,
            _ => &[],
        }
    }

    /// The content range the clip plays: `[0, loop)` when looped, else
    /// `[offset, offset + length)`.
    fn region(&self) -> (i64, i64) {
        match &self.clip {
            Some(c) => match c.loop_length {
                Some(n) if n > 0 => (0, n),
                _ => (c.offset, c.offset + c.length),
            },
            None => (0, 4 * PPQ),
        }
    }

    fn grid_rect(&self) -> Rect {
        let b = self.bounds;
        let y0 = b.y0 + HEADER_HEIGHT + ROLL_RULER_HEIGHT;
        let y1 = (b.y1 - self.ctl_height() - VELOCITY_HEIGHT).max(y0);
        Rect::new(self.axis.x0, y0, b.x1.max(self.axis.x0), y1)
    }

    fn velocity_range(&self) -> (f64, f64) {
        let r = self.velocity_rect();
        (r.y0 + VEL_PAD, (r.height() - 2.0 * VEL_PAD).max(1.0))
    }

    /// The top of a key's row (a key a fold hides is drawn on the nearest row).
    fn key_top(&self, key: u8) -> f64 {
        self.row_top(self.rows.nearest_row(key))
    }

    fn row_top(&self, row: usize) -> f64 {
        self.grid_rect().y0 + row as f64 * self.key_h - self.scroll_y
    }

    /// The rows that show between `y0` and `y1` (logical y), first to last.
    fn visible_rows(&self, y0: f64, y1: f64) -> std::ops::RangeInclusive<usize> {
        self.row_at(y0)..=self.row_at(y1 - 0.01)
    }

    fn rect_of(&self, n: &Note) -> Rect {
        let y = self.key_top(n.key);
        Rect::new(
            self.axis.tick_to_x(n.tick),
            y,
            self.axis.tick_to_x(n.tick + n.length),
            y + self.key_h,
        )
    }

    fn clamp_scroll(&mut self) {
        let max = (self.rows.len() as f64 * self.key_h - self.grid_rect().height()).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max);
    }

    fn top_rect(&self, item: TopItem) -> Rect {
        let b = self.bounds;
        let y = b.y0 + 5.0;
        match item {
            TopItem::Close => Rect::new(b.x1 - 30.0, y, b.x1 - 8.0, y + 18.0),
            TopItem::Quantize => Rect::new(b.x1 - 118.0, y, b.x1 - 66.0, y + 18.0),
            TopItem::QuantizeOptions => Rect::new(b.x1 - 62.0, y, b.x1 - 40.0, y + 18.0),
            TopItem::Snap => Rect::new(b.x1 - 186.0, y, b.x1 - 132.0, y + 18.0),
            TopItem::Tool(t) => {
                let i = Tool::NOTE.iter().position(|x| *x == t).unwrap_or(0) as f64;
                let x = b.x1 - 294.0 + i * 25.0;
                Rect::new(x, y, x + 23.0, y + 18.0)
            }
            scale_bar => {
                let (x0, x1) = match scale_bar {
                    TopItem::ScaleRoot => (56.0, 90.0),
                    TopItem::ScaleKind => (94.0, 206.0),
                    TopItem::ScaleDetect => (210.0, 240.0),
                    TopItem::ScaleSnap => (244.0, 300.0),
                    TopItem::Fold => (336.0, 386.0),
                    TopItem::Chord => (432.0, 502.0),
                    TopItem::Controllers => (556.0, 646.0),
                    _ => (506.0, 548.0),
                };
                let y = b.y0 + TOP_BAR_HEIGHT + 3.0;
                Rect::new(b.x0 + x0, y, b.x0 + x1, y + 16.0)
            }
        }
    }

    fn step(&self, modifiers: Modifiers) -> Option<i64> {
        self.vp
            .step(self.snap, self.meter, self.axis.px_per_tick, modifiers)
    }

    fn content_to_song(&self, tick: i64) -> i64 {
        let Some(c) = &self.clip else { return tick };
        let windows = seq_edit::content_windows(c);
        windows
            .iter()
            .find(|w| {
                tick >= w.content_start && tick < w.content_start + (w.song_end - w.song_start)
            })
            .map_or(c.start - c.offset + tick, |w| {
                w.song_start + tick - w.content_start
            })
    }

    fn hit(&self, pos: Point) -> Hit {
        let b = self.bounds;
        if !b.contains(pos) || self.clip.is_none() {
            return Hit::None;
        }
        if pos.y < b.y0 + HEADER_HEIGHT {
            let item = [
                TopItem::Snap,
                TopItem::Quantize,
                TopItem::QuantizeOptions,
                TopItem::Close,
            ]
            .into_iter()
            .chain(Tool::NOTE.into_iter().map(TopItem::Tool))
            .chain(TopItem::SCALE_BAR)
            .find(|i| self.top_rect(*i).contains(pos));
            return Hit::Top(item);
        }
        let g = self.grid_rect();
        if pos.y < g.y0 {
            return if pos.x >= g.x0 { Hit::Ruler } else { Hit::None };
        }
        if self.ctl.open && pos.y >= self.velocity_rect().y1 {
            let a = self.controller_area();
            if pos.y < a.y0 + CTL_HEADER_HEIGHT {
                return match cl::chip_at(self.axis.x0, a.y0, pos.x, pos.y) {
                    Some((chip, part)) => Hit::CtlChip(chip, part),
                    None => Hit::None,
                };
            }
            return if pos.x >= self.axis.x0 {
                Hit::CtlBody
            } else {
                Hit::None
            };
        }
        if pos.y >= g.y1 {
            if pos.x < g.x0 {
                return Hit::None;
            }
            return Hit::Velocity(self.velocity_bar_at(pos.x));
        }
        if pos.x < g.x0 {
            return Hit::Keys(self.key_at(pos.y));
        }
        for (i, n) in self.notes().iter().enumerate().rev() {
            let r = self.rect_of(n);
            if pos.y >= r.y0
                && pos.y < r.y1
                && let Some(part) =
                    seq_edit::hit_span_edges(pos.x, r.x0, r.x1.max(r.x0 + 3.0), EDGE_PX)
            {
                return Hit::Note(i, part);
            }
        }
        // The playhead's line, only where no note was hit, with the Select tool and clear of
        // the grid's left edge (a stopped playhead sits there, and a band starts there).
        if self.vp.shown_tool() == Tool::Select
            && self.playhead_content().is_some_and(|t| {
                let x = self.axis.tick_to_x(t);
                x - g.x0 > PLAYHEAD_GRAB && (x - pos.x).abs() <= PLAYHEAD_GRAB
            })
        {
            return Hit::Playhead;
        }
        Hit::Grid
    }

    /// What the button under `pos` does, for its tooltip (`None` during a gesture and away from
    /// a button).
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        if !matches!(self.drag, Drag::None) {
            return None;
        }
        if let Some(h) = self.insp_help(pos) {
            return Some(h);
        }
        match self.hit(pos) {
            Hit::Top(Some(item)) => Some(self.top_help(item)),
            Hit::CtlChip(chip, _) => {
                let a = self.controller_area();
                let rect = cl::chip_rect(self.axis.x0, a.y0, chip);
                let text = match chip {
                    Chip::Preset(_) => "Show this controller lane under the velocity lane",
                    Chip::Cc => "Show a CC lane: the arrows step the controller number",
                    Chip::Poly => "Show a poly aftertouch lane: the arrows step the key",
                    Chip::Curve => "Curve of new points: click to switch ramp and step",
                };
                Some(Help::new(rect, text))
            }
            _ => None,
        }
    }

    fn top_help(&self, item: TopItem) -> Help {
        let rect = self.top_rect(item);
        let help = |text: &'static str| Help::new(rect, text);
        match item {
            TopItem::Snap => {
                help("Snap grid: click to step to the next value, drag or scroll to choose")
            }
            TopItem::Quantize => help("Quantize the selected notes with the last settings")
                .with_command("editor.quantize"),
            TopItem::QuantizeOptions => help("Quantize options"),
            TopItem::Close => help("Close the piano roll and show the arrangement"),
            TopItem::Tool(t) => help(match t {
                Tool::Select => "Select tool: click to select, drag to move, resize or box select",
                Tool::Pencil => "Pencil tool: click to draw a note",
                Tool::Eraser => "Eraser tool: click a note to delete it",
                Tool::Razor => "Razor tool: click a note to split it at the snapped position",
                Tool::Speaker | Tool::Mute | Tool::Magnifier | Tool::Hand => {
                    "A tool of the arrangement"
                }
            })
            .with_command(t.command_id()),
            TopItem::ScaleRoot => help("Root note of the scale: click to choose"),
            TopItem::ScaleKind => help("Kind of scale: click to choose"),
            TopItem::ScaleDetect => help("Detect the scale from the notes of the clip"),
            TopItem::ScaleSnap => help("Snap drawn notes and moves to the scale")
                .with_command("editor.scale_snap_toggle"),
            TopItem::Fold => help("Fold the keys: hide the keys outside the scale or not used")
                .with_command("editor.fold_toggle"),
            TopItem::Chord => help("Chord stamp: the pencil drops a whole chord; click to choose"),
            TopItem::Inversion => {
                help("Chord inversion: click for the next, right-click for the previous")
            }
            TopItem::Controllers => {
                help("Show or hide the controller lanes under the velocity lane")
            }
        }
    }

    /// The playhead in content ticks (`None` while it is outside the clip's windows).
    fn playhead_content(&self) -> Option<i64> {
        self.clip
            .as_ref()
            .and_then(|c| seq_edit::song_to_content(c, self.playhead))
    }

    fn velocity_bar_at(&self, x: f64) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (i, n) in self.notes().iter().enumerate() {
            let bx = self.axis.tick_to_x(n.tick) + 1.0;
            let d = (x - bx).abs();
            let better = best.is_none_or(|(j, bd)| {
                d < bd - 1e-9
                    || (d < bd + 1e-9
                        && self.selection.contains(&i)
                        && !self.selection.contains(&j))
            });
            if d <= 5.0 && better {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    fn merge_id(&mut self) -> u64 {
        let m = self.next_merge;
        self.next_merge += 1;
        m
    }

    fn content(&self, notes: Vec<Note>, merge: Option<u64>) -> Option<UiAction> {
        Some(UiAction::Command(Command::Song(
            SongCommand::SetClipContent {
                id: self.clip_id?,
                content: ClipContent::Notes(notes.into()),
                merge,
            },
        )))
    }

    fn audition(&self, key: u8, on: bool) -> Option<UiAction> {
        Some(UiAction::Audition {
            device: self.device.clone()?,
            key,
            on,
        })
    }

    /// A request for the menu of a scale bar field, opened below it.
    fn bar_menu(&self, item: TopItem, target: ContextTarget) -> UiAction {
        let r = self.top_rect(item);
        UiAction::ContextMenu {
            target,
            at: Point::new(r.x0, r.y1),
        }
    }

    /// The key a drawn note gets: the key under the pointer, moved to the nearest scale key
    /// (ties down) with snap to scale on.
    fn drawn_key(&self, key: u8) -> u8 {
        match self.scale() {
            Some(s) if self.snap_scale && !s.is_chromatic() => s.nearest(key),
            _ => key,
        }
    }

    /// The keys the pencil drops on `key`: the stamp's chord, or `key` alone.
    fn stamp_keys(&self, key: u8) -> Vec<u8> {
        let keys = self.stamp.keys(key, self.scale(), self.inversion);
        if keys.is_empty() { vec![key] } else { keys }
    }

    /// `SetClipScale` for the open clip.
    fn scale_action(&self, scale: Option<Scale>) -> Option<UiAction> {
        Some(UiAction::Command(Command::Song(
            SongCommand::SetClipScale {
                clip: self.clip_id?,
                scale,
            },
        )))
    }

    /// Picks the scale that fits the notes best (the longer a note, the more it counts).
    fn detect_scale(&self, out: &mut Vec<UiAction>) {
        let weights: Vec<(u8, i64)> = self.notes().iter().map(|n| (n.key, n.length)).collect();
        if let Some(s) = detect_scale(&weights)
            && self.scale() != Some(s)
        {
            out.extend(self.scale_action(Some(s)));
        }
    }

    // ------------------------------------------------------------------ events

    /// Handles one event and returns what the app must do. Calls [`Self::sync`] first.
    pub fn handle(&mut self, event: &UiEvent, doc: &RackDoc, registry: &Registry) -> Vec<UiAction> {
        self.sync_current(doc, registry);
        let mut out = Vec::new();
        if let UiEvent::ScaleChanged(s) = event {
            self.set_scale(*s);
            return out;
        }
        if let UiEvent::ModifiersChanged { modifiers } = event {
            if self.vp.set_held(*modifiers) {
                self.repaint = true;
            }
            return out;
        }
        if self.clip.is_none() {
            return out;
        }
        if self.insp.visible && self.insp_event(event, &mut out) {
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
            } => {
                self.release_audition_into(&mut out);
                self.pointer_down(*pos, *button, *modifiers, *clicks, &mut out)
            }
            UiEvent::PointerUp { .. } => self.pointer_up(&mut out),
            UiEvent::PointerLeave => {
                self.hover = Hit::None;
                self.repaint = true;
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => {
                if self.hit(*pos) == Hit::Top(Some(TopItem::Snap)) {
                    self.snap = self.snap.offset(wheel_steps(delta.y));
                } else if modifiers.zooms_vertically() {
                    // Alt or Ctrl+Shift + wheel: the key zoom, the key under the pointer
                    // stays under it.
                    self.zoom_keys_to(
                        self.key_h
                            * KEY_ZOOM_STEP.powf(crate::event::zoom_wheel_amount(delta.x, delta.y)),
                        pos.y,
                    );
                } else if modifiers.command() {
                    self.axis
                        .zoom_at(ZOOM_STEP.powf(delta.y), pos.x.max(self.axis.x0));
                    self.vp.set_zoomed_to_selection(false);
                } else if modifiers.shift && delta.x.abs() < 1e-9 {
                    self.vp.manual_scroll();
                    self.axis.scroll_px(-delta.y * SCROLL_STEP);
                } else {
                    self.vp.manual_scroll();
                    self.axis.scroll_px(-delta.x * SCROLL_STEP);
                    self.scroll_y -= delta.y * SCROLL_STEP;
                    self.clamp_scroll();
                }
                self.repaint = true;
            }
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
        if !matches!(self.drag, Drag::None) {
            return;
        }
        if button == PointerButton::Middle {
            self.drag = Drag::Pan { last: pos };
            return;
        }
        let primary = button == PointerButton::Primary;
        let step = self.step(modifiers);
        let tool = self.vp.effective_tool(modifiers);
        let hit = match self.hit(pos) {
            // The other buttons and the Shift and Ctrl gestures (band, toggle) reach the grid
            // under the line.
            Hit::Playhead if !primary || modifiers.shift || modifiers.command() => Hit::Grid,
            h => h,
        };
        match hit {
            Hit::Top(Some(TopItem::Tool(t))) if primary => self.set_tool(t),
            Hit::Top(Some(TopItem::Snap)) if primary => {
                self.drag = Drag::Snap {
                    steps: StepDrag::new(pos.y),
                }
            }
            Hit::Top(Some(TopItem::Snap)) => self.snap = self.snap.cycle(false),
            // The same settings as Cmd+K: the quantize panel's last ones. A right click (or the
            // neighbouring button) opens that panel.
            Hit::Top(Some(TopItem::Quantize)) if primary => self.quantize(out),
            Hit::Top(Some(TopItem::Quantize | TopItem::QuantizeOptions)) => {
                out.push(UiAction::Quantize(QuantizeAction::Options));
            }
            Hit::Top(Some(TopItem::Close)) if primary => out.push(UiAction::ClosePianoRoll),
            Hit::Top(Some(TopItem::ScaleRoot)) if primary => {
                out.push(self.bar_menu(TopItem::ScaleRoot, ContextTarget::PianoScaleRoot));
            }
            Hit::Top(Some(TopItem::ScaleKind)) if primary => {
                out.push(self.bar_menu(TopItem::ScaleKind, ContextTarget::PianoScaleKind));
            }
            Hit::Top(Some(TopItem::Chord)) if primary => {
                out.push(self.bar_menu(TopItem::Chord, ContextTarget::PianoChord));
            }
            Hit::Top(Some(TopItem::Controllers)) if primary => {
                self.set_controllers_open(!self.ctl.open);
            }
            Hit::CtlChip(chip, part) if primary => {
                self.ctl.press(chip, part);
                self.ctl_sel = None;
            }
            Hit::CtlBody => self.ctl_down(pos, button, modifiers, tool, out),
            Hit::Top(Some(TopItem::ScaleDetect)) if primary => self.detect_scale(out),
            Hit::Top(Some(TopItem::ScaleSnap)) if primary => self.snap_scale = !self.snap_scale,
            Hit::Top(Some(TopItem::Fold)) => {
                let next = if primary {
                    cycle(&Fold::ALL, self.fold, 1)
                } else {
                    cycle(&Fold::ALL, self.fold, -1)
                };
                self.set_fold(next);
            }
            Hit::Top(Some(TopItem::Inversion)) => {
                let d: i32 = if primary { 1 } else { -1 };
                self.inversion = (i32::from(self.inversion) + d).rem_euclid(4) as u8;
            }
            Hit::Ruler if primary => {
                let c = snap_round(self.axis.x_to_tick(pos.x), step);
                let t = self.content_to_song(c).max(0);
                out.push(UiAction::Transport(TransportAction::Locate(t)));
                self.drag = Drag::Playhead { grab: 0, last: t };
            }
            Hit::Playhead => {
                let at = self.playhead_content().unwrap_or(0);
                self.drag = Drag::Playhead {
                    grab: self.axis.x_to_tick(pos.x) - at,
                    last: self.playhead,
                };
            }
            Hit::Keys(key) if primary => {
                // A double click on the key column selects every note of that pitch.
                if clicks >= 2 {
                    let same: Vec<usize> = (0..self.notes().len())
                        .filter(|&i| self.notes()[i].key == key)
                        .collect();
                    self.selection.set(same);
                }
                out.extend(self.audition(key, true));
                self.drag = Drag::Keys { key };
            }
            // Select: a right click on a note opens the notes menu on it. An unselected note
            // becomes the whole selection, a selected one keeps a multi-selection whole. The
            // other tools delete it, as the pencil does in FL.
            Hit::Note(i, _) if button == PointerButton::Secondary && tool == Tool::Select => {
                if !self.selection.contains(&i) {
                    self.selection.select_only(i);
                }
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::Notes,
                    at: pos,
                });
            }
            Hit::Note(i, _) if button == PointerButton::Secondary => self.remove_note(i, out),
            Hit::Grid if button == PointerButton::Secondary => out.push(UiAction::ContextMenu {
                target: ContextTarget::Notes,
                at: pos,
            }),
            Hit::Note(i, _) if primary && tool == Tool::Eraser => self.remove_note(i, out),
            Hit::Note(i, _) if primary && tool == Tool::Razor => {
                self.razor_split(i, pos.x, step, out);
            }
            // The pencil only draws: a click on a note neither selects nor moves it.
            Hit::Note(..) if primary && tool == Tool::Pencil => {}
            Hit::Note(i, part) if primary => self.note_down(i, part, pos, modifiers, out),
            Hit::Grid if primary && matches!(tool, Tool::Razor | Tool::Eraser) => {}
            // Select draws on a double-click only (the first press of it already cleared the
            // selection through the zero-size band below, which changes nothing else); the
            // press then drags the new note's length like the pencil.
            Hit::Grid if primary && tool == Tool::Select && clicks >= 2 => {
                self.draw_down(pos, step, out);
            }
            // A single press on the empty grid starts a rubber band (Shift or Ctrl keeps the
            // selection and adds to it, a plain press replaces it, so a click clears).
            Hit::Grid if primary && tool == Tool::Select => {
                let base = if modifiers.shift || modifiers.command() {
                    self.selection.items().clone()
                } else {
                    self.selection.clear();
                    BTreeSet::new()
                };
                self.drag = Drag::Band {
                    start: pos,
                    pos,
                    base,
                };
            }
            Hit::Grid if primary => self.draw_down(pos, step, out),
            Hit::Velocity(Some(i)) if primary => {
                let sel = if self.selection.contains(&i) {
                    self.selection.items().clone()
                } else {
                    self.selection.select_only(i);
                    [i].into_iter().collect()
                };
                let originals = self.notes().to_vec();
                let merge = self.merge_id();
                let (top, h) = self.velocity_range();
                let delta = seq_edit::velocity_from_y(pos.y, top, h) - originals[i].velocity;
                let list = seq_edit::shift_velocity(&originals, &sel, delta);
                out.extend(self.content(list.clone(), Some(merge)));
                self.drag = Drag::Velocity {
                    originals,
                    sel,
                    grabbed: i,
                    merge,
                    last: list,
                };
            }
            // The empty lane is a pencil: drag across the bars to set them (Alt: only the
            // selected notes; the constrain role (Shift): a straight line from the press).
            Hit::Velocity(None) if primary => {
                let (top, h) = self.velocity_range();
                let here = (
                    self.axis.x_to_tick(pos.x),
                    seq_edit::velocity_from_y(pos.y, top, h),
                );
                let line =
                    self.vp
                        .roles()
                        .is_active(Role::Constrain, modifiers, GestureClass::DragItem);
                let only = (self.vp.roles().is_active(
                    Role::SelectedOnly,
                    modifiers,
                    GestureClass::ClickItem,
                ) && !self.selection.is_empty())
                .then(|| self.selection.items().clone());
                let originals = self.notes().to_vec();
                self.drag = Drag::VelocityDraw {
                    last: originals.clone(),
                    originals,
                    only,
                    line,
                    from: here,
                    prev: here,
                    merge: self.merge_id(),
                };
            }
            _ => {}
        }
    }

    /// A press that draws: a note (or the stamp's chord) at the snapped tick and the row under
    /// `pos`, selected, with a drag that sets its length.
    fn draw_down(&mut self, pos: Point, step: Option<i64>, out: &mut Vec<UiAction>) {
        let tick = snap_floor(self.axis.x_to_tick(pos.x), step).max(0);
        let length = step.or(self.last_length).unwrap_or(OFF_LENGTH).max(1);
        let key = self.drawn_key(self.key_at(pos.y));
        let notes: Vec<Note> = self
            .stamp_keys(key)
            .into_iter()
            .map(|k| seq_edit::clamp_note(Note::new(tick, length, k, DEFAULT_VELOCITY)))
            .collect();
        let base = self.notes().to_vec();
        let merge = self.merge_id();
        let (list, sel) = with_notes(&base, &notes);
        self.selection.set(sel);
        out.extend(self.content(list.clone(), Some(merge)));
        for n in &notes {
            out.extend(self.audition(n.key, true));
        }
        self.drag = Drag::Draw {
            start: pos,
            base,
            notes,
            merge,
            last: list,
        };
    }

    fn remove_note(&mut self, i: usize, out: &mut Vec<UiAction>) {
        let mut notes = self.notes().to_vec();
        if i < notes.len() {
            notes.remove(i);
        }
        self.selection.clear();
        out.extend(self.content(notes, None));
    }

    /// The razor: splits note `i` at the snapped tick under `x` (nothing at its edges).
    fn razor_split(&mut self, i: usize, x: f64, step: Option<i64>, out: &mut Vec<UiAction>) {
        let Some(n) = self.notes().get(i).copied() else {
            return;
        };
        let at = snap_round(self.axis.x_to_tick(x), step);
        if at <= n.tick || at >= n.tick + n.length {
            return;
        }
        let mut list = self.notes().to_vec();
        list[i].length = at - n.tick;
        list.push(Note {
            tick: at,
            length: n.tick + n.length - at,
            ..n
        });
        let (list, _) = seq_edit::normalize_notes(list, &BTreeSet::new());
        self.selection.clear();
        out.extend(self.content(list, None));
    }

    fn note_down(
        &mut self,
        i: usize,
        part: SpanPart,
        pos: Point,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        let was = self.selection.contains(&i);
        // The copy role adds the note to the selection and a drag from it copies the selection;
        // a click without a drag takes a selected note out again (on the release).
        let copy = matches!(part, SpanPart::Body)
            && self
                .vp
                .roles()
                .is_active(Role::CopyDrag, modifiers, GestureClass::DragItem);
        let toggle = modifiers.command() && !copy;
        if copy {
            self.selection.add(i);
        } else {
            self.selection.click(i, toggle, modifiers.shift && !toggle);
        }
        if !self.selection.contains(&i) {
            return;
        }
        let originals = self.notes().to_vec();
        let sel = self.selection.items().clone();
        let merge = self.merge_id();
        let key = originals[i].key;
        match part {
            SpanPart::Start => {
                self.drag = Drag::ResizeStart {
                    start: pos,
                    last: originals.clone(),
                    originals,
                    sel,
                    grabbed: i,
                    merge,
                    moved: false,
                    emitted: false,
                };
            }
            SpanPart::End => {
                self.drag = Drag::Resize {
                    start: pos,
                    last: originals.clone(),
                    originals,
                    sel,
                    grabbed: i,
                    merge,
                    moved: false,
                    emitted: false,
                };
            }
            SpanPart::Body => {
                out.extend(self.audition(key, true));
                let reselect = (was && !toggle && !modifiers.shift && sel.len() > 1).then_some(i);
                self.drag = Drag::Move {
                    start: pos,
                    last: originals.clone(),
                    originals,
                    sel,
                    grabbed: i,
                    anchor_row: self.row_at(pos.y) as i32,
                    merge,
                    moved: false,
                    emitted: false,
                    sounding: key,
                    reselect,
                    copy,
                    toggle_off: (copy && was).then_some(i),
                };
            }
        }
    }

    /// The content tick a controller point lands on at logical `x`: on the snap grid, or with
    /// snap off a multiple of about three pixels (so a freehand stroke stays sparse).
    fn ctl_tick(&self, x: f64, modifiers: Modifiers) -> i64 {
        let raw = self.axis.x_to_tick(x);
        let step = self.step(modifiers);
        let t = match step {
            Some(_) => snap_round(raw, step),
            None => {
                let grain = (3.0 / self.axis.px_per_tick).round().max(1.0) as i64;
                (raw + grain / 2).div_euclid(grain) * grain
            }
        };
        t.max(0)
    }

    /// One `SetClipControllers` for the open clip.
    fn ctl_command(&self, lanes: Vec<ControllerLane>, merge: Option<u64>) -> Option<UiAction> {
        Some(UiAction::Command(Command::Song(
            SongCommand::SetClipControllers {
                clip: self.clip_id?,
                lanes,
                merge,
            },
        )))
    }

    /// A press in the lane body (see the module docs of `controller_lane`).
    fn ctl_down(
        &mut self,
        pos: Point,
        button: PointerButton,
        modifiers: Modifiers,
        tool: Tool,
        out: &mut Vec<UiAction>,
    ) {
        let primary = button == PointerButton::Primary;
        let kind = self.ctl.kind;
        let Some(lanes) = self.clip.as_ref().map(|c| c.controllers.clone()) else {
            return;
        };
        let (top, h) = self.ctl_range();
        let axis = self.axis;
        let hit = cl::lane_of(&lanes, kind).and_then(|l| {
            cl::nearest_point(
                &l.points,
                |p| {
                    (
                        axis.tick_to_x(p.tick),
                        cl::y_of_value(kind, p.value, top, h),
                    )
                },
                pos.x,
                pos.y,
            )
        });
        match hit {
            Some(i) if !primary || tool == Tool::Eraser => {
                self.ctl_sel = None;
                out.extend(self.ctl_command(cl::remove_point(&lanes, kind, i), None));
            }
            Some(i) if modifiers.command() => {
                self.ctl_sel = Some(i);
                out.extend(self.ctl_command(cl::toggle_curve(&lanes, kind, i), None));
            }
            Some(i) => {
                self.ctl_sel = Some(i);
                let merge = self.merge_id();
                self.drag = Drag::Ctl {
                    current: lanes.clone(),
                    original: lanes,
                    kind,
                    grabbed: Some(i),
                    start: pos,
                    merge,
                    moved: false,
                };
            }
            None if primary && matches!(tool, Tool::Select | Tool::Pencil) => {
                let tick = self.ctl_tick(pos.x, modifiers);
                let value = cl::value_at_y(kind, pos.y, top, h);
                let (next, idx) = cl::set_point(&lanes, kind, tick, value, self.ctl.new_curve());
                let merge = self.merge_id();
                out.extend(self.ctl_command(next.clone(), Some(merge)));
                self.ctl_sel = Some(idx);
                self.drag = Drag::Ctl {
                    original: lanes,
                    current: next,
                    kind,
                    grabbed: None,
                    start: pos,
                    merge,
                    moved: true,
                };
            }
            None => {}
        }
    }

    /// A pointer move during a controller drag: the grabbed point follows (time on the grid,
    /// value at the MIDI resolution) or the stroke adds a point per grid step.
    fn ctl_move(&mut self, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        let tick = self.ctl_tick(pos.x, modifiers);
        let (top, h) = self.ctl_range();
        let curve = self.ctl.new_curve();
        let id = self.clip_id;
        let Drag::Ctl {
            original,
            current,
            kind,
            grabbed,
            start,
            merge,
            moved,
        } = &mut self.drag
        else {
            return;
        };
        let value = cl::value_at_y(*kind, pos.y, top, h);
        let (next, idx) = match grabbed {
            Some(g) => {
                *moved |= pos.distance(*start) > DRAG_THRESHOLD;
                if !*moved {
                    return;
                }
                cl::move_point(original, *kind, *g, tick, value)
            }
            None => cl::set_point(current, *kind, tick, value, curve),
        };
        let mut sel = None;
        if next != *current {
            if let Some(id) = id {
                out.push(UiAction::Command(Command::Song(
                    SongCommand::SetClipControllers {
                        clip: id,
                        lanes: next.clone(),
                        merge: Some(*merge),
                    },
                )));
            }
            *current = next;
            sel = Some(idx);
        }
        if sel.is_some() {
            self.ctl_sel = sel;
        }
        self.repaint = true;
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
        if matches!(self.drag, Drag::Ctl { .. }) {
            self.ctl_move(pos, modifiers, out);
            return;
        }
        if let Drag::Snap { steps } = &mut self.drag {
            let n = steps.update(pos.y, PX_PER_STEP);
            if n != 0 {
                self.snap = self.snap.offset(n);
            }
            self.repaint = true;
            return;
        }
        if let Drag::Playhead { grab, last } = self.drag {
            let c = snap_round(self.axis.x_to_tick(pos.x) - grab, self.step(modifiers));
            let t = self.content_to_song(c).max(0);
            if t != last {
                self.drag = Drag::Playhead { grab, last: t };
                out.push(UiAction::Transport(TransportAction::Locate(t)));
            }
            return;
        }
        let step = self.step(modifiers);
        let min = seq_edit::min_length(step, MIN_TICKS);
        let axis = self.axis;
        let key_here = self.key_at(pos.y);
        let row_here = self.row_at(pos.y) as i32;
        let scale = self.scale();
        let snap_scale = self.snap_scale;
        let snap_mode = self.vp.snap_mode();
        let constrain =
            self.vp
                .roles()
                .is_active(Role::Constrain, modifiers, GestureClass::DragItem);
        let rows = &self.rows;
        let (vtop, vh) = self.velocity_range();
        let rects: Vec<Rect> = if matches!(self.drag, Drag::Band { .. }) {
            self.notes().iter().map(|n| self.rect_of(n)).collect()
        } else {
            Vec::new()
        };
        let device = self.device.clone();
        let id = self.clip_id;
        let content = |notes: Vec<Note>, merge: u64| {
            id.map(|id| {
                UiAction::Command(Command::Song(SongCommand::SetClipContent {
                    id,
                    content: ClipContent::Notes(notes.into()),
                    merge: Some(merge),
                }))
            })
        };
        let audition = |key: u8, on: bool| {
            device
                .clone()
                .map(|device| UiAction::Audition { device, key, on })
        };
        self.repaint = true;
        let mut new_sel: Option<BTreeSet<usize>> = None;
        match &mut self.drag {
            Drag::None | Drag::Ctl { .. } | Drag::Playhead { .. } | Drag::Snap { .. } => {}
            Drag::Pan { last } => {
                let d = *last - pos;
                *last = pos;
                self.axis.scroll_px(d.x);
                self.scroll_y += d.y;
                self.clamp_scroll();
                self.vp.manual_scroll();
            }
            Drag::Keys { key } => {
                if key_here != *key {
                    out.extend(audition(*key, false));
                    out.extend(audition(key_here, true));
                    *key = key_here;
                }
            }
            Drag::Band {
                start,
                pos: p,
                base,
            } => {
                *p = pos;
                let mut s = base.clone();
                s.extend(seq_edit::rubber_band(*start, pos, &rects));
                new_sel = Some(s);
            }
            Drag::Draw {
                start,
                base,
                notes,
                merge,
                last,
            } => {
                let first = notes[0];
                // A press without a drag keeps the default length; a drag ends the note at
                // the (snapped) pointer, at least the minimum long.
                let len = if pos.distance(*start) > DRAG_THRESHOLD {
                    (snap_round(axis.x_to_tick(pos.x), step) - first.tick).max(min)
                } else {
                    first.length
                };
                let drawn: Vec<Note> = notes.iter().map(|n| Note { length: len, ..*n }).collect();
                let (list, sel) = with_notes(base, &drawn);
                if list != *last {
                    out.extend(content(list.clone(), *merge));
                    *last = list;
                    new_sel = Some(sel);
                }
                self.last_length = Some(len);
            }
            Drag::Move {
                start,
                originals,
                sel,
                grabbed,
                anchor_row,
                merge,
                moved,
                emitted,
                last,
                sounding,
                copy,
                ..
            } => {
                *moved |= pos.distance(*start) > DRAG_THRESHOLD;
                if !*moved {
                    return;
                }
                let g = originals[*grabbed];
                let delta = axis.dx_to_ticks(pos.x - start.x);
                // The constrain role keeps the move on one axis, the one the pointer went furthest.
                let (dx, dy) = ((pos.x - start.x).abs(), (pos.y - start.y).abs());
                let (along_time, along_pitch) = if constrain {
                    (dx >= dy, dx < dy)
                } else {
                    (true, true)
                };
                let dt = if along_time {
                    seq_edit::drag_start_by(snap_mode, g.tick, delta, step) - g.tick
                } else {
                    0
                };
                let timed = seq_edit::move_notes(originals, sel, dt, 0);
                let moved_notes = vertical_move(
                    rows,
                    scale,
                    snap_scale,
                    &timed,
                    sel,
                    *grabbed,
                    if along_pitch {
                        *anchor_row - row_here
                    } else {
                        0
                    },
                );
                let now_key = moved_notes[*grabbed].key;
                let (list, new) = if *copy {
                    // The notes stay and their moved copies join them.
                    let copies: Vec<Note> = sel.iter().map(|i| moved_notes[*i]).collect();
                    with_notes(originals, &copies)
                } else {
                    seq_edit::normalize_notes(moved_notes, sel)
                };
                if now_key != *sounding {
                    out.extend(audition(*sounding, false));
                    out.extend(audition(now_key, true));
                    *sounding = now_key;
                }
                if list != *last {
                    out.extend(content(list.clone(), *merge));
                    *last = list;
                    *emitted = true;
                    new_sel = Some(new);
                }
            }
            Drag::Resize {
                start,
                originals,
                sel,
                grabbed,
                merge,
                moved,
                emitted,
                last,
            } => {
                *moved |= pos.distance(*start) > DRAG_THRESHOLD;
                if !*moved {
                    return;
                }
                let g = originals[*grabbed];
                let delta = axis.dx_to_ticks(pos.x - start.x);
                let len = seq_edit::drag_length(g.tick, g.tick + g.length, delta, step, min);
                let resized = seq_edit::resize_notes(originals, sel, len - g.length, min);
                let (list, new) = seq_edit::normalize_notes(resized, sel);
                if list != *last {
                    out.extend(content(list.clone(), *merge));
                    *last = list;
                    *emitted = true;
                    new_sel = Some(new);
                }
                self.last_length = Some(len);
            }
            Drag::ResizeStart {
                start,
                originals,
                sel,
                grabbed,
                merge,
                moved,
                emitted,
                last,
            } => {
                *moved |= pos.distance(*start) > DRAG_THRESHOLD;
                if !*moved {
                    return;
                }
                let g = originals[*grabbed];
                let delta = axis.dx_to_ticks(pos.x - start.x);
                let dt = seq_edit::drag_start(g.tick, delta, step) - g.tick;
                let resized = seq_edit::resize_notes_start(originals, sel, dt, min);
                let (list, new) = seq_edit::normalize_notes(resized, sel);
                if list != *last {
                    out.extend(content(list.clone(), *merge));
                    *last = list;
                    *emitted = true;
                    new_sel = Some(new);
                }
            }
            Drag::Velocity {
                originals,
                sel,
                grabbed,
                merge,
                last,
            } => {
                let delta =
                    seq_edit::velocity_from_y(pos.y, vtop, vh) - originals[*grabbed].velocity;
                let list = seq_edit::shift_velocity(originals, sel, delta);
                if list != *last {
                    out.extend(content(list.clone(), *merge));
                    *last = list;
                }
            }
            Drag::VelocityDraw {
                originals,
                only,
                line,
                from,
                prev,
                merge,
                last,
            } => {
                let here = (
                    axis.x_to_tick(pos.x),
                    seq_edit::velocity_from_y(pos.y, vtop, vh),
                );
                let list = if *line {
                    seq_edit::velocity_stroke(originals, only.as_ref(), *from, here)
                } else {
                    let l = seq_edit::velocity_stroke(last, only.as_ref(), *prev, here);
                    *prev = here;
                    l
                };
                if list != *last {
                    out.extend(content(list.clone(), *merge));
                    *last = list;
                }
            }
        }
        if let Some(s) = new_sel {
            self.selection.set(s);
        }
    }

    fn pointer_up(&mut self, out: &mut Vec<UiAction>) {
        self.repaint = true;
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None | Drag::Pan { .. } | Drag::Band { .. } | Drag::Playhead { .. } => {}
            Drag::Snap { steps } => {
                if !steps.moved() {
                    self.snap = self.snap.cycle(true);
                }
            }
            Drag::Keys { key } => out.extend(self.audition(key, false)),
            Drag::Draw { notes, .. } => {
                for n in &notes {
                    out.extend(self.audition(n.key, false));
                }
                out.push(UiAction::EndMerge);
            }
            Drag::Move {
                sounding,
                emitted,
                moved,
                reselect,
                toggle_off,
                ..
            } => {
                out.extend(self.audition(sounding, false));
                if emitted {
                    out.push(UiAction::EndMerge);
                }
                if !moved && let Some(i) = reselect {
                    self.selection.select_only(i);
                }
                if !moved && let Some(i) = toggle_off {
                    self.selection.toggle(i);
                }
            }
            Drag::Resize { emitted, .. } | Drag::ResizeStart { emitted, .. } => {
                if emitted {
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Velocity { .. } | Drag::VelocityDraw { .. } => out.push(UiAction::EndMerge),
            Drag::Ctl { moved: true, .. } => out.push(UiAction::EndMerge),
            Drag::Ctl { .. } => {}
        }
    }

    /// Asks the app to quantize with the quantize panel's last settings (the grid, strength,
    /// swing, groove and humanize it holds, not the roll's snap): the app owns them and
    /// applies them to the selected notes, all the notes of the clip when none is selected.
    fn quantize(&self, out: &mut Vec<UiAction>) {
        if !self.notes().is_empty() {
            out.push(UiAction::Quantize(QuantizeAction::ApplyLast));
        }
    }

    /// The keys that are not commands: Escape (cancels a drag, clears the selection, closes the
    /// roll). Everything else a key can do is a registry command ([`Self::command`]), so a
    /// remap or a context really applies.
    fn key(&mut self, key: &Key, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        self.repaint = true;
        if modifiers.command() {
            return;
        }
        if matches!(key, Key::Escape) {
            if !matches!(self.drag, Drag::None) {
                self.cancel_drag(out);
            } else if !self.selection.is_empty() {
                self.selection.clear();
            } else {
                out.push(UiAction::ClosePianoRoll);
            }
        }
    }

    // ------------------------------------------------------------------ commands

    /// Runs a registry command the shell offers to this view: the note edit
    /// commands (select all, delete, quantize, transpose, nudge), snap, zoom, follow and the
    /// tool palette. A command that is not the roll's, or has nothing to work on (no
    /// selection), is [`CommandOutcome::Declined`] so the next binding runs.
    pub fn command(
        &mut self,
        id: &str,
        arg: Option<&str>,
        doc: &RackDoc,
        registry: &Registry,
    ) -> CommandOutcome {
        self.sync_current(doc, registry);
        if self.clip.is_none() {
            return CommandOutcome::Declined;
        }
        let outcome = self.run_command(id, arg);
        if outcome.is_taken() {
            self.repaint = true;
        }
        outcome
    }

    fn run_command(&mut self, id: &str, arg: Option<&str>) -> CommandOutcome {
        if let Some(tool) = Tool::from_command(id).filter(|t| Tool::NOTE.contains(t)) {
            self.set_tool(tool);
            return CommandOutcome::Handled(Vec::new());
        }
        let step = self.step(Modifiers::NONE).unwrap_or(OFF_LENGTH);
        match id {
            "edit.select_all" => {
                self.selection.set(0..self.notes().len());
                CommandOutcome::Handled(Vec::new())
            }
            "edit.duplicate" => {
                let (Some(clip), false) = (self.clip_id, self.selection.is_empty()) else {
                    return CommandOutcome::Declined;
                };
                let indices: Vec<usize> = self.selection.items().iter().copied().collect();
                CommandOutcome::Handled(vec![UiAction::Command(Command::Workflow(
                    WorkflowCommand::DuplicateNotes { clip, indices },
                ))])
            }
            "edit.delete" => {
                if self.selection.is_empty() {
                    return CommandOutcome::Declined;
                }
                let sel = self.selection.items().clone();
                let notes: Vec<Note> = self
                    .notes()
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !sel.contains(i))
                    .map(|(_, n)| *n)
                    .collect();
                self.selection.clear();
                CommandOutcome::Handled(self.content(notes, None).into_iter().collect())
            }
            "editor.quantize" => {
                let mut out = Vec::new();
                self.quantize(&mut out);
                CommandOutcome::Handled(out)
            }
            "editor.toggle_note_mute" => {
                let sel = self.selection.items().clone();
                if sel.is_empty() {
                    return CommandOutcome::Handled(Vec::new());
                }
                let notes: Vec<Note> = self
                    .notes()
                    .iter()
                    .enumerate()
                    .map(|(i, n)| {
                        if sel.contains(&i) {
                            Note {
                                muted: !n.muted,
                                ..*n
                            }
                        } else {
                            *n
                        }
                    })
                    .collect();
                CommandOutcome::Handled(self.content(notes, None).into_iter().collect())
            }
            "editor.legato" => {
                let Some(overlap) = crate::note_tool_args::legato_overlap(arg) else {
                    return CommandOutcome::Declined;
                };
                let sel = self.selection_or_all();
                self.apply_edit(Some(seq_edit::make_legato(self.notes(), &sel, overlap)))
            }
            "editor.join_notes" => {
                if self.selection.is_empty() {
                    return CommandOutcome::Declined;
                }
                let sel = self.selection.items().clone();
                self.apply_edit(Some(seq_edit::join_notes(self.notes(), &sel)))
            }
            "editor.chop" => {
                let Some(grid) = crate::note_tool_args::chop_grid(arg, step) else {
                    return CommandOutcome::Declined;
                };
                let sel = self.selection_or_all();
                self.apply_edit(Some(seq_edit::chop_notes(self.notes(), &sel, grid)))
            }
            "editor.select_same_pitch" => {
                let keys: BTreeSet<u8> = self
                    .selection
                    .items()
                    .iter()
                    .filter_map(|&i| self.notes().get(i))
                    .map(|n| n.key)
                    .collect();
                if keys.is_empty() {
                    return CommandOutcome::Declined;
                }
                let same = (0..self.notes().len()).filter(|&i| keys.contains(&self.notes()[i].key));
                let same: Vec<usize> = same.collect();
                self.selection.set(same);
                CommandOutcome::Handled(Vec::new())
            }
            "editor.note_tool" => self.note_tool(arg),
            "editor.transpose_up" => self.transpose(1),
            "editor.transpose_down" => self.transpose(-1),
            "editor.octave_up" => self.move_selection(0, 12),
            "editor.octave_down" => self.move_selection(0, -12),
            "editor.select_next_note" => self.step_selection(true, false),
            "editor.select_prev_note" => self.step_selection(false, false),
            "editor.extend_next_note" => self.step_selection(true, true),
            "editor.extend_prev_note" => self.step_selection(false, true),
            "editor.nudge_left" => self.move_selection(-step, 0),
            "editor.nudge_right" => self.move_selection(step, 0),
            // One beat of the meter (the Beat snap step) or one tick, whatever the snap.
            "editor.nudge_left_beat" => self.move_selection(-self.meter.beat_ticks(), 0),
            "editor.nudge_right_beat" => self.move_selection(self.meter.beat_ticks(), 0),
            "editor.nudge_left_tick" => self.move_selection(-1, 0),
            "editor.nudge_right_tick" => self.move_selection(1, 0),
            "editor.scale_snap_toggle" => {
                self.snap_scale = !self.snap_scale;
                CommandOutcome::Handled(Vec::new())
            }
            "editor.fold_toggle" => {
                let next = match (self.fold, self.scale()) {
                    (Fold::Off, Some(s)) if !s.is_chromatic() => Fold::Scale,
                    (Fold::Off, _) => Fold::Used,
                    _ => Fold::Off,
                };
                self.set_fold(next);
                CommandOutcome::Handled(Vec::new())
            }
            "editor.fold_set" => match arg.and_then(Fold::from_id) {
                Some(fold) => {
                    self.set_fold(fold);
                    CommandOutcome::Handled(Vec::new())
                }
                None => CommandOutcome::Declined,
            },
            "editor.set_scale_root" => {
                let Some(root) = arg.and_then(|a| ROOT_ARGS.iter().position(|r| *r == a)) else {
                    return CommandOutcome::Declined;
                };
                let root = root as u8;
                match self.scale() {
                    Some(s) => CommandOutcome::Handled(
                        self.scale_action(Some(Scale::new(root, s.kind)))
                            .into_iter()
                            .collect(),
                    ),
                    None => {
                        // Nothing to move: the root is the one the next scale gets.
                        self.root_pref = root;
                        CommandOutcome::Handled(Vec::new())
                    }
                }
            }
            "editor.set_scale" => {
                let Some(kind) = arg.and_then(ScaleKind::from_id) else {
                    return CommandOutcome::Declined;
                };
                let root = self.scale().map_or(self.root_pref, |s| s.root);
                let scale = (kind != ScaleKind::Chromatic).then(|| Scale::new(root, kind));
                CommandOutcome::Handled(self.scale_action(scale).into_iter().collect())
            }
            "editor.detect_scale" => {
                let mut out = Vec::new();
                self.detect_scale(&mut out);
                CommandOutcome::Handled(out)
            }
            "editor.conform_to_scale" => {
                if self.selection.is_empty() {
                    return CommandOutcome::Declined;
                }
                let Some(scale) = self.scale() else {
                    return CommandOutcome::Handled(Vec::new());
                };
                let sel = self.selection.items().clone();
                let moved = chord_edit::conform(self.notes(), &sel, scale);
                let result = chord_edit::unique(moved, &sel);
                self.apply_edit(Some(result))
            }
            "editor.set_chord" => match arg.and_then(Stamp::from_id) {
                Some(stamp) => {
                    self.stamp = stamp;
                    CommandOutcome::Handled(Vec::new())
                }
                None => CommandOutcome::Declined,
            },
            "editor.set_chord_inversion" => match arg.and_then(|a| a.parse::<u8>().ok()) {
                Some(i) if i <= 3 => {
                    self.inversion = i;
                    CommandOutcome::Handled(Vec::new())
                }
                _ => CommandOutcome::Declined,
            },
            "editor.chordify" => {
                if self.selection.is_empty() {
                    return CommandOutcome::Declined;
                }
                let sel = self.selection.items().clone();
                let result = chord_edit::chordify(
                    self.notes(),
                    &sel,
                    self.stamp,
                    self.inversion,
                    self.scale(),
                );
                self.apply_edit(result)
            }
            "editor.arpeggiate" => {
                if self.selection.is_empty() {
                    return CommandOutcome::Declined;
                }
                let sel = self.selection.items().clone();
                let result = chord_edit::arpeggiate(self.notes(), &sel, self.arp, step);
                self.apply_edit(result)
            }
            "editor.strum" => {
                if self.selection.is_empty() {
                    return CommandOutcome::Declined;
                }
                let sel = self.selection.items().clone();
                let result = chord_edit::strum(self.notes(), &sel, self.strum);
                self.apply_edit(result)
            }
            "editor.set_arp_pattern" => match arg.and_then(ArpPattern::from_id) {
                Some(pattern) => {
                    self.arp.pattern = pattern;
                    CommandOutcome::Handled(Vec::new())
                }
                None => CommandOutcome::Declined,
            },
            "editor.set_arp_gate" => match arg.and_then(|a| a.parse::<u8>().ok()) {
                Some(g) if ARP_GATES.contains(&g) => {
                    self.arp.gate_percent = g;
                    CommandOutcome::Handled(Vec::new())
                }
                _ => CommandOutcome::Declined,
            },
            "editor.set_arp_octaves" => match arg.and_then(|a| a.parse::<u8>().ok()) {
                Some(o) if ARP_OCTAVES.contains(&o) => {
                    self.arp.octaves = o;
                    CommandOutcome::Handled(Vec::new())
                }
                _ => CommandOutcome::Declined,
            },
            "editor.set_strum_amount" => match arg.and_then(|a| a.parse::<u16>().ok()) {
                Some(d) if STRUM_AMOUNTS.contains(&d) => {
                    self.strum.ticks = strum_ticks(d);
                    CommandOutcome::Handled(Vec::new())
                }
                _ => CommandOutcome::Declined,
            },
            "editor.set_strum_direction" => match arg.and_then(StrumDirection::from_id) {
                Some(direction) => {
                    self.strum.direction = direction;
                    CommandOutcome::Handled(Vec::new())
                }
                None => CommandOutcome::Declined,
            },
            "sequencer.snap_toggle" => {
                self.snap = self.vp.toggle_snap(self.snap);
                CommandOutcome::Handled(Vec::new())
            }
            "sequencer.set_snap" => match arg.and_then(snap_from_arg) {
                Some(snap) => {
                    self.snap = snap;
                    CommandOutcome::Handled(Vec::new())
                }
                None => CommandOutcome::Declined,
            },
            "sequencer.set_snap_mode" => match arg.and_then(crate::viewport::SnapMode::from_arg) {
                Some(mode) => {
                    self.vp.set_snap_mode(mode);
                    CommandOutcome::Handled(Vec::new())
                }
                None => CommandOutcome::Declined,
            },
            "sequencer.follow" => {
                self.vp.toggle_follow();
                CommandOutcome::Handled(Vec::new())
            }
            "view.zoom_in" | "view.zoom_out" => {
                let factor = if id == "view.zoom_in" {
                    ZOOM_KEY
                } else {
                    1.0 / ZOOM_KEY
                };
                let anchor = (self.axis.x0 + self.bounds.x1) / 2.0;
                self.axis.zoom_at(factor, anchor);
                self.vp.set_zoomed_to_selection(false);
                CommandOutcome::Handled(Vec::new())
            }
            "editor.zoom_keys_in" | "editor.zoom_keys_out" => {
                let factor = if id == "editor.zoom_keys_in" {
                    KEY_ZOOM_STEP
                } else {
                    1.0 / KEY_ZOOM_STEP
                };
                self.set_key_height(self.key_h * factor);
                CommandOutcome::Handled(Vec::new())
            }
            "editor.zoom_keys_reset" => {
                self.set_key_height(KEY_HEIGHT);
                CommandOutcome::Handled(Vec::new())
            }
            "sequencer.zoom_selection" => {
                self.zoom_selection();
                CommandOutcome::Handled(Vec::new())
            }
            _ => CommandOutcome::Declined,
        }
    }

    /// Z: zoom to the selected notes, or (again, or with no selection) to the whole clip.
    fn zoom_selection(&mut self) {
        let sel: Vec<Note> = self
            .selection
            .items()
            .iter()
            .filter_map(|i| self.notes().get(*i).copied())
            .collect();
        let span = sel
            .iter()
            .map(|n| n.tick)
            .min()
            .zip(sel.iter().map(|n| n.tick + n.length).max());
        match span {
            Some((s, e)) if !self.vp.zoomed_to_selection() => {
                self.axis.fit_span(s, e, self.bounds.x1);
                self.vp.set_zoomed_to_selection(true);
            }
            _ => {
                self.fit();
                self.vp.set_zoomed_to_selection(false);
            }
        }
    }

    /// Moves the selected notes by `dt` ticks and `dkey` keys (Declined without a selection).
    fn move_selection(&mut self, dt: i64, dkey: i32) -> CommandOutcome {
        if self.selection.is_empty() {
            return CommandOutcome::Declined;
        }
        let sel = self.selection.items().clone();
        let moved = seq_edit::move_notes(self.notes(), &sel, dt, dkey);
        let (list, new) = seq_edit::normalize_notes(moved, &sel);
        self.apply_edit(Some((list, new)))
    }

    /// Selects the next (`forward`) or previous note in time order (ties by pitch, low to
    /// high) and plays it. With `extend` the selection grows or shrinks like a text selection:
    /// it has an anchor (where stepping started, or the earliest note of a selection made
    /// otherwise) and an active end (the one stepping moves, the latest note for a selection
    /// made otherwise). A step away from the anchor adds the next note; a step toward it drops
    /// the active note, and past the anchor the selection grows on the other side. With nothing
    /// selected the next note is the first at or after the playhead (the first one if there is
    /// none) and the previous the last before it. At either end nothing changes (no wrap). The
    /// view scrolls to keep the note in sight; a shrink plays the new active end. Declined when
    /// the clip has no notes.
    fn step_selection(&mut self, forward: bool, extend: bool) -> CommandOutcome {
        let notes = self.notes();
        if notes.is_empty() {
            return CommandOutcome::Declined;
        }
        let mut order: Vec<usize> = (0..notes.len()).collect();
        order.sort_by_key(|&i| (notes[i].tick, notes[i].key, i));
        let pos_of = |i: usize| order.iter().position(|&o| o == i);
        let sel: BTreeSet<usize> = self
            .selection
            .items()
            .iter()
            .filter_map(|&i| pos_of(i))
            .collect();
        let (lo, hi) = (sel.first().copied(), sel.last().copied());
        let mut out = Vec::new();
        let Some((lo, hi)) = lo.zip(hi) else {
            // The playhead outside the clip counts as before every note for the
            // next one and after every note for the previous one.
            let at = self.playhead_content();
            let last = order.len() - 1;
            let p = if forward {
                match at {
                    Some(at) => order.iter().position(|&i| notes[i].tick >= at),
                    None => Some(0),
                }
                .unwrap_or(last)
            } else {
                match at {
                    Some(at) => order.iter().rposition(|&i| notes[i].tick < at),
                    None => Some(last),
                }
                .unwrap_or(0)
            };
            self.step_to(order[p], order[p], Some(order[p]), &mut out);
            return CommandOutcome::Handled(out);
        };
        if !extend {
            let p = if forward {
                (hi + 1 < order.len()).then_some(hi + 1)
            } else {
                lo.checked_sub(1)
            };
            if let Some(p) = p {
                self.step_to(order[p], order[p], Some(order[p]), &mut out);
            }
            return CommandOutcome::Handled(out);
        }
        // The anchor and active end of the selection as positions: the remembered ones while
        // the selection still has their shape, the earliest and latest note otherwise.
        let kept = self.step_sel.and_then(|(a, h)| pos_of(a).zip(pos_of(h)));
        let (anchor, active) = match kept {
            Some((a, h)) if sel.contains(&a) && (if h >= a { h == hi } else { h == lo }) => (a, h),
            _ => (lo, hi),
        };
        // The active end is right of the anchor (or on it, when a step to the right extends).
        let right = active > anchor || (active == anchor && forward);
        let (new_sel, new_active) = if forward == right {
            // Away from the anchor: the neighbour joins.
            let next = if forward {
                (active + 1 < order.len()).then_some(active + 1)
            } else {
                active.checked_sub(1)
            };
            let Some(next) = next else {
                return CommandOutcome::Handled(out);
            };
            let mut s = sel.clone();
            s.insert(next);
            (s, next)
        } else {
            // Toward the anchor: the active note leaves, the next selected one is the end.
            let mut s = sel.clone();
            s.remove(&active);
            let end = if right {
                s.range(..active).next_back().copied()
            } else {
                s.range(active + 1..).next().copied()
            };
            (s, end.unwrap_or(anchor))
        };
        let anchor_note = order[anchor];
        self.selection.set(new_sel.iter().map(|&p| order[p]));
        self.step_to(anchor_note, order[new_active], None, &mut out);
        CommandOutcome::Handled(out)
    }

    /// Finishes a step: remembers the anchor and active end, scrolls the active note into
    /// view, selects it alone when `only` and plays it (the sounding step note goes first).
    fn step_to(
        &mut self,
        anchor: usize,
        active: usize,
        only: Option<usize>,
        out: &mut Vec<UiAction>,
    ) {
        if let Some(i) = only {
            self.selection.select_only(i);
        }
        self.step_sel = Some((anchor, active));
        self.reveal_note(active);
        self.release_audition_into(out);
        let (Some(device), Some(n)) = (self.device.clone(), self.notes().get(active).copied())
        else {
            return;
        };
        out.push(UiAction::Audition {
            device: device.clone(),
            key: n.key,
            on: true,
        });
        self.step_note = Some(StepNote {
            device,
            key: n.key,
            off_at: None,
        });
    }

    /// Releases the note a step is sounding, at once. The host calls it before the roll closes
    /// or its clip changes, so no note hangs.
    pub fn release_audition(&mut self) -> Vec<UiAction> {
        let mut out = Vec::new();
        self.release_audition_into(&mut out);
        out
    }

    fn release_audition_into(&mut self, out: &mut Vec<UiAction>) {
        if let Some(s) = self.step_note.take() {
            out.push(UiAction::Audition {
                device: s.device,
                key: s.key,
                on: false,
            });
        }
    }

    /// Per-frame clock (`now` in seconds, any monotonic clock): releases the note a step is
    /// sounding once it has sounded for [`STEP_AUDITION_SECS`].
    pub fn tick(&mut self, now: f64) -> Vec<UiAction> {
        let Some(s) = &mut self.step_note else {
            return Vec::new();
        };
        let due = *s.off_at.get_or_insert(now + STEP_AUDITION_SECS);
        if now >= due {
            return self.release_audition();
        }
        Vec::new()
    }

    /// Scrolls the view (time and keys) so note `i` is inside the grid.
    fn reveal_note(&mut self, i: usize) {
        let Some(n) = self.notes().get(i).copied() else {
            return;
        };
        self.vp.manual_scroll();
        let width = self.axis.visible_ticks(self.bounds.x1);
        let (a, z) = (self.axis.scroll, self.axis.scroll + width);
        if (n.tick as f64) < a || (n.tick as f64) > z {
            self.axis.page_to(n.tick, self.bounds.x1);
        }
        let g = self.grid_rect();
        let top = self.key_top(n.key);
        if top < g.y0 {
            self.scroll_y -= g.y0 - top;
        } else if top + self.key_h > g.y1 {
            self.scroll_y += top + self.key_h - g.y1;
        }
        self.clamp_scroll();
    }

    /// Transposes the selection by one semitone, or by one scale degree when snap to scale is
    /// on and the clip has a scale (Declined without a selection).
    fn transpose(&mut self, by: i32) -> CommandOutcome {
        match self.scale() {
            Some(s) if self.snap_scale && !s.is_chromatic() => {
                if self.selection.is_empty() {
                    return CommandOutcome::Declined;
                }
                let sel = self.selection.items().clone();
                let moved = chord_edit::move_degrees(self.notes(), &sel, by, s);
                let (list, new) = seq_edit::normalize_notes(moved, &sel);
                self.apply_edit(Some((list, new)))
            }
            _ => self.move_selection(0, by),
        }
    }

    /// The selected notes, or every note of the clip when none is selected (the way a quantize
    /// treats an empty selection).
    fn selection_or_all(&self) -> BTreeSet<usize> {
        if self.selection.is_empty() {
            (0..self.notes().len()).collect()
        } else {
            self.selection.items().clone()
        }
    }

    /// Runs a note tool of the Notes menu on the selected notes (every note when none is
    /// selected) as one `ApplyNoteTool`, keeping the selection on the same notes. Nothing is
    /// sent when the tool would change no note.
    fn note_tool(&mut self, arg: Option<&str>) -> CommandOutcome {
        let (Some(clip), Some(arg)) = (self.clip_id, arg) else {
            return CommandOutcome::Declined;
        };
        let Some(tool) = crate::note_tool_args::parse_note_tool(arg, self.tool_seed) else {
            return CommandOutcome::Declined;
        };
        let picked = (!self.selection.is_empty()).then(|| self.selection.items().clone());
        let (out, index) = atomik_model::note_tools::apply_note_tool_indexed(
            self.notes(),
            picked.as_ref(),
            clip.0,
            tool,
        );
        if out == self.notes() {
            return CommandOutcome::Handled(Vec::new());
        }
        self.tool_seed = self.tool_seed.wrapping_add(1);
        if let Some(picked) = &picked {
            self.selection.set(picked.iter().map(|&i| index[i]));
        }
        CommandOutcome::Handled(vec![UiAction::Command(Command::Song(
            SongCommand::ApplyNoteTool {
                clips: vec![clip],
                selection: picked.map(|s| s.into_iter().collect()),
                tool,
            },
        ))])
    }

    /// Sends a new note list (one `SetClipContent`, so one undo step) and selects `new`;
    /// nothing is sent when the list is the clip's own.
    fn apply_edit(&mut self, result: Option<(Vec<Note>, BTreeSet<usize>)>) -> CommandOutcome {
        let mut out = Vec::new();
        if let Some((list, new)) = result
            && list != self.notes()
        {
            self.selection.set(new);
            out.extend(self.content(list, None));
        }
        CommandOutcome::Handled(out)
    }

    fn cancel_drag(&mut self, out: &mut Vec<UiAction>) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Keys { key } => out.extend(self.audition(key, false)),
            Drag::Draw {
                base, notes, merge, ..
            } => {
                for n in &notes {
                    out.extend(self.audition(n.key, false));
                }
                out.extend(self.content(base, Some(merge)));
                out.push(UiAction::EndMerge);
                self.selection.clear();
            }
            Drag::Move {
                originals,
                sel,
                merge,
                emitted,
                sounding,
                ..
            } => {
                out.extend(self.audition(sounding, false));
                if emitted {
                    out.extend(self.content(originals, Some(merge)));
                    out.push(UiAction::EndMerge);
                    self.selection.set(sel);
                }
            }
            Drag::Resize {
                originals,
                sel,
                merge,
                emitted: true,
                ..
            }
            | Drag::ResizeStart {
                originals,
                sel,
                merge,
                emitted: true,
                ..
            } => {
                out.extend(self.content(originals, Some(merge)));
                out.push(UiAction::EndMerge);
                self.selection.set(sel);
            }
            Drag::Velocity {
                originals, merge, ..
            }
            | Drag::VelocityDraw {
                originals, merge, ..
            } => {
                out.extend(self.content(originals, Some(merge)));
                out.push(UiAction::EndMerge);
            }
            Drag::Ctl {
                original,
                merge,
                moved: true,
                ..
            } => {
                out.extend(self.ctl_command(original, Some(merge)));
                out.push(UiAction::EndMerge);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ painting

    /// Paints the roll into its bounds (physical pixels, scaled like the other views).
    pub fn paint(&mut self, canvas: &mut dyn Canvas, doc: &RackDoc, registry: &Registry) {
        self.sync_current(doc, registry);
        let b = self.bounds;
        let c = canvas;
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&self.outer.into());
        sp::rect(c, self.outer, self.st.background);
        if self.clip.is_some() {
            self.paint_grid(c);
            self.paint_ruler(c);
            self.paint_keyboard(c);
            self.paint_velocity(c);
            if self.ctl.open {
                self.paint_controllers(c);
            }
        } else {
            let t = &self.st.base;
            sp::label(
                c,
                "No clip open",
                b.center().x,
                b.center().y,
                11.0,
                t.text_muted,
                Align::Center,
            );
        }
        self.paint_top_bar(c);
        if self.clip.is_some() {
            self.paint_scale_bar(c);
            self.paint_inspector(c);
        }
        if self.focused {
            sp::line(c, b.x0, b.y0 + 0.5, b.x1, b.y0 + 0.5, self.st.base.focus);
        }
        c.pop_clip();
        c.pop_transform();
        self.repaint = false;
    }

    fn paint_grid(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let g = self.grid_rect();
        c.push_clip(&g.into());
        sp::rect(c, g, st.lane);
        let scale = self.scale().filter(|s| !s.is_chromatic());
        for r in self.visible_rows(g.y0, g.y1) {
            let k = self.rows.keys()[r];
            let y = self.row_top(r);
            let row = Rect::new(g.x0, y, g.x1, y + self.key_h);
            match scale {
                // In the scale the row is tinted (the root more), out of it the row is dark.
                Some(s) if s.is_root(k) => {
                    sp::rect(c, row, mix(st.lane, self.accent, 0.30));
                    sp::rect(
                        c,
                        Rect::new(g.x0, y, g.x0 + 3.0, y + self.key_h),
                        self.accent,
                    );
                }
                Some(s) if s.contains(k) => sp::rect(c, row, mix(st.lane, self.accent, 0.10)),
                Some(_) => sp::rect(c, row, st.row_black),
                None if is_black(k) => sp::rect(c, row, st.row_black),
                None => {}
            }
            let col = if k.is_multiple_of(12) {
                st.grid_bar
            } else {
                st.grid_sub
            };
            sp::line(c, g.x0, row.y1 - 0.5, g.x1, row.y1 - 0.5, col);
        }
        sp::grid(c, st, &self.axis, g, &self.bars);
        let (a, z) = self.region();
        let xa = self.axis.tick_to_x(a);
        let xz = self.axis.tick_to_x(z);
        if xa > g.x0 {
            sp::rect(c, Rect::new(g.x0, g.y0, xa.min(g.x1), g.y1), st.outside);
        }
        if xz < g.x1 {
            sp::rect(c, Rect::new(xz.max(g.x0), g.y0, g.x1, g.y1), st.outside);
            sp::line(
                c,
                xz.round() + 0.5,
                g.y0,
                xz.round() + 0.5,
                g.y1,
                st.loop_on.with_alpha(0.6),
            );
        }
        for (i, n) in self.notes().iter().enumerate() {
            let r = self.rect_of(n);
            if r.x1 < g.x0 || r.x0 > g.x1 || r.y1 < g.y0 || r.y0 > g.y1 {
                continue;
            }
            let r = Rect::new(
                r.x0.max(g.x0 - 4.0),
                r.y0 + 0.5,
                r.x1.min(g.x1 + 4.0),
                r.y1 - 0.5,
            );
            let selected = self.selection.contains(&i);
            // The fill is the velocity's colour; a selected note is lifted toward the text
            // colour (and gets the light outline), so a selection reads at any velocity.
            let base = st.velocity_color(n.velocity);
            let base = if selected {
                mix(base, st.base.text, 0.4)
            } else {
                base
            };
            let fill = base;
            let rr = RoundedRect::from_rect(Rect::new(r.x0, r.y0, r.x1.max(r.x0 + 2.0), r.y1), 2.0);
            // A muted note is hollow: a faint fill inside a clear outline in the note colour,
            // which velocity shading (a solid fill) never produces.
            c.fill_with(rr, if n.muted { fill.with_alpha(0.22) } else { fill });
            let (w, col) = match (selected, n.muted) {
                (true, _) => (1.5, st.base.text),
                (false, true) => (1.0, base.with_alpha(0.85)),
                (false, false) => (1.0, st.base.bevel_dark),
            };
            c.stroke_with(rr, w, col);
        }
        if let Drag::Band { start, pos, .. } = &self.drag {
            let r = Rect::from_points(*start, *pos);
            c.fill_with(r, st.selection.with_alpha(0.12));
            c.stroke_with(r, 1.0, st.selection.with_alpha(0.8));
        }
        if let Some(t) = self
            .clip
            .as_ref()
            .and_then(|cl| seq_edit::song_to_content(cl, self.playhead))
        {
            let x = self.axis.tick_to_x(t).round() + 0.5;
            c.stroke_with(
                vello::kurbo::Line::new(pt(x, g.y0), pt(x, g.y1)),
                1.5,
                st.playhead,
            );
        }
        c.pop_clip();
    }

    fn paint_ruler(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let g = self.grid_rect();
        let r = Rect::new(g.x0, g.y0 - ROLL_RULER_HEIGHT, g.x1, g.y0);
        c.push_clip(&r.into());
        sp::rect(c, r, st.ruler);
        sp::ruler_labels(c, st, &self.axis, r, &self.bars);
        let (a, z) = self.region();
        let band = Rect::new(
            self.axis.tick_to_x(a),
            r.y1 - 3.0,
            self.axis.tick_to_x(z),
            r.y1,
        );
        sp::rect(c, band, self.accent.with_alpha(0.8));
        sp::line(c, r.x0, r.y1 - 0.5, r.x1, r.y1 - 0.5, st.separator);
        c.pop_clip();
        let corner = Rect::new(self.bounds.x0, r.y0, g.x0, r.y1);
        sp::rect(c, corner, st.header);
        sp::line(
            c,
            corner.x0,
            r.y1 - 0.5,
            corner.x1,
            r.y1 - 0.5,
            st.separator,
        );
    }

    fn paint_keyboard(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let g = self.grid_rect();
        let kb = Rect::new(self.bounds.x0, g.y0, g.x0, g.y1);
        c.push_clip(&kb.into());
        sp::rect(c, kb, st.key_white);
        let sounding: Vec<u8> = match &self.drag {
            Drag::Keys { key } => vec![*key],
            Drag::Move { sounding, .. } => vec![*sounding],
            Drag::Draw { notes, .. } => notes.iter().map(|n| n.key).collect(),
            _ => Vec::new(),
        };
        let scale = self.scale().filter(|s| !s.is_chromatic());
        for r in self.visible_rows(g.y0, g.y1) {
            let k = self.rows.keys()[r];
            let y = self.row_top(r);
            let row = Rect::new(kb.x0, y, kb.x1, y + self.key_h);
            if is_black(k) {
                let split = row.center().y.round() + 0.5;
                sp::line(
                    c,
                    kb.x0 + KEYBOARD_WIDTH * 0.6,
                    split,
                    kb.x1,
                    split,
                    t.text_muted.with_alpha(0.6),
                );
                let r = Rect::new(
                    kb.x0,
                    row.y0 + 1.0,
                    kb.x0 + KEYBOARD_WIDTH * 0.6,
                    row.y1 - 1.0,
                );
                let col = if sounding.contains(&k) {
                    self.accent
                } else {
                    st.key_black
                };
                c.fill_with(RoundedRect::from_rect(r, 1.5), col);
            } else {
                if sounding.contains(&k) {
                    sp::rect(c, row, mix(st.key_white, self.accent, 0.6));
                }
                if matches!(k % 12, 4 | 11) || k == 127 {
                    sp::line(
                        c,
                        kb.x0,
                        row.y0 + 0.5,
                        kb.x1,
                        row.y0 + 0.5,
                        t.text_muted.with_alpha(0.6),
                    );
                }
            }
            // Labels follow the key zoom: smaller type on short keys, and from 16 px every
            // white key is named, not only the Cs.
            let size = if self.key_h < 10.0 {
                (self.key_h - 2.0).max(5.0)
            } else {
                8.0
            };
            let base = if self.key_h < 10.0 { 1.0 } else { 3.0 };
            if let Some(s) = scale
                && s.contains(k)
            {
                // A strip at the edge marks the scale keys, a bright one the root.
                let strip = Rect::new(kb.x1 - 3.0, row.y0 + 1.0, kb.x1 - 1.0, row.y1 - 1.0);
                sp::rect(
                    c,
                    strip,
                    self.accent
                        .with_alpha(if s.is_root(k) { 1.0 } else { 0.55 }),
                );
            }
            let is_root = scale.is_some_and(|s| s.is_root(k));
            let pad_name = self
                .key_labels
                .as_ref()
                .and_then(|l| l.iter().find(|(key, _)| *key == k))
                .map(|(_, name)| name);
            if let Some(name) = pad_name
                && self.key_h >= 9.0
            {
                let short: String = name.chars().take(KEY_LABEL_CHARS).collect();
                sp::label(
                    c,
                    &short,
                    kb.x1 - 6.0,
                    row.y1 - base,
                    size as f32,
                    self.accent,
                    Align::End,
                );
            } else if k.is_multiple_of(12) {
                sp::line(c, kb.x0, row.y1 - 0.5, kb.x1, row.y1 - 0.5, t.knob_bottom);
                let name = format!("C{}", i32::from(k) / 12 - 1);
                sp::label(
                    c,
                    &name,
                    kb.x1 - 6.0,
                    row.y1 - base,
                    size as f32,
                    if is_root { self.accent } else { t.plate },
                    Align::End,
                );
            } else if is_root {
                // The root of the scale is named wherever it is.
                sp::label(
                    c,
                    &crate::zone_edit::note_name(k),
                    kb.x1 - 6.0,
                    row.y1 - base,
                    size as f32,
                    self.accent,
                    Align::End,
                );
            } else if (self.key_h >= 16.0 && !is_black(k))
                || (!self.rows.is_full() && self.key_h >= 9.0)
            {
                // A folded roll is a list of notes, so every row is named.
                sp::label(
                    c,
                    &crate::zone_edit::note_name(k),
                    kb.x1 - 6.0,
                    row.y1 - base,
                    size as f32,
                    t.plate.with_alpha(0.7),
                    Align::End,
                );
            }
        }
        sp::line(c, kb.x1 - 0.5, kb.y0, kb.x1 - 0.5, kb.y1, t.bevel_dark);
        c.pop_clip();
    }

    fn paint_velocity(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let vr = self.velocity_rect();
        let side = Rect::new(self.bounds.x0, vr.y0, vr.x0, vr.y1);
        sp::rect(c, side, st.header);
        c.text(
            &TextRun::new("VEL", pt(side.x0 + 10.0, side.y0 + 16.0), 8.0, t.text_muted)
                .weight(700)
                .spacing(1.0),
        );
        c.push_clip(&vr.into());
        sp::rect(c, vr, st.lane_auto);
        let (top, h) = self.velocity_range();
        for f in [0.0, 0.5, 1.0] {
            let y = (top + f * h).round() + 0.5;
            sp::line(c, vr.x0, y, vr.x1, y, st.grid_sub);
        }
        for (i, n) in self.notes().iter().enumerate() {
            let x = self.axis.tick_to_x(n.tick) + 1.0;
            if x < vr.x0 - 4.0 || x > vr.x1 + 4.0 {
                continue;
            }
            let selected = self.selection.contains(&i);
            let col = st.velocity_color(n.velocity);
            let col = if selected {
                mix(col, st.base.text, 0.4)
            } else {
                col
            };
            let y = top + (1.0 - f64::from(n.velocity)) * h;
            let bottom = top + h;
            let (bar, cap) = if n.muted { (0.25, 0.4) } else { (0.85, 1.0) };
            c.fill_with(Rect::new(x - 1.0, y, x + 1.0, bottom), col.with_alpha(bar));
            c.fill_with(
                Rect::new(x - 3.0, y - 1.5, x + 3.0, y + 1.5),
                col.with_alpha(cap),
            );
        }
        sp::line(c, vr.x0, vr.y0 + 0.5, vr.x1, vr.y0 + 0.5, st.separator);
        c.pop_clip();
        sp::line(c, side.x0, vr.y0 + 0.5, side.x1, vr.y0 + 0.5, st.separator);
    }

    /// The controller area: the chip row and the lane of the chosen controller.
    fn paint_controllers(&self, c: &mut dyn Canvas) {
        use vello::kurbo::BezPath;
        let st = &self.st;
        let t = &st.base;
        let area = self.controller_area();
        let body = self.controller_rect();
        let (top, h) = self.ctl_range();
        let kind = self.ctl.kind;
        let lanes: &[ControllerLane] = self
            .clip
            .as_ref()
            .map_or(&[], |clip| clip.controllers.as_slice());
        // The chip row.
        let header = Rect::new(area.x0, area.y0, area.x1, area.y0 + CTL_HEADER_HEIGHT);
        sp::rect(c, header, st.header);
        sp::line(
            c,
            header.x0,
            header.y0 + 0.5,
            header.x1,
            header.y0 + 0.5,
            st.separator,
        );
        c.text(
            &TextRun::new(
                "CTRL",
                pt(area.x0 + 10.0, header.y0 + 14.0),
                8.0,
                t.text_muted,
            )
            .weight(700)
            .spacing(1.0),
        );
        for chip in cl::chips() {
            let r = cl::chip_rect(self.axis.x0, area.y0, chip);
            let active = self.ctl.is_active(chip);
            let hovered = matches!(self.hover, Hit::CtlChip(h, _) if h == chip);
            let has_data = match chip {
                Chip::Preset(i) => cl::PRESETS
                    .get(i)
                    .is_some_and(|k| cl::lane_of(lanes, *k).is_some()),
                Chip::Cc => lanes.iter().any(|l| {
                    l.kind == ControllerKind::Cc(self.ctl.cc) && !cl::PRESETS.contains(&l.kind)
                }),
                Chip::Poly => lanes
                    .iter()
                    .any(|l| matches!(l.kind, ControllerKind::PolyPressure(_))),
                Chip::Curve => false,
            };
            sp::lcd(
                c,
                st,
                r,
                (hovered || active).then_some(if active { t.highlight } else { t.focus }),
            );
            let colour = if active {
                t.highlight
            } else if has_data {
                t.text
            } else {
                t.text_muted
            };
            sp::label(
                c,
                &cl::chip_label(chip, &self.ctl),
                r.center().x,
                r.center().y + 3.5,
                9.0,
                colour,
                Align::Center,
            );
            if has_data && !active {
                sp::rect(
                    c,
                    Rect::new(r.x1 - 6.0, r.y0 + 2.0, r.x1 - 3.0, r.y0 + 5.0),
                    self.accent,
                );
            }
        }
        // The side panel: the lane's name and its value range.
        let side = Rect::new(area.x0, body.y0, body.x0, body.y1);
        sp::rect(c, side, st.header);
        let (lo, hi) = kind.range();
        sp::label(
            c,
            &cl::value_text(kind, hi),
            side.x1 - 5.0,
            top + 7.0,
            8.0,
            t.text_muted,
            Align::End,
        );
        sp::label(
            c,
            &cl::value_text(kind, lo),
            side.x1 - 5.0,
            top + h,
            8.0,
            t.text_muted,
            Align::End,
        );
        sp::label(
            c,
            &cl::kind_name(kind),
            side.x0 + 8.0,
            side.center().y + 3.0,
            8.0,
            t.text,
            Align::Start,
        );
        // The lane.
        c.push_clip(&body.into());
        sp::rect(c, body, st.lane_auto);
        sp::grid(c, st, &self.axis, body, &self.bars);
        for (i, f) in [0.0, 0.5, 1.0].into_iter().enumerate() {
            let y = (top + f * h).round() + 0.5;
            let col = if kind == ControllerKind::PitchBend && i == 1 {
                st.grid_bar
            } else {
                st.grid_sub
            };
            sp::line(c, body.x0, y, body.x1, y, col);
        }
        let (a, z) = self.region();
        let (xa, xz) = (self.axis.tick_to_x(a), self.axis.tick_to_x(z));
        if xa > body.x0 {
            sp::rect(
                c,
                Rect::new(body.x0, body.y0, xa.min(body.x1), body.y1),
                st.outside,
            );
        }
        if xz < body.x1 {
            sp::rect(
                c,
                Rect::new(xz.max(body.x0), body.y0, body.x1, body.y1),
                st.outside,
            );
        }
        let points: &[atomik_model::AutomationPoint] =
            cl::lane_of(lanes, kind).map_or(&[], |l| l.points.as_slice());
        let xy = |p: &atomik_model::AutomationPoint| {
            (
                self.axis.tick_to_x(p.tick),
                cl::y_of_value(kind, p.value, top, h),
            )
        };
        if let (Some(first), Some(last)) = (points.first(), points.last()) {
            // Held to the end of the clip's region after the last point.
            let end = self.axis.tick_to_x(z).max(xy(last).0);
            let mut path = BezPath::new();
            let (x0, y0) = xy(first);
            path.move_to(pt(x0, y0));
            for (prev, next) in points.iter().zip(points.iter().skip(1)) {
                let (nx, ny) = xy(next);
                if prev.curve == Curve::Step {
                    path.line_to(pt(nx, xy(prev).1));
                }
                path.line_to(pt(nx, ny));
            }
            path.line_to(pt(end, xy(last).1));
            if kind != ControllerKind::PitchBend {
                let mut fill = path.clone();
                fill.line_to(pt(end, top + h));
                fill.line_to(pt(x0, top + h));
                fill.close_path();
                c.fill_with(fill, self.accent.with_alpha(0.16));
            }
            c.stroke_with(path, 1.5, self.accent);
        }
        for (i, p) in points.iter().enumerate() {
            let (x, y) = xy(p);
            if x < body.x0 - 6.0 || x > body.x1 + 6.0 {
                continue;
            }
            let selected = self.ctl_sel == Some(i);
            let col = if selected { st.selection } else { self.accent };
            let r = Rect::new(x - 3.0, y - 3.0, x + 3.0, y + 3.0);
            if p.curve == Curve::Step {
                c.fill_with(r, col);
                c.stroke_with(r, 1.0, t.bevel_dark);
            } else {
                let rr = RoundedRect::from_rect(r, 3.0);
                c.fill_with(rr, col);
                c.stroke_with(rr, 1.0, t.bevel_dark);
            }
            if selected {
                sp::label(
                    c,
                    &cl::value_text(kind, p.value),
                    x,
                    (y - 7.0).max(body.y0 + 9.0),
                    9.0,
                    t.text,
                    Align::Center,
                );
            }
        }
        if let Some(tick) = self
            .clip
            .as_ref()
            .and_then(|clip| seq_edit::song_to_content(clip, self.playhead))
        {
            let x = self.axis.tick_to_x(tick).round() + 0.5;
            sp::line(c, x, body.y0, x, body.y1, st.playhead);
        }
        sp::line(
            c,
            body.x0,
            body.y0 + 0.5,
            body.x1,
            body.y0 + 0.5,
            st.separator,
        );
        c.pop_clip();
        sp::line(
            c,
            side.x0,
            body.y0 + 0.5,
            side.x1,
            body.y0 + 0.5,
            st.separator,
        );
    }

    /// The scale bar: scale, detect, snap to scale, fold and chord stamp.
    fn paint_scale_bar(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let b = self.bounds;
        let bar = Rect::new(
            b.x0,
            b.y0 + TOP_BAR_HEIGHT,
            b.x1,
            b.y0 + TOP_BAR_HEIGHT + SCALE_BAR_HEIGHT,
        );
        sp::rect(c, bar, st.header);
        sp::line(c, bar.x0, bar.y1 - 0.5, bar.x1, bar.y1 - 0.5, st.separator);
        let hovered = |item| self.hover == Hit::Top(Some(item));
        let field = |c: &mut dyn Canvas, item: TopItem, text: &str, color: Color, active: bool| {
            let r = self.top_rect(item);
            sp::lcd(
                c,
                st,
                r,
                (hovered(item) || active).then_some(if active { t.highlight } else { t.focus }),
            );
            sp::label(
                c,
                text,
                r.center().x,
                r.center().y + 3.5,
                9.0,
                color,
                Align::Center,
            );
        };
        let caption = |c: &mut dyn Canvas, item: TopItem, text: &str| {
            let r = self.top_rect(item);
            sp::label(
                c,
                text,
                r.x0 - 6.0,
                r.center().y + 3.0,
                8.0,
                t.text_muted,
                Align::End,
            );
        };
        let scale = self.scale().filter(|s| !s.is_chromatic());
        caption(c, TopItem::ScaleRoot, "SCALE");
        let root = scale.map_or(self.root_pref, |s| s.root);
        field(
            c,
            TopItem::ScaleRoot,
            pitch_name(root),
            if scale.is_some() {
                t.highlight
            } else {
                t.text_muted
            },
            false,
        );
        field(
            c,
            TopItem::ScaleKind,
            scale.map_or(ScaleKind::Chromatic, |s| s.kind).label(),
            if scale.is_some() {
                t.highlight
            } else {
                t.text_muted
            },
            false,
        );
        field(c, TopItem::ScaleDetect, "DET", t.text, false);
        field(
            c,
            TopItem::ScaleSnap,
            "SCL SNAP",
            if self.snap_scale {
                t.highlight
            } else {
                t.text_muted
            },
            self.snap_scale,
        );
        caption(c, TopItem::Fold, "FOLD");
        field(
            c,
            TopItem::Fold,
            match self.fold {
                Fold::Off => "OFF",
                Fold::Scale => "SCALE",
                Fold::Used => "USED",
            },
            if self.fold == Fold::Off {
                t.text_muted
            } else {
                t.highlight
            },
            self.fold != Fold::Off,
        );
        field(
            c,
            TopItem::Controllers,
            "CONTROLLERS",
            if self.ctl.open {
                t.highlight
            } else {
                t.text_muted
            },
            self.ctl.open,
        );
        caption(c, TopItem::Chord, "CHORD");
        let stamped = self.stamp.is_active(self.scale());
        field(
            c,
            TopItem::Chord,
            self.stamp.short(),
            if stamped { t.highlight } else { t.text_muted },
            stamped,
        );
        field(
            c,
            TopItem::Inversion,
            &format!("INV {}", self.inversion),
            if self.inversion > 0 {
                t.highlight
            } else {
                t.text_muted
            },
            false,
        );
    }

    fn paint_top_bar(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let b = self.bounds;
        let r = Rect::new(b.x0, b.y0, b.x1, b.y0 + TOP_BAR_HEIGHT);
        let grad = vello::peniko::Gradient::new_linear(pt(0.0, r.y0), pt(0.0, r.y1))
            .with_stops([t.panel_top, t.panel_bottom]);
        c.fill(&r.into(), &grad.into());
        sp::line(c, r.x0, r.y1 - 0.5, r.x1, r.y1 - 0.5, t.bevel_dark);
        sp::rect(c, Rect::new(r.x0, r.y0, r.x0 + 4.0, r.y1), self.accent);
        c.text(
            &TextRun::new(
                "PIANO ROLL",
                pt(r.x0 + 14.0, r.y0 + 18.0),
                8.0,
                t.text_muted,
            )
            .weight(700)
            .spacing(1.2),
        );
        if let Some(clip) = &self.clip {
            let bar = self.meter.bar_ticks();
            let (b0, b1) = (
                clip.start / bar + 1,
                (clip.start + clip.length + bar - 1) / bar,
            );
            let text = format!(
                "{}  ·  bars {b0}–{b1}  ·  {} notes",
                self.title,
                self.notes().len()
            );
            let text = fit_label(
                &text,
                (self.top_rect(TopItem::Tool(Tool::Select)).x0 - r.x0 - 100.0).max(20.0),
                11.0,
            );
            c.text(&TextRun::new(text, pt(r.x0 + 92.0, r.y0 + 18.5), 11.0, t.text).weight(600));
        }
        let sr = self.top_rect(TopItem::Snap);
        sp::label(
            c,
            "SNAP",
            sr.x0 - 6.0,
            sr.center().y + 3.0,
            8.0,
            t.text_muted,
            Align::End,
        );
        sp::lcd(
            c,
            st,
            sr,
            (self.hover == Hit::Top(Some(TopItem::Snap))).then_some(t.focus),
        );
        sp::label(
            c,
            self.snap.label(),
            sr.center().x,
            sr.center().y + 3.5,
            9.5,
            t.highlight,
            Align::Center,
        );
        let shown = self.vp.shown_tool();
        for tool in Tool::NOTE {
            let tr = self.top_rect(TopItem::Tool(tool));
            let active = tool == shown;
            sp::lcd(
                c,
                st,
                tr,
                (self.hover == Hit::Top(Some(TopItem::Tool(tool))) || active)
                    .then_some(if active { t.highlight } else { t.focus }),
            );
            sp::label(
                c,
                tool.label(),
                tr.center().x,
                tr.center().y + 3.0,
                7.5,
                if active { t.highlight } else { t.text_muted },
                Align::Center,
            );
        }
        let qr = self.top_rect(TopItem::Quantize);
        sp::button(
            c,
            st,
            qr,
            "",
            None,
            self.hover == Hit::Top(Some(TopItem::Quantize)),
        );
        sp::label(
            c,
            "QUANT",
            qr.center().x,
            qr.center().y + 3.5,
            9.0,
            t.text,
            Align::Center,
        );
        let or = self.top_rect(TopItem::QuantizeOptions);
        sp::button(
            c,
            st,
            or,
            "",
            None,
            self.hover == Hit::Top(Some(TopItem::QuantizeOptions)),
        );
        sp::label(
            c,
            "...",
            or.center().x,
            or.center().y + 3.5,
            9.0,
            t.text,
            Align::Center,
        );
        let cr = self.top_rect(TopItem::Close);
        sp::button(
            c,
            st,
            cr,
            "",
            None,
            self.hover == Hit::Top(Some(TopItem::Close)),
        );
        let k = cr.center();
        for (dx, dy) in [(4.0, 4.0), (4.0, -4.0)] {
            c.stroke_with(
                vello::kurbo::Line::new(pt(k.x - dx, k.y - dy), pt(k.x + dx, k.y + dy)),
                1.6,
                t.text,
            );
        }
    }
}

/// `base` plus `notes`, sorted and without two notes on one start and key; the returned
/// selection holds the indices of the new notes.
fn with_notes(base: &[Note], notes: &[Note]) -> (Vec<Note>, BTreeSet<usize>) {
    let mut list = base.to_vec();
    list.extend_from_slice(notes);
    let tracked: BTreeSet<usize> = (base.len()..list.len()).collect();
    chord_edit::unique(list, &tracked)
}

/// The next (`by` = 1) or previous (-1) value of a list, wrapping around.
fn cycle<T: Copy + PartialEq>(all: &[T], current: T, by: isize) -> T {
    let i = all.iter().position(|x| *x == current).unwrap_or(0) as isize;
    all[(i + by).rem_euclid(all.len() as isize) as usize]
}

/// Moves the `sel` notes vertically by `steps` (positive = up) the way the roll's state says:
/// by rows when a fold hides keys (the pointer's rows are the unit), by scale degree when snap
/// to scale is on and the clip has a scale (the grabbed note lands on the in-scale key nearest
/// to the pointer and the others move the same number of degrees), else by semitones. The
/// group keeps its shape: the move is cut so that every note stays on the keyboard.
fn vertical_move(
    rows: &KeyRows,
    scale: Option<Scale>,
    snap_scale: bool,
    notes: &[Note],
    sel: &BTreeSet<usize>,
    grabbed: usize,
    steps: i32,
) -> Vec<Note> {
    if steps == 0 {
        return notes.to_vec();
    }
    if !rows.is_full() {
        let picked: Vec<usize> = notes
            .iter()
            .enumerate()
            .filter(|(i, _)| sel.contains(i))
            .map(|(_, n)| rows.nearest_row(n.key))
            .collect();
        let (Some(lo), Some(hi)) = (picked.iter().min(), picked.iter().max()) else {
            return notes.to_vec();
        };
        // A row index grows downwards: moving up by `steps` subtracts.
        let steps = steps.clamp(*hi as i32 - (rows.len() as i32 - 1), *lo as i32);
        return notes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                if sel.contains(&i) {
                    let r = rows.nearest_row(n.key) as i64 - i64::from(steps);
                    Note {
                        key: rows.key_at_row(r),
                        ..*n
                    }
                } else {
                    *n
                }
            })
            .collect();
    }
    match scale {
        Some(s) if snap_scale && !s.is_chromatic() => {
            let from = notes[grabbed].key;
            let to = s.nearest((i32::from(from) + steps).clamp(0, 127) as u8);
            chord_edit::move_degrees(notes, sel, s.degrees_between(from, to), s)
        }
        _ => seq_edit::move_notes(notes, sel, 0, steps),
    }
}

fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}
