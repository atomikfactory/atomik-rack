//! `atomik.phase-meter`: a stereo and level meter that passes the audio through untouched.
//!
//! ```text
//! in_l, in_r ---------------------------------> out_l, out_r   (the same samples, no latency)
//!      |--- peaks, RMS, correlation, balance, width, true peak, K-weighted loudness --> meter values
//!      '--- 64 frames per batch --> ScopeTap  (the goniometer is drawn from it by the UI)
//! ```
//!
//! * **Audio path.** The outputs are the inputs, bit for bit, with no latency; the meters only
//!   read. Everything is allocation-free after `prepare`, and a bounce with the device equals a
//!   bounce without it.
//! * **Levels.** The peak of the last 16 batches (about 21 ms), and a held peak that stays for
//!   `hold` (off, 1 s, 3 s, 10 s or until reset) before it drops to the recent peak; RMS of each
//!   channel over 300 ms; the sample peak since the last reset in dBFS.
//! * **Stereo image** ([`atomik_dsp::stereo_meter`]): the correlation over `corr_time` (-1 inverted,
//!   0 unrelated, +1 mono), the balance, the width and the mid and side levels.
//! * **Loudness** (`loudness` on): BS.1770-4 K-weighted momentary (400 ms), short-term (3 s) and
//!   gated integrated loudness in LUFS, and the true peak (4x oversampled) since the last reset
//!   in dBTP, from the same code as `atomik.mastering` (`mastering::loudness`). A 1 kHz sine at
//!   -23 dBFS peak in both channels reads about -23 LUFS. Off, the loudness and the true peak read
//!   -120 and cost nothing.
//! * **`meter.reset`** is a momentary toggle like the mastering meter's: a rising edge clears the
//!   integrated loudness, the loudness windows, the peak maxima and the holds (as does `reset()`).
//! * **Scope.** Every batch goes into the device's [`ScopeTap`]; the UI draws the goniometer
//!   (`scope` 0, M/S: mid up, side across, so mono is a vertical line) or the plain Lissajous
//!   figure (`scope` 1, L/R: left across, right up) from the newest frames, `scope_gain` dB larger.
//! * **Meter values** ([`PHASE_METERS`] = 16): `[recent peak L, recent peak R, held peak L, held
//!   peak R, RMS L, RMS R]` (linear), `[sample peak since reset dBFS, true peak since reset
//!   dBTP]`, `[correlation, balance, width]`, `[mid RMS, side RMS]` (linear), `[momentary,
//!   short-term, integrated LUFS]`. Silence and "off" read -120 where a level is in dB.

use crate::bypass::{Bypassable, bypass_param};
use crate::mastering::loudness::{FLOOR_DB, LoudnessMeter, PeakDetector};
use crate::recent_peak::{RecentPeak, batch_peak};
use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    ScopeTap, SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::log2_det;
use atomik_dsp::stereo_meter::StereoMeter;
use std::sync::Arc;

/// Number of meter values.
pub const PHASE_METERS: usize = 16;
/// Names of the `hold` choice.
pub const HOLD_NAMES: [&str; 5] = ["Off", "1 s", "3 s", "10 s", "Infinite"];
/// Names of the `scope` choice.
pub const SCOPE_NAMES: [&str; 2] = ["M/S", "L/R"];

/// Hold time in seconds of each `hold` choice (`None`: until reset).
const HOLD_SECONDS: [Option<f32>; 5] = [Some(0.0), Some(1.0), Some(3.0), Some(10.0), None];

const P_CORR_TIME: u32 = 0;
const P_HOLD: u32 = 1;
const P_SCOPE: u32 = 2;
const P_SCOPE_GAIN: u32 = 3;
const P_LOUDNESS: u32 = 4;
const P_RESET: u32 = 5;

/// The index of each published value (see the module docs).
pub mod value {
    /// Recent peak, left.
    pub const PEAK_L: usize = 0;
    /// Recent peak, right.
    pub const PEAK_R: usize = 1;
    /// Held peak, left.
    pub const HOLD_L: usize = 2;
    /// Held peak, right.
    pub const HOLD_R: usize = 3;
    /// RMS, left.
    pub const RMS_L: usize = 4;
    /// RMS, right.
    pub const RMS_R: usize = 5;
    /// Sample peak since reset, dBFS.
    pub const SAMPLE_PEAK_DB: usize = 6;
    /// True peak since reset, dBTP.
    pub const TRUE_PEAK_DB: usize = 7;
    /// Correlation, -1..=1.
    pub const CORRELATION: usize = 8;
    /// Balance, -1 (left) to +1 (right).
    pub const BALANCE: usize = 9;
    /// Width, 0 (mono) to 1.
    pub const WIDTH: usize = 10;
    /// Mid RMS, linear.
    pub const MID_RMS: usize = 11;
    /// Side RMS, linear.
    pub const SIDE_RMS: usize = 12;
    /// Momentary loudness, LUFS.
    pub const MOMENTARY: usize = 13;
    /// Short-term loudness, LUFS.
    pub const SHORT_TERM: usize = 14;
    /// Integrated loudness, LUFS.
    pub const INTEGRATED: usize = 15;
}

#[allow(clippy::too_many_arguments)]
const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    (min, max, default): (f32, f32, f32),
    taper: Taper,
    unit: Unit,
    discrete: bool,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min,
        max,
        default,
        taper,
        unit,
        discrete,
    }
}

static PARAMS: [ParamDescriptor; 7] = [
    param(
        P_CORR_TIME,
        "corr_time",
        "Correlation time (ms)",
        (50.0, 2_000.0, 300.0),
        Taper::Exponential,
        Unit::Ms,
        false,
    ),
    param(
        P_HOLD,
        "hold",
        "Peak hold",
        (0.0, 4.0, 1.0),
        Taper::Linear,
        Unit::Choice(&HOLD_NAMES),
        true,
    ),
    param(
        P_SCOPE,
        "scope",
        "Scope",
        (0.0, 1.0, 0.0),
        Taper::Linear,
        Unit::Choice(&SCOPE_NAMES),
        true,
    ),
    param(
        P_SCOPE_GAIN,
        "scope_gain",
        "Scope gain (dB)",
        (-12.0, 24.0, 0.0),
        Taper::Linear,
        Unit::Db,
        false,
    ),
    param(
        P_LOUDNESS,
        "loudness",
        "Loudness",
        (0.0, 1.0, 1.0),
        Taper::Linear,
        Unit::Toggle,
        true,
    ),
    param(
        P_RESET,
        "meter.reset",
        "Reset meter",
        (0.0, 1.0, 0.0),
        Taper::Linear,
        Unit::Toggle,
        true,
    ),
    bypass_param(6),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);

static SOCKETS: [SocketDescriptor; 4] = [
    SocketDescriptor {
        id: IN_L,
        key: "in_l",
        name: "In L",
        kind: SocketKind::Audio,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: IN_R,
        key: "in_r",
        name: "In R",
        kind: SocketKind::Audio,
        direction: Direction::Input,
    },
    SocketDescriptor {
        id: OUT_L,
        key: "out_l",
        name: "Out L",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    },
    SocketDescriptor {
        id: OUT_R,
        key: "out_r",
        name: "Out R",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    },
];

/// Static description of the phase and stereo meter.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.phase-meter",
    name: "Phase Meter",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`PhaseMeter`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(PhaseMeter::new())),
    }
}

/// `20 log10(x)` of a linear amplitude, `-120` below -120 dB or at silence.
fn db(x: f32) -> f32 {
    if x > 1e-6 {
        (6.020_6 * log2_det(x)).max(FLOOR_DB)
    } else {
        FLOOR_DB
    }
}

/// The phase and stereo meter device (module docs).
pub struct PhaseMeter {
    sample_rate: f32,
    tap: Arc<ScopeTap>,
    stereo: StereoMeter,
    loudness: LoudnessMeter,
    true_peak: [PeakDetector; 2],
    recent: [RecentPeak; 2],
    held: [f32; 2],
    hold_left: [i64; 2],
    sample_peak: f32,
    true_peak_max: f32,
    // Settings.
    corr_ms: f32,
    hold_choice: usize,
    hold: Option<i64>,
    loudness_on: bool,
    reset_level: bool,
    reset_pending: bool,
    values: [f32; PHASE_METERS],
}

impl std::fmt::Debug for PhaseMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhaseMeter")
            .field("values", &self.values)
            .finish_non_exhaustive()
    }
}

impl Default for PhaseMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl PhaseMeter {
    /// A meter at 48 kHz with default settings (`prepare` sizes it for the real rate).
    pub fn new() -> Self {
        let mut meter = Self {
            sample_rate: 48_000.0,
            tap: Arc::new(ScopeTap::new()),
            stereo: StereoMeter::new(48_000.0, 300.0),
            loudness: LoudnessMeter::new(48_000.0),
            true_peak: [PeakDetector::new(BATCH), PeakDetector::new(BATCH)],
            recent: [RecentPeak::default(); 2],
            held: [0.0; 2],
            hold_left: [0; 2],
            sample_peak: 0.0,
            true_peak_max: 0.0,
            corr_ms: 300.0,
            hold_choice: 1,
            hold: Some(48_000),
            loudness_on: true,
            reset_level: false,
            reset_pending: false,
            values: [0.0; PHASE_METERS],
        };
        meter.clear_values();
        meter
    }

    fn hold_frames(&self, choice: usize) -> Option<i64> {
        HOLD_SECONDS
            .get(choice)
            .copied()
            .flatten()
            .map(|s| (f64::from(s) * f64::from(self.sample_rate)).round() as i64)
    }

    /// The published values at rest.
    fn clear_values(&mut self) {
        self.values = [0.0; PHASE_METERS];
        for i in [
            value::SAMPLE_PEAK_DB,
            value::TRUE_PEAK_DB,
            value::MOMENTARY,
            value::SHORT_TERM,
            value::INTEGRATED,
        ] {
            self.values[i] = FLOOR_DB;
        }
    }

    /// Clears the accumulated state (not the settings, not the scope).
    fn clear_state(&mut self) {
        self.stereo.reset();
        self.loudness.reset();
        for t in &mut self.true_peak {
            t.reset();
        }
        for r in &mut self.recent {
            r.clear();
        }
        self.held = [0.0; 2];
        self.hold_left = [0; 2];
        self.sample_peak = 0.0;
        self.true_peak_max = 0.0;
        self.reset_pending = false;
        self.clear_values();
    }

    /// Advances one channel's held peak by one batch.
    fn update_hold(&mut self, c: usize, batch: f32, recent: f32) {
        match self.hold {
            Some(0) => self.held[c] = recent,
            hold => {
                if batch >= self.held[c] {
                    self.held[c] = batch;
                    self.hold_left[c] = hold.unwrap_or(i64::MAX);
                } else if let Some(h) = hold {
                    self.hold_left[c] -= BATCH as i64;
                    if self.hold_left[c] <= 0 {
                        // The hold is over: fall to what is playing, which a new peak may raise
                        // again at once, and hold that.
                        self.held[c] = recent.max(batch);
                        self.hold_left[c] = h;
                    }
                }
            }
        }
    }
}

impl Device for PhaseMeter {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz() as f32;
        self.stereo = StereoMeter::new(self.sample_rate, self.corr_ms);
        self.loudness = LoudnessMeter::new(info.sample_rate.hz());
        self.hold = self.hold_frames(self.hold_choice);
        self.reset();
    }

    fn reset(&mut self) {
        self.clear_state();
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        match id.0 {
            P_CORR_TIME => {
                self.corr_ms = v.clamp(50.0, 2_000.0);
                self.stereo.set_corr_ms(self.corr_ms);
            }
            P_HOLD => {
                self.hold_choice = v.round().clamp(0.0, 4.0) as usize;
                self.hold = self.hold_frames(self.hold_choice);
            }
            P_LOUDNESS => {
                let on = v >= 0.5;
                if on && !self.loudness_on {
                    // Starts from nothing, as if the meter had just been reset.
                    self.loudness.reset();
                    for t in &mut self.true_peak {
                        t.reset();
                    }
                    self.true_peak_max = 0.0;
                }
                self.loudness_on = on;
            }
            P_RESET => {
                let level = v >= 0.5;
                if level && !self.reset_level {
                    self.reset_pending = true;
                }
                self.reset_level = level;
            }
            // The scope settings are the UI's.
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        if self.reset_pending {
            self.clear_state();
        }
        let (l, r) = (*ctx.input(IN_L), *ctx.input(IN_R));
        *ctx.output(OUT_L) = l;
        *ctx.output(OUT_R) = r;
        self.tap.push(&l, &r);

        let batch = [batch_peak(&l), batch_peak(&r)];
        let recent = [self.recent[0].push(batch[0]), self.recent[1].push(batch[1])];
        for (c, (b, r)) in batch.iter().zip(&recent).enumerate() {
            self.update_hold(c, *b, *r);
        }
        self.sample_peak = self.sample_peak.max(batch[0]).max(batch[1]);
        self.stereo.process(&l, &r);

        let mut true_peak_db = FLOOR_DB;
        if self.loudness_on {
            self.loudness.process(&l, &r);
            let mut pk = [0.0f32; BATCH];
            for (det, x) in self.true_peak.iter_mut().zip([&l, &r]) {
                det.process(x, &mut pk, false);
                for v in &pk {
                    self.true_peak_max = self.true_peak_max.max(*v);
                }
            }
            true_peak_db = db(self.true_peak_max);
        }

        let v = &mut self.values;
        v[value::PEAK_L] = recent[0];
        v[value::PEAK_R] = recent[1];
        v[value::HOLD_L] = self.held[0];
        v[value::HOLD_R] = self.held[1];
        v[value::RMS_L] = self.stereo.rms_left();
        v[value::RMS_R] = self.stereo.rms_right();
        v[value::SAMPLE_PEAK_DB] = db(self.sample_peak);
        v[value::TRUE_PEAK_DB] = true_peak_db;
        v[value::CORRELATION] = self.stereo.correlation();
        v[value::BALANCE] = self.stereo.balance();
        v[value::WIDTH] = self.stereo.width();
        v[value::MID_RMS] = self.stereo.mid_rms();
        v[value::SIDE_RMS] = self.stereo.side_rms();
        if self.loudness_on {
            v[value::MOMENTARY] = self.loudness.momentary();
            v[value::SHORT_TERM] = self.loudness.short_term();
            v[value::INTEGRATED] = self.loudness.integrated();
        } else {
            v[value::MOMENTARY] = FLOOR_DB;
            v[value::SHORT_TERM] = FLOOR_DB;
            v[value::INTEGRATED] = FLOOR_DB;
        }
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = self.values.len().min(out.len());
        out[..n].copy_from_slice(&self.values[..n]);
        n
    }

    fn scope_tap(&self) -> Option<Arc<ScopeTap>> {
        Some(Arc::clone(&self.tap))
    }
}

#[cfg(test)]
mod tests;
