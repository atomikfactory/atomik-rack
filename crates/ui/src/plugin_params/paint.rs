//! Painting of the plugin parameter pane (see the parent module for the layout).

use super::{Hit, Item, PluginParamsView, Row, Widget};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::paint::fit_label;
use crate::seq_paint as sp;
use crate::theme::mix;
use vello::kurbo::{Affine, Circle, Rect, RoundedRect};
use vello::peniko::Color;

/// The plain value as a number: integers for stepped params, 3 significant digits otherwise.
pub(super) fn format_plain(row: &Row, v: f32) -> String {
    if row.widget != Widget::Slider {
        return format!("{}", v.round() as i64);
    }
    let a = v.abs();
    if a >= 1000.0 {
        format!("{v:.0}")
    } else if a >= 100.0 {
        format!("{v:.1}")
    } else if a >= 10.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

pub(super) fn paint(v: &PluginParamsView, c: &mut dyn Canvas) {
    let b = v.bounds;
    c.push_transform(Affine::scale(v.scale));
    c.push_clip(&b.into());
    sp::rect(c, b, v.st.background);
    if v.device.is_some() && v.present {
        rows(v, c);
    } else {
        sp::label(
            c,
            "No plugin open",
            b.center().x,
            b.center().y,
            11.0,
            v.st.base.text_muted,
            Align::Center,
        );
    }
    chrome(v, c);
    menu(v, c);
    // The help line (what undo covers), replaced by the message of a refused gesture.
    let footer = Rect::new(b.x0, b.y1 - super::FOOTER_HEIGHT, b.x1, b.y1);
    sp::rect(c, footer, v.st.header);
    sp::line(
        c,
        footer.x0,
        footer.y0 + 0.5,
        footer.x1,
        footer.y0 + 0.5,
        v.st.separator,
    );
    sp::label(
        c,
        &fit_label(crate::plugin_data::PARAMS_HELP, footer.width() - 28.0, 8.5),
        footer.x0 + super::PAD,
        footer.y0 + 14.5,
        8.5,
        v.st.base.text_muted,
        Align::Start,
    );
    if let Some(t) = &v.toast {
        let r = Rect::new(b.x0 + 14.0, b.y1 - 24.0, b.x1 - 14.0, b.y1 - 4.0);
        c.fill_with(RoundedRect::from_rect(r, 4.0), v.st.base.tooltip_bg);
        sp::label(
            c,
            &fit_label(t, r.width() - 16.0, 9.5),
            r.x0 + 8.0,
            r.y0 + 13.5,
            9.5,
            v.st.base.text,
            Align::Start,
        );
    }
    if v.focused {
        sp::line(c, b.x0, b.y0 + 0.5, b.x1, b.y0 + 0.5, v.st.base.focus);
    }
    c.pop_clip();
    c.pop_transform();
}

/// The top bar, the search field and the close button (drawn over the rows).
fn chrome(v: &PluginParamsView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let b = v.bounds;
    let bar = Rect::new(b.x0, b.y0, b.x1, b.y0 + super::TOP_BAR_HEIGHT);
    sp::rect(c, bar, st.header);
    sp::line(c, bar.x0, bar.y1 - 0.5, bar.x1, bar.y1 - 0.5, st.separator);
    sp::label(
        c,
        "PLUGIN PARAMS",
        b.x0 + super::PAD,
        b.y0 + 18.0,
        9.0,
        t.text_muted,
        Align::Start,
    );
    if v.present {
        let title = format!("{}  ·  {}", v.name, v.vendor);
        sp::label(
            c,
            &fit_label(&title, b.width() - 320.0, 10.0),
            b.x0 + 130.0,
            b.y0 + 18.0,
            10.0,
            t.text,
            Align::Start,
        );
        sp::label(
            c,
            v.badge,
            b.x1 - 44.0,
            b.y0 + 17.5,
            7.5,
            v.accent,
            Align::End,
        );
    }
    sp::button(
        c,
        st,
        v.close_rect(),
        "x",
        None,
        matches!(v.hover, Hit::Close),
    );
    // The search row.
    let row = Rect::new(b.x0, bar.y1, b.x1, bar.y1 + super::SEARCH_HEIGHT);
    sp::rect(c, row, st.header_lane);
    let field = v.search_rect();
    sp::lcd(
        c,
        st,
        field,
        if v.query.is_empty() {
            None
        } else {
            Some(v.accent)
        },
    );
    let (text, color) = if v.query.is_empty() {
        ("Search name, module or key".to_string(), t.text_muted)
    } else {
        (format!("{}|", v.query), t.text)
    };
    sp::label(
        c,
        &fit_label(&text, field.width() - 14.0, 10.0),
        field.x0 + 8.0,
        field.y0 + 15.0,
        10.0,
        color,
        Align::Start,
    );
    sp::label(
        c,
        &format!("{} of {} params", v.shown_count(), v.total_count()),
        field.x1 + 14.0,
        field.y0 + 15.0,
        9.0,
        t.text_muted,
        Align::Start,
    );
    sp::line(c, row.x0, row.y1 - 0.5, row.x1, row.y1 - 0.5, st.separator);
}

fn rows(v: &PluginParamsView, c: &mut dyn Canvas) {
    let st = &v.st;
    let t = &st.base;
    let list = v.list_rect();
    c.push_clip(&list.into());
    if v.items.is_empty() {
        let msg = if v.query.is_empty() {
            "This plugin has no parameters"
        } else {
            "No parameters match the search"
        };
        sp::label(
            c,
            msg,
            list.center().x,
            list.y0 + 40.0,
            11.0,
            t.text_muted,
            Align::Center,
        );
    }
    for (i, y) in v.visible_items() {
        let bx = v.bounds;
        match &v.items[i] {
            Item::Header(h) => {
                let r = Rect::new(bx.x0, y, bx.x1, y + super::HEADER_HEIGHT);
                sp::rect(c, r, st.header_track);
                sp::label(
                    c,
                    &h.to_uppercase(),
                    bx.x0 + super::PAD,
                    y + 15.0,
                    8.5,
                    t.text_muted,
                    Align::Start,
                );
                sp::line(c, r.x0, r.y1 - 0.5, r.x1, r.y1 - 0.5, st.separator);
            }
            Item::Param(ri) => {
                let row = &v.rows[*ri];
                let r = Rect::new(bx.x0, y, bx.x1, y + super::ROW_HEIGHT);
                param_row(v, c, row, r, *ri);
            }
        }
    }
    // A scroll thumb.
    if v.content_h > list.height() {
        let track = Rect::new(list.x1 - 4.0, list.y0, list.x1 - 1.0, list.y1);
        let frac = list.height() / v.content_h;
        let h = (track.height() * frac).max(18.0);
        let top = track.y0 + (track.height() - h) * (v.scroll / v.max_scroll().max(1.0));
        c.fill_with(
            RoundedRect::from_rect(Rect::new(track.x0, top, track.x1, top + h), 1.5),
            t.text_muted.with_alpha(0.4),
        );
    }
    c.pop_clip();
}

fn param_row(v: &PluginParamsView, c: &mut dyn Canvas, row: &Row, r: Rect, ri: usize) {
    let st = &v.st;
    let t = &st.base;
    let id = row.id;
    let hovered = matches!(
        v.hover,
        Hit::Row(i) | Hit::Slider(i) | Hit::StepDec(i) | Hit::StepInc(i) | Hit::Toggle(i)
            | Hit::Pin(i) | Hit::Auto(i) | Hit::Map(i) if i == id
    );
    sp::rect(
        c,
        r,
        if hovered {
            mix(st.lane, t.text, 0.05)
        } else if ri.is_multiple_of(2) {
            st.lane
        } else {
            st.lane_alt
        },
    );
    let value = v.value_of(row);
    sp::label(
        c,
        &fit_label(&row.name, super::NAME_W, 10.0),
        r.x0 + super::PAD,
        r.y0 + 17.0,
        10.0,
        t.text,
        Align::Start,
    );
    let (slider, text, pin, auto, map) = v.parts(r);
    let driver = v.drivers.get(&id).map(|(_, m)| *m);
    let accent = v.accent;
    match row.widget {
        Widget::Slider => {
            let track = Rect::new(
                slider.x0,
                slider.center().y - 2.0,
                slider.x1,
                slider.center().y + 2.0,
            );
            c.fill_with(
                RoundedRect::from_rect(track, 2.0),
                mix(t.knob_track, t.text_muted, 0.45),
            );
            let n = super::norm_of(row, value);
            let fill = Rect::new(track.x0, track.y0, track.x0 + track.width() * n, track.y1);
            c.fill_with(RoundedRect::from_rect(fill, 2.0), accent);
            let thumb = pt(track.x0 + track.width() * n, track.center().y);
            c.fill_with(Circle::new(thumb, 5.5), t.knob_pointer);
            c.stroke_with(Circle::new(thumb, 5.5), 1.0, t.bevel_dark);
        }
        Widget::Stepper => {
            let dec = Rect::new(slider.x0, slider.y0, slider.x0 + super::STEP_W, slider.y1);
            let inc = Rect::new(
                slider.x0 + super::STEP_W + 4.0,
                slider.y0,
                slider.x0 + 2.0 * super::STEP_W + 4.0,
                slider.y1,
            );
            sp::button(
                c,
                st,
                dec,
                "-",
                None,
                matches!(v.hover, Hit::StepDec(i) if i == id),
            );
            sp::button(
                c,
                st,
                inc,
                "+",
                None,
                matches!(v.hover, Hit::StepInc(i) if i == id),
            );
        }
        Widget::Toggle => {
            let b = Rect::new(
                slider.x0,
                slider.y0,
                slider.x0 + 2.0 * super::STEP_W + 4.0,
                slider.y1,
            );
            let on = value >= (row.min + row.max) / 2.0;
            sp::button(
                c,
                st,
                b,
                if on { "ON" } else { "OFF" },
                on.then_some(accent),
                matches!(v.hover, Hit::Toggle(i) if i == id),
            );
        }
    }
    let shown = match v.texts.get(&id) {
        Some((bits, s)) if *bits == value.to_bits() => s.clone(),
        _ => format_plain(row, value),
    };
    sp::label(
        c,
        &fit_label(&shown, text.width(), 9.5),
        text.x1,
        r.y0 + 17.0,
        9.5,
        t.text,
        Align::End,
    );
    let pinned = v.is_pinned(id);
    sp::button(
        c,
        st,
        pin,
        "PIN",
        pinned.then_some(accent),
        matches!(v.hover, Hit::Pin(i) if i == id),
    );
    sp::button(
        c,
        st,
        auto,
        "AUTO",
        None,
        matches!(v.hover, Hit::Auto(i) if i == id),
    );
    if let Some(map) = map {
        let label = driver.map_or_else(|| "MAP".to_string(), |m| format!("M{}", u16::from(m) + 1));
        sp::button(
            c,
            st,
            map,
            &label,
            driver.map(|_| accent),
            matches!(v.hover, Hit::Map(i) if i == id),
        );
    }
    if driver.is_some() {
        // Locked: a veil over the widget and the value.
        let veil = Rect::new(slider.x0 - 2.0, r.y0 + 1.0, text.x1 + 2.0, r.y1 - 1.0);
        c.fill_with(veil, st.background.with_alpha(0.45));
    }
    sp::line(
        c,
        r.x0,
        r.y1 - 0.5,
        r.x1,
        r.y1 - 0.5,
        Color::from_rgba8(255, 255, 255, 10),
    );
}

fn menu(v: &PluginParamsView, c: &mut dyn Canvas) {
    let Some(m) = &v.menu else { return };
    let t = &v.st.base;
    c.fill_with(RoundedRect::from_rect(m.rect, 4.0), t.tooltip_bg);
    c.stroke_with(
        RoundedRect::from_rect(m.rect, 4.0),
        1.0,
        t.ink.with_alpha(0.14),
    );
    for (k, (_, label)) in m.entries.iter().enumerate() {
        let r = PluginParamsView::menu_row(m.rect, k);
        if m.hover == Some(k) {
            c.fill_with(RoundedRect::from_rect(r.inset(-1.0), 3.0), t.browser_row);
        }
        c.text(
            &TextRun::new(
                fit_label(label, r.width() - 14.0, 9.5),
                pt(r.x0 + 8.0, r.y1 - 5.5),
                9.5,
                t.text,
            )
            .align(Align::Start),
        );
    }
}
