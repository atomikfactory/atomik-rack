//! The step editor: a pane that replaces the arrangement and edits one
//! note clip of a drum machine as a grid of pads and steps.
//!
//! Hosting follows [`crate::SampleEditorView`]: on [`UiAction::OpenStepEditor`] resolve the
//! clip if the action carries none ([`StepEditorView::resolve_clip`]), call
//! [`StepEditorView::open`] and show the view in place of the arrangement; on
//! [`UiAction::CloseStepEditor`] show the arrangement again (also when
//! [`StepEditorView::is_present`] turns false because the drum machine or the clip was
//! deleted). On [`UiAction::CreateStepClip`] the app creates the clip and opens it.
//!
//! # Layout (logical pixels, inside the bounds)
//!
//! ```text
//! top bar     STEP EDITOR  <device> · <steps> x <res> · swing                       [x]
//! toolbar     GRID [1/8][1/16][1/32]  STEPS [16][32][48][64]  SWING [ 25 % ]  VEL [SOFT][MED][HARD]   n off-grid
//! ruler       (header column)  step numbers at beats, bar starts marked
//! rows        pad 16 (top) ... pad 1 (bottom, key 36, the GM kick); header: pad number + name
//! ```
//!
//! Pad 1 is at the **bottom** (GM convention: the kick at the bottom, like a drum-machine
//! pad block). A row header shows the pad number and the original file name of the first zone
//! of the pad (`RackDoc::assets`), else the note name of the pad key with C4 = 60. Empty pads
//! are dimmed. Columns are the clip's steps; shading marks beats and bars (from the song
//! meter). The cell width fits the steps when it can (14 to 44 px); otherwise the grid
//! scrolls horizontally (wheel with Shift, or horizontal wheel), the rows scroll vertically
//! (wheel) when the pane is too short for 16 rows.
//!
//! # The grid and the clip
//!
//! * A clip **without a grid** is shown with a default `16 x 1/16` grid, swing 0
//!   ([`DEFAULT_GRID`]). It is *not* written: note edits leave `Clip::grid` alone; the first
//!   toolbar edit (resolution, steps or swing) writes the complete grid.
//! * A cell `(pad, step k)` is **on** when a note of the pad's key starts inside the step:
//!   unswung tick in `[k*s, (k+1)*s)` with `s` the step length in ticks. Its velocity is the
//!   fill height; three colours mark soft (< 60 of 127), medium and hard (>= 100).
//! * A note is **off-grid** when its key is not a pad key (36..=51) or its tick lies outside
//!   `[0, steps*s)`. Off-grid notes are never drawn in the cells, never edited and never
//!   deleted by any grid edit; the toolbar shows their count.
//! * Changing the resolution or the steps emits **one** [`Command::Batch`] of
//!   `SetClipGrid` and (when the length differs) `SetClipGeometry`. **Rule: the clip length
//!   always follows the grid**, `length = steps * s`; start, offset, loop length and mute stay.
//!   Notes are never moved, so a finer resolution keeps every note at its tick.
//!
//! # Interaction
//!
//! * **Click an empty cell:** adds one note at the step start, length one step, at the default
//!   velocity ([`StepEditorView::default_level`], the SOFT/MED/HARD buttons: 40, 80, 120 of
//!   127). **Click an on cell:** removes its note(s). Both are one `SetClipContent` with the
//!   full sorted note list, exactly what the piano roll emits, merged per gesture and ended
//!   with [`UiAction::EndMerge`].
//! * **Drag from an empty cell** along its row paints every empty cell it enters. **Drag
//!   from an on cell** vertically changes the velocity of that cell continuously
//!   ([`VELOCITY_DRAG_PIXELS`] px for the full range); dragging it **sideways** erases it and
//!   the cells it enters. The row of a paint or erase drag is the row of the press. Esc during
//!   a drag restores the notes of the press.
//! * **Row header:** press auditions the pad ([`UiAction::AuditionVelocity`] at velocity 100,
//!   note off on release).
//! * **Swing:** drag the swing field horizontally (1 px = 1 %, Shift = 0.1 %), double-click
//!   resets, wheel steps by 1; every move is `SetClipGrid` with one merge id per drag.
//! * **Drop:** files over a row header emit [`UiAction::ImportSamples`] with
//!   `at_key: Some(pad key)`; elsewhere in the pane `at_key: None`; a hover indicator marks the
//!   target.
//! * **Empty state** (no clip): a message and a NEW PATTERN button emitting
//!   [`UiAction::CreateStepClip`].
//! * Esc (with nothing to cancel) and the close button emit [`UiAction::CloseStepEditor`];
//!   Ctrl/Cmd+Z / Y undo and redo.
//!
//! Merge ids come from 5·2⁴⁰, so they never collide with the other views.

use crate::action::UiAction;
use crate::canvas::Canvas;
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::keymap::CommandOutcome;
use crate::seq_edit;
use crate::seq_paint::SeqTheme;
use crate::theme::Theme;
use crate::tooltip::Help;
use crate::zone_edit::note_name;
use atomik_core::PPQ;
use atomik_device_sdk::Registry;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Note, PAD_COUNT, RackDoc, SongCommand, StepGrid,
    StepResolution, pad_key, pad_of_key, pad_zones,
};
use std::collections::BTreeSet;
use vello::kurbo::{Point, Rect};
use vello::peniko::Color;

mod paint;

/// The device type the step editor edits.
pub const DRUM_MACHINE_TYPE: &str = "atomik.drum-machine";
/// The grid shown for a clip that has none (not written until the user edits the grid).
pub const DEFAULT_GRID: StepGrid = StepGrid {
    resolution: StepResolution::Sixteenth,
    steps: 16,
    swing: 0,
};
/// The step counts of the length buttons.
pub const STEP_CHOICES: [u16; 4] = [16, 32, 48, 64];
/// Largest swing in percent (the model's `StepGrid` limit).
pub const SWING_MAX: u8 = 75;
/// Velocity (7-bit) of an audition from a row header.
pub const AUDITION_VELOCITY: u8 = 100;
/// Pixels of vertical drag for the full velocity range.
pub const VELOCITY_DRAG_PIXELS: f64 = 64.0;
/// Height of the top bar.
pub const TOP_BAR_HEIGHT: f64 = 28.0;
/// Height of the toolbar.
pub const TOOLBAR_HEIGHT: f64 = 32.0;
/// Height of the step ruler.
pub const RULER_HEIGHT: f64 = 18.0;
/// Width of the row header column.
pub const HEADER_WIDTH: f64 = 132.0;
/// Height of one pad row.
pub const ROW_HEIGHT: f64 = 22.0;
const MIN_CELL: f64 = 14.0;
const MAX_CELL: f64 = 44.0;
const PAD: f64 = 8.0;
const DRAG_THRESHOLD: f64 = 3.0;
const SCROLL_STEP: f64 = 36.0;
const MERGE_BASE: u64 = 5 << 40;
const FINE_FACTOR: f64 = 0.1;

/// The three velocity levels of the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VelocityLevel {
    /// 40 of 127.
    Soft,
    /// 80 of 127.
    Medium,
    /// 120 of 127.
    Hard,
}

impl VelocityLevel {
    /// All levels, soft to hard.
    pub const ALL: [VelocityLevel; 3] = [Self::Soft, Self::Medium, Self::Hard];

    /// The 7-bit velocity: 40, 80 or 120.
    pub fn velocity7(self) -> u8 {
        match self {
            Self::Soft => 40,
            Self::Medium => 80,
            Self::Hard => 120,
        }
    }

    /// The note velocity (`velocity7 / 127`).
    pub fn velocity(self) -> f32 {
        f32::from(self.velocity7()) / 127.0
    }

    /// The level a note velocity is drawn as: soft below 60 of 127, hard from 100, else medium.
    pub fn of_velocity(v: f32) -> Self {
        let v7 = (v * 127.0).round();
        if v7 < 60.0 {
            Self::Soft
        } else if v7 < 100.0 {
            Self::Medium
        } else {
            Self::Hard
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Soft => "SOFT",
            Self::Medium => "MED",
            Self::Hard => "HARD",
        }
    }
}

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    None,
    Close,
    NewPattern,
    Resolution(StepResolution),
    Steps(u16),
    Swing,
    Level(VelocityLevel),
    Header(u8),
    Cell(u8, u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Pressed an on cell, nothing decided yet (a click removes it on release).
    Pending,
    /// Painting empty cells along the row.
    Add,
    /// Erasing cells along the row.
    Erase,
    /// Changing the velocity of the pressed cell.
    Velocity,
}

#[derive(Debug, Clone)]
struct Gesture {
    pad: u8,
    mode: Mode,
    merge: u64,
    start: Point,
    step0: u16,
    last_step: u16,
    v0: f32,
    base: Vec<Note>,
    last: Vec<Note>,
    s: i64,
    emitted: bool,
}

#[derive(Debug, Clone)]
enum Drag {
    None,
    /// Auditioning a pad (row header held).
    Audition(u8),
    /// Painting, erasing or velocity-dragging cells.
    Cells(Box<Gesture>),
    /// Dragging the swing field.
    Swing {
        start_x: f64,
        v0: f64,
        grid: StepGrid,
        merge: u64,
        last: u8,
        emitted: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DropHover {
    pad_key: Option<u8>,
    count: usize,
}

/// The rectangles of the view, from its bounds.
#[derive(Debug, Clone, Copy)]
struct Layout {
    top: Rect,
    close: Rect,
    toolbar: Rect,
    ruler: Rect,
    rows: Rect,
    avail_w: f64,
    cell_w: f64,
}

/// The step editor. See the module documentation.
#[derive(Debug)]
pub struct StepEditorView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    device: Option<DeviceKey>,
    clip_id: Option<ClipId>,
    present: bool,
    name: String,
    accent: Color,
    clip: Option<Clip>,
    pad_files: [Option<String>; 16],
    pad_sound: [bool; 16],
    beat_ticks: i64,
    bar_ticks: i64,
    level: VelocityLevel,
    scroll_x: f64,
    scroll_y: f64,
    drag: Drag,
    hover: Hit,
    drop: Option<DropHover>,
    next_merge: u64,
    focused: bool,
    repaint: bool,
}

impl StepEditorView {
    /// A closed editor; call [`Self::set_bounds`], [`Self::open`] and [`Self::sync`].
    pub fn new(theme: Theme) -> Self {
        let st = SeqTheme::from_theme(&theme);
        let accent = st.base.highlight;
        Self {
            st,
            bounds: Rect::new(0.0, 0.0, 1000.0, 560.0),
            scale: 1.0,
            device: None,
            clip_id: None,
            present: false,
            name: String::new(),
            accent,
            clip: None,
            pad_files: Default::default(),
            pad_sound: [false; 16],
            beat_ticks: PPQ,
            bar_ticks: 4 * PPQ,
            level: VelocityLevel::Medium,
            scroll_x: 0.0,
            scroll_y: 0.0,
            drag: Drag::None,
            hover: Hit::None,
            drop: None,
            next_merge: MERGE_BASE,
            focused: false,
            repaint: true,
        }
    }

    // ------------------------------------------------------------------ host API

    /// Opens the editor on a drum machine and one of its note clips (`None` shows the empty
    /// state with the NEW PATTERN button).
    pub fn open(&mut self, device: DeviceKey, clip: Option<ClipId>) {
        if self.device.as_ref() != Some(&device) || self.clip_id != clip {
            self.scroll_x = 0.0;
            self.scroll_y = 0.0;
            self.clip = None;
        }
        self.device = Some(device);
        self.clip_id = clip;
        self.present = false;
        self.drag = Drag::None;
        self.drop = None;
        self.repaint = true;
    }

    /// Closes the editor (it then ignores input and paints an empty state).
    pub fn close(&mut self) {
        self.device = None;
        self.clip_id = None;
        self.clip = None;
        self.present = false;
        self.drag = Drag::None;
        self.drop = None;
        self.repaint = true;
    }

    /// The clip an Edit request opens: `selected` if it is a note clip on the device's track,
    /// else the note clip of that track under `playhead` (the first one if they overlap),
    /// else `None`.
    pub fn resolve_clip(
        doc: &RackDoc,
        device: &DeviceKey,
        selected: Option<ClipId>,
        playhead: i64,
    ) -> Option<ClipId> {
        let track = doc.song.tracks.iter().find(|t| &t.device == device)?;
        let clips = || {
            track
                .lanes
                .iter()
                .filter(|l| l.kind == atomik_model::LaneKind::Notes)
                .flat_map(|l| l.clips.iter())
                .filter(|c| c.content.is_notes())
        };
        if let Some(s) = selected
            && clips().any(|c| c.id == s)
        {
            return Some(s);
        }
        clips()
            .find(|c| c.start <= playhead && playhead < c.end())
            .map(|c| c.id)
    }

    /// The drum machine being edited.
    pub fn device(&self) -> Option<&DeviceKey> {
        self.device.as_ref()
    }

    /// The clip being edited (`None` in the empty state).
    pub fn clip(&self) -> Option<ClipId> {
        self.clip_id
    }

    /// True while the open drum machine exists in the document (and the open clip, if any,
    /// is still a note clip of its track). When it turns false the host shows the arrangement
    /// again.
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

    /// The default velocity level of new steps.
    pub fn default_level(&self) -> VelocityLevel {
        self.level
    }

    /// Sets the default velocity level of new steps.
    pub fn set_default_level(&mut self, level: VelocityLevel) {
        if level != self.level {
            self.level = level;
            self.repaint = true;
        }
    }

    /// The grid in use: the clip's, or [`DEFAULT_GRID`] for a clip without one.
    pub fn grid(&self) -> StepGrid {
        self.clip
            .as_ref()
            .and_then(|c| c.grid)
            .unwrap_or(DEFAULT_GRID)
    }

    /// Whether the clip carries a grid (false = the default grid is only displayed).
    pub fn has_grid(&self) -> bool {
        self.clip.as_ref().is_some_and(|c| c.grid.is_some())
    }

    /// The notes of the open clip, as last synced.
    pub fn notes(&self) -> &[Note] {
        match self.clip.as_ref().map(|c| &c.content) {
            Some(ClipContent::Notes(n)) => n,
            _ => &[],
        }
    }

    /// The velocity of a cell (the loudest note starting in it), `None` when it is off.
    pub fn cell_velocity(&self, pad: u8, step: u16) -> Option<f32> {
        let key = pad_key(pad);
        let s = self.step_ticks();
        self.notes()
            .iter()
            .filter(|n| n.key == key && in_cell(n.tick, step, s))
            .map(|n| n.velocity)
            .fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v))))
    }

    /// Whether a cell is on and every note starting in it is muted.
    pub fn cell_muted(&self, pad: u8, step: u16) -> bool {
        let key = pad_key(pad);
        let s = self.step_ticks();
        let mut inside = self
            .notes()
            .iter()
            .filter(|n| n.key == key && in_cell(n.tick, step, s))
            .peekable();
        inside.peek().is_some() && inside.all(|n| n.muted)
    }

    /// Number of notes the grid does not show (key outside the pads, tick outside the grid).
    pub fn off_grid_count(&self) -> usize {
        let g = self.grid();
        let end = i64::from(g.steps) * g.resolution.ticks();
        self.notes()
            .iter()
            .filter(|n| pad_of_key(n.key).is_none() || n.tick < 0 || n.tick >= end)
            .count()
    }

    /// The name shown in the header of `pad`: the original file name of its first zone's
    /// asset, else the note name of its key (C4 = 60).
    pub fn pad_name(&self, pad: u8) -> String {
        let pad = pad.clamp(1, PAD_COUNT);
        self.pad_files[usize::from(pad - 1)]
            .clone()
            .unwrap_or_else(|| note_name(pad_key(pad)))
    }

    /// Whether `pad` has at least one zone (empty pads are dimmed).
    pub fn pad_has_sound(&self, pad: u8) -> bool {
        self.pad_sound[usize::from(pad.clamp(1, PAD_COUNT) - 1)]
    }

    /// The drop indicator while files hover over the view: `Some(Some(key))` over a row header
    /// (the pad key), `Some(None)` elsewhere in the view.
    pub fn drop_hover(&self) -> Option<Option<u8>> {
        self.drop.map(|d| d.pad_key)
    }

    // ------------------------------------------------------------------ geometry (public, for tests and hosts)

    /// Step length of the grid in ticks.
    pub fn step_ticks(&self) -> i64 {
        self.grid().resolution.ticks()
    }

    /// The row header of `pad` (clipped to the visible rows only by the caller).
    pub fn header_rect(&self, pad: u8) -> Rect {
        let l = self.layout();
        let y = self.row_y(pad);
        Rect::new(
            self.bounds.x0,
            y,
            self.bounds.x0 + HEADER_WIDTH,
            y + ROW_HEIGHT,
        )
        .intersect(Rect::new(
            l.rows.x0,
            l.rows.y0,
            l.rows.x0 + HEADER_WIDTH,
            l.rows.y1,
        ))
    }

    /// Centre of a row header.
    pub fn header_center(&self, pad: u8) -> Point {
        self.header_rect(pad).center()
    }

    /// The cell of `pad` and `step` (may lie partly outside the visible area when scrolled).
    pub fn cell_rect(&self, pad: u8, step: u16) -> Rect {
        let l = self.layout();
        let x = self.bounds.x0 + HEADER_WIDTH + f64::from(step) * l.cell_w - self.scroll_x;
        let y = self.row_y(pad);
        Rect::new(x, y, x + l.cell_w, y + ROW_HEIGHT)
    }

    /// Centre of a cell.
    pub fn cell_center(&self, pad: u8, step: u16) -> Point {
        self.cell_rect(pad, step).center()
    }

    /// Centre of the close button.
    pub fn close_button_center(&self) -> Point {
        self.layout().close.center()
    }

    /// Centre of the NEW PATTERN button of the empty state.
    pub fn new_pattern_button_center(&self) -> Point {
        self.new_pattern_rect().center()
    }

    /// Centre of a resolution button.
    pub fn resolution_button_center(&self, res: StepResolution) -> Point {
        self.res_rect(res).center()
    }

    /// Centre of a steps button.
    pub fn steps_button_center(&self, steps: u16) -> Point {
        self.steps_rect(steps).center()
    }

    /// Centre of the swing field.
    pub fn swing_field_center(&self) -> Point {
        self.swing_rect().center()
    }

    /// Centre of a velocity level button.
    pub fn level_button_center(&self, level: VelocityLevel) -> Point {
        self.level_rect(level).center()
    }

    // ------------------------------------------------------------------ sync

    /// Reads the open drum machine and clip from the document. Emits nothing.
    pub fn sync(&mut self, doc: &RackDoc, _registry: &Registry) {
        let Some(key) = &self.device else { return };
        let dev = doc.device(key).filter(|d| d.type_id == DRUM_MACHINE_TYPE);
        let Some(d) = dev else {
            if self.present || self.clip.is_some() {
                self.present = false;
                self.clip = None;
                self.drag = Drag::None;
                self.repaint = true;
            }
            return;
        };
        let clip = self.clip_id.and_then(|id| {
            doc.song
                .tracks
                .iter()
                .filter(|t| &t.device == key)
                .flat_map(|t| t.lanes.iter())
                .flat_map(|l| l.clips.iter())
                .find(|c| c.id == id && c.content.is_notes())
                .cloned()
        });
        let present = self.clip_id.is_none() || clip.is_some();
        let mut files: [Option<String>; 16] = Default::default();
        let mut sound = [false; 16];
        for pad in 1..=PAD_COUNT {
            let i = usize::from(pad - 1);
            if let Some(zi) = pad_zones(&d.zones, pad).next() {
                sound[i] = true;
                files[i] = doc
                    .assets
                    .get(&d.zones[zi].asset)
                    .map(|m| m.original_name.clone());
            }
        }
        // The clip grid counts content ticks from the clip start: the meter in effect there.
        let meter = crate::timeline::Meter::at(&doc.song, clip.as_ref().map_or(0, |c| c.start));
        let (beat_ticks, bar_ticks) = (meter.beat_ticks(), meter.bar_ticks());
        let accent = self.st.base.accent_for(d);
        if self.present != present
            || self.clip != clip
            || self.pad_files != files
            || self.pad_sound != sound
            || self.beat_ticks != beat_ticks
            || self.bar_ticks != bar_ticks
            || self.name != d.name
            || self.accent != accent
        {
            self.present = present;
            self.clip = clip;
            self.pad_files = files;
            self.pad_sound = sound;
            self.beat_ticks = beat_ticks;
            self.bar_ticks = bar_ticks;
            self.name.clone_from(&d.name);
            self.accent = accent;
            self.repaint = true;
        }
        self.clamp_scrolls();
    }

    // ------------------------------------------------------------------ geometry

    fn layout(&self) -> Layout {
        let b = self.bounds;
        let rect = |x0: f64, y0: f64, x1: f64, y1: f64| Rect::new(x0, y0, x1.max(x0), y1.max(y0));
        let top = rect(b.x0, b.y0, b.x1, b.y0 + TOP_BAR_HEIGHT);
        let close = Rect::new(b.x1 - 30.0, b.y0 + 5.0, b.x1 - 8.0, b.y0 + 23.0);
        let toolbar = rect(b.x0, top.y1, b.x1, top.y1 + TOOLBAR_HEIGHT);
        let ruler = rect(b.x0, toolbar.y1, b.x1, toolbar.y1 + RULER_HEIGHT);
        let rows = rect(b.x0, ruler.y1, b.x1, b.y1);
        let avail_w = (b.width() - HEADER_WIDTH - PAD).max(1.0);
        let steps = f64::from(self.grid().steps.max(1));
        let cell_w = (avail_w / steps).clamp(MIN_CELL, MAX_CELL);
        Layout {
            top,
            close,
            toolbar,
            ruler,
            rows,
            avail_w,
            cell_w,
        }
    }

    fn row_y(&self, pad: u8) -> f64 {
        let l = self.layout();
        l.rows.y0 + f64::from(PAD_COUNT - pad.clamp(1, PAD_COUNT)) * ROW_HEIGHT - self.scroll_y
    }

    fn clamp_scrolls(&mut self) {
        let l = self.layout();
        let content_w = f64::from(self.grid().steps) * l.cell_w;
        self.scroll_x = self.scroll_x.clamp(0.0, (content_w - l.avail_w).max(0.0));
        let content_h = f64::from(PAD_COUNT) * ROW_HEIGHT;
        self.scroll_y = self
            .scroll_y
            .clamp(0.0, (content_h - l.rows.height()).max(0.0));
    }

    fn btn(&self, x: f64, w: f64) -> Rect {
        let t = self.layout().toolbar;
        Rect::new(x, t.y0 + 6.0, x + w, t.y0 + 26.0)
    }

    fn res_rect(&self, res: StepResolution) -> Rect {
        let i = match res {
            StepResolution::Eighth => 0.0,
            StepResolution::Sixteenth => 1.0,
            StepResolution::ThirtySecond => 2.0,
        };
        self.btn(self.bounds.x0 + PAD + 34.0 + i * 46.0, 42.0)
    }

    fn steps_rect(&self, steps: u16) -> Rect {
        let i = STEP_CHOICES.iter().position(|s| *s == steps).unwrap_or(0) as f64;
        self.btn(self.bounds.x0 + PAD + 224.0 + i * 36.0, 32.0)
    }

    fn swing_rect(&self) -> Rect {
        self.btn(self.bounds.x0 + PAD + 424.0, 76.0)
    }

    fn level_rect(&self, level: VelocityLevel) -> Rect {
        let i = VelocityLevel::ALL
            .iter()
            .position(|l| *l == level)
            .unwrap_or(0) as f64;
        self.btn(self.bounds.x0 + PAD + 540.0 + i * 44.0, 40.0)
    }

    fn new_pattern_rect(&self) -> Rect {
        let c = self.bounds.center();
        Rect::new(c.x - 60.0, c.y + 14.0, c.x + 60.0, c.y + 38.0)
    }

    fn pad_at_y(&self, y: f64) -> Option<u8> {
        let l = self.layout();
        if y < l.rows.y0 || y >= l.rows.y1 {
            return None;
        }
        let row = ((y - l.rows.y0 + self.scroll_y) / ROW_HEIGHT).floor();
        (row >= 0.0 && row < f64::from(PAD_COUNT)).then(|| PAD_COUNT - row as u8)
    }

    /// The step column under a logical x, clamped into the grid.
    fn step_at_x(&self, x: f64) -> u16 {
        let l = self.layout();
        let k = ((x - self.bounds.x0 - HEADER_WIDTH + self.scroll_x) / l.cell_w).floor();
        k.clamp(0.0, f64::from(self.grid().steps - 1)) as u16
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
            Hit::Close => help(l.close, "Close the step editor"),
            Hit::NewPattern => help(self.new_pattern_rect(), "Create the first pattern"),
            Hit::Resolution(r) => help(self.res_rect(r), "Step length of the grid"),
            Hit::Steps(s) => help(self.steps_rect(s), "Number of steps in the pattern"),
            Hit::Swing => help(
                self.swing_rect(),
                "Swing: drag or scroll to change, double-click to reset",
            ),
            Hit::Level(lv) => help(
                self.level_rect(lv),
                match lv {
                    VelocityLevel::Soft => "New steps are soft",
                    VelocityLevel::Medium => "New steps are medium",
                    VelocityLevel::Hard => "New steps are hard",
                },
            ),
            Hit::None | Hit::Header(_) | Hit::Cell(..) => None,
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
        let Some(_) = &self.clip else {
            return if self.present && self.new_pattern_rect().contains(pos) {
                Hit::NewPattern
            } else {
                Hit::None
            };
        };
        if l.toolbar.contains(pos) {
            for r in [
                StepResolution::Eighth,
                StepResolution::Sixteenth,
                StepResolution::ThirtySecond,
            ] {
                if self.res_rect(r).contains(pos) {
                    return Hit::Resolution(r);
                }
            }
            for s in STEP_CHOICES {
                if self.steps_rect(s).contains(pos) {
                    return Hit::Steps(s);
                }
            }
            if self.swing_rect().contains(pos) {
                return Hit::Swing;
            }
            for lv in VelocityLevel::ALL {
                if self.level_rect(lv).contains(pos) {
                    return Hit::Level(lv);
                }
            }
            return Hit::None;
        }
        if l.rows.contains(pos)
            && let Some(pad) = self.pad_at_y(pos.y)
        {
            if pos.x < self.bounds.x0 + HEADER_WIDTH {
                return Hit::Header(pad);
            }
            let grid_end = self.bounds.x0 + HEADER_WIDTH + l.avail_w;
            if pos.x < grid_end {
                let cell = self.step_at_x(pos.x);
                let r = self.cell_rect(pad, cell);
                if r.contains(pos) {
                    return Hit::Cell(pad, cell);
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

    fn content(&self, notes: Vec<Note>, merge: Option<u64>) -> Option<UiAction> {
        Some(UiAction::Command(Command::Song(
            SongCommand::SetClipContent {
                id: self.clip_id?,
                content: ClipContent::Notes(notes),
                merge,
            },
        )))
    }

    fn audition(&self, key: u8, on: bool) -> Option<UiAction> {
        Some(UiAction::AuditionVelocity {
            device: self.device.clone()?,
            key,
            velocity: AUDITION_VELOCITY,
            on,
        })
    }

    fn set_grid(&self, grid: StepGrid, merge: Option<u64>) -> Option<UiAction> {
        Some(UiAction::Command(Command::Song(SongCommand::SetClipGrid {
            clip: self.clip_id?,
            grid: Some(grid),
            merge,
        })))
    }

    /// One `Batch` that sets `grid` and, when needed, the clip length (`steps * s`).
    fn grid_batch(&self, grid: StepGrid) -> Option<UiAction> {
        let clip = self.clip.as_ref()?;
        let mut cmds = vec![Command::Song(SongCommand::SetClipGrid {
            clip: clip.id,
            grid: Some(grid),
            merge: None,
        })];
        let length = i64::from(grid.steps) * grid.resolution.ticks();
        if length != clip.length {
            cmds.push(Command::Song(SongCommand::SetClipGeometry {
                id: clip.id,
                start: clip.start,
                length,
                offset: clip.offset,
                loop_length: clip.loop_length,
                muted: clip.muted,
                merge: None,
            }));
        }
        let unchanged = clip.grid == Some(grid) && cmds.len() == 1;
        (!unchanged).then_some(UiAction::Command(Command::Batch(cmds)))
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
                let hover = if paths.is_empty() {
                    None
                } else {
                    self.drop_target(*pos).map(|pad_key| DropHover {
                        pad_key,
                        count: paths.len(),
                    })
                };
                self.set_drop(hover);
            }
            UiEvent::FileHoverCancel => self.set_drop(None),
            UiEvent::FileDrop { pos, paths } => {
                let target = self.drop_target(*pos);
                self.set_drop(None);
                if let (Some(at_key), Some(device)) = (target, self.device.clone())
                    && !paths.is_empty()
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

    /// `Some(at_key)` when files dropped at `pos` go to this editor: the pad key over a row
    /// header, `None` elsewhere in the pane.
    fn drop_target(&self, pos: Point) -> Option<Option<u8>> {
        if !self.bounds.contains(pos) || !self.present {
            return None;
        }
        let key = match self.hit(pos) {
            Hit::Header(pad) => Some(pad_key(pad)),
            _ => None,
        };
        Some(key)
    }

    fn set_drop(&mut self, hover: Option<DropHover>) {
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
        let g = self.grid();
        match self.hit(pos) {
            Hit::Close => out.push(UiAction::CloseStepEditor),
            Hit::NewPattern => {
                if let Some(device) = self.device.clone() {
                    out.push(UiAction::CreateStepClip { device });
                }
            }
            Hit::Resolution(resolution) => {
                out.extend(self.grid_batch(StepGrid { resolution, ..g }));
            }
            Hit::Steps(steps) => out.extend(self.grid_batch(StepGrid { steps, ..g })),
            Hit::Level(level) => self.set_default_level(level),
            Hit::Swing => {
                if clicks >= 2 {
                    if g.swing != 0 || !self.has_grid() {
                        out.extend(self.set_grid(StepGrid { swing: 0, ..g }, None));
                    }
                } else {
                    let merge = self.merge_id();
                    self.drag = Drag::Swing {
                        start_x: pos.x,
                        v0: f64::from(g.swing),
                        grid: g,
                        merge,
                        last: g.swing,
                        emitted: false,
                    };
                }
            }
            Hit::Header(pad) => {
                out.extend(self.audition(pad_key(pad), true));
                self.drag = Drag::Audition(pad);
            }
            Hit::Cell(pad, step) => self.cell_down(pad, step, pos, out),
            Hit::None => {}
        }
    }

    fn cell_down(&mut self, pad: u8, step: u16, pos: Point, out: &mut Vec<UiAction>) {
        let s = self.step_ticks();
        let key = pad_key(pad);
        let base = self.notes().to_vec();
        let v0 = self.cell_velocity(pad, step);
        let merge = self.merge_id();
        let mut gesture = Gesture {
            pad,
            mode: Mode::Pending,
            merge,
            start: pos,
            step0: step,
            last_step: step,
            v0: v0.unwrap_or(0.0),
            last: base.clone(),
            base,
            s,
            emitted: false,
        };
        if v0.is_none() {
            gesture.mode = Mode::Add;
            let list = with_step(&gesture.last, key, step, s, self.level.velocity());
            gesture.last.clone_from(&list);
            gesture.emitted = true;
            out.extend(self.content(list, Some(merge)));
        }
        self.drag = Drag::Cells(Box::new(gesture));
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
        let level = self.level.velocity();
        let mut emit: Option<(Vec<Note>, u64)> = None;
        let mut swing: Option<(StepGrid, u64)> = None;
        match &mut self.drag {
            Drag::None | Drag::Audition(_) => {}
            Drag::Swing {
                start_x,
                v0,
                grid,
                merge,
                last,
                emitted,
            } => {
                let k = if modifiers.shift { FINE_FACTOR } else { 1.0 };
                let v = (*v0 + (pos.x - *start_x) * k)
                    .round()
                    .clamp(0.0, f64::from(SWING_MAX)) as u8;
                if v != *last {
                    *last = v;
                    *emitted = true;
                    swing = Some((StepGrid { swing: v, ..*grid }, *merge));
                }
            }
            Drag::Cells(g) => {
                let key = pad_key(g.pad);
                let before = g.last.clone();
                if g.mode == Mode::Pending {
                    if step_here != g.step0 {
                        g.mode = Mode::Erase;
                        g.last = without_step(&g.last, key, g.step0, g.s);
                        g.emitted = true;
                    } else if (pos.y - g.start.y).abs() >= DRAG_THRESHOLD {
                        g.mode = Mode::Velocity;
                    }
                }
                match g.mode {
                    Mode::Add | Mode::Erase => {
                        let (a, b) = (g.last_step.min(step_here), g.last_step.max(step_here));
                        let order: Vec<u16> = if g.last_step <= step_here {
                            (a..=b).collect()
                        } else {
                            (a..=b).rev().collect()
                        };
                        for k in order {
                            g.last = if g.mode == Mode::Add {
                                if cell_has(&g.last, key, k, g.s) {
                                    g.last.clone()
                                } else {
                                    with_step(&g.last, key, k, g.s, level)
                                }
                            } else {
                                without_step(&g.last, key, k, g.s)
                            };
                        }
                        g.last_step = step_here;
                    }
                    Mode::Velocity => {
                        let v = (f64::from(g.v0) + (g.start.y - pos.y) / VELOCITY_DRAG_PIXELS)
                            .clamp(f64::from(seq_edit::MIN_VELOCITY), 1.0)
                            as f32;
                        g.last = with_velocity(&g.base, key, g.step0, g.s, v);
                    }
                    Mode::Pending => {}
                }
                if g.last != before {
                    g.emitted = true;
                    emit = Some((g.last.clone(), g.merge));
                }
            }
        }
        if let Some((notes, merge)) = emit {
            out.extend(self.content(notes, Some(merge)));
        }
        if let Some((grid, merge)) = swing {
            out.extend(self.set_grid(grid, Some(merge)));
        }
    }

    fn pointer_up(&mut self, out: &mut Vec<UiAction>) {
        self.repaint = true;
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None => {}
            Drag::Audition(pad) => out.extend(self.audition(pad_key(pad), false)),
            Drag::Swing { emitted, .. } => {
                if emitted {
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Cells(g) => {
                let mut emitted = g.emitted;
                if g.mode == Mode::Pending {
                    // A click on an on cell removes it.
                    let list = without_step(&g.base, pad_key(g.pad), g.step0, g.s);
                    out.extend(self.content(list, Some(g.merge)));
                    emitted = true;
                }
                if emitted {
                    out.push(UiAction::EndMerge);
                }
            }
        }
    }

    fn cancel_drag(&mut self, out: &mut Vec<UiAction>) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None => {}
            Drag::Audition(pad) => out.extend(self.audition(pad_key(pad), false)),
            Drag::Swing {
                grid,
                v0,
                merge,
                emitted,
                ..
            } => {
                if emitted {
                    out.extend(self.set_grid(
                        StepGrid {
                            swing: v0 as u8,
                            ..grid
                        },
                        Some(merge),
                    ));
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Cells(g) => {
                if g.emitted {
                    out.extend(self.content(g.base, Some(g.merge)));
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
        if self.clip.is_none() {
            return;
        }
        if self.hit(pos) == Hit::Swing {
            let g = self.grid();
            let step = (dy.signum() * dy.abs().ceil()) as i32;
            let swing = (i32::from(g.swing) + step).clamp(0, i32::from(SWING_MAX)) as u8;
            if swing != g.swing {
                out.extend(self.set_grid(StepGrid { swing, ..g }, None));
            }
            return;
        }
        let l = self.layout();
        if l.rows.contains(pos) || l.ruler.contains(pos) {
            if modifiers.shift || dx.abs() > 1e-9 {
                let d = if dx.abs() > 1e-9 { dx } else { dy };
                self.scroll_x -= d * SCROLL_STEP;
            } else {
                self.scroll_y -= dy * SCROLL_STEP;
            }
        }
        self.clamp_scrolls();
    }

    /// Runs a registry command the shell offers to this view. `editor.toggle_note_mute`
    /// toggles the notes of the cell under the pointer (the grid has no selection): all muted
    /// becomes unmuted, else all become muted; one `SetClipContent`. No on cell under the
    /// pointer is a quiet no-op; any other command is declined.
    pub fn command(
        &mut self,
        id: &str,
        _arg: Option<&str>,
        doc: &RackDoc,
        registry: &Registry,
    ) -> CommandOutcome {
        if id != "editor.toggle_note_mute" {
            return CommandOutcome::Declined;
        }
        self.sync(doc, registry);
        let Hit::Cell(pad, step) = self.hover else {
            return CommandOutcome::Handled(Vec::new());
        };
        if self.cell_velocity(pad, step).is_none() {
            return CommandOutcome::Handled(Vec::new());
        }
        let (key, s) = (pad_key(pad), self.step_ticks());
        let muted = !self.cell_muted(pad, step);
        let notes: Vec<Note> = self
            .notes()
            .iter()
            .map(|n| {
                if n.key == key && in_cell(n.tick, step, s) {
                    Note { muted, ..*n }
                } else {
                    *n
                }
            })
            .collect();
        self.repaint = true;
        CommandOutcome::Handled(self.content(notes, None).into_iter().collect())
    }

    fn key(&mut self, key: &Key, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        self.repaint = true;
        // Command chords (undo, redo, clipboard) are registry commands the shell resolves;
        // what no command took is not a key of the view.
        if modifiers.command() {
            return;
        }
        if *key == Key::Escape {
            if matches!(self.drag, Drag::None) {
                out.push(UiAction::CloseStepEditor);
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

    /// The pads (1..=16) whose rows have notes, for tests.
    pub fn pads_with_steps(&self) -> BTreeSet<u8> {
        self.notes()
            .iter()
            .filter_map(|n| pad_of_key(n.key))
            .collect()
    }
}

/// Whether a note starting at `tick` lies in step `step` of length `s`.
fn in_cell(tick: i64, step: u16, s: i64) -> bool {
    let lo = i64::from(step) * s;
    tick >= lo && tick < lo + s
}

fn cell_has(notes: &[Note], key: u8, step: u16, s: i64) -> bool {
    notes
        .iter()
        .any(|n| n.key == key && in_cell(n.tick, step, s))
}

/// `notes` plus one note at the start of `step` (length one step), sorted like the piano roll.
fn with_step(notes: &[Note], key: u8, step: u16, s: i64, velocity: f32) -> Vec<Note> {
    let mut list = notes.to_vec();
    list.push(Note::new(i64::from(step) * s, s, key, velocity));
    seq_edit::normalize_notes(list, &BTreeSet::new()).0
}

/// `notes` without the notes of `key` that start in `step`.
fn without_step(notes: &[Note], key: u8, step: u16, s: i64) -> Vec<Note> {
    notes
        .iter()
        .copied()
        .filter(|n| !(n.key == key && in_cell(n.tick, step, s)))
        .collect()
}

/// `notes` with the velocity of the notes of `key` in `step` set to `v`.
fn with_velocity(notes: &[Note], key: u8, step: u16, s: i64, v: f32) -> Vec<Note> {
    notes
        .iter()
        .map(|n| {
            if n.key == key && in_cell(n.tick, step, s) {
                Note { velocity: v, ..*n }
            } else {
                *n
            }
        })
        .collect()
}
