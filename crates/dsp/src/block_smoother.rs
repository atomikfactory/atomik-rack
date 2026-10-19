//! Control-rate smoothing: a one-pole smoother that advances once per batch and hands out a
//! linear [`Ramp`] for the samples of that batch (no per-sample exponentials, no zipper noise).

/// Relative distance to the target below which the smoother snaps onto it.
const SNAP: f32 = 5e-5;

/// A linear per-sample ramp over one batch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ramp {
    start: f32,
    inc: f32,
}

impl Ramp {
    /// A constant ramp.
    #[inline]
    pub fn constant(v: f32) -> Self {
        Self { start: v, inc: 0.0 }
    }

    /// Value at sample `n` of the batch (`start + inc * n`, no accumulation error).
    #[inline]
    pub fn at(&self, n: usize) -> f32 {
        self.start + self.inc * n as f32
    }
}

/// One-pole smoother stepped once per batch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockSmoother {
    value: f32,
    coeff: f32,
}

impl BlockSmoother {
    /// A smoother at `value` that jumps straight to its target (coefficient 1).
    pub fn new(value: f32) -> Self {
        Self { value, coeff: 1.0 }
    }

    /// Sets the time constant: the smoother reaches ~63 % of a step after `time_ms`, stepping
    /// once per `block` samples at `sample_rate`. `time_ms <= 0` disables smoothing.
    pub fn set_time(&mut self, time_ms: f32, sample_rate: f32, block: usize) {
        let samples = time_ms * 0.001 * sample_rate;
        self.coeff = if samples > 0.0 {
            1.0 - crate::exp2_det(-(block as f32) / samples * std::f32::consts::LOG2_E)
        } else {
            1.0
        };
    }

    /// Jumps to `value` without a ramp.
    #[inline]
    pub fn set_immediate(&mut self, value: f32) {
        self.value = value;
    }

    /// The current value (the end of the last ramp).
    #[inline]
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Advances one batch of `block` samples towards `target` and returns the ramp from the old
    /// value to the new one.
    #[inline]
    pub fn advance(&mut self, target: f32, block: usize) -> Ramp {
        let start = self.value;
        let mut v = start + (target - start) * self.coeff;
        if !v.is_finite() || (target - v).abs() <= SNAP * target.abs().max(1.0) {
            v = target;
        }
        self.value = v;
        Ramp {
            start,
            inc: (v - start) / block as f32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_smoother_converges_and_snaps() {
        let mut s = BlockSmoother::new(0.0);
        s.set_time(10.0, 48_000.0, 64); // 480 samples
        let r = s.advance(1.0, 64);
        assert_eq!(r.at(0), 0.0);
        assert!(s.value() > 0.1 && s.value() < 0.15, "{}", s.value());
        for _ in 0..2_000 {
            s.advance(1.0, 64);
        }
        assert_eq!(s.value(), 1.0);
        let r = s.advance(1.0, 64);
        assert_eq!(r.at(63).to_bits(), 1.0f32.to_bits());
    }

    #[test]
    fn block_smoother_ramp_is_continuous_across_batches() {
        let mut s = BlockSmoother::new(2.0);
        s.set_time(5.0, 48_000.0, 64);
        let a = s.advance(-1.0, 64);
        let b = s.advance(-1.0, 64);
        assert!((a.at(64) - b.at(0)).abs() < 1e-5);
        let mut m = BlockSmoother::new(3.0);
        let r = m.advance(5.0, 64);
        assert_eq!(r.at(64), 5.0); // coefficient 1 jumps
        assert_eq!(Ramp::constant(0.5).at(7), 0.5);
    }
}
