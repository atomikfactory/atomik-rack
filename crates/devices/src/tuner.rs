//! `atomik.tuner`: a chromatic tuner that passes the audio through.
//!
//! ```text
//! in_l, in_r --(x gain)--> out_l, out_r      gain 1, or 0 with `mute` on (5 ms ramp)
//!      '--- 64 frames per batch --> ScopeTap  (the app runs the pitch analysis from it)
//! ```
//!
//! * **Audio path.** The outputs are the inputs, bit for bit, with no latency, unless `mute` is
//!   on: the output then ramps to silence over 5 ms (and back when it is off), so a guitar can be
//!   tuned in silence. The tap always hears the input, muted or not.
//! * **Analysis.** The device only keeps the newest frames in its [`ScopeTap`]; the app reads them
//!   about once per frame and runs the pitch analysis ([`atomik_dsp::tuner`], a two-stage period
//!   tracker with smoothing and a hold) on its own thread, with `ref` as the pitch of A4 shifted
//!   by the project's master tune. The panel shows the note, the deviation in cents on a needle
//!   scale and the frequency. The audio thread only writes two ring buffers.
//! * No latency; non-finite input samples count as 0 on the outputs; allocation-free after
//!   construction.

use crate::bypass::{Bypassable, bypass_param};
use crate::tool_support::{audio_in, audio_out, finite, toggle};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext, ScopeTap,
    SocketDescriptor, SocketId,
};
use std::sync::Arc;

const P_MUTE: u32 = 0;
const P_REF: u32 = 1;

/// Length of the mute ramp, ms.
const MUTE_MS: f32 = 5.0;
/// Lowest and highest reference pitch for A4, Hz (the tuner's own limits).
const REF_MIN: f32 = atomik_dsp::tuner::REF_MIN_HZ;
const REF_MAX: f32 = atomik_dsp::tuner::REF_MAX_HZ;

static PARAMS: [ParamDescriptor; 3] = [
    toggle(P_MUTE, "mute", "Mute", 0.0),
    ParamDescriptor {
        id: ParamId(P_REF),
        key: "ref",
        name: "Reference A4",
        min: REF_MIN,
        max: REF_MAX,
        default: atomik_dsp::tuner::REF_DEFAULT_HZ,
        taper: Taper::Linear,
        unit: Unit::Hz,
        discrete: true,
    },
    bypass_param(2),
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

/// Static description of the tuner.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.tuner",
    name: "Tuner",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Tuner`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(Tuner::new())),
    }
}

/// The tuner device.
pub struct Tuner {
    mute: bool,
    /// 1 when the output sounds, 0 while muted.
    gain: f32,
    /// Gain change per sample.
    step: f32,
    tap: Arc<ScopeTap>,
}

impl std::fmt::Debug for Tuner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tuner")
            .field("mute", &self.mute)
            .field("gain", &self.gain)
            .finish_non_exhaustive()
    }
}

impl Default for Tuner {
    fn default() -> Self {
        Self::new()
    }
}

impl Tuner {
    /// A tuner with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        Self {
            mute: false,
            gain: 1.0,
            step: 1.0 / (MUTE_MS * 0.001 * 48_000.0),
            tap: Arc::new(ScopeTap::new()),
        }
    }
}

impl Device for Tuner {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.step = 1.0 / (MUTE_MS * 0.001 * info.sample_rate.hz() as f32);
        self.reset();
    }

    fn reset(&mut self) {
        self.gain = if self.mute { 0.0 } else { 1.0 };
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        // The reference pitch is the app's: it reads it from the document.
        if id.0 == P_MUTE {
            self.mute = finite(plain) >= 0.5;
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let target = if self.mute { 0.0 } else { 1.0 };
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut tap_l = [0.0f32; BATCH];
        let mut tap_r = [0.0f32; BATCH];
        for n in 0..BATCH {
            if self.gain < target {
                self.gain = (self.gain + self.step).min(target);
            } else if self.gain > target {
                self.gain = (self.gain - self.step).max(target);
            }
            let (l, r) = (finite(in_l[n]), finite(in_r[n]));
            tap_l[n] = l;
            tap_r[n] = r;
            if self.gain >= 1.0 {
                out_l[n] = l;
                out_r[n] = r;
            } else {
                out_l[n] = l * self.gain;
                out_r[n] = r * self.gain;
            }
        }
        self.tap.push(&tap_l, &tap_r);
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
    }

    fn scope_tap(&self) -> Option<Arc<ScopeTap>> {
        Some(Arc::clone(&self.tap))
    }
}

#[cfg(test)]
mod tests;
