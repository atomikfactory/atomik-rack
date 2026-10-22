//! Output buses (model half): the bus commands and their undo, cycle and range refusals, the
//! console-link classification of bus cables, `output_dest`, the solo rule, inserts on a bus, and
//! what removing, copying and duplicating the strips around a bus does.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::console::{
    BUS_TYPE, LinkKind, MAX_BUS_NAME_CHARS, OutputDest, link_kind, new_bus_key, output_dest,
};
use atomik_model::{
    Command, ConsoleBus, ConsoleCommand, Document, FX_SLOTS, InsertEffect, RackDoc,
    WorkflowCommand, insert_chain, new_project_doc,
};
use common::console::BUS as BUS_DESC;
use common::console_ops::assert_console_invariants;
use common::workflow::wf;
use common::*;

/// The starter with three synths and their channels `channel`, `channel-2`, `channel-3`, and 8
/// interface outputs.
fn project() -> (Document, Registry) {
    let r = console_registry();
    let mut doc = new_project_doc();
    doc.output_channels = 8;
    let mut d = Document::new(doc);
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, SYNTH, Some("a")),
            add(usize::MAX, SYNTH, Some("b")),
            add(usize::MAX, SYNTH, Some("c")),
        ],
    );
    (d, r)
}

fn con(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn create(name: &str, dest: OutputDest) -> Command {
    con(ConsoleCommand::CreateBus {
        name: name.into(),
        dest,
    })
}

fn set_output(strip: &str, dest: OutputDest) -> Command {
    con(ConsoleCommand::SetOutput {
        strip: key(strip),
        dest,
    })
}

fn bus(name: &str) -> OutputDest {
    OutputDest::Bus(key(name))
}

fn dest_of(d: &Document, strip: &str) -> OutputDest {
    output_dest(d.doc(), &key(strip))
}

/// Applies `c` and expects a refusal; the document must be untouched.
fn refused(d: &mut Document, r: &Registry, c: Command) -> String {
    let before = d.doc().clone();
    let can_undo = d.can_undo();
    let e = d.apply(c.clone(), r).expect_err(&format!("{c:?} accepted"));
    assert_eq!(d.doc(), &before, "{c:?} changed the document");
    assert_eq!(d.can_undo(), can_undo, "{c:?} recorded a step");
    e.to_string()
}

/// Applies `c` and checks it is exactly one undo step: undo restores the document, redo
/// brings the result back.
fn one_step(d: &mut Document, r: &Registry, c: Command) {
    let before = d.doc().clone();
    d.apply(c.clone(), r)
        .unwrap_or_else(|e| panic!("{c:?} rejected: {e}"));
    let after = d.doc().clone();
    assert_ne!(after, before, "{c:?} changed nothing");
    d.undo(r).expect("undo");
    assert_eq!(d.doc(), &before, "{c:?} undone in one step");
    d.redo(r).expect("redo");
    assert_eq!(d.doc(), &after, "{c:?} redone");
}

fn cables_from(doc: &RackDoc, device: &str, socket: &str) -> Vec<String> {
    doc.cables
        .iter()
        .filter(|c| c.from.device.as_str() == device && c.from.socket == socket)
        .map(|c| c.to.to_string())
        .collect()
}

fn set_param(k: &str, p: &str, v: f32) -> Command {
    Command::SetParam {
        key: key(k),
        param: p.into(),
        value: v,
        merge: None,
    }
}

fn silence(d: &Document, r: &Registry) -> Vec<(String, bool)> {
    d.doc()
        .solo_silence(r)
        .into_iter()
        .map(|(k, s)| (k.0, s))
        .collect()
}

fn silent(d: &Document, r: &Registry) -> Vec<String> {
    silence(d, r)
        .into_iter()
        .filter(|(_, s)| *s)
        .map(|(k, _)| k)
        .collect()
}

// ---- CreateBus ----------------------------------------------------------------------------

#[test]
fn a_new_bus_is_a_strip_with_sends_to_the_master_and_its_output_on_the_master() {
    let (mut d, r) = project();
    let predicted = new_bus_key(d.doc());
    assert_eq!(predicted, key("bus"));
    d.apply(create("Drums", OutputDest::Master), &r).unwrap();
    let doc = d.doc();
    assert_eq!(
        doc.console.buses,
        [ConsoleBus {
            bus: predicted.clone(),
            name: "Drums".into()
        }]
    );
    let device = doc.device(&predicted).unwrap();
    assert_eq!(device.type_id, BUS_TYPE);
    assert_eq!(device.name, "Drums");
    assert_eq!(
        doc.devices.last().unwrap().key,
        predicted,
        "a new bus goes to the end of the rack"
    );
    assert_eq!(dest_of(&d, "bus"), OutputDest::Master);
    assert_eq!(cables_from(doc, "bus", "out_l"), ["master:bus_l"]);
    assert_eq!(cables_from(doc, "bus", "out_r"), ["master:bus_r"]);
    for k in 1..=FX_SLOTS {
        for side in ["l", "r"] {
            assert_eq!(
                cables_from(doc, "bus", &format!("send{k}_{side}")),
                [format!("master:send_bus{k}_{side}")]
            );
        }
    }
    assert_console_invariants(doc);
}

#[test]
fn creating_a_bus_is_one_undo_step() {
    let (mut d, r) = project();
    one_step(&mut d, &r, create("Drums", OutputDest::Master));
    one_step(&mut d, &r, create("Vox", bus("bus")));
    assert_eq!(new_bus_key(d.doc()), key("bus-3"));
}

#[test]
fn the_second_bus_goes_after_the_first_and_both_keep_console_order() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("One", OutputDest::Master),
            add(usize::MAX, SYNTH, Some("late")),
            create("Two", OutputDest::Master),
        ],
    );
    let names: Vec<&str> = d.doc().console.buses.iter().map(|b| &*b.name).collect();
    assert_eq!(names, ["One", "Two"]);
    let at = |k: &str| d.doc().devices.iter().position(|x| x.key.as_str() == k);
    assert!(
        at("bus-2") == at("bus").map(|i| i + 1),
        "after the first bus"
    );
}

#[test]
fn a_bus_in_a_document_without_a_master_creates_the_master() {
    let r = console_registry();
    let mut d = Document::new(atomik_model::empty_doc());
    assert!(d.doc().console.master.is_none());
    one_step(&mut d, &r, create("Bus A", OutputDest::Master));
    let doc = d.doc();
    let master = doc.console.master.clone().expect("master created");
    assert_eq!(cables_from(doc, "bus", "out_l"), ["master:bus_l"]);
    assert_eq!(
        cables_from(doc, master.as_str(), "main_l"),
        ["interface:out_1"]
    );
    assert_console_invariants(doc);
}

#[test]
fn bus_names_are_trimmed_non_empty_short_and_unique_ignoring_case() {
    let (mut d, r) = project();
    d.apply(create("  Drums  ", OutputDest::Master), &r)
        .unwrap();
    assert_eq!(d.doc().console.buses[0].name, "Drums");
    assert_eq!(d.doc().device(&key("bus")).unwrap().name, "Drums");
    for bad in ["", "   ", "drums", "DRUMS "] {
        let e = refused(&mut d, &r, create(bad, OutputDest::Master));
        assert!(!e.is_empty(), "{bad:?}");
    }
    let long = "x".repeat(MAX_BUS_NAME_CHARS + 1);
    let e = refused(&mut d, &r, create(&long, OutputDest::Master));
    assert!(e.contains("at most"), "{e}");
    d.apply(
        create(&"x".repeat(MAX_BUS_NAME_CHARS), OutputDest::Master),
        &r,
    )
    .unwrap();
    assert_eq!(d.doc().console.default_bus_name(), "Bus 1");
    assert!(d.doc().console.bus_by_name(" DRUMS").is_some());
}

#[test]
fn a_bus_can_be_created_onto_another_bus_or_an_audio_io_pair() {
    let (mut d, r) = project();
    run(&mut d, &r, [create("Sub", OutputDest::Master)]);
    one_step(&mut d, &r, create("Top", bus("bus")));
    assert_eq!(dest_of(&d, "bus-2"), bus("bus"));
    one_step(&mut d, &r, create("Phones", OutputDest::AudioIo(2)));
    assert_eq!(dest_of(&d, "bus-3"), OutputDest::AudioIo(2));
    assert_eq!(cables_from(d.doc(), "bus-3", "out_l"), ["interface:out_3"]);
    assert_eq!(cables_from(d.doc(), "bus-3", "out_r"), ["interface:out_4"]);
    assert_console_invariants(d.doc());
}

#[test]
fn a_bad_destination_refuses_the_whole_creation() {
    let (mut d, r) = project();
    for dest in [
        bus("nope"),
        OutputDest::AudioIo(0),
        OutputDest::AudioIo(5),
        OutputDest::Mixed,
        OutputDest::None,
        OutputDest::Direct(Vec::new()),
    ] {
        refused(&mut d, &r, create("X", dest));
    }
    assert!(d.doc().console.buses.is_empty());
}

// ---- SetOutput ----------------------------------------------------------------------------

#[test]
fn a_channel_goes_to_a_bus_and_back_to_the_master() {
    let (mut d, r) = project();
    d.apply(create("Drums", OutputDest::Master), &r).unwrap();
    one_step(&mut d, &r, set_output("channel", bus("bus")));
    assert_eq!(dest_of(&d, "channel"), bus("bus"));
    assert_eq!(cables_from(d.doc(), "channel", "out_l"), ["bus:bus_l"]);
    assert_eq!(cables_from(d.doc(), "channel", "out_r"), ["bus:bus_r"]);
    // The sends stay on the master.
    assert_eq!(
        cables_from(d.doc(), "channel", "send3_r"),
        ["master:send_bus3_r"]
    );
    assert_eq!(dest_of(&d, "channel-2"), OutputDest::Master);
    one_step(&mut d, &r, set_output("channel", OutputDest::Master));
    assert_eq!(dest_of(&d, "channel"), OutputDest::Master);
    assert_console_invariants(d.doc());
}

#[test]
fn a_strip_goes_to_a_free_audio_io_pair_without_a_warning_kind() {
    let (mut d, r) = project();
    one_step(&mut d, &r, set_output("channel", OutputDest::AudioIo(3)));
    assert_eq!(dest_of(&d, "channel"), OutputDest::AudioIo(3));
    assert_eq!(
        cables_from(d.doc(), "channel", "out_l"),
        ["interface:out_5"]
    );
    assert_eq!(
        cables_from(d.doc(), "channel", "out_r"),
        ["interface:out_6"]
    );
    // Going there again changes nothing.
    let before = d.doc().clone();
    d.apply(set_output("channel", OutputDest::AudioIo(3)), &r)
        .unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn an_audio_io_pair_beyond_the_outputs_or_already_fed_is_refused_clearly() {
    let (mut d, r) = project();
    d.apply(create("Drums", OutputDest::Master), &r).unwrap();
    let e = refused(&mut d, &r, set_output("channel", OutputDest::AudioIo(5)));
    assert!(e.contains("output channels"), "{e}");
    let e = refused(&mut d, &r, set_output("channel", OutputDest::AudioIo(0)));
    assert!(e.contains("Audio I/O"), "{e}");
    // The master plays on outputs 1+2.
    let e = refused(&mut d, &r, set_output("channel", OutputDest::AudioIo(1)));
    assert!(e.contains("already fed"), "{e}");
    d.apply(set_output("channel", OutputDest::AudioIo(2)), &r)
        .unwrap();
    let e = refused(&mut d, &r, set_output("channel-2", OutputDest::AudioIo(2)));
    assert!(e.contains("already fed"), "{e}");
}

#[test]
fn buses_nest_and_a_loop_is_refused() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("A", OutputDest::Master),
            create("B", OutputDest::Master),
            create("C", OutputDest::Master),
        ],
    );
    one_step(&mut d, &r, set_output("bus", bus("bus-2")));
    assert_eq!(dest_of(&d, "bus"), bus("bus-2"));
    assert_eq!(
        link_kind_of(&d, "bus:out_l", "bus-2:bus_l"),
        Some(LinkKind::BusToBus)
    );
    d.apply(set_output("bus-2", bus("bus-3")), &r).unwrap();
    // A -> B -> C: C -> A, C -> B, B -> A, and a bus onto itself make loops.
    for (strip, to) in [
        ("bus-3", "bus"),
        ("bus-3", "bus-2"),
        ("bus-2", "bus"),
        ("bus", "bus"),
    ] {
        let e = refused(&mut d, &r, set_output(strip, bus(to)));
        assert!(
            e.contains("would make a loop") || e.contains("itself"),
            "{strip}->{to}: {e}"
        );
    }
    // The other way round is fine.
    d.apply(set_output("bus", bus("bus-3")), &r).unwrap();
    assert_console_invariants(d.doc());
}

#[test]
fn set_output_names_what_it_refuses() {
    let (mut d, r) = project();
    d.apply(create("A", OutputDest::Master), &r).unwrap();
    for (strip, dest) in [
        ("master", OutputDest::Master),
        ("a", OutputDest::Master),
        ("nope", OutputDest::Master),
        ("channel", bus("nope")),
        ("channel", bus("channel-2")),
        ("channel", bus("master")),
        ("channel", OutputDest::Direct(Vec::new())),
        ("channel", OutputDest::Mixed),
        ("channel", OutputDest::None),
    ] {
        refused(&mut d, &r, set_output(strip, dest));
    }
}

#[test]
fn set_output_replaces_a_hand_cable_on_the_outputs() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("A", OutputDest::Master),
            add(usize::MAX, MIXER, Some("mixer")),
            Command::Disconnect {
                to: sock("master:bus_l"),
            },
            connect("channel:out_l", "mixer:ch1_in_l"),
        ],
    );
    assert!(matches!(dest_of(&d, "channel"), OutputDest::Mixed));
    one_step(&mut d, &r, set_output("channel", bus("bus")));
    assert_eq!(dest_of(&d, "channel"), bus("bus"));
    assert!(cables_from(d.doc(), "channel", "out_l") == ["bus:bus_l"]);
}

// ---- RouteToNewBus ------------------------------------------------------------------------

#[test]
fn route_to_new_bus_creates_the_bus_and_routes_the_strips_in_one_step() {
    let (mut d, r) = project();
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::RouteToNewBus {
            strips: vec![key("channel"), key("channel-3")],
            name: "Group".into(),
        }),
    );
    assert_eq!(d.doc().console.buses[0].name, "Group");
    assert_eq!(dest_of(&d, "channel"), bus("bus"));
    assert_eq!(dest_of(&d, "channel-3"), bus("bus"));
    assert_eq!(dest_of(&d, "channel-2"), OutputDest::Master);
    assert_eq!(dest_of(&d, "bus"), OutputDest::Master);
    assert_console_invariants(d.doc());
}

#[test]
fn route_to_new_bus_takes_buses_too_and_refuses_bad_input() {
    let (mut d, r) = project();
    d.apply(create("Old", OutputDest::Master), &r).unwrap();
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::RouteToNewBus {
            strips: vec![key("bus")],
            name: "Parent".into(),
        }),
    );
    assert_eq!(dest_of(&d, "bus"), bus("bus-2"));
    let route = |strips: &[&str], name: &str| {
        con(ConsoleCommand::RouteToNewBus {
            strips: strips.iter().map(|s| key(s)).collect(),
            name: name.into(),
        })
    };
    refused(&mut d, &r, route(&[], "X"));
    refused(&mut d, &r, route(&["channel", "channel"], "X"));
    refused(&mut d, &r, route(&["a"], "X"));
    refused(&mut d, &r, route(&["master"], "X"));
    refused(&mut d, &r, route(&["channel"], " "));
    refused(&mut d, &r, route(&["channel"], "parent"));
}

// ---- RenameBus / ReorderBus ---------------------------------------------------------------

#[test]
fn renaming_a_bus_renames_its_device_and_is_one_step() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("A", OutputDest::Master),
            create("B", OutputDest::Master),
        ],
    );
    let rename = |bus: &str, name: &str| {
        con(ConsoleCommand::RenameBus {
            bus: key(bus),
            name: name.into(),
        })
    };
    one_step(&mut d, &r, rename("bus", "  Drums "));
    assert_eq!(d.doc().console.buses[0].name, "Drums");
    assert_eq!(d.doc().device(&key("bus")).unwrap().name, "Drums");
    // The generic rename of the device is the same command.
    one_step(
        &mut d,
        &r,
        Command::Rename {
            key: key("bus-2"),
            name: "Vox".into(),
        },
    );
    assert_eq!(d.doc().console.buses[1].name, "Vox");
    refused(&mut d, &r, rename("bus", "vox"));
    refused(&mut d, &r, rename("bus", ""));
    refused(&mut d, &r, rename("channel", "Drums 2"));
    refused(&mut d, &r, rename("nope", "Drums 2"));
    refused(
        &mut d,
        &r,
        Command::Rename {
            key: key("bus"),
            name: "VOX".into(),
        },
    );
    // Its own name again, in another case, is allowed.
    d.apply(rename("bus", "DRUMS"), &r).unwrap();
}

#[test]
fn reordering_buses_changes_only_the_console_order() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("A", OutputDest::Master),
            create("B", OutputDest::Master),
            create("C", OutputDest::Master),
        ],
    );
    let cables = d.doc().cables.clone();
    let reorder = |bus: &str, to: usize| con(ConsoleCommand::ReorderBus { bus: key(bus), to });
    one_step(&mut d, &r, reorder("bus", 2));
    let order = |d: &Document| -> Vec<String> {
        d.doc()
            .console
            .buses
            .iter()
            .map(|b| b.name.clone())
            .collect()
    };
    assert_eq!(order(&d), ["B", "C", "A"]);
    assert_eq!(d.doc().cables, cables);
    one_step(&mut d, &r, reorder("bus-3", 0));
    assert_eq!(order(&d), ["C", "B", "A"]);
    d.apply(reorder("bus-3", 99), &r).unwrap();
    assert_eq!(order(&d), ["B", "A", "C"]);
    refused(&mut d, &r, reorder("channel", 0));
}

// ---- DeleteBus ----------------------------------------------------------------------------

fn delete(bus: &str) -> Command {
    con(ConsoleCommand::DeleteBus { bus: key(bus) })
}

#[test]
fn deleting_a_bus_moves_its_feeders_to_the_master() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [con(ConsoleCommand::RouteToNewBus {
            strips: vec![key("channel"), key("channel-2")],
            name: "G".into(),
        })],
    );
    one_step(&mut d, &r, delete("bus"));
    let doc = d.doc();
    assert!(doc.console.buses.is_empty());
    assert!(doc.device(&key("bus")).is_none());
    assert!(
        doc.cables
            .iter()
            .all(|c| c.from.device != key("bus") && c.to.device != key("bus"))
    );
    assert_eq!(dest_of(&d, "channel"), OutputDest::Master);
    assert_eq!(dest_of(&d, "channel-2"), OutputDest::Master);
    assert_console_invariants(doc);
}

#[test]
fn deleting_a_bus_moves_its_feeders_to_the_bus_it_played_into() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("Top", OutputDest::Master),
            create("Sub", bus("bus")),
            set_output("channel", bus("bus-2")),
            create("Side", bus("bus-2")),
        ],
    );
    one_step(&mut d, &r, delete("bus-2"));
    assert_eq!(dest_of(&d, "channel"), bus("bus"));
    assert_eq!(dest_of(&d, "bus-3"), bus("bus"));
    assert_console_invariants(d.doc());
}

#[test]
fn a_bus_on_an_audio_io_pair_hands_it_to_a_single_feeder_else_the_master_does() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("Phones", OutputDest::AudioIo(2)),
            set_output("channel", bus("bus")),
        ],
    );
    one_step(&mut d, &r, delete("bus"));
    assert_eq!(dest_of(&d, "channel"), OutputDest::AudioIo(2));

    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("Phones", OutputDest::AudioIo(2)),
            set_output("channel", bus("bus")),
            set_output("channel-2", bus("bus")),
        ],
    );
    one_step(&mut d, &r, delete("bus"));
    assert_eq!(dest_of(&d, "channel"), OutputDest::Master);
    assert_eq!(dest_of(&d, "channel-2"), OutputDest::Master);
    assert!(cables_from(d.doc(), "interface", "out_3").is_empty());
}

#[test]
fn removing_the_bus_device_generically_is_delete_bus() {
    let (mut a, r) = project();
    run(
        &mut a,
        &r,
        [con(ConsoleCommand::RouteToNewBus {
            strips: vec![key("channel"), key("channel-3")],
            name: "G".into(),
        })],
    );
    let mut b = a.clone();
    a.apply(delete("bus"), &r).unwrap();
    b.apply(Command::RemoveDevice { key: key("bus") }, &r)
        .unwrap();
    assert_eq!(a.doc(), b.doc());
    b.undo(&r).unwrap();
    assert_eq!(dest_of(&b, "channel"), bus("bus"));
    // Through the workflow removal too (with the feeder removed in the same selection).
    b.apply(
        wf(WorkflowCommand::RemoveDevices {
            keys: vec![key("bus"), key("channel")],
        }),
        &r,
    )
    .unwrap();
    assert!(b.doc().console.buses.is_empty());
    assert_eq!(dest_of(&b, "channel-3"), OutputDest::Master);
    assert_console_invariants(b.doc());
}

#[test]
fn deleting_what_is_no_bus_is_refused() {
    let (mut d, r) = project();
    refused(&mut d, &r, delete("channel"));
    refused(&mut d, &r, delete("nope"));
}

#[test]
fn a_bus_insert_effect_stays_in_the_rack_when_the_bus_goes() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("A", OutputDest::Master),
            add(usize::MAX, DELAY, Some("fx")),
            con(ConsoleCommand::SetInsert {
                channel: key("bus"),
                slot: 0,
                effect: Some(InsertEffect::Existing(key("fx"))),
            }),
        ],
    );
    assert_eq!(insert_chain(d.doc(), &r, &key("bus")), [key("fx")]);
    d.apply(delete("bus"), &r).unwrap();
    assert!(d.doc().device(&key("fx")).is_some());
    assert!(
        d.doc()
            .cables
            .iter()
            .all(|c| c.from.device != key("fx") && c.to.device != key("fx"))
    );
}

// ---- Channels around a bus ----------------------------------------------------------------

#[test]
fn removing_a_channel_that_feeds_a_bus_leaves_the_bus_alone() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            con(ConsoleCommand::RouteToNewBus {
                strips: vec![key("channel"), key("channel-2")],
                name: "G".into(),
            }),
            con(ConsoleCommand::RemoveChannel {
                channel: key("channel"),
            }),
        ],
    );
    assert_eq!(d.doc().console.buses.len(), 1);
    assert_eq!(dest_of(&d, "channel-2"), bus("bus"));
    // Removing the instrument takes its channel with it, the bus stays.
    d.apply(Command::RemoveDevice { key: key("b") }, &r)
        .unwrap();
    assert_eq!(d.doc().console.buses.len(), 1);
    assert_console_invariants(d.doc());
}

#[test]
fn a_duplicated_instrument_gets_a_master_channel_and_the_bus_keeps_its_feeders() {
    let (mut d, r) = project();
    d.apply(
        con(ConsoleCommand::RouteToNewBus {
            strips: vec![key("channel")],
            name: "G".into(),
        }),
        &r,
    )
    .unwrap();
    let before = d.doc().console.channels.len();
    d.apply(
        wf(WorkflowCommand::DuplicateDevices {
            keys: vec![key("a")],
        }),
        &r,
    )
    .unwrap();
    let doc = d.doc();
    assert_eq!(doc.console.channels.len(), before + 1);
    let new = doc.console.channels.last().unwrap().channel.clone();
    assert_eq!(output_dest(doc, &new), OutputDest::Master);
    assert_eq!(dest_of(&d, "channel"), bus("bus"));
    assert_console_invariants(doc);
}

#[test]
fn a_bus_is_not_duplicated() {
    let (mut d, r) = project();
    d.apply(create("G", OutputDest::Master), &r).unwrap();
    let c = wf(WorkflowCommand::DuplicateDevices {
        keys: vec![key("bus")],
    });
    let before = d.doc().clone();
    d.apply(c, &r).expect_err("refused");
    assert_eq!(d.doc(), &before);
}

// ---- link_kind / output_dest --------------------------------------------------------------

fn link_kind_of(d: &Document, from: &str, to: &str) -> Option<LinkKind> {
    link_kind(d.doc(), &cable(from, to))
}

#[test]
fn bus_cables_are_console_links_and_the_rest_are_not() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("A", OutputDest::Master),
            create("B", bus("bus")),
            set_output("channel", bus("bus")),
        ],
    );
    let kind = |f: &str, t: &str| link_kind_of(&d, f, t);
    assert_eq!(
        kind("channel:out_l", "bus:bus_l"),
        Some(LinkKind::ChannelToBus)
    );
    assert_eq!(
        kind("channel:out_r", "bus:bus_r"),
        Some(LinkKind::ChannelToBus)
    );
    assert_eq!(
        kind("channel-2:out_l", "master:bus_l"),
        Some(LinkKind::ChannelOut)
    );
    assert_eq!(kind("bus:out_l", "master:bus_l"), Some(LinkKind::BusOut));
    assert_eq!(kind("bus-2:out_r", "bus:bus_r"), Some(LinkKind::BusToBus));
    for k in 1..=FX_SLOTS {
        for side in ["l", "r"] {
            assert_eq!(
                kind(
                    &format!("bus:send{k}_{side}"),
                    &format!("master:send_bus{k}_{side}")
                ),
                Some(LinkKind::BusSend(k as u8))
            );
        }
    }
    // Crossed sides, a wrong send number, a send into a bus, a bus onto itself, the master's
    // output, an audio-I/O cable and a plain device are ordinary.
    assert_eq!(kind("channel:out_l", "bus:bus_r"), None);
    assert_eq!(kind("bus:send2_l", "master:send_bus3_l"), None);
    assert_eq!(kind("bus:send1_l", "bus-2:bus_l"), None);
    assert_eq!(kind("bus:out_l", "bus:bus_l"), None);
    assert_eq!(kind("master:main_l", "interface:out_1"), None);
    assert_eq!(kind("bus:out_l", "interface:out_3"), None);
    assert_eq!(kind("a:out_l", "channel:in_l"), None);
    // Every cable of the project is classified once: 3 channels and 2 buses, 18 each.
    let doc = d.doc();
    let links = doc
        .cables
        .iter()
        .filter(|c| link_kind(doc, c).is_some())
        .count();
    assert_eq!(links, 5 * 18);
}

#[test]
fn output_dest_reads_master_bus_audio_io_direct_mixed_and_none() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("A", OutputDest::Master),
            create("B", OutputDest::AudioIo(2)),
            add(usize::MAX, MIXER, Some("mixer")),
        ],
    );
    assert_eq!(dest_of(&d, "channel"), OutputDest::Master);
    assert_eq!(dest_of(&d, "bus"), OutputDest::Master);
    assert_eq!(dest_of(&d, "bus-2"), OutputDest::AudioIo(2));
    assert_eq!(dest_of(&d, "a"), OutputDest::None, "not a strip");
    assert_eq!(dest_of(&d, "nope"), OutputDest::None);
    assert_eq!(dest_of(&d, "master"), OutputDest::None);

    run(&mut d, &r, [set_output("channel", bus("bus"))]);
    assert_eq!(dest_of(&d, "channel"), bus("bus"));
    // Left to a bus, right to the master.
    run(
        &mut d,
        &r,
        [
            Command::Disconnect {
                to: sock("bus:bus_r"),
            },
            connect("channel:out_r", "master:bus_r"),
        ],
    );
    assert_eq!(dest_of(&d, "channel"), OutputDest::Mixed);
    // Swapped sides on an Audio I/O pair, and an odd start, are direct.
    run(
        &mut d,
        &r,
        [
            set_output("channel-2", OutputDest::Master),
            Command::Disconnect {
                to: sock("master:bus_l"),
            },
            Command::Disconnect {
                to: sock("master:bus_r"),
            },
        ],
    );
    run(
        &mut d,
        &r,
        [
            connect("channel-2:out_l", "interface:out_6"),
            connect("channel-2:out_r", "interface:out_5"),
        ],
    );
    assert!(matches!(dest_of(&d, "channel-2"), OutputDest::Direct(_)));
    run(
        &mut d,
        &r,
        [
            Command::Disconnect {
                to: sock("interface:out_5"),
            },
            Command::Disconnect {
                to: sock("interface:out_6"),
            },
            connect("channel-2:out_l", "interface:out_6"),
            connect("channel-2:out_r", "interface:out_7"),
        ],
    );
    assert!(
        matches!(dest_of(&d, "channel-2"), OutputDest::Direct(_)),
        "6+7 is not a pair"
    );
    run(
        &mut d,
        &r,
        [
            Command::Disconnect {
                to: sock("interface:out_6"),
            },
            Command::Disconnect {
                to: sock("interface:out_7"),
            },
        ],
    );
    assert_eq!(dest_of(&d, "channel-2"), OutputDest::None);
}

#[test]
fn a_channel_cabled_to_a_free_interface_pair_is_audio_io_not_direct() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            Command::Disconnect {
                to: sock("master:bus_l"),
            },
            Command::Disconnect {
                to: sock("master:bus_r"),
            },
            connect("channel:out_l", "interface:out_3"),
            connect("channel:out_r", "interface:out_4"),
        ],
    );
    assert_eq!(dest_of(&d, "channel"), OutputDest::AudioIo(2));
}

// ---- Insert on a bus ----------------------------------------------------------------------

#[test]
fn a_bus_has_the_channels_insert_chain() {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("A", OutputDest::Master),
            add(usize::MAX, DELAY, Some("fx")),
        ],
    );
    one_step(
        &mut d,
        &r,
        con(ConsoleCommand::SetInsert {
            channel: key("bus"),
            slot: 0,
            effect: Some(InsertEffect::Existing(key("fx"))),
        }),
    );
    assert_eq!(insert_chain(d.doc(), &r, &key("bus")), [key("fx")]);
    // The same effect cannot also sit on a channel.
    refused(
        &mut d,
        &r,
        con(ConsoleCommand::SetInsert {
            channel: key("channel"),
            slot: 0,
            effect: Some(InsertEffect::Existing(key("fx"))),
        }),
    );
}

// ---- Solo ---------------------------------------------------------------------------------

/// Channels `channel`..`channel-3`, buses `bus` (A) <- `bus-2` (B) <- channel, and `bus-3` (C)
/// on the master fed by channel-2; channel-3 on the master.
fn solo_project() -> (Document, Registry) {
    let (mut d, r) = project();
    run(
        &mut d,
        &r,
        [
            create("A", OutputDest::Master),
            create("B", bus("bus")),
            create("C", OutputDest::Master),
            set_output("channel", bus("bus-2")),
            set_output("channel-2", bus("bus-3")),
        ],
    );
    (d, r)
}

#[test]
fn nothing_is_silenced_without_a_solo_and_buses_are_listed_after_the_channels() {
    let (d, r) = solo_project();
    let list = silence(&d, &r);
    let keys: Vec<&str> = list.iter().map(|(k, _)| &**k).collect();
    assert_eq!(
        keys,
        ["channel", "channel-2", "channel-3", "bus", "bus-2", "bus-3"]
    );
    assert!(list.iter().all(|(_, s)| !s));
}

#[test]
fn soloing_a_channel_keeps_the_buses_on_its_path_audible() {
    let (mut d, r) = solo_project();
    d.apply(set_param("channel", "solo", 1.0), &r).unwrap();
    // channel -> B -> A -> master: B and A play; their other feeders do not exist; C and the
    // other channels are silenced.
    assert_eq!(silent(&d, &r), ["channel-2", "channel-3", "bus-3"]);
    d.apply(set_param("channel-3", "solo", 1.0), &r).unwrap();
    assert_eq!(silent(&d, &r), ["channel-2", "bus-3"]);
}

#[test]
fn soloing_a_bus_keeps_everything_that_feeds_it_and_the_buses_after_it_audible() {
    let (mut d, r) = solo_project();
    d.apply(set_param("bus", "solo", 1.0), &r).unwrap();
    // A is fed by B, which is fed by channel; channel-2 / C / channel-3 are silenced.
    assert_eq!(silent(&d, &r), ["channel-2", "channel-3", "bus-3"]);
    d.apply(set_param("bus", "solo", 0.0), &r).unwrap();
    d.apply(set_param("bus-2", "solo", 1.0), &r).unwrap();
    // B: its feeder channel and its path A stay; sibling feeders of A would be silenced.
    assert_eq!(silent(&d, &r), ["channel-2", "channel-3", "bus-3"]);
}

#[test]
fn a_sibling_feeder_of_a_bus_on_the_path_stays_silenced() {
    let (mut d, r) = solo_project();
    d.apply(set_output("channel-3", bus("bus")), &r).unwrap();
    d.apply(set_param("channel", "solo", 1.0), &r).unwrap();
    // channel-3 feeds A too, but only the buses on channel's path are kept, not their feeders.
    assert_eq!(silent(&d, &r), ["channel-2", "channel-3", "bus-3"]);
    // Soloing A (a bus) brings every feeder back, transitively.
    d.apply(set_param("channel", "solo", 0.0), &r).unwrap();
    d.apply(set_param("bus", "solo", 1.0), &r).unwrap();
    assert_eq!(silent(&d, &r), ["channel-2", "bus-3"]);
}

#[test]
fn the_solo_flag_of_a_bus_reports_a_console_change_and_returns_stay_solo_safe() {
    let (mut d, r) = solo_project();
    let ops = d.apply(set_param("bus", "solo", 1.0), &r).unwrap();
    assert!(
        ops.contains(&atomik_model::SyncOp::ConsoleChanged),
        "{ops:?}"
    );
    // Clearing the solos is about channels today; a bus solo is cleared by setting it off.
    assert!(d.doc().solo_silence(&r).iter().any(|(_, s)| *s));
}

#[test]
fn the_bus_stub_registers_the_contract_keys() {
    // The contract: the channel's params with the same keys, the channel's sends and outputs.
    assert_eq!(BUS_DESC.type_id, BUS_TYPE);
    let r = console_registry();
    let ch = r.get(CHANNEL).unwrap().descriptor;
    let b = r.get(BUS_TYPE).unwrap().descriptor;
    let keys = |d: &atomik_device_sdk::DeviceDescriptor| -> Vec<&str> {
        d.params.iter().map(|p| p.key).collect()
    };
    assert_eq!(keys(ch), keys(b));
    assert!(b.socket_by_key("bus_l").is_some() && b.socket_by_key("in_l").is_none());
}
