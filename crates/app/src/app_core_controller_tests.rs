//! Controllers set up with a profile, running in the core: the knobs follow the selected device,
//! the faders follow the bank, the keys are the transport, the feedback follows the project, and
//! two controllers work at once.

use super::*;
use crate::controller_driver::ProfileDriver;
use crate::controller_profile::{ControlMode, ControlSpec, ControllerProfile, Role};
use crate::sink::RecordingSink;
use crate::surface::LoopbackSurface;
use atomik_model::{RelativeKind, SourceKind};

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn spec(role: Role, source: SourceKind, mode: ControlMode, feedback: bool) -> ControlSpec {
    ControlSpec {
        role,
        source,
        channel: 0,
        mode,
        feedback,
    }
}

/// A controller with eight knobs on CC 21..=28 (the second one an encoder), two faders on CC 1
/// and 2, the transport on notes 90..=93 and the bank keys on notes 94 and 95.
fn profile(name: &str, cc_base: u8, feedback: bool) -> ControllerProfile {
    let mut controls = Vec::new();
    for n in 1..=8u8 {
        let mode = if n == 2 {
            ControlMode::Relative(RelativeKind::BinaryOffset)
        } else {
            ControlMode::Absolute
        };
        controls.push(spec(
            Role::Knob(n),
            SourceKind::Cc(cc_base + 20 + n),
            mode,
            feedback,
        ));
    }
    for n in 1..=2u8 {
        controls.push(spec(
            Role::Fader(n),
            SourceKind::Cc(cc_base + n),
            ControlMode::Absolute,
            feedback,
        ));
    }
    for (role, note) in [
        (Role::Play, 90),
        (Role::Stop, 91),
        (Role::Record, 92),
        (Role::Loop, 93),
        (Role::BankLeft, 94),
        (Role::BankRight, 95),
    ] {
        controls.push(spec(
            role,
            SourceKind::Note(note),
            ControlMode::Button,
            feedback,
        ));
    }
    ControllerProfile {
        name: name.to_owned(),
        input: format!("{name} In"),
        output: Some(format!("{name} Out")),
        controls,
    }
}

fn core(synths: usize) -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    for i in 1..=synths {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(&format!("s{i}"))),
        })
        .unwrap();
    }
    c
}

fn attach(c: &mut AppCore<RecordingSink>, p: ControllerProfile) -> LoopbackSurface {
    let name = p.name.clone();
    c.add_loopback_driver(&name, Box::new(ProfileDriver::new(p)))
}

/// The key and the normalised value of remote knob `n` (0-based) of `device`.
fn remote(c: &AppCore<RecordingSink>, device: &str, n: usize) -> (&'static str, f32) {
    let d = c.doc().resolve(&key(device)).unwrap();
    let desc = c
        .registry()
        .get(&d.type_id)
        .unwrap()
        .descriptor
        .params
        .iter()
        .filter(|p| !atomik_model::console::is_hidden_param(p.key) && p.key != "bypass")
        .nth(n)
        .unwrap();
    let v = c
        .doc()
        .param_value(&key(device), desc.key, c.registry())
        .unwrap();
    (desc.key, desc.to_normalized(v))
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

struct Clock(f64);

impl Clock {
    fn frame(&mut self, c: &mut AppCore<RecordingSink>) {
        self.0 += 0.1;
        c.surface_frame(self.0);
    }
}

fn cc(n: u8, v: u8) -> [u8; 3] {
    [0xB0, n, v]
}

fn note(n: u8) -> [u8; 3] {
    [0x90, n, 100]
}

#[test]
fn the_knobs_drive_the_first_parameters_of_the_selected_device_and_follow_the_selection() {
    let mut c = core(2);
    let pad = attach(&mut c, profile("Pad", 0, false));
    let mut t = Clock(0.0);
    t.frame(&mut c);
    c.select(Some(key("s1")));
    // Knob 1 and knob 3: absolute positions of parameters 1 and 3 of the synth.
    pad.feed(&cc(21, 127));
    pad.feed(&cc(23, 0));
    t.frame(&mut c);
    assert!(
        near(remote(&c, "s1", 0).1, 1.0),
        "{:?}",
        remote(&c, "s1", 0)
    );
    assert!(near(remote(&c, "s1", 2).1, 0.0));
    // The selection moves: the same knob now drives the other device and leaves the first alone.
    c.select(Some(key("s2")));
    pad.feed(&cc(21, 127));
    t.frame(&mut c);
    assert!(near(remote(&c, "s2", 0).1, 1.0));
    pad.feed(&cc(21, 0));
    t.frame(&mut c);
    assert!(near(remote(&c, "s2", 0).1, 0.0));
    assert!(near(remote(&c, "s1", 0).1, 1.0));
}

#[test]
fn an_encoder_knob_turns_the_parameter_from_where_it_is() {
    let mut c = core(1);
    let pad = attach(&mut c, profile("Pad", 0, false));
    let mut t = Clock(0.0);
    t.frame(&mut c);
    c.select(Some(key("s1")));
    // Knob 2 is an encoder (binary offset): make parameter 2 mid-range first.
    let (param, _) = remote(&c, "s1", 1);
    let d = c.doc().resolve(&key("s1")).unwrap();
    let desc = c
        .registry()
        .get(&d.type_id)
        .unwrap()
        .descriptor
        .param_by_key(param)
        .unwrap();
    c.apply_command(Command::SetParam {
        key: key("s1"),
        param: param.to_owned(),
        value: desc.from_normalized(0.5),
        merge: None,
    })
    .unwrap();
    let before = remote(&c, "s1", 1).1;
    pad.feed(&cc(22, 70));
    t.frame(&mut c);
    let up = remote(&c, "s1", 1).1;
    assert!(up > before, "{before} -> {up}");
    pad.feed(&cc(22, 50));
    pad.feed(&cc(22, 50));
    t.frame(&mut c);
    assert!(remote(&c, "s1", 1).1 < up);
}

#[test]
fn a_surface_locked_to_a_device_keeps_its_knobs_on_it() {
    let mut c = core(2);
    let pad = attach(&mut c, profile("Pad", 0, false));
    let mut t = Clock(0.0);
    t.frame(&mut c);
    assert!(c.lock_surface("Pad", Some(key("s2"))));
    c.select(Some(key("s1")));
    pad.feed(&cc(21, 127));
    t.frame(&mut c);
    assert!(near(remote(&c, "s2", 0).1, 1.0));
    assert_eq!(c.followed_device("Pad"), Some(key("s2")));
    c.lock_surface("Pad", None);
    assert_eq!(c.followed_device("Pad"), Some(key("s1")));
}

#[test]
fn the_faders_are_the_strips_of_the_bank_and_the_bank_keys_page_through_them() {
    let mut c = core(12);
    let pad = attach(&mut c, profile("Pad", 0, false));
    let mut t = Clock(0.0);
    t.frame(&mut c);
    let fader = |c: &AppCore<RecordingSink>, strip: usize| {
        let k = if strip == 0 {
            key("channel")
        } else {
            key(&format!("channel-{}", strip + 1))
        };
        c.doc().param_value(&k, "fader", c.registry()).unwrap()
    };
    let before_9 = fader(&c, 8);
    pad.feed(&cc(1, 0));
    t.frame(&mut c);
    let first = fader(&c, 0);
    assert!(first < fader(&c, 1), "fader 1 moved strip 1 only");
    // Bank right: fader 1 is strip 9 now.
    pad.feed(&note(95));
    pad.feed(&cc(1, 0));
    t.frame(&mut c);
    assert!(fader(&c, 8) < before_9, "strip 9 moved");
    assert!(near(fader(&c, 0), first), "strip 1 stayed");
    // Bank left again: fader 2 is strip 2.
    pad.feed(&note(94));
    pad.feed(&cc(2, 0));
    t.frame(&mut c);
    assert!(fader(&c, 1) <= first);
}

#[test]
fn the_transport_keys_are_the_transport() {
    let mut c = core(1);
    let pad = attach(&mut c, profile("Pad", 0, false));
    let mut t = Clock(0.0);
    t.frame(&mut c);
    assert!(!c.playing);
    pad.feed(&note(90));
    t.frame(&mut c);
    assert!(c.playing, "Play starts the transport");
    pad.feed(&note(91));
    t.frame(&mut c);
    assert!(!c.playing, "Stop stops it");
    let looped = c.doc().song.loop_enabled;
    pad.feed(&note(93));
    t.frame(&mut c);
    assert_ne!(c.doc().song.loop_enabled, looped);
}

#[test]
fn feedback_shows_the_selected_devices_parameters_and_the_transport() {
    let mut c = core(2);
    let pad = attach(&mut c, profile("Pad", 0, true));
    let mut t = Clock(0.0);
    c.select(Some(key("s1")));
    t.frame(&mut c);
    let sent = pad.take_sent();
    // Everything is sent when the controller connects: eight knobs, two faders, three lamps
    // (the Record key is dark, Stop lit) and no bank-key lamp.
    assert!(sent.len() >= 8 + 2 + 3, "{}", sent.len());
    let knob1 = sent.iter().find(|m| m[..2] == [0xB0, 21]).unwrap();
    let want = (remote(&c, "s1", 0).1 * 127.0).round() as u8;
    assert_eq!(knob1[2], want);
    // Nothing changed: nothing is sent.
    t.frame(&mut c);
    assert!(pad.take_sent().is_empty());
    // Playing lights Play and darkens Stop.
    c.transport_action(TransportAction::Play);
    t.frame(&mut c);
    let sent = pad.take_sent();
    assert!(sent.contains(&vec![0x90, 90, 127]), "{sent:?}");
    assert!(sent.contains(&vec![0x80, 91, 0]), "{sent:?}");
    // A new selection shows the other device's knobs (only those that differ).
    let (param, now) = remote(&c, "s1", 0);
    let d = c.doc().resolve(&key("s2")).unwrap();
    let desc = c
        .registry()
        .get(&d.type_id)
        .unwrap()
        .descriptor
        .param_by_key(param)
        .unwrap();
    let to = if now < 0.5 { 1.0 } else { 0.0 };
    c.apply_command(Command::SetParam {
        key: key("s2"),
        param: param.to_owned(),
        value: desc.from_normalized(to),
        merge: None,
    })
    .unwrap();
    c.select(Some(key("s2")));
    t.frame(&mut c);
    let sent = pad.take_sent();
    assert!(
        sent.contains(&vec![0xB0, 21, (to * 127.0).round() as u8]),
        "{sent:?}"
    );
}

#[test]
fn two_controllers_work_at_once_each_on_its_own_device() {
    let mut c = core(2);
    let a = attach(&mut c, profile("Left", 0, false));
    let b = attach(&mut c, profile("Right", 0, false));
    assert_eq!(c.surfaces().len(), 2);
    let mut t = Clock(0.0);
    t.frame(&mut c);
    assert!(c.lock_surface("Left", Some(key("s1"))));
    assert!(c.lock_surface("Right", Some(key("s2"))));
    // The same message from each controller drives its own device.
    a.feed(&cc(21, 127));
    b.feed(&cc(21, 0));
    t.frame(&mut c);
    assert!(near(remote(&c, "s1", 0).1, 1.0));
    assert!(near(remote(&c, "s2", 0).1, 0.0));
    b.feed(&cc(21, 127));
    a.feed(&cc(21, 0));
    t.frame(&mut c);
    assert!(near(remote(&c, "s1", 0).1, 0.0));
    assert!(near(remote(&c, "s2", 0).1, 1.0));
    // Each keeps its own bank and its own feedback.
    a.feed(&note(95));
    t.frame(&mut c);
    assert_eq!(c.surfaces().len(), 2);
}
