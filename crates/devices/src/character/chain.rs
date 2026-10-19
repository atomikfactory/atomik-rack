//! The preset chain contract of `atomik.character`: the
//! [`PresetChain`] trait, the control block ([`SUB`] frames) and its buses, the latency class
//! constants, the delay helpers and the pass-through [`StubChain`] (a stand-in for tests).
//!
//! # Latency classes (user decision "A, tiered")
//!
//! Every preset without the spectral stage is in the **short class**, whose latency is
//! [`short_latency`]: the oversampler's 29 frames plus [`wobble_centre`], the fixed centre delay
//! of the wow and flutter block (a modulated delay cannot sound before its centre; the budget is
//! the same for every short preset so that a switch inside the class never changes the reported
//! latency). A chain without an oversampled region or without a wobble is padded up to it. A
//! chain with the spectral stage is a long class chain and reports its own larger latency.

use super::voicing::{ChainSpec, Morph, ParamSpec, PresetVoicing, StageSpec, Val};
use atomik_dsp::exp2_det;
use atomik_dsp::oversample::LATENCY_2X;

/// Frames per control block: the device runs its 64-frame engine batch as four blocks.
pub const SUB: usize = 16;

/// A stereo block: `[0]` left, `[1]` right.
pub type Bus = [[f32; SUB]; 2];

/// Presets `0..IMPLEMENTED_PRESETS` are voiced: all 16.
pub const IMPLEMENTED_PRESETS: usize = 16;

/// Largest wobble depth in ms of every chain (sets the wobble centre, see the module docs).
pub const WOBBLE_MAX_MS: f32 = 0.3;

/// The wobble centre delay in frames at `sample_rate` (equals `Wobble::center_delay_samples`).
pub fn wobble_centre(sample_rate: f32) -> u32 {
    (f64::from(WOBBLE_MAX_MS) * 0.001 * f64::from(sample_rate)).ceil() as u32 + 2
}

/// Latency of the short class in frames at `sample_rate`.
pub fn short_latency(sample_rate: f32) -> u32 {
    LATENCY_2X + wobble_centre(sample_rate)
}

/// The spectral frame in samples at 44.1 and 48 kHz (a data constant; 512 is allowed). The frame
/// doubles for every doubling of the rate so that it spans the same time (see
/// [`atomik_dsp::frame_for_rate`]).
pub const SPECTRAL_FRAME: usize = 1024;

/// The latency of the spectral stage in frames at `sample_rate`: the frame length.
pub fn spectral_latency(sample_rate: f32) -> u32 {
    atomik_dsp::frame_for_rate(sample_rate, SPECTRAL_FRAME) as u32
}

/// Latency of the long class (a preset with the spectral stage) in frames: the short class
/// latency plus the spectral frame.
pub fn long_latency(sample_rate: f32) -> u32 {
    short_latency(sample_rate) + spectral_latency(sample_rate)
}

/// Per-sample side information of a control block.
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    /// Programme-presence gain of the device's gate (1 open, 0 closed) for each frame.
    pub presence: &'a [f32; SUB],
    /// The smoothed input gain for each frame (what the noise follows when the table says so).
    pub gain: &'a [f32; SUB],
}

/// How the device feeds and reads a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Routing {
    /// Two channels.
    Stereo,
    /// The chain input and output are the mono sum.
    MonoSum,
    /// Two independent mono chains (treated like stereo by the device).
    DualMono,
}

/// The universal `chain` stage every preset declares.
pub const CHAIN_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "target_gain_db",
        min: -12.0,
        max: 12.0,
    },
    ParamSpec {
        name: "noise_follows_gain",
        min: 0.0,
        max: 1.0,
    },
    ParamSpec {
        name: "presence_hold_s",
        min: 0.0,
        max: 60.0,
    },
    ParamSpec {
        name: "presence_fade_s",
        min: 0.001,
        max: 60.0,
    },
];

/// The spec of the universal `chain` stage.
pub fn chain_stage_spec() -> StageSpec {
    StageSpec {
        name: "chain",
        params: CHAIN_PARAMS,
    }
}

/// The values of the universal `chain` stage of one preset.
#[derive(Debug, Clone, Copy)]
pub struct ChainCommon {
    /// Output trim that level-matches the preset, in dB.
    pub target_gain_db: Val,
    /// 1 when generated noise follows the input gain.
    pub noise_follows_gain: Val,
    /// Presence gate hold in seconds.
    pub presence_hold_s: Val,
    /// Presence gate fade in seconds.
    pub presence_fade_s: Val,
}

impl ChainCommon {
    /// Reads the `chain` rows (constants when the preset has none).
    pub fn read(v: &PresetVoicing) -> Self {
        let get = |p: &str, default: f32| {
            let val = v.val("chain", p);
            if v.has_rows() {
                val
            } else {
                Val::constant(default)
            }
        };
        Self {
            target_gain_db: get("target_gain_db", 0.0),
            noise_follows_gain: get("noise_follows_gain", 0.0),
            presence_hold_s: get("presence_hold_s", 3.0),
            presence_fade_s: get("presence_fade_s", 2.0),
        }
    }

    /// The static output trim as a linear gain.
    pub fn trim(&self) -> f32 {
        exp2_det(self.target_gain_db.base() * 0.166_096_4)
    }

    /// The presence gate times `(hold, fade)` in seconds.
    pub fn presence(&self) -> (f32, f32) {
        (self.presence_hold_s.base(), self.presence_fade_s.base())
    }
}

/// The slow envelope of the wet signal that animates the screen: a peak follower with instant
/// attack and a 150 ms fall.
#[derive(Debug, Clone, Copy)]
pub struct Activity {
    env: f32,
    decay: f32,
}

impl Activity {
    /// A silent follower at `sample_rate`.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            env: 0.0,
            decay: 1.0 - 1.0 / (0.15 * sample_rate),
        }
    }

    /// Feeds one stereo output sample.
    #[inline]
    pub fn update(&mut self, a: f32, b: f32) {
        let pk = a.abs().max(b.abs());
        self.env = pk.max(self.env * self.decay).min(8.0);
        if self.env < 1e-12 {
            self.env = 0.0;
        }
    }

    /// The current value (about 0..1).
    pub fn value(&self) -> f32 {
        self.env
    }

    /// Clears the follower.
    pub fn reset(&mut self) {
        self.env = 0.0;
    }
}

/// A mono integer delay (a ring of exactly `d` samples; `d == 0` passes through).
#[derive(Debug, Clone)]
pub struct IntDelay {
    buf: Vec<f32>,
    pos: usize,
}

impl IntDelay {
    /// A delay of `d` samples (allocates; non-real-time).
    pub fn new(d: usize) -> Self {
        Self {
            buf: vec![0.0; d],
            pos: 0,
        }
    }

    /// The delay in samples.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// True for a zero delay.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Clears the contents.
    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }

    /// Writes `x` and returns the sample written `len` calls ago.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        if self.buf.is_empty() {
            return x;
        }
        let y = self.buf[self.pos];
        self.buf[self.pos] = x;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        y
    }
}

/// A stereo integer delay.
#[derive(Debug, Clone)]
pub struct StereoDelay {
    l: IntDelay,
    r: IntDelay,
}

impl StereoDelay {
    /// A delay of `d` samples per channel.
    pub fn new(d: usize) -> Self {
        Self {
            l: IntDelay::new(d),
            r: IntDelay::new(d),
        }
    }

    /// The delay in samples.
    pub fn len(&self) -> usize {
        self.l.len()
    }

    /// True for a zero delay.
    pub fn is_empty(&self) -> bool {
        self.l.is_empty()
    }

    /// Clears both channels.
    pub fn reset(&mut self) {
        self.l.reset();
        self.r.reset();
    }

    /// Delays one stereo sample.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        (self.l.process(l), self.r.process(r))
    }
}

/// One preset's processing chain. All 16 are built in `prepare` (allocation allowed there); only
/// the active one, plus the outgoing one during a crossfade, runs.
pub trait PresetChain: Send {
    /// The stages and params the chain takes from the voicing tables.
    fn spec(&self) -> ChainSpec;

    /// Checks the chain against its voicing (the rack compiles its route table here) at
    /// `sample_rate`; non-real-time.
    fn check(&self, _v: &PresetVoicing, _sample_rate: f32) -> Result<(), String> {
        Ok(())
    }

    /// Builds all state for `sample_rate` (allocates).
    fn prepare(&mut self, sample_rate: f32, seed: u64, v: &PresetVoicing);

    /// Clears audio state (parameters stay).
    fn reset(&mut self);

    /// Processes one control block in place with the resolved morph values.
    fn process(&mut self, l: &mut [f32; SUB], r: &mut [f32; SUB], m: &Morph, ctx: &Ctx);

    /// The declared latency in frames.
    fn latency(&self) -> u32;

    /// How the device feeds and reads the chain.
    fn routing(&self) -> Routing;

    /// A slow envelope of the wet signal for the screen (about 0..1).
    fn activity(&self) -> f32;

    /// Presence gate `(hold, fade)` in seconds for this preset's generated noise.
    fn presence(&self) -> (f32, f32);
}

/// A pass-through chain that declares the short class latency (never panics). Every preset is
/// voiced; the stub stays as a stand-in for the shell tests.
#[derive(Debug, Clone, Default)]
pub struct StubChain {
    pad: Option<StereoDelay>,
    latency: u32,
}

impl StubChain {
    /// A placeholder chain.
    pub fn new() -> Self {
        Self::default()
    }
}

impl PresetChain for StubChain {
    fn spec(&self) -> ChainSpec {
        ChainSpec {
            stages: vec![chain_stage_spec()],
            implemented: false,
        }
    }

    fn prepare(&mut self, sample_rate: f32, _seed: u64, _v: &PresetVoicing) {
        self.latency = short_latency(sample_rate);
        self.pad = Some(StereoDelay::new(self.latency as usize));
    }

    fn reset(&mut self) {
        if let Some(p) = self.pad.as_mut() {
            p.reset();
        }
    }

    fn process(&mut self, l: &mut [f32; SUB], r: &mut [f32; SUB], _m: &Morph, _ctx: &Ctx) {
        if let Some(p) = self.pad.as_mut() {
            for n in 0..SUB {
                let (a, b) = p.process(l[n], r[n]);
                l[n] = a;
                r[n] = b;
            }
        }
    }

    fn latency(&self) -> u32 {
        self.latency
    }

    fn routing(&self) -> Routing {
        Routing::Stereo
    }

    fn activity(&self) -> f32 {
        0.0
    }

    fn presence(&self) -> (f32, f32) {
        (3.0, 2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_dsp::Wobble;

    #[test]
    fn the_wobble_centre_formula_matches_the_dsp_block() {
        for sr in [44_100.0f32, 48_000.0, 88_200.0, 96_000.0, 192_000.0] {
            let w = Wobble::new(sr, 0, WOBBLE_MAX_MS);
            assert_eq!(wobble_centre(sr) as f32, w.center_delay_samples(), "{sr}");
        }
    }

    #[test]
    fn short_latency_is_29_plus_the_wobble_centre() {
        assert_eq!(short_latency(48_000.0), 29 + wobble_centre(48_000.0));
        assert!(short_latency(44_100.0) < short_latency(96_000.0));
    }

    #[test]
    fn int_delay_delays_by_exactly_its_length() {
        for d in [0usize, 1, 5, 29] {
            let mut z = IntDelay::new(d);
            let out: Vec<f32> = (0..40)
                .map(|n| z.process(if n == 0 { 1.0 } else { 0.0 }))
                .collect();
            let at = out.iter().position(|v| *v > 0.5).unwrap();
            assert_eq!(at, d);
        }
    }

    #[test]
    fn the_stub_passes_the_input_through_after_the_class_latency() {
        let mut s = StubChain::new();
        let v = PresetVoicing::default();
        s.prepare(48_000.0, 0, &v);
        assert_eq!(s.latency(), short_latency(48_000.0));
        let presence = [1.0; SUB];
        let gain = [1.0; SUB];
        let ctx = Ctx {
            presence: &presence,
            gain: &gain,
        };
        let mut out = Vec::new();
        for blk in 0..8 {
            let mut l = [0.0f32; SUB];
            let mut r = [0.0f32; SUB];
            if blk == 0 {
                l[0] = 1.0;
                r[1] = 1.0;
            }
            s.process(&mut l, &mut r, &Morph::default(), &ctx);
            out.push((l, r));
        }
        let lat = s.latency() as usize;
        assert!(out[lat / SUB].0[lat % SUB] > 0.99);
        assert!(out[(lat + 1) / SUB].1[(lat + 1) % SUB] > 0.99);
    }
}
