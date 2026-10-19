//! Critically damped second-order smoother in three speed classes.
//!
//! Two identical one-pole stages in cascade: the step response has no overshoot, a smooth start
//! (zero initial slope) and a smooth end. `rise_ms` is the time to reach 90 % of a step; the
//! three classes of the Character device are [`SmootherClass::Fast`] (about 5 ms),
//! [`SmootherClass::Medium`] (about 25 ms) and [`SmootherClass::Slow`] (about 50 ms). The state is
//! `f64`, the pole comes from the deterministic [`crate::exp2_det_f64`] (no libm), and the
//! smoother snaps to its target once within 1e-9, so no denormal tail remains.

use crate::exp2_det_f64;

/// Time for a critically damped step response to reach 90 %, in units of the stage time
/// constant (the root of `1 - e^-x (1 + x) = 0.9`).
const T90_TAUS: f64 = 3.889_720_170_6;

/// Speed class of a smoother (starting values; the voicing may use [`Smoother2::new`] directly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmootherClass {
    /// Routing gains, EQ gains, drive and mix: about 5 ms.
    Fast,
    /// Filter and crossover frequencies, delay centres: about 25 ms.
    Medium,
    /// Output volume: about 50 ms.
    Slow,
}

impl SmootherClass {
    /// The 90 % rise time of the class in milliseconds.
    pub const fn rise_ms(self) -> f32 {
        match self {
            Self::Fast => 5.0,
            Self::Medium => 25.0,
            Self::Slow => 50.0,
        }
    }
}

/// Critically damped second-order smoother.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Smoother2 {
    y1: f64,
    y2: f64,
    target: f64,
    coeff: f64,
}

impl Smoother2 {
    /// A smoother at `value` that reaches 90 % of a step in `rise_ms` at `sample_rate`
    /// (`rise_ms <= 0` follows the target at once).
    pub fn new(value: f32, rise_ms: f32, sample_rate: f32) -> Self {
        let mut s = Self {
            y1: f64::from(value),
            y2: f64::from(value),
            target: f64::from(value),
            coeff: 1.0,
        };
        s.set_time(rise_ms, sample_rate);
        s
    }

    /// A smoother of the given class at `value`.
    pub fn with_class(value: f32, class: SmootherClass, sample_rate: f32) -> Self {
        Self::new(value, class.rise_ms(), sample_rate)
    }

    /// Changes the 90 % rise time (state is kept).
    pub fn set_time(&mut self, rise_ms: f32, sample_rate: f32) {
        let tau_samples = f64::from(rise_ms) * 0.001 * f64::from(sample_rate) / T90_TAUS;
        self.coeff = if tau_samples > 1e-3 && tau_samples.is_finite() {
            1.0 - exp2_det_f64(-std::f64::consts::LOG2_E / tau_samples)
        } else {
            1.0
        };
    }

    /// Sets the value to move towards (non-finite values are ignored).
    #[inline]
    pub fn set_target(&mut self, target: f32) {
        if target.is_finite() {
            self.target = f64::from(target);
        }
    }

    /// Jumps to `value` with no transition (non-finite gives 0).
    #[inline]
    pub fn set_immediate(&mut self, value: f32) {
        let v = if value.is_finite() {
            f64::from(value)
        } else {
            0.0
        };
        self.y1 = v;
        self.y2 = v;
        self.target = v;
    }

    /// Advances one sample and returns the smoothed value.
    #[inline]
    pub fn next_value(&mut self) -> f32 {
        self.y1 += self.coeff * (self.target - self.y1);
        self.y2 += self.coeff * (self.y1 - self.y2);
        let tol = 1e-9 * self.target.abs().max(1.0);
        if (self.target - self.y2).abs() < tol && (self.target - self.y1).abs() < tol {
            self.y1 = self.target;
            self.y2 = self.target;
        }
        self.y2 as f32
    }

    /// The current output.
    #[inline]
    pub fn value(&self) -> f32 {
        self.y2 as f32
    }

    /// The target.
    #[inline]
    pub fn target(&self) -> f32 {
        self.target as f32
    }

    /// True when the output equals the target exactly.
    #[inline]
    pub fn is_settled(&self) -> bool {
        self.y1.to_bits() == self.target.to_bits() && self.y2.to_bits() == self.target.to_bits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn step(rise_ms: f32, n: usize) -> Vec<f32> {
        let mut s = Smoother2::new(0.0, rise_ms, SR);
        s.set_target(1.0);
        (0..n).map(|_| s.next_value()).collect()
    }

    #[test]
    fn smoother2_step_has_no_overshoot_and_is_monotone() {
        for ms in [1.0, 5.0, 25.0, 50.0, 500.0] {
            let y = step(ms, (ms * 0.001 * SR * 14.0) as usize + 200);
            assert!(y.iter().all(|v| *v <= 1.0), "overshoot at {ms} ms");
            assert!(
                y.windows(2).all(|w| w[1] >= w[0]),
                "not monotone at {ms} ms"
            );
            assert_eq!(*y.last().unwrap(), 1.0, "settles exactly");
        }
    }

    #[test]
    fn smoother2_reaches_90_percent_at_the_rise_time() {
        for class in [
            SmootherClass::Fast,
            SmootherClass::Medium,
            SmootherClass::Slow,
        ] {
            let ms = class.rise_ms();
            let y = step(ms, (SR * 0.5) as usize);
            let n90 = y.iter().position(|v| *v >= 0.9).unwrap() as f32;
            let want = ms * 0.001 * SR;
            assert!(
                (n90 - want).abs() / want < 0.03,
                "{class:?}: {n90} vs {want}"
            );
        }
    }

    #[test]
    fn smoother2_starts_smoothly_with_two_cascaded_poles() {
        let y = step(25.0, 4);
        // Second order: the first sample moves by c^2 only, far less than one pole would.
        let mut one = crate::OnePoleSmoother::new(0.0, 25.0 / 3.89, SR);
        assert!(y[0] < one.next_value(1.0) * 0.5);
        assert!(y[0] > 0.0 && y[0] < y[1]);
    }

    #[test]
    fn smoother2_zero_time_follows_and_immediate_jumps() {
        let mut s = Smoother2::new(0.0, 0.0, SR);
        s.set_target(0.7);
        assert_eq!(s.next_value(), 0.7);
        assert!(s.is_settled());
        s.set_immediate(-2.0);
        assert_eq!(s.value(), -2.0);
        s.set_target(f32::NAN);
        assert_eq!(s.target(), -2.0, "non-finite target ignored");
        s.set_immediate(f32::INFINITY);
        assert_eq!(s.value(), 0.0);
        let c = Smoother2::with_class(1.0, SmootherClass::Slow, SR);
        assert_eq!(c.value(), 1.0);
    }

    #[test]
    fn smoother2_retarget_is_continuous_and_downward_steps_work() {
        let mut s = Smoother2::new(0.0, 10.0, SR);
        s.set_target(1.0);
        let mut last = 0.0;
        for _ in 0..200 {
            last = s.next_value();
        }
        s.set_target(0.0);
        let mut prev = last;
        for _ in 0..50_000 {
            let v = s.next_value();
            assert!((v - prev).abs() < 0.05, "continuous");
            assert!(v > -1e-6, "no undershoot");
            prev = v;
        }
        assert_eq!(prev, 0.0);
    }

    #[test]
    fn the_three_classes_have_their_documented_rise_times() {
        assert_eq!(SmootherClass::Fast.rise_ms(), 5.0);
        assert_eq!(SmootherClass::Medium.rise_ms(), 25.0);
        assert_eq!(SmootherClass::Slow.rise_ms(), 50.0);
        for class in [
            SmootherClass::Fast,
            SmootherClass::Medium,
            SmootherClass::Slow,
        ] {
            let mut s = Smoother2::with_class(0.0, class, SR);
            s.set_target(1.0);
            let n90 = (class.rise_ms() * 0.001 * SR) as usize;
            let mut v = 0.0;
            for _ in 0..n90 {
                v = s.next_value();
            }
            assert!((v - 0.9).abs() < 0.02, "{class:?}: {v} at its rise time");
        }
    }
}
