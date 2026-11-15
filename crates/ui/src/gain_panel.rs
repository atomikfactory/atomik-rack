//! The rack panel of `atomik.gain-tool`: a 3 U strip that reads left to right and top to bottom
//! like a small channel strip.
//!
//! ```text
//! MUTE                    INPUT        GAIN                                   OUTPUT
//! o SIGNAL [MUTE] (FADE)  L ======     |---------------[=]-------------|      L ======
//! o CV                    R ====       -60 -48 -36 -24 -12  0  +12 +24        R ====
//! o FADING
//! MODE                              POLARITY                 PEAK HOLD
//! [mode] (BLEND) [fader mode]       [Ø L] [Ø R] [MONO]       [ -3.2 dB ] [RESET] [AUTO]
//! PAN                              IMAGE
//! (PAN L) (PAN R) [law]            (BALANCE) (WIDTH)
//! ```
//!
//! Everything but the meters, the LEDs, the peak readout and the fader is an ordinary placed
//! control (knobs, switches, selectors), so the generic value, automation and MIDI machinery
//! of the rack applies to it. The BLEND knob is only there while the mode mixes in the aux
//! input ([`hidden`]). RESET is a momentary button: it does not store anything, the app sends
//! the device 1 and then 0 ([`crate::UiAction::PulseParam`]).
//!
//! # The fader
//!
//! The `gain` param is stored in dB from -60 to +24 whatever the fader mode. **Trim** draws
//! that whole range along the track (marks every 12 dB, 0 dB longer). **VCA** draws only
//! -inf..0 dB: the left end is -inf (the device mutes at `gain <= -60`), the right end 0 dB, and
//! the cap never goes beyond it (the device clamps a stored boost). Both are linear in dB, so
//! a drag moves the value by the same dB per pixel everywhere on the track.
//!
//! The back is the nine sockets in four groups, with the two CV trim knobs under the gain and
//! the balance CV jacks.

use crate::autolayout::{
    CONTENT_X, FxPanel, MARGIN_TOP, PanelSpec, PlacedControl, PlacedSocket, SOCKET_CELL, Section,
    TITLE_H,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx::{FxKind, level_norm};
use crate::fx_paint::{lcd, meter_color, small};
use crate::paint::{PanelInfo, vgrad};
use crate::theme::{Theme, mix};
use crate::widget::current_value;
use atomik_device_sdk::{ControlKind, DeviceDescriptor};
use atomik_model::RackDevice;
use vello::kurbo::{Circle, Line, Rect, RoundedRect, Vec2};
use vello::peniko::Color;

/// The device type of this panel.
pub const GAIN_TOOL_TYPE: &str = "atomik.gain-tool";
/// Height of the panel in rack units.
pub const GAIN_UNITS: u8 = 3;

/// The meter values (`atomik_devices::gain_tool::METER_*`; a test pins them together).
pub const METER_IN_L: usize = 0;
/// Input peak, right.
pub const METER_IN_R: usize = 1;
/// Output peak, left.
pub const METER_OUT_L: usize = 2;
/// Output peak, right.
pub const METER_OUT_R: usize = 3;
/// The held output peak, dB.
pub const METER_HOLD: usize = 4;
/// Signal present.
pub const METER_SIGNAL: usize = 5;
/// CV activity.
pub const METER_CV: usize = 6;
/// A mute fade is running.
pub const METER_FADING: usize = 7;

/// The lowest stored gain, dB (the bottom of the fader in VCA mode, where it reads -inf).
pub const GAIN_MIN_DB: f32 = -60.0;
/// The highest stored gain, dB.
pub const GAIN_MAX_DB: f32 = 24.0;
/// The meters' scale: the left end is this level, dB.
pub const METER_LOW_DB: f32 = -60.0;
/// The held peak reads this (dB) or less while nothing has passed.
pub const HOLD_NONE_DB: f32 = -119.5;

/// The key of the momentary RESET param.
pub const PEAK_RESET_KEY: &str = "peak_reset";

/// `Choice` value of `fader_mode` for the VCA fader.
const FADER_VCA: f32 = 1.0;
/// `Choice` value of `mode` that does not use the aux input.
const MODE_GAIN: f32 = 0.0;

/// The rows of the panel: title top.
const ROW_Y: [f64; 3] = [MARGIN_TOP, MARGIN_TOP + 73.0, MARGIN_TOP + 146.0];
/// Height of a knob cell.
const KNOB_H: f64 = 58.0;
const KNOB_W: f64 = 48.0;
const TOGGLE: (f64, f64) = (44.0, 28.0);
const CHOICE: (f64, f64) = (62.0, 36.0);
const METER_W: f64 = 80.0;

/// Geometry of the gain tool's own parts (front, panel coordinates, and the back trims).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainGeom {
    /// The SIGNAL, CV and FADING lamps (a lamp and its caption), in that order.
    pub leds: [Rect; 3],
    /// The input meter (two bars, a scale).
    pub input: Rect,
    /// The output meter.
    pub output: Rect,
    /// The fader cell (the control rect of `gain`).
    pub fader: Rect,
    /// The peak hold readout.
    pub readout: Rect,
    /// Descriptor indices of the controls that sit on the back face (the two CV trims).
    pub back_params: [usize; 2],
}

impl GainGeom {
    /// The fader's track: left end, right end and y of its centre line.
    pub fn travel(&self) -> (f64, f64, f64) {
        (
            self.fader.x0 + 8.0,
            self.fader.x1 - 8.0,
            self.fader.y0 + 27.0,
        )
    }
}

/// The two bars of a meter: left channel on top.
fn bars_of(m: Rect) -> [Rect; 2] {
    let (x0, x1) = (m.x0 + 10.0, m.x1 - 2.0);
    [
        Rect::new(x0, m.y0 + 4.0, x1, m.y0 + 15.0),
        Rect::new(x0, m.y0 + 19.0, x1, m.y0 + 30.0),
    ]
}

/// Position (0..1) of `db` along a meter scale.
pub fn meter_pos(db: f32) -> f64 {
    level_norm(db, METER_LOW_DB, 0.0)
}

/// The panel: 3 U with the strip described in the module docs; the back has the nine sockets
/// and the two trims.
pub fn gain_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let x0 = CONTENT_X.0;
    let x1 = CONTENT_X.1;
    let index = |k: &str| desc.params.iter().position(|p| p.key == k);
    let content = |row: usize| ROW_Y[row] + TITLE_H;
    let mut front: Vec<PlacedControl> = Vec::new();
    let mut put = |key: &str, kind: ControlKind, rect: Rect| {
        if let Some(param) = index(key) {
            front.push(PlacedControl { param, kind, rect });
        }
    };
    let knob = |x: f64, row: usize| Rect::new(x, content(row), x + KNOB_W, content(row) + KNOB_H);
    let toggle = |x: f64, row: usize| {
        let y = content(row) + 12.0;
        Rect::new(x, y, x + TOGGLE.0, y + TOGGLE.1)
    };
    let choice = |x: f64, row: usize| {
        let y = content(row) + 10.0;
        Rect::new(x, y, x + CHOICE.0, y + CHOICE.1)
    };

    // Row 1: mute, input meter, fader, output meter.
    let c1 = content(0);
    let leds = [0.0, 1.0, 2.0].map(|n| {
        let cy = c1 + 10.0 + 18.0 * n;
        Rect::new(x0, cy - 8.0, x0 + 46.0, cy + 8.0)
    });
    put("mute", ControlKind::Toggle, toggle(x0 + 50.0, 0));
    put("fade", ControlKind::Knob, knob(x0 + 98.0, 0));
    let input = Rect::new(x0 + 156.0, c1, x0 + 156.0 + METER_W, c1 + KNOB_H);
    let fader = Rect::new(input.x1 + 12.0, c1, x1 - METER_W - 12.0, c1 + KNOB_H);
    let output = Rect::new(x1 - METER_W, c1, x1, c1 + KNOB_H);
    put("gain", ControlKind::Fader, fader);

    // Row 2: mode, polarity, peak hold.
    put("mode", ControlKind::Choice, choice(x0, 1));
    put("blend", ControlKind::Knob, knob(x0 + 68.0, 1));
    put("fader_mode", ControlKind::Choice, choice(x0 + 122.0, 1));
    let pol = x0 + 196.0;
    put("invert_l", ControlKind::Toggle, toggle(pol, 1));
    put("invert_r", ControlKind::Toggle, toggle(pol + 48.0, 1));
    put("mono", ControlKind::Toggle, toggle(pol + 96.0, 1));
    let hold = x0 + 348.0;
    let c2 = content(1);
    let readout = Rect::new(hold, c2 + 12.0, hold + 72.0, c2 + 42.0);
    put("peak_reset", ControlKind::Toggle, toggle(hold + 80.0, 1));
    put("peak_auto", ControlKind::Toggle, toggle(hold + 128.0, 1));

    // Row 3: pan and image.
    put("pan_l", ControlKind::Knob, knob(x0, 2));
    put("pan_r", ControlKind::Knob, knob(x0 + 52.0, 2));
    put("pan_law", ControlKind::Choice, choice(x0 + 106.0, 2));
    put("pan", ControlKind::Knob, knob(x0 + 184.0, 2));
    put("width", ControlKind::Knob, knob(x0 + 236.0, 2));

    // The back face: four socket groups, each CV jack with its trim under it.
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
    let (aux_x, cv_x, out_x) = (x0 + 124.0, x0 + 248.0, x1 - 110.0);
    place("in_l", x0);
    place("in_r", x0 + 60.0);
    place("aux_l", aux_x);
    place("aux_r", aux_x + 60.0);
    place("gain_cv_in", cv_x);
    place("pan_cv_in", cv_x + 60.0);
    place("mute_cv_in", cv_x + 120.0);
    place("out_l", out_x);
    place("out_r", out_x + 60.0);
    let mut back_params = [usize::MAX; 2];
    for (n, (trim, x)) in [("gain_cv_trim", cv_x), ("pan_cv_trim", cv_x + 60.0)]
        .iter()
        .enumerate()
    {
        if let Some(param) = index(trim) {
            back_params[n] = param;
            let top = jack_y + SOCKET_CELL.1 + 8.0;
            front.push(PlacedControl {
                param,
                kind: ControlKind::Knob,
                rect: Rect::new(x + 1.0, top, x + 1.0 + KNOB_W, top + KNOB_H),
            });
        }
    }

    let title = |label: &str, a: f64, b: f64, row: usize| Section {
        label: label.to_string(),
        rect: Rect::new(a, ROW_Y[row], b, ROW_Y[row] + TITLE_H),
    };
    let front_sections = vec![
        title("MUTE", x0, x0 + 148.0, 0),
        title("INPUT", input.x0, input.x1, 0),
        title("GAIN", fader.x0, fader.x1, 0),
        title("OUTPUT", output.x0, output.x1, 0),
        title("MODE", x0, x0 + 184.0, 1),
        title("POLARITY", pol, pol + 140.0, 1),
        title("PEAK HOLD", hold, hold + 172.0, 1),
        title("PAN", x0, x0 + 168.0, 2),
        title("IMAGE", x0 + 184.0, x0 + 284.0, 2),
    ];
    let back_sections = vec![
        title("AUDIO IN", x0, x0 + 110.0, 0),
        title("AUX IN", aux_x, aux_x + 110.0, 0),
        title("CV IN", cv_x, cv_x + 170.0, 0),
        title("AUDIO OUT", out_x, out_x + 110.0, 0),
    ];
    let geom = GainGeom {
        leds,
        input,
        output,
        fader,
        readout,
        back_params,
    };
    PanelSpec {
        height_units: GAIN_UNITS,
        samples: None,
        drum: None,
        slicer: None,
        fx: Some(FxPanel {
            kind: FxKind::GainTool,
            visual: readout,
            plot: readout,
            meters: output,
            readout,
            aux: Rect::ZERO,
            button: None,
            character: None,
            eq: None,
            gain: Some(geom),
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

// ------------------------------------------------------------------ the fader's maths

/// Whether the stored fader mode is VCA.
pub fn is_vca(mode: f32) -> bool {
    mode >= FADER_VCA - 0.5
}

/// The largest normalised position (in the `gain` param's own 0..1 range) the cap can take:
/// everything in Trim, up to 0 dB in VCA.
pub fn max_norm(vca: bool) -> f64 {
    if vca {
        f64::from(-GAIN_MIN_DB / (GAIN_MAX_DB - GAIN_MIN_DB))
    } else {
        1.0
    }
}

/// The cap's position along the track, 0 (left) to 1 (right), for a stored gain in dB.
pub fn fader_pos(gain_db: f32, vca: bool) -> f64 {
    let db = if gain_db.is_finite() {
        gain_db
    } else {
        GAIN_MIN_DB
    };
    if vca {
        f64::from((db.min(0.0) - GAIN_MIN_DB) / -GAIN_MIN_DB).clamp(0.0, 1.0)
    } else {
        f64::from((db.clamp(GAIN_MIN_DB, GAIN_MAX_DB) - GAIN_MIN_DB) / (GAIN_MAX_DB - GAIN_MIN_DB))
    }
}

/// The gain in dB at a position along the track (the inverse of [`fader_pos`]).
pub fn gain_at(pos: f64, vca: bool) -> f32 {
    let pos = pos.clamp(0.0, 1.0) as f32;
    if vca {
        GAIN_MIN_DB + pos * -GAIN_MIN_DB
    } else {
        GAIN_MIN_DB + pos * (GAIN_MAX_DB - GAIN_MIN_DB)
    }
}

/// The text of a gain: one decimal, `-inf` for a VCA fader at its bottom, and the clamped value
/// the VCA actually applies.
pub fn fader_text(gain_db: f32, vca: bool) -> String {
    if vca {
        if gain_db <= GAIN_MIN_DB {
            return "-inf dB".to_string();
        }
        return format!("{:.1} dB", gain_db.min(0.0) + 0.0);
    }
    let g = gain_db.clamp(GAIN_MIN_DB, GAIN_MAX_DB);
    if g > 0.0 {
        format!("+{g:.1} dB")
    } else {
        format!("{:.1} dB", g + 0.0)
    }
}

/// The marks of the scale: position along the track, caption and whether it is a long one.
pub fn scale_marks(vca: bool) -> Vec<(f64, String, bool)> {
    let dbs: &[f32] = if vca {
        &[-60.0, -48.0, -36.0, -24.0, -12.0, 0.0]
    } else {
        &[-60.0, -48.0, -36.0, -24.0, -12.0, 0.0, 12.0, 24.0]
    };
    dbs.iter()
        .map(|&db| {
            let long = db.abs() < 0.5;
            let text = if vca && db <= GAIN_MIN_DB {
                "-inf".to_string()
            } else if db > 0.0 {
                format!("+{db:.0}")
            } else {
                format!("{db:.0}")
            };
            (fader_pos(db, vca), text, long)
        })
        .collect()
}

/// The held peak as the readout shows it: `--` while nothing has passed.
pub fn hold_text(db: f32) -> String {
    if db <= HOLD_NONE_DB {
        "--".to_string()
    } else {
        format!("{:.1} dB", db + 0.0)
    }
}

/// Whether the control of param `param` (descriptor index) is out of sight now: the BLEND knob
/// while the mode does not mix in the aux input.
pub fn hidden(desc: &DeviceDescriptor, device: Option<&RackDevice>, param: usize) -> bool {
    if desc.type_id != GAIN_TOOL_TYPE || desc.params.get(param).is_none_or(|p| p.key != "blend") {
        return false;
    }
    let mode = desc
        .param_by_key("mode")
        .map_or(0.0, |p| device.map_or(p.default, |d| current_value(d, p)));
    mode < MODE_GAIN + 0.5
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

/// A two-bar meter with its scale: `db` are the shown peaks (left, right).
fn paint_meter<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, m: Rect, db: [f32; 2]) {
    for ((r, level), name) in bars_of(m).into_iter().zip(db).zip(["L", "R"]) {
        bar_h(c, r, meter_pos(level), meter_color(level));
        small(
            c,
            theme,
            name,
            pt(m.x0 + 3.0, r.y1 - 2.0),
            Align::Center,
            None,
        );
    }
    let (a, b) = (m.x0 + 10.0, m.x1 - 2.0);
    for (db, caption) in [
        (-60.0f32, true),
        (-40.0, true),
        (-20.0, true),
        (-10.0, false),
        (0.0, true),
    ] {
        let x = a + (b - a) * meter_pos(db);
        c.stroke_with(
            Line::new(pt(x, m.y0 + 33.0), pt(x, m.y0 + 36.0)),
            1.0,
            theme.text_muted.with_alpha(0.6),
        );
        // The -10 mark has no caption: it would touch its neighbours.
        if caption {
            small(
                c,
                theme,
                format!("{db:.0}"),
                pt(x, m.y0 + 45.0),
                Align::Center,
                None,
            );
        }
    }
}

/// A horizontal bar filled from the left to `norm`.
fn bar_h<C: Canvas + ?Sized>(c: &mut C, r: Rect, norm: f64, color: Color) {
    c.fill_with(r, Color::BLACK.with_alpha(0.55));
    let w = (r.width() * norm.clamp(0.0, 1.0)).round();
    if w > 0.0 {
        c.fill_with(Rect::new(r.x0, r.y0, r.x0 + w, r.y1), color);
    }
    c.stroke_with(
        Rect::new(r.x0 - 0.5, r.y0 - 0.5, r.x1 + 0.5, r.y1 + 0.5),
        1.0,
        Color::WHITE.with_alpha(0.08),
    );
}

/// The fader: scale, track, cap and the value.
fn paint_fader<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    g: &GainGeom,
    gain_db: f32,
    vca: bool,
    accent: Color,
) {
    let (x0, x1, y) = g.travel();
    for (pos, text, long) in scale_marks(vca) {
        let x = x0 + (x1 - x0) * pos;
        let top = y + 11.0;
        c.stroke_with(
            Line::new(pt(x, top), pt(x, top + if long { 6.0 } else { 4.0 })),
            1.0,
            theme.text_muted.with_alpha(if long { 0.9 } else { 0.5 }),
        );
        small(c, theme, text, pt(x, top + 14.0), Align::Center, None);
    }
    c.fill_with(
        RoundedRect::new(x0 - 2.0, y - 2.5, x1 + 2.0, y + 2.5, 2.5),
        theme.knob_track,
    );
    let x = x0 + (x1 - x0) * fader_pos(gain_db, vca);
    c.stroke_with(Line::new(pt(x0, y), pt(x, y)), 2.0, accent);
    let cap = Rect::new(x - 5.5, y - 9.0, x + 5.5, y + 9.0);
    c.fill_with(
        RoundedRect::from_rect(cap + Vec2::new(0.0, 1.5), 2.0),
        Color::BLACK.with_alpha(0.5),
    );
    c.fill_with(
        RoundedRect::from_rect(cap, 2.0),
        vgrad(cap.y0, cap.y1, theme.knob_top, theme.knob_bottom),
    );
    c.stroke_with(
        Line::new(pt(x, cap.y0 + 3.0), pt(x, cap.y1 - 3.0)),
        1.2,
        theme.knob_pointer,
    );
    c.text(
        &TextRun::new(
            fader_text(gain_db, vca),
            pt(g.fader.x1 - 4.0, g.fader.y0 + 8.0),
            9.5,
            theme.text,
        )
        .align(Align::End)
        .weight(700),
    );
}

/// Paints the front: lamps, meters, fader and the peak hold readout.
pub fn paint_front<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: &FxPanel,
    info: &PanelInfo<'_>,
) {
    let Some(g) = fx.gain else { return };
    let lit = |i: usize, color: Color| (meter(info, i, 0.0) >= 0.5).then_some(color);
    let ok = crate::theme::hsl(140.0, 0.55, 0.5);
    led(c, theme, g.leds[0], "SIGNAL", lit(METER_SIGNAL, ok));
    led(c, theme, g.leds[1], "CV", lit(METER_CV, theme.cable_cv));
    led(
        c,
        theme,
        g.leds[2],
        "FADING",
        lit(METER_FADING, theme.warning),
    );

    let floor = crate::fx::LEVEL_FLOOR_DB;
    paint_meter(
        c,
        theme,
        g.input,
        [
            meter(info, METER_IN_L, floor),
            meter(info, METER_IN_R, floor),
        ],
    );
    paint_meter(
        c,
        theme,
        g.output,
        [
            meter(info, METER_OUT_L, floor),
            meter(info, METER_OUT_R, floor),
        ],
    );

    let vca = is_vca(value(info, "fader_mode"));
    paint_fader(c, theme, &g, value(info, "gain"), vca, info.accent);

    let hold = meter(info, METER_HOLD, -120.0);
    lcd(c, theme, g.readout);
    let color = if hold > 0.0 {
        theme.clip
    } else {
        mix(info.accent, Color::WHITE, 0.3)
    };
    c.text(
        &TextRun::new(
            hold_text(hold),
            pt(g.readout.center().x, g.readout.center().y + 4.0),
            11.0,
            color,
        )
        .align(Align::Center)
        .weight(700),
    );
}
