//! The clipper's transfer curve: a hard-to-soft knee with an adjustable symmetry, and a DC
//! blocker for the asymmetric case. Pure per-sample maths (no state, no allocation), used by the
//! clipper device and by the panel that draws the curve.
//!
//! # The curve
//!
//! Per side (positive or negative half) there is a ceiling `c` and a knee fraction
//! `k = 1 - curve` (`curve` 0..1: 0 is hard, 1 is soft). For an input magnitude `a`:
//!
//! ```text
//! a <= k c                   out = a                       (untouched: unity gain, bit for bit)
//! a >  k c   (hard, k = 1)   out = c
//! a >  k c   (soft)          t   = (a - k c) / ((1 - k) c)
//!                            out = k c + (1 - k) c * t / sqrt(1 + t^2)
//! ```
//!
//! The soft branch leaves the knee with slope 1 (no corner) and settles on `c` without ever
//! passing it, so the output never exceeds the ceiling. At `curve` 1 the knee is at 0 and the
//! curve is `c * u / sqrt(1 + u^2)` with `u = x / c`, an algebraic sigmoid; at `curve` 0 it is
//! the hard clip `clamp(x, -c, c)`. Only `+ - * / sqrt` are used, so the result is identical on
//! every platform.
//!
//! # Symmetry
//!
//! `symmetry` -1..+1 lowers the ceiling of one side: positive values lower the positive ceiling
//! to `c (1 - symmetry / 2)`, negative values lower the negative one the same way. The other
//! side keeps `c`. A curve that treats the two halves differently adds even harmonics to the odd
//! ones a symmetric curve makes, and a DC offset; [`DcBlock`] takes the offset out.

/// One half of the curve.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Side {
    ceiling: f32,
    knee: f32,
    inv_width: f32,
    width: f32,
    hard: bool,
}

impl Side {
    fn new(ceiling: f32, curve: f32) -> Self {
        let width = ceiling * curve;
        Self {
            ceiling,
            knee: ceiling - width,
            inv_width: if width > 0.0 { 1.0 / width } else { 0.0 },
            width,
            // A knee narrower than a ten-thousandth of the ceiling is a hard clip.
            hard: width < ceiling * 1.0e-4,
        }
    }

    /// The output for an input magnitude `a >= 0`.
    #[inline]
    fn shape(&self, a: f32) -> f32 {
        if a <= self.knee {
            a
        } else if self.hard {
            self.ceiling
        } else {
            let t = (a - self.knee) * self.inv_width;
            (self.knee + self.width * t / (1.0 + t * t).sqrt()).min(self.ceiling)
        }
    }
}

/// The transfer curve for one ceiling, curve and symmetry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipCurve {
    pos: Side,
    neg: Side,
    ceiling: f32,
}

impl ClipCurve {
    /// The curve for a linear `ceiling` (> 0), `curve` 0..1 (0 hard, 1 soft) and `symmetry`
    /// -1..+1. Out-of-range values are clamped; NaN counts as the neutral value.
    pub fn new(ceiling: f32, curve: f32, symmetry: f32) -> Self {
        let c = if ceiling.is_finite() {
            ceiling.max(1.0e-6)
        } else {
            1.0
        };
        let k = if curve.is_nan() {
            0.0
        } else {
            curve.clamp(0.0, 1.0)
        };
        let s = if symmetry.is_nan() {
            0.0
        } else {
            symmetry.clamp(-1.0, 1.0)
        };
        Self {
            pos: Side::new(c * (1.0 - 0.5 * s.max(0.0)), k),
            neg: Side::new(c * (1.0 - 0.5 * (-s).max(0.0)), k),
            ceiling: c,
        }
    }

    /// The ceiling the curve was made for (the larger of the two sides).
    pub fn ceiling(&self) -> f32 {
        self.ceiling
    }

    /// The output for the input `x`.
    #[inline]
    pub fn apply(&self, x: f32) -> f32 {
        if x >= 0.0 {
            self.pos.shape(x)
        } else {
            -self.neg.shape(-x)
        }
    }

    /// True when `x` is beyond the ceiling of its side: the sample the curve holds at, or
    /// squeezes against, the ceiling.
    #[inline]
    pub fn reaches_ceiling(&self, x: f32) -> bool {
        if x >= 0.0 {
            x > self.pos.ceiling
        } else {
            -x > self.neg.ceiling
        }
    }
}

/// The curve with a ceiling of 1, for drawing: `x` in units of the ceiling.
pub fn transfer(x: f32, curve: f32, symmetry: f32) -> f32 {
    ClipCurve::new(1.0, curve, symmetry).apply(x)
}

/// The corner of the DC blocker, Hz.
pub const DC_BLOCK_HZ: f64 = 8.0;

/// A one-pole high-pass at [`DC_BLOCK_HZ`] that removes the offset an asymmetric curve makes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DcBlock {
    r: f64,
    x1: f64,
    y1: f64,
}

impl DcBlock {
    /// A blocker for `sample_rate`.
    pub fn new(sample_rate: f32) -> Self {
        let sr = f64::from(sample_rate.max(1.0));
        Self {
            r: (1.0 - std::f64::consts::TAU * DC_BLOCK_HZ / sr).clamp(0.0, 0.999_999),
            x1: 0.0,
            y1: 0.0,
        }
    }

    /// Clears the state.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    /// One sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = if y.abs() < 1.0e-30 { 0.0 } else { y };
        y as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_curve_is_a_plain_clamp_and_untouched_below_the_ceiling() {
        let c = ClipCurve::new(0.5, 0.0, 0.0);
        for i in -100..=100 {
            let x = i as f32 * 0.01;
            let want = x.clamp(-0.5, 0.5);
            assert_eq!(c.apply(x).to_bits(), want.to_bits(), "x = {x}");
        }
    }

    #[test]
    fn soft_curve_is_unity_below_the_knee_and_never_passes_the_ceiling() {
        let c = ClipCurve::new(1.0, 0.5, 0.0);
        // Knee at 0.5.
        for i in 0..=50 {
            let x = i as f32 * 0.01;
            assert_eq!(c.apply(x).to_bits(), x.to_bits());
        }
        let mut last = 0.0f32;
        for i in 0..2000 {
            let y = c.apply(i as f32 * 0.01);
            assert!(y <= 1.0 && y >= last, "{y} after {last}");
            last = y;
        }
        assert!(c.apply(1.0e6) <= 1.0);
        // Slope 1 at the knee (no corner): the step just above it is nearly the input step.
        let d = c.apply(0.5 + 1.0e-3) - c.apply(0.5);
        assert!((d - 1.0e-3).abs() < 2.0e-6, "{d}");
    }

    #[test]
    fn fully_soft_is_the_algebraic_sigmoid_and_is_odd() {
        let c = ClipCurve::new(1.0, 1.0, 0.0);
        for i in -50..=50 {
            let x = i as f32 * 0.1;
            let want = x / (1.0 + x * x).sqrt();
            assert!((c.apply(x) - want).abs() < 1.0e-6, "x = {x}");
            assert_eq!(c.apply(-x).to_bits(), (-c.apply(x)).to_bits());
        }
    }

    #[test]
    fn symmetry_lowers_one_side_only() {
        let up = ClipCurve::new(1.0, 0.0, 1.0);
        assert_eq!(up.apply(2.0), 0.5);
        assert_eq!(up.apply(-2.0), -1.0);
        let down = ClipCurve::new(1.0, 0.0, -0.5);
        assert_eq!(down.apply(2.0), 1.0);
        assert_eq!(down.apply(-2.0), -0.75);
        assert_eq!(down.ceiling(), 1.0);
        assert!(up.reaches_ceiling(0.6) && !up.reaches_ceiling(-0.6));
    }

    #[test]
    fn odd_curves_make_odd_harmonics_and_asymmetric_ones_add_even_ones() {
        let n = 4096usize;
        let sine: Vec<f32> = (0..n)
            .map(|i| 1.6 * (std::f64::consts::TAU * 8.0 * i as f64 / n as f64).sin() as f32)
            .collect();
        let harmonic = |sym: f32, h: usize| {
            let c = ClipCurve::new(1.0, 0.3, sym);
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &x) in sine.iter().enumerate() {
                let y = f64::from(c.apply(x));
                let ph = std::f64::consts::TAU * (8 * h) as f64 * i as f64 / n as f64;
                re += y * ph.cos();
                im += y * ph.sin();
            }
            2.0 * (re * re + im * im).sqrt() / n as f64
        };
        assert!(harmonic(0.0, 3) > 1.0e-2);
        assert!(harmonic(0.0, 2) < 1.0e-6, "{}", harmonic(0.0, 2));
        assert!(harmonic(0.5, 2) > 1.0e-2, "{}", harmonic(0.5, 2));
    }

    #[test]
    fn bad_numbers_are_tamed() {
        let c = ClipCurve::new(f32::NAN, f32::NAN, f32::NAN);
        assert_eq!(c.apply(3.0), 1.0);
        let c = ClipCurve::new(0.0, 5.0, 9.0);
        assert!(c.apply(1.0).is_finite());
    }

    #[test]
    fn dc_block_removes_an_offset_and_passes_a_tone() {
        let mut b = DcBlock::new(48_000.0);
        let mut last = 1.0f32;
        for _ in 0..48_000 {
            last = b.process(0.5);
        }
        assert!(last.abs() < 1.0e-3, "{last}");
        let mut b = DcBlock::new(48_000.0);
        let mut peak = 0.0f32;
        for i in 0..48_000 {
            let x = (std::f64::consts::TAU * 1000.0 * i as f64 / 48_000.0).sin() as f32;
            let y = b.process(x);
            if i > 4_800 {
                peak = peak.max(y.abs());
            }
        }
        assert!((peak - 1.0).abs() < 0.01, "{peak}");
    }
}
