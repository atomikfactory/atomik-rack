//! ADSR envelope with exponential-ish (one-pole towards an overshooting target) segments.
//!
//! Curves (all textbook analog-style):
//! * **Attack:** one-pole rise towards `1 + 0.3`, cut off at 1.0, so it reaches 1.0 in exactly
//!   the attack time from 0 (a rounded, slightly convex rise).
//! * **Decay:** one-pole fall towards `sustain − 0.001`, reaching `sustain` in the decay time
//!   (measured from 1.0). Then **Sustain** follows later sustain changes with a 5 ms one-pole.
//! * **Release:** one-pole fall towards `−0.001`, reaching 0 in the release time when starting
//!   from 1.0 (shorter from lower levels: the rate is fixed, as on analog envelopes). Then Idle.
//! * **Steal fade:** a linear 2 ms ramp from the current level to 0, then Idle.
//!
//! Every transition starts from the current level, so gate-on during any stage (retrigger) is
//! continuous. Segments end by clamping at their target, so no denormals are produced.

/// The 2 ms steal fade duration.
pub const STEAL_FADE_MS: f32 = 2.0;

const ATTACK_OVERSHOOT: f32 = 0.3;
const FALL_UNDERSHOOT: f32 = 0.001;
const SUSTAIN_FOLLOW_MS: f32 = 5.0;

/// Envelope stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdsrStage {
    /// Silent, level 0.
    Idle,
    /// Rising to 1.
    Attack,
    /// Falling to sustain.
    Decay,
    /// Holding (following) the sustain level.
    Sustain,
    /// Falling to 0 after gate-off.
    Release,
    /// Linear 2 ms fade to 0 (voice stealing).
    Fade,
}

/// ADSR envelope generator. Times in milliseconds, levels 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adsr {
    stage: AdsrStage,
    level: f32,
    sample_rate: f32,
    attack_ms: f32,
    decay_ms: f32,
    sustain: f32,
    release_ms: f32,
    attack_coef: f32,
    decay_coef: f32,
    release_coef: f32,
    follow_coef: f32,
    fade_step: f32,
    fade_remaining: u32,
}

impl Adsr {
    /// An idle envelope at `sample_rate` with A 2 ms, D 200 ms, S 0.7, R 300 ms.
    pub fn new(sample_rate: f32) -> Self {
        let mut e = Self {
            stage: AdsrStage::Idle,
            level: 0.0,
            sample_rate,
            attack_ms: 2.0,
            decay_ms: 200.0,
            sustain: 0.7,
            release_ms: 300.0,
            attack_coef: 0.0,
            decay_coef: 0.0,
            release_coef: 0.0,
            follow_coef: 0.0,
            fade_step: 0.0,
            fade_remaining: 0,
        };
        e.update();
        e
    }

    fn samples(&self, ms: f32) -> f32 {
        (ms * 0.001 * self.sample_rate).max(1.0)
    }

    /// Recomputes the per-sample coefficients (non-per-sample work: uses `powf`).
    fn update(&mut self) {
        let a = self.samples(self.attack_ms);
        self.attack_coef = (ATTACK_OVERSHOOT / (1.0 + ATTACK_OVERSHOOT)).powf(1.0 / a);
        let d = self.samples(self.decay_ms);
        self.decay_coef = (FALL_UNDERSHOOT / (1.0 - self.sustain + FALL_UNDERSHOOT)).powf(1.0 / d);
        let r = self.samples(self.release_ms);
        self.release_coef = (FALL_UNDERSHOOT / (1.0 + FALL_UNDERSHOOT)).powf(1.0 / r);
        self.follow_coef = (-1.0 / self.samples(SUSTAIN_FOLLOW_MS)).exp();
    }

    /// Changes the sample rate (keeps times in ms).
    #[inline]
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.update();
    }

    /// Attack time in ms.
    #[inline]
    pub fn set_attack_ms(&mut self, ms: f32) {
        self.attack_ms = ms;
        self.update();
    }

    /// Decay time in ms (from 1.0 down to sustain).
    #[inline]
    pub fn set_decay_ms(&mut self, ms: f32) {
        self.decay_ms = ms;
        self.update();
    }

    /// Sustain level `0..=1` (clamped).
    #[inline]
    pub fn set_sustain(&mut self, level: f32) {
        self.sustain = level.clamp(0.0, 1.0);
        self.update();
    }

    /// Release time in ms (from 1.0 down to 0).
    #[inline]
    pub fn set_release_ms(&mut self, ms: f32) {
        self.release_ms = ms;
        self.update();
    }

    /// Starts (or retriggers) the attack from the current level.
    #[inline]
    pub fn gate_on(&mut self) {
        self.stage = AdsrStage::Attack;
    }

    /// Starts the release from the current level (no-op when idle or fading).
    #[inline]
    pub fn gate_off(&mut self) {
        if !matches!(self.stage, AdsrStage::Idle | AdsrStage::Fade) {
            self.stage = AdsrStage::Release;
        }
    }

    /// Starts the 2 ms linear steal fade from the current level.
    #[inline]
    pub fn steal_fade(&mut self) {
        if self.stage == AdsrStage::Idle {
            return;
        }
        let n = self.samples(STEAL_FADE_MS).round();
        self.fade_remaining = n as u32;
        self.fade_step = self.level / n;
        self.stage = AdsrStage::Fade;
    }

    /// Returns to Idle at level 0 immediately.
    #[inline]
    pub fn reset(&mut self) {
        self.stage = AdsrStage::Idle;
        self.level = 0.0;
    }

    /// Advances one sample and returns the level.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        match self.stage {
            AdsrStage::Idle => {}
            AdsrStage::Attack => {
                let target = 1.0 + ATTACK_OVERSHOOT;
                self.level = target + (self.level - target) * self.attack_coef;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = AdsrStage::Decay;
                }
            }
            AdsrStage::Decay => {
                let target = self.sustain - FALL_UNDERSHOOT;
                self.level = target + (self.level - target) * self.decay_coef;
                if self.level <= self.sustain {
                    // No snap: Sustain follows the sustain level smoothly from here.
                    self.stage = AdsrStage::Sustain;
                }
            }
            AdsrStage::Sustain => {
                self.level = self.sustain + (self.level - self.sustain) * self.follow_coef;
                if (self.level - self.sustain).abs() < 1e-7 {
                    self.level = self.sustain;
                }
            }
            AdsrStage::Release => {
                let target = -FALL_UNDERSHOOT;
                self.level = target + (self.level - target) * self.release_coef;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = AdsrStage::Idle;
                }
            }
            AdsrStage::Fade => {
                self.fade_remaining = self.fade_remaining.saturating_sub(1);
                if self.fade_remaining == 0 {
                    self.level = 0.0;
                    self.stage = AdsrStage::Idle;
                } else {
                    self.level = (self.level - self.fade_step).max(0.0);
                }
            }
        }
        self.level
    }

    /// Current stage.
    #[inline]
    pub fn stage(&self) -> AdsrStage {
        self.stage
    }

    /// Current level.
    #[inline]
    pub fn level(&self) -> f32 {
        self.level
    }

    /// True unless Idle.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.stage != AdsrStage::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn env(a: f32, d: f32, s: f32, r: f32) -> Adsr {
        let mut e = Adsr::new(SR);
        e.set_attack_ms(a);
        e.set_decay_ms(d);
        e.set_sustain(s);
        e.set_release_ms(r);
        e
    }

    /// Runs until the stage changes; returns (samples taken, levels).
    fn run_until_change(e: &mut Adsr, max: usize) -> (usize, Vec<f32>) {
        let start = e.stage();
        let mut levels = Vec::new();
        for i in 1..=max {
            levels.push(e.next_sample());
            if e.stage() != start {
                return (i, levels);
            }
        }
        (max, levels)
    }

    #[test]
    fn adsr_stage_transitions() {
        let mut e = env(10.0, 20.0, 0.5, 30.0);
        assert_eq!(e.stage(), AdsrStage::Idle);
        e.gate_on();
        assert_eq!(e.stage(), AdsrStage::Attack);
        let (n, lv) = run_until_change(&mut e, 10_000);
        assert!(n.abs_diff(480) <= 2, "attack took {n}");
        assert_eq!(e.stage(), AdsrStage::Decay);
        assert!((e.level() - 1.0).abs() < 1e-6);
        assert!(lv.windows(2).all(|w| w[1] >= w[0]), "attack rises");
        assert!(
            lv[239] > 0.5,
            "attack is exponential-ish (convex): {}",
            lv[239]
        );
        let (n, lv) = run_until_change(&mut e, 10_000);
        assert!(n.abs_diff(960) <= 3, "decay took {n}");
        assert_eq!(e.stage(), AdsrStage::Sustain);
        assert!(lv.windows(2).all(|w| w[1] <= w[0]), "decay falls");
        for _ in 0..1_000 {
            e.next_sample();
        }
        assert!((e.level() - 0.5).abs() < 1e-3);
        assert_eq!(e.stage(), AdsrStage::Sustain);
        e.gate_off();
        assert_eq!(e.stage(), AdsrStage::Release);
        let (n, _) = run_until_change(&mut e, 10_000);
        assert!(n <= 1_441, "release from 0.5 took {n}");
        assert_eq!(e.stage(), AdsrStage::Idle);
        assert!(!e.is_active());
    }

    #[test]
    fn adsr_release_reaches_zero() {
        let mut e = env(0.5, 1.0, 1.0, 50.0);
        e.gate_on();
        for _ in 0..1_000 {
            e.next_sample();
        }
        assert!((e.level() - 1.0).abs() < 1e-6);
        e.gate_off();
        let (n, lv) = run_until_change(&mut e, 100_000);
        assert!(n.abs_diff(2_400) <= 2, "release took {n}");
        assert!(lv.windows(2).all(|w| w[1] <= w[0]));
        assert_eq!(e.level().to_bits(), 0.0f32.to_bits());
        assert_eq!(e.next_sample().to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn adsr_retrigger_is_continuous() {
        let mut e = env(10.0, 50.0, 0.6, 100.0);
        let mut lv = Vec::new();
        e.gate_on();
        lv.extend((0..3_000).map(|_| e.next_sample()));
        e.gate_off();
        lv.extend((0..1_000).map(|_| e.next_sample()));
        let before = e.level();
        assert!(before > 0.05, "still releasing");
        e.gate_on(); // retrigger mid-release
        assert_eq!(e.stage(), AdsrStage::Attack);
        let after = e.next_sample();
        assert!((after - before).abs() < 0.01, "{before} -> {after}");
        lv.push(after);
        lv.extend((0..3_000).map(|_| e.next_sample()));
        e.gate_on(); // retrigger during sustain
        lv.extend((0..3_000).map(|_| e.next_sample()));
        let max_step = lv
            .windows(2)
            .fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(max_step < 0.01, "max step {max_step}");
    }

    #[test]
    fn adsr_sustain_change_is_continuous() {
        let mut e = env(1.0, 1.0, 0.8, 10.0);
        e.gate_on();
        for _ in 0..2_000 {
            e.next_sample();
        }
        e.set_sustain(0.2);
        let mut prev = e.level();
        for _ in 0..5_000 {
            let v = e.next_sample();
            assert!((v - prev).abs() < 0.01);
            prev = v;
        }
        assert!((prev - 0.2).abs() < 1e-3);
        // Raising sustain during decay does not jump either.
        let mut e = env(1.0, 500.0, 0.1, 10.0);
        e.gate_on();
        for _ in 0..5_000 {
            e.next_sample();
        }
        e.set_sustain(0.9);
        let mut prev = e.level();
        for _ in 0..5_000 {
            let v = e.next_sample();
            assert!((v - prev).abs() < 0.01, "{prev} -> {v}");
            prev = v;
        }
    }

    #[test]
    fn adsr_steal_fade_is_linear_over_2ms() {
        let mut e = env(0.5, 1.0, 0.8, 1_000.0);
        e.gate_on();
        for _ in 0..1_000 {
            e.next_sample();
        }
        e.steal_fade();
        assert_eq!(e.stage(), AdsrStage::Fade);
        let (n, lv) = run_until_change(&mut e, 1_000);
        assert!(n.abs_diff(96) <= 1, "fade took {n}");
        assert_eq!(e.stage(), AdsrStage::Idle);
        let step = 0.8 / 96.0;
        assert!(
            lv.windows(2)
                .all(|w| ((w[0] - w[1]) - step).abs() < 1e-4 || w[1] == 0.0)
        );
    }

    #[test]
    fn adsr_gate_off_when_idle_stays_idle() {
        let mut e = Adsr::new(SR);
        e.gate_off();
        assert_eq!(e.stage(), AdsrStage::Idle);
        assert_eq!(e.next_sample().to_bits(), 0.0f32.to_bits());
    }
}
