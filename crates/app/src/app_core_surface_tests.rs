//! A control surface in the core: a loopback surface with the eight-fader driver, the console
//! as its strips, what its faders, V-pots, keys and wheel do to the project, the feedback it
//! gets (motors, LEDs, rings, displays), banking, touch and the throttle.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use crate::surface::{LoopbackSurface, SurfaceHost};
use atomik_midi_io::surface::Mcu;
use atomik_midi_io::surface::mcu::{fader_to_bend, lcd_message};
use atomik_model::ConsoleCommand;

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// `n` synths named `s1`... with their channels (`channel`, `channel-2`, ...).
fn core(n: usize) -> (AppCore<RecordingSink>, LoopbackSurface) {
    let mut c = AppCore::new(RecordingSink::new());
    for i in 1..=n {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(&format!("s{i}"))),
        })
        .unwrap();
        c.apply_command(Command::Rename {
            key: key(&format!("s{i}")),
            name: format!("Synth {i}"),
        })
        .unwrap();
    }
    let surface = LoopbackSurface::new();
    c.set_surface(Some(SurfaceHost::new(
        Box::new(Mcu::new()),
        Box::new(surface.clone()),
    )));
    c.sink_mut().take();
    (c, surface)
}

fn channel(i: usize) -> DeviceKey {
    key(&if i == 1 {
        "channel".to_owned()
    } else {
        format!("channel-{i}")
    })
}

fn value(c: &AppCore<RecordingSink>, device: &DeviceKey, p: &str) -> f32 {
    c.doc().param_value(device, p, c.registry()).unwrap()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// The first frame connects; later frames are due `0.1` s apart.
struct Clock(f64);

impl Clock {
    fn frame(&mut self, c: &mut AppCore<RecordingSink>) {
        self.0 += 0.1;
        c.surface_frame(self.0);
    }
}

fn bend(ch: u8, v: u16) -> Vec<u8> {
    atomik_core::ChannelMessage::pitch_bend(ch, v)
        .to_bytes()
        .to_vec()
}

#[test]
fn the_first_frame_connects_and_shows_the_console() {
    let (mut c, s) = core(2);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    let sent = s.take_sent();
    assert_eq!(sent[0], vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x00, 0xF7]);
    // The scribble strips carry the instruments' names, six characters and a space.
    assert!(sent.contains(&lcd_message(0, b"Synth  ")));
    assert!(sent.contains(&lcd_message(7, b"Synth  ")));
    // Faders of both channels at the unity position and the master.
    let unity = fader_to_bend(0.75);
    let msg = vec![0xE0, (unity & 0x7F) as u8, (unity >> 7) as u8];
    assert!(sent.contains(&msg));
    // Nothing changed: nothing more is sent.
    t.frame(&mut c);
    assert!(s.take_sent().is_empty());
    assert!(c.surface().unwrap().sent() > 20);
}

#[test]
fn the_strips_are_in_console_order_channels_then_buses_then_vca_groups() {
    let (mut c, _s) = core(2);
    c.apply_command(Command::Console(ConsoleCommand::CreateBus {
        name: "Drums".into(),
        dest: atomik_model::console::OutputDest::Master,
    }))
    .unwrap();
    c.apply_command(Command::Console(ConsoleCommand::CreateVca {
        name: "All".into(),
        members: vec![channel(1)],
    }))
    .unwrap();
    let (state, refs) = c.surface_view();
    let names: Vec<&str> = state.strips.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Synth 1", "Synth 2", "Drums", "All"]);
    let kinds: Vec<StripKind> = state.strips.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            StripKind::Channel,
            StripKind::Channel,
            StripKind::Bus,
            StripKind::Vca
        ]
    );
    assert_eq!(refs[3], StripRef::Vca(1));
    assert!(state.master.is_some());
    // Moving a channel in the console moves it on the surface.
    c.apply_command(Command::Console(ConsoleCommand::MoveChannel {
        channel: channel(2),
        to: 0,
    }))
    .unwrap();
    let (state, _) = c.surface_view();
    assert_eq!(state.strips[0].name, "Synth 2");
}

#[test]
fn a_fader_moves_the_console_fader_and_the_motor_follows_the_project() {
    let (mut c, s) = core(2);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    s.take_sent();
    // The surface sends 12000 on fader 1 (strip 2).
    s.feed(&bend(1, 12_000));
    t.frame(&mut c);
    let want = 12_000.0 / 16_383.0;
    assert!(near(value(&c, &channel(2), "fader"), want));
    assert!(
        near(value(&c, &channel(1), "fader"), 0.75),
        "only that strip"
    );
    assert!(
        c.sink()
            .calls
            .iter()
            .any(|call| matches!(call, SinkCall::Param { .. })),
        "the engine heard it"
    );
    // The surface is where it was put: no echo...
    assert!(s.take_sent().iter().all(|m| m[0] != 0xE1), "no echo");
    // ...but a move from the mouse (or automation) drives the motor.
    c.apply_command(Command::SetParam {
        key: channel(2),
        param: "fader".into(),
        value: 0.25,
        merge: None,
    })
    .unwrap();
    t.frame(&mut c);
    let motor = fader_to_bend(0.25);
    assert_eq!(
        s.take_sent()
            .into_iter()
            .filter(|m| m[0] == 0xE1)
            .collect::<Vec<_>>(),
        vec![vec![0xE1, (motor & 0x7F) as u8, (motor >> 7) as u8]]
    );
}

#[test]
fn a_fader_round_trip_keeps_the_wire_value() {
    let (mut c, s) = core(1);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    for v in [0u16, 1, 5000, 8192, 16_383] {
        s.take_sent();
        s.feed(&bend(0, v));
        t.frame(&mut c);
        // Read the project back through the surface's own view of it.
        let (state, _) = c.surface_view();
        assert_eq!(fader_to_bend(state.strips[0].fader), v, "{v}");
    }
}

#[test]
fn a_touched_fader_is_one_undo_step_and_the_release_ends_it() {
    let (mut c, s) = core(1);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    s.feed(&[0x90, 0x68, 0x7F]);
    t.frame(&mut c);
    // A sweep with the finger on the fader, longer than the gesture timeout.
    for (i, v) in [2000u16, 4000, 6000].into_iter().enumerate() {
        s.feed(&bend(0, v));
        c.surface_frame(10.0 + i as f64);
    }
    assert!(near(value(&c, &channel(1), "fader"), 6000.0 / 16_383.0));
    s.feed(&[0x90, 0x68, 0x00]);
    c.surface_frame(20.0);
    // A new move after the release is another step.
    s.feed(&bend(0, 9000));
    c.surface_frame(21.0);
    assert!(c.undo());
    assert!(
        near(value(&c, &channel(1), "fader"), 6000.0 / 16_383.0),
        "the second move"
    );
    assert!(c.undo());
    assert!(
        near(value(&c, &channel(1), "fader"), 0.75),
        "the whole sweep, held through any pause"
    );
}

#[test]
fn v_pots_turn_the_pan_and_the_ring_shows_it() {
    let (mut c, s) = core(1);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    s.take_sent();
    // 8 detents clockwise: 8/32 of the pan range (the range is -1..1: 0.5 wide per 0.25).
    s.feed(&[0xB0, 0x10, 0x08]);
    t.frame(&mut c);
    let pan = value(&c, &channel(1), "pan");
    assert!(near(pan, 0.5), "{pan}");
    let ring = vec![
        0xB0,
        0x30,
        atomik_midi_io::surface::mcu::ring_value(Some(pan)),
    ];
    let sent = s.take_sent();
    assert!(sent.contains(&ring), "{sent:?}");
    // The lower scribble row shows the pan as text.
    assert!(sent.contains(&lcd_message(0x38, b"R50    ")));
    // Counter-clockwise.
    s.feed(&[0xB0, 0x10, 0x48]);
    t.frame(&mut c);
    assert!(near(value(&c, &channel(1), "pan"), 0.0));
    assert!(s.take_sent().contains(&lcd_message(0x38, b"C      ")));
}

#[test]
fn mute_solo_record_and_select_keys_act_and_their_leds_follow() {
    let (mut c, s) = core(2);
    let mut t = Clock(0.0);
    // The synths have tracks, so the record key has something to arm.
    for k in ["s1", "s2"] {
        c.apply_command(Command::Song(SongCommand::AddTrack { device: key(k) }))
            .unwrap();
    }
    t.frame(&mut c);
    s.take_sent();
    s.feed(&[0x90, 0x10, 0x7F]); // mute 1
    s.feed(&[0x90, 0x09, 0x7F]); // solo 2
    s.feed(&[0x90, 0x01, 0x7F]); // rec 2
    s.feed(&[0x90, 0x18, 0x7F]); // select 1
    t.frame(&mut c);
    assert!(near(value(&c, &channel(1), "mute"), 1.0));
    assert!(near(value(&c, &channel(2), "solo"), 1.0));
    assert!(c.doc().song.track(&key("s2")).unwrap().armed);
    assert_eq!(c.console().selected_strips(), vec![channel(1)]);
    t.frame(&mut c);
    let sent = s.take_sent();
    for led in [
        [0x90u8, 0x10, 0x7F],
        [0x90, 0x09, 0x7F],
        [0x90, 0x01, 0x7F],
        [0x90, 0x18, 0x7F],
    ] {
        assert!(sent.contains(&led.to_vec()), "{led:?} in {sent:?}");
    }
    // Pressing mute again unmutes; LEDs go dark.
    s.feed(&[0x90, 0x10, 0x7F]);
    t.frame(&mut c);
    assert!(near(value(&c, &channel(1), "mute"), 0.0));
    t.frame(&mut c);
    assert!(s.take_sent().contains(&vec![0x90, 0x10, 0x00]));
}

#[test]
fn banks_move_by_eight_and_address_the_strips_of_the_bank() {
    let (mut c, s) = core(10);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    s.take_sent();
    s.feed(&[0x90, 0x2F, 0x7F]); // bank right
    t.frame(&mut c);
    let sent = s.take_sent();
    // The first fader now shows strip 9 (Synth 9) and the assignment display says 09.
    assert!(sent.contains(&lcd_message(0, b"Synth  ")));
    assert!(sent.contains(&vec![0xB0, 0x4A, b'9' & 0x3F]));
    // The mute key of the first fader mutes channel 9.
    s.feed(&[0x90, 0x10, 0x7F]);
    t.frame(&mut c);
    assert!(near(value(&c, &channel(9), "mute"), 1.0));
    assert!(near(value(&c, &channel(1), "mute"), 0.0));
    // Fader 3 of this bank has no strip: its messages do nothing.
    s.feed(&bend(2, 100));
    t.frame(&mut c);
    for i in 1..=10 {
        assert!(value(&c, &channel(i), "fader") >= 0.7, "strip {i}");
    }
    // Bank left returns.
    s.feed(&[0x90, 0x2E, 0x7F]);
    t.frame(&mut c);
    s.feed(&[0x90, 0x10, 0x7F]);
    t.frame(&mut c);
    assert!(near(value(&c, &channel(1), "mute"), 1.0));
}

#[test]
fn the_transport_keys_are_the_transport_and_their_lamps_follow() {
    let (mut c, s) = core(1);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    s.take_sent();
    s.feed(&[0x90, 0x5E, 0x7F]); // play
    t.frame(&mut c);
    assert!(c.is_playing());
    t.frame(&mut c);
    let sent = s.take_sent();
    assert!(sent.contains(&vec![0x90, 0x5E, 0x7F]));
    assert!(sent.contains(&vec![0x90, 0x5D, 0x00]), "stop lamp off");
    s.feed(&[0x90, 0x5D, 0x7F]); // stop
    t.frame(&mut c);
    assert!(!c.is_playing());
    // Cycle toggles the loop.
    let before = c.doc().song.loop_enabled;
    s.feed(&[0x90, 0x56, 0x7F]);
    t.frame(&mut c);
    assert_ne!(c.doc().song.loop_enabled, before);
    t.frame(&mut c);
    assert!(s.take_sent().contains(&vec![0x90, 0x56, 0x7F]));
    // Record starts a take when a track is armed.
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key("s1") }))
        .unwrap();
    c.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: key("s1"),
        muted: false,
        solo: false,
        armed: true,
    }))
    .unwrap();
    s.feed(&[0x90, 0x5F, 0x7F]);
    t.frame(&mut c);
    assert!(c.is_recording());
}

#[test]
fn rewind_and_fast_forward_move_a_bar_and_the_jog_wheel_a_sixteenth() {
    let (mut c, s) = core(1);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    let bar = 4 * atomik_core::PPQ;
    s.feed(&[0x90, 0x5C, 0x7F]); // fast forward
    t.frame(&mut c);
    assert_eq!(c.position(), bar);
    s.feed(&[0x90, 0x5C, 0x7F]);
    t.frame(&mut c);
    assert_eq!(c.position(), 2 * bar);
    s.feed(&[0x90, 0x5B, 0x7F]); // rewind
    t.frame(&mut c);
    assert_eq!(c.position(), bar);
    s.feed(&[0xB0, 0x3C, 0x03]); // jog 3 detents on
    t.frame(&mut c);
    assert_eq!(c.position(), bar + 3 * atomik_core::PPQ / 4);
    s.feed(&[0xB0, 0x3C, 0x4F]); // 15 detents back, twice: clamped at the start
    s.feed(&[0xB0, 0x3C, 0x4F]);
    t.frame(&mut c);
    assert_eq!(c.position(), 0);
    assert!(
        c.sink()
            .calls
            .iter()
            .any(|call| matches!(call, SinkCall::Transport(_)))
    );
}

#[test]
fn the_time_display_follows_the_position_in_beats_and_timecode() {
    let (mut c, s) = core(1);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    s.take_sent();
    // Bar 2, beat 1: tick 4 beats in.
    c.transport_action(TransportAction::Locate(4 * atomik_core::PPQ));
    t.frame(&mut c);
    let sent = s.take_sent();
    // 002.01.01.000: the last digit changed to 0 (it was 0), the bar digit to 2.
    assert!(
        sent.contains(&vec![0xB0, 0x47, (b'2' & 0x3F) | 0x40]),
        "{sent:?}"
    );
    // At 120 BPM a bar is two seconds: switch to timecode and see 00:00:02:00.
    s.feed(&[0x90, 0x35, 0x7F]);
    t.frame(&mut c);
    t.frame(&mut c);
    let sent = s.take_sent();
    assert!(
        sent.contains(&vec![0xB0, 0x43, (b'2' & 0x3F) | 0x40]),
        "{sent:?}"
    );
}

#[test]
fn the_master_fader_and_its_touch_drive_the_master() {
    let (mut c, s) = core(1);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    let master = c.doc().console.master.clone().unwrap();
    s.feed(&bend(8, 16_383));
    t.frame(&mut c);
    assert!(near(value(&c, &master, "fader"), 1.0));
    s.feed(&[0x90, 0x70, 0x7F]);
    s.feed(&bend(8, 0));
    t.frame(&mut c);
    s.feed(&[0x90, 0x70, 0x00]);
    t.frame(&mut c);
    assert!(near(value(&c, &master, "fader"), 0.0));
}

#[test]
fn feedback_is_throttled_to_thirty_passes_a_second() {
    let (mut c, s) = core(1);
    c.surface_frame(0.0);
    s.take_sent();
    c.apply_command(Command::SetParam {
        key: channel(1),
        param: "mute".into(),
        value: 1.0,
        merge: None,
    })
    .unwrap();
    // Too soon after the connect pass: held back.
    c.surface_frame(0.01);
    assert!(s.take_sent().is_empty());
    c.surface_frame(0.02);
    assert!(s.take_sent().is_empty());
    // Due: the change goes out once, then nothing more.
    c.surface_frame(0.05);
    assert_eq!(s.take_sent(), vec![vec![0x90, 0x10, 0x7F]]);
    c.surface_frame(0.2);
    assert!(s.take_sent().is_empty());
}

#[test]
fn a_connection_query_from_the_surface_is_answered_and_the_surface_is_shown_again() {
    let (mut c, s) = core(1);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    s.take_sent();
    let mut query = vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x01];
    query.extend_from_slice(b"ATOMIKS");
    query.extend_from_slice(&[1, 2, 3, 4, 0xF7]);
    s.feed(&query);
    t.frame(&mut c);
    let sent = s.take_sent();
    assert_eq!(sent[0][5], 0x02, "the reply comes first");
    assert!(sent.len() > 20, "and the whole surface after it");
}

#[test]
fn removing_the_surface_lets_it_go_offline_and_a_core_without_one_ignores_frames() {
    let (mut c, s) = core(1);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    s.take_sent();
    c.set_surface(None);
    assert_eq!(
        s.take_sent(),
        vec![vec![0xF0, 0x00, 0x00, 0x66, 0x14, 0x0F, 0xF7]]
    );
    assert!(c.surface().is_none());
    c.surface_frame(5.0);
    assert!(s.take_sent().is_empty());
}
