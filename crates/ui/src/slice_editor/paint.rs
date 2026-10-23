//! Painting of the slice editor (see the parent module for the layout).

use super::{DIVISIONS, Drag, Hit, SLICE_FIELDS, SliceEditorView, ZoneDragKind};
use crate::audio;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::fit_label;
use crate::seq_paint as sp;
use crate::theme::mix;
use crate::wave_paint::{MAX_COLUMNS, WaveChannel, WaveDetail, paint_channel};
use crate::zone_edit;
use vello::kurbo::{Affine, BezPath, Line, Rect};

pub(super) fn paint(v: &SliceEditorView, c: &mut dyn Canvas) {
    let b = v.bounds;
    c.push_transform(Affine::scale(v.scale));
    c.push_clip(&b.into());
    sp::rect(c, b, v.st.background);
    if v.device.is_some() && v.present {
        toolbar(v, c);
        if v.order.is_empty() {
            empty(v, c);
        } else {
            strip(v, c);
            waveform(v, c);
            fields(v, c);
        }
    } else {
        sp::label(
            c,
            "No loop slicer open",
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

fn muted_label(v: &SliceEditorView, c: &mut dyn Canvas, text: &str, x: f64, y: f64) {
    c.text(
        &TextRun::new(text, pt(x, y), 7.0, v.st.base.text_muted)
            .weight(700)
            .spacing(0.8),
    );
}

fn toolbar(v: &SliceEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout();
    sp::rect(c, l.bar, st.header);
    sp::line(
        c,
        l.bar.x0,
        l.bar.y1 - 0.5,
        l.bar.x1,
        l.bar.y1 - 0.5,
        st.separator,
    );
    let ty = l.bar.center().y + 2.5;
    muted_label(v, c, "MODE", l.bar.x0 + 6.0, ty);
    let lit = |on: bool| on.then_some(v.accent);
    sp::button(
        c,
        st,
        l.mode_transients,
        "TRANSIENTS",
        lit(!v.grid_mode),
        v.hover == Hit::ModeTransients,
    );
    sp::button(
        c,
        st,
        l.mode_grid,
        "GRID",
        lit(v.grid_mode),
        v.hover == Hit::ModeGrid,
    );
    muted_label(v, c, "SENS", l.sens.x0 - 28.0, ty);
    let sens_drag = matches!(v.drag, Drag::Sens { .. });
    let border = if sens_drag {
        Some(v.accent)
    } else {
        (v.hover == Hit::Sens).then_some(t.focus)
    };
    sp::lcd(c, st, l.sens, border);
    let col = if v.grid_mode {
        t.text_muted
    } else {
        t.highlight
    };
    sp::label(
        c,
        &format!("{} %", v.sensitivity),
        l.sens.center().x,
        l.sens.center().y + 3.5,
        9.5,
        col,
        Align::Center,
    );
    muted_label(v, c, "DIV", l.divs[0].x0 - 22.0, ty);
    for (i, (d, text)) in DIVISIONS.iter().enumerate() {
        sp::button(
            c,
            st,
            l.divs[i],
            text,
            lit(v.grid_mode && v.division == *d),
            v.hover == Hit::Div(*d),
        );
    }
    muted_label(v, c, "BEATS", l.beats.x0 - 36.0, ty);
    let beats_border = if matches!(v.drag, Drag::Beats { .. }) {
        Some(v.accent)
    } else {
        (v.hover == Hit::Beats).then_some(t.focus)
    };
    sp::lcd(c, st, l.beats, beats_border);
    sp::label(
        c,
        &v.beats.to_string(),
        l.beats.center().x,
        l.beats.center().y + 3.5,
        9.5,
        t.highlight,
        Align::Center,
    );
    let ready = v.editable();
    sp::button(
        c,
        st,
        l.slice_button,
        "SLICE",
        None,
        ready && v.hover == Hit::SliceButton,
    );
    sp::button(
        c,
        st,
        l.to_track,
        "TO TRACK",
        None,
        ready && v.hover == Hit::ToTrack,
    );
    if !ready {
        for r in [l.slice_button, l.to_track] {
            sp::rect(c, r, t.background.with_alpha(0.55));
        }
    }
}

fn empty(v: &SliceEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let l = v.layout();
    let area = Rect::new(l.strip.x0, l.strip.y0, l.strip.x1, l.wave.y1);
    sp::rect(c, area, st.lane_auto);
    c.stroke_with(area.inset(-6.0), 1.0, st.base.text_muted.with_alpha(0.35));
    let ctr = area.center();
    sp::label(
        c,
        "Drop a loop here",
        ctr.x,
        ctr.y - 6.0,
        13.0,
        st.base.text,
        Align::Center,
    );
    sp::button(
        c,
        st,
        l.empty_load,
        "LOAD...",
        None,
        v.hover == Hit::EmptyLoad,
    );
}

fn strip(v: &SliceEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout();
    sp::rect(c, l.strip, st.header);
    c.push_clip(&l.strip.into());
    for i in 0..v.order.len() {
        let (Some(z), Some((a, b))) = (v.zone(i), v.slice_range(i)) else {
            continue;
        };
        let x0 = v.frame_x(a).max(l.strip.x0);
        let x1 = v.frame_x(b).min(l.strip.x1);
        if x1 <= x0 {
            continue;
        }
        let cell = Rect::new(x0, l.strip.y0 + 1.0, x1 - 1.0, l.strip.y1 - 1.0);
        let selected = v.selected == Some(i);
        let face = if selected {
            mix(v.accent, t.panel_bottom, 0.55)
        } else if i % 2 == 0 {
            t.panel_bottom
        } else {
            mix(t.panel_bottom, t.background, 0.4)
        };
        sp::rect(c, cell, face);
        let col = if selected { t.text } else { t.text_muted };
        let key = zone_edit::note_name(z.key_lo);
        let room = cell.width() - 6.0;
        if room >= 8.0 {
            let text = format!("{}", i + 1);
            c.text(&TextRun::new(text, pt(cell.x0 + 4.0, cell.y0 + 10.0), 9.0, col).weight(700));
        }
        if room >= 34.0 {
            c.text(&TextRun::new(
                fit_label(&key, room - 8.0, 8.0),
                pt(cell.x0 + 4.0, cell.y1 - 4.0),
                8.0,
                col,
            ));
        }
        if z.reverse && room >= 14.0 {
            c.text(
                &TextRun::new("R", pt(cell.x1 - 4.0, cell.y0 + 10.0), 9.0, t.highlight)
                    .align(Align::End)
                    .weight(700),
            );
        }
    }
    c.pop_clip();
}

fn waveform(v: &SliceEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout();
    let w = l.wave;
    c.push_clip(&w.into());
    sp::rect(c, w, st.lane_auto);
    let asset = v.asset().unwrap_or_default();
    let (_, scroll, fpp) = v.wave_geo();
    let missing = v.meta.is_none() || v.missing.contains(asset);
    if missing {
        sp::label(
            c,
            "Missing asset: the loop file is not available",
            w.center().x,
            w.center().y,
            10.0,
            st.record,
            Align::Center,
        );
    } else if let Some(peaks) = v.peaks.get(asset) {
        let samples = v.samples.get(asset).map(|s| &s[..]);
        let detail = WaveDetail::pick(peaks, samples, fpp);
        let wave = st.audio_wave(v.accent, false);
        let bands = audio::channel_bands(w.y0 + 4.0, w.y1 - 4.0, detail.channels());
        let columns = (w.width().max(0.0) as usize).min(MAX_COLUMNS);
        let source = |x: f64| scroll + (x - w.x0) * fpp;
        let frame_x = |f: f64| w.x0 + (f - scroll) / fpp;
        for (ch, (cy, half)) in bands.into_iter().enumerate() {
            let channel = WaveChannel {
                detail,
                ch,
                cy,
                half,
                gain: 1.0,
                color: wave,
                x_start: w.x0,
                columns,
                line_x: (w.x0, w.x1),
                limit: v.frames,
                frames_per_pixel: fpp,
            };
            paint_channel(c, &channel, &source, &frame_x);
        }
    } else {
        sp::label(
            c,
            "no waveform",
            w.center().x,
            w.center().y,
            9.0,
            t.text_muted,
            Align::Center,
        );
    }
    // The selected slice, and the reversed ones.
    for i in 0..v.order.len() {
        let (Some(z), Some((a, b))) = (v.zone(i), v.slice_range(i)) else {
            continue;
        };
        let (x0, x1) = (v.frame_x(a).max(w.x0), v.frame_x(b).min(w.x1));
        if x1 <= x0 {
            continue;
        }
        if v.selected == Some(i) {
            sp::rect(c, Rect::new(x0, w.y0, x1, w.y1), v.accent.with_alpha(0.16));
        }
        if z.reverse && x1 - x0 >= 14.0 {
            let (cx, y) = ((x0 + x1) / 2.0, w.y0 + 9.0);
            let mut tri = BezPath::new();
            tri.move_to(pt(cx - 6.0, y));
            tri.line_to(pt(cx + 4.0, y - 5.0));
            tri.line_to(pt(cx + 4.0, y + 5.0));
            tri.close_path();
            c.fill_with(tri, t.highlight);
        }
    }
    let active = match &v.drag {
        Drag::Zone(e) if matches!(e.kind, ZoneDragKind::Marker(_)) => Some(e.slice),
        _ => None,
    };
    for (i, m) in v.markers().into_iter().enumerate() {
        let x = v.frame_x(m).round() + 0.5;
        if x < w.x0 - 1.0 || x > w.x1 + 1.0 {
            continue;
        }
        let lit = active == Some(i) || v.hover == Hit::Marker(i);
        let sel = v.selected == Some(i);
        let col = if sel { v.accent } else { t.text };
        let width = if lit { 2.5 } else { 1.5 };
        c.stroke_with(
            Line::new(pt(x, l.strip.y0), pt(x, w.y1)),
            width,
            col.with_alpha(if i == 0 { 0.5 } else { 1.0 }),
        );
        let mut flag = BezPath::new();
        flag.move_to(pt(x, w.y1));
        flag.line_to(pt(x + 9.0, w.y1));
        flag.line_to(pt(x, w.y1 - 9.0));
        flag.close_path();
        c.fill_with(flag, col);
    }
    c.pop_clip();
}

fn fields(v: &SliceEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout().fields;
    sp::rect(c, l, st.header);
    sp::line(c, l.x0, l.y0 + 0.5, l.x1, l.y0 + 0.5, st.separator);
    let zone = v.selected.and_then(|i| v.zone(i));
    let dragging = match &v.drag {
        Drag::Zone(e) => match e.kind {
            ZoneDragKind::Field(f, _) => Some(f),
            ZoneDragKind::Marker(_) => None,
        },
        _ => None,
    };
    for (f, name) in SLICE_FIELDS.iter().zip(["LEVEL", "PAN", "PITCH", "TUNE"]) {
        let f = *f;
        let r = v.field_rect(f);
        c.text(
            &TextRun::new(name, pt(r.x0 + 2.0, r.y0 + 8.0), 7.0, t.text_muted)
                .weight(700)
                .spacing(0.8),
        );
        let vr = v.field_value_rect(f);
        let border = if dragging == Some(f) {
            Some(v.accent)
        } else {
            (v.hover == Hit::Field(f)).then_some(t.focus)
        };
        sp::lcd(c, st, vr, border);
        let text = zone.map_or_else(
            || "-".to_string(),
            |z| zone_edit::format_field(z, f, v.doc_rate),
        );
        let col = if zone.is_some() {
            t.highlight
        } else {
            t.text_muted
        };
        sp::label(
            c,
            &text,
            vr.center().x,
            vr.center().y + 3.5,
            9.5,
            col,
            Align::Center,
        );
    }
    let rr = v.reverse_rect();
    c.text(
        &TextRun::new("DIRECTION", pt(rr.x0 + 2.0, rr.y0 - 6.0), 7.0, t.text_muted)
            .weight(700)
            .spacing(0.8),
    );
    let lit = zone.is_some_and(|z| z.reverse).then_some(v.accent);
    sp::button(c, st, rr, "REVERSE", lit, v.hover == Hit::Reverse);
    if let (Some(s), Some(z)) = (v.selected, zone) {
        let ms = (z.end - z.start) as f64 * 1000.0 / f64::from(v.doc_rate.max(1));
        let info = format!(
            "SLICE {}  ·  key {}  ·  {ms:.0} ms",
            s + 1,
            zone_edit::note_name(z.key_lo)
        );
        sp::label(
            c,
            &info,
            l.x1 - 10.0,
            l.center().y + 3.0,
            9.5,
            t.text,
            Align::End,
        );
    }
}

fn top_bar(v: &SliceEditorView, c: &mut dyn Canvas) {
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
            "SLICE EDITOR",
            pt(r.x0 + 14.0, r.y0 + 18.0),
            8.0,
            t.text_muted,
        )
        .weight(700)
        .spacing(1.2),
    );
    if v.present {
        let n = v.order.len();
        let loop_name = v
            .meta
            .as_ref()
            .map_or("no loop", |m| m.original_name.as_str());
        let text = format!(
            "{}  ·  {n} {}  ·  {loop_name}",
            v.name,
            if n == 1 { "slice" } else { "slices" }
        );
        let text = fit_label(&text, (l.load.x0 - r.x0 - 130.0).max(20.0), 11.0);
        c.text(&TextRun::new(text, pt(r.x0 + 112.0, r.y0 + 18.5), 11.0, t.text).weight(600));
        sp::button(c, st, l.load, "LOAD...", None, v.hover == Hit::Load);
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

fn drop_indicator(v: &SliceEditorView, c: &mut dyn Canvas) {
    let Some(count) = v.drop else { return };
    let st = &v.st;
    let plural = if count == 1 { "file" } else { "files" };
    c.stroke_with(v.bounds.inset(-2.0), 3.0, st.base.highlight);
    sp::label(
        c,
        &format!("Drop {count} {plural} to replace the loop and slice it"),
        v.bounds.center().x,
        v.bounds.center().y,
        11.0,
        st.base.text,
        Align::Center,
    );
}
