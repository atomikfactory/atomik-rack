use super::*;

fn feed_all(c: &mut Capture, msgs: &[ChannelMessage]) {
    for m in msgs {
        c.feed(m);
    }
}

fn ccs(channel: u8, number: u8, values: &[u8]) -> Vec<ChannelMessage> {
    values
        .iter()
        .map(|v| ChannelMessage::cc(channel, number, *v))
        .collect()
}

fn verdict_of(want: Want, msgs: &[ChannelMessage]) -> Verdict {
    let mut c = Capture::new(want, Vec::new());
    feed_all(&mut c, msgs);
    c.verdict()
}

fn found(source: SourceKind, channel: u8, mode: ControlMode) -> Verdict {
    Verdict::Found(Detection {
        source,
        channel,
        mode,
    })
}

/// The messages of an NRPN data entry: selection, then the 14 bit value.
fn nrpn(channel: u8, number: u16, value: u16) -> Vec<ChannelMessage> {
    vec![
        ChannelMessage::cc(channel, 99, (number >> 7) as u8),
        ChannelMessage::cc(channel, 98, (number & 0x7F) as u8),
        ChannelMessage::cc(channel, 6, (value >> 7) as u8),
        ChannelMessage::cc(channel, 38, (value & 0x7F) as u8),
    ]
}

#[test]
fn nothing_heard_is_nothing() {
    assert_eq!(verdict_of(Want::Continuous, &[]), Verdict::Nothing);
    assert_eq!(verdict_of(Want::Button, &[]), Verdict::Nothing);
}

#[test]
fn an_absolute_knob_is_told_by_the_range_it_sweeps() {
    let sweep: Vec<u8> = (0..=127).step_by(5).collect();
    assert_eq!(
        verdict_of(Want::Continuous, &ccs(2, 21, &sweep)),
        found(SourceKind::Cc(21), 2, ControlMode::Absolute)
    );
    // A nudge is not enough to tell.
    assert!(matches!(
        verdict_of(Want::Continuous, &ccs(0, 21, &[60, 61, 62])),
        Verdict::NeedsMore(_)
    ));
    // One message is not enough either.
    assert!(matches!(
        verdict_of(Want::Continuous, &ccs(0, 21, &[100])),
        Verdict::NeedsMore(_)
    ));
}

#[test]
fn a_fast_absolute_sweep_is_not_an_encoder() {
    assert_eq!(
        verdict_of(Want::Continuous, &ccs(0, 7, &[0, 40, 90, 127])),
        found(SourceKind::Cc(7), 0, ControlMode::Absolute)
    );
}

#[test]
fn encoders_are_told_in_each_of_the_three_codings() {
    // Two's complement: 1, 2 clockwise, 127, 126 counter-clockwise.
    assert_eq!(
        verdict_of(Want::Continuous, &ccs(0, 16, &[1, 1, 2, 127, 127, 126])),
        found(
            SourceKind::Cc(16),
            0,
            ControlMode::Relative(RelativeKind::TwosComplement)
        )
    );
    // Binary offset: 65, 66 clockwise, 63, 62 counter-clockwise.
    assert_eq!(
        verdict_of(Want::Continuous, &ccs(0, 17, &[65, 65, 66, 63, 63, 62])),
        found(
            SourceKind::Cc(17),
            0,
            ControlMode::Relative(RelativeKind::BinaryOffset)
        )
    );
    // Sign and size: 1, 2 clockwise, 65, 66 counter-clockwise.
    assert_eq!(
        verdict_of(Want::Continuous, &ccs(0, 18, &[1, 2, 2, 65, 66, 65])),
        found(
            SourceKind::Cc(18),
            0,
            ControlMode::Relative(RelativeKind::SignMagnitude)
        )
    );
}

#[test]
fn an_encoder_turned_one_way_asks_for_the_other_way() {
    let v = verdict_of(Want::Continuous, &ccs(0, 16, &[1, 1, 2, 1, 1]));
    assert!(matches!(v, Verdict::NeedsMore(_)), "{v:?}");
    // Then the other way settles it.
    let mut c = Capture::new(Want::Continuous, Vec::new());
    feed_all(&mut c, &ccs(0, 16, &[1, 1, 2, 1, 1]));
    feed_all(&mut c, &ccs(0, 16, &[127, 127]));
    assert_eq!(
        c.verdict(),
        found(
            SourceKind::Cc(16),
            0,
            ControlMode::Relative(RelativeKind::TwosComplement)
        )
    );
}

#[test]
fn pitch_bend_is_absolute_once_it_moves() {
    let bend: Vec<ChannelMessage> = [0u16, 4000, 8192, 12000, 16383]
        .iter()
        .map(|v| ChannelMessage::pitch_bend(3, *v))
        .collect();
    assert_eq!(
        verdict_of(Want::Continuous, &bend),
        found(SourceKind::PitchBend, 3, ControlMode::Absolute)
    );
    assert!(matches!(
        verdict_of(Want::Continuous, &bend[2..3]),
        Verdict::NeedsMore(_)
    ));
}

#[test]
fn a_14_bit_pair_is_one_control() {
    let mut msgs = Vec::new();
    for raw in [0u16, 2000, 5000, 9000, 16000] {
        msgs.push(ChannelMessage::cc(0, 1, (raw >> 7) as u8));
        msgs.push(ChannelMessage::cc(0, 33, (raw & 0x7F) as u8));
    }
    assert_eq!(
        verdict_of(Want::Continuous, &msgs),
        found(SourceKind::Cc14(1), 0, ControlMode::Absolute)
    );
}

#[test]
fn an_nrpn_knob_is_absolute_and_its_steps_are_an_encoder() {
    let mut sweep = Vec::new();
    for v in [0u16, 3000, 6000, 12000, 16383] {
        sweep.extend(nrpn(1, 300, v));
    }
    assert_eq!(
        verdict_of(Want::Continuous, &sweep),
        found(SourceKind::Nrpn(300), 1, ControlMode::Absolute)
    );
    // The selecting controllers are not controls of their own.
    let mut c = Capture::new(Want::Continuous, Vec::new());
    feed_all(&mut c, &sweep);
    assert!(
        c.streams
            .iter()
            .all(|s| matches!(s.source, SourceKind::Nrpn(_)))
    );

    let mut steps = nrpn(0, 7, 0)[..2].to_vec();
    steps.push(ChannelMessage::cc(0, 96, 1));
    steps.push(ChannelMessage::cc(0, 97, 1));
    assert_eq!(
        verdict_of(Want::Continuous, &steps),
        found(
            SourceKind::Nrpn(7),
            0,
            ControlMode::Relative(RelativeKind::TwosComplement)
        )
    );
}

#[test]
fn the_busiest_stream_is_the_control_the_user_moves() {
    // A stray message of another controller does not outvote the knob.
    let mut msgs = ccs(0, 74, &[10]);
    msgs.extend(ccs(0, 21, &(0..=127).step_by(8).collect::<Vec<u8>>()));
    assert_eq!(
        verdict_of(Want::Continuous, &msgs),
        found(SourceKind::Cc(21), 0, ControlMode::Absolute)
    );
}

#[test]
fn keys_are_notes_or_controllers_and_the_first_press_wins() {
    assert_eq!(
        verdict_of(
            Want::Button,
            &[
                ChannelMessage::note_on(9, 36, 100),
                ChannelMessage::note_off(9, 36)
            ]
        ),
        found(SourceKind::Note(36), 9, ControlMode::Button)
    );
    assert_eq!(
        verdict_of(Want::Button, &ccs(0, 115, &[127, 0])),
        found(SourceKind::Cc(115), 0, ControlMode::Button)
    );
    // A release alone (value 0) is not a press.
    assert_eq!(
        verdict_of(Want::Button, &ccs(0, 115, &[0])),
        Verdict::Nothing
    );
    let mut both = vec![ChannelMessage::note_on(0, 60, 90)];
    both.extend(ccs(0, 20, &[127]));
    assert_eq!(
        verdict_of(Want::Button, &both),
        found(SourceKind::Note(60), 0, ControlMode::Button)
    );
    // A key sending an NRPN.
    assert_eq!(
        verdict_of(Want::Button, &nrpn(0, 9, 16383)),
        found(SourceKind::Nrpn(9), 0, ControlMode::Button)
    );
}

#[test]
fn notes_are_not_knobs_and_bend_is_not_a_key() {
    assert_eq!(
        verdict_of(Want::Continuous, &[ChannelMessage::note_on(0, 60, 90)]),
        Verdict::Nothing
    );
    assert_eq!(
        verdict_of(Want::Button, &[ChannelMessage::pitch_bend(0, 16000)]),
        Verdict::Nothing
    );
}

#[test]
fn controls_already_set_up_are_not_heard() {
    let ignore = vec![(SourceKind::Cc(21), 0), (SourceKind::Note(36), 0)];
    let mut c = Capture::new(Want::Continuous, ignore.clone());
    feed_all(&mut c, &ccs(0, 21, &[0, 50, 100, 127]));
    assert_eq!(c.verdict(), Verdict::Nothing);
    feed_all(&mut c, &ccs(0, 22, &[0, 50, 100, 127]));
    assert_eq!(
        c.verdict(),
        found(SourceKind::Cc(22), 0, ControlMode::Absolute)
    );
    // The same number on another channel is another control.
    let mut other = Capture::new(Want::Continuous, ignore.clone());
    feed_all(&mut other, &ccs(1, 21, &[0, 50, 100, 127]));
    assert_eq!(
        other.verdict(),
        found(SourceKind::Cc(21), 1, ControlMode::Absolute)
    );
    let mut key = Capture::new(Want::Button, ignore);
    key.feed(&ChannelMessage::note_on(0, 36, 100));
    assert_eq!(key.verdict(), Verdict::Nothing);
    // A 14 bit control hides both of its halves.
    let mut pair = Capture::new(Want::Continuous, vec![(SourceKind::Cc14(1), 0)]);
    feed_all(&mut pair, &ccs(0, 1, &[0, 50, 100]));
    feed_all(&mut pair, &ccs(0, 33, &[0, 50, 100]));
    assert_eq!(pair.verdict(), Verdict::Nothing);
}

#[test]
fn a_noisy_port_cannot_grow_a_capture_without_bound() {
    let mut c = Capture::new(Want::Continuous, Vec::new());
    for n in 0..120u8 {
        for ch in 0..16u8 {
            c.feed(&ChannelMessage::cc(ch, n, 5));
        }
    }
    for i in 0..10_000 {
        c.feed(&ChannelMessage::cc(0, 1, (i % 128) as u8));
    }
    assert!(c.streams.len() <= MAX_STREAMS);
    assert!(c.streams.iter().all(|s| s.values.len() <= MAX_VALUES));
}

#[test]
fn a_detection_describes_itself() {
    let d = Detection {
        source: SourceKind::Cc(21),
        channel: 0,
        mode: ControlMode::Relative(RelativeKind::BinaryOffset),
    };
    assert_eq!(d.describe(), "CC 21 on channel 1, relative (offset)");
}
