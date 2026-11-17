//! Chord diagrams on paper: the same fretboard window as the editor's widget (any string count, a
//! nut or an `Nfr` label, open and muted markers, finger numbers, barres and the banjo's short
//! string that starts at its own fret), drawn into a page as lines, dots and texts.

use super::draw::{Anchor, Ink, Page};
use super::font::Font;
use crate::instrument::{Capo, Tuning};
use crate::voicing::Voicing;

/// How many fret rows a diagram shows.
pub const FRET_ROWS: usize = 5;

// Row heights as fractions of the width (the editor's proportions).
const TITLE: f32 = 0.26;
const MARKERS: f32 = 0.14;
const ROW: f32 = 0.2;
const LEFT: f32 = 0.2;
const RIGHT: f32 = 0.08;

/// The first fret each string can be pressed at, relative to the capo, in tab order: 0 for an
/// ordinary string, the drone's fret for a banjo (one less per fret of capo).
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

/// The height of a diagram `width` points wide.
pub fn diagram_height(width: f32, titled: bool) -> f32 {
    width * (if titled { TITLE } else { 0.0 } + MARKERS + ROW * FRET_ROWS as f32)
}

/// Draws `voicing` with its `name` in a box whose top-left corner is `(x, y)` and whose width is
/// `w`; returns the height used.
pub fn draw_diagram(
    page: &mut Page,
    (x, y): (f32, f32),
    w: f32,
    name: Option<&str>,
    voicing: &Voicing,
    start_frets: &[u8],
) -> f32 {
    let n = voicing.frets.len().max(1);
    let titled = name.is_some();
    let title_h = if titled { w * TITLE } else { 0.0 };
    let marker_h = w * MARKERS;
    let row_h = w * ROW;
    let gx0 = x + w * LEFT;
    let gx1 = x + w - w * RIGHT;
    let gy0 = y + title_h + marker_h;
    let gy1 = gy0 + row_h * FRET_ROWS as f32;
    let dx = if n > 1 {
        (gx1 - gx0) / (n - 1) as f32
    } else {
        0.0
    };
    let sx = |i: usize| {
        if n > 1 {
            gx0 + dx * i as f32
        } else {
            (gx0 + gx1) / 2.0
        }
    };
    let base = i32::from(voicing.base_fret.max(1));
    let start_of = |i: usize| i32::from(start_frets.get(i).copied().unwrap_or(0));
    let top_of = |i: usize| -> Option<f32> {
        let wire = (start_of(i) - (base - 1)).max(0);
        (wire < FRET_ROWS as i32).then_some(gy0 + row_h * wire as f32)
    };

    if let Some(name) = name {
        let size = (w * 0.24).clamp(6.5, 11.0);
        page.text(
            x + w * 0.5,
            y + title_h * 0.8,
            size,
            Font::Bold,
            Ink::Black,
            Anchor::Middle,
            name,
        );
    }
    for r in 0..=FRET_ROWS {
        let yy = gy0 + row_h * r as f32;
        if r == 0 && base == 1 {
            page.line((gx0, yy), (gx1, yy), (w * 0.04).max(1.4), Ink::Black);
        } else {
            page.line((gx0, yy), (gx1, yy), 0.4, Ink::Mid);
        }
    }
    for i in 0..n {
        if let Some(top) = top_of(i) {
            page.line((sx(i), top), (sx(i), gy1), 0.5, Ink::Dark);
        }
    }
    if base > 1 {
        let size = (row_h * 0.62).clamp(5.0, 8.0);
        page.text(
            gx0 - w * 0.04,
            gy0 + row_h * 0.7,
            size,
            Font::Regular,
            Ink::Black,
            Anchor::End,
            &format!("{base}fr"),
        );
    }
    let r_dot = (dx * 0.42).min(row_h * 0.42).max(1.6);
    let r_mark = (r_dot * 0.7).max(1.3);
    for (i, f) in voicing.frets.iter().enumerate() {
        let Some(top) = top_of(i) else { continue };
        let yy = if top > gy0 + 0.5 {
            top - r_mark - 1.0
        } else {
            gy0 - marker_h * 0.5 - 0.5
        };
        match f {
            Some(0) => page.ring((sx(i), yy), r_mark, 0.7, Ink::Black),
            None => {
                let k = r_mark * 0.85;
                page.line((sx(i) - k, yy - k), (sx(i) + k, yy + k), 0.7, Ink::Dark);
                page.line((sx(i) - k, yy + k), (sx(i) + k, yy - k), 0.7, Ink::Dark);
            }
            Some(_) => {}
        }
    }
    let row_center = |fret: u8| -> Option<f32> {
        let row = i32::from(fret) - base;
        (0..FRET_ROWS as i32)
            .contains(&row)
            .then_some(gy0 + row_h * (row as f32 + 0.5))
    };
    for b in &voicing.barres {
        let Some(yy) = row_center(b.fret) else {
            continue;
        };
        let (a, z) = (b.from.min(b.to), b.from.max(b.to).min(n - 1));
        page.round_line((sx(a), yy), (sx(z), yy), r_dot * 2.0, Ink::Black);
    }
    for (i, f) in voicing.frets.iter().enumerate() {
        let Some(fret) = f.filter(|f| *f > 0) else {
            continue;
        };
        let Some(yy) = row_center(fret) else { continue };
        let under_barre = voicing
            .barres
            .iter()
            .any(|b| b.fret == fret && (b.from.min(b.to)..=b.from.max(b.to)).contains(&i));
        if !under_barre {
            page.dot((sx(i), yy), r_dot, Ink::Black);
        }
        if let Some(Some(finger)) = voicing.fingers.get(i)
            && r_dot >= 2.4
        {
            page.text(
                sx(i),
                yy + r_dot * 0.4,
                r_dot * 1.2,
                Font::Bold,
                Ink::White,
                Anchor::Middle,
                &finger.to_string(),
            );
        }
    }
    title_h + marker_h + row_h * FRET_ROWS as f32
}
