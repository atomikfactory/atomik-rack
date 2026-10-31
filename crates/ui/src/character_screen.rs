//! The artwork of the Character panel: one small glyph per preset
//! for the 4 x 4 matrix and one animated scene per preset for the screen, all drawn from vector
//! primitives (no bitmaps, nothing taken from any other product). The scene functions are pure:
//! they get the screen rectangle, the effective Transform, the activity (both 0..1) and a phase
//! (cycles; any value) and draw the same list for the same inputs, so goldens use a fixed phase.
//! The scenes react to Transform, to the activity and to the phase.

use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::fx::CHARACTER_NAMES;
use crate::theme::{Theme, mix};
use std::f64::consts::{PI, TAU};
use vello::kurbo::{Arc, BezPath, Circle, Line, Point, Rect, RoundedRect, Shape};
use vello::peniko::Color;

fn alpha(c: Color, a: f32) -> Color {
    c.with_alpha(a)
}

fn poly(points: &[Point], close: bool) -> BezPath {
    let mut path = BezPath::new();
    for (i, p) in points.iter().enumerate() {
        if i == 0 {
            path.move_to(*p);
        } else {
            path.line_to(*p);
        }
    }
    if close && !points.is_empty() {
        path.close_path();
    }
    path
}

fn arc(center: Point, r: f64, start: f64, sweep: f64) -> BezPath {
    Arc::new(center, (r, r), start, sweep, 0.0)
        .path_elements(0.1)
        .collect()
}

/// A deterministic value in 0..1 from an index (SplitMix-style integer hash).
fn hash(i: u32) -> f64 {
    let mut x = u64::from(i).wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

fn unit(v: f32, default: f32) -> f64 {
    f64::from(if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        default
    })
}

fn cycles(v: f32) -> f64 {
    if v.is_finite() {
        f64::from(v).rem_euclid(1.0)
    } else {
        0.0
    }
}

// ------------------------------------------------------------------ glyphs

/// Draws the glyph of `preset` (0..15, matrix order) in the middle of `rect`, in `color`; a lit
/// glyph (the active preset) is drawn a little heavier. Unknown presets draw nothing.
pub fn glyph<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    rect: Rect,
    preset: usize,
    lit: bool,
    color: Color,
) {
    let _ = theme;
    let s = (rect.width().min(rect.height()) * 0.30).max(2.0);
    let (cx, cy) = (rect.center().x, rect.center().y);
    let p = |x: f64, y: f64| pt(cx + x * s, cy + y * s);
    let w = if lit { 1.9 } else { 1.4 };
    let dim = alpha(color, 0.55);
    match preset {
        // Tape reel: two reels joined by tape over a head.
        0 => {
            for x in [-0.62, 0.62] {
                c.stroke_with(Circle::new(p(x, -0.1), 0.52 * s), w, color);
                c.fill_with(Circle::new(p(x, -0.1), 0.14 * s), color);
            }
            c.stroke_with(
                poly(
                    &[p(-0.62, 0.42), p(-0.2, 0.78), p(0.2, 0.78), p(0.62, 0.42)],
                    false,
                ),
                w,
                dim,
            );
        }
        // Loudness contour: high ends, dipped middle.
        1 => {
            let pts: Vec<Point> = (0..=16)
                .map(|k| {
                    let x = -1.0 + 2.0 * f64::from(k) / 16.0;
                    p(x * 1.15, 0.75 - 1.5 * x * x)
                })
                .collect();
            c.stroke_with(poly(&pts, false), w, color);
            c.stroke_with(Line::new(p(-1.15, 0.0), p(1.15, 0.0)), 1.0, dim);
        }
        // Sparkle: one large and two small four-point stars.
        2 => {
            let star = |c: &mut C, at: Point, r: f64, fill: bool| {
                let k = 0.22;
                let pts = [
                    pt(at.x, at.y - r),
                    pt(at.x + k * r, at.y - k * r),
                    pt(at.x + r, at.y),
                    pt(at.x + k * r, at.y + k * r),
                    pt(at.x, at.y + r),
                    pt(at.x - k * r, at.y + k * r),
                    pt(at.x - r, at.y),
                    pt(at.x - k * r, at.y - k * r),
                ];
                if fill {
                    c.fill_with(poly(&pts, true), color);
                } else {
                    c.stroke_with(poly(&pts, true), w, color);
                }
            };
            star(c, p(-0.3, 0.05), 0.95 * s, false);
            star(c, p(0.7, -0.55), 0.38 * s, true);
            star(c, p(0.75, 0.6), 0.28 * s, true);
        }
        // Low end: heavy bars falling to the right.
        3 => {
            for (i, h) in [1.0, 0.82, 0.6, 0.4, 0.24, 0.12].iter().enumerate() {
                let x = -0.95 + i as f64 * 0.38;
                c.stroke_with(
                    Line::new(p(x, 0.85), p(x, 0.85 - 1.7 * h)),
                    2.6,
                    if i < 3 { color } else { dim },
                );
            }
        }
        // Wide: a centre dot with arcs spreading to both sides.
        4 => {
            c.fill_with(Circle::new(p(0.0, 0.1), 0.16 * s), color);
            for (k, r) in [0.55, 0.95, 1.35].iter().enumerate() {
                let a = if k == 2 { dim } else { color };
                c.stroke_with(arc(p(0.0, 0.1), r * s, -0.62, 1.24), w, a);
                c.stroke_with(arc(p(0.0, 0.1), r * s, PI - 0.62, 1.24), w, a);
            }
        }
        // Pocket radio: a small body, a speaker, a dial and an antenna.
        5 => {
            c.stroke_with(
                RoundedRect::from_rect(
                    Rect::new(cx - 0.9 * s, cy - 0.5 * s, cx + 0.9 * s, cy + 0.9 * s),
                    3.0,
                ),
                w,
                color,
            );
            c.stroke_with(Circle::new(p(-0.38, 0.22), 0.32 * s), w, color);
            c.fill_with(Circle::new(p(-0.38, 0.22), 0.08 * s), color);
            c.stroke_with(Line::new(p(0.2, 0.05), p(0.7, 0.05)), w, dim);
            c.stroke_with(Line::new(p(0.2, 0.4), p(0.7, 0.4)), w, dim);
            c.stroke_with(Line::new(p(0.55, -0.5), p(0.95, -1.05)), w, color);
        }
        // Camcorder: a body, a lens and a viewfinder.
        6 => {
            c.stroke_with(
                RoundedRect::from_rect(
                    Rect::new(cx - 0.95 * s, cy - 0.5 * s, cx + 0.3 * s, cy + 0.75 * s),
                    3.0,
                ),
                w,
                color,
            );
            c.stroke_with(
                poly(
                    &[p(0.3, -0.2), p(0.95, -0.55), p(0.95, 0.8), p(0.3, 0.45)],
                    true,
                ),
                w,
                color,
            );
            c.stroke_with(
                Rect::new(cx - 0.65 * s, cy - 0.85 * s, cx - 0.1 * s, cy - 0.5 * s),
                w,
                dim,
            );
            c.fill_with(Circle::new(p(-0.55, 0.1), 0.1 * s), color);
        }
        // Dusty groove: a record with a tonearm.
        7 => {
            for r in [0.92, 0.64, 0.36] {
                c.stroke_with(
                    Circle::new(p(-0.18, 0.05), r * s),
                    if r > 0.9 { w } else { 1.0 },
                    if r > 0.9 { color } else { dim },
                );
            }
            c.fill_with(Circle::new(p(-0.18, 0.05), 0.09 * s), color);
            c.stroke_with(
                poly(&[p(0.95, -0.95), p(0.85, 0.0), p(0.4, 0.4)], false),
                w,
                color,
            );
        }
        // Lossy: a block grid with holes.
        8 => {
            let rows = [[1, 1, 0, 1], [1, 0, 1, 0], [0, 1, 1, 1]];
            for (r, row) in rows.iter().enumerate() {
                for (k, on) in row.iter().enumerate() {
                    let (x, y) = (-0.95 + k as f64 * 0.5, -0.8 + r as f64 * 0.55);
                    let b = Rect::new(
                        cx + x * s,
                        cy + y * s,
                        cx + (x + 0.4) * s,
                        cy + (y + 0.4) * s,
                    );
                    if *on == 1 {
                        c.fill_with(b, if r == 1 { dim } else { color });
                    } else {
                        c.stroke_with(b, 1.0, alpha(color, 0.35));
                    }
                }
            }
        }
        // Swirl: a spiral.
        9 => {
            let pts: Vec<Point> = (0..=48)
                .map(|k| {
                    let t = f64::from(k) / 48.0;
                    let (r, a) = (0.08 + 0.95 * t, t * 2.6 * TAU);
                    p(r * a.cos(), r * a.sin())
                })
                .collect();
            c.stroke_with(poly(&pts, false), w, color);
            c.fill_with(Circle::new(p(0.0, 0.0), 0.1 * s), color);
        }
        // Torn cone: a speaker with a jagged tear.
        10 => {
            c.stroke_with(
                Rect::new(cx - 0.95 * s, cy - 0.4 * s, cx - 0.45 * s, cy + 0.4 * s),
                w,
                color,
            );
            c.stroke_with(
                poly(
                    &[p(-0.45, -0.4), p(0.55, -1.0), p(0.55, 1.0), p(-0.45, 0.4)],
                    true,
                ),
                w,
                color,
            );
            c.stroke_with(
                poly(
                    &[
                        p(-0.15, -0.55),
                        p(0.08, -0.22),
                        p(-0.12, 0.0),
                        p(0.15, 0.28),
                        p(-0.02, 0.6),
                    ],
                    false,
                ),
                w,
                dim,
            );
        }
        // Tin robot: a square head with eyes, a grille and an antenna.
        11 => {
            c.stroke_with(
                RoundedRect::from_rect(
                    Rect::new(cx - 0.75 * s, cy - 0.55 * s, cx + 0.75 * s, cy + 0.8 * s),
                    2.5,
                ),
                w,
                color,
            );
            for x in [-0.33, 0.33] {
                c.fill_with(Circle::new(p(x, -0.12), 0.15 * s), color);
            }
            for x in [-0.4, -0.13, 0.13, 0.4] {
                c.stroke_with(Line::new(p(x, 0.3), p(x, 0.6)), w, dim);
            }
            c.stroke_with(Line::new(p(0.0, -0.55), p(0.0, -0.95)), w, color);
            c.fill_with(Circle::new(p(0.0, -1.0), 0.1 * s), color);
        }
        // Bit bender: a staircase wave.
        12 => {
            let pts = [
                (-1.0, 0.5),
                (-0.6, 0.5),
                (-0.6, -0.1),
                (-0.2, -0.1),
                (-0.2, -0.75),
                (0.2, -0.75),
                (0.2, 0.2),
                (0.6, 0.2),
                (0.6, 0.75),
                (1.0, 0.75),
            ]
            .map(|(x, y)| p(x * 1.1, y));
            c.stroke_with(Line::new(p(-1.1, 0.0), p(1.1, 0.0)), 1.0, alpha(color, 0.3));
            c.stroke_with(poly(&pts, false), w, color);
        }
        // Spin cycle: a drum with a turning arrow.
        13 => {
            c.stroke_with(Circle::new(p(0.0, 0.0), 0.95 * s), w, color);
            c.stroke_with(arc(p(0.0, 0.0), 0.55 * s, 0.4, 4.4), w, color);
            let tip = p(0.55 * 0.4f64.cos(), 0.55 * 0.4f64.sin());
            c.fill_with(
                poly(
                    &[
                        tip,
                        pt(tip.x + 0.28 * s, tip.y - 0.05 * s),
                        pt(tip.x + 0.1 * s, tip.y - 0.3 * s),
                    ],
                    true,
                ),
                color,
            );
            c.fill_with(Circle::new(p(0.0, 0.0), 0.1 * s), dim);
        }
        // Ion sweep: a comb under a rising chirp.
        14 => {
            for i in 0..8 {
                let t = f64::from(i) / 7.0;
                let x = -1.0 + 2.0 * t.powf(1.35);
                c.stroke_with(
                    Line::new(p(x, 0.9), p(x, 0.05 + 0.3 * f64::from(i % 2))),
                    w,
                    dim,
                );
            }
            let pts: Vec<Point> = (0..=32)
                .map(|k| {
                    let t = f64::from(k) / 32.0;
                    p(
                        -1.0 + 2.0 * t,
                        -0.55 + 0.32 * (TAU * (1.5 * t + 1.5 * t * t)).sin(),
                    )
                })
                .collect();
            c.stroke_with(poly(&pts, false), w, color);
        }
        // Ghost pipes: three pipes of different lengths with mouths.
        15 => {
            for (x, h) in [(-0.75, 1.3), (-0.2, 1.75), (0.35, 1.05)] {
                let b = Rect::new(
                    cx + x * s,
                    cy + (0.9 - h) * s,
                    cx + (x + 0.42) * s,
                    cy + 0.9 * s,
                );
                c.stroke_with(RoundedRect::from_rect(b, 3.0), w, color);
                c.stroke_with(
                    Line::new(
                        pt(b.x0 + 3.0, b.y0 + 0.4 * s),
                        pt(b.x1 - 3.0, b.y0 + 0.4 * s),
                    ),
                    1.0,
                    dim,
                );
            }
            c.stroke_with(arc(p(0.56, -0.45), 0.3 * s, -2.6, 1.5), 1.0, dim);
        }
        _ => {}
    }
}

// ------------------------------------------------------------------ scenes

/// The unit square of the screen: `u` and `v` in 0..1 to points of the rectangle.
#[derive(Clone, Copy)]
struct Stage {
    r: Rect,
}

impl Stage {
    fn at(&self, u: f64, v: f64) -> Point {
        pt(
            self.r.x0 + u * self.r.width(),
            self.r.y0 + v * self.r.height(),
        )
    }
    /// Pixels per unit of height (circles are drawn round).
    fn h(&self) -> f64 {
        self.r.height()
    }
}

/// Draws the screen scene of `preset` into `rect` (the inside of the screen). `transform` and
/// `activity` are 0..1 (non-finite values count as 0.5 and 0), `phase` is in cycles. Everything
/// is clipped to `rect`.
#[allow(
    clippy::too_many_arguments,
    reason = "a scene is a pure function of these inputs"
)]
pub fn scene<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    rect: Rect,
    preset: usize,
    transform: f32,
    activity: f32,
    phase: f32,
    accent: Color,
) {
    let t = unit(transform, 0.5);
    let a = unit(activity, 0.0);
    let ph = cycles(phase);
    let st = Stage { r: rect };
    let bright = mix(accent, Color::WHITE, 0.45);
    let faint = alpha(theme.ink, 0.09);
    c.push_clip(&rect.into());
    let sc = Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
    };
    match preset {
        0 => scene_reel(c, &sc),
        1 => scene_loudness(c, &sc),
        2 => scene_sparkle(c, &sc),
        3 => scene_low_end(c, &sc),
        4 => scene_wide(c, &sc),
        5 => scene_radio(c, &sc),
        6 => scene_camcorder(c, &sc),
        7 => scene_groove(c, &sc),
        8 => scene_lossy(c, &sc),
        9 => scene_swirl(c, &sc),
        10 => scene_cone(c, &sc),
        11 => scene_robot(c, &sc),
        12 => scene_bits(c, &sc),
        13 => scene_spin(c, &sc),
        14 => scene_ion(c, &sc),
        15 => scene_pipes(c, &sc),
        _ => {}
    }
    c.pop_clip();
    let _ = theme;
}

/// The inputs of one scene.
#[derive(Clone, Copy)]
struct Sc {
    st: Stage,
    t: f64,
    a: f64,
    ph: f64,
    accent: Color,
    bright: Color,
    faint: Color,
}

fn scene_reel<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let big = 0.34 * st.h();
    let (yc, xl, xr) = (st.at(0.0, 0.46).y, st.at(0.27, 0.0).x, st.at(0.73, 0.0).x);
    let (rl, rr) = (big * (0.35 + 0.6 * (1.0 - t)), big * (0.35 + 0.6 * t));
    for (x, wound) in [(xl, rl), (xr, rr)] {
        let at = pt(x, yc);
        c.fill_with(Circle::new(at, wound), alpha(accent, 0.28));
        c.stroke_with(Circle::new(at, big), 1.5, accent);
        c.stroke_with(Circle::new(at, big * 0.28), 1.25, bright);
        for k in 0..3 {
            let ang = ph * TAU + f64::from(k) * TAU / 3.0;
            let d = pt(ang.cos(), ang.sin());
            c.stroke_with(
                Line::new(
                    pt(x + d.x * big * 0.28, yc + d.y * big * 0.28),
                    pt(x + d.x * big * 0.9, yc + d.y * big * 0.9),
                ),
                1.5,
                alpha(bright, (0.35 + 0.65 * a) as f32),
            );
        }
    }
    let head = st.at(0.5, 0.86);
    c.stroke_with(
        poly(
            &[
                pt(xl, yc + rl),
                pt(head.x - 14.0, head.y),
                pt(head.x + 14.0, head.y),
                pt(xr, yc + rr),
            ],
            false,
        ),
        1.25,
        alpha(bright, 0.8),
    );
    c.fill_with(
        Circle::new(head, 2.0 + 5.0 * a),
        alpha(bright, (0.35 + 0.5 * a) as f32),
    );
    c.stroke_with(Line::new(st.at(0.0, 0.97), st.at(1.0, 0.97)), 1.0, faint);
}

fn scene_loudness<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let mid = st.at(0.0, 0.5).y;
    let amp = 0.4 * st.h();
    c.stroke_with(Line::new(st.at(0.02, 0.5), st.at(0.98, 0.5)), 1.0, faint);
    let gain = |x: f64| -> f64 {
        let bass = 1.0 / (1.0 + ((x - 0.2) * 14.0).exp());
        let treble = 1.0 / (1.0 + (-(x - 0.8) * 14.0).exp());
        let dip = (-((x - 0.5) / 0.16).powi(2)).exp();
        bass + treble - 0.45 * dip
    };
    let pts: Vec<Point> = (0..=64)
        .map(|k| {
            let x = f64::from(k) / 64.0;
            pt(
                st.at(0.04 + 0.92 * x, 0.0).x,
                mid - amp * t * (gain(x) - 0.3),
            )
        })
        .collect();
    let mut fill = poly(&pts, false);
    fill.line_to(pt(pts[64].x, mid));
    fill.line_to(pt(pts[0].x, mid));
    fill.close_path();
    c.fill_with(fill, alpha(accent, 0.18));
    c.stroke_with(poly(&pts, false), 1.75, bright);
    let k = ((ph * 64.0) as usize).min(64);
    c.fill_with(Circle::new(pts[k], 2.0 + 3.5 * a), accent);
}

fn scene_sparkle<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        ..
    } = *s;
    let n = 6 + (t * 10.0) as u32;
    for i in 0..n {
        let (u, v) = (0.06 + 0.88 * hash(i * 3 + 1), 0.1 + 0.8 * hash(i * 3 + 2));
        let tw = 0.5 + 0.5 * (TAU * (ph + hash(i * 3 + 3))).sin();
        let r = st.h() * 0.2 * tw * (0.35 + 0.65 * a) + 1.2;
        let at = st.at(u, v);
        let col = if i % 3 == 0 { bright } else { accent };
        c.stroke_with(Line::new(pt(at.x - r, at.y), pt(at.x + r, at.y)), 1.25, col);
        c.stroke_with(Line::new(pt(at.x, at.y - r), pt(at.x, at.y + r)), 1.25, col);
        c.fill_with(Circle::new(at, 1.0 + 0.8 * tw), col);
    }
}

fn scene_low_end<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let n = 24;
    let pulse = 0.7 + 0.6 * a * (TAU * ph).sin().abs();
    c.stroke_with(Line::new(st.at(0.02, 0.88), st.at(0.98, 0.88)), 1.0, faint);
    for i in 0..n {
        let x = f64::from(i) / f64::from(n - 1);
        let base = 0.12 + 0.1 * (x * 9.0 + 1.0).sin().abs();
        let bump = t * 0.65 * (-(x / 0.2).powi(2)).exp() * pulse;
        let h = (base + bump).clamp(0.02, 0.95) * 0.78 * st.h();
        let xx = st.at(0.04 + 0.92 * x, 0.0).x;
        let y1 = st.at(0.0, 0.88).y;
        c.fill_with(
            Rect::new(xx - 3.0, y1 - h, xx + 3.0, y1),
            if i < 6 {
                alpha(bright, 0.9)
            } else {
                alpha(accent, 0.7)
            },
        );
    }
}

fn scene_wide<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let origin = st.at(0.5, 0.9);
    let spread = 0.15 + 1.0 * t;
    for k in 0..5 {
        let ang = -PI / 2.0 + spread * (f64::from(k) / 2.0 - 1.0);
        let d = pt(ang.cos(), ang.sin());
        let len = 0.8 * st.h();
        c.stroke_with(
            Line::new(origin, pt(origin.x + d.x * len, origin.y + d.y * len)),
            1.0,
            faint,
        );
        let f = (ph + f64::from(k) / 5.0).rem_euclid(1.0);
        let dot = pt(origin.x + d.x * len * f, origin.y + d.y * len * f);
        c.fill_with(
            Circle::new(dot, 1.5 + 2.0 * a),
            if k == 2 { bright } else { accent },
        );
    }
    for side in [-1.0, 1.0] {
        let at = st.at(0.5 + side * (0.06 + 0.28 * t), 0.5);
        c.fill_with(
            Circle::new(at, 0.17 * st.h() * (0.8 + 0.4 * a)),
            alpha(accent, 0.45),
        );
        c.stroke_with(
            Circle::new(at, 0.17 * st.h() * (0.8 + 0.4 * a)),
            1.5,
            bright,
        );
    }
}

fn scene_radio<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    for i in 0..=20 {
        let x = st.at(0.06 + 0.88 * f64::from(i) / 20.0, 0.0).x;
        let len = if i % 5 == 0 { 0.2 } else { 0.1 } * st.h();
        let y = st.at(0.0, 0.28).y;
        c.stroke_with(
            Line::new(pt(x, y), pt(x, y + len)),
            1.0,
            alpha(Color::WHITE, 0.3),
        );
    }
    let nx = st.at(0.06 + 0.88 * t, 0.0).x;
    c.stroke_with(
        Line::new(pt(nx, st.at(0.0, 0.18).y), pt(nx, st.at(0.0, 0.6).y)),
        2.0,
        bright,
    );
    c.stroke_with(Line::new(st.at(0.06, 0.28), st.at(0.94, 0.28)), 1.0, faint);
    for i in 0..7 {
        let f = 0.5 + 0.5 * (TAU * (ph * 3.0 + f64::from(i) / 7.0)).sin();
        let len = (0.1 + 0.28 * (0.2 + 0.8 * a) * f) * st.r.width();
        let y = st.at(0.0, 0.7 + 0.04 * f64::from(i)).y;
        c.stroke_with(
            Line::new(pt(st.at(0.06, 0.0).x, y), pt(st.at(0.06, 0.0).x + len, y)),
            1.5,
            accent,
        );
    }
}

fn scene_camcorder<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let r = st.r.inset(-6.0);
    let l = 14.0;
    for (x, y, dx, dy) in [
        (r.x0, r.y0, 1.0, 1.0),
        (r.x1, r.y0, -1.0, 1.0),
        (r.x0, r.y1, 1.0, -1.0),
        (r.x1, r.y1, -1.0, -1.0),
    ] {
        c.stroke_with(
            poly(&[pt(x + dx * l, y), pt(x, y), pt(x, y + dy * l)], false),
            1.5,
            bright,
        );
    }
    if ph < 0.5 {
        c.fill_with(
            Circle::new(pt(r.x0 + 12.0, r.y0 + 12.0), 3.5),
            Color::from_rgba8(0xe8, 0x54, 0x54, 255),
        );
    }
    let amp = 2.0 + 14.0 * t;
    let pts: Vec<Point> = (0..=80)
        .map(|k| {
            let x = f64::from(k) / 80.0;
            pt(
                st.at(0.1 + 0.8 * x, 0.0).x,
                st.at(0.0, 0.52).y
                    + amp * (TAU * (3.0 * x + ph)).sin() * (TAU * (0.5 * x + ph)).cos(),
            )
        })
        .collect();
    c.stroke_with(poly(&pts, false), 1.5, accent);
    let n = (a * 24.0) as u32;
    let seed = (ph * 8.0) as u32 * 100;
    for i in 0..n {
        let at = st.at(
            0.08 + 0.84 * hash(seed + i * 2),
            0.1 + 0.8 * hash(seed + i * 2 + 1),
        );
        c.fill_with(
            Rect::new(at.x, at.y, at.x + 1.5, at.y + 1.5),
            alpha(Color::WHITE, 0.55),
        );
    }
    c.stroke_with(Line::new(st.at(0.1, 0.52), st.at(0.9, 0.52)), 1.0, faint);
}

fn scene_groove<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let (ctr, r) = (st.at(0.42, 0.5), 0.44 * st.h());
    c.fill_with(Circle::new(ctr, r), alpha(Color::BLACK, 0.4));
    for k in 1..=8 {
        let rr = r * (0.34 + 0.08 * f64::from(k));
        c.stroke_with(
            Circle::new(ctr, rr),
            1.0,
            if k == 8 { accent } else { faint },
        );
    }
    c.fill_with(Circle::new(ctr, r * 0.2), alpha(accent, 0.6));
    let ang = ph * TAU;
    c.stroke_with(
        Line::new(
            ctr,
            pt(ctr.x + ang.cos() * r * 0.9, ctr.y + ang.sin() * r * 0.9),
        ),
        1.25,
        alpha(bright, 0.8),
    );
    let hub = st.at(0.88, 0.14);
    c.stroke_with(
        poly(
            &[hub, st.at(0.8, 0.42), pt(ctr.x + r * 0.62, ctr.y - r * 0.2)],
            false,
        ),
        1.75,
        bright,
    );
    let n = 2 + (t * 14.0 + a * 8.0) as u32;
    let seed = (ph * 8.0) as u32 * 64;
    for i in 0..n {
        let (rr, th) = (
            r * (0.4 + 0.55 * hash(seed + i * 2)),
            TAU * hash(seed + i * 2 + 1),
        );
        c.fill_with(
            Circle::new(pt(ctr.x + rr * th.cos(), ctr.y + rr * th.sin()), 1.2 + a),
            alpha(Color::WHITE, 0.7),
        );
    }
}

fn scene_lossy<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let (cols, rows) = (16u32, 6u32);
    let flick = (ph * 6.0) as u32 * 1000;
    let (w, h) = (
        st.r.width() * 0.84 / f64::from(cols),
        st.h() * 0.78 / f64::from(rows),
    );
    let x0 = st.at(0.04, 0.0).x;
    let y0 = st.at(0.0, 0.1).y;
    for r in 0..rows {
        for k in 0..cols {
            let i = r * cols + k;
            let b = Rect::new(
                x0 + f64::from(k) * w + 1.0,
                y0 + f64::from(r) * h + 1.0,
                x0 + f64::from(k + 1) * w - 1.0,
                y0 + f64::from(r + 1) * h - 1.0,
            );
            let level = hash(i + 7);
            let alive = level > t * 0.8 + 0.1 * hash(i + flick);
            if alive {
                let s = 0.35 + 0.65 * level * (0.5 + 0.5 * a);
                c.fill_with(
                    b,
                    alpha(if k > cols * 2 / 3 { bright } else { accent }, s as f32),
                );
            } else {
                c.stroke_with(b, 1.0, faint);
            }
        }
    }
    let edge = st.at(0.04 + 0.84 * (1.0 - 0.35 * t), 0.0).x;
    c.stroke_with(
        Line::new(pt(edge, st.at(0.0, 0.04).y), pt(edge, st.at(0.0, 0.96).y)),
        1.5,
        bright,
    );
}

fn scene_swirl<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        ..
    } = *s;
    let arms = 2 + (t * 2.99) as u32;
    let ctr = st.at(0.5, 0.5);
    let r = 0.46 * st.h() * (0.6 + 0.4 * a);
    for arm in 0..arms {
        let pts: Vec<Point> = (0..=36)
            .map(|k| {
                let f = f64::from(k) / 36.0;
                let ang = f64::from(arm) * TAU / f64::from(arms) + f * 3.6 + ph * TAU;
                pt(ctr.x + ang.cos() * r * f * 1.5, ctr.y + ang.sin() * r * f)
            })
            .collect();
        c.stroke_with(
            poly(&pts, false),
            1.5,
            if arm == 0 { bright } else { accent },
        );
    }
    c.fill_with(Circle::new(ctr, 2.5), bright);
}

fn scene_cone<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let shake = (TAU * ph * 6.0).sin() * a * 3.0;
    let (cx, cy, h) = (st.at(0.4, 0.5).x + shake, st.at(0.4, 0.5).y, st.h());
    c.stroke_with(
        Rect::new(cx - 0.34 * h, cy - 0.16 * h, cx - 0.14 * h, cy + 0.16 * h),
        1.5,
        accent,
    );
    let cone = [
        pt(cx - 0.14 * h, cy - 0.16 * h),
        pt(cx + 0.2 * h, cy - 0.4 * h),
        pt(cx + 0.2 * h, cy + 0.4 * h),
        pt(cx - 0.14 * h, cy + 0.16 * h),
    ];
    c.fill_with(poly(&cone, true), alpha(accent, 0.2));
    c.stroke_with(poly(&cone, true), 1.5, accent);
    let segs = 2 + (t * 8.0) as u32;
    let pts: Vec<Point> = (0..=segs)
        .map(|k| {
            let f = f64::from(k) / f64::from(segs);
            let jag = if k % 2 == 0 { -1.0 } else { 1.0 } * 0.05 * h;
            pt(
                cx - 0.1 * h + f * 0.28 * h + jag,
                cy - 0.3 * h + f * 0.6 * h * (0.4 + 0.6 * t),
            )
        })
        .collect();
    c.stroke_with(poly(&pts, false), 1.75, bright);
    for k in 1..=3 {
        let r = f64::from(k) * 0.16 * h + (TAU * (ph + f64::from(k) * 0.2)).sin() * a * 3.0;
        c.stroke_with(
            arc(pt(cx + 0.2 * h, cy), r, -0.8, 1.6),
            1.0,
            alpha(bright, (0.7 - 0.15 * f64::from(k)) as f32),
        );
    }
    c.stroke_with(Line::new(st.at(0.04, 0.9), st.at(0.96, 0.9)), 1.0, faint);
}

fn scene_robot<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let h = st.h();
    let face = Rect::new(
        st.at(0.5, 0.0).x - 0.36 * h,
        st.at(0.0, 0.2).y,
        st.at(0.5, 0.0).x + 0.36 * h,
        st.at(0.0, 0.92).y,
    );
    c.fill_with(RoundedRect::from_rect(face, 4.0), alpha(accent, 0.14));
    c.stroke_with(RoundedRect::from_rect(face, 4.0), 1.5, accent);
    let lines = 3 + (t * 9.0) as u32;
    for i in 0..lines {
        let y = face.y0 + face.height() * (f64::from(i) + 0.5) / f64::from(lines);
        c.stroke_with(
            Line::new(pt(face.x0 + 2.0, y), pt(face.x1 - 2.0, y)),
            1.0,
            faint,
        );
    }
    for sx in [-1.0, 1.0] {
        let ex = face.center().x + sx * 0.17 * h;
        let ey = face.y0 + 0.28 * face.height();
        c.stroke_with(Circle::new(pt(ex, ey), 0.075 * h), 1.25, bright);
        c.fill_with(
            Circle::new(pt(ex + (TAU * ph).sin() * 2.0, ey), 0.03 * h),
            bright,
        );
    }
    let level = 0.15 + 0.85 * a;
    for i in 0..7 {
        let f = 0.25 + 0.75 * (TAU * (ph * 2.0 + f64::from(i) * 0.37)).sin().abs();
        let x = face.x0 + face.width() * (0.2 + 0.1 * f64::from(i));
        let hh = 0.2 * face.height() * f * level;
        let y = face.y0 + 0.78 * face.height();
        c.stroke_with(Line::new(pt(x, y - hh), pt(x, y + hh)), 2.0, bright);
    }
    let top = pt(face.center().x, face.y0);
    c.stroke_with(
        Line::new(top, pt(top.x + (TAU * ph).sin() * 3.0, top.y - 0.12 * h)),
        1.25,
        bright,
    );
}

fn scene_bits<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let levels = 2.0 + ((1.0 - t) * 14.0).round();
    let step_px = 2.0 + t * 28.0;
    let amp = (0.5 + 0.4 * a) * 0.42 * st.h();
    let mid = st.at(0.0, 0.5).y;
    let (x0, x1) = (st.at(0.03, 0.0).x, st.at(0.97, 0.0).x);
    c.stroke_with(Line::new(pt(x0, mid), pt(x1, mid)), 1.0, faint);
    let mut pts = Vec::new();
    let mut x = x0;
    while x < x1 {
        let f = (x - x0) / (x1 - x0);
        let s = (TAU * (2.0 * f + ph)).sin();
        let q = (s * levels / 2.0).round() / (levels / 2.0);
        let y = mid - q * amp;
        pts.push(pt(x, y));
        pts.push(pt((x + step_px).min(x1), y));
        x += step_px;
    }
    c.stroke_with(poly(&pts, false), 1.75, bright);
    let smooth: Vec<Point> = (0..=64)
        .map(|k| {
            let f = f64::from(k) / 64.0;
            pt(x0 + f * (x1 - x0), mid - (TAU * (2.0 * f + ph)).sin() * amp)
        })
        .collect();
    c.stroke_with(poly(&smooth, false), 1.0, alpha(accent, 0.35));
}

fn scene_spin<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let (ctr, r) = (st.at(0.5, 0.5), 0.43 * st.h());
    c.stroke_with(Circle::new(ctr, r), 1.75, accent);
    c.stroke_with(Circle::new(ctr, r * 0.86), 1.0, faint);
    let level = ctr.y + r * (0.45 - 0.9 * t);
    let pts: Vec<Point> = (0..=24)
        .map(|k| {
            let f = f64::from(k) / 24.0;
            let x = ctr.x - r * 0.8 + f * r * 1.6;
            pt(x, level + (TAU * (ph + 1.5 * f)).sin() * 3.0)
        })
        .collect();
    c.stroke_with(poly(&pts, false), 1.25, alpha(bright, 0.8));
    for i in 0..6 {
        let ang = ph * TAU * (1.0 + 0.5 * t) + f64::from(i) * TAU / 6.0;
        let rr = r * (0.3 + 0.35 * hash(i + 40));
        c.fill_with(
            Circle::new(
                pt(ctr.x + ang.cos() * rr, ctr.y + ang.sin() * rr),
                2.0 + 3.5 * a,
            ),
            if i % 2 == 0 { bright } else { accent },
        );
    }
}

fn scene_ion<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let n = 24;
    let bend = 1.0 + 2.0 * (1.0 - t);
    let base = st.at(0.0, 0.86).y;
    for i in 0..n {
        let f = f64::from(i) / f64::from(n - 1);
        let u = 0.04 + 0.92 * f.powf(bend);
        let shimmer = if i % 2 == 0 { (TAU * ph).sin() } else { 0.0 };
        let x = st.at(u, 0.0).x + shimmer;
        let top = st.at(0.0, if i % 2 == 0 { 0.2 } else { 0.38 }).y;
        c.stroke_with(
            Line::new(pt(x, base), pt(x, top)),
            1.5,
            alpha(accent, (0.35 + 0.65 * a) as f32),
        );
    }
    let sx = st.at(0.04 + 0.92 * t, 0.0).x;
    c.stroke_with(
        Line::new(pt(sx, st.at(0.0, 0.08).y), pt(sx, base)),
        2.0,
        bright,
    );
    c.stroke_with(Line::new(st.at(0.03, 0.88), st.at(0.97, 0.88)), 1.0, faint);
}

fn scene_pipes<C: Canvas + ?Sized>(c: &mut C, s: &Sc) {
    let Sc {
        st,
        t,
        a,
        ph,
        accent,
        bright,
        faint,
        ..
    } = *s;
    let n = 8;
    let base = st.at(0.0, 0.9).y;
    c.stroke_with(Line::new(st.at(0.03, 0.9), st.at(0.97, 0.9)), 1.0, faint);
    for k in 0..n {
        let kf = f64::from(k);
        let x = st.at(0.1 + 0.8 * kf / f64::from(n - 1), 0.0).x;
        let height = 0.7 * st.h() / (1.0 + kf * (0.3 + 0.7 * t));
        c.stroke_with(
            Line::new(pt(x, base), pt(x, base - height)),
            3.0,
            alpha(accent, 0.6),
        );
        let f = (ph + kf / f64::from(n)).rem_euclid(1.0);
        c.stroke_with(
            Circle::new(
                pt(x, base - height),
                f * 0.22 * st.h() * (0.3 + 0.7 * a) + 1.0,
            ),
            1.0,
            alpha(bright, ((1.0 - f) * (0.4 + 0.6 * a)) as f32),
        );
    }
}

// ------------------------------------------------------------------ contact sheet

/// Draws all 16 screens on a 4 x 4 sheet inside `rect`, each in its own framed cell with its name
/// below the scene: the golden of the artwork (`transform`, `activity` and `phase` as for
/// [`scene`]).
pub fn contact_sheet<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    rect: Rect,
    transform: f32,
    activity: f32,
    phase: f32,
) {
    let accent = theme.accent_for_type("atomik.character");
    let (cw, ch) = (rect.width() / 4.0, rect.height() / 4.0);
    for (i, name) in CHARACTER_NAMES.iter().enumerate() {
        let (col, row) = ((i % 4) as f64, (i / 4) as f64);
        let cell = Rect::new(
            rect.x0 + col * cw + 4.0,
            rect.y0 + row * ch + 4.0,
            rect.x0 + (col + 1.0) * cw - 4.0,
            rect.y0 + (row + 1.0) * ch - 4.0,
        );
        c.fill_with(RoundedRect::from_rect(cell, 3.0), theme.lcd);
        c.stroke_with(
            RoundedRect::from_rect(cell.inset(-0.5), 3.5),
            1.0,
            alpha(theme.ink, 0.07),
        );
        let inner = Rect::new(cell.x0 + 2.0, cell.y0 + 2.0, cell.x1 - 2.0, cell.y1 - 14.0);
        scene(c, theme, inner, i, transform, activity, phase, accent);
        c.text(
            &TextRun::new(
                name.to_uppercase(),
                pt(cell.x0 + 6.0, cell.y1 - 4.0),
                7.5,
                theme.text_muted,
            )
            .align(Align::Start)
            .weight(600),
        );
    }
}
