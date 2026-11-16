//! The maths of the tonal balance meter (`atomik.tonal-balance`): where the spectral shape of a
//! mix sits against a target, region by region. Pure functions and small accumulators; the app
//! feeds them the band levels of [`crate::spectrum`] and keeps the targets.
//!
//! # Bands and regions
//!
//! Everything works on the 31 one-third-octave bands of [`crate::spectrum`] (20 Hz to 20 kHz).
//! The four **regions** of the meters group them by centre frequency:
//!
//! | region | name | crossover | bands (centres) |
//! |---|---|---|---|
//! | 0 | LOW | below 150 Hz | 20 Hz to 125 Hz (9 bands) |
//! | 1 | LOW-MID | 150 Hz to 1 kHz | 160 Hz to 800 Hz (8 bands) |
//! | 2 | HIGH-MID | 1 kHz to 5 kHz | 1 kHz to 4 kHz (7 bands) |
//! | 3 | HIGH | 5 kHz and above | 5 kHz to 20 kHz (7 bands) |
//!
//! A region's level is the energy of its bands (the sum of their power), in dB.
//!
//! # Shape, not level
//!
//! A reading is **loudness-normalised**: the power of the 31 bands is divided by the K-weighted
//! energy of the *core* bands, 160 Hz to 4 kHz (every band weighted by the BS.1770 K-weighting at
//! its centre, [`crate::k_weighting`]), where the loudness of a mix is decided. A louder or
//! quieter version of the same mix reads the same, and every curve, target and region level is
//! "dB relative to the core". Anchoring on the core, rather than on everything, keeps an excess
//! of bass or treble on its own meter instead of pulling every other meter down. A region's
//! *deviation* is the mix's region level minus the target's, in dB: positive is too loud,
//! negative too quiet.
//!
//! # Targets
//!
//! A [`Target`] is a band curve (normalised dB), a tolerance per band (the half-height of the
//! target *range* drawn around it) and a tolerance per region (the half-width of the range a
//! meter accepts).
//!
//! * **Built-in targets** are our own shapes, a spectral tilt with a gentle low shelf and an
//!   optional high shelf on the 1/3-octave band levels ([`shape_db`]): with `x = log2(f / 1 kHz)`,
//!   `L(f) = tilt * x + low / (1 + (f / f_low)^2) + high * f^2 / (f^2 + f_high^2)` dB, then
//!   normalised. A flat 1/3-octave band level is pink noise (equal energy per octave); the usual
//!   mix falls about 4.5 dB per octave, which is the tilt of "Balanced". The tolerance of a
//!   built-in is a fixed [`BUILTIN_TOL_DB`] (and [`BUILTIN_EDGE_TOL_DB`] on the three outermost
//!   bands at either end of the axis, where music varies most).
//! * **Learned targets** come from observations: the normalised band levels of a stretch of
//!   about three seconds of a reference (or two seconds of the live signal). The curve is the
//!   power mean of the observations, normalised again; the tolerance is half the standard
//!   deviation of the observations' dB values in each band, and in each region, kept between
//!   [`MIN_TOL_DB`] and [`MAX_TOL_DB`] (so the spread of one song, or of several, sets how wide
//!   its range is); with fewer than two observations it is [`LONE_TOL_DB`]. The [`Learner`]
//!   holds running sums, so it takes any length of material in constant memory.

use crate::k_weighting::k_weighting_power;
use crate::spectrum::{
    BAND_COUNT, ChannelMode, FLOOR_DB, Settings, Smoothing, Spectrum, Weighting, WindowKind,
    band_centres,
};

/// The number of regions.
pub const REGIONS: usize = 4;
/// The crossover frequencies between the regions in Hz: a band belongs to the region its centre
/// falls in.
pub const CROSSOVER_HZ: [f32; 3] = [150.0, 1000.0, 5000.0];
/// The names of the regions, lowest first.
pub const REGION_NAMES: [&str; REGIONS] = ["LOW", "LOW-MID", "HIGH-MID", "HIGH"];

/// Tolerance of a built-in target, dB.
pub const BUILTIN_TOL_DB: f32 = 3.0;
/// Tolerance of a built-in target on its three outermost bands at each end, dB.
pub const BUILTIN_EDGE_TOL_DB: f32 = 4.5;
/// The narrowest learned tolerance, dB.
pub const MIN_TOL_DB: f32 = 2.0;
/// The widest learned tolerance, dB.
pub const MAX_TOL_DB: f32 = 6.0;
/// The tolerance of a target learned from a single observation, dB.
pub const LONE_TOL_DB: f32 = 3.0;
/// The size of the FFT behind a reading.
pub const FFT_SIZE: usize = 16_384;
/// Seconds of a file one observation spans.
pub const FILE_OBSERVATION_S: f32 = 3.0;
/// Seconds of live audio between two observations of a live learn.
pub const LIVE_OBSERVATION_S: f32 = 2.0;
/// The K-weighted core power below which an observation is silence (about -90 dB).
const SILENCE: f64 = 1e-9;
/// The loudest and the quietest level a band of a stored target may hold, dB.
const LEVEL_LIMITS: (f32, f32) = (-90.0, 30.0);

/// The region (0..4) of band `i` (0..31).
pub fn region_of_band(i: usize) -> usize {
    let hz = band_centres()[i.min(BAND_COUNT - 1)];
    CROSSOVER_HZ.iter().take_while(|c| hz >= **c).count()
}

/// The bands of region `r`.
pub fn region_bands(r: usize) -> std::ops::Range<usize> {
    let first = (0..BAND_COUNT).find(|i| region_of_band(*i) == r);
    let end = (0..BAND_COUNT).rfind(|i| region_of_band(*i) == r);
    match (first, end) {
        (Some(a), Some(b)) => a..b + 1,
        _ => 0..0,
    }
}

fn db_of(p: f64) -> f32 {
    (10.0 * p.max(1e-20).log10()) as f32
}

/// The K-weighting power factor of every band.
fn k_gains() -> [f64; BAND_COUNT] {
    let centres = band_centres();
    std::array::from_fn(|i| k_weighting_power(f64::from(centres[i]), 48_000.0))
}

/// The bands whose K-weighted energy is the reference level: 160 Hz to 4 kHz.
pub const CORE_BANDS: std::ops::Range<usize> = 9..24;

/// Normalises the band levels `levels_db` (a spectrum's [`crate::spectrum::Spectrum::band_levels`],
/// [`FLOOR_DB`] for silence) to a K-weighted core energy ([`CORE_BANDS`]) of 0 dB; `None` for
/// silence.
pub fn normalise(levels_db: &[f32; BAND_COUNT]) -> Option<[f32; BAND_COUNT]> {
    let power: [f64; BAND_COUNT] =
        std::array::from_fn(|i| 10f64.powf(f64::from(levels_db[i]) / 10.0));
    let k = k_gains();
    let total: f64 = CORE_BANDS.map(|i| power[i] * k[i]).sum();
    if !total.is_finite() || total < SILENCE {
        return None;
    }
    Some(std::array::from_fn(|i| db_of(power[i] / total)))
}

/// The energy of each region of normalised band levels, in dB.
pub fn region_levels(norm_db: &[f32; BAND_COUNT]) -> [f32; REGIONS] {
    std::array::from_fn(|r| {
        let sum: f64 = region_bands(r)
            .map(|i| 10f64.powf(f64::from(norm_db[i]) / 10.0))
            .sum();
        db_of(sum)
    })
}

/// A tonal balance target (module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The band curve in dB, normalised.
    pub bands: [f32; BAND_COUNT],
    /// The half-height of the range around each band, dB.
    pub tol: [f32; BAND_COUNT],
    /// The half-width of the range of each region meter, dB.
    pub region_tol: [f32; REGIONS],
}

impl Target {
    /// The region levels of the target in dB.
    pub fn regions(&self) -> [f32; REGIONS] {
        region_levels(&self.bands)
    }

    /// The same target with every number made finite and kept inside what a curve and a range
    /// can hold (for values read from a file).
    pub fn sanitised(&self) -> Target {
        let level = |v: f32| {
            if v.is_finite() {
                v.clamp(LEVEL_LIMITS.0, LEVEL_LIMITS.1)
            } else {
                LEVEL_LIMITS.0
            }
        };
        let tol = |v: f32| {
            if v.is_finite() {
                v.clamp(1.0, 12.0)
            } else {
                LONE_TOL_DB
            }
        };
        Target {
            bands: self.bands.map(level),
            tol: self.tol.map(tol),
            region_tol: self.region_tol.map(tol),
        }
    }
}

/// The names of the built-in targets.
pub const BUILTIN_NAMES: [&str; 4] = ["Balanced", "Bright", "Warm", "Bass-heavy"];

/// The parameters of a built-in shape (module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    /// Slope in dB per octave of the band level (negative falls towards the treble).
    pub tilt: f32,
    /// The lift of the low shelf at the bottom of the axis, dB.
    pub low_db: f32,
    /// The corner of the low shelf, Hz.
    pub low_hz: f32,
    /// The lift of the high shelf at the top of the axis, dB (negative cuts).
    pub high_db: f32,
    /// The corner of the high shelf, Hz.
    pub high_hz: f32,
}

/// The shapes of the built-in targets, in the order of [`BUILTIN_NAMES`].
pub const SHAPES: [Shape; 4] = [
    // Balanced: the usual mix, 4.5 dB per octave down with a gentle lift at the bottom.
    Shape {
        tilt: -4.5,
        low_db: 2.0,
        low_hz: 100.0,
        high_db: 0.0,
        high_hz: 5000.0,
    },
    // Bright: a flatter fall and a lift above 5 kHz.
    Shape {
        tilt: -4.0,
        low_db: 0.0,
        low_hz: 100.0,
        high_db: 3.0,
        high_hz: 5000.0,
    },
    // Warm: a steeper fall, a lift in the low mids and a cut above 6 kHz.
    Shape {
        tilt: -5.0,
        low_db: 1.5,
        low_hz: 150.0,
        high_db: -2.0,
        high_hz: 6000.0,
    },
    // Bass-heavy: the usual fall with a strong low shelf.
    Shape {
        tilt: -4.5,
        low_db: 6.0,
        low_hz: 200.0,
        high_db: 0.0,
        high_hz: 5000.0,
    },
];

/// The level in dB of `shape` at `hz` before normalising (module docs).
pub fn shape_db(shape: &Shape, hz: f32) -> f32 {
    let f = f64::from(hz.max(1.0));
    let x = (f / 1000.0).log2();
    let low = f64::from(shape.low_db) / (1.0 + (f / f64::from(shape.low_hz)).powi(2));
    let f2 = f * f;
    let high = f64::from(shape.high_db) * f2 / (f2 + f64::from(shape.high_hz).powi(2));
    (f64::from(shape.tilt) * x + low + high) as f32
}

/// Built-in target `i` (the index into [`BUILTIN_NAMES`], clamped).
pub fn builtin(i: usize) -> Target {
    let shape = &SHAPES[i.min(SHAPES.len() - 1)];
    let centres = band_centres();
    let raw: [f32; BAND_COUNT] = std::array::from_fn(|b| shape_db(shape, centres[b]));
    let bands = normalise(&raw).unwrap_or([0.0; BAND_COUNT]);
    let tol = std::array::from_fn(|b| {
        if !(3..BAND_COUNT - 3).contains(&b) {
            BUILTIN_EDGE_TOL_DB
        } else {
            BUILTIN_TOL_DB
        }
    });
    Target {
        bands,
        tol,
        region_tol: [BUILTIN_TOL_DB; REGIONS],
    }
}

/// One comparison of a mix with a target.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// The normalised band levels of the mix, dB.
    pub bands: [f32; BAND_COUNT],
    /// The region levels of the mix, dB.
    pub regions: [f32; REGIONS],
    /// The region levels of the target, dB.
    pub target_regions: [f32; REGIONS],
    /// Mix minus target per region, dB: positive is too loud.
    pub deviation: [f32; REGIONS],
}

/// Compares the band levels of a mix with `target`; `None` for silence.
pub fn compare(levels_db: &[f32; BAND_COUNT], target: &Target) -> Option<Reading> {
    let bands = normalise(levels_db)?;
    let regions = region_levels(&bands);
    let target_regions = target.regions();
    let deviation = std::array::from_fn(|r| regions[r] - target_regions[r]);
    Some(Reading {
        bands,
        regions,
        target_regions,
        deviation,
    })
}

impl Reading {
    /// Whether region `r` sits inside the range `target` accepts.
    pub fn in_range(&self, target: &Target, r: usize) -> bool {
        self.deviation[r].abs() <= target.region_tol[r]
    }
}

/// Collects observations of reference material and builds a [`Target`] from them (module docs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Learner {
    count: u32,
    power: [f64; BAND_COUNT],
    db: [f64; BAND_COUNT],
    db2: [f64; BAND_COUNT],
    region_db: [f64; REGIONS],
    region_db2: [f64; REGIONS],
}

impl Learner {
    /// An empty learner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The number of observations taken.
    pub fn len(&self) -> usize {
        self.count as usize
    }

    /// No observation yet.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Takes the band levels of one stretch of material as an observation; false (nothing taken)
    /// for silence.
    pub fn push(&mut self, levels_db: &[f32; BAND_COUNT]) -> bool {
        let Some(bands) = normalise(levels_db) else {
            return false;
        };
        let regions = region_levels(&bands);
        self.count += 1;
        for (i, v) in bands.iter().enumerate() {
            let v = f64::from(*v);
            self.power[i] += 10f64.powf(v / 10.0);
            self.db[i] += v;
            self.db2[i] += v * v;
        }
        for (r, v) in regions.iter().enumerate() {
            let v = f64::from(*v);
            self.region_db[r] += v;
            self.region_db2[r] += v * v;
        }
        true
    }

    /// Takes the observations of another learner (several files learned on several workers).
    pub fn merge(&mut self, other: &Learner) {
        self.count += other.count;
        for i in 0..BAND_COUNT {
            self.power[i] += other.power[i];
            self.db[i] += other.db[i];
            self.db2[i] += other.db2[i];
        }
        for r in 0..REGIONS {
            self.region_db[r] += other.region_db[r];
            self.region_db2[r] += other.region_db2[r];
        }
    }

    /// The target the observations make; `None` without any.
    pub fn build(&self) -> Option<Target> {
        if self.count == 0 {
            return None;
        }
        let n = f64::from(self.count);
        let mean_power: [f32; BAND_COUNT] = std::array::from_fn(|i| db_of(self.power[i] / n));
        let bands = normalise(&mean_power)?;
        let tol_of = |sum: f64, sum2: f64| -> f32 {
            if self.count < 2 {
                return LONE_TOL_DB;
            }
            let mean = sum / n;
            let var = (sum2 / n - mean * mean).max(0.0);
            ((0.5 * var.sqrt()) as f32).clamp(MIN_TOL_DB, MAX_TOL_DB)
        };
        Some(Target {
            bands,
            tol: std::array::from_fn(|i| tol_of(self.db[i], self.db2[i])),
            region_tol: std::array::from_fn(|r| tol_of(self.region_db[r], self.region_db2[r])),
        })
    }
}

/// Feeds `learner` with the observations of a stereo file: the channels cut into stretches of
/// [`FILE_OBSERVATION_S`], each the power average of its [`FFT_SIZE`]-point windows (half
/// overlapping), summed over left and right as the spectrum meter's sum channel. `progress` is
/// called with the fraction done after every stretch and stops the analysis by returning false.
/// Returns the number of observations taken.
pub fn learn_from_channels(
    learner: &mut Learner,
    left: &[f32],
    right: &[f32],
    sample_rate: f32,
    mut progress: impl FnMut(f32) -> bool,
) -> usize {
    let len = left.len().min(right.len());
    let hop = FFT_SIZE / 2;
    let stretch = ((FILE_OBSERVATION_S * sample_rate) as usize).max(FFT_SIZE);
    if len < FFT_SIZE || sample_rate < 8000.0 {
        return 0;
    }
    let settings = Settings {
        size: FFT_SIZE,
        window: WindowKind::Hann,
        channel: ChannelMode::Sum,
        bands: true,
        averaging_ms: 0.0,
        hold_ms: 0.0,
        decay_db_per_s: 0.0,
        tilt_db_per_oct: 0.0,
        smoothing: Smoothing::Off,
        weighting: Weighting::Flat,
    };
    let mut spectrum = Spectrum::new();
    let mut taken = 0;
    let mut start = 0;
    while start + FFT_SIZE <= len {
        let end = (start + stretch).min(len);
        let mut power = [0.0f64; BAND_COUNT];
        let mut windows = 0u32;
        let mut at = start;
        while at + FFT_SIZE <= end {
            spectrum.update(
                &left[at..at + FFT_SIZE],
                &right[at..at + FFT_SIZE],
                FFT_SIZE as u64,
                sample_rate,
                &settings,
                0,
            );
            let levels = spectrum.band_levels();
            if levels.iter().any(|v| *v > FLOOR_DB + 50.0) {
                for (p, v) in power.iter_mut().zip(&levels) {
                    *p += 10f64.powf(f64::from(*v) / 10.0);
                }
                windows += 1;
            }
            at += hop;
        }
        // A last stretch of fewer than two windows is too short to say anything.
        if windows >= 2 || (windows == 1 && end - start >= stretch) {
            let mean: [f32; BAND_COUNT] =
                std::array::from_fn(|i| db_of(power[i] / f64::from(windows)));
            if learner.push(&mean) {
                taken += 1;
            }
        }
        start = end;
        if !progress(start as f32 / len as f32) {
            break;
        }
    }
    taken
}

/// The 31 band levels of a stereo signal in one analysis (no averaging), for tests and probes.
pub fn band_levels_of(left: &[f32], right: &[f32], sample_rate: f32) -> [f32; BAND_COUNT] {
    let n = left.len().min(right.len()).min(FFT_SIZE);
    let settings = Settings {
        size: FFT_SIZE,
        averaging_ms: 0.0,
        hold_ms: 0.0,
        bands: true,
        ..Settings::default()
    };
    let mut s = Spectrum::new();
    s.update(
        &left[left.len() - n..],
        &right[right.len() - n..],
        n as u64,
        sample_rate,
        &settings,
        0,
    );
    s.band_levels()
}

#[cfg(test)]
mod tests;
