//! The expression strip of the piano roll: the pitch, pressure and timbre curves of a note,
//! drawn in a strip beside the note and edited by hand.
//!
//! ```text
//! +----------------------------------------------------+
//! | PITCH  PRESS  TIMBRE   +-48     APPLY            x  |   tabs, pitch range, apply, clear
//! |  .----.                                            |
//! | /      '--.        *-------*                       |   the curve of the chosen tab
//! |           '--------'                               |
//! +----------------------------------------------------+
//! ```
//!
//! The strip belongs to the *edited note*: the lowest selected note, shown while that note
//! carries expression, or while the strip was opened with `editor.edit_expression` (then also for
//! a note without curves, and with several notes selected). Its three tabs pick the curve; the
//! plot spans the note's length. Every gesture is one undo step (one `SetClipMidi`, merged while
//! the pointer moves):
//!
//! * a click on empty space adds a point; a drag from empty space with the Select tool is a
//!   rubber band that selects the points inside it; with the Pencil tool it draws the curve
//!   freehand, thinned like Cleanup Automation;
//! * a drag on a point moves it (and the selected points with it); Shift moves finely (a tenth
//!   of the pointer's travel); Ctrl snaps a pitch to whole semitones; time stays inside the note
//!   and the value inside the range of the curve;
//! * Alt-click, a double click, a right click and the Eraser tool delete a point (every selected
//!   point when the point is one of them); Delete removes the selected points;
//! * APPLY (and `editor.apply_expression_curve`) copies the edited note's curve of the chosen tab
//!   to the other selected notes, stretched to the length of each.
//!
//! The pitch scale of the plot is plus or minus 48 semitones (the member range of MPE) and steps
//! through 24, 12 and 2 for fine bends; a pitch is held to the scale it is edited on.

use super::{DRAG_THRESHOLD, Drag, Hit, PianoRollView};
use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt, pt};
use crate::event::{Modifiers, PointerButton};
use crate::keymap::CommandOutcome;
use crate::seq_paint as sp;
use crate::tooltip::Help;
use crate::viewport::Tool;
use atomik_model::expression_edit as ee;
use atomik_model::{ExpressionKind, ExpressionPoint, MidiData};
use std::collections::BTreeSet;
use vello::kurbo::{Point, Rect};

/// Height of the strip's header row (tabs and buttons).
const HEAD: f64 = 16.0;
/// Height of the plot area including its margins.
const BODY: f64 = 64.0;
/// The narrowest the strip is drawn (a short note gets a strip wider than itself).
const MIN_WIDTH: f64 = 240.0;
/// Side of the clear button.
const CLEAR: f64 = 12.0;
/// Width of a tab.
const TAB_W: f64 = 44.0;
/// Height of a header button.
const BTN_H: f64 = 12.0;
/// How near (in pixels, on both axes) a press grabs a point.
const GRAB: f64 = 7.0;
/// Half the side of a point's square.
const DOT: f64 = 2.5;
/// The pitch scales the range chip steps through (semitones either way).
const RANGES: [f32; 4] = [48.0, 24.0, 12.0, 2.0];
/// The most points a freehand stroke keeps (a long slow stroke is cut, not slowed down).
const MAX_STROKE: usize = 8192;

/// Which part of the strip a pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExprHit {
    /// The tab of a curve.
    Tab(ExpressionKind),
    /// The pitch scale chip.
    Range,
    /// The button that copies the curve to the other selected notes.
    Apply,
    /// The button that clears every curve of the note.
    Clear,
    /// The plot.
    Body,
}

/// The state of the strip that outlives a gesture.
#[derive(Debug, Clone)]
pub(super) struct ExprUi {
    /// The strip was opened by the command (it shows without curves too).
    pub open: bool,
    /// The curve being edited.
    pub kind: ExpressionKind,
    /// The pitch scale: semitones above and below the middle of the plot.
    pub range: f32,
    /// The selected points, valid for the note and curve in `sel_for`.
    sel: BTreeSet<usize>,
    sel_for: Option<(i64, u8, ExpressionKind)>,
    /// The points own Delete and Select All: set by a press in the plot, cleared by a press
    /// anywhere else.
    pub focus: bool,
}

impl Default for ExprUi {
    fn default() -> Self {
        Self {
            open: false,
            kind: ExpressionKind::Pitch,
            range: RANGES[0],
            sel: BTreeSet::new(),
            sel_for: None,
            focus: false,
        }
    }
}

/// One gesture in the plot.
#[derive(Debug, Clone)]
pub(super) struct ExprDrag {
    /// The note (start tick, key), its length in ticks and the curve being edited.
    note: (i64, u8),
    length: i64,
    kind: ExpressionKind,
    /// The curve and the clip's data at the press: every step of the gesture derives from them.
    curve: Vec<ExpressionPoint>,
    midi: MidiData,
    action: Action,
    start: Point,
    merge: u64,
    moved: bool,
    emitted: bool,
    /// The Pencil tool was in use at the press.
    pencil: bool,
}

#[derive(Debug, Clone)]
enum Action {
    /// A press on empty space: a click adds a point, a drag becomes a band or a stroke.
    Pending,
    /// A point (and the selected ones with it) follows the pointer.
    Move { group: Vec<usize>, grabbed: usize },
    /// A rubber band; the points inside it join `base`.
    Band { base: BTreeSet<usize>, pos: Point },
    /// A freehand stroke of `(tick, value)` pairs.
    Stroke { points: Vec<(i64, f32)> },
}

/// Where the parts of the strip are.
#[derive(Debug, Clone, Copy)]
pub(super) struct ExprLayout {
    /// Index of the edited note.
    pub index: usize,
    pub strip: Rect,
    pub tabs: [Rect; 3],
    /// The pitch scale chip (only the pitch tab has one).
    pub range: Option<Rect>,
    /// The apply button (only with several notes selected).
    pub apply: Option<Rect>,
    pub clear: Rect,
    /// The area the curve is drawn and edited in.
    pub plot: Rect,
}

impl PianoRollView {
    // ------------------------------------------------------------------ geometry

    /// The index of the edited note (module docs).
    pub(super) fn expr_index(&self) -> Option<usize> {
        if self.track.is_some() {
            return None;
        }
        let clip = self.clip.as_ref()?;
        let items = self.selection.items();
        let first = *items.iter().next()?;
        let n = self.notes().get(first)?;
        let has = clip.midi.expression_of(n.tick, n.key).is_some();
        (self.expr.open || (items.len() == 1 && has)).then_some(first)
    }

    pub(super) fn expr_layout(&self) -> Option<ExprLayout> {
        let index = self.expr_index()?;
        let g = self.grid_rect();
        let r = self.rect_of(&self.notes()[index]);
        let w = r.width().max(MIN_WIDTH).min(g.width());
        let h = HEAD + BODY;
        let y0 = if r.y1 + 2.0 + h <= g.y1 {
            r.y1 + 2.0
        } else if r.y0 - 2.0 - h >= g.y0 {
            r.y0 - 2.0 - h
        } else {
            (g.y1 - h).max(g.y0)
        };
        let x0 = r.x0.clamp(g.x0, (g.x1 - w).max(g.x0));
        let strip = Rect::new(x0, y0, x0 + w, y0 + h);
        let tab = |i: usize| {
            let x = strip.x0 + 3.0 + i as f64 * (TAB_W + 2.0);
            Rect::new(x, strip.y0 + 2.0, x + TAB_W, strip.y0 + 2.0 + BTN_H)
        };
        let tabs = [tab(0), tab(1), tab(2)];
        let range = (self.expr.kind == ExpressionKind::Pitch).then(|| {
            let x = tabs[2].x1 + 6.0;
            Rect::new(x, strip.y0 + 2.0, x + 40.0, strip.y0 + 2.0 + BTN_H)
        });
        let clear = Rect::new(
            strip.x1 - CLEAR - 2.0,
            strip.y0 + 2.0,
            strip.x1 - 2.0,
            strip.y0 + 2.0 + CLEAR,
        );
        let apply = (self.selection.items().len() > 1).then(|| {
            Rect::new(
                clear.x0 - 6.0 - 46.0,
                strip.y0 + 2.0,
                clear.x0 - 6.0,
                strip.y0 + 2.0 + BTN_H,
            )
        });
        let plot = Rect::new(
            strip.x0 + 6.0,
            strip.y0 + HEAD + 3.0,
            strip.x1 - 6.0,
            strip.y1 - 4.0,
        );
        Some(ExprLayout {
            index,
            strip,
            tabs,
            range,
            apply,
            clear,
            plot,
        })
    }

    /// The strip and its clear button, when there is a strip.
    pub fn expression_rects(&self) -> Option<(Rect, Rect)> {
        self.expr_layout().map(|l| (l.strip, l.clear))
    }

    /// The middle of the strip's clear button (for tests and tours).
    pub fn expression_clear_center(&self) -> Option<Point> {
        self.expr_layout().map(|l| l.clear.center())
    }

    /// The middle of the tab of `kind` (for tests and tours).
    pub fn expression_tab_center(&self, kind: ExpressionKind) -> Option<Point> {
        let i = ExpressionKind::ALL.iter().position(|k| *k == kind)?;
        self.expr_layout().map(|l| l.tabs[i].center())
    }

    /// The middle of the pitch scale chip (only while the pitch tab is chosen).
    pub fn expression_range_center(&self) -> Option<Point> {
        self.expr_layout()?.range.map(|r| r.center())
    }

    /// The middle of the apply button (only with several notes selected).
    pub fn expression_apply_center(&self) -> Option<Point> {
        self.expr_layout()?.apply.map(|r| r.center())
    }

    /// The plot of the strip.
    pub fn expression_plot(&self) -> Option<Rect> {
        self.expr_layout().map(|l| l.plot)
    }

    /// The curve being edited: its tab, and the points of the edited note's curve.
    pub fn expression_curve(&self) -> Option<(ExpressionKind, Vec<ExpressionPoint>)> {
        let l = self.expr_layout()?;
        Some((self.expr.kind, self.expr_points(l.index)))
    }

    /// The screen position of point `i` of the curve being edited.
    pub fn expression_point_center(&self, i: usize) -> Option<Point> {
        let l = self.expr_layout()?;
        let n = self.notes()[l.index];
        let p = *self.expr_points(l.index).get(i)?;
        Some(self.expr_pos(&l.plot, n.length, self.expr.kind, p))
    }

    /// The indices of the selected points of the curve being edited.
    pub fn expression_selection(&self) -> Vec<usize> {
        self.expr_layout()
            .map(|l| self.expr_sel(l.index).into_iter().collect())
            .unwrap_or_default()
    }

    /// True while the strip is open by the command (see `editor.edit_expression`).
    pub fn expression_open(&self) -> bool {
        self.expr.open
    }

    /// The pitch scale of the plot, in semitones above and below its middle.
    pub fn expression_range(&self) -> f32 {
        self.expr.range
    }

    /// The curve the strip edits.
    pub fn expression_kind(&self) -> ExpressionKind {
        self.expr.kind
    }

    fn expr_points(&self, index: usize) -> Vec<ExpressionPoint> {
        let (Some(clip), Some(n)) = (self.clip.as_ref(), self.notes().get(index)) else {
            return Vec::new();
        };
        clip.midi
            .expression_of(n.tick, n.key)
            .map(|e| e.curve(self.expr.kind).to_vec())
            .unwrap_or_default()
    }

    /// The selected points of the edited note's curve (the selection of another note or curve is
    /// not this one's).
    fn expr_sel(&self, index: usize) -> BTreeSet<usize> {
        let Some(n) = self.notes().get(index) else {
            return BTreeSet::new();
        };
        if self.expr.sel_for != Some((n.tick, n.key, self.expr.kind)) {
            return BTreeSet::new();
        }
        let len = self.expr_points(index).len();
        self.expr.sel.iter().copied().filter(|i| *i < len).collect()
    }

    fn set_expr_sel(&mut self, index: usize, sel: BTreeSet<usize>) {
        if let Some(n) = self.notes().get(index) {
            self.expr.sel_for = Some((n.tick, n.key, self.expr.kind));
        }
        self.expr.sel = sel;
    }

    /// The half-height of the plot in value units: the pitch scale, or half of `0..=1`.
    fn expr_span(kind: ExpressionKind, range: f32) -> f64 {
        match kind {
            ExpressionKind::Pitch => f64::from(range) * 2.0,
            _ => 1.0,
        }
    }

    /// Where the point `p` of a curve of `kind` is drawn in `plot`.
    fn expr_pos(
        &self,
        plot: &Rect,
        length: i64,
        kind: ExpressionKind,
        p: ExpressionPoint,
    ) -> Point {
        let x = plot.x0 + (p.tick as f64 / length.max(1) as f64).clamp(0.0, 1.0) * plot.width();
        let v = f64::from(p.value);
        let y = match kind {
            ExpressionKind::Pitch => {
                let half = f64::from(self.expr.range);
                (plot.y0 + plot.y1) / 2.0 - (v / half).clamp(-1.0, 1.0) * plot.height() / 2.0
            }
            _ => plot.y1 - v.clamp(0.0, 1.0) * plot.height(),
        };
        pt(x, y)
    }

    /// The tick and value under `pos` in `plot` (held inside the note and the scale).
    fn expr_at(&self, plot: &Rect, length: i64, kind: ExpressionKind, pos: Point) -> (i64, f32) {
        let fx = ((pos.x - plot.x0) / plot.width().max(1.0)).clamp(0.0, 1.0);
        let tick = (fx * length as f64).round() as i64;
        let fy = ((plot.y1 - pos.y) / plot.height().max(1.0)).clamp(0.0, 1.0);
        let value = match kind {
            ExpressionKind::Pitch => ((fy * 2.0 - 1.0) * f64::from(self.expr.range)) as f32,
            _ => fy as f32,
        };
        (tick, value)
    }

    // ------------------------------------------------------------------ hits and help

    pub(super) fn expr_hit(&self, pos: Point) -> Option<ExprHit> {
        let l = self.expr_layout()?;
        if !l.strip.contains(pos) {
            return None;
        }
        if l.clear.contains(pos) {
            return Some(ExprHit::Clear);
        }
        if l.apply.is_some_and(|r| r.contains(pos)) {
            return Some(ExprHit::Apply);
        }
        if l.range.is_some_and(|r| r.contains(pos)) {
            return Some(ExprHit::Range);
        }
        for (i, kind) in ExpressionKind::ALL.into_iter().enumerate() {
            if l.tabs[i].contains(pos) {
                return Some(ExprHit::Tab(kind));
            }
        }
        Some(ExprHit::Body)
    }

    pub(super) fn expr_help(&self, h: ExprHit) -> Option<Help> {
        let l = self.expr_layout()?;
        Some(match h {
            ExprHit::Tab(kind) => {
                let i = ExpressionKind::ALL.iter().position(|k| *k == kind)?;
                Help::new(l.tabs[i], "Choose the curve to edit: pitch, pressure or timbre")
                    .id("roll.expr_tab")
            }
            ExprHit::Range => Help::new(
                l.range?,
                "Pitch scale of the plot in semitones: click for the next, right-click for the last",
            )
            .id("roll.expr_range"),
            ExprHit::Apply => Help::new(
                l.apply?,
                "Copy this curve to the other selected notes, stretched to each note",
            )
            .id("roll.expr_apply"),
            ExprHit::Clear => Help::new(
                l.clear,
                "Clear this note's pitch, pressure and timbre curves",
            )
            .id("roll.expr_clear"),
            ExprHit::Body => Help::new(
                l.plot,
                "Click to add a point, drag to move it, Alt-click to delete, drag empty space to select",
            )
            .id("roll.expr_curve"),
        })
    }

    // ------------------------------------------------------------------ gestures

    /// The point of `curve` nearest to `pos` within the grab distance.
    fn expr_grab(
        &self,
        l: &ExprLayout,
        length: i64,
        curve: &[ExpressionPoint],
        pos: Point,
    ) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (i, p) in curve.iter().enumerate() {
            let at = self.expr_pos(&l.plot, length, self.expr.kind, *p);
            let (dx, dy) = ((at.x - pos.x).abs(), (at.y - pos.y).abs());
            if dx <= GRAB && dy <= GRAB {
                let d = dx * dx + dy * dy;
                if best.is_none_or(|(_, b)| d < b) {
                    best = Some((i, d));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    /// A press on the strip.
    pub(super) fn expr_down(
        &mut self,
        h: ExprHit,
        pos: Point,
        button: PointerButton,
        modifiers: Modifiers,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        let primary = button == PointerButton::Primary;
        let Some(l) = self.expr_layout() else { return };
        self.expr.focus = h == ExprHit::Body;
        match h {
            ExprHit::Tab(kind) if primary => {
                if self.expr.kind != kind {
                    self.expr.kind = kind;
                    self.expr.sel.clear();
                    self.expr.sel_for = None;
                }
            }
            ExprHit::Range => {
                let at = RANGES
                    .iter()
                    .position(|r| (*r - self.expr.range).abs() < 1e-3)
                    .unwrap_or(0);
                let next = if primary {
                    (at + 1) % RANGES.len()
                } else {
                    (at + RANGES.len() - 1) % RANGES.len()
                };
                self.expr.range = RANGES[next];
            }
            ExprHit::Apply if primary => out.extend(self.apply_expression_curve()),
            ExprHit::Clear if primary => self.clear_expression(out),
            ExprHit::Body => self.expr_body_down(&l, pos, primary, modifiers, clicks, out),
            _ => {}
        }
    }

    fn expr_body_down(
        &mut self,
        l: &ExprLayout,
        pos: Point,
        primary: bool,
        modifiers: Modifiers,
        clicks: u8,
        out: &mut Vec<UiAction>,
    ) {
        let (Some(clip), Some(note)) = (self.clip.as_ref(), self.notes().get(l.index).copied())
        else {
            return;
        };
        let midi = (*clip.midi).clone();
        let kind = self.expr.kind;
        let curve = self.expr_points(l.index);
        let tool = self.vp.effective_tool(modifiers);
        let hit = self.expr_grab(l, note.length, &curve, pos);
        let key = (note.tick, note.key);
        if let Some(i) = hit
            && (!primary || modifiers.alt || tool == Tool::Eraser || clicks >= 2)
        {
            let sel = self.expr_sel(l.index);
            let gone: Vec<usize> = if sel.contains(&i) {
                sel.into_iter().collect()
            } else {
                vec![i]
            };
            let next = ee::remove_points(&curve, &gone);
            self.set_expr_sel(l.index, BTreeSet::new());
            if let Some(m) = ee::with_curve(&midi, key.0, key.1, kind, next) {
                out.extend(self.midi_command(m, None));
            }
            return;
        }
        if !primary {
            return;
        }
        let merge = self.merge_id();
        let action = match hit {
            Some(i) => {
                let mut sel = self.expr_sel(l.index);
                if !sel.contains(&i) {
                    sel = [i].into_iter().collect();
                }
                self.set_expr_sel(l.index, sel.clone());
                Action::Move {
                    group: sel.into_iter().collect(),
                    grabbed: i,
                }
            }
            None => Action::Pending,
        };
        self.drag = Drag::Expr(ExprDrag {
            note: key,
            length: note.length,
            kind,
            curve,
            midi,
            action,
            start: pos,
            merge,
            moved: false,
            emitted: false,
            pencil: tool == Tool::Pencil,
        });
    }

    /// A pointer move during a gesture in the plot.
    pub(super) fn expr_move(&mut self, pos: Point, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        let Some(l) = self.expr_layout() else { return };
        let range = self.expr.range;
        if !matches!(self.drag, Drag::Expr(_)) {
            return;
        }
        let Drag::Expr(mut d) = std::mem::replace(&mut self.drag, Drag::None) else {
            return;
        };
        self.repaint = true;
        let plot = l.plot;
        let kind = d.kind;
        let far = pos.distance(d.start) > DRAG_THRESHOLD;
        // A press on empty space turns into the band or the stroke once it travels.
        if matches!(d.action, Action::Pending) && far {
            d.moved = true;
            d.action = if d.pencil {
                let (t, v) = self.expr_at(&plot, d.length, kind, d.start);
                Action::Stroke {
                    points: vec![(t, v)],
                }
            } else {
                Action::Band {
                    base: if modifiers.shift {
                        self.expr_sel(l.index)
                    } else {
                        BTreeSet::new()
                    },
                    pos,
                }
            };
        }
        let mut new_sel: Option<BTreeSet<usize>> = None;
        let mut next: Option<Vec<ExpressionPoint>> = None;
        match &mut d.action {
            Action::Pending => {}
            Action::Move { group, grabbed } => {
                d.moved |= far;
                if d.moved {
                    let scale = if modifiers.shift { 0.1 } else { 1.0 };
                    let dx = (pos.x - d.start.x) * scale;
                    let dy = (pos.y - d.start.y) * scale;
                    let dtick = (dx / plot.width().max(1.0) * d.length as f64).round() as i64;
                    let mut dvalue =
                        (-dy / plot.height().max(1.0) * Self::expr_span(kind, range)) as f32;
                    if modifiers.command()
                        && kind == ExpressionKind::Pitch
                        && let Some(g) = d.curve.get(*grabbed)
                    {
                        // The grabbed point lands on a whole semitone; the others keep their
                        // distance from it.
                        dvalue = ee::snap_semitone(g.value + dvalue) - g.value;
                    }
                    let (curve, picked) =
                        ee::move_points(&d.curve, kind, d.length, group, dtick, dvalue);
                    new_sel = Some(picked.into_iter().collect());
                    next = Some(curve);
                }
            }
            Action::Band { base, pos: end } => {
                *end = pos;
                let band = Rect::from_points(d.start, pos);
                let inside = d
                    .curve
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| band.contains(self.expr_pos(&plot, d.length, kind, **p)))
                    .map(|(i, _)| i);
                new_sel = Some(base.iter().copied().chain(inside).collect());
            }
            Action::Stroke { points } => {
                let (t, v) = self.expr_at(&plot, d.length, kind, pos);
                if points.len() < MAX_STROKE && points.last() != Some(&(t, v)) {
                    points.push((t, v));
                    d.moved = true;
                    let (curve, drawn) =
                        ee::draw_stroke(&d.curve, kind, d.length, points, self.ctl_thinning);
                    new_sel = Some(drawn.into_iter().collect());
                    next = Some(curve);
                }
            }
        }
        if let Some(curve) = next
            && let Some(m) = ee::with_curve(&d.midi, d.note.0, d.note.1, kind, curve)
        {
            out.extend(self.midi_command(m, Some(d.merge)));
            d.emitted = true;
        }
        self.drag = Drag::Expr(d);
        if let Some(sel) = new_sel {
            self.set_expr_sel(l.index, sel);
        }
    }

    /// The release that ends a gesture in the plot.
    pub(super) fn expr_up(&mut self, d: ExprDrag, out: &mut Vec<UiAction>) {
        self.repaint = true;
        match d.action {
            // A click without a drag adds a point where it was pressed.
            Action::Pending => {
                let Some(l) = self.expr_layout() else { return };
                let (tick, value) = self.expr_at(&l.plot, d.length, d.kind, d.start);
                if let Some((curve, at)) = ee::set_point(&d.curve, d.kind, d.length, tick, value)
                    && let Some(m) = ee::with_curve(&d.midi, d.note.0, d.note.1, d.kind, curve)
                {
                    out.extend(self.midi_command(m, None));
                    self.set_expr_sel(l.index, [at].into_iter().collect());
                }
            }
            Action::Move { .. } | Action::Stroke { .. } => {
                if d.emitted {
                    out.push(UiAction::EndMerge);
                }
            }
            Action::Band { .. } => {}
        }
    }

    /// Escape during a gesture: the clip's data as it was at the press.
    pub(super) fn expr_cancel(&mut self, d: ExprDrag, out: &mut Vec<UiAction>) {
        if d.emitted {
            out.extend(self.midi_command(d.midi, Some(d.merge)));
            out.push(UiAction::EndMerge);
        }
        self.expr.sel.clear();
        self.expr.sel_for = None;
    }

    /// The rubber band of a gesture in progress, for painting.
    fn expr_band(&self) -> Option<Rect> {
        match &self.drag {
            Drag::Expr(ExprDrag {
                action: Action::Band { pos, .. },
                start,
                ..
            }) => Some(Rect::from_points(*start, *pos)),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ commands

    /// Clears the per-note expression of the edited note: one `SetClipMidi`, one undo step.
    pub(super) fn clear_expression(&mut self, out: &mut Vec<UiAction>) {
        let Some(l) = self.expr_layout() else { return };
        let (n, Some(clip)) = (self.notes()[l.index], self.clip.as_ref()) else {
            return;
        };
        if let Some(midi) = clip.midi.without_expression(n.tick, n.key) {
            self.expr.sel.clear();
            out.extend(self.midi_command(midi, None));
        }
    }

    /// Copies the edited note's curve (of the tab chosen) to the other selected notes, stretched
    /// to the length of each: one `SetClipMidi`, one undo step. `None` with fewer than two notes
    /// selected or when nothing would change.
    pub(super) fn apply_expression_curve(&mut self) -> Option<UiAction> {
        let clip = self.clip.as_ref()?;
        if self.track.is_some() || self.selection.items().len() < 2 {
            return None;
        }
        let notes = self.notes();
        let mut it = self.selection.items().iter();
        let source = notes.get(*it.next()?)?;
        let targets: Vec<(i64, u8, i64)> = it
            .filter_map(|i| notes.get(*i))
            .map(|n| (n.tick, n.key, n.length))
            .collect();
        let midi = ee::apply_curve(
            &clip.midi,
            (source.tick, source.key),
            source.length,
            self.expr.kind,
            &targets,
        )?;
        self.midi_command(midi, None)
    }

    /// The commands of the strip: `editor.edit_expression` shows or hides it,
    /// `editor.apply_expression_curve` copies a curve to the other selected notes, `edit.delete`
    /// and `edit.select_all` work on the points while the plot has the focus.
    pub(super) fn expr_command(&mut self, id: &str) -> Option<CommandOutcome> {
        match id {
            "editor.edit_expression" => {
                if self.track.is_some() || (self.selection.is_empty() && !self.expr.open) {
                    return Some(CommandOutcome::Declined);
                }
                self.expr.open = !self.expr.open;
                Some(CommandOutcome::Handled(Vec::new()))
            }
            "editor.apply_expression_curve" => Some(match self.apply_expression_curve() {
                Some(a) => CommandOutcome::Handled(vec![a]),
                None => CommandOutcome::Declined,
            }),
            "edit.select_all" if self.expr_focused() => {
                let l = self.expr_layout()?;
                let all = (0..self.expr_points(l.index).len()).collect();
                self.set_expr_sel(l.index, all);
                Some(CommandOutcome::Handled(Vec::new()))
            }
            "edit.delete" if self.expr_focused() => {
                let l = self.expr_layout()?;
                let (n, Some(clip)) = (self.notes()[l.index], self.clip.as_ref()) else {
                    return Some(CommandOutcome::Declined);
                };
                let sel: Vec<usize> = self.expr_sel(l.index).into_iter().collect();
                let next = ee::remove_points(&self.expr_points(l.index), &sel);
                let midi = ee::with_curve(&clip.midi, n.tick, n.key, self.expr.kind, next)?;
                self.set_expr_sel(l.index, BTreeSet::new());
                Some(CommandOutcome::Handled(
                    self.midi_command(midi, None).into_iter().collect(),
                ))
            }
            _ => None,
        }
    }

    /// The plot has the focus and some of its points are selected.
    fn expr_focused(&self) -> bool {
        self.expr.focus
            && self
                .expr_layout()
                .is_some_and(|l| !self.expr_sel(l.index).is_empty())
    }

    // ------------------------------------------------------------------ painting

    /// The strip: tabs, buttons and the plot with the curve of the tab chosen (the others are
    /// drawn thin behind it).
    pub(super) fn paint_expression(&self, c: &mut dyn Canvas) {
        let Some(l) = self.expr_layout() else { return };
        let st = &self.st;
        let t = &st.base;
        let note = self.notes()[l.index];
        let sel = self.expr_sel(l.index);
        sp::rect(c, l.strip, st.lane.with_alpha(0.96));
        let header = Rect::new(l.strip.x0, l.strip.y0, l.strip.x1, l.strip.y0 + HEAD);
        sp::rect(c, header, st.header);
        sp::line(
            c,
            header.x0,
            header.y1 - 0.5,
            header.x1,
            header.y1 - 0.5,
            st.separator,
        );
        c.stroke_with(l.strip, 1.0, t.bevel_dark);
        let hovered = |h: ExprHit| self.hover == Hit::Expr(h);
        for (i, kind) in ExpressionKind::ALL.into_iter().enumerate() {
            let active = kind == self.expr.kind;
            let r = l.tabs[i];
            sp::lcd(
                c,
                st,
                r,
                (active || hovered(ExprHit::Tab(kind))).then_some(if active {
                    t.highlight
                } else {
                    t.focus
                }),
            );
            let label = match kind {
                ExpressionKind::Pitch => "PITCH",
                ExpressionKind::Pressure => "PRESS",
                ExpressionKind::Timbre => "TIMBRE",
            };
            let has = self
                .clip
                .as_ref()
                .and_then(|clip| clip.midi.expression_of(note.tick, note.key))
                .is_some_and(|e| !e.curve(kind).is_empty());
            let colour = if active {
                t.highlight
            } else if has {
                t.text
            } else {
                t.text_muted
            };
            sp::label(
                c,
                label,
                r.center().x,
                r.center().y + 3.0,
                8.5,
                colour,
                Align::Center,
            );
        }
        if let Some(r) = l.range {
            sp::lcd(c, st, r, hovered(ExprHit::Range).then_some(t.focus));
            let text = format!("+-{}", self.expr.range as i32);
            sp::label(
                c,
                &text,
                r.center().x,
                r.center().y + 3.0,
                8.5,
                t.text,
                Align::Center,
            );
        }
        if let Some(r) = l.apply {
            sp::button(c, st, r, "APPLY", None, hovered(ExprHit::Apply));
        }
        let b = l.clear.inset(-3.0);
        let colour = if hovered(ExprHit::Clear) {
            t.text
        } else {
            t.text_muted
        };
        c.stroke_with(
            vello::kurbo::Line::new(pt(b.x0, b.y0), pt(b.x1, b.y1)),
            1.2,
            colour,
        );
        c.stroke_with(
            vello::kurbo::Line::new(pt(b.x0, b.y1), pt(b.x1, b.y0)),
            1.2,
            colour,
        );
        // The plot: a dark ground, the middle line of the pitch, quarter lines.
        sp::rect(c, l.plot, st.background);
        c.push_clip(&l.plot.into());
        let quarter = |f: f64| l.plot.y0 + f * l.plot.height();
        for f in [0.25, 0.75] {
            sp::line(c, l.plot.x0, quarter(f), l.plot.x1, quarter(f), st.grid_sub);
        }
        sp::line(
            c,
            l.plot.x0,
            quarter(0.5),
            l.plot.x1,
            quarter(0.5),
            st.grid_beat,
        );
        let e = self
            .clip
            .as_ref()
            .and_then(|clip| clip.midi.expression_of(note.tick, note.key));
        for kind in ExpressionKind::ALL {
            let Some(points) = e.map(|e| e.curve(kind)).filter(|p| !p.is_empty()) else {
                continue;
            };
            let main = kind == self.expr.kind;
            let colour = match kind {
                ExpressionKind::Pitch => st.selection,
                ExpressionKind::Pressure => t.text,
                ExpressionKind::Timbre => t.text_muted,
            };
            let at = |p: ExpressionPoint| self.expr_pos(&l.plot, note.length, kind, p);
            let mut path = vello::kurbo::BezPath::new();
            // Held from the note's start to the first point and from the last point to its end.
            path.move_to(at(ExpressionPoint::new(0, points[0].value)));
            for p in points {
                path.line_to(at(*p));
            }
            if let Some(last) = points.last() {
                path.line_to(at(ExpressionPoint::new(note.length, last.value)));
            }
            c.stroke_with(
                path,
                if main { 1.6 } else { 0.8 },
                if main {
                    colour
                } else {
                    colour.with_alpha(0.45)
                },
            );
        }
        if let Some(points) = e.map(|e| e.curve(self.expr.kind)) {
            for (i, p) in points.iter().enumerate() {
                let at = self.expr_pos(&l.plot, note.length, self.expr.kind, *p);
                let r = Rect::new(at.x - DOT, at.y - DOT, at.x + DOT, at.y + DOT);
                let picked = sel.contains(&i);
                sp::rect(c, r, if picked { t.highlight } else { t.text });
                c.stroke_with(r, 1.0, t.bevel_dark);
            }
        }
        if let Some(band) = self.expr_band() {
            c.stroke_with(band, 1.0, t.highlight);
        }
        c.pop_clip();
    }
}
