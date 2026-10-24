//! [`ScopeTap`]: a lock-free ring of recent stereo frames that a device writes on the audio
//! thread and the UI reads from another.
//!
//! [`Device::meter_values`](crate::Device::meter_values) carries at most 32 numbers per batch,
//! which is enough for a level or a correlation but not for a spectrum or a goniometer: those
//! need the recent samples themselves. A device that wants to show them creates one tap in its
//! constructor, returns it from [`Device::scope_tap`](crate::Device::scope_tap) and pushes every
//! batch it processes; the engine hands the same `Arc` to the UI through its meter bank, and the
//! UI runs the analysis (FFT, decimation, drawing) on its own thread. The audio thread only does
//! relaxed atomic stores and one release store of the write position per batch.
//!
//! * **Single writer, any number of readers.** `push` must be called from one thread at a time
//!   (a device runs on one thread per batch; the hand-over between threads synchronises).
//! * **Reads are copies.** [`ScopeTap::read_latest`] copies the newest frames and then checks
//!   that the writer did not lap the part it copied; a lapped read is retried, and after a few
//!   attempts reports nothing. With [`SCOPE_FRAMES`] frames of history that only happens when a
//!   reader is suspended for most of a second in the middle of a copy.
//! * **Frames are stored as `f32` bits in `AtomicU32`s**, so there is no `unsafe`, no lock and
//!   no allocation after [`ScopeTap::new`].

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Frames of history in a tap (a power of two): 0.68 s at 48 kHz, four times the largest
/// analysis window the spectrum device offers.
pub const SCOPE_FRAMES: usize = 32_768;

// A read is capped at half a ring: room for the largest analysis window (8192) with margin.
const _: () = assert!(SCOPE_FRAMES / 2 >= 8192);

/// How many times a read that the writer lapped is repeated before it gives up.
const READ_ATTEMPTS: usize = 4;

/// A ring of the most recent [`SCOPE_FRAMES`] stereo frames (see the module docs).
#[derive(Debug)]
pub struct ScopeTap {
    left: Box<[AtomicU32]>,
    right: Box<[AtomicU32]>,
    /// Frames written since creation (never wraps in practice: 2^64 frames).
    head: AtomicU64,
}

impl Default for ScopeTap {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopeTap {
    /// An empty tap (allocates; non-RT).
    pub fn new() -> Self {
        let ring = || {
            (0..SCOPE_FRAMES)
                .map(|_| AtomicU32::new(0))
                .collect::<Box<[AtomicU32]>>()
        };
        Self {
            left: ring(),
            right: ring(),
            head: AtomicU64::new(0),
        }
    }

    /// RT-safe. Appends the frames `(left[i], right[i])` for the shorter of the two slices.
    /// Single writer only.
    pub fn push(&self, left: &[f32], right: &[f32]) {
        let n = left.len().min(right.len());
        let start = self.head.load(Ordering::Relaxed);
        for (i, (l, r)) in left.iter().zip(right).enumerate().take(n) {
            let at = (start.wrapping_add(i as u64) as usize) & (SCOPE_FRAMES - 1);
            self.left[at].store(l.to_bits(), Ordering::Relaxed);
            self.right[at].store(r.to_bits(), Ordering::Relaxed);
        }
        self.head
            .store(start.wrapping_add(n as u64), Ordering::Release);
    }

    /// Frames written so far. It grows with every push, so an unchanged value means no news.
    pub fn head(&self) -> u64 {
        self.head.load(Ordering::Acquire)
    }

    /// Forgets the history (the next reads see silence from now on). The writer's thread only.
    pub fn clear(&self) {
        for cell in self.left.iter().chain(self.right.iter()) {
            cell.store(0, Ordering::Relaxed);
        }
    }

    /// Copies the newest frames into `left` and `right`, oldest first, so that the last element
    /// written is the most recent frame. Copies at most `left.len().min(right.len())` and at
    /// most [`SCOPE_FRAMES`] / 2 frames, and never more than were written; the unused tail of
    /// the slices is untouched. Returns `(frames copied, head the copy ends at)`; `(0, head)`
    /// when nothing was written yet or the writer kept lapping the copy.
    pub fn read_latest(&self, left: &mut [f32], right: &mut [f32]) -> (usize, u64) {
        let want = left.len().min(right.len()).min(SCOPE_FRAMES / 2);
        for _ in 0..READ_ATTEMPTS {
            let end = self.head.load(Ordering::Acquire);
            let n = (want as u64).min(end) as usize;
            let first = end - n as u64;
            for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate().take(n) {
                let at = (first.wrapping_add(i as u64) as usize) & (SCOPE_FRAMES - 1);
                *l = f32::from_bits(self.left[at].load(Ordering::Relaxed));
                *r = f32::from_bits(self.right[at].load(Ordering::Relaxed));
            }
            // The writer may have run ahead while we copied; the oldest frames we read are
            // intact only while it is still less than a ring behind the end of the copy.
            let now = self.head.load(Ordering::Acquire);
            if now.saturating_sub(first) <= SCOPE_FRAMES as u64 {
                return (n, end);
            }
        }
        (0, self.head.load(Ordering::Acquire))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(from: usize, n: usize) -> (Vec<f32>, Vec<f32>) {
        (
            (from..from + n).map(|i| i as f32).collect(),
            (from..from + n).map(|i| -(i as f32)).collect(),
        )
    }

    #[test]
    fn an_empty_tap_reads_nothing() {
        let tap = ScopeTap::new();
        let (mut l, mut r) = (vec![9.0; 8], vec![9.0; 8]);
        assert_eq!(tap.read_latest(&mut l, &mut r), (0, 0));
        assert_eq!(l, vec![9.0; 8], "the buffers are untouched");
    }

    #[test]
    fn the_newest_frames_come_back_oldest_first() {
        let tap = ScopeTap::new();
        let (l, r) = ramp(0, 100);
        tap.push(&l, &r);
        let (mut a, mut b) = (vec![0.0; 10], vec![0.0; 10]);
        assert_eq!(tap.read_latest(&mut a, &mut b), (10, 100));
        assert_eq!(a, (90..100).map(|i| i as f32).collect::<Vec<_>>());
        assert_eq!(b, (90..100).map(|i| -(i as f32)).collect::<Vec<_>>());
        // Fewer frames written than asked for: only those come back.
        let tap = ScopeTap::new();
        let (l, r) = ramp(0, 5);
        tap.push(&l, &r);
        let (mut a, mut b) = (vec![0.0; 10], vec![0.0; 10]);
        assert_eq!(tap.read_latest(&mut a, &mut b), (5, 5));
        assert_eq!(&a[..5], &[0.0, 1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn the_ring_wraps_and_keeps_the_order() {
        let tap = ScopeTap::new();
        // Several laps of the ring in odd-sized pushes.
        let mut next = 0;
        while next < SCOPE_FRAMES * 3 + 17 {
            let (l, r) = ramp(next, 64);
            tap.push(&l, &r);
            next += 64;
        }
        let want = 1000;
        let (mut a, mut b) = (vec![0.0; want], vec![0.0; want]);
        let (n, head) = tap.read_latest(&mut a, &mut b);
        assert_eq!((n, head), (want, next as u64));
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            let frame = next - want + i;
            assert_eq!(x.to_bits(), (frame as f32).to_bits());
            assert_eq!(y.to_bits(), (-(frame as f32)).to_bits());
        }
        // Never more than half a ring, however large the buffers.
        let (mut a, mut b) = (vec![0.0; SCOPE_FRAMES], vec![0.0; SCOPE_FRAMES]);
        assert_eq!(tap.read_latest(&mut a, &mut b).0, SCOPE_FRAMES / 2);
    }

    #[test]
    fn head_grows_with_every_push_and_clear_silences_the_history() {
        let tap = ScopeTap::new();
        let before = tap.head();
        tap.push(&[1.0; 64], &[1.0; 64]);
        assert_eq!(tap.head(), before + 64);
        tap.clear();
        let (mut a, mut b) = (vec![5.0; 4], vec![5.0; 4]);
        assert_eq!(tap.read_latest(&mut a, &mut b), (4, 64));
        assert_eq!(a, vec![0.0; 4]);
    }

    #[test]
    fn a_reader_on_another_thread_sees_whole_frames() {
        use std::sync::Arc;
        let tap = Arc::new(ScopeTap::new());
        let writer = {
            let tap = Arc::clone(&tap);
            std::thread::spawn(move || {
                // Frame k carries (k, -k): a torn copy would break the pairing or the order.
                for block in 0..2000usize {
                    let (l, r) = ramp(block * 64, 64);
                    tap.push(&l, &r);
                }
            })
        };
        let (mut a, mut b) = (vec![0.0; 256], vec![0.0; 256]);
        for _ in 0..2000 {
            let (n, _) = tap.read_latest(&mut a, &mut b);
            for (i, (x, y)) in a[..n].iter().zip(&b[..n]).enumerate() {
                assert_eq!(x.to_bits(), (-y).to_bits(), "L and R of one frame");
                if i > 0 {
                    assert_eq!((x - a[i - 1]).to_bits(), 1.0f32.to_bits(), "in order");
                }
            }
        }
        writer.join().unwrap();
    }
}
