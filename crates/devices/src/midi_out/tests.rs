use super::*;
use crate::test_support::{harness, off, on};
use atomik_core::{ExprKind, NoteEvent};
use atomik_testkit::harness::DeviceHarness;

fn device() -> (DeviceHarness, Arc<MidiTap>) {
    let d = MidiOut::new();
    let tap = d.midi_tap().unwrap();
    (harness(Box::new(d)), tap)
}

/// Everything queued so far as `(frame, bytes)`.
fn drain(tap: &MidiTap) -> Vec<(u64, Vec<u8>)> {
    std::iter::from_fn(|| tap.pop())
        .map(|e| (e.frame, e.bytes().to_vec()))
        .collect()
}

/// The queue without the program change the device sends when it starts.
fn started() -> (DeviceHarness, Arc<MidiTap>) {
    let (mut h, tap) = device();
    h.process();
    drain(&tap);
    (h, tap)
}

fn event(offset: u16, kind: NoteKind) -> NoteEvent {
    NoteEvent { offset, kind }
}

#[test]
fn the_descriptor_is_valid_and_the_tap_is_the_devices_own() {
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
    let d = MidiOut::new();
    assert!(Arc::ptr_eq(&d.midi_tap().unwrap(), &d.midi_tap().unwrap()));
    assert_eq!(d.latency_frames(), 0);
    assert_eq!(DESCRIPTOR.params.len(), 5 + 3 * CC_SLOTS + 2);
    assert_eq!(DESCRIPTOR.sockets.len(), 1 + CC_SLOTS);
}

#[test]
fn notes_go_out_on_the_channel_at_their_frame() {
    let (mut h, tap) = started();
    h.set_param("channel", 3.0);
    h.process(); // frame 64
    h.push_note("notes_in", on(5, 60, 1.0));
    h.push_note("notes_in", on(9, 64, 0.5));
    h.process();
    h.push_note("notes_in", off(2, 60));
    h.process();
    assert_eq!(
        drain(&tap),
        vec![
            (128 + 5, vec![0x92, 60, 127]),
            (128 + 9, vec![0x92, 64, 64]),
            (192 + 2, vec![0x82, 60, 0]),
        ]
    );
}

#[test]
fn transpose_shifts_keys_and_drops_what_leaves_the_range() {
    let (mut h, tap) = started();
    h.set_param("transpose", 12.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(1, 120, 1.0));
    h.process();
    h.push_note("notes_in", off(0, 60));
    h.push_note("notes_in", off(1, 120));
    h.process();
    let sent = drain(&tap);
    assert_eq!(
        sent.iter().map(|(_, b)| b.clone()).collect::<Vec<_>>(),
        vec![vec![0x90, 72, 127], vec![0x80, 72, 0]],
        "key 120 + 12 is out of range: neither its on nor its off is sent"
    );
}

#[test]
fn a_change_of_transpose_or_channel_still_releases_the_note_that_started() {
    let (mut h, tap) = started();
    h.set_param("transpose", 7.0);
    h.set_param("channel", 2.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.process();
    h.set_param("transpose", -5.0);
    h.set_param("channel", 9.0);
    h.push_note("notes_in", off(0, 60));
    h.process();
    let sent: Vec<_> = drain(&tap).into_iter().map(|(_, b)| b).collect();
    assert_eq!(sent, vec![vec![0x91, 67, 127], vec![0x81, 67, 0]]);
}

#[test]
fn a_note_on_for_a_key_that_is_down_retriggers() {
    let (mut h, tap) = started();
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(8, 60, 0.5));
    h.process();
    let sent: Vec<_> = drain(&tap).into_iter().map(|(_, b)| b).collect();
    assert_eq!(
        sent,
        vec![vec![0x90, 60, 127], vec![0x80, 60, 0], vec![0x90, 60, 64]]
    );
    // An off for a key that is not down sends nothing.
    h.push_note("notes_in", off(0, 70));
    h.process();
    assert!(drain(&tap).is_empty());
}

#[test]
fn all_off_releases_every_note_then_all_notes_off_on_the_used_channels() {
    let (mut h, tap) = started();
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(1, 64, 1.0));
    h.process();
    h.set_param("channel", 5.0);
    h.push_note("notes_in", on(0, 67, 1.0));
    h.process();
    drain(&tap);
    h.push_note("notes_in", event(4, NoteKind::AllOff));
    h.process();
    let sent: Vec<_> = drain(&tap).into_iter().map(|(_, b)| b).collect();
    assert_eq!(
        sent,
        vec![
            vec![0x80, 60, 0],
            vec![0x80, 64, 0],
            vec![0x84, 67, 0],
            vec![0xB0, 123, 0],
            vec![0xB4, 123, 0],
        ],
        "one note off per started note, then CC 123 on channels 1 and 5"
    );
    // Nothing is left to release.
    h.push_note("notes_in", event(0, NoteKind::AllOff));
    h.process();
    assert!(drain(&tap).is_empty());
}

#[test]
fn a_reset_releases_at_the_start_of_the_next_batch() {
    let (mut h, tap) = started();
    h.push_note("notes_in", on(0, 60, 1.0));
    h.process();
    drain(&tap);
    h.device_mut().reset();
    h.process();
    let sent: Vec<_> = drain(&tap).into_iter().map(|(_, b)| b).collect();
    assert_eq!(sent, vec![vec![0x80, 60, 0], vec![0xB0, 123, 0]]);
}

#[test]
fn controllers_bend_and_pressure_pass_through_on_the_channel() {
    let (mut h, tap) = started();
    h.set_param("channel", 2.0);
    for (offset, kind) in [
        (
            0,
            NoteKind::Cc {
                controller: 64,
                value: 1.0,
            },
        ),
        (1, NoteKind::PitchBend { value: 0.0 }),
        (2, NoteKind::PitchBend { value: 1.0 }),
        (3, NoteKind::PitchBend { value: -1.0 }),
        (4, NoteKind::ChannelPressure { value: 0.5 }),
        (
            5,
            NoteKind::PolyPressure {
                key: 60,
                value: 1.0,
            },
        ),
    ] {
        h.push_note("notes_in", event(offset, kind));
    }
    h.process();
    let sent: Vec<_> = drain(&tap).into_iter().map(|(_, b)| b).collect();
    assert_eq!(
        sent,
        vec![
            vec![0xB1, 64, 127],
            vec![0xE1, 0, 64],
            vec![0xE1, 127, 127],
            vec![0xE1, 0, 0],
            vec![0xD1, 64],
            vec![0xA1, 60, 127],
        ]
    );
}

#[test]
fn a_program_goes_out_when_the_device_starts_and_when_it_changes() {
    let (mut h, tap) = device();
    h.process();
    assert!(drain(&tap).is_empty(), "no program by default");
    h.set_param("program", 5.0);
    h.process();
    assert_eq!(drain(&tap), vec![(64, vec![0xC0, 5])]);
    h.process();
    assert!(drain(&tap).is_empty(), "only once");
    h.set_param("bank", 300.0); // 2 * 128 + 44
    h.set_param("channel", 4.0);
    h.process();
    assert_eq!(
        drain(&tap),
        vec![
            (192, vec![0xB3, 0, 2]),
            (192, vec![0xB3, 32, 44]),
            (192, vec![0xC3, 5]),
        ]
    );
    h.set_param("program", -1.0);
    h.process();
    assert!(drain(&tap).is_empty(), "-1 sends nothing");
}

#[test]
fn a_program_set_before_the_first_batch_is_sent_with_it() {
    let (mut h, tap) = device();
    h.set_param("program", 9.0);
    h.process();
    assert_eq!(drain(&tap), vec![(0, vec![0xC0, 9])]);
}

#[test]
fn latency_moves_every_message_and_a_negative_one_stops_at_frame_zero() {
    let (mut h, tap) = started();
    h.set_param("latency_ms", 10.0); // 480 frames at 48 kHz
    h.push_note("notes_in", on(3, 60, 1.0));
    h.process(); // base 64
    h.set_param("latency_ms", -2.0); // -96 frames
    h.push_note("notes_in", off(0, 60));
    h.process(); // base 128
    assert_eq!(
        drain(&tap),
        vec![
            (64 + 3 + 480, vec![0x90, 60, 127]),
            (128 - 96, vec![0x80, 60, 0])
        ]
    );
    let (mut h, tap) = device();
    h.set_param("latency_ms", -200.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.process();
    assert_eq!(drain(&tap)[0].0, 0, "never before frame 0");
}

#[test]
fn a_cv_input_sends_its_controller_when_connected_and_changed() {
    let (mut h, tap) = started();
    for s in h.input_mut("cv1").iter_mut() {
        *s = 0.5;
    }
    h.set_connected("cv1", false);
    h.process();
    assert!(drain(&tap).is_empty(), "an unconnected input sends nothing");
    h.set_connected("cv1", true);
    h.process();
    assert_eq!(drain(&tap), vec![(128 + 63, vec![0xB0, 1, 64])]);
    h.process();
    assert!(drain(&tap).is_empty(), "an unchanged value is not repeated");
}

#[test]
fn a_cv_controller_takes_its_number_scale_and_offset_and_is_throttled() {
    let (mut h, tap) = started();
    h.set_connected("cv2", true);
    h.set_param("cc2.number", 74.0);
    h.set_param("cc2.scale", 0.5);
    h.set_param("cc2.offset", 0.25);
    let set = |h: &mut DeviceHarness, v: f32| {
        for s in h.input_mut("cv2").iter_mut() {
            *s = v;
        }
    };
    set(&mut h, 1.0); // 1 * 0.5 + 0.25 = 0.75 -> 95
    h.process();
    let first = drain(&tap);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].1, vec![0xB0, 74, 95]);
    // A new value inside the 10 ms (480 frames, 8 batches) is held back, then it goes.
    set(&mut h, 0.5);
    let mut sent = Vec::new();
    for i in 0..12 {
        h.process();
        let now = drain(&tap);
        if i < 6 {
            assert!(now.is_empty(), "batch {i}: {now:?}");
        }
        sent.extend(now);
    }
    assert_eq!(
        sent.len(),
        1,
        "one message in the next twelve batches: {sent:?}"
    );
    assert_eq!(sent[0].1, vec![0xB0, 74, 64], "0.5 * 0.5 + 0.25 = 0.5");
    // Out of range is clamped.
    for _ in 0..10 {
        h.process();
    }
    drain(&tap);
    set(&mut h, 100.0);
    h.process();
    for _ in 0..10 {
        h.process();
    }
    let sent = drain(&tap);
    assert_eq!(sent.last().unwrap().1[2], 127);
}

#[test]
fn offline_the_device_is_silent() {
    let (mut h, tap) = device();
    h.device_mut().set_run_mode(RunMode::Offline);
    h.set_param("program", 1.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.process();
    h.push_note("notes_in", event(0, NoteKind::AllOff));
    h.process();
    assert!(drain(&tap).is_empty());
}

#[test]
fn a_full_queue_still_takes_note_offs_and_counts_the_loss() {
    let (mut h, tap) = started();
    h.push_note("notes_in", on(0, 60, 1.0));
    h.process();
    // Nobody reads: controllers fill the queue.
    for _ in 0..200 {
        for i in 0..20 {
            h.push_note(
                "notes_in",
                event(
                    i,
                    NoteKind::Cc {
                        controller: 1,
                        value: 0.5,
                    },
                ),
            );
        }
        h.process();
    }
    assert!(tap.dropped() > 0);
    h.push_note("notes_in", off(0, 60));
    h.process();
    assert!(!tap.take_release_lost(), "the off found room");
    let mut last = None;
    while let Some(e) = tap.pop() {
        last = Some(e);
    }
    assert_eq!(last.unwrap().bytes(), [0x80, 60, 0]);
}

#[test]
fn param_sweeps_never_panic_and_stay_in_range() {
    let (mut h, tap) = device();
    for p in DESCRIPTOR.params {
        for v in [p.min, p.max, p.default] {
            h.set_param(p.key, v);
            h.push_note("notes_in", on(0, 127, 1.0));
            h.push_note("notes_in", off(1, 127));
            h.process();
        }
    }
    while let Some(e) = tap.pop() {
        let b = e.bytes();
        assert!(b[0] >= 0x80);
        assert!(b[1..].iter().all(|x| *x < 0x80), "{b:?}");
    }
}

// ----- MPE -------------------------------------------------------------------------------------

fn expr(offset: u16, key: u8, kind: ExprKind, value: f32) -> NoteEvent {
    event(offset, NoteKind::NoteExpr { key, kind, value })
}

/// The bytes of everything queued, in order.
fn sent(tap: &MidiTap) -> Vec<Vec<u8>> {
    drain(tap).into_iter().map(|(_, b)| b).collect()
}

/// A device with an MPE zone of `members` channels (zone 1 lower, 2 upper) that has sent its
/// configuration message.
fn mpe_device(zone: f32, members: f32) -> (DeviceHarness, Arc<MidiTap>) {
    let (mut h, tap) = device();
    h.set_param("mpe.zone", zone);
    h.set_param("mpe.members", members);
    h.process();
    drain(&tap);
    (h, tap)
}

#[test]
fn the_zone_is_announced_with_the_configuration_message_when_it_is_set() {
    let (mut h, tap) = device();
    h.set_param("mpe.zone", 1.0);
    h.set_param("mpe.members", 7.0);
    h.process();
    assert_eq!(
        sent(&tap),
        vec![
            vec![0xB0, 101, 0],
            vec![0xB0, 100, 6],
            vec![0xB0, 6, 7],
            vec![0xB0, 101, 127],
            vec![0xB0, 100, 127],
        ]
    );
    h.process();
    assert!(sent(&tap).is_empty(), "once, not every batch");
}

#[test]
fn an_upper_zone_is_announced_on_channel_sixteen() {
    let (mut h, tap) = device();
    h.set_param("mpe.zone", 2.0);
    h.set_param("mpe.members", 3.0);
    h.process();
    let m = sent(&tap);
    assert!(m.iter().all(|b| b[0] == 0xBF), "{m:?}");
    assert_eq!(m[2], vec![0xBF, 6, 3]);
    // Its notes count down from channel 15.
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(1, 62, 1.0));
    h.process();
    assert_eq!(sent(&tap), vec![vec![0x9E, 60, 127], vec![0x9D, 62, 127]]);
}

#[test]
fn the_zone_is_announced_again_each_time_playback_starts() {
    let (mut h, tap) = mpe_device(1.0, 15.0);
    h.time_mut().playing = false;
    h.process();
    assert!(sent(&tap).is_empty());
    h.time_mut().playing = true;
    h.process();
    assert_eq!(sent(&tap).len(), 5, "playback started");
    h.process();
    assert!(sent(&tap).is_empty(), "not while it plays");
    h.time_mut().playing = false;
    h.process();
    h.time_mut().playing = true;
    h.process();
    assert_eq!(sent(&tap).len(), 5, "and again after a stop");
}

#[test]
fn with_mpe_off_the_expression_is_dropped_and_the_output_is_what_it_was() {
    let (mut h, tap) = started();
    h.set_param("channel", 3.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", expr(0, 60, ExprKind::Tuning, 5.0));
    h.push_note("notes_in", expr(4, 60, ExprKind::Pressure, 1.0));
    h.push_note("notes_in", expr(8, 60, ExprKind::Slide, 1.0));
    h.push_note("notes_in", off(12, 60));
    h.process();
    assert_eq!(sent(&tap), vec![vec![0x92, 60, 127], vec![0x82, 60, 0]]);
}

#[test]
fn notes_rotate_over_the_member_channels_and_their_offs_follow() {
    let (mut h, tap) = mpe_device(1.0, 3.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(1, 64, 1.0));
    h.push_note("notes_in", on(2, 67, 1.0));
    h.process();
    assert_eq!(
        sent(&tap),
        vec![
            vec![0x91, 60, 127],
            vec![0x92, 64, 127],
            vec![0x93, 67, 127]
        ]
    );
    // Released in a different order: the next notes take the channels in the order they were freed.
    h.push_note("notes_in", off(0, 64));
    h.push_note("notes_in", off(1, 60));
    h.push_note("notes_in", on(2, 70, 1.0));
    h.push_note("notes_in", on(3, 72, 1.0));
    h.process();
    assert_eq!(
        sent(&tap),
        vec![
            vec![0x82, 64, 0],
            vec![0x81, 60, 0],
            vec![0x92, 70, 127],
            vec![0x91, 72, 127]
        ]
    );
}

#[test]
fn a_note_with_a_pitch_curve_sends_bends_on_its_own_channel() {
    let (mut h, tap) = mpe_device(1.0, 4.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(0, 64, 1.0));
    h.push_note("notes_in", expr(10, 60, ExprKind::Tuning, 12.0));
    h.push_note("notes_in", expr(20, 60, ExprKind::Tuning, -48.0));
    h.push_note("notes_in", expr(30, 64, ExprKind::Pressure, 1.0));
    h.push_note("notes_in", expr(40, 64, ExprKind::Slide, 0.5));
    h.process();
    assert_eq!(
        drain(&tap),
        vec![
            (64, vec![0x91, 60, 127]),
            (64, vec![0x92, 64, 127]),
            // 12 of 48 semitones up: 8192 + 2048.
            (64 + 10, vec![0xE1, 0x00, 0x50]),
            (64 + 20, vec![0xE1, 0x00, 0x00]),
            (64 + 30, vec![0xD2, 127]),
            (64 + 40, vec![0xB2, 74, 64]),
        ]
    );
}

#[test]
fn the_first_values_of_a_note_go_out_before_its_note_on() {
    let (mut h, tap) = mpe_device(1.0, 4.0);
    h.push_note("notes_in", on(5, 60, 1.0));
    h.push_note("notes_in", expr(5, 60, ExprKind::Tuning, 24.0));
    h.push_note("notes_in", expr(5, 60, ExprKind::Pressure, 1.0));
    h.process();
    assert_eq!(
        sent(&tap),
        vec![vec![0xE1, 0x00, 0x60], vec![0xD1, 127], vec![0x91, 60, 127]],
        "the same values after the note on add nothing"
    );
}

#[test]
fn a_reused_channel_does_not_start_with_the_bend_of_the_note_before() {
    let (mut h, tap) = mpe_device(1.0, 1.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", expr(1, 60, ExprKind::Tuning, 7.0));
    h.push_note("notes_in", off(2, 60));
    h.push_note("notes_in", on(3, 62, 1.0));
    h.process();
    let m = sent(&tap);
    assert_eq!(m.len(), 5, "{m:?}");
    assert_eq!(m[3], vec![0xE1, 0x00, 0x40], "centred before the new note");
    assert_eq!(m[4], vec![0x91, 62, 127]);
}

#[test]
fn global_messages_go_to_the_master_channel_and_the_channel_param_is_not_used() {
    let (mut h, tap) = mpe_device(1.0, 4.0);
    h.set_param("channel", 6.0);
    h.push_note(
        "notes_in",
        event(
            0,
            NoteKind::Cc {
                controller: 64,
                value: 1.0,
            },
        ),
    );
    h.push_note("notes_in", event(1, NoteKind::PitchBend { value: 0.0 }));
    h.push_note(
        "notes_in",
        event(2, NoteKind::ChannelPressure { value: 0.5 }),
    );
    h.push_note("notes_in", on(3, 60, 1.0));
    h.process();
    assert_eq!(
        sent(&tap),
        vec![
            vec![0xB0, 64, 127],
            vec![0xE0, 0, 64],
            vec![0xD0, 64],
            vec![0x91, 60, 127]
        ]
    );
}

#[test]
fn transpose_shifts_the_keys_of_member_notes_and_their_offs() {
    let (mut h, tap) = mpe_device(1.0, 4.0);
    h.set_param("transpose", 12.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(1, 120, 1.0));
    h.process();
    h.set_param("transpose", 0.0);
    h.push_note("notes_in", off(0, 60));
    h.push_note("notes_in", off(1, 120));
    h.process();
    assert_eq!(
        sent(&tap),
        vec![vec![0x91, 72, 127], vec![0x81, 72, 0]],
        "120 + 12 is out of range: no note, no off"
    );
}

#[test]
fn all_off_releases_the_member_notes_and_closes_the_channels_it_used() {
    let (mut h, tap) = mpe_device(1.0, 4.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(1, 64, 1.0));
    h.process();
    drain(&tap);
    h.push_note("notes_in", event(0, NoteKind::AllOff));
    h.process();
    assert_eq!(
        sent(&tap),
        vec![
            vec![0x81, 60, 0],
            vec![0x82, 64, 0],
            vec![0xB0, 123, 0],
            vec![0xB1, 123, 0],
            vec![0xB2, 123, 0],
        ]
    );
    h.push_note("notes_in", event(0, NoteKind::AllOff));
    h.process();
    assert!(sent(&tap).is_empty());
}

#[test]
fn switching_the_zone_off_releases_the_notes_and_switches_the_zone_off_at_the_receiver() {
    let (mut h, tap) = mpe_device(1.0, 5.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.process();
    drain(&tap);
    h.set_param("mpe.zone", 0.0);
    h.set_param("channel", 4.0);
    h.process();
    let m = sent(&tap);
    assert_eq!(m[0], vec![0x81, 60, 0], "the member note is released");
    assert!(m.contains(&vec![0xB0, 6, 0]), "zone off: {m:?}");
    // Plain MIDI again, on the channel param.
    h.push_note("notes_in", on(0, 62, 1.0));
    h.process();
    assert_eq!(sent(&tap), vec![vec![0x93, 62, 127]]);
}

#[test]
fn changing_the_member_count_announces_the_new_zone_and_a_reset_releases_member_notes() {
    let (mut h, tap) = mpe_device(1.0, 4.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.process();
    drain(&tap);
    h.set_param("mpe.members", 9.0);
    h.process();
    let m = sent(&tap);
    assert_eq!(m[0], vec![0x81, 60, 0]);
    assert!(m.contains(&vec![0xB0, 6, 9]), "{m:?}");
    h.push_note("notes_in", on(0, 61, 1.0));
    h.process();
    drain(&tap);
    h.device_mut().reset();
    h.process();
    let m = sent(&tap);
    assert_eq!(m[0], vec![0x81, 61, 0], "{m:?}");
}

#[test]
fn a_program_goes_to_the_master_channel_of_the_zone() {
    let (mut h, tap) = device();
    h.set_param("program", 5.0);
    h.set_param("mpe.zone", 2.0);
    h.process();
    let m = sent(&tap);
    assert_eq!(m.last().unwrap(), &vec![0xCF, 5]);
}

#[test]
fn more_notes_than_members_take_the_channel_of_the_oldest_note() {
    let (mut h, tap) = mpe_device(1.0, 2.0);
    for (i, k) in [60, 62, 64].into_iter().enumerate() {
        h.push_note("notes_in", on(i as u16, k, 1.0));
    }
    h.process();
    assert_eq!(
        sent(&tap),
        vec![
            vec![0x91, 60, 127],
            vec![0x92, 62, 127],
            vec![0x81, 60, 0],
            vec![0x91, 64, 127]
        ]
    );
}
