//! `atomik.loudness-meter`: a loudness meter with a target, that passes the audio through
//! untouched.
//!
//! ```text
//! in_l, in_r ------------------------------------> out_l, out_r   (the same samples, no latency)
//!      |--- K-weighted loudness, true peak, sample peak, energy ----> meter values
//!      '--- a point every half second (short-term, momentary) ------> ScopeTap  (history graph)
//! ```
//!
//! * **Audio path.** The outputs are the inputs, bit for bit, with no latency; the meter only
//!   reads. Everything is allocation-free after `prepare`, and a bounce with the device equals a
//!   bounce without it.
//! * **Loudness** (BS.1770-4 / EBU R128, from `mastering::loudness`): momentary (400 ms),
//!   short-term (3 s) and gated integrated loudness in LUFS. A 1 kHz sine at -23 dBFS peak in
//!   both channels reads about -23 LUFS.
//! * **LRA**: the loudness range as EBU Tech 3342 defines it, from the short-term loudness once
//!   a second (`atomik_dsp::loudness_history`, the code the phase meter's history uses).
//! * **True peak** (4x oversampled, `mastering::loudness::PeakDetector`) per channel and overall,
//!   in dBTP, and the sample peak in dBFS, all since the last reset.
//! * **Ratios** (dB, `-120` while there is nothing to compare):
//!   * **PLR** (peak to loudness ratio) = true peak since reset - integrated loudness.
//!   * **PSR** (peak to short-term ratio) = the largest true peak of the last 3 s - the
//!     short-term loudness now. It follows the signal; PLR is the programme.
//!   * **DR** (the programme's crest factor) = sample peak since reset - the RMS level of
//!     everything since reset (both channels together, unweighted, in dBFS). A full-scale sine
//!     reads 3.0 dB, noise about 11 dB, a drum loop 15 or more.
//! * **Distribution**: the short-term loudness once a second, counted in 1 LU bins from -46 to
//!   0 LUFS (quieter and louder values land in the first and the last bin), published as the
//!   share of the tallest bin (0..1) in [`BINS`] values.
//! * **History.** Every half second a frame (short-term, momentary) goes into the device's
//!   [`ScopeTap`] (left channel short-term, right channel momentary, LUFS); the tap holds more
//!   than [`HISTORY_MAX`] of them and the UI reads the newest `HISTORY_POINTS` of them (the
//!   points since the last reset) and draws the span it was asked for. `history_span` is a
//!   display setting: the device stores it and does not use it.
//! * **Accumulation.** Like the phase meter, the device measures what flows through it, so
//!   nothing accumulates while the engine is stopped. The integrated loudness, the LRA, the
//!   maxima, the distribution and the history start from nothing after a reset (`reset` rising
//!   edge, `reset()`, `prepare`); the distribution, the LRA and the history leave out the first
//!   3 s, while the short-term window fills. **`hold`** pauses the accumulation: the integrated
//!   loudness, the LRA, the peaks, the ratios, the distribution and the history stay as they
//!   are while the momentary and short-term loudness go on following the signal.
//! * **Target.** `preset` names a target loudness and a maximum true peak ([`PRESETS`]);
//!   `Custom` uses `target_lufs` and `max_tp`. The target in force is published. A programme is
//!   [`Verdict::Ok`] when its integrated loudness is within [`TOLERANCE_LU`] of the target,
//!   [`Verdict::TooLoud`] above that and [`Verdict::TooQuiet`] below; its true peak passes when
//!   it does not exceed the maximum ([`peak_passes`]).
//! * **Meter values** ([`METERS`] = 62, see [`value`]): [`SCALARS`] numbers, then [`BINS`]
//!   distribution values from [`value::BINS`]. Silence and "no data" read -120 where a level is
//!   in dB.

use crate::bypass::{Bypassable, bypass_param};
use crate::mastering::loudness::{FLOOR_DB, LoudnessMeter, PeakDetector};
use crate::phase_meter::db;
use crate::recent_peak::batch_peak;
use atomik_core::{BATCH, Batch, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    ScopeTap, SocketDescriptor, SocketId, SocketKind,
};
use atomik_dsp::loudness_history::LoudnessHistory;
use std::sync::Arc;

/// Scalar meter values (see [`value`]).
pub const SCALARS: usize = 16;
/// Distribution bins, 1 LU each from [`DIST_LOW`].
pub const BINS: usize = 46;
/// Number of meter values: the scalars and the distribution.
pub const METERS: usize = SCALARS + BINS;
/// The lowest LUFS of the distribution (the lower edge of the first bin).
pub const DIST_LOW: f32 = -46.0;
/// Most history points kept (15 minutes at one every half second).
pub const HISTORY_MAX: usize = 1_800;
/// Seconds of programme the distribution, the LRA and the history leave out after a reset.
pub const SETTLE_S: f64 = 3.0;
/// "On target" is within this many LU of it.
pub const TOLERANCE_LU: f32 = 1.0;

/// Names of the `preset` choice, `Custom` last.
pub const PRESET_NAMES: [&str; 7] = [
    "Streaming -14 LUFS / -1 dBTP",
    "Streaming -16 LUFS",
    "Broadcast EBU R128 -23 LUFS",
    "Broadcast ATSC A/85 -24 LKFS",
    "Podcast -16 LUFS",
    "Club / loud master -9 LUFS",
    "Custom",
];
/// The `preset` value of `Custom`.
pub const PRESET_CUSTOM: usize = 6;
/// Target loudness (LUFS) and maximum true peak (dBTP) of each preset but `Custom`.
pub const PRESETS: [(f32, f32); 6] = [
    (-14.0, -1.0),
    (-16.0, -1.0),
    (-23.0, -1.0),
    (-24.0, -2.0),
    (-16.0, -1.0),
    (-9.0, -1.0),
];
/// Names of the `history_span` choice.
pub const SPAN_NAMES: [&str; 4] = ["30 s", "1 min", "5 min", "15 min"];
/// Seconds of each `history_span` choice.
pub const SPAN_SECONDS: [f32; 4] = [30.0, 60.0, 300.0, 900.0];

const P_PRESET: u32 = 0;
const P_TARGET: u32 = 1;
const P_MAX_TP: u32 = 2;
const P_SPAN: u32 = 3;
const P_RESET: u32 = 4;
const P_HOLD: u32 = 5;

/// The index of each published value (see the module docs).
pub mod value {
    /// Momentary loudness, LUFS.
    pub const MOMENTARY: usize = 0;
    /// Short-term loudness, LUFS.
    pub const SHORT_TERM: usize = 1;
    /// Integrated loudness, LUFS.
    pub const INTEGRATED: usize = 2;
    /// Loudness range, LU (-120 before there is one).
    pub const LRA: usize = 3;
    /// True peak since reset, both channels, dBTP.
    pub const TRUE_PEAK: usize = 4;
    /// True peak since reset, left, dBTP.
    pub const TRUE_PEAK_L: usize = 5;
    /// True peak since reset, right, dBTP.
    pub const TRUE_PEAK_R: usize = 6;
    /// Sample peak since reset, dBFS.
    pub const SAMPLE_PEAK: usize = 7;
    /// Peak to loudness ratio, dB (-120: none yet).
    pub const PLR: usize = 8;
    /// Peak to short-term ratio, dB (-120: none yet).
    pub const PSR: usize = 9;
    /// Crest factor of the programme, dB (-120: none yet).
    pub const DR: usize = 10;
    /// The target loudness in force, LUFS.
    pub const TARGET: usize = 11;
    /// The maximum true peak in force, dBTP.
    pub const MAX_TP: usize = 12;
    /// Counts the resets (never cleared): a change means the history started over.
    pub const EPOCH: usize = 13;
    /// History points since the last reset (the newest frames of the scope tap to read).
    pub const HISTORY_POINTS: usize = 14;
    /// Seconds of programme accumulated since the last reset.
    pub const TIME: usize = 15;
    /// The first distribution bin; [`super::BINS`] values follow.
    pub const BINS: usize = 16;
}

/// How an integrated loudness compares with a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing measured yet.
    NoData,
    /// Within [`TOLERANCE_LU`] of the target.
    Ok,
    /// More than [`TOLERANCE_LU`] above the target.
    TooLoud,
    /// More than [`TOLERANCE_LU`] below the target.
    TooQuiet,
}

/// Compares an integrated loudness (LUFS; -120 or less is "nothing yet") with a target.
pub fn verdict(integrated: f32, target: f32) -> Verdict {
    if !integrated.is_finite() || integrated <= -119.5 {
        Verdict::NoData
    } else if integrated > target + TOLERANCE_LU + 1e-4 {
        Verdict::TooLoud
    } else if integrated < target - TOLERANCE_LU - 1e-4 {
        Verdict::TooQuiet
    } else {
        Verdict::Ok
    }
}

/// Whether a true peak (dBTP) respects the maximum: it fails only when it is above it.
pub fn peak_passes(true_peak: f32, max_tp: f32) -> bool {
    !(true_peak.is_finite() && true_peak > max_tp + 1e-4)
}

/// The target loudness and maximum true peak of preset choice `preset`, with `custom` the
/// `(target_lufs, max_tp)` pair `Custom` uses.
pub fn target_of(preset: usize, custom: (f32, f32)) -> (f32, f32) {
    PRESETS.get(preset).copied().unwrap_or(custom)
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
        P_PRESET,
        "preset",
        "Target preset",
        (0.0, 6.0, 0.0),
        Taper::Linear,
        Unit::Choice(&PRESET_NAMES),
        true,
    ),
    param(
        P_TARGET,
        "target_lufs",
        "Custom target (LUFS)",
        (-40.0, 0.0, -14.0),
        Taper::Linear,
        Unit::None,
        false,
    ),
    param(
        P_MAX_TP,
        "max_tp",
        "Custom max true peak (dBTP)",
        (-12.0, 0.0, -1.0),
        Taper::Linear,
        Unit::Db,
        false,
    ),
    param(
        P_SPAN,
        "history_span",
        "History span",
        (0.0, 3.0, 1.0),
        Taper::Linear,
        Unit::Choice(&SPAN_NAMES),
        true,
    ),
    param(
        P_RESET,
        "reset",
        "Reset meter",
        (0.0, 1.0, 0.0),
        Taper::Linear,
        Unit::Toggle,
        true,
    ),
    param(
        P_HOLD,
        "hold",
        "Hold",
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

/// Static description of the loudness meter.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.loudness-meter",
    name: "Loudness Meter",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`LoudnessMeterDevice`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(LoudnessMeterDevice::new())),
    }
}

/// The bin of a short-term loudness in the distribution.
fn bin_of(lufs: f32) -> usize {
    (((lufs - DIST_LOW).floor().max(0.0)) as usize).min(BINS - 1)
}

/// The loudness meter device (module docs).
pub struct LoudnessMeterDevice {
    sample_rate: f64,
    tap: Arc<ScopeTap>,
    loudness: LoudnessMeter,
    true_peak: [PeakDetector; 2],
    history: LoudnessHistory,
    // Accumulated since the last reset.
    true_peak_max: [f32; 2],
    sample_peak: f32,
    energy: f64,
    energy_frames: u64,
    frames: u64,
    second: u64,
    half: u64,
    points: usize,
    counts: [u32; BINS],
    bins: [f32; BINS],
    lra: Option<f32>,
    epoch: u32,
    // The largest true peak of each of the last 30 sub-blocks of 100 ms (for PSR).
    block_peaks: [f32; 30],
    block_pos: usize,
    block_max: f32,
    block_frames: usize,
    sub_len: usize,
    // Settings.
    preset: usize,
    custom: (f32, f32),
    held: bool,
    reset_level: bool,
    reset_pending: bool,
    values: [f32; METERS],
}

impl std::fmt::Debug for LoudnessMeterDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoudnessMeterDevice")
            .field("values", &self.values)
            .finish_non_exhaustive()
    }
}

impl Default for LoudnessMeterDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl LoudnessMeterDevice {
    /// A meter at 48 kHz with default settings (`prepare` sizes it for the real rate).
    pub fn new() -> Self {
        let mut meter = Self {
            sample_rate: 48_000.0,
            // 4096 frames: a read returns up to half of them, more than HISTORY_MAX.
            tap: Arc::new(ScopeTap::with_frames(4_096)),
            loudness: LoudnessMeter::new(48_000.0),
            true_peak: [PeakDetector::new(BATCH), PeakDetector::new(BATCH)],
            history: LoudnessHistory::new(),
            true_peak_max: [0.0; 2],
            sample_peak: 0.0,
            energy: 0.0,
            energy_frames: 0,
            frames: 0,
            second: 0,
            half: 0,
            points: 0,
            counts: [0; BINS],
            bins: [0.0; BINS],
            lra: None,
            epoch: 0,
            block_peaks: [0.0; 30],
            block_pos: 0,
            block_max: 0.0,
            block_frames: 0,
            sub_len: 4_800,
            preset: 0,
            custom: (-14.0, -1.0),
            held: false,
            reset_level: false,
            reset_pending: false,
            values: [0.0; METERS],
        };
        meter.clear_state();
        meter
    }

    fn rate(&self) -> u64 {
        self.sample_rate.round() as u64
    }

    /// Clears everything accumulated (not the settings, not the epoch counter, which counts
    /// the clears).
    fn clear_state(&mut self) {
        self.loudness.reset();
        for t in &mut self.true_peak {
            t.reset();
        }
        self.history.reset();
        self.true_peak_max = [0.0; 2];
        self.sample_peak = 0.0;
        self.energy = 0.0;
        self.energy_frames = 0;
        self.frames = 0;
        self.second = 0;
        self.half = 0;
        self.points = 0;
        self.counts = [0; BINS];
        self.bins = [0.0; BINS];
        self.lra = None;
        self.block_peaks = [0.0; 30];
        self.block_pos = 0;
        self.block_max = 0.0;
        self.block_frames = 0;
        self.reset_pending = false;
        self.epoch = self.epoch.wrapping_add(1) & 0x00ff_ffff;
        self.publish();
    }

    /// The target loudness and maximum true peak in force.
    pub fn target(&self) -> (f32, f32) {
        target_of(self.preset, self.custom)
    }

    /// Adds one batch to everything that accumulates.
    fn accumulate(&mut self, l: &Batch, r: &Batch, tp: [f32; 2]) {
        for (m, t) in self.true_peak_max.iter_mut().zip(tp) {
            *m = m.max(t);
        }
        self.sample_peak = self.sample_peak.max(batch_peak(l)).max(batch_peak(r));
        let mut e = 0.0f64;
        for x in l.iter().chain(r) {
            if x.is_finite() {
                e += f64::from(*x) * f64::from(*x);
            }
        }
        self.energy += e;
        self.energy_frames += BATCH as u64;
        self.frames += BATCH as u64;
        let rate = self.rate().max(1);
        if self.frames as f64 >= SETTLE_S * self.sample_rate {
            self.second += BATCH as u64;
            if self.second >= rate {
                self.second -= rate;
                self.record_second(rate);
            }
            self.half += BATCH as u64;
            if self.half >= rate / 2 {
                self.half -= rate / 2;
                self.tap
                    .push(&[self.loudness.short_term()], &[self.loudness.momentary()]);
                self.points = (self.points + 1).min(HISTORY_MAX);
            }
        }
    }

    /// Once a second: the short-term loudness joins the range and the distribution.
    fn record_second(&mut self, rate: u64) {
        let short = self.loudness.short_term();
        self.history.push(short, rate, self.sample_rate as f32);
        self.lra = self.history.lra();
        if short > -70.0 {
            self.counts[bin_of(short)] += 1;
        }
        let tallest = self.counts.iter().copied().max().unwrap_or(0).max(1);
        for (b, c) in self.bins.iter_mut().zip(&self.counts) {
            *b = *c as f32 / tallest as f32;
        }
    }

    /// Writes the published values from the state.
    fn publish(&mut self) {
        let short = self.loudness.short_term();
        let integrated = self.loudness.integrated();
        let tp = self.true_peak_max[0].max(self.true_peak_max[1]);
        let tp_db = db(tp);
        let sample_peak_db = db(self.sample_peak);
        let (target, max_tp) = self.target();
        let none = FLOOR_DB;
        let v = &mut self.values;
        v[value::MOMENTARY] = self.loudness.momentary();
        v[value::SHORT_TERM] = short;
        v[value::INTEGRATED] = integrated;
        v[value::LRA] = self.lra.unwrap_or(none);
        v[value::TRUE_PEAK] = tp_db;
        v[value::TRUE_PEAK_L] = db(self.true_peak_max[0]);
        v[value::TRUE_PEAK_R] = db(self.true_peak_max[1]);
        v[value::SAMPLE_PEAK] = sample_peak_db;
        v[value::PLR] = if tp_db > -119.5 && integrated > -119.5 {
            tp_db - integrated
        } else {
            none
        };
        let recent = self.block_peaks.iter().fold(0.0f32, |m, p| m.max(*p));
        v[value::PSR] = if recent > 0.0 && short > -119.5 {
            db(recent) - short
        } else {
            none
        };
        v[value::DR] = if self.energy_frames > 0 && self.energy > 0.0 && sample_peak_db > -119.5 {
            let mean = self.energy / (2 * self.energy_frames) as f64;
            sample_peak_db - (10.0 * mean.log10()) as f32
        } else {
            none
        };
        v[value::TARGET] = target;
        v[value::MAX_TP] = max_tp;
        v[value::EPOCH] = self.epoch as f32;
        v[value::HISTORY_POINTS] = self.points as f32;
        v[value::TIME] = (self.frames as f64 / self.sample_rate) as f32;
        v[value::BINS..].copy_from_slice(&self.bins);
    }
}

impl Device for LoudnessMeterDevice {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz();
        self.loudness = LoudnessMeter::new(self.sample_rate);
        self.loudness.set_accumulate(!self.held);
        self.sub_len = ((self.sample_rate * 0.1).round() as usize).max(1);
        self.reset();
    }

    fn reset(&mut self) {
        self.clear_state();
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        match id.0 {
            P_PRESET => self.preset = v.round().clamp(0.0, PRESET_CUSTOM as f32) as usize,
            P_TARGET => self.custom.0 = v.clamp(-40.0, 0.0),
            P_MAX_TP => self.custom.1 = v.clamp(-12.0, 0.0),
            P_RESET => {
                let level = v >= 0.5;
                if level && !self.reset_level {
                    self.reset_pending = true;
                }
                self.reset_level = level;
            }
            P_HOLD => {
                self.held = v >= 0.5;
                self.loudness.set_accumulate(!self.held);
            }
            // The history span is the UI's.
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

        self.loudness.process(&l, &r);
        // The detectors see finite samples only, so a stray NaN cannot poison their filters.
        let mut tp = [0.0f32; 2];
        let mut pk = [0.0f32; BATCH];
        for (c, x) in [&l, &r].into_iter().enumerate() {
            let clean: Batch = std::array::from_fn(|i| if x[i].is_finite() { x[i] } else { 0.0 });
            self.true_peak[c].process(&clean, &mut pk, false);
            tp[c] = pk.iter().fold(0.0f32, |m, v| m.max(*v));
        }
        self.block_max = self.block_max.max(tp[0]).max(tp[1]);
        self.block_frames += BATCH;
        if self.block_frames >= self.sub_len {
            self.block_peaks[self.block_pos] = self.block_max;
            self.block_pos = (self.block_pos + 1) % self.block_peaks.len();
            self.block_max = 0.0;
            self.block_frames = 0;
        }
        if !self.held {
            self.accumulate(&l, &r, tp);
        }
        self.publish();
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
