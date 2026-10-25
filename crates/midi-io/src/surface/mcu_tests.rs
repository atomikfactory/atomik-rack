//! The eight-fader surface driver: every message in both directions, with exact bytes.

use super::*;
use crate::surface::{MasterState, PositionState, StripKind, StripState, TransportLeds};

fn strip(name: &str) -> StripState {
    StripState {
        name: name.into(),
        kind: StripKind::Channel,
        fader: 0.5,
        fader_text: "-6.0dB".into(),
        pan: Some(0.0),
        pan_text: "C".into(),
        ..StripState::default()
    }
}

fn state(n: usize) -> SurfaceState {
    SurfaceState {
        strips: (0..n).map(|i| strip(&format!("Ch{}", i + 1))).collect(),
        master: Some(MasterState { fader: 0.75 }),
        transport: TransportLeds::default(),
        position: PositionState {
            bar: 1,
            beat: 1,
            sixteenth: 1,
            ticks: 0,
            seconds: 0.0,
        },
    }
}

fn input(m: &mut Mcu, s: &SurfaceState, bytes: &[u8]) -> (Vec<SurfaceAction>, Vec<Vec<u8>>) {
    let (mut actions, mut replies) = (Vec::new(), Vec::new());
    m.on_message(bytes, s, &mut actions, &mut replies);
    (actions, replies)
}

fn actions(m: &mut Mcu, s: &SurfaceState, bytes: &[u8]) -> Vec<SurfaceAction> {
    input(m, s, bytes).0
}

fn feedback(m: &mut Mcu, s: &SurfaceState) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    m.feedback(s, &mut out);
    out
}

/// A fader value close to `want`, within one step of the 14 bit wire.
fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1.0 / 16_383.0
}

#[test]
fn a_fader_pitch_bend_becomes_a_fader_action_and_back_to_the_same_bytes() {
    let s = state(8);
    let mut m = Mcu::new();
    for bend in [0u16, 1, 8192, 12_345, 16_383] {
        let bytes = ChannelMessage::pitch_bend(3, bend).to_bytes();
        let got = actions(&mut m, &s, &bytes);
        let [SurfaceAction::Fader { strip, value }] = got.as_slice() else {
            panic!("one fader action, got {got:?}");
        };
        assert_eq!(*strip, 3);
        // The round trip through the normalised value is exact.
        assert_eq!(fader_to_bend(*value), bend);
        assert!(near(*value, f32::from(bend) / 16_383.0));
    }
    assert_eq!(fader_to_bend(0.0), 0);
    assert_eq!(fader_to_bend(1.0), 16_383);
    assert_eq!(fader_to_bend(2.0), 16_383, "clamped");
    assert_eq!(fader_to_bend(-1.0), 0, "clamped");
}

#[test]
fn the_master_fader_is_channel_eight_and_a_strip_past_the_end_is_ignored() {
    let s = state(3);
    let mut m = Mcu::new();
    let got = actions(&mut m, &s, &[0xE8, 0x7F, 0x7F]);
    assert_eq!(got, [SurfaceAction::MasterFader(1.0)]);
    assert!(
        actions(&mut m, &s, &[0xE5, 0x00, 0x40]).is_empty(),
        "fader 6 has no strip in a project of three"
    );
}

#[test]
fn the_motor_is_driven_by_the_same_message_and_a_moved_fader_is_not_echoed() {
    let mut s = state(8);
    let mut m = Mcu::new();
    let first = feedback(&mut m, &s);
    // Strip 2 at 0.5 -> 8192 (0x00, 0x40) on channel 2; the master at 0.75 on channel 8.
    assert!(first.contains(&vec![0xE2, 0x00, 0x40]));
    let master = fader_to_bend(0.75);
    assert!(first.contains(&vec![0xE8, (master & 0x7F) as u8, (master >> 7) as u8]));
    // The user moves fader 2 by hand; the project follows; nothing is sent back.
    let got = actions(&mut m, &s, &[0xE2, 0x7F, 0x7F]);
    assert_eq!(got.len(), 1);
    s.strips[2].fader = 1.0;
    assert!(
        feedback(&mut m, &s).is_empty(),
        "no echo of the user's move"
    );
    // The project moves the fader (automation): the motor gets the new position.
    s.strips[2].fader = 0.25;
    let moved = feedback(&mut m, &s);
    let want = fader_to_bend(0.25);
    assert_eq!(
        moved,
        vec![vec![0xE2, (want & 0x7F) as u8, (want >> 7) as u8]]
    );
}

#[test]
fn a_touched_fader_is_left_alone_and_touch_is_reported() {
    let mut s = state(8);
    let mut m = Mcu::new();
    feedback(&mut m, &s);
    assert_eq!(
        actions(&mut m, &s, &[0x90, 0x68 + 4, 0x7F]),
        [SurfaceAction::Touch {
            strip: 4,
            touched: true
        }]
    );
    s.strips[4].fader = 0.9;
    let during: Vec<_> = feedback(&mut m, &s)
        .into_iter()
        .filter(|m| m[0] == 0xE4)
        .collect();
    assert!(during.is_empty(), "the motor lets go of a touched fader");
    assert_eq!(
        actions(&mut m, &s, &[0x90, 0x68 + 4, 0x00]),
        [SurfaceAction::Touch {
            strip: 4,
            touched: false
        }]
    );
    let after: Vec<_> = feedback(&mut m, &s)
        .into_iter()
        .filter(|m| m[0] == 0xE4)
        .collect();
    assert_eq!(after.len(), 1, "the motor catches up once it is released");
    // A note off releases as well, and the master has its own touch note.
    assert_eq!(
        actions(&mut m, &s, &[0x80, 0x70, 0x00]),
        [SurfaceAction::MasterTouch(false)]
    );
    assert_eq!(
        actions(&mut m, &s, &[0x90, 0x70, 0x7F]),
        [SurfaceAction::MasterTouch(true)]
    );
}

#[test]
fn v_pots_decode_relative_values_as_sign_and_magnitude() {
    assert_eq!(relative_detents(0x01), 1);
    assert_eq!(relative_detents(0x05), 5);
    assert_eq!(relative_detents(0x41), -1);
    assert_eq!(relative_detents(0x4F), -15);
    assert_eq!(relative_detents(0x00), 0);
    let s = state(8);
    let mut m = Mcu::new();
    let got = actions(&mut m, &s, &[0xB0, 0x10 + 6, 0x03]);
    let [SurfaceAction::Pan { strip, delta }] = got.as_slice() else {
        panic!("one pan action, got {got:?}");
    };
    assert_eq!(*strip, 6);
    assert!((delta - 3.0 / 32.0).abs() < 1e-6);
    let got = actions(&mut m, &s, &[0xB0, 0x10, 0x42]);
    let [SurfaceAction::Pan { delta, .. }] = got.as_slice() else {
        panic!("one pan action, got {got:?}");
    };
    assert!((delta + 2.0 / 32.0).abs() < 1e-6);
}

#[test]
fn the_jog_wheel_is_relative_and_a_zero_turn_is_nothing() {
    let s = state(2);
    let mut m = Mcu::new();
    assert_eq!(
        actions(&mut m, &s, &[0xB0, 0x3C, 0x02]),
        [SurfaceAction::Jog(2)]
    );
    assert_eq!(
        actions(&mut m, &s, &[0xB0, 0x3C, 0x41]),
        [SurfaceAction::Jog(-1)]
    );
    assert!(actions(&mut m, &s, &[0xB0, 0x3C, 0x00]).is_empty());
}

#[test]
fn the_ring_shows_the_pan_as_one_dot_with_the_centre_led() {
    assert_eq!(ring_value(None), 0);
    assert_eq!(ring_value(Some(-1.0)), 1);
    assert_eq!(ring_value(Some(0.0)), 0x40 | 6);
    assert_eq!(ring_value(Some(1.0)), 11);
    assert_eq!(ring_value(Some(0.5)), 9);
    let mut s = state(8);
    s.strips[1].pan = Some(1.0);
    s.strips[2].pan = None;
    let mut m = Mcu::new();
    let out = feedback(&mut m, &s);
    assert!(out.contains(&vec![0xB0, 0x30, 0x46]));
    assert!(out.contains(&vec![0xB0, 0x31, 11]));
    assert!(out.contains(&vec![0xB0, 0x32, 0]));
}

#[test]
fn buttons_toggle_their_strip_and_their_leds_follow() {
    let mut s = state(8);
    let mut m = Mcu::new();
    for (key, want) in [
        (1, SurfaceAction::ToggleArm(1)),
        (0x08 + 2, SurfaceAction::ToggleSolo(2)),
        (0x10 + 3, SurfaceAction::ToggleMute(3)),
        (0x18 + 4, SurfaceAction::Select(4)),
    ] {
        assert_eq!(actions(&mut m, &s, &[0x90, key, 0x7F]), [want]);
        assert!(
            actions(&mut m, &s, &[0x90, key, 0x00]).is_empty(),
            "a release is nothing"
        );
    }
    feedback(&mut m, &s);
    s.strips[1].armed = true;
    s.strips[2].solo = true;
    s.strips[3].mute = true;
    s.strips[4].selected = true;
    let out = feedback(&mut m, &s);
    assert_eq!(
        out,
        vec![
            vec![0x90, 0x01, 0x7F],
            vec![0x90, 0x0A, 0x7F],
            vec![0x90, 0x13, 0x7F],
            vec![0x90, 0x1C, 0x7F],
        ]
    );
    s.strips[3].mute = false;
    assert_eq!(feedback(&mut m, &s), vec![vec![0x90, 0x13, 0x00]]);
}

#[test]
fn the_bank_moves_by_eight_and_by_one_and_stops_at_the_ends() {
    let s = state(20);
    let mut m = Mcu::new();
    let press = |m: &mut Mcu, key: u8| {
        actions(m, &s, &[0x90, key, 0x7F]);
    };
    press(&mut m, 0x2F);
    assert_eq!(m.bank(), 8);
    press(&mut m, 0x2F);
    assert_eq!(m.bank(), 16, "the last page starts at 16");
    press(&mut m, 0x2F);
    assert_eq!(m.bank(), 16, "no page beyond the last");
    press(&mut m, 0x2E);
    assert_eq!(m.bank(), 8);
    press(&mut m, 0x30);
    assert_eq!(m.bank(), 7);
    press(&mut m, 0x31);
    press(&mut m, 0x31);
    assert_eq!(m.bank(), 9);
    for _ in 0..4 {
        press(&mut m, 0x2E);
    }
    assert_eq!(m.bank(), 0, "no bank before the first");
    press(&mut m, 0x30);
    assert_eq!(m.bank(), 0);
    // A strip button now addresses the strip on that fader of the bank.
    press(&mut m, 0x2F);
    assert_eq!(
        actions(&mut m, &s, &[0x90, 0x10 + 2, 0x7F]),
        [SurfaceAction::ToggleMute(10)]
    );
}

#[test]
fn a_bank_change_sends_the_whole_surface_again_with_the_new_names() {
    let s = state(12);
    let mut m = Mcu::new();
    feedback(&mut m, &s);
    assert!(feedback(&mut m, &s).is_empty());
    actions(&mut m, &s, &[0x90, 0x2F, 0x7F]);
    let out = feedback(&mut m, &s);
    // Strips 9..12 exist; the fifth fader onwards has none and shows blanks.
    let name = lcd_message(0, b"Ch9    ");
    assert!(out.contains(&name), "{out:?}");
    let blank = lcd_message(4 * 7, b"       ");
    assert!(out.contains(&blank));
    // The assignment display shows the number of the first strip, 09.
    assert!(out.contains(&vec![0xB0, 0x4B, b'0' & 0x3F]));
    assert!(out.contains(&vec![0xB0, 0x4A, b'9' & 0x3F]));
}

#[test]
fn transport_keys_are_actions_and_their_leds_follow_the_transport() {
    let mut s = state(1);
    let mut m = Mcu::new();
    for (key, want) in [
        (0x5E, TransportButton::Play),
        (0x5D, TransportButton::Stop),
        (0x5F, TransportButton::Record),
        (0x5B, TransportButton::Rewind),
        (0x5C, TransportButton::FastForward),
        (0x56, TransportButton::Cycle),
    ] {
        assert_eq!(
            actions(&mut m, &s, &[0x90, key, 0x7F]),
            [SurfaceAction::Transport(want)]
        );
        assert!(actions(&mut m, &s, &[0x90, key, 0x00]).is_empty());
    }
    feedback(&mut m, &s);
    s.transport = TransportLeds {
        playing: true,
        recording: true,
        looping: true,
    };
    let out = feedback(&mut m, &s);
    assert_eq!(
        out,
        vec![
            vec![0x90, 0x5E, 0x7F],
            vec![0x90, 0x5D, 0x00],
            vec![0x90, 0x5F, 0x7F],
            vec![0x90, 0x56, 0x7F],
        ]
    );
}

#[test]
fn scribble_strips_are_sysex_with_exact_bytes() {
    assert_eq!(
        lcd_message(0, b"Kick   "),
        vec![
            0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, 0x00, b'K', b'i', b'c', b'k', b' ', b' ', b' ',
            0xF7
        ]
    );
    assert_eq!(
        lcd_message(0x38 + 14, b"-6.0dB "),
        vec![
            0xF0, 0x00, 0x00, 0x66, 0x14, 0x12, 0x46, b'-', b'6', b'.', b'0', b'd', b'B', b' ',
            0xF7
        ]
    );
    let mut s = state(8);
    s.strips[1].name = "Lead Synth".into();
    s.strips[1].pan_text = "L20".into();
    let mut m = Mcu::new();
    let out = feedback(&mut m, &s);
    assert!(out.contains(&lcd_message(7, b"Lead S ")));
    assert!(out.contains(&lcd_message(0x38 + 7, b"L20    ")));
    // A name is cut at six characters, a non-ASCII one is a question mark.
    s.strips[1].name = "Caf\u{e9}Longer".into();
    let out = feedback(&mut m, &s);
    assert_eq!(out, vec![lcd_message(7, b"Caf?Lo ")]);
}

#[test]
fn the_lower_row_shows_the_fader_value_while_it_is_touched() {
    let s = state(8);
    let mut m = Mcu::new();
    feedback(&mut m, &s);
    actions(&mut m, &s, &[0x90, 0x68, 0x7F]);
    assert_eq!(feedback(&mut m, &s), vec![lcd_message(0x38, b"-6.0dB ")]);
    actions(&mut m, &s, &[0x90, 0x68, 0x00]);
    assert_eq!(feedback(&mut m, &s), vec![lcd_message(0x38, b"C      ")]);
}

#[test]
fn the_time_display_shows_beats_and_timecode_digit_by_digit() {
    let mut s = state(1);
    s.position = PositionState {
        bar: 12,
        beat: 3,
        sixteenth: 4,
        ticks: 7,
        seconds: 3725.5,
    };
    let mut m = Mcu::new();
    let digits = |out: &[Vec<u8>]| -> Vec<(u8, u8)> {
        let mut d: Vec<(u8, u8)> = out
            .iter()
            .filter(|m| m[0] == 0xB0 && (0x40..=0x49).contains(&m[1]))
            .map(|m| (m[1], m[2]))
            .collect();
        d.sort_by_key(|a| std::cmp::Reverse(a.0));
        d
    };
    let out = feedback(&mut m, &s);
    // 012.03.04.007 -> "0120304007" with a point after the 3rd, 5th and 7th digit.
    let want: Vec<(u8, u8)> = "0120304007"
        .bytes()
        .enumerate()
        .map(|(i, c)| {
            let dot = if matches!(i, 2 | 4 | 6) { 0x40 } else { 0 };
            (0x49 - i as u8, (c & 0x3F) | dot)
        })
        .collect();
    assert_eq!(digits(&out), want);
    // The timecode button swaps the display: 1 h 2 min 5 s and frame 15 (0.5 s at 30 fps).
    actions(&mut m, &s, &[0x90, 0x35, 0x7F]);
    assert!(m.shows_timecode());
    let out = feedback(&mut m, &s);
    let want: Vec<(u8, u8)> = "0010205015"
        .bytes()
        .enumerate()
        .map(|(i, c)| {
            let dot = if matches!(i, 2 | 4 | 6) { 0x40 } else { 0 };
            (0x49 - i as u8, (c & 0x3F) | dot)
        })
        .collect();
    assert_eq!(digits(&out), want);
    // The mode LEDs swapped too.
    assert!(out.contains(&vec![0x90, 0x71, 0x7F]));
    assert!(out.contains(&vec![0x90, 0x72, 0x00]));
    // Only the digits that change are sent: one frame on.
    s.position.seconds += 1.0 / 30.0;
    let out = feedback(&mut m, &s);
    assert_eq!(out, vec![vec![0xB0, 0x40, b'6' & 0x3F]]);
}

#[test]
fn a_second_feedback_with_the_same_state_sends_nothing() {
    let mut s = state(10);
    s.strips[0].armed = true;
    s.transport.playing = true;
    let mut m = Mcu::new();
    assert!(!feedback(&mut m, &s).is_empty());
    assert!(feedback(&mut m, &s).is_empty());
    assert!(feedback(&mut m, &s).is_empty());
    // One change sends only that change.
    s.strips[5].mute = true;
    assert_eq!(feedback(&mut m, &s), vec![vec![0x90, 0x15, 0x7F]]);
}

#[test]
fn a_change_of_one_wire_step_is_not_a_move() {
    let mut s = state(1);
    let mut m = Mcu::new();
    feedback(&mut m, &s);
    s.strips[0].fader = 0.5 + 1.0 / 16_383.0;
    assert!(feedback(&mut m, &s).is_empty());
    s.strips[0].fader = 0.5 + 3.0 / 16_383.0;
    assert_eq!(feedback(&mut m, &s).len(), 1);
}

#[test]
fn connect_asks_the_device_and_sends_everything_and_disconnect_goes_offline() {
    let s = state(2);
    let mut m = Mcu::new();
    let mut out = Vec::new();
    m.connect(&s, &mut out);
    assert_eq!(
        out[0],
        vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7],
        "the device query comes first"
    );
    assert!(out.len() > 20, "the whole surface follows");
    let mut off = Vec::new();
    m.disconnect(&mut off);
    assert_eq!(off, vec![vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x0F, 0xF7]]);
}

#[test]
fn a_host_connection_query_is_answered_with_the_challenge_response() {
    // Challenge 1 2 3 4 -> 5 5 0x7B 0x2F, worked by hand from the documented formula.
    assert_eq!(challenge_response([1, 2, 3, 4]), [5, 5, 0x7B, 0x2F]);
    assert_eq!(challenge_response([0, 0, 0, 0]), [10, 0, 0x00, 0x70]);
    let s = state(1);
    let mut m = Mcu::new();
    let mut query = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x01];
    query.extend_from_slice(b"ABCDEFG");
    query.extend_from_slice(&[1, 2, 3, 4, 0xF7]);
    let (acts, replies) = input(&mut m, &s, &query);
    assert!(acts.is_empty());
    let mut want = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x02];
    want.extend_from_slice(b"ABCDEFG");
    want.extend_from_slice(&[5, 5, 0x7B, 0x2F, 0xF7]);
    assert_eq!(replies, vec![want]);
    // An extender id is understood too; a foreign manufacturer is not.
    let mut ext = query.clone();
    ext[4] = 0x15;
    assert_eq!(input(&mut m, &s, &ext).1.len(), 1);
    let mut foreign = query;
    foreign[2] = 0x01;
    assert!(input(&mut m, &s, &foreign).1.is_empty());
    // After the handshake the surface shows nothing: everything is sent again.
    feedback(&mut m, &s);
    assert!(feedback(&mut m, &s).is_empty());
    input(&mut m, &s, &ext);
    assert!(!feedback(&mut m, &s).is_empty());
}

#[test]
fn garbage_and_other_messages_are_ignored() {
    let s = state(2);
    let mut m = Mcu::new();
    for bytes in [
        &[][..],
        &[0xF0, 0x7E, 0xF7],
        &[0xC0, 5],
        &[0xB0, 0x7B, 0],
        &[0x90, 0x7F, 0x7F],
        &[0xF8],
    ] {
        let (a, r) = input(&mut m, &s, bytes);
        assert!(a.is_empty() && r.is_empty(), "{bytes:?}");
    }
}

#[test]
fn the_driver_is_found_by_its_id() {
    assert_eq!(super::super::driver_by_id("mcu").unwrap().name(), "mcu");
    assert!(super::super::driver_by_id("nope").is_none());
    for (id, label) in super::super::DRIVERS {
        assert!(super::super::driver_by_id(id).is_some(), "{label}");
    }
}
