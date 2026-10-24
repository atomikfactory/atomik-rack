//! VCA strips of the console view: the layout, the descriptor of the group fader, the painter and
//! the hit parts. The state lives in [`crate::ConsoleView`], which places the strips after the buses
//! (one strip slot each) and routes the pointer here.
//!
//! A VCA strip is a tall fader strip in a distinct style: a plate filled with the group's colour
//! (name, `VCA n` and the member count), the M and S buttons, the offset readout and the fader
//! (`VCA_MIN_DB..=VCA_MAX_DB`, the bottom is off). It has no meter and no output: a VCA carries no
//! audio. The members' strips show the group's colour as a chip on their name plate and the level
//! they play at ([`effective_marker`]).

use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::console::{FADER_UNITY_POS, fader, lcd_text, small_button, strip_background};
use crate::console_layout::{BUTTON_H, LCD_H, PAD, STRIP_W};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::theme::mix;
use atomik_core::{ParamDescriptor, ParamId, Taper, Unit, VCA_MAX_DB, VCA_MIN_DB};
use atomik_model::VcaGroup;
use vello::kurbo::{BezPath, Rect, RoundedRect, Vec2};
use vello::peniko::Color;

/// The height of the plate (colour band, name, caption).
pub const PLATE_H: f64 = 46.0;
/// The shortest fader travel a strip keeps in a short pane.
const FADER_MIN_H: f64 = 48.0;

/// The group fader as a parameter: the offset in dB, linear in dB, the bottom is off.
pub static VCA_LEVEL: ParamDescriptor = ParamDescriptor {
    id: ParamId(0),
    key: "vca.level",
    name: "VCA level",
    min: VCA_MIN_DB,
    max: VCA_MAX_DB,
    default: 0.0,
    taper: Taper::Decibel,
    unit: Unit::Db,
    discrete: false,
};

/// What a VCA strip draws and answers hits for, copied from the document by `sync`.
#[derive(Debug, Clone, PartialEq)]
pub struct VcaView {
    /// The group id.
    pub id: u32,
    /// The name.
    pub name: String,
    /// The chip colour (the group's, or a neutral one when it has none).
    pub color: Color,
    /// The offset in dB.
    pub level_db: f32,
    /// Muted.
    pub mute: bool,
    /// Soloed.
    pub solo: bool,
    /// How many strips are in the group.
    pub members: usize,
}

impl VcaView {
    /// The view of a group; `neutral` is the colour of a group without one.
    pub fn of(g: &VcaGroup, neutral: Color) -> Self {
        Self {
            id: g.id,
            name: g.name.clone(),
            color: g.color.map_or(neutral, color_of),
            level_db: g.level_db,
            mute: g.mute,
            solo: g.solo,
            members: g.members.len(),
        }
    }
}

/// A model colour as a paint colour.
pub fn color_of([r, g, b]: [u8; 3]) -> Color {
    Color::from_rgb8(r, g, b)
}

/// A part of a VCA strip a pointer can hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcaPart {
    /// The name plate (click selects the members, double-click renames, right-click the menu).
    Plate,
    /// The mute button.
    Mute,
    /// The solo button.
    Solo,
    /// The offset readout (double-click resets).
    Readout,
    /// The fader.
    Fader,
}

/// The rectangles of a VCA strip, in strip coordinates (x from the strip's left edge, y from the
/// top of the strip area).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VcaLayout {
    /// The name plate.
    pub plate: Rect,
    /// The mute button.
    pub mute: Rect,
    /// The solo button.
    pub solo: Rect,
    /// The offset readout.
    pub lcd: Rect,
    /// The fader area (the painter's cell: scale, trough, cap).
    pub fader: Rect,
}

/// The layout of a VCA strip `height` tall.
pub fn vca_layout(height: f64) -> VcaLayout {
    let plate = Rect::new(0.0, 0.0, STRIP_W, PLATE_H);
    let by = PLATE_H + 4.0;
    let half = STRIP_W / 2.0;
    let mute = Rect::new(PAD, by, half - 1.0, by + BUTTON_H);
    let solo = Rect::new(half + 1.0, by, STRIP_W - PAD, by + BUTTON_H);
    let lcd = Rect::new(
        PAD,
        by + BUTTON_H + 4.0,
        STRIP_W - PAD,
        by + BUTTON_H + 4.0 + LCD_H,
    );
    let top = lcd.y1 + 4.0;
    let fader = Rect::new(0.0, top, STRIP_W, (height - 4.0).max(top + FADER_MIN_H));
    VcaLayout {
        plate,
        mute,
        solo,
        lcd,
        fader,
    }
}

impl VcaLayout {
    /// The part at strip point `p`.
    pub fn part(&self, p: vello::kurbo::Point) -> Option<VcaPart> {
        if self.plate.contains(p) {
            Some(VcaPart::Plate)
        } else if self.mute.contains(p) {
            Some(VcaPart::Mute)
        } else if self.solo.contains(p) {
            Some(VcaPart::Solo)
        } else if self.lcd.contains(p) {
            Some(VcaPart::Readout)
        } else if self.fader.contains(p) {
            Some(VcaPart::Fader)
        } else {
            None
        }
    }
}

/// The level in dB a member at fader position `pos` plays at under a group at `vca_db`: the two
/// add (`-inf` when either is off). The same law as the strip's gain (`atomik_dsp::fader_gain_vca`).
pub fn effective_db(pos: f32, vca_db: f32) -> f32 {
    let p = pos.clamp(0.0, 1.0) / FADER_UNITY_POS;
    if p <= 0.0 || vca_db <= VCA_MIN_DB {
        f32::NEG_INFINITY
    } else {
        60.0 * p.log10() + vca_db.min(VCA_MAX_DB)
    }
}

/// The fader position showing `db` (the inverse taper, clamped to `0..=1`; `-inf` is 0).
pub fn pos_for_db(db: f32) -> f32 {
    (FADER_UNITY_POS * 10f32.powf(db / 60.0)).clamp(0.0, 1.0)
}

/// The readout of an offset ("-6.0 dB", "off" at the bottom).
pub fn level_text(db: f32) -> String {
    if db <= VCA_MIN_DB {
        "off".into()
    } else if db > 0.05 {
        format!("+{db:.1} dB")
    } else {
        format!("{:.1} dB", db + 0.0)
    }
}

/// Paints the VCA strip `v` with its left edge at `x` and its top at `top`, `height` tall.
/// `rename` is the text being typed in the plate, `drop_target` outlines the strip while a strip
/// name is dragged over it, `selected_members` lights the plate when the members are selected.
#[allow(clippy::too_many_arguments, reason = "a strip is drawn from its parts")]
pub fn paint_strip(
    c: &mut dyn Canvas,
    st: &SeqTheme,
    x: f64,
    top: f64,
    height: f64,
    v: &VcaView,
    rename: Option<&str>,
    drop_target: bool,
    selected_members: bool,
) {
    let t = &st.base;
    let l = vca_layout(height);
    let bg = Rect::new(x, top, x + STRIP_W, top + height);
    strip_background(c, st, bg);
    // The group's colour tints the strip: a faint wash and a bright edge at both sides, so a VCA
    // reads apart from a channel or a bus at a glance.
    sp::rect(
        c,
        Rect::new(x + 1.0, top, x + STRIP_W - 1.0, top + height),
        v.color.with_alpha(0.07),
    );
    for edge in [x + 1.0, x + STRIP_W - 2.0] {
        sp::rect(
            c,
            Rect::new(edge, top, edge + 1.0, top + height),
            v.color.with_alpha(0.55),
        );
    }
    c.push_transform(vello::kurbo::Affine::translate((x, top)));
    // The plate: filled with the colour, the name over it.
    let plate = l.plate.inset((-2.0, -2.0));
    if selected_members {
        crate::paint::outline(c, plate.inset(1.5), 4.0, 1.6, t.highlight);
    }
    c.fill_with(
        RoundedRect::from_rect(plate, 3.0),
        mix(v.color, t.plate, 0.55),
    );
    c.fill_with(
        RoundedRect::new(plate.x0, plate.y0, plate.x1, plate.y0 + 4.0, 2.0),
        v.color,
    );
    if let Some(text) = rename {
        let field = Rect::new(
            plate.x0 + 3.0,
            plate.y0 + 6.0,
            plate.x1 - 3.0,
            plate.y0 + 24.0,
        );
        sp::lcd(c, st, field, Some(t.focus));
        c.text(&TextRun::new(
            format!("{text}|"),
            pt(field.x0 + 4.0, field.center().y + 3.6),
            10.0,
            t.text,
        ));
    } else {
        c.text(
            &TextRun::new(
                fit_label(&v.name, plate.width() - 8.0, 10.0),
                pt(plate.x0 + 5.0, plate.y0 + 18.0),
                10.0,
                Color::WHITE,
            )
            .weight(700),
        );
    }
    c.text(
        &TextRun::new(
            format!("VCA {}", v.id),
            pt(plate.x0 + 5.0, plate.y1 - 4.0),
            6.0,
            mix(Color::WHITE, v.color, 0.35),
        )
        .weight(600)
        .spacing(0.8),
    );
    c.text(
        &TextRun::new(
            format!("{} ch", v.members),
            pt(plate.x1 - 5.0, plate.y1 - 4.0),
            6.5,
            mix(Color::WHITE, v.color, 0.35),
        )
        .align(Align::End)
        .weight(600),
    );
    small_button(c, st, l.mute, "M", v.mute.then_some(st.mute), true, 7.5);
    small_button(c, st, l.solo, "S", v.solo.then_some(st.solo), true, 7.5);
    lcd_text(
        c,
        st,
        l.lcd,
        &level_text(v.level_db),
        mix(v.color, Color::WHITE, 0.35),
        7.5,
    );
    fader(c, st, l.fader, &VCA_LEVEL, v.level_db, v.color);
    c.pop_transform();
    if drop_target {
        crate::paint::outline(c, bg.inset(-1.0), 4.0, 2.0, t.highlight);
    }
}

/// Draws, on the member fader cell `cell`, a marker at the level the strip plays at under its
/// group (`pos` is that level as a fader position, 0..=1), in the group's colour. The cap itself
/// stays at the member's own position.
pub fn effective_marker(c: &mut dyn Canvas, cell: Rect, pos: f64, color: Color) {
    let x = cell.x0 + 13.0;
    let (top, bottom) = (cell.y0 + 8.0, cell.y1 - 8.0);
    let y = bottom - (bottom - top) * pos.clamp(0.0, 1.0);
    let mut tri = BezPath::new();
    tri.move_to(pt(x - 5.0, y));
    tri.line_to(pt(x - 12.0, y - 4.0));
    tri.line_to(pt(x - 12.0, y + 4.0));
    tri.close_path();
    c.fill_with(tri, color);
    sp::line(c, x - 5.0, y, x + 5.0, y, color.with_alpha(0.8));
}

/// The chip on a member's name plate (`plate` is the plate rectangle).
pub fn chip(c: &mut dyn Canvas, plate: Rect, color: Color) {
    let r = plate.inset((-2.0, -2.0));
    let chip = Rect::new(r.x1 - 20.0, r.y0 + 6.0, r.x1 - 5.0, r.y0 + 12.0);
    c.fill_with(
        RoundedRect::from_rect(chip + Vec2::new(0.0, 0.5), 2.0),
        Color::BLACK.with_alpha(0.4),
    );
    c.fill_with(RoundedRect::from_rect(chip, 2.0), color);
}
