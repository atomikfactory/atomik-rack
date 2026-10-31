//! Painting of the container editor (see the parent module for the layout).

use super::{ContainerEditorView, ContainerTab, Drag, Hit};
use crate::canvas::{Align, Canvas, CanvasExt, pt};
use crate::container_edit::{Bound, Edge};
use crate::container_panel::split_color;
use crate::paint::{fit_label, knob};
use crate::seq_paint as sp;
use crate::theme::mix;
use crate::widget::format_value;
use crate::zone_edit::note_name;
use atomik_model::MACRO_COUNT;
use vello::kurbo::{Affine, Line, Rect, RoundedRect};

pub(super) fn paint(v: &ContainerEditorView, c: &mut dyn Canvas) {
    let b = v.bounds;
    c.push_transform(Affine::scale(v.scale));
    c.push_clip(&b.into());
    sp::rect(c, b, v.st.background);
    if v.container.is_some() && v.present {
        match v.tab {
            ContainerTab::Splits => splits(v, c),
            ContainerTab::Macros => macros(v, c),
        }
    } else {
        sp::label(
            c,
            "No container open",
            b.center().x,
            b.center().y,
            11.0,
            v.st.base.text_muted,
            Align::Center,
        );
    }
    chrome(v, c);
    if v.focused {
        sp::line(c, b.x0, b.y0 + 0.5, b.x1, b.y0 + 0.5, v.st.base.focus);
    }
    c.pop_clip();
    c.pop_transform();
}

/// The top bar, the tabs and the close button (drawn last so they stay over the content).
fn chrome(v: &ContainerEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let b = v.bounds;
    let bar = Rect::new(b.x0, b.y0, b.x1, b.y0 + super::TOP_BAR_HEIGHT);
    sp::rect(c, bar, st.header);
    sp::line(c, bar.x0, bar.y1 - 0.5, bar.x1, bar.y1 - 0.5, st.separator);
    let title = if v.container.is_some() && v.present {
        format!(
            "{}  ·  {} devices  ·  {} splits",
            v.name,
            v.inner.devices.len(),
            v.inner.splits.len()
        )
    } else {
        String::new()
    };
    sp::label(
        c,
        "CONTAINER EDITOR",
        b.x0 + super::PAD,
        b.y0 + 18.0,
        9.0,
        t.text_muted,
        Align::Start,
    );
    sp::label(
        c,
        &fit_label(&title, b.width() - 260.0, 10.0),
        b.x0 + 150.0,
        b.y0 + 18.0,
        10.0,
        t.text,
        Align::Start,
    );
    sp::button(c, st, v.close_rect(), "x", None, v.hover == Hit::Close);
    let tabs = Rect::new(b.x0, bar.y1, b.x1, bar.y1 + super::TAB_HEIGHT);
    sp::rect(c, tabs, mix(st.header, st.background, 0.5));
    sp::line(
        c,
        tabs.x0,
        tabs.y1 - 0.5,
        tabs.x1,
        tabs.y1 - 0.5,
        st.separator,
    );
    for tab in ContainerTab::ALL {
        let lit = (v.tab == tab).then_some(v.accent);
        sp::button(
            c,
            st,
            v.tab_rect(tab),
            tab.label(),
            lit,
            v.hover == Hit::Tab(tab),
        );
    }
}

// ------------------------------------------------------------------ SPLITS

fn splits(v: &ContainerEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let m = v.map_rect();
    sp::lcd(c, st, m, None);
    // Velocity grid and labels.
    for vel in [1u8, 32, 64, 96, 127] {
        let y = m.y1 - f64::from(vel - 1).max(0.0) * v.vel_unit();
        let y = y.clamp(m.y0 + 0.5, m.y1 - 0.5);
        sp::line(
            c,
            m.x0,
            y,
            m.x1,
            y,
            v.st.base
                .ink
                .with_alpha(if vel == 64 { 0.10 } else { 0.05 }),
        );
        sp::label(
            c,
            &vel.to_string(),
            m.x0 - 6.0,
            y + 3.0,
            8.0,
            t.text_muted,
            Align::End,
        );
    }
    // Octave lines.
    for oct in 0..11 {
        let x = v.key_x(oct * 12).round() + 0.5;
        sp::line(
            c,
            x,
            m.y0,
            x,
            m.y1,
            v.st.base.ink.with_alpha(if oct == 5 { 0.16 } else { 0.07 }),
        );
    }
    // Bars.
    for i in 0..v.inner.splits.len() {
        let Some(r) = v.bar_rect(i) else { continue };
        let selected = v.selected_split == Some(i);
        let col = split_color(i);
        c.fill_with(
            RoundedRect::from_rect(r, 2.0),
            col.with_alpha(if selected { 0.78 } else { 0.5 }),
        );
        c.stroke_with(
            RoundedRect::from_rect(r, 2.0),
            if selected { 2.0 } else { 1.0 },
            if selected { t.text } else { col },
        );
        let name = v
            .devices
            .iter()
            .find(|d| d.key == v.inner.splits[i].target)
            .map_or_else(|| v.inner.splits[i].target.0.clone(), |d| d.name.clone());
        if r.width() > 40.0 && r.height() > 14.0 {
            sp::label(
                c,
                &fit_label(&name, r.width() - 12.0, 9.0),
                r.x0 + 6.0,
                r.y0 + 12.0,
                9.0,
                t.plate,
                Align::Start,
            );
        }
        if let Hit::Bar(h, edge) = v.hover
            && h == i
        {
            let e = match edge {
                Edge::Left => Some((r.x0, r.y0, r.x0, r.y1)),
                Edge::Right => Some((r.x1, r.y0, r.x1, r.y1)),
                Edge::Top => Some((r.x0, r.y0, r.x1, r.y0)),
                Edge::Bottom => Some((r.x0, r.y1, r.x1, r.y1)),
                Edge::Body => None,
            };
            if let Some((x0, y0, x1, y1)) = e {
                c.stroke_with(Line::new(pt(x0, y0), pt(x1, y1)), 3.0, t.highlight);
            }
        }
    }
    if v.inner.splits.is_empty() {
        sp::label(
            c,
            "No splits: no notes reach the inside",
            m.center().x,
            m.center().y + 3.0,
            10.0,
            t.text_muted,
            Align::Center,
        );
    }
    keyboard(v, c);
    list(v, c);
}

fn keyboard(v: &ContainerEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let k = v.keys_rect();
    let held = match v.drag {
        Drag::Audition(key) => Some(key),
        _ => None,
    };
    for key in 0..128u8 {
        let x0 = v.key_x(i32::from(key));
        let x1 = v.key_x(i32::from(key) + 1);
        let black = matches!(key % 12, 1 | 3 | 6 | 8 | 10);
        let r = Rect::new(
            x0,
            k.y0,
            x1,
            if black {
                k.y0 + k.height() * 0.62
            } else {
                k.y1
            },
        );
        let lit = held == Some(key);
        let col = match (lit, black) {
            (true, _) => v.accent,
            (false, true) => st.key_black,
            (false, false) => st.key_white,
        };
        sp::rect(c, r, col);
        if !black {
            sp::line(c, x1 - 0.5, k.y0, x1 - 0.5, k.y1, t.bevel_dark);
        }
    }
    for oct in 0..11 {
        let key = oct * 12;
        sp::label(
            c,
            &note_name(key as u8),
            v.key_x(key) + 1.0,
            k.y1 + 9.0,
            7.5,
            t.text_muted,
            Align::Start,
        );
    }
}

fn list(v: &ContainerEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let top = v.list_top();
    sp::label(
        c,
        "TARGET",
        v.bounds.x0 + super::PAD + 22.0,
        top + 10.0,
        8.0,
        t.text_muted,
        Align::Start,
    );
    sp::label(
        c,
        "TRANSPOSE",
        v.dec_rect(0).x0 + 50.0,
        top + 10.0,
        8.0,
        t.text_muted,
        Align::Center,
    );
    let can_add = !v.eligible.is_empty() && v.inner.splits.len() < atomik_model::MAX_SPLITS;
    sp::button(
        c,
        st,
        v.add_rect(),
        "ADD",
        None,
        can_add && v.hover == Hit::Add,
    );
    if !can_add {
        sp::rect(c, v.add_rect(), st.header.with_alpha(0.55));
    }
    for (i, s) in v.inner.splits.iter().enumerate() {
        let r = v.row_rect(i);
        if v.selected_split == Some(i) {
            sp::rect(c, r, mix(st.header, t.highlight, 0.12));
        } else if i % 2 == 1 {
            sp::rect(c, r, st.lane_alt);
        }
        c.fill_with(
            vello::kurbo::Circle::new(pt(r.x0 + 10.0, r.center().y), 5.0),
            split_color(i),
        );
        let tr = v.target_rect(i);
        sp::lcd(c, st, tr, (v.hover == Hit::Target(i)).then_some(v.accent));
        let name = v
            .devices
            .iter()
            .find(|d| d.key == s.target)
            .map_or_else(|| s.target.0.clone(), |d| d.name.clone());
        sp::label(
            c,
            &fit_label(&name, tr.width() - 26.0, 10.0),
            tr.x0 + 7.0,
            tr.y0 + 13.0,
            10.0,
            t.text,
            Align::Start,
        );
        sp::label(
            c,
            "v",
            tr.x1 - 9.0,
            tr.y0 + 12.5,
            9.0,
            t.text_muted,
            Align::Center,
        );
        let keys = format!("{} - {}", note_name(s.key_lo), note_name(s.key_hi));
        sp::label(
            c,
            &keys,
            r.x0 + 204.0,
            r.y0 + 17.0,
            10.0,
            t.text,
            Align::Start,
        );
        let vel = format!("vel {}-{}", s.vel_lo, s.vel_hi);
        sp::label(
            c,
            &vel,
            r.x0 + 340.0,
            r.y0 + 17.0,
            10.0,
            t.text_muted,
            Align::Start,
        );
        sp::button(
            c,
            st,
            v.dec_rect(i),
            "-",
            None,
            v.hover == Hit::TransposeDec(i),
        );
        let tv = v.transpose_rect(i);
        sp::lcd(c, st, tv, None);
        sp::label(
            c,
            &format!("{:+}", s.transpose),
            tv.center().x,
            tv.center().y + 3.5,
            10.0,
            t.text,
            Align::Center,
        );
        sp::button(
            c,
            st,
            v.inc_rect(i),
            "+",
            None,
            v.hover == Hit::TransposeInc(i),
        );
        sp::button(
            c,
            st,
            v.delete_rect(i),
            "x",
            None,
            v.hover == Hit::Delete(i),
        );
    }
    if v.inner.splits.is_empty() {
        let r = v.row_rect(0);
        sp::label(
            c,
            "ADD gives an instrument a zone of the keyboard. A device takes one split.",
            r.x0 + 22.0,
            r.y0 + 17.0,
            10.0,
            t.text_muted,
            Align::Start,
        );
    }
}

// ------------------------------------------------------------------ MACROS

fn macros(v: &ContainerEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    for i in 0..MACRO_COUNT {
        let r = v.macro_row_rect(i);
        let selected = v.selected_macro == i;
        if selected {
            sp::rect(c, r, mix(st.header, t.highlight, 0.12));
        }
        let chip = Rect::new(r.x0 + 2.0, r.y0 + 6.0, r.x0 + 22.0, r.y1 - 6.0);
        sp::button(
            c,
            st,
            chip,
            &(i + 1).to_string(),
            selected.then_some(v.accent),
            v.hover == Hit::MacroRow(i),
        );
        let nr = v.macro_name_rect(i);
        let editing = selected.then_some(v.rename.as_deref()).flatten();
        sp::lcd(c, st, nr, editing.map(|_| v.accent));
        let stored = &v.inner.macros[i].name;
        let (text, col) = match editing {
            Some(e) => (format!("{e}|"), t.text),
            None if stored.is_empty() => (format!("Macro {}", i + 1), t.text_muted),
            None => (stored.clone(), t.text),
        };
        sp::label(
            c,
            &fit_label(&text, nr.width() - 12.0, 10.0),
            nr.x0 + 7.0,
            nr.y0 + 13.5,
            10.0,
            col,
            Align::Start,
        );
        // The value knob.
        let kr = v.macro_knob_rect(i);
        let value =
            f64::from(v.macro_params[i].map_or(0.0, |p| p.to_normalized(v.macro_values[i])));
        knob(c, t, kr.center(), value, false, v.accent);
        sp::label(
            c,
            &format!("{:.0} %", value * 100.0),
            kr.x1 + 6.0,
            kr.center().y + 3.5,
            9.0,
            t.text_muted,
            Align::Start,
        );
        let n = v.inner.macros[i].targets.len();
        sp::label(
            c,
            &format!("{n}"),
            r.x1 - 10.0,
            r.y0 + 17.0,
            10.0,
            if n > 0 { v.accent } else { t.text_muted },
            Align::End,
        );
    }
    targets(v, c);
}

fn targets(v: &ContainerEditorView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let m = v.selected_macro;
    let body = v.body();
    let x0 = v.targets_x();
    let name = v.inner.macros[m].display_name(m);
    sp::label(
        c,
        &format!("MACRO {}  ·  {}", m + 1, name),
        x0,
        body.y0 + 22.0,
        10.0,
        t.text,
        Align::Start,
    );
    let hdr = v.target_row_rect(0);
    for (text, x) in [
        ("MIN", v.field_rect(0, Bound::Min).center().x),
        ("MAX", v.field_rect(0, Bound::Max).center().x),
    ] {
        sp::label(c, text, x, body.y0 + 34.0, 8.0, t.text_muted, Align::Center);
    }
    let _ = hdr;
    let view = v.targets_view();
    c.push_clip(&view.into());
    let list = &v.targets[m];
    for (j, info) in list.iter().enumerate() {
        let r = v.target_row_rect(j);
        if r.y1 < view.y0 || r.y0 > view.y1 {
            continue;
        }
        if j % 2 == 1 {
            sp::rect(c, r, st.lane_alt);
        }
        let label = format!("{}  ·  {}", info.device_name, info.param.name);
        sp::label(
            c,
            &fit_label(&label, r.width() - 230.0, 10.0),
            r.x0 + 8.0,
            r.y0 + 17.0,
            10.0,
            t.text,
            Align::Start,
        );
        for (bound, value) in [(Bound::Min, info.target.min), (Bound::Max, info.target.max)] {
            let fr = v.field_rect(j, bound);
            let active = matches!(&v.drag, Drag::Bound(d) if d.row == j && d.bound == bound)
                || v.hover == Hit::Field(j, bound);
            sp::lcd(c, st, fr, active.then_some(v.accent));
            sp::label(
                c,
                &fit_label(&format_value(&info.param, value), fr.width() - 8.0, 9.5),
                fr.center().x,
                fr.center().y + 3.5,
                9.5,
                t.text,
                Align::Center,
            );
        }
        sp::button(
            c,
            st,
            v.target_delete_rect(j),
            "x",
            None,
            v.hover == Hit::TargetDelete(j),
        );
    }
    if list.is_empty() {
        sp::label(
            c,
            "No targets. Right-click a control inside the container and",
            x0,
            body.y0 + 60.0,
            10.0,
            t.text_muted,
            Align::Start,
        );
        sp::label(
            c,
            &format!("choose \"Map to macro {}\".", m + 1),
            x0,
            body.y0 + 76.0,
            10.0,
            t.text_muted,
            Align::Start,
        );
    }
    c.pop_clip();
}
