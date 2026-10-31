//! Copying a strip's settings, resetting a strip, clearing its inserts, and the one undo step of
//! a ganged drag.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    ChannelSettingsClip, Command, ConsoleCommand, DeviceKey, Document, InsertEffect, SnapshotScope,
    insert_chain, output_dest,
};
use common::*;

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn set(device: &DeviceKey, param: &str, value: f32, merge: Option<u64>) -> Command {
    Command::SetParam {
        key: device.clone(),
        param: param.into(),
        value,
        merge,
    }
}

fn param(d: &Document, device: &DeviceKey, name: &str) -> Option<f32> {
    d.doc().device(device).unwrap().params.get(name).copied()
}

/// two synths, each with a channel, and a compressor and an eq to insert.
fn rack(reg: &Registry) -> (Document, DeviceKey, DeviceKey) {
    let mut d = three_devices(reg);
    for (i, (t, k)) in [("atomik.compressor", "comp"), ("atomik.eq", "eq")]
        .into_iter()
        .enumerate()
    {
        d.apply(add(3 + i, t, Some(k)), reg).unwrap();
    }
    d.apply(add(5, SYNTH, Some("synth2")), reg).unwrap();
    let a = d.doc().console.channels[0].channel.clone();
    let b = d.doc().console.channels[1].channel.clone();
    (d, a, b)
}

/// A strip with a few params moved and two inserts, one of them with a param moved.
fn dressed(reg: &Registry) -> (Document, DeviceKey, DeviceKey) {
    let (mut d, a, b) = rack(reg);
    for (p, v) in [("fader", 0.5), ("pan", 0.25), ("mute", 1.0)] {
        d.apply(set(&a, p, v, None), reg).unwrap();
    }
    for (slot, id) in ["atomik.compressor", "atomik.eq"].into_iter().enumerate() {
        d.apply(
            cc(ConsoleCommand::SetInsert {
                channel: a.clone(),
                slot,
                effect: Some(InsertEffect::New { type_id: id.into() }),
            }),
            reg,
        )
        .unwrap();
    }
    let first = insert_chain(d.doc(), reg, &a)[0].clone();
    d.apply(set(&first, "threshold", -30.0, None), reg).unwrap();
    (d, a, b)
}

#[test]
fn a_paste_gives_equal_params_and_an_insert_chain_with_equal_effect_params() {
    let reg = console_registry();
    let (mut d, a, b) = dressed(&reg);
    let clip = d
        .doc()
        .copy_channel_settings(&reg, &a, SnapshotScope::ALL)
        .unwrap();
    assert_eq!(clip.inserts.len(), 2);
    let before = d.doc().clone();
    let out_before = output_dest(d.doc(), &b);
    d.apply(
        cc(ConsoleCommand::PasteSettings {
            strips: vec![b.clone()],
            clip,
        }),
        &reg,
    )
    .unwrap();
    for p in ["fader", "pan", "mute"] {
        assert_eq!(param(&d, &a, p), param(&d, &b, p), "{p}");
    }
    let (ca, cb) = (
        insert_chain(d.doc(), &reg, &a),
        insert_chain(d.doc(), &reg, &b),
    );
    assert_eq!(cb.len(), 2);
    for (x, y) in ca.iter().zip(&cb) {
        assert_eq!(
            d.doc().device(x).unwrap().type_id,
            d.doc().device(y).unwrap().type_id
        );
        assert_ne!(x, y, "the pasted effects are new devices");
    }
    assert_eq!(param(&d, &cb[0], "threshold"), Some(-30.0));
    assert_eq!(output_dest(d.doc(), &b), out_before, "the routing stays");
    // One undo step puts everything back, one redo does it again.
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&reg).unwrap();
    assert_eq!(insert_chain(d.doc(), &reg, &b).len(), 2);
}

#[test]
fn a_paste_of_one_section_leaves_the_others_alone() {
    let reg = console_registry();
    let (mut d, a, b) = dressed(&reg);
    d.apply(set(&b, "pan", -0.5, None), &reg).unwrap();
    let clip = d
        .doc()
        .copy_channel_settings(&reg, &a, SnapshotScope::ALL)
        .unwrap()
        .restricted(SnapshotScope::MUTES);
    assert!(clip.inserts.is_empty());
    d.apply(
        cc(ConsoleCommand::PasteSettings {
            strips: vec![b.clone()],
            clip,
        }),
        &reg,
    )
    .unwrap();
    assert_eq!(param(&d, &b, "mute"), Some(1.0));
    assert_eq!(param(&d, &b, "pan"), Some(-0.5));
    assert_eq!(param(&d, &b, "fader"), None);
    assert!(insert_chain(d.doc(), &reg, &b).is_empty());
}

#[test]
fn a_paste_without_inserts_in_the_clip_clears_the_chain_only_when_inserts_are_a_section() {
    let reg = console_registry();
    let (mut d, a, b) = dressed(&reg);
    let bare = ChannelSettingsClip {
        sections: SnapshotScope::ALL,
        params: Default::default(),
        inserts: Vec::new(),
    };
    d.apply(
        cc(ConsoleCommand::PasteSettings {
            strips: vec![a.clone()],
            clip: bare.clone(),
        }),
        &reg,
    )
    .unwrap();
    assert!(insert_chain(d.doc(), &reg, &a).is_empty());
    assert_eq!(param(&d, &a, "fader"), None, "back to the default");
    let levels = ChannelSettingsClip {
        sections: SnapshotScope::LEVELS,
        ..bare
    };
    d.apply(
        cc(ConsoleCommand::PasteSettings {
            strips: vec![b],
            clip: levels,
        }),
        &reg,
    )
    .unwrap();
}

#[test]
fn a_reset_puts_the_params_back_and_clears_the_inserts_but_not_the_routing() {
    let reg = console_registry();
    let (mut d, a, _) = dressed(&reg);
    let before = d.doc().clone();
    let out = output_dest(d.doc(), &a);
    d.apply(
        cc(ConsoleCommand::ResetStrip {
            strip: a.clone(),
            keep_routing: true,
        }),
        &reg,
    )
    .unwrap();
    for p in ["fader", "pan", "mute"] {
        assert_eq!(param(&d, &a, p), None, "{p}");
    }
    assert!(insert_chain(d.doc(), &reg, &a).is_empty());
    assert_eq!(output_dest(d.doc(), &a), out);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn clearing_the_inserts_leaves_the_params() {
    let reg = console_registry();
    let (mut d, a, _) = dressed(&reg);
    d.apply(cc(ConsoleCommand::ClearInserts { strip: a.clone() }), &reg)
        .unwrap();
    assert!(insert_chain(d.doc(), &reg, &a).is_empty());
    assert_eq!(param(&d, &a, "fader"), Some(0.5));
    d.undo(&reg).unwrap();
    assert_eq!(insert_chain(d.doc(), &reg, &a).len(), 2);
}

#[test]
fn the_settings_commands_refuse_what_is_no_strip() {
    let reg = console_registry();
    let (mut d, a, _) = dressed(&reg);
    let clip = d
        .doc()
        .copy_channel_settings(&reg, &a, SnapshotScope::ALL)
        .unwrap();
    assert!(
        d.doc()
            .copy_channel_settings(&reg, &key("synth"), SnapshotScope::ALL)
            .is_err()
    );
    let before = d.doc().clone();
    for command in [
        ConsoleCommand::PasteSettings {
            strips: Vec::new(),
            clip: clip.clone(),
        },
        ConsoleCommand::PasteSettings {
            strips: vec![key("synth")],
            clip,
        },
        ConsoleCommand::ResetStrip {
            strip: key("delay"),
            keep_routing: true,
        },
        ConsoleCommand::ClearInserts { strip: key("eq") },
    ] {
        assert!(d.apply(cc(command), &reg).is_err());
    }
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_ganged_drag_is_one_undo_step() {
    let reg = console_registry();
    let (mut d, a, b) = rack(&reg);
    let before = d.doc().clone();
    let gang = |db: f32, merge: u64| {
        Command::Batch(vec![
            set(&a, "fader", db, Some(merge)),
            set(&b, "fader", db + 0.5, Some(merge)),
        ])
    };
    for db in [0.1, 0.2, 0.3] {
        d.apply(gang(db, 7), &reg).unwrap();
    }
    assert_eq!(param(&d, &a, "fader"), Some(0.3));
    assert_eq!(param(&d, &b, "fader"), Some(0.8));
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before, "the whole drag is one step");
    // Another gesture is a step of its own.
    d.apply(gang(0.1, 8), &reg).unwrap();
    d.apply(gang(0.2, 9), &reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(param(&d, &a, "fader"), Some(0.1));
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
}
