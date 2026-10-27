//! The bitmaps of the edit-tool mouse cursors (pen, eraser, razor), drawn in code.
//!
//! Each cursor is a few shapes in a 32 x 32 design grid, rasterised at the size the window's
//! scale asks for (4 x 4 samples per pixel, straight alpha), so it is crisp at 1x and 2x. A
//! dark outline around light fills keeps it readable on dark and light content. The platform
//! layer wraps the result in a window-system cursor; nothing here touches the window system.

/// An edit tool's cursor picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CursorArt {
    /// A pencil: the tip is the hotspot.
    Pen,
    /// An eraser block: the middle of its working end is the hotspot.
    Eraser,
    /// A pair of scissors: the point between the blade tips is the hotspot.
    Razor,
}

/// A rendered cursor: straight-alpha RGBA, `size` x `size` pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorBitmap {
    /// The pixels, row by row.
    pub rgba: Vec<u8>,
    /// The side in pixels.
    pub size: u32,
    /// The pixel that is the pointer position.
    pub hotspot: (u32, u32),
}

/// The side of the design grid.
const DESIGN: f32 = 32.0;
/// The smallest and largest bitmap side, in pixels.
const MIN_SIZE: u32 = 32;
const MAX_SIZE: u32 = 128;
/// Samples per pixel side.
const SAMPLES: u32 = 4;
/// How far the outline reaches around a shape, in design units.
const OUTLINE: f32 = 1.3;
const OUTLINE_COLOR: [u8; 3] = [24, 24, 30];

type Pt = (f32, f32);

/// One filled shape.
enum Shape {
    /// A polygon (any winding).
    Poly(Vec<Pt>, [u8; 3]),
    /// A thick segment with round ends: the point pair and the radius.
    Capsule(Pt, Pt, f32, [u8; 3]),
    /// A ring: the centre, the inner and the outer radius.
    Ring(Pt, f32, f32, [u8; 3]),
}

impl Shape {
    fn color(&self) -> [u8; 3] {
        match self {
            Shape::Poly(_, c) | Shape::Capsule(.., c) | Shape::Ring(.., c) => *c,
        }
    }

    /// The signed distance of `p` to the shape: negative inside.
    fn distance(&self, p: Pt) -> f32 {
        match self {
            Shape::Poly(points, _) => poly_distance(points, p),
            Shape::Capsule(a, b, r, _) => segment_distance(*a, *b, p) - r,
            Shape::Ring(c, inner, outer, _) => {
                let d = ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt();
                (d - outer).max(inner - d)
            }
        }
    }
}

fn segment_distance(a: Pt, b: Pt, p: Pt) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt()
}

fn poly_distance(points: &[Pt], p: Pt) -> f32 {
    let mut dist = f32::MAX;
    let mut inside = false;
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        dist = dist.min(segment_distance(a, b, p));
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
    }
    if inside { -dist } else { dist }
}

/// A point on a slanted axis: `u` along it (from `origin`, toward the upper right), `v` across.
fn slanted(origin: Pt, u: f32, v: f32) -> Pt {
    let k = std::f32::consts::FRAC_1_SQRT_2;
    (origin.0 + (u + v) * k, origin.1 + (v - u) * k)
}

/// A slanted rectangle spanning `u0..u1` along the axis and `v0..v1` across it.
fn slab(origin: Pt, u: (f32, f32), v: (f32, f32), color: [u8; 3]) -> Shape {
    Shape::Poly(
        vec![
            slanted(origin, u.0, v.0),
            slanted(origin, u.1, v.0),
            slanted(origin, u.1, v.1),
            slanted(origin, u.0, v.1),
        ],
        color,
    )
}

/// The shapes of a cursor, back to front, and its hotspot (design units).
fn design(art: CursorArt) -> (Vec<Shape>, Pt) {
    match art {
        CursorArt::Pen => {
            let o = (3.0, 29.0);
            let shapes = vec![
                // The sharpened wood, the lead, the body and the rubber at the end.
                Shape::Poly(
                    vec![
                        slanted(o, 0.0, 0.0),
                        slanted(o, 8.0, -2.6),
                        slanted(o, 8.0, 2.6),
                    ],
                    [238, 206, 160],
                ),
                Shape::Poly(
                    vec![
                        slanted(o, 0.0, 0.0),
                        slanted(o, 2.6, -0.9),
                        slanted(o, 2.6, 0.9),
                    ],
                    [50, 50, 56],
                ),
                slab(o, (8.0, 24.0), (-2.6, 2.6), [250, 205, 60]),
                slab(o, (24.0, 28.0), (-2.6, 2.6), [236, 120, 140]),
            ];
            (shapes, o)
        }
        CursorArt::Eraser => {
            let o = (6.0, 26.0);
            let shapes = vec![
                slab(o, (0.0, 9.0), (-4.5, 4.5), [240, 130, 150]),
                slab(o, (9.0, 24.0), (-4.5, 4.5), [236, 238, 242]),
                slab(o, (9.0, 11.0), (-4.5, 4.5), [120, 130, 150]),
            ];
            (shapes, o)
        }
        CursorArt::Razor => {
            let blade = [226, 232, 238];
            let ring = [226, 72, 72];
            let shapes = vec![
                // Two blades that cross at the pivot, each ending in a handle ring.
                Shape::Capsule((22.0, 22.0), (10.5, 2.5), 1.5, blade),
                Shape::Capsule((10.0, 22.0), (21.5, 2.5), 1.5, blade),
                Shape::Ring((22.0, 26.0), 2.0, 4.6, ring),
                Shape::Ring((10.0, 26.0), 2.0, 4.6, ring),
                Shape::Capsule((16.0, 16.0), (16.0, 16.0), 1.3, [90, 96, 110]),
            ];
            (shapes, (16.0, 3.0))
        }
    }
}

/// The side in pixels for a window scale factor.
pub fn size_for(scale: f64) -> u32 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    ((DESIGN as f64 * scale).round() as u32).clamp(MIN_SIZE, MAX_SIZE)
}

/// Renders `art` as a square bitmap of `size` pixels.
pub fn render(art: CursorArt, size: u32) -> CursorBitmap {
    let size = size.clamp(MIN_SIZE, MAX_SIZE);
    let (shapes, hot) = design(art);
    let unit = DESIGN / size as f32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for py in 0..size {
        for px in 0..size {
            let (mut r, mut g, mut b, mut hits) = (0.0f32, 0.0f32, 0.0f32, 0u32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let p = (
                        (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) * unit,
                        (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) * unit,
                    );
                    if let Some(c) = sample(&shapes, p) {
                        r += c[0] as f32;
                        g += c[1] as f32;
                        b += c[2] as f32;
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                let i = ((py * size + px) * 4) as usize;
                let n = hits as f32;
                rgba[i] = (r / n).round() as u8;
                rgba[i + 1] = (g / n).round() as u8;
                rgba[i + 2] = (b / n).round() as u8;
                rgba[i + 3] = (255 * hits / (SAMPLES * SAMPLES)) as u8;
            }
        }
    }
    let at = |v: f32| ((v / unit).round() as u32).min(size - 1);
    CursorBitmap {
        rgba,
        size,
        hotspot: (at(hot.0), at(hot.1)),
    }
}

/// The colour at design point `p`: the front-most fill, else the outline, else clear.
fn sample(shapes: &[Shape], p: Pt) -> Option<[u8; 3]> {
    let mut near = f32::MAX;
    for s in shapes.iter().rev() {
        let d = s.distance(p);
        if d < 0.0 {
            return Some(s.color());
        }
        near = near.min(d);
    }
    (near <= OUTLINE).then_some(OUTLINE_COLOR)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [CursorArt; 3] = [CursorArt::Pen, CursorArt::Eraser, CursorArt::Razor];

    #[test]
    fn a_bitmap_has_its_size_a_hotspot_inside_and_visible_pixels() {
        for art in ALL {
            for size in [32, 48, 64] {
                let b = render(art, size);
                assert_eq!(b.size, size);
                assert_eq!(b.rgba.len(), (size * size * 4) as usize);
                assert!(b.hotspot.0 < size && b.hotspot.1 < size, "{art:?} {size}");
                let opaque = b.rgba.chunks(4).filter(|p| p[3] == 255).count();
                assert!(opaque > (size * size / 20) as usize, "{art:?} {size}");
                assert!(b.rgba.chunks(4).any(|p| p[3] == 0), "{art:?} has air");
            }
        }
    }

    #[test]
    fn the_hotspot_sits_on_the_drawing() {
        // A hotspot on empty pixels would make the cursor point at nothing.
        for art in ALL {
            let b = render(art, 32);
            let (x, y) = b.hotspot;
            let near = (-2i32..=2).any(|dy| {
                (-2i32..=2).any(|dx| {
                    let (px, py) = (x as i32 + dx, y as i32 + dy);
                    (0..32).contains(&px)
                        && (0..32).contains(&py)
                        && b.rgba[((py * 32 + px) * 4 + 3) as usize] > 0
                })
            });
            assert!(near, "{art:?} hotspot {:?}", b.hotspot);
        }
    }

    #[test]
    fn the_hotspot_scales_with_the_size() {
        for art in ALL {
            let (a, b) = (render(art, 32).hotspot, render(art, 64).hotspot);
            assert!(
                b.0.abs_diff(2 * a.0) <= 1 && b.1.abs_diff(2 * a.1) <= 1,
                "{art:?}"
            );
        }
    }

    #[test]
    fn the_size_follows_the_scale_and_stays_bounded() {
        assert_eq!(size_for(1.0), 32);
        assert_eq!(size_for(2.0), 64);
        assert_eq!(size_for(0.5), MIN_SIZE);
        assert_eq!(size_for(10.0), MAX_SIZE);
        assert_eq!(size_for(f64::NAN), 32);
        assert_eq!(size_for(0.0), 32);
    }

    #[test]
    fn every_cursor_differs_and_is_outlined() {
        let bitmaps: Vec<_> = ALL.iter().map(|&a| render(a, 32)).collect();
        assert_ne!(bitmaps[0].rgba, bitmaps[1].rgba);
        assert_ne!(bitmaps[1].rgba, bitmaps[2].rgba);
        for b in &bitmaps {
            assert!(
                b.rgba
                    .chunks(4)
                    .any(|p| p[3] == 255 && p[..3] == OUTLINE_COLOR),
                "a dark outline"
            );
        }
    }
}
