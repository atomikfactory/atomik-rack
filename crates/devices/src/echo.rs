//! `atomik.echo`: a stereo echo whose repeats can be coloured, wobbled, diffused, crushed and
//! ducked, with three modes.
//!
//! # Signal flow (per channel, per sample)
//!
//! ```text
//! in -> + ----------------------> [ delay line (dsp::Echo, glided time, wobble) ] -> wet
//!       ^                                                  |
//!       +-- x feedback <- diffusion <- crush <- tone <- drive <-+
//! out = dry * (1 - mix) + wet * duck gain * trigger gate * mix
//! ```
//!
//! * **Time** is `time.ms`, or the `time.sync` note value at the transport tempo (the same table
//!   as the stereo delay). A change glides: the sounding time moves by at most 0.25 samples per
//!   sample, so a jump bends the pitch of the repeats by at most 25 %. The line holds 4 s.
//! * **Colour** shapes every pass through the loop: `color.drive` (0..1, up to +12 dB) is a soft
//!   saturator `u / sqrt(1 + u^2) / g` (unity small-signal gain, so it never raises the loop
//!   gain); `color.low` and `color.high` are one-pole high-pass and low-pass filters (off at
//!   20 Hz and 20 kHz), so repeats grow darker or thinner.
//! * **Crush** (`crush.bits` below 16) quantises the loop to that many bits.
//! * **Diffusion** (0..1) runs two Schroeder allpass sections (113 and 173 samples at 48 kHz,
//!   coefficient `0.6 diffusion`) in the loop: repeats smear into a wash. They add a few
//!   milliseconds to each pass; at 0 they are skipped (no delay).
//! * **Wobble** (`wobble.depth` 0..1 for up to 8 ms of time modulation, `wobble.rate` 0.1..8 Hz)
//!   is the delay line's own sine modulation, inverted between the channels.
//! * **Duck** (0..1) lowers the repeats while the input is loud: the wet gain is
//!   `1 - duck * min(1, 4 * env)`, `env` a follower of the louder channel (5 ms attack, 150 ms
//!   release). The loop itself keeps running underneath.
//! * **Ping-pong** sums the input to mono into the left line and crosses the feedback.
//! * **Mode**: `normal`; `triggered`: the repeats are heard only while the `trigger_in` cable is
//!   high (above 0.5, 2 ms fade; unconnected, always heard); `roll`: the input is cut off and the
//!   feedback is 100 %, so the loop content repeats without decay (colour, crush and diffusion
//!   still act, so those make it evolve).
//! * `feedback` and `mix` are smoothed over 5 ms. The feedback is at most 0.95, every stage in
//!   the loop has a gain of at most 1, so the loop is stable (a `roll` held forever stays bounded).
//! * `mix = 0` is the dry input, bit for bit. Latency is 0. The buffers are allocated in
//!   `prepare`.

use crate::bypass::{Bypassable, bypass_param};
use crate::stereo_delay::SYNC_BEATS;
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{Allpass, Smoother, exp2_det, exp2_det_f64};

/// Longest echo time in ms.
pub const MAX_ECHO_MS: f32 = 4_000.0;

const P_MODE: u32 = 0;
const P_MS: u32 = 1;
const P_SYNC: u32 = 2;
const P_FEEDBACK: u32 = 3;
const P_MIX: u32 = 4;
const P_ECHO_MODE: u32 = 5;
const P_DRIVE: u32 = 6;
const P_LOW: u32 = 7;
const P_HIGH: u32 = 8;
const P_DIFFUSION: u32 = 9;
const P_WOBBLE_DEPTH: u32 = 10;
const P_WOBBLE_RATE: u32 = 11;
const P_BITS: u32 = 12;
const P_DUCK: u32 = 13;
const P_PING_PONG: u32 = 14;

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
const TRIGGER: SocketId = SocketId(4);

/// Wobble depth in ms at `wobble.depth = 1`.
const WOBBLE_MAX_MS: f32 = 8.0;
/// Drive in dB at `color.drive = 1`.
const DRIVE_DB: f32 = 12.0;
const LOG2_10_20: f32 = 0.166_096_4;
/// The low and high corners count as off at or beyond these.
const LOW_OFF_HZ: f32 = 20.5;
const HIGH_OFF_HZ: f32 = 19_500.0;
/// Allpass delays at 48 kHz.
const DIFFUSER_DELAYS: [f32; 2] = [113.0, 173.0];
const SMOOTH_MS: f32 = 5.0;

static PARAMS: [ParamDescriptor; 16] = [
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
        default: 5.0,
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
        id: ParamId(P_ECHO_MODE),
        key: "mode",
        name: "Mode",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&["normal", "triggered", "roll"]),
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_DRIVE),
        key: "color.drive",
        name: "Drive",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_LOW),
        key: "color.low",
        name: "Low cut",
        min: 20.0,
        max: 2_000.0,
        default: 20.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_HIGH),
        key: "color.high",
        name: "High cut",
        min: 500.0,
        max: 20_000.0,
        default: 20_000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_DIFFUSION),
        key: "diffusion",
        name: "Diffusion",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_WOBBLE_DEPTH),
        key: "wobble.depth",
        name: "Wobble",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_WOBBLE_RATE),
        key: "wobble.rate",
        name: "Wobble rate",
        min: 0.1,
        max: 8.0,
        default: 0.6,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_BITS),
        key: "crush.bits",
        name: "Crush bits",
        min: 2.0,
        max: 16.0,
        default: 16.0,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_DUCK),
        key: "duck",
        name: "Duck",
        min: 0.0,
        max: 1.0,
        default: 0.0,
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
        unit: Unit::Choice(&["off", "on"]),
        discrete: true,
    },
    bypass_param(15),
];

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
        id: TRIGGER,
        key: "trigger_in",
        name: "Trigger",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
];

/// Static description of the echo.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.echo",
    name: "Echo",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Echo`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(Echo::new())),
    }
}

/// How the repeats are produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Triggered,
    Roll,
}

/// One channel's loop state.
#[derive(Debug, Clone)]
struct Loop {
    lp: f64,
    hp: f64,
    diffusers: [Allpass; 2],
}

impl Loop {
    fn new(sr: f32) -> Self {
        let size = |d: f32| ((d * sr / 48_000.0).round() as usize).max(2);
        Self {
            lp: 0.0,
            hp: 0.0,
            diffusers: [
                Allpass::new(size(DIFFUSER_DELAYS[0])),
                Allpass::new(size(DIFFUSER_DELAYS[1])),
            ],
        }
    }

    fn reset(&mut self) {
        self.lp = 0.0;
        self.hp = 0.0;
        for a in &mut self.diffusers {
            a.clear();
        }
    }
}

/// The echo device.
#[derive(Debug, Clone)]
pub struct Echo {
    sample_rate: f32,
    line: atomik_dsp::Echo,
    loops: [Loop; 2],
    sync: bool,
    time_ms: f32,
    sync_index: usize,
    mode: Mode,
    ping_pong: bool,
    drive: f32,
    low_hz: f32,
    high_hz: f32,
    diffusion: f32,
    wobble_depth: f32,
    wobble_rate: f32,
    bits: f32,
    duck: f32,
    feedback: Smoother,
    mix: Smoother,
    /// One-pole coefficients of the loop filters (`1 - exp(-2 pi f / sr)`).
    low_k: f64,
    high_k: f64,
    /// Duck follower, its coefficients, and the trigger gate with its coefficient.
    env: f32,
    env_attack: f32,
    env_release: f32,
    gate: f32,
    gate_k: f32,
    fresh: bool,
}

impl Default for Echo {
    fn default() -> Self {
        Self::new()
    }
}

impl Echo {
    /// An echo with default params at 48 kHz (`prepare` sizes the buffers).
    pub fn new() -> Self {
        let mut e = Self {
            sample_rate: 48_000.0,
            line: atomik_dsp::Echo::new(48_000.0, MAX_ECHO_MS),
            loops: [Loop::new(48_000.0), Loop::new(48_000.0)],
            sync: true,
            time_ms: 375.0,
            sync_index: 5,
            mode: Mode::Normal,
            ping_pong: false,
            drive: 0.0,
            low_hz: 20.0,
            high_hz: 20_000.0,
            diffusion: 0.0,
            wobble_depth: 0.0,
            wobble_rate: 0.6,
            bits: 16.0,
            duck: 0.0,
            feedback: Smoother::new(0.4, SMOOTH_MS, 48_000.0),
            mix: Smoother::new(0.3, SMOOTH_MS, 48_000.0),
            low_k: 0.0,
            high_k: 1.0,
            env: 0.0,
            env_attack: 0.0,
            env_release: 0.0,
            gate: 1.0,
            gate_k: 0.0,
            fresh: true,
        };
        e.setup(48_000.0);
        e
    }

    fn setup(&mut self, sr: f32) {
        self.sample_rate = sr;
        self.feedback.set_time(SMOOTH_MS, sr);
        self.mix.set_time(SMOOTH_MS, sr);
        self.apply_filters();
        self.apply_wobble();
        let coef = |ms: f32| 1.0 - exp2_det(-1.0 / (ms * 0.001 * sr) * std::f32::consts::LOG2_E);
        self.env_attack = coef(5.0);
        self.env_release = coef(150.0);
        self.gate_k = coef(2.0);
    }

    fn one_pole_k(&self, hz: f32) -> f64 {
        let w = std::f64::consts::TAU * f64::from(hz) / f64::from(self.sample_rate);
        1.0 - exp2_det_f64(-w * std::f64::consts::LOG2_E)
    }

    fn apply_filters(&mut self) {
        self.low_k = self.one_pole_k(self.low_hz);
        self.high_k = self.one_pole_k(self.high_hz);
    }

    fn apply_wobble(&mut self) {
        self.line
            .set_modulation(self.wobble_rate, self.wobble_depth * WOBBLE_MAX_MS);
    }

    /// One pass of `v` through the loop's colour, crush and diffusion for channel `c`.
    #[inline]
    fn loop_pass(&mut self, c: usize, v: f32, drive_g: f32, levels: f32) -> f32 {
        let mut v = v;
        if drive_g > 1.0 {
            let u = v * drive_g;
            v = u / (1.0 + u * u).sqrt() / drive_g;
        }
        let st = &mut self.loops[c];
        if self.high_hz < HIGH_OFF_HZ {
            st.lp += self.high_k * (f64::from(v) - st.lp);
            if st.lp.abs() < 1e-30 {
                st.lp = 0.0;
            }
            v = st.lp as f32;
        }
        if self.low_hz > LOW_OFF_HZ {
            st.hp += self.low_k * (f64::from(v) - st.hp);
            if st.hp.abs() < 1e-30 {
                st.hp = 0.0;
            }
            v -= st.hp as f32;
        }
        if levels > 0.0 {
            v = (v.clamp(-1.0, 1.0) * levels).round() / levels;
        }
        if self.diffusion > 0.0 {
            let g = 0.6 * self.diffusion;
            for a in &mut st.diffusers {
                v = a.process(v, g);
            }
        }
        v
    }
}

impl Device for Echo {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.line = atomik_dsp::Echo::new(sr, MAX_ECHO_MS);
        self.loops = [Loop::new(sr), Loop::new(sr)];
        self.setup(sr);
        self.reset();
    }

    fn reset(&mut self) {
        self.line.reset();
        for l in &mut self.loops {
            l.reset();
        }
        self.env = 0.0;
        self.gate = 1.0;
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        match id.0 {
            P_MODE => self.sync = plain >= 0.5,
            P_MS => self.time_ms = plain.clamp(1.0, MAX_ECHO_MS),
            P_SYNC => self.sync_index = (plain.max(0.0) as usize).min(SYNC_BEATS.len() - 1),
            P_FEEDBACK => self.feedback.set_target(plain.clamp(0.0, 0.95)),
            P_MIX => self.mix.set_target(plain.clamp(0.0, 1.0)),
            P_ECHO_MODE => {
                self.mode = match plain.round() as i32 {
                    0 => Mode::Normal,
                    1 => Mode::Triggered,
                    _ => Mode::Roll,
                }
            }
            P_DRIVE => self.drive = plain.clamp(0.0, 1.0),
            P_LOW => {
                self.low_hz = plain.clamp(20.0, 2_000.0);
                self.apply_filters();
            }
            P_HIGH => {
                self.high_hz = plain.clamp(500.0, 20_000.0);
                self.apply_filters();
            }
            P_DIFFUSION => self.diffusion = plain.clamp(0.0, 1.0),
            P_WOBBLE_DEPTH => {
                self.wobble_depth = plain.clamp(0.0, 1.0);
                self.apply_wobble();
            }
            P_WOBBLE_RATE => {
                self.wobble_rate = plain.clamp(0.1, 8.0);
                self.apply_wobble();
            }
            P_BITS => self.bits = plain.round().clamp(2.0, 16.0),
            P_DUCK => self.duck = plain.clamp(0.0, 1.0),
            P_PING_PONG => self.ping_pong = plain >= 0.5,
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let trig = *ctx.input(TRIGGER);
        let trig_on = ctx.is_connected(TRIGGER) && self.mode == Mode::Triggered;
        let ms = if self.sync {
            (SYNC_BEATS[self.sync_index] * 60_000.0 / ctx.time.tempo.bpm()) as f32
        } else {
            self.time_ms
        };
        self.line.set_time_ms(ms);
        if self.fresh {
            self.line.snap_time();
            self.feedback.set_immediate(self.feedback.target());
            self.mix.set_immediate(self.mix.target());
            self.fresh = false;
        }
        let drive_g = if self.drive > 0.0 {
            exp2_det(self.drive * DRIVE_DB * LOG2_10_20)
        } else {
            1.0
        };
        let levels = if self.bits < 15.5 {
            exp2_det(self.bits - 1.0)
        } else {
            0.0
        };
        let roll = self.mode == Mode::Roll;
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        for n in 0..BATCH {
            let fb_knob = self.feedback.next_value();
            let fb = if roll { 1.0 } else { fb_knob };
            let mix = self.mix.next_value();
            let (x_l, x_r) = (in_l[n], in_r[n]);
            let (wl, wr) = self.line.read_pair();

            // Duck follower on the louder input channel.
            let lvl = x_l.abs().max(x_r.abs());
            let k = if lvl > self.env {
                self.env_attack
            } else {
                self.env_release
            };
            self.env += k * (lvl - self.env);
            let duck_g = 1.0 - self.duck * (4.0 * self.env).min(1.0);
            // Trigger gate.
            let gate_target = if trig_on && trig[n] <= 0.5 { 0.0 } else { 1.0 };
            self.gate += self.gate_k * (gate_target - self.gate);
            let wet_g = duck_g * self.gate;

            // The loop.
            let (fl, fr) = if self.ping_pong { (wr, wl) } else { (wl, wr) };
            let pl = self.loop_pass(0, fl, drive_g, levels);
            let pr = self.loop_pass(1, fr, drive_g, levels);
            let (src_l, src_r) = if roll {
                (0.0, 0.0)
            } else if self.ping_pong {
                (0.5 * (x_l + x_r), 0.0)
            } else {
                (x_l, x_r)
            };
            self.line.write_pair(src_l + fb * pl, src_r + fb * pr);

            out_l[n] = x_l * (1.0 - mix) + wl * wet_g * mix;
            out_r[n] = x_r * (1.0 - mix) + wr * wet_g * mix;
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{assert_keys, harness, run_signal, sine, sweep_params_min_max};
    use atomik_dsp::Rng;
    use atomik_testkit::harness::DeviceHarness;

    const IN: [&str; 2] = ["in_l", "in_r"];

    fn echo() -> DeviceHarness {
        let mut h = harness(Box::new(Echo::new()));
        for (k, v) in [("time.mode", 0.0), ("time.ms", 100.0), ("mix", 1.0)] {
            h.set_param(k, v);
        }
        h
    }

    fn bits(v: &[f32]) -> Vec<u32> {
        v.iter().map(|x| x.to_bits()).collect()
    }

    fn impulse(h: &mut DeviceHarness, samples: usize) -> Vec<f32> {
        run_signal(
            h,
            &IN,
            |n| if n == 0 { 1.0 } else { 0.0 },
            samples / BATCH,
            &["out_l"],
        )
        .remove(0)
    }

    fn rms(y: &[f32]) -> f32 {
        (y.iter().map(|v| v * v).sum::<f32>() / y.len() as f32).sqrt()
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
                "mode",
                "color.drive",
                "color.low",
                "color.high",
                "diffusion",
                "wobble.depth",
                "wobble.rate",
                "crush.bits",
                "duck",
                "ping_pong",
                "bypass",
            ],
            &["in_l", "in_r", "out_l", "out_r", "trigger_in"],
        );
        assert_eq!(DESCRIPTOR.type_id, "atomik.echo");
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    }

    #[test]
    fn repeats_sit_at_exact_multiples_with_feedback_powers() {
        let mut h = echo();
        h.set_param("feedback", 0.5);
        let y = impulse(&mut h, 4800 * 4 + 64);
        for k in 1..4usize {
            assert_eq!(y[4800 * k], 0.5f32.powi(k as i32 - 1), "repeat {k}");
        }
        // No dry signal at mix 1, nothing between the repeats.
        assert_eq!(y[0], 0.0);
        assert!(
            y.iter()
                .enumerate()
                .all(|(i, v)| i % 4800 == 0 || *v == 0.0)
        );
    }

    #[test]
    fn mix_zero_is_the_dry_input_bit_for_bit() {
        let mut h = echo();
        h.set_param("mix", 0.0);
        h.set_param("feedback", 0.9);
        h.set_param("color.drive", 1.0);
        let mut rng = Rng::new(3);
        let x: Vec<f32> = (0..2_000).map(|_| rng.next_bipolar()).collect();
        let y = run_signal(
            &mut h,
            &IN,
            |n| x.get(n).copied().unwrap_or(0.0),
            30,
            &["out_l"],
        );
        for (a, b) in x.iter().zip(&y[0]) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }

    #[test]
    fn the_loop_filters_darken_and_thin_the_repeats() {
        let level = |params: &[(&str, f32)], hz: f64| {
            let mut h = echo();
            for (k, v) in params {
                h.set_param(k, *v);
            }
            h.set_param("feedback", 0.8);
            let y = run_signal(&mut h, &IN, sine(hz, 0.3), 1_500, &["out_l"]);
            let s = &y[0][4800 * 12..4800 * 14];
            s.iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        let flat = level(&[], 8_000.0);
        let dark = level(&[("color.high", 1_000.0)], 8_000.0);
        assert!(dark < flat * 0.5, "dark {dark} vs {flat}");
        let flat_lo = level(&[], 60.0);
        let thin = level(&[("color.low", 1_000.0)], 60.0);
        assert!(thin < flat_lo * 0.5, "thin {thin} vs {flat_lo}");
    }

    #[test]
    fn drive_crush_and_diffusion_change_the_repeats_but_stay_bounded() {
        let base = {
            let mut h = echo();
            h.set_param("feedback", 0.6);
            impulse(&mut h, 4800 * 4 + 64)
        };
        for (k, v) in [
            ("color.drive", 1.0f32),
            ("crush.bits", 3.0),
            ("diffusion", 1.0),
        ] {
            let mut h = echo();
            h.set_param("feedback", 0.6);
            h.set_param(k, v);
            let y = impulse(&mut h, 4800 * 4 + 64);
            assert_ne!(bits(&y), bits(&base), "{k}");
            assert!(y.iter().all(|v| v.is_finite() && v.abs() <= 1.0), "{k}");
        }
        // Diffusion smears the recirculating repeats over several samples, not the first one.
        let mut h = echo();
        h.set_param("feedback", 0.8);
        h.set_param("diffusion", 1.0);
        let y = impulse(&mut h, 4800 * 8 + 64);
        let tail = y[4800 * 6 - 400..4800 * 6 + 1_000]
            .iter()
            .filter(|v| v.abs() > 1e-4)
            .count();
        assert!(tail > 10, "diffused repeat spans {tail} samples");
    }

    #[test]
    fn wobble_modulates_the_time_and_is_deterministic() {
        let run = |depth: f32| {
            let mut h = echo();
            h.set_param("feedback", 0.0);
            h.set_param("wobble.depth", depth);
            h.set_param("wobble.rate", 5.0);
            run_signal(&mut h, &IN, sine(1_000.0, 0.5), 400, &["out_l"]).remove(0)
        };
        let a = run(1.0);
        assert_eq!(bits(&a), bits(&run(1.0)));
        assert_ne!(bits(&a), bits(&run(0.0)));
    }

    #[test]
    fn ducking_lowers_the_repeats_while_the_input_is_loud() {
        let level = |duck: f32| {
            let mut h = echo();
            h.set_param("time.ms", 10.0);
            h.set_param("feedback", 0.0);
            h.set_param("duck", duck);
            let y = run_signal(&mut h, &IN, sine(200.0, 0.6), 600, &["out_l"]).remove(0);
            rms(&y[y.len() / 2..])
        };
        let (open, ducked) = (level(0.0), level(1.0));
        assert!(open > 0.3 && ducked < open * 0.2, "{open} vs {ducked}");
    }

    #[test]
    fn the_trigger_gate_decides_when_the_repeats_are_heard() {
        let level_with = |gate: f32| {
            let mut h = echo();
            h.set_param("mode", 1.0);
            h.set_param("time.ms", 10.0);
            h.set_param("feedback", 0.0);
            h.set_connected("trigger_in", true);
            // run_signal never touches the cv input, so it keeps this value for every batch.
            *h.input_mut("trigger_in") = [gate; BATCH];
            let y = run_signal(&mut h, &IN, sine(200.0, 0.6), 300, &["out_l"]).remove(0);
            rms(&y[y.len() / 2..])
        };
        assert!(level_with(1.0) > 0.3);
        assert!(level_with(0.0) < 0.01);
        // Unconnected, triggered behaves like normal.
        let mut h = echo();
        h.set_param("mode", 1.0);
        h.set_param("time.ms", 10.0);
        h.set_param("feedback", 0.0);
        let y = run_signal(&mut h, &IN, sine(200.0, 0.6), 300, &["out_l"]).remove(0);
        assert!(y[y.len() / 2..].iter().any(|v| v.abs() > 0.3));
    }

    #[test]
    fn roll_holds_the_loop_without_decay() {
        let mut h = echo();
        h.set_param("feedback", 0.0);
        let mut rng = Rng::new(9);
        let burst: Vec<f32> = (0..4800).map(|_| rng.next_bipolar() * 0.5).collect();
        // Fill the line with one loop of noise, then roll with the input still arriving.
        let first = run_signal(&mut h, &IN, |n| burst[n], 75, &["out_l"]).remove(0);
        h.set_param("mode", 2.0);
        let rolled = run_signal(&mut h, &IN, |_| 0.9, 75 * 6, &["out_l"]).remove(0);
        assert!(first.iter().all(|v| *v == 0.0));
        // The input is cut off: the noise comes round again, unchanged, every 4800 samples.
        for k in 0..5usize {
            for n in 0..4800usize {
                assert_eq!(
                    rolled[4800 * k + n].to_bits(),
                    rolled[4800 * (k + 1) + n].to_bits(),
                    "pass {k} sample {n}"
                );
            }
        }
        assert!(rolled[..4800].iter().any(|v| v.abs() > 0.3));
    }

    #[test]
    fn the_loop_is_stable_at_every_extreme() {
        let mut h = echo();
        for (k, v) in [
            ("feedback", 0.95),
            ("color.drive", 1.0),
            ("diffusion", 1.0),
            ("crush.bits", 4.0),
            ("wobble.depth", 1.0),
            ("ping_pong", 1.0),
            ("time.ms", 7.0),
        ] {
            h.set_param(k, v);
        }
        let mut rng = Rng::new(21);
        let noise: Vec<f32> = (0..4_000).map(|_| rng.next_bipolar()).collect();
        let y = run_signal(
            &mut h,
            &IN,
            |n| noise.get(n).copied().unwrap_or(0.0),
            800,
            &["out_l"],
        );
        assert!(y[0].iter().all(|v| v.is_finite() && v.abs() < 4.0));
        // Roll at a feedback of 100 % stays bounded for a long time.
        h.set_param("mode", 2.0);
        let z = run_signal(&mut h, &IN, |_| 0.0, 2_000, &["out_l"]);
        assert!(z[0].iter().all(|v| v.is_finite() && v.abs() < 4.0));
    }

    #[test]
    fn a_normal_loop_decays() {
        let mut h = echo();
        h.set_param("feedback", 0.8);
        h.set_param("time.ms", 20.0);
        let y = impulse(&mut h, 48_000 * 2);
        let peak = |a: usize, b: usize| y[a..b].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak(48_000, 96_000) < peak(0, 48_000) * 0.2);
    }

    #[test]
    fn sync_follows_the_tempo() {
        let mut h = harness(Box::new(Echo::new()));
        h.set_param("mix", 1.0);
        h.set_param("feedback", 0.0);
        h.set_param("time.mode", 1.0);
        h.set_param("time.sync", 8.0); // 1/4
        let bpm = h.time_mut().tempo.bpm();
        let want = (60.0 / bpm * 48_000.0).round() as usize;
        let y = impulse(&mut h, want + 128);
        let at = y
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        assert_eq!(at, want);
    }

    #[test]
    fn params_sweep_and_automation_stay_finite_and_allocation_free() {
        let mut h = harness(Box::new(Echo::new()));
        *h.input_mut("in_l") = [0.4; BATCH];
        *h.input_mut("in_r") = [-0.4; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
        let mut rng = Rng::new(4);
        for i in 0..400 {
            let mut x = [0.0f32; BATCH];
            for v in &mut x {
                *v = rng.next_bipolar();
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            h.set_param("time.ms", 1.0 + (i % 50) as f32 * 40.0);
            h.set_param("mode", (i / 13 % 3) as f32);
            h.set_param("diffusion", (i % 7) as f32 / 6.0);
            h.set_param("color.low", 20.0 + (i % 9) as f32 * 200.0);
            h.set_param("color.high", 500.0 + (i % 9) as f32 * 2_000.0);
            h.process();
            assert!(h.output("out_l").iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn two_instances_agree_bit_for_bit_and_a_reset_restores_the_start() {
        let run = |h: &mut DeviceHarness| {
            let mut rng = Rng::new(17);
            let noise: Vec<f32> = (0..300 * BATCH).map(|_| rng.next_bipolar() * 0.5).collect();
            run_signal(h, &IN, |n| noise[n], 300, &["out_l", "out_r"])
        };
        let setup = |h: &mut DeviceHarness| {
            for (k, v) in [
                ("wobble.depth", 0.5),
                ("diffusion", 0.5),
                ("color.drive", 0.5),
            ] {
                h.set_param(k, v);
            }
        };
        let (mut a, mut b) = (echo(), echo());
        setup(&mut a);
        setup(&mut b);
        let (ya, yb) = (run(&mut a), run(&mut b));
        assert_eq!(bits(&ya[0]), bits(&yb[0]));
        a.device_mut().reset();
        let yc = run(&mut a);
        assert_eq!(bits(&ya[1]), bits(&yc[1]));
    }
}
