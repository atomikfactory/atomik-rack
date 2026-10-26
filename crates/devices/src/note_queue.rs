//! The scheduled-event queue of the time-shifting note players (note repeater, note delay).
//!
//! A time-shifting player keeps notes that are due in the future. They live in a fixed queue of
//! [`QUEUE_CAP`] entries `{frame, kind}`, ordered by frame, then by insertion order, on a
//! device-owned frame clock: the device adds [`atomik_core::BATCH`] to its `now` every batch, so
//! the queue runs the same while the transport plays or is stopped, and a bounce is a pure
//! function of the batch sequence. Frames are absolute on that clock.
//!
//! * **Room for the Off.** An `On` is scheduled only when its `Off` fits: [`NoteQueue::push_pair`]
//!   needs two free slots, and [`NoteQueue::push_on_reserving`] takes two and keeps one reserved
//!   for the `Off` that [`NoteQueue::push_off_reserved`] sends later. A full queue refuses
//!   `On`s and never an `Off`.
//! * **Output.** [`NoteSink`] writes the due entries into the output buffer. It counts the users
//!   of every key (an `On` of a sounding key is an `Off` then an `On` at the same frame, an `Off`
//!   goes out when the count returns to zero) and swallows the `Off` of an `On` the output
//!   buffer had no room for, so nothing hangs and nothing is cut short.
//!
//! Everything is allocation-free and fixed size (HI-1). Times convert to frames with
//! [`round_frames`] (f64, round half up), which only uses `+ - * /` and `floor`.

use atomik_core::{BATCH, NoteBuffer, NoteEvent, NoteKind};

use crate::note_fx::{ON_HEADROOM, push_sorted};

/// Entries the queue holds.
pub(crate) const QUEUE_CAP: usize = 512;

/// A non-negative frame count from a real number of frames, rounding half up.
pub(crate) fn round_frames(frames: f64) -> u64 {
    if frames.is_finite() && frames > 0.0 {
        (frames + 0.5).floor() as u64
    } else {
        0
    }
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    frame: u64,
    kind: NoteKind,
}

/// The queue (see the module docs).
#[derive(Debug, Clone)]
pub(crate) struct NoteQueue {
    /// Descending by frame, newer entries first among equal frames: the entry due next is last.
    entries: [Entry; QUEUE_CAP],
    len: usize,
    /// Slots kept free for the `Off`s of scheduled `On`s that have not been sent yet.
    reserved: usize,
}

impl NoteQueue {
    pub fn new() -> Self {
        Self {
            entries: [Entry {
                frame: 0,
                kind: NoteKind::AllOff,
            }; QUEUE_CAP],
            len: 0,
            reserved: 0,
        }
    }

    /// Forgets every entry and reservation without emitting anything.
    pub fn clear(&mut self) {
        self.len = 0;
        self.reserved = 0;
    }

    /// Whether nothing is scheduled.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Scheduled entries.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Slots a new entry can take (not counting the reserved ones).
    pub fn free(&self) -> usize {
        QUEUE_CAP.saturating_sub(self.len + self.reserved)
    }

    fn insert(&mut self, frame: u64, kind: NoteKind) {
        if self.len >= QUEUE_CAP {
            return;
        }
        // Newer entries sort after older ones of the same frame, which is earlier in a
        // descending table, so the position is the count of strictly later frames.
        let at = self.entries[..self.len].partition_point(|e| e.frame > frame);
        self.entries.copy_within(at..self.len, at + 1);
        self.entries[at] = Entry { frame, kind };
        self.len += 1;
    }

    /// Schedules an `On` and its `Off` together; false (nothing scheduled) when both do not fit.
    /// `off_frame` is raised to `on_frame` when earlier.
    pub fn push_pair(&mut self, on_frame: u64, off_frame: u64, key: u8, velocity: f32) -> bool {
        if self.free() < 2 {
            return false;
        }
        self.insert(on_frame, NoteKind::On { key, velocity });
        self.insert(off_frame.max(on_frame), NoteKind::Off { key });
        true
    }

    /// Schedules an `On` whose `Off` is not known yet and keeps a slot for it; false when
    /// there is no room for both. Pair it with [`Self::push_off_reserved`].
    pub fn push_on_reserving(&mut self, frame: u64, key: u8, velocity: f32) -> bool {
        if self.free() < 2 {
            return false;
        }
        self.insert(frame, NoteKind::On { key, velocity });
        self.reserved += 1;
        true
    }

    /// Schedules the `Off` of an `On` scheduled with [`Self::push_on_reserving`] (always fits).
    pub fn push_off_reserved(&mut self, frame: u64, key: u8) {
        self.reserved = self.reserved.saturating_sub(1);
        self.insert(frame, NoteKind::Off { key });
    }

    /// Schedules any other event; false when the queue is full.
    pub fn push_other(&mut self, frame: u64, kind: NoteKind) -> bool {
        if self.free() < 1 {
            return false;
        }
        self.insert(frame, kind);
        true
    }

    /// Removes and returns the next entry when it is due before `end` (exclusive).
    pub fn pop_due(&mut self, end: u64) -> Option<(u64, NoteKind)> {
        let last = self.len.checked_sub(1)?;
        let e = self.entries[last];
        if e.frame >= end {
            return None;
        }
        self.len = last;
        Some((e.frame, e.kind))
    }
}

/// The output stage (see the module docs).
#[derive(Debug, Clone)]
pub(crate) struct NoteSink {
    users: [u8; 128],
    /// `Off`s to swallow per key (their `On` was refused by the output buffer).
    skip: [u8; 128],
}

impl NoteSink {
    pub const fn new() -> Self {
        Self {
            users: [0; 128],
            skip: [0; 128],
        }
    }

    /// Forgets everything without emitting anything.
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// Whether any key sounds.
    pub fn any_sounding(&self) -> bool {
        self.users.iter().any(|u| *u > 0)
    }

    /// Writes one due event at `offset` in the batch.
    pub fn emit(&mut self, out: &mut NoteBuffer, offset: usize, kind: NoteKind) {
        let offset = offset.min(BATCH - 1) as u16;
        let ev = |kind| NoteEvent { offset, kind };
        match kind {
            NoteKind::On { key, velocity } => {
                let k = usize::from(key & 0x7F);
                if self.users[k] > 0 {
                    // Retrigger: the sounding note ends, the new one starts.
                    if out.capacity().saturating_sub(out.len()) <= ON_HEADROOM {
                        self.skip[k] = self.skip[k].saturating_add(1);
                        return;
                    }
                    push_sorted(out, ev(NoteKind::Off { key }));
                }
                if push_sorted(out, ev(NoteKind::On { key, velocity })) {
                    self.users[k] = self.users[k].saturating_add(1);
                } else {
                    self.skip[k] = self.skip[k].saturating_add(1);
                }
            }
            NoteKind::Off { key } => {
                let k = usize::from(key & 0x7F);
                if self.skip[k] > 0 {
                    self.skip[k] -= 1;
                } else if self.users[k] > 0 {
                    self.users[k] -= 1;
                    if self.users[k] == 0 {
                        push_sorted(out, ev(NoteKind::Off { key }));
                    }
                }
            }
            NoteKind::AllOff => {
                self.close_all(out, usize::from(offset));
                push_sorted(out, ev(NoteKind::AllOff));
            }
            NoteKind::PolyPressure { key, .. } | NoteKind::NoteExpr { key, .. } => {
                // Per-note expression only means something while its note sounds.
                if self.users[usize::from(key & 0x7F)] > 0 {
                    push_sorted(out, ev(kind));
                }
            }
            _ => {
                push_sorted(out, ev(kind));
            }
        }
    }

    /// An `Off` for every key that sounds, ascending; forgets everything.
    pub fn close_all(&mut self, out: &mut NoteBuffer, offset: usize) {
        let offset = offset.min(BATCH - 1) as u16;
        for k in 0..128 {
            if self.users[k] > 0 {
                push_sorted(
                    out,
                    NoteEvent {
                        offset,
                        kind: NoteKind::Off { key: k as u8 },
                    },
                );
            }
        }
        self.clear();
    }

    /// Writes every entry of `queue` due in the batch that starts at frame `now`.
    pub fn drain(&mut self, queue: &mut NoteQueue, out: &mut NoteBuffer, now: u64) {
        let end = now + BATCH as u64;
        while let Some((frame, kind)) = queue.pop_due(end) {
            self.emit(out, frame.saturating_sub(now) as usize, kind);
        }
    }
}

#[cfg(test)]
mod tests;
