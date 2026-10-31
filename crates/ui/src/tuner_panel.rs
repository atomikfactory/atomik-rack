//! The tuner display of the audio track panel: the geometry, the texts and the painting of the
//! note name, the needle scale, the lamp and the readouts.
//!
//! The app runs the analysis ([`atomik_dsp::tuner`]) and hands the view what to draw in
//! [`AnalyserData`](crate::analyser::AnalyserData): whether the tuner is shown and the smoothed
//! [`Reading`]. Nothing here touches a clock or the document (`tests/tuner_panel.rs`).
//!
//! * **Scale.** The needle sits on a strip from -50 to +50 cents with a tick every 10 cents and
//!   labels at -50, 0 and +50; its colour is green within [`IN_TUNE_CENTS`] of the note, amber
//!   within 15 cents and red beyond. The IN TUNE lamp lights with the green.
//! * **Texts.** The note and its octave (`E2`), the frequency (`82.41 Hz`), the deviation
//!   (`+3.2 ct`) and the reference (`A4 = 440 Hz`).
//! * **Held.** A note held after the signal decayed is drawn dim, with no lamp.
//! * **Off.** With the tuner hidden the panel says how to show it.

use crate::analyser::AnalyserData;
use crate::autolayout::FxPanel;
use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx_paint::{lcd, small};
use crate::paint::PanelInfo;
use crate::theme::{Theme, hsl};
use atomik_dsp::tuner::{IN_TUNE_CENTS, Reading, key_name, key_octave};
use vello::kurbo::{Circle, Line, Point, Rect, RoundedRect};
use vello::peniko::Color;

/// Cents either side of the centre the scale shows.
pub const SCALE_CENTS: f64 = 50.0;
/// A needle within this many cents is amber rather than red.
pub const NEAR_CENTS: f32 = 15.0;
/// Width of the block with the note name.
const NOTE_W: f64 = 112.0;

/// Where the parts of the display sit inside its LCD.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TunerGeom {
    /// The note name and its octave.
    pub note: Rect,
    /// The needle strip (the ticks and the needle run across its width).
    pub scale: Rect,
    /// The frequency, deviation and reference texts, under the strip.
    pub readout: Rect,
    /// The IN TUNE lamp.
    pub lamp: Point,
}

/// The geometry of a display whose LCD is `plot`.
pub fn geometry(plot: Rect) -> TunerGeom {
    let inner = plot.inset(-8.0);
    let note = Rect::new(inner.x0, inner.y0, inner.x0 + NOTE_W, inner.y1);
    let right = Rect::new(note.x1 + 10.0, inner.y0, inner.x1, inner.y1);
    let scale = Rect::new(
        right.x0 + 14.0,
        right.y0 + 8.0,
        right.x1 - 14.0,
        right.y0 + 30.0,
    );
    let readout = Rect::new(right.x0, scale.y1 + 14.0, right.x1, right.y1);
    TunerGeom {
        note,
        scale,
        readout,
        lamp: pt(right.x1 - 6.0, right.y0 + 2.0),
    }
}

/// The x of a deviation on the scale (clamped to it).
pub fn needle_x(scale: Rect, cents: f32) -> f64 {
    let t = (f64::from(cents) / SCALE_CENTS).clamp(-1.0, 1.0);
    scale.center().x + t * scale.width() / 2.0
}

/// The note and octave (`E2`, `C#4`).
pub fn note_text(key: i32) -> String {
    format!("{}{}", key_name(key), key_octave(key))
}

/// The frequency (`82.41 Hz`, `1318.5 Hz`).
pub fn hz_text(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{hz:.1} Hz")
    } else {
        format!("{hz:.2} Hz")
    }
}

/// The deviation with its sign (`+3.2 ct`, `-12.0 ct`, `0.0 ct`).
pub fn cents_text(cents: f32) -> String {
    let c = if cents.abs() < 0.05 { 0.0 } else { cents };
    if c > 0.0 {
        format!("+{c:.1} ct")
    } else {
        format!("{c:.1} ct")
    }
}

/// The reference pitch (`A4 = 440 Hz`).
pub fn reference_text(ref_hz: f32) -> String {
    format!("A4 = {ref_hz:.0} Hz")
}

/// How the needle is coloured: in tune, near or far.
pub fn needle_color(cents: f32) -> Color {
    let c = cents.abs();
    if c <= IN_TUNE_CENTS {
        hsl(140.0, 0.6, 0.52)
    } else if c <= NEAR_CENTS {
        hsl(42.0, 0.85, 0.55)
    } else {
        hsl(6.0, 0.75, 0.55)
    }
}

/// The text the display shows for each state, for tests and the session texts.
pub fn status_text(on: bool, reading: Option<&Reading>) -> &'static str {
    match (on, reading) {
        (false, _) => "TUNER OFF",
        (true, None) => "PLAY A NOTE",
        (true, Some(r)) if r.held => "HOLD",
        (true, Some(r)) if r.in_tune() => "IN TUNE",
        (true, Some(r)) if r.note.cents < 0.0 => "FLAT",
        (true, Some(_)) => "SHARP",
    }
}

fn alpha(c: Color, a: f32) -> Color {
    c.with_alpha(a)
}

/// Paints the display of an audio track panel (the TUNER button is on the name plate). `ref_hz` is the track's
/// `tuner_ref`.
pub fn paint<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    fx: FxPanel,
    info: &PanelInfo<'_>,
    ref_hz: f32,
) {
    let data: &AnalyserData = info.analyser;
    let r = fx.plot;
    lcd(c, theme, r);
    let g = geometry(r);
    let reading = data.tuner.filter(|_| data.tuner_on);
    let dim = if reading.is_some_and(|r| r.held) {
        0.45
    } else {
        1.0
    };
    c.push_clip(&r.into());
    paint_scale(c, theme, &g, reading.as_ref(), dim);
    paint_note(c, theme, &g, data.tuner_on, reading.as_ref(), dim);
    paint_readout(c, theme, &g, data.tuner_on, reading.as_ref(), ref_hz, dim);
    c.pop_clip();
}

fn paint_scale<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    g: &TunerGeom,
    reading: Option<&Reading>,
    dim: f32,
) {
    let s = g.scale;
    for k in -5..=5 {
        let cents = k as f32 * 10.0;
        let x = needle_x(s, cents).round() + 0.5;
        let major = k % 5 == 0;
        let (y0, y1) = if major {
            (s.y0, s.y1)
        } else {
            (s.y0 + 8.0, s.y1 - 8.0)
        };
        let color = if k == 0 {
            alpha(hsl(140.0, 0.6, 0.52), 0.8)
        } else {
            alpha(theme.ink, if major { 0.3 } else { 0.16 })
        };
        c.stroke_with(Line::new(pt(x, y0), pt(x, y1)), 1.0, color);
        if major {
            let label = match k {
                -5 => "-50".to_string(),
                5 => "+50".to_string(),
                _ => "0".to_string(),
            };
            small(c, theme, label, pt(x, s.y1 + 9.0), Align::Center, None);
        }
    }
    if let Some(r) = reading {
        let x = needle_x(s, r.note.cents);
        let color = alpha(needle_color(r.note.cents), dim);
        c.stroke_with(Line::new(pt(x, s.y0 - 4.0), pt(x, s.y1 + 1.0)), 2.5, color);
        c.fill_with(Circle::new(pt(x, s.y0 - 5.0), 3.0), color);
    }
}

fn paint_note<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    g: &TunerGeom,
    on: bool,
    reading: Option<&Reading>,
    dim: f32,
) {
    let n = g.note;
    match reading {
        Some(r) => {
            let color = alpha(needle_color(r.note.cents), dim);
            let name = key_name(r.note.key);
            c.text(&TextRun::new(name, pt(n.x0 + 2.0, n.y0 + 40.0), 44.0, color).weight(700));
            let w = if name.len() > 1 { 56.0 } else { 36.0 };
            c.text(
                &TextRun::new(
                    key_octave(r.note.key).to_string(),
                    pt(n.x0 + 2.0 + w, n.y0 + 40.0),
                    18.0,
                    alpha(color, 0.8),
                )
                .weight(600),
            );
        }
        None => {
            c.text(
                &TextRun::new(
                    "--",
                    pt(n.x0 + 2.0, n.y0 + 40.0),
                    44.0,
                    alpha(theme.text_muted, 0.5),
                )
                .weight(700),
            );
        }
    }
    small(
        c,
        theme,
        status_text(on, reading),
        pt(n.x0 + 3.0, n.y1 - 2.0),
        Align::Start,
        Some(match reading {
            Some(r) if r.in_tune() && !r.held => needle_color(0.0),
            _ => theme.text_muted,
        }),
    );
}

fn paint_readout<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    g: &TunerGeom,
    on: bool,
    reading: Option<&Reading>,
    ref_hz: f32,
    dim: f32,
) {
    let r = g.readout;
    let text = alpha(theme.text, dim);
    match reading {
        Some(rd) => {
            c.text(&TextRun::new(hz_text(rd.hz), pt(r.x0, r.y0 + 8.0), 11.0, text).weight(600));
            c.text(
                &TextRun::new(
                    cents_text(rd.note.cents),
                    pt(r.center().x, r.y0 + 8.0),
                    11.0,
                    alpha(needle_color(rd.note.cents), dim),
                )
                .weight(600)
                .align(Align::Center),
            );
        }
        None => {
            let hint = if on {
                "-- Hz"
            } else {
                "Press TUNER to tune the input"
            };
            c.text(&TextRun::new(
                hint,
                pt(r.x0, r.y0 + 8.0),
                11.0,
                alpha(theme.text_muted, 0.7),
            ));
        }
    }
    small(
        c,
        theme,
        reference_text(ref_hz),
        pt(r.x1, r.y0 + 8.0),
        Align::End,
        None,
    );
    // The IN TUNE lamp.
    let lit = reading.is_some_and(|r| r.in_tune() && !r.held);
    let lamp = RoundedRect::from_rect(Rect::from_center_size(g.lamp, (14.0, 7.0)), 3.0);
    if lit {
        c.fill_with(lamp, needle_color(0.0));
    } else {
        c.fill_with(lamp, alpha(theme.ink, 0.1));
    }
}
