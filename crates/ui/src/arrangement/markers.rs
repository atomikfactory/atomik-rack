//! Markers and range markers of the [`ArrangementView`]: the strip under the ruler (ranges in
//! the top row, point markers in the bottom row), its gestures, the commands behind the
//! context menus and the keys, and the marker list docked at the right edge. The pure edits
//! are in [`crate::marker_edit`].
//!
//! A drag sends `DragMarkers` / `DragRanges` with one merge id (one undo step) and ends with
//! [`UiAction::EndMerge`]; Escape puts the lists back. A click on a marker selects it and
//! locates the playhead there; a click on a range selects it and sets the loop to it. A
//! double-click on a label types a new name in place (the view is the text owner meanwhile,
//! see [`ArrangementView::is_text_owner`]).

use super::{
    ArrangementView, DRAG_THRESHOLD, Drag, Hit, LINE_GRAB, MIN_TICKS, locator_range, song_cmd,
};
use crate::action::{ContextTarget, TransportAction, UiAction};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{Key, Modifiers, PointerButton};
use crate::keymap::CommandOutcome;
use crate::marker_edit::{self as edit, MarkSel, RangePart};
use crate::paint::{estimate_width, fit_label};
use crate::seq_edit;
use crate::seq_paint as sp;
use crate::theme::mix;
use crate::timeline::{Quant, format_bbt};
use crate::viewport::Tool;
use atomik_model::{ClipId, MAX_MARKER_NAME, Marker, RangeMarker, SongCommand};
use vello::kurbo::{Point, Rect, RoundedRect};
use vello::peniko::Color;

/// Height of one row of the strip.
const MARK_ROW: f64 = 15.0;
/// Height of the marker strip (a row of ranges over a row of markers).
pub const MARKER_STRIP_HEIGHT: f64 = 2.0 * MARK_ROW;
/// Width of the grab zone at each end of a range.
const EDGE: f64 = 5.0;
/// Width of the marker list.
pub const MARKER_LIST_WIDTH: f64 = 280.0;
/// Height of the list's header (title and the add buttons).
const LIST_HEADER: f64 = 28.0;
/// Height of a list row.
const LIST_ROW: f64 = 22.0;

/// What a pointer press on the list hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ListHit {
    /// "+ Marker".
    AddMarker,
    /// "+ Range".
    AddRange,
    /// The delete button of a row.
    Delete(MarkSel),
    /// The name of a row.
    Name(MarkSel),
    /// A row elsewhere.
    Row(MarkSel),
    /// The header or the empty part of the list.
    Empty,
}

/// A row of the list.
#[derive(Debug, Clone, PartialEq)]
struct ListRow {
    sel: MarkSel,
    tick: i64,
    end: Option<i64>,
    name: String,
    color: Option<[u8; 3]>,
}

/// A name being typed.
#[derive(Debug, Clone)]
pub(super) struct Rename {
    target: MarkSel,
    text: String,
    /// The field sits in the list, not in the strip.
    in_list: bool,
}

/// The gesture of a marker or range drag (the view's `Drag` only says that one is going on).
#[derive(Debug, Clone)]
pub(super) enum MarkDrag {
    Marker {
        start: Point,
        id: u32,
        /// Ticks from the marker to the pointer at the press.
        grab: i64,
        original: Vec<Marker>,
        merge: u64,
        moved: bool,
        emitted: bool,
        last: Vec<Marker>,
    },
    Range {
        start: Point,
        id: u32,
        part: RangePart,
        grab: i64,
        original: Vec<RangeMarker>,
        merge: u64,
        moved: bool,
        emitted: bool,
        last: Vec<RangeMarker>,
    },
}

/// The marker state of the view.
#[derive(Debug, Default)]
pub(super) struct MarkerUi {
    /// The strip is shown.
    pub strip: bool,
    /// The list is shown.
    pub list: bool,
    /// The selected marker or range.
    pub sel: Option<MarkSel>,
    /// The name being typed.
    pub rename: Option<Rename>,
    /// What the last context menu opened on, and where (the snapped tick).
    pub ctx: Option<MarkSel>,
    pub ctx_tick: i64,
    /// The snapped tick of the last ruler menu for the loop's locators (zero is allowed).
    pub ctx_loop_tick: i64,
    /// The list's scroll offset in pixels.
    pub scroll: f64,
    pub drag: Option<MarkDrag>,
}

fn color_of(c: Option<[u8; 3]>, default: Color) -> Color {
    c.map_or(default, |[r, g, b]| Color::from_rgba8(r, g, b, 255))
}

fn handled(actions: Vec<UiAction>) -> Option<CommandOutcome> {
    Some(CommandOutcome::Handled(actions))
}

impl ArrangementView {
    // ------------------------------------------------------------------ host API

    /// Whether the marker strip is shown under the ruler.
    pub fn marker_strip_visible(&self) -> bool {
        self.mk.strip
    }

    /// Shows or hides the marker strip.
    pub fn set_marker_strip(&mut self, on: bool) {
        if on != self.mk.strip {
            self.mk.strip = on;
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// Whether the marker list is docked at the right edge.
    pub fn marker_list_visible(&self) -> bool {
        self.mk.list
    }

    /// Shows or hides the marker list (the timeline gives up its width).
    pub fn set_marker_list(&mut self, on: bool) {
        if on != self.mk.list {
            self.mk.list = on;
            self.apply_bounds();
            self.repaint = true;
        }
    }

    /// The strip's rectangle (empty when it is hidden).
    pub fn marker_strip_rect(&self) -> Rect {
        Rect::new(
            self.axis.x0,
            self.ruler_bottom(),
            self.bounds.x1.max(self.axis.x0),
            self.markers_bottom(),
        )
    }

    /// The list's rectangle (empty when it is hidden).
    pub fn marker_list_rect(&self) -> Rect {
        if self.mk.list {
            Rect::new(
                self.outer.x1 - MARKER_LIST_WIDTH,
                self.outer.y0,
                self.outer.x1,
                self.outer.y1,
            )
        } else {
            Rect::new(self.outer.x1, self.outer.y0, self.outer.x1, self.outer.y1)
        }
    }

    /// The label of marker `id` in the strip (the grab area; its left edge is the marker's
    /// tick).
    pub fn marker_rect(&self, id: u32) -> Option<Rect> {
        self.marker_rects()
            .into_iter()
            .find(|(i, _)| *i == id)
            .map(|(_, r)| r)
    }

    /// The band of range `id` in the strip.
    pub fn range_rect(&self, id: u32) -> Option<Rect> {
        let r = self.song.ranges.iter().find(|r| r.id == id)?;
        Some(self.range_geom(r))
    }

    /// The selected marker or range.
    pub fn selected_marker(&self) -> Option<MarkSel> {
        self.mk.sel
    }

    /// Selects a marker or range (nothing when it does not exist).
    pub fn set_selected_marker(&mut self, sel: Option<MarkSel>) {
        self.mk.sel = sel.filter(|s| self.mark_exists(*s));
        self.repaint = true;
    }

    /// Whether the view owns the keyboard as a text field: a marker or range name is being
    /// typed (double-click on a label, [`Self::begin_marker_rename`]), or a folder name or a
    /// track comment is (see [`Self::org_edit_text`]). The shell then reports
    /// the `Text` context over `Sequencer`, so plain keys reach [`Self::handle`] instead of
    /// commands.
    pub fn is_text_owner(&self) -> bool {
        self.mk.rename.is_some() || self.org_editing() || self.insp.editing() || self.pts.editing()
    }

    /// The name being typed, if an edit is open.
    pub fn marker_rename_text(&self) -> Option<&str> {
        self.mk.rename.as_ref().map(|r| r.text.as_str())
    }

    /// Starts typing the name of a marker or range in the strip. False when it does not exist.
    pub fn begin_marker_rename(&mut self, sel: MarkSel) -> bool {
        self.begin_rename(sel, false)
    }

    /// Ends an open name edit without renaming.
    pub fn cancel_marker_rename(&mut self) {
        if self.mk.rename.take().is_some() {
            self.repaint = true;
        }
    }

    /// The row of `sel` in the list (`None` when the list is hidden, the row is scrolled out
    /// or there is no such marker).
    pub fn list_row_rect(&self, sel: MarkSel) -> Option<Rect> {
        let i = self.list_rows().iter().position(|r| r.sel == sel)?;
        self.list_row_at(i)
    }

    /// The name cell of `sel` in the list.
    pub fn list_name_rect(&self, sel: MarkSel) -> Option<Rect> {
        let r = self.list_row_rect(sel)?;
        Some(Rect::new(r.x0 + 14.0, r.y0, r.x0 + 114.0, r.y1))
    }

    /// The delete button of `sel` in the list.
    pub fn list_delete_rect(&self, sel: MarkSel) -> Option<Rect> {
        let r = self.list_row_rect(sel)?;
        Some(Rect::new(r.x1 - 30.0, r.y0, r.x1 - 10.0, r.y1))
    }

    /// The "+ Marker" button of the list.
    pub fn list_add_marker_rect(&self) -> Rect {
        let l = self.marker_list_rect();
        Rect::new(l.x1 - 134.0, l.y0 + 4.0, l.x1 - 72.0, l.y0 + 24.0)
    }

    /// The "+ Range" button of the list.
    pub fn list_add_range_rect(&self) -> Rect {
        let l = self.marker_list_rect();
        Rect::new(l.x1 - 68.0, l.y0 + 4.0, l.x1 - 8.0, l.y0 + 24.0)
    }

    /// The marker and ranges the list shows, in order (by position; at one position markers
    /// come first).
    pub fn list_order(&self) -> Vec<MarkSel> {
        self.list_rows().into_iter().map(|r| r.sel).collect()
    }

    // ------------------------------------------------------------------ geometry

    fn mark_exists(&self, sel: MarkSel) -> bool {
        match sel {
            MarkSel::Marker(id) => self.song.markers.iter().any(|m| m.id == id),
            MarkSel::Range(id) => self.song.ranges.iter().any(|r| r.id == id),
        }
    }

    fn ranges_row(&self) -> Rect {
        let s = self.marker_strip_rect();
        Rect::new(s.x0, s.y0, s.x1, s.y0 + MARK_ROW)
    }

    fn markers_row(&self) -> Rect {
        let s = self.marker_strip_rect();
        Rect::new(s.x0, s.y0 + MARK_ROW, s.x1, s.y1.max(s.y0 + MARK_ROW))
    }

    /// The band of a range in the strip (not clipped to the visible timeline).
    fn range_geom(&self, r: &RangeMarker) -> Rect {
        let row = self.ranges_row();
        let x0 = self.axis.tick_to_x(r.start);
        let x1 = self.axis.tick_to_x(r.end).max(x0 + 3.0);
        Rect::new(x0, row.y0 + 1.0, x1, row.y1 - 1.0)
    }

    /// The labels of the markers: each runs from its tick to the next marker (at most as wide
    /// as its text).
    fn marker_rects(&self) -> Vec<(u32, Rect)> {
        let row = self.markers_row();
        if row.height() <= 0.0 {
            return Vec::new();
        }
        let xs: Vec<f64> = self
            .song
            .markers
            .iter()
            .map(|m| self.axis.tick_to_x(m.tick))
            .collect();
        self.song
            .markers
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let own = estimate_width(&m.name, 9.0) + 12.0;
                let room = xs
                    .get(i + 1)
                    .map_or(own, |next| (next - xs[i] - 2.0).max(8.0));
                let w = own.min(room);
                (
                    m.id,
                    Rect::new(xs[i], row.y0 + 1.0, xs[i] + w, row.y1 - 1.0),
                )
            })
            .collect()
    }

    /// The marker whose line is within [`LINE_GRAB`] of `x` (the nearest one); only while the
    /// strip is shown, as the lines are.
    pub(super) fn marker_line_at(&self, x: f64) -> Option<u32> {
        if !self.mk.strip {
            return None;
        }
        self.song
            .markers
            .iter()
            .map(|m| (m.id, (self.axis.tick_to_x(m.tick) - x).abs()))
            .filter(|(_, d)| *d <= LINE_GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// The marker lines over the lanes: faint, in the colour of the marker.
    pub(super) fn paint_marker_lines(&self, c: &mut dyn Canvas, tl: Rect) {
        if !self.mk.strip {
            return;
        }
        let t = &self.st.base;
        for m in &self.song.markers {
            let x = self.axis.tick_to_x(m.tick).round() + 0.5;
            if x < tl.x0 || x > tl.x1 {
                continue;
            }
            let lit = self.mk.sel == Some(MarkSel::Marker(m.id))
                || self.hover == Some(Hit::MarkerLine(m.id));
            let col = color_of(m.color, t.highlight).with_alpha(if lit { 0.8 } else { 0.35 });
            sp::line(c, x, tl.y0, x, tl.y1, col);
        }
    }

    /// What is under `pos` in the strip.
    pub(super) fn strip_hit(&self, pos: Point) -> Hit {
        let row = self.markers_row();
        if row.contains(pos) {
            let hit = self
                .marker_rects()
                .into_iter()
                .rev()
                .find(|(_, r)| Rect::new(r.x0 - 4.0, r.y0, r.x1, r.y1).contains(pos))
                .map(|(id, _)| Hit::Marker(id));
            return hit.unwrap_or(Hit::MarkerStrip(true));
        }
        let hit = self
            .song
            .ranges
            .iter()
            .rev()
            .find(|r| self.range_geom(r).contains(pos))
            .map(|r| {
                let g = self.range_geom(r);
                let part = if g.width() < 3.0 * EDGE {
                    RangePart::Body
                } else if pos.x < g.x0 + EDGE {
                    RangePart::Start
                } else if pos.x > g.x1 - EDGE {
                    RangePart::End
                } else {
                    RangePart::Body
                };
                Hit::Range(r.id, part)
            });
        hit.unwrap_or(Hit::MarkerStrip(false))
    }

    fn list_rows(&self) -> Vec<ListRow> {
        let mut rows: Vec<ListRow> = self
            .song
            .markers
            .iter()
            .map(|m| ListRow {
                sel: MarkSel::Marker(m.id),
                tick: m.tick,
                end: None,
                name: m.name.clone(),
                color: m.color,
            })
            .chain(self.song.ranges.iter().map(|r| ListRow {
                sel: MarkSel::Range(r.id),
                tick: r.start,
                end: Some(r.end),
                name: r.name.clone(),
                color: r.color,
            }))
            .collect();
        rows.sort_by_key(|r| (r.tick, matches!(r.sel, MarkSel::Range(_)), r.sel));
        rows
    }

    fn list_body(&self) -> Rect {
        let l = self.marker_list_rect();
        Rect::new(l.x0, l.y0 + LIST_HEADER, l.x1, l.y1)
    }

    fn list_row_at(&self, i: usize) -> Option<Rect> {
        if !self.mk.list {
            return None;
        }
        let body = self.list_body();
        let y = body.y0 + i as f64 * LIST_ROW - self.mk.scroll;
        let r = Rect::new(body.x0, y, body.x1, y + LIST_ROW);
        (r.y0 >= body.y0 - 0.5 && r.y1 <= body.y1 + 0.5).then_some(r)
    }

    fn list_max_scroll(&self) -> f64 {
        let h = self.list_rows().len() as f64 * LIST_ROW;
        (h - self.list_body().height()).max(0.0)
    }

    /// What is under `pos` in the list.
    pub(super) fn list_hit(&self, pos: Point) -> ListHit {
        if self.list_add_marker_rect().contains(pos) {
            return ListHit::AddMarker;
        }
        if self.list_add_range_rect().contains(pos) {
            return ListHit::AddRange;
        }
        if !self.list_body().contains(pos) {
            return ListHit::Empty;
        }
        for (i, row) in self.list_rows().iter().enumerate() {
            let Some(r) = self.list_row_at(i) else {
                continue;
            };
            if !r.contains(pos) {
                continue;
            }
            return if pos.x >= r.x1 - 30.0 && pos.x < r.x1 - 10.0 {
                ListHit::Delete(row.sel)
            } else if pos.x >= r.x0 + 14.0 && pos.x < r.x0 + 114.0 {
                ListHit::Name(row.sel)
            } else {
                ListHit::Row(row.sel)
            };
        }
        ListHit::Empty
    }

    /// Scrolls the list with the wheel; true when `pos` is over it.
    pub(super) fn list_wheel(&mut self, dy: f64, pos: Point) -> bool {
        if !self.mk.list || !self.marker_list_rect().contains(pos) {
            return false;
        }
        let max = self.list_max_scroll();
        self.mk.scroll = (self.mk.scroll - dy * LIST_ROW * 2.0).clamp(0.0, max);
        true
    }

    /// Keeps the selection and the open edit pointing at markers that exist (after a sync).
    pub(super) fn marker_sync(&mut self) {
        if let Some(s) = self.mk.sel
            && !self.mark_exists(s)
        {
            self.mk.sel = None;
        }
        if let Some(t) = self.mk.rename.as_ref().map(|r| r.target)
            && !self.mark_exists(t)
        {
            self.mk.rename = None;
        }
        self.mk.ctx = self.mk.ctx.filter(|s| self.mark_exists(*s));
        let max = self.list_max_scroll();
        self.mk.scroll = self.mk.scroll.clamp(0.0, max);
    }

    // ------------------------------------------------------------------ edits

    fn set_markers(&self, markers: Vec<Marker>) -> UiAction {
        song_cmd(SongCommand::SetMarkers(markers))
    }

    fn set_ranges(&self, ranges: Vec<RangeMarker>) -> UiAction {
        song_cmd(SongCommand::SetRanges(ranges))
    }

    fn add_marker_cmd(&mut self, tick: i64) -> Option<UiAction> {
        let name = edit::auto_name("Marker", self.song.markers.iter().map(|m| m.name.as_str()));
        let (list, id) = edit::add_marker(&self.song.markers, tick, &name)?;
        self.mk.sel = Some(MarkSel::Marker(id));
        Some(self.set_markers(list))
    }

    fn add_range_cmd(&mut self, start: i64, end: i64) -> Option<UiAction> {
        let name = edit::auto_name("Range", self.song.ranges.iter().map(|r| r.name.as_str()));
        let (list, id) = edit::add_range(&self.song.ranges, start, end, &name)?;
        self.mk.sel = Some(MarkSel::Range(id));
        Some(self.set_ranges(list))
    }

    fn delete_mark_cmd(&self, sel: MarkSel) -> UiAction {
        match sel {
            MarkSel::Marker(id) => self.set_markers(edit::delete_marker(&self.song.markers, id)),
            MarkSel::Range(id) => self.set_ranges(edit::delete_range(&self.song.ranges, id)),
        }
    }

    fn color_cmd(&self, sel: MarkSel, color: Option<[u8; 3]>) -> UiAction {
        match sel {
            MarkSel::Marker(id) => {
                self.set_markers(edit::color_marker(&self.song.markers, id, color))
            }
            MarkSel::Range(id) => self.set_ranges(edit::color_range(&self.song.ranges, id, color)),
        }
    }

    fn loop_to_range_cmd(&self, id: u32) -> Option<UiAction> {
        let r = self.song.ranges.iter().find(|r| r.id == id)?;
        Some(song_cmd(SongCommand::SetLoop {
            range: (r.start, r.end),
            enabled: true,
        }))
    }

    fn clear_other_selection(&mut self) {
        self.selection.clear();
        self.selected_track = None;
        self.point_sel = None;
        self.tempo_sel = None;
        self.meter_sel = None;
    }

    // ------------------------------------------------------------------ rename

    fn mark_name(&self, sel: MarkSel) -> Option<&str> {
        match sel {
            MarkSel::Marker(id) => self
                .song
                .markers
                .iter()
                .find(|m| m.id == id)
                .map(|m| m.name.as_str()),
            MarkSel::Range(id) => self
                .song
                .ranges
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.name.as_str()),
        }
    }

    fn begin_rename(&mut self, sel: MarkSel, in_list: bool) -> bool {
        let Some(name) = self.mark_name(sel).map(str::to_owned) else {
            return false;
        };
        self.mk.rename = Some(Rename {
            target: sel,
            text: name,
            in_list,
        });
        self.mk.drag = None;
        self.drag = Drag::None;
        self.menu = None;
        self.repaint = true;
        true
    }

    /// Ends the edit; committing emits the rename when the name changed (an empty name
    /// cancels).
    fn finish_rename(&mut self, commit: bool, out: &mut Vec<UiAction>) {
        let Some(r) = self.mk.rename.take() else {
            return;
        };
        self.repaint = true;
        if !commit || self.mark_name(r.target) == edit::clean_name(&r.text).as_deref() {
            return;
        }
        let action = match r.target {
            MarkSel::Marker(id) => {
                edit::rename_marker(&self.song.markers, id, &r.text).map(|l| self.set_markers(l))
            }
            MarkSel::Range(id) => {
                edit::rename_range(&self.song.ranges, id, &r.text).map(|l| self.set_ranges(l))
            }
        };
        out.extend(action);
    }

    pub(super) fn rename_key(&mut self, key: &Key, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        match key {
            Key::Enter => self.finish_rename(true, out),
            Key::Escape => self.finish_rename(false, out),
            _ if modifiers.command() => {}
            Key::Backspace => {
                if let Some(r) = &mut self.mk.rename {
                    r.text.pop();
                }
            }
            Key::Space => self.rename_push(' '),
            Key::Char(c) if !c.is_control() => self.rename_push(*c),
            _ => {}
        }
        self.repaint = true;
    }

    fn rename_push(&mut self, c: char) {
        if let Some(r) = &mut self.mk.rename
            && r.text.chars().count() < MAX_MARKER_NAME
        {
            r.text.push(c);
        }
    }

    /// The rectangle of the name field being typed in.
    fn rename_rect(&self) -> Option<Rect> {
        let r = self.mk.rename.as_ref()?;
        if r.in_list {
            return self.list_name_rect(r.target);
        }
        let base = match r.target {
            MarkSel::Marker(id) => self.marker_rect(id)?,
            MarkSel::Range(id) => self.range_rect(id)?,
        };
        Some(Rect::new(
            base.x0,
            base.y0,
            base.x1.max(base.x0 + 96.0),
            base.y1,
        ))
    }

    /// A press while a name is typed: inside the field it keeps typing, anywhere else it
    /// commits the name. True when the press was inside the field (and is used up).
    pub(super) fn rename_press(&mut self, pos: Point, out: &mut Vec<UiAction>) -> bool {
        if self.mk.rename.is_none() {
            return false;
        }
        if self.rename_rect().is_some_and(|r| r.contains(pos)) {
            return true;
        }
        self.finish_rename(true, out);
        false
    }

    /// The view lost the focus: an open name edit is dropped.
    pub(super) fn marker_blur(&mut self) {
        self.cancel_marker_rename();
    }

    // ------------------------------------------------------------------ gestures

    /// A press on the strip away from a marker or range.
    #[allow(clippy::too_many_arguments, reason = "event fields are passed through")]
    pub(super) fn strip_down(
        &mut self,
        marker_row: bool,
        pos: Point,
        button: PointerButton,
        modifiers: Modifiers,
        clicks: u8,
        tool: Tool,
        out: &mut Vec<UiAction>,
    ) {
        self.clear_other_selection();
        self.mk.sel = None;
        let tick = self
            .grid(modifiers)
            .round_to(self.axis.x_to_tick(pos.x))
            .max(0);
        match button {
            PointerButton::Secondary => {
                self.mk.ctx = None;
                self.mk.ctx_tick = tick;
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::MarkerStrip,
                    at: pos,
                });
            }
            PointerButton::Primary if clicks >= 2 || tool == Tool::Pencil => {
                let action = if marker_row {
                    self.add_marker_cmd(tick)
                } else {
                    let len = self.bars.bar_ticks_at(tick);
                    self.add_range_cmd(tick, tick + len)
                };
                out.extend(action);
            }
            _ => {}
        }
    }

    /// A press on a marker: select it; drag moves it, a click locates the playhead on it, a
    /// double-click renames it, the eraser deletes it, the right button opens its menu.
    pub(super) fn marker_down(
        &mut self,
        id: u32,
        pos: Point,
        button: PointerButton,
        clicks: u8,
        tool: Tool,
        out: &mut Vec<UiAction>,
    ) {
        self.clear_other_selection();
        self.mk.sel = Some(MarkSel::Marker(id));
        let Some(m) = self.song.markers.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        match button {
            PointerButton::Secondary => {
                self.mk.ctx = Some(MarkSel::Marker(id));
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::Marker,
                    at: pos,
                });
            }
            PointerButton::Primary if tool == Tool::Eraser => {
                self.mk.sel = None;
                out.push(self.delete_mark_cmd(MarkSel::Marker(id)));
            }
            PointerButton::Primary if clicks >= 2 => {
                self.begin_rename(MarkSel::Marker(id), false);
            }
            PointerButton::Primary => {
                let merge = self.merge_id();
                self.mk.drag = Some(MarkDrag::Marker {
                    start: pos,
                    id,
                    grab: self.axis.x_to_tick(pos.x) - m.tick,
                    original: self.song.markers.clone(),
                    merge,
                    moved: false,
                    emitted: false,
                    last: self.song.markers.clone(),
                });
                self.drag = Drag::Mark;
            }
            PointerButton::Middle => {}
        }
    }

    /// A press on a range: select it; drag moves it (the edges resize it), a click sets the
    /// loop to it, a double-click renames it, the eraser deletes it, the right button opens
    /// its menu.
    #[allow(clippy::too_many_arguments, reason = "event fields are passed through")]
    pub(super) fn range_down(
        &mut self,
        id: u32,
        part: RangePart,
        pos: Point,
        button: PointerButton,
        clicks: u8,
        tool: Tool,
        out: &mut Vec<UiAction>,
    ) {
        self.clear_other_selection();
        self.mk.sel = Some(MarkSel::Range(id));
        let Some(r) = self.song.ranges.iter().find(|r| r.id == id).cloned() else {
            return;
        };
        match button {
            PointerButton::Secondary => {
                self.mk.ctx = Some(MarkSel::Range(id));
                out.push(UiAction::ContextMenu {
                    target: ContextTarget::MarkerRange,
                    at: pos,
                });
            }
            PointerButton::Primary if tool == Tool::Eraser => {
                self.mk.sel = None;
                out.push(self.delete_mark_cmd(MarkSel::Range(id)));
            }
            PointerButton::Primary if clicks >= 2 => {
                self.begin_rename(MarkSel::Range(id), false);
            }
            PointerButton::Primary => {
                let merge = self.merge_id();
                self.mk.drag = Some(MarkDrag::Range {
                    start: pos,
                    id,
                    part,
                    grab: self.axis.x_to_tick(pos.x) - r.start,
                    original: self.song.ranges.clone(),
                    merge,
                    moved: false,
                    emitted: false,
                    last: self.song.ranges.clone(),
                });
                self.drag = Drag::Mark;
            }
            PointerButton::Middle => {}
        }
    }

    /// The pointer moved during a marker or range drag.
    pub(super) fn marker_drag_move(&mut self, pos: Point, modifiers: Modifiers) -> Vec<UiAction> {
        let (axis, grid) = (self.axis, self.grid(modifiers));
        let min = seq_edit::min_length(&grid, MIN_TICKS);
        let mut out = Vec::new();
        match &mut self.mk.drag {
            Some(MarkDrag::Marker {
                start,
                id,
                grab,
                original,
                merge,
                moved,
                emitted,
                last,
            }) => {
                *moved |= pos.distance(*start) > DRAG_THRESHOLD;
                if !*moved {
                    return out;
                }
                let tick = grid.round_to(axis.x_to_tick(pos.x) - *grab).max(0);
                let new = edit::move_marker(original, *id, tick);
                if new != *last {
                    out.push(song_cmd(SongCommand::DragMarkers {
                        markers: new.clone(),
                        merge: *merge,
                    }));
                    *last = new;
                    *emitted = true;
                }
            }
            Some(MarkDrag::Range {
                start,
                id,
                part,
                grab,
                original,
                merge,
                moved,
                emitted,
                last,
            }) => {
                *moved |= pos.distance(*start) > DRAG_THRESHOLD;
                if !*moved {
                    return out;
                }
                let new = match part {
                    RangePart::Body => {
                        let s = grid.round_to(axis.x_to_tick(pos.x) - *grab).max(0);
                        edit::move_range(original, *id, s)
                    }
                    edge => {
                        let t = grid.round_to(axis.x_to_tick(pos.x));
                        edit::resize_range(original, *id, *edge, t, min)
                    }
                };
                if new != *last {
                    out.push(song_cmd(SongCommand::DragRanges {
                        ranges: new.clone(),
                        merge: *merge,
                    }));
                    *last = new;
                    *emitted = true;
                }
            }
            None => {}
        }
        self.repaint = true;
        out
    }

    /// The release of a marker or range drag: a drag ends its merged step; a click on a marker
    /// locates the playhead on it and a click on a range sets the loop to it.
    pub(super) fn marker_drag_up(&mut self, out: &mut Vec<UiAction>) {
        match self.mk.drag.take() {
            Some(MarkDrag::Marker {
                id,
                moved,
                emitted,
                original,
                ..
            }) => {
                if emitted {
                    out.push(UiAction::EndMerge);
                } else if !moved && let Some(m) = original.iter().find(|m| m.id == id) {
                    out.push(UiAction::Transport(TransportAction::Locate(m.tick)));
                }
            }
            Some(MarkDrag::Range {
                id,
                moved,
                emitted,
                original,
                ..
            }) => {
                if emitted {
                    out.push(UiAction::EndMerge);
                } else if !moved && let Some(r) = original.iter().find(|r| r.id == id) {
                    out.push(song_cmd(SongCommand::SetLoop {
                        range: (r.start, r.end),
                        enabled: true,
                    }));
                }
            }
            None => {}
        }
    }

    /// Escape during a drag: the original lists go back (one merged step, then the end of the
    /// drag).
    pub(super) fn marker_cancel_drag(&mut self, out: &mut Vec<UiAction>) {
        match self.mk.drag.take() {
            Some(MarkDrag::Marker {
                original,
                merge,
                emitted: true,
                ..
            }) => {
                out.push(song_cmd(SongCommand::DragMarkers {
                    markers: original,
                    merge,
                }));
                out.push(UiAction::EndMerge);
            }
            Some(MarkDrag::Range {
                original,
                merge,
                emitted: true,
                ..
            }) => {
                out.push(song_cmd(SongCommand::DragRanges {
                    ranges: original,
                    merge,
                }));
                out.push(UiAction::EndMerge);
            }
            _ => {}
        }
    }

    /// A press on the list.
    pub(super) fn list_down(
        &mut self,
        hit: ListHit,
        pos: Point,
        button: PointerButton,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        if button == PointerButton::Middle {
            return;
        }
        let row = match hit {
            ListHit::Delete(s) | ListHit::Name(s) | ListHit::Row(s) => Some(s),
            _ => None,
        };
        if let Some(sel) = row {
            self.clear_other_selection();
            self.mk.sel = Some(sel);
        }
        if button == PointerButton::Secondary {
            if let Some(sel) = row {
                self.mk.ctx = Some(sel);
                out.push(UiAction::ContextMenu {
                    target: match sel {
                        MarkSel::Marker(_) => ContextTarget::Marker,
                        MarkSel::Range(_) => ContextTarget::MarkerRange,
                    },
                    at: pos,
                });
            }
            return;
        }
        match hit {
            ListHit::AddMarker => {
                if let Some(CommandOutcome::Handled(a)) = self.add_marker_at_playhead() {
                    out.extend(a);
                }
            }
            ListHit::AddRange => {
                if let Some(CommandOutcome::Handled(a)) = self.add_range_from_loop() {
                    out.extend(a);
                }
            }
            ListHit::Delete(sel) => {
                self.mk.sel = None;
                out.push(self.delete_mark_cmd(sel));
            }
            ListHit::Name(sel) if clicks >= 2 => {
                self.begin_rename(sel, true);
            }
            ListHit::Name(sel) | ListHit::Row(sel) => self.locate_mark(sel, out),
            ListHit::Empty => {}
        }
    }

    /// Clicking a list row: a marker locates the playhead on it; a range sets the loop to it
    /// and locates the playhead on its start.
    fn locate_mark(&mut self, sel: MarkSel, out: &mut Vec<UiAction>) {
        match sel {
            MarkSel::Marker(id) => {
                if let Some(m) = self.song.markers.iter().find(|m| m.id == id) {
                    out.push(UiAction::Transport(TransportAction::Locate(m.tick)));
                }
            }
            MarkSel::Range(id) => {
                if let Some(a) = self.loop_to_range_cmd(id) {
                    out.push(a);
                }
                if let Some(r) = self.song.ranges.iter().find(|r| r.id == id) {
                    out.push(UiAction::Transport(TransportAction::Locate(r.start)));
                }
            }
        }
    }

    // ------------------------------------------------------------------ commands

    fn add_marker_at_playhead(&mut self) -> Option<CommandOutcome> {
        match self.add_marker_cmd(self.playhead.max(0)) {
            Some(a) => handled(vec![a]),
            None => Some(CommandOutcome::NotApplicable(
                "There is a marker at the playhead already".into(),
            )),
        }
    }

    fn add_range_from_loop(&mut self) -> Option<CommandOutcome> {
        let (a, z) = self.song.loop_range;
        match self.add_range_cmd(a, z) {
            Some(a) => handled(vec![a]),
            None => Some(CommandOutcome::NotApplicable(
                "The loop range cannot become a range marker".into(),
            )),
        }
    }

    /// Sets the song End at `tick`: one undoable command, declined where it would change
    /// nothing or put the End at the very start.
    fn set_end_at(&mut self, tick: i64) -> Option<CommandOutcome> {
        if tick <= 0 {
            return Some(CommandOutcome::NotApplicable(
                "The End marker cannot be at the very start".into(),
            ));
        }
        if tick == self.song.end_tick() {
            return Some(CommandOutcome::NotApplicable(
                "The End marker is there already".into(),
            ));
        }
        handled(vec![song_cmd(SongCommand::SetSongEnd(tick))])
    }

    /// Moves one loop locator to `tick`: one undoable command that keeps the loop's enabled
    /// flag, declined where the range would not change. The other locator gives way when the
    /// loop would get shorter than one snap step.
    fn set_locator_at(&mut self, right: bool, tick: i64) -> Option<CommandOutcome> {
        let min = seq_edit::min_length(self.step(Modifiers::NONE), MIN_TICKS);
        let range = locator_range(self.song.loop_range, right, tick.max(0), min);
        if range == self.song.loop_range {
            let side = if right { "right" } else { "left" };
            return Some(CommandOutcome::NotApplicable(format!(
                "The {side} locator is there already"
            )));
        }
        handled(vec![song_cmd(SongCommand::SetLoop {
            range,
            enabled: self.song.loop_enabled,
        })])
    }

    /// The marker commands: add, go to the previous and next marker, the items of the context
    /// menus (on the marker or range the menu opened on, else the selection) and Delete on a
    /// selected marker or range. `None` when `id` is not one of them.
    pub(super) fn marker_command(&mut self, id: &str, arg: Option<&str>) -> Option<CommandOutcome> {
        let target = self.mk.ctx.or(self.mk.sel);
        let result = match id {
            "transport.add_marker" => self.add_marker_at_playhead(),
            "transport.add_range_from_loop" => self.add_range_from_loop(),
            "transport.set_end_marker" => self.set_end_at(self.playhead),
            "sequencer.marker_end_here" => self.set_end_at(self.mk.ctx_tick),
            "transport.set_left_locator" => self.set_locator_at(false, self.playhead),
            "transport.set_right_locator" => self.set_locator_at(true, self.playhead),
            "sequencer.locator_left_here" => self.set_locator_at(false, self.mk.ctx_loop_tick),
            "sequencer.locator_right_here" => self.set_locator_at(true, self.mk.ctx_loop_tick),
            "transport.prev_marker" => match self.song.prev_marker_tick(self.playhead) {
                Some(t) => handled(vec![UiAction::Transport(TransportAction::Locate(t))]),
                None => Some(CommandOutcome::NotApplicable(
                    "There is no marker before the playhead".into(),
                )),
            },
            "transport.next_marker" => match self.song.next_marker_tick(self.playhead) {
                Some(t) => handled(vec![UiAction::Transport(TransportAction::Locate(t))]),
                None => Some(CommandOutcome::NotApplicable(
                    "There is no marker after the playhead".into(),
                )),
            },
            "sequencer.marker_add_here" => {
                let tick = self.mk.ctx_tick;
                match self.add_marker_cmd(tick) {
                    Some(a) => handled(vec![a]),
                    None => Some(CommandOutcome::NotApplicable(
                        "There is a marker here already".into(),
                    )),
                }
            }
            "sequencer.marker_range_from_selection" => match self.selection_span() {
                Some((a, z)) => match self.add_range_cmd(a, z) {
                    Some(a) => handled(vec![a]),
                    None => Some(CommandOutcome::NotApplicable(
                        "The selection cannot become a range marker".into(),
                    )),
                },
                None => Some(CommandOutcome::NotApplicable(
                    "Select clips to make a range marker from".into(),
                )),
            },
            "sequencer.marker_rename" => {
                self.begin_rename(target?, false);
                handled(Vec::new())
            }
            "sequencer.marker_delete" => {
                let sel = target?;
                self.mk.sel = None;
                handled(vec![self.delete_mark_cmd(sel)])
            }
            "sequencer.marker_color" => match (target, arg.and_then(edit::palette_color)) {
                (Some(sel), Some(color)) => handled(vec![self.color_cmd(sel, color)]),
                _ => return None,
            },
            "sequencer.marker_loop_range" => match target {
                Some(MarkSel::Range(id)) => {
                    handled(self.loop_to_range_cmd(id).into_iter().collect())
                }
                _ => return None,
            },
            "sequencer.marker_select_clips" => match target {
                Some(MarkSel::Range(id)) => {
                    let r = self.song.ranges.iter().find(|r| r.id == id)?;
                    let ids: Vec<ClipId> = self
                        .song_clips()
                        .filter(|c| c.start < r.end && c.start + c.length > r.start)
                        .map(|c| c.id)
                        .collect();
                    self.clear_other_selection();
                    self.selection.set(ids);
                    handled(Vec::new())
                }
                _ => return None,
            },
            "edit.delete" if self.mk.sel.is_some() => {
                let sel = self.mk.sel.take()?;
                handled(vec![self.delete_mark_cmd(sel)])
            }
            _ => return None,
        };
        if id.starts_with("sequencer.marker_") {
            self.mk.ctx = None;
        }
        result
    }

    // ------------------------------------------------------------------ painting

    /// The strip and its header cell.
    pub(super) fn paint_marker_strip(&self, c: &mut dyn Canvas) {
        if !self.mk.strip {
            return;
        }
        let st = &self.st;
        let t = &st.base;
        let strip = self.marker_strip_rect();
        let header = Rect::new(self.bounds.x0, strip.y0, strip.x0, strip.y1);
        sp::rect(c, header, st.header_lane);
        sp::label(
            c,
            "RANGES",
            header.x0 + 12.0,
            header.y0 + 11.0,
            8.0,
            t.text_muted,
            Align::Start,
        );
        sp::label(
            c,
            "MARKERS",
            header.x0 + 12.0,
            header.y0 + MARK_ROW + 11.0,
            8.0,
            t.text_muted,
            Align::Start,
        );
        c.push_clip(&strip.into());
        let (top, bottom) = (self.ranges_row(), self.markers_row());
        sp::rect(c, top, mix(st.ruler, st.background, 0.35));
        sp::rect(c, bottom, mix(st.ruler, st.background, 0.6));
        sp::line(
            c,
            strip.x0,
            bottom.y0 - 0.5,
            strip.x1,
            bottom.y0 - 0.5,
            st.separator,
        );
        let renaming = self
            .mk
            .rename
            .as_ref()
            .filter(|r| !r.in_list)
            .map(|r| r.target);
        for r in &self.song.ranges {
            let sel = MarkSel::Range(r.id);
            let g = self.range_geom(r);
            if g.x1 < strip.x0 || g.x0 > strip.x1 {
                continue;
            }
            let lit =
                self.mk.sel == Some(sel) || self.hover == Some(Hit::Range(r.id, RangePart::Body));
            let accent = color_of(r.color, t.focus);
            let fill = mix(
                accent,
                t.plate,
                if self.mk.sel == Some(sel) {
                    0.45
                } else if lit {
                    0.6
                } else {
                    0.72
                },
            );
            c.fill_with(RoundedRect::from_rect(g, 2.0), fill);
            c.fill_with(Rect::new(g.x0, g.y0, g.x1, g.y0 + 2.0), accent);
            for x in [g.x0 + 0.5, g.x1 - 0.5] {
                sp::line(c, x, g.y0, x, g.y1, accent);
            }
            if renaming != Some(sel) && g.width() > 14.0 {
                c.push_clip(&Rect::new(g.x0 + 3.0, g.y0, g.x1 - 3.0, g.y1).into());
                let text = fit_label(&r.name, g.width() - 8.0, 9.0);
                c.text(&TextRun::new(text, pt(g.x0 + 5.0, g.y1 - 3.5), 9.0, t.text).weight(600));
                c.pop_clip();
            }
        }
        for m in &self.song.markers {
            let sel = MarkSel::Marker(m.id);
            let Some(r) = self.marker_rect(m.id) else {
                continue;
            };
            if r.x1 < strip.x0 || r.x0 > strip.x1 {
                continue;
            }
            let selected = self.mk.sel == Some(sel);
            let lit = selected || self.hover == Some(Hit::Marker(m.id));
            let accent = color_of(m.color, t.highlight);
            let pill = Rect::new(r.x0, r.y0, r.x1, r.y1);
            c.fill_with(
                RoundedRect::from_rect(pill, 2.0),
                mix(
                    accent,
                    t.plate,
                    if selected {
                        0.4
                    } else if lit {
                        0.58
                    } else {
                        0.72
                    },
                ),
            );
            c.fill_with(Rect::new(r.x0, bottom.y0, r.x0 + 1.5, bottom.y1), accent);
            if selected {
                c.stroke_with(RoundedRect::from_rect(pill, 2.0), 1.0, t.text);
            }
            if renaming != Some(sel) && r.width() > 8.0 {
                c.push_clip(&Rect::new(r.x0 + 3.0, r.y0, r.x1 - 1.0, r.y1).into());
                c.text(&TextRun::new(
                    m.name.clone(),
                    pt(r.x0 + 5.0, r.y1 - 3.5),
                    9.0,
                    t.text,
                ));
                c.pop_clip();
            }
        }
        if let Some(rect) = self.rename_rect().filter(|_| renaming.is_some())
            && let Some(rn) = &self.mk.rename
        {
            name_field(c, st, rect, &rn.text);
        }
        let x = self.axis.tick_to_x(self.playhead);
        let color = if self.recording {
            st.record
        } else {
            st.playhead
        };
        sp::line(
            c,
            x.round() + 0.5,
            strip.y0,
            x.round() + 0.5,
            strip.y1,
            color,
        );
        c.pop_clip();
        sp::line(
            c,
            header.x0,
            strip.y1 - 0.5,
            strip.x1,
            strip.y1 - 0.5,
            st.separator,
        );
        sp::line(
            c,
            strip.x0 - 0.5,
            strip.y0,
            strip.x0 - 0.5,
            strip.y1,
            st.separator,
        );
    }

    /// The list docked at the right edge.
    pub(super) fn paint_marker_list(&self, c: &mut dyn Canvas) {
        if !self.mk.list {
            return;
        }
        let st = &self.st;
        let t = &st.base;
        let l = self.marker_list_rect();
        c.push_clip(&l.into());
        sp::rect(c, l, st.header);
        sp::line(c, l.x0 - 0.5, l.y0, l.x0 - 0.5, l.y1, st.separator);
        sp::label(
            c,
            "MARKERS",
            l.x0 + 12.0,
            l.y0 + 18.0,
            8.5,
            t.text_muted,
            Align::Start,
        );
        let hover_btn = |r: Rect| match &self.hover {
            Some(Hit::Panel(ListHit::AddMarker)) => r == self.list_add_marker_rect(),
            Some(Hit::Panel(ListHit::AddRange)) => r == self.list_add_range_rect(),
            _ => false,
        };
        let (am, ar) = (self.list_add_marker_rect(), self.list_add_range_rect());
        sp::button(c, st, am, "+ Marker", None, hover_btn(am));
        sp::button(c, st, ar, "+ Range", None, hover_btn(ar));
        let body = self.list_body();
        sp::line(c, l.x0, body.y0 - 0.5, l.x1, body.y0 - 0.5, st.separator);
        c.push_clip(&body.into());
        let meter = &self.bars;
        let rows = self.list_rows();
        if rows.is_empty() {
            sp::label(
                c,
                "No markers yet",
                l.x0 + 12.0,
                body.y0 + 18.0,
                9.0,
                t.text_muted,
                Align::Start,
            );
        }
        let renaming = self
            .mk
            .rename
            .as_ref()
            .filter(|r| r.in_list)
            .map(|r| r.target);
        for (i, row) in rows.iter().enumerate() {
            let y0 = body.y0 + i as f64 * LIST_ROW - self.mk.scroll;
            let r = Rect::new(l.x0, y0, l.x1, y0 + LIST_ROW);
            if r.y1 < body.y0 || r.y0 > body.y1 {
                continue;
            }
            let selected = self.mk.sel == Some(row.sel);
            let hovered = matches!(
                &self.hover,
                Some(Hit::Panel(ListHit::Row(s) | ListHit::Name(s) | ListHit::Delete(s))) if *s == row.sel
            );
            if selected {
                sp::rect(c, r, mix(t.highlight, t.plate, 0.78));
            } else if hovered {
                sp::rect(c, r, mix(t.text, t.plate, 0.92));
            }
            let accent = color_of(
                row.color,
                if matches!(row.sel, MarkSel::Range(_)) {
                    t.focus
                } else {
                    t.highlight
                },
            );
            let swatch = Rect::new(r.x0 + 5.0, r.y0 + 5.0, r.x0 + 9.0, r.y1 - 5.0);
            sp::rect(c, swatch, accent);
            if renaming != Some(row.sel) {
                sp::label(
                    c,
                    &fit_label(&row.name, 98.0, 10.0),
                    r.x0 + 14.0,
                    r.y0 + 15.0,
                    10.0,
                    t.text,
                    Align::Start,
                );
            }
            sp::label(
                c,
                &format_bbt(row.tick, meter),
                r.x0 + 118.0,
                r.y0 + 15.0,
                9.0,
                t.text_muted,
                Align::Start,
            );
            if let Some(end) = row.end {
                sp::label(
                    c,
                    &format_bbt(end, meter),
                    r.x0 + 184.0,
                    r.y0 + 15.0,
                    9.0,
                    t.text_muted,
                    Align::Start,
                );
            }
            let x = Rect::new(r.x1 - 30.0, r.y0, r.x1 - 10.0, r.y1);
            let lit = matches!(&self.hover, Some(Hit::Panel(ListHit::Delete(s))) if *s == row.sel);
            sp::label(
                c,
                "x",
                x.center().x,
                x.center().y + 3.5,
                10.0,
                if lit { t.text } else { t.text_muted },
                Align::Center,
            );
            sp::line(
                c,
                r.x0,
                r.y1 - 0.5,
                r.x1,
                r.y1 - 0.5,
                mix(st.separator, t.plate, 0.5),
            );
        }
        if let Some(rect) = self.rename_rect().filter(|_| renaming.is_some())
            && let Some(rn) = &self.mk.rename
        {
            name_field(c, st, rect, &rn.text);
        }
        c.pop_clip();
        c.pop_clip();
    }
}

/// The text field drawn over a label while its name is typed: the end of the text that fits,
/// and a caret.
pub(super) fn name_field(c: &mut dyn Canvas, st: &sp::SeqTheme, r: Rect, text: &str) {
    let t = &st.base;
    c.fill_with(RoundedRect::from_rect(r, 2.0), t.lcd);
    c.stroke_with(RoundedRect::from_rect(r, 2.0), 1.2, t.focus);
    let room = r.width() - 10.0;
    let mut shown: &str = text;
    while estimate_width(shown, 9.5) > room {
        let mut chars = shown.chars();
        chars.next();
        shown = chars.as_str();
    }
    let baseline = r.center().y + 3.5;
    c.text(&TextRun::new(shown, pt(r.x0 + 5.0, baseline), 9.5, t.text).weight(600));
    let cx = r.x0 + 5.0 + estimate_width(shown, 9.5);
    sp::line(c, cx + 1.0, r.y0 + 3.0, cx + 1.0, r.y1 - 3.0, t.focus);
}
