//! The curve editor on the Sidechain Tool's panel: the drawn duck curve, its points and bends,
//! and the LEARN button.
//!
//! The plot shows the **gain** over one curve length from the trigger (the left edge): the top is
//! no ducking, the bottom the full depth, so the curve dips where the audio is turned down. The
//! device stores the curve as up to eight points ([`atomik_dsp::duck_curve`]) in its hidden
//! params; a device that has none draws the built-in curve, and the first edit writes the points
//! out. Gestures (all pure maths here, the rack view turns them into commands):
//!
//! * press on a point and drag: moves it (the first and last point only up and down; the others
//!   stay between their neighbours),
//! * press on the line between two points and drag: bends that segment, so the line follows the
//!   pointer ([`bend_for`]),
//! * press elsewhere on the plot: adds a point there (up to eight),
//! * double-click on a point that is not the first or last: removes it.

use crate::canvas::{Align, Canvas, CanvasExt, pt};
use crate::fx_paint::{lcd, small};
use crate::paint::PanelInfo;
use crate::theme::{Theme, hsl};
use crate::widget::current_value;
use atomik_core::bend::{BEND_MAX, bend_shape};
use atomik_device_sdk::DeviceDescriptor;
use atomik_dsp::duck_curve::{COUNT_KEY, MAX_POINTS, POINT_KEYS, Point, points_from_raw, value_at};
use atomik_model::RackDevice;
use vello::kurbo::{BezPath, Circle, Line, Point as Pt, Rect};
use vello::peniko::Color;

/// Width of the LEARN button, px.
pub const LEARN_W: f64 = 54.0;
/// Height of the LEARN button, px.
pub const LEARN_H: f64 = 16.0;
/// Distance within which a press picks a point, px.
pub const PICK_R: f64 = 9.0;
/// Distance from the line within which a press picks its segment, px.
pub const LINE_R: f64 = 6.0;
/// The closest two points may come in `x`.
const MIN_GAP: f32 = 0.004;
/// The values of the `length` param, for the label.
const LENGTHS: [&str; 5] = ["1/16", "1/8", "1/4", "1/2", "1 BAR"];
/// The value of the `mode` param that plays the drawn curve.
pub const CURVE_MODE: f32 = 3.0;

/// The LEARN button in the top right corner of the plot `plot` (panel coordinates).
pub fn learn_button(plot: Rect) -> Rect {
    let top = plot.y0 + 5.0;
    Rect::new(plot.x1 - 6.0 - LEARN_W, top, plot.x1 - 6.0, top + LEARN_H)
}

/// The part of the plot the curve is drawn in.
pub fn inner(plot: Rect) -> Rect {
    Rect::new(
        plot.x0 + 10.0,
        plot.y0 + 26.0,
        plot.x1 - 10.0,
        plot.y1 - 12.0,
    )
}

/// The pixel of curve position `x` (0 to 1) and dip `y` (0 to 1: the top is no ducking).
pub fn to_px(inner: Rect, x: f64, y: f64) -> Pt {
    pt(inner.x0 + x * inner.width(), inner.y0 + y * inner.height())
}

/// The curve position and dip under the pixel `p`, each clamped to 0..=1.
pub fn from_px(inner: Rect, p: Pt) -> (f32, f32) {
    let x = ((p.x - inner.x0) / inner.width().max(1.0)).clamp(0.0, 1.0);
    let y = ((p.y - inner.y0) / inner.height().max(1.0)).clamp(0.0, 1.0);
    (x as f32, y as f32)
}

/// The curve of `device`: its points, and whether it has stored points (`false` for the built-in
/// curve of a device that has none).
pub fn points_of(device: Option<&RackDevice>, desc: &DeviceDescriptor) -> (Vec<Point>, bool) {
    let value = |key: &str| {
        desc.params
            .iter()
            .find(|p| p.key == key)
            .map_or(0.0, |p| device.map_or(p.default, |d| current_value(d, p)))
    };
    let count = value(COUNT_KEY);
    let mut raw = [[0.0f32; 3]; MAX_POINTS];
    for (r, keys) in raw.iter_mut().zip(&POINT_KEYS) {
        for (v, k) in r.iter_mut().zip(keys) {
            *v = value(k);
        }
    }
    let mut points = [Point::new(0.0, 0.0, 0); MAX_POINTS];
    let n = points_from_raw(count, &raw, &mut points);
    (points[..n].to_vec(), count.round() >= 1.0)
}

/// The point under `p` (within [`PICK_R`]; the nearest when several are), if any.
pub fn pick_point(inner: Rect, points: &[Point], p: Pt) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .map(|(i, q)| (i, to_px(inner, f64::from(q.x), f64::from(q.y)).distance(p)))
        .filter(|(_, d)| *d <= PICK_R)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The segment whose line passes within [`LINE_R`] of `p` (segment `i` runs from point `i` to
/// point `i + 1`), if any.
pub fn pick_segment(inner: Rect, points: &[Point], p: Pt) -> Option<usize> {
    let (x, _) = from_px(inner, p);
    let i = points
        .windows(2)
        .position(|w| x >= w[0].x && x < w[1].x.max(w[0].x + f32::EPSILON))?;
    let line = to_px(
        inner,
        f64::from(x),
        f64::from(value_at(points, f64::from(x))),
    );
    ((line.y - p.y).abs() <= LINE_R).then_some(i)
}

/// The points with point `i` at (`x`, `y`): the first and last stay at the edges, the others stay
/// between their neighbours.
pub fn moved(points: &[Point], i: usize, x: f32, y: f32) -> Vec<Point> {
    let mut out = points.to_vec();
    let Some(p) = out.get(i).copied() else {
        return out;
    };
    let x = if i == 0 || i + 1 == points.len() {
        p.x
    } else {
        x.clamp(points[i - 1].x + MIN_GAP, points[i + 1].x - MIN_GAP)
    };
    out[i] = Point::new(x, y, p.bend);
    out
}

/// The points with a new one at (`x`, `y`) (no bend), or `None` when the curve is full or the
/// place is taken (another point within [`MIN_GAP`]).
pub fn inserted(points: &[Point], x: f32, y: f32) -> Option<Vec<Point>> {
    if points.len() >= MAX_POINTS || points.iter().any(|p| (p.x - x).abs() < MIN_GAP) {
        return None;
    }
    let at = points.iter().position(|p| p.x > x).unwrap_or(points.len());
    let mut out = points.to_vec();
    out.insert(at, Point::new(x, y, 0));
    Some(out)
}

/// The points without point `i`, or `None` for the first and the last (the curve keeps its
/// edges) and for a curve of two points.
pub fn removed(points: &[Point], i: usize) -> Option<Vec<Point>> {
    if points.len() <= 2 || i == 0 || i + 1 >= points.len() {
        return None;
    }
    let mut out = points.to_vec();
    out.remove(i);
    Some(out)
}

/// The points with the bend of segment `seg` set to `bend`.
pub fn bent(points: &[Point], seg: usize, bend: i8) -> Vec<Point> {
    let mut out = points.to_vec();
    if let Some(p) = out.get_mut(seg) {
        *p = Point::new(p.x, p.y, bend);
    }
    out
}

/// The bend of segment `seg` that puts the line through the dip `target` at the position `x`
/// (the pointer drags the line): the bend whose shape comes nearest to it, 0 for a flat segment.
pub fn bend_for(points: &[Point], seg: usize, x: f32, target: f32) -> i8 {
    let (Some(a), Some(b)) = (points.get(seg), points.get(seg + 1)) else {
        return 0;
    };
    let rise = f64::from(b.y) - f64::from(a.y);
    let width = f64::from(b.x) - f64::from(a.x);
    if rise.abs() < 1e-6 || width <= 0.0 {
        return a.bend;
    }
    let u = ((f64::from(x) - f64::from(a.x)) / width).clamp(0.0, 1.0);
    let want = ((f64::from(target) - f64::from(a.y)) / rise).clamp(0.0, 1.0);
    (-BEND_MAX..=BEND_MAX)
        .min_by(|p, q| {
            let (dp, dq) = (
                (bend_shape(u, *p) - want).abs(),
                (bend_shape(u, *q) - want).abs(),
            );
            dp.total_cmp(&dq)
        })
        .unwrap_or(0)
}

/// The params that must change to turn the curve `old` into `new`: `(key, value)` for the count
/// and each point field that differs. A device that has no stored points (`stored` false) gets
/// every field written, so the edit starts from the curve that was on screen.
pub fn changes(old: &[Point], stored: bool, new: &[Point]) -> Vec<(&'static str, f32)> {
    let mut out = Vec::new();
    if !stored || old.len() != new.len() {
        out.push((COUNT_KEY, new.len() as f32));
    }
    for (i, p) in new.iter().enumerate().take(MAX_POINTS) {
        let before = old.get(i).filter(|_| stored);
        let fields = [p.x, p.y, f32::from(p.bend)];
        let was = before.map(|q| [q.x, q.y, f32::from(q.bend)]);
        for (k, v) in fields.iter().enumerate() {
            if was.is_none_or(|w| w[k].to_bits() != v.to_bits()) {
                out.push((POINT_KEYS[i][k], *v));
            }
        }
    }
    out
}

fn color_of(info: &PanelInfo<'_>, live: bool) -> Color {
    if live {
        info.accent
    } else {
        info.accent.with_alpha(0.4)
    }
}

/// Paints the plot: the curve, its points, the position of the running curve and the LEARN
/// button.
pub(crate) fn paint<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    plot: Rect,
    info: &PanelInfo<'_>,
) {
    lcd(c, theme, plot);
    let Some(desc) = info.desc else {
        return;
    };
    let value = |key: &str| {
        desc.params.iter().find(|p| p.key == key).map_or(0.0, |p| {
            info.device.map_or(p.default, |d| current_value(d, p))
        })
    };
    let live = (value("mode") - CURVE_MODE).abs() < 0.5;
    let (points, stored) = points_of(info.device, desc);
    let area = inner(plot);
    let faint = theme.ink.with_alpha(0.1);
    for q in [0.25, 0.5, 0.75] {
        let x = to_px(area, q, 0.0).x.round() + 0.5;
        c.stroke_with(Line::new(pt(x, area.y0), pt(x, area.y1)), 1.0, faint);
    }
    let mid = to_px(area, 0.0, 0.5).y.round() + 0.5;
    c.stroke_with(Line::new(pt(area.x0, mid), pt(area.x1, mid)), 1.0, faint);
    c.stroke_with(
        Line::new(pt(area.x0, area.y0), pt(area.x1, area.y0)),
        1.0,
        theme.ink.with_alpha(0.22),
    );
    // The curve and the ducked area under the top line.
    let color = color_of(info, live);
    let steps = 120;
    let mut line = BezPath::new();
    for k in 0..=steps {
        let x = f64::from(k) / f64::from(steps);
        let p = to_px(area, x, f64::from(value_at(&points, x)));
        if k == 0 {
            line.move_to(p);
        } else {
            line.line_to(p);
        }
    }
    let mut fill = line.clone();
    fill.line_to(pt(area.x1, area.y0));
    fill.line_to(pt(area.x0, area.y0));
    fill.close_path();
    c.push_clip(&plot.into());
    c.fill_with(fill, color.with_alpha(if live { 0.16 } else { 0.07 }));
    c.stroke_with(line, 1.75, color);
    for p in &points {
        let at = to_px(area, f64::from(p.x), f64::from(p.y));
        let dot = Circle::new(at, 3.5);
        if stored {
            c.fill_with(dot, color);
        } else {
            c.stroke_with(dot, 1.25, color);
        }
    }
    // Where the running curve is.
    let pos = info.fx_meters.get(1).copied().unwrap_or(1.0);
    if live && pos < 0.995 {
        let x = to_px(area, f64::from(pos), 0.0).x.round() + 0.5;
        c.stroke_with(
            Line::new(pt(x, area.y0), pt(x, area.y1)),
            1.0,
            info.accent.with_alpha(0.7),
        );
        let at = to_px(
            area,
            f64::from(pos),
            f64::from(value_at(&points, f64::from(pos))),
        );
        c.fill_with(Circle::new(at, 3.0), Color::WHITE.with_alpha(0.9));
    }
    c.pop_clip();
    let length = LENGTHS[(value("length").round().clamp(0.0, 4.0)) as usize];
    small(
        c,
        theme,
        if live {
            format!("GAIN OVER {length}")
        } else {
            format!("CURVE MODE PLAYS THIS ({length})")
        },
        pt(plot.x0 + 8.0, plot.y0 + 12.0),
        Align::Start,
        Some(if live {
            info.accent
        } else {
            hsl(28.0, 0.5, 0.6)
        }),
    );
    small(
        c,
        theme,
        "NO DUCK",
        pt(area.x0 + 2.0, area.y0 - 2.0),
        Align::Start,
        None,
    );
    crate::paint::button_sized(c, theme, learn_button(plot), "LEARN", true, 8.5, 0.8);
}

/// The tooltip of a Sidechain Tool control: what the knob or button does, one short line. `None`
/// for a key the device does not have (and for `bypass`, which has the effect strip's own).
pub fn duck_help(key: &str) -> Option<&'static str> {
    Some(match key {
        "mode" => {
            "What starts the ducking: the song position, a hit on the key, its level, or a drawn curve"
        }
        "rate" => "Pump: how often the dip restarts, in note values of the song",
        "length" => "Curve: how long the drawn curve lasts, a note value or one bar of the song",
        "trigger" => "Curve: start it from hits on the key, from notes, or from either",
        "depth" => "How far down the dip pulls the audio, 100 percent is silence",
        "attack" => "How fast the dip falls once it starts",
        "time" => "How long the audio takes to come back",
        "curve" => "Shape of the way back: above 1 it hangs low, below 1 it returns at once",
        "threshold" => "Key level that starts the dip",
        "send" => "Leave the audio alone and only move the duck CV output",
        "band" => "Duck only the low band: the sound above the split passes with its level",
        "band_freq" => "The split of the low band: the sound under it is ducked",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_dsp::duck_curve::DEFAULT_POINTS;

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 400.0, 100.0)
    }

    #[test]
    fn pixels_and_positions_map_both_ways() {
        let a = area();
        let p = to_px(a, 0.25, 0.5);
        assert_eq!((p.x, p.y), (100.0, 50.0));
        assert_eq!(from_px(a, p), (0.25, 0.5));
        assert_eq!(from_px(a, pt(-10.0, 500.0)), (0.0, 1.0));
    }

    #[test]
    fn a_point_is_picked_by_position_and_a_segment_by_its_line() {
        let a = area();
        let pts = DEFAULT_POINTS.to_vec();
        let top = to_px(a, 0.04, 1.0);
        assert_eq!(pick_point(a, &pts, pt(top.x + 3.0, top.y - 2.0)), Some(1));
        assert_eq!(pick_point(a, &pts, pt(200.0, 10.0)), None);
        // On the line of the way back, and well off it.
        let x = 0.5;
        let y = value_at(&pts, x);
        let on = to_px(a, f64::from(x as f32), f64::from(y));
        assert_eq!(pick_segment(a, &pts, pt(on.x, on.y + 2.0)), Some(1));
        assert_eq!(pick_segment(a, &pts, pt(on.x, on.y + 30.0)), None);
    }

    #[test]
    fn moving_keeps_the_edges_and_the_order() {
        let pts = DEFAULT_POINTS.to_vec();
        // The first and last point only move up and down.
        let m = moved(&pts, 0, 0.5, 0.3);
        assert_eq!((m[0].x, m[0].y), (0.0, 0.3));
        let m = moved(&pts, 2, 0.1, 0.4);
        assert_eq!((m[2].x, m[2].y), (1.0, 0.4));
        // An inner point stays between its neighbours.
        let m = moved(&pts, 1, 5.0, 0.9);
        assert!(m[1].x < 1.0 && m[1].x > 0.9, "{}", m[1].x);
        let m = moved(&pts, 1, -5.0, 0.9);
        assert!(m[1].x > 0.0 && m[1].x < 0.01);
        assert_eq!(m[1].bend, pts[1].bend, "a move keeps the bend");
    }

    #[test]
    fn points_are_added_in_order_up_to_the_most_and_removed_inside() {
        let mut pts = DEFAULT_POINTS.to_vec();
        pts = inserted(&pts, 0.5, 0.3).unwrap();
        assert_eq!(pts.len(), 4);
        assert!(pts.windows(2).all(|w| w[0].x < w[1].x));
        assert!(inserted(&pts, 0.5005, 0.1).is_none(), "the place is taken");
        while pts.len() < MAX_POINTS {
            let x = 0.52 + (pts.len() - 4) as f32 * 0.06;
            pts = inserted(&pts, x, 0.2).unwrap();
        }
        assert!(inserted(&pts, 0.97, 0.2).is_none(), "full");
        assert!(removed(&pts, 0).is_none());
        assert!(removed(&pts, pts.len() - 1).is_none());
        assert_eq!(removed(&pts, 2).unwrap().len(), MAX_POINTS - 1);
        assert!(removed(&DEFAULT_POINTS[..2], 1).is_none());
    }

    #[test]
    fn dragging_a_line_finds_the_bend_that_passes_through_the_pointer() {
        let pts = vec![Point::new(0.0, 0.0, 0), Point::new(1.0, 1.0, 0)];
        // Pull the middle up to 0.7: an ease-out (negative bend) gets there.
        let b = bend_for(&pts, 0, 0.5, 0.7);
        assert!(b < 0, "{b}");
        let got = value_at(&bent(&pts, 0, b), 0.5);
        assert!((got - 0.7).abs() < 0.02, "{got}");
        // Down to 0.2: an ease-in.
        assert!(bend_for(&pts, 0, 0.5, 0.2) > 0);
        // The middle on the straight line: no bend.
        assert_eq!(bend_for(&pts, 0, 0.5, 0.5), 0);
        // A flat segment keeps its bend.
        let flat = vec![Point::new(0.0, 0.4, 12), Point::new(1.0, 0.4, 0)];
        assert_eq!(bend_for(&flat, 0, 0.5, 0.9), 12);
    }

    #[test]
    fn changes_write_only_what_moved_and_everything_for_a_default_curve() {
        let old = DEFAULT_POINTS.to_vec();
        let new = moved(&old, 1, 0.1, 0.8);
        let c = changes(&old, true, &new);
        assert_eq!(c, vec![(POINT_KEYS[1][0], 0.1), (POINT_KEYS[1][1], 0.8)]);
        // A device with no stored points gets the count and all nine fields.
        let c = changes(&old, false, &new);
        assert_eq!(c.len(), 1 + 9);
        assert_eq!(c[0], (COUNT_KEY, 3.0));
        // Inserting changes the count and the shifted points.
        let more = inserted(&old, 0.5, 0.3).unwrap();
        let c = changes(&old, true, &more);
        assert!(c.contains(&(COUNT_KEY, 4.0)));
        assert!(c.iter().any(|(k, _)| *k == POINT_KEYS[3][0]));
        assert!(changes(&old, true, &old).is_empty());
    }

    #[test]
    fn every_control_of_the_device_has_a_tooltip() {
        for key in [
            "mode",
            "rate",
            "length",
            "trigger",
            "depth",
            "attack",
            "time",
            "curve",
            "threshold",
            "send",
            "band",
            "band_freq",
        ] {
            let text = duck_help(key).unwrap();
            assert!(text.len() <= 100, "{key}: {} chars", text.len());
        }
        assert!(duck_help("bypass").is_none());
    }
}
