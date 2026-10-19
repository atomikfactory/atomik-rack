//! Band-limited oscillator using polynomial band-limited steps (PolyBLEP) and ramps (PolyBLAMP).
//!
//! * Saw and square: naive waveform plus the textbook two-sample polynomial BLEP residual at
//!   each discontinuity.
//! * Triangle: naive triangle plus the two-sample polynomial BLAMP residual (the integral of the
//!   BLEP residual) at each slope corner. This is stateless, so it has no leaky-integrator DC
//!   drift or amplitude dependence on frequency.
//! * Sine: `sin(2π·phase)`.
//!
//! The phase accumulator is `f64`. Changing frequency only changes the increment, so frequency
//! changes are phase-continuous.

/// Oscillator waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    /// Rising sawtooth −1→+1.
    Saw,
    /// Square, +1 for the first half of the cycle.
    Square,
    /// Triangle, −1 at phase 0 and +1 at phase 0.5.
    Triangle,
    /// Sine.
    Sine,
}

impl Waveform {
    /// Maps a choice index (0 saw, 1 square, 2 triangle, 3 sine; out of range clamps) to a waveform.
    #[inline]
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Saw,
            1 => Self::Square,
            2 => Self::Triangle,
            _ => Self::Sine,
        }
    }
}

/// PolyBLEP/PolyBLAMP oscillator. Output is nominally ±1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolyBlepOsc {
    phase: f64,
    inc: f64,
    wave: Waveform,
}

impl PolyBlepOsc {
    /// A zero-frequency oscillator at phase 0.
    pub fn new(wave: Waveform) -> Self {
        Self {
            phase: 0.0,
            inc: 0.0,
            wave,
        }
    }

    /// Selects the waveform (phase is kept).
    #[inline]
    pub fn set_waveform(&mut self, wave: Waveform) {
        self.wave = wave;
    }

    /// The current waveform.
    #[inline]
    pub fn waveform(&self) -> Waveform {
        self.wave
    }

    /// Sets the frequency. Clamped to `0..=0.5·sample_rate`.
    #[inline]
    pub fn set_frequency(&mut self, hz: f32, sample_rate: f32) {
        let inc = f64::from(hz) / f64::from(sample_rate);
        self.inc = if inc.is_finite() {
            inc.clamp(0.0, 0.5)
        } else {
            0.0
        };
    }

    /// Sets the phase (wrapped into `0..1`).
    #[inline]
    pub fn set_phase(&mut self, phase: f64) {
        self.phase = phase.rem_euclid(1.0);
    }

    /// The current phase in `0..1`.
    #[inline]
    pub fn phase(&self) -> f64 {
        self.phase
    }

    /// Produces one sample and advances the phase.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let p = self.phase;
        let dt = self.inc;
        let half = || if p < 0.5 { p + 0.5 } else { p - 0.5 };
        let y = match self.wave {
            Waveform::Saw => 2.0 * p - 1.0 - blep(p, dt),
            Waveform::Square => {
                let naive = if p < 0.5 { 1.0 } else { -1.0 };
                naive + blep(p, dt) - blep(half(), dt)
            }
            Waveform::Triangle => {
                // Slope changes by ±8 per cycle (±8·dt per sample) at the corners.
                let naive = 1.0 - 4.0 * (p - 0.5).abs();
                naive + 8.0 * dt * (blamp(p, dt) - blamp(half(), dt))
            }
            Waveform::Sine => (std::f64::consts::TAU * p).sin(),
        };
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        y as f32
    }
}

/// Two-sample polynomial BLEP residual for a step of +2 at phase 0 (`t` = phase, `dt` = increment).
#[inline]
fn blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let x = t / dt;
        2.0 * x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + 2.0 * x + 1.0
    } else {
        0.0
    }
}

/// Two-sample polynomial BLAMP residual (integral of the unit-step BLEP residual) for a slope
/// increase of 1 per sample at phase 0.
#[inline]
fn blamp(t: f64, dt: f64) -> f64 {
    if t < dt {
        let x = 1.0 - t / dt;
        x * x * x * (1.0 / 6.0)
    } else if t > 1.0 - dt {
        let x = 1.0 + (t - 1.0) / dt;
        x * x * x * (1.0 / 6.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{dominant_hz, max_step, spectrum};

    const SR: f32 = 48_000.0;

    fn render(wave: Waveform, hz: f32, n: usize) -> Vec<f32> {
        let mut o = PolyBlepOsc::new(wave);
        o.set_frequency(hz, SR);
        (0..n).map(|_| o.next_sample()).collect()
    }

    fn naive(wave: Waveform, hz: f32, n: usize) -> Vec<f32> {
        let dt = f64::from(hz / SR);
        (0..n)
            .map(|i| {
                let p = (i as f64 * dt).fract();
                match wave {
                    Waveform::Saw => (2.0 * p - 1.0) as f32,
                    Waveform::Square => {
                        if p < 0.5 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    Waveform::Triangle => (1.0 - 4.0 * (p - 0.5).abs()) as f32,
                    Waveform::Sine => (2.0 * std::f64::consts::PI * p).sin() as f32,
                }
            })
            .collect()
    }

    /// (worst alias below the fundamental, worst alias below 16 kHz), in dB re the fundamental.
    fn alias_levels(x: &[f32], f0: f32) -> (f64, f64) {
        let m = spectrum(x);
        let n = x.len() as f64;
        let bin = |hz: f64| (hz * n / f64::from(SR)).round() as usize;
        let c0 = bin(f64::from(f0));
        let fund = m[c0 - 3..=c0 + 3].iter().fold(0.0f64, |a, b| a.max(*b));
        let mut masked = m.clone();
        let mut k = 1.0;
        while k * f64::from(f0) < f64::from(SR) / 2.0 {
            let c = bin(k * f64::from(f0));
            for v in masked
                .iter_mut()
                .take((c + 7).min(m.len()))
                .skip(c.saturating_sub(6))
            {
                *v = 0.0;
            }
            k += 1.0;
        }
        for v in masked.iter_mut().take(6) {
            *v = 0.0; // DC leakage of the window
        }
        let worst = |hi: f64| {
            masked[..bin(hi)]
                .iter()
                .fold(0.0f64, |a, b| a.max(*b))
                .max(1e-300)
        };
        (
            20.0 * (worst(f64::from(f0)) / fund).log10(),
            20.0 * (worst(16_000.0) / fund).log10(),
        )
    }

    #[test]
    fn polyblep_saw_dc_free_and_bounded() {
        for hz in [55.0, 440.0, 1_000.0, 5_000.0, 15_000.0] {
            let x = render(Waveform::Saw, hz, 48_000);
            let mean = x.iter().map(|s| f64::from(*s)).sum::<f64>() / x.len() as f64;
            assert!(mean.abs() < 2e-3, "{hz} Hz: DC {mean}");
            let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak <= 1.05, "{hz} Hz: peak {peak}");
        }
    }

    #[test]
    fn polyblep_aliasing_below_threshold_at_5khz() {
        let n = 1 << 15;
        for wave in [Waveform::Saw, Waveform::Square, Waveform::Triangle] {
            let (below_f0, below_16k) = alias_levels(&render(wave, 5_000.0, n), 5_000.0);
            assert!(below_f0 < -50.0, "{wave:?}: alias below f0 {below_f0} dB");
            assert!(
                below_16k < -30.0,
                "{wave:?}: alias below 16 kHz {below_16k} dB"
            );
            // The measurement bites: the naive waveform fails it.
            let (naive_f0, _) = alias_levels(&naive(wave, 5_000.0, n), 5_000.0);
            assert!(naive_f0 > -40.0, "{wave:?}: naive {naive_f0} dB");
        }
    }

    #[test]
    fn polyblep_every_wave_has_expected_pitch_and_level() {
        for wave in [
            Waveform::Saw,
            Waveform::Square,
            Waveform::Triangle,
            Waveform::Sine,
        ] {
            let x = render(wave, 440.0, 48_000);
            let f = dominant_hz(&x, f64::from(SR));
            assert!((f - 440.0).abs() < 1.0, "{wave:?}: {f} Hz");
            let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!((0.9..=1.05).contains(&peak), "{wave:?}: peak {peak}");
        }
    }

    #[test]
    fn polyblep_triangle_matches_naive_at_low_frequency() {
        let a = render(Waveform::Triangle, 100.0, 4_800);
        let b = naive(Waveform::Triangle, 100.0, 4_800);
        let err = a
            .iter()
            .zip(&b)
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(err < 0.01, "{err}");
    }

    #[test]
    fn polyblep_frequency_change_is_phase_continuous() {
        let mut o = PolyBlepOsc::new(Waveform::Sine);
        o.set_frequency(1_000.0, SR);
        let mut x: Vec<f32> = (0..1_000).map(|_| o.next_sample()).collect();
        o.set_frequency(2_000.0, SR);
        x.extend((0..1_000).map(|_| o.next_sample()));
        let bound = 2.0 * std::f32::consts::PI * 2_000.0 / SR;
        assert!(max_step(&x) <= bound * 1.01, "{}", max_step(&x));
    }

    #[test]
    fn polyblep_frequency_is_clamped_to_nyquist() {
        let mut o = PolyBlepOsc::new(Waveform::Saw);
        o.set_frequency(-5.0, SR);
        assert!((0..100).map(|_| o.next_sample()).all(f32::is_finite));
        o.set_frequency(1.0e9, SR);
        assert!(
            (0..100)
                .map(|_| o.next_sample())
                .all(|s| s.is_finite() && s.abs() <= 2.0)
        );
    }
}
