//! Queue and output stage tests.

use super::*;
use atomik_core::MAX_EVENTS_PER_BATCH;

fn buf() -> NoteBuffer {
    NoteBuffer::with_capacity(MAX_EVENTS_PER_BATCH)
}

fn on(key: u8) -> NoteKind {
    NoteKind::On { key, velocity: 0.5 }
}

fn off(key: u8) -> NoteKind {
    NoteKind::Off { key }
}

fn drain_all(q: &mut NoteQueue) -> Vec<(u64, NoteKind)> {
    std::iter::from_fn(|| q.pop_due(u64::MAX)).collect()
}

#[test]
fn queue_pops_by_frame_then_insertion_order() {
    let mut q = NoteQueue::new();
    assert!(q.push_other(100, on(1)));
    assert!(q.push_other(50, on(2)));
    assert!(q.push_other(100, on(3)));
    assert!(q.push_other(50, on(4)));
    assert!(q.push_other(75, on(5)));
    let keys: Vec<_> = drain_all(&mut q)
        .iter()
        .map(|(f, k)| (*f, k.key().unwrap()))
        .collect();
    assert_eq!(keys, [(50, 2), (50, 4), (75, 5), (100, 1), (100, 3)]);
    assert!(q.is_empty());
}

#[test]
fn pop_due_leaves_entries_at_or_after_the_end() {
    let mut q = NoteQueue::new();
    q.push_other(10, on(1));
    q.push_other(64, on(2));
    assert_eq!(q.pop_due(64).map(|e| e.0), Some(10));
    assert_eq!(q.pop_due(64), None);
    assert_eq!(q.pop_due(65).map(|e| e.0), Some(64));
}

#[test]
fn pair_needs_room_for_both_and_a_full_queue_refuses() {
    let mut q = NoteQueue::new();
    for i in 0..(QUEUE_CAP / 2 - 1) {
        assert!(q.push_pair(i as u64, i as u64 + 5, 60, 0.5), "pair {i}");
    }
    // Two slots left: one more pair, then nothing.
    assert_eq!(q.free(), 2);
    assert!(q.push_pair(1_000, 1_001, 60, 0.5));
    assert!(!q.push_pair(2_000, 2_001, 60, 0.5));
    assert_eq!(q.len(), QUEUE_CAP);
    assert!(!q.push_other(3_000, on(1)));
}

#[test]
fn reserved_off_fits_when_the_queue_is_otherwise_full() {
    let mut q = NoteQueue::new();
    assert!(q.push_on_reserving(10, 60, 0.5));
    // Fill every other slot with unreserved entries.
    while q.push_other(20, NoteKind::PitchBend { value: 0.0 }) {}
    assert_eq!(q.free(), 0);
    assert!(!q.push_on_reserving(11, 61, 0.5), "no room for its Off");
    q.push_off_reserved(30, 60);
    assert_eq!(q.len(), QUEUE_CAP);
    let last = drain_all(&mut q).pop().unwrap();
    assert_eq!(last, (30, off(60)));
}

#[test]
fn pair_off_never_precedes_its_on() {
    let mut q = NoteQueue::new();
    q.push_pair(100, 40, 60, 0.5);
    let frames: Vec<_> = drain_all(&mut q).iter().map(|e| e.0).collect();
    assert_eq!(frames, [100, 100]);
}

#[test]
fn clear_forgets_entries_and_reservations() {
    let mut q = NoteQueue::new();
    q.push_on_reserving(1, 60, 0.5);
    q.push_pair(2, 3, 61, 0.5);
    q.clear();
    assert!(q.is_empty());
    assert_eq!(q.free(), QUEUE_CAP);
}

#[test]
fn round_frames_rounds_half_up_and_never_goes_negative() {
    assert_eq!(round_frames(2.5), 3);
    assert_eq!(round_frames(2.49), 2);
    assert_eq!(round_frames(-3.0), 0);
    assert_eq!(round_frames(f64::NAN), 0);
}

#[test]
fn sink_counts_users_and_closes_a_key_on_its_last_off() {
    let mut s = NoteSink::new();
    let mut o = buf();
    s.emit(&mut o, 0, on(60));
    s.emit(&mut o, 5, on(60));
    s.emit(&mut o, 6, off(60));
    s.emit(&mut o, 9, off(60));
    // On, retrigger (Off then On), then one Off when the count returns to zero.
    assert_eq!(o.len(), 4);
    assert!(matches!(o.as_slice()[1].kind, NoteKind::Off { .. }));
    assert!(matches!(o.as_slice()[2].kind, NoteKind::On { .. }));
    assert_eq!(o.as_slice()[3].offset, 9);
    assert!(!s.any_sounding());
}

#[test]
fn sink_swallows_the_off_of_an_on_the_buffer_refused() {
    let mut s = NoteSink::new();
    let mut o = NoteBuffer::with_capacity(ON_HEADROOM - 1);
    s.emit(&mut o, 0, on(60)); // refused: less than the headroom free
    assert_eq!(o.len(), 0);
    s.emit(&mut o, 1, off(60));
    assert_eq!(o.len(), 0, "no stray Off");
    assert!(!s.any_sounding());
}

#[test]
fn sink_close_all_is_ascending_and_forgets() {
    let mut s = NoteSink::new();
    let mut o = buf();
    s.emit(&mut o, 0, on(70));
    s.emit(&mut o, 0, on(60));
    o.clear();
    s.close_all(&mut o, 3);
    let keys: Vec<_> = o.as_slice().iter().map(|e| e.kind.key().unwrap()).collect();
    assert_eq!(keys, [60, 70]);
    assert!(!s.any_sounding());
}

#[test]
fn sink_drain_writes_due_entries_at_their_batch_offsets() {
    let mut q = NoteQueue::new();
    q.push_pair(130, 190, 60, 0.5);
    q.push_pair(300, 310, 61, 0.5);
    let mut s = NoteSink::new();
    let mut o = buf();
    s.drain(&mut q, &mut o, 128);
    let seen: Vec<_> = o.as_slice().iter().map(|e| e.offset).collect();
    assert_eq!(seen, [2, 62]);
    o.clear();
    s.drain(&mut q, &mut o, 192);
    assert_eq!(o.len(), 0);
    assert_eq!(q.len(), 2);
}
