//! Helpers for hot-edit tests: a [`Rig`] keeps a [`Topology`] in step with a list of
//! scripted [`TimedEdit`]s, the way the app keeps one in step with the engine.

use super::ep;
use atomik_core::{NoteKind, ParamId};
use atomik_device_sdk::DeviceFactory;
use atomik_engine::{
    DeviceInstanceId, Endpoint, EngineConfig, Graph, HotEdit, ScheduledNote, TimedEdit, Topology,
};

/// A topology plus the edit script that builds and changes it.
pub struct Rig {
    pub topo: Topology,
    pub edits: Vec<TimedEdit>,
}

impl Rig {
    pub fn new(c: &EngineConfig) -> Self {
        Self {
            topo: Topology::new(c),
            edits: Vec::new(),
        }
    }

    /// Endpoint of `key` on `device` (`Graph::INTERFACE` for interface sockets).
    pub fn sock(&self, device: DeviceInstanceId, key: &str) -> Endpoint {
        let d = self.topo.descriptor(device).expect("known device");
        ep(device, d.socket_by_key(key).expect("known socket").id)
    }

    /// Schedules an insert of a fresh `factory` device with keyed params, and adds the node.
    pub fn insert(
        &mut self,
        frame: u64,
        id: u32,
        factory: DeviceFactory,
        params: &[(&str, f32)],
    ) -> DeviceInstanceId {
        let id = DeviceInstanceId(id);
        let params = params
            .iter()
            .map(|(k, v)| (param_id(factory, k), *v))
            .collect();
        self.edits.push(TimedEdit {
            frame,
            edit: HotEdit::Insert {
                id,
                device: (factory.create)(),
                params,
            },
        });
        self.topo.add_node(id, factory.descriptor).unwrap();
        id
    }

    /// Removes the node (and its cables) from the topology, then schedules the routing without
    /// it followed by the removal: the order the model's `SyncOp`s use.
    pub fn unroute_and_remove(&mut self, frame: u64, id: DeviceInstanceId) {
        self.topo.remove_node(id).unwrap();
        self.route(frame);
        self.edits.push(TimedEdit {
            frame,
            edit: HotEdit::Remove(id),
        });
    }

    /// Connects `from_l`/`from_r` to `to_l`/`to_r` (`prefix` + `l`/`r`). For the interface,
    /// pass `"out_1"`-style keys through [`Rig::connect`] instead.
    pub fn stereo(&mut self, from: DeviceInstanceId, out: &str, to: DeviceInstanceId, inp: &str) {
        for side in ["l", "r"] {
            let (f, t) = (
                self.sock(from, &format!("{out}{side}")),
                self.sock(to, &format!("{inp}{side}")),
            );
            self.topo.connect(f, t).unwrap();
        }
    }

    /// Connects one socket pair by key.
    pub fn connect(&mut self, from: (DeviceInstanceId, &str), to: (DeviceInstanceId, &str)) {
        let (f, t) = (self.sock(from.0, from.1), self.sock(to.0, to.1));
        self.topo.connect(f, t).unwrap();
    }

    /// Disconnects the input by key.
    pub fn disconnect(&mut self, to: (DeviceInstanceId, &str)) {
        let t = self.sock(to.0, to.1);
        self.topo.disconnect(t).unwrap();
    }

    /// `out_l`/`out_r` of `device` to interface `out_a`/`out_b`.
    pub fn wire_outputs(&mut self, device: DeviceInstanceId, a: &str, b: &str) {
        self.connect((device, "out_l"), (Graph::INTERFACE, a));
        self.connect((device, "out_r"), (Graph::INTERFACE, b));
    }

    /// Schedules the current topology as a routing at `frame`.
    pub fn route(&mut self, frame: u64) {
        self.edits.push(TimedEdit {
            frame,
            edit: HotEdit::Routing(self.topo.compile().unwrap()),
        });
    }

    /// Schedules a keyed param change.
    pub fn param(&mut self, frame: u64, device: DeviceInstanceId, key: &str, plain: f32) {
        let p = self
            .topo
            .descriptor(device)
            .unwrap()
            .param_by_key(key)
            .unwrap()
            .id;
        self.edits.push(TimedEdit {
            frame,
            edit: HotEdit::Param {
                device,
                param: p,
                plain,
            },
        });
    }

    /// Schedules a tempo change.
    pub fn tempo(&mut self, frame: u64, bpm: f64) {
        self.edits.push(TimedEdit {
            frame,
            edit: HotEdit::Tempo(atomik_core::Tempo::new(bpm).unwrap()),
        });
    }
}

pub fn param_id(factory: DeviceFactory, key: &str) -> ParamId {
    factory.descriptor.param_by_key(key).unwrap().id
}

/// A note On (`velocity` 0.8) or Off for a synth's `notes_in`.
pub fn note(frame: u64, device: DeviceInstanceId, on: bool, key: u8) -> ScheduledNote {
    let socket = atomik_devices::poly_synth::factory()
        .descriptor
        .socket_by_key("notes_in")
        .unwrap()
        .id;
    ScheduledNote {
        frame,
        device,
        socket,
        kind: if on {
            NoteKind::On { key, velocity: 0.8 }
        } else {
            NoteKind::Off { key }
        },
    }
}

/// Sum of absolute values over a range of one channel.
pub fn energy(channel: &[f32], range: std::ops::Range<usize>) -> f64 {
    channel[range].iter().map(|s| f64::from(s.abs())).sum()
}
