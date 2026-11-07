//! `atomik.convolver`: a convolution reverb that plays an impulse response from the project.
//!
//! The impulse response (IR) is an ordinary audio file in the project's sample pool: drop it on
//! the device like a sample (the device takes one, a new drop replaces the old one). It reaches
//! the device through [`Device::swap_samples`] as the first zone of a sample map; the zone's
//! `start..end` frames are the part used. A mono IR convolves both channels; a stereo IR
//! convolves the left input with its left channel and the right input with its right channel.
//!
//! # Signal flow
//!
//! ```text
//! in L/R -> pre-delay -> partitioned convolution (zero latency) -> width -> low cut -> high cut
//!        -> output gain -> mix with the dry input
//! ```
//!
//! * **Convolution.** One [`atomik_dsp::Convolver`] per channel: a 256-sample head convolved
//!   directly and the rest in uniform 256-sample partitions in the frequency domain. The device
//!   adds **no latency** and `latency_frames()` is 0. IRs longer than 5 s are cut with a short
//!   fade.
//! * **Level.** The IR is normalised to unit energy (the louder channel), so white noise keeps
//!   its power through a mix of 1 and `output` 0 dB; swapping IRs does not change the level.
//! * **Loading.** Everything is allocated in `prepare`. [`Device::swap_samples`] only stores the
//!   map and returns the old one for the caller to drop off the audio thread; the partitions
//!   are transformed a few per batch (a few hundred milliseconds for the longest IR). The first
//!   IR of a freshly prepared device is exact from the first sample; a later IR is built in the
//!   background while the old one keeps sounding and is then crossfaded in over 256 samples, so
//!   a swap never clicks. Without an IR the wet signal is silent.
//! * **Mix.** `out = dry * (1 - mix) + wet * mix * output`; at `mix == 0` the input is copied
//!   bit-exactly.
//! * **Determinism.** The convolver sums in a fixed order and the build advances once per batch,
//!   so bounces are bit-identical at any host block size.

use crate::bypass::{Bypassable, bypass_param};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SampleMapRef, SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{BlockSmoother, Convolver, DelayLine, Ramp, exp2_det, flush_denormal};

#[cfg(test)]
mod tests;

/// Longest impulse response in seconds (longer ones are cut with a fade).
pub const MAX_IR_SECONDS: f32 = 5.0;
/// Longest pre-delay in ms.
pub const MAX_PREDELAY_MS: f32 = 250.0;
/// Partitions transformed per channel and batch while an IR loads.
const BUILD_BUDGET: usize = 8;
const WET_LIMIT: f32 = 32.0;
/// From this high cut upwards the lowpass is out of the signal path.
const HIGHCUT_OFF: f32 = 19_999.0;

const P_PREDELAY: u32 = 0;
const P_LOWCUT: u32 = 1;
const P_HIGHCUT: u32 = 2;
const P_WIDTH: u32 = 3;
const P_OUTPUT: u32 = 4;
const P_MIX: u32 = 5;

static PARAMS: [ParamDescriptor; 7] = [
    ParamDescriptor {
        id: ParamId(P_PREDELAY),
        key: "predelay",
        name: "Pre-delay",
        min: 0.0,
        max: MAX_PREDELAY_MS,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Ms,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_LOWCUT),
        key: "lowcut",
        name: "Low cut",
        min: 20.0,
        max: 1_000.0,
        default: 20.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_HIGHCUT),
        key: "highcut",
        name: "High cut",
        min: 1_000.0,
        max: 20_000.0,
        default: 20_000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_WIDTH),
        key: "width",
        name: "Width",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_OUTPUT),
        key: "output",
        name: "Output",
        min: -24.0,
        max: 12.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Db,
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
    bypass_param(6),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);

static SOCKETS: [SocketDescriptor; 4] = [
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
];

/// Static description of the convolution reverb.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.convolver",
    name: "Convolution Reverb",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`ConvolutionReverb`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(ConvolutionReverb::new())),
    }
}

const C_PRE: usize = 0;
const C_LOWCUT: usize = 1;
const C_HIGHCUT: usize = 2;
const C_WIDTH: usize = 3;
const C_OUT: usize = 4;
const C_MIX: usize = 5;
const NC: usize = 6;

/// The impulse response of a map: channel `ch` of the first zone's frames (a mono IR serves
/// both channels), empty without a map or zone.
fn ir_of(map: &Option<SampleMapRef>, ch: usize) -> &[f32] {
    let Some(map) = map else {
        return &[];
    };
    let Some(zone) = map.zones().first() else {
        return &[];
    };
    let buf = map.buffer_of(zone);
    let data = &buf.data;
    let chan = data.get(ch).unwrap_or(&data[0]);
    let end = (zone.end as usize).min(chan.len());
    let start = (zone.start as usize).min(end);
    &chan[start..end]
}

/// The factor that gives an impulse response unit energy (the louder channel), 1 for silence.
fn normal_gain(map: &Option<SampleMapRef>, max_len: usize) -> f32 {
    let energy = |ch: usize| -> f64 {
        let ir = ir_of(map, ch);
        ir[..ir.len().min(max_len)]
            .iter()
            .map(|v| f64::from(*v) * f64::from(*v))
            .sum()
    };
    let e = energy(0).max(energy(1));
    if e > 1e-12 {
        (1.0 / e.sqrt()) as f32
    } else {
        1.0
    }
}

/// The convolution reverb device.
#[derive(Debug, Clone)]
pub struct ConvolutionReverb {
    sample_rate: f32,
    predelay_ms: f32,
    lowcut: f32,
    highcut: f32,
    width: f32,
    output_db: f32,
    mix: f32,
    fresh: bool,
    ctl: [BlockSmoother; NC],
    conv: [Convolver; 2],
    pre: [DelayLine; 2],
    hp: [f32; 2],
    lp: [f32; 2],
    map: Option<SampleMapRef>,
    gain: f32,
    /// A new map waits for its build to start.
    want_build: bool,
}

impl Default for ConvolutionReverb {
    fn default() -> Self {
        Self::new()
    }
}

impl ConvolutionReverb {
    /// A convolution reverb with default params; its buffers are sized by `prepare`.
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            predelay_ms: 0.0,
            lowcut: 20.0,
            highcut: 20_000.0,
            width: 1.0,
            output_db: 0.0,
            mix: 0.3,
            fresh: true,
            ctl: [BlockSmoother::new(0.0); NC],
            conv: [Convolver::new(1), Convolver::new(1)],
            pre: [DelayLine::new(1), DelayLine::new(1)],
            hp: [0.0; 2],
            lp: [0.0; 2],
            map: None,
            gain: 1.0,
            want_build: false,
        }
    }

    fn targets(&self) -> [f32; NC] {
        let sr = self.sample_rate;
        let pole =
            |hz: f32| 1.0 - exp2_det(-std::f32::consts::TAU * hz / sr * std::f32::consts::LOG2_E);
        let mut t = [0.0f32; NC];
        t[C_PRE] = self.predelay_ms * 0.001 * sr;
        t[C_LOWCUT] = pole(self.lowcut);
        t[C_HIGHCUT] = pole(self.highcut);
        t[C_WIDTH] = self.width;
        t[C_OUT] = exp2_det(self.output_db * (std::f32::consts::LOG2_10 / 20.0));
        t[C_MIX] = self.mix;
        t
    }

    /// Starts and advances the build of the installed map.
    fn load_step(&mut self) {
        if self.want_build && !self.conv[0].fading() {
            self.want_build = false;
            let max = self.conv[0].max_ir();
            self.gain = normal_gain(&self.map, max);
            for ch in 0..2 {
                self.conv[ch].begin_build(ir_of(&self.map, ch), self.gain);
            }
        }
        if self.conv[0].building() {
            for ch in 0..2 {
                self.conv[ch].build_step(ir_of(&self.map, ch), BUILD_BUDGET);
            }
        }
    }

    /// The impulse response length in frames of the installed map (0 without one).
    pub fn ir_frames(&self) -> usize {
        ir_of(&self.map, 0).len()
    }
}

impl Device for ConvolutionReverb {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.sample_rate = sr;
        let max_ir = (MAX_IR_SECONDS * sr).ceil() as usize;
        self.conv = [Convolver::new(max_ir), Convolver::new(max_ir)];
        let pre = (MAX_PREDELAY_MS * 0.001 * sr).ceil() as usize + 4;
        self.pre = [DelayLine::new(pre), DelayLine::new(pre)];
        for (i, c) in self.ctl.iter_mut().enumerate() {
            let tau = if i == C_MIX { 20.0 } else { 30.0 };
            c.set_time(tau, sr, BATCH);
        }
        // The installed map (if any) is rebuilt into the new convolvers.
        self.want_build = self.map.is_some();
        self.reset();
    }

    fn reset(&mut self) {
        for c in &mut self.conv {
            c.reset();
        }
        for p in &mut self.pre {
            p.clear();
        }
        self.hp = [0.0; 2];
        self.lp = [0.0; 2];
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        match id.0 {
            P_PREDELAY => self.predelay_ms = v.clamp(0.0, MAX_PREDELAY_MS),
            P_LOWCUT => self.lowcut = v.clamp(20.0, 1_000.0),
            P_HIGHCUT => self.highcut = v.clamp(1_000.0, 20_000.0),
            P_WIDTH => self.width = v.clamp(0.0, 1.0),
            P_OUTPUT => self.output_db = v.clamp(-24.0, 12.0),
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            _ => {}
        }
    }

    fn swap_samples(&mut self, map: Option<SampleMapRef>) -> Option<SampleMapRef> {
        self.want_build = true;
        std::mem::replace(&mut self.map, map)
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        self.load_step();

        let target = self.targets();
        if self.fresh {
            for (c, t) in self.ctl.iter_mut().zip(&target) {
                c.set_immediate(*t);
            }
            self.fresh = false;
        }
        let mut ramps = [Ramp::constant(0.0); NC];
        for i in 0..NC {
            ramps[i] = self.ctl[i].advance(target[i], BATCH);
        }

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        for n in 0..BATCH {
            let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
            let (dry_l, dry_r) = (in_l[n], in_r[n]);
            let (xl, xr) = (finite(dry_l), finite(dry_r));
            self.pre[0].write(xl);
            self.pre[1].write(xr);
            let pd = ramps[C_PRE].at(n) + 1.0;
            let (pl, pr) = (self.pre[0].read(pd), self.pre[1].read(pd));
            let (wl, wr) = (self.conv[0].tick(pl), self.conv[1].tick(pr));

            let m = 0.5 * (wl + wr);
            let s = 0.5 * (wl - wr) * ramps[C_WIDTH].at(n);
            let (mut wet_l, mut wet_r) = (
                (m + s).clamp(-WET_LIMIT, WET_LIMIT),
                (m - s).clamp(-WET_LIMIT, WET_LIMIT),
            );
            let a_lc = ramps[C_LOWCUT].at(n);
            let a_hc = ramps[C_HIGHCUT].at(n);
            self.hp[0] = flush_denormal(self.hp[0] + a_lc * (wet_l - self.hp[0]));
            self.hp[1] = flush_denormal(self.hp[1] + a_lc * (wet_r - self.hp[1]));
            wet_l -= self.hp[0];
            wet_r -= self.hp[1];
            if self.highcut < HIGHCUT_OFF {
                self.lp[0] = flush_denormal(self.lp[0] + a_hc * (wet_l - self.lp[0]));
                self.lp[1] = flush_denormal(self.lp[1] + a_hc * (wet_r - self.lp[1]));
            } else {
                self.lp = [wet_l, wet_r];
            }
            let og = ramps[C_OUT].at(n);
            let (wet_l, wet_r) = (self.lp[0] * og, self.lp[1] * og);
            let mix = ramps[C_MIX].at(n);
            if mix == 0.0 {
                out_l[n] = dry_l;
                out_r[n] = dry_r;
            } else {
                out_l[n] = dry_l * (1.0 - mix) + wet_l * mix;
                out_r[n] = dry_r * (1.0 - mix) + wet_r * mix;
            }
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
    }
}
