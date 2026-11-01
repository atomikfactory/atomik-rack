//! `atomik.gain-tool`: gain, balance, width and mute for a stereo signal, with an optional second
//! stereo input to add or cross-fade in.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! gain mode   s = x
//! mix mode    s = x + aux * blend                   (`aux_l`/`aux_r` added at the `blend` level)
//! xfade mode  s = x * (1 - blend) + aux * blend     (linear cross-fade from `x` to `aux`)
//! s -> width (mid/side, skipped at exactly 1) -> * gain * balance gains -> out
//! ```
//!
//! * **Gain** is `gain` (dB) plus `gain_cv_trim * gain_cv_in * 24 dB` (the console's level
//!   convention), converted with the deterministic `exp2_det`. **Mute** ramps the gain to 0.
//! * **Balance** (`pan`, -1..+1, plus `pan_cv_trim * pan_cv_in`) is unity on both sides at the
//!   centre; moving it away from the centre lowers the far side linearly to silence at the end.
//! * **Width** is the mid/side width of `atomik_dsp::stereo_width`: 0 is mono, 1 leaves the
//!   signal alone (the path is skipped, so the defaults are the input bit for bit), 2 doubles the
//!   side signal.
//! * **Smoothing.** Gain, balance gains, width and blend are smoothed per batch (10 ms,
//!   `BlockSmoother`) and ramped linearly across the batch; CV is its mean over the batch.
//! * No latency. Non-finite input samples count as 0. Allocation-free after construction.

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::{batch_mean, clean};
use crate::tool_support::{
    audio_in, audio_out, balance_gains, choice, cv_in, finite, linear, toggle, trim,
};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{BlockSmoother, exp2_det};

const P_MODE: u32 = 0;
const P_GAIN: u32 = 1;
const P_PAN: u32 = 2;
const P_WIDTH: u32 = 3;
const P_BLEND: u32 = 4;
const P_MUTE: u32 = 5;
const P_GAIN_TRIM: u32 = 6;
const P_PAN_TRIM: u32 = 7;

/// The CV range of `gain_cv_in` at full trim, in dB.
const CV_GAIN_DB: f32 = 24.0;
/// `log2(10) / 20`: `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;

static MODES: [&str; 3] = ["gain", "mix", "xfade"];

static PARAMS: [ParamDescriptor; 9] = [
    choice(P_MODE, "mode", "Mode", &MODES, 0.0),
    linear(P_GAIN, "gain", "Gain", -60.0, 24.0, 0.0, Unit::Db),
    linear(P_PAN, "pan", "Balance", -1.0, 1.0, 0.0, Unit::None),
    linear(P_WIDTH, "width", "Width", 0.0, 2.0, 1.0, Unit::None),
    linear(P_BLEND, "blend", "Blend", 0.0, 1.0, 0.5, Unit::Percent),
    toggle(P_MUTE, "mute", "Mute", 0.0),
    trim(P_GAIN_TRIM, "gain_cv_trim", "Gain CV trim"),
    trim(P_PAN_TRIM, "pan_cv_trim", "Balance CV trim"),
    bypass_param(8),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const AUX_L: SocketId = SocketId(2);
const AUX_R: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);
const GAIN_CV: SocketId = SocketId(6);
const PAN_CV: SocketId = SocketId(7);

static SOCKETS: [SocketDescriptor; 8] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_in(2, "aux_l", "Aux L"),
    audio_in(3, "aux_r", "Aux R"),
    audio_out(4, "out_l", "Out L"),
    audio_out(5, "out_r", "Out R"),
    cv_in(6, "gain_cv_in", "Gain CV"),
    cv_in(7, "pan_cv_in", "Balance CV"),
];

/// Static description of the gain tool.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.gain-tool",
    name: "Gain Tool",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`GainTool`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(GainTool::new())),
    }
}

/// The gain tool device.
#[derive(Debug, Clone)]
pub struct GainTool {
    mode: u8,
    gain_db: f32,
    pan: f32,
    width: f32,
    blend: f32,
    mute: bool,
    gain_trim: f32,
    pan_trim: f32,
    fresh: bool,
    gain: BlockSmoother,
    left: BlockSmoother,
    right: BlockSmoother,
    width_sm: BlockSmoother,
    blend_sm: BlockSmoother,
}

impl Default for GainTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GainTool {
    /// A gain tool with default params.
    pub fn new() -> Self {
        Self {
            mode: 0,
            gain_db: 0.0,
            pan: 0.0,
            width: 1.0,
            blend: 0.5,
            mute: false,
            gain_trim: 1.0,
            pan_trim: 1.0,
            fresh: true,
            gain: BlockSmoother::new(1.0),
            left: BlockSmoother::new(1.0),
            right: BlockSmoother::new(1.0),
            width_sm: BlockSmoother::new(1.0),
            blend_sm: BlockSmoother::new(0.5),
        }
    }
}

impl Device for GainTool {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        for s in [
            &mut self.gain,
            &mut self.left,
            &mut self.right,
            &mut self.width_sm,
            &mut self.blend_sm,
        ] {
            s.set_time(10.0, sr, BATCH);
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_MODE => self.mode = v.clamp(0.0, 2.0).round() as u8,
            P_GAIN => self.gain_db = v.clamp(-60.0, 24.0),
            P_PAN => self.pan = v.clamp(-1.0, 1.0),
            P_WIDTH => self.width = v.clamp(0.0, 2.0),
            P_BLEND => self.blend = v.clamp(0.0, 1.0),
            P_MUTE => self.mute = v >= 0.5,
            P_GAIN_TRIM => self.gain_trim = v.clamp(-1.0, 1.0),
            P_PAN_TRIM => self.pan_trim = v.clamp(-1.0, 1.0),
            _ => {}
        }
    }

    #[allow(clippy::float_cmp)]
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let (in_l, in_r) = (*ctx.input(IN_L), *ctx.input(IN_R));
        let (aux_l, aux_r) = (*ctx.input(AUX_L), *ctx.input(AUX_R));
        let cv_gain = batch_mean(ctx.input(GAIN_CV));
        let cv_pan = batch_mean(ctx.input(PAN_CV));

        let gain_target = if self.mute {
            0.0
        } else {
            let db = self.gain_db + self.gain_trim * clean(cv_gain) * CV_GAIN_DB;
            exp2_det(db * LOG2_10_20)
        };
        let (bl, br) = balance_gains(self.pan + self.pan_trim * clean(cv_pan));
        if self.fresh {
            self.gain.set_immediate(gain_target);
            self.left.set_immediate(bl);
            self.right.set_immediate(br);
            self.width_sm.set_immediate(self.width);
            self.blend_sm.set_immediate(self.blend);
            self.fresh = false;
        }
        let g = self.gain.advance(gain_target, BATCH);
        let gl = self.left.advance(bl, BATCH);
        let gr = self.right.advance(br, BATCH);
        let w = self.width_sm.advance(self.width, BATCH);
        let b = self.blend_sm.advance(self.blend, BATCH);

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        for n in 0..BATCH {
            let (xl, xr) = (finite(in_l[n]), finite(in_r[n]));
            let (al, ar) = (finite(aux_l[n]), finite(aux_r[n]));
            let bn = b.at(n);
            let (mut l, mut r) = match self.mode {
                1 => (xl + al * bn, xr + ar * bn),
                2 => (xl * (1.0 - bn) + al * bn, xr * (1.0 - bn) + ar * bn),
                _ => (xl, xr),
            };
            let wn = w.at(n);
            if wn != 1.0 {
                let m = 0.5 * (l + r);
                let s = 0.5 * (l - r) * wn;
                l = m + s;
                r = m - s;
            }
            let gn = g.at(n);
            out_l[n] = l * (gn * gl.at(n));
            out_r[n] = r * (gn * gr.at(n));
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
    }
}

#[cfg(test)]
mod tests;
