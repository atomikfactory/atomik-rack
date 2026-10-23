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

impl AudioOp {
    /// The op for a code, `None` for a code no parent writes.
    pub fn from_code(code: u32) -> Option<Self> {
        match code {
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
    use atomik_core::ExprKind;
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
        ];
        (any::<u16>(), kind).prop_map(|(offset, kind)| PluginEvent { offset, kind })
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
}
