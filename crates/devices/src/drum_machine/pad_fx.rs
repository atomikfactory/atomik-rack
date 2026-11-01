//! The tone, cuts and drive of one pad and its two send levels.
//!
//! A pad's voices are summed into a stereo pair; when the pad has an effect set (or a send) this
//! chain runs on the pair, in this order: **low cut** (12 dB/oct high-pass), **high cut**
//! (12 dB/oct low-pass), **tone** (a tilt around 1 kHz, +-6 dB: positive brightens, negative
//! darkens), **drive** (a soft-clip shaper, 0..=30 dB, level-compensated). A stage at its
//! default is skipped, so a pad with every effect at its default is not touched at all and the
//! drum machine renders exactly as it did before the effects existed. The filters are
//! zero-delay-feedback with deterministic coefficients; their state lives here and is cleared by
//! `reset`.

use atomik_core::BATCH;
use atomik_dsp::exp2_det;
use atomik_dsp::shaper::Shaper;
use atomik_dsp::{FilterMode, Svf};

/// Lowest low-cut frequency; at this value the stage is off.
pub(crate) const LOWCUT_MIN: f32 = 20.0;
/// Highest high-cut frequency; at this value the stage is off.
pub(crate) const HIGHCUT_MAX: f32 = 20_000.0;
/// Pivot of the tone tilt in Hz.
const TILT_HZ: f32 = 1_000.0;
/// Tilt at the ends of the tone range, in dB.
const TILT_DB: f32 = 6.0;
/// Drive at 100 %, in dB.
const DRIVE_DB: f32 = 30.0;
/// `log2(10) / 20`: dB to the log2 of an amplitude ratio.
const DB_TO_LOG2: f32 = 0.166_096_4;
/// Resonance of the cuts: a Butterworth response.
const CUT_RESONANCE: f32 = 0.094;

/// The effect settings of a pad as plain values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PadFxParams {
    /// Tilt, -1..=1.
    pub tone: f32,
    /// Low cut in Hz, [`LOWCUT_MIN`] = off.
    pub lowcut: f32,
    /// High cut in Hz, [`HIGHCUT_MAX`] = off.
    pub highcut: f32,
    /// Drive, 0..=1.
    pub drive: f32,
    /// Level of the first send, 0..=1.
    pub send1: f32,
    /// Level of the second send, 0..=1.
    pub send2: f32,
}

impl PadFxParams {
    pub const DEFAULT: Self = Self {
        tone: 0.0,
        lowcut: LOWCUT_MIN,
        highcut: HIGHCUT_MAX,
        drive: 0.0,
        send1: 0.0,
        send2: 0.0,
    };
}

/// One channel's filters and shaper.
#[derive(Debug, Clone)]
struct Channel {
    low: Svf,
    high: Svf,
    tilt: f32,
    shaper: Shaper,
}

impl Channel {
    fn new(sample_rate: f32) -> Self {
        Self {
            low: Svf::new(),
            high: Svf::new(),
            tilt: 0.0,
            shaper: Shaper::new(sample_rate),
        }
    }
}

/// The effect chain and sends of one pad.
#[derive(Debug, Clone)]
pub(crate) struct PadFx {
    params: PadFxParams,
    sample_rate: f32,
    ch: [Channel; 2],
    /// One-pole gain of the tilt's low-pass.
    tilt_g: f32,
    /// Gain of the tilt's high side (the low side is its inverse).
    tilt_hi: f32,
}

impl PadFx {
    pub fn new(sample_rate: f32) -> Self {
        let mut fx = Self {
            params: PadFxParams::DEFAULT,
            sample_rate,
            ch: [Channel::new(sample_rate), Channel::new(sample_rate)],
            tilt_g: 0.0,
            tilt_hi: 1.0,
        };
        fx.update();
        fx
    }

    /// Adopts a new sample rate (coefficients are recomputed, state cleared).
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.ch = [Channel::new(sample_rate), Channel::new(sample_rate)];
        self.update();
        self.sync_drive();
    }

    pub fn params(&self) -> PadFxParams {
        self.params
    }

    /// Sets the effect values (each clamped to its range).
    pub fn set(&mut self, p: PadFxParams) {
        let clamp = |v: f32, lo: f32, hi: f32| if v.is_nan() { lo } else { v.clamp(lo, hi) };
        self.params = PadFxParams {
            tone: clamp(p.tone, -1.0, 1.0),
            lowcut: clamp(p.lowcut, LOWCUT_MIN, HIGHCUT_MAX),
            highcut: clamp(p.highcut, LOWCUT_MIN, HIGHCUT_MAX),
            drive: clamp(p.drive, 0.0, 1.0),
            send1: clamp(p.send1, 0.0, 1.0),
            send2: clamp(p.send2, 0.0, 1.0),
        };
        self.update();
        self.sync_drive();
    }

    fn sync_drive(&mut self) {
        let db = self.params.drive * DRIVE_DB;
        for c in &mut self.ch {
            c.shaper.set_drive_db(db);
        }
    }

    fn update(&mut self) {
        let sr = self.sample_rate;
        let k = Svf::resonance_to_k_det(CUT_RESONANCE);
        for c in &mut self.ch {
            c.high
                .set_coefficients(Svf::cutoff_to_g_det(self.params.lowcut, sr), k);
            c.low
                .set_coefficients(Svf::cutoff_to_g_det(self.params.highcut, sr), k);
        }
        self.tilt_g = Svf::cutoff_to_g_det(TILT_HZ, sr);
        self.tilt_hi = exp2_det(self.params.tone * TILT_DB * DB_TO_LOG2);
    }

    pub fn lowcut_on(&self) -> bool {
        self.params.lowcut > LOWCUT_MIN
    }

    pub fn highcut_on(&self) -> bool {
        self.params.highcut < HIGHCUT_MAX
    }

    pub fn tone_on(&self) -> bool {
        self.params.tone != 0.0
    }

    pub fn drive_on(&self) -> bool {
        self.params.drive > 0.0
    }

    /// Whether the pad has an effect or a send set (a pad that does not is left alone).
    pub fn active(&self) -> bool {
        self.lowcut_on()
            || self.highcut_on()
            || self.tone_on()
            || self.drive_on()
            || self.params.send1 > 0.0
            || self.params.send2 > 0.0
    }

    /// Clears the filter and shaper state.
    pub fn reset(&mut self) {
        for c in &mut self.ch {
            c.low.reset();
            c.high.reset();
            c.tilt = 0.0;
            c.shaper.reset();
        }
    }

    /// Runs the chain over one batch of the pad's stereo pair, in place.
    pub fn process(&mut self, buf: &mut [[f32; BATCH]; 2]) {
        let (lowcut, highcut, tone, drive) = (
            self.lowcut_on(),
            self.highcut_on(),
            self.tone_on(),
            self.drive_on(),
        );
        let (g, hi) = (self.tilt_g, self.tilt_hi);
        let big = g / (1.0 + g);
        for (c, row) in self.ch.iter_mut().zip(buf.iter_mut()) {
            for s in row.iter_mut() {
                let mut x = if s.is_finite() { *s } else { 0.0 };
                if lowcut {
                    x = c.high.process_mode(x, FilterMode::Highpass);
                }
                if highcut {
                    x = c.low.process_mode(x, FilterMode::Lowpass);
                }
                if tone {
                    // One-pole low-pass; the low side gets the inverse of the high side's gain.
                    let v = (x - c.tilt) * big;
                    let lp = v + c.tilt;
                    c.tilt = atomik_dsp::flush_denormal(lp + v);
                    x = lp / hi + (x - lp) * hi;
                }
                if drive {
                    x = c.shaper.process(x);
                }
                *s = x;
            }
        }
    }
}
