//! MPE zones: which MIDI channels carry one note each.
//!
//! A controller that plays MPE (per-note expression) gives every sounding note a MIDI channel of
//! its own, so the pitch bend, the channel pressure and the CC 74 on that channel belong to that
//! note alone. The 16 channels are split into at most two zones. The *lower zone* has its master
//! channel on channel 1 (index 0) and its member channels on 2, 3, ... (indexes 1, 2, ...); the
//! *upper zone* has its master on channel 16 (index 15) and its members on 15, 14, ... (indexes
//! 14, 13, ...). The master channel carries what is meant for the whole zone. A controller
//! announces its zones with the MPE Configuration Message (RPN 6 on the master channel, the data
//! entry being the number of member channels, 0 switching the zone off); see [`MpeZones::config`].
//!
//! This module is the pure part: the zone layout and the check of the configuration message.
//! The conversion of live MIDI into per-note expression is in `atomik-midi-io`.

/// The most member channels one zone has.
pub const MAX_MEMBERS: u8 = 15;
/// The most member channels two zones have together (each also has a master channel).
pub const MAX_MEMBERS_BOTH: u8 = 14;
/// The pitch-bend range of a member channel in semitones until the controller says otherwise
/// (MPE says 48; the master channel keeps the usual 2).
pub const MEMBER_BEND_RANGE: f32 = 48.0;

/// Which end of the channels a zone sits at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ZoneSide {
    /// Master on channel 1, members on 2, 3, ...
    #[default]
    Lower,
    /// Master on channel 16, members on 15, 14, ...
    Upper,
}

impl ZoneSide {
    /// The master channel index (0 or 15).
    pub const fn master(self) -> u8 {
        match self {
            ZoneSide::Lower => 0,
            ZoneSide::Upper => 15,
        }
    }

    /// The other side.
    pub const fn other(self) -> Self {
        match self {
            ZoneSide::Lower => ZoneSide::Upper,
            ZoneSide::Upper => ZoneSide::Lower,
        }
    }
}

/// What a MIDI channel is in the zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRole {
    /// The master channel of a zone: messages on it are meant for every note of the zone.
    Master(ZoneSide),
    /// A member channel of a zone: one note at a time, its expression is the note's own.
    Member(ZoneSide),
    /// Not part of a zone: an ordinary channel.
    Outside,
}

/// The zones of an MPE input: the number of member channels of each (`None` = no such zone).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MpeZones {
    /// Members of the lower zone (`1..=15`).
    pub lower: Option<u8>,
    /// Members of the upper zone (`1..=15`).
    pub upper: Option<u8>,
}

impl MpeZones {
    /// Zones with `members` member channels on `side` and no other zone (`members` is held to
    /// `1..=15`; 0 gives no zone).
    pub fn one(side: ZoneSide, members: u8) -> Self {
        let mut z = Self::default();
        z.set(side, members);
        z
    }

    /// The number of member channels of `side`.
    pub fn members(&self, side: ZoneSide) -> Option<u8> {
        match side {
            ZoneSide::Lower => self.lower,
            ZoneSide::Upper => self.upper,
        }
    }

    /// True when no zone is set.
    pub fn is_empty(&self) -> bool {
        self.lower.is_none() && self.upper.is_none()
    }

    /// Sets `side` to `members` member channels (0 removes it; more than 15 are held to 15).
    /// A zone that would overlap the other one shrinks the other (the new one wins), as the MPE
    /// specification asks: two zones keep at most [`MAX_MEMBERS_BOTH`] members between them, and a
    /// zone left without a member is gone.
    pub fn set(&mut self, side: ZoneSide, members: u8) {
        let members = members.min(MAX_MEMBERS);
        let this = (members > 0).then_some(members);
        let other = self.members(side.other());
        let other = match (this, other) {
            (Some(n), Some(o)) if n + o > MAX_MEMBERS_BOTH => {
                MAX_MEMBERS_BOTH.checked_sub(n).filter(|r| *r > 0)
            }
            (_, o) => o,
        };
        match side {
            ZoneSide::Lower => {
                self.lower = this;
                self.upper = other;
            }
            ZoneSide::Upper => {
                self.upper = this;
                self.lower = other;
            }
        }
    }

    /// What `channel` (0..=15) is in these zones.
    pub fn role(&self, channel: u8) -> ChannelRole {
        let channel = channel & 0x0F;
        for side in [ZoneSide::Lower, ZoneSide::Upper] {
            let Some(n) = self.members(side) else {
                continue;
            };
            let master = side.master();
            if channel == master {
                return ChannelRole::Master(side);
            }
            let distance = match side {
                ZoneSide::Lower => channel.checked_sub(1),
                ZoneSide::Upper => 14u8.checked_sub(channel),
            };
            if distance.is_some_and(|d| d < n) {
                return ChannelRole::Member(side);
            }
        }
        ChannelRole::Outside
    }

    /// The member channels of `side`, as indexes (nearest the master first).
    pub fn member_channels(&self, side: ZoneSide) -> impl Iterator<Item = u8> {
        let n = self.members(side).unwrap_or(0);
        (0..n).map(move |i| match side {
            ZoneSide::Lower => 1 + i,
            ZoneSide::Upper => 14 - i,
        })
    }

    /// Takes one MPE Configuration Message: RPN 6 on `channel` with `members` as its data
    /// entry. Only a master channel (index 0 or 15) is a zone's manager: the message sets that
    /// zone (see [`MpeZones::set`]) and returns `true`; on any other channel it is not a zone
    /// message and nothing changes (`false`).
    pub fn config(&mut self, channel: u8, members: u8) -> bool {
        let side = match channel & 0x0F {
            0 => ZoneSide::Lower,
            15 => ZoneSide::Upper,
            _ => return false,
        };
        self.set(side, members);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lower_zone_has_its_master_on_channel_one_and_members_after_it() {
        let z = MpeZones::one(ZoneSide::Lower, 3);
        assert_eq!(z.role(0), ChannelRole::Master(ZoneSide::Lower));
        for c in 1..=3 {
            assert_eq!(z.role(c), ChannelRole::Member(ZoneSide::Lower), "{c}");
        }
        assert_eq!(z.role(4), ChannelRole::Outside);
        assert_eq!(z.role(15), ChannelRole::Outside);
        assert_eq!(
            z.member_channels(ZoneSide::Lower).collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }

    #[test]
    fn an_upper_zone_counts_down_from_channel_sixteen() {
        let z = MpeZones::one(ZoneSide::Upper, 2);
        assert_eq!(z.role(15), ChannelRole::Master(ZoneSide::Upper));
        assert_eq!(z.role(14), ChannelRole::Member(ZoneSide::Upper));
        assert_eq!(z.role(13), ChannelRole::Member(ZoneSide::Upper));
        assert_eq!(z.role(12), ChannelRole::Outside);
        assert_eq!(z.role(0), ChannelRole::Outside);
        assert_eq!(
            z.member_channels(ZoneSide::Upper).collect::<Vec<_>>(),
            [14, 13]
        );
    }

    #[test]
    fn a_zone_of_fifteen_members_uses_every_other_channel() {
        let z = MpeZones::one(ZoneSide::Lower, 15);
        assert_eq!(z.role(15), ChannelRole::Member(ZoneSide::Lower));
        assert_eq!(z.lower, Some(15));
        assert_eq!(MpeZones::one(ZoneSide::Lower, 99).lower, Some(15));
        assert!(MpeZones::one(ZoneSide::Upper, 0).is_empty());
    }

    #[test]
    fn two_zones_share_fourteen_members_and_the_newer_one_wins() {
        let mut z = MpeZones::one(ZoneSide::Lower, 7);
        z.set(ZoneSide::Upper, 7);
        assert_eq!((z.lower, z.upper), (Some(7), Some(7)));
        assert_eq!(z.role(7), ChannelRole::Member(ZoneSide::Lower));
        assert_eq!(z.role(8), ChannelRole::Member(ZoneSide::Upper));
        assert_eq!(z.role(9), ChannelRole::Member(ZoneSide::Upper));
        // A bigger lower zone takes channels from the upper one.
        z.set(ZoneSide::Lower, 10);
        assert_eq!((z.lower, z.upper), (Some(10), Some(4)));
        // One that leaves nothing removes it.
        z.set(ZoneSide::Lower, 14);
        assert_eq!((z.lower, z.upper), (Some(14), None));
    }

    #[test]
    fn the_configuration_message_only_counts_on_a_master_channel() {
        let mut z = MpeZones::default();
        assert!(!z.config(5, 3));
        assert!(z.is_empty());
        assert!(z.config(0, 7));
        assert!(z.config(15, 4));
        assert_eq!((z.lower, z.upper), (Some(7), Some(4)));
        assert!(z.config(0, 0), "zero switches the zone off");
        assert_eq!((z.lower, z.upper), (None, Some(4)));
    }
}
