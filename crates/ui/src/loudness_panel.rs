//! The rack panel of `atomik.loudness-meter`: a 4 U strip with the loudness as big numbers, a
//! history graph, a distribution and a pass or fail line against the chosen target.
//!
//! ```text
//! LOUDNESS               RANGE AND PEAKS        TARGET
//! INTEGRATED             LRA        TRUE PEAK   -14.0 LUFS  /  -1.0 dBTP
//! -14.2 LUFS             PLR        PSR         LOUDNESS   ON TARGET
//! SHORT-TERM  MOMENTARY  DR         PEAK        TRUE PEAK  OK
//! HISTORY (click or scroll: span)               DISTRIBUTION
//! +-----------------------------------------+   +------------------+
//! | short-term line, momentary (faint),     |   | bars of the      |
//! | the target line                         |   | short-term level |
//! +-----------------------------------------+   +------------------+
//! TARGET PRESET          CUSTOM TARGET          METER        PROGRAMME TIME
//! [ Streaming -14 ... ]  (LUFS) (TP max)        [RESET][HOLD]  01:23
//! ```
//!
//! * **Readouts.** The integrated loudness is the biggest number, short-term and momentary
//!   under it; the range, the true peak, PLR, PSR, DR and the sample peak are in the middle box
//!   (the definitions are the device's, see `atomik_devices::loudness_meter`). `--` stands for
//!   "nothing measured yet".
//! * **Target.** The right box shows the target in force (the preset's, or the Custom knobs')
//!   and two lines: the loudness is **on target** (green) within 1 LU of it, **too loud** or
//!   **too quiet** (amber, with the distance) outside that, and the true peak is **OK** (green)
//!   or **FAIL** (red, with the overshoot) when it is above the limit.
//! * **History.** The short-term loudness as a line and the momentary loudness fainter, over
//!   the span of the `history_span` choice, with a line at the target; the graph is the control
//!   of that choice, so a click steps to the next span (a right click to the previous one) and
//!   the wheel steps too. The newest value is at the right edge. 36 LU are drawn, 10 above the
//!   target.
//! * **Distribution.** One bar per 1 LU from -46 to 0 LUFS, as tall as that level was common among
//!   the short-term values (the tallest fills the box), a line at the target and a mark at the
//!   short-term loudness now.
//! * **Controls.** PRESET, the two CUSTOM knobs (they matter with the preset Custom), RESET and
//!   HOLD are ordinary placed controls, so the generic value, automation and MIDI machinery
//!   applies. RESET is momentary: it stores nothing, the app sends the device 1 and then 0
//!   ([`crate::UiAction::PulseParam`]).
//!
//! The back is the audio in and the audio out.

use crate::autolayout::{
    CONTENT_X, FxPanel, MARGIN_TOP, PanelSpec, PlacedControl, PlacedSocket, SOCKET_CELL, Section,
    TITLE_H,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx::{FxKind, loudness as slots};
use crate::fx_paint::{lcd, small};
use crate::paint::{PanelInfo, with_alpha};
use crate::theme::{Theme, mix};
use crate::widget::current_value;
use atomik_device_sdk::{ControlKind, DeviceDescriptor};
use vello::kurbo::{BezPath, Line, Rect};
use vello::peniko::Color;

/// The device type of this panel.
pub const LOUDNESS_TYPE: &str = "atomik.loudness-meter";
/// Height of the panel in rack units.
pub const LOUDNESS_UNITS: u8 = 4;
/// The key of the momentary RESET param.
pub const RESET_KEY: &str = "reset";
/// The key of the span choice, which the history graph is the control of.
pub const SPAN_KEY: &str = "history_span";

/// Seconds of each `history_span` choice (`atomik_devices::loudness_meter::SPAN_SECONDS`; a test
/// pins them together).
pub const SPAN_SECONDS: [f32; 4] = [30.0, 60.0, 300.0, 900.0];
/// The names of the spans.
pub const SPAN_NAMES: [&str; 4] = ["30 s", "1 min", "5 min", "15 min"];
/// History points per second (one every half second).
pub const POINTS_PER_SECOND: f32 = 2.0;
/// The tolerance of "on target", LU (`atomik_devices::loudness_meter::TOLERANCE_LU`).
pub const TOLERANCE_LU: f32 = 1.0;
/// LU drawn above the target line in the history graph.
pub const GRAPH_ABOVE_LU: f32 = 10.0;
/// LU drawn from the top to the bottom of the history graph.
pub const GRAPH_RANGE_LU: f32 = 36.0;
/// Readings at or below this are "nothing yet".
pub const NONE_DB: f32 = -119.5;
/// The loudness an old value must exceed to be drawn (the absolute gate).
pub const GATE_LUFS: f32 = -70.0;

const KNOB_W: f64 = 48.0;
const KNOB_H: f64 = 58.0;
const TOGGLE: (f64, f64) = (44.0, 28.0);

/// The rows of the panel: title top.
const ROW_Y: [f64; 3] = [MARGIN_TOP, MARGIN_TOP + 116.0, MARGIN_TOP + 222.0];

/// Geometry of the loudness meter's own parts (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessGeom {
    /// The big readouts: integrated, short-term and momentary.
    pub loud: Rect,
    /// The range, peaks and ratios.
    pub stats: Rect,
    /// The target and the two pass or fail lines.
    pub verdict: Rect,
    /// The history graph (also the control rect of the span choice).
    pub history: Rect,
    /// The distribution graph.
    pub dist: Rect,
    /// The programme time readout.
    pub time: Rect,
}

impl LoudnessGeom {
    /// The plot inside the history graph's rect (room left for the scale, under for the time).
    pub fn history_plot(&self) -> Rect {
        let r = self.history;
        Rect::new(r.x0 + 26.0, r.y0 + 12.0, r.x1 - 5.0, r.y1 - 11.0)
    }

    /// The plot inside the distribution's rect.
    pub fn dist_plot(&self) -> Rect {
        let r = self.dist;
        Rect::new(r.x0 + 5.0, r.y0 + 12.0, r.x1 - 5.0, r.y1 - 11.0)
    }
}

/// Whether the front control of `key` is painted by this panel instead of the generic cell.
pub fn custom_painted(key: &str) -> bool {
    key == SPAN_KEY
}

/// The text of a jack on the back.
pub fn socket_label(_key: &str) -> Option<&'static str> {
    None
}

/// The panel: 4 U with the layout of the module docs; the back has the four audio sockets.
pub fn loudness_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let x0 = CONTENT_X.0;
    let x1 = CONTENT_X.1;
    let index = |k: &str| desc.params.iter().position(|p| p.key == k);
    let c = |row: usize| ROW_Y[row] + TITLE_H;
    let mut front: Vec<PlacedControl> = Vec::new();
    let mut put = |key: &str, kind: ControlKind, rect: Rect| {
        if let Some(param) = index(key) {
            front.push(PlacedControl { param, kind, rect });
        }
    };

    // Row 1: the readouts.
    let loud = Rect::new(x0, c(0), x0 + 212.0, c(0) + 100.0);
    let stats = Rect::new(loud.x1 + 8.0, c(0), loud.x1 + 208.0, c(0) + 100.0);
    let verdict = Rect::new(stats.x1 + 8.0, c(0), x1, c(0) + 100.0);

    // Row 2: the graphs.
    let history = Rect::new(x0, c(1), stats.x1, c(1) + 90.0);
    let dist = Rect::new(verdict.x0, c(1), x1, c(1) + 90.0);
    put(SPAN_KEY, ControlKind::Choice, history);

    // Row 3: the controls.
    let y = c(2);
    let preset_w = 220.0;
    put(
        "preset",
        ControlKind::Choice,
        Rect::new(x0, y + 10.0, x0 + preset_w, y + 46.0),
    );
    let custom_x = x0 + preset_w + 16.0;
    put(
        "target_lufs",
        ControlKind::Knob,
        Rect::new(custom_x, y, custom_x + KNOB_W, y + KNOB_H),
    );
    put(
        "max_tp",
        ControlKind::Knob,
        Rect::new(custom_x + 56.0, y, custom_x + 56.0 + KNOB_W, y + KNOB_H),
    );
    let meter_x = custom_x + 128.0;
    // RESET is wider than HOLD: its French label is the longer.
    let (reset_w, hold_w) = (64.0, 48.0);
    put(
        RESET_KEY,
        ControlKind::Toggle,
        Rect::new(meter_x, y + 12.0, meter_x + reset_w, y + 12.0 + TOGGLE.1),
    );
    let hold_x = meter_x + reset_w + 8.0;
    put(
        "hold",
        ControlKind::Toggle,
        Rect::new(hold_x, y + 12.0, hold_x + hold_w, y + 12.0 + TOGGLE.1),
    );
    let time_x = hold_x + hold_w + 20.0;
    let time = Rect::new(time_x, y + 12.0, x1, y + 12.0 + 32.0);

    // The back: audio in and audio out.
    let socket = |k: &str| desc.sockets.iter().position(|s| s.key == k);
    let jack_y = MARGIN_TOP + TITLE_H + 6.0;
    let jack = |x: f64| Rect::new(x, jack_y, x + SOCKET_CELL.0, jack_y + SOCKET_CELL.1);
    let mut back = Vec::new();
    let mut place = |k: &str, x: f64| {
        if let Some(s) = socket(k) {
            back.push(PlacedSocket {
                socket: s,
                rect: jack(x),
            });
        }
    };
    let out_x = x1 - 110.0;
    place("in_l", x0);
    place("in_r", x0 + 60.0);
    place("out_l", out_x);
    place("out_r", out_x + 60.0);

    let title = |label: &str, a: f64, b: f64, row: usize| Section {
        label: label.to_string(),
        rect: Rect::new(a, ROW_Y[row], b, ROW_Y[row] + TITLE_H),
    };
    let front_sections = vec![
        title("LOUDNESS", loud.x0, loud.x1, 0),
        title("RANGE AND PEAKS", stats.x0, stats.x1, 0),
        title("TARGET", verdict.x0, verdict.x1, 0),
        title("HISTORY", history.x0, history.x1, 1),
        title("DISTRIBUTION", dist.x0, dist.x1, 1),
        title("TARGET PRESET", x0, x0 + preset_w, 2),
        title("CUSTOM TARGET", custom_x, custom_x + 104.0, 2),
        title("METER", meter_x, hold_x + hold_w, 2),
        title("PROGRAMME TIME", time.x0, time.x1, 2),
    ];
    let back_sections = vec![
        title("AUDIO IN", x0, x0 + 110.0, 0),
        title("AUDIO OUT", out_x, out_x + 110.0, 0),
    ];
    let geom = LoudnessGeom {
        loud,
        stats,
        verdict,
        history,
        dist,
        time,
    };
    PanelSpec {
        height_units: LOUDNESS_UNITS,
        samples: None,
        drum: None,
        slicer: None,
        fx: Some(FxPanel {
            kind: FxKind::LoudnessMeter,
            visual: loud,
            plot: loud,
            meters: loud,
            readout: loud,
            aux: Rect::ZERO,
            button: None,
            character: None,
            eq: None,
            stereo: None,
            gain: None,
            saturation: None,
            loudness: Some(geom),
            headphone: None,
        }),
        seq: None,
        drum_gen: None,
        arp: None,
        key_chord: None,
        scale_strip: None,
        chord_wheel: None,
        modsrc: None,
        wavetable: None,
        fm: None,
        granular: None,
        container: None,
        plugin: None,
        front,
        front_sections,
        back,
        back_sections,
    }
}

// ------------------------------------------------------------------ the texts and the maths

/// A level in LUFS, dBTP or dB as the readouts show it: one decimal, `--` for nothing yet.
pub fn level_text(v: f32) -> String {
    if v.is_finite() && v > NONE_DB {
        format!("{:.1}", v + 0.0)
    } else {
        "--".to_string()
    }
}

/// A value with its unit, or `--` alone.
pub fn with_unit(v: f32, unit: &str) -> String {
    if v.is_finite() && v > NONE_DB {
        format!("{} {unit}", level_text(v))
    } else {
        "--".to_string()
    }
}

/// The programme time as `mm:ss`, or `h:mm:ss` from an hour.
pub fn time_text(seconds: f32) -> String {
    let s = if seconds.is_finite() {
        seconds.max(0.0) as u64
    } else {
        0
    };
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// How a line of the verdict is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Nothing to judge yet.
    None,
    /// Within the target.
    Ok,
    /// Outside the target, but not a failure: too loud or too quiet.
    Warning,
    /// A failure: the true peak is over its limit.
    Error,
}

/// The loudness line of the verdict (the device's `verdict`): the word and its tone, with the
/// distance from the target in LU for the two ways out.
pub fn loudness_verdict(integrated: f32, target: f32) -> (String, Tone) {
    if !integrated.is_finite() || integrated <= NONE_DB {
        return ("--".to_string(), Tone::None);
    }
    let d = integrated - target;
    if d > TOLERANCE_LU + 1e-4 {
        (
            format!("{} +{d:.1} LU", crate::i18n::tr("TOO LOUD")),
            Tone::Warning,
        )
    } else if d < -TOLERANCE_LU - 1e-4 {
        (
            format!("{} {d:.1} LU", crate::i18n::tr("TOO QUIET")),
            Tone::Warning,
        )
    } else {
        (crate::i18n::tr("ON TARGET").into_owned(), Tone::Ok)
    }
}

/// The true-peak line of the verdict (the device's `peak_passes`): `OK`, or `FAIL` with the
/// overshoot in dB.
pub fn peak_verdict(true_peak: f32, max_tp: f32) -> (String, Tone) {
    if !true_peak.is_finite() || true_peak <= NONE_DB {
        return ("--".to_string(), Tone::None);
    }
    let over = true_peak - max_tp;
    if over > 1e-4 {
        (
            format!("{} +{over:.1} dB", crate::i18n::tr("FAIL")),
            Tone::Error,
        )
    } else {
        (crate::i18n::tr("OK").into_owned(), Tone::Ok)
    }
}

/// The colour of a tone.
pub fn tone_color(theme: &Theme, tone: Tone) -> Color {
    match tone {
        Tone::None => theme.text_muted,
        Tone::Ok => theme.meter_low,
        Tone::Warning => theme.warning,
        Tone::Error => theme.clip,
    }
}

/// History points the span choice `span` shows.
pub fn span_points(span: f32) -> usize {
    let i = (span.round().max(0.0) as usize).min(SPAN_SECONDS.len() - 1);
    (SPAN_SECONDS[i] * POINTS_PER_SECOND) as usize
}

/// The name of the span choice `span`.
pub fn span_name(span: f32) -> &'static str {
    SPAN_NAMES[(span.round().max(0.0) as usize).min(SPAN_NAMES.len() - 1)]
}

/// The y of a loudness in the history plot `plot` for a graph centred on `target`: the top is
/// [`GRAPH_ABOVE_LU`] above it, the bottom [`GRAPH_RANGE_LU`] below the top; values outside are
/// drawn on the edge.
pub fn graph_y(plot: Rect, lufs: f32, target: f32) -> f64 {
    let top = target + GRAPH_ABOVE_LU;
    let t = f64::from(((top - lufs) / GRAPH_RANGE_LU).clamp(0.0, 1.0));
    plot.y0 + t * plot.height()
}

/// The runs of drawable points of a series (newest last) in the history plot: x runs from the
/// right edge back by one step per point, and a value at or below the absolute gate (silence)
/// breaks the line. At most `want` of the newest points are used, spread over the whole width,
/// so a longer history shows only its newest span and a shorter one starts partway across.
pub fn graph_runs(plot: Rect, series: &[f32], want: usize, target: f32) -> Vec<Vec<(f64, f64)>> {
    let used = &series[series.len().saturating_sub(want)..];
    let step = if want > 1 {
        plot.width() / (want - 1) as f64
    } else {
        0.0
    };
    let mut runs: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut open = false;
    for (i, v) in used.iter().enumerate() {
        if !v.is_finite() || *v <= GATE_LUFS {
            open = false;
            continue;
        }
        let x = plot.x1 - (used.len() - 1 - i) as f64 * step;
        if !open {
            runs.push(Vec::new());
            open = true;
        }
        if let Some(run) = runs.last_mut() {
            run.push((x, graph_y(plot, *v, target)));
        }
    }
    runs
}

/// The x of a loudness along the distribution plot `plot`.
pub fn dist_x(plot: Rect, lufs: f32) -> f64 {
    let t = f64::from(((lufs - slots::DIST_LOW) / slots::BIN_COUNT as f32).clamp(0.0, 1.0));
    plot.x0 + t * plot.width()
}

// ------------------------------------------------------------------ painting

fn value(info: &PanelInfo<'_>, key: &str) -> f32 {
    let Some(desc) = info.desc else { return 0.0 };
    desc.params
        .iter()
        .find(|p| p.key == key)
        .map_or(0.0, |p| match info.device {
            Some(d) => current_value(d, p),
            None => p.default,
        })
}

fn meter(info: &PanelInfo<'_>, i: usize, default: f32) -> f32 {
    info.fx_meters.get(i).copied().unwrap_or(default)
}

fn text<C: Canvas + ?Sized>(
    c: &mut C,
    s: impl Into<String>,
    at: vello::kurbo::Point,
    size: f32,
    color: Color,
    align: Align,
) {
    c.text(&TextRun::new(s, at, size, color).align(align).weight(700));
}

/// Paints the front: the readouts, the two graphs and the verdict.
pub fn paint_front<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: &FxPanel,
    info: &PanelInfo<'_>,
) {
    let Some(g) = fx.loudness else { return };
    let lit = mix(info.accent, Color::WHITE, 0.3);
    let target = meter(info, slots::TARGET, -14.0);
    let max_tp = meter(info, slots::MAX_TP, -1.0);
    let integrated = meter(info, slots::INTEGRATED, -120.0);
    let true_peak = meter(info, slots::TRUE_PEAK, -120.0);

    // The big readouts.
    lcd(c, theme, g.loud);
    let r = g.loud;
    small(
        c,
        theme,
        crate::i18n::tr("INTEGRATED"),
        pt(r.x0 + 8.0, r.y0 + 12.0),
        Align::Start,
        None,
    );
    text(
        c,
        level_text(integrated),
        pt(r.x0 + 8.0, r.y0 + 47.0),
        32.0,
        lit,
        Align::Start,
    );
    small(
        c,
        theme,
        "LUFS",
        pt(r.x1 - 8.0, r.y0 + 47.0),
        Align::End,
        None,
    );
    for (k, (label, i)) in [
        ("SHORT-TERM", slots::SHORT_TERM),
        ("MOMENTARY", slots::MOMENTARY),
    ]
    .into_iter()
    .enumerate()
    {
        let x = r.x0 + 8.0 + k as f64 * 100.0;
        small(
            c,
            theme,
            crate::i18n::tr(label),
            pt(x, r.y0 + 66.0),
            Align::Start,
            None,
        );
        text(
            c,
            level_text(meter(info, i, -120.0)),
            pt(x, r.y0 + 88.0),
            17.0,
            theme.text,
            Align::Start,
        );
    }

    // The range, the peaks and the ratios.
    lcd(c, theme, g.stats);
    let r = g.stats;
    let peak_tone = peak_verdict(true_peak, max_tp).1;
    let cells = [
        (
            "LRA",
            with_unit(meter(info, slots::LRA, -120.0), "LU"),
            Tone::None,
        ),
        ("TRUE PEAK", with_unit(true_peak, "dBTP"), peak_tone),
        (
            "PLR",
            with_unit(meter(info, slots::PLR, -120.0), "dB"),
            Tone::None,
        ),
        (
            "PSR",
            with_unit(meter(info, slots::PSR, -120.0), "dB"),
            Tone::None,
        ),
        (
            "DR",
            with_unit(meter(info, slots::DR, -120.0), "dB"),
            Tone::None,
        ),
        (
            "PEAK",
            with_unit(meter(info, slots::SAMPLE_PEAK, -120.0), "dBFS"),
            Tone::None,
        ),
    ];
    for (k, (label, v, tone)) in cells.iter().enumerate() {
        let (col, row) = ((k % 2) as f64, (k / 2) as f64);
        let (x, y) = (r.x0 + 8.0 + col * 98.0, r.y0 + 8.0 + row * 31.0);
        small(
            c,
            theme,
            crate::i18n::tr(label),
            pt(x, y + 7.0),
            Align::Start,
            None,
        );
        let color = if *tone == Tone::None {
            theme.text
        } else {
            tone_color(theme, *tone)
        };
        text(c, v.clone(), pt(x, y + 23.0), 12.5, color, Align::Start);
    }

    // The target and the verdict.
    lcd(c, theme, g.verdict);
    let r = g.verdict;
    small(
        c,
        theme,
        crate::i18n::tr("TARGET"),
        pt(r.x0 + 8.0, r.y0 + 14.0),
        Align::Start,
        None,
    );
    text(
        c,
        format!("{target:.1} LUFS  /  {max_tp:.1} dBTP"),
        pt(r.x0 + 8.0, r.y0 + 30.0),
        11.5,
        lit,
        Align::Start,
    );
    let (word, tone) = loudness_verdict(integrated, target);
    small(
        c,
        theme,
        crate::i18n::tr("LOUDNESS"),
        pt(r.x0 + 8.0, r.y0 + 50.0),
        Align::Start,
        None,
    );
    text(
        c,
        word,
        pt(r.x0 + 8.0, r.y0 + 66.0),
        12.5,
        tone_color(theme, tone),
        Align::Start,
    );
    let (word, tone) = peak_verdict(true_peak, max_tp);
    small(
        c,
        theme,
        crate::i18n::tr("TRUE PEAK"),
        pt(r.x0 + 8.0, r.y0 + 80.0),
        Align::Start,
        None,
    );
    text(
        c,
        word,
        pt(r.x0 + 8.0, r.y0 + 95.0),
        12.5,
        tone_color(theme, tone),
        Align::Start,
    );

    paint_history(c, theme, &g, info, target);
    paint_dist(c, theme, &g, info, target);

    // The programme time.
    lcd(c, theme, g.time);
    let held = value(info, "hold") >= 0.5;
    text(
        c,
        time_text(meter(info, slots::TIME, 0.0)),
        pt(g.time.x0 + 10.0, g.time.center().y + 5.5),
        15.0,
        if held { theme.warning } else { lit },
        Align::Start,
    );
    if held {
        small(
            c,
            theme,
            crate::i18n::tr("HOLD"),
            pt(g.time.x1 - 8.0, g.time.center().y + 3.0),
            Align::End,
            Some(theme.warning),
        );
    }
}

/// The history graph: scale, grid, target line, the two series and the span.
fn paint_history<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    g: &LoudnessGeom,
    info: &PanelInfo<'_>,
    target: f32,
) {
    let plot = g.history_plot();
    let span = value(info, SPAN_KEY);
    c.fill_with(plot, with_alpha(theme.ink, 0.04));
    c.push_clip(&plot.into());
    // A line every 6 LU from the target (the target's own is drawn over the data, in orange).
    for offset in [-24.0f32, -18.0, -12.0, -6.0, 6.0] {
        let y = graph_y(plot, target + offset, target).round() + 0.5;
        c.stroke_with(
            Line::new(pt(plot.x0, y), pt(plot.x1, y)),
            1.0,
            with_alpha(theme.ink, 0.06),
        );
    }
    let want = span_points(span);
    let draw = |c: &mut C, series: &[f32], color: Color, width: f64| {
        for run in graph_runs(plot, series, want, target) {
            if let [(x, y)] = run[..] {
                c.fill_with(Rect::new(x - 1.0, y - 1.0, x + 1.0, y + 1.0), color);
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
            c.stroke_with(path, width, color);
        }
    };
    let data = info.analyser;
    draw(
        c,
        &data.loud_momentary,
        with_alpha(mix(info.accent, theme.ink, 0.2), 0.35),
        1.0,
    );
    draw(
        c,
        &data.loud_short,
        mix(info.accent, Color::WHITE, 0.3),
        1.6,
    );
    let ty = graph_y(plot, target, target).round() + 0.5;
    c.stroke_with(
        Line::new(pt(plot.x0, ty), pt(plot.x1, ty)),
        1.0,
        with_alpha(theme.warning, 0.9),
    );
    c.pop_clip();
    // The scale: the target in orange, a number at +6 and every 12 LU below it.
    for offset in [6.0f32, 0.0, -12.0, -24.0] {
        let v = target + offset;
        let color = (offset == 0.0).then(|| with_alpha(theme.warning, 1.0));
        small(
            c,
            theme,
            format!("{v:.0}"),
            pt(plot.x0 - 3.0, graph_y(plot, v, target) + 2.5),
            Align::End,
            color,
        );
    }
    // The span and the key to the lines.
    let r = g.history;
    small(
        c,
        theme,
        format!("-{}", span_name(span)),
        pt(plot.x0, plot.y1 + 9.0),
        Align::Start,
        None,
    );
    small(
        c,
        theme,
        crate::i18n::tr("now"),
        pt(plot.x1, plot.y1 + 9.0),
        Align::End,
        None,
    );
    small(
        c,
        theme,
        format!("{}  {}", crate::i18n::tr("SPAN"), span_name(span)),
        pt(r.x1 - 5.0, r.y0 + 8.0),
        Align::End,
        None,
    );
    small(
        c,
        theme,
        crate::i18n::tr("SHORT-TERM"),
        pt(plot.x0 + 14.0, r.y0 + 8.0),
        Align::Start,
        Some(mix(info.accent, Color::WHITE, 0.3)),
    );
    small(
        c,
        theme,
        crate::i18n::tr("MOMENTARY"),
        pt(plot.x0 + 80.0, r.y0 + 8.0),
        Align::Start,
        Some(with_alpha(mix(info.accent, theme.ink, 0.2), 0.7)),
    );
    if data.loud_short.is_empty() {
        small(
            c,
            theme,
            crate::i18n::tr("collecting..."),
            plot.center(),
            Align::Center,
            None,
        );
    }
}

/// The distribution: one bar per 1 LU, the target line and the short-term mark.
fn paint_dist<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    g: &LoudnessGeom,
    info: &PanelInfo<'_>,
    target: f32,
) {
    let plot = g.dist_plot();
    c.fill_with(plot, with_alpha(theme.ink, 0.04));
    let w = plot.width() / slots::BIN_COUNT as f64;
    for i in 0..slots::BIN_COUNT {
        let share = meter(info, slots::BINS + i, 0.0).clamp(0.0, 1.0);
        if share <= 0.0 {
            continue;
        }
        let h = (plot.height() * f64::from(share)).round().max(1.0);
        let x = plot.x0 + i as f64 * w;
        c.fill_with(
            Rect::new(x + 0.5, plot.y1 - h, x + w - 0.5, plot.y1),
            with_alpha(mix(info.accent, Color::WHITE, 0.2), 0.8),
        );
    }
    for v in [-40.0f32, -30.0, -20.0, -10.0, 0.0] {
        let x = dist_x(plot, v).round() + 0.5;
        c.stroke_with(
            Line::new(pt(x, plot.y1), pt(x, plot.y1 + 3.0)),
            1.0,
            with_alpha(theme.text_muted, 0.7),
        );
        small(
            c,
            theme,
            format!("{v:.0}"),
            pt(x.clamp(plot.x0 + 6.0, plot.x1 - 3.0), plot.y1 + 10.0),
            Align::Center,
            None,
        );
    }
    let tx = dist_x(plot, target).round() + 0.5;
    c.stroke_with(
        Line::new(pt(tx, plot.y0), pt(tx, plot.y1)),
        1.0,
        with_alpha(theme.warning, 0.9),
    );
    small(
        c,
        theme,
        format!("{target:.0}"),
        pt(tx, g.dist.y0 + 8.0),
        Align::Center,
        Some(theme.warning),
    );
    let short = meter(info, slots::SHORT_TERM, -120.0);
    if short > NONE_DB {
        let x = dist_x(plot, short);
        let mut mark = BezPath::new();
        mark.move_to(pt(x - 3.0, plot.y0));
        mark.line_to(pt(x + 3.0, plot.y0));
        mark.line_to(pt(x, plot.y0 + 5.0));
        mark.close_path();
        c.fill_with(mark, mix(info.accent, Color::WHITE, 0.5));
    }
}
