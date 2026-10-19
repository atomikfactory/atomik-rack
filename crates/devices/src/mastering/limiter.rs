//! Look-ahead true-peak limiter of `atomik.mastering`.
//!
//! # Design
//!
//! ```text
//! x --+--> [4x oversampled peak detector, per channel] -> p = max(L, R)
//!     |                                                      |
//!     |     r = min(1, ceiling / p)  (required gain per detector group)
//!     |                                                      v
//!     |     mn = sliding minimum of r over LA + 2 groups -> boxcar mean over LA samples = g
//!     |                                                      |
//!     |     env = g when g < env, else a one-pole release towards g (time constant `release`)
//!     v                                                      v
//!     +--> delay (DETECT_DELAY + LA samples) ---------------> x env, clamp to +-ceiling
//! ```
//!
//! * **Detection.** [`PeakDetector`] (4x halfband oversampler plus a parabolic refinement of the
//!   local maxima) per channel; the two channels are linked (one gain for both, so the stereo
//!   image is kept).
//! * **Attack by look-ahead.** `LA = round(1.5 ms * sr)`. The minimum of the required gain over
//!   the last `LA + 2` groups, averaged over `LA` samples, ramps the gain down linearly over `LA`
//!   samples and reaches the required value exactly when the peak leaves the delay line. Because
//!   the window is two groups longer than the boxcar, the gain applied to an audio sample is at
//!   most the required gain of the three detector groups `n + 16 ..= n + 18` around it
//!   (`n + 17` is the group of the sample itself, `n + 18` the one after it, so an inter-sample
//!   peak on either side is covered).
//! * **Release.** `env` follows `g` down instantly (`g` itself is already a smooth ramp) and
//!   returns with `exp(-1/(release * sr))` (exponential, time constant `release`). `env <= g`
//!   always, so the guarantee above holds after the release stage. At rest `env == 1` exactly
//!   and the audio passes bit for bit.
//! * **Last resort.** The output is clamped to `+-ceiling` (sample peak); this only acts if the
//!   detector missed something (e.g. the ceiling was lowered in the same batch).
//! * **Latency** `DETECT_DELAY + LA` samples (constant per sample rate): 17 + 72 = 89 at 48 kHz.
//! * **Sustained tones.** With release times of tens of milliseconds the gain stays
//!   essentially constant over a cycle of a low tone (the envelope only follows the peaks), so
//!   the limiter does not distort steady tones beyond the gain change.

use super::loudness::{DETECT_DELAY, PeakDetector};
use atomik_core::BATCH;
use atomik_dsp::log2_det;

/// Look-ahead in ms.
pub const LOOKAHEAD_MS: f32 = 1.5;

/// Sliding window minimum (monotone deque in fixed arrays).
#[derive(Debug, Clone)]
struct SlidingMin {
    idx: Vec<u64>,
    val: Vec<f32>,
    mask: usize,
    head: u64,
    tail: u64,
    i: u64,
    w: u64,
}

impl SlidingMin {
    fn new(window: usize) -> Self {
        let cap = (window + 1).next_power_of_two();
        Self {
            idx: vec![0; cap],
            val: vec![0.0; cap],
            mask: cap - 1,
            head: 0,
            tail: 0,
            i: 0,
            w: window as u64,
        }
    }

    fn reset(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.i = 0;
    }

    /// Pushes `v` and returns the minimum of the last `window` values (including `v`).
    #[inline]
    fn push(&mut self, v: f32) -> f32 {
        while self.tail > self.head && self.val[(self.tail - 1) as usize & self.mask] >= v {
            self.tail -= 1;
        }
        let t = self.tail as usize & self.mask;
        self.idx[t] = self.i;
        self.val[t] = v;
        self.tail += 1;
        while self.idx[self.head as usize & self.mask] + self.w <= self.i {
            self.head += 1;
        }
        self.i += 1;
        self.val[self.head as usize & self.mask]
    }
}

/// The limiter (stereo, linked).
#[derive(Debug, Clone)]
pub struct Limiter {
    det: [PeakDetector; 2],
    la: usize,
    delay: usize,
    buf: [Vec<f32>; 2],
    mask: usize,
    wpos: usize,
    smin: SlidingMin,
    box_ring: Vec<f32>,
    box_pos: usize,
    box_sum: f64,
    box_since: usize,
    env: f32,
}

impl Limiter {
    /// A limiter for sample rate `sr` (allocates; non-RT).
    pub fn new(sr: f32) -> Self {
        let la = ((LOOKAHEAD_MS * 0.001 * sr).round() as usize).max(2);
        let delay = DETECT_DELAY + la;
        let len = (delay + 2).next_power_of_two();
        Self {
            det: [PeakDetector::new(BATCH), PeakDetector::new(BATCH)],
            la,
            delay,
            buf: [vec![0.0; len], vec![0.0; len]],
            mask: len - 1,
            wpos: 0,
            smin: SlidingMin::new(la + 2),
            box_ring: vec![1.0; la],
            box_pos: 0,
            box_sum: la as f64,
            box_since: 0,
            env: 1.0,
        }
    }

    /// Total delay of the audio path in samples (the limiter's latency).
    pub fn latency(&self) -> u32 {
        self.delay as u32
    }

    /// Look-ahead in samples.
    pub fn lookahead(&self) -> usize {
        self.la
    }

    /// Clears all state (no allocation).
    pub fn reset(&mut self) {
        for d in &mut self.det {
            d.reset();
        }
        for b in &mut self.buf {
            b.fill(0.0);
        }
        self.wpos = 0;
        self.smin.reset();
        self.box_ring.fill(1.0);
        self.box_pos = 0;
        self.box_sum = self.la as f64;
        self.box_since = 0;
        self.env = 1.0;
    }

    /// Limits one batch to `ceiling` (linear) with release coefficient `rel_alpha`
    /// (`exp(-1/(release * sr))`). `env_out` receives the applied gain per sample. Returns the
    /// largest gain reduction in dB of the batch.
    pub fn process(
        &mut self,
        x: [&[f32; BATCH]; 2],
        ceiling: f32,
        rel_alpha: f32,
        out: [&mut [f32; BATCH]; 2],
        env_out: &mut [f32; BATCH],
    ) -> f32 {
        let mut peaks = [[0.0f32; BATCH]; 2];
        for c in 0..2 {
            self.det[c].process(x[c], &mut peaks[c], true);
        }
        let la = self.la;
        let mut gr_max = 0.0f32;
        let [out_l, out_r] = out;
        for n in 0..BATCH {
            let p = peaks[0][n].max(peaks[1][n]);
            let r = if p > ceiling { ceiling / p } else { 1.0 };
            let mn = self.smin.push(r);
            let old = self.box_ring[self.box_pos];
            self.box_ring[self.box_pos] = mn;
            self.box_pos += 1;
            if self.box_pos == la {
                self.box_pos = 0;
            }
            self.box_sum += f64::from(mn) - f64::from(old);
            self.box_since += 1;
            if self.box_since == la {
                self.box_since = 0;
                self.box_sum = self.box_ring.iter().map(|v| f64::from(*v)).sum();
            }
            let g = ((self.box_sum / la as f64) as f32).min(1.0);
            if g < self.env {
                self.env = g;
            } else {
                self.env += (g - self.env) * (1.0 - rel_alpha);
                if g - self.env < 1e-7 {
                    self.env = g;
                }
            }
            let w = self.wpos;
            let rd = w.wrapping_sub(self.delay) & self.mask;
            self.buf[0][w & self.mask] = x[0][n];
            self.buf[1][w & self.mask] = x[1][n];
            out_l[n] = (self.buf[0][rd] * self.env).clamp(-ceiling, ceiling);
            out_r[n] = (self.buf[1][rd] * self.env).clamp(-ceiling, ceiling);
            self.wpos = w.wrapping_add(1);
            env_out[n] = self.env;
            if self.env < 1.0 {
                gr_max = gr_max.max(-6.020_6 * log2_det(self.env));
            }
        }
        gr_max
    }
}
