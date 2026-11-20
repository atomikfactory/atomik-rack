//! The organ's rack panel: the nine drawbars drawn as real drawbars, the other controls in the
//! generated sections to their right.
//!
//! ```text
//! DRAWBARS                                    PERC   KEY   SCANNER  ...
//! 16' 5 1/3' 8'  4' 2 2/3' 2' 1 3/5' 1 1/3' 1'
//!  8    8    8   0    0    0    0      0    0     <- the position, 0 to 8
//!  |    |    |   |    |    |    |      |    |
//! [#]  [#]  [#]  |    |    |    |      |    |     <- pulled down to raise the level
//! ```
//!
//! A drawbar is a vertical slider of nine positions, 0 (pushed in, at the top) to 8 (pulled all
//! the way down). The two sub footages (16' and 5 1/3') are brown, the octaves (8', 4', 2' and
//! 1') white and the three mutation footages (2 2/3', 1 3/5' and 1 1/3') black, as on the
//! instrument. They are placed as [`ControlKind::Fader`] cells, so they take the gestures of every
//! fader (drag, Shift for fine, double-click to reset, wheel, the MIDI, automation and macro
//! menu), except that a drag follows the pointer along the track and a pull down raises the value.

use crate::autolayout::{self, CONTENT_X, MARGIN_BOTTOM, MARGIN_TOP, PanelSpec, Section, TITLE_H};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::theme::{Theme, mix};
use atomik_core::ParamDescriptor;
use atomik_device_sdk::{ControlKind, DeviceDescriptor};
use vello::kurbo::{Line, Rect, RoundedRect};
use vello::peniko::Color;

/// Type id of the organ (it has its own panel, [`organ_layout`]).
pub const ORGAN_TYPE: &str = "atomik.organ";
/// The key prefix of the drawbar params.
const DRAWBAR_PREFIX: &str = "drawbar.";
/// Width of one drawbar cell and the gap between two.
const BAR_W: f64 = 26.0;
const BAR_GAP: f64 = 3.0;
/// Number of drawbars.
const BARS: usize = 9;
/// Width of the drawbar block: nine cells and the gaps between them.
const BLOCK_W: f64 = BARS as f64 * BAR_W + (BARS as f64 - 1.0) * BAR_GAP;
/// Space above the track (the position number) and below it (the footage name).
const TOP_PAD: f64 = 14.0;
const BOTTOM_PAD: f64 = 14.0;

/// Whether a param key is one of the nine drawbars.
pub fn is_drawbar_key(key: &str) -> bool {
    key.starts_with(DRAWBAR_PREFIX)
}

/// The index of a drawbar key in footage order (16' is 0); 0 for any other key.
pub fn drawbar_index(key: &str) -> usize {
    const KEYS: [&str; BARS] = [
        "drawbar.16",
        "drawbar.5_1_3",
        "drawbar.8",
        "drawbar.4",
        "drawbar.2_2_3",
        "drawbar.2",
        "drawbar.1_3_5",
        "drawbar.1_1_3",
        "drawbar.1",
    ];
    KEYS.iter().position(|k| *k == key).unwrap_or(0)
}

/// The colour of the drawbar `n` (0 is the 16'): brown for the sub footages, white for the
/// octaves, black for the mutations.
pub fn bar_color(n: usize) -> Color {
    match n {
        0 | 1 => Color::new([0.45, 0.27, 0.13, 1.0]),
        4 | 6 | 7 => Color::new([0.08, 0.08, 0.09, 1.0]),
        _ => Color::new([0.93, 0.92, 0.88, 1.0]),
    }
}

/// The section of a param key.
fn section_prefix(key: &str) -> &str {
    match key {
        "sound" => "sound",
        _ => autolayout::group_prefix(key),
    }
}

/// The short label of a control (the generated one for the others).
pub fn organ_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "sound" => "Sound",
        "perc.mode" => "Mode",
        "perc.decay" => "Decay",
        "perc.level" => "Level",
        "tone.leak" => "Leakage",
        "tone.fold" => "Foldback",
        "wheel.target" => "Mod wheel",
        _ => return None,
    })
}

/// The tooltip of a control: one short line. `None` for a key without one.
pub fn organ_help(key: &str) -> Option<&'static str> {
    Some(match key {
        "drawbar.16" | "drawbar.5_1_3" | "drawbar.8" | "drawbar.4" | "drawbar.2_2_3"
        | "drawbar.2" | "drawbar.1_3_5" | "drawbar.1_1_3" | "drawbar.1" => {
            "Drawbar: pull down to raise the level, 0 to 8 in steps of 3 dB. Double-click resets, Shift drags finely"
        }
        "tone.leak" => "Faint crosstalk of the tonewheels a semitone either side of each note",
        "tone.fold" => {
            "Folds the highest partials back an octave below the top tonewheel, and the 16' of low keys up"
        }
        "wheel.target" => {
            "What the mod wheel does: the scanner depth, or the rotary speed through the Wheel CV socket"
        }
        _ => return None,
    })
}

/// The cell of drawbar `n` for a panel of `height` units: from below the title to the bottom
/// margin.
fn bar_cell(n: usize, height: f64) -> Rect {
    let x = CONTENT_X.0 + n as f64 * (BAR_W + BAR_GAP);
    Rect::new(
        x,
        MARGIN_TOP + TITLE_H + 2.0,
        x + BAR_W,
        height - MARGIN_BOTTOM,
    )
}

/// The panel of `atomik.organ`: the drawbar block at the left, the other controls packed to its
/// right; the back is the generic socket layout.
pub fn organ_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let mut spec = autolayout::auto_layout_skipping(
        desc,
        BLOCK_W,
        section_prefix,
        &["sound", "perc", "key", "scanner", "tone", "wheel", "out"],
        is_drawbar_key,
    );
    let height = spec.height();
    let x0 = CONTENT_X.0;
    spec.front_sections.insert(
        0,
        Section {
            label: "DRAWBARS".to_string(),
            rect: Rect::new(x0, MARGIN_TOP, x0 + BLOCK_W, MARGIN_TOP + TITLE_H),
        },
    );
    for (n, key) in desc
        .params
        .iter()
        .enumerate()
        .filter(|(_, p)| is_drawbar_key(p.key))
        .enumerate()
        .map(|(n, (i, _))| (n, i))
    {
        spec.front.push(autolayout::PlacedControl {
            param: key,
            kind: ControlKind::Fader,
            rect: bar_cell(n, height),
        });
    }
    spec
}

/// The track of a drawbar in its cell: the centre x and the y of position 0 and of position 8.
pub fn drawbar_travel(r: Rect) -> (f64, f64, f64) {
    (r.center().x, r.y0 + TOP_PAD, r.y1 - BOTTOM_PAD)
}

/// The position `0..=1` of a plain value of drawbar `p`.
fn position(p: &ParamDescriptor, value: f32) -> f64 {
    f64::from(p.to_normalized(value))
}

/// Draws drawbar `n` in its cell `r` with the value `value` of `p`: the position number above,
/// the slot with its marks, the bar pulled out to the handle and the footage name below.
pub fn paint_drawbar<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    r: Rect,
    n: usize,
    p: &ParamDescriptor,
    value: f32,
    label: &str,
) {
    let (x, top, bottom) = drawbar_travel(r);
    let pos = position(p, value);
    c.text(
        &TextRun::new(
            format!("{}", value.round() as i32),
            pt(x, r.y0 + 9.0),
            9.0,
            theme.text,
        )
        .align(Align::Center),
    );
    // The slot and the marks of the nine positions.
    c.fill_with(
        RoundedRect::new(x - 3.0, top - 2.0, x + 3.0, bottom + 2.0, 3.0),
        theme.knob_track,
    );
    for i in 0..=8 {
        let y = top + (bottom - top) * f64::from(i) / 8.0;
        let len = if i % 4 == 0 { 4.0 } else { 2.5 };
        c.stroke_with(
            Line::new(pt(x + 5.0, y), pt(x + 5.0 + len, y)),
            1.0,
            theme.text_muted.with_alpha(0.5),
        );
    }
    // The bar pulled out of the top down to the handle.
    let y = top + (bottom - top) * pos;
    let body = bar_color(n);
    let edge = mix(body, theme.text_muted, 0.45);
    if y > top {
        let shaft = Rect::new(x - 2.5, top, x + 2.5, y);
        c.fill_with(RoundedRect::from_rect(shaft, 1.0), body);
        c.stroke_with(RoundedRect::from_rect(shaft, 1.0), 0.6, edge);
    }
    let cap = Rect::new(x - 9.0, y - 5.0, x + 9.0, y + 5.0);
    c.fill_with(RoundedRect::from_rect(cap, 2.5), body);
    c.stroke_with(RoundedRect::from_rect(cap, 2.5), 1.0, edge);
    c.stroke_with(
        Line::new(pt(cap.x0 + 3.0, y), pt(cap.x1 - 3.0, y)),
        1.0,
        mix(body, Color::WHITE, 0.45),
    );
    c.text(&TextRun::new(label, pt(x, r.y1 - 3.0), 8.0, theme.text_muted).align(Align::Center));
}
