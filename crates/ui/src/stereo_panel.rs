//! The rack panel of `atomik.stereo-tool`: a 3 U dual-band stereo imager.
//!
//! ```text
//! LO BAND                    X-OVER              HI BAND                     SOLO
//! o ACTIVE   MONO..ORIGINAL..WIDE   o SPLIT      o ACTIVE   MONO..ORIGINAL..WIDE   [o SOLO HI BAND]
//!            (   big knob    )       ( knob )               (   big knob    )       [o SOLO LO BAND]
//!            [   1.00    ]           [ 1.2 kHz ]            [   1.00    ]           [o NORMAL      ]
//! CORRELATION -1 ---|--- +1                              CORRELATION -1 ---|--- +1
//! POLARITY         ROTATE                BALANCE      M/S MODE        OUTPUT
//! [Ø L] [Ø R]      (ROTATE) (ROTATE AT)  (BALANCE)    [stereo]        [MONO]
//! ```
//!
//! The three big knobs (`lo_width`, `low_bypass` shown as X-OVER, `width` which is the high
//! band's) and the three SOLO radio buttons are painted here; everything else (polarity,
//! rotation, balance, M/S mode, mono) is an ordinary placed control, so the generic value,
//! automation and MIDI machinery of the rack applies to all of them. Every control is always
//! there: nothing hides or shows with a param.
//!
//! * The width knobs read MONO (0) at the start of the sweep, ORIGINAL (1) at the top and WIDE
//!   (2) at the end; the value ring fills from the top, like a pan knob.
//! * The X-OVER knob runs 20 Hz to 6 kHz; at its bottom (20.5 Hz or less) there is no split
//!   and the readout says `off`.
//! * ACTIVE lamps and the SPLIT lamp come from the meters, the correlation bars from the two
//!   correlation meters (-1 to +1, the centre marked).
//! * SOLO is one `Choice` param (`solo`: normal, lo, hi) drawn as three radio buttons: a press
//!   on a button picks that value, the wheel and the arrow keys step through the three.
//!
//! The back has four groups: AUDIO IN, AUDIO OUT, CV IN (three jacks, each with its trim knob
//! under it; the trims live on the back face only) and BAND OUTPUTS (low L/R, high L/R).

use crate::autolayout::{
    CONTENT_X, FxPanel, MARGIN_TOP, PanelSpec, PlacedControl, PlacedSocket, SOCKET_CELL, Section,
    TITLE_H,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx::FxKind;
use crate::fx_paint::{lcd, small};
use crate::paint::{KNOB_START, KNOB_SWEEP, PanelInfo, vgrad};
use crate::theme::{Theme, hsl, mix};
use crate::widget::current_value;
use atomik_device_sdk::{ControlKind, DeviceDescriptor};
use vello::kurbo::{
    Arc, BezPath, Cap, Circle, Line, Point, Rect, RoundedRect, Shape, Stroke, Vec2,
};
use vello::peniko::{Brush, Color};

/// The device type of this panel.
pub const STEREO_TOOL_TYPE: &str = "atomik.stereo-tool";
/// Height of the panel in rack units.
pub const STEREO_UNITS: u8 = 3;

/// The meter values (`atomik_devices::stereo_tool::METER_*`; a test pins them together).
pub const METER_LO_CORR: usize = 0;
/// Correlation of the high band.
pub const METER_HI_CORR: usize = 1;
/// The low band carries signal and is not at width 1.
pub const METER_LO_ACTIVE: usize = 2;
/// The high band carries signal and is not at width 1.
pub const METER_HI_ACTIVE: usize = 3;
/// The crossover splits.
pub const METER_SPLIT: usize = 4;

/// The crossover reads `off` at this frequency or below (Hz).
pub const XOVER_OFF_HZ: f32 = 20.5;

/// `solo` values of the three radio buttons, top to bottom: SOLO HI BAND, SOLO LO BAND, NORMAL.
pub const SOLO_VALUES: [f32; 3] = [2.0, 1.0, 0.0];

/// Height of a row's content cell.
const KNOB_H: f64 = 58.0;
const KNOB_W: f64 = 56.0;
const TOGGLE: (f64, f64) = (44.0, 28.0);
const CHOICE: (f64, f64) = (62.0, 36.0);
/// Rows: title top. The second row's content starts under its title.
const ROW_Y: [f64; 2] = [MARGIN_TOP, MARGIN_TOP + TITLE_H + 130.0];
/// Radius of the value ring of a big knob and of its cap.
const RING_R: f64 = 27.0;
const CAP_R: f64 = 19.5;
/// Half the side of the square a big knob is grabbed by.
const GRAB: f64 = 32.0;
/// Width of the band sections, of the crossover and of the solo section.
const BAND_W: f64 = 190.0;
const XOVER_W: f64 = 126.0;
const SOLO_W: f64 = 94.0;
const GAP: f64 = 8.0;

/// Geometry of the stereo tool's own parts (front, panel coordinates, and the back trims).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StereoGeom {
    /// The low band's knob (the control rect of `lo_width`).
    pub lo_knob: Rect,
    /// The crossover knob.
    pub xover_knob: Rect,
    /// The high band's knob.
    pub hi_knob: Rect,
    /// The ACTIVE lamp of the low band, the SPLIT lamp, the ACTIVE lamp of the high band (a
    /// lamp and its caption).
    pub lamps: [Rect; 3],
    /// The correlation bars of the low and the high band.
    pub corr: [Rect; 2],
    /// The three SOLO buttons, top to bottom ([`SOLO_VALUES`]).
    pub solo: [Rect; 3],
    /// Descriptor indices of the controls that sit on the back face (the three CV trims).
    pub back_params: [usize; 3],
}

impl StereoGeom {
    /// The cell of the whole SOLO group: what the `solo` control covers.
    pub fn solo_cell(&self) -> Rect {
        self.solo[0].union(self.solo[2])
    }

    /// The `solo` value of the button at `p` (panel coordinates; the nearest button when `p` is
    /// between two).
    pub fn solo_at(&self, p: Point) -> f32 {
        let hit = self.solo.iter().position(|r| r.contains(p));
        let n = hit.unwrap_or_else(|| {
            (0..3)
                .min_by(|a, b| {
                    let da = (self.solo[*a].center().y - p.y).abs();
                    let db = (self.solo[*b].center().y - p.y).abs();
                    da.total_cmp(&db)
                })
                .unwrap_or(2)
        });
        SOLO_VALUES[n]
    }
}

/// Whether the front control of `key` is painted by this panel instead of the generic cell.
pub fn custom_painted(key: &str) -> bool {
    matches!(key, "lo_width" | "width" | "low_bypass" | "solo")
}

/// The panel: 3 U with the layout of the module docs; the back has the eleven sockets and the
/// three trims.
pub fn stereo_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let x0 = CONTENT_X.0;
    let x1 = CONTENT_X.1;
    let index = |k: &str| desc.params.iter().position(|p| p.key == k);
    let c1 = ROW_Y[0] + TITLE_H;
    let c2 = ROW_Y[1] + TITLE_H;
    let mut front: Vec<PlacedControl> = Vec::new();
    let mut put = |key: &str, kind: ControlKind, rect: Rect| {
        if let Some(param) = index(key) {
            front.push(PlacedControl { param, kind, rect });
        }
    };
    let knob = |x: f64| Rect::new(x, c2, x + KNOB_W, c2 + KNOB_H);
    let toggle = |x: f64| {
        let y = c2 + 12.0;
        Rect::new(x, y, x + TOGGLE.0, y + TOGGLE.1)
    };

    // Row 1: lo band, crossover, hi band, solo.
    let lo_x = x0;
    let xo_x = lo_x + BAND_W + GAP;
    let hi_x = xo_x + XOVER_W + GAP;
    let so_x = hi_x + BAND_W + GAP;
    let big = |x: f64, w: f64| {
        let cx = x + w / 2.0;
        let cy = c1 + 42.0;
        Rect::new(cx - GRAB, cy - GRAB, cx + GRAB, cy + GRAB)
    };
    let (lo_knob, xover_knob, hi_knob) = (big(lo_x, BAND_W), big(xo_x, XOVER_W), big(hi_x, BAND_W));
    put("lo_width", ControlKind::Knob, lo_knob);
    put("low_bypass", ControlKind::Knob, xover_knob);
    put("width", ControlKind::Knob, hi_knob);
    let lamp = |x: f64| Rect::new(x + 2.0, c1 + 2.0, x + 66.0, c1 + 18.0);
    let lamps = [lamp(lo_x), lamp(xo_x), lamp(hi_x)];
    let corr = |x: f64| Rect::new(x + 10.0, c1 + 101.0, x + BAND_W - 10.0, c1 + 109.0);
    let corr = [corr(lo_x), corr(hi_x)];
    let solo = [0.0, 1.0, 2.0].map(|n| {
        let y = c1 + 4.0 + 40.0 * n;
        Rect::new(so_x, y, so_x + SOLO_W, y + 32.0)
    });
    let solo_cell = solo[0].union(solo[2]);
    put("solo", ControlKind::Choice, solo_cell);

    // Row 2: polarity, rotation, balance, mode, output.
    let pol = x0;
    put("invert_l", ControlKind::Toggle, toggle(pol));
    put("invert_r", ControlKind::Toggle, toggle(pol + 48.0));
    let rot = x0 + 128.0;
    put("rotate", ControlKind::Knob, knob(rot));
    put("rotate_hz", ControlKind::Knob, knob(rot + 60.0));
    let bal = x0 + 270.0;
    put("balance", ControlKind::Knob, knob(bal));
    let ms = x0 + 366.0;
    put(
        "mode",
        ControlKind::Choice,
        Rect::new(ms, c2 + 10.0, ms + CHOICE.0, c2 + 10.0 + CHOICE.1),
    );
    let out = x0 + 482.0;
    put("mono", ControlKind::Toggle, toggle(out + 6.0));

    // The back face: four groups, each CV jack with its trim under it.
    let socket = |k: &str| desc.sockets.iter().position(|s| s.key == k);
    let jack_y = MARGIN_TOP + TITLE_H + 6.0;
    let cell = |x: f64| Rect::new(x, jack_y, x + SOCKET_CELL.0, jack_y + SOCKET_CELL.1);
    let mut back = Vec::new();
    let mut place = |k: &str, x: f64| {
        if let Some(s) = socket(k) {
            back.push(PlacedSocket {
                socket: s,
                rect: cell(x),
            });
        }
    };
    let pitch = 54.0;
    let (in_x, out_x, cv_x, band_x) = (x0, x0 + 118.0, x0 + 238.0, x0 + 412.0);
    place("in_l", in_x);
    place("in_r", in_x + pitch);
    place("out_l", out_x);
    place("out_r", out_x + pitch);
    place("lo_width_cv", cv_x);
    place("hi_width_cv", cv_x + pitch);
    place("xover_cv", cv_x + 2.0 * pitch);
    place("lo_out_l", band_x);
    place("lo_out_r", band_x + pitch);
    place("hi_out_l", band_x + 2.0 * pitch);
    place("hi_out_r", band_x + 3.0 * pitch);
    let mut back_params = [usize::MAX; 3];
    let trims = ["lo_width_cv_trim", "hi_width_cv_trim", "xover_cv_trim"];
    for (n, trim) in trims.iter().enumerate() {
        if let Some(param) = index(trim) {
            back_params[n] = param;
            let x = cv_x + pitch * n as f64;
            let top = jack_y + SOCKET_CELL.1 + 8.0;
            front.push(PlacedControl {
                param,
                kind: ControlKind::Knob,
                rect: Rect::new(x, top, x + KNOB_W - 6.0, top + KNOB_H),
            });
        }
    }

    let title = |label: &str, a: f64, w: f64, row: usize| Section {
        label: label.to_string(),
        rect: Rect::new(a, ROW_Y[row], a + w, ROW_Y[row] + TITLE_H),
    };
    let front_sections = vec![
        title("LO BAND", lo_x, BAND_W, 0),
        title("X-OVER", xo_x, XOVER_W, 0),
        title("HI BAND", hi_x, BAND_W, 0),
        title("SOLO", so_x, (x1 - so_x).min(SOLO_W), 0),
        title("POLARITY", pol, 96.0, 1),
        title("ROTATE", rot, 116.0, 1),
        title("BALANCE", bal, 56.0, 1),
        title("M/S MODE", ms, 72.0, 1),
        title("OUTPUT", out, 56.0, 1),
    ];
    let back_sections = vec![
        title("AUDIO IN", in_x, 104.0, 0),
        title("AUDIO OUT", out_x, 104.0, 0),
        title("CV IN", cv_x, 158.0, 0),
        title("BAND OUTPUTS", band_x, 212.0, 0),
    ];
    // The generic visual cells of an effect panel are the free space under the crossover readout
    // (this panel has no single plot; nothing is drawn or hit there).
    let free = Rect::new(xo_x + 2.0, c1 + 96.0, xo_x + XOVER_W - 2.0, c1 + 122.0);
    let geom = StereoGeom {
        lo_knob,
        xover_knob,
        hi_knob,
        lamps,
        corr,
        solo,
        back_params,
    };
    PanelSpec {
        height_units: STEREO_UNITS,
        samples: None,
        drum: None,
        slicer: None,
        fx: Some(FxPanel {
            kind: FxKind::StereoTool,
            visual: free,
            plot: free,
            meters: free,
            readout: free,
            aux: Rect::ZERO,
            button: None,
            character: None,
            eq: None,
            stereo: Some(geom),
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

/// The text of a jack on the back (the descriptor's names are long for a 54 px column).
pub fn socket_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "lo_width_cv" => "Lo width",
        "hi_width_cv" => "Hi width",
        "xover_cv" => "X-over",
        "lo_out_l" => "Lo L",
        "lo_out_r" => "Lo R",
        "hi_out_l" => "Hi L",
        "hi_out_r" => "Hi R",
        _ => return None,
    })
}

// ------------------------------------------------------------------ the texts

/// The readout of a width: two decimals.
pub fn width_text(width: f32) -> String {
    format!("{:.2}", width + 0.0)
}

/// The readout of the crossover: `off` with no split, Hz below 1 kHz and kHz above.
pub fn xover_text(hz: f32) -> String {
    if hz <= XOVER_OFF_HZ {
        crate::i18n::tr("off").into_owned()
    } else if hz < 1000.0 {
        format!("{hz:.0} Hz")
    } else {
        format!("{:.2} kHz", hz / 1000.0)
    }
}

/// The caption of a SOLO button.
pub fn solo_caption(value: f32) -> &'static str {
    if value > 1.5 {
        "SOLO HI BAND"
    } else if value > 0.5 {
        "SOLO LO BAND"
    } else {
        "NORMAL"
    }
}

/// Position (-1 to +1 mapped to 0..1) of a correlation along its bar.
pub fn corr_pos(corr: f32) -> f64 {
    f64::from((corr.clamp(-1.0, 1.0) + 1.0) / 2.0)
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

fn norm(info: &PanelInfo<'_>, key: &str) -> f64 {
    let Some(desc) = info.desc else { return 0.0 };
    desc.params.iter().find(|p| p.key == key).map_or(0.0, |p| {
        let v = match info.device {
            Some(d) => current_value(d, p),
            None => p.default,
        };
        f64::from(p.to_normalized(v))
    })
}

fn meter(info: &PanelInfo<'_>, i: usize) -> f32 {
    info.fx_meters.get(i).copied().unwrap_or(0.0)
}

fn arc_path(center: Point, r: f64, start: f64, sweep: f64) -> BezPath {
    Arc::new(center, (r, r), start, sweep, 0.0)
        .path_elements(0.1)
        .collect()
}

/// A lamp with its caption: lit with `color` or dark.
fn led<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, r: Rect, label: &str, lit: Option<Color>) {
    let at = pt(r.x0 + 6.0, r.center().y);
    match lit {
        Some(color) => {
            c.fill_with(Circle::new(at, 6.0), color.with_alpha(0.25));
            c.fill_with(Circle::new(at, 2.8), mix(color, Color::WHITE, 0.25));
        }
        None => c.fill_with(Circle::new(at, 2.8), theme.led_off),
    }
    small(
        c,
        theme,
        crate::i18n::tr(label),
        pt(r.x0 + 15.0, r.center().y + 2.6),
        Align::Start,
        None,
    );
}

/// The point of the sweep at `n` (0..1), `r` from the centre.
fn on_ring(center: Point, n: f64, r: f64) -> Point {
    let a = KNOB_START + n * KNOB_SWEEP;
    center + Vec2::new(a.cos(), a.sin()) * r
}

/// A big knob: value ring from `from` to `n`, a cap with its pointer.
fn big_knob<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    center: Point,
    n: f64,
    from: f64,
    accent: Color,
) {
    let stroke = |w: f64| Stroke::new(w).with_caps(Cap::Round);
    c.stroke(
        &arc_path(center, RING_R, KNOB_START, KNOB_SWEEP).into(),
        &stroke(3.2),
        &Brush::Solid(theme.knob_track),
    );
    let (a, b) = (from.min(n), from.max(n));
    if b - a > 1e-4 {
        c.stroke(
            &arc_path(
                center,
                RING_R,
                KNOB_START + a * KNOB_SWEEP,
                (b - a) * KNOB_SWEEP,
            )
            .into(),
            &stroke(3.2),
            &Brush::Solid(accent),
        );
    }
    c.fill_with(
        Circle::new(center + Vec2::new(0.0, 2.5), CAP_R + 0.5),
        Color::BLACK.with_alpha(0.55),
    );
    c.fill_with(
        Circle::new(center, CAP_R),
        vgrad(
            center.y - CAP_R,
            center.y + CAP_R,
            theme.knob_top,
            theme.knob_bottom,
        ),
    );
    c.fill_with(
        Circle::new(center, CAP_R - 3.0),
        vgrad(
            center.y - CAP_R,
            center.y + CAP_R,
            mix(theme.knob_bottom, theme.knob_top, 0.35),
            mix(theme.knob_top, Color::WHITE, 0.04),
        ),
    );
    c.stroke_with(
        Circle::new(center, CAP_R - 0.5),
        1.0,
        theme.ink.with_alpha(0.07),
    );
    let tip = on_ring(center, n, CAP_R - 2.5);
    let tail = on_ring(center, n, 4.0);
    c.stroke(
        &Line::new(tail, tip).into(),
        &stroke(2.4),
        &Brush::Solid(theme.knob_pointer),
    );
}

/// The ticks of a knob scale: `marks` are positions (0..1); the long ones are `long`.
fn ticks<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, center: Point, marks: &[(f64, bool)]) {
    for (n, long) in marks {
        let a = on_ring(center, *n, RING_R + 4.0);
        let b = on_ring(center, *n, RING_R + if *long { 8.0 } else { 6.0 });
        c.stroke_with(
            Line::new(a, b),
            1.0,
            theme.text_muted.with_alpha(if *long { 0.9 } else { 0.5 }),
        );
    }
}

/// A readout under a big knob.
fn readout<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, center: Point, text: &str, accent: Color) {
    let r = Rect::new(
        center.x - 30.0,
        center.y + 34.0,
        center.x + 30.0,
        center.y + 48.0,
    );
    lcd(c, theme, r);
    c.text(
        &TextRun::new(
            text,
            pt(r.center().x, r.center().y + 3.5),
            9.5,
            mix(accent, Color::WHITE, 0.3),
        )
        .align(Align::Center)
        .weight(700),
    );
}

/// One band: the width knob with its scale, readout, lamp and correlation bar.
#[allow(clippy::too_many_arguments, reason = "all of it is one band's picture")]
fn paint_band<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    knob: Rect,
    lamp: Rect,
    bar: Rect,
    width: (f32, f64),
    meters: (f32, f32),
    accent: Color,
) {
    let center = knob.center();
    let ok = hsl(140.0, 0.55, 0.5);
    led(c, theme, lamp, "ACTIVE", (meters.1 >= 0.5).then_some(ok));
    ticks(
        c,
        theme,
        center,
        &[
            (0.0, true),
            (0.25, false),
            (0.5, true),
            (0.75, false),
            (1.0, true),
        ],
    );
    big_knob(c, theme, center, width.1, 0.5, accent);
    small(
        c,
        theme,
        crate::i18n::tr("ORIGINAL"),
        pt(center.x, center.y - RING_R - 10.0),
        Align::Center,
        None,
    );
    let start = on_ring(center, 0.0, RING_R + 8.0);
    let end = on_ring(center, 1.0, RING_R + 8.0);
    small(
        c,
        theme,
        crate::i18n::tr("MONO"),
        pt(start.x - 3.0, start.y + 3.0),
        Align::End,
        None,
    );
    small(
        c,
        theme,
        crate::i18n::tr("WIDE"),
        pt(end.x + 3.0, end.y + 3.0),
        Align::Start,
        None,
    );
    readout(c, theme, center, &width_text(width.0), accent);
    paint_corr(c, theme, bar, meters.0);
}

/// A correlation bar: -1 to +1 with the centre marked; the bar grows from the centre.
fn paint_corr<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, bar: Rect, corr: f32) {
    small(
        c,
        theme,
        crate::i18n::tr("CORRELATION"),
        pt(bar.center().x, bar.y0 - 3.0),
        Align::Center,
        None,
    );
    c.fill_with(bar, Color::BLACK.with_alpha(0.55));
    let mid = bar.center().x;
    let x = bar.x0 + bar.width() * corr_pos(corr);
    let color = if corr < 0.0 {
        hsl(6.0, 0.75, 0.55)
    } else {
        hsl(140.0, 0.55, 0.5)
    };
    let (a, b) = (mid.min(x), mid.max(x));
    if b - a > 0.5 {
        c.fill_with(Rect::new(a, bar.y0, b, bar.y1), color);
    }
    c.stroke_with(
        Rect::new(bar.x0 - 0.5, bar.y0 - 0.5, bar.x1 + 0.5, bar.y1 + 0.5),
        1.0,
        Color::WHITE.with_alpha(0.08),
    );
    c.stroke_with(
        Line::new(pt(mid, bar.y0 - 2.0), pt(mid, bar.y1 + 2.0)),
        1.0,
        theme.text,
    );
    for (pos, caption) in [(0.0, "-1"), (0.5, "0"), (1.0, "+1")] {
        let tx = bar.x0 + bar.width() * pos;
        c.stroke_with(
            Line::new(pt(tx, bar.y1 + 1.0), pt(tx, bar.y1 + 4.0)),
            1.0,
            theme.text_muted.with_alpha(0.6),
        );
        small(
            c,
            theme,
            caption,
            pt(tx, bar.y1 + 12.0),
            Align::Center,
            None,
        );
    }
}

/// Paints the front: the three knobs with their scales, lamps and bars, and the SOLO buttons.
pub fn paint_front<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: &FxPanel,
    info: &PanelInfo<'_>,
) {
    let Some(g) = fx.stereo else { return };
    paint_band(
        c,
        theme,
        g.lo_knob,
        g.lamps[0],
        g.corr[0],
        (value(info, "lo_width"), norm(info, "lo_width")),
        (meter(info, METER_LO_CORR), meter(info, METER_LO_ACTIVE)),
        info.accent,
    );
    paint_band(
        c,
        theme,
        g.hi_knob,
        g.lamps[2],
        g.corr[1],
        (value(info, "width"), norm(info, "width")),
        (meter(info, METER_HI_CORR), meter(info, METER_HI_ACTIVE)),
        info.accent,
    );

    // The crossover.
    let center = g.xover_knob.center();
    let hz = value(info, "low_bypass");
    led(
        c,
        theme,
        g.lamps[1],
        "SPLIT",
        (meter(info, METER_SPLIT) >= 0.5).then(|| hsl(140.0, 0.55, 0.5)),
    );
    let pos = |hz: f64| (hz / 20.0).ln() / (6000.0f64 / 20.0).ln();
    ticks(
        c,
        theme,
        center,
        &[
            (0.0, true),
            (pos(100.0), false),
            (pos(1000.0), false),
            (1.0, true),
        ],
    );
    big_knob(c, theme, center, norm(info, "low_bypass"), 0.0, info.accent);
    for (hz, caption) in [(100.0, "100"), (1000.0, "1k")] {
        let at = on_ring(center, pos(hz), RING_R + 14.0);
        small(c, theme, caption, pt(at.x, at.y + 3.0), Align::Center, None);
    }
    let start = on_ring(center, 0.0, RING_R + 8.0);
    let end = on_ring(center, 1.0, RING_R + 8.0);
    small(
        c,
        theme,
        crate::i18n::tr("OFF"),
        pt(start.x - 3.0, start.y + 3.0),
        Align::End,
        None,
    );
    small(
        c,
        theme,
        "6k",
        pt(end.x + 3.0, end.y + 3.0),
        Align::Start,
        None,
    );
    readout(c, theme, center, &xover_text(hz), info.accent);

    // The SOLO radio buttons.
    let solo = value(info, "solo");
    for (r, v) in g.solo.iter().zip(SOLO_VALUES) {
        let on = (solo - v).abs() < 0.5;
        let b = r.inset(-0.0);
        c.fill_with(
            RoundedRect::from_rect(b + Vec2::new(0.0, 1.0), 4.0),
            Color::BLACK.with_alpha(0.5),
        );
        c.fill_with(
            RoundedRect::from_rect(b, 4.0),
            vgrad(
                b.y0,
                b.y1,
                mix(theme.button, Color::WHITE, 0.08),
                theme.button,
            ),
        );
        let led = pt(b.x0 + 11.0, b.center().y);
        if on {
            c.fill_with(Circle::new(led, 6.5), info.accent.with_alpha(0.25));
            c.fill_with(Circle::new(led, 3.0), mix(info.accent, Color::WHITE, 0.25));
        } else {
            c.fill_with(Circle::new(led, 3.0), theme.led_off);
        }
        small(
            c,
            theme,
            crate::i18n::tr(solo_caption(v)),
            pt(b.x0 + 22.0, b.center().y + 2.6),
            Align::Start,
            Some(if on { theme.text } else { theme.text_muted }),
        );
    }
}
