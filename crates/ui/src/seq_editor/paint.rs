//! Painting of the sequencer grid editor (see the parent module for the layout).

use super::{
    Drag, HEADER_WIDTH, Hit, KEY_ROW_HEIGHT, Lane, MAX_KEY, PatternButton, SWING_MAX,
    SeqEditorView, beat_steps,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::fit_label;
use crate::seq_paint as sp;
use crate::seq_panel::resolution_label;
use crate::theme::mix;
use crate::zone_edit::note_name;
use atomik_model::{PatternDoc, SeqResolution};
use vello::kurbo::{Affine, Line, Rect};
use vello::peniko::Color;

pub(super) fn paint(v: &SeqEditorView, c: &mut dyn Canvas) {
    let b = v.bounds;
    c.push_transform(Affine::scale(v.scale));
    c.push_clip(&b.into());
    sp::rect(c, b, v.st.background);
    match (v.device.is_some() && v.present, v.pattern()) {
        (true, Some(p)) => {
            toolbars(v, c, p);
            ruler(v, c, p);
            note_lane(v, c, p);
            tie_row(v, c, p);
            for lane in [Lane::Velocity, Lane::Gate, Lane::Probability] {
                bar_lane(v, c, p, lane);
            }
            if v.is_cv {
                let curves = v.curve_pattern();
                for row in 0..2 {
                    curve_lane(v, c, p, &curves, row);
                }
            }
            cursor(v, c, p);
        }
        _ => sp::label(
            c,
            "No sequencer open",
            b.center().x,
            b.center().y,
            11.0,
            v.st.base.text_muted,
            Align::Center,
        ),
    }
    top_bar(v, c);
    if v.focused {
        sp::line(c, b.x0, b.y0 + 0.5, b.x1, b.y0 + 0.5, v.st.base.focus);
    }
    c.pop_clip();
    c.pop_transform();
}

fn toolbars(v: &SeqEditorView, c: &mut dyn Canvas, p: &PatternDoc) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout();
    for r in [l.patterns, l.settings] {
        sp::rect(c, r, st.header);
        sp::line(c, r.x0, r.y1 - 0.5, r.x1, r.y1 - 0.5, st.separator);
    }
    let label = |c: &mut dyn Canvas, text: &str, row: Rect, x: f64| {
        sp::label(c, text, x, row.y0 + 18.0, 8.0, t.text_muted, Align::Start);
    };
    let x0 = v.bounds.x0 + super::PAD;
    label(c, "PATTERN", l.patterns, x0);
    for i in 0..super::MAX_PATTERNS {
        let r = v.slot_rect(i);
        let exists = i < v.patterns.len();
        let edited = i == v.index;
        let lit = (exists && edited).then_some(v.accent);
        if exists {
            sp::button(
                c,
                st,
                r,
                &(i + 1).to_string(),
                lit,
                v.hover == Hit::Slot(i as u8),
            );
        } else {
            sp::lcd(c, st, r, None);
            sp::label(
                c,
                &(i + 1).to_string(),
                r.center().x,
                r.center().y + 3.5,
                9.5,
                t.text_muted.with_alpha(0.45),
                Align::Center,
            );
        }
        if exists && i == v.play_state().playing {
            c.fill_with(
                vello::kurbo::Circle::new(pt(r.x1 - 5.0, r.y0 + 5.0), 2.4),
                v.accent,
            );
        }
    }
    let count = v.patterns.len();
    for b in PatternButton::ALL {
        let enabled = match b {
            PatternButton::Add | PatternButton::Duplicate => count < super::MAX_PATTERNS,
            PatternButton::Remove => count > 1,
            _ => true,
        };
        let r = v.pattern_button_rect(b);
        sp::button(
            c,
            st,
            r,
            b.label(),
            None,
            enabled && v.hover == Hit::Button(b),
        );
        if !enabled {
            sp::rect(c, r, st.header.with_alpha(0.55));
        }
    }
    if let Some(text) = v.renaming() {
        let r = Rect::new(
            l.patterns.x1 - 250.0,
            l.patterns.y0 + 4.0,
            l.patterns.x1 - 8.0,
            l.patterns.y0 + 24.0,
        );
        sp::lcd(c, st, r, Some(v.accent));
        sp::label(
            c,
            &format!("{}|", fit_label(text, r.width() - 12.0, 9.5)),
            r.x0 + 6.0,
            r.y0 + 14.0,
            9.5,
            v.accent,
            Align::Start,
        );
    }
    // Settings row.
    label(c, "LENGTH", l.settings, x0);
    sp::button(
        c,
        st,
        v.length_rect(0),
        "-",
        None,
        v.hover == Hit::LengthDec,
    );
    sp::button(
        c,
        st,
        v.length_rect(2),
        "+",
        None,
        v.hover == Hit::LengthInc,
    );
    let lf = v.length_rect(1);
    let active = matches!(&v.drag, Drag::Field(d) if d.field == super::Field::Length)
        || v.hover == Hit::LengthField;
    sp::lcd(c, st, lf, active.then_some(v.accent));
    sp::label(
        c,
        &p.length.to_string(),
        lf.center().x,
        lf.center().y + 3.5,
        10.0,
        t.text,
        Align::Center,
    );
    label(
        c,
        "RES",
        l.settings,
        v.res_rect(SeqResolution::Quarter).x0 - 28.0,
    );
    for r in SeqResolution::ALL {
        let lit = (p.resolution == r).then_some(v.accent);
        sp::button(
            c,
            st,
            v.res_rect(r),
            resolution_label(r),
            lit,
            v.hover == Hit::Resolution(r),
        );
    }
    let sw = v.swing_rect();
    label(c, "SWING", l.settings, sw.x0 - 42.0);
    let active = matches!(&v.drag, Drag::Field(d) if d.field == super::Field::Swing)
        || v.hover == Hit::Swing;
    sp::lcd(c, st, sw, active.then_some(v.accent));
    let frac = f64::from(p.swing) / f64::from(SWING_MAX);
    sp::rect(
        c,
        Rect::new(
            sw.x0 + 2.0,
            sw.y1 - 4.0,
            sw.x0 + 2.0 + (sw.width() - 4.0) * frac,
            sw.y1 - 2.0,
        ),
        v.accent,
    );
    sp::label(
        c,
        &format!("{} %", p.swing),
        sw.center().x,
        sw.y0 + 13.0,
        10.0,
        t.text,
        Align::Center,
    );
}

fn grid_lines(v: &SeqEditorView, c: &mut dyn Canvas, p: &PatternDoc, y0: f64, y1: f64) {
    let st = &v.st;
    let l = v.layout();
    let gx0 = v.bounds.x0 + HEADER_WIDTH;
    let per_beat = beat_steps(p.resolution);
    for k in 0..usize::from(p.length) {
        let x = v.cell_x(k);
        if x + l.cell_w < gx0 || x > gx0 + l.avail_w {
            continue;
        }
        let col = if k % (per_beat * 4) == 0 {
            st.grid_bar
        } else if k % per_beat == 0 {
            st.grid_beat
        } else {
            st.grid_sub
        };
        let gx = x.round() + 0.5;
        sp::line(c, gx, y0, gx, y1, col);
    }
    let end = v.cell_x(usize::from(p.length));
    if end < gx0 + l.avail_w {
        sp::rect(c, Rect::new(end, y0, gx0 + l.avail_w, y1), st.outside);
    }
}

fn ruler(v: &SeqEditorView, c: &mut dyn Canvas, p: &PatternDoc) {
    let st = &v.st;
    let l = v.layout();
    let r = l.ruler;
    sp::rect(c, r, st.ruler);
    sp::line(c, r.x0, r.y1 - 0.5, r.x1, r.y1 - 0.5, st.separator);
    let gx0 = r.x0 + HEADER_WIDTH;
    sp::rect(c, Rect::new(r.x0, r.y0, gx0, r.y1), st.header);
    c.push_clip(&Rect::new(gx0, r.y0, gx0 + l.avail_w, r.y1).into());
    let per_beat = beat_steps(p.resolution);
    for k in 0..usize::from(p.length) {
        let x = v.cell_x(k);
        if x + l.cell_w < gx0 || x > gx0 + l.avail_w {
            continue;
        }
        let beat = k % per_beat == 0;
        if beat || l.cell_w >= 22.0 {
            sp::label(
                c,
                &(k + 1).to_string(),
                x + 3.0,
                r.y1 - 4.0,
                if beat { 8.0 } else { 7.0 },
                if beat {
                    st.base.text
                } else {
                    st.base.text_muted.with_alpha(0.55)
                },
                Align::Start,
            );
        }
    }
    c.pop_clip();
}

fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

fn note_lane(v: &SeqEditorView, c: &mut dyn Canvas, p: &PatternDoc) {
    let st = &v.st;
    let l = v.layout();
    let area = l.notes;
    c.push_clip(&area.into());
    let gx0 = area.x0 + HEADER_WIDTH;
    let gx1 = gx0 + l.avail_w;
    let used = crate::seq_panel::key_span(p);
    for key in (0..=MAX_KEY).rev() {
        let y = v.key_y(key);
        if y + KEY_ROW_HEIGHT < area.y0 || y > area.y1 {
            continue;
        }
        let row = Rect::new(gx0, y, gx1, y + KEY_ROW_HEIGHT);
        sp::rect(c, row, if is_black(key) { st.row_black } else { st.lane });
        if key % 12 == 0 {
            sp::line(
                c,
                gx0,
                y + KEY_ROW_HEIGHT - 0.5,
                gx1,
                y + KEY_ROW_HEIGHT - 0.5,
                st.grid_bar,
            );
        }
        // Key header.
        let h = Rect::new(area.x0, y, gx0, y + KEY_ROW_HEIGHT);
        let sounding = matches!(v.drag, Drag::Audition(k) if k == key);
        let face = if sounding {
            v.accent
        } else if is_black(key) {
            st.key_black
        } else {
            st.key_white
        };
        sp::rect(c, h, face);
        sp::line(c, h.x0, h.y1 - 0.5, h.x1, h.y1 - 0.5, st.separator);
        if !is_black(key)
            && (key % 12 == 0 || used.is_some_and(|(lo, hi)| (lo..=hi).contains(&key)))
        {
            sp::label(
                c,
                &note_name(key),
                h.x1 - 4.0,
                h.y1 - 3.0,
                7.5,
                if sounding {
                    st.base.plate
                } else {
                    st.base.plate.with_alpha(0.85)
                },
                Align::End,
            );
        }
    }
    grid_lines(v, c, p, area.y0, area.y1);
    for (k, s) in p.steps.iter().take(usize::from(p.length)).enumerate() {
        if !s.on {
            continue;
        }
        let x = v.cell_x(k);
        if x + l.cell_w < gx0 || x > gx1 {
            continue;
        }
        let y = v.key_y(s.key);
        let level = f64::from(s.velocity) / 127.0;
        let col = mix(
            mix(v.accent, st.background, 0.5),
            mix(v.accent, Color::WHITE, 0.45),
            level as f32,
        )
        .with_alpha((0.45 + 0.55 * f64::from(s.prob) / 100.0) as f32);
        let w = (l.cell_w * f64::from(s.gate) / 100.0).max(3.0);
        let body = Rect::new(x + 1.0, y + 1.0, x + w - 1.0, y + KEY_ROW_HEIGHT - 1.0);
        sp::rect(c, body, col);
        c.stroke_with(body, 1.0, v.accent.with_alpha(0.9));
        if s.tie && k + 1 < usize::from(p.length) {
            let bridge = Rect::new(
                x + w - 1.0,
                y + 4.0,
                x + l.cell_w + 1.0,
                y + KEY_ROW_HEIGHT - 4.0,
            );
            sp::rect(c, bridge, col);
        }
    }
    sp::line(c, gx0 - 0.5, area.y0, gx0 - 0.5, area.y1, st.separator);
    c.pop_clip();
}

fn tie_row(v: &SeqEditorView, c: &mut dyn Canvas, p: &PatternDoc) {
    let st = &v.st;
    let l = v.layout();
    let r = l.tie;
    sp::rect(c, r, st.lane_auto);
    sp::rect(
        c,
        Rect::new(r.x0, r.y0, r.x0 + HEADER_WIDTH, r.y1),
        st.header,
    );
    sp::label(
        c,
        "TIE",
        r.x0 + 6.0,
        r.y1 - 5.0,
        8.0,
        st.base.text_muted,
        Align::Start,
    );
    sp::line(c, r.x0, r.y0 + 0.5, r.x1, r.y0 + 0.5, st.separator);
    c.push_clip(
        &Rect::new(
            r.x0 + HEADER_WIDTH,
            r.y0,
            r.x0 + HEADER_WIDTH + l.avail_w,
            r.y1,
        )
        .into(),
    );
    for k in 0..usize::from(p.length) {
        let x = v.cell_x(k + 1);
        let on = p.steps[k].tie;
        let hover = v.hover == Hit::Tie(k);
        let half = (l.cell_w * 0.3).clamp(4.0, 9.0);
        let box_ = Rect::new(x - half, r.y0 + 3.0, x + half, r.y1 - 3.0);
        let face = if on {
            v.accent
        } else if hover {
            mix(st.lane_auto, st.base.text, 0.2)
        } else {
            mix(st.lane_auto, st.base.text, 0.07)
        };
        sp::rect(c, box_, face);
    }
    c.pop_clip();
}

fn bar_lane(v: &SeqEditorView, c: &mut dyn Canvas, p: &PatternDoc, lane: Lane) {
    let st = &v.st;
    let l = v.layout();
    let r = v.lane_rect(lane);
    sp::rect(c, r, st.lane);
    sp::rect(
        c,
        Rect::new(r.x0, r.y0, r.x0 + HEADER_WIDTH, r.y1),
        st.header,
    );
    sp::line(c, r.x0, r.y0 + 0.5, r.x1, r.y0 + 0.5, st.separator);
    sp::label(
        c,
        lane.label(),
        r.x0 + 6.0,
        r.y0 + 14.0,
        8.0,
        st.base.text_muted,
        Align::Start,
    );
    c.push_clip(
        &Rect::new(
            r.x0 + HEADER_WIDTH,
            r.y0,
            r.x0 + HEADER_WIDTH + l.avail_w,
            r.y1,
        )
        .into(),
    );
    grid_lines(v, c, p, r.y0, r.y1);
    let inner = r.inset(-2.0);
    for (k, s) in p.steps.iter().take(usize::from(p.length)).enumerate() {
        let x = v.cell_x(k);
        let value = f64::from(lane.get(s));
        let frac = (value / lane.max()).clamp(0.0, 1.0);
        let h = (inner.height() * frac).max(if value > 0.0 { 1.5 } else { 0.0 });
        let col = if s.on {
            v.accent
        } else {
            mix(v.accent, st.background, 0.65)
        };
        let bar = Rect::new(x + 2.0, inner.y1 - h, x + l.cell_w - 2.0, inner.y1);
        sp::rect(c, bar, col);
        if l.cell_w >= 24.0 {
            sp::label(
                c,
                &(value as u32).to_string(),
                x + l.cell_w / 2.0,
                r.y0 + 9.0,
                7.0,
                st.base.text_muted,
                Align::Center,
            );
        }
    }
    c.pop_clip();
}

/// A curve lane: the zero line, a bar per step from it to the value, and a line from the bar
/// top to the next bar's top on a step that slides.
fn curve_lane(
    v: &SeqEditorView,
    c: &mut dyn Canvas,
    p: &PatternDoc,
    curves: &atomik_model::CurvePatternDoc,
    row: usize,
) {
    let st = &v.st;
    let l = v.layout();
    let r = l.curves[row];
    sp::rect(c, r, st.lane);
    sp::rect(
        c,
        Rect::new(r.x0, r.y0, r.x0 + HEADER_WIDTH, r.y1),
        st.header,
    );
    sp::line(c, r.x0, r.y0 + 0.5, r.x1, r.y0 + 0.5, st.separator);
    sp::label(
        c,
        if row == 0 { "CURVE A" } else { "CURVE B" },
        r.x0 + 6.0,
        r.y0 + 14.0,
        8.0,
        st.base.text_muted,
        Align::Start,
    );
    c.push_clip(
        &Rect::new(
            r.x0 + HEADER_WIDTH,
            r.y0,
            r.x0 + HEADER_WIDTH + l.avail_w,
            r.y1,
        )
        .into(),
    );
    grid_lines(v, c, p, r.y0, r.y1);
    let mid = r.center().y;
    sp::line(
        c,
        r.x0 + HEADER_WIDTH,
        mid,
        r.x0 + HEADER_WIDTH + l.avail_w,
        mid,
        st.separator,
    );
    let tint = if row == 0 {
        v.accent
    } else {
        mix(v.accent, st.base.highlight, 0.6)
    };
    let steps = &curves.rows[row];
    let n = usize::from(p.length);
    for (k, s) in steps.iter().take(n).enumerate() {
        let x = v.cell_x(k);
        let top = v.curve_point(row, k, f64::from(s.value)).y;
        let (y0, y1) = if top < mid { (top, mid) } else { (mid, top) };
        let bar = Rect::new(x + 2.0, y0, x + l.cell_w - 2.0, y1.max(y0 + 1.5));
        sp::rect(c, bar, tint);
        if s.slide {
            let next = steps[(k + 1) % n];
            let to = v.curve_point(row, k + 1, f64::from(next.value)).y;
            c.stroke_with(
                Line::new(pt(x + l.cell_w / 2.0, top), pt(x + l.cell_w * 1.5, to)),
                1.5,
                st.base.text,
            );
        }
        if l.cell_w >= 24.0 {
            sp::label(
                c,
                &s.value.to_string(),
                x + l.cell_w / 2.0,
                r.y0 + 9.0,
                7.0,
                st.base.text_muted,
                Align::Center,
            );
        }
    }
    c.pop_clip();
}

fn cursor(v: &SeqEditorView, c: &mut dyn Canvas, _p: &PatternDoc) {
    let Some(k) = v.cursor() else { return };
    let l = v.layout();
    let x = v.cell_x(k);
    let gx0 = v.bounds.x0 + HEADER_WIDTH;
    let top = l.ruler.y1;
    let rect = Rect::new(
        x.max(gx0),
        top,
        (x + l.cell_w).min(gx0 + l.avail_w),
        v.bounds.y1,
    );
    if rect.width() > 0.0 {
        sp::rect(c, rect, v.st.playhead.with_alpha(0.16));
        sp::line(
            c,
            x.round() + 0.5,
            top,
            x.round() + 0.5,
            v.bounds.y1,
            v.st.playhead,
        );
    }
}

fn top_bar(v: &SeqEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout();
    let r = l.top;
    let grad = vello::peniko::Gradient::new_linear(pt(0.0, r.y0), pt(0.0, r.y1))
        .with_stops([t.panel_top, t.panel_bottom]);
    c.fill(&r.into(), &grad.into());
    sp::line(c, r.x0, r.y1 - 0.5, r.x1, r.y1 - 0.5, t.bevel_dark);
    sp::rect(c, Rect::new(r.x0, r.y0, r.x0 + 4.0, r.y1), v.accent);
    c.text(
        &TextRun::new(
            "SEQ EDITOR",
            pt(r.x0 + 14.0, r.y0 + 18.0),
            8.0,
            t.text_muted,
        )
        .weight(700)
        .spacing(1.2),
    );
    if v.present {
        let text = match v.pattern() {
            Some(p) => format!(
                "{}  ·  {}  ·  {} steps  ·  {}  ·  swing {} %",
                v.name,
                p.name,
                p.length,
                resolution_label(p.resolution),
                p.swing
            ),
            None => v.name.clone(),
        };
        let text = fit_label(&text, (l.close.x0 - r.x0 - 120.0).max(20.0), 11.0);
        c.text(&TextRun::new(text, pt(r.x0 + 100.0, r.y0 + 18.5), 11.0, t.text).weight(600));
    }
    sp::button(c, st, l.close, "", None, v.hover == Hit::Close);
    let k = l.close.center();
    for (dx, dy) in [(4.0, 4.0), (4.0, -4.0)] {
        c.stroke_with(
            Line::new(pt(k.x - dx, k.y - dy), pt(k.x + dx, k.y + dy)),
            1.6,
            t.text,
        );
    }
}
