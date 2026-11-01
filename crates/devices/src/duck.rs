//! `atomik.duck`, the Sidechain Tool: pumping and ducking, from the song position, from a key
//! signal's hits, or from the key's level, with a CV output for ducking other things.
//!
//! The audio is multiplied by `gain = 1 - depth * dip`, where `dip` (0..1) is a curve that falls
//! to 1 over `attack` milliseconds and recovers to 0 over `time` milliseconds:
//!
//! ```text
//! t <  attack            dip = t / attack
//! t >= attack            dip = (1 - (t - attack) / time) ^ curve     (0 once t - attack >= time)
//! ```
//!
//! `curve` (0.25..4) bends the recovery: 1 is a straight line back, above 1 it hangs low and
//! comes back late, below 1 it comes back at once. What starts the curve depends on `mode`:
//!
//! * **pump.** The curve restarts every `rate` of the song (a half, quarter, eighth or sixteenth
//!   note): `t` is the time since the last multiple of the rate, computed from the transport
//!   position of the batch (`TimeInfo::tick`, plus the loop wrap), so it needs no history: playing
//!   from a locate gives the same gain as playing through to it, and a bounce is repeatable. The
//!   gain is 1 while the transport is stopped. No key is read.
//! * **trigger.** A hit on the key restarts the curve. The key is `sc_in_l/r` when connected,
//!   else the main input; its peak (instant rise, 10 ms fall) crossing `threshold` triggers, and
//!   it re-arms 6 dB below. A hit during a curve continues from the current dip instead of
//!   jumping, so retriggering does not click.
//! * **classic.** `dip` follows the key's level: the peak follower with `attack` and `time` as its
//!   rise and fall times, `dip = (clamp((level_dB - threshold) / 12, 0, 1)) ^ curve` (12 dB above
//!   the threshold ducks the full depth).
//!
//! * **send.** With `send` on the audio passes untouched and only `duck_cv_out` moves, to duck
//!   something else (cable it to another device's level CV with a negative trim).
//! * **`duck_cv_out`** is `depth * dip` (= `1 - gain`): 0 at rest, up to `depth` when fully ducked.
//! * `depth` is smoothed per batch (10 ms). The maths is the deterministic `log2_det` /
//!   `exp2_det`. No latency. Non-finite input samples count as 0; allocation-free.

use crate::bypass::{Bypassable, bypass_param};
use crate::tool_support::{audio_in, audio_out, choice, cv_out, expo, finite, linear, toggle};
use atomik_core::{BATCH, PPQ, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{BlockSmoother, exp2_det, exp2_det_f64, log2_det};

const P_MODE: u32 = 0;
const P_RATE: u32 = 1;
const P_DEPTH: u32 = 2;
const P_ATTACK: u32 = 3;
const P_TIME: u32 = 4;
const P_CURVE: u32 = 5;
const P_THRESHOLD: u32 = 6;
const P_SEND: u32 = 7;

static MODES: [&str; 3] = ["pump", "trigger", "classic"];
static RATES: [&str; 4] = ["1/2", "1/4", "1/8", "1/16"];

/// Length of one pump cycle in ticks for each entry of [`RATES`].
const RATE_TICKS: [f64; 4] = [
    2.0 * PPQ as f64,
    PPQ as f64,
    PPQ as f64 / 2.0,
    PPQ as f64 / 4.0,
];

/// `log2(10) / 20`: `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// `20 * log10(2)`: dB per octave of amplitude.
const DB_PER_LOG2: f32 = 6.020_6;
/// The key follower's fall in trigger mode, ms.
const TRIGGER_FALL_MS: f32 = 10.0;
/// The level range above the threshold that ducks the full depth in classic mode, dB.
const CLASSIC_RANGE_DB: f32 = 12.0;
/// A trigger re-arms once the key is this many dB under the threshold.
const REARM_DB: f32 = 6.0;

static PARAMS: [ParamDescriptor; 9] = [
    choice(P_MODE, "mode", "Mode", &MODES, 0.0),
    choice(P_RATE, "rate", "Rate", &RATES, 1.0),
    linear(P_DEPTH, "depth", "Depth", 0.0, 1.0, 0.8, Unit::Percent),
    expo(P_ATTACK, "attack", "Attack", 0.1, 50.0, 2.0, Unit::Ms),
    expo(P_TIME, "time", "Time", 10.0, 2_000.0, 250.0, Unit::Ms),
    expo(P_CURVE, "curve", "Curve", 0.25, 4.0, 1.0, Unit::None),
    linear(
        P_THRESHOLD,
        "threshold",
        "Threshold",
        -60.0,
        0.0,
        -30.0,
        Unit::Db,
    ),
    toggle(P_SEND, "send", "Send (CV only)", 0.0),
    bypass_param(8),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const SC_L: SocketId = SocketId(2);
const SC_R: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);
const DUCK_CV: SocketId = SocketId(6);

static SOCKETS: [SocketDescriptor; 7] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_in(2, "sc_in_l", "Sidechain L"),
    audio_in(3, "sc_in_r", "Sidechain R"),
    audio_out(4, "out_l", "Out L"),
    audio_out(5, "out_r", "Out R"),
    cv_out(6, "duck_cv_out", "Duck CV"),
];

/// Static description of the sidechain tool.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.duck",
    name: "Sidechain Tool",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Duck`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(Duck::new())),
    }
}

/// The dip (0..1) `t_ms` after the start of a curve: a fall over `attack_ms`, then a recovery
/// over `time_ms` bent by `curve`.
#[inline]
pub fn dip_at(t_ms: f64, attack_ms: f32, time_ms: f32, curve: f32) -> f32 {
    if t_ms < 0.0 {
        return 0.0;
    }
    let t = t_ms as f32;
    if t < attack_ms {
        return t / attack_ms;
    }
    let y = (t - attack_ms) / time_ms;
    if y >= 1.0 {
        0.0
    } else {
        exp2_det(curve * log2_det(1.0 - y))
    }
}

/// The curve time at which the fall has reached `dip` (a retrigger continues from there).
#[inline]
fn fall_time_for(dip: f32, attack_ms: f32) -> f32 {
    dip.clamp(0.0, 1.0) * attack_ms
}

/// The duck device.
#[derive(Debug, Clone)]
pub struct Duck {
    sample_rate: f32,
    mode: u8,
    rate: u8,
    depth: f32,
    attack_ms: f32,
    time_ms: f32,
    curve: f32,
    threshold_db: f32,
    send: bool,
    // Derived.
    thr_lin: f32,
    fall_alpha: f64,
    attack_alpha: f64,
    release_alpha: f64,
    // State.
    fresh: bool,
    depth_sm: BlockSmoother,
    /// Peak follower of the key.
    env: f32,
    armed: bool,
    /// Samples since the current curve began (trigger mode); `None` before the first hit.
    since: Option<u64>,
    /// Offset in ms added to `since` (a retrigger that continues from a partial dip).
    since_base_ms: f32,
}

impl Default for Duck {
    fn default() -> Self {
        Self::new()
    }
}

impl Duck {
    /// A sidechain tool with default params.
    pub fn new() -> Self {
        let mut d = Self {
            sample_rate: 48_000.0,
            mode: 0,
            rate: 1,
            depth: 0.8,
            attack_ms: 2.0,
            time_ms: 250.0,
            curve: 1.0,
            threshold_db: -30.0,
            send: false,
            thr_lin: 0.0,
            fall_alpha: 0.0,
            attack_alpha: 0.0,
            release_alpha: 0.0,
            fresh: true,
            depth_sm: BlockSmoother::new(0.8),
            env: 0.0,
            armed: true,
            since: None,
            since_base_ms: 0.0,
        };
        d.derive();
        d
    }

    fn alpha(&self, ms: f32) -> f64 {
        let n = f64::from(ms) * 0.001 * f64::from(self.sample_rate);
        if n > 0.0 {
            exp2_det_f64(-std::f64::consts::LOG2_E / n)
        } else {
            0.0
        }
    }

    fn derive(&mut self) {
        self.thr_lin = exp2_det(self.threshold_db * LOG2_10_20);
        self.fall_alpha = self.alpha(TRIGGER_FALL_MS);
        self.attack_alpha = self.alpha(self.attack_ms);
        self.release_alpha = self.alpha(self.time_ms);
    }

    /// Milliseconds since the current trigger curve began (`None` before any hit).
    fn trigger_ms(&self) -> Option<f64> {
        self.since.map(|s| {
            f64::from(self.since_base_ms) + s as f64 * 1000.0 / f64::from(self.sample_rate)
        })
    }
}

impl Device for Duck {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.depth_sm.set_time(10.0, self.sample_rate, BATCH);
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        self.env = 0.0;
        self.armed = true;
        self.since = None;
        self.since_base_ms = 0.0;
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_MODE => self.mode = v.clamp(0.0, 2.0).round() as u8,
            P_RATE => self.rate = v.clamp(0.0, 3.0).round() as u8,
            P_DEPTH => self.depth = v.clamp(0.0, 1.0),
            P_ATTACK => self.attack_ms = v.clamp(0.1, 50.0),
            P_TIME => self.time_ms = v.clamp(10.0, 2_000.0),
            P_CURVE => self.curve = v.clamp(0.25, 4.0),
            P_THRESHOLD => self.threshold_db = v.clamp(-60.0, 0.0),
            P_SEND => self.send = v >= 0.5,
            _ => return,
        }
        self.derive();
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let (sc_l_conn, sc_r_conn) = (ctx.is_connected(SC_L), ctx.is_connected(SC_R));
        let sc_l = *ctx.input(SC_L);
        let sc_r = *ctx.input(SC_R);
        let time = ctx.time;
        if self.fresh {
            self.depth_sm.set_immediate(self.depth);
            self.fresh = false;
        }
        let depth = self.depth_sm.advance(self.depth, BATCH);

        let cycle = RATE_TICKS[usize::from(self.rate)];
        let tpf = time.ticks_per_frame.max(1e-12);
        let sr = f64::from(self.sample_rate);
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut cv = [0.0f32; BATCH];
        for n in 0..BATCH {
            let x = [finite(in_l[n]), finite(in_r[n])];
            let dip = match self.mode {
                0 => {
                    if time.playing {
                        let tick = match time.loop_wrap {
                            Some(w) if n >= usize::from(w.offset) => {
                                w.to_tick + (n - usize::from(w.offset)) as f64 * tpf
                            }
                            _ => time.tick + n as f64 * tpf,
                        };
                        let phase = tick.rem_euclid(cycle);
                        let t_ms = phase / tpf / sr * 1000.0;
                        dip_at(t_ms, self.attack_ms, self.time_ms, self.curve)
                    } else {
                        0.0
                    }
                }
                _ => {
                    // The key: a connected sidechain (a lone side feeds both), else the input.
                    let k = if sc_l_conn || sc_r_conn {
                        let (a, b) = (finite(sc_l[n]), finite(sc_r[n]));
                        match (sc_l_conn, sc_r_conn) {
                            (true, false) => a.abs(),
                            (false, true) => b.abs(),
                            _ => a.abs().max(b.abs()),
                        }
                    } else {
                        x[0].abs().max(x[1].abs())
                    };
                    if self.mode == 1 {
                        self.env = k.max((f64::from(self.env) * self.fall_alpha) as f32);
                        if self.env < 1e-20 {
                            self.env = 0.0;
                        }
                        if self.armed && self.env >= self.thr_lin {
                            let current = self.trigger_ms().map_or(0.0, |t| {
                                dip_at(t, self.attack_ms, self.time_ms, self.curve)
                            });
                            self.armed = false;
                            self.since = Some(0);
                            self.since_base_ms = fall_time_for(current, self.attack_ms);
                        } else if !self.armed
                            && self.env < self.thr_lin * exp2_det(-REARM_DB * LOG2_10_20)
                        {
                            self.armed = true;
                        }
                        let dip = self
                            .trigger_ms()
                            .map_or(0.0, |t| dip_at(t, self.attack_ms, self.time_ms, self.curve));
                        if let Some(s) = self.since.as_mut() {
                            *s = s.saturating_add(1);
                        }
                        dip
                    } else {
                        let a = if k > self.env {
                            self.attack_alpha
                        } else {
                            self.release_alpha
                        };
                        self.env = (f64::from(k) + a * (f64::from(self.env) - f64::from(k))) as f32;
                        if self.env < 1e-20 {
                            self.env = 0.0;
                        }
                        let db = DB_PER_LOG2 * log2_det(self.env);
                        let over = ((db - self.threshold_db) / CLASSIC_RANGE_DB).clamp(0.0, 1.0);
                        if over <= 0.0 {
                            0.0
                        } else {
                            exp2_det(self.curve * log2_det(over))
                        }
                    }
                }
            };
            let duck = depth.at(n) * dip;
            cv[n] = duck;
            if self.send {
                out_l[n] = x[0];
                out_r[n] = x[1];
            } else {
                let g = 1.0 - duck;
                out_l[n] = x[0] * g;
                out_r[n] = x[1] * g;
            }
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
        *ctx.output(DUCK_CV) = cv;
    }
}

#[cfg(test)]
mod tests;
