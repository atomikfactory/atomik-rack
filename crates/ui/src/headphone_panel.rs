//! The rack panel of `atomik.headphone-correction`: a 2 U panel built around a frequency graph.
//!
//! ```text
//! HEADPHONES                                        AMOUNT   OUTPUT   AUTO GAIN
//! [ file name   Frequency response, 120 points  ] [IMPORT][x]  (Amt)   (Out)   [o]  [-4.2 dB]
//! +----------------------------------------------+                              PREAMP
//! |  response / target / correction, 20 Hz-20 kHz |  TARGET     DATA IS       SIMULATE
//! |                                               |  [ Flat ]  [Measured resp.]  [o]
//! +----------------------------------------------+
//! MONITOR ONLY - bypass before export     --- RESPONSE  --- TARGET  --- CORRECTION
//! ```
//!
//! The graph runs 20 Hz to 20 kHz (log) with a dB grid whose range follows the curves (6, 12 or
//! 24 dB each way). It draws what [`atomik_devices::headphone_correction::curve::snapshot`] gives
//! for the device's data and settings: the headphone's own RESPONSE and the TARGET (only for a
//! measured response), and the CORRECTION the device applies (scaled by AMOUNT, inverted by
//! SIMULATE, limited as the device limits it), highlighted. With SIMULATE on the correction is
//! drawn in the warning colour of the SIMULATE lamp and the legend says SIMULATION: it is the
//! headphone's deviation, not a correction. The curves are computed when the data or a setting
//! changes ([`HpGraph::build`], kept per device by the rack view), never while painting.
//!
//! Without data the graph says "Import headphone data..." and a click on it runs the import
//! (`rack.headphone_import`, the file dialog). The IMPORT button does the same at any time;
//! the x button removes the data (`rack.headphone_remove`) and is dim without data. For
//! parametric data the TARGET and DATA IS selectors do not apply: they are drawn dimmed (they
//! still take a value, which is kept for the day a response is imported).
//!
//! The AUTO GAIN lamp is followed by the preamp the device applies (meter `preamp`, dB).
//! Everything else (AMOUNT, OUTPUT, TARGET, DATA IS, AUTO GAIN) is an ordinary placed control,
//! so the generic value, automation and MIDI machinery of the rack applies; SIMULATE is a toggle
//! painted here for its warning colour.
//!
//! The back is the audio in and the audio out.

use crate::autolayout::{
    CONTENT_X, FxPanel, MARGIN_TOP, PanelSpec, PlacedControl, PlacedSocket, SOCKET_CELL, Section,
    TITLE_H,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx::FxKind;
use crate::fx_paint::{lcd, polyline, small};
use crate::paint::{
    KNOB_DY, KNOB_RING_R, KNOB_START, KNOB_SWEEP, PanelInfo, button_sized, estimate_width,
    fit_label, fit_label_raw, solid, vgrad, with_alpha,
};
use crate::theme::{Theme, hsl, mix};
use crate::widget::current_value;
use atomik_device_sdk::headphone::{HeadphoneBody, HeadphoneData};
use atomik_device_sdk::{ControlKind, DeviceDescriptor};
use atomik_devices::headphone_correction::curve::{
    self, CurveSnapshot, DataKind, Settings, Target,
};
use atomik_model::RackDevice;
use vello::kurbo::{BezPath, Circle, Line, Point, Rect, RoundedRect, Stroke, Vec2};
use vello::peniko::Color;

/// The device type of this panel.
pub const HEADPHONE_TYPE: &str = "atomik.headphone-correction";
/// Height of the panel in rack units.
pub const HEADPHONE_UNITS: u8 = 2;

/// The meter values (`atomik_devices::headphone_correction::meter`; a test pins them together).
pub const METER_LOADED: usize = 0;
/// The preamp the filter applies, dB.
pub const METER_PREAMP: usize = 1;
/// The largest boost of the correction before the preamp, dB.
pub const METER_BOOST: usize = 2;
/// 1 while a filter is being built.
pub const METER_BUSY: usize = 3;
/// How many meter values the device publishes (`atomik_devices::headphone_correction::METERS`).
pub const HEADPHONE_METERS: usize = 4;

/// Points the graph's curves are sampled at.
pub const GRAPH_POINTS: usize = 288;
/// The sample rate the curves are drawn at (it only matters for parametric bands near Nyquist).
pub const DRAW_RATE: f32 = 48_000.0;
/// The dB ranges (each way) the graph picks from: the smallest that holds every curve.
pub const RANGES_DB: [f32; 3] = [6.0, 12.0, 24.0];
/// The frequencies of the vertical grid lines, Hz.
const GRID_HZ: [f64; 8] = [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0];

const LEFT_W: f64 = 396.0;
const KNOB_W: f64 = 48.0;
const KNOB_H: f64 = 58.0;
/// Content rows: the first under the top titles, the second under the titles of row B.
const ROW_B_TITLE: f64 = MARGIN_TOP + TITLE_H + KNOB_H + 2.0;

/// Geometry of the headphone panel's own parts (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadphoneGeom {
    /// The strip with the data's name and format.
    pub name: Rect,
    /// The IMPORT button.
    pub import: Rect,
    /// The REMOVE (x) button.
    pub remove: Rect,
    /// The frequency graph.
    pub plot: Rect,
    /// The line under the graph: the reminder on the left, the legend on the right.
    pub caption: Rect,
    /// The preamp readout.
    pub readout: Rect,
    /// The cell of the TARGET selector and the cell of the DATA IS selector.
    pub selectors: [Rect; 2],
    /// The cell of the SIMULATE toggle.
    pub simulate: Rect,
    /// The cell of the AMOUNT knob (the 100 % mark sits on its ring).
    pub amount: Rect,
}

/// Whether the control of descriptor index `param` is painted by this panel itself.
pub fn own_painted(desc: &DeviceDescriptor, param: usize) -> bool {
    desc.type_id == HEADPHONE_TYPE && desc.params.get(param).is_some_and(|p| p.key == "simulate")
}

/// The panel: 2 U with the layout of the module docs; the back has the four sockets.
pub fn headphone_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let x0 = CONTENT_X.0;
    let x1 = CONTENT_X.1;
    let index = |k: &str| desc.params.iter().position(|p| p.key == k);
    let c = MARGIN_TOP + TITLE_H;
    let rc = ROW_B_TITLE + TITLE_H;
    let mut front: Vec<PlacedControl> = Vec::new();
    let mut put = |key: &str, kind: ControlKind, rect: Rect| {
        if let Some(param) = index(key) {
            front.push(PlacedControl { param, kind, rect });
        }
    };
    let rx = x0 + LEFT_W + 12.0;
    let knob = |x: f64| Rect::new(x, c, x + KNOB_W, c + KNOB_H);
    let amount = knob(rx);
    put("amount", ControlKind::Knob, amount);
    put("output", ControlKind::Knob, knob(rx + 56.0));
    put(
        "auto_gain",
        ControlKind::Toggle,
        Rect::new(rx + 116.0, c + 12.0, rx + 160.0, c + 40.0),
    );
    let readout = Rect::new(rx + 166.0, c + 14.0, rx + 220.0, c + 27.0);
    let target = Rect::new(rx, rc, rx + 58.0, rc + 36.0);
    let file_kind = Rect::new(rx + 64.0, rc, rx + 156.0, rc + 36.0);
    let simulate = Rect::new(rx + 162.0, rc, rx + 220.0, rc + 28.0);
    put("target", ControlKind::Choice, target);
    put("file_kind", ControlKind::Choice, file_kind);
    put("simulate", ControlKind::Toggle, simulate);

    let remove = Rect::new(x0 + LEFT_W - 18.0, c, x0 + LEFT_W, c + 16.0);
    let import = Rect::new(remove.x0 - 4.0 - 54.0, c, remove.x0 - 4.0, c + 16.0);
    let name = Rect::new(x0, c, import.x0 - 4.0, c + 16.0);
    let plot = Rect::new(x0, c + 20.0, x0 + LEFT_W, c + 20.0 + 93.0);
    let caption = Rect::new(x0, plot.y1 + 3.0, x0 + LEFT_W, plot.y1 + 15.0);

    // The back: audio in, audio out.
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

    let title = |label: &str, a: f64, w: f64, y: f64| Section {
        label: label.to_string(),
        rect: Rect::new(a, y, a + w, y + TITLE_H),
    };
    let front_sections = vec![
        title("HEADPHONES", x0, LEFT_W, MARGIN_TOP),
        title("AMOUNT", rx, 54.0, MARGIN_TOP),
        title("OUTPUT", rx + 56.0, KNOB_W, MARGIN_TOP),
        title("AUTO GAIN", rx + 116.0, x1 - rx - 116.0, MARGIN_TOP),
        title("TARGET", rx, 58.0, ROW_B_TITLE),
        title("DATA IS", rx + 64.0, 92.0, ROW_B_TITLE),
        title("SIMULATE", rx + 162.0, x1 - rx - 162.0, ROW_B_TITLE),
    ];
    let back_sections = vec![
        title("AUDIO IN", x0, 110.0, MARGIN_TOP),
        title("AUDIO OUT", out_x, 110.0, MARGIN_TOP),
    ];
    let geom = HeadphoneGeom {
        name,
        import,
        remove,
        plot,
        caption,
        readout,
        selectors: [target, file_kind],
        simulate,
        amount,
    };
    PanelSpec {
        height_units: HEADPHONE_UNITS,
        samples: None,
        drum: None,
        slicer: None,
        fx: Some(FxPanel {
            kind: FxKind::HeadphoneCorrection,
            visual: plot,
            plot,
            meters: readout,
            readout,
            aux: Rect::ZERO,
            button: None,
            character: None,
            eq: None,
            stereo: None,
            gain: None,
            saturation: None,
            headphone: Some(geom),
            loudness: None,
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

// ------------------------------------------------------------------ the curves

/// What the device's data looks like, for the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HpFormat {
    /// A frequency response: its points, and whether it has a left and a right column.
    Response {
        /// Number of points.
        points: usize,
        /// Left and right columns.
        stereo: bool,
    },
    /// A parametric EQ text: its bands.
    Filters {
        /// Number of bands.
        bands: usize,
    },
}

impl HpFormat {
    /// The format of `data`.
    pub fn of(data: &HeadphoneData) -> Self {
        match &data.body {
            HeadphoneBody::Response(r) => Self::Response {
                points: r.freq.len(),
                stereo: r.right.is_some(),
            },
            HeadphoneBody::Filters(f) => Self::Filters {
                bands: f.filters.len(),
            },
        }
    }

    /// The text of the status line: the kind of data and its size, in the current language.
    pub fn text(self) -> String {
        use crate::i18n::{tn, tr};
        match self {
            Self::Response { points, stereo } => {
                let base = format!(
                    "{}, {}",
                    tr("Frequency response"),
                    tn(points as u64, "{n} point", "{n} points")
                );
                if stereo {
                    format!("{base}, {}", tr("left and right"))
                } else {
                    base
                }
            }
            Self::Filters { bands } => format!(
                "{}, {}",
                tr("Parametric EQ"),
                tn(bands as u64, "{n} band", "{n} bands")
            ),
        }
    }
}

/// The curves of one Headphone Correction device, computed once for its data and settings.
#[derive(Debug, Clone, PartialEq)]
pub struct HpGraph {
    /// What the curves were computed from: a fingerprint of the data and the settings' bits.
    key: (u64, [u32; 5]),
    /// The sampled curves ([`curve::snapshot`]).
    pub snap: CurveSnapshot,
    /// The dB range of the graph, each way.
    pub range_db: f32,
    /// The data is a measured response that is corrected towards a target: the RESPONSE and the
    /// TARGET curves are drawn.
    pub shows_response: bool,
    /// The correction is the inverse (SIMULATE).
    pub simulate: bool,
    /// The data's display name.
    pub name: String,
    /// The data's format.
    pub format: HpFormat,
}

/// The device's settings as the curve code reads them (the param values, defaults for the rest).
pub fn settings_of(desc: &DeviceDescriptor, device: &RackDevice) -> Settings {
    let value = |key: &str, default: f32| {
        desc.param_by_key(key)
            .map_or(default, |p| current_value(device, p))
    };
    Settings {
        kind: if value("file_kind", 0.0) < 0.5 {
            DataKind::Response
        } else {
            DataKind::Correction
        },
        target: if value("target", 0.0) < 0.5 {
            Target::Flat
        } else {
            Target::Studio
        },
        amount: value("amount", 1.0),
        simulate: value("simulate", 0.0) >= 0.5,
        auto_gain: value("auto_gain", 1.0) >= 0.5,
    }
}

/// A fingerprint of the data (FNV-1a over its name and every number).
fn fingerprint(data: &HeadphoneData) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    eat(data.name.as_bytes());
    match &data.body {
        HeadphoneBody::Response(r) => {
            eat(&[1]);
            for v in r.freq.iter().chain(&r.left).chain(r.right.iter().flatten()) {
                eat(&v.to_bits().to_le_bytes());
            }
            eat(&[u8::from(r.right.is_some())]);
        }
        HeadphoneBody::Filters(f) => {
            eat(&[2]);
            eat(&f.preamp_db.to_bits().to_le_bytes());
            for b in &f.filters {
                eat(b.kind.name().as_bytes());
                for v in [b.freq, b.gain_db, b.q] {
                    eat(&v.to_bits().to_le_bytes());
                }
            }
        }
    }
    h
}

fn settings_bits(s: &Settings) -> [u32; 5] {
    [
        u32::from(s.kind == DataKind::Correction),
        u32::from(s.target == Target::Studio),
        s.amount.to_bits(),
        u32::from(s.simulate),
        u32::from(s.auto_gain),
    ]
}

impl HpGraph {
    /// Computes the curves of `data` for `settings`.
    pub fn build(data: &HeadphoneData, settings: &Settings) -> Self {
        let snap = curve::snapshot(data, 0, settings, DRAW_RATE, GRAPH_POINTS);
        let shows_response =
            matches!(data.body, HeadphoneBody::Response(_)) && settings.kind == DataKind::Response;
        let mut peak = 0.0_f32;
        let mut look = |v: &[f32]| {
            for x in v {
                if x.is_finite() {
                    peak = peak.max(x.abs());
                }
            }
        };
        look(&snap.correction);
        if shows_response {
            look(&snap.response);
            look(&snap.target);
        }
        let range_db = RANGES_DB
            .iter()
            .copied()
            .find(|r| peak <= *r)
            .unwrap_or(RANGES_DB[RANGES_DB.len() - 1]);
        Self {
            key: (fingerprint(data), settings_bits(settings)),
            snap,
            range_db,
            shows_response,
            simulate: settings.simulate,
            name: data.name.clone(),
            format: HpFormat::of(data),
        }
    }

    /// Whether this graph was computed from `data` and `settings`.
    pub fn matches(&self, data: &HeadphoneData, settings: &Settings) -> bool {
        self.key == (fingerprint(data), settings_bits(settings))
    }
}

/// The graph of `device`: `prev` when it still fits the data and settings, else a new one;
/// `None` for a device without data. Called when the device changes, not while painting.
pub fn graph_of(
    desc: Option<&DeviceDescriptor>,
    device: &RackDevice,
    prev: Option<std::sync::Arc<HpGraph>>,
) -> Option<std::sync::Arc<HpGraph>> {
    let desc = desc?;
    let data = device.headphone.as_ref()?;
    let settings = settings_of(desc, device);
    match prev {
        Some(p) if p.matches(data, &settings) => Some(p),
        _ => Some(std::sync::Arc::new(HpGraph::build(data, &settings))),
    }
}

/// Whether the data is a parametric EQ text, which TARGET and DATA IS do not apply to.
pub fn is_parametric(device: Option<&RackDevice>) -> bool {
    device
        .and_then(|d| d.headphone.as_ref())
        .is_some_and(|h| matches!(h.body, HeadphoneBody::Filters(_)))
}

/// The x of frequency `hz` in `plot` (log, 20 Hz to 20 kHz).
pub fn x_of(plot: Rect, hz: f64) -> f64 {
    let t = (hz.max(20.0) / 20.0).log2() / (1000.0_f64).log2();
    plot.x0 + t.clamp(0.0, 1.0) * plot.width()
}

/// The y of `db` in `plot` for a graph of `range` dB each way.
pub fn y_of(plot: Rect, range: f32, db: f32) -> f64 {
    plot.center().y - f64::from(db / range) * plot.height() / 2.0
}

/// The text of the preamp readout.
pub fn preamp_text(loaded: bool, db: f32) -> String {
    if loaded && db.is_finite() {
        format!("{db:+.1} dB")
    } else {
        "-".to_string()
    }
}

// ------------------------------------------------------------------ painting

/// Small muted text that is already in the language (a name, a number, a composed line).
fn small_verbatim<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    text: impl Into<String>,
    at: Point,
    align: Align,
    color: Option<Color>,
) {
    c.text(
        &TextRun::verbatim(text, at, 7.5, color.unwrap_or(theme.text_muted))
            .align(align)
            .weight(600),
    );
}

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

fn meter(info: &PanelInfo<'_>, i: usize) -> f32 {
    info.fx_meters.get(i).copied().unwrap_or(0.0)
}

/// The colour of the SIMULATE lamp and of the correction while it is on.
pub fn warning() -> Color {
    hsl(30.0, 0.95, 0.55)
}

fn response_color() -> Color {
    hsl(222.0, 0.10, 0.66)
}

fn target_color() -> Color {
    hsl(48.0, 0.65, 0.72)
}

fn correction_color(theme: &Theme, accent: Color, simulate: bool) -> Color {
    let _ = theme;
    if simulate {
        warning()
    } else {
        mix(accent, Color::WHITE, 0.25)
    }
}

fn curve_path(plot: Rect, g: &HpGraph, ys: &[f32]) -> BezPath {
    let n = ys.len().max(2);
    polyline(ys.iter().enumerate().map(|(i, db)| {
        let x = plot.x0 + i as f64 / (n - 1) as f64 * plot.width();
        pt(x, y_of(plot, g.range_db, *db))
    }))
}

fn paint_grid<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, plot: Rect, range: f32) {
    let step = range / 2.0;
    for k in -2..=2 {
        let db = k as f32 * step;
        let y = y_of(plot, range, db).round() + 0.5;
        c.stroke_with(
            Line::new(pt(plot.x0, y), pt(plot.x1, y)),
            1.0,
            with_alpha(theme.ink, if k == 0 { 0.22 } else { 0.06 }),
        );
        let label = if db == 0.0 {
            "0".to_string()
        } else {
            format!("{db:+.0}")
        };
        small_verbatim(
            c,
            theme,
            label,
            pt(plot.x0 + 3.0, y - 2.0),
            Align::Start,
            None,
        );
    }
    for f in GRID_HZ {
        let x = x_of(plot, f).round() + 0.5;
        let major = matches!(f as u32, 100 | 1000 | 10_000);
        c.stroke_with(
            Line::new(pt(x, plot.y0), pt(x, plot.y1)),
            1.0,
            with_alpha(theme.ink, if major { 0.14 } else { 0.05 }),
        );
        if major {
            let label = match f as u32 {
                100 => "100",
                1000 => "1k",
                _ => "10k",
            };
            small_verbatim(
                c,
                theme,
                label,
                pt(x + 3.0, plot.y1 - 3.0),
                Align::Start,
                None,
            );
        }
    }
}

fn paint_curves<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    plot: Rect,
    g: &HpGraph,
    accent: Color,
) {
    if g.shows_response {
        c.stroke(
            &curve_path(plot, g, &g.snap.response).into(),
            &Stroke::new(1.0),
            &solid(with_alpha(response_color(), 0.85)),
        );
        c.stroke(
            &curve_path(plot, g, &g.snap.target).into(),
            &Stroke::new(1.0).with_dashes(0.0, [3.0, 3.0]),
            &solid(with_alpha(target_color(), 0.9)),
        );
    }
    let color = correction_color(theme, accent, g.simulate);
    let line = curve_path(plot, g, &g.snap.correction);
    let zero = y_of(plot, g.range_db, 0.0);
    let mut fill = line.clone();
    fill.line_to(pt(plot.x1, zero));
    fill.line_to(pt(plot.x0, zero));
    fill.close_path();
    c.fill_with(fill, with_alpha(color, 0.14));
    c.stroke(&line.into(), &Stroke::new(2.0), &solid(color));
}

/// One legend entry: a short line and its name; returns the x it ended at.
fn legend_entry<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    right: f64,
    y: f64,
    text: &str,
    color: Color,
    dashed: bool,
) -> f64 {
    let w = estimate_width(&crate::i18n::tr(text), 7.5) * 1.06 + 1.0;
    let x = right - w;
    small(c, theme, text, pt(x, y + 3.0), Align::Start, None);
    let stroke = if dashed {
        Stroke::new(1.5).with_dashes(0.0, [2.5, 2.0])
    } else {
        Stroke::new(2.0)
    };
    c.stroke(
        &Line::new(pt(x - 16.0, y), pt(x - 4.0, y)).into(),
        &stroke,
        &solid(color),
    );
    x - 32.0
}

fn paint_toggle<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    cell: Rect,
    on: bool,
    lit: Color,
    label: &str,
) {
    let cx = cell.center().x;
    let b = Rect::new(cx - 15.0, cell.y0 + 2.0, cx + 15.0, cell.y0 + 15.0);
    c.fill_with(
        RoundedRect::from_rect(b + Vec2::new(0.0, 1.0), 3.0),
        with_alpha(Color::BLACK, 0.5),
    );
    c.fill_with(
        RoundedRect::from_rect(b, 3.0),
        vgrad(
            b.y0,
            b.y1,
            mix(theme.button, Color::WHITE, 0.08),
            theme.button,
        ),
    );
    let led = pt(cx, b.center().y);
    if on {
        c.fill_with(Circle::new(led, 6.0), with_alpha(lit, 0.3));
        c.fill_with(Circle::new(led, 2.6), mix(lit, Color::WHITE, 0.25));
    } else {
        c.fill_with(Circle::new(led, 2.6), theme.led_off);
    }
    c.text(
        &TextRun::new(
            fit_label(label, cell.width() + 6.0, theme.label_size),
            pt(cx, cell.y0 + 25.0),
            theme.label_size,
            theme.text_muted,
        )
        .align(Align::Center),
    );
}

/// Paints the front: the name line with its buttons, the graph, the readout, the SIMULATE lamp,
/// the 100 % mark and the dimmed selectors.
pub fn paint_front<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: &FxPanel,
    info: &PanelInfo<'_>,
) {
    let Some(g) = fx.headphone else { return };
    let has_data = info.device.is_some_and(|d| d.headphone.is_some());
    let busy = meter(info, METER_BUSY) >= 0.5;

    // The name line.
    lcd(c, theme, g.name);
    let graph = info.headphone;
    match (graph, has_data) {
        (Some(gr), _) => {
            let max = g.name.width() * 0.5;
            let name = fit_label_raw(&gr.name, max, 8.5);
            let w = estimate_width(&name, 8.5);
            c.text(
                &TextRun::verbatim(
                    name,
                    pt(g.name.x0 + 6.0, g.name.y0 + 11.0),
                    8.5,
                    mix(info.accent, Color::WHITE, 0.3),
                )
                .weight(700),
            );
            let room = g.name.width() - 12.0 - w - 10.0 - if busy { 54.0 } else { 0.0 };
            let fmt = fit_label_raw(&gr.format.text(), room, 7.5);
            small_verbatim(
                c,
                theme,
                fmt,
                pt(g.name.x0 + 6.0 + w + 10.0, g.name.y0 + 10.5),
                Align::Start,
                None,
            );
        }
        _ => small(
            c,
            theme,
            "No headphone data",
            pt(g.name.x0 + 6.0, g.name.y0 + 10.5),
            Align::Start,
            None,
        ),
    }
    if busy {
        small(
            c,
            theme,
            "building...",
            pt(g.name.x1 - 6.0, g.name.y0 + 10.5),
            Align::End,
            Some(warning()),
        );
    }
    button_sized(c, theme, g.import, "IMPORT", true, 7.5, 0.5);
    button_sized(c, theme, g.remove, "\u{d7}", has_data, 10.0, 0.0);

    // The graph.
    lcd(c, theme, g.plot);
    c.push_clip(&g.plot.into());
    match graph {
        Some(gr) => {
            paint_grid(c, theme, g.plot, gr.range_db);
            paint_curves(c, theme, g.plot, gr, info.accent);
        }
        None => {
            paint_grid(c, theme, g.plot, RANGES_DB[1]);
            let mid = g.plot.center();
            c.text(
                &TextRun::new(
                    "Import headphone data...",
                    pt(mid.x, mid.y - 2.0),
                    11.0,
                    mix(info.accent, Color::WHITE, 0.3),
                )
                .weight(700)
                .align(Align::Center),
            );
            small(
                c,
                theme,
                "a frequency response or a parametric EQ text",
                pt(mid.x, mid.y + 12.0),
                Align::Center,
                None,
            );
        }
    }
    c.pop_clip();

    // The caption and the legend.
    small(
        c,
        theme,
        "MONITOR ONLY \u{2014} bypass before export",
        pt(g.caption.x0 + 1.0, g.caption.y0 + 8.5),
        Align::Start,
        Some(with_alpha(warning(), 0.9)),
    );
    if let Some(gr) = graph {
        let y = g.caption.y0 + 5.5;
        let mut right = g.caption.x1 - 2.0;
        let label = if gr.simulate {
            "SIMULATION"
        } else {
            "CORRECTION"
        };
        right = legend_entry(
            c,
            theme,
            right,
            y,
            label,
            correction_color(theme, info.accent, gr.simulate),
            false,
        );
        if gr.shows_response {
            right = legend_entry(c, theme, right, y, "TARGET", target_color(), true);
            legend_entry(c, theme, right, y, "RESPONSE", response_color(), false);
        }
    }

    // The preamp readout.
    lcd(c, theme, g.readout);
    let loaded = meter(info, METER_LOADED) >= 0.5;
    c.text(
        &TextRun::verbatim(
            preamp_text(loaded, meter(info, METER_PREAMP)),
            pt(g.readout.center().x, g.readout.y0 + 9.5),
            8.0,
            mix(info.accent, Color::WHITE, 0.3),
        )
        .weight(600)
        .align(Align::Center),
    );
    small(
        c,
        theme,
        "PREAMP",
        pt(g.readout.center().x, g.readout.y1 + 11.0),
        Align::Center,
        None,
    );

    // The SIMULATE lamp, in the warning colour.
    let sim = value(info, "simulate") >= 0.5;
    paint_toggle(c, theme, g.simulate, sim, warning(), "Simulate");

    // The 100 % mark of the AMOUNT knob: straight up, outside the ring.
    let at = pt(g.amount.center().x, g.amount.y0 + KNOB_DY);
    let a = KNOB_START + 0.5 * KNOB_SWEEP;
    let dir = Vec2::new(a.cos(), a.sin());
    c.stroke_with(
        Line::new(
            at + dir * (KNOB_RING_R + 2.0),
            at + dir * (KNOB_RING_R + 5.0),
        ),
        1.4,
        with_alpha(theme.text, 0.8),
    );

    // The selectors do not apply to a parametric file.
    if is_parametric(info.device) {
        for r in g.selectors {
            c.fill_with(
                RoundedRect::from_rect(r.inflate(1.0, 1.0), 3.0),
                with_alpha(Color::BLACK, 0.45),
            );
        }
    }
}
