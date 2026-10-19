//! Sample-accurate note delivery (and note routing).
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{MAX_EVENTS_PER_BATCH, NoteKind};
use atomik_device_sdk::SocketId;
use atomik_engine::{DeviceInstanceId, Engine, EngineHandle, Graph, ScheduledNote, new_engine};
use atomik_testkit::fakes::{CallLog, ImpulseOnNote, NoteThru};
use common::*;
use proptest::prelude::*;

const ON: NoteKind = NoteKind::On {
    key: 60,
    velocity: 1.0,
};

fn note(frame: u64, device: DeviceInstanceId, socket: SocketId) -> ScheduledNote {
    ScheduledNote {
        frame,
        device,
        socket,
        kind: ON,
    }
}

/// ImpulseOnNote -> out_1. Returns the engine, its handle and the impulse device id.
fn impulse_engine() -> (Engine, EngineHandle, DeviceInstanceId) {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let imp = g.add_device(ImpulseOnNote::boxed());
    g.connect(ep(imp, ImpulseOnNote::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    let (engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    (engine, handle, imp)
}

fn impulses(x: &[f32]) -> Vec<usize> {
    x.iter()
        .enumerate()
        .filter(|(_, s)| s.abs() > 0.0)
        .map(|(i, _)| i)
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]
    /// (engine level): a note at frame f is the impulse at timeline frame f, for any block.
    #[test]
    fn render_note_at_frame_lands_at_frame(frame in 0u64..5000, block in 1usize..600) {
        let (mut engine, mut handle, imp) = impulse_engine();
        handle.send_note(note(frame, imp, ImpulseOnNote::NOTES_IN)).unwrap();
        let latency = engine.latency_frames() as usize;
        let total = 5000 + latency;
        let out = render_blocks(&mut engine, &[], 1, total, block);
        let hits = impulses(&out[0][latency..]);
        prop_assert_eq!(hits, vec![frame as usize]);
        prop_assert!((out[0][latency + frame as usize] - 1.0).abs() < 1e-7);
        prop_assert_eq!(handle.dropped_events(), 0);
    }
}

#[test]
fn render_note_in_past_lands_at_offset_zero() {
    let (mut engine, mut handle, imp) = impulse_engine();
    render_blocks(&mut engine, &[], 1, 128, 128);
    // Frame 10 is already rendered: it lands at the start of the next batch (frame 128).
    handle
        .send_note(note(10, imp, ImpulseOnNote::NOTES_IN))
        .unwrap();
    let out = render_blocks(&mut engine, &[], 1, 256, 50);
    // Host frames 128..384 were rendered; the FIFO delays by 64.
    assert_eq!(impulses(&out[0]), vec![128 + 64 - 128]);
}

#[test]
fn render_notes_merge_cable_and_scheduled_in_offset_order() {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let thru = g.add_device(NoteThru::boxed());
    let imp = g.add_device(ImpulseOnNote::boxed());
    g.connect(
        ep(thru, NoteThru::NOTES_OUT),
        ep(imp, ImpulseOnNote::NOTES_IN),
    )
    .unwrap();
    g.connect(ep(imp, ImpulseOnNote::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    let (mut engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    // Same batch: direct notes at 3 and 9, one through the cable at 5; next batch: cable at 70.
    handle
        .send_note(note(3, imp, ImpulseOnNote::NOTES_IN))
        .unwrap();
    handle.send_note(note(5, thru, NoteThru::NOTES_IN)).unwrap();
    handle
        .send_note(note(9, imp, ImpulseOnNote::NOTES_IN))
        .unwrap();
    handle
        .send_note(note(70, thru, NoteThru::NOTES_IN))
        .unwrap();
    let out = render_blocks(&mut engine, &[], 1, 256, 256);
    assert_eq!(impulses(&out[0][64..]), vec![3, 5, 9, 70]);
    assert_eq!(handle.dropped_events(), 0);
}

#[test]
fn render_note_to_invalid_target_is_dropped_and_counted() {
    let (mut engine, mut handle, imp) = impulse_engine();
    handle
        .send_note(note(0, DeviceInstanceId(99), SocketId(0)))
        .unwrap();
    handle
        .send_note(note(1, imp, ImpulseOnNote::AUDIO_OUT))
        .unwrap();
    handle.send_note(note(2, imp, SocketId(42))).unwrap();
    handle
        .send_note(note(3, Graph::INTERFACE, SocketId(0)))
        .unwrap();
    handle
        .send_note(note(4, imp, ImpulseOnNote::NOTES_IN))
        .unwrap();
    let out = render_blocks(&mut engine, &[], 1, 128, 128);
    assert_eq!(impulses(&out[0][64..]), vec![4]);
    assert_eq!(handle.dropped_events(), 4);
}

#[test]
fn render_notes_without_plan_are_dropped_and_counted() {
    let (mut engine, mut handle) = new_engine(cfg(0, 1));
    handle
        .send_note(note(0, DeviceInstanceId(1), SocketId(0)))
        .unwrap();
    handle
        .send_note(note(500, DeviceInstanceId(1), SocketId(0)))
        .unwrap();
    render_blocks(&mut engine, &[], 1, 128, 128);
    assert_eq!(
        handle.dropped_events(),
        1,
        "only notes due so far are consumed"
    );
}

#[test]
fn render_event_overflow_drops_and_counts() {
    // more than MAX_EVENTS_PER_BATCH notes in one batch.
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let (log, counts) = CallLog::boxed();
    let l = g.add_device(log);
    let (mut engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let extra = 77;
    for i in 0..MAX_EVENTS_PER_BATCH + extra {
        handle
            .send_note(note((i % 64) as u64, l, CallLog::NOTES_IN))
            .unwrap();
    }
    handle.send_note(note(64, l, CallLog::NOTES_IN)).unwrap();
    render_blocks(&mut engine, &[], 1, 128, 128);
    assert_eq!(handle.dropped_events(), extra as u64);
    assert_eq!(counts.notes_seen(), MAX_EVENTS_PER_BATCH as u64 + 1);
}
