//! `atomik.container`: a device whose inside is another rack.
//!
//! The container has no DSP of its own. The model flattens it into its inner devices before the
//! engine sees it ([`atomik-model`'s `expand`]), so the engine never instantiates one; the
//! factory below builds a harmless pass-through only so that a descriptor-only user (patch files,
//! registry tests) never crashes. The render patch builder rejects the type with a clear message.
//!
//! Sockets: `in_l`, `in_r` (Audio in), `notes_in` (Notes in), `out_l`, `out_r` (Audio out).
//! Params: `macro1` to `macro8` (ids 0 to 7, 0..1, default 0) and `bypass` (id 8, a 0/1
//! toggle). The macro values are applied to inner params outside the engine, and the model's
//! expansion consumes `bypass`: while it is on the inside is left out and `in_l`, `in_r` go
//! straight to `out_l`, `out_r`, so the engine never sees the switch.

use atomik_core::{BATCH, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceFactory, DeviceKind, Direction, PrepareInfo, ProcessContext,
    SocketDescriptor, SocketId, SocketKind,
};

/// Type id of the container device.
pub const TYPE_ID: &str = "atomik.container";

const fn sock(
    id: u16,
    key: &'static str,
    name: &'static str,
    kind: SocketKind,
    direction: Direction,
) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name,
        kind,
        direction,
    }
}

static SOCKETS: [SocketDescriptor; 5] = [
    sock(0, "in_l", "In L", SocketKind::Audio, Direction::Input),
    sock(1, "in_r", "In R", SocketKind::Audio, Direction::Input),
    sock(
        2,
        "notes_in",
        "Notes in",
        SocketKind::Notes,
        Direction::Input,
    ),
    sock(3, "out_l", "Out L", SocketKind::Audio, Direction::Output),
    sock(4, "out_r", "Out R", SocketKind::Audio, Direction::Output),
];

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

/// Key of the bypass param (the model's expansion reads it by this name).
pub const BYPASS_KEY: &str = "bypass";

static PARAMS: [ParamDescriptor; 9] = [
    macro_param(0, "macro1", "Macro 1"),
    macro_param(1, "macro2", "Macro 2"),
    macro_param(2, "macro3", "Macro 3"),
    macro_param(3, "macro4", "Macro 4"),
    macro_param(4, "macro5", "Macro 5"),
    macro_param(5, "macro6", "Macro 6"),
    macro_param(6, "macro7", "Macro 7"),
    macro_param(7, "macro8", "Macro 8"),
    ParamDescriptor {
        id: ParamId(8),
        key: BYPASS_KEY,
        name: "Bypass",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        taper: Taper::Linear,
        unit: Unit::Toggle,
        discrete: true,
    },
];

/// Static description of the container.
pub static DESCRIPTOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: TYPE_ID,
    name: "Container",
    kind: DeviceKind::Container,
    version: 1,
    params: &PARAMS,
    sockets: &SOCKETS,
};

/// Factory registering [`Container`].
pub fn factory() -> DeviceFactory {
    DeviceFactory {
        descriptor: &DESCRIPTOR,
        create: || Box::new(Container),
    }
}

/// The pass-through stand-in (never instantiated by the engine): `in_l` to `out_l`,
/// `in_r` to `out_r`, notes ignored.
#[derive(Debug, Default, Clone, Copy)]
pub struct Container;

impl Device for Container {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DESCRIPTOR
    }

    fn prepare(&mut self, _info: &PrepareInfo) {}

    fn reset(&mut self) {}

    fn set_param(&mut self, _id: ParamId, _plain: f32) {}

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let l: [f32; BATCH] = *ctx.input(SocketId(0));
        let r: [f32; BATCH] = *ctx.input(SocketId(1));
        *ctx.output(SocketId(3)) = l;
        *ctx.output(SocketId(4)) = r;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::harness;

    #[test]
    fn descriptor_matches_the_contract() {
        assert_eq!(DESCRIPTOR.validate(), Ok(()));
        assert_eq!(DESCRIPTOR.type_id, "atomik.container");
        assert_eq!(DESCRIPTOR.kind, DeviceKind::Container);
        let keys: Vec<_> = DESCRIPTOR
            .sockets
            .iter()
            .map(|s| (s.key, s.kind, s.direction))
            .collect();
        assert_eq!(
            keys,
            [
                ("in_l", SocketKind::Audio, Direction::Input),
                ("in_r", SocketKind::Audio, Direction::Input),
                ("notes_in", SocketKind::Notes, Direction::Input),
                ("out_l", SocketKind::Audio, Direction::Output),
                ("out_r", SocketKind::Audio, Direction::Output),
            ]
        );
        assert_eq!(DESCRIPTOR.params.len(), 9);
        for (i, p) in DESCRIPTOR.params[..8].iter().enumerate() {
            assert_eq!(p.id, ParamId(i as u32));
            assert_eq!(p.key, format!("macro{}", i + 1));
            assert_eq!((p.min, p.max, p.default), (0.0, 1.0, 0.0));
            assert_eq!(p.unit, Unit::None);
            assert!(!p.discrete);
        }
        let bypass = &DESCRIPTOR.params[8];
        assert_eq!((bypass.id, bypass.key), (ParamId(8), BYPASS_KEY));
        assert_eq!((bypass.min, bypass.max, bypass.default), (0.0, 1.0, 0.0));
        assert_eq!(bypass.unit, Unit::Toggle);
        assert!(bypass.discrete);
    }

    #[test]
    fn factory_builds_a_pass_through() {
        let f = factory();
        let mut h = harness((f.create)());
        for p in DESCRIPTOR.params {
            h.set_param(p.key, 1.0);
        }
        h.input_mut("in_l")
            .iter_mut()
            .enumerate()
            .for_each(|(i, x)| *x = i as f32);
        *h.input_mut("in_r") = [0.5; BATCH];
        h.process();
        assert_eq!(h.output("out_l")[7].to_bits(), 7.0f32.to_bits());
        assert_eq!(h.output("out_r")[0].to_bits(), 0.5f32.to_bits());
        assert_eq!(h.device().latency_frames(), 0);
    }
}
