//! The record tap: a lock-free path that copies selected host input
//! channels out of the audio callback, tagged with the engine frame of every batch, for a
//! non-RT writer thread (the app writes WAV files from it).
//!
//! # Design
//!
//! * `EngineHandle::record_tap(channels, capacity_frames)` (non-RT) allocates an SPSC ring of
//!   `u32` words and queues a [`TapRt`] through the ordered control queue, so the tap starts at
//!   a batch boundary; `EngineHandle::stop_record_tap` queues its removal the same way (the
//!   engine hands the old tap back instead of freeing it). One tap at a time: a new one
//!   replaces the old.
//! * At every batch the audio thread appends one *record* with a single wait-free commit:
//!   `[engine_frame as 2 words (low, high), then 64 words (f32 bits) per tapped channel]`.
//!   `engine_frame` is `Engine`'s batch frame (the timeline of `TransportState::engine_frame`
//!   and `SongSnapshot::position_at`), so the first frame of the record is host input frame
//!   `engine_frame` of the stream. Channels beyond the engine's input count read as silence.
//! * If the ring has no room for a whole record the batch is dropped and counted
//!   ([`RecordTap::overflow_frames`], in frames); nothing blocks or allocates.
//! * [`RecordTap::drain`] copies out whole records that are *contiguous* in engine frames, so
//!   the returned start tag is exact; a gap (overflow) ends the run, and the next call starts
//!   at the first frame after the gap.

use atomik_core::{BATCH, Batch};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The audio-thread half of a tap (travels through the control queue).
pub(crate) struct TapRt {
    tx: Producer<u32>,
    channels: Vec<usize>,
    record: Vec<u32>,
    overflow: Arc<AtomicU64>,
}

impl TapRt {
    /// Appends the batch starting at engine frame `frame`. Never allocates or blocks.
    pub(crate) fn capture(&mut self, frame: u64, inputs: &[Batch]) {
        if self.tx.slots() < self.record.len() {
            self.overflow.fetch_add(BATCH as u64, Ordering::Relaxed);
            return;
        }
        self.record[0] = frame as u32;
        self.record[1] = (frame >> 32) as u32;
        for (k, &ch) in self.channels.iter().enumerate() {
            let dst = &mut self.record[2 + k * BATCH..2 + (k + 1) * BATCH];
            match inputs.get(ch) {
                Some(src) => {
                    for (d, s) in dst.iter_mut().zip(src) {
                        *d = s.to_bits();
                    }
                }
                None => dst.fill(0.0f32.to_bits()),
            }
        }
        if self.tx.push_entire_slice(&self.record).is_err() {
            self.overflow.fetch_add(BATCH as u64, Ordering::Relaxed);
        }
    }
}

/// The non-RT consumer of a record tap.
pub struct RecordTap {
    rx: Consumer<u32>,
    channels: usize,
    overflow: Arc<AtomicU64>,
    record: Vec<u32>,
}

impl std::fmt::Debug for RecordTap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordTap")
            .field("channels", &self.channels)
            .field("overflow_frames", &self.overflow_frames())
            .finish_non_exhaustive()
    }
}

/// Creates both halves: the tap sized for `capacity_frames` (rounded up to whole batches, at
/// least one) of `channels`.
pub(crate) fn new_tap(channels: &[usize], capacity_frames: usize) -> (Box<TapRt>, RecordTap) {
    let words = 2 + channels.len() * BATCH;
    let records = capacity_frames.div_ceil(BATCH).max(1);
    let (tx, rx) = RingBuffer::new(records * words);
    let overflow = Arc::new(AtomicU64::new(0));
    let rt = TapRt {
        tx,
        channels: channels.to_vec(),
        record: vec![0; words],
        overflow: Arc::clone(&overflow),
    };
    let tap = RecordTap {
        rx,
        channels: channels.len(),
        overflow,
        record: vec![0; words],
    };
    (Box::new(rt), tap)
}

impl RecordTap {
    /// Number of tapped channels (the length `out` gets in [`RecordTap::drain`]).
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames dropped so far because the ring was full (a multiple of 64).
    pub fn overflow_frames(&self) -> u64 {
        self.overflow.load(Ordering::Relaxed)
    }

    /// Appends the available input frames to `out` (one `Vec` per tapped channel, in the order
    /// given to `record_tap`; `out` is grown to that many vectors if shorter) and returns
    /// `(first engine frame, frames appended)`, or `None` if nothing was available.
    ///
    /// Only frames that are contiguous in engine time are appended in one call, so the first
    /// returned frame number applies to every appended frame; after an overflow the rest is
    /// returned by the next call, with its own start frame.
    pub fn drain(&mut self, out: &mut Vec<Vec<f32>>) -> Option<(u64, usize)> {
        let words = self.record.len();
        if out.len() < self.channels {
            out.resize_with(self.channels, Vec::new);
        }
        let mut run: Option<(u64, usize)> = None;
        while self.rx.slots() >= words {
            let Ok(chunk) = self.rx.read_chunk(words) else {
                break;
            };
            let (a, b) = chunk.as_slices();
            self.record[..a.len()].copy_from_slice(a);
            self.record[a.len()..].copy_from_slice(b);
            let frame = u64::from(self.record[0]) | u64::from(self.record[1]) << 32;
            if let Some((start, n)) = run
                && frame != start + n as u64
            {
                break; // a gap: leave this record for the next call
            }
            for (k, dst) in out.iter_mut().take(self.channels).enumerate() {
                let src = &self.record[2 + k * BATCH..2 + (k + 1) * BATCH];
                dst.extend(src.iter().map(|w| f32::from_bits(*w)));
            }
            chunk.commit_all();
            run = Some(match run {
                Some((start, n)) => (start, n + BATCH),
                None => (frame, BATCH),
            });
        }
        run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(v: f32) -> Batch {
        [v; BATCH]
    }

    #[test]
    fn drain_returns_contiguous_runs_with_exact_tags_and_counts_overflow() {
        let (mut rt, mut tap) = new_tap(&[1, 0], 3 * BATCH);
        let inputs = [batch(0.25), batch(-0.5)];
        for k in 0..5u64 {
            rt.capture(k * BATCH as u64, &inputs);
        }
        // Room for three batches: two were dropped.
        assert_eq!(tap.overflow_frames(), 2 * BATCH as u64);
        let mut out = Vec::new();
        assert_eq!(tap.drain(&mut out), Some((0, 3 * BATCH)));
        assert_eq!(out.len(), 2);
        assert!(
            out[0].iter().all(|s| s.to_bits() == (-0.5f32).to_bits()),
            "channel 1 first"
        );
        assert!(out[1].iter().all(|s| s.to_bits() == 0.25f32.to_bits()));
        assert_eq!(tap.drain(&mut out), None);
        // After the overflow a gap: the run restarts at the new tag.
        rt.capture(7 * BATCH as u64, &inputs);
        rt.capture(8 * BATCH as u64, &inputs);
        rt.capture(10 * BATCH as u64, &inputs);
        let mut again = Vec::new();
        assert_eq!(tap.drain(&mut again), Some((7 * BATCH as u64, 2 * BATCH)));
        assert_eq!(tap.drain(&mut again), Some((10 * BATCH as u64, BATCH)));
        assert_eq!(again[0].len(), 3 * BATCH);
    }

    #[test]
    fn frame_tags_use_both_words_and_missing_channels_read_zero() {
        let (mut rt, mut tap) = new_tap(&[5], 2 * BATCH);
        let big = (1u64 << 40) + 192;
        rt.capture(big, &[batch(1.0)]);
        let mut out = Vec::new();
        assert_eq!(tap.drain(&mut out), Some((big, BATCH)));
        assert!(out[0].iter().all(|s| s.to_bits() == 0));
    }
}
