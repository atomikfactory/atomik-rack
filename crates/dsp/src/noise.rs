//! Deterministic noise and crackle generators and the presence gate.
//!
//! Everything here is synthesised from a seeded [`crate::Rng`] with per-sample draws only, so the
//! output depends on the seed and the sample index and nothing else (not the host block size, not
//! the platform: integer draws, thresholds on uniform draws, `+ - * / sqrt` and the deterministic
//! `exp2`/`log2` of the private `det_math` module). Nothing is looped from stored audio.
//!
//! * [`WhiteNoise`]: uniform white noise scaled to unit RMS.
//! * [`PinkNoise`]: white noise through a fixed-coefficient filter bank, `-3 dB/oct`, unit RMS.
//!   The coefficients are tied to the sample rate (the shape scales with it).
//! * [`Rumble`]: band-limited low-frequency rumble, white noise through four equal one-pole
//!   low-passes whose corner wanders between two frequencies; normalised to about unit RMS.
//! * [`Crackle`]: Poisson events (a threshold on a uniform draw), heavy-tailed amplitudes
//!   (`A = a_min / sqrt(1 - u)`, clipped at 1: a Pareto tail) and exponentially decaying clicks.
//! * [`Wander`]: a smooth seeded random control in `0..1` (random targets joined by a smoothstep).
//! * [`PresenceGate`]: a detector on the dry programme; the gain is 1 while programme has been
//!   present within the hold time, then fades linearly to 0; an idle gate is closed.

use crate::{Rng, exp2_det, exp2_det_f64, flush_denormal, log2_det};

/// Unit-RMS scale of a uniform `-1..1` draw (`sqrt 3`).
const UNIFORM_TO_UNIT_RMS: f32 = 1.732_050_8;
/// Output scale that gives the pink filter bank unit RMS for a unit-RMS white input.
const PINK_GAIN: f32 = 0.327;
/// Samples between corner updates of [`Rumble`].
const RUMBLE_INTERVAL: u32 = 32;

/// Uniform white noise with unit RMS.
#[derive(Debug, Clone)]
pub struct WhiteNoise {
    rng: Rng,
}

impl WhiteNoise {
    /// A generator for `seed`.
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
        }
    }

    /// The next sample (uniform in `+-sqrt 3`, RMS 1).
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        self.rng.next_bipolar() * UNIFORM_TO_UNIT_RMS
    }
}

/// Pink noise (`-3 dB/oct`) with about unit RMS.
#[derive(Debug, Clone)]
pub struct PinkNoise {
    white: WhiteNoise,
    b: [f32; 7],
}

impl PinkNoise {
    /// A generator for `seed`.
    pub fn new(seed: u64) -> Self {
        Self {
            white: WhiteNoise::new(seed),
            b: [0.0; 7],
        }
    }

    /// The next sample.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let w = self.white.next_sample();
        let b = &mut self.b;
        b[0] = 0.998_86 * b[0] + w * 0.055_517_9;
        b[1] = 0.993_32 * b[1] + w * 0.075_075_9;
        b[2] = 0.969 * b[2] + w * 0.153_852;
        b[3] = 0.866_50 * b[3] + w * 0.310_485_6;
        b[4] = 0.55 * b[4] + w * 0.532_952_2;
        b[5] = -0.761_6 * b[5] - w * 0.016_898;
        let out = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.536_2;
        b[6] = w * 0.115_926;
        PINK_GAIN * out
    }
}

/// A smooth seeded random control in `0..1`: random targets joined by a smoothstep, with segment
/// lengths drawn uniformly around `1 / rate` (0.5 to 1.5 times).
#[derive(Debug, Clone)]
pub struct Wander {
    rng: Rng,
    sample_rate: f32,
    rate_hz: f32,
    from: f32,
    to: f32,
    pos: f32,
    inc: f32,
}

impl Wander {
    /// A control with `rate_hz` new targets per second on average.
    pub fn new(seed: u64, sample_rate: f32, rate_hz: f32) -> Self {
        let mut rng = Rng::new(seed);
        let from = rng.next_f32();
        let to = rng.next_f32();
        let mut w = Self {
            rng,
            sample_rate,
            rate_hz: 1.0,
            from,
            to,
            pos: 0.0,
            inc: 0.0,
        };
        w.set_rate_hz(rate_hz);
        w.inc = w.draw_inc();
        w
    }

    /// Average number of new targets per second (clamped to `0.01..=100`).
    pub fn set_rate_hz(&mut self, hz: f32) {
        if !hz.is_nan() {
            self.rate_hz = hz.clamp(0.01, 100.0);
        }
    }

    fn draw_inc(&mut self) -> f32 {
        let len = self.sample_rate / self.rate_hz * (0.5 + self.rng.next_f32());
        1.0 / len.max(1.0)
    }

    /// The next value in `0..1`.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let t = self.pos;
        let s = t * t * (3.0 - 2.0 * t);
        let y = self.from + (self.to - self.from) * s;
        self.pos += self.inc;
        if self.pos >= 1.0 {
            self.pos -= 1.0;
            self.from = self.to;
            self.to = self.rng.next_f32();
            self.inc = self.draw_inc();
        }
        y
    }
}

/// Band-limited rumble with a wandering corner.
#[derive(Debug, Clone)]
pub struct Rumble {
    white: WhiteNoise,
    wander: Wander,
    sample_rate: f32,
    lo_hz: f32,
    hi_hz: f32,
    coeff: f32,
    gain: f32,
    state: [f32; 4],
    counter: u32,
}

impl Rumble {
    /// A rumble generator with its corner wandering between 15 and 45 Hz.
    pub fn new(seed: u64, sample_rate: f32) -> Self {
        let mut r = Self {
            white: WhiteNoise::new(seed),
            wander: Wander::new(crate::rng::hash64(seed, 0x52, 0), sample_rate, 0.3),
            sample_rate,
            lo_hz: 15.0,
            hi_hz: 45.0,
            coeff: 0.0,
            gain: 0.0,
            state: [0.0; 4],
            counter: 0,
        };
        r.update(0.5);
        r
    }

    /// The range the corner frequency wanders in (each clamped to 5..=150 Hz, `lo <= hi`).
    pub fn set_corner_range_hz(&mut self, lo: f32, hi: f32) {
        if lo.is_nan() || hi.is_nan() {
            return;
        }
        self.lo_hz = lo.clamp(5.0, 150.0);
        self.hi_hz = hi.clamp(self.lo_hz, 150.0);
    }

    /// How fast the corner wanders (new targets per second).
    pub fn set_wander_rate_hz(&mut self, hz: f32) {
        self.wander.set_rate_hz(hz);
    }

    fn update(&mut self, w: f32) {
        let ratio = self.hi_hz / self.lo_hz;
        let fc = self.lo_hz * exp2_det(w * log2_det(ratio));
        self.coeff = (1.0
            - exp2_det_f64(
                -std::f64::consts::TAU * f64::from(fc) / f64::from(self.sample_rate)
                    * std::f64::consts::LOG2_E,
            )) as f32;
        // Four equal poles: output variance is about 0.9818 fc/fs of the (unit) input variance.
        self.gain = 1.0 / (0.981_75 * fc / self.sample_rate).sqrt();
    }

    /// The next sample (about unit RMS, almost all energy below 80 Hz).
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let w = self.wander.next_sample();
        if self.counter == 0 {
            self.update(w);
        }
        self.counter += 1;
        if self.counter == RUMBLE_INTERVAL {
            self.counter = 0;
        }
        let mut x = self.white.next_sample();
        for s in &mut self.state {
            *s = flush_denormal(*s + self.coeff * (x - *s));
            x = *s;
        }
        x * self.gain
    }
}

/// Poisson crackle: decaying clicks at random times with heavy-tailed amplitudes.
#[derive(Debug, Clone)]
pub struct Crackle {
    rng: Rng,
    sample_rate: f32,
    p: f32,
    a_min: f32,
    decay: f32,
    state: f32,
    events: u64,
    last_amp: f32,
}

impl Crackle {
    /// A silent crackle generator (rate 0): defaults a_min 0.05 and click decay 0.5 ms.
    pub fn new(seed: u64, sample_rate: f32) -> Self {
        let mut c = Self {
            rng: Rng::new(seed),
            sample_rate,
            p: 0.0,
            a_min: 0.05,
            decay: 0.0,
            state: 0.0,
            events: 0,
            last_amp: 0.0,
        };
        c.set_decay_ms(0.5);
        c
    }

    /// Mean events per second (clamped to `0..=20000`).
    pub fn set_rate_hz(&mut self, rate: f32) {
        if !rate.is_nan() {
            self.p = rate.clamp(0.0, 20_000.0) / self.sample_rate;
        }
    }

    /// Smallest event amplitude, `0 < a_min <= 1` (the Pareto scale; clamped to `0.001..=1`).
    pub fn set_min_amplitude(&mut self, a: f32) {
        if !a.is_nan() {
            self.a_min = a.clamp(0.001, 1.0);
        }
    }

    /// Click decay time constant in ms (clamped to `0.01..=50`).
    pub fn set_decay_ms(&mut self, ms: f32) {
        if !ms.is_nan() {
            let n = f64::from(ms.clamp(0.01, 50.0)) * 0.001 * f64::from(self.sample_rate);
            self.decay = exp2_det_f64(-std::f64::consts::LOG2_E / n) as f32;
        }
    }

    /// Expected event amplitude `E[min(A, 1)] = 2 a_min - a_min^2` for the current `a_min`.
    pub fn expected_amplitude(&self) -> f32 {
        2.0 * self.a_min - self.a_min * self.a_min
    }

    /// Number of events fired so far.
    pub fn events(&self) -> u64 {
        self.events
    }

    /// Amplitude (unsigned) of the most recent event.
    pub fn last_event_amplitude(&self) -> f32 {
        self.last_amp
    }

    /// The next sample.
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        if self.rng.next_f32() < self.p {
            let bits = self.rng.next_u64();
            let u = (bits >> 40) as f32 * (1.0 / 16_777_216.0);
            let a = (self.a_min / (1.0 - u).sqrt()).min(1.0);
            self.last_amp = a;
            self.events += 1;
            self.state += if bits & 1 == 0 { a } else { -a };
        }
        let y = self.state;
        self.state = flush_denormal(self.state * self.decay);
        y
    }
}

/// Programme-presence gate for generated noise: open while programme has been present within the
/// hold time, fading out linearly afterwards; closed (silent) when idle.
#[derive(Debug, Clone)]
pub struct PresenceGate {
    sample_rate: f32,
    threshold: f32,
    env: f32,
    env_decay: f32,
    hold_samples: u32,
    hold_left: u32,
    open_step: f32,
    fade_step: f32,
    gain: f32,
}

impl PresenceGate {
    /// A closed gate with hold 3 s, fade 2 s, open time 20 ms and threshold -70 dBFS.
    pub fn new(sample_rate: f32) -> Self {
        let mut g = Self {
            sample_rate,
            threshold: 0.0,
            env: 0.0,
            env_decay: 0.0,
            hold_samples: 0,
            hold_left: 0,
            open_step: 0.0,
            fade_step: 0.0,
            gain: 0.0,
        };
        g.set_threshold_db(-70.0);
        g.set_hold_s(3.0);
        g.set_fade_s(2.0);
        g.set_open_ms(20.0);
        g.env_decay = 1.0 / (0.050 * sample_rate);
        g
    }

    /// Detector threshold in dBFS (clamped to -120..=0).
    pub fn set_threshold_db(&mut self, db: f32) {
        if !db.is_nan() {
            self.threshold = exp2_det(db.clamp(-120.0, 0.0) * 0.166_096_4);
        }
    }

    /// How long the gate stays fully open after programme was last present (0..=60 s).
    pub fn set_hold_s(&mut self, s: f32) {
        if !s.is_nan() {
            self.hold_samples = (s.clamp(0.0, 60.0) * self.sample_rate) as u32;
        }
    }

    /// Length of the linear fade to silence after the hold (0.001..=60 s).
    pub fn set_fade_s(&mut self, s: f32) {
        if !s.is_nan() {
            self.fade_step = 1.0 / (s.clamp(0.001, 60.0) * self.sample_rate);
        }
    }

    /// Length of the linear fade in when programme arrives (0.1..=1000 ms).
    pub fn set_open_ms(&mut self, ms: f32) {
        if !ms.is_nan() {
            self.open_step = 1.0 / (ms.clamp(0.1, 1000.0) * 0.001 * self.sample_rate);
        }
    }

    /// Closes the gate and clears the detector.
    pub fn reset(&mut self) {
        self.env = 0.0;
        self.hold_left = 0;
        self.gain = 0.0;
    }

    /// The current gain.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Feeds one sample of the dry programme (non-finite counts as silence) and returns the gain
    /// to apply to the generated noise: it is exactly 0 once the hold and the fade have elapsed.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let a = if x.is_finite() { x.abs() } else { 0.0 };
        self.env = flush_denormal(a.max(self.env - self.env_decay * self.env));
        if self.env > self.threshold {
            self.hold_left = self.hold_samples.max(1);
        } else if self.hold_left > 0 {
            self.hold_left -= 1;
        }
        if self.hold_left > 0 {
            self.gain = (self.gain + self.open_step).min(1.0);
        } else {
            self.gain = (self.gain - self.fade_step).max(0.0);
        }
        self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{db, rms, sine, spectrum};

    const SR: f32 = 48_000.0;

    /// Power of `x` (Hann-windowed periodogram, averaged over blocks) per octave band.
    fn octave_powers(x: &[f32], first_hz: f64, bands: usize) -> Vec<f64> {
        let n = 16_384usize;
        let mut acc = vec![0.0f64; n / 2];
        let mut blocks = 0.0;
        for c in x.chunks_exact(n) {
            for (a, m) in acc.iter_mut().zip(spectrum(c)) {
                *a += m * m;
            }
            blocks += 1.0;
        }
        let bin = f64::from(SR) / n as f64;
        (0..bands)
            .map(|b| {
                let lo = first_hz * 2f64.powi(b as i32);
                let hi = lo * 2.0;
                let s: f64 = (0..n / 2)
                    .filter(|k| {
                        let f = *k as f64 * bin;
                        f >= lo && f < hi
                    })
                    .map(|k| acc[k])
                    .sum();
                s / blocks
            })
            .collect()
    }

    #[test]
    fn white_noise_has_zero_mean_unit_rms_and_a_flat_spectrum() {
        let mut g = WhiteNoise::new(1);
        let x: Vec<f32> = (0..1 << 19).map(|_| g.next_sample()).collect();
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        assert!(mean.abs() < 0.01, "{mean}");
        assert!((rms(&x) - 1.0).abs() < 0.02);
        // White noise has power proportional to bandwidth: each octave doubles.
        let p = octave_powers(&x, 100.0, 7);
        for w in p.windows(2) {
            assert!(
                (db(w[1] / w[0]) / 2.0 - 3.01).abs() < 0.6,
                "{}",
                db(w[1] / w[0]) / 2.0
            );
        }
    }

    #[test]
    fn pink_noise_has_equal_energy_per_octave_and_unit_level() {
        let mut g = PinkNoise::new(2);
        let x: Vec<f32> = (0..1 << 20).map(|_| g.next_sample()).collect();
        assert!((rms(&x) - 1.0).abs() < 0.12, "rms {}", rms(&x));
        let p = octave_powers(&x, 100.0, 8);
        let mean = p.iter().sum::<f64>() / p.len() as f64;
        for (i, v) in p.iter().enumerate() {
            let dev = 10.0 * (v / mean).log10();
            assert!(dev.abs() < 1.5, "band {i}: {dev} dB from the mean");
        }
    }

    #[test]
    fn rumble_lives_below_80_hz_at_about_unit_level() {
        let mut r = Rumble::new(3, SR);
        r.set_corner_range_hz(30.0, 30.0);
        let x: Vec<f32> = (0..1 << 19).map(|_| r.next_sample()).collect();
        assert!(
            (rms(&x[10_000..]) - 1.0).abs() < 0.3,
            "{}",
            rms(&x[10_000..])
        );
        let n = 16_384;
        let mut lo = 0.0;
        let mut hi = 0.0;
        for c in x[10_000..].chunks_exact(n) {
            for (k, m) in spectrum(c).iter().enumerate() {
                let f = k as f64 * f64::from(SR) / n as f64;
                if f < 80.0 {
                    lo += m * m;
                } else {
                    hi += m * m;
                }
            }
        }
        assert!(hi / (lo + hi) < 0.03, "{}", hi / (lo + hi));
    }

    #[test]
    fn rumble_corner_wanders_within_its_range() {
        let mut r = Rumble::new(4, SR);
        r.set_corner_range_hz(10.0, 80.0);
        r.set_wander_rate_hz(2.0);
        let x: Vec<f32> = (0..SR as usize * 20).map(|_| r.next_sample()).collect();
        assert!(x.iter().all(|v| v.is_finite() && v.abs() < 12.0));
        // Short-time levels vary as the corner moves (the normalisation keeps them similar).
        let lv: Vec<f32> = x.chunks(48_000).map(rms).collect();
        assert!(lv.iter().all(|v| *v > 0.3 && *v < 2.0), "{lv:?}");
        r.set_corner_range_hz(f32::NAN, 1.0);
        r.set_corner_range_hz(100.0, 20.0);
        assert!(r.next_sample().is_finite());
    }

    #[test]
    fn crackle_rate_amplitude_and_decay_follow_the_settings() {
        for (rate, a_min) in [(0.5f32, 0.05f32), (6.0, 0.1), (30.0, 0.2)] {
            let mut c = Crackle::new(11, SR);
            c.set_rate_hz(rate);
            c.set_min_amplitude(a_min);
            // At least 1500 expected events so the Poisson count is within 10 % (2.6 % sigma).
            let secs = (1500.0 / rate).ceil().max(120.0) as usize;
            let mut sum = 0.0f64;
            let mut count = 0u64;
            for _ in 0..SR as usize * secs {
                let before = c.events();
                c.next_sample();
                if c.events() != before {
                    sum += f64::from(c.last_event_amplitude());
                    count += 1;
                }
            }
            let measured = count as f64 / secs as f64;
            assert!(
                (measured / f64::from(rate) - 1.0).abs() < 0.1,
                "rate {rate}: {measured}"
            );
            let mean = sum / count as f64;
            let want = f64::from(c.expected_amplitude());
            assert!(
                (mean / want - 1.0).abs() < 0.1,
                "amp {a_min}: {mean} vs {want}"
            );
        }
    }

    #[test]
    fn crackle_clicks_decay_exponentially_and_are_two_signed() {
        let mut c = Crackle::new(5, SR);
        c.set_rate_hz(2.0);
        c.set_decay_ms(0.5);
        let d = (-1.0f64 / (0.0005 * f64::from(SR))).exp();
        let x: Vec<f32> = (0..SR as usize * 20).map(|_| c.next_sample()).collect();
        let mut checked = 0;
        for w in x.windows(3) {
            // Inside a click tail (no new event): the ratio of successive samples is the decay.
            if w[0].abs() > 1e-3 && w[1].abs() > 1e-3 && (w[1] / w[0]).abs() < 1.0 {
                assert!((f64::from(w[1] / w[0]) - d).abs() < 2e-3);
                checked += 1;
            }
        }
        assert!(checked > 20);
        assert!(x.iter().any(|v| *v > 0.0) && x.iter().any(|v| *v < 0.0));
        c.set_rate_hz(0.0);
        let q: Vec<f32> = (0..10_000).map(|_| c.next_sample()).collect();
        assert_eq!(*q.last().unwrap(), 0.0, "decays to exact zero");
    }

    #[test]
    fn generators_are_deterministic_and_seed_dependent() {
        let mk = |seed: u64| {
            let mut w = WhiteNoise::new(seed);
            let mut p = PinkNoise::new(seed);
            let mut r = Rumble::new(seed, SR);
            let mut c = Crackle::new(seed, SR);
            c.set_rate_hz(500.0);
            (0..2_000)
                .map(|_| {
                    [
                        w.next_sample(),
                        p.next_sample(),
                        r.next_sample(),
                        c.next_sample(),
                    ]
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(mk(9), mk(9));
        assert_ne!(mk(9), mk(10));
        // A window of 64 samples taken from a long run equals the same 64 samples drawn from a
        // fresh generator that skipped to that index: output depends on seed and index only.
        let long: Vec<f32> = {
            let mut p = PinkNoise::new(4);
            (0..5_000).map(|_| p.next_sample()).collect()
        };
        let mut p = PinkNoise::new(4);
        for _ in 0..4_000 {
            p.next_sample();
        }
        let tail: Vec<f32> = (0..64).map(|_| p.next_sample()).collect();
        assert_eq!(&long[4_000..4_064], &tail[..]);
    }

    #[test]
    fn wander_is_smooth_bounded_and_deterministic() {
        let mut w = Wander::new(1, SR, 2.0);
        let x: Vec<f32> = (0..SR as usize * 30).map(|_| w.next_sample()).collect();
        assert!(x.iter().all(|v| (0.0..1.0).contains(v)));
        let max_step = x.windows(2).fold(0.0f32, |a, p| a.max((p[1] - p[0]).abs()));
        // Smoothstep slope is at most 1.5 per segment; the shortest segment is 0.25 s.
        assert!(max_step < 1.5 / (0.25 * SR) * 1.1, "{max_step}");
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        assert!((mean - 0.5).abs() < 0.15, "{mean}");
        let mut w2 = Wander::new(1, SR, 2.0);
        assert_eq!(w2.next_sample().to_bits(), x[0].to_bits());
        w2.set_rate_hz(f32::NAN);
        assert!(w2.next_sample().is_finite());
    }

    #[test]
    fn presence_gate_is_silent_when_idle_open_during_programme_and_fades_after() {
        // Idle: no programme at all keeps the gate closed forever.
        let mut g = PresenceGate::new(SR);
        for _ in 0..SR as usize * 3 {
            assert_eq!(g.process(0.0), 0.0);
        }
        // Programme for 10 s opens it within the open time.
        g.set_hold_s(2.0);
        g.set_fade_s(3.0);
        let x = sine(220.0, f64::from(SR), SR as usize * 10, 0.1);
        let mut gain = Vec::new();
        for v in &x {
            gain.push(g.process(*v));
        }
        assert!(gain[(0.05 * SR) as usize] > 0.99, "open after 50 ms");
        assert_eq!(*gain.last().unwrap(), 1.0);
        // Programme stops: gain stays 1 for about the hold (plus the detector's 50 ms decay),
        // then ramps linearly to exactly 0 after hold + fade.
        let mut after = Vec::new();
        for _ in 0..SR as usize * 7 {
            after.push(g.process(0.0));
        }
        assert_eq!(
            after[(1.8 * SR) as usize],
            1.0,
            "still open inside the hold"
        );
        assert!(after[(2.5 * SR) as usize] < 1.0 && after[(2.5 * SR) as usize] > 0.5);
        assert_eq!(after[(5.5 * SR) as usize], 0.0, "closed after hold + fade");
        // Below -80 dBFS of a 0 dBFS-noise: the gain is exactly zero, so the noise is too.
        assert_eq!(g.gain(), 0.0);
        g.reset();
        assert_eq!(g.process(f32::NAN), 0.0);
        assert_eq!(g.process(f32::INFINITY), 0.0);
    }

    #[test]
    fn presence_gate_threshold_and_setters() {
        let mut g = PresenceGate::new(SR);
        g.set_threshold_db(-40.0);
        for _ in 0..SR as usize {
            // -60 dBFS programme is below the threshold: stays closed.
            assert_eq!(g.process(0.001), 0.0);
        }
        for _ in 0..SR as usize / 2 {
            g.process(0.1);
        }
        assert_eq!(g.gain(), 1.0);
        g.set_threshold_db(f32::NAN);
        g.set_hold_s(f32::NAN);
        g.set_fade_s(f32::NAN);
        g.set_open_ms(f32::NAN);
        g.set_hold_s(1.0e9);
        g.set_fade_s(0.0);
        g.set_open_ms(0.0);
        assert!(g.process(0.5).is_finite());
        let mut c = Crackle::new(1, SR);
        c.set_rate_hz(f32::NAN);
        c.set_min_amplitude(f32::NAN);
        c.set_decay_ms(f32::NAN);
        c.set_min_amplitude(5.0);
        assert!((c.expected_amplitude() - 1.0).abs() < 1e-6);
    }

    proptest::proptest! {
        #[test]
        fn generators_are_finite_and_bounded_for_any_parameters(
            seed in 0u64..1000, rate in 0.0f32..30_000.0, a in 0.0f32..2.0, decay in 0.0f32..100.0,
            lo in 0.0f32..200.0, hi in 0.0f32..200.0
        ) {
            let mut c = Crackle::new(seed, SR);
            c.set_rate_hz(rate);
            c.set_min_amplitude(a);
            c.set_decay_ms(decay);
            let mut r = Rumble::new(seed, SR);
            r.set_corner_range_hz(lo, hi);
            let mut p = PinkNoise::new(seed);
            let mut g = PresenceGate::new(SR);
            for _ in 0..4096 {
                let v = c.next_sample();
                proptest::prop_assert!(v.is_finite() && v.abs() < 5_000.0);
                proptest::prop_assert!(r.next_sample().is_finite() && r.next_sample().abs() < 100.0);
                proptest::prop_assert!(p.next_sample().abs() < 10.0);
                let gain = g.process(v);
                proptest::prop_assert!((0.0..=1.0).contains(&gain));
            }
        }
    }

    #[test]
    fn the_default_presence_threshold_is_minus_70_dbfs() {
        // -60 dBFS programme opens a default gate, -80 dBFS programme does not.
        let mut open = PresenceGate::new(SR);
        for _ in 0..SR as usize {
            open.process(0.001);
        }
        assert_eq!(open.gain(), 1.0);
        let mut shut = PresenceGate::new(SR);
        for _ in 0..SR as usize {
            assert_eq!(shut.process(1.0e-4), 0.0);
        }
    }
}
