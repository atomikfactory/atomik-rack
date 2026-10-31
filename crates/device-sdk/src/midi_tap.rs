//! [`MidiTap`]: a lock-free queue of timestamped MIDI messages that a device writes on the audio
//! thread and the app's MIDI output thread drains.
//!
//! A device that sends MIDI out of the program (the MIDI Out device) creates one tap in its
//! constructor, returns it from [`Device::midi_tap`](crate::Device::midi_tap) and pushes every
//! message it makes, stamped with the engine host frame the message belongs to
//! ([`atomik_core::TimeInfo::frame`] plus the offset inside the batch). The engine hands the same
//! `Arc` to the app through its meter bank; a thread of the app turns frames into wall-clock
//! times and sends the bytes to a port. The audio thread only does relaxed atomic stores and one
//! release store of the write position per message, so there is no `unsafe`, no lock and no
//! allocation after [`MidiTap::new`].
//!
//! * **One writer, one reader.** `push` is called from one thread at a time (a device runs on one
//!   thread per batch; the hand-over between threads synchronises) and `pop` from one reader.
//! * **Note offs are never dropped silently.** The queue holds [`MIDI_TAP_EVENTS`] messages. When
//!   it is nearly full only the messages that release something (note offs, All Notes Off) are
//!   still accepted; when it is completely full the loss is counted ([`MidiTap::dropped`]) and,
//!   if the lost message released something, remembered ([`MidiTap::take_release_lost`]) so the
//!   reader can silence the port instead of leaving a note stuck.
//! * **Messages are at most three bytes** (a channel message, or a one-byte real-time message such
//!   as a clock tick): everything the app sends from the audio engine fits.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Messages a tap holds (a power of two).
pub const MIDI_TAP_EVENTS: usize = 4096;

/// Slots only messages that release something may use, so a flood of controllers can never
/// crowd out a note off.
const RELEASE_RESERVE: usize = 256;

const _: () = assert!(MIDI_TAP_EVENTS.is_power_of_two() && RELEASE_RESERVE < MIDI_TAP_EVENTS);

/// One MIDI message and the engine host frame it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiEvent {
    /// The engine host frame the message is due at (the [`atomik_core::TimeInfo::frame`]
    /// timeline).
    pub frame: u64,
    len: u8,
    bytes: [u8; 3],
}

impl MidiEvent {
    /// A message of 1 to 3 bytes, or `None` for any other length.
    pub fn new(frame: u64, bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > 3 {
            return None;
        }
        let mut b = [0; 3];
        b[..bytes.len()].copy_from_slice(bytes);
        Some(Self {
            frame,
            len: bytes.len() as u8,
            bytes: b,
        })
    }

    /// The message bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    /// Whether the message releases sound: a note off, a note on with velocity 0, All Notes Off
    /// or All Sound Off. These are the messages a full queue must not lose silently.
    pub fn releases(&self) -> bool {
        releases(self.bytes())
    }
}

/// Whether the bytes are a message that releases sound (see [`MidiEvent::releases`]).
pub fn releases(bytes: &[u8]) -> bool {
    match bytes {
        [status, _, velocity] if status & 0xF0 == 0x90 => *velocity == 0,
        [status, _, _] if status & 0xF0 == 0x80 => true,
        [status, 120 | 123, _] if status & 0xF0 == 0xB0 => true,
        _ => false,
    }
}

#[derive(Debug, Default)]
struct Slot {
    frame: AtomicU64,
    /// `len << 24 | b2 << 16 | b1 << 8 | b0`.
    packed: AtomicU32,
}

/// The queue (see the module docs).
#[derive(Debug)]
pub struct MidiTap {
    slots: Box<[Slot]>,
    /// Messages written since creation (written by the device only).
    head: AtomicU64,
    /// Messages read since creation (written by the reader only).
    tail: AtomicU64,
    dropped: AtomicU64,
    release_lost: AtomicBool,
}

impl Default for MidiTap {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiTap {
    /// An empty tap (allocates; non-RT).
    pub fn new() -> Self {
        Self {
            slots: (0..MIDI_TAP_EVENTS).map(|_| Slot::default()).collect(),
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            release_lost: AtomicBool::new(false),
        }
    }

    /// RT-safe. Queues a message; `false` when the queue had no room for it (counted, see the
    /// module docs). Single writer only.
    pub fn push(&self, event: MidiEvent) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let used = head.wrapping_sub(self.tail.load(Ordering::Acquire)) as usize;
        let release = event.releases();
        let room = if release {
            MIDI_TAP_EVENTS
        } else {
            MIDI_TAP_EVENTS - RELEASE_RESERVE
        };
        if used >= room {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            if release {
                self.release_lost.store(true, Ordering::Release);
            }
            return false;
        }
        let slot = &self.slots[(head as usize) & (MIDI_TAP_EVENTS - 1)];
        let b = event.bytes;
        let packed = (u32::from(event.len) << 24)
            | (u32::from(b[2]) << 16)
            | (u32::from(b[1]) << 8)
            | u32::from(b[0]);
        slot.frame.store(event.frame, Ordering::Relaxed);
        slot.packed.store(packed, Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }

    /// The oldest queued message. Single reader only; never blocks.
    pub fn pop(&self) -> Option<MidiEvent> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let slot = &self.slots[(tail as usize) & (MIDI_TAP_EVENTS - 1)];
        let frame = slot.frame.load(Ordering::Relaxed);
        let packed = slot.packed.load(Ordering::Relaxed);
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(MidiEvent {
            frame,
            len: ((packed >> 24) as u8).clamp(1, 3),
            bytes: [packed as u8, (packed >> 8) as u8, (packed >> 16) as u8],
        })
    }

    /// Messages waiting to be read.
    pub fn len(&self) -> usize {
        self.head
            .load(Ordering::Acquire)
            .wrapping_sub(self.tail.load(Ordering::Acquire)) as usize
    }

    /// Whether no message is waiting.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Messages lost to a full queue since creation.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Whether a message that releases sound was lost since the last call (and forgets it): the
    /// reader then silences the port itself.
    pub fn take_release_lost(&self) -> bool {
        self.release_lost.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(frame: u64, bytes: &[u8]) -> MidiEvent {
        MidiEvent::new(frame, bytes).unwrap()
    }

    #[test]
    fn messages_come_back_in_order_with_their_frames() {
        let tap = MidiTap::new();
        assert!(tap.is_empty() && tap.pop().is_none());
        assert!(tap.push(ev(10, &[0x90, 60, 100])));
        assert!(tap.push(ev(10, &[0xF8])));
        assert!(tap.push(ev(99, &[0xC1, 5])));
        assert_eq!(tap.len(), 3);
        assert_eq!(tap.pop(), Some(ev(10, &[0x90, 60, 100])));
        assert_eq!(tap.pop().unwrap().bytes(), [0xF8]);
        let last = tap.pop().unwrap();
        assert_eq!((last.frame, last.bytes()), (99, &[0xC1, 5][..]));
        assert!(tap.pop().is_none());
    }

    #[test]
    fn messages_of_the_wrong_length_are_refused() {
        assert!(MidiEvent::new(0, &[]).is_none());
        assert!(MidiEvent::new(0, &[1, 2, 3, 4]).is_none());
    }

    #[test]
    fn the_ring_wraps_many_times() {
        let tap = MidiTap::new();
        for i in 0..3 * MIDI_TAP_EVENTS as u64 {
            assert!(tap.push(ev(i, &[0xB0, (i % 128) as u8, 1])));
            let got = tap.pop().unwrap();
            assert_eq!((got.frame, got.bytes()[1]), (i, (i % 128) as u8));
        }
        assert_eq!(tap.dropped(), 0);
    }

    #[test]
    fn what_releases_sound_is_recognised() {
        assert!(ev(0, &[0x80, 60, 0]).releases());
        assert!(
            ev(0, &[0x93, 60, 0]).releases(),
            "a note on with velocity 0"
        );
        assert!(ev(0, &[0xB2, 123, 0]).releases());
        assert!(ev(0, &[0xB2, 120, 0]).releases());
        assert!(!ev(0, &[0x90, 60, 1]).releases());
        assert!(!ev(0, &[0xB0, 7, 100]).releases());
        assert!(!ev(0, &[0xF8]).releases());
    }

    #[test]
    fn a_nearly_full_queue_keeps_its_last_slots_for_releases() {
        let tap = MidiTap::new();
        let open = MIDI_TAP_EVENTS - RELEASE_RESERVE;
        for i in 0..open {
            assert!(tap.push(ev(i as u64, &[0x90, 60, 100])));
        }
        assert!(!tap.push(ev(0, &[0x90, 61, 100])), "a note on is refused");
        assert!(!tap.push(ev(0, &[0xB0, 1, 5])), "so is a controller");
        assert_eq!(tap.dropped(), 2);
        assert!(!tap.take_release_lost(), "nothing that releases was lost");
        for _ in 0..RELEASE_RESERVE {
            assert!(tap.push(ev(0, &[0x80, 60, 0])), "a note off still fits");
        }
        assert!(!tap.push(ev(0, &[0x80, 60, 0])), "now it is full");
        assert!(tap.take_release_lost());
        assert!(!tap.take_release_lost(), "the flag is read once");
        assert_eq!(tap.len(), MIDI_TAP_EVENTS);
    }

    #[test]
    fn a_reader_on_another_thread_sees_every_message_once_in_order() {
        use std::sync::Arc;
        let tap = Arc::new(MidiTap::new());
        let reader = {
            let tap = Arc::clone(&tap);
            std::thread::spawn(move || {
                let mut next = 0u64;
                while next < 50_000 {
                    if let Some(e) = tap.pop() {
                        assert_eq!(e.frame, next);
                        assert_eq!(e.bytes()[1], (next % 128) as u8);
                        next += 1;
                    } else {
                        std::thread::yield_now();
                    }
                }
            })
        };
        let mut i = 0u64;
        while i < 50_000 {
            if tap.push(ev(i, &[0xB0, (i % 128) as u8, 0])) {
                i += 1;
            } else {
                std::thread::yield_now();
            }
        }
        reader.join().unwrap();
    }
}
