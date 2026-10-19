//! QA gap tests for core.
//!
//! Says "exactly 24,000"; the unit test compares with a tolerance. is checked at
//! the capacity the engine actually uses.

use atomik_core::{
    BufferFull, MAX_EVENTS_PER_BATCH, NoteBuffer, NoteEvent, NoteKind, SampleRate, Tempo,
    beats_to_frames,
};

#[test]
fn qa_ac_c4_one_beat_at_120_bpm_48k_is_exactly_24000_frames() {
    let f = beats_to_frames(
        1.0,
        Tempo::new(120.0).unwrap(),
        SampleRate::new(48_000.0).unwrap(),
    );
    assert_eq!(f.to_bits(), 24_000.0f64.to_bits(), "{f}");
}

#[test]
fn qa_ac_c3_full_buffer_at_engine_capacity_rejects_and_keeps_length() {
    let ev = |offset: u16| NoteEvent {
        offset,
        kind: NoteKind::Off { key: 1 },
    };
    let mut b = NoteBuffer::with_capacity(MAX_EVENTS_PER_BATCH);
    for i in 0..MAX_EVENTS_PER_BATCH {
        assert_eq!(b.push(ev((i * 64 / MAX_EVENTS_PER_BATCH) as u16)), Ok(()));
    }
    assert_eq!(b.push(ev(63)), Err(BufferFull));
    assert_eq!(b.len(), MAX_EVENTS_PER_BATCH);
    assert_eq!(
        b.as_slice().last(),
        Some(&ev(
            ((MAX_EVENTS_PER_BATCH - 1) * 64 / MAX_EVENTS_PER_BATCH) as u16
        ))
    );
}
