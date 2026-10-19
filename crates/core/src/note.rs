//! Note events and the fixed-capacity, allocation-free note buffer.

/// What a note event does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoteKind {
    /// Start a note. `velocity` is in (0, 1].
    On {
        /// MIDI key number 0..=127.
        key: u8,
        /// Velocity in (0, 1].
        velocity: f32,
    },
    /// Release a note.
    Off {
        /// MIDI key number 0..=127.
        key: u8,
    },
    /// Release every sounding note.
    AllOff,
    /// Channel pitch bend: `value` in -1..=1, 0 is centre.
    PitchBend {
        /// Bend amount, -1 (full down) to 1 (full up).
        value: f32,
    },
    /// A MIDI continuous controller: `controller` 0..=127, `value` in 0..=1.
    Cc {
        /// Controller number 0..=127.
        controller: u8,
        /// Controller value in 0..=1.
        value: f32,
    },
    /// Channel (mono) aftertouch, `value` in 0..=1.
    ChannelPressure {
        /// Pressure in 0..=1.
        value: f32,
    },
    /// Polyphonic key pressure on one key, `value` in 0..=1.
    PolyPressure {
        /// MIDI key number 0..=127.
        key: u8,
        /// Pressure in 0..=1.
        value: f32,
    },
    /// A per-note expression value for the sounding note on `key`.
    NoteExpr {
        /// MIDI key number 0..=127.
        key: u8,
        /// Which expression.
        kind: ExprKind,
        /// The value: tuning in semitones, the others 0..=1 (pan -1..=1).
        value: f32,
    },
}

/// The kind of a per-note expression ([`NoteKind::NoteExpr`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExprKind {
    /// Pitch offset of the note in semitones.
    Tuning,
    /// Per-note pressure.
    Pressure,
    /// Per-note slide (the MPE third dimension).
    Slide,
    /// Per-note volume.
    Volume,
    /// Per-note pan, -1..=1.
    Pan,
}

impl NoteKind {
    /// True for the messages that address the whole channel (no key): pitch bend, CC and channel
    /// pressure. `AllOff` is not one (it has its own rules everywhere).
    pub fn is_channel_message(&self) -> bool {
        matches!(
            self,
            NoteKind::PitchBend { .. } | NoteKind::Cc { .. } | NoteKind::ChannelPressure { .. }
        )
    }

    /// The key a message addresses, for `On`, `Off`, `PolyPressure` and `NoteExpr`.
    pub fn key(&self) -> Option<u8> {
        match *self {
            NoteKind::On { key, .. }
            | NoteKind::Off { key }
            | NoteKind::PolyPressure { key, .. }
            | NoteKind::NoteExpr { key, .. } => Some(key),
            _ => None,
        }
    }
}

/// A note event inside one batch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteEvent {
    /// Frame offset inside the batch, `< BATCH`.
    pub offset: u16,
    /// The event.
    pub kind: NoteKind,
}

/// Returned when pushing into a full [`NoteBuffer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("note buffer is full")]
pub struct BufferFull;

/// Fixed-capacity note event list. Allocates only in [`NoteBuffer::with_capacity`]; never afterwards.
#[derive(Debug, Clone)]
pub struct NoteBuffer {
    events: Vec<NoteEvent>,
    capacity: usize,
}

impl NoteBuffer {
    /// Creates an empty buffer able to hold `capacity` events.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            events: Vec::with_capacity(capacity),
            capacity,
        }
    }

    /// Appends an event. Events must be pushed in non-decreasing `offset` order.
    /// Returns [`BufferFull`] (and leaves the buffer unchanged) when at capacity. Never allocates.
    pub fn push(&mut self, event: NoteEvent) -> Result<(), BufferFull> {
        if self.events.len() >= self.capacity {
            return Err(BufferFull);
        }
        debug_assert!(
            self.events
                .last()
                .is_none_or(|last| last.offset <= event.offset),
            "note events must be pushed in non-decreasing offset order"
        );
        self.events.push(event);
        Ok(())
    }

    /// Removes every event, keeping the capacity.
    pub fn clear(&mut self) {
        self.events.clear();
    }

    /// The events in push order.
    pub fn as_slice(&self) -> &[NoteEvent] {
        &self.events
    }

    /// Number of events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// True when there are no events.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Maximum number of events.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(offset: u16) -> NoteEvent {
        NoteEvent {
            offset,
            kind: NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        }
    }

    #[test]
    fn notebuffer_push_beyond_capacity_returns_full() {
        let mut b = NoteBuffer::with_capacity(2);
        assert_eq!(b.push(on(0)), Ok(()));
        assert_eq!(b.push(on(1)), Ok(()));
        assert_eq!(b.push(on(2)), Err(BufferFull));
        assert_eq!(b.len(), 2);
        assert_eq!(b.as_slice(), &[on(0), on(1)]);
    }

    #[test]
    fn notebuffer_clear_keeps_capacity() {
        let mut b = NoteBuffer::with_capacity(3);
        b.push(on(5)).unwrap();
        b.clear();
        assert!(b.is_empty());
        assert_eq!(b.capacity(), 3);
        for i in 0..3 {
            b.push(on(i)).unwrap();
        }
        assert_eq!(b.len(), 3);
    }

    #[test]
    fn new_kinds_travel_through_the_buffer_unchanged() {
        let kinds = [
            NoteKind::PitchBend { value: -0.5 },
            NoteKind::Cc {
                controller: 1,
                value: 0.25,
            },
            NoteKind::ChannelPressure { value: 0.75 },
            NoteKind::PolyPressure {
                key: 60,
                value: 0.5,
            },
            NoteKind::NoteExpr {
                key: 61,
                kind: ExprKind::Tuning,
                value: -2.0,
            },
        ];
        let mut b = NoteBuffer::with_capacity(8);
        for (i, kind) in kinds.iter().enumerate() {
            b.push(NoteEvent {
                offset: i as u16,
                kind: *kind,
            })
            .unwrap();
        }
        let got: Vec<NoteKind> = b.as_slice().iter().map(|e| e.kind).collect();
        assert_eq!(got, kinds);
    }

    #[test]
    fn channel_messages_and_keys_are_classified() {
        assert!(NoteKind::PitchBend { value: 0.0 }.is_channel_message());
        assert!(
            NoteKind::Cc {
                controller: 64,
                value: 1.0
            }
            .is_channel_message()
        );
        assert!(NoteKind::ChannelPressure { value: 0.1 }.is_channel_message());
        assert!(!NoteKind::AllOff.is_channel_message());
        assert!(!NoteKind::Off { key: 3 }.is_channel_message());
        assert!(
            !NoteKind::NoteExpr {
                key: 3,
                kind: ExprKind::Pan,
                value: 0.0
            }
            .is_channel_message()
        );
        assert_eq!(NoteKind::Off { key: 3 }.key(), Some(3));
        assert_eq!(NoteKind::PolyPressure { key: 9, value: 0.0 }.key(), Some(9));
        assert_eq!(
            NoteKind::NoteExpr {
                key: 7,
                kind: ExprKind::Slide,
                value: 0.0
            }
            .key(),
            Some(7)
        );
        assert_eq!(NoteKind::PitchBend { value: 0.0 }.key(), None);
        assert_eq!(NoteKind::AllOff.key(), None);
    }

    #[test]
    fn notebuffer_zero_capacity_is_always_full() {
        let mut b = NoteBuffer::with_capacity(0);
        assert_eq!(b.push(on(0)), Err(BufferFull));
    }
}
