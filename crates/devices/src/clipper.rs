//! `atomik.clipper`: a stereo clipper. The signal is driven into a ceiling through a curve that
//! runs from a hard clip to a soft knee, with an optional asymmetry, oversampling, a multiband
//! mode with a drive per band, a dry/wet mix, and a delta switch that plays what the clipper
//! takes away.
//!
//! # Signal flow (per channel; the two channels are processed independently)
//!
//! ```text
//! single band   x -> input gain -> [oversample -> curve -> filter] -> DC block -> guard -> wet
//! multiband     x -> crossover -> band_i * input gain * band drive_i -> [curve, as above] -> sum
//!                                                              -> DC block -> wet
//! out = (dry * (1 - mix) + wet * mix) * output gain          (dry: x delayed by the latency)
//! delta on:  out = (driven - wet) * output gain              (driven: the pre-curve signal)
//! ```
//!
//! * **Curve.** `ceiling` (dB, -24..0) sets the level the curve holds the signal under. `curve`
//!   0..1 runs from a hard clip to a soft knee; the formula is that of
//!   [`atomik_dsp::clipper`]: with `k = 1 - curve`, a magnitude `a` at or below `k * ceiling`
//!   passes untouched, and above it `out = k c + (1 - k) c * t / sqrt(1 + t^2)` with
//!   `t = (a - k c) / ((1 - k) c)` (a hard clip when `k` is 1). The knee has slope 1 where it
//!   starts and the curve settles on the ceiling without passing it.
//! * **Symmetry.** -1..+1 lowers the ceiling of one half of the wave to `ceiling * (1 - |s| / 2)`
//!   (positive values lower the positive half, negative ones the negative half). That adds even
//!   harmonics and a DC offset; a one-pole high-pass at 8 Hz takes the offset out after the
//!   curve (it is in the path only while `symmetry` is not 0).
//! * **Oversampling.** `oversample` off, 2x, 4x or 8x runs the curve between the halfband
//!   stages of [`atomik_dsp::oversample::Oversampler`] (one per channel and band). The reported
//!   latency is that of the filters: 0 (off), 29, 35 or 37 samples. It changes with the setting
//!   (`variable_latency`). The dry path and the delta reference are delayed by the same amount.
//! * **Multiband.** `bands` 2 splits at `xover.low`; 3 adds a second split at `xover.high`
//!   (never below 1.25 times the low one). The crossovers are the 24 dB/oct Linkwitz-Riley
//!   splits of [`atomik_dsp::Crossover`] wired as in the mastering device, with the low band
//!   passed through an allpass of the high split: the bands add up to an allpass, so with no
//!   clipping the magnitude is flat. Each band has a drive `bandN.drive` (dB) in front of
//!   its own curve (band 3 does nothing in the 2-band mode). In this mode the ceiling is that of
//!   each band: there is no guard on the sum, so it can pass the ceiling by what the other bands
//!   carry at the same moment (follow it with a single-band clipper for a hard ceiling).
//! * **Guard.** `guard` (on by default), in the single-band mode: after the oversampling filters
//!   (which overshoot the ceiling a little, by up to several percent after a long hard clip) and
//!   the DC block, the wet signal is held to the ceiling with a hard clamp, so the ceiling is a
//!   hard limit on the output. The clamp runs at the base rate, so on heavy clipping of high
//!   tones it brings back a little of the aliasing the oversampling removed; switch it off to
//!   let the overshoot through. With no oversampling and symmetry 0 the guard never acts.
//! * **Bit-transparent below the ceiling** when `curve` is 0 (hard), `oversample` is off,
//!   `symmetry` is 0, `bands` is off, `mix` is 100 %, `delta` is off and the input and output
//!   gains are 0 dB: every sample below the ceiling leaves exactly as it came in, and every
//!   sample above it leaves at the ceiling. A soft `curve` also leaves everything below its knee
//!   (`(1 - curve) * ceiling`) untouched.
//! * **Clip activity.** A sample counts as clipped when either channel (any band, in multiband
//!   mode) is beyond its ceiling before the curve. Over the last one to two seconds the device
//!   reports the share of clipped samples and the longest run of consecutive clipped samples in
//!   milliseconds (a run of 1 ms or less is rarely heard as anything but a click of density;
//!   longer runs are audible as distortion).
//! * **Meters** (`meter_values`): the peak of the driven signal, the peak of the output (both
//!   linear), the clipped share in percent and the longest clipped run in ms.
//! * Gains, ceiling and mix are smoothed per batch (10 ms). Deterministic maths; non-finite input
//!   samples count as 0; allocation-free after construction.

use crate::bypass::{Bypassable, bypass_param};
use crate::tool_support::{audio_in, audio_out, choice, expo, finite, linear, toggle};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::clipper::{ClipCurve, DcBlock};
use atomik_dsp::oversample::{OversampleFactor, Oversampler};
use atomik_dsp::{BlockSmoother, Crossover, DelayLine, Ramp, exp2_det};

const P_GAIN: u32 = 0;
const P_CEILING: u32 = 1;
const P_CURVE: u32 = 2;
const P_SYMMETRY: u32 = 3;
const P_OVERSAMPLE: u32 = 4;
const P_OUTPUT: u32 = 5;
const P_MIX: u32 = 6;
const P_DELTA: u32 = 7;
const P_BANDS: u32 = 8;
const P_XOVER_LOW: u32 = 9;
const P_XOVER_HIGH: u32 = 10;
const P_DRIVE1: u32 = 11;
const P_DRIVE2: u32 = 12;
const P_DRIVE3: u32 = 13;
const P_GUARD: u32 = 14;

/// The `oversample` choices.
static OVERSAMPLES: [&str; 4] = ["Off", "2x", "4x", "8x"];
/// The `bands` choices.
static BANDS: [&str; 3] = ["Off", "2 bands", "3 bands"];

/// `log2(10) / 20`: `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// The meter values: driven peak, output peak, clipped share (percent), longest run (ms).
pub const METERS: usize = 4;
/// Indices into the meter values.
pub mod meter {
    /// Peak of the driven signal (linear).
    pub const DRIVEN: usize = 0;
    /// Peak of the output (linear).
    pub const OUTPUT: usize = 1;
    /// Share of clipped samples, percent.
    pub const SHARE: usize = 2;
    /// Longest clipped run, milliseconds.
    pub const LONGEST: usize = 3;
}
/// The length of one activity window slot, seconds (the report covers one to two slots).
const SLOT_SECONDS: f64 = 1.0;

static PARAMS: [ParamDescriptor; 16] = [
    linear(P_GAIN, "gain", "Input gain", -24.0, 36.0, 0.0, Unit::Db),
    linear(P_CEILING, "ceiling", "Ceiling", -24.0, 0.0, -1.0, Unit::Db),
    linear(P_CURVE, "curve", "Curve", 0.0, 1.0, 0.0, Unit::Percent),
    linear(
        P_SYMMETRY,
        "symmetry",
        "Symmetry",
        -1.0,
        1.0,
        0.0,
        Unit::None,
    ),
    choice(
        P_OVERSAMPLE,
        "oversample",
        "Oversampling",
        &OVERSAMPLES,
        1.0,
    ),
    linear(P_OUTPUT, "output", "Output", -24.0, 12.0, 0.0, Unit::Db),
    linear(P_MIX, "mix", "Mix", 0.0, 1.0, 1.0, Unit::Percent),
    toggle(P_DELTA, "delta", "Delta", 0.0),
    choice(P_BANDS, "bands", "Multiband", &BANDS, 0.0),
    expo(
        P_XOVER_LOW,
        "xover.low",
        "Crossover low",
        40.0,
        2_000.0,
        150.0,
        Unit::Hz,
    ),
    expo(
        P_XOVER_HIGH,
        "xover.high",
        "Crossover high",
        500.0,
        12_000.0,
        3_000.0,
        Unit::Hz,
    ),
    linear(
        P_DRIVE1,
        "band1.drive",
        "Band 1 drive",
        -12.0,
        24.0,
        0.0,
        Unit::Db,
    ),
    linear(
        P_DRIVE2,
        "band2.drive",
        "Band 2 drive",
        -12.0,
        24.0,
        0.0,
        Unit::Db,
    ),
    linear(
        P_DRIVE3,
        "band3.drive",
        "Band 3 drive",
        -12.0,
        24.0,
        0.0,
        Unit::Db,
    ),
    toggle(P_GUARD, "guard", "Hold ceiling", 1.0),
    bypass_param(15),
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

/// Static description of the clipper.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.clipper",
    name: "Clipper",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Clipper`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(Clipper::new())),
    }
}

/// Linear gain of `db` (exactly 1 at 0 dB).
#[inline]
fn lin(db: f32) -> f32 {
    if db == 0.0 {
        1.0
    } else {
        exp2_det(db * LOG2_10_20)
    }
}

/// The oversampling factor of a `oversample` choice.
fn factor_of(choice: u8) -> OversampleFactor {
    match choice {
        0 => OversampleFactor::X1,
        1 => OversampleFactor::X2,
        2 => OversampleFactor::X4,
        _ => OversampleFactor::X8,
    }
}

/// Everything the batch needs from the params, resolved once per batch.
struct Batch {
    curve: ClipCurve,
    ceiling: f32,
    gain: Ramp,
    drives: [Ramp; 3],
    latency: usize,
    bands: u8,
    asymmetric: bool,
    guard: bool,
}

/// The state of one channel.
#[derive(Debug, Clone)]
struct Lane {
    split_a: Crossover,
    split_b: Crossover,
    comp: Crossover,
    os: [Oversampler; 3],
    dry: DelayLine,
    driven: DelayLine,
    dc: DcBlock,
}

impl Lane {
    fn new(sample_rate: f32, factor: OversampleFactor) -> Self {
        let mut os = [
            Oversampler::new(BATCH),
            Oversampler::new(BATCH),
            Oversampler::new(BATCH),
        ];
        for o in &mut os {
            o.set_factor(factor);
        }
        Self {
            split_a: Crossover::new(sample_rate),
            split_b: Crossover::new(sample_rate),
            comp: Crossover::new(sample_rate),
            os,
            dry: DelayLine::new(64),
            driven: DelayLine::new(64),
            dc: DcBlock::new(sample_rate),
        }
    }

    fn reset(&mut self) {
        self.split_a.reset();
        self.split_b.reset();
        self.comp.reset();
        for o in &mut self.os {
            o.reset();
        }
        self.dry.clear();
        self.driven.clear();
        self.dc.reset();
    }
}

/// What one lane gives for one batch.
struct LaneOut {
    wet: [f32; BATCH],
    dry: [f32; BATCH],
    driven: [f32; BATCH],
    /// Peak of the driven signal as it reaches the curve (not delayed).
    driven_peak: f32,
}

/// The curve over one band: oversampled when `factor` is not 1x. Marks the samples that reach
/// the ceiling in `flags`.
fn clip_band(
    os: &mut Oversampler,
    curve: &ClipCurve,
    d: &[f32; BATCH],
    flags: &mut [bool; BATCH],
) -> [f32; BATCH] {
    let mut out = [0.0f32; BATCH];
    for n in 0..BATCH {
        flags[n] |= curve.reaches_ceiling(d[n]);
    }
    if os.factor() == OversampleFactor::X1 {
        for n in 0..BATCH {
            out[n] = curve.apply(d[n]);
        }
    } else {
        for v in os.upsample(d) {
            *v = curve.apply(*v);
        }
        os.downsample(&mut out);
    }
    out
}

/// The clipper device.
#[derive(Debug, Clone)]
pub struct Clipper {
    sample_rate: f32,
    gain_db: f32,
    ceiling_db: f32,
    curve: f32,
    symmetry: f32,
    oversample: u8,
    output_db: f32,
    mix: f32,
    delta: bool,
    guard: bool,
    bands: u8,
    xover_low: f32,
    xover_high: f32,
    drive_db: [f32; 3],
    fresh: bool,
    gain: BlockSmoother,
    ceiling: BlockSmoother,
    output: BlockSmoother,
    mix_sm: BlockSmoother,
    drives: [BlockSmoother; 3],
    lanes: [Lane; 2],
    // Clip activity: the current and the previous slot.
    slot_len: u64,
    cur: Slot,
    prev: Slot,
    run: u64,
    meters: [f32; METERS],
}

/// Counts of one activity slot.
#[derive(Debug, Clone, Copy, Default)]
struct Slot {
    samples: u64,
    clipped: u64,
    longest: u64,
}

impl Default for Clipper {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipper {
    /// A clipper with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let mut c = Self {
            sample_rate: 48_000.0,
            gain_db: 0.0,
            ceiling_db: -1.0,
            curve: 0.0,
            symmetry: 0.0,
            oversample: 1,
            output_db: 0.0,
            mix: 1.0,
            delta: false,
            guard: true,
            bands: 0,
            xover_low: 150.0,
            xover_high: 3_000.0,
            drive_db: [0.0; 3],
            fresh: true,
            gain: BlockSmoother::new(1.0),
            ceiling: BlockSmoother::new(1.0),
            output: BlockSmoother::new(1.0),
            mix_sm: BlockSmoother::new(1.0),
            drives: [BlockSmoother::new(1.0); 3],
            lanes: [
                Lane::new(48_000.0, OversampleFactor::X2),
                Lane::new(48_000.0, OversampleFactor::X2),
            ],
            slot_len: 48_000,
            cur: Slot::default(),
            prev: Slot::default(),
            run: 0,
            meters: [0.0, 0.0, 0.0, 0.0],
        };
        c.set_crossovers();
        c
    }

    fn factor(&self) -> OversampleFactor {
        factor_of(self.oversample)
    }

    /// The effective crossover frequencies `(low, high)` in Hz (`high >= 1.25 * low`, both
    /// below 0.45 of the sample rate).
    fn crossovers(&self) -> (f32, f32) {
        let ny = 0.45 * self.sample_rate;
        let low = self.xover_low.min(ny / 1.25);
        let high = self.xover_high.max(low * 1.25).min(ny);
        (low, high)
    }

    fn set_crossovers(&mut self) {
        let (low, high) = self.crossovers();
        for lane in &mut self.lanes {
            lane.split_a.set_freq(low);
            lane.split_b.set_freq(high);
            lane.comp.set_freq(high);
        }
    }

    /// The band signals of one sample in the multiband mode: `[low, mid, high]` (the mid band is
    /// silent in the 2-band mode, whose second band is `high`).
    #[inline]
    fn split(lane: &mut Lane, x: f32, bands: u8) -> [f32; 3] {
        let (low, h1) = lane.split_a.process(x);
        if bands >= 2 {
            let (mid, high) = lane.split_b.process(h1);
            let (a, b) = lane.comp.process(low);
            [a + b, mid, high]
        } else {
            [low, 0.0, h1]
        }
    }

    /// One batch of one channel.
    fn run_lane(
        lane: &mut Lane,
        b: &Batch,
        input: &[f32; BATCH],
        flags: &mut [bool; BATCH],
    ) -> LaneOut {
        let mut driven = [0.0f32; BATCH];
        let mut dry_src = [0.0f32; BATCH];
        let mut wet = [0.0f32; BATCH];
        if b.bands == 0 {
            for n in 0..BATCH {
                let x = finite(input[n]);
                dry_src[n] = x;
                driven[n] = x * b.gain.at(n);
            }
            wet = clip_band(&mut lane.os[0], &b.curve, &driven, flags);
        } else {
            let count = if b.bands >= 2 { 3 } else { 2 };
            let mut bands = [[0.0f32; BATCH]; 3];
            for n in 0..BATCH {
                let x = finite(input[n]);
                let s = Self::split(lane, x, b.bands);
                let slots: [usize; 3] = if b.bands >= 2 { [0, 1, 2] } else { [0, 2, 2] };
                dry_src[n] = if b.bands >= 2 {
                    s[0] + s[1] + s[2]
                } else {
                    s[0] + s[2]
                };
                for (i, &slot) in slots.iter().enumerate().take(count) {
                    let drive = b.drives[i].at(n);
                    let d = s[slot] * (b.gain.at(n) * drive);
                    bands[i][n] = d;
                    driven[n] += d;
                }
            }
            for (os, band) in lane.os.iter_mut().zip(&bands).take(count) {
                let y = clip_band(os, &b.curve, band, flags);
                for (w, v) in wet.iter_mut().zip(y) {
                    *w += v;
                }
            }
        }
        // Past the filters: the offset an asymmetric curve makes, then the guard.
        let mut dry = [0.0f32; BATCH];
        let mut driven_d = [0.0f32; BATCH];
        let mut driven_peak = 0.0f32;
        for n in 0..BATCH {
            driven_peak = driven_peak.max(driven[n].abs());
            let mut y = wet[n];
            if b.asymmetric {
                y = lane.dc.process(y);
            }
            wet[n] = if b.guard && b.bands == 0 {
                y.clamp(-b.ceiling, b.ceiling)
            } else {
                y
            };
            if b.latency == 0 {
                dry[n] = dry_src[n];
                driven_d[n] = driven[n];
            } else {
                dry[n] = lane.dry.read(b.latency as f32);
                driven_d[n] = lane.driven.read(b.latency as f32);
            }
            lane.dry.write(dry_src[n]);
            lane.driven.write(driven[n]);
        }
        if !b.asymmetric {
            lane.dc.reset();
        }
        LaneOut {
            wet,
            dry,
            driven: driven_d,
            driven_peak,
        }
    }

    /// Folds one batch of clip flags into the activity counts.
    fn record(&mut self, flags: &[bool; BATCH]) {
        for &f in flags {
            self.cur.samples += 1;
            if f {
                self.cur.clipped += 1;
                self.run += 1;
                self.cur.longest = self.cur.longest.max(self.run);
            } else {
                self.run = 0;
            }
        }
        if self.cur.samples >= self.slot_len {
            self.prev = self.cur;
            self.cur = Slot {
                samples: 0,
                clipped: 0,
                longest: self.run,
            };
        }
        let samples = (self.cur.samples + self.prev.samples).max(1);
        let clipped = self.cur.clipped + self.prev.clipped;
        let longest = self.cur.longest.max(self.prev.longest);
        self.meters[meter::SHARE] = 100.0 * clipped as f32 / samples as f32;
        self.meters[meter::LONGEST] = 1000.0 * longest as f32 / self.sample_rate;
    }
}

impl Device for Clipper {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        let factor = self.factor();
        self.lanes = [
            Lane::new(self.sample_rate, factor),
            Lane::new(self.sample_rate, factor),
        ];
        self.set_crossovers();
        for s in [
            &mut self.gain,
            &mut self.ceiling,
            &mut self.output,
            &mut self.mix_sm,
        ]
        .into_iter()
        .chain(self.drives.iter_mut())
        {
            s.set_time(10.0, self.sample_rate, BATCH);
        }
        self.slot_len = (f64::from(self.sample_rate) * SLOT_SECONDS) as u64;
        self.reset();
    }

    fn reset(&mut self) {
        self.fresh = true;
        for lane in &mut self.lanes {
            lane.reset();
        }
        self.cur = Slot::default();
        self.prev = Slot::default();
        self.run = 0;
        self.meters = [0.0; METERS];
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = finite(plain);
        match id.0 {
            P_GAIN => self.gain_db = v.clamp(-24.0, 36.0),
            P_CEILING => self.ceiling_db = v.clamp(-24.0, 0.0),
            P_CURVE => self.curve = v.clamp(0.0, 1.0),
            P_SYMMETRY => self.symmetry = v.clamp(-1.0, 1.0),
            P_OVERSAMPLE => {
                self.oversample = v.round().clamp(0.0, 3.0) as u8;
                let factor = self.factor();
                for lane in &mut self.lanes {
                    for o in &mut lane.os {
                        o.set_factor(factor);
                    }
                }
            }
            P_OUTPUT => self.output_db = v.clamp(-24.0, 12.0),
            P_MIX => self.mix = v.clamp(0.0, 1.0),
            P_DELTA => self.delta = v >= 0.5,
            P_GUARD => self.guard = v >= 0.5,
            P_BANDS => {
                let bands = v.round().clamp(0.0, 2.0) as u8;
                if bands != self.bands {
                    self.bands = bands;
                    for lane in &mut self.lanes {
                        lane.split_a.reset();
                        lane.split_b.reset();
                        lane.comp.reset();
                    }
                }
            }
            P_XOVER_LOW => {
                self.xover_low = v.clamp(40.0, 2_000.0);
                self.set_crossovers();
            }
            P_XOVER_HIGH => {
                self.xover_high = v.clamp(500.0, 12_000.0);
                self.set_crossovers();
            }
            P_DRIVE1 => self.drive_db[0] = v.clamp(-12.0, 24.0),
            P_DRIVE2 => self.drive_db[1] = v.clamp(-12.0, 24.0),
            P_DRIVE3 => self.drive_db[2] = v.clamp(-12.0, 24.0),
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let inputs = [*ctx.input(IN_L), *ctx.input(IN_R)];
        let targets = [
            lin(self.gain_db),
            lin(self.ceiling_db),
            lin(self.output_db),
            self.mix,
        ];
        let drive_targets = [
            lin(self.drive_db[0]),
            lin(self.drive_db[1]),
            lin(self.drive_db[2]),
        ];
        if self.fresh {
            self.gain.set_immediate(targets[0]);
            self.ceiling.set_immediate(targets[1]);
            self.output.set_immediate(targets[2]);
            self.mix_sm.set_immediate(targets[3]);
            for (s, t) in self.drives.iter_mut().zip(drive_targets) {
                s.set_immediate(t);
            }
            self.fresh = false;
        }
        let gain = self.gain.advance(targets[0], BATCH);
        let ceiling = self.ceiling.advance(targets[1], BATCH).at(BATCH);
        let output = self.output.advance(targets[2], BATCH);
        let mix = self.mix_sm.advance(targets[3], BATCH);
        let drives = [
            self.drives[0].advance(drive_targets[0], BATCH),
            self.drives[1].advance(drive_targets[1], BATCH),
            self.drives[2].advance(drive_targets[2], BATCH),
        ];
        let b = Batch {
            curve: ClipCurve::new(ceiling, self.curve, self.symmetry),
            ceiling,
            gain,
            drives,
            latency: self.factor().latency_frames() as usize,
            bands: self.bands,
            asymmetric: self.symmetry != 0.0,
            guard: self.guard,
        };

        let mut flags = [false; BATCH];
        let mut outs = [[0.0f32; BATCH]; 2];
        let mut driven_peak = 0.0f32;
        let mut out_peak = 0.0f32;
        for (c, out) in outs.iter_mut().enumerate() {
            let lane = &mut self.lanes[c];
            let r = Self::run_lane(lane, &b, &inputs[c], &mut flags);
            for (n, slot) in out.iter_mut().enumerate() {
                let y = if self.delta {
                    (r.driven[n] - r.wet[n]) * output.at(n)
                } else {
                    let m = mix.at(n);
                    let mixed = if m >= 1.0 {
                        r.wet[n]
                    } else {
                        r.dry[n] * (1.0 - m) + r.wet[n] * m
                    };
                    mixed * output.at(n)
                };
                *slot = y;
                out_peak = out_peak.max(y.abs());
            }
            driven_peak = driven_peak.max(r.driven_peak);
        }
        self.meters[meter::DRIVEN] = driven_peak;
        self.meters[meter::OUTPUT] = out_peak;
        self.record(&flags);
        *ctx.output(OUT_L) = outs[0];
        *ctx.output(OUT_R) = outs[1];
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
        self.factor().latency_frames()
    }

    fn variable_latency(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
