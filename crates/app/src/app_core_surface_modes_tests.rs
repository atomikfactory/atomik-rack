//! The eight-fader surface beyond faders and pans: level meters, Flip, what the V-pots drive
//! (sends, device parameters), the function keys and an extender unit.

use super::*;
use crate::sink::RecordingSink;
use crate::surface::LoopbackSurface;
use atomik_midi_io::surface::Mcu;
use atomik_ui::ChannelMeters;

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
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

fn project(n: usize) -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    for i in 1..=n {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(&format!("s{i}"))),
        })
        .unwrap();
    }
    c.sink_mut().take();
    c
}

/// A core with the main unit of an eight-fader surface.
fn core(n: usize) -> (AppCore<RecordingSink>, LoopbackSurface) {
    let mut c = project(n);
    let s = c.add_loopback_driver("Desk", Box::new(Mcu::new()));
    (c, s)
}

struct Clock(f64);

impl Clock {
    fn frame(&mut self, c: &mut AppCore<RecordingSink>) {
        self.0 += 0.1;
        c.surface_frame(self.0);
    }
}

fn press(s: &LoopbackSurface, note: u8) {
    s.feed(&[0x90, note, 0x7F]);
    s.feed(&[0x90, note, 0x00]);
}

fn bend(ch: u8, v: u16) -> Vec<u8> {
    atomik_core::ChannelMessage::pitch_bend(ch, v)
        .to_bytes()
        .to_vec()
}

#[test]
fn the_state_carries_meters_and_a_send_only_when_the_driver_asks() {
    let (mut c, _s) = core(2);
    let peak = ChannelMeters {
        out_peak_l: 0.5,
        out_peak_r: 0.25,
        out_rms_l: 0.1,
        out_rms_r: 0.1,
        gr: 1.0,
    };
    c.console_mut().set_meters_of([(&channel(1), peak)]);
    c.console_mut().set_frame_time(1.0);
    let (plain, _) = c.surface_view_with(Needs::default());
    assert!(plain.strips.iter().all(|s| s.meter < f32::EPSILON));
    assert!(plain.strips[0].send.is_none());
    let (state, _) = c.surface_view_with(Needs {
        meters: true,
        send: Some(1),
    });
    // 0.5 is -6 dB: 54 of the 60 dB of the meter.
    assert!(
        near(state.strips[0].meter, 0.9),
        "{}",
        state.strips[0].meter
    );
    assert!(state.strips[1].meter < 0.01, "a silent strip");
    // The first send exists on a channel strip: its level and its text.
    let send = state.strips[0].send.expect("send 1");
    assert!((0.0..=1.0).contains(&send));
    assert!(state.strips[0].send_text.ends_with("dB"));
    let (none, _) = c.surface_view_with(Needs {
        meters: false,
        send: Some(9),
    });
    assert!(none.strips[0].send.is_none(), "there is no send 9");
}

#[test]
fn a_surface_with_meters_has_the_console_meters_read_even_with_the_console_hidden() {
    let (mut c, _s) = core(1);
    assert!(!c.surfaces_want_meters(), "not before it is connected");
    Clock(0.0).frame(&mut c);
    assert!(c.surfaces_want_meters());
    let mut plain = project(1);
    plain.add_loopback_surface("g", "generic").unwrap();
    Clock(0.0).frame(&mut plain);
    assert!(
        !plain.surfaces_want_meters(),
        "a generic controller shows none"
    );
}

#[test]
fn the_meters_go_to_the_unit_as_channel_pressure() {
    let (mut c, s) = core(1);
    let peak = ChannelMeters {
        out_peak_l: 1.0,
        out_peak_r: 1.0,
        out_rms_l: 0.5,
        out_rms_r: 0.5,
        gr: 1.0,
    };
    c.console_mut().set_meters_of([(&channel(1), peak)]);
    c.console_mut().set_frame_time(1.0);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    let sent = s.take_sent();
    assert!(sent.contains(&vec![0xD0, 0x0C]), "a full scale peak clips");
}

#[test]
fn flip_makes_a_fader_set_the_pan_and_a_v_pot_the_level() {
    let (mut c, s) = core(1);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    press(&s, 0x32);
    t.frame(&mut c);
    // Fader 0 to the top is a hard right pan; the fader of the channel is left alone.
    let level = value(&c, &channel(1), "fader");
    s.feed(&bend(0, 16_383));
    t.frame(&mut c);
    assert!(near(value(&c, &channel(1), "pan"), 1.0));
    assert!(near(value(&c, &channel(1), "fader"), level));
    // A V-pot turned clockwise raises the level by its detents.
    s.feed(&[0xB0, 0x10, 0x04]);
    t.frame(&mut c);
    let (state, _) = c.surface_view();
    assert!(state.strips[0].fader > 0.0);
    assert!(value(&c, &channel(1), "fader") > level);
}

#[test]
fn the_send_mode_turns_the_level_of_one_send() {
    let (mut c, s) = core(1);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    press(&s, 0x29);
    t.frame(&mut c);
    let before = value(&c, &channel(1), "send1.level");
    s.feed(&[0xB0, 0x10, 0x08]);
    t.frame(&mut c);
    assert!(value(&c, &channel(1), "send1.level") > before);
    // The next press of the key takes the second send.
    let second = value(&c, &channel(1), "send2.level");
    press(&s, 0x29);
    s.feed(&[0xB0, 0x10, 0x08]);
    t.frame(&mut c);
    assert!(value(&c, &channel(1), "send2.level") > second);
    // The lower row of the strip shows the send.
    let shown = s.take_sent();
    assert!(
        shown
            .iter()
            .any(|m| m.starts_with(&[0xF0, 0, 0, 0x66, 0x14, 0x12, 0x38])),
        "the lower display row was written"
    );
}

#[test]
fn the_plug_in_mode_turns_the_parameters_of_the_followed_device() {
    let (mut c, s) = core(1);
    let device = key("s1");
    c.select(Some(device.clone()));
    let mut t = Clock(0.0);
    t.frame(&mut c);
    press(&s, 0x2B);
    t.frame(&mut c);
    let first = c.remote_param(&device, 0).expect("a parameter");
    let before = c.target_value(&first).unwrap();
    s.feed(&[0xB0, 0x10, 0x05]);
    t.frame(&mut c);
    let after = c.target_value(&first).unwrap();
    assert!(
        (after - before).abs() > 1e-4,
        "parameter 1 moved: {before} -> {after}"
    );
}

#[test]
fn a_function_key_queues_the_command_bound_to_it() {
    let mut c = project(1);
    let s = LoopbackSurface::new();
    c.set_surface(Some(
        crate::surface::SurfaceHost::new(Box::new(Mcu::new()), Box::new(s.clone()))
            .named("Desk", "Desk")
            .with_functions(vec![
                "transport.play".to_owned(),
                String::new(),
                "window.song=2".to_owned(),
            ]),
    ));
    let mut t = Clock(0.0);
    t.frame(&mut c);
    press(&s, 0x36);
    press(&s, 0x38);
    t.frame(&mut c);
    assert_eq!(
        c.take_surface_commands(),
        ["transport.play", "window.song=2"]
    );
    assert!(c.take_surface_commands().is_empty(), "taken once");
    // F2 has no command: the user is told, nothing is queued.
    press(&s, 0x37);
    t.frame(&mut c);
    assert!(c.take_surface_commands().is_empty());
    let message = c.rejection.clone().expect("a message");
    assert!(message.contains("F2"), "{message}");
}

#[test]
fn an_extender_shows_the_strips_after_the_main_unit_and_moves_with_its_bank() {
    let mut c = project(12);
    let main = c.add_loopback_driver("Desk", Box::new(Mcu::new()));
    let ext = c.add_loopback_driver("Desk 2", Box::new(Mcu::extender()));
    let mut t = Clock(0.0);
    t.frame(&mut c);
    t.frame(&mut c);
    let _ = main.take_sent();
    let first = ext.take_sent();
    // The extender's first fader is strip 9 (index 8): fader 0 of the extender sets it.
    assert!(
        first.iter().any(|m| m[0] == 0xE0),
        "the extender's motors are driven: {first:?}"
    );
    ext.feed(&bend(0, 0));
    t.frame(&mut c);
    assert!(near(value(&c, &channel(9), "fader"), 0.0));
    assert!(
        !near(value(&c, &channel(1), "fader"), 0.0),
        "not strip 1: the extender has the strips after the main unit's"
    );
}
