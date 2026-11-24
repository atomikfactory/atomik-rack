//! `atomik.eq`: seven-band stereo parametric equaliser with an analyser feed.
//!
//! # Bands
//!
//! The UI shows the bands in the order LCF, 1, 2, 3, 4, 5, HCF. The keys keep the numbers they
//! had before the upgrade, so band 7 (new) sits between `band4` and `band5` on screen:
//!
//! | Band | UI | Type | Keys | Notes |
//! |---|---|---|---|---|
//! | 1 | LCF | low cut (high-pass) | `band1.on/freq/q`, `lowcut.slope` | 6 / 12 / 24 / 36 / 48 dB/oct |
//! | 2 | 1 | middle (default low shelf) | `band2.on/freq/gain/q/shape` | RBJ shelf, `q` 0.707 is the cookbook slope S = 1 |
//! | 3 | 2 | middle (default bell) | `band3.on/freq/gain/q/shape` | RBJ peaking bell |
//! | 4 | 3 | middle (default bell) | `band4.on/freq/gain/q/shape` | RBJ peaking bell |
//! | 7 | 4 | middle (default bell, **new**) | `band7.on/freq/gain/q/shape` | gain 0 dB by default: off |
//! | 5 | 5 | middle (default high shelf) | `band5.on/freq/gain/q/shape` | RBJ shelf |
//! | 6 | HCF | high cut (low-pass) | `band6.on/freq/q`, `highcut.slope` | 6 / 12 / 24 / 36 / 48 dB/oct |
//!
//! plus `output` (-24 to +24 dB), `stereo.mode`, and two display settings the device stores and
//! does not use (`spectrum.on`, `display.range`). The bands run in the order 1, 2, 3, 4, 7, 5, 6
//! (the sections are linear, so the order only matters for rounding).
//!
//! * **Filters.** Every section is an [`atomik_dsp::SmoothedBiquad`] (RBJ cookbook, `f64`
//!   coefficients and state, direct form I). Its frequency (log2 Hz), Q (ln Q) and gain (dB) are
//!   smoothed with a 10 ms one-pole and the coefficients are recomputed every 16 samples while a
//!   parameter moves, so sweeps do not zipper (the *parameters* are interpolated, not the
//!   coefficients).
//! * **Shapes.** Each middle band has a `bandN.shape` choice: 0 Bell, 1 Low shelf, 2 High shelf,
//!   3 Notch ([`SHAPE_NAMES`]). The defaults are the old fixed types (band2 low shelf, band5
//!   high shelf, the rest bells). A Bell or shelf with a gain of exactly 0 dB counts as off; a
//!   Notch ignores the gain and is active whenever the band is on. Shelves clamp their effective Q
//!   to 0.3..2 in the DSP, bells and notches use 0.1..18 (the `q` ranges of band2 and band5 were
//!   widened from 0.3..2 to 0.1..18 so they can be bells; a shelf never sees more than 2).
//!   Changing the shape fades the band out (3 ms), rebuilds its section from a clean state and
//!   fades it back in (10 ms), like a slope change, so it does not click.
//! * **Cut slopes.** `*.slope` is 0 = 12, 1 = 24, 2 = 48 dB/oct (the original values) and the
//!   appended 3 = 6 dB/oct, 4 = 36 dB/oct; [`SLOPE_ORDER`] lists them in ascending steepness for
//!   the UI. 12 dB/oct is one 2nd-order section (Q = `bandN.q`, default 1/sqrt(2): a Butterworth,
//!   -3.01 dB at the corner). 24 dB/oct is a 4th-order Butterworth made of two sections with
//!   Q = 0.5412 and 1.3066; 36 dB/oct a 6th-order one of three (Q = 0.5176, 0.7071, 1.9319); 48
//!   dB/oct an 8th-order one of four (Q = 0.5098, 0.6013, 0.8999, 2.5629); 6 dB/oct is one
//!   first-order section (-3.01 dB at the corner, `q` ignored). `bandN.q` of a cut is its
//!   resonance: it scales the highest-Q section by `q / 0.7071`, so the default is the exact
//!   Butterworth and larger values add resonance at the corner (at 12 dB/oct the gain at the
//!   corner is `q`). Changing the slope fades the band out (3 ms time constant), rebuilds its
//!   sections from a clean state and fades it back in (10 ms), so the change does not click (the
//!   band dips for roughly 20 ms).
//! * **Off bands are bit-transparent.** Each band has a bypass weight `w` (10 ms one-pole, one
//!   step per batch, ramped linearly across the batch) that is 1 when the band is on and 0
//!   when it is off; a bell or shelf band with a gain of exactly 0 dB counts as off. While `w > 0`
//!   the band runs and its output is `x + w * (y - x)`; once `w` is 0 and its target is 0 the band
//!   is skipped entirely (and its state reset), so a disabled band, and an EQ with every band off
//!   at `output` 0 dB, copy the input bit for bit. Turning a band on or off fades and does not
//!   click; the fade takes about 100 ms to finish (the weight snaps to exactly 0 within 5e-5).
//! * **Stereo mode.** `stereo.mode` ([`STEREO_MODE_NAMES`]): 0 Stereo (L and R through the bands,
//!   the original path, bit-identical), 1 Mid (the bands process `M = (L + R) / 2`, `S = (L - R)
//!   / 2` passes, `L = M' + S`, `R = M' - S`), 2 Side (the bands process `S`), 3 Mid solo (the
//!   EQ'd `M` on both outputs), 4 Side solo (the EQ'd `S` on both outputs). Modes 1..4 run the
//!   bands on the one signal (twice the work of a channel). Switching the mode fades the output
//!   out over one batch (1.3 ms), clears every band's filter state and fades back in over the
//!   next batch, so there is neither a click nor a burst from state that belonged to the other
//!   signal. The first batch after `prepare` or `reset` takes the mode at once.
//! * **CV.** Every band has a gain CV (the middle bands: `band2_gain_cv_in` .. `band5_gain_cv_in`,
//!   `band7_gain_cv_in`), a frequency CV (`bandN_freq_cv_in`) and a Q CV (`bandN_q_cv_in`, the
//!   resonance of a cut), N = 1..7. Each CV is the batch mean clamped to -1..+1, taken once per
//!   batch, times its trim (`bandN.gain_cv_trim`, `bandN.freq_cv_trim`, `bandN.q_cv_trim`:
//!   -1..+1, default +1): gain adds `cv * trim * 24` dB (the sum clamped to -24..+24 dB), the
//!   frequency is multiplied by `2^(cv * trim * 4)` (4 octaves at full scale, clamped to the
//!   range of the frequency param), Q by `2^(cv * trim * 2)` (clamped to the range of the shape).
//!   The section's own 10 ms smoothers remove zipper noise. A band whose effective gain is exactly
//!   0 dB is off as before. Unconnected: bit-identical.
//! * **Output gain** is smoothed per batch (BlockSmoother) and is exactly 1.0 at 0 dB.
//! * **Spectrum tap.** The device owns a [`ScopeTap`] (it hands it out through
//!   [`Device::scope_tap`]) and pushes the post-EQ output (after the output gain) of every batch
//!   while the tap's live flag is set ([`ScopeTap::set_live`], the same switch the console strips
//!   use for their meter panels; the app sets it while a panel with the display is on screen,
//!   which also makes the tap's ring: an EQ nobody looks at holds none). Not live: one relaxed
//!   load per batch, nothing written, and the audio is the same either way.
//!   The ring is the default one (32768 frames, reads up to 16384); the app can ask for the large
//!   ring with [`ScopeTap::reserve_large`] for longer windows. `spectrum.on` and `display.range`
//!   ([`DISPLAY_RANGE_DB`]) are display settings of the panel: stored with the project, unused
//!   here.
//!
//! # Analyser (meter values)
//!
//! The mono mix `(L + R) / 2` of the *output* feeds a fixed bank of 16 band-pass biquads (RBJ
//! constant 0 dB peak gain) with centres log-spaced from 40 Hz to 16 kHz (ratio 1.4915 per band,
//! Q = 2.485), computed once in `prepare`. The bank runs only while the app has set
//! `internal.analyser_live` (never stored, default off; the app no longer sets it for the EQ, whose panel
//! reads only the output peaks, values 16 and 17), so the EQ costs the bands alone and its band
//! energies read zero unless a host sets it; the bank starts from silence each time it is switched on, and the audio output is the
//! same either way. While it runs it works at the full sample rate on the audio
//! thread (16 `f64` biquads per sample, a few million multiply-adds per second at 48 kHz, small
//! next to the EQ bands, so no decimation or round-robin was needed) and the mean square of
//! every band is taken once per batch and smoothed over batches with a 50 ms one-pole (so low
//! bands do not flicker with the phase of a 64-sample window).
//!
//! `meter_values` layout (18 values): `[0..16)` band energies (mean square of the band-passed
//! signal, linear power; a sine of peak `A` at a band centre reads about `A^2 / 2`), `[16]`
//! output peak L and `[17]` output peak R (linear, last batch). Convert energies to dB with
//! `10 log10`. [`analyser_centres`] returns the centre frequencies.
//!
//! # Drawing the curve
//!
//! [`middle_response_db`] and [`cut_response_db`] return the exact magnitude response of one
//! band for any shape and slope (the coefficients the DSP designs), [`middle_active`] says
//! whether a middle band is audible at a gain, and [`effective_q`] is the Q the DSP uses.
//!
//! Non-finite input samples are treated as 0. The per-sample path uses only `+ - * /` (the
//! biquad state is `f64`); coefficient design runs at control rate.

use crate::bypass::{Bypassable, bypass_param};
use crate::cv_in::batch_mean;
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    ScopeTap, SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::{
    Biquad, BiquadCoeffs, BiquadShape, BlockSmoother, SmoothedBiquad, exp2_det, exp2_det_f64,
};
use std::sync::Arc;

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

/// Names of the `bandN.shape` choice (0 Bell, 1 Low shelf, 2 High shelf, 3 Notch).
pub const SHAPE_NAMES: [&str; 4] = ["Bell", "Low shelf", "High shelf", "Notch"];
/// Names of the cut `slope` choice, indexed by the param value (the original three first, the
/// appended 6 and 36 dB/oct last).
pub const SLOPE_NAMES: [&str; 5] = [
    "12 dB/oct",
    "24 dB/oct",
    "48 dB/oct",
    "6 dB/oct",
    "36 dB/oct",
];
/// The slope param values in ascending steepness (6, 12, 24, 36, 48 dB/oct), for the UI.
pub const SLOPE_ORDER: [usize; 5] = [3, 0, 1, 4, 2];
/// Names of the `stereo.mode` choice.
pub const STEREO_MODE_NAMES: [&str; 5] = ["Stereo", "Mid", "Side", "Mid solo", "Side solo"];
/// The vertical range in dB (plus and minus) of each `display.range` choice.
pub const DISPLAY_RANGE_DB: [f64; 3] = [6.0, 12.0, 24.0];

/// Param value of the 6 dB/oct slope.
const SLOPE_6: usize = 3;
/// Highest slope value.
const SLOPE_MAX: usize = 4;
/// Butterworth section Qs per slope value (12, 24, 48, 6 (unused), 36 dB/oct), padded with 0.
const BUTTER_Q: [[f64; 4]; 5] = [
    [std::f64::consts::FRAC_1_SQRT_2, 0.0, 0.0, 0.0],
    [0.541_196_1, 1.306_563, 0.0, 0.0],
    [0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4],
    [std::f64::consts::FRAC_1_SQRT_2, 0.0, 0.0, 0.0],
    [
        0.517_638_1,
        std::f64::consts::FRAC_1_SQRT_2,
        1.931_851_7,
        0.0,
    ],
];
const SLOPE_SECTIONS: [usize; 5] = [1, 2, 4, 1, 3];

/// Number of bands (band 7 is the last, its index 6).
const NB: usize = 7;
/// Fade time constant of a band's bypass weight, ms.
const BAND_FADE_MS: f32 = 10.0;
/// Faster fade used while a slope or shape change is pending, ms.
const SLOPE_FADE_MS: f32 = 3.0;

const SHAPE_NOTCH: usize = 3;
/// Q range of a shelf in the DSP.
const SHELF_Q: (f64, f64) = (0.3, 2.0);
/// Q range of a bell or notch.
const BELL_Q: (f64, f64) = (0.1, 18.0);
/// Octaves of frequency a full-scale CV moves a band.
const FREQ_CV_OCT: f32 = 4.0;
/// Octaves of Q (powers of two) a full-scale CV moves a band.
const Q_CV_OCT: f32 = 2.0;

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

const fn choice(
    id: u32,
    key: &'static str,
    name: &'static str,
    max: f32,
    default: f32,
    names: &'static [&'static str],
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: 0.0,
        max,
        default,
        taper: Taper::Linear,
        unit: Unit::Choice(names),
        discrete: true,
    }
}

const fn slope(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    choice(id, key, name, SLOPE_MAX as f32, 0.0, &SLOPE_NAMES)
}

const fn shape(id: u32, key: &'static str, name: &'static str, default: f32) -> ParamDescriptor {
    choice(id, key, name, 3.0, default, &SHAPE_NAMES)
}

/// First param id of every band: cuts have `on, freq, q, slope`, middle bands
/// `on, freq, gain, q`. Band 7 (index 6) comes after the original table.
const BASE: [u32; NB] = [0, 4, 8, 12, 16, 20, 31];
const P_OUTPUT: u32 = 24;
/// First gain-CV trim param (bands 2..5 in order).
const P_GAIN_TRIM: u32 = 25;
/// The analyser switch (`internal.analyser_live`), not set by the app for the EQ (its panel reads only the peaks).
const P_ANALYSER_LIVE: u32 = 30;
/// The band 7 params `on, freq, gain, q` are 31..=34.
const P_BAND7: u32 = 31;
/// First shape param (`band2.shape`, `band3.shape`, `band4.shape`, `band5.shape`,
/// `band7.shape`).
const P_SHAPE: u32 = 35;
const P_BAND7_GAIN_TRIM: u32 = 40;
const P_STEREO_MODE: u32 = 41;
const P_SPECTRUM_ON: u32 = 42;
const P_DISPLAY_RANGE: u32 = 43;
/// First frequency CV trim (bands 1..7 in order).
const P_FREQ_TRIM: u32 = 44;
/// First Q CV trim (bands 1..7 in order).
const P_Q_TRIM: u32 = 51;
/// Band index of each shape param.
const SHAPE_BANDS: [usize; 5] = [1, 2, 3, 4, 6];
/// The shape each band starts with (the old fixed types).
const DEFAULT_SHAPE: [usize; NB] = [0, 1, 0, 0, 2, 0, 0];
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

static PARAMS: [ParamDescriptor; 58] = [
    on(0, "band1.on", "Low cut on", 0.0),
    freq(1, "band1.freq", "Low cut freq", 20.0, 2_000.0, 80.0),
    q(
        2,
        "band1.q",
        "Low cut Q",
        0.3,
        4.0,
        std::f32::consts::FRAC_1_SQRT_2,
    ),
    slope(3, "lowcut.slope", "Low cut slope"),
    on(4, "band2.on", "Band 1 on", 1.0),
    freq(5, "band2.freq", "Band 1 freq", 20.0, 5_000.0, 120.0),
    gain(6, "band2.gain", "Band 1 gain"),
    q(
        7,
        "band2.q",
        "Band 1 Q",
        0.1,
        18.0,
        std::f32::consts::FRAC_1_SQRT_2,
    ),
    on(8, "band3.on", "Band 2 on", 1.0),
    freq(9, "band3.freq", "Band 2 freq", 20.0, 20_000.0, 500.0),
    gain(10, "band3.gain", "Band 2 gain"),
    q(11, "band3.q", "Band 2 Q", 0.1, 18.0, 1.0),
    on(12, "band4.on", "Band 3 on", 1.0),
    freq(13, "band4.freq", "Band 3 freq", 20.0, 20_000.0, 2_500.0),
    gain(14, "band4.gain", "Band 3 gain"),
    q(15, "band4.q", "Band 3 Q", 0.1, 18.0, 1.0),
    on(16, "band5.on", "Band 5 on", 1.0),
    freq(17, "band5.freq", "Band 5 freq", 500.0, 20_000.0, 8_000.0),
    gain(18, "band5.gain", "Band 5 gain"),
    q(
        19,
        "band5.q",
        "Band 5 Q",
        0.1,
        18.0,
        std::f32::consts::FRAC_1_SQRT_2,
    ),
    on(20, "band6.on", "High cut on", 0.0),
    freq(
        21,
        "band6.freq",
        "High cut freq",
        1_000.0,
        20_000.0,
        16_000.0,
    ),
    q(
        22,
        "band6.q",
        "High cut Q",
        0.3,
        4.0,
        std::f32::consts::FRAC_1_SQRT_2,
    ),
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
    trim(25, "band2.gain_cv_trim", "Band 1 gain CV trim"),
    trim(26, "band3.gain_cv_trim", "Band 2 gain CV trim"),
    trim(27, "band4.gain_cv_trim", "Band 3 gain CV trim"),
    trim(28, "band5.gain_cv_trim", "Band 5 gain CV trim"),
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
    on(31, "band7.on", "Band 4 on", 1.0),
    freq(32, "band7.freq", "Band 4 freq", 20.0, 20_000.0, 6_000.0),
    gain(33, "band7.gain", "Band 4 gain"),
    q(34, "band7.q", "Band 4 Q", 0.1, 18.0, 1.0),
    shape(35, "band2.shape", "Band 1 shape", 1.0),
    shape(36, "band3.shape", "Band 2 shape", 0.0),
    shape(37, "band4.shape", "Band 3 shape", 0.0),
    shape(38, "band5.shape", "Band 5 shape", 2.0),
    shape(39, "band7.shape", "Band 4 shape", 0.0),
    trim(40, "band7.gain_cv_trim", "Band 4 gain CV trim"),
    choice(
        41,
        "stereo.mode",
        "Stereo mode",
        4.0,
        0.0,
        &STEREO_MODE_NAMES,
    ),
    on(42, "spectrum.on", "Spectrum", 1.0),
    choice(
        43,
        "display.range",
        "Display range",
        2.0,
        1.0,
        &["+-6 dB", "+-12 dB", "+-24 dB"],
    ),
    trim(44, "band1.freq_cv_trim", "Low cut freq CV trim"),
    trim(45, "band2.freq_cv_trim", "Band 1 freq CV trim"),
    trim(46, "band3.freq_cv_trim", "Band 2 freq CV trim"),
    trim(47, "band4.freq_cv_trim", "Band 3 freq CV trim"),
    trim(48, "band5.freq_cv_trim", "Band 5 freq CV trim"),
    trim(49, "band6.freq_cv_trim", "High cut freq CV trim"),
    trim(50, "band7.freq_cv_trim", "Band 4 freq CV trim"),
    trim(51, "band1.q_cv_trim", "Low cut Q CV trim"),
    trim(52, "band2.q_cv_trim", "Band 1 Q CV trim"),
    trim(53, "band3.q_cv_trim", "Band 2 Q CV trim"),
    trim(54, "band4.q_cv_trim", "Band 3 Q CV trim"),
    trim(55, "band5.q_cv_trim", "Band 5 Q CV trim"),
    trim(56, "band6.q_cv_trim", "High cut Q CV trim"),
    trim(57, "band7.q_cv_trim", "Band 4 Q CV trim"),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
/// Gain CV input of each band (ids 4..=7 for bands 2..5, 8 for band 7; the cuts have none).
const GAIN_CV: [Option<SocketId>; NB] = [
    None,
    Some(SocketId(4)),
    Some(SocketId(5)),
    Some(SocketId(6)),
    Some(SocketId(7)),
    None,
    Some(SocketId(8)),
];
/// Frequency CV input of band `i` (bands 1..7): ids 9..=15.
const fn freq_cv(i: usize) -> SocketId {
    SocketId(9 + i as u16)
}
/// Q CV input of band `i` (bands 1..7): ids 16..=22.
const fn q_cv(i: usize) -> SocketId {
    SocketId(16 + i as u16)
}

const fn cv_in(id: u16, key: &'static str, name: &'static str) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name,
        kind: SocketKind::Cv,
        direction: Direction::Input,
    }
}

static SOCKETS: [SocketDescriptor; 23] = [
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
    cv_in(4, "band2_gain_cv_in", "Band 1 gain CV"),
    cv_in(5, "band3_gain_cv_in", "Band 2 gain CV"),
    cv_in(6, "band4_gain_cv_in", "Band 3 gain CV"),
    cv_in(7, "band5_gain_cv_in", "Band 5 gain CV"),
    cv_in(8, "band7_gain_cv_in", "Band 4 gain CV"),
    cv_in(9, "band1_freq_cv_in", "Low cut freq CV"),
    cv_in(10, "band2_freq_cv_in", "Band 1 freq CV"),
    cv_in(11, "band3_freq_cv_in", "Band 2 freq CV"),
    cv_in(12, "band4_freq_cv_in", "Band 3 freq CV"),
    cv_in(13, "band5_freq_cv_in", "Band 5 freq CV"),
    cv_in(14, "band6_freq_cv_in", "High cut freq CV"),
    cv_in(15, "band7_freq_cv_in", "Band 4 freq CV"),
    cv_in(16, "band1_q_cv_in", "Low cut Q CV"),
    cv_in(17, "band2_q_cv_in", "Band 1 Q CV"),
    cv_in(18, "band3_q_cv_in", "Band 2 Q CV"),
    cv_in(19, "band4_q_cv_in", "Band 3 Q CV"),
    cv_in(20, "band5_q_cv_in", "Band 5 Q CV"),
    cv_in(21, "band6_q_cv_in", "High cut Q CV"),
    cv_in(22, "band7_q_cv_in", "Band 4 Q CV"),
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
        descriptor: crate::bypass::matched(&DESCRIPTOR),
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
pub(crate) fn bandpass(freq: f64, q: f64, sample_rate: f64) -> BiquadCoeffs {
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

/// The biquad shape of a middle band's `shape` value (anything unknown is a bell).
fn mid_shape(shape: usize) -> BiquadShape {
    match shape {
        1 => BiquadShape::LowShelf,
        2 => BiquadShape::HighShelf,
        SHAPE_NOTCH => BiquadShape::Notch,
        _ => BiquadShape::Peaking,
    }
}

/// The Q a middle band of `shape` really uses for the knob value `q`: shelves 0.3..2, bells and
/// notches 0.1..18.
pub fn effective_q(shape: usize, q: f64) -> f64 {
    let (lo, hi) = if matches!(shape, 1 | 2) {
        SHELF_Q
    } else {
        BELL_Q
    };
    q.clamp(lo, hi)
}

/// Whether a middle band of `shape` with `gain_db` is audible when it is switched on: a bell or a
/// shelf at exactly 0 dB is off, a notch ignores the gain.
pub fn middle_active(shape: usize, gain_db: f32) -> bool {
    shape == SHAPE_NOTCH || gain_db != 0.0
}

/// Magnitude response in dB at `f` Hz of a middle band (`shape` 0 Bell, 1 Low shelf, 2 High
/// shelf, 3 Notch) at sample rate `sr`: the coefficients the DSP designs, with its Q clamp. The
/// band's `on` flag and the 0 dB rule ([`middle_active`]) are the caller's.
pub fn middle_response_db(shape: usize, freq: f64, q: f64, gain_db: f64, f: f64, sr: f64) -> f64 {
    let gain_db = if shape == SHAPE_NOTCH { 0.0 } else { gain_db };
    BiquadCoeffs::design(mid_shape(shape), freq, effective_q(shape, q), gain_db, sr)
        .response_db(f, sr)
}

/// The sections of a cut at `slope` with resonance knob `q`: the Q of each section (padded with
/// 0) and how many there are. The highest-Q section is scaled by `q / 0.7071`; the 6 dB/oct
/// slope is one first-order section that ignores `q`.
fn cut_sections(slope: usize, q: f64) -> ([f64; 4], usize) {
    let slope = slope.min(SLOPE_MAX);
    let n = SLOPE_SECTIONS[slope];
    let qs = q / std::f64::consts::FRAC_1_SQRT_2;
    let mut out = [0.0; 4];
    for (i, (o, bq)) in out.iter_mut().zip(BUTTER_Q[slope]).enumerate().take(n) {
        *o = if i + 1 == n && slope != SLOPE_6 {
            bq * qs
        } else {
            bq
        };
    }
    (out, n)
}

/// The biquad shape of section `i` of a cut at `slope`.
fn cut_shape(low: bool, slope: usize, i: usize) -> BiquadShape {
    match (low, slope == SLOPE_6 && i == 0) {
        (true, true) => BiquadShape::FirstOrderHighpass,
        (true, false) => BiquadShape::Highpass,
        (false, true) => BiquadShape::FirstOrderLowpass,
        (false, false) => BiquadShape::Lowpass,
    }
}

/// Magnitude response in dB at `f` Hz of a cut (`low` for the low cut, a high-pass; `slope` is
/// the param value, see [`SLOPE_NAMES`]; `q` the resonance knob, ignored at 6 dB/oct) at sample
/// rate `sr`. The `on` flag is the caller's.
pub fn cut_response_db(low: bool, slope: usize, freq: f64, q: f64, f: f64, sr: f64) -> f64 {
    let slope = slope.min(SLOPE_MAX);
    let (qs, n) = cut_sections(slope, q);
    (0..n)
        .map(|i| {
            BiquadCoeffs::design(cut_shape(low, slope, i), freq, qs[i], 0.0, sr).response_db(f, sr)
        })
        .sum()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    LowCut,
    Middle,
    HighCut,
}

const KINDS: [Kind; NB] = [
    Kind::LowCut,
    Kind::Middle,
    Kind::Middle,
    Kind::Middle,
    Kind::Middle,
    Kind::HighCut,
    Kind::Middle,
];

/// The order the bands process in (display order of the sections: 1, 2, 3, 4, 7, 5, 6).
const PROCESS_ORDER: [usize; NB] = [0, 1, 2, 3, 6, 4, 5];

impl Kind {
    fn is_cut(self) -> bool {
        matches!(self, Self::LowCut | Self::HighCut)
    }
}

/// One EQ band: its raw params, its sections and its bypass weight.
#[derive(Debug, Clone)]
struct Band {
    idx: usize,
    kind: Kind,
    on: bool,
    freq: f32,
    gain: f32,
    /// CV offset added to `gain`, in dB (0 unless a gain CV is connected).
    cv_db: f32,
    /// CV offset of the frequency in octaves.
    cv_oct: f32,
    /// CV offset of the Q in powers of two.
    cv_q: f32,
    /// Gain, frequency and Q CV amounts (-1..+1).
    trims: [f32; 3],
    q: f32,
    slope: usize,
    /// The slope the sections are built for (differs from `slope` while a change is pending).
    applied: usize,
    /// The shape of a middle band (0 Bell, 1 Low shelf, 2 High shelf, 3 Notch).
    shape: usize,
    /// The shape the section is built for (differs from `shape` while a change is pending).
    applied_shape: usize,
    sample_rate: f32,
    /// Bypass weight (1 when on).
    w: BlockSmoother,
    secs: Vec<SmoothedBiquad>,
    /// Number of sections in use (1 for middle bands).
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
        let q = default(".q", std::f32::consts::FRAC_1_SQRT_2);
        let gain = default(".gain", 0.0);
        let on = default(".on", 0.0) >= 0.5;
        let first = match kind {
            Kind::LowCut => BiquadShape::Highpass,
            Kind::HighCut => BiquadShape::Lowpass,
            Kind::Middle => mid_shape(DEFAULT_SHAPE[i]),
        };
        let secs = (0..4)
            .map(|_| SmoothedBiquad::new(first, freq, q, gain, sample_rate))
            .collect();
        let mut b = Self {
            idx: i,
            kind,
            on,
            freq,
            gain,
            cv_db: 0.0,
            cv_oct: 0.0,
            cv_q: 0.0,
            trims: [1.0; 3],
            q,
            slope: 0,
            applied: 0,
            shape: DEFAULT_SHAPE[i],
            applied_shape: DEFAULT_SHAPE[i],
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
        self.on && (self.kind.is_cut() || middle_active(self.applied_shape, self.eff_gain()))
    }

    /// Knob gain plus the CV offset, clamped to the parameter range.
    fn eff_gain(&self) -> f32 {
        (self.gain + self.cv_db).clamp(-GAIN_RANGE_DB, GAIN_RANGE_DB)
    }

    /// Knob frequency moved by the CV, clamped to the parameter range.
    fn eff_freq(&self) -> f32 {
        if self.cv_oct == 0.0 {
            return self.freq;
        }
        let p = &PARAMS[BASE[self.idx] as usize + 1];
        (self.freq * exp2_det(self.cv_oct)).clamp(p.min, p.max)
    }

    /// Knob Q moved by the CV, clamped to the range of the cut or of the shape.
    fn eff_q(&self) -> f32 {
        let q = if self.cv_q == 0.0 {
            self.q
        } else {
            self.q * exp2_det(self.cv_q)
        };
        if self.kind.is_cut() {
            let p = &PARAMS[BASE[self.idx] as usize + 2];
            q.clamp(p.min, p.max)
        } else {
            effective_q(self.applied_shape, f64::from(q)) as f32
        }
    }

    /// Pushes the raw params into the sections' smoothed targets.
    fn apply(&mut self) {
        let f = self.eff_freq();
        let qe = self.eff_q();
        if self.kind.is_cut() {
            let low = self.kind == Kind::LowCut;
            let (qs, n) = cut_sections(self.applied, f64::from(qe));
            for (i, (sec, qq)) in self.secs.iter_mut().zip(qs).enumerate().take(n) {
                sec.set_shape(cut_shape(low, self.applied, i));
                sec.set_freq(f);
                sec.set_q(qq as f32);
            }
            for i in self.nsec..n {
                self.secs[i].reset(); // newly active sections start clean
            }
            self.nsec = n;
        } else {
            let g = if self.applied_shape == SHAPE_NOTCH {
                0.0
            } else {
                self.eff_gain()
            };
            let sec = &mut self.secs[0];
            sec.set_shape(mid_shape(self.applied_shape));
            sec.set_freq(f);
            sec.set_q(qe);
            sec.set_gain_db(g);
        }
        self.stale = true;
    }

    /// Clears every section's state.
    fn clear(&mut self) {
        for s in &mut self.secs {
            s.reset();
        }
    }

    /// Runs the band over one batch in place.
    fn process(&mut self, l: &mut [f32; BATCH], r: &mut [f32; BATCH]) {
        let mut pending = self.slope != self.applied || self.shape != self.applied_shape;
        if pending && self.w.value() < 0.01 {
            // Faded out: rebuild the sections for the new slope or shape from a clean state and
            // fade in.
            self.applied = self.slope;
            self.applied_shape = self.shape;
            self.apply();
            self.clear();
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
                self.clear();
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
    /// The `stereo.mode` param.
    mode: usize,
    /// The mode the bands run in (differs from `mode` for the one batch of a fade-out).
    mode_applied: usize,
    /// The next batch fades the output back in after a mode change.
    mode_in: bool,
    bank: [Biquad; ANALYSER_BANDS],
    bank_coeffs: [BiquadCoeffs; ANALYSER_BANDS],
    energy: [f64; ANALYSER_BANDS],
    energy_alpha: f64,
    peaks: [f32; 2],
    /// The analyser bank runs (a panel with the display is on screen).
    analyser: bool,
    /// The post-EQ output for the spectrum display (written while its live flag is set).
    tap: Arc<ScopeTap>,
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
            mode: 0,
            mode_applied: 0,
            mode_in: false,
            bank: [Biquad::default(); ANALYSER_BANDS],
            bank_coeffs: [BiquadCoeffs::IDENTITY; ANALYSER_BANDS],
            energy: [0.0; ANALYSER_BANDS],
            energy_alpha: 1.0,
            peaks: [0.0; 2],
            analyser: false,
            // Created here, not in `prepare`: the tap outlives re-prepares (no ring until a view asks).
            tap: Arc::new(ScopeTap::on_demand()),
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
                    b.shape = o.shape;
                    b.applied_shape = o.shape;
                    b.trims = o.trims;
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

    /// Reads the CV inputs of every band (once per batch) and pushes changes into the sections.
    fn read_cv(&mut self, ctx: &ProcessContext<'_>) {
        for (i, band) in self.bands.iter_mut().enumerate() {
            let cv = |sock: SocketId, scale: f32, trim: f32| {
                if ctx.is_connected(sock) {
                    trim * scale * batch_mean(ctx.input(sock))
                } else {
                    0.0
                }
            };
            let db = GAIN_CV[i].map_or(0.0, |s| cv(s, GAIN_RANGE_DB, band.trims[0]));
            let oct = cv(freq_cv(i), FREQ_CV_OCT, band.trims[1]);
            let qv = cv(q_cv(i), Q_CV_OCT, band.trims[2]);
            if db.to_bits() != band.cv_db.to_bits()
                || oct.to_bits() != band.cv_oct.to_bits()
                || qv.to_bits() != band.cv_q.to_bits()
            {
                band.cv_db = db;
                band.cv_oct = oct;
                band.cv_q = qv;
                band.apply();
            }
        }
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
            b.applied_shape = b.shape;
            b.apply();
            b.clear();
            b.w.set_immediate(if b.wants_on() { 1.0 } else { 0.0 });
            b.stale = false;
        }
        for f in &mut self.bank {
            f.reset();
        }
        self.energy = [0.0; ANALYSER_BANDS];
        self.peaks = [0.0; 2];
        self.mode_applied = self.mode;
        self.mode_in = false;
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        let Some(p) = PARAMS.get(id.0 as usize) else {
            return;
        };
        let v = v.clamp(p.min, p.max);
        let id = id.0;
        match id {
            P_OUTPUT => self.output_db = v,
            P_ANALYSER_LIVE => {
                let on = v >= 0.5;
                if on && !self.analyser {
                    // The bank was idle: it starts from silence, not from an old tail.
                    for f in &mut self.bank {
                        f.reset();
                    }
                    self.energy = [0.0; ANALYSER_BANDS];
                }
                self.analyser = on;
            }
            P_GAIN_TRIM..=28 => self.bands[(id - P_GAIN_TRIM) as usize + 1].trims[0] = v,
            P_BAND7_GAIN_TRIM => self.bands[6].trims[0] = v,
            P_FREQ_TRIM..=50 => self.bands[(id - P_FREQ_TRIM) as usize].trims[1] = v,
            P_Q_TRIM..=57 => self.bands[(id - P_Q_TRIM) as usize].trims[2] = v,
            P_STEREO_MODE => {
                self.mode = v.round() as usize;
                if self.fresh {
                    self.mode_applied = self.mode;
                }
            }
            P_SHAPE..=39 => {
                self.bands[SHAPE_BANDS[(id - P_SHAPE) as usize]].shape = v.round() as usize;
                // Applied by `process` after a fade-out.
            }
            P_SPECTRUM_ON | P_DISPLAY_RANGE => {} // display settings, unused here
            _ => self.set_band_param(id, v),
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let finite = |x: f32| if x.is_finite() { x } else { 0.0 };
        let mut l = *ctx.input(IN_L);
        let mut r = *ctx.input(IN_R);
        for n in 0..BATCH {
            l[n] = finite(l[n]);
            r[n] = finite(r[n]);
        }
        self.read_cv(ctx);
        // A mode change fades the output out over this batch and switches after it; the next
        // batch fades back in.
        let mut ramp: Option<(f32, f32)> = None;
        let mut switch = false;
        if self.mode != self.mode_applied {
            // The output is still silent when the fade-in of the last change is pending: stay
            // silent and start the cycle again, instead of ramping down from a level never reached.
            ramp = Some(if self.mode_in { (0.0, 0.0) } else { (1.0, 0.0) });
            switch = true;
        } else if self.mode_in {
            ramp = Some((0.0, 1.0));
            self.mode_in = false;
        }
        if self.mode_applied == 0 {
            for &i in &PROCESS_ORDER {
                self.bands[i].process(&mut l, &mut r);
            }
        } else {
            let (mut mid, mut side) = ([0.0f32; BATCH], [0.0f32; BATCH]);
            for n in 0..BATCH {
                mid[n] = 0.5 * (l[n] + r[n]);
                side[n] = 0.5 * (l[n] - r[n]);
            }
            let side_eq = matches!(self.mode_applied, 2 | 4);
            let mut x = if side_eq { side } else { mid };
            let mut twin = x;
            for &i in &PROCESS_ORDER {
                self.bands[i].process(&mut x, &mut twin);
            }
            for n in 0..BATCH {
                let (m, s) = if side_eq {
                    (mid[n], x[n])
                } else {
                    (x[n], side[n])
                };
                (l[n], r[n]) = match self.mode_applied {
                    1 | 2 => (m + s, m - s),
                    _ => (x[n], x[n]),
                };
            }
        }
        if let Some((g0, g1)) = ramp {
            for n in 0..BATCH {
                let g = g0 + (g1 - g0) * ((n + 1) as f32 / BATCH as f32);
                l[n] *= g;
                r[n] *= g;
            }
        }
        if switch {
            // Nothing the filters hold belongs to the new signal: start them clean.
            self.mode_applied = self.mode;
            self.mode_in = true;
            for b in &mut self.bands {
                b.clear();
            }
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
        if self.tap.is_live() {
            self.tap.push(&l, &r);
        }
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

    fn scope_tap(&self) -> Option<Arc<ScopeTap>> {
        Some(Arc::clone(&self.tap))
    }
}

impl Equalizer {
    /// `on/freq/gain-or-q/q-or-slope` of one band (the params of the band tables).
    fn set_band_param(&mut self, id: u32, v: f32) {
        let i = if id >= P_BAND7 {
            6
        } else {
            match BASE.iter().take(6).rposition(|b| *b <= id) {
                Some(i) => i,
                None => return,
            }
        };
        let off = id - BASE[i];
        if off > 3 {
            return;
        }
        let band = &mut self.bands[i];
        let cut = band.kind.is_cut();
        match (off, cut) {
            (0, _) => band.on = v >= 0.5,
            (1, _) => band.freq = v,
            (2, true) => band.q = v,
            (2, false) => band.gain = v,
            (3, true) => {
                band.slope = (v.round() as usize).min(SLOPE_MAX);
                return; // applied by `process` after a fade
            }
            (3, false) => band.q = v,
            _ => return,
        }
        band.apply();
    }
}
