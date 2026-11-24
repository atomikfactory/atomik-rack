//! Schroeder allpass diffuser with an integer delay: `w = x + g*w[n-D]`, `y = w[n-D] - g*w`.
//! Unity magnitude at every frequency for `|g| < 1`; average group delay `D`.

use crate::flush_denormal;

/// Integer-delay Schroeder allpass.
#[derive(Debug, Clone, PartialEq)]
pub struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

impl Allpass {
    /// An allpass with a delay of `delay` samples (at least 1). Non-RT.
    pub fn new(delay: usize) -> Self {
        Self {
            buf: vec![0.0; delay.max(1)],
            pos: 0,
        }
    }

    /// The delay in samples.
    #[inline]
    pub fn delay(&self) -> usize {
        self.buf.len()
    }

    /// Zeroes the state.
    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }

    /// Whether every stored sample is `+0.0` (a scan of the delay: for a rest check made once a
    /// batch after the cheaper conditions hold, not for the per-sample path). A silent input then
    /// gives `+0.0` at the output and leaves the state as it is, for any `g` in range.
    pub fn holds_only_zeros(&self) -> bool {
        self.buf.iter().all(|x| x.to_bits() == 0)
    }

    /// Processes one sample with coefficient `g` (clamped to +-0.99).
    #[inline]
    pub fn process(&mut self, x: f32, g: f32) -> f32 {
        let g = g.clamp(-0.99, 0.99);
        let wd = self.buf[self.pos];
        let w = x + g * wd;
        let y = wd - g * w;
        self.buf[self.pos] = flush_denormal(w);
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allpass_is_energy_preserving() {
        let mut a = Allpass::new(37);
        let mut e = 0.0f64;
        for n in 0..4_000 {
            let y = a.process(if n == 0 { 1.0 } else { 0.0 }, 0.6);
            e += f64::from(y) * f64::from(y);
        }
        assert!((e - 1.0).abs() < 1e-4, "energy {e}");
    }

    #[test]
    fn allpass_first_output_is_minus_g_and_delay_reports() {
        let mut a = Allpass::new(5);
        assert_eq!(a.delay(), 5);
        assert!((a.process(1.0, 0.5) + 0.5).abs() < 1e-7);
        a.clear();
        assert_eq!(a.process(0.0, 0.5), 0.0);
    }

    #[test]
    fn the_scan_finds_a_value_anywhere_in_the_delay_and_a_silent_run_empties_it() {
        let mut a = Allpass::new(7);
        assert!(a.holds_only_zeros());
        a.process(1.0, 0.5);
        assert!(!a.holds_only_zeros());
        let mut n = 0;
        while !a.holds_only_zeros() {
            a.process(0.0, 0.5);
            n += 1;
            assert!(n < 2_000, "the flush never emptied it");
        }
        // Silence in, `+0.0` out, and nothing stored but `+0.0`, for either sign of `g`.
        for g in [0.5, -0.5, 0.0] {
            assert_eq!(a.process(0.0, g).to_bits(), 0);
        }
        assert!(a.holds_only_zeros());
    }
}
