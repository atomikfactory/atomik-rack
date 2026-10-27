//! The power amp: supply sag, a symmetric soft clip, presence and depth.
//!
//! ```text
//! x -> x drive / (1 + sag env) -> soft clip (ceiling lowered by the sag) -> presence -> depth
//! ```
//!
//! * **Drive** (`master`) is the level into the power stage, so it sets how hard the stage
//!   works as well as how loud it is; the device's output level makes up the volume.
//! * **Sag.** A rectifier-like envelope of the signal (30 ms attack, 250 ms release) lowers the
//!   gain by `1 / (1 + 0.9 sag env)` and the ceiling of the clip by `0.2 sag env` (`env` capped
//!   at 1): a loud passage squashes the stage and a note recovers as it decays, the way a
//!   sagging supply does. `sag = 0` is a static curve.
//! * **Clip.** The symmetric algebraic curve `u / sqrt(1 + u^2)`, antiderivative anti-aliased (a push-pull stage: odd
//!   harmonics), unity slope at 0, ceiling 1.
//! * **Presence** is a high shelf at 3.4 kHz and **depth** a low shelf at 95 Hz, both after the
//!   clip, `0..=1` mapping to `0..=10` dB and `0..=9` dB. In a real amp they are the feedback
//!   loop's high and low frequency response.

use super::{Adaa, BlockBiquad, db_to_gain, glide_samples};
use crate::{BiquadCoeffs, BiquadShape, OnePoleSmoother};

const ATTACK_MS: f32 = 30.0;
const RELEASE_MS: f32 = 250.0;
const GLIDE_MS: f32 = 12.0;
/// Presence range in dB and corner in Hz.
const PRESENCE_DB: f32 = 10.0;
const PRESENCE_HZ: f32 = 3_400.0;
/// Depth range in dB and corner in Hz.
const DEPTH_DB: f32 = 9.0;
const DEPTH_HZ: f32 = 95.0;

/// The power amp (mono).
#[derive(Debug, Clone)]
pub struct PowerAmp {
    sample_rate: f32,
    drive: OnePoleSmoother,
    drive_target: f32,
    sag: f32,
    env: f32,
    adaa: Adaa,
    attack: f32,
    release: f32,
    presence: BlockBiquad,
    depth: BlockBiquad,
    knobs: [f32; 2],
}

/// The clip and its antiderivative.
#[inline]
fn curve(u: f64) -> f64 {
    u / (1.0 + u * u).sqrt()
}

#[inline]
fn anti(u: f64) -> f64 {
    (1.0 + u * u).sqrt()
}

fn coeff(ms: f32, sample_rate: f32) -> f32 {
    1.0 - crate::exp2_det(-std::f32::consts::LOG2_E / (ms * 0.001 * sample_rate))
}

impl PowerAmp {
    /// A power amp fed at `sample_rate`: drive 0 dB, no sag, presence and depth flat.
    pub fn new(sample_rate: f32) -> Self {
        let mut p = Self {
            sample_rate,
            drive: OnePoleSmoother::new(1.0, 10.0, sample_rate),
            drive_target: 1.0,
            sag: 0.0,
            env: 0.0,
            adaa: Adaa::new(0.0, anti),
            attack: coeff(ATTACK_MS, sample_rate),
            release: coeff(RELEASE_MS, sample_rate),
            presence: BlockBiquad::new(),
            depth: BlockBiquad::new(),
            knobs: [0.0; 2],
        };
        p.apply(true);
        p
    }

    /// Drive into the stage in dB (clamped to -30..=30).
    pub fn set_drive_db(&mut self, db: f32) {
        let db = if db.is_finite() {
            db.clamp(-30.0, 30.0)
        } else {
            0.0
        };
        self.drive_target = db_to_gain(db);
    }

    /// Sag amount `0..=1`.
    pub fn set_sag(&mut self, sag: f32) {
        self.sag = if sag.is_finite() {
            sag.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    /// Presence and depth `0..=1`.
    pub fn set_voice(&mut self, presence: f32, depth: f32) {
        let k = [presence, depth].map(|v| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        });
        if k.map(f32::to_bits) != self.knobs.map(f32::to_bits) {
            self.knobs = k;
            self.apply(false);
        }
    }

    fn apply(&mut self, now: bool) {
        let sr = f64::from(self.sample_rate);
        let shelf = |shape, hz: f32, db: f32| {
            if db == 0.0 {
                BiquadCoeffs::IDENTITY
            } else {
                BiquadCoeffs::design(shape, f64::from(hz), 0.6, f64::from(db), sr)
            }
        };
        let p = shelf(
            BiquadShape::HighShelf,
            PRESENCE_HZ,
            PRESENCE_DB * self.knobs[0],
        );
        let d = shelf(BiquadShape::LowShelf, DEPTH_HZ, DEPTH_DB * self.knobs[1]);
        let n = glide_samples(GLIDE_MS, self.sample_rate);
        if now {
            self.presence.set_now(p);
            self.depth.set_now(d);
        } else {
            self.presence.glide_to(p, n);
            self.depth.glide_to(d, n);
        }
    }

    /// The supply envelope `0..=1` (what the sag follows).
    pub fn envelope(&self) -> f32 {
        self.env
    }

    /// Clears the state; the drive jumps to its target.
    pub fn reset(&mut self) {
        self.env = 0.0;
        self.adaa.reset(0.0, anti);
        self.drive.set_immediate(self.drive_target);
        self.apply(true);
        self.presence.reset();
        self.depth.reset();
    }

    /// Processes one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let a = x.abs();
        let c = if a > self.env {
            self.attack
        } else {
            self.release
        };
        self.env += c * (a.min(1.0) - self.env);
        if self.env < 1e-20 {
            self.env = 0.0;
        }
        let s = self.sag * self.env;
        let g = self.drive.next_value(self.drive_target) / (1.0 + 0.9 * s);
        let u = f64::from(x) * f64::from(g);
        let y = ((1.0 - 0.2 * s) as f64 * self.adaa.process(u, curve, anti)) as f32;
        self.depth.process(self.presence.process(y))
    }
}
