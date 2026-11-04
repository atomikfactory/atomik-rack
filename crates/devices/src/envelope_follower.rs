//! `atomik.envelope-follower`: turns the level of an audio signal into a control voltage.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! peak  x = max(|l|, |r|)          env follows x with `attack` (rising) and `release` (falling)
//! rms   x = (l^2 + r^2) / 2        env follows x the same way, level = sqrt(env)
//! level = env * gain               (rms: sqrt(env) * gain)
//! cv_out   = level                 (linear)   or   (20 log10(level) + 60) / 60 limited to 0..1 (db)
//! gate_out = 1 while level is above `threshold`, back to 0 3 dB lower
//! out_l, out_r = the input, unchanged
//! ```
//!
//! * **Detector.** `peak` follows the louder of the two channels sample by sample; `rms` follows
//!   the mean power of both and takes the square root, so a steady sine reads 0.707 of its
//!   amplitude and a full-scale square wave reads 1. Attack and release are one-pole time
//!   constants (the level covers 63 % of a jump in that time), `attack` 0.1..500 ms and `release`
//!   1..5000 ms.
//! * **Gain** (-24..+48 dB) scales the level before the output curve and the gate.
//! * **Curve.** `linear` outputs the level itself (limited to 8). `db` maps -60 dB and below to 0
//!   and 0 dB to 1, linearly in dB, which suits a CV that should move evenly with the loudness.
//! * **Gate.** `gate_out` opens when the level reaches `threshold` (dB, after the gain) and
//!   closes when it falls 3 dB below it.
//! * The input passes through bit for bit, with a non-finite sample counted as 0 (detection
//!   limits each sample to +-64). No latency. The dB maths use the deterministic `log2` and
//!   `2^x`; allocation-free.

use crate::tool_support::{audio_in, audio_out, choice, cv_out, expo, finite, linear, time_coef};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{exp2_det, log2_det};

const P_MODE: u32 = 0;
const P_ATTACK: u32 = 1;
const P_RELEASE: u32 = 2;
const P_GAIN: u32 = 3;
const P_CURVE: u32 = 4;
const P_THRESHOLD: u32 = 5;

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
const CV_OUT: SocketId = SocketId(4);
const GATE_OUT: SocketId = SocketId(5);

/// `log2(10) / 20`: `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// `20 * log10(2)`.
const DB_PER_OCTAVE: f32 = 6.020_6;
/// The `db` curve reads 0 at this level and below.
const FLOOR_DB: f32 = -60.0;
/// The gate closes this many dB below its threshold.
const GATE_HYSTERESIS_DB: f32 = 3.0;
/// Detector input limit.
const DETECT_LIMIT: f32 = 64.0;
/// Linear output limit.
const OUT_LIMIT: f32 = 8.0;

static MODES: [&str; 2] = ["peak", "rms"];
static CURVES: [&str; 2] = ["linear", "db"];

static PARAMS: [ParamDescriptor; 6] = [
    choice(P_MODE, "mode", "Mode", &MODES, 0.0),
    expo(P_ATTACK, "attack", "Attack", 0.1, 500.0, 5.0, Unit::Ms),
    expo(
        P_RELEASE,
        "release",
        "Release",
        1.0,
        5000.0,
        100.0,
        Unit::Ms,
    ),
    linear(P_GAIN, "gain", "Gain", -24.0, 48.0, 0.0, Unit::Db),
    choice(P_CURVE, "curve", "Curve", &CURVES, 0.0),
    linear(
        P_THRESHOLD,
        "threshold",
        "Threshold",
        -60.0,
        0.0,
        -30.0,
        Unit::Db,
    ),
];

static SOCKETS: [SocketDescriptor; 6] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_out(2, "out_l", "Out L"),
    audio_out(3, "out_r", "Out R"),
    cv_out(4, "cv_out", "CV out"),
    cv_out(5, "gate_out", "Gate out"),
];

/// Static description of Envelope Follower.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.envelope-follower",
    name: "Envelope Follower",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`EnvelopeFollower`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(EnvelopeFollower::new()),
    }
}

/// The audio-rate envelope follower.
#[derive(Debug, Clone, Copy)]
pub struct EnvelopeFollower {
    rms: bool,
    attack_ms: f32,
    release_ms: f32,
    gain: f32,
    db_curve: bool,
    threshold: f32,
    sample_rate: f32,
    attack: f64,
    release: f64,
    open_at: f32,
    close_at: f32,
    env: f64,
    open: bool,
}

impl EnvelopeFollower {
    fn new() -> Self {
        let mut f = Self {
            rms: false,
            attack_ms: 5.0,
            release_ms: 100.0,
            gain: 1.0,
            db_curve: false,
            threshold: 0.0,
            sample_rate: 48_000.0,
            attack: 0.0,
            release: 0.0,
            open_at: 0.0,
            close_at: 0.0,
            env: 0.0,
            open: false,
        };
        f.set_threshold(-30.0);
        f.derive();
        f
    }

    fn derive(&mut self) {
        self.attack = time_coef(self.attack_ms, self.sample_rate);
        self.release = time_coef(self.release_ms, self.sample_rate);
    }

    fn set_threshold(&mut self, db: f32) {
        self.threshold = db;
        self.open_at = exp2_det(db * LOG2_10_20);
        self.close_at = exp2_det((db - GATE_HYSTERESIS_DB) * LOG2_10_20);
    }
}

impl Device for EnvelopeFollower {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        self.env = 0.0;
        self.open = false;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_MODE => self.rms = v >= 0.5,
            P_ATTACK => {
                self.attack_ms = v.clamp(0.1, 500.0);
                self.derive();
            }
            P_RELEASE => {
                self.release_ms = v.clamp(1.0, 5000.0);
                self.derive();
            }
            P_GAIN => self.gain = exp2_det(v.clamp(-24.0, 48.0) * LOG2_10_20),
            P_CURVE => self.db_curve = v >= 0.5,
            P_THRESHOLD => self.set_threshold(v.clamp(-60.0, 0.0)),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut cv = [0.0f32; BATCH];
        let mut gate = [0.0f32; BATCH];
        for n in 0..BATCH {
            let (l, r) = (finite(in_l[n]), finite(in_r[n]));
            out_l[n] = l;
            out_r[n] = r;
            let (dl, dr) = (
                f64::from(l.clamp(-DETECT_LIMIT, DETECT_LIMIT)),
                f64::from(r.clamp(-DETECT_LIMIT, DETECT_LIMIT)),
            );
            let x = if self.rms {
                (dl * dl + dr * dr) * 0.5
            } else {
                dl.abs().max(dr.abs())
            };
            let coef = if x > self.env {
                self.attack
            } else {
                self.release
            };
            self.env += (x - self.env) * coef;
            if self.env < 1e-20 {
                self.env = 0.0;
            }
            let level = if self.rms { self.env.sqrt() } else { self.env } as f32 * self.gain;
            if self.open {
                if level < self.close_at {
                    self.open = false;
                }
            } else if level >= self.open_at && level > 0.0 {
                self.open = true;
            }
            gate[n] = if self.open { 1.0 } else { 0.0 };
            cv[n] = if self.db_curve {
                let db = log2_det(level.max(1e-6)) * DB_PER_OCTAVE;
                ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
            } else {
                level.min(OUT_LIMIT)
            };
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
        *ctx.output(CV_OUT) = cv;
        *ctx.output(GATE_OUT) = gate;
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::needless_range_loop)]
mod tests {
    use super::*;
    use crate::test_support::{assert_keys, harness, sweep_params_min_max};
    use atomik_testkit::harness::DeviceHarness;

    /// Feeds `x(n)` to both channels for `samples` samples; returns `(cv, gate)`.
    fn feed(
        h: &mut DeviceHarness,
        samples: usize,
        x: impl Fn(usize) -> f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let batches = samples.div_ceil(BATCH);
        let (mut cv, mut gate) = (Vec::new(), Vec::new());
        for b in 0..batches {
            let mut s = [0.0f32; BATCH];
            for (i, v) in s.iter_mut().enumerate() {
                *v = x(b * BATCH + i);
            }
            *h.input_mut("in_l") = s;
            *h.input_mut("in_r") = s;
            h.process();
            cv.extend_from_slice(h.output("cv_out"));
            gate.extend_from_slice(h.output("gate_out"));
        }
        (cv, gate)
    }

    fn sine(n: usize, hz: f32, amp: f32) -> f32 {
        amp * (std::f32::consts::TAU * hz * n as f32 / 48_000.0).sin()
    }

    #[test]
    fn descriptor_lists_the_keys_in_order() {
        assert_keys(
            &DESCRIPTOR,
            &["mode", "attack", "release", "gain", "curve", "threshold"],
            &["in_l", "in_r", "out_l", "out_r", "cv_out", "gate_out"],
        );
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
    }

    #[test]
    fn the_audio_passes_through_unchanged() {
        let mut h = harness(Box::new(EnvelopeFollower::new()));
        let mut l = [0.0f32; BATCH];
        for (i, v) in l.iter_mut().enumerate() {
            *v = (i as f32 * 0.37).sin() * 0.9;
        }
        *h.input_mut("in_l") = l;
        *h.input_mut("in_r") = [0.25; BATCH];
        h.process();
        for i in 0..BATCH {
            assert_eq!(h.output("out_l")[i].to_bits(), l[i].to_bits());
        }
        assert_eq!(*h.output("out_r"), [0.25; BATCH]);
    }

    #[test]
    fn peak_follows_a_step_with_its_attack_and_release() {
        let mut h = harness(Box::new(EnvelopeFollower::new()));
        h.set_param("attack", 10.0);
        h.set_param("release", 50.0);
        let (cv, _) = feed(&mut h, 4800, |_| 0.8);
        // After one attack constant (480 samples) it reached 63 % of 0.8.
        assert!((f64::from(cv[479]) - 0.8 * (1.0 - (-1.0f64).exp())).abs() < 0.01);
        assert!(cv.windows(2).all(|w| w[1] >= w[0]));
        assert!((cv[4799] - 0.8).abs() < 0.01);
        let top = cv[4799];
        let (cv, _) = feed(&mut h, 4800, |_| 0.0);
        // After one release constant (2400 samples) it fell to 37 %.
        assert!((f64::from(cv[2399]) - f64::from(top) * (-1.0f64).exp()).abs() < 0.01);
        assert!(cv.windows(2).all(|w| w[1] <= w[0]));
    }

    #[test]
    fn peak_reads_the_louder_channel() {
        let mut h = harness(Box::new(EnvelopeFollower::new()));
        h.set_param("attack", 0.1);
        *h.input_mut("in_l") = [0.2; BATCH];
        *h.input_mut("in_r") = [-0.6; BATCH];
        for _ in 0..8 {
            h.process();
        }
        assert!((h.output("cv_out")[BATCH - 1] - 0.6).abs() < 1e-3);
    }

    #[test]
    fn rms_reads_a_sine_at_0_707_and_a_square_at_one() {
        let mut h = harness(Box::new(EnvelopeFollower::new()));
        h.set_param("mode", 1.0);
        h.set_param("attack", 50.0);
        h.set_param("release", 50.0);
        let (cv, _) = feed(&mut h, 48_000, |n| sine(n, 440.0, 1.0));
        assert!((cv[47_999] - 0.707).abs() < 0.02, "{}", cv[47_999]);
        let mut sq = harness(Box::new(EnvelopeFollower::new()));
        sq.set_param("mode", 1.0);
        sq.set_param("attack", 50.0);
        sq.set_param("release", 50.0);
        let (cv, _) = feed(
            &mut sq,
            48_000,
            |n| if (n / 50) % 2 == 0 { 0.5 } else { -0.5 },
        );
        assert!((cv[47_999] - 0.5).abs() < 0.01);
        // Peak mode reads the sine's top.
        let mut p = harness(Box::new(EnvelopeFollower::new()));
        p.set_param("attack", 0.1);
        p.set_param("release", 500.0);
        let (cv, _) = feed(&mut p, 48_000, |n| sine(n, 440.0, 1.0));
        assert!(cv[47_999] > 0.9);
    }

    #[test]
    fn gain_scales_the_level_and_the_db_curve_maps_minus_sixty_to_zero_and_zero_to_one() {
        let mut h = harness(Box::new(EnvelopeFollower::new()));
        h.set_param("attack", 0.1);
        h.set_param("gain", 6.0);
        let (cv, _) = feed(&mut h, 4800, |_| 0.25);
        assert!((cv[4799] - 0.25 * 1.995).abs() < 0.01);
        h.set_param("curve", 1.0);
        h.set_param("gain", 0.0);
        for (amp, want) in [
            (1.0, 1.0),
            (0.1, (-20.0 + 60.0) / 60.0),
            (0.001, 0.0),
            (0.0, 0.0),
        ] {
            let (cv, _) = feed(&mut h, 48_000, |_| amp);
            assert!(
                (cv[47_999] - want).abs() < 0.01,
                "amp {amp}: {}",
                cv[47_999]
            );
        }
        let (cv, _) = feed(&mut h, 4800, |_| 2.0);
        assert_eq!(cv[4799], 1.0, "the db curve is limited to 1");
    }

    #[test]
    fn the_linear_output_is_limited_to_eight() {
        let mut h = harness(Box::new(EnvelopeFollower::new()));
        h.set_param("attack", 0.1);
        h.set_param("gain", 48.0);
        let (cv, _) = feed(&mut h, 4800, |_| 50.0);
        assert_eq!(cv[4799], 8.0);
    }

    #[test]
    fn the_gate_opens_at_the_threshold_and_closes_three_db_lower() {
        let mut h = harness(Box::new(EnvelopeFollower::new()));
        h.set_param("attack", 0.1);
        h.set_param("release", 1.0);
        h.set_param("threshold", -20.0); // 0.1
        let (_, g) = feed(&mut h, 960, |_| 0.05);
        assert!(g.iter().all(|v| *v == 0.0));
        let (_, g) = feed(&mut h, 960, |_| 0.15);
        assert_eq!(g[959], 1.0);
        // -3 dB below 0.1 is 0.0708: 0.08 keeps it open, 0.06 closes it.
        let (_, g) = feed(&mut h, 960, |_| 0.08);
        assert_eq!(g[959], 1.0);
        let (_, g) = feed(&mut h, 960, |_| 0.06);
        assert_eq!(g[959], 0.0);
        let (_, g) = feed(&mut h, 960, |_| 0.09);
        assert_eq!(
            g[959], 0.0,
            "back above the closing level but below the threshold"
        );
    }

    #[test]
    fn silence_stays_silent_and_non_finite_input_counts_as_zero() {
        let mut h = harness(Box::new(EnvelopeFollower::new()));
        let (cv, g) = feed(&mut h, 480, |_| f32::NAN);
        assert!(cv.iter().chain(&g).all(|v| *v == 0.0));
        assert!(h.output("out_l").iter().all(|v| *v == 0.0));
        h.set_param("curve", 1.0);
        let (cv, _) = feed(&mut h, 480, |_| 0.0);
        assert!(cv.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn reset_empties_the_follower_and_sweeps_stay_finite() {
        let mut h = harness(Box::new(EnvelopeFollower::new()));
        h.set_param("attack", 0.1);
        feed(&mut h, 480, |_| 0.9);
        h.device_mut().reset();
        let (cv, _) = feed(&mut h, BATCH, |_| 0.0);
        assert_eq!(cv[0], 0.0);
        *h.input_mut("in_l") = [0.7; BATCH];
        *h.input_mut("in_r") = [-0.7; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }
}
