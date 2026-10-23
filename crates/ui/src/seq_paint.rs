//! Shared look and helpers of the sequencer views (arrangement, piano roll, transport bar).
//!
//! Every colour derives from the approved dark rack [`Theme`] (graphite surfaces, warm-white
//! type, one accent hue per device); nothing is a bitmap.

use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::theme::{Theme, mix};
use crate::timeline::{
    Bars, GRID_MIN_PX, GridLevel, TimeAxis, bar_labels, deep_labels, grid_lines,
};
use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{Direction, Registry, SocketKind};
use atomik_model::{DeviceKey, RackDoc};
use vello::kurbo::{Line, Rect, RoundedRect};
use vello::peniko::Color;

/// Colour tokens of the sequencer views, derived from the rack theme.
#[derive(Debug, Clone, PartialEq)]
pub struct SeqTheme {
    /// The rack theme the tokens come from.
    pub base: Theme,
    /// View background.
    pub background: Color,
    /// Track list / header background.
    pub header: Color,
    /// Track list row of the note lane.
    pub header_track: Color,
    /// Track list row of an automation lane.
    pub header_lane: Color,
    /// Lane background (even rows).
    pub lane: Color,
    /// Lane background (odd rows).
    pub lane_alt: Color,
    /// Automation lane background.
    pub lane_auto: Color,
    /// Ruler background.
    pub ruler: Color,
    /// Bar grid line.
    pub grid_bar: Color,
    /// Beat grid line.
    pub grid_beat: Color,
    /// Subdivision grid line.
    pub grid_sub: Color,
    /// Separator lines between areas.
    pub separator: Color,
    /// Loop range when enabled.
    pub loop_on: Color,
    /// Loop range when disabled.
    pub loop_off: Color,
    /// Loop shading over the lanes.
    pub loop_shade: Color,
    /// Playhead.
    pub playhead: Color,
    /// Recording (playhead while recording, arm button, record button).
    pub record: Color,
    /// Mute button when active.
    pub mute: Color,
    /// Solo button when active.
    pub solo: Color,
    /// Selection outline / selected notes.
    pub selection: Color,
    /// White piano key.
    pub key_white: Color,
    /// Black piano key.
    pub key_black: Color,
    /// Grid row of a black key in the piano roll.
    pub row_black: Color,
    /// Area outside the clip content in the piano roll.
    pub outside: Color,
    /// Shading of the faded-out part of an audio clip (above the fade envelope).
    pub fade_shade: Color,
    /// The gain line and handle of an audio clip.
    pub gain_line: Color,
    /// Fade and gain handles of an audio clip (lit while hovered).
    pub handle: Color,
}

impl SeqTheme {
    /// Derives the sequencer tokens from a rack theme.
    pub fn from_theme(t: &Theme) -> Self {
        let white = |a: u8| Color::from_rgba8(0xff, 0xff, 0xff, a);
        Self {
            base: t.clone(),
            background: t.background,
            header: mix(t.panel_bottom, t.background, 0.35),
            header_track: t.panel_bottom,
            header_lane: mix(t.panel_bottom, t.background, 0.55),
            lane: Color::from_rgba8(0x16, 0x18, 0x1c, 0xff),
            lane_alt: Color::from_rgba8(0x13, 0x15, 0x18, 0xff),
            lane_auto: Color::from_rgba8(0x11, 0x13, 0x16, 0xff),
            ruler: t.plate,
            grid_bar: white(0x2e),
            grid_beat: white(0x14),
            grid_sub: white(0x09),
            separator: t.bevel_dark,
            loop_on: t.highlight,
            loop_off: mix(t.text_muted, t.background, 0.45),
            loop_shade: t.highlight.with_alpha(0.05),
            playhead: t.knob_pointer,
            record: t.cable_audio,
            mute: Color::from_rgba8(0xe0, 0xa8, 0x3c, 0xff),
            solo: t.cable_notes,
            selection: t.highlight,
            key_white: Color::from_rgba8(0xd9, 0xd5, 0xcc, 0xff),
            key_black: Color::from_rgba8(0x1b, 0x1d, 0x21, 0xff),
            row_black: Color::from_rgba8(0x12, 0x14, 0x17, 0xff),
            outside: Color::from_rgba8(0x0a, 0x0b, 0x0d, 0xa0),
            fade_shade: Color::from_rgba8(0x00, 0x00, 0x00, 0x66),
            gain_line: t.highlight,
            handle: t.text,
        }
    }

    /// Clip body fill for an accent colour.
    pub fn clip_body(&self, accent: Color, muted: bool) -> Color {
        let c = mix(accent, self.base.panel_bottom, 0.62);
        if muted { mix(c, self.lane, 0.6) } else { c }
    }

    /// Audio clip body fill for an accent colour: darker than a note clip so the waveform
    /// stands out.
    pub fn audio_body(&self, accent: Color, muted: bool) -> Color {
        let c = mix(accent, self.base.background, 0.84);
        if muted { mix(c, self.lane, 0.6) } else { c }
    }

    /// Waveform fill of an audio clip for an accent colour.
    pub fn audio_wave(&self, accent: Color, muted: bool) -> Color {
        let c = mix(accent, self.base.text, 0.28);
        if muted { mix(c, self.lane, 0.65) } else { c }
    }

    /// Clip header strip / note fill for an accent colour.
    pub fn clip_head(&self, accent: Color, muted: bool) -> Color {
        let c = mix(accent, self.base.panel_bottom, 0.32);
        if muted { mix(c, self.lane, 0.65) } else { c }
    }
}

/// The track name: the device's user-visible name, else its key.
pub fn track_name(doc: &RackDoc, device: &DeviceKey) -> String {
    doc.device(device)
        .map_or_else(|| device.0.clone(), |d| d.name.clone())
}

/// The accent colour of a device (its own colour or the hashed type accent).
pub fn device_accent(theme: &Theme, doc: &RackDoc, device: &DeviceKey) -> Color {
    doc.device(device)
        .map_or(theme.text_muted, |d| theme.accent_for(d))
}

/// Whether a device type has a Notes input (and so can have a track).
pub fn has_notes_input(registry: &Registry, type_id: &str) -> bool {
    registry.get(type_id).is_some_and(|f| {
        f.descriptor
            .sockets
            .iter()
            .any(|s| s.kind == SocketKind::Notes && s.direction == Direction::Input)
    })
}

/// A plain 0..1 linear descriptor, used for automation lanes whose param is unknown.
pub const FALLBACK_PARAM: ParamDescriptor = ParamDescriptor {
    id: ParamId(0),
    key: "",
    name: "?",
    min: 0.0,
    max: 1.0,
    default: 0.0,
    taper: Taper::Linear,
    unit: Unit::None,
    discrete: false,
};

/// The descriptor of a device's param (`FALLBACK_PARAM` when unknown).
pub fn param_desc(
    doc: &RackDoc,
    registry: &Registry,
    device: &DeviceKey,
    param: &str,
) -> ParamDescriptor {
    doc.device(device)
        .and_then(|d| registry.get(&d.type_id))
        .and_then(|f| f.descriptor.param_by_key(param).copied())
        .unwrap_or(FALLBACK_PARAM)
}

/// Fills a rectangle.
pub fn rect<C: Canvas + ?Sized>(c: &mut C, r: Rect, color: Color) {
    c.fill_with(r, color);
}

/// A horizontal or vertical hairline.
pub fn line<C: Canvas + ?Sized>(c: &mut C, x0: f64, y0: f64, x1: f64, y1: f64, color: Color) {
    c.stroke_with(Line::new(pt(x0, y0), pt(x1, y1)), 1.0, color);
}

/// A small text label.
pub fn label<C: Canvas + ?Sized>(
    c: &mut C,
    text: &str,
    x: f64,
    y: f64,
    size: f32,
    color: Color,
    align: Align,
) {
    c.text(&TextRun::new(text, pt(x, y), size, color).align(align));
}

/// A push button (rounded, bevelled), lit with `lit` when active.
pub fn button<C: Canvas + ?Sized>(
    c: &mut C,
    st: &SeqTheme,
    r: Rect,
    text: &str,
    lit: Option<Color>,
    hover: bool,
) {
    let t = &st.base;
    let face = match lit {
        Some(col) => mix(col, t.button, 0.25),
        None if hover => mix(t.button, t.text, 0.12),
        None => t.button,
    };
    let rr = RoundedRect::from_rect(r, 3.0);
    c.fill_with(rr, face);
    c.stroke_with(
        Line::new(pt(r.x0 + 2.0, r.y0 + 0.5), pt(r.x1 - 2.0, r.y0 + 0.5)),
        1.0,
        t.bevel_light,
    );
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 3.5),
        1.0,
        t.bevel_dark,
    );
    if !text.is_empty() {
        let col = if lit.is_some() { t.plate } else { t.text };
        c.text(
            &TextRun::new(text, pt(r.center().x, r.center().y + 3.5), 9.5, col)
                .align(Align::Center)
                .weight(700),
        );
    }
}

/// An LCD readout window.
pub fn lcd<C: Canvas + ?Sized>(c: &mut C, st: &SeqTheme, r: Rect, border: Option<Color>) {
    let t = &st.base;
    c.fill_with(RoundedRect::from_rect(r, 3.0), t.lcd);
    c.stroke_with(
        RoundedRect::from_rect(r.inset(-0.5), 3.5),
        1.0,
        border.unwrap_or(t.bevel_dark),
    );
}

/// Vertical grid lines over `area` for the visible ticks of `axis`.
pub fn grid<C: Canvas + ?Sized>(
    c: &mut C,
    st: &SeqTheme,
    axis: &TimeAxis,
    area: Rect,
    bars: &Bars,
) {
    let t0 = axis.x_to_tick(area.x0) - 1;
    let t1 = axis.x_to_tick(area.x1) + 1;
    for g in grid_lines(t0, t1, axis.px_per_tick, bars, GRID_MIN_PX) {
        let x = axis.tick_to_x(g.tick).round() + 0.5;
        let col = match g.level {
            GridLevel::Bar => st.grid_bar,
            GridLevel::Beat => st.grid_beat,
            GridLevel::Sub => st.grid_sub,
        };
        line(c, x, area.y0, x, area.y1, col);
    }
}

/// Bar labels and tick marks along a ruler strip. `tick_offset` is added to the label tick
/// (the piano roll labels content ticks from bar 1).
pub fn ruler_labels<C: Canvas + ?Sized>(
    c: &mut C,
    st: &SeqTheme,
    axis: &TimeAxis,
    strip: Rect,
    bars: &Bars,
) {
    let t = &st.base;
    let t0 = axis.x_to_tick(strip.x0) - 1;
    let t1 = axis.x_to_tick(strip.x1) + 1;
    let labels = bar_labels(t0, t1, axis.px_per_tick, bars, 34.0);
    let paint_label = |c: &mut C, tick: i64, bar: i64| {
        let x = axis.tick_to_x(tick).round() + 0.5;
        label(
            c,
            &format!("{bar}"),
            x + 3.0,
            strip.y0 + 11.0,
            9.0,
            t.text,
            Align::Start,
        );
    };
    let lines = grid_lines(t0, t1, axis.px_per_tick, bars, GRID_MIN_PX);
    for g in &lines {
        let x = axis.tick_to_x(g.tick).round() + 0.5;
        let (h, col) = match g.level {
            GridLevel::Bar => (strip.height() * 0.55, t.text_muted),
            GridLevel::Beat => (strip.height() * 0.28, mix(t.text_muted, t.plate, 0.4)),
            GridLevel::Sub => (strip.height() * 0.14, mix(t.text_muted, t.plate, 0.6)),
        };
        line(c, x, strip.y1 - h, x, strip.y1, col);
        // A bar line carries its number right after its tick.
        if let Ok(k) = labels.binary_search_by_key(&g.tick, |l| l.0) {
            paint_label(c, labels[k].0, labels[k].1);
        }
    }
    // The first bar of a meter segment is numbered even where the zoom draws no line for it.
    for &(tick, bar) in &labels {
        if !lines.iter().any(|g| g.tick == tick) {
            paint_label(c, tick, bar);
        }
    }
    // Deep zoom: the lines between the bars carry bar.beat.sixteenth.tick names.
    for (tick, text) in deep_labels(t0, t1, axis.px_per_tick, bars, 64.0) {
        let x = axis.tick_to_x(tick).round() + 0.5;
        label(
            c,
            &text,
            x + 3.0,
            strip.y0 + 11.0,
            8.0,
            t.text_muted,
            Align::Start,
        );
    }
}

/// The playhead: a line over `[top, bottom]` with a small flag at `flag_y`.
pub fn playhead<C: Canvas + ?Sized>(c: &mut C, x: f64, top: f64, bottom: f64, color: Color) {
    let x = x.round() + 0.5;
    c.stroke_with(Line::new(pt(x, top), pt(x, bottom)), 1.5, color);
    let mut p = vello::kurbo::BezPath::new();
    p.move_to(pt(x - 5.0, top));
    p.line_to(pt(x + 5.0, top));
    p.line_to(pt(x, top + 6.0));
    p.close_path();
    c.fill_with(p, color);
}
