//! The note tap: the notes a device receives are copied, with their host frames, into a queue
//! the app drains (Direct Record, Send to Track); it can be put on and taken off while the engine
//! runs and costs no allocation.

atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::NoteKind;
use atomik_device_sdk::MidiEvent;
use atomik_engine::{DeviceInstanceId, Graph, ScheduledNote};
use atomik_testkit::fakes::NoteLog;
use common::*;

fn sched(frame: u64, device: DeviceInstanceId, kind: NoteKind) -> ScheduledNote {
    ScheduledNote {
        frame,
        device,
        socket: NoteLog::NOTES_IN,
        kind,
    }
}

fn drain(tap: &atomik_device_sdk::MidiTap) -> Vec<MidiEvent> {
    std::iter::from_fn(|| tap.pop()).collect()
}

#[test]
fn a_tapped_device_reports_its_notes_with_host_frames_for_any_block_and_workers() {
    for block in [1, 64, 333] {
        for threads in [0, 2] {
            let c = cfg(0, 2);
            let mut g = Graph::new(&c);
            let (dev, _log) = NoteLog::boxed(64);
            let id = g.add_device(dev);
            let (mut engine, mut handle) = engine_with(c, threads, None);
            handle.submit_plan(g.compile().unwrap()).unwrap();
            let tap = handle.tap_notes(id).unwrap();
            let on = NoteKind::On {
                key: 64,
                velocity: 0.5,
            };
            handle.send_note(sched(5, id, on)).unwrap();
            handle
                .send_note(sched(200, id, NoteKind::Off { key: 64 }))
                .unwrap();
            handle
                .send_note(sched(201, id, NoteKind::PitchBend { value: 0.5 }))
                .unwrap();
            render_blocks(&mut engine, &[], 2, 512, block);
            let got: Vec<(u64, Vec<u8>)> = drain(&tap)
                .iter()
                .map(|e| (e.frame, e.bytes().to_vec()))
                .collect();
            assert_eq!(
                got,
                vec![(5, vec![0x90, 64, 64]), (200, vec![0x80, 64, 0])],
                "{block}/{threads}"
            );
        }
    }
}

#[test]
fn untapping_stops_the_copy_and_a_device_without_a_tap_costs_nothing() {
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let (dev, _log) = NoteLog::boxed(64);
    let id = g.add_device(dev);
    let (mut engine, mut handle) = engine_with(c, 0, None);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let tap = handle.tap_notes(id).unwrap();
    handle.untap_notes(id).unwrap();
    handle
        .send_note(sched(
            5,
            id,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        ))
        .unwrap();
    render_blocks(&mut engine, &[], 2, 256, 64);
    assert!(drain(&tap).is_empty());
    assert!(
        handle.collect_garbage() > 0,
        "the tap is retired, not dropped on the audio thread"
    );
}
