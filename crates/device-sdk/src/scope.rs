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
//! * **A live flag** ([`ScopeTap::set_live`], [`ScopeTap::is_live`]) is how a device that taps
//!   only on request (a console strip, a plugin device) learns that something looks at its tap:
//!   the UI sets it, the device reads it once per batch and writes nothing while it is off, so
//!   a tap nobody shows costs one relaxed load per batch. Devices whose whole job is the display
//!   (the spectrum and phase meter devices) ignore it and always write.
//! * **A point** ([`ScopeTap::set_pre`], [`ScopeTap::is_pre`]): a console strip can write the
//!   signal before its fader instead of after it. The UI sets it like the live flag and the
//!   device reads it only while the tap is live, so a tap nobody shows is still one load.
//! * **A large ring on request** ([`ScopeTap::reserve_large`]): a default tap holds
//!   [`SCOPE_FRAMES`] frames, enough for analysis windows up to 16384. A window of 32768 or 65536
//!   needs a ring of [`LARGE_SCOPE_FRAMES`] frames (1 MiB), which most taps never use. The UI
//!   thread allocates it when a panel picks such a size and stores it in a `OnceLock`; the
//!   writer does one `OnceLock::get` per batch (an atomic load, no lock, no allocation) and,
//!   once the ring is there, writes every frame into both rings. A read that wants more than
//!   half the small ring takes the large one, whose history starts at the first batch written
//!   after it appeared: the first second of a large window is shorter than asked for.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Frames of history in a tap (a power of two): 0.68 s at 48 kHz, twice the analysis window of
/// the largest size a strip's panel offers.
pub const SCOPE_FRAMES: usize = 32_768;

/// Frames of history of the large taps ([`ScopeTap::large`]): twice the largest analysis
/// window (65536) the spectrum device offers, 1 MiB for the two channels.
pub const LARGE_SCOPE_FRAMES: usize = 131_072;

// A read is capped at half a ring: room for the largest analysis window of a default tap.
const _: () = assert!(SCOPE_FRAMES / 2 >= 16_384);
const _: () = assert!(LARGE_SCOPE_FRAMES / 2 >= 65_536);

/// How many times a read that the writer lapped is repeated before it gives up.
const READ_ATTEMPTS: usize = 4;

/// The large ring a tap grows on request (see the module docs).
#[derive(Debug)]
struct Big {
    left: Box<[AtomicU32]>,
    right: Box<[AtomicU32]>,
    /// The head the ring's history starts at: set by the writer on its first batch after the
    /// ring appeared (`u64::MAX` until then).
    from: AtomicU64,
}

/// A ring of the most recent [`SCOPE_FRAMES`] stereo frames (see the module docs).
#[derive(Debug)]
pub struct ScopeTap {
    left: Box<[AtomicU32]>,
    right: Box<[AtomicU32]>,
    /// Frames written since creation (never wraps in practice: 2^64 frames).
    head: AtomicU64,
    /// Whether anything looks at the tap (see the module docs).
    live: AtomicBool,
    /// Whether a strip writes the signal before its fader (see the module docs).
    pre: AtomicBool,
    /// The large ring, once a reader asked for it.
    big: OnceLock<Big>,
}

impl Default for ScopeTap {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopeTap {
    /// An empty tap of [`SCOPE_FRAMES`] frames (allocates; non-RT).
    pub fn new() -> Self {
        Self::with_frames(SCOPE_FRAMES)
    }

    /// An empty tap of [`LARGE_SCOPE_FRAMES`] frames, for a window of up to 65536 frames.
    pub fn large() -> Self {
        Self::with_frames(LARGE_SCOPE_FRAMES)
    }

    /// An empty tap holding `frames` frames, rounded up to a power of two (at least 64;
    /// allocates; non-RT).
    pub fn with_frames(frames: usize) -> Self {
        let frames = frames.next_power_of_two().max(64);
        let ring = || {
            (0..frames)
                .map(|_| AtomicU32::new(0))
                .collect::<Box<[AtomicU32]>>()
        };
        Self {
            left: ring(),
            right: ring(),
            head: AtomicU64::new(0),
            live: AtomicBool::new(false),
            pre: AtomicBool::new(false),
            big: OnceLock::new(),
        }
    }

    /// Makes room for windows of up to 65536 frames: allocates the large ring when this tap
    /// does not hold one yet (1 MiB; non-RT, any thread, idempotent). The writer starts filling
    /// it with its next batch.
    pub fn reserve_large(&self) {
        if self.left.len() >= LARGE_SCOPE_FRAMES {
            return;
        }
        self.big.get_or_init(|| {
            let ring = || {
                (0..LARGE_SCOPE_FRAMES)
                    .map(|_| AtomicU32::new(0))
                    .collect::<Box<[AtomicU32]>>()
            };
            Big {
                left: ring(),
                right: ring(),
                from: AtomicU64::new(u64::MAX),
            }
        });
    }

    /// Whether the large ring is there (or the tap is large from the start).
    pub fn has_large(&self) -> bool {
        self.left.len() >= LARGE_SCOPE_FRAMES || self.big.get().is_some()
    }

    /// Frames of history the tap holds; a read returns at most half of them.
    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// Tells the writer whether anything looks at the tap (module docs). Any thread.
    pub fn set_live(&self, live: bool) {
        self.live.store(live, Ordering::Relaxed);
    }

    /// RT-safe. Whether something looks at the tap: one relaxed load.
    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    /// Tells a strip to write the signal before its fader (`true`) or after it (module docs).
    pub fn set_pre(&self, pre: bool) {
        self.pre.store(pre, Ordering::Relaxed);
    }

    /// RT-safe. Whether the signal before the fader is wanted: one relaxed load.
    pub fn is_pre(&self) -> bool {
        self.pre.load(Ordering::Relaxed)
    }

    /// RT-safe. Appends the frames `(left[i], right[i])` for the shorter of the two slices.
    /// Single writer only.
    pub fn push(&self, left: &[f32], right: &[f32]) {
        let n = left.len().min(right.len());
        let start = self.head.load(Ordering::Relaxed);
        for (i, (l, r)) in left.iter().zip(right).enumerate().take(n) {
            let at = (start.wrapping_add(i as u64) as usize) & (self.left.len() - 1);
            self.left[at].store(l.to_bits(), Ordering::Relaxed);
            self.right[at].store(r.to_bits(), Ordering::Relaxed);
        }
        // The large ring, once the UI made it: one atomic load when it is not there.
        if let Some(big) = self.big.get() {
            if big.from.load(Ordering::Relaxed) == u64::MAX {
                big.from.store(start, Ordering::Relaxed);
            }
            for (i, (l, r)) in left.iter().zip(right).enumerate().take(n) {
                let at = (start.wrapping_add(i as u64) as usize) & (big.left.len() - 1);
                big.left[at].store(l.to_bits(), Ordering::Relaxed);
                big.right[at].store(r.to_bits(), Ordering::Relaxed);
            }
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
        if let Some(big) = self.big.get() {
            for cell in big.left.iter().chain(big.right.iter()) {
                cell.store(0, Ordering::Relaxed);
            }
        }
    }

    /// Copies the newest frames into `left` and `right`, oldest first, so that the last element
    /// written is the most recent frame. Copies at most `left.len().min(right.len())` and at
    /// most [`ScopeTap::frames`] / 2 frames (with a large ring: [`LARGE_SCOPE_FRAMES`] / 2 when
    /// more is asked for and the large ring holds more), and never more than were written; the
    /// unused tail of the slices is untouched. Returns `(frames copied, head the copy ends at)`;
    /// `(0, head)` when nothing was written yet or the writer kept lapping the copy.
    pub fn read_latest(&self, left: &mut [f32], right: &mut [f32]) -> (usize, u64) {
        let want = left.len().min(right.len());
        if want > self.left.len() / 2
            && let Some(big) = self.big.get()
        {
            let from = big.from.load(Ordering::Relaxed);
            let end = self.head.load(Ordering::Acquire);
            let held = if from == u64::MAX {
                0
            } else {
                end.saturating_sub(from)
            };
            // Only when it holds more than the small ring could give.
            if held > (self.left.len() as u64 / 2).min(end) {
                return copy_ring(&big.left, &big.right, &self.head, from, want, left, right);
            }
        }
        copy_ring(&self.left, &self.right, &self.head, 0, want, left, right)
    }
}

/// Copies up to `want` of the newest frames of a ring (see [`ScopeTap::read_latest`]); frames
/// before the head `floor` were never written to it.
fn copy_ring(
    ring_l: &[AtomicU32],
    ring_r: &[AtomicU32],
    head: &AtomicU64,
    floor: u64,
    want: usize,
    left: &mut [f32],
    right: &mut [f32],
) -> (usize, u64) {
    let ring = ring_l.len();
    let want = want.min(ring / 2);
    for _ in 0..READ_ATTEMPTS {
        let end = head.load(Ordering::Acquire);
        let n = (want as u64).min(end.saturating_sub(floor)) as usize;
        let first = end - n as u64;
        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate().take(n) {
            let at = (first.wrapping_add(i as u64) as usize) & (ring - 1);
            *l = f32::from_bits(ring_l[at].load(Ordering::Relaxed));
            *r = f32::from_bits(ring_r[at].load(Ordering::Relaxed));
        }
        // The writer may have run ahead while we copied; the oldest frames we read are
        // intact only while it is still less than a ring behind the end of the copy.
        let now = head.load(Ordering::Acquire);
        if now.saturating_sub(first) <= ring as u64 {
            return (n, end);
        }
    }
    (0, head.load(Ordering::Acquire))
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

    #[test]
    fn a_large_tap_holds_a_65536_frame_window_and_a_default_tap_16384() {
        let big = ScopeTap::large();
        assert_eq!(big.frames(), LARGE_SCOPE_FRAMES);
        let (l, r) = ramp(0, 70_000);
        big.push(&l, &r);
        let (mut a, mut b) = (vec![0.0; 65_536], vec![0.0; 65_536]);
        assert_eq!(big.read_latest(&mut a, &mut b), (65_536, 70_000));
        assert_eq!(a[65_535].to_bits(), 69_999.0f32.to_bits());
        assert_eq!(a[0].to_bits(), ((70_000 - 65_536) as f32).to_bits());
        let small = ScopeTap::new();
        small.push(&l, &r);
        assert_eq!(
            small.read_latest(&mut a, &mut b).0,
            16_384,
            "capped at half the ring"
        );
    }

    #[test]
    fn the_live_flag_starts_off_and_is_set_from_any_thread() {
        let tap = ScopeTap::new();
        assert!(!tap.is_live());
        tap.set_live(true);
        assert!(tap.is_live());
        tap.set_live(false);
        assert!(!tap.is_live());
    }

    #[test]
    fn a_large_ring_on_request_gives_windows_of_65536_with_the_same_bins() {
        let tap = ScopeTap::new();
        let (l, r) = ramp(0, 40_000);
        tap.push(&l, &r);
        assert!(!tap.has_large());
        tap.reserve_large();
        assert!(tap.has_large());
        // Nothing was written since it appeared: a big window still gets the small ring's frames.
        let (mut a, mut b) = (vec![0.0; 65_536], vec![0.0; 65_536]);
        assert_eq!(tap.read_latest(&mut a, &mut b).0, SCOPE_FRAMES / 2);
        // Write on: the history of the large ring grows with every batch.
        let mut next = 40_000;
        while next < 40_000 + 70_000 {
            let (l, r) = ramp(next, 64);
            tap.push(&l, &r);
            next += 64;
        }
        let (n, head) = tap.read_latest(&mut a, &mut b);
        assert_eq!((n, head), (65_536, next as u64));
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            let frame = next - 65_536 + i;
            assert_eq!(x.to_bits(), (frame as f32).to_bits(), "frame {i}");
            assert_eq!(y.to_bits(), (-(frame as f32)).to_bits());
        }
        // A window the small ring covers reads the same frames from either ring.
        let (mut c, mut d) = (vec![0.0; 4096], vec![0.0; 4096]);
        assert_eq!(tap.read_latest(&mut c, &mut d), (4096, next as u64));
        assert_eq!(&c[..], &a[65_536 - 4096..]);
    }

    #[test]
    fn a_large_ring_that_is_still_filling_gives_what_it_has() {
        let tap = ScopeTap::new();
        tap.reserve_large();
        let mut next = 0;
        while next < 20_000 {
            let (l, r) = ramp(next, 64);
            tap.push(&l, &r);
            next += 64;
        }
        // 20 000 frames are in both rings; the small one can give 16384 of them.
        let (mut a, mut b) = (vec![0.0; 65_536], vec![0.0; 65_536]);
        let (n, head) = tap.read_latest(&mut a, &mut b);
        assert_eq!(n, next, "the large ring holds all of it");
        assert_eq!(head, next as u64);
        assert_eq!(a[n - 1].to_bits(), ((next - 1) as f32).to_bits());
    }

    #[test]
    fn the_point_flag_starts_post_and_a_large_tap_needs_no_reserve() {
        let tap = ScopeTap::new();
        assert!(!tap.is_pre());
        tap.set_pre(true);
        assert!(tap.is_pre());
        let big = ScopeTap::large();
        assert!(big.has_large());
        big.reserve_large();
        assert_eq!(big.frames(), LARGE_SCOPE_FRAMES);
    }
}
