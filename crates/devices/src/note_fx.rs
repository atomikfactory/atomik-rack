//! Shared machinery of the stateless note players (filters, mappers, bypass): the input-key to
//! emitted-notes map, the bypass of a player and a capacity-safe emitter.
//!
//! A stateless player turns every input note into zero or more output notes by a rule that its
//! params control. Params change while keys are held, so the rule at Off time can differ from the
//! rule at On time; the [`NoteRouter`] therefore remembers what each input key emitted and the
//! Off releases exactly that. Two input keys may map to one output key: the output key is
//! reference counted and only closed when its last user goes away.
//!
//! * **Output rule.** An On of a key that already sounds is an `Off` then an `On` at the same
//!   frame (a retrigger) and the count goes up; an `Off` is only emitted when the count returns
//!   to zero.
//! * **Capacity.** [`push_sorted`] never pushes past the buffer and keeps offsets non-decreasing;
//!   an On is refused while less than [`ON_HEADROOM`] slots are free so the Offs that follow
//!   always fit. A refused On is not recorded, so its Off emits nothing.
//! * **Bypass.** [`PlayerBypass`] switches a player to a thru at a batch boundary: what the
//!   device sent is closed, keys held at the switch are ignored until their own Off (no
//!   retrigger), and while bypassed `notes_in` is copied to `notes_out`.
//!
//! Everything is allocation-free and fixed size (HI-1).

use atomik_core::{NoteBuffer, NoteEvent, NoteKind};

/// Keys per map (all MIDI keys).
const KEYS: usize = 128;
/// Output keys one input key can emit.
pub(crate) const MAX_OUT: usize = 8;
/// Free slots an On needs in the output buffer (room for a full set of Offs).
pub(crate) const ON_HEADROOM: usize = KEYS + 1;

/// Pushes `event` unless the buffer is full; the offset is raised to the last event's offset so
/// the order stays non-decreasing. An `On` additionally needs [`ON_HEADROOM`] free slots.
/// Returns whether the event was pushed.
pub(crate) fn push_sorted(out: &mut NoteBuffer, mut event: NoteEvent) -> bool {
    let free = out.capacity().saturating_sub(out.len());
    let needed = if matches!(event.kind, NoteKind::On { .. }) {
        ON_HEADROOM
    } else {
        1
    };
    if free < needed {
        return false;
    }
    if let Some(last) = out.as_slice().last() {
        event.offset = event.offset.max(last.offset);
    }
    out.push(event).is_ok()
}

fn emit_on(out: &mut NoteBuffer, frame: usize, key: u8, velocity: f32) -> bool {
    push_sorted(
        out,
        NoteEvent {
            offset: frame.min(atomik_core::BATCH - 1) as u16,
            kind: NoteKind::On { key, velocity },
        },
    )
}

fn emit_off(out: &mut NoteBuffer, frame: usize, key: u8) -> bool {
    push_sorted(
        out,
        NoteEvent {
            offset: frame.min(atomik_core::BATCH - 1) as u16,
            kind: NoteKind::Off { key },
        },
    )
}

/// The map from input keys to the output keys their On emitted (see the module docs).
#[derive(Debug, Clone)]
pub(crate) struct NoteRouter {
    emitted: [[u8; MAX_OUT]; KEYS],
    counts: [u8; KEYS],
    /// Users of each output key.
    refc: [u8; KEYS],
    /// Input keys that are down.
    held: [bool; KEYS],
    /// Input keys whose events are ignored until their Off.
    swallow: [bool; KEYS],
}

impl NoteRouter {
    pub const fn new() -> Self {
        Self {
            emitted: [[0; MAX_OUT]; KEYS],
            counts: [0; KEYS],
            refc: [0; KEYS],
            held: [false; KEYS],
            swallow: [false; KEYS],
        }
    }

    /// Forgets everything without emitting anything (the device `reset` path).
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// Whether any output key sounds.
    pub fn any_sounding(&self) -> bool {
        self.refc.iter().any(|c| *c > 0)
    }

    // Used by the players that mute or hold (and the tests); the note filter does not need it.
    #[allow(dead_code)]
    /// Whether the input key `in_key` is down.
    pub fn is_held(&self, in_key: u8) -> bool {
        self.held[usize::from(in_key & 0x7F)]
    }

    /// The first output key `in_key` emitted, which the per-key expressions follow.
    pub fn follow(&self, in_key: u8) -> Option<u8> {
        let k = usize::from(in_key & 0x7F);
        (self.counts[k] > 0 && !self.swallow[k]).then(|| self.emitted[k][0])
    }

    /// The input key goes down and emits `outs` (`(key, velocity)`, at most [`MAX_OUT`], keys
    /// deduplicated by the caller or here). An input key that is already down is released first.
    /// Ignored while the key is swallowed.
    pub fn on(&mut self, out: &mut NoteBuffer, frame: usize, in_key: u8, outs: &[(u8, f32)]) {
        let ik = usize::from(in_key & 0x7F);
        if self.swallow[ik] {
            // A fresh press of a swallowed key is a real press again.
            self.swallow[ik] = false;
        } else if self.counts[ik] > 0 {
            self.release(out, frame, ik);
        }
        self.held[ik] = true;
        for (i, (key, velocity)) in outs.iter().enumerate() {
            if i >= MAX_OUT || *key > 127 {
                break;
            }
            let ok = usize::from(*key);
            let n = usize::from(self.counts[ik]);
            if self.emitted[ik][..n].contains(key) {
                continue;
            }
            if self.refc[ok] > 0 {
                // Retrigger: close the sounding note, then start it again.
                if !emit_on_after_off(out, frame, *key, *velocity) {
                    continue;
                }
            } else if !emit_on(out, frame, *key, *velocity) {
                continue;
            }
            self.refc[ok] = self.refc[ok].saturating_add(1);
            self.emitted[ik][n] = *key;
            self.counts[ik] += 1;
        }
    }

    /// The input key goes up: releases exactly what its On emitted, whatever the params are now.
    pub fn off(&mut self, out: &mut NoteBuffer, frame: usize, in_key: u8) {
        let ik = usize::from(in_key & 0x7F);
        self.held[ik] = false;
        if self.swallow[ik] {
            self.swallow[ik] = false;
            return;
        }
        self.release(out, frame, ik);
    }

    fn release(&mut self, out: &mut NoteBuffer, frame: usize, ik: usize) {
        for i in 0..usize::from(self.counts[ik]) {
            let ok = usize::from(self.emitted[ik][i]);
            self.refc[ok] = self.refc[ok].saturating_sub(1);
            if self.refc[ok] == 0 {
                emit_off(out, frame, ok as u8);
            }
        }
        self.counts[ik] = 0;
    }

    /// Emits an Off for every output key that sounds, ascending, and forgets what the input
    /// keys emitted. Input keys stay down: their later Offs emit nothing.
    pub fn close_all(&mut self, out: &mut NoteBuffer, frame: usize) {
        for k in 0..KEYS {
            if self.refc[k] > 0 {
                emit_off(out, frame, k as u8);
            }
        }
        self.refc = [0; KEYS];
        self.counts = [0; KEYS];
    }

    /// Input keys that are down are ignored until their Off (used after a bypass or a mute, so
    /// a key held across the switch is not retriggered).
    #[allow(dead_code)] // for the players that mute; the bypass keeps its own held set
    pub fn swallow_held(&mut self) {
        self.swallow = self.held;
    }

    /// An `AllOff` on the input: closes everything and forgets the held keys.
    pub fn all_off(&mut self, out: &mut NoteBuffer, frame: usize) {
        self.close_all(out, frame);
        self.held = [false; KEYS];
        self.swallow = [false; KEYS];
    }
}

/// An `Off` then an `On` of one key at one frame; false (and nothing sent) when the buffer has
/// no room for both.
fn emit_on_after_off(out: &mut NoteBuffer, frame: usize, key: u8, velocity: f32) -> bool {
    if out.capacity().saturating_sub(out.len()) <= ON_HEADROOM {
        return false;
    }
    emit_off(out, frame, key);
    emit_on(out, frame, key, velocity)
}

/// The bypass of a player (see the module docs).
#[derive(Debug, Clone)]
pub(crate) struct PlayerBypass {
    wanted: bool,
    active: bool,
    held: [bool; KEYS],
    swallow: [bool; KEYS],
    /// Keys whose On went through while bypassed and whose Off has not.
    passed: [bool; KEYS],
}

impl PlayerBypass {
    pub const fn new() -> Self {
        Self {
            wanted: false,
            active: false,
            held: [false; KEYS],
            swallow: [false; KEYS],
            passed: [false; KEYS],
        }
    }

    /// The `bypass` param (takes effect at the next batch start).
    pub fn set_param(&mut self, plain: f32) {
        self.wanted = plain >= 0.5;
    }

    /// Whether the player is bypassed in the current batch.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The device `reset`: forgets the keys. Returns whether a key was passed through (the
    /// device then owes an `AllOff`).
    pub fn reset(&mut self) -> bool {
        let sounding = self.passed.iter().any(|p| *p);
        self.held = [false; KEYS];
        self.swallow = [false; KEYS];
        self.passed = [false; KEYS];
        sounding
    }

    /// Adopts the param at the start of a batch. On a change the keys passed through are closed
    /// at frame 0 and the keys held now are swallowed; returns whether it changed, so the device
    /// closes its own notes and clears its own state.
    pub fn begin_batch(&mut self, out: &mut NoteBuffer) -> bool {
        if self.wanted == self.active {
            return false;
        }
        self.active = self.wanted;
        for k in 0..KEYS {
            if self.passed[k] {
                emit_off(out, 0, k as u8);
            }
        }
        self.passed = [false; KEYS];
        self.swallow = self.held;
        true
    }

    /// Routes one input event. Returns the event for the device to process, or `None` when it
    /// was passed through (bypassed) or swallowed.
    pub fn route(&mut self, out: &mut NoteBuffer, e: NoteEvent) -> Option<NoteEvent> {
        match e.kind {
            NoteKind::On { key, .. } => {
                let k = usize::from(key & 0x7F);
                self.held[k] = true;
                self.swallow[k] = false;
                if self.active {
                    self.passed[k] = push_sorted(out, e);
                    None
                } else {
                    Some(e)
                }
            }
            NoteKind::Off { key } => {
                let k = usize::from(key & 0x7F);
                self.held[k] = false;
                if std::mem::take(&mut self.swallow[k]) {
                    return None;
                }
                if self.active {
                    if std::mem::take(&mut self.passed[k]) {
                        push_sorted(out, e);
                    }
                    None
                } else {
                    Some(e)
                }
            }
            NoteKind::AllOff => {
                self.held = [false; KEYS];
                self.swallow = [false; KEYS];
                if self.active {
                    self.passed = [false; KEYS];
                    push_sorted(out, e);
                    None
                } else {
                    Some(e)
                }
            }
            kind => {
                if let Some(key) = kind.key()
                    && self.swallow[usize::from(key & 0x7F)]
                {
                    return None;
                }
                if self.active {
                    push_sorted(out, e);
                    None
                } else {
                    Some(e)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
