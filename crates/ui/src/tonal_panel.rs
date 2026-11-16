//! The rack panel of `atomik.tonal-balance`: four region meters and the balance curve against a
//! target, on the usual effect layout (a visual across the top, the settings under it).
//!
//! ```text
//! TARGET  Balanced  v                                     status text
//! +4.2      -0.6      +1.1      -2.0            20 Hz ........................ 20 kHz
//!  |#|       | |       | |       | |                 target range (shaded), target,
//!  |#|-------|=|-------|=|-------|=|-----            and the mix curve over it
//!  LOW     LOW-MID   HIGH-MID    HIGH
//! ```
//!
//! Each meter is centred on the target's level for its region (0 dB at the middle, plus and minus
//! [`METER_RANGE_DB`] over the height); the shaded band is the range the target accepts, and the
//! marker is where the mix sits, in the OK colour inside the range and the warning colour outside
//! it. The number over it is the deviation in dB: positive is too loud, negative too quiet.
//!
//! The analysis is the app's (`atomik_dsp::tonal`, fed as a [`TonalView`] inside
//! [`crate::analyser::AnalyserData`]). The TARGET button opens a list ([`menu_rows`]): the
//! built-in targets, the learned one, the saved ones, and the actions that learn from files, save
//! the learned target, rename and delete a saved one. Choosing a target is the `target` param; the
//! actions are [`crate::UiAction::TonalTarget`]. A name is typed in place of the button
//! ([`TonalOp::Save`], [`TonalOp::Rename`]).

use crate::analyser::{band_centre_hz, freq_x};
use crate::autolayout::FxPanel;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx_paint::{lcd, small};
use crate::paint::PanelInfo;
use crate::theme::Theme;
use atomik_dsp::spectrum::BAND_COUNT;
use atomik_dsp::tonal::REGIONS;
use vello::kurbo::{BezPath, Line, Point, Rect, RoundedRect};
use vello::peniko::Color;

/// The device type of this panel.
pub const TONAL_TYPE: &str = "atomik.tonal-balance";
/// The deviation the top and the bottom of a meter stand for, dB.
pub const METER_RANGE_DB: f32 = 9.0;
/// The names of the regions on the meters.
pub const REGION_LABELS: [&str; REGIONS] = ["LOW", "LOW-MID", "HIGH-MID", "HIGH"];
/// The crossovers under each name.
pub const REGION_HZ: [&str; REGIONS] = ["< 150 Hz", "150 Hz - 1 kHz", "1 - 5 kHz", "> 5 kHz"];
/// The `view` choice showing only the curve.
pub const VIEW_CURVE: u8 = 1;
/// The `view` choice showing only the meters.
pub const VIEW_BANDS: u8 = 0;

/// Height of the header row (the target button and the status), px.
const HEADER_H: f64 = 17.0;
/// Width of the TARGET button, px.
const TARGET_W: f64 = 226.0;
/// Height of a row of the target list, px.
pub const ROW_H: f64 = 16.0;
/// Width of the target list, px.
pub const MENU_W: f64 = 232.0;
/// Width of a meter column when the curve is shown beside the meters, px.
const COLUMN_W: f64 = 66.0;

/// What a target of the list is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// One of the built-in targets.
    Builtin,
    /// The target just learned and not saved yet.
    Learned,
    /// A saved target of the user.
    User,
}

/// One target of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetRow {
    /// The `target` param value that picks it.
    pub value: f32,
    /// Its name.
    pub name: String,
    /// What it is.
    pub kind: TargetKind,
}

/// What the app hands the view about one tonal balance device.
#[derive(Debug, Clone, PartialEq)]
pub struct TonalView {
    /// A reading exists (there is signal).
    pub ready: bool,
    /// Mix minus target per region, dB.
    pub deviation: [f32; REGIONS],
    /// The half-width of each region's accepted range, dB.
    pub tolerance: [f32; REGIONS],
    /// The mix's normalised band levels, dB (empty without a reading).
    pub bands: Vec<f32>,
    /// The target's normalised band levels, dB.
    pub target_bands: Vec<f32>,
    /// The half-height of the target's range at each band, dB.
    pub target_tol: Vec<f32>,
    /// The name of the target in force.
    pub target_name: String,
    /// The `target` param asked for a target that is not there: Balanced is in force.
    pub missing: bool,
    /// Every target the TARGET list offers.
    pub targets: Vec<TargetRow>,
    /// The row of `targets` in force.
    pub selected: usize,
    /// The live learn is running.
    pub learning: bool,
    /// Seconds of audio the live learn has taken in.
    pub learned_seconds: f32,
    /// Progress of the analysis of files, `0.0..=1.0`, while one runs.
    pub files: Option<f32>,
    /// A learned target waits to be saved.
    pub pending: bool,
}

impl Default for TonalView {
    fn default() -> Self {
        Self {
            ready: false,
            deviation: [0.0; REGIONS],
            tolerance: [3.0; REGIONS],
            bands: Vec::new(),
            target_bands: Vec::new(),
            target_tol: Vec::new(),
            target_name: String::new(),
            missing: false,
            targets: Vec::new(),
            selected: 0,
            learning: false,
            learned_seconds: 0.0,
            files: None,
            pending: false,
        }
    }
}

impl TonalView {
    /// The kind of the target in force.
    pub fn selected_kind(&self) -> Option<TargetKind> {
        self.targets.get(self.selected).map(|t| t.kind)
    }
}

/// What an entry of the target list does besides picking a target.
#[derive(Debug, Clone, PartialEq)]
pub enum TonalOp {
    /// Pick files and learn a target from them.
    LearnFiles,
    /// Save the learned target under this name.
    Save(String),
    /// Rename the saved target in force.
    Rename(String),
    /// Delete the saved target in force.
    Delete,
}

/// An entry of the target list.
#[derive(Debug, Clone, PartialEq)]
pub enum RowAction {
    /// Pick the target with this `target` value.
    Pick(f32),
    /// Learn from files.
    LearnFiles,
    /// Ask a name, then save the learned target.
    Save,
    /// Ask a name, then rename the target in force.
    Rename,
    /// Delete the target in force.
    Delete,
}

/// A row of the open target list.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuRow {
    /// The text.
    pub label: String,
    /// Whether the text is a name of the user's (shown as it is) rather than a label.
    pub verbatim: bool,
    /// What a press does.
    pub action: RowAction,
    /// A press does something.
    pub enabled: bool,
    /// The row is the target in force.
    pub checked: bool,
    /// A rule is drawn above the row.
    pub rule: bool,
}

/// The rows of the target list for `view`: the targets, then the actions.
pub fn menu_rows(view: &TonalView) -> Vec<MenuRow> {
    let mut rows: Vec<MenuRow> = view
        .targets
        .iter()
        .enumerate()
        .map(|(i, t)| MenuRow {
            label: if t.kind == TargetKind::Learned {
                format!("{} ({})", t.name, crate::i18n::tr("not saved"))
            } else {
                t.name.clone()
            },
            verbatim: t.kind == TargetKind::User,
            action: RowAction::Pick(t.value),
            enabled: true,
            checked: i == view.selected && !view.missing,
            rule: i > 0 && t.kind != view.targets[i - 1].kind,
        })
        .collect();
    let user = view.selected_kind() == Some(TargetKind::User);
    let action = |label: &str, action: RowAction, enabled: bool, rule: bool| MenuRow {
        label: label.to_string(),
        verbatim: false,
        action,
        enabled,
        checked: false,
        rule,
    };
    rows.push(action(
        "Learn from files…",
        RowAction::LearnFiles,
        view.files.is_none(),
        true,
    ));
    rows.push(action(
        "Save learned target…",
        RowAction::Save,
        view.pending,
        false,
    ));
    rows.push(action("Rename target…", RowAction::Rename, user, false));
    rows.push(action("Delete target", RowAction::Delete, user, false));
    rows
}

/// The parts of the visual, in panel coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TonalGeom {
    /// The TARGET button (and the field a name is typed in).
    pub target: Rect,
    /// The status text's box, right of the button.
    pub status: Rect,
    /// The column of each region meter (bar, number and names); empty when the meters are hidden.
    pub meters: [Rect; REGIONS],
    /// The curve plot; empty when the curve is hidden.
    pub curve: Rect,
}

/// The parts of the visual `v` for the `view` choice (0 meters, 1 curve, 2 both).
pub fn geometry(v: Rect, view: u8) -> TonalGeom {
    let target = Rect::new(
        v.x0 + 6.0,
        v.y0 + 4.0,
        v.x0 + 6.0 + TARGET_W,
        v.y0 + 4.0 + HEADER_H,
    );
    let status = Rect::new(target.x1 + 10.0, target.y0, v.x1 - 8.0, target.y1);
    let body = Rect::new(v.x0 + 6.0, target.y1 + 6.0, v.x1 - 6.0, v.y1 - 5.0);
    let (meters_w, curve) = match view {
        VIEW_BANDS => (body.width(), Rect::ZERO),
        VIEW_CURVE => (0.0, body),
        _ => {
            let w = COLUMN_W * REGIONS as f64;
            (w, Rect::new(body.x0 + w + 14.0, body.y0, body.x1, body.y1))
        }
    };
    let column = meters_w / REGIONS as f64;
    let meters = std::array::from_fn(|i| {
        if meters_w <= 0.0 {
            Rect::ZERO
        } else {
            Rect::new(
                body.x0 + column * i as f64,
                body.y0,
                body.x0 + column * (i as f64 + 1.0),
                body.y1,
            )
        }
    });
    TonalGeom {
        target,
        status,
        meters,
        curve,
    }
}

/// The bar of a meter column: between the number above it and the names under it.
pub fn bar_rect(column: Rect) -> Rect {
    let w = 24.0_f64.min(column.width() - 8.0).max(8.0);
    let cx = column.center().x;
    Rect::new(
        cx - w / 2.0,
        column.y0 + 13.0,
        cx + w / 2.0,
        column.y1 - 22.0,
    )
}

/// The y of a deviation `db` on `bar` (0 dB at the middle, clamped to the bar).
pub fn meter_y(bar: Rect, db: f32) -> f64 {
    let t = f64::from((db / METER_RANGE_DB).clamp(-1.0, 1.0));
    bar.center().y - t * bar.height() / 2.0
}

/// The deviation readout: `+4.2`, `-0.6`, or `--` without a reading.
pub fn deviation_text(db: f32, ready: bool) -> String {
    if ready && db.is_finite() {
        format!("{db:+.1}")
    } else {
        "--".to_string()
    }
}

/// Whether a deviation is inside the range of tolerance `tol`.
pub fn in_range(db: f32, tol: f32) -> bool {
    db.abs() <= tol
}

/// The status line: what the panel is doing or what is wrong, empty when all is well.
pub fn status_text(view: &TonalView) -> String {
    if let Some(p) = view.files {
        format!("{} {:.0}%", crate::i18n::tr("ANALYSING FILES"), p * 100.0)
    } else if view.learning {
        format!(
            "{} {:.0} s",
            crate::i18n::tr("LEARNING"),
            view.learned_seconds
        )
    } else if view.missing {
        crate::i18n::tr("TARGET MISSING, USING BALANCED").into_owned()
    } else if view.pending {
        crate::i18n::tr("LEARNED, NOT SAVED").into_owned()
    } else if !view.ready {
        crate::i18n::tr("NO SIGNAL").into_owned()
    } else {
        String::new()
    }
}

/// The vertical range `(low, high)` in dB the curve plot shows: the target's range and the mix
/// with some air, at least 30 dB tall.
pub fn curve_range(view: &TonalView) -> (f32, f32) {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for (i, v) in view.target_bands.iter().enumerate() {
        let tol = view.target_tol.get(i).copied().unwrap_or(3.0);
        lo = lo.min(v - tol);
        hi = hi.max(v + tol);
    }
    for v in &view.bands {
        lo = lo.min(*v);
        hi = hi.max(*v);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (-30.0, 6.0);
    }
    let (mut lo, mut hi) = (lo - 3.0, hi + 3.0);
    if hi - lo < 30.0 {
        let mid = (hi + lo) / 2.0;
        (lo, hi) = (mid - 15.0, mid + 15.0);
    }
    (lo.max(-90.0), hi)
}

/// The y of `db` in a plot showing `range` over `plot`.
pub fn curve_y(plot: Rect, db: f32, range: (f32, f32)) -> f64 {
    let t = f64::from(((db - range.0) / (range.1 - range.0).max(1.0)).clamp(0.0, 1.0));
    plot.y1 - t * plot.height()
}

/// Points of a smooth curve through the band values `values` (Catmull-Rom between the band
/// centres, `steps` points per band) as `(hz, value)`.
pub fn smooth_curve(values: &[f32], steps: usize) -> Vec<(f64, f32)> {
    let n = values.len().min(BAND_COUNT);
    let mut out = Vec::new();
    if n < 2 {
        return out;
    }
    let at = |i: isize| values[i.clamp(0, n as isize - 1) as usize];
    for i in 0..n - 1 {
        for s in 0..steps.max(1) {
            let t = s as f32 / steps.max(1) as f32;
            let (p0, p1, p2, p3) = (
                at(i as isize - 1),
                at(i as isize),
                at(i as isize + 1),
                at(i as isize + 2),
            );
            let v = 0.5
                * (2.0 * p1
                    + (p2 - p0) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
                    + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t * t * t);
            let (f1, f2) = (band_centre_hz(i), band_centre_hz(i + 1));
            let hz = f1 * (f2 / f1).powf(f64::from(t));
            out.push((hz, v));
        }
    }
    out.push((band_centre_hz(n - 1), values[n - 1]));
    out
}

// ------------------------------------------------------------------ painting

fn value(info: &PanelInfo<'_>, key: &str) -> f32 {
    let Some(desc) = info.desc else { return 0.0 };
    desc.params
        .iter()
        .find(|p| p.key == key)
        .map_or(0.0, |p| match info.device {
            Some(d) => crate::widget::current_value(d, p),
            None => p.default,
        })
}

fn alpha(c: Color, a: f32) -> Color {
    c.with_alpha(a)
}

/// Paints the visual: the target button, the status, the meters and the curve.
pub fn paint<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, fx: FxPanel, info: &PanelInfo<'_>) {
    let v = fx.visual;
    lcd(c, theme, v);
    let view = info.analyser.tonal.clone();
    let mode = value(info, "view").round().clamp(0.0, 2.0) as u8;
    let bypassed = value(info, "bypass") >= 0.5;
    let dim = if bypassed { 0.35 } else { 1.0 };
    let g = geometry(v, mode);
    c.push_clip(&v.into());
    paint_target(c, theme, &g, &view, info);
    let status = status_text(&view);
    if !status.is_empty() {
        let warn = view.missing || view.pending;
        c.text(
            &TextRun::new(
                status,
                pt(g.status.x1, g.status.y1 - 4.0),
                8.0,
                if warn {
                    theme.warning
                } else {
                    theme.text_muted
                },
            )
            .align(Align::End)
            .weight(600),
        );
    }
    if g.meters[0].width() > 0.0 {
        for (i, column) in g.meters.iter().enumerate() {
            paint_meter(c, theme, *column, i, &view, dim);
        }
    }
    if g.curve.width() > 0.0 {
        paint_curve(c, theme, g.curve, &view, info.accent, dim);
    }
    if bypassed {
        c.text(
            &TextRun::new(
                "BYPASSED",
                pt(v.center().x, v.center().y + 4.0),
                11.0,
                theme.text,
            )
            .align(Align::Center)
            .weight(700),
        );
    }
    c.pop_clip();
}

fn paint_target<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    g: &TonalGeom,
    view: &TonalView,
    info: &PanelInfo<'_>,
) {
    let b = g.target;
    if let Some(edit) = info.tonal_edit {
        c.fill_with(RoundedRect::from_rect(b, 3.0), theme.lcd);
        c.stroke_with(RoundedRect::from_rect(b.inset(-0.5), 3.5), 1.0, info.accent);
        crate::seq_panel::paint_name_field(c, b, edit, 9.0, info.accent);
        return;
    }
    crate::paint::button_sized(c, theme, b, "", true, 8.5, 0.8);
    let kind = view.targets.get(view.selected).map(|t| t.kind);
    let name = if view.target_name.is_empty() {
        crate::i18n::tr("Balanced").into_owned()
    } else if kind == Some(TargetKind::User) {
        view.target_name.clone()
    } else {
        crate::i18n::tr(&view.target_name).into_owned()
    };
    c.text(
        &TextRun::new("TARGET", pt(b.x0 + 7.0, b.y1 - 5.0), 7.5, theme.text_muted)
            .weight(600)
            .spacing(0.8),
    );
    c.text(
        &TextRun::verbatim(
            crate::paint::fit_label_raw(&name, b.width() - 78.0, 9.0),
            pt(b.x0 + 52.0, b.y1 - 4.5),
            9.0,
            theme.text,
        )
        .weight(700),
    );
    // The arrow of a list.
    let (ax, ay) = (b.x1 - 11.0, b.center().y);
    let mut arrow = BezPath::new();
    arrow.move_to(pt(ax - 3.5, ay - 1.5));
    arrow.line_to(pt(ax + 3.5, ay - 1.5));
    arrow.line_to(pt(ax, ay + 2.5));
    arrow.close_path();
    c.fill_with(arrow, theme.text_muted);
}

fn paint_meter<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    column: Rect,
    i: usize,
    view: &TonalView,
    dim: f32,
) {
    let bar = bar_rect(column);
    let tol = view.tolerance[i];
    let dev = view.deviation[i];
    let ready = view.ready;
    let ok = in_range(dev, tol);
    let colour = if ok {
        theme.io_connected
    } else {
        theme.warning
    };
    c.fill_with(bar, alpha(Color::BLACK, 0.55));
    // The accepted range.
    let (top, bottom) = (meter_y(bar, tol), meter_y(bar, -tol));
    c.fill_with(
        Rect::new(bar.x0, top, bar.x1, bottom),
        alpha(theme.io_connected, 0.22 * dim),
    );
    c.stroke_with(
        Rect::new(bar.x0, top, bar.x1, bottom),
        1.0,
        alpha(theme.io_connected, 0.55 * dim),
    );
    // Ticks every 3 dB and the target's level.
    for k in [-6.0f32, -3.0, 3.0, 6.0] {
        let y = meter_y(bar, k).round() + 0.5;
        c.stroke_with(
            Line::new(pt(bar.x0, y), pt(bar.x0 + 4.0, y)),
            1.0,
            alpha(theme.ink, 0.25),
        );
        c.stroke_with(
            Line::new(pt(bar.x1 - 4.0, y), pt(bar.x1, y)),
            1.0,
            alpha(theme.ink, 0.25),
        );
    }
    let mid = meter_y(bar, 0.0).round() + 0.5;
    c.stroke_with(
        Line::new(pt(bar.x0 - 3.0, mid), pt(bar.x1 + 3.0, mid)),
        1.0,
        alpha(theme.ink, 0.45),
    );
    if ready {
        let y = meter_y(bar, dev);
        let cx = bar.center().x;
        c.fill_with(
            Rect::new(cx - 2.0, y.min(mid), cx + 2.0, y.max(mid)),
            alpha(colour, 0.7 * dim),
        );
        c.fill_with(
            RoundedRect::new(bar.x0 - 3.0, y - 1.5, bar.x1 + 3.0, y + 1.5, 1.5),
            alpha(colour, dim),
        );
    }
    c.text(
        &TextRun::new(
            deviation_text(dev, ready),
            pt(column.center().x, column.y0 + 9.0),
            9.0,
            if ready { colour } else { theme.text_muted },
        )
        .align(Align::Center)
        .weight(700),
    );
    small(
        c,
        theme,
        REGION_LABELS[i],
        pt(column.center().x, column.y1 - 11.0),
        Align::Center,
        Some(theme.text),
    );
    c.text(
        &TextRun::new(
            REGION_HZ[i],
            pt(column.center().x, column.y1 - 2.0),
            6.5,
            theme.text_muted,
        )
        .align(Align::Center),
    );
}

fn paint_curve<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    plot: Rect,
    view: &TonalView,
    accent: Color,
    dim: f32,
) {
    c.fill_with(plot, alpha(Color::BLACK, 0.3));
    c.push_clip(&plot.into());
    for f in [
        30.0, 40.0, 50.0, 60.0, 80.0, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 800.0, 1000.0,
        2000.0, 3000.0, 4000.0, 5000.0, 6000.0, 8000.0, 10_000.0,
    ] {
        let x = freq_x(plot, f).round() + 0.5;
        let major = matches!(f as u32, 100 | 1000 | 10_000);
        c.stroke_with(
            Line::new(pt(x, plot.y0), pt(x, plot.y1)),
            1.0,
            alpha(theme.ink, if major { 0.14 } else { 0.05 }),
        );
    }
    let range = curve_range(view);
    let n = view.target_bands.len();
    if n >= 2 {
        // The range: the target plus and minus its tolerance, smoothed.
        let upper: Vec<f32> = (0..n)
            .map(|i| view.target_bands[i] + view.target_tol.get(i).copied().unwrap_or(3.0))
            .collect();
        let lower: Vec<f32> = (0..n)
            .map(|i| view.target_bands[i] - view.target_tol.get(i).copied().unwrap_or(3.0))
            .collect();
        let up = smooth_curve(&upper, 4);
        let down = smooth_curve(&lower, 4);
        let mut shape = BezPath::new();
        for (k, (hz, v)) in up.iter().enumerate() {
            let p = pt(freq_x(plot, *hz), curve_y(plot, *v, range));
            if k == 0 {
                shape.move_to(p);
            } else {
                shape.line_to(p);
            }
        }
        for (hz, v) in down.iter().rev() {
            shape.line_to(pt(freq_x(plot, *hz), curve_y(plot, *v, range)));
        }
        shape.close_path();
        c.fill_with(shape, alpha(theme.io_connected, 0.2 * dim));
        let centre = smooth_curve(&view.target_bands, 4);
        let mut line = BezPath::new();
        for (k, (hz, v)) in centre.iter().enumerate() {
            let p = pt(freq_x(plot, *hz), curve_y(plot, *v, range));
            if k == 0 {
                line.move_to(p);
            } else {
                line.line_to(p);
            }
        }
        c.stroke_with(line, 1.2, alpha(theme.text, 0.55 * dim));
    }
    if view.ready && view.bands.len() >= 2 {
        let curve = smooth_curve(&view.bands, 4);
        let mut line = BezPath::new();
        for (k, (hz, v)) in curve.iter().enumerate() {
            let p = pt(freq_x(plot, *hz), curve_y(plot, *v, range));
            if k == 0 {
                line.move_to(p);
            } else {
                line.line_to(p);
            }
        }
        c.stroke_with(line, 1.8, alpha(accent, dim));
    }
    c.pop_clip();
    for (f, label) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        c.text(&TextRun::new(
            label,
            Point::new(freq_x(plot, f) + 2.0, plot.y1 - 2.0),
            6.5,
            theme.text_muted,
        ));
    }
}
