//! The Launcher view: a grid of clip slots to launch live, one column per note track and one row
//! per scene.
//!
//! * **Columns** are the song's note tracks in the arrangement's order, in their colours. A slot
//!   shows its clip's name and colour, a tiny overview of its notes and a play triangle. A track
//!   that plays a slot shows a stop square in its empty slots and a progress bar along the
//!   playing slot; a slot that waits for its boundary is outlined and blinks.
//! * **Scenes** are the rows. The scene column on the right has a launch triangle and the
//!   scene's name (double click renames it). The row under the grid has a stop square per track
//!   and STOP ALL in the scene column.
//! * **The bar on top** has BACK TO ARRANGEMENT (lit while any track plays a slot instead of its
//!   arrangement), RECORD LAUNCHES, SCENE STOPS EMPTY and the song's launch quantize.
//! * **Mouse.** The triangle launches, the stop square stops. A click elsewhere on a slot moves
//!   the cursor; a double click opens the clip in the piano roll, or makes an empty one (one bar)
//!   and opens it. A drag moves a slot (Ctrl copies). Right click opens a list: rename, colour,
//!   loop length, quantize, duplicate and delete for a slot; insert, duplicate, delete for a
//!   scene.
//! * **Keyboard.** The arrows move the cursor, Enter launches the slot under it, Shift+Enter
//!   launches its scene; Delete, Cut, Copy, Paste and Duplicate (the Edit commands) act on the
//!   slot under the cursor.
//!
//! The view owns no document and no engine state: the host hands it the document
//! ([`LauncherView::sync`]) and what the engine publishes ([`LauncherView::set_feed`]), and
//! carries out the actions it returns.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt, Geom, TextRun, pt};
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::keymap::CommandOutcome;
use crate::marker_edit::palette_color;
use crate::pick_list::{PAD, PickList};
use crate::seq_paint::{SeqTheme, device_accent};
use crate::text_field::{FieldColors, FieldKey, TextField};
use crate::theme::Theme;
use crate::tooltip::Help;
use atomik_model::{
    ClipContent, ClipId, Command, DeviceKey, LaunchQuantize, LauncherClip, LauncherCommand,
    MAX_CLIP_NAME_CHARS, MetaChange, RackDoc, Song, SongCommand,
};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;
use vello::kurbo::{BezPath, Point, Rect, RoundedRect};
use vello::peniko::Color;

/// Height of the bar of buttons on top.
pub const TOOL_H: f64 = 30.0;
/// Height of the row of track names.
pub const HEAD_H: f64 = 24.0;
/// Height of a slot row.
pub const ROW_H: f64 = 44.0;
/// Width of a track column.
pub const COL_W: f64 = 116.0;
/// Width of the scene column on the right.
pub const SCENE_W: f64 = 140.0;
/// Height of the stop row under the grid.
pub const STOP_H: f64 = 30.0;
/// The fewest rows the grid shows (a song without scenes still has slots to fill).
pub const MIN_ROWS: usize = 8;
/// Width of the launch triangle's zone at the left of a slot.
const PLAY_W: f64 = 26.0;
/// How far the pointer moves before a press on a slot becomes a drag.
const DRAG_START: f64 = 5.0;
/// Notes the overview of one slot draws at most.
const OVERVIEW_NOTES: usize = 256;

/// What the engine publishes about one track.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrackLaunch {
    /// The slot (scene row) that plays.
    pub playing: Option<usize>,
    /// The slot that waits for its boundary.
    pub queued: Option<usize>,
    /// The track waits to go back to its arrangement.
    pub queued_stop: bool,
    /// Where the playing slot is in its loop, `0.0..1.0`.
    pub phase: f32,
}

/// What the host shows besides the document: the tracks' launch states, the Record Launches
/// switch and the blink phase of the slots that wait.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LauncherFeed {
    /// The launch state of the tracks that have one.
    pub tracks: BTreeMap<DeviceKey, TrackLaunch>,
    /// Record Launches is on.
    pub record: bool,
    /// The slots that wait are drawn lit (they blink).
    pub blink: bool,
}

/// What the view asks the host to do besides editing the document.
#[derive(Debug, Clone, PartialEq)]
pub enum LauncherAction {
    /// Launch the slot.
    Launch {
        /// The track.
        device: DeviceKey,
        /// The scene row.
        row: usize,
    },
    /// Launch a whole scene.
    LaunchScene(usize),
    /// Stop a track's slot (at its quantize).
    Stop(DeviceKey),
    /// Stop every slot at once: the tracks play their arrangement again now.
    Back,
    /// Stop every slot (at the launch quantize).
    StopAll,
    /// Switch Record Launches.
    ToggleRecord,
    /// Switch "scene stops empty tracks".
    ToggleStopEmpty,
    /// Set the song's launch quantize.
    SetQuantize(LaunchQuantize),
    /// Open the slot's clip in the piano roll.
    OpenClip(ClipId),
    /// Make an empty one-bar clip in the slot and open it in the piano roll.
    NewClip {
        /// The track.
        device: DeviceKey,
        /// The scene row.
        row: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct Col {
    device: DeviceKey,
    name: String,
    accent: Color,
}

/// A place the pointer can be on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spot {
    Back,
    Record,
    StopEmpty,
    Quantize,
    Head(usize),
    SlotPlay(usize, usize),
    Slot(usize, usize),
    SceneLaunch(usize),
    Scene(usize),
    Stop(usize),
    StopAll,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    from: (usize, usize),
    start: Point,
    at: Point,
    started: bool,
    copy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuTarget {
    Slot(usize, usize),
    Scene(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Root,
    Colour,
    Length,
    Quantize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Item {
    Rename,
    Colour,
    Length,
    Quantize,
    Duplicate,
    Delete,
    SetColour(&'static str),
    SetLength(i64),
    SetQuantize(Option<LaunchQuantize>),
    InsertScene,
    DuplicateScene,
    DeleteScene,
}

#[derive(Debug, Clone)]
struct Menu {
    target: MenuTarget,
    list: PickList,
    rows: Vec<(String, Item)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditTarget {
    Slot(usize, usize),
    Scene(usize),
}

#[derive(Debug, Clone)]
struct Editing {
    target: EditTarget,
    field: TextField,
}

/// The colours a slot's menu offers: the track's own, then the palette of the clip colours.
const COLOURS: &[&str] = &[
    "track", "red", "orange", "yellow", "green", "blue", "purple",
];

/// The loop lengths a slot's menu offers, in bars.
const LENGTH_BARS: [i64; 6] = [1, 2, 4, 8, 16, 32];

/// The Launcher view.
pub struct LauncherView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    focused: bool,
    song: Option<Arc<Song>>,
    cols: Vec<Col>,
    feed: LauncherFeed,
    cursor: (usize, usize),
    top_row: usize,
    left_col: usize,
    hover: Option<Spot>,
    drag: Option<Drag>,
    menu: Option<Menu>,
    edit: Option<Editing>,
    clipboard: Option<LauncherClip>,
    repaint: Cell<bool>,
}

impl LauncherView {
    /// An empty view.
    pub fn new(theme: Theme) -> Self {
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 800.0, 400.0),
            scale: 1.0,
            focused: false,
            song: None,
            cols: Vec::new(),
            feed: LauncherFeed::default(),
            cursor: (0, 0),
            top_row: 0,
            left_col: 0,
            hover: None,
            drag: None,
            menu: None,
            edit: None,
            clipboard: None,
            repaint: Cell::new(true),
        }
    }

    /// Switches to `theme` (the next paint uses it).
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint.set(true);
    }

    /// Whether something the view shows changed since the last paint.
    pub fn needs_repaint(&self) -> bool {
        self.repaint.get()
    }

    /// Where the view is, in window logical pixels.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.menu = None;
            self.reveal();
            self.repaint.set(true);
        }
    }

    /// Where the view is.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Display scale factor (the view paints in logical units).
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
    }

    /// Gives the view the keys (the cursor shows while it has them).
    pub fn set_focused(&mut self, focused: bool) {
        if focused != self.focused {
            self.focused = focused;
            self.repaint.set(true);
        }
    }

    /// Whether the view has the keys.
    pub fn focused(&self) -> bool {
        self.focused
    }

    /// Whether the view is typing a name: plain keys go to it.
    pub fn is_text_owner(&self) -> bool {
        self.edit.is_some()
    }

    /// Reads the tracks, their colours and the launcher from the document.
    pub fn sync(&mut self, doc: &RackDoc) {
        let same = self
            .song
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(s, &doc.song));
        let cols: Vec<Col> = doc
            .song
            .tracks
            .iter()
            .filter(|t| !t.is_audio() && !t.is_automation_only())
            .map(|t| Col {
                device: t.device.clone(),
                name: doc
                    .device(&t.device)
                    .map_or_else(|| t.device.0.clone(), |d| d.name.clone()),
                accent: match doc.song.track_color(&t.device) {
                    Some([r, g, b]) => Color::from_rgb8(r, g, b),
                    None => device_accent(&self.st.base, doc, &t.device),
                },
            })
            .collect();
        if same && cols == self.cols {
            return;
        }
        self.song = Some(Arc::clone(&doc.song));
        self.cols = cols;
        let rows = self.rows_total();
        self.cursor.0 = self.cursor.0.min(self.cols.len().saturating_sub(1));
        self.cursor.1 = self.cursor.1.min(rows - 1);
        self.reveal();
        self.repaint.set(true);
    }

    /// Hands the view what the engine publishes and the host's switches.
    pub fn set_feed(&mut self, feed: LauncherFeed) {
        if feed != self.feed {
            self.feed = feed;
            self.repaint.set(true);
        }
    }

    /// The feed the view shows.
    pub fn feed(&self) -> &LauncherFeed {
        &self.feed
    }

    // ------------------------------------------------------------------ data

    fn launcher(&self) -> Option<&atomik_model::Launcher> {
        self.song.as_deref().map(|s| &s.launcher)
    }

    fn rows_total(&self) -> usize {
        self.launcher()
            .map_or(MIN_ROWS, |l| l.scenes.len().max(MIN_ROWS - 1) + 1)
    }

    fn slot(&self, col: usize, row: usize) -> Option<&LauncherClip> {
        self.launcher()?.slot(&self.cols.get(col)?.device, row)
    }

    /// The slot under the cursor: its track and scene row.
    pub fn cursor_slot(&self) -> Option<(DeviceKey, usize)> {
        Some((self.cols.get(self.cursor.0)?.device.clone(), self.cursor.1))
    }

    /// The cursor: track column and scene row.
    pub fn cursor(&self) -> (usize, usize) {
        self.cursor
    }

    /// Moves the cursor (clamped) and scrolls it into view.
    pub fn set_cursor(&mut self, col: usize, row: usize) {
        self.cursor = (
            col.min(self.cols.len().saturating_sub(1)),
            row.min(self.rows_total() - 1),
        );
        self.reveal();
        self.repaint.set(true);
    }

    /// The track columns' devices, left to right.
    pub fn tracks(&self) -> Vec<DeviceKey> {
        self.cols.iter().map(|c| c.device.clone()).collect()
    }

    // ------------------------------------------------------------------ geometry

    fn grid(&self) -> Rect {
        let b = self.bounds;
        Rect::new(
            b.x0,
            b.y0 + TOOL_H + HEAD_H,
            (b.x1 - SCENE_W).max(b.x0 + 1.0),
            (b.y1 - STOP_H).max(b.y0 + TOOL_H + HEAD_H + 1.0),
        )
    }

    fn visible_cols(&self) -> usize {
        ((self.grid().width() / COL_W).floor() as usize).max(1)
    }

    fn visible_rows(&self) -> usize {
        ((self.grid().height() / ROW_H).floor() as usize).max(1)
    }

    fn reveal(&mut self) {
        let (vc, vr) = (self.visible_cols(), self.visible_rows());
        if self.cursor.0 < self.left_col {
            self.left_col = self.cursor.0;
        } else if self.cursor.0 >= self.left_col + vc {
            self.left_col = self.cursor.0 + 1 - vc;
        }
        if self.cursor.1 < self.top_row {
            self.top_row = self.cursor.1;
        } else if self.cursor.1 >= self.top_row + vr {
            self.top_row = self.cursor.1 + 1 - vr;
        }
        self.left_col = self.left_col.min(self.cols.len().saturating_sub(vc));
        self.top_row = self.top_row.min(self.rows_total().saturating_sub(vr));
    }

    fn col_x(&self, col: usize) -> Option<f64> {
        (col >= self.left_col && col < self.left_col + self.visible_cols())
            .then(|| self.bounds.x0 + (col - self.left_col) as f64 * COL_W)
    }

    fn row_y(&self, row: usize) -> Option<f64> {
        (row >= self.top_row && row < self.top_row + self.visible_rows())
            .then(|| self.grid().y0 + (row - self.top_row) as f64 * ROW_H)
    }

    /// The rectangle of slot `(col, row)`, `None` when it is scrolled out or past the tracks.
    pub fn slot_rect(&self, col: usize, row: usize) -> Option<Rect> {
        if col >= self.cols.len() || row >= self.rows_total() {
            return None;
        }
        let (x, y) = (self.col_x(col)?, self.row_y(row)?);
        Some(Rect::new(
            x + 1.0,
            y + 1.0,
            x + COL_W - 1.0,
            y + ROW_H - 1.0,
        ))
    }

    /// The play triangle's zone of a slot.
    pub fn play_rect(&self, col: usize, row: usize) -> Option<Rect> {
        let r = self.slot_rect(col, row)?;
        Some(Rect::new(r.x0, r.y0, r.x0 + PLAY_W, r.y1))
    }

    /// The launch zone of scene `row`.
    pub fn scene_launch_rect(&self, row: usize) -> Option<Rect> {
        let y = self.row_y(row)?;
        let x = self.bounds.x1 - SCENE_W;
        Some(Rect::new(x + 1.0, y + 1.0, x + PLAY_W, y + ROW_H - 1.0))
    }

    /// The name zone of scene `row`.
    pub fn scene_rect(&self, row: usize) -> Option<Rect> {
        let y = self.row_y(row)?;
        let x = self.bounds.x1 - SCENE_W;
        Some(Rect::new(
            x + PLAY_W + 1.0,
            y + 1.0,
            self.bounds.x1 - 1.0,
            y + ROW_H - 1.0,
        ))
    }

    /// The stop square of track `col`.
    pub fn stop_rect(&self, col: usize) -> Option<Rect> {
        if col >= self.cols.len() {
            return None;
        }
        let x = self.col_x(col)?;
        let y = self.bounds.y1 - STOP_H;
        Some(Rect::new(
            x + 1.0,
            y + 3.0,
            x + COL_W - 1.0,
            y + STOP_H - 3.0,
        ))
    }

    /// The STOP ALL button.
    pub fn stop_all_rect(&self) -> Rect {
        let x = self.bounds.x1 - SCENE_W;
        let y = self.bounds.y1 - STOP_H;
        Rect::new(x + 1.0, y + 3.0, self.bounds.x1 - 1.0, y + STOP_H - 3.0)
    }

    fn head_rect(&self, col: usize) -> Option<Rect> {
        let x = self.col_x(col)?;
        let y = self.bounds.y0 + TOOL_H;
        Some(Rect::new(x + 1.0, y, x + COL_W - 1.0, y + HEAD_H))
    }

    /// The BACK TO ARRANGEMENT button.
    pub fn back_rect(&self) -> Rect {
        let b = self.bounds;
        Rect::new(b.x0 + 4.0, b.y0 + 4.0, b.x0 + 164.0, b.y0 + TOOL_H - 4.0)
    }

    /// The RECORD LAUNCHES button.
    pub fn record_rect(&self) -> Rect {
        let r = self.back_rect();
        Rect::new(r.x1 + 6.0, r.y0, r.x1 + 142.0, r.y1)
    }

    /// The SCENE STOPS EMPTY button.
    pub fn stop_empty_rect(&self) -> Rect {
        let r = self.record_rect();
        Rect::new(r.x1 + 6.0, r.y0, r.x1 + 160.0, r.y1)
    }

    /// The launch quantize button.
    pub fn quantize_rect(&self) -> Rect {
        let b = self.bounds;
        Rect::new(
            (b.x1 - 150.0).max(self.stop_empty_rect().x1 + 6.0),
            b.y0 + 4.0,
            (b.x1 - 4.0).max(self.stop_empty_rect().x1 + 150.0),
            b.y0 + TOOL_H - 4.0,
        )
    }

    fn spot_at(&self, pos: Point) -> Option<Spot> {
        if !self.bounds.contains(pos) {
            return None;
        }
        if self.back_rect().contains(pos) {
            return Some(Spot::Back);
        }
        if self.record_rect().contains(pos) {
            return Some(Spot::Record);
        }
        if self.stop_empty_rect().contains(pos) {
            return Some(Spot::StopEmpty);
        }
        if self.quantize_rect().contains(pos) {
            return Some(Spot::Quantize);
        }
        if self.stop_all_rect().contains(pos) {
            return Some(Spot::StopAll);
        }
        for col in self.left_col..(self.left_col + self.visible_cols()).min(self.cols.len()) {
            if self.stop_rect(col).is_some_and(|r| r.contains(pos)) {
                return Some(Spot::Stop(col));
            }
            if self.head_rect(col).is_some_and(|r| r.contains(pos)) {
                return Some(Spot::Head(col));
            }
            for row in self.top_row..(self.top_row + self.visible_rows()).min(self.rows_total()) {
                if self.play_rect(col, row).is_some_and(|r| r.contains(pos)) {
                    return Some(Spot::SlotPlay(col, row));
                }
                if self.slot_rect(col, row).is_some_and(|r| r.contains(pos)) {
                    return Some(Spot::Slot(col, row));
                }
            }
        }
        for row in self.top_row..(self.top_row + self.visible_rows()).min(self.rows_total()) {
            if self.scene_launch_rect(row).is_some_and(|r| r.contains(pos)) {
                return Some(Spot::SceneLaunch(row));
            }
            if self.scene_rect(row).is_some_and(|r| r.contains(pos)) {
                return Some(Spot::Scene(row));
            }
        }
        None
    }

    // ------------------------------------------------------------------ events

    /// The hover help under `pos`.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let spot = self.spot_at(pos)?;
        let (rect, text) = match spot {
            Spot::Back => (
                self.back_rect(),
                "Every track plays its arrangement again, now",
            ),
            Spot::Record => (
                self.record_rect(),
                "Write what you launch into the arrangement while recording",
            ),
            Spot::StopEmpty => (
                self.stop_empty_rect(),
                "A scene launch stops the tracks that have no clip in its row",
            ),
            Spot::Quantize => (
                self.quantize_rect(),
                "How long a launch waits: click for the next value",
            ),
            Spot::StopAll => (
                self.stop_all_rect(),
                "Stop every clip at the next launch boundary",
            ),
            Spot::Stop(c) => (
                self.stop_rect(c)?,
                "Stop this track's clip at the next boundary",
            ),
            Spot::Head(c) => (self.head_rect(c)?, "A track of the arrangement"),
            Spot::SlotPlay(c, r) => (self.play_rect(c, r)?, "Launch this clip"),
            Spot::Slot(c, r) => (
                self.slot_rect(c, r)?,
                if self.slot(c, r).is_some() {
                    "Double click to edit; drag to move, Ctrl-drag to copy"
                } else {
                    "Double click to make a clip here"
                },
            ),
            Spot::SceneLaunch(r) => (
                self.scene_launch_rect(r)?,
                "Launch every clip of this scene",
            ),
            Spot::Scene(r) => (self.scene_rect(r)?, "Double click to rename the scene"),
        };
        Some(Help::new(rect, text))
    }

    fn slot_ref(&self, col: usize, row: usize) -> Option<(DeviceKey, usize)> {
        Some((self.cols.get(col)?.device.clone(), row))
    }

    fn track_state(&self, col: usize) -> TrackLaunch {
        self.cols
            .get(col)
            .and_then(|c| self.feed.tracks.get(&c.device))
            .copied()
            .unwrap_or_default()
    }

    fn launcher_cmd(c: LauncherCommand) -> UiAction {
        UiAction::Command(Command::Song(SongCommand::Launcher(c)))
    }

    /// Handles an event; the actions are document commands and launcher requests for the host.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        if let UiEvent::Ime(ime) = event {
            if let Some(e) = &mut self.edit
                && e.field.handle_ime(ime)
            {
                self.repaint.set(true);
            }
            return out;
        }
        match event {
            UiEvent::PointerMove { pos, .. } => self.on_move(*pos),
            UiEvent::PointerDown {
                pos,
                button,
                modifiers,
                clicks,
            } => self.on_down(*pos, *button, *modifiers, *clicks, &mut out),
            UiEvent::PointerUp {
                pos,
                modifiers,
                button: PointerButton::Primary,
            } => {
                self.on_up(*pos, *modifiers, &mut out);
            }
            UiEvent::PointerLeave => {
                if self.hover.take().is_some() {
                    self.repaint.set(true);
                }
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } if self.bounds.contains(*pos) => {
                self.on_wheel(delta.x, delta.y, *modifiers);
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => self.on_key(key.clone(), *modifiers, &mut out),
            _ => {}
        }
        out
    }

    fn on_move(&mut self, pos: Point) {
        if let Some(m) = &mut self.menu {
            let row = m.list.row_at(pos);
            if row != m.list.hover {
                m.list.hover = row;
                self.repaint.set(true);
            }
        }
        let hover = self.spot_at(pos);
        if hover != self.hover {
            self.hover = hover;
            self.repaint.set(true);
        }
        if let Some(d) = &mut self.drag {
            d.at = pos;
            if !d.started && ((pos.x - d.start.x).hypot(pos.y - d.start.y)) > DRAG_START {
                d.started = true;
            }
            self.repaint.set(true);
        }
    }

    fn on_wheel(&mut self, dx: f64, dy: f64, m: Modifiers) {
        let (vc, vr) = (self.visible_cols(), self.visible_rows());
        let sideways = m.shift || dx.abs() > dy.abs();
        let amount = if sideways && dx.abs() > dy.abs() {
            dx
        } else {
            dy
        };
        if sideways {
            if amount > 0.0 {
                self.left_col = self.left_col.saturating_sub(1);
            } else if amount < 0.0 {
                self.left_col = (self.left_col + 1).min(self.cols.len().saturating_sub(vc));
            }
        } else if amount > 0.0 {
            self.top_row = self.top_row.saturating_sub(1);
        } else if amount < 0.0 {
            self.top_row = (self.top_row + 1).min(self.rows_total().saturating_sub(vr));
        }
        self.repaint.set(true);
    }

    fn on_down(
        &mut self,
        pos: Point,
        button: PointerButton,
        m: Modifiers,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        if self.menu.is_some() {
            self.menu_click(pos, button, out);
            return;
        }
        if self.edit.is_some() {
            // A click outside the name being typed keeps it.
            self.finish_edit(true, out);
        }
        let Some(spot) = self.spot_at(pos) else {
            return;
        };
        self.repaint.set(true);
        if button == PointerButton::Secondary {
            self.open_menu(spot, pos);
            return;
        }
        if button != PointerButton::Primary {
            return;
        }
        let launch = |c: LauncherAction| UiAction::Launcher(c);
        match spot {
            Spot::Back => out.push(launch(LauncherAction::Back)),
            Spot::Record => out.push(launch(LauncherAction::ToggleRecord)),
            Spot::StopEmpty => out.push(launch(LauncherAction::ToggleStopEmpty)),
            Spot::Quantize => {
                let now = self.launcher().map_or(LaunchQuantize::Bar, |l| l.quantize);
                let all = LaunchQuantize::ALL;
                let at = all.iter().position(|q| *q == now).unwrap_or(0);
                let next = if m.shift {
                    all[(at + all.len() - 1) % all.len()]
                } else {
                    all[(at + 1) % all.len()]
                };
                out.push(launch(LauncherAction::SetQuantize(next)));
            }
            Spot::StopAll => out.push(launch(LauncherAction::StopAll)),
            Spot::Stop(c) => {
                if let Some(d) = self.cols.get(c).map(|c| c.device.clone()) {
                    out.push(launch(LauncherAction::Stop(d)));
                }
            }
            Spot::Head(c) => self.set_cursor(c, self.cursor.1),
            Spot::SlotPlay(c, r) => {
                self.set_cursor(c, r);
                let Some(d) = self.cols.get(c).map(|c| c.device.clone()) else {
                    return;
                };
                if self.slot(c, r).is_some() {
                    out.push(launch(LauncherAction::Launch { device: d, row: r }));
                } else if self.track_state(c).playing.is_some() {
                    out.push(launch(LauncherAction::Stop(d)));
                }
            }
            Spot::Slot(c, r) => {
                self.set_cursor(c, r);
                if clicks >= 2 {
                    let Some(d) = self.cols.get(c).map(|c| c.device.clone()) else {
                        return;
                    };
                    match self.slot(c, r) {
                        Some(s) => {
                            out.push(UiAction::Launcher(LauncherAction::OpenClip(s.clip.id)))
                        }
                        None => out.push(launch(LauncherAction::NewClip { device: d, row: r })),
                    }
                } else if self.slot(c, r).is_some() {
                    self.drag = Some(Drag {
                        from: (c, r),
                        start: pos,
                        at: pos,
                        started: false,
                        copy: m.command(),
                    });
                }
            }
            Spot::SceneLaunch(r) => {
                self.set_cursor(self.cursor.0, r);
                out.push(launch(LauncherAction::LaunchScene(r)));
            }
            Spot::Scene(r) => {
                self.set_cursor(self.cursor.0, r);
                if clicks >= 2 {
                    self.begin_scene_rename(r);
                }
            }
        }
    }

    fn on_up(&mut self, pos: Point, m: Modifiers, out: &mut Vec<UiAction>) {
        let Some(d) = self.drag.take() else {
            return;
        };
        self.repaint.set(true);
        if !d.started {
            return;
        }
        let Some(Spot::Slot(c, r) | Spot::SlotPlay(c, r)) = self.spot_at(pos) else {
            return;
        };
        if (c, r) == d.from {
            return;
        }
        let (Some(from), Some(to)) = (self.slot_ref(d.from.0, d.from.1), self.slot_ref(c, r))
        else {
            return;
        };
        self.set_cursor(c, r);
        out.push(Self::launcher_cmd(LauncherCommand::MoveSlot {
            from,
            to,
            copy: d.copy || m.command(),
        }));
    }

    fn on_key(&mut self, key: Key, m: Modifiers, out: &mut Vec<UiAction>) {
        if let Some(e) = &mut self.edit {
            match e.field.handle_key(&key, m) {
                FieldKey::Ignored => match key {
                    Key::Enter => self.finish_edit(true, out),
                    Key::Escape => self.finish_edit(false, out),
                    _ => {}
                },
                _ => self.repaint.set(true),
            }
            return;
        }
        if self.menu.is_some() {
            if key == Key::Escape {
                self.menu = None;
                self.repaint.set(true);
            }
            return;
        }
        if m.command() || m.alt {
            return;
        }
        let (c, r) = self.cursor;
        let rows = self.rows_total();
        match key {
            Key::Left => self.set_cursor(c.saturating_sub(1), r),
            Key::Right => self.set_cursor(c + 1, r),
            Key::Up => self.set_cursor(c, r.saturating_sub(1)),
            Key::Down => self.set_cursor(c, (r + 1).min(rows - 1)),
            Key::Home => self.set_cursor(0, r),
            Key::End => self.set_cursor(self.cols.len(), r),
            Key::PageUp => self.set_cursor(c, r.saturating_sub(self.visible_rows())),
            Key::PageDown => self.set_cursor(c, (r + self.visible_rows()).min(rows - 1)),
            Key::Enter if m.shift => out.push(UiAction::Launcher(LauncherAction::LaunchScene(r))),
            Key::Enter => {
                if let Some(d) = self.cols.get(c).map(|c| c.device.clone()) {
                    if self.slot(c, r).is_some() {
                        out.push(UiAction::Launcher(LauncherAction::Launch {
                            device: d,
                            row: r,
                        }));
                    } else if self.track_state(c).playing.is_some() {
                        out.push(UiAction::Launcher(LauncherAction::Stop(d)));
                    }
                }
            }
            _ => {}
        }
    }

    /// The Edit commands, on the slot under the cursor: Delete, Cut, Copy, Paste and Duplicate.
    pub fn command(&mut self, id: &str, _arg: Option<&str>) -> CommandOutcome {
        if self.edit.is_some() {
            return match self.edit.as_mut().and_then(|e| e.field.command(id)) {
                Some(_) => {
                    self.repaint.set(true);
                    CommandOutcome::Handled(Vec::new())
                }
                None => CommandOutcome::Declined,
            };
        }
        let Some((device, row)) = self.cursor_slot() else {
            return CommandOutcome::Declined;
        };
        let (c, r) = self.cursor;
        let filled = self.slot(c, r).cloned();
        let cmd = |c: LauncherCommand| UiAction::Command(Command::Song(SongCommand::Launcher(c)));
        match id {
            "launcher.launch" => match self.cols.get(c).map(|col| col.device.clone()) {
                Some(d) if filled.is_some() => {
                    CommandOutcome::Handled(vec![UiAction::Launcher(LauncherAction::Launch {
                        device: d,
                        row,
                    })])
                }
                Some(d) if self.track_state(c).playing.is_some() => {
                    CommandOutcome::Handled(vec![UiAction::Launcher(LauncherAction::Stop(d))])
                }
                _ => CommandOutcome::NotApplicable("That slot is empty".into()),
            },
            "launcher.launch_scene" => {
                CommandOutcome::Handled(vec![UiAction::Launcher(LauncherAction::LaunchScene(r))])
            }
            "edit.copy" => {
                let Some(clip) = filled else {
                    return CommandOutcome::NotApplicable("That slot is empty".into());
                };
                self.clipboard = Some(clip);
                CommandOutcome::Handled(Vec::new())
            }
            "edit.cut" => {
                let Some(clip) = filled else {
                    return CommandOutcome::NotApplicable("That slot is empty".into());
                };
                self.clipboard = Some(clip);
                CommandOutcome::Handled(vec![cmd(LauncherCommand::ClearSlots {
                    slots: vec![(device, row)],
                })])
            }
            "edit.delete" => {
                if filled.is_none() {
                    return CommandOutcome::NotApplicable("That slot is empty".into());
                }
                CommandOutcome::Handled(vec![cmd(LauncherCommand::ClearSlots {
                    slots: vec![(device, row)],
                })])
            }
            "edit.paste" => match self.clipboard.clone() {
                Some(clip) => CommandOutcome::Handled(vec![cmd(LauncherCommand::SetSlot {
                    device,
                    row,
                    clip,
                })]),
                None => CommandOutcome::NotApplicable("Copy a launcher clip first".into()),
            },
            "edit.duplicate" => {
                if filled.is_none() {
                    return CommandOutcome::NotApplicable("That slot is empty".into());
                }
                CommandOutcome::Handled(vec![cmd(LauncherCommand::MoveSlot {
                    from: (device.clone(), row),
                    to: (device, row + 1),
                    copy: true,
                })])
            }
            _ => CommandOutcome::Declined,
        }
    }

    // ------------------------------------------------------------------ rename

    fn begin_scene_rename(&mut self, row: usize) {
        let name = self
            .launcher()
            .and_then(|l| l.scenes.get(row))
            .map(|s| s.name.clone())
            .unwrap_or_default();
        self.edit = Some(Editing {
            target: EditTarget::Scene(row),
            field: TextField::new(&name, MAX_CLIP_NAME_CHARS, false),
        });
        if let Some(e) = &mut self.edit {
            e.field.select_all();
        }
        self.repaint.set(true);
    }

    fn begin_slot_rename(&mut self, col: usize, row: usize) {
        let Some(s) = self.slot(col, row) else {
            return;
        };
        let name = s.clip.name.clone().unwrap_or_default();
        self.edit = Some(Editing {
            target: EditTarget::Slot(col, row),
            field: TextField::new(&name, MAX_CLIP_NAME_CHARS, false),
        });
        if let Some(e) = &mut self.edit {
            e.field.select_all();
        }
        self.repaint.set(true);
    }

    fn finish_edit(&mut self, keep: bool, out: &mut Vec<UiAction>) {
        let Some(e) = self.edit.take() else {
            return;
        };
        self.repaint.set(true);
        if !keep {
            return;
        }
        let text = e.field.text().trim().to_owned();
        match e.target {
            EditTarget::Scene(at) => out.push(Self::launcher_cmd(LauncherCommand::RenameScene {
                at,
                name: (!text.is_empty()).then_some(text),
            })),
            EditTarget::Slot(c, row) => {
                if let Some(device) = self.cols.get(c).map(|c| c.device.clone()) {
                    out.push(Self::launcher_cmd(LauncherCommand::EditSlot {
                        device,
                        row,
                        name: if text.is_empty() {
                            MetaChange::Clear
                        } else {
                            MetaChange::Set(text)
                        },
                        color: MetaChange::Keep,
                        length: None,
                        quantize: None,
                    }));
                }
            }
        }
    }

    // ------------------------------------------------------------------ menu

    fn open_menu(&mut self, spot: Spot, pos: Point) {
        let target = match spot {
            Spot::Slot(c, r) | Spot::SlotPlay(c, r) if self.slot(c, r).is_some() => {
                MenuTarget::Slot(c, r)
            }
            Spot::Scene(r) | Spot::SceneLaunch(r) => MenuTarget::Scene(r),
            _ => return,
        };
        self.cursor = match target {
            MenuTarget::Slot(c, r) => (c, r),
            MenuTarget::Scene(r) => (self.cursor.0, r),
        };
        self.build_menu(target, Level::Root, pos);
    }

    fn build_menu(&mut self, target: MenuTarget, level: Level, at: Point) {
        let rows: Vec<(String, Item)> = match (target, level) {
            (MenuTarget::Slot(..), Level::Root) => vec![
                ("Rename".into(), Item::Rename),
                ("Colour".into(), Item::Colour),
                ("Loop Length".into(), Item::Length),
                ("Quantize".into(), Item::Quantize),
                ("Duplicate".into(), Item::Duplicate),
                ("Delete".into(), Item::Delete),
            ],
            (MenuTarget::Scene(_), Level::Root) => vec![
                ("Rename".into(), Item::Rename),
                ("Insert Scene".into(), Item::InsertScene),
                ("Duplicate Scene".into(), Item::DuplicateScene),
                ("Delete Scene".into(), Item::DeleteScene),
            ],
            (_, Level::Colour) => COLOURS
                .iter()
                .map(|n| {
                    let label = if *n == "track" { "Track colour" } else { n };
                    (label.to_string(), Item::SetColour(n))
                })
                .collect(),
            (_, Level::Length) => LENGTH_BARS
                .iter()
                .map(|n| {
                    let label = if *n == 1 {
                        "1 bar".to_owned()
                    } else {
                        format!("{n} bars")
                    };
                    (label, Item::SetLength(*n))
                })
                .collect(),
            (_, Level::Quantize) => {
                std::iter::once(("Song setting".to_owned(), Item::SetQuantize(None)))
                    .chain(
                        LaunchQuantize::ALL
                            .iter()
                            .map(|q| (q.label().to_owned(), Item::SetQuantize(Some(*q)))),
                    )
                    .collect()
            }
        };
        let mut list = PickList::new(at, 150.0, rows.len().min(12), rows.len(), 0);
        list.keep_above(self.bounds.y1, self.bounds.y0);
        if list.origin.x + list.width > self.bounds.x1 {
            list.origin.x = (self.bounds.x1 - list.width).max(self.bounds.x0);
        }
        self.menu = Some(Menu { target, list, rows });
        self.repaint.set(true);
    }

    fn menu_click(&mut self, pos: Point, button: PointerButton, out: &mut Vec<UiAction>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        self.repaint.set(true);
        if button != PointerButton::Primary {
            return;
        }
        let Some(i) = menu.list.row_at(pos) else {
            return;
        };
        let Some((_, item)) = menu.rows.get(i).cloned() else {
            return;
        };
        let at = menu.list.origin;
        match (menu.target, item) {
            (t, Item::Colour) => self.build_menu(t, Level::Colour, at),
            (t, Item::Length) => self.build_menu(t, Level::Length, at),
            (t, Item::Quantize) => self.build_menu(t, Level::Quantize, at),
            (MenuTarget::Slot(c, r), Item::Rename) => self.begin_slot_rename(c, r),
            (MenuTarget::Scene(r), Item::Rename) => self.begin_scene_rename(r),
            (MenuTarget::Slot(c, r), Item::Duplicate) => {
                if let Some((device, row)) = self.slot_ref(c, r) {
                    out.push(Self::launcher_cmd(LauncherCommand::MoveSlot {
                        from: (device.clone(), row),
                        to: (device, row + 1),
                        copy: true,
                    }));
                }
            }
            (MenuTarget::Slot(c, r), Item::Delete) => {
                if let Some(slot) = self.slot_ref(c, r) {
                    out.push(Self::launcher_cmd(LauncherCommand::ClearSlots {
                        slots: vec![slot],
                    }));
                }
            }
            (MenuTarget::Slot(c, r), Item::SetColour(name)) => {
                let color = match palette_color(name) {
                    _ if name == "track" => MetaChange::Clear,
                    Some(Some(rgb)) => MetaChange::Set(rgb),
                    _ => return,
                };
                if let Some((device, row)) = self.slot_ref(c, r) {
                    out.push(Self::launcher_cmd(LauncherCommand::EditSlot {
                        device,
                        row,
                        name: MetaChange::Keep,
                        color,
                        length: None,
                        quantize: None,
                    }));
                }
            }
            (MenuTarget::Slot(c, r), Item::SetLength(bars)) => {
                if let Some((device, row)) = self.slot_ref(c, r) {
                    let bar = self.bar_ticks();
                    out.push(Self::launcher_cmd(LauncherCommand::EditSlot {
                        device,
                        row,
                        name: MetaChange::Keep,
                        color: MetaChange::Keep,
                        length: Some(bars * bar),
                        quantize: None,
                    }));
                }
            }
            (MenuTarget::Slot(c, r), Item::SetQuantize(q)) => {
                if let Some((device, row)) = self.slot_ref(c, r) {
                    out.push(Self::launcher_cmd(LauncherCommand::EditSlot {
                        device,
                        row,
                        name: MetaChange::Keep,
                        color: MetaChange::Keep,
                        length: None,
                        quantize: Some(q),
                    }));
                }
            }
            (MenuTarget::Scene(r), Item::InsertScene) => {
                out.push(Self::launcher_cmd(LauncherCommand::InsertScene { at: r }));
            }
            (MenuTarget::Scene(r), Item::DuplicateScene) => {
                out.push(Self::launcher_cmd(LauncherCommand::DuplicateScene {
                    at: r,
                }));
            }
            (MenuTarget::Scene(r), Item::DeleteScene) => {
                out.push(Self::launcher_cmd(LauncherCommand::DeleteScene { at: r }));
            }
            _ => {}
        }
    }

    /// The length of a bar at the start of the song, in ticks.
    fn bar_ticks(&self) -> i64 {
        self.song.as_deref().map_or(atomik_core::PPQ * 4, |s| {
            s.meter_table().map().bar_ticks_at(0)
        })
    }

    /// The labels of the open list's rows (for tests).
    pub fn menu_labels(&self) -> Vec<String> {
        self.menu
            .as_ref()
            .map(|m| m.rows.iter().map(|(l, _)| l.clone()).collect())
            .unwrap_or_default()
    }

    /// The rectangle of row `i` of the open list (for tests).
    pub fn menu_row_rect(&self, i: usize) -> Option<Rect> {
        self.menu.as_ref()?.list.row_rect(i)
    }

    // ------------------------------------------------------------------ paint

    /// Draws the view inside its bounds.
    pub fn paint(&self, c: &mut dyn Canvas) {
        self.repaint.set(false);
        c.push_transform(vello::kurbo::Affine::scale(self.scale));
        c.push_clip(&Geom::Rect(self.bounds));
        self.paint_logical(c);
        c.pop_clip();
        c.pop_transform();
    }

    fn button(&self, c: &mut dyn Canvas, r: Rect, label: &str, lit: bool, hot: bool) {
        let t = &self.st.base;
        let fill = if lit {
            t.warning.with_alpha(0.85)
        } else {
            t.button
        };
        c.fill_with(RoundedRect::from_rect(r, 3.0), fill);
        if hot {
            c.stroke_with(RoundedRect::from_rect(r, 3.0), 1.0, t.focus);
        }
        let ink = if lit { t.ink } else { t.text };
        c.text(
            &TextRun::new(label, pt(r.center().x, r.center().y + 3.5), 9.5, ink)
                .align(Align::Center)
                .weight(700),
        );
    }

    fn paint_logical(&self, c: &mut dyn Canvas) {
        let t = &self.st.base;
        let b = self.bounds;
        c.fill_with(b, self.st.background);
        // The bar of buttons.
        c.fill_with(Rect::new(b.x0, b.y0, b.x1, b.y0 + TOOL_H), self.st.header);
        let overriding = self.feed.tracks.values().any(|s| s.playing.is_some());
        self.button(
            c,
            self.back_rect(),
            "BACK TO ARRANGEMENT",
            overriding,
            self.hover == Some(Spot::Back),
        );
        self.button(
            c,
            self.record_rect(),
            "RECORD LAUNCHES",
            self.feed.record,
            self.hover == Some(Spot::Record),
        );
        let stop_empty = self.launcher().is_none_or(|l| l.stop_empty);
        self.button(
            c,
            self.stop_empty_rect(),
            "SCENE STOPS EMPTY",
            stop_empty,
            self.hover == Some(Spot::StopEmpty),
        );
        let q = self.launcher().map_or(LaunchQuantize::Bar, |l| l.quantize);
        self.button(
            c,
            self.quantize_rect(),
            &format!("QUANTIZE {}", q.label().to_uppercase()),
            false,
            self.hover == Some(Spot::Quantize),
        );
        // Track names.
        c.fill_with(
            Rect::new(b.x0, b.y0 + TOOL_H, b.x1 - SCENE_W, b.y0 + TOOL_H + HEAD_H),
            self.st.header_track,
        );
        let grid = self.grid();
        c.push_clip(&Geom::Rect(Rect::new(b.x0, b.y0 + TOOL_H, grid.x1, b.y1)));
        for col in self.left_col..(self.left_col + self.visible_cols()).min(self.cols.len()) {
            self.paint_column(c, col);
        }
        c.pop_clip();
        if self.cols.is_empty() {
            c.text(&TextRun::new(
                "No note tracks yet: add an instrument track to fill the launcher",
                pt(b.x0 + 12.0, grid.y0 + 24.0),
                10.5,
                t.text_muted,
            ));
        }
        // Scenes.
        c.fill_with(
            Rect::new(b.x1 - SCENE_W, b.y0 + TOOL_H, b.x1, b.y1),
            self.st.header,
        );
        c.fill_with(
            Rect::new(b.x1 - SCENE_W, b.y0 + TOOL_H, b.x1 - SCENE_W + 1.0, b.y1),
            self.st.separator,
        );
        for row in self.top_row..(self.top_row + self.visible_rows()).min(self.rows_total()) {
            self.paint_scene(c, row);
        }
        self.paint_stop_row(c);
        if let Some(d) = self.drag.filter(|d| d.started) {
            self.paint_drag(c, d);
        }
        if let Some(m) = &self.menu {
            self.paint_menu(c, m);
        }
    }

    fn paint_column(&self, c: &mut dyn Canvas, col: usize) {
        let t = &self.st.base;
        let Some(head) = self.head_rect(col) else {
            return;
        };
        let accent = self.cols[col].accent;
        c.fill_with(Rect::new(head.x0, head.y1 - 3.0, head.x1, head.y1), accent);
        let max = ((head.width() - 8.0) / 5.6).max(4.0) as usize;
        c.text(
            &TextRun::new(
                shorten(&self.cols[col].name, max),
                pt(head.x0 + 6.0, head.y0 + 15.0),
                10.0,
                t.text,
            )
            .weight(700),
        );
        for row in self.top_row..(self.top_row + self.visible_rows()).min(self.rows_total()) {
            self.paint_slot(c, col, row);
        }
    }

    fn paint_slot(&self, c: &mut dyn Canvas, col: usize, row: usize) {
        let t = &self.st.base;
        let Some(r) = self.slot_rect(col, row) else {
            return;
        };
        let state = self.track_state(col);
        let accent = self.cols[col].accent;
        let slot = self.slot(col, row);
        let playing = state.playing == Some(row) && slot.is_some();
        let queued = state.queued == Some(row);
        let base = if row.is_multiple_of(2) {
            self.st.lane
        } else {
            self.st.lane_alt
        };
        let shape = RoundedRect::from_rect(r, 3.0);
        match slot {
            Some(s) => {
                let tint = s
                    .clip
                    .color
                    .map_or(accent, |[r, g, b]| Color::from_rgb8(r, g, b));
                c.fill_with(shape, tint.with_alpha(if playing { 0.85 } else { 0.45 }));
                self.paint_overview(c, r, s, playing);
                let ink = if playing { t.ink } else { t.text };
                let name = s
                    .clip
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Clip {}", row + 1));
                let max = ((r.width() - PLAY_W - 6.0) / 5.6).max(3.0) as usize;
                c.text(
                    &TextRun::new(
                        shorten(&name, max),
                        pt(r.x0 + PLAY_W, r.y0 + 13.0),
                        9.5,
                        ink,
                    )
                    .weight(700),
                );
                self.paint_triangle(c, self.play_rect(col, row).unwrap_or(r), ink);
                if playing {
                    let w = (r.width() * f64::from(state.phase.clamp(0.0, 1.0))).max(1.0);
                    c.fill_with(Rect::new(r.x0, r.y1 - 3.0, r.x0 + w, r.y1), ink);
                }
            }
            None => {
                c.fill_with(shape, base);
                if state.playing.is_some()
                    && let Some(z) = self.play_rect(col, row)
                {
                    let sq = Rect::from_center_size(z.center(), (9.0, 9.0));
                    c.fill_with(sq, t.text_muted);
                }
            }
        }
        if queued && self.feed.blink {
            c.stroke_with(shape, 2.0, self.st.playhead);
        } else if queued {
            c.stroke_with(shape, 1.0, self.st.playhead.with_alpha(0.5));
        }
        if self.focused && self.cursor == (col, row) {
            c.stroke_with(
                RoundedRect::from_rect(r.inset(-1.0), 4.0),
                1.5,
                self.st.selection,
            );
        }
        if self.hover == Some(Spot::SlotPlay(col, row)) {
            c.stroke_with(
                RoundedRect::from_rect(self.play_rect(col, row).unwrap_or(r).inset(-2.0), 3.0),
                1.0,
                t.focus,
            );
        }
        if let Some(e) = &self.edit
            && e.target == EditTarget::Slot(col, row)
        {
            self.paint_field(
                c,
                e,
                Rect::new(r.x0 + PLAY_W, r.y0 + 2.0, r.x1 - 3.0, r.y0 + 18.0),
            );
        }
    }

    fn paint_field(&self, c: &mut dyn Canvas, e: &Editing, rect: Rect) {
        let t = &self.st.base;
        c.fill_with(RoundedRect::from_rect(rect, 2.0), t.lcd);
        e.field.paint(c, rect, 9.5, 14.0, FieldColors::of(t), true);
    }

    fn paint_triangle(&self, c: &mut dyn Canvas, zone: Rect, color: Color) {
        let p = zone.center();
        let mut path = BezPath::new();
        path.move_to((p.x - 4.0, p.y - 6.0));
        path.line_to((p.x + 6.0, p.y));
        path.line_to((p.x - 4.0, p.y + 6.0));
        path.close_path();
        c.fill_with(Geom::Path(path), color);
    }

    /// A tiny picture of the slot's notes.
    fn paint_overview(&self, c: &mut dyn Canvas, r: Rect, s: &LauncherClip, playing: bool) {
        let ClipContent::Notes(notes) = &s.clip.content else {
            return;
        };
        if notes.is_empty() {
            return;
        }
        let area = Rect::new(r.x0 + PLAY_W, r.y0 + 17.0, r.x1 - 4.0, r.y1 - 5.0);
        let len = s.clip.length.max(1) as f64;
        let (lo, hi) = notes
            .iter()
            .fold((127u8, 0u8), |(lo, hi), n| (lo.min(n.key), hi.max(n.key)));
        let span = f64::from(hi - lo).max(7.0);
        let ink = if playing {
            self.st.base.ink
        } else {
            self.st.base.text
        };
        for n in notes.iter().take(OVERVIEW_NOTES) {
            if n.tick < 0 || n.tick as f64 >= len {
                continue;
            }
            let x0 = area.x0 + area.width() * n.tick as f64 / len;
            let x1 = (area.x0 + area.width() * ((n.tick + n.length) as f64).min(len) / len)
                .max(x0 + 1.0);
            let y = area.y1 - area.height() * (f64::from(n.key - lo) + 0.5) / (span + 1.0);
            c.fill_with(Rect::new(x0, y - 1.0, x1, y + 1.0), ink.with_alpha(0.8));
        }
    }

    fn paint_scene(&self, c: &mut dyn Canvas, row: usize) {
        let t = &self.st.base;
        let (Some(launch), Some(name)) = (self.scene_launch_rect(row), self.scene_rect(row)) else {
            return;
        };
        let scene = self.launcher().and_then(|l| l.scenes.get(row));
        let tint = scene
            .and_then(|s| s.color)
            .map_or(t.button, |[r, g, b]| Color::from_rgb8(r, g, b));
        c.fill_with(
            RoundedRect::from_rect(Rect::new(launch.x0, launch.y0, name.x1, name.y1), 3.0),
            tint.with_alpha(0.5),
        );
        self.paint_triangle(c, launch, t.text);
        if self.hover == Some(Spot::SceneLaunch(row)) {
            c.stroke_with(
                RoundedRect::from_rect(launch.inset(-1.0), 3.0),
                1.0,
                t.focus,
            );
        }
        let label = self
            .launcher()
            .map_or_else(|| format!("Scene {}", row + 1), |l| l.scene_label(row));
        c.text(&TextRun::new(
            shorten(&label, ((name.width() - 8.0) / 5.6) as usize),
            pt(name.x0 + 6.0, name.y0 + 17.0),
            10.0,
            t.text,
        ));
        if let Some(tempo) = scene.and_then(|s| s.tempo) {
            c.text(&TextRun::new(
                format!("{tempo:.0} BPM"),
                pt(name.x0 + 6.0, name.y0 + 31.0),
                8.5,
                t.text_muted,
            ));
        }
        if self.focused && self.cursor.1 == row {
            c.stroke_with(
                RoundedRect::from_rect(
                    Rect::new(launch.x0, launch.y0, name.x1, name.y1).inset(-1.0),
                    4.0,
                ),
                1.0,
                self.st.selection,
            );
        }
        if let Some(e) = &self.edit
            && e.target == EditTarget::Scene(row)
        {
            self.paint_field(
                c,
                e,
                Rect::new(name.x0 + 3.0, name.y0 + 4.0, name.x1 - 3.0, name.y0 + 22.0),
            );
        }
    }

    fn paint_stop_row(&self, c: &mut dyn Canvas) {
        let t = &self.st.base;
        let b = self.bounds;
        c.fill_with(Rect::new(b.x0, b.y1 - STOP_H, b.x1, b.y1), self.st.header);
        for col in self.left_col..(self.left_col + self.visible_cols()).min(self.cols.len()) {
            let Some(r) = self.stop_rect(col) else {
                continue;
            };
            let state = self.track_state(col);
            let lit = state.playing.is_some();
            let hot = self.hover == Some(Spot::Stop(col));
            c.fill_with(
                RoundedRect::from_rect(r, 3.0),
                if lit { t.button } else { t.led_off },
            );
            if hot {
                c.stroke_with(RoundedRect::from_rect(r, 3.0), 1.0, t.focus);
            }
            if state.queued_stop {
                c.stroke_with(RoundedRect::from_rect(r, 3.0), 1.5, self.st.playhead);
            }
            let sq = Rect::from_center_size(r.center(), (9.0, 9.0));
            c.fill_with(sq, if lit { t.text } else { t.text_muted });
        }
        self.button(
            c,
            self.stop_all_rect(),
            "STOP ALL",
            false,
            self.hover == Some(Spot::StopAll),
        );
    }

    fn paint_drag(&self, c: &mut dyn Canvas, d: Drag) {
        let t = &self.st.base;
        if let Some(r) = self.slot_rect(d.from.0, d.from.1) {
            c.fill_with(RoundedRect::from_rect(r, 3.0), t.dim.with_alpha(0.35));
        }
        if let Some(Spot::Slot(col, row) | Spot::SlotPlay(col, row)) = self.spot_at(d.at)
            && let Some(r) = self.slot_rect(col, row)
        {
            let color = if d.copy {
                self.st.selection
            } else {
                self.st.record
            };
            c.stroke_with(RoundedRect::from_rect(r, 3.0), 2.0, color);
        }
        let ghost = Rect::new(d.at.x - 40.0, d.at.y - 12.0, d.at.x + 40.0, d.at.y + 12.0);
        c.fill_with(
            RoundedRect::from_rect(ghost, 3.0),
            t.selection.with_alpha(0.5),
        );
    }

    fn paint_menu(&self, c: &mut dyn Canvas, m: &Menu) {
        let t = &self.st.base;
        m.list.paint_frame(c, &self.st);
        for (i, (label, _)) in m.rows.iter().enumerate() {
            let Some(r) = m.list.row_rect(i) else {
                continue;
            };
            if m.list.hover == Some(i) {
                c.fill_with(RoundedRect::from_rect(r, 3.0), t.browser_row);
            }
            c.text(&TextRun::new(
                label.as_str(),
                pt(r.x0 + PAD, r.center().y + 3.5),
                10.0,
                t.text,
            ));
        }
        m.list.paint_scrollbar(c, &self.st);
    }
}

/// `text` cut to `max` characters, with an ellipsis when it was longer.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        let cut: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}...")
    } else {
        text.to_owned()
    }
}
