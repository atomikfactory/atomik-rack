//! The HUI driver: scripted byte streams in, exact bytes out.

use super::*;
use crate::surface::{StripKind, StripState, TransportLeds};

fn state(n: usize) -> SurfaceState {
    SurfaceState {
        strips: (0..n)
            .map(|i| StripState {
                name: format!("Channel {}", i + 1),
                kind: StripKind::Channel,
                fader: 0.5,
                pan: Some(0.0),
                ..StripState::default()
            })
            .collect(),
        ..SurfaceState::default()
    }
}

/// What a surface sends for a switch: the zone, then the port with bit 6 for a press.
fn press(zone: u8, port: u8, down: bool) -> [Vec<u8>; 2] {
    [
        vec![0xB0, 0x0F, zone],
        vec![0xB0, 0x2F, if down { 0x40 | port } else { port }],
    ]
}

fn feed(d: &mut Hui, s: &SurfaceState, messages: &[Vec<u8>]) -> (Vec<SurfaceAction>, Vec<Vec<u8>>) {
    let (mut actions, mut replies) = (Vec::new(), Vec::new());
    for m in messages {
        d.on_message(m, s, &mut actions, &mut replies);
    }
    (actions, replies)
}

fn switch(d: &mut Hui, s: &SurfaceState, zone: u8, port: u8, down: bool) -> Vec<SurfaceAction> {
    feed(d, s, &press(zone, port, down)).0
}

fn feedback(d: &mut Hui, s: &SurfaceState) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    d.feedback(s, &mut out);
    out
}

#[test]
fn the_ping_is_answered_with_the_reply_of_the_protocol() {
    let mut d = Hui::new();
    let s = state(8);
    let (actions, replies) = feed(&mut d, &s, &[vec![0x90, 0x00, 0x00]]);
    assert!(actions.is_empty());
    assert_eq!(replies, vec![vec![0x90, 0x00, 0x7F]]);
}

#[test]
fn strip_switches_are_zone_and_port_pairs() {
    let mut d = Hui::new();
    let s = state(8);
    assert_eq!(switch(&mut d, &s, 2, 1, true), [SurfaceAction::Select(2)]);
    assert_eq!(
        switch(&mut d, &s, 3, 2, true),
        [SurfaceAction::ToggleMute(3)]
    );
    assert_eq!(
        switch(&mut d, &s, 4, 3, true),
        [SurfaceAction::ToggleSolo(4)]
    );
    assert_eq!(
        switch(&mut d, &s, 5, 7, true),
        [SurfaceAction::ToggleArm(5)]
    );
    // A release is not a press.
    assert!(switch(&mut d, &s, 3, 2, false).is_empty());
    // A zone that is not a strip and not known is ignored; so is a port message with no zone.
    assert!(switch(&mut d, &s, 0x20, 2, true).is_empty());
    let mut fresh = Hui::new();
    assert!(feed(&mut fresh, &s, &[vec![0xB0, 0x2F, 0x41]]).0.is_empty());
}

#[test]
fn a_fader_is_an_upper_and_a_lower_controller_and_a_touch_is_a_switch() {
    let mut d = Hui::new();
    let s = state(8);
    // Fader 3 to 0x2345 (14 bit): the upper 7 bits then the lower 7.
    let value = 0x2345u16;
    let got = feed(
        &mut d,
        &s,
        &[
            vec![0xB0, 3, (value >> 7) as u8],
            vec![0xB0, 0x20 + 3, (value & 0x7F) as u8],
        ],
    )
    .0;
    let [SurfaceAction::Fader { strip, value: v }] = got.as_slice() else {
        panic!("{got:?}");
    };
    assert_eq!(*strip, 3);
    assert_eq!(fader_to_bend(*v), value);
    assert_eq!(
        switch(&mut d, &s, 3, 0, true),
        [SurfaceAction::Touch {
            strip: 3,
            touched: true
        }]
    );
    assert_eq!(
        switch(&mut d, &s, 3, 0, false),
        [SurfaceAction::Touch {
            strip: 3,
            touched: false
        }]
    );
}

#[test]
fn v_pots_and_the_jog_wheel_are_relative() {
    let mut d = Hui::new();
    let s = state(8);
    let got = feed(&mut d, &s, &[vec![0xB0, 0x40 + 2, 0x03]]).0;
    assert_eq!(
        got,
        [SurfaceAction::Pan {
            strip: 2,
            delta: 3.0 / 32.0
        }]
    );
    let got = feed(&mut d, &s, &[vec![0xB0, 0x40 + 2, 0x41]]).0;
    assert_eq!(
        got,
        [SurfaceAction::Pan {
            strip: 2,
            delta: -1.0 / 32.0
        }]
    );
    assert_eq!(
        feed(&mut d, &s, &[vec![0xB0, 0x0D, 0x02]]).0,
        [SurfaceAction::Jog(2)]
    );
    assert_eq!(
        feed(&mut d, &s, &[vec![0xB0, 0x0D, 0x43]]).0,
        [SurfaceAction::Jog(-3)]
    );
}

#[test]
fn the_transport_zone_and_the_bank_zone() {
    let mut d = Hui::new();
    let s = state(20);
    for (port, button) in [
        (0, TransportButton::Rewind),
        (1, TransportButton::FastForward),
        (2, TransportButton::Stop),
        (3, TransportButton::Play),
        (4, TransportButton::Record),
    ] {
        assert_eq!(
            switch(&mut d, &s, 0x0E, port, true),
            [SurfaceAction::Transport(button)]
        );
    }
    // Eight strips right, one right, one left, eight left; the last page ends at the strips.
    assert!(switch(&mut d, &s, 0x0A, 3, true).is_empty());
    assert_eq!(d.bank(), 8);
    switch(&mut d, &s, 0x0A, 3, true);
    assert_eq!(d.bank(), 16, "stops on the last page");
    switch(&mut d, &s, 0x0A, 3, true);
    assert_eq!(d.bank(), 16);
    switch(&mut d, &s, 0x0A, 0, true);
    assert_eq!(d.bank(), 15);
    switch(&mut d, &s, 0x0A, 2, true);
    assert_eq!(d.bank(), 16);
    switch(&mut d, &s, 0x0A, 1, true);
    assert_eq!(d.bank(), 8);
    // The strips of the bank are the ones the switches address.
    assert_eq!(
        switch(&mut d, &s, 1, 2, true),
        [SurfaceAction::ToggleMute(9)]
    );
}

#[test]
fn the_first_feedback_shows_everything_and_the_second_nothing() {
    let mut d = Hui::new();
    let mut s = state(8);
    s.strips[1].mute = true;
    s.strips[1].meter = 0.5;
    s.strips[2].pan = Some(1.0);
    s.transport = TransportLeds {
        playing: true,
        ..TransportLeds::default()
    };
    let first = feedback(&mut d, &s);
    // Fader 0 at 0.5: 8192 -> upper 0x40, lower 0x00.
    assert!(first.contains(&vec![0xB0, 0x00, 0x40]));
    assert!(first.contains(&vec![0xB0, 0x20, 0x00]));
    // The mute lamp of strip 1 is the pair (zone 1, 0x40 | port 2); an unlit lamp port only.
    let at = first
        .windows(2)
        .position(|w| w[0] == [0xB0, 0x0C, 1] && w[1] == [0xB0, 0x2C, 0x42])
        .expect("mute lamp");
    assert!(at > 0);
    assert!(
        first
            .windows(2)
            .any(|w| w[0] == [0xB0, 0x0C, 1] && w[1] == [0xB0, 0x2C, 0x03]),
        "the solo lamp is off"
    );
    // The play lamp is on, the stop lamp off.
    assert!(
        first
            .windows(2)
            .any(|w| w[0] == [0xB0, 0x0C, 0x0E] && w[1] == [0xB0, 0x2C, 0x43])
    );
    // The display of strip 0: its name cut to four characters.
    let mut name = HEADER.to_vec();
    name.extend_from_slice(&[0x10, 0]);
    name.extend_from_slice(b"Chan");
    name.push(0xF7);
    assert!(first.contains(&name));
    // The meter of strip 1 at 0.5: step 6 (0.5 * 11 rounds to 6) on both sides.
    for side in 0..2u8 {
        let mut m = HEADER.to_vec();
        m.extend_from_slice(&[0x20, 1, (side << 4) | 6, 0xF7]);
        assert!(first.contains(&m), "side {side}");
    }
    // The pan ring of strip 2, hard right: position 11.
    assert!(first.contains(&vec![0xB0, 0x12, ring_value(Some(1.0))]));
    assert!(feedback(&mut d, &s).is_empty(), "nothing changed");
    // One change, one pair of messages.
    s.strips[4].solo = true;
    assert_eq!(
        feedback(&mut d, &s),
        vec![vec![0xB0, 0x0C, 4], vec![0xB0, 0x2C, 0x43]]
    );
}

#[test]
fn a_held_fader_is_not_driven_and_a_moved_fader_is_not_echoed() {
    let mut d = Hui::new();
    let mut s = state(8);
    feedback(&mut d, &s);
    switch(&mut d, &s, 0, 0, true);
    s.strips[0].fader = 0.9;
    assert!(feedback(&mut d, &s).is_empty(), "the finger is on it");
    switch(&mut d, &s, 0, 0, false);
    // The user moved it to where the project now is: nothing to send back.
    let bend = fader_to_bend(0.9);
    feed(
        &mut d,
        &s,
        &[
            vec![0xB0, 0, (bend >> 7) as u8],
            vec![0xB0, 0x20, (bend & 0x7F) as u8],
        ],
    );
    assert!(feedback(&mut d, &s).is_empty());
    s.strips[0].fader = 0.1;
    assert_eq!(feedback(&mut d, &s).len(), 2, "the motor follows");
}

#[test]
fn a_strip_past_the_end_shows_nothing_and_reaches_nothing() {
    let mut d = Hui::new();
    let s = state(3);
    assert!(switch(&mut d, &s, 6, 2, true).is_empty());
    assert!(
        feed(&mut d, &s, &[vec![0xB0, 6, 0x10], vec![0xB0, 0x26, 0x10]])
            .0
            .is_empty()
    );
    let first = feedback(&mut d, &s);
    // Strip 5's display is four spaces.
    let mut blank = HEADER.to_vec();
    blank.extend_from_slice(&[0x10, 5]);
    blank.extend_from_slice(b"    ");
    blank.push(0xF7);
    assert!(first.contains(&blank));
}

#[test]
fn it_is_in_the_driver_list_and_wants_meters() {
    assert!(crate::surface::DRIVERS.iter().any(|(id, _)| *id == "hui"));
    let d = crate::surface::driver_by_id("hui").unwrap();
    assert_eq!(d.name(), "hui");
    assert!(d.needs().meters);
    assert!(!d.feeds_mappings());
}
