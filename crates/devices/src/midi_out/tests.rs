use super::*;
use crate::test_support::{harness, off, on};
use atomik_core::NoteEvent;
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
    assert_eq!(DESCRIPTOR.params.len(), 5 + 3 * CC_SLOTS);
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
