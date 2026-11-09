use super::*;
use atomik_midi_io::surface::{MasterState, StripState, TransportLeds};
use atomik_model::RelativeKind;

fn spec(role: Role, source: SourceKind, mode: ControlMode) -> ControlSpec {
    ControlSpec {
        role,
        source,
        channel: 0,
        mode,
        feedback: false,
    }
}

fn fb(mut c: ControlSpec) -> ControlSpec {
    c.feedback = true;
    c
}

fn profile(controls: Vec<ControlSpec>, output: bool) -> ControllerProfile {
    ControllerProfile {
        name: "Test".to_owned(),
        input: "In".to_owned(),
        output: output.then(|| "Out".to_owned()),
        controls,
    }
}

fn layout() -> Vec<ControlSpec> {
    vec![
        spec(Role::Knob(1), SourceKind::Cc(21), ControlMode::Absolute),
        spec(
            Role::Knob(2),
            SourceKind::Cc(22),
            ControlMode::Relative(RelativeKind::BinaryOffset),
        ),
        spec(Role::Fader(1), SourceKind::Cc(1), ControlMode::Absolute),
        spec(Role::Fader(2), SourceKind::Cc(2), ControlMode::Absolute),
        spec(Role::MasterFader, SourceKind::Cc(3), ControlMode::Absolute),
        spec(Role::Play, SourceKind::Note(94), ControlMode::Button),
        spec(Role::Stop, SourceKind::Note(93), ControlMode::Button),
        spec(Role::Record, SourceKind::Cc(117), ControlMode::Button),
        spec(Role::Loop, SourceKind::Note(95), ControlMode::Button),
        spec(Role::BankLeft, SourceKind::Note(91), ControlMode::Button),
        spec(Role::BankRight, SourceKind::Note(92), ControlMode::Button),
    ]
}

fn state(strips: usize) -> SurfaceState {
    SurfaceState {
        strips: (0..strips)
            .map(|i| StripState {
                name: format!("Ch{}", i + 1),
                fader: i as f32 / 100.0,
                ..StripState::default()
            })
            .collect(),
        master: Some(MasterState { fader: 0.5 }),
        transport: TransportLeds::default(),
        knobs: vec![Some(0.25), None],
        ..SurfaceState::default()
    }
}

fn send(d: &mut ProfileDriver, s: &SurfaceState, bytes: &[u8]) -> Vec<SurfaceAction> {
    let (mut actions, mut replies) = (Vec::new(), Vec::new());
    d.on_message(bytes, s, &mut actions, &mut replies);
    assert!(replies.is_empty());
    actions
}

fn driver() -> ProfileDriver {
    ProfileDriver::new(profile(layout(), false))
}

fn out_of(d: &mut ProfileDriver, s: &SurfaceState) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    d.feedback(s, &mut out);
    out
}

#[test]
fn an_absolute_knob_sets_a_position_and_an_encoder_turns() {
    let (mut d, s) = (driver(), state(4));
    assert_eq!(
        send(&mut d, &s, &[0xB0, 21, 127]),
        [SurfaceAction::Knob {
            knob: 0,
            change: KnobMove::To(1.0)
        }]
    );
    // Binary offset: 65 is one detent clockwise, 62 two counter-clockwise.
    assert_eq!(
        send(&mut d, &s, &[0xB0, 22, 65]),
        [SurfaceAction::Knob {
            knob: 1,
            change: KnobMove::By(DEFAULT_SENSITIVITY)
        }]
    );
    assert_eq!(
        send(&mut d, &s, &[0xB0, 22, 62]),
        [SurfaceAction::Knob {
            knob: 1,
            change: KnobMove::By(-2.0 * DEFAULT_SENSITIVITY)
        }]
    );
    // Another channel or another controller is not this knob.
    assert!(send(&mut d, &s, &[0xB1, 21, 10]).is_empty());
    assert!(send(&mut d, &s, &[0xB0, 99, 10]).is_empty());
}

#[test]
fn faders_are_the_strips_of_the_bank_and_bank_keys_move_by_eight() {
    let (mut d, s) = (driver(), state(20));
    assert_eq!(
        send(&mut d, &s, &[0xB0, 2, 127]),
        [SurfaceAction::Fader {
            strip: 1,
            value: 1.0
        }]
    );
    assert!(send(&mut d, &s, &[0x90, 92, 100]).is_empty());
    assert_eq!(d.bank(), 8);
    assert_eq!(
        send(&mut d, &s, &[0xB0, 1, 0]),
        [SurfaceAction::Fader {
            strip: 8,
            value: 0.0
        }]
    );
    send(&mut d, &s, &[0x90, 92, 100]);
    assert_eq!(d.bank(), 16);
    // 20 strips: the last page starts at 16; one more press stays there.
    send(&mut d, &s, &[0x90, 92, 100]);
    assert_eq!(d.bank(), 16);
    // Fader 2 would be strip 17, fader 1 strip 16: both exist; with 17 strips fader 2 is past the end.
    let few = state(17);
    assert!(send(&mut d, &few, &[0xB0, 2, 50]).is_empty());
    assert_eq!(send(&mut d, &few, &[0xB0, 1, 50]).len(), 1);
    send(&mut d, &s, &[0x90, 91, 100]);
    assert_eq!(d.bank(), 8);
    send(&mut d, &s, &[0x90, 91, 100]);
    send(&mut d, &s, &[0x90, 91, 100]);
    assert_eq!(d.bank(), 0);
    // A key release is not a press.
    assert!(send(&mut d, &s, &[0x80, 92, 0]).is_empty());
    assert_eq!(d.bank(), 0);
}

#[test]
fn the_master_fader_and_the_transport_keys() {
    let (mut d, s) = (driver(), state(2));
    assert_eq!(
        send(&mut d, &s, &[0xB0, 3, 127]),
        [SurfaceAction::MasterFader(1.0)]
    );
    for (bytes, button) in [
        ([0x90, 94, 100], TransportButton::Play),
        ([0x90, 93, 100], TransportButton::Stop),
        ([0xB0, 117, 127], TransportButton::Record),
        ([0x90, 95, 100], TransportButton::Cycle),
    ] {
        assert_eq!(send(&mut d, &s, &bytes), [SurfaceAction::Transport(button)]);
    }
    // The release of a controller key is not a press.
    assert!(send(&mut d, &s, &[0xB0, 117, 0]).is_empty());
    assert!(send(&mut d, &s, &[0x80, 94, 0]).is_empty());
}

#[test]
fn a_fader_that_is_an_encoder_moves_from_where_the_strip_is() {
    let mut controls = layout();
    controls[2] = spec(
        Role::Fader(1),
        SourceKind::Cc(1),
        ControlMode::Relative(RelativeKind::TwosComplement),
    );
    let mut d = ProfileDriver::new(profile(controls, false));
    let mut s = state(4);
    s.strips[0].fader = 0.5;
    assert_eq!(
        send(&mut d, &s, &[0xB0, 1, 2]),
        [SurfaceAction::Fader {
            strip: 0,
            value: 0.5 + 2.0 * DEFAULT_SENSITIVITY
        }]
    );
    s.strips[0].fader = 0.0;
    assert_eq!(
        send(&mut d, &s, &[0xB0, 1, 127]),
        [SurfaceAction::Fader {
            strip: 0,
            value: 0.0
        }]
    );
}

#[test]
fn pitch_bend_nrpn_and_14_bit_controls_are_read() {
    let controls = vec![
        spec(Role::Fader(1), SourceKind::PitchBend, ControlMode::Absolute),
        spec(Role::Knob(1), SourceKind::Nrpn(300), ControlMode::Absolute),
        spec(
            Role::Knob(2),
            SourceKind::Nrpn(301),
            ControlMode::Relative(RelativeKind::TwosComplement),
        ),
        spec(Role::Knob(3), SourceKind::Cc14(4), ControlMode::Absolute),
        spec(Role::Loop, SourceKind::Nrpn(9), ControlMode::Button),
    ];
    let mut d = ProfileDriver::new(profile(controls, false));
    let s = state(4);
    assert_eq!(
        send(&mut d, &s, &[0xE0, 0x7F, 0x7F]),
        [SurfaceAction::Fader {
            strip: 0,
            value: 1.0
        }]
    );
    // NRPN 300 = MSB 2, LSB 44; value 16383.
    for b in [[0xB0, 99, 2], [0xB0, 98, 44], [0xB0, 6, 127]] {
        assert!(send(&mut d, &s, &b).len() <= 1);
    }
    let knob = send(&mut d, &s, &[0xB0, 38, 127]);
    assert_eq!(
        knob,
        [SurfaceAction::Knob {
            knob: 0,
            change: KnobMove::To(1.0)
        }]
    );
    // NRPN 301: data increment is one detent clockwise.
    send(&mut d, &s, &[0xB0, 99, 2]);
    send(&mut d, &s, &[0xB0, 98, 45]);
    assert_eq!(
        send(&mut d, &s, &[0xB0, 96, 1]),
        [SurfaceAction::Knob {
            knob: 1,
            change: KnobMove::By(DEFAULT_SENSITIVITY)
        }]
    );
    // 14 bit: MSB 64 and LSB 0 is the middle.
    send(&mut d, &s, &[0xB0, 4, 64]);
    let mid = send(&mut d, &s, &[0xB0, 36, 0]);
    let [
        SurfaceAction::Knob {
            knob: 2,
            change: KnobMove::To(v),
        },
    ] = mid[..]
    else {
        panic!("{mid:?}");
    };
    assert!((v - 8192.0 / 16383.0).abs() < 1e-6);
    // An NRPN key.
    for b in [[0xB0, 99, 0], [0xB0, 98, 9], [0xB0, 6, 127]] {
        send(&mut d, &s, &b);
    }
    assert!(
        send(&mut d, &s, &[0xB0, 38, 127])
            .contains(&SurfaceAction::Transport(TransportButton::Cycle))
    );
}

#[test]
fn running_status_is_followed() {
    let (mut d, s) = (driver(), state(4));
    assert_eq!(send(&mut d, &s, &[0xB0, 21, 127]).len(), 1);
    assert_eq!(send(&mut d, &s, &[21, 0]).len(), 1);
}

fn lit() -> Vec<ControlSpec> {
    vec![
        fb(spec(
            Role::Knob(1),
            SourceKind::Cc(21),
            ControlMode::Absolute,
        )),
        fb(spec(
            Role::Knob(2),
            SourceKind::Nrpn(300),
            ControlMode::Absolute,
        )),
        fb(spec(
            Role::Fader(1),
            SourceKind::PitchBend,
            ControlMode::Absolute,
        )),
        fb(spec(
            Role::Fader(2),
            SourceKind::Cc14(1),
            ControlMode::Absolute,
        )),
        fb(spec(Role::Play, SourceKind::Note(94), ControlMode::Button)),
        fb(spec(Role::Stop, SourceKind::Cc(116), ControlMode::Button)),
        spec(Role::Record, SourceKind::Note(93), ControlMode::Button),
        spec(Role::BankRight, SourceKind::Note(92), ControlMode::Button),
    ]
}

#[test]
fn feedback_shows_values_with_the_exact_bytes() {
    let mut d = ProfileDriver::new(profile(lit(), true));
    let mut s = state(4);
    s.strips[0].fader = 1.0;
    s.strips[1].fader = 0.5;
    s.knobs = vec![Some(1.0), Some(0.5)];
    s.transport.playing = true;
    let mut out = Vec::new();
    d.connect(&s, &mut out);
    assert_eq!(
        out,
        vec![
            vec![0xB0, 21, 127],
            // NRPN 300 (MSB 2, LSB 44), value 0.5 = 8192 (MSB 64, LSB 0).
            vec![0xB0, 99, 2],
            vec![0xB0, 98, 44],
            vec![0xB0, 6, 64],
            vec![0xB0, 38, 0],
            // Pitch bend, full.
            vec![0xE0, 0x7F, 0x7F],
            // 14 bit controller pair 1 and 33, value 8192.
            vec![0xB0, 1, 64],
            vec![0xB0, 33, 0],
            // Play lit, Stop dark.
            vec![0x90, 94, 127],
            vec![0xB0, 116, 0],
        ]
    );
    // Nothing changed: nothing is sent, and a control without feedback never is.
    assert!(out_of(&mut d, &s).is_empty());
    s.transport.recording = true;
    assert!(out_of(&mut d, &s).is_empty());
}

#[test]
fn feedback_sends_only_what_changed() {
    let mut d = ProfileDriver::new(profile(lit(), true));
    let mut s = state(4);
    let mut out = Vec::new();
    d.connect(&s, &mut out);
    s.strips[0].fader = 0.25;
    s.transport.playing = true;
    let sent = out_of(&mut d, &s);
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert!(sent.contains(&vec![0x90, 94, 127]));
    assert!(sent.contains(&vec![0xB0, 116, 0]));
    // Stop went from lit to dark; the bend value is 0.25 of 16383.
    assert!(sent.contains(&vec![0xE0, 0x00, 0x20]));
    // Play going dark is a note off.
    s.transport.playing = false;
    assert!(out_of(&mut d, &s).contains(&vec![0x80, 94, 0]));
}

#[test]
fn a_bank_change_shows_the_other_strips() {
    let mut d = ProfileDriver::new(profile(lit(), true));
    let s = state(20);
    let mut out = Vec::new();
    d.connect(&s, &mut out);
    send(&mut d, &s, &[0x90, 92, 100]);
    let sent = out_of(&mut d, &s);
    // Strip 8 and 9 (0.08 and 0.09) replace strips 0 and 1.
    assert!(sent.contains(&vec![0xE0, 0x1F, 0x0A]), "{sent:?}");
    assert_eq!(sent.len(), 3, "{sent:?}");
    // Past the last strip a fader shows zero.
    let few = state(9);
    send(&mut d, &few, &[0x90, 92, 100]);
    let past = out_of(&mut d, &few);
    assert!(past.contains(&vec![0xB0, 1, 0]) || past.contains(&vec![0xE0, 0, 0]));
}

#[test]
fn a_control_the_hand_is_on_is_left_alone_for_a_moment() {
    let controls = vec![fb(spec(
        Role::Fader(1),
        SourceKind::Cc(1),
        ControlMode::Absolute,
    ))];
    let mut d = ProfileDriver::new(profile(controls, true));
    let mut s = state(4);
    let mut out = Vec::new();
    d.connect(&s, &mut out);
    // The hand moves the fader; the project follows (the new value) and the feedback waits.
    send(&mut d, &s, &[0xB0, 1, 100]);
    s.strips[0].fader = 100.0 / 127.0;
    for _ in 0..QUIET_PASSES {
        assert!(out_of(&mut d, &s).is_empty());
    }
    // Then the value is confirmed (a motor ends where the hand left it).
    assert_eq!(out_of(&mut d, &s), [vec![0xB0, 1, 100]]);
    assert!(out_of(&mut d, &s).is_empty());
}

#[test]
fn no_output_port_means_no_feedback_whatever_the_profile_says() {
    let mut d = ProfileDriver::new(profile(lit(), false));
    let mut out = Vec::new();
    d.connect(&state(4), &mut out);
    assert!(out.is_empty());
}

#[test]
fn the_driver_is_not_a_mapping_feed_and_is_named() {
    let d = driver();
    assert!(!d.feeds_mappings());
    assert_eq!(d.name(), "profile");
}

#[test]
fn feedback_messages_read_back_as_the_same_control() {
    // What the driver sends to a control is what the same control sends: a round trip through
    // the reader gives the value back.
    for source in [
        SourceKind::Cc(21),
        SourceKind::Cc14(3),
        SourceKind::PitchBend,
        SourceKind::Nrpn(1000),
    ] {
        let c = spec(Role::Knob(1), source, ControlMode::Absolute);
        let (_, messages) = encode(&c, 0.5);
        let mut d = ProfileDriver::new(profile(vec![c], false));
        let s = state(1);
        let mut last = Vec::new();
        for m in &messages {
            last = send(&mut d, &s, m);
        }
        let [
            SurfaceAction::Knob {
                change: KnobMove::To(v),
                ..
            },
        ] = last[..]
        else {
            panic!("{source:?}: {last:?}");
        };
        assert!((v - 0.5).abs() < 0.01, "{source:?}: {v}");
    }
}
