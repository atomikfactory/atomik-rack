//! Deterministic sine and cosine for the stage blocks (phase in cycles, `f64`, `+ - * /` only).
//!
//! The Taylor series of `sin` on `[-pi/2, pi/2]` (to `x^17`, error below 5e-14) after a symmetry
//! reduction, so every platform produces the same bits (no libm call).

/// `sin(2 pi p)` for any finite `p` in cycles (non-finite gives 0).
#[inline]
pub(crate) fn sin_cycles(p: f64) -> f64 {
    if !p.is_finite() {
        return 0.0;
    }
    // Reduce to [-0.5, 0.5), then fold to [-0.25, 0.25] with sin(pi - x) = sin(x).
    let mut q = p - p.floor();
    if q >= 0.5 {
        q -= 1.0;
    }
    if q > 0.25 {
        q = 0.5 - q;
    } else if q < -0.25 {
        q = -0.5 - q;
    }
    let x = q * std::f64::consts::TAU;
    let x2 = x * x;
    // sin x = x (1 - x^2/(2*3) (1 - x^2/(4*5) (1 - ...))), eight nested terms.
    let mut acc = 1.0;
    let mut k = 8.0;
    while k >= 1.0 {
        acc = 1.0 - x2 * acc / ((2.0 * k) * (2.0 * k + 1.0));
        k -= 1.0;
    }
    x * acc
}

/// `cos(2 pi p)` for any finite `p` in cycles.
#[inline]
pub(crate) fn cos_cycles(p: f64) -> f64 {
    sin_cycles(p + 0.25)
}

/// A free-running sine oscillator on a `f64` phase accumulator (cycles), output in `-1..=1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SineLfo {
    phase: f64,
    inc: f64,
}

impl SineLfo {
    /// An oscillator at `phase` (cycles) and 0 Hz.
    pub(crate) fn new(phase: f64) -> Self {
        Self {
            phase: phase - phase.floor(),
            inc: 0.0,
        }
    }

    /// Sets the rate (clamped to `0..=0.5 sample_rate`; non-finite gives 0).
    pub(crate) fn set_rate(&mut self, hz: f32, sample_rate: f32) {
        let inc = f64::from(hz) / f64::from(sample_rate);
        self.inc = if inc.is_finite() {
            inc.clamp(0.0, 0.5)
        } else {
            0.0
        };
    }

    /// Restarts at `phase` cycles.
    pub(crate) fn set_phase(&mut self, phase: f64) {
        self.phase = phase - phase.floor();
    }

    /// The output at the current phase plus `offset` cycles, then advances one sample.
    #[inline]
    pub(crate) fn next_with_offset(&mut self, offset: f64) -> f32 {
        let y = sin_cycles(self.phase + offset) as f32;
        self.phase += self.inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        y
    }

    /// The output at the current phase, then advances one sample.
    #[inline]
    pub(crate) fn next_sample(&mut self) -> f32 {
        self.next_with_offset(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_lfo_has_the_requested_rate_and_range() {
        let mut l = SineLfo::new(0.0);
        l.set_rate(5.0, 48_000.0);
        let x: Vec<f32> = (0..96_000).map(|_| l.next_sample()).collect();
        assert!(x.iter().all(|v| v.abs() <= 1.0));
        let up = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!(up.abs_diff(10) <= 1, "{up}");
        l.set_phase(0.25);
        assert!((l.next_with_offset(0.0) - 1.0).abs() < 1e-6);
        l.set_rate(f32::NAN, 48_000.0);
        let a = l.next_sample();
        assert_eq!(a.to_bits(), l.next_sample().to_bits(), "NaN rate stops it");
    }

    #[test]
    fn sin_cycles_matches_libm_within_1e_12() {
        let mut p = -3.7f64;
        while p < 3.7 {
            let want = (std::f64::consts::TAU * p).sin();
            assert!((sin_cycles(p) - want).abs() < 1e-12, "p={p}");
            assert!((cos_cycles(p) - (std::f64::consts::TAU * p).cos()).abs() < 1e-12);
            p += 0.00731;
        }
        assert_eq!(sin_cycles(0.0), 0.0);
        assert_eq!(sin_cycles(f64::NAN), 0.0);
        assert!((sin_cycles(0.25) - 1.0).abs() < 1e-13);
    }

    #[test]
    fn sin_cycles_is_odd_and_periodic() {
        for k in 0..100 {
            let p = f64::from(k) * 0.0137;
            assert!((sin_cycles(p) + sin_cycles(-p)).abs() < 1e-13);
            assert!((sin_cycles(p + 1.0) - sin_cycles(p)).abs() < 1e-12);
        }
    }
}
