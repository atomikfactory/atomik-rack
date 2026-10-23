//! Painting of the sample editor (see the parent module for the layout).

use super::{AXIS_WIDTH, Drag, EditKind, Hit, LIST_HEAD, SampleEditorView};
use crate::audio;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::fit_label;
use crate::seq_paint as sp;
use crate::theme::mix;
use crate::wave_paint::{MAX_COLUMNS, WaveChannel, WaveDetail, paint_channel};
use crate::zone_edit::{self, Field, Marker};
use atomik_device_sdk::{LoopMode, TriggerMode};
use atomik_model::ZoneDoc;
use vello::kurbo::{Affine, BezPath, Rect};
use vello::peniko::Color;

fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

pub(super) fn paint(v: &SampleEditorView, c: &mut dyn Canvas) {
    let b = v.bounds;
    c.push_transform(Affine::scale(v.scale));
    c.push_clip(&b.into());
    sp::rect(c, b, v.st.background);
    if v.device.is_some() && v.present {
        map(v, c);
        keyboard(v, c);
        list(v, c);
        head(v, c);
        waveform(v, c);
        fields(v, c);
    } else {
        sp::label(
            c,
            "No sampler open",
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

fn zone_tint(v: &SampleEditorView, i: usize, z: &ZoneDoc) -> Color {
    if v.is_missing(z) {
        v.st.record
    } else {
        mix(v.accent, v.st.base.text, [0.0, 0.28, 0.5, 0.14][i % 4])
    }
}

fn asset_name(v: &SampleEditorView, z: &ZoneDoc) -> String {
    v.metas
        .get(&z.asset)
        .map_or_else(|| "missing asset".to_string(), |m| m.original_name.clone())
}

fn map(v: &SampleEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let l = v.layout();
    let g = v.geo();
    let m = l.map;
    // Velocity axis.
    let axis = Rect::new(v.bounds.x0, m.y0, m.x0, m.y1);
    sp::rect(c, axis, st.header);
    for vel in [127u8, 96, 64, 32, 1] {
        sp::label(
            c,
            &vel.to_string(),
            axis.x1 - 4.0,
            v.velocity_y(vel) + 3.0,
            7.5,
            st.base.text_muted,
            Align::End,
        );
    }
    c.push_clip(&m.into());
    sp::rect(c, m, st.lane);
    let (k0, k1) = (v.key_at(m.x0), v.key_at(m.x1 - 0.01));
    for k in k0..=k1 {
        let x = v.key_x(k);
        if is_black(k) {
            sp::rect(c, Rect::new(x, m.y0, x + g.ppk, m.y1), st.row_black);
        }
        let gx = x.round() + 0.5;
        if k % 12 == 0 {
            sp::line(c, gx, m.y0, gx, m.y1, st.grid_bar);
        } else if g.ppk >= 6.0 {
            sp::line(c, gx, m.y0, gx, m.y1, st.grid_sub);
        }
    }
    for vel in [96u8, 64, 32] {
        let y = v.velocity_y(vel).round() + 0.5;
        sp::line(c, m.x0, y, m.x1, y, st.grid_beat);
    }
    if v.zones.is_empty() {
        sp::label(
            c,
            "Drop sample files here",
            m.center().x,
            m.center().y,
            10.0,
            st.base.text_muted,
            Align::Center,
        );
    }
    for i in (0..v.zones.len()).filter(|i| Some(*i) != v.selected) {
        zone(v, c, i, false);
    }
    if let Some(i) = v.selected {
        zone(v, c, i, true);
    }
    if let Drag::Keys { key, .. } = &v.drag {
        column(v, c, *key, v.accent.with_alpha(0.25));
    }
    sp::line(c, m.x0, m.y1 - 0.5, m.x1, m.y1 - 0.5, st.separator);
    c.pop_clip();
}

fn column(v: &SampleEditorView, c: &mut dyn Canvas, key: u8, color: Color) {
    let m = v.layout().map;
    let w = v.geo().ppk.max(2.0);
    sp::rect(
        c,
        Rect::new(v.key_x(key), m.y0, v.key_x(key) + w, m.y1),
        color,
    );
}

fn zone(v: &SampleEditorView, c: &mut dyn Canvas, i: usize, selected: bool) {
    let st = &v.st;
    let z = &v.zones[i];
    let r = v.rect_of(z);
    let tint = zone_tint(v, i, z);
    let missing = v.is_missing(z);
    let alpha = match (selected, missing) {
        (true, _) => 0.5,
        (false, true) => 0.14,
        (false, false) => 0.28,
    };
    c.fill_with(r, tint.with_alpha(alpha));
    let (w, col) = if selected {
        (2.0, st.selection)
    } else {
        (1.0, tint.with_alpha(0.9))
    };
    c.stroke_with(r, w, col);
    let rx = v.key_center_x(z.root_key);
    let root = if selected {
        st.base.text
    } else {
        st.base.text.with_alpha(0.6)
    };
    if rx >= r.x0 - 0.5 && rx <= r.x1 + 0.5 {
        sp::line(c, rx, r.y0, rx, r.y1, root);
    }
    let mut tip = BezPath::new();
    tip.move_to(pt(rx - 3.5, r.y1));
    tip.line_to(pt(rx + 3.5, r.y1));
    tip.line_to(pt(rx, r.y1 - 6.0));
    tip.close_path();
    c.fill_with(tip, root);
    if r.width() > 46.0 && r.height() > 14.0 {
        let name = fit_label(&asset_name(v, z), r.width() - 8.0, 8.0);
        sp::label(
            c,
            &name,
            r.x0 + 4.0,
            r.y0 + 11.0,
            8.0,
            st.base.text,
            Align::Start,
        );
    }
}

fn keyboard(v: &SampleEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout();
    let kb = l.keys;
    let g = v.geo();
    c.push_clip(&kb.into());
    sp::rect(c, kb, st.key_white);
    let sounding = match &v.drag {
        Drag::Keys { key, .. } => Some(*key),
        Drag::Edit(e) => e.sounding.map(|(k, _)| k),
        _ => None,
    };
    let (k0, k1) = (v.key_at(kb.x0), v.key_at(kb.x1 - 0.01));
    for k in k0..=k1 {
        let x = v.key_x(k);
        let col = Rect::new(x, kb.y0, x + g.ppk, kb.y1);
        if is_black(k) {
            let r = Rect::new(
                col.x0 + 0.5,
                kb.y0,
                col.x1 - 0.5,
                kb.y0 + kb.height() * 0.62,
            );
            let fill = if sounding == Some(k) {
                v.accent
            } else {
                st.key_black
            };
            c.fill_with(r, fill);
        } else {
            if sounding == Some(k) {
                sp::rect(c, col, mix(st.key_white, v.accent, 0.6));
            }
            sp::line(
                c,
                x.round() + 0.5,
                kb.y0 + kb.height() * 0.62,
                x.round() + 0.5,
                kb.y1,
                t.text_muted.with_alpha(0.5),
            );
        }
        if k % 12 == 0 {
            sp::label(
                c,
                &zone_edit::note_name(k),
                x + 2.0,
                kb.y1 - 3.0,
                7.5,
                t.plate,
                Align::Start,
            );
        }
    }
    sp::line(c, kb.x0, kb.y0 + 0.5, kb.x1, kb.y0 + 0.5, t.bevel_dark);
    c.pop_clip();
    sp::rect(c, Rect::new(v.bounds.x0, kb.y0, kb.x0, kb.y1), st.header);
}

fn list(v: &SampleEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout().list;
    c.push_clip(&l.into());
    sp::rect(c, l, st.header);
    c.text(
        &TextRun::new("ZONES", pt(l.x0 + 8.0, l.y0 + 14.0), 8.0, t.text_muted)
            .weight(700)
            .spacing(1.0),
    );
    sp::label(
        c,
        &v.zones.len().to_string(),
        l.x1 - 8.0,
        l.y0 + 14.0,
        8.0,
        t.text_muted,
        Align::End,
    );
    let rows = Rect::new(l.x0, l.y0 + LIST_HEAD, l.x1, l.y1);
    c.push_clip(&rows.into());
    for (i, z) in v.zones.iter().enumerate() {
        let r = v.row_rect(i);
        if r.y1 < rows.y0 || r.y0 > rows.y1 {
            continue;
        }
        let selected = v.selected == Some(i);
        let fill = if selected {
            st.selection.with_alpha(0.22)
        } else if v.hover == Hit::Row(i) {
            t.text.with_alpha(0.07)
        } else {
            t.text.with_alpha(0.03)
        };
        c.fill_with(r, fill);
        if selected {
            c.stroke_with(r, 1.0, st.selection.with_alpha(0.8));
        }
        sp::rect(
            c,
            Rect::new(r.x0, r.y0, r.x0 + 3.0, r.y1),
            zone_tint(v, i, z),
        );
        let name = fit_label(&asset_name(v, z), r.width() - 16.0, 9.5);
        let name_col = if v.is_missing(z) { st.record } else { t.text };
        sp::label(
            c,
            &name,
            r.x0 + 8.0,
            r.y0 + 12.0,
            9.5,
            name_col,
            Align::Start,
        );
        let summary = fit_label(&SampleEditorView::zone_summary(z), r.width() - 16.0, 8.0);
        sp::label(
            c,
            &summary,
            r.x0 + 8.0,
            r.y0 + 24.0,
            8.0,
            t.text_muted,
            Align::Start,
        );
    }
    if v.zones.is_empty() {
        sp::label(
            c,
            "No zones",
            l.center().x,
            l.y0 + LIST_HEAD + 18.0,
            9.0,
            t.text_muted,
            Align::Center,
        );
    }
    c.pop_clip();
    sp::line(c, l.x0 + 0.5, l.y0, l.x0 + 0.5, l.y1, st.separator);
    c.pop_clip();
}

fn head(v: &SampleEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout().head;
    sp::rect(c, l, st.header);
    sp::line(c, l.x0, l.y0 + 0.5, l.x1, l.y0 + 0.5, st.separator);
    let zone = v.selected.and_then(|i| v.zones.get(i));
    let x0 = l.x0 + AXIS_WIDTH;
    let ty = l.center().y + 3.0;
    sp::label(c, "LOOP", x0, ty, 8.0, t.text_muted, Align::Start);
    for (mode, text) in [
        (LoopMode::Off, "OFF"),
        (LoopMode::Forward, "FWD"),
        (LoopMode::PingPong, "PING"),
    ] {
        let lit = zone
            .is_some_and(|z| z.loop_mode == mode)
            .then_some(v.accent);
        sp::button(
            c,
            st,
            v.loop_rect(mode),
            text,
            lit,
            v.hover == Hit::Loop(mode),
        );
    }
    sp::label(
        c,
        "TRIGGER",
        x0 + 180.0,
        ty,
        8.0,
        t.text_muted,
        Align::Start,
    );
    for (mode, text) in [
        (TriggerMode::Gate, "GATE"),
        (TriggerMode::OneShot, "ONE-SHOT"),
    ] {
        let lit = zone.is_some_and(|z| z.trigger == mode).then_some(v.accent);
        sp::button(
            c,
            st,
            v.trigger_rect(mode),
            text,
            lit,
            v.hover == Hit::Trigger(mode),
        );
    }
    if let Some((_, z, m)) = v.selected_with_meta() {
        let secs = (z.end - z.start) as f64 / f64::from(m.sample_rate.max(1));
        let info = format!(
            "{}  ·  {}ch  ·  {:.1} kHz  ·  {secs:.2} s",
            m.original_name,
            m.channels,
            f64::from(m.sample_rate) / 1000.0
        );
        let room = (l.x1 - (v.trigger_rect(TriggerMode::OneShot).x1 + 16.0)).max(20.0);
        sp::label(
            c,
            &fit_label(&info, room, 9.0),
            l.x1 - 8.0,
            ty,
            9.0,
            t.text,
            Align::End,
        );
    }
}

fn waveform(v: &SampleEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout();
    let w = l.wave;
    sp::rect(c, Rect::new(v.bounds.x0, w.y0, w.x0, w.y1), st.header);
    c.push_clip(&w.into());
    sp::rect(c, w, st.lane_auto);
    let Some(i) = v.selected else {
        sp::label(
            c,
            "No zone selected",
            w.center().x,
            w.center().y,
            10.0,
            t.text_muted,
            Align::Center,
        );
        c.pop_clip();
        return;
    };
    let z = &v.zones[i];
    let Some((_, _, meta)) = v.selected_with_meta() else {
        sp::label(
            c,
            "Missing asset: the sample file is not available",
            w.center().x,
            w.center().y,
            10.0,
            st.record,
            Align::Center,
        );
        c.pop_clip();
        return;
    };
    let g = v.geo();
    let tint = zone_tint(v, i, z);
    if let Some(peaks) = v.peaks.get(&z.asset) {
        let samples = v.samples.get(&z.asset).map(|s| &s[..]);
        let detail = WaveDetail::pick(peaks, samples, g.fpp);
        let wave = st.audio_wave(tint, false);
        let bands = audio::channel_bands(w.y0 + 4.0, w.y1 - 4.0, detail.channels());
        let columns = (w.width().max(0.0) as usize).min(MAX_COLUMNS);
        let source = |x: f64| g.wave_scroll + (x - w.x0) * g.fpp;
        let frame_x = |f: f64| w.x0 + (f - g.wave_scroll) / g.fpp;
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
                limit: meta.frames,
                frames_per_pixel: g.fpp,
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
    let fx = |f: u64| v.frame_x(f);
    // Outside start..end.
    let (xs, xe) = (fx(z.start), fx(z.end));
    if xs > w.x0 {
        sp::rect(c, Rect::new(w.x0, w.y0, xs.min(w.x1), w.y1), st.outside);
    }
    if xe < w.x1 {
        sp::rect(c, Rect::new(xe.max(w.x0), w.y0, w.x1, w.y1), st.outside);
    }
    if z.loop_mode != LoopMode::Off {
        let (ls, le) = (fx(z.loop_start), fx(z.loop_end));
        sp::rect(
            c,
            Rect::new(ls, w.y0, le, w.y1),
            st.loop_on.with_alpha(0.08),
        );
        if z.crossfade > 0 {
            let xf = fx(z.loop_start - z.crossfade);
            sp::rect(c, Rect::new(xf, w.y0, ls, w.y1), v.accent.with_alpha(0.28));
            sp::line(c, xf.round() + 0.5, w.y0, xf.round() + 0.5, w.y1, v.accent);
        }
    }
    let active = match &v.drag {
        Drag::Edit(e) => match e.kind {
            EditKind::Marker(m, _) => Some(m),
            _ => None,
        },
        _ => None,
    };
    for m in [
        Marker::Start,
        Marker::End,
        Marker::LoopStart,
        Marker::LoopEnd,
    ] {
        if !SampleEditorView::marker_visible(z, m) {
            continue;
        }
        let x = fx(zone_edit::marker_frame(z, m)).round() + 0.5;
        let lit = active == Some(m) || v.hover == Hit::Marker(m);
        let loop_type = matches!(m, Marker::LoopStart | Marker::LoopEnd);
        let col = if loop_type { st.loop_on } else { t.text };
        let width = if lit { 2.5 } else { 1.5 };
        c.stroke_with(
            vello::kurbo::Line::new(pt(x, w.y0), pt(x, w.y1)),
            width,
            col,
        );
        // A flag pointing into the played region, at the top (start/end) or bottom (loop).
        let (y0, dy) = if loop_type { (w.y1, -1.0) } else { (w.y0, 1.0) };
        let dx = if matches!(m, Marker::Start | Marker::LoopStart) {
            1.0
        } else {
            -1.0
        };
        let mut flag = BezPath::new();
        flag.move_to(pt(x, y0));
        flag.line_to(pt(x + dx * 10.0, y0));
        flag.line_to(pt(x, y0 + dy * 10.0));
        flag.close_path();
        c.fill_with(flag, col);
    }
    c.pop_clip();
}

fn fields(v: &SampleEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout().fields;
    sp::rect(c, l, st.header);
    sp::line(c, l.x0, l.y0 + 0.5, l.x1, l.y0 + 0.5, st.separator);
    let zone = v.selected.and_then(|i| v.zones.get(i));
    let dragging = match &v.drag {
        Drag::Edit(e) => match e.kind {
            EditKind::Field(f, _) => Some(f),
            _ => None,
        },
        _ => None,
    };
    for f in Field::ALL {
        let r = v.field_rect(f);
        c.text(
            &TextRun::new(f.label(), pt(r.x0 + 2.0, r.y0 + 8.0), 7.0, t.text_muted)
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
            |z| zone_edit::format_field(z, f, v.rate_of(z)),
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
}

fn top_bar(v: &SampleEditorView, c: &mut dyn Canvas) {
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
            "SAMPLE EDITOR",
            pt(r.x0 + 14.0, r.y0 + 18.0),
            8.0,
            t.text_muted,
        )
        .weight(700)
        .spacing(1.2),
    );
    if v.present {
        let bytes: u64 = v
            .metas
            .values()
            .map(|m| m.frames * u64::from(m.channels) * 4)
            .sum();
        let size = if bytes >= 1 << 20 {
            format!("{:.1} MiB", bytes as f64 / f64::from(1u32 << 20))
        } else {
            format!("{:.0} KiB", bytes as f64 / 1024.0)
        };
        let n = v.zones.len();
        let text = format!(
            "{}  ·  {n} {}  ·  {size}",
            v.name,
            if n == 1 { "zone" } else { "zones" }
        );
        let text = fit_label(&text, (l.load.x0 - r.x0 - 130.0).max(20.0), 11.0);
        c.text(&TextRun::new(text, pt(r.x0 + 118.0, r.y0 + 18.5), 11.0, t.text).weight(600));
    }
    if v.present {
        sp::button(c, st, l.load, "LOAD...", None, v.hover == Hit::Load);
    }
    sp::button(c, st, l.close, "", None, v.hover == Hit::Close);
    let k = l.close.center();
    for (dx, dy) in [(4.0, 4.0), (4.0, -4.0)] {
        c.stroke_with(
            vello::kurbo::Line::new(pt(k.x - dx, k.y - dy), pt(k.x + dx, k.y + dy)),
            1.6,
            t.text,
        );
    }
}

fn drop_indicator(v: &SampleEditorView, c: &mut dyn Canvas) {
    let Some(d) = v.drop else { return };
    let st = &v.st;
    let l = v.layout();
    let hl = st.base.highlight;
    let plural = if d.count == 1 { "file" } else { "files" };
    match d.at_key {
        Some(key) => {
            c.push_clip(&l.map.into());
            column(v, c, key, hl.with_alpha(0.35));
            c.pop_clip();
            c.stroke_with(l.map.inset(-1.0), 2.0, hl);
            c.stroke_with(l.keys.inset(-1.0), 2.0, hl);
            sp::label(
                c,
                &format!("Drop {} {plural} at {}", d.count, zone_edit::note_name(key)),
                l.map.center().x,
                l.map.y0 + 16.0,
                10.0,
                st.base.text,
                Align::Center,
            );
        }
        None => {
            c.stroke_with(v.bounds.inset(-2.0), 3.0, hl);
            sp::label(
                c,
                &format!("Drop {} {plural} to add zones", d.count),
                v.bounds.center().x,
                v.bounds.center().y,
                11.0,
                st.base.text,
                Align::Center,
            );
        }
    }
}
