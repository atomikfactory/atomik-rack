//! The test effect: gain, mute, latency, sidechain ducking and a tail.

use crate::{BLOCK, Block, decode_state, encode_state};

const MAX_LATENCY: u32 = 512;
const RING: usize = MAX_LATENCY as usize + 1;
/// Per-sample decay of the duck envelope.
const DUCK_DECAY: f32 = 0.999;
const TAG: [u8; 4] = *b"ATG1";

/// A stereo effect: `out = delay(in, latency) * gain * duck(sidechain)`.
#[derive(Debug, Clone)]
pub struct GainFx {
    gain_db: f64,
    mute: f64,
    latency: u32,
    duck: f64,
    ring: [Vec<f32>; 2],
    pos: usize,
    env: f32,
}

impl GainFx {
    /// Gain in dB, -60 to 12, default 0.
    pub const GAIN: u32 = 1;
    /// Mute, stepped 0 or 1.
    pub const MUTE: u32 = 2;
    /// Latency in frames, stepped 0 to 512, default 0. The effect really delays by it.
    pub const LATENCY: u32 = 3;
    /// Sidechain ducking depth, 0 to 1, default 1.
    pub const DUCK: u32 = 4;

    /// A unity effect (the sample rate is accepted for symmetry; the DSP is rate-free).
    pub fn new(_sample_rate: f64) -> Self {
        Self {
            gain_db: 0.0,
            mute: 0.0,
            latency: 0,
            duck: 1.0,
            ring: [vec![0.0; RING], vec![0.0; RING]],
            pos: 0,
            env: 0.0,
        }
    }

    /// Sets a param by id (clamped, stepped where it is; NaN gives the default; unknown ids are
    /// ignored).
    pub fn set_param(&mut self, id: u32, value: f64) {
        let v = |default: f64, lo: f64, hi: f64| {
            if value.is_nan() {
                default
            } else {
                value.clamp(lo, hi)
            }
        };
        match id {
            Self::GAIN => self.gain_db = v(0.0, -60.0, 12.0),
            Self::MUTE => self.mute = v(0.0, 0.0, 1.0).round(),
            Self::LATENCY => self.latency = v(0.0, 0.0, f64::from(MAX_LATENCY)).round() as u32,
            Self::DUCK => self.duck = v(1.0, 0.0, 1.0),
            _ => {}
        }
    }

    /// The value of a param (0 for an unknown id).
    pub fn param(&self, id: u32) -> f64 {
        match id {
            Self::GAIN => self.gain_db,
            Self::MUTE => self.mute,
            Self::LATENCY => f64::from(self.latency),
            Self::DUCK => self.duck,
            _ => 0.0,
        }
    }

    /// The reported latency in frames.
    pub fn latency_frames(&self) -> u32 {
        self.latency
    }

    /// The tail in frames (the delay's length).
    pub fn tail_frames(&self) -> u32 {
        self.latency
    }

    /// Clears the delay line and the duck envelope.
    pub fn reset(&mut self) {
        for r in &mut self.ring {
            r.fill(0.0);
        }
        self.pos = 0;
        self.env = 0.0;
    }

    /// The state as bytes.
    pub fn save_state(&self) -> Vec<u8> {
        encode_state(
            TAG,
            &[self.gain_db, self.mute, f64::from(self.latency), self.duck],
        )
    }

    /// Restores a state saved by [`GainFx::save_state`]; false (and no change) if it is not one.
    pub fn load_state(&mut self, bytes: &[u8]) -> bool {
        let Some([gain, mute, latency, duck]) = decode_state::<4>(TAG, bytes) else {
            return false;
        };
        self.set_param(Self::GAIN, gain);
        self.set_param(Self::MUTE, mute);
        self.set_param(Self::LATENCY, latency);
        self.set_param(Self::DUCK, duck);
        true
    }

    /// Processes one block. `sidechain` of `None` behaves like an all-zero sidechain.
    pub fn process(
        &mut self,
        input: &[Block; 2],
        sidechain: Option<&[Block; 2]>,
        out: &mut [Block; 2],
    ) {
        let gain = if self.mute >= 0.5 {
            0.0
        } else {
            10f64.powf(self.gain_db / 20.0) as f32
        };
        let depth = self.duck as f32;
        let latency = self.latency as usize;
        for i in 0..BLOCK {
            let sc = sidechain.map_or(0.0, |s| s[0][i].abs().max(s[1][i].abs()));
            self.env = sc.max(self.env * DUCK_DECAY);
            let duck = 1.0 / (1.0 + 4.0 * depth * self.env);
            for ch in 0..2 {
                self.ring[ch][self.pos] = input[ch][i];
                let read = (self.pos + RING - latency) % RING;
                out[ch][i] = self.ring[ch][read] * gain * duck;
            }
            self.pos = (self.pos + 1) % RING;
        }
    }
}
