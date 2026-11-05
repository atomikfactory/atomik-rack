//! Registered and non-registered parameter numbers (RPN and NRPN): the 14 bit controls that a
//! hardware surface sends as a short run of controller messages.
//!
//! A control is selected with two controllers (NRPN: 99 for the high half of the number and 98
//! for the low half; RPN: 101 and 100), then moved with data entry (controller 6 for the high
//! half of the 14 bit value, 38 for the low half) or stepped with data increment (96) and data
//! decrement (97). A [`ParamParser`] follows one port and one channel and turns that run into
//! [`ParamEvent`]s; a [`ParamBank`] keeps one parser per port and channel so that two surfaces
//! (or two channels of one) never mix their selections. Everything here is pure: bytes in,
//! events out, no clock and no allocation per message.
//!
//! A sender that never sends the low half of the data (a 7 bit surface) still reaches the whole
//! range: until a low half has been seen the high half is spread over 14 bits, so 127 is the top.
//!
//! The controllers of the run are ordinary controllers too: a mapping on one of them still hears
//! it. [`is_param_controller`] says which controller numbers are part of the machinery (the
//! learner never learns the selecting ones as a control, and a consuming mapping keeps all of
//! them away from the notes).

use atomik_core::{ChannelMessage, MessageKind};
use std::collections::BTreeMap;

/// The largest parameter number (14 bit).
pub const MAX_PARAM_NUMBER: u16 = 16_383;
/// The data value at which a button reads as pressed (the middle of the 14 bit range).
pub const PARAM_BUTTON_THRESHOLD: u16 = 8_192;
/// Most ports and channels a bank follows at once (a bank that grows past it starts over).
pub const MAX_BANK_ENTRIES: usize = 1_024;

const DATA_ENTRY_MSB: u8 = 6;
const DATA_ENTRY_LSB: u8 = 38;
const DATA_INCREMENT: u8 = 96;
const DATA_DECREMENT: u8 = 97;
const RPN_LSB: u8 = 100;
const RPN_MSB: u8 = 101;
const NRPN_LSB: u8 = 98;
const NRPN_MSB: u8 = 99;

/// Which family of numbers a control is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ParamSpace {
    /// A non-registered parameter number.
    Nrpn,
    /// A registered parameter number.
    Rpn,
}

/// What a data message did to the selected number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamChange {
    /// The data entry now reads this 14 bit value.
    Value(u16),
    /// Data increment (positive) or decrement (negative) by this many steps, never 0.
    Step(i16),
}

/// One move of a parameter control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamEvent {
    /// The family.
    pub space: ParamSpace,
    /// The parameter number, 0..=16383.
    pub number: u16,
    /// What happened to it.
    pub change: ParamChange,
}

impl ParamEvent {
    /// The position of a value event as a fraction, 0..=1 (`None` for a step).
    pub fn level(&self) -> Option<f32> {
        match self.change {
            ParamChange::Value(v) => Some(f32::from(v.min(MAX_PARAM_NUMBER)) / 16_383.0),
            ParamChange::Step(_) => None,
        }
    }

    /// A short text: `NRPN 1234 = 8192`, `RPN 0 +1`.
    pub fn describe(&self) -> String {
        let family = match self.space {
            ParamSpace::Nrpn => "NRPN",
            ParamSpace::Rpn => "RPN",
        };
        match self.change {
            ParamChange::Value(v) => format!("{family} {} = {v}", self.number),
            ParamChange::Step(d) => format!("{family} {} {d:+}", self.number),
        }
    }
}

/// True for the controller numbers that make up a parameter run: data entry, increment,
/// decrement and the four selecting controllers.
pub fn is_param_controller(number: u8) -> bool {
    matches!(
        number,
        DATA_ENTRY_MSB | DATA_ENTRY_LSB | DATA_INCREMENT | DATA_DECREMENT | NRPN_LSB..=RPN_MSB
    )
}

/// True for the four controllers that only select a number (never a control by themselves).
pub fn is_param_selector(number: u8) -> bool {
    matches!(number, NRPN_LSB..=RPN_MSB)
}

/// Follows the controllers of one port and one channel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParamParser {
    nrpn: Half,
    rpn: Half,
    /// The family the last selecting controller was in.
    space: Option<ParamSpace>,
    data_msb: u8,
    data_lsb: u8,
    /// A low half of the data has been seen since the number was selected.
    lsb_seen: bool,
}

/// The two halves of a selected number.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Half {
    msb: Option<u8>,
    lsb: Option<u8>,
}

impl Half {
    fn number(self) -> Option<u16> {
        Some((u16::from(self.msb?) << 7) | u16::from(self.lsb?))
    }
}

impl ParamParser {
    /// A parser with nothing selected.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number data messages would address now, or `None` (nothing selected, or the null
    /// RPN 16383 that deselects).
    pub fn selected(&self) -> Option<(ParamSpace, u16)> {
        let space = self.space?;
        let number = match space {
            ParamSpace::Nrpn => self.nrpn.number()?,
            ParamSpace::Rpn => {
                let n = self.rpn.number()?;
                if n == MAX_PARAM_NUMBER {
                    return None;
                }
                n
            }
        };
        Some((space, number))
    }

    /// Takes one message of the parser's port and channel; the event it completes, if any.
    pub fn feed(&mut self, msg: &ChannelMessage) -> Option<ParamEvent> {
        let MessageKind::Cc { number, value } = msg.kind else {
            return None;
        };
        let value = value & 0x7F;
        match number {
            NRPN_MSB | NRPN_LSB | RPN_MSB | RPN_LSB => {
                self.select(number, value);
                None
            }
            DATA_ENTRY_MSB => {
                self.data_msb = value;
                self.data_event()
            }
            DATA_ENTRY_LSB => {
                self.data_lsb = value;
                self.lsb_seen = true;
                self.data_event()
            }
            DATA_INCREMENT | DATA_DECREMENT => {
                let (space, selected) = self.selected()?;
                let amount = i16::from(value.max(1));
                Some(ParamEvent {
                    space,
                    number: selected,
                    change: ParamChange::Step(if number == DATA_DECREMENT {
                        -amount
                    } else {
                        amount
                    }),
                })
            }
            _ => None,
        }
    }

    fn select(&mut self, controller: u8, value: u8) {
        let (half, space) = match controller {
            NRPN_MSB | NRPN_LSB => (&mut self.nrpn, ParamSpace::Nrpn),
            _ => (&mut self.rpn, ParamSpace::Rpn),
        };
        if matches!(controller, NRPN_MSB | RPN_MSB) {
            half.msb = Some(value);
        } else {
            half.lsb = Some(value);
        }
        self.space = Some(space);
        // A new number starts with a clean data entry.
        self.data_msb = 0;
        self.data_lsb = 0;
        self.lsb_seen = false;
    }

    fn data_event(&self) -> Option<ParamEvent> {
        let (space, number) = self.selected()?;
        let raw = if self.lsb_seen {
            (u16::from(self.data_msb) << 7) | u16::from(self.data_lsb)
        } else {
            // A 7 bit sender: spread the high half so that 127 reaches the top.
            (u16::from(self.data_msb) << 7) | u16::from(self.data_msb)
        };
        Some(ParamEvent {
            space,
            number,
            change: ParamChange::Value(raw),
        })
    }
}

/// One parser per port and channel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParamBank {
    parsers: BTreeMap<(String, u8), ParamParser>,
}

impl ParamBank {
    /// An empty bank.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets every selection.
    pub fn reset(&mut self) {
        self.parsers.clear();
    }

    /// Takes a message of `port`; the event it completes, if any. Messages that are not part of
    /// a parameter run cost one comparison.
    pub fn feed(&mut self, port: &str, msg: &ChannelMessage) -> Option<ParamEvent> {
        let MessageKind::Cc { number, .. } = msg.kind else {
            return None;
        };
        if !is_param_controller(number) {
            return None;
        }
        let key = (port.to_owned(), msg.channel);
        if !self.parsers.contains_key(&key) && self.parsers.len() >= MAX_BANK_ENTRIES {
            self.parsers.clear();
        }
        self.parsers.entry(key).or_default().feed(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(ch: u8, n: u8, v: u8) -> ChannelMessage {
        ChannelMessage::cc(ch, n, v)
    }

    fn run(p: &mut ParamParser, msgs: &[(u8, u8)]) -> Vec<ParamEvent> {
        msgs.iter()
            .filter_map(|&(n, v)| p.feed(&cc(0, n, v)))
            .collect()
    }

    #[test]
    fn a_14_bit_data_entry_follows_the_selected_nrpn() {
        let mut p = ParamParser::new();
        // NRPN 130 = (1 << 7) | 2, data 100/17.
        let events = run(&mut p, &[(99, 1), (98, 2), (6, 100), (38, 17)]);
        assert_eq!(events.len(), 2, "one event per data message");
        for e in &events {
            assert_eq!((e.space, e.number), (ParamSpace::Nrpn, 130));
        }
        assert_eq!(events[1].change, ParamChange::Value((100 << 7) | 17));
        assert!(
            (events[1].level().unwrap() - f32::from((100u16 << 7) | 17) / 16383.0).abs() < 1e-6
        );
    }

    #[test]
    fn a_7_bit_sender_reaches_both_ends_of_the_range() {
        let mut p = ParamParser::new();
        let events = run(&mut p, &[(99, 0), (98, 5), (6, 127), (6, 0)]);
        assert_eq!(events[0].change, ParamChange::Value(16_383));
        assert_eq!(events[1].change, ParamChange::Value(0));
    }

    #[test]
    fn the_low_half_first_order_works_too() {
        let mut p = ParamParser::new();
        let events = run(&mut p, &[(98, 9), (99, 0), (38, 3), (6, 4)]);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].number, 9);
        assert_eq!(events[1].change, ParamChange::Value((4 << 7) | 3));
    }

    #[test]
    fn data_without_a_selection_is_nothing() {
        let mut p = ParamParser::new();
        assert!(run(&mut p, &[(6, 10), (38, 1), (96, 1), (97, 1)]).is_empty());
        // Half a number is not a selection either.
        assert!(run(&mut p, &[(99, 1), (6, 10)]).is_empty());
    }

    #[test]
    fn increment_and_decrement_step_the_selection() {
        let mut p = ParamParser::new();
        let events = run(&mut p, &[(101, 0), (100, 2), (96, 0), (96, 5), (97, 1)]);
        assert_eq!(
            events.iter().map(|e| e.change).collect::<Vec<_>>(),
            [
                ParamChange::Step(1),
                ParamChange::Step(5),
                ParamChange::Step(-1)
            ]
        );
        assert!(
            events
                .iter()
                .all(|e| e.space == ParamSpace::Rpn && e.number == 2)
        );
    }

    #[test]
    fn the_null_rpn_deselects() {
        let mut p = ParamParser::new();
        assert_eq!(run(&mut p, &[(101, 0), (100, 0), (6, 2)]).len(), 1);
        assert!(run(&mut p, &[(101, 127), (100, 127), (6, 2), (96, 1)]).is_empty());
        assert_eq!(p.selected(), None);
    }

    #[test]
    fn the_last_family_selected_wins() {
        let mut p = ParamParser::new();
        run(&mut p, &[(99, 0), (98, 7), (101, 0), (100, 1)]);
        assert_eq!(p.selected(), Some((ParamSpace::Rpn, 1)));
        run(&mut p, &[(99, 0), (98, 8)]);
        assert_eq!(p.selected(), Some((ParamSpace::Nrpn, 8)));
    }

    #[test]
    fn a_new_selection_starts_with_a_clean_data_entry() {
        let mut p = ParamParser::new();
        run(&mut p, &[(99, 0), (98, 1), (6, 50), (38, 60)]);
        let events = run(&mut p, &[(98, 2), (6, 10)]);
        assert_eq!(events[0].number, 2);
        // The old low half is gone: the new run is read as 7 bit until a low half arrives.
        assert_eq!(events[0].change, ParamChange::Value((10 << 7) | 10));
    }

    #[test]
    fn ports_and_channels_keep_their_own_selection() {
        let mut bank = ParamBank::new();
        for (port, ch, n) in [("A", 0, 1), ("A", 1, 2), ("B", 0, 3)] {
            assert!(bank.feed(port, &cc(ch, 99, 0)).is_none());
            assert!(bank.feed(port, &cc(ch, 98, n)).is_none());
        }
        let number = |port: &str, ch: u8| match bank.clone().feed(port, &cc(ch, 6, 64)) {
            Some(e) => e.number,
            None => u16::MAX,
        };
        assert_eq!(number("A", 0), 1);
        assert_eq!(number("A", 1), 2);
        assert_eq!(number("B", 0), 3);
        assert_eq!(number("B", 1), u16::MAX, "nothing selected there");
    }

    #[test]
    fn other_messages_and_controllers_are_ignored() {
        let mut bank = ParamBank::new();
        assert!(bank.feed("A", &cc(0, 7, 100)).is_none());
        assert!(
            bank.feed("A", &ChannelMessage::note_on(0, 60, 90))
                .is_none()
        );
        assert!(
            bank.feed("A", &ChannelMessage::pitch_bend(0, 8192))
                .is_none()
        );
        assert!(
            bank.parsers.is_empty(),
            "no parser for an unrelated message"
        );
    }

    #[test]
    fn the_bank_is_bounded() {
        let mut bank = ParamBank::new();
        for i in 0..MAX_BANK_ENTRIES + 10 {
            bank.feed(&format!("port {i}"), &cc(0, 99, 1));
        }
        assert!(bank.parsers.len() <= MAX_BANK_ENTRIES);
    }

    #[test]
    fn the_controller_classes() {
        for n in [6, 38, 96, 97, 98, 99, 100, 101] {
            assert!(is_param_controller(n), "{n}");
        }
        for n in [0, 7, 32, 64, 95, 102, 120] {
            assert!(!is_param_controller(n), "{n}");
        }
        assert!(is_param_selector(99) && is_param_selector(100));
        assert!(!is_param_selector(6) && !is_param_selector(96));
    }

    #[test]
    fn events_describe_themselves() {
        let v = ParamEvent {
            space: ParamSpace::Nrpn,
            number: 12,
            change: ParamChange::Value(100),
        };
        assert_eq!(v.describe(), "NRPN 12 = 100");
        let s = ParamEvent {
            space: ParamSpace::Rpn,
            number: 0,
            change: ParamChange::Step(-2),
        };
        assert_eq!(s.describe(), "RPN 0 -2");
        assert_eq!(s.level(), None);
    }
}
