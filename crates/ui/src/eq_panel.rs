//! The rack panel of `atomik.eq`: a 7 U panel with a big response graph across the top, one
//! control strip per band and a column for the output, the stereo mode and the display.
//!
//! ```text
//! RESPONSE
//! +-----------------------------------------------------------------------------------+
//! |  spectrum of the output, a tint per band, the summed curve, a handle per band      |
//! +-----------------------------------------------------------------------------------+
//!  20   50  100  200  500  1k   2k   5k  10k  20k
//!  LCF    1     2     3     4     5    HCF     OUTPUT
//!  [ON]  [ON]  [ON]  [ON]  [ON]  [ON]  [ON]    (Output)
//!  Slope Shape Shape Shape Shape Shape Slope    L R meters
//!  (Freq)(Freq)(Freq)(Freq)(Freq)(Freq)(Freq)   Stereo mode
//!        (Gain)(Gain)(Gain)(Gain)(Gain)         [Spectrum]
//!  (Reso)( Q  )( Q  )( Q  )( Q  )( Q  )(Reso)   Range
//! ```
//!
//! # Bands
//!
//! The strips run LCF, 1, 2, 3, 4, 5, HCF (UI index 0..7); the params keep the numbers they had
//! before the device grew a seventh band, so the UI order is bands 1, 2, 3, 4, 7, 5, 6
//! ([`BAND_NUMBER`], [`KEYS`]). The two cuts have a frequency, a resonance and a slope; the five
//! middle bands a frequency, a gain, a Q and a shape (bell, low shelf, high shelf, notch).
//!
//! # The graph
//!
//! The curve is computed with the sections the device designs ([`middle_response_db`],
//! [`cut_response_db`], the same maths on [`atomik_dsp::BiquadCoeffs`]; a test pins them against
//! `atomik_devices::eq`), so it matches the sound for every shape and slope. Each active band is
//! tinted in its colour, the sum is drawn over it, and the spectrum of the output (the app's FFT
//! of the device's scope tap, [`crate::analyser::AnalyserData::trace`]) sits behind them while
//! `spectrum.on` is set. The vertical range is the `display.range` choice (+-6, 12 or 24 dB).
//!
//! Handles: a middle band sits at its frequency and gain (a notch on the 0 dB line), a cut on its
//! own curve at its corner. A band that is off has a hollow handle.
//!
//! # Editing the graph
//!
//! * Drag a handle: frequency and gain together (a cut and a notch: frequency only), the whole
//!   drag one undo step; Alt-drag changes the Q (the resonance of a cut) instead; Esc cancels.
//! * The wheel over a handle steps its Q (resonance).
//! * Double-click a handle: reset the band (frequency, gain, Q).
//! * Double-click on the empty graph: the nearest middle band that is not in use is switched on
//!   and put there.
//! * Right-click a handle: a menu with the shape (middle bands) or the slope (cuts), On and Reset.
//!
//! # The back
//!
//! The 19 CV inputs are a grid: the rows FREQ, GAIN and Q (RESO for a cut), the columns LCF, 1 ..
//! 5, HCF; each cell is the jack with its trim knob under it. The cuts have no GAIN cell. The four
//! audio sockets are at the right.

use crate::autolayout::{
    CONTENT_X, FxPanel, MARGIN_TOP, PanelSpec, PlacedControl, PlacedSocket, SOCKET_CELL, Section,
    TITLE_H, choose_widget,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx::{self, FxKind, LEVEL_FLOOR_DB, level_norm};
use crate::fx_paint::{bar, lcd, meter_color, polyline, small};
use crate::paint::{PanelInfo, fit_label_shrink};
use crate::theme::{Theme, hsl, mix};
use crate::widget::{current_value, format_value};
use atomik_device_sdk::{ControlKind, DeviceDescriptor};
use atomik_dsp::{BiquadCoeffs, BiquadShape};
use atomik_model::RackDevice;
use vello::kurbo::{BezPath, Circle, Line, Point, Rect, RoundedRect};
use vello::peniko::Color;

/// Height of the panel in rack units.
pub const EQ_UNITS: u8 = 7;
/// Number of bands.
pub const BANDS: usize = 7;
/// The `bandN` number of each UI band (LCF, 1, 2, 3, 4, 5, HCF).
pub const BAND_NUMBER: [usize; BANDS] = [1, 2, 3, 4, 7, 5, 6];
/// The label of each UI band.
pub const BAND_LABELS: [&str; BANDS] = ["LCF", "1", "2", "3", "4", "5", "HCF"];
/// The sample rate the curve is drawn at.
pub const DRAW_RATE: f64 = 48_000.0;
/// The vertical range (plus and minus, dB) of each `display.range` choice.
pub const RANGE_DB: [f64; 3] = [6.0, 12.0, 24.0];
/// The slope param values in ascending steepness (6, 12, 24, 36, 48 dB/oct).
pub const SLOPE_ORDER: [usize; 5] = [3, 0, 1, 4, 2];
/// Names of the stereo modes, as the descriptor's choice.
pub const STEREO_NAMES: [&str; 5] = ["Stereo", "Mid", "Side", "Mid solo", "Side solo"];
/// Radius of a handle of a middle band.
pub const HANDLE_R: f64 = 5.5;
/// Distance within which a press or wheel picks a handle.
pub const PICK_R: f64 = 11.0;
/// Lowest Q of a middle band the Alt-drag reaches is the descriptor's; this many pixels of drag
/// double or halve it.
pub const Q_DRAG_PIXELS: f64 = 40.0;

/// The kind of a band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The low cut (a high-pass).
    LowCut,
    /// A band with a frequency, a gain, a Q and a shape.
    Middle,
    /// The high cut (a low-pass).
    HighCut,
}

/// The kind of UI band `i`.
pub fn kind_of(i: usize) -> Kind {
    match i {
        0 => Kind::LowCut,
        6 => Kind::HighCut,
        _ => Kind::Middle,
    }
}

/// Whether UI band `i` is one of the two cuts.
pub fn is_cut(i: usize) -> bool {
    kind_of(i) != Kind::Middle
}

/// The param and socket keys of one UI band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keys {
    /// `bandN.on`.
    pub on: &'static str,
    /// `bandN.freq`.
    pub freq: &'static str,
    /// `bandN.gain` (middle bands).
    pub gain: Option<&'static str>,
    /// `bandN.q` (the resonance of a cut).
    pub q: &'static str,
    /// `bandN.shape` (middle bands).
    pub shape: Option<&'static str>,
    /// `lowcut.slope` / `highcut.slope` (cuts).
    pub slope: Option<&'static str>,
    /// `bandN.freq_cv_trim`.
    pub freq_trim: &'static str,
    /// `bandN.q_cv_trim`.
    pub q_trim: &'static str,
    /// `bandN.gain_cv_trim` (middle bands).
    pub gain_trim: Option<&'static str>,
    /// `bandN_freq_cv_in`.
    pub freq_cv: &'static str,
    /// `bandN_q_cv_in`.
    pub q_cv: &'static str,
    /// `bandN_gain_cv_in` (middle bands).
    pub gain_cv: Option<&'static str>,
}

macro_rules! keys {
    ($n:literal, middle $shape:expr) => {
        Keys {
            on: concat!("band", $n, ".on"),
            freq: concat!("band", $n, ".freq"),
            gain: Some(concat!("band", $n, ".gain")),
            q: concat!("band", $n, ".q"),
            shape: $shape,
            slope: None,
            freq_trim: concat!("band", $n, ".freq_cv_trim"),
            q_trim: concat!("band", $n, ".q_cv_trim"),
            gain_trim: Some(concat!("band", $n, ".gain_cv_trim")),
            freq_cv: concat!("band", $n, "_freq_cv_in"),
            q_cv: concat!("band", $n, "_q_cv_in"),
            gain_cv: Some(concat!("band", $n, "_gain_cv_in")),
        }
    };
    ($n:literal, cut $slope:literal) => {
        Keys {
            on: concat!("band", $n, ".on"),
            freq: concat!("band", $n, ".freq"),
            gain: None,
            q: concat!("band", $n, ".q"),
            shape: None,
            slope: Some($slope),
            freq_trim: concat!("band", $n, ".freq_cv_trim"),
            q_trim: concat!("band", $n, ".q_cv_trim"),
            gain_trim: None,
            freq_cv: concat!("band", $n, "_freq_cv_in"),
            q_cv: concat!("band", $n, "_q_cv_in"),
            gain_cv: None,
        }
    };
}

/// The keys of the seven UI bands.
pub const KEYS: [Keys; BANDS] = [
    keys!("1", cut "lowcut.slope"),
    keys!("2", middle Some("band2.shape")),
    keys!("3", middle Some("band3.shape")),
    keys!("4", middle Some("band4.shape")),
    keys!("7", middle Some("band7.shape")),
    keys!("5", middle Some("band5.shape")),
    keys!("6", cut "highcut.slope"),
];

/// The colour of UI band `i`.
pub fn band_color(i: usize) -> Color {
    hsl(i as f32 * 50.0 + 8.0, 0.65, 0.62)
}

// ------------------------------------------------------------------ the band model

/// One band as the device reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    /// Switched on.
    pub on: bool,
    /// Frequency, Hz.
    pub freq: f64,
    /// Gain, dB (a cut has none).
    pub gain: f64,
    /// Q (the resonance of a cut).
    pub q: f64,
    /// Shape of a middle band: 0 Bell, 1 Low shelf, 2 High shelf, 3 Notch.
    pub shape: usize,
    /// Slope of a cut (the param value, see [`SLOPE_ORDER`]).
    pub slope: usize,
}

/// The value of a param of a document device (the descriptor's default for an unstored one).
pub fn param_value(desc: &DeviceDescriptor, device: Option<&RackDevice>, key: &str) -> f32 {
    desc.params
        .iter()
        .find(|p| p.key == key)
        .map_or(0.0, |p| device.map_or(p.default, |d| current_value(d, p)))
}

/// The seven bands of an EQ device (descriptor defaults for unstored params).
pub fn bands_of(desc: &DeviceDescriptor, device: Option<&RackDevice>) -> [Band; BANDS] {
    std::array::from_fn(|i| {
        let k = &KEYS[i];
        let get = |key: &str| f64::from(param_value(desc, device, key));
        let opt = |key: Option<&str>| key.map_or(0.0, get);
        Band {
            on: get(k.on) >= 0.5,
            freq: get(k.freq),
            gain: opt(k.gain),
            q: get(k.q),
            shape: opt(k.shape).round().clamp(0.0, 3.0) as usize,
            slope: opt(k.slope).round().clamp(0.0, 4.0) as usize,
        }
    })
}

/// The vertical range of the graph, +- dB.
pub fn range_of(desc: &DeviceDescriptor, device: Option<&RackDevice>) -> f64 {
    let i = param_value(desc, device, "display.range").round().max(0.0) as usize;
    RANGE_DB[i.min(RANGE_DB.len() - 1)]
}

/// Whether band `i` changes the sound: on, and a cut, a notch, or a bell or shelf with a gain
/// other than 0 dB.
pub fn active(i: usize, b: &Band) -> bool {
    b.on && (is_cut(i) || middle_active(b.shape, b.gain as f32))
}

// ------------------------------------------------------------------ the curve

/// The Q a middle band of `shape` really uses for the knob value `q` (shelves 0.3..2, bells and
/// notches 0.1..18).
pub fn effective_q(shape: usize, q: f64) -> f64 {
    let (lo, hi) = if matches!(shape, 1 | 2) {
        (0.3, 2.0)
    } else {
        (0.1, 18.0)
    };
    q.clamp(lo, hi)
}

/// Whether a middle band of `shape` with `gain_db` is audible when it is on: a bell or a shelf
/// at exactly 0 dB is off, a notch ignores the gain.
pub fn middle_active(shape: usize, gain_db: f32) -> bool {
    shape == 3 || gain_db != 0.0
}

fn mid_shape(shape: usize) -> BiquadShape {
    match shape {
        1 => BiquadShape::LowShelf,
        2 => BiquadShape::HighShelf,
        3 => BiquadShape::Notch,
        _ => BiquadShape::Peaking,
    }
}

/// Magnitude response in dB at `f` Hz of a middle band: the section the device designs.
pub fn middle_response_db(shape: usize, freq: f64, q: f64, gain_db: f64, f: f64, sr: f64) -> f64 {
    middle_section(shape, freq, q, gain_db, sr).response_db(f, sr)
}

fn middle_section(shape: usize, freq: f64, q: f64, gain_db: f64, sr: f64) -> BiquadCoeffs {
    let gain_db = if shape == 3 { 0.0 } else { gain_db };
    BiquadCoeffs::design(mid_shape(shape), freq, effective_q(shape, q), gain_db, sr)
}

const BUTTER_Q: [[f64; 4]; 5] = [
    [std::f64::consts::FRAC_1_SQRT_2, 0.0, 0.0, 0.0],
    [0.541_196_1, 1.306_563, 0.0, 0.0],
    [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4],
    [std::f64::consts::FRAC_1_SQRT_2, 0.0, 0.0, 0.0],
    [
        0.517_638_1,
        std::f64::consts::FRAC_1_SQRT_2,
        1.931_851_7,
        0.0,
    ],
];
const SLOPE_SECTIONS: [usize; 5] = [1, 2, 4, 1, 3];
const SLOPE_6: usize = 3;

fn cut_sections(low: bool, slope: usize, freq: f64, q: f64, sr: f64) -> Vec<BiquadCoeffs> {
    let slope = slope.min(4);
    let n = SLOPE_SECTIONS[slope];
    let scale = q / std::f64::consts::FRAC_1_SQRT_2;
    (0..n)
        .map(|i| {
            let bq = BUTTER_Q[slope][i];
            let first_order = slope == SLOPE_6 && i == 0;
            let q = if i + 1 == n && !first_order {
                bq * scale
            } else {
                bq
            };
            let shape = match (low, first_order) {
                (true, true) => BiquadShape::FirstOrderHighpass,
                (true, false) => BiquadShape::Highpass,
                (false, true) => BiquadShape::FirstOrderLowpass,
                (false, false) => BiquadShape::Lowpass,
            };
            BiquadCoeffs::design(shape, freq, q, 0.0, sr)
        })
        .collect()
}

/// Magnitude response in dB at `f` Hz of a cut (`low` for the low cut) at slope param `slope`
/// with resonance `q`: the sections the device designs.
pub fn cut_response_db(low: bool, slope: usize, freq: f64, q: f64, f: f64, sr: f64) -> f64 {
    cut_sections(low, slope, freq, q, sr)
        .iter()
        .map(|s| s.response_db(f, sr))
        .sum()
}

/// The sections of band `i`, designed once so a curve of many points costs one design.
pub fn sections(i: usize, b: &Band, sr: f64) -> Vec<BiquadCoeffs> {
    match kind_of(i) {
        Kind::Middle => vec![middle_section(b.shape, b.freq, b.q, b.gain, sr)],
        Kind::LowCut => cut_sections(true, b.slope, b.freq, b.q, sr),
        Kind::HighCut => cut_sections(false, b.slope, b.freq, b.q, sr),
    }
}

fn sections_db(s: &[BiquadCoeffs], f: f64, sr: f64) -> f64 {
    s.iter().map(|c| c.response_db(f, sr)).sum()
}

/// The own response of band `i` at `f` Hz in dB, whether it is on or not.
pub fn band_response_db(i: usize, b: &Band, f: f64, sr: f64) -> f64 {
    sections_db(&sections(i, b, sr), f, sr)
}

/// The response in dB at `f` Hz of the bands that are active (the output gain is not in it).
pub fn total_response_db(bands: &[Band; BANDS], f: f64, sr: f64) -> f64 {
    bands
        .iter()
        .enumerate()
        .filter(|(i, b)| active(*i, b))
        .map(|(i, b)| band_response_db(i, b, f, sr))
        .sum()
}

// ------------------------------------------------------------------ graph geometry

/// Plot x of a frequency.
pub fn x_of(plot: Rect, freq: f64) -> f64 {
    plot.x0 + fx::eq_pos_of(freq) * plot.width()
}

/// Frequency at a plot x (clamped to the plot).
pub fn freq_of_x(plot: Rect, x: f64) -> f64 {
    fx::eq_freq_at((x - plot.x0) / plot.width())
}

/// Plot y of a level in dB for a graph of +- `range` dB (clamped to the plot).
pub fn y_of(plot: Rect, range: f64, db: f64) -> f64 {
    let t = (db.clamp(-range, range) + range) / (2.0 * range);
    plot.y1 - t * plot.height()
}

/// Level in dB at a plot y (clamped to the plot).
pub fn gain_of_y(plot: Rect, range: f64, y: f64) -> f64 {
    let t = ((plot.y1 - y) / plot.height()).clamp(0.0, 1.0);
    t * 2.0 * range - range
}

/// Where the handle of band `i` sits: a middle band at its frequency and gain (a notch on the
/// 0 dB line), a cut on its own curve at its corner.
pub fn handle_pos(plot: Rect, range: f64, i: usize, b: &Band) -> Point {
    let db = match kind_of(i) {
        Kind::Middle if b.shape == 3 => 0.0,
        Kind::Middle => b.gain,
        _ => band_response_db(i, b, b.freq, DRAW_RATE),
    };
    Point::new(x_of(plot, b.freq), y_of(plot, range, db))
}

/// The band whose handle is nearest to `p` within [`PICK_R`] (later bands win ties, so the
/// topmost drawn handle is picked).
pub fn pick(plot: Rect, range: f64, bands: &[Band; BANDS], p: Point) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (i, b) in bands.iter().enumerate() {
        let d = handle_pos(plot, range, i, b).distance(p);
        if d <= PICK_R && best.is_none_or(|(_, bd)| d <= bd) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// The middle band that is not in use (off, or a bell or shelf at 0 dB) whose frequency is
/// nearest (on the log axis) to `freq`; `None` when every middle band is in use.
pub fn nearest_free_band(bands: &[Band; BANDS], freq: f64) -> Option<usize> {
    let at = freq.max(1.0).ln();
    (0..BANDS)
        .filter(|&i| !is_cut(i) && !active(i, &bands[i]))
        .min_by(|&a, &b| {
            let da = (bands[a].freq.max(1.0).ln() - at).abs();
            let db = (bands[b].freq.max(1.0).ln() - at).abs();
            da.total_cmp(&db)
        })
}

/// The new Q after dragging `dy` pixels down (up raises it) from `start`: 40 px double or halve
/// it.
pub fn q_after_drag(start: f64, dy: f64) -> f64 {
    start * 2f64.powf(-dy / Q_DRAG_PIXELS)
}

/// The next (or previous) slope in ascending steepness, wrapping (the click of a slope control).
pub fn cycle_slope(value: f32, forward: bool) -> f32 {
    let at = SLOPE_ORDER
        .iter()
        .position(|&s| s == value.round().max(0.0) as usize)
        .unwrap_or(1);
    let n = SLOPE_ORDER.len();
    let next = if forward {
        (at + 1) % n
    } else {
        (at + n - 1) % n
    };
    SLOPE_ORDER[next] as f32
}

/// The grid of a graph of +- `range` dB: the step in dB and the number of lines each side of 0
/// (6 dB steps at +-24, 3 dB at +-12, 2 dB at +-6).
pub fn grid(range: f64) -> (f64, i32) {
    if range >= 24.0 {
        (6.0, 4)
    } else if range >= 12.0 {
        (3.0, 4)
    } else {
        (2.0, 3)
    }
}

// ------------------------------------------------------------------ the right-click menu

/// One row of the menu of a handle.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuRow {
    /// The text (translated where it is drawn).
    pub label: String,
    /// A check beside it: the current shape or slope, or On while the band is on.
    pub checked: bool,
    /// The params it sets (all in one command).
    pub sets: Vec<(&'static str, f32)>,
}

fn default_of(desc: &DeviceDescriptor, key: &str) -> f32 {
    desc.param_by_key(key).map_or(0.0, |p| p.default)
}

/// The params a reset puts back: frequency, gain and Q; with `full` the shape or the slope too.
pub fn reset_sets(desc: &DeviceDescriptor, i: usize, full: bool) -> Vec<(&'static str, f32)> {
    let k = &KEYS[i];
    let mut sets = vec![(k.freq, default_of(desc, k.freq))];
    if let Some(g) = k.gain {
        sets.push((g, default_of(desc, g)));
    }
    sets.push((k.q, default_of(desc, k.q)));
    if full {
        for key in [k.shape, k.slope].into_iter().flatten() {
            sets.push((key, default_of(desc, key)));
        }
    }
    sets
}

/// The rows of the menu of band `i`: the shapes of a middle band or the slopes of a cut, then On
/// and Reset band.
pub fn menu_rows(desc: &DeviceDescriptor, i: usize, b: &Band) -> Vec<MenuRow> {
    let k = &KEYS[i];
    let mut rows = Vec::new();
    if let Some(shape) = k.shape
        && let Some(p) = desc.param_by_key(shape)
        && let atomik_core::Unit::Choice(names) = p.unit
    {
        for (n, name) in names.iter().enumerate() {
            rows.push(MenuRow {
                label: (*name).to_string(),
                checked: b.shape == n,
                sets: vec![(shape, n as f32)],
            });
        }
    }
    if let Some(slope) = k.slope
        && let Some(p) = desc.param_by_key(slope)
        && let atomik_core::Unit::Choice(names) = p.unit
    {
        for &n in &SLOPE_ORDER {
            rows.push(MenuRow {
                label: names.get(n).copied().unwrap_or("").to_string(),
                checked: b.slope == n,
                sets: vec![(slope, n as f32)],
            });
        }
    }
    rows.push(MenuRow {
        label: "Band on".to_string(),
        checked: b.on,
        sets: vec![(k.on, if b.on { 0.0 } else { 1.0 })],
    });
    rows.push(MenuRow {
        label: "Reset band".to_string(),
        checked: false,
        sets: reset_sets(desc, i, true),
    });
    rows
}

// ------------------------------------------------------------------ layout

const X0: f64 = CONTENT_X.0;
const X1: f64 = CONTENT_X.1;
const PLOT_TOP: f64 = MARGIN_TOP + TITLE_H + 4.0;
const PLOT_H: f64 = 196.0;
const GUTTER: f64 = 26.0;
const AXIS_H: f64 = 12.0;
const STRIP_TOP: f64 = PLOT_TOP + PLOT_H + AXIS_H + 5.0;
const RIGHT_W: f64 = 82.0;
const STRIP_PITCH: f64 = (X1 - RIGHT_W - 8.0 - X0) / BANDS as f64;
/// Cell tops of the strip rows (from [`STRIP_TOP`]).
const ON_Y: f64 = STRIP_TOP + TITLE_H + 1.0;
const SHAPE_Y: f64 = ON_Y + 30.0;
const FREQ_Y: f64 = SHAPE_Y + 38.0;
const GAIN_Y: f64 = FREQ_Y + 70.0;
const Q_Y: f64 = GAIN_Y + 70.0;
const KNOB_CELL: (f64, f64) = (48.0, 70.0);
const CHOICE_CELL: (f64, f64) = (62.0, 36.0);
const TOGGLE_CELL: (f64, f64) = (44.0, 28.0);
/// Where a value readout sits under a knob (from the top of its cell).
const READOUT_DY: f64 = 63.0;

const BACK_LABEL_W: f64 = 34.0;
const COL_PITCH: f64 = 62.0;
const BACK_CHIP_Y: f64 = MARGIN_TOP + TITLE_H + 2.0;
const JACK_Y0: f64 = BACK_CHIP_Y + 17.0;
const ROW_PITCH: f64 = 110.0;
const TRIM_CELL: (f64, f64) = (48.0, 58.0);

/// The CV trim params placed on the back: 7 frequency, 5 gain, 7 Q.
pub const BACK_PARAMS: usize = 19;

/// Geometry of the EQ panel (front and back, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqGeom {
    /// The graph plot (the curve, the spectrum and the handles).
    pub plot: Rect,
    /// The band chips above the strips.
    pub chips: [Rect; BANDS],
    /// The meter bars of the right column.
    pub meters: Rect,
    /// The band chips on the back.
    pub back_chips: [Rect; BANDS],
    /// Where the row names (FREQ, GAIN, Q) go on the back.
    pub back_rows: [Rect; 3],
    /// Descriptor indices of the controls that sit on the back face (the CV trims).
    pub back_params: [usize; BACK_PARAMS],
}

fn centered(cx: f64, y: f64, size: (f64, f64)) -> Rect {
    Rect::new(cx - size.0 / 2.0, y, cx + size.0 / 2.0, y + size.1)
}

/// The centre x of strip `i` on the front.
pub fn strip_cx(i: usize) -> f64 {
    X0 + (i as f64 + 0.5) * STRIP_PITCH
}

/// The cell of the jack of UI band `i` in back row `row` (0 FREQ, 1 GAIN, 2 Q).
pub fn back_cell(i: usize, row: usize) -> Rect {
    let x = X0 + BACK_LABEL_W + i as f64 * COL_PITCH + (COL_PITCH - SOCKET_CELL.0) / 2.0;
    let y = JACK_Y0 + row as f64 * ROW_PITCH;
    Rect::new(x, y, x + SOCKET_CELL.0, y + SOCKET_CELL.1)
}

/// The panel (7 U): the graph, the strips and the right column on the front, the CV grid and the
/// audio sockets on the back.
pub fn eq_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let index = |k: &str| desc.params.iter().position(|p| p.key == k);
    let plot = Rect::new(X0 + GUTTER, PLOT_TOP, X1 - 2.0, PLOT_TOP + PLOT_H);
    let visual = Rect::new(X0, PLOT_TOP, X1, PLOT_TOP + PLOT_H + AXIS_H);
    let mut front: Vec<PlacedControl> = Vec::new();
    let mut place = |key: &str, rect: Rect| {
        if let Some(param) = index(key) {
            front.push(PlacedControl {
                param,
                kind: choose_widget(&desc.params[param]),
                rect,
            });
        }
    };
    let mut chips = [Rect::ZERO; BANDS];
    for (i, k) in KEYS.iter().enumerate() {
        let cx = strip_cx(i);
        chips[i] = Rect::new(cx - 22.0, STRIP_TOP, cx + 22.0, STRIP_TOP + TITLE_H);
        place(k.on, centered(cx, ON_Y, TOGGLE_CELL));
        if let Some(s) = k.shape.or(k.slope) {
            place(s, centered(cx, SHAPE_Y, CHOICE_CELL));
        }
        place(k.freq, centered(cx, FREQ_Y, KNOB_CELL));
        if let Some(g) = k.gain {
            place(g, centered(cx, GAIN_Y, KNOB_CELL));
        }
        place(k.q, centered(cx, Q_Y, KNOB_CELL));
    }
    // The right column: output, meters, stereo mode, spectrum, range.
    let rx0 = X1 - RIGHT_W;
    let rcx = rx0 + RIGHT_W / 2.0;
    place("output", centered(rcx, ON_Y, KNOB_CELL));
    let meters = Rect::new(
        rx0 + 10.0,
        ON_Y + KNOB_CELL.1 + 4.0,
        X1 - 10.0,
        ON_Y + 160.0,
    );
    let mode_y = meters.y1 + 6.0;
    place("stereo.mode", centered(rcx, mode_y, CHOICE_CELL));
    place(
        "spectrum.on",
        centered(rcx, mode_y + CHOICE_CELL.1 + 4.0, TOGGLE_CELL),
    );
    place(
        "display.range",
        centered(
            rcx,
            mode_y + CHOICE_CELL.1 + 4.0 + TOGGLE_CELL.1 + 4.0,
            CHOICE_CELL,
        ),
    );

    // The back: the CV grid, each jack with its trim knob under it.
    let socket = |k: &str| desc.sockets.iter().position(|s| s.key == k);
    let mut back = Vec::new();
    let mut back_params = [usize::MAX; BACK_PARAMS];
    let mut n_back = 0;
    let mut back_chips = [Rect::ZERO; BANDS];
    for (i, k) in KEYS.iter().enumerate() {
        let cell = back_cell(i, 0);
        back_chips[i] = Rect::new(
            cell.center().x - 22.0,
            BACK_CHIP_Y,
            cell.center().x + 22.0,
            BACK_CHIP_Y + TITLE_H,
        );
        let rows = [
            (0, Some((k.freq_cv, k.freq_trim))),
            (1, k.gain_cv.zip(k.gain_trim)),
            (2, Some((k.q_cv, k.q_trim))),
        ];
        for (row, cell_keys) in rows {
            let Some((cv, trim)) = cell_keys else {
                continue;
            };
            let rect = back_cell(i, row);
            if let Some(s) = socket(cv) {
                back.push(PlacedSocket { socket: s, rect });
            }
            if let Some(param) = index(trim) {
                back_params[n_back] = param;
                n_back += 1;
                let top = rect.y1 + 4.0;
                front.push(PlacedControl {
                    param,
                    kind: ControlKind::Knob,
                    rect: Rect::new(
                        rect.center().x - TRIM_CELL.0 / 2.0,
                        top,
                        rect.center().x + TRIM_CELL.0 / 2.0,
                        top + TRIM_CELL.1,
                    ),
                });
            }
        }
    }
    let back_rows = std::array::from_fn(|row| {
        let c = back_cell(0, row);
        Rect::new(X0, c.y0, X0 + BACK_LABEL_W - 4.0, c.y0 + 28.0)
    });
    let audio_x = X0 + BACK_LABEL_W + BANDS as f64 * COL_PITCH + 14.0;
    let audio_cell = |x: f64, y: f64| Rect::new(x, y, x + SOCKET_CELL.0, y + SOCKET_CELL.1);
    let audio_out_title = JACK_Y0 + SOCKET_CELL.1 + 14.0;
    for (k, x, y) in [
        ("in_l", audio_x, JACK_Y0),
        ("in_r", audio_x + 56.0, JACK_Y0),
        ("out_l", audio_x, audio_out_title + TITLE_H + 4.0),
        ("out_r", audio_x + 56.0, audio_out_title + TITLE_H + 4.0),
    ] {
        if let Some(s) = socket(k) {
            back.push(PlacedSocket {
                socket: s,
                rect: audio_cell(x, y),
            });
        }
    }
    let title = |label: &str, x0: f64, x1: f64, y: f64| Section {
        label: label.to_string(),
        rect: Rect::new(x0, y, x1, y + TITLE_H),
    };
    let front_sections = vec![
        title("RESPONSE", X0, X1, MARGIN_TOP),
        title("OUTPUT", rx0, X1, STRIP_TOP),
    ];
    let back_sections = vec![
        title(
            "CV INPUTS",
            X0 + BACK_LABEL_W,
            X0 + BACK_LABEL_W + BANDS as f64 * COL_PITCH,
            MARGIN_TOP,
        ),
        title("AUDIO IN", audio_x, audio_x + 106.0, MARGIN_TOP),
        title("AUDIO OUT", audio_x, audio_x + 106.0, audio_out_title),
    ];
    let geom = EqGeom {
        plot,
        chips,
        meters,
        back_chips,
        back_rows,
        back_params,
    };
    PanelSpec {
        height_units: EQ_UNITS,
        samples: None,
        drum: None,
        slicer: None,
        fx: Some(FxPanel {
            kind: FxKind::Eq,
            visual,
            plot,
            meters,
            readout: Rect::ZERO,
            aux: Rect::ZERO,
            button: None,
            character: None,
            eq: Some(geom),
            stereo: None,
            gain: None,
            saturation: None,
            headphone: None,
        }),
        seq: None,
        drum_gen: None,
        arp: None,
        key_chord: None,
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

// ------------------------------------------------------------------ labels

/// The label under a knob or toggle of the EQ panel (`None`: the generic label).
pub fn label(key: &str) -> Option<&'static str> {
    Some(match key {
        "output" => "Output",
        "spectrum.on" => "Spectrum",
        "band1.q" | "band6.q" => "Reso",
        k if k.ends_with("_cv_trim") => "Trim",
        _ => return None,
    })
}

/// The row names of the back grid.
pub const ROW_NAMES: [&str; 3] = ["FREQ", "GAIN", "Q/RESO"];

// ------------------------------------------------------------------ painting

fn alpha(c: Color, a: f32) -> Color {
    c.with_alpha(a)
}

fn value(info: &PanelInfo<'_>, key: &str) -> f32 {
    info.desc.map_or(0.0, |d| param_value(d, info.device, key))
}

/// The spectrum trace's vertical placement: -90 dB at the bottom of the graph, -6 dB at 85 % of
/// its height.
fn trace_h(db: f32, plot: Rect) -> f64 {
    level_norm(db, -90.0, -6.0) * plot.height() * 0.85
}

fn paint_graph<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    plot: Rect,
    info: &PanelInfo<'_>,
    bands: &[Band; BANDS],
    range: f64,
) {
    lcd(c, theme, plot);
    c.push_clip(&plot.into());
    // The grid: a line every third of the range, the decades of the frequency axis.
    let (step, lines) = grid(range);
    for k in -lines..=lines {
        let db = f64::from(k) * step;
        let y = y_of(plot, range, db).round() + 0.5;
        let a = if k == 0 { 0.22 } else { 0.06 };
        c.stroke_with(
            Line::new(pt(plot.x0, y), pt(plot.x1, y)),
            1.0,
            alpha(theme.ink, a),
        );
    }
    for f in GRID_HZ {
        let x = x_of(plot, f).round() + 0.5;
        let major = matches!(f as u32, 100 | 1000 | 10_000);
        c.stroke_with(
            Line::new(pt(x, plot.y0), pt(x, plot.y1)),
            1.0,
            alpha(theme.ink, if major { 0.14 } else { 0.05 }),
        );
    }
    // The spectrum of the output.
    let trace = &info.analyser.trace;
    if value(info, "spectrum.on") >= 0.5 && trace.len() > 1 {
        let n = trace.len();
        let x = |k: usize| plot.x0 + (k as f64 + 0.5) / n as f64 * plot.width();
        let line = trace.iter().enumerate().map(|(k, v)| {
            let h = trace_h(*v, plot);
            pt(x(k), plot.y1 - h)
        });
        let mut fill = polyline(line.clone());
        fill.line_to(pt(x(n - 1), plot.y1));
        fill.line_to(pt(x(0), plot.y1));
        fill.close_path();
        c.fill_with(fill, alpha(info.accent, 0.2));
        c.stroke_with(
            polyline(line),
            1.25,
            alpha(mix(info.accent, Color::WHITE, 0.3), 0.55),
        );
    }
    // The curves: each active band tinted, then the sum.
    let steps = (plot.width() / 2.0).floor().max(2.0) as usize;
    let freqs: Vec<(f64, f64)> = (0..=steps)
        .map(|k| {
            let t = k as f64 / steps as f64;
            (plot.x0 + t * plot.width(), fx::eq_freq_at(t))
        })
        .collect();
    let zero = y_of(plot, range, 0.0);
    let mut sum = vec![0.0f64; freqs.len()];
    for (i, b) in bands.iter().enumerate() {
        if !active(i, b) {
            continue;
        }
        let s = sections(i, b, DRAW_RATE);
        let ys: Vec<f64> = freqs
            .iter()
            .map(|(_, f)| sections_db(&s, *f, DRAW_RATE))
            .collect();
        for (a, y) in sum.iter_mut().zip(&ys) {
            *a += y;
        }
        let pts = freqs
            .iter()
            .zip(&ys)
            .map(|((x, _), db)| pt(*x, y_of(plot, range, *db)));
        let mut fill = polyline(pts.clone());
        fill.line_to(pt(plot.x1, zero));
        fill.line_to(pt(plot.x0, zero));
        fill.close_path();
        c.fill_with(fill, alpha(band_color(i), 0.12));
        c.stroke_with(polyline(pts), 1.0, alpha(band_color(i), 0.4));
    }
    let pts: Vec<Point> = freqs
        .iter()
        .zip(&sum)
        .map(|((x, _), db)| pt(*x, y_of(plot, range, *db)))
        .collect();
    let mut fill = polyline(pts.iter().copied());
    fill.line_to(pt(plot.x1, zero));
    fill.line_to(pt(plot.x0, zero));
    fill.close_path();
    c.fill_with(fill, alpha(info.accent, 0.1));
    c.stroke_with(
        polyline(pts.iter().copied()),
        1.75,
        mix(info.accent, Color::WHITE, 0.45),
    );
    // The handles: the bands that are off under the ones that are on.
    for pass_on in [false, true] {
        for (i, b) in bands.iter().enumerate() {
            if b.on == pass_on {
                handle(c, theme, plot, range, i, b);
            }
        }
    }
    // The stereo mode, when the bands do not act on plain left and right.
    let mode = value(info, "stereo.mode").round().max(0.0) as usize;
    if mode > 0 {
        let name = STEREO_NAMES[mode.min(STEREO_NAMES.len() - 1)].to_uppercase();
        let w = name.chars().count() as f64 * 5.4 + 12.0;
        let r = Rect::new(
            plot.x0 + 6.0,
            plot.y0 + 5.0,
            plot.x0 + 6.0 + w,
            plot.y0 + 17.0,
        );
        c.fill_with(RoundedRect::from_rect(r, 3.0), alpha(info.accent, 0.22));
        c.text(
            &TextRun::new(
                name,
                pt(r.center().x, r.y1 - 3.5),
                7.5,
                mix(info.accent, Color::WHITE, 0.5),
            )
            .align(Align::Center)
            .weight(700),
        );
    }
    c.pop_clip();
    // The axes.
    for (f, text) in AXIS_HZ {
        small(
            c,
            theme,
            text,
            pt(
                x_of(plot, f).clamp(plot.x0 + 6.0, plot.x1 - 6.0),
                plot.y1 + 9.0,
            ),
            Align::Center,
            None,
        );
    }
    for k in -lines..=lines {
        let db = f64::from(k) * step;
        let text = if k == 0 {
            "0".to_string()
        } else {
            format!("{db:+.0}")
        };
        small(
            c,
            theme,
            text,
            pt(plot.x0 - 4.0, y_of(plot, range, db) + 2.5),
            Align::End,
            None,
        );
    }
}

const GRID_HZ: [f64; 26] = [
    30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0,
    800.0, 900.0, 1000.0, 2000.0, 3000.0, 4000.0, 5000.0, 6000.0, 7000.0, 8000.0, 9000.0, 10000.0,
];

const AXIS_HZ: [(f64, &str); 10] = [
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
];

fn handle<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    plot: Rect,
    range: f64,
    i: usize,
    b: &Band,
) {
    let at = handle_pos(plot, range, i, b);
    let col = band_color(i);
    let text = BAND_LABELS[i];
    let ink = if b.on { Color::BLACK } else { theme.text_muted };
    if is_cut(i) {
        let r = Rect::new(at.x - 11.0, at.y - 6.0, at.x + 11.0, at.y + 6.0);
        if b.on {
            c.fill_with(
                RoundedRect::from_rect(r.inflate(3.0, 3.0), 8.0),
                alpha(col, 0.2),
            );
            c.fill_with(RoundedRect::from_rect(r, 6.0), col);
        } else {
            c.fill_with(RoundedRect::from_rect(r, 6.0), alpha(Color::BLACK, 0.5));
            c.stroke_with(
                RoundedRect::from_rect(r.inset(-0.5), 5.5),
                1.25,
                alpha(col, 0.6),
            );
        }
        c.text(
            &TextRun::new(text, pt(at.x, at.y + 2.6), 7.0, ink)
                .align(Align::Center)
                .weight(700),
        );
        return;
    }
    if b.on {
        c.fill_with(Circle::new(at, HANDLE_R + 3.0), alpha(col, 0.2));
        c.fill_with(Circle::new(at, HANDLE_R), col);
    } else {
        c.fill_with(Circle::new(at, HANDLE_R), alpha(Color::BLACK, 0.5));
        c.stroke_with(Circle::new(at, HANDLE_R - 0.5), 1.25, alpha(col, 0.6));
    }
    c.text(
        &TextRun::new(text, pt(at.x, at.y + 2.6), 7.0, ink)
            .align(Align::Center)
            .weight(700),
    );
}

/// A small sketch of a shape (bell, low shelf, high shelf, notch) in `r`.
fn shape_glyph<C: Canvas + ?Sized>(c: &mut C, r: Rect, shape: usize, color: Color) {
    let smooth = |a: f64, b: f64, t: f64| {
        let u = ((t - a) / (b - a)).clamp(0.0, 1.0);
        u * u * (3.0 - 2.0 * u)
    };
    // The height of the sketch at `t` in 0..1: +1 is a boost, -1 a cut.
    let v = |t: f64| match shape {
        1 => 0.8 * (1.0 - smooth(0.25, 0.75, t)),
        2 => 0.8 * smooth(0.25, 0.75, t),
        3 => 0.5 - 1.5 * (-((t - 0.5) / 0.1).powi(2)).exp(),
        _ => (-((t - 0.5) / 0.18).powi(2)).exp(),
    };
    let mid = r.center().y;
    c.stroke_with(
        Line::new(pt(r.x0, mid), pt(r.x1, mid)),
        1.0,
        alpha(color, 0.25),
    );
    let n = 24;
    let path = polyline((0..=n).map(|k| {
        let t = f64::from(k) / f64::from(n);
        pt(r.x0 + t * r.width(), mid - v(t) * r.height() / 2.0)
    }));
    c.stroke_with(path, 1.25, color);
}

/// A choice cell: the display with its text and the two arrows, the label under it.
fn choice_cell<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    text: &str,
    label: &str,
    accent: Color,
) -> Rect {
    let cx = r.center().x;
    let half = (r.width() / 2.0 - 3.0).max(28.0);
    let lcd_r = Rect::new(cx - half, r.y0 + 3.0, cx + half, r.y0 + 19.0);
    c.fill_with(RoundedRect::from_rect(lcd_r, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(lcd_r.inset(-0.5), 3.5),
        1.0,
        alpha(theme.ink, 0.07),
    );
    let (shown, size) = fit_label_shrink(
        c,
        &crate::i18n::tr(text),
        lcd_r.width() - 14.0,
        8.5,
        6.5,
        600,
    );
    c.text(
        &TextRun::new(
            shown,
            pt(cx, lcd_r.y0 + 11.5),
            size,
            mix(accent, Color::WHITE, 0.3),
        )
        .weight(600)
        .align(Align::Center),
    );
    for (x, dir) in [(lcd_r.x0 + 5.0, -1.0), (lcd_r.x1 - 5.0, 1.0)] {
        let y = lcd_r.center().y;
        let mut t = BezPath::new();
        t.move_to(pt(x + 1.6 * dir, y));
        t.line_to(pt(x - 1.6 * dir, y - 2.6));
        t.line_to(pt(x - 1.6 * dir, y + 2.6));
        t.close_path();
        c.fill_with(t, theme.text_muted);
    }
    if !label.is_empty() {
        c.text(
            &TextRun::new(
                label,
                pt(cx, r.y0 + 31.0),
                theme.label_size,
                theme.text_muted,
            )
            .align(Align::Center),
        );
    }
    lcd_r
}

/// The text of choice param `p` at `plain` on its display: the descriptor's name, with the
/// range spelled with a plus-minus sign.
fn choice_text(p: &atomik_core::ParamDescriptor, plain: f32) -> String {
    if p.key == "display.range" {
        let i = (plain.round().max(0.0) as usize).min(RANGE_DB.len() - 1);
        return format!("\u{b1}{:.0} dB", RANGE_DB[i]);
    }
    format_value(p, plain)
}

/// Paints the front: graph, band chips, choice cells, value readouts and the output meters.
pub fn paint_front<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: &FxPanel,
    info: &PanelInfo<'_>,
) {
    let Some(g) = fx.eq else { return };
    let Some(desc) = info.desc else { return };
    let bands = bands_of(desc, info.device);
    let range = range_of(desc, info.device);
    paint_graph(c, theme, g.plot, info, &bands, range);
    // The chips over the strips.
    for (i, r) in g.chips.iter().enumerate() {
        chip(c, theme, *r, i, bands[i].on);
    }
    // The choice cells and the readouts.
    for pc in info.spec.front.iter().filter(|pc| !info.spec.on_back(pc)) {
        let p = &desc.params[pc.param];
        let v = info.device.map_or(p.default, |d| current_value(d, p));
        match pc.kind {
            ControlKind::Choice => {
                let (label, glyph) = match p.key {
                    "stereo.mode" => ("Stereo", None),
                    "display.range" => ("Range", None),
                    k if k.ends_with(".slope") => ("Slope", None),
                    _ => ("", Some(v.round().max(0.0) as usize)),
                };
                let lcd_r = choice_cell(c, theme, pc.rect, &choice_text(p, v), label, info.accent);
                if let Some(shape) = glyph {
                    let band = KEYS.iter().position(|k| k.shape == Some(p.key));
                    let col = match band {
                        Some(i) if bands[i].on => band_color(i),
                        _ => theme.text_muted,
                    };
                    let box_r = Rect::new(
                        lcd_r.center().x - 12.0,
                        pc.rect.y0 + 23.0,
                        lcd_r.center().x + 12.0,
                        pc.rect.y0 + 33.0,
                    );
                    shape_glyph(c, box_r, shape, col);
                }
            }
            ControlKind::Knob => {
                let band = KEYS
                    .iter()
                    .position(|k| k.freq == p.key || k.gain == Some(p.key) || k.q == p.key);
                let live = band.is_none_or(|i| bands[i].on);
                c.text(
                    &TextRun::new(
                        format_value(p, v),
                        pt(pc.rect.center().x, pc.rect.y0 + READOUT_DY),
                        8.0,
                        if live { theme.text } else { theme.text_muted },
                    )
                    .align(Align::Center)
                    .weight(600),
                );
            }
            _ => {}
        }
    }
    // The output peaks.
    let m = g.meters;
    let label_h = 10.0;
    let (w, gap) = (16.0, 12.0);
    let x0 = m.x0 + (m.width() - (2.0 * w + gap)) / 2.0;
    for (k, label) in ["L", "R"].into_iter().enumerate() {
        let db = info
            .fx_meters
            .get(16 + k)
            .copied()
            .unwrap_or(LEVEL_FLOOR_DB);
        let x = x0 + k as f64 * (w + gap);
        bar(
            c,
            Rect::new(x, m.y0, x + w, m.y1 - label_h),
            level_norm(db, -60.0, 0.0),
            meter_color(db),
            false,
        );
        small(
            c,
            theme,
            label,
            pt(x + w / 2.0, m.y1 - 1.0),
            Align::Center,
            None,
        );
    }
}

fn chip<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, r: Rect, i: usize, on: bool) {
    let col = band_color(i);
    c.fill_with(
        RoundedRect::from_rect(r, 3.0),
        alpha(col, if on { 0.28 } else { 0.1 }),
    );
    c.text(
        &TextRun::new(
            BAND_LABELS[i],
            pt(r.center().x, r.y1 - 2.8),
            7.5,
            if on { col } else { theme.text_muted },
        )
        .align(Align::Center)
        .weight(700),
    );
}

/// The name of an audio socket or `""` for a CV jack of the grid (the row and column say what it
/// is).
pub fn socket_label(key: &str) -> Option<&'static str> {
    key.contains("_cv_in").then_some("")
}
/// Paints the back: the column chips and the row names of the CV grid.
pub fn paint_back<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: &FxPanel,
    info: &PanelInfo<'_>,
) {
    let Some(g) = fx.eq else { return };
    let bands = info.desc.map(|d| bands_of(d, info.device));
    for (i, r) in g.back_chips.iter().enumerate() {
        chip(c, theme, *r, i, bands.is_some_and(|b| b[i].on));
    }
    for (row, r) in g.back_rows.iter().enumerate() {
        small(
            c,
            theme,
            ROW_NAMES[row],
            pt(r.x1, r.y0 + 17.0),
            Align::End,
            None,
        );
    }
}
