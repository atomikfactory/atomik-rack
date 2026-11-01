//! Several control surfaces at once: two generic controllers driving two devices, the mappings
//! tied to a surface, learning from a surface, the lock of a surface to a device, and the
//! commands a mapping can run (undo, redo, select a track, step a preset).

use super::*;
use crate::sink::RecordingSink;
use atomik_core::ChannelMessage;
use atomik_model::{
    MapMode, MapOptions, MidiMapCommand, MidiMapping, MidiSource, MidiTarget, RemoteCommand,
    SourceKind,
};

const SYNTH: &str = "atomik.poly-synth";
const CUTOFF: &str = "filter.cutoff";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// Two synths `a` and `b`, and two generic surfaces `Pads` and `Faders`.
fn core() -> (
    AppCore<RecordingSink>,
    crate::surface::LoopbackSurface,
    crate::surface::LoopbackSurface,
) {
    let mut c = AppCore::new(RecordingSink::new());
    for k in ["a", "b"] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key(k)),
        })
        .unwrap();
    }
    let pads = c.add_loopback_surface("Pads", "generic").unwrap();
    let faders = c.add_loopback_surface("Faders", "generic").unwrap();
    c.sink_mut().take();
    (c, pads, faders)
}

fn from(surface: Option<&str>, n: u8) -> MidiSource {
    MidiSource {
        surface: surface.map(Into::into),
        ..MidiSource::any_port(Some(0), SourceKind::Cc(n))
    }
}

/// Adds a mapping with no soft takeover, so the first message sets the target.
fn add(c: &mut AppCore<RecordingSink>, source: MidiSource, target: MidiTarget) {
    let options = MapOptions {
        pickup: false,
        ..MapOptions::for_source(source.kind)
    };
    c.apply_command(Command::MidiMap(MidiMapCommand::Add(MidiMapping {
        id: 0,
        source,
        target,
        options,
    })))
    .unwrap();
}

fn cutoff_of(device: &str) -> MidiTarget {
    MidiTarget::Param {
        device: key(device),
        param: CUTOFF.into(),
    }
}

fn focused() -> MidiTarget {
    MidiTarget::Focused {
        param: CUTOFF.into(),
    }
}

/// Sends a controller message from a surface and runs a frame (the frame number sets the time).
fn send(c: &mut AppCore<RecordingSink>, s: &crate::surface::LoopbackSurface, n: u8, v: u8, t: f64) {
    s.feed(&[0xB0, n, v]);
    c.surface_frame(t);
}

fn value(c: &AppCore<RecordingSink>, device: &str) -> f32 {
    c.doc()
        .param_value(&key(device), CUTOFF, c.registry())
        .unwrap()
}

fn norm(c: &AppCore<RecordingSink>, device: &str) -> f32 {
    let d = c.doc().resolve(&key(device)).unwrap();
    c.registry()
        .get(&d.type_id)
        .unwrap()
        .descriptor
        .param_by_key(CUTOFF)
        .unwrap()
        .to_normalized(value(c, device))
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn two_surfaces_drive_two_devices_at_once() {
    let (mut c, pads, faders) = core();
    assert_eq!(c.surfaces().len(), 2);
    assert_eq!(c.surfaces()[1].id(), "Faders");
    // The same controller number on each surface, a different device each.
    add(&mut c, from(Some("Pads"), 20), cutoff_of("a"));
    add(&mut c, from(Some("Faders"), 20), cutoff_of("b"));
    pads.feed(&[0xB0, 20, 127]);
    faders.feed(&[0xB0, 20, 0]);
    c.surface_frame(0.0);
    assert!(near(norm(&c, "a"), 1.0), "{}", norm(&c, "a"));
    assert!(near(norm(&c, "b"), 0.0), "{}", norm(&c, "b"));
    // And the other way round on the next frame.
    pads.feed(&[0xB0, 20, 0]);
    faders.feed(&[0xB0, 20, 127]);
    c.surface_frame(0.1);
    assert!(near(norm(&c, "a"), 0.0));
    assert!(near(norm(&c, "b"), 1.0));
    assert_eq!(c.midi_received(), 4);
    assert!(c.midi_last_message().unwrap().starts_with("Faders"));
}

#[test]
fn a_mapping_tied_to_a_surface_ignores_other_surfaces_and_plain_ports() {
    let (mut c, pads, faders) = core();
    add(&mut c, from(Some("Pads"), 20), cutoff_of("a"));
    let before = norm(&c, "a");
    faders.feed(&[0xB0, 20, 127]);
    c.surface_frame(0.0);
    assert!(near(norm(&c, "a"), before), "another surface");
    c.inject_midi("Pads", ChannelMessage::cc(0, 20, 127));
    c.midi_control_frame(0.1);
    assert!(
        near(norm(&c, "a"), before),
        "a port that happens to be named alike"
    );
    pads.feed(&[0xB0, 20, 127]);
    c.surface_frame(0.2);
    assert!(near(norm(&c, "a"), 1.0));
    // A mapping with no surface hears every surface and every port.
    add(&mut c, from(None, 21), cutoff_of("b"));
    send(&mut c, &faders, 21, 127, 0.3);
    assert!(near(norm(&c, "b"), 1.0));
    c.inject_midi("Keys", ChannelMessage::cc(0, 21, 0));
    c.midi_control_frame(0.4);
    assert!(near(norm(&c, "b"), 0.0));
}

#[test]
fn learning_from_a_surface_ties_the_mapping_to_it() {
    let (mut c, pads, _faders) = core();
    assert!(c.midi_arm(cutoff_of("a")));
    send(&mut c, &pads, 74, 64, 0.0);
    let m = &c.doc().midi_maps[0];
    assert_eq!(m.source.surface.as_deref(), Some("Pads"));
    assert_eq!(m.target, cutoff_of("a"));
    // A control of a plain port is learnt without a surface.
    assert!(c.midi_arm(cutoff_of("b")));
    c.inject_midi("Keys", ChannelMessage::cc(0, 75, 64));
    c.midi_control_frame(0.1);
    assert_eq!(c.doc().midi_maps[1].source.surface, None);
}

#[test]
fn the_focused_device_follows_the_selection_until_a_surface_is_locked() {
    let (mut c, pads, faders) = core();
    add(&mut c, from(Some("Pads"), 20), focused());
    add(&mut c, from(Some("Faders"), 20), focused());
    let start = norm(&c, "b");
    c.select(Some(key("a")));
    send(&mut c, &pads, 20, 127, 0.0);
    assert!(near(norm(&c, "a"), 1.0), "the selected device");
    assert!(near(norm(&c, "b"), start));
    // Lock Pads to b: the selection no longer matters to it, and Faders still follow it.
    assert!(c.lock_surface("Pads", Some(key("b"))));
    assert_eq!(c.surface_lock("Pads"), Some(&key("b")));
    c.select(Some(key("a")));
    send(&mut c, &pads, 20, 0, 0.1);
    assert!(near(norm(&c, "b"), 0.0), "the locked device");
    assert!(near(norm(&c, "a"), 1.0), "a was left alone");
    send(&mut c, &faders, 20, 64, 0.2);
    assert!(
        near(norm(&c, "a"), 64.0 / 127.0),
        "Faders follow the selection"
    );
    // The lock holds when the focus moves to another device and back.
    c.select(Some(key("b")));
    c.select(None);
    send(&mut c, &pads, 20, 127, 0.3);
    assert!(near(norm(&c, "b"), 1.0));
    // Unlocking returns it to the selection.
    assert!(c.lock_surface("Pads", None));
    assert_eq!(c.surface_lock("Pads"), None);
    c.select(Some(key("a")));
    send(&mut c, &pads, 20, 0, 0.4);
    assert!(near(norm(&c, "a"), 0.0));
    assert!(near(norm(&c, "b"), 1.0));
}

#[test]
fn locking_needs_a_surface_and_a_device_and_the_toggle_uses_the_selection() {
    let (mut c, _pads, _faders) = core();
    assert!(!c.lock_surface("Nobody", Some(key("a"))));
    assert!(c.last_rejection().unwrap().contains("Nobody"));
    assert!(!c.lock_surface("Pads", Some(key("ghost"))));
    c.select(None);
    assert!(!c.toggle_surface_lock("Pads"));
    c.select(Some(key("a")));
    assert!(c.toggle_surface_lock("Pads"));
    assert_eq!(c.surface_lock("Pads"), Some(&key("a")));
    assert!(c.toggle_surface_lock("Pads"));
    assert_eq!(c.surface_lock("Pads"), None);
    // A lock is forgotten with its surface and with the project.
    assert!(c.lock_surface("Faders", Some(key("b"))));
    c.set_surfaces(Vec::new());
    assert_eq!(c.surface_lock("Faders"), None);
}

#[test]
fn a_locked_surface_does_nothing_for_a_device_without_the_param() {
    let (mut c, pads, _faders) = core();
    add(&mut c, from(Some("Pads"), 20), focused());
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.channel".into(),
        key: Some(key("strip")),
    })
    .unwrap();
    assert!(c.lock_surface("Pads", Some(key("strip"))));
    c.select(Some(key("a")));
    let before = norm(&c, "a");
    send(&mut c, &pads, 20, 127, 0.0);
    assert!(
        near(norm(&c, "a"), before),
        "never falls back to the selection"
    );
}

#[test]
fn undo_and_redo_run_from_buttons_and_a_mapped_sweep_is_one_step() {
    let (mut c, pads, _faders) = core();
    let button = |n: u8| MidiSource::any_port(Some(1), SourceKind::Note(n));
    add(&mut c, from(Some("Pads"), 20), cutoff_of("a"));
    add(&mut c, button(36), MidiTarget::Command(RemoteCommand::Undo));
    add(&mut c, button(37), MidiTarget::Command(RemoteCommand::Redo));
    let start = norm(&c, "a");
    // A sweep of the mapped fader: several messages, one undo step.
    for (i, v) in [10u8, 40, 90, 127].into_iter().enumerate() {
        send(&mut c, &pads, 20, v, i as f64 * 0.05);
    }
    assert!(near(norm(&c, "a"), 1.0));
    // The button press ends the sweep and undoes all of it.
    pads.feed(&[0x91, 36, 100]);
    c.surface_frame(0.3);
    assert!(near(norm(&c, "a"), start), "{}", norm(&c, "a"));
    pads.feed(&[0x81, 36, 0]);
    pads.feed(&[0x91, 37, 100]);
    c.surface_frame(0.4);
    assert!(near(norm(&c, "a"), 1.0), "redone");
    // Nothing more to redo: said, nothing breaks.
    pads.feed(&[0x81, 37, 0]);
    pads.feed(&[0x91, 37, 100]);
    c.surface_frame(0.5);
    assert_eq!(c.last_rejection(), Some("Nothing to redo"));
}

#[test]
fn a_command_mapping_fires_once_per_press_not_on_the_release() {
    let (mut c, pads, _faders) = core();
    add(
        &mut c,
        MidiSource::any_port(Some(1), SourceKind::Note(36)),
        MidiTarget::Command(RemoteCommand::Undo),
    );
    c.apply_command(Command::Rename {
        key: key("a"),
        name: "One".into(),
    })
    .unwrap();
    c.apply_command(Command::Rename {
        key: key("a"),
        name: "Two".into(),
    })
    .unwrap();
    pads.feed(&[0x91, 36, 100]);
    pads.feed(&[0x81, 36, 0]);
    c.surface_frame(0.0);
    assert_eq!(
        c.doc().device(&key("a")).unwrap().name,
        "One",
        "one step back"
    );
}

#[test]
fn select_track_commands_move_the_track_and_the_focused_device() {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::new_project_doc(), None);
    for k in ["a", "b"] {
        c.apply_command(Command::Workflow(
            atomik_model::WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: atomik_model::CreateSource::Type(SYNTH.into()),
                key: Some(key(k)),
                options: atomik_model::CreateOptions::default(),
            },
        ))
        .unwrap();
    }
    let pads = c.add_loopback_surface("Pads", "generic").unwrap();
    let button = |n: u8| MidiSource::any_port(Some(1), SourceKind::Note(n));
    add(
        &mut c,
        button(36),
        MidiTarget::Command(RemoteCommand::SelectTrack { delta: 1 }),
    );
    add(
        &mut c,
        button(37),
        MidiTarget::Command(RemoteCommand::SelectTrack { delta: -1 }),
    );
    let press = |c: &mut AppCore<RecordingSink>, n: u8, t: f64| {
        pads.feed(&[0x91, n, 100]);
        pads.feed(&[0x81, n, 0]);
        c.surface_frame(t);
    };
    press(&mut c, 36, 0.0);
    let first = c.arrangement().selected_track().cloned().unwrap();
    press(&mut c, 36, 0.1);
    let second = c.arrangement().selected_track().cloned().unwrap();
    assert_ne!(first, second);
    assert_eq!(
        c.view().primary(),
        Some(second.clone()),
        "its device is focused"
    );
    press(&mut c, 37, 0.2);
    assert_eq!(c.arrangement().selected_track(), Some(&first));
    // At the end it stays at the end.
    for i in 0..4 {
        press(&mut c, 36, 0.3 + f64::from(i) * 0.1);
    }
    assert_eq!(c.arrangement().selected_track(), Some(&second));
}

#[test]
fn select_preset_steps_the_focused_device_or_says_why_not() {
    let (mut c, pads, _faders) = core();
    add(
        &mut c,
        MidiSource::any_port(Some(1), SourceKind::Note(36)),
        MidiTarget::Command(RemoteCommand::SelectPreset { delta: 1 }),
    );
    c.select(None);
    pads.feed(&[0x91, 36, 100]);
    c.surface_frame(0.0);
    assert!(c.last_rejection().unwrap().contains("Select a device"));
    c.select(Some(key("a")));
    pads.feed(&[0x81, 36, 0]);
    pads.feed(&[0x91, 36, 100]);
    c.surface_frame(0.1);
    assert!(
        c.last_rejection().unwrap().contains("library folder"),
        "{:?}",
        c.last_rejection()
    );
}

#[test]
fn a_command_mapping_is_a_button_not_a_fader() {
    let m = MapOptions::for_source(SourceKind::Note(1));
    assert_eq!(m.mode, MapMode::Toggle);
    assert!(MidiTarget::Command(RemoteCommand::Redo).is_trigger());
}

#[test]
fn replacing_a_surface_of_the_same_name_keeps_the_others() {
    let (mut c, _pads, _faders) = core();
    let again = c.add_loopback_surface("Pads", "mcu").unwrap();
    assert_eq!(c.surfaces().len(), 2);
    assert_eq!(c.surfaces()[0].driver_name(), "mcu");
    assert_eq!(c.surfaces()[1].driver_name(), "generic");
    c.surface_frame(0.0);
    assert!(!again.take_sent().is_empty(), "the new one says hello");
    assert!(c.add_loopback_surface("X", "nonsense").is_err());
    let _ = c.attach_loopback_surface("generic").unwrap();
    assert_eq!(c.surfaces().len(), 1, "attach replaces every surface");
}
