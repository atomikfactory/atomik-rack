//! Painting of the six effects' visuals: decay sketch, transfer curves,
//! EQ response with analyser bars and handles, LFO sketch, crossover display and loudness
//! readouts, with small level, gain-reduction and peak meters. Everything is drawn from the
//! device's params and the display meter values (`crate::fx::update_display`); a panel with no
//! meter feed draws the meters empty.

use crate::autolayout::FxPanel;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx::{
    self, FxKind, GR_SCALE_DB, LEVEL_FLOOR_DB, SYNC_NAMES, compressor_out_db, level_norm,
};
use crate::fx_layout::{EQ_HANDLE_R, eq_handle_pos, eq_x_of, eq_y_of};
use crate::paint::{PanelInfo, edit_button};
use crate::theme::{Theme, hsl, mix};
use crate::widget::current_value;
use vello::kurbo::{BezPath, Circle, Line, Point, Rect, RoundedRect};
use vello::peniko::Color;

/// The sample rate the EQ response is drawn at.
pub const EQ_DRAW_RATE: f64 = 48_000.0;

fn alpha(c: Color, a: f32) -> Color {
    c.with_alpha(a)
}

struct Params<'a> {
    info: &'a PanelInfo<'a>,
}

impl Params<'_> {
    fn get(&self, key: &str) -> f32 {
        let Some(desc) = self.info.desc else {
            return 0.0;
        };
        desc.params
            .iter()
            .find(|p| p.key == key)
            .map_or(0.0, |p| match self.info.device {
                Some(d) => current_value(d, p),
                None => p.default,
            })
    }
    fn on(&self, key: &str) -> bool {
        self.get(key) >= 0.5
    }
}

fn meter(info: &PanelInfo<'_>, i: usize, default: f32) -> f32 {
    info.fx_meters.get(i).copied().unwrap_or(default)
}

pub(crate) fn lcd<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, r: Rect) {
    c.fill_with(RoundedRect::from_rect(r, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(0.5), 3.5),
        1.0,
        alpha(Color::WHITE, 0.07),
    );
}

pub(crate) fn small<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    text: impl Into<String>,
    at: Point,
    align: Align,
    color: Option<Color>,
) {
    c.text(
        &TextRun::new(text, at, 7.5, color.unwrap_or(theme.text_muted))
            .align(align)
            .weight(600),
    );
}

pub(crate) fn meter_color(db: f32) -> Color {
    if db > -3.0 {
        hsl(6.0, 0.75, 0.55)
    } else if db > -12.0 {
        hsl(42.0, 0.8, 0.55)
    } else {
        hsl(140.0, 0.55, 0.5)
    }
}

/// A vertical bar from the bottom up (`down` false) or from the top down (`down` true) with
/// fill `norm` in 0..1.
pub(crate) fn bar<C: Canvas + ?Sized>(c: &mut C, r: Rect, norm: f64, color: Color, down: bool) {
    c.fill_with(
        Rect::from_origin_size(r.origin(), r.size()),
        alpha(Color::BLACK, 0.55),
    );
    let h = (r.height() * norm.clamp(0.0, 1.0)).round();
    if h > 0.0 {
        let fill = if down {
            Rect::new(r.x0, r.y0, r.x1, r.y0 + h)
        } else {
            Rect::new(r.x0, r.y1 - h, r.x1, r.y1)
        };
        c.fill_with(fill, color);
    }
    c.stroke_with(
        Rect::new(r.x0 - 0.5, r.y0 - 0.5, r.x1 + 0.5, r.y1 + 0.5),
        1.0,
        alpha(Color::WHITE, 0.08),
    );
}

/// Peak bars (dB, -60..0) with labels under them, laid out left to right inside `m`.
fn level_bars<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    m: Rect,
    bars: &[(&str, f32)],
    label_h: f64,
) {
    let n = bars.len().max(1) as f64;
    let w = ((m.width() - (n - 1.0) * 6.0) / n).clamp(6.0, 14.0);
    let total = n * w + (n - 1.0) * 6.0;
    let x0 = m.x0 + (m.width() - total) / 2.0;
    for (k, (label, db)) in bars.iter().enumerate() {
        let x = x0 + k as f64 * (w + 6.0);
        let r = Rect::new(x, m.y0, x + w, m.y1 - label_h);
        bar(c, r, level_norm(*db, -60.0, 0.0), meter_color(*db), false);
        small(
            c,
            theme,
            *label,
            pt(x + w / 2.0, m.y1 - 1.0),
            Align::Center,
            None,
        );
    }
}

fn gr_bar<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, r: Rect, label: &str, gr: f32, scale: f32) {
    bar(
        c,
        r,
        level_norm(gr, 0.0, scale),
        hsl(28.0, 0.85, 0.55),
        true,
    );
    small(
        c,
        theme,
        label,
        pt(r.center().x, r.y1 + 9.0),
        Align::Center,
        None,
    );
}

fn polyline(points: impl Iterator<Item = Point>) -> BezPath {
    let mut path = BezPath::new();
    for (i, p) in points.enumerate() {
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    path
}

/// Paints the visual of an effect panel.
pub fn fx_visual<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, fx: FxPanel, info: &PanelInfo<'_>) {
    let p = Params { info };
    match fx.kind {
        FxKind::Reverb => reverb(c, theme, fx, info, &p),
        FxKind::Compressor => compressor(c, theme, fx, info, &p),
        FxKind::Eq => eq(c, theme, fx, info),
        FxKind::Distortion => distortion(c, theme, fx, info, &p),
        FxKind::Modulation => modulation(c, theme, fx, info, &p),
        FxKind::Mastering => mastering(c, theme, fx, info, &p),
        FxKind::Character => crate::character_panel::paint_front(c, theme, &fx, info),
        FxKind::Spectrum => crate::analyser_paint::spectrum(
            c,
            theme,
            fx,
            info,
            crate::analyser_paint::SpectrumLook {
                range_db: p.get("range").clamp(30.0, 120.0),
                hold: p.get("hold") > 0.0,
                bypassed: crate::paint::bypassed(info),
            },
        ),
        FxKind::PhaseMeter => crate::analyser_paint::phase_meter(
            c,
            theme,
            fx,
            info,
            crate::analyser_paint::PhaseLook {
                mode: crate::analyser::ScopeMode::of_choice(p.get("scope")),
                gain_db: p.get("scope_gain"),
                loudness: p.on("loudness"),
                bypassed: crate::paint::bypassed(info),
            },
        ),
    }
}

// ------------------------------------------------------------------ reverb

fn reverb<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: FxPanel,
    info: &PanelInfo<'_>,
    p: &Params<'_>,
) {
    let r = fx.plot;
    lcd(c, theme, r);
    let sketch = fx::reverb_sketch(
        p.get("algorithm").round().max(0.0) as usize,
        p.get("predelay"),
        p.get("size"),
        p.get("decay"),
        p.get("early"),
        p.on("freeze"),
    );
    let inner = r.inset(-5.0);
    let mid = inner.center().y;
    let half = inner.height() / 2.0 - 8.0;
    // The time axis is square-rooted so the early taps stay visible next to a long tail.
    let x_of = |t: f32| inner.x0 + f64::from((t / sketch.total).max(0.0).sqrt()) * inner.width();
    c.stroke_with(
        Line::new(pt(inner.x0, mid), pt(inner.x1, mid)),
        1.0,
        alpha(Color::WHITE, 0.08),
    );
    if sketch.pre > 0.0 {
        c.fill_with(
            Rect::new(inner.x0, inner.y0, x_of(sketch.pre), inner.y1),
            alpha(info.accent, 0.1),
        );
    }
    // The tail envelope, drawn on a dB scale (a straight edge down to -60 dB).
    let steps = 64;
    let t_end = sketch.total;
    let pts: Vec<(f32, f64)> = (0..=steps)
        .map(|k| {
            let t = sketch.tail_start + (t_end - sketch.tail_start) * k as f32 / steps as f32;
            let h = f64::from(sketch.tail_level)
                * (1.0 + f64::from(sketch.tail_db_at(t)) / 60.0).clamp(0.0, 1.0);
            (t, h * half)
        })
        .collect();
    if sketch.tail_level > 0.0 {
        let mut path = BezPath::new();
        path.move_to(pt(x_of(pts[0].0), mid - pts[0].1));
        for (t, h) in &pts[1..] {
            path.line_to(pt(x_of(*t), mid - h));
        }
        for (t, h) in pts.iter().rev() {
            path.line_to(pt(x_of(*t), mid + h));
        }
        path.close_path();
        c.fill_with(path, alpha(info.accent, 0.38));
        c.stroke_with(
            polyline(pts.iter().map(|(t, h)| pt(x_of(*t), mid - h))),
            1.25,
            info.accent,
        );
        c.stroke_with(
            polyline(pts.iter().map(|(t, h)| pt(x_of(*t), mid + h))),
            1.25,
            alpha(info.accent, 0.6),
        );
    }
    for (t, lvl) in sketch.taps {
        if lvl > 0.0 {
            let x = x_of(t).round() + 0.5;
            let h = f64::from(lvl) * half;
            c.stroke_with(
                Line::new(pt(x, mid - h), pt(x, mid + h)),
                1.5,
                mix(info.accent, Color::WHITE, 0.45),
            );
        }
    }
    let rt = if sketch.frozen {
        "FROZEN".to_string()
    } else {
        format!("RT60 {:.1} s", sketch.decay)
    };
    small(
        c,
        theme,
        rt,
        pt(r.x1 - 6.0, r.y0 + 11.0),
        Align::End,
        Some(info.accent),
    );
    let name = ["ROOM", "HALL", "PLATE"][p.get("algorithm").round().clamp(0.0, 2.0) as usize];
    small(
        c,
        theme,
        name,
        pt(r.x0 + 6.0, r.y0 + 11.0),
        Align::Start,
        None,
    );
    small(
        c,
        theme,
        format!("PRE {:.0} ms", p.get("predelay")),
        pt(r.x0 + 6.0, r.y1 - 5.0),
        Align::Start,
        None,
    );
    level_bars(
        c,
        theme,
        fx.meters,
        &[
            ("L", meter(info, 0, LEVEL_FLOOR_DB)),
            ("R", meter(info, 1, LEVEL_FLOOR_DB)),
            ("TAIL", meter(info, 2, LEVEL_FLOOR_DB)),
        ],
        10.0,
    );
}

// ------------------------------------------------------------------ compressor

/// Plot position of an (in, out) level pair in dB on the transfer curve plot.
pub fn transfer_point(plot: Rect, in_db: f32, out_db: f32) -> Point {
    let x = plot.x0 + f64::from((in_db + 60.0) / 60.0).clamp(0.0, 1.0) * plot.width();
    let y = plot.y1 - f64::from((out_db + 60.0) / 60.0).clamp(0.0, 1.0) * plot.height();
    pt(x, y)
}

fn compressor<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: FxPanel,
    info: &PanelInfo<'_>,
    p: &Params<'_>,
) {
    let r = fx.plot;
    lcd(c, theme, r);
    let (thr, ratio, knee, limit) = (
        p.get("threshold"),
        p.get("ratio"),
        p.get("knee"),
        p.on("limit"),
    );
    for db in [-48.0f32, -36.0, -24.0, -12.0] {
        let a = transfer_point(r, db, -60.0);
        let b = transfer_point(r, db, 0.0);
        c.stroke_with(Line::new(a, b), 1.0, alpha(Color::WHITE, 0.05));
        let a = transfer_point(r, -60.0, db);
        let b = transfer_point(r, 0.0, db);
        c.stroke_with(Line::new(a, b), 1.0, alpha(Color::WHITE, 0.05));
    }
    c.stroke_with(
        Line::new(transfer_point(r, -60.0, -60.0), transfer_point(r, 0.0, 0.0)),
        1.0,
        alpha(Color::WHITE, 0.14),
    );
    let tx = transfer_point(r, thr, -60.0).x;
    c.stroke_with(
        Line::new(pt(tx, r.y1), pt(tx, r.y1 - 5.0)),
        1.5,
        mix(info.accent, Color::WHITE, 0.4),
    );
    let curve = polyline((0..=120).map(|k| {
        let x = -60.0 + k as f32 * 0.5;
        transfer_point(r, x, compressor_out_db(x, thr, ratio, knee, limit))
    }));
    c.stroke_with(curve, 1.75, info.accent);
    let in_db = meter(info, 0, LEVEL_FLOOR_DB);
    if in_db > -59.0 {
        let out = compressor_out_db(in_db.min(0.0), thr, ratio, knee, limit);
        let dot = transfer_point(r, in_db.min(0.0), out);
        c.fill_with(Circle::new(dot, 5.5), alpha(info.accent, 0.3));
        c.fill_with(Circle::new(dot, 2.6), Color::WHITE);
    }
    small(c, theme, "IN", pt(r.x1 - 5.0, r.y1 - 4.0), Align::End, None);
    small(
        c,
        theme,
        "OUT",
        pt(r.x0 + 4.0, r.y0 + 10.0),
        Align::Start,
        None,
    );
    small(
        c,
        theme,
        if limit {
            "INF:1".to_string()
        } else {
            format!("{ratio:.1}:1")
        },
        pt(r.x1 - 5.0, r.y0 + 10.0),
        Align::End,
        Some(info.accent),
    );
    // Meters: input and output peak, then the gain reduction.
    let m = fx.meters;
    let lh = 10.0;
    let w = 12.0;
    let x0 = m.x0 + 2.0;
    for (k, (label, db)) in [
        ("IN", meter(info, 0, LEVEL_FLOOR_DB)),
        ("OUT", meter(info, 1, LEVEL_FLOOR_DB)),
    ]
    .iter()
    .enumerate()
    {
        let x = x0 + k as f64 * (w + 6.0);
        let br = Rect::new(x, m.y0, x + w, m.y1 - lh);
        bar(c, br, level_norm(*db, -60.0, 0.0), meter_color(*db), false);
        small(
            c,
            theme,
            *label,
            pt(x + w / 2.0, m.y1 - 1.0),
            Align::Center,
            None,
        );
    }
    let gx = x0 + 2.0 * (w + 6.0);
    gr_bar(
        c,
        theme,
        Rect::new(gx, m.y0, gx + w, m.y1 - lh),
        "GR",
        meter(info, 2, 0.0),
        GR_SCALE_DB,
    );
    // The sidechain button and its indicator.
    if let Some(b) = fx.button {
        let enabled = p.on("sc_enable");
        let active = enabled && info.sc_source.is_some();
        let label = match info.sc_source {
            Some(name) => format!(
                "SC: {}",
                crate::paint::fit_label(name, b.width() - 44.0, 9.0)
            ),
            None => "SC: off".to_string(),
        };
        edit_button(c, theme, b, &label, true);
        let led = pt(b.x0 + 9.0, b.center().y);
        if active {
            c.fill_with(Circle::new(led, 5.0), alpha(info.accent, 0.3));
            c.fill_with(Circle::new(led, 2.4), mix(info.accent, Color::WHITE, 0.3));
        } else {
            c.fill_with(Circle::new(led, 2.4), theme.led_off);
        }
        let (ax, ay) = (b.x1 - 10.0, b.center().y);
        let mut arrow = BezPath::new();
        arrow.move_to(pt(ax - 3.5, ay - 1.5));
        arrow.line_to(pt(ax + 3.5, ay - 1.5));
        arrow.line_to(pt(ax, ay + 2.5));
        arrow.close_path();
        c.fill_with(arrow, theme.text_muted);
    }
}

// ------------------------------------------------------------------ EQ

fn band_color(i: usize) -> Color {
    hsl(i as f32 * 52.0 + 8.0, 0.65, 0.62)
}

fn eq<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, fx: FxPanel, info: &PanelInfo<'_>) {
    let r = fx.plot;
    lcd(c, theme, r);
    let Some(desc) = info.desc else { return };
    let bands = fx::eq_bands_of(desc, info.device);
    c.push_clip(&r.into());
    // Grid.
    for db in (-4..=4).map(|k| f64::from(k) * 6.0) {
        let y = eq_y_of(r, db).round() + 0.5;
        let a = if db == 0.0 { 0.2 } else { 0.06 };
        c.stroke_with(
            Line::new(pt(r.x0, y), pt(r.x1, y)),
            1.0,
            alpha(Color::WHITE, a),
        );
    }
    for f in [
        30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0,
        800.0, 900.0, 1000.0, 2000.0, 3000.0, 4000.0, 5000.0, 6000.0, 7000.0, 8000.0, 9000.0,
        10000.0,
    ] {
        let x = eq_x_of(r, f).round() + 0.5;
        let major = f == 100.0 || f == 1000.0 || f == 10000.0;
        let a = if major { 0.14 } else { 0.05 };
        c.stroke_with(
            Line::new(pt(x, r.y0), pt(x, r.y1)),
            1.0,
            alpha(Color::WHITE, a),
        );
    }
    // Analyser bars.
    for k in 0..fx::EQ_BANDS_ANALYSER {
        let db = meter(info, k, LEVEL_FLOOR_DB);
        let n = level_norm(db, -84.0, -6.0);
        if n <= 0.0 {
            continue;
        }
        let f = fx::analyser_centre(k);
        let (xa, xb) = (eq_x_of(r, f / 1.19), eq_x_of(r, f * 1.19));
        let h = n * r.height() * 0.6;
        c.fill_with(
            Rect::new(xa + 1.0, r.y1 - h, xb - 1.0, r.y1),
            alpha(info.accent, 0.28),
        );
    }
    // The response curve.
    let steps = (r.width() / 2.0).floor().max(2.0) as usize;
    let pts: Vec<Point> = (0..=steps)
        .map(|k| {
            let t = k as f64 / steps as f64;
            let f = fx::eq_freq_at(t);
            let db = fx::eq_response_db(&bands, f, EQ_DRAW_RATE);
            pt(r.x0 + t * r.width(), eq_y_of(r, db))
        })
        .collect();
    let zero = eq_y_of(r, 0.0);
    let mut fill = polyline(pts.iter().copied());
    fill.line_to(pt(r.x1, zero));
    fill.line_to(pt(r.x0, zero));
    fill.close_path();
    c.fill_with(fill, alpha(info.accent, 0.14));
    c.stroke_with(
        polyline(pts.iter().copied()),
        1.75,
        mix(info.accent, Color::WHITE, 0.35),
    );
    // Handles.
    for (i, b) in bands.iter().enumerate() {
        let at = eq_handle_pos(r, b);
        let col = band_color(i);
        if b.on {
            c.fill_with(Circle::new(at, EQ_HANDLE_R + 3.0), alpha(col, 0.2));
            c.fill_with(Circle::new(at, EQ_HANDLE_R), col);
        } else {
            c.fill_with(Circle::new(at, EQ_HANDLE_R), alpha(Color::BLACK, 0.5));
            c.stroke_with(Circle::new(at, EQ_HANDLE_R - 0.5), 1.25, alpha(col, 0.6));
        }
        c.text(
            &TextRun::new(
                format!("{}", i + 1),
                pt(at.x, at.y + 2.6),
                7.0,
                if b.on { Color::BLACK } else { theme.text_muted },
            )
            .align(Align::Center)
            .weight(700),
        );
    }
    c.pop_clip();
    // Axis labels.
    for (f, label) in [
        (30.0, "30"),
        (100.0, "100"),
        (300.0, "300"),
        (1000.0, "1k"),
        (3000.0, "3k"),
        (10000.0, "10k"),
    ] {
        small(
            c,
            theme,
            label,
            pt(eq_x_of(r, f), r.y1 + 9.0),
            Align::Center,
            None,
        );
    }
    for db in [-18.0, -12.0, 0.0, 12.0, 18.0] {
        let text = if db == 0.0 {
            "0".to_string()
        } else {
            format!("{db:+.0}")
        };
        small(
            c,
            theme,
            text,
            pt(r.x0 - 4.0, eq_y_of(r, db) + 2.5),
            Align::End,
            None,
        );
    }
    level_bars(
        c,
        theme,
        Rect::new(
            fx.meters.x0,
            fx.meters.y0,
            fx.meters.x1,
            fx.meters.y1 + 10.0,
        ),
        &[
            ("L", meter(info, 16, LEVEL_FLOOR_DB)),
            ("R", meter(info, 17, LEVEL_FLOOR_DB)),
        ],
        10.0,
    );
}

// ------------------------------------------------------------------ distortion

const CURVE_NAMES: [&str; 5] = ["SOFT CLIP", "HARD CLIP", "TUBE", "FOLDBACK", "BITCRUSH"];

fn distortion<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: FxPanel,
    info: &PanelInfo<'_>,
    p: &Params<'_>,
) {
    let r = fx.plot;
    lcd(c, theme, r);
    let curve = p.get("curve").round().clamp(0.0, 4.0) as usize;
    let (drive, bias, bits) = (p.get("drive"), p.get("bias"), p.get("crush.bits"));
    let at = |x: f32, y: f32| {
        pt(
            r.x0 + f64::from((x + 1.0) / 2.0) * r.width(),
            r.y1 - f64::from((y.clamp(-1.0, 1.0) + 1.0) / 2.0) * r.height(),
        )
    };
    c.stroke_with(
        Line::new(at(-1.0, 0.0), at(1.0, 0.0)),
        1.0,
        alpha(Color::WHITE, 0.12),
    );
    c.stroke_with(
        Line::new(at(0.0, -1.0), at(0.0, 1.0)),
        1.0,
        alpha(Color::WHITE, 0.12),
    );
    c.stroke_with(
        Line::new(at(-1.0, -1.0), at(1.0, 1.0)),
        1.0,
        alpha(Color::WHITE, 0.07),
    );
    let n = 160;
    let path = polyline((0..=n).map(|k| {
        let x = -1.0 + 2.0 * k as f32 / n as f32;
        at(x, fx::distortion_transfer(curve, drive, bias, bits, x))
    }));
    c.push_clip(&r.into());
    c.stroke_with(path, 1.75, info.accent);
    c.pop_clip();
    small(
        c,
        theme,
        CURVE_NAMES[curve],
        pt(r.x0 + 5.0, r.y0 + 10.0),
        Align::Start,
        Some(info.accent),
    );
    small(
        c,
        theme,
        format!("{drive:+.1} dB"),
        pt(r.x1 - 5.0, r.y1 - 4.0),
        Align::End,
        None,
    );
    level_bars(
        c,
        theme,
        fx.meters,
        &[
            ("IN", meter(info, 0, LEVEL_FLOOR_DB)),
            ("OUT", meter(info, 1, LEVEL_FLOOR_DB)),
        ],
        10.0,
    );
}

// ------------------------------------------------------------------ modulation

const MODE_NAMES: [&str; 3] = ["CHORUS", "FLANGER", "PHASER"];

fn modulation<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: FxPanel,
    info: &PanelInfo<'_>,
    p: &Params<'_>,
) {
    let r = fx.plot;
    lcd(c, theme, r);
    let inner = Rect::new(r.x0 + 6.0, r.y0 + 16.0, r.x1 - 6.0, r.y1 - 6.0);
    let mid = inner.center().y;
    let amp = (inner.height() / 2.0 - 2.0) * f64::from(p.get("depth").clamp(0.05, 1.0));
    let spread = p.get("spread") / 360.0;
    c.stroke_with(
        Line::new(pt(inner.x0, mid), pt(inner.x1, mid)),
        1.0,
        alpha(Color::WHITE, 0.1),
    );
    let cycles = 2.0f32;
    let n = 160;
    let curve = |offset: f32| {
        polyline((0..=n).map(|k| {
            let t = k as f32 / n as f32;
            pt(
                inner.x0 + f64::from(t) * inner.width(),
                mid - amp * f64::from(fx::lfo_wave(t * cycles + offset)),
            )
        }))
    };
    c.stroke_with(
        curve(spread),
        1.5,
        alpha(mix(info.accent, Color::WHITE, 0.5), 0.8),
    );
    c.stroke_with(curve(0.0), 1.75, info.accent);
    let sync = p.get("sync").round().clamp(0.0, 7.0) as usize;
    let rate = if sync == 0 {
        format!("{:.2} Hz", p.get("rate"))
    } else {
        format!("SYNC {}", SYNC_NAMES[sync - 1])
    };
    let mode = p.get("mode").round().clamp(0.0, 2.0) as usize;
    small(
        c,
        theme,
        MODE_NAMES[mode],
        pt(r.x0 + 6.0, r.y0 + 11.0),
        Align::Start,
        Some(info.accent),
    );
    small(
        c,
        theme,
        rate,
        pt(r.x1 - 6.0, r.y0 + 11.0),
        Align::End,
        None,
    );
    small(
        c,
        theme,
        if mode == 2 {
            format!("{} STAGES", (p.get("stages") / 2.0).ceil() as u32 * 2)
        } else {
            format!("{:.1} ms", p.get("delay"))
        },
        pt(r.x0 + 6.0, r.y1 - 5.0),
        Align::Start,
        None,
    );
    small(
        c,
        theme,
        "L / R",
        pt(r.x1 - 6.0, r.y1 - 5.0),
        Align::End,
        None,
    );
}

// ------------------------------------------------------------------ mastering

fn lufs_text(v: f32) -> String {
    if v <= -70.0 {
        "-inf".to_string()
    } else {
        format!("{v:.1}")
    }
}

fn mastering<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: FxPanel,
    info: &PanelInfo<'_>,
    p: &Params<'_>,
) {
    // Crossover display: log frequency axis, three band regions with their gain reduction.
    let r = fx.plot;
    lcd(c, theme, r);
    let multiband = p.on("multiband.on");
    let low = f64::from(p.get("xover.low"));
    let high = f64::from(p.get("xover.high")).max(1.25 * low);
    let pos = |f: f64| ((f / 20.0).ln() / (20_000.0f64 / 20.0).ln()).clamp(0.0, 1.0);
    let xs = [
        r.x0,
        r.x0 + pos(low) * r.width(),
        r.x0 + pos(high) * r.width(),
        r.x1,
    ];
    for b in 0..3 {
        let col = hsl(190.0 + b as f32 * 62.0, 0.6, 0.55);
        let reg = Rect::new(xs[b] + 0.5, r.y0 + 1.0, xs[b + 1] - 0.5, r.y1 - 1.0);
        let a = if multiband { 0.16 } else { 0.05 };
        c.fill_with(reg, alpha(col, a));
        if multiband {
            let gr = meter(info, 4 + b, 0.0);
            let h = (reg.height() - 14.0) * level_norm(gr, 0.0, 12.0);
            c.fill_with(
                Rect::new(reg.x0 + 2.0, reg.y0 + 1.0, reg.x1 - 2.0, reg.y0 + 1.0 + h),
                alpha(col, 0.65),
            );
        }
        small(
            c,
            theme,
            ["LOW", "MID", "HIGH"][b],
            pt((reg.x0 + reg.x1) / 2.0, r.y1 - 4.0),
            Align::Center,
            Some(alpha(col, 0.9)),
        );
    }
    for (x, f) in [(xs[1], low), (xs[2], high)] {
        c.stroke_with(
            Line::new(pt(x.round() + 0.5, r.y0), pt(x.round() + 0.5, r.y1)),
            1.0,
            alpha(Color::WHITE, 0.4),
        );
        let text = if f >= 1000.0 {
            format!("{:.1}k", f / 1000.0)
        } else {
            format!("{f:.0}")
        };
        small(c, theme, text, pt(x + 3.0, r.y1 - 14.0), Align::Start, None);
    }
    if !multiband {
        small(
            c,
            theme,
            "MULTIBAND OFF",
            pt(r.center().x, r.y0 + 12.0),
            Align::Center,
            None,
        );
    }
    // Loudness readouts.
    let ro = fx.readout;
    lcd(c, theme, ro);
    let rows = [
        ("MOMENTARY", lufs_text(meter(info, 0, -120.0)), "LUFS"),
        ("SHORT-TERM", lufs_text(meter(info, 1, -120.0)), "LUFS"),
        ("INTEGRATED", lufs_text(meter(info, 2, -120.0)), "LUFS"),
        ("TRUE PEAK", lufs_text(meter(info, 3, -120.0)), "dBTP"),
    ];
    let step = (ro.height() - 8.0) / rows.len() as f64;
    for (k, (label, value, unit)) in rows.iter().enumerate() {
        let y = ro.y0 + 8.0 + (k as f64 + 0.5) * step + 3.0;
        let strong = k == 2;
        small(c, theme, *label, pt(ro.x0 + 8.0, y), Align::Start, None);
        c.text(
            &TextRun::new(
                value.clone(),
                pt(ro.x1 - 40.0, y + 1.0),
                if strong { 13.0 } else { 11.0 },
                if strong {
                    mix(info.accent, Color::WHITE, 0.3)
                } else {
                    theme.text
                },
            )
            .align(Align::End)
            .weight(700),
        );
        small(c, theme, *unit, pt(ro.x1 - 36.0, y), Align::Start, None);
    }
    // Gain reduction meters: three bands and the limiter.
    let m = fx.meters;
    let w = 16.0;
    let gap = ((m.width() - 4.0 * w) / 5.0).max(2.0);
    for (k, label) in ["B1", "B2", "B3", "LIM"].iter().enumerate() {
        let x = m.x0 + gap + k as f64 * (w + gap);
        gr_bar(
            c,
            theme,
            Rect::new(x, m.y0, x + w, m.y1 - 12.0),
            label,
            meter(info, 4 + k, 0.0),
            12.0,
        );
    }
}
