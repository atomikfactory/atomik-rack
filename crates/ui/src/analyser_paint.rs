//! Painting of the two analyser panels: the spectrum plot (log frequency, dB range, trace or
//! one-third-octave bars, held peaks, loudest-bin readout) and the phase meter (goniometer,
//! correlation, balance and width bars, level bars with RMS and hold, loudness readouts).
//!
//! Drawn from the display meter values (`crate::fx::update_display`) and the
//! [`AnalyserData`](crate::analyser::AnalyserData) the app fed; with no feed the panels draw
//! empty. A bypassed device shows what it last saw, dimmed, with `BYPASSED` over it.

use crate::analyser::{
    AnalyserData, ScopeMode, balance_text, band_edges, correlation_text, freq_x, grid_step_db,
    level_text, level_y, peak_text, scope_point, width_text,
};
use crate::autolayout::FxPanel;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx::{LEVEL_FLOOR_DB, level_norm, phase};
use crate::fx_paint::{bar, lcd, meter_color, small};
use crate::paint::PanelInfo;
use crate::theme::{Theme, hsl, mix};
use vello::kurbo::{BezPath, Circle, Line, Point, Rect};
use vello::peniko::Color;

fn alpha(c: Color, a: f32) -> Color {
    c.with_alpha(a)
}

fn meter(info: &PanelInfo<'_>, i: usize, default: f32) -> f32 {
    info.fx_meters.get(i).copied().unwrap_or(default)
}

/// The settings of the spectrum panel the painter needs (read from the device's params).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpectrumLook {
    /// dB shown below 0 dB.
    pub range_db: f32,
    /// Peak hold is on (the peaks are drawn).
    pub hold: bool,
    /// Frozen: the device is bypassed.
    pub bypassed: bool,
}

/// The settings of the phase meter panel the painter needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhaseLook {
    /// Goniometer or Lissajous.
    pub mode: ScopeMode,
    /// Scope gain in dB.
    pub gain_db: f32,
    /// The loudness readouts are on.
    pub loudness: bool,
    /// Frozen: the device is bypassed.
    pub bypassed: bool,
}

/// Frequencies of the vertical grid lines, Hz; the decades are drawn brighter.
const GRID_HZ: [f64; 28] = [
    30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0,
    800.0, 900.0, 1000.0, 2000.0, 3000.0, 4000.0, 5000.0, 6000.0, 7000.0, 8000.0, 9000.0, 10_000.0,
    20_000.0, 20.0,
];

fn bypassed_banner<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, r: Rect) {
    c.text(
        &TextRun::new(
            "BYPASSED",
            pt(r.center().x, r.center().y + 4.0),
            11.0,
            theme.text,
        )
        .align(Align::Center)
        .weight(700),
    );
}

// ------------------------------------------------------------------ spectrum

/// The spectrum panel's visual.
pub fn spectrum<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: FxPanel,
    info: &PanelInfo<'_>,
    look: SpectrumLook,
) {
    let r = fx.plot;
    lcd(c, theme, r);
    let data: &AnalyserData = info.analyser;
    let dim = if look.bypassed { 0.35 } else { 1.0 };
    c.push_clip(&r.into());
    // Grid: levels every 5, 10 or 20 dB, frequencies every decade step.
    let step = grid_step_db(look.range_db);
    let mut db = 0.0f32;
    while db <= look.range_db + 0.01 {
        let y = level_y(r, -db, look.range_db).round() + 0.5;
        let a = if db == 0.0 { 0.2 } else { 0.06 };
        c.stroke_with(
            Line::new(pt(r.x0, y), pt(r.x1, y)),
            1.0,
            alpha(Color::WHITE, a),
        );
        db += step;
    }
    for f in GRID_HZ {
        let x = freq_x(r, f).round() + 0.5;
        let major = matches!(f as u32, 100 | 1000 | 10_000);
        c.stroke_with(
            Line::new(pt(x, r.y0), pt(x, r.y1)),
            1.0,
            alpha(Color::WHITE, if major { 0.14 } else { 0.05 }),
        );
    }
    let n = data.trace.len();
    if n > 0 && data.bands {
        for (i, v) in data.trace.iter().enumerate() {
            let (lo, hi) = band_edges(i);
            let (xa, xb) = (freq_x(r, lo), freq_x(r, hi));
            let y = level_y(r, *v, look.range_db);
            if y < r.y1 {
                c.fill_with(
                    Rect::new(xa + 1.0, y, xb - 1.0, r.y1),
                    alpha(info.accent, 0.5 * dim),
                );
                c.fill_with(
                    Rect::new(xa + 1.0, y, xb - 1.0, y + 1.5),
                    alpha(mix(info.accent, Color::WHITE, 0.4), dim),
                );
            }
        }
        if look.hold {
            for (i, v) in data.peaks.iter().enumerate() {
                let (lo, hi) = band_edges(i);
                let y = level_y(r, *v, look.range_db).round() + 0.5;
                c.stroke_with(
                    Line::new(pt(freq_x(r, lo) + 1.0, y), pt(freq_x(r, hi) - 1.0, y)),
                    1.0,
                    alpha(Color::WHITE, 0.6 * dim),
                );
            }
        }
    } else if n > 1 {
        let x_of = |i: usize| r.x0 + (i as f64 + 0.5) / n as f64 * r.width();
        let line = |values: &[f32]| {
            let mut path = BezPath::new();
            for (i, v) in values.iter().enumerate() {
                let p = pt(x_of(i), level_y(r, *v, look.range_db));
                if i == 0 {
                    path.move_to(p);
                } else {
                    path.line_to(p);
                }
            }
            path
        };
        let mut fill = line(&data.trace);
        fill.line_to(pt(x_of(n - 1), r.y1));
        fill.line_to(pt(x_of(0), r.y1));
        fill.close_path();
        c.fill_with(fill, alpha(info.accent, 0.2 * dim));
        c.stroke_with(
            line(&data.trace),
            1.5,
            alpha(mix(info.accent, Color::WHITE, 0.35), dim),
        );
        if look.hold && data.peaks.len() == n {
            c.stroke_with(line(&data.peaks), 1.0, alpha(Color::WHITE, 0.45 * dim));
        }
    }
    c.pop_clip();
    // Axis labels.
    for (f, label) in [
        (20.0, "20"),
        (50.0, "50"),
        (100.0, "100"),
        (200.0, "200"),
        (500.0, "500"),
        (1000.0, "1k"),
        (2000.0, "2k"),
        (5000.0, "5k"),
        (10_000.0, "10k"),
        (20_000.0, "20k"),
    ] {
        small(
            c,
            theme,
            label,
            pt(freq_x(r, f).clamp(r.x0 + 6.0, r.x1 - 6.0), r.y1 + 9.0),
            Align::Center,
            None,
        );
    }
    let mut db = 0.0f32;
    while db <= look.range_db + 0.01 {
        let text = if db == 0.0 {
            "0".to_string()
        } else {
            format!("-{db:.0}")
        };
        small(
            c,
            theme,
            text,
            pt(r.x0 - 4.0, level_y(r, -db, look.range_db) + 2.5),
            Align::End,
            None,
        );
        db += step;
    }
    // The loudest bin.
    let ro = fx.readout;
    c.text(
        &TextRun::new(
            peak_text(data.peak_hz, data.peak_db),
            pt(ro.x0, ro.y1 - 3.0),
            9.0,
            alpha(theme.text, if look.bypassed { 0.5 } else { 1.0 }),
        )
        .align(Align::Start)
        .weight(600),
    );
    if look.bypassed {
        bypassed_banner(c, theme, r);
    }
    // The input peaks.
    let m = fx.meters;
    let w = 10.0;
    for (k, label) in ["L", "R"].iter().enumerate() {
        let x = m.x0 + 2.0 + k as f64 * (w + 4.0);
        let db = meter(info, k, LEVEL_FLOOR_DB);
        bar(
            c,
            Rect::new(x, m.y0, x + w, m.y1),
            level_norm(db, -60.0, 0.0),
            meter_color(db),
            false,
        );
        small(
            c,
            theme,
            *label,
            pt(x + w / 2.0, m.y1 + 9.0),
            Align::Center,
            None,
        );
    }
}

// ------------------------------------------------------------------ phase meter

/// A bipolar horizontal bar: the fill runs from the centre to `value` (-1..=1).
fn centre_bar<C: Canvas + ?Sized>(c: &mut C, r: Rect, value: f32, color: Color) {
    c.fill_with(r, alpha(Color::BLACK, 0.55));
    let mid = r.center().x;
    let x = mid + f64::from(value.clamp(-1.0, 1.0)) * r.width() / 2.0;
    if (x - mid).abs() >= 0.5 {
        c.fill_with(Rect::new(mid.min(x), r.y0, mid.max(x), r.y1), color);
    }
    c.stroke_with(
        Line::new(
            pt(mid.round() + 0.5, r.y0 - 2.0),
            pt(mid.round() + 0.5, r.y1 + 2.0),
        ),
        1.0,
        alpha(Color::WHITE, 0.45),
    );
    c.stroke_with(
        Rect::new(r.x0 - 0.5, r.y0 - 0.5, r.x1 + 0.5, r.y1 + 0.5),
        1.0,
        alpha(Color::WHITE, 0.08),
    );
}

fn corr_color(v: f32) -> Color {
    if v >= 0.0 {
        hsl(140.0, 0.55, 0.5)
    } else if v > -0.5 {
        hsl(42.0, 0.8, 0.55)
    } else {
        hsl(6.0, 0.75, 0.55)
    }
}

/// The goniometer in `r`: guides, then the recent frames as a thin line.
fn goniometer<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    data: &AnalyserData,
    look: PhaseLook,
    accent: Color,
) {
    lcd(c, theme, r);
    let centre = r.center();
    let rad = (r.width().min(r.height()) / 2.0 - 7.0).max(10.0);
    let dim = if look.bypassed { 0.35 } else { 1.0 };
    c.push_clip(&r.into());
    c.stroke_with(Circle::new(centre, rad), 1.0, alpha(Color::WHITE, 0.14));
    c.stroke_with(
        Circle::new(centre, rad * 0.5),
        1.0,
        alpha(Color::WHITE, 0.06),
    );
    let axis = alpha(Color::WHITE, 0.1);
    c.stroke_with(
        Line::new(pt(centre.x, centre.y - rad), pt(centre.x, centre.y + rad)),
        1.0,
        axis,
    );
    c.stroke_with(
        Line::new(pt(centre.x - rad, centre.y), pt(centre.x + rad, centre.y)),
        1.0,
        axis,
    );
    let d = rad * std::f64::consts::FRAC_1_SQRT_2;
    if look.mode == ScopeMode::Goniometer {
        c.stroke_with(
            Line::new(
                pt(centre.x - d, centre.y - d),
                pt(centre.x + d, centre.y + d),
            ),
            1.0,
            axis,
        );
        c.stroke_with(
            Line::new(
                pt(centre.x - d, centre.y + d),
                pt(centre.x + d, centre.y - d),
            ),
            1.0,
            axis,
        );
    }
    let frames = data.scope_l.len().min(data.scope_r.len());
    if frames > 1 {
        let mut path = BezPath::new();
        for (i, (l, rr)) in data.scope_l.iter().zip(&data.scope_r).enumerate() {
            let (x, y) = scope_point(*l, *rr, look.mode, look.gain_db);
            let p = pt(centre.x + x * rad, centre.y - y * rad);
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        c.stroke_with(path, 0.9, alpha(mix(accent, Color::WHITE, 0.3), 0.7 * dim));
    }
    c.pop_clip();
    let lab = |c: &mut C, t: &str, at: Point, align: Align| small(c, theme, t, at, align, None);
    match look.mode {
        ScopeMode::Goniometer => {
            lab(c, "M", pt(centre.x, r.y0 + 8.0), Align::Center);
            lab(c, "L", pt(r.x0 + 3.0, r.y0 + 8.0), Align::Start);
            lab(c, "R", pt(r.x1 - 3.0, r.y0 + 8.0), Align::End);
            lab(c, "S", pt(r.x1 - 3.0, centre.y - 3.0), Align::End);
        }
        ScopeMode::Lissajous => {
            lab(c, "R", pt(centre.x, r.y0 + 8.0), Align::Center);
            lab(c, "L", pt(r.x1 - 3.0, centre.y - 3.0), Align::End);
        }
    }
    if look.bypassed {
        bypassed_banner(c, theme, r);
    }
}

/// The phase meter panel's visual.
pub fn phase_meter<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: FxPanel,
    info: &PanelInfo<'_>,
    look: PhaseLook,
) {
    goniometer(c, theme, fx.plot, info.analyser, look, info.accent);
    let dim: f32 = if look.bypassed { 0.5 } else { 1.0 };

    // Correlation, balance and width.
    let ro = fx.readout;
    lcd(c, theme, ro);
    let rows = [
        (
            "CORRELATION",
            meter(info, phase::CORRELATION, 0.0),
            correlation_text(meter(info, phase::CORRELATION, 0.0)),
            ("-1", "0", "+1"),
        ),
        (
            "BALANCE",
            meter(info, phase::BALANCE, 0.0),
            balance_text(meter(info, phase::BALANCE, 0.0)),
            ("L", "C", "R"),
        ),
        (
            "WIDTH",
            meter(info, phase::WIDTH, 0.0) * 2.0 - 1.0,
            width_text(meter(info, phase::WIDTH, 0.0)),
            ("0", "50", "100"),
        ),
    ];
    let pad = 8.0;
    let step = (ro.height() - 10.0) / rows.len() as f64;
    for (k, (label, value, text, (lo, mid, hi))) in rows.iter().enumerate() {
        let y = ro.y0 + 8.0 + k as f64 * step;
        small(
            c,
            theme,
            *label,
            pt(ro.x0 + pad, y + 5.0),
            Align::Start,
            None,
        );
        c.text(
            &TextRun::new(
                text.clone(),
                pt(ro.x1 - pad, y + 6.0),
                11.0,
                alpha(theme.text, dim),
            )
            .align(Align::End)
            .weight(700),
        );
        let bar_r = Rect::new(ro.x0 + pad, y + 11.0, ro.x1 - pad, y + 21.0);
        let color = if k == 0 {
            corr_color(*value)
        } else {
            mix(info.accent, Color::WHITE, 0.15)
        };
        // Width runs 0..1 from the left: a bar from the centre of a bipolar scale would
        // read as "negative" for narrow signals, so it is drawn as a plain fill.
        if k == 2 {
            bar_horizontal(c, bar_r, (*value + 1.0) / 2.0, alpha(color, dim));
        } else {
            centre_bar(c, bar_r, *value, alpha(color, dim));
        }
        small(
            c,
            theme,
            *lo,
            pt(bar_r.x0, bar_r.y1 + 7.0),
            Align::Start,
            None,
        );
        small(
            c,
            theme,
            *mid,
            pt(bar_r.center().x, bar_r.y1 + 7.0),
            Align::Center,
            None,
        );
        small(
            c,
            theme,
            *hi,
            pt(bar_r.x1, bar_r.y1 + 7.0),
            Align::End,
            None,
        );
    }

    // Level bars: peak, RMS and hold per channel on a -60 to 0 dB scale.
    let m = fx.meters;
    let w = 22.0;
    let plot = Rect::new(m.x0 + 2.0, m.y0, m.x0 + 2.0 + 2.0 * w + 8.0, m.y1 - 12.0);
    for (k, label) in ["L", "R"].iter().enumerate() {
        let x = plot.x0 + k as f64 * (w + 8.0);
        let col = Rect::new(x, plot.y0, x + w, plot.y1);
        let peak = meter(info, phase::PEAK_L + k, LEVEL_FLOOR_DB);
        let rms = meter(info, phase::RMS_L + k, LEVEL_FLOOR_DB);
        let hold = meter(info, phase::HOLD_L + k, LEVEL_FLOOR_DB);
        bar(c, col, 0.0, meter_color(peak), false);
        let h = |db: f32| col.height() * level_norm(db, -60.0, 0.0);
        c.fill_with(
            Rect::new(col.x0, col.y1 - h(peak), col.x1, col.y1),
            alpha(meter_color(peak), 0.4 * dim),
        );
        c.fill_with(
            Rect::new(col.x0 + 5.0, col.y1 - h(rms), col.x1 - 5.0, col.y1),
            alpha(meter_color(rms), dim),
        );
        if hold > -60.0 {
            let y = (col.y1 - h(hold)).round();
            c.fill_with(
                Rect::new(col.x0, y, col.x1, y + 2.0),
                alpha(Color::WHITE, 0.85 * dim),
            );
        }
        small(
            c,
            theme,
            *label,
            pt(col.center().x, m.y1 - 1.0),
            Align::Center,
            None,
        );
    }
    for db in [0.0f32, -6.0, -12.0, -24.0, -48.0] {
        let y = plot.y1 - plot.height() * level_norm(db, -60.0, 0.0);
        small(
            c,
            theme,
            format!("{db:.0}"),
            pt(plot.x1 + 4.0, y + 2.5),
            Align::Start,
            None,
        );
    }

    // Loudness and peaks.
    let ao = fx.aux;
    lcd(c, theme, ao);
    let on = look.loudness;
    let text = |v: f32, enabled: bool| {
        if enabled {
            level_text(v)
        } else {
            "--".to_string()
        }
    };
    let lines = [
        (
            "MOMENTARY",
            text(meter(info, phase::MOMENTARY, -120.0), on),
            "LUFS",
        ),
        (
            "SHORT-TERM",
            text(meter(info, phase::SHORT_TERM, -120.0), on),
            "LUFS",
        ),
        (
            "INTEGRATED",
            text(meter(info, phase::INTEGRATED, -120.0), on),
            "LUFS",
        ),
        (
            "PEAK",
            text(meter(info, phase::SAMPLE_PEAK_DB, -120.0), true),
            "dBFS",
        ),
        (
            "TRUE PEAK",
            text(meter(info, phase::TRUE_PEAK_DB, -120.0), on),
            "dBTP",
        ),
    ];
    let step = (ao.height() - 6.0) / lines.len() as f64;
    for (k, (label, value, unit)) in lines.iter().enumerate() {
        let y = ao.y0 + 6.0 + (k as f64 + 0.5) * step + 3.0;
        let strong = k == 2;
        small(
            c,
            theme,
            *label,
            pt(ao.x0 + 7.0, y - 5.0),
            Align::Start,
            None,
        );
        c.text(
            &TextRun::new(
                value.clone(),
                pt(ao.x1 - 34.0, y + 2.0),
                if strong { 12.0 } else { 10.0 },
                alpha(
                    if strong {
                        mix(info.accent, Color::WHITE, 0.3)
                    } else {
                        theme.text
                    },
                    dim,
                ),
            )
            .align(Align::End)
            .weight(700),
        );
        small(
            c,
            theme,
            *unit,
            pt(ao.x1 - 30.0, y + 1.0),
            Align::Start,
            None,
        );
    }
}

/// A plain horizontal fill from the left (`fill` 0..=1).
fn bar_horizontal<C: Canvas + ?Sized>(c: &mut C, r: Rect, fill: f32, color: Color) {
    c.fill_with(r, alpha(Color::BLACK, 0.55));
    let w = (r.width() * f64::from(fill.clamp(0.0, 1.0))).round();
    if w > 0.0 {
        c.fill_with(Rect::new(r.x0, r.y0, r.x0 + w, r.y1), color);
    }
    c.stroke_with(
        Rect::new(r.x0 - 0.5, r.y0 - 0.5, r.x1 + 0.5, r.y1 + 0.5),
        1.0,
        alpha(Color::WHITE, 0.08),
    );
}
