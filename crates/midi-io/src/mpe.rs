//! Live MPE input: one note per MIDI channel, turned into per-note expression events.
//!
//! An [`MpeInput`] belongs to one input port. It keeps the port's zones ([`MpeZones`]: which
//! channels are masters and which are members), set by hand when the input is opened and changed
//! by an MPE Configuration Message (RPN 6 on a master channel) when the controller sends one, and
//! the small state of each channel. [`MpeInput::feed`] takes the bytes of one message and returns
//! the note events it means:
//!
//! * A **master channel** and a channel outside the zones behave as they always did: the
//!   channel messages are meant for everything ([`crate::kind_of`]).
//! * On a **member channel** the note on and note off are ordinary notes, remembered by channel.
//!   The pitch bend, the channel pressure and CC 74 on that channel are the expression of the
//!   note sounding there: [`NoteKind::NoteExpr`] with the note's key, tuning in semitones (the
//!   bend times the channel's range, 48 semitones until RPN 0 on that channel says otherwise),
//!   pressure `0..=1` and slide `0..=1`. A controller sends the bend just before the note on, so
//!   what a channel holds is sent right after the note on of the next note there.
//! * The registered parameter messages of a member channel (the bend range, RPN 0) are the
//!   input's own and are not passed on. An MPE Configuration Message is taken and not passed on
//!   either; a zone it names replaces the one set by hand (and one with 0 member channels
//!   removes it).
//!
//! A message gives at most [`MAX_EVENTS`] events, so nothing here allocates.

use crate::{RunningStatus, frame, kind_of};
use atomik_core::mpe::MEMBER_BEND_RANGE;
use atomik_core::{ChannelRole, ExprKind, MpeZones, NoteKind};

/// The most events one message gives (a note on and the three expressions waiting on its channel).
pub const MAX_EVENTS: usize = 4;

/// The events of one message, in order.
#[derive(Debug, Clone, Copy)]
pub struct Events {
    items: [NoteKind; MAX_EVENTS],
    len: usize,
}

impl Events {
    const fn none() -> Self {
        Self {
            items: [NoteKind::AllOff; MAX_EVENTS],
            len: 0,
        }
    }

    fn push(&mut self, kind: NoteKind) {
        if self.len < MAX_EVENTS {
            self.items[self.len] = kind;
            self.len += 1;
        }
    }

    /// The one event `kind`.
    pub fn one(kind: NoteKind) -> Self {
        let mut e = Self::none();
        e.push(kind);
        e
    }

    /// The events in order.
    pub fn as_slice(&self) -> &[NoteKind] {
        &self.items[..self.len]
    }

    /// True when the message gave no event.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// What the input knows about one channel.
#[derive(Debug, Clone, Copy)]
struct Channel {
    /// The key of the note sounding on the channel (member channels).
    key: Option<u8>,
    /// Pitch bend, `-1..=1`.
    bend: f32,
    /// Channel pressure, `0..=1`.
    pressure: f32,
    /// CC 74, `0..=1`; `None` until the controller sent it.
    slide: Option<f32>,
    /// Pitch-bend range in semitones.
    range: f32,
    /// The registered parameter the select controllers point at (101, 100); 127 127 is none.
    rpn: [u8; 2],
    /// Data entry of RPN 0: semitones and cents.
    semitones: u8,
    cents: u8,
}

impl Channel {
    const fn new() -> Self {
        Self {
            key: None,
            bend: 0.0,
            pressure: 0.0,
            slide: None,
            range: MEMBER_BEND_RANGE,
            rpn: [127, 127],
            semitones: MEMBER_BEND_RANGE as u8,
            cents: 0,
        }
    }
}

/// The MPE state of one input port (module docs).
#[derive(Debug, Clone)]
pub struct MpeInput {
    zones: MpeZones,
    state: RunningStatus,
    channels: [Channel; 16],
}

impl MpeInput {
    /// An input with `zones` until the controller announces others.
    pub fn new(zones: MpeZones) -> Self {
        Self {
            zones,
            state: RunningStatus::default(),
            channels: [Channel::new(); 16],
        }
    }

    /// The zones in force: the ones set by hand, or the last the controller announced.
    pub fn zones(&self) -> MpeZones {
        self.zones
    }

    /// Takes one message and returns the note events it means (none for a message the note path
    /// does not use or one the input keeps for itself).
    pub fn feed(&mut self, bytes: &[u8]) -> Events {
        let mut out = Events::none();
        let Some((status, d0, d1)) = frame(bytes, &mut self.state) else {
            return out;
        };
        let channel = status & 0x0F;
        let role = self.zones.role(channel);
        let member = matches!(role, ChannelRole::Member(_));
        // The registered parameter messages: the zones and the range of a member channel.
        if status & 0xF0 == 0xB0 && self.controller(channel, member, d0, d1) {
            return out;
        }
        if !member {
            if let Some(kind) = kind_of(status, d0, d1) {
                out.push(kind);
            }
            return out;
        }
        let c = usize::from(channel);
        match status & 0xF0 {
            0x90 if d1 > 0 => {
                let ch = &mut self.channels[c];
                ch.key = Some(d0);
                out.push(NoteKind::On {
                    key: d0,
                    velocity: f32::from(d1) / 127.0,
                });
                // What the channel holds starts the note: the bend a controller sends first.
                let ch = self.channels[c];
                if ch.bend != 0.0 {
                    out.push(tuning(d0, &ch));
                }
                if ch.pressure != 0.0 {
                    out.push(note_expr(d0, ExprKind::Pressure, ch.pressure));
                }
                if let Some(slide) = ch.slide.filter(|s| *s != 0.0) {
                    out.push(note_expr(d0, ExprKind::Slide, slide));
                }
            }
            0x80 | 0x90 => {
                let ch = &mut self.channels[c];
                if ch.key == Some(d0) {
                    ch.key = None;
                }
                out.push(NoteKind::Off { key: d0 });
            }
            0xE0 => {
                let raw = i32::from(d0) | (i32::from(d1) << 7);
                let ch = &mut self.channels[c];
                ch.bend = ((raw - 8192) as f32 / 8192.0).clamp(-1.0, 1.0);
                if let Some(key) = ch.key {
                    out.push(tuning(key, ch));
                }
            }
            0xD0 => {
                let ch = &mut self.channels[c];
                ch.pressure = f32::from(d0) / 127.0;
                if let Some(key) = ch.key {
                    out.push(note_expr(key, ExprKind::Pressure, ch.pressure));
                }
            }
            0xB0 if d0 == 74 => {
                let ch = &mut self.channels[c];
                let slide = f32::from(d1) / 127.0;
                ch.slide = Some(slide);
                if let Some(key) = ch.key {
                    out.push(note_expr(key, ExprKind::Slide, slide));
                }
            }
            _ => {
                if let Some(kind) = kind_of(status, d0, d1) {
                    out.push(kind);
                }
            }
        }
        out
    }

    /// Handles the registered-parameter controllers of `channel`; true when the message is the
    /// input's own and goes no further.
    fn controller(&mut self, channel: u8, member: bool, number: u8, value: u8) -> bool {
        let ch = &mut self.channels[usize::from(channel)];
        match number {
            101 => ch.rpn[0] = value,
            100 => ch.rpn[1] = value,
            98 | 99 => ch.rpn = [127, 127],
            6 if ch.rpn == [0, 6] => {
                // The MPE Configuration Message: the data entry is the member count. On a
                // channel that is no master it changes nothing, and it is still not a note.
                self.zones.config(channel, value);
                return true;
            }
            6 if member && ch.rpn == [0, 0] => {
                ch.semitones = value.min(96);
                ch.range = f32::from(ch.semitones) + f32::from(ch.cents) * 0.01;
                return true;
            }
            38 if member && ch.rpn == [0, 0] => {
                ch.cents = value.min(99);
                ch.range = f32::from(ch.semitones) + f32::from(ch.cents) * 0.01;
                return true;
            }
            _ => {}
        }
        // A member channel keeps every registered-parameter controller to itself.
        member && matches!(number, 101 | 100 | 98 | 99 | 6 | 38)
    }
}

fn note_expr(key: u8, kind: ExprKind, value: f32) -> NoteKind {
    NoteKind::NoteExpr { key, kind, value }
}

fn tuning(key: u8, ch: &Channel) -> NoteKind {
    note_expr(key, ExprKind::Tuning, ch.bend * ch.range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::ZoneSide;

    fn lower(members: u8) -> MpeInput {
        MpeInput::new(MpeZones::one(ZoneSide::Lower, members))
    }

    fn feed(m: &mut MpeInput, bytes: &[u8]) -> Vec<NoteKind> {
        m.feed(bytes).as_slice().to_vec()
    }

    fn tune(key: u8, v: f32) -> NoteKind {
        note_expr(key, ExprKind::Tuning, v)
    }

    #[test]
    fn a_member_channel_note_carries_its_own_bend_pressure_and_slide() {
        let mut m = lower(4);
        let on = feed(&mut m, &[0x91, 60, 100]);
        assert_eq!(
            on,
            [NoteKind::On {
                key: 60,
                velocity: 100.0 / 127.0
            }]
        );
        // Full bend up is 48 semitones on a member channel.
        assert_eq!(
            feed(&mut m, &[0xE1, 0x7F, 0x7F])[0],
            tune(60, 8191.0 / 8192.0 * 48.0)
        );
        assert_eq!(feed(&mut m, &[0xE1, 0x00, 0x40]), [tune(60, 0.0)]);
        assert_eq!(
            feed(&mut m, &[0xD1, 127]),
            [note_expr(60, ExprKind::Pressure, 1.0)]
        );
        assert_eq!(
            feed(&mut m, &[0xB1, 74, 127]),
            [note_expr(60, ExprKind::Slide, 1.0)]
        );
    }

    #[test]
    fn two_notes_on_two_channels_do_not_share_their_expression() {
        let mut m = lower(4);
        feed(&mut m, &[0x91, 60, 90]);
        feed(&mut m, &[0x92, 64, 90]);
        assert_eq!(
            feed(&mut m, &[0xD2, 64]),
            [note_expr(64, ExprKind::Pressure, 64.0 / 127.0)]
        );
        assert_eq!(
            feed(&mut m, &[0xD1, 32]),
            [note_expr(60, ExprKind::Pressure, 32.0 / 127.0)]
        );
    }

    #[test]
    fn what_a_channel_holds_starts_the_next_note_there() {
        let mut m = lower(4);
        // The controller sets the bend, the pressure and the slide before the note on.
        assert!(feed(&mut m, &[0xE3, 0x00, 0x60]).is_empty());
        assert!(feed(&mut m, &[0xD3, 127]).is_empty());
        assert!(feed(&mut m, &[0xB3, 74, 127]).is_empty());
        let on = feed(&mut m, &[0x93, 67, 100]);
        assert_eq!(on.len(), 4);
        assert!(matches!(on[0], NoteKind::On { key: 67, .. }));
        assert_eq!(on[1], tune(67, 0.5 * 48.0));
        assert_eq!(on[2], note_expr(67, ExprKind::Pressure, 1.0));
        assert_eq!(on[3], note_expr(67, ExprKind::Slide, 1.0));
    }

    #[test]
    fn a_note_off_ends_the_note_and_later_bends_belong_to_nobody() {
        let mut m = lower(4);
        feed(&mut m, &[0x91, 60, 90]);
        assert_eq!(feed(&mut m, &[0x81, 60, 0]), [NoteKind::Off { key: 60 }]);
        assert!(feed(&mut m, &[0xE1, 0x00, 0x50]).is_empty());
        // A note on with velocity 0 is a note off as well.
        feed(&mut m, &[0x92, 62, 90]);
        assert_eq!(feed(&mut m, &[0x92, 62, 0]), [NoteKind::Off { key: 62 }]);
    }

    #[test]
    fn the_master_channel_and_channels_outside_the_zone_keep_the_plain_meaning() {
        let mut m = lower(2);
        // Channel 1 is the master: a bend is the whole zone's.
        assert_eq!(
            feed(&mut m, &[0xE0, 0x00, 0x60]),
            [NoteKind::PitchBend { value: 0.5 }]
        );
        // Channel 9 is outside the zone.
        assert_eq!(
            feed(&mut m, &[0xE8, 0x00, 0x60]),
            [NoteKind::PitchBend { value: 0.5 }]
        );
        assert!(matches!(
            feed(&mut m, &[0x98, 50, 100])[0],
            NoteKind::On { key: 50, .. }
        ));
        assert_eq!(
            feed(&mut m, &[0xB0, 1, 127]),
            [NoteKind::Cc {
                controller: 1,
                value: 1.0
            }]
        );
        assert_eq!(
            feed(&mut m, &[0xD0, 127]),
            [NoteKind::ChannelPressure { value: 1.0 }]
        );
    }

    #[test]
    fn a_configuration_message_sets_the_zone() {
        let mut m = MpeInput::new(MpeZones::default());
        // Without a zone every channel is plain.
        assert_eq!(
            feed(&mut m, &[0xE1, 0x00, 0x60]),
            [NoteKind::PitchBend { value: 0.5 }]
        );
        // RPN 6 on channel 1, data entry 7.
        assert!(feed(&mut m, &[0xB0, 101, 0]).len() == 1);
        assert!(feed(&mut m, &[0xB0, 100, 6]).len() == 1);
        assert!(
            feed(&mut m, &[0xB0, 6, 7]).is_empty(),
            "the message is taken"
        );
        assert_eq!(m.zones().lower, Some(7));
        assert_eq!(m.zones().upper, None);
        feed(&mut m, &[0x91, 60, 90]);
        assert_eq!(feed(&mut m, &[0xE1, 0x00, 0x60])[0], tune(60, 24.0));
        // The upper zone comes from channel 16, and 0 members remove a zone.
        feed(&mut m, &[0xBF, 101, 0]);
        feed(&mut m, &[0xBF, 100, 6]);
        feed(&mut m, &[0xBF, 6, 4]);
        assert_eq!((m.zones().lower, m.zones().upper), (Some(7), Some(4)));
        feed(&mut m, &[0xB0, 101, 0]);
        feed(&mut m, &[0xB0, 100, 6]);
        feed(&mut m, &[0xB0, 6, 0]);
        assert_eq!((m.zones().lower, m.zones().upper), (None, Some(4)));
    }

    #[test]
    fn the_configuration_message_on_a_member_channel_is_not_a_zone() {
        let mut m = lower(4);
        feed(&mut m, &[0xB2, 101, 0]);
        feed(&mut m, &[0xB2, 100, 6]);
        assert!(feed(&mut m, &[0xB2, 6, 2]).is_empty());
        assert_eq!(m.zones().lower, Some(4));
    }

    #[test]
    fn the_bend_range_of_a_member_channel_follows_rpn_zero() {
        let mut m = lower(4);
        feed(&mut m, &[0xB1, 101, 0]);
        feed(&mut m, &[0xB1, 100, 0]);
        assert!(feed(&mut m, &[0xB1, 6, 12]).is_empty(), "kept by the input");
        assert!(feed(&mut m, &[0xB1, 38, 50]).is_empty());
        feed(&mut m, &[0x91, 60, 90]);
        assert_eq!(feed(&mut m, &[0xE1, 0x00, 0x60])[0], tune(60, 0.5 * 12.5));
        // Another channel keeps 48.
        feed(&mut m, &[0x92, 62, 90]);
        assert_eq!(feed(&mut m, &[0xE2, 0x00, 0x60])[0], tune(62, 24.0));
    }

    #[test]
    fn running_status_and_malformed_bytes_are_handled() {
        let mut m = lower(4);
        feed(&mut m, &[0x91, 60, 90]);
        // Running status: the data bytes alone repeat the last status (a note on).
        assert!(matches!(
            feed(&mut m, &[61, 90])[0],
            NoteKind::On { key: 61, .. }
        ));
        assert!(feed(&mut m, &[0xF8]).is_empty());
        assert!(feed(&mut m, &[]).is_empty());
        assert!(feed(&mut m, &[0xE1]).is_empty());
    }

    #[test]
    fn an_upper_zone_uses_the_channels_from_the_top() {
        let mut m = MpeInput::new(MpeZones::one(ZoneSide::Upper, 2));
        feed(&mut m, &[0x9E, 60, 90]); // channel 15, the first member
        assert_eq!(
            feed(&mut m, &[0xDE, 127])[0],
            note_expr(60, ExprKind::Pressure, 1.0)
        );
        // Channel 2 is outside the upper zone: plain.
        assert_eq!(
            feed(&mut m, &[0xD1, 127]),
            [NoteKind::ChannelPressure { value: 1.0 }]
        );
    }
}
