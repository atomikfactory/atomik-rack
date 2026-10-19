//! Feed-forward dynamics: a compressor and a gate/expander. Both compute a *gain* from a
//! stereo-linked side-chain; the caller multiplies its audio by that gain. Textbook designs
//! (log-domain gain computer, branching one-pole ballistics; cf. Giannoulis, Massberg & Reiss,
//! "Digital Dynamic Range Compressor Design — A Tutorial and Analysis", JAES 2012).
//!
//! ## Compressor
//! * **Detector.** Stereo-linked.
//!   * *Peak*: `|x| = max(|l|, |r|)` per sample.
//!   * *RMS*: one-pole mean square of `(l² + r²)/2` with a [`RMS_WINDOW_MS`] time constant,
//!     **sine-calibrated**: the level is `√(2·ms)`, so a sine of peak `A` reads `A` in both
//!     modes (a 0 dBFS sine reads 0 dB).
//! * **Static curve** (gain computer, dB domain, threshold `T`, ratio `R`, fixed soft knee
//!   `W =` [`KNEE_DB`] centred on `T`):
//!   * `x < T − W/2`: no reduction;
//!   * `|x − T| ≤ W/2`: `gr = (1 − 1/R)·(x − T + W/2)² / (2W)`;
//!   * `x > T + W/2`: `gr = (1 − 1/R)·(x − T)` (i.e. output `T + (x − T)/R`).
//! * **Ballistics** ("smooth decoupled peak" form, on the gain reduction `c` in dB):
//!   `y₁ = max(c, αR·y₁ + (1−αR)·c)`, then `y = αA·y + (1−αA)·y₁`, with
//!   `α = e^(−1/(τ·sr))`. The release stage holds peaks, the attack stage smooths onsets.
//!   Steady-state error on a 1 kHz sine is under 0.05 dB for releases ≥ 50 ms.
//!
//! ## Gate / expander
//! * **Detector:** peak follower, instant attack, [`GATE_DETECTOR_MS`] release (stereo-linked).
//! * **Open** while the level ≥ threshold. After the level falls below, the gate stays open
//!   for `hold` ms, then moves to its closed gain.
//! * **Gate mode:** closed gain = `−range` dB.
//! * **Expander mode:** downward expansion with a fixed ratio of [`EXPANDER_RATIO`]:1 below
//!   the threshold (`gain = (level − T)·(R − 1)` dB), floored at `−range` dB.
//! * **Ballistics:** the gain in dB follows a one-pole with the *attack* time constant when
//!   opening and the *release* time constant when closing.

/// Fixed compressor soft-knee width in dB (centred on the threshold).
pub const KNEE_DB: f32 = 6.0;
/// RMS detector time constant.
pub const RMS_WINDOW_MS: f32 = 10.0;
/// Gate detector release time constant.
pub const GATE_DETECTOR_MS: f32 = 5.0;
/// Fixed expansion ratio of the gate's expander mode.
pub const EXPANDER_RATIO: f32 = 2.0;

/// Compressor level detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detector {
    /// Instantaneous stereo-linked peak.
    Peak,
    /// Sine-calibrated RMS over [`RMS_WINDOW_MS`].
    Rms,
}

/// Static compressor curve: gain reduction in dB (>= 0) for a detected level in dB, with the
/// fixed [`KNEE_DB`] soft knee.
pub fn static_gain_reduction_db(level_db: f32, threshold_db: f32, ratio: f32) -> f32 {
    static_gain_reduction_knee_db(level_db, threshold_db, ratio, KNEE_DB)
}

/// Static compressor curve with an explicit knee width `knee_db` (>= 0, centred on the
/// threshold; 0 is a hard knee). `ratio` >= 1, `f32::INFINITY` is a limiter. Same formulas as
/// [`static_gain_reduction_db`].
pub fn static_gain_reduction_knee_db(
    level_db: f32,
    threshold_db: f32,
    ratio: f32,
    knee_db: f32,
) -> f32 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    let over = level_db - threshold_db;
    let knee = knee_db.max(0.0);
    let half = knee / 2.0;
    if over.is_nan() || over <= -half {
        0.0
    } else if over < half {
        let k = over + half;
        slope * k * k / (2.0 * knee)
    } else {
        slope * over
    }
}

/// One-pole coefficient `e^(−1/(τ·sr))` (0 for `τ ≤ 0`).
#[inline]
pub(crate) fn coeff(time_ms: f32, sample_rate: f32) -> f32 {
    let samples = time_ms * 0.001 * sample_rate;
    if samples > 0.0 {
        (-1.0 / samples).exp()
    } else {
        0.0
    }
}

/// Lowest level the detectors report (−200 dB), so `log10` never sees 0.
const LEVEL_FLOOR: f32 = 1e-10;

#[inline]
fn level_db(level: f32) -> f32 {
    20.0 * level.max(LEVEL_FLOOR).log10()
}

#[inline]
fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v } else { fallback }
}

/// Feed-forward compressor (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct Compressor {
    sample_rate: f32,
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    detector: Detector,
    alpha_attack: f32,
    alpha_release: f32,
    alpha_rms: f32,
    /// Mean square (RMS mode).
    ms: f32,
    /// Release-stage (peak-holding) gain reduction, dB.
    y1: f32,
    /// Smoothed gain reduction, dB.
    gr: f32,
}

impl Compressor {
    /// Threshold 0 dB, ratio 4, attack 10 ms, release 200 ms, RMS, at `sample_rate`.
    pub fn new(sample_rate: f32) -> Self {
        let mut c = Self {
            sample_rate,
            threshold_db: 0.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 200.0,
            detector: Detector::Rms,
            alpha_attack: 0.0,
            alpha_release: 0.0,
            alpha_rms: 0.0,
            ms: 0.0,
            y1: 0.0,
            gr: 0.0,
        };
        c.set_sample_rate(sample_rate);
        c
    }

    /// Changes the sample rate (recomputes the time constants) and resets.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.alpha_attack = coeff(self.attack_ms, sample_rate);
        self.alpha_release = coeff(self.release_ms, sample_rate);
        self.alpha_rms = coeff(RMS_WINDOW_MS, sample_rate);
        self.reset();
    }

    /// Threshold in dB.
    pub fn set_threshold_db(&mut self, db: f32) {
        self.threshold_db = finite_or(db, 0.0);
    }

    /// Ratio (≥ 1).
    pub fn set_ratio(&mut self, ratio: f32) {
        self.ratio = finite_or(ratio, 1.0).max(1.0);
    }

    /// Attack time constant in ms.
    pub fn set_attack_ms(&mut self, ms: f32) {
        self.attack_ms = finite_or(ms, 0.0).max(0.0);
        self.alpha_attack = coeff(self.attack_ms, self.sample_rate);
    }

    /// Release time constant in ms.
    pub fn set_release_ms(&mut self, ms: f32) {
        self.release_ms = finite_or(ms, 0.0).max(0.0);
        self.alpha_release = coeff(self.release_ms, self.sample_rate);
    }

    /// Detector mode.
    pub fn set_detector(&mut self, d: Detector) {
        self.detector = d;
    }

    /// Clears the detector and the gain reduction.
    pub fn reset(&mut self) {
        self.ms = 0.0;
        self.y1 = 0.0;
        self.gr = 0.0;
    }

    /// Advances one sample with the side-chain `(l, r)` and returns the linear gain to apply.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> f32 {
        let (l, r) = (finite_or(l, 0.0), finite_or(r, 0.0));
        let level = match self.detector {
            Detector::Peak => l.abs().max(r.abs()),
            Detector::Rms => {
                let x2 = 0.5 * (l * l + r * r);
                self.ms = x2 + self.alpha_rms * (self.ms - x2);
                if self.ms < 1e-30 {
                    self.ms = 0.0;
                }
                (2.0 * self.ms).sqrt()
            }
        };
        let c = static_gain_reduction_db(level_db(level), self.threshold_db, self.ratio);
        self.y1 = c.max(c + self.alpha_release * (self.y1 - c));
        self.gr = self.y1 + self.alpha_attack * (self.gr - self.y1);
        if self.gr < 1e-9 {
            self.gr = 0.0;
        }
        10f32.powf(-self.gr / 20.0)
    }

    /// Current gain reduction in dB (≥ 0).
    pub fn gain_reduction_db(&self) -> f32 {
        self.gr
    }
}

/// Gate / downward expander (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    sample_rate: f32,
    threshold_db: f32,
    range_db: f32,
    hold_ms: f32,
    attack_ms: f32,
    release_ms: f32,
    expander: bool,
    alpha_attack: f32,
    alpha_release: f32,
    alpha_detector: f32,
    hold_samples: u32,
    env: f32,
    hold_left: u32,
    /// Current gain in dB (≤ 0).
    gain_db: f32,
}

impl Gate {
    /// Threshold −80 dB, range 80 dB, hold 10 ms, attack 1 ms, release 100 ms, gate mode.
    pub fn new(sample_rate: f32) -> Self {
        let mut g = Self {
            sample_rate,
            threshold_db: -80.0,
            range_db: 80.0,
            hold_ms: 10.0,
            attack_ms: 1.0,
            release_ms: 100.0,
            expander: false,
            alpha_attack: 0.0,
            alpha_release: 0.0,
            alpha_detector: 0.0,
            hold_samples: 0,
            env: 0.0,
            hold_left: 0,
            gain_db: 0.0,
        };
        g.set_sample_rate(sample_rate);
        g
    }

    /// Changes the sample rate and resets.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.alpha_attack = coeff(self.attack_ms, sample_rate);
        self.alpha_release = coeff(self.release_ms, sample_rate);
        self.alpha_detector = coeff(GATE_DETECTOR_MS, sample_rate);
        self.set_hold_ms(self.hold_ms);
        self.reset();
    }

    /// Threshold in dB.
    pub fn set_threshold_db(&mut self, db: f32) {
        self.threshold_db = finite_or(db, -80.0);
    }

    /// Maximum attenuation in dB (≥ 0) when closed.
    pub fn set_range_db(&mut self, db: f32) {
        self.range_db = finite_or(db, 0.0).clamp(0.0, 200.0);
    }

    /// Hold time in ms.
    pub fn set_hold_ms(&mut self, ms: f32) {
        self.hold_ms = finite_or(ms, 0.0).clamp(0.0, 10_000.0);
        self.hold_samples = (self.hold_ms * 0.001 * self.sample_rate).round() as u32;
    }

    /// Opening time constant in ms.
    pub fn set_attack_ms(&mut self, ms: f32) {
        self.attack_ms = finite_or(ms, 0.0).max(0.0);
        self.alpha_attack = coeff(self.attack_ms, self.sample_rate);
    }

    /// Closing time constant in ms.
    pub fn set_release_ms(&mut self, ms: f32) {
        self.release_ms = finite_or(ms, 0.0).max(0.0);
        self.alpha_release = coeff(self.release_ms, self.sample_rate);
    }

    /// Expander (true) or gate (false) mode.
    pub fn set_expander(&mut self, on: bool) {
        self.expander = on;
    }

    /// Opens the gate and clears the detector.
    pub fn reset(&mut self) {
        self.env = 0.0;
        self.hold_left = 0;
        self.gain_db = 0.0;
    }

    /// Advances one sample with the side-chain `(l, r)` and returns the linear gain to apply.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> f32 {
        let x = finite_or(l, 0.0).abs().max(finite_or(r, 0.0).abs());
        self.env = x.max(self.alpha_detector * self.env);
        if self.env < 1e-30 {
            self.env = 0.0;
        }
        let lvl = level_db(self.env);
        let target = if lvl >= self.threshold_db {
            self.hold_left = self.hold_samples;
            0.0
        } else if self.hold_left > 0 {
            self.hold_left -= 1;
            0.0
        } else if self.expander {
            ((lvl - self.threshold_db) * (EXPANDER_RATIO - 1.0)).max(-self.range_db)
        } else {
            -self.range_db
        };
        let a = if target > self.gain_db {
            self.alpha_attack
        } else {
            self.alpha_release
        };
        self.gain_db = target + a * (self.gain_db - target);
        if (self.gain_db - target).abs() < 1e-6 {
            self.gain_db = target;
        }
        10f32.powf(self.gain_db / 20.0)
    }

    /// Current attenuation in dB (≥ 0).
    pub fn gain_reduction_db(&self) -> f32 {
        -self.gain_db
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn db(x: f32) -> f32 {
        20.0 * x.log10()
    }

    /// Feeds a 1 kHz sine of `level_db` peak for 1.5 s; returns output peak − input peak (dB)
    /// over the last 100 ms.
    fn steady_gain_db(c: &mut Compressor, level_db: f32) -> f32 {
        c.reset();
        let a = 10f32.powf(level_db / 20.0);
        let n = (1.5 * SR) as usize;
        let mut peak = 0.0f32;
        for i in 0..n {
            let x = a * (std::f32::consts::TAU * 1_000.0 * i as f32 / SR).sin();
            let y = x * c.process(x, x);
            if i >= n - 4_800 {
                peak = peak.max(y.abs());
            }
        }
        db(peak) - level_db
    }

    #[test]
    fn static_curve_matches_formula_and_knee() {
        let (t, r) = (-20.0, 4.0);
        assert!(static_gain_reduction_db(-40.0, t, r).abs() < 1e-6);
        assert!(static_gain_reduction_db(t - KNEE_DB / 2.0, t, r).abs() < 1e-6);
        // Above the knee: output = T + (x − T)/R.
        let x = -8.0;
        assert!((static_gain_reduction_db(x, t, r) - (x - (t + (x - t) / r))).abs() < 1e-5);
        // At the threshold (knee centre): (1 − 1/R)·W/8.
        let want = (1.0 - 1.0 / r) * KNEE_DB / 8.0;
        assert!((static_gain_reduction_db(t, t, r) - want).abs() < 1e-5);
        // Continuous at the knee edge.
        let edge = t + KNEE_DB / 2.0;
        let a = static_gain_reduction_db(edge - 1e-3, t, r);
        let b = static_gain_reduction_db(edge + 1e-3, t, r);
        assert!((a - b).abs() < 2e-3, "slope·2e-3 at most");
        // Ratio 1 never reduces.
        assert!(static_gain_reduction_db(0.0, -60.0, 1.0).abs() < 1e-6);
    }

    #[test]
    fn compressor_steady_sine_follows_static_curve_peak_and_rms() {
        for det in [Detector::Peak, Detector::Rms] {
            for (t, r) in [(-20.0f32, 4.0f32), (-30.0, 10.0), (-10.0, 2.0)] {
                let mut c = Compressor::new(SR);
                c.set_detector(det);
                c.set_threshold_db(t);
                c.set_ratio(r);
                c.set_release_ms(200.0);
                for level in (-60..=0).step_by(4) {
                    let level = level as f32;
                    if (level - t).abs() <= KNEE_DB / 2.0 {
                        continue; // knee region checked separately
                    }
                    let got = steady_gain_db(&mut c, level);
                    let want = -static_gain_reduction_db(level, t, r);
                    assert!(
                        (got - want).abs() < 0.2,
                        "{det:?} T{t} R{r} at {level} dB: {got:.3} vs {want:.3}"
                    );
                }
            }
        }
    }

    #[test]
    fn compressor_attack_and_release_are_time_constants() {
        let mut c = Compressor::new(SR);
        c.set_detector(Detector::Peak);
        c.set_threshold_db(-20.0);
        c.set_ratio(20.0);
        c.set_attack_ms(10.0);
        c.set_release_ms(100.0);
        // A 0 dBFS step: after one attack time constant ~63 % of the final reduction.
        for _ in 0..480 {
            c.process(1.0, 1.0);
        }
        let target = static_gain_reduction_db(0.0, -20.0, 20.0);
        let gr = c.gain_reduction_db();
        assert!(
            (gr / target - 0.632).abs() < 0.03,
            "{gr} of {target} after τA"
        );
        for _ in 0..48_000 {
            c.process(1.0, 1.0);
        }
        assert!((c.gain_reduction_db() - target).abs() < 1e-3);
        // Release: drop to silence, the reduction decays.
        for _ in 0..4_800 {
            c.process(0.0, 0.0);
        }
        let gr = c.gain_reduction_db();
        assert!(gr < 0.5 * target && gr > 0.1, "after 100 ms release: {gr}");
        c.reset();
        assert!(c.gain_reduction_db().abs() < 1e-9);
    }

    #[test]
    fn compressor_is_stereo_linked() {
        let mut c = Compressor::new(SR);
        c.set_detector(Detector::Peak);
        c.set_threshold_db(-20.0);
        c.set_attack_ms(0.0);
        let g_left_only = c.process(0.0, 1.0);
        c.reset();
        let g_both = c.process(1.0, 1.0);
        assert!((g_left_only - g_both).abs() < 1e-6);
        assert!(g_both < 1.0);
    }

    fn gate() -> Gate {
        let mut g = Gate::new(SR);
        g.set_threshold_db(-30.0);
        g.set_range_db(60.0);
        g.set_hold_ms(50.0);
        g.set_attack_ms(0.1);
        g.set_release_ms(10.0);
        g
    }

    #[test]
    fn gate_range_attenuates_below_threshold_only() {
        let mut g = gate();
        // Above threshold: unity.
        let mut gain = 0.0;
        for _ in 0..9_600 {
            gain = g.process(0.5, 0.5);
        }
        assert!(db(gain).abs() < 0.01, "open: {}", db(gain));
        // Far below threshold: −range.
        for _ in 0..48_000 {
            gain = g.process(0.001, 0.001);
        }
        assert!((db(gain) + 60.0).abs() < 0.1, "closed: {}", db(gain));
        assert!((g.gain_reduction_db() - 60.0).abs() < 0.1);
        // Re-opens fast.
        for _ in 0..96 {
            gain = g.process(0.5, 0.5);
        }
        assert!(db(gain).abs() < 0.1, "re-opened: {}", db(gain));
    }

    #[test]
    fn gate_holds_before_releasing() {
        let mut g = gate();
        for _ in 0..4_800 {
            g.process(0.5, 0.5);
        }
        // Detector falls from −6 dB to −30 dB in 5 ms·ln(0.5/0.0316) ≈ 13.8 ms, then 50 ms hold.
        let gains: Vec<f32> = (0..9_600).map(|_| g.process(0.001, 0.001)).collect();
        let ms = |t: f32| (t * 48.0) as usize;
        assert!(db(gains[ms(60.0)]).abs() < 0.01, "held at 60 ms");
        assert!(db(gains[ms(75.0)]) < -6.0, "releasing at 75 ms");
        assert!(
            (db(gains[ms(199.0)]) + 60.0).abs() < 0.5,
            "closed at 199 ms"
        );
        // Hold 0: releases right after the detector falls.
        let mut g = gate();
        g.set_hold_ms(0.0);
        for _ in 0..4_800 {
            g.process(0.5, 0.5);
        }
        let gains: Vec<f32> = (0..4_800).map(|_| g.process(0.001, 0.001)).collect();
        assert!(db(gains[ms(30.0)]) < -6.0, "no hold");
    }

    #[test]
    fn expander_follows_ratio_down_to_range() {
        let mut g = gate();
        g.set_expander(true);
        let mut gain = 0.0;
        for _ in 0..48_000 {
            gain = g.process(0.01, 0.01); // −40 dB, 10 dB under the threshold
        }
        let want = -10.0 * (EXPANDER_RATIO - 1.0);
        assert!((db(gain) - want).abs() < 0.1, "{} vs {want}", db(gain));
        for _ in 0..48_000 {
            gain = g.process(1e-6, 1e-6); // −120 dB: floored at −range
        }
        assert!((db(gain) + 60.0).abs() < 0.1, "{}", db(gain));
    }
}
