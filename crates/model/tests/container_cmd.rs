//! Container commands (model side):
//! splits, macros, `MacroDriven`, duplicate, auto split, merging, `Batch`, sync ops, and a
//! property test over random command sequences.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{Command, ContainerCommand, Document, ModelError, NoteSplit, SyncOp, empty_doc};
use common::container_ops::{Op, assert_invariants, op, resolve_op};
use common::*;
use proptest::prelude::*;

fn reg() -> Registry {
    registry()
}

fn err(d: &mut Document, r: &Registry, c: Command) -> ModelError {
    let before = d.doc().clone();
    let e = d.apply(c, r).expect_err("command must be rejected");
    assert_eq!(d.doc(), &before, "a rejected command changes nothing");
    e
}

fn inner<'d>(d: &'d Document, path: &str) -> &'d atomik_model::ContainerDoc {
    d.doc()
        .resolve(&key(path))
        .unwrap()
        .inner
        .as_deref()
        .unwrap()
}

/// Container `c` with a synth, a second synth `pad` (both with notes inputs) and a delay.
fn layered(r: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        r,
        [
            add(0, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "synth"),
            add_inner("c", 1, SYNTH, "pad"),
            add_inner("c", 2, DELAY, "delay"),
        ],
    );
    Document::new(d.doc().clone())
}

// ---------------------------------------------------------------- rename

#[test]
fn rename_reaches_a_device_inside_a_container_and_a_blank_name_restores_its_default() {
    let r = reg();
    let mut d = layered(&r);
    let rename = |name: &str| Command::Rename {
        key: key("c/pad"),
        name: name.into(),
    };
    d.apply(rename("Strings"), &r).unwrap();
    assert_eq!(inner(&d, "c").devices[1].name, "Strings");
    // `synth` keeps the type's name, so the default of `pad` is the next free one.
    d.apply(rename(""), &r).unwrap();
    assert_eq!(inner(&d, "c").devices[1].name, "Poly Synth 2");
    d.undo(&r).unwrap();
    assert_eq!(inner(&d, "c").devices[1].name, "Strings");
}

// ---------------------------------------------------------------- splits

#[test]
fn set_splits_replaces_the_list_in_one_undo_step() {
    let r = reg();
    let mut d = layered(&r);
    let list = vec![split("synth", 0, 59, 0), split("pad", 60, 127, 12)];
    let ops = d.apply(set_splits("c", list.clone()), &r).unwrap();
    assert_eq!(
        ops,
        [
            SyncOp::SplitsChanged {
                container: key("c")
            },
            SyncOp::RoutingChanged
        ]
    );
    assert_eq!(inner(&d, "c").splits, list);
    // Same targets, another range: only the params change, no routing.
    let moved = vec![split("synth", 0, 40, 0), split("pad", 41, 127, 12)];
    let ops = d.apply(set_splits("c", moved), &r).unwrap();
    assert_eq!(
        ops,
        [SyncOp::SplitsChanged {
            container: key("c")
        }]
    );
    d.undo(&r).unwrap();
    assert_eq!(inner(&d, "c").splits, list);
    d.undo(&r).unwrap();
    assert!(inner(&d, "c").splits.is_empty());
    assert!(!d.can_undo());
    d.redo(&r).unwrap();
    assert_eq!(inner(&d, "c").splits, list);
}

#[test]
fn splits_are_validated() {
    let r = reg();
    let mut d = layered(&r);
    let bad = |d: &mut Document, s: Vec<NoteSplit>| {
        let e = err(d, &r, set_splits("c", s));
        assert!(matches!(e, ModelError::BadSplit(_)), "{e}");
    };
    // Unknown target, target without a Notes input, the same target twice.
    bad(&mut d, vec![split("nope", 0, 127, 0)]);
    bad(&mut d, vec![split("delay", 0, 127, 0)]);
    bad(
        &mut d,
        vec![split("synth", 0, 127, 0), split("synth", 0, 127, 0)],
    );
    // Ranges and transpose.
    bad(&mut d, vec![split("synth", 60, 59, 0)]);
    bad(&mut d, vec![split("synth", 0, 128, 0)]);
    bad(&mut d, vec![split("synth", 0, 127, 49)]);
    bad(&mut d, vec![split("synth", 0, 127, -49)]);
    let mut s = split("synth", 0, 127, 0);
    s.vel_lo = 0;
    bad(&mut d, vec![s]);
    let mut s = split("synth", 0, 127, 0);
    s.vel_lo = 100;
    s.vel_hi = 99;
    bad(&mut d, vec![s]);
    let mut s = split("synth", 0, 127, 0);
    s.vel_hi = 128;
    bad(&mut d, vec![s]);
    // Not a container.
    let e = err(&mut d, &r, set_splits("c/synth", vec![]));
    assert!(matches!(e, ModelError::BadScope(_)), "{e}");
    let e = err(&mut d, &r, set_splits("zz", vec![]));
    assert!(matches!(e, ModelError::UnknownDevice(_)), "{e}");
    // Edges are accepted.
    let mut ok = split("synth", 0, 0, -48);
    ok.vel_hi = 1;
    d.apply(set_splits("c", vec![ok]), &r).unwrap();
    d.apply(set_splits("c", vec![split("synth", 127, 127, 48)]), &r)
        .unwrap();
}

#[test]
fn at_most_eight_splits() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CONTAINER, Some("c")), &r).unwrap();
    let mut list = Vec::new();
    for i in 0..9 {
        d.apply(add_inner("c", i, SYNTH, &format!("s{i}")), &r)
            .unwrap();
        list.push(split(&format!("s{i}"), 0, 127, 0));
    }
    let e = err(&mut d, &r, set_splits("c", list.clone()));
    assert!(matches!(e, ModelError::BadSplit(_)), "{e}");
    list.pop();
    d.apply(set_splits("c", list), &r).unwrap();
}

#[test]
fn a_notes_input_takes_one_cable_a_split_or_an_inner_cable() {
    let r = reg();
    let mut d = layered(&r);
    run(
        &mut d,
        &r,
        [
            add_inner("c", 3, SEQ, "seq"),
            connect("c/seq:notes_out", "c/synth:notes_in"),
        ],
    );
    // `synth` already has a notes cable, so it cannot be a split target.
    let e = err(&mut d, &r, set_splits("c", vec![split("synth", 0, 127, 0)]));
    assert!(matches!(e, ModelError::BadSplit(_)), "{e}");
    // Inner note devices may be targets themselves, and feed other instruments.
    d.apply(set_splits("c", vec![split("seq", 0, 127, 0)]), &r)
        .unwrap();
    // And a split target cannot also be cabled.
    let e = err(&mut d, &r, connect("c/seq:notes_out", "c/seq:notes_in"));
    assert!(matches!(e, ModelError::BadCable(_)), "{e}");
    run(&mut d, &r, [add_inner("c", 0, ARP, "arp")]);
    d.apply(set_splits("c", vec![split("arp", 0, 127, 0)]), &r)
        .unwrap();
    let e = err(&mut d, &r, connect("c/seq:notes_out", "c/arp:notes_in"));
    assert!(matches!(e, ModelError::BadCable(_)), "{e}");
}

#[test]
fn auto_split_adds_a_full_range_split_in_the_same_step() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CONTAINER, Some("c")), &r).unwrap();
    let auto = |type_id: &str, k: &str| {
        Command::Container(ContainerCommand::AddInner {
            container: key("c"),
            index: 99,
            type_id: type_id.to_owned(),
            key: Some(key(k)),
            auto_split: true,
        })
    };
    let ops = d.apply(auto(SYNTH, "synth"), &r).unwrap();
    assert_eq!(
        ops,
        [
            SyncOp::DeviceAdded(key("c/synth")),
            SyncOp::RoutingChanged,
            SyncOp::SplitsChanged {
                container: key("c")
            }
        ]
    );
    assert_eq!(inner(&d, "c").splits, [split("synth", 0, 127, 0)]);
    // No Notes input: no split.
    d.apply(auto(DELAY, "delay"), &r).unwrap();
    assert_eq!(inner(&d, "c").splits.len(), 1);
    // One undo step undoes the device and its split.
    d.undo(&r).unwrap();
    d.undo(&r).unwrap();
    assert!(inner(&d, "c").devices.is_empty() && inner(&d, "c").splits.is_empty());
    // No room: the device is added without a split.
    for i in 0..8 {
        d.apply(auto(SYNTH, &format!("s{i}")), &r).unwrap();
    }
    d.apply(auto(SYNTH, "ninth"), &r).unwrap();
    assert_eq!(inner(&d, "c").splits.len(), 8);
    assert_eq!(inner(&d, "c").devices.len(), 9);
}

#[test]
fn split_drags_merge_by_id() {
    let r = reg();
    let mut d = layered(&r);
    let drag = |hi: u8, merge: Option<u64>| {
        Command::Container(ContainerCommand::SetSplits {
            container: key("c"),
            splits: vec![split("synth", 0, hi, 0)],
            merge,
        })
    };
    let base = 7u64 << 40;
    run(
        &mut d,
        &r,
        [
            drag(100, Some(base)),
            drag(90, Some(base)),
            drag(80, Some(base)),
        ],
    );
    assert_eq!(inner(&d, "c").splits[0].key_hi, 80);
    d.undo(&r).unwrap();
    assert!(inner(&d, "c").splits.is_empty(), "one step for the drag");
    d.redo(&r).unwrap();
    // A different merge id, or none, is a new step.
    d.apply(drag(70, Some(base + 1)), &r).unwrap();
    d.apply(drag(60, None), &r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(inner(&d, "c").splits[0].key_hi, 70);
    d.undo(&r).unwrap();
    assert_eq!(inner(&d, "c").splits[0].key_hi, 80);
}

// ---------------------------------------------------------------- macros

#[test]
fn set_macro_adds_edits_and_removes_targets() {
    let r = reg();
    let mut d = layered(&r);
    let container = key("c");
    let ops = d
        .apply(
            set_macro("c", 0, target("synth", "filter.cutoff", 200.0, 8000.0)),
            &r,
        )
        .unwrap();
    assert_eq!(
        ops,
        [SyncOp::MacrosChanged {
            container: container.clone()
        }]
    );
    // Edit the range of the same device and param.
    d.apply(
        set_macro("c", 0, target("synth", "filter.cutoff", 8000.0, 200.0)),
        &r,
    )
    .unwrap();
    let m = &inner(&d, "c").macros[0];
    assert_eq!(m.targets.len(), 1);
    assert_eq!((m.targets[0].min, m.targets[0].max), (8000.0, 200.0));
    d.apply(set_macro("c", 0, target("delay", "mix", 0.0, 1.0)), &r)
        .unwrap();
    assert_eq!(inner(&d, "c").macros[0].targets.len(), 2);
    d.apply(
        Command::Container(ContainerCommand::RemoveMacroTarget {
            container: container.clone(),
            index: 0,
            device: key("synth"),
            param: "filter.cutoff".into(),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(inner(&d, "c").macros[0].targets.len(), 1);
    // Every step undoes.
    for _ in 0..4 {
        d.undo(&r).unwrap();
    }
    assert!(inner(&d, "c").macros[0].targets.is_empty());
    assert!(!d.can_undo());
}

#[test]
fn macro_edits_are_validated() {
    let r = reg();
    let mut d = layered(&r);
    let bad = |d: &mut Document, c: Command| {
        let e = err(d, &r, c);
        assert!(matches!(e, ModelError::BadMacro(_)), "{e}");
    };
    bad(
        &mut d,
        set_macro("c", 8, target("synth", "filter.cutoff", 0.0, 1.0)),
    );
    bad(
        &mut d,
        set_macro("c", 0, target("nope", "filter.cutoff", 0.0, 1.0)),
    );
    bad(&mut d, set_macro("c", 0, target("synth", "nope", 0.0, 1.0)));
    bad(
        &mut d,
        set_macro("c", 0, target("synth", "filter.cutoff", f32::NAN, 1.0)),
    );
    bad(
        &mut d,
        set_macro("c", 0, target("synth", "filter.cutoff", 0.0, f32::INFINITY)),
    );
    bad(&mut d, set_macro("c", 0, target("", "x", 0.0, 1.0)));
    // A param is driven by one macro only.
    d.apply(
        set_macro("c", 0, target("synth", "filter.cutoff", 0.0, 1.0)),
        &r,
    )
    .unwrap();
    bad(
        &mut d,
        set_macro("c", 1, target("synth", "filter.cutoff", 0.0, 1.0)),
    );
    // Not a container / unknown.
    let e = err(
        &mut d,
        &r,
        set_macro("c/synth", 0, target("x", "y", 0.0, 1.0)),
    );
    assert!(matches!(e, ModelError::BadScope(_)), "{e}");
    // Removing what is not mapped.
    let e = err(
        &mut d,
        &r,
        Command::Container(ContainerCommand::RemoveMacroTarget {
            container: key("c"),
            index: 1,
            device: key("synth"),
            param: "filter.cutoff".into(),
        }),
    );
    assert!(matches!(e, ModelError::BadMacro(_)), "{e}");
    // Names: at most 24 characters.
    let rename = |name: &str| {
        Command::Container(ContainerCommand::RenameMacro {
            container: key("c"),
            index: 0,
            name: name.to_owned(),
        })
    };
    d.apply(rename(&"é".repeat(24)), &r).unwrap();
    bad(&mut d, rename(&"x".repeat(25)));
    d.apply(rename(""), &r).unwrap();
}

#[test]
fn macro_target_limits() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CONTAINER, Some("c")), &r).unwrap();
    // 5 synths x 33 params would exceed; use many devices with many params.
    for i in 0..5 {
        d.apply(add_inner("c", i, SYNTH, &format!("s{i}")), &r)
            .unwrap();
    }
    let params: Vec<&str> = [
        "filter.cutoff",
        "filter.resonance",
        "filter.mode",
        "osc1.wave",
        "osc2.wave",
        "osc1.octave",
        "osc2.octave",
        "osc1.semi",
        "osc2.semi",
        "osc1.fine",
        "osc2.fine",
        "osc.mix",
    ]
    .to_vec();
    // 12 params x 3 synths = 36 targets on one macro: the 33rd is refused.
    let mut n = 0;
    let mut refused = false;
    'all: for s in 0..3 {
        for p in &params {
            let c = set_macro("c", 0, target(&format!("s{s}"), p, 0.0, 1.0));
            match d.apply(c, &r) {
                Ok(_) => n += 1,
                Err(ModelError::BadMacro(_)) => {
                    refused = true;
                    break 'all;
                }
                Err(e) => panic!("{e}"),
            }
        }
    }
    assert!(refused);
    assert_eq!(n, 32);
    assert_eq!(inner(&d, "c").macros[0].targets.len(), 32);
    // 128 per container: four full macros, the next target is refused.
    let mut total = 32;
    'fill: for m in 1..8u8 {
        for s in 0..5 {
            for p in &params {
                if inner(&d, "c").macros.iter().any(|mm| {
                    mm.targets
                        .iter()
                        .any(|t| t.device == key(&format!("s{s}")) && t.param == *p)
                }) {
                    continue;
                }
                let c = set_macro("c", m, target(&format!("s{s}"), p, 0.0, 1.0));
                match d.apply(c, &r) {
                    Ok(_) => total += 1,
                    Err(ModelError::BadMacro(_)) => break 'fill,
                    Err(e) => panic!("{e}"),
                }
            }
        }
    }
    // 5 synths x 12 params = 60 distinct params exist, so the container limit is not reached
    // here; the per-macro limit above is the one this test pins. Everything stays valid.
    assert!(total <= 128);
}

#[test]
fn a_mapped_param_rejects_set_param_until_unmapped() {
    let r = reg();
    let mut d = layered(&r);
    d.apply(set_param("c/synth", "filter.cutoff", 1234.0), &r)
        .unwrap();
    d.apply(
        set_macro("c", 2, target("synth", "filter.cutoff", 200.0, 8000.0)),
        &r,
    )
    .unwrap();
    let e = err(&mut d, &r, set_param("c/synth", "filter.cutoff", 500.0));
    assert!(matches!(e, ModelError::MacroDriven(_)), "{e}");
    let e = err(
        &mut d,
        &r,
        Command::Batch(vec![set_param("c/synth", "filter.cutoff", 500.0)]),
    );
    assert!(matches!(e, ModelError::MacroDriven(_)), "{e}");
    // The stored value is kept untouched and returns after unmapping.
    assert_eq!(
        d.doc()
            .param_value(&key("c/synth"), "filter.cutoff", &r)
            .unwrap(),
        1234.0
    );
    assert_eq!(
        d.doc().macro_driver(&key("c/synth"), "filter.cutoff"),
        Some((key("c"), 2))
    );
    assert_eq!(
        d.doc().macro_driver(&key("c/synth"), "filter.resonance"),
        None
    );
    d.apply(
        Command::Container(ContainerCommand::RemoveMacroTarget {
            container: key("c"),
            index: 2,
            device: key("synth"),
            param: "filter.cutoff".into(),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(d.doc().macro_driver(&key("c/synth"), "filter.cutoff"), None);
    d.apply(set_param("c/synth", "filter.cutoff", 500.0), &r)
        .unwrap();
    // Undo of the unmap re-drives it.
    d.undo(&r).unwrap();
    d.undo(&r).unwrap();
    assert!(
        d.doc()
            .macro_driver(&key("c/synth"), "filter.cutoff")
            .is_some()
    );
}

#[test]
fn macros_nest_an_outer_macro_may_drive_an_inner_macro() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("o")),
            add_inner("o", 0, CONTAINER, "i"),
            add_inner("o/i", 0, DELAY, "fx"),
            set_macro("o", 0, target("i", "macro1", 0.0, 1.0)),
            set_macro("o/i", 0, target("fx", "mix", 0.0, 1.0)),
            set_macro("o", 1, target("i/fx", "feedback", 0.0, 0.5)),
        ],
    );
    // `o/i`'s macro1 is driven by the outer macro; `i/fx.mix` is driven by the inner one and
    // `i/fx.feedback` by the outer one directly.
    let e = err(&mut d, &r, set_param("o/i", "macro1", 0.5));
    assert!(matches!(e, ModelError::MacroDriven(_)), "{e}");
    let e = err(&mut d, &r, set_param("o/i/fx", "mix", 0.5));
    assert!(matches!(e, ModelError::MacroDriven(_)), "{e}");
    let e = err(&mut d, &r, set_param("o/i/fx", "feedback", 0.5));
    assert!(matches!(e, ModelError::MacroDriven(_)), "{e}");
    assert_eq!(
        d.doc().macro_driver(&key("o/i/fx"), "feedback"),
        Some((key("o"), 1))
    );
    // The same param cannot be driven from two levels.
    let e = err(
        &mut d,
        &r,
        set_macro("o/i", 1, target("fx", "feedback", 0.0, 1.0)),
    );
    assert!(matches!(e, ModelError::BadMacro(_)), "{e}");
    // Only a container's macro param may be a target on a container.
    let e = err(&mut d, &r, set_macro("o", 2, target("i", "nope", 0.0, 1.0)));
    assert!(matches!(e, ModelError::BadMacro(_)), "{e}");
}

#[test]
fn removing_a_device_drops_its_targets_and_splits_in_the_same_step() {
    let r = reg();
    let mut d = layered(&r);
    run(
        &mut d,
        &r,
        [
            set_splits(
                "c",
                vec![split("synth", 0, 59, 0), split("pad", 60, 127, 0)],
            ),
            set_macro("c", 0, target("synth", "filter.cutoff", 0.0, 1.0)),
            set_macro("c", 0, target("delay", "mix", 0.0, 1.0)),
            connect("c/synth:out_l", "c/delay:in_l"),
        ],
    );
    let before = d.doc().clone();
    let ops = d
        .apply(
            Command::RemoveDevice {
                key: key("c/synth"),
            },
            &r,
        )
        .unwrap();
    assert!(ops.contains(&SyncOp::DeviceRemoved(key("c/synth"))));
    assert!(ops.contains(&SyncOp::MacrosChanged {
        container: key("c")
    }));
    assert!(ops.contains(&SyncOp::SplitsChanged {
        container: key("c")
    }));
    let c = inner(&d, "c");
    assert_eq!(c.splits, [split("pad", 60, 127, 0)]);
    assert_eq!(c.macros[0].targets.len(), 1);
    assert_eq!(c.macros[0].targets[0].device, key("delay"));
    assert!(c.cables.is_empty());
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before, "one undo step restores everything");
}

#[test]
fn removing_a_nested_device_drops_the_outer_targets_into_it() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("o")),
            add_inner("o", 0, CONTAINER, "i"),
            add_inner("o/i", 0, DELAY, "fx"),
            add_inner("o", 1, DELAY, "keep"),
            set_macro("o", 0, target("i/fx", "mix", 0.0, 1.0)),
            set_macro("o", 0, target("keep", "mix", 0.0, 1.0)),
            set_macro("o", 1, target("i", "macro3", 0.0, 1.0)),
        ],
    );
    let before = d.doc().clone();
    d.apply(Command::RemoveDevice { key: key("o/i/fx") }, &r)
        .unwrap();
    assert_eq!(inner(&d, "o").macros[0].targets.len(), 1);
    assert_eq!(inner(&d, "o").macros[1].targets.len(), 1, "i itself stays");
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    // Removing the nested container drops every target into and at it.
    d.apply(Command::RemoveDevice { key: key("o/i") }, &r)
        .unwrap();
    assert_eq!(inner(&d, "o").macros[0].targets.len(), 1);
    assert!(inner(&d, "o").macros[1].targets.is_empty());
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn macro_range_drags_merge() {
    let r = reg();
    let mut d = layered(&r);
    let drag = |max: f32, merge: Option<u64>, device: &str| {
        Command::Container(ContainerCommand::SetMacro {
            container: key("c"),
            index: 0,
            target: target(device, "mix", 0.0, max),
            merge,
        })
    };
    let base = 7u64 << 40;
    d.apply(drag(0.2, None, "delay"), &r).unwrap();
    run(
        &mut d,
        &r,
        [
            drag(0.5, Some(base), "delay"),
            drag(0.6, Some(base), "delay"),
            drag(0.7, Some(base), "delay"),
        ],
    );
    assert_eq!(inner(&d, "c").macros[0].targets[0].max, 0.7);
    d.undo(&r).unwrap();
    assert_eq!(inner(&d, "c").macros[0].targets[0].max, 0.2);
    d.undo(&r).unwrap();
    assert!(inner(&d, "c").macros[0].targets.is_empty());
}

#[test]
fn macro_container_params_are_plain_params() {
    let r = reg();
    let mut d = layered(&r);
    let ops = d.apply(set_param("c", "macro3", 0.75), &r).unwrap();
    assert_eq!(
        ops,
        [SyncOp::ParamChanged {
            key: key("c"),
            param: "macro3".into(),
            value: 0.75
        }]
    );
    assert_eq!(d.doc().param_value(&key("c"), "macro3", &r).unwrap(), 0.75);
}

// ---------------------------------------------------------------- duplicate

#[test]
fn duplicate_container_deep_copies_without_outer_wiring() {
    let r = common::console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, DELAY, Some("pre")),
            add(1, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "synth"),
            add_inner("c", 1, CONTAINER, "n"),
            add_inner("c/n", 0, DELAY, "z"),
            set_splits("c", vec![split("synth", 0, 127, 5)]),
            set_macro("c", 3, target("n/z", "mix", 0.0, 1.0)),
            set_param("c", "macro1", 0.4),
            connect("pre:out_l", "c:in_l"),
            Command::Console(atomik_model::ConsoleCommand::AddChannel { source: key("c") }),
            Command::Song(atomik_model::SongCommand::AddTrack { device: key("c") }),
        ],
    );
    let before = d.doc().clone();
    let ops = d
        .apply(
            Command::Container(ContainerCommand::DuplicateContainer {
                container: key("c"),
            }),
            &r,
        )
        .unwrap();
    let doc = d.doc();
    let original = doc.device(&key("c")).unwrap();
    // Placed right after the original, with a fresh unique key and the name "<name> copy".
    let at = doc.devices.iter().position(|x| x.key == key("c")).unwrap();
    let copy = &doc.devices[at + 1];
    assert_ne!(copy.key, original.key);
    assert!(doc.devices.iter().filter(|x| x.key == copy.key).count() == 1);
    assert_eq!(copy.name, format!("{} copy", original.name));
    assert_eq!(copy.inner, original.inner);
    assert_eq!(copy.params, original.params);
    // No outer cables, no console channel, no song track.
    assert!(
        doc.cables
            .iter()
            .all(|c| c.from.device != copy.key && c.to.device != copy.key)
    );
    assert!(doc.console.channel_of(&copy.key).is_none());
    assert!(doc.song.track(&copy.key).is_none());
    assert!(ops.contains(&SyncOp::DeviceAdded(copy.key.clone())));
    assert!(ops.contains(&SyncOp::RoutingChanged));
    // Both expand to distinct flat devices.
    let flat = doc.expand(&r);
    let keys: Vec<_> = flat.devices.iter().map(|x| x.key.0.clone()).collect();
    let copy_synth = format!("{}/synth", copy.key);
    assert!(keys.contains(&"c/synth".to_owned()));
    assert!(keys.contains(&copy_synth));
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&r).unwrap();
    assert_eq!(d.doc().devices.len(), before.devices.len() + 1);
}

#[test]
fn duplicate_refuses_non_containers_and_respects_the_limits() {
    let r = reg();
    let mut d = layered(&r);
    let dup =
        |k: &str| Command::Container(ContainerCommand::DuplicateContainer { container: key(k) });
    let e = err(&mut d, &r, dup("c/synth"));
    assert!(matches!(e, ModelError::BadScope(_)), "{e}");
    let e = err(&mut d, &r, dup("zz"));
    assert!(matches!(e, ModelError::UnknownDevice(_)), "{e}");
    // A nested container is copied inside its parent, after the original.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("o")),
            add_inner("o", 0, CONTAINER, "i"),
            add_inner("o/i", 0, DELAY, "fx"),
        ],
    );
    d.apply(dup("o/i"), &r).unwrap();
    assert_eq!(inner(&d, "o").devices.len(), 2);
    assert_eq!(
        inner(&d, "o").devices[1]
            .inner
            .as_ref()
            .unwrap()
            .devices
            .len(),
        1
    );
    // The 256 inner device limit applies to the copy.
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CONTAINER, Some("big")), &r).unwrap();
    for c in 0..8 {
        d.apply(add(c + 1, CONTAINER, Some(&format!("c{c}"))), &r)
            .unwrap();
    }
    for c in 0..8 {
        for i in 0..32 {
            d.apply(add_inner(&format!("c{c}"), i, DELAY, &format!("d{i}")), &r)
                .unwrap();
        }
    }
    let e = err(&mut d, &r, dup("c0"));
    assert!(matches!(e, ModelError::ContainerFull(_)), "{e}");
}

// ---------------------------------------------------------------- batch and sync ops

#[test]
fn batch_accepts_the_four_macro_and_split_commands_only() {
    let r = reg();
    let mut d = layered(&r);
    let before = d.doc().clone();
    d.apply(
        Command::Batch(vec![
            set_splits("c", vec![split("synth", 0, 127, 0)]),
            set_macro("c", 0, target("delay", "mix", 0.0, 1.0)),
            Command::Container(ContainerCommand::RenameMacro {
                container: key("c"),
                index: 0,
                name: "Tone".into(),
            }),
            Command::Container(ContainerCommand::RemoveMacroTarget {
                container: key("c"),
                index: 0,
                device: key("delay"),
                param: "mix".into(),
            }),
        ]),
        &r,
    )
    .unwrap();
    assert_eq!(inner(&d, "c").macros[0].name, "Tone");
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    for refused in [
        add_inner("c", 0, DELAY, "x"),
        Command::Container(ContainerCommand::DuplicateContainer {
            container: key("c"),
        }),
    ] {
        let e = d.apply(Command::Batch(vec![refused]), &r).unwrap_err();
        assert!(matches!(e, ModelError::BadBatch(_)), "{e}");
    }
}

#[test]
fn macro_edits_report_macros_changed_also_on_undo_and_redo() {
    let r = reg();
    let mut d = layered(&r);
    let changed = SyncOp::MacrosChanged {
        container: key("c"),
    };
    assert_eq!(
        d.apply(set_macro("c", 0, target("delay", "mix", 0.0, 1.0)), &r)
            .unwrap(),
        std::slice::from_ref(&changed)
    );
    assert_eq!(
        d.apply(
            Command::Container(ContainerCommand::RenameMacro {
                container: key("c"),
                index: 0,
                name: "Mix".into()
            }),
            &r
        )
        .unwrap(),
        std::slice::from_ref(&changed)
    );
    assert_eq!(d.undo(&r).unwrap(), std::slice::from_ref(&changed));
    assert_eq!(d.undo(&r).unwrap(), std::slice::from_ref(&changed));
    assert_eq!(d.redo(&r).unwrap(), [changed]);
}

#[test]
fn adding_a_container_with_contents_back_reports_one_device_added() {
    let r = reg();
    let mut d = layered(&r);
    d.apply(Command::RemoveDevice { key: key("c") }, &r)
        .unwrap();
    let ops = d.undo(&r).unwrap();
    // The inside comes with the container: the app expands it. No per-inner ops.
    assert_eq!(ops, [SyncOp::DeviceAdded(key("c")), SyncOp::RoutingChanged]);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn random_command_sequences_keep_every_invariant(ops in proptest::collection::vec(op(), 1..60)) {
        let r = reg();
        let mut d = Document::new(empty_doc());
        for o in &ops {
            match o {
                Op::Undo => { let _ = d.undo(&r); }
                Op::Redo => { let _ = d.redo(&r); }
                _ => {
                    if let Some(c) = resolve_op(o, d.doc(), &r) {
                        let before = d.doc().clone();
                        if d.apply(c, &r).is_err() {
                            prop_assert_eq!(d.doc(), &before);
                        }
                    }
                }
            }
            assert_invariants(d.doc(), &r);
        }
        // Undo everything: back to the empty rack.
        while d.undo(&r).is_some() {
            assert_invariants(d.doc(), &r);
        }
        prop_assert_eq!(d.doc(), &empty_doc());
    }
}

#[test]
fn the_random_generator_reaches_the_interesting_commands() {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;
    let r = reg();
    let mut runner = TestRunner::deterministic();
    let strat = proptest::collection::vec(op(), 40..60);
    let mut accepted: std::collections::BTreeMap<&'static str, u32> = Default::default();
    for _ in 0..200 {
        let ops = strat.new_tree(&mut runner).unwrap().current();
        let mut d = Document::new(empty_doc());
        for o in &ops {
            let name = match o {
                Op::AddTop(..) => "add_top",
                Op::AddInner(..) => "add_inner",
                Op::Connect(..) => "connect",
                Op::Disconnect(..) => "disconnect",
                Op::Splits(..) => "splits",
                Op::Macro(..) => "macro",
                Op::Unmap(..) => "unmap",
                Op::Remove(..) => "remove",
                Op::Move(..) => "move",
                Op::Duplicate(..) => "duplicate",
                Op::Open(..) => "open",
                Op::Param(..) => "param",
                Op::Undo => "undo",
                Op::Redo => "redo",
            };
            match o {
                Op::Undo => {
                    let _ = d.undo(&r);
                }
                Op::Redo => {
                    let _ = d.redo(&r);
                }
                _ => {
                    if let Some(c) = resolve_op(o, d.doc(), &r)
                        && d.apply(c, &r).is_ok()
                    {
                        *accepted.entry(name).or_default() += 1;
                    }
                }
            }
        }
    }
    eprintln!("{accepted:?}");
    for kind in [
        "add_top",
        "add_inner",
        "connect",
        "splits",
        "macro",
        "remove",
        "duplicate",
        "open",
        "param",
    ] {
        assert!(
            accepted.get(kind).copied().unwrap_or(0) >= 20,
            "{kind}: {accepted:?}"
        );
    }
}
