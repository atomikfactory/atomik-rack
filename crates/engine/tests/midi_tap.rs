//! A device's `MidiTap` is registered with the engine when the device is inserted or its plan is
//! submitted, and forgotten when it is removed or the plan replaced; the messages the real MIDI
//! Out device queues carry the engine host frame of the note that made them. Own binary: the
//! worker violation counter is process-global.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::NoteKind;
use atomik_device_sdk::{Device, MidiTap, SocketId};
use atomik_devices::midi_out::{self, MidiOut};
use atomik_engine::{DeviceInstanceId, Graph, ScheduledNote, new_engine_with_arena};
use common::*;
use std::sync::Arc;

const NOTES_IN: SocketId = SocketId(0);

fn id(n: u32) -> DeviceInstanceId {
    DeviceInstanceId(n)
}

fn note(frame: u64, device: DeviceInstanceId, kind: NoteKind) -> ScheduledNote {
    ScheduledNote {
        frame,
        device,
        socket: NOTES_IN,
        kind,
    }
}

fn drain(tap: &MidiTap) -> Vec<(u64, Vec<u8>)> {
    std::iter::from_fn(|| tap.pop())
        .map(|e| (e.frame, e.bytes().to_vec()))
        .collect()
}

#[test]
fn a_plan_registers_its_midi_taps_and_replaces_the_old_ones() {
    let (mut engine, mut handle) = new_engine_with_arena(cfg(0, 2), Default::default(), 8);
    let mut g = Graph::new(&cfg(0, 2));
    let device = MidiOut::new();
    let own = device.midi_tap().unwrap();
    let k = g.add_device(Box::new(device));
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let taps = handle.device_midi_taps();
    assert_eq!(taps.len(), 1);
    assert_eq!(taps[0].0, k);
    assert!(Arc::ptr_eq(&taps[0].1, &own), "the device's own tap");
    // The note reaches the device at its engine host frame.
    handle
        .send_note(note(
            200,
            k,
            NoteKind::On {
                key: 64,
                velocity: 1.0,
            },
        ))
        .unwrap();
    handle
        .send_note(note(300, k, NoteKind::Off { key: 64 }))
        .unwrap();
    render_blocks(&mut engine, &[], 2, 6 * 64, 64);
    // The first batch also sent the (absent) program: nothing.
    assert_eq!(
        drain(&own),
        vec![(200, vec![0x90, 64, 127]), (300, vec![0x80, 64, 0])]
    );
    // A plan without it forgets the tap.
    handle
        .submit_plan(Graph::new(&cfg(0, 2)).compile().unwrap())
        .unwrap();
    assert!(handle.device_midi_taps().is_empty());
}

#[test]
fn an_inserted_device_registers_and_a_removed_one_leaves() {
    let (mut engine, mut handle) = new_engine_with_arena(cfg(0, 2), Default::default(), 8);
    let device = MidiOut::new();
    let own = device.midi_tap().unwrap();
    handle.insert_device(id(1), Box::new(device), &[]).unwrap();
    assert!(Arc::ptr_eq(&handle.meters().midi_taps()[0].1, &own));
    // Not routed to anything: a device with no audio still runs once it is in a plan.
    let mut t = atomik_engine::Topology::new(&cfg(0, 2));
    t.add_node(id(1), &midi_out::DESCRIPTOR).unwrap();
    handle.submit_routing(t.compile().unwrap()).unwrap();
    handle
        .send_note(note(
            10,
            id(1),
            NoteKind::On {
                key: 60,
                velocity: 0.5,
            },
        ))
        .unwrap();
    render_blocks(&mut engine, &[], 2, 2 * 64, 64);
    assert_eq!(drain(&own), vec![(10, vec![0x90, 60, 64])]);
    handle
        .submit_routing(atomik_engine::Topology::new(&cfg(0, 2)).compile().unwrap())
        .unwrap();
    handle.remove_device(id(1)).unwrap();
    assert!(handle.meters().midi_taps().is_empty());
}
