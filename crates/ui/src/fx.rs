//! The effect panels' pure maths (adds Character, Spectrum and Phase Meter): which device types have a visual, the
//! compressor transfer curve, the EQ magnitude response, the distortion transfer function, the
//! reverb decay sketch, the LFO sketch and the conversion of published meter values into what
//! the panels draw. Nothing here touches a canvas, a clock or the document, so every formula is
//! tested against the DSP or the device it mirrors (`tests/fx_maths.rs`).

use crate::widget::current_value;
use atomik_core::ParamDescriptor;
use atomik_device_sdk::DeviceDescriptor;
use atomik_model::RackDevice;
use std::f64::consts::{FRAC_1_SQRT_2, PI};

/// The effects and the audio track with a hand-made panel and visual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FxKind {
    /// `atomik.reverb`.
    Reverb,
    /// `atomik.compressor`.
    Compressor,
    /// `atomik.eq`.
    Eq,
    /// `atomik.distortion`.
    Distortion,
    /// `atomik.guitar-amp`: the speakers of the selected cabinet with the mic, in/out and gate meters.
    GuitarAmp,
    /// `atomik.modulation`.
    Modulation,
    /// `atomik.mastering`.
    Mastering,
    /// `atomik.character`: the 4 x 4 preset matrix, a screen and a peak meter.
    Character,
    /// `atomik.spectrum`: the spectrum analyser (fed by the app from the device's scope tap).
    Spectrum,
    /// `atomik.phase-meter`: goniometer, correlation, balance, width, levels and loudness.
    PhaseMeter,
    /// `atomik.audio-track`: the input settings and the tuner display.
    AudioTrack,
}

impl FxKind {
    /// Every kind.
    pub const ALL: [FxKind; 11] = [
        FxKind::Reverb,
        FxKind::Compressor,
        FxKind::Eq,
        FxKind::Distortion,
        FxKind::GuitarAmp,
        FxKind::Modulation,
        FxKind::Mastering,
        FxKind::Character,
        FxKind::Spectrum,
        FxKind::PhaseMeter,
        FxKind::AudioTrack,
    ];

    /// The ten effects: every kind but the audio track's, which has the BYPASS switch of an
    /// effect, the meters of one and a plain "meters change the painting" behaviour that the
    /// audio track's tuner does not share.
    pub const EFFECTS: [FxKind; 10] = [
        FxKind::Reverb,
        FxKind::Compressor,
        FxKind::Eq,
        FxKind::Distortion,
        FxKind::GuitarAmp,
        FxKind::Modulation,
        FxKind::Mastering,
        FxKind::Character,
        FxKind::Spectrum,
        FxKind::PhaseMeter,
    ];

    /// The kind of a device type id.
    pub fn of(type_id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.type_id() == type_id)
    }

    /// The device type id.
    pub fn type_id(self) -> &'static str {
        match self {
            FxKind::Reverb => "atomik.reverb",
            FxKind::Compressor => "atomik.compressor",
            FxKind::Eq => "atomik.eq",
            FxKind::Distortion => "atomik.distortion",
            FxKind::GuitarAmp => "atomik.guitar-amp",
            FxKind::Modulation => "atomik.modulation",
            FxKind::Mastering => "atomik.mastering",
            FxKind::Character => "atomik.character",
            FxKind::Spectrum => "atomik.spectrum",
            FxKind::PhaseMeter => "atomik.phase-meter",
            FxKind::AudioTrack => "atomik.audio-track",
        }
    }

    /// How many meter values the device publishes (`Device::meter_values`).
    pub fn meter_count(self) -> usize {
        match self {
            FxKind::Reverb => 3,
            FxKind::Compressor => 3,
            FxKind::Eq => EQ_METERS,
            FxKind::Distortion => 2,
            FxKind::GuitarAmp => 3,
            FxKind::Modulation => 0,
            FxKind::Mastering => 8,
            FxKind::Character => CHARACTER_METERS,
            FxKind::Spectrum => SPECTRUM_METERS,
            FxKind::PhaseMeter => phase::COUNT,
            FxKind::AudioTrack => 0,
        }
    }
}

// ------------------------------------------------------------------ meters

/// Lowest level shown for peaks, tails and analyser bands (dB).
pub const LEVEL_FLOOR_DB: f32 = -90.0;
/// Number of analyser bands of the EQ (`meter_values[0..16]`).
pub const EQ_BANDS_ANALYSER: usize = 16;
/// The EQ publishes 16 band energies and the output peaks L and R.
pub const EQ_METERS: usize = EQ_BANDS_ANALYSER + 2;
/// Peak-type values fall by this many dB per feed (the views have no clock, so the release is
/// per call: about 90 dB/s at 60 feeds per second).
pub const PEAK_FALL_DB: f32 = 1.5;
/// Gain reduction falls by this many dB per feed.
pub const GR_FALL_DB: f32 = 0.6;
/// Analyser bands fall by this many dB per feed.
pub const BAND_FALL_DB: f32 = 2.0;
/// Largest gain reduction drawn (dB).
pub const GR_SCALE_DB: f32 = 24.0;
/// Character publishes `[in peak, out peak, transform_eff, activity]`.
pub const CHARACTER_METERS: usize = 4;
/// The spectrum analyser publishes the recent peaks of its two inputs.
pub const SPECTRUM_METERS: usize = 2;

/// The phase meter's published values (`atomik_devices::phase_meter::value`; a test pins the
/// two together): which index holds what.
pub mod phase {
    /// Number of values.
    pub const COUNT: usize = 16;
    /// Recent peak, left (linear).
    pub const PEAK_L: usize = 0;
    /// Recent peak, right.
    pub const PEAK_R: usize = 1;
    /// Held peak, left.
    pub const HOLD_L: usize = 2;
    /// Held peak, right.
    pub const HOLD_R: usize = 3;
    /// RMS, left (linear).
    pub const RMS_L: usize = 4;
    /// RMS, right.
    pub const RMS_R: usize = 5;
    /// Sample peak since reset, dBFS.
    pub const SAMPLE_PEAK_DB: usize = 6;
    /// True peak since reset, dBTP.
    pub const TRUE_PEAK_DB: usize = 7;
    /// Correlation, -1..=1.
    pub const CORRELATION: usize = 8;
    /// Balance, -1 (left) to +1 (right).
    pub const BALANCE: usize = 9;
    /// Width, 0 to 1.
    pub const WIDTH: usize = 10;
    /// Mid RMS (linear).
    pub const MID_RMS: usize = 11;
    /// Side RMS (linear).
    pub const SIDE_RMS: usize = 12;
    /// Momentary loudness, LUFS.
    pub const MOMENTARY: usize = 13;
    /// Short-term loudness, LUFS.
    pub const SHORT_TERM: usize = 14;
    /// Integrated loudness, LUFS.
    pub const INTEGRATED: usize = 15;
}

/// The activity value (0..1) falls by this much per feed: about 0.4 s from full to nothing at
/// 60 feeds per second, long enough for the screen to ease out instead of freezing.
pub const ACTIVITY_FALL: f32 = 0.04;

#[derive(Clone, Copy, PartialEq)]
enum Slot {
    /// Linear amplitude, shown in dB, peak ballistics.
    Peak,
    /// Linear power (`A^2/2`), shown in dB, analyser ballistics.
    Power,
    /// Gain reduction in dB (positive), reduction ballistics.
    Gr,
    /// A level in dB (LUFS, dBTP) shown as published.
    Raw,
    /// A value 0..1 shown as published (Character's effective Transform), no release.
    Unit,
    /// A value 0..1 with a linear release (Character's preset activity).
    Activity,
    /// A value -1..1 shown as published (the phase meter's correlation and balance, which the
    /// device already averages), no release.
    Signed,
}

fn slot(kind: FxKind, i: usize) -> Slot {
    match (kind, i) {
        (FxKind::Compressor, 2) => Slot::Gr,
        (FxKind::Eq, i) if i < EQ_BANDS_ANALYSER => Slot::Power,
        (FxKind::Mastering, 0..=3) => Slot::Raw,
        (FxKind::Mastering, _) => Slot::Gr,
        (FxKind::GuitarAmp, 2) => Slot::Gr,
        (FxKind::Character, 2) => Slot::Unit,
        (FxKind::Character, 3) => Slot::Activity,
        (FxKind::PhaseMeter, phase::SAMPLE_PEAK_DB | phase::TRUE_PEAK_DB) => Slot::Raw,
        (FxKind::PhaseMeter, phase::MOMENTARY..=phase::INTEGRATED) => Slot::Raw,
        (FxKind::PhaseMeter, phase::CORRELATION | phase::BALANCE) => Slot::Signed,
        (FxKind::PhaseMeter, phase::WIDTH) => Slot::Unit,
        _ => Slot::Peak,
    }
}

fn quantise(v: f32, step: f32) -> f32 {
    (v / step).round() * step
}

/// Folds one frame of published device values `raw` into the display values `shown` of a
/// panel (dB, quantised, with per-feed release) and returns whether anything changed. `shown`
/// is resized on the first call and reused afterwards; silence settles at the floor, so a quiet
/// device stops requesting repaints.
pub fn update_display(kind: FxKind, raw: &[f32], shown: &mut Vec<f32>) -> bool {
    let n = kind.meter_count();
    let fresh = shown.len() != n;
    if fresh {
        shown.clear();
        shown.resize(n, LEVEL_FLOOR_DB);
        for (i, v) in shown.iter_mut().enumerate() {
            match slot(kind, i) {
                Slot::Gr => *v = 0.0,
                Slot::Raw => *v = -120.0,
                Slot::Unit | Slot::Activity | Slot::Signed => *v = 0.0,
                _ => {}
            }
        }
    }
    let mut changed = fresh;
    for (i, cell) in shown.iter_mut().enumerate().take(n) {
        let r = raw.get(i).copied().unwrap_or(0.0);
        let r = if r.is_finite() { r } else { 0.0 };
        let (new, fall, step, floor) = match slot(kind, i) {
            Slot::Peak => (to_db(r, 20.0), PEAK_FALL_DB, 0.5, LEVEL_FLOOR_DB),
            Slot::Power => (to_db(r, 10.0), BAND_FALL_DB, 0.5, LEVEL_FLOOR_DB),
            Slot::Gr => (r.clamp(0.0, 96.0), GR_FALL_DB, 0.1, 0.0),
            Slot::Raw => (r.clamp(-120.0, 24.0), f32::INFINITY, 0.1, -120.0),
            // Hundredths, as a fraction so a value like 0.7 stays the nearest float to 0.7.
            Slot::Unit => (r.clamp(0.0, 1.0), f32::INFINITY, 0.01, 0.0),
            Slot::Activity => (r.clamp(0.0, 1.0), ACTIVITY_FALL, 0.02, 0.0),
            Slot::Signed => (r.clamp(-1.0, 1.0), f32::INFINITY, 0.01, -1.0),
        };
        let old = *cell;
        let v = if new >= old {
            new
        } else {
            (old - fall).max(new)
        };
        let v = if matches!(slot(kind, i), Slot::Unit | Slot::Activity | Slot::Signed) {
            let n = (1.0f32 / step).round();
            // `+ 0.0` turns a negative zero into a zero, so a value that hovers around nothing
            // does not read as a change.
            (v.max(floor) * n).round() / n + 0.0
        } else {
            quantise(v.max(floor), step)
        };
        if v.to_bits() != old.to_bits() {
            *cell = v;
            changed = true;
        }
    }
    changed
}

fn to_db(linear: f32, scale: f32) -> f32 {
    if linear <= 0.0 {
        LEVEL_FLOOR_DB
    } else {
        (scale * linear.log10()).max(LEVEL_FLOOR_DB)
    }
}

/// Bar height 0..1 of a dB level between `lo` and `hi`.
pub fn level_norm(db: f32, lo: f32, hi: f32) -> f64 {
    f64::from(((db - lo) / (hi - lo)).clamp(0.0, 1.0))
}

// ------------------------------------------------------------------ compressor

/// Gain reduction (dB, >= 0) of the compressor's static curve for a level; the same formula as
/// `atomik_dsp::static_gain_reduction_knee_db` (`limit` is an infinite ratio).
pub fn compressor_gr_db(level_db: f32, threshold: f32, ratio: f32, knee: f32, limit: bool) -> f32 {
    let slope = if limit {
        1.0
    } else {
        1.0 - 1.0 / ratio.max(1.0)
    };
    let over = level_db - threshold;
    let knee = knee.max(0.0);
    let half = knee / 2.0;
    if over.is_nan() || over <= -half {
        0.0
    } else if over < half {
        let k = over + half;
        slope * k * k / (2.0 * knee)
    } else {
        slope * over
    }
}

/// Output level (dB) of the static curve, before make-up gain.
pub fn compressor_out_db(level_db: f32, threshold: f32, ratio: f32, knee: f32, limit: bool) -> f32 {
    level_db - compressor_gr_db(level_db, threshold, ratio, knee, limit)
}

// ------------------------------------------------------------------ EQ

/// The kind of an EQ band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EqShape {
    /// High-pass (band 1).
    LowCut,
    /// Low shelf (band 2).
    LowShelf,
    /// Bell (bands 3 and 4).
    Peak,
    /// High shelf (band 5).
    HighShelf,
    /// Low-pass (band 6).
    HighCut,
}

impl EqShape {
    /// True for the two cuts (no gain, a slope).
    pub fn is_cut(self) -> bool {
        matches!(self, EqShape::LowCut | EqShape::HighCut)
    }
}

/// The shapes of bands 1 to 6.
pub const EQ_SHAPES: [EqShape; 6] = [
    EqShape::LowCut,
    EqShape::LowShelf,
    EqShape::Peak,
    EqShape::Peak,
    EqShape::HighShelf,
    EqShape::HighCut,
];

/// One EQ band as the device reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqBand {
    /// Shape.
    pub shape: EqShape,
    /// Enabled.
    pub on: bool,
    /// Frequency, Hz.
    pub freq: f64,
    /// Gain, dB (ignored by the cuts).
    pub gain: f64,
    /// Q.
    pub q: f64,
    /// Cut slope 0..=2 (12, 24, 48 dB/oct).
    pub slope: usize,
}

/// The param keys of one EQ band.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EqKeys {
    /// `bandN.on`.
    pub on: String,
    /// `bandN.freq`.
    pub freq: String,
    /// `bandN.gain` (not for the cuts).
    pub gain: Option<String>,
    /// `bandN.q`.
    pub q: String,
    /// `lowcut.slope` / `highcut.slope` (cuts only).
    pub slope: Option<String>,
}

/// Param keys of band `i` (0-based).
pub fn eq_keys(i: usize) -> EqKeys {
    let n = i + 1;
    EqKeys {
        on: format!("band{n}.on"),
        freq: format!("band{n}.freq"),
        gain: (!EQ_SHAPES[i].is_cut()).then(|| format!("band{n}.gain")),
        q: format!("band{n}.q"),
        slope: match i {
            0 => Some("lowcut.slope".to_string()),
            5 => Some("highcut.slope".to_string()),
            _ => None,
        },
    }
}

/// The device's defaults (every band flat; bands 1 and 6 off).
pub fn default_eq_bands() -> [EqBand; 6] {
    let b = |i: usize, on: bool, freq: f64, q: f64| EqBand {
        shape: EQ_SHAPES[i],
        on,
        freq,
        gain: 0.0,
        q,
        slope: 0,
    };
    [
        b(0, false, 80.0, FRAC_1_SQRT_2),
        b(1, true, 120.0, FRAC_1_SQRT_2),
        b(2, true, 500.0, 1.0),
        b(3, true, 2500.0, 1.0),
        b(4, true, 8000.0, FRAC_1_SQRT_2),
        b(5, false, 16000.0, FRAC_1_SQRT_2),
    ]
}

/// Reads the six bands from a document device (descriptor defaults for unstored params).
pub fn eq_bands_of(desc: &DeviceDescriptor, device: Option<&RackDevice>) -> [EqBand; 6] {
    let mut bands = default_eq_bands();
    let get = |key: &str| -> Option<f64> {
        let p: &ParamDescriptor = desc.params.iter().find(|p| p.key == key)?;
        Some(f64::from(device.map_or(p.default, |d| current_value(d, p))))
    };
    for (i, band) in bands.iter_mut().enumerate() {
        let keys = eq_keys(i);
        band.on = get(&keys.on).unwrap_or(0.0) >= 0.5;
        band.freq = get(&keys.freq).unwrap_or(band.freq);
        band.q = get(&keys.q).unwrap_or(band.q);
        if let Some(g) = &keys.gain {
            band.gain = get(g).unwrap_or(0.0);
        }
        if let Some(s) = &keys.slope {
            band.slope = (get(s).unwrap_or(0.0).round().max(0.0) as usize).min(2);
        }
    }
    bands
}

const BUTTER_Q: [&[f64]; 3] = [
    &[FRAC_1_SQRT_2],
    &[0.541_196_1, 1.306_563],
    &[0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4],
];

#[derive(Clone, Copy)]
enum Rbj {
    Peak,
    LowShelf,
    HighShelf,
    Lowpass,
    Highpass,
}

/// Magnitude (dB) of one RBJ-cookbook section at `f` Hz: the maths of
/// `atomik_dsp::BiquadCoeffs::design` and `response_db` (same clamps), evaluated directly.
fn rbj_db(shape: Rbj, f0: f64, q: f64, gain_db: f64, f: f64, sr: f64) -> f64 {
    let f0 = if f0.is_nan() {
        0.49 * sr
    } else {
        f0.clamp(10.0, 0.49 * sr)
    };
    let q = if q.is_nan() {
        FRAC_1_SQRT_2
    } else {
        q.clamp(0.1, 40.0)
    };
    let gain_db = if gain_db.is_finite() { gain_db } else { 0.0 };
    let w0 = 2.0 * PI * f0 / sr;
    let (sn, cs) = w0.sin_cos();
    let alpha = sn / (2.0 * q);
    let a = 10f64.powf(gain_db / 40.0);
    let (b0, b1, b2, a0, a1, a2) = match shape {
        Rbj::Peak => (
            1.0 + alpha * a,
            -2.0 * cs,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cs,
            1.0 - alpha / a,
        ),
        Rbj::LowShelf => {
            let k = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) - (a - 1.0) * cs + k),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - k),
                (a + 1.0) + (a - 1.0) * cs + k,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - k,
            )
        }
        Rbj::HighShelf => {
            let k = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) + (a - 1.0) * cs + k),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - k),
                (a + 1.0) - (a - 1.0) * cs + k,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - k,
            )
        }
        Rbj::Lowpass => (
            (1.0 - cs) / 2.0,
            1.0 - cs,
            (1.0 - cs) / 2.0,
            1.0 + alpha,
            -2.0 * cs,
            1.0 - alpha,
        ),
        Rbj::Highpass => (
            (1.0 + cs) / 2.0,
            -(1.0 + cs),
            (1.0 + cs) / 2.0,
            1.0 + alpha,
            -2.0 * cs,
            1.0 - alpha,
        ),
    };
    let w = 2.0 * PI * f / sr;
    let (s1, c1) = w.sin_cos();
    let (s2, c2) = (2.0 * w).sin_cos();
    let nr = b0 + b1 * c1 + b2 * c2;
    let ni = -(b1 * s1 + b2 * s2);
    let dr = a0 + a1 * c1 + a2 * c2;
    let di = -(a1 * s1 + a2 * s2);
    10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()
}

/// Magnitude (dB) of one band at `f` Hz at sample rate `sr`, ignoring its `on` flag.
pub fn eq_band_response_db(b: &EqBand, f: f64, sr: f64) -> f64 {
    match b.shape {
        EqShape::LowShelf => rbj_db(Rbj::LowShelf, b.freq, b.q, b.gain, f, sr),
        EqShape::HighShelf => rbj_db(Rbj::HighShelf, b.freq, b.q, b.gain, f, sr),
        EqShape::Peak => rbj_db(Rbj::Peak, b.freq, b.q, b.gain, f, sr),
        EqShape::LowCut | EqShape::HighCut => {
            let kind = if b.shape == EqShape::LowCut {
                Rbj::Highpass
            } else {
                Rbj::Lowpass
            };
            let qs = BUTTER_Q[b.slope.min(2)];
            let scale = b.q / FRAC_1_SQRT_2;
            qs.iter()
                .enumerate()
                .map(|(i, &q)| {
                    let q = if i + 1 == qs.len() { q * scale } else { q };
                    rbj_db(kind, b.freq, q, 0.0, f, sr)
                })
                .sum()
        }
    }
}

/// Total response (dB) of the enabled bands (output gain excluded).
pub fn eq_response_db(bands: &[EqBand; 6], f: f64, sr: f64) -> f64 {
    bands
        .iter()
        .filter(|b| b.on && (b.shape.is_cut() || b.gain != 0.0))
        .map(|b| eq_band_response_db(b, f, sr))
        .sum()
}

/// Lowest frequency of the EQ plot.
pub const EQ_PLOT_LOW_HZ: f64 = 20.0;
/// Highest frequency of the EQ plot.
pub const EQ_PLOT_HIGH_HZ: f64 = 20_000.0;
/// Gain range of the EQ plot, +- dB.
pub const EQ_PLOT_DB: f64 = 24.0;

/// Frequency of normalised plot position `t` (0..1, log scale).
pub fn eq_freq_at(t: f64) -> f64 {
    EQ_PLOT_LOW_HZ * (EQ_PLOT_HIGH_HZ / EQ_PLOT_LOW_HZ).powf(t.clamp(0.0, 1.0))
}

/// Normalised plot position (0..1) of a frequency.
pub fn eq_pos_of(freq: f64) -> f64 {
    ((freq.max(1e-3) / EQ_PLOT_LOW_HZ).ln() / (EQ_PLOT_HIGH_HZ / EQ_PLOT_LOW_HZ).ln())
        .clamp(0.0, 1.0)
}

/// Centre frequency of analyser band `k` (40 Hz to 16 kHz, log-spaced, as the device).
pub fn analyser_centre(k: usize) -> f64 {
    40.0 * (16_000.0f64 / 40.0).powf(k as f64 / (EQ_BANDS_ANALYSER - 1) as f64)
}

// ------------------------------------------------------------------ distortion

/// Transfer function of the distortion panel's curve `curve` (0 soft, 1 hard, 2 tube, 3
/// foldback, 4 bitcrush, 5 tape, 6 fuzz, 7 modulate, 8 warp, 9 scream) for an input `x` in
/// -1..=1: drive (dB), then bias, then the curve with its value at the bias subtracted, as
/// `atomik.distortion` does (the crush curve quantises the clamped driven sample to `bits` bits).
pub fn distortion_transfer(curve: usize, drive_db: f32, bias: f32, bits: f32, x: f32) -> f32 {
    let u = x * 10f32.powf(drive_db / 20.0) + bias;
    match curve {
        4 => {
            let levels = 2f32.powf(bits.round().clamp(1.0, 16.0) - 1.0);
            (u.clamp(-1.0, 1.0) * levels).round() / levels
        }
        c => shape(c, u) - shape(c, bias),
    }
}

fn shape(curve: usize, u: f32) -> f32 {
    use atomik_dsp::shaper::{CharacterKind, character_curve};
    let character = |kind| character_curve(kind, f64::from(u)) as f32;
    match curve {
        5 => character(CharacterKind::Tape),
        6 => character(CharacterKind::Fuzz),
        7 => character(CharacterKind::Modulate),
        8 => character(CharacterKind::Warp),
        9 => character(CharacterKind::Scream),
        0 => u / (1.0 + u * u).sqrt(),
        1 => u.clamp(-1.0, 1.0),
        2 => {
            if u >= 0.0 {
                u / (1.0 + u * u).sqrt()
            } else {
                let v = 1.25 * u;
                u / (1.0 + v * v).sqrt()
            }
        }
        _ => {
            let x = u.clamp(-1.0e4, 1.0e4) + 3.0;
            let m = x - 4.0 * (x * 0.25).floor();
            (m - 2.0).abs() - 1.0
        }
    }
}

// ------------------------------------------------------------------ reverb

/// The reverb decay sketch: times in seconds on one axis.
#[derive(Debug, Clone, PartialEq)]
pub struct ReverbSketch {
    /// Pre-delay end (the first early tap is later).
    pub pre: f32,
    /// Six early taps: (time, level 0..1).
    pub taps: [(f32, f32); 6],
    /// Time the late tail starts.
    pub tail_start: f32,
    /// Level of the tail start, 0..1.
    pub tail_level: f32,
    /// RT60 in seconds.
    pub decay: f32,
    /// Frozen: the tail does not decay.
    pub frozen: bool,
    /// Length of the time axis.
    pub total: f32,
}

impl ReverbSketch {
    /// Tail level in dB relative to its start at `t` (0 before it starts, -60 at one decay time
    /// after it; flat when frozen).
    pub fn tail_db_at(&self, t: f32) -> f32 {
        if self.frozen || t <= self.tail_start {
            0.0
        } else {
            -60.0 * (t - self.tail_start) / self.decay
        }
    }

    /// Tail amplitude 0..1 at `t` (0 before the tail starts).
    pub fn tail_amp_at(&self, t: f32) -> f32 {
        if t < self.tail_start {
            0.0
        } else {
            self.tail_level * 10f32.powf(self.tail_db_at(t) / 20.0)
        }
    }
}

/// The sketch for the reverb's params: `algorithm` 0 room, 1 hall, 2 plate; pre-delay in ms,
/// `size`, `decay` seconds, `early` balance (0 late only, 1 early only).
pub fn reverb_sketch(
    algorithm: usize,
    predelay_ms: f32,
    size: f32,
    decay: f32,
    early: f32,
    frozen: bool,
) -> ReverbSketch {
    let pre = predelay_ms.max(0.0) / 1000.0;
    let decay = decay.max(0.05);
    let spacing_ms = match algorithm {
        0 => 7.0,
        1 => 14.0,
        _ => 3.0,
    } * (0.5 + size.clamp(0.0, 1.0));
    let early_level = (2.0 * early).clamp(0.0, 1.0);
    let tail_level = (2.0 * (1.0 - early)).clamp(0.0, 1.0);
    let weak = if algorithm == 2 { 0.5 } else { 1.0 };
    let mut taps = [(0.0, 0.0); 6];
    for (k, t) in taps.iter_mut().enumerate() {
        let kf = k as f32;
        *t = (
            pre + spacing_ms * (kf + 1.0) * (1.0 + 0.12 * kf) / 1000.0,
            early_level * weak * 0.9f32.powf(kf) * if k % 2 == 0 { 1.0 } else { 0.8 },
        );
    }
    let tail_start = taps[5].0 + spacing_ms / 1000.0;
    ReverbSketch {
        pre,
        taps,
        tail_start,
        tail_level,
        decay,
        frozen,
        total: tail_start + decay * 1.05,
    }
}

// ------------------------------------------------------------------ modulation

/// The modulation LFO (`atomik_dsp::ParabolicLfo`) at cycle position `phase` (periodic).
pub fn lfo_wave(phase: f32) -> f32 {
    let p = phase - phase.floor();
    let u = 2.0 * p - 1.0;
    4.0 * u * (1.0 - u.abs())
}

/// Names of the modulation `sync` choices 1..=7 (4/4).
pub const SYNC_NAMES: [&str; 7] = ["1/16", "1/8", "1/4", "1/2", "1 bar", "2 bars", "4 bars"];

// ------------------------------------------------------------------ character

/// The number of character presets (the 4 x 4 matrix).
pub const CHARACTER_PRESETS: usize = 16;

/// The keys of the 16 presets in matrix order (row-major, rows run from subtle to extreme); the
/// glyph and the screen scene of a preset are chosen by its index in this list. A test pins it
/// against the device.
pub const CHARACTER_KEYS: [&str; CHARACTER_PRESETS] = [
    "reel",
    "loudness",
    "sparkle",
    "low-end",
    "wide",
    "pocket-radio",
    "camcorder",
    "dusty-groove",
    "lossy",
    "swirl",
    "torn-cone",
    "tin-robot",
    "bit-bender",
    "spin-cycle",
    "ion-sweep",
    "ghost-pipes",
];

/// The display names of the presets in matrix order (the descriptor's choice names; the contact
/// sheet of the screens uses this copy, a test pins it against the device).
pub const CHARACTER_NAMES: [&str; CHARACTER_PRESETS] = [
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

/// Whether a preset is in the long latency class (the spectral stage, only `lossy`). A test pins
/// it against the device's reported latency.
pub fn character_is_long(preset: usize) -> bool {
    preset == 8
}

/// The gain of a Character knob position: `4 x^2` (silence at 0, 0 dB at 0.5, +12 dB at 1), the
/// law of `atomik_devices::character::knob_gain` (the UI reads descriptors only and does not link
/// the devices; a test pins the two together).
pub fn character_gain(x: f32) -> f32 {
    let x = if x.is_nan() { 0.5 } else { x.clamp(0.0, 1.0) };
    4.0 * x * x
}

/// [`character_gain`] in dB (minus infinity at 0).
pub fn character_gain_db(x: f32) -> f32 {
    let g = character_gain(x);
    if g <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * g.log10()
    }
}

/// The dB text of a gain or volume knob position: `"-inf dB"`, `"+0.0 dB"`, `"-12.0 dB"`.
pub fn character_gain_text(x: f32) -> String {
    let db = character_gain_db(x);
    if db == f32::NEG_INFINITY {
        "-inf dB".to_string()
    } else {
        format!("{db:+.1} dB")
    }
}

/// The value text of a Character param that is not shown by the generic unit formatting: the
/// dB of `gain` and `volume`.
pub fn character_value_text(key: &str, plain: f32) -> Option<String> {
    matches!(key, "gain" | "volume").then(|| character_gain_text(plain))
}

/// The range a bipolar full-scale CV sweeps a 0..1 knob over: `knob -+ trim`, clamped.
pub fn cv_span(knob: f32, trim: f32) -> (f32, f32) {
    let knob = if knob.is_nan() {
        0.5
    } else {
        knob.clamp(0.0, 1.0)
    };
    let trim = if trim.is_nan() {
        0.0
    } else {
        trim.clamp(0.0, 1.0)
    };
    ((knob - trim).clamp(0.0, 1.0), (knob + trim).clamp(0.0, 1.0))
}

// ------------------------------------------------------------------ guitar amp

/// How many speakers the cabinet choice `cab` (`1x12 open`, `2x12`, `4x12 closed`, `off`) draws.
pub fn amp_speaker_count(cab: usize) -> usize {
    match cab {
        0 => 1,
        1 => 2,
        2 => 4,
        _ => 0,
    }
}

/// The speakers of a cabinet in the unit square of the panel's plot (x right, y down): the
/// centres and the radius as a fraction of the square's width. The first centre is the one the
/// mic points at.
pub fn amp_speakers(count: usize) -> (&'static [(f64, f64)], f64) {
    match count {
        1 => (&[(0.5, 0.5)], 0.38),
        2 => (&[(0.5, 0.26), (0.5, 0.74)], 0.24),
        4 => (
            &[(0.27, 0.27), (0.73, 0.27), (0.27, 0.73), (0.73, 0.73)],
            0.2,
        ),
        _ => (&[], 0.0),
    }
}

/// Where the mic sits on the first speaker: the offset of its marker from the speaker's centre as
/// a fraction of the speaker's radius, 0 at the centre of the cone (`mic` 0) to 0.9 at its edge.
pub fn amp_mic_offset(mic: f32) -> f64 {
    0.9 * f64::from(if mic.is_finite() {
        mic.clamp(0.0, 1.0)
    } else {
        0.0
    })
}

/// The tooltip of a guitar amp control: what the knob or button does, one short line. `None` for
/// a key the amp does not have (and for `bypass`, which has the effect strip's own tooltip).
pub fn amp_help(key: &str) -> Option<&'static str> {
    Some(match key {
        "setup" => "Factory sound: click for the next, right-click for the previous. One undo step",
        "input" => "Level into the amp: raise it for a quiet pickup",
        "gate" => "Noise gate threshold: quieter than this is silenced. Fully left it is off",
        "tight" => "High-pass before the gain: higher keeps low, fast riffs tight",
        "boost" => "Mid boost into the preamp, like a booster pedal in front of the amp",
        "gain" => "How hard the three preamp stages are driven",
        "bass" => "Low end of the tone stack. Bass, mid and treble affect each other",
        "mid" => "Middle of the tone stack: fills in the scoop between bass and treble",
        "treble" => "High end of the tone stack",
        "presence" => "High end of the power stage: more bite and air",
        "depth" => "Low end of the power stage: more thump",
        "sag" => "How much a loud passage squashes the power stage, and a note blooms back",
        "master" => "Drive into the power stage: more is louder and more compressed",
        "level" => "Output volume, after the cabinet and the effects",
        "cab" => "Cabinet: 1x12 open, 2x12, 4x12 closed, or none",
        "mic" => "Microphone position, from the centre of the cone (bright) to its edge (dark)",
        "chorus.on" => "Switch the chorus on or off",
        "chorus.mix" => "How much of the chorus is heard",
        "chorus.rate" => "Speed of the chorus sweep",
        "chorus.depth" => "Width of the chorus sweep",
        "delay.on" => "Switch the delay on or off",
        "delay.time" => "Delay time in ms, used when Sync is free",
        "delay.sync" => "Delay time as a note value at the project tempo, or free",
        "delay.feedback" => "How many repeats: the part of each echo fed back",
        "delay.tone" => "Brightness of the repeats: lower is darker",
        "delay.mix" => "How much of the delay is added to the sound",
        "reverb.on" => "Switch the reverb on or off",
        "reverb.size" => "Size of the reverb space",
        "reverb.decay" => "How long the reverb rings, in seconds",
        "reverb.mix" => "How much of the reverb is added to the sound",
        _ => return None,
    })
}
