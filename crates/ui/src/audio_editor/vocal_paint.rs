//! Painting of the audio clip editor's Vocal mode: the toolbar, the piano keys, the pitch grid,
//! the notes and the footer (see the parent module).

use super::vocal::{Analysis, edit_of};
use super::{AudioEditorView, Drag, Hit};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::seq_paint as sp;
use vello::kurbo::{BezPath, Rect, RoundedRect};

/// The toolbar of Vocal mode (after the mode buttons).
pub(super) fn toolbar(v: &AudioEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let l = v.layout();
    let ty = l.bar.center().y + 2.5;
    let ready = v.voicing();
    let chosen = ready && !v.vocal.selected.is_empty();
    sp::button(
        c,
        st,
        l.correct,
        "CORRECT",
        None,
        chosen && v.hover == Hit::Correct,
    );
    sp::button(
        c,
        st,
        l.flatten,
        "FLATTEN",
        None,
        chosen && v.hover == Hit::Flatten,
    );
    sp::lcd(
        c,
        st,
        l.flatten_value,
        (v.hover == Hit::FlattenValue || matches!(v.drag, Drag::Flatten(..)))
            .then_some(st.base.focus),
    );
    c.text(
        &TextRun::new(
            format!("{}%", v.vocal.flatten),
            pt(l.flatten_value.center().x, l.flatten_value.center().y + 3.5),
            10.0,
            st.base.text,
        )
        .align(Align::Center),
    );
    let reset_on = ready && !v.edits().is_empty();
    sp::button(
        c,
        st,
        l.reset,
        "RESET",
        None,
        reset_on && v.hover == Hit::Reset,
    );
    sp::button(
        c,
        st,
        l.scale,
        "SCALE",
        v.vocal.scale.then_some(v.accent),
        v.hover == Hit::ScaleToggle,
    );
    c.text(
        &TextRun::new(
            "SENS",
            pt(l.sens_down.x0 - 32.0, ty),
            7.0,
            st.base.text_muted,
        )
        .weight(700)
        .spacing(0.8),
    );
    sp::button(
        c,
        st,
        l.sens_down,
        "-",
        None,
        ready && v.hover == Hit::SensDown,
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
    sp::button(c, st, l.sens_up, "+", None, ready && v.hover == Hit::SensUp);
    for (r, on) in [
        (l.correct, chosen),
        (l.flatten, chosen),
        (l.reset, reset_on),
        (l.sens_down, ready),
        (l.sens_up, ready),
    ] {
        if !on {
            sp::rect(c, r, st.base.background.with_alpha(0.55));
        }
    }
}

/// The rows of the pitch grid (under the dimmed waveform) and the piano keys beside them.
pub(super) fn grid(v: &AudioEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let l = v.layout();
    let (lo, hi) = v.vocal.range;
    let row = v.vocal_row_height();
    let first = (lo + 0.5).ceil() as i32;
    let last = (hi - 0.5).floor() as i32;
    for key in first..=last.min(127) {
        let k = key.clamp(0, 127) as u8;
        let y0 = v.y_of_key(key as f32 + 0.5);
        let r = Rect::new(l.wave.x0, y0, l.wave.x1, y0 + row);
        let black = matches!(k % 12, 1 | 3 | 6 | 8 | 10);
        if black {
            sp::rect(c, r, st.row_black.with_alpha(0.5));
        }
        if let Some(scale) = &v.vocal.key
            && scale.contains(k)
        {
            let a = if scale.is_root(k) { 0.16 } else { 0.08 };
            sp::rect(c, r, v.accent.with_alpha(a));
        }
        sp::line(c, r.x0, r.y1, r.x1, r.y1, st.grid_sub);
        // The key.
        let kr = Rect::new(l.keys.x0, y0, l.keys.x1 - 1.0, y0 + row);
        let face = if black { st.key_black } else { st.key_white };
        sp::rect(c, kr, face);
        sp::line(c, kr.x0, kr.y1, kr.x1, kr.y1, st.separator);
        if k.is_multiple_of(12) && row >= 7.0 {
            c.text(
                &TextRun::verbatim(
                    atomik_model::note_name(k),
                    pt(kr.x1 - 3.0, kr.y1 - (row - 7.0).max(0.0) / 2.0 - 1.0),
                    7.0,
                    st.key_black,
                )
                .align(Align::End),
            );
        }
    }
    sp::line(
        c,
        l.keys.x1 - 0.5,
        l.keys.y0,
        l.keys.x1 - 0.5,
        l.keys.y1,
        st.separator,
    );
}

/// The notes: a blob each, with the original pitch curve (dim) and the result (bright).
pub(super) fn notes(v: &AudioEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let w = v.layout().wave;
    let edits = v.edits();
    for (i, n) in v.vocal.notes.iter().enumerate() {
        let Some(r) = v.blob(i) else { continue };
        if r.x1 < w.x0 || r.x0 > w.x1 {
            continue;
        }
        let sel = v.vocal.selected.contains(&i);
        let edited = !edit_of(edits, n).is_neutral();
        let fill = if sel {
            v.accent.with_alpha(0.62)
        } else if edited {
            v.accent.with_alpha(0.42)
        } else {
            v.accent.with_alpha(0.28)
        };
        let rr = RoundedRect::from_rect(r, (r.height() / 2.0).min(6.0));
        c.fill_with(rr, fill);
        c.stroke_with(
            rr,
            if sel { 1.5 } else { 1.0 },
            if sel { st.selection } else { v.accent },
        );
        // The curves through the note.
        let offsets = v.vocal.offsets.get(i);
        let mut orig = BezPath::new();
        let mut result = BezPath::new();
        let mut open = false;
        for (k, p) in n.pitch.keys.iter().enumerate() {
            let at = n.pitch.frame_of(k);
            let (Some(p), true) = (p, at >= n.start && at < n.end) else {
                open = false;
                continue;
            };
            let Some(x) = v.x_of_frame(at) else {
                open = false;
                continue;
            };
            let off = offsets.and_then(|o| o.get(k)).copied().unwrap_or(0.0);
            let (yo, yr) = (v.y_of_key(*p), v.y_of_key(*p + off));
            if open {
                orig.line_to(pt(x, yo));
                result.line_to(pt(x, yr));
            } else {
                orig.move_to(pt(x, yo));
                result.move_to(pt(x, yr));
                open = true;
            }
        }
        c.stroke_with(orig, 1.0, t.text_muted.with_alpha(0.7));
        c.stroke_with(result, 1.4, t.text);
    }
    if let Drag::Band(b) = &v.drag {
        let r = Rect::from_points(b.start, b.now);
        sp::rect(c, r, st.selection.with_alpha(0.12));
        c.stroke_with(r, 1.0, st.selection);
    }
}

/// The footer of Vocal mode: the analysis' progress and CANCEL, or `None` to show the usual
/// text.
pub(super) fn footer(v: &AudioEditorView, c: &mut dyn Canvas) -> bool {
    let st = &v.st;
    let t = &st.base;
    let l = v.layout();
    let y = l.footer.center().y + 3.0;
    let Analysis::Running(p) = v.vocal.analysis else {
        return false;
    };
    sp::label(
        c,
        "Finding the notes…",
        l.footer.x0 + 8.0,
        y,
        9.0,
        t.text,
        Align::Start,
    );
    let bar = Rect::new(
        l.footer.x0 + 130.0,
        l.footer.y0 + 7.0,
        (l.cancel.x0 - 12.0).max(l.footer.x0 + 140.0),
        l.footer.y1 - 7.0,
    );
    sp::rect(c, bar, t.lcd);
    sp::rect(
        c,
        Rect::new(bar.x0, bar.y0, bar.x0 + bar.width() * f64::from(p), bar.y1),
        v.accent,
    );
    sp::button(c, st, l.cancel, "CANCEL", None, v.hover == Hit::Cancel);
    true
}

/// The footer's text in Vocal mode when no analysis runs: a refusal, the inspector line, the
/// empty-clip notice or the hint.
pub(super) fn footer_text(v: &AudioEditorView) -> (String, bool) {
    if !v.status.is_empty() {
        return (v.status.clone(), true);
    }
    if v.vocal.analysis == Analysis::Ready && v.vocal.notes.is_empty() && v.voicing() {
        return ("No pitched notes found in this clip".to_owned(), true);
    }
    if let Some(line) = v.vocal_inspector() {
        return (line, true);
    }
    (v.hint(), false)
}
