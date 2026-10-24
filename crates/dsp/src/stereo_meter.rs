//! Stereo image statistics: the correlation between the channels, the balance, the mid and side
//! levels and a width figure, from one-pole averages of the powers `L^2`, `R^2` and `L*R`.
//!
//! * **Correlation** `lr / sqrt(ll * rr)` over a short time constant (the `corr_ms` of the
//!   meter): +1 for identical channels (mono), -1 for inverted ones, about 0 for unrelated
//!   signals and for a pair a quarter cycle apart. Silence, and a channel that is silent while
//!   the other is not, read 0 (there is nothing to correlate).
//! * **Levels** use a fixed 300 ms average of the same powers: RMS of each channel, of the mid
//!   `M = (L + R) / 2` and of the side `S = (L - R) / 2`, with `M^2 = (ll + rr + 2 lr) / 4` and
//!   `S^2 = (ll + rr - 2 lr) / 4`.
//! * **Balance** `(R_rms - L_rms) / (R_rms + L_rms)`: -1 is all left, 0 centred, +1 all right.
//! * **Width** `S_rms / (M_rms + S_rms)`: 0 for mono, 0.5 when mid and side are equal (unrelated
//!   channels of equal level), 1 for channels in opposite polarity.
//!
//! Averages are `f64`, deterministic (`exp2_det_f64`), allocation-free after construction, and
//! identical for any block chunking. A non-finite input sample counts as 0.

use crate::exp2_det_f64;

/// The fixed averaging time of the level readouts, ms.
pub const LEVEL_MS: f32 = 300.0;

/// Power below which a channel counts as silent.
const SILENT: f64 = 1e-18;

/// One-pole averaged `L^2`, `R^2` and `L*R`.
#[derive(Debug, Clone, Copy, Default)]
struct Moments {
    ll: f64,
    rr: f64,
    lr: f64,
}

impl Moments {
    #[inline]
    fn push(&mut self, alpha: f64, l: f64, r: f64) {
        self.ll += alpha * (l * l - self.ll);
        self.rr += alpha * (r * r - self.rr);
        self.lr += alpha * (l * r - self.lr);
    }
}

/// `1 - exp(-1 / (ms * 0.001 * sample_rate))`: the one-pole step per sample.
fn alpha_for(ms: f32, sample_rate: f32) -> f64 {
    let n = f64::from(ms.max(0.0)) * 0.001 * f64::from(sample_rate);
    if n < 1.0 {
        1.0
    } else {
        1.0 - exp2_det_f64(-std::f64::consts::LOG2_E / n)
    }
}

/// The stereo statistics of one signal (module docs).
#[derive(Debug, Clone)]
pub struct StereoMeter {
    sample_rate: f32,
    a_corr: f64,
    a_level: f64,
    corr: Moments,
    level: Moments,
}

impl StereoMeter {
    /// A meter at `sample_rate` Hz whose correlation averages over `corr_ms`.
    pub fn new(sample_rate: f32, corr_ms: f32) -> Self {
        Self {
            sample_rate,
            a_corr: alpha_for(corr_ms, sample_rate),
            a_level: alpha_for(LEVEL_MS, sample_rate),
            corr: Moments::default(),
            level: Moments::default(),
        }
    }

    /// Changes the correlation time constant (the state is kept).
    pub fn set_corr_ms(&mut self, corr_ms: f32) {
        self.a_corr = alpha_for(corr_ms, self.sample_rate);
    }

    /// Forgets the averages.
    pub fn reset(&mut self) {
        self.corr = Moments::default();
        self.level = Moments::default();
    }

    /// Feeds frames (the shorter of the two slices).
    pub fn process(&mut self, left: &[f32], right: &[f32]) {
        for (&l, &r) in left.iter().zip(right) {
            let l = if l.is_finite() { f64::from(l) } else { 0.0 };
            let r = if r.is_finite() { f64::from(r) } else { 0.0 };
            self.corr.push(self.a_corr, l, r);
            self.level.push(self.a_level, l, r);
        }
    }

    /// The correlation, -1..=1 (0 for silence or one silent channel).
    pub fn correlation(&self) -> f32 {
        let m = &self.corr;
        if m.ll < SILENT || m.rr < SILENT {
            return 0.0;
        }
        (m.lr / (m.ll * m.rr).sqrt()).clamp(-1.0, 1.0) as f32
    }

    /// RMS of the left channel (linear).
    pub fn rms_left(&self) -> f32 {
        self.level.ll.max(0.0).sqrt() as f32
    }

    /// RMS of the right channel (linear).
    pub fn rms_right(&self) -> f32 {
        self.level.rr.max(0.0).sqrt() as f32
    }

    /// RMS of the mid `(L + R) / 2` (linear).
    pub fn mid_rms(&self) -> f32 {
        let m = &self.level;
        (0.25 * (m.ll + m.rr + 2.0 * m.lr)).max(0.0).sqrt() as f32
    }

    /// RMS of the side `(L - R) / 2` (linear).
    pub fn side_rms(&self) -> f32 {
        let m = &self.level;
        (0.25 * (m.ll + m.rr - 2.0 * m.lr)).max(0.0).sqrt() as f32
    }

    /// The balance, -1 (left) to +1 (right); 0 for silence.
    pub fn balance(&self) -> f32 {
        let (l, r) = (self.rms_left(), self.rms_right());
        if l + r <= 1e-9 {
            0.0
        } else {
            (r - l) / (r + l)
        }
    }

    /// The width, 0 (mono) to 1 (opposite polarity); 0 for silence.
    pub fn width(&self) -> f32 {
        let (m, s) = (self.mid_rms(), self.side_rms());
        if m + s <= 1e-9 { 0.0 } else { s / (m + s) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{noise, sine};

    const SR: f32 = 48_000.0;

    fn feed(meter: &mut StereoMeter, l: &[f32], r: &[f32]) {
        for (a, b) in l.chunks(64).zip(r.chunks(64)) {
            meter.process(a, b);
        }
    }

    fn near(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn identical_channels_correlate_at_plus_one_and_are_mono() {
        let x = sine(440.0, f64::from(SR), 144_000, 0.5);
        let mut m = StereoMeter::new(SR, 300.0);
        feed(&mut m, &x, &x);
        assert!(near(m.correlation(), 1.0, 1e-5), "{}", m.correlation());
        assert!(near(m.balance(), 0.0, 1e-6));
        assert!(m.width() < 1e-3, "mono has no side: {}", m.width());
        assert!(near(m.mid_rms(), 0.5 / 2f32.sqrt(), 1e-3));
        assert!(m.side_rms() < 1e-4);
    }

    #[test]
    fn inverted_channels_correlate_at_minus_one_and_are_all_side() {
        let x = sine(440.0, f64::from(SR), 48_000, 0.5);
        let inv: Vec<f32> = x.iter().map(|v| -v).collect();
        let mut m = StereoMeter::new(SR, 300.0);
        feed(&mut m, &x, &inv);
        assert!(near(m.correlation(), -1.0, 1e-5), "{}", m.correlation());
        assert!(near(m.width(), 1.0, 1e-3), "{}", m.width());
        assert!(m.mid_rms() < 1e-4);
    }

    #[test]
    fn unrelated_noise_and_a_quarter_cycle_pair_correlate_near_zero() {
        let n = 96_000;
        let (a, b) = (noise(1, n, 0.5), noise(2, n, 0.5));
        let mut m = StereoMeter::new(SR, 300.0);
        feed(&mut m, &a, &b);
        assert!(m.correlation().abs() < 0.06, "{}", m.correlation());
        // Equal level and unrelated: mid and side are equal, so the width is about one half.
        assert!(near(m.width(), 0.5, 0.05), "{}", m.width());
        // A sine against its quarter-cycle shift.
        let l = sine(1000.0, f64::from(SR), 48_000, 0.5);
        let r: Vec<f32> = (0..48_000)
            .map(|i| (0.5 * (std::f64::consts::TAU * 1000.0 * i as f64 / 48_000.0).cos()) as f32)
            .collect();
        let mut q = StereoMeter::new(SR, 300.0);
        feed(&mut q, &l, &r);
        assert!(q.correlation().abs() < 0.01, "{}", q.correlation());
    }

    #[test]
    fn mid_and_side_follow_the_matrix() {
        // L = 0.8, R = 0.2 (DC): M = 0.5, S = 0.3.
        let mut m = StereoMeter::new(SR, 100.0);
        feed(&mut m, &vec![0.8; 240_000], &vec![0.2; 240_000]);
        assert!(near(m.mid_rms(), 0.5, 1e-4), "{}", m.mid_rms());
        assert!(near(m.side_rms(), 0.3, 1e-4), "{}", m.side_rms());
        assert!(near(m.rms_left(), 0.8, 1e-4));
        assert!(near(m.balance(), -0.6, 1e-4), "{}", m.balance());
        assert!(near(m.width(), 0.3 / 0.8, 1e-4));
    }

    #[test]
    fn silence_and_a_silent_channel_read_zero() {
        let mut m = StereoMeter::new(SR, 300.0);
        assert!(near(m.correlation(), 0.0, 0.0));
        feed(&mut m, &vec![0.0; 4800], &vec![0.0; 4800]);
        assert!(near(m.balance(), 0.0, 0.0) && near(m.width(), 0.0, 0.0));
        let x = noise(3, 48_000, 0.5);
        feed(&mut m, &x, &vec![0.0; 48_000]);
        assert!(near(m.correlation(), 0.0, 0.0), "one silent channel");
        assert!(near(m.balance(), -1.0, 1e-3));
    }

    #[test]
    fn the_result_does_not_depend_on_the_chunking_and_survives_non_finite_input() {
        let a = noise(4, 10_000, 0.5);
        let b = noise(5, 10_000, 0.5);
        let mut one = StereoMeter::new(SR, 200.0);
        one.process(&a, &b);
        let mut many = StereoMeter::new(SR, 200.0);
        feed(&mut many, &a, &b);
        assert_eq!(one.correlation().to_bits(), many.correlation().to_bits());
        let mut bad = StereoMeter::new(SR, 200.0);
        bad.process(
            &[f32::NAN, f32::INFINITY, 0.5],
            &[0.5, f32::NEG_INFINITY, 0.5],
        );
        assert!(bad.correlation().is_finite() && bad.width().is_finite());
        bad.reset();
        assert!(near(bad.rms_left(), 0.0, 0.0));
    }

    #[test]
    fn the_correlation_time_constant_can_change_while_running() {
        let x = sine(300.0, f64::from(SR), 4_800, 0.5);
        let mut m = StereoMeter::new(SR, 2000.0);
        feed(&mut m, &x, &x);
        m.set_corr_ms(5.0);
        feed(&mut m, &x, &x);
        assert!(m.correlation() > 0.99);
    }
}
