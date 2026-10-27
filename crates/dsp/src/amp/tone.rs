//! The tone stack: bass, middle and treble in a passive style.
//!
//! A real passive stack is a network of capacitors and potentiometers: it always loses level, it
//! has a broad scoop in the middle that the middle knob fills in, and its three knobs load each
//! other. This one is built from three filters (a low shelf, a peak that makes the scoop and a
//! high shelf) whose gains follow the knobs, with that interaction added on purpose (it is not a
//! circuit solution):
//!
//! * the **scoop** between the shelves is `-(11 - 9 mid)` dB at 480 Hz and gets a little deeper
//!   with every unit the bass and the treble are raised above the middle position;
//! * a high **middle** setting loads the shelves: it takes up to a quarter of their gain swing
//!   away;
//! * a stack at the middle position has a net loss at the shelves of 9 dB (the insertion loss of
//!   a passive stack), which the next stage's gain makes up.
//!
//! Knobs are `0..=1`. The filters are designed at the rate the stack runs at.

use super::{BlockBiquad, glide_samples};
use crate::{BiquadCoeffs, BiquadShape};

/// Glide time in ms of a knob move.
const GLIDE_MS: f32 = 12.0;
/// Corner of the bass shelf.
pub const BASS_HZ: f32 = 110.0;
/// Centre of the scoop.
pub const MID_HZ: f32 = 480.0;
/// Corner of the treble shelf.
pub const TREBLE_HZ: f32 = 2_800.0;

/// The tone stack.
#[derive(Debug, Clone)]
pub struct ToneStack {
    sample_rate: f32,
    low: BlockBiquad,
    scoop: BlockBiquad,
    high: BlockBiquad,
    knobs: [f32; 3],
}

/// The gains in dB of the three filters (low shelf, scoop, high shelf) for the knobs
/// `(bass, mid, treble)`, each `0..=1`.
pub fn tone_gains_db(bass: f32, mid: f32, treble: f32) -> [f32; 3] {
    let (b, m, t) = (
        bass.clamp(0.0, 1.0),
        mid.clamp(0.0, 1.0),
        treble.clamp(0.0, 1.0),
    );
    // Shelves: -23 dB (knob at 0) to +5 dB (knob at 1), -9 dB at the middle position; the
    // middle knob loads them.
    let load = 1.0 - 0.25 * m;
    let low = -9.0 + 28.0 * (b - 0.5) * load;
    let high = -9.0 + 28.0 * (t - 0.5) * load;
    // The scoop deepens as the shelves are raised.
    let raised = (b - 0.5).max(0.0) + (t - 0.5).max(0.0);
    let scoop = -(11.0 - 9.0 * m) - 3.0 * raised;
    [low, scoop, high]
}

impl ToneStack {
    /// A stack at the middle position for a stage running at `sample_rate`.
    pub fn new(sample_rate: f32) -> Self {
        let mut s = Self {
            sample_rate,
            low: BlockBiquad::new(),
            scoop: BlockBiquad::new(),
            high: BlockBiquad::new(),
            knobs: [0.5; 3],
        };
        s.apply(true);
        s
    }

    /// Sets the knobs (`0..=1` each) and starts the glide to the new filters.
    pub fn set(&mut self, bass: f32, mid: f32, treble: f32) {
        let k = [bass, mid, treble].map(|v| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.5
            }
        });
        if k.map(f32::to_bits) != self.knobs.map(f32::to_bits) {
            self.knobs = k;
            self.apply(false);
        }
    }

    /// Jumps to the current knobs and clears the state.
    pub fn reset(&mut self) {
        self.apply(true);
        self.low.reset();
        self.scoop.reset();
        self.high.reset();
    }

    fn apply(&mut self, now: bool) {
        let [lo, sc, hi] = tone_gains_db(self.knobs[0], self.knobs[1], self.knobs[2]);
        let sr = f64::from(self.sample_rate);
        let design = |shape, f: f32, q: f64, g: f32| {
            BiquadCoeffs::design(shape, f64::from(f), q, f64::from(g), sr)
        };
        let c = [
            design(BiquadShape::LowShelf, BASS_HZ, 0.6, lo),
            design(BiquadShape::Peaking, MID_HZ, 0.7, sc),
            design(BiquadShape::HighShelf, TREBLE_HZ, 0.6, hi),
        ];
        let n = glide_samples(GLIDE_MS, self.sample_rate);
        for (f, c) in [&mut self.low, &mut self.scoop, &mut self.high]
            .into_iter()
            .zip(c)
        {
            if now {
                f.set_now(c);
            } else {
                f.glide_to(c, n);
            }
        }
    }

    /// Magnitude response in dB at `hz` of the target filters.
    pub fn response_db(&self, hz: f32) -> f32 {
        let sr = f64::from(self.sample_rate);
        let f = f64::from(hz);
        (self.low.target().response_db(f, sr)
            + self.scoop.target().response_db(f, sr)
            + self.high.target().response_db(f, sr)) as f32
    }

    /// Filters one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.high.process(self.scoop.process(self.low.process(x)))
    }
}
