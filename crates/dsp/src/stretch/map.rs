//! The warp time map: which source frame plays at each output frame.

/// A piecewise-linear, non-decreasing map from output frames to source frames.
///
/// Outside the first and last knot the map continues with the slope of the end segment, so a
/// frame a little before the first knot (the analysis windows reach back) still has a source
/// position. A slope of 1 plays the source at its own speed, 2 plays it twice as fast (the
/// output is shorter than the source), 0.5 stretches it to twice the length.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeMap {
    knots: Vec<(f64, f64)>,
}

impl TimeMap {
    /// A map through `knots` `(output frame, source frame)`: at least two, finite, with the
    /// output frames strictly increasing and the source frames non-decreasing. `None` otherwise.
    pub fn new(knots: &[(f64, f64)]) -> Option<Self> {
        let ok = knots.len() >= 2
            && knots.iter().all(|k| k.0.is_finite() && k.1.is_finite())
            && knots
                .windows(2)
                .all(|w| w[0].0 < w[1].0 && w[0].1 <= w[1].1);
        ok.then(|| Self {
            knots: knots.to_vec(),
        })
    }

    /// The straight map `source = ratio * output` (a constant stretch of `1 / ratio`).
    pub fn linear(ratio: f64) -> Self {
        Self {
            knots: vec![(0.0, 0.0), (1.0, ratio.max(0.0))],
        }
    }

    /// The knots.
    pub fn knots(&self) -> &[(f64, f64)] {
        &self.knots
    }

    /// The source frame (fractional) played at output frame `out`.
    pub fn src_at(&self, out: f64) -> f64 {
        let k = self.knots.partition_point(|p| p.0 <= out);
        let k = k.clamp(1, self.knots.len() - 1);
        let (a, b) = (self.knots[k - 1], self.knots[k]);
        a.1 + (out - a.0) * (b.1 - a.1) / (b.0 - a.0)
    }

    /// The output frame at which source frame `src` plays (the inverse of [`TimeMap::src_at`];
    /// where the map stands still the first such frame).
    pub fn out_at(&self, src: f64) -> f64 {
        let k = self.knots.partition_point(|p| p.1 < src);
        let k = k.clamp(1, self.knots.len() - 1);
        let (a, b) = (self.knots[k - 1], self.knots[k]);
        let ds = b.1 - a.1;
        if ds <= 0.0 {
            return a.0;
        }
        a.0 + (src - a.1) * (b.0 - a.0) / ds
    }

    /// A copy with the output axis scaled by `factor` (`> 0`): the map of a signal that is
    /// `factor` times as long.
    pub fn scaled_out(&self, factor: f64) -> Self {
        Self {
            knots: self.knots.iter().map(|k| (k.0 * factor, k.1)).collect(),
        }
    }

    /// The least and greatest source frame the map reads over output frames `0..out_frames`.
    pub fn src_range(&self, out_frames: f64) -> (f64, f64) {
        (self.src_at(0.0), self.src_at(out_frames))
    }
}
