//! The note-shaping stage of the note filter: fixed note length, a polyphony limit and hold.
//!
//! It sits after the per-key router, so every `On` it sees is for a key that does not sound
//! downstream or is a retrigger (the router writes the closing `Off` first), and every `Off`
//! belongs to the last `On` of its key. With all three features off the stage forwards every event
//! unchanged.
//!
//! * **Length.** A note with a fixed length ignores the `Off` the router sends and ends at its own
//!   frame on the device-owned frame clock (`BATCH` frames per batch, like the time-shifting
//!   players), so a bounce is a pure function of the batch sequence.
//! * **Polyphony.** At most `limit` keys sound; the next `On` either closes the oldest sounding
//!   key (`steal`) or is dropped together with its `Off` (`ignore`).
//! * **Hold.** While hold is on an `Off` only latches the key; turning hold off, an `AllOff` or a
//!   reset releases every latched key. Pressing a latched key again closes it first.
//!
//! The table is one slot per key (128), so it cannot overflow; an `On` the output buffer has no room
//! for is dropped with its `Off`. Everything is fixed size and allocation-free.

use atomik_core::{BATCH, NoteBuffer, NoteEvent, NoteKind};

use crate::note_fx::push_sorted;
use crate::note_queue::round_frames;

const KEYS: usize = 128;
/// No timed end.
const NO_END: u64 = u64::MAX;

/// What happens to a note that would exceed the polyphony limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Overflow {
    /// The oldest sounding note is closed.
    Steal,
    /// The new note is dropped.
    Ignore,
}

/// The stage (see the module docs).
#[derive(Debug, Clone)]
pub(crate) struct NoteShape {
    /// Frame clock: the first frame of the current batch.
    now: u64,
    on: [bool; KEYS],
    /// Start order of the sounding notes (larger = newer).
    age: [u64; KEYS],
    next_age: u64,
    latched: [bool; KEYS],
    /// End frame of a timed note.
    end: [u64; KEYS],
    /// The `Off` of a dropped `On` is to be swallowed.
    dropped: [bool; KEYS],
    sounding: usize,
    // Params.
    pub length_frames: Option<u64>,
    pub limit: usize,
    pub overflow: Overflow,
    pub hold: bool,
}

impl NoteShape {
    pub const fn new() -> Self {
        Self {
            now: 0,
            on: [false; KEYS],
            age: [0; KEYS],
            next_age: 0,
            latched: [false; KEYS],
            end: [NO_END; KEYS],
            dropped: [false; KEYS],
            sounding: 0,
            length_frames: None,
            limit: 0,
            overflow: Overflow::Steal,
            hold: false,
        }
    }

    /// Forgets everything (the device `reset` path); the frame clock keeps running.
    pub fn clear(&mut self) {
        let now = self.now;
        *self = Self {
            length_frames: self.length_frames,
            limit: self.limit,
            overflow: self.overflow,
            hold: self.hold,
            ..Self::new()
        };
        self.now = now;
    }

    /// Whether a note sounds downstream.
    pub fn any_sounding(&self) -> bool {
        self.sounding > 0
    }

    /// A length in milliseconds as frames (`None` = the note's own length).
    pub fn set_length_ms(&mut self, ms: Option<f32>, sample_rate: f64) {
        self.length_frames = ms.map(|ms| round_frames(f64::from(ms) * sample_rate / 1000.0).max(1));
    }

    fn put(out: &mut NoteBuffer, offset: usize, kind: NoteKind) -> bool {
        push_sorted(
            out,
            NoteEvent {
                offset: offset.min(BATCH - 1) as u16,
                kind,
            },
        )
    }

    fn off(&mut self, out: &mut NoteBuffer, offset: usize, k: usize) {
        if self.on[k] {
            self.on[k] = false;
            self.latched[k] = false;
            self.end[k] = NO_END;
            self.sounding -= 1;
            Self::put(out, offset, NoteKind::Off { key: k as u8 });
        }
    }

    /// Closes every sounding key, ascending, and forgets the state.
    pub fn close_all(&mut self, out: &mut NoteBuffer, offset: usize) {
        for k in 0..KEYS {
            self.off(out, offset, k);
        }
        self.dropped = [false; KEYS];
    }

    /// Releases the keys only the hold keeps.
    pub fn release_latched(&mut self, out: &mut NoteBuffer, offset: usize) {
        for k in 0..KEYS {
            if self.latched[k] {
                self.off(out, offset, k);
            }
        }
    }

    /// Closes the timed notes that end before `limit` (an absolute frame), in end order.
    fn expire(&mut self, out: &mut NoteBuffer, limit: u64) {
        loop {
            let mut best: Option<usize> = None;
            for k in 0..KEYS {
                if self.on[k]
                    && self.end[k] < limit
                    && best.is_none_or(|b| self.end[k] < self.end[b])
                {
                    best = Some(k);
                }
            }
            let Some(k) = best else { return };
            let at = self.end[k].saturating_sub(self.now) as usize;
            self.off(out, at, k);
        }
    }

    /// Ends the batch: closes the notes whose length ran out in it and advances the clock.
    pub fn end_batch(&mut self, out: &mut NoteBuffer) {
        self.expire(out, self.now + BATCH as u64);
        self.now += BATCH as u64;
    }

    fn oldest(&self) -> Option<usize> {
        (0..KEYS)
            .filter(|k| self.on[*k])
            .min_by_key(|k| self.age[*k])
    }

    /// One event of the router, in order.
    pub fn event(&mut self, out: &mut NoteBuffer, e: NoteEvent) {
        let at = usize::from(e.offset);
        self.expire(out, self.now + at as u64);
        match e.kind {
            NoteKind::On { key, .. } => {
                let k = usize::from(key & 0x7F);
                if self.limit > 0 && !self.on[k] && self.sounding >= self.limit {
                    match self.overflow {
                        Overflow::Steal => {
                            if let Some(o) = self.oldest() {
                                self.off(out, at, o);
                            }
                        }
                        Overflow::Ignore => {
                            self.dropped[k] = true;
                            return;
                        }
                    }
                }
                // A retrigger (or a latched key pressed again) closes the old note first.
                self.off(out, at, k);
                if Self::put(out, at, e.kind) {
                    self.on[k] = true;
                    self.sounding += 1;
                    self.next_age += 1;
                    self.age[k] = self.next_age;
                    self.dropped[k] = false;
                    self.end[k] = match self.length_frames {
                        Some(n) => self.now + at as u64 + n,
                        None => NO_END,
                    };
                } else {
                    self.dropped[k] = true;
                }
            }
            NoteKind::Off { key } => {
                let k = usize::from(key & 0x7F);
                if self.dropped[k] {
                    self.dropped[k] = false;
                } else if self.on[k] && self.end[k] == NO_END {
                    if self.hold {
                        self.latched[k] = true;
                    } else {
                        self.off(out, at, k);
                    }
                }
            }
            NoteKind::AllOff => {
                self.close_all(out, at);
                Self::put(out, at, NoteKind::AllOff);
            }
            NoteKind::PolyPressure { key, .. } | NoteKind::NoteExpr { key, .. } => {
                if self.on[usize::from(key & 0x7F)] {
                    Self::put(out, at, e.kind);
                }
            }
            _ => {
                Self::put(out, at, e.kind);
            }
        }
    }
}
