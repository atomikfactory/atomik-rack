//! `atomik.macro-bank`: eight macro knobs that drive params of other devices.
//!
//! The device makes no sound and the engine does nothing with its params: the macro values are
//! applied to the assigned params by the model ([`atomik-model`'s `macros`](../../model/src/macros.rs)),
//! which turns each assignment into a plain param value of the target before the engine sees
//! the project. The bank is a device so that its knobs are ordinary params: they are
//! automatable, undoable, visible in the rack's generated panel and MIDI-learnable like any
//! other.
//!
//! It has one Notes input that it ignores. That is what lets the song give the bank a track,
//! and the track its automation lanes, which is how an effect or a channel strip is automated
//! (a track exists only for a device that takes notes): assign the param to a macro and
//! automate the macro.
//!
//! Sockets: `notes_in` (Notes in). Params: `macro1` to `macro8` (ids 0 to 7, 0..1, default 0).

use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};

/// Type id of the macro bank.
pub const TYPE_ID: &str = "atomik.macro-bank";

static SOCKETS: [SocketDescriptor; 1] = [SocketDescriptor {
    id: SocketId(0),
    key: "notes_in",
    name: "Notes in",
    kind: SocketKind::Notes,
    direction: Direction::Input,
}];

const fn macro_param(id: u32, key: &'static str, name: &'static str) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    }
}

static PARAMS: [ParamDescriptor; 8] = [
    macro_param(0, "macro1", "Macro 1"),
    macro_param(1, "macro2", "Macro 2"),
    macro_param(2, "macro3", "Macro 3"),
    macro_param(3, "macro4", "Macro 4"),
    macro_param(4, "macro5", "Macro 5"),
    macro_param(5, "macro6", "Macro 6"),
    macro_param(6, "macro7", "Macro 7"),
    macro_param(7, "macro8", "Macro 8"),
];

/// Static description of the macro bank.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: TYPE_ID,
    name: "Macro Bank",
    kind: DeviceKind::Utility,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`MacroBank`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(MacroBank),
    }
}

/// The inert device: it holds no state and writes no output.
#[derive(Debug, Default, Clone, Copy)]
pub struct MacroBank;

impl Device for MacroBank {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, _info: &PrepareInfo) {}

    fn reset(&mut self) {}

    fn set_param(&mut self, _id: ParamId, _plain: f32) {}

    fn process(&mut self, _ctx: &mut ProcessContext<'_>) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::harness;

    #[test]
    fn descriptor_matches_the_contract() {
        assert_eq!(DESCRIPTOR.validate(), Ok(()));
        assert_eq!(DESCRIPTOR.type_id, "atomik.macro-bank");
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
        assert_eq!(DESCRIPTOR.sockets.len(), 1);
        assert_eq!(DESCRIPTOR.sockets[0].kind, SocketKind::Notes);
        assert_eq!(DESCRIPTOR.sockets[0].direction, Direction::Input);
        assert_eq!(DESCRIPTOR.params.len(), 8);
        for (i, p) in DESCRIPTOR.params.iter().enumerate() {
            assert_eq!(p.id, ParamId(i as u32));
            assert_eq!(p.key, format!("macro{}", i + 1));
            assert_eq!((p.min, p.max, p.default), (0.0, 1.0, 0.0));
            assert!(!p.discrete);
        }
    }

    #[test]
    fn the_device_is_inert() {
        let f = factory();
        let mut h = harness((f.create)());
        for p in DESCRIPTOR.params {
            h.set_param(p.key, 1.0);
        }
        h.process();
        assert_eq!(h.device().latency_frames(), 0);
    }
}
