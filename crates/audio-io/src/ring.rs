//! A wait-free single-producer single-consumer `f32` ring carrying captured input samples
//! from the input stream callback to the output (render) callback. Safe code only: samples are
//! stored as `AtomicU32` bit patterns. Allocated once in [`sample_ring`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

struct Shared {
    slots: Box<[AtomicU32]>,
    /// Total samples written (monotonic).
    head: AtomicUsize,
    /// Total samples read or skipped (monotonic).
    tail: AtomicUsize,
    /// Samples dropped because the ring was full.
    overruns: AtomicU64,
}

/// Producer half (input callback).
pub(crate) struct RingWriter(Arc<Shared>);

/// Consumer half (output callback).
pub(crate) struct RingReader(Arc<Shared>);

/// A ring holding up to `capacity` samples.
pub(crate) fn sample_ring(capacity: usize) -> (RingWriter, RingReader) {
    let shared = Arc::new(Shared {
        slots: (0..capacity.max(1)).map(|_| AtomicU32::new(0)).collect(),
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        overruns: AtomicU64::new(0),
    });
    (RingWriter(Arc::clone(&shared)), RingReader(shared))
}

impl RingWriter {
    /// Appends as many samples as fit; the rest are dropped and counted. Returns the number
    /// written.
    pub(crate) fn write(&mut self, samples: &[f32]) -> usize {
        let s = &*self.0;
        let cap = s.slots.len();
        let head = s.head.load(Ordering::Relaxed);
        let tail = s.tail.load(Ordering::Acquire);
        let free = cap - head.wrapping_sub(tail).min(cap);
        let n = samples.len().min(free);
        for (i, x) in samples[..n].iter().enumerate() {
            s.slots[head.wrapping_add(i) % cap].store(x.to_bits(), Ordering::Relaxed);
        }
        s.head.store(head.wrapping_add(n), Ordering::Release);
        let dropped = (samples.len() - n) as u64;
        if dropped > 0 {
            s.overruns.fetch_add(dropped, Ordering::Relaxed);
        }
        n
    }
}

impl RingReader {
    /// Samples available to read.
    pub(crate) fn available(&self) -> usize {
        let s = &*self.0;
        s.head
            .load(Ordering::Acquire)
            .wrapping_sub(s.tail.load(Ordering::Relaxed))
    }

    /// Fills `out` from the ring; samples not available read as 0. Returns the number read.
    pub(crate) fn read(&mut self, out: &mut [f32]) -> usize {
        let n = self.available().min(out.len());
        let s = &*self.0;
        let cap = s.slots.len();
        let tail = s.tail.load(Ordering::Relaxed);
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = f32::from_bits(s.slots[tail.wrapping_add(i) % cap].load(Ordering::Relaxed));
        }
        s.tail.store(tail.wrapping_add(n), Ordering::Release);
        out[n..].fill(0.0);
        n
    }

    /// Discards the oldest samples so that at most `keep` remain (bounds drift latency).
    /// Returns how many were discarded.
    pub(crate) fn trim(&mut self, keep: usize) -> usize {
        let excess = self.available().saturating_sub(keep);
        if excess > 0 {
            let tail = self.0.tail.load(Ordering::Relaxed);
            self.0
                .tail
                .store(tail.wrapping_add(excess), Ordering::Release);
        }
        excess
    }

    /// Samples dropped by the writer because the ring was full.
    pub(crate) fn overruns(&self) -> u64 {
        self.0.overruns.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_preserves_fifo_order_across_wraparound() {
        let (mut w, mut r) = sample_ring(8);
        let mut out = [0.0; 5];
        let mut next = 0.0f32;
        let mut expect = 0.0f32;
        for _ in 0..20 {
            let block: Vec<f32> = (0..5).map(|i| next + i as f32).collect();
            assert_eq!(w.write(&block), 5);
            next += 5.0;
            assert_eq!(r.read(&mut out), 5);
            for s in out {
                assert!((s - expect).abs() < f32::EPSILON);
                expect += 1.0;
            }
        }
    }

    #[test]
    fn ring_underrun_zero_fills() {
        let (mut w, mut r) = sample_ring(8);
        w.write(&[1.0, 2.0]);
        let mut out = [9.0; 4];
        assert_eq!(r.read(&mut out), 2);
        assert_eq!(
            out.map(f32::to_bits),
            [1.0f32, 2.0, 0.0, 0.0].map(f32::to_bits)
        );
    }

    #[test]
    fn ring_overflow_drops_newest_and_counts() {
        let (mut w, mut r) = sample_ring(4);
        assert_eq!(w.write(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), 4);
        assert_eq!(r.overruns(), 2);
        assert_eq!(r.available(), 4);
        let mut out = [0.0; 4];
        r.read(&mut out);
        assert_eq!(
            out.map(f32::to_bits),
            [1.0f32, 2.0, 3.0, 4.0].map(f32::to_bits)
        );
    }

    #[test]
    fn ring_trim_keeps_newest_samples() {
        let (mut w, mut r) = sample_ring(16);
        w.write(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(r.trim(2), 4);
        assert_eq!(r.available(), 2);
        let mut out = [0.0; 2];
        r.read(&mut out);
        assert_eq!(out.map(f32::to_bits), [5.0f32, 6.0].map(f32::to_bits));
        assert_eq!(r.trim(10), 0); // nothing to trim
        assert_eq!(r.available(), 0);
    }

    #[test]
    fn ring_is_allocation_free_and_works_across_threads() {
        let (mut w, mut r) = sample_ring(1024);
        let producer = std::thread::spawn(move || {
            let block = [0.5f32; 64];
            let mut sent = 0;
            atomik_testkit::no_alloc::assert_no_alloc(|| {
                while sent < 64 * 100 {
                    sent += w.write(&block[..(64 * 100 - sent).min(64)]);
                    std::hint::spin_loop();
                }
            });
        });
        let mut got = 0;
        let mut out = [0.0f32; 64];
        atomik_testkit::no_alloc::assert_no_alloc(|| {
            while got < 64 * 100 {
                let n = r.available().min(64);
                if n > 0 {
                    got += r.read(&mut out[..n]);
                    assert!(out[..n].iter().all(|s| (s - 0.5).abs() < f32::EPSILON));
                }
                std::hint::spin_loop();
            }
        });
        producer.join().unwrap();
    }
}
