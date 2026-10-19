//! Zero-delay-feedback (topology-preserving transform) state-variable filter, 12 dB/octave,
//! after the public-domain trapezoidal-integrator SVF formulation (Zavalishin, Simper).
//!
//! * `g = tan(π·fc/sr)` with `fc` clamped to `1 Hz..=0.49·sr`.
//! * Damping `k = 1/Q`. **Resonance mapping:** `Q = 0.5·40^r` for `r` in `0..=1`, so
//!   `r = 0` → Q 0.5 (critically damped, no peak), `r ≈ 0.094` → Q 0.707 (Butterworth),
//!   `r = 1` → Q 20 (strong peak, but `k = 0.05 > 0` keeps the filter strictly stable: it never
//!   self-oscillates or blows up).
//! * **Denormals:** the two integrator states are flushed to zero when their magnitude drops
//!   below 1e-20.

/// Which output a single-output call returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// 12 dB/oct lowpass.
    Lowpass,
    /// 12 dB/oct (6 dB/oct per side) bandpass, peak gain Q.
    Bandpass,
    /// 12 dB/oct highpass.
    Highpass,
}

impl FilterMode {
    /// Maps a choice index (0 LP, 1 BP, 2 HP; out of range clamps) to a mode.
    #[inline]
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Lowpass,
            1 => Self::Bandpass,
            _ => Self::Highpass,
        }
    }
}

/// All three simultaneous outputs of one [`Svf`] step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SvfOutputs {
    /// Lowpass.
    pub lp: f32,
    /// Bandpass.
    pub bp: f32,
    /// Highpass.
    pub hp: f32,
}

/// TPT state-variable filter (see the module docs for the resonance mapping).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Svf {
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: f32,
    ic2: f32,
}

impl Default for Svf {
    fn default() -> Self {
        Self::new()
    }
}

impl Svf {
    /// Highest cutoff as a fraction of the sample rate.
    pub const MAX_CUTOFF_RATIO: f32 = 0.49;

    /// A filter at 1 kHz / 48 kHz with resonance 0 and cleared state.
    pub fn new() -> Self {
        let mut f = Self {
            k: 2.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            ic1: 0.0,
            ic2: 0.0,
        };
        f.set_params(1_000.0, 0.0, 48_000.0);
        f
    }

    /// Quality factor for a resonance in `0..=1` (clamped): `Q = 0.5·40^r`.
    #[inline]
    pub fn resonance_to_q(resonance: f32) -> f32 {
        let r = if resonance.is_nan() {
            0.0
        } else {
            resonance.clamp(0.0, 1.0)
        };
        0.5 * 40.0f32.powf(r)
    }

    /// The integrator gain `g = tan(π·fc/sr)`, with `fc` clamped to `1 Hz..=0.49·sr`.
    #[inline]
    pub fn cutoff_to_g(cutoff_hz: f32, sample_rate: f32) -> f32 {
        let hi = Self::MAX_CUTOFF_RATIO * sample_rate;
        let fc = if cutoff_hz.is_nan() {
            hi
        } else {
            cutoff_hz.clamp(1.0, hi)
        };
        (std::f32::consts::PI * fc / sample_rate).tan()
    }

    /// Sets cutoff (Hz), resonance (`0..=1`) and sample rate.
    #[inline]
    pub fn set_params(&mut self, cutoff_hz: f32, resonance: f32, sample_rate: f32) {
        let g = Self::cutoff_to_g(cutoff_hz, sample_rate);
        self.set_coefficients(g, 1.0 / Self::resonance_to_q(resonance));
    }

    /// Sets the raw coefficients: integrator gain `g > 0` and damping `k = 1/Q > 0`.
    #[inline]
    pub fn set_coefficients(&mut self, g: f32, k: f32) {
        self.k = k;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    /// Clears the filter state.
    #[inline]
    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    /// Filters one sample and returns all three outputs.
    #[inline]
    pub fn process(&mut self, x: f32) -> SvfOutputs {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = flush(2.0 * v1 - self.ic1);
        self.ic2 = flush(2.0 * v2 - self.ic2);
        SvfOutputs {
            lp: v2,
            bp: v1,
            hp: x - self.k * v1 - v2,
        }
    }

    /// Filters one sample and returns the output selected by `mode`.
    #[inline]
    pub fn process_mode(&mut self, x: f32, mode: FilterMode) -> f32 {
        let o = self.process(x);
        match mode {
            FilterMode::Lowpass => o.lp,
            FilterMode::Bandpass => o.bp,
            FilterMode::Highpass => o.hp,
        }
    }
}

/// Flushes denormal-range values to zero.
#[inline]
fn flush(v: f32) -> f32 {
    if v.abs() < 1e-20 { 0.0 } else { v }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;
    use crate::test_util::rms;
    use proptest::prelude::*;

    const SR: f32 = 48_000.0;

    /// Steady-state gain in dB of `mode` for a sine at `hz`.
    fn gain_db(f: &mut Svf, mode: FilterMode, hz: f32) -> f32 {
        f.reset();
        let n = 48_000;
        let y: Vec<f32> = (0..n)
            .map(|i| {
                let x = (std::f32::consts::TAU * hz * i as f32 / SR).sin();
                f.process_mode(x, mode)
            })
            .collect();
        20.0 * (rms(&y[n / 2..]) / std::f32::consts::FRAC_1_SQRT_2).log10()
    }

    fn butterworth_resonance() -> f32 {
        std::f32::consts::SQRT_2.ln() / 40.0f32.ln()
    }

    #[test]
    fn svf_resonance_mapping_endpoints() {
        assert!((Svf::resonance_to_q(0.0) - 0.5).abs() < 1e-6);
        assert!((Svf::resonance_to_q(1.0) - 20.0).abs() < 1e-3);
        assert!(
            (Svf::resonance_to_q(butterworth_resonance()) - std::f32::consts::FRAC_1_SQRT_2).abs()
                < 1e-4
        );
        assert!((Svf::resonance_to_q(7.0) - 20.0).abs() < 1e-3, "clamped");
    }

    #[test]
    fn svf_lowpass_attenuates_above_cutoff_12db_per_octave() {
        let mut f = Svf::new();
        f.set_params(1_000.0, butterworth_resonance(), SR);
        let pass = gain_db(&mut f, FilterMode::Lowpass, 100.0);
        let at_fc = gain_db(&mut f, FilterMode::Lowpass, 1_000.0);
        let g4 = gain_db(&mut f, FilterMode::Lowpass, 4_000.0);
        let g8 = gain_db(&mut f, FilterMode::Lowpass, 8_000.0);
        assert!(pass.abs() < 0.2, "passband {pass}");
        assert!((at_fc + 3.01).abs() < 0.3, "at fc {at_fc}");
        assert!((g4 - g8 - 12.0).abs() < 1.5, "octave slope {g4} -> {g8}");
        assert!(g4 < -22.0, "4 kHz {g4}");
    }

    #[test]
    fn svf_highpass_and_bandpass_shapes() {
        let mut f = Svf::new();
        f.set_params(1_000.0, butterworth_resonance(), SR);
        let hp_lo = gain_db(&mut f, FilterMode::Highpass, 125.0);
        let hp_hi = gain_db(&mut f, FilterMode::Highpass, 10_000.0);
        assert!(hp_lo < -30.0 && hp_hi.abs() < 0.5, "HP {hp_lo} {hp_hi}");
        let q = Svf::resonance_to_q(butterworth_resonance());
        let bp_fc = gain_db(&mut f, FilterMode::Bandpass, 1_000.0);
        assert!((bp_fc - 20.0 * q.log10()).abs() < 0.3, "BP peak {bp_fc}");
        assert!(gain_db(&mut f, FilterMode::Bandpass, 100.0) < bp_fc - 15.0);
    }

    #[test]
    fn svf_silence_flushes_state_to_zero() {
        let mut f = Svf::new();
        f.set_params(200.0, 1.0, SR);
        f.process(1.0);
        for _ in 0..(SR as usize * 20) {
            f.process(0.0);
        }
        let o = f.process(0.0);
        assert!(o.lp.to_bits() == 0 && o.bp.to_bits() == 0 && o.hp.to_bits() == 0);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn svf_stable_at_max_resonance_all_cutoffs(
            ratio in 0.0f32..=1.0,
            sr in prop::sample::select(vec![44_100.0f32, 48_000.0, 96_000.0]),
            seed in any::<u64>(),
            modulate in any::<bool>(),
        ) {
            // Cutoff spans 20 Hz..0.49·sr logarithmically.
            let lo = 20.0f32;
            let hi = Svf::MAX_CUTOFF_RATIO * sr;
            let cutoff = lo * (hi / lo).powf(ratio);
            let mut f = Svf::new();
            f.set_params(cutoff, 1.0, sr);
            let mut rng = Rng::new(seed);
            let n = (sr * 0.25) as usize;
            let mut peak = 0.0f32;
            for i in 0..n {
                if modulate && i % 32 == 0 {
                    let r = rng.next_f32();
                    f.set_params(lo * (hi / lo).powf(r), 1.0, sr);
                }
                // Full-scale noise plus a full-scale square at the cutoff (worst-case resonance).
                let sq = if (i as f32 * cutoff / sr).fract() < 0.5 { 1.0 } else { -1.0 };
                let x = 0.5 * rng.next_bipolar() + 0.5 * sq;
                let o = f.process(x);
                for v in [o.lp, o.bp, o.hp] {
                    prop_assert!(v.is_finite());
                    peak = peak.max(v.abs());
                }
            }
            // Q = 20: steady-state peak gain is about 20; allow transient headroom.
            prop_assert!(peak < 60.0, "peak {}", peak);
            // Free decay: energy must not grow.
            let tail: Vec<f32> = (0..(sr * 0.1) as usize).map(|_| f.process(0.0).lp).collect();
            let q = tail.len() / 4;
            prop_assert!(rms(&tail[3 * q..]) <= rms(&tail[..q]) + 1e-6);
        }
    }
}
