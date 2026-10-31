//! The arrangement inspector: a strip under the track lanes (View > Inspector) that edits the
//! selected clips numerically, whatever their kind.
//!
//! * **Position**, **Length**, the loop's **Offset** and **Loop** length are segmented fields
//!   (bar, beat, sixteenth, tick; see [`crate::segment_field`]): press a part and drag, scroll
//!   over it, or double-click to type. Typing `+240` adds 240 ticks, `-240` subtracts them,
//!   digits replace the part and `3.2.1.0` sets the whole field.
//! * With several clips selected a field shows a dash when they differ and an edit is
//!   **relative**: the change of the first clip's value is added to every selected clip, each
//!   clamped to its range (a clip starts at tick zero or later, a length is at least one tick).
//!   **Match** gives every selected clip the first clip's length, offset and loop.
//! * An audio clip has no loop offset (its source offset lives in its content), so that field
//!   leaves audio clips alone; its **Loop** length works unless the clip plays backwards.
//! * One typed value, wheel notch, arrow key or Match is one undo step (one batch); a drag is
//!   one merge group.
//!
//! The commands are the ones a clip drag emits (`SetClipGeometry`, `SetAudioClip`), so the
//! model validates every edit as it does a drag.

use super::{ArrangementView, Drag};
use crate::action::UiAction;
use crate::canvas::{Align, Canvas, TextRun, pt};
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::segment_field::{
    self as seg, EditKey, FIELD_WIDTH, FieldLook, FieldValue, SEGMENTS, SegmentDrag, SegmentEdit,
    SegmentField, SegmentMode,
};
use crate::seq_paint as sp;
use crate::step_drag::wheel_steps;
use atomik_model::song::MAX_TICK;
use atomik_model::{Clip, ClipContent, ClipId, Command, SongCommand};
use vello::kurbo::{Point, Rect};

/// Height of the strip in logical pixels.
pub const INSPECTOR_HEIGHT: f64 = 38.0;
/// Pixels between two fields.
pub(super) const GAP: f64 = 8.0;
/// Width of the Match button.
const MATCH_W: f64 = 54.0;
/// Height of a field.
const FIELD_H: f64 = 30.0;

/// A field of the strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipField {
    /// Where the clip starts.
    Position,
    /// How long the clip is.
    Length,
    /// The content tick shown at the clip start.
    Offset,
    /// The length of the loop (zero: no loop).
    Loop,
}

const FIELDS: [ClipField; 4] = [
    ClipField::Position,
    ClipField::Length,
    ClipField::Offset,
    ClipField::Loop,
];

impl ClipField {
    fn label(self) -> &'static str {
        match self {
            ClipField::Position => "POSITION",
            ClipField::Length => "LENGTH",
            ClipField::Offset => "LOOP OFFSET",
            ClipField::Loop => "LOOP LENGTH",
        }
    }

    /// The value on a clip, in ticks (no loop reads as zero).
    fn value(self, c: &Clip) -> i64 {
        match self {
            ClipField::Position => c.start,
            ClipField::Length => c.length,
            ClipField::Offset => c.offset,
            ClipField::Loop => c.loop_length.unwrap_or(0),
        }
    }

    /// The range an edit is clamped to.
    fn range(self) -> (i64, i64) {
        match self {
            ClipField::Position | ClipField::Offset | ClipField::Loop => (0, MAX_TICK),
            ClipField::Length => (1, MAX_TICK),
        }
    }

    /// An audio clip has no loop offset, and a reversed one cannot loop.
    fn applies(self, c: &Clip) -> bool {
        match (self, &c.content) {
            (ClipField::Position | ClipField::Length, _) => true,
            (ClipField::Offset, content) => !content.is_audio(),
            (ClipField::Loop, ClipContent::Audio(a)) => !a.reverse,
            (ClipField::Loop, _) => true,
        }
    }
}

/// A drag on one part of a field.
#[derive(Debug, Clone)]
struct InspDrag {
    field: ClipField,
    drag: SegmentDrag,
    /// The value of every selected clip when the drag started, and the first clip's as the
    /// base of the relative change: the document catching up never moves them.
    origs: Vec<(ClipId, i64)>,
    base: i64,
    merge: u64,
    changed: bool,
}

/// The inspector's view state.
#[derive(Debug, Default)]
pub(super) struct InspectorUi {
    pub visible: bool,
    drag: Option<InspDrag>,
    edit: Option<(ClipField, SegmentEdit)>,
}

impl InspectorUi {
    /// A value is being typed.
    pub(super) fn editing(&self) -> bool {
        self.edit.is_some()
    }
}

impl ArrangementView {
    // ------------------------------------------------------------------ host API

    /// Whether the inspector strip is shown under the track lanes.
    pub fn inspector_visible(&self) -> bool {
        self.insp.visible
    }

    /// Shows or hides the inspector strip.
    pub fn set_inspector(&mut self, on: bool) {
        if on != self.insp.visible {
            self.insp.visible = on;
            if !on {
                self.insp.edit = None;
                self.insp.drag = None;
            }
            self.apply_bounds();
            self.clamp_scroll();
            self.repaint = true;
        }
    }

    /// The strip's rectangle (empty when it is hidden).
    pub fn inspector_rect(&self) -> Rect {
        let top = if self.insp.visible {
            self.outer.y1 - INSPECTOR_HEIGHT
        } else {
            self.outer.y1
        };
        Rect::new(self.outer.x0, top, self.bounds.x1, self.outer.y1)
    }

    /// The rectangle of `field` (`None` when the strip is hidden or too narrow for it).
    pub fn inspector_field_rect(&self, field: ClipField) -> Option<Rect> {
        if !self.insp.visible {
            return None;
        }
        let i = FIELDS.iter().position(|f| *f == field)?;
        let strip = self.inspector_rect();
        let x0 = self.axis.x0 + GAP + i as f64 * (FIELD_WIDTH + GAP);
        let y0 = strip.y0 + (INSPECTOR_HEIGHT - FIELD_H) / 2.0;
        let r = Rect::new(x0, y0, x0 + FIELD_WIDTH, y0 + FIELD_H);
        (r.x1 <= strip.x1).then_some(r)
    }

    /// The centre of one part of `field` (0 bar, 1 beat, 2 sixteenth, 3 tick).
    pub fn inspector_part_point(&self, field: ClipField, part: usize) -> Option<Point> {
        let r = self.inspector_field_rect(field)?;
        Some(seg::segment_rects(r)[part.min(SEGMENTS - 1)].center())
    }

    /// The Match button's rectangle.
    pub fn inspector_match_rect(&self) -> Option<Rect> {
        if !self.insp.visible {
            return None;
        }
        let strip = self.inspector_rect();
        let x0 = self.axis.x0 + GAP + FIELDS.len() as f64 * (FIELD_WIDTH + GAP);
        let r = Rect::new(
            x0,
            strip.y0 + (INSPECTOR_HEIGHT - 20.0) / 2.0,
            x0 + MATCH_W,
            strip.y0 + (INSPECTOR_HEIGHT + 20.0) / 2.0,
        );
        (r.x1 <= strip.x1).then_some(r)
    }

    /// What `field` shows over the selected clips: the shared value, a dash when they differ,
    /// nothing without a clip it applies to.
    pub fn inspector_value(&self, field: ClipField) -> FieldValue {
        let mut values = self
            .selected_in_order()
            .into_iter()
            .filter(|c| field.applies(c))
            .map(|c| field.value(c));
        let Some(first) = values.next() else {
            return FieldValue::Empty;
        };
        if values.all(|v| v == first) {
            FieldValue::Value(first)
        } else {
            FieldValue::Mixed
        }
    }

    /// The text being typed into the inspector, if any.
    pub fn inspector_text(&self) -> Option<&str> {
        self.insp.edit.as_ref().map(|(_, e)| e.text.as_str())
    }

    // ------------------------------------------------------------------ model

    /// The widget of `field` over the first selected clip.
    fn insp_widget(&self, field: ClipField) -> SegmentField {
        let anchor = self.selected_in_order().first().map_or(0, |c| c.start);
        let mode = match field {
            ClipField::Position => SegmentMode::Position,
            _ => SegmentMode::Length { anchor },
        };
        SegmentField::new(&self.bars, mode)
    }

    /// The value of `field` on every selected clip it applies to, and the first clip's.
    fn insp_origs(&self, field: ClipField) -> (Vec<(ClipId, i64)>, i64) {
        let origs: Vec<(ClipId, i64)> = self
            .selected_in_order()
            .into_iter()
            .filter(|c| field.applies(c))
            .map(|c| (c.id, field.value(c)))
            .collect();
        let base = origs.first().map_or(0, |o| o.1);
        (origs, base)
    }

    /// The new value of each clip when `field` of the first clip becomes `new`: every clip moves
    /// by the same change, clamped to the field's range. A loop length nobody has yet is set
    /// outright.
    fn insp_plan(
        &self,
        field: ClipField,
        origs: &[(ClipId, i64)],
        base: i64,
        new: i64,
    ) -> Vec<(ClipId, i64)> {
        let (lo, hi) = field.range();
        let delta = new.saturating_sub(base);
        let fresh_loop = field == ClipField::Loop && origs.iter().all(|o| o.1 == 0);
        origs
            .iter()
            .filter_map(|&(id, v)| {
                if field == ClipField::Loop && !fresh_loop && v == 0 {
                    return None;
                }
                let value = if fresh_loop {
                    new
                } else {
                    v.saturating_add(delta)
                };
                let value = value.clamp(lo, hi);
                // A loop of zero is no loop.
                let value = if field == ClipField::Loop && value < 1 {
                    0
                } else {
                    value
                };
                Some((id, value))
            })
            .collect()
    }

    /// The command that gives a clip its new `field` value (`None` when nothing changes).
    fn insp_command(
        &self,
        c: &Clip,
        field: ClipField,
        value: i64,
        merge: Option<u64>,
    ) -> Option<Command> {
        if field.value(c) == value {
            return None;
        }
        let (start, length, offset, loop_length) = match field {
            ClipField::Position => (value, c.length, c.offset, c.loop_length),
            ClipField::Length => (c.start, value, c.offset, c.loop_length),
            ClipField::Offset => (c.start, c.length, value, c.loop_length),
            ClipField::Loop => (c.start, c.length, c.offset, (value > 0).then_some(value)),
        };
        Some(Command::Song(match &c.content {
            // The loop is not part of the audio command: it goes with the geometry.
            ClipContent::Audio(content) if field != ClipField::Loop => SongCommand::SetAudioClip {
                id: c.id,
                start,
                length,
                muted: c.muted,
                content: content.clone(),
                merge,
            },
            _ => SongCommand::SetClipGeometry {
                id: c.id,
                start,
                length,
                offset,
                loop_length,
                muted: c.muted,
                merge,
            },
        }))
    }

    /// The commands of a plan, one per clip it changes.
    fn insp_commands(
        &self,
        field: ClipField,
        plan: &[(ClipId, i64)],
        merge: Option<u64>,
    ) -> Vec<Command> {
        plan.iter()
            .filter_map(|&(id, value)| {
                let (_, _, c) = self.find_clip(id)?;
                self.insp_command(c, field, value, merge)
            })
            .collect()
    }

    /// One undo step: the command itself for one clip, a batch for several.
    fn insp_one_step(cmds: Vec<Command>) -> Option<UiAction> {
        let mut cmds = cmds;
        match cmds.len() {
            0 => None,
            1 => Some(UiAction::Command(cmds.remove(0))),
            _ => Some(UiAction::Command(Command::Batch(cmds))),
        }
    }

    /// Sets `field` of the first clip to `new` and moves the others by the same change, as one
    /// undo step.
    fn insp_set(&self, field: ClipField, new: i64) -> Option<UiAction> {
        let (origs, base) = self.insp_origs(field);
        let plan = self.insp_plan(field, &origs, base, new);
        Self::insp_one_step(self.insp_commands(field, &plan, None))
    }

    /// Match Values: every selected clip takes the first clip's length, offset and loop, as one
    /// undo step.
    pub fn inspector_match(&self) -> Option<UiAction> {
        let clips = self.selected_in_order();
        let first = *clips.first()?;
        let cmds: Vec<Command> = clips
            .iter()
            .skip(1)
            .filter_map(|c| {
                let audio = c.content.is_audio();
                let (offset, loop_length) = if audio {
                    (c.offset, c.loop_length)
                } else {
                    (first.offset, first.loop_length)
                };
                if (c.length, c.offset, c.loop_length) == (first.length, offset, loop_length) {
                    return None;
                }
                Some(Command::Song(match &c.content {
                    ClipContent::Audio(content) => SongCommand::SetAudioClip {
                        id: c.id,
                        start: c.start,
                        length: first.length,
                        muted: c.muted,
                        content: content.clone(),
                        merge: None,
                    },
                    _ => SongCommand::SetClipGeometry {
                        id: c.id,
                        start: c.start,
                        length: first.length,
                        offset,
                        loop_length,
                        muted: c.muted,
                        merge: None,
                    },
                }))
            })
            .collect();
        Self::insp_one_step(cmds)
    }

    // ------------------------------------------------------------------ events

    /// Takes the events the strip owns: presses, wheel turns and moves over it, the rest of a
    /// drag on a field, and every key while a value is typed. True when the event is consumed.
    pub(super) fn insp_event(&mut self, event: &UiEvent, out: &mut Vec<UiAction>) -> bool {
        // Selected points replace the clip fields; a gesture begun on one goes on.
        if self.inspector_shows_points() || self.pts.editing() || self.pts.dragging() {
            return self.pt_event(event, out);
        }
        match event {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                clicks,
                ..
            } if self.insp_contains(*pos) && matches!(self.drag, Drag::None) => {
                self.insp_press(*pos, *clicks, out);
                true
            }
            UiEvent::PointerDown { pos, .. } if self.insp.editing() => {
                // A press elsewhere ends typing and goes on to its own target.
                self.insp_commit(out);
                self.insp_contains(*pos)
            }
            UiEvent::PointerMove { pos, modifiers } if self.insp.drag.is_some() => {
                self.insp_drag_to(pos.y, *modifiers, out);
                true
            }
            UiEvent::PointerUp { .. } if self.insp.drag.is_some() => {
                self.insp_release(out);
                true
            }
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } if self.insp_contains(*pos) => {
                self.insp_wheel(delta.y, *pos, *modifiers, out);
                true
            }
            UiEvent::Key {
                key, pressed: true, ..
            } if self.insp.editing() => {
                self.insp_key(key, out);
                true
            }
            _ => false,
        }
    }

    /// Whether `pos` is over the strip.
    pub(super) fn insp_contains(&self, pos: Point) -> bool {
        self.insp.visible && self.inspector_rect().contains(pos)
    }

    fn insp_field_at(&self, pos: Point) -> Option<(ClipField, usize)> {
        FIELDS.into_iter().find_map(|f| {
            let r = self.inspector_field_rect(f)?;
            Some((f, seg::hit(r, pos)?))
        })
    }

    /// A field can be edited: a clip it applies to is selected.
    fn insp_enabled(&self, field: ClipField) -> bool {
        self.selected_in_order().iter().any(|c| field.applies(c))
    }

    /// A primary press in the strip.
    pub(super) fn insp_press(&mut self, pos: Point, clicks: u8, out: &mut Vec<UiAction>) {
        self.insp_commit(out);
        self.repaint = true;
        if self.inspector_match_rect().is_some_and(|r| r.contains(pos)) {
            out.extend(self.inspector_match());
            return;
        }
        let Some((field, part)) = self.insp_field_at(pos) else {
            return;
        };
        if !self.insp_enabled(field) {
            return;
        }
        let (origs, base) = self.insp_origs(field);
        if clicks >= 2 {
            let widget = self.insp_widget(field);
            let text = widget.split(base)[part].to_string();
            self.insp.edit = Some((field, SegmentEdit::new(part, text)));
            return;
        }
        let merge = self.merge_id();
        self.insp.drag = Some(InspDrag {
            field,
            drag: SegmentDrag::new(part, pos.y, base),
            origs,
            base,
            merge,
            changed: false,
        });
    }

    /// The pointer moved with a drag on an inspector field held.
    pub(super) fn insp_drag_to(&mut self, y: f64, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        let Some(mut d) = self.insp.drag.take() else {
            return;
        };
        let widget = self.insp_widget(d.field);
        // One clip stops at its own limit; with several the first clip's limit must not stop the
        // others, so each clip is clamped on its own when the plan is made.
        let (lo, hi) = if d.origs.len() > 1 {
            (i64::MIN, i64::MAX)
        } else {
            d.field.range()
        };
        if let Some(new) = d
            .drag
            .update(&widget, y, modifiers.shift, |v| v.clamp(lo, hi))
        {
            let plan = self.insp_plan(d.field, &d.origs, d.base, new);
            let cmds = self.insp_commands(d.field, &plan, Some(d.merge));
            d.changed |= !cmds.is_empty();
            out.extend(cmds.into_iter().map(UiAction::Command));
            self.repaint = true;
        }
        self.insp.drag = Some(d);
    }

    /// The release of a drag on an inspector field.
    pub(super) fn insp_release(&mut self, out: &mut Vec<UiAction>) {
        if let Some(d) = self.insp.drag.take() {
            if d.changed {
                out.push(UiAction::EndMerge);
            }
            self.repaint = true;
        }
    }

    /// One wheel event over the strip: one notch is one step of the part under the pointer.
    pub(super) fn insp_wheel(
        &self,
        delta_y: f64,
        pos: Point,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        let Some((field, part)) = self.insp_field_at(pos) else {
            return;
        };
        let n = wheel_steps(delta_y);
        if n == 0 || !self.insp_enabled(field) {
            return;
        }
        let (_, base) = self.insp_origs(field);
        let new = self.insp_widget(field).step(base, part, n, modifiers.shift);
        out.extend(self.insp_set(field, new));
    }

    /// Commits the text being typed.
    pub(super) fn insp_commit(&mut self, out: &mut Vec<UiAction>) {
        let Some((field, edit)) = self.insp.edit.take() else {
            return;
        };
        self.repaint = true;
        let (_, base) = self.insp_origs(field);
        if let Some(new) = self.insp_widget(field).parse(base, edit.part, &edit.text) {
            out.extend(self.insp_set(field, new));
        }
    }

    /// A key while a value is being typed.
    pub(super) fn insp_key(&mut self, key: &Key, out: &mut Vec<UiAction>) {
        let Some((field, mut edit)) = self.insp.edit.take() else {
            return;
        };
        self.repaint = true;
        match edit.key(key) {
            EditKey::Commit => {
                self.insp.edit = Some((field, edit));
                self.insp_commit(out);
                return;
            }
            EditKey::Cancel => return,
            EditKey::Step(n) => {
                let (_, base) = self.insp_origs(field);
                let widget = self.insp_widget(field);
                let new = widget.step(base, edit.part, n, false);
                out.extend(self.insp_set(field, new));
                let shown = new.clamp(field.range().0, field.range().1);
                edit = SegmentEdit::new(edit.part, widget.split(shown)[edit.part].to_string());
            }
            EditKey::Move(d) => {
                let part = (edit.part as i32 + d).clamp(0, SEGMENTS as i32 - 1) as usize;
                let (_, base) = self.insp_origs(field);
                let text = self.insp_widget(field).split(base)[part].to_string();
                edit = SegmentEdit::new(part, text);
            }
            EditKey::Typed | EditKey::Ignored => {}
        }
        self.insp.edit = Some((field, edit));
    }

    /// What a clip selection is called in the strip's left box.
    fn insp_title(&self) -> (&'static str, String) {
        let clips = self.selected_in_order();
        match clips.as_slice() {
            [] => ("INSPECTOR", "no clip selected".to_owned()),
            [c] => (
                match &c.content {
                    ClipContent::Audio(_) => "AUDIO CLIP",
                    ClipContent::Notes(_) => "NOTE CLIP",
                    _ => "AUTOMATION CLIP",
                },
                format!("clip {}", c.id.0),
            ),
            many => ("CLIPS", format!("{} clips", many.len())),
        }
    }

    // ------------------------------------------------------------------ paint

    /// Paints the strip.
    pub(super) fn paint_inspector(&self, c: &mut dyn Canvas) {
        if !self.insp.visible {
            return;
        }
        let strip = self.inspector_rect();
        let t = &self.st.base;
        sp::rect(c, strip, self.st.header);
        sp::line(
            c,
            strip.x0,
            strip.y0 + 0.5,
            strip.x1,
            strip.y0 + 0.5,
            self.st.separator,
        );
        let (tag, name) = self.insp_title();
        sp::label(
            c,
            tag,
            strip.x0 + 10.0,
            strip.y0 + 14.0,
            6.5,
            t.text_muted,
            Align::Start,
        );
        c.text(
            &TextRun::new(
                name,
                pt(strip.x0 + 10.0, strip.y0 + 29.0),
                11.0,
                t.highlight,
            )
            .weight(600),
        );
        if self.inspector_shows_points() {
            self.paint_point_fields(c);
            return;
        }
        for field in FIELDS {
            let Some(r) = self.inspector_field_rect(field) else {
                continue;
            };
            let widget = self.insp_widget(field);
            let editing = self.insp.edit.as_ref().filter(|(f, _)| *f == field);
            let part = editing.map(|(_, e)| e.part).or_else(|| {
                self.insp
                    .drag
                    .as_ref()
                    .filter(|d| d.field == field)
                    .map(|d| d.drag.part)
            });
            seg::paint(
                c,
                &self.st,
                &widget,
                r,
                self.inspector_value(field),
                FieldLook {
                    label: field.label(),
                    part,
                    typing: editing.map(|(_, e)| e.text.as_str()),
                    enabled: self.insp_enabled(field),
                },
            );
        }
        if let Some(r) = self.inspector_match_rect() {
            let on = self.selection.items().len() > 1;
            sp::button(c, &self.st, r, "MATCH", on.then_some(t.focus), false);
        }
    }

    /// The tooltip of the strip under `pos`.
    pub(super) fn insp_help(&self, pos: Point) -> Option<crate::tooltip::Help> {
        if !self.insp_contains(pos) {
            return None;
        }
        if self.inspector_shows_points() {
            return self.pt_help(pos);
        }
        if let Some(r) = self.inspector_match_rect().filter(|r| r.contains(pos)) {
            return Some(crate::tooltip::Help::new(
                r,
                "Match Values: give every selected clip the first clip's length, loop offset and loop length",
            ));
        }
        let (field, _) = self.insp_field_at(pos)?;
        let r = self.inspector_field_rect(field)?;
        let text = match field {
            ClipField::Position => {
                "Clip start as bar.beat.sixteenth.tick: drag or scroll a part (Shift: finer), double-click to type; +n and -n add or subtract ticks. With several clips the change applies to each"
            }
            ClipField::Length => {
                "Clip length in bars, beats, sixteenths and ticks: drag or scroll a part (Shift: finer), double-click to type; +n and -n add or subtract ticks"
            }
            ClipField::Offset => {
                "Where in its content the clip starts (note and automation clips; an audio clip keeps its source offset in its content)"
            }
            ClipField::Loop => {
                "Length of the loop that repeats inside the clip; zero turns the loop off (an audio clip that plays backwards cannot loop)"
            }
        };
        Some(crate::tooltip::Help::new(r, text))
    }
}
