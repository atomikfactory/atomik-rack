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
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::keymap::{CommandOutcome, Roles};
use crate::paint::fit_label;
use crate::seq_edit::{self, Selection, SpanPart};
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::{Theme, mix};
use crate::timeline::{Meter, PPQ, Snap, TimeAxis, snap_floor, snap_round};
use crate::viewport::{Tool, ViewportState};
use atomik_device_sdk::Registry;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Note, RackDoc, SongCommand, WorkflowCommand,
};
use std::collections::BTreeSet;
use vello::kurbo::{Affine, Point, Rect, RoundedRect};
use vello::peniko::Color;

/// Height of the top bar (title, snap, quantize, close).
pub const TOP_BAR_HEIGHT: f64 = 28.0;
/// Height of the ruler.
pub const ROLL_RULER_HEIGHT: f64 = 20.0;
/// Width of the keyboard strip.
pub const KEYBOARD_WIDTH: f64 = 56.0;
/// Height of the velocity lane.
pub const VELOCITY_HEIGHT: f64 = 76.0;
/// Height of one key row.
pub const KEY_HEIGHT: f64 = 12.0;
const VEL_PAD: f64 = 8.0;
const EDGE_PX: f64 = 5.0;
const DRAG_THRESHOLD: f64 = 3.0;
const SCROLL_STEP: f64 = 36.0;
const ZOOM_STEP: f64 = 1.15;
/// Zoom factor of the zoom commands (H and G).
const ZOOM_KEY: f64 = 1.25;
const MERGE_BASE: u64 = 2 << 40;
const DEFAULT_VELOCITY: f32 = 0.8;
/// Note length / minimum with snapping off (a sixteenth / a 64th).
const OFF_LENGTH: i64 = PPQ / 4;
const MIN_TICKS: i64 = PPQ / 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopItem {
    Snap,
    Quantize,
    Close,
    Tool(Tool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Top(Option<TopItem>),
    Ruler,
    Keys(u8),
    Note(usize, SpanPart),
    Grid,
    Velocity(Option<usize>),
    None,
}

#[derive(Debug, Clone)]
enum Drag {
    None,
    Draw {
        start: Point,
        base: Vec<Note>,
        note: Note,
        merge: u64,
        last: Vec<Note>,
    },
    Move {
        start: Point,
        originals: Vec<Note>,
        sel: BTreeSet<usize>,
        grabbed: usize,
        anchor_key: i32,
        merge: u64,
        moved: bool,
        emitted: bool,
        last: Vec<Note>,
        sounding: u8,
        reselect: Option<usize>,
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
    Velocity {
        originals: Vec<Note>,
        sel: BTreeSet<usize>,
        grabbed: usize,
        merge: u64,
        last: Vec<Note>,
    },
    Band {
        start: Point,
        pos: Point,
        base: BTreeSet<usize>,
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
    bounds: Rect,
    scale: f64,
    axis: TimeAxis,
    scroll_y: f64,
    snap: Snap,
    vp: ViewportState,
    strength: f32,
    clip_id: Option<ClipId>,
    clip: Option<Clip>,
    device: Option<DeviceKey>,
    title: String,
    accent: Color,
    meter: Meter,
    fit_pending: bool,
    selection: Selection<usize>,
    drag: Drag,
    hover: Hit,
    next_merge: u64,
    last_length: Option<i64>,
    playhead: i64,
    focused: bool,
    repaint: bool,
}

impl PianoRollView {
    /// A closed piano roll; call [`Self::set_bounds`], [`Self::open`] and [`Self::sync`].
    pub fn new(theme: Theme) -> Self {
        let st = SeqTheme::from_theme(&theme);
        let accent = st.base.highlight;
        Self {
            st,
            bounds: Rect::new(0.0, 0.0, 800.0, 400.0),
            scale: 1.0,
            axis: TimeAxis::new(KEYBOARD_WIDTH, 200.0),
            scroll_y: 0.0,
            snap: Snap::default(),
            vp: ViewportState::new(),
            strength: 1.0,
            clip_id: None,
            clip: None,
            device: None,
            title: String::new(),
            accent,
            meter: Meter::default(),
            fit_pending: false,
            selection: Selection::new(),
            drag: Drag::None,
            hover: Hit::None,
            next_merge: MERGE_BASE,
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
        self.drag = Drag::None;
        self.fit_pending = true;
        self.repaint = true;
    }

    /// Closes the roll (it then ignores input and paints an empty state).
    pub fn close(&mut self) {
        self.clip_id = None;
        self.clip = None;
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
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.axis.x0 = bounds.x0 + KEYBOARD_WIDTH;
            self.clamp_scroll();
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

    /// Sets the transport position (song ticks), drawn where the clip plays it.
    pub fn set_playhead(&mut self, tick: i64) {
        if tick != self.playhead {
            self.playhead = tick;
            self.repaint = true;
            if self.vp.following()
                && let Some(c) = self
                    .clip
                    .as_ref()
                    .and_then(|cl| seq_edit::song_to_content(cl, tick))
            {
                self.axis.page_to(c, self.bounds.x1);
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
        if self.vp.set_tool(tool) {
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

    /// Quantize strength (0..=1, default 1).
    pub fn quantize_strength(&self) -> f32 {
        self.strength
    }

    /// Sets the quantize strength (clamped to 0..=1).
    pub fn set_quantize_strength(&mut self, strength: f32) {
        self.strength = if strength.is_nan() {
            1.0
        } else {
            strength.clamp(0.0, 1.0)
        };
        self.repaint = true;
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

    /// Vertical centre of a key row.
    pub fn key_center_y(&self, key: u8) -> f64 {
        self.key_top(key) + KEY_HEIGHT / 2.0
    }

    /// The key at a logical y (clamped to 0..=127).
    pub fn key_at(&self, y: f64) -> u8 {
        let k = 127.0 - ((y - self.grid_rect().y0 + self.scroll_y) / KEY_HEIGHT).floor();
        k.clamp(0.0, 127.0) as u8
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
        Rect::new(self.axis.x0, (b.y1 - VELOCITY_HEIGHT).max(b.y0), b.x1, b.y1)
    }

    /// The y of the ruler (click to locate).
    pub fn ruler_y(&self) -> f64 {
        self.bounds.y0 + TOP_BAR_HEIGHT + ROLL_RULER_HEIGHT / 2.0
    }

    /// Centre of the close button.
    pub fn close_button_center(&self) -> Point {
        self.top_rect(TopItem::Close).center()
    }

    /// Centre of the quantize button.
    pub fn quantize_button_center(&self) -> Point {
        self.top_rect(TopItem::Quantize).center()
    }

    /// Centre of the snap selector.
    pub fn snap_button_center(&self) -> Point {
        self.top_rect(TopItem::Snap).center()
    }

    // ------------------------------------------------------------------ sync

    /// Reads the open clip from the document. Emits nothing.
    pub fn sync(&mut self, doc: &RackDoc, _registry: &Registry) {
        let Some(id) = self.clip_id else {
            return;
        };
        let found = doc.song.tracks.iter().find_map(|t| {
            t.lanes
                .iter()
                .find_map(|l| l.clips.iter().find(|c| c.id == id))
                .map(|c| (t, c))
        });
        let (clip, device) = match found {
            Some((t, c)) => (Some(c.clone()), Some(t.device.clone())),
            None => (None, None),
        };
        let meter = Meter::of(&doc.song);
        if clip != self.clip || device != self.device || meter != self.meter {
            self.clip = clip;
            self.device = device;
            self.meter = meter;
            let n = self.notes().len();
            self.selection.retain(|i| *i < n);
            self.repaint = true;
        }
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
        self.scroll_y = (127.0 - mid) * KEY_HEIGHT + KEY_HEIGHT / 2.0 - g.height() / 2.0;
        self.clamp_scroll();
    }

    // ------------------------------------------------------------------ geometry

    fn notes(&self) -> &[Note] {
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
        let y0 = b.y0 + TOP_BAR_HEIGHT + ROLL_RULER_HEIGHT;
        let y1 = (b.y1 - VELOCITY_HEIGHT).max(y0);
        Rect::new(self.axis.x0, y0, b.x1.max(self.axis.x0), y1)
    }

    fn velocity_range(&self) -> (f64, f64) {
        let r = self.velocity_rect();
        (r.y0 + VEL_PAD, (r.height() - 2.0 * VEL_PAD).max(1.0))
    }

    fn key_top(&self, key: u8) -> f64 {
        self.grid_rect().y0 + (127.0 - f64::from(key)) * KEY_HEIGHT - self.scroll_y
    }

    fn rect_of(&self, n: &Note) -> Rect {
        let y = self.key_top(n.key);
        Rect::new(
            self.axis.tick_to_x(n.tick),
            y,
            self.axis.tick_to_x(n.tick + n.length),
            y + KEY_HEIGHT,
        )
    }

    fn clamp_scroll(&mut self) {
        let max = (128.0 * KEY_HEIGHT - self.grid_rect().height()).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max);
    }

    fn top_rect(&self, item: TopItem) -> Rect {
        let b = self.bounds;
        let y = b.y0 + 5.0;
        match item {
            TopItem::Close => Rect::new(b.x1 - 30.0, y, b.x1 - 8.0, y + 18.0),
            TopItem::Quantize => Rect::new(b.x1 - 118.0, y, b.x1 - 40.0, y + 18.0),
            TopItem::Snap => Rect::new(b.x1 - 186.0, y, b.x1 - 132.0, y + 18.0),
            TopItem::Tool(t) => {
                let i = Tool::ALL.iter().position(|x| *x == t).unwrap_or(0) as f64;
                let x = b.x1 - 294.0 + i * 25.0;
                Rect::new(x, y, x + 23.0, y + 18.0)
            }
        }
    }

    fn step(&self, modifiers: Modifiers) -> Option<i64> {
        self.vp.step(self.snap, self.meter, modifiers)
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
        if pos.y < b.y0 + TOP_BAR_HEIGHT {
            let item = [TopItem::Snap, TopItem::Quantize, TopItem::Close]
                .into_iter()
                .chain(Tool::ALL.into_iter().map(TopItem::Tool))
                .find(|i| self.top_rect(*i).contains(pos));
            return Hit::Top(item);
        }
        let g = self.grid_rect();
        if pos.y < g.y0 {
            return if pos.x >= g.x0 { Hit::Ruler } else { Hit::None };
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
                && let Some(part) = seq_edit::hit_span(pos.x, r.x0, r.x1.max(r.x0 + 3.0), EDGE_PX)
            {
                return Hit::Note(i, part);
            }
        }
        Hit::Grid
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
                content: ClipContent::Notes(notes),
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

    // ------------------------------------------------------------------ events

    /// Handles one event and returns what the app must do. Calls [`Self::sync`] first.
    pub fn handle(&mut self, event: &UiEvent, doc: &RackDoc, registry: &Registry) -> Vec<UiAction> {
        self.sync(doc, registry);
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
        match event {
            UiEvent::PointerMove { pos, modifiers } => {
                self.pointer_move(*pos, *modifiers, &mut out)
            }
            UiEvent::PointerDown {
                pos,
                button,
                modifiers,
                ..
            } => self.pointer_down(*pos, *button, *modifiers, &mut out),
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
                if modifiers.command() {
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
        match self.hit(pos) {
            Hit::Top(Some(TopItem::Tool(t))) if primary => self.set_tool(t),
            Hit::Top(Some(TopItem::Snap)) => self.snap = self.snap.cycle(primary),
            Hit::Top(Some(TopItem::Quantize)) if primary => self.quantize(out),
            Hit::Top(Some(TopItem::Close)) if primary => out.push(UiAction::ClosePianoRoll),
            Hit::Ruler if primary => {
                let c = snap_round(self.axis.x_to_tick(pos.x), step);
                let t = self.content_to_song(c).max(0);
                out.push(UiAction::Transport(TransportAction::Locate(t)));
            }
            Hit::Keys(key) if primary => {
                out.extend(self.audition(key, true));
                self.drag = Drag::Keys { key };
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
            Hit::Grid if primary => {
                if tool == Tool::Select && (modifiers.shift || modifiers.command()) {
                    let base = if modifiers.command() {
                        self.selection.items().clone()
                    } else {
                        BTreeSet::new()
                    };
                    self.drag = Drag::Band {
                        start: pos,
                        pos,
                        base,
                    };
                    return;
                }
                let tick = snap_floor(self.axis.x_to_tick(pos.x), step).max(0);
                let length = step.or(self.last_length).unwrap_or(OFF_LENGTH).max(1);
                let note = seq_edit::clamp_note(Note {
                    tick,
                    length,
                    key: self.key_at(pos.y),
                    velocity: DEFAULT_VELOCITY,
                });
                let base = self.notes().to_vec();
                let merge = self.merge_id();
                let (list, sel) = with_note(&base, note);
                self.selection.set(sel);
                out.extend(self.content(list.clone(), Some(merge)));
                out.extend(self.audition(note.key, true));
                self.drag = Drag::Draw {
                    start: pos,
                    base,
                    note,
                    merge,
                    last: list,
                };
            }
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
            _ => {}
        }
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
        let toggle = modifiers.command();
        self.selection.click(i, toggle, modifiers.shift && !toggle);
        if !self.selection.contains(&i) {
            return;
        }
        let originals = self.notes().to_vec();
        let sel = self.selection.items().clone();
        let merge = self.merge_id();
        let key = originals[i].key;
        match part {
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
                    anchor_key: i32::from(self.key_at(pos.y)),
                    merge,
                    moved: false,
                    emitted: false,
                    sounding: key,
                    reselect,
                };
            }
        }
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
        let step = self.step(modifiers);
        let min = seq_edit::min_length(step, MIN_TICKS);
        let axis = self.axis;
        let key_here = self.key_at(pos.y);
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
                    content: ClipContent::Notes(notes),
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
            Drag::None => {}
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
                note,
                merge,
                last,
            } => {
                let delta = axis.dx_to_ticks(pos.x - start.x);
                let len =
                    seq_edit::drag_length(note.tick, note.tick + note.length, delta, step, min);
                let (list, sel) = with_note(
                    base,
                    Note {
                        length: len,
                        ..*note
                    },
                );
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
                anchor_key,
                merge,
                moved,
                emitted,
                last,
                sounding,
                ..
            } => {
                *moved |= pos.distance(*start) > DRAG_THRESHOLD;
                if !*moved {
                    return;
                }
                let g = originals[*grabbed];
                let delta = axis.dx_to_ticks(pos.x - start.x);
                let dt = seq_edit::drag_start(g.tick, delta, step) - g.tick;
                let dkey = i32::from(key_here) - *anchor_key;
                let moved_notes = seq_edit::move_notes(originals, sel, dt, dkey);
                let now_key = moved_notes[*grabbed].key;
                let mut tracked = BTreeSet::new();
                tracked.insert(*grabbed);
                tracked.extend(sel.iter().copied());
                let (list, new) = seq_edit::normalize_notes(moved_notes, sel);
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
        }
        if let Some(s) = new_sel {
            self.selection.set(s);
        }
    }

    fn pointer_up(&mut self, out: &mut Vec<UiAction>) {
        self.repaint = true;
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None | Drag::Pan { .. } | Drag::Band { .. } => {}
            Drag::Keys { key } => out.extend(self.audition(key, false)),
            Drag::Draw { note, .. } => {
                out.extend(self.audition(note.key, false));
                out.push(UiAction::EndMerge);
            }
            Drag::Move {
                sounding,
                emitted,
                moved,
                reselect,
                ..
            } => {
                out.extend(self.audition(sounding, false));
                if emitted {
                    out.push(UiAction::EndMerge);
                }
                if !moved && let Some(i) = reselect {
                    self.selection.select_only(i);
                }
            }
            Drag::Resize { emitted, .. } => {
                if emitted {
                    out.push(UiAction::EndMerge);
                }
            }
            Drag::Velocity { .. } => out.push(UiAction::EndMerge),
        }
    }

    fn quantize(&mut self, out: &mut Vec<UiAction>) {
        let notes = self.notes().to_vec();
        if notes.is_empty() {
            return;
        }
        let sel: BTreeSet<usize> = if self.selection.is_empty() {
            (0..notes.len()).collect()
        } else {
            self.selection.items().clone()
        };
        let step = self.snap.step(self.meter).or(Some(OFF_LENGTH));
        let q = seq_edit::quantize_notes(&notes, &sel, step, self.strength);
        let (list, new) = seq_edit::normalize_notes(q, &sel);
        if list != notes {
            if !self.selection.is_empty() {
                self.selection.set(new);
            }
            out.extend(self.content(list, None));
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
        self.sync(doc, registry);
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
        if let Some(tool) = Tool::from_command(id) {
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
            "editor.transpose_up" => self.move_selection(0, 1),
            "editor.transpose_down" => self.move_selection(0, -1),
            "editor.octave_up" => self.move_selection(0, 12),
            "editor.octave_down" => self.move_selection(0, -12),
            "editor.nudge_left" => self.move_selection(-step, 0),
            "editor.nudge_right" => self.move_selection(step, 0),
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
        let mut out = Vec::new();
        if list != self.notes() {
            self.selection.set(new);
            out.extend(self.content(list, None));
        }
        CommandOutcome::Handled(out)
    }

    fn cancel_drag(&mut self, out: &mut Vec<UiAction>) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Keys { key } => out.extend(self.audition(key, false)),
            Drag::Draw {
                base, note, merge, ..
            } => {
                out.extend(self.audition(note.key, false));
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
            } => {
                out.extend(self.content(originals, Some(merge)));
                out.push(UiAction::EndMerge);
                self.selection.set(sel);
            }
            Drag::Velocity {
                originals, merge, ..
            } => {
                out.extend(self.content(originals, Some(merge)));
                out.push(UiAction::EndMerge);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ painting

    /// Paints the roll into its bounds (physical pixels, scaled like the other views).
    pub fn paint(&mut self, canvas: &mut dyn Canvas, doc: &RackDoc, registry: &Registry) {
        self.sync(doc, registry);
        let b = self.bounds;
        let c = canvas;
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&b.into());
        sp::rect(c, b, self.st.background);
        if self.clip.is_some() {
            self.paint_grid(c);
            self.paint_ruler(c);
            self.paint_keyboard(c);
            self.paint_velocity(c);
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
        let k_hi = self.key_at(g.y0);
        let k_lo = self.key_at(g.y1 - 0.01);
        for k in k_lo..=k_hi {
            let y = self.key_top(k);
            let row = Rect::new(g.x0, y, g.x1, y + KEY_HEIGHT);
            if is_black(k) {
                sp::rect(c, row, st.row_black);
            }
            let col = if k % 12 == 0 {
                st.grid_bar
            } else {
                st.grid_sub
            };
            sp::line(c, g.x0, row.y1 - 0.5, g.x1, row.y1 - 0.5, col);
        }
        sp::grid(c, st, &self.axis, g, self.meter);
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
            let base = if selected { st.selection } else { self.accent };
            let fill = mix(mix(base, st.lane, 0.45), base, n.velocity);
            let rr = RoundedRect::from_rect(Rect::new(r.x0, r.y0, r.x1.max(r.x0 + 2.0), r.y1), 2.0);
            c.fill_with(rr, fill);
            let (w, col) = if selected {
                (1.5, st.base.text)
            } else {
                (1.0, st.base.bevel_dark)
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
        sp::ruler_labels(c, st, &self.axis, r, self.meter);
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
        let sounding = match &self.drag {
            Drag::Keys { key } => Some(*key),
            Drag::Move { sounding, .. } => Some(*sounding),
            Drag::Draw { note, .. } => Some(note.key),
            _ => None,
        };
        let k_hi = self.key_at(g.y0);
        let k_lo = self.key_at(g.y1 - 0.01);
        for k in k_lo..=k_hi {
            let y = self.key_top(k);
            let row = Rect::new(kb.x0, y, kb.x1, y + KEY_HEIGHT);
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
                let col = if sounding == Some(k) {
                    self.accent
                } else {
                    st.key_black
                };
                c.fill_with(RoundedRect::from_rect(r, 1.5), col);
            } else {
                if sounding == Some(k) {
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
            if k % 12 == 0 {
                sp::line(c, kb.x0, row.y1 - 0.5, kb.x1, row.y1 - 0.5, t.knob_bottom);
                let name = format!("C{}", i32::from(k) / 12 - 1);
                sp::label(
                    c,
                    &name,
                    kb.x1 - 4.0,
                    row.y1 - 3.0,
                    8.0,
                    t.plate,
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
            let col = if selected { st.selection } else { self.accent };
            let y = top + (1.0 - f64::from(n.velocity)) * h;
            let bottom = top + h;
            c.fill_with(Rect::new(x - 1.0, y, x + 1.0, bottom), col.with_alpha(0.85));
            c.fill_with(Rect::new(x - 3.0, y - 1.5, x + 3.0, y + 1.5), col);
        }
        sp::line(c, vr.x0, vr.y0 + 0.5, vr.x1, vr.y0 + 0.5, st.separator);
        c.pop_clip();
        sp::line(c, side.x0, vr.y0 + 0.5, side.x1, vr.y0 + 0.5, st.separator);
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
        for tool in Tool::ALL {
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
        let q = format!("QUANT {:.0}%", self.strength * 100.0);
        sp::label(
            c,
            &q,
            qr.center().x,
            qr.center().y + 3.5,
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

/// `base` plus `note`, normalized; the returned selection holds the new note's index.
fn with_note(base: &[Note], note: Note) -> (Vec<Note>, BTreeSet<usize>) {
    let mut list = base.to_vec();
    list.push(note);
    let tracked: BTreeSet<usize> = [base.len()].into_iter().collect();
    seq_edit::normalize_notes(list, &tracked)
}

fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}
