//! The instrument bodies of the plucked strings: a short cascade of peaking, shelving and pass
//! sections per body, run on both channels and mixed in parallel with the dry signal. The
//! sections are rough sketches of what an air cavity, a top plate, a drum head or a pickup does
//! to a string's sound: where it adds weight, where it rings, where it rolls off.

use crate::biquad::{Biquad, BiquadCoeffs, BiquadShape};

/// Sections in a body.
pub const SECTIONS: usize = 6;

/// One section: shape, frequency in Hz, Q and gain in dB.
type Section = (BiquadShape, f64, f64, f64);

use BiquadShape::{HighShelf, Highpass, Lowpass, Peaking};

/// The names of the bodies, in the order of [`BODIES`].
pub const BODY_NAMES: [&str; 11] = [
    "None",
    "Steel",
    "Nylon",
    "Electric",
    "Bass",
    "Upright",
    "Ukulele",
    "Banjo",
    "Mandolin",
    "Resonator",
    "Harp",
];

/// The sections of each body (an empty list is a bit-exact bypass).
static BODIES: [&[Section]; 11] = [
    &[],
    // Steel string acoustic: air and top resonances, a little scoop, the top rolling off softly.
    &[
        (Highpass, 60.0, 0.7, 0.0),
        (Peaking, 100.0, 4.0, 6.0),
        (Peaking, 210.0, 3.0, 3.5),
        (Peaking, 480.0, 1.5, -2.0),
        (Peaking, 2_300.0, 1.2, 3.0),
        (HighShelf, 6_000.0, 0.7, -3.0),
    ],
    // Nylon: warmer, with less top.
    &[
        (Highpass, 60.0, 0.7, 0.0),
        (Peaking, 95.0, 4.0, 6.0),
        (Peaking, 190.0, 3.0, 3.0),
        (Peaking, 1_200.0, 1.5, 2.0),
        (HighShelf, 3_500.0, 0.7, -6.0),
        (Lowpass, 9_000.0, 0.7, 0.0),
    ],
    // Solid body with a pickup: no cavity, a presence peak and a roll-off.
    &[
        (Highpass, 70.0, 0.7, 0.0),
        (Peaking, 2_400.0, 1.0, 3.0),
        (Lowpass, 6_500.0, 0.7, 0.0),
    ],
    // Electric bass: weight at the bottom and a closed top.
    &[
        (Highpass, 30.0, 0.7, 0.0),
        (Peaking, 85.0, 1.2, 3.0),
        (Peaking, 700.0, 1.0, 2.0),
        (Lowpass, 3_600.0, 0.7, 0.0),
    ],
    // Upright bass: a big hollow body.
    &[
        (Highpass, 35.0, 0.7, 0.0),
        (Peaking, 90.0, 3.5, 6.0),
        (Peaking, 190.0, 3.0, 4.0),
        (Peaking, 800.0, 1.2, 2.0),
        (Lowpass, 4_200.0, 0.7, 0.0),
    ],
    // Ukulele: a small box, resonating high.
    &[
        (Highpass, 120.0, 0.7, 0.0),
        (Peaking, 290.0, 3.0, 5.0),
        (Peaking, 580.0, 2.5, 2.5),
        (Peaking, 2_000.0, 1.5, 2.0),
        (HighShelf, 7_000.0, 0.7, -2.0),
    ],
    // Banjo: a drum head, tight and tinny.
    &[
        (Highpass, 150.0, 0.7, 0.0),
        (Peaking, 520.0, 3.0, 5.0),
        (Peaking, 1_100.0, 2.0, 6.0),
        (Peaking, 2_600.0, 2.0, 4.0),
        (HighShelf, 5_000.0, 0.7, 2.0),
    ],
    // Mandolin: a small carved top.
    &[
        (Highpass, 140.0, 0.7, 0.0),
        (Peaking, 450.0, 3.0, 4.0),
        (Peaking, 1_800.0, 1.5, 3.0),
        (HighShelf, 6_500.0, 0.7, -2.0),
    ],
    // Resonator: a metal cone, nasal and loud in the mids.
    &[
        (Highpass, 100.0, 0.7, 0.0),
        (Peaking, 700.0, 2.0, 4.0),
        (Peaking, 1_250.0, 1.5, 8.0),
        (Peaking, 2_500.0, 1.5, 5.0),
        (Lowpass, 5_500.0, 0.7, 0.0),
    ],
    // Harp: a broad soundboard.
    &[
        (Highpass, 50.0, 0.7, 0.0),
        (Peaking, 160.0, 1.0, 3.0),
        (Peaking, 400.0, 0.8, 2.0),
        (HighShelf, 5_000.0, 0.7, -2.0),
    ],
];

/// A stereo body filter.
#[derive(Debug, Clone)]
pub struct Body {
    sample_rate: f64,
    kind: usize,
    count: usize,
    coeffs: [BiquadCoeffs; SECTIONS],
    left: [Biquad; SECTIONS],
    right: [Biquad; SECTIONS],
}

impl Body {
    /// A bypassed body at `sample_rate`.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate: f64::from(sample_rate),
            kind: 0,
            count: 0,
            coeffs: [BiquadCoeffs::IDENTITY; SECTIONS],
            left: [Biquad::default(); SECTIONS],
            right: [Biquad::default(); SECTIONS],
        }
    }

    /// Selects body `kind` (an index into [`BODY_NAMES`]; out of range is none).
    pub fn set_kind(&mut self, kind: usize) {
        let kind = if kind < BODIES.len() { kind } else { 0 };
        if kind == self.kind && (kind == 0 || self.count > 0) {
            return;
        }
        self.kind = kind;
        let sections = BODIES[kind];
        self.count = sections.len();
        for (c, &(shape, hz, q, db)) in self.coeffs.iter_mut().zip(sections) {
            *c = BiquadCoeffs::design(shape, hz, q, db, self.sample_rate);
        }
    }

    /// The selected body.
    pub fn kind(&self) -> usize {
        self.kind
    }

    /// True when the body does nothing.
    pub fn is_bypass(&self) -> bool {
        self.count == 0
    }

    /// Clears the filter states.
    pub fn reset(&mut self) {
        self.left = [Biquad::default(); SECTIONS];
        self.right = [Biquad::default(); SECTIONS];
    }

    /// Filters one stereo sample and mixes `mix` (`0..=1`) of the result with the rest dry.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32, mix: f32) -> (f32, f32) {
        if self.count == 0 || mix <= 0.0 {
            return (l, r);
        }
        let (mut yl, mut yr) = (f64::from(l), f64::from(r));
        for ((c, a), b) in self.coeffs[..self.count]
            .iter()
            .zip(&mut self.left)
            .zip(&mut self.right)
        {
            yl = a.process(c, yl);
            yr = b.process(c, yr);
        }
        let m = f64::from(mix.min(1.0));
        (
            (f64::from(l) + m * (yl - f64::from(l))) as f32,
            (f64::from(r) + m * (yr - f64::from(r))) as f32,
        )
    }
}
