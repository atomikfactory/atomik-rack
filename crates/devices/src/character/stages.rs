//! The stage kinds of the generic rack as thin wrappers over `atomik-dsp`
//! blocks. Every stage is stereo, allocation-free after construction and works on buses of
//! [`SUB`] frames; its parameters arrive resolved (already morphed and clamped) in the order of
//! [`Kind::params`] once per control block, and the audible ones are smoothed inside the stage
//! (second-order smoothers).
//!
//! | Kind | Params | Notes |
//! |---|---|---|
//! | `Peak`, `LowShelf`, `HighShelf` | `freq_hz`, (`q`), `gain_db` | smoothed biquads, unity away from the centre |
//! | `HighPass`, `LowPass` | `freq_hz`, `q` | 12 dB/oct, resonant |
//! | `Sat(kind)` | `drive_db`, `makeup_db` | **the oversampled region**: 2x halfband round trip (latency 29), bypassed at 88.2 kHz and above behind a 29-frame pad so the latency never changes |
//! | `Comp` | `threshold_db`, `ratio`, `attack_ms`, `release_ms`, `makeup_db` | stereo-linked downward compressor |
//! | `Leveller` | `ratio`, `ceiling_db`, `attack_ms`, `release_ms`, `makeup_db` | the upward leveller (AGC) |
//! | `Wobble` | `depth_ms`, `wow_hz`, `flutter_hz` | latency = the wobble centre (data constant `WOBBLE_MAX_MS`) |
//! | `Hiss`, `Rumble`, `Crackle`, `Clicks` | levels and rates | **generators**: no audio input, output gated by the programme-presence gain |
//! | `Width` | `side_gain` | mid/side width |
//! | `MonoLow` | `freq_hz` | the band below the corner summed to mono, the rest stereo |
//! | `Mono` | none | mono sum |
//! | `Split` | `freq_hz` | two outputs: send 1 low band, send 2 high band |
//! | `Spectral` | `thresh_db`, `hold_ms`, `step_db`, `dropout` | the STFT stage with the combined lossy algorithm; **long latency class** (latency = the frame, [`spectral_latency`]) |
//! | `Phaser` | `rate_hz`, `depth_oct`, `feedback`, `center_hz`, `mix`, `stereo` | six all-pass poles |
//! | `AutoPan` | `rate_hz`, `depth` | constant-power rotating balance |
//! | `Echo` | `time_ms`, `feedback`, `mix`, `mod_rate_hz`, `mod_depth_ms` | gliding feedback echo |
//! | `RingMod` | `freq_hz`, `amount` | base-rate ring modulator |
//! | `Comb` | `delay_ms`, `feedback`, `feedforward`, `detune`, `wander` | normalised comb per channel, the right channel detuned, seeded slow wander |
//! | `BandPass` | `freq_hz`, `q`, `makeup_db` | state-variable band-pass with unity peak gain |
//! | `Crush` | `bits`, `rate_hz`, `mod_rate_hz`, `mod_depth`, `makeup_db` | base-rate bit and rate reduction with seeded stepped random modulation per channel |
//! | `Tremolo` | `rate_hz`, `depth`, `phase` | drum tremolo, level-compensated, right channel phase-offset |
//! | `SweepLp` | `lo_hz`, `hi_hz`, `sweep_hz`, `depth`, `q` | low-pass whose cutoff sweeps between the corners |
//! | `Hum`, `Bursts` | levels, rates | **generators** (presence gated): motor hum with harmonics, Poisson noise bursts |
//! | `Rattle` | `amount`, `tone_hz` | noise keyed by the input envelope, added to the signal |
//! | `Modal` | `root_hz`, `decay_s`, `detune`, `active`, `gain` | twelve inharmonic resonators excited by the mono sum |
//! | `Room` | `size`, `rt60_s`, `damping` | small resonant box (wet only) |
// Fixed-size sample buses are indexed by the sample position next to parallel arrays; iterator
// forms would obscure the per-sample DSP loops.
#![allow(clippy::needless_range_loop)]

use super::chain::{
    Bus, Ctx, SPECTRAL_FRAME, SUB, StereoDelay, WOBBLE_MAX_MS, spectral_latency, wobble_centre,
};
use super::voicing::ParamSpec;
use atomik_dsp::oversample::{LATENCY_2X, OversampleFactor, Oversampler};
use atomik_dsp::rng::hash64;
use atomik_dsp::{
    BiquadShape, Bitcrush, Comb, Compressor, Crackle, Crossover, Echo, Leveller, Modal, Phaser,
    PinkNoise, RingMod, Rng, Room, RoomRange, Rumble, Shaper, ShaperKind, SmoothedBiquad,
    Smoother2, SmootherClass, Spectral, SpectralAlgo, Svf, Wander, WhiteNoise, Wobble, exp2_det,
    exp2_det_f64, frame_for_rate, log2_det,
};

/// `log2(10)/20`: dB to log2 of amplitude.
const DB_TO_LOG2: f32 = 0.166_096_4;

const fn ps(name: &'static str, min: f32, max: f32) -> ParamSpec {
    ParamSpec { name, min, max }
}

const P_PEAK: &[ParamSpec] = &[
    ps("freq_hz", 20.0, 20_000.0),
    ps("q", 0.1, 20.0),
    ps("gain_db", -24.0, 24.0),
];
const P_SHELF: &[ParamSpec] = &[ps("freq_hz", 20.0, 20_000.0), ps("gain_db", -24.0, 24.0)];
const P_PASS: &[ParamSpec] = &[ps("freq_hz", 10.0, 20_000.0), ps("q", 0.3, 8.0)];
const P_SAT: &[ParamSpec] = &[ps("drive_db", -12.0, 48.0), ps("makeup_db", -24.0, 24.0)];
const P_COMP: &[ParamSpec] = &[
    ps("threshold_db", -60.0, 0.0),
    ps("ratio", 1.0, 20.0),
    ps("attack_ms", 0.1, 200.0),
    ps("release_ms", 5.0, 2_000.0),
    ps("makeup_db", -12.0, 24.0),
];
const P_LEVELLER: &[ParamSpec] = &[
    ps("ratio", 0.0, 1.0),
    ps("ceiling_db", -30.0, 0.0),
    ps("attack_ms", 0.1, 500.0),
    ps("release_ms", 5.0, 2_000.0),
    ps("makeup_db", -24.0, 24.0),
];
const P_WOBBLE: &[ParamSpec] = &[
    ps("depth_ms", 0.0, WOBBLE_MAX_MS),
    ps("wow_hz", 0.05, 5.0),
    ps("flutter_hz", 1.0, 30.0),
];
const P_LEVEL: &[ParamSpec] = &[ps("level_db", -120.0, 0.0)];
const P_CRACKLE: &[ParamSpec] = &[
    ps("rate_hz", 0.0, 100.0),
    ps("amp", 0.01, 0.5),
    ps("decay_ms", 0.05, 5.0),
    ps("level_db", -60.0, 6.0),
    ps("pop_rate_hz", 0.0, 10.0),
    ps("pop_level_db", -60.0, 6.0),
];
const P_CLICKS: &[ParamSpec] = &[ps("level_db", -120.0, 0.0), ps("rate_hz", 5.0, 400.0)];
const P_WIDTH: &[ParamSpec] = &[ps("side_gain", 0.0, 3.0)];
const P_FREQ: &[ParamSpec] = &[ps("freq_hz", 40.0, 1_000.0)];
const P_NONE: &[ParamSpec] = &[];
const P_SPECTRAL: &[ParamSpec] = &[
    ps("thresh_db", -100.0, 0.0),
    ps("hold_ms", 0.0, 500.0),
    ps("step_db", 0.5, 24.0),
    ps("dropout", 0.0, 1.0),
];
const P_PHASER: &[ParamSpec] = &[
    ps("rate_hz", 0.02, 8.0),
    ps("depth_oct", 0.0, 3.0),
    ps("feedback", -0.9, 0.9),
    ps("center_hz", 100.0, 4_000.0),
    ps("mix", 0.0, 1.0),
    ps("stereo", 0.0, 1.0),
];
const P_AUTOPAN: &[ParamSpec] = &[ps("rate_hz", 0.02, 8.0), ps("depth", 0.0, 1.0)];
const P_ECHO: &[ParamSpec] = &[
    ps("time_ms", 20.0, 1_500.0),
    ps("feedback", 0.0, 0.9),
    ps("mix", 0.0, 1.0),
    ps("mod_rate_hz", 0.0, 10.0),
    ps("mod_depth_ms", 0.0, 10.0),
];
const P_RING: &[ParamSpec] = &[ps("freq_hz", 0.5, 4_000.0), ps("amount", 0.0, 1.0)];
const P_COMB: &[ParamSpec] = &[
    ps("delay_ms", 0.1, 20.0),
    ps("feedback", -0.9, 0.9),
    ps("feedforward", -1.0, 1.0),
    ps("detune", 0.0, 0.3),
    ps("wander", 0.0, 0.1),
];
const P_BANDPASS: &[ParamSpec] = &[
    ps("freq_hz", 60.0, 16_000.0),
    ps("q", 0.3, 20.0),
    ps("makeup_db", -12.0, 24.0),
];
const P_CRUSH: &[ParamSpec] = &[
    ps("bits", 1.0, 16.0),
    ps("rate_hz", 100.0, 96_000.0),
    ps("mod_rate_hz", 0.0, 30.0),
    ps("mod_depth", 0.0, 1.0),
    ps("makeup_db", -24.0, 12.0),
];
const P_TREMOLO: &[ParamSpec] = &[
    ps("rate_hz", 0.1, 12.0),
    ps("depth", 0.0, 1.0),
    ps("phase", 0.0, 0.5),
];
const P_SWEEP: &[ParamSpec] = &[
    ps("lo_hz", 100.0, 4_000.0),
    ps("hi_hz", 500.0, 12_000.0),
    ps("sweep_hz", 0.02, 4.0),
    ps("depth", 0.0, 1.0),
    ps("q", 0.3, 4.0),
];
const P_HUM: &[ParamSpec] = &[ps("level_db", -120.0, 0.0), ps("freq_hz", 30.0, 120.0)];
const P_BURSTS: &[ParamSpec] = &[
    ps("rate_hz", 0.0, 20.0),
    ps("level_db", -120.0, 0.0),
    ps("decay_ms", 20.0, 400.0),
    ps("tone_hz", 300.0, 6_000.0),
];
const P_RATTLE: &[ParamSpec] = &[ps("amount", 0.0, 1.0), ps("tone_hz", 1_000.0, 8_000.0)];
const P_MODAL: &[ParamSpec] = &[
    ps("root_hz", 40.0, 1_500.0),
    ps("decay_s", 0.05, 8.0),
    ps("detune", 0.0, 0.04),
    ps("active", 1.0, 12.0),
    ps("gain", 0.05, 64.0),
];
const P_ROOM: &[ParamSpec] = &[
    ps("size", 0.0, 1.0),
    ps("rt60_s", 0.02, 1.0),
    ps("damping", 0.0, 1.0),
];

/// A stage kind of the rack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Peaking bell.
    Peak,
    /// Low shelf.
    LowShelf,
    /// High shelf.
    HighShelf,
    /// 12 dB/oct resonant high-pass.
    HighPass,
    /// 12 dB/oct resonant low-pass.
    LowPass,
    /// Oversampled saturation of the given curve.
    Sat(ShaperKind),
    /// Downward compressor.
    Comp,
    /// Upward leveller.
    Leveller,
    /// Wow and flutter.
    Wobble,
    /// Pink hiss generator.
    Hiss,
    /// Low-frequency rumble generator.
    Rumble,
    /// Poisson crackle and pops generator.
    Crackle,
    /// Periodic click train generator.
    Clicks,
    /// Mid/side width.
    Width,
    /// Mono below a corner.
    MonoLow,
    /// Mono sum.
    Mono,
    /// Two-band split, send 1 low and send 2 high.
    Split,
    /// STFT stage with the combined lossy algorithm (long latency class).
    Spectral,
    /// Six-pole phaser.
    Phaser,
    /// Rotating constant-power auto-pan.
    AutoPan,
    /// Gliding feedback echo.
    Echo,
    /// Ring modulator.
    RingMod,
    /// Normalised comb filter (flanger and hollow resonance).
    Comb,
    /// State-variable band-pass.
    BandPass,
    /// Bit and sample-rate reduction with random modulation.
    Crush,
    /// Drum tremolo.
    Tremolo,
    /// Swept low-pass.
    SweepLp,
    /// Motor hum generator.
    Hum,
    /// Noise burst generator.
    Bursts,
    /// Noise keyed by the input envelope.
    Rattle,
    /// Inharmonic resonator bank.
    Modal,
    /// Small resonant box.
    Room,
}

impl Kind {
    /// The parameters of the kind, in the order of the resolved slice handed to the stage.
    pub fn params(self) -> &'static [ParamSpec] {
        match self {
            Self::Peak => P_PEAK,
            Self::LowShelf | Self::HighShelf => P_SHELF,
            Self::HighPass | Self::LowPass => P_PASS,
            Self::Sat(_) => P_SAT,
            Self::Comp => P_COMP,
            Self::Leveller => P_LEVELLER,
            Self::Wobble => P_WOBBLE,
            Self::Hiss | Self::Rumble => P_LEVEL,
            Self::Crackle => P_CRACKLE,
            Self::Clicks => P_CLICKS,
            Self::Width => P_WIDTH,
            Self::MonoLow | Self::Split => P_FREQ,
            Self::Mono => P_NONE,
            Self::Spectral => P_SPECTRAL,
            Self::Phaser => P_PHASER,
            Self::AutoPan => P_AUTOPAN,
            Self::Echo => P_ECHO,
            Self::RingMod => P_RING,
            Self::Comb => P_COMB,
            Self::BandPass => P_BANDPASS,
            Self::Crush => P_CRUSH,
            Self::Tremolo => P_TREMOLO,
            Self::SweepLp => P_SWEEP,
            Self::Hum => P_HUM,
            Self::Bursts => P_BURSTS,
            Self::Rattle => P_RATTLE,
            Self::Modal => P_MODAL,
            Self::Room => P_ROOM,
        }
    }

    /// True for the kinds without an audio input.
    pub fn generator(self) -> bool {
        matches!(
            self,
            Self::Hiss | Self::Rumble | Self::Crackle | Self::Clicks | Self::Hum | Self::Bursts
        )
    }

    /// The latency of the stage in frames at `sample_rate`.
    pub fn latency(self, sample_rate: f32) -> u32 {
        match self {
            Self::Sat(_) => LATENCY_2X,
            Self::Wobble => wobble_centre(sample_rate),
            Self::Spectral => spectral_latency(sample_rate),
            _ => 0,
        }
    }

    /// How far the kind lifts the chain into a longer latency class: the long class
    /// (a preset with the spectral stage) is the short class plus this many frames.
    pub fn class_extra(self, sample_rate: f32) -> u32 {
        match self {
            Self::Spectral => spectral_latency(sample_rate),
            _ => 0,
        }
    }
}

/// What the rack needs from a stage.
pub trait Stage: Send {
    /// Latency in frames.
    fn latency(&self) -> u32 {
        0
    }
    /// Clears audio state (not parameters).
    fn reset(&mut self);
    /// Receives the resolved parameters; `snap` jumps the smoothers to them.
    fn set(&mut self, p: &[f32], snap: bool);
    /// Processes one control block. Normal stages write `out1` (the rack reads it for both
    /// sends); a two-output stage also writes `out2`.
    fn process(&mut self, inp: &Bus, out1: &mut Bus, out2: &mut Bus, ctx: &Ctx);
    /// True when send 2 differs from send 1.
    fn two_outputs(&self) -> bool {
        false
    }
}

/// Everything a stage needs to be built.
#[derive(Debug, Clone, Copy)]
pub struct BuildInfo {
    /// Sample rate.
    pub sample_rate: f32,
    /// Seed of this stage (already mixed with preset and stage index).
    pub seed: u64,
    /// Whether generated noise follows the input gain.
    pub noise_follows_gain: bool,
}

/// Builds the stage of `kind`.
pub fn build(kind: Kind, info: &BuildInfo) -> Box<dyn Stage> {
    let sr = info.sample_rate;
    match kind {
        Kind::Peak | Kind::LowShelf | Kind::HighShelf | Kind::HighPass | Kind::LowPass => {
            Box::new(EqStage::new(kind, sr))
        }
        Kind::Sat(k) => Box::new(SatStage::new(k, sr)),
        Kind::Comp => Box::new(CompStage::new(sr)),
        Kind::Leveller => Box::new(LevellerStage::new(sr)),
        Kind::Wobble => Box::new(WobbleStage::new(sr, info.seed)),
        Kind::Hiss => Box::new(HissStage::new(sr, info)),
        Kind::Rumble => Box::new(RumbleStage::new(sr, info)),
        Kind::Crackle => Box::new(CrackleStage::new(sr, info)),
        Kind::Clicks => Box::new(ClicksStage::new(sr, info)),
        Kind::Width => Box::new(WidthStage::new(sr)),
        Kind::MonoLow => Box::new(MonoLowStage::new(sr)),
        Kind::Mono => Box::new(MonoStage),
        Kind::Split => Box::new(SplitStage::new(sr)),
        Kind::Spectral => Box::new(SpectralStage::new(sr, info.seed)),
        Kind::Phaser => Box::new(PhaserStage::new(sr)),
        Kind::AutoPan => Box::new(AutoPanStage::new(sr)),
        Kind::Echo => Box::new(EchoStage::new(sr)),
        Kind::RingMod => Box::new(RingStage::new(sr)),
        Kind::Comb => Box::new(CombStage::new(sr, info.seed)),
        Kind::BandPass => Box::new(BandPassStage::new(sr)),
        Kind::Crush => Box::new(CrushStage::new(sr, info.seed)),
        Kind::Tremolo => Box::new(TremoloStage::new(sr)),
        Kind::SweepLp => Box::new(SweepStage::new(sr)),
        Kind::Hum => Box::new(HumStage::new(sr, info)),
        Kind::Bursts => Box::new(BurstStage::new(sr, info)),
        Kind::Rattle => Box::new(RattleStage::new(sr, info.seed)),
        Kind::Modal => Box::new(ModalStage::new(sr, info.seed)),
        Kind::Room => Box::new(RoomStage::new(sr, info.seed)),
    }
}

#[inline]
fn db_to_lin(db: f32) -> f32 {
    exp2_det(db * DB_TO_LOG2)
}

/// Linear gain of a level in dB, exactly 0 at the bottom of the range (a silent generator).
#[inline]
fn level_to_lin(db: f32) -> f32 {
    if db <= -119.5 { 0.0 } else { db_to_lin(db) }
}

// --- EQ ---------------------------------------------------------------------------------------

struct EqStage {
    bq: SmoothedBiquad,
    kind: Kind,
}

impl EqStage {
    fn new(kind: Kind, sr: f32) -> Self {
        let shape = match kind {
            Kind::LowShelf => BiquadShape::LowShelf,
            Kind::HighShelf => BiquadShape::HighShelf,
            Kind::HighPass => BiquadShape::Highpass,
            Kind::LowPass => BiquadShape::Lowpass,
            _ => BiquadShape::Peaking,
        };
        Self {
            bq: SmoothedBiquad::new(shape, 1_000.0, 0.707, 0.0, sr),
            kind,
        }
    }
}

impl Stage for EqStage {
    fn reset(&mut self) {
        self.bq.reset();
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        let (f, q, g) = match self.kind {
            Kind::Peak => (p[0], p[1], p[2]),
            Kind::LowShelf | Kind::HighShelf => (p[0], 0.707, p[1]),
            _ => (p[0], p[1], 0.0),
        };
        self.bq.set_freq(f);
        self.bq.set_q(q);
        self.bq.set_gain_db(g);
        if snap {
            self.bq.snap();
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let (l, r) = self.bq.process(inp[0][n], inp[1][n]);
            out1[0][n] = l;
            out1[1][n] = r;
        }
    }
}

// --- Saturation (the oversampled region) -------------------------------------------------------

struct SatStage {
    ratio: usize,
    os: [Oversampler; 2],
    shaper: [Shaper; 2],
    drive: Smoother2,
    makeup: Smoother2,
    pad: Option<StereoDelay>,
    dc_r: f64,
    dc: [[f64; 2]; 2],
}

impl SatStage {
    fn new(kind: ShaperKind, sr: f32) -> Self {
        let factor = if sr >= 88_200.0 {
            OversampleFactor::X1
        } else {
            OversampleFactor::X2
        };
        let mut os = [Oversampler::new(SUB), Oversampler::new(SUB)];
        for o in &mut os {
            o.set_factor(factor);
        }
        let mut shaper = [
            Shaper::new(sr * factor.ratio() as f32),
            Shaper::new(sr * factor.ratio() as f32),
        ];
        for s in &mut shaper {
            s.set_kind(kind);
            // First-order antiderivative anti-aliasing on top of the 2x oversampling.
            s.set_adaa(true);
        }
        Self {
            ratio: factor.ratio(),
            os,
            shaper,
            drive: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            makeup: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            pad: (factor == OversampleFactor::X1).then(|| StereoDelay::new(LATENCY_2X as usize)),
            dc_r: exp2_det_f64(
                -std::f64::consts::TAU * 8.0 / f64::from(sr) * std::f64::consts::LOG2_E,
            ),
            dc: [[0.0; 2]; 2],
        }
    }
}

impl Stage for SatStage {
    fn latency(&self) -> u32 {
        LATENCY_2X
    }

    fn reset(&mut self) {
        for o in &mut self.os {
            o.reset();
        }
        for s in &mut self.shaper {
            s.reset();
        }
        if let Some(p) = self.pad.as_mut() {
            p.reset();
        }
        self.dc = [[0.0; 2]; 2];
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.drive.set_target(p[0]);
        self.makeup.set_target(p[1]);
        if snap {
            self.drive.set_immediate(p[0]);
            self.makeup.set_immediate(p[1]);
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        let mut d = [0.0f32; SUB];
        let mut mk = [0.0f32; SUB];
        for n in 0..SUB {
            d[n] = self.drive.next_value();
            mk[n] = db_to_lin(self.makeup.next_value());
        }
        for ch in 0..2 {
            let hi = self.os[ch].upsample(&inp[ch]);
            for (i, x) in hi.iter_mut().enumerate() {
                if i % self.ratio == 0 {
                    self.shaper[ch].set_drive_db(d[i / self.ratio]);
                }
                *x = self.shaper[ch].process(*x);
            }
            self.os[ch].downsample(&mut out1[ch]);
            // DC blocker (the asymmetric curves leave an offset), then the make-up gain.
            let [x1, y1] = &mut self.dc[ch];
            for n in 0..SUB {
                let x = f64::from(out1[ch][n]);
                let y = x - *x1 + self.dc_r * *y1;
                *x1 = x;
                *y1 = if y.abs() < 1e-20 { 0.0 } else { y };
                out1[ch][n] = (*y1 as f32) * mk[n];
            }
        }
        if let Some(pad) = self.pad.as_mut() {
            for n in 0..SUB {
                let (l, r) = pad.process(out1[0][n], out1[1][n]);
                out1[0][n] = l;
                out1[1][n] = r;
            }
        }
    }
}

// --- Dynamics ----------------------------------------------------------------------------------

/// Calls `f` only when the value changed (the dsp setters recompute coefficients).
fn changed(slot: &mut f32, v: f32) -> bool {
    if slot.to_bits() == v.to_bits() {
        false
    } else {
        *slot = v;
        true
    }
}

struct CompStage {
    comp: Compressor,
    makeup: Smoother2,
    last: [f32; 4],
}

impl CompStage {
    fn new(sr: f32) -> Self {
        Self {
            comp: Compressor::new(sr),
            makeup: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            last: [f32::NAN; 4],
        }
    }
}

impl Stage for CompStage {
    fn reset(&mut self) {
        self.comp.reset();
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        if changed(&mut self.last[0], p[0]) {
            self.comp.set_threshold_db(p[0]);
        }
        if changed(&mut self.last[1], p[1]) {
            self.comp.set_ratio(p[1]);
        }
        if changed(&mut self.last[2], p[2]) {
            self.comp.set_attack_ms(p[2]);
        }
        if changed(&mut self.last[3], p[3]) {
            self.comp.set_release_ms(p[3]);
        }
        self.makeup.set_target(p[4]);
        if snap {
            self.makeup.set_immediate(p[4]);
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            // The returned gain uses libm; the reduction in dB is used with the deterministic exp2.
            let _ = self.comp.process(inp[0][n], inp[1][n]);
            let g =
                exp2_det((self.makeup.next_value() - self.comp.gain_reduction_db()) * DB_TO_LOG2);
            out1[0][n] = inp[0][n] * g;
            out1[1][n] = inp[1][n] * g;
        }
    }
}

struct LevellerStage {
    lev: Leveller,
    makeup: Smoother2,
    last: [f32; 4],
}

impl LevellerStage {
    fn new(sr: f32) -> Self {
        Self {
            lev: Leveller::new(sr),
            makeup: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            last: [f32::NAN; 4],
        }
    }
}

impl Stage for LevellerStage {
    fn reset(&mut self) {
        self.lev.reset();
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        if changed(&mut self.last[0], p[0]) {
            self.lev.set_ratio(p[0]);
        }
        if changed(&mut self.last[1], p[1]) {
            self.lev.set_ceiling_db(p[1]);
        }
        if changed(&mut self.last[2], p[2]) {
            self.lev.set_attack_ms(p[2]);
        }
        if changed(&mut self.last[3], p[3]) {
            self.lev.set_release_ms(p[3]);
        }
        self.makeup.set_target(p[4]);
        if snap {
            self.makeup.set_immediate(p[4]);
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let (l, r) = self.lev.process(inp[0][n], inp[1][n]);
            let g = db_to_lin(self.makeup.next_value());
            out1[0][n] = l * g;
            out1[1][n] = r * g;
        }
    }
}

// --- Wobble ------------------------------------------------------------------------------------

struct WobbleStage {
    w: Wobble,
    depth: Smoother2,
    last: [f32; 2],
    centre: u32,
}

impl WobbleStage {
    fn new(sr: f32, seed: u64) -> Self {
        let w = Wobble::new(sr, seed, WOBBLE_MAX_MS);
        let centre = w.center_delay_samples() as u32;
        Self {
            w,
            depth: Smoother2::with_class(0.0, SmootherClass::Medium, sr),
            last: [f32::NAN; 2],
            centre,
        }
    }
}

impl Stage for WobbleStage {
    fn latency(&self) -> u32 {
        self.centre
    }

    fn reset(&mut self) {
        // A restart (not just a clear of the delay lines): the modulation phases and the random
        // drift return to their seeded start, so a render after a reset equals the first one.
        self.w.restart();
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.depth.set_target(p[0]);
        if snap {
            self.depth.set_immediate(p[0]);
        }
        if changed(&mut self.last[0], p[1]) {
            self.w.set_wow_hz(p[1]);
        }
        if changed(&mut self.last[1], p[2]) {
            self.w.set_flutter_hz(p[2]);
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            self.w.set_depth_ms(self.depth.next_value());
            let (l, r) = self.w.process(inp[0][n], inp[1][n]);
            out1[0][n] = l;
            out1[1][n] = r;
        }
    }
}

// --- Generators --------------------------------------------------------------------------------

/// The per-sample gain of a generator: its level times the presence gate (and the input gain
/// when the voicing says the noise follows it).
#[inline]
fn gen_gain(level: f32, ctx: &Ctx, follows: bool, n: usize) -> f32 {
    let g = level * ctx.presence[n];
    if follows { g * ctx.gain[n] } else { g }
}

struct HissStage {
    seed: u64,
    pink: [PinkNoise; 2],
    level: Smoother2,
    follows: bool,
}

impl HissStage {
    fn streams(seed: u64) -> [PinkNoise; 2] {
        [
            PinkNoise::new(hash64(seed, 0x48, 0)),
            PinkNoise::new(hash64(seed, 0x48, 1)),
        ]
    }

    fn new(sr: f32, info: &BuildInfo) -> Self {
        Self {
            seed: info.seed,
            pink: Self::streams(info.seed),
            level: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            follows: info.noise_follows_gain,
        }
    }
}

impl Stage for HissStage {
    fn reset(&mut self) {
        // The generators depend on the seed and the sample index only: a restart re-seeds.
        self.pink = Self::streams(self.seed);
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.level.set_target(level_to_lin(p[0]));
        if snap {
            self.level.set_immediate(level_to_lin(p[0]));
        }
    }

    fn process(&mut self, _inp: &Bus, out1: &mut Bus, _out2: &mut Bus, ctx: &Ctx) {
        for n in 0..SUB {
            let g = gen_gain(self.level.next_value(), ctx, self.follows, n);
            out1[0][n] = self.pink[0].next_sample() * g;
            out1[1][n] = self.pink[1].next_sample() * g;
        }
    }
}

struct RumbleStage {
    seed: u64,
    sr: f32,
    rumble: Rumble,
    level: Smoother2,
    follows: bool,
}

impl RumbleStage {
    fn new(sr: f32, info: &BuildInfo) -> Self {
        Self {
            seed: info.seed,
            sr,
            rumble: Rumble::new(hash64(info.seed, 0x52, 0), sr),
            level: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            follows: info.noise_follows_gain,
        }
    }
}

impl Stage for RumbleStage {
    fn reset(&mut self) {
        self.rumble = Rumble::new(hash64(self.seed, 0x52, 0), self.sr);
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.level.set_target(level_to_lin(p[0]));
        if snap {
            self.level.set_immediate(level_to_lin(p[0]));
        }
    }

    fn process(&mut self, _inp: &Bus, out1: &mut Bus, _out2: &mut Bus, ctx: &Ctx) {
        for n in 0..SUB {
            let g = gen_gain(self.level.next_value(), ctx, self.follows, n);
            let x = self.rumble.next_sample() * g;
            out1[0][n] = x;
            out1[1][n] = x;
        }
    }
}

/// Fraction of the crackle rate that both channels share (partly decorrelated crackle).
const CRACKLE_COMMON: f32 = 0.4;
/// Pop amplitude scale and decay (rare, loud, slower than the crackle).
const POP_MIN_AMP: f32 = 0.4;
const POP_DECAY_MS: f32 = 3.0;

/// Three Poisson streams (left, right, common): each channel sees the full rate.
struct Trio {
    g: [Crackle; 3],
}

impl Trio {
    fn new(seed: u64, sr: f32, tag: u64) -> Self {
        Self {
            g: [
                Crackle::new(hash64(seed, tag, 0), sr),
                Crackle::new(hash64(seed, tag, 1), sr),
                Crackle::new(hash64(seed, tag, 2), sr),
            ],
        }
    }

    fn set_rate(&mut self, rate: f32) {
        self.g[0].set_rate_hz(rate * (1.0 - CRACKLE_COMMON));
        self.g[1].set_rate_hz(rate * (1.0 - CRACKLE_COMMON));
        self.g[2].set_rate_hz(rate * CRACKLE_COMMON);
    }

    #[inline]
    fn next(&mut self) -> (f32, f32) {
        let a = self.g[0].next_sample();
        let b = self.g[1].next_sample();
        let c = self.g[2].next_sample();
        (a + c, b + c)
    }

    #[cfg(test)]
    fn events(&self) -> u64 {
        self.g.iter().map(Crackle::events).sum()
    }
}

struct CrackleStage {
    seed: u64,
    sr: f32,
    crackle: Trio,
    pops: Trio,
    level: Smoother2,
    pop_level: Smoother2,
    last: [f32; 4],
    follows: bool,
}

impl CrackleStage {
    fn streams(seed: u64, sr: f32) -> (Trio, Trio) {
        let mut pops = Trio::new(seed, sr, 0x50);
        for c in &mut pops.g {
            c.set_min_amplitude(POP_MIN_AMP);
            c.set_decay_ms(POP_DECAY_MS);
        }
        (Trio::new(seed, sr, 0x43), pops)
    }

    fn new(sr: f32, info: &BuildInfo) -> Self {
        let (crackle, pops) = Self::streams(info.seed, sr);
        Self {
            seed: info.seed,
            sr,
            crackle,
            pops,
            level: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            pop_level: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            last: [f32::NAN; 4],
            follows: info.noise_follows_gain,
        }
    }

    /// Events fired so far by the crackle and the pop streams (test hook).
    #[cfg(test)]
    fn events(&self) -> (u64, u64) {
        (self.crackle.events(), self.pops.events())
    }
}

impl Stage for CrackleStage {
    fn reset(&mut self) {
        let (crackle, pops) = Self::streams(self.seed, self.sr);
        self.crackle = crackle;
        self.pops = pops;
        self.last = [f32::NAN; 4];
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        // rate_hz, amp, decay_ms, level_db, pop_rate_hz, pop_level_db
        if changed(&mut self.last[0], p[0]) {
            self.crackle.set_rate(p[0]);
        }
        if changed(&mut self.last[1], p[1]) {
            for c in &mut self.crackle.g {
                c.set_min_amplitude(p[1]);
            }
        }
        if changed(&mut self.last[2], p[2]) {
            for c in &mut self.crackle.g {
                c.set_decay_ms(p[2]);
            }
        }
        if changed(&mut self.last[3], p[4]) {
            self.pops.set_rate(p[4]);
        }
        self.level.set_target(level_to_lin(p[3]));
        self.pop_level.set_target(level_to_lin(p[5]));
        if snap {
            self.level.set_immediate(level_to_lin(p[3]));
            self.pop_level.set_immediate(level_to_lin(p[5]));
        }
    }

    fn process(&mut self, _inp: &Bus, out1: &mut Bus, _out2: &mut Bus, ctx: &Ctx) {
        for n in 0..SUB {
            let gc = gen_gain(self.level.next_value(), ctx, self.follows, n);
            let gp = gen_gain(self.pop_level.next_value(), ctx, self.follows, n);
            let (cl, cr) = self.crackle.next();
            let (pl, pr) = self.pops.next();
            out1[0][n] = cl * gc + pl * gp;
            out1[1][n] = cr * gc + pr * gp;
        }
    }
}

struct ClicksStage {
    sr: f32,
    rate: f32,
    phase: f64,
    state: f32,
    decay: f32,
    flip: bool,
    level: Smoother2,
    follows: bool,
}

impl ClicksStage {
    fn new(sr: f32, info: &BuildInfo) -> Self {
        Self {
            sr,
            rate: 50.0,
            phase: 0.0,
            state: 0.0,
            decay: exp2_det_f64(-std::f64::consts::LOG2_E / (0.0004 * f64::from(sr))) as f32,
            flip: false,
            level: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            follows: info.noise_follows_gain,
        }
    }
}

impl Stage for ClicksStage {
    fn reset(&mut self) {
        self.phase = 0.0;
        self.state = 0.0;
        self.flip = false;
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.level.set_target(level_to_lin(p[0]));
        self.rate = p[1];
        if snap {
            self.level.set_immediate(level_to_lin(p[0]));
        }
    }

    fn process(&mut self, _inp: &Bus, out1: &mut Bus, _out2: &mut Bus, ctx: &Ctx) {
        let inc = f64::from(self.rate) / f64::from(self.sr);
        for n in 0..SUB {
            self.phase += inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                self.flip = !self.flip;
                self.state += if self.flip { 1.0 } else { -1.0 };
            }
            let g = gen_gain(self.level.next_value(), ctx, self.follows, n);
            let x = self.state * g;
            self.state = if self.state.abs() < 1e-20 {
                0.0
            } else {
                self.state * self.decay
            };
            out1[0][n] = x;
            out1[1][n] = x;
        }
    }
}

// --- Stereo structure --------------------------------------------------------------------------

struct WidthStage {
    side: Smoother2,
}

impl WidthStage {
    fn new(sr: f32) -> Self {
        Self {
            side: Smoother2::with_class(1.0, SmootherClass::Fast, sr),
        }
    }
}

impl Stage for WidthStage {
    fn reset(&mut self) {}

    fn set(&mut self, p: &[f32], snap: bool) {
        self.side.set_target(p[0]);
        if snap {
            self.side.set_immediate(p[0]);
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let m = 0.5 * (inp[0][n] + inp[1][n]);
            let s = 0.5 * (inp[0][n] - inp[1][n]) * self.side.next_value();
            out1[0][n] = m + s;
            out1[1][n] = m - s;
        }
    }
}

struct MonoLowStage {
    xo: [Crossover; 2],
    freq: Smoother2,
}

impl MonoLowStage {
    fn new(sr: f32) -> Self {
        Self {
            xo: [Crossover::new(sr), Crossover::new(sr)],
            freq: Smoother2::with_class(150.0, SmootherClass::Medium, sr),
        }
    }
}

impl Stage for MonoLowStage {
    fn reset(&mut self) {
        for x in &mut self.xo {
            x.reset();
        }
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.freq.set_target(p[0]);
        if snap {
            self.freq.set_immediate(p[0]);
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        let mut f = self.freq.value();
        for _ in 0..SUB {
            f = self.freq.next_value();
        }
        for x in &mut self.xo {
            x.set_freq(f);
        }
        for n in 0..SUB {
            let (ll, lh) = self.xo[0].process(inp[0][n]);
            let (rl, rh) = self.xo[1].process(inp[1][n]);
            let m = 0.5 * (ll + rl);
            out1[0][n] = m + lh;
            out1[1][n] = m + rh;
        }
    }
}

struct MonoStage;

impl Stage for MonoStage {
    fn reset(&mut self) {}

    fn set(&mut self, _p: &[f32], _snap: bool) {}

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let m = 0.5 * (inp[0][n] + inp[1][n]);
            out1[0][n] = m;
            out1[1][n] = m;
        }
    }
}

struct SplitStage {
    xo: [Crossover; 2],
    freq: Smoother2,
}

impl SplitStage {
    fn new(sr: f32) -> Self {
        Self {
            xo: [Crossover::new(sr), Crossover::new(sr)],
            freq: Smoother2::with_class(1_000.0, SmootherClass::Medium, sr),
        }
    }
}

impl Stage for SplitStage {
    fn reset(&mut self) {
        for x in &mut self.xo {
            x.reset();
        }
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.freq.set_target(p[0]);
        if snap {
            self.freq.set_immediate(p[0]);
        }
    }

    fn two_outputs(&self) -> bool {
        true
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, out2: &mut Bus, _ctx: &Ctx) {
        let mut f = self.freq.value();
        for _ in 0..SUB {
            f = self.freq.next_value();
        }
        for x in &mut self.xo {
            x.set_freq(f);
        }
        for n in 0..SUB {
            for ch in 0..2 {
                let (lo, hi) = self.xo[ch].process(inp[ch][n]);
                out1[ch][n] = lo;
                out2[ch][n] = hi;
            }
        }
    }
}

// --- Helpers of the row 3 and row 4 stages -------------------------------------------------------

/// A sine-like oscillator: an `f64` phase accumulator and a parabola with one correction step
/// (error below 0.2 %), only `+ - * /`, so it is bit-identical on every platform.
#[derive(Debug, Clone, Copy)]
struct Osc {
    phase: f64,
    inc: f64,
}

/// `sin(2 pi phase)` approximated by a corrected parabola.
#[inline]
fn psin(phase: f64) -> f32 {
    let u = 2.0 * (phase - phase.floor()) - 1.0;
    let s = (-4.0 * u * (1.0 - u.abs())) as f32;
    s * (0.775 + 0.225 * s.abs())
}

impl Osc {
    fn new(phase: f64) -> Self {
        Self {
            phase: phase - phase.floor(),
            inc: 0.0,
        }
    }

    fn set_rate(&mut self, hz: f32, sample_rate: f32) {
        let inc = f64::from(hz) / f64::from(sample_rate);
        self.inc = if inc.is_finite() {
            inc.clamp(0.0, 0.5)
        } else {
            0.0
        };
    }

    /// The value at the current phase plus `offset` cycles.
    #[inline]
    fn at(&self, offset: f64) -> f32 {
        psin(self.phase + offset)
    }

    #[inline]
    fn advance(&mut self, n: usize) {
        self.phase += self.inc * n as f64;
        self.phase -= self.phase.floor();
    }

    fn restart(&mut self) {
        self.phase = 0.0;
    }
}

/// The write-only value of a smoother after `SUB` steps (the value for the block's coefficients).
fn block_end(sm: &mut Smoother2) -> f32 {
    let mut v = sm.value();
    for _ in 0..SUB {
        v = sm.next_value();
    }
    v
}

// --- Spectral (the long latency class) ---------------------------------------------------------

struct SpectralStage {
    sp: Spectral,
    last: [f32; 4],
}

impl SpectralStage {
    fn new(sr: f32, seed: u64) -> Self {
        let mut sp = Spectral::new(sr, frame_for_rate(sr, SPECTRAL_FRAME));
        sp.set_seed(seed);
        Self {
            sp,
            last: [f32::NAN; 4],
        }
    }
}

impl Stage for SpectralStage {
    fn latency(&self) -> u32 {
        self.sp.latency_frames()
    }

    fn reset(&mut self) {
        self.sp.reset();
    }

    fn set(&mut self, p: &[f32], _snap: bool) {
        let mut any = false;
        for (slot, v) in self.last.iter_mut().zip(p) {
            any |= changed(slot, *v);
        }
        if any {
            self.sp.set_algorithm(SpectralAlgo::Lossy {
                threshold_db: p[0],
                hold_ms: p[1],
                step_db: p[2],
                dropout: p[3],
            });
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let (l, r) = self.sp.process(inp[0][n], inp[1][n]);
            out1[0][n] = l;
            out1[1][n] = r;
        }
    }
}

// --- Phaser, auto-pan, echo, ring modulator ------------------------------------------------------

struct PhaserStage {
    ph: Phaser,
    last: [f32; 6],
}

impl PhaserStage {
    fn new(sr: f32) -> Self {
        let mut ph = Phaser::new(sr);
        ph.set_stages(6);
        Self {
            ph,
            last: [f32::NAN; 6],
        }
    }
}

impl Stage for PhaserStage {
    fn reset(&mut self) {
        self.ph.reset();
        self.ph.set_phase(0.0);
    }

    fn set(&mut self, p: &[f32], _snap: bool) {
        if changed(&mut self.last[0], p[0]) {
            self.ph.set_rate_hz(p[0]);
        }
        if changed(&mut self.last[1], p[1]) {
            self.ph.set_depth_oct(p[1]);
        }
        if changed(&mut self.last[2], p[2]) {
            self.ph.set_feedback(p[2]);
        }
        if changed(&mut self.last[3], p[3]) {
            self.ph.set_center_hz(p[3]);
        }
        if changed(&mut self.last[4], p[4]) {
            self.ph.set_mix(p[4]);
        }
        if changed(&mut self.last[5], p[5]) {
            self.ph.set_stereo(p[5]);
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let (l, r) = self.ph.process(inp[0][n], inp[1][n]);
            out1[0][n] = l;
            out1[1][n] = r;
        }
    }
}

struct AutoPanStage {
    sr: f32,
    lfo: Osc,
    depth: Smoother2,
    rate: f32,
}

impl AutoPanStage {
    fn new(sr: f32) -> Self {
        Self {
            sr,
            lfo: Osc::new(0.0),
            depth: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            rate: f32::NAN,
        }
    }
}

impl Stage for AutoPanStage {
    fn reset(&mut self) {
        self.lfo.restart();
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        if changed(&mut self.rate, p[0]) {
            self.lfo.set_rate(p[0], self.sr);
        }
        self.depth.set_target(p[1]);
        if snap {
            self.depth.set_immediate(p[1]);
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let pos = self.depth.next_value() * self.lfo.at(0.0);
            self.lfo.advance(1);
            // Constant power: the two squared gains always sum to 2, each is 1 at the centre.
            let gl = (1.0 - pos).max(0.0).sqrt();
            let gr = (1.0 + pos).max(0.0).sqrt();
            out1[0][n] = inp[0][n] * gl;
            out1[1][n] = inp[1][n] * gr;
        }
    }
}

/// Longest echo time of the echo stage in ms.
const ECHO_MAX_MS: f32 = 1_600.0;

struct EchoStage {
    e: Echo,
    last: [f32; 5],
}

impl EchoStage {
    fn new(sr: f32) -> Self {
        let mut e = Echo::new(sr, ECHO_MAX_MS);
        e.set_invert_right(true);
        Self {
            e,
            last: [f32::NAN; 5],
        }
    }
}

impl Stage for EchoStage {
    fn reset(&mut self) {
        self.e.reset();
        self.e.snap_time();
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        if changed(&mut self.last[0], p[0]) {
            self.e.set_time_ms(p[0]);
            if snap {
                self.e.snap_time();
            }
        }
        if changed(&mut self.last[1], p[1]) {
            self.e.set_feedback(p[1]);
        }
        if changed(&mut self.last[2], p[2]) {
            self.e.set_mix(p[2]);
        }
        let a = changed(&mut self.last[3], p[3]);
        let b = changed(&mut self.last[4], p[4]);
        if a || b {
            self.e.set_modulation(p[3], p[4]);
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let (l, r) = self.e.process(inp[0][n], inp[1][n]);
            out1[0][n] = l;
            out1[1][n] = r;
        }
    }
}

struct RingStage {
    rm: RingMod,
    last: [f32; 2],
}

impl RingStage {
    fn new(sr: f32) -> Self {
        Self {
            rm: RingMod::new(sr),
            last: [f32::NAN; 2],
        }
    }
}

impl Stage for RingStage {
    fn reset(&mut self) {
        self.rm.reset();
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        if changed(&mut self.last[0], p[0]) {
            self.rm.set_freq_hz(p[0]);
        }
        if changed(&mut self.last[1], p[1]) {
            self.rm.set_amount(p[1]);
        }
        if snap {
            self.rm.snap();
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let (l, r) = self.rm.process(inp[0][n], inp[1][n]);
            out1[0][n] = l;
            out1[1][n] = r;
        }
    }
}

// --- Comb and band-pass ----------------------------------------------------------------------------

/// Longest comb delay in ms.
const COMB_MAX_MS: f32 = 25.0;
/// Rate of the slow seeded comb wander in Hz.
const COMB_WANDER_HZ: f32 = 0.15;

struct CombStage {
    sr: f32,
    seed: u64,
    c: [Comb; 2],
    wander: [Wander; 2],
    delay: Smoother2,
    detune: f32,
    wander_depth: f32,
    norm: f32,
    last: [f32; 2],
}

impl CombStage {
    fn wanders(seed: u64, sr: f32) -> [Wander; 2] {
        [
            Wander::new(hash64(seed, 0xC0, 0), sr, COMB_WANDER_HZ),
            Wander::new(hash64(seed, 0xC0, 1), sr, COMB_WANDER_HZ),
        ]
    }

    fn new(sr: f32, seed: u64) -> Self {
        Self {
            sr,
            seed,
            c: [Comb::new(sr, COMB_MAX_MS), Comb::new(sr, COMB_MAX_MS)],
            wander: Self::wanders(seed, sr),
            delay: Smoother2::with_class(5.0, SmootherClass::Fast, sr),
            detune: 0.0,
            wander_depth: 0.0,
            norm: 1.0,
            last: [f32::NAN; 2],
        }
    }
}

impl Stage for CombStage {
    fn reset(&mut self) {
        for c in &mut self.c {
            c.reset();
        }
        self.wander = Self::wanders(self.seed, self.sr);
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        let d = p[0] * 0.001 * self.sr;
        self.delay.set_target(d);
        if snap {
            self.delay.set_immediate(d);
        }
        let (fb, ff) = (p[1], p[2]);
        if changed(&mut self.last[0], fb) {
            for c in &mut self.c {
                c.set_feedback(fb);
            }
        }
        if changed(&mut self.last[1], ff) {
            for c in &mut self.c {
                c.set_feedforward(ff);
            }
        }
        // White-noise power gain of the comb is (1 + ff^2 + 2 ff fb) / (1 - fb^2): undo it.
        let g2 = (1.0 + ff * ff + 2.0 * ff * fb).max(0.05) / (1.0 - fb * fb).max(0.02);
        self.norm = 1.0 / g2.sqrt();
        self.detune = p[3];
        self.wander_depth = p[4];
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let d = self.delay.next_value();
            for ch in 0..2 {
                let spread = if ch == 1 { self.detune } else { 0.0 };
                let w = (2.0 * self.wander[ch].next_sample() - 1.0) * self.wander_depth;
                self.c[ch].set_delay_samples(d * (1.0 + spread + w));
                out1[ch][n] = self.c[ch].process(inp[ch][n]) * self.norm;
            }
        }
    }
}

struct BandPassStage {
    sr: f32,
    f: [Svf; 2],
    freq: Smoother2,
    k: f32,
    res: f32,
    makeup: f32,
}

impl BandPassStage {
    fn new(sr: f32) -> Self {
        Self {
            sr,
            f: [Svf::new(), Svf::new()],
            freq: Smoother2::with_class(1_000.0, SmootherClass::Fast, sr),
            k: 1.0,
            res: 0.0,
            makeup: 1.0,
        }
    }
}

impl Stage for BandPassStage {
    fn reset(&mut self) {
        for f in &mut self.f {
            f.reset();
        }
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.freq.set_target(p[0]);
        if snap {
            self.freq.set_immediate(p[0]);
        }
        // Q = 0.5 * 40^r of the state-variable filter.
        self.res = (log2_det((2.0 * p[1]).max(1.0)) / 5.321_928).clamp(0.0, 1.0);
        self.k = 1.0 / (0.5 * exp2_det(self.res * 5.321_928));
        self.makeup = db_to_lin(p[2]);
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        let f = block_end(&mut self.freq);
        let g = Svf::cutoff_to_g(f, self.sr);
        for s in &mut self.f {
            s.set_coefficients(g, self.k);
        }
        // The band output of the filter peaks at Q; times k = 1/Q it peaks at unity.
        let gain = self.k * self.makeup;
        for n in 0..SUB {
            for ch in 0..2 {
                out1[ch][n] = self.f[ch].process(inp[ch][n]).bp * gain;
            }
        }
    }
}

// --- Crush ----------------------------------------------------------------------------------------

struct CrushStage {
    sr: f32,
    seed: u64,
    c: [Bitcrush; 2],
    rng: [Rng; 2],
    /// Stepped random values per channel: `[bits, rate]`, each in -1..1.
    r: [[f32; 2]; 2],
    phase: f64,
    bits: f32,
    rate: f32,
    mod_rate: f32,
    depth: f32,
    makeup: Smoother2,
    /// True until the first block after a start or a reset: the hold takes its first sample at
    /// once with the first block's rate.
    fresh: bool,
}

impl CrushStage {
    fn rngs(seed: u64) -> [Rng; 2] {
        [
            Rng::new(hash64(seed, 0xB1, 0)),
            Rng::new(hash64(seed, 0xB1, 1)),
        ]
    }

    fn new(sr: f32, seed: u64) -> Self {
        let mut s = Self {
            sr,
            seed,
            c: [Bitcrush::new(sr), Bitcrush::new(sr)],
            rng: Self::rngs(seed),
            r: [[0.0; 2]; 2],
            phase: 0.0,
            bits: 16.0,
            rate: sr,
            mod_rate: 0.0,
            depth: 0.0,
            makeup: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            fresh: true,
        };
        s.draw();
        s
    }

    fn draw(&mut self) {
        for ch in 0..2 {
            self.r[ch] = [self.rng[ch].next_bipolar(), self.rng[ch].next_bipolar()];
        }
    }

    /// Pushes the base values plus the current random offsets into the crushers.
    fn apply(&mut self) {
        for ch in 0..2 {
            let bits = (self.bits + self.depth * self.r[ch][0] * 3.0).clamp(1.0, 16.0);
            let rate = (self.rate * exp2_det(self.depth * self.r[ch][1])).clamp(100.0, self.sr);
            self.c[ch].set_bits(bits);
            self.c[ch].set_rate_hz(rate);
        }
    }
}

impl Stage for CrushStage {
    fn reset(&mut self) {
        for c in &mut self.c {
            c.reset();
        }
        self.rng = Self::rngs(self.seed);
        self.phase = 0.0;
        self.draw();
        self.fresh = true;
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.bits = p[0];
        self.rate = p[1];
        self.mod_rate = p[2];
        self.depth = p[3];
        let mk = p[4];
        self.makeup.set_target(mk);
        if snap {
            self.makeup.set_immediate(mk);
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        self.apply();
        if self.fresh {
            // The crushers restart their hold ramp for the rate that is now set.
            for c in &mut self.c {
                c.reset();
            }
            self.fresh = false;
        }
        let inc = f64::from(self.mod_rate) / f64::from(self.sr);
        for n in 0..SUB {
            self.phase += inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                self.draw();
                self.apply();
            }
            let mk = db_to_lin(self.makeup.next_value());
            for ch in 0..2 {
                out1[ch][n] = self.c[ch].process(inp[ch][n]) * mk;
            }
        }
    }
}

// --- Tremolo and the swept low-pass ---------------------------------------------------------------

struct TremoloStage {
    sr: f32,
    lfo: Osc,
    depth: Smoother2,
    phase: f32,
    rate: f32,
}

impl TremoloStage {
    fn new(sr: f32) -> Self {
        Self {
            sr,
            lfo: Osc::new(0.0),
            depth: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            phase: 0.0,
            rate: f32::NAN,
        }
    }
}

impl Stage for TremoloStage {
    fn reset(&mut self) {
        self.lfo.restart();
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        if changed(&mut self.rate, p[0]) {
            self.lfo.set_rate(p[0], self.sr);
        }
        self.depth.set_target(p[1]);
        if snap {
            self.depth.set_immediate(p[1]);
        }
        self.phase = p[2];
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let d = self.depth.next_value();
            let (a, b) = (self.lfo.at(0.0), self.lfo.at(f64::from(self.phase)));
            self.lfo.advance(1);
            // Gain between 1 - d and 1; the compensation keeps the level of noise constant.
            let base = 1.0 - 0.5 * d;
            let comp = 1.0 / (base * base + d * d * 0.125).sqrt();
            out1[0][n] = inp[0][n] * (base + 0.5 * d * a) * comp;
            out1[1][n] = inp[1][n] * (base + 0.5 * d * b) * comp;
        }
    }
}

struct SweepStage {
    sr: f32,
    bq: SmoothedBiquad,
    lfo: Osc,
    depth: Smoother2,
    lo: f32,
    hi: f32,
    rate: f32,
}

impl SweepStage {
    fn new(sr: f32) -> Self {
        Self {
            sr,
            bq: SmoothedBiquad::new(BiquadShape::Lowpass, 8_000.0, 1.0, 0.0, sr),
            lfo: Osc::new(0.0),
            depth: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            lo: 400.0,
            hi: 3_000.0,
            rate: f32::NAN,
        }
    }

    /// The cutoff for the current phase and depth: fully open at depth 0, sweeping between the
    /// corners (a log sweep) at depth 1.
    fn cutoff(&self, depth: f32) -> f32 {
        let (lo, hi) = (self.lo.min(self.hi), self.hi);
        let top = log2_det(hi);
        let span = top - log2_det(lo);
        exp2_det(top - depth * span * (0.5 - 0.5 * self.lfo.at(0.0)))
    }
}

impl Stage for SweepStage {
    fn reset(&mut self) {
        self.bq.reset();
        self.lfo.restart();
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.lo = p[0];
        self.hi = p[1];
        if changed(&mut self.rate, p[2]) {
            self.lfo.set_rate(p[2], self.sr);
        }
        self.depth.set_target(p[3]);
        self.bq.set_q(p[4]);
        if snap {
            self.depth.set_immediate(p[3]);
            let f = self.cutoff(p[3]);
            self.bq.set_freq(f);
            self.bq.snap();
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        let d = block_end(&mut self.depth);
        let f = self.cutoff(d);
        self.bq.set_freq(f);
        self.lfo.advance(SUB);
        for n in 0..SUB {
            let (l, r) = self.bq.process(inp[0][n], inp[1][n]);
            out1[0][n] = l;
            out1[1][n] = r;
        }
    }
}

// --- Hum and bursts (generators) ------------------------------------------------------------------

/// Amplitudes of the first five harmonics of the motor hum.
const HUM_HARMONICS: [f32; 5] = [1.0, 0.55, 0.35, 0.2, 0.12];

struct HumStage {
    sr: f32,
    phase: f64,
    inc: f64,
    norm: f32,
    level: Smoother2,
    follows: bool,
}

impl HumStage {
    fn new(sr: f32, info: &BuildInfo) -> Self {
        let power: f32 = HUM_HARMONICS.iter().map(|a| a * a).sum::<f32>() * 0.5;
        Self {
            sr,
            phase: 0.0,
            inc: 0.0,
            norm: 1.0 / power.sqrt(),
            level: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            follows: info.noise_follows_gain,
        }
    }
}

impl Stage for HumStage {
    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.level.set_target(level_to_lin(p[0]));
        self.inc = f64::from(p[1]) / f64::from(self.sr);
        if snap {
            self.level.set_immediate(level_to_lin(p[0]));
        }
    }

    fn process(&mut self, _inp: &Bus, out1: &mut Bus, _out2: &mut Bus, ctx: &Ctx) {
        for n in 0..SUB {
            let mut x = 0.0f32;
            for (h, a) in HUM_HARMONICS.iter().enumerate() {
                x += a * psin(self.phase * (h + 1) as f64);
            }
            self.phase += self.inc;
            self.phase -= self.phase.floor();
            let g = gen_gain(self.level.next_value(), ctx, self.follows, n);
            let y = x * self.norm * g;
            out1[0][n] = y;
            out1[1][n] = y;
        }
    }
}

struct BurstStage {
    sr: f32,
    seed: u64,
    rng: [Rng; 2],
    white: [WhiteNoise; 2],
    lp: [f32; 2],
    env: [f32; 2],
    p: f32,
    a: f32,
    norm: f32,
    decay: f32,
    level: Smoother2,
    follows: bool,
    #[cfg(test)]
    events: u64,
}

impl BurstStage {
    fn streams(seed: u64) -> ([Rng; 2], [WhiteNoise; 2]) {
        (
            [
                Rng::new(hash64(seed, 0xB5, 0)),
                Rng::new(hash64(seed, 0xB5, 1)),
            ],
            [
                WhiteNoise::new(hash64(seed, 0xB6, 0)),
                WhiteNoise::new(hash64(seed, 0xB6, 1)),
            ],
        )
    }

    fn new(sr: f32, info: &BuildInfo) -> Self {
        let (rng, white) = Self::streams(info.seed);
        Self {
            sr,
            seed: info.seed,
            rng,
            white,
            lp: [0.0; 2],
            env: [0.0; 2],
            p: 0.0,
            a: 0.3,
            norm: 1.0,
            decay: 0.999,
            level: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            follows: info.noise_follows_gain,
            #[cfg(test)]
            events: 0,
        }
    }
}

impl Stage for BurstStage {
    fn reset(&mut self) {
        let (rng, white) = Self::streams(self.seed);
        self.rng = rng;
        self.white = white;
        self.lp = [0.0; 2];
        self.env = [0.0; 2];
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.p = p[0] / self.sr;
        self.level.set_target(level_to_lin(p[1]));
        self.decay = exp2_det(-std::f32::consts::LOG2_E / (p[2] * 0.001 * self.sr));
        // One-pole low-pass of the white noise; the norm restores unit RMS.
        self.a = 1.0 - exp2_det(-std::f32::consts::TAU * p[3] / self.sr * std::f32::consts::LOG2_E);
        self.norm = ((2.0 - self.a) / self.a).sqrt();
        if snap {
            self.level.set_immediate(level_to_lin(p[1]));
        }
    }

    fn process(&mut self, _inp: &Bus, out1: &mut Bus, _out2: &mut Bus, ctx: &Ctx) {
        for n in 0..SUB {
            let g = gen_gain(self.level.next_value(), ctx, self.follows, n);
            for ch in 0..2 {
                if self.rng[ch].next_f32() < self.p {
                    let a = 0.5 + 0.5 * self.rng[ch].next_f32();
                    self.env[ch] = self.env[ch].max(a);
                    #[cfg(test)]
                    {
                        self.events += 1;
                    }
                }
                let w = self.white[ch].next_sample();
                self.lp[ch] += self.a * (w - self.lp[ch]);
                if self.lp[ch].abs() < 1e-20 {
                    self.lp[ch] = 0.0;
                }
                out1[ch][n] = self.lp[ch] * self.norm * self.env[ch] * g;
                self.env[ch] = if self.env[ch] < 1e-6 {
                    0.0
                } else {
                    self.env[ch] * self.decay
                };
            }
        }
    }
}

// --- Rattle, modal bank, room ---------------------------------------------------------------------

/// Release of the rattle's input envelope in seconds.
const RATTLE_RELEASE_S: f32 = 0.03;

struct RattleStage {
    sr: f32,
    seed: u64,
    white: [WhiteNoise; 2],
    lp: [f32; 2],
    env: [f32; 2],
    amount: Smoother2,
    a: f32,
    rel: f32,
}

impl RattleStage {
    fn streams(seed: u64) -> [WhiteNoise; 2] {
        [
            WhiteNoise::new(hash64(seed, 0xA7, 0)),
            WhiteNoise::new(hash64(seed, 0xA7, 1)),
        ]
    }

    fn new(sr: f32, seed: u64) -> Self {
        Self {
            sr,
            seed,
            white: Self::streams(seed),
            lp: [0.0; 2],
            env: [0.0; 2],
            amount: Smoother2::with_class(0.0, SmootherClass::Fast, sr),
            a: 0.3,
            rel: 1.0 - 1.0 / (RATTLE_RELEASE_S * sr),
        }
    }
}

impl Stage for RattleStage {
    fn reset(&mut self) {
        self.white = Self::streams(self.seed);
        self.lp = [0.0; 2];
        self.env = [0.0; 2];
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.amount.set_target(p[0]);
        self.a = 1.0 - exp2_det(-std::f32::consts::TAU * p[1] / self.sr * std::f32::consts::LOG2_E);
        if snap {
            self.amount.set_immediate(p[0]);
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let amount = self.amount.next_value();
            for ch in 0..2 {
                let x = inp[ch][n];
                self.env[ch] = x.abs().max(self.env[ch] * self.rel).min(8.0);
                if self.env[ch] < 1e-9 {
                    self.env[ch] = 0.0;
                }
                let w = self.white[ch].next_sample();
                self.lp[ch] += self.a * (w - self.lp[ch]);
                // The high-passed noise (the white noise minus its low-pass), keyed by the envelope.
                let noise = w - self.lp[ch];
                out1[ch][n] = x + amount * 0.5 * self.env[ch].min(1.0) * noise;
            }
        }
    }
}

/// Frequency ratios of the twelve modes (inharmonic, our own).
const MODAL_RATIOS: [f32; 12] = [
    1.0, 1.59, 2.14, 2.30, 2.65, 2.92, 3.16, 3.50, 3.87, 4.21, 4.60, 5.02,
];

struct ModalStage {
    m: Modal,
    /// The level-dependent limiter after the bank (a resonator excited by a tone at its
    /// frequency would otherwise ring far louder than broadband material does).
    limit: [Shaper; 2],
    offs: [f32; 12],
    root: Smoother2,
    last: [u32; 4],
    p: [f32; 4],
}

impl ModalStage {
    fn new(sr: f32, seed: u64) -> Self {
        let mut rng = Rng::new(hash64(seed, 0x3D, 0));
        let offs = std::array::from_fn(|_| rng.next_bipolar());
        let mut limit = [Shaper::new(sr), Shaper::new(sr)];
        for l in &mut limit {
            l.set_kind(ShaperKind::Dynamic);
            l.set_drive_db(0.0);
            l.reset();
        }
        Self {
            m: Modal::new(sr),
            limit,
            offs,
            root: Smoother2::with_class(110.0, SmootherClass::Medium, sr),
            last: [u32::MAX; 4],
            p: [1.0, 0.0, 4.0, 1.0],
        }
    }

    fn refresh(&mut self, root: f32) {
        let [decay, detune, active, gain] = self.p;
        let key = [root, decay, detune, gain].map(f32::to_bits);
        if key[..2] == self.last[..2] && key[2..] == self.last[2..] {
            return;
        }
        self.last = [key[0], key[1], key[2], key[3]];
        for i in 0..12 {
            let f = root * MODAL_RATIOS[i] * (1.0 + detune * self.offs[i]);
            // Higher modes die a little sooner.
            let rt = decay * (1.0 - 0.04 * i as f32);
            // The bandwidth of a mode falls with its decay time, so the level of broadband
            // input would fall with it; the square root of the decay restores a constant level.
            self.m.set_mode(i, f, rt, gain * rt.max(0.01).sqrt());
        }
        self.m.set_active(active.round().clamp(1.0, 12.0) as usize);
    }
}

impl Stage for ModalStage {
    fn reset(&mut self) {
        self.m.reset();
        for l in &mut self.limit {
            l.reset();
        }
    }

    fn set(&mut self, p: &[f32], snap: bool) {
        self.root.set_target(p[0]);
        if snap {
            self.root.set_immediate(p[0]);
        }
        self.p = [p[1], p[2], p[3], p[4]];
        if snap {
            self.last = [u32::MAX; 4];
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        let root = block_end(&mut self.root);
        self.refresh(root);
        for n in 0..SUB {
            let (l, r) = self.m.process(0.5 * (inp[0][n] + inp[1][n]));
            out1[0][n] = self.limit[0].process(l);
            out1[1][n] = self.limit[1].process(r);
        }
    }
}

struct RoomStage {
    r: Room,
    last: [f32; 3],
}

impl RoomStage {
    fn new(sr: f32, seed: u64) -> Self {
        Self {
            r: Room::new(sr, seed, RoomRange::Small),
            last: [f32::NAN; 3],
        }
    }
}

impl Stage for RoomStage {
    fn reset(&mut self) {
        self.r.reset();
    }

    fn set(&mut self, p: &[f32], _snap: bool) {
        if changed(&mut self.last[0], p[0]) {
            self.r.set_size(p[0]);
        }
        if changed(&mut self.last[1], p[1]) {
            self.r.set_rt60_s(p[1]);
        }
        if changed(&mut self.last[2], p[2]) {
            self.r.set_damping(p[2]);
        }
    }

    fn process(&mut self, inp: &Bus, out1: &mut Bus, _out2: &mut Bus, _ctx: &Ctx) {
        for n in 0..SUB {
            let (l, r) = self.r.process(inp[0][n], inp[1][n]);
            out1[0][n] = l;
            out1[1][n] = r;
        }
    }
}

#[cfg(test)]
mod rows34;

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn info(seed: u64, follows: bool) -> BuildInfo {
        BuildInfo {
            sample_rate: SR,
            seed,
            noise_follows_gain: follows,
        }
    }

    fn open_ctx<'a>(presence: &'a [f32; SUB], gain: &'a [f32; SUB]) -> Ctx<'a> {
        Ctx { presence, gain }
    }

    #[test]
    fn the_crackle_rate_and_pop_rate_match_the_table_within_ten_percent() {
        // Poisson counts: scale the duration with the rate so each run sees >= 2000 events.
        for (rate, pop_rate) in [(30.0f32, 1.0f32), (6.0, 1.0)] {
            let mut st = CrackleStage::new(SR, &info(5, false));
            st.set(&[rate, 0.05, 0.5, 0.0, pop_rate, -20.0], true);
            let presence = [1.0f32; SUB];
            let gain = [1.0f32; SUB];
            let ctx = open_ctx(&presence, &gain);
            let secs = 2_500.0 / rate * 1.0;
            let blocks = (secs * SR / SUB as f32) as usize;
            let (mut a, mut b) = ([[0.0; SUB]; 2], [[0.0; SUB]; 2]);
            let inp = [[0.0; SUB]; 2];
            for _ in 0..blocks {
                st.process(&inp, &mut a, &mut b, &ctx);
            }
            let (ev, pops) = st.events();
            let measured = ev as f32 / (blocks * SUB) as f32 * SR;
            // Each channel sees the full rate: the three streams carry 0.6 + 0.6 + 0.4 of it.
            let expect = rate * (0.6 + 0.6 + 0.4);
            assert!(
                (measured / expect - 1.0).abs() < 0.1,
                "rate {rate}: {measured} vs {expect}"
            );
            let pop_measured = pops as f32 / (blocks * SUB) as f32 * SR;
            let pop_expect = pop_rate * 1.6;
            assert!(
                (pop_measured / pop_expect - 1.0).abs() < 0.25,
                "pops {pop_measured} vs {pop_expect}"
            );
        }
    }

    #[test]
    fn crackle_is_silent_when_the_gate_is_closed() {
        let mut st = CrackleStage::new(SR, &info(5, false));
        st.set(&[30.0, 0.05, 0.5, 0.0, 1.0, 0.0], true);
        let presence = [0.0f32; SUB];
        let gain = [1.0f32; SUB];
        let ctx = open_ctx(&presence, &gain);
        let (mut a, mut b) = ([[0.0; SUB]; 2], [[0.0; SUB]; 2]);
        let inp = [[0.0; SUB]; 2];
        for _ in 0..2000 {
            st.process(&inp, &mut a, &mut b, &ctx);
            assert!(a.iter().flatten().all(|v| v.abs() < f32::MIN_POSITIVE));
        }
    }

    #[test]
    fn the_noise_gain_flag_decides_whether_the_input_gain_scales_the_noise() {
        for follows in [false, true] {
            let mut st = HissStage::new(SR, &info(2, follows));
            st.set(&[-20.0], true);
            let presence = [1.0f32; SUB];
            let gain = [0.0f32; SUB];
            let ctx = open_ctx(&presence, &gain);
            let (mut a, mut b) = ([[0.0; SUB]; 2], [[0.0; SUB]; 2]);
            let inp = [[0.0; SUB]; 2];
            st.process(&inp, &mut a, &mut b, &ctx);
            let peak = a[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert_eq!(peak > 0.0, !follows, "follows={follows}");
        }
    }

    #[test]
    fn kind_tables_are_consistent() {
        for k in [
            Kind::Peak,
            Kind::LowShelf,
            Kind::HighShelf,
            Kind::HighPass,
            Kind::LowPass,
            Kind::Sat(ShaperKind::SoftClip),
            Kind::Comp,
            Kind::Leveller,
            Kind::Wobble,
            Kind::Hiss,
            Kind::Rumble,
            Kind::Crackle,
            Kind::Clicks,
            Kind::Width,
            Kind::MonoLow,
            Kind::Mono,
            Kind::Split,
        ] {
            for p in k.params() {
                assert!(p.min < p.max, "{k:?} {}", p.name);
            }
            let s = build(k, &info(1, false));
            assert_eq!(s.latency(), k.latency(SR), "{k:?}");
            assert_eq!(s.two_outputs(), k == Kind::Split);
        }
        assert!(Kind::Hiss.generator() && !Kind::Peak.generator());
    }

    #[test]
    fn the_sat_stage_bypasses_the_oversampler_at_high_rates_with_the_same_latency() {
        for sr in [88_200.0f32, 96_000.0] {
            let mut st = SatStage::new(ShaperKind::SoftClip, sr);
            assert_eq!(st.ratio, 1);
            st.set(&[0.0, 0.0], true);
            let presence = [1.0f32; SUB];
            let gain = [1.0f32; SUB];
            let ctx = open_ctx(&presence, &gain);
            let mut inp = [[0.0; SUB]; 2];
            inp[0][0] = 0.1;
            let (mut a, mut b) = ([[0.0; SUB]; 2], [[0.0; SUB]; 2]);
            let mut out = Vec::new();
            for blk in 0..4 {
                if blk > 0 {
                    inp = [[0.0; SUB]; 2];
                }
                st.process(&inp, &mut a, &mut b, &ctx);
                out.extend_from_slice(&a[0]);
            }
            let pk = out
                .iter()
                .enumerate()
                .fold(0, |m, (i, v)| if v.abs() > out[m].abs() { i } else { m });
            assert_eq!(pk, 29, "sr {sr}");
        }
    }
}
