//! Painting of the audio clip editor (see the parent module for the layout).

use super::{AudioEditorView, AudioMode, Drag, Hit};
use crate::audio;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::fit_label;
use crate::seq_paint as sp;
use crate::timeline::bar_beat_label;
use crate::wave_paint::{MAX_COLUMNS, WaveChannel, WaveDetail, paint_channel};
use vello::kurbo::{Affine, BezPath, Line, Rect};

pub(super) fn paint(v: &AudioEditorView, c: &mut dyn Canvas) {
    let b = v.bounds;
    c.push_transform(Affine::scale(v.scale));
    c.push_clip(&b.into());
    sp::rect(c, b, v.st.background);
    if v.present {
        toolbar(v, c);
        ruler(v, c);
        waveform(v, c);
        footer(v, c);
    } else {
        sp::label(
            c,
            "No audio clip open",
            b.center().x,
            b.center().y,
            11.0,
            v.st.base.text_muted,
            Align::Center,
        );
    }
    top_bar(v, c);
    if v.focused {
        sp::line(c, b.x0, b.y0 + 0.5, b.x1, b.y0 + 0.5, v.st.base.focus);
    }
    c.pop_clip();
    c.pop_transform();
}

fn muted_label(v: &AudioEditorView, c: &mut dyn Canvas, text: &str, x: f64, y: f64) {
    c.text(
        &TextRun::new(text, pt(x, y), 7.0, v.st.base.text_muted)
            .weight(700)
            .spacing(0.8),
    );
}

fn top_bar(v: &AudioEditorView, c: &mut dyn Canvas) {
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
            "AUDIO CLIP",
            pt(r.x0 + 14.0, r.y0 + 18.0),
            8.0,
            t.text_muted,
        )
        .weight(700)
        .spacing(1.2),
    );
    if v.present {
        if let Some(text) = &v.rename {
            sp::lcd(c, st, l.title, Some(t.focus));
            let text = format!("{text}|");
            let text = fit_label(&text, l.title.width() - 12.0, 11.0);
            c.text(
                &TextRun::new(text, pt(l.title.x0 + 8.0, r.y0 + 18.5), 11.0, t.text).weight(600),
            );
        } else {
            let name = v
                .meta
                .as_ref()
                .map_or("unknown file", |m| m.original_name.as_str());
            let text = format!("{}  ·  {name}", v.track_name);
            let text = fit_label(&text, l.title.width() - 12.0, 11.0);
            c.text(
                &TextRun::new(text, pt(l.title.x0 + 8.0, r.y0 + 18.5), 11.0, t.text).weight(600),
            );
        }
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

fn toolbar(v: &AudioEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
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
    for (i, m) in AudioMode::ALL.iter().enumerate() {
        let lit = (v.mode == *m).then_some(v.accent);
        let hover = m.is_enabled() && v.hover == Hit::Mode(*m);
        sp::button(c, st, l.modes[i], m.label(), lit, hover);
        if !m.is_enabled() {
            sp::rect(c, l.modes[i], st.base.background.with_alpha(0.55));
        }
    }
    sp::button(
        c,
        st,
        l.speaker,
        "SPEAKER",
        v.speaker.then_some(v.accent),
        v.hover == Hit::Speaker,
    );
    if v.mode != AudioMode::Slice {
        return;
    }
    muted_label(v, c, "DRAG", l.stretch.x0 - 30.0, ty);
    sp::button(
        c,
        st,
        l.stretch,
        "STRETCH",
        (!v.slide).then_some(v.accent),
        v.hover == Hit::Stretch,
    );
    sp::button(
        c,
        st,
        l.slide,
        "SLIDE",
        v.slide.then_some(v.accent),
        v.hover == Hit::Slide,
    );
    let has_markers = v.framed.len() > 2;
    let ready = v.editable();
    let revert_on = ready
        && v.info
            .as_ref()
            .is_some_and(|i| !i.content.warp.markers.is_empty());
    sp::button(
        c,
        st,
        l.revert,
        "REVERT",
        None,
        revert_on && v.hover == Hit::Revert,
    );
    sp::button(
        c,
        st,
        l.split,
        "SPLIT",
        None,
        ready && has_markers && v.hover == Hit::Split,
    );
    sp::button(
        c,
        st,
        l.to_slicer,
        "TO SLICER",
        None,
        ready && has_markers && v.hover == Hit::ToSlicer,
    );
    let detecting = ready;
    muted_label(v, c, "SENS", l.sens_down.x0 - 32.0, ty);
    sp::button(
        c,
        st,
        l.sens_down,
        "-",
        None,
        detecting && v.hover == Hit::SensDown,
    );
    sp::label(
        c,
        &format!("{}", v.sensitivity),
        (l.sens_down.x1 + l.sens_up.x0) / 2.0,
        l.bar.center().y + 4.0,
        10.0,
        st.base.text,
        Align::Center,
    );
    sp::button(
        c,
        st,
        l.sens_up,
        "+",
        None,
        detecting && v.hover == Hit::SensUp,
    );
    let can_add = ready && !v.transients.is_empty();
    sp::button(
        c,
        st,
        l.add_all,
        "ADD ALL",
        None,
        can_add && v.hover == Hit::AddAll,
    );
    for (r, on) in [
        (l.revert, revert_on),
        (l.split, ready && has_markers),
        (l.to_slicer, ready && has_markers),
        (l.sens_down, ready),
        (l.sens_up, ready),
        (l.add_all, can_add),
    ] {
        if !on {
            sp::rect(c, r, st.base.background.with_alpha(0.55));
        }
    }
}

fn ruler(v: &AudioEditorView, c: &mut dyn Canvas) {
    let l = v.layout();
    let st = &v.st;
    sp::rect(c, l.ruler, st.header);
    c.push_clip(&l.ruler.into());
    sp::ruler_labels(c, st, &v.axis(), l.ruler, &v.bars);
    // Detected transients: short ticks under the ruler, where the clip plays them.
    if let Some(info) = &v.info
        && !v.transients.is_empty()
    {
        let offset = info.content.source_offset as f64;
        for f in &v.transients {
            let Some(tick) = v.tick_of_source(*f as f64 - offset) else {
                continue;
            };
            let x = v.x_of(tick).round() + 0.5;
            if x >= l.ruler.x0 && x <= l.ruler.x1 {
                sp::line(
                    c,
                    x,
                    l.ruler.y1 - 6.0,
                    x,
                    l.ruler.y1,
                    v.accent.with_alpha(0.8),
                );
            }
        }
    }
    c.pop_clip();
    sp::line(
        c,
        l.ruler.x0,
        l.ruler.y1 - 0.5,
        l.ruler.x1,
        l.ruler.y1 - 0.5,
        st.separator,
    );
}

fn waveform(v: &AudioEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout();
    let w = l.wave;
    c.push_clip(&w.into());
    sp::rect(c, w, st.lane_auto);
    sp::grid(c, st, &v.axis(), w, &v.bars);
    let asset = v
        .info
        .as_ref()
        .map(|i| i.content.asset.as_str())
        .unwrap_or_default();
    let missing = v.meta.is_none() || v.missing.contains(asset);
    if missing {
        sp::label(
            c,
            "Missing asset: the audio file is not available",
            w.center().x,
            w.center().y,
            10.0,
            st.record,
            Align::Center,
        );
    } else if let Some(peaks) = v.peaks.get(asset) {
        segments(v, c, peaks, asset);
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
    markers(v, c);
    if v.mode == AudioMode::Slice
        && let Some(info) = &v.info
        && info.content.reverse
    {
        sp::label(
            c,
            "A reversed clip cannot be warped, so it cannot be sliced",
            w.center().x,
            w.y0 + 14.0,
            10.0,
            t.text,
            Align::Center,
        );
    }
    c.pop_clip();
}

/// The waveform: one linear piece per pair of neighbouring knots of the time curve, so a
/// stretched span is drawn wider or narrower.
fn segments(
    v: &AudioEditorView,
    c: &mut dyn Canvas,
    peaks: &atomik_audio_file::PeakPyramid,
    asset: &str,
) {
    let (Some(info), Some(meta)) = (&v.info, &v.meta) else {
        return;
    };
    let w = v.layout().wave;
    let offset = info.content.source_offset as f64;
    let samples = v.samples.get(asset).map(|s| &s[..]);
    let wave = v.st.audio_wave(v.accent, info.muted);
    for pair in v.curve.windows(2) {
        let ((t0, s0), (t1, s1)) = (pair[0], pair[1]);
        let (xa, xb) = (v.x_of(t0), v.x_of(t1));
        let (vx0, vx1) = (xa.max(w.x0).floor(), xb.min(w.x1).ceil());
        if xb - xa < 0.5 || vx1 <= vx0 {
            continue;
        }
        // Source frames per pixel here, negative for a reversed clip.
        let slope = (s1 - s0) / (xb - xa);
        if slope == 0.0 {
            continue;
        }
        let detail = WaveDetail::pick(peaks, samples, slope.abs());
        let bands = audio::channel_bands(w.y0 + 4.0, w.y1 - 4.0, detail.channels());
        let columns = ((vx1 - vx0) as usize).min(MAX_COLUMNS);
        let source = |x: f64| offset + s0 + (x - xa) * slope;
        let frame_x = |f: f64| xa + (f - offset - s0) / slope;
        c.push_clip(&Rect::new(vx0, w.y0, vx1, w.y1).into());
        for (ch, (cy, half)) in bands.into_iter().enumerate() {
            let channel = WaveChannel {
                detail,
                ch,
                cy,
                half,
                gain: 1.0,
                color: wave,
                x_start: vx0,
                columns,
                line_x: (vx0, vx1),
                limit: meta.frames,
                frames_per_pixel: slope.abs(),
            };
            paint_channel(c, &channel, &source, &frame_x);
        }
        c.pop_clip();
    }
}

fn markers(v: &AudioEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout();
    let w = l.wave;
    let n = v.framed.len();
    let active = match &v.drag {
        Drag::Marker(d) => Some(d.index),
        Drag::None | Drag::Audition => None,
    };
    for (i, m) in v.framed.iter().enumerate() {
        let x = v.x_of(m.tick as f64).round() + 0.5;
        if x < w.x0 - 1.0 || x > w.x1 + 1.0 {
            continue;
        }
        let end = i == 0 || i + 1 == n;
        let lit = active == Some(i) || v.hover == Hit::Marker(i);
        let sel = v.selected == Some(i);
        let col = if sel { v.accent } else { t.text };
        let width = if lit { 2.5 } else { 1.5 };
        let alpha = if end { 0.35 } else { 1.0 };
        let alpha = if v.mode == AudioMode::Slice {
            alpha
        } else {
            alpha * 0.55
        };
        c.stroke_with(
            Line::new(pt(x, l.ruler.y0 + 4.0), pt(x, w.y1)),
            width,
            col.with_alpha(alpha),
        );
        if !end {
            // A handle on the ruler.
            let mut tri = BezPath::new();
            tri.move_to(pt(x - 4.0, l.ruler.y1 - 7.0));
            tri.line_to(pt(x + 4.0, l.ruler.y1 - 7.0));
            tri.line_to(pt(x, l.ruler.y1 - 1.0));
            tri.close_path();
            c.fill_with(tri, col.with_alpha(alpha));
        }
    }
    // The position of the marker being dragged.
    if let (Some(i), Some(info)) = (active, &v.info)
        && let Some(m) = v.framed.get(i)
    {
        let x = v.x_of(m.tick as f64);
        let label = bar_beat_label(info.start + m.tick, &v.bars);
        sp::label(c, &label, x + 6.0, w.y1 - 6.0, 9.0, t.text, Align::Start);
    }
}

fn footer(v: &AudioEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout();
    sp::rect(c, l.footer, st.header);
    sp::line(
        c,
        l.footer.x0,
        l.footer.y0 + 0.5,
        l.footer.x1,
        l.footer.y0 + 0.5,
        st.separator,
    );
    let y = l.footer.center().y + 3.0;
    let (text, color) = if v.status.is_empty() {
        (v.hint(), t.text_muted)
    } else {
        (v.status.clone(), t.text)
    };
    let text = fit_label(&text, l.footer.width() - 220.0, 9.0);
    sp::label(c, &text, l.footer.x0 + 8.0, y, 9.0, color, Align::Start);
    if let Some(info) = &v.info {
        let markers = v.framed.len().saturating_sub(2);
        let info_text = format!(
            "{markers} {}  ·  {}",
            if markers == 1 { "marker" } else { "markers" },
            bar_beat_label(info.start + info.length, &v.bars)
        );
        sp::label(
            c,
            &info_text,
            l.footer.x1 - 8.0,
            y,
            9.0,
            t.text_muted,
            Align::End,
        );
    }
}
