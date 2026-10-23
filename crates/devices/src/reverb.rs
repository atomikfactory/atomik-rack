//! `atomik.reverb`: stereo algorithmic reverb (room / hall / plate).
//!
//! # Signal flow (per sample)
//!
//! ```text
//! in L/R -> pre-delay -> low cut -> +--> early taps (6 per side, tapped delay) ----------+
//!                                   |                                                   +-> early/late balance
//!                                   +--> 2 input allpasses per side -> inject -> 8-line FDN -> late taps -+
//!                                                                                                  |
//!                       (early*eg + late*lg) -> width (M/S) -> output gain -> mix with the dry input
//! ```
//!
//! * **FDN.** Eight modulated Hermite delay lines ([`atomik_dsp::Fdn8`]), each followed by a
//!   one-pole damping lowpass, a feedback gain and an in-loop Schroeder allpass, mixed by an
//!   orthonormal 8x8 Hadamard matrix. The input is injected with a sign pattern into the even
//!   lines from L and the odd lines from R; the late output taps use two orthogonal Hadamard
//!   rows (L: `+-+-+-+-`, R: `++--++--`), so the left and right tails are uncorrelated.
//! * **Decay (RT60).** Line `i` has loop time `T_i = d_i + a_i` samples (its delay plus the mean
//!   group delay of its allpass). Its feedback gain is `g_i = 10^(-3 T_i / (sr * decay))`
//!   (`= 2^(-9.9658 T_i / (sr * decay))`), so every line loses 60 dB in `decay` seconds and with
//!   the orthogonal mixing the whole tail does too. The damping lowpass has DC gain 1: `decay` is
//!   the low/mid-frequency RT60 and `damping` shortens the highs. The gains are recomputed once
//!   per batch from the smoothed delays and decay and ramped linearly across the batch.
//! * **Algorithms.** Each has its own FDN delay set, early-tap pattern, allpass coefficient,
//!   damping cutoff range and modulation depth: *room* (17..44 ms lines, dense early taps,
//!   bright, low modulation), *hall* (33..94 ms lines, late and widely spaced early taps, dark,
//!   full modulation), *plate* (11..44 ms lines, very dense, weak early taps, brightest, most
//!   modulation). Delay times scale with `size`: `scale = 0.25 + 1.5 * size`.
//! * **Damping.** `damping` 0..1 maps to a lowpass cutoff `fc = fc_top * 2^(-6 * damping)`
//!   (`fc_top` is 14 / 10 / 16 kHz for room / hall / plate), pole `a = exp(-2 pi fc / sr)`, faded
//!   to exactly 0 (filter bypassed) as `damping` goes to 0.
//! * **Modulation.** Every line has its own seeded parabolic LFO (0.2..1.3 Hz, random start
//!   phase from `PrepareInfo::seed`); the delay excursion is `modulation * mod_scale * 0.0004 *
//!   sr` samples (about 0.4 ms at the maximum). It breaks up metallic ringing.
//! * **Size and algorithm changes do not click.** Every line length, early tap time and tap
//!   gain is a control smoothed with a 250 ms / 100 ms one-pole stepped per batch and applied as
//!   a per-batch linear ramp. A change therefore bends the delay lengths slowly (a gentle
//!   tape-style pitch glide of the ringing tail, bounded by the smoothing time) instead of
//!   jumping; algorithm switches glide the same way and fade the tap gains.
//! * **Freeze.** The freeze control fades over 25 ms: feedback gains go to 1, the damping
//!   lowpass is bypassed and the input is muted, so the tail's energy circulates unchanged.
//! * **Mix CV.** `mix_cv_in` (batch mean, clamped to -1..+1) times `mix_cv_trim` (-1..+1, default
//!   +1) is added to `mix`, then clamped to 0..1, before the control smoother. Unconnected, the
//!   device is bit-identical to before.
//! * **Mix and latency.** `out = dry * (1 - mix) + wet * mix * output`. The dry path has no
//!   delay, and at `mix == 0` the input is copied bit-exactly. Latency is 0; the pre-delay only
//!   delays the wet signal.
//! * **Denormals / determinism.** All feedback state flushes values below 1e-20 to exactly 0.
//!   The per-sample path uses only `+ - * /` and `floor` (Hermite delays, parabolic LFOs); `exp`
//!   is replaced by [`atomik_dsp::exp2_det`] at batch rate. Non-finite input samples are
//!   treated as 0 on the wet path, and the wet output is clamped to +-32.
//! * **Meter values** (`has_meter_values`): `[output peak L, output peak R, tail RMS]`, linear,
//!   of the last batch.

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::{add_norm, batch_mean};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{
    Allpass, BlockSmoother, DelayLine, FDN_LINES, Fdn8, ParabolicLfo, Ramp, Rng, exp2_det,
    flush_denormal,
};

#[cfg(test)]
mod tests;

/// Longest pre-delay in ms.
pub const MAX_PREDELAY_MS: f32 = 250.0;

const P_ALGORITHM: u32 = 0;
const P_PREDELAY: u32 = 1;
const P_SIZE: u32 = 2;
const P_DECAY: u32 = 3;
const P_DAMPING: u32 = 4;
const P_LOWCUT: u32 = 5;
const P_EARLY: u32 = 6;
const P_WIDTH: u32 = 7;
const P_MODULATION: u32 = 8;
const P_MIX: u32 = 9;
const P_FREEZE: u32 = 10;
const P_OUTPUT: u32 = 11;
const P_MIX_TRIM: u32 = 12;

static PARAMS: [ParamDescriptor; 14] = [
    ParamDescriptor {
        id: ParamId(P_ALGORITHM),
        key: "algorithm",
        name: "Algorithm",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&["room", "hall", "plate"]),
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_PREDELAY),
        key: "predelay",
        name: "Pre-delay",
        min: 0.0,
        max: 250.0,
        default: 20.0,
        taper: Taper::Linear,
        unit: Unit::Ms,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_SIZE),
        key: "size",
        name: "Size",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_DECAY),
        key: "decay",
        name: "Decay (s)",
        min: 0.2,
        max: 20.0,
        default: 2.0,
        taper: Taper::Exponential,
        unit: Unit::None,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_DAMPING),
        key: "damping",
        name: "Damping",
        min: 0.0,
        max: 1.0,
        default: 0.4,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_LOWCUT),
        key: "lowcut",
        name: "Low cut",
        min: 20.0,
        max: 1_000.0,
        default: 80.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_EARLY),
        key: "early",
        name: "Early/late",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: Taper::Linear,
        unit: Unit::Percent,
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
        id: ParamId(P_MODULATION),
        key: "modulation",
        name: "Modulation",
        min: 0.0,
        max: 1.0,
        default: 0.5,
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
        id: ParamId(P_FREEZE),
        key: "freeze",
        name: "Freeze",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
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
        id: ParamId(P_MIX_TRIM),
        key: "mix_cv_trim",
        name: "Mix CV trim",
        min: -1.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    bypass_param(13),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
const MIX_CV: SocketId = SocketId(4);

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
        id: MIX_CV,
        key: "mix_cv_in",
        name: "Mix CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
];

/// Static description of the reverb.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.reverb",
    name: "Reverb",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Reverb`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(Reverb::new())),
    }
}

// ---------------------------------------------------------------------------------------------
// Algorithm tables (original designs; times in ms at size scale 1).

const ER_TAPS: usize = 6;

struct Algorithm {
    /// FDN line delays.
    lines_ms: [f32; FDN_LINES],
    /// Early taps for the left and right output: (time ms, gain).
    early: [[(f32, f32); ER_TAPS]; 2],
    /// In-loop allpass coefficient.
    ap_g: f32,
    /// Damping cutoff at `damping` = 0.
    fc_top: f32,
    /// Modulation depth scale.
    mod_scale: f32,
    /// Early tap level scale.
    early_scale: f32,
}

static ALGORITHMS: [Algorithm; 3] = [
    // Room.
    Algorithm {
        lines_ms: [17.3, 21.7, 24.9, 29.3, 31.7, 36.1, 39.3, 43.9],
        early: [
            [
                (2.9, 0.70),
                (6.7, -0.55),
                (10.3, 0.45),
                (14.9, -0.38),
                (19.7, 0.30),
                (26.3, -0.22),
            ],
            [
                (3.7, 0.68),
                (8.3, -0.52),
                (12.1, 0.43),
                (17.9, -0.34),
                (23.3, 0.27),
                (29.9, -0.20),
            ],
        ],
        ap_g: 0.45,
        fc_top: 14_000.0,
        mod_scale: 0.5,
        early_scale: 1.0,
    },
    // Hall.
    Algorithm {
        lines_ms: [33.1, 41.9, 49.7, 57.3, 66.1, 73.9, 83.3, 93.7],
        early: [
            [
                (7.1, 0.60),
                (15.7, -0.50),
                (24.3, 0.42),
                (33.9, -0.34),
                (44.1, 0.27),
                (56.3, -0.20),
            ],
            [
                (9.3, 0.58),
                (19.1, -0.48),
                (28.9, 0.40),
                (38.3, -0.32),
                (49.7, 0.25),
                (61.9, -0.18),
            ],
        ],
        ap_g: 0.55,
        fc_top: 10_000.0,
        mod_scale: 1.0,
        early_scale: 0.8,
    },
    // Plate.
    Algorithm {
        lines_ms: [11.3, 14.9, 19.1, 23.3, 28.7, 33.1, 38.9, 44.3],
        early: [
            [
                (1.9, 0.60),
                (3.7, -0.50),
                (5.9, 0.45),
                (8.3, -0.40),
                (11.3, 0.34),
                (14.9, -0.28),
            ],
            [
                (2.3, 0.58),
                (4.3, -0.50),
                (6.7, 0.44),
                (9.7, -0.38),
                (12.9, 0.32),
                (16.7, -0.26),
            ],
        ],
        ap_g: 0.62,
        fc_top: 16_000.0,
        mod_scale: 1.3,
        early_scale: 0.5,
    },
];

/// In-loop allpass delays in samples at 48 kHz (about 3 to 7 ms), scaled with the sample rate.
const LOOP_AP_48K: [f32; FDN_LINES] = [142.0, 167.0, 191.0, 223.0, 251.0, 277.0, 311.0, 337.0];
/// Input diffuser delays (samples at 48 kHz) for the left and right channel.
const IN_AP_48K: [[f32; 2]; 2] = [[113.0, 271.0], [149.0, 331.0]];
const IN_AP_G: f32 = 0.6;
/// Sign pattern for injecting the input into the lines.
const INJECT_SIGN: [f32; FDN_LINES] = [1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0, 1.0];
/// Late output tap signs (two orthogonal Hadamard rows).
const TAP_L: [f32; FDN_LINES] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
const TAP_R: [f32; FDN_LINES] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
/// Gain from the diffused input to each line, and from the lines to the output.
const INJECT_GAIN: f32 = 0.5;
const LATE_GAIN: f32 = 1.0;
/// Early tap level.
const EARLY_GAIN: f32 = 0.5;
/// `3 * log2(10)`: `10^(-3 x) = 2^(-DECAY_K x)`.
const DECAY_K: f32 = 9.965_784;
/// Empirical calibration of the loop time in the RT60 formula (see the module docs).
const DECAY_CAL: f32 = 0.93;
/// Longest delay-line margin: the largest `size` scale and modulation excursion.
const MAX_SCALE: f32 = 1.75;
const MAX_MOD_SECONDS: f32 = 0.0004 * 1.3;
const WET_LIMIT: f32 = 32.0;

// Smoothed control layout.
const C_PRE: usize = 0;
const C_DECAY: usize = 1;
const C_DAMP: usize = 2;
const C_LOWCUT: usize = 3;
const C_EARLY: usize = 4;
const C_LATE: usize = 5;
const C_WIDTH: usize = 6;
const C_MOD: usize = 7;
const C_MIX: usize = 8;
const C_OUT: usize = 9;
const C_FREEZE: usize = 10;
const C_APG: usize = 11;
const C_LINE: usize = 12;
const C_ERT: usize = C_LINE + FDN_LINES;
const C_ERG: usize = C_ERT + 2 * ER_TAPS;
const NC: usize = C_ERG + 2 * ER_TAPS;

/// Smoothing time constants in ms.
const TAU_FAST_MS: f32 = 30.0;
const TAU_MED_MS: f32 = 60.0;
const TAU_GEOMETRY_MS: f32 = 250.0;
const TAU_TAP_GAIN_MS: f32 = 100.0;
const TAU_FREEZE_MS: f32 = 25.0;

/// The reverb device.
#[derive(Debug, Clone)]
pub struct Reverb {
    sample_rate: f32,
    seed: u64,
    // Raw parameters.
    algorithm: usize,
    predelay_ms: f32,
    size: f32,
    decay: f32,
    damping: f32,
    lowcut: f32,
    early: f32,
    width: f32,
    modulation: f32,
    mix: f32,
    mix_trim: f32,
    freeze: bool,
    output_db: f32,
    // State.
    fresh: bool,
    ctl: [BlockSmoother; NC],
    pre_l: DelayLine,
    pre_r: DelayLine,
    er_l: DelayLine,
    er_r: DelayLine,
    hp: [f32; 2],
    in_ap: [[Allpass; 2]; 2],
    fdn: Fdn8,
    lfo: [ParabolicLfo; FDN_LINES],
    /// Loop-time offsets of the in-loop allpasses (samples).
    loop_ap: [f32; FDN_LINES],
    /// Effective feedback gains at the end of the last batch.
    g_prev: [f32; FDN_LINES],
    peak: [f32; 2],
    tail_rms: f32,
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

impl Reverb {
    /// A reverb with default params; its buffers are sized by `prepare`.
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            seed: 0,
            algorithm: 1,
            predelay_ms: 20.0,
            size: 0.5,
            decay: 2.0,
            damping: 0.4,
            lowcut: 80.0,
            early: 0.5,
            width: 1.0,
            modulation: 0.5,
            mix: 0.3,
            mix_trim: 1.0,
            freeze: false,
            output_db: 0.0,
            fresh: true,
            ctl: [BlockSmoother::new(0.0); NC],
            pre_l: DelayLine::new(1),
            pre_r: DelayLine::new(1),
            er_l: DelayLine::new(1),
            er_r: DelayLine::new(1),
            hp: [0.0; 2],
            in_ap: std::array::from_fn(|_| std::array::from_fn(|_| Allpass::new(1))),
            fdn: Fdn8::new(4, [1; FDN_LINES]),
            lfo: [ParabolicLfo::new(0.0, 0.0, 48_000.0); FDN_LINES],
            loop_ap: [0.0; FDN_LINES],
            g_prev: [0.0; FDN_LINES],
            peak: [0.0; 2],
            tail_rms: 0.0,
        }
    }

    /// Size scale of all delay times: 0.25 at `size` 0, 1.75 at `size` 1.
    #[inline]
    fn scale(&self) -> f32 {
        0.25 + 1.5 * self.size
    }

    /// Damping pole for the current `damping` and algorithm.
    fn damp_pole(&self) -> f32 {
        let alg = &ALGORITHMS[self.algorithm];
        let fc = alg.fc_top * exp2_det(-6.0 * self.damping);
        let a = exp2_det(-std::f32::consts::TAU * fc / self.sample_rate * std::f32::consts::LOG2_E);
        a * (self.damping * 10.0).min(1.0)
    }

    /// Target of every smoothed control from the raw parameters.
    fn targets(&self) -> [f32; NC] {
        let sr = self.sample_rate;
        let alg = &ALGORITHMS[self.algorithm];
        let scale = self.scale();
        let mut t = [0.0f32; NC];
        t[C_PRE] = self.predelay_ms * 0.001 * sr;
        t[C_DECAY] = self.decay;
        t[C_DAMP] = self.damp_pole();
        t[C_LOWCUT] =
            1.0 - exp2_det(-std::f32::consts::TAU * self.lowcut / sr * std::f32::consts::LOG2_E);
        t[C_EARLY] = (2.0 * self.early).min(1.0) * EARLY_GAIN;
        // Late level compensation: the steady-state level of a reverb grows with sqrt(decay);
        // a quarter-power law keeps the perceived wet level within a few dB over 0.2..20 s.
        let comp = (2.0 / self.decay).sqrt().sqrt();
        t[C_LATE] = (2.0 * (1.0 - self.early)).min(1.0) * comp;
        t[C_WIDTH] = self.width;
        t[C_MOD] = self.modulation * alg.mod_scale * 0.0004 * sr;
        t[C_MIX] = self.mix;
        t[C_OUT] = exp2_det(self.output_db * (std::f32::consts::LOG2_10 / 20.0));
        t[C_FREEZE] = if self.freeze { 1.0 } else { 0.0 };
        t[C_APG] = alg.ap_g;
        for i in 0..FDN_LINES {
            t[C_LINE + i] = alg.lines_ms[i] * 0.001 * sr * scale;
        }
        for ch in 0..2 {
            for k in 0..ER_TAPS {
                let (ms, g) = alg.early[ch][k];
                t[C_ERT + ch * ER_TAPS + k] = ms * 0.001 * sr * scale;
                t[C_ERG + ch * ER_TAPS + k] = g * alg.early_scale;
            }
        }
        t
    }

    fn set_smoothing_times(&mut self) {
        let sr = self.sample_rate;
        for (i, c) in self.ctl.iter_mut().enumerate() {
            let tau = match i {
                C_PRE | C_EARLY | C_LATE | C_WIDTH | C_OUT => TAU_FAST_MS,
                C_MIX => 20.0,
                C_FREEZE => TAU_FREEZE_MS,
                C_DECAY | C_DAMP | C_LOWCUT | C_MOD | C_APG => TAU_MED_MS,
                i if (C_LINE..C_ERG).contains(&i) => TAU_GEOMETRY_MS,
                _ => TAU_TAP_GAIN_MS,
            };
            c.set_time(tau, sr, BATCH);
        }
    }

    fn reseed(&mut self) {
        let mut rng = Rng::new(self.seed);
        for l in &mut self.lfo {
            *l = ParabolicLfo::seeded(&mut rng, 0.2, 1.3, self.sample_rate);
        }
    }
}

impl Device for Reverb {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.sample_rate = sr;
        self.seed = info.seed;
        let ceil = |x: f32| x.ceil() as usize;
        let pre = ceil(MAX_PREDELAY_MS * 0.001 * sr) + 4;
        self.pre_l = DelayLine::new(pre);
        self.pre_r = DelayLine::new(pre);
        let max_er = ALGORITHMS
            .iter()
            .flat_map(|a| a.early.iter().flatten())
            .fold(0.0f32, |m, (ms, _)| m.max(*ms));
        let er = ceil(max_er * 0.001 * sr * MAX_SCALE) + 8;
        self.er_l = DelayLine::new(er);
        self.er_r = DelayLine::new(er);
        let max_line = ALGORITHMS
            .iter()
            .flat_map(|a| a.lines_ms.iter())
            .fold(0.0f32, |m, ms| m.max(*ms));
        let max_delay = ceil(max_line * 0.001 * sr * MAX_SCALE + MAX_MOD_SECONDS * sr) + 8;
        let ratio = sr / 48_000.0;
        let round = |x: f32| ((x * ratio).round() as usize).max(2);
        let loop_ap: [usize; FDN_LINES] = std::array::from_fn(|i| round(LOOP_AP_48K[i]));
        self.loop_ap = std::array::from_fn(|i| loop_ap[i] as f32);
        self.fdn = Fdn8::new(max_delay, loop_ap);
        self.in_ap = std::array::from_fn(|ch| {
            std::array::from_fn(|k| Allpass::new(round(IN_AP_48K[ch][k])))
        });
        self.set_smoothing_times();
        self.reset();
    }

    fn reset(&mut self) {
        self.pre_l.clear();
        self.pre_r.clear();
        self.er_l.clear();
        self.er_r.clear();
        self.fdn.clear();
        for pair in &mut self.in_ap {
            for a in pair {
                a.clear();
            }
        }
        self.hp = [0.0; 2];
        self.g_prev = [0.0; FDN_LINES];
        self.peak = [0.0; 2];
        self.tail_rms = 0.0;
        self.reseed();
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        match id.0 {
            P_ALGORITHM => self.algorithm = (v.max(0.0).round() as usize).min(2),
            P_PREDELAY => self.predelay_ms = v.clamp(0.0, MAX_PREDELAY_MS),
            P_SIZE => self.size = v.clamp(0.0, 1.0),
            P_DECAY => self.decay = v.clamp(0.2, 20.0),
            P_DAMPING => self.damping = v.clamp(0.0, 1.0),
            P_LOWCUT => self.lowcut = v.clamp(20.0, 1_000.0),
            P_EARLY => self.early = v.clamp(0.0, 1.0),
            P_WIDTH => self.width = v.clamp(0.0, 1.0),
            P_MODULATION => self.modulation = v.clamp(0.0, 1.0),
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            P_MIX_TRIM => self.mix_trim = v.clamp(-1.0, 1.0),
            P_FREEZE => self.freeze = v >= 0.5,
            P_OUTPUT => self.output_db = v.clamp(-24.0, 12.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let sr = self.sample_rate;
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);

        // Control rate: advance every smoothed control one batch and make its ramp.
        let mut target = self.targets();
        if ctx.is_connected(MIX_CV) {
            target[C_MIX] = add_norm(self.mix, self.mix_trim, batch_mean(ctx.input(MIX_CV)));
        }
        if self.fresh {
            for (c, t) in self.ctl.iter_mut().zip(&target) {
                c.set_immediate(*t);
            }
        }
        let mut ramps = [Ramp::constant(0.0); NC];
        for i in 0..NC {
            ramps[i] = self.ctl[i].advance(target[i], BATCH);
        }
        // Feedback gains: RT60 = decay, evaluated at the end of the batch, then ramped.
        let decay_end = ramps[C_DECAY].at(BATCH).max(0.05);
        let freeze_end = ramps[C_FREEZE].at(BATCH).clamp(0.0, 1.0);
        let mut g_end = [0.0f32; FDN_LINES];
        for i in 0..FDN_LINES {
            let d = ramps[C_LINE + i].at(BATCH);
            let trip = (d + self.loop_ap[i]) * DECAY_CAL;
            let g = exp2_det(-DECAY_K * trip / (sr * decay_end)).min(1.0);
            g_end[i] = g + (1.0 - g) * freeze_end;
        }
        if self.fresh {
            self.g_prev = g_end;
            self.fresh = false;
        }
        let g_start = self.g_prev;
        self.g_prev = g_end;
        let g_inc: [f32; FDN_LINES] =
            std::array::from_fn(|i| (g_end[i] - g_start[i]) / BATCH as f32);

        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut peak = [0.0f32; 2];
        let mut tail_sq = 0.0f32;
        for n in 0..BATCH {
            let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
            let (dry_l, dry_r) = (in_l[n], in_r[n]);
            let (xl, xr) = (finite(dry_l), finite(dry_r));

            // Pre-delay (write first: delay 0 is the current sample), then low cut.
            self.pre_l.write(xl);
            self.pre_r.write(xr);
            let pd = ramps[C_PRE].at(n) + 1.0;
            let (pl, pr) = (self.pre_l.read(pd), self.pre_r.read(pd));
            let a_lc = ramps[C_LOWCUT].at(n);
            self.hp[0] = flush_denormal(self.hp[0] + a_lc * (pl - self.hp[0]));
            self.hp[1] = flush_denormal(self.hp[1] + a_lc * (pr - self.hp[1]));
            let (hl, hr) = (pl - self.hp[0], pr - self.hp[1]);

            // Early taps: left output reads the left line on even taps and the right line on
            // odd taps; the right output the other way round.
            let mut early_l = 0.0f32;
            let mut early_r = 0.0f32;
            for k in 0..ER_TAPS {
                let (tl, gl) = (ramps[C_ERT + k].at(n), ramps[C_ERG + k].at(n));
                let (tr, gr) = (
                    ramps[C_ERT + ER_TAPS + k].at(n),
                    ramps[C_ERG + ER_TAPS + k].at(n),
                );
                if k % 2 == 0 {
                    early_l += gl * self.er_l.read(tl);
                    early_r += gr * self.er_r.read(tr);
                } else {
                    early_l += gl * self.er_r.read(tl);
                    early_r += gr * self.er_l.read(tr);
                }
            }
            self.er_l.write(hl);
            self.er_r.write(hr);

            // Late tail.
            let ap_g = ramps[C_APG].at(n);
            let mut dl = hl;
            let mut dr = hr;
            for k in 0..2 {
                dl = self.in_ap[0][k].process(dl, IN_AP_G);
                dr = self.in_ap[1][k].process(dr, IN_AP_G);
            }
            let frz = ramps[C_FREEZE].at(n).clamp(0.0, 1.0);
            let inj_gain = INJECT_GAIN * (1.0 - frz);
            let damp = ramps[C_DAMP].at(n) * (1.0 - frz);
            let depth = ramps[C_MOD].at(n);
            let mut delays = [0.0f32; FDN_LINES];
            let mut gains = [0.0f32; FDN_LINES];
            let mut inject = [0.0f32; FDN_LINES];
            for i in 0..FDN_LINES {
                delays[i] = ramps[C_LINE + i].at(n) + depth * self.lfo[i].next_sample();
                gains[i] = g_start[i] + g_inc[i] * n as f32;
                inject[i] = inj_gain * INJECT_SIGN[i] * if i % 2 == 0 { dl } else { dr };
            }
            let lines = self.fdn.tick(&delays, &gains, damp, ap_g, &inject);
            let mut late_l = 0.0f32;
            let mut late_r = 0.0f32;
            for i in 0..FDN_LINES {
                late_l += TAP_L[i] * lines[i];
                late_r += TAP_R[i] * lines[i];
            }
            late_l *= LATE_GAIN;
            late_r *= LATE_GAIN;
            tail_sq += late_l * late_l + late_r * late_r;

            // Balance, width, output gain, mix.
            let (eg, lg) = (ramps[C_EARLY].at(n), ramps[C_LATE].at(n));
            let wl = early_l * eg + late_l * lg;
            let wr = early_r * eg + late_r * lg;
            let m = 0.5 * (wl + wr);
            let s = 0.5 * (wl - wr) * ramps[C_WIDTH].at(n);
            let og = ramps[C_OUT].at(n);
            let mix = ramps[C_MIX].at(n);
            let wet_l = (m + s).clamp(-WET_LIMIT, WET_LIMIT) * og;
            let wet_r = (m - s).clamp(-WET_LIMIT, WET_LIMIT) * og;
            let (yl, yr) = if mix == 0.0 {
                (dry_l, dry_r)
            } else {
                (
                    dry_l * (1.0 - mix) + wet_l * mix,
                    dry_r * (1.0 - mix) + wet_r * mix,
                )
            };
            out_l[n] = yl;
            out_r[n] = yr;
            peak[0] = peak[0].max(yl.abs());
            peak[1] = peak[1].max(yr.abs());
        }
        self.peak = peak;
        self.tail_rms = (tail_sq / (2.0 * BATCH as f32)).sqrt();
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let values = [self.peak[0], self.peak[1], self.tail_rms];
        let n = values.len().min(out.len());
        out[..n].copy_from_slice(&values[..n]);
        n
    }
}
