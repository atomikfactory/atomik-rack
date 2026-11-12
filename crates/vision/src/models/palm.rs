//! The palm detector: its anchors, how its raw output becomes boxes, and how a box becomes the
//! window the landmark model looks at.
//!
//! The model takes the whole frame letterboxed into 192 by 192 and gives, for each of 2016
//! anchors, a box, seven key points (the wrist, the knuckles of the four fingers, the base of
//! the thumb) and a score. The anchors are the fixed grid of the model: 24 by 24 cells with two
//! anchors each, then 12 by 12 cells with six.

use super::image::Roi;
use std::f32::consts::{FRAC_PI_2, PI};

/// The side of the model input.
pub const INPUT: usize = 192;
/// How many anchors the model has.
pub const ANCHORS: usize = 2016;
/// The numbers of one anchor in the box output: 4 for the box, 14 for the seven key points.
pub const COORDS: usize = 18;

/// A detected hand, in the letterboxed input (0 to 1 over the square that holds the frame).
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    /// The score, 0 to 1.
    pub score: f32,
    /// Box centre.
    pub cx: f32,
    /// Box centre.
    pub cy: f32,
    /// Box width.
    pub w: f32,
    /// Box height.
    pub h: f32,
    /// The seven key points.
    pub keypoints: [(f32, f32); 7],
}

/// The centres of the anchors, in the order of the model output.
pub fn anchors() -> Vec<(f32, f32)> {
    let strides = [8usize, 16, 16, 16];
    let mut out = Vec::with_capacity(ANCHORS);
    let mut layer = 0;
    while layer < strides.len() {
        // Layers of the same stride share one grid; each adds two anchors per cell (the
        // scale of the layer and the in-between scale).
        let mut per_cell = 0;
        let mut last = layer;
        while last < strides.len() && strides[last] == strides[layer] {
            per_cell += 2;
            last += 1;
        }
        let cells = INPUT.div_ceil(strides[layer]);
        for y in 0..cells {
            for x in 0..cells {
                for _ in 0..per_cell {
                    out.push((
                        (x as f32 + 0.5) / cells as f32,
                        (y as f32 + 0.5) / cells as f32,
                    ));
                }
            }
        }
        layer = last;
    }
    out
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x.clamp(-100.0, 100.0)).exp())
}

/// The boxes above `threshold` in the raw output, best first, before any overlap is removed.
pub fn decode(
    raw_boxes: &[f32],
    raw_scores: &[f32],
    anchors: &[(f32, f32)],
    threshold: f32,
) -> Vec<Detection> {
    let scale = INPUT as f32;
    let mut out = Vec::new();
    for (i, &(ax, ay)) in anchors.iter().enumerate() {
        let score = sigmoid(raw_scores[i]);
        if score < threshold {
            continue;
        }
        let r = &raw_boxes[i * COORDS..(i + 1) * COORDS];
        let mut keypoints = [(0.0, 0.0); 7];
        for (k, kp) in keypoints.iter_mut().enumerate() {
            *kp = (r[4 + 2 * k] / scale + ax, r[5 + 2 * k] / scale + ay);
        }
        out.push(Detection {
            score,
            cx: r[0] / scale + ax,
            cy: r[1] / scale + ay,
            w: r[2] / scale,
            h: r[3] / scale,
            keypoints,
        });
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

fn iou(a: &Detection, b: &Detection) -> f32 {
    let (ax0, ay0, ax1, ay1) = (
        a.cx - a.w / 2.0,
        a.cy - a.h / 2.0,
        a.cx + a.w / 2.0,
        a.cy + a.h / 2.0,
    );
    let (bx0, by0, bx1, by1) = (
        b.cx - b.w / 2.0,
        b.cy - b.h / 2.0,
        b.cx + b.w / 2.0,
        b.cy + b.h / 2.0,
    );
    let iw = (ax1.min(bx1) - ax0.max(bx0)).max(0.0);
    let ih = (ay1.min(by1) - ay0.max(by0)).max(0.0);
    let inter = iw * ih;
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// Weighted non-maximum suppression: the boxes that overlap the best one by more than
/// `overlap` are merged into it, weighted by their scores.
pub fn suppress(sorted: Vec<Detection>, overlap: f32) -> Vec<Detection> {
    let mut rest = sorted;
    let mut kept = Vec::new();
    while !rest.is_empty() {
        let best = rest.remove(0);
        let mut group = vec![best.clone()];
        rest.retain(|d| {
            if iou(&best, d) > overlap {
                group.push(d.clone());
                false
            } else {
                true
            }
        });
        let total: f32 = group.iter().map(|d| d.score).sum();
        let mut merged = best.clone();
        merged.cx = group.iter().map(|d| d.cx * d.score).sum::<f32>() / total;
        merged.cy = group.iter().map(|d| d.cy * d.score).sum::<f32>() / total;
        merged.w = group.iter().map(|d| d.w * d.score).sum::<f32>() / total;
        merged.h = group.iter().map(|d| d.h * d.score).sum::<f32>() / total;
        for k in 0..7 {
            merged.keypoints[k].0 = group
                .iter()
                .map(|d| d.keypoints[k].0 * d.score)
                .sum::<f32>()
                / total;
            merged.keypoints[k].1 = group
                .iter()
                .map(|d| d.keypoints[k].1 * d.score)
                .sum::<f32>()
                / total;
        }
        kept.push(merged);
    }
    kept
}

/// Brings an angle into -pi to pi.
pub fn normalise_radians(a: f32) -> f32 {
    let two = 2.0 * PI;
    let mut a = a - two * ((a + PI) / two).floor();
    if a >= PI {
        a -= two;
    }
    a
}

/// Scales a window, shifts it along the hand and makes it a square: how a hand is framed for the
/// landmark model. `width` and `height` are the window turned by `rotation` (in pixels).
pub fn frame_hand(
    cx: f32,
    cy: f32,
    width: f32,
    height: f32,
    rotation: f32,
    scale: f32,
    shift_y: f32,
) -> Roi {
    let (s, c) = rotation.sin_cos();
    let (cx, cy) = (cx - height * shift_y * s, cy + height * shift_y * c);
    Roi {
        cx,
        cy,
        size: width.max(height) * scale,
        rotation,
    }
}

/// The window for a detection. `square` is the side in pixels of the square the frame was
/// letterboxed into, and `(frame_w, frame_h)` the frame, so the detection (0 to 1 over the square)
/// becomes pixels of the frame.
pub fn roi_of(d: &Detection, square: f32, frame_w: f32, frame_h: f32) -> Roi {
    let to_px = |x: f32, y: f32| {
        (
            (x - 0.5) * square + frame_w / 2.0,
            (y - 0.5) * square + frame_h / 2.0,
        )
    };
    let (cx, cy) = to_px(d.cx, d.cy);
    let (x0, y0) = to_px(d.keypoints[0].0, d.keypoints[0].1);
    let (x1, y1) = to_px(d.keypoints[2].0, d.keypoints[2].1);
    // The hand is upright when the wrist is straight below the middle knuckle.
    let rotation = normalise_radians(FRAC_PI_2 - (-(y1 - y0)).atan2(x1 - x0));
    frame_hand(cx, cy, d.w * square, d.h * square, rotation, 2.6, -0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_has_2016_anchors_in_two_layers() {
        let a = anchors();
        assert_eq!(a.len(), ANCHORS);
        // 24 by 24 cells with two each, then 12 by 12 cells with six each.
        assert_eq!(24 * 24 * 2 + 12 * 12 * 6, ANCHORS);
        assert_eq!(a[0], (0.5 / 24.0, 0.5 / 24.0));
        assert_eq!(a[1], a[0]);
        assert_eq!(a[2], (1.5 / 24.0, 0.5 / 24.0));
        assert_eq!(a[24 * 24 * 2], (0.5 / 12.0, 0.5 / 12.0));
        assert_eq!(a[24 * 24 * 2 + 6], (1.5 / 12.0, 0.5 / 12.0));
    }

    fn fake(i: usize, score_logit: f32, boxes: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut raw = vec![0.0; ANCHORS * COORDS];
        raw[i * COORDS..(i + 1) * COORDS].copy_from_slice(boxes);
        let mut scores = vec![-10.0; ANCHORS];
        scores[i] = score_logit;
        (raw, scores)
    }

    #[test]
    fn a_raw_box_is_placed_on_its_anchor() {
        let a = anchors();
        let mut b = vec![0.0; COORDS];
        b[0] = 19.2; // +0.1 in x
        b[2] = 38.4; // width 0.2
        b[3] = 38.4;
        b[4] = -19.2; // the wrist key point 0.1 to the left of the anchor
        let (raw, scores) = fake(2000, 3.0, &b);
        let found = decode(&raw, &scores, &a, 0.5);
        assert_eq!(found.len(), 1);
        let d = &found[0];
        assert!((d.cx - (a[2000].0 + 0.1)).abs() < 1e-5);
        assert!((d.w - 0.2).abs() < 1e-5 && (d.h - 0.2).abs() < 1e-5);
        assert!((d.keypoints[0].0 - (a[2000].0 - 0.1)).abs() < 1e-5);
        assert!(d.score > 0.95);
    }

    #[test]
    fn a_low_score_is_dropped() {
        let a = anchors();
        let (raw, scores) = fake(5, -2.0, &[0.0; COORDS]);
        assert!(decode(&raw, &scores, &a, 0.5).is_empty());
    }

    #[test]
    fn overlapping_boxes_merge_and_distant_ones_stay() {
        let base = Detection {
            score: 0.9,
            cx: 0.3,
            cy: 0.3,
            w: 0.2,
            h: 0.2,
            keypoints: [(0.3, 0.3); 7],
        };
        let near = Detection {
            score: 0.6,
            cx: 0.32,
            ..base.clone()
        };
        let far = Detection {
            score: 0.7,
            cx: 0.8,
            ..base.clone()
        };
        let kept = suppress(vec![base.clone(), far.clone(), near], 0.3);
        assert_eq!(kept.len(), 2);
        assert!(
            kept[0].cx > 0.3 && kept[0].cx < 0.32,
            "weighted towards the best"
        );
        assert!((kept[1].cx - 0.8).abs() < 1e-6);
    }

    #[test]
    fn an_upright_hand_is_not_turned_and_a_hand_pointing_right_is_a_quarter_turn() {
        let mut d = Detection {
            score: 0.9,
            cx: 0.5,
            cy: 0.5,
            w: 0.2,
            h: 0.2,
            keypoints: [(0.5, 0.5); 7],
        };
        d.keypoints[0] = (0.5, 0.6); // the wrist below
        d.keypoints[2] = (0.5, 0.45); // the middle knuckle above
        let roi = roi_of(&d, 100.0, 100.0, 100.0);
        assert!(roi.rotation.abs() < 1e-5, "{}", roi.rotation);
        // 0.2 * 100 = 20 px, scaled by 2.6 = 52, centre moved up by half the height.
        assert!((roi.size - 52.0).abs() < 1e-3);
        assert!((roi.cy - 40.0).abs() < 1e-3, "{}", roi.cy);

        d.keypoints[0] = (0.4, 0.5);
        d.keypoints[2] = (0.55, 0.5); // fingers pointing right
        let roi = roi_of(&d, 100.0, 100.0, 100.0);
        assert!((roi.rotation - FRAC_PI_2).abs() < 1e-4, "{}", roi.rotation);
        // Shifted to the right (towards the fingers).
        assert!(roi.cx > 50.0);
    }

    #[test]
    fn radians_wrap() {
        assert!((normalise_radians(3.0 * PI / 2.0) + PI / 2.0).abs() < 1e-5);
        assert!((normalise_radians(-3.0 * PI / 2.0) - PI / 2.0).abs() < 1e-5);
        assert!(normalise_radians(0.3) - 0.3 < 1e-6);
    }
}
