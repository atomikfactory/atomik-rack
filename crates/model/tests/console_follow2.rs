//! The master's settings copy and paste, and cabling a strip's `key_in` switching its key on.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    ChannelSettingsClip, Command, ConsoleCommand, DeviceKey, Document, SnapshotScope,
};
use common::*;

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn set(device: &DeviceKey, param: &str, value: f32) -> Command {
    Command::SetParam {
        key: device.clone(),
        param: param.into(),
        value,
        merge: None,
    }
}

fn param(d: &Document, device: &DeviceKey, name: &str) -> Option<f32> {
    d.doc().device(device).unwrap().params.get(name).copied()
}

/// Two synths with a channel each (so the console has a master), plus a third synth that feeds
/// nothing, as a key source.
fn rack(reg: &Registry) -> (Document, DeviceKey, DeviceKey, DeviceKey) {
    let mut d = three_devices(reg);
    d.apply(add(3, SYNTH, Some("synth2")), reg).unwrap();
    let master = d.doc().console.master.clone().unwrap();
    let a = d.doc().console.channels[0].channel.clone();
    let b = d.doc().console.channels[1].channel.clone();
    (d, master, a, b)
}

#[test]
fn the_master_can_be_copied_and_pasted_back_in_one_undo_step() {
    let reg = console_registry();
    let (mut d, master, _, _) = rack(&reg);
    d.apply(set(&master, "fader", 0.5), &reg).unwrap();
    d.apply(set(&master, "comp.threshold", -20.0), &reg)
        .unwrap();
    let clip = d
        .doc()
        .copy_channel_settings(&reg, &master, SnapshotScope::ALL)
        .unwrap();
    assert!(clip.master);
    assert_eq!(clip.params.get("fader"), Some(&0.5));
    d.apply(set(&master, "fader", 0.9), &reg).unwrap();
    d.apply(set(&master, "comp.threshold", -5.0), &reg).unwrap();
    let before = d.doc().clone();
    let depth = d.undo_depth();
    d.apply(
        cc(ConsoleCommand::PasteSettings {
            strips: vec![master.clone()],
            clip,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(d.undo_depth(), depth + 1, "one undo step");
    assert_eq!(param(&d, &master, "fader"), Some(0.5));
    assert_eq!(param(&d, &master, "comp.threshold"), Some(-20.0));
    d.undo().unwrap();
    assert_eq!(*d.doc(), before);
}

#[test]
fn a_section_of_the_master_copies_alone() {
    let reg = console_registry();
    let (mut d, master, _, _) = rack(&reg);
    d.apply(set(&master, "fader", 0.5), &reg).unwrap();
    d.apply(set(&master, "comp.threshold", -20.0), &reg)
        .unwrap();
    let clip = d
        .doc()
        .copy_channel_settings(&reg, &master, SnapshotScope::LEVELS)
        .unwrap();
    assert!(clip.params.contains_key("fader"));
    assert!(!clip.params.contains_key("comp.threshold"));
    let all = d
        .doc()
        .copy_channel_settings(&reg, &master, SnapshotScope::ALL)
        .unwrap();
    assert_eq!(all.restricted(SnapshotScope::LEVELS), clip);
}

#[test]
fn master_settings_paste_onto_the_master_only_and_strip_settings_never_onto_it() {
    let reg = console_registry();
    let (mut d, master, a, b) = rack(&reg);
    d.apply(set(&a, "pan", 0.25), &reg).unwrap();
    let from_master = d
        .doc()
        .copy_channel_settings(&reg, &master, SnapshotScope::ALL)
        .unwrap();
    let from_channel = d
        .doc()
        .copy_channel_settings(&reg, &a, SnapshotScope::ALL)
        .unwrap();
    assert!(!from_channel.master);
    let before = d.doc().clone();
    let depth = d.undo_depth();
    for (strips, clip) in [
        (vec![b.clone()], from_master.clone()),
        (vec![master.clone()], from_channel),
        (vec![b, master.clone()], from_master),
    ] {
        let refused = d.apply(cc(ConsoleCommand::PasteSettings { strips, clip }), &reg);
        assert!(refused.is_err());
    }
    assert_eq!(*d.doc(), before, "a refusal changes nothing");
    assert_eq!(d.undo_depth(), depth);
}

#[test]
fn resetting_the_master_puts_its_params_back_and_keeps_the_routing() {
    let reg = console_registry();
    let (mut d, master, a, _) = rack(&reg);
    d.apply(set(&master, "fader", 0.5), &reg).unwrap();
    let cables = d.doc().cables.clone();
    d.apply(
        cc(ConsoleCommand::ResetStrip {
            strip: master.clone(),
            keep_routing: false,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(param(&d, &master, "fader"), None);
    assert_eq!(d.doc().cables, cables, "the master has no output to move");
    assert_eq!(
        param(&d, &a, "fader"),
        None,
        "the strips are not touched by it"
    );
}

#[test]
fn cabling_key_in_switches_the_key_on_in_the_same_undo_step() {
    let reg = console_registry();
    let (mut d, _, a, _) = rack(&reg);
    assert_eq!(param(&d, &a, "key.on"), None);
    let before = d.doc().clone();
    let depth = d.undo_depth();
    let to = format!("{}:key_in", a.0);
    d.apply(connect("synth2:out_l", &to), &reg).unwrap();
    assert_eq!(d.undo_depth(), depth + 1);
    assert_eq!(param(&d, &a, "key.on"), Some(1.0));
    d.undo().unwrap();
    assert_eq!(*d.doc(), before, "undo takes the cable and the switch back");
    d.redo().unwrap();
    assert_eq!(param(&d, &a, "key.on"), Some(1.0));
}

#[test]
fn a_key_that_is_already_on_and_other_sockets_are_left_alone() {
    let reg = console_registry();
    let (mut d, _, a, b) = rack(&reg);
    d.apply(set(&a, "key.on", 1.0), &reg).unwrap();
    let depth = d.undo_depth();
    d.apply(connect("synth2:out_l", &format!("{}:key_in", a.0)), &reg)
        .unwrap();
    assert_eq!(d.undo_depth(), depth + 1);
    assert_eq!(param(&d, &a, "key.on"), Some(1.0));
    // A cable into a different socket does not touch the switch.
    d.apply(
        connect("synth2:out_r", &format!("{}:insert_return_l", b.0)),
        &reg,
    )
    .unwrap();
    assert_eq!(param(&d, &b, "key.on"), None);
    // Removing the cable leaves the switch as the user has it.
    d.apply(
        Command::Disconnect {
            to: sock(&format!("{}:key_in", a.0)),
        },
        &reg,
    )
    .unwrap();
    assert_eq!(param(&d, &a, "key.on"), Some(1.0));
}

#[test]
fn cabling_the_key_of_a_bus_switches_it_on_and_the_master_keeps_its_own_switch() {
    let reg = console_registry();
    let (mut d, master, a, _) = rack(&reg);
    d.apply(
        cc(ConsoleCommand::RouteToNewBus {
            strips: vec![a],
            name: "Group".into(),
        }),
        &reg,
    )
    .unwrap();
    let bus = d.doc().console.buses[0].bus.clone();
    d.apply(connect("synth2:out_l", &format!("{}:key_in", bus.0)), &reg)
        .unwrap();
    assert_eq!(param(&d, &bus, "key.on"), Some(1.0));
    d.apply(
        connect("synth2:out_r", &format!("{}:key_in", master.0)),
        &reg,
    )
    .unwrap();
    // The master already switched its compressor key on; it has no `key.on` of its own.
    assert_eq!(param(&d, &master, "key.on"), None);
    assert_eq!(param(&d, &master, "comp.key"), Some(1.0));
}

#[test]
fn a_clip_built_by_hand_keeps_its_master_flag_when_restricted() {
    let clip = ChannelSettingsClip {
        master: true,
        sections: SnapshotScope::ALL,
        params: [("fader".to_owned(), 0.5)].into(),
        inserts: Vec::new(),
    };
    assert!(clip.restricted(SnapshotScope::LEVELS).master);
}
