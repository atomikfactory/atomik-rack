//! Live input: the live note queue, `Engine::host_frames` and the
//! seqlock `FrameClock`.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{MAX_EVENTS_PER_BATCH, NoteKind};
use atomik_engine::{
    DeviceInstanceId, Engine, EngineError, EngineHandle, FrameClock, Graph, LIVE_RELEASE_HEADROOM,
    NOTE_QUEUE_CAPACITY, RELEASE_HEADROOM, ScheduledNote, new_engine,
};
use atomik_testkit::fakes::{ImpulseOnNote, NoteEvents, NoteLog};
use atomik_testkit::no_alloc::assert_no_alloc;
use common::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn on(key: u8) -> NoteKind {
    NoteKind::On { key, velocity: 1.0 }
}

fn note(frame: u64, device: DeviceInstanceId, kind: NoteKind) -> ScheduledNote {
    ScheduledNote {
        frame,
        device,
        socket: NoteLog::NOTES_IN,
        kind,
    }
}

/// A NoteLog (capacity 4096) on a plan; returns the engine, handle, log id and its events.
fn log_engine() -> (Engine, EngineHandle, DeviceInstanceId, NoteEvents) {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let (log, events) = NoteLog::boxed(4096);
    let id = g.add_device(log);
    let (engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    (engine, handle, id, events)
}

#[test]
fn live_notes_returns_a_sender_exactly_once() {
    let (_engine, mut handle) = new_engine(cfg(0, 1));
    let sender = handle.live_notes();
    assert!(sender.is_some());
    assert!(handle.live_notes().is_none());
    assert!(handle.live_notes().is_none());
    // The sender is `Send` (owned by the MIDI thread).
    let mut sender = sender.unwrap();
    std::thread::spawn(move || {
        sender.push(note(0, DeviceInstanceId(1), on(1))).unwrap();
    })
    .join()
    .unwrap();
}

#[test]
fn live_and_scheduled_notes_merge_in_frame_order() {
    // interleaved frames within one batch and across batches.
    let (mut engine, mut handle, log, events) = log_engine();
    let mut live = handle.live_notes().unwrap();
    for f in [10, 40, 70, 130] {
        handle.send_note(note(f, log, on(1))).unwrap();
    }
    for f in [5, 20, 64, 129] {
        live.push(note(f, log, on(2))).unwrap();
    }
    render_blocks(&mut engine, &[], 1, 256, 100);
    assert_eq!(
        events.events(),
        vec![
            (5, on(2)),
            (10, on(1)),
            (20, on(2)),
            (40, on(1)),
            (64, on(2)),
            (70, on(1)),
            (129, on(2)),
            (130, on(1)),
        ]
    );
    assert_eq!(handle.dropped_events(), 0);
}

#[test]
fn live_and_scheduled_ties_deliver_scheduled_first() {
    let (mut engine, mut handle, log, events) = log_engine();
    let mut live = handle.live_notes().unwrap();
    live.push(note(7, log, NoteKind::Off { key: 9 })).unwrap();
    handle.send_note(note(7, log, on(9))).unwrap();
    live.push(note(7, log, NoteKind::AllOff)).unwrap();
    render_blocks(&mut engine, &[], 1, 128, 128);
    assert_eq!(
        events.events(),
        vec![
            (7, on(9)),
            (7, NoteKind::Off { key: 9 }),
            (7, NoteKind::AllOff)
        ]
    );
}

#[test]
fn live_note_in_the_past_lands_at_offset_zero_and_before_later_scheduled() {
    let (mut engine, mut handle, log, events) = log_engine();
    let mut live = handle.live_notes().unwrap();
    render_blocks(&mut engine, &[], 1, 64, 64); // batch 0..64 processed
    // The next batch processed is 64..128: frame 3 is in the past.
    handle.send_note(note(100, log, on(1))).unwrap();
    live.push(note(3, log, on(2))).unwrap();
    render_blocks(&mut engine, &[], 1, 64, 64);
    assert_eq!(events.events(), vec![(64, on(2)), (100, on(1))]);
}

#[test]
fn live_note_reaches_an_impulse_at_its_frame() {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let imp = g.add_device(ImpulseOnNote::boxed());
    g.connect(ep(imp, ImpulseOnNote::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    let (mut engine, mut handle) = new_engine(c);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let mut live = handle.live_notes().unwrap();
    live.push(ScheduledNote {
        frame: 300,
        device: imp,
        socket: ImpulseOnNote::NOTES_IN,
        kind: on(1),
    })
    .unwrap();
    let out = render_blocks(&mut engine, &[], 1, 1000, 77);
    let hits: Vec<usize> = (0..1000).filter(|&i| out[0][i] != 0.0).collect();
    assert_eq!(hits, vec![300 + engine.latency_frames() as usize]);
}

#[test]
fn live_queue_overflow_is_dropped_and_counted() {
    let (mut engine, mut handle, log, events) = log_engine();
    let mut live = handle.live_notes().unwrap();
    for _ in 0..NOTE_QUEUE_CAPACITY - LIVE_RELEASE_HEADROOM {
        live.push(note(1_000_000, log, on(1))).unwrap();
    }
    for _ in 0..2 {
        assert_eq!(
            live.push(note(1_000_000, log, on(1))),
            Err(EngineError::QueueFull)
        );
    }
    assert_eq!(handle.dropped_events(), 2);
    render_blocks(&mut engine, &[], 1, 128, 128);
    assert!(events.events().is_empty(), "not due yet");
}

#[test]
fn live_queue_keeps_room_for_releases() {
    let (mut engine, mut handle, log, events) = log_engine();
    let mut live = handle.live_notes().unwrap();
    for _ in 0..NOTE_QUEUE_CAPACITY {
        let _ = live.push(note(1, log, on(1)));
    }
    // Notes are refused, but every release up to the headroom still fits.
    for key in 0..LIVE_RELEASE_HEADROOM as u8 {
        live.push(note(1, log, NoteKind::Off { key })).unwrap();
    }
    assert_eq!(
        live.push(note(1, log, NoteKind::AllOff)),
        Err(EngineError::QueueFull),
        "only the headroom is reserved"
    );
    render_blocks(&mut engine, &[], 1, 128, 128);
    let offs = events
        .events()
        .iter()
        .filter(|(_, k)| matches!(k, NoteKind::Off { .. }))
        .count();
    assert_eq!(offs, LIVE_RELEASE_HEADROOM);
}

#[test]
fn live_and_scheduled_batch_overflow_is_dropped_and_counted() {
    let (mut engine, mut handle, log, events) = log_engine();
    let mut live = handle.live_notes().unwrap();
    let half = MAX_EVENTS_PER_BATCH / 2 + 10;
    for _ in 0..half {
        handle.send_note(note(1, log, on(1))).unwrap();
        live.push(note(1, log, on(2))).unwrap();
    }
    render_blocks(&mut engine, &[], 1, 128, 128);
    // The last slots of the input are kept for releases.
    let room = MAX_EVENTS_PER_BATCH - RELEASE_HEADROOM;
    assert_eq!(events.events().len(), room);
    assert_eq!(handle.dropped_events(), (2 * half - room) as u64);
}

#[test]
fn a_dense_lane_and_chords_never_leave_a_hanging_note() {
    let (mut engine, mut handle, log, events) = log_engine();
    // A chord, then a flood of controller events in one batch, then the chord's releases.
    let chord = [60u8, 64, 67, 71];
    for key in chord {
        handle.send_note(note(1, log, on(key))).unwrap();
    }
    for i in 0..MAX_EVENTS_PER_BATCH * 2 {
        let kind = NoteKind::Cc {
            controller: 1,
            value: (i % 100) as f32 / 100.0,
        };
        handle.send_note(note(2, log, kind)).unwrap();
    }
    for key in chord {
        handle
            .send_note(note(3, log, NoteKind::Off { key }))
            .unwrap();
    }
    handle.send_note(note(3, log, NoteKind::AllOff)).unwrap();
    render_blocks(&mut engine, &[], 1, 128, 128);
    let seen = events.events();
    let count = |f: fn(&NoteKind) -> bool| seen.iter().filter(|(_, k)| f(k)).count();
    assert_eq!(count(|k| matches!(k, NoteKind::On { .. })), chord.len());
    assert_eq!(count(|k| matches!(k, NoteKind::Off { .. })), chord.len());
    assert_eq!(count(|k| matches!(k, NoteKind::AllOff)), 1);
    assert!(handle.dropped_events() > 0, "the controllers were dropped");
}

#[test]
fn live_notes_without_plan_are_dropped_and_counted() {
    let (mut engine, mut handle) = new_engine(cfg(0, 1));
    let mut live = handle.live_notes().unwrap();
    live.push(note(0, DeviceInstanceId(1), on(1))).unwrap();
    live.push(note(10_000, DeviceInstanceId(1), on(1))).unwrap();
    render_blocks(&mut engine, &[], 1, 128, 128);
    assert_eq!(handle.dropped_events(), 1);
}

#[test]
fn host_frames_counts_every_rendered_host_frame() {
    let (mut engine, _handle) = new_engine(cfg(1, 2));
    assert_eq!(engine.host_frames(), 0);
    let mut total = 0;
    for block in [0usize, 1, 7, 64, 65, 333, 0, 4096] {
        render_block(&mut engine, &[vec![0.0; block]], 2, block);
        total += block as u64;
        assert_eq!(engine.host_frames(), total);
    }
}

#[test]
fn host_frames_is_allocation_free() {
    let (engine, _handle) = new_engine(cfg(0, 2));
    assert_eq!(assert_no_alloc(|| engine.host_frames()), 0);
}

/// Renders a NoteLog patch where the same notes come either all scheduled or all live.
fn render_notes(live_path: bool, block: usize) -> Vec<(u64, NoteKind)> {
    let (mut engine, mut handle, log, events) = log_engine();
    let mut live = handle.live_notes().unwrap();
    let mut notes: Vec<ScheduledNote> = (0..200u64)
        .map(|k| {
            let kind = if k % 3 == 0 {
                NoteKind::Off { key: k as u8 }
            } else {
                on(k as u8)
            };
            note(k * 37 % 5000 + k, log, kind)
        })
        .collect();
    notes.sort_by_key(|n| n.frame);
    for n in notes {
        if live_path {
            live.push(n).unwrap();
        } else {
            handle.send_note(n).unwrap();
        }
    }
    render_blocks(&mut engine, &[], 1, 6000, block);
    events.events()
}

#[test]
fn live_notes_with_the_same_frames_match_scheduled_notes() {
    let scheduled = render_notes(false, 64);
    assert_eq!(scheduled.len(), 200);
    for block in [1, 64, 333] {
        assert_eq!(render_notes(true, block), scheduled, "block {block}");
    }
}

#[test]
fn live_sender_push_is_allocation_free() {
    let (_engine, mut handle) = new_engine(cfg(0, 1));
    let mut live = handle.live_notes().unwrap();
    assert_no_alloc(|| {
        for f in 0..100 {
            live.push(note(f, DeviceInstanceId(1), on(1))).unwrap();
        }
    });
}

// ---------------------------------------------------------------------------------------------
// FrameClock
// ---------------------------------------------------------------------------------------------

#[test]
fn frame_clock_read_is_none_before_first_publish() {
    let clock = FrameClock::new();
    assert_eq!(clock.read(), None);
    assert_eq!(FrameClock::default().read(), None);
}

#[test]
#[allow(clippy::disallowed_methods)] // Justification: test builds instants to round-trip.
fn frame_clock_round_trips_frame_and_instant() {
    let clock = FrameClock::new();
    let t0 = Instant::now();
    let later = t0 + Duration::from_nanos(123_456_789);
    clock.publish(4800, later);
    assert_eq!(clock.read(), Some((4800, later)));
    clock.publish(9600, t0);
    assert_eq!(clock.read(), Some((9600, t0)));
}

#[test]
#[allow(clippy::disallowed_methods)] // Justification: test builds instants to round-trip.
fn frame_clock_round_trips_instants_before_its_epoch() {
    let before = Instant::now();
    std::thread::sleep(Duration::from_millis(2));
    let clock = FrameClock::new();
    clock.publish(1, before);
    assert_eq!(clock.read(), Some((1, before)));
}

#[test]
#[allow(clippy::disallowed_methods)] // Justification: test builds instants to round-trip.
fn frame_clock_publish_and_read_are_allocation_free() {
    let clock = FrameClock::new();
    let t = Instant::now();
    let got = assert_no_alloc(|| {
        clock.publish(64, t);
        clock.read()
    });
    assert_eq!(got, Some((64, t)));
}

#[test]
#[allow(clippy::disallowed_methods)] // Justification: test builds instants to round-trip.
fn frame_clock_concurrent_reads_are_never_torn() {
    // The publisher writes pairs with a fixed relation; readers check it on every read.
    let clock = Arc::new(FrameClock::new());
    let base = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    let readers: Vec<_> = (0..3)
        .map(|_| {
            let (clock, stop) = (Arc::clone(&clock), Arc::clone(&stop));
            std::thread::spawn(move || {
                let (mut reads, mut last) = (0u64, 0u64);
                loop {
                    let done = stop.load(Ordering::Acquire);
                    if let Some((frame, at)) = clock.read() {
                        assert_eq!(
                            at.duration_since(base),
                            Duration::from_nanos(frame * 1_000 + 7),
                            "torn read at frame {frame}"
                        );
                        assert!(frame >= last, "went backwards");
                        last = frame;
                        reads += 1;
                    }
                    if done {
                        break;
                    }
                }
                reads
            })
        })
        .collect();
    for frame in 0..2_000_000u64 {
        clock.publish(frame, base + Duration::from_nanos(frame * 1_000 + 7));
    }
    stop.store(true, Ordering::Release);
    for r in readers {
        assert!(r.join().unwrap() > 0);
    }
    assert_eq!(clock.read().map(|(f, _)| f), Some(1_999_999));
}
