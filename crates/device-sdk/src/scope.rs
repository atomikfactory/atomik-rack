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
//! * **Frames are stored as `f32` bits in `AtomicU32`s**, so there is no `unsafe` and no lock;
//!   a ring is allocated once, never by the writer.
//! * **Two kinds of tap.** [`ScopeTap::new`] allocates its ring at once (256 KiB): for a device
//!   that writes every batch whatever the interface does and whose readers never say they look
//!   (the spectrum, phase meter, loudness meter, tonal balance and tuner devices, an audio
//!   track's tuner feed). [`ScopeTap::on_demand`] holds only its header (a few dozen bytes)
//!   until somebody asks for the ring, the way the large ring is made (below): for a device that
//!   writes only while something looks at it (a console strip, the master, an EQ, a plugin
//!   device), which is most taps of a big song. The ring is made, on the thread of the one who
//!   asks, by [`ScopeTap::set_live`] with `true` (the interface calls it when a view shows the
//!   tap), [`ScopeTap::reserve`] (a switch that makes the device write without the live flag: the
//!   Spectrum EQ window's `internal.tap_live`) or [`ScopeTap::reserve_large`], and kept for the
//!   life of the tap. Two threads that ask together end with one ring.
//! * **No ring yet.** The writer does one `OnceLock::get` (an atomic load) and, finding no ring,
//!   only advances the write position: it writes nothing and allocates nothing. A read from a
//!   tap that has no ring gives `(0, head)`, like a read from a tap nothing was written to. A
//!   ring made on request starts empty: [`ScopeTap::read_latest`] gives only the frames written
//!   after it appeared (its history starts at the first batch the writer pushed into it, as
//!   the large ring's does), never frames from before. A tap that was never asked is never
//!   written by its device, so a reader that does not ask (an offline render, the QA server)
//!   reads exactly what it read before.
//! * **A live flag** ([`ScopeTap::set_live`], [`ScopeTap::is_live`]) is how a device that taps
//!   only on request (a console strip, a plugin device) learns that something looks at its tap:
//!   the UI sets it, the device reads it once per batch and writes nothing while it is off, so
//!   a tap nobody shows costs one relaxed load per batch. Devices whose whole job is the display
//!   (the spectrum and phase meter devices) ignore it and always write.
//! * **An observed flag** ([`ScopeTap::set_observed`], [`ScopeTap::is_observed`]): the other half
//!   of "does anything look?". It says whether the numbers a device only measures for its panel
//!   (loudness, true peak, stereo image) are wanted: the windowed app clears it while no panel of
//!   the device is on screen and sets it again before the panel is, and the device skips that
//!   display-only metering while it is clear. It is **on by default** and only the windowed app
//!   ever clears it, so an offline render, a test and the QA server measure exactly as before.
//!   It is not a parameter (it is not saved, automated or undone). A device with no samples to
//!   show carries it on a tap that never gets a ring ([`ScopeTap::flags_only`]).
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

/// Frames a [`ScopeTap::flags_only`] tap reports (it never gets a ring).
const FLAGS_ONLY_FRAMES: usize = 64;

/// One ring of stereo frames: the small one every tap has once it is asked for, and the large
/// one a tap grows on request (see the module docs).
#[derive(Debug)]
struct Ring {
    left: Box<[AtomicU32]>,
    right: Box<[AtomicU32]>,
    /// The head the ring's history starts at: 0 for a ring made with the tap; for a ring made
    /// on request, set by the writer on its first batch after the ring appeared (`u64::MAX`
    /// until then).
    from: AtomicU64,
}

impl Ring {
    /// A zeroed ring of `frames` frames (a power of two) whose history starts at `from`.
    fn new(frames: usize, from: u64) -> Self {
        let ring = || {
            (0..frames)
                .map(|_| AtomicU32::new(0))
                .collect::<Box<[AtomicU32]>>()
        };
        Self {
            left: ring(),
            right: ring(),
            from: AtomicU64::new(from),
        }
    }

    /// Writes `n` frames of `left` / `right` at the positions after head `start`.
    fn write(&self, start: u64, n: usize, left: &[f32], right: &[f32]) {
        if self.from.load(Ordering::Relaxed) == u64::MAX {
            self.from.store(start, Ordering::Relaxed);
        }
        let mask = self.left.len() - 1;
        for (i, (l, r)) in left.iter().zip(right).enumerate().take(n) {
            let at = (start.wrapping_add(i as u64) as usize) & mask;
            self.left[at].store(l.to_bits(), Ordering::Relaxed);
            self.right[at].store(r.to_bits(), Ordering::Relaxed);
        }
    }
}

/// A ring of the most recent [`SCOPE_FRAMES`] stereo frames (see the module docs).
#[derive(Debug)]
pub struct ScopeTap {
    /// Frames of history the tap holds once its ring exists.
    frames: usize,
    /// Whether the ring is made on request ([`ScopeTap::on_demand`]).
    on_demand: bool,
    /// The ring (made with the tap, or on request).
    small: OnceLock<Ring>,
    /// Frames written since creation (never wraps in practice: 2^64 frames).
    head: AtomicU64,
    /// Whether anything looks at the tap (see the module docs).
    live: AtomicBool,
    /// Whether the display-only metering of the device is wanted (see the module docs).
    observed: AtomicBool,
    /// Whether a strip writes the signal before its fader (see the module docs).
    pre: AtomicBool,
    /// The large ring, once a reader asked for it.
    big: OnceLock<Ring>,
}

impl Default for ScopeTap {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopeTap {
    /// An empty tap of [`SCOPE_FRAMES`] frames, its ring allocated at once (allocates; non-RT).
    pub fn new() -> Self {
        Self::with_frames(SCOPE_FRAMES)
    }

    /// An empty tap of [`SCOPE_FRAMES`] frames whose ring is made when something asks for it
    /// ([`ScopeTap::set_live`] with `true`, [`ScopeTap::reserve`], [`ScopeTap::reserve_large`]):
    /// until then it holds no ring and allocates nothing.
    pub fn on_demand() -> Self {
        Self::bare(SCOPE_FRAMES, true)
    }

    /// An empty tap of [`LARGE_SCOPE_FRAMES`] frames, for a window of up to 65536 frames.
    pub fn large() -> Self {
        Self::with_frames(LARGE_SCOPE_FRAMES)
    }

    /// An empty tap holding `frames` frames, rounded up to a power of two (at least 64;
    /// allocates; non-RT).
    pub fn with_frames(frames: usize) -> Self {
        let frames = frames.next_power_of_two().max(64);
        let tap = Self::bare(frames, false);
        let _ = tap.small.set(Ring::new(frames, 0));
        tap
    }

    /// A tap that carries the flags only: it never gets a ring and is never written, for a
    /// device that has no samples to show but is told whether its display-only metering is
    /// wanted ([`Self::set_observed`]). Holds no allocation.
    pub fn flags_only() -> Self {
        Self::bare(FLAGS_ONLY_FRAMES, false)
    }

    /// A tap with no ring.
    fn bare(frames: usize, on_demand: bool) -> Self {
        Self {
            frames,
            on_demand,
            small: OnceLock::new(),
            head: AtomicU64::new(0),
            live: AtomicBool::new(false),
            observed: AtomicBool::new(true),
            pre: AtomicBool::new(false),
            big: OnceLock::new(),
        }
    }

    /// Makes the ring of an [`ScopeTap::on_demand`] tap when it has none yet (256 KiB; non-RT,
    /// any thread, idempotent: two threads that ask together end with one ring). The writer
    /// starts filling it with its next batch. Nothing to do for a tap whose ring exists from the
    /// start and for a [`ScopeTap::flags_only`] tap, which never gets one.
    pub fn reserve(&self) {
        if self.on_demand {
            self.small.get_or_init(|| Ring::new(self.frames, u64::MAX));
        }
    }

    /// Whether the ring exists (always for a tap made with [`ScopeTap::new`], never for a
    /// [`ScopeTap::flags_only`] one).
    pub fn has_ring(&self) -> bool {
        self.small.get().is_some()
    }

    /// Makes room for windows of up to 65536 frames: allocates the large ring when this tap
    /// does not hold one yet (1 MiB; non-RT, any thread, idempotent; it also makes the ring of
    /// an on-demand tap). The writer starts filling it with its next batch.
    pub fn reserve_large(&self) {
        self.reserve();
        if self.frames >= LARGE_SCOPE_FRAMES {
            return;
        }
        self.big
            .get_or_init(|| Ring::new(LARGE_SCOPE_FRAMES, u64::MAX));
    }

    /// Whether the large ring is there (or the tap is large from the start).
    pub fn has_large(&self) -> bool {
        self.frames >= LARGE_SCOPE_FRAMES || self.big.get().is_some()
    }

    /// Frames of history the tap holds (once its ring exists); a read returns at most half of
    /// them.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Tells the writer whether anything looks at the tap (module docs). Any thread. Turning it
    /// on makes the ring of an on-demand tap first, so the writer finds it when it sees the flag.
    pub fn set_live(&self, live: bool) {
        if live {
            self.reserve();
        }
        self.live.store(live, Ordering::Relaxed);
    }

    /// RT-safe. Whether something looks at the tap: one relaxed load.
    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    /// Tells the device whether its display-only metering is wanted (module docs). Any thread.
    pub fn set_observed(&self, observed: bool) {
        self.observed.store(observed, Ordering::Relaxed);
    }

    /// RT-safe. Whether the display-only metering is wanted: one relaxed load. True until the
    /// interface says otherwise.
    pub fn is_observed(&self) -> bool {
        self.observed.load(Ordering::Relaxed)
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
    /// Single writer only. Never allocates: a tap that has no ring only advances its head.
    pub fn push(&self, left: &[f32], right: &[f32]) {
        let n = left.len().min(right.len());
        let start = self.head.load(Ordering::Relaxed);
        // The rings, once they are there: one atomic load each when they are not.
        if let Some(ring) = self.small.get() {
            ring.write(start, n, left, right);
        }
        if let Some(big) = self.big.get() {
            big.write(start, n, left, right);
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
        for ring in [self.small.get(), self.big.get()].into_iter().flatten() {
            for cell in ring.left.iter().chain(ring.right.iter()) {
                cell.store(0, Ordering::Relaxed);
            }
        }
    }

    /// Copies the newest frames into `left` and `right`, oldest first, so that the last element
    /// written is the most recent frame. Copies at most `left.len().min(right.len())` and at
    /// most [`ScopeTap::frames`] / 2 frames (with a large ring: [`LARGE_SCOPE_FRAMES`] / 2 when
    /// more is asked for and the large ring holds more), and never more than were written into
    /// the ring; the unused tail of the slices is untouched. Returns `(frames copied, head the
    /// copy ends at)`; `(0, head)` when nothing was written yet (or, for a tap with no ring,
    /// nothing could be) or the writer kept lapping the copy.
    pub fn read_latest(&self, left: &mut [f32], right: &mut [f32]) -> (usize, u64) {
        let want = left.len().min(right.len());
        if want > self.frames / 2
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
            if held > (self.frames as u64 / 2).min(end) {
                return copy_ring(big, &self.head, want, left, right);
            }
        }
        match self.small.get() {
            Some(ring) => copy_ring(ring, &self.head, want, left, right),
            None => (0, self.head.load(Ordering::Acquire)),
        }
    }
}

/// Copies up to `want` of the newest frames of a ring (see [`ScopeTap::read_latest`]); frames
/// before the head the ring's history starts at (`from`) were never written to it.
fn copy_ring(
    ring: &Ring,
    head: &AtomicU64,
    want: usize,
    left: &mut [f32],
    right: &mut [f32],
) -> (usize, u64) {
    let floor = ring.from.load(Ordering::Relaxed);
    let size = ring.left.len();
    let want = want.min(size / 2);
    for _ in 0..READ_ATTEMPTS {
        let end = head.load(Ordering::Acquire);
        let n = (want as u64).min(end.saturating_sub(floor)) as usize;
        let first = end - n as u64;
        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate().take(n) {
            let at = (first.wrapping_add(i as u64) as usize) & (size - 1);
            *l = f32::from_bits(ring.left[at].load(Ordering::Relaxed));
            *r = f32::from_bits(ring.right[at].load(Ordering::Relaxed));
        }
        // The writer may have run ahead while we copied; the oldest frames we read are
        // intact only while it is still less than a ring behind the end of the copy.
        let now = head.load(Ordering::Acquire);
        if now.saturating_sub(first) <= size as u64 {
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
    fn a_tap_is_observed_until_somebody_says_otherwise() {
        for tap in [ScopeTap::new(), ScopeTap::flags_only()] {
            assert!(tap.is_observed(), "the default is yes");
            tap.set_observed(false);
            assert!(!tap.is_observed());
            tap.set_observed(true);
            assert!(tap.is_observed());
        }
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

    #[test]
    fn an_on_demand_tap_has_no_ring_and_reads_like_an_empty_tap() {
        let tap = ScopeTap::on_demand();
        assert!(!tap.has_ring());
        assert_eq!(tap.frames(), SCOPE_FRAMES, "the capacity it will have");
        let (mut a, mut b) = (vec![9.0; 8], vec![9.0; 8]);
        assert_eq!(tap.read_latest(&mut a, &mut b), (0, 0));
        assert_eq!(a, vec![9.0; 8], "the buffers are untouched");
        // A write with no ring only moves the head; the read still finds nothing.
        let (l, r) = ramp(0, 100);
        tap.push(&l, &r);
        assert_eq!(tap.head(), 100);
        assert!(!tap.has_ring(), "a write never makes the ring");
        assert_eq!(tap.read_latest(&mut a, &mut b), (0, 100));
        assert_eq!(a, vec![9.0; 8]);
        tap.clear();
    }

    #[test]
    fn going_live_makes_the_ring_and_only_later_frames_are_seen() {
        let tap = ScopeTap::on_demand();
        let (l, r) = ramp(0, 100);
        tap.push(&l, &r);
        tap.set_live(true);
        assert!(tap.has_ring() && tap.is_live());
        let (mut a, mut b) = (vec![9.0; 300], vec![9.0; 300]);
        assert_eq!(
            tap.read_latest(&mut a, &mut b),
            (0, 100),
            "nothing since the ring"
        );
        let (l, r) = ramp(100, 64);
        tap.push(&l, &r);
        let (n, head) = tap.read_latest(&mut a, &mut b);
        assert_eq!(
            (n, head),
            (64, 164),
            "not the 100 frames from before the ring"
        );
        assert_eq!(a[0].to_bits(), 100.0f32.to_bits());
        assert_eq!(a[63].to_bits(), 163.0f32.to_bits());
        assert_eq!(b[63].to_bits(), (-163.0f32).to_bits());
        // Turning it off keeps the ring; the history is still readable.
        tap.set_live(false);
        assert!(tap.has_ring());
        assert_eq!(tap.read_latest(&mut a, &mut b), (64, 164));
        // The same read of a tap made with a ring: it reads from the first frame.
        let eager = ScopeTap::new();
        let (l, r) = ramp(0, 100);
        eager.push(&l, &r);
        assert_eq!(eager.read_latest(&mut a, &mut b), (100, 100));
    }

    #[test]
    fn every_way_of_asking_makes_the_ring_and_a_flags_only_tap_never_has_one() {
        let tap = ScopeTap::on_demand();
        tap.set_live(false);
        assert!(!tap.has_ring(), "turning it off asks for nothing");
        tap.reserve();
        assert!(tap.has_ring());
        let tap = ScopeTap::on_demand();
        tap.reserve_large();
        assert!(tap.has_ring() && tap.has_large());
        let flags = ScopeTap::flags_only();
        flags.set_live(true);
        flags.reserve();
        assert!(!flags.has_ring());
        assert_eq!(flags.frames(), 64);
        let (mut a, mut b) = (vec![0.0; 4], vec![0.0; 4]);
        assert_eq!(flags.read_latest(&mut a, &mut b), (0, 0));
        assert!(ScopeTap::new().has_ring() && ScopeTap::large().has_ring());
    }

    #[test]
    fn a_large_ring_on_an_on_demand_tap_reads_like_one_on_a_default_tap() {
        let lazy = ScopeTap::on_demand();
        lazy.set_live(true);
        lazy.reserve_large();
        let eager = ScopeTap::new();
        eager.reserve_large();
        let mut next = 0;
        while next < 70_000 {
            let (l, r) = ramp(next, 64);
            lazy.push(&l, &r);
            eager.push(&l, &r);
            next += 64;
        }
        for want in [4_096, 16_384, 65_536] {
            let (mut a, mut b) = (vec![0.0; want], vec![0.0; want]);
            let (mut c, mut d) = (vec![0.0; want], vec![0.0; want]);
            assert_eq!(
                lazy.read_latest(&mut a, &mut b),
                eager.read_latest(&mut c, &mut d)
            );
            assert_eq!(a, c);
            assert_eq!(b, d);
        }
    }

    #[test]
    fn two_threads_that_ask_together_end_with_one_ring() {
        use std::sync::{Arc, Barrier};
        for _ in 0..20 {
            let tap = Arc::new(ScopeTap::on_demand());
            let gate = Arc::new(Barrier::new(2));
            // Both threads start before either is joined: they wait for each other.
            let threads: Vec<_> = (0..2)
                .map(|k| {
                    let (tap, gate) = (Arc::clone(&tap), Arc::clone(&gate));
                    std::thread::spawn(move || {
                        gate.wait();
                        if k == 0 {
                            tap.set_live(true);
                        } else {
                            tap.reserve();
                        }
                        tap.small.get().map(|r| std::ptr::from_ref(r) as usize)
                    })
                })
                .collect();
            let rings: Vec<usize> = threads
                .into_iter()
                .map(|t| t.join().unwrap().expect("the ring exists after the call"))
                .collect();
            assert_eq!(rings[0], rings[1], "one ring");
        }
    }
}
