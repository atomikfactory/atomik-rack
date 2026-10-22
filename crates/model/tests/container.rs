//! The container in the document model: path
//! addressing, limits and rules, inner cables, cycles on the expanded graph, path commands,
//! and the console and song refusing inner keys.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, ContainerCommand, DeviceKey, Document, ModelError, SongCommand,
    SyncOp, empty_doc,
};
use common::audio::{AUDIO_TRACK, audio_registry};
use common::*;

fn reg() -> Registry {
    registry()
}

fn err(d: &mut Document, r: &Registry, c: Command) -> ModelError {
    let before = d.doc().clone();
    let e = d.apply(c, r).expect_err("command must be rejected");
    assert_eq!(d.doc(), &before, "a rejected command changes nothing");
    e
}

#[test]
fn a_new_container_is_empty_and_has_no_console_channel() {
    let r = common::console_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CONTAINER, Some("c")), &r).unwrap();
    let c = d.doc().device(&key("c")).unwrap();
    let inner = c.inner.as_ref().expect("a container has an inside");
    assert!(inner.devices.is_empty() && inner.cables.is_empty() && inner.splits.is_empty());
    assert!(
        inner
            .macros
            .iter()
            .all(|m| m.name.is_empty() && m.targets.is_empty())
    );
    assert_eq!(inner.macros.len(), 8);
    // No automatic channel: a container may be an effect.
    assert!(d.doc().console.channels.is_empty());
    assert_eq!(keys(&d), ["c"]);
}

#[test]
fn only_containers_have_an_inside() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, SYNTH, Some("s")), &r).unwrap();
    assert!(d.doc().device(&key("s")).unwrap().inner.is_none());
}

#[test]
fn inner_devices_are_addressed_by_path_keys() {
    let r = reg();
    let mut d = container_doc(&r);
    let doc = d.doc();
    assert_eq!(doc.resolve(&key("c/synth")).unwrap().type_id, SYNTH);
    assert_eq!(doc.resolve(&key("c")).unwrap().type_id, CONTAINER);
    // `device` stays top-level only: a missed engine-facing site fails loudly.
    assert!(doc.device(&key("c/synth")).is_none());
    assert!(doc.resolve(&key("c/nope")).is_none());
    assert!(doc.resolve(&key("nope/synth")).is_none());
    assert!(doc.resolve(&key("c/~split")).is_none());
    assert!(doc.resolve(&key("c/synth/x")).is_none());
    assert_eq!(doc.scope_devices(Some(&key("c"))).unwrap().len(), 2);
    assert_eq!(doc.scope_devices(None).unwrap().len(), 1);
    assert!(doc.scope_devices(Some(&key("c/synth"))).is_none());
    assert!(doc.scope_devices(Some(&key("zz"))).is_none());
    let (parent, local) = atomik_model::split_path(&key("a/b/c"));
    assert_eq!(parent, Some(key("a/b")));
    assert_eq!(local, key("c"));
    assert_eq!(atomik_model::split_path(&key("a")), (None, key("a")));
    d.apply(set_param("c/synth", "filter.cutoff", 500.0), &r)
        .unwrap();
    assert_eq!(
        d.doc()
            .param_value(&key("c/synth"), "filter.cutoff", &r)
            .unwrap(),
        500.0
    );
}

#[test]
fn inner_add_reports_path_keys() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CONTAINER, Some("c")), &r).unwrap();
    let ops = d.apply(add_inner("c", 0, SYNTH, "synth"), &r).unwrap();
    assert_eq!(
        ops,
        [SyncOp::DeviceAdded(key("c/synth")), SyncOp::RoutingChanged]
    );
    let c = d.doc().device(&key("c")).unwrap().inner.as_ref().unwrap();
    assert_eq!(c.devices[0].key, key("synth"), "inner keys are local");
    assert_eq!(c.devices[0].name, "Poly Synth");
    // Generated keys and names are unique within the container.
    d.apply(
        Command::Container(ContainerCommand::AddInner {
            container: key("c"),
            index: 9,
            type_id: SYNTH.to_owned(),
            key: None,
            auto_split: false,
        }),
        &r,
    )
    .unwrap();
    let c = d.doc().device(&key("c")).unwrap().inner.as_ref().unwrap();
    assert_eq!(c.devices[1].key, key("poly-synth"));
    assert_eq!(c.devices[1].name, "Poly Synth 2");
}

#[test]
fn the_same_key_may_live_in_two_containers_and_at_the_top() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("synth")),
            add(1, CONTAINER, Some("a")),
            add(2, CONTAINER, Some("b")),
            add_inner("a", 0, SYNTH, "synth"),
            add_inner("b", 0, SYNTH, "synth"),
        ],
    );
    assert!(d.doc().resolve(&key("a/synth")).is_some());
    assert!(d.doc().resolve(&key("b/synth")).is_some());
    let e = err(&mut d, &r, add_inner("a", 0, SYNTH, "synth"));
    assert!(matches!(e, ModelError::BadKey(_)), "{e}");
}

#[test]
fn key_rules_reserve_boundary_slash_and_tilde() {
    let r = reg();
    let mut d = container_doc(&r);
    for bad in ["boundary", "interface", "a/b", "~split", "UPPER", "", "1x"] {
        let e = err(&mut d, &r, add_inner("c", 0, DELAY, bad));
        assert!(matches!(e, ModelError::BadKey(_)), "{bad}: {e}");
    }
    for bad in ["boundary", "a/b", "~x"] {
        let e = err(&mut d, &r, add(0, SYNTH, Some(bad)));
        assert!(matches!(e, ModelError::BadKey(_)), "{bad}: {e}");
    }
}

#[test]
fn depth_is_limited_to_three_containers() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, CONTAINER, Some("a")),
            add_inner("a", 0, CONTAINER, "b"),
            add_inner("a/b", 0, CONTAINER, "c"),
            add_inner("a/b/c", 0, DELAY, "leaf"),
        ],
    );
    assert!(d.doc().resolve(&key("a/b/c/leaf")).is_some());
    let e = err(&mut d, &r, add_inner("a/b/c", 1, CONTAINER, "d"));
    assert!(matches!(e, ModelError::ContainerTooDeep(_)), "{e}");
    // The message is meant for the user.
    assert!(e.to_string().contains("nest"), "{e}");
}

#[test]
fn a_container_holds_at_most_32_devices() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CONTAINER, Some("c")), &r).unwrap();
    for i in 0..32 {
        d.apply(add_inner("c", i, DELAY, &format!("d{i}")), &r)
            .unwrap();
    }
    let e = err(&mut d, &r, add_inner("c", 0, DELAY, "one-more"));
    assert!(matches!(e, ModelError::ContainerFull(_)), "{e}");
}

#[test]
fn a_project_holds_at_most_256_inner_devices() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    for c in 0..9 {
        d.apply(add(c, CONTAINER, Some(&format!("c{c}"))), &r)
            .unwrap();
    }
    for c in 0..8 {
        for i in 0..32 {
            d.apply(add_inner(&format!("c{c}"), i, DELAY, &format!("d{i}")), &r)
                .unwrap();
        }
    }
    let e = err(&mut d, &r, add_inner("c8", 0, DELAY, "d"));
    assert!(matches!(e, ModelError::ContainerFull(_)), "{e}");
    assert!(e.to_string().contains("256"), "{e}");
}

#[test]
fn console_devices_audio_tracks_and_the_split_are_not_allowed_inside() {
    let r = audio_registry();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CONTAINER, Some("c")), &r).unwrap();
    for t in [CHANNEL, MASTER, AUDIO_TRACK, SPLIT] {
        let e = err(&mut d, &r, add_inner("c", 0, t, "x"));
        assert!(
            matches!(e, ModelError::NotAllowedInContainer(_)),
            "{t}: {e}"
        );
    }
    // The split is internal: not even at the top.
    let e = err(&mut d, &r, add(0, SPLIT, Some("sp")));
    assert!(matches!(e, ModelError::NotAllowedInContainer(_)), "{e}");
}

#[test]
fn inner_cables_follow_the_cable_rules() {
    let r = reg();
    let mut d = container_doc(&r);
    // Direction.
    let e = err(&mut d, &r, connect("c/delay:in_l", "c/synth:notes_in"));
    assert!(matches!(e, ModelError::BadCable(_)), "{e}");
    // Kind.
    let e = err(&mut d, &r, connect("c/synth:out_r", "c/synth:notes_in"));
    assert!(matches!(e, ModelError::BadCable(_)), "{e}");
    // One cable per plain input.
    let e = err(&mut d, &r, connect("c/synth:out_r", "c/delay:in_l"));
    assert!(matches!(e, ModelError::BadCable(_)), "{e}");
    // The boundary: outputs `in_l`, `in_r`; inputs `out_l`, `out_r`; no notes socket.
    d.apply(connect("c/boundary:in_l", "c/delay:in_r"), &r)
        .expect_err("in_r of the delay is taken");
    let e = err(&mut d, &r, connect("c/synth:out_l", "c/boundary:in_l"));
    assert!(matches!(e, ModelError::BadCable(_)), "{e}");
    let e = err(
        &mut d,
        &r,
        connect("c/boundary:in_l", "c/boundary:notes_in"),
    );
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e}");
    // A free input and an unknown device or socket.
    d.apply(connect("c/boundary:in_l", "c/boundary:out_l"), &r)
        .expect_err("out_l is taken");
    let e = err(&mut d, &r, connect("c/nope:out_l", "c/boundary:out_l"));
    assert!(matches!(e, ModelError::UnknownDevice(_)), "{e}");
    let e = err(&mut d, &r, connect("c/synth:nope", "c/boundary:out_l"));
    assert!(matches!(e, ModelError::UnknownKey(_)), "{e}");
}

#[test]
fn inner_cables_never_cross_scopes() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, DELAY, Some("top")),
            add(1, CONTAINER, Some("a")),
            add(2, CONTAINER, Some("b")),
            add_inner("a", 0, DELAY, "x"),
            add_inner("b", 0, DELAY, "y"),
            add_inner("a", 1, CONTAINER, "n"),
            add_inner("a/n", 0, DELAY, "z"),
        ],
    );
    for (from, to) in [
        ("top:out_l", "a/x:in_l"),
        ("a/x:out_l", "top:in_l"),
        ("a/x:out_l", "b/y:in_l"),
        ("a/x:out_l", "a/n/z:in_l"),
        ("a/n/z:out_l", "a/x:in_l"),
        ("a/boundary:in_l", "b/y:in_l"),
        ("a/x:out_l", "b/boundary:out_l"),
        ("a/x:out_l", "a/n/boundary:out_l"),
    ] {
        let e = err(&mut d, &r, connect(from, to));
        assert!(matches!(e, ModelError::BadScope(_)), "{from} -> {to}: {e}");
    }
    // The outer sockets of a nested container are ordinary sockets of its parent's scope.
    d.apply(connect("a/x:out_l", "a/n:in_l"), &r).unwrap();
    d.apply(connect("a/n/boundary:in_l", "a/n/z:in_l"), &r)
        .unwrap();
}

#[test]
fn an_outer_cable_to_a_container_socket_follows_the_top_level_rules() {
    let r = reg();
    let mut d = container_doc(&r);
    run(&mut d, &r, [add(0, DELAY, Some("pre"))]);
    d.apply(connect("pre:out_l", "c:in_l"), &r).unwrap();
    // One cable per plain input, kind and direction as usual.
    let e = err(&mut d, &r, connect("pre:out_r", "c:in_l"));
    assert!(matches!(e, ModelError::BadCable(_)), "{e}");
    let e = err(&mut d, &r, connect("pre:out_r", "c:notes_in"));
    assert!(matches!(e, ModelError::BadCable(_)), "{e}");
    // A container fed by itself is a cycle only when its inside connects the two sockets;
    // here it passes none, so this is legal.
    d.apply(connect("c:out_l", "c:in_r"), &r).unwrap();
}

#[test]
fn a_cycle_is_judged_on_the_expanded_graph() {
    let r = reg();
    // `pre -> c -> pre`: the inside connects in_l to out_l, so a cable back is a loop.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, DELAY, Some("pre")),
            add(1, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "fx"),
            connect("c/boundary:in_l", "c/fx:in_l"),
            connect("c/fx:out_l", "c/boundary:out_l"),
            connect("pre:out_l", "c:in_l"),
        ],
    );
    let e = err(&mut d, &r, connect("c:out_l", "pre:in_l"));
    assert!(
        matches!(&e, ModelError::BadCable(m) if m.contains("cycle")),
        "{e}"
    );
    // The same shape with an inside that does not connect the sockets is fine: the container
    // is judged by what its inside does, not as a black box.
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, DELAY, Some("pre")),
            add(1, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "fx"),
            add_inner("c", 1, DELAY, "other"),
            connect("c/boundary:in_l", "c/fx:in_l"),
            connect("c/other:out_l", "c/boundary:out_l"),
            connect("pre:out_l", "c:in_l"),
            connect("c:out_l", "pre:in_l"),
        ],
    );
    // Closing the inside afterwards would make the loop: refused.
    let e = err(&mut d, &r, connect("c/fx:out_l", "c/other:in_l"));
    assert!(
        matches!(&e, ModelError::BadCable(m) if m.contains("cycle")),
        "{e}"
    );
}

#[test]
fn an_inner_loop_is_a_cycle() {
    let r = reg();
    let mut d = container_doc(&r);
    let e = err(&mut d, &r, connect("c/delay:out_l", "c/synth:notes_in"));
    assert!(matches!(e, ModelError::BadCable(_)), "{e}");
    // delay -> delay2 -> delay is a loop.
    run(
        &mut d,
        &r,
        [
            add_inner("c", 2, DELAY, "d2"),
            connect("c/delay:out_l", "c/d2:in_l"),
        ],
    );
    let e = err(&mut d, &r, connect("c/d2:out_l", "c/delay:in_l"));
    assert!(
        matches!(&e, ModelError::BadCable(m) if m.contains("cycle") || m.contains("already")),
        "{e}"
    );
    let e = err(&mut d, &r, connect("c/d2:out_l", "c/delay:time_cv_in"));
    assert!(matches!(e, ModelError::BadCable(_)), "{e}");
}

#[test]
fn two_paths_into_one_summing_input_may_not_be_the_same_cable() {
    let r = common::console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, DELAY, Some("src")),
            add(1, CONTAINER, Some("a")),
            add(2, CONTAINER, Some("b")),
            add(3, MASTER, Some("master")),
            connect("src:out_l", "a:in_l"),
            connect("src:out_l", "b:in_l"),
            connect("a/boundary:in_l", "a/boundary:out_l"),
            connect("b/boundary:in_l", "b/boundary:out_l"),
            connect("a:out_l", "master:bus_l"),
        ],
    );
    // `b` would resolve to the very same flat cable `src:out_l -> master:bus_l`.
    let e = err(&mut d, &r, connect("b:out_l", "master:bus_l"));
    assert!(matches!(e, ModelError::BadCable(_)), "{e}");
}

#[test]
fn path_commands_edit_inner_devices() {
    let r = reg();
    let mut d = container_doc(&r);
    let ops = d.apply(set_param("c/delay", "mix", 0.25), &r).unwrap();
    assert_eq!(
        ops,
        [SyncOp::ParamChanged {
            key: key("c/delay"),
            param: "mix".into(),
            value: 0.25
        }]
    );
    d.apply(
        Command::Rename {
            key: key("c/delay"),
            name: "Echo".into(),
        },
        &r,
    )
    .unwrap();
    d.apply(
        Command::SetColor {
            key: key("c/delay"),
            color: Some([1, 2, 3]),
        },
        &r,
    )
    .unwrap();
    let ops = d
        .apply(
            Command::SetFolded {
                key: key("c/delay"),
                folded: true,
            },
            &r,
        )
        .unwrap();
    assert_eq!(ops, [SyncOp::ViewChanged]);
    let delay = d.doc().resolve(&key("c/delay")).unwrap();
    assert_eq!(delay.name, "Echo");
    assert_eq!(delay.color, Some([1, 2, 3]));
    assert!(delay.folded);
    assert!(matches!(
        err(&mut d, &r, set_param("c/delay", "nope", 1.0)),
        ModelError::UnknownKey(_)
    ));
    assert!(matches!(
        err(&mut d, &r, set_param("c/zz", "mix", 1.0)),
        ModelError::UnknownDevice(_)
    ));
    // Move within the container.
    let ops = d
        .apply(
            Command::MoveDevice {
                key: key("c/delay"),
                to: 0,
            },
            &r,
        )
        .unwrap();
    assert_eq!(ops, [SyncOp::ViewChanged]);
    let inner = d.doc().device(&key("c")).unwrap().inner.as_ref().unwrap();
    assert_eq!(inner.devices[0].key, key("delay"));
    // Undo each of them, one step each.
    for _ in 0..5 {
        assert!(d.undo(&r).is_some());
    }
    assert!(d.undo(&r).is_some(), "the earlier cable steps are separate");
}

#[test]
fn removing_an_inner_device_takes_its_cables() {
    let r = reg();
    let mut d = container_doc(&r);
    let before = d.doc().clone();
    let ops = d
        .apply(
            Command::RemoveDevice {
                key: key("c/delay"),
            },
            &r,
        )
        .unwrap();
    assert_eq!(
        ops,
        [
            SyncOp::RoutingChanged,
            SyncOp::DeviceRemoved(key("c/delay"))
        ]
    );
    let inner = d.doc().device(&key("c")).unwrap().inner.as_ref().unwrap();
    assert_eq!(inner.devices.len(), 1);
    assert!(inner.cables.is_empty());
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&r).unwrap();
    assert_eq!(
        d.doc()
            .device(&key("c"))
            .unwrap()
            .inner
            .as_ref()
            .unwrap()
            .devices
            .len(),
        1
    );
}

#[test]
fn removing_and_restoring_a_container_brings_back_everything() {
    let r = common::console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, DELAY, Some("pre")),
            add(1, CONTAINER, Some("c")),
            add_inner("c", 0, SYNTH, "synth"),
            add_inner("c", 1, DELAY, "delay"),
            connect("c/synth:out_l", "c/delay:in_l"),
            connect("c/delay:out_l", "c/boundary:out_l"),
            connect("pre:out_l", "c:in_l"),
            Command::Console(ConsoleCommand::AddChannel { source: key("c") }),
            Command::Song(SongCommand::AddTrack { device: key("c") }),
            set_splits("c", vec![split("synth", 0, 127, 0)]),
            set_macro("c", 0, target("delay", "mix", 0.0, 1.0)),
        ],
    );
    let before = d.doc().clone();
    assert!(before.song.track(&key("c")).is_some());
    assert_eq!(before.console.channels.len(), 1);
    let ops = d
        .apply(Command::RemoveDevice { key: key("c") }, &r)
        .unwrap();
    assert!(ops.contains(&SyncOp::DeviceRemoved(key("c"))));
    let after = d.doc();
    assert!(after.device(&key("c")).is_none());
    assert!(after.song.track(&key("c")).is_none());
    assert!(after.console.channels.is_empty());
    assert!(after.cables.iter().all(|c| c.to.device.0 != "c"));
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
    d.redo(&r).unwrap();
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn console_and_song_refuse_inner_keys() {
    let r = common::console_registry();
    let mut d = container_doc(&r);
    let e = err(
        &mut d,
        &r,
        Command::Console(ConsoleCommand::AddChannel {
            source: key("c/synth"),
        }),
    );
    assert!(matches!(e, ModelError::UnknownDevice(_)), "{e}");
    let e = err(
        &mut d,
        &r,
        Command::Console(ConsoleCommand::AddChannelFor {
            source: key("c/synth"),
            out_l: "out_l".into(),
            out_r: "out_r".into(),
            name: None,
        }),
    );
    assert!(matches!(e, ModelError::UnknownDevice(_)), "{e}");
    let e = err(
        &mut d,
        &r,
        Command::Song(SongCommand::AddTrack {
            device: key("c/synth"),
        }),
    );
    assert!(
        matches!(e, ModelError::UnknownDevice(_) | ModelError::BadSong(_)),
        "{e}"
    );
    // A container itself may have a track and a channel.
    d.apply(
        Command::Song(SongCommand::AddTrack { device: key("c") }),
        &r,
    )
    .unwrap();
    d.apply(
        Command::Console(ConsoleCommand::AddChannel { source: key("c") }),
        &r,
    )
    .unwrap();
}

#[test]
fn a_container_can_be_an_insert_effect_and_a_return() {
    let r = common::console_registry();
    let mut d = Document::new(empty_doc());
    run(
        &mut d,
        &r,
        [
            add(0, SYNTH, Some("s")),
            add(1, CONTAINER, Some("c")),
            add_inner("c", 0, DELAY, "fx"),
            connect("c/boundary:in_l", "c/fx:in_l"),
            connect("c/boundary:in_r", "c/fx:in_r"),
            connect("c/fx:out_l", "c/boundary:out_l"),
            connect("c/fx:out_r", "c/boundary:out_r"),
        ],
    );
    let channel = d.doc().console.channels[0].channel.clone();
    d.apply(
        Command::Console(ConsoleCommand::SetInsert {
            channel: channel.clone(),
            slot: 0,
            effect: Some(atomik_model::InsertEffect::Existing(key("c"))),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(
        atomik_model::insert_chain(d.doc(), &r, &channel),
        [key("c")]
    );
    // And as a new insert.
    d.apply(
        Command::Console(ConsoleCommand::SetInsert {
            channel: channel.clone(),
            slot: 1,
            effect: Some(atomik_model::InsertEffect::New {
                type_id: CONTAINER.to_owned(),
            }),
        }),
        &r,
    )
    .unwrap();
    let chain = atomik_model::insert_chain(d.doc(), &r, &channel);
    assert_eq!(chain.len(), 2);
    let second = d.doc().device(&chain[1]).unwrap();
    assert!(
        second.inner.is_some(),
        "a new container insert has an inside"
    );
}

#[test]
fn undo_and_redo_of_inner_structure_are_single_steps() {
    let r = reg();
    let mut d = Document::new(empty_doc());
    d.apply(add(0, CONTAINER, Some("c")), &r).unwrap();
    let s0 = d.doc().clone();
    d.apply(add_inner("c", 0, SYNTH, "synth"), &r).unwrap();
    let s1 = d.doc().clone();
    d.apply(add_inner("c", 1, DELAY, "delay"), &r).unwrap();
    let s2 = d.doc().clone();
    d.apply(connect("c/synth:out_l", "c/delay:in_l"), &r)
        .unwrap();
    let s3 = d.doc().clone();
    d.apply(
        Command::Disconnect {
            to: sock("c/delay:in_l"),
        },
        &r,
    )
    .unwrap();
    assert_eq!(
        d.doc()
            .device(&key("c"))
            .unwrap()
            .inner
            .as_ref()
            .unwrap()
            .cables
            .len(),
        0
    );
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &s3);
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &s2);
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &s1);
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &s0);
    for s in [&s1, &s2, &s3] {
        d.redo(&r).unwrap();
        assert_eq!(d.doc(), s);
    }
}

#[test]
fn batch_accepts_path_set_param_and_connect() {
    let r = reg();
    let mut d = container_doc(&r);
    let before = d.doc().clone();
    d.apply(
        Command::Batch(vec![
            set_param("c/delay", "mix", 0.9),
            Command::Disconnect {
                to: sock("c/boundary:out_l"),
            },
            connect("c/synth:out_l", "c/boundary:out_l"),
        ]),
        &r,
    )
    .unwrap();
    d.undo(&r).unwrap();
    assert_eq!(d.doc(), &before, "one undo step");
    // A bad member rejects the whole batch.
    let e = d
        .apply(
            Command::Batch(vec![
                set_param("c/delay", "mix", 0.9),
                connect("c/synth:out_l", "top:in_l"),
            ]),
            &r,
        )
        .unwrap_err();
    assert!(
        matches!(e, ModelError::UnknownDevice(_) | ModelError::BadScope(_)),
        "{e}"
    );
    assert_eq!(d.doc(), &before);
}

#[test]
fn a_key_type_is_hashable_path() {
    // Path keys are plain `DeviceKey`s.
    let k = DeviceKey("a/b".into());
    assert_eq!(k.as_str(), "a/b");
}

#[test]
fn returns_inserts_and_song_lanes_refuse_inner_keys_but_take_a_container() {
    let r = common::console_registry();
    let mut d = container_doc(&r);
    // Return and insert effects, and tracks, are top-level only.
    d.apply(add(0, SYNTH, Some("s")), &r).unwrap();
    let channel = d.doc().console.channels[0].channel.clone();
    let e = err(
        &mut d,
        &r,
        Command::Console(ConsoleCommand::SetInsert {
            channel,
            slot: 0,
            effect: Some(atomik_model::InsertEffect::Existing(key("c/delay"))),
        }),
    );
    assert!(
        matches!(e, ModelError::UnknownDevice(_) | ModelError::BadConsole(_)),
        "{e}"
    );
    let e = err(
        &mut d,
        &r,
        Command::Console(ConsoleCommand::SetReturn {
            slot: 0,
            effect: Some(key("c/delay")),
        }),
    );
    assert!(
        matches!(e, ModelError::UnknownDevice(_) | ModelError::BadConsole(_)),
        "{e}"
    );
    let e = err(
        &mut d,
        &r,
        Command::Song(SongCommand::AddAutomationLane {
            device: key("c/synth"),
            param: "filter.cutoff".into(),
        }),
    );
    assert!(
        matches!(e, ModelError::UnknownDevice(_) | ModelError::BadSong(_)),
        "{e}"
    );
    // A container's own macro params can be automated on a lane.
    d.apply(
        Command::Song(SongCommand::AddAutomationLane {
            device: key("c"),
            param: "macro1".into(),
        }),
        &r,
    )
    .unwrap();
    let e = err(
        &mut d,
        &r,
        Command::Song(SongCommand::AddAutomationLane {
            device: key("c"),
            param: "nope".into(),
        }),
    );
    assert!(
        matches!(e, ModelError::BadSong(_) | ModelError::UnknownKey(_)),
        "{e}"
    );
}

#[test]
fn a_flat_cable_maps_back_to_the_view_cable_of_its_scope() {
    use atomik_model::{Cable, SocketRef, scoped_cable};
    let cable = |a: &str, b: &str| Cable {
        from: SocketRef {
            device: DeviceKey(a.into()),
            socket: "o".into(),
        },
        to: SocketRef {
            device: DeviceKey(b.into()),
            socket: "i".into(),
        },
    };
    let scope = |s: &str| Some(DeviceKey(s.into()));
    assert_eq!(
        scoped_cable(&cable("a", "b")),
        Some((None, cable("a", "b"))),
        "top level"
    );
    assert_eq!(
        scoped_cable(&cable("layer/x", "layer/y")),
        Some((scope("layer"), cable("x", "y")))
    );
    assert_eq!(
        scoped_cable(&cable("a/b/x", "a/b/y")),
        Some((scope("a/b"), cable("x", "y"))),
        "nested"
    );
    assert_eq!(scoped_cable(&cable("x", "layer/y")), None, "across scopes");
    assert_eq!(
        scoped_cable(&cable("a/x", "b/y")),
        None,
        "across containers"
    );
    assert_eq!(scoped_cable(&cable("layer/~split", "layer/y")), None);
}
