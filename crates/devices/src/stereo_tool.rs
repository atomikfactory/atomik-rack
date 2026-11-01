//! `atomik.stereo-tool`: polarity, width, balance, mono and mid/side conversion for a stereo
//! signal.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! in -> polarity (invert L / R) -> width -> balance -> M/S mode -> mono -> out
//! ```
//!
//! * **Polarity.** `invert_l` / `invert_r` multiply a channel by -1 (a smoothed -1..+1 ramp, so
//!   the switch does not click).
//! * **Width.** The mid/side width of `atomik_dsp::stereo_width`: `M = (L + R) / 2`,
//!   `S = (L - R) / 2 * width`. 0 is mono, 1 leaves the signal alone, 2 doubles the side signal.
//!   With `low_bypass` above 20 Hz the signal is first split into a low and a high band by a
//!   fourth-order Linkwitz-Riley crossover at that frequency per channel
//!   ([`atomik_dsp::Crossover`]); only the high band is widened and the low band is added back
//!   as it was, so the bass stays centred when the width grows. At `width` 1 the whole stage is
//!   skipped, so **width 1 (with everything else at rest) is the input bit for bit**.
//! * **Balance** (-1..+1): unity at the centre, the far side falls linearly to silence at the end.
//! * **M/S mode.** `stereo` leaves the signal as it is. `encode` writes the mid `(L + R) / 2` to
//!   `out_l` and the side `(L - R) / 2` to `out_r`; `decode` reads `in_l` as the mid and `in_r`
//!   as the side and writes `L = M + S`, `R = M - S`. The mode switches at once (a hard cut).
//! * **Mono** (switch, smoothed over 10 ms), the last stage: both output channels become
//!   `(L + R) / 2` of what the mode produced, so the output is mono in every mode.
//! * **Smoothing.** Width, balance, polarity and mono are smoothed per batch (10 ms,
//!   `BlockSmoother`) and ramped linearly across it.
//! * No latency. Non-finite input samples count as 0. Allocation-free after construction.

use crate::bypass::{Bypassable, bypass_param};
use crate::tool_support::{
    audio_in, audio_out, balance_gains, choice, expo, finite, linear, toggle,
};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{BlockSmoother, Crossover};

const P_MODE: u32 = 0;
const P_INVERT_L: u32 = 1;
const P_INVERT_R: u32 = 2;
const P_WIDTH: u32 = 3;
const P_LOW_BYPASS: u32 = 4;
const P_BALANCE: u32 = 5;
const P_MONO: u32 = 6;

/// Corner at or below which the low-band bypass is off.
const LOW_BYPASS_OFF_HZ: f32 = 20.5;

static MODES: [&str; 3] = ["stereo", "encode", "decode"];

static PARAMS: [ParamDescriptor; 8] = [
    choice(P_MODE, "mode", "Mid/side", &MODES, 0.0),
    toggle(P_INVERT_L, "invert_l", "Invert L", 0.0),
    toggle(P_INVERT_R, "invert_r", "Invert R", 0.0),
    linear(P_WIDTH, "width", "Width", 0.0, 2.0, 1.0, Unit::None),
    expo(
        P_LOW_BYPASS,
        "low_bypass",
        "Low bypass",
        20.0,
        500.0,
        20.0,
        Unit::Hz,
    ),
    linear(P_BALANCE, "balance", "Balance", -1.0, 1.0, 0.0, Unit::None),
    toggle(P_MONO, "mono", "Mono", 0.0),
    bypass_param(7),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);

static SOCKETS: [SocketDescriptor; 4] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_out(2, "out_l", "Out L"),
    audio_out(3, "out_r", "Out R"),
];

/// Static description of the stereo tool.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.stereo-tool",
    name: "Stereo Tool",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`StereoTool`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(StereoTool::new())),
    }
}

/// The stereo tool device.
#[derive(Debug, Clone)]
pub struct StereoTool {
    mode: u8,
    invert_l: bool,
    invert_r: bool,
    width: f32,
    low_bypass: f32,
    balance: f32,
    mono: bool,
    fresh: bool,
    split: [Crossover; 2],
    pol_l: BlockSmoother,
    pol_r: BlockSmoother,
    width_sm: BlockSmoother,
    bal_l: BlockSmoother,
    bal_r: BlockSmoother,
    mono_sm: BlockSmoother,
}

impl Default for StereoTool {
    fn default() -> Self {
        Self::new()
    }
}

impl StereoTool {
    /// A stereo tool with default params.
    pub fn new() -> Self {
        Self {
            mode: 0,
            invert_l: false,
            invert_r: false,
            width: 1.0,
            low_bypass: 20.0,
            balance: 0.0,
            mono: false,
            fresh: true,
            split: [Crossover::new(48_000.0), Crossover::new(48_000.0)],
            pol_l: BlockSmoother::new(1.0),
            pol_r: BlockSmoother::new(1.0),
            width_sm: BlockSmoother::new(1.0),
            bal_l: BlockSmoother::new(1.0),
            bal_r: BlockSmoother::new(1.0),
            mono_sm: BlockSmoother::new(0.0),
        }
    }

    fn low_split(&self) -> bool {
        self.low_bypass > LOW_BYPASS_OFF_HZ
    }

    fn apply_low_bypass(&mut self) {
        for c in &mut self.split {
            c.set_freq(self.low_bypass);
        }
    }
}

impl Device for StereoTool {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.split = [Crossover::new(sr), Crossover::new(sr)];
        self.apply_low_bypass();
        for s in [
            &mut self.pol_l,
            &mut self.pol_r,
            &mut self.width_sm,
            &mut self.bal_l,
            &mut self.bal_r,
            &mut self.mono_sm,
        ] {
            s.set_time(10.0, sr, BATCH);
        }
        self.reset();
    }

    fn reset(&mut self) {
        for c in &mut self.split {
            c.reset();
        }
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_MODE => self.mode = v.clamp(0.0, 2.0).round() as u8,
            P_INVERT_L => self.invert_l = v >= 0.5,
            P_INVERT_R => self.invert_r = v >= 0.5,
            P_WIDTH => self.width = v.clamp(0.0, 2.0),
            P_LOW_BYPASS => {
                self.low_bypass = v.clamp(20.0, 500.0);
                self.apply_low_bypass();
            }
            P_BALANCE => self.balance = v.clamp(-1.0, 1.0),
            P_MONO => self.mono = v >= 0.5,
            _ => {}
        }
    }

    #[allow(clippy::float_cmp)]
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let (in_l, in_r) = (*ctx.input(IN_L), *ctx.input(IN_R));
        let sign = |on: bool| if on { -1.0 } else { 1.0 };
        let (bl, br) = balance_gains(self.balance);
        let mono = if self.mono { 1.0 } else { 0.0 };
        if self.fresh {
            self.pol_l.set_immediate(sign(self.invert_l));
            self.pol_r.set_immediate(sign(self.invert_r));
            self.width_sm.set_immediate(self.width);
            self.bal_l.set_immediate(bl);
            self.bal_r.set_immediate(br);
            self.mono_sm.set_immediate(mono);
            self.fresh = false;
        }
        let pl = self.pol_l.advance(sign(self.invert_l), BATCH);
        let pr = self.pol_r.advance(sign(self.invert_r), BATCH);
        let w = self.width_sm.advance(self.width, BATCH);
        let gl = self.bal_l.advance(bl, BATCH);
        let gr = self.bal_r.advance(br, BATCH);
        let mo = self.mono_sm.advance(mono, BATCH);
        let split = self.low_split();

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        for n in 0..BATCH {
            let mut l = finite(in_l[n]) * pl.at(n);
            let mut r = finite(in_r[n]) * pr.at(n);
            let wn = w.at(n);
            if wn != 1.0 {
                if split {
                    let (ll, lh) = self.split[0].process(l);
                    let (rl, rh) = self.split[1].process(r);
                    let m = 0.5 * (lh + rh);
                    let s = 0.5 * (lh - rh) * wn;
                    l = ll + (m + s);
                    r = rl + (m - s);
                } else {
                    let m = 0.5 * (l + r);
                    let s = 0.5 * (l - r) * wn;
                    l = m + s;
                    r = m - s;
                }
            } else if split {
                // Keep the crossover state running so a width change does not start cold.
                let _ = self.split[0].process(l);
                let _ = self.split[1].process(r);
            }
            let (g_l, g_r) = (gl.at(n), gr.at(n));
            if g_l != 1.0 || g_r != 1.0 {
                l *= g_l;
                r *= g_r;
            }
            match self.mode {
                1 => {
                    let (m, s) = (0.5 * (l + r), 0.5 * (l - r));
                    l = m;
                    r = s;
                }
                2 => {
                    let (m, s) = (l, r);
                    l = m + s;
                    r = m - s;
                }
                _ => {}
            }
            // Mono is the last stage, so the output is mono whatever the mode.
            let mn = mo.at(n);
            if mn >= 1.0 {
                // Exactly equal channels once the ramp has settled.
                let m = 0.5 * (l + r);
                l = m;
                r = m;
            } else if mn != 0.0 {
                let m = 0.5 * (l + r);
                l += (m - l) * mn;
                r += (m - r) * mn;
            }
            out_l[n] = l;
            out_r[n] = r;
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
    }
}

#[cfg(test)]
mod tests;
