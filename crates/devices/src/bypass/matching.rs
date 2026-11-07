//! Loudness matching for the bypass wrapper: the `internal.match` switch and its gain.
//!
//! While the switch is on, [`Matcher`] measures the K-weighted short-term loudness (the same
//! filters and 100 ms / 3 s windows as the Mastering meter) of the effect's input and of what
//! the effect produced, and applies a slowly smoothed gain to the produced signal so both read
//! the same. Switching the bypass then compares sound, not level.
//!
//! * The output is measured *before* the gain is applied, so the loop is open and cannot
//!   oscillate. Both windows start zero-filled together, so their ratio is right from the first
//!   100 ms block.
//! * The target is `loudness(input) - loudness(output)` in dB, refreshed every 100 ms and clamped
//!   to [`MAX_GAIN_DB`]. Silence on either side (below about -70 LUFS) holds the last target.
//! * The gain in dB follows its target with a [`SMOOTH_SECONDS`] time constant (one step per
//!   batch) and is ramped linearly per sample inside the batch.
//! * Switching the match off ramps the gain back to 1, then the wrapper is exactly as it was
//!   without the switch (a gain of exactly 1 is not applied at all).
//! * Fixed arrays only: nothing allocates after construction.

use atomik_core::{BATCH, Batch};
use atomik_dsp::k_weighting::k_weighting;
use atomik_dsp::{Biquad, BiquadCoeffs};

/// The `internal.match` param key.
pub const MATCH_KEY: &str = "internal.match";
/// Largest gain the match applies, either way, in dB.
pub const MAX_GAIN_DB: f32 = 24.0;
/// Time constant of the gain, in seconds.
pub const SMOOTH_SECONDS: f64 = 0.7;
/// The value that follows the applied gain at the end of the device's meter values.
pub const METER_TAG: f32 = 4242.0;

/// Sub-blocks in the short-term window (100 ms each).
const SHORT_SUBS: usize = 30;
/// Mean square below which a window counts as silence (about -70 LUFS).
const GATE: f64 = 1e-7;

/// Exactly 1.0 (the gain stage is skipped only then, so off is bit-identical).
fn is_one(x: f32) -> bool {
    x.to_bits() == 1.0f32.to_bits()
}

/// Exactly 0.0.
fn is_zero(x: f32) -> bool {
    x == 0.0 || x.to_bits() == (-0.0f32).to_bits()
}

/// One K-weighted stereo meter feed.
#[derive(Debug, Clone, Copy, Default)]
struct Feed {
    /// `[channel][stage]`.
    filt: [[Biquad; 2]; 2],
    acc: f64,
    ring: [f64; SHORT_SUBS],
}

impl Feed {
    fn clear(&mut self) {
        *self = Self::default();
    }

    fn push(&mut self, c: &(BiquadCoeffs, BiquadCoeffs), l: f32, r: f32) {
        let mut sum = 0.0;
        for (ch, x) in [l, r].into_iter().enumerate() {
            let y = self.filt[ch][0].process(&c.0, f64::from(x));
            let y = self.filt[ch][1].process(&c.1, y);
            sum += y * y;
        }
        self.acc += sum;
    }

    /// Closes a sub-block of `len` samples into slot `at`.
    fn close(&mut self, at: usize, len: usize) {
        self.ring[at] = self.acc / len as f64;
        self.acc = 0.0;
    }

    /// Mean square of the short-term window.
    fn power(&self) -> f64 {
        self.ring.iter().sum::<f64>() / SHORT_SUBS as f64
    }
}

/// The loudness match of a [`super::Bypassable`] (see the module docs).
#[derive(Debug, Clone)]
pub struct Matcher {
    coeffs: (BiquadCoeffs, BiquadCoeffs),
    input: Feed,
    output: Feed,
    sub_len: usize,
    sub_n: usize,
    slot: usize,
    /// Where the smoothed gain is heading, in dB.
    target_db: f32,
    /// The smoothed gain, in dB.
    db: f32,
    /// The linear gain at the end of the last batch.
    gain: f32,
    /// Per-batch smoothing coefficient.
    coef: f32,
}

impl Matcher {
    /// A matcher for `sr` Hz.
    pub fn new(sr: f64) -> Self {
        Self {
            coeffs: k_weighting(sr),
            input: Feed::default(),
            output: Feed::default(),
            sub_len: ((sr * 0.1).round() as usize).max(1),
            sub_n: 0,
            slot: 0,
            target_db: 0.0,
            db: 0.0,
            gain: 1.0,
            coef: (1.0 - (-(BATCH as f64) / (sr * SMOOTH_SECONDS)).exp()) as f32,
        }
    }

    /// Clears the meters and the gain.
    pub fn reset(&mut self) {
        self.input.clear();
        self.output.clear();
        self.sub_n = 0;
        self.slot = 0;
        self.target_db = 0.0;
        self.db = 0.0;
        self.gain = 1.0;
    }

    /// Clears the meters (a new measurement starts) but keeps the gain where it is.
    pub fn restart(&mut self) {
        self.input.clear();
        self.output.clear();
        self.sub_n = 0;
        self.slot = 0;
        self.target_db = self.db;
    }

    /// Whether the gain is exactly 1 and heading nowhere: the wrapper skips the gain stage.
    pub fn is_unity(&self) -> bool {
        is_one(self.gain) && is_zero(self.db)
    }

    /// The applied gain in dB (what the panel shows).
    pub fn applied_db(&self) -> f32 {
        self.db
    }

    /// Measures one batch of the effect's input and its produced output.
    pub fn measure(&mut self, input: &[Batch; 2], output: &[Batch; 2]) {
        for i in 0..BATCH {
            self.input.push(&self.coeffs, input[0][i], input[1][i]);
            self.output.push(&self.coeffs, output[0][i], output[1][i]);
            self.sub_n += 1;
            if self.sub_n >= self.sub_len {
                self.sub_n = 0;
                self.input.close(self.slot, self.sub_len);
                self.output.close(self.slot, self.sub_len);
                self.slot = (self.slot + 1) % SHORT_SUBS;
                self.retarget();
            }
        }
    }

    fn retarget(&mut self) {
        let (pi, po) = (self.input.power(), self.output.power());
        if pi >= GATE && po >= GATE {
            let db = 10.0 * (pi / po).log10();
            self.target_db = (db as f32).clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
        }
    }

    /// Moves the gain one batch towards its target (`matching`) or back to 0 dB, and returns the
    /// linear gains at the start and at the end of the batch.
    pub fn step(&mut self, matching: bool) -> (f32, f32) {
        let start = self.gain;
        let goal = if matching { self.target_db } else { 0.0 };
        self.db += (goal - self.db) * self.coef;
        if !matching && self.db.abs() < 1e-3 {
            self.db = 0.0;
        }
        self.gain = if is_zero(self.db) {
            1.0
        } else {
            10f32.powf(self.db / 20.0)
        };
        (start, self.gain)
    }

    /// Applies the batch gain ramp `(start, end)` to `left` and `right`.
    pub fn apply(ramp: (f32, f32), left: &mut Batch, right: &mut Batch) {
        let (g0, g1) = ramp;
        if is_one(g0) && is_one(g1) {
            return;
        }
        let step = (g1 - g0) / BATCH as f32;
        for i in 0..BATCH {
            let g = g0 + step * (i + 1) as f32;
            left[i] *= g;
            right[i] *= g;
        }
    }
}
