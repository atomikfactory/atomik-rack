//! `atomik.feedback`: the supported way to send audio back on itself.
//!
//! A cable that would close a loop is refused everywhere in the rack, except through this device:
//! a loop that passes through a Feedback Route is legal, because the device is a delay stage that
//! breaks the cycle. Cable the output of the chain that should feed back into `in_l`/`in_r`, and
//! cable `out_l`/`out_r` to wherever the fed-back signal should enter (the input of the first
//! effect of the loop, or a mixer input). Every other loop is still refused.
//!
//! # How it breaks the cycle
//!
//! The device has two processing stages. Its outputs belong to stage 0 and its inputs to stage 1
//! (`DeviceDescriptor::stages`), so within one batch the engine runs the outputs first, then
//! everything downstream of them, and the inputs last. The engine's order of the plan stays
//! acyclic, and the model and the engine judge a cable that would close a loop per (device, stage)
//! node, so a loop is only accepted when it runs through a stage boundary. Stage 0 reads what
//! stage 1 wrote at least one batch earlier.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! stage 1   line[t] = clamp(in[t], -LIMIT, LIMIT)             (non-finite input counts as 0)
//! stage 0   out[t]  = line[t - D] * gain                      D = max(BATCH, time * sample rate)
//! ```
//!
//! * **The delay is at least one engine batch** ([`BATCH`] frames, 64): `time` of 0 means "the
//!   shortest delay", and a larger `time` (milliseconds, up to one second) is rounded to a whole
//!   number of frames. The delay counts engine batches, never host buffers, so the output is
//!   bit-identical at every host block size and with any number of workers, and the signal that
//!   comes back is sample-accurate: the n-th frame out is the (n - D)-th frame in.
//! * **No latency is reported.** The delay is part of the sound, not a processing delay, so the
//!   delay compensation does not try to align the loop (the signal that comes back is `D` frames
//!   late by design). Other paths that join the loop are not delayed to match it.
//! * **`gain`** (dB, -60 to 0, default 0) scales what comes back. At 0 dB a loop that adds no gain
//!   rings for ever; any gain added by the devices in the loop is the loop's own business, and the
//!   limit below keeps a runaway loop finite.
//! * **Smoothing.** A change of `time` or `gain` glides over about 50 ms (stepped once per batch and
//!   ramped across it); the delay moves like a tape delay (the pitch of what rings in the loop
//!   bends while it moves) and never clicks. A line reads linearly interpolated between frames
//!   only while the time moves; at rest the read is the exact frame.
//! * **Bounded.** What enters the line is clamped to ±[`LIMIT`] (about +24 dBFS) and denormals
//!   are flushed, so a loop with too much gain saturates instead of overflowing.
//! * Reset clears the line. Allocation-free after `prepare` (which sizes the line for one second
//!   at the sample rate).
//!
//! `DeviceKind::Utility`, no bypass: a bypassed feedback device could not keep a loop acyclic.

use crate::tool_support::{audio_in, audio_out, finite, linear};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext, SocketId,
};
use atomik_dsp::{BlockSmoother, DelayLine, exp2_det};

const P_TIME: u32 = 0;
const P_GAIN: u32 = 1;

/// Longest `time`, in milliseconds.
pub const MAX_TIME_MS: f32 = 1000.0;
/// What enters the line is clamped to plus or minus this (about +24 dBFS).
pub const LIMIT: f32 = 16.0;
/// `log2(10) / 20`: `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// The time constant of the glide of `time` and `gain`, in milliseconds.
const GLIDE_MS: f32 = 50.0;

static PARAMS: [ParamDescriptor; 2] = [
    linear(P_TIME, "time", "Time", 0.0, MAX_TIME_MS, 0.0, Unit::Ms),
    linear(P_GAIN, "gain", "Gain", -60.0, 0.0, 0.0, Unit::Db),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);

static SOCKETS: [atomik_device_sdk::SocketDescriptor; 4] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_out(2, "out_l", "Out L"),
    audio_out(3, "out_r", "Out R"),
];

/// The inputs are read last (stage 1), the outputs written first (stage 0).
static STAGES: [u8; 4] = [1, 1, 0, 0];

/// Static description of the feedback route.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &STAGES,
    type_id: "atomik.feedback",
    name: "Feedback Route",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`FeedbackDelay`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(FeedbackDelay::new()),
    }
}

/// The feedback route device.
#[derive(Debug, Clone)]
pub struct FeedbackDelay {
    time_ms: f32,
    gain_db: f32,
    sample_rate: f32,
    left: DelayLine,
    right: DelayLine,
    /// The delay in frames, smoothed.
    delay: BlockSmoother,
    /// The linear gain, smoothed.
    level: BlockSmoother,
    fresh: bool,
}

impl Default for FeedbackDelay {
    fn default() -> Self {
        Self::new()
    }
}

impl FeedbackDelay {
    /// A feedback route with default params (the shortest delay, 0 dB), prepared at 48 kHz.
    pub fn new() -> Self {
        let mut d = Self {
            time_ms: 0.0,
            gain_db: 0.0,
            sample_rate: 48_000.0,
            left: DelayLine::new(BATCH),
            right: DelayLine::new(BATCH),
            delay: BlockSmoother::new(BATCH as f32),
            level: BlockSmoother::new(1.0),
            fresh: true,
        };
        d.size_lines();
        d
    }

    /// The longest delay in frames at the current rate: one second, the batch the reads of one
    /// batch reach back over, and a frame of interpolation.
    fn max_frames(&self) -> usize {
        (self.sample_rate * MAX_TIME_MS * 0.001).ceil() as usize + BATCH + 2
    }

    /// Allocates the lines for the current rate. Non-RT.
    fn size_lines(&mut self) {
        let max = self.max_frames();
        self.left = DelayLine::new(max);
        self.right = DelayLine::new(max);
    }

    /// The delay the params ask for, in frames: never under one batch, never over the line.
    fn target_frames(&self) -> f32 {
        let frames = (self.time_ms * 0.001 * self.sample_rate).round();
        frames.clamp(BATCH as f32, (self.max_frames() - 2) as f32)
    }

    /// Stage 0: the outputs, from what earlier batches wrote.
    fn emit(&mut self, ctx: &mut ProcessContext<'_>) {
        let target = self.target_frames();
        let gain = exp2_det(self.gain_db * LOG2_10_20);
        if self.fresh {
            self.delay.set_immediate(target);
            self.level.set_immediate(gain);
            self.fresh = false;
        }
        let d = self.delay.advance(target, BATCH);
        let g = self.level.advance(gain, BATCH);
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        for n in 0..BATCH {
            // The line holds everything up to the frame before this batch; the frame `n` of the
            // batch reads `delay - n` back, which is at least 1 because the delay is at least
            // one batch (a ramp between two such values stays above it).
            let back = d.at(n) - n as f32;
            let gn = g.at(n);
            out_l[n] = self.left.read(back) * gn;
            out_r[n] = self.right.read(back) * gn;
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
    }

    /// Stage 1: the inputs, into the line.
    fn capture(&mut self, ctx: &ProcessContext<'_>) {
        let (in_l, in_r) = (*ctx.input(IN_L), *ctx.input(IN_R));
        for n in 0..BATCH {
            self.left.write(finite(in_l[n]).clamp(-LIMIT, LIMIT));
            self.right.write(finite(in_r[n]).clamp(-LIMIT, LIMIT));
        }
    }
}

impl Device for FeedbackDelay {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.size_lines();
        self.delay.set_time(GLIDE_MS, self.sample_rate, BATCH);
        self.level.set_time(GLIDE_MS, self.sample_rate, BATCH);
        self.reset();
    }

    fn reset(&mut self) {
        self.left.clear();
        self.right.clear();
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_TIME => self.time_ms = v.clamp(0.0, MAX_TIME_MS),
            P_GAIN => self.gain_db = v.clamp(-60.0, 0.0),
            _ => {}
        }
    }

    /// Both stages in order (the engine calls [`Device::process_stage`]; this is for hosts that
    /// run a device in one call).
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        self.emit(ctx);
        self.capture(ctx);
    }

    fn process_stage(&mut self, stage: u8, ctx: &mut ProcessContext<'_>) {
        match stage {
            0 => self.emit(ctx),
            _ => self.capture(ctx),
        }
    }
}

#[cfg(test)]
mod tests;
