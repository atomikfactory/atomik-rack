//! The wavetable synth's rack panel: the generated control layout with a display of each
//! oscillator's current frame in a strip at the left.
//!
//! ```text
//! TABLE 1                      OSC 1   OSC 2   MIX   UNISON   FILTER   ENV AMP   ...
//! +--------------------------+ [table] [table]  ...
//! | vowels            41 %   | [pos]   [pos]    ...
//! |   ~~~~/\~~~~~/\~~~~      |
//! | ▮▮▮▮▮▮▮▯▯▯▯▯▯▯▯▯         |
//! +--------------------------+
//! TABLE 2
//! +--------------------------+
//! ```
//!
//! The controls are the device's params, placed by the generic widgets (the table choice cycles
//! on a click, the knobs take drags and the wheel as everywhere). Each display draws one cycle
//! of the selected table at the oscillator's `position` (the blend of the two neighbouring
//! frames, as the device plays it, full bandwidth) and a row of one tick per frame with the
//! frames the position lies between lit. The curve comes from
//! [`atomik_dsp::wavetable::display_frame`], so it is the same arithmetic as the device's tables.
//! The display shows the knob positions, not the modulated position of a sounding voice.

use crate::autolayout::{self, CONTENT_X, MARGIN_BOTTOM, MARGIN_TOP, PanelSpec, Section, TITLE_H};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::theme::{Theme, mix};
use crate::widget::current_value;
use atomik_device_sdk::DeviceDescriptor;
use atomik_dsp::wavetable::{FRAMES, TABLE_COUNT, TABLE_NAMES, display_frame};
use atomik_model::RackDevice;
use vello::kurbo::{BezPath, Line, Rect, RoundedRect};
use vello::peniko::Color;

/// Type id of the wavetable synth (it has its own panel, [`wavetable_layout`]).
pub const WAVETABLE_TYPE: &str = "atomik.wavetable-synth";
/// Width of the display strip left of the sections.
const STRIP_W: f64 = 128.0;
/// Points of the curve of one display.
const CURVE_POINTS: usize = 128;
/// Gap between the two displays.
const GAP: f64 = 6.0;
/// Height of the frame tick row inside a display.
const TICKS_H: f64 = 7.0;

/// Colour of the second oscillator's curve.
const OSC2_COLOR: Color = Color::new([0.38, 0.78, 0.98, 1.0]);

/// Geometry of the panel's extras (front, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WavetablePanel {
    /// The two frame displays (oscillator 1 above oscillator 2).
    pub displays: [Rect; 2],
}

/// The sections in layout order: chosen so that neighbours share a shelf (see the layout tests).
const SECTION_ORDER: [&str; 11] = [
    "osc1", "mix", "osc2", "voice", "filter", "env.amp", "lfo", "unison", "env.mod", "master", "cv",
];

/// The section of a param key: the single knobs gather in the VOICE, CV and MASTER sections
/// instead of one section each.
fn section_prefix(key: &str) -> &str {
    match key {
        "voices" | "velocity.amp" | "bend.range" => "voice",
        "pitch_cv_trim"
        | "pan_cv_trim"
        | "filter.resonance_cv_trim"
        | "master.level_cv_trim"
        | "cv.cutoff_amount"
        | "cv.position_amount" => "cv",
        _ => autolayout::group_prefix(key),
    }
}

/// The short label of a param control (the generated one for the others).
pub fn wavetable_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "unison.voices" => "Count",
        "mix.sub" => "Sub",
        "mix.noise" => "Noise",
        "filter.env_amount" => "Mod env",
        "filter.key_track" => "Key track",
        "env.mod.to_pos1" | "lfo.to_pos1" => "Pos 1",
        "env.mod.to_pos2" | "lfo.to_pos2" => "Pos 2",
        "env.mod.to_pitch" | "lfo.to_pitch" => "Pitch",
        "lfo.to_cutoff" => "Cutoff",
        "velocity.amp" => "Velocity",
        "bend.range" => "Bend",
        "cv.cutoff_amount" => "Cutoff",
        "cv.position_amount" => "Position",
        "pitch_cv_trim" => "Pitch",
        "pan_cv_trim" => "Pan",
        "filter.resonance_cv_trim" => "Reson.",
        "master.level_cv_trim" => "Level",
        _ => return None,
    })
}

/// The panel of `atomik.wavetable-synth`: the generated layout with the display strip reserved;
/// the back is the generic socket layout.
pub fn wavetable_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let mut spec =
        autolayout::auto_layout_with_order(desc, STRIP_W, section_prefix, &SECTION_ORDER);
    // The strip takes the vertical extent of the sections, split in two blocks.
    let (top, bottom) = spec
        .front_sections
        .iter()
        .fold((f64::MAX, 0.0f64), |(t, b), s| {
            (t.min(s.rect.y0), b.max(s.rect.y1))
        });
    let (top, bottom) = if top < bottom {
        (top, bottom)
    } else {
        (MARGIN_TOP, spec.height() - MARGIN_BOTTOM)
    };
    // The sections end at the control rows; the strip runs on to the bottom margin of the
    // tallest shelf, so the blocks are as tall as the section column.
    let block = ((bottom - top - GAP) / 2.0).floor();
    let x = CONTENT_X.0;
    let mut displays = [Rect::ZERO; 2];
    for (n, display) in displays.iter_mut().enumerate() {
        let y = top + n as f64 * (block + GAP);
        spec.front_sections.push(Section {
            label: format!("TABLE {}", n + 1),
            rect: Rect::new(x, y, x + STRIP_W, y + TITLE_H),
        });
        *display = Rect::new(x, y + TITLE_H + 2.0, x + STRIP_W, y + block);
    }
    spec.wavetable = Some(WavetablePanel { displays });
    spec
}

fn param(device: &RackDevice, desc: &DeviceDescriptor, key: &str) -> f32 {
    desc.param_by_key(key)
        .map_or(0.0, |p| current_value(device, p))
}

/// Draws the panel's extras.
pub(crate) fn paint_panel<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    panel: &WavetablePanel,
    device: &RackDevice,
    desc: &DeviceDescriptor,
    accent: Color,
) {
    let colors = [accent, OSC2_COLOR];
    for (n, r) in panel.displays.iter().enumerate() {
        let key = |k: &str| format!("osc{}.{k}", n + 1);
        let table = param(device, desc, &key("table")).round().max(0.0) as usize;
        let position = param(device, desc, &key("position")).clamp(0.0, 1.0);
        paint_display(
            c,
            theme,
            *r,
            table.min(TABLE_COUNT - 1),
            position,
            colors[n],
        );
    }
}

/// The display of one oscillator: the curve, the table name and position, the frame ticks.
fn paint_display<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    table: usize,
    position: f32,
    color: Color,
) {
    c.fill_with(RoundedRect::from_rect(r, 3.0), theme.lcd);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 3.5),
        1.0,
        theme.ink.with_alpha(0.07),
    );
    let inner = r.inset(-5.0);
    // Name and position on the first line, the tick row at the bottom, the curve between.
    c.text(&TextRun::new(
        TABLE_NAMES[table],
        pt(inner.x0, inner.y0 + 6.5),
        8.5,
        mix(color, Color::WHITE, 0.3),
    ));
    c.text(
        &TextRun::new(
            format!("{:.0} %", position * 100.0),
            pt(inner.x1, inner.y0 + 6.5),
            8.5,
            theme.text_muted,
        )
        .align(Align::End),
    );
    let ticks = Rect::new(inner.x0, inner.y1 - TICKS_H, inner.x1, inner.y1);
    let wave = Rect::new(inner.x0, inner.y0 + 12.0, inner.x1, ticks.y0 - 3.0);
    let mid = wave.center().y;
    c.stroke_with(
        Line::new(pt(wave.x0, mid), pt(wave.x1, mid)),
        0.5,
        theme.section_line,
    );
    let mut cycle = [0.0f32; CURVE_POINTS];
    display_frame(table, position, &mut cycle);
    let mut path = BezPath::new();
    for i in 0..=CURVE_POINTS {
        let v = f64::from(cycle[i % CURVE_POINTS]).clamp(-1.0, 1.0);
        let p = pt(
            wave.x0 + i as f64 / CURVE_POINTS as f64 * wave.width(),
            mid - v * wave.height() / 2.0,
        );
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    c.stroke_with(path, 1.4, color);
    // One tick per frame; the frames the position lies between are lit, the nearer brighter.
    let at = f64::from(position) * (FRAMES - 1) as f64;
    let cell = ticks.width() / FRAMES as f64;
    for f in 0..FRAMES {
        let near = (1.0 - (f as f64 - at).abs()).clamp(0.0, 1.0);
        let x0 = ticks.x0 + f as f64 * cell + 1.0;
        let tick = Rect::new(x0, ticks.y0 + 2.0, x0 + cell - 2.0, ticks.y1);
        let base = mix(theme.lcd, theme.text_muted, 0.35);
        c.fill_with(tick, mix(base, color, near as f32));
    }
}
