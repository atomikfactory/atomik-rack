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
    ///
    /// A smoother that already holds the target, bit for bit, and that target is finite, is not
    /// stepped: the step adds `(target - value) * coeff`, which is `+0` for a finite `coeff` in
    /// `0..=1` (what `set_time` makes), and the snap then writes `target` back, so the state and
    /// the result are what they were (a `-0` value ends on the `-0` target, as the snap leaves
    /// it). An infinite or NaN value takes the full path, which turns it into NaN as before.
    #[inline]
    pub fn next_value(&mut self, target: f32) -> f32 {
        let target = f64::from(target);
        if self.value.to_bits() == target.to_bits() && target.is_finite() {
            return self.value as f32;
        }
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

    /// `next_value` as it was before the shortcut for a smoother on its target.
    fn full_step(s: &mut OnePoleSmoother, target: f32) -> f32 {
        let target = f64::from(target);
        s.value += (target - s.value) * s.coeff;
        if (target - s.value).abs() < 1e-7 {
            s.value = target;
        }
        s.value as f32
    }

    fn same(a: &OnePoleSmoother, b: &OnePoleSmoother) -> bool {
        a.value.to_bits() == b.value.to_bits() && a.coeff.to_bits() == b.coeff.to_bits()
    }

    #[test]
    fn one_pole_shortcut_matches_the_full_step_bit_for_bit() {
        let specials = [
            0.0f32,
            -0.0,
            1.0,
            -1.0,
            0.5,
            1e-7,
            -1e-7,
            1.0 + 1e-7,
            1e-8,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
            -f32::from_bits(1),
            f32::MAX,
            f32::MIN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
        ];
        // Every coefficient `set_time` can make: no smoothing, usual, a time that rounds to
        // nothing, an infinite time, a NaN time, a zero rate.
        let times: [(f32, f32); 6] = [
            (0.0, 48_000.0),
            (10.0, 48_000.0),
            (1e30, 48_000.0),
            (f32::INFINITY, 48_000.0),
            (f32::NAN, 48_000.0),
            (10.0, 0.0),
        ];
        for (ms, sr) in times {
            let mut proto = OnePoleSmoother::new(0.0, ms, sr);
            proto.set_time(ms, sr);
            assert!(proto.coeff.is_finite() && (0.0..=1.0).contains(&proto.coeff));
            for &v in &specials {
                for &t in &specials {
                    let mut a = proto;
                    a.set_immediate(v);
                    let mut b = a;
                    for _ in 0..3 {
                        let ra = a.next_value(t);
                        let rb = full_step(&mut b, t);
                        assert_eq!(ra.to_bits(), rb.to_bits(), "{ms} {sr} {v} {t}");
                        assert!(same(&a, &b), "{ms} {sr} {v} {t}");
                    }
                }
            }
        }
    }

    #[test]
    fn one_pole_shortcut_matches_over_a_long_seeded_sequence() {
        let mut seed = 0x9E37_79B9u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed >> 8
        };
        let mut a = OnePoleSmoother::new(0.0, 0.5, 48_000.0);
        let mut b = a;
        let mut target = 0.0f32;
        let mut hits = 0u32;
        for _ in 0..100_000 {
            // The target holds for long stretches, sometimes a zero of either sign.
            match next() % 4000 {
                0 => target = (next() as f32 / (1u32 << 24) as f32) * 4.0 - 2.0,
                1 => target = 0.0,
                2 => target = -0.0,
                3 => target += 5e-8,
                _ => {}
            }
            let before = a;
            let ra = a.next_value(target);
            let rb = full_step(&mut b, target);
            assert_eq!(ra.to_bits(), rb.to_bits());
            assert!(same(&a, &b));
            if same(&before, &a) {
                hits += 1;
            }
        }
        assert!(
            hits > 50_000,
            "the sequence rarely sits on its target: {hits}"
        );
    }

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
