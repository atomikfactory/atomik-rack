//! Unit tests for the pure MIDI logic.

use super::*;
use proptest::prelude::*;
use std::time::Duration;

fn on(key: u8, vel: u8) -> Option<NoteKind> {
    Some(NoteKind::On {
        key,
        velocity: f32::from(vel) / 127.0,
    })
}

#[test]
fn parse_note_on_returns_on_with_normalised_velocity() {
    let mut s = RunningStatus::default();
    assert_eq!(parse(&[0x90, 60, 127], &mut s), on(60, 127));
    assert_eq!(parse(&[0x95, 61, 64], &mut s), on(61, 64));
}

#[test]
fn parse_note_off_returns_off() {
    let mut s = RunningStatus::default();
    assert_eq!(
        parse(&[0x80, 60, 40], &mut s),
        Some(NoteKind::Off { key: 60 })
    );
}

#[test]
fn parse_note_on_velocity_zero_returns_off() {
    let mut s = RunningStatus::default();
    assert_eq!(
        parse(&[0x90, 60, 0], &mut s),
        Some(NoteKind::Off { key: 60 })
    );
}

#[test]
fn parse_cc_120_and_123_return_all_off() {
    let mut s = RunningStatus::default();
    assert_eq!(parse(&[0xB0, 123, 0], &mut s), Some(NoteKind::AllOff));
    assert_eq!(parse(&[0xB3, 120, 0], &mut s), Some(NoteKind::AllOff));
}

#[test]
fn parse_other_controllers_return_cc_with_a_normalised_value() {
    // Mod wheel, sustain and every other controller reach the plugins.
    let mut s = RunningStatus::default();
    assert_eq!(
        parse(&[0xB0, 1, 127], &mut s),
        Some(NoteKind::Cc {
            controller: 1,
            value: 1.0
        })
    );
    assert_eq!(
        parse(&[0xB2, 64, 0], &mut s),
        Some(NoteKind::Cc {
            controller: 64,
            value: 0.0
        })
    );
    assert_eq!(
        parse(&[0xB0, 7, 100], &mut s),
        Some(NoteKind::Cc {
            controller: 7,
            value: 100.0 / 127.0
        })
    );
    // Running status carries controllers too.
    assert_eq!(
        parse(&[11, 64], &mut s),
        Some(NoteKind::Cc {
            controller: 11,
            value: 64.0 / 127.0
        })
    );
}

#[test]
fn parse_pitch_bend_centres_on_zero_and_spans_minus_one_to_one() {
    let mut s = RunningStatus::default();
    let bend = |lsb: u8, msb: u8, s: &mut RunningStatus| match parse(&[0xE1, lsb, msb], s) {
        Some(NoteKind::PitchBend { value }) => value,
        other => panic!("{other:?}"),
    };
    assert_eq!(bend(0, 64, &mut s), 0.0, "14-bit 8192 is the centre");
    assert_eq!(bend(0, 0, &mut s), -1.0);
    let top = bend(127, 127, &mut s);
    assert!((top - 8191.0 / 8192.0).abs() < 1e-6 && top < 1.0, "{top}");
    let mid_up = bend(0, 96, &mut s);
    assert!((mid_up - 0.5).abs() < 1e-6, "{mid_up}");
    // LSB first: 0x2001 is a hair above the centre.
    assert!(bend(1, 64, &mut s) > 0.0);
    // Running status.
    assert_eq!(
        parse(&[0, 64], &mut s),
        Some(NoteKind::PitchBend { value: 0.0 })
    );
}

#[test]
fn parse_channel_and_poly_pressure() {
    let mut s = RunningStatus::default();
    assert_eq!(
        parse(&[0xD0, 127], &mut s),
        Some(NoteKind::ChannelPressure { value: 1.0 })
    );
    // Channel pressure has one data byte, so running status takes it alone.
    assert_eq!(
        parse(&[64], &mut s),
        Some(NoteKind::ChannelPressure {
            value: 64.0 / 127.0
        })
    );
    assert_eq!(
        parse(&[0xA3, 60, 127], &mut s),
        Some(NoteKind::PolyPressure {
            key: 60,
            value: 1.0
        })
    );
    assert_eq!(
        parse(&[0xA0, 61, 0], &mut s),
        Some(NoteKind::PolyPressure {
            key: 61,
            value: 0.0
        })
    );
    assert_eq!(parse(&[0xA0, 61], &mut s), None, "truncated");
    assert_eq!(parse(&[0xD0], &mut s), None, "truncated");
}

#[test]
fn parse_running_status_reuses_last_channel_status() {
    let mut s = RunningStatus::default();
    assert_eq!(parse(&[0x90, 60, 100], &mut s), on(60, 100));
    assert_eq!(parse(&[62, 90], &mut s), on(62, 90));
    assert_eq!(parse(&[60, 0], &mut s), Some(NoteKind::Off { key: 60 }));
}

#[test]
fn parse_data_without_status_returns_none() {
    let mut s = RunningStatus::default();
    assert_eq!(parse(&[60, 100], &mut s), None);
}

#[test]
fn parse_realtime_bytes_are_ignored_and_keep_running_status() {
    let mut s = RunningStatus::default();
    assert_eq!(parse(&[0x90, 60, 100], &mut s), on(60, 100));
    assert_eq!(parse(&[0xF8], &mut s), None);
    assert_eq!(parse(&[0xFE], &mut s), None);
    assert_eq!(parse(&[0xFF], &mut s), None);
    assert_eq!(parse(&[0xF8, 64, 0xFA, 50], &mut s), on(64, 50));
    assert_eq!(parse(&[0x90, 0xF8, 65, 0xFC, 51], &mut s), on(65, 51));
}

#[test]
fn parse_system_common_cancels_running_status() {
    let mut s = RunningStatus::default();
    assert_eq!(parse(&[0x90, 60, 100], &mut s), on(60, 100));
    assert_eq!(parse(&[0xF0, 0x7E, 0x7F, 0xF7], &mut s), None);
    assert_eq!(parse(&[60, 100], &mut s), None);
}

#[test]
fn parse_other_channel_messages_return_none_but_set_running_status() {
    let mut s = RunningStatus::default();
    assert_eq!(parse(&[0xC0, 5], &mut s), None); // program change
    assert_eq!(parse(&[5], &mut s), None); // ... and its running status
    assert_eq!(parse(&[0x90, 60, 1], &mut s), on(60, 1));
}

#[test]
fn parse_truncated_or_malformed_returns_none() {
    let mut s = RunningStatus::default();
    assert_eq!(parse(&[], &mut s), None);
    assert_eq!(parse(&[0x90], &mut s), None);
    assert_eq!(parse(&[0x90, 60], &mut s), None);
    assert_eq!(parse(&[0x90, 60, 0x80], &mut s), None);
}

#[test]
fn parse_is_allocation_free() {
    let mut s = RunningStatus::default();
    let r = atomik_testkit::no_alloc::assert_no_alloc(|| parse(&[0x90, 60, 100], &mut s));
    assert_eq!(r, on(60, 100));
}

proptest! {
    #[test]
    fn parse_random_bytes_never_panics_and_keys_are_valid(
        msgs in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..8), 0..32)
    ) {
        let mut s = RunningStatus::default();
        for m in &msgs {
            match parse(m, &mut s) {
                Some(NoteKind::On { key, velocity }) => {
                    prop_assert!(key < 128);
                    prop_assert!(velocity > 0.0 && velocity <= 1.0);
                }
                Some(NoteKind::Off { key }) => prop_assert!(key < 128),
                Some(NoteKind::PitchBend { value }) => prop_assert!((-1.0..1.0).contains(&value)),
                Some(NoteKind::Cc { controller, value }) => {
                    prop_assert!(controller < 128 && controller != 120 && controller != 123);
                    prop_assert!((0.0..=1.0).contains(&value));
                }
                Some(NoteKind::ChannelPressure { value }) => {
                    prop_assert!((0.0..=1.0).contains(&value));
                }
                Some(NoteKind::PolyPressure { key, value }) => {
                    prop_assert!(key < 128 && (0.0..=1.0).contains(&value));
                }
                Some(_) | None => {}
            }
        }
    }

    #[test]
    fn parse_interleaved_realtime_bytes_do_not_change_result(
        key in 0u8..128, vel in 1u8..128, rt in 0xF8u8..=0xFF, pos in 0usize..4
    ) {
        let mut plain = RunningStatus::default();
        let mut noisy = RunningStatus::default();
        let mut bytes = vec![0x91, key, vel];
        let expected = parse(&bytes, &mut plain);
        bytes.insert(pos.min(bytes.len()), rt);
        prop_assert_eq!(parse(&bytes, &mut noisy), expected);
        prop_assert_eq!(noisy, plain);
    }
}

#[allow(clippy::disallowed_methods)] // Justification: test-only origin for a simulated timeline.
fn origin() -> Instant {
    Instant::now()
}

#[test]
fn event_frame_at_clock_instant_is_clock_frame_plus_latency() {
    let t0 = origin();
    assert_eq!(event_frame(1000, t0, t0, 48_000.0, 256), 1256);
}

#[test]
fn event_frame_maps_elapsed_time_to_frames() {
    let t0 = origin();
    let at = t0 + Duration::from_millis(10);
    assert_eq!(event_frame(1000, t0, at, 48_000.0, 256), 1000 + 480 + 256);
}

#[test]
fn event_frame_before_clock_instant_saturates_at_zero() {
    let t0 = origin() + Duration::from_secs(1);
    let at = t0 - Duration::from_millis(500);
    assert_eq!(event_frame(1000, t0, at, 48_000.0, 256), 0);
    let at = t0 - Duration::from_millis(1);
    assert_eq!(event_frame(1000, t0, at, 48_000.0, 256), 1000 + 256 - 48);
}

#[test]
fn event_frame_saturates_at_u64_max() {
    let t0 = origin();
    let at = t0 + Duration::from_secs(10);
    assert_eq!(event_frame(u64::MAX - 5, t0, at, 48_000.0, 256), u64::MAX);
}

#[test]
fn event_frame_constant_spacing_within_one_buffer() {
    // events spaced by a constant interval keep that spacing (±1) and offset L.
    let t0 = origin();
    let sr = 44_100.0;
    let latency = 128;
    let step = Duration::from_micros(700);
    let frames: Vec<u64> = (0..8)
        .map(|i| event_frame(5000, t0, t0 + step * i, sr, latency))
        .collect();
    assert_eq!(frames[0], 5000 + latency);
    let expected = step.as_secs_f64() * sr;
    for w in frames.windows(2) {
        let d = (w[1] - w[0]) as f64;
        assert!((d - expected).abs() <= 1.0, "spacing {d} vs {expected}");
    }
}

proptest! {
    #[test]
    fn event_frame_is_monotonic_in_time(
        frame in 0u64..1_000_000, a in 0u64..50_000_000, b in 0u64..50_000_000,
        sr in prop_oneof![Just(44_100.0), Just(48_000.0), Just(96_000.0)],
    ) {
        let t0 = origin();
        let (lo, hi) = (a.min(b), a.max(b));
        let f_lo = event_frame(frame, t0, t0 + Duration::from_nanos(lo), sr, 64);
        let f_hi = event_frame(frame, t0, t0 + Duration::from_nanos(hi), sr, 64);
        prop_assert!(f_lo <= f_hi);
    }
}

#[test]
fn note_timer_before_first_publish_delivers_at_frame_zero() {
    let mut t = NoteTimer::new(48_000.0, 256);
    assert_eq!(t.frame(None, origin()), 0);
}

#[test]
fn note_timer_uses_clock_and_never_goes_backwards() {
    let t0 = origin();
    let ms = Duration::from_millis;
    let mut t = NoteTimer::new(48_000.0, 256);
    assert_eq!(t.frame(Some((1000, t0)), t0 + ms(1)), 1000 + 48 + 256);
    assert_eq!(t.frame(Some((1290, t0 + ms(6))), t0 + ms(6)), 1546);
    // An older/earlier-mapping clock sample must not reorder events.
    assert_eq!(t.frame(Some((1000, t0)), t0), 1546);
}

#[test]
fn match_port_exact_then_unique_case_insensitive_substring() {
    let names = vec![
        "Example Keyboard".to_string(),
        "Example USB MIDI".to_string(),
        "MIDIIN2 (Example Keyboard)".to_string(),
    ];
    assert_eq!(match_port(&names, "Example USB MIDI"), Ok(1));
    assert_eq!(match_port(&names, "example usb"), Ok(1));
    assert_eq!(match_port(&names, "Example Keyboard"), Ok(0));
    assert!(matches!(
        match_port(&names, "keyboard"),
        Err(MidiError::PortNotFound(_))
    ));
    assert!(matches!(
        match_port(&names, "bome"),
        Err(MidiError::PortNotFound(_))
    ));
    assert!(matches!(
        match_port(&names, ""),
        Err(MidiError::PortNotFound(_))
    ));
}

// ---------------------------------------------------------------------------------------------
// Forwarder end-to-end through the engine's live note queue (needs the engine's live note queue).
// ---------------------------------------------------------------------------------------------

mod forwarder {
    use super::super::*;
    use atomik_core::{SampleRate, Tempo};
    use atomik_engine::{Endpoint, Engine, EngineConfig, EngineHandle, Graph, new_engine};
    use atomik_testkit::fakes::ImpulseOnNote;
    use atomik_testkit::no_alloc::assert_no_alloc;

    /// ImpulseOnNote → out_1; returns the engine, its handle and the note target.
    fn impulse_engine() -> (Engine, EngineHandle, (DeviceInstanceId, SocketId)) {
        let cfg = EngineConfig {
            sample_rate: SampleRate::new(48_000.0).unwrap(),
            tempo: Tempo::new(120.0).unwrap(),
            input_channels: 0,
            output_channels: 1,
            seed: 1,
        };
        let mut g = Graph::new(&cfg);
        let id = g.add_device(ImpulseOnNote::boxed());
        let out_1 = g
            .descriptor(Graph::INTERFACE)
            .unwrap()
            .socket_by_key("out_1")
            .unwrap()
            .id;
        g.connect(
            Endpoint {
                device: id,
                socket: ImpulseOnNote::AUDIO_OUT,
            },
            Endpoint {
                device: Graph::INTERFACE,
                socket: out_1,
            },
        )
        .unwrap();
        let (engine, mut handle) = new_engine(cfg);
        handle.submit_plan(g.compile().unwrap()).unwrap();
        (engine, handle, (id, ImpulseOnNote::NOTES_IN))
    }

    fn render(engine: &mut Engine, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; frames];
        for chunk in out.chunks_mut(256) {
            assert_no_alloc(|| engine.render(&[], &mut [chunk]));
        }
        out
    }

    fn forwarder(
        handle: &mut EngineHandle,
        target: (DeviceInstanceId, SocketId),
    ) -> (Forwarder, Arc<FrameClock>) {
        let clock = Arc::new(FrameClock::new());
        let fwd = Forwarder {
            state: RunningStatus::default(),
            timer: NoteTimer::new(48_000.0, 256),
            sender: handle.live_notes().unwrap(),
            clock: Arc::clone(&clock),
            target,
            dropped: Arc::new(AtomicU64::new(0)),
        };
        (fwd, clock)
    }

    #[test]
    fn forwarder_before_first_clock_publish_delivers_at_frame_zero() {
        let (mut engine, mut handle, target) = impulse_engine();
        let (mut fwd, _clock) = forwarder(&mut handle, target);
        fwd.on_message(&[0x90, 60, 100]);
        fwd.on_message(&[0xF8]); // realtime: ignored
        let out = render(&mut engine, 512);
        let hits: Vec<usize> = (0..out.len()).filter(|&i| out[i] > 0.5).collect();
        assert_eq!(hits, [engine.latency_frames() as usize]);
        assert_eq!(fwd.dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn forwarder_maps_events_through_the_published_clock() {
        let (mut engine, mut handle, target) = impulse_engine();
        let (mut fwd, clock) = forwarder(&mut handle, target);
        clock.publish(1000, timing::now());
        fwd.on_message(&[0x90, 60, 100]);
        let out = render(&mut engine, 4096);
        let hits: Vec<usize> = (0..out.len()).filter(|&i| out[i] > 0.5).collect();
        assert_eq!(hits.len(), 1, "{hits:?}");
        // frame = 1000 + elapsed·sr + 256 latency; elapsed is tiny (allow up to 20 ms).
        let expected = 1000 + 256 + engine.latency_frames() as usize;
        assert!((expected..expected + 960).contains(&hits[0]), "{hits:?}");
    }

    #[test]
    fn forwarder_note_off_and_all_off_are_queued_without_drops() {
        let (_engine, mut handle, target) = impulse_engine();
        let (mut fwd, _clock) = forwarder(&mut handle, target);
        for msg in [
            &[0x90u8, 60, 100][..],
            &[60, 0],
            &[0xB0, 123, 0],
            &[0xB0, 7, 1],
        ] {
            fwd.on_message(msg);
        }
        // Three notes queued (CC 7 ignored); none dropped.
        assert_eq!(fwd.dropped.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn parse_message_keeps_the_channel_and_the_raw_values() {
    let mut s = RunningStatus::default();
    assert_eq!(
        parse_message(&[0xB3, 74, 100], &mut s),
        Some(ChannelMessage::cc(3, 74, 100))
    );
    assert_eq!(
        parse_message(&[0x92, 60, 90], &mut s),
        Some(ChannelMessage::note_on(2, 60, 90))
    );
    assert_eq!(
        parse_message(&[0x92, 60, 0], &mut s),
        Some(ChannelMessage::note_off(2, 60))
    );
    assert_eq!(
        parse_message(&[0x85, 61, 40], &mut s),
        Some(ChannelMessage::note_off(5, 61))
    );
    assert_eq!(
        parse_message(&[0xE1, 0x7F, 0x7F], &mut s),
        Some(ChannelMessage::pitch_bend(1, 16_383))
    );
    assert_eq!(
        parse_message(&[0xE1, 0x00, 0x40], &mut s),
        Some(ChannelMessage::pitch_bend(1, 8192))
    );
}

#[test]
fn parse_message_treats_controllers_120_and_123_as_controllers_and_uses_running_status() {
    let mut s = RunningStatus::default();
    assert_eq!(
        parse_message(&[0xB0, 123, 0], &mut s),
        Some(ChannelMessage::cc(0, 123, 0))
    );
    // Running status: the status byte is the channel's of the last message.
    assert_eq!(
        parse_message(&[7, 33], &mut s),
        Some(ChannelMessage::cc(0, 7, 33))
    );
    // Program change, pressure, system messages and noise are not control messages.
    let mut s = RunningStatus::default();
    assert_eq!(parse_message(&[0xC0, 5], &mut s), None);
    assert_eq!(parse_message(&[0xD0, 5], &mut s), None);
    assert_eq!(parse_message(&[0xF0, 1, 0xF7], &mut s), None);
    assert_eq!(parse_message(&[0x40], &mut s), None);
    assert_eq!(parse_message(&[], &mut s), None);
}

proptest! {
    #[test]
    fn parse_message_never_panics_and_agrees_with_parse_on_notes(bytes in proptest::collection::vec(any::<u8>(), 0..8)) {
        let mut a = RunningStatus::default();
        let mut b = RunningStatus::default();
        let message = parse_message(&bytes, &mut a);
        let note = parse(&bytes, &mut b);
        if let (Some(m), Some(NoteKind::On { key, .. })) = (message, note) {
            let is_on = matches!(m.kind, atomik_core::MessageKind::NoteOn { key: k, .. } if k == key);
            prop_assert!(is_on);
        }
    }
}
