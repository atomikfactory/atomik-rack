//! `atomik.mastering`: multiband compressor, stereo width, true-peak limiter and loudness
//! meter.
//!
//! # Chain
//!
//! ```text
//! in -> input gain -> [3-band multiband compressor] -> stereo width -> true-peak limiter
//!    -> output gain -> out;  loudness / true-peak meter on the output
//! ```
//!
//! * **Crossovers.** Two Linkwitz-Riley 4th-order crossovers (two cascaded Butterworth
//!   2nd-order sections per branch), at `xover.low` and `xover.high`. The effective high
//!   frequency is `max(xover.high, 1.25 * xover.low)` (the low crossover wins, so high > low
//!   always) and is kept below 0.45 sr. The input is split at the low frequency into `L1`/`H1`;
//!   `H1` is split at the high frequency into mid and high; to keep the low band in step, `L1`
//!   goes through an allpass made of the high crossover's own low-pass plus high-pass. The
//!   three bands therefore sum to `AP_high(AP_low(x))`: **the magnitude is exactly flat**
//!   (measured within 0.001 dB with unity band gains and no compression), the **phase is that
//!   of the two cascaded LR4 allpasses** (an impulse is smeared, as with any LR4 crossover).
//! * **Band compressors.** Per band the peak level `max(|L|, |R|)` of the band signal (stereo
//!   linked: both channels get the same gain, so the image is kept) goes through the gain
//!   computer [`atomik_dsp::static_gain_reduction_knee_db`] (soft knee fixed at 6 dB) and the
//!   same ballistics as `atomik.compressor` (release with peak hold, then attack, both one-pole
//!   time constants of the gain reduction in dB). `bandN.on` 0 bypasses the *compression* of the
//!   band (its `bandN.gain` still applies). `multiband.on` 0 bypasses the crossovers and the
//!   band compressors altogether (a pure gain; the filters are idle).
//! * **Width.** Mid/side: `m = (l + r) / 2`, `s = (l - r) / 2 * width`, `l = m + s`,
//!   `r = m - s`. `width` 0 gives exactly mono (`l == r` bit for bit), 1 (100 %) with
//!   `mono_below` 0 skips the matrix, so the signal is **bit-exact**; 2 doubles the side.
//!   `mono_below` > 0 high-passes the side signal with an LR4 high-pass at that frequency (the
//!   side is attenuated 6 dB at the corner, 24 dB one octave below it and 48 dB two octaves
//!   below), keeping the low end mono.
//! * **Soft clip** (`softclip` above 0, between the width and the limiter): `x / sqrt(1 + (x/c)^2)`
//!   with `c = 2^(1 - 2 amount)`, unity for small signals, first-order ADAA; off is bit-exact.
//! * **Split-band width** (`width.split`): mid and side are split at `width.freq` (LR4); the side
//!   bands get `width.lo` and `width.hi` instead of `width`; `width.solo` keeps one band.
//! * **Limiter.** See `limiter.rs`: 4x oversampled true-peak detection, a look-ahead of 0.5, 1.5
//!   (default) or 4 ms (`lookahead`, three limiters built in `prepare`, only the chosen one runs),
//!   `ceiling` in dBTP, `release` (exponential). The `output` gain is applied **after** the
//!   limiter (as the chain says), so the final ceiling is `ceiling + output`; leave `output`
//!   at 0 dB to guarantee the ceiling.
//! * **Loudness meter.** BS.1770-4 / R128, see `loudness.rs`; true peak is the 4x oversampled
//!   maximum (unrefined, as in Annex 2) of the **output**. `meter.reset` is a momentary toggle:
//!   a rising edge (0 to 1) clears the integrated loudness, the windows and the true-peak
//!   maximum (as does `reset()`); the transport does not reset it. Leave it at 0 afterwards.
//! * **Latency** `DETECT_DELAY (17) + round(look-ahead * sr)` samples (89 at 48 kHz for 1.5 ms, 83 at 44.1 kHz,
//!   161 at 96 kHz), constant and set in `prepare`: it is the whole audio path (the oversampler
//!   is only used for detection; the audio never goes through it).
//! * **Meter values** (8): `[momentary LUFS, short-term LUFS, integrated LUFS, true peak dBTP,
//!   GR band 1, GR band 2, GR band 3 (dB, the largest in the batch), limiter GR (dB)]`;
//!   silence reads -120.
//! * **`gr_cv_out`** is `1 - g` per sample with `g` the limiter gain times the smallest band
//!   compression gain.
//! * **Safety.** Non-finite input is treated as 0; filter state is `f64` with denormal flushes.

use crate::bypass::{Bypassable, bypass_param};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{
    Biquad, BiquadCoeffs, BiquadShape, BlockSmoother, exp2_det, exp2_det_f64, log2_det,
    static_gain_reduction_knee_db,
};

pub mod limiter;
pub mod loudness;

#[cfg(test)]
mod tests;

use limiter::Limiter;
use loudness::{FLOOR_DB, LoudnessMeter, PeakDetector};

/// Fixed soft knee of the band compressors in dB.
const BAND_KNEE_DB: f32 = 6.0;

const P_INPUT: u32 = 0;
const P_MB_ON: u32 = 1;
const P_XLOW: u32 = 2;
const P_XHIGH: u32 = 3;
/// First band parameter id; band `k` (0..3) uses `P_BAND0 + 6k + {0..5}` =
/// threshold, ratio, attack, release, gain, on.
const P_BAND0: u32 = 4;
const P_WIDTH: u32 = 22;
const P_MONO_BELOW: u32 = 23;
const P_CEILING: u32 = 24;
const P_RELEASE: u32 = 25;
const P_OUTPUT: u32 = 26;
const P_METER_RESET: u32 = 27;
// The soft clip, look-ahead and split-band width sit before the bypass param (id 35).
const P_SOFTCLIP: u32 = 28;
const P_LOOKAHEAD: u32 = 29;
const P_SPLIT: u32 = 30;
const P_SPLIT_FREQ: u32 = 31;
const P_WIDTH_LO: u32 = 32;
const P_WIDTH_HI: u32 = 33;
const P_SOLO: u32 = 34;

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

const fn band_params(
    k: u32,
    keys: [&'static str; 6],
    names: [&'static str; 6],
    attack: f32,
    release: f32,
) -> [ParamDescriptor; 6] {
    let b = P_BAND0 + 6 * k;
    [
        param(
            b,
            keys[0],
            names[0],
            -60.0,
            0.0,
            -18.0,
            Taper::Linear,
            Unit::Db,
            false,
        ),
        param(
            b + 1,
            keys[1],
            names[1],
            1.0,
            20.0,
            2.0,
            Taper::Exponential,
            Unit::None,
            false,
        ),
        param(
            b + 2,
            keys[2],
            names[2],
            0.5,
            200.0,
            attack,
            Taper::Exponential,
            Unit::Ms,
            false,
        ),
        param(
            b + 3,
            keys[3],
            names[3],
            10.0,
            2_000.0,
            release,
            Taper::Exponential,
            Unit::Ms,
            false,
        ),
        param(
            b + 4,
            keys[4],
            names[4],
            -12.0,
            12.0,
            0.0,
            Taper::Linear,
            Unit::Db,
            false,
        ),
        param(
            b + 5,
            keys[5],
            names[5],
            0.0,
            1.0,
            1.0,
            Taper::Linear,
            Unit::Toggle,
            true,
        ),
    ]
}

const B1: [ParamDescriptor; 6] = band_params(
    0,
    [
        "band1.threshold",
        "band1.ratio",
        "band1.attack",
        "band1.release",
        "band1.gain",
        "band1.on",
    ],
    [
        "Low threshold",
        "Low ratio",
        "Low attack",
        "Low release",
        "Low gain",
        "Low on",
    ],
    30.0,
    300.0,
);
const B2: [ParamDescriptor; 6] = band_params(
    1,
    [
        "band2.threshold",
        "band2.ratio",
        "band2.attack",
        "band2.release",
        "band2.gain",
        "band2.on",
    ],
    [
        "Mid threshold",
        "Mid ratio",
        "Mid attack",
        "Mid release",
        "Mid gain",
        "Mid on",
    ],
    15.0,
    200.0,
);
const B3: [ParamDescriptor; 6] = band_params(
    2,
    [
        "band3.threshold",
        "band3.ratio",
        "band3.attack",
        "band3.release",
        "band3.gain",
        "band3.on",
    ],
    [
        "High threshold",
        "High ratio",
        "High attack",
        "High release",
        "High gain",
        "High on",
    ],
    5.0,
    100.0,
);

static PARAMS: [ParamDescriptor; 36] = [
    param(
        P_INPUT,
        "input",
        "Input gain",
        -24.0,
        24.0,
        0.0,
        Taper::Linear,
        Unit::Db,
        false,
    ),
    param(
        P_MB_ON,
        "multiband.on",
        "Multiband",
        0.0,
        1.0,
        1.0,
        Taper::Linear,
        Unit::Toggle,
        true,
    ),
    param(
        P_XLOW,
        "xover.low",
        "Crossover low",
        40.0,
        2_000.0,
        200.0,
        Taper::Exponential,
        Unit::Hz,
        false,
    ),
    param(
        P_XHIGH,
        "xover.high",
        "Crossover high",
        500.0,
        12_000.0,
        3_000.0,
        Taper::Exponential,
        Unit::Hz,
        false,
    ),
    B1[0],
    B1[1],
    B1[2],
    B1[3],
    B1[4],
    B1[5],
    B2[0],
    B2[1],
    B2[2],
    B2[3],
    B2[4],
    B2[5],
    B3[0],
    B3[1],
    B3[2],
    B3[3],
    B3[4],
    B3[5],
    param(
        P_WIDTH,
        "width",
        "Width",
        0.0,
        2.0,
        1.0,
        Taper::Linear,
        Unit::Percent,
        false,
    ),
    param(
        P_MONO_BELOW,
        "mono_below",
        "Mono below",
        0.0,
        500.0,
        0.0,
        Taper::Linear,
        Unit::Hz,
        false,
    ),
    param(
        P_CEILING,
        "ceiling",
        "Ceiling (dBTP)",
        -12.0,
        0.0,
        -1.0,
        Taper::Linear,
        Unit::Db,
        false,
    ),
    param(
        P_RELEASE,
        "release",
        "Limiter release",
        10.0,
        2_000.0,
        150.0,
        Taper::Exponential,
        Unit::Ms,
        false,
    ),
    param(
        P_OUTPUT,
        "output",
        "Output gain",
        -24.0,
        12.0,
        0.0,
        Taper::Linear,
        Unit::Db,
        false,
    ),
    param(
        P_METER_RESET,
        "meter.reset",
        "Reset meter",
        0.0,
        1.0,
        0.0,
        Taper::Linear,
        Unit::Toggle,
        true,
    ),
    param(
        P_SOFTCLIP,
        "softclip",
        "Soft clip",
        0.0,
        1.0,
        0.0,
        Taper::Linear,
        Unit::Percent,
        false,
    ),
    param(
        P_LOOKAHEAD,
        "lookahead",
        "Look-ahead",
        0.0,
        2.0,
        1.0,
        Taper::Linear,
        Unit::Choice(&["0.5 ms", "1.5 ms", "4 ms"]),
        true,
    ),
    param(
        P_SPLIT,
        "width.split",
        "Split width",
        0.0,
        1.0,
        0.0,
        Taper::Linear,
        Unit::Toggle,
        true,
    ),
    param(
        P_SPLIT_FREQ,
        "width.freq",
        "Width split",
        40.0,
        2_000.0,
        250.0,
        Taper::Exponential,
        Unit::Hz,
        false,
    ),
    param(
        P_WIDTH_LO,
        "width.lo",
        "Low width",
        0.0,
        2.0,
        1.0,
        Taper::Linear,
        Unit::Percent,
        false,
    ),
    param(
        P_WIDTH_HI,
        "width.hi",
        "High width",
        0.0,
        2.0,
        1.0,
        Taper::Linear,
        Unit::Percent,
        false,
    ),
    param(
        P_SOLO,
        "width.solo",
        "Width solo",
        0.0,
        2.0,
        0.0,
        Taper::Linear,
        Unit::Choice(&["off", "low", "high"]),
        true,
    ),
    bypass_param(35),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
const GR_CV: SocketId = SocketId(4);

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
        id: GR_CV,
        key: "gr_cv_out",
        name: "GR CV",
        kind: SocketKind::Cv,
        direction: Direction::Output,
    },
];

/// Static description of the mastering suite.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.mastering",
    name: "Mastering",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Mastering`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(Mastering::new())),
    }
}

/// `10^(x/20) = 2^(x * LOG2_10_20)`.
const LOG2_10_20: f32 = 0.166_096_4;
/// `20 * log10(2)`.
const DB_PER_LOG2: f32 = 6.020_6;

#[inline]
fn level_db(x: f32) -> f32 {
    (DB_PER_LOG2 * log2_det(x)).max(-200.0)
}

fn alpha(ms: f32, sr: f32) -> f64 {
    let n = f64::from(ms) * 0.001 * f64::from(sr);
    if n > 0.0 {
        exp2_det_f64(-std::f64::consts::LOG2_E / n)
    } else {
        0.0
    }
}

#[inline]
fn flush(v: f64) -> f64 {
    if v.abs() < 1e-18 { 0.0 } else { v }
}

/// Linkwitz-Riley 4th order section: two cascaded identical Butterworth biquads.
#[derive(Debug, Clone, Copy, Default)]
struct Lr4 {
    a: Biquad,
    b: Biquad,
}

impl Lr4 {
    #[inline]
    fn process(&mut self, c: &BiquadCoeffs, x: f64) -> f64 {
        let y = self.a.process(c, x);
        self.b.process(c, y)
    }
}

/// The three limiters, one per `lookahead` choice (0.5, 1.5 and 4 ms), built in `prepare`; only
/// the chosen one runs, so a change of the choice is a change of the reported latency and no
/// allocation.
fn limiters_for(sr: f32) -> [Limiter; 3] {
    [0.5, 1.5, 4.0].map(|ms| Limiter::with_lookahead(sr, ms))
}

/// The split-band width filters of the mid or the side signal: a low-pass and a high-pass
/// Linkwitz-Riley section at `width.freq`.
#[derive(Debug, Clone, Copy, Default)]
struct WidthBand {
    lp: Lr4,
    hp: Lr4,
}

/// The soft clip at one sample: `x / sqrt(1 + (x/c)^2)`, first-order antiderivative
/// anti-aliased (`x1` is the previous input; the mean of the curve over the step). Unity gain for
/// small signals, `c` is the ceiling it approaches.
#[inline]
fn soft_clip_adaa(x: f64, x1: f64, c: f64) -> f64 {
    let d = x - x1;
    if d.abs() < 1.0e-3 {
        let m = 0.5 * (x + x1) / c;
        return 0.5 * (x + x1) / (1.0 + m * m).sqrt();
    }
    let big = |v: f64| {
        let t = v / c;
        c * c * ((1.0 + t * t).sqrt() - 1.0)
    };
    (big(x) - big(x1)) / d
}

/// The crossover filters of one channel.
#[derive(Debug, Clone, Copy, Default)]
struct Split {
    lp1: Lr4,
    hp1: Lr4,
    /// High crossover on `H1` (mid and high bands).
    lp2a: Lr4,
    hp2a: Lr4,
    /// High crossover on `L1` (allpass compensation of the low band).
    lp2b: Lr4,
    hp2b: Lr4,
}

/// Band compressor settings.
#[derive(Debug, Clone, Copy)]
struct BandParams {
    threshold: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    gain_db: f32,
    on: bool,
}

/// Band ballistics state (dB).
#[derive(Debug, Clone, Copy, Default)]
struct BandState {
    y1: f64,
    gr: f64,
}

/// The mastering device.
#[derive(Debug, Clone)]
pub struct Mastering {
    sample_rate: f32,
    input_db: f32,
    mb_on: bool,
    xover_low: f32,
    xover_high: f32,
    bands: [BandParams; 3],
    width: f32,
    mono_below: f32,
    ceiling_db: f32,
    release_ms: f32,
    output_db: f32,
    reset_prev: bool,
    reset_pending: bool,
    // Derived.
    c_lp1: BiquadCoeffs,
    c_hp1: BiquadCoeffs,
    c_lp2: BiquadCoeffs,
    c_hp2: BiquadCoeffs,
    c_side: BiquadCoeffs,
    att: [f64; 3],
    rel: [f64; 3],
    rel_limiter: f32,
    // State.
    fresh: bool,
    split: [Split; 2],
    band_state: [BandState; 3],
    side_hp: Lr4,
    input_sm: BlockSmoother,
    output_sm: BlockSmoother,
    width_sm: BlockSmoother,
    band_gain_sm: [BlockSmoother; 3],
    limiters: [Limiter; 3],
    la_idx: usize,
    softclip: f32,
    sc_x1: [f64; 2],
    wsplit_on: bool,
    wsplit_hz: f32,
    width_lo: f32,
    width_hi: f32,
    solo: usize,
    wband: [WidthBand; 2],
    c_wlp: BiquadCoeffs,
    c_whp: BiquadCoeffs,
    meter: LoudnessMeter,
    tp: [PeakDetector; 2],
    tp_max: f32,
    meters: [f32; 8],
}

impl Default for Mastering {
    fn default() -> Self {
        Self::new()
    }
}

impl Mastering {
    /// A mastering device with default params (sized for 48 kHz until `prepare`).
    pub fn new() -> Self {
        let def = |k: usize| {
            let p = &PARAMS[(P_BAND0 as usize) + 6 * k..][..6];
            BandParams {
                threshold: p[0].default,
                ratio: p[1].default,
                attack_ms: p[2].default,
                release_ms: p[3].default,
                gain_db: p[4].default,
                on: true,
            }
        };
        let mut m = Self {
            sample_rate: 48_000.0,
            input_db: 0.0,
            mb_on: true,
            xover_low: 200.0,
            xover_high: 3_000.0,
            bands: [def(0), def(1), def(2)],
            width: 1.0,
            mono_below: 0.0,
            ceiling_db: -1.0,
            release_ms: 150.0,
            output_db: 0.0,
            reset_prev: false,
            reset_pending: false,
            c_lp1: BiquadCoeffs::IDENTITY,
            c_hp1: BiquadCoeffs::IDENTITY,
            c_lp2: BiquadCoeffs::IDENTITY,
            c_hp2: BiquadCoeffs::IDENTITY,
            c_side: BiquadCoeffs::IDENTITY,
            att: [0.0; 3],
            rel: [0.0; 3],
            rel_limiter: 0.0,
            fresh: true,
            split: [Split::default(); 2],
            band_state: [BandState::default(); 3],
            side_hp: Lr4::default(),
            input_sm: BlockSmoother::new(1.0),
            output_sm: BlockSmoother::new(1.0),
            width_sm: BlockSmoother::new(1.0),
            band_gain_sm: [BlockSmoother::new(1.0); 3],
            limiters: limiters_for(48_000.0),
            la_idx: 1,
            softclip: 0.0,
            sc_x1: [0.0; 2],
            wsplit_on: false,
            wsplit_hz: 250.0,
            width_lo: 1.0,
            width_hi: 1.0,
            solo: 0,
            wband: [WidthBand::default(); 2],
            c_wlp: BiquadCoeffs::IDENTITY,
            c_whp: BiquadCoeffs::IDENTITY,
            meter: LoudnessMeter::new(48_000.0),
            tp: [PeakDetector::new(BATCH), PeakDetector::new(BATCH)],
            tp_max: 0.0,
            meters: [FLOOR_DB, FLOOR_DB, FLOOR_DB, FLOOR_DB, 0.0, 0.0, 0.0, 0.0],
        };
        m.derive();
        m
    }

    /// The effective crossover frequencies `(low, high)` in Hz for the current params and
    /// sample rate (`high > low` always).
    pub fn effective_crossovers(&self) -> (f32, f32) {
        let ny = 0.45 * self.sample_rate;
        let low = self.xover_low.min(ny / 1.25);
        let high = self.xover_high.max(low * 1.25).min(ny);
        (low, high)
    }

    fn derive(&mut self) {
        let sr = self.sample_rate;
        let sr64 = f64::from(sr);
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let (low, high) = self.effective_crossovers();
        self.c_lp1 = BiquadCoeffs::design(BiquadShape::Lowpass, f64::from(low), q, 0.0, sr64);
        self.c_hp1 = BiquadCoeffs::design(BiquadShape::Highpass, f64::from(low), q, 0.0, sr64);
        self.c_lp2 = BiquadCoeffs::design(BiquadShape::Lowpass, f64::from(high), q, 0.0, sr64);
        self.c_hp2 = BiquadCoeffs::design(BiquadShape::Highpass, f64::from(high), q, 0.0, sr64);
        self.c_side = BiquadCoeffs::design(
            BiquadShape::Highpass,
            f64::from(self.mono_below.max(10.0)),
            q,
            0.0,
            sr64,
        );
        self.c_wlp = BiquadCoeffs::design(
            BiquadShape::Lowpass,
            f64::from(self.wsplit_hz),
            q,
            0.0,
            sr64,
        );
        self.c_whp = BiquadCoeffs::design(
            BiquadShape::Highpass,
            f64::from(self.wsplit_hz),
            q,
            0.0,
            sr64,
        );
        for k in 0..3 {
            self.att[k] = alpha(self.bands[k].attack_ms, sr);
            self.rel[k] = alpha(self.bands[k].release_ms, sr);
        }
        self.rel_limiter = alpha(self.release_ms, sr) as f32;
    }
}

impl Device for Mastering {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.limiters = limiters_for(self.sample_rate);
        self.meter = LoudnessMeter::new(info.sample_rate.hz());
        let sr = self.sample_rate;
        self.input_sm.set_time(20.0, sr, BATCH);
        self.output_sm.set_time(20.0, sr, BATCH);
        self.width_sm.set_time(20.0, sr, BATCH);
        for s in &mut self.band_gain_sm {
            s.set_time(20.0, sr, BATCH);
        }
        self.derive();
        self.reset();
    }

    fn reset(&mut self) {
        self.split = [Split::default(); 2];
        self.band_state = [BandState::default(); 3];
        self.side_hp = Lr4::default();
        for l in &mut self.limiters {
            l.reset();
        }
        self.sc_x1 = [0.0; 2];
        self.wband = [WidthBand::default(); 2];
        self.meter.reset();
        for t in &mut self.tp {
            t.reset();
        }
        self.tp_max = 0.0;
        self.reset_pending = false;
        self.meters = [FLOOR_DB, FLOOR_DB, FLOOR_DB, FLOOR_DB, 0.0, 0.0, 0.0, 0.0];
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        match id.0 {
            P_INPUT => self.input_db = v.clamp(-24.0, 24.0),
            P_MB_ON => self.mb_on = v >= 0.5,
            P_XLOW => self.xover_low = v.clamp(40.0, 2_000.0),
            P_XHIGH => self.xover_high = v.clamp(500.0, 12_000.0),
            i if (P_BAND0..P_BAND0 + 18).contains(&i) => {
                let k = ((i - P_BAND0) / 6) as usize;
                let b = &mut self.bands[k];
                match (i - P_BAND0) % 6 {
                    0 => b.threshold = v.clamp(-60.0, 0.0),
                    1 => b.ratio = v.clamp(1.0, 20.0),
                    2 => b.attack_ms = v.clamp(0.5, 200.0),
                    3 => b.release_ms = v.clamp(10.0, 2_000.0),
                    4 => b.gain_db = v.clamp(-12.0, 12.0),
                    _ => b.on = v >= 0.5,
                }
            }
            P_SOFTCLIP => self.softclip = v.clamp(0.0, 1.0),
            P_LOOKAHEAD => self.la_idx = (v.round() as usize).min(2),
            P_SPLIT => self.wsplit_on = v >= 0.5,
            P_SPLIT_FREQ => self.wsplit_hz = v.clamp(40.0, 2_000.0),
            P_WIDTH_LO => self.width_lo = v.clamp(0.0, 2.0),
            P_WIDTH_HI => self.width_hi = v.clamp(0.0, 2.0),
            P_SOLO => self.solo = (v.round() as usize).min(2),
            P_WIDTH => self.width = v.clamp(0.0, 2.0),
            P_MONO_BELOW => self.mono_below = v.clamp(0.0, 500.0),
            P_CEILING => self.ceiling_db = v.clamp(-12.0, 0.0),
            P_RELEASE => self.release_ms = v.clamp(10.0, 2_000.0),
            P_OUTPUT => self.output_db = v.clamp(-24.0, 12.0),
            P_METER_RESET => {
                let on = v >= 0.5;
                if on && !self.reset_prev {
                    self.reset_pending = true;
                }
                self.reset_prev = on;
            }
            _ => return,
        }
        self.derive();
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);

        let db_to_lin = |db: f32| exp2_det(db * LOG2_10_20);
        let in_g_t = db_to_lin(self.input_db);
        let out_g_t = db_to_lin(self.output_db);
        let band_g_t = [
            db_to_lin(self.bands[0].gain_db),
            db_to_lin(self.bands[1].gain_db),
            db_to_lin(self.bands[2].gain_db),
        ];
        if self.fresh {
            self.input_sm.set_immediate(in_g_t);
            self.output_sm.set_immediate(out_g_t);
            self.width_sm.set_immediate(self.width);
            for (sm, g) in self.band_gain_sm.iter_mut().zip(band_g_t) {
                sm.set_immediate(g);
            }
            self.fresh = false;
        }
        if self.reset_pending {
            self.reset_pending = false;
            self.meter.reset();
            for t in &mut self.tp {
                t.reset();
            }
            self.tp_max = 0.0;
        }
        let in_ramp = self.input_sm.advance(in_g_t, BATCH);
        let out_ramp = self.output_sm.advance(out_g_t, BATCH);
        let width_ramp = self.width_sm.advance(self.width, BATCH);
        let band_ramp = [
            self.band_gain_sm[0].advance(band_g_t[0], BATCH),
            self.band_gain_sm[1].advance(band_g_t[1], BATCH),
            self.band_gain_sm[2].advance(band_g_t[2], BATCH),
        ];

        let mb_on = self.mb_on;
        let mono_on = self.mono_below > 0.0;
        let one = 1.0f32.to_bits();
        let split_on = self.wsplit_on;
        let width_unity = width_ramp.at(0).to_bits() == one
            && width_ramp.at(BATCH).to_bits() == one
            && !mono_on
            && !split_on;
        if !mono_on {
            self.side_hp = Lr4::default();
        }
        let sc_on = self.softclip > 0.0;
        let sc_c = f64::from(exp2_det(1.0 - 2.0 * self.softclip));
        let (lo_w, hi_w) = (f64::from(self.width_lo), f64::from(self.width_hi));
        let mut pre = [[0.0f32; BATCH]; 2];
        let mut cv_mb = [1.0f32; BATCH];
        let mut gr_band = [0.0f64; 3];
        let finite = |x: f32| if x.is_finite() { x } else { 0.0 };

        for n in 0..BATCH {
            let ig = in_ramp.at(n);
            let xs = [
                f64::from(finite(in_l[n]) * ig),
                f64::from(finite(in_r[n]) * ig),
            ];
            let mut y = xs;
            if mb_on {
                let k = (self.c_lp1, self.c_hp1, self.c_lp2, self.c_hp2);
                let mut bands = [[0.0f64; 2]; 3];
                for c in 0..2 {
                    let s = &mut self.split[c];
                    let l1 = s.lp1.process(&k.0, xs[c]);
                    let h1 = s.hp1.process(&k.1, xs[c]);
                    bands[0][c] = s.lp2b.process(&k.2, l1) + s.hp2b.process(&k.3, l1);
                    bands[1][c] = s.lp2a.process(&k.2, h1);
                    bands[2][c] = s.hp2a.process(&k.3, h1);
                }
                y = [0.0; 2];
                let mut min_gain = 1.0f32;
                for b in 0..3 {
                    let p = self.bands[b];
                    let lvl = bands[b][0].abs().max(bands[b][1].abs()) as f32;
                    let target = if p.on {
                        f64::from(static_gain_reduction_knee_db(
                            level_db(lvl),
                            p.threshold,
                            p.ratio,
                            BAND_KNEE_DB,
                        ))
                    } else {
                        0.0
                    };
                    let st = &mut self.band_state[b];
                    st.y1 = flush(if st.y1 > target {
                        target + self.rel[b] * (st.y1 - target)
                    } else {
                        target
                    });
                    st.gr = flush(if st.y1 > st.gr {
                        st.y1 + self.att[b] * (st.gr - st.y1)
                    } else {
                        st.y1
                    });
                    gr_band[b] = gr_band[b].max(st.gr);
                    let comp = exp2_det(-(st.gr as f32) * LOG2_10_20);
                    min_gain = min_gain.min(comp);
                    let g = f64::from(comp * band_ramp[b].at(n));
                    y[0] += bands[b][0] * g;
                    y[1] += bands[b][1] * g;
                }
                cv_mb[n] = min_gain;
            }
            let mut l = y[0] as f32;
            let mut r = y[1] as f32;
            if split_on {
                // Split-band width: mid and side are each split at `width.freq`; the side's
                // bands take their own width (`width.lo`, `width.hi`, the plain width is not
                // used); a solo keeps one band of both.
                let m = 0.5 * f64::from(l + r);
                let mut s = 0.5 * f64::from(l - r);
                if mono_on {
                    s = self.side_hp.process(&self.c_side, s);
                }
                let (ml, mh) = (
                    self.wband[0].lp.process(&self.c_wlp, m),
                    self.wband[0].hp.process(&self.c_whp, m),
                );
                let (sl, sh) = (
                    self.wband[1].lp.process(&self.c_wlp, s) * lo_w,
                    self.wband[1].hp.process(&self.c_whp, s) * hi_w,
                );
                let (m2, s2) = match self.solo {
                    1 => (ml, sl),
                    2 => (mh, sh),
                    _ => (ml + mh, sl + sh),
                };
                l = (m2 + s2) as f32;
                r = (m2 - s2) as f32;
            } else if !width_unity {
                let w = width_ramp.at(n);
                let m = 0.5 * (l + r);
                let mut s = 0.5 * (l - r);
                if mono_on {
                    s = self.side_hp.process(&self.c_side, f64::from(s)) as f32;
                }
                s *= w;
                l = m + s;
                r = m - s;
            }
            if sc_on {
                let (xl, xr) = (f64::from(l), f64::from(r));
                l = soft_clip_adaa(xl, self.sc_x1[0], sc_c) as f32;
                r = soft_clip_adaa(xr, self.sc_x1[1], sc_c) as f32;
                self.sc_x1 = [xl, xr];
            } else {
                self.sc_x1 = [f64::from(l), f64::from(r)];
            }
            pre[0][n] = l;
            pre[1][n] = r;
        }

        // Limiter, output gain, meters.
        let ceiling = exp2_det(self.ceiling_db * LOG2_10_20);
        let mut lim = [[0.0f32; BATCH]; 2];
        let mut env = [1.0f32; BATCH];
        let [lim_l, lim_r] = &mut lim;
        let lim_gr = self.limiters[self.la_idx].process(
            [&pre[0], &pre[1]],
            ceiling,
            self.rel_limiter,
            [lim_l, lim_r],
            &mut env,
        );
        let mut out = [[0.0f32; BATCH]; 2];
        let mut cv = [0.0f32; BATCH];
        for n in 0..BATCH {
            let g = out_ramp.at(n);
            out[0][n] = lim[0][n] * g;
            out[1][n] = lim[1][n] * g;
            cv[n] = 1.0 - env[n] * cv_mb[n];
        }
        self.meter.process(&out[0], &out[1]);
        let mut pk = [0.0f32; BATCH];
        for (det, o) in self.tp.iter_mut().zip(&out) {
            det.process(o, &mut pk, false);
            for v in &pk {
                self.tp_max = self.tp_max.max(*v);
            }
        }
        let tp_db = if self.tp_max > 1e-6 {
            (DB_PER_LOG2 * log2_det(self.tp_max)).max(FLOOR_DB)
        } else {
            FLOOR_DB
        };
        self.meters = [
            self.meter.momentary(),
            self.meter.short_term(),
            self.meter.integrated(),
            tp_db,
            gr_band[0] as f32,
            gr_band[1] as f32,
            gr_band[2] as f32,
            lim_gr,
        ];
        *ctx.output(OUT_L) = out[0];
        *ctx.output(OUT_R) = out[1];
        *ctx.output(GR_CV) = cv;
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
        self.limiters[self.la_idx].latency()
    }

    fn variable_latency(&self) -> bool {
        true
    }
}
