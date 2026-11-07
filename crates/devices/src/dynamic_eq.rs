//! `atomik.dynamic-eq`: a five-band stereo equaliser whose bands move with the level of the
//! signal (or of a key signal) in their own frequency range.
//!
//! # Bands
//!
//! | Band | Type | Keys |
//! |---|---|---|
//! | 1 | low shelf | `band1.*` |
//! | 2, 3, 4 | bell | `band2.*` .. `band4.*` |
//! | 5 | high shelf | `band5.*` |
//!
//! Every band has the same eleven params `on`, `freq`, `q`, `gain` (the static gain, dB),
//! `threshold` (dB), `range` (dB), `ratio`, `attack`, `release`, `source` (`band` or `key`) and
//! `channel` (`stereo`, `mid` or `side`), plus the shared `output` gain and the `delta` switch. The bands run in series,
//! 1 to 5, as RBJ cookbook sections of [`atomik_dsp::BiquadCoeffs`] (`f64` coefficients and
//! state, direct form I).
//!
//! # What a band does (per sample)
//!
//! ```text
//! key   = the signal the band listens to: the device input (`source` band) or the KEY input
//! b     = key -> band-pass at `freq`, unity gain at the centre, width `q` (clamped 0.5 .. 8)
//! level = follower of |b| (`attack` rising, `release` falling) x pi/2, in dB
//! over  = level - threshold
//! red   = soft-knee curve of over with slope 1 - 1/ratio (6 dB knee), at most |range|
//! dyn   = red x sign(range)
//! gain  = static gain + dyn            (dB, the gain of the band's shelf or bell)
//! ```
//!
//! * **Range.** A negative `range` cuts the band while its level is above the threshold (the
//!   dynamic cut of a resonance or of masking); a positive `range` lifts it (an expanding boost).
//!   `range` 0 makes the band a plain static band. The band never moves further than `|range|`.
//! * **Level scale.** The follower of the rectified band is scaled by pi/2, so a sine of peak `A`
//!   at the centre reads `A` (a peak of 1.0 is 0 dB) when the attack and release are equal and span
//!   several cycles; an attack much faster than the release reads up to a couple of dB higher.
//!   The threshold is in those dB.
//! * **Source.** `band` listens to the device input, whatever the other bands do to it, so the
//!   bands do not chase each other. `key` listens to the `sc_in_l`/`sc_in_r` sockets band-passed at
//!   the band's own frequency (a lone connected side feeds both channels, as on the compressor);
//!   with no cable in the key sockets a band set to `key` listens to the input.
//! * **Channel.** `stereo` runs the filter on left and right and the detector follows the louder
//!   of the two, so the image does not move. `mid` and `side` run one filter on the mid
//!   `(L + R) / 2` or side `(L - R) / 2` signal and add the change back (`+` on both for the mid,
//!   `+` left and `-` right for the side); the detector listens to the same signal.
//! * **Shelves** listen around their corner frequency (the same band-pass), so a low shelf set to
//!   cut reacts to the energy at its corner.
//! * **Smoothing.** The static gain, the frequency and the Q follow a 10 ms one-pole; the dynamic
//!   gain is smooth by the follower. The coefficients are recomputed every 16 samples while
//!   anything moves and not at all when nothing does.
//! * **Off bands are bit-transparent.** A band that is off, or on with a static gain and a range
//!   of exactly 0 dB, fades out (10 ms) and is then skipped with its state reset, so with every
//!   band off and `output` at 0 dB the input is copied bit for bit. A band that comes on starts
//!   from a clean state and fades in over the same 10 ms.
//! * **Delta.** With `delta` on the device plays what the bands took out: the input minus the
//!   output of the bands (a cut shows as the cut part of the sound, a lift with the opposite sign),
//!   before the `output` gain. The switch fades over 10 ms, and with every band off the delta is
//!   exactly 0.0, so a neutral device is silent. Meter values and peaks follow what is played.
//! * **Latency.** None: no look-ahead.
//!
//! # Meter values (`has_meter_values`, 40 values)
//!
//! `[0..5)` the gain each band applies now in dB (static plus dynamic, 0 for a band that is
//! off), `[5..21)` the energies of a 16-band analyser of the INPUT (mean square of the band-passed
//! mono mix, linear power, the bank of [`crate::eq::analyser_centres`]), `[21..37)` the same for
//! the KEY (zero without a cable), `[37]`/`[38]` the output peaks and `[39]` 1 when a cable is in
//! the key sockets. The two banks run only while the app has set `internal.analyser_live` (never
//! stored, default off; the rack view sets it for the panels on screen); they start from silence
//! and the audio is the same either way.
//!
//! Non-finite input samples count as 0. Deterministic maths for the level (`log2_det`,
//! `exp2_det`); the coefficient design uses the standard library like the EQ. Allocation-free.

use crate::bypass::{Bypassable, bypass_param};
use crate::eq::{analyser_centres, bandpass};
use crate::tool_support::{audio_in, audio_out, choice, expo, finite, linear, toggle};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId,
};
use atomik_dsp::{
    Biquad, BiquadCoeffs, BiquadShape, BlockSmoother, Svf, exp2_det, exp2_det_f64, log2_det,
};

pub mod cuts;
#[cfg(test)]
mod tests;

/// Number of bands.
pub const BANDS: usize = 5;
/// Params per band.
pub const PER_BAND: usize = 11;
/// Number of analyser bands (the EQ's bank).
pub const ANALYSER_BANDS: usize = 16;
/// Where the band gains start in `meter_values`.
pub const METER_GAINS: usize = 0;
/// Where the input analyser starts.
pub const METER_INPUT: usize = BANDS;
/// Where the key analyser starts.
pub const METER_KEY: usize = METER_INPUT + ANALYSER_BANDS;
/// The output peaks (L, then R).
pub const METER_PEAKS: usize = METER_KEY + ANALYSER_BANDS;
/// 1 when a cable is in the key sockets.
pub const METER_KEY_CONNECTED: usize = METER_PEAKS + 2;
/// Number of `meter_values`.
pub const METER_COUNT: usize = METER_KEY_CONNECTED + 1;

/// The gain a band can reach in dB, static plus dynamic.
const MAX_TOTAL_DB: f32 = 36.0;
/// Width of the soft knee of the gain computer, dB.
const KNEE_DB: f32 = 6.0;
/// `20 * log10(2)`.
const DB_PER_OCTAVE: f32 = 6.020_6;
/// `log2(10) / 20`.
const LOG2_10_20: f32 = 0.166_096_4;
/// The level below which a detector reads silence, dB.
const FLOOR_DB: f32 = -120.0;
/// Mean of a rectified sine is `2 / pi` of its peak.
const FOLLOW_CAL: f32 = std::f32::consts::FRAC_PI_2;
/// Time constant of the fade of a band in or out, and of the smoothing of its settings, ms.
const SMOOTH_MS: f64 = 10.0;
/// Samples between coefficient updates.
const UPDATE: u8 = 16;
/// The weight that counts as fully faded.
const SNAP: f32 = 5e-5;

const P_OUTPUT: u32 = (BANDS * PER_BAND) as u32;
const P_DELTA: u32 = P_OUTPUT + 1;
const P_ANALYSER_LIVE: u32 = P_OUTPUT + 3;

const SOURCES: &[&str] = &["band", "key"];
const CHANNELS: &[&str] = &["stereo", "mid", "side"];

/// The eleven rows of one band.
#[allow(clippy::too_many_arguments)]
const fn band_rows(
    b: u32,
    keys: [&'static str; PER_BAND],
    names: [&'static str; PER_BAND],
    freq: (f32, f32, f32),
    q: (f32, f32, f32),
) -> [ParamDescriptor; PER_BAND] {
    let base = b * PER_BAND as u32;
    [
        toggle(base, keys[0], names[0], 0.0),
        expo(
            base + 1,
            keys[1],
            names[1],
            freq.0,
            freq.1,
            freq.2,
            Unit::Hz,
        ),
        expo(base + 2, keys[2], names[2], q.0, q.1, q.2, Unit::None),
        linear(base + 3, keys[3], names[3], -24.0, 24.0, 0.0, Unit::Db),
        linear(base + 4, keys[4], names[4], -60.0, 0.0, -24.0, Unit::Db),
        linear(base + 5, keys[5], names[5], -24.0, 24.0, -6.0, Unit::Db),
        expo(base + 6, keys[6], names[6], 1.5, 20.0, 4.0, Unit::None),
        expo(base + 7, keys[7], names[7], 0.1, 200.0, 10.0, Unit::Ms),
        expo(base + 8, keys[8], names[8], 5.0, 2_000.0, 100.0, Unit::Ms),
        choice(base + 9, keys[9], names[9], SOURCES, 0.0),
        choice(base + 10, keys[10], names[10], CHANNELS, 0.0),
    ]
}

macro_rules! keys {
    ($n:literal) => {
        [
            concat!("band", $n, ".on"),
            concat!("band", $n, ".freq"),
            concat!("band", $n, ".q"),
            concat!("band", $n, ".gain"),
            concat!("band", $n, ".threshold"),
            concat!("band", $n, ".range"),
            concat!("band", $n, ".ratio"),
            concat!("band", $n, ".attack"),
            concat!("band", $n, ".release"),
            concat!("band", $n, ".source"),
            concat!("band", $n, ".channel"),
        ]
    };
}

macro_rules! names {
    ($label:literal) => {
        [
            concat!($label, " on"),
            concat!($label, " freq"),
            concat!($label, " Q"),
            concat!($label, " gain"),
            concat!($label, " threshold"),
            concat!($label, " range"),
            concat!($label, " ratio"),
            concat!($label, " attack"),
            concat!($label, " release"),
            concat!($label, " source"),
            concat!($label, " channel"),
        ]
    };
}

const TOTAL_PARAMS: usize = BANDS * PER_BAND + 4;

const fn build_params() -> [ParamDescriptor; TOTAL_PARAMS] {
    let bands = [
        band_rows(
            0,
            keys!(1),
            names!("Low shelf"),
            (20.0, 5_000.0, 120.0),
            (0.3, 2.0, std::f32::consts::FRAC_1_SQRT_2),
        ),
        band_rows(
            1,
            keys!(2),
            names!("Bell 1"),
            (20.0, 20_000.0, 400.0),
            (0.1, 18.0, 1.0),
        ),
        band_rows(
            2,
            keys!(3),
            names!("Bell 2"),
            (20.0, 20_000.0, 1_500.0),
            (0.1, 18.0, 1.0),
        ),
        band_rows(
            3,
            keys!(4),
            names!("Bell 3"),
            (20.0, 20_000.0, 4_000.0),
            (0.1, 18.0, 1.0),
        ),
        band_rows(
            4,
            keys!(5),
            names!("High shelf"),
            (500.0, 20_000.0, 8_000.0),
            (0.3, 2.0, std::f32::consts::FRAC_1_SQRT_2),
        ),
    ];
    let tail = [
        linear(P_OUTPUT, "output", "Output", -24.0, 24.0, 0.0, Unit::Db),
        toggle(P_DELTA, "delta", "Delta", 0.0),
        bypass_param(P_DELTA + 1),
        toggle(
            P_ANALYSER_LIVE,
            "internal.analyser_live",
            "Analyser live",
            0.0,
        ),
    ];
    let mut out = [tail[0]; TOTAL_PARAMS];
    let mut b = 0;
    while b < BANDS {
        let mut i = 0;
        while i < PER_BAND {
            out[b * PER_BAND + i] = bands[b][i];
            i += 1;
        }
        b += 1;
    }
    out[BANDS * PER_BAND] = tail[0];
    out[BANDS * PER_BAND + 1] = tail[1];
    out[BANDS * PER_BAND + 2] = tail[2];
    out[BANDS * PER_BAND + 3] = tail[3];
    out
}

static PARAMS: [ParamDescriptor; TOTAL_PARAMS] = build_params();

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const SC_L: SocketId = SocketId(2);
const SC_R: SocketId = SocketId(3);
const OUT_L: SocketId = SocketId(4);
const OUT_R: SocketId = SocketId(5);

static SOCKETS: [SocketDescriptor; 6] = [
    audio_in(0, "in_l", "In L"),
    audio_in(1, "in_r", "In R"),
    audio_in(2, "sc_in_l", "Key L"),
    audio_in(3, "sc_in_r", "Key R"),
    audio_out(4, "out_l", "Out L"),
    audio_out(5, "out_r", "Out R"),
];

/// Static description of the dynamic EQ.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.dynamic-eq",
    name: "Dynamic EQ",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`DynamicEq`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(DynamicEq::new())),
    }
}

/// Which signal a band filters and listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chan {
    Stereo,
    Mid,
    Side,
}

/// Rates derived from the sample rate.
#[derive(Debug, Clone, Copy)]
struct Rates {
    sample_rate: f32,
    /// One-pole step of the fade, per sample.
    fade: f32,
    /// One-pole step of the setting smoothers, per update.
    update: f32,
}

impl Rates {
    fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            fade: one_pole(SMOOTH_MS as f32, sample_rate),
            update: one_pole(SMOOTH_MS as f32 / f32::from(UPDATE), sample_rate),
        }
    }
}

fn one_pole(ms: f32, sample_rate: f32) -> f32 {
    let n = f64::from(ms) * 0.001 * f64::from(sample_rate);
    (1.0 - exp2_det_f64(-std::f64::consts::LOG2_E / n)) as f32
}

/// The soft-knee gain computer: how many dB of the (scaled) overshoot get through.
fn curve(over: f32, slope: f32) -> f32 {
    let half = KNEE_DB / 2.0;
    if over <= -half {
        0.0
    } else if over < half {
        let k = over + half;
        slope * k * k / (2.0 * KNEE_DB)
    } else {
        slope * over
    }
}

/// One band: settings, detector and filter.
#[derive(Debug, Clone)]
struct Band {
    shape: BiquadShape,
    // Settings.
    on: bool,
    freq: f32,
    q: f32,
    gain: f32,
    threshold: f32,
    range: f32,
    slope: f32,
    attack_ms: f32,
    release_ms: f32,
    use_key: bool,
    chan: Chan,
    // Derived.
    attack_c: f32,
    release_c: f32,
    det_k: f32,
    // State.
    /// 0..1: how much of the filtered signal is heard.
    weight: f32,
    awake: bool,
    det: [Svf; 2],
    level: f32,
    dyn_db: f32,
    filt: [Biquad; 2],
    coeffs: BiquadCoeffs,
    /// Smoothed `[log2 freq, log2 q, static gain]`.
    cur: [f32; 3],
    last_total: f32,
    countdown: u8,
    /// The gain applied at the end of the last batch, dB.
    shown_db: f32,
}

impl Band {
    fn new(index: usize, rates: &Rates) -> Self {
        let p = &PARAMS[index * PER_BAND..(index + 1) * PER_BAND];
        let shape = match index {
            0 => BiquadShape::LowShelf,
            i if i == BANDS - 1 => BiquadShape::HighShelf,
            _ => BiquadShape::Peaking,
        };
        let mut b = Self {
            shape,
            on: false,
            freq: p[1].default,
            q: p[2].default,
            gain: 0.0,
            threshold: p[4].default,
            range: p[5].default,
            slope: 1.0 - 1.0 / p[6].default,
            attack_ms: p[7].default,
            release_ms: p[8].default,
            use_key: false,
            chan: Chan::Stereo,
            attack_c: 0.0,
            release_c: 0.0,
            det_k: 1.0,
            weight: 0.0,
            awake: false,
            det: [Svf::new(); 2],
            level: 0.0,
            dyn_db: 0.0,
            filt: [Biquad::default(); 2],
            coeffs: BiquadCoeffs::IDENTITY,
            cur: [0.0; 3],
            last_total: 0.0,
            countdown: 0,
            shown_db: 0.0,
        };
        b.derive(rates);
        b
    }

    fn derive(&mut self, rates: &Rates) {
        self.attack_c = one_pole(self.attack_ms, rates.sample_rate);
        self.release_c = one_pole(self.release_ms, rates.sample_rate);
        self.det_k = 1.0 / self.q.clamp(0.5, 8.0);
        let g = Svf::cutoff_to_g_det(self.freq, rates.sample_rate);
        for f in &mut self.det {
            f.set_coefficients(g, self.det_k);
        }
    }

    /// The band changes the sound: on, with a static gain or a range.
    fn active(&self) -> bool {
        self.on && (self.gain != 0.0 || self.range != 0.0)
    }

    fn targets(&self) -> [f32; 3] {
        [log2_det(self.freq), log2_det(self.q), self.gain]
    }

    fn design(&mut self, rates: &Rates, total_db: f32) {
        self.coeffs = BiquadCoeffs::design(
            self.shape,
            exp2_det_f64(f64::from(self.cur[0])),
            exp2_det_f64(f64::from(self.cur[1])),
            f64::from(total_db),
            f64::from(rates.sample_rate),
        );
        self.last_total = total_db;
    }

    fn wake(&mut self, rates: &Rates) {
        self.cur = self.targets();
        self.level = 0.0;
        self.dyn_db = 0.0;
        for f in &mut self.det {
            f.reset();
        }
        for f in &mut self.filt {
            f.reset();
        }
        self.design(rates, self.gain.clamp(-MAX_TOTAL_DB, MAX_TOTAL_DB));
        self.countdown = UPDATE;
        self.awake = true;
    }

    fn sleep(&mut self) {
        self.awake = false;
        self.weight = 0.0;
        self.level = 0.0;
        self.dyn_db = 0.0;
        self.shown_db = 0.0;
        for f in &mut self.det {
            f.reset();
        }
        for f in &mut self.filt {
            f.reset();
        }
    }

    /// Moves the smoothed settings one step and redesigns the section when something changed.
    fn refresh(&mut self, rates: &Rates) {
        let t = self.targets();
        let mut moved = false;
        for (c, t) in self.cur.iter_mut().zip(t) {
            let d = t - *c;
            if d.abs() > 1e-6 {
                *c += d * rates.update;
                moved = true;
            } else {
                *c = t;
            }
        }
        let total = (self.cur[2] + self.dyn_db).clamp(-MAX_TOTAL_DB, MAX_TOTAL_DB);
        if moved || (total - self.last_total).abs() > 0.004 {
            self.design(rates, total);
        }
    }

    /// Runs one batch: the detector reads `det` (the input or the key) and the filter changes
    /// `out`.
    fn run(&mut self, rates: &Rates, det: &[[f32; BATCH]; 2], out: &mut [[f32; BATCH]; 2]) {
        let active = self.active();
        if !self.awake {
            if !active {
                return;
            }
            self.wake(rates);
        } else if !active && self.weight == 0.0 {
            self.sleep();
            return;
        }
        let target = if active { 1.0 } else { 0.0 };
        for n in 0..BATCH {
            self.weight += (target - self.weight) * rates.fade;
            if !active && self.weight < SNAP {
                self.weight = 0.0;
            } else if active && self.weight > 1.0 - SNAP {
                self.weight = 1.0;
            }

            // The band of the listened-to signal and its level.
            let (a, b) = (det[0][n], det[1][n]);
            let d = match self.chan {
                Chan::Stereo => [a, b],
                Chan::Mid => [0.5 * (a + b), 0.0],
                Chan::Side => [0.5 * (a - b), 0.0],
            };
            let mut rect = (self.det[0].process(d[0]).bp * self.det_k).abs();
            if self.chan == Chan::Stereo {
                rect = rect.max((self.det[1].process(d[1]).bp * self.det_k).abs());
            }
            let c = if rect > self.level {
                self.attack_c
            } else {
                self.release_c
            };
            self.level += c * (rect - self.level);
            if self.level < 1e-20 {
                self.level = 0.0;
            }
            let level_db = if self.level > 0.0 {
                (log2_det(self.level * FOLLOW_CAL) * DB_PER_OCTAVE).max(FLOOR_DB)
            } else {
                FLOOR_DB
            };
            let red = curve(level_db - self.threshold, self.slope).min(self.range.abs());
            self.dyn_db = if self.range < 0.0 { -red } else { red };

            if self.countdown == 0 {
                self.refresh(rates);
                self.countdown = UPDATE;
            }
            self.countdown -= 1;

            let (xl, xr) = (out[0][n], out[1][n]);
            let (yl, yr) = match self.chan {
                Chan::Stereo => (
                    self.filt[0].process(&self.coeffs, f64::from(xl)) as f32,
                    self.filt[1].process(&self.coeffs, f64::from(xr)) as f32,
                ),
                Chan::Mid => {
                    let m = 0.5 * (f64::from(xl) + f64::from(xr));
                    let delta = (self.filt[0].process(&self.coeffs, m) - m) as f32;
                    (xl + delta, xr + delta)
                }
                Chan::Side => {
                    let s = 0.5 * (f64::from(xl) - f64::from(xr));
                    let delta = (self.filt[0].process(&self.coeffs, s) - s) as f32;
                    (xl + delta, xr - delta)
                }
            };
            let w = self.weight;
            out[0][n] = xl + w * (yl - xl);
            out[1][n] = xr + w * (yr - xr);
        }
        self.shown_db = self.weight * self.last_total;
    }
}

/// 16 band-pass filters over a mono signal with smoothed energies (the analyser of the EQ).
#[derive(Debug, Clone)]
struct Bank {
    filters: [Biquad; ANALYSER_BANDS],
    energy: [f64; ANALYSER_BANDS],
}

impl Bank {
    fn new() -> Self {
        Self {
            filters: [Biquad::default(); ANALYSER_BANDS],
            energy: [0.0; ANALYSER_BANDS],
        }
    }

    fn clear(&mut self) {
        for f in &mut self.filters {
            f.reset();
        }
        self.energy = [0.0; ANALYSER_BANDS];
    }

    fn feed(&mut self, coeffs: &[BiquadCoeffs; ANALYSER_BANDS], mono: &[f64; BATCH], alpha: f64) {
        let mut sq = [0.0f64; ANALYSER_BANDS];
        for x in mono {
            for ((f, c), acc) in self.filters.iter_mut().zip(coeffs).zip(&mut sq) {
                let y = f.process(c, *x);
                *acc += y * y;
            }
        }
        for (en, s) in self.energy.iter_mut().zip(sq) {
            let e = s / BATCH as f64;
            let v = *en + alpha * (e - *en);
            *en = if v < 1e-30 { 0.0 } else { v };
        }
    }
}

/// The dynamic EQ device.
#[derive(Debug, Clone)]
pub struct DynamicEq {
    rates: Rates,
    bands: Vec<Band>,
    output_db: f32,
    /// The `delta` switch and how far its fade has got (0 = the sound, 1 = the delta).
    delta_on: bool,
    delta_w: f32,
    out_sm: BlockSmoother,
    fresh: bool,
    analyser: bool,
    bank_coeffs: [BiquadCoeffs; ANALYSER_BANDS],
    energy_alpha: f64,
    input_bank: Bank,
    key_bank: Bank,
    peaks: [f32; 2],
    key_connected: bool,
}

impl Default for DynamicEq {
    fn default() -> Self {
        Self::new()
    }
}

impl DynamicEq {
    /// A dynamic EQ with default params at 48 kHz (`prepare` sets the real rate).
    pub fn new() -> Self {
        let rates = Rates::new(48_000.0);
        let mut d = Self {
            rates,
            bands: (0..BANDS).map(|i| Band::new(i, &rates)).collect(),
            output_db: 0.0,
            delta_on: false,
            delta_w: 0.0,
            out_sm: BlockSmoother::new(1.0),
            fresh: true,
            analyser: false,
            bank_coeffs: [BiquadCoeffs::IDENTITY; ANALYSER_BANDS],
            energy_alpha: 1.0,
            input_bank: Bank::new(),
            key_bank: Bank::new(),
            peaks: [0.0; 2],
            key_connected: false,
        };
        d.setup(48_000.0);
        d
    }

    fn setup(&mut self, sample_rate: f32) {
        self.rates = Rates::new(sample_rate);
        for b in &mut self.bands {
            b.derive(&self.rates);
        }
        self.out_sm.set_time(20.0, sample_rate, BATCH);
        let sr = f64::from(sample_rate);
        let centres = analyser_centres(sr);
        let ratio = (crate::eq::ANALYSER_HIGH_HZ / crate::eq::ANALYSER_LOW_HZ)
            .powf(1.0 / (ANALYSER_BANDS - 1) as f64);
        let qb = ratio.sqrt() / (ratio - 1.0);
        for (c, f) in self.bank_coeffs.iter_mut().zip(centres) {
            *c = bandpass(f, qb, sr);
        }
        self.energy_alpha =
            1.0 - exp2_det_f64(-std::f64::consts::LOG2_E * BATCH as f64 / (0.05 * sr));
    }
}

impl Device for DynamicEq {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.setup(info.sample_rate.hz() as f32);
        self.reset();
    }

    fn reset(&mut self) {
        for b in &mut self.bands {
            b.sleep();
        }
        self.input_bank.clear();
        self.key_bank.clear();
        self.peaks = [0.0; 2];
        self.key_connected = false;
        self.delta_w = if self.delta_on { 1.0 } else { 0.0 };
        self.fresh = true;
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let Some(p) = PARAMS.get(id.0 as usize) else {
            return;
        };
        let v = finite(plain).clamp(p.min, p.max);
        match id.0 {
            P_OUTPUT => {
                self.output_db = v;
                return;
            }
            P_DELTA => {
                self.delta_on = v >= 0.5;
                return;
            }
            P_ANALYSER_LIVE => {
                let on = v >= 0.5;
                if on && !self.analyser {
                    self.input_bank.clear();
                    self.key_bank.clear();
                }
                self.analyser = on;
                return;
            }
            n if n as usize >= BANDS * PER_BAND => return,
            _ => {}
        }
        let (i, off) = (id.0 as usize / PER_BAND, id.0 as usize % PER_BAND);
        let band = &mut self.bands[i];
        match off {
            0 => band.on = v >= 0.5,
            1 => band.freq = v,
            2 => band.q = v,
            3 => band.gain = v,
            4 => band.threshold = v,
            5 => band.range = v,
            6 => band.slope = 1.0 - 1.0 / v,
            7 => band.attack_ms = v,
            8 => band.release_ms = v,
            9 => band.use_key = v >= 0.5,
            _ => {
                let chan = match v.round() as u32 {
                    1 => Chan::Mid,
                    2 => Chan::Side,
                    _ => Chan::Stereo,
                };
                if chan != band.chan {
                    band.chan = chan;
                    // The filter now works on another signal: start it clean.
                    band.sleep();
                }
                return;
            }
        }
        if matches!(off, 1 | 2 | 7 | 8) {
            band.derive(&self.rates);
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let in_l = *ctx.input(IN_L);
        let in_r = *ctx.input(IN_R);
        let sc_l_conn = ctx.is_connected(SC_L);
        let sc_r_conn = ctx.is_connected(SC_R);
        let key_conn = sc_l_conn || sc_r_conn;
        let sc_l = *ctx.input(SC_L);
        let sc_r = *ctx.input(SC_R);

        let mut x = [[0.0f32; BATCH]; 2];
        let mut key = [[0.0f32; BATCH]; 2];
        for n in 0..BATCH {
            x[0][n] = finite(in_l[n]);
            x[1][n] = finite(in_r[n]);
            if key_conn {
                let (a, b) = (finite(sc_l[n]), finite(sc_r[n]));
                (key[0][n], key[1][n]) = match (sc_l_conn, sc_r_conn) {
                    (true, false) => (a, a),
                    (false, true) => (b, b),
                    _ => (a, b),
                };
            }
        }
        self.key_connected = key_conn;

        let mut out = x;
        for b in &mut self.bands {
            let det = if b.use_key && key_conn { &key } else { &x };
            b.run(&self.rates, det, &mut out);
        }

        let target = exp2_det(self.output_db * LOG2_10_20);
        if self.fresh {
            self.out_sm.set_immediate(target);
            self.fresh = false;
        }
        let g = self.out_sm.advance(target, BATCH);
        let mut peaks = [0.0f32; 2];
        let [out_l, out_r] = &mut out;
        let delta_target = if self.delta_on { 1.0 } else { 0.0 };
        for (n, (l, r)) in out_l.iter_mut().zip(out_r.iter_mut()).enumerate() {
            let gn = g.at(n);
            let w = if (self.delta_w - delta_target).abs() <= 0.0 {
                self.delta_w
            } else {
                self.delta_w += (delta_target - self.delta_w) * self.rates.fade;
                if (self.delta_w - delta_target).abs() < SNAP {
                    self.delta_w = delta_target;
                }
                self.delta_w
            };
            if w <= 0.0 {
                *l *= gn;
                *r *= gn;
            } else if w >= 1.0 {
                *l = x[0][n] - *l;
                *r = x[1][n] - *r;
            } else {
                *l = *l * gn * (1.0 - w) + (x[0][n] - *l) * w;
                *r = *r * gn * (1.0 - w) + (x[1][n] - *r) * w;
            }
            peaks[0] = peaks[0].max(l.abs());
            peaks[1] = peaks[1].max(r.abs());
        }
        self.peaks = peaks;

        if self.analyser {
            let mono = |s: &[[f32; BATCH]; 2]| {
                let mut m = [0.0f64; BATCH];
                for (n, v) in m.iter_mut().enumerate() {
                    *v = 0.5 * (f64::from(s[0][n]) + f64::from(s[1][n]));
                }
                m
            };
            self.input_bank
                .feed(&self.bank_coeffs, &mono(&x), self.energy_alpha);
            if key_conn {
                self.key_bank
                    .feed(&self.bank_coeffs, &mono(&key), self.energy_alpha);
            } else {
                self.key_bank.clear();
            }
        }

        *ctx.output(OUT_L) = out[0];
        *ctx.output(OUT_R) = out[1];
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let mut values = [0.0f32; METER_COUNT];
        for (v, b) in values[METER_GAINS..METER_INPUT].iter_mut().zip(&self.bands) {
            *v = b.shown_db;
        }
        for (v, e) in values[METER_INPUT..METER_KEY]
            .iter_mut()
            .zip(&self.input_bank.energy)
        {
            *v = *e as f32;
        }
        for (v, e) in values[METER_KEY..METER_PEAKS]
            .iter_mut()
            .zip(&self.key_bank.energy)
        {
            *v = *e as f32;
        }
        values[METER_PEAKS] = self.peaks[0];
        values[METER_PEAKS + 1] = self.peaks[1];
        values[METER_KEY_CONNECTED] = if self.key_connected { 1.0 } else { 0.0 };
        let n = values.len().min(out.len());
        out[..n].copy_from_slice(&values[..n]);
        n
    }
}
