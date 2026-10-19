//! Parameter smoothing: a linear ramp over a fixed time and a one-pole (exponential) smoother.

/// Linear ramp: after [`Smoother::set_target`], the value moves in a straight line and reaches
/// the target exactly after the configured ramp time. Retargeting mid-ramp starts a new ramp
/// from the current value (continuous).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Smoother {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
    ramp_samples: u32,
}

impl Smoother {
    /// A smoother at `value` whose ramps last `time_ms` at `sample_rate` (at least 1 sample).
    pub fn new(value: f32, time_ms: f32, sample_rate: f32) -> Self {
        let mut s = Self {
            current: value,
            target: value,
            step: 0.0,
            remaining: 0,
            ramp_samples: 1,
        };
        s.set_time(time_ms, sample_rate);
        s
    }

    /// Changes the ramp time for future ramps.
    #[inline]
    pub fn set_time(&mut self, time_ms: f32, sample_rate: f32) {
        let n = (time_ms.max(0.0) * 0.001 * sample_rate).round();
        self.ramp_samples = if n >= 1.0 { n as u32 } else { 1 };
    }

    /// Starts a ramp from the current value to `target` (no-op if `target` is already the target).
    #[inline]
    pub fn set_target(&mut self, target: f32) {
        if target.to_bits() == self.target.to_bits() {
            return;
        }
        self.target = target;
        self.remaining = self.ramp_samples;
        self.step = (target - self.current) / self.ramp_samples as f32;
    }

    /// Jumps to `value` immediately (no ramp).
    #[inline]
    pub fn set_immediate(&mut self, value: f32) {
        self.current = value;
        self.target = value;
        self.step = 0.0;
        self.remaining = 0;
    }

    /// Advances one sample and returns the new value.
    #[inline]
    pub fn next_value(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target;
            } else {
                self.current += self.step;
            }
        }
        self.current
    }

    /// The current value.
    #[inline]
    pub fn value(&self) -> f32 {
        self.current
    }

    /// The target value.
    #[inline]
    pub fn target(&self) -> f32 {
        self.target
    }

    /// True when the ramp is finished.
    #[inline]
    pub fn is_settled(&self) -> bool {
        self.remaining == 0
    }
}

/// One-pole lowpass smoother: `y += (target − y)·(1 − e^(−1/(τ·sr)))`. Reaches ~63 % of a step
/// after `time_ms`, ~99.3 % after 5·`time_ms`. The state is `f64` so long time constants do not
/// stall short of the target in `f32`; it snaps to the target once within 1e-7 (no denormals).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnePoleSmoother {
    value: f64,
    coeff: f64,
}

impl OnePoleSmoother {
    /// A smoother at `value` with time constant `time_ms` at `sample_rate`.
    pub fn new(value: f32, time_ms: f32, sample_rate: f32) -> Self {
        let mut s = Self {
            value: f64::from(value),
            coeff: 1.0,
        };
        s.set_time(time_ms, sample_rate);
        s
    }

    /// Changes the time constant (`time_ms <= 0` means no smoothing).
    #[inline]
    pub fn set_time(&mut self, time_ms: f32, sample_rate: f32) {
        let samples = f64::from(time_ms) * 0.001 * f64::from(sample_rate);
        self.coeff = if samples > 0.0 {
            1.0 - (-1.0 / samples).exp()
        } else {
            1.0
        };
    }

    /// Jumps to `value`.
    #[inline]
    pub fn set_immediate(&mut self, value: f32) {
        self.value = f64::from(value);
    }

    /// Advances one sample towards `target` and returns the new value.
    #[inline]
    pub fn next_value(&mut self, target: f32) -> f32 {
        let target = f64::from(target);
        self.value += (target - self.value) * self.coeff;
        if (target - self.value).abs() < 1e-7 {
            self.value = target;
        }
        self.value as f32
    }

    /// The current value.
    #[inline]
    pub fn value(&self) -> f32 {
        self.value as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoother_reaches_target_within_time() {
        let sr = 48_000.0;
        let mut s = Smoother::new(0.0, 5.0, sr); // 240 samples
        s.set_target(1.0);
        let mut prev = 0.0;
        for i in 0..240 {
            let v = s.next_value();
            assert!(v >= prev, "monotonic at {i}");
            assert!(v <= 1.0 + 1e-6);
            prev = v;
        }
        assert!((s.value() - 1.0).abs() < 1e-6);
        assert!(s.is_settled());
        // Halfway point of a linear ramp.
        let mut s = Smoother::new(0.0, 5.0, sr);
        s.set_target(2.0);
        for _ in 0..120 {
            s.next_value();
        }
        assert!((s.value() - 1.0).abs() < 1e-3);
        // Retarget mid-ramp is continuous.
        let before = s.value();
        s.set_target(-1.0);
        let after = s.next_value();
        assert!((after - before).abs() < 0.02);
    }

    #[test]
    fn smoother_set_immediate_jumps() {
        let mut s = Smoother::new(0.0, 5.0, 48_000.0);
        s.set_immediate(0.25);
        assert!((s.next_value() - 0.25).abs() < 1e-9);
        assert!(s.is_settled());
    }

    #[test]
    fn one_pole_smoother_converges() {
        let sr = 48_000.0;
        let mut s = OnePoleSmoother::new(0.0, 10.0, sr);
        let mut v = 0.0;
        for _ in 0..480 {
            v = s.next_value(1.0);
        }
        assert!((v - (1.0 - (-1.0f32).exp())).abs() < 0.01, "{v}");
        for _ in 0..48_000 {
            v = s.next_value(1.0);
        }
        assert!((v - 1.0).abs() < 1e-9, "snaps to target: {v}");
    }
}
