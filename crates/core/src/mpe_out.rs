//! MPE output: notes on member channels with their expression as bends, pressure and CC 74.
//!
//! An [`MpeEncoder`] is the sending side of [`crate::mpe`]. It owns one zone (a side and a number
//! of member channels) and turns the note events of a device into MIDI messages: every note goes
//! to a member channel of its own, and the note's per-note expression becomes the channel's pitch
//! bend (the offset in semitones over the member range of 48), channel pressure and CC 74 (the
//! slide). The master channel is left to the messages that mean the whole zone.
//!
//! * **Allocation.** A note takes a free member channel: a channel never used yet first (nearest
//!   the master first), then the one released longest ago, so a channel rests as long as it can
//!   before another note reuses it and a release tail is not cut. With every member sounding, the
//!   oldest note is released and its channel taken.
//! * **A fresh note starts clean.** What the channel last held (a bend, pressure or slide of the
//!   note before) is reset before the note on, or set to the new note's first values, so the note
//!   never starts with the last one's expression. A message that repeats what the channel
//!   already holds is not sent.
//! * **Zone.** [`mcm`] gives the MPE Configuration Message that announces the zone.
//!
//! Nothing here allocates: every call fills a fixed-size [`Out`] (or hands each note-off to a
//! closure), so a device can call it on the audio thread.

use crate::mpe::{MAX_MEMBERS, MEMBER_BEND_RANGE, ZoneSide};
use crate::note::{ExprKind, NoteEvent, NoteKind};

/// The most messages one call of the encoder gives.
pub const MAX_OUT: usize = 8;
/// The pitch-bend value of a centred wheel.
const CENTER: u16 = 8192;
/// The timbre controller of MPE (CC 74).
const SLIDE_CC: u8 = 74;

/// One short MIDI message (at most three bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message {
    bytes: [u8; 3],
    len: u8,
}

impl Message {
    const fn new(bytes: [u8; 3], len: u8) -> Self {
        Self { bytes, len }
    }

    /// The bytes of the message.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

/// The messages of one call, in the order to send them.
#[derive(Debug, Clone, Copy)]
pub struct Out {
    items: [Message; MAX_OUT],
    len: usize,
}

impl Default for Out {
    fn default() -> Self {
        Self::new()
    }
}

impl Out {
    /// An empty list.
    pub const fn new() -> Self {
        Self {
            items: [Message::new([0; 3], 0); MAX_OUT],
            len: 0,
        }
    }

    fn push(&mut self, bytes: [u8; 3], len: u8) {
        if self.len < MAX_OUT {
            self.items[self.len] = Message::new(bytes, len);
            self.len += 1;
        }
    }

    /// The messages in order.
    pub fn as_slice(&self) -> &[Message] {
        &self.items[..self.len]
    }

    /// True when there is no message.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// The MPE Configuration Message of a zone with `members` member channels (0 switches it off):
/// RPN 6 on the zone's master channel, then the registered parameter is closed (RPN null), as five
/// controller messages.
pub fn mcm(side: ZoneSide, members: u8) -> [[u8; 3]; 5] {
    let status = 0xB0 | side.master();
    [
        [status, 101, 0],
        [status, 100, 6],
        [status, 6, members.min(MAX_MEMBERS)],
        [status, 101, 127],
        [status, 100, 127],
    ]
}

/// The 14-bit pitch-bend value for `semitones` over a bend range of `range` semitones (held to
/// the ends of the wheel; a value that is not finite is the centre).
pub fn bend_value(semitones: f32, range: f32) -> u16 {
    if !semitones.is_finite() || range <= 0.0 {
        return CENTER;
    }
    let v = (semitones / range).clamp(-1.0, 1.0);
    ((v * 8192.0).round() as i32 + 8192).clamp(0, 16_383) as u16
}

/// A `0..=1` value as a 7-bit data byte (not finite is 0).
fn unit_7bit(value: f32) -> u8 {
    if value.is_finite() {
        (value * 127.0).round().clamp(0.0, 127.0) as u8
    } else {
        0
    }
}

/// What a channel holds, and which note is on it.
#[derive(Debug, Clone, Copy)]
struct Slot {
    /// The input key of the note sounding here.
    key: Option<u8>,
    /// The key the note on went out with.
    out_key: u8,
    /// When the note started and when the channel was last released (0: never).
    started: u64,
    released: u64,
    bend: u16,
    pressure: u8,
    slide: u8,
}

impl Slot {
    const FREE: Slot = Slot {
        key: None,
        out_key: 0,
        started: 0,
        released: 0,
        bend: CENTER,
        pressure: 0,
        slide: 0,
    };
}

/// The first values of a note's expression, sent before its note on (`None`: the channel is
/// reset instead).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Initial {
    /// Pitch offset in semitones.
    pub tuning: Option<f32>,
    /// Pressure `0..=1`.
    pub pressure: Option<f32>,
    /// Slide `0..=1`.
    pub slide: Option<f32>,
}

impl Initial {
    /// The first expression of the note on `key` that comes with its note on: the expression
    /// events of that key after it at the same frame offset (`rest` is what follows the note on
    /// in the batch). Sent before the note on, it lets the note start with its bend.
    pub fn of(key: u8, offset: u16, rest: &[NoteEvent]) -> Self {
        let mut initial = Self::default();
        for e in rest.iter().take_while(|e| e.offset == offset) {
            if let NoteKind::NoteExpr {
                key: k,
                kind,
                value,
            } = e.kind
                && k == key
            {
                match kind {
                    ExprKind::Tuning => initial.tuning = Some(value),
                    ExprKind::Pressure => initial.pressure = Some(value),
                    ExprKind::Slide => initial.slide = Some(value),
                    ExprKind::Volume | ExprKind::Pan => {}
                }
            }
        }
        initial
    }
}

/// The sending side of one MPE zone (module docs).
#[derive(Debug, Clone)]
pub struct MpeEncoder {
    side: ZoneSide,
    members: u8,
    slots: [Slot; MAX_MEMBERS as usize],
    clock: u64,
}

impl MpeEncoder {
    /// An encoder for a zone on `side` with `members` member channels (held to `1..=15`).
    pub fn new(side: ZoneSide, members: u8) -> Self {
        Self {
            side,
            members: members.clamp(1, MAX_MEMBERS),
            slots: [Slot::FREE; MAX_MEMBERS as usize],
            clock: 0,
        }
    }

    /// The side of the zone.
    pub fn side(&self) -> ZoneSide {
        self.side
    }

    /// The number of member channels.
    pub fn members(&self) -> u8 {
        self.members
    }

    /// The master channel (an index, 0 or 15).
    pub fn master(&self) -> u8 {
        self.side.master()
    }

    /// The channel index of member `i` (`0..members`, nearest the master first).
    fn channel(&self, i: usize) -> u8 {
        match self.side {
            ZoneSide::Lower => 1 + i as u8,
            ZoneSide::Upper => 14 - i as u8,
        }
    }

    fn slot_of(&self, key: u8) -> Option<usize> {
        self.slots[..usize::from(self.members)]
            .iter()
            .position(|s| s.key == Some(key))
    }

    /// The member channel the note of input `key` sounds on.
    pub fn channel_of(&self, key: u8) -> Option<u8> {
        self.slot_of(key).map(|i| self.channel(i))
    }

    /// The number of notes sounding.
    pub fn sounding(&self) -> usize {
        self.slots[..usize::from(self.members)]
            .iter()
            .filter(|s| s.key.is_some())
            .count()
    }

    /// Starts the note of input `key`, sent as `out_key` with `velocity` (a 7-bit value) on a
    /// member channel, after the messages that reset the channel or set `initial` (module docs).
    /// A note that is already sounding on `key` is released first. Returns the channel.
    pub fn note_on(
        &mut self,
        key: u8,
        out_key: u8,
        velocity: u8,
        initial: Initial,
        out: &mut Out,
    ) -> u8 {
        self.note_off(key, out);
        self.clock += 1;
        let n = usize::from(self.members);
        let free = self.slots[..n]
            .iter()
            .enumerate()
            .filter(|(_, s)| s.key.is_none())
            .min_by_key(|(i, s)| (s.released, *i))
            .map(|(i, _)| i);
        let i = match free {
            Some(i) => i,
            None => {
                // Every member sounds: the oldest note makes room.
                let i = self.slots[..n]
                    .iter()
                    .enumerate()
                    .min_by_key(|(i, s)| (s.started, *i))
                    .map_or(0, |(i, _)| i);
                let channel = self.channel(i);
                let s = &mut self.slots[i];
                out.push([0x80 | channel, s.out_key, 0], 3);
                s.key = None;
                s.released = self.clock;
                i
            }
        };
        let channel = self.channel(i);
        let slot = &mut self.slots[i];
        let bend = initial
            .tuning
            .map_or(CENTER, |t| bend_value(t, MEMBER_BEND_RANGE));
        if bend != slot.bend {
            slot.bend = bend;
            out.push([0xE0 | channel, (bend & 0x7F) as u8, (bend >> 7) as u8], 3);
        }
        let pressure = initial.pressure.map_or(0, unit_7bit);
        if pressure != slot.pressure {
            slot.pressure = pressure;
            out.push([0xD0 | channel, pressure, 0], 2);
        }
        let slide = initial.slide.map_or(0, unit_7bit);
        if slide != slot.slide {
            slot.slide = slide;
            out.push([0xB0 | channel, SLIDE_CC, slide], 3);
        }
        slot.key = Some(key);
        slot.out_key = out_key;
        slot.started = self.clock;
        out.push([0x90 | channel, out_key, velocity.max(1)], 3);
        channel
    }

    /// Releases the note of input `key`; the note off goes on its channel. Returns the channel,
    /// or `None` when no note sounds on `key`.
    pub fn note_off(&mut self, key: u8, out: &mut Out) -> Option<u8> {
        let i = self.slot_of(key)?;
        self.clock += 1;
        let channel = self.channel(i);
        let slot = &mut self.slots[i];
        out.push([0x80 | channel, slot.out_key, 0], 3);
        slot.key = None;
        slot.released = self.clock;
        Some(channel)
    }

    /// Releases every note: `each` gets the channel and the key of each note off to send.
    pub fn release_all(&mut self, mut each: impl FnMut(u8, u8)) {
        for i in 0..usize::from(self.members) {
            if let Some(_key) = self.slots[i].key {
                self.clock += 1;
                let channel = self.channel(i);
                let slot = &mut self.slots[i];
                each(channel, slot.out_key);
                slot.key = None;
                slot.released = self.clock;
            }
        }
    }

    /// The pitch of the note on `key` is `semitones` away from its key: a pitch bend on its
    /// channel (nothing when no note sounds there or the channel already holds the value).
    pub fn tuning(&mut self, key: u8, semitones: f32, out: &mut Out) {
        let Some(i) = self.slot_of(key) else {
            return;
        };
        let channel = self.channel(i);
        let bend = bend_value(semitones, MEMBER_BEND_RANGE);
        let slot = &mut self.slots[i];
        if bend != slot.bend {
            slot.bend = bend;
            out.push([0xE0 | channel, (bend & 0x7F) as u8, (bend >> 7) as u8], 3);
        }
    }

    /// The pressure of the note on `key`: channel pressure on its channel.
    pub fn pressure(&mut self, key: u8, value: f32, out: &mut Out) {
        let Some(i) = self.slot_of(key) else {
            return;
        };
        let channel = self.channel(i);
        let v = unit_7bit(value);
        let slot = &mut self.slots[i];
        if v != slot.pressure {
            slot.pressure = v;
            out.push([0xD0 | channel, v, 0], 2);
        }
    }

    /// The slide of the note on `key`: CC 74 on its channel.
    pub fn slide(&mut self, key: u8, value: f32, out: &mut Out) {
        let Some(i) = self.slot_of(key) else {
            return;
        };
        let channel = self.channel(i);
        let v = unit_7bit(value);
        let slot = &mut self.slots[i];
        if v != slot.slide {
            slot.slide = v;
            out.push([0xB0 | channel, SLIDE_CC, v], 3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(out: &Out) -> Vec<Vec<u8>> {
        out.as_slice().iter().map(|m| m.bytes().to_vec()).collect()
    }

    fn lower(members: u8) -> MpeEncoder {
        MpeEncoder::new(ZoneSide::Lower, members)
    }

    fn on(e: &mut MpeEncoder, key: u8) -> (u8, Vec<Vec<u8>>) {
        let mut out = Out::new();
        let c = e.note_on(key, key, 100, Initial::default(), &mut out);
        (c, bytes(&out))
    }

    #[test]
    fn the_configuration_message_is_rpn_6_on_the_master_channel() {
        assert_eq!(
            mcm(ZoneSide::Lower, 15),
            [
                [0xB0, 101, 0],
                [0xB0, 100, 6],
                [0xB0, 6, 15],
                [0xB0, 101, 127],
                [0xB0, 100, 127]
            ]
        );
        let up = mcm(ZoneSide::Upper, 7);
        assert!(up.iter().all(|m| m[0] == 0xBF));
        assert_eq!(up[2], [0xBF, 6, 7]);
        assert_eq!(
            mcm(ZoneSide::Lower, 99)[2][2],
            15,
            "held to fifteen members"
        );
        assert_eq!(
            mcm(ZoneSide::Lower, 0)[2][2],
            0,
            "zero switches the zone off"
        );
    }

    #[test]
    fn the_bend_is_the_offset_over_the_member_range() {
        assert_eq!(bend_value(0.0, 48.0), 8192);
        assert_eq!(bend_value(48.0, 48.0), 16383);
        assert_eq!(bend_value(-48.0, 48.0), 0);
        assert_eq!(bend_value(24.0, 48.0), 12288);
        assert_eq!(bend_value(500.0, 48.0), 16383, "held to the wheel");
        assert_eq!(bend_value(f32::NAN, 48.0), 8192);
        assert_eq!(bend_value(1.0, 0.0), 8192);
    }

    #[test]
    fn notes_take_member_channels_in_turn_and_never_the_master() {
        let mut e = lower(3);
        assert_eq!(on(&mut e, 60), (1, vec![vec![0x91, 60, 100]]));
        assert_eq!(on(&mut e, 64).0, 2);
        assert_eq!(on(&mut e, 67).0, 3);
        assert_eq!(e.sounding(), 3);
        assert_eq!(e.channel_of(64), Some(2));
        assert_eq!(e.channel_of(61), None);
    }

    #[test]
    fn an_upper_zone_counts_down_from_channel_fifteen() {
        let mut e = MpeEncoder::new(ZoneSide::Upper, 2);
        assert_eq!(e.master(), 15);
        assert_eq!(on(&mut e, 60).0, 14);
        assert_eq!(on(&mut e, 62).0, 13);
    }

    #[test]
    fn the_channel_released_longest_ago_is_reused_first() {
        let mut e = lower(3);
        for k in [60, 62, 64] {
            on(&mut e, k);
        }
        let mut out = Out::new();
        // Released in the order 62 (channel 2), 60 (channel 1), 64 (channel 3).
        for k in [62, 60, 64] {
            e.note_off(k, &mut out);
        }
        assert_eq!(on(&mut e, 70).0, 2, "62 was released first");
        assert_eq!(on(&mut e, 71).0, 1);
        assert_eq!(on(&mut e, 72).0, 3);
    }

    #[test]
    fn a_channel_never_used_comes_before_a_released_one() {
        let mut e = lower(3);
        on(&mut e, 60);
        let mut out = Out::new();
        e.note_off(60, &mut out);
        assert_eq!(on(&mut e, 62).0, 2, "channel 1 was used, 2 never was");
    }

    #[test]
    fn a_note_off_goes_on_the_notes_channel_with_the_key_it_started_on() {
        let mut e = lower(4);
        let mut out = Out::new();
        e.note_on(60, 67, 90, Initial::default(), &mut out);
        let mut off = Out::new();
        assert_eq!(e.note_off(60, &mut off), Some(1));
        assert_eq!(bytes(&off), vec![vec![0x81, 67, 0]]);
        assert_eq!(e.note_off(60, &mut off), None, "already released");
        assert_eq!(off.as_slice().len(), 1);
    }

    #[test]
    fn with_every_member_sounding_the_oldest_note_makes_room() {
        let mut e = lower(2);
        on(&mut e, 60);
        on(&mut e, 62);
        let (c, sent) = on(&mut e, 64);
        assert_eq!(c, 1);
        assert_eq!(sent, vec![vec![0x81, 60, 0], vec![0x91, 64, 100]]);
        assert_eq!(e.channel_of(60), None);
        assert_eq!(e.channel_of(64), Some(1));
    }

    #[test]
    fn a_retriggered_key_releases_its_note_first() {
        let mut e = lower(4);
        on(&mut e, 60);
        let (c, sent) = on(&mut e, 60);
        assert_eq!(sent[0], vec![0x81, 60, 0]);
        assert_eq!(*sent.last().unwrap(), vec![0x90 | c, 60, 100]);
        assert_eq!(e.sounding(), 1);
    }

    #[test]
    fn a_pitch_curve_is_a_bend_on_the_member_channel_and_a_repeat_is_not_sent() {
        let mut e = lower(4);
        on(&mut e, 60);
        let mut out = Out::new();
        e.tuning(60, 12.0, &mut out);
        // 12 / 48 of the wheel: 8192 + 2048 = 10240 = 0x2800.
        assert_eq!(bytes(&out), vec![vec![0xE1, 0x00, 0x50]]);
        let mut again = Out::new();
        e.tuning(60, 12.0, &mut again);
        assert!(again.is_empty(), "the channel already holds it");
        let mut none = Out::new();
        e.tuning(61, 3.0, &mut none);
        assert!(none.is_empty(), "no note sounds on 61");
    }

    #[test]
    fn pressure_and_slide_are_channel_pressure_and_cc_74() {
        let mut e = lower(4);
        on(&mut e, 60);
        on(&mut e, 62);
        let mut out = Out::new();
        e.pressure(62, 1.0, &mut out);
        e.slide(62, 0.5, &mut out);
        e.pressure(60, 0.25, &mut out);
        assert_eq!(
            bytes(&out),
            vec![vec![0xD2, 127], vec![0xB2, 74, 64], vec![0xD1, 32]]
        );
    }

    #[test]
    fn a_new_note_on_a_used_channel_starts_clean() {
        let mut e = lower(1);
        on(&mut e, 60);
        let mut out = Out::new();
        e.tuning(60, 5.0, &mut out);
        e.pressure(60, 0.8, &mut out);
        e.slide(60, 0.3, &mut out);
        e.note_off(60, &mut out);
        let (c, sent) = on(&mut e, 62);
        assert_eq!(c, 1);
        assert_eq!(
            sent,
            vec![
                vec![0xE1, 0x00, 0x40],
                vec![0xD1, 0],
                vec![0xB1, 74, 0],
                vec![0x91, 62, 100]
            ],
            "bend centred, pressure and slide zeroed, then the note"
        );
        // A channel that is already clean gets no resets.
        let mut e = lower(1);
        assert_eq!(on(&mut e, 60).1.len(), 1);
    }

    #[test]
    fn the_first_expression_of_a_note_goes_out_before_its_note_on() {
        let mut e = lower(2);
        let mut out = Out::new();
        e.note_on(
            60,
            60,
            100,
            Initial {
                tuning: Some(-48.0),
                pressure: Some(1.0),
                slide: Some(1.0),
            },
            &mut out,
        );
        assert_eq!(
            bytes(&out),
            vec![
                vec![0xE1, 0, 0],
                vec![0xD1, 127],
                vec![0xB1, 74, 127],
                vec![0x91, 60, 100]
            ]
        );
        // The same values arriving as events afterwards add nothing.
        let mut after = Out::new();
        e.tuning(60, -48.0, &mut after);
        e.pressure(60, 1.0, &mut after);
        e.slide(60, 1.0, &mut after);
        assert!(after.is_empty());
    }

    #[test]
    fn release_all_gives_every_note_off() {
        let mut e = lower(3);
        for k in [60, 62] {
            on(&mut e, k);
        }
        let mut got = Vec::new();
        e.release_all(|c, k| got.push((c, k)));
        assert_eq!(got, [(1, 60), (2, 62)]);
        assert_eq!(e.sounding(), 0);
        e.release_all(|_, _| panic!("nothing left"));
    }

    #[test]
    fn a_note_on_with_velocity_zero_is_sent_as_one() {
        let mut e = lower(1);
        let mut out = Out::new();
        e.note_on(60, 60, 0, Initial::default(), &mut out);
        assert_eq!(bytes(&out), vec![vec![0x91, 60, 1]]);
    }
}
