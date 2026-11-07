//! `atomik.space-reverb`: a stereo algorithmic reverb for big and strange spaces.
//!
//! It shares the engine of [`crate::reverb`] (eight modulated feedback lines, tapped early
//! reflections, RT60-calibrated decay; see that module for the signal flow, the decay and
//! damping maths, the determinism rules and the mix behaviour) and differs in its algorithm set
//! and its two extra output stages:
//!
//! * **Algorithms.** *Arena* (47..134 ms lines, widely spaced early reflections), *cathedral*
//!   (62..181 ms lines, late and soft early reflections, dark), *spring* (8..24 ms lines with
//!   a high allpass coefficient: the highs arrive behind the lows, the chirp of a sprung tank)
//!   and *echoverb* (83..311 ms lines with sparse diffusion behind evenly spaced early
//!   repeats, an echo that melts into a tail). `size` scales every delay time as in the
//!   classic reverb.
//! * **High cut.** A one-pole lowpass on the wet signal (1 kHz..20 kHz), next to the low cut
//!   of the pre-delay path.
//! * **Gate.** `gate` is the hold time (0 is off, up to 500 ms) and `gate_level` the input level
//!   in dB that keeps it open: the wet output closes (12 ms fade) `gate` ms after the input
//!   last exceeded the level and opens again within 1 ms, the cut-off tail of a gated
//!   reverb.
//!
//! Latency is 0 and `mix == 0` copies the input bit-exactly, like the classic reverb. Meter
//! values are the same three (output peaks and tail RMS).

use crate::bypass::{Bypassable, bypass_param};
use crate::reverb::{P_GATE, P_GATE_LEVEL, P_HIGHCUT, Reverb};
use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};

const fn param(
    id: u32,
    key: &'static str,
    name: &'static str,
    range: (f32, f32),
    default: f32,
    taper: Taper,
    unit: Unit,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: range.0,
        max: range.1,
        default,
        taper,
        unit,
        discrete: matches!(unit, Unit::Choice(_) | Unit::Toggle),
    }
}

// The ids 0..=12 are those of `atomik.reverb`; the shared engine reads them by number.
static PARAMS: [ParamDescriptor; 17] = [
    param(
        0,
        "algorithm",
        "Algorithm",
        (0.0, 3.0),
        0.0,
        Taper::Linear,
        Unit::Choice(&["arena", "cathedral", "spring", "echoverb"]),
    ),
    param(
        1,
        "predelay",
        "Pre-delay",
        (0.0, 250.0),
        20.0,
        Taper::Linear,
        Unit::Ms,
    ),
    param(
        2,
        "size",
        "Size",
        (0.0, 1.0),
        0.5,
        Taper::Linear,
        Unit::Percent,
    ),
    param(
        3,
        "decay",
        "Decay (s)",
        (0.2, 20.0),
        3.0,
        Taper::Exponential,
        Unit::None,
    ),
    param(
        4,
        "damping",
        "Damping",
        (0.0, 1.0),
        0.4,
        Taper::Linear,
        Unit::Percent,
    ),
    param(
        5,
        "lowcut",
        "Low cut",
        (20.0, 1_000.0),
        80.0,
        Taper::Exponential,
        Unit::Hz,
    ),
    param(
        6,
        "early",
        "Early/late",
        (0.0, 1.0),
        0.5,
        Taper::Linear,
        Unit::Percent,
    ),
    param(
        7,
        "width",
        "Width",
        (0.0, 1.0),
        1.0,
        Taper::Linear,
        Unit::Percent,
    ),
    param(
        8,
        "modulation",
        "Modulation",
        (0.0, 1.0),
        0.5,
        Taper::Linear,
        Unit::Percent,
    ),
    param(
        9,
        "mix",
        "Mix",
        (0.0, 1.0),
        0.3,
        Taper::Linear,
        Unit::Percent,
    ),
    param(
        10,
        "freeze",
        "Freeze",
        (0.0, 1.0),
        0.0,
        Taper::Linear,
        Unit::Toggle,
    ),
    param(
        11,
        "output",
        "Output",
        (-24.0, 12.0),
        0.0,
        Taper::Linear,
        Unit::Db,
    ),
    param(
        12,
        "mix_cv_trim",
        "Mix CV trim",
        (-1.0, 1.0),
        1.0,
        Taper::Linear,
        Unit::Percent,
    ),
    param(
        P_HIGHCUT,
        "highcut",
        "High cut",
        (1_000.0, 20_000.0),
        20_000.0,
        Taper::Exponential,
        Unit::Hz,
    ),
    param(
        P_GATE,
        "gate",
        "Gate hold",
        (0.0, 500.0),
        0.0,
        Taper::Linear,
        Unit::Ms,
    ),
    param(
        P_GATE_LEVEL,
        "gate_level",
        "Gate level",
        (-60.0, 0.0),
        -40.0,
        Taper::Linear,
        Unit::Db,
    ),
    bypass_param(16),
];

const IN_L: SocketId = SocketId(0);
const IN_R: SocketId = SocketId(1);
const OUT_L: SocketId = SocketId(2);
const OUT_R: SocketId = SocketId(3);
const MIX_CV: SocketId = SocketId(4);

const fn socket(
    id: SocketId,
    key: &'static str,
    name: &'static str,
    kind: SocketKind,
    direction: Direction,
) -> SocketDescriptor {
    SocketDescriptor {
        id,
        key,
        name,
        kind,
        direction,
    }
}

static SOCKETS: [SocketDescriptor; 5] = [
    socket(IN_L, "in_l", "In L", SocketKind::Audio, Direction::Input),
    socket(IN_R, "in_r", "In R", SocketKind::Audio, Direction::Input),
    socket(
        OUT_L,
        "out_l",
        "Out L",
        SocketKind::Audio,
        Direction::Output,
    ),
    socket(
        OUT_R,
        "out_r",
        "Out R",
        SocketKind::Audio,
        Direction::Output,
    ),
    socket(
        MIX_CV,
        "mix_cv_in",
        "Mix CV",
        SocketKind::Cv,
        Direction::Input,
    ),
];

/// Static description of the space reverb.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "atomik.space-reverb",
    name: "Space Reverb",
    kind: DeviceKind::Effect,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`SpaceReverb`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: crate::bypass::matched(&DESCRIPTOR),
        create: || Box::new(Bypassable::new(SpaceReverb::new())),
    }
}

/// The space reverb: the shared reverb engine with the space algorithm set.
#[derive(Debug, Clone)]
pub struct SpaceReverb {
    core: Reverb,
}

impl Default for SpaceReverb {
    fn default() -> Self {
        Self::new()
    }
}

impl SpaceReverb {
    /// A space reverb with default params; its buffers are sized by `prepare`.
    pub fn new() -> Self {
        let mut core = Reverb::space();
        // The default decay of this device is longer than the classic reverb's.
        core.set_param(ParamId(3), 3.0);
        Self { core }
    }
}

impl Device for SpaceReverb {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.core.prepare(info);
    }

    fn reset(&mut self) {
        self.core.reset();
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        self.core.set_param(id, plain);
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        self.core.process(ctx);
    }

    fn has_meter_values(&self) -> bool {
        self.core.has_meter_values()
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        self.core.meter_values(out)
    }
}

#[cfg(test)]
mod tests;
