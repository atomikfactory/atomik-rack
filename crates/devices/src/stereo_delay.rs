//! `atomik.stereo-delay`: stereo delay with ms or tempo-synced time, feedback, dry/wet mix
//! and ping-pong (03 §4.3).
//!
//! * Delay time = `time.ms`, or the `time.sync` note value at `ctx.time.tempo`. It is then
//!   scaled by `2^cv` (time CV clamped to −1..1) and clamped to 1..=4000 ms.
//! * Every change of the target time starts a linear glide of the read position (read with
//!   linear interpolation). Small changes complete in 50 ms; the glide speed is capped at
//!   [`MAX_GLIDE_SLEW`] samples of delay per sample, so the read head never runs faster than 2x
//!   or slower than standstill. Large jumps therefore take longer (about 1 s per second of change)
//!   and are heard as a bounded tape-style pitch sweep, never as a click.
//! * Feedback and mix are smoothed over 5 ms. The first batch after `prepare`/`reset` starts at
//!   the target time, feedback and mix without a glide (no audio has passed yet).
//! * Ping-pong: the input is summed to mono as `(L + R)/2` into the left line; the feedback path
//!   crosses L→R and R→L. Off: two independent lines.
//! * Buffers hold 4 s at the prepared sample rate (allocated in `prepare`).
//! * Denormals: the delay lines flush values below 1e-20 on write.

use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{DelayLine, Smoother};

/// Longest delay in ms.
pub const MAX_DELAY_MS: f32 = 4_000.0;
/// Shortest delay in ms.
pub const MIN_DELAY_MS: f32 = 1.0;
/// Glide time for delay-time changes (small changes; large ones are slew-limited).
pub const GLIDE_MS: f32 = 50.0;
/// Maximum change of the delay time, in samples per sample, during a glide (read-head speed
/// stays within 0..=2x).
pub const MAX_GLIDE_SLEW: f32 = 1.0;
const SMOOTH_MS: f32 = 5.0;

/// Sync note values in quarter-note beats, in `time.sync` choice order.
pub const SYNC_BEATS: [f64; 12] = [
    0.125,     // 1/32
    1.0 / 6.0, // 1/16T
    0.25,      // 1/16
    0.375,     // 1/16D
    1.0 / 3.0, // 1/8T
    0.5,       // 1/8
    0.75,      // 1/8D
    2.0 / 3.0, // 1/4T
    1.0,       // 1/4
    1.5,       // 1/4D
    2.0,       // 1/2
    4.0,       // 1/1
];

const P_MODE: u32 = 0;
const P_MS: u32 = 1;
const P_SYNC: u32 = 2;
const P_FEEDBACK: u32 = 3;
const P_MIX: u32 = 4;
const P_PING_PONG: u32 = 5;

static PARAMS: [ParamDescriptor; 6] = [
    ParamDescriptor {
        id: ParamId(P_MODE),
        key: "time.mode",
        name: "Time mode",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&["ms", "sync"]),
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_MS),
        key: "time.ms",
        name: "Time",
        min: 1.0,
        max: 4_000.0,
        default: 375.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_SYNC),
        key: "time.sync",
        name: "Sync",
        min: 0.0,
        max: 11.0,
        default: 6.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&[
            "1/32", "1/16T", "1/16", "1/16D", "1/8T", "1/8", "1/8D", "1/4T", "1/4", "1/4D", "1/2",
            "1/1",
        ]),
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_FEEDBACK),
        key: "feedback",
        name: "Feedback",
        min: 0.0,
        max: 0.95,
        default: 0.4,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_MIX),
        key: "mix",
        name: "Mix",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_PING_PONG),
        key: "ping_pong",
        name: "Ping-pong",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    },
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
const TIME_CV: SocketId = SocketId(4);

static SOCKETS: [SocketDescriptor; 5] = [
    SocketDescriptor {
        id: IN_L,
        key: "in_l",
        name: "In L",
        kind: SocketKind::Audio,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: IN_R,
        key: "in_r",
        name: "In R",
        kind: SocketKind::Audio,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: OUT_L,
        key: "out_l",
        name: "Out L",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    },
    SocketDescriptor {
        id: OUT_R,
        key: "out_r",
        name: "Out R",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    },
    SocketDescriptor {
        id: TIME_CV,
        key: "time_cv_in",
        name: "Time CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
];

/// Static description of the stereo delay.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.stereo-delay",
    name: "Stereo Delay",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`StereoDelay`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(StereoDelay::new()),
    }
}

/// The stereo delay device.
#[derive(Debug, Clone)]
pub struct StereoDelay {
    sample_rate: f32,
    sync: bool,
    time_ms: f32,
    sync_index: usize,
    ping_pong: bool,
    feedback: Smoother,
    mix: Smoother,
    /// Current delay in samples.
    delay: f32,
    /// Target of the current glide, in samples.
    glide_target: f32,
    /// Glide speed in samples per sample (≤ `MAX_GLIDE_SLEW`).
    glide_rate: f32,
    fresh: bool,
    line_l: DelayLine,
    line_r: DelayLine,
}

impl Default for StereoDelay {
    fn default() -> Self {
        Self::new()
    }
}

impl StereoDelay {
    /// A delay with default params. Its buffers are sized by `prepare`.
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            sync: true,
            time_ms: 375.0,
            sync_index: 6,
            ping_pong: false,
            feedback: Smoother::new(0.4, SMOOTH_MS, 48_000.0),
            mix: Smoother::new(0.3, SMOOTH_MS, 48_000.0),
            delay: 1.0,
            glide_target: 1.0,
            glide_rate: 0.0,
            fresh: true,
            line_l: DelayLine::new(1),
            line_r: DelayLine::new(1),
        }
    }
}

impl StereoDelay {
    /// Advances the slew-limited glide one sample towards `target` and returns the delay.
    fn next_delay(&mut self, target: f32) -> f32 {
        if (target - self.glide_target).abs() > f32::EPSILON {
            let glide_samples = (GLIDE_MS * 0.001 * self.sample_rate).max(1.0);
            self.glide_rate = ((target - self.delay).abs() / glide_samples).min(MAX_GLIDE_SLEW);
            self.glide_target = target;
        }
        let remaining = self.glide_target - self.delay;
        self.delay += remaining.clamp(-self.glide_rate, self.glide_rate);
        self.delay
    }
}

impl Device for StereoDelay {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        // 4 s plus one sample of interpolation headroom.
        let max = (f64::from(MAX_DELAY_MS) * 0.001 * info.sample_rate.hz()).ceil() as usize + 1;
        self.line_l.resize(max);
        self.line_r.resize(max);
        self.feedback.set_time(SMOOTH_MS, self.sample_rate);
        self.mix.set_time(SMOOTH_MS, self.sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.line_l.clear();
        self.line_r.clear();
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        match id.0 {
            P_MODE => self.sync = plain >= 0.5,
            P_MS => self.time_ms = plain,
            P_SYNC => self.sync_index = (plain.max(0.0) as usize).min(SYNC_BEATS.len() - 1),
            P_FEEDBACK => self.feedback.set_target(plain.clamp(0.0, 0.95)),
            P_MIX => self.mix.set_target(plain.clamp(0.0, 1.0)),
            P_PING_PONG => self.ping_pong = plain >= 0.5,
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let sr = self.sample_rate;
        let base_ms = if self.sync {
            (SYNC_BEATS[self.sync_index] * 60_000.0 / ctx.time.tempo.bpm()) as f32
        } else {
            self.time_ms
        };
        let to_samples = |ms: f32| ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS) * sr / 1_000.0;
        let fixed_target = to_samples(base_ms);
        let cv_connected = ctx.is_connected(TIME_CV);
        let cv = *ctx.input(TIME_CV);
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        if self.fresh {
            let (f, m) = (self.feedback.target(), self.mix.target());
            self.feedback.set_immediate(f);
            self.mix.set_immediate(m);
            let first = if cv_connected {
                to_samples(base_ms * cv[0].clamp(-1.0, 1.0).exp2())
            } else {
                fixed_target
            };
            self.delay = first;
            self.glide_target = first;
            self.glide_rate = 0.0;
            self.fresh = false;
        }
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        for n in 0..BATCH {
            let target = if cv_connected {
                to_samples(base_ms * cv[n].clamp(-1.0, 1.0).exp2())
            } else {
                fixed_target
            };
            let d = self.next_delay(target);
            let fb = self.feedback.next_value();
            let mix = self.mix.next_value();
            let wet_l = self.line_l.read(d);
            let wet_r = self.line_r.read(d);
            let (x_l, x_r) = (in_l[n], in_r[n]);
            if self.ping_pong {
                self.line_l.write(0.5 * (x_l + x_r) + fb * wet_r);
                self.line_r.write(fb * wet_l);
            } else {
                self.line_l.write(x_l + fb * wet_l);
                self.line_r.write(x_r + fb * wet_r);
            }
            out_l[n] = x_l * (1.0 - mix) + wet_l * mix;
            out_r[n] = x_r * (1.0 - mix) + wet_r * mix;
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{SR, assert_keys, harness, sweep_params_min_max};
    use atomik_testkit::harness::DeviceHarness;
    use atomik_testkit::signal::{db, max_step, rms};

    const OUT: [&str; 2] = ["out_l", "out_r"];

    fn delay() -> DeviceHarness {
        harness(Box::new(StereoDelay::new()))
    }

    /// Feeds a unit impulse at frame 0 on `keys`, then silence; returns (L, R).
    fn impulse_response(
        h: &mut DeviceHarness,
        keys: &[&str],
        batches: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        for k in keys {
            h.input_mut(k)[0] = 1.0;
        }
        let mut o = h.render(1, &OUT);
        for k in keys {
            *h.input_mut(k) = [0.0; BATCH];
        }
        let rest = h.render(batches - 1, &OUT);
        o[0].extend_from_slice(&rest[0]);
        o[1].extend_from_slice(&rest[1]);
        (o[0].clone(), o[1].clone())
    }

    fn argmax_abs(x: &[f32]) -> usize {
        (0..x.len())
            .max_by(|a, b| x[*a].abs().total_cmp(&x[*b].abs()))
            .unwrap()
    }

    fn ms_mode(h: &mut DeviceHarness, ms: f32, feedback: f32, mix: f32) {
        h.set_param("time.mode", 0.0);
        h.set_param("time.ms", ms);
        h.set_param("feedback", feedback);
        h.set_param("mix", mix);
    }

    #[test]
    fn descriptor_validates() {
        assert_keys(
            &DESCRIPTOR,
            &[
                "time.mode",
                "time.ms",
                "time.sync",
                "feedback",
                "mix",
                "ping_pong",
            ],
            &["in_l", "in_r", "out_l", "out_r", "time_cv_in"],
        );
        assert_eq!(DESCRIPTOR.type_id, "atomik.stereo-delay");
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
        let s = DESCRIPTOR.param_by_key("time.sync").unwrap();
        assert!((s.default - 6.0).abs() < 1e-6 && s.discrete);
    }

    #[test]
    fn ac_d7_ms_mode_100ms_impulse_lands_at_frame_4800() {
        let mut h = delay();
        ms_mode(&mut h, 100.0, 0.0, 1.0);
        let (l, r) = impulse_response(&mut h, &["in_l", "in_r"], 100);
        for x in [&l, &r] {
            let at = argmax_abs(x);
            assert!(at.abs_diff(4_800) <= 1, "impulse at {at}");
            assert!((x[at] - 1.0).abs() < 1e-3, "amplitude {}", x[at]);
            let energy_elsewhere: f32 = x
                .iter()
                .enumerate()
                .filter(|(i, _)| i.abs_diff(4_800) > 1)
                .map(|(_, v)| v * v)
                .sum();
            assert!(energy_elsewhere < 1e-9);
        }
    }

    #[test]
    fn ac_d8_feedback_half_echoes_drop_6_02_db() {
        let mut h = delay();
        ms_mode(&mut h, 100.0, 0.5, 1.0);
        let (l, _) = impulse_response(&mut h, &["in_l", "in_r"], 400);
        let echoes: Vec<f32> = (1..=4).map(|k| l[4_800 * k].abs()).collect();
        for w in echoes.windows(2) {
            let d = db(w[1]) - db(w[0]);
            assert!((d + 6.02).abs() < 0.1, "echo ratio {d} dB ({echoes:?})");
        }
    }

    #[test]
    fn ac_d9_sync_quarter_at_120_bpm_is_half_a_second() {
        let mut h = delay(); // 120 BPM
        h.set_param("time.mode", 1.0);
        h.set_param("time.sync", 8.0); // 1/4
        h.set_param("feedback", 0.0);
        h.set_param("mix", 1.0);
        let (l, _) = impulse_response(&mut h, &["in_l", "in_r"], 400);
        let at = argmax_abs(&l);
        assert!(at.abs_diff(24_000) <= 1, "1/4 at 120 BPM: {at}");
    }

    #[test]
    fn process_sync_follows_tempo_and_note_values() {
        // 1/8D at 90 BPM = 0.75 beat · 60/90 s = 0.5 s; 1/16T at 90 BPM = 1/6·2/3 s.
        for (idx, expect) in [(6.0, 24_000usize), (1.0, 5_333)] {
            let mut h = DeviceHarness::with_options(Box::new(StereoDelay::new()), SR, 90.0, 0);
            h.set_param("time.sync", idx);
            h.set_param("feedback", 0.0);
            h.set_param("mix", 1.0);
            let (l, _) = impulse_response(&mut h, &["in_l"], 400);
            let at = argmax_abs(&l);
            assert!(at.abs_diff(expect) <= 1, "sync {idx}: {at}");
        }
    }

    #[test]
    fn process_time_cv_scales_by_power_of_two_and_clamps() {
        for (ms, cv, expect) in [
            (100.0, 1.0, 9_600usize),
            (100.0, -1.0, 2_400),
            (100.0, 5.0, 9_600),     // CV clamped to +1
            (3_000.0, 1.0, 192_000), // clamped to 4000 ms
        ] {
            let mut h = delay();
            ms_mode(&mut h, ms, 0.0, 1.0);
            *h.input_mut("time_cv_in") = [cv; BATCH];
            let (l, _) = impulse_response(&mut h, &["in_l"], 192_000 / BATCH + 10);
            let at = argmax_abs(&l);
            assert!(at.abs_diff(expect) <= 1, "{ms} ms, cv {cv}: {at}");
        }
    }

    #[test]
    fn process_ping_pong_alternates_sides() {
        let mut h = delay();
        ms_mode(&mut h, 100.0, 0.5, 1.0);
        h.set_param("ping_pong", 1.0);
        let (l, r) = impulse_response(&mut h, &["in_l"], 300);
        // Mono sum (L+R)/2 into the left line, feedback crosses sides.
        assert!((l[4_800] - 0.5).abs() < 1e-4 && r[4_800].abs() < 1e-6);
        assert!((r[9_600] - 0.25).abs() < 1e-4 && l[9_600].abs() < 1e-6);
        assert!((l[14_400] - 0.125).abs() < 1e-4 && r[14_400].abs() < 1e-6);
    }

    #[test]
    fn process_mix_zero_is_dry_and_mix_blends() {
        let mut h = delay();
        ms_mode(&mut h, 10.0, 0.0, 0.0);
        let (l, r) = impulse_response(&mut h, &["in_l", "in_r"], 20);
        assert!((l[0] - 1.0).abs() < 1e-6 && (r[0] - 1.0).abs() < 1e-6);
        assert!(l[1..].iter().all(|v| v.abs() < 1e-9));
        let mut h = delay();
        ms_mode(&mut h, 10.0, 0.0, 0.25);
        let (l, _) = impulse_response(&mut h, &["in_l"], 20);
        assert!((l[0] - 0.75).abs() < 1e-6 && (l[480] - 0.25).abs() < 1e-6);
    }

    fn sine_into(h: &mut DeviceHarness, phase: &mut f64) {
        let (l, r) = {
            let mut b = [0.0f32; BATCH];
            for s in &mut b {
                *s = (0.1 * (std::f64::consts::TAU * *phase).sin()) as f32;
                *phase = (*phase + 1_000.0 / SR).fract();
            }
            (b, b)
        };
        *h.input_mut("in_l") = l;
        *h.input_mut("in_r") = r;
    }

    #[test]
    fn ac_d10_time_change_has_no_clicks() {
        let mut h = delay();
        ms_mode(&mut h, 375.0, 0.4, 0.5);
        let mut phase = 0.0;
        let mut out = Vec::new();
        for i in 0..1_500 {
            match i {
                400 => h.set_param("time.ms", 500.0),
                700 => h.set_param("time.ms", 250.0),
                900 => *h.input_mut("time_cv_in") = [0.5; BATCH],
                1_100 => {
                    h.set_param("time.mode", 1.0); // 1/8D at 120 BPM = 375 ms
                    *h.input_mut("time_cv_in") = [0.0; BATCH];
                }
                _ => {}
            }
            sine_into(&mut h, &mut phase);
            h.process();
            out.extend_from_slice(h.output("out_l"));
        }
        let step = max_step(&out);
        assert!(step <= 0.1, "max step {step}");
        assert!(rms(&out) > 0.03, "signal present: rms {}", rms(&out));
    }

    #[test]
    fn set_param_every_param_min_max_does_not_panic() {
        let mut h = delay();
        *h.input_mut("in_l") = [0.5; BATCH];
        *h.input_mut("in_r") = [-0.5; BATCH];
        *h.input_mut("time_cv_in") = [1.0; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }

    #[test]
    fn reset_silences_tails() {
        let mut h = delay();
        ms_mode(&mut h, 50.0, 0.9, 1.0);
        let (l, _) = impulse_response(&mut h, &["in_l", "in_r"], 100);
        assert!(l[4_800..].iter().any(|v| v.abs() > 0.1), "tail exists");
        h.device_mut().reset();
        let o = h.render(400, &OUT);
        assert!(o[0].iter().chain(&o[1]).all(|v| v.to_bits() == 0));
    }

    #[test]
    fn process_is_allocation_free() {
        let mut h = delay();
        *h.input_mut("in_l") = [0.3; BATCH];
        *h.input_mut("in_r") = [-0.3; BATCH];
        for i in 0..400 {
            *h.input_mut("time_cv_in") = [((i % 7) as f32 - 3.0) / 3.0; BATCH];
            h.set_param("ping_pong", (i / 50 % 2) as f32);
            h.set_param("time.mode", (i / 100 % 2) as f32);
            h.set_param("time.sync", (i % 12) as f32);
            h.process();
        }
        h.device_mut().reset();
        h.process();
    }

    #[test]
    fn prepare_sizes_buffers_for_four_seconds_at_96k() {
        let mut h = DeviceHarness::new(Box::new(StereoDelay::new()), 96_000.0);
        ms_mode(&mut h, 4_000.0, 0.0, 1.0);
        let (l, _) = impulse_response(&mut h, &["in_l"], 384_000 / BATCH + 10);
        let at = argmax_abs(&l);
        assert!(at.abs_diff(384_000) <= 1, "{at}");
    }
}
