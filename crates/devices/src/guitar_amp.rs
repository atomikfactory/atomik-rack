//! `atomik.guitar-amp` ("Guitar Amp"): a guitar amplifier with a cabinet, chorus, delay and reverb,
//! and factory setups. An original design from general valve-amp topology, not a model of a
//! product. The blocks are in [`atomik_dsp::amp`].
//!
//! # Signal flow
//!
//! ```text
//! in L/R -> mono -> x input -> gate -> tight high-pass, mid boost
//!   -> [ up 4x -> preamp (3 stages, tone stack) -> power amp -> down ]
//!   -> cabinet -> chorus -> delay (ping-pong) -> reverb -> x level -> out L/R
//! ```
//!
//! * **Mono amp, stereo effects.** The amp sees the mono sum of the two inputs (the connected one
//!   alone when only one is cabled); the cabinet is mono, the chorus, delay and reverb make the
//!   stereo image. With no effect on, both outputs are the same signal.
//! * **Gate.** `gate` is the threshold in dB of a noise gate on the input (after `input`), 50 dB
//!   deep with a 0.5 ms attack, 30 ms hold and 80 ms release. At -80 dB the gate is off and the
//!   signal is not touched.
//! * **Input stage.** `tight` is the corner of a high-pass in front of the gain (20 Hz is open)
//!   and `boost` a mid boost at 780 Hz: the tight low end of a palm-muted chug.
//! * **Preamp.** `gain` 0..10 drives three cascaded asymmetric soft-clip stages (see
//!   [`atomik_dsp::amp`]); `bass`, `mid` and `treble` (0..10) are a passive-style tone stack
//!   between the second and third stage and interact with each other.
//! * **Power amp.** `master` (0..10) sets the drive into the power stage, `sag` (0..10) how much a
//!   loud passage squashes it, `presence` and `depth` (0..10) its high and low frequency response.
//!   `level` (dB) is the volume after everything.
//! * **Oversampling.** Preamp and power amp run at four times the sample rate between the halfband
//!   stages of [`atomik_dsp::oversample::Oversampler`], and every clip is antiderivative
//!   anti-aliased. The round trip delays the signal by [`LATENCY`] samples, reported through
//!   `latency_frames` (so plugin delay compensation lines the amp up with the rest of a project,
//!   and the bypass switch delays its dry path by the same amount); the effects add none.
//! * **Cabinet.** `cab`: open 1x12, 2x12, closed 4x12 or off; `mic` moves the microphone from
//!   the centre of the cone (bright) to its edge (dark, more low end). Synthesised from designed
//!   filters (no recorded impulse response).
//! * **Effects.** Chorus (`chorus.*`, a wet blend), a ping-pong delay (`delay.*`: `delay.time` in
//!   ms, or a note value at the project tempo with `delay.sync`; `delay.tone` darkens the repeats;
//!   it adds to the dry signal) and a plate-like reverb (`reverb.*`, adds to the dry signal). Each
//!   has an `on` switch and a mix; an effect whose mix has reached zero is not run, so `mix = 0`
//!   or `on = 0` leaves the signal bit for bit as it was, and a section switched back on starts
//!   from an empty line.
//! * **Setups.** `setup` selects a factory setup ([`setups`]). The device ignores it; the app
//!   turns a change of `setup` into a batch of param changes (one undo step).
//! * **Realtime.** Nothing allocates, locks or blocks in `process`, `reset` and `set_param`: the
//!   delay lines and the reverb are allocated in `prepare`. Non-finite input is treated as 0
//!   and every param is clamped, so the output is finite; a last clamp at +-16 bounds it.
//!   Deterministic: the reverb is seeded by the patch seed.
//! * **Meter values:** `[input peak, output peak, gate reduction in dB]`.

pub mod setups;

use crate::bypass::{Bypassable, bypass_param};
use crate::stereo_delay::SYNC_BEATS;
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::amp::{CabType, Cabinet, InputStage, PowerAmp, Preamp};
use atomik_dsp::oversample::{OversampleFactor, Oversampler};
use atomik_dsp::{
    BlockSmoother, Chorus, DelayLine, Gate, Room, RoomRange, exp2_det, flush_denormal,
};

#[cfg(test)]
mod tests;

/// Round-trip latency of the oversampler in samples.
pub const LATENCY: u32 = atomik_dsp::oversample::LATENCY_4X;

/// Names of the cabinets (`cab`).
pub const CAB_NAMES: [&str; 4] = ["1x12 open", "2x12", "4x12 closed", "off"];

/// `delay.sync` choices: free time, then the note values of [`SYNC_BEATS`].
pub const SYNC_NAMES: [&str; 13] = [
    "free", "1/32", "1/16T", "1/16", "1/16D", "1/8T", "1/8", "1/8D", "1/4T", "1/4", "1/4D", "1/2",
    "1/1",
];

const P_SETUP: u32 = 0;
const P_INPUT: u32 = 1;
const P_GATE: u32 = 2;
const P_TIGHT: u32 = 3;
const P_BOOST: u32 = 4;
const P_GAIN: u32 = 5;
const P_BASS: u32 = 6;
const P_MID: u32 = 7;
const P_TREBLE: u32 = 8;
const P_PRESENCE: u32 = 9;
const P_DEPTH: u32 = 10;
const P_SAG: u32 = 11;
const P_MASTER: u32 = 12;
const P_LEVEL: u32 = 13;
const P_CAB: u32 = 14;
const P_MIC: u32 = 15;
const P_CHORUS_ON: u32 = 16;
const P_CHORUS_MIX: u32 = 17;
const P_CHORUS_RATE: u32 = 18;
const P_CHORUS_DEPTH: u32 = 19;
const P_DELAY_ON: u32 = 20;
const P_DELAY_TIME: u32 = 21;
const P_DELAY_SYNC: u32 = 22;
const P_DELAY_FB: u32 = 23;
const P_DELAY_TONE: u32 = 24;
const P_DELAY_MIX: u32 = 25;
const P_REVERB_ON: u32 = 26;
const P_REVERB_SIZE: u32 = 27;
const P_REVERB_DECAY: u32 = 28;
const P_REVERB_MIX: u32 = 29;
/// Number of params including `setup` and `bypass`.
const N_PARAMS: usize = 31;

/// Gate threshold at or below which the gate is off.
const GATE_OFF_DB: f32 = -79.9;
/// Longest delay in ms.
const MAX_DELAY_MS: f32 = 1_500.0;
/// Time in ms in which a level or a mix follows its knob.
const SMOOTH_MS: f32 = 15.0;
/// Bound of the output.
const OUT_LIMIT: f32 = 16.0;

// A descriptor is nine fields; the helpers below shorten the table.
#[allow(clippy::too_many_arguments)]
const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    taper: Taper,
    unit: Unit,
    discrete: bool,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min,
        max,
        default,
        taper,
        unit,
        discrete,
    }
}

const fn knob(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: Unit,
) -> ParamDescriptor {
    param(id, key, name, min, max, default, Taper::Linear, unit, false)
}

const fn toggle(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    param(
        id,
        key,
        name,
        0.0,
        1.0,
        0.0,
        Taper::Linear,
        Unit::Toggle,
        true,
    )
}

static PARAMS: [ParamDescriptor; N_PARAMS] = [
    param(
        P_SETUP,
        "setup",
        "Setup",
        0.0,
        4.0,
        0.0,
        Taper::Linear,
        Unit::Choice(&setups::SETUP_NAMES),
        true,
    ),
    knob(P_INPUT, "input", "Input", -24.0, 24.0, 0.0, Unit::Db),
    knob(P_GATE, "gate", "Gate", -80.0, -20.0, -80.0, Unit::Db),
    param(
        P_TIGHT,
        "tight",
        "Tight",
        20.0,
        500.0,
        20.0,
        Taper::Exponential,
        Unit::Hz,
        false,
    ),
    knob(P_BOOST, "boost", "Boost", 0.0, 18.0, 0.0, Unit::Db),
    knob(P_GAIN, "gain", "Gain", 0.0, 10.0, 2.5, Unit::None),
    knob(P_BASS, "bass", "Bass", 0.0, 10.0, 5.0, Unit::None),
    knob(P_MID, "mid", "Mid", 0.0, 10.0, 5.5, Unit::None),
    knob(P_TREBLE, "treble", "Treble", 0.0, 10.0, 5.5, Unit::None),
    knob(
        P_PRESENCE,
        "presence",
        "Presence",
        0.0,
        10.0,
        4.0,
        Unit::None,
    ),
    knob(P_DEPTH, "depth", "Depth", 0.0, 10.0, 3.0, Unit::None),
    knob(P_SAG, "sag", "Sag", 0.0, 10.0, 3.0, Unit::None),
    knob(P_MASTER, "master", "Master", 0.0, 10.0, 6.0, Unit::None),
    knob(P_LEVEL, "level", "Level", -24.0, 12.0, -3.0, Unit::Db),
    param(
        P_CAB,
        "cab",
        "Cabinet",
        0.0,
        3.0,
        1.0,
        Taper::Linear,
        Unit::Choice(&CAB_NAMES),
        true,
    ),
    knob(P_MIC, "mic", "Mic position", 0.0, 1.0, 0.35, Unit::Percent),
    toggle(P_CHORUS_ON, "chorus.on", "Chorus on"),
    knob(
        P_CHORUS_MIX,
        "chorus.mix",
        "Chorus mix",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    param(
        P_CHORUS_RATE,
        "chorus.rate",
        "Chorus rate",
        0.1,
        6.0,
        0.8,
        Taper::Exponential,
        Unit::Hz,
        false,
    ),
    knob(
        P_CHORUS_DEPTH,
        "chorus.depth",
        "Chorus depth",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    toggle(P_DELAY_ON, "delay.on", "Delay on"),
    param(
        P_DELAY_TIME,
        "delay.time",
        "Delay time",
        20.0,
        1_000.0,
        380.0,
        Taper::Exponential,
        Unit::Ms,
        false,
    ),
    param(
        P_DELAY_SYNC,
        "delay.sync",
        "Delay sync",
        0.0,
        12.0,
        0.0,
        Taper::Linear,
        Unit::Choice(&SYNC_NAMES),
        true,
    ),
    knob(
        P_DELAY_FB,
        "delay.feedback",
        "Delay feedback",
        0.0,
        0.9,
        0.3,
        Unit::Percent,
    ),
    param(
        P_DELAY_TONE,
        "delay.tone",
        "Delay tone",
        500.0,
        12_000.0,
        5_000.0,
        Taper::Exponential,
        Unit::Hz,
        false,
    ),
    knob(
        P_DELAY_MIX,
        "delay.mix",
        "Delay mix",
        0.0,
        1.0,
        0.25,
        Unit::Percent,
    ),
    toggle(P_REVERB_ON, "reverb.on", "Reverb on"),
    knob(
        P_REVERB_SIZE,
        "reverb.size",
        "Reverb size",
        0.0,
        1.0,
        0.4,
        Unit::Percent,
    ),
    param(
        P_REVERB_DECAY,
        "reverb.decay",
        "Reverb decay",
        0.3,
        8.0,
        1.2,
        Taper::Exponential,
        Unit::None,
        false,
    ),
    knob(
        P_REVERB_MIX,
        "reverb.mix",
        "Reverb mix",
        0.0,
        1.0,
        0.2,
        Unit::Percent,
    ),
    bypass_param(30),
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

/// Static description of the guitar amp.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.guitar-amp",
    name: "Guitar Amp",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`GuitarAmp`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(GuitarAmp::new())),
    }
}

/// The mix and the on switch of an effect section: a ramp to `on * mix`, and whether the section
/// has gone quiet (its mix reached zero) and holds no state.
#[derive(Debug, Clone)]
struct Section {
    mix: BlockSmoother,
    idle: bool,
}

impl Section {
    fn new() -> Self {
        Self {
            mix: BlockSmoother::new(0.0),
            idle: true,
        }
    }
}

/// The guitar amp.
pub struct GuitarAmp {
    sample_rate: f32,
    vals: [f32; N_PARAMS],
    dirty: bool,
    fresh: bool,
    input: InputStage,
    gate: Gate,
    gate_on: bool,
    gate_gr: f32,
    os: Oversampler,
    preamp: Preamp,
    power: PowerAmp,
    cab: Cabinet,
    chorus: Chorus,
    delay: [DelayLine; 2],
    delay_lp: [f32; 2],
    delay_cur: f32,
    room: Room,
    in_gain: BlockSmoother,
    out_gain: BlockSmoother,
    chorus_sec: Section,
    delay_sec: Section,
    reverb_sec: Section,
    meters: [f32; 3],
}

impl std::fmt::Debug for GuitarAmp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuitarAmp")
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

impl Default for GuitarAmp {
    fn default() -> Self {
        Self::new()
    }
}

fn db(v: f32) -> f32 {
    exp2_det(v * 0.166_096_4)
}

impl GuitarAmp {
    /// A guitar amp with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let mut vals = [0.0; N_PARAMS];
        for (v, p) in vals.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        let mut a = Self::build(48_000.0, 0);
        a.vals = vals;
        a.reset();
        a
    }

    fn build(sample_rate: f32, seed: u64) -> Self {
        let hi = sample_rate * 4.0;
        let max_delay = (MAX_DELAY_MS * 0.001 * sample_rate).ceil() as usize + 2;
        let mut a = Self {
            sample_rate,
            vals: [0.0; N_PARAMS],
            dirty: true,
            fresh: true,
            input: InputStage::new(sample_rate),
            gate: Gate::new(sample_rate),
            gate_on: false,
            gate_gr: 0.0,
            os: Oversampler::new(BATCH),
            preamp: Preamp::new(hi),
            power: PowerAmp::new(hi),
            cab: Cabinet::new(sample_rate),
            chorus: Chorus::new(sample_rate, 40.0),
            delay: [DelayLine::new(max_delay), DelayLine::new(max_delay)],
            delay_lp: [0.0; 2],
            delay_cur: 0.0,
            room: Room::new(sample_rate, seed, RoomRange::Large),
            in_gain: BlockSmoother::new(1.0),
            out_gain: BlockSmoother::new(1.0),
            chorus_sec: Section::new(),
            delay_sec: Section::new(),
            reverb_sec: Section::new(),
            meters: [0.0; 3],
        };
        a.os.set_factor(OversampleFactor::X4);
        a.gate.set_range_db(50.0);
        a.gate.set_attack_ms(0.5);
        a.gate.set_hold_ms(30.0);
        a.gate.set_release_ms(80.0);
        for sm in [
            &mut a.in_gain,
            &mut a.out_gain,
            &mut a.chorus_sec.mix,
            &mut a.delay_sec.mix,
            &mut a.reverb_sec.mix,
        ] {
            sm.set_time(SMOOTH_MS, sample_rate, BATCH);
        }
        a.chorus.set_mix(1.0);
        a.chorus.set_center_ms(10.0);
        a.room.set_damping(0.35);
        a
    }

    fn v(&self, id: u32) -> f32 {
        self.vals[id as usize]
    }

    /// Pushes the params into the blocks (cheap when nothing changed).
    fn apply(&mut self) {
        self.dirty = false;
        let v = |id| self.vals[id as usize];
        let (gate, tight, boost) = (v(P_GATE), v(P_TIGHT), v(P_BOOST));
        let (gain, bass, mid, treble) = (v(P_GAIN), v(P_BASS), v(P_MID), v(P_TREBLE));
        let (presence, depth, sag, master) = (v(P_PRESENCE), v(P_DEPTH), v(P_SAG), v(P_MASTER));
        let (cab, mic) = (v(P_CAB), v(P_MIC));
        let (c_rate, c_depth) = (v(P_CHORUS_RATE), v(P_CHORUS_DEPTH));
        let (r_size, r_decay) = (v(P_REVERB_SIZE), v(P_REVERB_DECAY));
        self.gate_on = gate > GATE_OFF_DB;
        self.gate.set_threshold_db(gate);
        self.input.set_tight_hz(tight);
        self.input.set_boost_db(boost);
        self.preamp.set_gain(gain / 10.0);
        self.preamp.set_tone(bass / 10.0, mid / 10.0, treble / 10.0);
        self.power.set_drive_db(-24.0 + 3.6 * master);
        self.power.set_sag(sag / 10.0);
        self.power.set_voice(presence / 10.0, depth / 10.0);
        self.cab.set_type(CabType::from_index(cab.round() as usize));
        self.cab.set_mic(mic);
        self.chorus.set_rate_hz(c_rate);
        self.chorus.set_depth_ms(0.4 + 3.6 * c_depth);
        self.chorus.set_feedback(0.0);
        self.room.set_size(r_size);
        self.room.set_rt60_s(r_decay);
    }

    fn clear_delay(&mut self) {
        for d in &mut self.delay {
            d.clear();
        }
        self.delay_lp = [0.0; 2];
    }
}

impl Device for GuitarAmp {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let vals = self.vals;
        *self = Self::build(info.sample_rate.hz() as f32, info.seed);
        self.vals = vals;
        self.reset();
    }

    fn reset(&mut self) {
        // The blocks jump to the current params (no glide from their defaults at the start).
        self.apply();
        self.input.reset();
        self.gate.reset();
        self.gate_gr = 0.0;
        self.os.reset();
        self.preamp.reset();
        self.power.reset();
        self.cab.reset();
        self.chorus.reset();
        self.clear_delay();
        self.room.reset();
        self.delay_cur = 0.0;
        for s in [
            &mut self.chorus_sec,
            &mut self.delay_sec,
            &mut self.reverb_sec,
        ] {
            s.idle = true;
        }
        self.meters = [0.0; 3];
        self.fresh = true;
        self.dirty = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let Some(p) = PARAMS.get(id.0 as usize) else {
            return;
        };
        if id.0 == P_SETUP || id.0 as usize == N_PARAMS - 1 {
            // `setup` is a label (the app applies a setup as a batch); bypass is the wrapper's.
            if id.0 == P_SETUP {
                self.vals[0] = if plain.is_finite() {
                    plain.clamp(p.min, p.max).round()
                } else {
                    0.0
                };
            }
            return;
        }
        let v = if plain.is_finite() { plain } else { p.default };
        let v = v.clamp(p.min, p.max);
        self.vals[id.0 as usize] = if p.discrete { v.round() } else { v };
        self.dirty = true;
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.dirty {
            self.apply();
        }
        let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
        let (l, r) = (*ctx.input(IN_L), *ctx.input(IN_R));
        let (cl, cr) = (ctx.is_connected(IN_L), ctx.is_connected(IN_R));
        let sr = self.sample_rate;

        let in_t = db(self.v(P_INPUT));
        let out_t = db(self.v(P_LEVEL));
        let on = |id| self.v(id) >= 0.5;
        let mix_t = [
            if on(P_CHORUS_ON) {
                self.v(P_CHORUS_MIX)
            } else {
                0.0
            },
            if on(P_DELAY_ON) {
                self.v(P_DELAY_MIX)
            } else {
                0.0
            },
            if on(P_REVERB_ON) {
                self.v(P_REVERB_MIX)
            } else {
                0.0
            },
        ];
        if self.fresh {
            self.in_gain.set_immediate(in_t);
            self.out_gain.set_immediate(out_t);
            self.chorus_sec.mix.set_immediate(mix_t[0]);
            self.delay_sec.mix.set_immediate(mix_t[1]);
            self.reverb_sec.mix.set_immediate(mix_t[2]);
            self.fresh = false;
        }
        let in_g = self.in_gain.advance(in_t, BATCH);
        let out_g = self.out_gain.advance(out_t, BATCH);
        let c_mix = self.chorus_sec.mix.advance(mix_t[0], BATCH);
        let d_mix = self.delay_sec.mix.advance(mix_t[1], BATCH);
        let r_mix = self.reverb_sec.mix.advance(mix_t[2], BATCH);
        let c_run = !(self.chorus_sec.mix.value() == 0.0 && mix_t[0] == 0.0);
        let d_run = !(self.delay_sec.mix.value() == 0.0 && mix_t[1] == 0.0);
        let r_run = !(self.reverb_sec.mix.value() == 0.0 && mix_t[2] == 0.0);
        // A section that has just gone quiet starts from silence next time it runs.
        if !c_run && !self.chorus_sec.idle {
            self.chorus.reset();
            self.chorus_sec.idle = true;
        }
        if !d_run && !self.delay_sec.idle {
            self.clear_delay();
            self.delay_sec.idle = true;
        }
        if !r_run && !self.reverb_sec.idle {
            self.room.reset();
            self.reverb_sec.idle = true;
        }
        self.chorus_sec.idle &= !c_run;
        self.delay_sec.idle &= !d_run;
        self.reverb_sec.idle &= !r_run;

        // The delay time in samples, from ms or from a note value at the project tempo.
        let sync = self.v(P_DELAY_SYNC) as usize;
        let ms = if sync > 0 {
            (SYNC_BEATS[(sync - 1).min(SYNC_BEATS.len() - 1)] * 60_000.0 / ctx.time.tempo.bpm())
                as f32
        } else {
            self.v(P_DELAY_TIME)
        };
        let d_target = (ms.clamp(1.0, MAX_DELAY_MS) * 0.001 * sr).max(4.0);
        if self.delay_cur == 0.0 {
            self.delay_cur = d_target;
        }
        let d_fb = self.v(P_DELAY_FB);
        // One-pole low-pass of the repeats: `a = 1 - exp(-2 pi fc / sr)`.
        let a_lp = 1.0
            - exp2_det(
                -std::f32::consts::TAU * self.v(P_DELAY_TONE) / sr * std::f32::consts::LOG2_E,
            );

        // Stage 1: mono, gate, tight and boost (base rate).
        let mut pre = [0.0f32; BATCH];
        let mut pk_in = 0.0f32;
        let mut gr = 0.0f32;
        for n in 0..BATCH {
            let (a, b) = (finite(l[n]), finite(r[n]));
            let m = if cl != cr {
                if cl { a } else { b }
            } else {
                0.5 * (a + b)
            };
            pk_in = pk_in.max(m.abs());
            let mut x = m * in_g.at(n);
            if self.gate_on {
                x *= self.gate.process(x, x);
                gr = gr.max(self.gate.gain_reduction_db());
            }
            pre[n] = self.input.process(x);
        }
        self.gate_gr = gr;

        // Stage 2: preamp and power amp at four times the rate.
        let up = self.os.upsample(&pre);
        for v in up.iter_mut() {
            *v = self.power.process(self.preamp.process(*v));
        }
        let mut amp = [0.0f32; BATCH];
        self.os.downsample(&mut amp);

        // Stage 3: cabinet, effects, level.
        let mut out_l = [0.0f32; BATCH];
        let mut out_r = [0.0f32; BATCH];
        let mut pk_out = 0.0f32;
        for n in 0..BATCH {
            let c = self.cab.process(finite(amp[n]));
            let (mut yl, mut yr) = (c, c);
            if c_run {
                let (wl, wr) = self.chorus.process(c, c);
                let m = c_mix.at(n);
                yl = c * (1.0 - m) + wl * m;
                yr = c * (1.0 - m) + wr * m;
            }
            if d_run {
                let step = self.delay_cur - d_target;
                self.delay_cur -= step.clamp(-0.25, 0.25);
                let (wl, wr) = (
                    self.delay[0].read(self.delay_cur),
                    self.delay[1].read(self.delay_cur),
                );
                self.delay_lp[0] += a_lp * (wl - self.delay_lp[0]);
                self.delay_lp[1] += a_lp * (wr - self.delay_lp[1]);
                let (fl, fr) = (
                    flush_denormal(self.delay_lp[0]),
                    flush_denormal(self.delay_lp[1]),
                );
                self.delay_lp = [fl, fr];
                // Ping-pong: the mono sum goes into the left line, the repeats cross.
                self.delay[0].write(0.5 * (yl + yr) + d_fb * fr);
                self.delay[1].write(d_fb * fl);
                let m = d_mix.at(n);
                yl += m * wl;
                yr += m * wr;
            }
            if r_run {
                let (wl, wr) = self.room.process(yl, yr);
                let m = r_mix.at(n);
                yl += m * wl;
                yr += m * wr;
            }
            let g = out_g.at(n);
            let (yl, yr) = (
                (yl * g).clamp(-OUT_LIMIT, OUT_LIMIT),
                (yr * g).clamp(-OUT_LIMIT, OUT_LIMIT),
            );
            out_l[n] = yl;
            out_r[n] = yr;
            pk_out = pk_out.max(yl.abs()).max(yr.abs());
        }
        self.meters = [pk_in, pk_out, self.gate_gr];
        *ctx.output(OUT_L) = out_l;
        *ctx.output(OUT_R) = out_r;
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = self.meters.len().min(out.len());
        out[..n].copy_from_slice(&self.meters[..n]);
        n
    }

    fn latency_frames(&self) -> u32 {
        LATENCY
    }
}
