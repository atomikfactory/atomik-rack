//! Loudness metering per ITU-R BS.1770-4 / EBU R128 (Tech 3341) and the 4x true-peak detector
//! shared with the limiter (BS.1770-4 Annex 2 style).
//!
//! # K-weighting
//!
//! Two cascaded biquads, designed for the actual sample rate with the bilinear method used to
//! derive the standard's 48 kHz coefficients (so at 48 kHz they reproduce the published
//! numbers):
//!
//! * **Pre-filter** (head-related high shelf): `f0 = 1681.9744509555319 Hz`,
//!   `G = 3.999843853973347 dB`, `Q = 0.7071752369554196`. With `K = tan(pi f0 / sr)`,
//!   `Vh = 10^(G/20)`, `Vb = Vh^0.4996667741545416`, `a0 = 1 + K/Q + K^2`:
//!   `b = [Vh + Vb K/Q + K^2, 2 (K^2 - Vh), Vh - Vb K/Q + K^2] / a0`,
//!   `a = [1, 2 (K^2 - 1) / a0, (1 - K/Q + K^2) / a0]`.
//! * **RLB high-pass**: `f0 = 38.13547087613982 Hz`, `Q = 0.5003270373253953`,
//!   `b = [1, -2, 1]` (not normalised, as in the standard), `a = [1, 2 (K^2 - 1) / a0,
//!   (1 - K/Q + K^2) / a0]`.
//!
//! # Blocks, windows, gating
//!
//! The channel-summed K-weighted power (`L` and `R` weight 1) is accumulated in 100 ms
//! sub-blocks (`round(0.1 sr)` samples, mean square). Every 100 ms (the 75 % hop of the 400 ms
//! gating blocks):
//!
//! * **Momentary** = loudness of the mean of the last 4 sub-blocks (400 ms),
//! * **Short-term** = loudness of the mean of the last 30 sub-blocks (3 s),
//!
//! both over a window that starts zero-filled (the first readings are low while the window
//! fills), and `LUFS = -0.691 + 10 log10(mean square)`. Both are held between updates.
//!
//! * **Integrated**: every 400 ms block (once 4 sub-blocks exist) with loudness above the
//!   **absolute gate** of -70 LUFS is stored in a **histogram of 0.1 LU bins** from -70 to +10
//!   LUFS (louder blocks go into the last bin): per bin a block count (`u64`) and the exact
//!   sum of the mean-square values (`f64`). The **relative gate** is the loudness of the mean
//!   of all absolute-gated blocks minus 10 LU; the integrated loudness is the loudness of the
//!   mean of the blocks whose bin centre lies above the relative gate. Storage is fixed
//!   (801 bins), so any programme length works without allocation. **Accuracy:** the energies
//!   are exact; only the *membership* of blocks within half a bin (0.05 LU) of the relative
//!   threshold is quantised, which moves the result by well under 0.01 LU on programme material
//!   (it is exactly the standard's result for signals whose gated blocks are far from the
//!   threshold, e.g. steady tones).
//!
//! Silence and "no data yet" read [`FLOOR_DB`] (-120). The long-term state accumulates in `f64`.

use atomik_dsp::oversample::Oversampler;
use atomik_dsp::{Biquad, BiquadCoeffs};

/// Reading for silence / no data.
pub const FLOOR_DB: f32 = -120.0;
/// Absolute gate in LUFS.
const ABS_GATE: f64 = -70.0;
/// Relative gate offset in LU.
const REL_GATE: f64 = -10.0;
/// Histogram bin width in LU.
const BIN: f64 = 0.1;
/// Number of histogram bins (-70 to +10 LUFS, plus the overflow bin).
const BINS: usize = 801;
/// Sub-blocks in the momentary window.
const MOMENTARY_SUBS: usize = 4;
/// Sub-blocks in the short-term window.
const SHORT_SUBS: usize = 30;

/// `10 log10(z) - 0.691` for a mean square, [`FLOOR_DB`] for silence.
fn lufs(z: f64) -> f64 {
    if z > 1e-12 {
        (-0.691 + 10.0 * z.log10()).max(f64::from(FLOOR_DB))
    } else {
        f64::from(FLOOR_DB)
    }
}

/// K-weighting coefficients `(pre-filter, RLB high-pass)` for `sr`.
pub fn k_weighting(sr: f64) -> (BiquadCoeffs, BiquadCoeffs) {
    let pi = std::f64::consts::PI;
    let (f0, g, q) = (
        1_681.974_450_955_532,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (pi * f0 / sr).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = BiquadCoeffs {
        b0: (vh + vb * k / q + k * k) / a0,
        b1: 2.0 * (k * k - vh) / a0,
        b2: (vh - vb * k / q + k * k) / a0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    };
    let (f0, q) = (38.135_470_876_139_82, 0.500_327_037_325_395_3);
    let k = (pi * f0 / sr).tan();
    let a0 = 1.0 + k / q + k * k;
    let rlb = BiquadCoeffs {
        b0: 1.0,
        b1: -2.0,
        b2: 1.0,
        a1: 2.0 * (k * k - 1.0) / a0,
        a2: (1.0 - k / q + k * k) / a0,
    };
    (shelf, rlb)
}

/// Stereo BS.1770 / R128 loudness meter (see the module docs).
#[derive(Debug, Clone)]
pub struct LoudnessMeter {
    shelf: BiquadCoeffs,
    rlb: BiquadCoeffs,
    /// `[channel][stage]`.
    filt: [[Biquad; 2]; 2],
    sub_len: usize,
    sub_n: usize,
    sub_acc: f64,
    ring: [f64; SHORT_SUBS],
    ring_pos: usize,
    filled: usize,
    hist_n: Vec<u64>,
    hist_z: Vec<f64>,
    momentary: f32,
    short_term: f32,
    integrated: f32,
}

impl LoudnessMeter {
    /// A meter for `sr` Hz (allocates the histogram; non-RT).
    pub fn new(sr: f64) -> Self {
        let (shelf, rlb) = k_weighting(sr);
        Self {
            shelf,
            rlb,
            filt: [[Biquad::default(); 2]; 2],
            sub_len: ((sr * 0.1).round() as usize).max(1),
            sub_n: 0,
            sub_acc: 0.0,
            ring: [0.0; SHORT_SUBS],
            ring_pos: 0,
            filled: 0,
            hist_n: vec![0; BINS],
            hist_z: vec![0.0; BINS],
            momentary: FLOOR_DB,
            short_term: FLOOR_DB,
            integrated: FLOOR_DB,
        }
    }

    /// Clears the filters, windows and the gating histogram (no allocation).
    pub fn reset(&mut self) {
        self.filt = [[Biquad::default(); 2]; 2];
        self.sub_n = 0;
        self.sub_acc = 0.0;
        self.ring = [0.0; SHORT_SUBS];
        self.ring_pos = 0;
        self.filled = 0;
        self.hist_n.fill(0);
        self.hist_z.fill(0.0);
        self.momentary = FLOOR_DB;
        self.short_term = FLOOR_DB;
        self.integrated = FLOOR_DB;
    }

    /// Momentary loudness (400 ms) in LUFS.
    pub fn momentary(&self) -> f32 {
        self.momentary
    }

    /// Short-term loudness (3 s) in LUFS.
    pub fn short_term(&self) -> f32 {
        self.short_term
    }

    /// Gated integrated loudness in LUFS.
    pub fn integrated(&self) -> f32 {
        self.integrated
    }

    /// Feeds one batch (equal-length slices).
    pub fn process(&mut self, l: &[f32], r: &[f32]) {
        for (xl, xr) in l.iter().zip(r) {
            let mut p = 0.0;
            for (c, x) in [*xl, *xr].into_iter().enumerate() {
                let x = if x.is_finite() { f64::from(x) } else { 0.0 };
                let y = self.filt[c][0].process(&self.shelf, x);
                let y = self.filt[c][1].process(&self.rlb, y);
                p += y * y;
            }
            self.sub_acc += p;
            self.sub_n += 1;
            if self.sub_n == self.sub_len {
                let z = self.sub_acc / self.sub_len as f64;
                self.sub_acc = 0.0;
                self.sub_n = 0;
                self.push_sub(z);
            }
        }
    }

    fn mean_last(&self, n: usize) -> f64 {
        let mut s = 0.0;
        for k in 0..n {
            s += self.ring[(self.ring_pos + SHORT_SUBS - 1 - k) % SHORT_SUBS];
        }
        s / n as f64
    }

    fn push_sub(&mut self, z: f64) {
        self.ring[self.ring_pos] = z;
        self.ring_pos = (self.ring_pos + 1) % SHORT_SUBS;
        self.filled = (self.filled + 1).min(SHORT_SUBS);
        let z400 = self.mean_last(MOMENTARY_SUBS);
        self.momentary = lufs(z400) as f32;
        self.short_term = lufs(self.mean_last(SHORT_SUBS)) as f32;
        if self.filled >= MOMENTARY_SUBS {
            let l = lufs(z400);
            if l > ABS_GATE {
                let idx = (((l - ABS_GATE) / BIN) as usize).min(BINS - 1);
                self.hist_n[idx] += 1;
                self.hist_z[idx] += z400;
            }
            self.update_integrated();
        }
    }

    fn update_integrated(&mut self) {
        let (mut n, mut z) = (0u64, 0.0f64);
        for i in 0..BINS {
            n += self.hist_n[i];
            z += self.hist_z[i];
        }
        if n == 0 {
            self.integrated = FLOOR_DB;
            return;
        }
        let rel = lufs(z / n as f64) + REL_GATE;
        let (mut n2, mut z2) = (0u64, 0.0f64);
        for i in 0..BINS {
            let centre = ABS_GATE + (i as f64 + 0.5) * BIN;
            if centre > rel {
                n2 += self.hist_n[i];
                z2 += self.hist_z[i];
            }
        }
        self.integrated = if n2 == 0 {
            FLOOR_DB
        } else {
            lufs(z2 / n2 as f64) as f32
        };
    }
}

/// 4x oversampled peak detector for one channel (BS.1770-4 Annex 2 style).
///
/// [`PeakDetector::process`] returns, per base-rate sample `m`, the largest absolute value among
/// the four 4x samples of group `m`. The oversampler's up-path delay is 70 samples at the 4x
/// rate, so an input sample `n` lands in group `n + DETECT_DELAY` ([`DETECT_DELAY`] = 17); a
/// peak between samples `n` and `n + 1` lands in group `n + 17` or `n + 18`.
///
/// With `refine`, every 4x sample that is a local maximum of `|x|` is replaced by the vertex of
/// the parabola through it and its two neighbours (capped at +10 %), which reads the true peak
/// of near-Nyquist signals within a few hundredths of a dB instead of the 0.7 dB a pure 4x
/// reading can miss. The centre sample of a parabola needs its newer neighbour, so a refined
/// group `m` covers the 4x samples `4m - 1 ..= 4m + 2`; the group delay is unchanged.
#[derive(Debug, Clone)]
pub struct PeakDetector {
    os: Oversampler,
    /// `[older, newer]` of the last two 4x samples.
    prev: [f32; 2],
}

/// Group delay of [`PeakDetector`] in base-rate samples (see the type docs).
pub const DETECT_DELAY: usize = 17;

impl PeakDetector {
    /// A detector for batches of up to `max_block` samples (allocates; non-RT).
    pub fn new(max_block: usize) -> Self {
        Self {
            os: Oversampler::new(max_block),
            prev: [0.0; 2],
        }
    }

    /// Clears the filter state.
    pub fn reset(&mut self) {
        self.os.reset();
        self.prev = [0.0; 2];
    }

    /// Writes the per-sample group peaks of `x` (non-negative) into `peaks` (`x.len()` long,
    /// at most the `max_block` of [`PeakDetector::new`]).
    pub fn process(&mut self, x: &[f32], peaks: &mut [f32], refine: bool) {
        let hi = self.os.upsample(x);
        let (mut a, mut b) = (self.prev[0], self.prev[1]);
        for (m, grp) in hi.chunks(4).enumerate() {
            let mut pk = 0.0f32;
            for &c in grp {
                let v = if refine {
                    let v = b.abs();
                    let mut p = v;
                    if v > 0.0 && v >= a.abs() && v >= c.abs() {
                        let s = b.signum();
                        let (aa, cc) = (a * s, c * s);
                        let den = 2.0 * v - aa - cc;
                        if den > 1e-9 {
                            p = (v + (aa - cc) * (aa - cc) / (8.0 * den)).min(v * 1.1);
                        }
                    }
                    p
                } else {
                    c.abs()
                };
                pk = pk.max(v);
                a = b;
                b = c;
            }
            peaks[m] = pk;
        }
        self.prev = [a, b];
    }
}
