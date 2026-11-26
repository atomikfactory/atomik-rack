//! The OSC driver: the address map in both directions.

use super::*;
use crate::surface::{MasterState, StripKind, StripState, TransportLeds};

fn state(n: usize) -> SurfaceState {
    SurfaceState {
        strips: (0..n)
            .map(|i| StripState {
                name: format!("Ch{}", i + 1),
                kind: StripKind::Channel,
                fader: 0.5,
                pan: Some(0.0),
                ..StripState::default()
            })
            .collect(),
        master: Some(MasterState { fader: 0.75 }),
        ..SurfaceState::default()
    }
}

fn send(d: &mut Osc, s: &SurfaceState, m: &Message) -> (Vec<SurfaceAction>, Vec<Vec<u8>>) {
    let (mut actions, mut replies) = (Vec::new(), Vec::new());
    d.on_message(&osc_wire::encode(m), s, &mut actions, &mut replies);
    (actions, replies)
}

fn act(d: &mut Osc, s: &SurfaceState, address: &str, args: Vec<Arg>) -> Vec<SurfaceAction> {
    send(
        d,
        s,
        &Message {
            address: address.to_owned(),
            args,
        },
    )
    .0
}

fn feedback(d: &mut Osc, s: &SurfaceState) -> Vec<Message> {
    let mut out = Vec::new();
    d.feedback(s, &mut out);
    let mut messages = Vec::new();
    for packet in &out {
        assert!(packet.len() <= PACKET_LIMIT, "{} bytes", packet.len());
        osc_wire::decode(packet, &mut messages).unwrap();
    }
    messages
}

fn value_of<'a>(messages: &'a [Message], address: &str) -> Option<&'a Arg> {
    messages
        .iter()
        .find(|m| m.address == address)
        .and_then(|m| m.args.first())
}

#[test]
fn faders_pans_and_touch_of_a_strip() {
    let mut d = Osc::new();
    let s = state(4);
    assert_eq!(
        act(&mut d, &s, "/strip/3/fader", vec![Arg::Float(0.25)]),
        [SurfaceAction::Fader {
            strip: 2,
            value: 0.25
        }]
    );
    // Out of range values are clamped; an integer is a number too.
    assert_eq!(
        act(&mut d, &s, "/strip/1/fader", vec![Arg::Int(3)]),
        [SurfaceAction::Fader {
            strip: 0,
            value: 1.0
        }]
    );
    assert_eq!(
        act(&mut d, &s, "/strip/2/pan", vec![Arg::Float(-1.0)]),
        [SurfaceAction::Strip {
            strip: 1,
            control: StripControl::Pan,
            change: KnobMove::To(0.0)
        }]
    );
    assert_eq!(
        act(&mut d, &s, "/strip/2/touch", vec![Arg::Bool(true)]),
        [SurfaceAction::Touch {
            strip: 1,
            touched: true
        }]
    );
    assert_eq!(
        act(&mut d, &s, "/strip/2/touch", vec![Arg::Int(0)]),
        [SurfaceAction::Touch {
            strip: 1,
            touched: false
        }]
    );
    assert_eq!(
        act(&mut d, &s, "/strip/4/send/3", vec![Arg::Float(0.5)]),
        [SurfaceAction::Strip {
            strip: 3,
            control: StripControl::Send(3),
            change: KnobMove::To(0.5)
        }]
    );
    // Strips are numbered from 1, and a strip that does not exist is ignored.
    for bad in [
        "/strip/0/fader",
        "/strip/5/fader",
        "/strip/x/fader",
        "/strip/1/nope",
    ] {
        assert!(
            act(&mut d, &s, bad, vec![Arg::Float(0.5)]).is_empty(),
            "{bad}"
        );
    }
    assert!(act(&mut d, &s, "/strip/1/send/9", vec![Arg::Float(0.5)]).is_empty());
    assert!(
        act(&mut d, &s, "/strip/1/fader", vec![]).is_empty(),
        "no value"
    );
}

#[test]
fn switches_are_set_to_what_the_message_says_and_toggle_with_no_argument() {
    let mut d = Osc::new();
    let mut s = state(2);
    assert_eq!(
        act(&mut d, &s, "/strip/1/mute", vec![Arg::Int(1)]),
        [SurfaceAction::ToggleMute(0)]
    );
    assert!(
        act(&mut d, &s, "/strip/1/mute", vec![Arg::Int(0)]).is_empty(),
        "already off"
    );
    s.strips[0].mute = true;
    assert!(act(&mut d, &s, "/strip/1/mute", vec![Arg::Bool(true)]).is_empty());
    assert_eq!(
        act(&mut d, &s, "/strip/1/mute", vec![Arg::Bool(false)]),
        [SurfaceAction::ToggleMute(0)]
    );
    assert_eq!(
        act(&mut d, &s, "/strip/1/mute", vec![]),
        [SurfaceAction::ToggleMute(0)]
    );
    assert_eq!(
        act(&mut d, &s, "/strip/2/solo", vec![Arg::Float(1.0)]),
        [SurfaceAction::ToggleSolo(1)]
    );
    assert_eq!(
        act(&mut d, &s, "/strip/2/arm", vec![]),
        [SurfaceAction::ToggleArm(1)]
    );
    assert_eq!(
        act(&mut d, &s, "/strip/2/select", vec![Arg::Int(1)]),
        [SurfaceAction::Select(1)]
    );
    assert!(act(&mut d, &s, "/strip/2/select", vec![Arg::Int(0)]).is_empty());
}

#[test]
fn master_transport_jog_functions_and_device_parameters() {
    let mut d = Osc::new();
    let mut s = state(1);
    assert_eq!(
        act(&mut d, &s, "/master/fader", vec![Arg::Float(0.5)]),
        [SurfaceAction::MasterFader(0.5)]
    );
    assert_eq!(
        act(&mut d, &s, "/master/touch", vec![Arg::Int(1)]),
        [SurfaceAction::MasterTouch(true)]
    );
    for (key, button) in [
        ("stop", TransportButton::Stop),
        ("rewind", TransportButton::Rewind),
        ("forward", TransportButton::FastForward),
        ("play", TransportButton::Play),
        ("record", TransportButton::Record),
        ("cycle", TransportButton::Cycle),
    ] {
        assert_eq!(
            act(&mut d, &s, &format!("/transport/{key}"), vec![]),
            [SurfaceAction::Transport(button)],
            "{key}"
        );
    }
    // With an argument the switch is set, not flipped.
    assert!(act(&mut d, &s, "/transport/play", vec![Arg::Int(0)]).is_empty());
    assert_eq!(
        act(&mut d, &s, "/transport/play", vec![Arg::Int(1)]),
        [SurfaceAction::Transport(TransportButton::Play)]
    );
    s.transport.playing = true;
    assert!(act(&mut d, &s, "/transport/play", vec![Arg::Int(1)]).is_empty());
    assert!(act(&mut d, &s, "/transport/stop", vec![Arg::Int(0)]).is_empty());
    assert_eq!(
        act(&mut d, &s, "/jog", vec![Arg::Int(-3)]),
        [SurfaceAction::Jog(-3)]
    );
    assert_eq!(
        act(&mut d, &s, "/function/8", vec![]),
        [SurfaceAction::Function(8)]
    );
    assert!(act(&mut d, &s, "/function/9", vec![]).is_empty());
    assert_eq!(
        act(&mut d, &s, "/device/param/12", vec![Arg::Float(0.5)]),
        [SurfaceAction::Knob {
            knob: 11,
            change: KnobMove::To(0.5)
        }]
    );
    assert!(act(&mut d, &s, "/device/param/0", vec![Arg::Float(0.5)]).is_empty());
}

#[test]
fn a_bundle_gives_every_action_and_ping_is_answered() {
    let mut d = Osc::new();
    let s = state(2);
    let packet = osc_wire::encode_bundle(&[
        Message::with("/strip/1/fader", Arg::Float(0.1)),
        Message::with("/strip/2/fader", Arg::Float(0.2)),
    ]);
    let (mut actions, mut replies) = (Vec::new(), Vec::new());
    d.on_message(&packet, &s, &mut actions, &mut replies);
    assert_eq!(actions.len(), 2);
    let (_, replies) = send(
        &mut d,
        &s,
        &Message {
            address: "/ping".to_owned(),
            args: vec![Arg::Int(7)],
        },
    );
    let mut back = Vec::new();
    osc_wire::decode(&replies[0], &mut back).unwrap();
    assert_eq!(
        back,
        vec![Message::with("/pong", Arg::Int(7))],
        "the same arguments"
    );
    // Garbage is dropped without a word.
    let mut actions = Vec::new();
    d.on_message(b"\xFF\xFFgarbage", &s, &mut actions, &mut Vec::new());
    assert!(actions.is_empty());
}

#[test]
fn feedback_names_every_value_once_and_then_only_changes() {
    let mut d = Osc::new();
    let mut s = state(2);
    s.strips[1].mute = true;
    s.strips[1].meter = 0.5;
    s.transport = TransportLeds {
        playing: true,
        ..TransportLeds::default()
    };
    s.knobs = vec![Some(0.25), None];
    s.remote.id = Some("dev".to_owned());
    s.remote.name = "Chorus".to_owned();
    s.remote.params = vec!["Rate".to_owned(), "Depth".to_owned()];
    s.position.bar = 3;
    s.position.beat = 2;
    s.position.sixteenth = 4;
    let first = feedback(&mut d, &s);
    assert_eq!(
        value_of(&first, "/strip/1/name"),
        Some(&Arg::Str("Ch1".into()))
    );
    assert_eq!(value_of(&first, "/strip/1/fader"), Some(&Arg::Float(0.5)));
    assert_eq!(value_of(&first, "/strip/2/mute"), Some(&Arg::Int(1)));
    assert_eq!(value_of(&first, "/strip/1/mute"), Some(&Arg::Int(0)));
    assert_eq!(value_of(&first, "/strip/2/meter"), Some(&Arg::Float(0.5)));
    assert_eq!(value_of(&first, "/strip/1/pan"), Some(&Arg::Float(0.0)));
    assert_eq!(value_of(&first, "/master/fader"), Some(&Arg::Float(0.75)));
    assert_eq!(value_of(&first, "/transport/play"), Some(&Arg::Int(1)));
    assert_eq!(
        value_of(&first, "/transport/position"),
        Some(&Arg::Str("3.2.4".into()))
    );
    assert_eq!(
        value_of(&first, "/device/name"),
        Some(&Arg::Str("Chorus".into()))
    );
    assert_eq!(value_of(&first, "/device/param/1"), Some(&Arg::Float(0.25)));
    assert_eq!(
        value_of(&first, "/device/param/2/name"),
        Some(&Arg::Str("Depth".into()))
    );
    assert!(feedback(&mut d, &s).is_empty(), "nothing changed");
    s.strips[0].fader = 0.8;
    s.strips[0].meter = 0.2;
    let changed = feedback(&mut d, &s);
    let addresses: Vec<&str> = changed.iter().map(|m| m.address.as_str()).collect();
    assert_eq!(addresses, ["/strip/1/fader", "/strip/1/meter"]);
    // A change below the resolution of the wire is not sent.
    s.strips[0].fader = 0.8 + 1e-6;
    assert!(feedback(&mut d, &s).is_empty());
}

#[test]
fn a_new_client_or_a_refresh_gets_everything_again() {
    let mut d = Osc::new();
    let s = state(2);
    let mut out = Vec::new();
    d.connect(&s, &mut out);
    let all = feedback(&mut Osc::new(), &s).len();
    assert!(feedback(&mut d, &s).is_empty());
    for hello in ["/surface/hello", "/surface/refresh"] {
        send(&mut d, &s, &Message::new(hello));
        assert_eq!(feedback(&mut d, &s).len(), all, "{hello}");
    }
}

#[test]
fn many_strips_are_packed_into_datagrams_that_fit() {
    let mut d = Osc::new();
    let s = state(100);
    let mut out = Vec::new();
    d.feedback(&s, &mut out);
    assert!(out.len() > 1, "{} packets", out.len());
    assert!(out.iter().all(|p| p.len() <= PACKET_LIMIT));
}

#[test]
fn it_is_in_the_driver_list() {
    assert!(crate::surface::DRIVERS.iter().any(|(id, _)| *id == "osc"));
    let d = crate::surface::driver_by_id("osc").unwrap();
    assert_eq!(d.name(), "osc");
    assert!(d.needs().meters && !d.feeds_mappings());
}
