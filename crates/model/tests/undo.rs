//! Undo/redo: inverse steps, SyncOps on undo/redo, redo clearing, drag-merge.

mod common;

use atomik_model::{Command, Document, SyncOp, empty_doc};
use common::*;

fn set(k: &str, p: &str, value: f32, merge: Option<u64>) -> Command {
    Command::SetParam {
        key: key(k),
        param: p.into(),
        value,
        merge,
    }
}

fn param(d: &Document, k: &str, p: &str) -> Option<f32> {
    d.doc().device(&key(k)).unwrap().params.get(p).copied()
}

#[test]
fn empty_history_has_nothing_to_undo_or_redo() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    assert!(!d.can_undo() && !d.can_redo());
    assert_eq!(d.undo(&reg), None);
    assert_eq!(d.redo(&reg), None);
}

#[test]
fn undo_add_device_removes_it_routing_first() {
    let reg = registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("synth")), &reg).unwrap();
    assert!(d.can_undo());
    let ops = d.undo(&reg).unwrap();
    assert_eq!(ops, [routing(), SyncOp::DeviceRemoved(key("synth"))]);
    assert_eq!(d.doc(), &empty_doc());
    assert!(d.can_redo() && !d.can_undo());
    let ops = d.redo(&reg).unwrap();
    assert_eq!(ops, [SyncOp::DeviceAdded(key("synth")), routing()]);
    assert_eq!(keys(&d), ["synth"]);
}

#[test]
fn undo_remove_device_restores_index_params_view_and_cables() {
    let reg = registry();
    let mut d = chain(&reg);
    d.apply(set("delay", "mix", 0.5, None), &reg).unwrap();
    d.apply(set("delay", "feedback", 0.25, None), &reg).unwrap();
    d.apply(
        Command::SetFolded {
            key: key("delay"),
            folded: true,
        },
        &reg,
    )
    .unwrap();
    let before = d.doc().clone();
    d.apply(Command::RemoveDevice { key: key("delay") }, &reg)
        .unwrap();
    let ops = d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before);
    // The engine re-creates the device, re-applies its stored params, then re-routes.
    assert_eq!(
        ops,
        [
            SyncOp::DeviceAdded(key("delay")),
            SyncOp::ParamChanged {
                key: key("delay"),
                param: "feedback".into(),
                value: 0.25
            },
            SyncOp::ParamChanged {
                key: key("delay"),
                param: "mix".into(),
                value: 0.5
            },
            routing(),
        ]
    );
    let ops = d.redo(&reg).unwrap();
    assert_eq!(ops, [routing(), SyncOp::DeviceRemoved(key("delay"))]);
}

#[test]
fn undo_connect_disconnect_move_and_view() {
    let reg = registry();
    let mut d = chain(&reg);
    let original = d.doc().clone();
    let cmds = [
        Command::Disconnect {
            to: sock("delay:in_l"),
        },
        connect("synth:out_l", "mixer:ch2_in_l"),
        Command::MoveDevice {
            key: key("synth"),
            to: 2,
        },
        Command::Rename {
            key: key("mixer"),
            name: "Main".into(),
        },
        Command::SetColor {
            key: key("mixer"),
            color: Some([200, 10, 10]),
        },
        Command::SetFolded {
            key: key("mixer"),
            folded: true,
        },
        Command::SetTempo(140.0),
    ];
    let mut states = vec![original.clone()];
    for c in cmds {
        d.apply(c, &reg).unwrap();
        states.push(d.doc().clone());
    }
    let expected_undo_ops = [
        vec![SyncOp::TempoChanged(120.0)],
        vec![SyncOp::ViewChanged],
        vec![SyncOp::ViewChanged],
        vec![SyncOp::ViewChanged],
        vec![SyncOp::ViewChanged],
        vec![routing()],
        vec![routing()],
    ];
    for (i, ops) in expected_undo_ops.iter().enumerate() {
        assert_eq!(&d.undo(&reg).unwrap(), ops, "undo #{i}");
        assert_eq!(d.doc(), &states[states.len() - 2 - i], "after undo #{i}");
    }
    assert_eq!(d.doc(), &original);
    for state in &states[1..] {
        d.redo(&reg).unwrap();
        assert_eq!(d.doc(), state);
    }
    assert!(!d.can_redo());
}

#[test]
fn undo_set_param_restores_default_and_reports_it() {
    let reg = registry();
    let mut d = three_devices(&reg);
    d.apply(set("delay", "mix", 0.9, None), &reg).unwrap();
    let ops = d.undo(&reg).unwrap();
    let default = d.doc().param_value(&key("delay"), "mix", &reg).unwrap();
    assert_eq!(param(&d, "delay", "mix"), None, "back to 'not stored'");
    assert_eq!(
        ops,
        [SyncOp::ParamChanged {
            key: key("delay"),
            param: "mix".into(),
            value: default
        }]
    );
}

#[test]
fn new_command_clears_redo() {
    let reg = registry();
    let mut d = three_devices(&reg);
    d.undo(&reg).unwrap();
    assert!(d.can_redo());
    d.apply(Command::SetTempo(90.0), &reg).unwrap();
    assert!(!d.can_redo());
    assert_eq!(d.redo(&reg), None);
}

#[test]
fn rejected_command_keeps_redo() {
    let reg = registry();
    let mut d = three_devices(&reg);
    d.undo(&reg).unwrap();
    d.apply(Command::SetTempo(5.0), &reg).unwrap_err();
    assert!(d.can_redo());
}

#[test]
fn drag_merge_collapses_into_one_step() {
    let reg = registry();
    let mut d = three_devices(&reg);
    for v in [0.1, 0.2, 0.3, 0.4] {
        let ops = d.apply(set("delay", "mix", v, Some(7)), &reg).unwrap();
        assert_eq!(ops.len(), 1, "every drag step still syncs the engine");
    }
    assert_eq!(param(&d, "delay", "mix"), Some(0.4));
    d.undo(&reg).unwrap();
    assert_eq!(
        param(&d, "delay", "mix"),
        None,
        "one undo reverts the whole drag"
    );
    d.redo(&reg).unwrap();
    assert_eq!(
        param(&d, "delay", "mix"),
        Some(0.4),
        "redo restores the final value"
    );
    d.undo(&reg).unwrap();
    // The step before the drag (adding the mixer) is intact.
    d.undo(&reg).unwrap();
    assert_eq!(keys(&d), ["synth", "delay"]);
}

#[test]
fn end_merge_splits_drags_with_same_id() {
    let reg = registry();
    let mut d = three_devices(&reg);
    d.apply(set("delay", "mix", 0.1, Some(1)), &reg).unwrap();
    d.apply(set("delay", "mix", 0.2, Some(1)), &reg).unwrap();
    d.end_merge();
    d.apply(set("delay", "mix", 0.3, Some(1)), &reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(param(&d, "delay", "mix"), Some(0.2));
    d.undo(&reg).unwrap();
    assert_eq!(param(&d, "delay", "mix"), None);
}

#[test]
fn merge_breaks_on_other_id_param_device_or_command() {
    let reg = registry();
    let mut d = three_devices(&reg);
    let steps = [
        set("delay", "mix", 0.1, Some(1)),
        set("delay", "mix", 0.2, Some(2)),             // other id
        set("delay", "feedback", 0.3, Some(2)),        // other param
        set("synth", "filter.cutoff", 500.0, Some(2)), // other device
        set("synth", "filter.cutoff", 600.0, None),    // standalone
        set("synth", "filter.cutoff", 700.0, Some(2)),
        Command::SetTempo(100.0), // a different command…
        set("synth", "filter.cutoff", 800.0, Some(2)), // …so this starts a new step
    ];
    let n = steps.len();
    for c in steps {
        d.apply(c, &reg).unwrap();
    }
    let mut undone = 0;
    while d.can_undo() {
        d.undo(&reg).unwrap();
        undone += 1;
    }
    assert_eq!(undone, n + 3, "no merging happened (+3 device adds)");
}

#[test]
fn merge_does_not_survive_undo() {
    let reg = registry();
    let mut d = three_devices(&reg);
    d.apply(set("delay", "mix", 0.3, None), &reg).unwrap();
    d.apply(set("delay", "mix", 0.1, Some(1)), &reg).unwrap();
    d.undo(&reg).unwrap();
    // The last step is now the standalone 0.3; a new drag must not merge into it.
    d.apply(set("delay", "mix", 0.2, Some(1)), &reg).unwrap();
    d.undo(&reg).unwrap();
    assert_eq!(
        param(&d, "delay", "mix"),
        Some(0.3),
        "the drag after undo is its own step"
    );
}
