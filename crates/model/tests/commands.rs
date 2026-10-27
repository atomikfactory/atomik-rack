//! `Document::apply`: one test per command, including every rejection.

mod common;

use atomik_model::{Command, Document, ModelError, SyncOp, empty_doc};
use common::*;

/// Applies a command that must be rejected and checks the document did not change.
fn rejects(d: &mut Document, cmd: Command, reg: &atomik_device_sdk::Registry) -> ModelError {
    let before = d.doc().clone();
    let undo = d.can_undo();
    let e = d.apply(cmd, reg).expect_err("command must be rejected");
    assert_eq!(
        d.doc(),
        &before,
        "a rejected command leaves the document unchanged"
    );
    assert_eq!(
        d.can_undo(),
        undo,
        "a rejected command records no undo step"
    );
    e
}

// ---- AddDevice ------------------------------------------------------------------------------

#[test]
fn add_device_inserts_with_defaults_and_reports_sync() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    let ops = d.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    assert_eq!(ops, [SyncOp::DeviceAdded(key("synth")), routing()]);
    let dev = &d.doc().devices[0];
    assert_eq!(dev.key, key("synth"));
    assert_eq!(dev.type_id, SYNTH);
    assert_eq!(dev.name, "Poly Synth");
    assert!(dev.params.is_empty());
    assert!(!dev.folded);
    assert_eq!(dev.color, None);
}

#[test]
fn add_device_index_is_clamped_and_inserts_in_order() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(99, SYNTH, Some("a")), &reg).unwrap();
    d.apply(add(99, SYNTH, Some("c")), &reg).unwrap();
    d.apply(add(1, SYNTH, Some("b")), &reg).unwrap();
    d.apply(add(0, SYNTH, Some("z")), &reg).unwrap();
    assert_eq!(keys(&d), ["z", "a", "b", "c"]);
}

#[test]
fn add_device_generates_keys_from_type() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    for _ in 0..3 {
        d.apply(add(99, SYNTH, None), &reg).unwrap();
    }
    d.apply(add(99, DELAY, None), &reg).unwrap();
    assert_eq!(
        keys(&d),
        ["poly-synth", "poly-synth-2", "poly-synth-3", "stereo-delay"]
    );
    // The first free suffix is reused.
    d.apply(
        Command::RemoveDevice {
            key: key("poly-synth-2"),
        },
        &reg,
    )
    .unwrap();
    let ops = d.apply(add(0, SYNTH, None), &reg).unwrap();
    assert_eq!(ops[0], SyncOp::DeviceAdded(key("poly-synth-2")));
}

#[test]
fn add_device_generates_unique_names() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    for _ in 0..3 {
        d.apply(add(99, SYNTH, None), &reg).unwrap();
    }
    let names: Vec<_> = d.doc().devices.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["Poly Synth", "Poly Synth 2", "Poly Synth 3"]);
}

#[test]
fn add_device_rejects_unknown_type() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    let e = rejects(&mut d, add(0, "atomik.flanger", None), &reg);
    assert_eq!(e, ModelError::UnknownType("atomik.flanger".into()));
}

#[test]
fn add_device_rejects_bad_and_duplicate_keys() {
    let reg = registry();
    let mut d = three_devices(&reg);
    for bad in ["Synth", "1x", "", "a b", "x!", "interface", "synth"] {
        let e = rejects(&mut d, add(0, SYNTH, Some(bad)), &reg);
        assert_eq!(e, ModelError::BadKey(bad.into()), "{bad}");
    }
    d.apply(add(0, SYNTH, Some("my_synth-2")), &reg).unwrap();
}

// ---- RemoveDevice ---------------------------------------------------------------------------

#[test]
fn remove_device_drops_its_cables_and_reports_routing_first() {
    let reg = registry();
    let mut d = chain(&reg);
    let ops = d
        .apply(Command::RemoveDevice { key: key("delay") }, &reg)
        .unwrap();
    assert_eq!(ops, [routing(), SyncOp::DeviceRemoved(key("delay"))]);
    assert_eq!(keys(&d), ["synth", "mixer"]);
    assert_eq!(
        d.doc().cables,
        [
            cable("mixer:master_l", "interface:out_1"),
            cable("mixer:master_r", "interface:out_2"),
        ]
    );
}

#[test]
fn remove_device_rejects_unknown_and_interface() {
    let reg = registry();
    let mut d = three_devices(&reg);
    for k in ["nope", "interface"] {
        let e = rejects(&mut d, Command::RemoveDevice { key: key(k) }, &reg);
        assert_eq!(e, ModelError::UnknownDevice(k.into()));
    }
}

// ---- MoveDevice -----------------------------------------------------------------------------

#[test]
fn move_device_reorders_with_clamp() {
    let reg = registry();
    let mut d = three_devices(&reg);
    let mv = |k: &str, to| Command::MoveDevice { key: key(k), to };
    assert_eq!(
        d.apply(mv("synth", 2), &reg).unwrap(),
        [SyncOp::ViewChanged]
    );
    assert_eq!(keys(&d), ["delay", "mixer", "synth"]);
    d.apply(mv("delay", 99), &reg).unwrap();
    assert_eq!(keys(&d), ["mixer", "synth", "delay"]);
    d.apply(mv("delay", 0), &reg).unwrap();
    assert_eq!(keys(&d), ["delay", "mixer", "synth"]);
    let e = rejects(&mut d, mv("nope", 0), &reg);
    assert_eq!(e, ModelError::UnknownDevice("nope".into()));
}

// ---- View commands --------------------------------------------------------------------------

#[test]
fn fold_rename_color_change_view_only() {
    let reg = registry();
    let mut d = three_devices(&reg);
    let ops = d
        .apply(
            Command::SetFolded {
                key: key("delay"),
                folded: true,
            },
            &reg,
        )
        .unwrap();
    assert_eq!(ops, [SyncOp::ViewChanged]);
    assert!(d.doc().devices[1].folded);
    let ops = d
        .apply(
            Command::Rename {
                key: key("delay"),
                name: "Echo".into(),
            },
            &reg,
        )
        .unwrap();
    assert_eq!(ops, [SyncOp::ViewChanged]);
    assert_eq!(d.doc().devices[1].name, "Echo");
    let ops = d
        .apply(
            Command::SetColor {
                key: key("delay"),
                color: Some([1, 2, 3]),
            },
            &reg,
        )
        .unwrap();
    assert_eq!(ops, [SyncOp::ViewChanged]);
    assert_eq!(d.doc().devices[1].color, Some([1, 2, 3]));
}

#[test]
fn rename_trims_the_name_and_undoes_in_one_step() {
    let reg = registry();
    let mut d = three_devices(&reg);
    d.apply(
        Command::Rename {
            key: key("delay"),
            name: "  Echo  ".into(),
        },
        &reg,
    )
    .unwrap();
    assert_eq!(d.doc().devices[1].name, "Echo");
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().devices[1].name, "Stereo Delay");
    d.redo(&reg).unwrap();
    assert_eq!(d.doc().devices[1].name, "Echo");
}

#[test]
fn rename_to_a_blank_name_restores_the_default_name() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    for _ in 0..2 {
        d.apply(add(99, SYNTH, None), &reg).unwrap();
    }
    let rename = |key: &str, name: &str| Command::Rename {
        key: self::key(key),
        name: name.into(),
    };
    d.apply(rename("poly-synth-2", "Lead"), &reg).unwrap();
    // The default is the type's name made unique among the others, as when it was added.
    d.apply(rename("poly-synth-2", "   "), &reg).unwrap();
    assert_eq!(d.doc().devices[1].name, "Poly Synth 2");
    d.apply(rename("poly-synth", ""), &reg).unwrap();
    assert_eq!(d.doc().devices[0].name, "Poly Synth");
    d.undo(&reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(d.doc().devices[1].name, "Lead");
}

#[test]
fn view_commands_reject_unknown_device() {
    let reg = registry();
    let mut d = three_devices(&reg);
    let cmds = [
        Command::SetFolded {
            key: key("x"),
            folded: true,
        },
        Command::Rename {
            key: key("x"),
            name: "X".into(),
        },
        Command::SetColor {
            key: key("x"),
            color: None,
        },
    ];
    for c in cmds {
        assert_eq!(
            rejects(&mut d, c, &reg),
            ModelError::UnknownDevice("x".into())
        );
    }
}

// ---- Connect / Disconnect -------------------------------------------------------------------

#[test]
fn connect_keeps_cables_sorted_and_reports_routing() {
    let reg = registry();
    let mut d = three_devices(&reg);
    assert_eq!(
        d.apply(connect("mixer:master_r", "interface:out_2"), &reg)
            .unwrap(),
        [routing()]
    );
    d.apply(connect("synth:out_l", "delay:in_l"), &reg).unwrap();
    d.apply(connect("delay:out_l", "mixer:ch1_in_l"), &reg)
        .unwrap();
    let mut sorted = d.doc().cables.clone();
    sorted.sort();
    assert_eq!(d.doc().cables, sorted);
    assert_eq!(d.doc().cables.len(), 3);
}

#[test]
fn connect_rejects_unknown_device_and_socket() {
    let reg = registry();
    let mut d = three_devices(&reg);
    assert_eq!(
        rejects(&mut d, connect("nope:out_l", "delay:in_l"), &reg),
        ModelError::UnknownDevice("nope".into())
    );
    assert_eq!(
        rejects(&mut d, connect("synth:out_l", "nope:in_l"), &reg),
        ModelError::UnknownDevice("nope".into())
    );
    assert_eq!(
        rejects(&mut d, connect("synth:out_x", "delay:in_l"), &reg),
        ModelError::UnknownKey("out_x".into())
    );
    // Stereo out: the interface has out_1 and out_2 only.
    assert_eq!(
        rejects(&mut d, connect("synth:out_l", "interface:out_3"), &reg),
        ModelError::UnknownKey("out_3".into())
    );
    // New documents have two host inputs: in_1 and in_2 only.
    assert_eq!(
        rejects(&mut d, connect("interface:in_3", "delay:in_l"), &reg),
        ModelError::UnknownKey("in_3".into())
    );
    d.apply(Command::SetInputChannels(0), &reg).unwrap();
    assert_eq!(
        rejects(&mut d, connect("interface:in_1", "delay:in_l"), &reg),
        ModelError::UnknownKey("in_1".into())
    );
}

#[test]
fn connect_rejects_direction_kind_busy_input_and_cycles() {
    let reg = registry();
    let mut d = chain(&reg);
    let bad = |e: ModelError| matches!(e, ModelError::BadCable(_));
    // Direction: from an input, or to an output.
    assert!(bad(rejects(
        &mut d,
        connect("synth:notes_in", "delay:in_l"),
        &reg
    )));
    assert!(bad(rejects(
        &mut d,
        connect("synth:out_l", "delay:out_l"),
        &reg
    )));
    // Kind: audio into a CV input.
    assert!(bad(rejects(
        &mut d,
        connect("synth:out_l", "delay:time_cv_in"),
        &reg
    )));
    // Input already connected (also an exact duplicate).
    assert!(bad(rejects(
        &mut d,
        connect("synth:out_r", "delay:in_l"),
        &reg
    )));
    assert!(bad(rejects(
        &mut d,
        connect("synth:out_l", "delay:in_l"),
        &reg
    )));
    // Cycles: mixer feeds back into the delay; a device into itself.
    assert!(bad(rejects(
        &mut d,
        connect("mixer:master_l", "delay:time_cv_in"),
        &reg
    )));
    d.apply(
        Command::Disconnect {
            to: sock("delay:in_r"),
        },
        &reg,
    )
    .unwrap();
    assert!(bad(rejects(
        &mut d,
        connect("mixer:master_l", "delay:in_r"),
        &reg
    )));
    assert!(bad(rejects(
        &mut d,
        connect("delay:out_l", "delay:in_r"),
        &reg
    )));
}

#[test]
fn connect_through_interface_is_not_a_cycle() {
    let reg = registry();
    let mut d = three_devices(&reg);
    d.apply(connect("synth:out_l", "interface:out_1"), &reg)
        .unwrap();
    d.apply(connect("mixer:master_l", "interface:out_2"), &reg)
        .unwrap();
    d.apply(connect("synth:out_r", "mixer:ch1_in_l"), &reg)
        .unwrap();
}

#[test]
fn disconnect_removes_cable_and_reports_routing() {
    let reg = registry();
    let mut d = chain(&reg);
    let ops = d
        .apply(
            Command::Disconnect {
                to: sock("delay:in_l"),
            },
            &reg,
        )
        .unwrap();
    assert_eq!(ops, [routing()]);
    assert_eq!(d.doc().cables.len(), 5);
    assert!(!d.doc().cables.contains(&cable("synth:out_l", "delay:in_l")));
}

#[test]
fn disconnect_rejects_unknown_and_unconnected() {
    let reg = registry();
    let mut d = chain(&reg);
    let dis = |s: &str| Command::Disconnect { to: sock(s) };
    assert_eq!(
        rejects(&mut d, dis("nope:in_l"), &reg),
        ModelError::UnknownDevice("nope".into())
    );
    assert_eq!(
        rejects(&mut d, dis("delay:in_x"), &reg),
        ModelError::UnknownKey("in_x".into())
    );
    assert!(matches!(
        rejects(&mut d, dis("delay:time_cv_in"), &reg),
        ModelError::BadCable(_)
    ));
    assert!(matches!(
        rejects(&mut d, dis("delay:out_l"), &reg),
        ModelError::BadCable(_)
    ));
}

// ---- SetParam -------------------------------------------------------------------------------

fn set(k: &str, p: &str, value: f32, merge: Option<u64>) -> Command {
    Command::SetParam {
        key: key(k),
        param: p.into(),
        value,
        merge,
    }
}

#[test]
fn set_param_clamps_through_descriptor() {
    let reg = registry();
    let mut d = three_devices(&reg);
    let ops = d
        .apply(set("synth", "filter.cutoff", 50_000.0, None), &reg)
        .unwrap();
    assert_eq!(
        ops,
        [SyncOp::ParamChanged {
            key: key("synth"),
            param: "filter.cutoff".into(),
            value: 20_000.0
        }]
    );
    assert_eq!(
        d.doc().devices[0].params["filter.cutoff"].to_bits(),
        20_000f32.to_bits()
    );
    // Discrete params are rounded.
    let ops = d.apply(set("synth", "osc1.wave", 1.4, None), &reg).unwrap();
    assert!(
        matches!(&ops[0], SyncOp::ParamChanged { value, .. } if value.to_bits() == 1f32.to_bits())
    );
    d.apply(set("delay", "mix", 0.35, None), &reg).unwrap();
    assert_eq!(
        d.doc().devices[1].params["mix"].to_bits(),
        0.35f32.to_bits()
    );
}

#[test]
fn set_param_rejects_unknown_device_param_and_nan() {
    let reg = registry();
    let mut d = three_devices(&reg);
    assert_eq!(
        rejects(&mut d, set("nope", "mix", 0.1, None), &reg),
        ModelError::UnknownDevice("nope".into())
    );
    assert_eq!(
        rejects(&mut d, set("delay", "filter.cutoff", 0.1, None), &reg),
        ModelError::UnknownKey("filter.cutoff".into())
    );
    assert!(matches!(
        rejects(&mut d, set("delay", "mix", f32::NAN, None), &reg),
        ModelError::OutOfRange(_)
    ));
}

// ---- SetTempo -------------------------------------------------------------------------------

#[test]
fn set_tempo_in_range() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    assert_eq!(
        d.apply(Command::SetTempo(97.5), &reg).unwrap(),
        [SyncOp::TempoChanged(97.5)]
    );
    assert!((d.doc().tempo_bpm - 97.5).abs() < 1e-12);
    for ok in [20.0, 999.0] {
        d.apply(Command::SetTempo(ok), &reg).unwrap();
    }
    for bad in [19.99, 999.01, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            rejects(&mut d, Command::SetTempo(bad), &reg),
            ModelError::OutOfRange(_)
        ));
    }
}
