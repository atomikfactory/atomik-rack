//! Contracts: multicore options, the live note queue and the frame clock.
//!
//! Public signatures are a stable contract: changes need review.

use crate::engine::{EngineError, ScheduledNote};
use atomik_core::NoteKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::time::{Duration, Instant};

/// Worker-thread configuration for multicore rendering.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkerConfig {
    /// Extra worker threads besides the audio thread. `0` renders single-threaded.
    pub threads: u16,
    /// Called once on each worker thread when it starts, before it renders anything
    /// (tests install the allocation guard; hosts may promote thread priority).
    pub on_thread_start: Option<fn()>,
}

/// Options for [`crate::new_engine_with_options`].
#[derive(Debug, Clone, Copy, Default)]
pub struct EngineOptions {
    /// Multicore configuration.
    pub workers: WorkerConfig,
    /// Busy-wait iterations an idle worker performs before it parks; `0` means
    /// [`crate::WORKER_SPIN_LIMIT`]. A live host sets it to cover at least one callback period
    /// so workers stay awake between consecutive callbacks instead of being woken (unparked)
    /// by every callback, and still park after a longer idle. It only changes when
    /// workers sleep, never what they render.
    pub worker_linger_spins: u64,
}

/// Slots of the live note queue kept for releases (see [`LiveNoteSender::push`]).
pub const LIVE_RELEASE_HEADROOM: usize = 64;

/// Producer half of the live note queue. `Send`; owned by exactly one thread
/// (typically the MIDI input thread). Obtain it once from `EngineHandle::live_notes`.
pub struct LiveNoteSender {
    pub(crate) tx: rtrb::Producer<ScheduledNote>,
    /// Shared with the engine's `dropped_events` counter.
    pub(crate) dropped: Arc<AtomicU64>,
}

impl std::fmt::Debug for LiveNoteSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveNoteSender").finish_non_exhaustive()
    }
}

impl LiveNoteSender {
    /// Queues a live note. Frames should be non-decreasing; a frame already in the past is
    /// delivered at offset 0 of the next batch. Never blocks or allocates.
    ///
    /// When the queue ([`crate::NOTE_QUEUE_CAPACITY`]) is full the note is dropped, counted in
    /// `EngineHandle::dropped_events`, and `Err(QueueFull)` is returned. The last
    /// [`LIVE_RELEASE_HEADROOM`] slots are kept for releases (`Off`, `AllOff`): any other note
    /// is refused once the queue is that close to full, so a flood of notes or controllers
    /// cannot swallow the release of a key that is already sounding.
    pub fn push(&mut self, note: ScheduledNote) -> Result<(), EngineError> {
        let release = matches!(note.kind, NoteKind::Off { .. } | NoteKind::AllOff);
        if !release && self.tx.slots() <= LIVE_RELEASE_HEADROOM {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return Err(EngineError::QueueFull);
        }
        self.tx.push(note).map_err(|_| {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            EngineError::QueueFull
        })
    }
}

/// A published `(engine host frame, wall-clock instant)` pair, written by the audio callback
/// at the start of each callback and read by the MIDI thread to timestamp events.
/// Lock-free (seqlock over atomics); `publish` never blocks or allocates.
///
/// **Seqlock.** `seq` is even when the pair is stable and odd while a publish is in progress;
/// `seq == 0` means nothing was published yet. The instant is stored as signed nanoseconds
/// relative to `epoch` (the clock's creation), so instants before the epoch round-trip too.
/// A reader retries until it sees the same even sequence before and after reading both values,
/// so a torn pair is never returned. Intended for one publishing thread (the audio callback);
/// a publish that races another publish is skipped rather than waited for, so `publish` never
/// spins and the pair is never torn even if the contract is broken.
#[derive(Debug)]
pub struct FrameClock {
    epoch: Instant,
    seq: AtomicU64,
    frame: AtomicU64,
    /// `i64` nanoseconds since `epoch`, stored as bits.
    nanos: AtomicU64,
}

impl FrameClock {
    /// A clock with no published sample yet.
    pub fn new() -> Self {
        Self {
            epoch: Instant::now_for_clock(),
            seq: AtomicU64::new(0),
            frame: AtomicU64::new(0),
            nanos: AtomicU64::new(0),
        }
    }

    /// Publishes that the engine's host frame `frame` is being rendered at `at`.
    /// Wait-free: a single compare-exchange plus three stores.
    pub fn publish(&self, frame: u64, at: Instant) {
        let nanos = match at.checked_duration_since(self.epoch) {
            Some(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
            None => {
                i64::try_from(self.epoch.duration_since(at).as_nanos()).map_or(i64::MIN, |n| -n)
            }
        };
        let seq = self.seq.load(Ordering::Relaxed);
        if seq % 2 == 1
            || self
                .seq
                .compare_exchange(seq, seq + 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            return; // another publish is in progress (contract breach): skip, never wait
        }
        fence(Ordering::Release); // the odd sequence is visible before the data changes
        self.frame.store(frame, Ordering::Relaxed);
        self.nanos.store(nanos as u64, Ordering::Relaxed);
        self.seq.store(seq + 2, Ordering::Release);
    }

    /// The latest published pair, or `None` before the first publish. Lock-free: retries only
    /// while a publish (a few stores) is in progress.
    pub fn read(&self) -> Option<(u64, Instant)> {
        loop {
            let before = self.seq.load(Ordering::Acquire);
            if before == 0 {
                return None;
            }
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let frame = self.frame.load(Ordering::Relaxed);
            let nanos = self.nanos.load(Ordering::Relaxed) as i64;
            fence(Ordering::Acquire); // the data loads happen before the re-check
            if self.seq.load(Ordering::Relaxed) == before {
                let magnitude = Duration::from_nanos(nanos.unsigned_abs());
                let at = if nanos >= 0 {
                    self.epoch.checked_add(magnitude)
                } else {
                    self.epoch.checked_sub(magnitude)
                };
                return Some((frame, at.unwrap_or(self.epoch)));
            }
            std::hint::spin_loop();
        }
    }
}

impl Default for FrameClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Wall-clock access for the frame clock only. Clock readings never influence rendered audio:
/// they only place live events, which are inherently non-deterministic input.
trait NowForClock {
    fn now_for_clock() -> Instant;
}

impl NowForClock for Instant {
    #[allow(clippy::disallowed_methods)] // Justification: see trait docs (live input timing only).
    fn now_for_clock() -> Instant {
        Instant::now()
    }
}
