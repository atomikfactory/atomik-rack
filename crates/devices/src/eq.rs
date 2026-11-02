//! `atomik.eq`: six-band stereo parametric equaliser with an analyser feed.
//!
//! # Bands
//!
//! | Band | Type | Keys | Notes |
//! |---|---|---|---|
//! | 1 | low cut (high-pass) | `band1.on/freq/q`, `lowcut.slope` | 12 / 24 / 48 dB/oct |
//! | 2 | low shelf | `band2.on/freq/gain/q` | RBJ shelf, `q` 0.707 is the cookbook slope S = 1 |
//! | 3 | peak 1 | `band3.on/freq/gain/q` | RBJ peaking bell |
//! | 4 | peak 2 | `band4.on/freq/gain/q` | RBJ peaking bell |
//! | 5 | high shelf | `band5.on/freq/gain/q` | RBJ shelf |
//! | 6 | high cut (low-pass) | `band6.on/freq/q`, `highcut.slope` | 12 / 24 / 48 dB/oct |
//!
//! plus `output` (-24 to +24 dB). The bands run in the order 1..6, both channels identically.
//!
//! * **Filters.** Every section is an [`atomik_dsp::SmoothedBiquad`] (RBJ cookbook, `f64`
//!   coefficients and state, direct form I). Its frequency (log2 Hz), Q (ln Q) and gain (dB) are
//!   smoothed with a 10 ms one-pole and the coefficients are recomputed every 16 samples while a
//!   parameter moves, so sweeps do not zipper (the *parameters* are interpolated, not the
//!   coefficients).
//! * **Cut slopes.** 12 dB/oct is one 2nd-order section (Q = `bandN.q`, default 1/sqrt(2): a
//!   Butterworth, -3.01 dB at the corner). 24 dB/oct is a 4th-order Butterworth made of two
//!   sections with Q = 0.5412 and 1.3066; 48 dB/oct an 8th-order Butterworth of four sections with
//!   Q = 0.5098, 0.6013, 0.8999, 2.5629. `bandN.q` of a cut scales the highest-Q section by
//!   `q / 0.7071`, so the default is the exact Butterworth and larger values add resonance at the
//!   corner (at 12 dB/oct the gain at the corner is `q`). Changing the slope fades the band out
//!   (3 ms time constant), rebuilds its sections from a clean state and fades it back in (10 ms),
//!   so the change does not click (the band dips for roughly 20 ms).
//! * **Off bands are bit-transparent.** Each band has a bypass weight `w` (10 ms one-pole, one
//!   step per batch, ramped linearly across the batch) that is 1 when the band is on and 0
//!   when it is off; a shelf or peak band with a gain of exactly 0 dB counts as off. While `w > 0`
//!   the band runs and its output is `x + w * (y - x)`; once `w` is 0 and its target is 0 the band
//!   is skipped entirely (and its state reset), so a disabled band, and an EQ with every band off
//!   at `output` 0 dB, copy the input bit for bit. Turning a band on or off fades and does not
//!   click; the fade takes about 100 ms to finish (the weight snaps to exactly 0 within 5e-5).
//! * **Gain CV.** Bands 2..5 (low shelf, both peaks, high shelf) have `band2_gain_cv_in` ..
//!   `band5_gain_cv_in`. The CV (batch mean, clamped to -1..+1) times `bandN.gain_cv_trim`
//!   (-1..+1, default +1) times 24 dB is added to `bandN.gain` and the sum clamped to -24..+24 dB,
//!   once per batch; the section's own 10 ms gain smoother removes zipper noise. A band whose
//!   effective gain is exactly 0 dB is off as before. Unconnected: bit-identical.
//! * **Output gain** is smoothed per batch (BlockSmoother) and is exactly 1.0 at 0 dB.
//!
//! # Analyser (meter values)
//!
//! The mono mix `(L + R) / 2` of the *output* feeds a fixed bank of 16 band-pass biquads (RBJ
//! constant 0 dB peak gain) with centres log-spaced from 40 Hz to 16 kHz (ratio 1.4915 per band,
//! Q = 2.485), computed once in `prepare`. The bank runs only while the app has set
//! `internal.analyser_live` (never stored, default off; the rack view turns it on for the panels
//! on screen), so an EQ nobody looks at costs the six bands alone and its band energies read
//! zero; the bank starts from silence each time it is switched on, and the audio output is the
//! same either way. While it runs it works at the full sample rate on the audio
//! thread (16 `f64` biquads per sample, a few million multiply-adds per second at 48 kHz, small
//! next to the six EQ bands, so no decimation or round-robin was needed) and the mean square of
//! every band is taken once per batch and smoothed over batches with a 50 ms one-pole (so low
//! bands do not flicker with the phase of a 64-sample window).
//!
//! `meter_values` layout (18 values): `[0..16)` band energies (mean square of the band-passed
//! signal, linear power; a sine of peak `A` at a band centre reads about `A^2 / 2`), `[16]`
//! output peak L and `[17]` output peak R (linear, last batch). Convert energies to dB with
//! `10 log10`. [`analyser_centres`] returns the centre frequencies.
//!
//! Non-finite input samples are treated as 0. The per-sample path uses only `+ - * /` (the
//! biquad state is `f64`); coefficient design runs at control rate.

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::batch_mean;
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{
    Biquad, BiquadCoeffs, BiquadShape, BlockSmoother, SmoothedBiquad, exp2_det, exp2_det_f64,
};

#[cfg(test)]
mod tests;

/// Number of analyser bands.
pub const ANALYSER_BANDS: usize = 16;
/// Lowest analyser centre in Hz.
pub const ANALYSER_LOW_HZ: f64 = 40.0;
/// Highest analyser centre in Hz.
pub const ANALYSER_HIGH_HZ: f64 = 16_000.0;
/// Number of `meter_values`: 16 energies and the two output peaks.
pub const METER_COUNT: usize = ANALYSER_BANDS + 2;

const FRAC_1_SQRT_2: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// Butterworth section Qs per slope (12, 24, 48 dB/oct), padded with 0.
const BUTTER_Q: [[f64; 4]; 3] = [
    [std::f64::consts::FRAC_1_SQRT_2, 0.0, 0.0, 0.0],
    [0.541_196_1, 1.306_563, 0.0, 0.0],
    [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4],
];
const SLOPE_SECTIONS: [usize; 3] = [1, 2, 4];

const NB: usize = 6;
/// Fade time constant of a band's bypass weight, ms.
const BAND_FADE_MS: f32 = 10.0;
/// Faster fade used while a cut's slope change is pending, ms.
const SLOPE_FADE_MS: f32 = 3.0;

const fn on(id: u32, key: &'static str, name: &'static str, default: f32) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: 0.0,
        max: 1.0,
        default,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    }
}

const fn freq(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min,
        max,
        default,
        taper: Taper::Exponential,
        unit: Unit::Hz,
        discrete: false,
    }
}

const fn gain(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: -24.0,
        max: 24.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Db,
        discrete: false,
    }
}

const fn q(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min,
        max,
        default,
        taper: Taper::Exponential,
        unit: Unit::None,
        discrete: false,
    }
}

const fn slope(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: 0.0,
        max: 2.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Choice(&["12 dB/oct", "24 dB/oct", "48 dB/oct"]),
        discrete: true,
    }
}

/// First param id of every band: cuts have `on, freq, q, slope`, shelves and peaks
/// `on, freq, gain, q`.
const BASE: [u32; NB] = [0, 4, 8, 12, 16, 20];
const P_OUTPUT: u32 = 24;
/// The analyser switch (`internal.analyser_live`), set by the app for the panels on screen.
const P_ANALYSER_LIVE: u32 = 30;
/// First gain-CV trim param (bands 2..5 in order).
const P_GAIN_TRIM: u32 = 25;
/// Range of a band gain in dB, the CV's full-scale swing.
const GAIN_RANGE_DB: f32 = 24.0;

const fn trim(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: -1.0,
        max: 1.0,
        default: 1.0,
        taper: Taper::Linear,
        unit: Unit::Percent,
        discrete: false,
    }
}

static PARAMS: [ParamDescriptor; 31] = [
    on(0, "band1.on", "Low cut on", 0.0),
    freq(1, "band1.freq", "Low cut freq", 20.0, 2_000.0, 80.0),
    q(2, "band1.q", "Low cut Q", 0.3, 4.0, FRAC_1_SQRT_2),
    slope(3, "lowcut.slope", "Low cut slope"),
    on(4, "band2.on", "Low shelf on", 1.0),
    freq(5, "band2.freq", "Low shelf freq", 20.0, 5_000.0, 120.0),
    gain(6, "band2.gain", "Low shelf gain"),
    q(7, "band2.q", "Low shelf Q", 0.3, 2.0, FRAC_1_SQRT_2),
    on(8, "band3.on", "Peak 1 on", 1.0),
    freq(9, "band3.freq", "Peak 1 freq", 20.0, 20_000.0, 500.0),
    gain(10, "band3.gain", "Peak 1 gain"),
    q(11, "band3.q", "Peak 1 Q", 0.1, 18.0, 1.0),
    on(12, "band4.on", "Peak 2 on", 1.0),
    freq(13, "band4.freq", "Peak 2 freq", 20.0, 20_000.0, 2_500.0),
    gain(14, "band4.gain", "Peak 2 gain"),
    q(15, "band4.q", "Peak 2 Q", 0.1, 18.0, 1.0),
    on(16, "band5.on", "High shelf on", 1.0),
    freq(
        17,
        "band5.freq",
        "High shelf freq",
        500.0,
        20_000.0,
        8_000.0,
    ),
    gain(18, "band5.gain", "High shelf gain"),
    q(19, "band5.q", "High shelf Q", 0.3, 2.0, FRAC_1_SQRT_2),
    on(20, "band6.on", "High cut on", 0.0),
    freq(
        21,
        "band6.freq",
        "High cut freq",
        1_000.0,
        20_000.0,
        16_000.0,
    ),
    q(22, "band6.q", "High cut Q", 0.3, 4.0, FRAC_1_SQRT_2),
    slope(23, "highcut.slope", "High cut slope"),
    ParamDescriptor {
        id: ParamId(P_OUTPUT),
        key: "output",
        name: "Output",
        min: -24.0,
        max: 24.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Db,
        discrete: false,
    },
    trim(25, "band2.gain_cv_trim", "Low shelf gain CV trim"),
    trim(26, "band3.gain_cv_trim", "Peak 1 gain CV trim"),
    trim(27, "band4.gain_cv_trim", "Peak 2 gain CV trim"),
    trim(28, "band5.gain_cv_trim", "High shelf gain CV trim"),
    bypass_param(29),
    ParamDescriptor {
        id: ParamId(P_ANALYSER_LIVE),
        key: "internal.analyser_live",
        name: "Analyser live",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    },
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
/// Gain CV inputs of bands 2..5, ids 4..=7.
const GAIN_CV: [SocketId; 4] = [SocketId(4), SocketId(5), SocketId(6), SocketId(7)];

static SOCKETS: [SocketDescriptor; 8] = [
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
        id: GAIN_CV[0],
        key: "band2_gain_cv_in",
        name: "Low shelf gain CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: GAIN_CV[1],
        key: "band3_gain_cv_in",
        name: "Peak 1 gain CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: GAIN_CV[2],
        key: "band4_gain_cv_in",
        name: "Peak 2 gain CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: GAIN_CV[3],
        key: "band5_gain_cv_in",
        name: "High shelf gain CV",
        kind: SocketKind::Cv,
        direction: Direction::Input,
    },
];

/// Static description of the EQ.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.eq",
    name: "EQ",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Equalizer`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(Equalizer::new())),
    }
}

/// Centre frequencies of the analyser bands at `sample_rate` (log-spaced 40 Hz to 16 kHz,
/// capped below 0.45 sr).
pub fn analyser_centres(sample_rate: f64) -> [f64; ANALYSER_BANDS] {
    let log2_span = (ANALYSER_HIGH_HZ / ANALYSER_LOW_HZ).log2();
    std::array::from_fn(|k| {
        let f = ANALYSER_LOW_HZ * exp2_det_f64(log2_span * k as f64 / (ANALYSER_BANDS - 1) as f64);
        f.min(0.45 * sample_rate)
    })
}

/// RBJ band-pass with constant 0 dB peak gain.
fn bandpass(freq: f64, q: f64, sample_rate: f64) -> BiquadCoeffs {
    let w0 = std::f64::consts::TAU * freq / sample_rate;
    let (sn, cs) = w0.sin_cos();
    let alpha = sn / (2.0 * q);
    let a0 = 1.0 + alpha;
    BiquadCoeffs {
        b0: alpha / a0,
        b1: 0.0,
        b2: -alpha / a0,
        a1: -2.0 * cs / a0,
        a2: (1.0 - alpha) / a0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    LowCut,
    LowShelf,
    Peak,
    HighShelf,
    HighCut,
}

const KINDS: [Kind; NB] = [
    Kind::LowCut,
    Kind::LowShelf,
    Kind::Peak,
    Kind::Peak,
    Kind::HighShelf,
    Kind::HighCut,
];

impl Kind {
    fn shape(self) -> BiquadShape {
        match self {
            Self::LowCut => BiquadShape::Highpass,
            Self::LowShelf => BiquadShape::LowShelf,
            Self::Peak => BiquadShape::Peaking,
            Self::HighShelf => BiquadShape::HighShelf,
            Self::HighCut => BiquadShape::Lowpass,
        }
    }

    fn is_cut(self) -> bool {
        matches!(self, Self::LowCut | Self::HighCut)
    }
}

/// One EQ band: its raw params, its sections and its bypass weight.
#[derive(Debug, Clone)]
struct Band {
    kind: Kind,
    on: bool,
    freq: f32,
    gain: f32,
    /// CV offset added to `gain`, in dB (0 unless a gain CV is connected).
    cv_db: f32,
    /// Gain CV amount (-1..+1).
    cv_trim: f32,
    q: f32,
    slope: usize,
    /// The slope the sections are built for (differs from `slope` while a change is pending).
    applied: usize,
    sample_rate: f32,
    /// Bypass weight (1 when on).
    w: BlockSmoother,
    secs: Vec<SmoothedBiquad>,
    /// Number of sections in use (1 for non-cuts).
    nsec: usize,
    /// The sections hold state or coefficients that are out of date (reset when bypassed).
    stale: bool,
}

impl Band {
    fn new(i: usize, sample_rate: f32) -> Self {
        let kind = KINDS[i];
        let table = &PARAMS[BASE[i] as usize..BASE[i] as usize + 4];
        let default = |suffix: &str, fallback: f32| {
            table
                .iter()
                .find(|p| p.key.ends_with(suffix))
                .map_or(fallback, |p| p.default)
        };
        let freq = default(".freq", 1_000.0);
        let q = default(".q", FRAC_1_SQRT_2);
        let gain = default(".gain", 0.0);
        let on = default(".on", 0.0) >= 0.5;
        let secs = (0..4)
            .map(|_| SmoothedBiquad::new(kind.shape(), freq, q, gain, sample_rate))
            .collect();
        let mut b = Self {
            kind,
            on,
            freq,
            gain,
            cv_db: 0.0,
            cv_trim: 1.0,
            q,
            slope: 0,
            applied: 0,
            sample_rate,
            w: BlockSmoother::new(0.0),
            secs,
            nsec: 1,
            stale: true,
        };
        b.w.set_time(BAND_FADE_MS, sample_rate, BATCH);
        b.apply();
        b
    }

    /// True when the band should currently be audible.
    fn wants_on(&self) -> bool {
        self.on && (self.kind.is_cut() || self.eff_gain() != 0.0)
    }

    /// Knob gain plus the CV offset, clamped to the parameter range.
    fn eff_gain(&self) -> f32 {
        (self.gain + self.cv_db).clamp(-GAIN_RANGE_DB, GAIN_RANGE_DB)
    }

    /// Pushes the raw params into the sections' smoothed targets.
    fn apply(&mut self) {
        if self.kind.is_cut() {
            let n = SLOPE_SECTIONS[self.applied];
            let qs = f64::from(self.q) / std::f64::consts::FRAC_1_SQRT_2;
            for (i, (sec, bq)) in self
                .secs
                .iter_mut()
                .zip(BUTTER_Q[self.applied])
                .enumerate()
                .take(n)
            {
                let qq = if i + 1 == n { bq * qs } else { bq };
                sec.set_freq(self.freq);
                sec.set_q(qq as f32);
            }
            for i in self.nsec..n {
                self.secs[i].reset(); // newly active sections start clean
            }
            self.nsec = n;
        } else {
            self.secs[0].set_freq(self.freq);
            self.secs[0].set_q(self.q);
            let g = self.eff_gain();
            self.secs[0].set_gain_db(g);
        }
        self.stale = true;
    }

    /// Runs the band over one batch in place.
    fn process(&mut self, l: &mut [f32; BATCH], r: &mut [f32; BATCH]) {
        let mut pending = self.kind.is_cut() && self.slope != self.applied;
        if pending && self.w.value() < 0.01 {
            // Faded out: rebuild the sections for the new slope from a clean state and fade in.
            self.applied = self.slope;
            self.apply();
            for s in &mut self.secs {
                s.reset();
            }
            self.w.set_immediate(0.0);
            self.stale = false;
            pending = false;
        }
        let tau = if pending { SLOPE_FADE_MS } else { BAND_FADE_MS };
        self.w.set_time(tau, self.sample_rate, BATCH);
        let target = if self.wants_on() && !pending {
            1.0
        } else {
            0.0
        };
        let prev = self.w.value();
        let ramp = self.w.advance(target, BATCH);
        if prev.to_bits() == 0 && self.w.value().to_bits() == 0 {
            if self.stale {
                for s in &mut self.secs {
                    s.reset();
                }
                self.stale = false;
            }
            return;
        }
        let one = 1.0f32.to_bits();
        let full = prev.to_bits() == one && self.w.value().to_bits() == one;
        self.stale = true;
        for n in 0..BATCH {
            let (xl, xr) = (l[n], r[n]);
            let (mut yl, mut yr) = (xl, xr);
            for s in &mut self.secs[..self.nsec] {
                (yl, yr) = s.process(yl, yr);
            }
            if full {
                l[n] = yl;
                r[n] = yr;
            } else {
                let w = ramp.at(n);
                l[n] = xl + w * (yl - xl);
                r[n] = xr + w * (yr - xr);
            }
        }
    }
}

/// The parametric EQ device.
#[derive(Debug, Clone)]
pub struct Equalizer {
    sample_rate: f32,
    bands: Vec<Band>,
    output_db: f32,
    out_sm: BlockSmoother,
    fresh: bool,
    bank: [Biquad; ANALYSER_BANDS],
    bank_coeffs: [BiquadCoeffs; ANALYSER_BANDS],
    energy: [f64; ANALYSER_BANDS],
    energy_alpha: f64,
    peaks: [f32; 2],
    /// The analyser bank runs (a panel with the display is on screen).
    analyser: bool,
}

impl Default for Equalizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Equalizer {
    /// An EQ with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let mut e = Self {
            sample_rate: 48_000.0,
            bands: Vec::new(),
            output_db: 0.0,
            out_sm: BlockSmoother::new(1.0),
            fresh: true,
            bank: [Biquad::default(); ANALYSER_BANDS],
            bank_coeffs: [BiquadCoeffs::IDENTITY; ANALYSER_BANDS],
            energy: [0.0; ANALYSER_BANDS],
            energy_alpha: 1.0,
            peaks: [0.0; 2],
            analyser: false,
        };
        e.setup(48_000.0);
        e.reset();
        e
    }

    /// (Re)builds every sample-rate dependent part; keeps the raw params.
    fn setup(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        let old: Vec<Band> = std::mem::take(&mut self.bands);
        self.bands = (0..NB)
            .map(|i| {
                let mut b = Band::new(i, sample_rate);
                if let Some(o) = old.get(i) {
                    b.on = o.on;
                    b.freq = o.freq;
                    b.gain = o.gain;
                    b.q = o.q;
                    b.slope = o.slope;
                    b.applied = o.slope;
                    b.apply();
                }
                b
            })
            .collect();
        self.out_sm.set_time(20.0, sample_rate, BATCH);
        let sr = f64::from(sample_rate);
        let centres = analyser_centres(sr);
        let ratio = (ANALYSER_HIGH_HZ / ANALYSER_LOW_HZ).powf(1.0 / (ANALYSER_BANDS - 1) as f64);
        let qb = ratio.sqrt() / (ratio - 1.0);
        for (c, f) in self.bank_coeffs.iter_mut().zip(centres) {
            *c = bandpass(f, qb, sr);
        }
        self.energy_alpha =
            1.0 - exp2_det_f64(-std::f64::consts::LOG2_E * BATCH as f64 / (0.05 * sr));
    }
}

impl Device for Equalizer {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.setup(info.sample_rate.hz() as f32);
        self.reset();
    }

    fn reset(&mut self) {
        for b in &mut self.bands {
            b.applied = b.slope;
            b.apply();
            for s in &mut b.secs {
                s.reset();
            }
            b.w.set_immediate(if b.wants_on() { 1.0 } else { 0.0 });
            b.stale = false;
        }
        for f in &mut self.bank {
            f.reset();
        }
        self.energy = [0.0; ANALYSER_BANDS];
        self.peaks = [0.0; 2];
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        let Some(p) = PARAMS.get(id.0 as usize) else {
            return;
        };
        let v = v.clamp(p.min, p.max);
        if id.0 == P_OUTPUT {
            self.output_db = v;
            return;
        }
        if id.0 == P_ANALYSER_LIVE {
            let on = v >= 0.5;
            if on && !self.analyser {
                // The bank was idle: it starts from silence, not from an old tail.
                for f in &mut self.bank {
                    f.reset();
                }
                self.energy = [0.0; ANALYSER_BANDS];
            }
            self.analyser = on;
            return;
        }
        if id.0 >= P_GAIN_TRIM {
            self.bands[(id.0 - P_GAIN_TRIM) as usize + 1].cv_trim = v;
            return;
        }
        let Some(i) = BASE.iter().rposition(|b| *b <= id.0) else {
            return;
        };
        let off = id.0 - BASE[i];
        let band = &mut self.bands[i];
        let cut = band.kind.is_cut();
        match (off, cut) {
            (0, _) => band.on = v >= 0.5,
            (1, _) => band.freq = v,
            (2, true) => band.q = v,
            (2, false) => band.gain = v,
            (3, true) => {
                band.slope = (v.round() as usize).min(2);
                return; // applied by `process` after a fade-out
            }
            (3, false) => band.q = v,
            _ => return,
        }
        band.apply();
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
        let mut l = *ctx.input(IN_L);
        let mut r = *ctx.input(IN_R);
        for n in 0..BATCH {
            l[n] = finite(l[n]);
            r[n] = finite(r[n]);
        }
        for (k, sock) in GAIN_CV.iter().enumerate() {
            let band = &mut self.bands[k + 1];
            let db = if ctx.is_connected(*sock) {
                band.cv_trim * GAIN_RANGE_DB * batch_mean(ctx.input(*sock))
            } else {
                0.0
            };
            if db.to_bits() != band.cv_db.to_bits() {
                band.cv_db = db;
                band.apply();
            }
        }
        for b in &mut self.bands {
            b.process(&mut l, &mut r);
        }
        let target = exp2_det(self.output_db * (std::f32::consts::LOG2_10 / 20.0));
        if self.fresh {
            self.out_sm.set_immediate(target);
            self.fresh = false;
        }
        let g = self.out_sm.advance(target, BATCH);
        let mut sq = [0.0f64; ANALYSER_BANDS];
        let mut peaks = [0.0f32; 2];
        for n in 0..BATCH {
            let gn = g.at(n);
            let (yl, yr) = (l[n] * gn, r[n] * gn);
            l[n] = yl;
            r[n] = yr;
            peaks[0] = peaks[0].max(yl.abs());
            peaks[1] = peaks[1].max(yr.abs());
            if self.analyser {
                let mono = 0.5 * (f64::from(yl) + f64::from(yr));
                for ((f, c), acc) in self.bank.iter_mut().zip(&self.bank_coeffs).zip(&mut sq) {
                    let y = f.process(c, mono);
                    *acc += y * y;
                }
            }
        }
        if self.analyser {
            for (en, s) in self.energy.iter_mut().zip(sq) {
                let e = s / BATCH as f64;
                let v = *en + self.energy_alpha * (e - *en);
                *en = if v < 1e-30 { 0.0 } else { v };
            }
        }
        self.peaks = peaks;
        *ctx.output(OUT_L) = l;
        *ctx.output(OUT_R) = r;
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let mut values = [0.0f32; METER_COUNT];
        for (v, e) in values.iter_mut().zip(&self.energy) {
            *v = *e as f32;
        }
        values[ANALYSER_BANDS] = self.peaks[0];
        values[ANALYSER_BANDS + 1] = self.peaks[1];
        let n = values.len().min(out.len());
        out[..n].copy_from_slice(&values[..n]);
        n
    }
}
