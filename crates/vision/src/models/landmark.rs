//! The hand landmark model's side: the window of the next frame from this frame's landmarks, and
//! the projection of the model's points back into the frame.

use super::image::Roi;
use super::palm::{frame_hand, normalise_radians};
use crate::hand::{LANDMARKS, Point3, joint};
use std::f32::consts::FRAC_PI_2;

/// The side of the model input.
pub const INPUT: usize = 224;

/// What the model gave for one crop.
#[derive(Debug, Clone, PartialEq)]
pub struct RawLandmarks {
    /// 21 points as `x, y, z` in pixels of the 224 by 224 crop.
    pub points: Vec<f32>,
    /// The chance that a hand is in the crop, 0 to 1.
    pub presence: f32,
    /// The chance that it is a right hand, 0 to 1.
    pub right: f32,
}

/// The model's points as landmarks of the frame (normalised by the frame size).
pub fn project(raw: &[f32], roi: &Roi, frame_w: f32, frame_h: f32) -> [Point3; LANDMARKS] {
    let mut out = [Point3::default(); LANDMARKS];
    let n = INPUT as f32;
    for (i, p) in out.iter_mut().enumerate() {
        let (u, v) = (raw[i * 3] / n, raw[i * 3 + 1] / n);
        let (x, y) = roi.to_frame(u, v);
        // The depth is in the scale of the crop; give it in the scale of the frame width.
        *p = Point3::new(
            x / frame_w,
            y / frame_h,
            raw[i * 3 + 2] / n * roi.size / frame_w,
        );
    }
    out
}

/// The window for the next frame, from the landmarks found in this one: turned so the hand is
/// upright, the box that holds every landmark, enlarged.
pub fn next_roi(landmarks: &[Point3; LANDMARKS], frame_w: f32, frame_h: f32) -> Roi {
    let px = |i: usize| (landmarks[i].x * frame_w, landmarks[i].y * frame_h);
    let (x0, y0) = px(joint::WRIST);
    let (a, b, c) = (
        px(joint::INDEX_MCP),
        px(joint::MIDDLE_MCP),
        px(joint::RING_MCP),
    );
    let (x1, y1) = ((a.0 + b.0 + c.0) / 3.0, (a.1 + b.1 + c.1) / 3.0);
    let rotation = normalise_radians(FRAC_PI_2 - (-(y1 - y0)).atan2(x1 - x0));

    // The box that holds the landmarks in the frame of the hand (turned back by the rotation).
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for i in 0..LANDMARKS {
        let (x, y) = px(i);
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    let (mid_x, mid_y) = ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
    let (s, co) = rotation.sin_cos();
    let (mut hx0, mut hy0, mut hx1, mut hy1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for i in 0..LANDMARKS {
        let (x, y) = px(i);
        let (dx, dy) = (x - mid_x, y - mid_y);
        let (hx, hy) = (dx * co + dy * s, -dx * s + dy * co);
        hx0 = hx0.min(hx);
        hx1 = hx1.max(hx);
        hy0 = hy0.min(hy);
        hy1 = hy1.max(hy);
    }
    let (hcx, hcy) = ((hx0 + hx1) / 2.0, (hy0 + hy1) / 2.0);
    let cx = mid_x + hcx * co - hcy * s;
    let cy = mid_y + hcx * s + hcy * co;
    frame_hand(cx, cy, hx1 - hx0, hy1 - hy0, rotation, 2.0, -0.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand_upright() -> [Point3; LANDMARKS] {
        let mut l = [Point3::new(0.5, 0.5, 0.0); LANDMARKS];
        l[joint::WRIST] = Point3::new(0.5, 0.7, 0.0);
        l[joint::INDEX_MCP] = Point3::new(0.45, 0.55, 0.0);
        l[joint::MIDDLE_MCP] = Point3::new(0.5, 0.52, 0.0);
        l[joint::RING_MCP] = Point3::new(0.55, 0.55, 0.0);
        l[joint::INDEX_TIP] = Point3::new(0.44, 0.3, 0.0);
        l[12] = Point3::new(0.5, 0.25, 0.0);
        l
    }

    #[test]
    fn an_upright_hand_gives_an_upright_window_around_it() {
        let roi = next_roi(&hand_upright(), 100.0, 100.0);
        assert!(roi.rotation.abs() < 0.12, "{}", roi.rotation);
        assert!((roi.cx - 50.0).abs() < 3.0, "{}", roi.cx);
        // Taller than the 45 pixels of the hand: about twice, and a square.
        assert!(roi.size > 80.0 && roi.size < 110.0, "{}", roi.size);
    }

    #[test]
    fn the_model_points_come_back_to_the_frame() {
        // A window turned by a quarter: the centre of the crop is the centre of the window and
        // the top middle of the crop is to the right of it.
        let roi = Roi {
            cx: 60.0,
            cy: 40.0,
            size: 50.0,
            rotation: FRAC_PI_2,
        };
        let mut raw = vec![0.0; 63];
        raw[0] = 112.0;
        raw[1] = 112.0;
        raw[3] = 112.0;
        raw[4] = 0.0;
        let p = project(&raw, &roi, 200.0, 100.0);
        assert!((p[0].x * 200.0 - 60.0).abs() < 1e-3 && (p[0].y * 100.0 - 40.0).abs() < 1e-3);
        assert!((p[1].x * 200.0 - 85.0).abs() < 1e-3 && (p[1].y * 100.0 - 40.0).abs() < 1e-3);
    }
}
