//! The drawn duck curve: a few points with bends, one curve over one beat, bar or note value.
//!
//! The Sidechain Tool's curve mode multiplies the audio by `1 - depth * dip`, where `dip` (0 to 1)
//! is read from this curve a position `x` (0 to 1) along its length, counted from the trigger.
//! The curve is a list of at most [`MAX_POINTS`] [`Point`]s sorted by `x`. Between two points it
//! runs `y0 + (y1 - y0) * shape(u)` with `u` the position inside the segment and `shape` the bend
//! of the segment's first point (the power curve of [`atomik_core::bend`], the same bend an
//! automation segment has: 0 is a straight line, a positive bend eases in, a negative one eases
//! out). Before the first point the curve holds the first value, after the last it holds the
//! last. The evaluation uses only additions, multiplications, divisions and the bend shape,
//! so it gives the same bits on every platform.
//!
//! The device reads a [`Curve`] per sample (fixed size, no allocation), and a retrigger enters it
//! where the curve first reaches the dip that is playing ([`Curve::first_reach`]), so a new hit
//! never lifts the gain.

use atomik_core::bend::{BEND_MAX, bend_shape, clamp_bend};

/// The most points a curve holds.
pub const MAX_POINTS: usize = 8;
/// One point of the curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Position along the curve, 0 to 1.
    pub x: f32,
    /// Dip at the point, 0 (no ducking) to 1 (the full depth).
    pub y: f32,
    /// The bend of the segment that starts here, `-64..=64` (unused on the last point).
    pub bend: i8,
}

impl Point {
    /// A point with every field in range.
    pub fn new(x: f32, y: f32, bend: i8) -> Self {
        let fin = |v: f32| if v.is_finite() { v } else { 0.0 };
        Self {
            x: fin(x).clamp(0.0, 1.0),
            y: fin(y).clamp(0.0, 1.0),
            bend: clamp_bend(bend),
        }
    }
}

/// The curve a device without drawn points uses: a fast fall to full ducking, then a smooth
/// recovery that eases out.
pub const DEFAULT_POINTS: [Point; 3] = [
    Point {
        x: 0.0,
        y: 0.0,
        bend: 0,
    },
    Point {
        x: 0.04,
        y: 1.0,
        bend: -24,
    },
    Point {
        x: 1.0,
        y: 0.0,
        bend: 0,
    },
];

/// The largest bend of a point.
pub const BEND_LIMIT: i8 = BEND_MAX;

/// The key of the device param that holds how many points the curve has (0 means the
/// [`DEFAULT_POINTS`]).
pub const COUNT_KEY: &str = "hidden.curve_n";

/// The keys of the device params that hold point `n` (0-based): `x`, `y` and `bend`. They are
/// hidden params (the panel draws them, no knob shows them), stored in the project with the other
/// params of the device.
pub const POINT_KEYS: [[&str; 3]; MAX_POINTS] = [
    [
        "hidden.curve_p1_x",
        "hidden.curve_p1_y",
        "hidden.curve_p1_bend",
    ],
    [
        "hidden.curve_p2_x",
        "hidden.curve_p2_y",
        "hidden.curve_p2_bend",
    ],
    [
        "hidden.curve_p3_x",
        "hidden.curve_p3_y",
        "hidden.curve_p3_bend",
    ],
    [
        "hidden.curve_p4_x",
        "hidden.curve_p4_y",
        "hidden.curve_p4_bend",
    ],
    [
        "hidden.curve_p5_x",
        "hidden.curve_p5_y",
        "hidden.curve_p5_bend",
    ],
    [
        "hidden.curve_p6_x",
        "hidden.curve_p6_y",
        "hidden.curve_p6_bend",
    ],
    [
        "hidden.curve_p7_x",
        "hidden.curve_p7_y",
        "hidden.curve_p7_bend",
    ],
    [
        "hidden.curve_p8_x",
        "hidden.curve_p8_y",
        "hidden.curve_p8_bend",
    ],
];

/// The points a device holds: `count` of `raw` (`x`, `y`, `bend` as stored), or the
/// [`DEFAULT_POINTS`] for a count of 0.
pub fn points_from_raw(
    count: f32,
    raw: &[[f32; 3]; MAX_POINTS],
    out: &mut [Point; MAX_POINTS],
) -> usize {
    let n = if count.is_finite() {
        count.round().clamp(0.0, MAX_POINTS as f32) as usize
    } else {
        0
    };
    if n == 0 {
        out[..DEFAULT_POINTS.len()].copy_from_slice(&DEFAULT_POINTS);
        return DEFAULT_POINTS.len();
    }
    for (o, r) in out.iter_mut().zip(raw).take(n) {
        *o = Point::new(
            r[0],
            r[1],
            if r[2].is_finite() {
                r[2].round() as i8
            } else {
                0
            },
        );
    }
    n
}

/// The value of the curve through `points` at `x` (module docs). No points read 0. The `x` of the
/// points are taken as non-decreasing: a point to the left of its predecessor counts as at it.
pub fn value_at(points: &[Point], x: f64) -> f32 {
    let Some(first) = points.first() else {
        return 0.0;
    };
    if x.is_nan() || x <= f64::from(first.x) {
        return first.y;
    }
    let mut prev = *first;
    for p in &points[1..] {
        let px = p.x.max(prev.x);
        if x < f64::from(px) {
            let width = f64::from(px) - f64::from(prev.x);
            let u = if width > 0.0 {
                (x - f64::from(prev.x)) / width
            } else {
                1.0
            };
            let shape = bend_shape(u, prev.bend);
            return (f64::from(prev.y) + (f64::from(p.y) - f64::from(prev.y)) * shape) as f32;
        }
        prev = Point { x: px, ..*p };
    }
    prev.y
}

/// A curve: its points, ready to be read per sample (fixed size, nothing allocates).
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    points: [Point; MAX_POINTS],
    len: usize,
}

impl Default for Curve {
    fn default() -> Self {
        Self::new(&DEFAULT_POINTS)
    }
}

impl Curve {
    /// The curve through the first [`MAX_POINTS`] of `points`.
    pub fn new(points: &[Point]) -> Self {
        let mut c = Self {
            points: [Point::new(0.0, 0.0, 0); MAX_POINTS],
            len: 0,
        };
        c.set(points);
        c
    }

    /// Replaces the points (the first [`MAX_POINTS`] of `points`) without allocating.
    pub fn set(&mut self, points: &[Point]) {
        let n = points.len().min(MAX_POINTS);
        self.points[..n].copy_from_slice(&points[..n]);
        self.len = n;
    }

    /// The points of the curve.
    pub fn points(&self) -> &[Point] {
        &self.points[..self.len]
    }

    /// The curve at `pos` (0 to 1; outside it the end values hold), clamped to 0..=1.
    pub fn read(&self, pos: f64) -> f32 {
        value_at(self.points(), pos).clamp(0.0, 1.0)
    }

    /// Where a hit that arrives while the dip `level` is playing enters the curve: the first
    /// position where the curve reaches `level` (found to a millionth of the curve), with
    /// nothing to fade out; a curve that starts above `level` is entered at its start. A curve
    /// that never gets as high as `level` is entered at its highest point, and the second number
    /// says how far under `level` that is, for the caller to fade out.
    pub fn first_reach(&self, level: f32) -> (f64, f32) {
        let level = level.clamp(0.0, 1.0);
        let pts = self.points();
        let Some(first) = pts.first() else {
            return (0.0, 0.0);
        };
        if self.read(0.0) >= level {
            return (0.0, 0.0);
        }
        let mut prev = *first;
        let mut best = (0.0f64, self.read(0.0));
        for p in &pts[1..] {
            let px = p.x.max(prev.x);
            let (x0, x1) = (f64::from(prev.x), f64::from(px));
            if p.y >= level && x1 > x0 {
                // The segment is monotone: bisect for the first position at the level.
                let (mut lo, mut hi) = (x0, x1);
                for _ in 0..24 {
                    let mid = 0.5 * (lo + hi);
                    if self.read(mid) >= level {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                return (hi, 0.0);
            }
            if p.y > best.1 {
                best = (x1, p.y);
            }
            prev = Point { x: px, ..*p };
        }
        // Never high enough: the top of the curve, and the gap to fade.
        (best.0, level - best.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_straight_segment_is_linear_and_holds_its_ends() {
        let p = [Point::new(0.2, 0.0, 0), Point::new(0.6, 1.0, 0)];
        assert_eq!(value_at(&p, 0.0), 0.0);
        assert_eq!(value_at(&p, 0.2), 0.0);
        assert!((value_at(&p, 0.4) - 0.5).abs() < 1e-6);
        assert!((value_at(&p, 0.6) - 1.0).abs() < 1e-6);
        assert_eq!(value_at(&p, 0.9), 1.0);
        assert_eq!(value_at(&[], 0.5), 0.0);
    }

    #[test]
    fn the_bend_eases_in_or_out_like_an_automation_segment() {
        let eased_in = [Point::new(0.0, 0.0, 32), Point::new(1.0, 1.0, 0)];
        let eased_out = [Point::new(0.0, 0.0, -32), Point::new(1.0, 1.0, 0)];
        assert!(value_at(&eased_in, 0.5) < 0.5);
        assert!(value_at(&eased_out, 0.5) > 0.5);
        // The same shape as the automation bend.
        let want = bend_shape(0.5, 32) as f32;
        assert_eq!(value_at(&eased_in, 0.5).to_bits(), want.to_bits());
    }

    #[test]
    fn points_out_of_order_never_break_the_curve() {
        let p = [
            Point::new(0.5, 0.2, 0),
            Point::new(0.3, 0.9, 0),
            Point::new(1.0, 0.0, 0),
        ];
        for i in 0..=100 {
            let v = value_at(&p, f64::from(i) / 100.0);
            assert!((0.0..=1.0).contains(&v), "{v}");
        }
    }

    #[test]
    fn the_curve_reads_its_points_exactly_and_holds_its_ends() {
        let t = Curve::new(&DEFAULT_POINTS);
        assert_eq!(t.read(-1.0), 0.0);
        assert_eq!(t.read(0.0), 0.0);
        assert_eq!(t.read(f64::from(0.04f32)), 1.0);
        assert_eq!(t.read(1.0), 0.0);
        assert_eq!(t.read(5.0), 0.0);
        assert_eq!(
            t.read(0.5).to_bits(),
            value_at(&DEFAULT_POINTS, 0.5).to_bits()
        );
        assert_eq!(Curve::new(&[]).read(0.5), 0.0);
        // More than the points a curve holds are ignored.
        let many = [Point::new(0.5, 1.0, 0); 12];
        assert_eq!(Curve::new(&many).points().len(), MAX_POINTS);
    }

    #[test]
    fn a_retrigger_enters_where_the_curve_reaches_the_dip() {
        let t = Curve::new(&DEFAULT_POINTS);
        // A dip of 0.5 is first reached on the way up, in the fast fall of the curve.
        let (pos, short) = t.first_reach(0.5);
        assert!(pos < 0.04, "{pos}");
        assert_eq!(short, 0.0);
        assert!((t.read(pos) - 0.5).abs() < 1e-5);
        // Nothing to catch up: starts at the beginning.
        assert_eq!(t.first_reach(0.0), (0.0, 0.0));
        // A curve that stays low enters at its top and reports the gap.
        let low = Curve::new(&[Point::new(0.0, 0.0, 0), Point::new(1.0, 0.3, 0)]);
        let (pos, short) = low.first_reach(0.8);
        assert_eq!(pos, 1.0);
        assert!((short - 0.5).abs() < 1e-6);
        // A curve that starts above the dip simply starts.
        let high = Curve::new(&[Point::new(0.0, 1.0, 0), Point::new(1.0, 0.0, 0)]);
        assert_eq!(high.first_reach(0.4), (0.0, 0.0));
        // A level at the very top is found.
        let (pos, _) = t.first_reach(1.0);
        assert!((pos - 0.04).abs() < 1e-5, "{pos}");
    }
}
