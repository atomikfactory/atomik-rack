//! The correction curve of the Headphone Correction device and the minimum-phase FIR made from
//! it, computed in small steps so the audio thread can do one step per batch.
//!
//! # From data to a curve ([`Curve`])
//!
//! Everything works on one dB grid of [`GRID_LEN`] points, 96 per octave from [`GRID_F0`] (20 Hz)
//! up to 20.48 kHz; outside it the curve is held at its end values.
//!
//! 1. **Source.** A response is read at the grid frequencies (linear in dB over log frequency,
//!    held beyond its first and last point); a parametric file is the sum of the magnitude
//!    responses of its bands (the same biquads the cookbook gives, evaluated at the device's
//!    sample rate).
//! 2. **Smoothing.** A response (and a correction curve) is averaged over 1/12 octave (8 grid
//!    points). A parametric file is not smoothed.
//! 3. **Correction.** A measured response `R` becomes `target - (R - R(1 kHz))`: the response is
//!    first levelled to 0 dB at 1 kHz. A correction curve and a parametric file are used as they
//!    are.
//! 4. **Amount and simulate.** The correction in dB is multiplied by `amount` (0..2) and negated
//!    when `simulate` is on (the headphone's own deviation from the target instead of its
//!    correction), then limited to +-[`LIMIT_DB`].
//! 5. **Preamp.** The largest boost is `max(0, max(curve))`. With `auto_gain` the preamp is minus
//!    that boost, so the curve never exceeds 0 dB. Without it the preamp is the parametric file's
//!    own `Preamp` (times `amount`, negated when simulating) and 0 for a response.
//!
//! # The studio target
//!
//! `T(f) = 3 / (1 + (f / 120)^2) - clamp(log2(f / 1000), 0, log2(10))` dB: a gentle low shelf of
//! +3 dB at the bottom (+1.5 dB at 120 Hz, under +0.3 dB above 400 Hz) and a slope of -1 dB per
//! octave above 1 kHz that reaches -3.3 dB at 10 kHz and stays there. The flat target is 0 dB.
//!
//! # From a curve to an FIR ([`Design`])
//!
//! The grid is resampled onto the `M / 2 + 1` bins of an `M`-point FFT (`M` = [`FFT_SIZE`]), the
//! natural log of the magnitude goes through the real-cepstrum method (inverse FFT, fold the
//! anti-causal half onto the causal half, FFT, exponentiate, inverse FFT) and the first
//! [`TAPS`] samples, with the last eighth faded out, are the minimum-phase impulse response,
//! scaled by the preamp. A flat curve becomes an exact unit impulse. The steps are `begin`, the
//! grid in chunks, the shaping, the bins in chunks and three FFTs: each call of [`Design::step`]
//! does at most one of them.
//!
//! Only `+ - * /` and the deterministic `exp2`, `log2` and sine of `atomik_dsp` are used on the
//! way (plus the cookbook coefficients of `BiquadCoeffs`), so a curve is the same on every
//! platform.

use atomik_device_sdk::headphone::{FilterKind, HeadphoneBody, HeadphoneData};
use atomik_dsp::{BiquadCoeffs, BiquadShape, Fft, exp2_det, exp2_det_f64, log2_det, sin_cycles};

/// Grid points per octave.
pub const GRID_PER_OCTAVE: usize = 96;
/// Octaves of the grid (20 Hz up to 20.48 kHz).
pub const GRID_OCTAVES: usize = 10;
/// Points of the grid.
pub const GRID_LEN: usize = GRID_PER_OCTAVE * GRID_OCTAVES + 1;
/// Frequency of the first grid point, Hz.
pub const GRID_F0: f64 = 20.0;
/// The most the curve boosts or cuts, dB.
pub const LIMIT_DB: f32 = 20.0;
/// Length of the FIR in samples.
pub const TAPS: usize = 4096;
/// Size of the FFT the design runs.
pub const FFT_SIZE: usize = 4 * TAPS;
/// Smoothing width in grid points (1/12 octave).
const SMOOTH: usize = GRID_PER_OCTAVE / 12;
/// The grid position of 1 kHz (`96 * log2(1000 / 20)`).
const GRID_1K: f64 = 541.810_19;
/// Bins converted per step.
const BIN_CHUNK: usize = 4096;
/// `ln(10) / 20`.
const LN_10_20: f64 = std::f64::consts::LN_10 / 20.0;
/// `log2(10) / 20`.
const LOG2_10_20: f32 = std::f32::consts::LOG2_10 / 20.0;
/// `10 * log10(2)`.
const DB_PER_OCTAVE_POWER: f64 = 10.0 * std::f64::consts::LOG10_2;
/// `log2(10)`.
const LOG2_10: f32 = std::f32::consts::LOG2_10;

/// What a CSV or text response means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataKind {
    /// The measured response of the headphones: the correction is `target - response`.
    Response,
    /// The correction curve itself.
    Correction,
}

/// The target a measured response is brought to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// 0 dB everywhere.
    Flat,
    /// The studio tilt (see the module docs).
    Studio,
}

impl Target {
    /// The target level at `freq` Hz, dB.
    pub fn db_at(self, freq: f64) -> f32 {
        match self {
            Self::Flat => 0.0,
            Self::Studio => {
                let f = freq as f32;
                let shelf = 3.0 / (1.0 + (f / 120.0) * (f / 120.0));
                let slope = log2_det(f / 1000.0).clamp(0.0, LOG2_10);
                shelf - slope
            }
        }
    }
}

/// Everything besides the data and the sample rate that shapes the curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// What a response in the data means.
    pub kind: DataKind,
    /// The target of a measured response.
    pub target: Target,
    /// How much of the correction is applied, 0..=2.
    pub amount: f32,
    /// Apply the inverse instead.
    pub simulate: bool,
    /// Take the preamp from the largest boost.
    pub auto_gain: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            kind: DataKind::Response,
            target: Target::Flat,
            amount: 1.0,
            simulate: false,
            auto_gain: true,
        }
    }
}

/// Linear gain of `db`.
fn lin(db: f32) -> f32 {
    exp2_det(db * LOG2_10_20)
}

/// The frequency of grid point `k`, Hz.
pub fn grid_freq(k: usize) -> f64 {
    GRID_F0 * exp2_det_f64(k as f64 / GRID_PER_OCTAVE as f64)
}

/// The grid position (fractional) of `freq` Hz.
fn grid_pos(freq: f32) -> f32 {
    log2_det(freq / GRID_F0 as f32) * GRID_PER_OCTAVE as f32
}

/// Magnitude response of a biquad in dB at `cycles` (frequency over sample rate).
fn biquad_db(c: &BiquadCoeffs, cycles: f64) -> f32 {
    let (s1, c1) = (sin_cycles(cycles), sin_cycles(cycles + 0.25));
    let (s2, c2) = (sin_cycles(2.0 * cycles), sin_cycles(2.0 * cycles + 0.25));
    let nr = c.b0 + c.b1 * c1 + c.b2 * c2;
    let ni = -(c.b1 * s1 + c.b2 * s2);
    let dr = 1.0 + c.a1 * c1 + c.a2 * c2;
    let di = -(c.a1 * s1 + c.a2 * s2);
    let ratio = (nr * nr + ni * ni) / (dr * dr + di * di).max(1.0e-30);
    (DB_PER_OCTAVE_POWER * f64::from(log2_det(ratio as f32))) as f32
}

/// Linear interpolation of `vals` (at the rising positions `pos`) at grid position `g`, held at
/// both ends; `seg` is a cursor that only moves forward for rising `g`.
fn interp(pos: &[f32], vals: &[f32], seg: &mut usize, g: f32) -> f32 {
    let n = pos.len();
    if g <= pos[0] {
        return vals[0];
    }
    if g >= pos[n - 1] {
        return vals[n - 1];
    }
    while *seg + 2 < n && pos[*seg + 1] <= g {
        *seg += 1;
    }
    let (p0, p1) = (pos[*seg], pos[*seg + 1]);
    let t = (g - p0) / (p1 - p0);
    vals[*seg] + (vals[*seg + 1] - vals[*seg]) * t
}

/// The correction curve on the grid, built in chunks (see the module docs).
#[derive(Debug, Clone)]
pub struct Curve {
    sample_rate: f64,
    settings: Settings,
    ch: usize,
    /// Grid positions of the response points.
    pos: Vec<f32>,
    coeffs: Vec<BiquadCoeffs>,
    next: usize,
    seg: usize,
    /// The source on the grid: the smoothed response (levelled at 1 kHz for a measured one), the
    /// correction curve or the parametric sum, dB.
    pub source: Vec<f32>,
    /// The target on the grid (0 for data that is not a measured response), dB.
    pub target: Vec<f32>,
    /// The finished curve (amount, simulate and limit applied), dB.
    pub curve: Vec<f32>,
    /// The largest boost of the curve (0 or more), dB.
    pub max_boost_db: f32,
    /// The preamp, dB (0 or less with `auto_gain`).
    pub preamp_db: f32,
    /// The curve is 0 dB everywhere.
    pub flat: bool,
}

impl Default for Curve {
    fn default() -> Self {
        Self::new()
    }
}

impl Curve {
    /// An empty curve; allocates its arrays (non-RT).
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            settings: Settings::default(),
            ch: 0,
            pos: Vec::with_capacity(atomik_device_sdk::headphone::MAX_POINTS),
            coeffs: Vec::with_capacity(atomik_device_sdk::headphone::MAX_FILTERS),
            next: GRID_LEN,
            seg: 0,
            source: vec![0.0; GRID_LEN],
            target: vec![0.0; GRID_LEN],
            curve: vec![0.0; GRID_LEN],
            max_boost_db: 0.0,
            preamp_db: 0.0,
            flat: true,
        }
    }

    /// Starts a curve of channel `ch` (0 left, 1 right) of `data`. Does not allocate.
    pub fn begin(&mut self, data: &HeadphoneData, ch: usize, s: &Settings, sample_rate: f32) {
        self.sample_rate = f64::from(sample_rate.max(8_000.0));
        self.settings = *s;
        self.ch = ch;
        self.next = 0;
        self.seg = 0;
        self.pos.clear();
        self.coeffs.clear();
        match &data.body {
            HeadphoneBody::Response(r) => {
                self.pos.extend(r.freq.iter().map(|f| grid_pos(*f)));
            }
            HeadphoneBody::Filters(f) => {
                for b in &f.filters {
                    let shape = match b.kind {
                        FilterKind::Peak => BiquadShape::Peaking,
                        FilterKind::LowShelf => BiquadShape::LowShelf,
                        FilterKind::HighShelf => BiquadShape::HighShelf,
                        FilterKind::LowPass => BiquadShape::Lowpass,
                        FilterKind::HighPass => BiquadShape::Highpass,
                    };
                    self.coeffs.push(BiquadCoeffs::design(
                        shape,
                        f64::from(b.freq),
                        f64::from(b.q),
                        f64::from(b.gain_db),
                        self.sample_rate,
                    ));
                }
            }
        }
    }

    /// Whether the grid still has points to read.
    pub fn grid_pending(&self) -> bool {
        self.next < GRID_LEN
    }

    /// Reads the next chunk of the grid from `data` (the data `begin` was given); true when the
    /// grid is complete.
    pub fn grid_chunk(&mut self, data: &HeadphoneData) -> bool {
        let n = match &data.body {
            HeadphoneBody::Response(_) => 256,
            HeadphoneBody::Filters(_) => (4096 / self.coeffs.len().max(1)).max(8),
        };
        let end = (self.next + n).min(GRID_LEN);
        match &data.body {
            HeadphoneBody::Response(r) => {
                let vals = match (&r.right, self.ch) {
                    (Some(right), 1) => right.as_slice(),
                    _ => r.left.as_slice(),
                };
                for k in self.next..end {
                    self.source[k] = interp(&self.pos, vals, &mut self.seg, k as f32);
                }
            }
            HeadphoneBody::Filters(_) => {
                let nyquist = 0.499 * self.sample_rate;
                for k in self.next..end {
                    let f = grid_freq(k).min(nyquist);
                    let cycles = f / self.sample_rate;
                    let mut sum = 0.0;
                    for c in &self.coeffs {
                        sum += biquad_db(c, cycles);
                    }
                    self.source[k] = sum;
                }
            }
        }
        self.next = end;
        end == GRID_LEN
    }

    /// Smooths, levels, applies amount, simulate and limit, and finds the preamp. Call once the
    /// grid is complete.
    pub fn shape(&mut self, data: &HeadphoneData) {
        let s = self.settings;
        let is_response = matches!(data.body, HeadphoneBody::Response(_));
        if is_response {
            // A moving average of 8 points (the window is cut at the ends).
            self.curve.copy_from_slice(&self.source);
            for k in 0..GRID_LEN {
                let lo = k.saturating_sub(SMOOTH / 2);
                let hi = (k + SMOOTH / 2).min(GRID_LEN);
                let mut sum = 0.0;
                for v in &self.curve[lo..hi] {
                    sum += *v;
                }
                self.source[k] = sum / (hi - lo) as f32;
            }
        }
        let measured = is_response && s.kind == DataKind::Response;
        let reference = if measured {
            let i = GRID_1K as usize;
            let t = (GRID_1K - i as f64) as f32;
            self.source[i] + (self.source[i + 1] - self.source[i]) * t
        } else {
            0.0
        };
        let sign = if s.simulate { -1.0 } else { 1.0 };
        let mut max_boost = 0.0_f32;
        let mut flat = true;
        for k in 0..GRID_LEN {
            let t = if measured {
                s.target.db_at(grid_freq(k))
            } else {
                0.0
            };
            self.target[k] = t;
            let corr = if measured {
                self.source[k] -= reference;
                t - self.source[k]
            } else {
                self.source[k]
            };
            let v = (sign * s.amount * corr).clamp(-LIMIT_DB, LIMIT_DB);
            self.curve[k] = v;
            max_boost = max_boost.max(v);
            flat &= v.abs() < 1.0e-4;
        }
        self.max_boost_db = max_boost;
        self.flat = flat;
        self.preamp_db = if s.auto_gain {
            -max_boost
        } else if let HeadphoneBody::Filters(f) = &data.body {
            sign * s.amount * f.preamp_db
        } else {
            0.0
        };
    }

    /// Runs every step to the end (for a snapshot or a test).
    pub fn compute(&mut self, data: &HeadphoneData, ch: usize, s: &Settings, sample_rate: f32) {
        self.begin(data, ch, s, sample_rate);
        while !self.grid_chunk(data) {}
        self.shape(data);
    }

    /// The curve's level at `freq` Hz (held beyond the grid), dB.
    pub fn curve_at(&self, freq: f32) -> f32 {
        at_grid(&self.curve, freq)
    }
}

/// The value of a grid array at `freq` Hz, linear in log frequency, held at the ends.
fn at_grid(grid: &[f32], freq: f32) -> f32 {
    let g = grid_pos(freq.max(1.0e-3));
    let last = (GRID_LEN - 1) as f32;
    if g <= 0.0 {
        return grid[0];
    }
    if g >= last {
        return grid[GRID_LEN - 1];
    }
    let i = g as usize;
    let t = g - i as f32;
    grid[i] + (grid[i + 1] - grid[i]) * t
}

/// What a panel draws: the curves of `data` at `points` log-spaced frequencies from 20 Hz to
/// 20 kHz, for the channel `ch`, as the device builds them.
#[derive(Debug, Clone, PartialEq)]
pub struct CurveSnapshot {
    /// The frequencies, Hz.
    pub freq: Vec<f32>,
    /// The headphone's own response levelled at 1 kHz (a measured response), or the correction
    /// curve or the parametric sum as given; dB.
    pub response: Vec<f32>,
    /// The target (0 for data that is not a measured response), dB.
    pub target: Vec<f32>,
    /// The correction the device applies (amount, simulate and limit included), dB.
    pub correction: Vec<f32>,
    /// The largest boost of the correction, dB.
    pub max_boost_db: f32,
    /// The preamp the device applies, dB.
    pub preamp_db: f32,
}

/// The curves of `data` for a panel to draw (a pure function of its arguments).
pub fn snapshot(
    data: &HeadphoneData,
    ch: usize,
    s: &Settings,
    sample_rate: f32,
    points: usize,
) -> CurveSnapshot {
    let mut c = Curve::new();
    c.compute(data, ch, s, sample_rate);
    let points = points.max(2);
    let freq: Vec<f32> = (0..points)
        .map(|i| {
            let t = i as f32 / (points - 1) as f32;
            20.0 * exp2_det(t * 9.965_784)
        })
        .collect();
    let sample = |grid: &[f32]| freq.iter().map(|f| at_grid(grid, *f)).collect();
    CurveSnapshot {
        response: sample(&c.source),
        target: sample(&c.target),
        correction: sample(&c.curve),
        freq,
        max_boost_db: c.max_boost_db,
        preamp_db: c.preamp_db,
    }
}

/// Where a [`Design`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Grid,
    Shape,
    Bins,
    Cep1,
    Cep2,
    Cep3,
}

/// The incremental design of one minimum-phase FIR.
#[derive(Debug, Clone)]
pub struct Design {
    curve: Curve,
    fft: Fft,
    re: Vec<f32>,
    im: Vec<f32>,
    stage: Stage,
    bin: usize,
}

impl Default for Design {
    fn default() -> Self {
        Self::new()
    }
}

impl Design {
    /// A design with its buffers (non-RT).
    pub fn new() -> Self {
        Self {
            curve: Curve::new(),
            fft: Fft::new(FFT_SIZE),
            re: vec![0.0; FFT_SIZE],
            im: vec![0.0; FFT_SIZE],
            stage: Stage::Idle,
            bin: 0,
        }
    }

    /// Whether a design is running.
    pub fn busy(&self) -> bool {
        self.stage != Stage::Idle
    }

    /// Abandons a running design.
    pub fn cancel(&mut self) {
        self.stage = Stage::Idle;
    }

    /// The curve of the last finished (or running) design.
    pub fn curve(&self) -> &Curve {
        &self.curve
    }

    /// Starts the design of channel `ch` of `data`.
    pub fn begin(&mut self, data: &HeadphoneData, ch: usize, s: &Settings, sample_rate: f32) {
        self.curve.begin(data, ch, s, sample_rate);
        self.stage = Stage::Grid;
        self.bin = 0;
    }

    /// Does one step of the design that `begin` started, with the same `data`. When it returns
    /// true the design is complete and `ir` (at least [`TAPS`] long) holds the FIR; the curve and
    /// the preamp are in [`Design::curve`].
    pub fn step(&mut self, data: &HeadphoneData, ir: &mut [f32]) -> bool {
        let m = FFT_SIZE;
        match self.stage {
            Stage::Idle => return true,
            Stage::Grid => {
                if self.curve.grid_chunk(data) {
                    self.stage = Stage::Shape;
                }
            }
            Stage::Shape => {
                self.curve.shape(data);
                if self.curve.flat {
                    ir[..TAPS].fill(0.0);
                    ir[0] = lin(self.curve.preamp_db);
                    self.stage = Stage::Idle;
                    return true;
                }
                self.stage = Stage::Bins;
                self.bin = 0;
            }
            Stage::Bins => {
                let end = (self.bin + BIN_CHUNK).min(m / 2 + 1);
                let sr = self.curve.sample_rate as f32;
                let last = (GRID_LEN - 1) as f32;
                for k in self.bin..end {
                    let db = if k == 0 {
                        self.curve.curve[0]
                    } else {
                        let g = grid_pos(k as f32 * sr / m as f32);
                        if g <= 0.0 {
                            self.curve.curve[0]
                        } else if g >= last {
                            self.curve.curve[GRID_LEN - 1]
                        } else {
                            let i = g as usize;
                            let t = g - i as f32;
                            self.curve.curve[i]
                                + (self.curve.curve[i + 1] - self.curve.curve[i]) * t
                        }
                    };
                    self.re[k] = (f64::from(db) * LN_10_20) as f32;
                }
                self.bin = end;
                if end == m / 2 + 1 {
                    for k in 1..m / 2 {
                        self.re[m - k] = self.re[k];
                    }
                    self.stage = Stage::Cep1;
                }
            }
            Stage::Cep1 => {
                self.im.fill(0.0);
                self.fft.inverse(&mut self.re, &mut self.im);
                // Fold: keep the causal half, double it; the anti-causal half goes.
                for v in &mut self.re[1..m / 2] {
                    *v *= 2.0;
                }
                self.re[m / 2 + 1..].fill(0.0);
                self.im.fill(0.0);
                self.stage = Stage::Cep2;
            }
            Stage::Cep2 => {
                self.fft.forward(&mut self.re, &mut self.im);
                self.stage = Stage::Cep3;
            }
            Stage::Cep3 => {
                let log2e = std::f64::consts::LOG2_E;
                for k in 0..m {
                    let mag = exp2_det_f64(f64::from(self.re[k]) * log2e);
                    let phase = f64::from(self.im[k]) / std::f64::consts::TAU;
                    self.re[k] = (mag * sin_cycles(phase + 0.25)) as f32;
                    self.im[k] = (mag * sin_cycles(phase)) as f32;
                }
                self.fft.inverse(&mut self.re, &mut self.im);
                let gain = lin(self.curve.preamp_db);
                let fade = TAPS / 8;
                for (n, out) in ir[..TAPS].iter_mut().enumerate() {
                    let mut w = 1.0;
                    if n + fade > TAPS {
                        let t = (n + fade - TAPS) as f64 / fade as f64;
                        w = (0.5 * (1.0 + sin_cycles(0.25 + 0.5 * t))) as f32;
                    }
                    *out = self.re[n] * w * gain;
                }
                self.stage = Stage::Idle;
                return true;
            }
        }
        false
    }
}
