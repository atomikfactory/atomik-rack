//! Painting of the step editor (see the parent module for the layout).

use super::{
    Drag, HEADER_WIDTH, Hit, ROW_HEIGHT, STEP_CHOICES, SWING_MAX, StepEditorView, VelocityLevel,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::fit_label;
use crate::seq_paint as sp;
use crate::theme::mix;
use atomik_model::{PAD_COUNT, StepResolution, pad_key};
use vello::kurbo::{Affine, Line, Rect};
use vello::peniko::Color;

fn res_label(res: StepResolution) -> &'static str {
    match res {
        StepResolution::Eighth => "1/8",
        StepResolution::Sixteenth => "1/16",
        StepResolution::ThirtySecond => "1/32",
    }
}

pub(super) fn paint(v: &StepEditorView, c: &mut dyn Canvas) {
    let b = v.bounds;
    c.push_transform(Affine::scale(v.scale));
    c.push_clip(&b.into());
    sp::rect(c, b, v.st.background);
    if v.device.is_some() && v.present {
        if v.clip.is_some() {
            toolbar(v, c);
            ruler(v, c);
            rows(v, c);
        } else {
            empty_state(v, c);
        }
    } else {
        sp::label(
            c,
            "No drum machine open",
            b.center().x,
            b.center().y,
            11.0,
            v.st.base.text_muted,
            Align::Center,
        );
    }
    top_bar(v, c);
    drop_indicator(v, c);
    if v.focused {
        sp::line(c, b.x0, b.y0 + 0.5, b.x1, b.y0 + 0.5, v.st.base.focus);
    }
    c.pop_clip();
    c.pop_transform();
}

fn level_color(v: &StepEditorView, level: VelocityLevel) -> Color {
    match level {
        VelocityLevel::Soft => mix(v.accent, v.st.background, 0.5),
        VelocityLevel::Medium => v.accent,
        VelocityLevel::Hard => mix(v.accent, Color::WHITE, 0.45),
    }
}

fn toolbar(v: &StepEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let r = v.layout().toolbar;
    let g = v.grid();
    sp::rect(c, r, st.header);
    sp::line(c, r.x0, r.y1 - 0.5, r.x1, r.y1 - 0.5, st.separator);
    let ty = r.y0 + 20.0;
    let label = |c: &mut dyn Canvas, text: &str, anchor: Rect| {
        sp::label(c, text, anchor.x0 - 6.0, ty, 8.0, t.text_muted, Align::End);
    };
    let res = [
        StepResolution::Eighth,
        StepResolution::Sixteenth,
        StepResolution::ThirtySecond,
    ];
    label(c, "GRID", v.res_rect(res[0]));
    for rs in res {
        let lit = (g.resolution == rs).then_some(v.accent);
        sp::button(
            c,
            st,
            v.res_rect(rs),
            res_label(rs),
            lit,
            v.hover == Hit::Resolution(rs),
        );
    }
    label(c, "STEPS", v.steps_rect(STEP_CHOICES[0]));
    for n in STEP_CHOICES {
        let lit = (g.steps == n).then_some(v.accent);
        sp::button(
            c,
            st,
            v.steps_rect(n),
            &n.to_string(),
            lit,
            v.hover == Hit::Steps(n),
        );
    }
    label(c, "SWING", v.swing_rect());
    let sw = v.swing_rect();
    let active = matches!(v.drag, Drag::Swing { .. }) || v.hover == Hit::Swing;
    sp::lcd(c, st, sw, active.then_some(v.accent));
    let frac = f64::from(g.swing) / f64::from(SWING_MAX);
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
        &format!("{} %", g.swing),
        sw.center().x,
        sw.y0 + 13.0,
        10.0,
        t.text,
        Align::Center,
    );
    label(c, "VEL", v.level_rect(VelocityLevel::Soft));
    for lv in VelocityLevel::ALL {
        let lit = (v.level == lv).then_some(level_color(v, lv));
        sp::button(
            c,
            st,
            v.level_rect(lv),
            lv.label(),
            lit,
            v.hover == Hit::Level(lv),
        );
    }
    let off = v.off_grid_count();
    if off > 0 {
        sp::label(
            c,
            &format!("{off} off-grid"),
            r.x1 - 10.0,
            ty,
            9.0,
            t.text_muted,
            Align::End,
        );
    }
}

fn ruler(v: &StepEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let l = v.layout();
    let r = l.ruler;
    sp::rect(c, r, st.ruler);
    sp::line(c, r.x0, r.y1 - 0.5, r.x1, r.y1 - 0.5, st.separator);
    let grid = Rect::new(
        r.x0 + HEADER_WIDTH,
        r.y0,
        r.x0 + HEADER_WIDTH + l.avail_w,
        r.y1,
    );
    sp::rect(c, Rect::new(r.x0, r.y0, grid.x0, r.y1), st.header);
    c.push_clip(&grid.into());
    let s = v.step_ticks();
    for k in 0..v.grid().steps {
        let x = v.cell_rect(1, k).x0;
        if x + l.cell_w < grid.x0 || x > grid.x1 {
            continue;
        }
        let t = i64::from(k) * s;
        let bar = t % v.bar_ticks == 0;
        let beat = t % v.beat_ticks == 0;
        if bar || beat {
            let gx = x.round() + 0.5;
            sp::line(
                c,
                gx,
                r.y0 + if bar { 2.0 } else { 9.0 },
                gx,
                r.y1,
                if bar { st.grid_bar } else { st.grid_beat },
            );
            let col = if bar {
                st.base.text
            } else {
                st.base.text_muted
            };
            sp::label(
                c,
                &(k + 1).to_string(),
                x + 3.0,
                r.y1 - 4.0,
                8.0,
                col,
                Align::Start,
            );
        } else if l.cell_w >= 22.0 {
            sp::label(
                c,
                &(k + 1).to_string(),
                x + 3.0,
                r.y1 - 4.0,
                7.0,
                st.base.text_muted.with_alpha(0.55),
                Align::Start,
            );
        }
    }
    c.pop_clip();
}

fn rows(v: &StepEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let l = v.layout();
    c.push_clip(&l.rows.into());
    let gx0 = l.rows.x0 + HEADER_WIDTH;
    let gx1 = gx0 + l.avail_w;
    let s = v.step_ticks();
    let steps = v.grid().steps;
    let alt = mix(st.lane, st.base.text, 0.035);
    for pad in (1..=PAD_COUNT).rev() {
        let y = v.row_y(pad);
        if y + ROW_HEIGHT < l.rows.y0 || y > l.rows.y1 {
            continue;
        }
        let row = Rect::new(gx0, y, gx1, y + ROW_HEIGHT);
        c.push_clip(&row.into());
        sp::rect(c, row, st.lane);
        for k in 0..steps {
            let cell = v.cell_rect(pad, k);
            if cell.x1 < gx0 || cell.x0 > gx1 {
                continue;
            }
            let t = i64::from(k) * s;
            if (t / v.beat_ticks) % 2 == 1 {
                sp::rect(c, cell, alt);
            }
            let bar = t % v.bar_ticks == 0;
            let beat = t % v.beat_ticks == 0;
            let gx = cell.x0.round() + 0.5;
            let col = if bar {
                st.grid_bar
            } else if beat {
                st.grid_beat
            } else {
                st.grid_sub
            };
            sp::line(c, gx, y, gx, y + ROW_HEIGHT, col);
            if let Some(vel) = v.cell_velocity(pad, k) {
                on_cell(v, c, cell, vel, v.cell_muted(pad, k));
            }
        }
        sp::line(
            c,
            gx0,
            y + ROW_HEIGHT - 0.5,
            gx1,
            y + ROW_HEIGHT - 0.5,
            st.grid_beat,
        );
        c.pop_clip();
        header(v, c, pad, y);
    }
    // The end of the grid.
    let end = v.cell_rect(1, steps).x0;
    if end < gx1 {
        sp::rect(c, Rect::new(end, l.rows.y0, gx1, l.rows.y1), st.outside);
    }
    sp::line(c, gx0 - 0.5, l.rows.y0, gx0 - 0.5, l.rows.y1, st.separator);
    c.pop_clip();
}

fn on_cell(v: &StepEditorView, c: &mut dyn Canvas, cell: Rect, vel: f32, muted: bool) {
    let col = level_color(v, VelocityLevel::of_velocity(vel));
    let slot = cell.inset(-2.0);
    sp::rect(c, slot, col.with_alpha(0.16));
    let h = (slot.height() * f64::from(vel.clamp(0.0, 1.0))).max(1.5);
    // A muted cell is hollow: a faint level bar inside the outline.
    let bar = if muted { col.with_alpha(0.2) } else { col };
    sp::rect(c, Rect::new(slot.x0, slot.y1 - h, slot.x1, slot.y1), bar);
    c.stroke_with(slot, 1.0, col.with_alpha(if muted { 0.5 } else { 0.9 }));
    if muted {
        // A strike across the cell: never produced by a velocity level.
        c.stroke_with(
            vello::kurbo::Line::new((slot.x0, slot.y1), (slot.x1, slot.y0)),
            1.0,
            col.with_alpha(0.5),
        );
    }
}

fn header(v: &StepEditorView, c: &mut dyn Canvas, pad: u8, y: f64) {
    let st = &v.st;
    let t = &st.base;
    let r = Rect::new(v.bounds.x0, y, v.bounds.x0 + HEADER_WIDTH, y + ROW_HEIGHT);
    let key = pad_key(pad);
    let sounding = matches!(v.drag, Drag::Audition(p) if p == pad);
    let drop = v.drop.is_some_and(|d| d.pad_key == Some(key));
    let fill = if drop {
        t.highlight.with_alpha(0.3)
    } else if sounding {
        v.accent.with_alpha(0.45)
    } else if v.hover == Hit::Header(pad) {
        mix(st.header, t.text, 0.1)
    } else {
        st.header
    };
    sp::rect(c, r, fill);
    sp::line(c, r.x0, r.y1 - 0.5, r.x1, r.y1 - 0.5, st.separator);
    let (num_col, name_col) = if v.pad_has_sound(pad) {
        (v.accent, t.text)
    } else {
        (t.text_muted.with_alpha(0.6), t.text_muted.with_alpha(0.7))
    };
    c.text(
        &TextRun::new(
            format!("{pad:02}"),
            pt(r.x0 + 8.0, r.y0 + 15.0),
            9.5,
            num_col,
        )
        .weight(700),
    );
    sp::label(
        c,
        &fit_label(&v.pad_name(pad), HEADER_WIDTH - 36.0, 9.0),
        r.x0 + 28.0,
        r.y0 + 15.0,
        9.0,
        name_col,
        Align::Start,
    );
    if drop {
        c.stroke_with(r.inset(-1.0), 2.0, t.highlight);
    }
}

fn empty_state(v: &StepEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let ctr = v.bounds.center();
    sp::label(
        c,
        "No pattern",
        ctr.x,
        ctr.y - 14.0,
        13.0,
        t.text,
        Align::Center,
    );
    sp::label(
        c,
        "Create a one-bar clip with a 16-step grid on this drum machine's track",
        ctr.x,
        ctr.y + 2.0,
        9.5,
        t.text_muted,
        Align::Center,
    );
    sp::button(
        c,
        st,
        v.new_pattern_rect(),
        "NEW PATTERN",
        None,
        v.hover == Hit::NewPattern,
    );
}

fn top_bar(v: &StepEditorView, c: &mut dyn Canvas) {
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
            "STEP EDITOR",
            pt(r.x0 + 14.0, r.y0 + 18.0),
            8.0,
            t.text_muted,
        )
        .weight(700)
        .spacing(1.2),
    );
    if v.present {
        let text = match &v.clip {
            Some(_) => {
                let g = v.grid();
                format!(
                    "{}  ·  {} steps  ·  {}  ·  swing {} %",
                    v.name,
                    g.steps,
                    res_label(g.resolution),
                    g.swing
                )
            }
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

fn drop_indicator(v: &StepEditorView, c: &mut dyn Canvas) {
    let Some(d) = v.drop else { return };
    let st = &v.st;
    let hl = st.base.highlight;
    let plural = if d.count == 1 { "file" } else { "files" };
    match d.pad_key {
        Some(key) => {
            let pad = atomik_model::pad_of_key(key).unwrap_or(1);
            let l = v.layout();
            sp::label(
                c,
                &format!("Drop {} {plural} on pad {pad}", d.count),
                l.rows.x1 - 12.0,
                l.toolbar.y0 + 20.0,
                10.0,
                st.base.text,
                Align::End,
            );
        }
        None => {
            c.stroke_with(v.bounds.inset(-2.0), 3.0, hl);
            sp::label(
                c,
                &format!("Drop {} {plural} to fill empty pads", d.count),
                v.bounds.center().x,
                v.bounds.center().y,
                11.0,
                st.base.text,
                Align::Center,
            );
        }
    }
}
