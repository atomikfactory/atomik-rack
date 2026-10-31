//! MIDI Out devices in the app core: the sink is told which port each device sends to, once per
//! change, by engine device id, across edits, undo, removal and an engine rebuild.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::empty_doc;

const OUT: &str = "atomik.midi-out";
const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    c.view_mut().set_show_console_devices(false);
    c
}

fn add(c: &mut AppCore<RecordingSink>, type_id: &str, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.to_string(),
        key: Some(key(k)),
    })
    .unwrap();
}

fn id_of(c: &AppCore<RecordingSink>, k: &str) -> DeviceInstanceId {
    *c.ids.get(&key(k)).unwrap()
}

#[test]
fn the_sink_hears_of_each_midi_out_device_and_its_port_and_only_when_it_changes() {
    let mut c = core();
    add(&mut c, SYNTH, "synth");
    assert!(
        c.sink().midi_out_ports.is_empty(),
        "no MIDI Out device, nothing to say"
    );
    add(&mut c, OUT, "out");
    let out = id_of(&c, "out");
    assert_eq!(c.sink().midi_out_ports, vec![(out, None)]);
    let pushes = c.sink().midi_out_pushes;
    // An unrelated edit says nothing again.
    c.apply_command(Command::SetParam {
        key: key("synth"),
        param: "filter.cutoff".into(),
        value: 900.0,
        merge: None,
    })
    .ok();
    assert_eq!(c.sink().midi_out_pushes, pushes);
    c.set_midi_out_port(&key("out"), Some("Example Synth".into()));
    assert_eq!(
        c.sink().midi_out_ports,
        vec![(out, Some("Example Synth".into()))]
    );
    assert_eq!(
        c.midi_out_port_of(&key("out")),
        Some(Some("Example Synth".into()))
    );
    assert_eq!(
        c.midi_out_port_of(&key("synth")),
        None,
        "not a MIDI Out device"
    );
    // Undo and redo follow.
    c.undo();
    assert_eq!(c.sink().midi_out_ports, vec![(out, None)]);
    c.redo();
    assert_eq!(
        c.sink().midi_out_ports,
        vec![(out, Some("Example Synth".into()))]
    );
}

#[test]
fn a_second_device_is_listed_in_id_order_and_a_removed_one_leaves_the_list() {
    let mut c = core();
    add(&mut c, OUT, "a");
    add(&mut c, SYNTH, "s");
    add(&mut c, OUT, "b");
    c.set_midi_out_port(&key("b"), Some("B".into()));
    let (a, b) = (id_of(&c, "a"), id_of(&c, "b"));
    assert_eq!(
        c.sink().midi_out_ports,
        vec![(a, None), (b, Some("B".into()))]
    );
    c.apply_command(Command::RemoveDevice { key: key("a") })
        .unwrap();
    assert_eq!(c.sink().midi_out_ports, vec![(b, Some("B".into()))]);
}

#[test]
fn an_engine_rebuild_tells_the_new_session_again() {
    let mut c = core();
    add(&mut c, OUT, "out");
    c.set_midi_out_port(&key("out"), Some("Example Synth".into()));
    let before = c.sink().midi_out_pushes;
    c.rebuild_engine();
    assert_eq!(
        c.sink().midi_out_pushes,
        before + 1,
        "told again after a reset"
    );
    let out = id_of(&c, "out");
    assert_eq!(
        c.sink().midi_out_ports,
        vec![(out, Some("Example Synth".into()))]
    );
}

#[test]
fn choosing_a_port_for_another_device_is_refused_with_a_message() {
    let mut c = core();
    add(&mut c, SYNTH, "synth");
    c.set_midi_out_port(&key("synth"), Some("Port".into()));
    assert!(c.sink().midi_out_ports.is_empty());
    assert!(c.doc().resolve(&key("synth")).unwrap().midi_out.is_none());
}

#[test]
fn the_port_survives_a_save_and_open() {
    let mut c = core();
    add(&mut c, OUT, "out");
    c.set_midi_out_port(&key("out"), Some("Example Synth".into()));
    let text = atomik_model::save_project(c.doc());
    let mut d = core();
    d.open_str(&text, None).unwrap();
    assert_eq!(
        d.midi_out_port_of(&key("out")),
        Some(Some("Example Synth".into()))
    );
    let out = id_of(&d, "out");
    assert_eq!(
        d.sink().midi_out_ports,
        vec![(out, Some("Example Synth".into()))]
    );
}
