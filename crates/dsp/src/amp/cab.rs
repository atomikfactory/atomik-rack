//! The cabinet: a speaker, a box and a microphone, synthesised as a cascade of designed filters.
//! There is no recorded impulse response in it; every number below is a choice of this design,
//! shaped after how a guitar speaker in a box behaves in general.
//!
//! ```text
//! x -> high-pass -> resonance -> low-mid -> upper mid -> dip -> presence -> low-pass x2
//!   -> mic high shelf -> mic low shelf -> + box reflection -> x trim
//! ```
//!
//! | Type | Character | High-pass | Resonance | Upper mid | Low-pass (two sections) | Reflection |
//! |---|---|---|---|---|---|---|
//! | `Open1x12` | small, open back: thin lows, airy top | 92 Hz | 130 Hz +2.5 dB | 2.8 kHz +4 dB | 6.0 / 7.2 kHz | 12 % |
//! | `Combo2x12` | in between | 82 Hz | 115 Hz +3.8 dB | 2.5 kHz +4.2 dB | 5.6 / 6.7 kHz | 17 % |
//! | `Closed4x12` | closed, big: tight and full low end, dark top | 72 Hz | 105 Hz +5 dB | 2.3 kHz +4.5 dB | 5.2 / 6.2 kHz | 22 % |
//! | `Off` | no cabinet: every filter is a pass-through | | | | | |
//!
//! The **reflection** is one echo of the filtered signal 0.37 ms later (the back panel), which
//! puts the comb-like colour of a box on the response. The **mic** position `0..=1` (centre of
//! the cone to its edge) tilts the top: a high shelf at 3.2 kHz from +4 dB to -5 dB, a low shelf
//! at 180 Hz from -1 dB to +2 dB (the proximity of a mic at the edge), and the second low-pass
//! drops by up to 30 %. Each type has a trim so the loudness of the three is alike.
//!
//! Switching the type or moving the mic glides every coefficient over 15 ms (no click).

use super::{BlockBiquad, db_to_gain, glide_samples};
use crate::{BiquadCoeffs, BiquadShape, OnePoleSmoother};

const GLIDE_MS: f32 = 15.0;
/// Delay of the box reflection in seconds.
const REFLECTION_S: f32 = 0.000_37;
/// Ring length (a power of two, above the reflection at 192 kHz).
const RING: usize = 256;
const STAGES: usize = 10;

/// Which cabinet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CabType {
    /// One 12 inch speaker, open back.
    Open1x12,
    /// Two 12 inch speakers.
    Combo2x12,
    /// Four 12 inch speakers, closed back.
    Closed4x12,
    /// No cabinet.
    Off,
}

impl CabType {
    /// The types in choice order.
    pub const ALL: [CabType; 4] = [
        CabType::Open1x12,
        CabType::Combo2x12,
        CabType::Closed4x12,
        CabType::Off,
    ];

    /// The type of choice index `i` (clamped).
    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(3)]
    }

    fn spec(self) -> Option<Spec> {
        match self {
            CabType::Open1x12 => Some(Spec {
                hp: 92.0,
                peaks: [
                    (130.0, 1.1, 2.5),
                    (550.0, 0.8, 1.5),
                    (2_800.0, 1.3, 4.0),
                    (3_600.0, 3.0, -3.0),
                    (5_200.0, 1.8, 2.0),
                ],
                lp: [6_000.0, 7_200.0],
                reflection: 0.12,
                trim_db: 2.0,
            }),
            CabType::Combo2x12 => Some(Spec {
                hp: 82.0,
                peaks: [
                    (115.0, 1.2, 3.8),
                    (450.0, 0.9, -2.0),
                    (2_500.0, 1.2, 4.2),
                    (3_400.0, 3.0, -3.5),
                    (4_900.0, 1.9, 2.5),
                ],
                lp: [5_600.0, 6_700.0],
                reflection: 0.17,
                trim_db: 1.5,
            }),
            CabType::Closed4x12 => Some(Spec {
                hp: 72.0,
                peaks: [
                    (105.0, 1.3, 5.0),
                    (380.0, 0.9, -3.0),
                    (2_300.0, 1.1, 4.5),
                    (3_300.0, 3.0, -4.0),
                    (4_700.0, 2.0, 3.0),
                ],
                lp: [5_200.0, 6_200.0],
                reflection: 0.22,
                trim_db: 1.0,
            }),
            CabType::Off => None,
        }
    }
}

/// The design numbers of one cabinet: `(frequency, Q, gain dB)` per peak.
struct Spec {
    hp: f32,
    peaks: [(f32, f32, f32); 5],
    lp: [f32; 2],
    reflection: f32,
    trim_db: f32,
}

/// The cabinet (mono).
#[derive(Debug, Clone)]
pub struct Cabinet {
    sample_rate: f32,
    kind: CabType,
    mic: f32,
    stages: [BlockBiquad; STAGES],
    reflection: OnePoleSmoother,
    trim: OnePoleSmoother,
    refl_target: f32,
    trim_target: f32,
    ring: [f32; RING],
    pos: usize,
    delay: usize,
}

impl Cabinet {
    /// A cabinet of the closed 4x12 type with the mic at 0.3, for `sample_rate`.
    pub fn new(sample_rate: f32) -> Self {
        let delay = ((REFLECTION_S * sample_rate).round() as usize).clamp(1, RING - 1);
        let mut c = Self {
            sample_rate,
            kind: CabType::Closed4x12,
            mic: 0.3,
            stages: std::array::from_fn(|_| BlockBiquad::new()),
            reflection: OnePoleSmoother::new(0.0, GLIDE_MS, sample_rate),
            trim: OnePoleSmoother::new(1.0, GLIDE_MS, sample_rate),
            refl_target: 0.0,
            trim_target: 1.0,
            ring: [0.0; RING],
            pos: 0,
            delay,
        };
        c.apply(true);
        c
    }

    /// Selects the cabinet.
    pub fn set_type(&mut self, kind: CabType) {
        if kind != self.kind {
            self.kind = kind;
            self.apply(false);
        }
    }

    /// The mic position `0..=1`: centre of the cone to its edge.
    pub fn set_mic(&mut self, mic: f32) {
        let mic = if mic.is_finite() {
            mic.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if mic.to_bits() != self.mic.to_bits() {
            self.mic = mic;
            self.apply(false);
        }
    }

    /// The selected cabinet.
    pub fn cab_type(&self) -> CabType {
        self.kind
    }

    /// The filter designs for the current type and mic (identity for `Off`).
    fn design(&self) -> ([BiquadCoeffs; STAGES], f32, f32) {
        let Some(s) = self.kind.spec() else {
            return ([BiquadCoeffs::IDENTITY; STAGES], 0.0, 1.0);
        };
        let sr = f64::from(self.sample_rate);
        let d = |shape, f: f32, q: f32, g: f32| {
            BiquadCoeffs::design(shape, f64::from(f), f64::from(q), f64::from(g), sr)
        };
        let m = self.mic;
        let mut c = [BiquadCoeffs::IDENTITY; STAGES];
        c[0] = d(BiquadShape::Highpass, s.hp, 0.7, 0.0);
        for (i, (f, q, g)) in s.peaks.into_iter().enumerate() {
            c[1 + i] = d(BiquadShape::Peaking, f, q, g);
        }
        c[6] = d(BiquadShape::Lowpass, s.lp[0], 0.6, 0.0);
        c[7] = d(BiquadShape::Lowpass, s.lp[1] * (1.0 - 0.3 * m), 0.8, 0.0);
        c[8] = d(BiquadShape::HighShelf, 3_200.0, 0.6, 4.0 - 9.0 * m);
        c[9] = d(BiquadShape::LowShelf, 180.0, 0.6, -1.0 + 3.0 * m);
        (c, s.reflection, db_to_gain(s.trim_db))
    }

    fn apply(&mut self, now: bool) {
        let (c, refl, trim) = self.design();
        let n = glide_samples(GLIDE_MS, self.sample_rate);
        for (st, c) in self.stages.iter_mut().zip(c) {
            if now {
                st.set_now(c);
            } else {
                st.glide_to(c, n);
            }
        }
        self.refl_target = refl;
        self.trim_target = trim;
        if now {
            self.reflection.set_immediate(refl);
            self.trim.set_immediate(trim);
        }
    }

    /// Magnitude response in dB at `hz` of the target filters (without the reflection).
    pub fn response_db(&self, hz: f32) -> f32 {
        let sr = f64::from(self.sample_rate);
        let f = f64::from(hz);
        self.stages
            .iter()
            .map(|s| s.target().response_db(f, sr))
            .sum::<f64>() as f32
            + 20.0 * self.trim_target.log10()
    }

    /// Clears the state and jumps to the current design.
    pub fn reset(&mut self) {
        self.apply(true);
        for s in &mut self.stages {
            s.reset();
        }
        self.ring = [0.0; RING];
        self.pos = 0;
    }

    /// Processes one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let mut y = x;
        for s in &mut self.stages {
            y = s.process(y);
        }
        self.ring[self.pos] = y;
        let echo = self.ring[(self.pos.wrapping_sub(self.delay)) & (RING - 1)];
        self.pos = (self.pos + 1) & (RING - 1);
        let r = self.reflection.next_value(self.refl_target);
        let t = self.trim.next_value(self.trim_target);
        (y + r * echo) * t
    }
}
