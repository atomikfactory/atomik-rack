//! `atomik.spectrum`: a spectrum analyser that passes the audio through untouched.
//!
//! ```text
//! in_l, in_r ---------------------------------> out_l, out_r   (the same samples, no latency)
//!      '--- 64 frames per batch --> ScopeTap  (the UI reads it and does the rest)
//! ```
//!
//! * **Audio path.** Each batch the device copies the two inputs to the two outputs bit for bit
//!   and pushes the same 64 frames into its [`ScopeTap`](atomik_device_sdk::ScopeTap), a
//!   lock-free ring of the last 131 072 frames (1 MiB: the analysis of 65536 points needs a
//!   ring twice its size): 128 relaxed atomic stores and one release store.
//!   No FFT, no allocation, no lock, no latency, and nothing in the output depends on the
//!   settings, so a bounce with the device equals a bounce without it, to the bit.
//! * **The analysis is the UI's.** The app reads the tap at the display rate and runs
//!   `atomik_dsp::spectrum` on it (FFT of `size` points with the `window`, the `channel`, the
//!   `averaging`, the peak `hold` and `decay`, the `tilt`, the 20 Hz to 20 kHz log axis over the
//!   `range` dB below 0 dBFS, and with `mode` 1 the one-third-octave bands, with `mode` 2 a
//!   spectrogram: the same fine trace as a scrolling image, newest row on top). `size` goes up
//!   to 65536 points (the FFT runs in the UI, never on the audio thread), `smoothing` averages the fine trace across neighbouring bins over a
//!   fraction of an octave and `weighting` applies the K-weighting of the loudness meters. The
//!   `reference` choice draws a stored band curve over the trace (as captured, or shifted to
//!   the level of the signal) and a rising edge of `reference.capture` stores the current
//!   bands as that curve (kept with the user's settings, not in the project). The params are
//!   therefore display settings: the device stores them (they are part of the project and of
//!   presets) and does not use them. A bypassed device keeps its audio path but the panel stops
//!   updating and says so.
//! * **Measurements, exact.** The device sees every sample, so what must be exact is measured
//!   here and published as meter values (see [`value`]): the stereo correlation over about 300 ms
//!   and over 3 s ([`StereoMeter`]), the sample peak and the true peak (4x oversampled,
//!   [`PeakDetector`], the limiter's detector) of each channel since the last reset in dBFS, and
//!   the number of clipped samples since the last reset (a sample whose magnitude reaches
//!   `clip_level`, 0 dBFS by default; both channels are counted, the count saturates at 2^24 so
//!   it stays exact as a float). `stats.reset` is a momentary toggle: a rising edge clears the
//!   peaks and the clip count (leave it at 0 afterwards; the UI pulses it). They also clear when
//!   the device is prepared, and not on a transport reset. The headroom is 0 dBFS minus the
//!   largest peak: the UI works it out. Non-finite samples pass through and count as silence in
//!   the measurements.
//! * **Overlay channels.** The `channel` choice has two more entries at the end, `L + R` and
//!   `M + S`: the UI analyses both channels (or mid and side) and draws two traces. The audio
//!   path does not care.
//! * **Meter values** ([`METER_COUNT`] = 9, indices in [`value`]): the peak of the last 16
//!   batches of the left and the right input (linear), then the measurements above.
//! * **Sockets.** `in_l`, `in_r`, `out_l`, `out_r`: unconnected inputs read silence.

use crate::bypass::{Bypassable, bypass_param};
use crate::mastering::loudness::{FLOOR_DB, PeakDetector};
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
pub const METER_COUNT: usize = 9;

/// Indices of the meter values.
pub mod value {
    /// Peak of the last 16 batches, left input (linear).
    pub const PEAK_L: usize = 0;
    /// Peak of the last 16 batches, right input (linear).
    pub const PEAK_R: usize = 1;
    /// Stereo correlation over about 300 ms, -1..=1 (0 for silence or one silent side).
    pub const CORRELATION: usize = 2;
    /// Stereo correlation over about 3 s, -1..=1.
    pub const CORRELATION_LONG: usize = 3;
    /// Largest sample magnitude since the reset, left, dBFS (-120 for none).
    pub const SAMPLE_PEAK_L_DB: usize = 4;
    /// Largest sample magnitude since the reset, right, dBFS.
    pub const SAMPLE_PEAK_R_DB: usize = 5;
    /// Largest 4x oversampled magnitude since the reset, left, dBFS.
    pub const TRUE_PEAK_L_DB: usize = 6;
    /// Largest 4x oversampled magnitude since the reset, right, dBFS.
    pub const TRUE_PEAK_R_DB: usize = 7;
    /// Samples at or over the clip level since the reset (both channels, at most 2^24).
    pub const CLIPS: usize = 8;
}

/// The clip count stops here: the largest integer an `f32` holds exactly.
pub const CLIP_MAX: u32 = 1 << 24;

/// Transform sizes of the `size` choice.
pub const SIZES: [usize; 7] = [1024, 2048, 4096, 8192, 16_384, 32_768, 65_536];
/// Names of the `size` choice.
pub const SIZE_NAMES: [&str; 7] = ["1024", "2048", "4096", "8192", "16384", "32768", "65536"];
/// Names of the `window` choice.
pub const WINDOW_NAMES: [&str; 2] = ["Hann", "B-Harris"];
/// Names of the `channel` choice.
/// The last two draw two traces (left and right, mid and side) at once.
pub const CHANNEL_NAMES: [&str; 7] = ["Left", "Right", "Mid", "Side", "Sum", "L + R", "M + S"];
/// Names of the `mode` choice.
pub const MODE_NAMES: [&str; 3] = ["Fine", "1/3 oct", "Spectrogram"];
/// Names of the `tilt` choice.
pub const TILT_NAMES: [&str; 3] = ["0 dB", "3 dB", "4.5 dB"];
/// Names of the `smoothing` choice: the fraction of an octave the fine trace is averaged over.
pub const SMOOTHING_NAMES: [&str; 4] = ["Off", "1/12 oct", "1/6 oct", "1/3 oct"];
/// Names of the `weighting` choice.
pub const WEIGHTING_NAMES: [&str; 2] = ["Flat", "K-weighted"];
/// Names of the `reference` choice: the stored reference curve is hidden, drawn as it was
/// captured, or drawn shifted to the level of the signal.
pub const REFERENCE_NAMES: [&str; 3] = ["Off", "Stored", "Stored, level matched"];
/// The slope of each `tilt` choice in dB per octave.
pub const TILT_DB_PER_OCT: [f32; 3] = [0.0, 3.0, 4.5];

const P_SIZE: u32 = 0;
const P_WINDOW: u32 = 1;
const P_CHANNEL: u32 = 2;
const P_MODE: u32 = 3;
const P_AVERAGING: u32 = 4;
const P_HOLD: u32 = 5;
const P_DECAY: u32 = 6;
const P_RANGE: u32 = 7;
const P_TILT: u32 = 8;
const P_SMOOTHING: u32 = 9;
const P_WEIGHTING: u32 = 10;
const P_REFERENCE: u32 = 11;
const P_CAPTURE: u32 = 12;
// 13 is the bypass.
const P_STATS_RESET: u32 = 14;
const P_CLIP_LEVEL: u32 = 15;

#[allow(clippy::too_many_arguments)]
const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
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
        taper: Taper::Linear,
        unit,
        discrete,
    }
}

static PARAMS: [ParamDescriptor; 16] = [
    param(
        P_SIZE,
        "size",
        "FFT size",
        0.0,
        6.0,
        2.0,
        Unit::Choice(&SIZE_NAMES),
        true,
    ),
    param(
        P_WINDOW,
        "window",
        "Window",
        0.0,
        1.0,
        0.0,
        Unit::Choice(&WINDOW_NAMES),
        true,
    ),
    param(
        P_CHANNEL,
        "channel",
        "Channel",
        0.0,
        6.0,
        4.0,
        Unit::Choice(&CHANNEL_NAMES),
        true,
    ),
    param(
        P_MODE,
        "mode",
        "Display",
        0.0,
        2.0,
        0.0,
        Unit::Choice(&MODE_NAMES),
        true,
    ),
    param(
        P_AVERAGING,
        "averaging",
        "Averaging (ms)",
        0.0,
        2_000.0,
        150.0,
        Unit::Ms,
        false,
    ),
    param(
        P_HOLD,
        "hold",
        "Peak hold (ms)",
        0.0,
        5_000.0,
        1_000.0,
        Unit::Ms,
        false,
    ),
    param(
        P_DECAY,
        "decay",
        "Peak decay (dB/s)",
        1.0,
        60.0,
        15.0,
        Unit::None,
        false,
    ),
    param(
        P_RANGE,
        "range",
        "Range (dB)",
        30.0,
        120.0,
        90.0,
        Unit::Db,
        false,
    ),
    param(
        P_TILT,
        "tilt",
        "Tilt (dB/oct)",
        0.0,
        2.0,
        0.0,
        Unit::Choice(&TILT_NAMES),
        true,
    ),
    param(
        P_SMOOTHING,
        "smoothing",
        "Smoothing",
        0.0,
        3.0,
        0.0,
        Unit::Choice(&SMOOTHING_NAMES),
        true,
    ),
    param(
        P_WEIGHTING,
        "weighting",
        "Weighting",
        0.0,
        1.0,
        0.0,
        Unit::Choice(&WEIGHTING_NAMES),
        true,
    ),
    param(
        P_REFERENCE,
        "reference",
        "Reference curve",
        0.0,
        2.0,
        0.0,
        Unit::Choice(&REFERENCE_NAMES),
        true,
    ),
    param(
        P_CAPTURE,
        "reference.capture",
        "Capture reference",
        0.0,
        1.0,
        0.0,
        Unit::Toggle,
        true,
    ),
    bypass_param(13),
    param(
        P_STATS_RESET,
        "stats.reset",
        "Reset peaks and clips",
        0.0,
        1.0,
        0.0,
        Unit::Toggle,
        true,
    ),
    param(
        P_CLIP_LEVEL,
        "clip_level",
        "Clip level (dB)",
        -12.0,
        0.0,
        0.0,
        Unit::Db,
        false,
    ),
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

/// Static description of the spectrum analyser.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.spectrum",
    name: "Spectrum",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Spectrum`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(Spectrum::new())),
    }
}

/// The spectrum analyser device (module docs).
#[derive(Debug)]
pub struct Spectrum {
    tap: Arc<ScopeTap>,
    peaks: [RecentPeak; 2],
    meters: [f32; METER_COUNT],
    corr: StereoMeter,
    corr_long: StereoMeter,
    true_peak: [PeakDetector; 2],
    /// Largest sample magnitude since the reset, per channel (linear).
    sample_max: [f32; 2],
    /// Largest 4x magnitude since the reset, per channel (linear).
    true_max: [f32; 2],
    clips: u32,
    clip_lin: f32,
    reset_level: bool,
    reset_pending: bool,
}

impl Default for Spectrum {
    fn default() -> Self {
        Self::new()
    }
}

impl Spectrum {
    /// A device with its tap (allocated here, not in `prepare`: the tap outlives re-prepares).
    pub fn new() -> Self {
        Self {
            tap: Arc::new(ScopeTap::large()),
            peaks: [RecentPeak::default(); 2],
            meters: Self::resting(),
            corr: StereoMeter::new(48_000.0, CORR_MS),
            corr_long: StereoMeter::new(48_000.0, CORR_LONG_MS),
            true_peak: [PeakDetector::new(BATCH), PeakDetector::new(BATCH)],
            sample_max: [0.0; 2],
            true_max: [0.0; 2],
            clips: 0,
            clip_lin: 1.0,
            reset_level: false,
            reset_pending: false,
        }
    }

    /// The published values at rest.
    fn resting() -> [f32; METER_COUNT] {
        let mut m = [0.0; METER_COUNT];
        for i in PEAK_DB_VALUES {
            m[i] = FLOOR_DB;
        }
        m
    }

    /// Clears the peaks and the clip count (not the settings).
    fn clear_stats(&mut self) {
        for t in &mut self.true_peak {
            t.reset();
        }
        self.sample_max = [0.0; 2];
        self.true_max = [0.0; 2];
        self.clips = 0;
        self.reset_pending = false;
        for i in PEAK_DB_VALUES {
            self.meters[i] = FLOOR_DB;
        }
        self.meters[value::CLIPS] = 0.0;
    }
}

/// The meter values that are peaks in dBFS.
const PEAK_DB_VALUES: [usize; 4] = [
    value::SAMPLE_PEAK_L_DB,
    value::SAMPLE_PEAK_R_DB,
    value::TRUE_PEAK_L_DB,
    value::TRUE_PEAK_R_DB,
];

/// Averaging time of the correlation readout, ms.
const CORR_MS: f32 = 300.0;
/// Averaging time of the long correlation readout, ms.
const CORR_LONG_MS: f32 = 3_000.0;

/// `20 log10(x)` of a linear amplitude, [`FLOOR_DB`] at silence.
fn db(x: f32) -> f32 {
    if x > 1e-6 {
        (6.020_6 * log2_det(x)).max(FLOOR_DB)
    } else {
        FLOOR_DB
    }
}

impl Device for Spectrum {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        let rate = info.sample_rate.hz() as f32;
        self.corr = StereoMeter::new(rate, CORR_MS);
        self.corr_long = StereoMeter::new(rate, CORR_LONG_MS);
        self.reset();
        self.clear_stats();
    }

    fn reset(&mut self) {
        for p in &mut self.peaks {
            p.clear();
        }
        self.corr.reset();
        self.corr_long.reset();
        for i in [
            value::PEAK_L,
            value::PEAK_R,
            value::CORRELATION,
            value::CORRELATION_LONG,
        ] {
            self.meters[i] = 0.0;
        }
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        let v = if plain.is_finite() { plain } else { 0.0 };
        match id.0 {
            P_STATS_RESET => {
                let level = v >= 0.5;
                if level && !self.reset_level {
                    self.reset_pending = true;
                }
                self.reset_level = level;
            }
            P_CLIP_LEVEL => {
                // Exactly 1.0 at 0 dB, so a full-scale sample is a clip.
                self.clip_lin = if v >= 0.0 {
                    1.0
                } else {
                    (v.clamp(-12.0, 0.0) * 0.115_129_25).exp()
                };
            }
            // The other params are display settings: the UI reads them from the document.
            _ => {}
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let (l, r) = (*ctx.input(IN_L), *ctx.input(IN_R));
        *ctx.output(OUT_L) = l;
        *ctx.output(OUT_R) = r;
        self.tap.push(&l, &r);
        if self.reset_pending {
            self.clear_stats();
        }
        self.meters[value::PEAK_L] = self.peaks[0].push(batch_peak(&l));
        self.meters[value::PEAK_R] = self.peaks[1].push(batch_peak(&r));
        self.corr.process(&l, &r);
        self.corr_long.process(&l, &r);
        self.meters[value::CORRELATION] = self.corr.correlation();
        self.meters[value::CORRELATION_LONG] = self.corr_long.correlation();
        let mut pk = [0.0f32; BATCH];
        for (c, x) in [l, r].iter().enumerate() {
            // A non-finite sample is silence to the measurements (it would poison the filter).
            let clean: [f32; BATCH] =
                std::array::from_fn(|i| if x[i].is_finite() { x[i] } else { 0.0 });
            let clipped = clean.iter().filter(|v| v.abs() >= self.clip_lin).count() as u32;
            self.clips = (self.clips + clipped).min(CLIP_MAX);
            self.sample_max[c] = self.sample_max[c].max(batch_peak(&clean));
            self.true_peak[c].process(&clean, &mut pk, false);
            for v in &pk {
                self.true_max[c] = self.true_max[c].max(*v);
            }
            // The sample peak is part of the true peak: a reading cannot be below it.
            self.true_max[c] = self.true_max[c].max(self.sample_max[c]);
        }
        self.meters[value::SAMPLE_PEAK_L_DB] = db(self.sample_max[0]);
        self.meters[value::SAMPLE_PEAK_R_DB] = db(self.sample_max[1]);
        self.meters[value::TRUE_PEAK_L_DB] = db(self.true_max[0]);
        self.meters[value::TRUE_PEAK_R_DB] = db(self.true_max[1]);
        self.meters[value::CLIPS] = self.clips as f32;
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = self.meters.len().min(out.len());
        out[..n].copy_from_slice(&self.meters[..n]);
        n
    }

    fn scope_tap(&self) -> Option<Arc<ScopeTap>> {
        Some(Arc::clone(&self.tap))
    }
}

#[cfg(test)]
mod tests;
