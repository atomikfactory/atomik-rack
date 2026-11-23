//! Codes for the small enums that cross in the block header and the request/response areas.
//! The event and transport encoders are [`WireEvent::encode`] and [`WireTransport::encode`]
//! in `layout`; this module holds the rest, so every value that crosses has one place to be
//! read.

use atomik_plugin_host::{ProcessStatus, SleepRule, TailLength};

/// Op codes of the audio exchange.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioOp {
    /// Process one 64-frame block.
    Process = 1,
    /// `start_processing`.
    Start = 2,
    /// `stop_processing`.
    Stop = 3,
    /// `reset`.
    Reset = 4,
}

/// Work the child does before the op of a request, in the order stop, start, reset. It is
/// what lets a start (or a reset) ride on the batch that needs it: the parent never waits for
/// a request of its own for them, so a plugin that is slow to start only makes that batch late.
/// The bits sit above the op code in the request's op word ([`AudioOp::from_code`] ignores
/// them).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Before(u32);

impl Before {
    /// Nothing before the op.
    pub const NONE: Before = Before(0);
    /// `stop_processing` first.
    pub const STOP: Before = Before(1);
    /// `start_processing` first (answered false: the request carries no audio and says
    /// [`ProcessStatus::StartFailed`]).
    pub const START: Before = Before(2);
    /// `reset` first (after the start).
    pub const RESET: Before = Before(4);

    const SHIFT: u32 = 8;
    const MASK: u32 = 7;

    /// Both sets of work.
    pub const fn with(self, other: Before) -> Before {
        Before(self.0 | other.0)
    }

    /// `self` without the work of `other`.
    pub const fn without(self, other: Before) -> Before {
        Before(self.0 & !other.0)
    }

    /// True when all the work of `other` is in `self` (and `other` is not empty).
    pub const fn has(self, other: Before) -> bool {
        other.0 != 0 && self.0 & other.0 == other.0
    }

    /// True when there is nothing to do.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The op word: the op code with this work above it.
    pub fn word(self, op: AudioOp) -> u32 {
        (op as u32) | (self.0 << Self::SHIFT)
    }

    /// The work in an op word (bits nobody writes are ignored).
    pub fn from_word(word: u32) -> Before {
        Before((word >> Self::SHIFT) & Self::MASK)
    }
}

impl AudioOp {
    /// The op for an op word (the work above the op code is [`Before::from_word`]), `None` for
    /// a code no parent writes.
    pub fn from_code(code: u32) -> Option<Self> {
        match code & ((1 << Before::SHIFT) - 1) {
            1 => Some(Self::Process),
            2 => Some(Self::Start),
            3 => Some(Self::Stop),
            4 => Some(Self::Reset),
            _ => None,
        }
    }
}

/// The wire code of a status.
pub fn status_code(status: ProcessStatus) -> u32 {
    match status {
        ProcessStatus::Continue => 0,
        ProcessStatus::ContinueIfNotQuiet => 1,
        ProcessStatus::Tail => 2,
        ProcessStatus::Sleep => 3,
        ProcessStatus::Error => 4,
        ProcessStatus::Late => 5,
        ProcessStatus::Lost => 6,
        ProcessStatus::Refused => 7,
        ProcessStatus::StartFailed => 8,
    }
}

/// The status of a wire code; a code nobody writes is [`ProcessStatus::Error`].
pub fn status_from_code(code: u32) -> ProcessStatus {
    match code {
        0 => ProcessStatus::Continue,
        1 => ProcessStatus::ContinueIfNotQuiet,
        2 => ProcessStatus::Tail,
        3 => ProcessStatus::Sleep,
        5 => ProcessStatus::Late,
        6 => ProcessStatus::Lost,
        7 => ProcessStatus::Refused,
        8 => ProcessStatus::StartFailed,
        _ => ProcessStatus::Error,
    }
}

/// The wire code of a sleep rule.
pub fn sleep_rule_code(rule: SleepRule) -> u32 {
    match rule {
        SleepRule::PluginDecides => 0,
        SleepRule::ZeroTail => 1,
    }
}

/// The sleep rule of a wire code (0, the zeroed header, is the CLAP rule).
pub fn sleep_rule_from_code(code: u32) -> SleepRule {
    match code {
        1 => SleepRule::ZeroTail,
        _ => SleepRule::PluginDecides,
    }
}

/// The wire code of a tail: its frames, or `u64::MAX` for an infinite one.
pub fn tail_code(tail: TailLength) -> u64 {
    match tail {
        TailLength::Finite(frames) => u64::from(frames),
        TailLength::Infinite => u64::MAX,
    }
}

/// The tail of a wire code (any value above `u32::MAX` is infinite).
pub fn tail_from_code(code: u64) -> TailLength {
    match u32::try_from(code) {
        Ok(frames) => TailLength::Finite(frames),
        Err(_) => TailLength::Infinite,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{WireEvent, WireTransport};
    use atomik_core::{BATCH, ExprKind};
    use atomik_plugin_host::{PluginEvent, PluginEventKind, PluginTransport};
    use proptest::prelude::*;

    fn expr_kind() -> impl Strategy<Value = ExprKind> {
        prop_oneof![
            Just(ExprKind::Tuning),
            Just(ExprKind::Pressure),
            Just(ExprKind::Slide),
            Just(ExprKind::Volume),
            Just(ExprKind::Pan),
        ]
    }

    fn f32_bits() -> impl Strategy<Value = f32> {
        any::<u32>().prop_map(f32::from_bits)
    }

    fn f64_bits() -> impl Strategy<Value = f64> {
        any::<u64>().prop_map(f64::from_bits)
    }

    fn event() -> impl Strategy<Value = PluginEvent> {
        let kind = prop_oneof![
            (any::<u8>(), f32_bits(), any::<i32>()).prop_map(|(key, velocity, note_id)| {
                PluginEventKind::NoteOn {
                    key,
                    velocity,
                    note_id,
                }
            }),
            (any::<u8>(), any::<i32>())
                .prop_map(|(key, note_id)| PluginEventKind::NoteOff { key, note_id }),
            f32_bits().prop_map(|value| PluginEventKind::PitchBend { value }),
            (any::<u8>(), f32_bits())
                .prop_map(|(controller, value)| PluginEventKind::Cc { controller, value }),
            f32_bits().prop_map(|value| PluginEventKind::ChannelPressure { value }),
            (any::<u8>(), any::<i32>(), f32_bits()).prop_map(|(key, note_id, value)| {
                PluginEventKind::PolyPressure {
                    key,
                    note_id,
                    value,
                }
            }),
            (any::<u8>(), any::<i32>(), expr_kind(), f32_bits()).prop_map(
                |(key, note_id, kind, value)| PluginEventKind::NoteExpr {
                    key,
                    note_id,
                    kind,
                    value,
                }
            ),
            (any::<u32>(), f64_bits()).prop_map(|(id, value)| PluginEventKind::Param { id, value }),
            (any::<[u8; 3]>(), 2u8..=3)
                .prop_map(|(bytes, len)| PluginEventKind::Midi { bytes, len }),
        ];
        (0..BATCH as u16, kind).prop_map(|(offset, kind)| PluginEvent { offset, kind })
    }

    proptest! {
        #[test]
        fn every_event_round_trips_bit_for_bit(e in event()) {
            let wire = WireEvent::encode(&e);
            let back = wire.decode().expect("an encoded event decodes");
            prop_assert_eq!(back.offset, e.offset);
            // NaN is never equal to itself: compare the bits through the wire form, and the
            // variant and integer fields through Debug (which prints NaN without a payload).
            prop_assert_eq!(WireEvent::encode(&back), wire);
            prop_assert_eq!(format!("{:?}", back), format!("{:?}", e));
        }

        #[test]
        fn the_float_payload_of_an_event_keeps_its_nan_bits(bits in any::<u32>(), wide in any::<u64>()) {
            let note = PluginEvent {
                offset: 3,
                kind: PluginEventKind::PitchBend { value: f32::from_bits(bits) },
            };
            prop_assert_eq!(WireEvent::encode(&note).value, bits);
            let param = PluginEvent {
                offset: 3,
                kind: PluginEventKind::Param { id: 9, value: f64::from_bits(wide) },
            };
            prop_assert_eq!(WireEvent::encode(&param).wide, wide);
            match WireEvent::encode(&param).decode().unwrap().kind {
                PluginEventKind::Param { value, .. } => prop_assert_eq!(value.to_bits(), wide),
                other => prop_assert!(false, "{:?}", other),
            }
        }

        #[test]
        fn a_transport_round_trips_bit_for_bit(
            playing in any::<bool>(),
            loop_active in any::<bool>(),
            f in proptest::array::uniform6(f64_bits()),
            rate in f64_bits(),
            steady in any::<u64>(),
            ts in (any::<u8>(), any::<u8>()),
        ) {
            let t = PluginTransport {
                playing,
                tempo_bpm: f[0],
                song_pos_beats: f[1],
                song_pos_seconds: f[2],
                steady_frames: steady,
                time_signature: ts,
                loop_active,
                loop_start_beats: f[3],
                loop_end_beats: f[4],
                sample_rate: rate,
            };
            let back = WireTransport::encode(&t).decode();
            prop_assert_eq!(back.playing, t.playing);
            prop_assert_eq!(back.loop_active, t.loop_active);
            prop_assert_eq!(back.steady_frames, t.steady_frames);
            prop_assert_eq!(back.time_signature, t.time_signature);
            prop_assert_eq!(back.tempo_bpm.to_bits(), t.tempo_bpm.to_bits());
            prop_assert_eq!(back.song_pos_beats.to_bits(), t.song_pos_beats.to_bits());
            prop_assert_eq!(back.song_pos_seconds.to_bits(), t.song_pos_seconds.to_bits());
            prop_assert_eq!(back.loop_start_beats.to_bits(), t.loop_start_beats.to_bits());
            prop_assert_eq!(back.loop_end_beats.to_bits(), t.loop_end_beats.to_bits());
            prop_assert_eq!(back.sample_rate.to_bits(), t.sample_rate.to_bits());
        }
    }

    #[test]
    fn an_offset_past_the_batch_is_pulled_back_to_its_last_frame() {
        let at = |offset: u32| WireEvent {
            tag: crate::layout::EV_PITCH_BEND,
            offset,
            ..WireEvent::default()
        };
        let last = BATCH as u16 - 1;
        for (wire, want) in [
            (63, 63),
            (0, 0),
            (64, last),
            (65, last),
            (65_535, last),
            (65_536, last),
            (u32::MAX, last),
        ] {
            let e = at(wire)
                .decode()
                .expect("an offset never refuses the event");
            assert_eq!(e.offset, want, "offset {wire}");
        }
    }

    #[test]
    fn an_empty_or_unknown_slot_does_not_decode() {
        assert!(WireEvent::default().decode().is_none());
        let bad = WireEvent {
            tag: 999,
            ..WireEvent::default()
        };
        assert!(bad.decode().is_none());
        let bad_key = WireEvent {
            tag: crate::layout::EV_NOTE_ON,
            a: 300,
            ..WireEvent::default()
        };
        assert!(bad_key.decode().is_none());
        let bad_expr = WireEvent {
            tag: crate::layout::EV_NOTE_EXPR,
            c: 77,
            ..WireEvent::default()
        };
        assert!(bad_expr.decode().is_none());
    }

    #[test]
    fn every_status_round_trips_and_an_unknown_code_is_an_error() {
        for s in [
            ProcessStatus::Continue,
            ProcessStatus::ContinueIfNotQuiet,
            ProcessStatus::Tail,
            ProcessStatus::Sleep,
            ProcessStatus::Error,
            ProcessStatus::Late,
            ProcessStatus::Lost,
            ProcessStatus::Refused,
            ProcessStatus::StartFailed,
        ] {
            assert_eq!(status_from_code(status_code(s)), s);
        }
        assert_eq!(status_from_code(0xdead), ProcessStatus::Error);
    }

    #[test]
    fn the_status_codes_are_stable() {
        assert_eq!(status_code(ProcessStatus::Continue), 0);
        assert_eq!(status_code(ProcessStatus::Late), 5);
        assert_eq!(status_code(ProcessStatus::Lost), 6);
        assert_eq!(status_code(ProcessStatus::StartFailed), 8);
    }

    #[test]
    fn sleep_rules_and_tails_round_trip() {
        for r in [SleepRule::PluginDecides, SleepRule::ZeroTail] {
            assert_eq!(sleep_rule_from_code(sleep_rule_code(r)), r);
        }
        for t in [
            TailLength::Finite(0),
            TailLength::Finite(48_000),
            TailLength::Finite(u32::MAX),
            TailLength::Infinite,
        ] {
            assert_eq!(tail_from_code(tail_code(t)), t);
        }
    }

    #[test]
    fn a_zeroed_header_means_the_plugin_decides_and_no_tail() {
        assert_eq!(sleep_rule_from_code(0), SleepRule::PluginDecides);
        assert_eq!(tail_from_code(0), TailLength::Finite(0));
    }

    #[test]
    fn ops_round_trip_and_an_unknown_op_is_refused() {
        for op in [
            AudioOp::Process,
            AudioOp::Start,
            AudioOp::Stop,
            AudioOp::Reset,
        ] {
            assert_eq!(AudioOp::from_code(op as u32), Some(op));
        }
        assert_eq!(AudioOp::from_code(0), None);
        assert_eq!(AudioOp::from_code(99), None);
    }

    #[test]
    fn the_work_before_an_op_rides_above_its_code_and_does_not_change_it() {
        let all = Before::STOP.with(Before::START).with(Before::RESET);
        for op in [
            AudioOp::Process,
            AudioOp::Start,
            AudioOp::Stop,
            AudioOp::Reset,
        ] {
            let word = all.word(op);
            assert_eq!(AudioOp::from_code(word), Some(op));
            assert_eq!(Before::from_word(word), all);
            assert_eq!(Before::from_word(Before::NONE.word(op)), Before::NONE);
        }
        assert!(all.has(Before::START) && !all.without(Before::START).has(Before::START));
        assert!(!Before::NONE.has(Before::NONE), "nothing is not work");
        assert!(Before::NONE.is_empty() && !Before::RESET.is_empty());
        assert_eq!(AudioOp::Process as u32, Before::NONE.word(AudioOp::Process));
        // Bits nobody writes do not make an op out of an unknown code, nor work out of nothing.
        assert_eq!(AudioOp::from_code(99 | (7 << 8)), None);
        assert_eq!(Before::from_word(u32::MAX), all);
    }
}
