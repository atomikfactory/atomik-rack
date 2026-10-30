//! The note inspector: a strip under the piano roll (View > Inspector, the switch that also
//! shows the arrangement's clip strip) that edits the selected notes numerically.
//!
//! * **Note** is the pitch (`C#3`): drag, scroll (Shift: an octave) or double-click to type a
//!   name (`C#3`, `a4`), a MIDI number, or `+n` and `-n` semitones.
//! * **Position** and **Length** are segmented `bar.beat.16th.tick` fields
//!   (see [`crate::segment_field`]) counted in the clip's own ticks; **Velocity** is 1 to 127.
//! * With several notes selected a field shows a dash where they differ and an edit is
//!   **relative**: the change of the first selected note's value is added to every selected
//!   note, each clamped on its own (pitch 0..127, velocity 1..127, length at least one tick,
//!   position within the song's range).
//! * A typed value, a wheel notch or an arrow key is one `SetClipContent` (one undo step); a
//!   drag is one merge group closed by `EndMerge`. Values outside the clip are accepted: the
//!   notes are masked as they are by every other edit.

use super::{Note, PianoRollView, UiAction};
use crate::canvas::{Align, Canvas, TextRun, pt};
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::segment_field::{
    self as seg, EditKey, FIELD_WIDTH, FieldLook, FieldValue, SEGMENTS, SegmentEdit, SegmentField,
    SegmentMode,
};
use crate::seq_edit::normalize_notes;
use crate::seq_paint as sp;
use crate::step_drag::{PX_PER_STEP, StepDrag, wheel_steps};
use crate::zone_edit::note_name;
use atomik_model::parse_note_name;
use atomik_model::song::MAX_TICK;
use std::collections::BTreeSet;
use vello::kurbo::{Point, Rect};

/// Height of the strip in logical pixels.
pub const NOTE_INSPECTOR_HEIGHT: f64 = 38.0;
const GAP: f64 = 8.0;
const FIELD_H: f64 = 30.0;
const PITCH_W: f64 = 76.0;
const VELOCITY_W: f64 = 64.0;
/// Left box with what is selected.
const TITLE_W: f64 = 120.0;

/// A field of the note strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteField {
    /// The pitch.
    Pitch,
    /// Where the note starts in the clip.
    Position,
    /// How long the note is.
    Length,
    /// Velocity, 1 to 127.
    Velocity,
}

const FIELDS: [NoteField; 4] = [
    NoteField::Pitch,
    NoteField::Position,
    NoteField::Length,
    NoteField::Velocity,
];

impl NoteField {
    fn label(self) -> &'static str {
        match self {
            NoteField::Pitch => "NOTE",
            NoteField::Position => "POSITION",
            NoteField::Length => "LENGTH",
            NoteField::Velocity => "VELOCITY",
        }
    }

    fn width(self) -> f64 {
        match self {
            NoteField::Pitch => PITCH_W,
            NoteField::Velocity => VELOCITY_W,
            _ => FIELD_WIDTH,
        }
    }

    fn segmented(self) -> bool {
        matches!(self, NoteField::Position | NoteField::Length)
    }

    /// The value on a note: the key, ticks, ticks or velocity out of 127.
    fn value(self, n: &Note) -> i64 {
        match self {
            NoteField::Pitch => i64::from(n.key),
            NoteField::Position => n.tick,
            NoteField::Length => n.length,
            NoteField::Velocity => (f64::from(n.velocity) * 127.0).round() as i64,
        }
    }

    /// `n` with the field set to `v` (clamped to the field's range).
    fn set(self, n: &Note, v: i64) -> Note {
        let mut n = *n;
        match self {
            NoteField::Pitch => n.key = v.clamp(0, 127) as u8,
            NoteField::Position => n.tick = v.clamp(0, MAX_TICK),
            NoteField::Length => n.length = v.clamp(1, MAX_TICK),
            NoteField::Velocity => n.velocity = v.clamp(1, 127) as f32 / 127.0,
        }
        n
    }
}

/// A drag on a field.
#[derive(Debug, Clone)]
struct NoteDrag {
    field: NoteField,
    part: usize,
    steps: StepDrag,
    value: i64,
    sent: i64,
    /// The clip's notes and the selected ones when the drag started: the document catching up
    /// never moves them.
    notes: Vec<Note>,
    sel: BTreeSet<usize>,
    base: i64,
    merge: u64,
    changed: bool,
}

/// The strip's view state.
#[derive(Debug, Default)]
pub(super) struct NoteInspectorUi {
    pub visible: bool,
    drag: Option<NoteDrag>,
    edit: Option<(NoteField, SegmentEdit)>,
}

impl NoteInspectorUi {
    pub(super) fn editing(&self) -> bool {
        self.edit.is_some()
    }
}

impl PianoRollView {
    // ------------------------------------------------------------------ host API

    /// Whether the note inspector strip is shown under the roll.
    pub fn inspector_visible(&self) -> bool {
        self.insp.visible
    }

    /// Shows or hides the note inspector strip.
    pub fn set_inspector(&mut self, on: bool) {
        if on != self.insp.visible {
            self.insp.visible = on;
            if !on {
                self.insp.edit = None;
                self.insp.drag = None;
            }
            let outer = self.outer;
            self.set_bounds(outer);
            self.repaint = true;
        }
    }

    /// The strip's rectangle (empty when it is hidden).
    pub fn inspector_rect(&self) -> Rect {
        let top = if self.insp.visible {
            (self.outer.y1 - NOTE_INSPECTOR_HEIGHT).max(self.outer.y0)
        } else {
            self.outer.y1
        };
        Rect::new(self.outer.x0, top, self.outer.x1, self.outer.y1)
    }

    /// The rectangle of `field` (`None` when the strip is hidden or too narrow for it).
    pub fn inspector_field_rect(&self, field: NoteField) -> Option<Rect> {
        if !self.insp.visible {
            return None;
        }
        let strip = self.inspector_rect();
        let mut x = strip.x0 + 10.0 + TITLE_W + GAP;
        for f in FIELDS {
            if f == field {
                let y0 = strip.y0 + (NOTE_INSPECTOR_HEIGHT - FIELD_H) / 2.0;
                let r = Rect::new(x, y0, x + f.width(), y0 + FIELD_H);
                return (r.x1 <= strip.x1).then_some(r);
            }
            x += f.width() + GAP;
        }
        None
    }

    /// The centre of one part of a segmented field (0 bar .. 3 tick), or of a plain field.
    pub fn inspector_part_point(&self, field: NoteField, part: usize) -> Option<Point> {
        let r = self.inspector_field_rect(field)?;
        Some(if field.segmented() {
            seg::segment_rects(r)[part.min(SEGMENTS - 1)].center()
        } else {
            r.center()
        })
    }

    /// What `field` shows over the selected notes.
    pub fn inspector_value(&self, field: NoteField) -> FieldValue {
        let notes = self.notes();
        let mut values = self
            .selection
            .items()
            .iter()
            .filter_map(|&i| notes.get(i))
            .map(|n| field.value(n));
        let Some(first) = values.next() else {
            return FieldValue::Empty;
        };
        if values.all(|v| v == first) {
            FieldValue::Value(first)
        } else {
            FieldValue::Mixed
        }
    }

    /// The text being typed into the strip, if any.
    pub fn inspector_text(&self) -> Option<&str> {
        self.insp.edit.as_ref().map(|(_, e)| e.text.as_str())
    }

    /// Whether the view owns the keyboard as a text field: a value of the strip is being typed.
    pub fn is_text_owner(&self) -> bool {
        self.insp.editing()
    }

    // ------------------------------------------------------------------ model

    fn note_widget(&self, field: NoteField) -> SegmentField {
        let mode = if field == NoteField::Position {
            SegmentMode::Position
        } else {
            SegmentMode::Length { anchor: 0 }
        };
        SegmentField::new(&self.bars, mode)
    }

    /// The value one step of `part` moves a field.
    fn note_unit(&self, field: NoteField, part: usize, value: i64, fine: bool) -> i64 {
        match field {
            NoteField::Pitch => {
                if fine {
                    12
                } else {
                    1
                }
            }
            NoteField::Velocity => {
                if fine {
                    10
                } else {
                    1
                }
            }
            _ => self.note_widget(field).unit(value, part, fine),
        }
    }

    /// The selected notes' indices and the first one's value of `field`.
    fn note_base(&self, field: NoteField) -> Option<i64> {
        let notes = self.notes();
        let first = *self.selection.items().iter().next()?;
        notes.get(first).map(|n| field.value(n))
    }

    /// The note list when the first selected note's `field` becomes `new` and every selected
    /// note moves by the same change, with the selection's new positions.
    fn note_plan(
        field: NoteField,
        notes: &[Note],
        sel: &BTreeSet<usize>,
        base: i64,
        new: i64,
    ) -> (Vec<Note>, BTreeSet<usize>) {
        let delta = new.saturating_sub(base);
        let list: Vec<Note> = notes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                if sel.contains(&i) {
                    field.set(n, field.value(n).saturating_add(delta))
                } else {
                    *n
                }
            })
            .collect();
        normalize_notes(list, sel)
    }

    /// Sends the plan as one `SetClipContent` and follows the selection; nothing when the
    /// notes do not change.
    fn note_send(
        &mut self,
        plan: (Vec<Note>, BTreeSet<usize>),
        merge: Option<u64>,
        out: &mut Vec<UiAction>,
    ) -> bool {
        let (list, sel) = plan;
        if list == self.notes() {
            return false;
        }
        self.selection.set(sel);
        out.extend(self.content(list, merge));
        true
    }

    /// Sets `field` of the first selected note to `new` as one undo step.
    fn note_set(&mut self, field: NoteField, new: i64, out: &mut Vec<UiAction>) {
        let Some(base) = self.note_base(field) else {
            return;
        };
        let plan = Self::note_plan(field, self.notes(), self.selection.items(), base, new);
        self.note_send(plan, None, out);
    }

    /// The text a pitch or velocity starts typing from, and a part of a segmented field.
    fn note_edit_text(&self, field: NoteField, part: usize, base: i64) -> String {
        match field {
            NoteField::Pitch => note_name(base.clamp(0, 127) as u8),
            NoteField::Velocity => base.to_string(),
            _ => self.note_widget(field).split(base)[part].to_string(),
        }
    }

    /// What typed `text` means for `field`, in the field's value.
    fn note_parse(&self, field: NoteField, part: usize, base: i64, text: &str) -> Option<i64> {
        let t = text.trim();
        match field {
            NoteField::Pitch | NoteField::Velocity => {
                if let Some(n) = t.strip_prefix('+') {
                    return Some(base.saturating_add(n.parse::<i64>().ok()?.clamp(0, 1000)));
                }
                if let Some(n) = t.strip_prefix('-')
                    && n.chars().all(|c| c.is_ascii_digit())
                    && !n.is_empty()
                {
                    return Some(base.saturating_sub(n.parse::<i64>().ok()?.clamp(0, 1000)));
                }
                if let Ok(n) = t.parse::<i64>() {
                    return Some(n);
                }
                if field == NoteField::Pitch {
                    return parse_note_name(t).map(i64::from);
                }
                None
            }
            _ => self.note_widget(field).parse(base, part, t),
        }
    }

    // ------------------------------------------------------------------ events

    /// Whether `pos` is over the strip.
    pub(super) fn insp_contains(&self, pos: Point) -> bool {
        self.insp.visible && self.inspector_rect().contains(pos)
    }

    fn insp_field_at(&self, pos: Point) -> Option<(NoteField, usize)> {
        FIELDS.into_iter().find_map(|f| {
            let r = self.inspector_field_rect(f)?;
            if !r.contains(pos) {
                return None;
            }
            Some((f, if f.segmented() { seg::hit(r, pos)? } else { 0 }))
        })
    }

    fn insp_enabled(&self) -> bool {
        !self.selected_notes().is_empty()
    }

    /// Takes the events the strip owns. True when the event is consumed.
    pub(super) fn insp_event(&mut self, event: &UiEvent, out: &mut Vec<UiAction>) -> bool {
        match event {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                clicks,
                ..
            } if self.insp_contains(*pos) => {
                self.insp_press(*pos, *clicks, out);
                true
            }
            UiEvent::PointerDown { pos, .. } if self.insp.editing() => {
                self.insp_commit(out);
                self.insp_contains(*pos)
            }
            UiEvent::PointerMove { pos, modifiers } if self.insp.drag.is_some() => {
                self.insp_drag_to(pos.y, *modifiers, out);
                true
            }
            UiEvent::PointerUp { .. } if self.insp.drag.is_some() => {
                if let Some(d) = self.insp.drag.take()
                    && d.changed
                {
                    out.push(UiAction::EndMerge);
                }
                self.repaint = true;
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

    fn insp_press(&mut self, pos: Point, clicks: u8, out: &mut Vec<UiAction>) {
        self.insp_commit(out);
        self.repaint = true;
        let Some((field, part)) = self.insp_field_at(pos) else {
            return;
        };
        let Some(base) = self.note_base(field).filter(|_| self.insp_enabled()) else {
            return;
        };
        if clicks >= 2 {
            let text = self.note_edit_text(field, part, base);
            self.insp.edit = Some((field, SegmentEdit::new(part, text)));
            return;
        }
        let merge = self.merge_id();
        self.insp.drag = Some(NoteDrag {
            field,
            part,
            steps: StepDrag::new(pos.y),
            value: base,
            sent: i64::MIN,
            notes: self.notes().to_vec(),
            sel: self.selection.items().clone(),
            base,
            merge,
            changed: false,
        });
    }

    fn insp_drag_to(&mut self, y: f64, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        let Some(mut d) = self.insp.drag.take() else {
            return;
        };
        let n = d.steps.update(y, PX_PER_STEP);
        if n != 0 {
            let unit = self.note_unit(d.field, d.part, d.value, modifiers.shift);
            d.value = d.value.saturating_add(i64::from(n) * unit);
            if d.value != d.sent {
                d.sent = d.value;
                let plan = Self::note_plan(d.field, &d.notes, &d.sel, d.base, d.value);
                d.changed |= self.note_send(plan, Some(d.merge), out);
            }
            self.repaint = true;
        }
        self.insp.drag = Some(d);
    }

    fn insp_wheel(
        &mut self,
        delta_y: f64,
        pos: Point,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        let Some((field, part)) = self.insp_field_at(pos) else {
            return;
        };
        let n = wheel_steps(delta_y);
        let Some(base) = self.note_base(field) else {
            return;
        };
        if n == 0 {
            return;
        }
        let unit = self.note_unit(field, part, base, modifiers.shift);
        self.note_set(field, base.saturating_add(i64::from(n) * unit), out);
    }

    fn insp_commit(&mut self, out: &mut Vec<UiAction>) {
        let Some((field, edit)) = self.insp.edit.take() else {
            return;
        };
        self.repaint = true;
        let Some(base) = self.note_base(field) else {
            return;
        };
        if let Some(new) = self.note_parse(field, edit.part, base, &edit.text) {
            self.note_set(field, new, out);
        }
    }

    fn insp_key(&mut self, key: &Key, out: &mut Vec<UiAction>) {
        let Some((field, mut edit)) = self.insp.edit.take() else {
            return;
        };
        self.repaint = true;
        let letters = field == NoteField::Pitch;
        match edit.key_with(key, |c| letters && (c.is_ascii_alphabetic() || c == '#')) {
            EditKey::Commit => {
                self.insp.edit = Some((field, edit));
                self.insp_commit(out);
                return;
            }
            EditKey::Cancel => return,
            EditKey::Step(n) => {
                if let Some(base) = self.note_base(field) {
                    let unit = self.note_unit(field, edit.part, base, false);
                    let new = base.saturating_add(i64::from(n) * unit);
                    self.note_set(field, new, out);
                    let shown = field.value(&field.set(&Note::new(0, 1, 60, 0.5), new));
                    edit =
                        SegmentEdit::new(edit.part, self.note_edit_text(field, edit.part, shown));
                }
            }
            EditKey::Move(d) if field.segmented() => {
                let part = (edit.part as i32 + d).clamp(0, SEGMENTS as i32 - 1) as usize;
                let base = self.note_base(field).unwrap_or(0);
                edit = SegmentEdit::new(part, self.note_edit_text(field, part, base));
            }
            _ => {}
        }
        self.insp.edit = Some((field, edit));
    }

    // ------------------------------------------------------------------ paint

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
        let count = self.selection.items().len();
        let name = match count {
            0 => "no note selected".to_owned(),
            1 => "1 note".to_owned(),
            n => format!("{n} notes"),
        };
        sp::label(
            c,
            "NOTES",
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
        let enabled = self.insp_enabled();
        for field in FIELDS {
            let Some(r) = self.inspector_field_rect(field) else {
                continue;
            };
            let value = self.inspector_value(field);
            let editing = self.insp.edit.as_ref().filter(|(f, _)| *f == field);
            if field.segmented() {
                let part = editing.map(|(_, e)| e.part).or_else(|| {
                    self.insp
                        .drag
                        .as_ref()
                        .filter(|d| d.field == field)
                        .map(|d| d.part)
                });
                seg::paint(
                    c,
                    &self.st,
                    &self.note_widget(field),
                    r,
                    value,
                    FieldLook {
                        label: field.label(),
                        part,
                        typing: editing.map(|(_, e)| e.text.as_str()),
                        enabled,
                    },
                );
            } else {
                let text = match value {
                    FieldValue::Value(v) if field == NoteField::Pitch => {
                        note_name(v.clamp(0, 127) as u8)
                    }
                    FieldValue::Value(v) => v.to_string(),
                    FieldValue::Mixed => "\u{2014}".to_owned(),
                    FieldValue::Empty => String::new(),
                };
                seg::paint_scalar(
                    c,
                    &self.st,
                    r,
                    field.label(),
                    &text,
                    editing.map(|(_, e)| e.text.as_str()),
                    enabled,
                );
            }
        }
    }

    /// The tooltip of the strip under `pos`.
    pub(super) fn insp_help(&self, pos: Point) -> Option<crate::tooltip::Help> {
        let (field, _) = self.insp_field_at(pos)?;
        let r = self.inspector_field_rect(field)?;
        let text = match field {
            NoteField::Pitch => {
                "Pitch of the selected notes: drag or scroll (Shift: an octave), double-click to type a name like C#3, a number, or +n and -n semitones"
            }
            NoteField::Position => {
                "Note start in the clip as bar.beat.sixteenth.tick: drag or scroll a part (Shift: finer), double-click to type; +n and -n add or subtract ticks"
            }
            NoteField::Length => {
                "Note length in bars, beats, sixteenths and ticks: drag or scroll a part (Shift: finer), double-click to type"
            }
            NoteField::Velocity => {
                "Velocity 1 to 127: drag or scroll (Shift: ten), double-click to type; with several notes the change applies to each"
            }
        };
        Some(crate::tooltip::Help::new(r, text))
    }
}
