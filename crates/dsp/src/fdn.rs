//! Eight-line feedback delay network core.
//!
//! Per line: a Hermite-interpolated modulated delay, a one-pole damping lowpass (DC gain 1), a
//! feedback gain, and an in-loop Schroeder allpass for echo density. The lines are mixed with an
//! orthonormal Hadamard matrix, so with gains of 1, no damping and allpass lines the network is
//! lossless; with gains `g_i < 1` the energy decays by `g_i^2` per trip. Delays, gains and the
//! damping coefficient are supplied per sample by the caller (which owns modulation and
//! smoothing). Allocation only in [`Fdn8::new`].

use crate::{Allpass, ModDelay, flush_denormal, hadamard8};

/// Number of lines.
pub const FDN_LINES: usize = 8;

/// Eight-line Hadamard FDN.
#[derive(Debug, Clone)]
pub struct Fdn8 {
    lines: [ModDelay; FDN_LINES],
    allpass: [Allpass; FDN_LINES],
    lp: [f32; FDN_LINES],
}

impl Fdn8 {
    /// A network whose lines can delay up to `max_delay` samples, with the in-loop allpass
    /// delays `allpass_delays` (samples). Non-RT.
    pub fn new(max_delay: usize, allpass_delays: [usize; FDN_LINES]) -> Self {
        Self {
            lines: std::array::from_fn(|_| ModDelay::new(max_delay)),
            allpass: std::array::from_fn(|i| Allpass::new(allpass_delays[i])),
            lp: [0.0; FDN_LINES],
        }
    }

    /// Clears every line and filter state.
    pub fn clear(&mut self) {
        for l in &mut self.lines {
            l.clear();
        }
        for a in &mut self.allpass {
            a.clear();
        }
        self.lp = [0.0; FDN_LINES];
    }

    /// Advances one sample and returns the filtered line outputs (before mixing).
    ///
    /// * `delays`: line delays in samples (clamped to the line range).
    /// * `gains`: feedback gain per line.
    /// * `damp`: damping pole `a` in `0..1` (`y = (1 - a)*x + a*y`; 0 bypasses).
    /// * `ap_g`: allpass coefficient.
    /// * `inject`: input added to each line after mixing.
    #[inline]
    pub fn tick(
        &mut self,
        delays: &[f32; FDN_LINES],
        gains: &[f32; FDN_LINES],
        damp: f32,
        ap_g: f32,
        inject: &[f32; FDN_LINES],
    ) -> [f32; FDN_LINES] {
        let mut out = [0.0f32; FDN_LINES];
        for i in 0..FDN_LINES {
            let v = self.lines[i].read(delays[i]);
            let y = flush_denormal((1.0 - damp) * v + damp * self.lp[i]);
            self.lp[i] = y;
            out[i] = self.allpass[i].process(y * gains[i], ap_g);
        }
        let mut m = out;
        hadamard8(&mut m);
        for i in 0..FDN_LINES {
            self.lines[i].write(m[i] + inject[i]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: [f32; 8] = [101.0, 131.0, 157.0, 199.0, 223.0, 257.0, 293.0, 331.0];

    fn energy_run(gain: f32, damp: f32, ap_g: f32, n: usize) -> Vec<f64> {
        let mut f = Fdn8::new(400, [11, 13, 17, 19, 23, 29, 31, 37]);
        let mut inj = [0.0f32; 8];
        inj[0] = 1.0;
        let zero = [0.0f32; 8];
        let mut e = Vec::new();
        for s in 0..n {
            let o = f.tick(
                &D,
                &[gain; 8],
                damp,
                ap_g,
                if s == 0 { &inj } else { &zero },
            );
            e.push(o.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>());
        }
        e
    }

    #[test]
    fn fdn_unity_gain_is_lossless() {
        let e = energy_run(1.0, 0.0, 0.5, 20_000);
        let first: f64 = e[..2_000].iter().sum();
        let last: f64 = e[18_000..].iter().sum();
        assert!(first > 0.5, "first {first}");
        assert!((last / first - 1.0).abs() < 0.25, "{first} vs {last}");
    }

    #[test]
    fn fdn_sub_unity_gain_decays_and_flushes_to_zero() {
        let e = energy_run(0.9, 0.2, 0.5, 40_000);
        let first: f64 = e[..2_000].iter().sum();
        let last: f64 = e[38_000..].iter().sum();
        assert!(last < first * 1e-6, "{first} vs {last}");
        assert!(e.iter().all(|v| v.is_finite()));
        let e = energy_run(0.5, 0.0, 0.5, 40_000);
        assert_eq!(e[39_999], 0.0, "denormal flush reaches exact zero");
    }
}
