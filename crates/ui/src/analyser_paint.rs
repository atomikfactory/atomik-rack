//! Painting of the two analyser panels: the spectrum plot (log frequency, dB range, trace or
//! one-third-octave bars, held peaks, loudest-bin readout) and the phase meter (goniometer,
//! correlation, balance and width bars, level bars with RMS and hold, loudness readouts).
//!
//! Drawn from the display meter values (`crate::fx::update_display`) and the
//! [`AnalyserData`](crate::analyser::AnalyserData) the app fed; with no feed the panels draw
//! empty. A bypassed device shows what it last saw, dimmed, with `BYPASSED` over it.

use crate::analyser::{
    AnalyserData, LOUDNESS_ABOVE_LU, LOUDNESS_BELOW_LU, SPECTROGRAM_STEPS, ScopeMode, balance_text,
    band_centre_hz, band_edges, cell_intensity, correlation_text, freq_x, grid_step_db, level_text,
    level_y, loudness_runs, loudness_y, lra_text, peak_text, scope_point, spectrogram_step,
    target_text, width_text,
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
    /// The spectrogram mode: the image replaces the trace.
    pub spectrogram: bool,
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
    /// The loudness box shows the history graph instead of the readouts.
    pub history: bool,
    /// The target loudness in LUFS (the line of the history graph).
    pub target: f32,
    /// Frozen: the device is bypassed.
    pub bypassed: bool,
}

/// Frequencies of the vertical grid lines, Hz; the decades are drawn brighter.
const GRID_HZ: [f64; 28] = [
    30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0,
    800.0, 900.0, 1000.0, 2000.0, 3000.0, 4000.0, 5000.0, 6000.0, 7000.0, 8000.0, 9000.0, 10_000.0,
    20_000.0, 20.0,
];

/// The second curve of a Spectrum device: a line over the trace, in the warning colour.
fn second_curve<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    data: &AnalyserData,
    range_db: f32,
    dim: f32,
) {
    let n = data.second.len();
    if n < 2 {
        return;
    }
    let mut path = BezPath::new();
    let mut down = false;
    for (i, v) in data.second.iter().enumerate() {
        if *v <= -150.0 {
            down = false;
            continue;
        }
        let x = r.x0 + (i as f64 + 0.5) / n as f64 * r.width();
        let p = pt(x, level_y(r, *v, range_db));
        if down {
            path.line_to(p);
        } else {
            path.move_to(p);
            down = true;
        }
    }
    if !path.elements().is_empty() {
        c.stroke_with(path, 1.5, alpha(theme.warning, 0.9 * dim));
    }
}

/// The REF button in the corner of the plot, with the name of the chosen source to its left.
fn reference_button<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, r: Rect, data: &AnalyserData) {
    let b = crate::rack::spectrum_ref_button(r);
    let chosen = data
        .second_sources
        .get(data.second_source)
        .filter(|_| data.second_source > 0);
    crate::paint::button_sized(
        c,
        theme,
        b,
        "REF",
        !data.second_sources.is_empty(),
        8.5,
        0.8,
    );
    if let Some(source) = chosen {
        let label = source.label();
        c.text(
            &TextRun::verbatim(
                crate::paint::fit_label_raw(&label, 110.0, 8.5),
                pt(b.x0 - 5.0, b.y1 - 3.5),
                8.5,
                alpha(theme.warning, 0.95),
            )
            .align(Align::End)
            .weight(600),
        );
    }
}

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
    while db <= look.range_db + 0.01 && !look.spectrogram {
        let y = level_y(r, -db, look.range_db).round() + 0.5;
        let a = if db == 0.0 { 0.2 } else { 0.06 };
        c.stroke_with(
            Line::new(pt(r.x0, y), pt(r.x1, y)),
            1.0,
            alpha(theme.ink, a),
        );
        db += step;
    }
    if look.spectrogram {
        spectrogram_image(c, theme, r, data, look, info.accent, dim);
    }
    for f in GRID_HZ {
        let x = freq_x(r, f).round() + 0.5;
        let major = matches!(f as u32, 100 | 1000 | 10_000);
        c.stroke_with(
            Line::new(pt(x, r.y0), pt(x, r.y1)),
            1.0,
            alpha(theme.ink, if major { 0.14 } else { 0.05 }),
        );
    }
    let n = data.trace.len();
    if look.spectrogram {
        // The image is drawn under the grid above.
    } else if n > 0 && data.bands {
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
                    alpha(theme.ink, 0.6 * dim),
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
            c.stroke_with(line(&data.peaks), 1.0, alpha(theme.ink, 0.45 * dim));
        }
    }
    if !look.spectrogram {
        reference_line(c, theme, r, data, look.range_db, dim);
        second_curve(c, theme, r, data, look.range_db, dim);
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
    if look.spectrogram {
        small(
            c,
            theme,
            "NOW",
            pt(r.x0 - 4.0, r.y0 + 8.0),
            Align::End,
            None,
        );
        small(
            c,
            theme,
            format!("-{:.0}s", data.spectrogram_span_s.round()),
            pt(r.x0 - 4.0, r.y1 - 1.0),
            Align::End,
            None,
        );
    }
    let mut db = 0.0f32;
    while db <= look.range_db + 0.01 && !look.spectrogram {
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
    // The REF button and, when a second curve is chosen, its name in the curve's colour.
    if !look.spectrogram {
        reference_button(c, theme, r, data);
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

/// The colour of palette step `step` (`1..=SPECTROGRAM_STEPS`) of the spectrogram: from the
/// display's own dark through the device accent to a bright top.
fn spectrogram_color(theme: &Theme, accent: Color, step: usize, dim: f32) -> Color {
    let t = step as f32 / SPECTROGRAM_STEPS as f32;
    let c = if t < 0.6 {
        mix(theme.lcd, accent, t / 0.6)
    } else {
        mix(accent, theme.ink, (t - 0.6) / 0.4 * 0.7)
    };
    alpha(c, dim)
}

/// The spectrogram image: one row of cells per period, the newest at the top, a cell column per
/// column of the fine trace. Runs of equal colour in a row are one rectangle.
fn spectrogram_image<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    data: &AnalyserData,
    look: SpectrumLook,
    accent: Color,
    dim: f32,
) {
    let (cols, rows) = (data.spectrogram_cols, data.spectrogram_rows);
    if cols == 0 || rows == 0 || data.spectrogram.len() < cols * rows {
        return;
    }
    let (cw, rh) = (r.width() / cols as f64, r.height() / rows as f64);
    let palette: Vec<Color> = (1..=SPECTROGRAM_STEPS)
        .map(|s| spectrogram_color(theme, accent, s, dim))
        .collect();
    for row in 0..rows {
        let cells = &data.spectrogram[row * cols..(row + 1) * cols];
        let (y0, y1) = (r.y0 + row as f64 * rh, r.y0 + (row + 1) as f64 * rh + 0.5);
        let mut col = 0;
        while col < cols {
            let step = spectrogram_step(cell_intensity(cells[col], look.range_db));
            let mut end = col + 1;
            while end < cols && spectrogram_step(cell_intensity(cells[end], look.range_db)) == step
            {
                end += 1;
            }
            if step > 0 {
                c.fill_with(
                    Rect::new(
                        r.x0 + col as f64 * cw,
                        y0,
                        (r.x0 + end as f64 * cw + 0.5).min(r.x1),
                        y1.min(r.y1),
                    ),
                    palette[step - 1],
                );
            }
            col = end;
        }
    }
}

/// The reference curve over the plot: a line through the band levels at the band centres.
fn reference_line<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    data: &AnalyserData,
    range_db: f32,
    dim: f32,
) {
    let mut path = BezPath::new();
    let mut pen_down = false;
    for (i, v) in data.reference.iter().enumerate() {
        if *v <= -150.0 {
            pen_down = false;
            continue;
        }
        let p = pt(freq_x(r, band_centre_hz(i)), level_y(r, *v, range_db));
        if pen_down {
            path.line_to(p);
        } else {
            path.move_to(p);
            pen_down = true;
        }
    }
    if !path.elements().is_empty() {
        c.stroke_with(path, 1.5, alpha(theme.warning, 0.9 * dim));
    }
}

// ------------------------------------------------------------------ phase meter

/// A bipolar horizontal bar: the fill runs from the centre to `value` (-1..=1).
pub(crate) fn centre_bar<C: Canvas + ?Sized>(c: &mut C, r: Rect, value: f32, color: Color) {
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

pub(crate) fn corr_color(v: f32) -> Color {
    if v >= 0.0 {
        hsl(140.0, 0.55, 0.5)
    } else if v > -0.5 {
        hsl(42.0, 0.8, 0.55)
    } else {
        hsl(6.0, 0.75, 0.55)
    }
}

/// The goniometer in `r`: guides, then the recent frames as a thin line.
pub(crate) fn goniometer<C: Canvas + ?Sized>(
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
    c.stroke_with(Circle::new(centre, rad), 1.0, alpha(theme.ink, 0.14));
    c.stroke_with(Circle::new(centre, rad * 0.5), 1.0, alpha(theme.ink, 0.06));
    let axis = alpha(theme.ink, 0.1);
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
                alpha(theme.ink, 0.85 * dim),
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
    if look.history {
        loudness_history(c, theme, ao, info, look, dim);
        if look.bypassed {
            bypassed_banner(c, theme, fx.plot);
        }
        return;
    }
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

/// The loudness box in its history form: the short-term loudness of the last five minutes with
/// a line at the target, and below it the integrated loudness, the loudness range and the target.
fn loudness_history<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    ao: Rect,
    info: &PanelInfo<'_>,
    look: PhaseLook,
    dim: f32,
) {
    let data: &AnalyserData = info.analyser;
    let on = look.loudness;
    let g = Rect::new(ao.x0 + 6.0, ao.y0 + 14.0, ao.x1 - 22.0, ao.y1 - 22.0);
    small(
        c,
        theme,
        "SHORT-TERM",
        pt(ao.x0 + 7.0, ao.y0 + 9.0),
        Align::Start,
        None,
    );
    let now = if on {
        level_text(meter(info, phase::SHORT_TERM, -120.0))
    } else {
        "--".to_string()
    };
    c.text(
        &TextRun::new(
            format!("{now} LUFS"),
            pt(ao.x1 - 7.0, ao.y0 + 10.0),
            9.0,
            alpha(theme.text, dim),
        )
        .align(Align::End)
        .weight(700),
    );
    c.fill_with(g, alpha(theme.ink, 0.04));
    c.push_clip(&g.into());
    // A line every 6 LU from the target, the target brighter and in the warning colour.
    let mut offset = -LOUDNESS_BELOW_LU + 6.0;
    while offset < LOUDNESS_ABOVE_LU {
        if offset.abs() > 0.01 {
            let y = loudness_y(g, look.target + offset, look.target).round() + 0.5;
            c.stroke_with(
                Line::new(pt(g.x0, y), pt(g.x1, y)),
                1.0,
                alpha(theme.ink, 0.06),
            );
        }
        offset += 6.0;
    }
    let trace_color = alpha(mix(info.accent, theme.ink, 0.3), dim);
    if on {
        for run in loudness_runs(g, &data.loudness_history, look.target) {
            // A lone second is drawn as a dot (a line of no length shows nothing).
            if let [(x, y)] = run[..] {
                c.fill_with(Rect::new(x - 1.0, y - 1.0, x + 1.0, y + 1.0), trace_color);
                continue;
            }
            let mut path = BezPath::new();
            for (i, (x, y)) in run.iter().enumerate() {
                if i == 0 {
                    path.move_to(pt(*x, *y));
                } else {
                    path.line_to(pt(*x, *y));
                }
            }
            c.stroke_with(path, 1.5, trace_color);
        }
    }
    let ty = loudness_y(g, look.target, look.target).round() + 0.5;
    c.stroke_with(
        Line::new(pt(g.x0, ty), pt(g.x1, ty)),
        1.0,
        alpha(theme.warning, 0.9 * dim),
    );
    c.pop_clip();
    small(
        c,
        theme,
        format!("{:.0}", look.target),
        pt(g.x1 + 3.0, ty + 2.5),
        Align::Start,
        Some(alpha(theme.warning, dim)),
    );
    // The foot: integrated loudness, range and target.
    let foot = ao.y1 - 7.0;
    let integrated = if on {
        level_text(meter(info, phase::INTEGRATED, -120.0))
    } else {
        "--".to_string()
    };
    small(
        c,
        theme,
        format!("INT {integrated}"),
        pt(ao.x0 + 7.0, foot),
        Align::Start,
        None,
    );
    small(
        c,
        theme,
        if on {
            lra_text(data.lra)
        } else {
            "LRA --".to_string()
        },
        pt(ao.center().x, foot),
        Align::Center,
        None,
    );
    small(
        c,
        theme,
        target_text(look.target),
        pt(ao.x1 - 7.0, foot),
        Align::End,
        Some(alpha(theme.warning, dim)),
    );
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
