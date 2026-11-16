//! The rack panel of `atomik.saturation`: a 1 U strip built around one big knob.
//!
//! ```text
//! SATURATION             SATURATION TYPE        MIX      OUTPUT                OVERSAMPLING
//!   .:'':.                 o--  KEEP HIGH      (Mix)    (Output)  [AUTO LEVEL]   [ 2x ]
//!  :  O  :  (*)  DRIVE     |   NEUTRAL
//! OFF `...'                o--  KEEP LOW
//! ```
//!
//! The big SATURATION knob has a tick scale (eleven ticks, the ones up to the knob accent-lit) and
//! the word OFF at its minimum, where the device is bit-exact transparent. The lamp beside it
//! follows the device's DRIVE meter (0..1, how hard the curve is working): dark at 0, then
//! brighter and with a wider glow. The TYPE switch is vertical with three positions, KEEP HIGH on
//! top, NEUTRAL, KEEP LOW at the bottom (the `type` choice runs 0 = keep high, 1 = neutral, 2 =
//! keep low, so the top is the lowest value). A press on a label or on the switch picks that
//! position and a drag steps through them (one undo step, Esc puts it back); the wheel and the
//! arrow keys step without wrapping, up being up ([`crate::RackView`]).
//!
//! Everything but the big knob, the lamp and the switch is an ordinary placed control (knobs,
//! a toggle and a selector), so the generic value, automation and MIDI machinery of the rack
//! applies to it.
//!
//! The back is the audio in, the CV in and the audio out. At 1 U there is no room under the CV
//! jack, so its trim knob sits beside it.

use crate::autolayout::{
    CONTENT_X, FxPanel, MARGIN_TOP, PanelSpec, PlacedControl, PlacedSocket, SOCKET_CELL, Section,
    TITLE_H,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx::FxKind;
use crate::fx_paint::small;
use crate::paint::{
    KNOB_START, KNOB_SWEEP, PanelInfo, arc_path, round_stroke, solid, vgrad, with_alpha,
};
use crate::theme::{Theme, hsl, mix};
use crate::widget::current_value;
use atomik_device_sdk::{ControlKind, DeviceDescriptor};
use vello::kurbo::{Circle, Line, Point, Rect, RoundedRect, Vec2};
use vello::peniko::Color;

/// The device type of this panel.
pub const SATURATION_TYPE: &str = "atomik.saturation";
/// Height of the panel in rack units.
pub const SATURATION_UNITS: u8 = 1;

/// The drive lamp's meter value, 0..1 (`atomik_devices::saturation::meter::DRIVE`; a test pins
/// it).
pub const METER_DRIVE: usize = 0;
/// The input peak (`atomik_devices::saturation::meter::INPUT`).
pub const METER_INPUT: usize = 1;
/// The output peak (`atomik_devices::saturation::meter::OUTPUT`).
pub const METER_OUTPUT: usize = 2;

/// The labels of the type switch, top to bottom (the `type` choice's values 0, 1, 2).
pub const TYPE_LABELS: [&str; 3] = ["KEEP HIGH", "NEUTRAL", "KEEP LOW"];

const KNOB_W: f64 = 48.0;
const KNOB_H: f64 = 58.0;
/// Radius of the big knob's body, its ring and the ticks' inner end.
const BIG_R: f64 = 17.0;
const BIG_RING_R: f64 = 22.0;
const TICK_R: f64 = 25.5;
/// Ticks of the scale (the first and the last are the ends, the middle one is long too).
const TICKS: usize = 11;

/// Geometry of the saturation panel's own parts (front, panel coordinates, and the back trim).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SaturationGeom {
    /// The big knob's cell (the control rect of `saturation`).
    pub knob: Rect,
    /// The drive lamp (and its caption).
    pub lamp: Rect,
    /// The type switch with its labels (the control rect of `type`).
    pub switch: Rect,
    /// Descriptor index of the control that sits on the back face (the CV trim).
    pub back_params: [usize; 1],
}

impl SaturationGeom {
    /// The centre of the big knob.
    pub fn knob_center(&self) -> Point {
        pt(self.knob.center().x, self.knob.y0 + 29.0)
    }

    /// The centre of the lamp's light.
    pub fn lamp_center(&self) -> Point {
        pt(self.lamp.center().x, self.lamp.y0 + 21.0)
    }

    /// The y of switch position `i` (0 = keep high, on top).
    pub fn switch_y(&self, i: usize) -> f64 {
        self.switch.y0 + self.switch.height() * (i as f64 + 0.5) / 3.0
    }

    /// The x of the switch's slot.
    pub fn switch_x(&self) -> f64 {
        self.switch.x0 + 12.0
    }
}

/// The switch position (0 = keep high .. 2 = keep low) at height `y` (panel coordinates) of the
/// switch cell `switch`: three equal bands, so a press anywhere on a label or on the switch picks
/// the nearest one. Heights outside the cell give the first or the last.
pub fn type_at(switch: Rect, y: f64) -> u8 {
    let t = (y - switch.y0) / switch.height().max(1.0);
    ((t * 3.0).floor().clamp(0.0, 2.0)) as u8
}

/// The `type` value after a step of `notches` (positive = up, towards keep high), without
/// wrapping.
pub fn type_step(value: f32, notches: f64) -> f32 {
    let v = value.round().clamp(0.0, 2.0);
    let up = notches.round() as f32;
    (v - up).clamp(0.0, 2.0)
}

/// Whether the control of descriptor index `param` is painted by this panel itself (the big knob
/// and the type switch) rather than by the generic cells.
pub fn own_painted(desc: &DeviceDescriptor, param: usize) -> bool {
    desc.type_id == SATURATION_TYPE
        && desc
            .params
            .get(param)
            .is_some_and(|p| matches!(p.key, "saturation" | "type"))
}

/// The short name a jack of this panel shows (the generic one would be cut to fit its cell).
pub fn socket_label(key: &str) -> Option<&'static str> {
    (key == "saturation_cv_in").then_some("Sat. CV")
}

/// The text of the saturation knob: `OFF` at the bottom, else a whole percent.
pub fn knob_text(value: f32) -> String {
    if value <= 0.0 {
        "OFF".to_string()
    } else {
        format!("{:.0} %", (value * 100.0).clamp(0.0, 100.0))
    }
}

/// The lamp's brightness 0..1 for a published drive meter.
pub fn lamp_level(drive: f32) -> f32 {
    if drive.is_finite() {
        drive.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The panel: 1 U with the row described in the module docs; the back has the five sockets and
/// the CV trim.
pub fn saturation_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let x0 = CONTENT_X.0;
    let x1 = CONTENT_X.1;
    let index = |k: &str| desc.params.iter().position(|p| p.key == k);
    let c = MARGIN_TOP + TITLE_H;
    let title_y = MARGIN_TOP;
    let mut front: Vec<PlacedControl> = Vec::new();
    let mut put = |key: &str, kind: ControlKind, rect: Rect| {
        if let Some(param) = index(key) {
            front.push(PlacedControl { param, kind, rect });
        }
    };
    let cell = |x: f64, w: f64| Rect::new(x, c, x + w, c + KNOB_H);
    let toggle = |x: f64, w: f64| Rect::new(x, c + 12.0, x + w, c + 12.0 + 28.0);
    let choice = |x: f64, w: f64| Rect::new(x, c + 10.0, x + w, c + 10.0 + 36.0);

    // The hit rect is the knob's own 58 x 58 (the scale included), centred in its 110 px slot;
    // the OFF caption beside it is a label, not a target.
    let knob = cell(x0 + 26.0, 58.0);
    let lamp = cell(x0 + 116.0, 32.0);
    let switch = cell(x0 + 168.0, 122.0);
    put("saturation", ControlKind::Knob, knob);
    put("type", ControlKind::Choice, switch);
    let mix_x = x0 + 318.0;
    put("mix", ControlKind::Knob, cell(mix_x, KNOB_W));
    put("output", ControlKind::Knob, cell(mix_x + 60.0, KNOB_W));
    put(
        "auto_level",
        ControlKind::Toggle,
        toggle(mix_x + 120.0, 52.0),
    );
    let os_x = mix_x + 190.0;
    put("oversampling", ControlKind::Choice, choice(os_x, 92.0));

    // The back: audio in, the CV in with its trim beside it, audio out.
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
    let (cv_x, out_x) = (x0 + 250.0, x1 - 110.0);
    place("in_l", x0);
    place("in_r", x0 + 60.0);
    place("saturation_cv_in", cv_x);
    place("out_l", out_x);
    place("out_r", out_x + 60.0);
    let mut back_params = [usize::MAX; 1];
    if let Some(param) = index("saturation_cv_trim") {
        back_params[0] = param;
        front.push(PlacedControl {
            param,
            kind: ControlKind::Knob,
            rect: cell(cv_x + 62.0, KNOB_W),
        });
    }

    let title = |label: &str, a: f64, b: f64| Section {
        label: label.to_string(),
        rect: Rect::new(a, title_y, b, title_y + TITLE_H),
    };
    let front_sections = vec![
        title("SATURATION", x0, lamp.x1),
        title("SATURATION TYPE", switch.x0, switch.x1),
        title("MIX", mix_x, mix_x + KNOB_W),
        title("OUTPUT", mix_x + 60.0, mix_x + 60.0 + 124.0),
        // Wider than the selector: the French name is 18 letters.
        title("OVERSAMPLING", os_x, x1),
    ];
    let back_sections = vec![
        title("AUDIO IN", x0, x0 + 110.0),
        title("CV IN", cv_x, cv_x + 62.0 + KNOB_W),
        title("AUDIO OUT", out_x, out_x + 110.0),
    ];
    let geom = SaturationGeom {
        knob,
        lamp,
        switch,
        back_params,
    };
    PanelSpec {
        height_units: SATURATION_UNITS,
        samples: None,
        drum: None,
        slicer: None,
        fx: Some(FxPanel {
            kind: FxKind::Saturation,
            visual: lamp,
            plot: lamp,
            meters: lamp,
            readout: lamp,
            aux: Rect::ZERO,
            button: None,
            character: None,
            eq: None,
            stereo: None,
            gain: None,
            saturation: Some(geom),
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

/// The big knob: ring, body, pointer and the tick scale with OFF at the minimum.
fn paint_knob<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    at: Point,
    n: f64,
    accent: Color,
    off: bool,
) {
    for i in 0..TICKS {
        let t = i as f64 / (TICKS - 1) as f64;
        let a = KNOB_START + t * KNOB_SWEEP;
        let dir = Vec2::new(a.cos(), a.sin());
        let long = i % 5 == 0;
        let lit = !off && t <= n + 1e-6;
        let (inner, width) = if long { (3.4, 1.6) } else { (2.0, 1.1) };
        let color = if lit {
            accent
        } else {
            with_alpha(theme.text_muted, if long { 0.9 } else { 0.55 })
        };
        c.stroke_with(
            Line::new(at + dir * (TICK_R - inner), at + dir * (TICK_R + 1.5)),
            width,
            color,
        );
    }
    c.stroke(
        &arc_path(at, BIG_RING_R, KNOB_START, KNOB_SWEEP).into(),
        &round_stroke(3.0),
        &solid(theme.knob_track),
    );
    if n > 1e-4 {
        c.stroke(
            &arc_path(at, BIG_RING_R, KNOB_START, n * KNOB_SWEEP).into(),
            &round_stroke(3.0),
            &solid(accent),
        );
    }
    c.fill_with(
        Circle::new(at + Vec2::new(0.0, 2.5), BIG_R + 0.8),
        with_alpha(Color::BLACK, 0.55),
    );
    c.fill_with(
        Circle::new(at, BIG_R),
        vgrad(
            at.y - BIG_R,
            at.y + BIG_R,
            theme.knob_top,
            theme.knob_bottom,
        ),
    );
    c.fill_with(
        Circle::new(at, BIG_R - 3.0),
        vgrad(
            at.y - BIG_R,
            at.y + BIG_R,
            mix(theme.knob_bottom, theme.knob_top, 0.35),
            mix(theme.knob_top, Color::WHITE, 0.04),
        ),
    );
    c.stroke_with(
        Circle::new(at, BIG_R - 0.5),
        1.0,
        with_alpha(theme.ink, 0.07),
    );
    let a = KNOB_START + n * KNOB_SWEEP;
    let dir = Vec2::new(a.cos(), a.sin());
    c.stroke(
        &Line::new(at + dir * 4.0, at + dir * (BIG_R - 2.5)).into(),
        &round_stroke(2.4),
        &solid(theme.knob_pointer),
    );
}

/// The drive lamp: a dark bead at 0, then a core and a glow that grow with `level`.
fn paint_lamp<C: Canvas + ?Sized>(c: &mut C, theme: &Theme, at: Point, level: f32) {
    c.fill_with(Circle::new(at, 7.5), with_alpha(Color::BLACK, 0.45));
    if level <= 0.001 {
        c.fill_with(Circle::new(at, 5.5), theme.led_off);
        c.stroke_with(Circle::new(at, 5.5), 1.0, with_alpha(theme.ink, 0.12));
        return;
    }
    let amber = hsl(30.0, 0.95, 0.55);
    let k = level.sqrt();
    c.fill_with(Circle::new(at, 14.0), with_alpha(amber, 0.10 * k));
    c.fill_with(Circle::new(at, 10.5), with_alpha(amber, 0.20 * k));
    c.fill_with(Circle::new(at, 7.0), with_alpha(amber, 0.30 + 0.30 * k));
    c.fill_with(Circle::new(at, 5.5), mix(theme.led_off, amber, k));
    c.fill_with(
        Circle::new(at - Vec2::new(1.2, 1.4), 2.0),
        mix(amber, Color::WHITE, 0.45).with_alpha(0.35 + 0.65 * level),
    );
}

/// The vertical three-way switch with its labels.
fn paint_switch<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    g: &SaturationGeom,
    selected: usize,
    accent: Color,
) {
    let x = g.switch_x();
    let (top, bottom) = (g.switch_y(0), g.switch_y(2));
    c.fill_with(
        RoundedRect::new(x - 3.0, top - 6.0, x + 3.0, bottom + 6.0, 3.0),
        theme.knob_track,
    );
    for (i, label) in TYPE_LABELS.iter().enumerate() {
        let y = g.switch_y(i);
        let on = i == selected;
        c.fill_with(
            Circle::new(pt(x, y), 1.8),
            if on {
                accent
            } else {
                with_alpha(theme.text_muted, 0.6)
            },
        );
        c.text(
            &TextRun::new(
                crate::i18n::tr(label),
                pt(x + 18.0, y + 2.8),
                8.0,
                if on { theme.text } else { theme.text_muted },
            )
            .weight(if on { 700 } else { 600 }),
        );
    }
    let y = g.switch_y(selected);
    let cap = Rect::new(x - 7.0, y - 5.5, x + 7.0, y + 5.5);
    c.fill_with(
        RoundedRect::from_rect(cap + Vec2::new(0.0, 1.5), 2.5),
        with_alpha(Color::BLACK, 0.5),
    );
    c.fill_with(
        RoundedRect::from_rect(cap, 2.5),
        vgrad(cap.y0, cap.y1, theme.knob_top, theme.knob_bottom),
    );
    c.stroke_with(
        Line::new(pt(cap.x0 + 3.0, y), pt(cap.x1 - 3.0, y)),
        1.2,
        theme.knob_pointer,
    );
}

/// Paints the front: the big knob with its scale, the lamp and the type switch.
pub fn paint_front<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: &FxPanel,
    info: &PanelInfo<'_>,
) {
    let Some(g) = fx.saturation else { return };
    let sat = value(info, "saturation");
    let off = sat <= 0.0;
    let n = f64::from(sat.clamp(0.0, 1.0));
    let at = g.knob_center();
    paint_knob(c, theme, at, n, info.accent, off);
    small(
        c,
        theme,
        crate::i18n::tr("OFF"),
        pt(at.x - 31.0, at.y + 23.0),
        Align::Center,
        off.then(|| mix(info.accent, Color::WHITE, 0.3)),
    );

    let drive = info.fx_meters.get(METER_DRIVE).copied().unwrap_or(0.0);
    let lamp = g.lamp_center();
    paint_lamp(c, theme, lamp, lamp_level(drive));
    small(
        c,
        theme,
        crate::i18n::tr("DRIVE"),
        pt(lamp.x, g.lamp.y0 + 48.0),
        Align::Center,
        None,
    );

    let selected = value(info, "type").round().clamp(0.0, 2.0) as usize;
    paint_switch(c, theme, &g, selected, info.accent);
}
