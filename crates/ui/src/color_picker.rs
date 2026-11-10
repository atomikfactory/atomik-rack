//! The colour picker: any colour for a track or a folder, where the colour menu offers six.
//!
//! ```text
//! COLOUR                                                                 [ CLOSE ]
//! [ saturation / value square ]  [hue]     Hex  [ #3f8fd2 ]
//! [                           ]  [   ]     [ preview ]
//! Palette  ( ) ( ) ( ) ( ) ( ) ( )
//! [ APPLY ]    Drag in the square and the strip, or type a hex colour; Enter applies
//! ```
//!
//! A modal view the app places over the window like the song information panel. The app opens
//! it with the colour the target has now ([`ColorPickerView::new`]) and reads the choice back
//! with [`ColorPickerView::color`] when [`UiAction::ColorPicker`] arrives:
//!
//! * `APPLY` (and Enter in the hex text, when it holds a colour) emits [`ColorPickerAction::Apply`].
//! * `CLOSE` emits [`ColorPickerAction::Close`]; the app also closes the panel on Esc.
//!
//! The colour is kept as hue, saturation and value so that dragging in the square never loses the
//! hue of a grey or black; the hex text follows the colour while the pointer works, and the colour
//! follows the text while it is typed (a text that is not a colour leaves the colour as it was
//! and is drawn as such). The hex text is a [`TextField`], so the clipboard and input methods work
//! in it. The six colours of the colour menu stay as quick picks.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas, CanvasExt};
use crate::event::{Key, PointerButton, UiEvent};
use crate::marker_edit::PALETTE;
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use atomik_model::DeviceKey;
use vello::kurbo::{Affine, Circle, Point, Rect, RoundedRect};
use vello::peniko::Color;

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const FOOT_H: f64 = 44.0;
const MARGIN: f64 = 16.0;
const WIDTH: f64 = 360.0;
const BTN_H: f64 = 22.0;
const SQUARE_W: f64 = 180.0;
const SQUARE_H: f64 = 130.0;
const STRIP_W: f64 = 20.0;
const HEX_ROW_H: f64 = 28.0;
const SWATCH: f64 = 22.0;
const FONT: f32 = 10.5;
/// Cells the saturation/value square is drawn in, across and down.
const CELLS: (usize, usize) = (24, 16);
/// Cells of the hue strip.
const HUE_CELLS: usize = 36;

/// What the colour is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorTarget {
    /// The colour of a folder row.
    Folder(u32),
    /// The colour of these tracks (and of the devices that play them).
    Tracks(Vec<DeviceKey>),
    /// The colour token of the theme editor at this place of [`crate::theme_doc::TOKENS`].
    ThemeToken(usize),
    /// The colour of these devices only (a console bus has no track).
    Devices(Vec<DeviceKey>),
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorPickerAction {
    /// Give the target the panel's colour.
    Apply,
    /// Close the panel.
    Close,
}

/// A colour as hue (degrees, `0..360`), saturation and value (`0..=1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsv {
    /// Hue in degrees.
    pub h: f32,
    /// Saturation.
    pub s: f32,
    /// Value (brightness).
    pub v: f32,
}

impl Hsv {
    /// The colour as hue, saturation and value. A grey has hue 0.
    pub fn of(rgb: [u8; 3]) -> Self {
        let [r, g, b] = rgb.map(|c| f32::from(c) / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d <= f32::EPSILON {
            0.0
        } else if (max - r).abs() <= f32::EPSILON {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if (max - g).abs() <= f32::EPSILON {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        Self {
            h: h.rem_euclid(360.0),
            s: if max <= f32::EPSILON { 0.0 } else { d / max },
            v: max,
        }
    }

    /// The nearest 8-bit colour.
    pub fn rgb(self) -> [u8; 3] {
        let h = self.h.rem_euclid(360.0) / 60.0;
        let (s, v) = (self.s.clamp(0.0, 1.0), self.v.clamp(0.0, 1.0));
        let c = v * s;
        let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v - c;
        [r + m, g + m, b + m].map(|u| (u * 255.0).round().clamp(0.0, 255.0) as u8)
    }
}

/// The colour a text names: `#rrggbb` or `rrggbb`, or the short `#rgb`.
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let t = text.trim().trim_start_matches('#');
    if !t.is_ascii() {
        return None;
    }
    let digits = |s: &str| u8::from_str_radix(s, 16).ok();
    match t.len() {
        6 => Some([digits(&t[0..2])?, digits(&t[2..4])?, digits(&t[4..6])?]),
        3 => {
            let d = |i: usize| digits(&t[i..=i]).map(|v| v * 17);
            Some([d(0)?, d(1)?, d(2)?])
        }
        _ => None,
    }
}

/// The colour as `#rrggbb`.
pub fn hex_of([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn color_of([r, g, b]: [u8; 3]) -> Color {
    Color::from_rgb8(r, g, b)
}

/// What a pointer drag is moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    Square,
    Strip,
    Text,
}

/// The colour picker panel. See the module documentation.
#[derive(Debug)]
pub struct ColorPickerView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    hsv: Hsv,
    hex: TextField,
    /// The text names a colour (false while it is half typed).
    valid: bool,
    drag: Option<Drag>,
    hover: Option<Point>,
    repaint: bool,
}

impl ColorPickerView {
    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// A panel starting from `color`, with the keys on the hex text; call [`Self::set_bounds`].
    pub fn new(theme: Theme, color: [u8; 3]) -> Self {
        let mut hex = TextField::new(&hex_of(color), 7, false);
        hex.select_all();
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            hsv: Hsv::of(color),
            hex,
            valid: true,
            drag: None,
            hover: None,
            repaint: true,
        }
    }

    /// Places the panel: the area it may use (logical pixels). The panel is centred in it.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// The colour as it is now.
    pub fn color(&self) -> [u8; 3] {
        self.hsv.rgb()
    }

    /// The colour as hue, saturation and value.
    pub fn hsv(&self) -> Hsv {
        self.hsv
    }

    /// The hex text as it is now.
    pub fn hex_text(&self) -> &str {
        self.hex.text()
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Whether the panel takes typed text (always: it is modal and every key goes to the text).
    pub fn is_text_owner(&self) -> bool {
        true
    }

    /// Makes the colour `color` (the text follows).
    pub fn set_color(&mut self, color: [u8; 3]) {
        self.hsv = Hsv::of(color);
        self.show_hex();
    }

    fn show_hex(&mut self) {
        self.hex.set_text(&hex_of(self.hsv.rgb()));
        self.valid = true;
        self.repaint = true;
    }

    /// The hex text was edited: the colour follows when it names one.
    fn read_hex(&mut self) {
        match parse_hex(self.hex.text()) {
            Some(c) => {
                // Keep the hue of a grey the pointer chose, unless the colour really differs.
                if c != self.hsv.rgb() {
                    self.hsv = Hsv::of(c);
                }
                self.valid = true;
            }
            None => self.valid = false,
        }
        self.repaint = true;
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + Self::height().min(room))
    }

    fn height() -> f64 {
        HEADER_H + SQUARE_H + 10.0 + SWATCH + 10.0 + FOOT_H
    }

    /// The saturation/value square.
    pub fn square_rect(&self) -> Rect {
        let p = self.panel();
        let y = p.y0 + HEADER_H + 4.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + SQUARE_W, y + SQUARE_H)
    }

    /// The hue strip.
    pub fn strip_rect(&self) -> Rect {
        let sq = self.square_rect();
        Rect::new(sq.x1 + 10.0, sq.y0, sq.x1 + 10.0 + STRIP_W, sq.y1)
    }

    /// The box of the hex text.
    pub fn hex_rect(&self) -> Rect {
        let sq = self.square_rect();
        let strip = self.strip_rect();
        let p = self.panel();
        let x0 = strip.x1 + 14.0 + 28.0;
        Rect::new(
            x0,
            sq.y0,
            (p.x1 - PAD).max(x0 + 1.0),
            sq.y0 + HEX_ROW_H - 6.0,
        )
    }

    /// The swatch showing the colour.
    pub fn preview_rect(&self) -> Rect {
        let hex = self.hex_rect();
        Rect::new(hex.x0, hex.y1 + 10.0, hex.x1, hex.y1 + 10.0 + 44.0)
    }

    /// The quick-pick swatch of palette entry `i`.
    pub fn swatch_rect(&self, i: usize) -> Rect {
        let sq = self.square_rect();
        let p = self.panel();
        let x = p.x0 + PAD + 60.0 + i as f64 * (SWATCH + 8.0);
        let y = sq.y1 + 10.0;
        Rect::new(x, y, x + SWATCH, y + SWATCH)
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 80.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H,
        )
    }

    /// The APPLY button.
    pub fn apply_rect(&self) -> Rect {
        let p = self.panel();
        let y = self.swatch_rect(0).y1 + 10.0 + (FOOT_H - BTN_H) / 2.0;
        Rect::new(p.x0 + PAD, y, p.x0 + PAD + 100.0, y + BTN_H)
    }

    // ------------------------------------------------------------------ events

    /// What the control under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(
                self.close_rect(),
                "Close the panel without changing anything",
            );
        }
        if self.apply_rect().contains(pos) {
            return help(self.apply_rect(), "Use this colour (one undo step)");
        }
        if self.square_rect().contains(pos) {
            return help(self.square_rect(), "Saturation across, brightness down");
        }
        if self.strip_rect().contains(pos) {
            return help(self.strip_rect(), "Hue");
        }
        if (0..PALETTE.len()).any(|i| self.swatch_rect(i).contains(pos)) {
            return help(
                Rect::new(
                    self.swatch_rect(0).x0,
                    self.swatch_rect(0).y0,
                    self.swatch_rect(PALETTE.len() - 1).x1,
                    self.swatch_rect(0).y1,
                ),
                "The colours of the colour menu",
            );
        }
        None
    }

    fn pick_square(&mut self, pos: Point) {
        let r = self.square_rect();
        self.hsv.s = ((pos.x - r.x0) / r.width()).clamp(0.0, 1.0) as f32;
        self.hsv.v = (1.0 - (pos.y - r.y0) / r.height()).clamp(0.0, 1.0) as f32;
        self.show_hex();
    }

    fn pick_strip(&mut self, pos: Point) {
        let r = self.strip_rect();
        let at = ((pos.y - r.y0) / r.height()).clamp(0.0, 1.0);
        self.hsv.h = (at * 359.999) as f32;
        self.show_hex();
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                match self.drag {
                    Some(Drag::Square) => self.pick_square(*pos),
                    Some(Drag::Strip) => self.pick_strip(*pos),
                    Some(Drag::Text) => {
                        self.hex.drag(*pos);
                        self.read_hex();
                    }
                    None => {}
                }
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => self.drag = None,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                clicks,
            } => {
                self.repaint = true;
                if self.close_rect().contains(*pos) {
                    out.push(UiAction::ColorPicker(ColorPickerAction::Close));
                } else if self.apply_rect().contains(*pos) {
                    if self.valid {
                        out.push(UiAction::ColorPicker(ColorPickerAction::Apply));
                    }
                } else if self.square_rect().contains(*pos) {
                    self.drag = Some(Drag::Square);
                    self.pick_square(*pos);
                } else if self.strip_rect().contains(*pos) {
                    self.drag = Some(Drag::Strip);
                    self.pick_strip(*pos);
                } else if self.hex_rect().contains(*pos) {
                    self.drag = Some(Drag::Text);
                    self.hex.press(*pos, *clicks, modifiers.shift);
                } else if let Some(i) =
                    (0..PALETTE.len()).find(|i| self.swatch_rect(*i).contains(*pos))
                {
                    self.set_color(PALETTE[i].1);
                }
            }
            UiEvent::Ime(ime) => {
                if self.hex.handle_ime(ime) {
                    self.read_hex();
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => {
                let used = self.hex.handle_key(key, *modifiers);
                if used.handled() {
                    self.read_hex();
                } else if matches!(key, Key::Enter) && self.valid {
                    out.push(UiAction::ColorPicker(ColorPickerAction::Apply));
                }
            }
            _ => {}
        }
        out
    }

    // ------------------------------------------------------------------ painting

    /// Paints the panel (scaled by the scale factor).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let hovered = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&p.inflate(1.0, 1.0).into());
        sp::rect(c, p, st.background);
        sp::rect(c, Rect::new(p.x0, p.y0, p.x1, p.y0 + HEADER_H), st.header);
        sp::line(c, p.x0, p.y0 + 0.5, p.x1, p.y0 + 0.5, t.highlight);
        sp::line(c, p.x0, p.y1 - 0.5, p.x1, p.y1 - 0.5, st.separator);
        sp::line(c, p.x0 + 0.5, p.y0, p.x0 + 0.5, p.y1, st.separator);
        sp::line(c, p.x1 - 0.5, p.y0, p.x1 - 0.5, p.y1, st.separator);
        sp::label(
            c,
            "COLOUR",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));

        // The saturation/value square, in cells.
        let sq = self.square_rect();
        let (cols, rows) = CELLS;
        let (cw, ch) = (sq.width() / cols as f64, sq.height() / rows as f64);
        for row in 0..rows {
            for col in 0..cols {
                let s = (col as f32 + 0.5) / cols as f32;
                let v = 1.0 - (row as f32 + 0.5) / rows as f32;
                let rgb = Hsv {
                    h: self.hsv.h,
                    s,
                    v,
                }
                .rgb();
                sp::rect(
                    c,
                    Rect::new(
                        sq.x0 + col as f64 * cw,
                        sq.y0 + row as f64 * ch,
                        sq.x0 + (col + 1) as f64 * cw + 0.5,
                        sq.y0 + (row + 1) as f64 * ch + 0.5,
                    ),
                    color_of(rgb),
                );
            }
        }
        c.stroke_with(sq.inset(-0.5), 1.0, st.separator);
        let mark = Point::new(
            sq.x0 + f64::from(self.hsv.s) * sq.width(),
            sq.y0 + (1.0 - f64::from(self.hsv.v)) * sq.height(),
        );
        c.stroke_with(Circle::new(mark, 5.0), 2.0, Color::from_rgb8(0, 0, 0));
        c.stroke_with(Circle::new(mark, 5.0), 1.0, Color::from_rgb8(255, 255, 255));

        // The hue strip.
        let strip = self.strip_rect();
        let sh = strip.height() / HUE_CELLS as f64;
        for i in 0..HUE_CELLS {
            let h = (i as f32 + 0.5) / HUE_CELLS as f32 * 360.0;
            let rgb = Hsv { h, s: 1.0, v: 1.0 }.rgb();
            sp::rect(
                c,
                Rect::new(
                    strip.x0,
                    strip.y0 + i as f64 * sh,
                    strip.x1,
                    strip.y0 + (i + 1) as f64 * sh + 0.5,
                ),
                color_of(rgb),
            );
        }
        c.stroke_with(strip.inset(-0.5), 1.0, st.separator);
        let y = strip.y0 + f64::from(self.hsv.h) / 360.0 * strip.height();
        let bar = Rect::new(strip.x0 - 2.0, y - 2.0, strip.x1 + 2.0, y + 2.0);
        c.stroke_with(bar, 2.0, Color::from_rgb8(0, 0, 0));
        c.stroke_with(bar, 1.0, Color::from_rgb8(255, 255, 255));

        // The hex text and the preview.
        let hex = self.hex_rect();
        sp::label(
            c,
            "Hex",
            hex.x0 - 28.0,
            hex.y0 + 14.0,
            FONT,
            t.text,
            Align::Start,
        );
        sp::lcd(
            c,
            st,
            hex,
            Some(if self.valid { t.focus } else { st.record }),
        );
        let colors = FieldColors {
            text: if self.valid { t.text } else { st.record },
            caret: t.text,
            selection: mix(t.lcd, t.focus, 0.5),
        };
        self.hex.paint(c, hex, FONT, 14.0, colors, true);
        let prev = self.preview_rect();
        c.fill_with(RoundedRect::from_rect(prev, 3.0), color_of(self.hsv.rgb()));
        c.stroke_with(
            RoundedRect::from_rect(prev.inset(-0.5), 3.5),
            1.0,
            st.separator,
        );

        // The quick picks.
        sp::label(
            c,
            "Palette",
            p.x0 + PAD,
            self.swatch_rect(0).y0 + 15.0,
            FONT,
            t.text,
            Align::Start,
        );
        for (i, (_, rgb)) in PALETTE.iter().enumerate() {
            let r = self.swatch_rect(i);
            c.fill_with(RoundedRect::from_rect(r, 3.0), color_of(*rgb));
            let chosen = *rgb == self.hsv.rgb();
            c.stroke_with(
                RoundedRect::from_rect(r.inset(-0.5), 3.5),
                if chosen || hovered(r) { 2.0 } else { 1.0 },
                if chosen { t.text } else { st.separator },
            );
        }

        let apply = self.apply_rect();
        sp::button(
            c,
            st,
            apply,
            "APPLY",
            self.valid.then_some(st.loop_on),
            hovered(apply),
        );
        sp::label(
            c,
            &fit_label(
                "Drag in the square and the strip, or type a hex colour; Enter applies",
                p.x1 - PAD - apply.x1 - 12.0,
                9.5,
            ),
            apply.x1 + 12.0,
            apply.y0 + 15.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trips_every_colour_of_a_coarse_cube() {
        for r in (0..=255u16).step_by(15) {
            for g in (0..=255u16).step_by(15) {
                for b in (0..=255u16).step_by(15) {
                    let rgb = [r as u8, g as u8, b as u8];
                    assert_eq!(Hsv::of(rgb).rgb(), rgb, "{rgb:?}");
                }
            }
        }
    }

    #[test]
    fn the_primaries_have_their_hues() {
        assert_eq!(Hsv::of([255, 0, 0]).h, 0.0);
        assert_eq!(Hsv::of([0, 255, 0]).h, 120.0);
        assert_eq!(Hsv::of([0, 0, 255]).h, 240.0);
        let grey = Hsv::of([128, 128, 128]);
        assert_eq!((grey.h, grey.s), (0.0, 0.0));
        assert_eq!(Hsv::of([0, 0, 0]).v, 0.0);
    }

    #[test]
    fn hex_text_parses_with_or_without_the_hash_and_in_short_form() {
        assert_eq!(parse_hex("#3f8fd2"), Some([0x3f, 0x8f, 0xd2]));
        assert_eq!(parse_hex("3F8FD2"), Some([0x3f, 0x8f, 0xd2]));
        assert_eq!(parse_hex(" #fa0 "), Some([0xff, 0xaa, 0x00]));
        for bad in ["", "#", "#12", "#12345", "#1234567", "#gg0000", "#é00000"] {
            assert_eq!(parse_hex(bad), None, "{bad:?}");
        }
        assert_eq!(hex_of([1, 2, 255]), "#0102ff");
    }

    use crate::event::Modifiers;

    fn view(color: [u8; 3]) -> ColorPickerView {
        let mut v = ColorPickerView::new(Theme::dark(), color);
        v.set_bounds(Rect::new(0.0, 0.0, 900.0, 600.0));
        v
    }

    fn press(v: &mut ColorPickerView, at: Point) -> Vec<UiAction> {
        v.handle(&UiEvent::PointerDown {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        })
    }

    fn release(v: &mut ColorPickerView, at: Point) {
        v.handle(&UiEvent::PointerUp {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        });
    }

    fn type_text(v: &mut ColorPickerView, text: &str) -> Vec<UiAction> {
        let mut out = Vec::new();
        for c in text.chars() {
            out.extend(v.handle(&UiEvent::Key {
                key: Key::Char(c),
                modifiers: Modifiers::NONE,
                pressed: true,
            }));
        }
        out
    }

    fn enter(v: &mut ColorPickerView) -> Vec<UiAction> {
        v.handle(&UiEvent::Key {
            key: Key::Enter,
            modifiers: Modifiers::NONE,
            pressed: true,
        })
    }

    #[test]
    fn the_panel_starts_on_the_colour_it_is_given_with_the_text_selected() {
        let v = view([0x3f, 0x8f, 0xd2]);
        assert_eq!(v.color(), [0x3f, 0x8f, 0xd2]);
        assert_eq!(v.hex_text(), "#3f8fd2");
    }

    #[test]
    fn dragging_in_the_square_and_the_strip_changes_the_colour_and_the_text() {
        let mut v = view([255, 0, 0]);
        let sq = v.square_rect();
        // The top right corner of the square is the full colour of the hue.
        press(&mut v, Point::new(sq.x1 - 0.001, sq.y0 + 0.001));
        assert_eq!(v.color(), [255, 0, 0]);
        // Dragging to the bottom edge goes to black.
        v.handle(&UiEvent::PointerMove {
            pos: Point::new(sq.x1 - 0.5, sq.y1 + 40.0),
            modifiers: Modifiers::NONE,
        });
        assert_eq!(v.color(), [0, 0, 0]);
        assert_eq!(v.hex_text(), "#000000");
        release(&mut v, sq.center());
        // A third of the way down the strip is green (hue 120); the square keeps its value.
        let strip = v.strip_rect();
        press(&mut v, sq.center());
        release(&mut v, sq.center());
        let before = v.hsv();
        press(
            &mut v,
            Point::new(strip.center().x, strip.y0 + strip.height() / 3.0),
        );
        release(&mut v, strip.center());
        assert!((v.hsv().h - 120.0).abs() < 0.5, "{}", v.hsv().h);
        assert_eq!((v.hsv().s, v.hsv().v), (before.s, before.v));
    }

    #[test]
    fn typing_a_hex_colour_moves_the_colour_and_enter_applies_it() {
        let mut v = view([255, 0, 0]);
        // The text starts selected: typing replaces it.
        let out = type_text(&mut v, "#10ff20");
        assert!(out.is_empty());
        assert_eq!(v.color(), [0x10, 0xff, 0x20]);
        let out = enter(&mut v);
        assert_eq!(out, [UiAction::ColorPicker(ColorPickerAction::Apply)]);
    }

    #[test]
    fn a_half_typed_colour_keeps_the_last_one_and_cannot_be_applied() {
        let mut v = view([1, 2, 3]);
        type_text(&mut v, "#12");
        assert_eq!(v.color(), [1, 2, 3], "the colour waits for six digits");
        assert!(enter(&mut v).is_empty(), "Enter does nothing yet");
        let apply = v.apply_rect().center();
        assert!(press(&mut v, apply).is_empty(), "nor does APPLY");
        type_text(&mut v, "3456");
        assert_eq!(v.color(), [0x12, 0x34, 0x56]);
        assert_eq!(
            press(&mut v, apply),
            [UiAction::ColorPicker(ColorPickerAction::Apply)]
        );
    }

    #[test]
    fn the_palette_swatches_are_quick_picks_and_close_closes() {
        let mut v = view([1, 2, 3]);
        for (i, (_, rgb)) in PALETTE.iter().enumerate() {
            let at = v.swatch_rect(i).center();
            press(&mut v, at);
            assert_eq!(v.color(), *rgb);
            assert_eq!(v.hex_text(), hex_of(*rgb));
        }
        let close = v.close_rect().center();
        assert_eq!(
            press(&mut v, close),
            [UiAction::ColorPicker(ColorPickerAction::Close)]
        );
    }

    #[test]
    fn the_panel_paints_and_its_controls_have_tooltips() {
        let mut v = view([0x3f, 0x8f, 0xd2]);
        let mut canvas = crate::display_list::DisplayList::new();
        v.paint(&mut canvas);
        assert!(v.help_at(v.apply_rect().center()).is_some());
        assert!(v.help_at(v.square_rect().center()).is_some());
        assert!(v.help_at(v.strip_rect().center()).is_some());
        assert!(v.help_at(v.swatch_rect(2).center()).is_some());
        assert!(v.help_at(Point::new(0.0, 0.0)).is_none());
        v.set_scale(2.0);
        assert!(v.needs_repaint());
    }
}
