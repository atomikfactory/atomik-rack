//! The rack panel of `atomik.character`: a 3 U panel with the
//! 4 x 4 preset matrix (radio buttons with a glyph each, row-major from the top left), a screen
//! that shows the active preset's animated scene and its latency class, a meter strip (input
//! peak, output peak, activity), the four knobs (Input Gain, Transform, Dry/Wet, Volume) with
//! their values (dB for the square-law gain and volume) and a CV-amount ring on Transform and
//! Dry/Wet while their CV input is cabled. The back is the six sockets with the two CV trim knobs
//! under their jacks.
//!
//! ```text
//! PRESET            SCREEN                                  METER
//! [ ][ ][ ][ ]      +--------------------------------+      |  |  |
//! [ ][ ][ ][ ]      |  REEL                LAT SHORT |      |  |  |
//! [ ][ ][ ][ ]      |         (scene)                |      IN OUT ACT
//! [ ][ ][ ][ ]      +--------------------------------+
//!                   CONTROLS
//!                   (Input Gain) (Transform) (Dry/Wet) (Volume)
//! ```
//!
//! `preset` is the matrix (every button is one `SetParam`, a click on the active button emits
//! nothing); the other five params are ordinary controls, the two trims on the back face
//! ([`PanelSpec::on_back`]). The matrix is one focusable node of the rack view: the arrow keys
//! move the selection by one and by row, Home and End jump to the first and the last preset.

use crate::autolayout::{
    CONTENT_X, FxPanel, MARGIN_TOP, PanelSpec, PlacedControl, PlacedSocket, SOCKET_CELL, Section,
    TITLE_H,
};
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::character_screen;
use crate::event::Key;
use crate::fx::{self, CHARACTER_PRESETS, FxKind, LEVEL_FLOOR_DB, level_norm};
use crate::fx_paint::{bar, lcd, meter_color, small};
use crate::paint::{KNOB_DY, KNOB_RING_R, KNOB_START, KNOB_SWEEP, PanelInfo};
use crate::theme::{Theme, mix};
use crate::widget::{current_value, format_value};
use atomik_core::Unit;
use atomik_device_sdk::{ControlKind, DeviceDescriptor};
use vello::kurbo::{Arc, Point, Rect, RoundedRect, Shape};
use vello::peniko::Color;

/// Height of the panel in rack units.
pub const CHARACTER_UNITS: u8 = 3;
/// Size of one matrix cell (the button is inset by [`CELL_INSET`]).
pub const CELL: (f64, f64) = (50.0, 46.0);
/// Gap between two buttons is twice this.
pub const CELL_INSET: f64 = 2.0;
const SCREEN_DX: f64 = 212.0;
const SCREEN_W: f64 = 320.0;
const SCREEN_TOP: f64 = MARGIN_TOP + TITLE_H + 3.0;
const SCREEN_H: f64 = 104.0;
const KNOB_PITCH: f64 = 104.0;
const KNOB_CELL: (f64, f64) = (48.0, 70.0);
/// Where a value readout sits under a knob (from the top of its cell).
const READOUT_DY: f64 = 63.0;

/// Geometry of the Character panel (front and back, panel coordinates).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterGeom {
    /// The 4 x 4 button matrix (cells by [`cell_rect`]).
    pub matrix: Rect,
    /// The screen (frame and scene).
    pub screen: Rect,
    /// The latency class badge, inside the screen at the top right.
    pub badge: Rect,
    /// The meter strip right of the screen.
    pub bars: Rect,
    /// Descriptor indices of the controls that sit on the back face (the two CV trims).
    pub back_params: [usize; 2],
}

/// The rectangle of button `i` (0..16, row-major from the top left) in `matrix`.
pub fn cell_rect(matrix: Rect, i: usize) -> Rect {
    let (col, row) = ((i % 4) as f64, (i / 4) as f64);
    Rect::new(
        matrix.x0 + col * CELL.0 + CELL_INSET,
        matrix.y0 + row * CELL.1 + CELL_INSET,
        matrix.x0 + (col + 1.0) * CELL.0 - CELL_INSET,
        matrix.y0 + (row + 1.0) * CELL.1 - CELL_INSET,
    )
}

/// The preset whose button contains `p`; the gaps between buttons belong to none.
pub fn preset_at(matrix: Rect, p: Point) -> Option<usize> {
    (0..CHARACTER_PRESETS).find(|i| cell_rect(matrix, *i).contains(p))
}

/// The preset a navigation key moves the selection to from `current`: Left and Right by one,
/// Up and Down by a row, Home and End to the first and the last; the ends stay (no wrap). `None`
/// for any other key.
pub fn move_selection(current: usize, key: &Key) -> Option<usize> {
    let last = CHARACTER_PRESETS - 1;
    let cur = current.min(last);
    Some(match key {
        Key::Left => cur.saturating_sub(1),
        Key::Right => (cur + 1).min(last),
        Key::Up => cur.saturating_sub(4),
        Key::Down => (cur + 4).min(last),
        Key::Home => 0,
        Key::End => last,
        _ => return None,
    })
}

/// The panel (3 U); the front has the matrix, screen, meters and four knobs, the back the six
/// sockets and the two trims.
pub fn character_layout(desc: &DeviceDescriptor) -> PanelSpec {
    let x0 = CONTENT_X.0;
    let matrix = Rect::new(x0, SCREEN_TOP, x0 + 4.0 * CELL.0, SCREEN_TOP + 4.0 * CELL.1);
    let screen_x0 = x0 + SCREEN_DX;
    let screen = Rect::new(
        screen_x0,
        SCREEN_TOP,
        screen_x0 + SCREEN_W,
        SCREEN_TOP + SCREEN_H,
    );
    let badge = Rect::new(
        screen.x1 - 70.0,
        screen.y0 + 5.0,
        screen.x1 - 5.0,
        screen.y0 + 18.0,
    );
    let bars = Rect::new(screen.x1 + 12.0, SCREEN_TOP, CONTENT_X.1, screen.y1);
    let index = |k: &str| desc.params.iter().position(|p| p.key == k);

    // The knob row under the screen.
    let row_title_y = screen.y1 + 6.0;
    let knob_y = row_title_y + TITLE_H + 2.0;
    let mut front = Vec::new();
    for (n, key) in ["gain", "transform", "mix", "volume"].iter().enumerate() {
        if let Some(param) = index(key) {
            let cx = screen_x0 + (n as f64 + 0.5) * KNOB_PITCH;
            front.push(PlacedControl {
                param,
                kind: ControlKind::Knob,
                rect: Rect::new(
                    cx - KNOB_CELL.0 / 2.0,
                    knob_y,
                    cx + KNOB_CELL.0 / 2.0,
                    knob_y + KNOB_CELL.1,
                ),
            });
        }
    }

    // The back face: three socket groups, each CV jack with its trim knob under it.
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
    let (in_x, cv_x, out_x) = (x0, x0 + 142.0, x0 + 296.0);
    place("in_l", in_x);
    place("in_r", in_x + 60.0);
    place("transform_cv_in", cv_x);
    place("mix_cv_in", cv_x + 60.0);
    place("out_l", out_x);
    place("out_r", out_x + 60.0);
    let mut back_params = [usize::MAX; 2];
    for (n, (trim, x)) in [("transform_cv_trim", cv_x), ("mix_cv_trim", cv_x + 60.0)]
        .iter()
        .enumerate()
    {
        if let Some(param) = index(trim) {
            back_params[n] = param;
            let top = jack_y + SOCKET_CELL.1 + 8.0;
            front.push(PlacedControl {
                param,
                kind: ControlKind::Knob,
                rect: Rect::new(x + 1.0, top, x + 1.0 + 48.0, top + 58.0),
            });
        }
    }
    let title = |label: &str, x0: f64, x1: f64, y: f64| Section {
        label: label.to_string(),
        rect: Rect::new(x0, y, x1, y + TITLE_H),
    };
    let front_sections = vec![
        title("PRESET", matrix.x0, matrix.x1, MARGIN_TOP),
        title("SCREEN", screen.x0, screen.x1, MARGIN_TOP),
        title("METER", bars.x0, bars.x1, MARGIN_TOP),
        title("CONTROLS", screen.x0, bars.x1, row_title_y),
    ];
    let back_sections = vec![
        title("AUDIO IN", in_x, in_x + 110.0, MARGIN_TOP),
        title("CV IN", cv_x, cv_x + 110.0, MARGIN_TOP),
        title("AUDIO OUT", out_x, out_x + 110.0, MARGIN_TOP),
    ];
    let geom = CharacterGeom {
        matrix,
        screen,
        badge,
        bars,
        back_params,
    };
    PanelSpec {
        height_units: CHARACTER_UNITS,
        samples: None,
        drum: None,
        slicer: None,
        fx: Some(FxPanel {
            kind: FxKind::Character,
            visual: screen,
            plot: screen,
            meters: bars,
            readout: Rect::ZERO,
            aux: Rect::ZERO,
            button: None,
            character: Some(geom),
        }),
        seq: None,
        drum_gen: None,
        arp: None,
        key_chord: None,
        modsrc: None,
        container: None,
        plugin: None,
        front,
        front_sections,
        back,
        back_sections,
    }
}

// ------------------------------------------------------------------ painting

fn param_value(info: &PanelInfo<'_>, key: &str) -> f32 {
    let Some(desc) = info.desc else { return 0.0 };
    desc.params
        .iter()
        .find(|p| p.key == key)
        .map_or(0.0, |p| match info.device {
            Some(d) => current_value(d, p),
            None => p.default,
        })
}

/// The value readout under a knob: dB for the square-law gain and volume, else the generic
/// unit text.
pub fn readout(p: &atomik_core::ParamDescriptor, value: f32) -> String {
    fx::character_value_text(p.key, value).unwrap_or_else(|| format_value(p, value))
}

/// The face of the active matrix button.
pub fn lit_face(theme: &Theme, accent: Color) -> Color {
    mix(theme.button, accent, 0.3)
}

/// The glyph colour on the active matrix button (a test pins its contrast with the face).
pub fn lit_glyph(accent: Color) -> Color {
    mix(accent, Color::WHITE, 0.45)
}

fn knob_center(r: Rect) -> Point {
    pt(r.center().x, r.y0 + KNOB_DY)
}

/// An arc of the knob sweep from `lo` to `hi` (both 0..1) on a ring of radius `r`.
fn sweep_arc(center: Point, r: f64, lo: f64, hi: f64) -> vello::kurbo::BezPath {
    Arc::new(
        center,
        (r, r),
        KNOB_START + lo * KNOB_SWEEP,
        (hi - lo) * KNOB_SWEEP,
        0.0,
    )
    .path_elements(0.1)
    .collect()
}

/// Paints the front: matrix, screen, meters, knob readouts and the CV rings.
pub fn paint_front<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: &FxPanel,
    info: &PanelInfo<'_>,
) {
    let Some(g) = fx.character else { return };
    let Some(desc) = info.desc else { return };
    let preset = param_value(info, "preset")
        .round()
        .clamp(0.0, (CHARACTER_PRESETS - 1) as f32) as usize;

    // The matrix.
    for i in 0..CHARACTER_PRESETS {
        let r = cell_rect(g.matrix, i);
        let on = i == preset;
        let face = if on {
            lit_face(theme, info.accent)
        } else {
            theme.button
        };
        c.fill_with(RoundedRect::from_rect(r, 3.0), face);
        c.stroke_with(
            RoundedRect::from_rect(r.inset(-0.5), 3.5),
            if on { 2.0 } else { 1.0 },
            if on { info.accent } else { theme.bevel_dark },
        );
        let col = if on {
            lit_glyph(info.accent)
        } else {
            theme.text_muted
        };
        character_screen::glyph(c, theme, r, i, on, col);
    }

    // The screen: the active preset's scene, its name and its latency class.
    let transform = f32_or(info.fx_meters.get(2), param_value(info, "transform"));
    let activity = f32_or(info.fx_meters.get(3), 0.0);
    lcd(c, theme, g.screen);
    character_screen::scene(
        c,
        theme,
        g.screen.inset(-1.0),
        preset,
        transform,
        activity,
        info.fx_phase,
        info.accent,
    );
    let name = match desc
        .params
        .iter()
        .find(|p| p.key == "preset")
        .map(|p| p.unit)
    {
        Some(Unit::Choice(names)) => names.get(preset).copied().unwrap_or(""),
        _ => "",
    };
    c.text(
        &TextRun::new(
            name.to_uppercase(),
            pt(g.screen.x0 + 8.0, g.screen.y0 + 14.0),
            9.5,
            mix(info.accent, Color::WHITE, 0.3),
        )
        .weight(700)
        .spacing(0.8),
    );
    let long = fx::character_is_long(preset);
    let (badge_text, badge_color) = if long {
        ("LAT LONG", theme.warning)
    } else {
        ("LAT SHORT", theme.text_muted)
    };
    c.fill_with(
        RoundedRect::from_rect(g.badge, 3.0),
        Color::BLACK.with_alpha(0.55),
    );
    c.stroke_with(
        RoundedRect::from_rect(g.badge, 3.0),
        1.0,
        badge_color.with_alpha(if long { 0.9 } else { 0.35 }),
    );
    c.text(
        &TextRun::new(
            badge_text,
            pt(g.badge.center().x, g.badge.y1 - 3.5),
            7.0,
            badge_color,
        )
        .align(Align::Center)
        .weight(700)
        .spacing(0.4),
    );

    // The meter strip: input peak, output peak and the activity.
    let m = g.bars;
    let label_h = 10.0;
    let (w, gap) = (16.0, 12.0);
    let total = 3.0 * w + 2.0 * gap;
    let x0 = m.x0 + (m.width() - total) / 2.0;
    let in_db = info.fx_meters.first().copied().unwrap_or(LEVEL_FLOOR_DB);
    let out_db = info.fx_meters.get(1).copied().unwrap_or(LEVEL_FLOOR_DB);
    for (k, (label, norm, col)) in [
        ("IN", level_norm(in_db, -60.0, 0.0), meter_color(in_db)),
        ("OUT", level_norm(out_db, -60.0, 0.0), meter_color(out_db)),
        ("ACT", f64::from(activity.clamp(0.0, 1.0)), info.accent),
    ]
    .into_iter()
    .enumerate()
    {
        let x = x0 + k as f64 * (w + gap);
        bar(
            c,
            Rect::new(x, m.y0, x + w, m.y1 - label_h),
            norm,
            col,
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

    // Knob readouts and the CV rings.
    for pc in info.spec.front.iter().filter(|pc| !info.spec.on_back(pc)) {
        let p = &desc.params[pc.param];
        let value = info.device.map_or(p.default, |d| current_value(d, p));
        c.text(
            &TextRun::new(
                readout(p, value),
                pt(pc.rect.center().x, pc.rect.y0 + READOUT_DY),
                8.0,
                theme.text,
            )
            .align(Align::Center)
            .weight(600),
        );
        let trim_key = match p.key {
            "transform" => ("transform_cv_trim", 0),
            "mix" => ("mix_cv_trim", 1),
            _ => continue,
        };
        if !info.fx_cv[trim_key.1] {
            continue;
        }
        let trim = param_value(info, trim_key.0);
        let (lo, hi) = fx::cv_span(p.to_normalized(value), trim);
        let ring = KNOB_RING_R + 4.5;
        let at = knob_center(pc.rect);
        c.stroke_with(
            sweep_arc(at, ring, 0.0, 1.0),
            1.0,
            theme.cable_cv.with_alpha(0.25),
        );
        if hi - lo > 1e-4 {
            c.stroke_with(
                sweep_arc(at, ring, f64::from(lo), f64::from(hi)),
                2.2,
                theme.cable_cv,
            );
        }
    }
}

fn f32_or(v: Option<&f32>, default: f32) -> f32 {
    v.copied().unwrap_or(default)
}
