//! `atomik.character` ("Character"): a stereo insert effect with 16 character presets in a 4 x 4
//! matrix, an input gain, a macro **Transform** morph, dry/wet and volume, and CV inputs with
//! back-panel trims for Transform and Dry/Wet. An original design with its own preset names.
//!
//! # Signal flow
//!
//! ```text
//! in_l/in_r (in_r unconnected: in_l feeds both) -> x
//! x ---------------------------------------> dry delay (the preset's latency) -+
//! x -> input gain (4 x^2, second-order smoother) -> chain (+ outgoing chain during a fade) -+-> mix -> volume -> safety -> out
//! CV: transform_cv_in, mix_cv_in -> trim -> clamp -> smoother -> Transform and mix per sample
//! Transform -> every 16 frames: the voicing resolves the Morph values of the active chain
//! x -> presence gate -> generated noise of the chains (only while programme is present)
//! ```
//!
//! * **Control rate.** The engine batch is 64 frames; the device runs it as four blocks of
//!   [`SUB`] = 16 frames. Per block it reads the per-sample CV values, smooths them, resolves the
//!   morph and runs the chain(s). The batch size is fixed, so the output never depends on the
//!   host block size.
//! * **Gain staging.** The dry tap is **before** the input gain and delayed by the active preset's
//!   latency (bit for bit: `mix = 0` at volume 0 dB is the input delayed by the latency, for any
//!   input gain); the mix is a linear amplitude crossfade; volume (`4 x^2`) comes after it; a
//!   transparent safety acts beyond +-4.0 only and bounds the output at 8.
//! * **Preset switch.** A change of `preset` starts a 30 ms crossfade (smoothstep weight, rounded up to whole
//!   16-frame blocks) from the outgoing chain to the incoming one, which starts from reset
//!   state; the outgoing chain is reset when the fade ends. A request during a fade is queued
//!   and the last one wins. The dry path is not involved while both chains share a latency; when
//!   they do not (a switch across the latency classes) the dry tap crossfades between the two
//!   delays with the same weight.
//! * **Latency (tiered).** Every chain declares its latency
//!   ([`PresetChain::latency`]): the short class ([`short_latency`]) for every preset without
//!   the spectral stage, larger for a long class chain. `latency_frames` reports the active
//!   chain's value and changes only **when a fade across classes ends** (the engine's PDC then
//!   re-aligns once).
//! * **Meters** (`meter_values`, 4): input peak after the input gain (larger of the two
//!   channels, instant attack), output peak, effective Transform, wet activity.
//!
//! # Tables
//!
//! `data/character-voicing.csv` and `data/character-routes.csv` are embedded at compile time and
//! carry every number of the voicing (see [`voicing`]).
//! All 16 presets are voiced; presets 0 to 15 are rack presets except `wide`.

pub mod chain;
pub mod rack;
mod row1;
mod row2;
mod row3;
mod row4;
pub mod stages;
#[cfg(test)]
mod tests;
pub mod voicing;

use crate::bypass::{Bypassable, bypass_param};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::rng::hash64;
use atomik_dsp::{PresenceGate, Smoother2, SmootherClass};
use chain::{Bus, Ctx, IMPLEMENTED_PRESETS, PresetChain, Routing, SUB};
use voicing::{ChainSpec, Morph, Voicing, VoicingError};

pub use chain::short_latency;

const P_GAIN: u32 = 0;
const P_PRESET: u32 = 1;
const P_TRANSFORM: u32 = 2;
const P_MIX: u32 = 3;
const P_VOLUME: u32 = 4;
const P_TCV_TRIM: u32 = 5;
const P_MCV_TRIM: u32 = 6;

/// The preset names shown in the matrix (keys are in [`voicing::PRESET_KEYS`]).
pub const PRESET_NAMES: [&str; 16] = [
    "Reel",
    "Loudness",
    "Sparkle",
    "Low End",
    "Wide",
    "Pocket Radio",
    "Camcorder",
    "Dusty Groove",
    "Lossy",
    "Swirl",
    "Torn Cone",
    "Tin Robot",
    "Bit Bender",
    "Spin Cycle",
    "Ion Sweep",
    "Ghost Pipes",
];

static PARAMS: [ParamDescriptor; 8] = [
    ParamDescriptor {
        id: ParamId(P_GAIN),
        key: "gain",
        name: "Input Gain",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_PRESET),
        key: "preset",
        name: "Preset",
        min: 0.0,
        max: 15.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&PRESET_NAMES),
        discrete: true,
    },
    ParamDescriptor {
        id: ParamId(P_TRANSFORM),
        key: "transform",
        name: "Transform",
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
        name: "Dry/Wet",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_VOLUME),
        key: "volume",
        name: "Volume",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_TCV_TRIM),
        key: "transform_cv_trim",
        name: "Transform CV trim",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    ParamDescriptor {
        id: ParamId(P_MCV_TRIM),
        key: "mix_cv_trim",
        name: "Dry/Wet CV trim",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    },
    bypass_param(7),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const TRANSFORM_CV_IN: SocketId = SocketId(2);
const MIX_CV_IN: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);

static SOCKETS: [SocketDescriptor; 6] = [
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
        id: TRANSFORM_CV_IN,
        key: "transform_cv_in",
        name: "Transform CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: MIX_CV_IN,
        key: "mix_cv_in",
        name: "Dry/Wet CV",
        kind: SocketKind::Cv,
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

/// Static description of the Character device.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.character",
    name: "Character",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Character`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(Character::new())),
    }
}

/// The input gain or volume of a knob position: `4 x^2` (silence at 0, exactly 1 at 0.5, 4 at 1).
pub fn knob_gain(x: f32) -> f32 {
    let x = if x.is_nan() { 0.5 } else { x.clamp(0.0, 1.0) };
    4.0 * x * x
}

/// [`knob_gain`] in decibels (minus infinity at 0), for a display.
pub fn knob_gain_db(x: f32) -> f32 {
    let g = knob_gain(x);
    if g <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * g.log10()
    }
}

/// The voicing table (original numbers, see the module docs).
pub(crate) const VOICING_CSV: &str = include_str!("../data/character-voicing.csv");
/// The send table of the rack presets.
pub(crate) const ROUTES_CSV: &str = include_str!("../data/character-routes.csv");

/// The chain of preset `i`.
pub(crate) fn build_chain(i: usize) -> Box<dyn PresetChain> {
    match i {
        0..=3 => row1::build(i),
        4..=7 => row2::build(i),
        8..=11 => row3::build(i),
        _ => row4::build(i),
    }
}

/// The rack definitions of all rack presets with their indices.
#[cfg(test)]
pub(crate) fn all_rack_defs() -> Vec<(usize, &'static rack::RackDef)> {
    row1::racks()
        .into_iter()
        .chain(row2::racks())
        .chain(row3::racks())
        .chain(row4::racks())
        .collect()
}

/// Loads voicing tables against the chains' specs.
pub(crate) fn load_voicing(voicing_csv: &str, routes_csv: &str) -> Result<Voicing, VoicingError> {
    let specs: Vec<ChainSpec> = (0..16).map(|i| build_chain(i).spec()).collect();
    Voicing::load(voicing_csv, routes_csv, &specs)
}

/// The embedded tables. If they are rejected (the unit tests make that impossible) the device
/// falls back to an empty voicing, whose chains pass the signal through.
fn embedded_voicing() -> Voicing {
    let ok = |v: &Voicing| (0..16).all(|i| build_chain(i).check(v.preset(i), 48_000.0).is_ok());
    load_voicing(VOICING_CSV, ROUTES_CSV)
        .ok()
        .filter(ok)
        .unwrap_or_else(Voicing::empty)
}

/// Length of the dry ring (a power of two above the largest latency a chain may declare).
const DRY_RING: usize = 8192;
/// Length of a preset crossfade in seconds.
const FADE_S: f32 = 0.030;
/// 90 % rise time of the CV and mix smoothers in ms (inside the 5 to 15 ms window).
const CV_RISE_MS: f32 = 8.0;
/// Output safety knee: the output is untouched below it.
const SAFE_KNEE: f32 = 4.0;

/// Bounds the output: untouched up to +-4, then a smooth saturation towards +-8.
#[inline]
fn safety(y: f32) -> f32 {
    if !y.is_finite() {
        return 0.0;
    }
    let a = y.abs();
    if a <= SAFE_KNEE {
        y
    } else {
        let u = (a - SAFE_KNEE) / SAFE_KNEE;
        (SAFE_KNEE + SAFE_KNEE * u / (1.0 + u)).copysign(y)
    }
}

#[inline]
fn clean(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// The Character device.
pub struct Character {
    sample_rate: f32,
    seed: u64,
    build: fn(usize) -> Box<dyn PresetChain>,
    voicing: Voicing,
    chains: Vec<Box<dyn PresetChain>>,
    // Parameters.
    gain_knob: f32,
    preset_req: usize,
    transform: f32,
    mix: f32,
    volume_knob: f32,
    tcv_trim: f32,
    mcv_trim: f32,
    // State.
    fresh: bool,
    active: usize,
    incoming: Option<usize>,
    queued: Option<usize>,
    fade_pos: u32,
    fade_len: u32,
    published_latency: u32,
    gain_sm: Smoother2,
    vol_sm: Smoother2,
    t_sm: Smoother2,
    mix_sm: Smoother2,
    presence: PresenceGate,
    dry: [Vec<f32>; 2],
    dry_pos: usize,
    morph_active: Morph,
    morph_in: Morph,
    meters: [f32; 4],
}

impl Default for Character {
    fn default() -> Self {
        Self::new()
    }
}

impl Character {
    /// A Character with the embedded tables (48 kHz until `prepare`).
    pub fn new() -> Self {
        Self::assemble(build_chain, embedded_voicing())
    }

    /// A Character whose chains come from `build` and which has no voicing rows (tests of the
    /// shell with stand-in chains).
    #[cfg(test)]
    pub(crate) fn with_chains(build: fn(usize) -> Box<dyn PresetChain>) -> Self {
        Self::assemble(build, Voicing::empty())
    }

    /// A Character with the given tables (tests).
    #[cfg(test)]
    pub(crate) fn with_voicing(v: Voicing) -> Self {
        Self::assemble(build_chain, v)
    }

    fn assemble(build: fn(usize) -> Box<dyn PresetChain>, voicing: Voicing) -> Self {
        let sr = 48_000.0;
        let mut c = Self {
            sample_rate: sr,
            seed: 0,
            build,
            voicing,
            chains: Vec::new(),
            gain_knob: 0.5,
            preset_req: 0,
            transform: 0.5,
            mix: 1.0,
            volume_knob: 0.5,
            tcv_trim: 1.0,
            mcv_trim: 1.0,
            fresh: true,
            active: 0,
            incoming: None,
            queued: None,
            fade_pos: 0,
            fade_len: 0,
            published_latency: 0,
            gain_sm: Smoother2::with_class(1.0, SmootherClass::Fast, sr),
            vol_sm: Smoother2::with_class(1.0, SmootherClass::Slow, sr),
            t_sm: Smoother2::new(0.5, CV_RISE_MS, sr),
            mix_sm: Smoother2::new(1.0, CV_RISE_MS, sr),
            presence: PresenceGate::new(sr),
            dry: [vec![0.0; DRY_RING], vec![0.0; DRY_RING]],
            dry_pos: 0,
            morph_active: Morph::default(),
            morph_in: Morph::default(),
            meters: [0.0; 4],
        };
        c.setup(sr, 0);
        c
    }

    /// The preset currently sounding (the target of a running fade is not yet active).
    pub fn active_preset(&self) -> usize {
        self.active
    }

    fn setup(&mut self, sr: f32, seed: u64) {
        self.sample_rate = sr;
        self.seed = seed;
        self.chains = (0..16)
            .map(|i| {
                let mut c = (self.build)(i);
                c.prepare(sr, hash64(seed, 0xC4A2, i as u64), self.voicing.preset(i));
                c
            })
            .collect();
        self.gain_sm.set_time(SmootherClass::Fast.rise_ms(), sr);
        self.vol_sm.set_time(SmootherClass::Slow.rise_ms(), sr);
        self.t_sm.set_time(CV_RISE_MS, sr);
        self.mix_sm.set_time(CV_RISE_MS, sr);
        self.presence = PresenceGate::new(sr);
        let blocks = (FADE_S * sr / SUB as f32).ceil() as u32;
        self.fade_len = blocks.max(1) * SUB as u32;
        self.reset_state();
    }

    fn reset_state(&mut self) {
        for c in &mut self.chains {
            c.reset();
        }
        for d in &mut self.dry {
            d.fill(0.0);
        }
        self.dry_pos = 0;
        self.presence.reset();
        self.incoming = None;
        self.queued = None;
        self.fade_pos = 0;
        self.meters = [0.0; 4];
        self.fresh = true;
    }

    fn apply_presence(&mut self, preset: usize) {
        let (hold, fade) = self.chains[preset].presence();
        self.presence.set_hold_s(hold);
        self.presence.set_fade_s(fade);
    }

    fn start_fade(&mut self, to: usize) {
        self.chains[to].reset();
        self.incoming = Some(to);
        self.fade_pos = 0;
        self.apply_presence(to);
    }

    fn finish_fade(&mut self) {
        if let Some(to) = self.incoming.take() {
            let old = self.active;
            self.active = to;
            self.chains[old].reset();
            self.published_latency = self.chains[to].latency();
            if let Some(q) = self.queued.take()
                && q != to
            {
                self.start_fade(q);
            }
        }
    }

    #[inline]
    fn dry_at(&self, ch: usize, latency: usize) -> f32 {
        let l = latency.min(DRY_RING - 1);
        self.dry[ch][(self.dry_pos.wrapping_sub(l)) & (DRY_RING - 1)]
    }
}

impl Device for Character {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.setup(info.sample_rate.hz() as f32, info.seed);
    }

    fn reset(&mut self) {
        self.reset_state();
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        let Some(p) = PARAMS.get(id.0 as usize) else {
            return;
        };
        let v = v.clamp(p.min, p.max);
        match id.0 {
            P_GAIN => self.gain_knob = v,
            P_PRESET => self.preset_req = (v.round() as usize).min(15),
            P_TRANSFORM => self.transform = v,
            P_MIX => self.mix = v,
            P_VOLUME => self.volume_knob = v,
            P_TCV_TRIM => self.tcv_trim = v,
            P_MCV_TRIM => self.mcv_trim = v,
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let cv_t = *ctx.input(TRANSFORM_CV_IN);
        let cv_m = *ctx.input(MIX_CV_IN);
        let r_connected = ctx.is_connected(IN_R);

        let gain_target = knob_gain(self.gain_knob);
        let vol_target = knob_gain(self.volume_knob);
        if self.fresh {
            self.active = self.preset_req;
            self.published_latency = self.chains[self.active].latency();
            self.apply_presence(self.active);
            self.gain_sm.set_immediate(gain_target);
            self.vol_sm.set_immediate(vol_target);
            self.t_sm
                .set_immediate((self.transform + self.tcv_trim * clean(cv_t[0])).clamp(0.0, 1.0));
            self.mix_sm
                .set_immediate((self.mix + self.mcv_trim * clean(cv_m[0])).clamp(0.0, 1.0));
            self.fresh = false;
        } else {
            let target = self.incoming.unwrap_or(self.active);
            if self.preset_req != target {
                if self.incoming.is_none() {
                    self.start_fade(self.preset_req);
                } else {
                    self.queued = Some(self.preset_req);
                }
            } else if self.incoming.is_some() {
                self.queued = None;
            }
        }
        self.gain_sm.set_target(gain_target);
        self.vol_sm.set_target(vol_target);

        let mut out = [[0.0f32; BATCH]; 2];
        let (mut in_peak, mut out_peak, mut t_last) = (0.0f32, 0.0f32, 0.0f32);

        for blk in 0..BATCH / SUB {
            let base = blk * SUB;
            let mut wet_in: Bus = [[0.0; SUB]; 2];
            let mut dry_a: Bus = [[0.0; SUB]; 2];
            let mut dry_b: Bus = [[0.0; SUB]; 2];
            let mut gain_arr = [0.0f32; SUB];
            let mut presence_arr = [0.0f32; SUB];
            let mut mix_arr = [0.0f32; SUB];
            let mut vol_arr = [0.0f32; SUB];
            let lat_a = self.chains[self.active].latency() as usize;
            let lat_b = self.incoming.map(|i| self.chains[i].latency() as usize);

            for n in 0..SUB {
                let i = base + n;
                let xl = clean(in_l[i]);
                let xr = if r_connected { clean(in_r[i]) } else { xl };
                self.dry[0][self.dry_pos] = xl;
                self.dry[1][self.dry_pos] = xr;
                dry_a[0][n] = self.dry_at(0, lat_a);
                dry_a[1][n] = self.dry_at(1, lat_a);
                if let Some(lb) = lat_b {
                    dry_b[0][n] = self.dry_at(0, lb);
                    dry_b[1][n] = self.dry_at(1, lb);
                }
                self.dry_pos = (self.dry_pos + 1) & (DRY_RING - 1);

                let mt = (self.mix + self.mcv_trim * clean(cv_m[i])).clamp(0.0, 1.0);
                self.mix_sm.set_target(mt);
                mix_arr[n] = self.mix_sm.next_value().clamp(0.0, 1.0);
                let tt = (self.transform + self.tcv_trim * clean(cv_t[i])).clamp(0.0, 1.0);
                self.t_sm.set_target(tt);
                t_last = self.t_sm.next_value().clamp(0.0, 1.0);
                let g = self.gain_sm.next_value();
                gain_arr[n] = g;
                vol_arr[n] = self.vol_sm.next_value();
                presence_arr[n] = self.presence.process(xl.abs().max(xr.abs()));
                wet_in[0][n] = xl * g;
                wet_in[1][n] = xr * g;
                in_peak = in_peak.max(wet_in[0][n].abs()).max(wet_in[1][n].abs());
            }

            let cx = Ctx {
                presence: &presence_arr,
                gain: &gain_arr,
            };
            self.voicing
                .preset(self.active)
                .resolve(t_last, &mut self.morph_active);
            let mut a = wet_in;
            run_chain(
                &mut self.chains[self.active],
                &mut a,
                &self.morph_active,
                &cx,
            );
            let wet = if let Some(inc) = self.incoming {
                self.voicing.preset(inc).resolve(t_last, &mut self.morph_in);
                let mut b = wet_in;
                run_chain(&mut self.chains[inc], &mut b, &self.morph_in, &cx);
                let mut mixed: Bus = [[0.0; SUB]; 2];
                for n in 0..SUB {
                    let w = (self.fade_pos + n as u32 + 1) as f32 / self.fade_len as f32;
                    let w = w.min(1.0);
                    // Smoothstep weight: zero slope at the start, so the filter transients of
                    // the incoming chain (which starts from reset) enter without a corner.
                    let w = w * w * (3.0 - 2.0 * w);
                    for ch in 0..2 {
                        mixed[ch][n] = (1.0 - w) * a[ch][n] + w * b[ch][n];
                        if lat_b != Some(lat_a) {
                            dry_a[ch][n] = (1.0 - w) * dry_a[ch][n] + w * dry_b[ch][n];
                        }
                    }
                }
                mixed
            } else {
                a
            };

            for n in 0..SUB {
                let m = mix_arr[n];
                for ch in 0..2 {
                    let y = ((1.0 - m) * dry_a[ch][n] + m * wet[ch][n]) * vol_arr[n];
                    let y = safety(y);
                    out[ch][base + n] = y;
                    out_peak = out_peak.max(y.abs());
                }
            }

            if self.incoming.is_some() {
                self.fade_pos += SUB as u32;
                if self.fade_pos >= self.fade_len {
                    self.finish_fade();
                }
            }
        }

        *ctx.output(OUT_L) = out[0];
        *ctx.output(OUT_R) = out[1];
        let act = self.chains[self.active].activity();
        let act = self
            .incoming
            .map_or(act, |i| act.max(self.chains[i].activity()));
        self.meters = [in_peak, out_peak, t_last, act.min(1.0)];
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = out.len().min(self.meters.len());
        out[..n].copy_from_slice(&self.meters[..n]);
        n
    }

    fn latency_frames(&self) -> u32 {
        // Right after prepare, reset or a preset parameter change before the first batch the
        // chain that will play is already known, so the engine reads the right value when it
        // inserts the device (no spurious latency bump on the first batch).
        if self.fresh {
            self.chains.get(self.preset_req).map_or(0, |c| c.latency())
        } else {
            self.published_latency
        }
    }

    fn variable_latency(&self) -> bool {
        true
    }
}

/// Feeds a chain (mono-summed in and out when its routing says so) in place.
fn run_chain(chain: &mut Box<dyn PresetChain>, bus: &mut Bus, m: &Morph, ctx: &Ctx) {
    let mono = chain.routing() == Routing::MonoSum;
    let sum_to_mono = |bus: &mut Bus| {
        let [l, r] = bus;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let s = 0.5 * (*a + *b);
            *a = s;
            *b = s;
        }
    };
    if mono {
        sum_to_mono(bus);
    }
    let [l, r] = &mut *bus;
    chain.process(l, r, m, ctx);
    if mono {
        sum_to_mono(bus);
    }
}

/// The number of presets that are fully voiced (all 16).
pub const fn voiced_presets() -> usize {
    IMPLEMENTED_PRESETS
}
