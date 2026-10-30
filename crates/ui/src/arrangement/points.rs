//! The point inspector: while automation points are selected the arrangement's inspector strip
//! (see [`super::inspector`]) shows their **Position** and **Value** instead of the clip fields.
//!
//! * Position is a segmented `bar.beat.16th.tick` field in the clip's own ticks; Value is the
//!   parameter's plain value shown in its unit (a percentage reads 0 to 100, typed the same
//!   way).
//! * With several points selected a field shows a dash where they differ and an edit is
//!   relative: the first selected point's change is added to every selected point, each
//!   clamped on its own (the value to the lane's range, the position to 0..MAX_TICK). A
//!   move that would put two points on one tick sends nothing.
//! * A typed value, wheel notch or arrow key is one `SetClipContent`; a drag is one merge
//!   group closed by `EndMerge`.

use super::{ArrangementView, content_cmd};
use crate::action::UiAction;
use crate::canvas::{Align, Canvas, TextRun, pt};
use crate::event::{Key, Modifiers, PointerButton, UiEvent};
use crate::segment_field::{
    self as seg, EditKey, FieldLook, FieldValue, SEGMENTS, SegmentEdit, SegmentField, SegmentMode,
};
use crate::seq_paint as sp;
use crate::step_drag::{PX_PER_STEP, StepDrag, wheel_steps};
use crate::widget::format_value;
use atomik_core::{ParamDescriptor, Unit};
use atomik_model::song::MAX_TICK;
use atomik_model::{AutomationPoint, ClipContent, ClipId};
use std::collections::BTreeSet;
use vello::kurbo::{Point, Rect};

/// Width of the Value field.
const VALUE_W: f64 = 110.0;

/// A field of the point strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointField {
    /// Where the point sits in the clip.
    Position,
    /// The parameter value.
    Value,
}

/// A drag on a point field.
#[derive(Debug, Clone)]
struct PointDrag {
    field: PointField,
    part: usize,
    steps: StepDrag,
    value: f64,
    sent: f64,
    clip: ClipId,
    points: Vec<AutomationPoint>,
    sel: BTreeSet<usize>,
    base: f64,
    merge: u64,
    changed: bool,
}

/// The point strip's view state.
#[derive(Debug, Default)]
pub(super) struct PointInspectorUi {
    drag: Option<PointDrag>,
    edit: Option<(PointField, SegmentEdit)>,
}

impl PointInspectorUi {
    pub(super) fn editing(&self) -> bool {
        self.edit.is_some()
    }

    pub(super) fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

/// The selected points of the clip and what they need to be edited.
struct Ctx {
    clip: ClipId,
    points: Vec<AutomationPoint>,
    sel: BTreeSet<usize>,
    param: ParamDescriptor,
}

/// The factor between a plain value and what the field shows.
fn scale(p: &ParamDescriptor) -> f64 {
    if p.unit == Unit::Percent { 100.0 } else { 1.0 }
}

impl ArrangementView {
    /// The selected points, when there are any.
    fn pt_ctx(&self) -> Option<Ctx> {
        let (id, idx) = self.point_sel.as_ref()?;
        let (track, lane, clip) = self.find_clip(*id)?;
        let ClipContent::Automation(points) = &clip.content else {
            return None;
        };
        let sel: BTreeSet<usize> = idx.iter().copied().filter(|i| *i < points.len()).collect();
        if sel.is_empty() {
            return None;
        }
        Some(Ctx {
            clip: *id,
            points: points.clone(),
            sel,
            param: self.param_of(track, lane),
        })
    }

    /// Whether the strip shows points now: some are selected.
    pub fn inspector_shows_points(&self) -> bool {
        self.insp.visible && self.pt_ctx().is_some()
    }

    /// The rectangle of a point field.
    pub fn inspector_point_rect(&self, field: PointField) -> Option<Rect> {
        if !self.inspector_shows_points() {
            return None;
        }
        let strip = self.inspector_rect();
        let x0 = self.axis.x0 + super::inspector::GAP;
        let y0 = strip.y0 + (super::inspector::INSPECTOR_HEIGHT - 30.0) / 2.0;
        let (x0, w) = match field {
            PointField::Position => (x0, seg::FIELD_WIDTH),
            PointField::Value => (x0 + seg::FIELD_WIDTH + super::inspector::GAP, VALUE_W),
        };
        let r = Rect::new(x0, y0, x0 + w, y0 + 30.0);
        (r.x1 <= strip.x1).then_some(r)
    }

    /// The centre of a part of the Position field (0 bar .. 3 tick), or of the Value field.
    pub fn inspector_point_part(&self, field: PointField, part: usize) -> Option<Point> {
        let r = self.inspector_point_rect(field)?;
        Some(match field {
            PointField::Position => seg::segment_rects(r)[part.min(SEGMENTS - 1)].center(),
            PointField::Value => r.center(),
        })
    }

    /// What a point field shows over the selected points; the value is in the parameter's
    /// plain units.
    pub fn inspector_point_value(&self, field: PointField) -> Option<PointShown> {
        let ctx = self.pt_ctx()?;
        let mut vals = ctx.sel.iter().map(|&i| match field {
            PointField::Position => ctx.points[i].tick as f64,
            PointField::Value => f64::from(ctx.points[i].value),
        });
        let first = vals.next()?;
        Some(if vals.all(|v| (v - first).abs() < 1e-9) {
            PointShown::Value(first)
        } else {
            PointShown::Mixed
        })
    }

    /// The text being typed into a point field.
    pub fn inspector_point_text(&self) -> Option<&str> {
        self.pts.edit.as_ref().map(|(_, e)| e.text.as_str())
    }

    fn pt_widget(&self) -> SegmentField {
        SegmentField::new(&self.bars, SegmentMode::Position)
    }

    /// Value of the first selected point.
    fn pt_base(ctx: &Ctx, field: PointField) -> f64 {
        let p = &ctx.points[*ctx.sel.iter().next().unwrap_or(&0)];
        match field {
            PointField::Position => p.tick as f64,
            PointField::Value => f64::from(p.value),
        }
    }

    /// One step of the field in its own units.
    fn pt_unit(&self, ctx: &Ctx, field: PointField, part: usize, value: f64, fine: bool) -> f64 {
        match field {
            PointField::Position => self.pt_widget().unit(value as i64, part, fine) as f64,
            PointField::Value => {
                if ctx.param.discrete {
                    1.0
                } else {
                    let step = f64::from(ctx.param.max - ctx.param.min) / 100.0;
                    if fine { step / 10.0 } else { step }
                }
            }
        }
    }

    /// The point list when the first selected point's `field` becomes `new`, every selected
    /// point moving by the same change; `None` when nothing changes or two points would share
    /// a tick.
    fn pt_plan(
        field: PointField,
        points: &[AutomationPoint],
        sel: &BTreeSet<usize>,
        param: &ParamDescriptor,
        base: f64,
        new: f64,
    ) -> Option<(Vec<AutomationPoint>, BTreeSet<usize>)> {
        let delta = new - base;
        let mut tagged: Vec<(AutomationPoint, bool)> = points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut p = *p;
                if sel.contains(&i) {
                    match field {
                        PointField::Position => {
                            p.tick = ((p.tick as f64 + delta).round() as i64).clamp(0, MAX_TICK);
                        }
                        PointField::Value => {
                            p.value = param.clamp((f64::from(p.value) + delta) as f32);
                        }
                    }
                }
                (p, sel.contains(&i))
            })
            .collect();
        tagged.sort_by_key(|(p, _)| p.tick);
        if tagged.windows(2).any(|w| w[0].0.tick == w[1].0.tick) {
            return None;
        }
        let new_sel: BTreeSet<usize> = tagged
            .iter()
            .enumerate()
            .filter(|(_, (_, t))| *t)
            .map(|(i, _)| i)
            .collect();
        let list: Vec<AutomationPoint> = tagged.into_iter().map(|(p, _)| p).collect();
        (list != points).then_some((list, new_sel))
    }

    fn pt_send(
        &mut self,
        clip: ClipId,
        plan: Option<(Vec<AutomationPoint>, BTreeSet<usize>)>,
        merge: Option<u64>,
        out: &mut Vec<UiAction>,
    ) -> bool {
        let Some((list, sel)) = plan else {
            return false;
        };
        self.point_sel = Some((clip, sel));
        out.push(content_cmd(clip, list, merge));
        self.repaint = true;
        true
    }

    fn pt_set(&mut self, field: PointField, new: f64, out: &mut Vec<UiAction>) {
        let Some(ctx) = self.pt_ctx() else { return };
        let base = Self::pt_base(&ctx, field);
        let plan = Self::pt_plan(field, &ctx.points, &ctx.sel, &ctx.param, base, new);
        self.pt_send(ctx.clip, plan, None, out);
    }

    fn pt_field_at(&self, pos: Point) -> Option<(PointField, usize)> {
        let r = self.inspector_point_rect(PointField::Position)?;
        if let Some(part) = seg::hit(r, pos) {
            return Some((PointField::Position, part));
        }
        let r = self.inspector_point_rect(PointField::Value)?;
        r.contains(pos).then_some((PointField::Value, 0))
    }

    fn pt_edit_text(&self, ctx: &Ctx, field: PointField, part: usize, base: f64) -> String {
        match field {
            PointField::Position => self.pt_widget().split(base as i64)[part].to_string(),
            PointField::Value => {
                let v = base * scale(&ctx.param);
                if ctx.param.discrete {
                    format!("{v:.0}")
                } else {
                    format!("{v:.3}")
                }
            }
        }
    }

    /// What typed text means for a field, in plain units.
    fn pt_parse(
        &self,
        ctx: &Ctx,
        field: PointField,
        part: usize,
        base: f64,
        text: &str,
    ) -> Option<f64> {
        let t = text.trim();
        match field {
            PointField::Position => self
                .pt_widget()
                .parse(base as i64, part, t)
                .map(|v| v as f64),
            PointField::Value => {
                let k = scale(&ctx.param);
                if let Some(n) = t.strip_prefix('+') {
                    return Some(base + n.parse::<f64>().ok()? / k);
                }
                if let Some(n) = t.strip_prefix('-') {
                    return Some(base - n.parse::<f64>().ok()? / k);
                }
                Some(t.parse::<f64>().ok()? / k).filter(|v| v.is_finite())
            }
        }
    }

    // ------------------------------------------------------------------ events

    /// Takes the events of the point strip. True when consumed.
    pub(super) fn pt_event(&mut self, event: &UiEvent, out: &mut Vec<UiAction>) -> bool {
        match event {
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                clicks,
                ..
            } if self.insp_contains(*pos) => {
                self.pt_press(*pos, *clicks, out);
                true
            }
            UiEvent::PointerDown { pos, .. } if self.pts.editing() => {
                self.pt_commit(out);
                self.insp_contains(*pos)
            }
            UiEvent::PointerMove { pos, modifiers } if self.pts.dragging() => {
                self.pt_drag_to(pos.y, *modifiers, out);
                true
            }
            UiEvent::PointerUp { .. } if self.pts.dragging() => {
                if let Some(d) = self.pts.drag.take()
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
                self.pt_wheel(delta.y, *pos, *modifiers, out);
                true
            }
            UiEvent::Key {
                key, pressed: true, ..
            } if self.pts.editing() => {
                self.pt_key(key, out);
                true
            }
            _ => false,
        }
    }

    fn pt_press(&mut self, pos: Point, clicks: u8, out: &mut Vec<UiAction>) {
        self.pt_commit(out);
        self.repaint = true;
        let (Some(ctx), Some((field, part))) = (self.pt_ctx(), self.pt_field_at(pos)) else {
            return;
        };
        let base = Self::pt_base(&ctx, field);
        if clicks >= 2 {
            let text = self.pt_edit_text(&ctx, field, part, base);
            self.pts.edit = Some((field, SegmentEdit::new(part, text)));
            return;
        }
        let merge = self.merge_id();
        self.pts.drag = Some(PointDrag {
            field,
            part,
            steps: StepDrag::new(pos.y),
            value: base,
            sent: f64::NAN,
            clip: ctx.clip,
            points: ctx.points,
            sel: ctx.sel,
            base,
            merge,
            changed: false,
        });
    }

    fn pt_drag_to(&mut self, y: f64, modifiers: Modifiers, out: &mut Vec<UiAction>) {
        let Some(mut d) = self.pts.drag.take() else {
            return;
        };
        let n = d.steps.update(y, PX_PER_STEP);
        if n != 0
            && let Some(ctx) = self.pt_ctx()
        {
            let unit = self.pt_unit(&ctx, d.field, d.part, d.value, modifiers.shift);
            d.value += f64::from(n) * unit;
            if d.sel.len() == 1 {
                // One point stops at its own limit; with several each is clamped in the plan.
                d.value = match d.field {
                    PointField::Position => d.value.clamp(0.0, MAX_TICK as f64),
                    PointField::Value => d
                        .value
                        .clamp(f64::from(ctx.param.min), f64::from(ctx.param.max)),
                };
            }
            if d.sent.is_nan() || (d.value - d.sent).abs() > 1e-12 {
                d.sent = d.value;
                let plan = Self::pt_plan(d.field, &d.points, &d.sel, &ctx.param, d.base, d.value);
                d.changed |= self.pt_send(d.clip, plan, Some(d.merge), out);
            }
            self.repaint = true;
        }
        self.pts.drag = Some(d);
    }

    fn pt_wheel(
        &mut self,
        delta_y: f64,
        pos: Point,
        modifiers: Modifiers,
        out: &mut Vec<UiAction>,
    ) {
        let (Some(ctx), Some((field, part))) = (self.pt_ctx(), self.pt_field_at(pos)) else {
            return;
        };
        let n = wheel_steps(delta_y);
        if n == 0 {
            return;
        }
        let base = Self::pt_base(&ctx, field);
        let unit = self.pt_unit(&ctx, field, part, base, modifiers.shift);
        self.pt_set(field, base + f64::from(n) * unit, out);
    }

    fn pt_commit(&mut self, out: &mut Vec<UiAction>) {
        let Some((field, edit)) = self.pts.edit.take() else {
            return;
        };
        self.repaint = true;
        let Some(ctx) = self.pt_ctx() else { return };
        let base = Self::pt_base(&ctx, field);
        if let Some(new) = self.pt_parse(&ctx, field, edit.part, base, &edit.text) {
            self.pt_set(field, new, out);
        }
    }

    fn pt_key(&mut self, key: &Key, out: &mut Vec<UiAction>) {
        let Some((field, mut edit)) = self.pts.edit.take() else {
            return;
        };
        self.repaint = true;
        match edit.key(key) {
            EditKey::Commit => {
                self.pts.edit = Some((field, edit));
                self.pt_commit(out);
                return;
            }
            EditKey::Cancel => return,
            EditKey::Step(n) => {
                if let Some(ctx) = self.pt_ctx() {
                    let base = Self::pt_base(&ctx, field);
                    let unit = self.pt_unit(&ctx, field, edit.part, base, false);
                    let new = base + f64::from(n) * unit;
                    self.pt_set(field, new, out);
                    edit =
                        SegmentEdit::new(edit.part, self.pt_edit_text(&ctx, field, edit.part, new));
                }
            }
            EditKey::Move(d) if field == PointField::Position => {
                if let Some(ctx) = self.pt_ctx() {
                    let part = (edit.part as i32 + d).clamp(0, SEGMENTS as i32 - 1) as usize;
                    let base = Self::pt_base(&ctx, field);
                    edit = SegmentEdit::new(part, self.pt_edit_text(&ctx, field, part, base));
                }
            }
            _ => {}
        }
        self.pts.edit = Some((field, edit));
    }

    // ------------------------------------------------------------------ paint

    pub(super) fn paint_point_fields(&self, c: &mut dyn Canvas) {
        let Some(ctx) = self.pt_ctx() else { return };
        let strip = self.inspector_rect();
        let t = &self.st.base;
        let n = ctx.sel.len();
        sp::label(
            c,
            "AUTOMATION",
            strip.x0 + 10.0,
            strip.y0 + 14.0,
            6.5,
            t.text_muted,
            Align::Start,
        );
        let name = if n == 1 {
            "1 point".to_owned()
        } else {
            format!("{n} points")
        };
        c.text(
            &TextRun::new(
                name,
                pt(strip.x0 + 10.0, strip.y0 + 29.0),
                11.0,
                t.highlight,
            )
            .weight(600),
        );
        if let Some(r) = self.inspector_point_rect(PointField::Position) {
            let value = match self.inspector_point_value(PointField::Position) {
                Some(PointShown::Value(v)) => FieldValue::Value(v as i64),
                Some(PointShown::Mixed) => FieldValue::Mixed,
                None => FieldValue::Empty,
            };
            let editing = self
                .pts
                .edit
                .as_ref()
                .filter(|(f, _)| *f == PointField::Position);
            let part = editing.map(|(_, e)| e.part).or_else(|| {
                self.pts
                    .drag
                    .as_ref()
                    .filter(|d| d.field == PointField::Position)
                    .map(|d| d.part)
            });
            seg::paint(
                c,
                &self.st,
                &self.pt_widget(),
                r,
                value,
                FieldLook {
                    label: "POSITION",
                    part,
                    typing: editing.map(|(_, e)| e.text.as_str()),
                    enabled: true,
                },
            );
        }
        if let Some(r) = self.inspector_point_rect(PointField::Value) {
            let text = match self.inspector_point_value(PointField::Value) {
                Some(PointShown::Value(v)) => format_value(&ctx.param, v as f32),
                Some(PointShown::Mixed) => "\u{2014}".to_owned(),
                None => String::new(),
            };
            let editing = self
                .pts
                .edit
                .as_ref()
                .filter(|(f, _)| *f == PointField::Value);
            seg::paint_scalar(
                c,
                &self.st,
                r,
                "VALUE",
                &text,
                editing.map(|(_, e)| e.text.as_str()),
                true,
            );
        }
    }

    pub(super) fn pt_help(&self, pos: Point) -> Option<crate::tooltip::Help> {
        let (field, _) = self.pt_field_at(pos)?;
        let r = self.inspector_point_rect(field)?;
        let text = match field {
            PointField::Position => {
                "Point position in the clip as bar.beat.sixteenth.tick: drag or scroll a part (Shift: finer), double-click to type; +n and -n add or subtract ticks"
            }
            PointField::Value => {
                "Parameter value in its unit: drag or scroll (Shift: finer), double-click to type; +n and -n add or subtract; with several points the change applies to each"
            }
        };
        Some(crate::tooltip::Help::new(r, text))
    }
}

/// What a point field shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointShown {
    /// The same value on every selected point (plain units; ticks for the position).
    Value(f64),
    /// The points differ.
    Mixed,
}
