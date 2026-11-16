//! `atomik.tonal-balance`: a tonal balance meter that passes the audio through untouched.
//!
//! ```text
//! in_l, in_r ---------------------------------> out_l, out_r   (the same samples, no latency)
//!      '--- 64 frames per batch --> ScopeTap  (the UI reads it and does the rest)
//! ```
//!
//! It is the spectrum analyser's architecture with another reading. The device copies its two
//! inputs to its two outputs bit for bit and pushes the same frames into its
//! [`ScopeTap`](atomik_device_sdk::ScopeTap); nothing in the output depends on the settings, so
//! a bounce with the device equals a bounce without it, to the bit. The analysis is the app's
//! (`atomik_dsp::tonal`): it reads the tap, averages the one-third-octave band levels over a few
//! seconds, loudness-normalises them and compares four regions (LOW below 150 Hz, LOW-MID 150 Hz
//! to 1 kHz, HIGH-MID 1 kHz to 5 kHz, HIGH above 5 kHz) with a target, showing for each whether
//! it sits inside the target's range or is too loud or too quiet. The params are display
//! settings: the device stores them (they are part of the project and of presets) and does not
//! use them.
//!
//! * **`hidden.target`** (set by the panel's target list, after `bypass`, so it has no knob and
//!   is not offered to automation) names the target: 0 to 3 are the built-ins (Balanced, Bright,
//!   Warm, Bass-heavy), 4 is the target just learned and not saved yet (it lives in the
//!   session), and 10 and above is a saved target of the user, `10 + id`. The saved targets are
//!   kept with the user's settings, not in the project, so a project opened where a target does
//!   not exist falls back to Balanced and the panel says so.
//! * **`response`** is the averaging time: fast 1.5 s, medium 4 s, slow 10 s.
//! * **`view`**: the four region meters, the detailed curve, or both.
//! * **`channel`**: the sum of left and right, the mid or the side.
//! * **`learn`** switches the live learn on: while it is on the app accumulates the balance of
//!   what plays, and when it goes off the result is the unsaved learned target.
//! * **Meter values** (2): the peak of the last 16 batches of the left and the right input.
//! * **Sockets.** `in_l`, `in_r`, `out_l`, `out_r`: unconnected inputs read silence.

use crate::bypass::{Bypassable, bypass_param};
use crate::recent_peak::{RecentPeak, batch_peak};
use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    ScopeTap, SocketDescriptor, SocketId, SocketKind,
};
use std::sync::Arc;

/// Names of the `response` choice.
pub const RESPONSE_NAMES: [&str; 3] = ["Fast", "Medium", "Slow"];
/// The averaging time of each `response` choice in ms.
pub const RESPONSE_MS: [f32; 3] = [1_500.0, 4_000.0, 10_000.0];
/// Names of the `view` choice.
pub const VIEW_NAMES: [&str; 3] = ["Bands", "Curve", "Both"];
/// Names of the `channel` choice.
pub const CHANNEL_NAMES: [&str; 3] = ["Sum", "Mid", "Side"];
/// The `target` value of the target learned and not saved yet.
pub const TARGET_LEARNED: f32 = 4.0;
/// The `target` value of the user target with id 0 (a user target with id `n` is `10 + n`).
pub const TARGET_USER_BASE: f32 = 10.0;
/// The largest `target` value, so the largest user id is 990.
pub const TARGET_MAX: f32 = 1_000.0;

const P_RESPONSE: u32 = 0;
const P_VIEW: u32 = 1;
const P_CHANNEL: u32 = 2;
const P_LEARN: u32 = 3;
const P_TARGET: u32 = 5;

const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    max: f32,
    default: f32,
    unit: Unit,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: 0.0,
        max,
        default,
        taper: Taper::Linear,
        unit,
        discrete: true,
    }
}

static PARAMS: [ParamDescriptor; 6] = [
    param(
        P_RESPONSE,
        "response",
        "Response",
        2.0,
        1.0,
        Unit::Choice(&RESPONSE_NAMES),
    ),
    param(P_VIEW, "view", "View", 2.0, 2.0, Unit::Choice(&VIEW_NAMES)),
    param(
        P_CHANNEL,
        "channel",
        "Channel",
        2.0,
        0.0,
        Unit::Choice(&CHANNEL_NAMES),
    ),
    param(P_LEARN, "learn", "Learn", 1.0, 0.0, Unit::Toggle),
    bypass_param(4),
    // The panel's own list sets it (no knob, no automation, no mapping).
    param(
        P_TARGET,
        "hidden.target",
        "Target",
        TARGET_MAX,
        0.0,
        Unit::None,
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

/// Static description of the tonal balance meter.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.tonal-balance",
    name: "Tonal Balance",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`TonalBalance`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(TonalBalance::new())),
    }
}

/// The tonal balance meter device (module docs).
#[derive(Debug)]
pub struct TonalBalance {
    tap: Arc<ScopeTap>,
    peaks: [RecentPeak; 2],
    meters: [f32; 2],
}

impl Default for TonalBalance {
    fn default() -> Self {
        Self::new()
    }
}

impl TonalBalance {
    /// A device with its tap (allocated here, not in `prepare`: the tap outlives re-prepares).
    pub fn new() -> Self {
        Self {
            tap: Arc::new(ScopeTap::new()),
            peaks: [RecentPeak::default(); 2],
            meters: [0.0; 2],
        }
    }
}

impl Device for TonalBalance {
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
