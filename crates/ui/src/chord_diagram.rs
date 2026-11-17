//! The chord diagram widget: a fretboard window with the dots of one [`Voicing`].
//!
//! It draws for any string count (1 to 12), in tab order (string 0 on the left, as the top line
//! of a tab staff is the highest course), five fret rows from [`Voicing::base_fret`], with
//!
//! - a thick nut when the window starts at the first fret and a `Nfr` label when it does not,
//! - the open (ring) and muted (cross) markers above the grid,
//! - one dot per fretted string with its finger number, and a bar for a barre,
//! - a short string (the banjo drone) that starts at its own fret: its line begins where the
//!   string does and its open marker sits there.
//!
//! The widget is one function over a [`Canvas`]: [`paint_diagram`] draws into the width of a
//! rectangle and [`diagram_height`] says how tall that is, so a caller can lay a row of diagrams
//! out. Frets are relative to the capo, like the voicing the generator returns and the tab
//! sidecar stores; [`Diagram::start_frets_of`] gives the matching `start_frets` of a tuning.
//!
//! ```text
//! let d = Diagram { name: Some("Am"), voicing: &v, start_frets: &starts };
//! let h = chord_diagram::diagram_height(64.0, true);
//! chord_diagram::paint_diagram(canvas, &theme, Rect::new(x, y, x + 64.0, y + h), &d);
//! ```

use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::theme::{Theme, mix};
use atomik_tab::voicing::Voicing;
use atomik_tab::{Capo, Tuning};
use vello::kurbo::{Circle, Line, Rect, RoundedRect};

/// How many fret rows a diagram shows.
pub const FRET_ROWS: usize = 5;

/// One diagram: what to draw.
#[derive(Debug, Clone, Copy)]
pub struct Diagram<'a> {
    /// The chord's name, drawn above the grid (verbatim); `None` leaves the title row out.
    pub name: Option<&'a str>,
    /// The fingering, frets relative to the capo, in tab order.
    pub voicing: &'a Voicing,
    /// The first fret each string can be pressed at, relative to the capo, in tab order: 0 for an
    /// ordinary string, 5 for the banjo's drone. A missing entry counts as 0.
    pub start_frets: &'a [u8],
}

impl Diagram<'_> {
    /// The `start_frets` of `tuning` played with `capo` (the drone of an open-G banjo is 5, 4 with
    /// a capo on the first fret, since a capo raises every string including the drone).
    pub fn start_frets_of(tuning: &Tuning, capo: Capo) -> Vec<u8> {
        tuning
            .strings
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if s.start_fret == 0 {
                    0
                } else {
                    s.start_fret.saturating_sub(capo.raise(i)).max(1)
                }
            })
            .collect()
    }
}

/// The height of a diagram drawn `width` wide: the title row (when `titled`), the marker row and
/// [`FRET_ROWS`] fret rows.
pub fn diagram_height(width: f64, titled: bool) -> f64 {
    width * (if titled { TITLE } else { 0.0 } + MARKERS + ROW * FRET_ROWS as f64)
}

// Row heights as fractions of the width.
const TITLE: f64 = 0.24;
const MARKERS: f64 = 0.13;
const ROW: f64 = 0.2;
/// The share of the width left of the first string (the base fret label sits there).
const LEFT: f64 = 0.2;
/// The share of the width right of the last string.
const RIGHT: f64 = 0.08;

/// Draws `d` into `area`, from its top-left corner and across its width (see [`diagram_height`]
/// for the height it needs). Text is clipped to nothing: a caller that wants a clip pushes one.
pub fn paint_diagram(c: &mut dyn Canvas, theme: &Theme, area: Rect, d: &Diagram<'_>) {
    let w = area.width();
    let n = d.voicing.frets.len().max(1);
    let ink = theme.text;
    let faint = mix(theme.text_muted, theme.background, 0.35);
    let titled = d.name.is_some();
    let title_h = if titled { w * TITLE } else { 0.0 };
    let marker_h = w * MARKERS;
    let row_h = w * ROW;
    let gx0 = area.x0 + w * LEFT;
    let gx1 = area.x1 - w * RIGHT;
    let gy0 = area.y0 + title_h + marker_h;
    let gy1 = gy0 + row_h * FRET_ROWS as f64;
    let dx = if n > 1 {
        (gx1 - gx0) / (n - 1) as f64
    } else {
        0.0
    };
    let sx = |i: usize| {
        if n > 1 {
            gx0 + dx * i as f64
        } else {
            (gx0 + gx1) / 2.0
        }
    };
    let base = i32::from(d.voicing.base_fret.max(1));
    let start_of = |i: usize| i32::from(d.start_frets.get(i).copied().unwrap_or(0));
    // The y of the top of string `i`: a short string starts at its own fret wire.
    let top_of = |i: usize| -> Option<f64> {
        let wire = (start_of(i) - (base - 1)).max(0);
        (wire < FRET_ROWS as i32).then(|| gy0 + row_h * f64::from(wire))
    };

    if let Some(name) = d.name {
        let size = (w * 0.19).clamp(8.0, 15.0) as f32;
        c.text(
            &TextRun::verbatim(
                name,
                pt(area.center().x, area.y0 + title_h * 0.78),
                size,
                ink,
            )
            .align(Align::Center)
            .weight(700),
        );
    }

    // Fret wires; the nut is thick when the window starts at the first fret.
    for r in 0..=FRET_ROWS {
        let y = gy0 + row_h * r as f64;
        let thick = r == 0 && base == 1;
        c.stroke_with(
            Line::new(pt(gx0, y), pt(gx1, y)),
            if thick { (w * 0.045).max(2.0) } else { 1.0 },
            if thick { ink } else { faint },
        );
    }
    // Strings.
    for i in 0..n {
        if let Some(top) = top_of(i) {
            let x = sx(i);
            c.stroke_with(Line::new(pt(x, top), pt(x, gy1)), 1.0, mix(ink, faint, 0.4));
        }
    }
    if base > 1 {
        let size = (row_h * 0.62).clamp(7.0, 12.0) as f32;
        c.text(
            &TextRun::verbatim(
                format!("{base}fr"),
                pt(gx0 - w * 0.05, gy0 + row_h * 0.72),
                size,
                ink,
            )
            .align(Align::End),
        );
    }

    let r_dot = (dx * 0.42).min(row_h * 0.42).max(2.0);
    let r_mark = (r_dot * 0.7).max(1.8);
    // Open and muted markers above the grid (at the top of a short string).
    for (i, f) in d.voicing.frets.iter().enumerate() {
        let Some(top) = top_of(i) else { continue };
        let y = if top > gy0 + 0.5 {
            top - r_mark - 1.0
        } else {
            gy0 - marker_h * 0.5 - 1.0
        };
        match f {
            Some(0) => c.stroke_with(Circle::new(pt(sx(i), y), r_mark), 1.2, ink),
            None => {
                let k = r_mark * 0.85;
                c.stroke_with(
                    Line::new(pt(sx(i) - k, y - k), pt(sx(i) + k, y + k)),
                    1.4,
                    faint,
                );
                c.stroke_with(
                    Line::new(pt(sx(i) - k, y + k), pt(sx(i) + k, y - k)),
                    1.4,
                    faint,
                );
            }
            Some(_) => {}
        }
    }

    let row_center = |fret: u8| -> Option<f64> {
        let row = i32::from(fret) - base;
        (0..FRET_ROWS as i32)
            .contains(&row)
            .then(|| gy0 + row_h * (f64::from(row) + 0.5))
    };
    // Barres: a rounded bar across its strings.
    for b in &d.voicing.barres {
        let Some(y) = row_center(b.fret) else {
            continue;
        };
        let (a, z) = (b.from.min(b.to), b.from.max(b.to).min(n - 1));
        let bar = Rect::new(sx(a) - r_dot, y - r_dot, sx(z) + r_dot, y + r_dot);
        c.fill_with(RoundedRect::from_rect(bar, r_dot), ink);
    }
    // Dots with their finger.
    for (i, f) in d.voicing.frets.iter().enumerate() {
        let Some(fret) = f.filter(|f| *f > 0) else {
            continue;
        };
        let Some(y) = row_center(fret) else { continue };
        let under_barre = d
            .voicing
            .barres
            .iter()
            .any(|b| b.fret == fret && (b.from.min(b.to)..=b.from.max(b.to)).contains(&i));
        if !under_barre {
            c.fill_with(Circle::new(pt(sx(i), y), r_dot), ink);
        }
        if let Some(Some(finger)) = d.voicing.fingers.get(i)
            && r_dot >= 4.0
        {
            c.text(
                &TextRun::verbatim(
                    finger.to_string(),
                    pt(sx(i), y + r_dot * 0.42),
                    (r_dot * 1.2) as f32,
                    theme.background,
                )
                .align(Align::Center)
                .weight(700),
            );
        }
    }
}
