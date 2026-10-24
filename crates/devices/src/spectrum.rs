//! `atomik.spectrum`: a spectrum analyser that passes the audio through untouched.
//!
//! ```text
//! in_l, in_r ---------------------------------> out_l, out_r   (the same samples, no latency)
//!      '--- 64 frames per batch --> ScopeTap  (the UI reads it and does the rest)
//! ```
//!
//! * **Audio path.** Each batch the device copies the two inputs to the two outputs bit for bit
//!   and pushes the same 64 frames into its [`ScopeTap`](atomik_device_sdk::ScopeTap), a
//!   lock-free ring of the last 32 768 frames: 128 relaxed atomic stores and one release store.
//!   No FFT, no allocation, no lock, no latency, and nothing in the output depends on the
//!   settings, so a bounce with the device equals a bounce without it, to the bit.
//! * **The analysis is the UI's.** The app reads the tap at the display rate and runs
//!   `atomik_dsp::spectrum` on it (FFT of `size` points with the `window`, the `channel`, the
//!   `averaging`, the peak `hold` and `decay`, the `tilt`, the 20 Hz to 20 kHz log axis over the
//!   `range` dB below 0 dBFS, and with `mode` 1 the one-third-octave bands). The params are
//!   therefore display settings: the device stores them (they are part of the project and of
//!   presets) and does not use them. A bypassed device keeps its audio path but the panel stops
//!   updating and says so.
//! * **Meter values** (2): the peak of the last 16 batches of the left and the right input, linear.
//! * **Sockets.** `in_l`, `in_r`, `out_l`, `out_r`: unconnected inputs read silence.

use crate::bypass::{Bypassable, bypass_param};
use crate::recent_peak::{RecentPeak, batch_peak};
use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    ScopeTap, SocketDescriptor, SocketId, SocketKind,
};
use std::sync::Arc;

/// Transform sizes of the `size` choice.
pub const SIZES: [usize; 4] = [1024, 2048, 4096, 8192];
/// Names of the `size` choice.
pub const SIZE_NAMES: [&str; 4] = ["1024", "2048", "4096", "8192"];
/// Names of the `window` choice.
pub const WINDOW_NAMES: [&str; 2] = ["Hann", "B-Harris"];
/// Names of the `channel` choice.
pub const CHANNEL_NAMES: [&str; 5] = ["Left", "Right", "Mid", "Side", "Sum"];
/// Names of the `mode` choice.
pub const MODE_NAMES: [&str; 2] = ["Fine", "1/3 oct"];
/// Names of the `tilt` choice.
pub const TILT_NAMES: [&str; 3] = ["0 dB", "3 dB", "4.5 dB"];
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

static PARAMS: [ParamDescriptor; 10] = [
    param(
        P_SIZE,
        "size",
        "FFT size",
        0.0,
        3.0,
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
        4.0,
        4.0,
        Unit::Choice(&CHANNEL_NAMES),
        true,
    ),
    param(
        P_MODE,
        "mode",
        "Display",
        0.0,
        1.0,
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
    bypass_param(9),
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
        descriptor: &DESCRIPTOR,
        create: || Box::new(Bypassable::new(Spectrum::new())),
    }
}

/// The spectrum analyser device (module docs).
#[derive(Debug)]
pub struct Spectrum {
    tap: Arc<ScopeTap>,
    peaks: [RecentPeak; 2],
    meters: [f32; 2],
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
            tap: Arc::new(ScopeTap::new()),
            peaks: [RecentPeak::default(); 2],
            meters: [0.0; 2],
        }
    }
}

impl Device for Spectrum {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, _info: &PrepareInfo) {
        self.reset();
    }

    fn reset(&mut self) {
        for p in &mut self.peaks {
            p.clear();
        }
        self.meters = [0.0; 2];
    }

    fn set_param(&mut self, _id: ParamId, _plain: f32) {
        // Display settings: the UI reads them from the document.
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let (l, r) = (*ctx.input(IN_L), *ctx.input(IN_R));
        *ctx.output(OUT_L) = l;
        *ctx.output(OUT_R) = r;
        self.tap.push(&l, &r);
        self.meters = [
            self.peaks[0].push(batch_peak(&l)),
            self.peaks[1].push(batch_peak(&r)),
        ];
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
