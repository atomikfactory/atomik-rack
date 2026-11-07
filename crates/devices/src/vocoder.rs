//! `atomik.vocoder`: a 4 to 32 band channel vocoder.
//!
//! The **modulator** is the main input (`in_l` + `in_r` summed to mono); the **carrier** is the
//! second stereo pair (`carrier_l`, `carrier_r`). With no carrier cabled the vocoder runs on an
//! internal white noise, so it works as a whisper effect on its own. The output is the carrier,
//! band by band, scaled by the modulator's level in the same band.
//!
//! # Signal flow (per sample)
//!
//! ```text
//! modulator -> N analysis bands (2 x 12 dB bandpass) -> rectify -> attack/release follower -+
//!                                                                                           v
//! carrier L/R -> N synthesis bands (2 x 12 dB bandpass, same centres) ------------------> x gain -> sum
//!                                                                                           |
//! modulator -> highpass 6 kHz -> follower -> internal noise ------ x hiss ------------------+
//! ```
//!
//! * **Bands.** Centres are log-spaced from `low` to `high`; each band is two cascaded TPT
//!   state-variable bandpasses of unit peak gain. `bandwidth` scales the overlap (1 means
//!   neighbours cross at about -3 dB per stage). Changing `bands`, `low`, `high` or `bandwidth`
//!   retunes at the next batch.
//! * **Followers.** Per band, a one-pole follower of the rectified analysis output with separate
//!   `attack` and `release` times. The band gain is the follower times a fixed 1.57 (the peak to
//!   mean-rectified ratio of a sine).
//! * **Hiss.** Consonants are mostly noise above the top band: the modulator is highpassed at
//!   6 kHz and its follower scales the internal noise, mixed in by `hiss`, so the output stays
//!   intelligible.
//! * **Mix.** `out = dry * (1 - mix) + wet * mix * output`; at `mix == 0` the input is copied
//!   bit-exactly. Latency is 0.
//! * **Determinism.** Coefficients use the deterministic `exp2`/`log2` and `tan`; the internal
//!   noise is a seeded generator ([`PrepareInfo::seed`]).
//!
//! A 512-band STFT mode is not part of this device.

use crate::bypass::{Bypassable, bypass_param};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{BlockSmoother, Ramp, Rng, Svf, exp2_det, flush_denormal, log2_det};

#[cfg(test)]
mod tests;

/// Most bands.
pub const MAX_BANDS: usize = 32;
/// Fewest bands.
pub const MIN_BANDS: usize = 4;
const HISS_CORNER_HZ: f32 = 6_000.0;
/// Peak to mean-rectified ratio of a sine.
const FOLLOW_COMP: f32 = 1.571;
const WET_LIMIT: f32 = 32.0;

const P_BANDS: u32 = 0;
const P_LOW: u32 = 1;
const P_HIGH: u32 = 2;
const P_BANDWIDTH: u32 = 3;
const P_ATTACK: u32 = 4;
const P_RELEASE: u32 = 5;
const P_HISS: u32 = 6;
const P_MIX: u32 = 7;
const P_OUTPUT: u32 = 8;

static PARAMS: [ParamDescriptor; 10] = [
    ParamDescriptor {
        id: ParamId(P_BANDS),
        key: "bands",
        name: "Bands",
        min: MIN_BANDS as f32,
        max: MAX_BANDS as f32,
        default: 16.0,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_LOW),
        key: "low",
        name: "Low",
        min: 40.0,
        max: 500.0,
        default: 100.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_HIGH),
        key: "high",
        name: "High",
        min: 2_000.0,
        max: 16_000.0,
        default: 8_000.0,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_BANDWIDTH),
        key: "bandwidth",
        name: "Bandwidth",
        min: 0.5,
        max: 2.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_ATTACK),
        key: "attack",
        name: "Attack",
        min: 0.1,
        max: 50.0,
        default: 2.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_RELEASE),
        key: "release",
        name: "Release",
        min: 1.0,
        max: 500.0,
        default: 40.0,
        taper: Taper::Exponential,
        unit: Unit::Ms,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_HISS),
        key: "hiss",
        name: "Hiss",
        min: 0.0,
        max: 1.0,
        default: 0.3,
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
    bypass_param(9),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const CARRIER_L: SocketId = SocketId(2);
const CARRIER_R: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);

const fn socket(
    id: SocketId,
    key: &'static str,
    name: &'static str,
    direction: Direction,
) -> SocketDescriptor {
    SocketDescriptor {
        id,
        key,
        name,
        kind: SocketKind::Audio,
        direction,
    }
}

static SOCKETS: [SocketDescriptor; 6] = [
    socket(IN_L, "in_l", "In L", Direction::Input),
    socket(IN_R, "in_r", "In R", Direction::Input),
    socket(CARRIER_L, "carrier_l", "Carrier L", Direction::Input),
    socket(CARRIER_R, "carrier_r", "Carrier R", Direction::Input),
    socket(OUT_L, "out_l", "Out L", Direction::Output),
    socket(OUT_R, "out_r", "Out R", Direction::Output),
];

/// Static description of the vocoder.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.vocoder",
    name: "Vocoder",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Vocoder`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(Vocoder::new())),
    }
}

const C_HISS: usize = 0;
const C_MIX: usize = 1;
const C_OUT: usize = 2;
const NC: usize = 3;

/// The vocoder device.
#[derive(Debug, Clone)]
pub struct Vocoder {
    sample_rate: f32,
    seed: u64,
    bands: usize,
    low: f32,
    high: f32,
    bandwidth: f32,
    attack_ms: f32,
    release_ms: f32,
    hiss: f32,
    mix: f32,
    output_db: f32,
    fresh: bool,
    retune: bool,
    ctl: [BlockSmoother; NC],
    /// Analysis bands, two stages each.
    analysis: [[Svf; 2]; MAX_BANDS],
    /// Synthesis bands: band, channel, stage.
    synth: [[[Svf; 2]; 2]; MAX_BANDS],
    /// Damping `k = 1/Q` of the bands (the bandpass is scaled by it to unit peak).
    k: [f32; MAX_BANDS],
    env: [f32; MAX_BANDS],
    attack: f32,
    release: f32,
    hiss_hp: Svf,
    hiss_env: f32,
    noise: [Rng; 2],
}

impl Default for Vocoder {
    fn default() -> Self {
        Self::new()
    }
}

/// `1 - exp(-1000 / (ms * sr))`: the one-pole coefficient of a time constant.
fn pole(ms: f32, sr: f32) -> f32 {
    1.0 - exp2_det(-1_000.0 / (ms.max(0.01) * sr) * std::f32::consts::LOG2_E)
}

impl Vocoder {
    /// A vocoder with default params; `prepare` sets the rate.
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            seed: 0,
            bands: 16,
            low: 100.0,
            high: 8_000.0,
            bandwidth: 1.0,
            attack_ms: 2.0,
            release_ms: 40.0,
            hiss: 0.3,
            mix: 1.0,
            output_db: 0.0,
            fresh: true,
            retune: true,
            ctl: [BlockSmoother::new(0.0); NC],
            analysis: [[Svf::new(); 2]; MAX_BANDS],
            synth: [[[Svf::new(); 2]; 2]; MAX_BANDS],
            k: [1.0; MAX_BANDS],
            env: [0.0; MAX_BANDS],
            attack: 0.0,
            release: 0.0,
            hiss_hp: Svf::new(),
            hiss_env: 0.0,
            noise: [Rng::new(0), Rng::new(1)],
        }
    }

    /// Centre frequency of band `i` of `n`.
    fn centre(&self, i: usize, n: usize) -> f32 {
        let span = log2_det(self.high / self.low);
        self.low * exp2_det(span * i as f32 / (n - 1) as f32)
    }

    /// Recomputes the band filters.
    fn tune(&mut self) {
        let n = self.bands;
        let sr = self.sample_rate;
        let span = log2_det(self.high / self.low);
        let octaves = span / (n - 1) as f32 * self.bandwidth;
        // Q of a bandpass `octaves` wide: 1 / (2 sinh(ln2 / 2 * octaves)).
        let x = std::f32::consts::LN_2 * 0.5 * octaves;
        let e = exp2_det(x * std::f32::consts::LOG2_E);
        let k = (e - 1.0 / e).max(0.05);
        for i in 0..n {
            let g = Svf::cutoff_to_g_det(self.centre(i, n), sr);
            self.k[i] = k;
            for s in 0..2 {
                self.analysis[i][s].set_coefficients(g, k);
                for ch in 0..2 {
                    self.synth[i][ch][s].set_coefficients(g, k);
                }
            }
        }
        for i in n..MAX_BANDS {
            self.env[i] = 0.0;
            for s in 0..2 {
                self.analysis[i][s].reset();
                self.synth[i][0][s].reset();
                self.synth[i][1][s].reset();
            }
        }
        self.retune = false;
    }

    fn targets(&self) -> [f32; NC] {
        [
            self.hiss,
            self.mix,
            exp2_det(self.output_db * (std::f32::consts::LOG2_10 / 20.0)),
        ]
    }
}

impl Device for Vocoder {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let sr = info.sample_rate.hz() as f32;
        self.sample_rate = sr;
        self.seed = info.seed;
        for c in &mut self.ctl {
            c.set_time(20.0, sr, BATCH);
        }
        let g = Svf::cutoff_to_g_det(HISS_CORNER_HZ, sr);
        self.hiss_hp.set_coefficients(g, std::f32::consts::SQRT_2);
        self.retune = true;
        self.reset();
    }

    fn reset(&mut self) {
        for i in 0..MAX_BANDS {
            for s in 0..2 {
                self.analysis[i][s].reset();
                self.synth[i][0][s].reset();
                self.synth[i][1][s].reset();
            }
        }
        self.env = [0.0; MAX_BANDS];
        self.hiss_hp.reset();
        self.hiss_env = 0.0;
        self.noise = [Rng::new(self.seed), Rng::new(self.seed ^ 0x9E37_79B9)];
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        match id.0 {
            P_BANDS => {
                self.bands = (v.round() as usize).clamp(MIN_BANDS, MAX_BANDS);
                self.retune = true;
            }
            P_LOW => {
                self.low = v.clamp(40.0, 500.0);
                self.retune = true;
            }
            P_HIGH => {
                self.high = v.clamp(2_000.0, 16_000.0);
                self.retune = true;
            }
            P_BANDWIDTH => {
                self.bandwidth = v.clamp(0.5, 2.0);
                self.retune = true;
            }
            P_ATTACK => self.attack_ms = v.clamp(0.1, 50.0),
            P_RELEASE => self.release_ms = v.clamp(1.0, 500.0),
            P_HISS => self.hiss = v.clamp(0.0, 1.0),
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            P_OUTPUT => self.output_db = v.clamp(-24.0, 12.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let car_l = *ctx.input(CARRIER_L);
        let car_r = *ctx.input(CARRIER_R);
        let external = ctx.is_connected(CARRIER_L) || ctx.is_connected(CARRIER_R);
        if self.retune {
            self.tune();
        }
        let sr = self.sample_rate;
        self.attack = pole(self.attack_ms, sr);
        self.release = pole(self.release_ms, sr);
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
        let n = self.bands;
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        for t in 0..BATCH {
            let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
            let (dry_l, dry_r) = (in_l[t], in_r[t]);
            let m = 0.5 * (finite(dry_l) + finite(dry_r));
            let noise = [self.noise[0].next_bipolar(), self.noise[1].next_bipolar()];
            let (cl, cr) = if external {
                (finite(car_l[t]), finite(car_r[t]))
            } else {
                (noise[0], noise[1])
            };
            let (mut wl, mut wr) = (0.0f32, 0.0f32);
            for i in 0..n {
                let k = self.k[i];
                let a = self.analysis[i][0].process(m).bp * k;
                let a = self.analysis[i][1].process(a).bp * k;
                let level = a.abs();
                let c = if level > self.env[i] {
                    self.attack
                } else {
                    self.release
                };
                self.env[i] = flush_denormal(self.env[i] + (level - self.env[i]) * c);
                let gain = self.env[i] * FOLLOW_COMP;
                let l = self.synth[i][0][0].process(cl).bp * k;
                let l = self.synth[i][0][1].process(l).bp * k;
                let r = self.synth[i][1][0].process(cr).bp * k;
                let r = self.synth[i][1][1].process(r).bp * k;
                wl += l * gain;
                wr += r * gain;
            }
            // Consonants: the highpassed modulator's level on the internal noise.
            let h = self.hiss_hp.process(m).hp.abs();
            let c = if h > self.hiss_env {
                self.attack
            } else {
                self.release
            };
            self.hiss_env = flush_denormal(self.hiss_env + (h - self.hiss_env) * c);
            let hiss = ramps[C_HISS].at(t) * self.hiss_env * FOLLOW_COMP;
            wl += noise[0] * hiss;
            wr += noise[1] * hiss;

            let og = ramps[C_OUT].at(t);
            let wet_l = wl.clamp(-WET_LIMIT, WET_LIMIT) * og;
            let wet_r = wr.clamp(-WET_LIMIT, WET_LIMIT) * og;
            let mix = ramps[C_MIX].at(t);
            if mix == 0.0 {
                out_l[t] = dry_l;
                out_r[t] = dry_r;
            } else {
                out_l[t] = dry_l * (1.0 - mix) + wet_l * mix;
                out_r[t] = dry_r * (1.0 - mix) + wet_r * mix;
            }
        }
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
    }
}
