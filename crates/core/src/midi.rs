//! MIDI channel messages as the control path sees them (learn, mappings, control surfaces).
//!
//! The note path of the engine uses [`crate::NoteKind`], which drops the channel and scales the
//! values. A mapping needs the channel and the raw 7 or 14 bit values, so the parser of
//! `atomik-midi-io` produces this type for the control path.

/// One MIDI channel message that a mapping or a control surface can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelMessage {
    /// MIDI channel, 0..=15 (channel 1 is 0).
    pub channel: u8,
    /// What the message says.
    pub kind: MessageKind,
}

/// The kinds of channel message that carry a control value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// Note on (velocity 1..=127).
    NoteOn {
        /// Key 0..=127.
        key: u8,
        /// Velocity 1..=127.
        velocity: u8,
    },
    /// Note off (also a note on with velocity 0).
    NoteOff {
        /// Key 0..=127.
        key: u8,
    },
    /// A continuous controller.
    Cc {
        /// Controller number 0..=127.
        number: u8,
        /// Value 0..=127.
        value: u8,
    },
    /// Pitch bend, 14 bit: 0..=16383, 8192 is the centre.
    PitchBend {
        /// The raw 14 bit value.
        value: u16,
    },
}

impl ChannelMessage {
    /// A note on message (a velocity of 0 is a note off, as on the wire).
    pub fn note_on(channel: u8, key: u8, velocity: u8) -> Self {
        let kind = if velocity == 0 {
            MessageKind::NoteOff { key }
        } else {
            MessageKind::NoteOn { key, velocity }
        };
        Self { channel, kind }
    }

    /// A note off message.
    pub fn note_off(channel: u8, key: u8) -> Self {
        Self {
            channel,
            kind: MessageKind::NoteOff { key },
        }
    }

    /// A controller message.
    pub fn cc(channel: u8, number: u8, value: u8) -> Self {
        Self {
            channel,
            kind: MessageKind::Cc { number, value },
        }
    }

    /// A pitch bend message.
    pub fn pitch_bend(channel: u8, value: u16) -> Self {
        Self {
            channel,
            kind: MessageKind::PitchBend { value },
        }
    }

    /// The wire bytes (status and two data bytes, three bytes in all).
    pub fn to_bytes(self) -> [u8; 3] {
        let ch = self.channel & 0x0F;
        match self.kind {
            MessageKind::NoteOn { key, velocity } => [0x90 | ch, key & 0x7F, velocity & 0x7F],
            MessageKind::NoteOff { key } => [0x80 | ch, key & 0x7F, 0],
            MessageKind::Cc { number, value } => [0xB0 | ch, number & 0x7F, value & 0x7F],
            MessageKind::PitchBend { value } => {
                [0xE0 | ch, (value & 0x7F) as u8, ((value >> 7) & 0x7F) as u8]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_serialise_to_their_wire_bytes() {
        assert_eq!(ChannelMessage::cc(2, 7, 100).to_bytes(), [0xB2, 7, 100]);
        assert_eq!(
            ChannelMessage::note_on(0, 60, 90).to_bytes(),
            [0x90, 60, 90]
        );
        assert_eq!(ChannelMessage::note_off(15, 60).to_bytes(), [0x8F, 60, 0]);
        assert_eq!(
            ChannelMessage::pitch_bend(8, 16_383).to_bytes(),
            [0xE8, 0x7F, 0x7F]
        );
        assert_eq!(
            ChannelMessage::pitch_bend(0, 8192).to_bytes(),
            [0xE0, 0, 0x40]
        );
    }

    #[test]
    fn a_note_on_with_velocity_zero_is_a_note_off() {
        assert_eq!(
            ChannelMessage::note_on(1, 64, 0),
            ChannelMessage::note_off(1, 64)
        );
    }
}
